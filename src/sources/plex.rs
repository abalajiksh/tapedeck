use crate::models::{AudioQuality, Play};
use crate::sources::MusicSource;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

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
        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value as i32))
        }
        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value as i32))
        }
        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value.parse::<i32>().map(Some).map_err(de::Error::custom)
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
    /// Plex account name (from the session's `User` element), used to map the
    /// listen to the matching Tapedeck user. None for history-derived tracks.
    pub plex_user: Option<String>,
    /// Plex's own id for a *play event*, present only on history rows. It is
    /// the one stable key a re-walked history entry keeps, so it becomes the
    /// `source_id` — the old suffix was the poll's wall clock, which made every
    /// walk of the same 50 rows look like 50 new plays to `Database::exists`.
    pub history_key: Option<String>,
    /// The player this came off, when it came from a live session.
    ///
    /// A Plex listen carries no submitting token, so three of the four rungs of
    /// the chain ladder can never match and every one of them landed with no
    /// signal chain at all. The `Player` element is the missing evidence: it
    /// maps onto `devices` exactly, which makes the device rung reachable.
    /// History rows have no player — nothing is playing by then.
    pub player: Option<PlexPlayer>,
}

/// An account on the Plex server — the server owner plus any managed or shared
/// users. `id` is Plex's own account id; sessions are matched on `name`, which
/// is what the `User` element on a session carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlexAccount {
    pub id: i64,
    pub name: String,
}

/// The playing device, reduced to what `devices` stores.
#[derive(Debug, Clone)]
pub struct PlexPlayer {
    pub machine_id: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub product: Option<String>,
}

