use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use std::sync::Arc;

use super::auth::{AuthUser, IngestAuth};
use super::models::ErrorResponse;
use super::AppState;
use crate::models::ScrobbleThreshold;

/// GET /api/v1/preferences → the session user's stored settings object.
async fn get_preferences(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.get_preferences(user.user_id).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

/// PUT /api/v1/preferences — shallow-merges the body into stored settings
/// (e.g. `{ "theme": "dark" }`). Returns the merged object.
async fn put_preferences(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !body.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Body must be a JSON object".into()
            })),
        )
            .into_response();
    }
    match state.db.merge_preferences(user.user_id, &body).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/v1/scrobble-settings — when a listen counts, for the caller.
///
/// Exists so a scrobble client can *follow the same rule as the server*
/// instead of hardcoding Last.fm's convention. Half the track or four minutes
/// is only the default here; it is a per-user setting, because that convention
/// fits a three-minute pop song far better than a forty-minute raga or a
/// ninety-second hardcore track. A client that hardcodes it disagrees with the
/// listener's own choice, silently, and nothing anywhere errors.
///
/// It is not cosmetic agreement. The same threshold decides whether an ingest
/// track that was announced as now-playing and never scrobbled is written off
/// as a **skip** (`ScrobbleEngine::resolve_pending_skips`), so a client
/// submitting later than this rule banks the listen *and* leaves a skip behind
/// it; one submitting earlier puts a listen on a permanent public record that
/// the listener's own setting says had not been heard.
///
/// **Reported as the server computes it, never as stored.** `from_prefs`
/// clamps, so a preferences object holding a fraction of zero is 5% here and
/// 5% in every source — reading the raw settings blob (which
/// `GET /api/v1/preferences` serves, session-only) would give a client a number
/// the server does not use.
///
/// **Its own endpoint rather than more keys on `/1/validate-token`**, which is
/// the opposite call from the introspection there and for a specific reason:
/// those keys describe the *token*, are asked once at setup, and a client that
/// caches them stays correct. This is a *setting*, changed from a settings
/// screen at any time — cached from a setup handshake it would go quietly
/// stale, which is the failure this endpoint exists to prevent.
///
/// **`IngestAuth`, so a plain `submit` token can ask.** Putting it behind
/// `read` would make every scrobble client on the instance carry read access to
/// the whole listening history in order to learn one number it is obliged to
/// obey — the grant should match what the read is *for*, and this read exists
/// only to decide a submission.
async fn get_scrobble_settings(
    State(state): State<Arc<AppState>>,
    auth: IngestAuth,
) -> impl IntoResponse {
    let prefs = match state.db.get_preferences(auth.user.user_id).await {
        Ok(prefs) => prefs,
        // Never report a failed read as "they have not set one": the default is
        // an answer, and answering it here would have a client follow a rule
        // that is not the one the server is applying.
        Err(e) => {
            tracing::error!("Scrobble settings read failed: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    code: 500,
                    error: "Could not read your scrobble settings".into()
                })),
            )
                .into_response();
        }
    };

    let t = ScrobbleThreshold::from_prefs(&prefs);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "fraction": t.fraction,
            "percent": t.percent(),
            "after_secs": t.after_secs,
            // Whichever comes first — stated rather than implied, because a
            // client that reads these as an AND scrobbles a long track hours
            // late and a short one never.
            "rule": "either",
            // With no duration the fraction is unanswerable, so `after_secs` is
            // the only rule left. Every source here already behaves this way;
            // saying so stops a client inventing its own answer.
            "no_duration_after_secs": t.after_secs,
            "source": if ScrobbleThreshold::is_configured(&prefs) { "user" } else { "default" },
            "default_percent": ScrobbleThreshold::DEFAULT_FRACTION * 100.0,
            "default_after_secs": ScrobbleThreshold::DEFAULT_AFTER_SECS,
        })),
    )
        .into_response()
}

pub fn create_preferences_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/preferences", get(get_preferences).put(put_preferences))
        .route("/api/v1/scrobble-settings", get(get_scrobble_settings))
}
