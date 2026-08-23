use axum::{
    extract::{ConnectInfo, Multipart, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::warn;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::Database;

/// `AuthUser` proves *authentication* only. Anything that reads or mutates
/// other users' data must gate on this explicitly.
fn forbidden(action: &str) -> axum::response::Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!(ErrorResponse {
            code: 403,
            error: format!("Admin privileges required to {}", action),
        })),
    )
        .into_response()
}

// ═══════════════════════════════════════════════════════════
//  Cookie helpers
// ═══════════════════════════════════════════════════════════

/// Whether to mark the session cookie `Secure`.
///
/// The flag is enforced by the *browser*, so what matters is the scheme the
/// browser used — not the scheme we were reached on. Behind a TLS-terminating
/// proxy (Cloudflare Tunnel, nginx, Caddy) the origin sees plain HTTP while the
/// browser is on HTTPS, so we read `X-Forwarded-Proto`.
///
/// Only trusted when `trust_proxy` is set: otherwise anyone could send the
/// header directly. With it off we leave `Secure` unset, which keeps a
/// plain-HTTP LAN deployment working — setting it there would make the cookie
/// unusable and lock everyone out.
fn cookie_is_secure(headers: &HeaderMap, trust_proxy: bool) -> bool {
    trust_proxy
        && headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            // May be a comma-separated chain; the first hop is the client's.
            .map(|v| v.split(',').next().unwrap_or("").trim().eq_ignore_ascii_case("https"))
            .unwrap_or(false)
}

fn session_set_cookie(token: &str, secure: bool) -> HeaderValue {
    format!(
        "td_session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800{}",
        token,
        if secure { "; Secure" } else { "" }
    )
    .parse()
    .unwrap()
}

/// Must carry the same attributes as the cookie it clears, or the browser keeps
/// the original and logout silently does nothing.
fn session_clear_cookie(secure: bool) -> HeaderValue {
    format!(
        "td_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{}",
        if secure { "; Secure" } else { "" }
    )
    .parse()
    .unwrap()
}

// ═══════════════════════════════════════════════════════════
//  Auth endpoints (mostly unauthenticated)
// ═══════════════════════════════════════════════════════════

/// GET /api/v1/auth/status — unauthenticated, returns setup + auth state
async fn auth_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    // **`unwrap_or(false)`, and this is the whole of a production incident.**
    //
    // It was `unwrap_or(true)`: when the database could not be asked, this
    // reported *"this instance has no users — create an admin account"*. That is
    // a failure to read being published as a fact about the data, and it is the
    // single most alarming thing this application can say. On 2026-08-23 a
    // connection-pool exhaustion made every query time out, and every member of
    // a live instance was shown a first-run setup screen with their years of
    // listening apparently gone. Nothing was gone; the app simply could not
    // count.
    //
    // Failing *closed* is the safe direction here, and it is the opposite of
    // what one assumes: an instance wrongly told "setup is done" shows a login
    // box, which is recoverable and true five seconds later. An instance
    // wrongly told "there are no users" tells its owner their data is gone.
    //
    // Same family as the fidelity cards — "we could not measure" rendered as a
    // measurement — except this one says it about the whole instance.
    let needs_setup = state.db.needs_setup().await.unwrap_or(false);

    let session_user = if let Some(session_token) =
        headers.get("cookie").and_then(|v| v.to_str().ok()).and_then(|c| {
            c.split(';').find_map(|s| s.trim().strip_prefix("td_session=").map(|v| v.to_string()))
        }) {
        state.db.validate_session(&session_token).await.ok().flatten()
    } else {
        None
    };

    Json(serde_json::json!({
        "needs_setup": needs_setup,
        "authenticated": session_user.is_some(),
        "is_admin": session_user.as_ref().map(|u| u.is_admin).unwrap_or(false),
    }))
}

/// POST /api/v1/auth/setup — first-run: set admin password, get token
#[derive(Deserialize)]
struct SetupRequest {
    username: String,
    password: String,
    display_name: Option<String>,
}

