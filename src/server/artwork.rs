//! Artwork proxy for listens that came from a user's own media server.
//!
//! Library artwork is the best source there is — it's the cover of the exact
//! file that was played, needs no external lookup, and costs no third party
//! anything. But the browser can't fetch it: a LAN Navidrome isn't reachable
//! from wherever the user has Tapedeck open, and handing the page the
//! credentials that *would* reach it is not an option.
//!
//! So Tapedeck fetches it server-side and streams it back. The stored
//! `artwork_url` for these listens is a path here rather than a remote URL.
//!
//! Two rules this endpoint keeps:
//!
//! * **Scoped to the caller.** The scrobble is looked up by `(id, user_id)`, so
//!   one user can't walk ids to pull images through someone else's credentials.
//! * **Cached hard.** Library art never changes for a given item, so a long
//!   `Cache-Control` keeps a scrolling history from re-proxying every row.

use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post, put},
    Router,
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

use super::auth::{AuthUser, ReadAuth, WriteAuth};
use super::models::ErrorResponse;
use crate::openapi::{BadRequest, Bytes, FileUpload, NotFound, Unauthorized};

/// An album cover, set or cleared. Null when cleared — the page then falls
/// back to the cover from your listens.
#[derive(Serialize, ToSchema)]
pub struct CoverSet {
    pub artwork_url: Option<String>,
}
use super::AppState;
use crate::sources::{MediaBrowserSource, NavidromeSource, PlexFilters, PlexSource};

pub(super) fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { code: 404, error: msg.into() })).into_response()
}

/// Recover the remote item id from a listen's `source_id`.
///
/// The three formats disagree about which end is safe to split, so this can't
/// be one rule:
///
/// * `plex-<ratingKey>-<suffix>` — the suffix is `np` for now-playing but
///   `scrobble-<timestamp>` for a real listen, i.e. *two* segments. Splitting
///   from the right yielded `<ratingKey>-scrobble` for every scrobble Plex ever
///   produced, so the proxy asked the server for an item that doesn't exist and
///   every polled listen fell back to the lettered tile. Rating keys are
///   numeric, so split from the left instead.
/// * `jellyfin-<itemId>-<device>` — device names contain dashes
///   (`Living-Room-Mac`), and item ids are dashless hex. Left again.
/// * `navidrome-<mediaFileId>-<submissionTime>` — ids are UUIDs, full of
///   dashes, and the suffix is a single numeric segment. This one must split
///   from the right.
pub(crate) fn remote_item_id(source_name: &str, source_id: &str) -> Option<String> {
    let (prefix, from_left) = match source_name {
        "navidrome" => ("navidrome-", false),
        "Plex" | "plex" => ("plex-", true),
        "jellyfin" | "Jellyfin" => ("jellyfin-", true),
        _ => return None,
    };
    let rest = source_id.strip_prefix(prefix)?;
    let id = if from_left { rest.split_once('-')?.0 } else { rest.rsplit_once('-')?.0 };
    (!id.is_empty()).then(|| id.to_string())
}

/// Cover art for a listen, proxied.
///
/// Scope: `read` *(since v0.120)*. Proxies the user's own library server, which the browser
/// usually cannot reach and must not hold credentials for.
#[utoipa::path(
    get,
    path = "/api/v1/art/{id}",
    tag = "Library",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = i64, Path, description = "The listen.")),
    responses(
        (status = 200, description = "Image bytes, `Cache-Control: private` for a week.", body = Bytes, content_type = "image/*"),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No artwork. The client falls back to a lettered tile.", body = ErrorResponse),
    )
)]
async fn scrobble_art(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let Ok(Some(scrobble)) = state.db.get_scrobble(user.user_id, id).await else {
        return not_found("No such listen");
    };

    let Some(item_id) = remote_item_id(&scrobble.source_name, &scrobble.source_id) else {
        return not_found("That listen didn't come from a media server");
    };

    // `source_ref` is what makes this work with two servers of the same kind:
    // the listen records exactly which one it came from, so there's nothing to
    // guess. Listens predating the column have none and can't be resolved.
    let Some(source_ref) = scrobble.source_ref else {
        return not_found(
            "This listen predates per-source artwork; re-fetch artwork to resolve it",
        );
    };
    let Ok(Some(source)) = state.db.get_source(user.user_id, source_ref).await else {
        return not_found("That source is no longer connected");
    };
    let egress = source.egress();
    if let Err(e) = egress.check_url(&source.url) {
        return not_found(&format!("Could not fetch artwork: {e}"));
    }

    let fetched = match source.kind.as_str() {
        "navidrome" => {
            let (Some(u), Some(p)) = (source.credential("username"), source.credential("password"))
            else {
                return not_found("Navidrome credentials are missing");
            };
            NavidromeSource::new(source.url.clone(), u, p, egress).fetch_cover_art(&item_id).await
        }
        "jellyfin" => {
            let Some(key) = source.credential("api_key") else {
                return not_found("Jellyfin API key is missing");
            };
            MediaBrowserSource::jellyfin(source.url.clone(), key, String::new(), egress)
                .fetch_cover_art(&item_id)
                .await
        }
        _ => {
            let Some(token) = source.credential("token") else {
                return not_found("Plex token is missing");
            };
            PlexSource::with_filters(source.url.clone(), token, PlexFilters::default(), egress)
                .fetch_cover_art(&item_id)
                .await
        }
    };

    match fetched {
        Ok((bytes, content_type)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                // Immutable for a given library item; without this every scroll
                // re-proxies each row through the user's media server.
                (header::CACHE_CONTROL, "private, max-age=604800".to_string()),
            ],
            Body::from(bytes),
        )
            .into_response(),
        // A 404 lets the UI fall back to the lettered tile, which is the right
        // outcome when a server is off or the item was deleted.
        Err(e) => not_found(&format!("Could not fetch artwork: {e}")),
    }
}

