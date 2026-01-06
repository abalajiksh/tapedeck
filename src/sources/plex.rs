use async_trait::async_trait;
use reqwest::Client;
use crate::models::Play;
use super::MusicSource;
use log::{debug, error};
use serde::Deserialize;

// --- Internal Structs for Plex XML Response ---

#[derive(Deserialize, Debug)]
struct MediaContainer {
    // We use a flat vector of "PlexItem" enums to handle mixed <Track> and <Video> tags.
    // serde-xml-rs will automatically map <Track> to PlexItem::Track and <Video> to PlexItem::Video
    #[serde(rename = "$value")]
    items: Vec<PlexItem>,
}

#[derive(Deserialize, Debug)]
enum PlexItem {
    #[serde(rename = "Track")]
    Track(PlexTrack),
    #[serde(rename = "Video")]
    Video(PlexVideo), // We'll ignore these, but we need to parse them to not break the list
}

#[derive(Deserialize, Debug)]
struct PlexVideo {
    // We don't care about fields here, just need to consume the tag
    #[serde(rename = "ratingKey")]
    rating_key: Option<String>,
}

#[derive(Deserialize, Debug)]
struct PlexTrack {
    #[serde(rename = "viewedAt")]
    viewed_at: i64,

    title: String,

    #[serde(rename = "parentTitle")] // Album
    album: Option<String>,

    #[serde(rename = "grandparentTitle")] // Artist
    artist: Option<String>,

    #[serde(rename = "historyKey")]
    history_key: String,

    #[serde(rename = "index")]
    track_number: Option<u32>,

    #[serde(rename = "duration")]
    duration: Option<u64>,
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
        // Fetch everything (mixed history)
        let url = format!("{}/status/sessions/history/all?sort=viewedAt:desc&limit=200", self.url);

        debug!("Fetching Plex history from: {}", url);

        let resp = self.client.get(&url)
            .header("X-Plex-Token", &self.token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            error!("Plex API error: {}", error_text);
            return Err(format!("Plex API error: {}", error_text).into());
        }

        let response_text = resp.text().await?;

        // Parse XML into mixed enum list
        let container: MediaContainer = match serde_xml_rs::from_str(&response_text) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to parse Plex XML: {}", e);
                // debug!("Response was: {}", response_text); // Uncomment if needed
                return Err(Box::new(e));
            }
        };

        let plays: Vec<Play> = container.items.into_iter()
            .filter_map(|item| match item {
                PlexItem::Track(track) => Some(track),
                PlexItem::Video(_) => None, // Ignore videos
            })
            .filter(|track| track.viewed_at as u64 > last_checked)
            .filter_map(|item| {
                if let Some(ref artist) = item.artist {
                    Some(Play {
                        title: item.title.clone(),
                        album: item.album.clone(),
                        artist: artist.clone(),
                        artists: Some(vec![artist.clone()]),

                        source_id: item.history_key.clone(),
                        source_name: "Plex".to_string(),
                        timestamp: item.viewed_at as u64,

                        track_number: item.track_number.map(|n| n as i32),
                        duration: item.duration.map(|d| d / 1000),

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
