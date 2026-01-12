mod config;
mod models;
mod sources;
mod sinks;
mod db;
mod musicbrainz;

use std::time::Duration;
use tokio::time::sleep;
use sqlx::SqlitePool;
use crate::sources::{MusicSource, PlexSource, PlexFilters};
use crate::sinks::ScrobbleSink;
use crate::sinks::ListenBrainzSink;
use crate::config::Config;
use crate::db::Database;
use crate::musicbrainz::MusicBrainzClient;
use log::{info, error, debug, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file first
    dotenv::dotenv().ok();

    // Initialize logger. Default to "info" if RUST_LOG isn't set.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    info!("🚀 Tapedeck Scrobbler Service Started");

    // 1. Load Configuration
    let config = Config::from_env();

    // 2. Initialize SQLite Pool for MusicBrainz Cache
    let sqlite_path = config.database.sqlite_path.clone();
    info!("📦 Initializing SQLite database at {}", sqlite_path);
    let sqlite_pool = SqlitePool::connect(&format!("sqlite:{}?mode=rwc", sqlite_path)).await?;

    // 3. Initialize MusicBrainz Client with 3-tier caching
    info!("🎵 Initializing MusicBrainz metadata client...");
    let mb_config = musicbrainz::MusicBrainzConfig {
        api_base_url: "https://musicbrainz.org/ws/2".to_string(),
        user_agent: config.musicbrainz.user_agent.clone(),
        rate_limit_per_second: config.musicbrainz.rate_limit_per_second,
        postgres_url: config.musicbrainz.postgres_url.clone(),
        enable_postgres: config.musicbrainz.enable_postgres,
    };
    
    let mb_client = MusicBrainzClient::new(mb_config, sqlite_pool.clone()).await?;
    mb_client.initialize_schema().await?;
    
    if config.musicbrainz.enable_postgres {
        info!("✅ MusicBrainz client initialized with PostgreSQL dump support");
    } else {
        info!("✅ MusicBrainz client initialized (SQLite + API)");
    }

    // 4. Initialize Scrobble Database
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:scrobbles.db?mode=rwc".to_string());
    info!("📦 Initializing scrobble database at {}", db_url);
    let db = match Database::new(&db_url).await {
        Ok(db) => db,
        Err(e) => {
            error!("❌ Failed to connect to scrobble database: {}", e);
            return Err(e.into());
        }
    };

    // 5. Initialize Sources
    let mut sources: Vec<Box<dyn MusicSource>> = Vec::new();

    // Initialize Plex with filters
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

        let mut plex_source = PlexSource::with_filters(
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

    // 6. Initialize Sinks
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

    info!("🎵 Starting scrobble loop with prioritized MusicBrainz metadata enrichment...");
    
    // We fetch history for the last 24 hours to catch offline plays
    // SQLite handles deduplication
    let history_window_seconds = 86400; // 24 hours

    loop {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Track if there are any active now playing sessions
        let mut has_active_now_playing = false;

        // 1. PRIORITY: Handle Now Playing and New Scrobbles with MusicBrainz enrichment
        for source in &mut sources {
            if source.name() == "Plex" {
                if let Some(plex) = source.as_any_mut().downcast_mut::<PlexSource>() {
                    // Fetch recent history + active sessions
                    // We look back 24h to catch any late-synced plays
                    let lookback_time = current_time.saturating_sub(history_window_seconds);
                    
                    match plex.fetch_sessions_extended(Some(lookback_time)).await {
                        Ok(session_result) => {
                            // A. Handle Now Playing (Stateless, immediate) - ALWAYS with metadata
                            if !session_result.now_playing.is_empty() {
                                has_active_now_playing = true;
                                info!("🎧 {} active now playing session(s) detected", session_result.now_playing.len());
                            }

                            for plex_track in &session_result.now_playing {
                                let mut play = plex_track.to_play("np");
                                
                                // PRIORITY: Always fetch MusicBrainz metadata for now playing
                                debug!("Fetching MusicBrainz metadata for now playing: {} - {}", play.artist, play.title);
                                match mb_client.fetch_metadata(
                                    &plex_track.title,
                                    &plex_track.artist,
                                    plex_track.album.as_deref(),
                                ).await {
                                    Ok(metadata) => {
                                        // Clone values for logging before moving them
                                        let track_mbid_str = metadata.track_mbid.as_deref().unwrap_or("none").to_string();
                                        let album_mbid_str = metadata.album_mbid.as_deref().unwrap_or("none").to_string();
                                        let artist_mbid_str = metadata.artist_mbid.as_deref().unwrap_or("none").to_string();
                                        
                                        play.mbid_recording = metadata.track_mbid;
                                        play.mbid_release = metadata.album_mbid;
                                        play.mbid_artist = metadata.artist_mbid.as_ref().map(|id| vec![id.clone()]);
                                        play.caa_id = metadata.caa_id;
                                        play.caa_release_mbid = metadata.caa_release_mbid;
                                        
                                        info!("✓ Enriched now playing: {} - {} [recording: {}, release: {}, artist: {}]",
                                            play.artist,
                                            play.title,
                                            track_mbid_str,
                                            album_mbid_str,
                                            artist_mbid_str
                                        );
                                    }
                                    Err(e) => {
                                        warn!("⚠ MusicBrainz lookup failed for now playing {} - {}: {}", 
                                            play.artist, play.title, e);
                                    }
                                }
                                
                                // Submit to sinks
                                for sink in &sinks {
                                    if let Some(lb_sink) = sink.as_any().downcast_ref::<ListenBrainzSink>() {
                                        if let Err(e) = lb_sink.submit_now_playing(&play).await {
                                            error!("Failed to submit now playing to {}: {}", sink.name(), e);
                                        }
                                    }
                                }
                            }

                            // B. Process Scrobble Candidates - ALWAYS with MusicBrainz Enrichment
                            if !session_result.ready_to_scrobble.is_empty() {
                                info!("📀 Processing {} ready-to-scrobble track(s)", session_result.ready_to_scrobble.len());
                                
                                for plex_track in session_result.ready_to_scrobble {
                                    let mut play = plex_track.to_play(&format!("scrobble-{}", current_time));
                                    
                                    // PRIORITY: Always fetch MusicBrainz metadata for scrobbles
                                    debug!("Fetching MusicBrainz metadata for scrobble: {} - {}", play.artist, play.title);
                                    match mb_client.fetch_metadata(
                                        &plex_track.title,
                                        &plex_track.artist,
                                        plex_track.album.as_deref(),
                                    ).await {
                                        Ok(metadata) => {
                                            // Clone values for logging before moving them
                                            let track_mbid_str = metadata.track_mbid.as_deref().unwrap_or("none").to_string();
                                            let album_mbid_str = metadata.album_mbid.as_deref().unwrap_or("none").to_string();
                                            let artist_mbid_str = metadata.artist_mbid.as_deref().unwrap_or("none").to_string();
                                            let caa_id_str = metadata.caa_id.map(|id| id.to_string()).unwrap_or_else(|| "none".to_string());
                                            
                                            play.mbid_recording = metadata.track_mbid;
                                            play.mbid_release = metadata.album_mbid;
                                            play.mbid_artist = metadata.artist_mbid.as_ref().map(|id| vec![id.clone()]);
                                            play.caa_id = metadata.caa_id;
                                            play.caa_release_mbid = metadata.caa_release_mbid;
                                            
                                            info!("✓ Enriched scrobble: {} - {} [recording: {}, release: {}, artist: {}, caa: {}]",
                                                play.artist,
                                                play.title,
                                                track_mbid_str,
                                                album_mbid_str,
                                                artist_mbid_str,
                                                caa_id_str
                                            );
                                        }
                                        Err(e) => {
                                            warn!("⚠ MusicBrainz lookup failed for scrobble {} - {}: {}", 
                                                play.artist, play.title, e);
                                            // Continue with basic metadata from Plex
                                        }
                                    }
                                    
                                    // Save to database (with or without MBIDs)
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

        // 2. DEFERRED: Process Pending Scrobbles ONLY when no active now playing
        if !has_active_now_playing {
            match db.get_pending_scrobbles().await {
                Ok(pending_plays) => {
                    if !pending_plays.is_empty() {
                        info!("🔄 No active sessions - processing {} pending scrobble(s) from history", pending_plays.len());
                        
                        for mut play in pending_plays {
                            // Enrich with MusicBrainz if we don't have MBIDs yet (for old scrobbles)
                            if play.mbid_recording.is_none() {
                                debug!("Enriching pending scrobble: {} - {}", play.artist, play.title);
                                match mb_client.fetch_metadata(
                                    &play.title,
                                    &play.artist,
                                    play.album.as_deref(),
                                ).await {
                                    Ok(metadata) => {
                                        play.mbid_recording = metadata.track_mbid;
                                        play.mbid_release = metadata.album_mbid;
                                        play.mbid_artist = metadata.artist_mbid.as_ref().map(|id| vec![id.clone()]);
                                        play.caa_id = metadata.caa_id;
                                        play.caa_release_mbid = metadata.caa_release_mbid;
                                        debug!("✓ Enriched pending scrobble with MBIDs");
                                    }
                                    Err(e) => {
                                        debug!("Could not enrich pending scrobble: {}", e);
                                    }
                                }
                            }
                            
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
        } else {
            debug!("⏸ Skipping pending scrobbles processing - active now playing session detected");
        }

        // Clean up old session states (Plex-specific)
        if let Some(plex_source) = sources.iter_mut()
            .find(|s| s.name() == "Plex")
            .and_then(|s| s.as_any_mut().downcast_mut::<PlexSource>())
        {
            plex_source.cleanup_sessions(3600);
        }

        sleep(Duration::from_secs(15)).await;
    }
}
