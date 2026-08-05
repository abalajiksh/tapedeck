use serde::{Deserialize, Serialize};

// ── Auth ──

/// Authenticated user identity, resolved from a session cookie or API token.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub username: String,
    pub is_admin: bool,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioQuality {
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i16>,
    pub channels: Option<i16>,
    pub container: Option<String>,
    pub is_lossless: Option<bool>,
    pub dsd_rate: Option<i64>,
    pub dsd_multiplier: Option<i16>,
    pub delivery_codec: Option<String>,
    pub delivery_bitrate: Option<i32>,
    pub delivery_sample_rate: Option<i32>,
    pub delivery_bit_depth: Option<i32>,
    /// What the player believes it was handed — "lossless", "hi-res", "cd",
    /// "lossy". The player's own claim about the file, kept apart from the
    /// measurements beside it because the two can disagree and the
    /// disagreement is the interesting part.
    pub source_quality: Option<String>,
    /// Was a DSD stream decoded to PCM before it reached the output?
    ///
    /// Deliberately separate from `is_transcoded`: a DSD→PCM conversion is not
    /// a bitrate compromise, it's a change of representation, and on a chain
    /// built around a DSD-capable DAC it is the difference between native
    /// playback and the thing you bought the DAC to avoid. It does **not**
    /// enter `compute_score` — see the note there.
    pub dsd_to_pcm_converted: Option<bool>,
    pub is_transcoded: Option<bool>,
    pub transcode_reason: Option<String>,
    pub quality_score: Option<f64>,
}

/// How a listen should be filed: is it a backfill, and was it skipped?
///
/// Both answers come out as `status = 'imported'`, which is what takes a row
/// out of `flush_pending`'s queue — so this is the one place that decides
/// whether a listen is ever forwarded to Last.fm / ListenBrainz / Libre.fm.
/// It exists as a struct rather than two more positional arguments because the
/// coupling is the point: forgetting either one puts a wrong scrobble on
/// someone's permanent public record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListenDisposition {
    /// The submission declared itself a backfill (`listen_type: "import"`, or
    /// a file import). A historical listen is not news — forwarding it
    /// rewrites the user's scrobble history at the far end.
    pub imported: bool,
    /// The client reported the track was skipped rather than played through.
    /// Stored so skip-rate analytics has something real to count, excluded
    /// from every count, and never forwarded.
    pub skipped: bool,
}

impl ListenDisposition {
    pub const LIVE: Self = Self { imported: false, skipped: false };

    /// `'pending'` only for a live, played-through listen; everything else is
    /// filed as already-delivered so the flush loop leaves it alone.
    pub fn status(&self) -> &'static str {
        if self.imported || self.skipped {
            "imported"
        } else {
            "pending"
        }
    }
}

impl AudioQuality {
    /// Note what is deliberately *not* in here: `dsd_to_pcm_converted` and
    /// `source_quality`. Both are stored and both are worth seeing, but the
    /// score is written once at ingest and never recomputed, so folding a new
    /// term in would silently make every listen recorded after the change
    /// incomparable with every listen before it. If these should count, the
    /// change is a backfill that rescores the whole history, not a new line
    /// here.
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
        let mut score: f64 = base;
        if self.is_transcoded.unwrap_or(false) {
            score -= 10.0;
        }
        if self.delivery_codec.is_some() {
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

    pub fn has_data(&self) -> bool {
        self.format_type.is_some()
            || self.codec.is_some()
            || self.bitrate.is_some()
            || self.sample_rate.is_some()
    }
}

// ── Phase 2: Signal Chains ──

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainComponent {
    #[serde(rename = "type")]
    pub component_type: ComponentType,
    pub name: String,
    pub detail: Option<String>,
    /// The `equipment` row this step *is*, when it was picked from the user's
    /// gear rather than typed. Optional on purpose: a chain can name something
    /// you don't own (a friend's amp, a rental) and old chains predate the link
    /// entirely. `name` stays a snapshot so a deleted gear row leaves the chain
    /// readable instead of blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equipment_id: Option<i64>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListeningContext {
    Active,
    ActiveMobile,
    Passive,
    Background,
    #[default]
    Unknown,
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

// Device, Equipment, and ListeningSession rows are read back as ad-hoc tuples
// or the DeviceRow/EquipmentRow types in db.rs — no shared model struct needed.
