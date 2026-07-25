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
use crate::musicbrainz::MusicBrainzClient;
use crate::language::{script_language, DEFAULT_LANGUAGE};

/// Classify up to `batch` un-languaged listens, using at most `mb_budget`
/// MusicBrainz lookups. Returns how many were classified this call. Rows that
/// need MusicBrainz but exceed the budget are left for the next call.
pub async fn backfill_languages(db: &Database, mb: &MusicBrainzClient, batch: i64, mb_budget: i64) -> usize {
    let rows = match db.scrobbles_needing_language(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, album, mbid) in rows {
        // Tier 1: script detection (instant, non-Latin).
        if let Some(lang) = script_language(&title, album.as_deref()) {
            if db.set_scrobble_language(id, lang).await.is_ok() { done += 1; }
            continue;
        }
        // Tier 2: MusicBrainz work-language (bounded), else default.
        match mbid.filter(|m| !m.is_empty()) {
            Some(m) if budget > 0 => {
                budget -= 1;
                let lang = mb.fetch_recording_language(&m).await.ok().flatten()
                    .unwrap_or_else(|| DEFAULT_LANGUAGE.to_string());
                if db.set_scrobble_language(id, &lang).await.is_ok() { done += 1; }
            }
            Some(_) => { /* has an MBID but budget spent — leave for the next pass */ }
            None => {
                if db.set_scrobble_language(id, DEFAULT_LANGUAGE).await.is_ok() { done += 1; }
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
pub async fn backfill_release_years(db: &Database, mb: &MusicBrainzClient, batch: i64, mb_budget: i64) -> usize {
    let rows = match db.scrobbles_needing_release_year(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, artist, album) in rows {
        if budget <= 0 { break; }
        budget -= 1;

        let (year, caa_id, caa_mbid) = match mb.fetch_metadata(&title, &artist, album.as_deref()).await {
            Ok(meta) => (
                meta.release_date.as_deref().and_then(parse_year).unwrap_or(0),
                meta.caa_id,
                meta.caa_release_mbid,
            ),
            Err(_) => (0, None, None),
        };
        if db.set_scrobble_release_year(id, year, caa_id, caa_mbid.as_deref()).await.is_ok() { done += 1; }
    }
    done
}

/// MusicBrainz dates are `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`.
fn parse_year(date: &str) -> Option<i32> {
    let y: i32 = date.get(..4)?.parse().ok()?;
    (1000..=2999).contains(&y).then_some(y)
}

/// Fill in cover art for listens that have none. Shares `fetch_metadata` with
/// the other backfills, so anything already in the MusicBrainz cache is free and
/// `mb_budget` only bounds live API calls.
///
/// A listen with no art available is recorded as `caa_id = -1` rather than left
/// NULL, so repeat passes move on instead of retrying the same misses forever;
/// the UI treats a negative id as "no artwork" and falls back to the initial.
pub async fn backfill_artwork(db: &Database, mb: &MusicBrainzClient, batch: i64, mb_budget: i64) -> usize {
    let rows = match db.scrobbles_needing_artwork(batch).await {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let mut done = 0usize;
    let mut budget = mb_budget;

    for (id, title, artist, album) in rows {
        if budget <= 0 { break; }
        budget -= 1;

        let (caa_id, caa_mbid) = match mb.fetch_metadata(&title, &artist, album.as_deref()).await {
            Ok(meta) => (meta.caa_id, meta.caa_release_mbid),
            Err(_) => (None, None),
        };
        let resolved = caa_id.unwrap_or(-1);
        if db.set_scrobble_artwork(id, Some(resolved), caa_mbid.as_deref()).await.is_ok() { done += 1; }
    }
    done
}
