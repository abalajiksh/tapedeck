//! Navidrome source — talks both of Navidrome's APIs at once.
//!
//! Navidrome exposes two, and neither alone is enough:
//!
//! * **Native** (`/auth/login` → `x-nd-authorization`) is the only one that
//!   returns *historical* scrobbles, via `/api/scrobble/?from=<ts>`. That's what
//!   makes a real backfill possible rather than only catching live playback.
//!   The rows are just `{mediaFileId, submissionTime}`, so track details come
//!   from `/api/song`.
//! * **Subsonic** (`/rest/*`, salted-MD5 auth) is what everything else speaks.
//!   It answers `getNowPlaying`, and its `getArtist`/`getAlbum` carry MusicBrainz
//!   IDs and cover art — which is why Phase 4 can lean on a user's own library
//!   instead of the MusicBrainz API.
//!
//! Subsonic hashes `password + salt` per request, so the account password is
//! required rather than a token; it's held encrypted at rest like every other
//! stored credential. The native session token expires, so it's re-established
//! transparently on a 401.
//!
//! Ported from `temp/navidrome-navanalyze`, deliberately *without* its
//! `tls_danger_accept_invalid_certs(true)` — that disables certificate checking
//! entirely, which is not a trade a scrobbler should make on the user's behalf.

use super::egress::Egress;
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::any::Any;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, warn};

use crate::models::Play;
use crate::sources::MusicSource;

const SUBSONIC_API_VERSION: &str = "1.16.1";

/// Where the next history walk starts.
///
/// Pure and separate from the request for the reason `consume_sessions` is:
/// this decides whether a listen is ever seen at all, and a rule like that has
/// to be testable without a server. See `fetch_new_plays` for why it rewinds.
fn resume_point(last_submission: u64, last_checked: u64) -> u64 {
    last_submission.max(last_checked).saturating_sub(HISTORY_SLACK_SECS)
}

/// How far the history cursor is rewound on every poll.
///
/// Generous on purpose: the cost of overlapping is rows the dedup throws away,
/// and the cost of not overlapping is a listen nobody ever sees again. Well
/// clear of any plausible disagreement between Tapedeck's clock and the
/// server's, and small enough that a poll still reads one page.
/// Rows per request when walking history, and a ceiling on one walk so a first
/// run against a long history cannot make a single poll unbounded.
const HISTORY_PAGE_SIZE: usize = 5_000;
const HISTORY_MAX_ROWS: usize = 100_000;

/// Tracks per request when caching the library.
const SONG_PAGE_SIZE: usize = 5_000;

/// Ceiling on one catalogue load. Generous — a library past this is unusual —
/// but bounded, because this runs inside a poll tick.
const SONG_MAX_ROWS: usize = 500_000;

const HISTORY_SLACK_SECS: u64 = 300;

/// How many rows to ask for at a time when walking the library for ratings.
const RATING_PAGE_SIZE: usize = 1_000;

/// Where that walk gives up. A ceiling rather than an expectation.
const RATING_MAX_ITEMS: usize = 500_000;

pub struct NavidromeSource {
    url: String,
    username: String,
    password: String,
    client: Client,
    /// Native-API bearer, re-fetched on expiry.
    nd_token: Option<String>,
    /// mediaFileId → track details, filled lazily from `/api/song`.
    songs: HashMap<String, NavidromeSong>,
    /// Highest `submissionTime` already returned, so a poll only asks for what's
    /// new. Zero means "we haven't looked yet" and triggers the initial backfill.
    last_submission: u64,
}

#[derive(Debug, Clone)]
pub struct NavidromeSong {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<u64>,
    pub track_number: Option<i32>,
    pub mbid: Option<String>,
    /// Straight off the file's tags, via OpenSubsonic. Free and exact, so a
    /// listen carrying one never needs the rate-limited lookup that would
    /// otherwise find it.
    pub isrc: Option<String>,
}