/// Find an artist in one of the user's own libraries and stream their portrait.
///
/// The same trade the cover proxy makes: the picture lives on a LAN server the
/// browser can't reach, and handing the page those credentials is not an
/// option, so Tapedeck fetches it. Sources are tried in turn and the first hit
/// wins — a portrait is a portrait, and asking the rest costs the user's own
/// servers a request for nothing.
pub(super) async fn artist_image_from_library(
    state: &AppState,
    user_id: i64,
    name: &str,
) -> Option<(Vec<u8>, String)> {
    let sources = state.db.list_sources_for_user(user_id).await.ok()?;
    for source in sources.iter().filter(|s| s.enabled) {
        let egress = source.egress();
        if egress.check_url(&source.url).is_err() {
            continue;
        }
        let found = match source.kind.as_str() {
            "plex" => {
                let token = source.credential("token")?;
                let plex = PlexSource::with_filters(
                    source.url.clone(),
                    token,
                    PlexFilters::default(),
                    egress,
                );
                match plex.find_artist(name).await {
                    Ok(Some(key)) => plex.fetch_cover_art(&key).await.ok(),
                    _ => None,
                }
            }
            "jellyfin" => {
                let key = source.credential("api_key")?;
                let jf =
                    MediaBrowserSource::jellyfin(source.url.clone(), key, String::new(), egress);
                match jf.find_artist(name).await {
                    Ok(Some(id)) => jf.fetch_cover_art(&id).await.ok(),
                    _ => None,
                }
            }
            // Navidrome's Subsonic API can answer this too, but it isn't wired
            // up yet — leaving it out is better than an untested path that
            // silently returns nothing.
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// An artist portrait, proxied.
///
/// Scope: `read` *(since v0.120)*.
#[utoipa::path(
    get,
    path = "/api/v1/artists/img/{id}",
    tag = "Library",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = i64, Path, description = "The `artists` row — `artist_row_id` on an artist page.")),
    responses(
        (status = 200, description = "Image bytes, `Cache-Control: private` for a week.", body = Bytes, content_type = "image/*"),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No portrait yet. Retried later — the answer changes when the artist is added to a library.", body = ErrorResponse),
    )
)]
async fn artist_image(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> Response {
    let Ok(Some((name, _))) = state.db.artist_by_id(id).await else {
        return not_found("No such artist");
    };

    match artist_image_from_library(&state, user.user_id, &name).await {
        Some((bytes, content_type)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "private, max-age=604800".to_string()),
            ],
            Body::from(bytes),
        )
            .into_response(),
        // 404 lets the UI fall back to the lettered tile, which is right when a
        // server is off or the artist isn't in the library.
        None => not_found("No portrait for that artist in your libraries"),
    }
}

#[derive(serde::Deserialize, ToSchema)]
pub struct ArtistImageUpdate {
    /// An `http(s)` URL, or a path on this server such as
    /// `/api/v1/artists/img/12` or `/api/v1/art/914` — which is how you point
    /// at your own library through the proxy. Empty clears it.
    pub url: String,
}

