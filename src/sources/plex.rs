use serde::{Deserialize, Deserializer};
use reqwest::Client;
use log::{debug, info, warn, error};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::models::Play;
use crate::sources::MusicSource;
use async_trait::async_trait;

// ==================== Deserialization Helpers ====================

/// Deserialize a u64 that might come as either a number or a string
fn deserialize_u64_flexible<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct U64Visitor;

    impl<'de> Visitor<'de> for U64Visitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a u64 as either a number or a string")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value as u64))
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value.parse::<u64>().map(Some).map_err(de::Error::custom)
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }
    }

    deserializer.deserialize_any(U64Visitor)
}

// ==================== Plex API Response Structures (JSON) ====================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionsResponse {
    pub media_container: SessionsContainer,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionsContainer {
    #[serde(default)]
    pub metadata: Vec<SessionMetadata>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    #[serde(rename = "type")]
    pub media_type: Option<String>,

    pub title: Option<String>,
    pub parent_title: Option<String>,
    pub grandparent_title: Option<String>,
    pub original_title: Option<String>,

    pub rating_key: Option<String>,
    pub parent_rating_key: Option<String>,
    pub grandparent_rating_key: Option<String>,

    pub library_section_title: Option<String>,
    pub library_section_id: Option<String>,

    #[serde(deserialize_with = "deserialize_u64_flexible", default)]
    pub duration: Option<u64>,

    #[serde(deserialize_with = "deserialize_u64_flexible", default)]
    pub view_offset: Option<u64>,

    pub session_key: Option<String>,

    #[serde(rename = "Player")]
    pub player: Option<Player>,

    #[serde(rename = "User")]
    pub user: Option<User>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub state: Option<String>, // "playing", "paused", "stopped"
    pub title: Option<String>,
    pub product: Option<String>,
    pub machine_identifier: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MetadataResponse {
    pub media_container: MetadataContainer,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MetadataContainer {
    #[serde(default)]
    pub metadata: Vec<MetadataItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MetadataItem {
    #[serde(default)]
    pub guid: Vec<Guid>,
}

#[derive(Debug, Deserialize)]
pub struct Guid {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LibrariesResponse {
    pub media_container: LibrariesContainer,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LibrariesContainer {
    #[serde(default)]
    pub directory: Vec<Library>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Library {
    pub title: String,
    #[serde(rename = "type")]
    pub collection_type: String,
    pub uuid: String,
    pub key: String,
}

// ==================== History XML Structures (Fixed for quick-xml) ====================

#[derive(Debug, Deserialize)]
#[serde(rename = "MediaContainer")]
struct HistoryMediaContainer {
    #[serde(rename = "Track", default)]
    tracks: Vec<HistoryTrack>,
}

#[derive(Debug, Deserialize)]
struct HistoryTrack {
    #[serde(rename = "@type")]
    media_type: Option<String>,
    
    #[serde(rename = "@viewedAt")]
    viewed_at: Option<u64>,
    
    #[serde(rename = "@historyKey")]
    history_key: Option<String>,
    
    #[serde(rename = "@title")]
    title: Option<String>,
    
    #[serde(rename = "@grandparentTitle")]
    artist: Option<String>,
    
    #[serde(rename = "@parentTitle")]
    album: Option<String>,
    
    #[serde(rename = "@originalTitle")]
    track_artist: Option<String>,
    
    #[serde(rename = "@duration")]
    duration: Option<u64>,

    #[serde(rename = "@ratingKey")]
    rating_key: Option<String>,

    #[serde(rename = "@parentRatingKey")]
    parent_rating_key: Option<String>,

    #[serde(rename = "@grandparentRatingKey")]
    grandparent_rating_key: Option<String>,
}

// ==================== MusicBrainz Cache ====================

#[derive(Clone)]
struct MBIDCache {
    cache: HashMap<String, Option<String>>,
    max_size: usize,
}

impl MBIDCache {
    fn new(max_size: usize) -> Self {
        Self {
            cache: HashMap::new(),
            max_size,
        }
    }

    fn get(&self, key: &str) -> Option<&Option<String>> {
        self.cache.get(key)
    }

    fn set(&mut self, key: String, value: Option<String>) {
        if self.cache.len() >= self.max_size {
            // Simple eviction: remove oldest (first) entry
            if let Some(first_key) = self.cache.keys().next().cloned() {
                self.cache.remove(&first_key);
            }
        }
        self.cache.insert(key, value);
    }
}

// ==================== Session Tracking ====================

#[derive(Debug, Clone)]
struct SessionState {
    session_key: String,
    rating_key: String,
    position: u64,
    duration: u64,
    started_at: u64,
    last_seen: u64,
    scrobbled: bool,
}

// ==================== Filter Configuration ====================

#[derive(Debug, Clone)]
pub struct PlexFilters {
    pub users_allow: Vec<String>,
    pub users_block: Vec<String>,
    pub devices_allow: Vec<String>,
    pub devices_block: Vec<String>,
    pub libraries_allow: Vec<String>,
    pub libraries_block: Vec<String>,
}

impl Default for PlexFilters {
    fn default() -> Self {
        Self {
            users_allow: Vec::new(),
            users_block: Vec::new(),
            devices_allow: Vec::new(),
            devices_block: Vec::new(),
            libraries_allow: Vec::new(),
            libraries_block: Vec::new(),
        }
    }
}

// ==================== Main Plex Source ====================

pub struct PlexSource {
    url: String,
    token: String,
    client: Client,
    filters: PlexFilters,
    mbid_cache: MBIDCache,
    session_states: HashMap<String, SessionState>,
    libraries: Vec<Library>,
}

/// Result from fetching sessions - separates active playing vs ready to scrobble
#[derive(Debug, Default)]
pub struct SessionResult {
    pub now_playing: Vec<Play>,      // Currently playing, send as "playing_now"
    pub ready_to_scrobble: Vec<Play>, // Met threshold, send as scrobble
}

impl PlexSource {
    pub fn new(url: String, token: String) -> Self {
        Self::with_filters(url, token, PlexFilters::default())
    }

    pub fn with_filters(url: String, token: String, filters: PlexFilters) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            token,
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            filters,
            mbid_cache: MBIDCache::new(1000),
            session_states: HashMap::new(),
            libraries: Vec::new(),
        }
    }

    /// Initialize by fetching library information
    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.libraries = self.fetch_libraries().await?;
        info!("Plex: Loaded {} libraries", self.libraries.len());
        Ok(())
    }

    /// Fetch all libraries from Plex
    async fn fetch_libraries(&self) -> Result<Vec<Library>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/library/sections", self.url);
        debug!("Fetching Plex libraries from: {}", endpoint);

        let resp = self.client.get(&endpoint)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(format!("Plex API error: {}", resp.status()).into());
        }

        let lib_response: LibrariesResponse = resp.json().await?;
        Ok(lib_response.media_container.directory)
    }

    /// Fetch active sessions (real-time monitoring)
    pub async fn fetch_sessions(&mut self) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/status/sessions", self.url);
        debug!("Fetching Plex sessions from: {}", endpoint);

        let resp = self.client.get(&endpoint)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(format!("Plex API error: {}", resp.status()).into());
        }

        // Get the response text first for debugging
        let text = resp.text().await?;
        debug!("Plex sessions response: {}", &text[..text.len().min(500)]);

        // Parse the JSON, handling empty/malformed responses
        let sessions_response: SessionsResponse = match serde_json::from_str(&text) {
            Ok(parsed) => parsed,
            Err(e) => {
                // If parsing fails, it might be an empty response - that's OK
                debug!("Failed to parse sessions response (might be empty): {}", e);
                return Ok(Vec::new());
            }
        };

        let mut plays = Vec::new();

        for session in sessions_response.media_container.metadata {
            // Validate session
            if let Some(reason) = self.validate_session(&session) {
                debug!("Skipping session: {}", reason);
                continue;
            }

            // Check if this is a music track
            if session.media_type.as_deref() != Some("track") {
                continue;
            }

            // Track session state for scrobbling
            if let Some(play) = self.process_session(session).await {
                plays.push(play);
            }
        }

        Ok(plays)
    }

    /// Fetch historical plays from Plex (for offline sync) - WITH MUSICBRAINZ METADATA
    async fn fetch_history_plays(&mut self, min_timestamp: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/status/sessions/history/all", self.url);
        debug!("Fetching Plex history (since {}) from: {}", min_timestamp, endpoint);

        let resp = self.client.get(&endpoint)
            .query(&[("sort", "viewedAt:desc"), ("limit", "200")])
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/xml")
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_else(|_| "<unable to read body>".to_string());
            error!("Plex History API error: {} - Body: {}", status, &body[..body.len().min(500)]);
            return Err(format!("Plex History API error: {}", status).into());
        }

        let text = resp.text().await?;
        debug!("History response (first 1000 chars): {}", &text[..text.len().min(1000)]);

        // FIXED: quick-xml with @ prefix for attributes
        let container: HistoryMediaContainer = match quick_xml::de::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to parse history XML: {} - Raw XML: {}", e, &text[..text.len().min(500)]);
                return Ok(Vec::new());
            }
        };

        let mut plays = Vec::new();
        for track in container.tracks {
            // Skip non-track entries (shouldn't happen, but be safe)
            if track.media_type.as_deref() != Some("track") {
                continue;
            }

            let viewed_at = match track.viewed_at {
                Some(ts) => ts,
                None => {
                    debug!("Skipping history entry with no viewedAt");
                    continue;
                }
            };

            let seven_days_ago = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .saturating_sub(7 * 86400);

            if viewed_at < seven_days_ago {
                debug!("Skipping very old play from {}", viewed_at);
                continue;
            }

            let history_key = track.history_key.unwrap_or_else(|| format!("hist-{}", viewed_at));

            // Build Play object
            let (artists, _) = if let Some(track_artist) = &track.track_artist {
                (vec![track_artist.clone()], track.artist.clone())
            } else {
                (track.artist.as_ref().map(|a| vec![a.clone()]).unwrap_or_default(), track.artist.clone())
            };

            // **FIX**: Fetch MusicBrainz IDs for historical plays
            let mbid_recording = if let Some(rk) = &track.rating_key {
                self.get_musicbrainz_id(rk).await
            } else { None };
            
            let mbid_release = if let Some(pk) = &track.parent_rating_key {
                self.get_musicbrainz_id(pk).await
            } else { None };
            
            let mbid_artist = if let Some(gk) = &track.grandparent_rating_key {
                self.get_musicbrainz_id(gk).await
            } else { None };

            let play = Play {
                title: track.title.unwrap_or_else(|| "Unknown".to_string()),
                artist: artists.first().cloned().unwrap_or_else(|| "Unknown".to_string()),
                artists: if artists.is_empty() { None } else { Some(artists) },
                album: track.album,
                timestamp: viewed_at,
                duration: track.duration.map(|d| d / 1000), // Convert ms to seconds
                track_number: None,
                mbid_artist: mbid_artist.map(|id| vec![id]),
                mbid_release,
                mbid_recording,
                mbid_release_group: None,
                source_id: format!("plex-hist-{}", history_key),
                source_name: "Plex".to_string(),
            };

            plays.push(play);
        }

        info!("Parsed {} track(s) from Plex history", plays.len());
        Ok(plays)
    }


    /// Fetch active sessions with separate now_playing and scrobble results
    pub async fn fetch_sessions_extended(&mut self, last_checked: Option<u64>) -> Result<SessionResult, Box<dyn std::error::Error>> {
        let mut result = SessionResult::default();

        // 1. Fetch Active Sessions (Current Logic)
        let endpoint = format!("{}/status/sessions", self.url);
        debug!("Fetching Plex sessions from: {}", endpoint);

        let resp = self.client.get(&endpoint)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status().is_success() {
            let text = resp.text().await?;
            // Restore debug logging
            debug!("Plex sessions response: {}", &text[..text.len().min(1000)]);

            let sessions_response: SessionsResponse = match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(e) => {
                    debug!("Failed to parse sessions JSON: {}", e);
                    // Don't just return empty, maybe log the error
                    SessionsResponse { media_container: SessionsContainer { metadata: vec![] } }
                }
            };

            for session in sessions_response.media_container.metadata {
                // Debug each session to see why it might be skipped
                debug!("Checking session: {:?}", session.title);

                if let Some(reason) = self.validate_session(&session) {
                    debug!("Skipping session: {}", reason);
                    continue;
                }

                if session.media_type.as_deref() != Some("track") {
                    debug!("Skipping non-track media type: {:?}", session.media_type);
                    continue;
                }

                let is_playing = session.player.as_ref()
                    .and_then(|p| p.state.as_deref())
                    .map(|s| s == "playing")
                    .unwrap_or(false);

                debug!("Session state: is_playing={}", is_playing);

                if let Some((play, is_scrobble)) = self.process_session_extended(session, is_playing).await {
                    debug!("Processed session -> Play: {}, is_scrobble: {}", play.title, is_scrobble);
                    if is_scrobble {
                        result.ready_to_scrobble.push(play);
                    } else if is_playing {
                        result.now_playing.push(play);
                    }
                } else {
                    debug!("process_session_extended returned None");
                }
            }
        } else {
            debug!("Plex sessions request failed: {}", resp.status());
        }

        // 2. Fetch History (Offline Sync) - now ignores last_checked for 7-day window
        debug!("Fetching Plex history (7-day window)");
        match self.fetch_history_plays(0).await {
            Ok(history_plays) => {
                if !history_plays.is_empty() {
                    info!("Found {} historical plays from Plex", history_plays.len());
                    result.ready_to_scrobble.extend(history_plays);
                }
            }
            Err(e) => {
                error!("Failed to fetch Plex history: {}", e);
            }
        }

        Ok(result)
    }

    /// Process session and return (Play, is_ready_to_scrobble)
    async fn process_session_extended(&mut self, session: SessionMetadata, is_playing: bool) -> Option<(Play, bool)> {
        let rating_key = session.rating_key.as_ref()?;
        let session_key = session.session_key.as_ref()?;
        let duration = session.duration?;
        let view_offset = session.view_offset.unwrap_or(0);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Check if track changed
        let track_changed = self.session_states.get(session_key)
            .map(|s| s.rating_key != *rating_key)
            .unwrap_or(false);

        if track_changed {
            debug!("Track changed in session {}, resetting", session_key);
            self.session_states.remove(session_key);
        }

        let (should_scrobble, needs_now_playing) = {
            let state = self.session_states.entry(session_key.clone()).or_insert_with(|| {
                SessionState {
                    session_key: session_key.clone(),
                    rating_key: rating_key.clone(),
                    position: view_offset,
                    duration,
                    started_at: now,
                    last_seen: now,
                    scrobbled: false,
                }
            });

            state.position = view_offset;
            state.last_seen = now;

            let progress_pct = (view_offset as f64 / duration as f64) * 100.0;
            let progress_time = view_offset / 1000;

            let ready = !state.scrobbled && (progress_pct >= 50.0 || progress_time >= 240);
            let needs_np = is_playing && !state.scrobbled; // Send now_playing while not yet scrobbled

            (ready, needs_np)
        };

        // Build the Play object
        let (artists, _album_artist) = if let Some(track_artist) = &session.original_title {
            (vec![track_artist.clone()], session.grandparent_title.clone())
        } else {
            (session.grandparent_title.as_ref().map(|a| vec![a.clone()]).unwrap_or_default(), session.grandparent_title.clone())
        };

        let mbid_recording = self.get_musicbrainz_id(rating_key).await;
        let mbid_release = if let Some(pk) = &session.parent_rating_key {
            self.get_musicbrainz_id(pk).await
        } else { None };
        let mbid_artist = if let Some(gk) = &session.grandparent_rating_key {
            self.get_musicbrainz_id(gk).await
        } else { None };

        let play = Play {
            title: session.title.unwrap_or_default(),
            artist: artists.first().cloned().unwrap_or_else(|| "Unknown".to_string()),
            artists: if artists.is_empty() { None } else { Some(artists) },
            album: session.parent_title,
            timestamp: now,
            duration: Some(duration / 1000),
            track_number: None,
            mbid_artist: mbid_artist.map(|id| vec![id]),
            mbid_release,
            mbid_release_group: None,
            mbid_recording,
            source_id: format!("plex-session-{}-{}", session_key, now),
            source_name: "Plex".to_string(),
        };

        if should_scrobble {
            if let Some(state) = self.session_states.get_mut(session_key) {
                state.scrobbled = true;
            }
            Some((play, true))
        } else if needs_now_playing {
            Some((play, false))
        } else {
            None
        }
    }


    /// Validate session against filters
    fn validate_session(&self, session: &SessionMetadata) -> Option<String> {
        let user = session.user.as_ref()
            .and_then(|u| u.title.as_deref())
            .unwrap_or("unknown")
            .to_lowercase();

        // User filters
        if !self.filters.users_allow.is_empty() && !self.filters.users_allow.contains(&user) {
            return Some(format!("User '{}' not in allow list", user));
        }
        if self.filters.users_block.contains(&user) {
            return Some(format!("User '{}' in block list", user));
        }

        // Device filters
        if let Some(player) = &session.player {
            let device = format!("{} {}",
                                 player.product.as_deref().unwrap_or(""),
                                 player.title.as_deref().unwrap_or("")
            ).to_lowercase();

            if !self.filters.devices_allow.is_empty()
                && !self.filters.devices_allow.iter().any(|d| device.contains(d)) {
                return Some(format!("Device '{}' not in allow list", device));
            }
            if self.filters.devices_block.iter().any(|d| device.contains(d)) {
                return Some(format!("Device '{}' in block list", device));
            }
        }

        // Library filters
        if let Some(library) = &session.library_section_title {
            let lib_lower = library.to_lowercase();

            if !self.filters.libraries_allow.is_empty()
                && !self.filters.libraries_allow.contains(&lib_lower) {
                return Some(format!("Library '{}' not in allow list", library));
            }
            if self.filters.libraries_block.contains(&lib_lower) {
                return Some(format!("Library '{}' in block list", library));
            }

            // Check if library is a music library
            if !self.is_music_library(library) {
                return Some(format!("Library '{}' is not a music library", library));
            }
        }

        None
    }

    /// Check if library is a music library
    fn is_music_library(&self, library_name: &str) -> bool {
        self.libraries.iter()
            .any(|lib| lib.title == library_name && lib.collection_type == "artist")
    }

    /// Process a session and convert to Play
    async fn process_session(&mut self, session: SessionMetadata) -> Option<Play> {
        let rating_key = session.rating_key.as_ref()?;
        let session_key = session.session_key.as_ref()?;
        let duration = session.duration?;
        let view_offset = session.view_offset.unwrap_or(0);

        // Check player state
        let _is_playing = session.player.as_ref()
            .and_then(|p| p.state.as_deref())
            .map(|s| s == "playing")
            .unwrap_or(false);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Check if track changed within the same session (new song started)
        let track_changed = self.session_states.get(session_key)
            .map(|s| s.rating_key != *rating_key)
            .unwrap_or(false);

        if track_changed {
            debug!("Track changed in session {} (new rating_key: {}), resetting state", session_key, rating_key);
            self.session_states.remove(session_key);
        }

        // Update or create session state
        let should_scrobble = {
            let state = self.session_states.entry(session_key.clone()).or_insert_with(|| {
                SessionState {
                    session_key: session_key.clone(),
                    rating_key: rating_key.clone(),
                    position: view_offset,
                    duration,
                    started_at: now,
                    last_seen: now,
                    scrobbled: false,
                }
            });

            state.position = view_offset;
            state.last_seen = now;

            // Scrobble if played >50% or >4 minutes
            let progress_pct = (view_offset as f64 / duration as f64) * 100.0;
            let progress_time = view_offset / 1000;

            !state.scrobbled && (progress_pct >= 50.0 || progress_time >= 240)
        };

        if !should_scrobble {
            return None;
        }

        // Mark as scrobbled
        if let Some(state) = self.session_states.get_mut(session_key) {
            state.scrobbled = true;
        }

        // Extract artist information (track artist vs album artist)
        let (artists, _album_artist) = if let Some(track_artist) = &session.original_title {
            // originalTitle is the track artist, grandparentTitle is album artist
            (
                vec![track_artist.clone()],
                session.grandparent_title.clone()
            )
        } else {
            // No separate track artist, use grandparentTitle
            (
                session.grandparent_title.as_ref().map(|a| vec![a.clone()]).unwrap_or_default(),
                session.grandparent_title.clone()
            )
        };

        // Fetch MusicBrainz IDs
        let mbid_recording = self.get_musicbrainz_id(rating_key).await;
        let mbid_release = if let Some(parent_key) = &session.parent_rating_key {
            self.get_musicbrainz_id(parent_key).await
        } else {
            None
        };
        let mbid_artist = if let Some(grandparent_key) = &session.grandparent_rating_key {
            self.get_musicbrainz_id(grandparent_key).await
        } else {
            None
        };

        let timestamp = now;
        let source_id = format!("plex-session-{}-{}", session_key, timestamp);

        Some(Play {
            title: session.title.unwrap_or_default(),
            artist: artists.first().cloned().unwrap_or_else(|| "Unknown".to_string()),
            artists: if artists.is_empty() { None } else { Some(artists) },
            album: session.parent_title,
            timestamp,
            duration: Some(duration / 1000), // Convert ms to seconds
            track_number: None,
            mbid_artist: mbid_artist.map(|id| vec![id]),
            mbid_release,
            mbid_release_group: None,
            mbid_recording,
            source_id,
            source_name: "Plex".to_string(),
        })
    }

    /// Fetch MusicBrainz ID for a Plex rating key
    async fn get_musicbrainz_id(&mut self, rating_key: &str) -> Option<String> {
        // Check cache first
        if let Some(cached) = self.mbid_cache.get(rating_key) {
            return cached.clone();
        }

        let endpoint = format!("{}/library/metadata/{}", self.url, rating_key);
        debug!("Fetching MusicBrainz ID for rating key: {}", rating_key);

        let result = async {
            let resp = self.client.get(&endpoint)
                .header("X-Plex-Token", &self.token)
                .header("Accept", "application/json")
                .timeout(Duration::from_secs(5))
                .send()
                .await?;

            if !resp.status().is_success() {
                return Ok::<Option<String>, Box<dyn std::error::Error>>(None);
            }

            let metadata: MetadataResponse = resp.json().await?;

            for item in metadata.media_container.metadata {
                for guid in item.guid {
                    if let Some(mbid) = guid.id.strip_prefix("mbid://") {
                        return Ok(Some(mbid.to_string()));
                    }
                }
            }

            Ok(None)
        }.await;

        match result {
            Ok(mbid) => {
                self.mbid_cache.set(rating_key.to_string(), mbid.clone());
                mbid
            }
            Err(e) => {
                warn!("Failed to fetch MBID for {}: {}", rating_key, e);
                self.mbid_cache.set(rating_key.to_string(), None);
                None
            }
        }
    }

    /// Fetch historical plays (backward compatibility)
    pub async fn fetch_history(&self) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/status/sessions/history/all", self.url);
        debug!("Fetching Plex history from: {}", endpoint);

        let resp = self.client.get(&endpoint)
            .query(&[("sort", "viewedAt:desc"), ("limit", "200")])
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/xml")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(format!("Plex API error: {}", resp.status()).into());
        }

        let text = resp.text().await?;
        // Parse XML (keeping your existing XML parsing logic)
        // ... (include your existing XML parsing code)

        Ok(Vec::new()) // Placeholder
    }

    /// Clean up old session states
    pub fn cleanup_sessions(&mut self, max_age_seconds: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.session_states.retain(|_, state| {
            now - state.last_seen < max_age_seconds
        });
    }
}

#[async_trait]
impl MusicSource for PlexSource {
    fn name(&self) -> &str {
        "Plex"
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    async fn fetch_new_plays(&mut self, _last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        // Use session monitoring for real-time tracking
        self.fetch_sessions().await
    }
}
