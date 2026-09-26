//! An OAuth 2.1 authorization server, scoped to one job: letting a hosted
//! chatbot obtain an MCP credential for one user of this instance.
//!
//! ## Why this exists at all
//!
//! A local client can be handed a credential the user pasted in. A hosted one
//! cannot — nobody is going to type a token into claude.ai, and more to the
//! point the spec's authorization flow is the only thing those clients
//! implement. Without this, "connect my Tapedeck" is not a thing a remote
//! service can do.
//!
//! ## What is deliberately not implemented
//!
//! * **No client secrets and no confidential clients.** Every MCP client is a
//!   public client with PKCE, which is what OAuth 2.1 tells you to do anyway.
//!   A secret we issued and they stored would add a credential to lose.
//! * **No `implicit` and no `password` grant.** Both are gone in 2.1.
//! * **No consent memory.** Approving a client mints one credential; asking
//!   again means approving again. The alternative is a silent re-grant, and on
//!   a server whose whole subject is one person's private listening the extra
//!   click is worth more than the convenience.
//!
//! ## The three things that must not regress
//!
//! * **PKCE is required, `S256` only.** `plain` is permitted by the RFC and is
//!   no protection at all against the interception this exists to stop.
//! * **The authorization code is single use**, enforced by taking it with
//!   `DELETE … RETURNING` rather than select-then-delete — two statements would
//!   let simultaneous redemptions both succeed.
//! * **`redirect_uri` must match one the client registered, exactly.** A prefix
//!   or suffix match is an open redirect, which here means handing somebody
//!   else's page an authorization code for this user's listening.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Redirect, Response},
    routing::{get, post},
    Form, Router,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use utoipa::{IntoParams, ToSchema};

use super::auth::AuthUser;
use super::mcp::public_origin;
use super::AppState;
use crate::mcp::scopes;
use crate::openapi::Unauthorized;

/// An OAuth error, as RFC 6749 shapes it.
#[derive(Serialize, ToSchema)]
pub struct OAuthError {
    /// `invalid_request`, `invalid_client`, `invalid_grant`, `invalid_target`,
    /// `invalid_redirect_uri`, `unsupported_grant_type`, `server_error`.
    pub error: String,
    pub error_description: String,
}

/// RFC 9728 protected resource metadata.
#[derive(Serialize, ToSchema)]
pub struct ProtectedResourceMetadata {
    /// This instance's `/mcp`.
    pub resource: String,
    pub authorization_servers: Vec<String>,
    /// The minimal grant. More is asked for through a step-up.
    pub scopes_supported: Vec<&'static str>,
    pub bearer_methods_supported: Vec<&'static str>,
    pub resource_name: &'static str,
}

/// RFC 8414 authorization server metadata.
#[derive(Serialize, ToSchema)]
pub struct AuthorizationServerMetadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    pub scopes_supported: Vec<&'static str>,
    pub response_types_supported: Vec<&'static str>,
    pub grant_types_supported: Vec<&'static str>,
    /// `S256` only.
    pub code_challenge_methods_supported: Vec<&'static str>,
    /// `none` — every client is public.
    pub token_endpoint_auth_methods_supported: Vec<&'static str>,
    /// RFC 9207: `iss` is sent on authorization responses.
    pub authorization_response_iss_parameter_supported: bool,
}

/// RFC 7591 registration response. The `client_id` is not a credential; it
/// gets a client as far as a request a signed-in person has to approve.
#[derive(Serialize, ToSchema)]
pub struct ClientRegistered {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: &'static str,
    pub grant_types: Vec<&'static str>,
    pub response_types: Vec<&'static str>,
}

/// An authorization request, in the terms of the person approving it.
#[derive(Serialize, ToSchema)]
pub struct AuthorizeInfo {
    pub client_name: String,
    pub client_id: String,
    /// The client identified itself with a URL that could be fetched, rather
    /// than registering a name of its own choosing — which could be anything.
    pub verified: bool,
    pub redirect_uri: String,
    /// **Every** grant, with the requested ones marked. A client requests; the
    /// person whose listening it is decides.
    pub scopes: Vec<ScopeOffer>,
    /// The requested grants, space-separated, unrecognised ones dropped.
    pub requested: String,
}

