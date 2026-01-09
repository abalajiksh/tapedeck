use serde::{Deserialize, Deserializer};
use reqwest::Client;
use log::{debug, info, warn, error};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::models::Play;
use crate::sources::MusicSource;
use async_trait::async_trait;

// ==================== Deserialization Helpers ====================

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
        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> where E: de::Error { Ok(Some(value)) }
        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> where E: de::Error { Ok(Some(value as u64)) }
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> where E: de::Error {
            value.parse::<u64>().map(Some).map_err(de::Error::custom)
        }
        fn visit_none<E>(self) -> Result<Self::Value, E> where E: de::Error { Ok(None) }
        fn visit_unit<E>(self) -> Result<Self::Value, E> where E: de::Error { Ok(None) }
    }
    deserializer.deserialize_any(U64Visitor)
}

// ==================== Unified Track Structure ====================

/// A unified representation of a Plex track, whether from Session (JSON) or History (XML).
/// This intermediate struct holds all the keys needed to fetch metadata.
#[derive(Debug)]
struct PlexTrack {
    title: String,
    artist: String, // Album Artist
    track_artist: Option<String>,
    album: Option<String>,
    duration: Option<u64>,
    view_offset: Option<u64>, // Only relevant for sessions
    rating_key: String,
    parent_rating_key: Option<String>,
    grandparent_rating_key: Option<String>,
    session_key: Option<String>, // Only for sessions
    viewed_at: Option<u64>,      // Only for history
}

impl PlexTrack {
    fn from_session(s: SessionMetadata) -> Option<Self> {
        Some(Self {
            title: s.title?,
            artist: s.grandparent_title.unwrap_or_else(|| "Unknown".to_string()),
            track_artist: s.original_title,
            album: s.parent_title,
            duration: s.duration,
            view_offset: s.view_offset,
            rating_key: s.rating_key?,
            parent_rating_key: s.parent_rating_key,
            grandparent_rating_key: s.grandparent_rating_key,
            session_key: s.session_key,
            viewed_at: None,
        })
    }

    fn from_history(h: HistoryTrack) -> Option<Self> {
        Some(Self {
            title: h.title?,
            artist: h.artist.unwrap_or_else(|| "Unknown".to_string()),
            track_artist: h.track_artist,
            album: h.album,
            duration: h.duration,
            view_offset: None,
            rating_key: h.rating_key?,
            parent_rating_key: h.parent_rating_key,
            grandparent_rating_key: h.grandparent_rating_key,
            session_key: None,
            viewed_at: h.viewed_at,
        })
    }
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
    pub state: Option<String>,
    pub title: Option<String>,
    pub product: Option<String>,
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
}

