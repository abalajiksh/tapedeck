//! The Radio page: the station list, presets, the tuner, today's schedule and
//! the air log with its review queue.
//!
//! Reads take a session or a token with `read`; anything that changes a
//! preset, the settings or the tuner takes `write`. Tuning in opens a stream
//! from this server for as long as it runs, which is why it is a write and
//! not a read.
//!
//! The model is `crate::radio`; read its module docs before changing what
//! becomes a listen.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::error;

use super::auth::{ReadAuth, WriteAuth};
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{AirLogEntry, RadioPreset, RadioSession};
use crate::openapi::{BadRequest, NotFound, Unauthorized};
use crate::radio::{self, RadioSettings, ScheduleOut, TunerStatus};
use crate::radiodns::directory::DirectoryStation;
use crate::radiodns::{self as rdns, Bearer, StationLookup};
use crate::sources::egress::Egress;

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

/// Today in the listener's own zone — a broadcaster's day is local, and so is
/// "what is on tonight".
async fn today(state: &AppState, user_id: i64) -> chrono::NaiveDate {
    let tz = crate::server::scrobbles::user_tz_for(state, user_id).await;
    chrono::Utc::now().with_timezone(&tz).date_naive()
}

/// A dial position as typed, normalised for its band: `95,8` → `95.8`,
/// `5c` → `5C`. Empty clears.
fn normalise_dial(band: &str, dial: &str) -> Result<Option<String>, String> {
    let dial = dial.trim();
    if dial.is_empty() {
        return Ok(None);
    }
    match band {
        "fm" => rdns::fm_code(dial)
            .map(|code| {
                let khz10: u32 = code.parse().unwrap_or(0);
                Some(format!("{:.1}", f64::from(khz10) / 100.0))
            })
            .ok_or_else(|| "An FM frequency is in MHz between 87.5 and 108, e.g. 95.8".into()),
        _ => rdns::dab_channel(dial)
            .map(Some)
            .ok_or_else(|| "A DAB channel is a block from 5A to 13F, e.g. 5C".into()),
    }
}

fn clean_text(s: Option<&str>, max: usize) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(max).collect())
}

// ═══════════════════════════════════════════════════════════
//  Stations
// ═══════════════════════════════════════════════════════════

#[derive(Serialize, utoipa::ToSchema)]
pub struct StationList {
    pub stations: Vec<DirectoryStation>,
    /// Every broadcaster area present, for the Browse chips.
    pub regions: Vec<String>,
    /// Every genre label present, most common first.
    pub genres: Vec<String>,
    pub note: &'static str,
}

