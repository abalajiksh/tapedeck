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
use crate::openapi::{BadRequest, Unauthorized};
use serde::Serialize;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use utoipa::openapi::RefOr;
use utoipa::ToSchema;

/// Your stored settings: a free-form object the web UI keeps its choices in.
///
/// Genuinely schemaless storage, so this is described by hand rather than
/// derived — these are the keys the **server** reads; anything else is the
/// UI's own and is kept verbatim.
pub struct Preferences;

impl utoipa::PartialSchema for Preferences {
    fn schema() -> RefOr<Schema> {
        let prop = |t: Type, d: &str| ObjectBuilder::new().schema_type(t).description(Some(d));
        ObjectBuilder::new()
            .schema_type(Type::Object)
            .description(Some(
                "Free-form. The keys below are the ones the server reads; any other key \
                 is the web UI's own and is stored verbatim.",
            ))
            .property(
                "timezone",
                prop(
                    Type::String,
                    "An IANA region, e.g. `Europe/Berlin` — never a fixed abbreviation like `CET`. \
                     Decides \"today\", the heatmap and every calendar bucket. UTC when unset.",
                ),
            )
            .property(
                "week_start",
                prop(Type::Integer, "First day of the week, 0 = Sunday … 6 = Saturday. Monday (1) when unset."),
            )
            .property(
                "scrobble_threshold_percent",
                prop(Type::Number, "When a listen counts: this share of the track, 5–100. Read the clamped value from `GET /api/v1/scrobble-settings`."),
            )
            .property(
                "scrobble_threshold_secs",
                prop(Type::Integer, "…or this many seconds, whichever comes first, 10–3600."),
            )
            .property(
                "rating_scale",
                prop(Type::String, "The scale `rating_love_threshold` is written on. Set by `POST /api/v1/ratings/apply`."),
            )
            .property(
                "rating_love_threshold",
                prop(Type::Number, "The media-server rating at or above which a track counts as loved, on `rating_scale`."),
            )
            .property(
                "rating_love_starred",
                prop(Type::Boolean, "Whether a starred (favourited) item counts as loved whatever its rating. True when unset."),
            )
            .property(
                "theme",
                prop(Type::String, "`light`, `dark` or `system`. The web UI's."),
            )
            .additional_properties(Some(utoipa::openapi::schema::AdditionalProperties::FreeForm(true)))
            .into()
    }
}

impl ToSchema for Preferences {}

/// When a listen counts, as the server applies it.
#[derive(Serialize, ToSchema)]
pub struct ScrobbleSettings {
    /// Share of the track that must have played, 0.05–1.0. This is the number
    /// the server multiplies the duration by.
    pub fraction: f64,
    /// `fraction` as a percentage, which is how the setting is worded.
    pub percent: f64,
    /// Seconds after which it counts regardless of the fraction.
    pub after_secs: u64,
    /// Always `either` — **whichever comes first.** Stated rather than implied:
    /// a client reading the two as an AND scrobbles a long track hours late
    /// and a short one never.
    #[schema(example = "either")]
    pub rule: &'static str,
    /// What applies when the track's length is unknown — the fraction is
    /// unanswerable, so the flat number is the only rule left. Equal to
    /// `after_secs`; named separately so a client does not invent its own
    /// answer for that case.
    pub no_duration_after_secs: u64,
    /// `user` or `default`: whether the listener chose these numbers or
    /// inherited them, so a settings screen can say which.
    pub source: &'static str,
    /// The instance default, for showing what "standard" is.
    pub default_percent: f64,
    /// The instance default, for showing what "standard" is.
    pub default_after_secs: u64,
}

// GET /api/v1/preferences → the session user's stored settings object.
/// User preferences.
///
/// Session-only.
#[utoipa::path(
    get,
    path = "/api/v1/preferences",
    tag = "Settings",
    responses(
        (status = 200, description = "OK.", body = Preferences),
        (status = 401, response = Unauthorized),
    )
)]
async fn get_preferences(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.get_preferences(user.user_id).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// PUT /api/v1/preferences — shallow-merges the body into stored settings
// (e.g. `{ "theme": "dark" }`). Returns the merged object.
/// Update preferences.
///
/// Session-only. A **shallow merge**: the keys sent replace the stored ones,
/// omitted keys are left alone, and the merged object comes back.
///
/// `timezone` must be an **IANA region**, not a fixed abbreviation. The
/// region carries the DST rules — `Europe/Berlin` *is* CET in winter and
/// CEST in summer, switching on the right dates by itself, and a region
/// with no summer time never shifts. Storing `CET` would freeze the offset
/// and put half a European year an hour out, silently: listens would still
/// store correctly and only "today", the hour-of-day heatmap and the
/// trajectory buckets would be wrong.
#[utoipa::path(
    put,
    path = "/api/v1/preferences",
    tag = "Settings",
    request_body = Preferences,
    responses(
        (status = 200, description = "Saved. The whole merged object.", body = Preferences),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn put_preferences(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !body.is_object() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse { code: 400, error: "Body must be a JSON object".into() }),
        )
            .into_response();
    }
    match state.db.merge_preferences(user.user_id, &body).await {
        Ok(prefs) => (StatusCode::OK, Json(prefs)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { code: 500, error: e.to_string() }),
        )
            .into_response(),
    }
}

