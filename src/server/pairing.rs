//! Device-code pairing.
//!
//! Pasting a token works when the client has a keyboard and a browser on the
//! same screen. A DAP, a headless player or a TV app has neither, so this is
//! the other shape: the client asks for a code, shows it, and polls; the user
//! types that code into Tapedeck on a device that *does* have a browser and
//! approves it; the client collects its token.
//!
//! Deliberately close to RFC 8628's device authorization grant in vocabulary —
//! `device_code`, `user_code`, `interval`, `authorization_pending` — because
//! anyone implementing a client has met that shape before. It is **not** OAuth
//! and does not pretend to be: there is no client registration, no
//! `client_id`, and the result is a Tapedeck API token rather than an access
//! token with a refresh cycle.
//!
//! The security story is short and worth stating plainly, because the user code
//! is only eight characters and could be guessed:
//!
//! * the window is **five minutes**, and expired requests are purged;
//! * a code is **single use** — approving mints exactly one token, and the
//!   token is handed to the client once and then deleted from the request;
//! * **polling is rate limited** per client address, on the same limiter the
//!   login endpoint uses;
//! * approval requires a **logged-in session**, so guessing a code gets you
//!   nothing without an account.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};
use utoipa::ToSchema;

use super::auth::AuthUser;
use super::models::{ErrorResponse, OkResponse};
use super::AppState;
use crate::db::{Database, PairingPoll};
use crate::openapi::{NotFound, Unauthorized};

/// How often a client should poll. Matched to the limiter's tolerance — faster
/// than this and a client throttles itself out of its own pairing.
const POLL_INTERVAL_SECS: i64 = 5;

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

/// Where to send the user to approve, as something they can actually type.
///
/// This used to return the bare path `/settings#tokens`, which is useless in
/// the one situation the whole flow exists for: a device with no browser
/// showing an instruction on a TV or a DAP screen. "Go to /settings#tokens" is
/// not an address.
///
/// The origin has to come from the request, because the server genuinely does
/// not know its own public URL — it may be behind a tunnel or a reverse proxy
/// on a different host and scheme entirely. Forwarded headers are believed only
/// when `trust_proxy` says there is a proxy to believe, exactly as
/// `cookie_is_secure` does for the session cookie. Falling back to the `Host`
/// header is right too: that is the name the client used to reach us, which is
/// the name that will work for whoever reads the screen.
fn verification_uri(state: &AppState, headers: &HeaderMap) -> String {
    build_verification_uri(state.trust_proxy, headers)
}

fn build_verification_uri(trust_proxy: bool, headers: &HeaderMap) -> String {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    // A forwarded header may be a comma-separated chain; the first hop is the
    // client's.
    let first = |v: &str| v.split(',').next().unwrap_or("").trim().to_string();

    let scheme = trust_proxy
        .then(|| header("x-forwarded-proto").map(first))
        .flatten()
        .filter(|s| s == "http" || s == "https")
        .unwrap_or_else(|| "http".into());

    let host = trust_proxy
        .then(|| header("x-forwarded-host").map(first))
        .flatten()
        .or_else(|| header("host").map(str::to_string))
        .filter(|h| !h.is_empty());

    match host {
        Some(host) => format!("{scheme}://{host}/settings#tokens"),
        // No Host header at all (HTTP/1.0, or a client that omits it). A path
        // is still better than nothing, and it is what this always returned.
        None => "/settings#tokens".into(),
    }
}

/// The client's address, for rate limiting. Mirrors the login endpoint: the
/// forwarded header is only believed behind a proxy the operator has declared.
fn client_key(state: &AppState, headers: &HeaderMap) -> String {
    if state.trust_proxy {
        if let Some(fwd) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(first) = fwd.split(',').next() {
                return first.trim().to_string();
            }
        }
    }
    "pairing".to_string()
}

/// Both fields are optional, and so is the body.
#[derive(Deserialize, ToSchema)]
pub struct StartBody {
    /// What to call the token once it exists — "Pano on Android", "fooyin".
    /// Shown to the user when approving, so name it per device. At most 60
    /// characters; `name` is accepted too.
    // `client_name` is what the published document has always said and what
    // Tapeout sends; the handler used to read only `name`, so every paired
    // device arrived as "Paired device". Both spellings are accepted now.
    #[serde(default, alias = "name")]
    pub client_name: Option<String>,
    /// Requested grants, space- or comma-separated: `submit`, `read`,
    /// `write`. `submit` is always granted; anything else unknown is ignored.
    /// The approval screen shows the user what they are granting.
    #[serde(default)]
    #[schema(example = "submit read write")]
    pub scopes: Option<String>,
}

