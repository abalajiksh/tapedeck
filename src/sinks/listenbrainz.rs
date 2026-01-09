use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use crate::models::Play;
use super::ScrobbleSink;
use log::{info, debug, error, warn};
use tokio::time::{sleep, Duration};

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

        // Simple retry logic for now playing, but less aggressive than scrobble
        let mut retries = 0;
        const MAX_RETRIES: u32 = 1;

        loop {
            let resp = self.client.post(&endpoint)
                .header("Authorization", &auth_header_value)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await;

            match resp {
                Ok(response) => {
                    if response.status().is_success() {
                        debug!("✅ Now playing submitted: {} - {}", play.artist, play.title);
                        return Ok(());
                    } else if response.status() == 429 {
                        if retries >= MAX_RETRIES {
                            let error_text = response.text().await.unwrap_or_default();
                            error!("Now playing rate limited (gave up): {}", error_text);
                            return Ok(()); // Don't fail the whole app for now playing
                        }
                        
                        let retry_after = response.headers()
                            .get("X-RateLimit-Reset-In")
                            .and_then(|h| h.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(2); // Default to 2s if header missing
                        
                        debug!("Rate limited on now playing, waiting {}s", retry_after);
                        sleep(Duration::from_secs(retry_after + 1)).await;
                        retries += 1;
                        continue;
                    } else {
                        let error_text = response.text().await.unwrap_or_default();
                        error!("Now playing submission failed: {}", error_text);
                        return Ok(()); // Swallow error for now playing
                    }
                }
                Err(e) => {
                    error!("Network error sending now playing: {}", e);
                    return Ok(());
                }
            }
        }
    }
}

#[async_trait]
impl ScrobbleSink for ListenBrainzSink {
    fn name(&self) -> &str {
        "ListenBrainz"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
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

        // ListenBrainz supports max 1000 listens per request
        // We'll chunk them just in case, though usually plays.len() is small
        for chunk in payload_items.chunks(100) {
            let body = json!({
                "listen_type": "import",
                "payload": chunk
            });

            let endpoint = if self.base_url.ends_with("submit-listens") {
                self.base_url.clone()
            } else {
                format!("{}/1/submit-listens", self.base_url)
            };

            let auth_header_value = format!("Token {}", self.token);
            
            let mut retries = 0;
            const MAX_RETRIES: u32 = 5;
            let mut backoff = 2;

            loop {
                debug!("Submitting chunk of {} plays (attempt {}/{})", chunk.len(), retries + 1, MAX_RETRIES);
                
                let resp = self.client.post(&endpoint)
                    .header("Authorization", &auth_header_value)
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await;

                match resp {
                    Ok(response) => {
                        if response.status().is_success() {
                            info!("✅ Successfully submitted {} plays to ListenBrainz", chunk.len());
                            break; // Chunk success, move to next chunk
                        } else if response.status() == 429 {
                            let reset_in = response.headers()
                                .get("X-RateLimit-Reset-In")
                                .and_then(|h| h.to_str().ok())
                                .and_then(|s| s.parse::<u64>().ok());
                            
                            let remaining = response.headers()
                                .get("X-RateLimit-Remaining")
                                .and_then(|h| h.to_str().ok())
                                .unwrap_or("?");

                            let wait_time = reset_in.unwrap_or(backoff);
                            
                            warn!("ListenBrainz rate limit exceeded (remaining: {}). Waiting {}s before retry...", remaining, wait_time);
                            
                            sleep(Duration::from_secs(wait_time + 1)).await;
                            
                            retries += 1;
                            if retries > MAX_RETRIES {
                                let error_text = response.text().await.unwrap_or_default();
                                error!("Max retries exceeded for ListenBrainz. Last error: {}", error_text);
                                return Err(format!("Rate limit exceeded after retries: {}", error_text).into());
                            }
                            
                            // Exponential backoff fallback if header was missing
                            if reset_in.is_none() {
                                backoff *= 2;
                            }
                            continue;
                        } else {
                            let status = response.status();
                            let error_text = response.text().await.unwrap_or_default();
                            error!("ListenBrainz submission failed. Status: {}, Response: {}", status, error_text);
                            return Err(format!("ListenBrainz API Error: {}", error_text).into());
                        }
                    }
                    Err(e) => {
                        error!("Network error sending to ListenBrainz: {}", e);
                        retries += 1;
                        if retries > MAX_RETRIES {
                            return Err(format!("Network error after retries: {}", e).into());
                        }
                        sleep(Duration::from_secs(backoff)).await;
                        backoff *= 2;
                        continue;
                    }
                }
            }
        }

        Ok(())
    }
}
