//! The MCP endpoint and the screens that manage it.
//!
//! `POST /mcp` is the whole protocol surface. Everything under
//! `/api/v1/mcp/…` is the user's own view of it: which connections exist, what
//! each was granted, what they have been doing, and how to revoke one.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tracing::{info, warn};

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::mcp::{dispatch, protocol, scopes};

/// Calls one connection may make per hour.
///
/// Not a security boundary — the credential is already the user's own — but a
/// blast radius. An agent in a retry loop on a Raspberry Pi is the realistic
/// failure, and 600 is far above anything a conversation does and far below
/// anything that hurts.
const CALLS_PER_HOUR: i64 = 600;

fn err(code: u16, msg: impl Into<String>) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

// ════════════════════════════════════════════════════════════
//  The MCP endpoint
// ════════════════════════════════════════════════════════════

/// Where a client discovers how to authorise itself.
///
/// The URL has to be built from the request for the reason `pairing`'s
/// verification URI does: behind a tunnel or a reverse proxy the server does
/// not know its own public address, and the name the client used to reach us is
/// the name that will work for it next time.
pub fn resource_metadata_url(state: &AppState, headers: &HeaderMap) -> String {
    format!("{}/.well-known/oauth-protected-resource", public_origin(state, headers))
}

pub fn public_origin(state: &AppState, headers: &HeaderMap) -> String {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let first = |v: &str| v.split(',').next().unwrap_or("").trim().to_string();

    let scheme = state
        .trust_proxy
        .then(|| get("x-forwarded-proto").map(first))
        .flatten()
        .filter(|s| s == "http" || s == "https")
        .unwrap_or_else(|| "http".into());
    let host = state
        .trust_proxy
        .then(|| get("x-forwarded-host").map(first))
        .flatten()
        .or_else(|| get("host").map(str::to_string))
        .unwrap_or_else(|| "localhost".into());
    format!("{scheme}://{host}")
}

/// `WWW-Authenticate`, which is how a client learns where to go and what to ask
/// for. Without it a 401 is a dead end and the OAuth flow can never start.
fn challenge(state: &AppState, headers: &HeaderMap, extra: Option<(&str, &str)>) -> String {
    let meta = resource_metadata_url(state, headers);
    let mut parts = vec![format!("resource_metadata=\"{meta}\"")];
    match extra {
        Some((error, scope)) => {
            parts.push(format!("error=\"{error}\""));
            parts.push(format!("scope=\"{scope}\""));
        }
        None => {
            // The minimum useful grant, per the spec's scope-selection
            // guidance: ask for the least, step up later.
            parts.push(format!("scope=\"{}\"", scopes::DEFAULT));
        }
    }
    format!("Bearer {}", parts.join(", "))
}

fn unauthorized(state: &AppState, headers: &HeaderMap, msg: &str) -> Response {
    let mut resp = err(401, msg);
    if let Ok(v) = challenge(state, headers, None).parse() {
        resp.headers_mut().insert("www-authenticate", v);
    }
    resp
}

/// The DNS-rebinding guard the transport spec requires — narrowed to the case
/// it actually protects.
///
/// The attack is specific: a page at `evil.example` resolves a hostname it
/// controls to a **private address**, so the victim's own browser reaches an MCP
/// server on their LAN that the attacker cannot route to. A cross-origin
/// `Origin` header is what gives that away.
///
/// The first version of this refused *any* cross-origin request, and that was
/// wrong in a way that cost a working connector: a hosted assistant whose HTTP
/// stack sets `Origin: https://claude.ai` was refused `403` before it ever
/// authenticated — which reads to the user as "this connector has no tools",
/// because the refusal lands on `tools/list` rather than on the sign-in.
///
/// Two things make the narrower rule safe. **Every request here needs a bearer
/// credential**, and a page in someone's browser cannot obtain one — it is not
/// in a cookie, so the browser never attaches it. And **no CORS headers are ever
/// sent**, so a cross-origin page could not read a response even if one came
/// back. The credential is the real defence; this is belt-and-braces over the
/// one shape where a request could reach a host the attacker otherwise cannot.
///
/// So: same origin is fine, a `null` origin is refused, and a cross-origin
/// request is refused **only when it is addressed to a private or loopback
/// host** — which is DNS rebinding and nothing else.
fn origin_is_acceptable(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        // No Origin at all: every non-browser MCP client, i.e. almost all.
        return true;
    };
    // A sandboxed or opaque context. Nothing legitimate here sends this.
    if origin == "null" {
        return false;
    }

    let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
    let origin_host = origin.split("://").nth(1).unwrap_or("");
    if origin_host.eq_ignore_ascii_case(host) {
        return true;
    }

    // Cross-origin. Only a private target has the rebinding shape.
    !is_private_host(host)
}

