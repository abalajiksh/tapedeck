//! Deciding the history: a background pass over listens the current
//! classifier has not seen, and the same pass run for one listener — without a
//! single lookup — when an override changes.
//!
//! **One decision per fact key, many listens.** A recording heard fifty times
//! is fetched and decided once; the other forty-nine copy the stored answer.
//! The budget counts API requests only — cache and mirror answers are free —
//! and when it runs out the pass keeps going through listens that need no
//! request (name-only ones, and ones already decided), so a slow API never
//! stalls the cheap part of the work.

use super::{
    apply_overrides, classify, role_from_name, ListenView, Outcome, PerformerRow, CLASSICAL,
    VERSION,
};
use crate::db::{Database, FacetListen, FacetOverride};
use crate::logging::OrWarn;
use crate::musicbrainz::{Fetched, MusicBrainzClient};
use std::collections::HashMap;

/// The engine's pass: `batch` listens, at most `budget` API requests.
pub async fn backfill(db: &Database, mb: &MusicBrainzClient, batch: i64, budget: i64) -> usize {
    let (done, left) = run(db, mb, None, batch, budget).await;
    dates(db, mb, batch, left).await;
    done
}

/// Artists' dates, with whatever budget the listens left: the 66 canon
/// composers straight from the list, everyone else by lookup.
async fn dates(db: &Database, mb: &MusicBrainzClient, batch: i64, mut budget: i64) {
    let wanted = db
        .artists_needing_dates(batch)
        .await
        .or_warn("classify: artists_needing_dates")
        .unwrap_or_default();
    for mbid in wanted {
        if let Some(c) = super::canon::by_mbid(&mbid) {
            let a = crate::classify::ArtistDates {
                name: c.name.to_string(),
                kind: Some("Person".into()),
                gender: Some(c.gender.to_string()).filter(|g| !g.is_empty()),
                country: Some(c.country.to_string()).filter(|g| !g.is_empty()),
                begin: Some(c.born_date.to_string()).filter(|g| !g.is_empty()),
                end: Some(c.died_date.to_string()).filter(|g| !g.is_empty()),
                ended: true,
            };
            let _ = db.store_artist_dates(&mbid, &a).await;
            continue;
        }
        match mb.artist_dates(&mbid, budget > 0).await {
            Ok(Fetched::Unasked) | Err(_) => continue,
            Ok(f) => {
                if f.asked() {
                    budget -= 1;
                }
                if let Some(a) = f.into_value() {
                    let _ = db.store_artist_dates(&mbid, &a).await;
                }
            }
        }
    }
}

/// Re-apply decided facts to one listener's requeued listens — after an
/// override, or a change of preference. Never asks the API: anything not yet
/// decided is left for [`backfill`].
pub async fn refresh_user(db: &Database, mb: &MusicBrainzClient, user_id: i64) -> usize {
    let mut total = 0;
    // Bounded: a listener with a requeued history of 60,000 listens is
    // caught up by the background pass instead of holding a request open.
    for _ in 0..10 {
        let (n, _) = run(db, mb, Some(user_id), 500, 0).await;
        total += n;
        if n == 0 {
            break;
        }
    }
    total
}

/// One pass over listens; returns how many were decided and the budget left.
async fn run(
    db: &Database,
    mb: &MusicBrainzClient,
    user: Option<i64>,
    batch: i64,
    budget: i64,
) -> (usize, i64) {
    let rows = db
        .listens_needing_facets(VERSION, user, batch)
        .await
        .or_warn("classify: listens_needing_facets")
        .unwrap_or_default();
    let mut budget = budget;
    let mut overrides: HashMap<i64, Vec<FacetOverride>> = HashMap::new();
    let mut decided: HashMap<String, (i64, String)> = HashMap::new();
    let mut done = 0;
    for row in rows {
        let key = row.fact_key();
        let (facets, piece_key) = match decided.get(&key) {
            Some(d) => d.clone(),
            None => {
                let stored =
                    db.stored_fact(&key, VERSION).await.or_warn("classify: stored_fact").flatten();
                let d = match stored {
                    Some(f) => (f.facets, f.piece_key),
                    None => {
                        let Some(o) = decide(mb, &row, &mut budget).await else { continue };
                        let inferred = infer_performers(db, &row, &o).await;
                        if db
                            .store_fact(&key, VERSION, &o, &inferred)
                            .await
                            .or_warn("classify: store_fact")
                            .is_none()
                        {
                            continue;
                        }
                        (o.facets, o.piece_key)
                    }
                };
                decided.insert(key.clone(), d.clone());
                d
            }
        };
        if let std::collections::hash_map::Entry::Vacant(e) = overrides.entry(row.user_id) {
            e.insert(
                db.facet_overrides(row.user_id)
                    .await
                    .or_warn("classify: facet_overrides")
                    .unwrap_or_default(),
            );
        }
        let artist_keys: Vec<String> =
            ListenView::new(&row.title, &row.artist, row.artist_credit.as_deref(), None, None)
                .credit_names
                .iter()
                .map(|n| Database::artist_key(n))
                .collect();
        let final_facets = apply_overrides(
            facets,
            &piece_key,
            row.album_key().as_deref(),
            &artist_keys,
            overrides.get(&row.user_id).map(Vec::as_slice).unwrap_or(&[]),
        );
        if db
            .set_listen_facets(row.id, &key, final_facets, VERSION)
            .await
            .or_warn("classify: set_listen_facets")
            .is_some()
        {
            done += 1;
        }
    }
    (done, budget)
}

