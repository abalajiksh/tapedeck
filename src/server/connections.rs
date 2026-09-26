//! Per-user scrobble connections: each user connects their own ListenBrainz,
//! Last.fm and Libre.fm accounts. Credentials live server-side; responses only
//! ever reveal the connected account name, never the token/session key.
//!
//! There is no server-wide equivalent — a listen is only ever forwarded to the
//! accounts its own user connected here.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use utoipa::ToSchema;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::openapi::{BadRequest, Unauthorized};

/// Your forwarding connections. Never carries a token or session key.
#[derive(Serialize, ToSchema)]
pub struct ConnectionList {
    pub connections: Vec<Connection>,
    pub lastfm_available: bool,
}

#[derive(Serialize, ToSchema)]
pub struct Connection {
    /// `listenbrainz`, `lastfm` or `librefm`.
    pub service: String,
    /// The account on that service, where it said.
    pub username: Option<String>,
    pub connected_at: i64,
}

/// Whether plays accepted out of a Shared Spool are forwarded.
#[derive(Serialize, ToSchema)]
pub struct SpoolForwarding {
    pub enabled: bool,
    /// Only plays this recent are relayed, whatever the switch says. Absent
    /// from the answer to a change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_days: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct Connected {
    pub connected: bool,
    pub username: Option<String>,
}

/// Where to send the browser to approve Tapedeck.
#[derive(Serialize, ToSchema)]
pub struct ScrobblerAuth {
    /// The request token — send it back to `…/complete` once approved.
    pub token: String,
    pub auth_url: String,
}

/// The switch deciding whether plays accepted out of a Shared Spool are relayed
/// on to this user's connections, or stored here only.
///
/// It lives beside the connections rather than with the rest of Patch because
/// this is the question "what do my connected services receive", and the answer
/// is only meaningful next to the list of them.
#[derive(Deserialize, ToSchema)]
struct SpoolForwardingRequest {
    /// Required, and said explicitly — an absent key is a 400, not a default.
    //
    // **Not `Option<bool>` with a default**, and the `Option` here is only so
    // an absent key can be told from an explicit `false`. A body that merely
    // misspelled the key must not be read as an answer either way — the
    // `bulk-chain` rule, in another place where guessing wrong writes to
    // somebody's permanent public record.
    #[schema(required, value_type = bool)]
    enabled: Option<bool>,
}

