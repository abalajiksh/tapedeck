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
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
pub async fn list_sessions(
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
    /// Which variant of the chain — "with the microlinear", "on the Hexas".
    /// Rewritten whenever `chain_id` is, since a variant belongs to one chain.
    #[serde(default, deserialize_with = "double_option")]
    pub chain_variant_id: Option<Option<i64>>,
    /// A note about the sitting. Not a liner note about a record — that is
    /// `/api/v1/notes`, which hangs off an entity or a single listen.
    #[serde(default, deserialize_with = "double_option_str")]
    pub notes: Option<Option<String>>,
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
        // A variant has to belong to the chain it is being paired with, or the
        // listen resolves to components that were never fitted together.
        let variant = body.chain_variant_id.flatten();
        if let (Some(cid), Some(vid)) = (chain, variant) {
            match state.db.variant_fits_chain(user.user_id, vid, cid).await {
                Ok(true) => {}
                Ok(false) => {
                    return err(StatusCode::BAD_REQUEST, "That variant belongs to another chain")
                }
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
        if variant.is_some() && chain.is_none() {
            return err(StatusCode::BAD_REQUEST, "A variant needs a chain");
        }
        if let Err(e) = state.db.set_session_chain(user.user_id, id, chain, variant).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    if let Some(notes) = body.notes {
        if let Err(e) = state.db.set_session_notes(user.user_id, id, notes.as_deref()).await {
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

// ── Rebuilding in the background ──
//
// A rebuild rewrites `session_id` on every listen the user has, inside one
// transaction so a crash cannot leave a history with no sessions at all. On the
// real host that ran past **100 seconds**, which is Cloudflare's origin timeout:
// the browser got a 524 while the rebuild carried on unseen, which is worse
// than failing — a destructive regroup half-visible, with no way to tell
// whether it finished.
//
// So the request starts the work and returns an id, and the page polls. Same
// shape as an import, and for the same reason.
//
// The registry is in memory and a running job is lost on restart, exactly like
// an import's. That is acceptable here for the reason it is there: the *work*
// is one transaction, so a process that dies mid-rebuild rolls back rather than
// leaving half a regrouping behind. Only the progress report is lost.

/// One rebuild, as it runs.
#[derive(Clone, Serialize)]
pub struct RebuildJob {
    pub job_id: String,
    #[serde(skip)]
    pub user_id: i64,
    /// `running` | `done` | `error`
    pub state: String,
    /// Sessions written. Only meaningful once `state` is `done`.
    pub sessions: u64,
    pub listens: u64,
    pub status: String,
    pub started_at: i64,
}

pub type RebuildRegistry = Arc<Mutex<HashMap<String, RebuildJob>>>;

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
        as i64
}

/// POST /api/v1/sessions/rebuild — start a regroup and return its id.
///
/// **Discards manual merges and splits**, because they live in the same column
/// this rewrites. It exists because sessions were never linked to their listens
/// until v0.42, and because an imported history bypasses the ingest path that
/// would have grouped it.
///
/// Answers **202** with a `job_id` rather than doing the work inline; poll
/// `/api/v1/sessions/rebuild/:job_id` for the outcome.
async fn rebuild(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<RebuildRequest>,
) -> impl IntoResponse {
    let gap = body.gap_seconds.unwrap_or(1800).clamp(60, 24 * 3600);

    let job_id = {
        use rand::Rng;
        hex::encode(rand::rng().random::<[u8; 8]>())
    };

    // One at a time per user, decided under a **single** lock acquisition.
    //
    // Checking and then inserting in two separate acquisitions is a race, and
    // not a theoretical one: a rebuild here runs for minutes, so the way to hit
    // it is to click the button twice. Two concurrent rebuilds would each clear
    // the other's work halfway through, and this is destructive — the column it
    // rewrites is where manual merges and splits live.
    //
    // Reported as 409 carrying the running job's id, so the page attaches to
    // the one already going rather than reporting a failure for work that is
    // very much in progress.
    let claimed = {
        let Ok(mut m) = state.rebuilds.lock() else {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the rebuild registry");
        };
        match m.values().find(|j| j.user_id == user.user_id && j.state == "running") {
            Some(running) => Err(running.job_id.clone()),
            None => {
                // Finished jobs of this user's are dropped as a new one starts,
                // so the registry cannot grow without bound.
                m.retain(|_, j| j.state == "running" || j.user_id != user.user_id);
                m.insert(
                    job_id.clone(),
                    RebuildJob {
                        job_id: job_id.clone(),
                        user_id: user.user_id,
                        state: "running".into(),
                        sessions: 0,
                        listens: 0,
                        status: "Regrouping your listens…".into(),
                        started_at: now_secs(),
                    },
                );
                Ok(())
            }
        }
    };
    if let Err(running) = claimed {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "code": 409,
                "error": "A rebuild is already running",
                "job_id": running,
            })),
        )
            .into_response();
    }

    let (db, reg, uid, id) =
        (state.db.clone(), state.rebuilds.clone(), user.user_id, job_id.clone());
    tokio::spawn(async move {
        let outcome = db.rebuild_sessions(uid, gap).await;
        if let Ok(mut m) = reg.lock() {
            if let Some(j) = m.get_mut(&id) {
                match outcome {
                    Ok((sessions, listens)) => {
                        j.state = "done".into();
                        j.sessions = sessions;
                        j.listens = listens;
                        j.status = format!(
                            "Regrouped {listens} listen{} into {sessions} session{}",
                            if listens == 1 { "" } else { "s" },
                            if sessions == 1 { "" } else { "s" }
                        );
                    }
                    Err(e) => {
                        j.state = "error".into();
                        j.status = e.to_string();
                    }
                }
            }
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({ "job_id": job_id, "state": "running" })))
        .into_response()
}

/// GET /api/v1/sessions/rebuild/:job_id — how the regroup is going.
///
/// Scoped to the caller: a job id is short and this is the only thing that
/// stops one household member watching another's. An unknown id and someone
/// else's are the same 404, so polling cannot be used to learn that a job
/// exists.
async fn rebuild_status(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let job = state
        .rebuilds
        .lock()
        .ok()
        .and_then(|m| m.get(&job_id).cloned())
        .filter(|j| j.user_id == user.user_id);
    match job {
        Some(j) => (StatusCode::OK, Json(serde_json::json!(j))).into_response(),
        // Also what a restart mid-rebuild looks like, which is why the message
        // says so rather than only "not found".
        None => err(
            StatusCode::NOT_FOUND,
            "No such rebuild — it may have finished before this process restarted",
        ),
    }
}

pub fn create_sessions_router() -> Router<Arc<AppState>> {
    Router::new()
        // `merge` is registered before `/:id` — matchit prefers a static
        // sibling, but the order is kept explicit the way `sources.rs` does it.
        .route("/api/v1/sessions/merge", post(merge))
        .route("/api/v1/sessions/rebuild", post(rebuild))
        // A literal segment deeper than "/:id", so matchit tells them apart.
        .route("/api/v1/sessions/rebuild/:job_id", get(rebuild_status))
        .route("/api/v1/sessions", get(list_sessions))
        .route("/api/v1/sessions/:id", get(get_session).patch(update_session))
        .route("/api/v1/sessions/:id/split", post(split))
}
