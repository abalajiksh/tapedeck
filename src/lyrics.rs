//! Lyrics from [LRCLIB](https://lrclib.net), cached beside the other
//! third-party metadata.
//!
//! **Primarily a language signal**, and that is what decides the design.
//! `language::script_language` resolves any non-Latin script with certainty and
//! gives up on Latin text — so a *transliterated* title ("Bam Haneul-ui Jeo
//! Byeolcheoreom") falls through to `DEFAULT_LANGUAGE`, which is 45% of a real
//! window and is a shrug drawn next to real categories. The elegant part, and
//! the reason this is worth a network call at all: **the same detector works
//! unchanged on the lyric text**, where a romanised Korean title has Hangul
//! lyrics.
//!
//! **They live in the MusicBrainz cache database, not in `scrobbles.db`.** Same
//! reasoning as the Every Noise map: this is somebody else's copyrighted text
//! rather than the user's listening, and `GET /api/v1/backup` is a
//! `VACUUM INTO` of the listening database — which is exactly what should not
//! carry a cache of third-party lyrics. That placement is the whole of how the
//! "never redistributed" property is enforced, rather than a rule somebody has
//! to remember at each export.
//!
//! ## What the API asks of a client, and it asks politely
//!
//! No key, no registration, and in return three things the docs state as
//! requirements rather than suggestions:
//!
//! * **Identify yourself** in `User-Agent` — name, version and a link.
//! * **Honour `Retry-After` on a 429.** Ignoring it "may result in a temporary
//!   ban", which would take the feature away from every Tapedeck instance
//!   rather than only this one.
//! * **Sequential requests with a short gap** (200–500ms), which the backfill's
//!   own budget already enforces at a far slower rate.
//!
//! A **404 is a normal answer**, not a failure: LRCLIB says a missing track may
//! be picked up by its own background fetcher and appear later, so a miss is a
//! tombstone with a TTL rather than a permanent verdict.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sqlx::{Pool, Sqlite};

/// How long a miss is believed before asking again.
///
/// LRCLIB fills in gaps on its own, so "nobody has this" is a fact about today.
/// Long enough not to re-ask a large history often; short enough that a track
/// that gains lyrics is picked up within a month.
const MISS_TTL_SECS: i64 = 30 * 24 * 3600;

/// The gap between requests.
///
/// The docs ask for 200–500ms and for requests to be made sequentially. The
/// backfill's budget makes the real rate far slower than this, so for that path
/// it is a floor rather than the pacing — but the backfill is no longer the only
/// caller. `/api/v1/lyrics/fetch` puts a **button** in front of this, and a
/// button can be pressed twice in a second, so the gap has to be enforced
/// *across* callers rather than by each one sleeping on its own.
///
/// See [`LyricsClient::pace`]: the sleep is taken under a lock, so two
/// concurrent callers start their requests at least this far apart instead of
/// both sleeping 350ms and then firing together.
const REQUEST_GAP: Duration = Duration::from_millis(350);

/// Bounded like every other outbound call made from inside this process.
const TIMEOUT: Duration = Duration::from_secs(15);

/// A 429 tells us how long to wait; this caps how long we will believe it
/// inside one backfill pass. A longer wait is honoured by simply stopping —
/// the next tick tries again — rather than by sleeping through it and holding
/// the budget.
const MAX_WAIT_SECS: u64 = 30;

/// What LRCLIB returns for a track. Only the fields Tapedeck reads.
/// Several fields here are read only by `the_response_shape_is_the_one_the_api_documents`,
/// and that is the point of keeping them: a wrong camelCase spelling is
/// *silent* — the field simply stays `None` for ever, which is the class of bug
/// that has cost this codebase a whole audio-quality record once already. The
/// test is the thing that fails when LRCLIB renames something.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct LrcResponse {
    pub id: Option<i64>,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub duration: Option<f64>,
    /// **A real third state, not a missing value.** An instrumental has no
    /// lyrics *and no language*, which the vocabulary could not previously
    /// express — every track was either classified or shrugged at.
    #[serde(default)]
    pub instrumental: bool,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
}

/// What a lookup concluded, which is three states rather than two.
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// Lyrics, and whatever language they are written in.
    Lyrics { plain: Option<String>, synced: Option<String>, language: Option<String> },
    /// LRCLIB knows the track and says it has no words.
    Instrumental,
    /// LRCLIB has never heard of it. Believed for `MISS_TTL_SECS`.
    Missing,
}