// GET /api/v1/connections/spool-forwarding — the switch, and the window that
// bounds it.
//
// `window_days` is reported rather than left for the screen to hardcode: it is
// a rule about what will actually be relayed, and a second copy of it in the
// UI would be free to drift from the one enforced.
/// Whether accepted Shared Spool plays are forwarded onward.
///
/// `enabled` is the switch; `window_days` is the freshness bound that
/// applies even when it is on, reported rather than hardcoded in a client
/// so the two cannot disagree about what will actually be relayed.
#[utoipa::path(
    get,
    path = "/api/v1/connections/spool-forwarding",
    tag = "Connections",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "The switch and its window.", body = SpoolForwarding),
        (status = 401, response = Unauthorized),
    )
)]
async fn get_spool_forwarding(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.db.forward_spool_listens(user.user_id).await {
        Ok(on) => (
            StatusCode::OK,
            Json(SpoolForwarding {
                enabled: on,
                window_days: Some(crate::db::SPOOL_FORWARD_WINDOW_SECS / 86_400),
            }),
        )
            .into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// PUT /api/v1/connections/spool-forwarding — set it.
//
// **Session-only, and it takes no token**, the line every switch that decides
// an outward-facing write sits on: a scrobble client has no business deciding
// that somebody else's deck may write to its owner's public history.
/// Set it.
///
/// **Off by default, and off for every account that predates it.** Turning
/// it on means a play somebody else's deck made — which you were in the
/// room for, and accepted one at a time — is relayed to your Last.fm and
/// ListenBrainz like any listen of your own.
///
/// A play older than `window_days` is stored and never relayed even with
/// the switch on. `retro_spool_from_session` can offer an evening of any
/// age and an unanswered invitation can be joined a month later, so
/// without that bound a single Accept could rewrite a year of a public
/// history.
///
/// **Session-only; a token gets 401 and scopes never enter into it.** A
/// scrobble client has no business deciding that another deck may write to
/// its owner's public record.
#[utoipa::path(
    put,
    path = "/api/v1/connections/spool-forwarding",
    tag = "Connections",
    security(("sessionCookie" = [])),
    request_body = SpoolForwardingRequest,
    responses(
        (status = 200, description = "The switch as it now stands.", body = SpoolForwarding),
        (status = 400, description = "`enabled` was absent.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn put_spool_forwarding(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SpoolForwardingRequest>,
) -> impl IntoResponse {
    let Some(enabled) = body.enabled else {
        return err(400, "`enabled` is required — say true or false explicitly.");
    };
    match state.db.set_forward_spool_listens(user.user_id, enabled).await {
        Ok(()) => {
            (StatusCode::OK, Json(SpoolForwarding { enabled, window_days: None })).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let code_status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code_status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

// GET /api/v1/connections — which services this user has connected.
/// Where your listens are forwarded.
///
/// **Session-only, permanently** — holds credentials.
///
/// Forwarding is strictly per user. A user with no connections keeps their
/// listens local; there is deliberately **no server-wide fallback**, which
/// previously sent every unconnected user's listens to the operator's own
/// account.
#[utoipa::path(
    get,
    path = "/api/v1/connections",
    tag = "Connections",
    responses(
        (status = 200, description = "OK.", body = ConnectionList),
        (status = 401, response = Unauthorized),
    )
)]
async fn list_connections(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.list_connections(user.user_id).await {
        Ok(rows) => {
            let connections = rows
                .into_iter()
                .map(|(service, username, connected_at)| Connection {
                    service,
                    username,
                    connected_at,
                })
                .collect();
            // Kept for the UI's shape; Tapedeck ships its own Last.fm app
            // credentials, so connecting is always possible.
            (StatusCode::OK, Json(ConnectionList { connections, lastfm_available: true }))
                .into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// Disconnect a service.
///
/// Session-only.
#[utoipa::path(
    delete,
    path = "/api/v1/connections/{service}",
    tag = "Connections",
    params(("service" = String, Path, description = "`lastfm`, `librefm` or `listenbrainz`.")),
    responses(
        (status = 204, description = "Disconnected, or was never connected."),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn disconnect(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(service): Path<String>,
) -> impl IntoResponse {
    if !matches!(service.as_str(), "lastfm" | "listenbrainz" | "librefm") {
        return err(400, "Unknown service");
    }
    match state.db.delete_connection(user.user_id, &service).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// ── ListenBrainz: paste a user token ──

#[derive(Deserialize, ToSchema)]
struct LbConnect {
    /// Your ListenBrainz user token. Checked against ListenBrainz before it is
    /// stored.
    token: String,
}

/// Store a ListenBrainz token.
///
/// Session-only. Stored encrypted. The endpoint is configurable for
/// self-hosted ListenBrainz instances — that is the one scrobble setting
/// still in the config file rather than the database.
#[utoipa::path(
    put,
    path = "/api/v1/connections/listenbrainz",
    tag = "Connections",
    request_body = LbConnect,
    responses(
        (status = 200, description = "Connected.", body = Connected),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 502, description = "ListenBrainz could not be reached.", body = ErrorResponse),
    )
)]
async fn connect_listenbrainz(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<LbConnect>,
) -> impl IntoResponse {
    let token = body.token.trim().to_string();
    if token.is_empty() {
        return err(400, "Token required");
    }
    // Validate against ListenBrainz and capture the account name.
    let url = format!("{}/1/validate-token", state.listenbrainz_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Token {token}"))
        .send()
        .await;
    let username = match resp {
        Ok(r) if r.status().is_success() => {
            let v: serde_json::Value = r.json().await.unwrap_or_default();
            if v["valid"].as_bool() != Some(true) {
                return err(400, "ListenBrainz rejected that token");
            }
            v["user_name"].as_str().map(str::to_string)
        }
        Ok(r) => return err(400, format!("ListenBrainz returned {}", r.status())),
        Err(e) => return err(502, format!("Could not reach ListenBrainz: {e}")),
    };

    let creds = serde_json::json!({ "token": token }).to_string();
    match state.db.set_connection(user.user_id, "listenbrainz", &creds, username.as_deref()).await {
        Ok(()) => (StatusCode::OK, Json(Connected { connected: true, username })).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// ── Last.fm: token → approve → session (button OAuth) ──

/// Last.fm signs requests with md5 of the sorted params + shared secret.
fn lastfm_sign(params: &BTreeMap<&str, &str>, secret: &str) -> String {
    let mut s = String::new();
    for (k, v) in params {
        s.push_str(k);
        s.push_str(v);
    }
    s.push_str(secret);
    format!("{:x}", md5::compute(s))
}

/// Libre.fm runs GNU FM, which implements the same AudioScrobbler 2.0 auth
/// methods as Last.fm — so both services share one flow, differing only in
/// which endpoints they talk to.
struct Scrobbler {
    label: &'static str,
    api: &'static str,
    auth_page: &'static str,
}

const LASTFM: Scrobbler = Scrobbler {
    label: "Last.fm",
    api: "https://ws.audioscrobbler.com/2.0/",
    auth_page: "https://www.last.fm/api/auth/",
};
const LIBREFM: Scrobbler = Scrobbler {
    label: "Libre.fm",
    api: "https://libre.fm/2.0/",
    auth_page: "https://libre.fm/api/auth/",
};

/// Step 1: get a request token and the URL the user approves it at.
async fn scrobbler_start(svc: &Scrobbler) -> axum::response::Response {
    let mut params = BTreeMap::new();
    params.insert("api_key", crate::sinks::LASTFM_API_KEY);
    params.insert("method", "auth.getToken");
    let sig = lastfm_sign(&params, crate::sinks::LASTFM_SECRET);

    let url = format!(
        "{}?method=auth.getToken&api_key={}&api_sig={}&format=json",
        svc.api,
        crate::sinks::LASTFM_API_KEY,
        sig,
    );
    let resp = reqwest::Client::new().get(&url).send().await;
    let token = match resp {
        Ok(r) => {
            let v: serde_json::Value = r.json().await.unwrap_or_default();
            match v["token"].as_str() {
                Some(t) => t.to_string(),
                None => {
                    return err(
                        502,
                        format!(
                            "{} getToken failed: {}",
                            svc.label,
                            v["message"].as_str().unwrap_or("unknown")
                        ),
                    )
                }
            }
        }
        Err(e) => return err(502, format!("Could not reach {}: {e}", svc.label)),
    };

    let auth_url =
        format!("{}?api_key={}&token={}", svc.auth_page, crate::sinks::LASTFM_API_KEY, token);
    (StatusCode::OK, Json(ScrobblerAuth { token, auth_url })).into_response()
}

/// Step 2: exchange the approved token for a long-lived session key.
async fn scrobbler_complete(
    state: &Arc<AppState>,
    user: &AuthUser,
    svc: &Scrobbler,
    service_key: &str,
    token: &str,
) -> axum::response::Response {
    if token.is_empty() {
        return err(400, "Missing token");
    }

    let mut params = BTreeMap::new();
    params.insert("api_key", crate::sinks::LASTFM_API_KEY);
    params.insert("method", "auth.getSession");
    params.insert("token", token);
    let sig = lastfm_sign(&params, crate::sinks::LASTFM_SECRET);

    let url = format!(
        "{}?method=auth.getSession&api_key={}&token={}&api_sig={}&format=json",
        svc.api,
        crate::sinks::LASTFM_API_KEY,
        token,
        sig,
    );
    let resp = reqwest::Client::new().get(&url).send().await;
    let (session_key, username) = match resp {
        Ok(r) => {
            let v: serde_json::Value = r.json().await.unwrap_or_default();
            match v["session"]["key"].as_str() {
                Some(k) => (k.to_string(), v["session"]["name"].as_str().map(str::to_string)),
                None => {
                    // error 14/15 = token not authorized yet
                    return err(
                        400,
                        format!(
                            "Not yet approved on {} — click Connect again after approving ({})",
                            svc.label,
                            v["message"].as_str().unwrap_or("no session")
                        ),
                    );
                }
            }
        }
        Err(e) => return err(502, format!("Could not reach {}: {e}", svc.label)),
    };

    let creds = serde_json::json!({ "session_key": session_key }).to_string();
    match state.db.set_connection(user.user_id, service_key, &creds, username.as_deref()).await {
        Ok(()) => (StatusCode::OK, Json(Connected { connected: true, username })).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// POST /api/v1/connections/lastfm/start — get a request token + the approval URL.
/// Begin the Last.fm connect flow.
///
/// Session-only. Returns a URL to send the browser to.
#[utoipa::path(
    post,
    path = "/api/v1/connections/lastfm/start",
    tag = "Connections",
    responses(
        (status = 200, description = "OK.", body = ScrobblerAuth),
        (status = 401, response = Unauthorized),
        (status = 502, description = "The service could not be reached, or refused a token.", body = ErrorResponse),
    )
)]
async fn lastfm_start(_user: AuthUser) -> impl IntoResponse {
    scrobbler_start(&LASTFM).await
}

/// Begin the Libre.fm connect flow.
///
/// Session-only. GNU FM implements the same AudioScrobbler auth, so this reuses
/// the Last.fm app credentials.
#[utoipa::path(
    post,
    path = "/api/v1/connections/librefm/start",
    tag = "Connections",
    responses(
        (status = 200, description = "OK.", body = ScrobblerAuth),
        (status = 401, response = Unauthorized),
        (status = 502, description = "The service could not be reached, or refused a token.", body = ErrorResponse),
    )
)]
async fn librefm_start(_user: AuthUser) -> impl IntoResponse {
    scrobbler_start(&LIBREFM).await
}

#[derive(Deserialize, ToSchema)]
#[schema(as = ScrobblerApproved)]
struct LastfmComplete {
    /// The token `…/start` returned, after you approved it.
    token: String,
}

// POST /api/v1/connections/lastfm/complete — exchange the approved token for a
// session key. Fails if the user hasn't approved on Last.fm's site yet.
/// Finish the Last.fm connect flow.
///
/// Session-only. Exchanges the approved request token for a session key, stored
/// encrypted.
#[utoipa::path(
    post,
    path = "/api/v1/connections/lastfm/complete",
    tag = "Connections",
    request_body = LastfmComplete,
    responses(
        (status = 200, description = "Connected.", body = Connected),
        (status = 400, description = "Not approved yet, or no token.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 502, description = "The service could not be reached.", body = ErrorResponse),
    )
)]
async fn lastfm_complete(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<LastfmComplete>,
) -> impl IntoResponse {
    scrobbler_complete(&state, &user, &LASTFM, "lastfm", body.token.trim()).await
}

/// Finish the Libre.fm connect flow.
///
/// Session-only.
#[utoipa::path(
    post,
    path = "/api/v1/connections/librefm/complete",
    tag = "Connections",
    request_body = LastfmComplete,
    responses(
        (status = 200, description = "Connected.", body = Connected),
        (status = 400, description = "Not approved yet, or no token.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 502, description = "The service could not be reached.", body = ErrorResponse),
    )
)]
async fn librefm_complete(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<LastfmComplete>,
) -> impl IntoResponse {
    scrobbler_complete(&state, &user, &LIBREFM, "librefm", body.token.trim()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lastfm_sign_is_deterministic_and_secret_sensitive() {
        let mut p = BTreeMap::new();
        p.insert("api_key", "KEY");
        p.insert("method", "auth.getToken");
        // md5("api_keyKEYmethodauth.getTokenSECRET")
        let sig = lastfm_sign(&p, "SECRET");
        assert_eq!(sig.len(), 32);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(sig, lastfm_sign(&p, "SECRET"), "same input → same signature");
        assert_ne!(sig, lastfm_sign(&p, "OTHER"), "signature depends on the secret");
    }

    #[test]
    fn lastfm_sign_sorts_params() {
        // BTreeMap sorts keys, so insertion order doesn't change the signature.
        let mut a = BTreeMap::new();
        a.insert("b", "2");
        a.insert("a", "1");
        let mut b = BTreeMap::new();
        b.insert("a", "1");
        b.insert("b", "2");
        assert_eq!(lastfm_sign(&a, "s"), lastfm_sign(&b, "s"));
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_connections,
    get_spool_forwarding,
    put_spool_forwarding,
    disconnect,
    connect_listenbrainz,
    lastfm_start,
    lastfm_complete,
    librefm_start,
    librefm_complete
))]
pub struct Api;

pub fn create_connections_router() -> Router<Arc<AppState>> {
    use axum::routing::{delete, post, put};
    Router::new()
        .route("/api/v1/connections", get(list_connections))
        // Before `/:service`, or matchit resolves this against the delete
        // route's parameter. A literal wins over a parameter in axum 0.7, but
        // reading it in this order is what makes that obvious.
        .route(
            "/api/v1/connections/spool-forwarding",
            get(get_spool_forwarding).put(put_spool_forwarding),
        )
        .route("/api/v1/connections/:service", delete(disconnect))
        .route("/api/v1/connections/listenbrainz", put(connect_listenbrainz))
        .route("/api/v1/connections/lastfm/start", post(lastfm_start))
        .route("/api/v1/connections/lastfm/complete", post(lastfm_complete))
        .route("/api/v1/connections/librefm/start", post(librefm_start))
        .route("/api/v1/connections/librefm/complete", post(librefm_complete))
}
