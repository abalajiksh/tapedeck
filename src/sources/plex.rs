use async_trait::async_trait;
use reqwest::Client;
use crate::models::Play; // Only import Play from models
use super::MusicSource;
use log::{debug, error}; // info and warn removed if unused
use serde::Deserialize; // <--- CRITICAL: Import Deserialize macro

// --- Internal Structs for Plex Response ---
// Defined here because they are specific to this source

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
    viewed_at: i64, // Use i64 for timestamps (or u64)
    title: String,
    #[serde(rename = "parentTitle")]
    album: Option<String>,
    #[serde(rename = "grandparentTitle")]
    artist: Option<String>,
    #[serde(rename = "historyKey")]
    history_key: String,
    // Add other fields if needed for debugging, but these are minimum required
}

// --- Source Implementation ---

pub struct PlexSource {
    url: String,
    token: String,
    client: Client,
}

impl PlexSource {
    pub fn new(url: String, token: String) -> Self {
        Self {
            url,
            token,
            client: Client::new(),
        }
    }
}

#[async_trait]
impl MusicSource for PlexSource {
    fn name(&self) -> &str {
        "Plex"
    }

    async fn fetch_new_plays(&self, last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let url = format!("{}/status/sessions/history/all?sort=viewedAt:desc&type=10&limit=200", self.url);

        debug!("Fetching Plex history from: {}", url);

        let resp = self.client.get(&url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            error!("Plex API error: {}", error_text);
            return Err(format!("Plex API error: {}", error_text).into());
        }

        let data: PlexHistoryResponse = resp.json().await?;

        let plays: Vec<Play> = data.container.metadata.into_iter()
            .filter(|item| item.viewed_at as u64 > last_checked) // Cast i64 -> u64
            .filter_map(|item| {
                if let Some(ref artist) = item.artist {
                    Some(Play {
                        title: item.title.to_string(),
                        album: item.album.clone(),
                        artist: artist.clone(),
                        artists: Some(vec![artist.clone()]),

                        source_id: item.history_key.clone(),
                        source_name: "Plex".to_string(),
                        timestamp: item.viewed_at as u64,

                        track_number: None,
                        duration: None,
                        mbid_artist: None,
                        mbid_recording: None,
                        mbid_release: None,
                        mbid_release_group: None,
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(plays)
    }
}
