//! Records and tapes: a shelf you catalogue, and sides you play.
//!
//! The unit here is a **side**, because that is the unit you physically handle —
//! you put one on, it plays through, you flip it. Playing a side writes one
//! ordinary `scrobbles` row per track, not one row for the side: every query in
//! this app counts listens, and a side-shaped row would be invisible to all of
//! them. The timestamps are *derived* from the side's running order, which is
//! the one thing here that is inferred rather than observed — see `play_side`.
//!
//! Alongside the listens, a `physical_plays` row records the physical act. That
//! is what stylus hours and a cassette counter are computed from, and it is
//! deliberately separate: a listen is about the music, this is about the object
//! and the equipment wearing out.

use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::now_playing::{NowPlaying, Progress};
use super::AppState;
use crate::db::PhysicalImage;
use crate::metadata::release_lookup::{self, PhysicalTrack};
use crate::models::{ListenDisposition, Play};

/// Where an admin-set Discogs token lives.
pub const DISCOGS_TOKEN_KEY: &str = "discogs_token";

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

/// Where an admin-set consumer key/secret pair lives.
pub const DISCOGS_KEY_KEY: &str = "discogs_key";
pub const DISCOGS_SECRET_KEY: &str = "discogs_secret";

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// The configured Discogs credential, if there is one.
///
/// Database first, environment second — the env var is the deployment-time
/// answer (matching how the Apple Music token is supplied) and the setting is
/// the one an admin can paste in without a restart.
///
/// Two credential shapes are accepted because Discogs issues two. Registering
/// an application gives a **consumer key and secret**, which is what the
/// developer settings page leads with; generating a **personal access token**
/// is a separate button on the same page and easy to miss. Either works for
/// reading the public database, so refusing one of them would just be a way to
/// look broken.
///
/// A token wins when both are set: it is the narrower credential, scoped to one
/// account rather than to an application.
async fn discogs_auth(state: &AppState) -> Option<release_lookup::DiscogsAuth> {
    if let Ok(Some(t)) = state.db.get_app_setting(DISCOGS_TOKEN_KEY).await {
        if !t.trim().is_empty() {
            return Some(release_lookup::DiscogsAuth::Token(t.trim().to_string()));
        }
    }
    let stored_key = state.db.get_app_setting(DISCOGS_KEY_KEY).await.ok().flatten();
    let stored_secret = state.db.get_app_setting(DISCOGS_SECRET_KEY).await.ok().flatten();
    if let (Some(k), Some(s)) = (&stored_key, &stored_secret) {
        if !k.trim().is_empty() && !s.trim().is_empty() {
            return Some(release_lookup::DiscogsAuth::KeySecret {
                key: k.trim().to_string(),
                secret: s.trim().to_string(),
            });
        }
    }

    if let Some(t) = env_nonempty("TAPEDECK_DISCOGS_TOKEN") {
        return Some(release_lookup::DiscogsAuth::Token(t));
    }
    // The namespaced names first. `Consumer_Key`/`Consumer_Secret` are what
    // Discogs' own docs call them and what people therefore export, but they
    // are generic enough to belong to some other application on the same host —
    // so they are accepted as a fallback rather than looked for first.
    let key = env_nonempty("TAPEDECK_DISCOGS_KEY").or_else(|| env_nonempty("Consumer_Key"));
    let secret =
        env_nonempty("TAPEDECK_DISCOGS_SECRET").or_else(|| env_nonempty("Consumer_Secret"));
    match (key, secret) {
        (Some(key), Some(secret)) => Some(release_lookup::DiscogsAuth::KeySecret { key, secret }),
        _ => None,
    }
}

// ════════════════════════════════════════════════════════════
//  Looking up what you're holding
// ════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
pub struct LookupQuery {
    /// Scanned off the sleeve or the spine.
    pub barcode: Option<String>,
    /// The label's catalogue number, which is on the spine when the barcode
    /// isn't — and is the only identifier on most pre-1980 pressings.
    pub catalog_number: Option<String>,
    pub artist: Option<String>,
    pub release: Option<String>,
    /// `vinyl`, `cassette`, `cd` or `sacd`; narrows the format filter.
    pub kind: Option<String>,
}

/// GET /api/v1/physical/lookup — find the pressing.
async fn lookup(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Query(q): Query<LookupQuery>,
) -> impl IntoResponse {
    let barcode = q.barcode.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let catno = q.catalog_number.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let artist = q.artist.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let release = q.release.as_deref().map(str::trim).filter(|s| !s.is_empty());

    if barcode.is_none() && catno.is_none() && artist.is_none() && release.is_none() {
        return err(400, "Give a barcode, a catalogue number, or an artist and release");
    }

    let auth = discogs_auth(&state).await;
    let mut candidates = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // Discogs first when a barcode is in hand and a credential is configured:
    // it catalogues *pressings*, so a barcode usually lands on the exact
    // edition, where MusicBrainz will often only have "the album".
    if let (Some(auth), Some(barcode)) = (auth.as_ref(), barcode) {
        match release_lookup::discogs_search(auth, Some(barcode), None).await {
            Ok(mut hits) => candidates.append(&mut hits),
            Err(e) => {
                warn!("Discogs barcode lookup failed: {}", e);
                notes.push(format!("Discogs: {e}"));
            }
        }
    }

    // MusicBrainz needs no credential, so it always runs — it is the only path
    // on an instance with no Discogs token, which is the default state.
    let format_filter = match q.kind.as_deref() {
        Some("vinyl") => "format:Vinyl",
        Some("cassette") => "format:Cassette",
        // SACD is a CD-family format in MusicBrainz and most hybrid discs are
        // catalogued as both, so asking for either finds the pressing.
        Some("cd") => "(format:CD OR format:SACD)",
        Some("sacd") => "(format:SACD OR format:CD)",
        _ => "(format:Vinyl OR format:Cassette OR format:CD OR format:SACD)",
    };
    let mb_query = if let Some(b) = barcode {
        format!("barcode:{b}")
    } else if let Some(c) = catno {
        format!("catno:\"{c}\" AND {format_filter}")
    } else {
        let mut parts = Vec::new();
        if let Some(a) = artist {
            parts.push(format!("artist:\"{a}\""));
        }
        if let Some(r) = release {
            parts.push(format!("release:\"{r}\""));
        }
        parts.push(format_filter.to_string());
        parts.join(" AND ")
    };

    match release_lookup::musicbrainz_search(&mb_query, 5).await {
        Ok(mut hits) => candidates.append(&mut hits),
        Err(e) => {
            warn!("MusicBrainz lookup failed: {}", e);
            notes.push(format!("MusicBrainz: {e}"));
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "candidates": candidates,
            "discogs_enabled": auth.is_some(),
            "notes": notes,
        })),
    )
        .into_response()
}

/// GET /api/v1/physical/discogs/:id — one Discogs release, with its tracklist.
///
/// Separate from search because the search endpoint carries no tracklist:
/// browsing ten results would otherwise be ten extra calls against a
/// rate-limited API for nine releases nobody picked.
async fn discogs_detail(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(auth) = discogs_auth(&state).await else {
        return err(400, "Discogs isn't configured on this server");
    };
    match release_lookup::discogs_release(&auth, &id).await {
        Ok(Some(c)) => {
            (StatusCode::OK, Json(serde_json::json!({ "candidate": c }))).into_response()
        }
        Ok(None) => err(404, "No such Discogs release"),
        Err(e) => err(502, e.to_string()),
    }
}

// ════════════════════════════════════════════════════════════
//  The shelf
// ════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
pub struct ShelfQuery {
    pub kind: Option<String>,
}

