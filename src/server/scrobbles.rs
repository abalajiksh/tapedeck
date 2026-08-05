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
    let variants = state.db.artists_with_spelling_variants(user.user_id).await.unwrap_or_default();
    let splits = state.db.tracks_with_split_attribution(user.user_id).await.unwrap_or_default();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "mixed_albums": albums,
            "featured_artists": artists,
            "artist_variants": variants,
            "split_attributions": splits,
        })),
    )
        .into_response()
}

/// One album to unify, and onto which release.
///
/// `mbid_release` is carried per edit rather than re-derived, so the operator
/// can overrule the plurality — which is the point of reviewing at all.
#[derive(Debug, Deserialize)]
pub struct AlbumEdit {
    pub artist: String,
    pub album: String,
    pub mbid_release: String,
}

#[derive(Debug, Deserialize)]
pub struct CreditEdit {
    /// The full credit as stored, e.g. `Cœur de pirate feat. Loud`.
    pub artist: String,
}

#[derive(Debug, Deserialize)]
pub struct VariantEdit {
    /// The spelling being retired.
    pub from: String,
    /// The spelling it becomes.
    pub to: String,
}

/// One track whose stray attribution goes back to the record's own artist.
#[derive(Debug, Deserialize)]
pub struct SplitEdit {
    pub title: String,
    pub album: String,
    /// The attribution being folded — a singer, on a soundtrack.
    pub from: String,
    /// The record's artist. Checked against the preview rather than trusted:
    /// taking this from the request would turn a narrow fix into "rename any
    /// track to any artist", the same reason `split_featured` derives its own
    /// target.
    pub to: String,
}

/// What to apply.
///
/// Two ways to say it, because both are the common case at different times: the
/// booleans mean "everything in this category", the lists name individual edits.
/// A non-empty list wins over its boolean — asking for specific edits and
/// getting all of them would be the worst possible reading.
#[derive(Debug, Deserialize, Default)]
pub struct SanitizeRequest {
    #[serde(default)]
    pub albums: bool,
    #[serde(default)]
    pub artists: bool,
    #[serde(default)]
    pub variants: bool,

    #[serde(default)]
    pub album_edits: Vec<AlbumEdit>,
    #[serde(default)]
    pub credit_edits: Vec<CreditEdit>,
    #[serde(default)]
    pub variant_edits: Vec<VariantEdit>,
    #[serde(default)]
    pub splits: bool,
    #[serde(default)]
    pub split_edits: Vec<SplitEdit>,
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
    let mut variant_rows = 0u64;
    let mut variants_merged = 0u64;