/// Whether a `Host` names something only reachable from inside the network.
///
/// Deliberately a string test rather than a parsed address: `Host` is a name as
/// often as an address, and `.local` or a bare hostname is exactly what a
/// rebinding attack would target. Anything not recognisably private is treated
/// as public, which is the safe direction — a public host is one the attacker
/// could reach directly anyway, so rebinding buys them nothing there.
fn is_private_host(host: &str) -> bool {
    let host = host.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return true;
    }
    if host == "localhost"
        || host == "::1"
        || host.ends_with(".local")
        || host.ends_with(".localhost")
    {
        return true;
    }
    if host.starts_with("127.") || host.starts_with("10.") || host.starts_with("192.168.") {
        return true;
    }
    // 172.16.0.0/12 — the range that gets written wrong, so it is spelled out.
    if let Some(rest) = host.strip_prefix("172.") {
        if let Some(second) = rest.split('.').next().and_then(|o| o.parse::<u8>().ok()) {
            if (16..=31).contains(&second) {
                return true;
            }
        }
    }
    // A bare hostname with no dot in it is a LAN name.
    !host.contains('.')
}

/// `POST /mcp`
pub async fn endpoint(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !origin_is_acceptable(&headers) {
        warn!("🤖 MCP request refused: cross-origin request from a browser context");
        return err(403, "Origin not allowed");
    }

    // Authenticate before parsing. A body from an unauthenticated caller is not
    // worth deserialising, and answering a protocol error first would let
    // anyone probe which revisions this server speaks.
    let Some(token) = bearer(&headers) else {
        return unauthorized(&state, &headers, "Authorization required");
    };
    let auth = match state.db.validate_mcp_token(&token).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return unauthorized(&state, &headers, "That credential is not valid or has expired")
        }
        Err(e) => {
            tracing::error!("MCP token validation failed: {e}");
            return err(500, "Internal server error");
        }
    };

    let since = crate::db::Database::now_secs() - 3600;
    if state.db.mcp_calls_since(auth.connection_id, since).await >= CALLS_PER_HOUR {
        warn!("🤖 MCP connection {} hit its hourly call budget", auth.connection_id);
        return err(429, format!("This connection is limited to {CALLS_PER_HOUR} calls an hour"));
    }

    let started = std::time::Instant::now();
    let lookup = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);

    let req = match protocol::parse(&body, lookup) {
        Ok(r) => r,
        Err(e) => {
            state
                .db
                .log_mcp_call(
                    Some(auth.connection_id),
                    auth.user.user_id,
                    "<malformed>",
                    None,
                    None,
                    false,
                    None,
                    started.elapsed().as_millis() as i64,
                )
                .await;
            return rpc_error(e, None);
        }
    };

    let (outcome, trace) = dispatch::handle(&state, &auth, &req).await;

    state
        .db
        .log_mcp_call(
            Some(auth.connection_id),
            auth.user.user_id,
            &req.method,
            trace.tool.as_deref(),
            trace.args.as_deref(),
            trace.ok,
            trace.rows,
            started.elapsed().as_millis() as i64,
        )
        .await;

    match outcome {
        dispatch::Outcome::Accepted => StatusCode::ACCEPTED.into_response(),
        dispatch::Outcome::Reply(v) => (StatusCode::OK, Json(v)).into_response(),
        dispatch::Outcome::Failed(e) => {
            let mut resp = rpc_error(e, req.id.clone());
            // A scope failure carries the grant it needs, so the client can
            // step up rather than only reporting a refusal.
            if resp.status() == StatusCode::FORBIDDEN {
                if let Some(scope) = missing_scope(&req) {
                    if let Ok(v) =
                        challenge(&state, &headers, Some(("insufficient_scope", scope))).parse()
                    {
                        resp.headers_mut().insert("www-authenticate", v);
                    }
                }
            }
            resp
        }
    }
}

