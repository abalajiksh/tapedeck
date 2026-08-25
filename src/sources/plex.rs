use crate::models::{AudioQuality, Play, ScrobbleThreshold};
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
    /// Plex account name (from the session's `User` element). None for
    /// history-derived tracks, which carry `plex_account_id` instead.
    pub plex_user: Option<String>,
    /// Plex account id, present only on history rows. This is the history
    /// path's half of per-user isolation: `validate_session` gates live
    /// sessions on the account *name*, and for a long time nothing gated
    /// history at all, so every play by every member of a Plex Home was
    /// attributed to whichever user's source row happened to walk it.
    pub plex_account_id: Option<i64>,
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

/// Are these two spellings of the same artist?
///
/// Delegates to `db::artist_matches` so there is exactly one answer to this in
/// the codebase — this used to be a second, subtly different implementation.
pub fn artist_matches(a: &str, b: &str) -> bool {
    crate::db::artist_matches(a, b)
}

/// Does this track credit already name `artist`?
///
/// Substring rather than equality, because the overlapping case is a credit
/// like "Anirudh Ravichander & Shruti Haasan" on a record whose artist is
/// "Anirudh Ravichander" — he directed it and sang this one. Bounded to whole
/// names so "Karthik" doesn't match inside "Karthika".
fn credit_includes(credit: &str, artist: &str) -> bool {
    let (credit, artist) = (credit.trim().to_lowercase(), artist.trim().to_lowercase());
    if artist.is_empty() {
        return false;
    }
    credit.match_indices(&artist).any(|(i, m)| {
        let before = credit[..i].chars().next_back();
        let after = credit[i + m.len()..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
        boundary(before) && boundary(after)
    })
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
            plex_account_id: None,
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
            plex_account_id: h.account_id,
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
            codec,
            bitrate,
            sample_rate,
            bit_depth: bit_depth.map(|b| b as i16),
            channels: channels.map(|c| c as i16),
            container,
            delivery_codec,
            delivery_bitrate,
            is_transcoded,
            transcode_reason,
            ..Default::default()
        };

        quality.classify_codec();
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

        // Plex gives two attributions and they are not the same question.
        // `grandparentTitle` is the record's artist — for a film soundtrack the
        // music director — and `originalTitle` is who performed this track.
        // Both are real credits and both are wanted; what was happening instead
        // is that the live path took one and the history path the other, so the
        // same play landed twice under two names and MusicBrainz then resolved
        // those to two different recordings.
        //
        // The record's artist is primary: it is how the album is filed, and it
        // keeps one row per record rather than one per performer combination.
        //
        // **Unless the record has no artist.** "Various Artists" is not an
        // artist — it is a placeholder that means *look at the track*. A
        // compilation files every track under it, so taking it as primary
        // recorded all of "Do You Like Brahms?" as Various Artists instead of
        // Punch and Kim Na Young, which is strictly worse than what it
        // replaced. The rule is "the record's artist wins", and a placeholder
        // is the case where there isn't one.
        //
        // These two are *not* disjoint roles. A music director routinely sings
        // on their own soundtrack, so the same person can be both the record's
        // artist and a performer on some of its tracks. Nothing here labels a
        // role for that reason — the credit is a list of everyone named, in
        // record-artist-first order, and `credit_includes` keeps a name from
        // appearing twice when the two overlap.
        //
        // Deliberately not asked of MusicBrainz. Its coverage for this
        // repertoire is empty (no composer relations at all on the recordings
        // checked) and its artist credits disagree with themselves, sometimes
        // listing the director, sometimes the singers, sometimes both — with
        // duplicate recordings for each. Plex already knows, for free.
        let performer = self.track_artist.as_deref().map(str::trim).filter(|t| !t.is_empty());

        let (primary, artists) = match performer {
            // A compilation has no artist of its own, so the performer is it.
            // Keeping the placeholder in the credit would only assert that the
            // record is a compilation, which the album already says.
            Some(p) if crate::db::is_various_artists(&self.artist) => {
                (p.to_string(), vec![p.to_string()])
            }
            // The performers already name the record's artist — the common case
            // when a music director sings on their own soundtrack, which
            // Anirudh Ravichander, Ilaiyaraaja, Harris Jayaraj and A. R. Rahman
            // all do. The credit is then simply the fuller one, and listing
            // them separately would repeat a name.
            Some(p) if credit_includes(p, &self.artist) => {
                (self.artist.clone(), vec![p.to_string()])
            }
            Some(p) => (self.artist.clone(), vec![self.artist.clone(), p.to_string()]),
            None => (self.artist.clone(), vec![self.artist.clone()]),
        };

        Play {
            title: self.title.clone(),
            // Decided above, never `artists.first()`. The credit list can
            // legitimately start with the fuller performer string when the two
            // overlap, and deriving the primary from it would put the whole
            // combination in `artist` — the fragmentation this exists to stop.
            artist: primary,
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
            isrc: None,
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
    /// Plex's own account id for whoever played this — the *only* thing on a
    /// history row that says whose listen it was. A live session carries a
    /// `User` element with a name; a history row carries this number instead,
    /// and nothing else. Without it the history path cannot tell one member of
    /// a Plex Home from another. See `accounts_allow`.
    account_id: Option<i64>,
}

// ==================== Session Tracking ====================

#[derive(Debug, Clone)]
struct SessionState {
    rating_key: String,
    last_seen: u64,
    scrobbled: bool,
    /// The track itself, so the one that just got displaced can be written as a
    /// skip. Held here because by the time Plex reports a new `ratingKey` the
    /// old item is gone from the payload — this map is the only thing that
    /// still knows what was playing a poll ago.
    track: PlexTrack,
    /// Previous playhead, in ms. A track on repeat keeps the same `sessionKey`
    /// *and* the same `ratingKey`, so the only evidence that it started over is
    /// the playhead jumping backwards — see `process_live_track`.
    last_offset: u64,
}

// ==================== Filter Configuration ====================

#[derive(Debug, Clone, Default)]
pub struct PlexFilters {
    pub users_allow: Vec<String>,
    /// `users_allow` resolved to Plex account ids, filled by `initialize`.
    ///
    /// It exists because the two endpoints identify a listener differently: a
    /// session says `User title="Colin"`, a history row says
    /// `accountID="777942758"` and carries no name at all. One allow-list
    /// expressed in the name-space of the source config therefore has to be
    /// translated once, at connect, into the number-space history speaks.
    pub accounts_allow: Vec<i64>,
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
    /// When a listen counts, per the owner of this source. Plex keeps its own
    /// copy of the state machine rather than using `SessionTracker`, so it needs
    /// its own copy of the threshold too — but the *decision* is one shared
    /// method, so the two cannot drift.
    threshold: ScrobbleThreshold,
}

/// How often the Plex history endpoint is walked, independent of how often
/// sessions are read.
const HISTORY_SYNC_INTERVAL_SECS: u64 = 60;

/// Rows per request on the history walk. Plex's own default, and the value this
/// asked for as a hard `limit` before it learned to page.
const HISTORY_PAGE_SIZE: usize = 50;

/// Ceiling on one walk, so a server with a pathological history cannot make a
/// single tick unbounded — the mistake this whole area is being fixed for. At
/// the 7-day window this is far more listening than anyone does; reaching it
/// says so in the log rather than truncating in silence.
const HISTORY_MAX_ROWS: usize = 2_000;

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
    /// Tracks replaced before they ever counted. `view_offset` is how far in
    /// the listener got, which is the half that makes a skip mean anything.
    pub skipped: Vec<PlexTrack>,
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
            threshold: ScrobbleThreshold::default(),
        }
    }

    /// Adopt the owner's scrobble threshold. Same reasoning as
    /// `SessionTracker::set_threshold`: the setting can change under a source
    /// that is already running, and rebuilding the source to carry a number
    /// would discard the per-session state that decides what has been banked.
    pub fn set_threshold(&mut self, threshold: ScrobbleThreshold) {
        self.threshold = threshold;
    }

    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.libraries = self.fetch_libraries().await?;
        info!("Plex: Loaded {} libraries", self.libraries.len());
        self.resolve_allowed_accounts().await;
        Ok(())
    }

    /// Translate `users_allow` (names) into `accounts_allow` (Plex account ids).
    ///
    /// Failure here is deliberately **not** fatal and deliberately **not**
    /// ignored: the live path keys on the name and keeps working, while the
    /// history path fails closed (see `fetch_history_tracks`). Attributing a
    /// housemate's listen to the wrong person writes to a permanent public
    /// record — losing the between-polls catch-up until the operator fixes the
    /// token is the cheaper of the two failures by a wide margin.
    async fn resolve_allowed_accounts(&mut self) {
        if self.filters.users_allow.is_empty() {
            return;
        }
        match Self::list_accounts(&self.url, &self.token).await {
            Ok(accounts) => {
                self.filters.accounts_allow = accounts
                    .iter()
                    .filter(|a| self.filters.users_allow.contains(&a.name.trim().to_lowercase()))
                    .map(|a| a.id)
                    .collect();
                if self.filters.accounts_allow.is_empty() {
                    warn!(
                        "Plex: none of {:?} matched an account on this server ({} known) — \
                         history-derived plays will be skipped rather than misattributed",
                        self.filters.users_allow,
                        accounts.len()
                    );
                } else {
                    info!(
                        "Plex: history isolated to account id(s) {:?} for {:?}",
                        self.filters.accounts_allow, self.filters.users_allow
                    );
                }
            }
            Err(e) => {
                warn!(
                    "Plex: could not read /accounts ({}) — history-derived plays will be \
                     skipped, since there is no way to tell whose they are",
                    e
                );
            }
        }
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
    /// Find an artist in the library by name, returning their rating key.
    ///
    /// `/hubs/search` rather than a section filter: it ranks across the whole
    /// server and needs no section id, which `Library` doesn't carry. The match
    /// is checked rather than trusted — the hub is a *search*, so asking for
    /// "Cœur de pirate" also comes back with "Arkells", and taking the first
    /// result would hang someone else's face on the artist page.
    pub async fn find_artist(
        &self,
        name: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Resp {
            media_container: Container,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Container {
            #[serde(rename = "Hub", default)]
            hub: Vec<Hub>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Hub {
            #[serde(rename = "type", default)]
            kind: Option<String>,
            #[serde(rename = "Metadata", default)]
            metadata: Vec<Hit>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Hit {
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            rating_key: Option<String>,
        }

        let resp = self
            .client
            .get(format!("{}/hubs/search", self.url))
            .query(&[("query", name), ("limit", "10")])
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Plex search returned {}", resp.status()).into());
        }
        let body: Resp = resp.json().await?;

        Ok(body
            .media_container
            .hub
            .into_iter()
            .filter(|h| h.kind.as_deref() == Some("artist"))
            .flat_map(|h| h.metadata)
            .find(|m| m.title.as_deref().is_some_and(|t| artist_matches(t, name)))
            .and_then(|m| m.rating_key))
    }

    /// This server's `machineIdentifier`.
    ///
    /// Needed only to build a playlist: Plex takes its contents as a
    /// `server://<machineIdentifier>/…` URI rather than a list of ids, so a
    /// playlist cannot be created without asking the server what it calls
    /// itself. `/identity` is the cheapest endpoint that carries it.
    pub async fn machine_identifier(
        &self,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Resp {
            media_container: Container,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Container {
            #[serde(default)]
            machine_identifier: Option<String>,
        }

        let resp = self
            .client
            .get(format!("{}/identity", self.url))
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Plex identity returned {}", resp.status()).into());
        }
        let body: Resp = resp.json().await?;
        body.media_container
            .machine_identifier
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| "Plex did not report a machine identifier".into())
    }

    /// Tracks matching a free-text query, as candidates for a playlist push.
    ///
    /// Same endpoint and the same caution as `find_artist`: `/hubs/search` ranks
    /// across the whole server and will happily return something else entirely,
    /// so these are suggestions. `crate::playlists::pick` decides.
    pub async fn search_tracks(
        &self,
        query: &str,
    ) -> Result<Vec<crate::playlists::Candidate>, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Resp {
            media_container: Container,
        }
        #[derive(Deserialize)]
        struct Container {
            #[serde(rename = "Hub", default)]
            hub: Vec<Hub>,
        }
        #[derive(Deserialize)]
        struct Hub {
            #[serde(rename = "type", default)]
            kind: Option<String>,
            #[serde(rename = "Metadata", default)]
            metadata: Vec<Hit>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Hit {
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            rating_key: Option<String>,
            /// A track's artist. `originalTitle` is the per-track credit and
            /// wins where it exists, for the same reason the ingest path keeps
            /// it: on a compilation `grandparentTitle` is "Various Artists".
            #[serde(default)]
            grandparent_title: Option<String>,
            #[serde(default)]
            original_title: Option<String>,
            #[serde(default)]
            parent_title: Option<String>,
        }

        let resp = self
            .client
            .get(format!("{}/hubs/search", self.url))
            .query(&[("query", query), ("limit", "20")])
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Plex search returned {}", resp.status()).into());
        }
        let body: Resp = resp.json().await?;

        Ok(body
            .media_container
            .hub
            .into_iter()
            .filter(|h| h.kind.as_deref() == Some("track"))
            .flat_map(|h| h.metadata)
            .filter_map(|m| {
                Some(crate::playlists::Candidate {
                    id: m.rating_key?,
                    title: m.title?,
                    artist: m
                        .original_title
                        .filter(|s| !s.trim().is_empty())
                        .or(m.grandparent_title)
                        .unwrap_or_default(),
                    album: m.parent_title,
                })
            })
            .collect())
    }

    /// Create an audio playlist holding `rating_keys`, in that order.
    ///
    /// `smart=0` matters: a smart playlist stores a *filter* and Plex would
    /// re-evaluate it, so the fixed list Tapedeck just built would not be what
    /// the user ends up with.
    pub async fn create_playlist(
        &self,
        title: &str,
        rating_keys: &[String],
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if rating_keys.is_empty() {
            return Err("Nothing to put in the playlist".into());
        }
        let machine = self.machine_identifier().await?;
        let uri = format!(
            "server://{}/com.plexapp.plugins.library/library/metadata/{}",
            machine,
            rating_keys.join(",")
        );

        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Resp {
            media_container: Container,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Container {
            #[serde(rename = "Metadata", default)]
            metadata: Vec<Made>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Made {
            #[serde(default)]
            rating_key: Option<String>,
        }

        let resp = self
            .client
            .post(format!("{}/playlists", self.url))
            .query(&[("type", "audio"), ("title", title), ("smart", "0"), ("uri", uri.as_str())])
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Plex playlist create returned {}", resp.status()).into());
        }
        let body: Resp = resp.json().await?;
        body.media_container
            .metadata
            .into_iter()
            .find_map(|m| m.rating_key)
            .ok_or_else(|| "Plex created the playlist but reported no id".into())
    }

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
            self.recent_scrobbles.retain(|_, at| now.saturating_sub(*at) < HISTORY_SUPPRESS_SECS);
            // `.ok()` rather than `if let Ok(..)`: the error type is a bare
            // `Box<dyn Error>`, which is not `Send`, and binding the whole
            // Result keeps it alive across the hydrate await below — which is
            // enough to make the trait method's future non-`Send`.
            let history = self.fetch_history_tracks(0).await.ok();
            if let Some(history) = history {
                let fetched = history.len();
                // Drop anything the live path already banked. History is the
                // fallback for plays that finished between polls, not a second
                // opinion on plays we watched happen — and its rows carry the
                // album artist and no quality metadata, so letting one through
                // is a visibly worse duplicate of a row we already have.
                let mut fresh: Vec<PlexTrack> = history
                    .into_iter()
                    .filter(|t| !self.recent_scrobbles.contains_key(&t.rating_key))
                    .collect();
                info!(
                    "Fetched {} tracks from Plex history, {} new ({} already banked live)",
                    fetched,
                    fresh.len(),
                    fetched - fresh.len()
                );
                // Only the rows that survive suppression are worth a lookup, and
                // at a 5s session cadence that is close to nothing — this is the
                // catch-up path, not the common one.
                for track in &mut fresh {
                    self.hydrate_from_library(track).await;
                }
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
                // Replaced before it ever counted: the listener moved on from
                // it deliberately. A track that *did* cross is already banked
                // and is simply over. Same rule as `SessionTracker`, which the
                // other session sources share — this is Plex's own copy of that
                // state machine, so it needs its own copy of the rule.
                if !state.scrobbled {
                    let mut gone = state.track.clone();
                    gone.view_offset = Some(state.last_offset);
                    result.skipped.push(gone);
                }
                self.session_states.remove(&session_key);
            }
        }

        // Copied out before the entry below borrows `self` mutably. It is
        // `Copy`, so this is two numbers, not a clone of anything.
        let threshold = self.threshold;

        let (should_scrobble, needs_np) = {
            let state =
                self.session_states.entry(session_key.clone()).or_insert_with(|| SessionState {
                    rating_key: rating_key.clone(),
                    last_seen: now,
                    scrobbled: false,
                    last_offset: view_offset,
                    track: track.clone(),
                });
            state.last_seen = now;
            state.track = track.clone();

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

            // Plex works in milliseconds throughout; the threshold works in
            // seconds, like every other source.
            let seconds = view_offset / 1000;
            let length = (duration > 0).then_some(duration / 1000);

            let ready = !state.scrobbled && threshold.reached(seconds, length);
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
                        account_id: None,
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
                            b"accountID" => track.account_id = val.parse().ok(),
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

    /// Fill in the duration and audio quality a history row does not carry.
    ///
    /// `/status/sessions/history/all` returns *no* `duration` and *no* `Media`
    /// element — verified against a live 1.42 server: a history row has 19
    /// attributes and neither is among them. So a listen banked from history
    /// landed with no running time and no codec, which is the whole of the
    /// "one track on the album lost its FLAC badge" report: the badge and the
    /// duration column go blank together, on exactly the tracks the live path
    /// missed (finished between polls, or the player stopped before the
    /// halfway gate).
    ///
    /// `/library/metadata/<ratingKey>` has both, and it describes the *file*
    /// rather than a playback session — which is the honest source for this.
    /// The one thing it cannot know is whether Plex transcoded on the way out;
    /// that is a property of a session that has already ended, so a
    /// history-derived listen keeps `is_transcoded` unset rather than claiming
    /// a direct play it cannot vouch for.
    async fn hydrate_from_library(&self, track: &mut PlexTrack) {
        let endpoint = format!("{}/library/metadata/{}", self.url, track.rating_key);
        let resp = match self
            .client
            .get(&endpoint)
            .header("X-Plex-Token", &self.token)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                debug!("Plex: metadata for {} returned {}", track.rating_key, r.status());
                return;
            }
            Err(e) => {
                debug!("Plex: metadata for {} failed: {}", track.rating_key, e);
                return;
            }
        };

        let Ok(parsed) = resp.json::<SessionsResponse>().await else {
            debug!("Plex: metadata for {} did not parse", track.rating_key);
            return;
        };
        let Some(meta) = parsed.media_container.metadata.into_iter().next() else {
            return;
        };

        if track.duration.is_none() {
            track.duration = meta.duration;
        }
        if track.audio_quality.is_none() {
            track.audio_quality = PlexTrack::extract_quality(&meta);
        }
    }

    /// Walk `/status/sessions/history/all` newest-first until it reaches
    /// something older than the window, then stop.
    ///
    /// **It used to ask for one page of 50 and stop there**, which is fine at a
    /// 60-second cadence and silently wrong after any gap: this walk exists to
    /// catch plays that finished between polls, and it was structurally
    /// incapable of catching more than fifty of them. On 2026-08-24 the tick
    /// stretched to 45–92 minutes and this page came back saturated at exactly
    /// fifty rows twenty-eight times — within roughly 2× of losing listens
    /// outright, with nothing anywhere saying so.
    ///
    /// Paging costs nothing in the ordinary case: at a healthy cadence the
    /// first page already reaches past the window at its bottom, so the loop
    /// stops after one request exactly as before. It spends more only when
    /// there is more to find, which is the case it exists for. Overlap is free
    /// — `source_id` is derived from the row, so `save_scrobble`'s uniqueness
    /// absorbs a row seen twice. Prefer a few redundant rows to a silent hole,
    /// which is the rule `navidrome::resume_point` already follows.
    async fn fetch_history_tracks(
        &mut self,
        _min_timestamp: u64,
    ) -> Result<Vec<PlexTrack>, Box<dyn std::error::Error>> {
        let endpoint = format!("{}/status/sessions/history/all", self.url);

        let seven_days_ago = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(7 * 86400);

        let mut history_tracks: Vec<HistoryTrack> = Vec::new();
        let mut start = 0usize;
        let mut pages = 0usize;
        loop {
            let resp = self
                .client
                .get(&endpoint)
                .query(&[("sort", "viewedAt:desc")])
                // Plex pages with these two headers rather than with `limit`.
                .header("X-Plex-Container-Start", start.to_string())
                .header("X-Plex-Container-Size", HISTORY_PAGE_SIZE.to_string())
                .header("X-Plex-Token", &self.token)
                .header("Accept", "application/xml")
                .send()
                .await?;

            if !resp.status().is_success() {
                warn!("History fetch failed: HTTP {}", resp.status());
                // Whatever earlier pages produced is still good — returning it
                // beats throwing the walk away because page four timed out.
                break;
            }

            let text = resp.text().await?;
            let page = Self::parse_history_xml(&text);
            let got = page.len();
            pages += 1;
            let more = Self::wants_another_page(&page, seven_days_ago, start + got);
            history_tracks.extend(page);

            if !more {
                if start + got >= HISTORY_MAX_ROWS {
                    warn!(
                        "Plex history walk stopped at its {}-row ceiling without reaching the \
                         7-day edge — anything older is not being caught up",
                        HISTORY_MAX_ROWS
                    );
                }
                break;
            }
            start += got;
        }

        info!("Processing {} history tracks from Plex ({} page(s))", history_tracks.len(), pages);
        let isolated = !self.filters.users_allow.is_empty();
        Ok(Self::own_history_tracks(
            history_tracks,
            isolated,
            &self.filters.accounts_allow,
            seven_days_ago,
        ))
    }

    /// Is another page of history worth asking for?
    ///
    /// Split out and pure for the reason `own_history_tracks` and
    /// `consume_sessions` are: it decides whether a listen is ever seen at all,
    /// and a rule like that must be testable without a server. The old code had
    /// no rule — it asked for fifty rows and stopped — so the answer to "did we
    /// catch everything?" was "only if fewer than fifty things happened", and
    /// nothing ever said which.
    ///
    /// Three ways to stop, and each is a different fact:
    ///
    /// * **A short page** is the end of the server's history. Nothing left.
    /// * **A page reaching past the window** is the end of what is wanted. Rows
    ///   come back `viewedAt:desc`, so once one predates the floor every row
    ///   after it does too.
    /// * **The ceiling** is the safety net, and the only one that is a *loss* —
    ///   so it is the only one the caller logs about.
    fn wants_another_page(page: &[HistoryTrack], window_floor: u64, rows_so_far: usize) -> bool {
        if page.len() < HISTORY_PAGE_SIZE {
            return false;
        }
        if rows_so_far >= HISTORY_MAX_ROWS {
            return false;
        }
        // A row with no `viewedAt` cannot place itself in time, so it is not
        // evidence that the edge has been reached — erring towards one more
        // request rather than towards a silent hole.
        !page.iter().filter_map(|t| t.viewed_at).any(|v| v < window_floor)
    }

    /// Keep the history rows that are this source's own, recent, and tracks.
    ///
    /// Split out of `fetch_history_tracks` so the ownership rule is testable
    /// without a server — the same reason `consume_sessions` was split out of
    /// `fetch_sessions`. It is state-free but it decides whose permanent public
    /// record a play lands on, which is the worst thing in this codebase to get
    /// wrong silently.
    ///
    /// `isolated` means the source is pinned to a Plex account (which every
    /// source built by `pipeline::build_source` is). When it is set and
    /// `accounts_allow` is empty, the allow-list could not be resolved to an
    /// id — so ownership is unknowable and **everything is dropped**. Banking
    /// them instead would attribute a housemate's listening to whoever's source
    /// row happened to walk the history first.
    fn own_history_tracks(
        history_tracks: Vec<HistoryTrack>,
        isolated: bool,
        accounts_allow: &[i64],
        oldest_allowed: u64,
    ) -> Vec<PlexTrack> {
        if isolated && accounts_allow.is_empty() {
            warn!(
                "Plex: skipping {} history rows — the allowed account id is unknown, so \
                 whose plays these are cannot be established",
                history_tracks.len()
            );
            return Vec::new();
        }

        let mut tracks = Vec::new();
        let mut foreign = 0usize;
        for h_track in history_tracks {
            if h_track.media_type.as_deref() != Some("track") {
                continue;
            }
            // A row with no `accountID` at all is unowned, not ours. Same rule
            // as an id belonging to someone else — an absent field must not be
            // the one gap that reopens this.
            if isolated && !h_track.account_id.is_some_and(|id| accounts_allow.contains(&id)) {
                foreign += 1;
                continue;
            }
            if let Some(ts) = h_track.viewed_at {
                if ts < oldest_allowed {
                    debug!("Skipping old history track (viewed_at: {})", ts);
                    continue;
                }
            }
            if let Some(track) = PlexTrack::from_history(h_track) {
                tracks.push(track);
            }
        }
        if foreign > 0 {
            debug!("Plex: {} history row(s) belonged to another account", foreign);
        }
        tracks
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
    fn set_scrobble_threshold(&mut self, threshold: crate::models::ScrobbleThreshold) {
        self.threshold = threshold;
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
            plex_account_id: None,
            history_key: None,
            player: None,
        }
    }

    fn hrow(viewed_at: Option<u64>) -> HistoryTrack {
        HistoryTrack {
            media_type: Some("track".into()),
            viewed_at,
            history_key: None,
            title: Some("Xtal".into()),
            artist: Some("Aphex Twin".into()),
            album: None,
            track_artist: None,
            duration: None,
            rating_key: Some("rk1".into()),
            parent_rating_key: None,
            grandparent_rating_key: None,
            account_id: Some(1),
        }
    }

    const DAY: u64 = 86_400;
    /// An arbitrary window floor, standing in for "seven days ago".
    const FLOOR: u64 = 1_000_000;

    /// The case reported on 2026-08-24, and the reason this paginates at all.
    ///
    /// The tick had stretched to 45–92 minutes, so a catch-up legitimately had
    /// ~70 plays waiting. One page of fifty came back **full**, which the old
    /// code read as "done" — the other twenty were never asked for and nothing
    /// anywhere said so. A saturated page still inside the window means there
    /// is more behind it.
    #[test]
    fn a_full_page_inside_the_window_is_not_the_end_of_the_backfill() {
        let page: Vec<HistoryTrack> =
            (0..HISTORY_PAGE_SIZE).map(|i| hrow(Some(FLOOR + DAY + i as u64))).collect();
        assert!(
            PlexSource::wants_another_page(&page, FLOOR, page.len()),
            "fifty rows all inside the window means there are probably more"
        );
    }

    /// And the ordinary case must stay one request: at a healthy cadence the
    /// first page already reaches past the window, so paging costs nothing.
    #[test]
    fn a_page_reaching_past_the_window_stops_the_walk() {
        let mut page: Vec<HistoryTrack> =
            (0..HISTORY_PAGE_SIZE - 1).map(|i| hrow(Some(FLOOR + DAY + i as u64))).collect();
        page.push(hrow(Some(FLOOR - 1))); // older than the floor
        assert!(!PlexSource::wants_another_page(&page, FLOOR, page.len()));
    }

    /// A short page is the end of the server's history, whatever its dates.
    #[test]
    fn a_short_page_is_the_end_of_the_history() {
        let page: Vec<HistoryTrack> = (0..3).map(|i| hrow(Some(FLOOR + DAY + i as u64))).collect();
        assert!(!PlexSource::wants_another_page(&page, FLOOR, page.len()));
    }

    /// The ceiling bounds one walk, so a pathological history cannot make a
    /// single tick unbounded — the failure this whole area is being fixed for.
    #[test]
    fn the_walk_has_a_ceiling_so_one_tick_cannot_run_away() {
        let page: Vec<HistoryTrack> =
            (0..HISTORY_PAGE_SIZE).map(|i| hrow(Some(FLOOR + DAY + i as u64))).collect();
        assert!(!PlexSource::wants_another_page(&page, FLOOR, HISTORY_MAX_ROWS));
    }

    /// A row with no `viewedAt` cannot place itself in time, so it is not
    /// evidence that the edge was reached. Erring towards one more request
    /// beats erring towards a silent hole — the rule `resume_point` follows.
    #[test]
    fn a_row_with_no_timestamp_does_not_end_the_walk() {
        let mut page: Vec<HistoryTrack> =
            (0..HISTORY_PAGE_SIZE - 1).map(|i| hrow(Some(FLOOR + DAY + i as u64))).collect();
        page.push(hrow(None));
        assert!(PlexSource::wants_another_page(&page, FLOOR, page.len()));
    }

    /// A track dropped before it counted is a skip, and says how far in.
    ///
    /// Plex keeps its own copy of the session state machine rather than using
    /// `SessionTracker`, so it needs its own copy of this rule — and its own
    /// test, because the two can drift without anything erroring.
    #[tokio::test]
    async fn a_track_dropped_before_the_threshold_is_recorded_as_a_skip() {
        let mut src = PlexSource::new("http://localhost:32400".into(), "t".into());

        // Twenty seconds into a five-minute track, then the listener moves on.
        let mut r = SessionResult::default();
        src.process_live_track(track(20_000), true, &mut r).await;
        assert!(r.skipped.is_empty(), "nothing has been displaced yet");

        let mut next = track(0);
        next.rating_key = "rk2".into();
        next.title = "Ageispolis".into();
        let mut r = SessionResult::default();
        src.process_live_track(next, true, &mut r).await;

        assert_eq!(r.skipped.len(), 1, "the displaced track is a skip");
        assert_eq!(r.skipped[0].title, "Xtal");
        assert_eq!(
            r.skipped[0].view_offset,
            Some(20_000),
            "how far in they got, not where the new track is"
        );
        assert!(r.ready_to_scrobble.is_empty());
    }

    /// A track that crossed the threshold is over, not skipped — otherwise
    /// every completed track in a queue would be filed as one.
    #[tokio::test]
    async fn a_banked_plex_track_is_not_also_a_skip() {
        let mut src = PlexSource::new("http://localhost:32400".into(), "t".into());

        let mut r = SessionResult::default();
        src.process_live_track(track(160_000), true, &mut r).await;
        assert_eq!(r.ready_to_scrobble.len(), 1);

        let mut next = track(0);
        next.rating_key = "rk2".into();
        let mut r = SessionResult::default();
        src.process_live_track(next, true, &mut r).await;
        assert!(r.skipped.is_empty(), "a banked listen is over, not abandoned");
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

    /// Ligatures and capitals are exactly what a media server and a scrobble
    /// tag disagree about — "Cœur de pirate" against "Coeur de Pirate" — and
    /// getting this wrong puts a stranger's photograph on an artist page.
    #[test]
    fn artist_names_match_across_ligatures_and_case() {
        assert!(artist_matches("Cœur de pirate", "Coeur de Pirate"));
        assert!(artist_matches("cœur de pirate", "Cœur De Pirate"));
        assert!(artist_matches("Sigur Rós", "Sigur Rós"));
        assert!(artist_matches("A.R. Rahman", "A R Rahman"));
        // Different artists must not collapse into one another.
        assert!(!artist_matches("Cœur de pirate", "Arkells"));
        assert!(!artist_matches("Karthik", "Karthika"));
        assert!(!artist_matches("", ""));
    }

    /// A film soundtrack names two different things and both are wanted: the
    /// album artist is the music director, the track artist is the singers.
    ///
    /// The director is primary — the live path used to take the singers while
    /// the history path took the director, which is how one play became two
    /// rows under two names and then two MusicBrainz recordings. Making both
    /// paths agree is half of that fix; keeping the performers in the credit is
    /// what stops it being a loss.
    #[test]
    fn a_soundtrack_keeps_the_director_and_the_singers() {
        let mut t = track(0);
        t.title = "Vaarayo Vaarayo".into();
        t.artist = "Harris Jayaraj".into();
        t.track_artist = Some("Unnikrishnan, Chinmayi & Megha".into());
        t.album = Some("Aadhavan".into());

        let play = t.to_play("np");
        assert_eq!(play.artist, "Harris Jayaraj", "the director is what charts and scrobbles use");
        assert_eq!(
            play.artists.as_deref(),
            Some(&["Harris Jayaraj".to_string(), "Unnikrishnan, Chinmayi & Megha".to_string()][..]),
            "the singers are kept alongside, not discarded"
        );
    }

    /// "Various Artists" is not an artist — it is a placeholder meaning *look
    /// at the track*. Taking it as the record's artist recorded the whole of
    /// "Do You Like Brahms?" under it instead of Punch and Kim Na Young, which
    /// was strictly worse than the behaviour it replaced.
    #[test]
    fn a_compilation_credits_the_performer_not_the_placeholder() {
        let mut t = track(0);
        t.artist = "Various Artists".into();
        t.album = Some("Do You Like Brahms? (Original Television Soundtrack)".into());
        t.track_artist = Some("Punch".into());

        let play = t.to_play("np");
        assert_eq!(play.artist, "Punch", "the performer is the artist on a compilation");
        assert_eq!(
            play.artists.as_deref(),
            Some(&["Punch".to_string()][..]),
            "and the placeholder is not kept — the album already says it is a compilation"
        );

        // With no performer named there is nothing better to use, so the
        // placeholder stands rather than the listen being dropped.
        t.track_artist = None;
        assert_eq!(t.to_play("np").artist, "Various Artists");
    }

    /// The forms that actually appear, and the ones that must not match —
    /// this decides which of two real names wins.
    #[test]
    fn various_artists_placeholders_are_recognised_narrowly() {
        for yes in ["Various Artists", "various artists", "  VARIOUS  ", "VA", "Various Artist"] {
            assert!(crate::db::is_various_artists(yes), "{yes:?} should be a placeholder");
        }
        for no in ["Various Production", "Varius Manx", "The Various", "V.A.S.", ""] {
            assert!(!crate::db::is_various_artists(no), "{no:?} is a real name");
        }
    }

    /// A music director singing on their own soundtrack is normal — Anirudh
    /// Ravichander, Ilaiyaraaja, Harris Jayaraj and A. R. Rahman all do it. The
    /// record's artist and the track's performers overlap rather than being
    /// disjoint roles, so the credit must not name the same person twice.
    #[test]
    fn a_director_who_sings_is_not_credited_twice() {
        let mut t = track(0);
        t.artist = "Anirudh Ravichander".into();
        t.album = Some("Vikram".into());

        // Sings it with someone else: the fuller credit already names him.
        t.track_artist = Some("Anirudh Ravichander & Shruti Haasan".into());
        let play = t.to_play("np");
        assert_eq!(play.artist, "Anirudh Ravichander");
        assert_eq!(
            play.artists.as_deref(),
            Some(&["Anirudh Ravichander & Shruti Haasan".to_string()][..]),
            "the record artist is already in the credit — don't repeat him"
        );

        // Sings it alone.
        t.track_artist = Some("Anirudh Ravichander".into());
        assert_eq!(
            t.to_play("np").artists.as_deref(),
            Some(&["Anirudh Ravichander".to_string()][..])
        );

        // Doesn't sing this one: both names, record artist first.
        t.track_artist = Some("Shreya Ghoshal".into());
        assert_eq!(
            t.to_play("np").artists.as_deref(),
            Some(&["Anirudh Ravichander".to_string(), "Shreya Ghoshal".to_string()][..])
        );
    }

    /// The containment check is on whole names — a shorter name that happens to
    /// sit inside a longer one is a different person.
    #[test]
    fn credit_containment_respects_name_boundaries() {
        assert!(credit_includes("Anirudh Ravichander & Shruti Haasan", "Anirudh Ravichander"));
        assert!(credit_includes("Shreya Ghoshal, Karthik", "Karthik"));
        assert!(!credit_includes("Karthika", "Karthik"), "Karthika is not Karthik");
        assert!(!credit_includes("Shreya Ghoshal", "Anirudh Ravichander"));
        assert!(!credit_includes("Anything", ""));
    }

    /// Most records name one artist twice over, and that must not become a
    /// credit listing the same name twice.
    #[test]
    fn a_single_artist_record_gets_no_redundant_credit() {
        let mut t = track(0);
        t.artist = "Aphex Twin".into();

        // No track artist at all.
        let play = t.to_play("np");
        assert_eq!(play.artist, "Aphex Twin");
        assert_eq!(play.artists.as_deref(), Some(&["Aphex Twin".to_string()][..]));

        // Track artist present but identical — Plex does send this.
        t.track_artist = Some("Aphex Twin".into());
        let play = t.to_play("np");
        assert_eq!(play.artists.as_deref(), Some(&["Aphex Twin".to_string()][..]));
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

    /// Two rows captured from a live Plex 1.42 server on 2026-08-11: one played
    /// by the server owner (`accountID="1"`) and one by a Plex Home member
    /// (`accountID="777942758"`). Both are real; the second is the play that
    /// was being attributed to the wrong person.
    ///
    /// Note what is *not* in either row: no `duration`, no `Media` element, and
    /// no user name anywhere. That is the whole reason both bugs existed.
    const REAL_HISTORY_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MediaContainer size="2">
<Track historyKey="/status/sessions/history/14747" key="/library/metadata/155345" ratingKey="155345" librarySectionID="3" title="Bom diggy diggy" parentTitle="Sonu Ke Titu Ki Sweety" grandparentTitle="Various Artists" type="track" index="4" parentIndex="1" viewedAt="1786478964" accountID="1" deviceID="90" />
<Track historyKey="/status/sessions/history/14317" key="/library/metadata/109501" ratingKey="109501" librarySectionID="3" title="Shape of You" parentTitle="&#247;" grandparentTitle="Ed Sheeran" type="track" index="4" parentIndex="1" viewedAt="1786300696" accountID="777942758" deviceID="86" />
</MediaContainer>"#;

    #[test]
    fn a_history_row_carries_the_account_that_played_it() {
        let rows = PlexSource::parse_history_xml(REAL_HISTORY_XML);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].account_id, Some(1));
        assert_eq!(rows[1].account_id, Some(777_942_758));
        // The fields that make a history-derived listen worse than a live one.
        assert_eq!(rows[1].duration, None);
        assert_eq!(rows[1].title.as_deref(), Some("Shape of You"));
    }

    /// The bug this exists to stop: a Plex Home member's play landing in
    /// somebody else's history. Both people poll the same server with their own
    /// token, so without this each one's history walk returns the other's plays
    /// and both accounts end up with both.
    #[test]
    fn another_home_members_play_is_not_mine() {
        let rows = PlexSource::parse_history_xml(REAL_HISTORY_XML);
        let mine = PlexSource::own_history_tracks(rows, true, &[1], 0);
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].title, "Bom diggy diggy");
        assert_eq!(mine[0].plex_account_id, Some(1));
    }

    /// …and the mirror image, from the other member's source row.
    #[test]
    fn my_play_is_not_another_home_members() {
        let rows = PlexSource::parse_history_xml(REAL_HISTORY_XML);
        let theirs = PlexSource::own_history_tracks(rows, true, &[777_942_758], 0);
        assert_eq!(theirs.len(), 1);
        assert_eq!(theirs[0].title, "Shape of You");
    }

    /// An unresolved allow-list fails closed. Losing the between-polls catch-up
    /// is recoverable; writing a housemate's listen to someone's public
    /// scrobble history is not.
    #[test]
    fn an_unresolvable_account_banks_nothing_rather_than_guessing() {
        let rows = PlexSource::parse_history_xml(REAL_HISTORY_XML);
        assert!(PlexSource::own_history_tracks(rows, true, &[], 0).is_empty());
    }

    /// A row with no `accountID` is unowned, not ours — an absent field must
    /// not be the gap that reopens the leak.
    #[test]
    fn a_row_with_no_account_is_not_claimed() {
        let xml = r#"<MediaContainer size="1">
<Track ratingKey="1" title="Xtal" grandparentTitle="Aphex Twin" type="track" viewedAt="1786478964" />
</MediaContainer>"#;
        let rows = PlexSource::parse_history_xml(xml);
        assert_eq!(rows[0].account_id, None);
        assert!(PlexSource::own_history_tracks(rows, true, &[1], 0).is_empty());
    }

    /// A source with no allow-list at all keeps the old unfiltered behaviour,
    /// so a single-user install is not broken by the isolation.
    #[test]
    fn an_unisolated_source_still_walks_the_whole_history() {
        let rows = PlexSource::parse_history_xml(REAL_HISTORY_XML);
        assert_eq!(PlexSource::own_history_tracks(rows, false, &[], 0).len(), 2);
    }

    /// The FLAC-badge drop. A history row has no `Media`, so the listen landed
    /// with no codec *and* no running time — the two blanks always arrive
    /// together, which is what the screenshots showed. `/library/metadata/:id`
    /// is where both live; this pins that the response shape parses and that
    /// `extract_quality` reads it. Body captured from the live server for
    /// ratingKey 109501, the same track as the history row above.
    #[test]
    fn library_metadata_supplies_what_a_history_row_lacks() {
        let body = r#"{"MediaContainer":{"Metadata":[{
            "ratingKey":"109501","type":"track","title":"Shape of You","duration":233720,
            "Media":[{"bitrate":1637,"audioCodec":"flac","audioChannels":2,"container":"flac",
              "Part":[{"container":"flac","Stream":[
                {"streamType":2,"codec":"flac","bitrate":1637,"samplingRate":44100,"bitDepth":24,"channels":2}
              ]}]}]
        }]}}"#;
        let parsed: SessionsResponse = serde_json::from_str(body).unwrap();
        let meta = parsed.media_container.metadata.into_iter().next().unwrap();
        assert_eq!(meta.duration, Some(233_720));

        let q = PlexTrack::extract_quality(&meta).expect("quality from library metadata");
        assert_eq!(q.codec.as_deref(), Some("flac"));
        assert_eq!(q.is_lossless, Some(true));
        assert_eq!(q.sample_rate, Some(44_100));
        assert_eq!(q.bit_depth, Some(24));
    }
}