/// `Some(None)` clears the picture; the outer `None` is a 400 — that isn't a
/// URL at all. Relative paths are allowed on purpose: `/api/v1/artists/img/12`
/// and `/api/v1/art/914` are both this server, and pasting one is how a user
/// points at their own library through the proxy.
pub(super) fn validate_image_url(raw: &str) -> Option<Option<&str>> {
    let url = raw.trim();
    if url.is_empty() {
        return Some(None);
    }
    (url.starts_with("http://") || url.starts_with("https://") || url.starts_with('/'))
        .then_some(Some(url))
}

fn not_an_image_url() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse { code: 400, error: "That doesn't look like an image URL".into() }),
    )
        .into_response()
}

// PUT /api/v1/albums/:id/cover — set the album's cover by hand.
//
// The counterpart of the artist override. Without it an album wears whatever
// artwork the newest listen of it happens to carry, which is right most of the
// time and unfixable when it isn't: a compilation's cover on a single, or the
// deluxe edition's when you own the original. The operator gets the last word,
// the same rule the artist portrait follows.
//
// The id is an *entity* id, and entities are shared across users with no
// `user_id` to scope by — so this is a household-wide edit, like the artist
// portrait and the entity table generally. Worth knowing before Phase 6.
/// Override an album cover.
///
/// Session-only. The counterpart of the artist portrait override: an album
/// otherwise wears whatever artwork the newest listen of it happens to
/// carry, which is unfixable when it is the wrong edition's.
///
/// 400s when the id is not a **release** entity — an artist's picture has
/// its own endpoint, and two places to set one would leave no rule about
/// which wins. Like every entity write, this is not scoped per user:
/// `entities` is shared.
#[utoipa::path(
    put,
    path = "/api/v1/albums/{id}/cover",
    tag = "Library",
    params(("id" = i64, Path, description = "The album's entity id.")),
    request_body = ArtistImageUpdate,
    responses(
        (status = 200, description = "Saved. Null when cleared.", body = CoverSet),
        (status = 400, description = "Not a URL.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such album, or that entity is not a release.", body = ErrorResponse),
    )
)]
async fn set_album_cover(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ArtistImageUpdate>,
) -> Response {
    let _ = &user;
    let Some(value) = validate_image_url(&body.url) else {
        return not_an_image_url();
    };
    let previous = state.db.entity_image(id).await.ok().flatten();
    match state.db.set_entity_image(id, value).await {
        Ok(true) => {
            forget_previous(&state, previous.as_deref()).await;
            (StatusCode::OK, Json(CoverSet { artwork_url: value.map(str::to_string) }))
                .into_response()
        }
        // Either there is no such entity or it isn't a release. Both are the
        // same mistake from the client's side, and an artist picture has its
        // own endpoint for the reason the query comment gives.
        Ok(false) => not_found("No such album"),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// ── Uploading a picture instead of pasting a URL ──
//
// The three overrides above take a URL, which covers a picture that is already
// somewhere on the web and covers nothing else. A photograph of a record you
// own, a scan of a sleeve, a band shot that exists only on your disk — those
// had no way in at all, and telling someone to host an image somewhere first
// is not an answer for a self-hosted journal.
//
// The shelf already solved this for scans and the profile for avatars, so the
// rules below are theirs: the extension comes from the *declared* content type
// and never from the client's filename, the stored name is random, and the
// path on disk is built here rather than taken from anything a request said.

/// A picture is a cover or a portrait, not a 600dpi booklet page — the shelf's
/// 24 MB is sized for scanning a gatefold, and nothing here needs that.
const MAX_ART_BYTES: usize = 8 * 1024 * 1024;

/// Where an uploaded picture is served from. Kept as a constant because the
/// upload writes it into a database column and the serve route has to
/// recognise it again to know a file is its own to delete.
pub(super) const UPLOAD_PREFIX: &str = "/api/v1/art/upload/";

/// Is this a name this module wrote?
///
/// The serve route takes the filename from the **request path**, never from the
/// stored column — `validate_image_url` accepts any `/`-prefixed string, so a
/// user can put whatever they like in `image_url`, and a route that read a path
/// out of it would serve arbitrary files out of the uploads directory. Here the
/// name has to be exactly what `random_name` produces plus a known extension,
/// so there is nothing to traverse with.
fn safe_upload_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    stem.len() == 24
        && stem.bytes().all(|b| b.is_ascii_hexdigit())
        && ["jpg", "png", "webp", "gif", "avif", "tif"].contains(&ext)
}

/// Take the one image out of a multipart body.
///
/// Same shape as the shelf's and the avatar's, and deliberately not shared with
/// them: those two each carry extra fields (a scan has a kind and a caption)
/// and folding three callers into one helper would mean a parameter for every
/// difference.
pub(super) async fn read_one_image(
    multipart: &mut Multipart,
) -> Result<(Vec<u8>, &'static str), Response> {
    let mut file: Option<(Vec<u8>, String)> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        let declared = field.content_type().unwrap_or("").to_string();
        let named = field.file_name().unwrap_or("").to_string();
        let mime = if super::physical::image_ext(&declared).is_some() {
            declared
        } else {
            mime_guess::from_path(&named).first_raw().unwrap_or("").to_string()
        };
        if let Ok(b) = field.bytes().await {
            file = Some((b.to_vec(), mime));
            break;
        }
    }

    let Some((bytes, mime)) = file else {
        return Err(bad(StatusCode::BAD_REQUEST, "No image in the request"));
    };
    if bytes.len() > MAX_ART_BYTES {
        return Err(bad(StatusCode::PAYLOAD_TOO_LARGE, "That picture is larger than 8 MB"));
    }
    let Some(ext) = super::physical::image_ext(&mime) else {
        return Err(bad(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Only JPEG, PNG, WebP, GIF, AVIF and TIFF images can be uploaded",
        ));
    };
    Ok((bytes, ext))
}