pub async fn list_shelf(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ShelfQuery>,
) -> impl IntoResponse {
    let kind = q.kind.as_deref().filter(|k| matches!(*k, "vinyl" | "cassette" | "cd" | "sacd"));
    match state.db.list_physical(user.user_id, kind).await {
        Ok(rows) => (StatusCode::OK, Json(serde_json::json!({ "releases": rows }))).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

async fn get_one(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.get_physical(user.user_id, id).await {
        Ok(Some(r)) => (StatusCode::OK, Json(serde_json::json!({ "release": r }))).into_response(),
        Ok(None) => err(404, "No such record"),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct NewRelease {
    pub kind: String,
    pub artist: String,
    pub title: String,
    pub year: Option<i64>,
    pub mbid_release: Option<String>,
    pub discogs_id: Option<String>,
    pub barcode: Option<String>,
    pub catalog_number: Option<String>,
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub tracks: Vec<PhysicalTrack>,
    pub notes: Option<String>,
    /// The medium the *release* declares — `"CD"`, `"12\" Vinyl"`, `"Hybrid
    /// SACD"` — as the lookup returned it. When present it **overrides `kind`**,
    /// because `kind` is what the user picked before anyone looked at the
    /// record and this is what the record says it is. See `settled_kind`.
    #[serde(default)]
    pub format: Option<String>,
    /// When you bought it, unix seconds. Optional — most of a shelf is
    /// catalogued long after the fact and guessing would be worse than blank.
    pub purchased_at: Option<i64>,
    /// Pull the sleeve, labels and booklet from the Cover Art Archive as part
    /// of adding it. On by default: it is one request and it is the whole
    /// reason for shelving from MusicBrainz rather than typing it in.
    #[serde(default = "yes")]
    pub fetch_images: bool,
}

fn yes() -> bool {
    true
}

async fn add_release(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<NewRelease>,
) -> impl IntoResponse {
    if !matches!(body.kind.as_str(), "vinyl" | "cassette" | "cd" | "sacd") {
        return err(400, "kind must be vinyl, cassette, cd or sacd");
    }
    if body.artist.trim().is_empty() || body.title.trim().is_empty() {
        return err(400, "An artist and a title are required");
    }
    let tracks = serde_json::to_string(&body.tracks).unwrap_or_else(|_| "[]".into());

    // What shelf it really goes on.
    //
    // The picker is what the user thought they were adding, chosen *before*
    // anyone looked at the record; `format` is what the release says it is, and
    // the release is the authority — the same reasoning `is_disc` already
    // follows for side splitting. A five-CD box set looked up by barcode could
    // otherwise be filed as vinyl with 76 tracks split into two sides, which is
    // exactly how one was.
    //
    // Silent when they agree, which is nearly always. When they disagree the
    // response says so, because quietly overriding what someone picked is its
    // own kind of wrong — they can move it back with the same picker.
    let declared = release_lookup::kind_from_format(body.format.as_deref());
    let kind = declared.unwrap_or(body.kind.as_str());
    let corrected = declared.filter(|k| *k != body.kind.as_str());

    let id = match state
        .db
        .create_physical(
            user.user_id,
            kind,
            body.artist.trim(),
            body.title.trim(),
            body.year,
            body.mbid_release.as_deref(),
            body.discogs_id.as_deref(),
            body.barcode.as_deref(),
            body.catalog_number.as_deref(),
            body.artwork_url.as_deref(),
            &tracks,
            body.notes.as_deref(),
            body.purchased_at,
        )
        .await
    {
        Ok(id) => id,
        Err(e) => return err(500, e.to_string()),
    };
    info!("💿 Added {} to the shelf: {} — {}", kind, body.artist, body.title);
    if let Some(k) = corrected {
        info!("   (filed as {k} rather than {} — the release says so)", body.kind);
    }

    // Awaited rather than spawned, and failing softly. Awaited because the next
    // thing the user does is open the record, and a gallery that fills in
    // thirty seconds later reads as broken; softly because a record you own is
    // still on the shelf whether or not an archive has scans of it.
    let mut images = 0usize;
    if body.fetch_images {
        if let Some(mbid) = body.mbid_release.as_deref() {
            match store_archive_images(&state, user.user_id, id, mbid).await {
                Ok(n) => images = n,
                Err(e) => warn!("Could not fetch cover art for {}: {}", mbid, e),
            }
        }
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "images": images,
            "kind": kind,
            // Only when it differs from what was asked for, so a client can say
            // "filed as CD — the release says so" rather than silently landing
            // it somewhere else.
            "corrected_from": corrected.map(|_| body.kind.clone()),
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct ReleaseUpdate {
    /// `Option<Option<_>>` throughout, for the reason `bulk-chain` learned the
    /// hard way: a bare `Option` reads a *misspelled* key as an explicit null,
    /// so a typo would clear the field it was trying to set and answer 200.
    /// Absent leaves it alone; an explicit null clears it.
    #[serde(default, deserialize_with = "explicit")]
    pub purchased_at: Option<Option<i64>>,
    #[serde(default, deserialize_with = "explicit")]
    pub chain_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "explicit")]
    pub equipment_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "explicit")]
    pub notes: Option<Option<String>>,
    /// The variant of `chain_id` this pressing is normally played on — the
    /// cartridge you keep fitted for it.
    #[serde(default, deserialize_with = "explicit")]
    pub chain_variant_id: Option<Option<i64>>,
    /// Move it to another shelf. A plain `Option` on purpose — a kind has no
    /// "cleared" state, so absent is the only thing `None` can mean.
    ///
    /// Changing this re-derives the tracklist's sides, because what a side *is*
    /// depends on the medium: a record has a break in it that `assign_sides`
    /// has to guess, and a disc has none at all. Leaving the old split behind
    /// would have the shelf offer to remind you to flip a CD.
    #[serde(default)]
    pub kind: Option<String>,
}

/// The shelf's formats. Mirrors the CHECK on `physical_releases.kind` — a value
/// outside this set is a 400 rather than a constraint violation surfaced as a
/// 500, and the CHECK stays as the backstop.
pub const SHELF_KINDS: [&str; 4] = ["vinyl", "cassette", "cd", "sacd"];

/// Keep an explicit `null` distinguishable from an omitted field.
fn explicit<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// PATCH /api/v1/physical/:id — the shelf-keeping fields.
async fn update_release(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ReleaseUpdate>,
) -> impl IntoResponse {
    // A chain or a piece of gear arriving in a body is checked against the
    // *owner's* rows before it is stored, the same rule ingest follows for a
    // token's default chain.
    if let Some(Some(cid)) = body.chain_id {
        match state.db.get_chain(user.user_id, cid).await {
            Ok(Some(_)) => {}
            Ok(None) => return err(400, "No such signal chain"),
            Err(e) => return err(500, e.to_string()),
        }
    }
    if let Some(Some(eid)) = body.equipment_id {
        match state.db.get_equipment(user.user_id).await {
            Ok(gear) if gear.iter().any(|g| g.id == eid) => {}
            Ok(_) => return err(400, "No such equipment"),
            Err(e) => return err(500, e.to_string()),
        }
    }

    let kind = body.kind.as_deref().map(|k| k.trim().to_ascii_lowercase());
    if let Some(ref k) = kind {
        if !SHELF_KINDS.contains(&k.as_str()) {
            return err(400, format!("kind must be one of {}", SHELF_KINDS.join(", ")));
        }
    }

    // What the sides were before, so we only rewrite them when the medium
    // actually changed. Re-splitting a record that is still a record would
    // discard a hand-edited side break for nothing.
    let previous_kind = match state.db.get_physical(user.user_id, id).await {
        Ok(Some(r)) => r.kind,
        Ok(None) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
    };

    match state
        .db
        .update_physical(
            user.user_id,
            id,
            body.purchased_at,
            body.chain_id,
            body.equipment_id,
            body.notes.as_ref().map(|n| n.as_deref()),
            kind.as_deref(),
            body.chain_variant_id,
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
    }

    let mut resplit = false;
    if let Some(ref k) = kind {
        if *k != previous_kind {
            match respread_sides(&state, user.user_id, id, k).await {
                Ok(did) => resplit = did,
                Err(e) => return err(500, e),
            }
        }
    }

    (StatusCode::OK, Json(serde_json::json!({ "status": "ok", "sides_rewritten": resplit })))
        .into_response()
}

/// Re-derive a tracklist's sides after the medium changed.
///
/// A disc plays start to finish, so its "sides" are disc numbers; a record or a
/// tape has a break that `assign_sides` places by playing time. Moving a
/// release between the two without redoing this leaves A/B lettering on a CD,
/// which is what makes the shelf offer to remind you to flip it.
///
/// The tracks keep their order and their titles — only `side` is rewritten,
/// because the order is a fact about the release and the split is a property of
/// the medium.
async fn respread_sides(
    state: &Arc<AppState>,
    user_id: i64,
    id: i64,
    kind: &str,
) -> Result<bool, String> {
    let Some(release) = state.db.get_physical(user_id, id).await.map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let mut tracks: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();
    if tracks.is_empty() {
        return Ok(false);
    }

    // `is_disc` is written against a *format* string; "sacd" and "cd" are the
    // two shelf kinds it recognises, and both are things you play through.
    let sides: Vec<String> = if release_lookup::is_disc(Some(kind)) {
        release_lookup::disc_sides(&[tracks.len()])
    } else {
        // Nothing states a side letter here — a tracklist that had one lost it
        // when it was filed as a disc — so this is the playing-time guess, the
        // same one the lookup makes. It is editable afterwards.
        let numbers: Vec<String> = (1..=tracks.len()).map(|n| n.to_string()).collect();
        let durations: Vec<Option<i64>> = tracks.iter().map(|t| t.duration_secs).collect();
        release_lookup::assign_sides(&numbers, &durations, 2)
    };

    // Position restarts within each side, which is what the side letter means.
    let mut seen: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for (t, side) in tracks.iter_mut().zip(sides) {
        let n = seen.entry(side.clone()).or_insert(0);
        *n += 1;
        t.position = *n;
        t.side = side;
    }

    let json = serde_json::to_string(&tracks).map_err(|e| e.to_string())?;
    state.db.set_physical_tracks(user_id, id, &json).await.map_err(|e| e.to_string())?;
    Ok(true)
}

#[derive(Debug, Deserialize)]
pub struct TracksUpdate {
    pub tracks: Vec<PhysicalTrack>,
}

/// PUT /api/v1/physical/:id/tracks — save a corrected side split.
///
/// The one edit that matters, because the split Tapedeck guessed is a guess:
/// MusicBrainz only records a side letter when an editor entered one.
async fn set_tracks(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<TracksUpdate>,
) -> impl IntoResponse {
    let json = serde_json::to_string(&body.tracks).unwrap_or_else(|_| "[]".into());
    match state.db.set_physical_tracks(user.user_id, id, &json).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Ok(false) => err(404, "No such record"),
        Err(e) => err(500, e.to_string()),
    }
}

async fn delete_release(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_physical(user.user_id, id).await {
        Ok(Some(files)) => {
            // The rows are already gone, so a file that refuses to delete is
            // litter rather than a failure — reporting a 500 here would say the
            // record is still on the shelf, which it is not.
            for f in files {
                if let Err(e) = tokio::fs::remove_file(state.uploads_dir.join(&f)).await {
                    warn!("Could not remove the scan at {}: {}", f, e);
                }
            }
            (StatusCode::OK, Json(serde_json::json!({ "status": "deleted" }))).into_response()
        }
        Ok(None) => err(404, "No such record"),
        Err(e) => err(500, e.to_string()),
    }
}

// ════════════════════════════════════════════════════════════
//  Everything else in the sleeve
// ════════════════════════════════════════════════════════════

/// What the browser is told about one image.
///
/// Built by hand rather than serialised straight off the row so that
/// `file_path` cannot reach the page: an uploaded scan is addressed by a
/// Tapedeck URL, and the server's own layout is nobody's business. The row type
/// also skips the field, so this is belt and braces on the thing that matters.
fn image_payload(img: &PhysicalImage) -> serde_json::Value {
    let own = format!("/api/v1/physical/images/{}/file", img.id);
    serde_json::json!({
        "id": img.id,
        "release_id": img.release_id,
        "kind": img.kind,
        "provider": img.provider,
        // One field the page renders, whichever provenance it came from.
        "src": img.url.clone().unwrap_or_else(|| own.clone()),
        // A remote provider's thumbnail, or the full file for an upload —
        // scans are the user's own and there is nothing else to serve.
        "thumb": img.thumb_url.clone().or_else(|| img.url.clone()).unwrap_or(own),
        "caption": img.caption,
        "position": img.position,
        "bytes": img.bytes,
        "mime": img.mime,
        "added_at": img.added_at,
    })
}

/// GET /api/v1/physical/:id/sides — how often each side has been played.
///
/// A shelf item's totals answer "how much has this record been played"; this
/// answers the question the object actually raises, which is that a record has
/// a side you reach for and a side you rarely turn over.
///
/// Reports only the sides that have plays. The page holds the tracklist and
/// zero-fills from it, which is what lets a side that has never been played
/// say so — and it is also how a side recorded under a *different* label shows
/// up: re-filing a release rewrites its sides (a vinyl A/B becomes a CD's 1),
/// and the plays keep the letter they were recorded with. Dropping those here
/// would leave the sides summing to less than the release's own total with
/// nothing to say why.
async fn side_plays(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Ownership before the stats: scoping the query alone would answer an
    // empty list for somebody else's record, which reads as "never played"
    // rather than as "not yours".
    match state.db.get_physical(user.user_id, id).await {
        Ok(None) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
        Ok(Some(_)) => {}
    }
    match state.db.side_plays(user.user_id, id).await {
        Ok(sides) => (StatusCode::OK, Json(serde_json::json!({ "sides": sides }))).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

/// GET /api/v1/physical/:id/images
async fn list_images(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.list_physical_images(user.user_id, id).await {
        Ok(rows) => {
            let images: Vec<_> = rows.iter().map(image_payload).collect();
            (StatusCode::OK, Json(serde_json::json!({ "images": images }))).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// Pull everything the Cover Art Archive holds for a release and store it.
///
/// Idempotent by the archive's own image id, so re-running this on a release
/// that has gained a scan adds the one and leaves the other twenty-eight alone.
/// The error is flattened to a `String` the moment it arrives, and that is not
/// cosmetic: a `Box<dyn Error>` is not `Send`, and holding one across the
/// awaits below makes the whole handler futures fail axum's `Handler` bound
/// with an error message that names neither.
async fn store_archive_images(
    state: &AppState,
    user_id: i64,
    release_id: i64,
    mbid: &str,
) -> Result<usize, String> {
    let images = release_lookup::cover_art_archive(mbid).await.map_err(|e| e.to_string())?;
    let mut n = 0;
    for img in &images {
        if state
            .db
            .add_physical_image(
                user_id,
                release_id,
                &img.kind,
                "caa",
                Some(&img.url),
                img.thumb_url.as_deref(),
                None,
                None,
                None,
                img.caption.as_deref(),
                img.position,
                Some(&img.id),
            )
            .await
            .is_ok()
        {
            n += 1;
        }
    }
    Ok(n)
}

/// POST /api/v1/physical/:id/images/fetch — ask the providers again.
///
/// Runs when the user asks rather than on a clock: the archive is not going to
/// change under a record that has been on a shelf for thirty years, and the one
/// case that does — an editor uploading the booklet you were missing — is
/// exactly the case where the user knows and Tapedeck cannot.
async fn fetch_images(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };

    let mut added = 0usize;
    let mut notes: Vec<String> = Vec::new();

    match release.mbid_release.as_deref() {
        Some(mbid) => match store_archive_images(&state, user.user_id, id, mbid).await {
            Ok(n) => added += n,
            Err(e) => notes.push(format!("Cover Art Archive: {e}")),
        },
        // Not an error, and worth saying rather than showing an empty gallery:
        // a record typed in by hand has no MusicBrainz id to ask about.
        None => notes.push(
            "This pressing has no MusicBrainz id, so there is nothing to look its sleeve up by. \
             Uploaded scans still work."
                .into(),
        ),
    }

    // Discogs second and only if configured, because it serves images only to
    // an authenticated caller and says nothing useful about what they *are* —
    // "secondary" covers a back cover and a booklet page equally.
    if let (Some(auth), Some(did)) = (discogs_auth(&state).await, release.discogs_id.as_deref()) {
        match release_lookup::discogs_images(&auth, did).await.map_err(|e| e.to_string()) {
            Ok(images) => {
                for img in &images {
                    if state
                        .db
                        .add_physical_image(
                            user.user_id,
                            id,
                            &img.kind,
                            "discogs",
                            Some(&img.url),
                            img.thumb_url.as_deref(),
                            None,
                            None,
                            None,
                            None,
                            // After the archive's, so a gallery holding both
                            // does not interleave them.
                            1000 + img.position,
                            Some(&img.id),
                        )
                        .await
                        .is_ok()
                    {
                        added += 1;
                    }
                }
            }
            Err(e) => notes.push(format!("Discogs: {e}")),
        }
    }

    let total = state.db.list_physical_images(user.user_id, id).await.map(|r| r.len()).unwrap_or(0);
    (StatusCode::OK, Json(serde_json::json!({ "stored": added, "total": total, "notes": notes })))
        .into_response()
}

/// The largest a single scan may be. A 600dpi A4 page is a few megabytes; this
/// leaves room for a gatefold without letting one upload fill the disk.
const MAX_SCAN_BYTES: usize = 24 * 1024 * 1024;

/// Extension for a declared image type. The list is exhaustive on purpose —
/// the extension is part of a path this server writes, so it is chosen from a
/// fixed set here rather than taken from the client's filename.
pub(super) fn image_ext(mime: &str) -> Option<&'static str> {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/avif" => Some("avif"),
        "image/tiff" => Some("tif"),
        _ => None,
    }
}

/// POST /api/v1/physical/:id/images — upload a scan of your own copy.
///
/// The one image source that is not somebody else's database. A pressing
/// nobody has photographed, a sleeve with your own handwriting on it, the
/// receipt tucked inside — none of that exists anywhere but here.
async fn upload_image(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let Ok(Some(_)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };

    let mut kind = "other".to_string();
    let mut caption: Option<String> = None;
    let mut files: Vec<(Vec<u8>, String)> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "kind" => {
                if let Ok(v) = field.text().await {
                    let v = v.trim().to_lowercase();
                    if !v.is_empty() {
                        kind = v;
                    }
                }
            }
            "caption" => {
                caption =
                    field.text().await.ok().map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
            }
            _ => {
                // The declared type decides the extension, and a filename from
                // the client never touches the path.
                let declared = field.content_type().unwrap_or("").to_string();
                let named = field.file_name().unwrap_or("").to_string();
                let mime = if image_ext(&declared).is_some() {
                    declared
                } else {
                    mime_guess::from_path(&named).first_raw().unwrap_or("").to_string()
                };
                if let Ok(b) = field.bytes().await {
                    files.push((b.to_vec(), mime));
                }
            }
        }
    }

    if files.is_empty() {
        return err(400, "No image in the request");
    }

    // Where the next one goes. A booklet is uploaded a page at a time, and
    // appending keeps that order without asking the user to number them.
    let mut position = state
        .db
        .list_physical_images(user.user_id, id)
        .await
        .map(|r| r.iter().map(|i| i.position).max().unwrap_or(-1) + 1)
        .unwrap_or(0);

    let dir = state.uploads_dir.join("shelf").join(user.user_id.to_string()).join(id.to_string());
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return err(500, format!("Could not write to the uploads directory: {e}"));
    }

    let mut stored = Vec::new();
    for (bytes, mime) in files {
        if bytes.len() > MAX_SCAN_BYTES {
            return err(413, "That scan is larger than 24 MB");
        }
        let Some(ext) = image_ext(&mime) else {
            return err(415, "Only JPEG, PNG, WebP, GIF, AVIF and TIFF images can be uploaded");
        };
        let name = format!("{}.{}", random_name(), ext);
        // Stored relative to the uploads directory, so moving that directory
        // (or restoring it beside a different install) does not orphan a row.
        let rel = format!("shelf/{}/{}/{}", user.user_id, id, name);
        if let Err(e) = tokio::fs::write(dir.join(&name), &bytes).await {
            return err(500, format!("Could not save the scan: {e}"));
        }
        match state
            .db
            .add_physical_image(
                user.user_id,
                id,
                &kind,
                "upload",
                None,
                None,
                Some(&rel),
                Some(&mime),
                Some(bytes.len() as i64),
                caption.as_deref(),
                position,
                None,
            )
            .await
        {
            Ok(new_id) => {
                stored.push(new_id);
                position += 1;
            }
            Err(e) => {
                // The row is what makes the file reachable, so a file with no
                // row is unreferenced bytes — clean it up rather than leaving it.
                let _ = tokio::fs::remove_file(dir.join(&name)).await;
                return err(500, e.to_string());
            }
        }
    }

    info!("🖼️ Stored {} scan(s) for shelf item {}", stored.len(), id);
    (StatusCode::CREATED, Json(serde_json::json!({ "ids": stored }))).into_response()
}

