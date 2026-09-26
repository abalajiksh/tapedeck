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
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use super::auth::IngestAuth;
use super::models::{ErrorResponse, OkResponse};
use super::AppState;
use crate::db::ScrobbleRecord;
use crate::openapi::{BadRequest, Forbidden, NotFound, Unauthorized};

fn error(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
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

/// A listen in ListenBrainz's shape. Tapedeck's audio-quality extensions
/// ride along in `additional_info.tapedeck_audio` under the keys the ingest
/// API accepts, so a listen submitted to Tapedeck reads back the way it went
/// in.
#[derive(Serialize, ToSchema)]
pub struct LbListen {
    /// Unix seconds. **Absent on a now-playing entry** — that is how a client
    /// tells it apart from history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listened_at: Option<i64>,
    pub track_metadata: LbTrackMetadata,
    /// Only on a now-playing entry, and then `true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playing_now: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct LbTrackMetadata {
    pub artist_name: String,
    pub track_name: String,
    pub release_name: Option<String>,
    pub additional_info: LbAdditionalInfo,
}

/// Every key is omitted when unknown rather than sent as null.
#[derive(Serialize, ToSchema, Default)]
pub struct LbAdditionalInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_mbid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_mbid: Option<String>,
    // Stored as a JSON array string; passed through as the array when it
    // parses, wrapped as a one-element array when it does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Vec<String>>)]
    pub artist_mbids: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_client: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_number: Option<i32>,
    /// Present when any audio detail is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tapedeck_audio: Option<LbAudio>,
}

/// The `tapedeck_audio` object a client submitted, read back. Every key is
/// omitted when unknown.
///
/// `quality_score` is deliberately absent: it is derived, and a reader should
/// recompute it from the measurements, which are the facts.
// Re-importing a score would fossilise a number computed under whatever
// formula was current when the listen arrived.
#[derive(Serialize, ToSchema, Default)]
pub struct LbAudio {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_lossless: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsd_rate: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsd_multiplier: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsd_to_pcm_converted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_codec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_bitrate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_sample_rate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_bit_depth: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_transcoded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcode_reason: Option<String>,
}

impl LbAudio {
    fn is_empty(&self) -> bool {
        serde_json::to_value(self).ok().and_then(|v| v.as_object().map(|o| o.is_empty()))
            == Some(true)
    }
}

fn to_lb_listen(s: &ScrobbleRecord, include_listened_at: bool) -> LbListen {
    let artist_mbids = s.mbid_artist.as_ref().map(|m| {
        // Stored as a JSON array string; pass it through as an array when it parses.
        match serde_json::from_str::<serde_json::Value>(m) {
            Ok(v) if v.is_array() => v,
            _ => serde_json::json!([m]),
        }
    });
    let audio = LbAudio {
        format_type: s.format_type.clone(),
        codec: s.codec.clone(),
        bitrate: s.bitrate,
        sample_rate: s.sample_rate,
        bit_depth: s.bit_depth,
        channels: s.channels,
        container: s.container.clone(),
        source_quality: s.source_quality.clone(),
        is_lossless: s.is_lossless,
        dsd_rate: s.dsd_rate,
        dsd_multiplier: s.dsd_multiplier,
        dsd_to_pcm_converted: s.dsd_to_pcm_converted,
        delivery_codec: s.delivery_codec.clone(),
        delivery_bitrate: s.delivery_bitrate,
        delivery_sample_rate: s.delivery_sample_rate,
        delivery_bit_depth: s.delivery_bit_depth,
        is_transcoded: s.is_transcoded,
        transcode_reason: s.transcode_reason.clone(),
    };
    LbListen {
        listened_at: include_listened_at.then_some(s.timestamp),
        track_metadata: LbTrackMetadata {
            artist_name: s.artist.clone(),
            track_name: s.title.clone(),
            release_name: s.album.clone(),
            additional_info: LbAdditionalInfo {
                duration_ms: s.duration.map(|d| d * 1000),
                recording_mbid: s.mbid_recording.clone(),
                release_mbid: s.mbid_release.clone(),
                artist_mbids,
                submission_client: s.submission_client.clone(),
                track_number: s.track_number,
                tapedeck_audio: (!audio.is_empty()).then_some(audio),
            },
        },
        playing_now: None,
    }
}