pub(super) fn bad(code: StatusCode, msg: &str) -> Response {
    (code, Json(ErrorResponse { code: code.as_u16(), error: msg.into() })).into_response()
}

/// Write the bytes and hand back the URL to store.
///
/// Flat under `art/`, **not** under `art/<user>/` like the shelf's scans. An
/// album cover set here is shared — `entities` carries no `user_id` — and an
/// artist picture, though it belongs to one person since v0.124.0, is found
/// through that person's `artist_pictures` row rather than its path. The name
/// is random and unguessable either way, and one directory keeps
/// `serve_upload`'s single rule.
pub(super) async fn store_upload(
    state: &Arc<AppState>,
    bytes: &[u8],
    ext: &str,
) -> Result<String, Response> {
    let dir = state.uploads_dir.join("art");
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!("art upload: could not create {}: {}", dir.display(), e);
        return Err(bad(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not write to the uploads directory",
        ));
    }
    // A fresh random name per upload, so the `immutable` cache header on the
    // serve route stays honest: a replaced picture is a new URL rather than the
    // same URL with different bytes behind it.
    let name = format!("{}.{}", super::physical::random_name(), ext);
    if let Err(e) = tokio::fs::write(dir.join(&name), bytes).await {
        tracing::warn!("art upload: could not save: {}", e);
        return Err(bad(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the picture"));
    }
    Ok(format!("{UPLOAD_PREFIX}{name}"))
}

/// Delete the file a new value supersedes, if it was one of ours.
///
/// Called from the upload handlers *and* from the three URL setters: pasting a
/// link over an uploaded picture replaces it just as much as another upload
/// does, and leaving the file behind is how an uploads directory fills up with
/// images nothing references. Guarded on the prefix so it can only ever remove
/// something this module wrote.
pub(super) async fn forget_previous(state: &Arc<AppState>, previous: Option<&str>) {
    let Some(old) = previous.and_then(|p| p.strip_prefix(UPLOAD_PREFIX)) else { return };
    if safe_upload_name(old) {
        let _ = tokio::fs::remove_file(state.uploads_dir.join("art").join(old)).await;
    }
}

// GET /api/v1/art/upload/:name — serve an uploaded picture.
//
// Behind auth like everything else here, and cached `private` even though the
// picture is instance-wide: a shared cache is a different thing from a shared
// *instance*, and this endpoint is reached through the same session boundary
// as the artwork proxy.
//
// The name is validated rather than trusted. It is the only user-supplied part
// of the path this reads from, and `image_url` accepts arbitrary `/`-prefixed
// strings — so the check is what stops a crafted request walking out of the
// uploads directory.
/// Serve an uploaded picture.
///
/// Scope: `read` *(since v0.120)*. Cached `private, immutable` — the name is random per
/// upload, so the bytes behind a given URL never change.
///
/// `name` is **validated, not trusted**: it must be exactly the 24 hex
/// characters this server generates plus a known image extension. The
/// override columns accept any `/`-prefixed string, so a route that read a
/// path out of one would serve arbitrary files from the uploads directory.
#[utoipa::path(
    get,
    path = "/api/v1/art/upload/{name}",
    tag = "Library",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("name" = String, Path, description = "As it appears in the stored URL.")),
    responses(
        (status = 200, description = "Image bytes, `Cache-Control: private, immutable`.", body = Bytes, content_type = "image/*"),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such picture, or the file is no longer on this server.", body = ErrorResponse),
    )
)]
async fn serve_upload(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(name): Path<String>,
) -> Response {
    let _ = &user;
    if !safe_upload_name(&name) {
        return not_found("No such picture");
    }
    match tokio::fs::read(state.uploads_dir.join("art").join(&name)).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    mime_guess::from_path(&name).first_raw().unwrap_or("application/octet-stream"),
                ),
                (header::CACHE_CONTROL, "private, max-age=31536000, immutable"),
            ],
            Body::from(bytes),
        )
            .into_response(),
        // The row still points at it but the file is gone. 404 lets the UI fall
        // back to the lettered tile, the same as a portrait nobody has.
        Err(_) => not_found("That picture is no longer on this server"),
    }
}

