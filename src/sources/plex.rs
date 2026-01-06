use async_trait::async_trait;
use reqwest::Client;
use crate::models::{Play, PlexHistoryResponse};
use super::MusicSource; // We need to IMPLEMENT this trait
use log::{info, debug, warn, error};

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
            .filter(|item| item.viewed_at > last_checked)
            .filter_map(|item| {
                if let Some(ref artist) = item.artist {
                    Some(Play {
                        title: item.title.to_string(),
                        album: item.album.clone(), // Option<String>
                        artist: artist.clone(),    // String
                        artists: Some(vec![artist.clone()]), // Option<Vec<String>>

                        source_id: item.history_key.clone(),
                        source_name: "Plex".to_string(),
                        timestamp: item.viewed_at as u64,

                        // Default missing fields
                        track_number: None, // Plex doesn't always provide this in history
                        duration: None,     // You can add item.duration (u64 ms) / 1000 if available
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
