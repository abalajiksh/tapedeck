use axum::{
    extract::{Path, Query, State},
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
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
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

/// PATCH body — every field optional; only provided fields change.
#[derive(Debug, Deserialize)]
pub struct UpdateScrobble {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub mbid_recording: Option<String>,
    pub signal_chain_id: Option<i64>,
    pub listening_context: Option<String>,
    pub format_type: Option<String>,
    pub codec: Option<String>,
}

/// PATCH /api/v1/scrobbles/:id — edit a listen (fix MBID / chain / metadata).
async fn update_scrobble(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateScrobble>,
) -> impl IntoResponse {
    let updated = state.db.update_scrobble(
        user.user_id, id,
        body.title.as_deref(), body.artist.as_deref(), body.album.as_deref(),
        body.mbid_recording.as_deref(), body.signal_chain_id,
        body.listening_context.as_deref(), body.format_type.as_deref(), body.codec.as_deref(),
    ).await;
    match updated {
        Ok(true) => match state.db.get_all_scrobbles(user.user_id).await {
            // return the fresh row so the UI can re-render without a refetch
            Ok(rows) => match rows.into_iter().find(|r| r.id == id) {
                Some(r) => (StatusCode::OK, Json(serde_json::json!(r))).into_response(),
                None => (StatusCode::OK, Json(serde_json::json!({ "id": id, "status": "updated" }))).into_response(),
            },
            Err(_) => (StatusCode::OK, Json(serde_json::json!({ "id": id, "status": "updated" }))).into_response(),
        },
        Ok(false) => not_found(),
        Err(e) => server_error(e.to_string()),
    }
}

/// DELETE /api/v1/scrobbles/:id — remove one listen owned by the user.
async fn delete_scrobble(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_scrobble(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(e) => server_error(e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ClearQuery {
    pub confirm: Option<bool>,
}

/// DELETE /api/v1/scrobbles?confirm=true — clear ALL listens for the user.
async fn clear_scrobbles(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ClearQuery>,
) -> impl IntoResponse {
    if q.confirm != Some(true) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!(
            ErrorResponse { code: 400, error: "Refusing to clear without ?confirm=true".into() }
        ))).into_response();
    }
    match state.db.clear_scrobbles(user.user_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub range: Option<String>,
}

/// GET /api/v1/stats?range=week|month|last30|year|all — ranged analytics.
async fn ranged_stats(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<StatsQuery>,
) -> impl IntoResponse {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let after = match q.range.as_deref().unwrap_or("all") {
        "week" => Some(now - 7 * 86400),
        "month" => Some(now - 30 * 86400),
        "last30" => Some(now - 30 * 86400),
        "year" => Some(now - 365 * 86400),
        _ => None, // "all"
    };
    match state.db.get_ranged_stats(user.user_id, after).await {
        Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

fn not_found() -> axum::response::Response {
    (StatusCode::NOT_FOUND, Json(serde_json::json!(
        ErrorResponse { code: 404, error: "Scrobble not found".into() }
    ))).into_response()
}
fn server_error(msg: String) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
        ErrorResponse { code: 500, error: msg }
    ))).into_response()
}

pub fn create_scrobbles_router() -> Router<Arc<AppState>> {
    use axum::routing::patch;
    Router::new()
        .route("/api/v1/scrobbles", get(list_scrobbles).delete(clear_scrobbles))
        .route("/api/v1/scrobbles/:id", patch(update_scrobble).delete(delete_scrobble))
        .route("/api/v1/stats/dashboard", get(dashboard_stats))
        .route("/api/v1/stats", get(ranged_stats))
}
