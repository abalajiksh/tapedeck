use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

#[derive(Debug, Deserialize)]
pub struct ScrobblesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<String>,
    pub after: Option<i64>,
    pub before: Option<i64>,
}

/// GET /api/v1/scrobbles
async fn list_scrobbles(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(params): Query<ScrobblesQuery>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let offset = params.offset.unwrap_or(0).max(0);

    match state
        .db
        .get_recent_scrobbles(
            user.user_id,
            limit,
            offset,
            params.artist.as_deref(),
            params.album.as_deref(),
            params.track.as_deref(),
            params.after,
            params.before,
        )
        .await
    {
        Ok(scrobbles) => {
            let count = scrobbles.len();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "scrobbles": scrobbles, "count": count, "limit": limit, "offset": offset,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                code: 500,
                error: format!("Failed to fetch scrobbles: {}", e)
            })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub limit: Option<i64>,
}

/// GET /api/v1/search?q= — grouped matches from the caller's own history.
///
/// Scoped to `user.user_id` like every other read: on a shared instance one
/// person's library is not searchable by another.
async fn search(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let q = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(10).clamp(1, 50);

    match state.db.search_library(user.user_id, &q, limit).await {
        Ok((artists, albums, tracks)) => {
            let artists: Vec<_> = artists
                .into_iter()
                .map(|(name, plays)| serde_json::json!({ "name": name, "plays": plays }))
                .collect();
            let albums: Vec<_> = albums
                .into_iter()
                .map(|(name, artist, plays)| {
                    serde_json::json!({ "name": name, "artist": artist, "plays": plays })
                })
                .collect();
            let tracks: Vec<_> = tracks
                .into_iter()
                .map(|(title, artist, album, plays)| {
                    serde_json::json!({
                        "title": title, "artist": artist, "album": album, "plays": plays
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "artists": artists, "albums": albums, "tracks": tracks,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/v1/sanitize — what the sanitiser *would* change, changing nothing.
///
/// Always a preview first. Scrobble metadata that arrived from a real
/// MusicBrainz mapping is still data, and rewriting it silently would be a
/// worse failure than the mismatch it fixes.
async fn sanitize_preview(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let albums = state.db.albums_with_mixed_releases(user.user_id).await.unwrap_or_default();
    let artists = state.db.artists_with_featured_credits(user.user_id).await.unwrap_or_default();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "mixed_albums": albums,
            "featured_artists": artists,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct SanitizeRequest {
    /// Unify every listen of an album onto the release the majority agree on.
    #[serde(default)]
    pub albums: bool,
    /// Fold `Artist feat. X` into `Artist`, keeping the full credit.
    #[serde(default)]
    pub artists: bool,
    /// Limit to one album (`artist` + `album`) instead of the whole library.
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// POST /api/v1/sanitize — apply the fixes the preview listed.
async fn sanitize_apply(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SanitizeRequest>,
) -> impl IntoResponse {
    let mut album_rows = 0u64;
    let mut albums_fixed = 0u64;
    let mut artist_rows = 0u64;
    let mut artists_folded = 0u64;

    if body.albums {
        let candidates = match state.db.albums_with_mixed_releases(user.user_id).await {
            Ok(c) => c,
            Err(e) => return server_error(e.to_string()),
        };
        for c in candidates {
            // Scoped run: skip everything but the album that was asked for.
            if let (Some(a), Some(al)) = (body.artist.as_deref(), body.album.as_deref()) {
                if !c.artist.eq_ignore_ascii_case(a.trim())
                    || !c.album.eq_ignore_ascii_case(al.trim())
                {
                    continue;
                }
            }
            let Some(winner) = c.winner.as_deref() else { continue };
            match state.db.unify_album_release(user.user_id, &c.artist, &c.album, winner).await {
                Ok(n) if n > 0 => {
                    album_rows += n;
                    albums_fixed += 1;
                }
                _ => {}
            }
        }
    }

    if body.artists {
        let candidates = match state.db.artists_with_featured_credits(user.user_id).await {
            Ok(c) => c,
            Err(e) => return server_error(e.to_string()),
        };
        for c in candidates {
            if let Ok(n) = state.db.fold_artist_credit(user.user_id, &c.artist, &c.primary).await {
                if n > 0 {
                    artist_rows += n;
                    artists_folded += 1;
                }
            }
        }
    }

    tracing::info!(
        "🧹 Sanitised: {} listens across {} albums, {} listens across {} artist credits",
        album_rows,
        albums_fixed,
        artist_rows,
        artists_folded
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "albums_fixed": albums_fixed,
            "album_listens_changed": album_rows,
            "artists_folded": artists_folded,
            "artist_listens_changed": artist_rows,
        })),
    )
        .into_response()
}

/// GET /api/v1/stats/dashboard
async fn dashboard_stats(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.get_dashboard_stats(user.user_id).await {
        Ok(stats) => (StatusCode::OK, Json(stats)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                code: 500,
                error: format!("Failed to compute stats: {}", e)
            })),
        )
            .into_response(),
    }
}

/// PATCH body — every field optional; only provided fields change.
#[derive(Debug, Deserialize)]
pub struct UpdateScrobble {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub mbid_recording: Option<String>,
    /// Fixing this also clears the resolved artwork, so the cover is re-fetched
    /// for the release you actually meant.
    pub mbid_release: Option<String>,
    pub signal_chain_id: Option<i64>,
    pub listening_context: Option<String>,
    pub format_type: Option<String>,
    pub codec: Option<String>,
}

/// PATCH /api/v1/scrobbles/:id — edit a listen (fix MBID / chain / metadata).
async fn update_scrobble(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateScrobble>,
) -> impl IntoResponse {
    let updated = state
        .db
        .update_scrobble(
            user.user_id,
            id,
            body.title.as_deref(),
            body.artist.as_deref(),
            body.album.as_deref(),
            body.mbid_recording.as_deref(),
            body.mbid_release.as_deref(),
            body.signal_chain_id,
            body.listening_context.as_deref(),
            body.format_type.as_deref(),
            body.codec.as_deref(),
        )
        .await;
    match updated {
        Ok(true) => match state.db.get_scrobble(user.user_id, id).await {
            // return the fresh row so the UI can re-render without a refetch
            Ok(Some(r)) => (StatusCode::OK, Json(serde_json::json!(r))).into_response(),
            _ => (StatusCode::OK, Json(serde_json::json!({ "id": id, "status": "updated" })))
                .into_response(),
        },
        Ok(false) => not_found(),
        Err(e) => server_error(e.to_string()),
    }
}

/// DELETE /api/v1/scrobbles/:id — remove one listen owned by the user.
async fn delete_scrobble(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_scrobble(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found(),
        Err(e) => server_error(e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct ClearQuery {
    pub confirm: Option<bool>,
}

/// DELETE /api/v1/scrobbles?confirm=true — clear ALL listens for the user.
async fn clear_scrobbles(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ClearQuery>,
) -> impl IntoResponse {
    if q.confirm != Some(true) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Refusing to clear without ?confirm=true".into()
            })),
        )
            .into_response();
    }
    match state.db.clear_scrobbles(user.user_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub range: Option<String>,
}

/// Map a range label to a lower-bound timestamp (None = all time).
fn range_after(range: Option<&str>) -> Option<i64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
        as i64;
    match range.unwrap_or("all") {
        "week" => Some(now - 7 * 86400),
        "month" | "last30" => Some(now - 30 * 86400),
        "year" => Some(now - 365 * 86400),
        _ => None,
    }
}

/// The user's IANA timezone from their preferences (defaults to UTC).
async fn user_tz(state: &AppState, user_id: i64) -> chrono_tz::Tz {
    let name = state
        .db
        .get_preferences(user_id)
        .await
        .ok()
        .and_then(|p| p.get("timezone").and_then(|v| v.as_str().map(str::to_string)))
        .unwrap_or_default();
    crate::analytics::parse_tz(&name)
}

fn to_listen_rows(
    rows: Vec<(i64, String, String, Option<String>, Option<i32>)>,
) -> Vec<crate::analytics::ListenRow> {
    rows.into_iter()
        .map(|(ts, artist, title, album, track_number)| crate::analytics::ListenRow {
            ts,
            artist,
            title,
            album,
            track_number,
        })
        .collect()
}

/// GET /api/v1/stats?range=week|month|last30|year|all — ranged analytics.
async fn ranged_stats(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<StatsQuery>,
) -> impl IntoResponse {
    let after = range_after(q.range.as_deref());
    let mut stats = match state.db.get_ranged_stats(user.user_id, after).await {
        Ok(s) => s,
        Err(e) => return server_error(e.to_string()),
    };
    // Recompute the heatmap in the user's local timezone.
    if let Ok(rows) = state.db.get_listen_rows(user.user_id, after, state.analytics_row_cap).await {
        let tz = user_tz(&state, user.user_id).await;
        stats["heatmap"] =
            serde_json::json!(crate::analytics::heatmap_local(&to_listen_rows(rows), tz));
    }
    (StatusCode::OK, Json(stats)).into_response()
}

/// GET /api/v1/personality?range=… — five-axis listening personality.
async fn personality(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<StatsQuery>,
) -> impl IntoResponse {
    match state
        .db
        .get_listen_rows(user.user_id, range_after(q.range.as_deref()), state.analytics_row_cap)
        .await
    {
        Ok(rows) => {
            let tz = user_tz(&state, user.user_id).await;
            (StatusCode::OK, Json(crate::analytics::compute_personality(&to_listen_rows(rows), tz)))
                .into_response()
        }
        Err(e) => server_error(e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct BulkChain {
    pub ids: Vec<i64>,
    pub signal_chain_id: Option<i64>, // null clears the chain on those listens
}

/// POST /api/v1/scrobbles/bulk-chain — reassign many listens' chain at once
/// (History bulk edit, for the one-output-many-rigs case).
async fn bulk_set_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<BulkChain>,
) -> impl IntoResponse {
    match state.db.bulk_set_scrobble_chain(user.user_id, &body.ids, body.signal_chain_id).await {
        Ok(n) => (StatusCode::OK, Json(serde_json::json!({ "updated": n }))).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

/// GET /api/v1/languages/status — instance-wide classification coverage.
async fn language_status(State(state): State<Arc<AppState>>, _user: AuthUser) -> impl IntoResponse {
    match state.db.language_coverage().await {
        Ok((total, resolved)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "total": total, "resolved": resolved, "pending": total - resolved,
            })),
        )
            .into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

/// POST /api/v1/languages/backfill — admin: run a full classification pass in
/// the background (script instantly, MusicBrainz for the rest).
async fn backfill_languages_now(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    if !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse { code: 403, error: "Admin only".into() })),
        )
            .into_response();
    }
    let (db, mb) = (state.db.clone(), state.mb_client.clone());
    tokio::spawn(async move {
        loop {
            if crate::enrich::backfill_languages(&db, &mb, 200, 200).await == 0 {
                break;
            }
        }
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "started" }))).into_response()
}

