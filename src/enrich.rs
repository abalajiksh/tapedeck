//! Backfills that fill in metadata the ingest path doesn't have: lyrics
//! language, and release year for the by-decade breakdown. Both are bounded per
//! call so the MusicBrainz rate limit is respected, and both are driven by the
//! engine's periodic tick.
//!
//! Language backfill: classify listens that don't have a language yet.
//!
//! Script detection is free and applied to everything; the MusicBrainz
//! work-language lookup is bounded per call (`mb_budget`) so we respect the API
//! rate limit. Shared by the engine's periodic tick and the manual "enrich now"
//! endpoint.

use crate::db::Database;
use crate::language::{script_language, DEFAULT_LANGUAGE};
use crate::metadata::ArtworkProvider;
use crate::models::Play;
use crate::musicbrainz::MusicBrainzClient;
use tracing::debug;

/// Classify up to `batch` un-languaged listens, using at most `mb_budget`
/// MusicBrainz lookups. Returns how many were classified this call. Rows that
/// need MusicBrainz but exceed the budget are left for the next call.
pub async fn backfill_languages(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_language(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, album, mbid) in rows {
        // Tier 1: script detection (instant, non-Latin).
        if let Some(lang) = script_language(&title, album.as_deref()) {
            if db.set_scrobble_language(id, lang).await.is_ok() {
                done += 1;
            }
            continue;
        }
        // Tier 2: MusicBrainz work-language (bounded), else default.
        match mbid.filter(|m| !m.is_empty()) {
            Some(m) if budget > 0 => {
                budget -= 1;
                let lang = mb
                    .fetch_recording_language(&m)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| DEFAULT_LANGUAGE.to_string());
                if db.set_scrobble_language(id, &lang).await.is_ok() {
                    done += 1;
                }
            }
            Some(_) => { /* has an MBID but budget spent — leave for the next pass */ }
            None => {
                if db.set_scrobble_language(id, DEFAULT_LANGUAGE).await.is_ok() {
                    done += 1;
                }
            }
        }
    }
    done
}

/// Resolve release years for up to `batch` listens, using at most `mb_budget`
/// MusicBrainz lookups. `fetch_metadata` checks its own SQLite cache first, so
/// a listen whose metadata was already fetched costs nothing.
///
/// A listen with no known release date is stored as year `0` rather than left
/// NULL — otherwise every pass would retry the same unresolvable rows forever
/// and never reach the rest. The decade query filters those out.
pub async fn backfill_release_years(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_release_year(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, artist, album) in rows {
        if budget <= 0 {
            break;
        }
        budget -= 1;

        let year = match mb.fetch_metadata(&title, &artist, album.as_deref()).await {
            Ok(meta) => meta.release_date.as_deref().and_then(parse_year).unwrap_or(0),
            Err(_) => 0,
        };
        if db.set_scrobble_release_year(id, year).await.is_ok() {
            done += 1;
        }
    }
    done
}

