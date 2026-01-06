use super::ScrobbleSink;
use crate::models::Play;
use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
// use serde_json::json; // Unused import removed
use log::{info, debug, error, trace}; // Added logging imports

pub struct ListenBrainzSink {
    pub token: String,
    client: Client,
    base_url: String,
}

impl ListenBrainzSink {
    pub fn new(token: String, base_url: String) -> Self {
        Self {
            token,
            client: Client::new(),
            base_url,
        }
    }
}

// ... [Struct definitions ListenPayload, PayloadItem, etc. remain unchanged] ...
#[derive(Serialize)]
struct ListenPayload<'a> {
    listen_type: &'static str,
    payload: Vec<PayloadItem<'a>>,
}

#[derive(Serialize)]
struct PayloadItem<'a> {
    listened_at: u64,
    track_metadata: TrackMetadata<'a>,
}

#[derive(Serialize)]
struct TrackMetadata<'a> {
    artist_name: &'a str,
    track_name: &'a str,
    release_name: Option<&'a str>,
    additional_info: AdditionalInfo<'a>,
}

#[derive(Serialize)]
struct AdditionalInfo<'a> {
    submission_client: &'static str,
    submission_client_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    track_number: Option<u32>, // Changed from Option to specific type if needed, assuming u32/i32
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist_names: Option<&'a Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist_mbids: Option<&'a Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recording_mbid: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_mbid: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_group_mbid: Option<&'a String>,
}

#[async_trait]
impl ScrobbleSink for ListenBrainzSink {
    fn name(&self) -> &str {
        "ListenBrainz"
    }

    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>> {
        if plays.is_empty() {
            return Ok(());
        }

        info!("Submitting {} plays to ListenBrainz...", plays.len());

        let payload_items: Vec<PayloadItem> = plays.iter().map(|play| {
            debug!("Preparing ListenBrainz payload for: {} - {}", play.artist, play.title);
            PayloadItem {
                listened_at: play.timestamp,
                track_metadata: TrackMetadata {
                    artist_name: &play.artist,
                    track_name: &play.title,
                    release_name: play.album.as_deref(), // Fixed to use as_deref() for Option<String> -> Option<&str>
                    additional_info: AdditionalInfo {
                        submission_client: "rust-plex-scrobbler",
                        submission_client_version: "0.1.0",
                        track_number: play.track_number.map(|n| n as u32),
                        duration_ms: play.duration.map(|d| d * 1000),
                        artist_names: play.artists.as_ref(),
                        artist_mbids: play.mbid_artist.as_ref(),
                        recording_mbid: play.mbid_recording.as_ref(),
                        release_mbid: play.mbid_release.as_ref(),
                        release_group_mbid: play.mbid_release_group.as_ref(),
                    },
                },
            }
        }).collect();

        let body = ListenPayload {
            listen_type: "import",
            payload: payload_items,
        };

        // Trace log the body for deep debugging
        if log::log_enabled!(log::Level::Trace) {
            match serde_json::to_string(&body) {
                Ok(json) => trace!("ListenBrainz Request Body: {}", json),
                Err(e) => error!("Failed to serialize debug payload: {}", e),
            }
        }

        let resp = self.client.post(&self.base_url)
            .header("Authorization", format!("Token {}", self.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        debug!("ListenBrainz API Response Status: {}", status);

        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            error!("ListenBrainz submission failed. Status: {}, Response: {}", status, error_text);
            return Err(format!("ListenBrainz API Error: {}", error_text).into());
        }

        info!("Successfully scrobbled {} plays to ListenBrainz", plays.len());
        Ok(())
    }
}