/// ListenBrainz wraps every read in `payload`.
#[derive(Serialize, ToSchema)]
pub struct LbListensResponse {
    pub payload: LbListensPayload,
}

#[derive(Serialize, ToSchema)]
pub struct LbListensPayload {
    pub count: usize,
    /// The username asked for, as given in the path.
    pub user_id: String,
    pub listens: Vec<LbListen>,
    /// The newest listen's timestamp, or 0 with none.
    pub latest_listen_ts: i64,
}

#[derive(Serialize, ToSchema)]
pub struct LbPlayingNowResponse {
    pub payload: LbPlayingNowPayload,
}

#[derive(Serialize, ToSchema)]
pub struct LbPlayingNowPayload {
    /// 0 when nothing is playing, else 1.
    pub count: usize,
    pub user_id: String,
    pub listens: Vec<LbListen>,
    /// Always `true`.
    pub playing_now: bool,
}

#[derive(Serialize, ToSchema)]
pub struct LbCountResponse {
    pub payload: LbCount,
}

#[derive(Serialize, ToSchema)]
pub struct LbCount {
    /// Every listen, skips excluded.
    pub count: i64,
}

#[derive(Serialize, ToSchema)]
pub struct LbServices {
    pub user_name: String,
    /// The forwarding services connected: `lastfm`, `listenbrainz`, `librefm`.
    pub services: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct LbLatestImport {
    /// The username asked about.
    pub musicbrainz_id: String,
    /// Unix seconds; 0 when none was ever recorded.
    pub latest_import: i64,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListensQuery {
    /// Only listens strictly before this, unix seconds. Not with `min_ts`.
    max_ts: Option<i64>,
    /// Only listens strictly after this, unix seconds. Not with `max_ts`.
    min_ts: Option<i64>,
    /// How many, newest first. 25 by default, at most 1000.
    #[param(minimum = 1, maximum = 1000, default = 25)]
    count: Option<i64>,
}

/// A user's listens, LB-shaped.
///
/// **Scoped to the caller.** LB serves this publicly; a self-hosted instance
/// must not, so asking for another user's listens is refused unless the caller
/// is an admin. Newest first.
#[utoipa::path(
    get,
    path = "/1/user/{user}/listens",
    tag = "ListenBrainz",
    security(("apiToken" = []), ("sessionCookie" = [])),
    params(("user" = String, Path, description = "Your own username."), ListensQuery),
    responses(
        (status = 200, description = "OK.", body = LbListensResponse),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 404, response = NotFound),
    )
)]
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
        .get_recent_scrobbles(user_id, count, 0, None, None, None, after, before, None)
        .await
    {
        Ok(r) => r,
        Err(e) => return error(500, e.to_string()),
    };
    let (_, latest) = state.db.listen_count_and_latest(user_id).await.unwrap_or((0, None));
    let listens: Vec<LbListen> = rows.iter().map(|s| to_lb_listen(s, true)).collect();

    (
        StatusCode::OK,
        Json(LbListensResponse {
            payload: LbListensPayload {
                count: listens.len(),
                user_id: user,
                listens,
                latest_listen_ts: latest.unwrap_or(0),
            },
        }),
    )
        .into_response()
}

/// What is on the deck, LB-shaped.
#[utoipa::path(
    get,
    path = "/1/user/{user}/playing-now",
    tag = "ListenBrainz",
    security(("apiToken" = []), ("sessionCookie" = [])),
    params(("user" = String, Path, description = "Your own username.")),
    responses(
        (status = 200, description = "OK. `count` is 0 when nothing is playing.",
            body = LbPlayingNowResponse),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 404, response = NotFound),
    )
)]
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
    let listens: Vec<LbListen> = match state.now_playing.get(user_id) {
        Some((np, _)) => vec![LbListen {
            listened_at: None,
            track_metadata: LbTrackMetadata {
                artist_name: np.artist.clone(),
                track_name: np.title.clone(),
                release_name: np.album.clone(),
                additional_info: LbAdditionalInfo {
                    duration_ms: np.duration.map(|d| d as i64 * 1000),
                    ..Default::default()
                },
            },
            playing_now: Some(true),
        }],
        None => Vec::new(),
    };

    (
        StatusCode::OK,
        Json(LbPlayingNowResponse {
            payload: LbPlayingNowPayload {
                count: listens.len(),
                user_id: user,
                listens,
                playing_now: true,
            },
        }),
    )
        .into_response()
}