async fn setup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<SetupRequest>,
) -> impl IntoResponse {
    // Guard: only works if no user has a real password yet
    match state.db.needs_setup().await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!(ErrorResponse {
                    code: 403,
                    error: "Setup already completed".into()
                })),
            )
                .into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response()
        }
    }

    if body.username.trim().is_empty() || body.password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Username required, password must be at least 8 characters".into(),
            })),
        )
            .into_response();
    }

    let password_hash = match Database::hash_password(&body.password) {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e })),
            )
                .into_response()
        }
    };

    let user_id = match state
        .db
        .setup_admin(body.username.trim(), body.display_name.as_deref(), &password_hash)
        .await
    {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response()
        }
    };

    // Create a default API token
    let token =
        match state.db.create_token(user_id, "default", "submit,read,admin", None, false).await {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!(ErrorResponse {
                        code: 500,
                        error: format!("Admin created but token failed: {}", e)
                    })),
                )
                    .into_response()
            }
        };

    // Log them in (create session)
    let session_token = match state.db.create_web_session(user_id).await {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response()
        }
    };

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::SET_COOKIE,
        session_set_cookie(&session_token, cookie_is_secure(&headers, state.trust_proxy)),
    );

    (
        StatusCode::OK,
        resp_headers,
        Json(serde_json::json!({
            "user_id": user_id,
            "username": body.username.trim(),
            "token": token,
            "message": "Admin account created. Save the API token — it won't be shown again.",
        })),
    )
        .into_response()
}

/// POST /api/v1/auth/login
#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

/// Client key for rate limiting. `X-Forwarded-For` is only trusted when the
/// instance is configured to sit behind a proxy — otherwise a direct client
/// could spoof the header and evade the throttle, so we use the socket IP.
fn client_key(headers: &HeaderMap, addr: SocketAddr, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return ip;
        }
    }
    addr.ip().to_string()
}

async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let key = client_key(&headers, addr, state.trust_proxy);
    if let Some(retry_after) = state.login_limiter.blocked_for(&key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.to_string())],
            Json(serde_json::json!(ErrorResponse {
                code: 429,
                error: format!("Too many failed logins. Try again in {}s.", retry_after),
            })),
        )
            .into_response();
    }

    let user = match state.db.authenticate(&body.username, &body.password).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            state.login_limiter.record_failure(&key);
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!(ErrorResponse {
                    code: 401,
                    error: "Invalid username or password".into()
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response()
        }
    };
    state.login_limiter.reset(&key);

    let session_token = match state.db.create_web_session(user.user_id).await {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response()
        }
    };

    // Read the *request* headers for the proto before building the response set.
    let secure = cookie_is_secure(&headers, state.trust_proxy);
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(header::SET_COOKIE, session_set_cookie(&session_token, secure));

    (
        StatusCode::OK,
        resp_headers,
        Json(serde_json::json!({
            "user_id": user.user_id,
            "username": user.username,
            "is_admin": user.is_admin,
        })),
    )
        .into_response()
}

/// POST /api/v1/auth/logout — reads cookie, no AuthUser required
async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    // Try to delete the session from DB
    if let Some(session_token) = headers.get("cookie").and_then(|v| v.to_str().ok()).and_then(|c| {
        c.split(';').find_map(|s| s.trim().strip_prefix("td_session=").map(|v| v.to_string()))
    }) {
        let _ = state.db.delete_web_session(&session_token).await;
    }

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::SET_COOKIE,
        session_clear_cookie(cookie_is_secure(&headers, state.trust_proxy)),
    );

    (StatusCode::OK, resp_headers, Json(serde_json::json!({ "status": "logged out" })))
}

/// GET /api/v1/auth/me — requires auth
async fn me(user: AuthUser) -> impl IntoResponse {
    Json(serde_json::json!({
        "user_id": user.user_id,
        "username": user.username,
        "is_admin": user.is_admin,
    }))
}

// ═══════════════════════════════════════════════════════════
//  User management (existing, requires auth)
// ═══════════════════════════════════════════════════════════

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub display_name: Option<String>,
    pub password: Option<String>,
}

