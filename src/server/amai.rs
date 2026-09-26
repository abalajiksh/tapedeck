//! Configuring the AudioMuse-AI sidecar, and asking it the first question
//! worth asking.
//!
//! **Server-wide and admin-only**, like the Discogs credential and the TIDAL
//! registration rather than like a `user_connections` row: amai is one box the
//! operator runs, analysing one library, and every user of this instance reads
//! the same catalogue. It is also **session-only** — a token gets 401 here and
//! scopes never enter into it, the line `/api/v1/sources` and the Discogs
//! settings already sit on. A scrobble client has no business pointing this
//! instance at a new host.
//!
//! ## Why there is a Test button, and why it asks twice
//!
//! Their `before_request` barrier exempts `/api/health` and nothing else, so
//! *reachable* and *credentialled* are separately answerable — and the two need
//! completely different things from the operator. Test asks both and says which
//! failed. That is the Discogs lesson inverted: there a bad credential comes
//! back as a normal 200 and a typo is indistinguishable from success, so the
//! button exists to catch a silent failure; here a bad token is a clean 401 and
//! the button exists because *reachability* is the half that would otherwise be
//! guessed at.
//!
//! ## Why there is a coverage probe
//!
//! Because the decision that comes next rests on one number nobody has: what
//! fraction of this history can actually be joined to amai. The join is on a
//! media-server provider id, so a listen that never came off Plex, Jellyfin or
//! Navidrome cannot be joined at all — and on a real instance 26,100 of 76,806
//! listens are a Spotify import, which is the reason to measure before building
//! rather than after. Same discipline that killed DuckDB and simd-json, and the
//! one the ISRC work skipped: that shipped a matcher whose ceiling was 43% and
//! whose ceiling did not cover the tracks that were failing.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use utoipa::{IntoParams, ToSchema};

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::amai::{self, Amai, AmaiServer, Lookup};
use crate::openapi::Unauthorized;

/// Where the AudioMuse-AI sidecar is, and never its token.
#[derive(Serialize, ToSchema)]
pub struct AmaiSettings {
    pub configured: bool,
    /// The address. Not a credential, and shown the way a source's URL is.
    pub url: Option<String>,
    pub has_token: bool,
    /// Which of amai's media servers this instance's provider ids belong to.
    /// Absent means amai's default server.
    pub server: Option<String>,
    /// Something is stored in this instance's settings — which beats the
    /// environment.
    pub stored: bool,
    /// `TAPEDECK_AMAI_URL` is set.
    pub from_env: bool,
}

/// Whether the sidecar is configured now.
#[derive(Serialize, ToSchema)]
pub struct AmaiConfigured {
    pub configured: bool,
}

/// Both halves of the check, reported separately.
#[derive(Serialize, ToSchema)]
pub struct AmaiTest {
    pub ok: bool,
    pub reachable: bool,
    pub authenticated: bool,
    pub message: String,
    /// amai's media servers. Present only when the token was accepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub servers: Option<Vec<AmaiServer>>,
}

/// A most-played track amai had not analysed.
#[derive(Serialize, ToSchema)]
pub struct AmaiMissing {
    pub artist: String,
    pub title: String,
    pub plays: i64,
}

/// How much of this history AudioMuse-AI can answer for. Every share is a
/// percentage, and null when its population is empty.
#[derive(Serialize, ToSchema)]
pub struct AmaiCoverage {
    pub total_plays: i64,
    pub distinct_tracks: usize,
    /// Tracks that came off a media server, so could join at all.
    pub joinable_tracks: usize,
    pub joinable_plays: i64,
    /// The structural ceiling, over the whole history.
    pub joinable_share_of_listening: Option<f64>,
    /// Joinable tracks actually asked about, most-played first.
    pub asked: usize,
    pub analysed: usize,
    pub analysed_share_of_asked: Option<f64>,
    /// Weighted by plays — the figure the decision rests on.
    pub analysed_share_of_sampled_listening: Option<f64>,
    /// What share of all listening the sample is.
    pub sample_share_of_listening: Option<f64>,
    pub sample_requested: usize,
    /// The probe stopped at its deadline before asking about the whole sample.
    pub deadline_hit: bool,
    /// The first failed ask. A failed ask is never counted as a miss.
    pub error: Option<String>,
    pub server_asked: Option<String>,
    /// Up to twenty.
    pub examples_not_analysed: Vec<AmaiMissing>,
}

