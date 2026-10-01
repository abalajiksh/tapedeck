//! Star ratings pulled off a media server, and the threshold that makes them
//! loves.
//!
//! Four endpoints, all **session-only**. Pulling spends the credential of a
//! configured source, which puts this on the same side of the line as
//! `/api/v1/sources`: a scrobble client has no business reaching into the
//! user's Plex, so an API token gets 401 here and its scopes never enter into
//! it.
//!
//! ## Preview, then apply
//!
//! `POST /api/v1/ratings/apply` takes `dry_run`, and the preview and the real
//! thing go through **the same function** — `Database::apply_rating_loves`. A
//! preview that said "42 tracks" about an apply that then did something else is
//! worse than no preview, which is the rule the sanitiser already follows and
//! the reason `import_is_live` shares its dry run too.
//!
//! That matters more than usual here because of what a love *is*. A love of a
//! recording is mirrored outward to Last.fm and ListenBrainz when it is made
//! through the loves endpoint — so a threshold set too low is not a screen
//! being wrong, it is somebody's permanent public record being written to. See
//! the note on forwarding below for what this does about that.
//!
//! ## Nothing here forwards
//!
//! A love made by this module is stored and **not** mirrored to Last.fm or
//! ListenBrainz, which is the same call `loves_pull` makes in the opposite
//! direction: those are loves being *imported*, not loves being *pressed*, and
//! pushing a few thousand of them at somebody's public profile because they
//! moved a slider is not a decision this feature has standing to make. The
//! threshold is a knob the user is expected to try at several settings, and
//! every setting would otherwise be a broadcast.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};
use utoipa::ToSchema;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{IncomingRating, UserSource};
use crate::openapi::{BadRequest, Unauthorized};
use crate::ratings::{display_scale_from_prefs, DisplayScale, LoveRule};
use crate::sources::{MediaBrowserSource, NavidromeSource, PlexSource};

fn err(code: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (code, Json(ErrorResponse { code: code.as_u16(), error: msg.into() })).into_response()
}

/// What one source's pull did.
///
/// `error` and a zero count are different answers and both are reported: a
/// server that could not be reached is not a library with no ratings in it, and
/// a screen that showed them the same way would send somebody looking for
/// ratings that are sitting right there.
/// What one source handed over.
#[derive(Debug, serde::Serialize, ToSchema)]
struct SourcePull {
    source_id: i64,
    kind: String,
    label: String,
    found: usize,
    error: Option<String>,
}

/// Pull the ratings one source holds.
///
/// Every failure is reported through [`crate::logging::error_chain`], because
/// the top-level message of a transport error names the *operation* and never
/// the fault — "error sending request for url (…)" is the same sentence for a
/// refused connection, a DNS failure, a connect timeout and a TLS handshake the
/// peer cut off. That cost four rounds of network archaeology on a live Pi
/// once; the cause was hanging off the error the whole time.
///
/// Roon is deliberately absent and is *reported* as having nothing rather than
/// returning an empty list: it publishes no rating, favourite or thumb field
/// anywhere in its extension API — checked against the crate's own types — and
/// a source that silently returned nothing would look broken. Same discipline
/// as Phase 10's rule that a source declares its own blind spots.
async fn pull_one(source: &UserSource) -> Result<Vec<IncomingRating>, String> {
    let egress = source.egress();
    if source.kind != "roon" {
        egress.check_url(&source.url).map_err(|e| e.to_string())?;
    }
    match source.kind.as_str() {
        "plex" => {
            let token = source.credential("token").ok_or("Plex token is missing")?;
            let mut plex = PlexSource::new(source.url.clone(), token, egress);
            // The section list is what the walk iterates, and it is only
            // populated by `initialize`.
            plex.initialize().await.map_err(|e| crate::logging::error_chain(&*e))?;
            plex.fetch_ratings().await.map_err(|e| crate::logging::error_chain(&*e))
        }
        "jellyfin" | "emby" => {
            let key = source
                .credential("api_key")
                .ok_or_else(|| format!("{} API key is missing", source.kind))?;
            let account = source.remote_account.clone().unwrap_or_default();
            let client = if source.kind == "emby" {
                MediaBrowserSource::emby(source.url.clone(), key, account, egress)
            } else {
                MediaBrowserSource::jellyfin(source.url.clone(), key, account, egress)
            };
            client.fetch_ratings().await.map_err(|e| crate::logging::error_chain(&*e))
        }
        "navidrome" => {
            let (Some(u), Some(p)) = (source.credential("username"), source.credential("password"))
            else {
                return Err("Navidrome credentials are missing".into());
            };
            let mut nd = NavidromeSource::new(source.url.clone(), u, p, egress);
            nd.fetch_ratings().await.map_err(|e| crate::logging::error_chain(&*e))
        }
        "roon" => Err("Roon publishes no ratings — its API has no rating, \
                       favourite or thumb field at all"
            .into()),
        other => Err(format!("A {other} source has no ratings to read")),
    }
}

