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

/// Cache only successful MBID lookups, not failures
#[derive(Clone)]
struct MBIDCache {
    cache: HashMap<String, String>, // Only stores successful MBIDs
    max_size: usize,
}

impl MBIDCache {
    fn new(max_size: usize) -> Self {
        Self { cache: HashMap::new(), max_size }
    }
    
    fn get(&self, key: &str) -> Option<String> { 
        self.cache.get(key).cloned()
    }
    
    fn set(&mut self, key: String, value: String) {
        if self.cache.len() >= self.max_size {
            if let Some(first_key) = self.cache.keys().next().cloned() {
                self.cache.remove(&first_key);
            }
        }
        self.cache.insert(key, value);
    }
}

// ==================== MBID Fetch Result ====================

#[derive(Debug)]
struct MBIDFetchResult {
    mbid: Option<String>,
    attempted: bool,
    error: Option<String>,
}

impl MBIDFetchResult {
    fn success(mbid: String) -> Self {
        Self { mbid: Some(mbid), attempted: true, error: None }
    }
    
    fn not_found() -> Self {
        Self { mbid: None, attempted: true, error: Some("No MBID in Plex metadata".to_string()) }
    }
    
    fn error(err: String) -> Self {
        Self { mbid: None, attempted: true, error: Some(err) }
    }
    
    fn skipped() -> Self {
        Self { mbid: None, attempted: false, error: None }
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
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            filters,
            mbid_cache: MBIDCache::new(2000), // Increased cache size
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
        let resp = self.client.get(&endpoint)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send().await?;
        if !resp.status().is_success() { 
            return Err(format!("Plex API error: {}", resp.status()).into()); 
        }
        let lib_response: LibrariesResponse = resp.json().await?;
        Ok(lib_response.media_container.directory)
    }

    // ==================== Core Resolution Logic ====================

