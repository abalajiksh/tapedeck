//! MusicBrainz credits: who a recording credits, applied to every listen of it.
//!
//! **MusicBrainz IDs are the first source of truth** for who is credited on a
//! listen (Ashwin, 2026-11-03). A recording MBID names one recording, and
//! MusicBrainz's credit for it names everyone on it under their own MBIDs —
//! "Cœur de pirate avec Loud", where every source sent only "Cœur de pirate".
//!
//! Two guards stand between that answer and a stored listen, because a
//! recording MBID can itself be a guess (ListenBrainz's mapping, or Tapedeck's
//! own name search, picks the wrong recording sometimes — "Boyz II Men, Mariah
//! Carey" mapped to Boyz II Men and an orchestra):
//!
//! * the credit must **account for everyone the source named** — it may add
//!   people the source left out (Loud), never contradict one it did name
//!   ([`credit_extends`]);
//! * the listen's own artist MBIDs, where it has any, must all be in the
//!   credit.
//!
//! A listen that fails either is marked `refused` and left exactly as it was.
//! Every rewrite keeps what it replaced (`artist_display`,
//! `previous_artist_credit`), so none is irreversible.

use super::*;
use crate::musicbrainz::{CreditedArtist, RecordingCredits};
use std::collections::HashMap;

/// The fewest letters a tightened name needs before
/// `unify_punctuation_spellings` will fold on it: acronyms are short, and
/// "M.I.A." is not "Mia".
const TIGHT_MIN: usize = 5;

/// A key with its spacing and punctuation gone: "a. r. rahman" → "arrahman".
/// Letters, digits and diacritics are kept — "Céline" and "Celine" are left
/// to MusicBrainz.
fn tighten(key: &str) -> String {
    key.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Two spellings of one name: equal once case, spacing, punctuation,
/// diacritics and a leading "The" are set aside ("The Black Eyed Peas" and
/// "Black Eyed Peas", "A.R. Rahman" and "A. R. Rahman").
pub fn same_spelling(a: &str, b: &str) -> bool {
    fn strip(s: &str) -> &str {
        let t = s.trim();
        match t.get(..4) {
            Some(p) if p.eq_ignore_ascii_case("the ") => &t[4..],
            _ => t,
        }
    }
    artist_matches(strip(a), strip(b))
}

/// Does `names` account for everyone `sent` names, with only joining words
/// left over? Unlike [`credit_covers`], a name need not appear in `sent`:
/// MusicBrainz may credit someone the source left out, which is the point.
/// At least one name must appear, or the credit is for something else.
pub fn credit_extends(sent: &str, names: &[&str]) -> bool {
    let mut rest = sent.to_lowercase();
    let mut found = 0;
    for name in names {
        let folded = name.trim().to_lowercase();
        if folded.is_empty() {
            continue;
        }
        if let Some(at) = rest.find(&folded) {
            rest.replace_range(at..at + folded.len(), " ");
            found += 1;
        }
    }
    found > 0 && only_joining_words(&rest)
}

/// Who a listen is credited to under `credits`, primary first, or `None` when
/// MusicBrainz's answer cannot be trusted for it.
///
/// The composers MusicBrainz names on the work join the performers only when
/// the source named them as well — a Plex library filing a concerto under
/// "Felix Mendelssohn" keeps him credited — and `prefer_composer` then makes
/// one of them the primary. Otherwise MusicBrainz's own order stands.
/// The album artist a listen's recording was found under, for the
/// album-artist rule in [`credited_for`].
pub(crate) struct AlbumArtist<'a> {
    /// The release's credit, as `album_artist` holds it.
    pub name: &'a str,
    /// Its MBIDs (`mbid_album_artist`).
    pub mbids: &'a [String],
}

/// What [`credited_for`] decided: who is credited, and whether it took the
/// album-artist rule to get there.
#[derive(Debug, PartialEq)]
pub(crate) struct Credited {
    pub artists: Vec<CreditedArtist>,
    pub via_album: bool,
}