#[derive(Serialize)]
pub struct CreateUserResponse {
    pub user_id: i64,
    pub username: String,
    pub token: String,
    pub message: String,
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if !user.is_admin {
        return forbidden("create users");
    }
    if body.username.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Username cannot be empty".into()
            })),
        )
            .into_response();
    }

    let password_hash = if let Some(ref pw) = body.password {
        match Database::hash_password(pw) {
            Ok(h) => h,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!(ErrorResponse { code: 500, error: e })),
                )
                    .into_response()
            }
        }
    } else {
        "not-used-yet".to_string()
    };

    let user_id = match state
        .db
        .create_user(body.username.trim(), body.display_name.as_deref(), &password_hash, "user")
        .await
    {
        Ok(id) => id,
        Err(e) => {
            let msg = e.to_string();
            let (status, error) = if msg.contains("UNIQUE") {
                (StatusCode::CONFLICT, format!("Username '{}' already exists", body.username))
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to create user: {}", msg))
            };
            return (
                status,
                Json(serde_json::json!(ErrorResponse { code: status.as_u16(), error })),
            )
                .into_response();
        }
    };

    let token = match state.db.create_token(user_id, "default", "submit", None, false).await {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    code: 500,
                    error: format!("User created but token failed: {}", e)
                })),
            )
                .into_response()
        }
    };

    (
        StatusCode::CREATED,
        Json(serde_json::json!(CreateUserResponse {
            user_id,
            username: body.username.trim().to_string(),
            token,
            message: "User created. Save the token — it won't be shown again.".into(),
        })),
    )
        .into_response()
}

async fn list_users(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return forbidden("list users");
    }
    match state.db.list_users().await {
        Ok(users) => (StatusCode::OK, Json(serde_json::json!({ "users": users }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                code: 500,
                error: format!("Failed to list users: {}", e)
            })),
        )
            .into_response(),
    }
}

/// PATCH /admin/users/:id — change role and/or reset password (admin-only).
#[derive(Deserialize)]
pub struct UpdateUserRequest {
    pub role: Option<String>,     // "admin" | "user"
    pub password: Option<String>, // reset to this (min 8 chars)
    pub disabled: Option<bool>,   // block/allow login without deleting data
}

async fn update_user(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateUserRequest>,
) -> impl IntoResponse {
    if !user.is_admin {
        return forbidden("modify users");
    }

    // Whether this action would strip the last admin who can still log in.
    let (target_role, target_disabled) = state
        .db
        .get_user_role_disabled(id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| ("user".into(), false));
    let target_is_active_admin = target_role == "admin" && !target_disabled;
    let is_last_admin =
        target_is_active_admin && state.db.count_active_admins().await.unwrap_or(0) <= 1;

    if let Some(ref role) = body.role {
        if role != "admin" && role != "user" {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    code: 400,
                    error: "role must be 'admin' or 'user'".into()
                })),
            )
                .into_response();
        }
        if role == "user" && is_last_admin {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    code: 400,
                    error: "Cannot demote the last admin".into()
                })),
            )
                .into_response();
        }
        if let Err(e) = state.db.update_user_role(id, role).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response();
        }
    }

    if let Some(disabled) = body.disabled {
        if disabled && id == user.user_id {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    code: 400,
                    error: "You cannot disable your own account".into()
                })),
            )
                .into_response();
        }
        if disabled && is_last_admin {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    code: 400,
                    error: "Cannot disable the last admin".into()
                })),
            )
                .into_response();
        }
        if let Err(e) = state.db.set_user_disabled(id, disabled).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response();
        }
    }

    if let Some(ref pw) = body.password {
        if pw.len() < 8 {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    code: 400,
                    error: "Password must be at least 8 characters".into()
                })),
            )
                .into_response();
        }
        let hash = match Database::hash_password(pw) {
            Ok(h) => h,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!(ErrorResponse { code: 500, error: e })),
                )
                    .into_response()
            }
        };
        if let Err(e) = state.db.update_user_password(id, &hash).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response();
        }
    }

    (StatusCode::OK, Json(serde_json::json!({ "status": "updated" }))).into_response()
}

/// DELETE /admin/users/:id — remove a user and all their data (admin-only).
async fn delete_user(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if !user.is_admin {
        return forbidden("delete users");
    }
    if id == user.user_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "You cannot delete your own account".into()
            })),
        )
            .into_response();
    }
    // Guard against removing the last admin who can still log in.
    let (role, disabled) = state
        .db
        .get_user_role_disabled(id)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| ("user".into(), false));
    if role == "admin" && !disabled && state.db.count_active_admins().await.unwrap_or(0) <= 1 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Cannot delete the last admin".into()
            })),
        )
            .into_response();
    }
    match state.db.delete_user(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "User not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

// ═══════════════════════════════════════════════════════════
//  Token management
// ═══════════════════════════════════════════════════════════

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    pub user_id: Option<i64>,
    pub name: String,
    #[serde(default = "default_scopes")]
    pub scopes: String,
    /// The chain every listen submitted with this token is attributed to,
    /// unless the listen carries better evidence. Optional — a token with no
    /// chain behaves exactly as before.
    pub default_chain_id: Option<i64>,
    /// Whether this client sends `listen_type: "import"` for *live* scrobbles
    /// rather than for a backfill. Off unless asked for — it is what lets the
    /// client's listens reach the owner's Last.fm and ListenBrainz.
    #[serde(default)]
    pub imports_are_live: bool,
}

