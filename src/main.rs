mod config;
mod models;
mod sources;
mod sinks;

// Remove `use dotenv::dotenv;` and `use std::env;`
// because `config::Config::from_env()` handles loading .env internally now.
use std::time::Duration;
use tokio::time::sleep;
use crate::sources::MusicSource;
use crate::sinks::ScrobbleSink;
use crate::config::Config; // Import the Config struct
use log::{info, error, debug, warn};

#[tokio::main]
async fn main() {
    // 1. Load Configuration
    // This loads .env and parses all variables into the struct
    let config = Config::from_env();

    // Initialize logger. Default to "info" if RUST_LOG isn't set.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    println!("🚀 Scrobbler Service Started");

    // 2. Initialize Sources
    let mut sources: Vec<Box<dyn MusicSource>> = Vec::new();

    // Use config.plex instead of env::var("PLEX_ENABLED")
    if config.plex.enabled {
        sources.push(Box::new(sources::PlexSource::new(
            config.plex.url.clone(),
            config.plex.token.clone(),
        )));
    }

    // Add other sources similarly using config.navidrome, etc.

    // 3. Initialize Sinks
    let mut sinks: Vec<Box<dyn ScrobbleSink>> = Vec::new();

    if config.listenbrainz.enabled {
        sinks.push(Box::new(sinks::ListenBrainzSink::new(
            config.listenbrainz.base_url.clone(),
            config.listenbrainz.token.clone(), // Now this works because `config` exists!
        )));
    }

    if config.lastfm.enabled {
        sinks.push(Box::new(sinks::LastFmSink::new(
            config.lastfm.api_key.clone(),
            config.lastfm.secret.clone(),
            config.lastfm.session_key.clone(),
        )));
    }

    // 4. State Management
    let mut last_check_time = 1700000000; // Ideally load from file

    loop {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        for source in &sources {
            match source.fetch_new_plays(last_check_time).await {
                Ok(plays) => {
                    if !plays.is_empty() {
                        println!("Found {} plays from {}", plays.len(), source.name());

                        for sink in &sinks {
                            if let Err(e) = sink.scrobble(&plays).await {
                                eprintln!("❌ Error sending to {}: {}", sink.name(), e);
                            } else {
                                println!("✅ Sent to {}", sink.name());
                            }
                        }
                    }
                },
                Err(e) => eprintln!("⚠️ Error fetching from {}: {}", source.name(), e),
            }
        }

        last_check_time = current_time;
        sleep(Duration::from_secs(600)).await;
    }
}