/// How many of the most-played tracks the probe asks about.
///
/// Weighted by plays rather than sampled evenly, and that is the *right*
/// weighting rather than a convenient one: a track played fifty times matters
/// fifty times as much to anything built on this, and the response says what
/// share of total listening the sample covers so the figure is never read as a
/// fact about the whole history. The same shape as the 2026-08-05 Every Noise
/// measurement — "the top 100 artists of a real 21,390-listen history (74.5% of
/// listening)".
const DEFAULT_SAMPLE: usize = 100;
const MAX_SAMPLE: usize = 500;

/// One request per track, inside a request handler, so the handler owns a
/// budget rather than trusting N × the client timeout to be survivable.
///
/// The rule earned three times over — a session rebuild's 524, the whole-record
/// button's 502 and the running-times lookup — is that the worst thing an
/// endpoint can do is not answer, because then a proxy answers instead and its
/// error says nothing about the question that was asked.
const PROBE_DEADLINE: Duration = Duration::from_secs(20);

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Where the instance is, stored first and then the environment.
///
/// **Stored wins**, the rule the TIDAL registration already follows: what
/// somebody typed into this instance's settings screen is a more deliberate
/// statement than a variable inherited from a shell.
async fn configured(state: &AppState) -> Option<Amai> {
    let url = match state.db.get_app_setting(amai::URL_KEY).await {
        Ok(Some(u)) => Some(u),
        // A failed read is not an absence. Reporting "amai is not configured"
        // because the database could not be asked is the `unwrap_or(true)` that
        // told an instance owner their history was gone.
        Ok(None) => None,
        Err(e) => {
            tracing::warn!("Could not read the AudioMuse-AI address: {e}");
            return None;
        }
    }
    .or_else(|| env_nonempty("TAPEDECK_AMAI_URL"))?;

    let token = state
        .db
        .get_app_setting(amai::TOKEN_KEY)
        .await
        .ok()
        .flatten()
        .or_else(|| env_nonempty("TAPEDECK_AMAI_TOKEN"));
    let server = state
        .db
        .get_app_setting(amai::SERVER_KEY)
        .await
        .ok()
        .flatten()
        .or_else(|| env_nonempty("TAPEDECK_AMAI_SERVER"));

    match Amai::new(&url, token, server) {
        Ok(a) => Some(a),
        Err(e) => {
            tracing::warn!("The stored AudioMuse-AI address is unusable: {e}");
            None
        }
    }
}

// ════════════════════════════════════════════════════════════
//  Settings
// ════════════════════════════════════════════════════════════

/// Whether the AudioMuse-AI sidecar is configured.
///
/// Admin, session-only. Returns the address — which is not a credential and
/// is shown the way a source's URL is — and never the API token.
#[utoipa::path(
    get,
    path = "/api/v1/amai/settings",
    tag = "Settings",
    responses(
        (status = 200, description = "OK.", body = AmaiSettings),
        (status = 401, response = Unauthorized),
        (status = 403, description = "Admin only.", body = ErrorResponse),
    )
)]
async fn get_settings(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let stored_url = state.db.get_app_setting(amai::URL_KEY).await.unwrap_or(None);
    let stored_token = state.db.get_app_setting(amai::TOKEN_KEY).await.unwrap_or(None);
    let stored_server = state.db.get_app_setting(amai::SERVER_KEY).await.unwrap_or(None);
    let env_url = env_nonempty("TAPEDECK_AMAI_URL");

    let client = configured(&state).await;
    (
        StatusCode::OK,
        Json(AmaiSettings {
            configured: client.is_some(),
            // The address is not a credential and is shown, exactly as a
            // source's URL is. The token never comes back over the wire.
            url: client.as_ref().map(|c| c.base_url().to_string()),
            has_token: client.as_ref().is_some_and(Amai::has_token),
            server: client.as_ref().and_then(|c| c.server().map(str::to_string)),
            stored: stored_url.is_some() || stored_token.is_some() || stored_server.is_some(),
            from_env: env_url.is_some(),
        }),
    )
        .into_response()
}

