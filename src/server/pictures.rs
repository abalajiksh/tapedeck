//! Choosing an artist's picture — per person, from everywhere one can come from.
//!
//! The shared `artists` row keeps what the backfill found. A choice made here
//! is a row in `artist_pictures` and changes what *this* person sees, nobody
//! else; see `init_artist_pictures` for why the override moved off the shared
//! row in v0.124.0.
//!
//! The picker is Plex's *Edit → Poster / Background*: every picture each of the
//! person's own servers holds or can find, Deezer's, and their own uploads and
//! links, each labelled with where it came from.
//!
//! Three rules this module keeps:
//!
//! * **A library picture is stored as a key, never as a URL to fetch.** The key
//!   comes back from the browser when a picture is chosen, so it is checked
//!   against the shape that server's lister writes — a Plex path under
//!   `/library/metadata/`, a Jellyfin `item/Type/index`, a Subsonic cover id —
//!   and the bytes are only ever asked of the server the source row names.
//! * **A remote picture is the browser's to load.** Deezer's and a server's
//!   online finds are public URLs; Tapedeck stores the link, the rule the
//!   artwork chain already follows, and never downloads them.
//! * **A choice that stops working falls back to the shared picture**, rather
//!   than to nothing: a server switched off should not blank a face the
//!   backfill could still supply.

use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use utoipa::ToSchema;

use super::artwork::{
    artist_image_from_library, bad, forget_previous, not_found, read_one_image, store_upload,
    validate_image_url, UPLOAD_PREFIX,
};
use super::auth::{AuthUser, ReadAuth};
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{ArtistPicture, NewArtistPicture, UserSource};
use crate::openapi::{BadRequest, Bytes, FileUpload, NotFound, Unauthorized};
use crate::sources::{LibraryImage, MediaBrowserSource, NavidromeSource, PlexFilters, PlexSource};

/// How long one server gets to answer the picker. Jellyfin's online search asks
/// its providers live, and one slow server must not hold up the rest.
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// The library kinds that can supply a picture.
const LIBRARY_KINDS: [&str; 4] = ["plex", "jellyfin", "emby", "navidrome"];

/// `portrait` or `wide`, from the path.
fn kind_ok(kind: &str) -> bool {
    matches!(kind, "portrait" | "wide")
}

fn bad_kind() -> Response {
    bad(StatusCode::BAD_REQUEST, "A picture is `portrait` or `wide`")
}

/// The size a library is asked for: a tile, or the picture itself.
fn max_px(kind: &str, preview: bool) -> u32 {
    match (kind, preview) {
        (_, true) => 400,
        ("wide", false) => 1600,
        _ => 800,
    }
}

/// One picture on offer.
#[derive(Debug, Serialize, ToSchema)]
pub struct PictureCandidate {
    /// `plex`, `jellyfin`, `emby`, `navidrome` or `deezer`.
    pub source: String,
    /// The connected server it came from; null for Deezer.
    pub source_ref: Option<i64>,
    /// What to call that source on the tile — the server's own label.
    pub source_label: String,
    /// The outside provider a server found it through, when it says
    /// (TheAudioDB, fanart.tv, a Plex agent).
    pub provider: Option<String>,
    /// Opaque. Send it back unchanged to choose this picture.
    pub key: String,
    /// What to draw on the tile.
    pub preview: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// The server itself currently shows this one.
    pub server_selected: bool,
    /// This is the picture you chose.
    pub chosen: bool,
}

/// Your current choice.
#[derive(Debug, Serialize, ToSchema)]
pub struct PictureChoice {
    /// `upload`, `link`, `deezer`, `plex`, `jellyfin`, `emby` or `navidrome`.
    pub source: String,
    pub source_ref: Option<i64>,
    /// The server's label for a library choice; null otherwise.
    pub source_label: Option<String>,
    pub provider: Option<String>,
    /// What the browser loads.
    pub url: String,
}

/// Everything the picker draws for one tab.
#[derive(Debug, Serialize, ToSchema)]
pub struct PictureChoices {
    /// `portrait` or `wide`.
    pub kind: String,
    /// What you chose, if anything. Null means you see the shared picture.
    pub current: Option<PictureChoice>,
    /// The picture everyone sees who has not chosen one — what the backfill
    /// found. Null when it found nothing.
    pub shared_url: Option<String>,
    pub candidates: Vec<PictureCandidate>,
    /// Sources that did not answer, by label, so the picker can say why a
    /// server's pictures are missing rather than looking empty.
    pub unreachable: Vec<String>,
}