/// Which grant a denied call needed. Read back off the tool catalogue rather
/// than threaded through the dispatcher — there is one list of tools and it
/// already knows.
fn missing_scope(req: &protocol::RpcRequest) -> Option<&'static str> {
    let name = req.param_str("name")?;
    crate::mcp::tools::catalogue().into_iter().find(|t| t.name == name).map(|t| t.scope)
}

fn rpc_error(e: protocol::RpcError, id: Option<serde_json::Value>) -> Response {
    let status = StatusCode::from_u16(e.http).unwrap_or(StatusCode::BAD_REQUEST);
    (status, Json(e.body(id))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("authorization")?.to_str().ok()?;
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        // Accepted as a convenience for anyone who has already met Tapedeck's
        // own header spelling. The credential is checked against
        // `mcp_connections` either way, so this widens what may be *typed*, not
        // what may be used.
        .or_else(|| raw.strip_prefix("Token "))?
        .trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// GET and DELETE on the MCP endpoint.
///
/// Both belonged to the 2025 revisions — a standalone SSE stream and session
/// termination — and neither is part of this one. `405` is what the spec tells
/// a modern-only transport to answer, and it is also what a dual-era client
/// reads as "no legacy stream here, use POST".
pub async fn method_not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {
            "code": protocol::METHOD_NOT_FOUND,
            "message": "This MCP endpoint accepts POST only; there is no GET stream and no session to delete",
        }
    }))).into_response()
}

// ════════════════════════════════════════════════════════════
//  Managing connections — session-only, always
// ════════════════════════════════════════════════════════════
//
// These are on `AuthUser`, which is session-only by construction. A connection
// that could mint another connection would make revocation meaningless, and one
// that could read the audit log could check whether anyone was watching it.

/// GET /api/v1/mcp/connections
async fn list_connections(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    user: AuthUser,
) -> Response {
    match state.db.list_mcp_connections(user.user_id).await {
        Ok(rows) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "connections": rows,
                // The address to paste into a client, worked out the same way
                // the pairing screen works out its verification URI.
                "endpoint": format!("{}/mcp", public_origin(&state, &headers)),
                "scopes": scopes::ALL.iter()
                    .map(|(s, d)| serde_json::json!({ "scope": s, "description": d }))
                    .collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize)]
struct NewConnection {
    name: Option<String>,
    /// Space- or comma-separated grants. Anything unrecognised is dropped
    /// rather than stored.
    scopes: Option<String>,
}

/// POST /api/v1/mcp/connections — mint one by hand, for a local client.
async fn create_connection(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<NewConnection>,
) -> Response {
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("AI assistant")
        .chars()
        .take(60)
        .collect::<String>();
    let granted = scopes::sanitise(body.scopes.as_deref().unwrap_or(scopes::DEFAULT));
    if granted.is_empty() {
        return err(400, "A connection needs at least one grant");
    }

    // `ttl_secs: None` — a credential the user pastes into a client themselves
    // does not expire. There is nothing for it to refresh against, and expiring
    // it would break their setup on a random Tuesday for no gain they asked
    // for. An OAuth grant is the one that expires, because a client can renew
    // it without anybody being present.
    match state.db.create_mcp_connection(user.user_id, &name, &granted, None, None).await {
        Ok(cred) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": cred.id,
                "name": name,
                "scopes": granted,
                // Shown once and never again — it is stored only as a hash.
                "token": cred.token,
            })),
        )
            .into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

