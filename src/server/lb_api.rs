//! ListenBrainz-compatible read endpoints under `/1/`.
//!
//! `submit-listens` and `validate-token` live in `ingest.rs`; this module covers
//! the rest of the Core API that a scrobble client actually exercises, so a
//! client pointed at Tapedeck doesn't 404 partway through its setup or sync.
//!
//! Two deliberate deviations from listenbrainz.org:
//!
//! * **These require authentication.** On ListenBrainz a user's listens are
//!   public, so `/1/user/{name}/listens` needs no token. A self-hosted Tapedeck
//!   is usually private and often reachable from the internet, so serving a
//!   full listen history to anyone who guesses a username would be a privacy
//!   regression. A token or session is required, and you may only read your own
//!   data unless you're an admin. Real clients always send their token and ask
//!   for their own username, so this costs nothing in practice.
//! * **No MSIDs.** ListenBrainz assigns every submission a `recording_msid`;
//!   Tapedeck keys listens by `(user, source_id, source_name)` and has no
//!   equivalent, so MSID fields are absent and `/1/delete-listen` (which
//!   identifies a listen by MSID) is not implemented — deleting goes through
//!   `DELETE /api/v1/scrobbles/:id` instead.
//!
//! Not implemented, because Tapedeck has no such concept and a stub would be
//! worse than a clean 404: playlists, `lb-radio`, similar-users/similar-to, and
//! `search/users` (which would also be a user-enumeration hole here).

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::auth::IngestAuth;
use super::AppState;
use crate::db::ScrobbleRecord;

fn error(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!({ "code": code, "error": msg.into() }))).into_response()
}

/// Resolve the `{user}` path segment, enforcing that it's the caller (or that
/// the caller is an admin). Returns the target user's id on success.
async fn authorize_for(
    state: &Arc<AppState>,
    caller: &crate::models::AuthUser,
    requested: &str,
) -> Result<i64, axum::response::Response> {
    if requested.eq_ignore_ascii_case(&caller.username) {
        return Ok(caller.user_id);
    }
    if !caller.is_admin {
        // Same answer whether or not the user exists — don't leak the roster.
        return Err(error(403, "You may only read your own listens"));
    }
    match state.db.get_user_id_by_username(requested).await {
        Ok(Some(id)) => Ok(id),
        Ok(None) => Err(error(404, format!("Cannot find user: {requested}"))),
        Err(e) => Err(error(500, e.to_string())),
    }
}

/// A stored listen in ListenBrainz's shape. Tapedeck's audio-quality extensions
/// ride along in `additional_info` under the same keys the ingest API accepts,
/// so a listen submitted to Tapedeck reads back the way it went in.
fn to_lb_listen(s: &ScrobbleRecord, include_listened_at: bool) -> serde_json::Value {
    let mut additional = serde_json::Map::new();
    if let Some(d) = s.duration {
        additional.insert("duration_ms".into(), serde_json::json!(d * 1000));
    }
    if let Some(ref m) = s.mbid_recording {
        additional.insert("recording_mbid".into(), serde_json::json!(m));
    }
    if let Some(ref m) = s.mbid_release {
        additional.insert("release_mbid".into(), serde_json::json!(m));
    }
    if let Some(ref m) = s.mbid_artist {
        // Stored as a JSON array string; pass it through as an array when it parses.
        match serde_json::from_str::<serde_json::Value>(m) {
            Ok(v) if v.is_array() => {
                additional.insert("artist_mbids".into(), v);
            }
            _ => {
                additional.insert("artist_mbids".into(), serde_json::json!([m]));
            }
        }
    }
    if let Some(ref c) = s.submission_client {
        additional.insert("submission_client".into(), serde_json::json!(c));
    }
    if let Some(n) = s.track_number {
        additional.insert("track_number".into(), serde_json::json!(n));
    }

    let mut audio = serde_json::Map::new();
    if let Some(ref v) = s.format_type {
        audio.insert("format_type".into(), serde_json::json!(v));
    }
    if let Some(ref v) = s.codec {
        audio.insert("codec".into(), serde_json::json!(v));
    }
    if let Some(v) = s.bitrate {
        audio.insert("bitrate".into(), serde_json::json!(v));
    }
    if let Some(v) = s.sample_rate {
        audio.insert("sample_rate".into(), serde_json::json!(v));
    }
    if let Some(v) = s.bit_depth {
        audio.insert("bit_depth".into(), serde_json::json!(v));
    }
    if let Some(v) = s.is_lossless {
        audio.insert("is_lossless".into(), serde_json::json!(v));
    }
    if let Some(v) = s.dsd_multiplier {
        audio.insert("dsd_multiplier".into(), serde_json::json!(v));
    }
    if let Some(ref v) = s.delivery_codec {
        audio.insert("delivery_codec".into(), serde_json::json!(v));
    }
    if !audio.is_empty() {
        additional.insert("tapedeck_audio".into(), serde_json::Value::Object(audio));
    }

    let mut listen = serde_json::Map::new();
    if include_listened_at {
        listen.insert("listened_at".into(), serde_json::json!(s.timestamp));
    }
    listen.insert(
        "track_metadata".into(),
        serde_json::json!({
            "artist_name": s.artist,
            "track_name": s.title,
            "release_name": s.album,
            "additional_info": serde_json::Value::Object(additional),
        }),
    );
    serde_json::Value::Object(listen)
}

#[derive(Deserialize)]
pub struct ListensQuery {
    max_ts: Option<i64>,
    min_ts: Option<i64>,
    count: Option<i64>,
}

