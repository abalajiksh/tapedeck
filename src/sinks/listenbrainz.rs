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
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            client: Client::new(),
        }
    }
}

#[async_trait]
impl ScrobbleSink for ListenBrainzSink {
    fn name(&self) -> &str {
        "ListenBrainz"
    }

    // FIX: Renamed to match trait definition
    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>> {
        if plays.is_empty() {
            return Ok(());
        }

        info!("Submitting {} plays to ListenBrainz...", plays.len());

        let payload_items: Vec<serde_json::Value> = plays.iter().map(|play| {
            debug!("Preparing ListenBrainz payload for: {} - {}", play.artist, play.title);

            let mut track_metadata = json!({
                "artist_name": play.artist,
                "track_name": play.title,
            });

            if let Some(meta) = track_metadata.as_object_mut() {
                if let Some(ref album) = play.album {
                    meta.insert("release_name".to_string(), json!(album));
                }

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
                    additional_info.insert("artist_mbids".to_string(), json!([mbid]));
                }
                if let Some(ref mbid) = play.mbid_release {
                    additional_info.insert("release_mbid".to_string(), json!(mbid));
                }
                if let Some(ref mbid) = play.mbid_recording {
                    additional_info.insert("recording_mbid".to_string(), json!(mbid));
                }

                if !additional_info.is_empty() {
                    meta.insert("additional_info".to_string(), serde_json::Value::Object(additional_info));
                }
            }

            json!({
                "listened_at": play.timestamp,
                "track_metadata": track_metadata
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

        let resp = self.client.post(&endpoint)
            .header("Authorization", format!("Token {}", self.token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let error_text = resp.text().await.unwrap_or_default();
            error!("ListenBrainz submission failed. Status: {}, Response: {}", status, error_text);
            return Err(format!("ListenBrainz API Error: {}", error_text).into());
        }

        info!("Successfully submitted {} plays to ListenBrainz", plays.len());
        Ok(())
    }
}