pub(super) fn random_name() -> String {
    use rand::Rng;
    let raw: [u8; 12] = rand::rng().random();
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

/// GET /api/v1/physical/images/:id/file — serve an uploaded scan.
///
/// Scoped to the caller by `(id, user_id)`, the same rule the artwork proxy
/// keeps: these are photographs of someone's own possessions and ids must not
/// be walkable. Cached hard — a scan never changes once uploaded — but
/// `private`, because it is per-user and a shared cache holding it would serve
/// it to the next person through the proxy.
async fn image_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let Ok(Some(img)) = state.db.get_physical_image(user.user_id, id).await else {
        return err(404, "No such image");
    };
    let Some(rel) = img.file_path else {
        return err(404, "That image lives at a provider, not on this server");
    };
    match tokio::fs::read(state.uploads_dir.join(&rel)).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    img.mime.unwrap_or_else(|| "application/octet-stream".into()),
                ),
                (header::CACHE_CONTROL, "private, max-age=31536000, immutable".into()),
            ],
            Body::from(bytes),
        )
            .into_response(),
        Err(e) => err(404, format!("The file is gone: {e}")),
    }
}

#[derive(Debug, Deserialize)]
pub struct ImageUpdate {
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "explicit")]
    pub caption: Option<Option<String>>,
    pub position: Option<i64>,
}

