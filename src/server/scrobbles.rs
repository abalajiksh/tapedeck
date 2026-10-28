use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use super::auth::{AuthUser, ReadAuth, WriteAuth};
use super::models::{ErrorResponse, OkResponse};
use super::AppState;
use crate::db::ScrobbleRecord;
use crate::openapi::{BadRequest, Forbidden, NotFound, Unauthorized};

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ScrobblesQuery {
    /// Page size. 50 by default, at most 500.
    #[param(minimum = 1, maximum = 500, default = 50)]
    pub limit: Option<i64>,
    /// Rows to skip.
    #[param(minimum = 0, default = 0)]
    pub offset: Option<i64>,
    /// Exact artist, as stored.
    pub artist: Option<String>,
    /// Exact album, as stored.
    pub album: Option<String>,
    /// Exact track title, as stored.
    pub track: Option<String>,
    /// Unix seconds, inclusive.
    pub after: Option<i64>,
    /// Unix seconds, inclusive.
    pub before: Option<i64>,
    /// `true` for listening marked as heard in company, `false` for the rest.
    ///
    /// Absent means no filter, which is the only sensible default: a listen
    /// nobody marked is *unmarked*, not proven solitary, and quietly excluding
    /// unmarked listening from a history page would hide most of it.
    pub social: Option<bool>,
}

/// A page of the history, newest first.
#[derive(Serialize, ToSchema)]
pub struct ListenPage {
    pub scrobbles: Vec<ScrobbleRecord>,
    /// Rows in this page, not in the history.
    pub count: usize,
    pub limit: i64,
    pub offset: i64,
}

/// The listen history.
///
/// Scope: `read` *(since v0.39)*. Newest first. Filterable.
#[utoipa::path(
    get,
    path = "/api/v1/scrobbles",
    tag = "Listens",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    params(ScrobblesQuery),
    responses(
        (status = 200, description = "OK.", body = ListenPage),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
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
            (StatusCode::OK, Json(ListenPage { scrobbles, count, limit, offset })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: format!("Failed to fetch scrobbles: {}", e) }),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchQuery {
    /// Matched as a substring, case-insensitively, against artist, album and
    /// title.
    pub q: Option<String>,
    /// Per group. 10 by default, at most 50.
    #[param(minimum = 1, maximum = 50, default = 10)]
    pub limit: Option<i64>,
}

/// Grouped matches from your own history, most-played first in each group.
#[derive(Serialize, ToSchema)]
pub struct SearchResults {
    pub artists: Vec<SearchArtist>,
    pub albums: Vec<SearchAlbum>,
    pub tracks: Vec<SearchTrack>,
}

#[derive(Serialize, ToSchema)]
pub struct SearchArtist {
    pub name: String,
    pub plays: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SearchAlbum {
    pub name: String,
    pub artist: String,
    pub plays: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SearchTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub plays: i64,
}

#[derive(serde::Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SkipsQuery {
    /// Page size. 25 by default, at most 100.
    #[param(minimum = 1, maximum = 100, default = 25)]
    pub limit: Option<i64>,
    #[param(minimum = 0, default = 0)]
    pub offset: Option<i64>,
}

/// Your skips, newest first, and the tracks skipped most.
#[derive(Serialize, ToSchema)]
pub struct Skips {
    /// Every skip on record, for paging.
    pub total: i64,
    pub skips: Vec<Skip>,
    /// The ten tracks skipped most often, over the whole history.
    pub most_skipped: Vec<MostSkipped>,
}

/// One skip, with what was playing either side of it.
#[derive(Serialize, ToSchema)]
pub struct Skip {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub timestamp: i64,
    /// Seconds.
    pub duration: Option<i64>,
    /// How far in, when something measured a playhead; null otherwise, never
    /// wall-clock arithmetic.
    pub listened_ms: Option<i64>,
    /// `listened_ms` over `duration`, 0–1. Only when both are known.
    pub listened_fraction: Option<f64>,
    pub source: String,
    /// The signal chain's name.
    pub chain: Option<String>,
    /// The listen before it.
    pub played_before: Option<TrackRef>,
    /// The listen after it.
    pub played_after: Option<TrackRef>,
}

#[derive(Serialize, ToSchema)]
pub struct TrackRef {
    pub title: String,
    pub artist: String,
}

#[derive(Serialize, ToSchema)]
pub struct MostSkipped {
    pub title: String,
    pub artist: String,
    pub skips: i64,
}

// GET /api/v1/skips — the tracks this user bailed out of, with context.
//
// Skips are stored and excluded from every count, which until now meant there
// was nowhere at all to look at them. The interesting columns are how far in
// the listener got and **what was playing either side** — bailing out is a
// judgement about a track in a sequence, and the neighbours are the only part
// of that this application can honestly record.
//
// It does **not** say why, and nothing here should ever guess: the reasons a
// person gives ("it killed the mood", "not today") are not derivable from a
// timestamp. A liner note attaches to a listen and is where a stated reason
// belongs.
/// Tracks you bailed out of.
///
/// Scope: `read` *(since v0.120)*. Skips are stored but excluded from every listening figure,
/// so this is the only place they are readable.
///
/// `listened_ms` is how far in the listener got, and is **null** when
/// nothing measured a playhead — a polled server reports one, a submitting
/// client often does not. `listened_fraction` is derived only when both the
/// playhead and the running time are known, rather than estimated.
///
/// `played_before` / `played_after` are the nearest real listens either
/// side, which is the only context this application can honestly record for
/// a skip. Both are null for a skip at the edge of a sitting. Nothing here
/// says *why* a track was skipped and nothing should infer it.
#[utoipa::path(
    get,
    path = "/api/v1/skips",
    tag = "Listens",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(SkipsQuery),
    responses(
        (status = 200, description = "OK.", body = Skips),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn list_skips(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(params): Query<SkipsQuery>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(25).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);

    let (total, rows) = match state.db.skips(user.user_id, limit, offset).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { code: 500, error: format!("Failed to fetch skips: {}", e) }),
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
            Skip {
                id: s.id,
                title: s.title,
                artist: s.artist,
                album: s.album,
                timestamp: s.timestamp,
                duration: s.duration,
                listened_ms: s.listened_ms,
                listened_fraction: fraction,
                source: s.source,
                chain: s.chain,
                played_before: s.before.map(|(title, artist)| TrackRef { title, artist }),
                played_after: s.after.map(|(title, artist)| TrackRef { title, artist }),
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(Skips {
            total,
            skips,
            most_skipped: most
                .into_iter()
                .map(|(title, artist, skips)| MostSkipped { title, artist, skips })
                .collect(),
        }),
    )
        .into_response()
}

// GET /api/v1/search?q= — grouped matches from the caller's own history.
//
// Scoped to `user.user_id` like every other read: on a shared instance one
// person's library is not searchable by another.
/// Search your listens.
///
/// Scope: `read` *(since v0.120)*. Searches `scrobbles`, not `entities` — searching entities
/// would only find records you had already been to.
#[utoipa::path(
    get,
    path = "/api/v1/search",
    tag = "Listens",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(SearchQuery),
    responses(
        (status = 200, description = "OK.", body = SearchResults),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn search(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let q = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(10).clamp(1, 50);

    match state.db.search_library(user.user_id, &q, limit).await {
        Ok((artists, albums, tracks)) => {
            let artists =
                artists.into_iter().map(|(name, plays)| SearchArtist { name, plays }).collect();
            let albums = albums
                .into_iter()
                .map(|(name, artist, plays)| SearchAlbum { name, artist, plays })
                .collect();
            let tracks = tracks
                .into_iter()
                .map(|(title, artist, album, plays)| SearchTrack { title, artist, album, plays })
                .collect();
            (StatusCode::OK, Json(SearchResults { artists, albums, tracks })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// GET /api/v1/sanitize — what the sanitiser *would* change, changing nothing.
//
// Always a preview first. Scrobble metadata that arrived from a real
// MusicBrainz mapping is still data, and rewriting it silently would be a
// worse failure than the mismatch it fixes.
/// Preview metadata fixes.
///
/// Session-only. **Preview-then-apply, never automatic** — and the preview
/// deliberately exposes how weak each guess is, because the operator has to
/// judge it.
///
/// Three passes. *Mixed releases*: one listen out of twelve routinely
/// arrives carrying a compilation's release MBID. The whole ballot is
/// returned, most-claimed first, so "15 releases, plurality of 4" is
/// visible as the guess it is. *Featured credits*: `Artist feat. X` is a
/// different artist to every query in the app; only the explicit
/// `feat.`/`ft.`/`featuring`/`with` forms match, because "Simon &
/// Garfunkel" is a name, not a credit. *Spelling variants*: grouped on the
/// exact string, with `case_only` flagged — a two-stage filter whose second
/// stage was coarser than the first once made every case-only variant
/// invisible.
#[utoipa::path(
    get,
    path = "/api/v1/sanitize",
    tag = "Maintenance",
    responses(
        (status = 200, description = "The proposed changes, each individually selectable.", body = SanitisePreview),
        (status = 401, response = Unauthorized),
    )
)]
async fn sanitize_preview(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let albums = state.db.albums_with_mixed_releases(user.user_id).await.unwrap_or_default();
    let artists = state.db.artists_with_featured_credits(user.user_id).await.unwrap_or_default();
    let variants = state.db.artists_with_spelling_variants(user.user_id).await.unwrap_or_default();
    let splits = state.db.tracks_with_split_attribution(user.user_id).await.unwrap_or_default();
    (
        StatusCode::OK,
        Json(SanitisePreview {
            mixed_albums: albums,
            featured_artists: artists,
            artist_variants: variants,
            split_attributions: splits,
        }),
    )
        .into_response()
}

/// What the sanitiser would change. Nothing has been changed.
#[derive(Serialize, ToSchema)]
pub struct SanitisePreview {
    /// Albums whose listens disagree about which release they are.
    pub mixed_albums: Vec<crate::db::MixedAlbum>,
    /// Artist strings carrying a featured credit, and the artist they fold
    /// into.
    pub featured_artists: Vec<crate::db::FeaturedArtist>,
    /// Artists with other spellings of themselves in the same history.
    pub artist_variants: Vec<crate::db::VariantArtist>,
    /// Tracks carrying two different artists on the same record.
    pub split_attributions: Vec<crate::db::SplitAttribution>,
}

/// One album to unify, and onto which release.
///
/// `mbid_release` is carried per edit rather than re-derived, so the operator
/// can overrule the plurality — which is the point of reviewing at all.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AlbumEdit {
    pub artist: String,
    pub album: String,
    pub mbid_release: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreditEdit {
    /// The full credit as stored, e.g. `Cœur de pirate feat. Loud`.
    pub artist: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct VariantEdit {
    /// The spelling being retired.
    pub from: String,
    /// The spelling it becomes.
    pub to: String,
}

/// One track whose stray attribution goes back to the record's own artist.
#[derive(Debug, Deserialize, ToSchema)]
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
#[derive(Debug, Deserialize, Default, ToSchema)]
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

// POST /api/v1/sanitize — apply the fixes the preview listed.
/// Apply selected metadata fixes.
///
/// Session-only. Explicit lists select individual edits; the booleans mean
/// "everything in this category". **A non-empty list wins over its
/// boolean** — asking for three edits and getting four hundred would be the
/// worst reading of an ambiguous request.
///
/// The featured-credit target is **derived, never taken from the request**,
/// or this would become "rename any artist to anything". A variant merge is
/// refused unless both spellings normalise to the same key, so merging
/// "Burial" into "Aphex Twin" cannot happen.
///
/// **A spelling merge is not reversible.** The `feat.` fold keeps the
/// original in `artist_credit`; a rename keeps nothing, because both
/// strings name the same artist.
#[utoipa::path(
    post,
    path = "/api/v1/sanitize",
    tag = "Maintenance",
    request_body = SanitizeRequest,
    responses(
        (status = 200, description = "Applied.", body = Sanitised),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
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
        Json(Sanitised {
            albums_fixed,
            album_listens_changed: album_rows,
            artists_folded,
            artist_listens_changed: artist_rows,
            variants_merged,
            variant_listens_changed: variant_rows,
            splits_merged,
            split_listens_changed: split_rows,
        }),
    )
        .into_response()
}

/// What the sanitiser changed: how many of each fix, and how many listens
/// each touched.
#[derive(Serialize, ToSchema)]
pub struct Sanitised {
    pub albums_fixed: u64,
    pub album_listens_changed: u64,
    pub artists_folded: u64,
    pub artist_listens_changed: u64,
    pub variants_merged: u64,
    pub variant_listens_changed: u64,
    pub splits_merged: u64,
    pub split_listens_changed: u64,
}

/// Dashboard counters.
///
/// Scope: `read` *(since v0.39)*. "Today" and "this week" are bucketed in
/// the user's own timezone and honour their week-start preference.
///
///
/// Deliberately **does not** carry distinct artist, album or track counts.
/// It used to compute all three, each a `COUNT(DISTINCT)` building a temp
/// B-tree over the whole history, on the most-loaded page in the
/// application — and nothing rendered any of them. `GET /api/v1/stats`
/// answers the same question against a range control, which is where it
/// belongs.
#[utoipa::path(
    get,
    path = "/api/v1/stats/dashboard",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = crate::db::DashboardStats),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
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
            Json(ErrorResponse { code: 500, error: format!("Failed to compute stats: {}", e) }),
        )
            .into_response(),
    }
}