// GET /api/v1/radio/stations — every station that can be picked.
/// Every station that can be picked.
///
/// Scope: `read`. German public radio for now: every service the nine ARD
/// broadcasters list in their RadioDNS service information, fetched by the
/// server and kept for a day. FM frequency, DAB channel and city are not in
/// that data, which is why a preset asks for them.
#[utoipa::path(
    get,
    path = "/api/v1/radio/stations",
    tag = "Radio",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK. Empty when no broadcaster could be reached.", body = StationList),
        (status = 401, response = Unauthorized),
    )
)]
async fn stations(
    State(state): State<Arc<AppState>>,
    ReadAuth { .. }: ReadAuth,
) -> impl IntoResponse {
    let list = state.radio.directory.stations().await;
    let mut regions: Vec<String> = Vec::new();
    // Keyed case-folded: broadcasters spell one label two ways ("Easy
    // Listening", "Easy listening"), and the first spelling seen is shown.
    let mut genres: std::collections::HashMap<String, (String, usize)> = Default::default();
    for s in list.iter() {
        if !regions.contains(&s.region) {
            regions.push(s.region.clone());
        }
        for g in &s.genres {
            genres.entry(g.to_lowercase()).or_insert_with(|| (g.clone(), 0)).1 += 1;
        }
    }
    regions.sort();
    let mut genres: Vec<(String, usize)> = genres.into_values().collect();
    genres.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Json(StationList {
        stations: list.to_vec(),
        regions,
        genres: genres.into_iter().map(|(g, _)| g).collect(),
        note: "German public radio (ARD) for now. Worldwide comes later.",
    })
    .into_response()
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LookupQuery {
    /// `fm:<gcc>.<pi>.<freq>` or `dab:<gcc>.<eid>.<sid>.<scids>`, e.g.
    /// `fm:de0.d3c3.09440`.
    pub bearer: String,
    /// `YYYY-MM-DD`; today in your timezone when absent.
    pub date: Option<String>,
}

// GET /api/v1/radio/lookup — any station by its broadcast identifiers.
/// Any station by its broadcast identifiers.
///
/// Scope: `read`. Resolves a bearer through RadioDNS to its broadcaster and
/// returns that broadcaster's description of the service and one day's
/// schedule. Works for stations outside the station list — any broadcaster
/// that runs RadioDNS — and is how one is identified from what a radio
/// shows (PI code and frequency, or ensemble and service ID).
#[utoipa::path(
    get,
    path = "/api/v1/radio/lookup",
    operation_id = "radio_lookup",
    tag = "Radio",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(LookupQuery),
    responses(
        (status = 200, description = "Found.", body = StationLookup),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, description = "Not in RadioDNS, or the broadcaster publishes no service information.", body = ErrorResponse),
        (status = 502, description = "DNS or the broadcaster's server failed.", body = ErrorResponse),
    )
)]
async fn lookup(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<LookupQuery>,
) -> impl IntoResponse {
    let bearer = match Bearer::parse(&q.bearer) {
        Ok(b) => b,
        Err(e) => return err(400, e),
    };
    let date = match q.date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => match chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => return err(400, "The date is YYYY-MM-DD"),
        },
        None => today(&state, user.user_id).await,
    };
    match rdns::lookup(&bearer, date, Egress::for_owner(user.is_admin)).await {
        Ok(found) => Json(found).into_response(),
        Err(e @ (rdns::LookupError::NotInRadioDns(_) | rdns::LookupError::NoSpi { .. })) => {
            err(404, e.to_string())
        }
        Err(e) => err(502, e.to_string()),
    }
}

// ═══════════════════════════════════════════════════════════
//  Presets
// ═══════════════════════════════════════════════════════════

#[derive(Serialize, utoipa::ToSchema)]
pub struct PresetList {
    pub presets: Vec<RadioPreset>,
}

