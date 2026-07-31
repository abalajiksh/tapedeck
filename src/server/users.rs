use axum::{
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;

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
    let needs_setup = state.db.needs_setup().await.unwrap_or(true);

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
    let token = match state.db.create_token(user_id, "default", "submit,read,admin").await {
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

    let token = match state.db.create_token(user_id, "default", "submit").await {
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
    match state.db.create_token(target_user_id, body.name.trim(), &body.scopes).await {
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
        .route("/admin/tokens/:id", delete(revoke_token))
        .route("/log-level", get(get_log_level).post(set_log_level))
}
