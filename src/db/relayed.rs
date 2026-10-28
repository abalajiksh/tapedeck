//! Spotify plays held twice: once from the Spotify export, and once more from a
//! ListenBrainz archive whose own Spotify importer had relayed the same export.
//!
//! The import no longer does this (see `listen_to_play`), but every history
//! imported both ways before that carries the second copy — roughly a whole
//! Spotify era counted double. Preview-then-apply like the sanitiser, and never
//! automatic: it deletes listens, and the operator should see what before it
//! does.
//!
//! **What a relayed copy looks like.** ListenBrainz stamped each play with the
//! export's `ts`, which is its *end*; the Spotify import stores the start. So
//! the copy is an `import` row of the same recording landing after a Spotify
//! row by no more than that track's length — the time it played. Nothing else
//! the old importer wrote can tell it apart: its client was always `import`
//! and it kept no Spotify id, which is also how a row written by the fixed
//! importer is recognised and left alone.

use super::import::{fill_blanks, play_from_sparse, sparse_columns};
use super::*;
use chrono::Datelike;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Slack on top of a track's length. A seek backwards makes a play run a
/// little longer than the track, and the export's `ts` is only to the second.
const OVERRUN_SECS: i64 = 10;

/// How far after a Spotify play to look when neither copy knows the track's
/// length. Ten minutes is longer than nearly anything played and much shorter
/// than the gap to a separate listening of the same song.
const UNKNOWN_LENGTH_SECS: i64 = 600;

/// One listen as the pairing reads it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RelayRow {
    pub id: i64,
    pub timestamp: i64,
    pub title: String,
    pub artist: String,
    pub duration: Option<i64>,
    pub source_name: String,
    pub submission_client: Option<String>,
    pub skipped: bool,
    pub mbid_recording: Option<String>,
    pub isrc: Option<String>,
    pub spotify_track_id: Option<String>,
    pub session_id: Option<i64>,
}

impl RelayRow {
    /// A play from the Spotify export itself.
    fn is_spotify(&self) -> bool {
        self.source_name == "spotify" && self.spotify_track_id.is_some()
    }

    /// Written by the importer before it knew about relays: an `import` row
    /// with no client of its own and no Spotify id. Skips are never copies —
    /// ListenBrainz relayed every play it kept as a listen.
    fn could_be_relayed_copy(&self) -> bool {
        self.source_name == "import"
            && !self.skipped
            && self.spotify_track_id.is_none()
            && self.submission_client.as_deref().is_none_or(|c| c == "import")
    }
}

/// The same recording, by any identifier both rows carry.
///
/// Titles are compared folded the way artists are — accents, case and
/// punctuation gone — and with a version suffix allowed on either side. The
/// export and ListenBrainz name the same Spotify track differently often
/// enough to matter: "Éblouie par la nuit" / "Eblouie par la nuit", "Forever
/// Young" / "Forever Young - 2019 Remaster". Measured, 2,304 of the copies a
/// plain comparison missed differed only like that.
fn same_recording(a: &RelayRow, b: &RelayRow) -> bool {
    let shared = |x: &Option<String>, y: &Option<String>| match (x.as_deref(), y.as_deref()) {
        (Some(x), Some(y)) => !x.trim().is_empty() && x.trim().eq_ignore_ascii_case(y.trim()),
        _ => false,
    };
    same_title(&a.title, &b.title)
        || shared(&a.mbid_recording, &b.mbid_recording)
        || shared(&a.isrc, &b.isrc)
}

fn same_title(a: &str, b: &str) -> bool {
    let (fa, fb) = (normalise_artist(a), normalise_artist(b));
    if fa.is_empty() || fb.is_empty() {
        return false;
    }
    let (ba, bb) = (normalise_artist(base_title(a)), normalise_artist(base_title(b)));
    fa == fb || (!ba.is_empty() && (ba == fb || ba == bb)) || (!bb.is_empty() && bb == fa)
}

/// A title without its version suffix: what comes before " - " or " (".
fn base_title(t: &str) -> &str {
    let cut = [" - ", " (", " ["].iter().filter_map(|sep| t.find(sep)).min();
    cut.map_or(t, |i| &t[..i])
}