// GET /api/v1/radio/presets — your presets, in order.
/// Your presets, in order.
///
/// Scope: `read`.
#[utoipa::path(
    get,
    path = "/api/v1/radio/presets",
    tag = "Radio",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = PresetList),
        (status = 401, response = Unauthorized),
    )
)]
async fn list_presets(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.radio_presets(user.user_id).await {
        Ok(presets) => Json(PresetList { presets }).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct NewPreset {
    /// A station list id, e.g. `de0.d3c3`.
    pub station_id: String,
    /// `fm` or `dab` — which you listen to it on.
    pub band: String,
    /// MHz for FM (`95.8`), the channel block for DAB (`5C`).
    pub dial: Option<String>,
    /// At most 60 characters.
    pub city: Option<String>,
}

// POST /api/v1/radio/presets — save a station.
/// Save a station as a preset.
///
/// Scope: `write`. One preset per station and band.
#[utoipa::path(
    post,
    path = "/api/v1/radio/presets",
    tag = "Radio",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    request_body = NewPreset,
    responses(
        (status = 201, description = "Saved.", body = RadioPreset),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, description = "Not in the station list.", body = ErrorResponse),
        (status = 409, description = "Already a preset on that band.", body = ErrorResponse),
    )
)]
async fn add_preset(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Json(body): Json<NewPreset>,
) -> impl IntoResponse {
    let band = body.band.trim().to_ascii_lowercase();
    if band != "fm" && band != "dab" {
        return err(400, "The band is fm or dab");
    }
    let Some(station) = state.radio.directory.find(body.station_id.trim()).await else {
        return err(404, "That station is not in the station list");
    };
    if !station.has_band(&band) {
        return err(400, format!("{} is not broadcast on {}", station.name, band.to_uppercase()));
    }
    let dial = match normalise_dial(&band, body.dial.as_deref().unwrap_or("")) {
        Ok(d) => d,
        Err(e) => return err(400, e),
    };
    let city = clean_text(body.city.as_deref(), 60);
    match state
        .db
        .add_radio_preset(
            user.user_id,
            &station.id,
            &station.name,
            station.short_name.as_deref(),
            &band,
            dial.as_deref(),
            city.as_deref(),
        )
        .await
    {
        Ok(Some(p)) => (StatusCode::CREATED, Json(p)).into_response(),
        Ok(None) => {
            err(409, format!("{} on {} is already a preset", station.name, band.to_uppercase()))
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// Each field distinguishes absent ("leave alone") from `null` ("clear").
#[derive(Deserialize, utoipa::ToSchema)]
pub struct PresetUpdate {
    #[serde(default, deserialize_with = "double_option_str")]
    #[schema(value_type = Option<String>)]
    pub dial: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option_str")]
    #[schema(value_type = Option<String>)]
    pub city: Option<Option<String>>,
    /// Order on the preset row, from 1.
    pub position: Option<i64>,
}

fn double_option_str<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(de).map(Some)
}

// PATCH /api/v1/radio/presets/:id — where a preset sits.
/// Change a preset's dial position, city or order.
///
/// Scope: `write`.
#[utoipa::path(
    patch,
    path = "/api/v1/radio/presets/{id}",
    tag = "Radio",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path, description = "Preset id")),
    request_body = PresetUpdate,
    responses(
        (status = 200, description = "Changed.", body = RadioPreset),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn update_preset(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    Json(body): Json<PresetUpdate>,
) -> impl IntoResponse {
    let Ok(Some(current)) = state.db.radio_preset(user.user_id, id).await else {
        return err(404, "No such preset");
    };
    let dial = match body.dial {
        None => None,
        Some(d) => match normalise_dial(&current.band, d.as_deref().unwrap_or("")) {
            Ok(d) => Some(d),
            Err(e) => return err(400, e),
        },
    };
    let city = body.city.map(|c| clean_text(c.as_deref(), 60));
    let position = body.position.map(|p| p.clamp(1, 999));
    match state
        .db
        .update_radio_preset(
            user.user_id,
            id,
            dial.as_ref().map(|d| d.as_deref()),
            city.as_ref().map(|c| c.as_deref()),
            position,
        )
        .await
    {
        Ok(true) => match state.db.radio_preset(user.user_id, id).await {
            Ok(Some(p)) => Json(p).into_response(),
            _ => err(404, "No such preset"),
        },
        Ok(false) => err(404, "No such preset"),
        Err(e) => err(500, e.to_string()),
    }
}

// DELETE /api/v1/radio/presets/:id
/// Remove a preset.
///
/// Scope: `write`. The air log and any listens it produced stay.
#[utoipa::path(
    delete,
    path = "/api/v1/radio/presets/{id}",
    tag = "Radio",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path, description = "Preset id")),
    responses(
        (status = 204, description = "Removed."),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn delete_preset(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_radio_preset(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(404, "No such preset"),
        Err(e) => err(500, e.to_string()),
    }
}

// ═══════════════════════════════════════════════════════════
//  Tuner
// ═══════════════════════════════════════════════════════════

// GET /api/v1/radio/tuner — what the tuner is doing.
/// What the tuner is doing.
///
/// Scope: `read`. `tuned: false` when it is off.
#[utoipa::path(
    get,
    path = "/api/v1/radio/tuner",
    tag = "Radio",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = TunerStatus),
        (status = 401, response = Unauthorized),
    )
)]
async fn tuner(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    Json(state.radio.status(user.user_id)).into_response()
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct TuneRequest {
    pub preset_id: i64,
}

// POST /api/v1/radio/tuner — tune in.
/// Tune in to a preset.
///
/// Scope: `write`. From now until you turn it off, tune elsewhere or the
/// auto-stop runs out, the server reads the station's stream for track names
/// and logs them under your rules. Replaces whatever you were tuned to.
#[utoipa::path(
    post,
    path = "/api/v1/radio/tuner",
    tag = "Radio",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    request_body = TuneRequest,
    responses(
        (status = 200, description = "Tuned in.", body = TunerStatus),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 409, description = "The station is not in the list, or publishes no stream.", body = ErrorResponse),
    )
)]
async fn tune(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Json(body): Json<TuneRequest>,
) -> impl IntoResponse {
    let Ok(Some(preset)) = state.db.radio_preset(user.user_id, body.preset_id).await else {
        return err(404, "No such preset");
    };
    match radio::tune_in(&state, user.user_id, user.is_admin, preset).await {
        Ok(status) => Json(status).into_response(),
        Err(radio::TuneError(code, msg)) => err(code, msg),
    }
}