/// Choose a picture: a candidate from the list, or a link.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PictureChoose {
    /// A candidate's `source`, or `link` for a URL of your own.
    pub source: String,
    /// A candidate's `source_ref`. Required for a library source.
    #[serde(default)]
    pub source_ref: Option<i64>,
    /// A candidate's `key`, or the URL for `link`.
    pub key: String,
}

/// What you see now. Null when nothing is left to show.
#[derive(Debug, Serialize, ToSchema)]
pub struct PictureSet {
    pub url: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PictureListQuery {
    /// `portrait` (the default) or `wide`.
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PicturePreviewQuery {
    /// The candidate's `source_ref`.
    pub source_ref: i64,
    /// The candidate's `key`.
    pub key: String,
}

/// The shared picture of `kind` for an artist, or `None` when there is no such
/// artist. The inner `None` is an artist the backfill found nothing for.
async fn shared_picture(state: &AppState, artist_id: i64, kind: &str) -> Option<Option<String>> {
    let (portrait, wide) = state.db.artist_picture_urls(artist_id).await.ok()??;
    let url = if kind == "wide" { wide } else { portrait };
    Some(url.filter(|u| !u.trim().is_empty()))
}

/// A library client for a source row, asked for the bytes behind a key.
async fn fetch_library_image(
    source: &UserSource,
    key: &str,
    max: u32,
) -> Result<(Vec<u8>, String), String> {
    let egress = source.egress();
    egress.check_url(&source.url).map_err(|e| e.to_string())?;
    let r = match source.kind.as_str() {
        "plex" => {
            let token = source.credential("token").ok_or("Plex token is missing")?;
            PlexSource::with_filters(source.url.clone(), token, PlexFilters::default(), egress)
                .fetch_image(key, max)
                .await
        }
        "jellyfin" | "emby" => {
            let api_key = source.credential("api_key").ok_or("API key is missing")?;
            mediabrowser(source, api_key).fetch_image(key, max).await
        }
        "navidrome" => {
            if !subsonic_cover_id_ok(key) {
                return Err("not a cover id".into());
            }
            let (Some(u), Some(p)) = (source.credential("username"), source.credential("password"))
            else {
                return Err("Navidrome credentials are missing".into());
            };
            NavidromeSource::new(source.url.clone(), u, p, egress).fetch_cover_art(key).await
        }
        _ => return Err("That source has no pictures".into()),
    };
    r.map_err(|e| e.to_string())
}

fn mediabrowser(source: &UserSource, api_key: String) -> MediaBrowserSource {
    let account = source.remote_account.clone().unwrap_or_default();
    if source.kind == "emby" {
        MediaBrowserSource::emby(source.url.clone(), api_key, account, source.egress())
    } else {
        MediaBrowserSource::jellyfin(source.url.clone(), api_key, account, source.egress())
    }
}

/// A Subsonic cover id as `artist_cover_id` returns it: Navidrome's are
/// `ar-<id>`, sometimes with a `_<n>` suffix.
fn subsonic_cover_id_ok(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A Deezer image is a link on Deezer's CDN and nothing else.
fn deezer_url_ok(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some_and(|h| h == "dzcdn.net" || h.ends_with(".dzcdn.net"))
    })
}

fn web_url_ok(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

/// Does this key have the shape the source's own lister writes?
///
/// Returns whether it is a **remote** key — a public URL the browser loads —
/// or `None` when it is not a key that source could have offered.
fn library_key(kind: &str, key: &str) -> Option<bool> {
    match kind {
        "plex" => crate::sources::plex::plex_image_key_ok(key).then_some(false),
        "jellyfin" | "emby" => {
            if crate::sources::mediabrowser::parse_mb_image_key(key).is_some() {
                Some(false)
            } else {
                web_url_ok(key).then_some(true)
            }
        }
        "navidrome" => subsonic_cover_id_ok(key).then_some(false),
        _ => None,
    }
}

/// Where a library choice is served from. The version changes with every
/// choice, so a long cache never shows the previous one.
fn proxy_url(artist_id: i64, kind: &str, at: i64) -> String {
    format!("/api/v1/artists/{artist_id}/pictures/{kind}?v={at}")
}

fn preview_url(artist_id: i64, kind: &str, source_ref: i64, key: &str) -> String {
    format!(
        "/api/v1/artists/{artist_id}/pictures/{kind}/preview?source_ref={source_ref}&key={}",
        urlencoding::encode(key)
    )
}

/// Ask one library what it has for this artist.
async fn library_candidates(
    source: &UserSource,
    name: &str,
    wide: bool,
) -> Result<Vec<LibraryImage>, String> {
    let egress = source.egress();
    egress.check_url(&source.url).map_err(|e| e.to_string())?;
    let found = match source.kind.as_str() {
        "plex" => {
            let token = source.credential("token").ok_or("Plex token is missing")?;
            let plex =
                PlexSource::with_filters(source.url.clone(), token, PlexFilters::default(), egress);
            match plex.find_artist(name).await.map_err(|e| e.to_string())? {
                Some(key) => plex.artist_images(&key, wide).await,
                None => Ok(Vec::new()),
            }
        }
        "jellyfin" | "emby" => {
            let api_key = source.credential("api_key").ok_or("API key is missing")?;
            let mb = mediabrowser(source, api_key);
            match mb.artist_item(name).await.map_err(|e| e.to_string())? {
                Some(id) => mb.artist_images(&id, wide).await,
                None => Ok(Vec::new()),
            }
        }
        "navidrome" => {
            // Subsonic has one picture per artist and no wide one.
            if wide {
                return Ok(Vec::new());
            }
            let (Some(u), Some(p)) = (source.credential("username"), source.credential("password"))
            else {
                return Err("Navidrome credentials are missing".into());
            };
            let nd = NavidromeSource::new(source.url.clone(), u, p, egress);
            nd.artist_cover_id(name).await.map(|id| {
                id.into_iter()
                    .map(|key| LibraryImage {
                        key,
                        remote: false,
                        provider: None,
                        selected: true,
                        width: None,
                        height: None,
                        thumb: None,
                    })
                    .collect()
            })
        }
        _ => Ok(Vec::new()),
    };
    found.map_err(|e| e.to_string())
}

fn label_of(source: &UserSource) -> String {
    if !source.label.trim().is_empty() {
        return source.label.clone();
    }
    let k = &source.kind;
    k[..1].to_uppercase() + &k[1..]
}

/// Every picture on offer for one tab of the picker.
///
/// Session-only. Asks each of your enabled media servers — Plex's poster or
/// background list, Jellyfin's and Emby's held images plus the ones they can
/// find online, Navidrome's artist picture — and, for a portrait, Deezer. The
/// servers are asked at once, with ten seconds each; one that does not answer
/// is named in `unreachable` rather than failing the list.
///
/// Library tiles preview through this server, since the browser cannot reach
/// your servers. Online finds and Deezer's are public URLs and load directly.
#[utoipa::path(
    get,
    path = "/api/v1/artists/{id}/pictures",
    tag = "Library",
    params(
        ("id" = i64, Path, description = "The `artists` row — `artist_row_id` on an artist page."),
        PictureListQuery,
    ),
    responses(
        (status = 200, description = "The picker's tiles.", body = PictureChoices),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn list_pictures(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<PictureListQuery>,
) -> Response {
    let kind = q.kind.unwrap_or_else(|| "portrait".into());
    if !kind_ok(&kind) {
        return bad_kind();
    }
    let wide = kind == "wide";
    let Ok(Some((name, _))) = state.db.artist_by_id(id).await else {
        return not_found("No such artist");
    };
    let shared_url = shared_picture(&state, id, &kind).await.flatten();
    let current_pick = state.db.artist_picture(user.user_id, id, &kind).await.ok().flatten();

    let sources: Vec<UserSource> = state
        .db
        .list_sources_for_user(user.user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.enabled && LIBRARY_KINDS.contains(&s.kind.as_str()))
        .collect();

    let asks = sources.iter().map(|s| {
        let name = name.clone();
        async move {
            match tokio::time::timeout(LIST_TIMEOUT, library_candidates(s, &name, wide)).await {
                Ok(r) => r,
                Err(_) => Err("did not answer in time".into()),
            }
        }
    });
    let deezer = async {
        if wide {
            return Ok(Vec::new());
        }
        let provider = crate::metadata::deezer::DeezerProvider::new();
        match tokio::time::timeout(LIST_TIMEOUT, provider.artist_pictures(&name)).await {
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(_) => Err("did not answer in time".to_string()),
        }
    };
    let (answers, deezer) = tokio::join!(join_all(asks), deezer);

    let is_chosen = |source: &str, source_ref: Option<i64>, key: &str| {
        current_pick
            .as_ref()
            .is_some_and(|p| p.source == source && p.source_ref == source_ref && p.image_key == key)
    };

    let mut candidates = Vec::new();
    let mut unreachable = Vec::new();
    for (source, answer) in sources.iter().zip(answers) {
        match answer {
            Ok(images) => {
                for img in images {
                    let preview = if img.remote {
                        img.thumb.clone().unwrap_or_else(|| img.key.clone())
                    } else {
                        preview_url(id, &kind, source.id, &img.key)
                    };
                    candidates.push(PictureCandidate {
                        chosen: is_chosen(&source.kind, Some(source.id), &img.key),
                        source: source.kind.clone(),
                        source_ref: Some(source.id),
                        source_label: label_of(source),
                        provider: img.provider,
                        key: img.key,
                        preview,
                        width: img.width,
                        height: img.height,
                        server_selected: img.selected,
                    });
                }
            }
            Err(e) => {
                tracing::debug!("🖼️ Picture picker: {} did not answer: {}", source.label, e);
                unreachable.push(label_of(source));
            }
        }
    }
    match deezer {
        Ok(urls) => {
            for url in urls {
                candidates.push(PictureCandidate {
                    chosen: is_chosen("deezer", None, &url),
                    source: "deezer".into(),
                    source_ref: None,
                    source_label: "Deezer".into(),
                    provider: None,
                    preview: url.clone(),
                    key: url,
                    width: None,
                    height: None,
                    server_selected: false,
                });
            }
        }
        Err(e) => {
            tracing::debug!("🖼️ Picture picker: Deezer did not answer: {}", e);
            unreachable.push("Deezer".into());
        }
    }

    let current = current_pick.map(|p| PictureChoice {
        source_label: p.source_ref.and_then(|r| sources.iter().find(|s| s.id == r)).map(label_of),
        source: p.source,
        source_ref: p.source_ref,
        provider: p.provider,
        url: p.url,
    });

    (StatusCode::OK, Json(PictureChoices { kind, current, shared_url, candidates, unreachable }))
        .into_response()
}

/// Remove an uploaded file a choice no longer points at.
async fn forget_pick(state: &Arc<AppState>, previous: Option<ArtistPicture>) {
    if let Some(p) = previous.filter(|p| p.source == "upload") {
        forget_previous(state, Some(&p.url)).await;
    }
}

/// Choose a picture.
///
/// Session-only. Only **you** see it; everyone else keeps their own choice or
/// the shared picture.
///
/// Send back a candidate's `source`, `source_ref` and `key` from
/// `GET /api/v1/artists/{id}/pictures`, or `{"source": "link", "key": "https://…"}`
/// for a URL of your own. A key is checked against the shape that source
/// offers, so a made-up one is a 400 rather than a request to your server.
/// Uploads have their own route.
#[utoipa::path(
    put,
    path = "/api/v1/artists/{id}/pictures/{kind}",
    tag = "Library",
    params(
        ("id" = i64, Path, description = "The `artists` row."),
        ("kind" = String, Path, description = "`portrait` or `wide`."),
    ),
    request_body = PictureChoose,
    responses(
        (status = 200, description = "Chosen. The body carries what the browser should load.", body = PictureSet),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn choose_picture(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, kind)): Path<(i64, String)>,
    Json(body): Json<PictureChoose>,
) -> Response {
    if !kind_ok(&kind) {
        return bad_kind();
    }
    if state.db.artist_by_id(id).await.ok().flatten().is_none() {
        return not_found("No such artist");
    }
    let key = body.key.trim();
    let now = crate::db::Database::now_secs();

    let (source_ref, provider, url) = match body.source.as_str() {
        "link" => {
            let Some(Some(url)) = validate_image_url(key) else {
                return bad(StatusCode::BAD_REQUEST, "That doesn't look like an image URL");
            };
            // A path here may only name the pictures this server already
            // proxies. An upload's path is refused because a file is deleted
            // when its owner replaces it, and a link to somebody else's would
            // break the moment they did.
            if url.starts_with('/')
                && (url.starts_with(UPLOAD_PREFIX)
                    || !(url.starts_with("/api/v1/artists/img/")
                        || url.starts_with("/api/v1/art/")))
            {
                return bad(StatusCode::BAD_REQUEST, "Upload the picture instead of linking to it");
            }
            (None, None, url.to_string())
        }
        "deezer" => {
            if !deezer_url_ok(key) {
                return bad(StatusCode::BAD_REQUEST, "That is not a Deezer picture");
            }
            (None, None, key.to_string())
        }
        kind_name if LIBRARY_KINDS.contains(&kind_name) => {
            let Some(source_ref) = body.source_ref else {
                return bad(StatusCode::BAD_REQUEST, "Which server is that picture on?");
            };
            let Ok(Some(source)) = state.db.get_source(user.user_id, source_ref).await else {
                return not_found("That server is not connected");
            };
            if source.kind != kind_name {
                return bad(StatusCode::BAD_REQUEST, "That picture is not from that server");
            }
            let Some(remote) = library_key(&source.kind, key) else {
                return bad(StatusCode::BAD_REQUEST, "That is not a picture that server offers");
            };
            let url = if remote { key.to_string() } else { proxy_url(id, &kind, now) };
            (Some(source_ref), None::<String>, url)
        }
        _ => return bad(StatusCode::BAD_REQUEST, "Unknown picture source"),
    };

    let pick = NewArtistPicture {
        source: &body.source,
        source_ref,
        image_key: key,
        provider: provider.as_deref(),
        url: &url,
    };
    match state.db.set_artist_picture(user.user_id, id, &kind, &pick).await {
        Ok(previous) => {
            forget_pick(&state, previous).await;
            (StatusCode::OK, Json(PictureSet { url: Some(url) })).into_response()
        }
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Go back to the shared picture.
///
/// Session-only. Removes your choice; an uploaded file it pointed at is
/// deleted. The body carries the shared picture you see now.
#[utoipa::path(
    delete,
    path = "/api/v1/artists/{id}/pictures/{kind}",
    tag = "Library",
    params(
        ("id" = i64, Path, description = "The `artists` row."),
        ("kind" = String, Path, description = "`portrait` or `wide`."),
    ),
    responses(
        (status = 200, description = "Cleared. Null when there is no shared picture either.", body = PictureSet),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn clear_picture(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, kind)): Path<(i64, String)>,
) -> Response {
    if !kind_ok(&kind) {
        return bad_kind();
    }
    let Some(shared) = shared_picture(&state, id, &kind).await else {
        return not_found("No such artist");
    };
    match state.db.clear_artist_picture(user.user_id, id, &kind).await {
        Ok(previous) => {
            forget_pick(&state, previous).await;
            (StatusCode::OK, Json(PictureSet { url: shared })).into_response()
        }
        Err(e) => bad(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Upload a picture and choose it.
///
/// Session-only. Same rules as every upload here: up to 8 MB, the extension
/// from the declared content type, a random name per file. Only you see it,
/// and it is deleted when you choose something else.
#[utoipa::path(
    post,
    path = "/api/v1/artists/{id}/pictures/{kind}/upload",
    tag = "Library",
    params(
        ("id" = i64, Path, description = "The `artists` row."),
        ("kind" = String, Path, description = "`portrait` or `wide`."),
    ),
    request_body(content = FileUpload, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "Saved and chosen. The URL is a path on this server.", body = PictureSet),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 413, description = "Larger than 8 MB.", body = ErrorResponse),
        (status = 415, description = "Not an image type this server stores (JPEG, PNG, WebP, GIF, AVIF, TIFF).", body = ErrorResponse),
    )
)]
async fn upload_picture(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((id, kind)): Path<(i64, String)>,
    mut multipart: Multipart,
) -> Response {
    if !kind_ok(&kind) {
        return bad_kind();
    }
    if state.db.artist_by_id(id).await.ok().flatten().is_none() {
        return not_found("No such artist");
    }
    let (bytes, ext) = match read_one_image(&mut multipart).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let url = match store_upload(&state, &bytes, ext).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let pick = NewArtistPicture {
        source: "upload",
        source_ref: None,
        image_key: &url,
        provider: None,
        url: &url,
    };
    match state.db.set_artist_picture(user.user_id, id, &kind, &pick).await {
        Ok(previous) => {
            forget_pick(&state, previous).await;
            (StatusCode::OK, Json(PictureSet { url: Some(url) })).into_response()
        }
        // The file is on disk already; one nothing points at is invisible
        // until the disk fills, so it goes now.
        Err(e) => {
            forget_previous(&state, Some(&url)).await;
            bad(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

fn image_response(bytes: Vec<u8>, content_type: String, cache: &str) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, cache.to_string())],
        Body::from(bytes),
    )
        .into_response()
}

/// Send the browser to the shared picture, uncached — the choice may start
/// working again, and the next load should ask.
async fn fall_back(state: &AppState, user_id: i64, id: i64, kind: &str) -> Response {
    match shared_picture(state, id, kind).await.flatten() {
        // The library proxy path is answered here rather than redirected, so a
        // portrait the backfill pointed at a library costs one round trip.
        Some(url) if url.starts_with("/api/v1/artists/img/") => {
            let Ok(Some((name, _))) = state.db.artist_by_id(id).await else {
                return not_found("No such artist");
            };
            match artist_image_from_library(state, user_id, &name).await {
                Some((bytes, ct)) => image_response(bytes, ct, "no-store"),
                None => not_found("No picture for that artist"),
            }
        }
        Some(url) => (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, url), (header::CACHE_CONTROL, "no-store".to_string())],
        )
            .into_response(),
        None => not_found("No picture for that artist"),
    }
}