impl PlexTrack {
    fn from_session(s: SessionMetadata) -> Option<Self> {
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
            plex_user: s.user.as_ref().and_then(|u| u.title.clone()),
            history_key: None,
            // No machineIdentifier means nothing stable to key a device on, so
            // the listen keeps its old behaviour rather than getting a device
            // row that a rename or reconnect would orphan.
            player: s.player.as_ref().and_then(|p| {
                p.machine_identifier.clone().map(|machine_id| PlexPlayer {
                    machine_id,
                    name: p.title.clone(),
                    // "Linux" is more use on a device list than "Linux" the
                    // platform, but Plex only sends `device` on some clients.
                    platform: p.device.clone().or_else(|| p.platform.clone()),
                    product: p.product.clone(),
                })
            }),
        })
    }

    fn from_history(h: HistoryTrack) -> Option<Self> {
        Some(Self {
            title: h.title?,
            artist: h.artist.unwrap_or_else(|| "Unknown".to_string()),
            track_artist: h.track_artist,
            history_key: h.history_key,
            album: h.album,
            duration: h.duration,
            view_offset: None,
            rating_key: h.rating_key?,
            parent_rating_key: h.parent_rating_key,
            grandparent_rating_key: h.grandparent_rating_key,
            session_key: None,
            viewed_at: h.viewed_at,
            audio_quality: None,
            plex_user: None,
            player: None,
        })
    }

    fn extract_quality(session: &SessionMetadata) -> Option<AudioQuality> {
        let media = session.media.as_ref()?.first()?;

        let audio_stream = media
            .part
            .as_ref()
            .and_then(|parts| parts.first())
            .and_then(|part| part.stream.as_ref())
            .and_then(|streams| streams.iter().find(|s| s.stream_type == Some(2)));

        let codec =
            audio_stream.and_then(|s| s.codec.clone()).or_else(|| media.audio_codec.clone());

        let bitrate = audio_stream.and_then(|s| s.bitrate).or(media.bitrate);

        let sample_rate = audio_stream.and_then(|s| s.sampling_rate);
        let bit_depth = audio_stream.and_then(|s| s.bit_depth);
        let channels = audio_stream.and_then(|s| s.channels).or(media.audio_channels);

        let codec_lower = codec.as_deref().unwrap_or("").to_lowercase();
        let is_lossless =
            matches!(codec_lower.as_str(), "flac" | "alac" | "wav" | "aiff" | "dsd" | "pcm");
        let format_type = if codec_lower == "dsd" { "dsd" } else { "pcm" };

        let container = media.container.clone();

        let (is_transcoded, delivery_codec, delivery_bitrate, transcode_reason) =
            if let Some(ref tc) = session.transcode_session {
                let tc_codec = tc.audio_codec.clone();
                let is_tc = tc_codec.as_deref() != codec.as_deref();
                let reason = if is_tc {
                    tc.transcode_hw_requested
                        .map(|_| "server_transcode".to_string())
                        .or_else(|| Some("bandwidth".to_string()))
                } else {
                    None
                };
                (Some(is_tc), if is_tc { tc_codec } else { None }, None, reason)
            } else {
                (Some(false), None, None, None)
            };

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
        // A history row carries Plex's own play-event id, which is stable
        // across walks; a live row has only the caller's suffix.
        let source_id = match &self.history_key {
            Some(hk) => format!("plex-hist-{}", hk),
            None => format!("plex-{}-{}", self.rating_key, source_id_suffix),
        };

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
    /// Art path Plex itself reports, e.g. `/library/metadata/55821/thumb/1699`.
    /// A track usually has none of its own and inherits the album's, which is
    /// what `parent_thumb` carries.
    pub thumb: Option<String>,
    pub parent_thumb: Option<String>,
    #[serde(deserialize_with = "deserialize_u64_flexible", default)]
    pub duration: Option<u64>,
    #[serde(deserialize_with = "deserialize_u64_flexible", default)]
    pub view_offset: Option<u64>,
    pub session_key: Option<String>,
    #[serde(rename = "Player")]
    pub player: Option<Player>,
    #[serde(rename = "User")]
    pub user: Option<User>,
    #[serde(rename = "Media", default)]
    pub media: Option<Vec<PlexMedia>>,
    #[serde(rename = "TranscodeSession")]
    pub transcode_session: Option<TranscodeSession>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub state: Option<String>,
    /// The player's own name — "fedora.fritz.box", "Living Room".
    pub title: Option<String>,
    /// The app: "Plexamp", "Plex for Android", "Plex Web".
    pub product: Option<String>,
    /// Stable per-player id, and the natural `devices.machine_id`.
    pub machine_identifier: Option<String>,
    pub platform: Option<String>,
    /// Hardware descriptor — "Linux", "iPhone", "Windows".
    pub device: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub title: Option<String>,
}

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

// ==================== History Track ====================

#[derive(Debug)]
struct HistoryTrack {
    media_type: Option<String>,
    viewed_at: Option<u64>,
    history_key: Option<String>,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    track_artist: Option<String>,
    duration: Option<u64>,
    rating_key: Option<String>,
    parent_rating_key: Option<String>,
    grandparent_rating_key: Option<String>,
}

// ==================== Session Tracking ====================

#[derive(Debug, Clone)]
struct SessionState {
    rating_key: String,
    last_seen: u64,
    scrobbled: bool,
    /// Previous playhead, in ms. A track on repeat keeps the same `sessionKey`
    /// *and* the same `ratingKey`, so the only evidence that it started over is
    /// the playhead jumping backwards — see `process_live_track`.
    last_offset: u64,
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
    /// When the history endpoint was last walked. Sessions are polled every few
    /// seconds so the deck is live; history is a much heavier request and only
    /// exists to catch plays that finished between polls, which at this cadence
    /// is close to nothing. Throttled separately rather than tied to the caller.
    last_history_at: u64,
    /// `rating_key` -> when the live path last emitted a scrobble for it.
    ///
    /// The history endpoint re-reports plays the live path already caught, and
    /// it reports them *differently*: Plex omits `originalTitle` there, so a
    /// compilation track that played as "Punch" comes back as "Various
    /// Artists". Neither dedup in `save_scrobble` catches that — `source_id`
    /// carried the poll's wall clock so it never matched, and `fuzzy_exists`
    /// keys on the artist, which is the very field that differs. The result was
    /// two rows per play, one with quality metadata and one without.
    recent_scrobbles: HashMap<String, u64>,
}

