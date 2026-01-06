use super::ScrobbleSink;
use crate::models::Play;
use async_trait::async_trait;
use reqwest::Client;
// use std::collections::HashMap; // Unused import removed
use log::{info, debug, warn, error}; // Added logging imports

pub struct LastFmSink {
    api_key: String,
    secret: String,
    session_key: String,
    client: Client,
    base_url: String,
}

impl LastFmSink {
    pub fn new(api_key: String, secret: String, session_key: String) -> Self {
        Self {
            api_key,
            secret,
            session_key,
            client: Client::new(),
            base_url: "https://ws.audioscrobbler.com/2.0/".to_string(),
        }
    }

    fn sign_params(&self, params: &mut Vec<(&str, String)>) {
        params.sort_by(|a, b| a.0.cmp(b.0));
        let mut sig_string = String::new();
        for (key, value) in params.iter() {
            sig_string.push_str(key);
            sig_string.push_str(value);
        }
        sig_string.push_str(&self.secret);

        let digest = md5::compute(sig_string);
        let api_sig = hex::encode(digest.0);
        params.push(("api_sig", api_sig));
    }
}

#[async_trait]
impl ScrobbleSink for LastFmSink {
    fn name(&self) -> &str {
        "Last.fm"
    }

    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>> {
        if plays.is_empty() {
            return Ok(());
        }

        info!("Submitting {} plays to Last.fm...", plays.len());

        for play in plays {
            // Filter short tracks
            if let Some(duration) = play.duration {
                if duration <= 30 {
                    warn!("Skipping short track (<30s): {} - {}", play.artist, play.title);
                    continue;
                }
            }

            debug!("Preparing Last.fm scrobble for: {} - {}", play.artist, play.title);

            let mut params: Vec<(&str, String)> = vec![
                ("method", "track.scrobble".to_string()),
                ("api_key", self.api_key.clone()),
                ("sk", self.session_key.clone()),
                ("artist", play.artist.clone()),
                ("track", play.title.clone()),
                ("timestamp", play.timestamp.to_string()),
            ];

            if let Some(album) = &play.album {
                params.push(("album", album.clone()));
            }
            if let Some(duration) = play.duration {
                params.push(("duration", duration.to_string()));
            }
            if let Some(track_num) = play.track_number {
                params.push(("trackNumber", track_num.to_string()));
            }
            if let Some(mbid) = &play.mbid_recording {
                params.push(("mbid", mbid.clone()));
            }

            // Sign request
            self.sign_params(&mut params);

            // Add format=json AFTER signing
            params.push(("format", "json".to_string()));

            let resp = self.client.post(&self.base_url)
                .form(&params)
                .send()
                .await?;

            let status = resp.status();
            debug!("Last.fm API Response Status for '{}': {}", play.title, status);

            if !status.is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                error!("Last.fm scrobble failed for '{}'. Status: {}, Body: {}", play.title, status, error_text);
                // Continue to next track instead of failing entire batch?
                // Currently failing batch as per original logic:
                return Err(format!("Last.fm API Error: {}", error_text).into());
            }
        }

        info!("Successfully scrobbled batch to Last.fm");
        Ok(())
    }
}