// DELETE /api/v1/radio/tuner — off.
/// Turn the tuner off.
///
/// Scope: `write`. Whatever was playing is judged as cut short.
#[utoipa::path(
    delete,
    path = "/api/v1/radio/tuner",
    tag = "Radio",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "Off.", body = TunerStatus),
        (status = 401, response = Unauthorized),
    )
)]
async fn tune_off(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
) -> impl IntoResponse {
    radio::stop(&state, user.user_id, "off").await;
    Json(state.radio.status(user.user_id)).into_response()
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ScheduleQuery {
    pub preset_id: i64,
}

// GET /api/v1/radio/schedule — today on a preset's station.
/// Today's programmes on a preset's station.
///
/// Scope: `read`. From RadioDNS; `status` says when the broadcaster publishes
/// none. Cached for half an hour.
#[utoipa::path(
    get,
    path = "/api/v1/radio/schedule",
    tag = "Radio",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(ScheduleQuery),
    responses(
        (status = 200, description = "OK.", body = ScheduleOut),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 409, description = "The station is not in the list right now.", body = ErrorResponse),
    )
)]
async fn schedule(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<ScheduleQuery>,
) -> impl IntoResponse {
    let Ok(Some(preset)) = state.db.radio_preset(user.user_id, q.preset_id).await else {
        return err(404, "No such preset");
    };
    let date = today(&state, user.user_id).await;
    match state.radio.schedule(&preset, date).await {
        Some(s) => Json(s).into_response(),
        None => err(409, "That station is not in the station list right now"),
    }
}

// ═══════════════════════════════════════════════════════════
//  Air log, review queue, recently tuned
// ═══════════════════════════════════════════════════════════

#[derive(Serialize, utoipa::ToSchema)]
pub struct AirLog {
    /// Newest first.
    pub entries: Vec<AirLogEntry>,
    /// Lines waiting for Keep or Drop, oldest first, however far back.
    pub review: Vec<AirLogEntry>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogQuery {
    #[param(minimum = 1, maximum = 200, default = 50)]
    pub limit: Option<i64>,
}

// GET /api/v1/radio/log — what the stations said.
/// The air log and the review queue.
///
/// Scope: `read`. Every line a station sent while you were tuned in —
/// tracks, guesses, breaks — and what became of it. Only `scrobbled` and
/// `kept` lines are listens.
#[utoipa::path(
    get,
    path = "/api/v1/radio/log",
    tag = "Radio",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(LogQuery),
    responses(
        (status = 200, description = "OK.", body = AirLog),
        (status = 401, response = Unauthorized),
    )
)]
async fn air_log(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<LogQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let entries = state.db.air_log(user.user_id, limit).await;
    let review = state.db.air_review_queue(user.user_id).await;
    match (entries, review) {
        (Ok(entries), Ok(review)) => Json(AirLog { entries, review }).into_response(),
        (Err(e), _) | (_, Err(e)) => {
            error!("📻 Air log: {e}");
            err(500, "Could not read the air log")
        }
    }
}

// POST /api/v1/radio/log/:id/keep — a guess is right.
/// Keep a line from the review queue: write it as a listen.
///
/// Scope: `write`. It is forwarded like any other listen.
#[utoipa::path(
    post,
    path = "/api/v1/radio/log/{id}/keep",
    tag = "Radio",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path, description = "Air log line id")),
    responses(
        (status = 200, description = "Kept.", body = AirLogEntry),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 409, description = "Already settled, or already in your history.", body = ErrorResponse),
    )
)]
async fn keep_line(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match radio::keep(&state, user.user_id, id).await {
        Ok(line) => Json(line).into_response(),
        Err(radio::TuneError(code, msg)) => err(code, msg),
    }
}