// POST /api/v1/ratings/pull — read every configured library server.
//
// Runs inline rather than as a background job, like `loves_pull`: a library's
// ratings are thousands of rows at most, not the hundreds of thousands a
// listen import walks, so there is nothing here worth the progress-tracking
// machinery.
//
// **One source failing does not stop the others**, and a source that answered
// has what it said replaced wholesale — a rating cleared on the server simply
// stops appearing in its answer, so the only way to notice is to compare, and
// the cheapest correct comparison is to forget that source's rows first. The
// forget and the write are *not* one transaction, and that is a deliberate
// trade: the worst case is a pull that dies halfway leaving a source's ratings
// partly read, which the next pull fixes, against holding a write lock for the
// length of a network walk.
/// Read the ratings your media servers hold.
///
/// Session-only — it spends the credential of a configured source.
///
/// Walks every enabled library source and replaces what that source said
/// last time, so a rating raised, lowered or cleared on the server is
/// raised, lowered or cleared here: the server is the authority on its own
/// ratings and this is a copy of them. One source failing does not stop the
/// others, and an error is reported separately from a count of zero — a
/// server that could not be reached is not a library with no ratings in it.
///
/// **Roon is reported as having nothing rather than returning an empty
/// list.** Its extension API publishes no rating, favourite or thumb field
/// at all, so a source that silently returned nothing would look broken.
///
/// Ratings are stored; nothing becomes a love until
/// `/api/v1/ratings/apply`.
#[utoipa::path(
    post,
    path = "/api/v1/ratings/pull",
    tag = "Loves",
    responses(
        (status = 200, description = "Done.", body = RatingsPulled),
        (status = 401, response = Unauthorized),
    )
)]
async fn pull_ratings(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let sources = match state.db.list_sources_for_user(user.user_id).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let mut reports: Vec<SourcePull> = Vec::new();
    for source in sources.iter().filter(|s| s.enabled) {
        let label =
            if source.label.trim().is_empty() { source.kind.clone() } else { source.label.clone() };
        match pull_one(source).await {
            Ok(rows) => {
                if let Err(e) = state.db.forget_source_ratings(user.user_id, source.id).await {
                    reports.push(SourcePull {
                        source_id: source.id,
                        kind: source.kind.clone(),
                        label,
                        found: 0,
                        error: Some(e.to_string()),
                    });
                    continue;
                }
                let written = state
                    .db
                    .upsert_library_ratings(user.user_id, source.id, &source.kind, &rows)
                    .await;
                match written {
                    Ok(n) => reports.push(SourcePull {
                        source_id: source.id,
                        kind: source.kind.clone(),
                        label,
                        found: n,
                        error: None,
                    }),
                    Err(e) => reports.push(SourcePull {
                        source_id: source.id,
                        kind: source.kind.clone(),
                        label,
                        found: 0,
                        error: Some(e.to_string()),
                    }),
                }
            }
            Err(e) => {
                warn!("⭐ {} ratings: {}", source.kind, e);
                reports.push(SourcePull {
                    source_id: source.id,
                    kind: source.kind.clone(),
                    label,
                    found: 0,
                    error: Some(e),
                })
            }
        }
    }

    let found: usize = reports.iter().map(|r| r.found).sum();
    info!("⭐ Pulled {} rating(s) across {} source(s)", found, reports.len());
    (StatusCode::OK, Json(RatingsPulled { found, sources: reports })).into_response()
}

/// Ratings read from every enabled media server.
#[derive(Serialize, ToSchema)]
struct RatingsPulled {
    /// Ratings stored, across every source.
    found: usize,
    /// One entry per enabled source. A source that failed says why and keeps
    /// what it held before.
    sources: Vec<SourcePull>,
}

/// The ratings held, on the scale you read them on, and the rule that turns
/// them into loves.
#[derive(Serialize, ToSchema)]
struct RatingsOverview {
    /// The display scale's id.
    scale: &'static str,
    scale_label: &'static str,
    /// Every threshold the scale offers.
    scale_choices: Vec<f32>,
    /// The love threshold, on this scale.
    threshold: f32,
    /// Whether a starred item counts as loved whatever its rating.
    include_starred: bool,
    sources: Vec<crate::db::RatingSourceSummary>,
    /// Up to 60 ratings, to show what the rule would do.
    sample: Vec<RatingSample>,
}

