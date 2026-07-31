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
    routing::get,
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
/// Formats are `navidrome-<mediaFileId>-<submissionTime>` and
/// `plex-<ratingKey>-<suffix>`. Both ids are opaque strings that may themselves
/// contain dashes, so split off the known prefix and the known trailing segment
/// rather than splitting on every dash.
fn remote_item_id(source_name: &str, source_id: &str) -> Option<String> {
    let prefix = match source_name {
        "navidrome" => "navidrome-",
        "Plex" | "plex" => "plex-",
        "jellyfin" | "Jellyfin" => "jellyfin-",
        _ => return None,
    };
    let rest = source_id.strip_prefix(prefix)?;
    let (id, _suffix) = rest.rsplit_once('-')?;
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

pub fn create_artwork_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/art/:id", get(scrobble_art))
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
        assert_eq!(
            remote_item_id("Plex", "plex-55821-scrobble-1700000000").as_deref(),
            Some("55821-scrobble")
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
