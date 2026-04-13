use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

#[derive(Debug, Deserialize)]
pub struct ScrobblesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub after: Option<i64>,
    pub before: Option<i64>,
}

/// GET /api/v1/scrobbles
async fn list_scrobbles(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(params): Query<ScrobblesQuery>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(500).max(1);
    let offset = params.offset.unwrap_or(0).max(0);

    match state.db.get_recent_scrobbles(
        user.user_id, limit, offset,
        params.artist.as_deref(), params.album.as_deref(),
        params.after, params.before,
    ).await {
        Ok(scrobbles) => {
            let count = scrobbles.len();
            (StatusCode::OK, Json(serde_json::json!({
                "scrobbles": scrobbles, "count": count, "limit": limit, "offset": offset,
            }))).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: format!("Failed to fetch scrobbles: {}", e) }
        ))).into_response(),
    }
}

/// GET /api/v1/stats/dashboard
async fn dashboard_stats(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.db.get_dashboard_stats(user.user_id).await {
        Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: format!("Failed to compute stats: {}", e) }
        ))).into_response(),
    }
}

pub fn create_scrobbles_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/scrobbles", get(list_scrobbles))
        .route("/api/v1/stats/dashboard", get(dashboard_stats))
}
