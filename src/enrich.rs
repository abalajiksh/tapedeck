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