/// Every field optional; only the fields sent change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateScrobble {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub mbid_recording: Option<String>,
    /// Fixing this also clears the resolved artwork, so the cover is re-fetched
    /// for the release you actually meant.
    pub mbid_release: Option<String>,
    /// **Not `chain_id`.** The chain to attribute this listen to.
    pub signal_chain_id: Option<i64>,
    /// Which setup of that chain was fitted. Only accepted alongside
    /// `signal_chain_id`, and must belong to that chain.
    // Written whenever `signal_chain_id` is — see `Database::update_scrobble`.
    pub chain_variant_id: Option<i64>,
    pub listening_context: Option<String>,
    pub format_type: Option<String>,
    pub codec: Option<String>,
}

// PATCH /api/v1/scrobbles/:id — edit a listen (fix MBID / chain / metadata).
/// Edit a listen.
///
/// Scope: `write` *(since v0.39)*. Only the caller's own rows.
#[utoipa::path(
    patch,
    path = "/api/v1/scrobbles/{id}",
    tag = "Listens",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = UpdateScrobble,
    responses(
        (status = 200, description = "Updated. The fresh row, so a client can re-render without a refetch.", body = EditedListen),
        (status = 400, description = "Not one of your chains, a variant without a chain, or one belonging to another chain.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 404, response = NotFound),
    )
)]
async fn update_scrobble(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateScrobble>,
) -> impl IntoResponse {
    // The chain has to be the caller's own. Unchecked, a listen could be filed
    // under another household member's chain — their gear hours and their
    // Reel — which is the Strix finding of v0.120.0 and the same hole
    // `physical::refuse_foreign_gear` closed on the shelf.
    if let Some(refused) = refuse_foreign_chain(&state, user.user_id, body.signal_chain_id).await {
        return refused;
    }
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
            Ok(Some(r)) => (StatusCode::OK, Json(r)).into_response(),
            _ => (StatusCode::OK, Json(Updated { id, status: "updated" })).into_response(),
        },
        Ok(false) => not_found(),
        Err(e) => server_error(e.to_string()),
    }
}

/// The fallback answer to an edit whose fresh row could not be read back.
#[derive(Serialize, ToSchema)]
#[schema(as = ListenUpdated)]
pub struct Updated {
    pub id: i64,
    /// Always `updated`.
    pub status: &'static str,
}

/// An edited listen: the fresh row, or — when it could not be read back —
/// just its id.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
#[allow(dead_code, clippy::large_enum_variant)] // describes the two bodies `update_scrobble` sends
pub enum EditedListen {
    Row(ScrobbleRecord),
    Id(Updated),
}

// DELETE /api/v1/scrobbles/:id — remove one listen owned by the user.
/// Delete one listen.
///
/// Scope: `write` *(since v0.39)*. Clears attached notes first.
#[utoipa::path(
    delete,
    path = "/api/v1/scrobbles/{id}",
    tag = "Listens",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 204, description = "Deleted."),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 404, response = NotFound),
    )
)]
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

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ClearQuery {
    /// Must be `true`. Anything else is a 400 and nothing is touched.
    pub confirm: Option<bool>,
}

// DELETE /api/v1/scrobbles?confirm=true — clear ALL listens for the user.
/// Delete the caller's entire history.
///
/// **Session-only and irreversible.** Deliberately not reachable with a
/// `write` token — this is the one listen operation where a lost phone
/// would be unrecoverable. Clears listen-attached notes first, since
/// foreign keys are enforced.
#[utoipa::path(
    delete,
    path = "/api/v1/scrobbles",
    tag = "Listens",
    params(ClearQuery),
    responses(
        (status = 204, description = "Cleared."),
        (status = 400, description = "`confirm=true` was not sent. Nothing was touched.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn clear_scrobbles(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ClearQuery>,
) -> impl IntoResponse {
    if q.confirm != Some(true) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                code: 400,
                error: "Refusing to clear without ?confirm=true".into(),
            }),
        )
            .into_response();
    }
    match state.db.clear_scrobbles(user.user_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StatsQuery {
    /// The window, rolling back from now: `today`, `d7` (or `week`), `month`
    /// (or `last30`), `year`, or `all`, the default. Anything else is all time.
    #[param(pattern = "^(today|d7|week|month|last30|year|all)$", default = "all")]
    pub range: Option<String>,
    /// `plays` (default) or `hours`. How the three top lists are **ranked**,
    /// not merely what number is shown. They are cut to 50 rows server-side,
    /// so re-sorting in the client would only reorder the top 50 *by plays*
    /// and would miss a long record played a handful of times — which is
    /// exactly the row switching to `hours` exists to surface. Listens whose
    /// source reported no duration count as zero and sort last.
    #[param(pattern = "^(plays|hours)$", default = "plays")]
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

/// The caller's IANA timezone from their preferences (defaults to UTC). Shared
/// with Rediscovery, which needs it for the
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
pub(crate) async fn user_week_start(state: &AppState, user_id: i64) -> u32 {
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

// GET /api/v1/stats?range=week|month|last30|year|all — ranged analytics.
/// Ranged statistics.
///
/// Scope: `read` *(since v0.39)*. Totals, top lists (50 rows each), fidelity
/// mix, decades, a 7×24 listening heatmap, breadth, genres and languages.
///
/// The genre breakdown is a share of **tag mentions, not listens** — one
/// track carries several tags. Never present it as a percentage of plays.
#[utoipa::path(
    get,
    path = "/api/v1/stats",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(StatsQuery),
    responses(
        (status = 200, description = "OK.", body = crate::db::RangedStats),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
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
        stats.heatmap = crate::analytics::heatmap_local(&to_listen_rows(rows), tz);
    }

    // Breadth, when the genre map has been fetched and genres backfilled.
    // Absent rather than zeroed otherwise — "you play 0 genres" would be a lie
    // about the listening rather than a statement about the data.
    let counts = state.db.genre_play_counts(user.user_id, after).await.unwrap_or_default();
    if !counts.is_empty() {
        let names: Vec<String> = counts.iter().map(|(n, _)| n.clone()).collect();
        let coords = state.everynoise.coordinates_for(&names).await.unwrap_or_default();
        let spread = state.everynoise.spread().await.unwrap_or(0.0);
        stats.breadth = Some(super::everynoise::breadth(&counts, &coords, spread));
    }
    (StatusCode::OK, Json(stats)).into_response()
}

// GET /api/v1/personality?range=… — five-axis listening personality.
/// Musical personality — a verdict, traits and loyalty.
///
/// Scope: `read` *(since v0.120)*.
#[utoipa::path(
    get,
    path = "/api/v1/personality",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(StatsQuery),
    responses(
        (status = 200, description = "OK.", body = crate::analytics::Personality),
        (status = 401, response = Unauthorized),
    )
)]
async fn personality(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct BulkChain {
    /// The listens to change. Anything not yours is skipped.
    pub ids: Vec<i64>,
    /// Which chain to put these listens on. **Required: present-and-null
    /// clears it; absent is a 400.** Not `chain_id` — a body with that
    /// spelling is refused rather than read as "clear".
    // Doubly wrapped for exactly that reason. It was a plain `Option<i64>`, and serde reads a bare `Option` field as
    /// defaulting to `None` — so a request that merely *misspelled* the key
    // deserialised cleanly, cleared the chain off every listen named in `ids`,
    // and answered `200 {"updated": 28}`. A typo silently undid the
    // attribution it was trying to set, and reported success. The hand-written
    // `openapi.yaml` made that easy to hit: it documented the field as
    // `chain_id`, which is exactly the spelling that wipes.
    //
    // Same shape and same reason as `default_chain_id` on
    // `PATCH /admin/tokens/:id` — the difference between "clear this" and
    // "leave it alone" has to survive deserialisation.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<i64>, required = true)]
    pub signal_chain_id: Option<Option<i64>>,
    /// Which setup of that chain was fitted. Absent means none — and it is
    /// **written either way**, never left alone: a listen moved from chain A to
    /// chain B must not keep a variant belonging to A.
    #[serde(default)]
    pub chain_variant_id: Option<i64>,
}

/// How many listens a bulk edit changed.
#[derive(Serialize, ToSchema)]
pub struct BulkUpdated {
    pub updated: u64,
}

/// A 400 when `chain_id` names a chain the caller does not own. Nonexistent
/// and somebody else's read the same, so the answer is no existence oracle.
async fn refuse_foreign_chain(
    state: &AppState,
    user_id: i64,
    chain_id: Option<i64>,
) -> Option<axum::response::Response> {
    let cid = chain_id?;
    match state.db.get_chain(user_id, cid).await {
        Ok(Some(_)) => None,
        Ok(None) => Some(bad_request("No such signal chain")),
        Err(e) => Some(server_error(e.to_string())),
    }
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

// POST /api/v1/scrobbles/bulk-chain — reassign many listens' chain at once
// (History bulk edit, for the one-output-many-rigs case).
/// Attribute many listens to one signal chain.
///
/// Session-only. Retro-attribution after adding gear — and since v0.46.0
/// the chain's *hours* follow the listens, so doing this moves the usage
/// numbers for listening you already have rather than only for future
/// plays.
///
/// **The field is `signal_chain_id`, and it is required.** This entry
/// documented it as `chain_id` for several releases, alongside three
/// parameters (`from`, `to`, `source_name`) the handler has never accepted
/// — and the mismatch was silently destructive rather than an error: the
/// body deserialised cleanly with the field missing, which meant "clear the
/// chain", so a client following the spec wiped the attribution on every
/// listen in `ids` and got `200 {"updated": N}` back. An absent field is a
/// `400` now.
///
/// Send an explicit `null` to clear. That case is real — it is how the
/// History bulk edit removes a chain — which is why absent and null cannot
/// be collapsed.
#[utoipa::path(
    post,
    path = "/api/v1/scrobbles/bulk-chain",
    tag = "Listens",
    request_body = BulkChain,
    responses(
        (status = 200, description = "Applied.", body = BulkUpdated),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
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
            Json(ErrorResponse {
                code: 400,
                error: "signal_chain_id is required — send a chain id, or an explicit null to \
                        clear the chain on these listens"
                    .into(),
            }),
        )
            .into_response();
    };

    // Only the caller's own chain, for the reason `update_scrobble` gives.
    if let Some(refused) = refuse_foreign_chain(&state, user.user_id, chain_id).await {
        return refused;
    }
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
        Ok(n) => (StatusCode::OK, Json(BulkUpdated { updated: n })).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

/// The body of `POST /api/v1/scrobbles/bulk-company`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BulkCompany {
    /// The listens to change. Anything not yours is skipped.
    pub ids: Vec<i64>,
    /// Who these listens were heard with, as names, or an explicit `null` to
    /// unmark them. **Required — absent is a 400.** Trimmed; an empty string
    /// means the same as `null`. At most 200 characters.
    // `Option<Option<String>>` for the reason `signal_chain_id` above is: this
    // overwrites the field on every listen in `ids`, so a body that merely
    // misspelled the key must not deserialise into "clear it".
    #[serde(default, deserialize_with = "deserialize_some")]
    #[schema(value_type = Option<String>, required = true, max_length = 200)]
    pub company: Option<Option<String>>,
}

