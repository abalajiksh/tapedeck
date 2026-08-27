use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::auth::{AuthUser, ReadAuth, WriteAuth};
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
    /// `1` for listening marked as heard in company, `0` for the rest.
    ///
    /// Absent means no filter, which is the only sensible default: a listen
    /// nobody marked is *unmarked*, not proven solitary, and quietly excluding
    /// unmarked listening from a history page would hide most of it.
    pub social: Option<bool>,
}

/// GET /api/v1/scrobbles
pub async fn list_scrobbles(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
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
            params.social,
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

#[derive(serde::Deserialize)]
pub struct SkipsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// GET /api/v1/skips — the tracks this user bailed out of, with context.
///
/// Skips are stored and excluded from every count, which until now meant there
/// was nowhere at all to look at them. The interesting columns are how far in
/// the listener got and **what was playing either side** — bailing out is a
/// judgement about a track in a sequence, and the neighbours are the only part
/// of that this application can honestly record.
///
/// It does **not** say why, and nothing here should ever guess: the reasons a
/// person gives ("it killed the mood", "not today") are not derivable from a
/// timestamp. A liner note attaches to a listen and is where a stated reason
/// belongs.
pub async fn list_skips(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(params): Query<SkipsQuery>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(25).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);

    let (total, rows) = match state.db.skips(user.user_id, limit, offset).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    code: 500,
                    error: format!("Failed to fetch skips: {}", e)
                })),
            )
                .into_response();
        }
    };
    let most = state.db.most_skipped(user.user_id, 10).await.unwrap_or_default();

    let skips: Vec<_> = rows
        .into_iter()
        .map(|s| {
            // Only when both halves are known. A fraction built from a missing
            // playhead or a missing running time would be a guess wearing a
            // percentage, which is the fidelity-card mistake in a new place.
            let fraction = match (s.listened_ms, s.duration) {
                (Some(ms), Some(d)) if d > 0 => Some((ms as f64 / 1000.0) / d as f64),
                _ => None,
            };
            serde_json::json!({
                "id": s.id,
                "title": s.title,
                "artist": s.artist,
                "album": s.album,
                "timestamp": s.timestamp,
                "duration": s.duration,
                "listened_ms": s.listened_ms,
                "listened_fraction": fraction,
                "source": s.source,
                "chain": s.chain,
                "played_before": s.before.map(|(t, a)| serde_json::json!({"title": t, "artist": a})),
                "played_after": s.after.map(|(t, a)| serde_json::json!({"title": t, "artist": a})),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "total": total,
            "skips": skips,
            "most_skipped": most
                .into_iter()
                .map(|(title, artist, n)| {
                    serde_json::json!({"title": title, "artist": artist, "skips": n})
                })
                .collect::<Vec<_>>(),
        })),
    )
        .into_response()
}

