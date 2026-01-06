use super::MusicSource;
use crate::models::Play;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use log::{info, debug, error, trace};

pub struct PlexSource {
    pub url: String,
    pub token: String,
    client: Client,
}

impl PlexSource {
    pub fn new(url: String, token: String) -> Self {
        Self { url, token, client: Client::new() }
    }
}

#[derive(Deserialize)]
struct PlexHistoryResponse {
    #[serde(rename = "MediaContainer")]
    container: MediaContainer,
}
#[derive(Deserialize)]
struct MediaContainer {
    #[serde(rename = "Metadata", default)]
    metadata: Vec<PlexItem>,
}
#[derive(Deserialize)]
struct PlexItem {
    #[serde(rename = "viewedAt")]
    viewed_at: u64,
    title: String,
    #[serde(rename = "parentTitle")]
    artist: Option<String>,
    #[serde(rename = "grandparentTitle")]
    album: Option<String>,
    #[serde(rename = "historyKey")]
    history_key: String,
}

#[async_trait]
impl MusicSource for PlexSource {
    fn name(&self) -> &str { "Plex" }

    async fn fetch_new_plays(&self, last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let url = format!("{}/status/sessions/history/all?sort=viewedAt:desc&type=10&limit=200", self.url);

        debug!("Fetching Plex history from: {}", url); // Debug level for URL

        let resp = self.client.get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = resp.status();
        debug!("Plex response status: {}", status);

        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            error!("Failed to fetch Plex history. Status: {}, Body: {}", status, error_text);
            return Err(format!("Plex API error: {}", status).into());
        }

        // Optional: Log the raw body only at TRACE level (very verbose)
        // let text = resp.text().await?;
        // trace!("Raw Plex Response: {}", text);
        // let data: PlexHistoryResponse = serde_json::from_str(&text)?;

        // Standard JSON parsing
        let data: PlexHistoryResponse = match resp.json().await {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to parse Plex JSON: {}", e);
                return Err(e.into());
            }
        };

        let items_found = data.container.metadata.len();
        debug!("Found {} total items in Plex history response", items_found);

        let plays: Vec<Play> = data.container.metadata.into_iter()
            .filter(|item| item.viewed_at > last_checked)
            .filter_map(|item| {
                // Log skipped items at TRACE or DEBUG level if needed
                // debug!("Processing item: {} - {}", item.title, item.viewed_at);

                if let Some(ref artist) = item.artist {
                    // ... your existing logic ...
                    Some(Play { ... })
                } else {
                    warn!("Skipping item '{}' (no artist found)", item.title);
                    None
                }
            })
            .collect();

        info!("Found {} new plays from Plex since last check", plays.len());

        Ok(plays)
    }
}