// POST /api/v1/albums/:id/cover/upload — upload a cover.
//
// Replaces the derived cover exactly as a pasted URL does, which is what makes
// the page offer *Reset* (back to whatever the listens carry) rather than
// *Clear*. The id is an entity id and entities are shared, so this is a
// household-wide edit — the same caveat the URL setter carries, and the page
// says so.
/// Upload an album cover.
///
/// Session-only. Same rules as the artist uploads. Takes an **entity** id.
///
/// Replaces the derived cover the way the `PUT` does, so the page still
/// offers *Reset* (back to whatever the listens carry) rather than *Clear*.
/// Entities are shared, so this is an **instance-wide** edit.
#[utoipa::path(
    post,
    path = "/api/v1/albums/{id}/cover/upload",
    tag = "Library",
    params(("id" = i64, Path, description = "The album's entity id.")),
    request_body(content = FileUpload, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "Saved. The body carries the new URL, which is a path on this server.", body = CoverSet),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 413, description = "Larger than 8 MB.", body = ErrorResponse),
        (status = 415, description = "Not an image type this server stores (JPEG, PNG, WebP, GIF, AVIF, TIFF).", body = ErrorResponse),
    )
)]
async fn upload_album_cover(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> Response {
    let _ = &user;
    let previous = state.db.entity_image(id).await.ok().flatten();
    let (bytes, ext) = match read_one_image(&mut multipart).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let url = match store_upload(&state, &bytes, ext).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    match state.db.set_entity_image(id, Some(&url)).await {
        Ok(true) => {
            forget_previous(&state, previous.as_deref()).await;
            (StatusCode::OK, Json(CoverSet { artwork_url: Some(url) })).into_response()
        }
        // The bytes are already on disk by this point, so a row that refused
        // the write leaves a file nothing references. Discarding it here is
        // what keeps "files in art/" equal to "URLs in the database" — an
        // orphan is invisible until the disk fills.
        Ok(false) => {
            forget_previous(&state, Some(&url)).await;
            not_found("No such album")
        }
        Err(e) => {
            forget_previous(&state, Some(&url)).await;
            bad(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

// ── The cover a player is holding ──
//
// A scrobbling client knows names and never entity ids, which rules out the
// routes that already exist: `PUT /api/v1/albums/:id/cover` takes an id, and
// `GET /api/v1/resolve` would *create* the entity as a side effect of asking —
// a write on a read path, which is fine for a browser that is about to annotate
// something and wrong for a player asking whether to bother.

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AlbumArtQuery {
    /// As you would submit it.
    pub artist: String,
    /// As you would submit it.
    pub album: String,
}

/// Whether the album already has a cover.
#[derive(Serialize, ToSchema)]
pub struct ArtHeld {
    pub held: bool,
}

/// A cover a player read off its own files, offered as `multipart/form-data`.
#[derive(ToSchema)]
#[allow(dead_code)] // describes a multipart body; never constructed
pub struct LibraryArtUpload {
    /// The album's artist, as you would submit it.
    artist: String,
    /// The album, as you would submit it.
    album: String,
    /// The image. Its declared `Content-Type` decides how it is stored.
    #[schema(value_type = String, format = Binary)]
    file: Vec<u8>,
    /// Accepted and **not read** — the cover is keyed on names like every other
    /// album query. Send the MBID on a listen's `additional_info` instead.
    release_mbid: Option<String>,
}

/// What became of an offered cover.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
#[allow(dead_code)] // describes the two bodies `upload_album_art_from_library` sends
pub enum LibraryArtStored {
    /// Something filled the cover since the client asked; nothing was stored.
    AlreadyHeld {
        /// Always `true`.
        already_held: bool,
    },
    Stored {
        /// Where the stored picture is served from.
        artwork_url: String,
        /// Your listens of the album that now carry it.
        listens_covered: u64,
    },
}

// Whether this album already has a cover, so the common case costs one small
// `GET` instead of a megabyte of JPEG.
//
// **`write` scope, not `read`** — deliberately, and it is the doc's argument
// rather than laziness: this exists only to decide a write, and making it
// `read` would force every scrobbling token that wants to offer a sleeve to
// also carry read access to the whole listening history.
/// Does this album already have a cover?
///
/// Scope: `write`.
///
/// The precondition of an upload, so the common case costs one small `GET`
/// rather than a megabyte of JPEG. A client reads the picture off disk only
/// when this answers `false`.
///
/// **`write`, not `read`** — this exists only to decide a write, and
/// requiring `read` would force a scrobbling token that wants to offer a
/// sleeve to carry read access to the whole listening history as well.
///
/// `held` is true when any listen of this album carries artwork (a
/// Cover Art Archive id counts, `-1` excluded), **or** when a cover has been
/// set by hand on the album — one set that way is what the album page
/// prefers over everything else, so answering `false` would invite a
/// picture that would never be shown.
///
/// Deliberately **not** `album_artwork`'s question, which excludes skips
/// because its job is choosing a cover to display. An album whose only
/// listens are skips would otherwise answer "nothing held" for ever and the
/// same sleeve would be re-uploaded on every run.
///
/// Creates nothing. `GET /api/v1/resolve` would mint the entity as a side
/// effect of being asked, which is right for a browser about to annotate
/// something and wrong for a player asking whether to bother.
#[utoipa::path(
    get,
    path = "/api/v1/art/library",
    tag = "Library",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(AlbumArtQuery),
    responses(
        (status = 200, description = "Whether a cover is already held.", body = ArtHeld),
        (status = 400, description = "A blank artist or album.", body = ErrorResponse),
        (status = 403, response = crate::openapi::Forbidden),
        (status = 401, response = Unauthorized),
    )
)]
async fn album_art_held(
    State(state): State<Arc<AppState>>,
    auth: WriteAuth,
    Query(q): Query<AlbumArtQuery>,
) -> impl IntoResponse {
    if q.artist.trim().is_empty() || q.album.trim().is_empty() {
        return bad(StatusCode::BAD_REQUEST, "An artist and an album, please");
    }
    match state.db.album_art_held(auth.user.user_id, &q.artist, &q.album).await {
        Ok(held) => (StatusCode::OK, Json(ArtHeld { held })).into_response(),
        // Never reported as `held: false`, which would have the client send a
        // picture on the strength of a failed read — the rule this codebase
        // learned the hard way from `needs_setup`.
        Err(e) => {
            tracing::warn!("art/library: could not check: {}", crate::logging::error_chain(&e));
            bad(StatusCode::INTERNAL_SERVER_ERROR, "Could not check for a cover")
        }
    }
}

// Take a cover a player read off its own files.
//
// Writes `scrobbles.artwork_url` rather than a cover override: that column is
// the artwork backfill's queue and `album_artwork` already reads it back, so a
// sleeve arriving with the listens is exactly what a Plex-sourced listen
// carries. An override would claim somebody set it by hand.
/// Offer a cover read from the client's own files.
///
/// Scope: `write`.
///
/// Writes `scrobbles.artwork_url` on every listen of this album that has
/// none, and nothing else. That column **is** the artwork backfill's queue
/// and the album page reads it back, so a sleeve arriving with the listens
/// is exactly what a Plex-sourced listen carries. A cover *override* would
/// claim somebody set it by hand, and nobody did.
///
/// Rows holding the `artwork_url = ''` tombstone are filled too, and that
/// is the point rather than an oversight: the tombstone means "asked every
/// provider, nobody had a picture", and the premise here is that the
/// listener's own file does. Skipped listens are filled as well — artwork
/// is not a count.
///
/// The held check runs again before the bytes are written, so a cover that
/// arrived between the `GET` and this costs no file on disk and is reported
/// rather than treated as an error.
#[utoipa::path(
    post,
    path = "/api/v1/art/library",
    tag = "Library",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    request_body(content = LibraryArtUpload, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "Stored, or already held.", body = LibraryArtStored),
        (status = 400, description = "No image, or a blank artist or album.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, response = crate::openapi::Forbidden),
        (status = 413, description = "Larger than 8 MB.", body = ErrorResponse),
        (status = 415, description = "Not an image type this server stores.", body = ErrorResponse),
    )
)]
async fn upload_album_art_from_library(
    State(state): State<Arc<AppState>>,
    auth: WriteAuth,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // **`read_one_image` cannot be reused.** It takes the first field that
    // yields bytes and stops, and this body carries text fields before the
    // file — so it would read "Charlie Puth" as the picture and refuse it as an
    // unsupported type, which is a confusing 415 about a request that was fine.
    let mut artist = String::new();
    let mut album = String::new();
    let mut file: Option<(Vec<u8>, String)> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("").to_string().as_str() {
            "artist" => artist = field.text().await.unwrap_or_default(),
            "album" => album = field.text().await.unwrap_or_default(),
            "file" => {
                let declared = field.content_type().unwrap_or("").to_string();
                let named = field.file_name().unwrap_or("").to_string();
                let mime = if super::physical::image_ext(&declared).is_some() {
                    declared
                } else {
                    mime_guess::from_path(&named).first_raw().unwrap_or("").to_string()
                };
                if let Ok(b) = field.bytes().await {
                    file = Some((b.to_vec(), mime));
                }
            }
            // `release_mbid` arrives and is **not read**, which is said out
            // loud here and in the spec rather than left for a client author to
            // discover. There is nothing for it to decide: the cover is keyed
            // on names like every other album query, and writing it to
            // `mbid_release` would be a different claim entirely — the client's
            // tagging, which already has a path through `additional_info` on a
            // submission.
            _ => {}
        }
    }

    if artist.trim().is_empty() || album.trim().is_empty() {
        return bad(StatusCode::BAD_REQUEST, "An artist and an album, please");
    }
    let Some((bytes, mime)) = file else {
        return bad(StatusCode::BAD_REQUEST, "No image in the request");
    };
    // The same limits the browser uploads use, kept rather than re-derived.
    if bytes.len() > MAX_ART_BYTES {
        return bad(StatusCode::PAYLOAD_TOO_LARGE, "That picture is larger than 8 MB");
    }
    let Some(ext) = super::physical::image_ext(&mime) else {
        return bad(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Only JPEG, PNG, WebP, GIF, AVIF and TIFF images can be uploaded",
        );
    };

    // Asked again, because the client's own check was a separate request and
    // anything may have filled the cover since — the artwork backfill runs on
    // the engine tick. Checked *before* the bytes are written, so a race costs
    // no file on disk.
    match state.db.album_art_held(auth.user.user_id, &artist, &album).await {
        Ok(true) => {
            return (StatusCode::OK, Json(LibraryArtStored::AlreadyHeld { already_held: true }))
                .into_response()
        }
        Ok(false) => {}
        Err(e) => {
            tracing::warn!("art/library: could not check: {}", crate::logging::error_chain(&e));
            return bad(StatusCode::INTERNAL_SERVER_ERROR, "Could not check for a cover");
        }
    }

    let url = match store_upload(&state, &bytes, ext).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    match state.db.set_album_artwork_from_library(auth.user.user_id, &artist, &album, &url).await {
        Ok(n) => (
            StatusCode::OK,
            Json(LibraryArtStored::Stored { artwork_url: url, listens_covered: n }),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!("art/library: could not store: {}", crate::logging::error_chain(&e));
            bad(StatusCode::INTERNAL_SERVER_ERROR, "Could not record that cover")
        }
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    scrobble_art,
    artist_image,
    set_album_cover,
    serve_upload,
    upload_album_cover,
    album_art_held,
    upload_album_art_from_library,
))]
pub struct Api;