pub(crate) fn credited_for(
    sent: &str,
    own_mbids: &[String],
    credits: &RecordingCredits,
    prefer_composer: bool,
    album: Option<AlbumArtist<'_>>,
) -> Option<Credited> {
    if credits.artists.is_empty() {
        return None;
    }
    let sent_lower = sent.to_lowercase();
    let named = |a: &CreditedArtist| {
        sent_lower.contains(&a.name.to_lowercase())
            || sent_lower.contains(&a.credited_as.to_lowercase())
    };
    let composers: Vec<CreditedArtist> = credits
        .composers
        .iter()
        .filter(|c| named(c) && !credits.artists.iter().any(|a| a.mbid == c.mbid))
        .cloned()
        .collect();

    // Matched on either spelling: the source writes the credited-as name.
    let spellings: Vec<String> = credits
        .artists
        .iter()
        .chain(&composers)
        .flat_map(|a| [a.credited_as.clone(), a.name.clone()])
        .collect();
    let refs: Vec<&str> = spellings.iter().map(String::as_str).collect();
    let mut all_mbids: Vec<&str> =
        credits.artists.iter().chain(&composers).map(|a| a.mbid.as_str()).collect();

    // **The album-artist rule** (Ashwin, 2026-11-04). A source that sends the
    // album's artist as the track's — "Victorious Cast" on a song MusicBrainz
    // credits to Elizabeth Gillies & Ariana Grande, "A.R. Rahman" on a song
    // others sing — names nobody the recording credits, so the rule above
    // refuses it. When that name *is* the album artist, the singers are
    // credited and the album artist stays credited, last. The caller passes
    // `album` only for a recording found by title inside that very release, so
    // a wrong ListenBrainz mapping never gets this far.
    let via_album = if credit_extends(sent, &refs) {
        false
    } else {
        match &album {
            Some(a) if !is_various_artists(sent) && same_spelling(sent, a.name) => {
                all_mbids.extend(a.mbids.iter().map(String::as_str));
                true
            }
            _ => return None,
        }
    };
    if own_mbids.iter().any(|m| !m.trim().is_empty() && !all_mbids.contains(&m.as_str())) {
        return None;
    }

    let mut out: Vec<CreditedArtist> = Vec::new();
    if prefer_composer {
        out.extend(composers.iter().cloned());
        out.extend(credits.artists.iter().cloned());
    } else {
        out.extend(credits.artists.iter().cloned());
        out.extend(composers.iter().cloned());
    }
    if let (true, Some(a)) = (via_album, &album) {
        let mbid = match a.mbids {
            [one] => one.clone(),
            _ => String::new(),
        };
        let already = out
            .iter()
            .any(|c| (!mbid.is_empty() && c.mbid == mbid) || same_spelling(&c.name, a.name));
        if !already {
            out.push(CreditedArtist {
                name: a.name.trim().to_string(),
                mbid,
                join: String::new(),
                credited_as: sent.trim().to_string(),
            });
        }
    }
    Some(Credited { artists: out, via_album })
}

/// The recording `title` is on `release`, matched with "(feat. …)", case,
/// spacing, punctuation and diacritics aside.
pub(crate) fn track_for<'a>(
    title: &str,
    release: &'a crate::musicbrainz::ReleaseTracks,
) -> Option<&'a str> {
    let key = normalise_artist(strip_featuring(title));
    if key.is_empty() {
        return None;
    }
    release
        .tracks
        .iter()
        .find(|t| normalise_artist(strip_featuring(&t.title)) == key)
        .map(|t| t.recording.as_str())
}

/// How many of `titles` are on `release` — how the lookup picks an edition.
pub fn tracks_matched(titles: &[String], release: &crate::musicbrainz::ReleaseTracks) -> usize {
    titles.iter().filter(|t| track_for(t, release).is_some()).count()
}