// POST /api/v1/scrobbles/bulk-company — mark many listens as heard in company.
//
// **Marking is retroactive by design.** Nobody stops mid-record to note who is
// in the room; they think of it afterwards, which is why this takes a set of
// listens rather than being a field at ingest. A Shared Spool fills the same
// column in automatically — this is for the far commoner case of somebody on
// the sofa who has no account on this instance.
/// Mark many listens as heard in company.
///
/// Session-only. Records **who you were listening with**, on the listens
/// themselves.
///
/// Marking is retroactive by design: nobody stops mid-record to note who
/// is in the room. A Shared Spool fills the same column in automatically
/// from its joined members; this is for the commoner case of somebody in
/// the room who has no account on this instance, which is why it is free
/// text rather than a list of users.
///
/// **It is stored on the listen and not on the session**, which is not a
/// preference — `POST /api/v1/sessions/rebuild` deletes every session row
/// and re-inserts it, so anything held there is erased by a regroup with
/// nothing to say so.
///
/// `company` is required. Send an explicit `null` to unmark; an absent
/// field is a `400`, for the reason `signal_chain_id` on
/// `/api/v1/scrobbles/bulk-chain` is — this overwrites the field on every
/// listen in `ids`, so a misspelled key must not deserialise into "clear
/// it". An empty or whitespace-only string is treated as `null` rather
/// than becoming a listen marked as heard with nobody.
///
/// This never writes `joint_session_id`. That column says *this exact play
/// happened on several decks at once*, which only a Shared Spool can know.
#[utoipa::path(
    post,
    path = "/api/v1/scrobbles/bulk-company",
    tag = "Listens",
    request_body = BulkCompany,
    responses(
        (status = 200, description = "How many listens were changed.", body = BulkUpdated),
        (status = 400, description = "`company` was absent, or longer than 200 characters.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
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
        Ok(n) => (StatusCode::OK, Json(BulkUpdated { updated: n })).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

/// The body of `POST /api/v1/scrobbles/bulk-time`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BulkTime {
    /// The listens to move. Required and non-empty; anything not yours is
    /// skipped.
    pub ids: Vec<i64>,
    /// Seconds to move these listens by. Negative moves them earlier.
    /// **Required**, not zero, and at most a year either way.
    ///
    /// **An offset rather than an absolute time, and that is the design.** A
    /// timestamp is almost never wrong on its own: a clock was out by an hour,
    /// an evening was reconstructed from times laid out by arithmetic, or a
    /// capture arrived carrying somebody else's clock. Every one of those moves
    /// a run of listens by a constant, and a constant cannot scramble an
    /// ordering the way a set of hand-typed times can. Correcting a single
    /// listen is this operation over one id — the caller subtracts.
    ///
    // Required rather than defaulted: a body that failed to say how far to
    // move must not read as zero and report success at having done nothing.
    #[schema(required = true, maximum = 31622400, minimum = -31622400)]
    pub offset_seconds: Option<i64>,
}

// POST /api/v1/scrobbles/bulk-time — move listens in time.
//
// The half of a time correction that cannot be done at the capture stage,
// which is where it should be done when it can be — nothing is in a history
// there, so none of the traps below apply.
//
// What comes back matters as much as what it does. Neither Last.fm nor
// ListenBrainz has an edit API, so a listen that has already been forwarded
// keeps the time it was sent with, permanently; and sessions are stored
// rather than computed, so their *grouping* is not re-derived here. Both are
// reported, because a correction somebody believes reached further than it
// did is worse than one they know the edges of.
/// Move listens in time.
///
/// **An offset over a selection, not an absolute time**, and that is the
/// design rather than a limitation. A timestamp is almost never wrong on
/// its own: a clock was out by an hour, an evening was reconstructed from
/// times laid out by arithmetic, or a capture arrived carrying somebody
/// else's clock. Every one of those moves a *run* of listens by a
/// *constant*, and a constant cannot scramble an ordering the way a set of
/// hand-typed times can. Correcting a single listen is this operation over
/// one id — subtract to get the offset.
///
/// Where the listen has not been accepted yet,
/// `PATCH /api/v1/patch/captures/{id}` is the cheaper place to do it:
/// nothing is in a history there, so none of the consequences below apply.
///
/// What it takes care of:
///
/// * **`timestamp_original` is written once**, so a second correction does
///   not overwrite the observation the first one replaced.
/// * **Nothing is moved on top of another listen of the same track.** Dedup
///   runs at ingest and never again, so a duplicate created here is one
///   nothing will ever reconcile. Such a listen is skipped and is not
///   counted in `moved`.
/// * **Sessions are recomputed** — their aggregates, not their grouping.
///
/// What it reports, because it cannot fix them:
///
/// * `already_forwarded` — how many of the moved listens had already gone
///   to Last.fm or ListenBrainz. **Neither has an edit API**, so their copy
///   keeps the time it was sent with, permanently and silently. The edit is
///   still right; a screen that did not say this would let somebody believe
///   they had corrected a public record they had not.
/// * `grouping_stale` — whether a moved listen now sits further from its
///   session's neighbours than the gap those sessions were grouped by.
///   Sessions are stored rather than computed, which is what lets a manual
///   merge survive, and `sessions.uuid` is what a handed-over sitting
///   points at — so re-grouping silently on a time edit would move an
///   identity somebody else holds a reference to. Rebuild stays something
///   the user asks for.
#[utoipa::path(
    post,
    path = "/api/v1/scrobbles/bulk-time",
    tag = "Listens",
    security(("sessionCookie" = [])),
    request_body = BulkTime,
    responses(
        (status = 200, description = "What moved, and what could not be reached.", body = crate::db::TimeEdit),
        (status = 400, description = "No ids, no offset, a zero offset, or one of more than a year.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn bulk_set_time(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<BulkTime>,
) -> impl IntoResponse {
    let Some(offset) = body.offset_seconds else {
        return bad_request(
            "`offset_seconds` is required — say how far to move these, in seconds.",
        );
    };
    if body.ids.is_empty() {
        return bad_request("`ids` is required — name the listens.");
    }
    if offset == 0 {
        return bad_request("`offset_seconds` is zero — that would move nothing.");
    }
    // A year either way. Not a safety boundary — the edit is reversible by
    // running it back — but a stray milliseconds-for-seconds mix-up parks a
    // listen in the wrong decade, where it is out of every range control on
    // every screen and therefore very hard to find again.
    if offset.abs() > 366 * 86_400 {
        return bad_request(
            "`offset_seconds` is more than a year — that is almost certainly a slip",
        );
    }

    match state.db.shift_listen_times(user.user_id, &body.ids, offset).await {
        Ok(edit) => (StatusCode::OK, Json(edit)).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

fn bad_request(msg: &str) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { code: 400, error: msg.into() })).into_response()
}

// GET /api/v1/languages/status — instance-wide classification coverage.
/// Language classification coverage.
///
/// Session-only. The denominator counts only rows the backfill will
/// actually process — it did not once, so the bar could never reach 100%
/// on any history containing a skip.
#[utoipa::path(
    get,
    path = "/api/v1/languages/status",
    tag = "Maintenance",
    responses(
        (status = 200, description = "OK.", body = Coverage),
        (status = 401, response = Unauthorized),
    )
)]
async fn language_status(State(state): State<Arc<AppState>>, _user: AuthUser) -> impl IntoResponse {
    match state.db.language_coverage().await {
        Ok((total, resolved)) => {
            (StatusCode::OK, Json(Coverage { total, resolved, pending: total - resolved }))
                .into_response()
        }
        Err(e) => server_error(e.to_string()),
    }
}

// POST /api/v1/languages/backfill — admin: run a full classification pass in
// the background (script instantly, MusicBrainz for the rest).
/// Run the language backfill now.
///
/// Session-only.
#[utoipa::path(
    post,
    path = "/api/v1/languages/backfill",
    tag = "Maintenance",
    responses(
        (status = 202, description = "Started. `status` is `started`.", body = OkResponse),
        (status = 403, response = Forbidden),
        (status = 401, response = Unauthorized),
    )
)]
async fn backfill_languages_now(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    if !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse { code: 403, error: "Admin only".into() }),
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
    (StatusCode::ACCEPTED, OkResponse::new("started")).into_response()
}

/// How far a backfill has got.
#[derive(Serialize, ToSchema)]
pub struct Coverage {
    pub total: i64,
    pub resolved: i64,
    /// `total - resolved`.
    pub pending: i64,
}

#[derive(serde::Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
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

// GET /api/v1/lyrics — the words to one track, as numbered lines.
//
// **Numbered including blanks.** A stanza break is a line, because a note
// refers to a line by its index and skipping empties would renumber every
// line after one — silently re-pointing every quote already written.
//
// Reads only what is stored and never triggers a fetch. LRCLIB is free and
// asks callers to be considerate; the backfill is what paces that, and a
// screen that fetched on open would let a page refresh drive the rate.
//
// Three answers, all of them real: lines, `instrumental` (a track with no
// words, which is not a gap), and nothing at all.
/// The words to one track, as numbered lines.
///
/// Scope: `read`.
///
/// **Numbered including blank lines.** A stanza break is a line, because a
/// liner note refers to a line by its *index* — `[[line:12]]` or
/// `[[line:A:3:12]]` — and skipping empties would renumber every line after
/// one, silently re-pointing every quote already written.
///
/// **Never triggers an LRCLIB fetch.** That service is free and asks callers
/// to be considerate; the backfill is what paces it, and a screen that
/// fetched on open would let a page refresh drive the rate.
///
/// It *does* reach the user's own media server when nothing is stored yet.
/// Their Plex or Jellyfin has no rate limit and no shared budget to spend,
/// and they tag their own albums — so those words are usually right there
/// while the LRCLIB trickle is still hours from this track. Only when the
/// cache holds nothing: a stored lyric is never re-fetched on a page load.
///
/// Three answers, all real: lines, `instrumental: true` (a track with no
/// words, which is not a gap), and an empty list.
///
/// Lyrics are the provider's text. They are cached outside the listening
/// database, so `GET /api/v1/backup` does not carry them, and they are not
/// served by the public now-playing endpoint or by Patch.
#[utoipa::path(
    get,
    path = "/api/v1/lyrics",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(LyricsQuery),
    responses(
        (status = 200, description = "OK — including for a track nothing is held for.", body = LyricDocument),
    )
)]
async fn lyrics_for_track(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<LyricsQuery>,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    let doc =
        lyric_document(&state, &lyrics, user.user_id, &q.artist, &q.title, q.album.clone()).await;
    (StatusCode::OK, Json(doc)).into_response()
}

