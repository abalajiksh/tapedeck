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
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{IncomingRating, UserSource};
use crate::ratings::{display_scale_from_prefs, DisplayScale, LoveRule};
use crate::sources::{MediaBrowserSource, NavidromeSource, PlexSource};

fn err(code: StatusCode, msg: impl Into<String>) -> axum::response::Response {
    (code, Json(serde_json::json!(ErrorResponse { code: code.as_u16(), error: msg.into() })))
        .into_response()
}

/// What one source's pull did.
///
/// `error` and a zero count are different answers and both are reported: a
/// server that could not be reached is not a library with no ratings in it, and
/// a screen that showed them the same way would send somebody looking for
/// ratings that are sitting right there.
#[derive(Debug, serde::Serialize)]
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
    match source.kind.as_str() {
        "plex" => {
            let token = source.credential("token").ok_or("Plex token is missing")?;
            let mut plex = PlexSource::new(source.url.clone(), token);
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
                MediaBrowserSource::emby(source.url.clone(), key, account)
            } else {
                MediaBrowserSource::jellyfin(source.url.clone(), key, account)
            };
            client.fetch_ratings().await.map_err(|e| crate::logging::error_chain(&*e))
        }
        "navidrome" => {
            let (Some(u), Some(p)) = (source.credential("username"), source.credential("password"))
            else {
                return Err("Navidrome credentials are missing".into());
            };
            let mut nd = NavidromeSource::new(source.url.clone(), u, p);
            nd.fetch_ratings().await.map_err(|e| crate::logging::error_chain(&*e))
        }
        "roon" => Err("Roon publishes no ratings — its API has no rating, \
                       favourite or thumb field at all"
            .into()),
        other => Err(format!("A {other} source has no ratings to read")),
    }
}

/// POST /api/v1/ratings/pull — read every configured library server.
///
/// Runs inline rather than as a background job, like `loves_pull`: a library's
/// ratings are thousands of rows at most, not the hundreds of thousands a
/// listen import walks, so there is nothing here worth the progress-tracking
/// machinery.
///
/// **One source failing does not stop the others**, and a source that answered
/// has what it said replaced wholesale — a rating cleared on the server simply
/// stops appearing in its answer, so the only way to notice is to compare, and
/// the cheapest correct comparison is to forget that source's rows first. The
/// forget and the write are *not* one transaction, and that is a deliberate
/// trade: the worst case is a pull that dies halfway leaving a source's ratings
/// partly read, which the next pull fixes, against holding a write lock for the
/// length of a network walk.
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
    (StatusCode::OK, Json(serde_json::json!({ "found": found, "sources": reports })))
        .into_response()
}

/// GET /api/v1/ratings — what is held, and the rule that would be applied.
///
/// Reports the display scale and the threshold **in that scale's own units**,
/// because that is what was typed and what a settings screen has to show back.
/// The fraction each rating actually sits at travels beside its raw number, so
/// a screen can draw the stars without re-deriving the conversion and drifting
/// from the server's.
async fn get_ratings(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let prefs = state.db.get_preferences(user.user_id).await.unwrap_or_default();
    let scale = display_scale_from_prefs(&prefs);
    let rule = LoveRule::from_prefs(&prefs);

    let sources = match state.db.rating_sources(user.user_id).await {
        Ok(s) => s,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let rows = state.db.list_library_ratings(user.user_id, 60).await.unwrap_or_default();
    let sample: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "kind": r.kind, "artist": r.artist, "name": r.name, "album": r.album,
                "source_kind": r.source_kind, "starred": r.starred,
                // The original, kept and shown: "7 out of 10, from Plex".
                "rating": r.rating, "scale_max": r.scale_max,
                // And the same thing on the scale this person reads in.
                "shown": scale.show(r.fraction()),
                "loved": rule.loves(
                    r.rating.filter(|v| *v > 0.0).map(|v| crate::ratings::Rating::new(
                        v as f32, crate::ratings::SourceScale::new(r.scale_max as f32))),
                    r.starred,
                ),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "scale": scale.id(),
            "scale_label": scale.label(),
            "scale_choices": scale.choices(),
            "threshold": scale.show(rule.at_least),
            "include_starred": rule.include_starred,
            "sources": sources,
            "sample": sample,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
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

/// POST /api/v1/ratings/apply — love what clears the bar, withdraw what no
/// longer does.
///
/// **`dry_run` is required and has no default.** The two things this endpoint
/// can do are "tell me what would happen" and "write to my Loved page", and
/// guessing from an absent field is the one thing it must not do — the
/// `bulk-chain` lesson, in a place where the wrong guess is a few thousand
/// rows. A body that merely misspelled the key is a 400.
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
                Json(serde_json::json!({
                    "threshold": scale.show(rule.at_least),
                    "scale": scale.id(),
                    "include_starred": rule.include_starred,
                    "report": report,
                })),
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
async fn clear_ratings(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.clear_library_ratings(user.user_id).await {
        Ok(n) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "forgotten": n,
                "note": "Loves already made are left alone — raise the threshold and apply to \
                         take those back.",
            })),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub fn create_ratings_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/ratings", get(get_ratings).delete(clear_ratings))
        .route("/api/v1/ratings/pull", post(pull_ratings))
        .route("/api/v1/ratings/apply", post(apply_ratings))
}