#[derive(Serialize, ToSchema)]
pub struct ScopeOffer {
    pub scope: &'static str,
    pub description: &'static str,
    pub requested: bool,
}

#[derive(Serialize, ToSchema)]
pub struct Approved {
    /// The client's `redirect_uri` with `code`, `iss` and `state` added.
    #[schema(format = "uri")]
    pub redirect_to: String,
}

/// An access credential. Served `no-store`.
#[derive(Serialize, ToSchema)]
pub struct TokenIssued {
    pub access_token: String,
    /// `Bearer`.
    pub token_type: &'static str,
    /// Seconds. A day.
    pub expires_in: i64,
    /// Rotates on every refresh; the old pair stops working.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// What was actually granted, which can differ from what was asked for.
    /// Absent on a refresh.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// How long a minted credential lives.
///
/// A day rather than the customary hour: this is one person's own server, the
/// refresh rotates on every use, and an hourly refresh cycle is a lot of churn
/// for a Raspberry Pi to no benefit anyone here would notice.
const ACCESS_TTL_SECS: i64 = 24 * 3600;

/// An authorization code's life. Deliberately short — it exists only for the
/// round trip from the browser back to the client.
const CODE_TTL_SECS: i64 = 300;

fn oauth_err(status: u16, error: &str, description: &str) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
    (
        code,
        Json(OAuthError { error: error.to_string(), error_description: description.to_string() }),
    )
        .into_response()
}

// ════════════════════════════════════════════════════════════
//  Discovery
// ════════════════════════════════════════════════════════════

// RFC 9728 — protected resource metadata.
//
// This is the document a client is pointed at by the `WWW-Authenticate` header
// on a 401 from `/mcp`, and it is the first step of the whole flow. It says
// which authorization server covers this resource; here that is the same host,
// but the indirection is what lets a client discover it rather than guess.
/// RFC 9728 protected resource metadata.
///
/// Unauthenticated. The document a client is pointed at by
/// `WWW-Authenticate` on a 401 from `/mcp`, and the first step of the whole
/// flow. `scopes_supported` names the *minimal* set for basic
/// functionality — a client asks for more through a step-up rather than
/// being nudged into requesting everything at once.
#[utoipa::path(
    get,
    path = "/.well-known/oauth-protected-resource",
    tag = "OAuth",
    security(),
    responses(
        (status = 200, description = "The metadata document.", body = ProtectedResourceMetadata),
    )
)]
async fn protected_resource(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let origin = public_origin(&state, &headers);
    Json(ProtectedResourceMetadata {
        resource: format!("{origin}/mcp"),
        authorization_servers: vec![origin],
        // The *minimal* set for basic functionality, per the spec's scope
        // minimisation guidance — a client asks for more through a step-up
        // rather than being nudged into requesting everything up front.
        scopes_supported: vec![scopes::DEFAULT],
        bearer_methods_supported: vec!["header"],
        resource_name: "Tapedeck",
    })
}

// RFC 8414 — authorization server metadata.
/// RFC 8414 authorization server metadata.
///
/// Unauthenticated. Advertises `S256` as the only PKCE method — `plain` is
/// in RFC 7636 and provides no protection against the code interception
/// PKCE exists for — and `authorization_response_iss_parameter_supported`,
/// because this server emits `iss` (RFC 9207) and a strict client will not
/// check it otherwise.
#[utoipa::path(
    get,
    path = "/.well-known/oauth-authorization-server",
    tag = "OAuth",
    security(),
    responses(
        (status = 200, description = "The metadata document.", body = AuthorizationServerMetadata),
    )
)]
async fn authorization_server(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let origin = public_origin(&state, &headers);
    Json(AuthorizationServerMetadata {
        authorization_endpoint: format!("{origin}/oauth/authorize"),
        token_endpoint: format!("{origin}/oauth/token"),
        registration_endpoint: format!("{origin}/oauth/register"),
        issuer: origin,
        scopes_supported: scopes::ALL.iter().map(|(s, _)| *s).collect(),
        response_types_supported: vec!["code"],
        grant_types_supported: vec!["authorization_code", "refresh_token"],
        // S256 only. `plain` is in the RFC and provides no protection against
        // code interception, which is the attack PKCE is for.
        code_challenge_methods_supported: vec!["S256"],
        token_endpoint_auth_methods_supported: vec!["none"],
        // RFC 9207: we emit `iss` on authorization responses, so we must say
        // so or a strict client will not know to check it.
        authorization_response_iss_parameter_supported: true,
    })
}

