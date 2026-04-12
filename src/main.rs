mod config;
mod db;
mod engine;
mod error;
mod logging;
mod models;
mod musicbrainz;
mod server;
mod sinks;
mod sources;

use std::sync::Arc;

use sqlx::SqlitePool;
use tracing::{error, info};

use crate::config::Config;
use crate::db::Database;
use crate::engine::ScrobbleEngine;
use crate::musicbrainz::MusicBrainzClient;
use crate::sinks::ScrobbleSink;
use crate::sources::{MusicSource, PlexSource, PlexFilters};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    // ── Logging ──
    let enable_file_logging = std::env::var("ENABLE_FILE_LOGGING")
        .unwrap_or_else(|_| "true".into())
        .parse::<bool>()
        .unwrap_or(true);
    let enable_console = std::env::var("ENABLE_CONSOLE_LOGGING")
        .unwrap_or_else(|_| "true".into())
        .parse::<bool>()
        .unwrap_or(true);
    let log_dir = std::env::var("LOG_DIR").ok();

    let log_handle = logging::init_logging(log_dir.as_deref(), enable_file_logging, enable_console)?;
    info!("🚀 Tapedeck Scrobbler Service Started");

    // ── Configuration ──
    let config = Config::from_env();

    // ── MusicBrainz client ──
    let sqlite_path = config.database.sqlite_path.clone();
    info!("📦 Initializing SQLite database at {}", sqlite_path);
    let sqlite_pool = SqlitePool::connect(&format!("sqlite:{}?mode=rwc", sqlite_path)).await?;

    let mb_config = musicbrainz::MusicBrainzConfig {
        api_base_url: "https://musicbrainz.org/ws/2".to_string(),
        user_agent: config.musicbrainz.user_agent.clone(),
        rate_limit_per_second: config.musicbrainz.rate_limit_per_second,
        postgres_url: config.musicbrainz.postgres_url.clone(),
        enable_postgres: config.musicbrainz.enable_postgres,
    };
    let mb_client = MusicBrainzClient::new(mb_config, sqlite_pool.clone()).await?;
    mb_client.initialize_schema().await?;
    info!("✅ MusicBrainz client initialized");

    let mb_client = Arc::new(mb_client);

    // ── Scrobble database ──
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:scrobbles.db?mode=rwc".into());
    info!("📦 Initializing scrobble database at {}", db_url);
    let db = Arc::new(Database::new(&db_url).await?);

    // ── First-run setup ──
    if !db.has_users().await? {
        info!("🔑 First run detected — creating admin user and token...");
        let user_id = db
            .create_user("admin", Some("Admin"), "not-used-yet")
            .await?;
        let token = db
            .create_token(user_id, "default", "submit")
            .await?;
        info!("════════════════════════════════════════════════════════");
        info!("🔑 Admin API token (save this — it won't be shown again!):");
        info!("   {}", token);
        info!("════════════════════════════════════════════════════════");
    }

    // ── Sources ──
    let mut sources: Vec<Box<dyn MusicSource>> = Vec::new();

    if config.plex.enabled {
        info!("Initializing Plex source...");
        let filters = PlexFilters {
            users_allow: config.plex.users_allow.clone(),
            users_block: config.plex.users_block.clone(),
            devices_allow: config.plex.devices_allow.clone(),
            devices_block: config.plex.devices_block.clone(),
            libraries_allow: config.plex.libraries_allow.clone(),
            libraries_block: config.plex.libraries_block.clone(),
        };
        let mut plex = PlexSource::with_filters(
            config.plex.url.clone(),
            config.plex.token.clone(),
            filters,
        );
        match plex.initialize().await {
            Ok(_) => {
                info!("✅ Plex source initialized");
                sources.push(Box::new(plex));
            }
            Err(e) => error!("❌ Failed to initialize Plex: {}", e),
        }
    }

    // Sources are optional now — ingest API can work without any polling sources
    if sources.is_empty() {
        info!("ℹ️ No polling sources enabled — running in ingest-only mode");
    }

    // ── Sinks ──
    let mut sink_vec: Vec<Box<dyn ScrobbleSink>> = Vec::new();

    if config.listenbrainz.enabled {
        info!("Initializing ListenBrainz sink...");
        sink_vec.push(Box::new(sinks::ListenBrainzSink::new(
            config.listenbrainz.base_url.clone(),
            config.listenbrainz.token.clone(),
        )));
    }
    if config.lastfm.enabled {
        info!("Initializing Last.fm sink...");
        sink_vec.push(Box::new(sinks::LastFmSink::new(
            config.lastfm.api_key.clone(),
            config.lastfm.secret.clone(),
            config.lastfm.session_key.clone(),
        )));
    }

    if sink_vec.is_empty() {
        info!("ℹ️ No scrobble sinks enabled — listens will be stored locally only");
    }

    let sinks: Arc<Vec<Box<dyn ScrobbleSink>>> = Arc::new(sink_vec);

    // ── HTTP server ──
    let server_port: u16 = std::env::var("PORT")
        .or_else(|_| std::env::var("ADMIN_PORT"))
        .unwrap_or_else(|_| "8080".into())
        .parse()
        .unwrap_or(8080);

    let app_state = Arc::new(server::AppState {
        db: db.clone(),
        mb_client: mb_client.clone(),
        sinks: sinks.clone(),
        log_handle: log_handle.clone(),
    });

    let app = server::build_app(app_state);

    tokio::spawn(async move {
        let addr = format!("0.0.0.0:{}", server_port);
        info!("🌐 Starting server on {}", addr);
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind server: {}", e);
                return;
            }
        };
        if let Err(e) = axum::serve(listener, app).await {
            error!("Server error: {}", e);
        }
    });

    // ── Scrobble engine ──
    let mut engine = ScrobbleEngine::new(sources, sinks, db, mb_client);
    engine.run().await;
}