/// Not known to be different people.
///
/// Containment rather than equality, because the two exports name a
/// collaboration differently: Spotify's export gives the primary
/// ("A.R. Rahman") where ListenBrainz wrote the whole credit
/// ("A.R. Rahman, Lady Kash"). 7,408 of 27,193 relayed plays disagree that way.
fn compatible_artists(a: &RelayRow, b: &RelayRow) -> bool {
    let (x, y) = (normalise_artist(&a.artist), normalise_artist(&b.artist));
    (!x.is_empty() && !y.is_empty() && (x.contains(&y) || y.contains(&x)))
        || is_various_artists(&a.artist)
        || is_various_artists(&b.artist)
}

/// Which `import` rows are relayed copies of which Spotify plays, as
/// `(copy, kept)` indices into `rows`. `rows` must be in timestamp order.
///
/// **Each Spotify play is claimed once, and full plays are paired first.**
/// A repeat makes this matter: a copy of play N is stamped at play N's end,
/// which is play N+1's start to the second, so both are in its window. Most
/// plays ran to the end, and then the copy sits one track length after its
/// own play's start, which no other play can match as closely; those pairs are
/// made first, across the whole history. What is left — partial plays — takes
/// the earliest unclaimed play in its window. Whichever is chosen, a copy is
/// only ever paired with *some* Spotify play of the same recording that it
/// cannot be a separate listening of, so a mis-pairing moves which metadata is
/// merged, never whether a listen is lost.
pub fn pair_relayed_copies(rows: &[RelayRow]) -> Vec<(usize, usize)> {
    let spotify: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].is_spotify()).collect();
    let copies: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].could_be_relayed_copy()).collect();
    let length = |copy: &RelayRow, kept: &RelayRow| {
        kept.duration.or(copy.duration).filter(|d| *d > 0).unwrap_or(UNKNOWN_LENGTH_SECS)
    };
    // The Spotify plays a copy could be a copy of, in start order.
    let window = |ci: usize| {
        let copy = &rows[ci];
        // Bounds the scan only; the per-pair test is the track's length.
        let from = spotify.partition_point(|&s| rows[s].timestamp < copy.timestamp - 3600);
        spotify[from..]
            .iter()
            .copied()
            .take_while(move |&s| rows[s].timestamp <= copy.timestamp)
            .filter(move |&s| {
                let kept = &rows[s];
                copy.timestamp - kept.timestamp <= length(copy, kept) + OVERRUN_SECS
                    && same_recording(copy, kept)
                    && compatible_artists(copy, kept)
            })
    };

    // Full plays, closest fit first across the whole history rather than copy
    // by copy: taken in time order, a neighbouring copy that fits a play
    // nearly as well can claim it first and push its own copy into the
    // partial-play pass. Measured, that left one copy in a thousand for a
    // second run of the repair to find.
    let mut fits: Vec<(i64, usize, usize)> = copies
        .iter()
        .flat_map(|&ci| {
            let copy = &rows[ci];
            window(ci).map(move |s| {
                let off = ((copy.timestamp - rows[s].timestamp) - length(copy, &rows[s])).abs();
                (off, ci, s)
            })
        })
        .filter(|&(off, _, _)| off <= OVERRUN_SECS)
        .collect();
    fits.sort_unstable();

    let mut claimed = vec![false; rows.len()];
    let mut paired = vec![false; rows.len()];
    let mut pairs = Vec::new();
    for (_, ci, s) in fits {
        if !claimed[s] && !paired[ci] {
            claimed[s] = true;
            paired[ci] = true;
            pairs.push((ci, s));
        }
    }
    for &ci in &copies {
        if paired[ci] {
            continue;
        }
        if let Some(s) = window(ci).find(|&s| !claimed[s]) {
            claimed[s] = true;
            pairs.push((ci, s));
        }
    }
    pairs.sort_unstable();
    pairs
}

/// One copy and the play it duplicates.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RelayedCopy {
    /// The listen that would be removed.
    pub copy_id: i64,
    /// The Spotify play it duplicates, which stays.
    pub kept_id: i64,
    pub title: String,
    pub artist: String,
    /// When the copy says the play happened — its end. Unix seconds.
    pub copy_at: i64,
    /// When the play started, from the Spotify export. Unix seconds.
    pub kept_at: i64,
    /// Spotify's own record calls this play a skip, so the copy was counting
    /// a skip as a listen.
    pub kept_is_skip: bool,
}

/// Copies per year, by the kept play's year in UTC.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RelayedYear {
    pub year: i32,
    pub copies: usize,
}