/// PATCH /api/v1/physical/images/:id — file it, name it, move it.
async fn update_image(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ImageUpdate>,
) -> impl IntoResponse {
    let kind = body.kind.as_deref().map(str::trim).filter(|k| !k.is_empty());
    match state
        .db
        .update_physical_image(
            user.user_id,
            id,
            kind,
            body.caption.as_ref().map(|c| c.as_deref()),
            body.position,
        )
        .await
    {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Ok(false) => err(404, "No such image"),
        Err(e) => err(500, e.to_string()),
    }
}

async fn delete_image(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_physical_image(user.user_id, id).await {
        Ok(Some(file)) => {
            if let Some(f) = file {
                if let Err(e) = tokio::fs::remove_file(state.uploads_dir.join(&f)).await {
                    warn!("Could not remove the scan at {}: {}", f, e);
                }
            }
            (StatusCode::OK, Json(serde_json::json!({ "status": "deleted" }))).into_response()
        }
        Ok(None) => err(404, "No such image"),
        Err(e) => err(500, e.to_string()),
    }
}

// ════════════════════════════════════════════════════════════
//  Playing a side
// ════════════════════════════════════════════════════════════

/// What actually happened to one track on the side.
///
/// The client sends this because it is the only thing that *knows*: it ran the
/// clock, so it saw the pauses and the skips. Deriving it server-side from a
/// start time and a running order can only describe an uninterrupted play.
#[derive(Debug, Deserialize)]
pub struct TrackOutcome {
    pub position: i64,
    /// When this track actually started, unix seconds.
    pub started_at: i64,
    /// Skipped rather than heard. Stored — `scrobbles.skipped` exists for
    /// exactly this — but excluded from every count and never forwarded, which
    /// is the rule the Rockbox import already follows. A skip forwarded to
    /// Last.fm would be a wrong scrobble on a permanent record.
    #[serde(default)]
    pub skipped: bool,
}

#[derive(Debug, Deserialize)]
pub struct PlaySide {
    pub side: String,
    /// Per-track outcomes from the deck. Authoritative when present; without it
    /// the running order is laid out from `started_at`, which is what the
    /// "already played" path sends.
    #[serde(default)]
    pub tracks: Vec<TrackOutcome>,
    /// When the side *started*, unix seconds. Defaults to now minus its running
    /// time, which is what you want when you press the button as it finishes.
    pub started_at: Option<i64>,
    /// The turntable, cartridge or deck. Its playback hours are summed from
    /// these rows.
    pub equipment_id: Option<i64>,
    pub chain_id: Option<i64>,
    /// The first track that actually played, when the side did not start at its
    /// beginning — the needle dropped at track 4, or a tape carried on from
    /// where it was parked.
    ///
    /// This is load-bearing rather than cosmetic. `play_side` writes a listen
    /// for **every** track on the side, so without it starting at track 4
    /// silently records tracks 1–3 as having played. They must not be written
    /// at all — and in particular must not be written as *skips*, because a
    /// skip means "I heard some of it and moved on" and feeds the skip-rate
    /// analytics `scrobbles.skipped` exists for. A track you never reached is
    /// not a track you skipped.
    #[serde(default)]
    pub from_position: Option<i64>,
    /// Where the tape was left when it came off, in seconds into the side.
    /// Cassettes only — `Database::park_tape` enforces that, since it is a fact
    /// about the medium rather than about this request.
    #[serde(default)]
    pub parked_secs: Option<i64>,
    /// Which variant of the chain was fitted — the cartridge in the headshell.
    /// The shelf is the one write path that legitimately knows this, because
    /// you told it when you dropped the needle.
    #[serde(default)]
    pub chain_variant_id: Option<i64>,
}

/// POST /api/v1/physical/:id/play — record a side.
///
/// **One listen per track**, not one per side. Every counting query in this app
/// is over listens, so a side-shaped row would be invisible to all of them.
///
/// The timestamps are the only inferred thing here, and worth being honest
/// about: you observed when the side started, not when track three did. They
/// are spaced by each track's running time from the pressing's tracklist, so
/// the ordering and the gaps are right even though no individual timestamp was
/// measured. A track with no known duration is given three minutes, which is
/// wrong but bounded — the alternative is stacking every track on one instant,
/// which would break session grouping and the hour-of-day heatmap outright.
async fn play_side(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<PlaySide>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };

    let all: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();
    let side = body.side.trim().to_uppercase();
    let mut tracks: Vec<&PhysicalTrack> =
        all.iter().filter(|t| t.side.eq_ignore_ascii_case(&side)).collect();
    tracks.sort_by_key(|t| t.position);

    if tracks.is_empty() {
        return err(400, format!("Side {side} has no tracks on it"));
    }

    // Everything before the needle went down never played. This has to happen
    // **before** `total`, which feeds both the derived start time and the wear
    // recorded below — crediting the stylus with a whole side for the half of
    // it you actually played is the same error `skipped` guards everywhere.
    if let Some(from) = body.from_position {
        tracks.retain(|t| t.position >= from);
        if tracks.is_empty() {
            return err(400, format!("Side {side} has no track at or after {from}"));
        }
    }

    const ASSUMED_SECS: i64 = 180;
    let total: i64 = tracks.iter().map(|t| t.duration_secs.unwrap_or(ASSUMED_SECS)).sum();
    let now = chrono::Utc::now().timestamp();
    let start = body.started_at.unwrap_or(now - total);

    // A chain makes this listen attributable the same way any other is. There
    // is no rung below it here: `equipment` carries no default chain (only
    // `devices` do), and a turntable is not a device that submits anything.
    //
    // What there *is* is a rung above the nothing there used to be — the
    // pressing's own default. A record lives with one turntable, so the deck
    // asks once and the answer is written back below; without that, every
    // shelf listen arrived with `chain_id = NULL` and the chain hours of the
    // gear it plainly ran through never moved.
    let chain_id = body.chain_id.or(release.chain_id);
    let equipment_id = body.equipment_id.or(release.equipment_id);
    // A variant only means anything paired with its own chain, so one naming a
    // different chain is dropped rather than stored — `equipment_seconds` would
    // ignore the pairing anyway, and a listen carrying an id nothing resolves
    // is worse than one carrying none.
    let variant_id = match (chain_id, body.chain_variant_id.or(release.chain_variant_id)) {
        (Some(cid), Some(vid)) => match state.db.variant_fits_chain(user.user_id, vid, cid).await {
            Ok(true) => Some(vid),
            Ok(false) => None,
            Err(e) => return err(500, e.to_string()),
        },
        _ => None,
    };

    // Outcomes the deck reported, by track position.
    let reported: std::collections::HashMap<i64, &TrackOutcome> =
        body.tracks.iter().map(|o| (o.position, o)).collect();

    let mut written = 0usize;
    let mut skipped_count = 0usize;
    let mut at = start;
    for t in &tracks {
        let dur = t.duration_secs.unwrap_or(ASSUMED_SECS);
        // The deck's own timing wins wherever it reported any — it ran the
        // clock, so it is the only thing that saw the pauses.
        let outcome = reported.get(&t.position);
        let ts = outcome.map(|o| o.started_at).unwrap_or(at);
        let was_skipped = outcome.is_some_and(|o| o.skipped);
        let play = Play {
            title: t.title.clone(),
            artist: t.artist.clone().unwrap_or_else(|| release.artist.clone()),
            artists: None,
            album: Some(release.title.clone()),
            timestamp: ts.max(0) as u64,
            duration: Some(dur.max(0) as u64),
            track_number: Some(t.position as i32),
            mbid_recording: None,
            mbid_release: release.mbid_release.clone(),
            mbid_release_group: None,
            mbid_artist: None,
            caa_id: None,
            caa_release_mbid: None,
            // Stable per side play, so pressing the button twice by accident
            // is caught by the same dedup every other source relies on.
            source_id: format!("physical-{}-{}-{}-{}", release.id, side, start, t.position),
            source_name: "physical".into(),
        };

        match state
            .db
            .save_scrobble(
                user.user_id,
                &play,
                None,
                None,
                chain_id,
                // Which cartridge was fitted. The shelf is the one write path that
                // legitimately knows — you told it when you dropped the needle.
                variant_id,
                None,
                Some(medium_label(&release.kind)),
                None,
                // Kept so skip-rate analytics has something real to count, and
                // filed so nothing else does. This used to be an UPDATE after
                // the insert, which left the row briefly `'pending'` — long
                // enough for a flush tick to forward a skip.
                ListenDisposition { imported: false, skipped: was_skipped },
            )
            .await
        {
            Ok(true) => {
                if was_skipped {
                    skipped_count += 1;
                } else {
                    written += 1;
                }
            }
            Ok(false) => {}
            Err(e) => warn!("Could not store a physical listen: {}", e),
        }
        // Only advances the derived clock; a reported track carries its own.
        at = ts + dur;
    }

    // Where the tape ends up. A cassette does not rewind itself, so stopping
    // halfway leaves it there — `park_tape` writes nothing for any other
    // medium, since a record has no such state and inventing one would be
    // modelling physics the object does not have.
    //
    // An explicit `parked_secs` is the deck reporting where it stopped; a side
    // played to the end winds through and is left at the start of the *other*
    // side, which is what an absent value with a `next_side` means.
    if let Some(secs) = body.parked_secs {
        // A tape cannot be parked past the end of its own side. Clamped here
        // rather than trusted, because the position is arithmetic done in the
        // browser and getting it wrong is silent — the first version of this
        // added the needle-drop offset twice and parked a 12:00 side at 12:14,
        // which then reads as "the tape ran out" on a side that had four
        // minutes left. The whole side, not the part that played.
        let side_secs: i64 = all
            .iter()
            .filter(|t| t.side.eq_ignore_ascii_case(&side))
            .map(|t| t.duration_secs.unwrap_or(ASSUMED_SECS))
            .sum();
        let parked = secs.clamp(0, side_secs);
        if let Err(e) = state.db.park_tape(user.user_id, release.id, &side, parked).await {
            warn!("Could not remember where the tape was left: {}", e);
        }
    } else if let Err(e) = state.db.rewind_tape(user.user_id, release.id).await {
        warn!("Could not wind the tape back: {}", e);
    }

    // Only when something was actually recorded. A duplicate submission — the
    // same side at the same start time, which is what a double-tap produces —
    // dedups every listen, and counting stylus hours for it would wear the
    // cartridge down on paper for a play that did not happen twice. Skips
    // count towards wear: the needle was still in the groove to get there.
    if written + skipped_count > 0 {
        if let Err(e) = state
            .db
            .record_physical_play(
                user.user_id,
                release.id,
                &side,
                start,
                total,
                tracks.len() as i64,
                equipment_id,
                chain_id,
            )
            .await
        {
            warn!("Could not record the side play: {}", e);
        }
    }

    // The side is off the deck, whether it ran through or was stopped early.
    // Without this the registry keeps extrapolating the last track it was told
    // about and the dashboard shows a record still playing.
    state.now_playing.clear_user_source(user.user_id, SHELF_SOURCE);
    // And the stored spin goes with it, or reopening the page on any device
    // would offer to resume a side that has already been recorded — which is
    // how the same side gets logged twice.
    if let Err(e) = state.db.clear_spin(user.user_id).await {
        warn!("Could not clear the spin after recording a side: {}", e);
    }

    // Remember what it was played on, so the next side of the same record —
    // and the next time it comes off the shelf — is pre-filled. Only when the
    // caller stated something: a play that fell through to the stored default
    // has nothing new to say.
    if body.chain_id.is_some() || body.equipment_id.is_some() || body.chain_variant_id.is_some() {
        if let Err(e) = state
            .db
            .update_physical(
                user.user_id,
                release.id,
                None,
                body.chain_id.map(Some),
                body.equipment_id.map(Some),
                None,
                None,
                // The cartridge you keep fitted for this record, so the next
                // side does not ask again. `variant_id` rather than the raw
                // body: a variant that failed the chain check is not stored.
                body.chain_variant_id.map(|_| variant_id),
            )
            .await
        {
            warn!("Could not remember what {} is played on: {}", release.title, e);
        }
    }

    info!("🎚️ Side {} of {} — {}: {} listen(s)", side, release.artist, release.title, written);

    // Which side to reach for next — a double LP is A/B/C/D, so this is the
    // next side that exists rather than always "the other one".
    let mut sides: Vec<String> = all.iter().map(|t| t.side.to_uppercase()).collect();
    sides.sort();
    sides.dedup();
    let next = sides.iter().position(|s| *s == side).map(|i| sides[(i + 1) % sides.len()].clone());

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "listens_added": written,
            "skipped": skipped_count,
            "tracks": tracks.len(),
            "duration_secs": total,
            "started_at": start,
            "next_side": next,
        })),
    )
        .into_response()
}