/// A pairing, begun.
#[derive(Serialize, ToSchema)]
pub struct PairingStarted {
    /// Poll `/1/pair/poll` with this. Keep it secret — it collects the token.
    pub device_code: String,
    /// The short code for the human to type into Settings → API Tokens, as
    /// `XXXX-XXXX`.
    pub user_code: String,
    /// Where the human goes to approve it, as far as the server can tell from
    /// the request. A bare path when it cannot.
    pub verification_uri: String,
    /// Seconds until the request lapses. Five minutes.
    pub expires_in: i64,
    /// Minimum seconds between polls.
    pub interval: i64,
}

/// The grants a pairing request asks for, as stored. `submit` always; `read`
/// and `write` only when asked for by name. Anything else is dropped rather
/// than stored, so a client cannot invent a scope by asking for it.
fn requested_scopes(asked: Option<&str>) -> String {
    let words: Vec<&str> = asked
        .unwrap_or("")
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    let mut granted = vec!["submit"];
    for scope in ["read", "write"] {
        if words.contains(&scope) {
            granted.push(scope);
        }
    }
    granted.join(" ")
}

// POST /1/pair/start — unauthenticated; anyone can ask, nobody gets anything
// without a human approving it.
/// Begin device-code pairing.
///
/// Unauthenticated by design — the client has no credential yet. Returns a
/// `device_code` to poll with and a short `user_code` for the human to type
/// into Settings.
///
/// Vocabulary echoes RFC 8628 because client authors have met that shape,
/// but this is **not OAuth**: no client registration, no `client_id`, and
/// the result is an ordinary Tapedeck token.
///
/// The user code is eight characters and therefore guessable, so its safety
/// is structural: a five-minute window, single use in both directions, rate
/// limiting, and approval requiring a signed-in session.
#[utoipa::path(
    post,
    path = "/1/pair/start",
    tag = "Pairing",
    security(),
    request_body(content = StartBody, description = "Optional."),
    responses(
        (status = 200, description = "Pairing started.", body = PairingStarted),
        (status = 429, description = "Rate limited.", body = ErrorResponse),
    )
)]
pub async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<StartBody>>,
) -> impl IntoResponse {
    let key = format!("pair-start:{}", client_key(&state, &headers));
    if let Some(retry) = state.login_limiter.blocked_for(&key) {
        return err(429, format!("Too many pairing attempts. Try again in {retry}s"));
    }

    let body = body.map(|Json(b)| b).unwrap_or(StartBody { client_name: None, scopes: None });
    let name = body
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Paired device")
        .chars()
        .take(60)
        .collect::<String>();
    // `write` is on offer since the approval screen learned to say so: a
    // client like Tapeout asks for `submit read write` in one approval, and
    // this used to drop the `write` without a word, so every love and upload
    // it made afterwards was a 403.
    let scopes = requested_scopes(body.scopes.as_deref());

    // Opportunistic cleanup — there is no scheduled job for this and it costs
    // one indexed delete.
    let _ = state.db.purge_expired_pairings().await;

    match state.db.start_pairing(&name, &scopes).await {
        Ok((device_code, user_code)) => {
            state.login_limiter.record_failure(&key);
            info!("🔗 Pairing requested for '{}'", name);
            (
                StatusCode::OK,
                Json(PairingStarted {
                    device_code,
                    user_code,
                    verification_uri: verification_uri(&state, &headers),
                    expires_in: Database::PAIRING_TTL_SECS,
                    interval: POLL_INTERVAL_SECS,
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!("Could not start pairing: {}", e);
            err(500, "Could not start pairing")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    /// The device shows this on a screen for someone to type. A bare path is
    /// not an address, which is what it used to return.
    /// `submit` always; `read` and `write` only by name; nothing invented.
    #[test]
    fn a_pairing_grants_what_it_asked_for_and_nothing_it_made_up() {
        assert_eq!(requested_scopes(None), "submit");
        assert_eq!(requested_scopes(Some("read")), "submit read");
        assert_eq!(requested_scopes(Some("submit read write")), "submit read write");
        assert_eq!(requested_scopes(Some("write,read")), "submit read write");
        assert_eq!(requested_scopes(Some("all admin rewrite")), "submit");
    }

    /// The published body field is `client_name`; `name` is what the handler
    /// used to read. Both have to land.
    #[test]
    fn the_client_name_is_read_under_either_spelling() {
        for body in [r#"{"client_name":"fooyin"}"#, r#"{"name":"fooyin"}"#] {
            let b: StartBody = serde_json::from_str(body).unwrap();
            assert_eq!(b.client_name.as_deref(), Some("fooyin"), "{body}");
        }
    }

    #[test]
    fn the_verification_uri_is_somewhere_you_can_actually_go() {
        let plain = hdrs(&[("host", "tapedeck.lan:8080")]);
        assert_eq!(
            build_verification_uri(false, &plain),
            "http://tapedeck.lan:8080/settings#tokens"
        );

        // Behind a trusted proxy the browser's scheme and host are the ones
        // that work, not the origin's — the same reasoning `cookie_is_secure`
        // applies to the session cookie.
        let proxied = hdrs(&[
            ("host", "127.0.0.1:8080"),
            ("x-forwarded-proto", "https,http"),
            ("x-forwarded-host", "music.example.com"),
        ]);
        assert_eq!(
            build_verification_uri(true, &proxied),
            "https://music.example.com/settings#tokens"
        );

        // Untrusted, the forwarded headers are a client-supplied lie and must
        // be ignored — otherwise anyone could make the device display a host
        // of their choosing.
        assert_eq!(
            build_verification_uri(false, &proxied),
            "http://127.0.0.1:8080/settings#tokens"
        );

        // No Host at all: a path is still better than nothing.
        assert_eq!(build_verification_uri(false, &hdrs(&[])), "/settings#tokens");
    }
}

#[derive(Deserialize, ToSchema)]
pub struct PollBody {
    pub device_code: String,
}

/// The token, collected.
#[derive(Serialize, ToSchema)]
pub struct PairingApproved {
    /// Always `approved`.
    pub status: &'static str,
    /// The new API token. **In this body and nowhere else** — it is deleted
    /// from the request the moment it is collected.
    pub token: String,
    /// Whose token it is — the account that approved the request.
    pub user_name: Option<String>,
}

/// Not collected, and why — RFC 8628's error vocabulary, in Tapedeck's error
/// shape.
#[derive(Serialize, ToSchema)]
pub struct PairingPending {
    /// Always 400.
    pub code: u16,
    /// `authorization_pending` — nobody has approved it yet, keep polling —
    /// or `expired_token`, which is also what an unknown device code gets.
    #[schema(pattern = "^(authorization_pending|expired_token)$")]
    pub error: &'static str,
    /// Human-readable.
    pub error_description: &'static str,
    /// Minimum seconds before the next poll. Only on `authorization_pending`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<i64>,
}

// POST /1/pair/poll — the client asks whether it has been approved yet.
/// Poll for approval.
///
/// `400 authorization_pending` until approved, then the token **once** — it is
/// deleted from the request the moment it is collected. Expired and unknown
/// device codes are both `400 expired_token`, so polling cannot be used to
/// learn whether a code was ever real.
#[utoipa::path(
    post,
    path = "/1/pair/poll",
    tag = "Pairing",
    security(),
    request_body = PollBody,
    responses(
        (status = 200, description = "Approved. The token is in this body and nowhere else.",
            body = PairingApproved),
        (status = 400, description = "`authorization_pending` (keep polling) or `expired_token` \
            (start again).", body = PairingPending),
        (status = 429, description = "Polling too fast. Back off and keep the pairing alive.",
            body = ErrorResponse),
    )
)]
pub async fn poll(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PollBody>,
) -> impl IntoResponse {
    let key = format!("pair-poll:{}", client_key(&state, &headers));
    if let Some(retry) = state.login_limiter.blocked_for(&key) {
        return err(429, format!("Polling too fast. Try again in {retry}s"));
    }

    match state.db.poll_pairing(body.device_code.trim()).await {
        Ok(PairingPoll::Approved(token)) => {
            // Succeeded, so stop counting this client's attempts against it.
            state.login_limiter.reset(&key);
            info!("🔗 Pairing approved and collected");
            // The name a client shows as "paired as …". Looked up from the
            // token rather than stored on the request: the token is the thing
            // that now says whose it is.
            let user_name =
                state.db.validate_token(&token).await.ok().flatten().map(|t| t.user.username);
            (StatusCode::OK, Json(PairingApproved { status: "approved", token, user_name }))
                .into_response()
        }
        // RFC 8628's shape: pending is a **400** carrying
        // `authorization_pending`, not a 200. The published document and the
        // clients built from it (Tapeout) have always expected that, and this
        // used to answer 200 `{status: "authorization_pending"}` — which such a
        // client reads as "approved, but sent no token" and gives up on the
        // first poll.
        Ok(PairingPoll::Pending) => (
            StatusCode::BAD_REQUEST,
            Json(PairingPending {
                code: 400,
                error: "authorization_pending",
                error_description: "Waiting for someone to approve this code in Tapedeck.",
                interval: Some(POLL_INTERVAL_SECS),
            }),
        )
            .into_response(),
        // Expired and unknown are reported identically, so polling cannot be
        // used to find out whether a device code was ever real.
        Ok(_) => {
            state.login_limiter.record_failure(&key);
            (
                StatusCode::BAD_REQUEST,
                Json(PairingPending {
                    code: 400,
                    error: "expired_token",
                    error_description: "That pairing request has expired. Start a new one.",
                    interval: None,
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!("Pairing poll failed: {}", e);
            err(500, "Could not check that pairing")
        }
    }
}

// GET /api/v1/pair/:user_code — what the user is about to approve.
/// Describe a pending pairing, for the approval screen.
///
/// Requires a signed-in session — this is the browser half.
#[utoipa::path(
    get,
    path = "/api/v1/pair/{user_code}",
    tag = "Pairing",
    params(("user_code" = String, Path, description = "As shown on the device, `XXXX-XXXX`.")),
    responses(
        (status = 200, description = "OK.", body = PairingRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
pub async fn describe(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Path(user_code): Path<String>,
) -> impl IntoResponse {
    match state.db.pairing_by_user_code(&user_code).await {
        Ok(Some((_, client_name, scopes, approved))) => (
            StatusCode::OK,
            Json(PairingRequest {
                can_read: scopes.split_whitespace().any(|w| w == "read"),
                can_write: scopes.split_whitespace().any(|w| w == "write"),
                client_name,
                scopes,
                approved,
            }),
        )
            .into_response(),
        Ok(None) => err(404, "No pending request for that code"),
        Err(e) => {
            warn!("Pairing lookup failed: {}", e);
            err(500, "Could not look that up")
        }
    }
}

/// A pending pairing, for the approval screen.
#[derive(Serialize, ToSchema)]
pub struct PairingRequest {
    /// What the client called itself.
    pub client_name: String,
    /// What approving grants, space-separated.
    pub scopes: String,
    /// Whether approving grants `read`.
    pub can_read: bool,
    /// Whether approving grants `write`.
    pub can_write: bool,
    pub approved: bool,
}

/// The body is optional.
#[derive(Deserialize, ToSchema)]
pub struct ApproveBody {
    /// The chain listens from this client should be attributed to — rung 3 of
    /// the ladder, and the reason one token per app is worth the trouble.
    /// Must be one of your chains.
    #[serde(default)]
    pub default_chain_id: Option<i64>,
}

// POST /api/v1/pair/:user_code/approve — mint the token.
//
// The token goes to the *polling client*, never into this response: the
// browser approving a pairing has no use for it, and putting a live credential
// in a second place is how it ends up in a screenshot.
/// Approve a pairing request.
///
/// Session-only, and single use — approving twice mints nothing the second
/// time. **The token is never in this response**; it goes to the polling
/// client. A live credential in a second place is how it ends up in a
/// screenshot.
#[utoipa::path(
    post,
    path = "/api/v1/pair/{user_code}/approve",
    tag = "Pairing",
    params(("user_code" = String, Path, description = "As shown on the device, `XXXX-XXXX`.")),
    request_body(content = ApproveBody, description = "Optional."),
    responses(
        (status = 200, description = "Approved. `status` is `approved`.", body = OkResponse),
        (status = 400, description = "Not one of your chains.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
pub async fn approve(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(user_code): Path<String>,
    body: Option<Json<ApproveBody>>,
) -> impl IntoResponse {
    let chain = body.and_then(|Json(b)| b.default_chain_id);

    // A chain named in the request has to be the approving user's own.
    if let Some(cid) = chain {
        match state.db.get_chain(user.user_id, cid).await {
            Ok(Some(_)) => {}
            Ok(None) => return err(400, "No such signal chain"),
            Err(e) => {
                warn!("Chain check failed: {}", e);
                return err(500, "Could not check that chain");
            }
        }
    }

    match state.db.approve_pairing(&user_code, user.user_id, chain).await {
        Ok(true) => {
            info!("🔗 Pairing approved by {}", user.username);
            (StatusCode::OK, OkResponse::new("approved")).into_response()
        }
        Ok(false) => err(404, "That code is unknown, expired, or already approved"),
        Err(e) => {
            warn!("Pairing approval failed: {}", e);
            err(500, "Could not approve that pairing")
        }
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(start, poll, describe, approve))]
pub struct Api;

pub fn create_pairing_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/1/pair/start", post(start))
        .route("/1/pair/poll", post(poll))
        .route("/api/v1/pair/:user_code", get(describe))
        .route("/api/v1/pair/:user_code/approve", post(approve))
}
