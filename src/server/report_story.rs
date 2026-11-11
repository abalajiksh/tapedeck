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
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::models::AuthUser;
use crate::report_image::story::{href_ok, Art, Story};
use crate::report_image::{Card, Theme, Unit};
use crate::report_video::{self, Images, Job, Phase};
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

/// The largest picture that will be read into memory for a video.
const MAX_PICTURE_BYTES: usize = 12 * 1024 * 1024;
/// How long one picture may take to arrive before the video goes without it.
const PICTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// A stored cover as an address: the listen's own URL, else the Cover Art
/// Archive one built from the ids enrichment left. `-1` is the tombstone for
/// "asked, nobody has one", which is why the id must be positive.
fn cover(row: Option<(Option<String>, Option<i64>, Option<String>)>) -> Option<String> {
    let (url, caa_id, release) = row?;
    url.filter(|u| !u.is_empty()).or_else(|| match (caa_id, release) {
        (Some(id), Some(mbid)) if id > 0 && !mbid.is_empty() => {
            Some(format!("https://coverartarchive.org/release/{mbid}/{id}-500.jpg"))
        }
        _ => None,
    })
}

/// A record's cover as its own page shows it: a hand-set one first, else the
/// one its listens carry.
async fn album_cover(
    state: &Arc<AppState>,
    user_id: i64,
    artist: &str,
    album: &str,
) -> Option<String> {
    // `find_entity` falls back to any record of that name when the exact one
    // has no entity, so the artist is checked: another artist's hand-set
    // cover is not this record's.
    let chosen = match state.db.find_entity("release", album, Some(artist)).await {
        Ok(Some(e)) if e.artist_name.trim().eq_ignore_ascii_case(artist.trim()) => {
            state.db.entity_image(e.id).await.ok().flatten().filter(|u| !u.is_empty())
        }
        _ => None,
    };
    match chosen {
        Some(url) => Some(url),
        None => cover(state.db.album_artwork(user_id, artist, album).await.ok().flatten()),
    }
}