// ════════════════════════════════════════════════════════════
//  What's on the deck
// ════════════════════════════════════════════════════════════

/// The name every shelf entry is filed under in the registry, and therefore the
/// key `clear_user_source` uses. One constant so a stopped side cannot fail to
/// clear the entry a started side wrote.
const SHELF_SOURCE: &str = "Shelf";

#[derive(Debug, Deserialize)]
pub struct DeckState {
    pub side: String,
    /// The track's position on the side — the deck knows which one it is
    /// inside, because it is the thing running the clock.
    pub position: i64,
    /// Seconds into that track.
    #[serde(default)]
    pub elapsed: u64,
    /// A paused record is not playing, but it is still on the deck. The entry
    /// stays and stops advancing, which is what the still platter shows.
    #[serde(default)]
    pub paused: bool,
}

/// POST /api/v1/physical/:id/now-playing — a side is on.
///
/// The shelf is the one source where the **browser** is the player: the clock
/// runs in front of the turntable, in localStorage, and the server cannot
/// derive a playhead from anything it holds. So the deck pushes, rather than
/// the engine polling — which is why this endpoint exists at all when Plex,
/// Jellyfin and Navidrome need nothing of the kind.
///
/// Called on every track change, on pause and resume, and on a heartbeat while
/// a side runs. The heartbeat is not optional: `NowPlaying::is_live` expires an
/// entry by *running past the end of the track*, so a paused record would drop
/// off the dashboard one track length after it was paused.
async fn set_now_playing(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<DeckState>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };
    let all: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();
    let side = body.side.trim().to_uppercase();
    let Some(track) =
        all.iter().find(|t| t.side.eq_ignore_ascii_case(&side) && t.position == body.position)
    else {
        return err(400, "No such track on that side");
    };

    let play = Play {
        title: track.title.clone(),
        artist: track.artist.clone().unwrap_or_else(|| release.artist.clone()),
        artists: None,
        album: Some(release.title.clone()),
        timestamp: 0,
        duration: track.duration_secs.map(|d| d.max(0) as u64),
        track_number: Some(track.position as i32),
        mbid_recording: None,
        mbid_release: release.mbid_release.clone(),
        mbid_release_group: None,
        mbid_artist: None,
        caa_id: None,
        caa_release_mbid: None,
        source_id: format!("physical-{}-{}-np", release.id, side),
        source_name: "physical".into(),
    };

    // The browser sitting in front of the turntable ran the clock, so
    // unlike every other push source this really is a measured position.
    let mut np = NowPlaying::from_play(&play, SHELF_SOURCE, None, Progress::Exact(body.elapsed));
    // `DeckState::paused` says the entry "stays and stops advancing", and until
    // the registry learned about pause only the first half was true: the
    // position counted forward regardless and the 10s heartbeat was the only
    // thing dragging it back to the truth. It really stops now.
    np.paused = body.paused;
    // The medium, which is the only thing resembling a format a turntable can
    // report — there is no codec here and never will be. Note the listen itself
    // files the same string under `submission_client` rather than
    // `listening_context`: "off a CD" describes where it came from, and that is
    // the column `play_side` has always used.
    np.listening_context = Some(medium_label(&release.kind).to_string());
    let changed = state.now_playing.set(user.user_id, np);

    // Only on a real change of track, for the reason the registry returns the
    // flag at all: Last.fm's and ListenBrainz's now-playing is a one-shot that
    // expires on its own, so re-sending it on every heartbeat is noise aimed at
    // someone else's servers. A paused side re-reports the same track, so this
    // is false for the whole time it stands still.
    if changed && !body.paused {
        let sinks =
            crate::sinks::sinks_for_user(&state.db, user.user_id, &state.listenbrainz_url).await;
        crate::sinks::submit_now_playing(&sinks, &play).await;
    }

    (StatusCode::OK, Json(serde_json::json!({ "status": "ok", "changed": changed })))
        .into_response()
}

/// DELETE /api/v1/physical/now-playing — nothing is on the deck.
///
/// Its own route rather than a field on the one above because it is also what
/// *cancelling* a side means, and that has no track to name.
async fn clear_now_playing(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    state.now_playing.clear_user_source(user.user_id, SHELF_SOURCE);
    (StatusCode::OK, Json(serde_json::json!({ "status": "cleared" }))).into_response()
}

// ═══════════════════════════════════════════════════════════
//  What is on the deck, as a fact about the shelf
// ═══════════════════════════════════════════════════════════
//
//  The clock still runs in the browser — nothing else can watch a turntable —
//  but *what is on* and *when it started* live here. That is the difference
//  between a side that survives a crash and one that does not, and it is what
//  lets the phone in your pocket see the record that is actually playing.

#[derive(Debug, Deserialize)]
pub struct SpinUpdate {
    pub release_id: i64,
    pub side: String,
    pub started_at: i64,
    #[serde(default)]
    pub paused_at: Option<i64>,
    #[serde(default)]
    pub paused_total: i64,
    #[serde(default)]
    pub outcomes: Vec<serde_json::Value>,
    #[serde(default)]
    pub chain_id: Option<i64>,
    #[serde(default)]
    pub equipment_id: Option<i64>,
    /// How far into the side this spin began — the needle dropped mid-side, or
    /// a tape resumed from where it was parked.
    #[serde(default)]
    pub start_secs: i64,
}