/// Each field is written only when present; an empty string clears it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AmaiUpdate {
    /// `http://host:8000`. Empty clears it and switches the sidecar off. No
    /// scheme is a 400 rather than a guess.
    #[serde(default)]
    pub url: Option<String>,
    /// Their `API_TOKEN`. Empty clears it.
    #[serde(default)]
    pub token: Option<String>,
    /// Which of amai's media servers our provider ids belong to — its display
    /// name or its internal id. Empty means their default server, which is the
    /// whole answer on a single-server install.
    #[serde(default)]
    pub server: Option<String>,
}

/// Point this instance at an AudioMuse-AI instance.
///
/// Admin, session-only — a token gets 401 and scopes never enter into it,
/// the line `/api/v1/sources` already sits on. Server-wide rather than
/// per-user: amai is one box the operator runs over one library, and every
/// user here reads the same catalogue. Stored encrypted, and a stored value
/// beats `TAPEDECK_AMAI_URL` / `_TOKEN` / `_SERVER`.
///
/// Each field is written only when present, so saving one does not wipe
/// another; clearing is an explicit empty string. An address with no
/// `http://` or `https://` scheme is a 400 rather than being guessed at —
/// guessing `http` would silently downgrade an instance meant to be reached
/// over TLS.
///
/// `server` names which of amai's own media servers this instance's
/// provider ids belong to (its display name or internal id). Absent means
/// amai's default server, which is the whole answer on a single-server
/// install.
#[utoipa::path(
    put,
    path = "/api/v1/amai/settings",
    tag = "Settings",
    request_body = AmaiUpdate,
    responses(
        (status = 200, description = "Saved.", body = AmaiConfigured),
        (status = 400, description = "The address is unusable.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, description = "Admin only.", body = ErrorResponse),
    )
)]
async fn set_settings(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<AmaiUpdate>,
) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    // Refuse an unusable address at the door rather than storing it and
    // reporting "not configured" afterwards, which reads as the save having
    // failed silently.
    if let Some(u) = body.url.as_ref().map(|u| u.trim()) {
        if !u.is_empty() {
            if let Err(e) = Amai::new(u, None, None) {
                return err(400, e.to_string());
            }
        }
    }
    // Each field is written only when present, so saving a server name does not
    // wipe a token that was already working, and clearing one is an explicit
    // empty string rather than an omission.
    for (key, value) in [
        (amai::URL_KEY, &body.url),
        (amai::TOKEN_KEY, &body.token),
        (amai::SERVER_KEY, &body.server),
    ] {
        if let Some(v) = value {
            if let Err(e) = state.db.set_app_setting(key, v.trim()).await {
                return err(500, e.to_string());
            }
        }
    }
    let configured = configured(&state).await.is_some();
    (StatusCode::OK, Json(AmaiConfigured { configured })).into_response()
}

// ════════════════════════════════════════════════════════════
//  Test
// ════════════════════════════════════════════════════════════