/// Everything the reader draws for one track: the words, and the furniture
/// round them.
///
/// Shared by the three ways in — the addressed reader, [`live_lyrics`] and the
/// *Look for the words* button — because they differ only in **where the track
/// comes from**, and a second copy of this would be a second answer to what a
/// lyric page shows. Same reasoning as `push_tracks` being shared by its
/// endpoint and its MCP tool.
async fn lyric_document(
    state: &Arc<AppState>,
    lyrics: &Arc<crate::lyrics::LyricsClient>,
    user_id: i64,
    artist: &str,
    title: &str,
    album_hint: Option<String>,
) -> LyricDocument {
    // The furniture round the words: which record this is on, how often it has
    // been played, and what has been written about it. All of it is about the
    // *track*, so it is gathered whether or not the lyric turns out to be held
    // — an instrumental still has a tracklist beside it, and a pin outlives the
    // words it was pinned to.
    let album = match album_hint.filter(|a| !a.trim().is_empty()) {
        Some(a) => Some(a),
        None => state.db.album_for_track(user_id, title, artist).await.ok().flatten(),
    };
    let plays = state.db.track_play_count(user_id, artist, title).await.unwrap_or(0);
    let rows = match album.as_deref() {
        Some(a) => state.db.album_track_list(user_id, artist, a).await.unwrap_or_default(),
        None => Vec::new(),
    };
    // What each sibling track has, so the sidebar can mark the instrumentals and
    // dim what there is nothing to open — the design's `· instrumental`. One
    // query for the whole record rather than a lookup per row.
    let states = lyrics
        .states_for(&rows.iter().map(|(t, ..)| (artist.to_string(), t.clone())).collect::<Vec<_>>())
        .await;
    let tracklist = rows
        .into_iter()
        .map(|(title, plays, duration, track_number)| {
            let key = crate::lyrics::LyricsClient::track_key(artist, &title);
            TracklistEntry {
                // Null for a track nobody has asked about, which is a
                // different thing from one that has no words.
                lyrics: states.get(&key).copied(),
                title,
                plays,
                duration,
                track_number,
            }
        })
        .collect::<Vec<_>>();

    // Pins, keyed by the line they hang off. One query for the whole reader
    // rather than one per line — a lyric is a couple of hundred lines.
    let mut pins: std::collections::HashMap<usize, LinePin> = std::collections::HashMap::new();
    for note in state.db.pins_for_track(user_id, artist, title).await.unwrap_or_default() {
        let Some(n) = note.line_key.as_deref().and_then(crate::db::NoteTarget::line_number) else {
            continue;
        };
        pins.insert(
            n,
            LinePin {
                note_id: note.id,
                excerpt: crate::notes::excerpt(&note.body_md, 200),
                body_md: note.body_md,
                visibility: note.visibility,
                updated_at: note.updated_at,
            },
        );
    }
    let quoted = crate::server::notes::quoted_lines(&state.db, user_id, artist, title).await;

    // Nothing held yet: try the user's own library before answering. Free —
    // their server, no rate limit — and it is the whole point of the tier: they
    // tag their albums, so the words are usually right there while the LRCLIB
    // trickle is still hours away from this track.
    //
    // Only when the cache has *nothing*. A stored `Missing` means LRCLIB was
    // asked and had none, which says nothing about their library, so that is
    // retried too — but a stored lyric is never re-fetched on a page load.
    if !matches!(
        lyrics.cached(artist, title).await,
        Some(crate::lyrics::Found::Lyrics { .. }) | Some(crate::lyrics::Found::Instrumental)
    ) {
        crate::enrich::lyrics_from_library_now(&state.db, &lyrics, artist, title).await;
    }

    let words = match lyrics.cached(artist, title).await {
        Some(crate::lyrics::Found::Lyrics { plain, synced, language }) => {
            let lines = crate::lyrics::parse_lines(plain.as_deref(), synced.as_deref());
            let held = lyrics.held(artist, title).await;
            // In Latin letters, line for line, when the lyric is in a script
            // that is romanized — made once and kept, in the reader's chosen
            // style. Index i is line i + 1, the same numbering as `lines`.
            let opts = romanize_options(state, user_id).await;
            let romanized = lyrics.romanized(artist, title, opts).await;
            let companion = lyrics.companion(artist, title).await;
            let mut roman = romanized.as_ref().map(|r| r.lines.clone()).unwrap_or_default();
            Words::Lyrics {
                language,
                timed: lines.iter().any(|l| l.at.is_some()),
                romanization: romanized.as_ref().map(|r| RomanizationInfo {
                    language: r.language.clone(),
                    indic: opts.indic.as_str(),
                    cyrillic: opts.cyrillic.as_str(),
                    macrons: opts.macrons,
                    from_companion: r.from_companion,
                    kanji: cfg!(feature = "cjk"),
                }),
                companion: companion.map(|c| CompanionInfo {
                    lrclib_id: c.lrclib_id,
                    language: c.language,
                    lines_up: c.lines_up,
                }),
                lines: lines
                    .iter()
                    .map(|l| ReaderLine {
                        n: l.index,
                        at: l.at,
                        text: l.text.clone(),
                        roman: roman.get_mut(l.index - 1).and_then(Option::take),
                        pin: pins.remove(&l.index),
                        // How many notes quote this line. Zero is sent as zero
                        // rather than omitted: the reader draws nothing for it,
                        // and a missing key would be indistinguishable from a
                        // count that failed to be computed.
                        quoted: quoted.get(&l.index).copied().unwrap_or(0),
                    })
                    .collect(),
                // The raw text, for correcting it by hand. Sent so "Fix the
                // words" opens on what is actually stored rather than on
                // the parsed lines put back together, which would quietly
                // rewrite a lyric nobody meant to touch.
                plain,
                synced,
                edited: held.as_ref().is_some_and(|h| h.edited),
                chosen: held.as_ref().is_some_and(|h| h.chosen),
                // So *Other versions* can mark the record already held.
                lrclib_id: held.as_ref().and_then(|h| h.lrclib_id),
                // Shown wherever the words are, because they are not ours
                // — and *which* not-ours matters: "your Plex" says these
                // are the pressing you played with your own tagging in it,
                // where "LRCLIB" says a stranger uploaded them. A row from
                // before provenance was recorded says so rather than
                // claiming a source it cannot vouch for.
                source: held.and_then(|h| h.source),
            }
        }
        Some(crate::lyrics::Found::Instrumental) => {
            Words::Instrumental { instrumental: true, lines: Vec::new() }
        }
        Some(crate::lyrics::Found::Missing) | None => Words::Missing { lines: Vec::new() },
    };
    LyricDocument {
        artist: artist.to_string(),
        title: title.to_string(),
        album,
        plays,
        tracklist,
        words,
    }
}

/// Everything the lyric reader draws for one track: the words, and the
/// furniture round them.
#[derive(Serialize, ToSchema)]
pub struct LyricDocument {
    pub artist: String,
    pub title: String,
    /// The record the tracklist is drawn from — as asked for, or else the one
    /// the track is filed under in your history.
    pub album: Option<String>,
    /// Your plays of this track.
    pub plays: i64,
    /// The record's tracks, empty with no record.
    pub tracklist: Vec<TracklistEntry>,
    #[serde(flatten)]
    pub words: Words,
}

#[derive(Serialize, ToSchema)]
pub struct TracklistEntry {
    pub title: String,
    pub plays: i64,
    /// Seconds.
    pub duration: Option<i64>,
    pub track_number: Option<i64>,
    /// `words` or `instrumental`; null for a track nobody has asked about,
    /// which is a different thing from one that has no words.
    pub lyrics: Option<&'static str>,
}

/// The words, or why there are none. Three answers, all of them real.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
pub enum Words {
    /// The words, as numbered lines.
    Lyrics {
        /// As detected from the text.
        language: Option<String>,
        /// Whether any line carries a time, so the reader can follow the deck.
        timed: bool,
        /// Present when the lyric is in a script that is romanized; each line
        /// then carries `roman`.
        romanization: Option<RomanizationInfo>,
        /// An LRCLIB record kept beside these words in their original script,
        /// which romanization is made from while it lines up.
        companion: Option<CompanionInfo>,
        /// **Numbered including blanks.** A stanza break is a line, because a
        /// note refers to a line by its index and skipping empties would
        /// renumber every line after one.
        lines: Vec<ReaderLine>,
        /// The stored text, for correcting it by hand.
        plain: Option<String>,
        /// The stored timed text, `[mm:ss.xx] words` per line.
        synced: Option<String>,
        /// Corrected by hand, and therefore never overwritten by a lookup.
        edited: bool,
        /// An LRCLIB record picked from *Other versions* — likewise never
        /// overwritten by a lookup or a library poll, but still LRCLIB's words.
        chosen: bool,
        /// The LRCLIB record these words are, when they are one.
        lrclib_id: Option<i64>,
        /// Where the words came from — your library, LRCLIB, or you. Shown
        /// wherever the words are, because they are not Tapedeck's.
        source: Option<String>,
    },
    /// A track with no words — an answer, not a gap.
    Instrumental {
        /// Always `true`.
        instrumental: bool,
        /// Always empty.
        lines: Vec<ReaderLine>,
    },
    /// Nothing held for this track.
    Missing {
        /// Always empty.
        lines: Vec<ReaderLine>,
    },
}

/// One numbered line.
#[derive(Serialize, ToSchema)]
pub struct ReaderLine {
    /// 1-based, blanks included.
    pub n: usize,
    /// Seconds into the track, when the lyric is timed.
    pub at: Option<f64>,
    /// Empty for a stanza break, which is a real line.
    pub text: String,
    /// The line in Latin letters, as words and parts (one Hangul block, one
    /// Japanese or Chinese token, one Indic word each). Absent where the line
    /// needs none — English, a stanza break — or the lyric is not romanized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roman: Option<crate::romanize::RomanLine>,
    /// The liner note pinned to this line, if any.
    pub pin: Option<LinePin>,
    /// How many notes quote this line. Zero is sent, never omitted.
    pub quoted: usize,
}

/// How a lyric was romanized.
#[derive(Serialize, ToSchema)]
pub struct RomanizationInfo {
    /// The language it was romanized as.
    pub language: Option<String>,
    /// `readable` or `iso`, from the `lyrics_indic_style` preference.
    pub indic: &'static str,
    /// `readable` or `iso`, from the `lyrics_cyrillic_style` preference.
    pub cyrillic: &'static str,
    /// Japanese long vowels with macrons, from the `lyrics_macrons` preference.
    pub macrons: bool,
    /// Made from the original-script companion rather than the words shown.
    pub from_companion: bool,
    /// Whether this build reads kanji and hanzi. Without it (a build with no
    /// default features) kana is romanized and ideographs are left as written.
    pub kanji: bool,
}

/// An original-script companion kept beside the words.
#[derive(Serialize, ToSchema)]
pub struct CompanionInfo {
    pub lrclib_id: Option<i64>,
    pub language: Option<String>,
    /// Same line count as the words. When false it is held but not used.
    pub lines_up: bool,
}

/// The romanization a user has chosen, from their preferences: Indic and
/// Russian styles in Settings, Japanese macrons from the reader's toggle.
pub(crate) async fn romanize_options(
    state: &Arc<AppState>,
    user_id: i64,
) -> crate::romanize::Options {
    let prefs = state.db.get_preferences(user_id).await.unwrap_or_default();
    crate::romanize::Options {
        indic: crate::romanize::IndicStyle::parse(
            prefs.get("lyrics_indic_style").and_then(|v| v.as_str()).unwrap_or("readable"),
        ),
        cyrillic: crate::romanize::CyrillicStyle::parse(
            prefs.get("lyrics_cyrillic_style").and_then(|v| v.as_str()).unwrap_or("readable"),
        ),
        macrons: prefs.get("lyrics_macrons").and_then(|v| v.as_bool()).unwrap_or(false),
    }
}

/// A liner note pinned to a line.
#[derive(Serialize, ToSchema)]
pub struct LinePin {
    pub note_id: i64,
    pub body_md: String,
    /// The first 200 characters, as text.
    pub excerpt: String,
    pub visibility: String,
    pub updated_at: i64,
}