/// Fill in the ISRC for listens that have none.
///
/// Keyed on the **recording MBID**, so this only reaches listens MusicBrainz has
/// already identified — 90% of a real history, and the ones a playlist push is
/// most likely to want. A listen with no recording MBID is left alone rather
/// than name-searched: the other backfills already resolve those, and this pass
/// will pick them up once they have one.
///
/// One request per *recording*, not per listen, and cached — so a track played
/// fifty times costs one lookup and one write that covers all fifty rows.
///
/// `''` is written when MusicBrainz has no ISRC for the recording, which is a
/// real answer and a common one. Without that tombstone the row would be
/// re-queued forever against a rate-limited service.
///
/// One resolution updates **every** listen of the same track, because a song
/// played fifty times is fifty rows sharing one recording and one ISRC.
/// Give imported listens a recording MBID.
///
/// **The gap this closes.** A recording MBID is written in one place —
/// `pipeline::flush_pending` — and that only ever sees `status = 'pending'`
/// listens on their way to a sink. An imported listen is `'imported'`, so it
/// never got one, and nothing else wrote the column. On a 92%-imported history
/// that left most of the listening with no MBID *permanently* — and every other
/// backfill that keys off a recording (ISRC, work-language, release year) is
/// therefore excluded from it. This is the one that unblocks the others, which
/// is why it should run before them rather than beside them.
///
/// **A wrong MBID is worse than none, so a match is checked.** `fetch_metadata`
/// searches by name with `limit=1` against a ranking MusicBrainz does not
/// promise is stable — the trap already recorded for recording-level tags. An
/// unchecked result would write a confident wrong id onto every play of the
/// track and then poison the ISRC and language lookups downstream, none of
/// which would error. So the answer has to *agree* with the question:
/// `artist_matches` on the artist, and the same fold on the title after the
/// trailing qualifier is stripped, since a search for "Xtal" legitimately
/// answers "Xtal (Remastered)".
///
/// A miss costs nothing twice: `fetch_metadata` keeps its own negative cache,
/// and `tracks_needing_recording_mbid` reads it so a batch is not spent on
/// tracks that have already been asked about.
pub async fn backfill_recording_mbids(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    // Over-fetch, then drop the ones already asked about. That filter cannot
    // be a SQL join: `musicbrainz_cache` lives in the *client's* database
    // (`SQLITE_DB_PATH`), not the one holding the listens, so the two sets can
    // only meet in Rust. Without it every batch would be spent re-asking the
    // tracks MusicBrainz has already said it does not have, and the ones it
    // could answer would never come up.
    let rows = match db.tracks_needing_recording_mbid(batch * 4).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    if rows.is_empty() {
        return 0;
    }
    let asked = mb.negative_track_keys().await;
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|(t, a, _)| !asked.contains(&(t.trim().to_lowercase(), a.trim().to_lowercase())))
        .take(batch as usize)
        .collect();

    let mut done = 0usize;
    let mut budget = mb_budget;

    for (title, artist, album) in rows {
        if budget == 0 {
            break;
        }
        budget -= 1;
        // A carrier for the lookup, not a listen: nothing here is ever stored
        // as a play. `enrich_play` reads the title and artist and writes the
        // ids back onto it.
        let mut play =
            Play::new(title.clone(), artist.clone(), 0, String::new(), "mbid-backfill".to_string());
        if !crate::engine::enrichment::enrich_play(mb, &mut play, album.as_deref()).await {
            continue;
        }
        let Some(found) = play.mbid_recording.as_deref() else { continue };
        if found.trim().is_empty() {
            continue;
        }
        if !answers_the_question(&title, &artist, &play) {
            debug!(
                "MBID backfill: refused {} — {} (MusicBrainz answered a different track)",
                artist, title
            );
            continue;
        }
        match db.set_recording_mbid_for_track(&title, &artist, &play).await {
            Ok(n) if n > 0 => done += 1,
            _ => {}
        }
    }
    done
}

/// Does the answer describe the track that was asked about?
///
/// The gate is on the *names MusicBrainz returned*, which on a fresh lookup are
/// its own canonical spellings rather than the query. `artist_matches` is the
/// codebase's single answer to "same performer?"; the title is folded with
/// `normalise_artist`, which is the same question asked of a different field —
/// the reuse `playlists::pick` already makes — and the trailing qualifier is
/// stripped because a search for "Xtal" answering "Xtal (Remastered 2008)" is
/// the *right* recording under a longer name.
fn answers_the_question(title: &str, artist: &str, play: &Play) -> bool {
    if !crate::db::artist_matches(&play.artist, artist) {
        return false;
    }
    let fold = |t: &str| crate::db::normalise_artist(crate::playlists::without_qualifier(t));
    let (want, got) = (fold(title), fold(&play.title));
    !want.is_empty() && (want == got || got.starts_with(&want) || want.starts_with(&got))
}