fn default_scopes() -> String {
    "submit".into()
}

#[derive(Serialize)]
pub struct CreateTokenResponse {
    pub token: String,
    pub name: String,
    pub user_id: i64,
    pub message: String,
}

async fn create_token(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateTokenRequest>,
) -> impl IntoResponse {
    // Minting a token *for someone else* is an account takeover, so it is
    // admin-only. Without this, any user can hand themselves an admin token.
    let target_user_id = body.user_id.unwrap_or(user.user_id);
    if target_user_id != user.user_id && !user.is_admin {
        return forbidden("issue tokens for another user");
    }
    if body.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Token name cannot be empty".into()
            })),
        )
            .into_response();
    }
    // A chain id from the request body is user input: check it belongs to the
    // token's owner, or a token could be pointed at someone else's chain.
    if let Some(cid) = body.default_chain_id {
        match state.db.get_chain(target_user_id, cid).await {
            Ok(Some(_)) => {}
            _ => return chain_not_found(),
        }
    }

    match state
        .db
        .create_token(
            target_user_id,
            body.name.trim(),
            &body.scopes,
            body.default_chain_id,
            body.imports_are_live,
        )
        .await
    {
        Ok(token) => (
            StatusCode::CREATED,
            Json(serde_json::json!(CreateTokenResponse {
                token,
                name: body.name.trim().to_string(),
                user_id: target_user_id,
                message: "Token created. Save it — it won't be shown again.".into(),
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                code: 500,
                error: format!("Failed to create token: {}", e)
            })),
        )
            .into_response(),
    }
}

fn chain_not_found() -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!(ErrorResponse { code: 400, error: "Unknown signal chain".into() })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct UpdateTokenRequest {
    pub name: Option<String>,
    /// Present-and-null clears the chain; absent leaves it alone. Serde can't
    /// tell those apart on a plain `Option`, so this is a nested one.
    #[serde(default, deserialize_with = "double_option")]
    pub default_chain_id: Option<Option<i64>>,
    /// Absent leaves the setting alone. A plain `Option` is right here — unlike
    /// `default_chain_id`, a bool has no third "explicitly cleared" state to
    /// distinguish from absent, so `None` can only mean "not mentioned".
    #[serde(default)]
    pub imports_are_live: Option<bool>,
}

fn double_option<'de, D>(de: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i64>::deserialize(de).map(Some)
}

/// Rename a token or repoint its chain, without re-issuing it — so fixing the
/// chain doesn't mean editing config in the client you already set up.
async fn update_token(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(token_id): Path<i64>,
    Json(body): Json<UpdateTokenRequest>,
) -> impl IntoResponse {
    let existing = match state.db.list_tokens(user.user_id).await {
        Ok(tokens) => tokens.into_iter().find(|t| t.id == token_id),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
            )
                .into_response()
        }
    };
    let Some(existing) = existing else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Token not found".into() })),
        )
            .into_response();
    };

    let name = match body.name.as_deref().map(str::trim) {
        Some("") => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    code: 400,
                    error: "Token name cannot be empty".into()
                })),
            )
                .into_response()
        }
        Some(n) => n.to_string(),
        None => existing.name.clone(),
    };
    let chain_id = body.default_chain_id.unwrap_or(existing.default_chain_id);
    let imports_are_live = body.imports_are_live.unwrap_or(existing.imports_are_live);

    if let Some(cid) = chain_id {
        match state.db.get_chain(user.user_id, cid).await {
            Ok(Some(_)) => {}
            _ => return chain_not_found(),
        }
    }

    match state.db.update_token(user.user_id, token_id, &name, chain_id, imports_are_live).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "status": "updated" }))).into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Token not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

async fn list_tokens(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.list_tokens(user.user_id).await {
        Ok(tokens) => {
            (StatusCode::OK, Json(serde_json::json!({ "tokens": tokens }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                code: 500,
                error: format!("Failed to list tokens: {}", e)
            })),
        )
            .into_response(),
    }
}