/// GET /1/user/:user/listens
async fn user_listens(
    State(state): State<Arc<AppState>>,
    IngestAuth { user: caller, .. }: IngestAuth,
    Path(user): Path<String>,
    Query(q): Query<ListensQuery>,
) -> impl IntoResponse {
    let user_id = match authorize_for(&state, &caller, &user).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    if q.max_ts.is_some() && q.min_ts.is_some() {
        return error(400, "max_ts and min_ts cannot be used together");
    }
    // LB's bounds are strict (< max_ts, > min_ts); ours are inclusive.
    let count = q.count.unwrap_or(25).clamp(1, 1000);
    let before = q.max_ts.map(|t| t - 1);
    let after = q.min_ts.map(|t| t + 1);

    let rows = match state
        .db
        .get_recent_scrobbles(user_id, count, 0, None, None, None, after, before)
        .await
    {
        Ok(r) => r,
        Err(e) => return error(500, e.to_string()),
    };
    let (_, latest) = state.db.listen_count_and_latest(user_id).await.unwrap_or((0, None));
    let listens: Vec<serde_json::Value> = rows.iter().map(|s| to_lb_listen(s, true)).collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "payload": {
                "count": listens.len(),
                "user_id": user,
                "listens": listens,
                "latest_listen_ts": latest.unwrap_or(0),
            }
        })),
    )
        .into_response()
}

/// GET /1/user/:user/playing-now
async fn user_playing_now(
    State(state): State<Arc<AppState>>,
    IngestAuth { user: caller, .. }: IngestAuth,
    Path(user): Path<String>,
) -> impl IntoResponse {
    let user_id = match authorize_for(&state, &caller, &user).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    // A now-playing listen carries no `listened_at` — that's how clients tell
    // it apart from history.
    let listens: Vec<serde_json::Value> = match state.now_playing.get(user_id) {
        Some((np, _)) => {
            let mut additional = serde_json::Map::new();
            if let Some(d) = np.duration {
                additional.insert("duration_ms".into(), serde_json::json!(d * 1000));
            }
            vec![serde_json::json!({
                "track_metadata": {
                    "artist_name": np.artist,
                    "track_name": np.title,
                    "release_name": np.album,
                    "additional_info": serde_json::Value::Object(additional),
                },
                "playing_now": true,
            })]
        }
        None => Vec::new(),
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "payload": {
                "count": listens.len(),
                "user_id": user,
                "listens": listens,
                "playing_now": true,
            }
        })),
    )
        .into_response()
}

/// GET /1/user/:user/listen-count
async fn user_listen_count(
    State(state): State<Arc<AppState>>,
    IngestAuth { user: caller, .. }: IngestAuth,
    Path(user): Path<String>,
) -> impl IntoResponse {
    let user_id = match authorize_for(&state, &caller, &user).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.db.listen_count_and_latest(user_id).await {
        Ok((count, _)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "payload": { "count": count }
            })),
        )
            .into_response(),
        Err(e) => error(500, e.to_string()),
    }
}

/// GET /1/user/:user/services — which scrobble services this user has connected.
async fn user_services(
    State(state): State<Arc<AppState>>,
    IngestAuth { user: caller, .. }: IngestAuth,
    Path(user): Path<String>,
) -> impl IntoResponse {
    let user_id = match authorize_for(&state, &caller, &user).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.db.list_connections(user_id).await {
        Ok(rows) => {
            let services: Vec<String> = rows.into_iter().map(|(s, _, _)| s).collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "user_name": user, "services": services,
                })),
            )
                .into_response()
        }
        Err(e) => error(500, e.to_string()),
    }
}

/// POST /1/playing-now/delete — stop reporting this user as playing.
async fn playing_now_delete(
    State(state): State<Arc<AppState>>,
    IngestAuth { user: caller, .. }: IngestAuth,
) -> impl IntoResponse {
    state.now_playing.clear_user(caller.user_id);
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
}

// ── latest-import ──
//
// Importers use this as a bookmark: read the timestamp, fetch everything newer,
// write the new high-water mark back. Stored in the user's settings blob rather
// than its own table — it's one integer per user.

#[derive(Deserialize)]
pub struct LatestImportQuery {
    user_name: Option<String>,
}

/// GET /1/latest-import?user_name=…
async fn get_latest_import(
    State(state): State<Arc<AppState>>,
    IngestAuth { user: caller, .. }: IngestAuth,
    Query(q): Query<LatestImportQuery>,
) -> impl IntoResponse {
    let user = q.user_name.unwrap_or_else(|| caller.username.clone());
    let user_id = match authorize_for(&state, &caller, &user).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    let ts = state
        .db
        .get_preferences(user_id)
        .await
        .ok()
        .and_then(|p| p.get("latest_import").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "musicbrainz_id": user, "latest_import": ts,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct LatestImportBody {
    ts: i64,
}

/// POST /1/latest-import — body `{"ts": <unix seconds>}`, always the caller's own.
async fn set_latest_import(
    State(state): State<Arc<AppState>>,
    IngestAuth { user: caller, .. }: IngestAuth,
    Json(body): Json<LatestImportBody>,
) -> impl IntoResponse {
    if body.ts < 0 {
        return error(400, "ts must be a positive integer");
    }
    match state
        .db
        .merge_preferences(caller.user_id, &serde_json::json!({ "latest_import": body.ts }))
        .await
    {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Err(e) => error(500, e.to_string()),
    }
}

pub fn create_lb_api_router() -> Router<Arc<AppState>> {
    // axum 0.7 path syntax — `:user`, not `{user}`.
    Router::new()
        .route("/1/user/:user/listens", get(user_listens))
        .route("/1/user/:user/playing-now", get(user_playing_now))
        .route("/1/user/:user/listen-count", get(user_listen_count))
        .route("/1/user/:user/services", get(user_services))
        .route("/1/playing-now/delete", post(playing_now_delete))
        .route("/1/latest-import", get(get_latest_import).post(set_latest_import))
}
