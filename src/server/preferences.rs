use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

/// GET /api/v1/preferences → the session user's stored settings object.
async fn get_preferences(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.db.get_preferences(user.user_id).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: e.to_string() }
        ))).into_response(),
    }
}

/// PUT /api/v1/preferences — shallow-merges the body into stored settings
/// (e.g. `{ "theme": "dark" }`). Returns the merged object.
async fn put_preferences(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !body.is_object() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!(
            ErrorResponse { code: 400, error: "Body must be a JSON object".into() }
        ))).into_response();
    }
    match state.db.merge_preferences(user.user_id, &body).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: e.to_string() }
        ))).into_response(),
    }
}

pub fn create_preferences_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/preferences", get(get_preferences).put(put_preferences))
}
