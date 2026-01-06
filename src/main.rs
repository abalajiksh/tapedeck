mod config;
mod models;
mod sources;
mod sinks;

use dotenv::dotenv;
use std::env;
use std::time::Duration;
use tokio::time::sleep;
use crate::sources::MusicSource;
use crate::sinks::ScrobbleSink;

#[tokio::main]
async fn main() {
    dotenv().ok(); // Load .env

    // 1. Initialize Sources
    let mut sources: Vec<Box<dyn MusicSource>> = Vec::new();

    if env::var("PLEX_ENABLED").unwrap_or("false".into()) == "true" {
        sources.push(Box::new(sources::plex::PlexSource::new(
            env::var("PLEX_URL").unwrap(),
            env::var("PLEX_TOKEN").unwrap(),
        )));
    }

    // (Add Navidrome/Jellyfin/Lyrion init here...)

    // 2. Initialize Sinks
    let mut sinks: Vec<Box<dyn ScrobbleSink>> = Vec::new();

    if env::var("LISTENBRAINZ_ENABLED").unwrap_or("false".into()) == "true" {
        sinks.push(Box::new(sinks::listenbrainz::ListenBrainzSink::new(
            env::var("LISTENBRAINZ_TOKEN").unwrap()
        )));
    }

    // 3. State Management (Simple timestamp tracker)
    // Ideally, load this from a file: state.json -> {"Plex": 1670000000, "Navidrome": ...}
    let mut last_check_time = 1700000000; // Example start time

    println!("🚀 Scrobbler Service Started");

    loop {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        for source in &sources {
            match source.fetch_new_plays(last_check_time).await {
                Ok(plays) => {
                    if !plays.is_empty() {
                        println!("Found {} plays from {}", plays.len(), source.name());

                        // Push to ALL sinks
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

        // Update checkpoint
        last_check_time = current_time;

        // Wait 10 minutes
        sleep(Duration::from_secs(600)).await;
    }
}
