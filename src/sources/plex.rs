use super::MusicSource;
use crate::models::Play;
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

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
        // Fetch last 50 items (deep enough to catch offline syncs)
        let url = format!("{}/status/sessions/history/all?sort=viewedAt:desc&type=10&limit=50", self.url);
        let resp = self.client.get(&url).header("X-Plex-Token", &self.token).send().await?;
        let data: PlexHistoryResponse = resp.json().await?;

        let plays = data.container.metadata.into_iter()
            .filter(|item| item.viewed_at > last_checked) // Basic filter
            .filter_map(|item| {
                if let Some(artist) = item.artist {
                    Some(Play {
                        title: item.title,
                        artist,
                        album: item.album,
                        timestamp: item.viewed_at,
                        duration: None,
                        source_id: item.history_key,
                    })
                } else { None }
            })
            .collect();
        Ok(plays)
    }
}
