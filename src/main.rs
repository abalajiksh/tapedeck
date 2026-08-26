mod analytics;
mod config;
mod crypto;
mod db;
mod engine;
mod enrich;
mod everynoise;
mod language;
mod logging;
mod lyrics;
mod mcp;
mod metadata;
mod models;
mod musicbrainz;
mod notes;
mod openapi;
mod patch;
mod playlist_lab;
mod playlists;
mod report_image;
mod rockbox;
mod server;
mod sinks;
mod sources;
mod spotify;
mod tidal;
mod web_assets;

use std::sync::Arc;

use sqlx::SqlitePool;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::db::{Database, PlexSettings};
use crate::engine::ScrobbleEngine;
use crate::musicbrainz::MusicBrainzClient;

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

    // ── Credential encryption ──
    // Deliberately a separate file from the database: `GET /api/v1/backup` hands
    // out a whole-DB snapshot, and a key stored inside it would ride along.
    let secrets = Arc::new(crypto::SecretBox::load(std::path::Path::new(&config.secret_key_path))?);

    // ── Scrobble database ──
    info!("📦 Initializing scrobble database at {}", config.database.scrobble_db_url);
    // Logged before it is propagated, because propagating it out of `main` only
    // reaches stderr — and under systemd the app's own log file is the thing
    // anyone actually reads. v0.61.0 crash-looped 297 times on a failed
    // migration and the log simply stopped mid-sequence with no error in it,
    // which is a far worse thing to be handed than the error itself.
    let db = Arc::new(
        Database::new(&config.database.scrobble_db_url, secrets.clone()).await.inspect_err(
            |e| {
                error!("❌ Could not open the scrobble database: {e}");
                error!("   Startup cannot continue. This is usually a migration that fails on an existing database.");
            },
        )?,
    );

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
    // Every source belongs to a user (`user_sources`), and the engine builds
    // them itself on its first tick — one code path for startup and for later
    // edits from the UI. `main` only migrates the old server-wide Plex row.
    migrate_global_plex(&db, &config).await?;

    // ── Sinks ──
    // There are no server-wide sinks: every listen forwards only to the
    // accounts its own user connected (Settings → Scrobble Connections), built
    // per user at forward time by `sinks::sinks_for_user`.

    // Shared between the HTTP layer (reads it for the deck, writes it from
    // `playing_now` ingests) and the poll engine (writes it from Plex sessions).
    let now_playing = server::now_playing::NowPlayingRegistry::default();

    // ── Genre map ──
    // Its own database: bulk reference data scraped from everynoise.com, not
    // anything the user did, and re-acquirable at any time. Opening it here
    // rather than lazily means a bad path fails at startup instead of the first
    // time someone opens Settings. Empty until an admin asks for a scrape.
    let everynoise =
        crate::everynoise::store::EveryNoiseStore::open(&config.database.everynoise_path).await?;

    // ── Lyrics ──
    // In the MusicBrainz cache database, not in `scrobbles.db`: third-party
    // copyrighted text, and `GET /api/v1/backup` is a `VACUUM INTO` of the
    // listening database — which should carry the user's history rather than a
    // cache of somebody else's words. That placement is how "never
    // redistributed" is enforced, instead of a rule each export has to remember.
    //
    // Failing to open it costs the lyric language tier and nothing else, so it
    // is a warning rather than a startup failure.
    let lyrics = match crate::lyrics::LyricsClient::new(sqlite_pool.clone()).await {
        Ok(l) => {
            info!("🎤 Lyrics cache ready (LRCLIB)");
            Some(Arc::new(l))
        }
        Err(e) => {
            warn!("🎤 Lyrics cache unavailable, language will fall back: {e}");
            None
        }
    };

    // ── HTTP server ──
    let app_state = Arc::new(server::AppState {
        db: db.clone(),
        mb_client: mb_client.clone(),
        lyrics: lyrics.clone(),
        log_handle: log_handle.clone(),
        listenbrainz_url: config.listenbrainz_url.clone(),
        imports: Default::default(),
        rebuilds: Default::default(),
        now_playing: now_playing.clone(),
        login_limiter: Arc::new(server::ratelimit::RateLimiter::new(
            8,
            std::time::Duration::from_secs(300),
        )),
        analytics_row_cap: analytics_row_cap(),
        trust_proxy: config.server.trust_proxy,
        public_url: config.server.public_url.clone(),
        started_at: std::time::Instant::now(),
        // Built once: each provider holds an HTTP client with its own
        // connection pool. Apple Music is unconfigured unless a developer token
        // is supplied, and simply steps aside when it is.
        bio_providers: metadata::bio::default_chain(
            std::env::var("TAPEDECK_AUDIODB_KEY").ok(),
            std::env::var("TAPEDECK_APPLE_MUSIC_TOKEN").ok(),
            std::env::var("TAPEDECK_APPLE_MUSIC_STOREFRONT").ok(),
        ),
        everynoise,
        everynoise_jobs: Default::default(),
        log_dir: if config.logging.file_enabled {
            config.logging.dir.clone()
        } else {
            // Empty rather than the configured path, so Operations says "file
            // logging is off" instead of "no files yet" — two different things
            // an admin would act on differently.
            String::new()
        },
        uploads_dir: std::path::PathBuf::from(&config.database.uploads_dir),
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
            Err(e) => {
                error!("Failed to bind server: {}", e);
                let _ = shutdown_tx.send(true);
                return;
            }
        };
        if let Err(e) =
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
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
    let mut engine =
        ScrobbleEngine::new(db, mb_client, now_playing, config.listenbrainz_url.clone());

    if let Some(l) = lyrics.clone() {
        engine = engine.with_lyrics(l);
    }
    engine.run(shutdown_rx).await;
    let _ = server_task.await;

    // An operator-requested restart exits **non-zero**, and that is
    // load-bearing rather than cosmetic: `deploy/tapedeck-user.service` is
    // `Restart=on-failure`, so a clean exit would stop the service and the
    // Restart button would be a very convincing off switch. Everything above
    // has already drained, so nothing is cut short by this.
    if server::ops::restart_requested() {
        info!(
            "🔁 Drained — exiting {} so the supervisor restarts us",
            server::ops::RESTART_EXIT_CODE
        );
        std::process::exit(server::ops::RESTART_EXIT_CODE);
    }

    info!("👋 Shut down cleanly");
    Ok(())
}

