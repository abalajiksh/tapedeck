use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use crate::models::Play;
use super::ScrobbleSink;
use log::{info, debug, error};

pub struct ListenBrainzSink {
    base_url: String,
    token: String,
    client: Client,
}

impl ListenBrainzSink {
    pub fn new(base_url: String, token: String) -> Self {
        let sanitized_token = token.trim()
            .chars()
            .filter(|c| c.is_ascii() && !c.is_control())
            .collect::<String>();

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: sanitized_token,
            client: Client::new(),
        }
    }

    /// Submit "now playing" status for a track
    pub async fn submit_now_playing(&self, play: &Play) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Submitting now playing: {} - {}", play.artist, play.title);

        let mut track_meta = serde_json::Map::new();
        track_meta.insert("artist_name".to_string(), json!(play.artist));
        track_meta.insert("track_name".to_string(), json!(play.title));

        if let Some(ref album) = play.album {
            track_meta.insert("release_name".to_string(), json!(album));
        }

        let mut additional_info = serde_json::Map::new();
        additional_info.insert("submission_client".to_string(), json!(env!("CARGO_PKG_NAME")));
        additional_info.insert("submission_client_version".to_string(), json!(env!("CARGO_PKG_VERSION")));

        if let Some(dur) = play.duration {
            additional_info.insert("duration_ms".to_string(), json!(dur * 1000));
        }

        if let Some(ref mbid) = play.mbid_recording {
            additional_info.insert("recording_mbid".to_string(), json!(mbid));
        }

        if !additional_info.is_empty() {
            track_meta.insert("additional_info".to_string(), serde_json::Value::Object(additional_info));
        }

        let body = json!({
            "listen_type": "playing_now",
            "payload": [{
                "track_metadata": track_meta
            }]
        });

        let endpoint = format!("{}/1/submit-listens", self.base_url);
        let auth_header_value = format!("Token {}", self.token);

        let resp = self.client.post(&endpoint)
            .header("Authorization", &auth_header_value)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            error!("Now playing submission failed: {}", error_text);
        } else {
            debug!("✅ Now playing submitted: {} - {}", play.artist, play.title);
        }

        Ok(())
    }
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

        let payload_items: Vec<serde_json::Value> = plays.iter().map(|play| {
            debug!("Preparing ListenBrainz payload for: {} - {}", play.artist, play.title);

            // Build track_metadata
            let mut track_meta = serde_json::Map::new();
            track_meta.insert("artist_name".to_string(), json!(play.artist));
            track_meta.insert("track_name".to_string(), json!(play.title));

            if let Some(ref album) = play.album {
                track_meta.insert("release_name".to_string(), json!(album));
            }

            // Build additional_info
            let mut additional_info = serde_json::Map::new();
            additional_info.insert("submission_client".to_string(), json!(env!("CARGO_PKG_NAME")));
            additional_info.insert("submission_client_version".to_string(), json!(env!("CARGO_PKG_VERSION")));

            if let Some(dur) = play.duration {
                additional_info.insert("duration_ms".to_string(), json!(dur * 1000));
            }

            if let Some(num) = play.track_number {
                additional_info.insert("track_number".to_string(), json!(num));
            }

            // Add MBIDs if available
            if let Some(ref mbid) = play.mbid_artist {
                if let Some(first_mbid) = mbid.first() {
                    additional_info.insert("artist_mbids".to_string(), json!([first_mbid]));
                }
            }

            if let Some(ref mbid) = play.mbid_release {
                additional_info.insert("release_mbid".to_string(), json!(mbid));
            }

            if let Some(ref mbid) = play.mbid_recording {
                additional_info.insert("recording_mbid".to_string(), json!(mbid));
            }

            if !additional_info.is_empty() {
                track_meta.insert("additional_info".to_string(), serde_json::Value::Object(additional_info));
            }

            json!({
                "listened_at": play.timestamp,
                "track_metadata": track_meta
            })
        }).collect();

        let body = json!({
            "listen_type": "import",
            "payload": payload_items
        });

        let endpoint = if self.base_url.ends_with("submit-listens") {
            self.base_url.clone()
        } else {
            format!("{}/1/submit-listens", self.base_url)
        };

        debug!("Submitting to ListenBrainz endpoint: {}", endpoint);
        debug!("Payload size: {} listens", payload_items.len());
        debug!("Token length: {} chars", self.token.len());

        // Build request step by step to isolate the error
        let auth_header_value = format!("Token {}", self.token);

        debug!("Building POST request...");
        let mut request_builder = self.client.post(&endpoint);

        debug!("Adding Authorization header...");
        request_builder = request_builder.header("Authorization", &auth_header_value);

        debug!("Adding Content-Type header...");
        request_builder = request_builder.header("Content-Type", "application/json");

        debug!("Adding JSON body...");
        request_builder = request_builder.json(&body);

        debug!("Sending request...");
        let resp = request_builder.send().await.map_err(|e| {
            error!("Failed to send request to ListenBrainz: {:?}", e);
            format!("Request error: {}", e)
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error_text = resp.text().await.unwrap_or_default();
            error!("ListenBrainz submission failed. Status: {}, Response: {}", status, error_text);
            return Err(format!("ListenBrainz API Error: {}", error_text).into());
        }

        info!("✅ Successfully submitted {} plays to ListenBrainz", plays.len());
        Ok(())
    }
}