/// Strip the `[mm:ss.xx]` cues out of a synced lyric.
///
/// Only ever used to *read* a synced lyric as text when there is no plain one.
/// The cues are ASCII digits, so they cannot fool the script detector — but
/// they are noise in anything that displays the text, and stripping them here
/// means one definition rather than one per reader.
pub fn strip_cues(synced: &str) -> String {
    synced
        .lines()
        .map(|line| {
            let mut rest = line;
            // A line can carry several cues; only a leading run is a cue.
            while rest.starts_with('[') {
                match rest.find(']') {
                    Some(end) => {
                        let inside = &rest[1..end];
                        // A cue is digits, colons and dots. Anything else is a
                        // metadata tag (`[ar: …]`) or part of the words.
                        if !inside.is_empty()
                            && inside.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '.')
                        {
                            rest = rest[end + 1..].trim_start();
                        } else {
                            break;
                        }
                    }
                    None => break,
                }
            }
            rest
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// One numbered line of a lyric.
///
/// **The index is the whole point.** A note refers to a line by its number, not
/// by its words, so correcting a fetched lyric later corrects every quote with
/// it rather than orphaning them — the same call `notes.rs` already makes for
/// entities, and the first of the four rules the design states.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    /// 1-based, and **blank lines are numbered**. A stanza break is a line, or
    /// renumbering breaks every reference already written — the design's second
    /// rule, and it has to be true of the parse rather than only of the
    /// renderer.
    pub index: usize,
    /// Seconds into the track, when the lyric is timed.
    pub at: Option<f64>,
    /// The words. Empty for a stanza break, which is a real line.
    pub text: String,
}

/// Split a lyric into numbered lines, preferring the timed form.
///
/// The synced lyric is preferred when there is one, because it carries the
/// timestamps *and* the same words — taking the plain one and trying to pair
/// them up afterwards would be inventing an alignment nobody measured.
pub fn parse_lines(plain: Option<&str>, synced: Option<&str>) -> Vec<LyricLine> {
    if let Some(sy) = synced.filter(|s| !s.trim().is_empty()) {
        return sy
            .lines()
            .enumerate()
            .map(|(i, raw)| {
                let (at, text) = split_cue(raw);
                LyricLine { index: i + 1, at, text }
            })
            .collect();
    }
    plain
        .filter(|p| !p.trim().is_empty())
        .map(|p| {
            p.lines()
                .enumerate()
                .map(|(i, l)| LyricLine { index: i + 1, at: None, text: l.trim_end().to_string() })
                .collect()
        })
        .unwrap_or_default()
}

/// `[01:04.20] the words` → `(Some(64.2), "the words")`.
///
/// A line whose leading bracket is not a cue keeps it: `[Chorus]` is a lyric in
/// a great many songs, and eating it would quietly rewrite the words.
fn split_cue(raw: &str) -> (Option<f64>, String) {
    let line = raw.trim_end();
    let Some(rest) = line.strip_prefix('[') else { return (None, line.trim().to_string()) };
    let Some(end) = rest.find(']') else { return (None, line.trim().to_string()) };
    let inside = &rest[..end];
    let mut parts = inside.split(':');
    let (Some(m), Some(s)) = (parts.next(), parts.next()) else {
        return (None, line.trim().to_string());
    };
    if parts.next().is_some() {
        return (None, line.trim().to_string());
    }
    let (Ok(mins), Ok(secs)) = (m.trim().parse::<f64>(), s.trim().parse::<f64>()) else {
        return (None, line.trim().to_string());
    };
    (Some(mins * 60.0 + secs), rest[end + 1..].trim().to_string())
}

/// The language of a lyric, by the script it is mostly written in.
///
/// **By weight, not by presence**, which is the difference between this and the
/// detector the titles use. `script_language` asks "does this script appear",
/// and on a title — a handful of words — that is decisive. On a lyric it is
/// not: measured against the live API, a Korean lyric is 88% Hangul while an
/// English one quoting a single foreign name is under 1%, and an existence test
/// calls both of them Korean. `dominant_script` counts.
///
/// Latin text still resolves to `None`. Telling French from English needs a
/// statistical detector, which is its own dependency and its own decision; the
/// bucket this exists to empty is *transliterated* non-Latin, where the title
/// gives nothing away and the lyric gives everything.
pub fn language_of(plain: Option<&str>, synced: Option<&str>) -> Option<&'static str> {
    let text = match (plain, synced) {
        (Some(p), _) if !p.trim().is_empty() => p.to_string(),
        (_, Some(s)) if !s.trim().is_empty() => strip_cues(s),
        _ => return None,
    };
    crate::language::dominant_script(&text)
}

/// Reads lyrics, and remembers what it read.
pub struct LyricsClient {
    http: reqwest::Client,
    pool: Pool<Sqlite>,
    base: String,
    /// When the last outbound request was *started*, so [`Self::pace`] can
    /// space the next one. `Mutex` rather than an atomic because the wait is
    /// taken while holding it — that is the whole mechanism.
    last_request: tokio::sync::Mutex<Option<Instant>>,
}

impl LyricsClient {
    pub async fn new(pool: Pool<Sqlite>) -> Result<Self, Box<dyn std::error::Error>> {
        // The docs require a name, a version and a link. A project URL rather
        // than an address, so this needs no configuration and cannot ship a
        // made-up contact — the objection the Every Noise scraper answers by
        // refusing to start without one.
        let ua =
            format!("Tapedeck/{} (https://codeberg.org/abksh/tapedeck)", env!("CARGO_PKG_VERSION"));
        let http = reqwest::Client::builder().user_agent(ua).timeout(TIMEOUT).build()?;
        let base = std::env::var("TAPEDECK_LRCLIB_URL")
            .unwrap_or_else(|_| "https://lrclib.net".to_string());
        let c = LyricsClient { http, pool, base, last_request: tokio::sync::Mutex::new(None) };
        c.init().await?;
        Ok(c)
    }

