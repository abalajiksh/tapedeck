use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Play {
    // Basic Metadata
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub timestamp: u64, // UTC Unix Timestamp
    pub duration: Option<u64>, // Duration in seconds (Go uses ms, we'll convert later)
    pub track_number: Option<i32>,

    // Internal Tracking
    pub source_id: String, // Unique ID from source (Plex historyKey, etc.)
    pub source_name: String, // "Plex", "Jellyfin", etc.

    // MusicBrainz / Advanced Metadata
    pub mbid_recording: Option<String>,
    pub mbid_release: Option<String>,       // Album ID
    pub mbid_artist: Option<Vec<String>>,   // Array of Artist MBIDs
    pub artists: Option<Vec<String>>,       // Array of Artist Names
    pub mbid_release_group: Option<String>,
    
    // Cover Art Archive
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

impl Play {
    // Helper to create a basic play
    pub fn new(title: String, artist: String, timestamp: u64, source_id: String, source_name: String) -> Self {
        Self {
            title,
            artist,
            album: None,
            timestamp,
            duration: None,
            track_number: None,
            source_id,
            source_name,
            mbid_recording: None,
            mbid_release: None,
            mbid_artist: None,
            artists: None,
            mbid_release_group: None,
            caa_id: None,
            caa_release_mbid: None,
        }
    }
}
