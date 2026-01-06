use async_trait::async_trait;
use reqwest::Client;
use crate::models::Play;
use super::ScrobbleSink;
use log::{info, error};
use std::collections::HashMap; // Import HashMap

pub struct LastFmSink {
    api_key: String,
    secret: String,
    session_key: String,
    client: Client,
}

impl LastFmSink {
    pub fn new(api_key: String, secret: String, session_key: String) -> Self {
        let app_name = env!("CARGO_PKG_NAME");
        let app_version = env!("CARGO_PKG_VERSION");
        let user_agent = format!("{}/{}", app_name, app_version);

        let client = Client::builder()
            .user_agent(user_agent)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            api_key,
            secret,
            session_key,
            client,
        }
    }

    fn generate_signature(&self, params: &Vec<(&str, String)>) -> String {
        let mut sorted_params = params.clone();
        sorted_params.sort_by(|a, b| a.0.cmp(b.0));

        let mut sig_base = String::new();
        for (k, v) in sorted_params {
            sig_base.push_str(k);
            sig_base.push_str(&v);
        }
        sig_base.push_str(&self.secret);

        let digest = md5::compute(sig_base.as_bytes());
        format!("{:x}", digest)
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

        for chunk in plays.chunks(50) {
            let mut params: Vec<(&str, String)> = Vec::new();
            params.push(("method", "track.scrobble".to_string()));
            params.push(("api_key", self.api_key.clone()));
            params.push(("sk", self.session_key.clone()));

            for (i, play) in chunk.iter().enumerate() {
                params.push((Box::leak(format!("artist[{}]", i).into_boxed_str()), play.artist.clone()));
                params.push((Box::leak(format!("track[{}]", i).into_boxed_str()), play.title.clone()));
                params.push((Box::leak(format!("timestamp[{}]", i).into_boxed_str()), play.timestamp.to_string()));

                if let Some(ref album) = play.album {
                    params.push((Box::leak(format!("album[{}]", i).into_boxed_str()), album.clone()));
                }
            }

            let signature = self.generate_signature(&params);
            params.push(("api_sig", signature));
            params.push(("format", "json".to_string()));

            // FIX: Convert Vec to HashMap for reliable serialization
            let form_data: HashMap<&str, String> = params.into_iter().collect();

            let resp = self.client.post("https://ws.audioscrobbler.com/2.0/")
                .form(&form_data)
                .send()
                .await?;

            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                error!("Last.fm submission failed: {}", error_text);
                return Err(format!("Last.fm API Error: {}", error_text).into());
            }
        }

        info!("Successfully submitted {} plays to Last.fm", plays.len());
        Ok(())
    }
}
