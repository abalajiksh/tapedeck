mod admin;
pub mod amai;
pub(crate) mod artwork;
pub mod auth;
pub mod chains;
mod connections;
pub mod crate_shelf;
pub mod everynoise;
mod export;
pub mod import;
pub mod ingest;
mod lb_api;
pub mod library;
mod loves_pull;
pub mod mcp;
mod metrics;
pub mod models;
pub mod notes;
pub mod now_playing;
pub mod oauth;
pub mod ops;
mod pairing;
pub mod patch;
pub mod physical;
mod playlist_lab;
pub mod playlists;
mod preferences;
pub mod public_np;
mod radio;
pub mod ratelimit;
mod ratings;
pub mod rediscovery;
pub mod report_image;
pub mod reports;
pub mod scrobbles;
pub mod sessions;
mod sources;
pub mod tidal;
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
    /// The LRCLIB cache, when it could be opened. `None` degrades to "no
    /// lyrics stored" rather than failing a request — the same normal state a
    /// missing Discogs token or Apple Music token is.
    pub lyrics: Option<Arc<crate::lyrics::LyricsClient>>,
    pub log_handle: LogLevelHandle,
    /// ListenBrainz endpoint (self-hosted instances override the default).
    pub listenbrainz_url: String,
    /// In-memory registry of running/finished import jobs.
    pub imports: import::ImportRegistry,
    /// In-memory registry of running/finished session rebuilds.
    ///
    /// Separate from `imports` rather than folded into it: an import job
    /// carries a source, a fetch count and an enrichment count, none of which
    /// mean anything here, and a shared struct would be mostly-null for both.
    pub rebuilds: sessions::RebuildRegistry,
    /// Live "now playing" per user. Shared with the poll engine, which writes
    /// it from Plex sessions; `ingest` writes it from `playing_now` submissions.
    pub now_playing: now_playing::NowPlayingRegistry,
    /// Brute-force throttle for the login endpoint, keyed by client IP.
    pub login_limiter: Arc<ratelimit::RateLimiter>,
    /// Max rows an in-memory analytics pass will load (scaled to host RAM).
    pub analytics_row_cap: i64,
    /// Only trust `X-Forwarded-For` for the client IP when behind a known proxy.
    pub trust_proxy: bool,
    /// What this instance is reached at from outside, when the operator has
    /// said. Empty means `server::mcp::public_origin` derives it. Published as
    /// the OAuth issuer, so a wrong value breaks the whole flow.
    pub public_url: String,
    /// When this process started serving, for the uptime metric.
    pub started_at: std::time::Instant,
    /// Description providers, in preference order. Built once — each holds an
    /// HTTP client with its own connection pool, so rebuilding per request
    /// would throw those away.
    pub bio_providers: Vec<crate::metadata::bio::BioProviderBox>,
    /// The Every Noise genre map, in a database of its own — bulk reference
    /// data acquired from elsewhere, not anything the user did.
    pub everynoise: crate::everynoise::store::EveryNoiseStore,
    /// Progress for the one genre-map scrape that may be running.
    pub everynoise_jobs: crate::everynoise::scrape::ScrapeRegistry,
    /// Where uploaded sleeve and booklet scans live on disk. Outside the
    /// database on purpose — see `DatabaseConfig::uploads_dir`.
    pub uploads_dir: std::path::PathBuf,
    /// Directory holding the rolling log files, or empty when file logging is
    /// off. Read by Settings → Operations; the alternative to knowing it here
    /// is telling an admin to go and find it over SSH, which is the situation
    /// that panel exists to end.
    pub log_dir: String,
    /// The station list and every running radio tuner. Tuners live here and
    /// nowhere else, like now-playing: a restart ends them.
    pub radio: crate::radio::Radio,
}

/// Every module's slice of the OpenAPI document, in the order the reference
/// reads best — which is roughly the order a client meets them. `openapi.rs`
/// merges these under the root document; a module missing from here is
/// undocumented, and the drift test says which routes that leaves.
pub fn api_docs() -> Vec<utoipa::openapi::OpenApi> {
    use utoipa::OpenApi as _;
    vec![
        admin::Api::openapi(),
        metrics::Api::openapi(),
        users::Api::openapi(),
        ingest::Api::openapi(),
        lb_api::Api::openapi(),
        pairing::Api::openapi(),
        scrobbles::Api::openapi(),
        sessions::Api::openapi(),
        now_playing::Api::openapi(),
        public_np::Api::openapi(),
        preferences::Api::openapi(),
        ratings::Api::openapi(),
        loves_pull::Api::openapi(),
        export::Api::openapi(),
        reports::Api::openapi(),
        report_image::Api::openapi(),
        rediscovery::Api::openapi(),
        playlist_lab::Api::openapi(),
        library::Api::openapi(),
        artwork::Api::openapi(),
        chains::Api::openapi(),
        notes::Api::openapi(),
        tidal::Api::openapi(),
        amai::Api::openapi(),
        crate_shelf::Api::openapi(),
        ops::Api::openapi(),
        sources::Api::openapi(),
        connections::Api::openapi(),
        import::Api::openapi(),
        playlists::Api::openapi(),
        everynoise::Api::openapi(),
        mcp::Api::openapi(),
        oauth::Api::openapi(),
        physical::Api::openapi(),
        radio::Api::openapi(),
        patch::Api::openapi(),
    ]
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
        .merge(public_np::create_public_router())
        .merge(artwork::create_artwork_router())
        .merge(sources::create_sources_router())
        .merge(export::create_export_router())
        .merge(import::create_import_router())
        .merge(connections::create_connections_router())
        .merge(notes::create_notes_router())
        .merge(library::create_library_router())
        .merge(loves_pull::create_loves_pull_router())
        .merge(ratings::create_ratings_router())
        .merge(metrics::create_metrics_router())
        .merge(ops::create_ops_router())
        .merge(sessions::create_sessions_router())
        .merge(physical::create_physical_router())
        .merge(radio::create_radio_router())
        .merge(rediscovery::create_rediscovery_router())
        .merge(playlists::create_playlists_router())
        .merge(playlist_lab::create_playlist_lab_router())
        .merge(everynoise::create_everynoise_router())
        .merge(pairing::create_pairing_router())
        .merge(patch::create_patch_router())
        .merge(reports::create_reports_router())
        .merge(report_image::create_report_image_router())
        .merge(crate_shelf::create_crate_router())
        .merge(mcp::create_mcp_router())
        .merge(oauth::create_oauth_router())
        .merge(tidal::create_tidal_router())
        .merge(amai::create_amai_router())
        // The spec describes *this* instance at *this* version — the one
        // thing a separately hosted docs site cannot do.
        .route("/api/openapi.yaml", axum::routing::get(crate::openapi::spec))
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
