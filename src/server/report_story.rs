//! `GET /api/v1/reports/story` and the video made from it.
//!
//! A report period told as a short animated story — see
//! `crate::report_image::story` for what a story is and
//! `crate::report_video` for how one becomes an MP4.
//!
//! Like the share image, this **computes nothing**: the period comes from
//! `reports()` through the same loader the image uses, so a story cannot
//! disagree with the chapter it was opened from.
//!
//! The export is `ReadAuth` even though it is a `POST`. It writes nothing of
//! the user's — the only thing created is a temporary file that expires — and
//! a phone holding a `read` token is exactly the client that wants a video to
//! share. What it does cost is CPU, which is why one render runs at a time
//! and each user has one in flight.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::report_image::story::Story;
use crate::report_image::{Card, Theme, Unit};
use crate::report_video::{self, Job, Phase};
use crate::server::auth::ReadAuth;
use crate::server::models::ErrorResponse;
use crate::server::report_image::{err, load_period, slug};
use crate::server::AppState;

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StoryQuery {
    /// `week`, `month` (default) or `year`.
    #[param(pattern = "^(week|month|year)$", default = "month")]
    pub unit: Option<String>,
    /// How many periods back from the newest, matching the Reports stepper.
    pub offset: Option<usize>,
    /// `dark` (default) or `light`. Asked for, not inherited, for the same
    /// reason the share image asks: the result lands on somebody else's feed.
    pub theme: Option<String>,
}

/// A story, and whether this server can turn it into a video.
#[derive(Serialize, utoipa::ToSchema)]
pub struct StoryResponse {
    #[serde(flatten)]
    pub story: Story,
    /// True when the server has an encoder and `POST
    /// /api/v1/reports/story/video` will work. False is not an error: the
    /// story still plays, there is just nothing to download.
    pub video_export: bool,
}

/// Where a video export has got to.
#[derive(Serialize, utoipa::ToSchema)]
pub struct StoryVideo {
    pub id: String,
    /// `queued` (another render is running), `rendering`, `done` or `failed`.
    #[schema(pattern = "^(queued|rendering|done|failed)$")]
    pub state: String,
    pub frames_done: u32,
    pub frames_total: u32,
    /// The file's size once `state` is `done`, otherwise 0.
    pub bytes: u64,
    /// Why it failed, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<&Job> for StoryVideo {
    fn from(j: &Job) -> Self {
        StoryVideo {
            id: j.id.clone(),
            state: j.phase.as_str().to_string(),
            frames_done: j.frames_done.load(Ordering::Relaxed).min(j.frames_total),
            frames_total: j.frames_total,
            bytes: j.bytes,
            error: j.error.clone(),
        }
    }
}

/// The story of the period the query names, and a filename stem for it.
async fn build(
    state: &Arc<AppState>,
    auth: ReadAuth,
    q: &StoryQuery,
) -> Result<(Story, String), axum::response::Response> {
    // An unknown bucket is refused rather than defaulted: the wrong one would
    // be an answer that looks entirely right.
    let unit_str = q.unit.clone().unwrap_or_else(|| "month".into());
    let Some(unit) = Unit::parse(&unit_str) else {
        return Err(err(400, "unit must be week, month or year"));
    };
    let theme = Theme::parse(q.theme.as_deref());
    let period = load_period(state, auth, unit, q.offset.unwrap_or(0)).await?;
    let stem = format!(
        "tapedeck-{}-story",
        slug(period.get("key").and_then(serde_json::Value::as_str).unwrap_or(unit.as_str()))
    );
    let card = Card::from_period(&period);
    // Measuring text walks the font tables; cheap, but not free on a Pi.
    let story = tokio::task::spawn_blocking(move || Story::build(&card, unit, theme))
        .await
        .map_err(|e| {
            tracing::error!("Report story could not be built: {e}");
            err(500, "Could not build the story")
        })?;
    Ok((story, stem))
}

/// A report period as a short animated story.
///
/// Scope: `read` *(since v0.131)*.
///
/// The same facts as the chapter and its share image, told one at a time:
/// a few scenes, each a stack of SVG layers that fade, rise and count into
/// place. The server draws every layer — text is already fitted, escaped and
/// set in the embedded fonts' metrics — and a client only **moves** them.
///
/// To play it: draw `background`, then for the current scene draw each
/// layer's `svg` inside a group whose transform and opacity come from its
/// `tracks` at the scene's elapsed milliseconds.
///
/// * A track is a list of `{t, v}` keys. Before the first key the value is
///   the first key's, after the last the last key's, and between two keys it
///   eases out: `a + (b - a) * (1 - (1 - p)^3)`. An absent track is at rest
///   (opacity 1, offsets 0, scales 1).
/// * The group's transform is
///   `translate(dx + ox, dy + oy) scale(scale * scale_x, scale * scale_y) translate(-ox, -oy)`
///   where `(ox, oy)` is the layer's `origin`.
/// * A layer with `count` carries the literal `{{n}}` in its `svg`; replace
///   it with the eased value between `start_ms` and `end_ms`, rounded and
///   written with thousands separators.
/// * In its last `fade_out_ms` a scene's layers fade out together. The last
///   scene has none and is held.
///
/// It **computes nothing**: every figure comes from `GET /api/v1/reports`
/// with the same `unit` and `offset`. A scene the period has nothing to say
/// in is absent rather than empty, so a thin period is a shorter story.
#[utoipa::path(
    get,
    path = "/api/v1/reports/story",
    tag = "Statistics",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    params(StoryQuery),
    responses(
        (status = 200, description = "The story.", body = StoryResponse),
        (status = 400, description = "An unsupported unit.", body = ErrorResponse),
        (status = 404, description = "No report for that period — an offset past the end of the history, or an instance with nothing in it yet.", body = ErrorResponse),
    )
)]
pub async fn report_story(
    State(state): State<Arc<AppState>>,
    auth: ReadAuth,
    Query(q): Query<StoryQuery>,
) -> impl IntoResponse {
    let (story, _) = match build(&state, auth, &q).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let video_export = tokio::task::spawn_blocking(report_video::available).await.unwrap_or(false);
    (
        StatusCode::OK,
        // One person's listening: out of any shared cache.
        [(header::CACHE_CONTROL, "private, max-age=60")],
        Json(StoryResponse { story, video_export }),
    )
        .into_response()
}