// ── Native API wire types ──

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeScrobble {
    media_file_id: String,
    submission_time: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeSong {
    id: String,
    title: String,
    artist: String,
    album: Option<String>,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    track_number: Option<i32>,
    /// Navidrome has spelled this several ways across versions, and getting it
    /// wrong is silent: the MBID is simply absent and every listen pays for a
    /// MusicBrainz lookup it didn't need. Accept the known spellings.
    #[serde(
        default,
        alias = "mbzRecordingID",
        alias = "mbzRecordingId",
        alias = "mbzTrackId",
        alias = "mbzTrackID",
        alias = "musicBrainzId"
    )]
    mbz_recording_id: Option<String>,
    /// The recording's ISRC, straight off the file's tags.
    ///
    /// **The best source Tapedeck has for one.** Picard writes ISRC into the
    /// tag when it stamps a file, and OpenSubsonic surfaces it on the song — so
    /// a Navidrome listen can carry an exact, free identifier that no
    /// rate-limited MusicBrainz lookup was needed for. It is the user's own
    /// library answering, which is the rule `metadata/` already follows for
    /// artwork and portraits.
    ///
    /// **An array**, per the OpenSubsonic schema: a recording issued across
    /// territories carries several, and any of them identifies it. Older
    /// servers send nothing at all, hence `default` — and Plex and Jellyfin
    /// expose no ISRC field whatsoever, so this is the only source of its kind.
    #[serde(default)]
    isrc: Vec<String>,
}

// ── Subsonic wire types ──

#[derive(Deserialize)]
struct SubsonicEnvelope {
    #[serde(rename = "subsonic-response")]
    response: SubsonicResponse,
}

#[derive(Deserialize)]
struct SubsonicResponse {
    status: String,
    #[serde(default)]
    error: Option<SubsonicError>,
    #[serde(rename = "nowPlaying", default)]
    now_playing: Option<NowPlayingContainer>,
    #[serde(rename = "searchResult3", default)]
    search_result3: Option<SearchResult3>,
    /// `createPlaylist` answers with the playlist it made. Older Subsonic
    /// servers answer with a bare `ok` and nothing else, which is why this is
    /// optional rather than the id being required.
    #[serde(default)]
    playlist: Option<SubsonicPlaylist>,
}

#[derive(Deserialize)]
struct SubsonicError {
    message: Option<String>,
}

#[derive(Deserialize)]
struct SearchResult3 {
    #[serde(default)]
    song: Vec<SubsonicSong>,
    #[serde(default)]
    artist: Vec<SubsonicArtist>,
}

#[derive(Deserialize)]
struct SubsonicArtist {
    id: String,
    #[serde(default)]
    name: String,
    /// The id `getCoverArt` takes for the artist's picture. Navidrome spells it
    /// `ar-<id>`, and sends it here so nobody has to know that.
    #[serde(rename = "coverArt", default)]
    cover_art: Option<String>,
}

#[derive(Deserialize)]
struct SubsonicSong {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    album: Option<String>,
}

#[derive(Deserialize)]
struct SubsonicPlaylist {
    /// Subsonic ids are strings, but a few servers emit playlist ids as bare
    /// numbers — `serde_json::Value` accepts either and the caller only ever
    /// echoes it back to the user.
    #[serde(default)]
    id: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct NowPlayingContainer {
    #[serde(default)]
    entry: Vec<NowPlayingEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NowPlayingEntry {
    id: String,
    title: String,
    artist: String,
    album: Option<String>,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    track: Option<i32>,
    /// Which Navidrome account is playing — used to keep a shared server's other
    /// users out of this source's owner's history.
    #[serde(default)]
    username: Option<String>,
    /// Seconds since this entry was last updated by the player.
    #[serde(default)]
    minutes_ago: Option<u64>,
}

/// One live track, already attributed to the account that's playing it.
#[derive(Debug, Clone)]
pub struct NavidromeNowPlaying {
    pub play: Play,
    pub username: Option<String>,
    pub minutes_ago: u64,
}

impl NavidromeSource {
    pub fn new(url: String, username: String, password: String, egress: Egress) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            username,
            password,
            client: egress.client(Duration::from_secs(15)),
            nd_token: None,
            songs: HashMap::new(),
            last_submission: 0,
        }
    }

    /// Resume from a known high-water mark so a restart doesn't re-walk history.
    ///
    /// See `fetch_new_plays` for why the mark is rewound rather than used as-is.
    pub fn resume_from(&mut self, submission_time: u64) {
        self.last_submission = submission_time;
    }

    pub fn last_submission(&self) -> u64 {
        self.last_submission
    }

    /// The account these credentials belong to. Unlike Plex there's no lookup
    /// needed — you log in *as* the user, so the username is the identity.
    pub fn username_owned(&self) -> String {
        self.username.clone()
    }

