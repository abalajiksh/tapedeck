mod admin;
mod artwork;
pub mod auth;
mod chains;
mod connections;
mod export;
pub mod import;
pub mod ingest;
mod lb_api;
mod library;
mod loves_pull;
mod metrics;
pub mod models;
mod notes;
pub mod now_playing;
mod preferences;
pub mod ratelimit;
pub mod scrobbles;
mod sessions;
mod sources;
mod users;

use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};

use crate::db::Database;
use crate::logging::LogLevelHandle;
use crate::musicbrainz::MusicBrainzClient;
use crate::web_assets;

/// Shared application state available to all request handlers.
pub struct AppState {
    pub db: Arc<Database>,
    pub mb_client: Arc<MusicBrainzClient>,
    pub log_handle: LogLevelHandle,
    /// ListenBrainz endpoint (self-hosted instances override the default).
    pub listenbrainz_url: String,
    /// In-memory registry of running/finished import jobs.
    pub imports: import::ImportRegistry,
    /// Live "now playing" per user. Shared with the poll engine, which writes
    /// it from Plex sessions; `ingest` writes it from `playing_now` submissions.
    pub now_playing: now_playing::NowPlayingRegistry,
    /// Brute-force throttle for the login endpoint, keyed by client IP.
    pub login_limiter: Arc<ratelimit::RateLimiter>,
    /// Max rows an in-memory analytics pass will load (scaled to host RAM).
    pub analytics_row_cap: i64,
    /// Only trust `X-Forwarded-For` for the client IP when behind a known proxy.
    pub trust_proxy: bool,
    /// When this process started serving, for the uptime metric.
    pub started_at: std::time::Instant,
    /// Description providers, in preference order. Built once — each holds an
    /// HTTP client with its own connection pool, so rebuilding per request
    /// would throw those away.
    pub bio_providers: Vec<crate::metadata::bio::BioProviderBox>,
}

/// Build the full Axum application.
pub fn build_app(state: Arc<AppState>) -> Router {
    let admin = admin::create_admin_router(state.db.clone());
    let auth_router = users::create_auth_router();
    let user_mgmt = users::create_user_management_router();
    let gear = chains::create_gear_router();
    let scrobbles_router = scrobbles::create_scrobbles_router();

    let api = Router::new()
        .route("/1/submit-listens", post(ingest::submit_listens))
        .route("/1/validate-token", get(ingest::validate_token))
        .merge(lb_api::create_lb_api_router())
        .merge(auth_router)
        .merge(user_mgmt)
        .merge(gear)
        .merge(scrobbles_router)
        .merge(preferences::create_preferences_router())
        .merge(now_playing::create_now_playing_router())
        .merge(artwork::create_artwork_router())
        .merge(sources::create_sources_router())
        .merge(export::create_export_router())
        .merge(import::create_import_router())
        .merge(connections::create_connections_router())
        .merge(notes::create_notes_router())
        .merge(library::create_library_router())
        .merge(loves_pull::create_loves_pull_router())
        .merge(metrics::create_metrics_router())
        .merge(sessions::create_sessions_router())
        .with_state(state);

    // The SPA is the fallback, not a `/*path` route: a route would only answer
    // the methods it registers (so an unmatched DELETE would 405 instead of
    // 404) and would let unknown API paths render the HTML shell.
    Router::new()
        .merge(admin)
        .merge(api)
        .fallback(web_assets::spa_fallback)
        // Allow large uploads (full history exports for import) — axum defaults
        // to 2MB, which rejects real Last.fm/ListenBrainz dumps.
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
}