// GET /api/v1/lyrics/live — the words for whatever is on the deck.
//
// **The reader's whole state, in one request, so the theatre stops being
// addressed by URL.** It opened as `/lyrics?artist=…&title=…`, which is right
// for a line somebody clicked in a note and wrong for the common case: the
// words were pinned to whichever track the address named, so when the next
// song started the reader went on showing the last one, and the only way out
// was to edit the query string. Reported exactly that way.
//
// Two things make this an endpoint rather than the browser stitching
// `/api/v1/now-playing` to `/api/v1/lyrics` itself:
//
// * **The server already knows what is playing.** A cold open — a fresh tab,
//   a bookmark, the shelf's *Read the words* — is one round trip instead of
//   two, and the reader has words on screen before its first deck poll.
// * **"Nothing is playing" is a state, not a failure**, and it belongs in the
//   payload rather than being assembled from an empty deck and a request the
//   caller then knows not to make.
//
// The playhead travels with it so the first paint lands on the right line; the
// shared `deck` store takes over from there, which is what keeps this to **one
// request per track** rather than one per poll. The client re-asks when the
// track changes and at no other time.
/// The words for whatever is on the deck.
///
/// Scope: `read`.
///
/// The reader's whole state in one request, so the lyric theatre stops
/// being addressed by URL. `GET /api/v1/lyrics` names a track, which is
/// right for a `[[line:...]]` chip and wrong for reading along: the words
/// stayed on whichever song the address named, so the next track left them
/// behind and the only way out was editing the query string.
///
/// The body is a `LyricDoc` with the playhead beside it, so a cold open
/// lands on the right line without waiting for a now-playing poll. Nothing
/// playing is `{ "playing": false }` and a normal answer, not an error.
///
/// **Ask it once per track, not once per poll.** The document carries the
/// tracklist and every pin on the track; a client should watch
/// `GET /api/v1/now-playing` (which it is already polling) and re-ask this
/// only when the track changes.
#[utoipa::path(
    get,
    path = "/api/v1/lyrics/live",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "The deck's track and its words, or nothing playing.", body = LiveLyrics),
    )
)]
async fn live_lyrics(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    // An empty deck is a normal answer and not an error: the reader says so and
    // waits, and picks the next track up on its own.
    let Some((np, position)) = state.now_playing.get(user.user_id) else {
        return (StatusCode::OK, Json(LiveLyrics::Idle { playing: false })).into_response();
    };
    let doc =
        lyric_document(&state, &lyrics, user.user_id, &np.artist, &np.title, np.album.clone())
            .await;
    let body = LiveLyrics::Playing {
        playing: true,
        position,
        position_ms: np.position_ms_now(),
        // Whether that position is a measurement. The reader will not drive a
        // highlight from a guess, so it has to be able to tell — the same
        // distinction `Progress` exists to keep, carried one screen further.
        position_known: np.position_known,
        paused: np.paused,
        duration: np.duration,
        doc,
    };
    (StatusCode::OK, Json(body)).into_response()
}

/// The words for whatever is on the deck, or that nothing is.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
// Built once per request and serialised straight away; boxing the big arm
// would buy nothing.
#[allow(clippy::large_enum_variant)]
pub enum LiveLyrics {
    /// Nothing is playing — a state, not a failure.
    Idle {
        /// Always `false`.
        playing: bool,
    },
    /// The lyric document for what is playing, plus the playhead.
    Playing {
        /// Always `true`.
        playing: bool,
        /// Seconds into the track.
        position: u64,
        /// The same, in milliseconds.
        position_ms: u64,
        /// Whether `position` is a measurement. Do not drive a highlight from a
        /// guess.
        position_known: bool,
        paused: bool,
        /// Seconds.
        duration: Option<u64>,
        #[serde(flatten)]
        doc: LyricDocument,
    },
}

#[derive(serde::Deserialize, ToSchema)]
pub struct FetchLyricsRequest {
    pub artist: String,
    pub title: String,
    /// Narrows the LRCLIB lookup when the caller knows it.
    pub album: Option<String>,
    /// Likewise — LRCLIB matches on length, and a wrong-length record is the
    /// usual reason a well-known song comes back with somebody else's words.
    pub duration: Option<i64>,
}

// POST /api/v1/lyrics/fetch — look for this track's words now.
//
// The backfill runs at eight tracks a minute, most-played first, which is the
// right pace for a queue nobody is watching and no use at all for the track a
// person is looking at — that one may be hours down the queue. This is the
// button for it, and it goes through `enrich::lyrics_now`, so it resolves the
// track exactly the way the backfill would have: their own library first, then
// LRCLIB, with cues winning.
//
// **Session-only, and that is deliberate**: it spends a shared politeness
// budget at a free service and writes an instance-wide cache. `read` on a
// token is for an app showing a lyric, not for driving fetches — so a token
// gets **401** here and its scopes never enter into it, the same line
// `PUT /api/v1/lyrics` and `/api/v1/sources` both sit on.
//
// A rate limit or an unreachable LRCLIB is **200 with `unavailable`**, never a
// 5xx — the convention `/lyrics/matches` and `/physical/:id/durations` already
// follow. The upstream asking us to wait is not this server failing, and
// collapsing the two would have the button report "nothing found" about a
// track it never managed to ask about.
/// Look for one track's words now.
///
/// **Session only.** A token is refused with 401 and its scopes never enter
/// into it: this spends a shared politeness budget at a free service and
/// writes an instance-wide cache, which is not something a scrobble client
/// holding `read` has any business doing.
///
/// The backfill runs at a few tracks a minute, most-played first, which is
/// the right pace for a queue nobody is watching and no use for the track
/// somebody is looking at. This is the button behind that, and it resolves
/// the track exactly as the backfill would: the user's own library first,
/// then LRCLIB, with cues winning.
///
/// **A deliberate press is not a page load**, which is why this may ask
/// LRCLIB where `GET /api/v1/lyrics` may not. It bypasses the 30-day miss
/// tombstone — LRCLIB fills its own gaps, so "nobody had this a fortnight
/// ago" is exactly the answer worth re-asking — but it will not displace
/// words already held: `already_held: true` says so, and
/// `/api/v1/lyrics/matches` is the control for disagreeing with them.
///
/// A rate limit or an unreachable LRCLIB is **200 with `unavailable`**, not
/// a 5xx. Being asked to wait is not this server failing, and reporting it
/// as "nothing found" would be a claim about the track rather than about
/// the request.
#[utoipa::path(
    post,
    path = "/api/v1/lyrics/fetch",
    tag = "Statistics",
    security(("sessionCookie" = [])),
    request_body = FetchLyricsRequest,
    responses(
        (status = 200, description = "The whole document as it now stands, so the caller can replace what it is showing — or `unavailable` with a `why`.", body = FetchedLyrics),
        (status = 400, description = "An artist or a title that is blank.", body = ErrorResponse),
        (status = 422, description = "The body is missing `artist` or `title` outright."),
    )
)]
async fn fetch_lyrics_now(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<FetchLyricsRequest>,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    if body.artist.trim().is_empty() || body.title.trim().is_empty() {
        return bad_request("A lookup needs an artist and a title");
    }

    // Words already held are not re-fetched. There is nothing to force here:
    // the cache has an answer, and *Pick a different match* is the control for
    // disagreeing with it — which already exists and spends its request on the
    // question actually being asked.
    let held = matches!(
        lyrics.cached(&body.artist, &body.title).await,
        Some(crate::lyrics::Found::Lyrics { .. }) | Some(crate::lyrics::Found::Instrumental)
    );
    let outcome = if held {
        None
    } else {
        Some(
            crate::enrich::lyrics_now(
                &state.db,
                &lyrics,
                &body.artist,
                &body.title,
                body.album.as_deref(),
                body.duration,
            )
            .await,
        )
    };

    // **"Could not ask" is not "there is nothing"**, and the reader must not
    // report the first as the second — "nobody has words for this one" is a
    // claim about the *track*, and making it after a failed request tells
    // somebody their song has no lyrics when nobody ever managed to look. The
    // cause goes to the log rather than the screen: it carries a `Debug`
    // wrapper and the configured LRCLIB URL, neither of which means anything to
    // a reader.
    let why = match &outcome {
        Some(crate::enrich::LyricOutcome {
            resolved: crate::enrich::Resolved::RateLimited(secs),
            ..
        }) => Some(format!("LRCLIB asked us to wait {secs}s — try again shortly.")),
        Some(crate::enrich::LyricOutcome {
            resolved: crate::enrich::Resolved::Unreachable,
            ..
        }) => Some("Could not reach LRCLIB just now — try again shortly.".to_string()),
        _ => None,
    };
    if let Some(why) = why {
        return (StatusCode::OK, Json(FetchedLyrics::Unavailable { unavailable: true, why }))
            .into_response();
    }

    // The whole document back, not a verdict: the reader replaces what it is
    // showing, so a fetch that found words puts them on screen without a second
    // request and without the page having to work out what changed.
    let doc = lyric_document(
        &state,
        &lyrics,
        user.user_id,
        &body.artist,
        &body.title,
        body.album.clone(),
    )
    .await;
    (StatusCode::OK, Json(FetchedLyrics::Document { already_held: held, doc })).into_response()
}

/// A lookup's outcome: the whole lyric document, or why nobody could be asked.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
// Built once per request and serialised straight away.
#[allow(clippy::large_enum_variant)]
pub enum FetchedLyrics {
    /// LRCLIB could not be asked — rate limited or unreachable. **Not** "there
    /// are no words": nothing was learned about the track.
    Unavailable {
        /// Always `true`.
        unavailable: bool,
        why: String,
    },
    /// The document, as the reader should now show it.
    Document {
        /// The words were already held, so nothing was fetched.
        already_held: bool,
        #[serde(flatten)]
        doc: LyricDocument,
    },
}

// GET /api/v1/lyrics/keys — every track this instance holds words for.
//
// One request for the whole set, exactly the shape `GET /api/v1/loves/keys`
// has and for the same reason: a history page wants to know which of fifty
// rows has something to read, and fifty questions is not the way to ask. The
// browser derives a row's key the same way `LyricsClient::track_key` does —
// **and those two must stay in step**, or "Read the words" quietly stops
// appearing and nothing errors. Same standing trap as `loveKey`.
//
// Not scoped to the caller, and it cannot be: the cache is one copy of one
// track's words for the whole instance, keyed by the track rather than by who
// played it. What it discloses is which tracks *anybody here* has had fetched,
// which is the same thing the backfill's own progress readout says.
/// Every track this instance holds words for.
///
/// Scope: `read`.
///
/// One request for the whole set, exactly the shape `GET /api/v1/loves/keys`
/// has and for the same reason: a history page wants to know which of fifty
/// rows has something to read, and fifty questions is not the way to ask.
///
/// A key is `<artist>|<title>`, both trimmed and lowercased — the same fold
/// `loveKey` uses for its artist half. **The browser's derivation and this
/// one must stay in step**, or "Read the words" quietly stops appearing on
/// rows that have lyrics and nothing errors.
///
/// Only tracks with words. An instrumental is a real answer and a finished
/// lookup, but there is nothing to open a reader on.
///
/// Not scoped to the caller, and it cannot be: the cache is one copy of one
/// track's words for the whole instance, keyed by the track rather than by
/// who played it.
#[utoipa::path(
    get,
    path = "/api/v1/lyrics/keys",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK. An instance with no lyrics cache answers an empty list rather than failing.", body = LyricKeys),
    )
)]
async fn lyrics_keys(State(state): State<Arc<AppState>>, _user: ReadAuth) -> impl IntoResponse {
    let keys = match state.lyrics.clone() {
        Some(l) => l.keys_with_lyrics().await,
        // No cache is not an error — it is an instance with no lyrics, and the
        // honest answer is that nothing has words rather than a failed request
        // that stops a history page rendering.
        None => Vec::new(),
    };
    (StatusCode::OK, Json(LyricKeys { keys })).into_response()
}

