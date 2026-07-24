mod admin;
pub mod auth;
mod chains;
mod connections;
mod export;
pub mod import;
pub mod ingest;
pub mod models;
mod preferences;
pub mod ratelimit;
mod scrobbles;
mod users;

use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, routing::post, Router};

use crate::db::Database;
use crate::logging::LogLevelHandle;
use crate::musicbrainz::MusicBrainzClient;
use crate::sinks::ScrobbleSink;
use crate::web_assets;

/// Shared application state available to all request handlers.
pub struct AppState {
    pub db: Arc<Database>,
    pub mb_client: Arc<MusicBrainzClient>,
    pub sinks: Arc<Vec<Box<dyn ScrobbleSink>>>,
    pub log_handle: LogLevelHandle,
    /// In-memory registry of running/finished import jobs.
    pub imports: import::ImportRegistry,
    /// Last.fm app credentials (for import + per-user OAuth connect); empty = disabled.
    pub lastfm_api_key: String,
    pub lastfm_secret: String,
    /// ListenBrainz base URL for validating per-user tokens.
    pub listenbrainz_base_url: String,
    /// Brute-force throttle for the login endpoint, keyed by client IP.
    pub login_limiter: Arc<ratelimit::RateLimiter>,
    /// Max rows an in-memory analytics pass will load (scaled to host RAM).
    pub analytics_row_cap: i64,
    /// Only trust `X-Forwarded-For` for the client IP when behind a known proxy.
    pub trust_proxy: bool,
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
        .merge(auth_router)
        .merge(user_mgmt)
        .merge(gear)
        .merge(scrobbles_router)
        .merge(preferences::create_preferences_router())
        .merge(export::create_export_router())
        .merge(import::create_import_router())
        .merge(connections::create_connections_router())
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