// GET /api/v1/scrobble-settings — when a listen counts, for the caller.
//
// Exists so a scrobble client can *follow the same rule as the server*
// instead of hardcoding Last.fm's convention. Half the track or four minutes
// is only the default here; it is a per-user setting, because that convention
// fits a three-minute pop song far better than a forty-minute raga or a
// ninety-second hardcore track. A client that hardcodes it disagrees with the
// listener's own choice, silently, and nothing anywhere errors.
//
// It is not cosmetic agreement. The same threshold decides whether an ingest
// track that was announced as now-playing and never scrobbled is written off
// as a **skip** (`ScrobbleEngine::resolve_pending_skips`), so a client
// submitting later than this rule banks the listen *and* leaves a skip behind
// it; one submitting earlier puts a listen on a permanent public record that
// the listener's own setting says had not been heard.
//
// **Reported as the server computes it, never as stored.** `from_prefs`
// clamps, so a preferences object holding a fraction of zero is 5% here and
// 5% in every source — reading the raw settings blob (which
// `GET /api/v1/preferences` serves, session-only) would give a client a number
// the server does not use.
//
// **Its own endpoint rather than more keys on `/1/validate-token`**, which is
// the opposite call from the introspection there and for a specific reason:
// those keys describe the *token*, are asked once at setup, and a client that
// caches them stays correct. This is a *setting*, changed from a settings
// screen at any time — cached from a setup handshake it would go quietly
// stale, which is the failure this endpoint exists to prevent.
//
// **`IngestAuth`, so a plain `submit` token can ask.** Putting it behind
// `read` would make every scrobble client on the instance carry read access to
// the whole listening history in order to learn one number it is obliged to
// obey — the grant should match what the read is *for*, and this read exists
// only to decide a submission.
/// When a listen counts — follow the same rule the server does.
///
/// Half the track **or** four minutes, whichever comes first, is only the
/// *default*: it is a per-user setting, because that convention fits a
/// three-minute pop song far better than a forty-minute raga or a
/// ninety-second hardcore track. A client that hardcodes it disagrees with
/// the listener's own choice, silently, and nothing anywhere errors.
///
/// Following it is not cosmetic agreement. The same threshold decides
/// whether a track this client announced as now-playing and never
/// scrobbled is written off as a **skip** — so submitting later than this
/// rule banks the listen *and* leaves a skip behind it, and submitting
/// earlier puts a listen on a permanent public record that the listener's
/// own setting says had not been heard.
///
/// Reported **as the server computes it, never as stored**: the values are
/// clamped (fraction 0.05–1.0, seconds 10–3600), so a preferences object
/// holding nonsense reads as the number every source here actually uses.
/// `GET /api/v1/preferences` serves the raw blob and is session-only.
///
/// Accepts a plain **`submit`** token, not `read`: this read exists only to
/// decide a submission, and putting it behind `read` would make every
/// scrobble client carry read access to the whole listening history to
/// learn one number it is obliged to obey.
///
/// It is its own endpoint rather than more keys on `/1/validate-token` —
/// the opposite call from the introspection there, and deliberately. Those
/// keys describe the *token*, are asked once at setup, and stay correct
/// when cached. This is a setting, changed from a settings screen at any
/// time; cached from a setup handshake it would go quietly stale, which is
/// the failure this endpoint exists to prevent. Re-read it when a session
/// starts.
#[utoipa::path(
    get,
    path = "/api/v1/scrobble-settings",
    tag = "Settings",
    security(("apiToken" = []), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = ScrobbleSettings),
        (status = 401, response = Unauthorized),
        (status = 500, description = "The settings could not be read. **Never answered as \"they have not set one\"** — the default is an answer, and giving it here would have a client follow a rule the server is not applying.", body = ErrorResponse),
    )
)]
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
                Json(ErrorResponse {
                    code: 500,
                    error: "Could not read your scrobble settings".into(),
                }),
            )
                .into_response();
        }
    };

    let t = ScrobbleThreshold::from_prefs(&prefs);
    (
        StatusCode::OK,
        Json(ScrobbleSettings {
            fraction: t.fraction,
            percent: t.percent(),
            after_secs: t.after_secs,
            // Whichever comes first — stated rather than implied, because a
            // client that reads these as an AND scrobbles a long track hours
            // late and a short one never.
            rule: "either",
            // With no duration the fraction is unanswerable, so `after_secs` is
            // the only rule left. Every source here already behaves this way;
            // saying so stops a client inventing its own answer.
            no_duration_after_secs: t.after_secs,
            source: if ScrobbleThreshold::is_configured(&prefs) { "user" } else { "default" },
            default_percent: ScrobbleThreshold::DEFAULT_FRACTION * 100.0,
            default_after_secs: ScrobbleThreshold::DEFAULT_AFTER_SECS,
        }),
    )
        .into_response()
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_preferences, put_preferences, get_scrobble_settings))]
pub struct Api;

pub fn create_preferences_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/preferences", get(get_preferences).put(put_preferences))
        .route("/api/v1/scrobble-settings", get(get_scrobble_settings))
}
