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
                        // 1. Title is available directly
                        title: item.title.to_string(),

                        // 2. Album is optional in PlexItem, so handle the None case
                        album: item.album.clone().unwrap_or_default(),

                        // 3. Artist comes from the item
                        artist: item.artist.clone(),
                        artists: Some(vec![item.artist.clone()]),

                        // 4. Use history_key for the ID (compiler confirmed this field exists)
                        source_id: item.history_key.clone(),
                        source_name: "Plex".to_string(),

                        // 5. Timestamp is mandatory u64. item.viewed_at is usually the timestamp.
                        timestamp: item.viewed_at as u64,

                        // 6. Track number is missing from PlexItem, so we default it
                        track_number: None,

                        // Other fields
                        duration: None,
                        mbid_artist: None,
                        mbid_recording: None,
                        mbid_release: None,
                        mbid_release_group: None,
                    })
                } else { None }
            })
            .collect();
        Ok(plays)
    }
}
