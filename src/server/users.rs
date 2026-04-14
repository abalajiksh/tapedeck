use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::auth::{extract_session_cookie, AuthUser};
use super::models::ErrorResponse;
use super::AppState;
use crate::db::Database;

// ═══════════════════════════════════════════════════════════
//  Cookie helpers
// ═══════════════════════════════════════════════════════════

fn session_set_cookie(token: &str) -> HeaderValue {
    format!("td_session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=604800", token)
        .parse().unwrap()
}

fn session_clear_cookie() -> HeaderValue {
    "td_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0".parse().unwrap()
}

// ═══════════════════════════════════════════════════════════
//  Auth endpoints (mostly unauthenticated)
// ═══════════════════════════════════════════════════════════

/// GET /api/v1/auth/status — unauthenticated, returns setup + auth state
async fn auth_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let needs_setup = state.db.needs_setup().await.unwrap_or(true);

    let authenticated = if let Some(session_token) = headers.get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|c| c.split(';').find_map(|s| s.trim().strip_prefix("td_session=").map(|v| v.to_string())))
    {
        state.db.validate_session(&session_token).await.ok().flatten().is_some()
    } else {
        false
    };

    Json(serde_json::json!({ "needs_setup": needs_setup, "authenticated": authenticated }))
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
    Json(body): Json<SetupRequest>,
) -> impl IntoResponse {
    // Guard: only works if no user has a real password yet
    match state.db.needs_setup().await {
        Ok(true) => {}
        Ok(false) => return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse { code: 403, error: "Setup already completed".into() })),
        ).into_response(),
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        ).into_response(),
    }

    if body.username.trim().is_empty() || body.password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400, error: "Username required, password must be at least 8 characters".into(),
            })),
        ).into_response();
    }

    let password_hash = match Database::hash_password(&body.password) {
        Ok(h) => h,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e })),
        ).into_response(),
    };

    let user_id = match state.db.setup_admin(
        body.username.trim(),
        body.display_name.as_deref(),
        &password_hash,
    ).await {
        Ok(id) => id,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        ).into_response(),
    };

    // Create a default API token
    let token = match state.db.create_token(user_id, "default", "submit,read,admin").await {
        Ok(t) => t,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: format!("Admin created but token failed: {}", e) })),
        ).into_response(),
    };

    // Log them in (create session)
    let session_token = match state.db.create_web_session(user_id).await {
        Ok(t) => t,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        ).into_response(),
    };

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(header::SET_COOKIE, session_set_cookie(&session_token));

    (StatusCode::OK, resp_headers, Json(serde_json::json!({
        "user_id": user_id,
        "username": body.username.trim(),
        "token": token,
        "message": "Admin account created. Save the API token — it won't be shown again.",
    }))).into_response()
}

/// POST /api/v1/auth/login
#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> impl IntoResponse {
    let user = match state.db.authenticate(&body.username, &body.password).await {
        Ok(Some(u)) => u,
        Ok(None) => return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!(ErrorResponse { code: 401, error: "Invalid username or password".into() })),
        ).into_response(),
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        ).into_response(),
    };

    let session_token = match state.db.create_web_session(user.user_id).await {
        Ok(t) => t,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        ).into_response(),
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, session_set_cookie(&session_token));

    (StatusCode::OK, headers, Json(serde_json::json!({
        "user_id": user.user_id,
        "username": user.username,
        "is_admin": user.is_admin,
    }))).into_response()
}

/// POST /api/v1/auth/logout — reads cookie, no AuthUser required
async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Try to delete the session from DB
    if let Some(session_token) = headers.get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|c| c.split(';').find_map(|s| s.trim().strip_prefix("td_session=").map(|v| v.to_string())))
    {
        let _ = state.db.delete_web_session(&session_token).await;
    }

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(header::SET_COOKIE, session_clear_cookie());

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
    _user: AuthUser,
    Json(body): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if body.username.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!(
            ErrorResponse { code: 400, error: "Username cannot be empty".into() }
        ))).into_response();
    }

    let password_hash = if let Some(ref pw) = body.password {
        match Database::hash_password(pw) {
            Ok(h) => h,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
                ErrorResponse { code: 500, error: e }
            ))).into_response(),
        }
    } else {
        "not-used-yet".to_string()
    };

    let user_id = match state.db.create_user(
        body.username.trim(), body.display_name.as_deref(), &password_hash, "user",
    ).await {
        Ok(id) => id,
        Err(e) => {
            let msg = e.to_string();
            let (status, error) = if msg.contains("UNIQUE") {
                (StatusCode::CONFLICT, format!("Username '{}' already exists", body.username))
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to create user: {}", msg))
            };
            return (status, Json(serde_json::json!(ErrorResponse { code: status.as_u16(), error }))).into_response();
        }
    };

    let token = match state.db.create_token(user_id, "default", "submit").await {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: format!("User created but token failed: {}", e) }
        ))).into_response(),
    };

    (StatusCode::CREATED, Json(serde_json::json!(CreateUserResponse {
        user_id, username: body.username.trim().to_string(), token,
        message: "User created. Save the token — it won't be shown again.".into(),
    }))).into_response()
}

async fn list_users(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> impl IntoResponse {
    match state.db.list_users().await {
        Ok(users) => (StatusCode::OK, Json(serde_json::json!({ "users": users }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: format!("Failed to list users: {}", e) }
        ))).into_response(),
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

fn default_scopes() -> String { "submit".into() }

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
    let target_user_id = body.user_id.unwrap_or(user.user_id);
    if body.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!(
            ErrorResponse { code: 400, error: "Token name cannot be empty".into() }
        ))).into_response();
    }
    match state.db.create_token(target_user_id, body.name.trim(), &body.scopes).await {
        Ok(token) => (StatusCode::CREATED, Json(serde_json::json!(CreateTokenResponse {
            token, name: body.name.trim().to_string(), user_id: target_user_id,
            message: "Token created. Save it — it won't be shown again.".into(),
        }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: format!("Failed to create token: {}", e) }
        ))).into_response(),
    }
}

async fn list_tokens(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.db.list_tokens(user.user_id).await {
        Ok(tokens) => (StatusCode::OK, Json(serde_json::json!({ "tokens": tokens }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: format!("Failed to list tokens: {}", e) }
        ))).into_response(),
    }
}

async fn revoke_token(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(token_id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_token(user.user_id, token_id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "status": "revoked" }))).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(serde_json::json!(
            ErrorResponse { code: 404, error: "Token not found".into() }
        ))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: e.to_string() }
        ))).into_response(),
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
    Router::new()
        .route("/admin/users", post(create_user).get(list_users))
        .route("/admin/tokens", post(create_token).get(list_tokens))
        .route("/admin/tokens/{id}", delete(revoke_token))
}