/// GET /api/v1/physical/spin — the side on the deck, with enough of the record
/// to draw it without a second request.
async fn get_spin(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let spin = match state.db.get_spin(user.user_id).await {
        Ok(s) => s,
        Err(e) => return err(500, e.to_string()),
    };
    let Some(spin) = spin else {
        return (StatusCode::OK, Json(serde_json::json!({ "spin": null }))).into_response();
    };

    // A spin whose record has since been deleted is not a spin. Clearing it
    // here rather than returning a dangling id keeps the page from having to
    // reason about a release it cannot fetch.
    let release = match state.db.get_physical(user.user_id, spin.release_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            let _ = state.db.clear_spin(user.user_id).await;
            return (StatusCode::OK, Json(serde_json::json!({ "spin": null }))).into_response();
        }
        Err(e) => return err(500, e.to_string()),
    };

    let outcomes: serde_json::Value =
        serde_json::from_str(&spin.outcomes).unwrap_or_else(|_| serde_json::json!([]));
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "spin": {
                "release_id": spin.release_id,
                "kind": release.kind,
                "artist": release.artist,
                "title": release.title,
                "artwork_url": release.artwork_url,
                "mbid_release": release.mbid_release,
                "side": spin.side,
                "started_at": spin.started_at,
                "paused_at": spin.paused_at,
                "paused_total": spin.paused_total,
                "outcomes": outcomes,
                "chain_id": spin.chain_id,
                "equipment_id": spin.equipment_id,
                "start_secs": spin.start_secs,
                "updated_at": spin.updated_at,
            }
        })),
    )
        .into_response()
}

/// PUT /api/v1/physical/spin — put a side on, or update the one that is on.
///
/// The whole state every time rather than a patch per field: it is half a dozen
/// small numbers, it changes only on real events (start, pause, skip, next),
/// and a partial update would let two devices interleave into a state neither
/// of them meant.
async fn put_spin(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SpinUpdate>,
) -> impl IntoResponse {
    // The release must be the caller's — this is the only id in the body that
    // names something owned.
    match state.db.get_physical(user.user_id, body.release_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
    }

    let outcomes = serde_json::to_string(&body.outcomes).unwrap_or_else(|_| "[]".into());
    match state
        .db
        .set_spin(
            user.user_id,
            body.release_id,
            &body.side,
            body.started_at,
            body.paused_at,
            body.paused_total,
            &outcomes,
            body.chain_id,
            body.equipment_id,
            body.start_secs.max(0),
        )
        .await
    {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

/// DELETE /api/v1/physical/spin — take it off the deck.
///
/// Also clears the now-playing entry, so the two cannot disagree: a deck
/// showing a record that came off twenty minutes ago is the failure the
/// now-playing design already calls out.
async fn delete_spin(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if let Err(e) = state.db.clear_spin(user.user_id).await {
        return err(500, e.to_string());
    }
    state.now_playing.clear_user_source(user.user_id, SHELF_SOURCE);
    (StatusCode::OK, Json(serde_json::json!({ "status": "cleared" }))).into_response()
}

/// How a medium is written on a listen. One place, because it is written by the
/// side play, the now-playing push and the export.
fn medium_label(kind: &str) -> &'static str {
    match kind {
        "vinyl" => "Vinyl",
        "cd" => "CD",
        "sacd" => "SACD",
        _ => "Cassette",
    }
}

/// GET /api/v1/physical/equipment/:id/wear — hours on a stylus or head.
async fn equipment_wear(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.equipment_playback_hours(user.user_id, id).await {
        Ok((hours, sides)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "hours": (hours * 10.0).round() / 10.0,
                "sides_played": sides,
            })),
        )
            .into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// ════════════════════════════════════════════════════════════
//  Discogs configuration (admin)
// ════════════════════════════════════════════════════════════

async fn get_discogs(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let token_env = env_nonempty("TAPEDECK_DISCOGS_TOKEN").is_some();
    let key_env = env_nonempty("TAPEDECK_DISCOGS_KEY").or_else(|| env_nonempty("Consumer_Key"));
    let secret_env =
        env_nonempty("TAPEDECK_DISCOGS_SECRET").or_else(|| env_nonempty("Consumer_Secret"));
    let pair_env = key_env.is_some() && secret_env.is_some();

    let stored_token = state
        .db
        .get_app_setting(DISCOGS_TOKEN_KEY)
        .await
        .unwrap_or(None)
        .is_some_and(|t| !t.trim().is_empty());
    let stored_pair = matches!(
        (
            state.db.get_app_setting(DISCOGS_KEY_KEY).await.unwrap_or(None),
            state.db.get_app_setting(DISCOGS_SECRET_KEY).await.unwrap_or(None)
        ),
        (Some(k), Some(s)) if !k.trim().is_empty() && !s.trim().is_empty()
    );

    let from_env = token_env || pair_env;
    let stored = stored_token || stored_pair;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            // Never the credential itself, the same rule source credentials follow.
            "configured": stored || from_env,
            "from_env": from_env,
            "stored": stored,
            // Which shape is in use, so the UI can say so rather than leaving
            // someone guessing which of the two fields took effect.
            "kind": if stored_token || (!stored && token_env) { "token" }
                    else if stored_pair || (!stored && pair_env) { "key_secret" }
                    else { "none" },
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct DiscogsUpdate {
    /// A personal access token. Empty clears it.
    #[serde(default)]
    pub token: Option<String>,
    /// An application's consumer key and secret. Either empty clears the pair.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
}

async fn set_discogs(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<DiscogsUpdate>,
) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    // Each field is written only when present, so saving a key/secret pair does
    // not silently wipe a token that was already working — and clearing one is
    // an explicit empty string rather than an omission.
    for (key, value) in [
        (DISCOGS_TOKEN_KEY, &body.token),
        (DISCOGS_KEY_KEY, &body.key),
        (DISCOGS_SECRET_KEY, &body.secret),
    ] {
        if let Some(v) = value {
            if let Err(e) = state.db.set_app_setting(key, v.trim()).await {
                return err(500, e.to_string());
            }
        }
    }
    let configured = discogs_auth(&state).await.is_some();
    (StatusCode::OK, Json(serde_json::json!({ "configured": configured }))).into_response()
}

/// POST /api/v1/physical/settings/discogs/test — does the credential work?
///
/// Separate from saving it because Discogs answers a bad key/secret with a
/// normal 200, serving the request as anonymous. Without this, a typo looks
/// exactly like success until someone notices barcode lookups are quietly
/// worse.
async fn test_discogs(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let Some(auth) = discogs_auth(&state).await else {
        return err(400, "Discogs isn't configured on this server");
    };
    match release_lookup::discogs_verify(&auth).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "message": "Working — Discogs accepted the credential.",
            })),
        )
            .into_response(),
        // A rejection is a 200 with `ok: false`, not an HTTP error: the request
        // to Tapedeck succeeded and the answer is "no". The message is
        // Discogs' own, which names which credential shape it rejected.
        Err(e) => {
            (StatusCode::OK, Json(serde_json::json!({ "ok": false, "message": e.to_string() })))
                .into_response()
        }
    }
}

// ════════════════════════════════════════════════════════════
//  Taking the shelf with you
// ════════════════════════════════════════════════════════════

/// What the zip says about itself.
const EXPORT_README: &str = "\
# Your shelf

Everything Tapedeck holds about the objects you own — the pressings, every side
you have put on, and the gear they ran through.

* `shelf.json` — one entry per pressing: what it is, when you bought it, its
  tracklist as sides, what it is normally played on, and its scans.
* `plays.csv` — every side ever played, oldest first, with the gear named.
* `gear.json` — your equipment and signal chains, with purchase dates.
* `scans/` — the images **you uploaded**, one directory per pressing.

Scans that live at the Cover Art Archive or Discogs are listed in `shelf.json`
with their URLs rather than copied in here. They are not yours to redistribute
and they are still there to fetch; your own scans exist nowhere else, which is
why those are the ones in the file.

That is also why this export exists separately from the database backup:
`GET /api/v1/backup` is a snapshot of the database, and uploaded scans are files
beside it rather than rows inside it. A backup and this export together are the
whole shelf.
";