pub fn create_artwork_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/art/:id", get(scrobble_art))
        // Choosing an artist's picture is per person now, in `pictures`; this
        // stays as the library proxy the portrait backfill points at.
        .route("/api/v1/artists/img/:id", get(artist_image))
        .route("/api/v1/albums/:id/cover", put(set_album_cover))
        // Uploads sit one segment deeper than the URL setters they mirror, so
        // matchit tells them apart without any ordering rule.
        .route("/api/v1/art/upload/:name", get(serve_upload))
        .route("/api/v1/albums/:id/cover/upload", post(upload_album_cover))
        .route("/api/v1/art/library", get(album_art_held).post(upload_album_art_from_library))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serve route reads a filename out of the request path, and
    /// `validate_image_url` lets a user store **any** `/`-prefixed string in
    /// `image_url` — so if the name were trusted, a crafted request could walk
    /// out of the uploads directory and into another user's shelf scans, which
    /// live at a guessable `shelf/<user>/<release>/`.
    ///
    /// Nothing errors when this is wrong; it just serves a file it shouldn't.
    #[test]
    fn only_a_name_this_module_wrote_is_ever_served() {
        // 24 hex characters is exactly what `random_name` produces.
        assert!(safe_upload_name("0123456789abcdef01234567.jpg"));
        assert!(safe_upload_name("0123456789abcdef01234567.webp"));

        for bad in [
            "../../../etc/passwd",
            "../avatars/1/0123456789abcdef01234567.jpg",
            "0123456789abcdef01234567.jpg/../../secret",
            // Right shape, wrong everything else.
            "0123456789abcdef01234567.svg",
            "0123456789abcdef01234567.html",
            "0123456789abcdef0123456.jpg",   // 23
            "0123456789abcdef012345678.jpg", // 25
            "0123456789abcdefzzzzzzzz.jpg",  // not hex
            "0123456789abcdef01234567",      // no extension
            ".jpg",
            "",
        ] {
            assert!(!safe_upload_name(bad), "{bad:?} must not be servable");
        }
    }

    /// `forget_previous` deletes files, so it must only ever recognise a value
    /// this module wrote. A remote URL, a library proxy path, or a hand-typed
    /// string are all things it has to leave completely alone.
    #[test]
    fn only_our_own_uploads_are_ever_deleted() {
        let ours = format!("{UPLOAD_PREFIX}0123456789abcdef01234567.jpg");
        assert_eq!(
            ours.strip_prefix(UPLOAD_PREFIX).map(safe_upload_name),
            Some(true),
            "an uploaded picture is recognised"
        );
        for foreign in [
            "https://example.com/pic.jpg",
            "/api/v1/artists/img/12",
            "/api/v1/art/914",
            "/api/v1/profile/avatar",
        ] {
            assert!(
                foreign.strip_prefix(UPLOAD_PREFIX).is_none(),
                "{foreign} is not ours to delete"
            );
        }
    }

    #[test]
    fn extracts_navidrome_and_plex_ids() {
        assert_eq!(
            remote_item_id("navidrome", "navidrome-abc123-1700000000").as_deref(),
            Some("abc123")
        );
        assert_eq!(remote_item_id("navidrome", "navidrome-abc123-np").as_deref(), Some("abc123"));
        assert_eq!(remote_item_id("Plex", "plex-55821-np").as_deref(), Some("55821"));
        // The suffix a scrobble carries is `scrobble-<ts>` — two segments. This
        // assertion used to pin the broken `55821-scrobble`, which is why the
        // proxy 404'd on every polled listen without anyone noticing.
        assert_eq!(
            remote_item_id("Plex", "plex-55821-scrobble-1700000000").as_deref(),
            Some("55821")
        );
        // Jellyfin's suffix is a device name, and those contain dashes.
        assert_eq!(
            remote_item_id("jellyfin", "jellyfin-a1b2c3d4-Living-Room-Mac").as_deref(),
            Some("a1b2c3d4")
        );
    }

    #[test]
    fn keeps_dashes_inside_an_opaque_id() {
        // Navidrome ids are UUID-ish and full of dashes; splitting on the first
        // one would truncate the id and fetch the wrong item (or nothing).
        let id = "6f2a1b3c-9d4e-4f5a-8b6c-7d8e9f0a1b2c";
        assert_eq!(
            remote_item_id("navidrome", &format!("navidrome-{id}-1700000000")).as_deref(),
            Some(id)
        );
    }

    #[test]
    fn ignores_sources_with_no_library_behind_them() {
        // An ingested or imported listen has no media server to ask.
        assert_eq!(remote_item_id("ingest:pano", "ingest-1-1700000000-Title"), None);
        assert_eq!(remote_item_id("rockbox", "rockbox-abc"), None);
        assert_eq!(remote_item_id("import", "import-x-1"), None);
    }

    #[test]
    fn malformed_ids_are_rejected_rather_than_guessed() {
        assert_eq!(remote_item_id("navidrome", "navidrome-"), None);
        assert_eq!(remote_item_id("navidrome", "navidrome-onlyonepart"), None);
        assert_eq!(remote_item_id("navidrome", "something-else-entirely"), None);
    }
}
