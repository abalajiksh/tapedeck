use serde::{Deserialize, Serialize};

// ── Auth ──

/// Authenticated user identity, resolved from a valid API token.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub username: String,
}

// ── Core Scrobble ──

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
    pub fn new(
        title: String,
        artist: String,
        timestamp: u64,
        source_id: String,
        source_name: String,
    ) -> Self {
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

// ── Phase 2: Audio Quality ──

/// Audio quality metadata attached to a scrobble.
/// Handles both PCM and DSD as first-class format types, plus
/// source vs. delivered quality for Bluetooth/transcode scenarios.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioQuality {
    // Source format
    pub format_type: Option<String>,   // "pcm", "dsd", "mqa"
    pub codec: Option<String>,         // "FLAC", "AAC", "DSD", "OPUS"
    pub bitrate: Option<i32>,          // kbps
    pub sample_rate: Option<i32>,      // Hz (PCM)
    pub bit_depth: Option<i16>,        // bits (PCM)
    pub channels: Option<i16>,
    pub container: Option<String>,     // "flac", "dsf", "mp4"
    pub is_lossless: Option<bool>,

    // DSD-specific
    pub dsd_rate: Option<i64>,         // Hz (e.g. 2822400 for DSD64)
    pub dsd_multiplier: Option<i16>,   // 64, 128, 256, 512

    // Delivery quality (what actually reached ears)
    pub delivery_codec: Option<String>,
    pub delivery_bitrate: Option<i32>,
    pub delivery_sample_rate: Option<i32>,
    pub delivery_bit_depth: Option<i32>,
    pub is_transcoded: Option<bool>,
    pub transcode_reason: Option<String>,

    // Computed
    pub quality_score: Option<f64>,
}

impl AudioQuality {
    /// Compute a 0–100 quality score for quick comparisons.
    pub fn compute_score(&mut self) {
        let base = match self.format_type.as_deref() {
            Some("dsd") => match self.dsd_multiplier {
                Some(m) if m >= 256 => 98.0,
                Some(128) => 95.0,
                Some(64) => 92.0,
                _ => 90.0,
            },
            Some("pcm") | None => {
                let sr = self.sample_rate.unwrap_or(0);
                let bd = self.bit_depth.unwrap_or(0);
                let lossless = self.is_lossless.unwrap_or(false);

                if !lossless {
                    // Lossy scoring by bitrate
                    match self.bitrate.unwrap_or(0) {
                        b if b >= 320 => 60.0,
                        b if b >= 256 => 55.0,
                        b if b >= 192 => 50.0,
                        b if b >= 128 => 40.0,
                        _ => 30.0,
                    }
                } else {
                    match (bd, sr) {
                        (24, s) if s >= 192000 => 90.0,
                        (24, s) if s >= 96000 => 85.0,
                        (24, _) => 82.0,
                        (16, 44100) => 80.0,
                        (16, s) if s >= 48000 => 81.0,
                        _ => 78.0,
                    }
                }
            }
            _ => 50.0,
        };

        // Penalties
        let mut score: f64 = base;
        if self.is_transcoded.unwrap_or(false) {
            score -= 10.0;
        }
        if self.delivery_codec.is_some() {
            // Bluetooth delivery detected
            match self.delivery_codec.as_deref() {
                Some("ldac") => score -= 3.0,
                Some("aptx_hd") => score -= 5.0,
                Some("aac") => score -= 8.0,
                Some("sbc") => score -= 15.0,
                _ => score -= 5.0,
            }
        }

        self.quality_score = Some(score.clamp(0.0, 100.0));
    }

    /// Check if any quality data is present.
    pub fn has_data(&self) -> bool {
        self.format_type.is_some()
            || self.codec.is_some()
            || self.bitrate.is_some()
            || self.sample_rate.is_some()
    }
}

// ── Phase 2: Signal Chains ──

/// An ordered list of audio components from source to transducer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalChain {
    pub id: Option<i64>,
    pub user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub components: Vec<ChainComponent>,
    pub listening_context: ListeningContext,
    pub is_active: bool,
    pub total_hours: f64,
    pub created_at: i64,
}

/// A single component in a signal chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainComponent {
    #[serde(rename = "type")]
    pub component_type: ComponentType,
    pub name: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentType {
    Source,
    Transport,
    Dac,
    Amp,
    Transducer,
    Network,
    Bluetooth,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListeningContext {
    Active,
    ActiveMobile,
    Passive,
    Background,
    Unknown,
}

impl Default for ListeningContext {
    fn default() -> Self {
        Self::Unknown
    }
}

impl ListeningContext {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::ActiveMobile => "active-mobile",
            Self::Passive => "passive",
            Self::Background => "background",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "active-mobile" | "active_mobile" => Self::ActiveMobile,
            "passive" => Self::Passive,
            "background" => Self::Background,
            _ => Self::Unknown,
        }
    }
}

// ── Phase 2: Devices ──

/// A playback device auto-discovered from scrobble metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: Option<i64>,
    pub user_id: i64,
    pub machine_id: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub product: Option<String>,
    pub device_type: Option<String>,
    pub default_chain_id: Option<i64>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub total_listens: i64,
}

// ── Phase 2: Equipment ──

/// A piece of audio gear tracked for usage hours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Equipment {
    pub id: Option<i64>,
    pub user_id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub equipment_type: String,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub total_hours: f64,
    pub first_used: Option<i64>,
    pub last_used: Option<i64>,
    pub notes: Option<String>,
}

// ── Phase 2: Listening Sessions ──

/// A contiguous listening session (gap < 30 min between scrobbles).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListeningSession {
    pub id: Option<i64>,
    pub user_id: i64,
    pub device_id: Option<i64>,
    pub chain_id: Option<i64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub track_count: i32,
    pub total_duration: i64,
    pub skip_count: i32,
    pub avg_quality_score: Option<f64>,
    pub listening_context: ListeningContext,
}