/// POST /api/v1/physical/import — put a shelf back.
///
/// The counterpart of the export above, and deliberately **not** a database
/// restore. It brings back the objects: pressings, their tracklists, the gear,
/// the sides you played, and your own scans.
///
/// What it cannot bring back is the *listening* — a shelf export holds no
/// `scrobbles`, so the listens a side generated are not in the file and the
/// chain hours derived from them stay where they are. That is stated in the
/// response rather than papered over, because a shelf that came back with its
/// play counts but no listens would make two screens disagree.
///
/// **Everything is matched before it is inserted**, so importing the same zip
/// twice changes nothing the second time. That matters more here than for a
/// listens import: a duplicated pressing is a visible mess on a page you look
/// at, and there is no dedup elsewhere that would catch it.
async fn import_shelf(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut body: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if let Ok(b) = field.bytes().await {
            if !b.is_empty() {
                body = Some(b.to_vec());
                break;
            }
        }
    }
    let Some(body) = body else { return err(400, "No file in the request") };
    if body.len() > MAX_IMPORT_BYTES {
        return err(413, "That archive is larger than 256 MB");
    }

    let reader = std::io::Cursor::new(body);
    let mut zip = match zip::ZipArchive::new(reader) {
        Ok(z) => z,
        Err(e) => return err(400, format!("That is not a zip archive: {e}")),
    };

    // Read the manifest first — nothing is written until it parses, so a
    // truncated or unrelated zip fails before it has half-populated a shelf.
    let shelf: Vec<serde_json::Value> = {
        let Ok(mut f) = zip.by_name("shelf.json") else {
            return err(400, "No shelf.json in that archive — is it a Tapedeck shelf export?");
        };
        let mut text = String::new();
        use std::io::Read;
        if f.read_to_string(&mut text).is_err() {
            return err(400, "shelf.json could not be read");
        }
        match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => return err(400, format!("shelf.json is not valid JSON: {e}")),
        }
    };

    let mut report = ImportReport::default();

    // ── Gear first: a pressing names its chain and deck by *name*, so they
    // have to exist before the releases that point at them.
    if let Ok(mut f) = zip.by_name("gear.json") {
        use std::io::Read;
        let mut text = String::new();
        if f.read_to_string(&mut text).is_ok() {
            if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) {
                for g in doc.get("equipment").and_then(|v| v.as_array()).into_iter().flatten() {
                    let Some(name) = g.get("name").and_then(|v| v.as_str()) else { continue };
                    let kind = g.get("type").and_then(|v| v.as_str()).unwrap_or("other");
                    // `upsert_equipment` matches on the name, so re-importing
                    // updates rather than duplicating. `hours` is deliberately
                    // not restored: it is derived from the listens, and writing
                    // it here would be a second answer that drifts.
                    match state
                        .db
                        .upsert_equipment(
                            user.user_id,
                            name,
                            kind,
                            g.get("brand").and_then(|v| v.as_str()),
                            g.get("model").and_then(|v| v.as_str()),
                            0.0,
                            g.get("purchased_at").and_then(|v| v.as_i64()),
                        )
                        .await
                    {
                        Ok(_) => report.gear += 1,
                        Err(e) => warn!("shelf import: gear {name}: {e}"),
                    }
                }
            }
        }
    }

    let gear = state.db.get_equipment(user.user_id).await.unwrap_or_default();
    let chains = state.db.get_chains(user.user_id).await.unwrap_or_default();
    // Chains are **matched, never created** — the same rule `[[gear:…]]`
    // follows. A chain is something you build step by step; conjuring an empty
    // one from a name in a file would produce a chain with no components that
    // silently attributes listens to nothing.
    let chain_by_name = |n: Option<&str>| {
        n.and_then(|n| chains.iter().find(|c| c.name.eq_ignore_ascii_case(n))).and_then(|c| c.id)
    };
    let gear_by_name = |n: Option<&str>| {
        n.and_then(|n| gear.iter().find(|g| g.name.eq_ignore_ascii_case(n))).map(|g| g.id)
    };

    let existing = state.db.list_physical(user.user_id, None).await.unwrap_or_default();

    for entry in &shelf {
        let (Some(artist), Some(title), Some(kind)) = (
            entry.get("artist").and_then(|v| v.as_str()),
            entry.get("title").and_then(|v| v.as_str()),
            entry.get("kind").and_then(|v| v.as_str()),
        ) else {
            report.skipped += 1;
            continue;
        };
        if !SHELF_KINDS.contains(&kind) {
            report.skipped += 1;
            continue;
        }

        // Already on the shelf? A barcode or an MBID identifies a *pressing*
        // exactly; without either, the artist, title and medium together are
        // the best available key — two copies of the same record on the same
        // shelf is a thing that happens, but importing the same file twice is
        // far more likely than owning a duplicate.
        let barcode = entry.get("barcode").and_then(|v| v.as_str());
        let mbid = entry.get("mbid_release").and_then(|v| v.as_str());
        let already = existing.iter().find(|r| {
            (barcode.is_some() && r.barcode.as_deref() == barcode)
                || (mbid.is_some() && r.mbid_release.as_deref() == mbid)
                || (r.kind == kind
                    && r.artist.eq_ignore_ascii_case(artist)
                    && r.title.eq_ignore_ascii_case(title))
        });
        if already.is_some() {
            report.already += 1;
            continue;
        }

        let tracks = entry
            .get("tracks")
            .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "[]".into()))
            .unwrap_or_else(|| "[]".into());

        let id = match state
            .db
            .create_physical(
                user.user_id,
                kind,
                artist,
                title,
                entry.get("year").and_then(|v| v.as_i64()),
                mbid,
                entry.get("discogs_id").and_then(|v| v.as_str()),
                barcode,
                entry.get("catalog_number").and_then(|v| v.as_str()),
                None,
                &tracks,
                entry.get("notes").and_then(|v| v.as_str()),
                entry.get("purchased_at").and_then(|v| v.as_i64()),
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                warn!("shelf import: {artist} — {title}: {e}");
                report.skipped += 1;
                continue;
            }
        };
        report.releases += 1;

        // What it is normally played on, resolved from the names the export
        // wrote. Missing either is normal — the chain may not exist here.
        let chain = chain_by_name(entry.get("played_on_chain").and_then(|v| v.as_str()));
        let deck = gear_by_name(entry.get("played_on_equipment").and_then(|v| v.as_str()));
        if chain.is_some() || deck.is_some() {
            let _ = state
                .db
                .update_physical(
                    user.user_id,
                    id,
                    None,
                    chain.map(Some),
                    deck.map(Some),
                    None,
                    None,
                    None,
                )
                .await;
        }

        // Images. A provider image is a URL the archive still serves, so it
        // comes back as a row pointing at it; an upload is a file in the zip
        // and has to be written back to disk before the row means anything.
        for img in entry.get("images").and_then(|v| v.as_array()).into_iter().flatten() {
            let ikind = img.get("kind").and_then(|v| v.as_str()).unwrap_or("other");
            let provider = img.get("provider").and_then(|v| v.as_str()).unwrap_or("upload");
            let caption = img.get("caption").and_then(|v| v.as_str());
            let position = img.get("position").and_then(|v| v.as_i64()).unwrap_or(0);

            if let Some(url) = img.get("url").and_then(|v| v.as_str()) {
                let _ = state
                    .db
                    .add_physical_image(
                        user.user_id,
                        id,
                        ikind,
                        provider,
                        Some(url),
                        None,
                        None,
                        None,
                        None,
                        caption,
                        position,
                        None,
                    )
                    .await;
                report.images += 1;
                continue;
            }

            let Some(name) = img.get("file").and_then(|v| v.as_str()) else { continue };
            let entry_name = format!("scans/{name}");
            let bytes = {
                use std::io::Read;
                match zip.by_name(&entry_name) {
                    Ok(mut f) => {
                        let mut b = Vec::new();
                        if f.read_to_end(&mut b).is_ok() {
                            Some(b)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            };
            let Some(bytes) = bytes else {
                report.missing_scans += 1;
                continue;
            };
            if bytes.len() > MAX_SCAN_BYTES {
                report.missing_scans += 1;
                continue;
            }
            // The extension comes from the stored name, but the *path* is built
            // here — a filename out of an archive never reaches the filesystem,
            // the same rule the upload handler follows for a client's filename.
            let ext = std::path::Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or("jpg")
                .to_ascii_lowercase();
            let dir =
                state.uploads_dir.join("shelf").join(user.user_id.to_string()).join(id.to_string());
            if tokio::fs::create_dir_all(&dir).await.is_err() {
                report.missing_scans += 1;
                continue;
            }
            let fname = format!("{}.{}", random_name(), ext);
            let rel = format!("shelf/{}/{}/{}", user.user_id, id, fname);
            if tokio::fs::write(dir.join(&fname), &bytes).await.is_err() {
                report.missing_scans += 1;
                continue;
            }
            let mime = mime_guess::from_path(&fname).first_raw().unwrap_or("image/jpeg");
            let _ = state
                .db
                .add_physical_image(
                    user.user_id,
                    id,
                    ikind,
                    "upload",
                    None,
                    None,
                    Some(&rel),
                    Some(mime),
                    Some(bytes.len() as i64),
                    caption,
                    position,
                    None,
                )
                .await;
            report.images += 1;
        }

        // The sides played. `plays.csv` is keyed by artist and title rather
        // than by id, so it is matched back here while this pressing is the one
        // in hand — which also means a play whose pressing was skipped as a
        // duplicate is not re-added against the wrong row.
        report.plays += restore_plays(&state, user.user_id, id, artist, title, &mut zip).await;
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "releases": report.releases,
            "already_on_the_shelf": report.already,
            "skipped": report.skipped,
            "gear": report.gear,
            "images": report.images,
            "missing_scans": report.missing_scans,
            "plays": report.plays,
            // Said plainly rather than left to be discovered: the listens a
            // side generated live in `scrobbles`, which a shelf export does not
            // carry. Chain hours are derived from those, so they do not return
            // with the objects.
            "note": concat!(
                "Pressings, gear, scans and sides are back. The listens those sides ",
                "generated are not in a shelf export — restore a database backup for ",
                "the history and the chain hours derived from it."
            ),
        })),
    )
        .into_response()
}

/// The biggest shelf archive we will read. A hundred records with booklets is
/// large; a quarter of a gigabyte is somebody's photo library.
const MAX_IMPORT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
struct ImportReport {
    releases: usize,
    already: usize,
    skipped: usize,
    gear: usize,
    images: usize,
    missing_scans: usize,
    plays: usize,
}