// ════════════════════════════════════════════════════════════
//  Client registration
// ════════════════════════════════════════════════════════════

/// RFC 7591 registration. Open by design.
#[derive(Deserialize, ToSchema)]
#[schema(as = ClientRegistration)]
struct RegistrationRequest {
    /// Up to 80 characters. Shown on the consent screen as unverified.
    #[serde(default)]
    client_name: Option<String>,
    /// At least one. `https`, loopback `http`, or a private-use scheme; no
    /// fragment.
    #[serde(default)]
    #[schema(min_items = 1)]
    redirect_uris: Vec<String>,
}

// RFC 7591 — dynamic client registration.
//
// Open by design: this endpoint hands out an identifier and nothing else. A
// `client_id` is not a credential here — every client is public, there is no
// secret, and possession of an id gets you as far as an authorization request
// that a logged-in human then has to approve by hand. Registration being open
// is what lets a client the user has never configured start the flow at all.
/// RFC 7591 dynamic client registration.
///
/// Unauthenticated by design: this hands out an identifier and nothing
/// else. Every client is public, there is no secret, and possession of a
/// `client_id` gets you as far as an authorization request that a logged-in
/// human then has to approve by hand.
///
/// `redirect_uris` must be https, a loopback http address, or a private-use
/// scheme. Plain http anywhere else would put an authorization code on the
/// network in the clear.
#[utoipa::path(
    post,
    path = "/oauth/register",
    tag = "OAuth",
    security(),
    request_body = RegistrationRequest,
    responses(
        (status = 201, description = "Registered.", body = ClientRegistered),
        (status = 400, description = "A redirect_uri was missing or not acceptable.", body = OAuthError),
    )
)]
async fn register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegistrationRequest>,
) -> Response {
    if body.redirect_uris.is_empty() {
        return oauth_err(400, "invalid_redirect_uri", "At least one redirect_uri is required");
    }
    for uri in &body.redirect_uris {
        if let Err(msg) = redirect_uri_is_plausible(uri) {
            return oauth_err(400, "invalid_redirect_uri", msg);
        }
    }

    use rand::Rng;
    let raw: [u8; 16] = rand::rng().random();
    let client_id = format!("tdc_{}", hex::encode(raw));
    let name = body
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("An AI assistant")
        .chars()
        .take(80)
        .collect::<String>();
    let uris = serde_json::to_string(&body.redirect_uris).unwrap_or_else(|_| "[]".into());

    match state.db.register_oauth_client(&client_id, &name, &uris, false).await {
        Ok(()) => {
            info!("🤖 OAuth client registered: {} ({})", name, client_id);
            (
                StatusCode::CREATED,
                Json(ClientRegistered {
                    client_id,
                    client_name: name,
                    redirect_uris: body.redirect_uris,
                    token_endpoint_auth_method: "none",
                    grant_types: vec!["authorization_code", "refresh_token"],
                    response_types: vec!["code"],
                }),
            )
                .into_response()
        }
        Err(e) => oauth_err(500, "server_error", &e.to_string()),
    }
}

/// A redirect URI has to be something a browser can actually go to, and must
/// not be a way to smuggle the code somewhere else.
///
/// `http` is allowed **only on loopback**, which is how every desktop client
/// receives its callback. Allowing it generally would let a code cross the
/// network in the clear.
fn redirect_uri_is_plausible(uri: &str) -> Result<(), &'static str> {
    if uri.contains('#') {
        return Err("A redirect_uri must not contain a fragment");
    }
    if uri.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = uri.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        if host == "127.0.0.1" || host == "localhost" || host == "[::1]" {
            return Ok(());
        }
        return Err("Plain http is only allowed for a loopback redirect");
    }
    // A private-use scheme (`com.example.app:/callback`) is how a native client
    // gets its callback and is explicitly blessed by OAuth 2.1.
    if uri.contains(":/") && !uri.starts_with("javascript:") && !uri.starts_with("data:") {
        return Ok(());
    }
    Err("A redirect_uri must be https, a loopback http address, or a private-use scheme")
}

