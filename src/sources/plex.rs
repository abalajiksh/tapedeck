use serde::{Deserialize, Deserializer, Serialize};
use reqwest::Client;
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::models::Play;
use crate::sources::MusicSource;
use async_trait::async_trait;

// ==================== Deserialization Helpers ====================

fn deserialize_u64_from_string<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s {
        Some(s) => s.parse::<u64>().map(Some).map_err(serde::de::Error::custom),
        None => Ok(None),
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

fn deserialize_f64_from_string<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s {
        Some(s) => s.parse::<f64>().map(Some).map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

// ==================== Plex API Response Structures ====================

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
    pub original_title: Option<String>, // Track artist when album artist exists

    pub rating_key: Option<String>,
    pub parent_rating_key: Option<String>,
    pub grandparent_rating_key: Option<String>,

    pub library_section_title: Option<String>,
    pub library_section_id: Option<String>,

    #[serde(deserialize_with = "deserialize_u64_from_string", default)]
    pub duration: Option<u64>,

    #[serde(deserialize_with = "deserialize_u64_from_string", default)]
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

        let sessions_response: SessionsResponse = resp.json().await?;
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
        let is_playing = session.player.as_ref()
            .and_then(|p| p.state.as_deref())
            .map(|s| s == "playing")
            .unwrap_or(false);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

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
        let (artists, album_artist) = if let Some(track_artist) = &session.original_title {
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

    async fn fetch_new_plays(&mut self, last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        // Use session monitoring for real-time tracking
        self.fetch_sessions().await
    }
}
