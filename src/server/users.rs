use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

// ── Request / Response types ──

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub display_name: Option<String>,
}

#[derive(Serialize)]
pub struct CreateUserResponse {
    pub user_id: i64,
    pub username: String,
    pub token: String,
    pub message: String,
}

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    /// User ID to create the token for. If omitted, creates for the requesting user.
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

// ── Handlers ──

/// POST /admin/users — Create a new user.
///
/// Returns the new user's ID and a ready-to-use API token.
async fn create_user(
    State(state): State<Arc<AppState>>,
    _user: AuthUser, // must be authenticated
    Json(body): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if body.username.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Username cannot be empty".into(),
            })),
        )
            .into_response();
    }

    let user_id = match state
        .db
        .create_user(body.username.trim(), body.display_name.as_deref(), "not-used-yet")
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
            return (status, Json(serde_json::json!(ErrorResponse { code: status.as_u16(), error }))).into_response();
        }
    };

    // Auto-create a default token so the new user can immediately start scrobbling
    let token = match state.db.create_token(user_id, "default", "submit").await {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    code: 500,
                    error: format!("User created but token generation failed: {}", e),
                })),
            )
                .into_response();
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

/// GET /admin/users — List all users.
async fn list_users(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> impl IntoResponse {
    match state.db.list_users().await {
        Ok(users) => (StatusCode::OK, Json(serde_json::json!({ "users": users }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                code: 500,
                error: format!("Failed to list users: {}", e),
            })),
        )
            .into_response(),
    }
}

/// POST /admin/tokens — Create a new API token.
async fn create_token(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<CreateTokenRequest>,
) -> impl IntoResponse {
    let target_user_id = body.user_id.unwrap_or(user.user_id);

    if body.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Token name cannot be empty".into(),
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
                error: format!("Failed to create token: {}", e),
            })),
        )
            .into_response(),
    }
}

/// GET /admin/tokens — List tokens for the authenticated user.
async fn list_tokens(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.db.list_tokens(user.user_id).await {
        Ok(tokens) => (StatusCode::OK, Json(serde_json::json!({ "tokens": tokens }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                code: 500,
                error: format!("Failed to list tokens: {}", e),
            })),
        )
            .into_response(),
    }
}

/// User/token management router.
pub fn create_user_management_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/users", post(create_user).get(list_users))
        .route("/admin/tokens", post(create_token).get(list_tokens))
}