/// GET /api/v1/search?q= — grouped matches from the caller's own history.
///
/// Scoped to `user.user_id` like every other read: on a shared instance one
/// person's library is not searchable by another.
pub async fn search(
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
pub async fn dashboard_stats(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
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
    /// Which setup of that chain was fitted. Only meaningful alongside
    /// `signal_chain_id`, and written whenever that is — see
    /// `Database::update_scrobble`.
    pub chain_variant_id: Option<i64>,
    pub listening_context: Option<String>,
    pub format_type: Option<String>,
    pub codec: Option<String>,
}

/// PATCH /api/v1/scrobbles/:id — edit a listen (fix MBID / chain / metadata).
async fn update_scrobble(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateScrobble>,
) -> impl IntoResponse {
    // A variant belongs to exactly one chain; pairing it with another resolves
    // to components never fitted at the same time.
    if let Some(variant) = body.chain_variant_id {
        let Some(chain) = body.signal_chain_id else {
            return bad_request("chain_variant_id needs a signal_chain_id to belong to");
        };
        match state.db.variant_fits_chain(user.user_id, variant, chain).await {
            Ok(true) => {}
            Ok(false) => return bad_request("That setup belongs to a different chain"),
            Err(e) => return server_error(e.to_string()),
        }
    }

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
            body.chain_variant_id,
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
    WriteAuth { user }: WriteAuth,
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
    /// `plays` (default) or `hours`. Decides how the three top lists are
    /// *ranked*, not just what number is shown — they are cut to 50 rows, so
    /// sorting in the browser would rank within the top 50 by plays and miss
    /// the long record played a handful of times.
    pub metric: Option<String>,
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
pub async fn ranged_stats(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<StatsQuery>,
) -> impl IntoResponse {
    let after = range_after(q.range.as_deref());
    let by_hours = q.metric.as_deref() == Some("hours");
    let mut stats = match state.db.get_ranged_stats(user.user_id, after, by_hours).await {
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
    /// Which chain to put these listens on. **Present-and-null clears it;
    /// absent is an error**, which is why this is doubly wrapped.
    ///
    /// It was a plain `Option<i64>`, and serde reads a bare `Option` field as
    /// defaulting to `None` — so a request that merely *misspelled* the key
    /// deserialised cleanly, cleared the chain off every listen named in `ids`,
    /// and answered `200 {"updated": 28}`. A typo silently undid the
    /// attribution it was trying to set, and reported success. `openapi.yaml`
    /// made that easy to hit: it documented the field as `chain_id`, which is
    /// exactly the spelling that wipes.
    ///
    /// Same shape and same reason as `default_chain_id` on
    /// `PATCH /admin/tokens/:id` — the difference between "clear this" and
    /// "leave it alone" has to survive deserialisation.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub signal_chain_id: Option<Option<i64>>,
    /// Which setup of that chain was fitted. Absent means none — and it is
    /// **written either way**, never left alone: a listen moved from chain A to
    /// chain B must not keep a variant belonging to A, which would pair it with
    /// components that were never fitted together.
    #[serde(default)]
    pub chain_variant_id: Option<i64>,
}

/// Distinguish an absent field from an explicit `null`.
///
/// `#[serde(default)]` alone cannot: both arrive as `None`. This is only ever
/// called when the key is present, so anything it yields is `Some(_)`.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// POST /api/v1/scrobbles/bulk-chain — reassign many listens' chain at once
/// (History bulk edit, for the one-output-many-rigs case).
async fn bulk_set_chain(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<BulkChain>,
) -> impl IntoResponse {
    // Refused rather than treated as "clear". This endpoint is destructive by
    // nature — it overwrites the chain on every listen in `ids` — so a body
    // that doesn't say what to set must not be guessed at.
    let Some(chain_id) = body.signal_chain_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "signal_chain_id is required — send a chain id, or an explicit null to \
                        clear the chain on these listens"
                    .into()
            })),
        )
            .into_response();
    };

    // A variant belongs to exactly one chain. Pairing it with another resolves
    // to components that were never fitted at the same time, so it is refused
    // at the door rather than stored and quietly ignored by `equipment_seconds`.
    if let Some(variant) = body.chain_variant_id {
        let Some(chain) = chain_id else {
            return bad_request("chain_variant_id needs a signal_chain_id to belong to");
        };
        match state.db.variant_fits_chain(user.user_id, variant, chain).await {
            Ok(true) => {}
            Ok(false) => return bad_request("That setup belongs to a different chain"),
            Err(e) => return server_error(e.to_string()),
        }
    }

    match state
        .db
        .bulk_set_scrobble_chain(user.user_id, &body.ids, chain_id, body.chain_variant_id)
        .await
    {
        Ok(n) => (StatusCode::OK, Json(serde_json::json!({ "updated": n }))).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

/// The body of `POST /api/v1/scrobbles/bulk-company`.
#[derive(Debug, Deserialize)]
pub struct BulkCompany {
    pub ids: Vec<i64>,
    /// Who these listens were heard with, or an explicit `null` to unmark them.
    ///
    /// `Option<Option<String>>` for the reason `signal_chain_id` above is: this
    /// overwrites the field on every listen in `ids`, so a body that merely
    /// misspelled the key must not deserialise into "clear it". Absent is a
    /// 400.
    #[serde(default, deserialize_with = "deserialize_some")]
    pub company: Option<Option<String>>,
}

/// POST /api/v1/scrobbles/bulk-company — mark many listens as heard in company.
///
/// **Marking is retroactive by design.** Nobody stops mid-record to note who is
/// in the room; they think of it afterwards, which is why this takes a set of
/// listens rather than being a field at ingest. A Shared Spool fills the same
/// column in automatically — this is for the far commoner case of somebody on
/// the sofa who has no account on this instance.
async fn bulk_set_company(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<BulkCompany>,
) -> impl IntoResponse {
    let Some(company) = body.company else {
        return bad_request(
            "company is required — send who you were listening with, or an explicit null to unmark these listens",
        );
    };
    // Trimmed, and an empty string means the same as null rather than becoming
    // a listen marked as heard with nobody — which would count as social in
    // every readout while naming no one.
    let company = company.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    if company.as_deref().is_some_and(|c| c.chars().count() > 200) {
        return bad_request("company is too long — 200 characters at most");
    }

    match state.db.bulk_set_scrobble_company(user.user_id, &body.ids, company.as_deref()).await {
        Ok(n) => (StatusCode::OK, Json(serde_json::json!({ "updated": n }))).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

fn bad_request(msg: &str) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!(ErrorResponse { code: 400, error: msg.into() })),
    )
        .into_response()
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

/// GET /api/v1/mbids/status — how much of this user's history MusicBrainz has
/// identified at recording level.
///
/// Three numbers, not two. `unmatched` is the distinct tracks MusicBrainz was
/// asked about and had nothing for — real listening simply not in their
/// database — and it is reported because without it a progress bar that stops
/// short reads as a stall rather than as an answer.
#[derive(serde::Deserialize)]
pub struct LyricsQuery {
    pub artist: String,
    pub title: String,
    /// Which record to draw the tracklist from.
    ///
    /// Optional and resolved from the history when it is absent, because the
    /// caller is usually a note or a history row that knows the track and not
    /// the record it was filed under. Passed explicitly it wins, since a track
    /// on three compilations has three honest answers and only the caller knows
    /// which one is being read.
    pub album: Option<String>,
}

/// GET /api/v1/lyrics — the words to one track, as numbered lines.
///
/// **Numbered including blanks.** A stanza break is a line, because a note
/// refers to a line by its index and skipping empties would renumber every
/// line after one — silently re-pointing every quote already written.
///
/// Reads only what is stored and never triggers a fetch. LRCLIB is free and
/// asks callers to be considerate; the backfill is what paces that, and a
/// screen that fetched on open would let a page refresh drive the rate.
///
/// Three answers, all of them real: lines, `instrumental` (a track with no
/// words, which is not a gap), and nothing at all.
async fn lyrics_for_track(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<LyricsQuery>,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    // The furniture round the words: which record this is on, how often it has
    // been played, and what has been written about it. All of it is about the
    // *track*, so it is gathered whether or not the lyric turns out to be held
    // — an instrumental still has a tracklist beside it, and a pin outlives the
    // words it was pinned to.
    let album = match q.album.clone().filter(|a| !a.trim().is_empty()) {
        Some(a) => Some(a),
        None => state.db.album_for_track(user.user_id, &q.title, &q.artist).await.ok().flatten(),
    };
    let plays = state.db.track_play_count(user.user_id, &q.artist, &q.title).await.unwrap_or(0);
    let rows = match album.as_deref() {
        Some(a) => state.db.album_track_list(user.user_id, &q.artist, a).await.unwrap_or_default(),
        None => Vec::new(),
    };
    // What each sibling track has, so the sidebar can mark the instrumentals and
    // dim what there is nothing to open — the design's `· instrumental`. One
    // query for the whole record rather than a lookup per row.
    let states = lyrics
        .states_for(&rows.iter().map(|(t, ..)| (q.artist.clone(), t.clone())).collect::<Vec<_>>())
        .await;
    let tracklist = rows
        .into_iter()
        .map(|(title, plays, duration, track_number)| {
            let key = crate::lyrics::LyricsClient::track_key(&q.artist, &title);
            serde_json::json!({
                "title": title, "plays": plays,
                "duration": duration, "track_number": track_number,
                // Absent for a track nobody has asked about, which is a
                // different thing from one that has no words.
                "lyrics": states.get(&key),
            })
        })
        .collect::<Vec<_>>();

    // Pins, keyed by the line they hang off. One query for the whole reader
    // rather than one per line — a lyric is a couple of hundred lines.
    let mut pins: std::collections::HashMap<usize, serde_json::Value> =
        std::collections::HashMap::new();
    for note in state.db.pins_for_track(user.user_id, &q.artist, &q.title).await.unwrap_or_default()
    {
        let Some(n) = note.line_key.as_deref().and_then(crate::db::NoteTarget::line_number) else {
            continue;
        };
        pins.insert(
            n,
            serde_json::json!({
                "note_id": note.id,
                "body_md": note.body_md,
                "excerpt": crate::notes::excerpt(&note.body_md, 200),
                "visibility": note.visibility,
                "updated_at": note.updated_at,
            }),
        );
    }
    let quoted =
        crate::server::notes::quoted_lines(&state.db, user.user_id, &q.artist, &q.title).await;

    let head = serde_json::json!({
        "artist": q.artist,
        "title": q.title,
        "album": album,
        "plays": plays,
        "tracklist": tracklist,
    });
    let merge = |mut base: serde_json::Value, extra: serde_json::Value| {
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    };

    let body = match lyrics.cached(&q.artist, &q.title).await {
        Some(crate::lyrics::Found::Lyrics { plain, synced, language }) => {
            let lines = crate::lyrics::parse_lines(plain.as_deref(), synced.as_deref());
            merge(
                head,
                serde_json::json!({
                    "language": language,
                    "timed": lines.iter().any(|l| l.at.is_some()),
                    "lines": lines.iter().map(|l| serde_json::json!({
                        "n": l.index, "at": l.at, "text": l.text,
                        "pin": pins.get(&l.index),
                        // How many notes quote this line. Zero is sent as zero
                        // rather than omitted: the reader draws nothing for it,
                        // and a missing key would be indistinguishable from a
                        // count that failed to be computed.
                        "quoted": quoted.get(&l.index).copied().unwrap_or(0),
                    })).collect::<Vec<_>>(),
                    // The raw text, for correcting it by hand. Sent so "Fix the
                    // words" opens on what is actually stored rather than on
                    // the parsed lines put back together, which would quietly
                    // rewrite a lyric nobody meant to touch.
                    "plain": plain,
                    "synced": synced,
                    "edited": lyrics.is_edited(&q.artist, &q.title).await,
                    // Shown wherever the words are, because they are not ours.
                    "source": "LRCLIB",
                }),
            )
        }
        Some(crate::lyrics::Found::Instrumental) => {
            merge(head, serde_json::json!({ "instrumental": true, "lines": [] }))
        }
        Some(crate::lyrics::Found::Missing) | None => {
            merge(head, serde_json::json!({ "lines": [] }))
        }
    };
    (StatusCode::OK, Json(body)).into_response()
}

/// GET /api/v1/lyrics/keys — every track this instance holds words for.
///
/// One request for the whole set, exactly the shape `GET /api/v1/loves/keys`
/// has and for the same reason: a history page wants to know which of fifty
/// rows has something to read, and fifty questions is not the way to ask. The
/// browser derives a row's key the same way `LyricsClient::track_key` does —
/// **and those two must stay in step**, or "Read the words" quietly stops
/// appearing and nothing errors. Same standing trap as `loveKey`.
///
/// Not scoped to the caller, and it cannot be: the cache is one copy of one
/// track's words for the whole instance, keyed by the track rather than by who
/// played it. What it discloses is which tracks *anybody here* has had fetched,
/// which is the same thing the backfill's own progress readout says.
async fn lyrics_keys(State(state): State<Arc<AppState>>, _user: ReadAuth) -> impl IntoResponse {
    let keys = match state.lyrics.clone() {
        Some(l) => l.keys_with_lyrics().await,
        // No cache is not an error — it is an instance with no lyrics, and the
        // honest answer is that nothing has words rather than a failed request
        // that stops a history page rendering.
        None => Vec::new(),
    };
    (StatusCode::OK, Json(serde_json::json!({ "keys": keys }))).into_response()
}

#[derive(serde::Deserialize)]
pub struct FixLyricsRequest {
    pub artist: String,
    pub title: String,
    /// The words, plain. Absent clears them.
    pub plain: Option<String>,
    /// The timed form, `[mm:ss.xx] words` per line.
    pub synced: Option<String>,
}

/// PUT /api/v1/lyrics — correct the words by hand.
///
/// **A value you supplied is never overwritten by a lookup**, the rule the
/// shelf's running times already follow, and it is what settles the collision
/// between this and the design's *newest fetch wins*: between two fetches the
/// newer one wins, and between a fetch and something a person typed, the person
/// does.
///
/// The honest warning is about the **line count**. Every reference already
/// written is an index, so a correction that adds or removes a line moves every
/// quote after it onto different words — silently, because an index cannot be
/// wrong, only pointed somewhere else. The response reports the count before
/// and after so the caller can say so; it is not refused, because the
/// correction is usually the point and the alternative is a lyric that stays
/// wrong for ever.
async fn fix_lyrics(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<FixLyricsRequest>,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    if body.artist.trim().is_empty() || body.title.trim().is_empty() {
        return bad_request("A correction needs an artist and a title");
    }
    if body.plain.as_deref().unwrap_or("").trim().is_empty()
        && body.synced.as_deref().unwrap_or("").trim().is_empty()
    {
        return bad_request("There are no words in that");
    }

    let before = match lyrics.cached(&body.artist, &body.title).await {
        Some(crate::lyrics::Found::Lyrics { plain, synced, .. }) => {
            crate::lyrics::parse_lines(plain.as_deref(), synced.as_deref()).len()
        }
        _ => 0,
    };
    match lyrics
        .set_words(&body.artist, &body.title, body.plain.as_deref(), body.synced.as_deref())
        .await
    {
        Ok(crate::lyrics::Found::Lyrics { plain, synced, language }) => {
            let after = crate::lyrics::parse_lines(plain.as_deref(), synced.as_deref()).len();
            let _ = user;
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "saved",
                    "language": language,
                    "lines_before": before,
                    "lines_after": after,
                    // The one thing worth saying out loud: a reference is an
                    // index, so a changed count re-points every quote past the
                    // change.
                    "renumbered": before > 0 && before != after,
                })),
            )
                .into_response()
        }
        Ok(_) => server_error("The correction did not come back as words".to_string()),
        Err(e) => server_error(e.to_string()),
    }
}