/// Start making a video of a story.
///
/// Scope: `read` *(since v0.131)*. A `POST` that needs only `read`: nothing
/// of yours is written, the result is a temporary file.
///
/// Returns at once with a job to poll at
/// `GET /api/v1/reports/story/video/{id}`. The video is 1080×1920 H.264 at
/// 30 frames a second with no sound, drawn from the same story the `GET`
/// returns, and is kept for an hour.
///
/// One video is made at a time on the whole server, so `queued` means
/// somebody else's is being drawn. Each user may have one export in flight.
#[utoipa::path(
    post,
    path = "/api/v1/reports/story/video",
    tag = "Statistics",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    params(StoryQuery),
    responses(
        (status = 202, description = "The export was queued.", body = StoryVideo),
        (status = 400, description = "An unsupported unit.", body = ErrorResponse),
        (status = 404, description = "No report for that period.", body = ErrorResponse),
        (status = 409, description = "You already have a video being made.", body = ErrorResponse),
        (status = 501, description = "This server has no video encoder (ffmpeg with libx264). The story still plays.", body = ErrorResponse),
    )
)]
pub async fn report_story_video_start(
    State(state): State<Arc<AppState>>,
    auth: ReadAuth,
    Query(q): Query<StoryQuery>,
) -> impl IntoResponse {
    let user_id = auth.user.user_id;
    if !tokio::task::spawn_blocking(report_video::available).await.unwrap_or(false) {
        return err(501, "This server cannot make videos: ffmpeg is not installed");
    }
    if state.video_jobs.active(user_id).is_some() {
        return err(409, "You already have a video being made");
    }
    let (story, stem) = match build(&state, auth, &q).await {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let job = state.video_jobs.start(user_id, stem, story);
    (StatusCode::ACCEPTED, Json(StoryVideo::from(&job))).into_response()
}

/// How a video export is getting on.
///
/// Scope: `read` *(since v0.131)*.
#[utoipa::path(
    get,
    path = "/api/v1/reports/story/video/{id}",
    tag = "Statistics",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    params(("id" = String, Path, description = "The export, as `POST /api/v1/reports/story/video` named it.")),
    responses(
        (status = 200, description = "The export's state.", body = StoryVideo),
        (status = 404, description = "No such export. They are kept for an hour, and only their owner can see them.", body = ErrorResponse),
    )
)]
pub async fn report_story_video_status(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.video_jobs.get(&id, user.user_id) {
        Some(job) => Json(StoryVideo::from(&job)).into_response(),
        None => err(404, "There is no such video"),
    }
}

/// The finished video.
///
/// Scope: `read` *(since v0.131)*.
#[utoipa::path(
    get,
    path = "/api/v1/reports/story/video/{id}/file",
    tag = "Statistics",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    params(("id" = String, Path, description = "The export.")),
    responses(
        (status = 200, description = "The video. `Cache-Control` is `private`.", content((crate::openapi::Bytes = "video/mp4"))),
        (status = 404, description = "No such export, or it has expired.", body = ErrorResponse),
        (status = 409, description = "The export is not finished yet.", body = ErrorResponse),
    )
)]
pub async fn report_story_video_file(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(job) = state.video_jobs.get(&id, user.user_id) else {
        return err(404, "There is no such video");
    };
    if job.phase != Phase::Done {
        return err(409, "The video is not finished yet");
    }
    match tokio::fs::read(&job.path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "video/mp4".to_string()),
                (header::CACHE_CONTROL, "private, max-age=300".to_string()),
                (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}.mp4\"", job.stem)),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Story video {id} is listed but could not be read: {e}");
            err(404, "There is no such video")
        }
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    report_story,
    report_story_video_start,
    report_story_video_status,
    report_story_video_file
))]
pub struct Api;

pub fn create_report_story_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/reports/story", get(report_story))
        .route("/api/v1/reports/story/video", post(report_story_video_start))
        .route("/api/v1/reports/story/video/:id", get(report_story_video_status))
        .route("/api/v1/reports/story/video/:id/file", get(report_story_video_file))
}