    async fn init(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS lyrics_cache (
                track_key TEXT PRIMARY KEY,   -- artist_key + '|' + folded title
                lrclib_id INTEGER,
                instrumental INTEGER NOT NULL DEFAULT 0,
                plain TEXT,
                synced TEXT,
                language TEXT,                -- by script, NULL for Latin text
                found INTEGER NOT NULL,       -- 0 = LRCLIB has nothing (yet)
                fetched_at INTEGER NOT NULL,
                edited INTEGER NOT NULL DEFAULT 0,  -- corrected here; a fetch must not overwrite
                source TEXT                        -- who these words came from
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        // A cache that predates "Fix the words" has no `edited` column, and
        // this table is created by `CREATE TABLE IF NOT EXISTS` so the line
        // above never reaches it. Unguarded and ignored on failure: the only
        // way this errors is the column already being there.
        let _ =
            sqlx::query("ALTER TABLE lyrics_cache ADD COLUMN edited INTEGER NOT NULL DEFAULT 0")
                .execute(&self.pool)
                .await;
        // Same again for provenance. A row written before this column existed
        // came from LRCLIB, because that was the only source — but it is left
        // NULL rather than backfilled to say so, since "we did not record it"
        // and "we know it was LRCLIB" are different claims and the reader can
        // say the first honestly.
        let _ = sqlx::query("ALTER TABLE lyrics_cache ADD COLUMN source TEXT")
            .execute(&self.pool)
            .await;
        Ok(())
    }

    /// Replace the words by hand, and stop asking LRCLIB for them.
    ///
    /// **A value you supplied is never overwritten by a lookup** — the rule the
    /// shelf's running times already follow, and the answer to the collision
    /// between this and *newest fetch wins*: a later fetch is newer, and a
    /// correction somebody typed is better. `edited` is what carries that, and
    /// [`Self::remember`] refuses to write over a row holding it.
    ///
    /// The language is re-read from the new words, because correcting a lyric
    /// is exactly the case where the old reading may have been wrong — a
    /// placeholder in the wrong script is precisely what somebody would be
    /// fixing.
    ///
    /// Instance-wide rather than per user, like everything else in this cache:
    /// these are one track's words, not one person's opinion of them.
    pub async fn set_words(
        &self,
        artist: &str,
        title: &str,
        plain: Option<&str>,
        synced: Option<&str>,
    ) -> Result<Found, sqlx::Error> {
        let plain = plain.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        let synced = synced.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        let language = language_of(plain.as_deref(), synced.as_deref()).map(str::to_string);
        let key = Self::track_key(artist, title);
        let now = crate::db::Database::now_secs();
        // `found` is 1 even for an empty correction: somebody has answered for
        // this track, and re-asking LRCLIB would undo the answer.
        sqlx::query(
            "INSERT INTO lyrics_cache
                (track_key, lrclib_id, instrumental, plain, synced, language, found, fetched_at, edited, source)
             VALUES (?, NULL, 0, ?, ?, ?, 1, ?, 1, 'You')
             ON CONFLICT(track_key) DO UPDATE SET
                plain = excluded.plain, synced = excluded.synced,
                language = excluded.language, instrumental = 0, found = 1,
                fetched_at = excluded.fetched_at, edited = 1,
                -- The words are yours now. Leaving the old provenance would
                -- have the reader credit a server for something typed here,
                -- which is a small lie about whose text it is.
                source = 'You'",
        )
        .bind(&key)
        .bind(&plain)
        .bind(&synced)
        .bind(&language)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(Found::Lyrics { plain, synced, language })
    }

    /// Store words that came out of the user's own library.
    ///
    /// **Not `set_words`**, which is for a correction and sets `edited`. These
    /// are fetched, so a later fetch may legitimately replace them — what must
    /// not happen is a fetch of any kind overwriting something a person typed,
    /// which is why this refuses a row holding `edited` exactly as
    /// [`Self::remember`] does.
    ///
    /// `source` is recorded rather than assumed: the reader says where a lyric
    /// came from, and "your Plex" is a materially different statement from
    /// "LRCLIB" — one of them is the pressing you actually played.
    pub async fn remember_library(
        &self,
        artist: &str,
        title: &str,
        plain: Option<String>,
        synced: Option<String>,
        source: &str,
    ) -> Result<Found, sqlx::Error> {
        let plain = plain.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let synced = synced.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let language = language_of(plain.as_deref(), synced.as_deref()).map(str::to_string);
        let found = Found::Lyrics {
            plain: plain.clone(),
            synced: synced.clone(),
            language: language.clone(),
        };
        if self.is_edited(artist, title).await {
            return Ok(found);
        }
        let key = Self::track_key(artist, title);
        let now = crate::db::Database::now_secs();
        sqlx::query(
            "INSERT INTO lyrics_cache
                (track_key, lrclib_id, instrumental, plain, synced, language, found, fetched_at, edited, source)
             VALUES (?, NULL, 0, ?, ?, ?, 1, ?, 0, ?)
             ON CONFLICT(track_key) DO UPDATE SET
                plain = excluded.plain, synced = excluded.synced,
                language = excluded.language, instrumental = 0, found = 1,
                fetched_at = excluded.fetched_at, source = excluded.source
              WHERE lyrics_cache.edited = 0",
        )
        .bind(&key)
        .bind(&plain)
        .bind(&synced)
        .bind(&language)
        .bind(now)
        .bind(source)
        .execute(&self.pool)
        .await?;
        Ok(found)
    }

    /// Where a track's stored words came from, for the line the reader prints
    /// under them.
    ///
    /// `None` for a row written before provenance was recorded. Reported as
    /// unknown rather than assumed to be LRCLIB: it almost certainly was, but
    /// "we did not record it" is the true statement and the reader can make it.
    pub async fn source_of(&self, artist: &str, title: &str) -> Option<String> {
        let key = Self::track_key(artist, title);
        sqlx::query_as::<_, (Option<String>,)>(
            "SELECT source FROM lyrics_cache WHERE track_key = ?",
        )
        .bind(&key)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.0)
    }

