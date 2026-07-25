mod analytics;
mod config;
mod db;
mod engine;
mod enrich;
mod language;
mod logging;
mod models;
mod musicbrainz;
mod server;
mod sinks;
mod sources;
mod web_assets;

use std::sync::Arc;

use sqlx::SqlitePool;
use tracing::{error, info};

use crate::config::Config;
use crate::db::{Database, PlexSettings};
use crate::engine::ScrobbleEngine;
use crate::musicbrainz::MusicBrainzClient;
use crate::sources::MusicSource;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();

    // ── Configuration (TOML + env overrides) ──
    let config = Config::load();

    // ── Logging ──
    let log_handle = logging::init_logging(
        Some(&config.logging.dir),
        config.logging.file_enabled,
        config.logging.console_enabled,
        &config.logging.level,
    )?;
    info!("🚀 Tapedeck v{} Started", env!("CARGO_PKG_VERSION"));

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
    info!("📦 Initializing scrobble database at {}", config.database.scrobble_db_url);
    let db = Arc::new(Database::new(&config.database.scrobble_db_url).await?);

    // Any import left 'running' by a previous process died with it — record that.
    match db.mark_running_imports_interrupted().await {
        Ok(n) if n > 0 => info!("⚠️ Marked {} interrupted import(s) from a previous run", n),
        _ => {}
    }

    // ── First-run setup ──
    if !db.has_users().await? {
        info!("🔑 First run — creating admin user (password not set yet)");
        let _ = db.create_user("admin", Some("Admin"), "not-used-yet", "admin").await?;
        info!("════════════════════════════════════════════════════════");
        info!("📋 Visit the web UI to complete setup and set your password.");
        info!("════════════════════════════════════════════════════════");
    } else if db.needs_setup().await? {
        info!("📋 Admin password not set — visit the web UI to complete setup.");
    }

    // ── Sources ──
    // Plex config lives in the database so it can be edited from the UI and
    // applied live. `tapedeck.toml` / PLEX_* env vars only *seed* the row on
    // first run — after that the database is authoritative and editing the
    // config file has no effect.
    let plex_settings = match db.get_plex_settings().await? {
        Some(s) => s,
        None => {
            let seeded = PlexSettings {
                enabled: config.plex.enabled,
                url: config.plex.url.clone(),
                token: config.plex.token.clone(),
                users_allow: config.plex.users_allow.join(","),
                users_block: config.plex.users_block.join(","),
                devices_allow: config.plex.devices_allow.join(","),
                devices_block: config.plex.devices_block.join(","),
                libraries_allow: config.plex.libraries_allow.join(","),
                libraries_block: config.plex.libraries_block.join(","),
                updated_at: 0,
            };
            if seeded.enabled {
                info!("⚙️ Seeding Plex settings from config (the UI owns them from now on)");
            }
            db.save_plex_settings(&seeded).await?;
            db.get_plex_settings().await?.unwrap_or(seeded)
        }
    };

    // The engine builds the Plex source itself on its first tick, from these
    // same settings — one code path for startup and for later UI edits.
    let sources: Vec<Box<dyn MusicSource>> = Vec::new();

    if plex_settings.is_usable() {
        info!("🎧 Plex source enabled ({})", plex_settings.url);
    } else {
        info!("ℹ️ No polling sources enabled — running in ingest-only mode");
    }

    // ── Sinks ──
    // There are no server-wide sinks: every listen forwards only to the
    // accounts its own user connected (Settings → Scrobble Connections), built
    // per user at forward time by `sinks::sinks_for_user`.

    // Shared between the HTTP layer (reads it for the deck, writes it from
    // `playing_now` ingests) and the poll engine (writes it from Plex sessions).
    let now_playing = server::now_playing::NowPlayingRegistry::default();

    // ── HTTP server ──
    let app_state = Arc::new(server::AppState {
        db: db.clone(),
        mb_client: mb_client.clone(),
        log_handle: log_handle.clone(),
        listenbrainz_url: config.listenbrainz_url.clone(),
        imports: Default::default(),
        now_playing: now_playing.clone(),
        login_limiter: Arc::new(server::ratelimit::RateLimiter::new(
            8, std::time::Duration::from_secs(300))),
        analytics_row_cap: analytics_row_cap(),
        trust_proxy: config.server.trust_proxy,
    });

    let app = server::build_app(app_state);
    let server_port = config.server.port;
    let server_host = config.server.host.clone();

    // Shutdown coordination: the HTTP server drains first, then flips this so
    // the engine finishes its current tick and returns cleanly.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // ── HTTP server (background task; drains in-flight requests on shutdown) ──
    let server_task = tokio::spawn(async move {
        let addr = format!("{}:{}", server_host, server_port);
        info!("🌐 Starting server on {}", addr);
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => { error!("Failed to bind server: {}", e); let _ = shutdown_tx.send(true); return; }
        };
        if let Err(e) = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
            .with_graceful_shutdown(shutdown_signal())
            .await
        {
            error!("Server error: {}", e);
        }
        // Server has drained all in-flight requests — now tell the engine to stop.
        let _ = shutdown_tx.send(true);
    });

    // ── Scrobble engine (foreground) ── runs until the server drains and signals
    // shutdown, at which point it finishes the current tick and returns. Between
    // that and the server's request draining, no in-progress work is cut off:
    // committed writes are durable (WAL) and pending scrobbles resume on restart.
    let mut engine = ScrobbleEngine::new(sources, db, mb_client, now_playing, config.listenbrainz_url.clone());
    engine.run(shutdown_rx).await;
    let _ = server_task.await;
    info!("👋 Shut down cleanly");
    Ok(())
}

/// Cap on rows an in-memory analytics/personality pass will load, scaled to the
/// machine's RAM so a large history can't OOM a request on a small box (e.g. a
/// Pi Zero). Rows are ~200 bytes; we budget ~1/16 of total RAM per pass.
fn analytics_row_cap() -> i64 {
    let total_mb = std::fs::read_to_string("/proc/meminfo").ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<i64>().ok()))
        .map(|kb| kb / 1024)
        .unwrap_or(2048); // assume 2 GB if unreadable
    (total_mb / 16 * 5_000).clamp(100_000, 5_000_000)
}

/// Resolves on Ctrl-C or (Unix) SIGTERM — the trigger for the server's graceful
/// drain, which then stops the engine.
async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async { let _ = signal::ctrl_c().await; };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("🛑 Shutdown signal received, draining…");
}