#[derive(Serialize, ToSchema)]
struct RatingSample {
    /// `track`, `album` or `artist`.
    kind: String,
    artist: String,
    name: String,
    album: Option<String>,
    source_kind: String,
    starred: bool,
    /// The raw number, exactly as the server gave it — "7 out of 10, from
    /// Plex".
    rating: Option<f64>,
    /// What `rating` was out of.
    scale_max: f64,
    /// The same rating on your display scale.
    shown: f32,
    /// Whether the current rule loves it.
    loved: bool,
}

// GET /api/v1/ratings — what is held, and the rule that would be applied.
//
// Reports the display scale and the threshold **in that scale's own units**,
// because that is what was typed and what a settings screen has to show back.
// The fraction each rating actually sits at travels beside its raw number, so
// a screen can draw the stars without re-deriving the conversion and drifting
// from the server's.
/// Star ratings held from your media servers, and the rule applied to them.
///
/// Session-only. Reading is a fact about your library and pulling spends a
/// source's credential, so a token gets 401 here and scopes never enter
/// into it — the same line `/api/v1/sources` sits on.
///
/// Every rating carries **the raw number the server gave and what that
/// number was out of**, because a bare `7` means nothing: Plex counts to
/// ten (half stars, so a 7 is three and a half), Subsonic to five, and
/// Jellyfin/Emby's legacy `Rating` to ten. `shown` is the same rating on
/// the display scale you chose, and is presentation only — every threshold
/// comparison is made on the underlying fraction, so a rating that *reads*
/// as four stars on a coarse scale does not thereby clear a four-star bar.
///
/// `threshold` comes back in the units of `scale`, which is what you typed.
#[utoipa::path(
    get,
    path = "/api/v1/ratings",
    tag = "Loves",
    responses(
        (status = 200, description = "What is held.", body = RatingsOverview),
        (status = 401, response = Unauthorized),
    )
)]
async fn get_ratings(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let prefs = state.db.get_preferences(user.user_id).await.unwrap_or_default();
    let scale = display_scale_from_prefs(&prefs);
    let rule = LoveRule::from_prefs(&prefs);

    let sources = match state.db.rating_sources(user.user_id).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let rows = state.db.list_library_ratings(user.user_id, 60).await.unwrap_or_default();
    let sample: Vec<RatingSample> = rows
        .iter()
        .map(|r| RatingSample {
            kind: r.kind.clone(),
            artist: r.artist.clone(),
            name: r.name.clone(),
            album: r.album.clone(),
            source_kind: r.source_kind.clone(),
            starred: r.starred,
            // The original, kept and shown: "7 out of 10, from Plex".
            rating: r.rating,
            scale_max: r.scale_max,
            // And the same thing on the scale this person reads in.
            shown: scale.show(r.fraction()),
            loved: rule.loves(
                r.rating.filter(|v| *v > 0.0).map(|v| {
                    crate::ratings::Rating::new(
                        v as f32,
                        crate::ratings::SourceScale::new(r.scale_max as f32),
                    )
                }),
                r.starred,
            ),
        })
        .collect();

    (
        StatusCode::OK,
        Json(RatingsOverview {
            scale: scale.id(),
            scale_label: scale.label(),
            scale_choices: scale.choices(),
            threshold: scale.show(rule.at_least),
            include_starred: rule.include_starred,
            sources,
            sample,
        }),
    )
        .into_response()
}

#[derive(Deserialize, ToSchema)]
struct ApplyRequest {
    /// The cut-off, in the units of `scale`. Absent means "whatever is stored".
    #[serde(default)]
    threshold: Option<f32>,
    /// Which scale `threshold` is expressed in. Absent means the stored one.
    #[serde(default)]
    scale: Option<String>,
    #[serde(default)]
    include_starred: Option<bool>,
    /// Preview only. **Required** — see the note.
    dry_run: bool,
    /// Store the rule as well as applying it.
    #[serde(default)]
    save: bool,
}