/// Every track this instance holds words for.
#[derive(Serialize, ToSchema)]
pub struct LyricKeys {
    /// `<artist key>|<title>` for each: the artist folded the way artist
    /// pages fold names (case, accents, spacing), the title trimmed and
    /// lowercased. Derive a row's key the same way.
    pub keys: Vec<String>,
}

#[derive(serde::Deserialize, ToSchema)]
pub struct FixLyricsRequest {
    pub artist: String,
    pub title: String,
    /// The words, plain. Absent clears them.
    pub plain: Option<String>,
    /// The timed form, `[mm:ss.xx] words` per line.
    pub synced: Option<String>,
}

/// Words a player found in its own files, offered by the player.
///
/// **Deliberately not `PUT /api/v1/lyrics`**, which is the correction path: that
/// one calls `set_words`, which sets `edited` and stamps the source as *You*.
/// Typing a line in is a claim only a person gets to make, and a client
/// reporting what a file contains is not correcting anything — so it must not
/// be able to mint the flag that outranks every later fetch.
///
/// Nor `additional_info` on a submission: a lyric is a few KB on every play of a
/// track already held, and ingest stays about the listening.
#[derive(serde::Deserialize, ToSchema)]
pub struct LibraryLyricsRequest {
    pub artist: String,
    pub title: String,
    /// Advisory. Not used to match — `track_key` is artist and title — and kept
    /// out of the store for the same reason.
    #[allow(dead_code)]
    pub album: Option<String>,
    pub plain: Option<String>,
    pub synced: Option<String>,
    /// Whose words these are, and **required**.
    ///
    /// The reader prints "Words via X", and that sentence is the entire reason
    /// this route exists apart from the LRCLIB one: *from your own files* and
    /// *from a lyrics service* are different statements about whose text
    /// somebody is reading. A client that will not say who it is should not get
    /// to write provenance, and inventing one here would be the placeholder
    /// User-Agent mistake in a new place.
    ///
    /// Typed `Option` and required **in the handler**, not by serde. A missing
    /// required field is rejected by axum's `Json` extractor with a 422 before
    /// any of this runs — so the client is handed a deserialiser's sentence
    /// about a field name instead of the reason, which is the trap
    /// `additional_info` already documents. The requirement is unchanged; only
    /// the answer is legible.
    pub source: Option<String>,
}

/// What became of words a player offered.
#[derive(Serialize, ToSchema)]
pub struct LibraryLyricsStored {
    /// `saved`, or `kept` when the words held were corrected by hand or picked
    /// from *Other versions* — either outranks a file's own tags, so those were
    /// left alone.
    pub status: &'static str,
    /// Why they were kept; null when saved.
    pub why: Option<&'static str>,
    /// The language of the words now held.
    pub language: Option<String>,
    /// Lines in the words now held.
    pub lines: usize,
}

/// Words a player found in its own files.
///
/// Scope: `write`.
///
/// **Not a correction.** `PUT /api/v1/lyrics` calls `set_words`, which sets
/// `edited` and stamps the source as *You* — a claim only a person typing
/// gets to make, and the flag that outranks every later fetch. A client
/// reporting what a file contains is not correcting anything, so this goes
/// through the library path instead: it refuses to write over an edited row,
/// re-reads the language from the new words, and upserts, so a repeat send
/// is harmless.
///
/// **Not `POST /api/v1/lyrics/fetch` either**, which spends a shared
/// politeness budget at LRCLIB. This is offering words, not asking for them.
///
/// Sends nothing to any third party, and the words never leave the instance
/// — not in a backup, not in an export, not to a patched deck.
#[utoipa::path(
    post,
    path = "/api/v1/lyrics/library",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    tag = "Statistics",
    request_body = LibraryLyricsRequest,
    responses(
        (status = 200, description = "Stored, or left alone because the row holds a correction.", body = LibraryLyricsStored),
        (status = 400, description = "A blank artist, title or source, or no words.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn library_lyrics(
    State(state): State<Arc<AppState>>,
    auth: WriteAuth,
    Json(body): Json<LibraryLyricsRequest>,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    if body.artist.trim().is_empty() || body.title.trim().is_empty() {
        return bad_request("Words need an artist and a title");
    }
    let source = body.source.as_deref().unwrap_or("").trim().to_string();
    if source.is_empty() {
        return bad_request("Say where the words came from — the reader shows it");
    }
    if body.plain.as_deref().unwrap_or("").trim().is_empty()
        && body.synced.as_deref().unwrap_or("").trim().is_empty()
    {
        return bad_request("There are no words in that");
    }

    // Asked before, so the answer can say *stored* rather than *left alone* —
    // `remember_library` refuses an edited row by returning early, which is the
    // right behaviour and tells the caller nothing.
    let held = lyrics.held(&body.artist, &body.title).await;
    let edited = held.as_ref().is_some_and(|h| h.edited);
    let chosen = held.as_ref().is_some_and(|h| h.chosen);
    match lyrics
        .remember_library(
            &body.artist,
            &body.title,
            body.plain.clone(),
            body.synced.clone(),
            &source,
        )
        .await
    {
        Ok(crate::lyrics::Found::Lyrics { plain, synced, language }) => {
            let _ = auth;
            (
                StatusCode::OK,
                Json(LibraryLyricsStored {
                    status: if edited || chosen { "kept" } else { "saved" },
                    why: if edited {
                        Some("A correction typed in here outranks a file's own tags")
                    } else if chosen {
                        Some("A version picked from Other versions outranks a file's own tags")
                    } else {
                        None
                    },
                    language,
                    lines: crate::lyrics::parse_lines(plain.as_deref(), synced.as_deref()).len(),
                }),
            )
                .into_response()
        }
        Ok(_) => server_error("The words did not come back".to_string()),
        Err(e) => server_error(format!("Could not store those words: {e}")),
    }
}

// PUT /api/v1/lyrics — correct the words by hand.
//
// **A value you supplied is never overwritten by a lookup**, the rule the
// shelf's running times already follow, and it is what settles the collision
// between this and the design's *newest fetch wins*: between two fetches the
// newer one wins, and between a fetch and something a person typed, the person
// does.
//
// The honest warning is about the **line count**. Every reference already
// written is an index, so a correction that adds or removes a line moves every
// quote after it onto different words — silently, because an index cannot be
// wrong, only pointed somewhere else. The response reports the count before
// and after so the caller can say so; it is not refused, because the
// correction is usually the point and the alternative is a lyric that stays
// wrong for ever.
/// Correct the words by hand.
///
/// Scope: session only.
///
/// **A value you supplied is never overwritten by a lookup** — the rule the
/// shelf's running times already follow, and what settles the collision
/// between this and "newest fetch wins": between two fetches the newer one
/// wins, and between a fetch and something a person typed, the person does.
/// The row is marked as corrected and the backfill will not write over it.
///
/// Instance-wide rather than per user, like everything else in this cache:
/// these are one track's words, not one person's opinion of them.
///
/// The response reports the line count **before and after**, and
/// `renumbered` when they differ. Every reference already written is an
/// index, so a correction that adds or removes a line moves every quote
/// after it onto different words — silently, because an index cannot be
/// wrong, only pointed somewhere else. It is reported rather than refused:
/// the correction is usually the point, and the alternative is a lyric that
/// stays wrong for ever.
#[utoipa::path(
    put,
    path = "/api/v1/lyrics",
    tag = "Statistics",
    security(("sessionCookie" = [])),
    request_body = FixLyricsRequest,
    responses(
        (status = 200, description = "Saved.", body = LyricsCorrected),
        (status = 400, description = "No artist, no title, or no words.", body = ErrorResponse),
    )
)]
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
                Json(LyricsCorrected {
                    status: "saved",
                    language,
                    lines_before: before,
                    lines_after: after,
                    // The one thing worth saying out loud: a reference is an
                    // index, so a changed count re-points every quote past the
                    // change.
                    renumbered: before > 0 && before != after,
                }),
            )
                .into_response()
        }
        Ok(_) => server_error("The correction did not come back as words".to_string()),
        Err(e) => server_error(e.to_string()),
    }
}

/// A hand correction, stored.
#[derive(Serialize, ToSchema)]
pub struct LyricsCorrected {
    /// Always `saved`.
    pub status: &'static str,
    pub language: Option<String>,
    pub lines_before: usize,
    pub lines_after: usize,
    /// The line count changed, so every `[[line:…]]` quote past the change now
    /// points at different words. Say so.
    pub renumbered: bool,
}

/// The records LRCLIB holds for a track.
#[derive(Serialize, ToSchema)]
pub struct LyricMatches {
    pub matches: Vec<LyricMatch>,
    /// Present, and `true`, when LRCLIB could not be asked. `matches` is then
    /// empty and says nothing about the track.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<bool>,
    /// Why, when `unavailable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One LRCLIB record for the track.
#[derive(Serialize, ToSchema)]
pub struct LyricMatch {
    /// LRCLIB's id — what `POST /api/v1/lyrics/adopt` takes.
    pub id: Option<i64>,
    pub title: Option<String>,
    pub artist: Option<String>,
    /// The release the record was filed under — usually what tells two
    /// uploads of one song apart.
    pub album: Option<String>,
    /// Seconds.
    pub duration: Option<i64>,
    /// Timed — a timed lyric follows the deck, a plain one cannot.
    pub synced: bool,
    /// Lines, blanks included.
    pub lines: usize,
    /// The language read off the record's script, null for Latin text. The
    /// question this list most often answers is "is there one in the original
    /// script", which a romanised upload and a native one look identical on
    /// every other field.
    pub language: Option<String>,
    /// This is the record already held.
    pub held: bool,
    pub plain: Option<String>,
    pub synced_text: Option<String>,
}

/// Which track the matches are for, and optionally what to search for.
#[derive(serde::Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LyricMatchesQuery {
    /// The track, as it is filed here.
    pub artist: String,
    pub title: String,
    /// Search LRCLIB under a different artist than the track's — for tags that
    /// name it differently (a romanised name, a "feat." credit).
    pub search_artist: Option<String>,
    /// Likewise for the title.
    pub search_title: Option<String>,
}

