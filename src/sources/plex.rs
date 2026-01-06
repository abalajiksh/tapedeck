use async_trait::async_trait;
use reqwest::Client;
use crate::models::Play;
use super::MusicSource;
use log::{debug, error, trace};
use serde::Deserialize;

// --- Internal Structs for Plex XML Response ---

#[derive(Deserialize, Debug)]
struct MediaContainer {
    // In XML, the root tag is MediaContainer.
    // The inner items are <Track> tags (since type=10).
    // serde-xml-rs handles vectors of children automatically if named correctly.
    #[serde(rename = "Track", default)]
    tracks: Vec<PlexTrack>,
}

#[derive(Deserialize, Debug)]
struct PlexTrack {
    // XML attributes map directly to fields
    #[serde(rename = "viewedAt")]
    viewed_at: i64,

    title: String,

    #[serde(rename = "parentTitle")] // Album
    album: Option<String>,

    #[serde(rename = "grandparentTitle")] // Artist
    artist: Option<String>,

    #[serde(rename = "historyKey")]
    history_key: String,

    // Additional helpful fields
    #[serde(rename = "originalTitle")]
    original_title: Option<String>,

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
        // We removed &Accept=application/json to let it default to XML (or whatever it wants)
        let url = format!("{}/status/sessions/history/all?sort=viewedAt:desc&type=10&limit=200", self.url);

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

        // Get Raw Text (XML)
        let response_text = resp.text().await?;

        // Log trace if needed
        // trace!("Plex XML: {}", response_text);

        // Parse XML
        let container: MediaContainer = match serde_xml_rs::from_str(&response_text) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to parse Plex XML: {}", e);
                debug!("Response was: {}", response_text);
                return Err(Box::new(e));
            }
        };

        let plays: Vec<Play> = container.tracks.into_iter()
            .filter(|item| item.viewed_at as u64 > last_checked)
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

                        track_number: item.track_number,
                        duration: item.duration.map(|d| d / 1000), // Plex duration is ms, usually Play wants seconds?
                        // Check your Play struct definition. If Play.duration is u64 seconds:
                        // duration: item.duration.map(|ms| ms / 1000),

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