    /// Whether these words were corrected here rather than fetched.
    pub async fn is_edited(&self, artist: &str, title: &str) -> bool {
        let key = Self::track_key(artist, title);
        sqlx::query_as::<_, (i64,)>("SELECT edited FROM lyrics_cache WHERE track_key = ?")
            .bind(&key)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .is_some_and(|(e,)| e != 0)
    }

    /// What is known about each of a handful of tracks, in one query.
    ///
    /// For the reader's tracklist, which wants to mark the instrumentals — the
    /// design's `· instrumental` — and dim the ones there is nothing to open.
    /// One `IN` rather than a lookup per track: a record is ten to twenty
    /// tracks and twenty round trips to decorate a sidebar is not the way.
    ///
    /// Three answers, matching [`Found`]: `"words"`, `"instrumental"`, and
    /// absent for a track nobody has asked about — which is a different thing
    /// from one that has no words, and the sidebar says so differently.
    pub async fn states_for(&self, tracks: &[(String, String)]) -> HashMap<String, &'static str> {
        let mut out = HashMap::new();
        if tracks.is_empty() {
            return out;
        }
        let keys: Vec<String> = tracks.iter().map(|(a, t)| Self::track_key(a, t)).collect();
        let holes = std::iter::repeat_n("?", keys.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT track_key, instrumental,
                    (COALESCE(plain, '') <> '' OR COALESCE(synced, '') <> '') AS has_words
               FROM lyrics_cache WHERE found = 1 AND track_key IN ({holes})"
        );
        let mut q = sqlx::query_as::<_, (String, i64, i64)>(&sql);
        for k in &keys {
            q = q.bind(k);
        }
        for (key, instrumental, has_words) in q.fetch_all(&self.pool).await.unwrap_or_default() {
            if instrumental != 0 {
                out.insert(key, "instrumental");
            } else if has_words != 0 {
                out.insert(key, "words");
            }
        }
        out
    }

    /// Every track this instance holds words for, as folded track keys.
    ///
    /// One request for the whole set rather than a lookup per row, which is
    /// exactly the shape `GET /api/v1/loves/keys` already has and for the same
    /// reason: a history page wants to know which of fifty rows has something
    /// to read, and fifty questions is not the way to ask.
    ///
    /// **Only tracks with words.** An instrumental is a real answer and a
    /// finished lookup, but there is nothing to open a reader on, so offering
    /// one would be offering an empty page.
    pub async fn keys_with_lyrics(&self) -> Vec<String> {
        sqlx::query_as::<_, (String,)>(
            "SELECT track_key FROM lyrics_cache
              WHERE found = 1 AND instrumental = 0
                AND (COALESCE(plain, '') <> '' OR COALESCE(synced, '') <> '')",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(k,)| k)
        .collect()
    }

    /// `artist|title`, folded the way the rest of the application folds both.
    ///
    /// `Database::artist_key` for the artist so a lyric found for "Cœur de
    /// pirate" is not looked up again for "Coeur de pirate", and a plain
    /// trim-lowercase for the title, which is what every track grouping uses.
    pub fn track_key(artist: &str, title: &str) -> String {
        format!("{}|{}", crate::db::Database::artist_key(artist), title.trim().to_lowercase())
    }

    /// What is already known, if it is still worth believing.
    pub async fn cached(&self, artist: &str, title: &str) -> Option<Found> {
        let key = Self::track_key(artist, title);
        /// `(found, instrumental, plain, synced, language, fetched_at)`.
        type CachedRow = (i64, i64, Option<String>, Option<String>, Option<String>, i64);
        let row: Option<CachedRow> = sqlx::query_as(
            "SELECT found, instrumental, plain, synced, language, fetched_at
                   FROM lyrics_cache WHERE track_key = ?",
        )
        .bind(&key)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        let (found, instrumental, plain, synced, language, fetched_at) = row?;
        if found == 0 {
            // A miss is believed for a while and then re-asked: LRCLIB fills
            // its own gaps in the background, so "nobody has this" is a fact
            // about the day it was asked.
            let age = crate::db::Database::now_secs() - fetched_at;
            return (age < MISS_TTL_SECS).then_some(Found::Missing);
        }
        if instrumental != 0 {
            return Some(Found::Instrumental);
        }
        Some(Found::Lyrics { plain, synced, language })
    }

    /// Ask LRCLIB, and remember the answer.
    ///
    /// `duration` is optional and strongly recommended: LRCLIB matches it
    /// within ±2s and will otherwise hand back a different recording of the
    /// same title. A listen whose source reported no length still gets a
    /// lookup, and the match is simply less certain — which is the same trade
    /// the playlist push makes when it searches by name.
    /// Wait our turn, and space this request from the last one.
    ///
    /// **The wait is taken under the lock**, which is the only part that
    /// matters: a bare `sleep(REQUEST_GAP)` per caller means two concurrent
    /// callers both sleep and then fire simultaneously, which is exactly what
    /// LRCLIB asks clients not to do. Holding it means the second caller starts
    /// its wait where the first one finished.
    ///
    /// This was a per-call sleep while the backfill was the only caller and it
    /// was sequential, so nothing could overlap. `/api/v1/lyrics/fetch` ended
    /// that: a person can press a button twice, and a bounded rate that depends
    /// on nobody clicking quickly is not a bounded rate.
    async fn pace(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(prev) = *last {
            let since = prev.elapsed();
            if since < REQUEST_GAP {
                tokio::time::sleep(REQUEST_GAP - since).await;
            }
        } else {
            // Nothing to space from, but still the documented courtesy gap
            // before the first request of a run.
            tokio::time::sleep(REQUEST_GAP).await;
        }
        *last = Some(Instant::now());
    }

    pub async fn fetch(
        &self,
        artist: &str,
        title: &str,
        album: Option<&str>,
        duration: Option<i64>,
    ) -> Result<Found, LyricsError> {
        let mut url = format!(
            "{}/api/get?artist_name={}&track_name={}",
            self.base,
            urlencoding::encode(artist.trim()),
            urlencoding::encode(title.trim())
        );
        if let Some(a) = album.map(str::trim).filter(|a| !a.is_empty()) {
            url.push_str(&format!("&album_name={}", urlencoding::encode(a)));
        }
        // The API rejects anything outside 1–3600.
        if let Some(d) = duration.filter(|d| (1..=3600).contains(d)) {
            url.push_str(&format!("&duration={d}"));
        }

        self.pace().await;
        let resp =
            self.http.get(&url).send().await.map_err(|e| LyricsError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            self.remember(artist, title, None).await;
            return Ok(Found::Missing);
        }
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            // **Honoured, not ignored.** The docs say a client that keeps going
            // may be banned — which would cost every Tapedeck instance the
            // feature, not only this one. Nothing is cached: we learned nothing
            // about the track.
            let wait = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(5);
            return Err(LyricsError::RateLimited(wait.min(MAX_WAIT_SECS)));
        }
        if !resp.status().is_success() {
            return Err(LyricsError::Network(format!("LRCLIB returned {}", resp.status())));
        }

        let body: LrcResponse =
            resp.json().await.map_err(|e| LyricsError::Network(e.to_string()))?;
        let found = if body.instrumental {
            Found::Instrumental
        } else {
            let language = language_of(body.plain_lyrics.as_deref(), body.synced_lyrics.as_deref())
                .map(str::to_string);
            Found::Lyrics { plain: body.plain_lyrics, synced: body.synced_lyrics, language }
        };
        self.remember(artist, title, Some((&found, body.id))).await;
        Ok(found)
    }

    /// What else LRCLIB holds for this track.
    ///
    /// **Not a version history**, which is what the design's `LRCLIB v3 ▾`
    /// label implies and what LRCLIB does not have: `/api/get` returns one
    /// record and there is no history behind it. `/api/search` returns up to
    /// twenty *different records* — other uploads, other pressings, a live
    /// take filed under the same name — and picking a better one is the real
    /// need underneath that control. So it is offered as what it is.
    ///
    /// Candidates with no words are dropped rather than shown: the whole
    /// gesture is "these are the wrong words, give me the other ones", and an
    /// instrumental record cannot answer it. Adopting one goes through the
    /// ordinary correction path, so it is marked `edited` and survives every
    /// later fetch — and it reports the line count either side, because a
    /// reference is an index and a different record renumbers every quote.
    pub async fn search(&self, artist: &str, title: &str) -> Result<Vec<LrcResponse>, LyricsError> {
        let url = format!(
            "{}/api/search?artist_name={}&track_name={}",
            self.base,
            urlencoding::encode(artist.trim()),
            urlencoding::encode(title.trim())
        );

        self.pace().await;
        let resp =
            self.http.get(&url).send().await.map_err(|e| LyricsError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(5);
            return Err(LyricsError::RateLimited(wait.min(MAX_WAIT_SECS)));
        }
        // A search that finds nothing is a 200 with an empty array; a 404 here
        // would be the endpoint being wrong, not the track being unknown.
        if !resp.status().is_success() {
            return Err(LyricsError::Network(format!("LRCLIB returned {}", resp.status())));
        }

        let body: Vec<LrcResponse> =
            resp.json().await.map_err(|e| LyricsError::Network(e.to_string()))?;
        Ok(body
            .into_iter()
            .filter(|r| {
                !r.instrumental
                    && (r.plain_lyrics.as_deref().is_some_and(|t| !t.trim().is_empty())
                        || r.synced_lyrics.as_deref().is_some_and(|t| !t.trim().is_empty()))
            })
            .collect())
    }

    async fn remember(&self, artist: &str, title: &str, hit: Option<(&Found, Option<i64>)>) {
        let key = Self::track_key(artist, title);
        // **A correction outlives every later fetch.** *Newest fetch wins* is
        // the rule for two fetches; between a fetch and something a person
        // typed, the person wins — the same trade `apply_durations` makes on
        // the shelf, where a running time somebody supplied survives every
        // lookup after it.
        if self.is_edited(artist, title).await {
            return;
        }
        let now = crate::db::Database::now_secs();
        let (found, instrumental, plain, synced, language, id) = match hit {
            None => (0, 0, None, None, None, None),
            Some((Found::Instrumental, id)) => (1, 1, None, None, None, id),
            Some((Found::Lyrics { plain, synced, language }, id)) => {
                (1, 0, plain.clone(), synced.clone(), language.clone(), id)
            }
            Some((Found::Missing, _)) => (0, 0, None, None, None, None),
        };
        // **Say who these words came from.** This wrote no `source` at all when
        // the column landed in v0.104.0 — only the library path and a hand
        // correction did — so every lyric fetched from LRCLIB since then read
        // as *"where these words came from was not recorded"*, and a row whose
        // library words LRCLIB later displaced with cues kept the **old
        // server's name** over somebody else's text. Both are the reader
        // crediting the wrong party, which is the one thing provenance exists
        // to stop.
        //
        // NULL on a miss: nothing was stored, so there is nobody to credit.
        let source = if found == 1 { Some("LRCLIB") } else { None };
        let _ = sqlx::query(
            "INSERT INTO lyrics_cache
                (track_key, lrclib_id, instrumental, plain, synced, language, found, fetched_at, source)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(track_key) DO UPDATE SET
                lrclib_id = excluded.lrclib_id, instrumental = excluded.instrumental,
                plain = excluded.plain, synced = excluded.synced,
                language = excluded.language, found = excluded.found,
                fetched_at = excluded.fetched_at, source = excluded.source",
        )
        .bind(&key)
        .bind(id)
        .bind(instrumental)
        .bind(plain)
        .bind(synced)
        .bind(language)
        .bind(found)
        .bind(now)
        .bind(source)
        .execute(&self.pool)
        .await;
    }

    /// How much of the library has been asked about, for the progress readout.
    ///
    /// `None` means the cache could not be **read**, which is not the same
    /// thing as a cache with nothing in it and must never be reported as one.
    /// This returned `(0, 0, 0)` for both until the Settings card gave it a
    /// reader, at which point a broken cache would have said "0 tracks asked
    /// about" in as many words — a failed read rendered as a measurement, the
    /// shape `needs_setup` took down the instance with in v0.81.0.
    ///
    /// The `COALESCE` is not load-bearing today and is kept deliberately.
    /// `SUM` over zero rows is NULL rather than 0, and this driver was
    /// **measured** decoding a NULL into a bare `i64` as `0` rather than
    /// refusing it — so an empty table happens to come back `(0, 0, 0)` on its
    /// own. That is undocumented driver behaviour to be leaning on: were it
    /// ever to become the decode error it looks like it should be, an empty
    /// cache would start reporting as an unreadable one, which is precisely
    /// the distinction above collapsing again in the other direction. The
    /// `COALESCE` makes the SQL say what it means and removes the dependency.
    pub async fn coverage(&self) -> Option<(i64, i64, i64)> {
        sqlx::query_as(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN found = 1 AND instrumental = 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN instrumental = 1 THEN 1 ELSE 0 END), 0)
               FROM lyrics_cache",
        )
        // An aggregate with no GROUP BY always returns exactly one row, so
        // `fetch_one` is the honest shape here — and this is a read, not one
        // of the `RETURNING` writes that must not be consumed this way.
        .fetch_one(&self.pool)
        .await
        .map_err(|e| tracing::warn!("🎤 Lyrics cache could not be read: {e}"))
        .ok()
    }
}

