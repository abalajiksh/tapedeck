//! Listening sessions as something you can look at and edit.
//!
//! `assign_session` has grouped listens since Phase 2 — contiguous listening
//! with gaps under 30 minutes, broken also by a change of submitting token —
//! but nothing ever read the result. These endpoints expose what was already
//! being written, and add the two edits a heuristic can't get right on its own:
//!
//! * **Merge**, when one sitting was split by a long pause, a phone call, or
//!   swapping apps mid-record.
//! * **Split**, when two unrelated sittings ran together because you never
//!   stopped for thirty minutes.
//!
//! Those edits are the reason the grouping stays **stored** rather than being
//! recomputed at read time from a configurable gap: a manual merge is a decision
//! no heuristic can re-derive, so `scrobbles.session_id` is the source of truth
//! and these rewrite it.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

fn err(code: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (code, Json(serde_json::json!(ErrorResponse { code: code.as_u16(), error: msg.into() })))
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// GET /api/v1/sessions
async fn list_sessions(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    match state.db.list_sessions(user.user_id, limit, offset).await {
        Ok(sessions) => (
            StatusCode::OK,
            Json(serde_json::json!({ "sessions": sessions, "count": sessions.len() })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// GET /api/v1/sessions/:id — the session plus the listens in it, in play order.
async fn get_session(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let session = match state.db.get_session(user.user_id, id).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(StatusCode::NOT_FOUND, "Session not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let scrobbles = state.db.session_scrobbles(user.user_id, id).await.unwrap_or_default();
    (StatusCode::OK, Json(serde_json::json!({ "session": session, "scrobbles": scrobbles })))
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    /// Two or more session ids. The earliest survives.
    pub ids: Vec<i64>,
}

/// POST /api/v1/sessions/merge
async fn merge(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<MergeRequest>,
) -> impl IntoResponse {
    if body.ids.len() < 2 {
        return err(StatusCode::BAD_REQUEST, "Merging takes at least two sessions");
    }
    match state.db.merge_sessions(user.user_id, &body.ids).await {
        // `None` means one of the ids wasn't the caller's — nothing was moved.
        Ok(None) => err(StatusCode::BAD_REQUEST, "Those sessions can't be merged"),
        Ok(Some(id)) => match state.db.get_session(user.user_id, id).await {
            Ok(Some(s)) => {
                (StatusCode::OK, Json(serde_json::json!({ "session": s }))).into_response()
            }
            _ => (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response(),
        },
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct SplitRequest {
    /// The listen that starts the new session.
    pub at_scrobble_id: i64,
}

/// POST /api/v1/sessions/:id/split
async fn split(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<SplitRequest>,
) -> impl IntoResponse {
    match state.db.split_session(user.user_id, id, body.at_scrobble_id).await {
        Ok(None) => err(
            StatusCode::BAD_REQUEST,
            "Nothing to split there — that listen isn't in this session, or it's already the first",
        ),
        Ok(Some(new_id)) => {
            let new = state.db.get_session(user.user_id, new_id).await.ok().flatten();
            let old = state.db.get_session(user.user_id, id).await.ok().flatten();
            (StatusCode::OK, Json(serde_json::json!({ "session": old, "new_session": new })))
                .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateSession {
    /// Present-and-null clears the chain; absent leaves it alone.
    #[serde(default, deserialize_with = "double_option")]
    pub chain_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option_str")]
    pub title: Option<Option<String>>,
}

fn double_option<'de, D>(de: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i64>::deserialize(de).map(Some)
}

fn double_option_str<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(de).map(Some)
}

/// PATCH /api/v1/sessions/:id — retag the whole sitting, or name it.
async fn update_session(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateSession>,
) -> impl IntoResponse {
    if state.db.get_session(user.user_id, id).await.ok().flatten().is_none() {
        return err(StatusCode::NOT_FOUND, "Session not found");
    }

    if let Some(chain) = body.chain_id {
        // A chain id from a request body is user input — check it's theirs.
        if let Some(cid) = chain {
            match state.db.get_chain(user.user_id, cid).await {
                Ok(Some(_)) => {}
                _ => return err(StatusCode::BAD_REQUEST, "Unknown signal chain"),
            }
        }
        if let Err(e) = state.db.set_session_chain(user.user_id, id, chain).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    if let Some(title) = body.title {
        if let Err(e) = state.db.set_session_title(user.user_id, id, title.as_deref()).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    match state.db.get_session(user.user_id, id).await {
        Ok(Some(s)) => (StatusCode::OK, Json(serde_json::json!({ "session": s }))).into_response(),
        _ => err(StatusCode::NOT_FOUND, "Session not found"),
    }
}

#[derive(Debug, Deserialize)]
pub struct RebuildRequest {
    /// Gap in seconds that starts a new session. Defaults to the 30 minutes the
    /// live assigner uses; exposed because that constant is a guess and a
    /// rebuild is the only place it can be reconsidered.
    pub gap_seconds: Option<i64>,
}

/// POST /api/v1/sessions/rebuild — regroup every listen from scratch.
///
/// **Discards manual merges and splits**, because they live in the same column
/// this rewrites. It exists because sessions were never linked to their listens
/// until now, and because an imported history bypasses the ingest path that
/// would have grouped it.
async fn rebuild(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<RebuildRequest>,
) -> impl IntoResponse {
    let gap = body.gap_seconds.unwrap_or(1800).clamp(60, 24 * 3600);
    match state.db.rebuild_sessions(user.user_id, gap).await {
        Ok((sessions, listens)) => {
            (StatusCode::OK, Json(serde_json::json!({ "sessions": sessions, "listens": listens })))
                .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub fn create_sessions_router() -> Router<Arc<AppState>> {
    Router::new()
        // `merge` is registered before `/:id` — matchit prefers a static
        // sibling, but the order is kept explicit the way `sources.rs` does it.
        .route("/api/v1/sessions/merge", post(merge))
        .route("/api/v1/sessions/rebuild", post(rebuild))
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/sessions/:id", get(get_session).patch(update_session))
        .route("/api/v1/sessions/:id/split", post(split))
}
