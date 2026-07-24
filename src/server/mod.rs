mod admin;
pub mod auth;
mod chains;
mod export;
pub mod import;
pub mod ingest;
pub mod models;
mod preferences;
mod scrobbles;
mod users;

use std::sync::Arc;

use axum::{routing::post, Router};

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
    /// Last.fm API key (for user.getRecentTracks import); empty = import disabled.
    pub lastfm_api_key: String,
}

/// Build the full Axum application.
pub fn build_app(state: Arc<AppState>) -> Router {
    let admin = admin::create_admin_router(state.log_handle.clone());
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
        .with_state(state);

    // The SPA is the fallback, not a `/*path` route: a route would only answer
    // the methods it registers (so an unmatched DELETE would 405 instead of
    // 404) and would let unknown API paths render the HTML shell.
    Router::new()
        .merge(admin)
        .merge(api)
        .fallback(web_assets::spa_fallback)
}