/// What the repair would remove. Nothing has been changed.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RelayedPreview {
    /// Listens that are relayed copies. Pass this back to apply.
    pub copies: usize,
    /// Of those, copies of plays Spotify's export calls skips: they were
    /// counting as listens.
    pub of_skips: usize,
    pub by_year: Vec<RelayedYear>,
    /// The first fifty, oldest first.
    pub sample: Vec<RelayedCopy>,
}

/// What the repair did.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RelayedApplied {
    /// Copies deleted.
    pub removed: usize,
    /// Copies left in place because both they and the play they duplicate
    /// carry a liner note, and a note cannot be merged.
    pub kept_for_notes: usize,
    /// Liner notes moved from a copy onto the play it duplicated.
    pub notes_moved: usize,
}

impl Database {
    /// Every listen that could be either side of a pair, oldest first.
    ///
    /// Skips are read on purpose: a copy can duplicate a play Spotify filed
    /// as a skip, and that pair is the one counting a skip as a listen.
    async fn relay_candidates(&self, user_id: i64) -> Result<Vec<RelayRow>, sqlx::Error> {
        sqlx::query_as::<_, RelayRow>(&crate::sql!(
            "SELECT id, timestamp, title, artist, duration, source_name, submission_client,
                    skipped, mbid_recording, isrc, spotify_track_id, session_id
               FROM scrobbles
              WHERE user_id = ? AND source_name IN ('spotify', 'import')
              ORDER BY timestamp, id",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// What `apply_relayed_copies` would remove.
    pub async fn preview_relayed_copies(
        &self,
        user_id: i64,
    ) -> Result<RelayedPreview, sqlx::Error> {
        let rows = self.relay_candidates(user_id).await?;
        let pairs = pair_relayed_copies(&rows);
        let mut years: BTreeMap<i32, usize> = BTreeMap::new();
        for &(_, k) in &pairs {
            let year = chrono::DateTime::from_timestamp(rows[k].timestamp, 0)
                .map(|d| d.year())
                .unwrap_or(1970);
            *years.entry(year).or_default() += 1;
        }
        Ok(RelayedPreview {
            copies: pairs.len(),
            of_skips: pairs.iter().filter(|&&(_, k)| rows[k].skipped).count(),
            by_year: years.into_iter().map(|(year, copies)| RelayedYear { year, copies }).collect(),
            sample: pairs
                .iter()
                .take(50)
                .map(|&(c, k)| RelayedCopy {
                    copy_id: rows[c].id,
                    kept_id: rows[k].id,
                    title: rows[c].title.clone(),
                    artist: rows[c].artist.clone(),
                    copy_at: rows[c].timestamp,
                    kept_at: rows[k].timestamp,
                    kept_is_skip: rows[k].skipped,
                })
                .collect(),
        })
    }

    /// Remove every relayed copy, **if** there are still exactly `expected`.
    ///
    /// The count is the preview's, passed back: if the history changed in
    /// between — an import ran, a listen was deleted — what would be removed
    /// is no longer what was shown, and `Ok(Err(now))` says how many there
    /// are now instead of removing them.
    ///
    /// Before a copy goes, whatever it knew that the kept play did not is
    /// filled in by the import's own merge rule (`blanks_to_fill`): a
    /// ListenBrainz copy carries a full `mbid_mapping`, so this is usually the
    /// recording, its ISRC and length, and — when both name the same album —
    /// the release and its cover. Never overwriting.
    /// A liner note on the copy moves to the kept play; if both have one, the
    /// pair is left alone.
    pub async fn apply_relayed_copies(
        &self,
        user_id: i64,
        expected: usize,
    ) -> Result<Result<RelayedApplied, usize>, sqlx::Error> {
        let rows = self.relay_candidates(user_id).await?;
        let pairs = pair_relayed_copies(&rows);
        if pairs.len() != expected {
            return Ok(Err(pairs.len()));
        }

        let noted: HashSet<i64> = sqlx::query_as::<_, (i64,)>(&crate::sql!(
            "SELECT scrobble_id FROM notes WHERE user_id = ? AND scrobble_id IS NOT NULL",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|(id,)| id)
        .collect();

        let mut applied = RelayedApplied { removed: 0, kept_for_notes: 0, notes_moved: 0 };
        let mut sessions: BTreeSet<i64> = BTreeSet::new();
        // One transaction per few hundred pairs: one per statement is what
        // makes a 20,000-row repair take minutes on a Pi's SD card, and one for
        // the lot holds the write lock for all of it.
        for chunk in pairs.chunks(500) {
            let mut tx = self.pool.begin().await?;
            for &(c, k) in chunk {
                let (copy, kept) = (&rows[c], &rows[k]);
                if noted.contains(&copy.id) && noted.contains(&kept.id) {
                    applied.kept_for_notes += 1;
                    continue;
                }
                // The same merge an import makes when it meets a listen it
                // holds: blanks only, and release facts only with their album.
                if let Some(known) = sparse_columns(&mut tx, copy.id).await? {
                    let what = play_from_sparse(&copy.title, &known);
                    fill_blanks(&mut tx, kept.id, &what, &ImportProvenance::default()).await?;
                }

                if noted.contains(&copy.id) {
                    sqlx::query(&crate::sql!(
                        "UPDATE notes SET scrobble_id = ? WHERE scrobble_id = ? AND user_id = ?",
                    ))
                    .bind(kept.id)
                    .bind(copy.id)
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await?;
                    applied.notes_moved += 1;
                }
                sqlx::query(&crate::sql!("DELETE FROM scrobble_artists WHERE scrobble_id = ?"))
                    .bind(copy.id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(&crate::sql!("DELETE FROM scrobbles WHERE id = ? AND user_id = ?"))
                    .bind(copy.id)
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await?;
                applied.removed += 1;
                sessions.extend(copy.session_id);
            }
            tx.commit().await?;
        }

        // A sitting's counts are recomputed from its listens, never adjusted,
        // and one left empty is deleted by the same call.
        for session_id in sessions {
            self.recompute_session(session_id).await?;
        }
        Ok(Ok(applied))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, ts: i64, title: &str, artist: &str, source: &str) -> RelayRow {
        RelayRow {
            id,
            timestamp: ts,
            title: title.into(),
            artist: artist.into(),
            duration: Some(203),
            source_name: source.into(),
            submission_client: Some(
                if source == "spotify" { "Spotify (windows)" } else { "import" }.into(),
            ),
            skipped: false,
            mbid_recording: None,
            isrc: None,
            spotify_track_id: (source == "spotify").then(|| "5QhBKPqsnX1uY9fZNaAtZg".into()),
            session_id: None,
        }
    }

    /// The real shape, off the household instance: a play stored from the
    /// export at its start, and ListenBrainz's copy at its end, 191s later.
    #[test]
    fn a_copy_stamped_at_the_end_of_its_play_is_found() {
        let rows = vec![
            row(1, 1713148049, "La grenade", "Clara Luciani", "spotify"),
            row(2, 1713148240, "La grenade", "Clara Luciani", "import"),
        ];
        assert_eq!(pair_relayed_copies(&rows), vec![(1, 0)]);
    }

    /// Six plays of "Stronger" back to back, one track length apart, with
    /// the copy of the last one — all the old importer stored, because each
    /// earlier copy had attached itself to the next play's start. It must
    /// pair with the play it is a copy of, and only one copy is removed.
    #[test]
    fn a_repeat_run_loses_exactly_its_one_copy() {
        let mut rows: Vec<RelayRow> =
            [1743753659, 1743753863, 1743754066, 1743754270, 1743754473, 1743754677]
                .iter()
                .enumerate()
                .map(|(i, &ts)| row(i as i64 + 1, ts, "Stronger", "Britney Spears", "spotify"))
                .collect();
        rows.push(row(7, 1743754880, "Stronger", "Britney Spears", "import"));
        assert_eq!(pair_relayed_copies(&rows), vec![(6, 5)]);
    }

    /// The old importer under the other order (archive first): every copy
    /// stored, each at its play's end, which is the next play's start. Each
    /// pairs with its own play, and none claims a play twice.
    #[test]
    fn copies_of_a_repeat_each_claim_their_own_play() {
        let rows = vec![
            row(1, 1000, "Stronger", "Britney Spears", "spotify"),
            row(2, 1203, "Stronger", "Britney Spears", "import"),
            row(3, 1203, "Stronger", "Britney Spears", "spotify"),
            row(4, 1406, "Stronger", "Britney Spears", "import"),
        ];
        assert_eq!(pair_relayed_copies(&rows), vec![(1, 0), (3, 2)]);
    }

    /// The two exports credit collaborations differently; that must not hide
    /// a copy. A different song of the same name must not be taken for one.
    #[test]
    fn the_artist_may_be_the_fuller_credit_but_not_someone_else() {
        let rows = vec![
            row(1, 5000, "Aila Aila", "A.R. Rahman", "spotify"),
            row(2, 5150, "Aila Aila", "A.R. Rahman, Aditya Rao, Natalie Ram", "import"),
            row(3, 9000, "Intro", "The xx", "spotify"),
            row(4, 9100, "Intro", "M83", "import"),
        ];
        assert_eq!(pair_relayed_copies(&rows), vec![(1, 0)]);
    }

    /// A ListenBrainz copy titled "Kokomo - 1995 Remaster" is still the play
    /// Spotify's export calls "Kokomo", when both carry the same recording.
    #[test]
    fn a_retitled_copy_is_found_by_its_recording() {
        let mut kept = row(1, 7000, "Kokomo", "The Beach Boys", "spotify");
        let mut copy = row(2, 7200, "Kokomo - 1995 Remaster", "The Beach Boys", "import");
        kept.mbid_recording = Some("rec-1".into());
        copy.mbid_recording = Some("REC-1".into());
        assert_eq!(pair_relayed_copies(&[kept, copy]), vec![(1, 0)]);
    }

    /// Accents and a version suffix are the same title; a different song by
    /// the same artist within the window is not.
    #[test]
    fn a_respelled_or_suffixed_title_is_the_same_play() {
        let rows = vec![
            row(1, 1000, "Éblouie par la nuit", "Zaz", "spotify"),
            row(2, 1190, "Eblouie par la nuit", "Zaz", "import"),
            row(3, 2000, "Forever Young", "Alphaville", "spotify"),
            row(4, 2200, "Forever Young - 2019 Remaster", "Alphaville", "import"),
            row(
                5,
                3000,
                "Writing's On The Wall - From \"Spectre\" Soundtrack",
                "Sam Smith",
                "spotify",
            ),
            row(6, 3200, "Writing's On The Wall", "Sam Smith", "import"),
            row(7, 4000, "Big in Japan", "Alphaville", "spotify"),
            row(8, 4200, "Forever Young", "Alphaville", "import"),
        ];
        assert_eq!(pair_relayed_copies(&rows), vec![(1, 0), (3, 2), (5, 4)]);
    }

    /// A partial play followed at once by a full one: each copy goes to its
    /// own play, though the first copy's window holds both starts.
    #[test]
    fn a_full_play_is_matched_before_a_partial_one() {
        let rows = vec![
            row(1, 0, "Song", "Artist", "spotify"),  // played 90s
            row(2, 90, "Song", "Artist", "import"),  // its copy
            row(3, 90, "Song", "Artist", "spotify"), // played in full
            row(4, 293, "Song", "Artist", "import"), // its copy
        ];
        assert_eq!(pair_relayed_copies(&rows), vec![(1, 0), (3, 2)]);
    }

    /// What is not a copy: a second listening of the song after the first one
    /// ended; anything the fixed importer wrote, which carries a client; and a
    /// live listen from another source, however close.
    #[test]
    fn real_listens_are_left_alone() {
        let mut fixed = row(3, 20_100, "Song", "Artist", "import");
        fixed.submission_client = Some("Pano Scrobbler".into());
        let rows = vec![
            row(1, 10_000, "Song", "Artist", "spotify"),
            row(2, 10_000 + 203 + 11, "Song", "Artist", "import"),
            row(4, 20_000, "Song", "Artist", "spotify"),
            fixed,
            row(5, 30_000, "Other", "Artist", "Plex"),
            row(6, 30_100, "Other", "Artist", "import"),
        ];
        assert!(pair_relayed_copies(&rows).is_empty());
    }

    /// Without a length on either side, a generous ten minutes stands in.
    #[test]
    fn an_unknown_length_falls_back_to_ten_minutes() {
        let mut kept = row(1, 0, "Long", "Artist", "spotify");
        let mut copy = row(2, 590, "Long", "Artist", "import");
        kept.duration = None;
        copy.duration = None;
        assert_eq!(pair_relayed_copies(&[kept.clone(), copy.clone()]), vec![(1, 0)]);
        copy.timestamp = 611;
        assert!(pair_relayed_copies(&[kept, copy]).is_empty());
    }
}