/// GET /api/v1/lyrics/status — how far the lyric language pass has got.
///
/// Reports what it is *for* rather than a lyric count: the pass exists to empty
/// the `English/Other` bucket, which is a shrug rather than a measurement and
/// is most of a transliterated history. `unresolved` is what is left to try.
///
/// `instrumental` is reported separately because it is a real answer — a track
/// with no words has no language — and folding it into "found" would present a
/// finished lookup as an unfinished one.
async fn lyrics_status(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let unresolved = state.db.tracks_wanting_language(100_000).await.map(|v| v.len()).unwrap_or(0);
    let Some(lyrics) = state.lyrics.clone() else {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "available": false,
                "why": "The lyrics cache could not be opened, so the language pass is skipped.",
                "unresolved": unresolved,
            })),
        )
            .into_response();
    };
    let (asked, with_lyrics, instrumental) = lyrics.coverage().await;
    let _ = user;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "available": true,
            "asked": asked,
            "with_lyrics": with_lyrics,
            "instrumental": instrumental,
            "unresolved": unresolved,
        })),
    )
        .into_response()
}

async fn mbid_status(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let (total, resolved) = match state.db.recording_mbid_coverage(user.user_id).await {
        Ok(v) => v,
        Err(e) => return server_error(e.to_string()),
    };

    // Folded in Rust because the two halves live in **different database
    // files**: the listens are in `DATABASE_URL`, and the MusicBrainz negative
    // cache is in `SQLITE_DB_PATH`. A join would not merely be slow — the table
    // is not in that database at all, and a query against it fails rather than
    // returning nothing, which is how a first version of this reported a
    // confident zero.
    let unmatched = match state.db.unresolved_track_keys(user.user_id).await {
        Ok(keys) if !keys.is_empty() => {
            let asked = state.mb_client.negative_track_keys().await;
            keys.into_iter().filter(|k| asked.contains(k)).count() as i64
        }
        _ => 0,
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "total": total,
            "resolved": resolved,
            "pending": total - resolved,
            "unmatched_tracks": unmatched,
        })),
    )
        .into_response()
}