// GET /api/v1/lyrics/matches — the other records LRCLIB holds for a track.
//
// The honest form of the design's `LRCLIB v3 ▾`, which implies a version
// history LRCLIB does not have. These are *different records* for the same
// track, and picking one is how a wrong match gets fixed without typing the
// words out.
//
// Adopting one is `POST /api/v1/lyrics/adopt` with the record's id. It was a
// `PUT /api/v1/lyrics` with the words until v0.120.1, which credited LRCLIB's
// upload to "You" and dropped the id — see `adopt_lyrics`.
//
// It reaches the network, so it is a deliberate press rather than something a
// page load does — and a rate limit comes back as a plain answer rather than
// a 5xx, because "LRCLIB asked us to wait" is not this server failing.
/// The other records LRCLIB holds for a track.
///
/// Session or a `read` token.
///
/// **Not a version history.** LRCLIB has none — `/api/get` returns one
/// record and nothing behind it. These are *different records* for the same
/// song, from `/api/search`: other uploads, other pressings, a live take
/// filed under the same name. Picking a better one is how a wrong match is
/// fixed without typing the words out.
///
/// Records with no words are omitted. The whole gesture is "these are the
/// wrong words, give me the other ones", which an instrumental cannot
/// answer.
///
/// **Adopt one with `POST /api/v1/lyrics/adopt`**, by its `id`. The pick
/// survives every later fetch and library poll, is credited to LRCLIB, and
/// the response reports `renumbered` — which matters here more than
/// anywhere: a `[[line:...]]` reference is an index, so a different record
/// re-points every quote already written.
///
/// `search_artist` and `search_title` search under other names than the
/// track's own, for tags LRCLIB files differently. `held` marks the record
/// already stored for the track.
///
/// This reaches the network, so it belongs on a press rather than a page
/// load. A rate limit or an unreachable LRCLIB comes back **200** with
/// `unavailable: true` and a reason rather than a 5xx: the upstream asking
/// us to wait is not this server failing, and the distinction is what stops
/// a client reporting "no other matches" when it never managed to ask.
#[utoipa::path(
    get,
    path = "/api/v1/lyrics/matches",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    tag = "Statistics",
    params(LyricMatchesQuery),
    responses(
        (status = 200, description = "OK.", body = LyricMatches),
        (status = 400, description = "Missing artist or title.", body = ErrorResponse),
    )
)]
async fn lyric_matches(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<LyricMatchesQuery>,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    if q.artist.trim().is_empty() || q.title.trim().is_empty() {
        return bad_request("A search needs an artist and a title");
    }
    let _ = user;
    // A blank override is no override: the field is there to be edited, and
    // clearing it means "the track's own name" rather than "search for nothing".
    let search_artist =
        q.search_artist.as_deref().filter(|a| !a.trim().is_empty()).unwrap_or(&q.artist);
    let search_title =
        q.search_title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&q.title);
    let held_id = lyrics.held(&q.artist, &q.title).await.and_then(|h| h.lrclib_id);
    match lyrics.search(search_artist, search_title).await {
        Ok(found) => {
            let matches: Vec<_> = found
                .iter()
                .map(|r| {
                    let synced = r.synced_lyrics.as_deref().is_some_and(|t| !t.trim().is_empty());
                    LyricMatch {
                        id: r.id,
                        title: r.track_name.clone(),
                        artist: r.artist_name.clone(),
                        album: r.album_name.clone(),
                        language: crate::lyrics::language_of(
                            r.plain_lyrics.as_deref(),
                            r.synced_lyrics.as_deref(),
                        )
                        .map(str::to_string),
                        held: held_id.is_some() && r.id == held_id,
                        duration: r.duration.map(|d| d as i64),
                        // What the reader gets if this one is adopted: a timed
                        // lyric follows the deck, a plain one cannot.
                        synced,
                        lines: crate::lyrics::parse_lines(
                            r.plain_lyrics.as_deref(),
                            r.synced_lyrics.as_deref(),
                        )
                        .len(),
                        plain: r.plain_lyrics.clone(),
                        synced_text: r.synced_lyrics.clone(),
                    }
                })
                .collect();
            (StatusCode::OK, Json(LyricMatches { matches, unavailable: None, why: None }))
                .into_response()
        }
        Err(crate::lyrics::LyricsError::RateLimited(secs)) => (
            StatusCode::OK,
            Json(LyricMatches {
                matches: Vec::new(),
                unavailable: Some(true),
                why: Some(format!("LRCLIB asked us to wait {secs}s — try again shortly.")),
            }),
        )
            .into_response(),
        // Deliberately not the transport error verbatim: it carries a `Debug`
        // wrapper and the configured LRCLIB URL, neither of which means
        // anything to the person reading it. The cause goes to the log.
        Err(e) => {
            tracing::debug!("🎤 lyric match search failed for {} — {}: {e:?}", q.artist, q.title);
            (
                StatusCode::OK,
                Json(LyricMatches {
                    matches: Vec::new(),
                    unavailable: Some(true),
                    why: Some("Could not reach LRCLIB just now — try again shortly.".into()),
                }),
            )
                .into_response()
        }
    }
}

// POST /api/v1/lyrics/adopt — hold an LRCLIB record picked by hand.
//
// **By id, fetched here.** The picker used to send the chosen words back
// through `PUT /api/v1/lyrics`, the correction path, which stamped them
// "You" and dropped the record id: the reader then credited a stranger's
// upload to whoever picked it, the one thing the provenance line exists to
// stop. Fetching by id also means what is credited to LRCLIB is what LRCLIB
// holds, not whatever a client sent.
//
// Session-only, like `/lyrics/fetch`: it spends the shared LRCLIB budget and
// writes the instance-wide cache. A rate limit is 200 `unavailable`, the
// convention every LRCLIB-facing route here follows.
/// Hold an LRCLIB record picked from *Other versions*.
///
/// **Session only.** It reaches LRCLIB and writes the instance-wide cache,
/// which a token is not for.
///
/// The server fetches the record by `id` itself. The words are credited to
/// LRCLIB, and the pick is kept: no later fetch and no library poll
/// replaces it. It replaces a hand correction, since choosing is as
/// deliberate as typing and the later act wins; *Fix the words* afterwards
/// is a correction again.
///
/// Reports the line count before and after, and `renumbered` when they
/// differ: a `[[line:…]]` reference is an index, so a record with a
/// different count re-points every quote past the change.
///
/// A rate limit or an unreachable LRCLIB is **200** with `unavailable` —
/// nothing was changed and nothing was learned about the record.
#[utoipa::path(
    post,
    path = "/api/v1/lyrics/adopt",
    tag = "Statistics",
    security(("sessionCookie" = [])),
    request_body = AdoptLyricsRequest,
    responses(
        (status = 200, description = "Held — or `unavailable` with a `why`.", body = AdoptedLyrics),
        (status = 400, description = "A blank artist or title, a record with no words — or, keeping it as the original script, one that does not line up with the words held.", body = ErrorResponse),
        (status = 404, description = "LRCLIB has no record with that id.", body = ErrorResponse),
        (status = 422, description = "The body is missing a field outright."),
    )
)]
async fn adopt_lyrics(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<AdoptLyricsRequest>,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    if body.artist.trim().is_empty() || body.title.trim().is_empty() {
        return bad_request("A track needs an artist and a title");
    }
    let record = match lyrics.record(body.id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    code: 404,
                    error: "LRCLIB has no record with that id any more".to_string(),
                }),
            )
                .into_response();
        }
        Err(crate::lyrics::LyricsError::RateLimited(secs)) => {
            return (
                StatusCode::OK,
                Json(AdoptedLyrics::Unavailable {
                    unavailable: true,
                    why: format!("LRCLIB asked us to wait {secs}s — try again shortly."),
                }),
            )
                .into_response();
        }
        Err(e) => {
            tracing::debug!("🎤 lyric record {} could not be fetched: {e:?}", body.id);
            return (
                StatusCode::OK,
                Json(AdoptedLyrics::Unavailable {
                    unavailable: true,
                    why: "Could not reach LRCLIB just now — try again shortly.".to_string(),
                }),
            )
                .into_response();
        }
    };
    let has_words = [&record.plain_lyrics, &record.synced_lyrics]
        .iter()
        .any(|t| t.as_deref().is_some_and(|t| !t.trim().is_empty()));
    if record.instrumental || !has_words {
        return bad_request("That record has no words to hold");
    }
    if body.original_script {
        return keep_companion(&lyrics, &body, &record).await;
    }

    let before = match lyrics.cached(&body.artist, &body.title).await {
        Some(crate::lyrics::Found::Lyrics { plain, synced, .. }) => {
            crate::lyrics::parse_lines(plain.as_deref(), synced.as_deref()).len()
        }
        _ => 0,
    };
    match lyrics.adopt(&body.artist, &body.title, &record).await {
        Ok(crate::lyrics::Found::Lyrics { plain, synced, language }) => {
            let after = crate::lyrics::parse_lines(plain.as_deref(), synced.as_deref()).len();
            let _ = user;
            (
                StatusCode::OK,
                Json(AdoptedLyrics::Adopted {
                    status: "adopted",
                    language,
                    lines_before: before,
                    lines_after: after,
                    renumbered: before > 0 && before != after,
                }),
            )
                .into_response()
        }
        Ok(_) => server_error("The record did not come back as words".to_string()),
        Err(e) => server_error(e.to_string()),
    }
}

/// An LRCLIB record to hold for a track.
#[derive(serde::Deserialize, ToSchema)]
pub struct AdoptLyricsRequest {
    /// The track, as it is filed here — not necessarily the record's own names.
    pub artist: String,
    pub title: String,
    /// The LRCLIB record, from `GET /api/v1/lyrics/matches`.
    pub id: i64,
    /// Keep the record **beside** the words held, as their original script,
    /// rather than replacing them. For words held in Latin letters (a lyric
    /// tagged romanised in your own files): they stay what a note quotes, and
    /// romanization is made from this record. Refused unless it lines up.
    #[serde(default)]
    pub original_script: bool,
}

/// How far apart two cues on the same line may be and still be the same line.
const CUE_TOLERANCE_SECS: f64 = 2.0;

/// Keep `record` as the original-script companion of the words held — when it
/// is in a script that is romanized and its lines are the held words' lines:
/// the same count, and where both are timed, the same moments. Anything else
/// would set romanized lines under the wrong lines of the song.
async fn keep_companion(
    lyrics: &crate::lyrics::LyricsClient,
    body: &AdoptLyricsRequest,
    record: &crate::lyrics::LrcResponse,
) -> axum::response::Response {
    let Some(crate::lyrics::Found::Lyrics { plain, synced, .. }) =
        lyrics.cached(&body.artist, &body.title).await
    else {
        return bad_request(
            "There are no words held to keep it beside — use it as the words instead",
        );
    };
    let language =
        crate::lyrics::language_of(record.plain_lyrics.as_deref(), record.synced_lyrics.as_deref());
    if !crate::romanize::supports(language) {
        return bad_request(
            "That version is not in a script Tapedeck romanizes, so it adds nothing as an original",
        );
    }
    let held = crate::lyrics::parse_lines(plain.as_deref(), synced.as_deref());
    let theirs =
        crate::lyrics::parse_lines(record.plain_lyrics.as_deref(), record.synced_lyrics.as_deref());
    if held.len() != theirs.len() {
        return bad_request(&format!(
            "That version has {} lines and yours {}, so its lines would sit under the wrong ones of yours",
            theirs.len(),
            held.len()
        ));
    }
    if let Some((n, off)) = held
        .iter()
        .zip(&theirs)
        .filter_map(|(a, b)| Some((a.index, (a.at? - b.at?).abs())))
        .find(|(_, off)| *off > CUE_TOLERANCE_SECS)
    {
        return bad_request(&format!(
            "That version's timing is {off:.1}s away from yours at line {n}, so its lines are not yours"
        ));
    }
    match lyrics.set_companion(&body.artist, &body.title, record).await {
        Ok(()) => (
            StatusCode::OK,
            Json(AdoptedLyrics::Companion {
                status: "companion",
                language: language.map(str::to_string),
                lines: theirs.len(),
            }),
        )
            .into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

// DELETE /api/v1/lyrics/companion — stop romanizing from a companion.
/// Forget the original-script companion kept beside a track's words.
///
/// Session only. The words are untouched; romanization goes back to being
/// made from them (which, for words held in Latin letters, means none).
#[utoipa::path(
    delete,
    path = "/api/v1/lyrics/companion",
    tag = "Statistics",
    security(("sessionCookie" = [])),
    params(LyricsQuery),
    responses(
        (status = 200, description = "Forgotten, or there was none.", body = CompanionCleared),
        (status = 400, description = "Missing artist or title.", body = ErrorResponse),
    )
)]
async fn clear_companion(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<LyricsQuery>,
) -> impl IntoResponse {
    let Some(lyrics) = state.lyrics.clone() else {
        return server_error("This instance has no lyrics cache".to_string());
    };
    if q.artist.trim().is_empty() || q.title.trim().is_empty() {
        return bad_request("A track needs an artist and a title");
    }
    let _ = user;
    match lyrics.clear_companion(&q.artist, &q.title).await {
        Ok(()) => (StatusCode::OK, Json(CompanionCleared { status: "cleared" })).into_response(),
        Err(e) => server_error(e.to_string()),
    }
}