#[derive(Debug)]
pub enum LyricsError {
    /// Wait this many seconds. The backfill stops for the rest of its pass
    /// rather than sleeping through it and holding its budget open.
    RateLimited(u64),
    Network(String),
}

impl std::fmt::Display for LyricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LyricsError::RateLimited(s) => write!(f, "LRCLIB asked us to wait {s}s"),
            LyricsError::Network(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn testpool() -> Pool<Sqlite> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("td-lyrics-{}-{}.db", std::process::id(), n));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new().filename(&path).create_if_missing(true),
            )
            .await
            .unwrap()
    }

    /// Words carry who they came from, and a miss carries nobody.
    ///
    /// `source` landed with the library tier and only the library path wrote
    /// it, so **everything fetched from LRCLIB read as "not recorded"** — and a
    /// row whose plain library words LRCLIB later displaced with cues kept the
    /// *server's* name over somebody else's text. Nothing errors either way;
    /// the reader simply credits the wrong party under words that are not ours,
    /// which is the whole reason the column exists.
    #[tokio::test]
    async fn a_fetched_lyric_says_where_it_came_from() {
        let c = LyricsClient {
            http: reqwest::Client::new(),
            pool: testpool().await,
            base: "http://127.0.0.1:1".into(),
            last_request: tokio::sync::Mutex::new(None),
        };
        c.init().await.unwrap();

        let found = Found::Lyrics { plain: Some("a line".into()), synced: None, language: None };
        c.remember("Massive Attack", "Teardrop", Some((&found, Some(7)))).await;
        assert_eq!(c.source_of("Massive Attack", "Teardrop").await.as_deref(), Some("LRCLIB"));

        // A library lyric names the server it came off, and LRCLIB not having
        // displaced it must not rewrite that.
        c.remember_library("Burial", "Archangel", Some("words".into()), None, "Plex")
            .await
            .unwrap();
        assert_eq!(c.source_of("Burial", "Archangel").await.as_deref(), Some("Plex"));

        // Nothing found is nobody to credit, not "LRCLIB said nothing".
        c.remember("Nobody", "Nothing", None).await;
        assert_eq!(c.source_of("Nobody", "Nothing").await, None);
    }

    /// **Two callers do not get to fire at the same instant.**
    ///
    /// LRCLIB asks for requests to be made sequentially, 200–500ms apart, and
    /// ignoring that "may result in a temporary ban" — which would cost every
    /// Tapedeck instance the feature rather than only this one. That was safe
    /// for free while the backfill was the sole caller and ran a track at a
    /// time; `/api/v1/lyrics/fetch` put a **button** in front of it, and a
    /// button can be pressed twice.
    ///
    /// The tempting spelling — `sleep(REQUEST_GAP)` at the top of each call —
    /// looks identical in review and enforces nothing: two callers both sleep
    /// 350ms and then fire together. The sleep has to be taken *under the
    /// lock*, which is what this asserts, and it fails against that spelling
    /// with a gap of about zero.
    #[tokio::test]
    async fn two_lookups_at_once_are_still_spaced_apart() {
        let c = std::sync::Arc::new(LyricsClient {
            http: reqwest::Client::new(),
            pool: testpool().await,
            base: "http://127.0.0.1:1".into(),
            last_request: tokio::sync::Mutex::new(None),
        });

        let (a, b) = (c.clone(), c.clone());
        let one = tokio::spawn(async move {
            a.pace().await;
            Instant::now()
        });
        let two = tokio::spawn(async move {
            b.pace().await;
            Instant::now()
        });
        let (x, y) = (one.await.unwrap(), two.await.unwrap());

        let gap = if x > y { x - y } else { y - x };
        assert!(
            gap >= REQUEST_GAP,
            "two concurrent lookups started {gap:?} apart, which is inside the gap LRCLIB asks for"
        );
    }

    /// A cache nobody has filled yet is an answer; one that cannot be read is not.
    ///
    /// These were the *same* value — `(0, 0, 0)` — until the Settings card gave
    /// `coverage` a reader, and then a broken cache would have reported "0
    /// tracks asked about" as a finding. Never report a failed read as an
    /// absence; the instance-wide version of this mistake is what made v0.80.0
    /// tell its owner their history was gone.
    ///
    /// The empty half is asserted in its own right rather than as a guard on
    /// the `COALESCE`: it pins that a cache with nothing in it is an *answer*,
    /// so this cannot later be "tidied" into returning `None` for zero rows —
    /// which would make every fresh instance report itself broken.
    #[tokio::test]
    async fn an_unreadable_lyrics_cache_is_not_an_empty_one() {
        let pool = testpool().await;
        let c = LyricsClient::new(pool.clone()).await.unwrap();

        assert_eq!(
            c.coverage().await,
            Some((0, 0, 0)),
            "an empty cache reports zeros rather than failing to be read"
        );

        let now = 1_700_000_000i64;
        for (key, instrumental, found) in
            [("a|one", 0, 1), ("a|two", 0, 1), ("a|three", 1, 1), ("a|four", 0, 0)]
        {
            sqlx::query(
                "INSERT INTO lyrics_cache (track_key, instrumental, found, fetched_at)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(key)
            .bind(instrumental)
            .bind(found)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        assert_eq!(
            c.coverage().await,
            Some((4, 2, 1)),
            "asked, with words, instrumental — the fourth is one LRCLIB does not have"
        );

        // The table going away stands in for any unreadable cache. The point is
        // the *shape* of the answer, not this particular cause.
        sqlx::query("DROP TABLE lyrics_cache").execute(&pool).await.unwrap();
        assert_eq!(
            c.coverage().await,
            None,
            "a cache that cannot be read must not read as a cache with nothing in it"
        );
    }

    /// Blank lines are numbered, because a reference is an index.
    ///
    /// The design's second rule, and it has to hold in the *parse*: a stanza
    /// break that is skipped renumbers every line after it, which silently
    /// re-points every quote already written. This is the assertion that fails
    /// if somebody filters empties out for tidiness.
    #[test]
    fn a_stanza_break_is_a_numbered_line() {
        let plain = "first\n\nthird";
        let lines = parse_lines(Some(plain), None);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1], LyricLine { index: 2, at: None, text: String::new() });
        assert_eq!(lines[2].index, 3, "the line after a break keeps its number");
    }

    /// Timed lyrics carry their cue as a number, and keep the words.
    #[test]
    fn a_timed_lyric_is_numbered_and_timed() {
        let synced = "[00:17.12] first line\n[01:04.20] second line";
        let lines = parse_lines(None, Some(synced));
        assert_eq!(lines[0].at, Some(17.12));
        assert_eq!(lines[0].text, "first line");
        assert_eq!(lines[1].at, Some(64.2));
        assert_eq!(lines[1].index, 2);
    }

    /// A bracket that is not a cue is part of the song.
    #[test]
    fn a_section_heading_is_not_eaten_as_a_timestamp() {
        let lines = parse_lines(None, Some("[Chorus]\n[00:10.00] words"));
        assert_eq!(lines[0].text, "[Chorus]", "a section heading is a line of the lyric");
        assert_eq!(lines[0].at, None);
        assert_eq!(lines[1].at, Some(10.0));
    }

    /// The timed form wins when there is one — it has the same words *and* the
    /// cues, so pairing a plain lyric against it would invent an alignment.
    #[test]
    fn the_timed_form_is_preferred_and_an_absent_lyric_is_no_lines() {
        let both = parse_lines(Some("plain"), Some("[00:01.00] timed"));
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].text, "timed");
        assert!(parse_lines(None, None).is_empty());
        assert!(parse_lines(Some("   "), None).is_empty());
    }

    /// The cues come off, and only the cues.
    ///
    /// A leading `[…]` that is not digits is a metadata tag or part of the
    /// words — `[Chorus]` is a lyric line in a great many songs, and eating it
    /// would quietly rewrite the text.
    #[test]
    fn stripping_cues_leaves_the_words_and_the_brackets_that_are_words() {
        let synced = "[00:17.12] first line\n[00:21.40] second line\n[Chorus]\n[ar: Somebody]";
        let out = strip_cues(synced);
        assert_eq!(out, "first line\nsecond line\n[Chorus]\n[ar: Somebody]");
        // Nothing to strip is not a reason to change anything.
        assert_eq!(strip_cues("plain line"), "plain line");
        assert_eq!(strip_cues(""), "");
    }

    /// The point of the whole path: a romanised title has native-script lyrics.
    ///
    /// This is the case `script_language` cannot reach from the title, and it
    /// is 45% of a real listening window sitting in `English/Other`.
    fn korean(n: usize) -> String {
        "\u{BC24}\u{D558}\u{B298}".repeat(n)
    }

    #[test]
    fn a_transliterated_title_is_resolved_by_the_script_of_its_lyrics() {
        // The title gives nothing away — Latin script, so the detector passes.
        assert_eq!(crate::language::script_language("Bam Haneului Jeo Byeolcheoreom", None), None);
        // The lyric does. Measured on the real thing: "Heize — Star" comes back
        // with a romanised title and artist and 504 Hangul characters to 69
        // Latin, which is the whole case this path exists for.
        assert_eq!(language_of(Some(&korean(40)), None), Some("Korean"));
        // And through a synced lyric, cues and all.
        assert_eq!(language_of(None, Some(&format!("[00:12.00] {}", korean(40)))), Some("Korean"));
    }

    /// One borrowed word does not make a song Korean.
    ///
    /// The reason `language_of` counts instead of reusing the title detector: a
    /// lyric is a thousand characters, and an existence test on a block is
    /// flipped by a single quoted name. This is the assertion that fails if
    /// somebody "simplifies" it back to `script_language`.
    #[test]
    fn a_loanword_does_not_decide_the_language_of_a_lyric() {
        let mostly_english =
            format!("{} {}", "we were young and it was summer ".repeat(12), korean(1));
        assert_eq!(language_of(Some(&mostly_english), None), None);
        // Ten times as much Hangul, and it is a Korean song with an English
        // chorus rather than the other way round.
        let mostly_korean = format!("{} {}", korean(40), "and I know ".repeat(3));
        assert_eq!(language_of(Some(&mostly_korean), None), Some("Korean"));
    }

    /// Latin lyrics stay unresolved rather than being guessed at.
    ///
    /// Telling French from English needs a statistical detector, which is its
    /// own dependency and its own decision. Returning `None` leaves the listen
    /// where it was instead of asserting a language nothing measured.
    #[test]
    fn latin_lyrics_are_left_unresolved_rather_than_guessed() {
        assert_eq!(language_of(Some("we were young and it was summer"), None), None);
        assert_eq!(language_of(Some("   "), None), None, "whitespace is not a lyric");
        assert_eq!(language_of(None, None), None);
    }

    /// The key folds case and whitespace, and **deliberately not accents**.
    ///
    /// `Database::artist_key` is the trim-and-lowercase fold, not
    /// `normalise_artist` — the codebase has one artist-identity function and
    /// this is not the place to reach for the aggressive one. The trade is
    /// asymmetric: an accent variant costs a *second lookup*, which is one
    /// request for a spelling the sanitiser exists to merge anyway, whereas
    /// over-folding would serve one artist's words under another's name.
    #[test]
    fn the_track_key_folds_case_and_space_but_never_accents() {
        assert_eq!(
            LyricsClient::track_key("Coeur de pirate", "Adieu"),
            LyricsClient::track_key("COEUR DE PIRATE", "  adieu  ")
        );
        assert_ne!(
            LyricsClient::track_key("Cœur de pirate", "Adieu"),
            LyricsClient::track_key("Coeur de pirate", "Adieu"),
            "an accent is a different key, and a second lookup is the cheap side"
        );
        assert_ne!(
            LyricsClient::track_key("Burial", "Archangel"),
            LyricsClient::track_key("Burial", "Shell of Light")
        );
        // The **literal shape**, because `GET /api/v1/lyrics/keys` hands this
        // string to the browser and `lyricKey` in `web/src/lib/api.ts` builds
        // the same one from a row in hand. Nothing links the two, so a change
        // to the separator or the fold here silently stops "Read the words"
        // appearing on rows that do have words. Same standing trap as
        // `loveKey`, and this is the half of it that can be pinned.
        assert_eq!(LyricsClient::track_key("Burial", "Archangel"), "burial|archangel");
    }

    /// `instrumental` is an answer, not an absence.
    #[test]
    fn an_instrumental_is_its_own_state() {
        let r: LrcResponse = serde_json::from_str(
            r#"{"id":1,"trackName":"Untitled","artistName":"X","duration":200,
                "instrumental":true,"plainLyrics":null,"syncedLyrics":null}"#,
        )
        .unwrap();
        assert!(r.instrumental);
        assert_eq!(language_of(r.plain_lyrics.as_deref(), r.synced_lyrics.as_deref()), None);
    }

    /// The camelCase spellings are what LRCLIB actually sends, and a wrong one
    /// is silent — the field simply stays `None` forever.
    #[test]
    fn the_response_shape_is_the_one_the_api_documents() {
        let r: LrcResponse = serde_json::from_str(
            r#"{"id":3396226,"name":"I Want to Live","trackName":"I Want to Live",
                "artistName":"Borislav Slavov","albumName":"Baldur's Gate 3",
                "duration":233,"instrumental":false,
                "plainLyrics":"a line","syncedLyrics":"[00:17.12] a line"}"#,
        )
        .unwrap();
        assert_eq!(r.id, Some(3396226));
        assert_eq!(r.track_name.as_deref(), Some("I Want to Live"));
        assert_eq!(r.artist_name.as_deref(), Some("Borislav Slavov"));
        assert_eq!(r.duration, Some(233.0));
        assert!(r.plain_lyrics.is_some() && r.synced_lyrics.is_some());
    }
}
