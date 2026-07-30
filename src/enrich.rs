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
use crate::musicbrainz::MusicBrainzClient;

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
            Ok(meta) => meta
                .release_date
                .as_deref()
                .and_then(parse_year)
                .unwrap_or(0),
            Err(_) => 0,
        };
        if db.set_scrobble_release_year(id, year).await.is_ok() {
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
        if matches!(
            source_name.as_str(),
            "navidrome" | "Plex" | "plex" | "jellyfin" | "Jellyfin"
        ) {
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
        if db
            .set_scrobble_artwork(id, &url, found_id, found_mbid.as_deref())
            .await
            .is_ok()
        {
            done += 1;
        }
    }
    done
}

fn caa_url(release_mbid: &str, caa_id: i64) -> String {
    format!("https://coverartarchive.org/release/{release_mbid}/{caa_id}-250.jpg")
}