/// An original-script companion, forgotten.
#[derive(Serialize, ToSchema)]
pub struct CompanionCleared {
    /// Always `cleared`.
    pub status: &'static str,
}

/// A picked record, held — or why LRCLIB could not be asked.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
pub enum AdoptedLyrics {
    /// LRCLIB could not be asked. Nothing was changed.
    Unavailable {
        /// Always `true`.
        unavailable: bool,
        why: String,
    },
    /// Kept beside the words as their original script (`original_script`).
    Companion {
        /// Always `companion`.
        status: &'static str,
        language: Option<String>,
        lines: usize,
    },
    /// Held.
    Adopted {
        /// Always `adopted`.
        status: &'static str,
        language: Option<String>,
        lines_before: usize,
        lines_after: usize,
        /// The line count changed, so every `[[line:…]]` quote past the change
        /// now points at different words. Say so.
        renumbered: bool,
    },
}

// GET /api/v1/lyrics/status — how far the lyric language pass has got.
//
// Reports what it is *for* rather than a lyric count: the pass exists to empty
// the `English/Other` bucket, which is a shrug rather than a measurement and
// is most of a transliterated history. `unresolved` is what is left to try.
//
// `instrumental` is reported separately because it is a real answer — a track
// with no words has no language — and folding it into "found" would present a
// finished lookup as an unfinished one.
/// How far the lyric language pass has got.
///
/// Scope: session only.
///
/// Tapedeck fetches lyrics from [LRCLIB](https://lrclib.net) for one
/// reason: **the language a transliterated title cannot give.** Script
/// detection settles any non-Latin title with certainty and gives up on
/// Latin text, so a romanised title falls through to `English/Other`, which
/// is a shrug rather than a measurement and is most of a real
/// transliterated history. The lyric is where that title's own script went.
///
/// Lyrics are stored in the MusicBrainz cache database rather than in the
/// listening database, so `GET /api/v1/backup` does **not** carry them —
/// they are third-party copyrighted text, not the user's listening.
///
/// `instrumental` is counted separately because it is a real answer: a
/// track with no words has no language, and folding it into "found" would
/// report a finished lookup as an unfinished one.
#[utoipa::path(
    get,
    path = "/api/v1/lyrics/status",
    tag = "Maintenance",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = LyricsCoverage),
    )
)]
async fn lyrics_status(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    // A COUNT. This asked `tracks_wanting_language(100_000)` and took `.len()`
    // of the answer — a hundred thousand rows, with their albums and their
    // running times, fetched to be counted and dropped. That query was also
    // quadratic at the time, so this endpoint could hold a pool connection for
    // twelve minutes; the card that reads it made a page load do that.
    let unresolved = state.db.count_tracks_wanting_language().await.unwrap_or(0);
    let Some(lyrics) = state.lyrics.clone() else {
        return (
            StatusCode::OK,
            Json(LyricsCoverage::unavailable(
                "The lyrics cache could not be opened, so the language pass is skipped.",
                unresolved,
            )),
        )
            .into_response();
    };
    // A cache that cannot be read is reported as unavailable rather than as a
    // cache with nothing in it. Those were the same answer until the Settings
    // card gave this a reader — an unreadable cache said "0 tracks asked
    // about", which is a failed read wearing the clothes of a measurement.
    let Some((asked, with_lyrics, instrumental)) = lyrics.coverage().await else {
        return (
            StatusCode::OK,
            Json(LyricsCoverage::unavailable(
                "The lyrics cache could not be read, so its coverage is unknown.",
                unresolved,
            )),
        )
            .into_response();
    };
    let _ = user;
    (
        StatusCode::OK,
        Json(LyricsCoverage {
            available: true,
            why: None,
            asked: Some(asked),
            with_lyrics: Some(with_lyrics),
            instrumental: Some(instrumental),
            unresolved,
        }),
    )
        .into_response()
}

/// How far the lyric language pass has got. The three cache counts are absent
/// when the cache cannot be read — unknown, not zero.
#[derive(Serialize, ToSchema)]
pub struct LyricsCoverage {
    pub available: bool,
    /// Why not, when not `available`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<&'static str>,
    /// Tracks LRCLIB has been asked about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub with_lyrics: Option<i64>,
    /// A real answer — a track with no words has no language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrumental: Option<i64>,
    /// Tracks still filed under `English/Other` that the pass could try.
    pub unresolved: i64,
}

impl LyricsCoverage {
    fn unavailable(why: &'static str, unresolved: i64) -> Self {
        Self {
            available: false,
            why: Some(why),
            asked: None,
            with_lyrics: None,
            instrumental: None,
            unresolved,
        }
    }
}

// GET /api/v1/mbids/status — how much of this user's history MusicBrainz has
// identified at recording level.
//
// Three numbers, not two. `unmatched` is the distinct tracks MusicBrainz was
// asked about and had nothing for — real listening simply not in their
// database — and it is reported because without it a progress bar that stops
// short reads as a stall rather than as an answer.
/// MusicBrainz recording coverage.
///
/// Session-only.
///
/// A recording MBID is written by `flush_pending` alone, which only ever
/// sees listens on their way to a sink — so an **imported** listen never got
/// one, and nothing else writes the column. On a mostly-imported history
/// that leaves most of the listening unidentified, and with it excluded from
/// every backfill that keys off a recording: ISRC, work-language, release
/// year.
///
/// `unmatched_tracks` is the reason this reports three numbers instead of
/// two. It counts distinct tracks MusicBrainz was **asked about and had
/// nothing for** — real listening simply not in their database. Without it a
/// bar that stops short reads as a stall rather than as an answer.
#[utoipa::path(
    get,
    path = "/api/v1/mbids/status",
    tag = "Maintenance",
    responses(
        (status = 200, description = "OK.", body = MbidCoverage),
        (status = 401, response = Unauthorized),
    )
)]
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
        Json(MbidCoverage {
            total,
            resolved,
            pending: total - resolved,
            unmatched_tracks: unmatched,
        }),
    )
        .into_response()
}

/// Recording-level MusicBrainz coverage of your history.
#[derive(Serialize, ToSchema)]
pub struct MbidCoverage {
    /// Listens.
    pub total: i64,
    /// Listens carrying a recording MBID.
    pub resolved: i64,
    pub pending: i64,
    /// Distinct tracks MusicBrainz was asked about and had nothing for — real
    /// listening simply not in their database. Without it a progress bar that
    /// stops short reads as a stall rather than as an answer.
    pub unmatched_tracks: i64,
}

// POST /api/v1/mbids/backfill — admin: resolve missing recording MBIDs in the
// background.
//
// This is the pass that unblocks the others: the ISRC, work-language and
// release-year lookups all key off a recording MBID, and an imported listen
// never had one because only `flush_pending` ever wrote the column.
//
// Bounded per iteration rather than let loose, because every miss is a
// rate-limited MusicBrainz search — and it stops as soon as a pass resolves
// nothing, which on a history where the rest is genuinely unknown is the
// honest end rather than a loop.
/// Resolve missing recording MBIDs now.
///
/// Session-only, **admin only**. Runs in the background; poll
/// `/api/v1/mbids/status` for progress.
///
/// This is the pass that unblocks the others, so it is worth running before
/// the ISRC or language backfills on a history that came from an import.
///
/// A name search can be wrong, and a wrong MBID is worse than none — it
/// would be written onto every play of the track and then poison the ISRC
/// and language lookups downstream, with nothing erroring. So a result is
/// only accepted when it **agrees with the question**: the artist matches,
/// and the title matches once a trailing qualifier is stripped. Misses are
/// negative-cached, so a track already asked about is not asked again while
/// that answer is current.
#[utoipa::path(
    post,
    path = "/api/v1/mbids/backfill",
    tag = "Maintenance",
    responses(
        (status = 202, description = "Started. `status` is `started`.", body = OkResponse),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
async fn backfill_mbids_now(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    if !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse { code: 403, error: "Admin only".into() }),
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
    (StatusCode::ACCEPTED, OkResponse::new("started")).into_response()
}

// GET /api/v1/artwork/status — cover-art coverage for the calling user.
/// Artwork coverage.
///
/// Session-only. **Numerator and denominator share a population** — both
/// exclude skips. They did not once, so the two were fractions of different
/// wholes.
#[utoipa::path(
    get,
    path = "/api/v1/artwork/status",
    tag = "Maintenance",
    responses(
        (status = 200, description = "OK.", body = Coverage),
        (status = 401, response = Unauthorized),
    )
)]
async fn artwork_status(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.artwork_coverage(user.user_id).await {
        Ok((total, resolved)) => {
            (StatusCode::OK, Json(Coverage { total, resolved, pending: total - resolved }))
                .into_response()
        }
        Err(e) => server_error(e.to_string()),
    }
}

// POST /api/v1/artwork/backfill — admin: resolve missing cover art in the
// background. Rate-limited by the MusicBrainz client, so a large history takes
// a while; the status endpoint is the progress bar.
/// Run the artwork backfill now.
///
/// Session-only. Providers return **URLs, never bytes** — Deezer's terms
/// discourage persisting artwork. The queue keys on `artwork_url` **only**;
/// `caa_id` is written by three other paths, so keying on it silently marks
/// rows done that have no URL.
#[utoipa::path(
    post,
    path = "/api/v1/artwork/backfill",
    tag = "Maintenance",
    responses(
        (status = 202, description = "Started. `status` is `started`.", body = OkResponse),
        (status = 403, response = Forbidden),
        (status = 401, response = Unauthorized),
    )
)]
async fn backfill_artwork_now(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    if !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse { code: 403, error: "Admin only".into() }),
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
    (StatusCode::ACCEPTED, OkResponse::new("started")).into_response()
}

fn not_found() -> axum::response::Response {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { code: 404, error: "Scrobble not found".into() }))
        .into_response()
}
/// A 4xx with a message, for the cases the caller can fix.
fn err_response(code: StatusCode, msg: String) -> axum::response::Response {
    (code, Json(ErrorResponse { code: code.as_u16(), error: msg })).into_response()
}

fn server_error(msg: String) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { code: 500, error: msg }))
        .into_response()
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_scrobbles,
    clear_scrobbles,
    update_scrobble,
    delete_scrobble,
    bulk_set_time,
    bulk_set_company,
    bulk_set_chain,
    search,
    list_skips,
    dashboard_stats,
    ranged_stats,
    personality,
    lyrics_for_track,
    fix_lyrics,
    library_lyrics,
    live_lyrics,
    fetch_lyrics_now,
    lyric_matches,
    adopt_lyrics,
    clear_companion,
    lyrics_keys,
    lyrics_status,
    mbid_status,
    backfill_mbids_now,
    artwork_status,
    backfill_artwork_now,
    language_status,
    backfill_languages_now,
    sanitize_preview,
    sanitize_apply,
))]
pub struct Api;

pub fn create_scrobbles_router() -> Router<Arc<AppState>> {
    use axum::routing::patch;
    Router::new()
        .route("/api/v1/scrobbles", get(list_scrobbles).delete(clear_scrobbles))
        .route("/api/v1/scrobbles/bulk-chain", axum::routing::post(bulk_set_chain))
        .route("/api/v1/scrobbles/bulk-company", axum::routing::post(bulk_set_company))
        .route("/api/v1/scrobbles/bulk-time", axum::routing::post(bulk_set_time))
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
        .route("/api/v1/lyrics/library", axum::routing::post(library_lyrics))
        .route("/api/v1/lyrics/live", get(live_lyrics))
        .route("/api/v1/lyrics/fetch", axum::routing::post(fetch_lyrics_now))
        .route("/api/v1/lyrics/keys", get(lyrics_keys))
        .route("/api/v1/lyrics/matches", get(lyric_matches))
        .route("/api/v1/lyrics/adopt", axum::routing::post(adopt_lyrics))
        .route("/api/v1/lyrics/companion", axum::routing::delete(clear_companion))
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
