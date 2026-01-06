use super::ScrobbleSink;
use crate::models::Play;
use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
use serde_json::json;

pub struct ListenBrainzSink {
    pub token: String,
    client: Client,
    base_url: String,
}

impl ListenBrainzSink {
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: Client::new(),
            base_url: "https://api.listenbrainz.org/1/submit-listen".to_string(),
        }
    }
}

// Structs to mirror the Go implementation's JSON payload
#[derive(Serialize)]
struct ListenPayload<'a> {
    listen_type: &'static str,
    payload: Vec<PayloadItem<'a>>,
}

#[derive(Serialize)]
struct PayloadItem<'a> {
    listened_at: u64,
    track_metadata: TrackMetadata<'a>,
}

#[derive(Serialize)]
struct TrackMetadata<'a> {
    artist_name: &'a str,
    track_name: &'a str,
    release_name: Option<&'a str>,
    additional_info: AdditionalInfo<'a>,
}

#[derive(Serialize)]
struct AdditionalInfo<'a> {
    submission_client: &'static str,
    submission_client_version: &'static str,

    // Optional fields (skip if null)
    #[serde(skip_serializing_if = "Option::is_none")]
    track_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,

    // Arrays for multi-artist support
    #[serde(skip_serializing_if = "Option::is_none")]
    artist_names: Option<&'a Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist_mbids: Option<&'a Vec<String>>,

    // Single MBIDs
    #[serde(skip_serializing_if = "Option::is_none")]
    recording_mbid: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_mbid: Option<&'a String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    release_group_mbid: Option<&'a String>,
}

#[async_trait]
impl ScrobbleSink for ListenBrainzSink {
    fn name(&self) -> &str { "ListenBrainz" }

    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>> {
        // Construct the payload in the format ListenBrainz expects
        let payload_items: Vec<PayloadItem> = plays.iter().map(|play| {
            PayloadItem {
                listened_at: play.timestamp,
                track_metadata: TrackMetadata {
                    artist_name: &play.artist,
                    track_name: &play.title,
                    release_name: play.album.as_deref(),
                    additional_info: AdditionalInfo {
                        submission_client: "rust-plex-scrobbler",
                        submission_client_version: "0.1.0",
                        track_number: play.track_number,
                        duration_ms: play.duration.map(|d| d * 1000), // Convert sec -> ms
                        artist_names: play.artists.as_ref(),
                        artist_mbids: play.mbid_artist.as_ref(),
                        recording_mbid: play.mbid_recording.as_ref(),
                        release_mbid: play.mbid_release.as_ref(),
                        release_group_mbid: play.mbid_release_group.as_ref(),
                    },
                },
            }
        }).collect();

        let body = ListenPayload {
            listen_type: "import", // "import" allows historical timestamps
            payload: payload_items,
        };

        // Send Request
        let resp = self.client.post(&self.base_url)
            .header("Authorization", format!("Token {}", self.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let error_text = resp.text().await?;
            return Err(format!("ListenBrainz API Error: {}", error_text).into());
        }

        Ok(())
    }
}