/// Re-issue a token you no longer have a copy of. Scoped to the caller's own
/// tokens by the query, same as revoke.
async fn rotate_token(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(token_id): Path<i64>,
) -> impl IntoResponse {
    match state.db.rotate_token(user.user_id, token_id).await {
        Ok(Some(token)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "token": token,
                "message": "Token re-issued. Copy it now — it won't be shown again.",
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Token not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

async fn revoke_token(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(token_id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_token(user.user_id, token_id).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "status": "revoked" }))).into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Token not found".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

// ═══════════════════════════════════════════════════════════
//  Runtime log level (admin-only)
// ═══════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct LogLevelRequest {
    level: String,
}

async fn get_log_level(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return forbidden("view the log level");
    }
    (StatusCode::OK, Json(serde_json::json!({ "current_level": state.log_handle.get_level() })))
        .into_response()
}

async fn set_log_level(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<LogLevelRequest>,
) -> impl IntoResponse {
    if !user.is_admin {
        return forbidden("set the log level");
    }
    match state.log_handle.set_level(&body.level) {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({ "current_level": body.level.to_uppercase() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse { code: 400, error: e })),
        )
            .into_response(),
    }
}

// ═══════════════════════════════════════════════════════════
//  Your own account
// ═══════════════════════════════════════════════════════════
//
//  Everything here is scoped to `user.user_id` and takes no id in the path.
//  That is the whole point: until this existed, an account created for someone
//  else could only ever be changed *by the admin who created it* — the owner
//  could not set their own password, which is a security gap rather than a
//  missing nicety. These are session-only (`AuthUser`, not `WriteAuth`): a
//  scrobble client has no business changing the credential it authenticates
//  with.

/// The biggest avatar we will store. Far smaller than a booklet scan
/// (`MAX_SCAN_BYTES`, 24 MB) because this is one small square shown at 40px,
/// and the limit is the only thing standing between the uploads directory and
/// someone's 40-megapixel camera roll.
const MAX_AVATAR_BYTES: usize = 4 * 1024 * 1024;

/// Turn the stored `avatar_url` into something the browser can actually load.
///
/// An avatar is either an external URL the user pasted or a file this server
/// holds, and the browser must not learn which path on disk the second one is
/// — same rule the shelf's `file_path` follows. So a stored relative path
/// becomes the endpoint that serves it and the raw value never leaves.
fn avatar_src(stored: Option<&str>) -> Option<String> {
    let v = stored?.trim();
    if v.is_empty() {
        return None;
    }
    if v.starts_with("http://") || v.starts_with("https://") {
        Some(v.to_string())
    } else {
        Some("/api/v1/profile/avatar".to_string())
    }
}

/// GET /api/v1/profile — your own account.
async fn get_profile(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.get_profile(user.user_id).await {
        Ok(Some(p)) => {
            let mut body = serde_json::json!(p);
            let src = avatar_src(p.avatar_url.as_deref());
            if let Some(obj) = body.as_object_mut() {
                // The path on disk is replaced, not accompanied.
                obj.remove("avatar_url");
                obj.insert("avatar".into(), serde_json::json!(src));
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "No such user".into() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    /// Present-and-null clears; absent leaves alone. See `update_profile`.
    #[serde(default, deserialize_with = "double_option_str")]
    pub display_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option_str")]
    pub bio: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option_str")]
    pub avatar_url: Option<Option<String>>,
}

fn double_option_str<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(de).map(Some)
}

/// How long a bio may be.
///
/// A bound rather than a free-for-all because this is the one piece of user
/// text that Phase 6 will show to *other people*, and an unbounded column that
/// has already been filled is far harder to put a limit on later.
const MAX_BIO_LEN: usize = 500;
const MAX_DISPLAY_NAME_LEN: usize = 80;

/// PATCH /api/v1/profile — change your display name, bio or avatar.
async fn patch_profile(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<UpdateProfileRequest>,
) -> impl IntoResponse {
    let too_long = |what: &str, max: usize| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: format!("{} must be {} characters or fewer", what, max),
            })),
        )
            .into_response()
    };

    // Trim, and treat an all-whitespace value as a clear rather than storing a
    // blank that renders as an empty line nobody can see to delete.
    let clean = |v: Option<Option<String>>| -> Option<Option<String>> {
        v.map(|inner| inner.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
    };
    let display_name = clean(body.display_name);
    let bio = clean(body.bio);
    let avatar_url = clean(body.avatar_url);

    if let Some(Some(ref s)) = display_name {
        if s.chars().count() > MAX_DISPLAY_NAME_LEN {
            return too_long("Display name", MAX_DISPLAY_NAME_LEN);
        }
    }
    if let Some(Some(ref s)) = bio {
        if s.chars().count() > MAX_BIO_LEN {
            return too_long("Bio", MAX_BIO_LEN);
        }
    }
    // The column holds either an external URL or a path this server wrote, and
    // only the upload handler may produce the second kind. Without this, a
    // client could set `avatar_url` to any relative path and have
    // `serve_avatar` read it back out of the uploads directory — including
    // another user's scans, which are laid out under a predictable
    // `shelf/<user>/<release>/` and are meant to be scoped by user id.
    if let Some(Some(ref s)) = avatar_url {
        if !(s.starts_with("http://") || s.starts_with("https://")) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!(ErrorResponse {
                    code: 400,
                    error: "avatar_url must be an http(s) URL — upload a file to \
                            /api/v1/profile/avatar instead"
                        .into()
                })),
            )
                .into_response();
        }
    }

    match state.db.update_profile(user.user_id, display_name, bio, avatar_url).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "status": "updated" }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// POST /api/v1/profile/password — change your own password.