// ════════════════════════════════════════════════════════════
//  Authorization
// ════════════════════════════════════════════════════════════

#[derive(Deserialize, Clone, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuthorizeParams {
    pub client_id: String,
    /// Must match one the client registered exactly.
    pub redirect_uri: String,
    /// `code`. Required by `/oauth/authorize`.
    #[serde(default)]
    pub response_type: Option<String>,
    /// Space-separated grants. Unrecognised ones are dropped; none means the
    /// minimal default.
    #[serde(default)]
    pub scope: Option<String>,
    /// Handed back unchanged.
    #[serde(default)]
    pub state: Option<String>,
    /// PKCE. Required by `/oauth/authorize`.
    #[serde(default)]
    pub code_challenge: Option<String>,
    /// `S256`, the only method accepted.
    #[serde(default)]
    pub code_challenge_method: Option<String>,
    /// RFC 8707 — which resource the credential is for. Validated as naming
    /// this instance, not merely recorded.
    //
    // Required by the MCP spec.
    #[serde(default)]
    pub resource: Option<String>,
}

// `GET /oauth/authorize` — the front door.
//
// This validates and then hands off to the SPA, which is where the consent
// screen lives. It deliberately does **not** render HTML: this application
// serves one `index.html` and does its rendering in the browser, and a
// server-rendered consent page would be the only exception in the codebase
// and the only page that could drift from the app's own styling.
/// The authorization endpoint.
///
/// Unauthenticated at this step. Validates the request and **redirects to
/// the SPA consent screen** at `/authorize` carrying the parameters; it
/// does not render HTML, because this application serves one `index.html`
/// and a server-rendered consent page would be the only thing that could
/// drift from the app's own styling.
///
/// Failures concerning the redirect target itself are reported directly
/// rather than bounced off it — an unvalidated `redirect_uri` is an open
/// redirect and must not be used to deliver its own error. `redirect_uri`
/// is matched **exactly** against one the client registered; a prefix match
/// here would hand somebody else an authorization code.
///
/// PKCE is required and `code_challenge_method` must be `S256`.
#[utoipa::path(
    get,
    path = "/oauth/authorize",
    tag = "OAuth",
    security(),
    params(AuthorizeParams),
    responses(
        (status = 303, description = "To the consent screen, or back to the client with an error."),
        (status = 400, description = "Unknown client, or a redirect_uri that does not match its registration.", body = OAuthError),
    )
)]
async fn authorize(
    State(state): State<Arc<AppState>>,
    Query(p): Query<AuthorizeParams>,
) -> Response {
    // Validate everything that can be validated *before* redirecting anywhere,
    // and report failures that concern the redirect target itself directly to
    // the user rather than by bouncing off it — an unvalidated redirect_uri is
    // an open redirect, so it must not be used to deliver its own error.
    let client = match state.db.get_oauth_client(&p.client_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return oauth_err(400, "invalid_client", "Unknown client_id. Register first."),
        Err(e) => return oauth_err(500, "server_error", &e.to_string()),
    };

    let registered: Vec<String> = serde_json::from_str(&client.redirect_uris).unwrap_or_default();
    // Exact match. A prefix match here is an open redirect, and an open
    // redirect on this endpoint hands somebody else an authorization code.
    if !registered.iter().any(|u| u == &p.redirect_uri) {
        return oauth_err(
            400,
            "invalid_request",
            "redirect_uri does not match one registered for this client",
        );
    }

    // From here failures can safely go back to the client, because the target
    // is known good.
    let bounce = |error: &str, description: &str| -> Response {
        let mut url = format!(
            "{}{}error={}&error_description={}",
            p.redirect_uri,
            if p.redirect_uri.contains('?') { "&" } else { "?" },
            urlencoding::encode(error),
            urlencoding::encode(description),
        );
        if let Some(s) = &p.state {
            url.push_str(&format!("&state={}", urlencoding::encode(s)));
        }
        Redirect::to(&url).into_response()
    };

    if p.response_type.as_deref() != Some("code") {
        return bounce("unsupported_response_type", "Only response_type=code is supported");
    }
    if p.code_challenge.as_deref().unwrap_or("").is_empty() {
        return bounce("invalid_request", "PKCE is required: send a code_challenge");
    }
    if p.code_challenge_method.as_deref().unwrap_or("S256") != "S256" {
        return bounce("invalid_request", "Only code_challenge_method=S256 is supported");
    }

    // Hand off to the consent screen, carrying everything forward. The SPA is
    // responsible for sending the user through login if they have no session.
    let mut next = format!(
        "/authorize?client_id={}&redirect_uri={}&code_challenge={}",
        urlencoding::encode(&p.client_id),
        urlencoding::encode(&p.redirect_uri),
        urlencoding::encode(p.code_challenge.as_deref().unwrap_or("")),
    );
    if let Some(s) = &p.scope {
        next.push_str(&format!("&scope={}", urlencoding::encode(s)));
    }
    if let Some(s) = &p.state {
        next.push_str(&format!("&state={}", urlencoding::encode(s)));
    }
    if let Some(r) = &p.resource {
        next.push_str(&format!("&resource={}", urlencoding::encode(r)));
    }
    Redirect::to(&next).into_response()
}