    /// Log in and confirm both APIs answer. Also the connection test.
    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.login().await?;
        self.subsonic_ping().await?;
        Ok(())
    }

    // ── Native API ──

    async fn login(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let resp = self
            .client
            .post(format!("{}/auth/login", self.url))
            .json(&serde_json::json!({ "username": self.username, "password": self.password }))
            .send()
            .await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err("Navidrome rejected those credentials".into());
        }
        if !resp.status().is_success() {
            return Err(format!("Navidrome login returned {}", resp.status()).into());
        }
        let body: LoginResponse = resp.json().await?;
        self.nd_token = Some(body.token);
        Ok(())
    }

    /// GET against the native API, logging in again once if the token expired.
    async fn native_get(
        &mut self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<reqwest::Response, Box<dyn std::error::Error>> {
        for attempt in 0..2 {
            if self.nd_token.is_none() {
                self.login().await?;
            }
            let token = self.nd_token.clone().unwrap_or_default();
            let resp = self
                .client
                .get(format!("{}{}", self.url, path))
                .header("x-nd-authorization", format!("Bearer {token}"))
                .query(query)
                .send()
                .await?;

            if resp.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                debug!("Navidrome native token expired; re-authenticating");
                self.nd_token = None;
                continue;
            }
            if !resp.status().is_success() {
                return Err(format!("Navidrome {} returned {}", path, resp.status()).into());
            }
            return Ok(resp);
        }
        Err("Navidrome authentication kept failing".into())
    }

    /// Scrobbles submitted after `from`. This is the backfill: Navidrome keeps
    /// its own play history, so a new Tapedeck install can pull years of it.
    pub async fn fetch_scrobbles(
        &mut self,
        from: u64,
    ) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        // Paged, because a single `_end=10000` is only safe under an ordering
        // Navidrome does not promise. If its history comes back **newest
        // first**, one capped request advances `last_submission` to the very
        // newest row and everything behind the cap is skipped for good — a
        // first run against years of history losing most of it, silently. If it
        // comes back oldest first the cap self-paces and nothing is lost. Under
        // either ordering, reading until a short page is correct.
        //
        // It costs one extra request in steady state and nothing at all in the
        // common case, since `resume_point` already keeps each poll's window
        // small.
        let mut rows: Vec<NativeScrobble> = Vec::new();
        let mut start = 0usize;
        loop {
            let resp = self
                .native_get(
                    "/api/scrobble/",
                    &[
                        ("from", from.to_string()),
                        ("_start", start.to_string()),
                        ("_end", (start + HISTORY_PAGE_SIZE).to_string()),
                    ],
                )
                .await?;
            let page: Vec<NativeScrobble> = resp.json().await?;
            let got = page.len();
            rows.extend(page);
            if got < HISTORY_PAGE_SIZE {
                break;
            }
            start += got;
            if start >= HISTORY_MAX_ROWS {
                warn!(
                    "Navidrome history walk stopped at its {}-row ceiling — the rest will be \
                     picked up on the next poll if the server orders oldest-first, and not at \
                     all if it does not",
                    HISTORY_MAX_ROWS
                );
                break;
            }
        }
        if rows.is_empty() {
            return Ok(Vec::new());
        }

        // Only load the catalogue when we actually have rows referencing it.
        if rows.iter().any(|r| !self.songs.contains_key(&r.media_file_id)) {
            self.load_songs().await?;
        }

        let mut plays = Vec::new();
        let mut newest = self.last_submission;
        for row in rows {
            newest = newest.max(row.submission_time);
            let Some(song) = self.songs.get(&row.media_file_id) else {
                // A listen is being **dropped** here, not merely degraded — so
                // this is a warning. After a full catalogue load an unknown id
                // means the track has been removed from the library since it
                // was played, which is real and rare; before the pagination fix
                // it just meant the library was bigger than one request.
                warn!(
                    "Navidrome play of unknown track {} — not banking it, since there is \
                     nothing to bank",
                    row.media_file_id
                );
                continue;
            };
            plays.push(song.to_play(row.submission_time, &row.media_file_id));
        }
        self.last_submission = newest;
        Ok(plays)
    }

    /// Cache the library, a page at a time.
    ///
    /// **It used to ask for `_end=100000` in one request**, which is two
    /// separate problems on a library bigger than that. The obvious one is that
    /// the tail is missing. The one that actually costs listens is what happens
    /// next: `fetch_scrobbles` looks a play's `media_file_id` up in this map and
    /// **`continue`s past anything it cannot find** — so a truncated catalogue
    /// does not degrade a listen's metadata, it drops the listen entirely, at
    /// `debug!`, permanently.
    ///
    /// It is also a single response holding an entire music library, parsed in
    /// one go on a Raspberry Pi. Pages are a fixed size and appended, so peak
    /// memory is one page rather than the whole thing twice.
    async fn load_songs(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut songs: Vec<NativeSong> = Vec::new();
        let mut start = 0usize;
        loop {
            let resp = self
                .native_get(
                    "/api/song",
                    &[
                        ("_start", start.to_string()),
                        ("_end", (start + SONG_PAGE_SIZE).to_string()),
                    ],
                )
                .await?;
            let page: Vec<NativeSong> = resp.json().await?;
            let got = page.len();
            songs.extend(page);
            if got < SONG_PAGE_SIZE {
                break;
            }
            start += got;
            if start >= SONG_MAX_ROWS {
                // Said out loud, because the consequence is dropped listens
                // rather than a cosmetic gap.
                warn!(
                    "Navidrome library exceeds {} tracks — plays of anything past that will \
                     not be recognised and will be skipped",
                    SONG_MAX_ROWS
                );
                break;
            }
        }
        self.songs = songs
            .into_iter()
            .map(|s| {
                (
                    s.id.clone(),
                    NavidromeSong {
                        title: s.title,
                        artist: s.artist,
                        album: s.album,
                        duration: (s.duration > 0.0).then_some(s.duration as u64),
                        track_number: s.track_number.filter(|n| *n > 0),
                        mbid: s.mbz_recording_id.filter(|m| !m.is_empty()),
                        // First of the array: several ISRCs on one recording
                        // are the same performance issued across territories,
                        // and each identifies it.
                        isrc: s
                            .isrc
                            .into_iter()
                            .map(|v| v.trim().to_uppercase())
                            .find(|v| !v.is_empty()),
                    },
                )
            })
            .collect();
        debug!("Navidrome: cached {} songs", self.songs.len());
        Ok(())
    }

    // ── Subsonic API ──

    /// Salted-MD5 auth, computed per request as the protocol requires.
    fn subsonic_params(&self) -> Vec<(String, String)> {
        use rand::Rng;
        let salt: String =
            (0..12).map(|_| rand::rng().sample(rand::distr::Alphanumeric) as char).collect();
        let token = format!("{:x}", md5::compute(format!("{}{}", self.password, salt)));
        vec![
            ("u".into(), self.username.clone()),
            ("t".into(), token),
            ("s".into(), salt),
            ("v".into(), SUBSONIC_API_VERSION.into()),
            ("c".into(), env!("CARGO_PKG_NAME").into()),
            ("f".into(), "json".into()),
        ]
    }

    async fn subsonic_get(
        &self,
        endpoint: &str,
    ) -> Result<SubsonicResponse, Box<dyn std::error::Error>> {
        self.subsonic_get_with(endpoint, &[]).await
    }

    /// The same call with extra query parameters.
    ///
    /// Takes a slice of pairs rather than a map because Subsonic repeats a key
    /// to pass a list — `createPlaylist` wants one `songId` per track, in order,
    /// and a map would collapse them to one.
    async fn subsonic_get_with(
        &self,
        endpoint: &str,
        extra: &[(String, String)],
    ) -> Result<SubsonicResponse, Box<dyn std::error::Error>> {
        let mut params = self.subsonic_params();
        params.extend_from_slice(extra);
        let resp = self
            .client
            .get(format!("{}/rest/{}", self.url, endpoint))
            .query(&params)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Subsonic {} returned {}", endpoint, resp.status()).into());
        }
        let env: SubsonicEnvelope = resp.json().await?;
        if env.response.status != "ok" {
            let msg = env
                .response
                .error
                .and_then(|e| e.message)
                .unwrap_or_else(|| "unknown Subsonic error".into());
            return Err(format!("Subsonic {endpoint}: {msg}").into());
        }
        Ok(env.response)
    }

    async fn subsonic_ping(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.subsonic_get("ping").await.map(|_| ())
    }

    /// Every rated or starred item in the library.
    ///
    /// **Subsonic's `userRating` is out of five**, whole stars, and `starred`
    /// is a separate binary — which is why the two travel apart here. A star is
    /// already the statement a love makes and takes no threshold; see
    /// [`crate::ratings::LoveRule`].
    ///
    /// Reads the **native** API rather than Subsonic, for the reason
    /// `fetch_scrobbles` does: stock Subsonic has no "list everything that is
    /// rated" call at all — `getStarred2` covers stars and nothing covers
    /// numbers — while Navidrome's own `/api/song`, `/api/album` and
    /// `/api/artist` carry `rating` and `starred` on every row. That makes this
    /// Navidrome-specific in the same way the history walk already is.
    ///
    /// It walks the whole library and filters here rather than asking the
    /// server to filter. A sort or a filter hint would only help if this then
    /// stopped early on it — and stopping early on a hint a server quietly
    /// ignored is how a walk silently under-reports, which is the one failure
    /// this cannot afford. Paging to a short page cannot be wrong, and this is
    /// a manual action against a box on the user's own network.
    pub async fn fetch_ratings(
        &mut self,
    ) -> Result<Vec<crate::db::IncomingRating>, Box<dyn std::error::Error>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Rated {
            id: String,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            artist: Option<String>,
            #[serde(default)]
            album_artist: Option<String>,
            #[serde(default)]
            album: Option<String>,
            #[serde(default)]
            rating: Option<f32>,
            #[serde(default)]
            starred: bool,
            #[serde(default)]
            starred_at: Option<String>,
            /// Same spelling drift as `NativeSong` — a miss here is silent and
            /// costs a MusicBrainz lookup that need not have happened.
            #[serde(
                default,
                alias = "mbzRecordingID",
                alias = "mbzRecordingId",
                alias = "mbzTrackId",
                alias = "mbzTrackID",
                alias = "mbzAlbumId",
                alias = "mbzAlbumID",
                alias = "mbzArtistId",
                alias = "mbzArtistID",
                alias = "musicBrainzId"
            )]
            mbz_id: Option<String>,
        }

        let mut out: Vec<crate::db::IncomingRating> = Vec::new();
        for (path, kind) in
            [("/api/song", "recording"), ("/api/album", "release"), ("/api/artist", "artist")]
        {
            let mut start = 0usize;
            loop {
                let resp = self
                    .native_get(
                        path,
                        &[
                            ("_start", start.to_string()),
                            ("_end", (start + RATING_PAGE_SIZE).to_string()),
                        ],
                    )
                    .await?;
                let rows: Vec<Rated> = resp.json().await?;
                let n = rows.len();

                for r in rows {
                    let rating = r.rating.filter(|v| *v > 0.0);
                    if rating.is_none() && !r.starred {
                        continue;
                    }
                    // `/api/song` names the track in `title`; `/api/album` and
                    // `/api/artist` name theirs in `name`.
                    let Some(name) =
                        r.title.or(r.name).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
                    else {
                        continue;
                    };
                    let artist = match kind {
                        "artist" => String::new(),
                        _ => r
                            .artist
                            .filter(|s| !s.trim().is_empty())
                            .or(r.album_artist)
                            .unwrap_or_default(),
                    };
                    if kind != "artist" && artist.trim().is_empty() {
                        continue;
                    }

                    out.push(crate::db::IncomingRating {
                        kind: kind.to_string(),
                        remote_id: r.id,
                        artist,
                        name,
                        album: if kind == "recording" { r.album } else { None },
                        mbid: r.mbz_id.filter(|m| !m.trim().is_empty()),
                        rating: rating.map(|v| {
                            crate::ratings::Rating::new(v, crate::ratings::SourceScale::SUBSONIC)
                        }),
                        starred: r.starred,
                        rated_at: r.starred_at.as_deref().and_then(parse_iso8601),
                    });
                }

                if n < RATING_PAGE_SIZE {
                    break;
                }
                start += n;
                if start >= RATING_MAX_ITEMS {
                    debug!("Navidrome: stopped walking {} for ratings at {}", path, start);
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Tracks matching a free-text query, as candidates for a playlist push.
    ///
    /// `search3` is OpenSubsonic's own search, so this works against anything
    /// speaking the protocol and not only Navidrome. Album and artist counts are
    /// zeroed: this is looking for songs, and asking for the other two costs the
    /// server work whose results are thrown away.
    pub async fn search_tracks(
        &self,
        query: &str,
    ) -> Result<Vec<crate::playlists::Candidate>, Box<dyn std::error::Error + Send + Sync>> {
        let resp = self
            .subsonic_get_with(
                "search3",
                &[
                    ("query".into(), query.to_string()),
                    ("songCount".into(), "20".into()),
                    ("artistCount".into(), "0".into()),
                    ("albumCount".into(), "0".into()),
                ],
            )
            .await
            .map_err(stringify)?;

        Ok(resp
            .search_result3
            .map(|r| r.song)
            .unwrap_or_default()
            .into_iter()
            .map(|s| crate::playlists::Candidate {
                id: s.id,
                title: s.title.unwrap_or_default(),
                artist: s.artist.unwrap_or_default(),
                album: s.album,
            })
            .collect())
    }

    /// Create a playlist holding `song_ids`, in that order.
    pub async fn create_playlist(
        &self,
        name: &str,
        song_ids: &[String],
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if song_ids.is_empty() {
            return Err("Nothing to put in the playlist".into());
        }
        let mut params = vec![("name".to_string(), name.to_string())];
        params.extend(song_ids.iter().map(|id| ("songId".to_string(), id.clone())));

        let resp = self.subsonic_get_with("createPlaylist", &params).await.map_err(stringify)?;

        // A server that answers a bare `ok` really did create it — the id is
        // only ever shown back to the user, so reporting "created" without one
        // beats failing a push that worked.
        Ok(resp
            .playlist
            .and_then(|p| p.id)
            .map(|v| match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
            .unwrap_or_default())
    }

    /// Cover art bytes for one library item.
    ///
    /// Streamed through Tapedeck rather than linked directly: the browser can't
    /// reach a LAN Navidrome, and it must never be handed the credentials that
    /// would let it. Returns the bytes plus the content type so the proxy can
    /// pass both through.
    pub async fn fetch_cover_art(
        &self,
        item_id: &str,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error + Send + Sync>> {
        let mut params = self.subsonic_params();
        params.push(("id".into(), item_id.to_string()));
        params.push(("size".into(), "500".into()));

        let resp =
            self.client.get(format!("{}/rest/getCoverArt", self.url)).query(&params).send().await?;

        if !resp.status().is_success() {
            return Err(format!("Navidrome cover art returned {}", resp.status()).into());
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        // Subsonic reports "no artwork" as a JSON error body with HTTP 200, so a
        // non-image response means there's nothing to show.
        if content_type.contains("json") || content_type.contains("xml") {
            return Err("no cover art for that item".into());
        }
        Ok((resp.bytes().await?.to_vec(), content_type))
    }

    /// The cover id of an artist's picture, found by name.
    ///
    /// `search3` with only artists asked for, checked with `artist_matches` the
    /// way Plex's and Jellyfin's searches are — a search hit is a suggestion,
    /// not an identification. Falls back to Navidrome's `ar-<id>` spelling
    /// when a server sends no `coverArt`.
    ///
    /// **Not verified against a live server**; the field names are
    /// OpenSubsonic's `ArtistID3`.
    pub async fn artist_cover_id(
        &self,
        name: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        let resp = self
            .subsonic_get_with(
                "search3",
                &[
                    ("query".into(), name.to_string()),
                    ("artistCount".into(), "10".into()),
                    ("albumCount".into(), "0".into()),
                    ("songCount".into(), "0".into()),
                ],
            )
            .await
            .map_err(stringify)?;
        Ok(resp
            .search_result3
            .map(|r| r.artist)
            .unwrap_or_default()
            .into_iter()
            .find(|a| crate::sources::plex::artist_matches(&a.name, name))
            .map(|a| a.cover_art.unwrap_or_else(|| format!("ar-{}", a.id))))
    }

    /// What's playing right now, per account.
    ///
    /// Navidrome keeps reporting a finished track for a while, so entries that
    /// haven't been touched in a few minutes are dropped — otherwise the deck
    /// would show a stale song long after playback stopped.
    pub async fn fetch_now_playing(
        &self,
    ) -> Result<Vec<NavidromeNowPlaying>, Box<dyn std::error::Error>> {
        let resp = self.subsonic_get("getNowPlaying").await?;
        let entries = resp.now_playing.map(|n| n.entry).unwrap_or_default();

        Ok(entries
            .into_iter()
            .filter(|e| e.minutes_ago.unwrap_or(0) < 5)
            .map(|e| {
                let minutes_ago = e.minutes_ago.unwrap_or(0);
                NavidromeNowPlaying {
                    play: Play {
                        source_id: format!("navidrome-{}-np", e.id),
                        source_name: "navidrome".into(),
                        title: e.title,
                        artist: e.artist,
                        album: e.album,
                        timestamp: now_secs(),
                        duration: e.duration,
                        track_number: e.track,
                        mbid_recording: None,
                        mbid_release: None,
                        mbid_artist: None,
                        artists: None,
                        mbid_release_group: None,
                        mbid_release_track: None,
                        mbid_work: None,
                        mbid_album_artist: None,
                        caa_id: None,
                        caa_release_mbid: None,
                        isrc: None,
                    },
                    username: e.username,
                    minutes_ago,
                }
            })
            .collect())
    }
}

impl NavidromeSong {
    fn to_play(&self, submission_time: u64, media_file_id: &str) -> Play {
        Play {
            // Keyed by the Navidrome id *and* the submission time: the same track
            // played twice is two listens, and re-polling the same window must
            // not create a third.
            source_id: format!("navidrome-{}-{}", media_file_id, submission_time),
            source_name: "navidrome".into(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            timestamp: submission_time,
            duration: self.duration,
            track_number: self.track_number,
            mbid_recording: self.mbid.clone(),
            mbid_release: None,
            mbid_artist: None,
            artists: None,
            mbid_release_group: None,
            mbid_release_track: None,
            mbid_work: None,
            mbid_album_artist: None,
            caa_id: None,
            caa_release_mbid: None,
            isrc: self.isrc.clone(),
        }
    }
}

/// Re-box a plain error as a `Send + Sync` one.
///
/// The Subsonic helpers predate any caller that needed to be held across an
/// await in a `Send` future, so they return a bare `Box<dyn Error>`. Flattening
/// to the message keeps the playlist path spawnable without rewriting the
/// polling code underneath it.
fn stringify(e: Box<dyn std::error::Error>) -> Box<dyn std::error::Error + Send + Sync> {
    e.to_string().into()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

#[async_trait]
impl MusicSource for NavidromeSource {
    fn name(&self) -> &str {
        "Navidrome"
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    async fn fetch_new_plays(
        &mut self,
        last_checked: u64,
    ) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        // **Rewound, not resumed exactly.** Asking from the newest thing we
        // have seen loses anything that lands on or just under that boundary
        // between two polls: `last_checked` is Tapedeck's wall clock while
        // `submission_time` is Navidrome's, so they need not agree, and two
        // scrobbles inside one second are ordinary. A miss here is silent —
        // the listen is simply never fetched again, because the mark has
        // already moved past it.
        //
        // Overlapping instead is free, and provably so rather than hopefully:
        // `to_play` derives `source_id` from `(submission_time, media_file_id)`
        // and nothing else, so re-reading a row yields the identical id and
        // `save_scrobble`'s uniqueness absorbs it —
        // `source_id_separates_repeat_plays_of_one_track` pins exactly that.
        //
        // The same trade rocksky's jetstream consumer makes on reconnect, for
        // the same reason: prefer a few redundant rows to a silent hole.
        let from = resume_point(self.last_submission, last_checked);
        match self.fetch_scrobbles(from).await {
            Ok(plays) => Ok(plays),
            Err(e) => {
                warn!("Navidrome scrobble fetch failed: {}", e);
                Err(e)
            }
        }
    }
}

/// When something was starred, out of whatever Navidrome felt like sending.
///
/// Accepts an RFC 3339 string *or* bare epoch seconds, because the native API's
/// timestamp spellings are not uniform — `submissionTime` on a scrobble is a
/// number and `starredAt` is a string — and neither is documented. A value it
/// cannot read is `None` rather than a guess: this is the date a star was
/// pressed, it decorates a row and decides nothing, and inventing one would be
/// worse than leaving it blank.
fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

#[cfg(test)]
mod tests {

    /// `search3` asked only for artists, in OpenSubsonic's `ArtistID3` shape.
    #[test]
    fn an_artist_search_carries_the_cover_id() {
        let env: SubsonicEnvelope = serde_json::from_value(serde_json::json!({
            "subsonic-response": { "status": "ok", "version": "1.16.1",
                "searchResult3": { "artist": [
                    { "id": "2f8a", "name": "Arca", "coverArt": "ar-2f8a_0", "albumCount": 4 },
                    { "id": "9b1c", "name": "Arcade Fire" }
                ]}
            }
        }))
        .unwrap();
        let artists = env.response.search_result3.unwrap().artist;
        assert_eq!(artists[0].cover_art.as_deref(), Some("ar-2f8a_0"));
        assert_eq!(artists[1].cover_art, None);
        assert_eq!(artists[1].name, "Arcade Fire");
    }
    #[test]
    fn a_star_date_is_read_from_either_spelling_and_never_guessed() {
        // The native API is not uniform about this — `submissionTime` on a
        // scrobble is a number where `starredAt` is a string — and neither is
        // documented, so both are accepted.
        assert_eq!(super::parse_iso8601("2026-09-13T10:04:00Z"), Some(1789293840));
        assert_eq!(super::parse_iso8601("1789293840"), Some(1789293840));
        // And anything else is blank rather than invented. This date decorates
        // a row and decides nothing, so a wrong one is strictly worse than
        // none.
        assert_eq!(super::parse_iso8601("last Tuesday"), None);
        assert_eq!(super::parse_iso8601(""), None);
        assert_eq!(super::parse_iso8601("   "), None);
    }

    use super::*;

    fn song() -> NavidromeSong {
        NavidromeSong {
            title: "Xtal".into(),
            artist: "Aphex Twin".into(),
            album: Some("Selected Ambient Works".into()),
            duration: Some(293),
            track_number: Some(1),
            mbid: Some("mbid-xtal".into()),
            isrc: None,
        }
    }

    /// The cursor rewinds, and never underflows past the epoch.
    ///
    /// Phrased about the *hole* rather than the arithmetic: a scrobble landing
    /// on the boundary between two polls used to be skipped for good, because
    /// the mark had already moved past it and nothing looks back.
    #[test]
    fn the_history_cursor_looks_back_far_enough_to_close_the_gap() {
        // The later of the two marks is what leads, then it steps back.
        assert_eq!(resume_point(1_000_000, 900_000), 1_000_000 - HISTORY_SLACK_SECS);
        assert_eq!(resume_point(900_000, 1_000_000), 1_000_000 - HISTORY_SLACK_SECS);

        // A listen submitted at the exact moment of the previous poll — the one
        // that used to vanish — is inside the window now.
        let polled_at = 1_700_000_000;
        assert!(resume_point(polled_at, polled_at) < polled_at);

        // And a fresh source with no marks asks for everything rather than
        // wrapping around to the end of time.
        assert_eq!(resume_point(0, 0), 0);
        assert_eq!(resume_point(0, 60), 0);
    }

    #[test]
    fn source_id_separates_repeat_plays_of_one_track() {
        let s = song();
        let first = s.to_play(1700000000, "abc");
        let second = s.to_play(1700000400, "abc");
        // Same track, two listens — they must not dedup into one.
        assert_ne!(first.source_id, second.source_id);
        // …but re-polling the same window yields the identical id, so the
        // existing (user, source_id, source_name) uniqueness absorbs it.
        assert_eq!(first.source_id, s.to_play(1700000000, "abc").source_id);
    }

    #[test]
    fn carries_navidrome_metadata_onto_the_play() {
        let p = song().to_play(1700000000, "abc");
        assert_eq!(p.timestamp, 1700000000);
        assert_eq!(p.duration, Some(293));
        assert_eq!(p.track_number, Some(1));
        // A library MBID means this listen never needs a MusicBrainz lookup.
        assert_eq!(p.mbid_recording.as_deref(), Some("mbid-xtal"));
        assert_eq!(p.source_name, "navidrome");
    }

    #[test]
    fn subsonic_params_are_salted_per_call() {
        let src = NavidromeSource::new("http://nd".into(), "u".into(), "pw".into(), Egress::Open);
        let a = src.subsonic_params();
        let b = src.subsonic_params();
        let salt = |p: &Vec<(String, String)>| p.iter().find(|(k, _)| k == "s").unwrap().1.clone();
        let tok = |p: &Vec<(String, String)>| p.iter().find(|(k, _)| k == "t").unwrap().1.clone();
        // A fixed salt would make the token a replayable password equivalent.
        assert_ne!(salt(&a), salt(&b));
        assert_ne!(tok(&a), tok(&b));
        // The password itself is never a query parameter.
        assert!(a.iter().all(|(_, v)| v != "pw"));
    }

    #[test]
    fn accepts_navidrome_mbid_field_spellings() {
        // Each of these has been the field name at some point; a mismatch is
        // invisible at runtime, so pin them all.
        for key in ["mbzRecordingID", "mbzRecordingId", "mbzTrackId", "musicBrainzId"] {
            let json = format!(
                r#"{{"id":"1","title":"T","artist":"A","duration":100.0,"{key}":"mb-123"}}"#
            );
            let song: NativeSong = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{key} failed to parse: {e}"));
            assert_eq!(song.mbz_recording_id.as_deref(), Some("mb-123"), "{key} did not map");
        }
    }

    /// Picard writes the ISRC into the tag and OpenSubsonic surfaces it, so a
    /// Navidrome listen can carry an exact identifier that cost nothing.
    ///
    /// It is an **array** in the schema, which is the shape a reasonable parser
    /// gets wrong — a bare `Option<String>` would fail the whole song rather
    /// than the field. Pinned against the spec's own example.
    #[test]
    fn a_song_carries_the_isrc_from_its_tags() {
        let song: NativeSong = serde_json::from_str(
            r#"{"id":"1","title":"Polar expedition","artist":"A","album":"B",
                "isrc":["USSM18300073","DELV42300297"]}"#,
        )
        .expect("parses");
        assert_eq!(song.isrc, vec!["USSM18300073", "DELV42300297"]);

        // A server that predates the field sends nothing, which is not an error.
        let older: NativeSong =
            serde_json::from_str(r#"{"id":"1","title":"t","artist":"a"}"#).expect("parses");
        assert!(older.isrc.is_empty());
    }

    #[test]
    fn url_trailing_slash_is_normalised() {
        let src =
            NavidromeSource::new("http://nd:4533/".into(), "u".into(), "p".into(), Egress::Open);
        assert_eq!(src.url, "http://nd:4533");
    }
}