/// POST /api/v1/mbids/backfill — admin: resolve missing recording MBIDs in the
/// background.
///
/// This is the pass that unblocks the others: the ISRC, work-language and
/// release-year lookups all key off a recording MBID, and an imported listen
/// never had one because only `flush_pending` ever wrote the column.
///
/// Bounded per iteration rather than let loose, because every miss is a
/// rate-limited MusicBrainz search — and it stops as soon as a pass resolves
/// nothing, which on a history where the rest is genuinely unknown is the
/// honest end rather than a loop.
async fn backfill_mbids_now(
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
            if crate::enrich::backfill_recording_mbids(&db, &mb, 50, 50).await == 0 {
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
        .route("/api/v1/scrobbles/bulk-company", axum::routing::post(bulk_set_company))
        .route("/api/v1/scrobbles/:id", patch(update_scrobble).delete(delete_scrobble))
        .route("/api/v1/search", get(search))
        .route("/api/v1/skips", get(list_skips))
        .route("/api/v1/sanitize", get(sanitize_preview).post(sanitize_apply))
        .route("/api/v1/stats/dashboard", get(dashboard_stats))
        .route("/api/v1/stats", get(ranged_stats))
        .route("/api/v1/personality", get(personality))
        .route("/api/v1/languages/status", get(language_status))
        .route("/api/v1/languages/backfill", axum::routing::post(backfill_languages_now))
        .route("/api/v1/lyrics", get(lyrics_for_track).put(fix_lyrics))
        .route("/api/v1/lyrics/keys", get(lyrics_keys))
        .route("/api/v1/lyrics/status", get(lyrics_status))
        .route("/api/v1/mbids/status", get(mbid_status))
        .route("/api/v1/mbids/backfill", axum::routing::post(backfill_mbids_now))
        .route("/api/v1/artwork/status", get(artwork_status))
        .route("/api/v1/artwork/backfill", axum::routing::post(backfill_artwork_now))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A destructive endpoint must not read a typo as an instruction.
    ///
    /// `bulk-chain` overwrites the chain on every listen in `ids`. With a plain
    /// `Option<i64>` a body that misspelled the key — `chain_id`, which is what
    /// `openapi.yaml` used to document — deserialised cleanly to `None`, cleared
    /// the attribution on all of them, and answered `200 {"updated": N}`. This
    /// pins the three cases apart at the layer where they were conflated.
    #[test]
    fn a_bulk_chain_body_tells_absent_from_null_from_a_value() {
        let parse = |s: &str| serde_json::from_str::<BulkChain>(s);

        // A number assigns.
        assert_eq!(
            parse(r#"{"ids":[1,2],"signal_chain_id":7}"#).unwrap().signal_chain_id,
            Some(Some(7))
        );

        // An explicit null clears — which the History bulk edit relies on, so
        // this is not a case that can be tightened away.
        assert_eq!(
            parse(r#"{"ids":[1],"signal_chain_id":null}"#).unwrap().signal_chain_id,
            Some(None)
        );

        // Absent is *not* null. The handler turns this into a 400.
        assert_eq!(parse(r#"{"ids":[1]}"#).unwrap().signal_chain_id, None);

        // The exact shape that used to wipe: the documented-but-wrong spelling.
        // It is an unknown field, so it lands in the same "absent" bucket.
        assert_eq!(parse(r#"{"ids":[1,2,3],"chain_id":7}"#).unwrap().signal_chain_id, None);
    }
}
