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
pub(crate) fn credited_for(
    sent: &str,
    own_mbids: &[String],
    credits: &RecordingCredits,
    prefer_composer: bool,
) -> Option<Vec<CreditedArtist>> {
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
    if !credit_extends(sent, &refs) {
        return None;
    }
    let all_mbids: Vec<&str> =
        credits.artists.iter().chain(&composers).map(|a| a.mbid.as_str()).collect();
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
    Some(out)
}

impl Database {
    /// Recording MBIDs whose listens have not had MusicBrainz's credit applied,
    /// most recently played first.
    pub async fn recordings_needing_credits(&self, limit: i64) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT mbid_recording FROM scrobbles
              WHERE credit_source IS NULL AND skipped = FALSE
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
        let rows: Vec<(i64, String, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as(&crate::sql!(
                "SELECT s.id, s.artist, s.artist_display, s.mbid_artist, u.credit_primary
                   FROM scrobbles s JOIN users u ON u.id = s.user_id
                  WHERE s.mbid_recording = ? AND s.credit_source IS NULL"
            ))
            .bind(recording_mbid)
            .fetch_all(&mut *tx)
            .await?;

        let (mut credited, mut refused) = (0u64, 0u64);
        for (id, artist, display, mbid_artist, preference) in rows {
            let sent = display.as_deref().unwrap_or(&artist).to_string();
            let own: Vec<String> = mbid_artist
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok())
                .unwrap_or_default();
            let prefer_composer = preference.as_deref() == Some("composer");
            let Some(names) = credited_for(&sent, &own, credits, prefer_composer) else {
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
                        credit_source = 'musicbrainz'
                  WHERE id = ?"
            ))
            .bind(&primary)
            .bind(&credit)
            .bind(serde_json::to_string(&mbids).unwrap_or_else(|_| "[]".into()))
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
        let title_key = normalise_artist(&info.title);

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
                && album.as_deref().map(normalise_artist).as_deref() == Some(title_key.as_str());
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

        let mut changed = 0u64;
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
            "UPDATE scrobbles SET credit_source = NULL
              WHERE user_id = ? AND credit_source = 'musicbrainz'"
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
        let got = credited_for("Cœur de pirate", &["cdp".into()], &loud, false).unwrap();
        assert_eq!(
            got.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Cœur de pirate", "Loud"]
        );

        // The wrong recording: the source names someone the credit does not.
        let wrong = RecordingCredits {
            artists: vec![a("Boyz II Men", "b2m"), a("An Orchestra", "orch")],
            composers: vec![],
        };
        assert!(credited_for("Boyz II Men, Mariah Carey", &[], &wrong, false).is_none());

        // The listen's own MBIDs disagree with the credit.
        assert!(credited_for("Cœur de pirate", &["someone-else".into()], &loud, false).is_none());
    }

    #[test]
    fn a_named_composer_stays_credited_and_the_setting_picks_the_primary() {
        let concerto = RecordingCredits {
            artists: vec![a("Hilary Hahn", "hahn"), a("Hugh Wolff", "wolff")],
            composers: vec![a("Felix Mendelssohn", "fm")],
        };
        let performer = credited_for("Felix Mendelssohn", &[], &concerto, false).unwrap();
        assert_eq!(
            performer.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Hilary Hahn", "Hugh Wolff", "Felix Mendelssohn"]
        );
        let composer = credited_for("Felix Mendelssohn", &[], &concerto, true).unwrap();
        assert_eq!(composer[0].name, "Felix Mendelssohn");

        // A composer the source never named is not added.
        let unnamed = credited_for("Hilary Hahn", &[], &concerto, true).unwrap();
        assert_eq!(
            unnamed.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Hilary Hahn", "Hugh Wolff"]
        );
    }
}
