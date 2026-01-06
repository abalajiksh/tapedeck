use serde::Deserialize;
use reqwest::Client;
use log::{info, debug, error};
use std::time::Duration;
use crate::models::Play;
use crate::sources::MusicSource;
use async_trait::async_trait;

#[derive(Debug, Deserialize)]
#[serde(rename = "MediaContainer")]
pub struct MediaContainer {
    #[serde(default)]
    pub size: Option<u32>,
    #[serde(rename = "$value", default)]
    pub children: Vec<MediaItem>,
}

#[derive(Debug, Deserialize)]
pub enum MediaItem {
    #[serde(rename = "Video")]
    Video(Video),
    #[serde(rename = "Track")]
    Track(Track),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Video {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub type_: String,
    #[serde(rename = "grandparentTitle", default)]
    pub grandparent_title: Option<String>,
    #[serde(rename = "parentTitle", default)]
    pub parent_title: Option<String>,
    #[serde(rename = "viewedAt", default)]
    pub viewed_at: Option<u64>,
    #[serde(rename = "duration", default)]
    pub duration: Option<u64>,
    #[serde(rename = "User", default)]
    pub user: Option<User>,
    #[serde(rename = "Player", default)]
    pub player: Option<Player>,
    #[serde(rename = "ratingKey", default)]
    pub rating_key: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Track {
    #[serde(default)]
    pub title: String,
    #[serde(rename = "grandparentTitle", default)]
    pub artist: Option<String>,
    #[serde(rename = "parentTitle", default)]
    pub album: Option<String>,
    #[serde(rename = "viewedAt", default)]
    pub viewed_at: Option<u64>,
    #[serde(rename = "User", default)]
    pub user: Option<User>,
    #[serde(rename = "Player", default)]
    pub player: Option<Player>,
    #[serde(rename = "ratingKey", default)]
    pub rating_key: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct User {
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Player {
    #[serde(default)]
    pub state: String,
}

pub struct PlexSource {
    url: String,
    token: String,
    client: Client,
}

impl PlexSource {
    pub fn new(url: String, token: String) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            token,
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    pub async fn fetch_history(&self) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/status/sessions/history/all", self.url);
        debug!("Fetching Plex history from: {}", endpoint);

        let resp = self.client.get(&endpoint)
            .query(&[("sort", "viewedAt:desc"), ("limit", "200"), ("X-Plex-Token", &self.token)])
            .send()
            .await?;

        debug!("Response received: status {}", resp.status());
        let text = resp.text().await?;
        debug!("Response text received, length: {}", text.len());

        // Parse XML using the Enum approach to handle mixed children
        let container: MediaContainer = serde_xml_rs::from_str(&text).map_err(|e| {
            error!("Failed to parse Plex XML: {}", e);
            format!("Plex XML Parse Error: {}", e)
        })?;

        let mut plays = Vec::new();

        for item in container.children {
            match item {
                MediaItem::Video(video) => {
                    if let Some(viewed_at) = video.viewed_at {
                        let artist = video.grandparent_title.clone().or(video.parent_title.clone()).unwrap_or("Unknown".to_string());
                        let title = video.title.clone();
                        let source_id = video.rating_key.clone().unwrap_or_else(|| format!("plex-hist-{}", viewed_at));

                        plays.push(Play {
                            title,
                            artist,
                            artists: None,
                            album: video.parent_title,
                            timestamp: viewed_at,
                            duration: video.duration,
                            track_number: None,
                            mbid_artist: None,
                            mbid_release: None,
                            mbid_release_group: None,
                            mbid_recording: None,
                            source_id,
                            source_name: "Plex".to_string(),
                        });
                    }
                },
                MediaItem::Track(track) => {
                    if let Some(viewed_at) = track.viewed_at {
                        let artist = track.artist.clone().unwrap_or("Unknown".to_string());
                        let source_id = track.rating_key.clone().unwrap_or_else(|| format!("plex-hist-{}", viewed_at));

                        plays.push(Play {
                            title: track.title,
                            artist,
                            artists: None,
                            album: track.album,
                            timestamp: viewed_at,
                            duration: None, // Tracks in history often don't have duration in attributes
                            track_number: None,
                            mbid_artist: None,
                            mbid_release: None,
                            mbid_release_group: None,
                            mbid_recording: None,
                            source_id,
                            source_name: "Plex".to_string(),
                        });
                    }
                },
                MediaItem::Other => {
                    // Ignore other tags
                }
            }
        }

        debug!("Plex: Found {} plays in history", plays.len());
        Ok(plays)
    }
}

#[async_trait]
impl MusicSource for PlexSource {
    fn name(&self) -> &str {
        "Plex"
    }

    async fn fetch_new_plays(&self, last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let history = self.fetch_history().await?;
        // Filter plays strictly newer than last_checked
        Ok(history.into_iter().filter(|p| p.timestamp > last_checked).collect())
    }
}
