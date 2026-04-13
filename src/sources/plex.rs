use serde::{Deserialize, Deserializer};
use reqwest::Client;
use tracing::{debug, info, warn};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::models::{AudioQuality, Play};
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

fn deserialize_i32_flexible<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    struct I32Visitor;
    impl<'de> Visitor<'de> for I32Visitor {
        type Value = Option<i32>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an i32 as either a number or a string")
        }
        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> where E: de::Error { Ok(Some(value as i32)) }
        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> where E: de::Error { Ok(Some(value as i32)) }
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> where E: de::Error {
            value.parse::<i32>().map(Some).map_err(de::Error::custom)
        }
        fn visit_none<E>(self) -> Result<Self::Value, E> where E: de::Error { Ok(None) }
        fn visit_unit<E>(self) -> Result<Self::Value, E> where E: de::Error { Ok(None) }
    }
    deserializer.deserialize_any(I32Visitor)
}

// ==================== Unified Track Structure ====================

#[derive(Debug, Clone)]
pub struct PlexTrack {
    pub title: String,
    pub artist: String,
    pub track_artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<u64>,
    pub view_offset: Option<u64>,
    pub rating_key: String,
    pub parent_rating_key: Option<String>,
    pub grandparent_rating_key: Option<String>,
    pub session_key: Option<String>,
    pub viewed_at: Option<u64>,
    /// Audio quality extracted from Plex Media/Stream metadata
    pub audio_quality: Option<AudioQuality>,
}