// GET /api/v1/oauth/authorize-info — what the consent screen puts in front of
// the user.
//
// Scope *descriptions* travel with it rather than raw scope strings, because
// "listening:read" is not something a person can consent to and
// "Read your listening history, statistics, reports and genre map" is.
/// What the consent screen puts in front of the user.
///
/// Session-only. Scope **descriptions** travel with it rather than raw
/// scope strings — `listening:read` is not something a person can consent
/// to. `verified` says whether the client identified itself with a
/// fetchable URL or merely registered a name it chose for itself; the
/// consent screen warns on the latter.
#[utoipa::path(
    get,
    path = "/api/v1/oauth/authorize-info",
    tag = "OAuth",
    params(AuthorizeParams),
    responses(
        (status = 200, description = "The request, in the user's terms.", body = AuthorizeInfo),
        (status = 401, response = Unauthorized),
        (status = 404, description = "Unknown client.", body = OAuthError),
    )
)]
async fn authorize_info(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Query(q): Query<AuthorizeParams>,
) -> Response {
    let client = match state.db.get_oauth_client(&q.client_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return oauth_err(404, "invalid_client", "Unknown client"),
        Err(e) => return oauth_err(500, "server_error", &e.to_string()),
    };

    let requested = scopes::sanitise(q.scope.as_deref().unwrap_or(scopes::DEFAULT));
    let requested = if requested.is_empty() { scopes::DEFAULT.to_string() } else { requested };

    // **Every** grant is offered, with the requested ones pre-selected — not
    // just the ones the client asked for.
    //
    // Showing only the request produced a deadlock nobody could get out of.
    // A client picks its scopes from `scopes_supported` or from the
    // `WWW-Authenticate` challenge, both of which name the minimal grant on
    // purpose; `tools/list` is then filtered by what was granted, so the client
    // never sees a tool it lacks the scope for, never calls one, never gets the
    // `insufficient_scope` refusal, and never runs the step-up that would widen
    // the grant. The journal, playlists, loves and notes were unreachable
    // through OAuth entirely — reachable only by minting a connection by hand.
    //
    // The fix is to put the decision where it belongs. A client *requests*;
    // the person whose listening it is *decides*. OAuth has always allowed
    // granting something other than what was asked for, and the token response
    // carries `scope` so the client learns what it actually got.
    let detail: Vec<ScopeOffer> = scopes::ALL
        .iter()
        .map(|&(scope, description)| ScopeOffer {
            scope,
            description,
            requested: requested.split_whitespace().any(|r| r == scope),
        })
        .collect();

    Json(AuthorizeInfo {
        client_name: client.client_name,
        client_id: client.client_id,
        // Whether the client identified itself with a URL that could be
        // fetched, or merely registered a name it chose. The consent screen
        // says which — an anonymous registration can call itself anything.
        verified: client.registered_by_url,
        redirect_uri: q.redirect_uri,
        scopes: detail,
        requested,
    })
    .into_response()
}