    /// Central method to convert a raw Plex track into a Play object with ENFORCED MBID resolution.
    /// This method deliberately fetches all available MBIDs sequentially and logs the results.
    async fn resolve_play(&mut self, track: PlexTrack, source_id_suffix: &str) -> Play {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        info!("Resolving track: '{}' by '{}' (rating_key: {})", 
              track.title, track.artist, track.rating_key);

        // 1. Deliberately fetch ALL MBIDs sequentially with retry logic
        // Sequential to avoid borrow checker issues with mutable self
        let recording_result = self.fetch_musicbrainz_id_with_retry(&track.rating_key, "recording", 3).await;
        
        let release_result = if let Some(pk) = &track.parent_rating_key {
            self.fetch_musicbrainz_id_with_retry(pk, "release", 3).await
        } else {
            MBIDFetchResult::skipped()
        };
        
        let artist_result = if let Some(gk) = &track.grandparent_rating_key {
            self.fetch_musicbrainz_id_with_retry(gk, "artist", 3).await
        } else {
            MBIDFetchResult::skipped()
        };

        // 2. Log MBID fetch results
        self.log_mbid_results(&track.title, &track.artist, &recording_result, &release_result, &artist_result);

        let mbid_recording = recording_result.mbid;
        let mbid_release = release_result.mbid;
        let mbid_artist = artist_result.mbid;

        // 3. Resolve Artists
        let (artists, _) = if let Some(ta) = &track.track_artist {
            (vec![ta.clone()], track.artist.clone())
        } else {
            (vec![track.artist.clone()], track.artist.clone())
        };

        // 4. Construct Play with all available metadata
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

    /// Fetch MusicBrainz ID with retry logic and exponential backoff
    async fn fetch_musicbrainz_id_with_retry(&mut self, rating_key: &str, entity_type: &str, max_retries: u32) -> MBIDFetchResult {
        // Check cache first (only successful results are cached)
        if let Some(cached_mbid) = self.mbid_cache.get(rating_key) {
            debug!("Cache hit for {} {}: {}", entity_type, rating_key, cached_mbid);
            return MBIDFetchResult::success(cached_mbid);
        }

        let endpoint = format!("{}/library/metadata/{}", self.url, rating_key);
        let mut last_error = String::new();
        
        for attempt in 1..=max_retries {
            let delay_ms = 100 * (2_u64.pow(attempt - 1)); // Exponential backoff: 100ms, 200ms, 400ms
            
            if attempt > 1 {
                debug!("Retry #{} for {} {} after {}ms", attempt, entity_type, rating_key, delay_ms);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let result: Result<Option<String>, Box<dyn std::error::Error>> = async {
                let resp = self.client.get(&endpoint)
                    .header("X-Plex-Token", &self.token)
                    .header("Accept", "application/json")
                    .timeout(Duration::from_secs(8))
                    .send().await?;

                if !resp.status().is_success() { 
                    return Err(format!("HTTP {}", resp.status()).into());
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
                Ok(Some(mbid)) => {
                    debug!("Successfully fetched {} MBID for {}: {}", entity_type, rating_key, mbid);
                    // Cache only successful results
                    self.mbid_cache.set(rating_key.to_string(), mbid.clone());
                    return MBIDFetchResult::success(mbid);
                }
                Ok(None) => {
                    // MBID not present in Plex metadata - not an error, just missing data
                    debug!("No {} MBID found in Plex metadata for {}", entity_type, rating_key);
                    return MBIDFetchResult::not_found();
                }
                Err(e) => {
                    last_error = format!("{}", e);
                    warn!("Attempt {}/{} failed for {} {}: {}", attempt, max_retries, entity_type, rating_key, e);
                }
            }
        }
        
        error!("Failed to fetch {} MBID for {} after {} attempts: {}", 
               entity_type, rating_key, max_retries, last_error);
        MBIDFetchResult::error(last_error)
    }

    /// Log comprehensive MBID fetch results for a track
    fn log_mbid_results(&self, title: &str, artist: &str, recording: &MBIDFetchResult, release: &MBIDFetchResult, artist_mbid: &MBIDFetchResult) {
        let recording_status = if let Some(ref mbid) = recording.mbid {
            format!("✓ {}", mbid)
        } else if recording.attempted {
            format!("✗ {}", recording.error.as_ref().unwrap_or(&"unknown".to_string()))
        } else {
            "⊘ skipped".to_string()
        };
        
        let release_status = if let Some(ref mbid) = release.mbid {
            format!("✓ {}", mbid)
        } else if release.attempted {
            format!("✗ {}", release.error.as_ref().unwrap_or(&"unknown".to_string()))
        } else {
            "⊘ skipped".to_string()
        };
        
        let artist_status = if let Some(ref mbid) = artist_mbid.mbid {
            format!("✓ {}", mbid)
        } else if artist_mbid.attempted {
            format!("✗ {}", artist_mbid.error.as_ref().unwrap_or(&"unknown".to_string()))
        } else {
            "⊘ skipped".to_string()
        };

        let total_found = [recording.mbid.is_some(), release.mbid.is_some(), artist_mbid.mbid.is_some()]
            .iter()
            .filter(|&&x| x)
            .count();
        
        info!("MBID enrichment for '{}' by '{}': [{}/3] Recording: {} | Release: {} | Artist: {}",
              title, artist, total_found, recording_status, release_status, artist_status);
    }

    // ==================== Session Processing ====================

    pub async fn fetch_sessions_extended(&mut self, _last_checked: Option<u64>) -> Result<SessionResult, Box<dyn std::error::Error>> {
        let mut result = SessionResult::default();

        // A. Live Sessions
        let endpoint = format!("{}/status/sessions", self.url);
        if let Ok(resp) = self.client.get(&endpoint)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send().await 
        {
            if resp.status().is_success() {
                if let Ok(sessions) = resp.json::<SessionsResponse>().await {
                    info!("Processing {} active Plex sessions", sessions.media_container.metadata.len());
                    
                    for session in sessions.media_container.metadata {
                        if let Some(reason) = self.validate_session(&session) {
                            debug!("Skipping session: {}", reason);
                            continue;
                        }
                        if session.media_type.as_deref() != Some("track") { continue; }

                        let is_playing = session.player.as_ref()
                            .map(|p| p.state.as_deref() == Some("playing"))
                            .unwrap_or(false);
                        
                        if let Some(track) = PlexTrack::from_session(session) {
                            self.process_live_track(track, is_playing, &mut result).await;
                        }
                    }
                }
            }
        }

        // B. History Sync
        if let Ok(history) = self.fetch_history_plays(0).await {
            info!("Fetched {} tracks from Plex history", history.len());
            result.ready_to_scrobble.extend(history);
        }

        info!("Session fetch complete: {} now playing, {} ready to scrobble", 
              result.now_playing.len(), result.ready_to_scrobble.len());
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
                debug!("Track changed in session {}: {} -> {}", session_key, state.rating_key, rating_key);
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
            info!("Track ready for scrobble: '{}' by '{}'", track.title, track.artist);
            if let Some(state) = self.session_states.get_mut(&session_key) { 
                state.scrobbled = true; 
            }
            let play = self.resolve_play(track, &format!("scrobble-{}", now)).await;
            result.ready_to_scrobble.push(play);
        } else if needs_np {
            info!("Track now playing: '{}' by '{}'", track.title, track.artist);
            let play = self.resolve_play(track, "np").await;
            result.now_playing.push(play);
        }
    }

    // ==================== History Processing ====================

    async fn fetch_history_plays(&mut self, _min_timestamp: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/status/sessions/history/all", self.url);
        let resp = self.client.get(&endpoint)
            .query(&[("sort", "viewedAt:desc"), ("limit", "50")])
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/xml")
            .send().await?;

        if !resp.status().is_success() { 
            warn!("History fetch failed: HTTP {}", resp.status());
            return Ok(Vec::new()); 
        }
        
        let text = resp.text().await?;
        
        let container: HistoryMediaContainer = match quick_xml::de::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to parse history XML: {}", e);
                return Ok(Vec::new());
            }
        };

        let mut plays = Vec::new();
        let seven_days_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(7 * 86400);

        info!("Processing {} history tracks from Plex", container.tracks.len());
        
        for h_track in container.tracks {
            if h_track.media_type.as_deref() != Some("track") { continue; }
            if let Some(ts) = h_track.viewed_at {
                if ts < seven_days_ago { 
                    debug!("Skipping old history track (viewed_at: {})", ts);
                    continue; 
                }
            }

            if let Some(track) = PlexTrack::from_history(h_track) {
                let play = self.resolve_play(track, "hist").await;
                plays.push(play);
            }
        }
        
        Ok(plays)
    }