impl PlexTrack {
    fn from_session(s: SessionMetadata) -> Option<Self> {
        // Extract audio quality from Media[0].Part[0].Stream[0] (audio streams)
        let audio_quality = Self::extract_quality(&s);

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
            audio_quality,
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
            audio_quality: None, // History API doesn't include media info
        })
    }

    /// Extract audio quality from Plex session metadata.
    ///
    /// Plex returns quality data in:
    ///   - `Media[0].audioCodec`, `Media[0].bitrate`
    ///   - `Media[0].Part[0].Stream[0]` (where streamType=2 is audio)
    ///   - `TranscodeSession` if transcoding is active
    fn extract_quality(session: &SessionMetadata) -> Option<AudioQuality> {
        let media = session.media.as_ref()?.first()?;

        // Find the audio stream (streamType == 2)
        let audio_stream = media.part.as_ref()
            .and_then(|parts| parts.first())
            .and_then(|part| part.stream.as_ref())
            .and_then(|streams| streams.iter().find(|s| s.stream_type == Some(2)));

        let codec = audio_stream
            .and_then(|s| s.codec.clone())
            .or_else(|| media.audio_codec.clone());

        let bitrate = audio_stream
            .and_then(|s| s.bitrate)
            .or(media.bitrate);

        let sample_rate = audio_stream.and_then(|s| s.sampling_rate);
        let bit_depth = audio_stream.and_then(|s| s.bit_depth);
        let channels = audio_stream
            .and_then(|s| s.channels)
            .or(media.audio_channels);

        // Determine format type and lossless status
        let codec_lower = codec.as_deref().unwrap_or("").to_lowercase();
        let is_lossless = matches!(codec_lower.as_str(), "flac" | "alac" | "wav" | "aiff" | "dsd" | "pcm");
        let format_type = if codec_lower == "dsd" { "dsd" } else { "pcm" };

        // Container from the media
        let container = media.container.clone();

        // Check for transcoding
        let (is_transcoded, delivery_codec, delivery_bitrate, transcode_reason) =
            if let Some(ref tc) = session.transcode_session {
                let tc_codec = tc.audio_codec.clone();
                let is_tc = tc_codec.as_deref() != codec.as_deref(); // different codec = transcoded
                let reason = if is_tc {
                    tc.transcode_hw_requested.map(|_| "server_transcode".to_string())
                        .or_else(|| Some("bandwidth".to_string()))
                } else {
                    None
                };
                (Some(is_tc), if is_tc { tc_codec } else { None }, None, reason)
            } else {
                (Some(false), None, None, None)
            };

        // Only return if we have meaningful data
        if codec.is_none() && bitrate.is_none() && sample_rate.is_none() {
            return None;
        }

        let mut quality = AudioQuality {
            format_type: Some(format_type.to_string()),
            codec,
            bitrate,
            sample_rate,
            bit_depth: bit_depth.map(|b| b as i16),
            channels: channels.map(|c| c as i16),
            container,
            is_lossless: Some(is_lossless),
            delivery_codec,
            delivery_bitrate,
            is_transcoded,
            transcode_reason,
            ..Default::default()
        };

        quality.compute_score();
        Some(quality)
    }

    pub fn to_play(&self, source_id_suffix: &str) -> Play {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let timestamp = self.viewed_at.unwrap_or(now);
        let source_id = format!("plex-{}-{}", self.rating_key, source_id_suffix);

        let (artists, _) = if let Some(ta) = &self.track_artist {
            (vec![ta.clone()], self.artist.clone())
        } else {
            (vec![self.artist.clone()], self.artist.clone())
        };

        Play {
            title: self.title.clone(),
            artist: artists.first().cloned().unwrap_or_else(|| "Unknown".to_string()),
            artists: if artists.is_empty() { None } else { Some(artists) },
            album: self.album.clone(),
            timestamp,
            duration: self.duration.map(|d| d / 1000),
            track_number: None,
            mbid_artist: None,
            mbid_release: None,
            mbid_release_group: None,
            mbid_recording: None,
            caa_id: None,
            caa_release_mbid: None,
            source_id,
            source_name: "Plex".to_string(),
        }
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
    /// Media streams with codec/quality info
    #[serde(rename = "Media", default)]
    pub media: Option<Vec<PlexMedia>>,
    /// Present when Plex is transcoding
    #[serde(rename = "TranscodeSession")]
    pub transcode_session: Option<TranscodeSession>,
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

/// Plex Media object containing codec and stream info
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlexMedia {
    #[serde(deserialize_with = "deserialize_i32_flexible", default)]
    pub bitrate: Option<i32>,
    pub audio_codec: Option<String>,
    #[serde(deserialize_with = "deserialize_i32_flexible", default)]
    pub audio_channels: Option<i32>,
    pub container: Option<String>,
    #[serde(rename = "Part", default)]
    pub part: Option<Vec<PlexPart>>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlexPart {
    pub container: Option<String>,
    #[serde(rename = "Stream", default)]
    pub stream: Option<Vec<PlexStream>>,
}

/// Individual stream within a Plex media part.
/// streamType 1 = video, 2 = audio, 3 = subtitle
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlexStream {
    #[serde(deserialize_with = "deserialize_i32_flexible", default)]
    pub stream_type: Option<i32>,
    pub codec: Option<String>,
    #[serde(deserialize_with = "deserialize_i32_flexible", default)]
    pub bitrate: Option<i32>,
    #[serde(deserialize_with = "deserialize_i32_flexible", default)]
    pub channels: Option<i32>,
    #[serde(deserialize_with = "deserialize_i32_flexible", default)]
    pub sampling_rate: Option<i32>,
    #[serde(deserialize_with = "deserialize_i32_flexible", default)]
    pub bit_depth: Option<i32>,
}

/// Plex transcode session info — present when server is transcoding
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TranscodeSession {
    pub audio_codec: Option<String>,
    #[serde(deserialize_with = "deserialize_i32_flexible", default)]
    pub audio_channels: Option<i32>,
    pub transcode_hw_requested: Option<bool>,
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
    session_states: HashMap<String, SessionState>,
    libraries: Vec<Library>,
}

#[derive(Debug, Default)]
pub struct SessionResult {
    pub now_playing: Vec<PlexTrack>,
    pub ready_to_scrobble: Vec<PlexTrack>,
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
                .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|_| Client::new()),
            filters,
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
                            if let Some(ref q) = track.audio_quality {
                                debug!("Plex quality: {} {} {}/{} score={:.0}{}",
                                    q.codec.as_deref().unwrap_or("?"),
                                    if q.is_lossless.unwrap_or(false) { "lossless" } else { "lossy" },
                                    q.bit_depth.unwrap_or(0),
                                    q.sample_rate.unwrap_or(0),
                                    q.quality_score.unwrap_or(0.0),
                                    if q.is_transcoded.unwrap_or(false) {
                                        format!(" → transcoded to {}", q.delivery_codec.as_deref().unwrap_or("?"))
                                    } else { String::new() },
                                );
                            }
                            self.process_live_track(track, is_playing, &mut result).await;
                        }
                    }
                }
            }
        }

        // B. History Sync
        if let Ok(history) = self.fetch_history_tracks(0).await {
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
            result.ready_to_scrobble.push(track);
        } else if needs_np {
            info!("Track now playing: '{}' by '{}'", track.title, track.artist);
            result.now_playing.push(track);
        }
    }

    async fn fetch_history_tracks(&mut self, _min_timestamp: u64) -> Result<Vec<PlexTrack>, Box<dyn std::error::Error>> {
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

        let mut tracks = Vec::new();
        let seven_days_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap().as_secs()
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
                tracks.push(track);
            }
        }
        Ok(tracks)
    }

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

    pub async fn get_playback_data(&mut self) -> Result<SessionResult, Box<dyn std::error::Error>> {
        self.fetch_sessions_extended(None).await
    }

    pub async fn fetch_sessions(&mut self) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let result = self.fetch_sessions_extended(None).await?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        Ok(result.ready_to_scrobble.into_iter()
            .map(|track| track.to_play(&format!("scrobble-{}", now)))
            .collect())
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