// POST /api/v1/radio/log/:id/drop — a guess is wrong.
/// Drop a line from the review queue.
///
/// Scope: `write`. It stays in the air log, marked dropped, and never becomes
/// a listen.
#[utoipa::path(
    post,
    path = "/api/v1/radio/log/{id}/drop",
    tag = "Radio",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path, description = "Air log line id")),
    responses(
        (status = 200, description = "Dropped.", body = AirLogEntry),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 409, description = "Already settled.", body = ErrorResponse),
    )
)]
async fn drop_line(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match radio::drop_line(&state, user.user_id, id).await {
        Ok(line) => Json(line).into_response(),
        Err(radio::TuneError(code, msg)) => err(code, msg),
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RecentTuned {
    /// Newest first, one per station and band.
    pub recent: Vec<RadioSession>,
}

// GET /api/v1/radio/recent — recently tuned.
/// Stations you tuned to recently.
///
/// Scope: `read`.
#[utoipa::path(
    get,
    path = "/api/v1/radio/recent",
    tag = "Radio",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = RecentTuned),
        (status = 401, response = Unauthorized),
    )
)]
async fn recent(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.recent_radio(user.user_id, 5).await {
        Ok(recent) => Json(RecentTuned { recent }).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// ═══════════════════════════════════════════════════════════
//  Settings
// ═══════════════════════════════════════════════════════════

// GET /api/v1/radio/settings — the scrobble rules.
/// Your radio scrobble rules.
///
/// Scope: `read`.
#[utoipa::path(
    get,
    path = "/api/v1/radio/settings",
    operation_id = "radio_get_settings",
    tag = "Radio",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = RadioSettings),
        (status = 401, response = Unauthorized),
    )
)]
async fn get_settings(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    let prefs = state.db.get_preferences(user.user_id).await.unwrap_or_default();
    Json(RadioSettings::from_prefs(&prefs)).into_response()
}

// PUT /api/v1/radio/settings — replace the rules.
/// Replace your radio scrobble rules.
///
/// Scope: `write`. The whole object; a missing field takes its default. A
/// running tuner picks up the change at once — the auto-stop moves, and the
/// next track is judged by the new rules — except the chain, which a session
/// keeps from when it started.
#[utoipa::path(
    put,
    path = "/api/v1/radio/settings",
    operation_id = "radio_put_settings",
    tag = "Radio",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    request_body = RadioSettings,
    responses(
        (status = 200, description = "Saved.", body = RadioSettings),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn put_settings(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Json(mut body): Json<RadioSettings>,
) -> impl IntoResponse {
    body.auto_stop_hours = body.auto_stop_hours.clamp(1, 12);
    if let Some(cid) = body.chain_id {
        if !matches!(state.db.get_chain(user.user_id, cid).await, Ok(Some(_))) {
            return err(400, "No such signal chain");
        }
    }
    let patch = serde_json::json!({ "radio": body });
    if let Err(e) = state.db.merge_preferences(user.user_id, &patch).await {
        return err(500, e.to_string());
    }
    state.radio.apply_settings(user.user_id, &body);
    Json(body).into_response()
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    stations,
    lookup,
    list_presets,
    add_preset,
    update_preset,
    delete_preset,
    tuner,
    tune,
    tune_off,
    schedule,
    air_log,
    keep_line,
    drop_line,
    recent,
    get_settings,
    put_settings
))]
pub struct Api;

pub fn create_radio_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/radio/stations", get(stations))
        .route("/api/v1/radio/lookup", get(lookup))
        .route("/api/v1/radio/presets", get(list_presets).post(add_preset))
        .route(
            "/api/v1/radio/presets/:id",
            axum::routing::patch(update_preset).delete(delete_preset),
        )
        .route("/api/v1/radio/tuner", get(tuner).post(tune).delete(tune_off))
        .route("/api/v1/radio/schedule", get(schedule))
        .route("/api/v1/radio/log", get(air_log))
        .route("/api/v1/radio/log/:id/keep", post(keep_line))
        .route("/api/v1/radio/log/:id/drop", post(drop_line))
        .route("/api/v1/radio/recent", get(recent))
        .route("/api/v1/radio/settings", get(get_settings).put(put_settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dial_positions_are_normalised_per_band() {
        assert_eq!(normalise_dial("fm", "95,8").unwrap().as_deref(), Some("95.8"));
        assert_eq!(normalise_dial("fm", "100.60").unwrap().as_deref(), Some("100.6"));
        assert_eq!(normalise_dial("dab", "5c").unwrap().as_deref(), Some("5C"));
        assert_eq!(normalise_dial("fm", "  ").unwrap(), None);
        assert!(normalise_dial("fm", "5C").is_err());
        assert!(normalise_dial("dab", "95.8").is_err());
    }

    #[test]
    fn an_absent_field_is_not_a_null_one() {
        let u: PresetUpdate = serde_json::from_str(r#"{"city": null}"#).unwrap();
        assert!(u.dial.is_none(), "absent leaves alone");
        assert_eq!(u.city, Some(None), "null clears");
    }
}