    // ==================== Validation ====================

    fn validate_session(&self, session: &SessionMetadata) -> Option<String> {
        let user = session.user.as_ref()
            .and_then(|u| u.title.as_deref())
            .unwrap_or("unknown")
            .to_lowercase();
            
        if !self.filters.users_allow.is_empty() && !self.filters.users_allow.contains(&user) { 
            return Some(format!("User not in allow list: {}", user)); 
        }
        if self.filters.users_block.contains(&user) { 
            return Some(format!("User in block list: {}", user)); 
        }
        
        if let Some(lib) = &session.library_section_title {
            let lib_lower = lib.to_lowercase();
            if !self.filters.libraries_allow.is_empty() && !self.filters.libraries_allow.contains(&lib_lower) { 
                return Some(format!("Library not in allow list: {}", lib)); 
            }
            if !self.libraries.iter().any(|l| l.title == *lib && l.collection_type == "artist") { 
                return Some(format!("Not a music library: {}", lib)); 
            }
        }
        None
    }

    pub fn cleanup_sessions(&mut self, max_age_seconds: u64) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let before_count = self.session_states.len();
        self.session_states.retain(|_, state| now - state.last_seen < max_age_seconds);
        let removed = before_count - self.session_states.len();
        if removed > 0 {
            debug!("Cleaned up {} stale sessions", removed);
        }
    }

    // ==================== Wrapper for trait compliance ====================

    pub async fn fetch_sessions(&mut self) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let result = self.fetch_sessions_extended(None).await?;
        Ok(result.ready_to_scrobble)
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