    // ── albums ──
    if !body.album_edits.is_empty() {
        for e in &body.album_edits {
            match state
                .db
                .unify_album_release(user.user_id, &e.artist, &e.album, &e.mbid_release)
                .await
            {
                Ok(n) if n > 0 => {
                    album_rows += n;
                    albums_fixed += 1;
                }
                _ => {}
            }
        }
    } else if body.albums {
        let candidates = match state.db.albums_with_mixed_releases(user.user_id).await {
            Ok(c) => c,
            Err(e) => return server_error(e.to_string()),
        };
        for c in candidates {
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

    // ── featured credits ──
    // The primary is derived here rather than taken from the request: it is a
    // pure function of the stored string, and accepting an arbitrary target
    // would turn this into "rename any artist to anything".
    if !body.credit_edits.is_empty() {
        for e in &body.credit_edits {
            let Some(primary) = crate::db::split_featured(&e.artist) else { continue };
            if let Ok(n) = state.db.fold_artist_credit(user.user_id, &e.artist, &primary).await {
                if n > 0 {
                    artist_rows += n;
                    artists_folded += 1;
                }
            }
        }
    } else if body.artists {
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

    // ── spelling variants ──
    // `from` and `to` must be two spellings of *one* name. Without that check a
    // caller could rename any artist to any other, which is a different and much
    // more destructive operation than the one being offered.
    if !body.variant_edits.is_empty() {
        for e in &body.variant_edits {
            if crate::db::normalise_artist(&e.from) != crate::db::normalise_artist(&e.to) {
                return err_response(
                    StatusCode::BAD_REQUEST,
                    format!("\"{}\" and \"{}\" aren't spellings of the same name", e.from, e.to),
                );
            }
            if let Ok(n) = state.db.rename_artist(user.user_id, &e.from, &e.to).await {
                if n > 0 {
                    variant_rows += n;
                    variants_merged += 1;
                }
            }
        }
    } else if body.variants {
        let candidates = match state.db.artists_with_spelling_variants(user.user_id).await {
            Ok(c) => c,
            Err(e) => return server_error(e.to_string()),
        };
        for c in candidates {
            for (spelling, _) in &c.variants {
                if let Ok(n) = state.db.rename_artist(user.user_id, spelling, &c.primary).await {
                    if n > 0 {
                        variant_rows += n;
                        variants_merged += 1;
                    }
                }
            }
        }
    }

    // ── Split attributions ──
    //
    // The target is never taken from the request. It is looked up in the same
    // preview the operator saw, so an edit can only move a track onto the
    // artist Tapedeck itself worked out for that record — the same discipline
    // as the featured-credit pass, where accepting an arbitrary primary would
    // turn a narrow fix into a rename-anything endpoint.
    let mut split_rows = 0u64;
    let mut splits_merged = 0u64;
    if !body.split_edits.is_empty() || body.splits {
        let candidates =
            state.db.tracks_with_split_attribution(user.user_id).await.unwrap_or_default();

        let apply = |title: &str, album: &str, from: &str, to: &str| {
            (title.to_string(), album.to_string(), from.to_string(), to.to_string())
        };
        let wanted: Vec<(String, String, String, String)> = if !body.split_edits.is_empty() {
            body.split_edits
                .iter()
                .filter_map(|e| {
                    let c = candidates.iter().find(|c| {
                        c.title.eq_ignore_ascii_case(e.title.trim())
                            && c.album.eq_ignore_ascii_case(e.album.trim())
                    })?;
                    // The named source must actually be one of the strays, and
                    // the named target must be the record's own artist.
                    if !c.folding.iter().any(|(n, _)| n.eq_ignore_ascii_case(e.from.trim())) {
                        return None;
                    }
                    if !c.primary.eq_ignore_ascii_case(e.to.trim()) {
                        return None;
                    }
                    Some(apply(&c.title, &c.album, e.from.trim(), &c.primary))
                })
                .collect()
        } else {
            candidates
                .iter()
                .flat_map(|c| {
                    c.folding
                        .iter()
                        .map(|(from, _)| apply(&c.title, &c.album, from, &c.primary))
                        .collect::<Vec<_>>()
                })
                .collect()
        };

        for (title, album, from, to) in wanted {
            if let Ok(n) =
                state.db.merge_track_attribution(user.user_id, &title, &album, &from, &to).await
            {
                if n > 0 {
                    split_rows += n;
                    splits_merged += 1;
                }
            }
        }
    }

    tracing::info!(
        "🧹 Sanitised: {} listens across {} albums, {} listens across {} artist credits, \
         {} listens across {} spellings",
        album_rows,
        albums_fixed,
        artist_rows,
        artists_folded,
        variant_rows,
        variants_merged
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "albums_fixed": albums_fixed,
            "album_listens_changed": album_rows,
            "artists_folded": artists_folded,
            "artist_listens_changed": artist_rows,
            "variants_merged": variants_merged,
            "variant_listens_changed": variant_rows,
            "splits_merged": splits_merged,
            "split_listens_changed": split_rows,
        })),
    )
        .into_response()
}

/// GET /api/v1/stats/dashboard
async fn dashboard_stats(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let tz = user_tz(&state, user.user_id).await;
    let week_start_day = user_week_start(&state, user.user_id).await;
    match state.db.get_dashboard_stats(user.user_id, tz, week_start_day).await {
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
pub fn range_after(range: Option<&str>) -> Option<i64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
        as i64;
    match range.unwrap_or("all") {
        // `today` and `d7` exist for the charts screen's timeframe pills.
        "today" => Some(now - 86400),
        "d7" | "week" => Some(now - 7 * 86400),
        "month" | "last30" => Some(now - 30 * 86400),
        "year" => Some(now - 365 * 86400),
        _ => None,
    }
}

/// The user's IANA timezone from their preferences (defaults to UTC).
/// The caller's timezone. Shared with Rediscovery, which needs it for the
/// after-midnight collection — "after midnight" in UTC means nothing.
pub async fn user_tz_for(state: &AppState, user_id: i64) -> chrono_tz::Tz {
    user_tz(state, user_id).await
}

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

/// First day of the week, 0 = Sunday .. 6 = Saturday.
///
/// Defaults to Monday, which is ISO-8601 and what most of the world means by
/// "this week". Last.fm runs its charts Friday to Friday, which is exactly why
/// this is a preference rather than a constant.
async fn user_week_start(state: &AppState, user_id: i64) -> u32 {
    state
        .db
        .get_preferences(user_id)
        .await
        .ok()
        .and_then(|p| p.get("week_start").and_then(|v| v.as_u64()))
        .map(|d| (d % 7) as u32)
        .unwrap_or(1)
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

    // Breadth, when the genre map has been fetched and genres backfilled.
    // Absent rather than zeroed otherwise — "you play 0 genres" would be a lie
    // about the listening rather than a statement about the data.
    let counts = state.db.genre_play_counts(user.user_id, after).await.unwrap_or_default();
    if !counts.is_empty() {
        let names: Vec<String> = counts.iter().map(|(n, _)| n.clone()).collect();
        let coords = state.everynoise.coordinates_for(&names).await.unwrap_or_default();
        let spread = state.everynoise.spread().await.unwrap_or(0.0);
        stats["breadth"] = super::everynoise::breadth(&counts, &coords, spread);
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
/// A 4xx with a message, for the cases the caller can fix.
fn err_response(code: StatusCode, msg: String) -> axum::response::Response {
    (code, Json(serde_json::json!(ErrorResponse { code: code.as_u16(), error: msg })))
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