/// Total listens. Excludes skips.
#[utoipa::path(
    get,
    path = "/1/user/{user}/listen-count",
    tag = "ListenBrainz",
    security(("apiToken" = []), ("sessionCookie" = [])),
    params(("user" = String, Path, description = "Your own username.")),
    responses(
        (status = 200, description = "OK.", body = LbCountResponse),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 404, response = NotFound),
    )
)]
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
        Ok((count, _)) => {
            (StatusCode::OK, Json(LbCountResponse { payload: LbCount { count } })).into_response()
        }
        Err(e) => error(500, e.to_string()),
    }
}

// GET /1/user/:user/services — which scrobble services this user has connected.
/// Connected forwarding services.
#[utoipa::path(
    get,
    path = "/1/user/{user}/services",
    tag = "ListenBrainz",
    security(("apiToken" = []), ("sessionCookie" = [])),
    params(("user" = String, Path, description = "Your own username.")),
    responses(
        (status = 200, description = "OK.", body = LbServices),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 404, response = NotFound),
    )
)]
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
            (StatusCode::OK, Json(LbServices { user_name: user, services })).into_response()
        }
        Err(e) => error(500, e.to_string()),
    }
}

// POST /1/playing-now/delete — stop reporting this user as playing.
/// Clear the caller's now-playing entry.
#[utoipa::path(
    post,
    path = "/1/playing-now/delete",
    tag = "ListenBrainz",
    security(("apiToken" = []), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "Cleared. `status` is `ok`.", body = OkResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn playing_now_delete(
    State(state): State<Arc<AppState>>,
    IngestAuth { user: caller, .. }: IngestAuth,
) -> impl IntoResponse {
    state.now_playing.clear_user(caller.user_id);
    (StatusCode::OK, OkResponse::new("ok")).into_response()
}

// ── latest-import ──
//
// Importers use this as a bookmark: read the timestamp, fetch everything newer,
// write the new high-water mark back. Stored in the user's settings blob rather
// than its own table — it's one integer per user.

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LatestImportQuery {
    /// Whose bookmark. Defaults to the caller; anyone else's is admin-only.
    user_name: Option<String>,
}

/// Timestamp of the caller's most recent import.
///
/// Importers use this as a bookmark: read the timestamp, fetch everything
/// newer, write the new high-water mark back.
#[utoipa::path(
    get,
    path = "/1/latest-import",
    tag = "ListenBrainz",
    security(("apiToken" = []), ("sessionCookie" = [])),
    params(LatestImportQuery),
    responses(
        (status = 200, description = "OK.", body = LbLatestImport),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 404, response = NotFound),
    )
)]
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
    (StatusCode::OK, Json(LbLatestImport { musicbrainz_id: user, latest_import: ts }))
        .into_response()
}

#[derive(Deserialize, ToSchema)]
pub struct LatestImportBody {
    /// Unix seconds. Not negative.
    #[schema(minimum = 0)]
    ts: i64,
}

// POST /1/latest-import — body `{"ts": <unix seconds>}`, always the caller's own.
/// Record an import high-water mark.
///
/// Always the caller's own.
#[utoipa::path(
    post,
    path = "/1/latest-import",
    tag = "ListenBrainz",
    security(("apiToken" = []), ("sessionCookie" = [])),
    request_body = LatestImportBody,
    responses(
        (status = 200, description = "Stored. `status` is `ok`.", body = OkResponse),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
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
        Ok(_) => (StatusCode::OK, OkResponse::new("ok")).into_response(),
        Err(e) => error(500, e.to_string()),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    user_listens,
    user_playing_now,
    user_listen_count,
    user_services,
    playing_now_delete,
    get_latest_import,
    set_latest_import,
))]
pub struct Api;

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