// POST /api/v1/amai/test — is it there, and does the token work?
//
// Both halves are reported rather than collapsed into one boolean. "Running,
// wrong token" and "not running" are the same colour on a screen that only
// says `ok: false`, and they need completely different things doing about
// them.
/// Is AudioMuse-AI there, and does the token work?
///
/// Admin, session-only. **Two questions, reported separately**, because
/// they need completely different things doing about them and a screen
/// that only says `ok: false` makes them the same colour. amai's own
/// `before_request` barrier exempts `/api/health` and nothing else, so
/// reachability is answerable without a credential and a bad token is a
/// clean 401 — the inverse of the Discogs trap, where a bad credential is
/// served as a normal 200.
///
/// A rejection is a 200 with `ok: false`, not an HTTP error: the request to
/// Tapedeck succeeded and the answer is "no".
///
/// `servers` carries only `server_id`, `name`, `server_type` and
/// `is_default`. amai's own response also holds a masked `creds` map;
/// nothing here stores or logs it.
#[utoipa::path(
    post,
    path = "/api/v1/amai/test",
    tag = "Settings",
    responses(
        (status = 200, description = "Result of the check.", body = AmaiTest),
        (status = 400, description = "Not configured on this server.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, description = "Admin only.", body = ErrorResponse),
    )
)]
async fn test(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let Some(client) = configured(&state).await else {
        return err(400, "AudioMuse-AI isn't configured on this server");
    };

    if let Err(e) = client.reachable().await {
        // A 200 with `ok: false`, not an HTTP error: the request to Tapedeck
        // succeeded and the answer is "no". Same call the Discogs test makes.
        return (
            StatusCode::OK,
            Json(AmaiTest {
                ok: false,
                reachable: false,
                authenticated: false,
                message: format!("Could not reach {}: {e}", client.base_url()),
                servers: None,
            }),
        )
            .into_response();
    }

    match client.servers().await {
        Ok(servers) => {
            let names: Vec<String> = servers.iter().map(|s| s.name.clone()).collect();
            // With one server configured an absent `?server=` is unambiguous.
            // With two, an id that belongs to the other one comes back as "not
            // analysed" — indistinguishable, from here, from a track amai has
            // genuinely never seen. So the count is reported and the panel says
            // to name one.
            let message = match names.len() {
                0 => "Reachable and the token works, but AudioMuse-AI has no media \
                      server configured yet — analyse a library there first."
                    .to_string(),
                1 => format!("Working — reachable, token accepted, analysing {}.", names[0]),
                n => format!(
                    "Working — reachable, token accepted. {n} media servers there ({}). \
                     Name the one your listens come from, or ids from the others will \
                     read as unanalysed.",
                    names.join(", ")
                ),
            };
            (
                StatusCode::OK,
                Json(AmaiTest {
                    ok: true,
                    reachable: true,
                    authenticated: true,
                    servers: Some(servers),
                    message,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::OK,
            Json(AmaiTest {
                ok: false,
                // The half that did work, stated, because it narrows the
                // problem to exactly one thing.
                reachable: true,
                authenticated: false,
                message: e.to_string(),
                servers: None,
            }),
        )
            .into_response(),
    }
}

// ════════════════════════════════════════════════════════════
//  Coverage
// ════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CoverageQuery {
    /// How many of the most-played joinable tracks to ask about. Default 100,
    /// at most 500.
    #[serde(default)]
    #[param(minimum = 1, maximum = 500)]
    pub sample: Option<usize>,
}

// GET /api/v1/amai/coverage — how much of this history can amai answer for?
//
// Three numbers, and they narrow in this order:
//
// 1. **The structural ceiling.** A listen joins on a media-server provider id,
//    so one that never came off Plex, Jellyfin or Navidrome cannot join at
//    all, whatever amai has analysed. This is a fact about the *history* and
//    no amount of analysis moves it.
// 2. **What amai has**, of the tracks that could join — measured, on the
//    most-played sample.
// 3. **What share of listening that sample is**, so neither of the above is
//    read as a fact about the whole history.
/// How much of this history can AudioMuse-AI answer for?
///
/// Admin, session-only. The measurement that decides whether anything is
/// worth building on the sidecar, and it narrows in three steps.
///
/// **The structural ceiling.** A listen joins to amai on a media-server
/// provider id, so one that never came off Plex, Jellyfin or Navidrome
/// cannot join at all whatever amai has analysed. `joinable_*` is that,
/// computed over the whole history because it is a plain string test.
///
/// **What amai has**, measured on the most-played tracks. Weighted by
/// plays rather than sampled evenly, because a track played fifty times is
/// worth fifty times as much to anything built on these vectors.
///
/// **What the sample is a sample of** — `sample_share_of_listening` — so
/// the figure above is never read as a fact about the whole history. A
/// share stated without its population is the fidelity-card failure.
///
/// One request per track against amai, so the handler owns a deadline
/// rather than trusting N × the client timeout. `deadline_hit` says when it
/// stopped early, and a failed ask is never counted as a miss: `error`
/// carries the first one.
#[utoipa::path(
    get,
    path = "/api/v1/amai/coverage",
    tag = "Settings",
    params(CoverageQuery),
    responses(
        (status = 200, description = "The measurement.", body = AmaiCoverage),
        (status = 400, description = "Not configured, or there are no listens to measure.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, description = "Admin only.", body = ErrorResponse),
    )
)]
async fn coverage(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<CoverageQuery>,
) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let Some(client) = configured(&state).await else {
        return err(400, "AudioMuse-AI isn't configured on this server");
    };
    let want = q.sample.unwrap_or(DEFAULT_SAMPLE).clamp(1, MAX_SAMPLE);

    let tracks = match state.db.tracks_by_plays(user.user_id).await {
        Ok(t) => t,
        Err(e) => return err(500, e.to_string()),
    };
    if tracks.is_empty() {
        return err(400, "There are no listens to measure yet");
    }

    let total_plays: i64 = tracks.iter().map(|t| t.plays).sum();
    let distinct_tracks = tracks.len();

    // The ceiling, over the whole history rather than over the sample: it is a
    // plain string test, so there is no reason to estimate something exact.
    let joinable: Vec<&crate::db::TrackByPlays> = tracks
        .iter()
        .filter(|t| super::artwork::remote_item_id(&t.source_name, &t.source_id).is_some())
        .collect();
    let joinable_plays: i64 = joinable.iter().map(|t| t.plays).sum();

    // Sequential, and on a LAN box with no rate limit that is fast enough
    // (~20 ms each) that concurrency would buy latency at the cost of being
    // able to stop cleanly at the deadline.
    let started = Instant::now();
    let mut asked = 0usize;
    let mut analysed = 0usize;
    let mut analysed_plays = 0i64;
    let mut asked_plays = 0i64;
    let mut first_error: Option<String> = None;
    let mut missing: Vec<AmaiMissing> = Vec::new();

    for t in joinable.iter().take(want) {
        if started.elapsed() >= PROBE_DEADLINE {
            break;
        }
        let Some(id) = super::artwork::remote_item_id(&t.source_name, &t.source_id) else {
            continue;
        };
        match client.score(&id).await {
            Ok(Lookup::Found(_)) => {
                analysed += 1;
                analysed_plays += t.plays;
            }
            Ok(Lookup::NotAnalysed) => {
                if missing.len() < 20 {
                    missing.push(AmaiMissing {
                        artist: t.artist.clone(),
                        title: t.title.clone(),
                        plays: t.plays,
                    });
                }
            }
            Err(e) => {
                // A failed ask is not a miss, so it is not counted as one. The
                // breaker will stop the rest of the loop dead after three, which
                // is the correct outcome — a partial answer that says how
                // partial it is beats a confident wrong one.
                first_error.get_or_insert_with(|| e.to_string());
                continue;
            }
        }
        asked += 1;
        asked_plays += t.plays;
    }

    let pct = |part: i64, whole: i64| -> Option<f64> {
        (whole > 0).then(|| (part as f64 / whole as f64) * 100.0)
    };

    (
        StatusCode::OK,
        Json(AmaiCoverage {
            total_plays,
            distinct_tracks,

            // 1. The ceiling.
            joinable_tracks: joinable.len(),
            joinable_plays,
            joinable_share_of_listening: pct(joinable_plays, total_plays),

            // 2. What amai has, of what was actually asked.
            asked,
            analysed,
            analysed_share_of_asked: pct(analysed as i64, asked as i64),
            // Weighted by plays, which is the figure the decision actually
            // rests on: a track played fifty times is worth fifty times as
            // much to anything built on these vectors, so a share of *tracks*
            // would understate a well-covered library and overstate a thin one.
            analysed_share_of_sampled_listening: pct(analysed_plays, asked_plays),

            // 3. What the sample is a sample of. Without this the figure above
            //    reads as a fact about the whole history, which is the
            //    fidelity-card failure in a new place.
            sample_share_of_listening: pct(asked_plays, total_plays),

            sample_requested: want,
            deadline_hit: started.elapsed() >= PROBE_DEADLINE,
            error: first_error,
            server_asked: client.server().map(str::to_string),
            examples_not_analysed: missing,
        }),
    )
        .into_response()
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_settings, set_settings, test, coverage))]
pub struct Api;

pub fn create_amai_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/amai/settings", get(get_settings))
        .route("/api/v1/amai/settings", put(set_settings))
        .route("/api/v1/amai/test", post(test))
        .route("/api/v1/amai/coverage", get(coverage))
}
