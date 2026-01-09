mod config;
mod models;
mod sources;
mod sinks;
mod db;

use std::time::Duration;
use tokio::time::sleep;
use crate::sources::MusicSource;
use crate::sinks::ScrobbleSink;
use crate::sinks::ListenBrainzSink;
use crate::config::Config;
use crate::db::Database;
use log::{info, error, debug, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logger. Default to "info" if RUST_LOG isn't set.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    info!("🚀 Tapedeck Scrobbler Service Started");

    // 1. Initialize Database
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:scrobbles.db".to_string());
    info!("📦 Initializing SQLite database at {}", db_url);
    let db = match Database::new(&db_url).await {
        Ok(db) => db,
        Err(e) => {
            error!("❌ Failed to connect to database: {}", e);
            return Err(e.into());
        }
    };

    // 2. Load Configuration
    let config = Config::from_env();

    // 3. Initialize Sources
    let mut sources: Vec<Box<dyn MusicSource>> = Vec::new();

    // Initialize Plex with filters
    if config.plex.enabled {
        info!("Initializing Plex source...");

        let filters = sources::plex::PlexFilters {
            users_allow: config.plex.users_allow.clone(),
            users_block: config.plex.users_block.clone(),
            devices_allow: config.plex.devices_allow.clone(),
            devices_block: config.plex.devices_block.clone(),
            libraries_allow: config.plex.libraries_allow.clone(),
            libraries_block: config.plex.libraries_block.clone(),
        };

        let mut plex_source = sources::plex::PlexSource::with_filters(
            config.plex.url.clone(),
            config.plex.token.clone(),
            filters,
        );

        match plex_source.initialize().await {
            Ok(_) => {
                info!("✅ Plex source initialized successfully");
                sources.push(Box::new(plex_source));
            }
            Err(e) => {
                error!("❌ Failed to initialize Plex: {}", e);
            }
        }
    }

    if sources.is_empty() {
        error!("❌ No music sources enabled!");
        return Ok(());
    }

    // 4. Initialize Sinks
    let mut sinks: Vec<Box<dyn ScrobbleSink>> = Vec::new();

    if config.listenbrainz.enabled {
        info!("Initializing ListenBrainz sink...");
        sinks.push(Box::new(sinks::ListenBrainzSink::new(
            config.listenbrainz.base_url.clone(),
            config.listenbrainz.token.clone(),
        )));
    }

    if config.lastfm.enabled {
        info!("Initializing Last.fm sink...");
        sinks.push(Box::new(sinks::LastFmSink::new(
            config.lastfm.api_key.clone(),
            config.lastfm.secret.clone(),
            config.lastfm.session_key.clone(),
        )));
    }

    if sinks.is_empty() {
        error!("❌ No scrobble destinations enabled!");
        return Ok(());
    }

    info!("🎵 Starting scrobble loop...");
    
    // We fetch history for the last 24 hours to catch offline plays
    // SQLite handles deduplication
    let history_window_seconds = 86400; // 24 hours

    loop {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // 1. Fetch from Sources and Store in DB
        for source in &mut sources {
            if source.name() == "Plex" {
                if let Some(plex) = source.as_any_mut().downcast_mut::<sources::plex::PlexSource>() {
                    // Fetch recent history + active sessions
                    // We look back 24h to catch any late-synced plays
                    let lookback_time = current_time.saturating_sub(history_window_seconds);
                    
                    match plex.fetch_sessions_extended(Some(lookback_time)).await {
                        Ok(session_result) => {
                            // A. Handle Now Playing (Stateless, immediate)
                            for play in &session_result.now_playing {
                                for sink in &sinks {
                                    if let Some(lb_sink) = sink.as_any().downcast_ref::<ListenBrainzSink>() {
                                        let _ = lb_sink.submit_now_playing(play).await;
                                    }
                                }
                            }

                            // B. Store Scrobble Candidates in DB
                            if !session_result.ready_to_scrobble.is_empty() {
                                debug!("Processing {} potential scrobbles from Plex", session_result.ready_to_scrobble.len());
                                for play in session_result.ready_to_scrobble {
                                    match db.save_scrobble(&play).await {
                                        Ok(saved) => {
                                            if saved {
                                                info!("📥 Queued new play: {} - {}", play.artist, play.title);
                                            }
                                        }
                                        Err(e) => error!("Database error: {}", e),
                                    }
                                }
                            }
                        }
                        Err(e) => error!("Error fetching from Plex: {}", e),
                    }
                }
            }
            // TODO: Add generic handling for other sources if needed
        }

        // 2. Process Pending Scrobbles from DB
        match db.get_pending_scrobbles().await {
            Ok(pending_plays) => {
                if !pending_plays.is_empty() {
                    info!("🚀 Processing {} pending scrobble(s)", pending_plays.len());
                    
                    for play in pending_plays {
                        let mut all_succeeded = true;
                        
                        for sink in &sinks {
                            match sink.scrobble(&vec![play.clone()]).await {
                                Ok(_) => debug!("Sent to {}", sink.name()),
                                Err(e) => {
                                    error!("Failed to send to {}: {}", sink.name(), e);
                                    all_succeeded = false;
                                }
                            }
                        }

                        if all_succeeded {
                            if let Err(e) = db.mark_as_scrobbled(&play.source_id, &play.source_name).await {
                                error!("Failed to mark as scrobbled: {}", e);
                            } else {
                                info!("✅ Synced: {} - {}", play.artist, play.title);
                            }
                        }
                    }
                }
            }
            Err(e) => error!("Failed to fetch pending scrobbles: {}", e),
        }

        // Clean up old session states (Plex-specific)
        if let Some(plex_source) = sources.iter_mut()
            .find(|s| s.name() == "Plex")
            .and_then(|s| s.as_any_mut().downcast_mut::<sources::plex::PlexSource>())
        {
            plex_source.cleanup_sessions(3600);
        }

        sleep(Duration::from_secs(15)).await;
    }
}