/// Put back the sides played for one pressing, from `plays.csv`.
///
/// Returns how many were written. Anything unparseable is skipped rather than
/// failing the import — a shelf coming back matters more than one malformed
/// row, and the count in the response says how many landed.
async fn restore_plays<R: std::io::Read + std::io::Seek>(
    state: &Arc<AppState>,
    user_id: i64,
    release_id: i64,
    artist: &str,
    title: &str,
    zip: &mut zip::ZipArchive<R>,
) -> usize {
    use std::io::Read;
    let mut text = String::new();
    let Ok(mut f) = zip.by_name("plays.csv") else { return 0 };
    if f.read_to_string(&mut text).is_err() {
        return 0;
    }

    let existing = state.db.plays_for_release(user_id, release_id).await.unwrap_or_default();
    let mut written = 0;
    for line in text.lines().skip(1) {
        let cols = parse_csv_row(line);
        if cols.len() < 7 {
            continue;
        }
        let (Ok(played_at), Some(row_artist), Some(row_title)) =
            (cols[0].parse::<i64>(), cols.get(1), cols.get(2))
        else {
            continue;
        };
        if !row_artist.eq_ignore_ascii_case(artist) || !row_title.eq_ignore_ascii_case(title) {
            continue;
        }
        let side = cols[4].clone();
        // Re-importing the same file must not double the wear on a stylus.
        if existing
            .iter()
            .any(|(s, at): &(String, i64)| *at == played_at && s.eq_ignore_ascii_case(&side))
        {
            continue;
        }
        let tracks = cols[5].parse::<i64>().unwrap_or(0);
        let secs = cols[6].parse::<i64>().unwrap_or(0);
        let deck = cols.get(7).filter(|v| !v.is_empty());
        let chain = cols.get(8).filter(|v| !v.is_empty());
        let gear_id = match deck {
            Some(n) => {
                state.db.find_equipment_by_name(user_id, n).await.unwrap_or(None).map(|(id, _)| id)
            }
            None => None,
        };
        let chain_id = match chain {
            Some(n) => state.db.get_chain_by_name(user_id, n).await.unwrap_or(None),
            None => None,
        };
        if state
            .db
            .record_physical_play(
                user_id, release_id, &side, played_at, secs, tracks, gear_id, chain_id,
            )
            .await
            .is_ok()
        {
            written += 1;
        }
    }
    written
}

/// Split one CSV row, honouring the quoting the export writes.
fn parse_csv_row(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// GET /api/v1/physical/export — the shelf as a zip.
///
/// Built in memory rather than streamed, unlike the listen export: a zip's
/// central directory is written after the entries anyway, and a shelf is
/// bounded by how many records fit in a room.
async fn export_shelf(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    use std::io::Write;

    let uid = user.user_id;
    let releases = match state.db.list_physical(uid, None).await {
        Ok(r) => r,
        Err(e) => return err(500, e.to_string()),
    };
    let images = state.db.all_physical_images(uid).await.unwrap_or_default();
    let plays = state.db.all_physical_plays(uid).await.unwrap_or_default();
    let gear = state.db.get_equipment(uid).await.unwrap_or_default();
    let chains = state.db.get_chains(uid).await.unwrap_or_default();

    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // Images are already compressed; deflating a JPEG spends CPU to make it
        // very slightly bigger.
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        let mut fail = false;
        let mut write = |zip: &mut zip::ZipWriter<_>, name: String, body: &[u8], o| {
            if zip.start_file(name, o).is_err() || zip.write_all(body).is_err() {
                fail = true;
            }
        };

        write(&mut zip, "README.md".into(), EXPORT_README.as_bytes(), opts);

        // One entry per pressing, with its tracklist parsed back out of the
        // stored blob — an export that made the reader parse a JSON string
        // inside a JSON document would be passing the work on rather than
        // doing it.
        let shelf: Vec<serde_json::Value> = releases
            .iter()
            .map(|r| {
                let tracks: serde_json::Value =
                    serde_json::from_str(&r.tracks).unwrap_or_else(|_| serde_json::json!([]));
                let mine: Vec<serde_json::Value> = images
                    .iter()
                    .filter(|i| i.release_id == r.id)
                    .map(|i| {
                        serde_json::json!({
                            "kind": i.kind,
                            "provider": i.provider,
                            "caption": i.caption,
                            "position": i.position,
                            // Exactly one of these is meaningful, and which one
                            // is what `provider` says.
                            "url": i.url,
                            "file": i.file_path.as_ref().map(|_| scan_name(r, i)),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "kind": r.kind,
                    "artist": r.artist,
                    "title": r.title,
                    "year": r.year,
                    "mbid_release": r.mbid_release,
                    "discogs_id": r.discogs_id,
                    "barcode": r.barcode,
                    "catalog_number": r.catalog_number,
                    "purchased_at": r.purchased_at,
                    "added_at": r.added_at,
                    "notes": r.notes,
                    "played_sides": r.play_count,
                    "played_secs": r.played_secs,
                    "last_played": r.last_played,
                    "played_on_chain": chains.iter().find(|c| c.id == r.chain_id).map(|c| c.name.clone()),
                    "played_on_equipment": gear.iter().find(|g| Some(g.id) == r.equipment_id).map(|g| g.name.clone()),
                    "tracks": tracks,
                    "images": mine,
                })
            })
            .collect();
        write(
            &mut zip,
            "shelf.json".into(),
            serde_json::to_string_pretty(&shelf).unwrap_or_default().as_bytes(),
            opts,
        );

        let mut csv_body = String::from(
            "played_at,artist,title,format,side,tracks,duration_secs,equipment,chain\n",
        );
        for p in &plays {
            csv_body.push_str(&format!(
                "{},{},{},{},{},{},{},{},{}\n",
                p.played_at,
                csv(&p.artist),
                csv(&p.title),
                medium_label(&p.kind),
                csv(&p.side),
                p.track_count,
                p.duration_secs,
                csv(p.equipment_name.as_deref().unwrap_or("")),
                csv(p.chain_name.as_deref().unwrap_or("")),
            ));
        }
        write(&mut zip, "plays.csv".into(), csv_body.as_bytes(), opts);

        let gear_doc = serde_json::json!({
            "equipment": gear.iter().map(|g| serde_json::json!({
                "name": g.name,
                "type": g.equipment_type,
                "brand": g.brand,
                "model": g.model,
                "purchased_at": g.purchased_at,
                "first_used": g.first_used,
                "last_used": g.last_used,
                // Derived from the listens, not an accumulator — so this is
                // the same number the app shows rather than a second answer.
                "hours": (g.total_hours * 10.0).round() / 10.0,
                "notes": g.notes,
            })).collect::<Vec<_>>(),
            "chains": chains.iter().map(|c| serde_json::json!({
                "name": c.name,
                "description": c.description,
                "icon": c.icon,
                "listening_context": c.listening_context,
                "components": c.components,
                "hours": (c.total_hours * 10.0).round() / 10.0,
            })).collect::<Vec<_>>(),
        });
        write(
            &mut zip,
            "gear.json".into(),
            serde_json::to_string_pretty(&gear_doc).unwrap_or_default().as_bytes(),
            opts,
        );

        for r in &releases {
            for img in images.iter().filter(|i| i.release_id == r.id) {
                let Some(rel) = &img.file_path else { continue };
                match tokio::fs::read(state.uploads_dir.join(rel)).await {
                    Ok(bytes) => {
                        write(&mut zip, format!("scans/{}", scan_name(r, img)), &bytes, stored)
                    }
                    // A missing file is not worth failing a whole export over;
                    // it is already listed in `shelf.json`, so the gap is
                    // visible rather than silent.
                    Err(e) => warn!("Scan {} could not be read for export: {}", rel, e),
                }
            }
        }

        if fail || zip.finish().is_err() {
            return err(500, "Could not build the export");
        }
    }

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"tapedeck-shelf.zip\""),
        ],
        buf.into_inner(),
    )
        .into_response()
}

/// Where one scan lands in the zip: `<artist> — <title>/<kind>-<n>.<ext>`.
///
/// Named after the record rather than after the row id, because the point of an
/// export is that it is readable somewhere that has never heard of this
/// database. The id is still in the stem, so two pressings of the same album
/// cannot collide.
fn scan_name(release: &crate::db::PhysicalRelease, img: &PhysicalImage) -> String {
    let ext = img
        .file_path
        .as_ref()
        .and_then(|p| p.rsplit('.').next())
        .filter(|e| e.len() <= 5)
        .unwrap_or("jpg");
    format!(
        "{} - {} [{}]/{}-{:02}.{}",
        safe(&release.artist),
        safe(&release.title),
        release.id,
        safe(&img.kind),
        img.position + 1,
        ext
    )
}

/// A path segment a filesystem will accept. Replaces rather than drops, so
/// `AC/DC` reads as `AC DC` rather than `ACDC` — the same rule the liner-note
/// export's filenames follow.
fn safe(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim().to_string();
    if trimmed.is_empty() {
        "Untitled".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// Minimal RFC-4180 field escaping, matching the listen export's.
fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn create_physical_router() -> Router<Arc<AppState>> {
    Router::new()
        // Registered before `/:id` so they aren't swallowed by the
        // parameterised route — axum 0.7 / matchit 0.7 matches in order.
        .route("/api/v1/physical/lookup", get(lookup))
        .route("/api/v1/physical/export", get(export_shelf))
        .route("/api/v1/physical/import", post(import_shelf))
        .route("/api/v1/physical/discogs/:id", get(discogs_detail))
        .route("/api/v1/physical/equipment/:id/wear", get(equipment_wear))
        .route("/api/v1/physical/settings/discogs", get(get_discogs).put(set_discogs))
        .route("/api/v1/physical/settings/discogs/test", post(test_discogs))
        .route("/api/v1/physical/now-playing", axum::routing::delete(clear_now_playing))
        // Before "/:id", or matchit reads "spin" as a release id.
        .route("/api/v1/physical/spin", get(get_spin).put(put_spin).delete(delete_spin))
        .route(
            "/api/v1/physical/images/:id",
            axum::routing::patch(update_image).delete(delete_image),
        )
        .route("/api/v1/physical/images/:id/file", get(image_file))
        .route("/api/v1/physical", get(list_shelf).post(add_release))
        .route("/api/v1/physical/:id", get(get_one).patch(update_release).delete(delete_release))
        .route("/api/v1/physical/:id/tracks", axum::routing::put(set_tracks))
        .route("/api/v1/physical/:id/play", post(play_side))
        .route("/api/v1/physical/:id/now-playing", post(set_now_playing))
        .route("/api/v1/physical/:id/sides", get(side_plays))
        .route("/api/v1/physical/:id/images", get(list_images).post(upload_image))
        .route("/api/v1/physical/:id/images/fetch", post(fetch_images))
}