/// Your chosen picture, when it lives on one of your servers.
///
/// Scope: `read`. This is the URL a library choice stores; a choice of any
/// other kind is a URL of its own. When the server cannot answer — off, or the
/// picture gone — this sends the shared picture instead, uncached, so a choice
/// that starts working again is shown again.
#[utoipa::path(
    get,
    path = "/api/v1/artists/{id}/pictures/{kind}",
    tag = "Library",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(
        ("id" = i64, Path, description = "The `artists` row."),
        ("kind" = String, Path, description = "`portrait` or `wide`."),
    ),
    responses(
        (status = 200, description = "Image bytes, `Cache-Control: private` for a week — the URL is versioned per choice.", body = Bytes, content_type = "image/*"),
        (status = 307, description = "Your choice could not be fetched, or you have none; this is the shared picture."),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No picture at all.", body = ErrorResponse),
    )
)]
async fn serve_picture(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path((id, kind)): Path<(i64, String)>,
) -> Response {
    if !kind_ok(&kind) {
        return bad_kind();
    }
    let pick = state.db.artist_picture(user.user_id, id, &kind).await.ok().flatten();
    let Some(pick) = pick else {
        return fall_back(&state, user.user_id, id, &kind).await;
    };
    let Some(source_ref) = pick.source_ref else {
        // Not a library choice: its URL is the picture.
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, pick.url), (header::CACHE_CONTROL, "no-store".to_string())],
        )
            .into_response();
    };
    let Ok(Some(source)) = state.db.get_source(user.user_id, source_ref).await else {
        return fall_back(&state, user.user_id, id, &kind).await;
    };
    match fetch_library_image(&source, &pick.image_key, max_px(&kind, false)).await {
        Ok((bytes, ct)) => image_response(bytes, ct, "private, max-age=604800"),
        Err(e) => {
            tracing::debug!("🖼️ Chosen picture unavailable from {}: {}", source.label, e);
            fall_back(&state, user.user_id, id, &kind).await
        }
    }
}