// ==================== History XML Structures ====================

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
        Self { cache: HashMap::new(), max_size }
    }
    fn get(&self, key: &str) -> Option<&Option<String>> { self.cache.get(key) }
    fn set(&mut self, key: String, value: Option<String>) {
        if self.cache.len() >= self.max_size {
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
    rating_key: String,
    last_seen: u64,
    scrobbled: bool,
}

// ==================== Filter Configuration ====================

#[derive(Debug, Clone, Default)]
pub struct PlexFilters {
    pub users_allow: Vec<String>,
    pub users_block: Vec<String>,
    pub devices_allow: Vec<String>,
    pub devices_block: Vec<String>,
    pub libraries_allow: Vec<String>,
    pub libraries_block: Vec<String>,
}

// ==================== Main Plex Source ====================

pub struct PlexSource {
    url: String,
    token: String,
    client: Client,
    filters: PlexFilters,
    mbid_cache: MBIDCache,
    session_states: HashMap<String, SessionState>, // Keyed by session_key
    libraries: Vec<Library>,
}

/// Result from fetching sessions
#[derive(Debug, Default)]
pub struct SessionResult {
    pub now_playing: Vec<Play>,
    pub ready_to_scrobble: Vec<Play>,
}

impl PlexSource {
    pub fn new(url: String, token: String) -> Self {
        Self::with_filters(url, token, PlexFilters::default())
    }

    pub fn with_filters(url: String, token: String, filters: PlexFilters) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            token,
            client: Client::builder().timeout(Duration::from_secs(10)).build().unwrap_or_else(|_| Client::new()),
            filters,
            mbid_cache: MBIDCache::new(1000),
            session_states: HashMap::new(),
            libraries: Vec::new(),
        }
    }

    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.libraries = self.fetch_libraries().await?;
        info!("Plex: Loaded {} libraries", self.libraries.len());
        Ok(())
    }

    async fn fetch_libraries(&self) -> Result<Vec<Library>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/library/sections", self.url);
        let resp = self.client.get(&endpoint).header("X-Plex-Token", &self.token).header("Accept", "application/json").send().await?;
        if !resp.status().is_success() { return Err(format!("Plex API error: {}", resp.status()).into()); }
        let lib_response: LibrariesResponse = resp.json().await?;
        Ok(lib_response.media_container.directory)
    }

    // ==================== Core Resolution Logic ====================

    /// Central method to convert a raw Plex track into a Play object, ENFORCING MBID resolution.
    async fn resolve_play(&mut self, track: PlexTrack, source_id_suffix: &str) -> Play {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        // 1. Resolve MBIDs (Forcefully fetched)
        let mbid_recording = self.get_musicbrainz_id(&track.rating_key).await;
        
        let mbid_release = if let Some(pk) = &track.parent_rating_key {
            self.get_musicbrainz_id(pk).await
        } else { None };
        
        let mbid_artist = if let Some(gk) = &track.grandparent_rating_key {
            self.get_musicbrainz_id(gk).await
        } else { None };

        // 2. Resolve Artists
        let (artists, _) = if let Some(ta) = &track.track_artist {
            (vec![ta.clone()], track.artist.clone())
        } else {
            (vec![track.artist.clone()], track.artist.clone())
        };

        // 3. Construct Play
        let timestamp = track.viewed_at.unwrap_or(now);
        let source_id = format!("plex-{}-{}", track.rating_key, source_id_suffix);

        Play {
            title: track.title,
            artist: artists.first().cloned().unwrap_or_else(|| "Unknown".to_string()),
            artists: if artists.is_empty() { None } else { Some(artists) },
            album: track.album,
            timestamp,
            duration: track.duration.map(|d| d / 1000),
            track_number: None,
            mbid_artist: mbid_artist.map(|id| vec![id]),
            mbid_release,
            mbid_release_group: None,
            mbid_recording,
            source_id,
            source_name: "Plex".to_string(),
        }
    }

    async fn get_musicbrainz_id(&mut self, rating_key: &str) -> Option<String> {
        if let Some(cached) = self.mbid_cache.get(rating_key) {
            return cached.clone();
        }

        let endpoint = format!("{}/library/metadata/{}", self.url, rating_key);
        debug!("Fetching MBID for rating key: {}", rating_key);

        let result = async {
            let resp = self.client.get(&endpoint)
                .header("X-Plex-Token", &self.token)
                .header("Accept", "application/json")
                .timeout(Duration::from_secs(5))
                .send().await?;

            if !resp.status().is_success() { return Ok(None); }
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
                warn!("MBID fetch failed for {}: {}", rating_key, e);
                self.mbid_cache.set(rating_key.to_string(), None);
                None
            }
        }
    }

    // ==================== Session Processing ====================

    pub async fn fetch_sessions_extended(&mut self, _last_checked: Option<u64>) -> Result<SessionResult, Box<dyn std::error::Error>> {
        let mut result = SessionResult::default();

        // A. Live Sessions
        let endpoint = format!("{}/status/sessions", self.url);
        if let Ok(resp) = self.client.get(&endpoint).header("X-Plex-Token", &self.token).header("Accept", "application/json").send().await {
            if resp.status().is_success() {
                if let Ok(sessions) = resp.json::<SessionsResponse>().await {
                    for session in sessions.media_container.metadata {
                        if let Some(reason) = self.validate_session(&session) {
                            debug!("Skipping session: {}", reason);
                            continue;
                        }
                        if session.media_type.as_deref() != Some("track") { continue; }

                        let is_playing = session.player.as_ref().map(|p| p.state.as_deref() == Some("playing")).unwrap_or(false);
                        
                        if let Some(track) = PlexTrack::from_session(session) {
                            self.process_live_track(track, is_playing, &mut result).await;
                        }
                    }
                }
            }
        }

        // B. History Sync
        if let Ok(history) = self.fetch_history_plays(0).await {
            result.ready_to_scrobble.extend(history);
        }

        Ok(result)
    }

    async fn process_live_track(&mut self, track: PlexTrack, is_playing: bool, result: &mut SessionResult) {
        let session_key = track.session_key.clone().unwrap_or_default();
        let rating_key = track.rating_key.clone();
        let duration = track.duration.unwrap_or(0);
        let view_offset = track.view_offset.unwrap_or(0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        // Detect track change
        if let Some(state) = self.session_states.get(&session_key) {
            if state.rating_key != rating_key {
                self.session_states.remove(&session_key);
            }
        }

        let (should_scrobble, needs_np) = {
            let state = self.session_states.entry(session_key.clone()).or_insert_with(|| SessionState {
                rating_key: rating_key.clone(),
                last_seen: now,
                scrobbled: false,
            });
            state.last_seen = now;

            let pct = if duration > 0 { (view_offset as f64 / duration as f64) * 100.0 } else { 0.0 };
            let seconds = view_offset / 1000;
            
            let ready = !state.scrobbled && (pct >= 50.0 || seconds >= 240);
            let np = is_playing && !state.scrobbled;
            (ready, np)
        };

        if should_scrobble {
            if let Some(state) = self.session_states.get_mut(&session_key) { state.scrobbled = true; }
            let play = self.resolve_play(track, &format!("scrobble-{}", now)).await;
            result.ready_to_scrobble.push(play);
        } else if needs_np {
            let play = self.resolve_play(track, "np").await;
            result.now_playing.push(play);
        }
    }

    // ==================== History Processing ====================

    async fn fetch_history_plays(&mut self, _min_timestamp: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/status/sessions/history/all", self.url);
        let resp = self.client.get(&endpoint)
            .query(&[("sort", "viewedAt:desc"), ("limit", "50")]) // Reduced limit for simpler sync
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/xml")
            .send().await?;

        if !resp.status().is_success() { return Ok(Vec::new()); }
        let text = resp.text().await?;
        
        let container: HistoryMediaContainer = match quick_xml::de::from_str(&text) {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };

        let mut plays = Vec::new();
        let seven_days_ago = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs().saturating_sub(7 * 86400);

        for h_track in container.tracks {
            if h_track.media_type.as_deref() != Some("track") { continue; }
            if let Some(ts) = h_track.viewed_at {
                if ts < seven_days_ago { continue; }
            }

            // Convert to intermediate struct
            if let Some(track) = PlexTrack::from_history(h_track) {
                // Resolve with full MBID enrichment
                let play = self.resolve_play(track, "hist").await;
                plays.push(play);
            }
        }
        Ok(plays)
    }

    // ==================== Validation ====================

    fn validate_session(&self, session: &SessionMetadata) -> Option<String> {
        let user = session.user.as_ref().and_then(|u| u.title.as_deref()).unwrap_or("unknown").to_lowercase();
        if !self.filters.users_allow.is_empty() && !self.filters.users_allow.contains(&user) { return Some(format!("User blocked: {}", user)); }
        if self.filters.users_block.contains(&user) { return Some(format!("User blocked: {}", user)); }
        
        if let Some(lib) = &session.library_section_title {
            let lib_lower = lib.to_lowercase();
            if !self.filters.libraries_allow.is_empty() && !self.filters.libraries_allow.contains(&lib_lower) { return Some(format!("Lib blocked: {}", lib)); }
            if !self.libraries.iter().any(|l| l.title == *lib && l.collection_type == "artist") { return Some("Not music lib".to_string()); }
        }
        None
    }

    pub fn cleanup_sessions(&mut self, max_age_seconds: u64) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        self.session_states.retain(|_, state| now - state.last_seen < max_age_seconds);
    }
}

#[async_trait]
impl MusicSource for PlexSource {
    fn name(&self) -> &str { "Plex" }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    async fn fetch_new_plays(&mut self, _last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        self.fetch_sessions().await
    }
}
