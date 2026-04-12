use serde::{Deserialize, Serialize};

/// Top-level request body for `/1/submit-listens`
#[derive(Debug, Deserialize)]
pub struct SubmitListensRequest {
    pub listen_type: ListenType,
    pub payload: Vec<ListenPayload>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListenType {
    Single,
    PlayingNow,
    Import,
}

#[derive(Debug, Deserialize)]
pub struct ListenPayload {
    /// Unix timestamp — required for `single` and `import`, absent for `playing_now`
    pub listened_at: Option<i64>,
    pub track_metadata: TrackMetadata,
}

#[derive(Debug, Deserialize)]
pub struct TrackMetadata {
    pub artist_name: String,
    pub track_name: String,
    pub release_name: Option<String>,
    #[serde(default)]
    pub additional_info: Option<AdditionalInfo>,
    #[serde(default)]
    pub mbid_mapping: Option<MbidMapping>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AdditionalInfo {
    pub submission_client: Option<String>,
    pub submission_client_version: Option<String>,
    pub duration_ms: Option<i64>,
    pub track_number: Option<i32>,

    // ── Tapedeck extensions ──
    pub tapedeck_audio: Option<TapedeckAudio>,
    pub tapedeck_device: Option<TapedeckDevice>,
    pub tapedeck_chain: Option<TapedeckChain>,
    pub tapedeck_session: Option<TapedeckSession>,
}

#[derive(Debug, Deserialize)]
pub struct MbidMapping {
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub artist_mbids: Option<Vec<String>>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

// ── Tapedeck-specific extensions (roadmap 4.2) ──

#[derive(Debug, Deserialize)]
pub struct TapedeckAudio {
    pub format_type: Option<String>,  // "pcm", "dsd", "mqa"
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i16>,
    pub channels: Option<i16>,
    pub container: Option<String>,
    pub is_lossless: Option<bool>,
    pub source_quality: Option<String>,
    // DSD fields
    pub dsd_rate: Option<i64>,
    pub dsd_multiplier: Option<i16>,
    // Delivery (BT / transcode)
    pub delivery_codec: Option<String>,
    pub delivery_bitrate: Option<i32>,
    pub delivery_sample_rate: Option<i32>,
    pub delivery_bit_depth: Option<i32>,
    pub dsd_to_pcm_converted: Option<bool>,
    pub is_transcoded: Option<bool>,
    pub transcode_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TapedeckDevice {
    pub player_name: Option<String>,
    pub player_version: Option<String>,
    pub platform: Option<String>,
    pub machine_id: Option<String>,
    pub output_device: Option<String>,
    pub output_type: Option<String>,
    pub interface: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TapedeckChain {
    pub chain_id: Option<String>,
    pub components: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct TapedeckSession {
    pub is_shuffle: Option<bool>,
    pub is_repeat: Option<bool>,
    pub queue_source: Option<String>,
    pub skip_count: Option<i32>,
    pub volume_level: Option<f32>,
}

// ── Response types ──

#[derive(Debug, Serialize)]
pub struct SubmitListensResponse {
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: u16,
    pub error: String,
}