/// `(id, artist, artist_display, mbid_artist, credit_primary, credit_source,
/// album_artist, mbid_album_artist)` — one listen of a recording being credited.
type CreditRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl Database {
    /// Recording MBIDs whose listens have not had MusicBrainz's credit applied,
    /// most recently played first.
    pub async fn recordings_needing_credits(&self, limit: i64) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT mbid_recording FROM scrobbles
              WHERE (credit_source IS NULL OR credit_source IN ('album-matched', 'album-miss'))
                AND skipped = FALSE
                AND mbid_recording IS NOT NULL AND TRIM(mbid_recording) != ''
              GROUP BY mbid_recording
              ORDER BY MAX(timestamp) DESC
              LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(m,)| m).collect())
    }

    /// Apply MusicBrainz's credit for one recording to every listen of it, for
    /// every user. Returns `(credited, refused)`.
    ///
    /// Each listen is judged on its own, because two copies of one recording
    /// can disagree: one source names the featured artist, another does not.
    pub async fn apply_recording_credits(
        &self,
        recording_mbid: &str,
        credits: &RecordingCredits,
    ) -> Result<(u64, u64), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        #[allow(clippy::type_complexity)]
        let rows: Vec<CreditRow> = sqlx::query_as(&crate::sql!(
            "SELECT s.id, s.artist, s.artist_display, s.mbid_artist, u.credit_primary,
                    s.credit_source, s.album_artist, s.mbid_album_artist
               FROM scrobbles s JOIN users u ON u.id = s.user_id
              WHERE s.mbid_recording = ?
                AND (s.credit_source IS NULL OR s.credit_source IN ('album-matched', 'album-miss'))"
        ))
        .bind(recording_mbid)
        .fetch_all(&mut *tx)
        .await?;

        let (mut credited, mut refused) = (0u64, 0u64);
        for (id, artist, display, mbid_artist, preference, queued, album_artist, album_mbids) in
            rows
        {
            let sent = display.as_deref().unwrap_or(&artist).to_string();
            let json_list = |j: Option<&str>| -> Vec<String> {
                j.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
            };
            let own = json_list(mbid_artist.as_deref());
            let album_mbids = json_list(album_mbids.as_deref());
            let prefer_composer = preference.as_deref() == Some("composer");
            // Only a recording the album lookup found by its title *on this
            // release* may take the album-artist rule.
            let album = match (queued.as_deref(), album_artist.as_deref()) {
                (Some("album-matched"), Some(name)) if !name.trim().is_empty() => {
                    Some(AlbumArtist { name, mbids: &album_mbids })
                }
                _ => None,
            };
            let Some(Credited { artists: names, via_album }) =
                credited_for(&sent, &own, credits, prefer_composer, album)
            else {
                let mark = if credits.artists.is_empty() { "none" } else { "refused" };
                sqlx::query(&crate::sql!("UPDATE scrobbles SET credit_source = ? WHERE id = ?"))
                    .bind(mark)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                refused += u64::from(mark == "refused");
                continue;
            };

            let primary = names[0].name.clone();
            let credit = (names.len() > 1)
                .then(|| names.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join("; "));
            let mbids: Vec<&str> = names.iter().map(|a| a.mbid.as_str()).collect();
            sqlx::query(&crate::sql!(
                "UPDATE scrobbles
                    SET artist_display = COALESCE(artist_display, artist),
                        previous_artist_credit =
                            COALESCE(previous_artist_credit, COALESCE(artist_credit, '')),
                        -- Frozen before the artist moves: a track re-credited
                        -- to its performer stays on the record it is on.
                        album_filed_under = COALESCE(album_filed_under, artist),
                        artist = ?, artist_credit = ?, mbid_artist = ?,
                        credit_source = ?
                  WHERE id = ?"
            ))
            .bind(&primary)
            .bind(&credit)
            .bind(serde_json::to_string(&mbids).unwrap_or_else(|_| "[]".into()))
            .bind(if via_album { "musicbrainz-album" } else { "musicbrainz" })
            .bind(id)
            .execute(&mut *tx)
            .await?;

            // Rebuilt rather than patched, keyed by the canonical name, with
            // each credit's MBID — the identity `unify_artist_spellings` reads.
            sqlx::query(&crate::sql!("DELETE FROM scrobble_artists WHERE scrobble_id = ?"))
                .bind(id)
                .execute(&mut *tx)
                .await?;
            for (position, a) in names.iter().enumerate() {
                sqlx::query(&crate::sql!(
                    "INSERT INTO scrobble_artists (scrobble_id, artist_key, name, position, mbid)
                     VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING"
                ))
                .bind(id)
                .bind(Database::artist_key(&a.name))
                .bind(&a.name)
                .bind(position as i64)
                .bind(&a.mbid)
                .execute(&mut *tx)
                .await?;
            }
            credited += 1;
        }
        tx.commit().await?;
        Ok((credited, refused))
    }

    /// Release MBIDs whose listens still lack the record's artist or its
    /// release group, most recently played first.
    pub async fn releases_needing_info(&self, limit: i64) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT mbid_release FROM scrobbles
              WHERE skipped = FALSE AND mbid_release IS NOT NULL AND TRIM(mbid_release) != ''
                AND mbid_release_group IS NULL
              GROUP BY mbid_release
              ORDER BY MAX(timestamp) DESC
              LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(m,)| m).collect())
    }

    /// Apply what a release lookup knows to every listen of the release.
    ///
    /// The release group is a fact about the release MBID and fills a blank on
    /// every listen carrying it. The release's **credit** files a listen only
    /// when the listen is actually on that record — its album name matches the
    /// release title — because a ListenBrainz mapping often points a box-set
    /// track at the album it first came out on ("Hilary Hahn: The Complete Sony
    /// Recordings" listens carry five releases, none of them the box). Where the
    /// source named an album artist, MusicBrainz's credit replaces it only if
    /// it accounts for that name; otherwise the source's stands.
    ///
    /// `''` in `mbid_release_group` records "asked, none": it takes the release
    /// off the queue.
    pub async fn apply_release_info(
        &self,
        release_mbid: &str,
        info: &crate::musicbrainz::ReleaseInfo,
    ) -> Result<u64, sqlx::Error> {
        let display = info.display();
        let mbids: Vec<&str> = info.artists.iter().map(|a| a.mbid.as_str()).collect();
        let mbids_json =
            (!mbids.is_empty()).then(|| serde_json::to_string(&mbids).unwrap_or_default());
        let spellings: Vec<String> =
            info.artists.iter().flat_map(|a| [a.credited_as.clone(), a.name.clone()]).collect();
        let refs: Vec<&str> = spellings.iter().map(String::as_str).collect();
        // "(feat. …)" aside on both: Spotify writes the album as "Victorious:
        // Music From The Hit TV Show (feat. Victoria Justice)".
        let title_key = normalise_artist(strip_featuring(&info.title));

        let mut tx = self.pool.begin().await?;
        let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(&crate::sql!(
            "SELECT id, album, album_artist FROM scrobbles WHERE mbid_release = ?"
        ))
        .bind(release_mbid)
        .fetch_all(&mut *tx)
        .await?;

        let mut filed = 0u64;
        for (id, album, album_artist) in rows {
            let on_this_record = !title_key.is_empty()
                && album.as_deref().map(|a| normalise_artist(strip_featuring(a))).as_deref()
                    == Some(title_key.as_str());
            let named = album_artist.as_deref().map(str::trim).filter(|a| !a.is_empty());
            let file = on_this_record
                && !display.is_empty()
                && match named {
                    None => true,
                    Some(source) => credit_extends(source, &refs),
                };
            if file {
                sqlx::query(&crate::sql!(
                    "UPDATE scrobbles
                        SET album_artist = ?, album_filed_under = ?,
                            mbid_album_artist = COALESCE(?, mbid_album_artist),
                            mbid_release_group = COALESCE(mbid_release_group, ?)
                      WHERE id = ?"
                ))
                .bind(&display)
                .bind(&display)
                .bind(&mbids_json)
                .bind(info.release_group.clone().unwrap_or_default())
                .bind(id)
                .execute(&mut *tx)
                .await?;
                filed += 1;
            } else {
                sqlx::query(&crate::sql!(
                    "UPDATE scrobbles SET mbid_release_group = COALESCE(mbid_release_group, ?)
                      WHERE id = ?"
                ))
                .bind(info.release_group.clone().unwrap_or_default())
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(filed)
    }

    /// Albums whose listens carry neither a recording nor a release MBID and
    /// have not been looked up, most recently played first — the album-first
    /// lookup's queue, as `(artist, album)` the way the listens spell them.
    pub async fn albums_needing_lookup(
        &self,
        limit: i64,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT MAX(artist), MAX(album) FROM scrobbles
              WHERE skipped = FALSE AND credit_source IS NULL
                AND album IS NOT NULL AND TRIM(album) != ''
                AND (mbid_recording IS NULL OR TRIM(mbid_recording) = '')
                AND (mbid_release IS NULL OR TRIM(mbid_release) = '')
              GROUP BY LOWER(TRIM(artist)), LOWER(TRIM(album))
              ORDER BY MAX(timestamp) DESC
              LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// The titles an album's queued listens carry, to choose the edition
    /// whose tracklist matches most of them.
    pub async fn album_lookup_titles(
        &self,
        artist: &str,
        album: &str,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT DISTINCT title FROM scrobbles
              WHERE skipped = FALSE AND credit_source IS NULL
                AND LOWER(TRIM(artist)) = LOWER(TRIM(?)) AND LOWER(TRIM(album)) = LOWER(TRIM(?))
                AND (mbid_recording IS NULL OR TRIM(mbid_recording) = '')
                AND (mbid_release IS NULL OR TRIM(mbid_release) = '')"
        ))
        .bind(artist)
        .bind(album)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(t,)| t).collect())
    }

    /// Write what the album-first lookup found onto the album's listens.
    ///
    /// A listen whose title is on `found`'s tracklist gets the release and
    /// recording MBIDs and is queued for credits as `album-matched` — the
    /// provenance the album-artist rule requires. Every other listen of the
    /// album, and all of them when `found` is `None`, is marked `album-miss`
    /// so it is not asked about again. Only blanks are filled. The release's
    /// credit and group then go through [`Self::apply_release_info`], the same
    /// title-matched filing as any release. Returns `(matched, missed)`.
    pub async fn apply_album_lookup(
        &self,
        artist: &str,
        album: &str,
        found: Option<&crate::musicbrainz::ReleaseTracks>,
    ) -> Result<(u64, u64), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let rows: Vec<(i64, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, title FROM scrobbles
              WHERE credit_source IS NULL
                AND LOWER(TRIM(artist)) = LOWER(TRIM(?)) AND LOWER(TRIM(album)) = LOWER(TRIM(?))
                AND (mbid_recording IS NULL OR TRIM(mbid_recording) = '')
                AND (mbid_release IS NULL OR TRIM(mbid_release) = '')"
        ))
        .bind(artist)
        .bind(album)
        .fetch_all(&mut *tx)
        .await?;
        let (mut matched, mut missed) = (0u64, 0u64);
        for (id, title) in rows {
            match found.and_then(|f| track_for(&title, f)) {
                Some(recording) => {
                    sqlx::query(&crate::sql!(
                        "UPDATE scrobbles
                            SET mbid_recording = ?, mbid_release = ?, credit_source = 'album-matched'
                          WHERE id = ?"
                    ))
                    .bind(recording)
                    .bind(&found.map(|f| f.id.clone()).unwrap_or_default())
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                    matched += 1;
                }
                None => {
                    sqlx::query(&crate::sql!(
                        "UPDATE scrobbles SET credit_source = 'album-miss' WHERE id = ?"
                    ))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                    missed += 1;
                }
            }
        }
        tx.commit().await?;
        if let (Some(f), true) = (found, matched > 0) {
            self.apply_release_info(&f.id, &f.info).await?;
        }
        Ok((matched, missed))
    }

    /// Albums of this user's that the album-first lookup could not find on
    /// MusicBrainz, most played first — the Sanitiser's "Not on MusicBrainz"
    /// list, so a record bought on Qobuz that MusicBrainz lacks can be added
    /// there and looked up again. `(artist, album, plays, last played)`.
    pub async fn musicbrainz_misses(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String, i64, i64)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT MAX(artist), MAX(album), COUNT(*), MAX(timestamp) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND credit_source = 'album-miss'
              GROUP BY LOWER(TRIM(artist)), LOWER(TRIM(album))
              ORDER BY COUNT(*) DESC, MAX(timestamp) DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Put one missed album back on the lookup queue — after it has been
    /// added to MusicBrainz. Only this user's listens of it, only the misses.
    pub async fn retry_album_lookup(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET credit_source = NULL
              WHERE user_id = ? AND credit_source = 'album-miss'
                AND LOWER(TRIM(artist)) = LOWER(TRIM(?)) AND LOWER(TRIM(album)) = LOWER(TRIM(?))"
        ))
        .bind(user_id)
        .bind(artist)
        .bind(album)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// Move each album entity — the thing a love or a liner note points at —
    /// onto the artist its record is now filed under.
    ///
    /// An album entity was keyed by whoever its listens were filed under, which
    /// for a soundtrack or a box set was a track's performer. Once the listens
    /// agree on one album artist the entity follows it, and its old key is
    /// kept as an alias so a love recorded against it still fills the heart and
    /// an old link still resolves. Where the listens name more than one album
    /// artist, or another entity already holds the new key, it is left where it
    /// is and returned by name for the operator. Returns `(moved, left)`.
    pub async fn migrate_album_entities(&self) -> Result<(u64, Vec<String>), sqlx::Error> {
        let entities: Vec<(i64, String, String, String)> = sqlx::query_as(&crate::sql!(
            "SELECT id, name, artist_name, entity_key FROM entities WHERE kind = 'release'"
        ))
        .fetch_all(&self.pool)
        .await?;
        let (mut moved, mut left) = (0u64, Vec::new());
        for (id, name, artist, old_key) in entities {
            let filed: Vec<(String,)> = sqlx::query_as(&crate::sql!(
                "SELECT MAX(COALESCE(album_filed_under, artist)) FROM scrobbles
                  WHERE skipped = FALSE AND LOWER(TRIM(album)) = ? AND LOWER(TRIM(artist)) = ?
                  GROUP BY LOWER(TRIM(COALESCE(album_filed_under, artist)))"
            ))
            .bind(name.trim().to_lowercase())
            .bind(artist.trim().to_lowercase())
            .fetch_all(&self.pool)
            .await?;
            let target = match filed.as_slice() {
                [(one,)] if one.trim().to_lowercase() != artist.trim().to_lowercase() => {
                    one.clone()
                }
                [_] | [] => continue,
                _ => {
                    left.push(format!("{name} — {artist}"));
                    continue;
                }
            };
            let new_key = Database::entity_key("release", &target, &name);
            let taken: Option<(i64,)> =
                sqlx::query_as(&crate::sql!("SELECT id FROM entities WHERE entity_key = ?"))
                    .bind(&new_key)
                    .fetch_optional(&self.pool)
                    .await?;
            if taken.is_some() {
                left.push(format!("{name} — {artist}"));
                continue;
            }
            let mut tx = self.pool.begin().await?;
            sqlx::query(&crate::sql!(
                "INSERT INTO entity_aliases (entity_id, alias_kind, value) VALUES (?, 'key', ?)
                 ON CONFLICT DO NOTHING"
            ))
            .bind(id)
            .bind(&old_key)
            .execute(&mut *tx)
            .await?;
            sqlx::query(&crate::sql!(
                "UPDATE entities SET artist_name = ?, entity_key = ?, match_key = ? WHERE id = ?"
            ))
            .bind(target.trim())
            .bind(&new_key)
            .bind(Database::match_key("release", &target, &name))
            .bind(id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            moved += 1;
        }
        Ok((moved, left))
    }

    /// One artist MBID, one artist: fold every spelling a credit carries under
    /// an MBID onto the most-played one.
    ///
    /// "The Black Eyed Peas" and "Black Eyed Peas" share an MBID and were two
    /// rows on every chart, because credits are keyed by name. This needs no
    /// lookup — the MBIDs are already on the listens. Two *different* MBIDs
    /// under one name (BLACKPINK's Lisa and LiSA) are not separated here: an
    /// artist page is found by name, and splitting them needs pages found by
    /// MBID first.
    pub async fn unify_artist_spellings(&self) -> Result<u64, sqlx::Error> {
        // Punctuation first: it weighs every credit of a spelling, so the name
        // that wins is the one most credits use, not whichever spelling won a
        // tie among the few that carry an MBID.
        let mut changed = self.unify_punctuation_spellings().await?;
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT mbid, artist_key, MAX(name), COUNT(*) FROM scrobble_artists
              WHERE mbid IS NOT NULL AND TRIM(mbid) != ''
              GROUP BY mbid, artist_key"
        ))
        .fetch_all(&self.pool)
        .await?;
        let mut by_mbid: HashMap<String, Vec<(String, String, i64)>> = HashMap::new();
        for (mbid, key, name, n) in rows {
            by_mbid.entry(mbid).or_default().push((key, name, n));
        }

        for (mbid, mut spellings) in by_mbid.into_iter().filter(|(_, v)| v.len() > 1) {
            spellings.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
            let (key, name, _) = spellings[0].clone();
            // **Only a spelling of the same name folds.** A row's MBID is not
            // proof of who it is: older rows were zipped by position, and a
            // ListenBrainz mapping can name the wrong artist outright — so an
            // MBID shared by Shostakovich and Bach, or by "Madonna & Sabrina
            // Carpenter" and Madonna, folds nobody (2026-11-04).
            for (loser, loser_name, _) in spellings[1..].iter() {
                if !same_spelling(loser_name, &name) {
                    continue;
                }
                let mut tx = self.pool.begin().await?;
                // A listen already crediting the winning spelling would collide
                // on the primary key; its duplicate row goes, not the listen.
                sqlx::query(&crate::sql!(
                    "DELETE FROM scrobble_artists
                      WHERE mbid = ? AND artist_key = ?
                        AND scrobble_id IN (SELECT scrobble_id FROM scrobble_artists WHERE artist_key = ?)"
                ))
                .bind(&mbid)
                .bind(loser)
                .bind(&key)
                .execute(&mut *tx)
                .await?;
                let r = sqlx::query(&crate::sql!(
                    "UPDATE scrobble_artists SET artist_key = ?, name = ?
                      WHERE mbid = ? AND artist_key = ?"
                ))
                .bind(&key)
                .bind(&name)
                .bind(&mbid)
                .bind(loser)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                changed += r.rows_affected();
            }
        }
        Ok(changed)
    }

    /// Fold spellings that only spacing and punctuation tell apart — "A. R.
    /// Rahman" onto "A.R. Rahman", "S.P. Balasubrahmanyam" onto "S. P.
    /// Balasubrahmanyam" — whether or not their credits carry an MBID: most
    /// of the 145 "A. R. Rahman" credits that charted at #81 beside "A.R.
    /// Rahman" at #10 had none (2026-11-04).
    ///
    /// The key already folds case. Two guards: the name must keep at least
    /// [`TIGHT_MIN`] letters once tightened, so "M.I.A." never meets "Mia";
    /// and where both spellings carry MBIDs, they must share one — two
    /// different artists who happen to spell alike stay apart.
    async fn unify_punctuation_spellings(&self) -> Result<u64, sqlx::Error> {
        let rows: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT artist_key, MAX(name), mbid, COUNT(*) FROM scrobble_artists
              GROUP BY artist_key, mbid"
        ))
        .fetch_all(&self.pool)
        .await?;
        // key → (name, credits, MBIDs)
        let mut keys: HashMap<String, (String, i64, Vec<String>)> = HashMap::new();
        for (key, name, mbid, n) in rows {
            let e = keys.entry(key).or_insert_with(|| (name, 0, Vec::new()));
            e.1 += n;
            if let Some(m) = mbid.filter(|m| !m.trim().is_empty()) {
                e.2.push(m);
            }
        }
        let mut by_tight: HashMap<String, Vec<String>> = HashMap::new();
        for key in keys.keys() {
            let t = tighten(key);
            if t.chars().count() >= TIGHT_MIN {
                by_tight.entry(t).or_default().push(key.clone());
            }
        }

        let mut changed = 0u64;
        for (_, mut group) in by_tight.into_iter().filter(|(_, g)| g.len() > 1) {
            group.sort_by(|a, b| keys[b].1.cmp(&keys[a].1).then(a.cmp(b)));
            let winner = group[0].clone();
            let (name, _, winner_mbids) = keys[&winner].clone();
            for loser in &group[1..] {
                let loser_mbids = &keys[loser].2;
                let disagree = !winner_mbids.is_empty()
                    && !loser_mbids.is_empty()
                    && !loser_mbids.iter().any(|m| winner_mbids.contains(m));
                if disagree {
                    continue;
                }
                let mut tx = self.pool.begin().await?;
                sqlx::query(&crate::sql!(
                    "DELETE FROM scrobble_artists
                      WHERE artist_key = ?
                        AND scrobble_id IN (SELECT scrobble_id FROM scrobble_artists WHERE artist_key = ?)"
                ))
                .bind(loser)
                .bind(&winner)
                .execute(&mut *tx)
                .await?;
                let r = sqlx::query(&crate::sql!(
                    "UPDATE scrobble_artists SET artist_key = ?, name = ? WHERE artist_key = ?"
                ))
                .bind(&winner)
                .bind(&name)
                .bind(loser)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                changed += r.rows_affected();
            }
        }
        Ok(changed)
    }

    /// The spelling an artist's credits are under, when `name` has none of its
    /// own because [`Self::unify_artist_spellings`] folded it ("Black Eyed
    /// Peas" → "The Black Eyed Peas"). A history row still says what the
    /// source sent, and clicking it should open the artist, not an empty page.
    pub async fn credited_spelling(
        &self,
        user_id: i64,
        name: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let name = name.trim();
        let own: Option<(i64,)> = sqlx::query_as(&crate::sql!(
            "SELECT 1 FROM scrobble_artists sa JOIN scrobbles s ON s.id = sa.scrobble_id
              WHERE s.user_id = ? AND s.skipped = FALSE AND sa.artist_key = ? LIMIT 1"
        ))
        .bind(user_id)
        .bind(Database::artist_key(name))
        .fetch_optional(&self.pool)
        .await?;
        if own.is_some() {
            return Ok(None);
        }
        // Only on a miss, so the unindexed `LIKE` is paid rarely. The same
        // function both sides, so SQLite's ASCII-only LOWER cannot disagree.
        let rows: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT DISTINCT sa.name FROM scrobbles s
               JOIN scrobble_artists sa ON sa.scrobble_id = s.id
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND (LOWER(TRIM(s.artist)) = LOWER(?)
                     OR LOWER(s.artist_credit) LIKE '%' || LOWER(?) || '%')
              LIMIT 200"
        ))
        .bind(user_id)
        .bind(name)
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(n,)| n).find(|n| same_spelling(n, name)))
    }

    /// Artist MBIDs credited on listens, most credited first — the order the
    /// band-membership lookups are spent in.
    pub async fn credited_artist_mbids(&self, limit: i64) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT sa.mbid FROM scrobble_artists sa JOIN scrobbles s ON s.id = sa.scrobble_id
              WHERE s.skipped = FALSE AND sa.mbid IS NOT NULL AND TRIM(sa.mbid) != ''
              GROUP BY sa.mbid ORDER BY COUNT(*) DESC LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(m,)| m).collect())
    }

    /// The MBID most of this user's credits of an artist carry, if any.
    pub async fn artist_mbid_for(
        &self,
        user_id: i64,
        artist: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT sa.mbid FROM scrobble_artists sa JOIN scrobbles s ON s.id = sa.scrobble_id
              WHERE s.user_id = ? AND s.skipped = FALSE AND sa.artist_key = ?
                AND sa.mbid IS NOT NULL AND TRIM(sa.mbid) != ''
              GROUP BY sa.mbid ORDER BY COUNT(*) DESC LIMIT 1"
        ))
        .bind(user_id)
        .bind(Database::artist_key(artist))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(m,)| m))
    }

    /// Which of `names` this user has listened to — so a band's page links only
    /// the members who are in the history, never presents someone else's as theirs.
    pub async fn artists_in_history(
        &self,
        user_id: i64,
        names: &[String],
    ) -> Result<Vec<String>, sqlx::Error> {
        let mut out = Vec::new();
        for name in names {
            let hit: Option<(i64,)> = sqlx::query_as(&crate::sql!(&format!(
                "SELECT 1 FROM scrobbles s WHERE s.user_id = ? AND s.skipped = FALSE AND {CREDITS_ARTIST} LIMIT 1"
            )))
            .bind(user_id)
            .bind(Database::artist_key(name))
            .fetch_optional(&self.pool)
            .await?;
            if hit.is_some() {
                out.push(name.clone());
            }
        }
        Ok(out)
    }

    /// A user's performer/composer preference.
    pub async fn credit_primary(&self, user_id: i64) -> Result<String, sqlx::Error> {
        let v: Option<String> =
            sqlx::query_scalar(&crate::sql!("SELECT credit_primary FROM users WHERE id = ?"))
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(v.unwrap_or_else(|| "performer".into()))
    }

    /// Set it, and requeue this user's MusicBrainz-credited listens so the
    /// credits backfill files them again under the new rule. Only listens
    /// MusicBrainz credited are touched; the cache answers, so no lookup is
    /// spent.
    pub async fn set_credit_primary(&self, user_id: i64, value: &str) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(&crate::sql!("UPDATE users SET credit_primary = ? WHERE id = ?"))
            .bind(value)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles
                SET credit_source = CASE credit_source WHEN 'musicbrainz-album'
                                         THEN 'album-matched' ELSE NULL END
              WHERE user_id = ? AND credit_source IN ('musicbrainz', 'musicbrainz-album')"
        ))
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(name: &str, mbid: &str) -> CreditedArtist {
        CreditedArtist {
            name: name.into(),
            mbid: mbid.into(),
            join: String::new(),
            credited_as: name.into(),
        }
    }

    #[test]
    fn musicbrainz_may_add_a_credit_but_never_contradict_one() {
        let loud = RecordingCredits {
            artists: vec![a("Cœur de pirate", "cdp"), a("Loud", "loud")],
            composers: vec![],
        };
        // The source left Loud out: he is added.
        let got =
            credited_for("Cœur de pirate", &["cdp".into()], &loud, false, None).unwrap().artists;
        assert_eq!(
            got.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Cœur de pirate", "Loud"]
        );

        // The wrong recording: the source names someone the credit does not.
        let wrong = RecordingCredits {
            artists: vec![a("Boyz II Men", "b2m"), a("An Orchestra", "orch")],
            composers: vec![],
        };
        assert!(credited_for("Boyz II Men, Mariah Carey", &[], &wrong, false, None).is_none());

        // The listen's own MBIDs disagree with the credit.
        assert!(
            credited_for("Cœur de pirate", &["someone-else".into()], &loud, false, None).is_none()
        );
    }

    #[test]
    fn a_named_composer_stays_credited_and_the_setting_picks_the_primary() {
        let concerto = RecordingCredits {
            artists: vec![a("Hilary Hahn", "hahn"), a("Hugh Wolff", "wolff")],
            composers: vec![a("Felix Mendelssohn", "fm")],
        };
        let performer =
            credited_for("Felix Mendelssohn", &[], &concerto, false, None).unwrap().artists;
        assert_eq!(
            performer.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Hilary Hahn", "Hugh Wolff", "Felix Mendelssohn"]
        );
        let composer =
            credited_for("Felix Mendelssohn", &[], &concerto, true, None).unwrap().artists;
        assert_eq!(composer[0].name, "Felix Mendelssohn");

        // A composer the source never named is not added.
        let unnamed = credited_for("Hilary Hahn", &[], &concerto, true, None).unwrap().artists;
        assert_eq!(
            unnamed.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Hilary Hahn", "Hugh Wolff"]
        );
    }

    /// The source sent the album's artist; MusicBrainz credits the singers.
    #[test]
    fn the_album_artist_rule_credits_the_singers_and_keeps_the_cast() {
        let give_it_up = RecordingCredits {
            artists: vec![a("Elizabeth Gillies", "eg"), a("Ariana Grande", "ag")],
            composers: vec![],
        };
        let cast_mbids = vec!["cast".to_string()];
        let cast = || Some(AlbumArtist { name: "Victorious Cast", mbids: &cast_mbids });

        // Without the album artist it is refused, as before.
        assert!(credited_for("Victorious Cast", &[], &give_it_up, false, None).is_none());

        let got = credited_for("Victorious Cast", &[], &give_it_up, false, cast()).unwrap();
        assert!(got.via_album);
        let names: Vec<&str> = got.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Elizabeth Gillies", "Ariana Grande", "Victorious Cast"]);
        assert_eq!(got.artists[2].mbid, "cast");

        // The listen's own MBID may be a singer's (Spotify sent Victoria
        // Justice's under the cast's name) or the cast's — never a stranger's.
        let freak =
            RecordingCredits { artists: vec![a("Victoria Justice", "vj")], composers: vec![] };
        assert!(credited_for("Victorious Cast", &["vj".into()], &freak, false, cast()).is_some());
        assert!(credited_for("Victorious Cast", &["cast".into()], &freak, false, cast()).is_some());
        assert!(
            credited_for("Victorious Cast", &["stranger".into()], &freak, false, cast()).is_none()
        );

        // The cast already credited on the recording is not added twice, and
        // the ordinary rule answers.
        let i_want_you_back = RecordingCredits {
            artists: vec![a("Victoria Justice", "vj"), a("Victorious Cast", "cast")],
            composers: vec![],
        };
        let got = credited_for("Victorious Cast", &[], &i_want_you_back, false, cast()).unwrap();
        assert!(!got.via_album);
        assert_eq!(got.artists.len(), 2);

        // Only when the sent name *is* the album artist.
        let other = vec![];
        let someone_else = Some(AlbumArtist { name: "iCarly Cast", mbids: &other });
        assert!(credited_for("Victorious Cast", &[], &give_it_up, false, someone_else).is_none());
        let va = Some(AlbumArtist { name: "Various Artists", mbids: &other });
        assert!(credited_for("Various Artists", &[], &give_it_up, false, va).is_none());
    }

    #[test]
    fn a_title_matches_its_track_with_the_featuring_credit_aside() {
        use crate::musicbrainz::{ReleaseTrack, ReleaseTracks};
        let release = ReleaseTracks {
            id: "rel".into(),
            tracks: vec![
                ReleaseTrack { title: "Give It Up".into(), recording: "r-give".into() },
                ReleaseTrack { title: "You’re the Reason".into(), recording: "r-reason".into() },
                ReleaseTrack {
                    title: "Make It Shine (Victorious Theme)".into(),
                    recording: "r-shine".into(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            track_for("Give It Up (feat. Elizabeth Gillies & Ariana Grande)", &release),
            Some("r-give")
        );
        assert_eq!(
            track_for("You're The Reason (feat. Victoria Justice)", &release),
            Some("r-reason")
        );
        assert_eq!(track_for("Make It Shine (Victorious Theme)", &release), Some("r-shine"));
        assert_eq!(
            track_for("Make It Shine", &release),
            None,
            "only a featuring credit is set aside"
        );
        assert_eq!(track_for("Song 2 You", &release), None);
        assert_eq!(
            strip_featuring("Victorious: Music From The Hit TV Show (feat. Victoria Justice)"),
            "Victorious: Music From The Hit TV Show"
        );
        assert_eq!(strip_featuring("Song [ft. Someone]"), "Song");
        assert_eq!(strip_featuring("Featuring Nobody"), "Featuring Nobody");
    }
}
