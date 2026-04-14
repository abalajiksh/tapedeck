mod admin;
pub mod auth;
mod chains;
pub mod ingest;
pub mod models;
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
        .with_state(state);

    Router::new()
        .merge(admin)
        .merge(api)
        .merge(web_assets::create_frontend_router())
}