/// How often the Plex history endpoint is walked, independent of how often
/// sessions are read.
const HISTORY_SYNC_INTERVAL_SECS: u64 = 60;

/// How far the playhead must jump backwards before it counts as the track
/// starting over rather than the user scrubbing. Plex reports `viewOffset` a
/// few seconds coarse, and a repeat always lands near zero, so this only has to
/// clear ordinary seek noise.
const REPEAT_REWIND_MS: u64 = 20_000;

/// How long a live scrobble suppresses the same item arriving from the history
/// endpoint. History is walked every 60s and returns the last 50 plays, so a
/// track the live path already banked shows up there for the rest of the
/// session — see `recent_scrobbles`.
const HISTORY_SUPPRESS_SECS: u64 = 30 * 60;

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
            last_history_at: 0,
            recent_scrobbles: HashMap::new(),
        }
    }

    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.libraries = self.fetch_libraries().await?;
        info!("Plex: Loaded {} libraries", self.libraries.len());
        Ok(())
    }

    /// Which Plex account a token belongs to.
    ///
    /// This is what makes per-user Plex possible: a Tapedeck user supplies their
    /// *own* token, we resolve it to a Plex username, and only that account's
    /// sessions become their listens. Without it we'd be back to polling every
    /// session on the server and guessing owners by name.
    ///
    /// Asks plex.tv rather than the media server, because the server reports who
    /// is *playing*, not who a token belongs to. That means this needs outbound
    /// internet access even for a LAN-only Plex — callers should treat failure as
    /// "ask the user to type their Plex username" rather than fatal.
    /// Every account the server knows about, for the source-setup picker.
    ///
    /// `resolve_account` answers "whose token is this", which is the server
    /// owner's — so a household where one admin holds the token could only ever
    /// scrobble the admin's own listens. This is the other half: the server's
    /// own `/accounts` lists managed and shared users, so a Tapedeck user can be
    /// pointed at the account that is actually theirs.
    ///
    /// Plex keeps tombstones for removed accounts, which come back with an empty
    /// `name`; they're dropped rather than shown as blank rows in a dropdown.
    pub async fn list_accounts(
        url: &str,
        token: &str,
    ) -> Result<Vec<PlexAccount>, Box<dyn std::error::Error>> {
        #[derive(Deserialize)]
        struct Container {
            #[serde(rename = "Account", default)]
            account: Vec<PlexAccount>,
        }
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(rename = "MediaContainer")]
            media_container: Container,
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
            .build()?;

        let resp = client
            .get(format!("{}/accounts", url.trim_end_matches('/')))
            .header("X-Plex-Token", token)
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("Plex rejected that token".into());
        }
        if !resp.status().is_success() {
            return Err(format!("Plex returned {}", resp.status()).into());
        }

        let wrapper: Wrapper = resp.json().await?;
        Ok(wrapper
            .media_container
            .account
            .into_iter()
            .filter(|a| !a.name.trim().is_empty())
            .collect())
    }

    pub async fn resolve_account(
        url: &str,
        token: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        #[derive(Deserialize)]
        struct PlexTvUser {
            username: Option<String>,
            title: Option<String>,
            email: Option<String>,
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
            .build()?;

        let resp = client
            .get("https://plex.tv/api/v2/user")
            .header("X-Plex-Token", token)
            .header("Accept", "application/json")
            .header("X-Plex-Product", env!("CARGO_PKG_NAME"))
            .header("X-Plex-Client-Identifier", format!("tapedeck-{}", env!("CARGO_PKG_VERSION")))
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("Plex rejected that token".into());
        }
        if !resp.status().is_success() {
            return Err(format!("plex.tv returned {}", resp.status()).into());
        }

        let user: PlexTvUser = resp.json().await?;
        // Prefer the stable username; `title` is the display name shown in
        // sessions, which is what we actually match against, so it's the better
        // fallback than email.
        user.username.or(user.title).or(user.email).filter(|s| !s.trim().is_empty()).ok_or_else(
            || {
                let _ = url; // server URL unused here; kept for call-site symmetry
                "plex.tv did not report an account name for that token".into()
            },
        )
    }

    /// Cover art bytes for one library item (a track's `ratingKey`).
    ///
    /// Proxied for the same reason as Navidrome's: a LAN Plex isn't reachable
    /// from the browser, and the token must not leave the server.
    pub async fn fetch_cover_art(
        &self,
        rating_key: &str,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error + Send + Sync>> {
        // Ask Plex where the art is rather than assuming the URL shape:
        // `/library/metadata/<key>/thumb` 404s for a track, because a track
        // normally has no art of its own and inherits the album's. The metadata
        // response carries both, already versioned.
        let meta_url = format!("{}/library/metadata/{}", self.url, rating_key);
        let meta: SessionsResponse = self
            .client
            .get(&meta_url)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?
            .json()
            .await?;

        let art_path = meta
            .media_container
            .metadata
            .first()
            .and_then(|m| m.thumb.clone().or_else(|| m.parent_thumb.clone()))
            .ok_or("Plex has no artwork for that item")?;

        let endpoint = format!("{}{}", self.url, art_path);
        let resp = self.client.get(&endpoint).header("X-Plex-Token", &self.token).send().await?;

        if !resp.status().is_success() {
            return Err(format!("Plex artwork returned {}", resp.status()).into());
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        if !content_type.starts_with("image/") {
            return Err("no artwork for that item".into());
        }
        Ok((resp.bytes().await?.to_vec(), content_type))
    }

    /// Library titles discovered by `initialize`. Used by the settings UI to
    /// show what a URL/token pair can actually see.
    pub fn library_names(&self) -> Vec<String> {
        self.libraries.iter().map(|l| l.title.clone()).collect()
    }

    async fn fetch_libraries(&self) -> Result<Vec<Library>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/library/sections", self.url);
        let resp = self
            .client
            .get(&endpoint)
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

    pub async fn fetch_sessions_extended(
        &mut self,
        _last_checked: Option<u64>,
    ) -> Result<SessionResult, Box<dyn std::error::Error>> {
        let mut result = SessionResult::default();

        // A. Live Sessions
        let endpoint = format!("{}/status/sessions", self.url);
        if let Ok(resp) = self
            .client
            .get(&endpoint)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(sessions) = resp.json::<SessionsResponse>().await {
                    info!(
                        "Processing {} active Plex sessions",
                        sessions.media_container.metadata.len()
                    );

                    for session in sessions.media_container.metadata {
                        if let Some(reason) = self.validate_session(&session) {
                            debug!("Skipping session: {}", reason);
                            continue;
                        }
                        if session.media_type.as_deref() != Some("track") {
                            continue;
                        }

                        let is_playing = session
                            .player
                            .as_ref()
                            .map(|p| p.state.as_deref() == Some("playing"))
                            .unwrap_or(false);

                        if let Some(track) = PlexTrack::from_session(session) {
                            if let Some(ref q) = track.audio_quality {
                                debug!(
                                    "Plex quality: {} {} {}/{} score={:.0}{}",
                                    q.codec.as_deref().unwrap_or("?"),
                                    if q.is_lossless.unwrap_or(false) {
                                        "lossless"
                                    } else {
                                        "lossy"
                                    },
                                    q.bit_depth.unwrap_or(0),
                                    q.sample_rate.unwrap_or(0),
                                    q.quality_score.unwrap_or(0.0),
                                    if q.is_transcoded.unwrap_or(false) {
                                        format!(
                                            " → transcoded to {}",
                                            q.delivery_codec.as_deref().unwrap_or("?")
                                        )
                                    } else {
                                        String::new()
                                    },
                                );
                            }
                            self.process_live_track(track, is_playing, &mut result).await;
                        }
                    }
                }
            }
        }

        // B. History Sync — on its own clock, see `last_history_at`.
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        if now.saturating_sub(self.last_history_at) >= HISTORY_SYNC_INTERVAL_SECS {
            self.last_history_at = now;
            self.recent_scrobbles
                .retain(|_, at| now.saturating_sub(*at) < HISTORY_SUPPRESS_SECS);
            if let Ok(history) = self.fetch_history_tracks(0).await {
                let fetched = history.len();
                // Drop anything the live path already banked. History is the
                // fallback for plays that finished between polls, not a second
                // opinion on plays we watched happen — and its rows carry the
                // album artist and no quality metadata, so letting one through
                // is a visibly worse duplicate of a row we already have.
                let fresh: Vec<PlexTrack> = history
                    .into_iter()
                    .filter(|t| !self.recent_scrobbles.contains_key(&t.rating_key))
                    .collect();
                info!(
                    "Fetched {} tracks from Plex history, {} new ({} already banked live)",
                    fetched,
                    fresh.len(),
                    fetched - fresh.len()
                );
                result.ready_to_scrobble.extend(fresh);
            }
        }

        debug!(
            "Session fetch complete: {} now playing, {} ready to scrobble",
            result.now_playing.len(),
            result.ready_to_scrobble.len()
        );
        Ok(result)
    }

    async fn process_live_track(
        &mut self,
        track: PlexTrack,
        is_playing: bool,
        result: &mut SessionResult,
    ) {
        let session_key = track.session_key.clone().unwrap_or_default();
        let rating_key = track.rating_key.clone();
        let duration = track.duration.unwrap_or(0);
        let view_offset = track.view_offset.unwrap_or(0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        if let Some(state) = self.session_states.get(&session_key) {
            if state.rating_key != rating_key {
                debug!(
                    "Track changed in session {}: {} -> {}",
                    session_key, state.rating_key, rating_key
                );
                self.session_states.remove(&session_key);
            }
        }

        let (should_scrobble, needs_np) = {
            let state = self.session_states.entry(session_key.clone()).or_insert_with(|| {
                SessionState {
                    rating_key: rating_key.clone(),
                    last_seen: now,
                    scrobbled: false,
                    last_offset: view_offset,
                }
            });
            state.last_seen = now;

            // A track on repeat is invisible to the `rating_key` check above:
            // Plex keeps the same session *and* the same item, so `scrobbled`
            // stayed true forever and only the first of N plays was ever
            // banked. The playhead jumping backwards is the one signal that
            // the track started over. Require a real rewind (not a small
            // seek-back) *and* that we'd already banked this pass, so scrubbing
            // around inside one listen can't mint extra scrobbles.
            if state.last_offset > view_offset
                && state.last_offset.saturating_sub(view_offset) >= REPEAT_REWIND_MS
            {
                if state.scrobbled {
                    debug!(
                        "Playhead rewound in session {} ({}ms -> {}ms): '{}' started over",
                        session_key, state.last_offset, view_offset, track.title
                    );
                }
                state.scrobbled = false;
            }
            state.last_offset = view_offset;

            let pct =
                if duration > 0 { (view_offset as f64 / duration as f64) * 100.0 } else { 0.0 };
            let seconds = view_offset / 1000;

            let ready = !state.scrobbled && (pct >= 50.0 || seconds >= 240);
            // Whether we've already banked the scrobble says nothing about
            // what's on the deck. Gating now-playing on `!scrobbled` meant a
            // track stopped being reported the moment it passed the halfway
            // mark, and the pipeline reads an empty session list as
            // "stopped" — so the deck went blank for the back half of every
            // track and fell back to the last listen.
            let np = is_playing;
            (ready, np)
        };

        if should_scrobble {
            info!("Track ready for scrobble: '{}' by '{}'", track.title, track.artist);
            if let Some(state) = self.session_states.get_mut(&session_key) {
                state.scrobbled = true;
            }
            // Claim this item so the history sync doesn't re-report the same
            // play under its album artist a minute later.
            self.recent_scrobbles.insert(rating_key.clone(), now);
            result.ready_to_scrobble.push(track.clone());
        }
        if needs_np {
            debug!("Track now playing: '{}' by '{}'", track.title, track.artist);
            result.now_playing.push(track);
        }
    }

    /// Parse Plex history XML manually using quick_xml::Reader.
    ///
    /// quick_xml's serde deserializer fails on repeated same-name elements
    /// ("duplicate field `Track`"), so we extract <Track> attributes directly.
    fn parse_history_xml(xml: &str) -> Vec<HistoryTrack> {
        use quick_xml::escape::unescape;
        use quick_xml::events::Event;
        use quick_xml::Reader;

        let mut reader = Reader::from_str(xml);
        let mut tracks = Vec::new();

        loop {
            match reader.read_event() {
                Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e))
                    if e.name().as_ref() == b"Track" =>
                {
                    let mut track = HistoryTrack {
                        media_type: None,
                        viewed_at: None,
                        history_key: None,
                        title: None,
                        artist: None,
                        album: None,
                        track_artist: None,
                        duration: None,
                        rating_key: None,
                        parent_rating_key: None,
                        grandparent_rating_key: None,
                    };

                    for attr in e.attributes().flatten() {
                        // FIX: Decode XML/HTML entities (&#339; → œ, &#8217; → ', etc.)
                        let raw = String::from_utf8_lossy(&attr.value);
                        let val = unescape(&raw)
                            .map(|s| s.into_owned())
                            .unwrap_or_else(|_| raw.into_owned());
                        match attr.key.as_ref() {
                            b"type" => track.media_type = Some(val),
                            b"viewedAt" => track.viewed_at = val.parse().ok(),
                            b"historyKey" => track.history_key = Some(val),
                            b"title" => track.title = Some(val),
                            b"grandparentTitle" => track.artist = Some(val),
                            b"parentTitle" => track.album = Some(val),
                            b"originalTitle" => track.track_artist = Some(val),
                            b"duration" => track.duration = val.parse().ok(),
                            b"ratingKey" => track.rating_key = Some(val),
                            b"parentRatingKey" => track.parent_rating_key = Some(val),
                            b"grandparentRatingKey" => track.grandparent_rating_key = Some(val),
                            _ => {}
                        }
                    }

                    tracks.push(track);
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    warn!("Error parsing history XML: {}", e);
                    break;
                }
                _ => {}
            }
        }

        tracks
    }

    async fn fetch_history_tracks(
        &mut self,
        _min_timestamp: u64,
    ) -> Result<Vec<PlexTrack>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/status/sessions/history/all", self.url);
        let resp = self
            .client
            .get(&endpoint)
            .query(&[("sort", "viewedAt:desc"), ("limit", "50")])
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/xml")
            .send()
            .await?;

        if !resp.status().is_success() {
            warn!("History fetch failed: HTTP {}", resp.status());
            return Ok(Vec::new());
        }

        let text = resp.text().await?;
        let history_tracks = Self::parse_history_xml(&text);

        let mut tracks = Vec::new();
        let seven_days_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(7 * 86400);

        info!("Processing {} history tracks from Plex", history_tracks.len());
        for h_track in history_tracks {
            if h_track.media_type.as_deref() != Some("track") {
                continue;
            }
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
        let user = session
            .user
            .as_ref()
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
            if !self.filters.libraries_allow.is_empty()
                && !self.filters.libraries_allow.contains(&lib_lower)
            {
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
        Ok(result
            .ready_to_scrobble
            .into_iter()
            .map(|track| track.to_play(&format!("scrobble-{}", now)))
            .collect())
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
    async fn fetch_new_plays(
        &mut self,
        _last_checked: u64,
    ) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        self.fetch_sessions().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(offset_ms: u64) -> PlexTrack {
        PlexTrack {
            title: "Xtal".into(),
            artist: "Aphex Twin".into(),
            track_artist: None,
            album: Some("Selected Ambient Works 85-92".into()),
            duration: Some(300_000),
            view_offset: Some(offset_ms),
            rating_key: "rk1".into(),
            parent_rating_key: None,
            grandparent_rating_key: None,
            session_key: Some("sk1".into()),
            viewed_at: None,
            audio_quality: None,
            plex_user: None,
            history_key: None,
            player: None,
        }
    }

    /// The deck must keep reporting a track after it has been banked for
    /// scrobbling. Gating now-playing on `!scrobbled` blanked it for the back
    /// half of every track, and the pipeline reads an empty list as "stopped".
    #[tokio::test]
    async fn now_playing_survives_the_scrobble_threshold() {
        let mut src = PlexSource::new("http://localhost:32400".into(), "t".into());

        // 20% in — playing, nothing to scrobble yet.
        let mut r = SessionResult::default();
        src.process_live_track(track(60_000), true, &mut r).await;
        assert!(r.ready_to_scrobble.is_empty());
        assert_eq!(r.now_playing.len(), 1);

        // Crosses halfway: scrobble queued *and* still on the deck.
        let mut r = SessionResult::default();
        src.process_live_track(track(160_000), true, &mut r).await;
        assert_eq!(r.ready_to_scrobble.len(), 1);
        assert_eq!(r.now_playing.len(), 1);

        // Rest of the track: still on the deck, never scrobbled twice.
        let mut r = SessionResult::default();
        src.process_live_track(track(260_000), true, &mut r).await;
        assert!(r.ready_to_scrobble.is_empty());
        assert_eq!(r.now_playing.len(), 1);

        // Paused stays off the deck — an idle session is a stopped one.
        let mut r = SessionResult::default();
        src.process_live_track(track(270_000), false, &mut r).await;
        assert!(r.now_playing.is_empty());
    }

    /// A track on repeat keeps the same `sessionKey` *and* `ratingKey`, so the
    /// track-changed reset never fires and only the first of N plays was
    /// banked. The playhead rewinding is the only evidence it started over.
    #[tokio::test]
    async fn a_looped_track_scrobbles_once_per_pass() {
        let mut src = PlexSource::new("http://localhost:32400".into(), "t".into());

        let mut scrobbles = 0;
        for pass in 0..3 {
            // Each pass walks the playhead from near zero to the end. Same
            // session, same item — only the offset says a new play began.
            for offset in [5_000u64, 160_000, 280_000] {
                let mut r = SessionResult::default();
                src.process_live_track(track(offset), true, &mut r).await;
                scrobbles += r.ready_to_scrobble.len();
            }
            assert_eq!(scrobbles, pass + 1, "pass {pass} should have banked exactly one");
        }
    }

    /// Scrubbing backwards inside one listen is not a new play. Only a rewind
    /// past `REPEAT_REWIND_MS` counts, so nudging the playhead back a few
    /// seconds must not mint a second scrobble.
    #[tokio::test]
    async fn a_small_seek_back_is_not_a_repeat() {
        let mut src = PlexSource::new("http://localhost:32400".into(), "t".into());

        let mut r = SessionResult::default();
        src.process_live_track(track(160_000), true, &mut r).await;
        assert_eq!(r.ready_to_scrobble.len(), 1);

        // Back five seconds, then forward again: still the same listen.
        for offset in [155_000u64, 158_000, 200_000] {
            let mut r = SessionResult::default();
            src.process_live_track(track(offset), true, &mut r).await;
            assert!(r.ready_to_scrobble.is_empty(), "seek at {offset}ms re-scrobbled");
        }
    }

    /// The `Player` element is what makes a Plex listen resolvable to a signal
    /// chain, and a wrong field name here would be completely silent — the
    /// device would just never be recorded and every listen would keep landing
    /// with no chain, exactly the bug this fixed. Pinned against a real
    /// /status/sessions response.
    #[test]
    fn the_player_element_maps_onto_a_device() {
        let body = r#"{
            "MediaContainer": {
                "size": 1,
                "Metadata": [{
                    "ratingKey": "113444",
                    "type": "track",
                    "title": "Wicked Game",
                    "grandparentTitle": "Chris Isaak",
                    "parentTitle": "Best of Chris Isaak",
                    "sessionKey": "49",
                    "duration": 286880,
                    "viewOffset": 182656,
                    "User": { "id": "1", "title": "s.ashwinbalaji" },
                    "Player": {
                        "address": "192.168.178.96",
                        "device": "Linux",
                        "machineIdentifier": "721c1772-4434-4275-aa9a-39b37237f1b2",
                        "platform": "Linux",
                        "product": "Plexamp",
                        "state": "playing",
                        "title": "fedora.fritz.box"
                    }
                }]
            }
        }"#;
        let mut resp: SessionsResponse = serde_json::from_str(body).expect("parses");
        let meta = resp.media_container.metadata.pop().expect("one track");
        let track = PlexTrack::from_session(meta).expect("becomes a track");

        let player = track.player.expect("Player was parsed");
        assert_eq!(player.machine_id, "721c1772-4434-4275-aa9a-39b37237f1b2");
        assert_eq!(player.name.as_deref(), Some("fedora.fritz.box"));
        assert_eq!(player.product.as_deref(), Some("Plexamp"));
        // `device` is preferred over `platform` — both say "Linux" here, but
        // only `device` is meaningful on clients that send both.
        assert_eq!(player.platform.as_deref(), Some("Linux"));
        assert_eq!(track.plex_user.as_deref(), Some("s.ashwinbalaji"));
    }

    /// The art path is read off Plex's own metadata response, so the field
    /// names have to match what it actually sends. A track carries no `thumb`
    /// of its own — it inherits the album's `parentThumb`, and missing that is
    /// how `fetch_cover_art` ends up with nothing to fetch.
    #[test]
    fn track_metadata_falls_back_to_the_album_thumb() {
        let body = r#"{
            "MediaContainer": {
                "size": 1,
                "Metadata": [{
                    "ratingKey": "55821",
                    "type": "track",
                    "title": "Xtal",
                    "parentRatingKey": "55820",
                    "parentTitle": "Selected Ambient Works 85-92",
                    "parentThumb": "/library/metadata/55820/thumb/1699999999",
                    "grandparentTitle": "Aphex Twin"
                }]
            }
        }"#;
        let parsed: SessionsResponse = serde_json::from_str(body).expect("Plex metadata shape");
        let m = parsed.media_container.metadata.first().expect("one item");
        assert_eq!(m.thumb, None);
        assert_eq!(
            m.thumb.clone().or_else(|| m.parent_thumb.clone()).as_deref(),
            Some("/library/metadata/55820/thumb/1699999999")
        );
    }

    /// An album asked for directly does have its own thumb, and it wins.
    #[test]
    fn an_items_own_thumb_wins() {
        let body = r#"{"MediaContainer":{"Metadata":[{
            "ratingKey":"55820","type":"album",
            "thumb":"/library/metadata/55820/thumb/17",
            "parentThumb":"/library/metadata/900/thumb/12"
        }]}}"#;
        let parsed: SessionsResponse = serde_json::from_str(body).unwrap();
        let m = parsed.media_container.metadata.first().unwrap();
        assert_eq!(
            m.thumb.clone().or_else(|| m.parent_thumb.clone()).as_deref(),
            Some("/library/metadata/55820/thumb/17")
        );
    }
}