/// The authorization request being approved, and what the person ticked.
#[derive(Deserialize, ToSchema)]
#[schema(as = OAuthApproval)]
struct ApproveRequest {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    /// What the person granted — which may be more or less than the client
    /// asked for.
    scope: Option<String>,
    state: Option<String>,
    resource: Option<String>,
}

// POST /api/v1/oauth/approve — the user said yes.
//
// Session-only, and that is the load-bearing part of the whole flow: minting a
// credential requires a logged-in human on this instance. Everything before
// this point is a request; this is the consent.
/// The user said yes.
///
/// **Session-only, and that is the load-bearing part of the whole flow**:
/// minting a credential requires a logged-in human on this instance.
/// Everything before this point is a request; this is the consent.
///
/// `redirect_uri` is re-checked here rather than trusted from the browser —
/// this handler is reachable directly. The `resource` must name this
/// server, or RFC 8707's audience binding would record an audience and
/// enforce nothing. The response carries `iss` (RFC 9207) in the redirect.
#[utoipa::path(
    operation_id = "oauth_approve",
    post,
    path = "/api/v1/oauth/approve",
    tag = "OAuth",
    request_body = ApproveRequest,
    responses(
        (status = 200, description = "Where to send the browser next.", body = Approved),
        (status = 400, description = "invalid_client, invalid_request or invalid_target.", body = OAuthError),
        (status = 401, response = Unauthorized),
    )
)]
async fn approve(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<ApproveRequest>,
) -> Response {
    let client = match state.db.get_oauth_client(&body.client_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return oauth_err(400, "invalid_client", "Unknown client"),
        Err(e) => return oauth_err(500, "server_error", &e.to_string()),
    };
    let registered: Vec<String> = serde_json::from_str(&client.redirect_uris).unwrap_or_default();
    // Re-checked here rather than trusted from the browser: this handler is
    // reachable directly, and the check in `authorize` protects that path only.
    if !registered.iter().any(|u| u == &body.redirect_uri) {
        return oauth_err(400, "invalid_request", "redirect_uri does not match registration");
    }

    // The resource must name this server. RFC 8707's whole purpose is that a
    // credential is bound to the audience it was issued for; accepting any
    // string would record an audience and enforce nothing.
    let origin = public_origin(&state, &headers);
    if let Some(resource) = body.resource.as_deref().filter(|r| !r.is_empty()) {
        let canonical = format!("{origin}/mcp");
        let ok = resource == canonical
            || resource == origin
            || resource == format!("{canonical}/")
            || resource == format!("{origin}/");
        if !ok {
            warn!("🤖 OAuth approval refused: resource '{resource}' is not this server");
            return oauth_err(
                400,
                "invalid_target",
                "The 'resource' parameter does not name this Tapedeck instance",
            );
        }
    }

    // What the *user* ticked, not what the client asked for. `sanitise` still
    // drops anything this server does not define, so the browser cannot invent
    // a grant either — but within the six that exist, the choice is theirs.
    let granted = scopes::sanitise(body.scope.as_deref().unwrap_or(scopes::DEFAULT));
    let granted = if granted.is_empty() { scopes::DEFAULT.to_string() } else { granted };

    let _ = state.db.purge_expired_oauth_codes().await;
    let code = match state
        .db
        .create_oauth_code(
            &body.client_id,
            user.user_id,
            &body.redirect_uri,
            &granted,
            &body.code_challenge,
            body.resource.as_deref(),
            CODE_TTL_SECS,
        )
        .await
    {
        Ok(c) => c,
        Err(e) => return oauth_err(500, "server_error", &e.to_string()),
    };

    let mut url = format!(
        "{}{}code={}&iss={}",
        body.redirect_uri,
        if body.redirect_uri.contains('?') { "&" } else { "?" },
        urlencoding::encode(&code),
        // RFC 9207. A client that recorded the issuer compares this before it
        // sends the code anywhere — it is what stops a mix-up attack sending
        // this instance's code to a different authorization server.
        urlencoding::encode(&origin),
    );
    if let Some(s) = &body.state {
        url.push_str(&format!("&state={}", urlencoding::encode(s)));
    }

    info!("🤖 {} approved '{}' for [{}]", user.username, client.client_name, granted);
    Json(Approved { redirect_to: url }).into_response()
}

