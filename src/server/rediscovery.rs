//! Rediscovery: what you already chose and are not choosing now.
//!
//! Every list here is built from the user's own history. Nothing recommends
//! music they don't own — that needs a corpus this app deliberately doesn't
//! have, and a self-hosted journal suggesting things to go and buy is the wrong
//! shape. What it can do is notice what has slipped.
//!
//! The lists are served together because they are cheap and the page shows all
//! of them at once; the one exception is the per-gear list, which needs to be
//! told which piece of gear.

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

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Debug, Deserialize)]
pub struct RediscoverQuery {
    /// How long is "not lately", in days. Defaults to six months — long enough
    /// that a gap is meaningful rather than just a busy fortnight.
    pub quiet_days: Option<i64>,
    pub limit: Option<i64>,
}

/// GET /api/v1/rediscovery
async fn rediscover(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<RediscoverQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(12).clamp(1, 50);
    let quiet_days = q.quiet_days.unwrap_or(180).clamp(7, 3650);
    let quiet_since = now() - quiet_days * 86_400;
    // A one-listen record needs longer to count as forgotten than a favourite
    // does — you haven't had the chance to come back to it yet.
    let heard_once_before = now() - (quiet_days.max(90)) * 86_400;

    let (forgotten, loved, once, seasonal) = tokio::join!(
        state.db.forgotten_favourites(user.user_id, quiet_since, limit),
        state.db.loved_but_unplayed(user.user_id, limit),
        state.db.heard_once(user.user_id, heard_once_before, limit),
        state.db.seasonal(user.user_id, limit),
    );

    // Gear the user owns, so the page can offer the per-gear list without a
    // second round trip just to populate a dropdown.
    let gear = state.db.get_equipment(user.user_id).await.unwrap_or_default();
    let gear_json: Vec<serde_json::Value> = gear
        .iter()
        .map(|g| serde_json::json!({ "id": g.id, "name": g.name, "type": g.equipment_type }))
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "quiet_days": quiet_days,
            "forgotten_favourites": forgotten.unwrap_or_default(),
            "loved_but_unplayed": loved.unwrap_or_default(),
            "heard_once": once.unwrap_or_default(),
            "seasonal": seasonal.unwrap_or_default(),
            "gear": gear_json,
        })),
    )
        .into_response()
}

/// GET /api/v1/rediscovery/gear/:id — favourites this gear has never played.
async fn never_on_gear(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<RediscoverQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(12).clamp(1, 50);

    // Scoped before the query rather than trusting the id: equipment is
    // per-user, and the list would otherwise describe someone else's shelf.
    match state.db.get_equipment(user.user_id).await {
        Ok(list) if list.iter().any(|g| g.id == id) => {}
        Ok(_) => return err(404, "No such gear"),
        Err(e) => return err(500, e.to_string()),
    }

    match state.db.never_on_gear(user.user_id, id, limit).await {
        Ok(rows) => (StatusCode::OK, Json(serde_json::json!({ "rows": rows }))).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

pub fn create_rediscovery_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/rediscovery", get(rediscover))
        .route("/api/v1/rediscovery/gear/:id", get(never_on_gear))
}
