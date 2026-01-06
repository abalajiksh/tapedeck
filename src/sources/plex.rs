use serde::Deserialize;
use reqwest::Client;
use log::{debug, error};
use std::time::Duration;
use crate::models::Play;
use crate::sources::MusicSource;
use async_trait::async_trait;

#[derive(Debug, Deserialize)]
#[serde(rename = "MediaContainer")]
pub struct MediaContainer {
    #[serde(default)]
    pub size: Option<u32>,
    #[serde(rename = "Track", default)]
    pub tracks: Vec<Track>,
    #[serde(rename = "Video", default)]
    pub videos: Vec<Video>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Video {
    // Attributes we use
    #[serde(default)]
    pub title: String,
    #[serde(rename = "grandparentTitle", default)]
    pub grandparent_title: Option<String>,
    #[serde(rename = "parentTitle", default)]
    pub parent_title: Option<String>,
    #[serde(rename = "viewedAt", default)]
    pub viewed_at: Option<u64>,
    #[serde(rename = "duration", default)]
    pub duration: Option<u64>,
    #[serde(rename = "ratingKey", default)]
    pub rating_key: Option<String>,

    // Other attributes (ignored but needed for deserialization)
    #[serde(rename = "historyKey", default)]
    pub history_key: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(rename = "librarySectionID", default)]
    pub library_section_id: Option<String>,
    #[serde(rename = "parentKey", default)]
    pub parent_key: Option<String>,
    #[serde(rename = "grandparentKey", default)]
    pub grandparent_key: Option<String>,
    #[serde(rename = "type", default)]
    pub type_: Option<String>,
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(rename = "parentThumb", default)]
    pub parent_thumb: Option<String>,
    #[serde(rename = "grandparentThumb", default)]
    pub grandparent_thumb: Option<String>,
    #[serde(rename = "grandparentArt", default)]
    pub grandparent_art: Option<String>,
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(rename = "parentIndex", default)]
    pub parent_index: Option<u32>,
    #[serde(rename = "accountID", default)]
    pub account_id: Option<String>,
    #[serde(rename = "deviceID", default)]
    pub device_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Track {
    // Attributes we use
    #[serde(default)]
    pub title: String,
    #[serde(rename = "grandparentTitle", default)]
    pub artist: Option<String>,
    #[serde(rename = "parentTitle", default)]
    pub album: Option<String>,
    #[serde(rename = "viewedAt", default)]
    pub viewed_at: Option<u64>,
    #[serde(rename = "ratingKey", default)]
    pub rating_key: Option<String>,

    // Other attributes (ignored but needed for deserialization)
    #[serde(rename = "historyKey", default)]
    pub history_key: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(rename = "librarySectionID", default)]
    pub library_section_id: Option<String>,
    #[serde(rename = "parentKey", default)]
    pub parent_key: Option<String>,
    #[serde(rename = "grandparentKey", default)]
    pub grandparent_key: Option<String>,
    #[serde(rename = "type", default)]
    pub type_: Option<String>,
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(rename = "parentThumb", default)]
    pub parent_thumb: Option<String>,
    #[serde(rename = "grandparentThumb", default)]
    pub grandparent_thumb: Option<String>,
    #[serde(rename = "grandparentArt", default)]
    pub grandparent_art: Option<String>,
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(rename = "parentIndex", default)]
    pub parent_index: Option<u32>,
    #[serde(rename = "accountID", default)]
    pub account_id: Option<String>,
    #[serde(rename = "deviceID", default)]
    pub device_id: Option<String>,
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

        let container: MediaContainer = serde_xml_rs::from_str(&text).map_err(|e| {
            error!("Failed to parse Plex XML: {}", e);
            format!("Plex XML Parse Error: {}", e)
        })?;

        debug!("Parsed {} videos and {} tracks", container.videos.len(), container.tracks.len());
        if let Some(first_track) = container.tracks.first() {
            debug!("First track: title='{}', artist={:?}, viewedAt={:?}",
           first_track.title, first_track.artist, first_track.viewed_at);
        }

        let mut plays = Vec::new();

        // Process Videos
        for video in container.videos {
            if let Some(viewed_at) = video.viewed_at {
                let artist = video.grandparent_title
                    .or(video.parent_title.clone())
                    .unwrap_or_else(|| "Unknown".to_string());
                let source_id = video.rating_key
                    .unwrap_or_else(|| format!("plex-hist-{}", viewed_at));

                plays.push(Play {
                    title: video.title,
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
        }

        // Process Tracks
        for track in container.tracks {
            if let Some(viewed_at) = track.viewed_at {
                let artist = track.artist.unwrap_or_else(|| "Unknown".to_string());
                let source_id = track.rating_key
                    .unwrap_or_else(|| format!("plex-hist-{}", viewed_at));

                plays.push(Play {
                    title: track.title,
                    artist,
                    artists: None,
                    album: track.album,
                    timestamp: viewed_at,
                    duration: None,
                    track_number: None,
                    mbid_artist: None,
                    mbid_release: None,
                    mbid_release_group: None,
                    mbid_recording: None,
                    source_id,
                    source_name: "Plex".to_string(),
                });
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
        Ok(history.into_iter().filter(|p| p.timestamp > last_checked).collect())
    }
}