///
/// The current password is **required**, which is what separates this from the
/// admin reset on `PATCH /admin/users/:id`. The caller already holds a valid
/// session, so without that check an unlocked browser is enough to lock the
/// real owner out of their own account.
///
/// Every other session is dropped on success. A password change is usually a
/// response to "someone else may have this", and leaving the other sessions
/// alive would defeat the reason for making it.
async fn change_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    user: AuthUser,
    Json(body): Json<ChangePasswordRequest>,
) -> impl IntoResponse {
    if body.new_password.chars().count() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Password must be at least 8 characters".into()
            })),
        )
            .into_response();
    }

    if !state.db.check_user_password(user.user_id, &body.current_password).await {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse {
                code: 403,
                error: "Current password is incorrect".into()
            })),
        )
            .into_response();
    }

    let hash = match Database::hash_password(&body.new_password) {
        Ok(h) => h,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse { code: 500, error: e })),
            )
                .into_response()
        }
    };
    if let Err(e) = state.db.update_user_password(user.user_id, &hash).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response();
    }

    // Every existing session goes, including the one making this request — so
    // a fresh one is issued straight back, or the user would be logged out of
    // the very tab they changed their password in.
    if let Err(e) = state.db.clear_user_sessions(user.user_id).await {
        warn!("Could not clear sessions after a password change: {}", e);
    }

    let secure = cookie_is_secure(&headers, state.trust_proxy);
    match state.db.create_web_session(user.user_id).await {
        Ok(token) => {
            let mut out =
                (StatusCode::OK, Json(serde_json::json!({ "status": "updated" }))).into_response();
            out.headers_mut().insert(header::SET_COOKIE, session_set_cookie(&token, secure));
            out
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/v1/profile/avatar — upload a profile picture.
///
/// The path written into `avatar_url` is produced here and never accepted from
/// a request, which is what `patch_profile` refuses a relative value for.
async fn upload_avatar(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let bad = |code: StatusCode, msg: &str| {
        (code, Json(serde_json::json!(ErrorResponse { code: code.as_u16(), error: msg.into() })))
            .into_response()
    };

    let mut file: Option<(Vec<u8>, String)> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        // The declared content type decides the extension; a filename from the
        // client never reaches the path we write.
        let declared = field.content_type().unwrap_or("").to_string();
        let named = field.file_name().unwrap_or("").to_string();
        let mime = if super::physical::image_ext(&declared).is_some() {
            declared
        } else {
            mime_guess::from_path(&named).first_raw().unwrap_or("").to_string()
        };
        if let Ok(b) = field.bytes().await {
            file = Some((b.to_vec(), mime));
            break;
        }
    }

    let Some((bytes, mime)) = file else {
        return bad(StatusCode::BAD_REQUEST, "No image in the request");
    };
    if bytes.len() > MAX_AVATAR_BYTES {
        return bad(StatusCode::PAYLOAD_TOO_LARGE, "That picture is larger than 4 MB");
    }
    let Some(ext) = super::physical::image_ext(&mime) else {
        return bad(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Only JPEG, PNG, WebP, GIF, AVIF and TIFF images can be uploaded",
        );
    };

    let dir = state.uploads_dir.join("avatars").join(user.user_id.to_string());
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        warn!("avatar: could not create {}: {}", dir.display(), e);
        return bad(StatusCode::INTERNAL_SERVER_ERROR, "Could not write to the uploads directory");
    }

    // A fresh random name each time rather than a fixed one, so the immutable
    // cache header on `serve_avatar` stays honest — a replaced picture is a new
    // URL, not the same URL with different bytes behind it.
    let name = format!("{}.{}", super::physical::random_name(), ext);
    let rel = format!("avatars/{}/{}", user.user_id, name);
    if let Err(e) = tokio::fs::write(dir.join(&name), &bytes).await {
        warn!("avatar: could not save: {}", e);
        return bad(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the picture");
    }

    // The one it replaces, so a user swapping their picture ten times leaves
    // one file rather than ten.
    let previous =
        state.db.get_profile(user.user_id).await.ok().flatten().and_then(|p| p.avatar_url);

    if let Err(e) = state.db.update_profile(user.user_id, None, None, Some(Some(rel))).await {
        return bad(StatusCode::INTERNAL_SERVER_ERROR, &format!("Could not save: {e}"));
    }
    if let Some(old) = previous {
        if !old.starts_with("http") {
            let _ = tokio::fs::remove_file(state.uploads_dir.join(&old)).await;
        }
    }

    (StatusCode::OK, Json(serde_json::json!({ "avatar": "/api/v1/profile/avatar" })))
        .into_response()
}

/// GET /api/v1/profile/avatar — the caller's own uploaded picture.
///
/// Scoped to the session like the artwork proxy, and cached `private`: a shared
/// cache holding one would hand it to the next person through the proxy.
async fn serve_avatar(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let stored = state.db.get_profile(user.user_id).await.ok().flatten().and_then(|p| p.avatar_url);
    let Some(rel) = stored.filter(|s| !s.starts_with("http")) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "No picture".into() })),
        )
            .into_response();
    };
    match tokio::fs::read(state.uploads_dir.join(&rel)).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    mime_guess::from_path(&rel).first_raw().unwrap_or("application/octet-stream"),
                ),
                (header::CACHE_CONTROL, "private, max-age=31536000, immutable"),
            ],
            axum::body::Body::from(bytes),
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "The file is gone".into() })),
        )
            .into_response(),
    }
}