/// DELETE /api/v1/mcp/connections/:id
async fn revoke_connection(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> Response {
    match state.db.revoke_mcp_connection(user.user_id, id).await {
        Ok(true) => {
            info!("🤖 MCP connection {} revoked by {}", id, user.username);
            (StatusCode::OK, Json(serde_json::json!({ "revoked": true }))).into_response()
        }
        Ok(false) => err(404, "No such connection"),
        Err(e) => err(500, e.to_string()),
    }
}

/// GET /api/v1/mcp/audit — what the connections have actually been doing.
async fn audit(State(state): State<Arc<AppState>>, user: AuthUser) -> Response {
    match state.db.list_mcp_audit(user.user_id, 200).await {
        Ok(rows) => (StatusCode::OK, Json(serde_json::json!({ "calls": rows }))).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

pub fn create_mcp_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/mcp", post(endpoint).get(method_not_allowed).delete(method_not_allowed))
        .route("/api/v1/mcp/connections", get(list_connections).post(create_connection))
        .route("/api/v1/mcp/connections/:id", axum::routing::delete(revoke_connection))
        .route("/api/v1/mcp/audit", get(audit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), v.parse().unwrap());
        }
        h
    }

    #[test]
    fn a_client_with_no_origin_header_is_allowed() {
        // Which is every non-browser MCP client, i.e. essentially all of them.
        assert!(origin_is_acceptable(&headers(&[("host", "tapedeck.local:8080")])));
    }

    #[test]
    fn a_page_on_another_site_cannot_reach_a_lan_instance() {
        // The DNS-rebinding attack the transport spec requires guarding
        // against: a page at evil.example resolving a name to a private host.
        for host in ["tapedeck.local:8080", "192.168.1.40:8080", "127.0.0.1:8080", "pi:8080"] {
            assert!(
                !origin_is_acceptable(&headers(&[
                    ("host", host),
                    ("origin", "https://evil.example")
                ])),
                "cross-origin request to private host {host} should be refused"
            );
        }
        // A sandboxed context sends a literal null and is never legitimate.
        assert!(!origin_is_acceptable(&headers(&[
            ("host", "tapedeck.local:8080"),
            ("origin", "null"),
        ])));
    }

    #[test]
    fn a_hosted_assistant_reaching_a_public_instance_is_allowed() {
        // What the first version got wrong, and it cost a working connector: a
        // hosted client whose HTTP stack sets an Origin was refused 403 before
        // it ever authenticated, which reads as "this connector has no tools".
        // Rebinding cannot reach a public host that the caller could simply
        // fetch directly, and every request needs a bearer credential a browser
        // page cannot hold.
        assert!(origin_is_acceptable(&headers(&[
            ("host", "tapedeck.example.com"),
            ("origin", "https://claude.ai"),
        ])));
    }

    #[test]
    fn the_private_ranges_are_the_ones_people_write_wrong() {
        assert!(is_private_host("172.16.0.5"));
        assert!(is_private_host("172.31.255.1"));
        // Just outside 172.16.0.0/12 — public, and a naive `starts_with("172.")`
        // would have called both of these private.
        assert!(!is_private_host("172.15.0.1"));
        assert!(!is_private_host("172.32.0.1"));
        assert!(is_private_host("[::1]:8080"));
        assert!(!is_private_host("tapedeck.example.com"));
    }

    #[test]
    fn the_apps_own_page_is_still_allowed() {
        assert!(origin_is_acceptable(&headers(&[
            ("host", "tapedeck.local:8080"),
            ("origin", "http://tapedeck.local:8080"),
        ])));
    }

    #[test]
    fn a_bearer_token_is_read_in_the_spellings_clients_actually_send() {
        assert_eq!(bearer(&headers(&[("authorization", "Bearer abc")])), Some("abc".into()));
        assert_eq!(bearer(&headers(&[("authorization", "Token abc")])), Some("abc".into()));
        assert_eq!(bearer(&headers(&[("authorization", "Bearer ")])), None);
        assert_eq!(bearer(&headers(&[("authorization", "Basic abc")])), None);
    }
}
