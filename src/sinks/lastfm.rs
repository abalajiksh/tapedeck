use super::ScrobbleSink;
use crate::models::Play;
use async_trait::async_trait;
use reqwest::Client;
use std::collections::HashMap;

pub struct LastFmSink {
    api_key: String,
    secret: String,
    session_key: String, // Authenticated Session Key (sk)
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

    /// Generates the api_sig md5 hash based on sorted parameters
    /// See: client-lastfm.go func (c *client) sign(params url.Values)
    fn sign_params(&self, params: &mut Vec<(&str, String)>) {
        // 1. Sort parameters alphabetically by key
        params.sort_by(|a, b| a.0.cmp(b.0));

        // 2. Concatenate key + value + secret
        let mut sig_string = String::new();
        for (key, value) in params.iter() {
            sig_string.push_str(key);
            sig_string.push_str(value);
        }
        sig_string.push_str(&self.secret);

        // 3. MD5 Hash
        let digest = md5::compute(sig_string);
        let api_sig = hex::encode(digest.0);

        params.push(("api_sig", api_sig));
    }
}

#[async_trait]
impl ScrobbleSink for LastFmSink {
    fn name(&self) -> &str { "Last.fm" }

    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>> {
        for play in plays {
            // Filter short tracks (Logic from agent-lastfm.go)
            if let Some(duration) = play.duration {
                if duration <= 30 {
                    println!("Skipping short track: {}", play.title);
                    continue;
                }
            }

            // Prepare Parameters
            // Note: Last.fm API requires "method", "api_key", "sk", and track info
            let mut params: Vec<(&str, String)> = vec![
                ("method", "track.scrobble".to_string()),
                ("api_key", self.api_key.clone()),
                ("sk", self.session_key.clone()),
                ("artist", play.artist.clone()),
                ("track", play.title.clone()),
                ("timestamp", play.timestamp.to_string()),
            ];

            // Optional Fields (Album, MBID, Duration, TrackNumber)
            if let Some(album) = &play.album {
                params.push(("album", album.clone()));
            }
            if let Some(duration) = play.duration {
                params.push(("duration", duration.to_string()));
            }
            if let Some(track_num) = play.track_number {
                params.push(("trackNumber", track_num.to_string()));
            }
            // Use Recording MBID if available (preferred), else fallback
            if let Some(mbid) = &play.mbid_recording {
                params.push(("mbid", mbid.clone()));
            }

            // Sign the request
            self.sign_params(&mut params);

            // Add format=json (Must be added AFTER signing, usually)
            // But Last.fm docs say format parameter is NOT part of signature.
            // In client-lastfm.go, it's added but skipped in the signing loop.
            // My sign_params function only signs what's in the Vec, so we add format here.
            params.push(("format", "json".to_string()));

            // Send Request
            let resp = self.client.post(&self.base_url)
                .form(&params) // Encodes as application/x-www-form-urlencoded
                .send()
                .await?;

            if !resp.status().is_success() {
                let error_text = resp.text().await?;
                return Err(format!("Last.fm API Error: {}", error_text).into());
            }
        }

        Ok(())
    }
}