// POST /api/v1/ratings/apply — love what clears the bar, withdraw what no
// longer does.
//
// **`dry_run` is required and has no default.** The two things this endpoint
// can do are "tell me what would happen" and "write to my Loved page", and
// guessing from an absent field is the one thing it must not do — the
// `bulk-chain` lesson, in a place where the wrong guess is a few thousand
// rows. A body that merely misspelled the key is a 400.
/// Love what clears the threshold, and withdraw what no longer does.
///
/// Session-only.
///
/// `dry_run` is **required and has no default**: the two things this can do
/// are "tell me what would happen" and "write to my Loved page", and a body
/// that merely misspelled the key must not be read as an answer either way.
/// A preview and a real run go through the same code, so a preview cannot
/// describe something the apply then does differently — and a preview
/// writes nothing at all, including no entities.
///
/// Two rules hold it safe. **A rating on something you have never listened
/// to is kept and not loved** — loves point at entities and entities are
/// things you actually played — and the count comes back as `unmatched` so
/// it is visible rather than silently absent. And **only loves this
/// instance derived are ever withdrawn**: one pressed by hand, or pulled
/// from Last.fm or ListenBrainz, is outside the reach of a threshold
/// however far it moves.
///
/// Nothing here is forwarded to Last.fm or ListenBrainz. These are loves
/// being imported, not loves being pressed, and a threshold you are
/// expected to try at several settings would otherwise be a broadcast at
/// every setting.
#[utoipa::path(
    post,
    path = "/api/v1/ratings/apply",
    tag = "Loves",
    request_body = ApplyRequest,
    responses(
        (status = 200, description = "What happened, or what would.", body = RatingsApplied),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn apply_ratings(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    body: Option<Json<ApplyRequest>>,
) -> impl IntoResponse {
    let Some(Json(req)) = body else {
        return err(
            StatusCode::BAD_REQUEST,
            "dry_run is required: true to preview, false to apply",
        );
    };

    let prefs = state.db.get_preferences(user.user_id).await.unwrap_or_default();
    let stored = LoveRule::from_prefs(&prefs);
    let scale = match req.scale.as_deref() {
        Some(s) => DisplayScale::from_id(s),
        None => display_scale_from_prefs(&prefs),
    };
    let rule = LoveRule::new(
        req.threshold.map(|t| scale.fraction_of(t)).unwrap_or(stored.at_least),
        req.include_starred.unwrap_or(stored.include_starred),
    );

    // Saved before applying, so a page that reloads after a real apply reads
    // the rule that produced what it is looking at.
    if req.save && !req.dry_run {
        let patch = serde_json::json!({
            "rating_scale": scale.id(),
            "rating_love_threshold": scale.show(rule.at_least),
            "rating_love_starred": rule.include_starred,
        });
        if let Err(e) = state.db.merge_preferences(user.user_id, &patch).await {
            return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }

    match state.db.apply_rating_loves(user.user_id, rule, req.dry_run).await {
        Ok(report) => {
            if !req.dry_run {
                info!(
                    "⭐ Ratings applied: {} loved, {} withdrawn, {} already, {} not in history",
                    report.loved, report.withdrawn, report.already, report.unmatched
                );
            }
            (
                StatusCode::OK,
                Json(RatingsApplied {
                    threshold: scale.show(rule.at_least),
                    scale: scale.id(),
                    include_starred: rule.include_starred,
                    report,
                }),
            )
                .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// DELETE /api/v1/ratings — forget the stored ratings.
///
/// **Does not un-love anything**, and that is the point rather than an
/// omission: a love already made is a statement that was made, and this is a
/// request to forget the ratings. Un-loving is what moving the threshold to the
/// top and applying does, which is a separate and visible act.
/// What a rule did, or would do.
#[derive(Serialize, ToSchema)]
struct RatingsApplied {
    /// The rule applied, on `scale`.
    threshold: f32,
    scale: &'static str,
    include_starred: bool,
    report: crate::db::RatingApply,
}

/// Ratings forgotten.
#[derive(Serialize, ToSchema)]
struct RatingsCleared {
    forgotten: u64,
    note: &'static str,
}

/// Forget the stored ratings.
///
/// Session-only. **Does not un-love anything** — a love already made is a
/// statement that was made. To take those back, raise the threshold and
/// apply, which is a separate and visible act.
#[utoipa::path(
    delete,
    path = "/api/v1/ratings",
    tag = "Loves",
    responses(
        (status = 200, description = "Forgotten.", body = RatingsCleared),
        (status = 401, response = Unauthorized),
    )
)]
async fn clear_ratings(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.clear_library_ratings(user.user_id).await {
        Ok(n) => (
            StatusCode::OK,
            Json(RatingsCleared {
                forgotten: n,
                note: "Loves already made are left alone — raise the threshold and apply to \
                       take those back.",
            }),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_ratings, clear_ratings, pull_ratings, apply_ratings))]
pub struct Api;

pub fn create_ratings_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/ratings", get(get_ratings).delete(clear_ratings))
        .route("/api/v1/ratings/pull", post(pull_ratings))
        .route("/api/v1/ratings/apply", post(apply_ratings))
}
