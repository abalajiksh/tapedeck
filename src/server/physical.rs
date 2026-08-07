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
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
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

async fn list_shelf(
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

    match state
        .db
        .create_physical(
            user.user_id,
            &body.kind,
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
        )
        .await
    {
        Ok(id) => {
            info!("💿 Added {} to the shelf: {} — {}", body.kind, body.artist, body.title);
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
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
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "status": "deleted" }))).into_response()
        }
        Ok(false) => err(404, "No such record"),
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

    const ASSUMED_SECS: i64 = 180;
    let total: i64 = tracks.iter().map(|t| t.duration_secs.unwrap_or(ASSUMED_SECS)).sum();
    let now = chrono::Utc::now().timestamp();
    let start = body.started_at.unwrap_or(now - total);

    // A chain makes this listen attributable the same way any other is. There
    // is no rung below it here: `equipment` carries no default chain (only
    // `devices` do), and a turntable is not a device that submits anything.
    let chain_id = body.chain_id;

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
                None,
                Some(match release.kind.as_str() {
                    "vinyl" => "Vinyl",
                    "cd" => "CD",
                    "sacd" => "SACD",
                    _ => "Cassette",
                }),
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
                body.equipment_id,
                chain_id,
            )
            .await
        {
            warn!("Could not record the side play: {}", e);
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

pub fn create_physical_router() -> Router<Arc<AppState>> {
    Router::new()
        // Registered before `/:id` so they aren't swallowed by the
        // parameterised route — axum 0.7 / matchit 0.7 matches in order.
        .route("/api/v1/physical/lookup", get(lookup))
        .route("/api/v1/physical/discogs/:id", get(discogs_detail))
        .route("/api/v1/physical/equipment/:id/wear", get(equipment_wear))
        .route("/api/v1/physical/settings/discogs", get(get_discogs).put(set_discogs))
        .route("/api/v1/physical/settings/discogs/test", post(test_discogs))
        .route("/api/v1/physical", get(list_shelf).post(add_release))
        .route("/api/v1/physical/:id", get(get_one).delete(delete_release))
        .route("/api/v1/physical/:id/tracks", axum::routing::put(set_tracks))
        .route("/api/v1/physical/:id/play", post(play_side))
}
