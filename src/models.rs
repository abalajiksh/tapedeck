use serde::{Deserialize, Serialize};

/// Authenticated user identity, resolved from a valid API token.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Play {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub timestamp: u64,
    pub duration: Option<u64>,
    pub track_number: Option<i32>,
    pub source_id: String,
    pub source_name: String,
    pub mbid_recording: Option<String>,
    pub mbid_release: Option<String>,
    pub mbid_artist: Option<Vec<String>>,
    pub artists: Option<Vec<String>>,
    pub mbid_release_group: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

impl Play {
    pub fn new(title: String, artist: String, timestamp: u64, source_id: String, source_name: String) -> Self {
        Self {
            title, artist, album: None, timestamp, duration: None, track_number: None,
            source_id, source_name, mbid_recording: None, mbid_release: None,
            mbid_artist: None, artists: None, mbid_release_group: None,
            caa_id: None, caa_release_mbid: None,
        }
    }
}
