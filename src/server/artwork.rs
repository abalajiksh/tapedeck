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
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, put},
    Router,
};
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::sources::{JellyfinSource, NavidromeSource, PlexFilters, PlexSource};

fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(serde_json::json!(ErrorResponse { code: 404, error: msg.into() })))
        .into_response()
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
fn remote_item_id(source_name: &str, source_id: &str) -> Option<String> {
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

/// GET /api/v1/art/:scrobble_id
async fn scrobble_art(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
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

    let fetched = match source.kind.as_str() {
        "navidrome" => {
            let (Some(u), Some(p)) = (source.credential("username"), source.credential("password"))
            else {
                return not_found("Navidrome credentials are missing");
            };
            NavidromeSource::new(source.url.clone(), u, p).fetch_cover_art(&item_id).await
        }
        "jellyfin" => {
            let Some(key) = source.credential("api_key") else {
                return not_found("Jellyfin API key is missing");
            };
            JellyfinSource::new(source.url.clone(), key, String::new())
                .fetch_cover_art(&item_id)
                .await
        }
        _ => {
            let Some(token) = source.credential("token") else {
                return not_found("Plex token is missing");
            };
            PlexSource::with_filters(source.url.clone(), token, PlexFilters::default())
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
async fn artist_image_from_library(
    state: &AppState,
    user_id: i64,
    name: &str,
) -> Option<(Vec<u8>, String)> {
    let sources = state.db.list_sources_for_user(user_id).await.ok()?;
    for source in sources.iter().filter(|s| s.enabled) {
        let found = match source.kind.as_str() {
            "plex" => {
                let token = source.credential("token")?;
                let plex =
                    PlexSource::with_filters(source.url.clone(), token, PlexFilters::default());
                match plex.find_artist(name).await {
                    Ok(Some(key)) => plex.fetch_cover_art(&key).await.ok(),
                    _ => None,
                }
            }
            "jellyfin" => {
                let key = source.credential("api_key")?;
                let jf = JellyfinSource::new(source.url.clone(), key, String::new());
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

/// GET /api/v1/artists/img/:id
async fn artist_image(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
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

#[derive(serde::Deserialize)]
pub struct ArtistImageUpdate {
    /// A URL to use, or empty to clear back to the lettered tile.
    pub url: String,
}

/// `Some(None)` clears the picture; the outer `None` is a 400 — that isn't a
/// URL at all. Relative paths are allowed on purpose: `/api/v1/artists/img/12`
/// and `/api/v1/art/914` are both this server, and pasting one is how a user
/// points at their own library through the proxy.
fn validate_image_url(raw: &str) -> Option<Option<&str>> {
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
        Json(serde_json::json!(ErrorResponse {
            code: 400,
            error: "That doesn't look like an image URL".into()
        })),
    )
        .into_response()
}

/// PUT /api/v1/artists/img/:id — set the portrait by hand.
///
/// Tapedeck is a listening journal, so the operator gets the last word over
/// anything a provider or a library guessed. Stored as given, and it beats the
/// backfill because the backfill only ever fills a NULL.
async fn set_artist_image(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ArtistImageUpdate>,
) -> Response {
    let _ = &user;
    let Some(value) = validate_image_url(&body.url) else {
        return not_an_image_url();
    };
    match state.db.set_artist_image(id, value).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "image_url": value }))).into_response()
        }
        Ok(false) => not_found("No such artist"),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

/// PUT /api/v1/artists/landscape/:id — set the **wide** image by hand.
///
/// A separate route rather than a field on the one above, because the two are
/// set independently and from different places: a portrait usually arrives from
/// the user's own library or a provider, while nothing Tapedeck can reach
/// supplies a landscape — it is always chosen by hand. They are also different
/// *photographs* rather than two crops, which is why there are two columns.
///
/// Used by a Reports chapter, which gives its top artist a whole tile; a square
/// portrait there either pillarboxes or crops the head off, and the giant
/// initial that stood in for it was a placeholder, not a design.
async fn set_artist_landscape(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ArtistImageUpdate>,
) -> Response {
    let _ = &user;
    let Some(value) = validate_image_url(&body.url) else {
        return not_an_image_url();
    };
    match state.db.set_artist_landscape(id, value).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "landscape_url": value }))).into_response()
        }
        Ok(false) => not_found("No such artist"),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

/// PUT /api/v1/albums/:id/cover — set the album's cover by hand.
///
/// The counterpart of the artist override. Without it an album wears whatever
/// artwork the newest listen of it happens to carry, which is right most of the
/// time and unfixable when it isn't: a compilation's cover on a single, or the
/// deluxe edition's when you own the original. The operator gets the last word,
/// the same rule the artist portrait follows.
///
/// The id is an *entity* id, and entities are shared across users with no
/// `user_id` to scope by — so this is a household-wide edit, like the artist
/// portrait and the entity table generally. Worth knowing before Phase 6.
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
    match state.db.set_entity_image(id, value).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "artwork_url": value }))).into_response()
        }
        // Either there is no such entity or it isn't a release. Both are the
        // same mistake from the client's side, and an artist picture has its
        // own endpoint for the reason the query comment gives.
        Ok(false) => not_found("No such album"),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

pub fn create_artwork_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/art/:id", get(scrobble_art))
        .route("/api/v1/artists/img/:id", get(artist_image).put(set_artist_image))
        .route("/api/v1/artists/landscape/:id", axum::routing::put(set_artist_landscape))
        .route("/api/v1/albums/:id/cover", put(set_album_cover))
}

#[cfg(test)]
mod tests {
    use super::*;

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