/// DELETE /api/v1/profile/avatar — go back to the lettered tile.
async fn delete_avatar(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let previous =
        state.db.get_profile(user.user_id).await.ok().flatten().and_then(|p| p.avatar_url);
    if let Err(e) = state.db.update_profile(user.user_id, None, None, Some(None)).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response();
    }
    if let Some(old) = previous {
        if !old.starts_with("http") {
            let _ = tokio::fs::remove_file(state.uploads_dir.join(&old)).await;
        }
    }
    (StatusCode::OK, Json(serde_json::json!({ "status": "removed" }))).into_response()
}

// ═══════════════════════════════════════════════════════════
//  Routers
// ═══════════════════════════════════════════════════════════

/// Auth routes — unauthenticated (except /me).
pub fn create_auth_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/auth/status", get(auth_status))
        .route("/api/v1/auth/setup", post(setup))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
}

/// User + token management (authenticated).
pub fn create_user_management_router() -> Router<Arc<AppState>> {
    use axum::routing::patch;
    Router::new()
        .route("/admin/users", post(create_user).get(list_users))
        .route("/admin/users/:id", patch(update_user).delete(delete_user))
        .route("/admin/tokens", post(create_token).get(list_tokens))
        .route("/admin/tokens/:id", patch(update_token).delete(revoke_token))
        .route("/admin/tokens/:id/rotate", post(rotate_token))
        // Your own account. No id in the path — these are always the caller's.
        .route("/api/v1/profile", get(get_profile).patch(patch_profile))
        .route("/api/v1/profile/password", post(change_password))
        .route(
            "/api/v1/profile/avatar",
            get(serve_avatar).post(upload_avatar).delete(delete_avatar),
        )
        .route("/log-level", get(get_log_level).post(set_log_level))
}
