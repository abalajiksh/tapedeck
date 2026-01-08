mod config;
mod models;
mod sources;
mod sinks;

use std::time::Duration;
use tokio::time::sleep;
use crate::sources::MusicSource;
use crate::sinks::ScrobbleSink;
use crate::sinks::ListenBrainzSink;
use crate::config::Config;
use log::{info, error, debug, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logger. Default to "info" if RUST_LOG isn't set.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    info!("🚀 Tapedeck Scrobbler Service Started");

    // 1. Load Configuration
    let config = Config::from_env();

    // Debug: Print loaded config
    debug!("ListenBrainz enabled: {}", config.listenbrainz.enabled);
    debug!("ListenBrainz token: '{}'", if config.listenbrainz.token.is_empty() { "<empty>" } else { "<set>" });
    debug!("ListenBrainz URL: {}", config.listenbrainz.base_url);

    // 2. Initialize Sources
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

        // Initialize to fetch libraries
        match plex_source.initialize().await {
            Ok(_) => {
                info!("✅ Plex source initialized successfully");
                sources.push(Box::new(plex_source));
            }
            Err(e) => {
                error!("❌ Failed to initialize Plex: {}", e);
                if config.plex.url.is_empty() || config.plex.token.is_empty() {
                    error!("Please set PLEX_URL and PLEX_TOKEN in your .env file");
                }
            }
        }
    }

    // TODO: Add other sources (Navidrome, Jellyfin) similarly

    if sources.is_empty() {
        error!("❌ No music sources enabled! Please enable at least one source in .env");
        return Ok(());
    }

    // 3. Initialize Sinks
    let mut sinks: Vec<Box<dyn ScrobbleSink>> = Vec::new();

    if config.listenbrainz.enabled {
        if config.listenbrainz.token.is_empty() {
            warn!("ListenBrainz is enabled but token is empty");
        } else {
            info!("Initializing ListenBrainz sink...");
            sinks.push(Box::new(sinks::ListenBrainzSink::new(
                config.listenbrainz.base_url.clone(),
                config.listenbrainz.token.clone(),
            )));
            info!("✅ ListenBrainz sink initialized");
        }
    }

    if config.lastfm.enabled {
        if config.lastfm.api_key.is_empty() || config.lastfm.secret.is_empty() || config.lastfm.session_key.is_empty() {
            warn!("Last.fm is enabled but credentials are incomplete");
        } else {
            info!("Initializing Last.fm sink...");
            sinks.push(Box::new(sinks::LastFmSink::new(
                config.lastfm.api_key.clone(),
                config.lastfm.secret.clone(),
                config.lastfm.session_key.clone(),
            )));
            info!("✅ Last.fm sink initialized");
        }
    }

    if sinks.is_empty() {
        error!("❌ No scrobble destinations enabled! Please enable ListenBrainz or Last.fm in .env");
        return Ok(());
    }

    // 4. State Management
    let mut last_check_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(3600); // Start from 1 hour ago

    info!("🎵 Starting scrobble loop...");
    info!("📊 Monitoring {} source(s) and {} destination(s)", sources.len(), sinks.len());

    let mut iteration = 0u64;

    loop {
        iteration += 1;
        debug!("Starting iteration #{}", iteration);

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Fetch plays from all sources
        for source in &mut sources {
            // Handle Plex specially for now_playing support
            if source.name() == "Plex" {
                if let Some(plex) = source.as_any_mut().downcast_mut::<sources::plex::PlexSource>() {
                    match plex.fetch_sessions_extended(Some(last_check_time)).await {
                        Ok(session_result) => {
                            // Send now_playing updates (only to ListenBrainz)
                            for play in &session_result.now_playing {
                                for sink in &sinks {
                                    if sink.name() == "ListenBrainz" {
                                        // Downcast to call submit_now_playing
                                        if let Some(lb_sink) = sink.as_any().downcast_ref::<ListenBrainzSink>() {
                                            if let Err(e) = lb_sink.submit_now_playing(play).await {
                                                debug!("Now playing error: {}", e);
                                            }
                                        }
                                    }
                                }
                            }

                            // Scrobble completed plays
                            if !session_result.ready_to_scrobble.is_empty() {
                                info!("🎵 Found {} new play(s) from Plex", session_result.ready_to_scrobble.len());
                                for sink in &sinks {
                                    match sink.scrobble(&session_result.ready_to_scrobble).await {
                                        Ok(_) => info!("✅ Successfully sent {} play(s) to {}", session_result.ready_to_scrobble.len(), sink.name()),
                                        Err(e) => error!("❌ Error sending to {}: {}", sink.name(), e),
                                    }
                                }
                            } else {
                                debug!("No new plays from Plex");
                            }
                        }
                        Err(e) => error!("⚠️  Error fetching from Plex: {}", e),
                    }
                    continue; // Skip the generic handling below
                }
            }

            // Generic handling for other sources
            match source.fetch_new_plays(last_check_time).await {
                Ok(plays) => {
                    if !plays.is_empty() {
                        info!("🎵 Found {} new play(s) from {}", plays.len(), source.name());

                        // Send to all sinks
                        for sink in &sinks {
                            match sink.scrobble(&plays).await {
                                Ok(_) => {
                                    info!("✅ Successfully sent {} play(s) to {}", plays.len(), sink.name());
                                }
                                Err(e) => {
                                    error!("❌ Error sending to {}: {}", sink.name(), e);
                                }
                            }
                        }
                    } else {
                        debug!("No new plays from {}", source.name());
                    }
                }
                Err(e) => {
                    error!("⚠️  Error fetching from {}: {}", source.name(), e);
                }
            }
        }

        // Clean up old session states (Plex-specific)
        if let Some(plex_source) = sources.iter_mut()
            .find(|s| s.name() == "Plex")
            .and_then(|s| s.as_any_mut().downcast_mut::<sources::plex::PlexSource>())
        {
            plex_source.cleanup_sessions(3600); // 1 hour
        }

        last_check_time = current_time;

        // Poll every 15 seconds for real-time session monitoring
        let sleep_duration = Duration::from_secs(15);
        debug!("Sleeping for {} seconds until next check...", sleep_duration.as_secs());
        sleep(sleep_duration).await;
    }
}