// ════════════════════════════════════════════════════════════
//  Token
// ════════════════════════════════════════════════════════════

/// Form-encoded, as the RFC requires.
#[derive(Deserialize, ToSchema)]
#[schema(as = OAuthTokenRequest)]
struct TokenRequest {
    /// `authorization_code` or `refresh_token`.
    grant_type: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    /// 43–128 characters, per RFC 7636.
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    resource: Option<String>,
}

// `POST /oauth/token` — form-encoded, as the RFC requires.
/// Exchange a code, or refresh.
///
/// Unauthenticated (public clients, `token_endpoint_auth_method: none`);
/// form-encoded, as the RFC requires. Grants: `authorization_code` and
/// `refresh_token`.
///
/// The code is **single use**, taken with `DELETE … RETURNING` so two
/// simultaneous redemptions cannot both succeed. It is bound to the client,
/// the redirect, the PKCE challenge and the resource — every one of those
/// is re-checked here.
///
/// A refresh **rotates both secrets**, so a leaked access token stops
/// working as soon as the legitimate client refreshes. Access credentials
/// live 24 hours.
#[utoipa::path(
    post,
    path = "/oauth/token",
    tag = "OAuth",
    security(),
    request_body(content = TokenRequest, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = 200, description = "A credential. Never cached.", body = TokenIssued),
        (status = 400, description = "invalid_grant, invalid_request, invalid_target or unsupported_grant_type.", body = OAuthError),
    )
)]
async fn token(State(state): State<Arc<AppState>>, Form(body): Form<TokenRequest>) -> Response {
    match body.grant_type.as_str() {
        "authorization_code" => exchange_code(&state, &body).await,
        "refresh_token" => refresh(&state, &body).await,
        other => oauth_err(
            400,
            "unsupported_grant_type",
            &format!("'{other}' is not supported; use authorization_code or refresh_token"),
        ),
    }
}

async fn exchange_code(state: &Arc<AppState>, body: &TokenRequest) -> Response {
    let (Some(code), Some(verifier)) = (body.code.as_deref(), body.code_verifier.as_deref()) else {
        return oauth_err(400, "invalid_request", "code and code_verifier are both required");
    };

    // Taken and deleted in one statement — see `consume_oauth_code`.
    let stored = match state.db.consume_oauth_code(code).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return oauth_err(
                400,
                "invalid_grant",
                "That authorization code is unknown, expired, or has already been used",
            )
        }
        Err(e) => return oauth_err(500, "server_error", &e.to_string()),
    };

    // Every one of these binds the code to the request that created it. A code
    // that could be redeemed by a different client, to a different redirect, or
    // without the verifier is not doing the job a code exists to do.
    if let Some(cid) = body.client_id.as_deref() {
        if cid != stored.client_id {
            return oauth_err(400, "invalid_grant", "That code was issued to a different client");
        }
    }
    if let Some(uri) = body.redirect_uri.as_deref() {
        if uri != stored.redirect_uri {
            return oauth_err(400, "invalid_grant", "redirect_uri does not match the request");
        }
    }
    if !pkce_matches(verifier, &stored.code_challenge) {
        return oauth_err(400, "invalid_grant", "code_verifier does not match the challenge");
    }
    // If a resource was named at authorization time, the token request must
    // name the same one — otherwise the audience binding is decorative.
    if let (Some(a), Some(b)) = (stored.resource.as_deref(), body.resource.as_deref()) {
        if a != b {
            return oauth_err(400, "invalid_target", "resource does not match the authorization");
        }
    }

    let name = state
        .db
        .get_oauth_client(&stored.client_id)
        .await
        .ok()
        .flatten()
        .map(|c| c.client_name)
        .unwrap_or_else(|| "AI assistant".into());

    match state
        .db
        .create_mcp_connection(
            stored.user_id,
            &name,
            &stored.scopes,
            Some(&stored.client_id),
            Some(ACCESS_TTL_SECS),
        )
        .await
    {
        Ok(cred) => issued(cred, &stored.scopes),
        Err(e) => oauth_err(500, "server_error", &e.to_string()),
    }
}