pub async fn backfill_isrc(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_isrc(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (mbid, title, artist) in rows {
        if budget <= 0 {
            break;
        }
        budget -= 1;

        let isrc = match mb.fetch_recording_isrcs(&mbid).await {
            // The first is as good as any: several ISRCs on one recording are
            // the same performance issued in different territories, and each
            // one identifies it.
            Ok(list) => list.first().cloned().unwrap_or_default(),
            // A failed request is not an answer. Leaving the row NULL means it
            // is asked again next pass, where writing the tombstone would
            // retire it permanently on the strength of a network blip.
            Err(_) => continue,
        };
        if let Ok(n) = db.set_scrobble_isrc(&title, &artist, &isrc).await {
            if n > 0 {
                done += 1;
            }
        }
    }
    done
}

/// Fill in genres for listens that have none.
///
/// Costs nothing new: `fetch_metadata` has always returned genres and the cache
/// has always stored them — they were simply never written onto the listen, so
/// a history that had been fully enriched already has every answer sitting in
/// `mb_cache` and this pass resolves it without a single API call.
///
/// A listen MusicBrainz has no tags for is recorded as `''` rather than left
/// NULL, so it isn't re-asked forever — the same tombstone `artwork_url` and
/// `caa_id = -1` use.
pub async fn backfill_genres(
    db: &Database,
    mb: &MusicBrainzClient,
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_genres(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, artist, album) in rows {
        if budget <= 0 {
            break;
        }

        // The recording's own tags are the precise answer when they exist, and
        // cost nothing on an enriched history — this is the same cached lookup
        // the language and release-year passes already made.
        let mut genres = match mb.fetch_metadata(&title, &artist, album.as_deref()).await {
            Ok(meta) => meta.genres,
            Err(_) => Vec::new(),
        };

        // Most recordings have none. Fall back to the artist's tags, which are
        // both better populated and stable — see `fetch_artist_tags`. Cached
        // per artist, so a library of one artist's records costs one lookup,
        // not one per listen, and only a real API call spends budget.
        if genres.is_empty() {
            genres = match db.artist_genres(&artist).await {
                Ok(Some(cached)) => cached,
                _ => {
                    budget -= 1;
                    let fetched = mb.fetch_artist_tags(&artist).await.unwrap_or_default();
                    // Written even when empty, so an artist MusicBrainz has
                    // nothing for isn't re-asked on every pass.
                    let _ = db.set_artist_genres(&artist, &fetched).await;
                    fetched
                }
            };
        }

        if db.set_scrobble_genres(id, &genres).await.is_ok() {
            done += 1;
        }
    }
    done
}

/// MusicBrainz dates are `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`.
fn parse_year(date: &str) -> Option<i32> {
    let y: i32 = date.get(..4)?.parse().ok()?;
    (1000..=2999).contains(&y).then_some(y)
}

/// Fill in cover art for listens that have none, cheapest provider first.
///
/// Deezer is tried before MusicBrainz because it needs no auth and imposes no
/// one-per-second limit, so a large backfill finishes in minutes rather than
/// hours — and every hit is one fewer request aimed at a free public service.
/// MusicBrainz/Cover Art Archive remains the fallback for everything Deezer
/// doesn't carry, which is most of the obscure end of a collection.
///
/// A listen with nothing anywhere is tombstoned as `caa_id = -1` so later passes
/// move on instead of re-asking forever.
pub async fn backfill_artwork(
    db: &Database,
    mb: &MusicBrainzClient,
    providers: &[Box<dyn ArtworkProvider>],
    batch: i64,
    mb_budget: i64,
) -> usize {
    let rows = match db.scrobbles_needing_artwork(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, artist, album, caa_id, caa_mbid, source_name) in rows {
        // 0a. Came from the user's own media server — the proxy can serve the
        //     cover of the exact file that was played, with no lookup at all and
        //     nothing asked of a third party. Always the best answer available.
        // ponytail: written without checking the image resolves, and
        // `artwork_url` is this queue — so a library item with genuinely no
        // cover is stuck here and never reaches Deezer or MusicBrainz. Verify by
        // fetching once (or have the proxy's 404 path re-queue the row) if that
        // turns out to be common; media servers almost always have album art.
        if matches!(source_name.as_str(), "navidrome" | "Plex" | "plex" | "jellyfin" | "Jellyfin") {
            let url = format!("/api/v1/art/{id}");
            if db.set_scrobble_artwork(id, &url, None, None).await.is_ok() {
                done += 1;
                continue;
            }
        }

        // 0b. Already have a Cover Art Archive id from earlier enrichment — build
        //    the URL from it rather than asking anyone anything.
        if let (Some(cid), Some(mbid)) = (caa_id.filter(|c| *c > 0), caa_mbid.as_deref()) {
            let url = caa_url(mbid, cid);
            if db.set_scrobble_artwork(id, &url, None, None).await.is_ok() {
                done += 1;
            }
            continue;
        }

        // 1. Providers that cost MusicBrainz nothing (Deezer today; the user's
        //    own library once Phase 4 finishes).
        if let Some(found) =
            crate::metadata::resolve(providers, &artist, &title, album.as_deref()).await
        {
            // Providers hand back an artist portrait alongside the cover; it has
            // nowhere to live on a listen, so it goes to the artist entity.
            if let Some(ref portrait) = found.artist_image_url {
                let _ = db.upsert_artist(&artist, Some(portrait), None).await;
            }
            if let Some(url) = found.image_url {
                if db.set_scrobble_artwork(id, &url, None, None).await.is_ok() {
                    done += 1;
                    continue;
                }
            }
        }

        // 2. MusicBrainz + Cover Art Archive, bounded by the rate limit.
        if budget <= 0 {
            break;
        }
        budget -= 1;

        let (found_id, found_mbid) =
            match mb.fetch_metadata(&title, &artist, album.as_deref()).await {
                Ok(meta) => (meta.caa_id, meta.caa_release_mbid),
                Err(_) => (None, None),
            };
        // Empty string tombstones "looked everywhere, nothing there".
        let url = match (found_id.filter(|c| *c > 0), found_mbid.as_deref()) {
            (Some(cid), Some(mbid)) => caa_url(mbid, cid),
            _ => String::new(),
        };
        if db.set_scrobble_artwork(id, &url, found_id, found_mbid.as_deref()).await.is_ok() {
            done += 1;
        }
    }
    done
}

fn caa_url(release_mbid: &str, caa_id: i64) -> String {
    format!("https://coverartarchive.org/release/{release_mbid}/{caa_id}-250.jpg")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(title: &str, artist: &str) -> Play {
        let mut p = Play::new(title.into(), artist.into(), 0, String::new(), "t".into());
        p.mbid_recording = Some("some-mbid".into());
        p
    }

    /// The gate on what the MBID backfill is allowed to write.
    ///
    /// `fetch_metadata` searches by name with `limit=1` against a ranking
    /// MusicBrainz does not promise is stable. Accepting whatever comes back
    /// would write a confident wrong id onto **every play** of a track and then
    /// poison the ISRC and work-language lookups that key off it — none of
    /// which would error. The answer has to agree with the question.
    #[test]
    fn a_backfilled_mbid_has_to_answer_the_question_that_was_asked() {
        // The ordinary case.
        assert!(answers_the_question("Xtal", "Aphex Twin", &answered("Xtal", "Aphex Twin")));

        // A longer name for the same recording is the *right* answer: a search
        // for "Xtal" legitimately comes back with the remaster.
        assert!(answers_the_question(
            "Xtal",
            "Aphex Twin",
            &answered("Xtal (Remastered 2008)", "Aphex Twin")
        ));

        // Spelling and punctuation fold, the way they do everywhere else.
        assert!(answers_the_question(
            "Coeur de pirate",
            "Coeur de pirate",
            &answered("Cœur de pirate", "Cœur de pirate")
        ));

        // A different song by the right artist is the failure that matters —
        // it looks entirely plausible and is wrong on every play of the track.
        assert!(!answers_the_question("Xtal", "Aphex Twin", &answered("Ageispolis", "Aphex Twin")));

        // And a different artist, which is what a fuzzy search returns when it
        // has nothing: the trap `find_artist` already documents.
        assert!(!answers_the_question("Xtal", "Aphex Twin", &answered("Xtal", "Arkells")));

        // Nothing to compare against is a refusal, not a pass.
        assert!(!answers_the_question("", "Aphex Twin", &answered("Xtal", "Aphex Twin")));
    }
}