/// Where the period's pictures are: the top artist's portrait as this person
/// sees it, the record's cover (a hand-set one first, as on its page), and
/// the cover of the album the track is from — never a compilation's.
///
/// Addresses only — nothing is fetched here. A browser follows them; the
/// video fetches them in `fetch_art`. Every one may be absent, and the story
/// then shows the lettered tile it always had.
async fn resolve_art(state: &Arc<AppState>, user_id: i64, period: &serde_json::Value) -> Art {
    let field = |entity: &str, key: &str| {
        period
            .get(entity)
            .and_then(|v| v.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
    };
    let mut art = Art::default();

    if let Some(name) = field("top_artist", "name") {
        art.artist = state
            .db
            .artist_images(user_id, std::slice::from_ref(&name))
            .await
            .unwrap_or_default()
            .into_values()
            .next()
            .filter(|u| !u.is_empty());
    }

    if let (Some(album), Some(artist)) = (field("top_album", "name"), field("top_album", "artist"))
    {
        art.album = album_cover(state, user_id, &artist, &album).await;
    }

    // A track wears its album's cover, found the way the record's is. The
    // album is the artist's own that the track was played from most; a track
    // only ever heard on a compilation keeps its lettered tile.
    if let (Some(title), Some(artist)) = (field("top_track", "title"), field("top_track", "artist"))
    {
        if let Ok(Some(album)) = state.db.track_album(user_id, &artist, &title).await {
            art.track = album_cover(state, user_id, &artist, &album).await;
        }
    }
    art
}

/// One picture's bytes, fetched the way its address says.
///
/// * **One of this server's own paths** is answered by the artwork and
///   picture routes themselves, called in-process with the caller's own
///   credentials — so whatever those handlers check (the listen is yours,
///   the pick is yours, the upload name is safe) is checked here too, by the
///   same code rather than a copy of it.
/// * **A web URL** goes out through `Egress` for this user: a member's
///   stored URL cannot send the server to a private address, the rule every
///   other outbound-from-a-stored-row path follows.
///
/// A redirect from a local path is followed once (a picture pick falls back
/// to the shared portrait with a 307).
async fn fetch_picture(
    state: &Arc<AppState>,
    user: &AuthUser,
    headers: &HeaderMap,
    href: &str,
) -> Option<Vec<u8>> {
    // `Service::call` directly rather than `oneshot`: a `Router` is always
    // ready, and `ServiceExt` would mean turning on a tower feature for it.
    use tower::Service;
    let mut href = href.to_string();
    for _ in 0..2 {
        if !href_ok(&href) {
            return None;
        }
        if !href.starts_with('/') {
            let egress = crate::sources::egress::Egress::for_owner(user.is_admin);
            egress.check_url(&href).ok()?;
            let resp = egress.client(PICTURE_TIMEOUT).get(&href).send().await.ok()?;
            if !resp.status().is_success()
                || resp.content_length().is_some_and(|n| n as usize > MAX_PICTURE_BYTES)
            {
                return None;
            }
            let bytes = resp.bytes().await.ok()?;
            return (bytes.len() <= MAX_PICTURE_BYTES).then(|| bytes.to_vec());
        }

        let mut app = super::artwork::create_artwork_router()
            .merge(super::pictures::create_pictures_router())
            .with_state(state.clone());
        let mut req = axum::http::Request::get(&href);
        for name in [header::COOKIE, header::AUTHORIZATION] {
            if let Some(v) = headers.get(&name) {
                req = req.header(name, v);
            }
        }
        let resp = app.call(req.body(axum::body::Body::empty()).ok()?).await.ok()?;
        if resp.status().is_redirection() {
            href = resp.headers().get(header::LOCATION)?.to_str().ok()?.to_string();
            continue;
        }
        if !resp.status().is_success() {
            return None;
        }
        return axum::body::to_bytes(resp.into_body(), MAX_PICTURE_BYTES)
            .await
            .ok()
            .map(|b| b.to_vec());
    }
    None
}

/// Fetch and size every picture the story draws, for the video.
///
/// All at once and each under a deadline: a media server that is off costs
/// the export eight seconds and one picture, never the export. Whatever does
/// not arrive, or is not a picture a frame can draw, is simply not in the map, and the
/// frame shows the lettered tile under it.
async fn fetch_art(
    state: &Arc<AppState>,
    user: &AuthUser,
    headers: &HeaderMap,
    story: &Story,
) -> Images {
    let fetches = story.art.iter().map(|(href, px)| async move {
        let bytes =
            tokio::time::timeout(PICTURE_TIMEOUT, fetch_picture(state, user, headers, href))
                .await
                .ok()
                .flatten()?;
        let px = *px;
        let ready = tokio::task::spawn_blocking(move || report_video::prepare(bytes, px))
            .await
            .ok()
            .flatten()?;
        Some((href.clone(), Arc::new(ready)))
    });
    futures_util::future::join_all(fetches).await.into_iter().flatten().collect()
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
    let user_id = auth.user.user_id;
    let period = load_period(state, auth, unit, q.offset.unwrap_or(0)).await?;
    let stem = format!(
        "tapedeck-{}-story",
        slug(period.get("key").and_then(serde_json::Value::as_str).unwrap_or(unit.as_str()))
    );
    let card = Card::from_period(&period);
    let art = resolve_art(state, user_id, &period).await;
    // Measuring text walks the font tables; cheap, but not free on a Pi.
    let story = tokio::task::spawn_blocking(move || Story::build(&card, unit, theme, &art))
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
/// **Pictures.** A layer may contain an `<image>` whose `href` is the top
/// artist's portrait, the record's cover or the track's: either a web URL
/// or a path on this server (fetch it with the same credentials). Each sits
/// over a lettered tile, so a picture that is absent or fails to load needs
/// no handling — draw the layer as it is.
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
/// Pictures are fetched by the server before the first frame, each with a
/// deadline. One that does not arrive, or is not a PNG, JPEG, GIF or WebP, is left
/// out and its lettered tile shows instead; the export does not fail.
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
    headers: HeaderMap,
    Query(q): Query<StoryQuery>,
) -> impl IntoResponse {
    let user = auth.user.clone();
    let user_id = user.user_id;
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
    let images = fetch_art(&state, &user, &headers, &story).await;
    let job = state.video_jobs.start(user_id, stem, story, images);
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