/// A picker tile from one of your servers, proxied.
///
/// Session-only. Takes a candidate's `source_ref` and `key`; the key is checked
/// against the shape that server offers before anything is asked of it.
#[utoipa::path(
    get,
    path = "/api/v1/artists/{id}/pictures/{kind}/preview",
    tag = "Library",
    params(
        ("id" = i64, Path, description = "The `artists` row."),
        ("kind" = String, Path, description = "`portrait` or `wide`."),
        PicturePreviewQuery,
    ),
    responses(
        (status = 200, description = "Image bytes, cached privately for an hour.", body = Bytes, content_type = "image/*"),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn preview_picture(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path((_id, kind)): Path<(i64, String)>,
    Query(q): Query<PicturePreviewQuery>,
) -> Response {
    if !kind_ok(&kind) {
        return bad_kind();
    }
    let Ok(Some(source)) = state.db.get_source(user.user_id, q.source_ref).await else {
        return not_found("That server is not connected");
    };
    if library_key(&source.kind, &q.key) != Some(false) {
        return bad(StatusCode::BAD_REQUEST, "That is not a picture that server holds");
    }
    match fetch_library_image(&source, &q.key, max_px(&kind, true)).await {
        Ok((bytes, ct)) => image_response(bytes, ct, "private, max-age=3600"),
        Err(e) => not_found(&format!("Could not fetch that picture: {e}")),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_pictures,
    choose_picture,
    clear_picture,
    upload_picture,
    serve_picture,
    preview_picture,
))]
pub struct Api;

