//! Connecting a TIDAL account, so a playlist can be pushed to it.
//!
//! Authorization Code + PKCE. The instance is the client, the operator
//! registered the app, and the token that comes back belongs to one user —
//! stored in `user_connections` under `tidal::SERVICE`, encrypted at rest like
//! every other credential this application replays.
//!
//! **Session-only, permanently.** Connecting spends nothing but reaches the
//! user's streaming account, and disconnecting revokes it; both sit on the same
//! side of the line as `/api/v1/sources`, which is the line `WriteAuth`
//! deliberately does not cross. A scrobble client has no business connecting a
//! TIDAL account.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use tracing::{info, warn};

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::tidal::{self, TidalApp, TidalCredentials};

/// How long a half-finished connect attempt is worth remembering.
const PENDING_TTL_SECS: i64 = 600;

/// The PKCE verifier, between the user clicking Connect and TIDAL redirecting
/// back.
///
/// **In memory rather than in the database**, and the trade is deliberate: this
/// is a ten-minute window, the whole of it lives inside one browser round trip,
/// and the cost of losing it to a restart is that the user clicks Connect
/// again. A table would be schema and a migration for state that is worthless
/// sixty seconds later. Same call `server/import.rs` makes for live job
/// progress.
struct Pending {
    user_id: i64,
    verifier: String,
    created_at: i64,
}

fn pending() -> &'static RwLock<HashMap<String, Pending>> {
    static P: OnceLock<RwLock<HashMap<String, Pending>>> = OnceLock::new();
    P.get_or_init(|| RwLock::new(HashMap::new()))
}

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

fn random_token() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::rng().random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The redirect URI, which must match what is registered at TIDAL **exactly**.
///
/// Derived from `PUBLIC_URL` rather than from the request, for the reason
/// v0.63.2 records: behind a tunnel the origin is reached over plain HTTP while
/// the browser is on HTTPS, and an `http://` redirect either fails the exact
/// match or hands an authorization code to a plaintext URL.
fn redirect_uri(state: &AppState, headers: &HeaderMap) -> String {
    format!("{}/api/v1/tidal/callback", super::mcp::public_origin(state, headers))
}

fn app_or_err() -> Result<TidalApp, Box<axum::response::Response>> {
    TidalApp::from_env().ok_or_else(|| {
        Box::new(err(
            501,
            "TIDAL is not configured on this instance — set TAPEDECK_TIDAL_CLIENT_ID and \
             TAPEDECK_TIDAL_CLIENT_SECRET from an app registered at developer.tidal.com",
        ))
    })
}

/// GET /api/v1/tidal/status — whether this user has TIDAL connected.
///
/// Reports `configured` separately from `connected`: an instance whose operator
/// has set no client id cannot offer the button at all, and that is a different
/// thing from an account nobody has linked yet. Saying so is the same courtesy
/// Discogs gets for "not configured".
async fn status(
    State(state): State<std::sync::Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    let configured = TidalApp::from_env().is_some();
    let connected = state
        .db
        .list_connections(user.user_id)
        .await
        .map(|rows| rows.iter().any(|(s, _, _)| s == tidal::SERVICE))
        .unwrap_or(false);
    (StatusCode::OK, Json(serde_json::json!({ "configured": configured, "connected": connected })))
        .into_response()
}