/// GET /api/v1/artwork/status — cover-art coverage for the calling user.
async fn artwork_status(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.artwork_coverage(user.user_id).await {
        Ok((total, resolved)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "total": total, "resolved": resolved, "pending": total - resolved,
            })),
        )
            .into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

/// POST /api/v1/artwork/backfill — admin: resolve missing cover art in the
/// background. Rate-limited by the MusicBrainz client, so a large history takes
/// a while; the status endpoint is the progress bar.
async fn backfill_artwork_now(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    if !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse { code: 403, error: "Admin only".into() })),
        )
            .into_response();
    }
    let (db, mb) = (state.db.clone(), state.mb_client.clone());
    tokio::spawn(async move {
        let providers: Vec<Box<dyn crate::metadata::ArtworkProvider>> =
            vec![Box::new(crate::metadata::deezer::DeezerProvider::new())];
        loop {
            if crate::enrich::backfill_artwork(&db, &mb, &providers, 200, 200).await == 0 {
                break;
            }
        }
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "started" }))).into_response()
}

fn not_found() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!(ErrorResponse { code: 404, error: "Scrobble not found".into() })),
    )
        .into_response()
}
fn server_error(msg: String) -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!(ErrorResponse { code: 500, error: msg })),
    )
        .into_response()
}

pub fn create_scrobbles_router() -> Router<Arc<AppState>> {
    use axum::routing::patch;
    Router::new()
        .route("/api/v1/scrobbles", get(list_scrobbles).delete(clear_scrobbles))
        .route("/api/v1/scrobbles/bulk-chain", axum::routing::post(bulk_set_chain))
        .route("/api/v1/scrobbles/:id", patch(update_scrobble).delete(delete_scrobble))
        .route("/api/v1/search", get(search))
        .route("/api/v1/sanitize", get(sanitize_preview).post(sanitize_apply))
        .route("/api/v1/stats/dashboard", get(dashboard_stats))
        .route("/api/v1/stats", get(ranged_stats))
        .route("/api/v1/personality", get(personality))
        .route("/api/v1/languages/status", get(language_status))
        .route("/api/v1/languages/backfill", axum::routing::post(backfill_languages_now))
        .route("/api/v1/artwork/status", get(artwork_status))
        .route("/api/v1/artwork/backfill", axum::routing::post(backfill_artwork_now))
}