/// Move the old server-wide Plex row onto the admin as a personal source.
///
/// Plex used to be one instance-wide config: one token, polling every session on
/// the server and attributing listens by matching the Plex account name against
/// Tapedeck usernames. That let the operator's token see everyone's listening and
/// broke whenever the names didn't line up. Sources are per-user now.
///
/// The old row becomes the *admin's* source, because that's whose token it was.
/// Anyone who was previously being name-matched is no longer ingested until they
/// connect their own Plex — which the warning below says out loud, since it's a
/// silent behaviour change otherwise.
async fn migrate_global_plex(
    db: &Database,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    // Seed the legacy row from config on a first run so an existing
    // docker-compose keeps working long enough to be migrated below.
    if db.get_plex_settings().await?.is_none() {
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
        db.save_plex_settings(&seeded).await?;
    }

    let Some(legacy) = db.get_plex_settings().await? else {
        return Ok(());
    };
    if !legacy.is_usable() {
        return Ok(());
    }

    let Some(admin_id) = db.first_admin_id().await? else {
        return Ok(());
    };
    // Already migrated if the admin has any Plex source at all.
    if db.list_sources_for_user(admin_id).await?.iter().any(|s| s.kind == "plex") {
        return Ok(());
    }

    // Resolve the token to its Plex account so the new source can filter to it.
    // If plex.tv is unreachable we still migrate, disabled, and let the user
    // finish in the UI — better than dropping their config on the floor.
    let account =
        match crate::sources::PlexSource::resolve_account(&legacy.url, &legacy.token).await {
            Ok(name) => Some(name),
            Err(e) => {
                warn!("⚠️ Could not resolve the Plex account for the existing token: {e}");
                None
            }
        };

    let credentials = serde_json::json!({ "token": legacy.token }).to_string();
    // Carry the library/device filters over. Dropping them would silently start
    // scrobbling whatever they were excluding — podcasts and audiobooks, usually.
    let options = serde_json::json!({
        "libraries_allow": PlexSettings::list(&legacy.libraries_allow),
        "libraries_block": PlexSettings::list(&legacy.libraries_block),
        "devices_allow": PlexSettings::list(&legacy.devices_allow),
        "devices_block": PlexSettings::list(&legacy.devices_block),
    })
    .to_string();
    db.create_source(
        admin_id,
        "plex",
        "Plex",
        account.is_some(),
        &legacy.url,
        &credentials,
        account.as_deref(),
        None,
        &options,
    )
    .await?;

    info!("⚙️ Migrated the server-wide Plex config to a personal source for the admin");
    if let Some(name) = &account {
        info!("   Plex account: {name} — only that account's sessions are ingested now");
    } else {
        info!("   Left disabled: open Settings → Sources and reconnect to finish");
    }
    warn!(
        "⚠️ Plex is per-user now. Anyone previously matched by Plex account name \
           will not be ingested until they connect their own Plex in Settings → Sources."
    );
    Ok(())
}

/// Cap on rows an in-memory analytics/personality pass will load, scaled to the
/// machine's RAM so a large history can't OOM a request on a small box (e.g. a
/// Pi Zero). Rows are ~200 bytes; we budget ~1/16 of total RAM per pass.
fn analytics_row_cap() -> i64 {
    let total_mb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<i64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(2048); // assume 2 GB if unreadable
    (total_mb / 16 * 5_000).clamp(100_000, 5_000_000)
}

/// Resolves on Ctrl-C or (Unix) SIGTERM — the trigger for the server's graceful
/// drain, which then stops the engine.
async fn shutdown_signal() {
    use tokio::signal;
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };
    // An admin pressing Restart is a third way to arrive here, and it goes
    // through the *same* drain as a signal rather than exiting on the spot —
    // the point of the button is that it is gentler than the SIGKILL somebody
    // reaches for otherwise, not faster.
    let requested = async {
        server::ops::restart_notify().notified().await;
    };
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
        _ = requested => {},
    }
    info!("🛑 Shutdown signal received, draining…");
}