pub fn create_pictures_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/artists/:id/pictures", get(list_pictures))
        .route(
            "/api/v1/artists/:id/pictures/:kind",
            get(serve_picture).put(choose_picture).delete(clear_picture),
        )
        .route("/api/v1/artists/:id/pictures/:kind/upload", post(upload_picture))
        .route("/api/v1/artists/:id/pictures/:kind/preview", get(preview_picture))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key comes back from the browser, so it is held to the shape each
    /// lister writes — the thing that keeps "choose a picture" from becoming
    /// "make my server fetch this".
    #[test]
    fn a_library_key_must_have_its_servers_shape() {
        assert_eq!(
            library_key("plex", "/library/metadata/12/file?url=upload%3A%2F%2Fx"),
            Some(false)
        );
        assert_eq!(library_key("plex", "/library/metadata/12/../../:/prefs"), None);
        assert_eq!(library_key("plex", "/:/prefs"), None);
        assert_eq!(library_key("plex", "https://metadata-static.plex.tv/a.jpg"), Some(false));
        assert_eq!(library_key("plex", "file:///etc/passwd"), None);

        assert_eq!(library_key("jellyfin", "a1b2c3/Primary/0"), Some(false));
        assert_eq!(library_key("emby", "a1b2c3/Backdrop/2"), Some(false));
        assert_eq!(library_key("jellyfin", "a1b2c3/Primary/0/../x"), None);
        assert_eq!(library_key("jellyfin", "../Primary/0"), None);
        assert_eq!(library_key("jellyfin", "a1b2c3/Logo/0"), None);
        assert_eq!(library_key("jellyfin", "https://assets.fanart.tv/a.jpg"), Some(true));

        assert_eq!(library_key("navidrome", "ar-2f8a_0"), Some(false));
        assert_eq!(library_key("navidrome", "ar-2f8a/../x"), None);
        assert_eq!(library_key("roon", "anything"), None);
    }

    #[test]
    fn a_deezer_choice_must_be_on_deezers_cdn() {
        assert!(deezer_url_ok("https://e-cdns-images.dzcdn.net/images/artist/abc/1000x1000.jpg"));
        assert!(!deezer_url_ok("http://e-cdns-images.dzcdn.net/images/artist/abc.jpg"));
        assert!(!deezer_url_ok("https://dzcdn.net.example.com/a.jpg"));
        assert!(!deezer_url_ok("https://example.com/a.jpg"));
    }

    /// The version is what lets the serve route cache for a week: a new choice
    /// is a new URL rather than the same URL with different bytes.
    #[test]
    fn a_library_choice_is_served_from_a_versioned_url() {
        assert_eq!(
            proxy_url(7, "wide", 1700000000),
            "/api/v1/artists/7/pictures/wide?v=1700000000"
        );
        assert_eq!(
            preview_url(7, "portrait", 3, "/library/metadata/1/thumb?x=1&y=2"),
            "/api/v1/artists/7/pictures/portrait/preview?source_ref=3&key=%2Flibrary%2Fmetadata%2F1%2Fthumb%3Fx%3D1%26y%3D2"
        );
    }
}