/// POST /api/v1/tidal/connect — begin the flow, and hand back the URL to open.
///
/// The browser is sent to TIDAL rather than redirected from here so the SPA can
/// open it in a new tab and keep its own state, which is the shape the Last.fm
/// connect flow already uses.
async fn connect(
    State(state): State<std::sync::Arc<AppState>>,
    user: AuthUser,
    headers: HeaderMap,
) -> impl IntoResponse {
    let app = match app_or_err() {
        Ok(a) => a,
        Err(r) => return *r,
    };

    // PKCE. `S256` only — `plain` is in RFC 7636 and is no protection at all
    // against the interception the mechanism exists for.
    let verifier = random_token();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let csrf = random_token();

    if let Ok(mut map) = pending().write() {
        let now = tidal::now_secs();
        map.retain(|_, p| now - p.created_at < PENDING_TTL_SECS);
        map.insert(csrf.clone(), Pending { user_id: user.user_id, verifier, created_at: now });
    }

    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge_method=S256&code_challenge={}&state={}",
        tidal::AUTHORIZE_URL,
        tidal::urlencoding(&app.client_id),
        tidal::urlencoding(&redirect_uri(&state, &headers)),
        tidal::urlencoding(tidal::SCOPES),
        tidal::urlencoding(&challenge),
        tidal::urlencoding(&csrf),
    );
    (StatusCode::OK, Json(serde_json::json!({ "authorize_url": url }))).into_response()
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// GET /api/v1/tidal/callback — where TIDAL sends the browser back.
///
/// **Deliberately takes no `AuthUser`.** The owner comes from the `state` token
/// minted at connect time, which is single-use and expires; requiring a session
/// cookie here would additionally depend on it surviving a cross-site
/// navigation from `login.tidal.com`, which is a `SameSite` question with no
/// good answer. The state token *is* the CSRF protection, which is what it is
/// for.
async fn callback(
    State(state): State<std::sync::Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> impl IntoResponse {
    // The user declining is a normal outcome, not a failure.
    if let Some(e) = q.error {
        let detail = q.error_description.unwrap_or(e);
        warn!("TIDAL authorization was refused: {}", detail);
        return Redirect::to("/settings?section=connections&tidal=denied").into_response();
    }
    let (Some(code), Some(csrf)) = (q.code, q.state) else {
        return err(400, "TIDAL sent no authorization code");
    };

    // Single use in both directions: taken out of the map whether or not the
    // exchange then succeeds, so a code cannot be replayed against it.
    let claimed = pending().write().ok().and_then(|mut m| m.remove(&csrf));
    let Some(p) = claimed else {
        return err(400, "That TIDAL sign-in has expired or was already used — start again");
    };
    if tidal::now_secs() - p.created_at >= PENDING_TTL_SECS {
        return err(400, "That TIDAL sign-in took too long — start again");
    }

    let app = match app_or_err() {
        Ok(a) => a,
        Err(r) => return *r,
    };
    let creds =
        match tidal::exchange_code(&app, &code, &p.verifier, &redirect_uri(&state, &headers)).await
        {
            Ok(c) => c,
            Err(e) => return err(502, e),
        };

    if let Err(e) = store(&state, p.user_id, &creds, None).await {
        return err(500, e);
    }
    info!("🎵 TIDAL connected for user {}", p.user_id);
    Redirect::to("/settings?section=connections&tidal=connected").into_response()
}

/// DELETE /api/v1/tidal/disconnect — forget the account.
async fn disconnect(
    State(state): State<std::sync::Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.db.delete_connection(user.user_id, tidal::SERVICE).await {
        Ok(removed) => {
            (StatusCode::OK, Json(serde_json::json!({ "disconnected": removed }))).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// Persist credentials, encrypted, under this user's TIDAL connection.
pub async fn store(
    state: &AppState,
    user_id: i64,
    creds: &TidalCredentials,
    username: Option<&str>,
) -> Result<(), String> {
    let json = serde_json::to_string(creds).map_err(|e| e.to_string())?;
    state
        .db
        .set_connection(user_id, tidal::SERVICE, &json, username)
        .await
        .map_err(|e| e.to_string())
}

/// This user's stored TIDAL credentials, if they have connected one.
pub async fn credentials(state: &AppState, user_id: i64) -> Option<TidalCredentials> {
    let rows = state.db.get_connections_for_forwarding(user_id).await.ok()?;
    rows.into_iter()
        .find(|(service, _)| service == tidal::SERVICE)
        .and_then(|(_, blob)| serde_json::from_str(&blob).ok())
}

pub fn create_tidal_router() -> Router<std::sync::Arc<AppState>> {
    Router::new()
        .route("/api/v1/tidal/status", get(status))
        .route("/api/v1/tidal/connect", post(connect))
        .route("/api/v1/tidal/callback", get(callback))
        .route("/api/v1/tidal/disconnect", axum::routing::delete(disconnect))
}