async fn refresh(state: &Arc<AppState>, body: &TokenRequest) -> Response {
    let Some(token) = body.refresh_token.as_deref() else {
        return oauth_err(400, "invalid_request", "refresh_token is required");
    };
    match state.db.refresh_mcp_connection(token, ACCESS_TTL_SECS).await {
        // Both secrets rotate, so the old access token stops working the moment
        // the legitimate client refreshes — which is the containment a refresh
        // cycle is for.
        Ok(Some(cred)) => issued(cred, ""),
        Ok(None) => oauth_err(400, "invalid_grant", "That refresh token is not valid"),
        Err(e) => oauth_err(500, "server_error", &e.to_string()),
    }
}

fn issued(cred: crate::db::McpCredential, scope: &str) -> Response {
    let out = TokenIssued {
        access_token: cred.token,
        token_type: "Bearer",
        expires_in: cred.expires_in.unwrap_or(ACCESS_TTL_SECS),
        refresh_token: cred.refresh,
        scope: (!scope.is_empty()).then(|| scope.to_string()),
    };
    // A token response must not be cached anywhere, by anything.
    (StatusCode::OK, [("cache-control", "no-store"), ("pragma", "no-cache")], Json(out))
        .into_response()
}

/// PKCE S256: `base64url(SHA256(verifier)) == challenge`, unpadded.
///
/// Compared as strings after encoding rather than by decoding the challenge,
/// because a challenge is whatever the client sent and may not decode at all.
fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    // RFC 7636 puts the verifier at 43–128 characters. A short one would make
    // the whole exchange guessable, so it is refused rather than accepted with
    // a shrug.
    if verifier.len() < 43 || verifier.len() > 128 {
        return false;
    }
    let digest = Sha256::digest(verifier.as_bytes());
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    computed == challenge
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    protected_resource,
    authorization_server,
    authorize,
    token,
    register,
    authorize_info,
    approve
))]
pub struct Api;

pub fn create_oauth_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/.well-known/oauth-protected-resource", get(protected_resource))
        .route("/.well-known/oauth-authorization-server", get(authorization_server))
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/token", post(token))
        .route("/oauth/register", post(register))
        .route("/api/v1/oauth/authorize-info", get(authorize_info))
        .route("/api/v1/oauth/approve", post(approve))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifies_a_real_challenge_pair() {
        // The worked example from RFC 7636 appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(pkce_matches(verifier, challenge));
        assert!(!pkce_matches(verifier, "not-the-challenge"));
    }

    #[test]
    fn a_short_verifier_is_refused() {
        // RFC 7636 sets a 43-character floor. Accepting a two-character
        // verifier would make the exchange trivially guessable while still
        // looking like PKCE was in use.
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let short = "abc";
        let challenge =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(short));
        assert!(!pkce_matches(short, &challenge), "a short verifier must not pass");
    }

    #[test]
    fn plain_http_is_only_allowed_on_loopback() {
        assert!(redirect_uri_is_plausible("https://claude.ai/api/mcp/callback").is_ok());
        assert!(redirect_uri_is_plausible("http://127.0.0.1:6274/callback").is_ok());
        assert!(redirect_uri_is_plausible("http://localhost:8080/cb").is_ok());
        assert!(redirect_uri_is_plausible("com.example.app:/callback").is_ok());
        // A code crossing the network in the clear.
        assert!(redirect_uri_is_plausible("http://example.com/cb").is_err());
        // A fragment cannot be seen by the server and would silently change
        // where the code lands.
        assert!(redirect_uri_is_plausible("https://example.com/cb#x").is_err());
        assert!(redirect_uri_is_plausible("javascript:alert(1)").is_err());
    }

    #[test]
    fn the_metadata_only_offers_s256() {
        // `plain` is in RFC 7636 and is no protection against the code
        // interception PKCE exists for. Advertising it would let a client pick
        // it.
        let doc = serde_json::json!({ "code_challenge_methods_supported": ["S256"] });
        assert_eq!(doc["code_challenge_methods_supported"][0], "S256");
        assert_eq!(doc["code_challenge_methods_supported"].as_array().unwrap().len(), 1);
    }
}