/// Fetch what MusicBrainz knows and decide. `None` when a fact this listen
/// needs is not cached and may not be asked for now — it is tried again later.
async fn decide(mb: &MusicBrainzClient, row: &FacetListen, budget: &mut i64) -> Option<Outcome> {
    let present = |s: &Option<String>| {
        s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
    };

    macro_rules! fetch {
        ($call:expr) => {{
            match $call {
                Ok(f) => {
                    if f.asked() {
                        *budget -= 1;
                    }
                    match f {
                        Fetched::Unasked => return None,
                        other => other.into_value(),
                    }
                }
                // A failed request decides nothing; the listen comes round again.
                Err(_) => return None,
            }
        }};
    }

    let view = ListenView::new(
        &row.title,
        &row.artist,
        row.artist_credit.as_deref(),
        row.album.as_deref(),
        row.mbid_artist.as_deref(),
    );
    let rec = match present(&row.mbid_recording) {
        Some(m) => fetch!(mb.recording_facts(&m, *budget > 0).await),
        None => None,
    };
    let piece_mbid = rec.as_ref().and_then(|r| r.works.first()).map(|w| match &w.parent {
        Some(p) => p.mbid.clone(),
        None => w.mbid.clone(),
    });
    let piece = match piece_mbid.filter(|m| !m.is_empty()) {
        Some(m) => fetch!(mb.work_facts(&m, *budget > 0).await),
        None => None,
    };
    let release = match present(&row.mbid_release) {
        Some(m) => fetch!(mb.release_facts(&m, *budget > 0).await),
        None => None,
    };
    let group_mbid = release
        .as_ref()
        .and_then(|r| r.group_mbid.clone())
        .or_else(|| present(&row.mbid_release_group));
    let group = match group_mbid {
        Some(m) => fetch!(mb.group_facts(&m, *budget > 0).await),
        None => None,
    };
    Some(classify(&view, rec.as_ref(), piece.as_ref(), release.as_ref(), group.as_ref()))
}

/// Who played a classical listen nobody described: each credited name that is
/// not the composer, as the role MusicBrainz states for them elsewhere, else
/// as a name like "… Philharmonic" says. Names that say nothing are left out.
async fn infer_performers(db: &Database, row: &FacetListen, o: &Outcome) -> Vec<PerformerRow> {
    if o.facets & CLASSICAL == 0 || !o.performers.is_empty() {
        return Vec::new();
    }
    let view = ListenView::new(&row.title, &row.artist, row.artist_credit.as_deref(), None, None);
    let composer = o.composer_name.as_deref().map(super::canon::name_key);
    let mut out: Vec<PerformerRow> = Vec::new();
    for name in &view.credit_names {
        // The joined credit itself ("A, B") is not a person.
        if name.contains(", ") || super::canon::by_name(name).is_some() {
            continue;
        }
        if composer.as_deref() == Some(super::canon::name_key(name).as_str()) {
            continue;
        }
        let key = Database::artist_key(name);
        let stated = db.stated_role(&key).await.ok().flatten();
        let row = match stated {
            Some((role, instrument)) => Some(PerformerRow {
                name: name.clone(),
                mbid: None,
                role,
                instrument,
                country: None,
            }),
            None => role_from_name(name).map(|role| PerformerRow {
                name: name.clone(),
                mbid: None,
                role: role.to_string(),
                instrument: None,
                country: None,
            }),
        };
        if let Some(r) = row {
            if !out.iter().any(|p| p.name.eq_ignore_ascii_case(&r.name)) {
                out.push(r);
            }
        }
    }
    out
}
