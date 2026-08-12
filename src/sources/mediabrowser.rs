//! Jellyfin and Emby, which are one protocol with a prefix on it.
//!
//! Jellyfin is a 2018 fork of Emby 3.5.2. The session shape, the `X-Emby-Token`
//! header (Jellyfin kept the name) and the .NET tick encoding are all
//! inheritance rather than coincidence, so both servers are driven from one
//! client parameterised by [`Flavour`].
//!
//! **The closeness is the trap.** Emby has developed independently for seven
//! years, and every field below is one that goes quietly `None` on a spelling
//! change — no error, no log, just a codec that is always absent or a device
//! that never registers. That is the exact failure class the Jellyfin tests were
//! written to pin, which is why this module has **separate tests per server**
//! rather than one set assumed to cover both. See `emby.rs` for what is
//! confirmed against a real server and what is not.
//!
//! Four differences are handled here:
//!
//! * **Base path.** Emby serves everything under `/emby/`; Jellyfin serves the
//!   same routes at the root.
//! * **Playlist creation.** Emby takes query parameters with a comma-joined
//!   `Ids`; Jellyfin deprecated those in favour of a JSON body with an array.
//!   Emby never did, so each gets its own call shape.
//! * **Item queries are user-scoped on Emby** (`/emby/Users/<id>/Items`), where
//!   Jellyfin accepts a bare `/Items`.
//! * **`source_name`**, so a listen records which server it came off.
//!
//! Ticks are .NET "ticks" (100-nanosecond units), which is why every duration
//! and position here is divided by ten million rather than a thousand.

use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

use crate::models::{AudioQuality, Play, ScrobbleThreshold};

/// .NET ticks per second — both servers report positions and durations in these.
pub(crate) const TICKS_PER_SECOND: u64 = 10_000_000;

/// How far the playhead must jump backwards before it counts as the track
/// starting over rather than the user scrubbing.
///
/// A repeat always lands near zero, so this only has to clear ordinary seek-back
/// noise. The number is about what a *person* means by "I skipped back a bit",
/// not about the precision of the source.
const REPEAT_REWIND_SECS: u64 = 20;

/// Which of the two servers this client is talking to.
///
/// Everything that differs between Jellyfin and Emby is a method on this, so
/// adding a third fork means adding a variant rather than threading booleans
/// through the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Jellyfin,
    Emby,
}

impl Flavour {
    /// Prefix every route with this. Emby namespaces its whole API under
    /// `/emby`; Jellyfin serves the same routes at the root.
    pub fn base(&self) -> &'static str {
        match self {
            Flavour::Jellyfin => "",
            Flavour::Emby => "/emby",
        }
    }

    /// Goes into `Play::source_name` and the `source_id` prefix, so a listen
    /// records which server it came off and the two never collide in `exists`.
    pub fn source_name(&self) -> &'static str {
        match self {
            Flavour::Jellyfin => "jellyfin",
            Flavour::Emby => "emby",
        }
    }

    /// The user-visible label, which is also the key the now-playing registry
    /// stores under.
    pub fn label(&self) -> &'static str {
        match self {
            Flavour::Jellyfin => "Jellyfin",
            Flavour::Emby => "Emby",
        }
    }
}

/// An account on the server. Sessions are matched on `name`, which is what
/// `SessionInfo.UserName` carries.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct MediaBrowserAccount {
    pub id: String,
    pub name: String,
}

pub struct MediaBrowserSource {
    flavour: Flavour,
    url: String,
    api_key: String,
    /// Which account this source belongs to. Everything from any other account
    /// is discarded — a shared server must not leak between users.
    account: String,
    client: Client,
    /// Per-session progress, so a track is scrobbled once rather than every
    /// poll.
    tracker: SessionTracker,
}

/// The three gates a repeat has to clear, held across polls.
///
/// Split out from the client because it is the part that fails *silently*: the
/// behaviour is state carried between calls, so a wrong answer is a missing
/// listen rather than an error. Shared by both flavours and by any future
/// session-shaped source — the same state machine Plex has, and the one that
/// took three attempts to get right.
#[derive(Debug, Default)]
pub struct SessionTracker {
    seen: HashMap<String, SessionProgress>,
    /// When a listen counts. Its own field rather than a constant because it is
    /// the listener's call — see `ScrobbleThreshold`. `Default` is the
    /// convention every source used to hardcode, so a tracker built and never
    /// told anything behaves exactly as it always did.
    threshold: ScrobbleThreshold,
}

#[derive(Debug, Clone)]
struct SessionProgress {
    item_id: String,
    scrobbled: bool,
    /// Last playhead seen, so a track starting over can be told from a seek.
    last_position: u64,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adopt the owner's threshold. Called by the engine rather than at
    /// construction: the setting can change under a source that is already
    /// running, and rebuilding the source to carry a number would throw away
    /// the state machine this struct exists to hold.
    pub fn set_threshold(&mut self, threshold: ScrobbleThreshold) {
        self.threshold = threshold;
    }

    /// Record where a session is, and answer whether this pass just crossed the
    /// scrobble threshold.
    ///
    /// Returns true exactly once per play. The three gates, in order:
    ///
    /// 1. **A new item in the same session resets the flag**, or only the first
    ///    song of a queue would ever count.
    /// 2. **A real rewind resets it too.** A track on repeat keeps the same
    ///    session *and* the same item, so the item check above cannot see it and
    ///    `scrobbled` would stay true forever. The playhead jumping backwards is
    ///    the one signal that the track started over. Requiring that this pass
    ///    was *already* banked is what stops scrubbing minting extra scrobbles:
    ///    a rewind before the threshold changes nothing, because there was
    ///    nothing to undo.
    /// 3. **A paused track never crosses.** It is still "now playing" for the
    ///    deck, but its position is not advancing.
    pub fn observe(
        &mut self,
        session_id: &str,
        item_id: &str,
        item_name: &str,
        position: u64,
        duration: Option<u64>,
        paused: bool,
    ) -> bool {
        let entry = self.seen.entry(session_id.to_string()).or_insert_with(|| SessionProgress {
            item_id: item_id.to_string(),
            scrobbled: false,
            last_position: position,
        });

        if entry.item_id != item_id {
            entry.item_id = item_id.to_string();
            entry.scrobbled = false;
        } else if entry.last_position > position
            && entry.last_position - position >= REPEAT_REWIND_SECS
        {
            if entry.scrobbled {
                debug!(
                    "Playhead rewound in session {} ({}s -> {}s): '{}' started over",
                    session_id, entry.last_position, position, item_name
                );
            }
            entry.scrobbled = false;
        }
        entry.last_position = position;

        let past_threshold = !paused && self.threshold.reached(position, duration);

        if past_threshold && !entry.scrobbled {
            entry.scrobbled = true;
            return true;
        }
        false
    }

    /// Forget sessions that have gone away, so the map doesn't grow forever and
    /// a returning session starts clean.
    pub fn retain_live(&mut self, live: &[String]) {
        self.seen.retain(|id, _| live.contains(id));
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct MbSession {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub now_playing_item: Option<NowPlayingItem>,
    #[serde(default)]
    pub play_state: Option<PlayState>,
    #[serde(default)]
    pub device_name: Option<String>,
    /// Stable per installed client — kept across restarts, which is what makes
    /// it usable as a `devices.machine_id`.
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub device_type: Option<String>,
    /// The app: "Jellyfin Media Player", "Finamp", "Emby for Android".
    #[serde(default)]
    pub client: Option<String>,
    /// Present **only while actually transcoding** — left null for a direct
    /// play, so its absence is itself the answer.
    #[serde(default)]
    pub transcoding_info: Option<TranscodingInfo>,
}

/// The delivery half: what the server actually sent, as opposed to what is on
/// disk. Null unless a transcode is running.
///
/// Only the fields Tapedeck has somewhere to put are parsed. `Container`,
/// `AudioChannels` and the video ones are deliberately left out: storing the
/// delivery container or a downmix would need columns that don't exist, and
/// parsing a field with nowhere to put it is how the five dropped quality fields
/// happened in the first place. Add the column first.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct TranscodingInfo {
    #[serde(default)]
    pub audio_codec: Option<String>,
    #[serde(default)]
    pub bitrate: Option<i32>,
    /// True when the audio is passed through untouched even though a transcode
    /// is running for some other reason.
    #[serde(default)]
    pub is_audio_direct: Option<bool>,
    /// An array of enum names in the Jellyfin 12.x schema, a comma-joined string
    /// in 10.8. Accepting one shape would be silently wrong on the other, and
    /// the symptom — an empty `transcode_reason` — looks exactly like a direct
    /// play.
    #[serde(default, deserialize_with = "string_or_seq")]
    pub transcode_reasons: Option<Vec<String>>,
}

/// Deserialise `"A, B"` or `["A", "B"]` into the same `Vec<String>`.
fn string_or_seq<'de, D>(d: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(Option::<OneOrMany>::deserialize(d)?.map(|v| match v {
        OneOrMany::One(s) => {
            s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
        }
        OneOrMany::Many(v) => v,
    }))
}

/// The client an item is playing on, in the shape `devices` wants.
#[derive(Debug, Clone)]
pub struct MediaBrowserPlayer {
    pub machine_id: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub product: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct NowPlayingItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "Type", default)]
    pub item_type: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub artists: Option<Vec<String>>,
    #[serde(default)]
    pub run_time_ticks: Option<u64>,
    #[serde(default)]
    pub index_number: Option<i32>,
    #[serde(default)]
    pub provider_ids: Option<ProviderIds>,
    #[serde(default)]
    pub media_streams: Option<Vec<MediaStream>>,
    /// The source container as it sits on disk — `flac`, `dsf`, `m4a`.
    #[serde(default)]
    pub container: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProviderIds {
    /// Jellyfin's own tag for the MusicBrainz recording id. Emby writes
    /// `MusicBrainzTrack` too, but this is unconfirmed against a live server —
    /// see `emby.rs`.
    #[serde(rename = "MusicBrainzTrack", default)]
    pub musicbrainz_track: Option<String>,
    #[serde(rename = "MusicBrainzAlbum", default)]
    pub musicbrainz_album: Option<String>,
    /// The performer. `MusicBrainzArtist` is the track artist and
    /// `MusicBrainzAlbumArtist` the record's — prefer the former and fall back,
    /// since that is the order `Play::mbid_artist` means.
    #[serde(rename = "MusicBrainzArtist", default)]
    pub musicbrainz_artist: Option<String>,
    #[serde(rename = "MusicBrainzAlbumArtist", default)]
    pub musicbrainz_album_artist: Option<String>,
    #[serde(rename = "MusicBrainzReleaseGroup", default)]
    pub musicbrainz_release_group: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct MediaStream {
    /// `Audio` | `Video` | `Subtitle`. Renamed explicitly rather than left to
    /// `rename_all`: this struct once carried a second field named `type_`,
    /// which PascalCase *also* maps to `Type`, so the two silently competed for
    /// one key and serde bound whichever was declared first. The explicit
    /// rename is the spelling that cannot be broken by editing `rename_all`.
    #[serde(rename = "Type", default)]
    pub stream_type: Option<String>,
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default)]
    pub bit_rate: Option<i32>,
    #[serde(default)]
    pub sample_rate: Option<i32>,
    #[serde(default)]
    pub bit_depth: Option<i32>,
    #[serde(default)]
    pub channels: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PlayState {
    #[serde(default)]
    pub position_ticks: Option<u64>,
    #[serde(default)]
    pub is_paused: Option<bool>,
    /// `DirectPlay` | `DirectStream` | `Transcode`. The authoritative answer to
    /// "did this file reach the listener intact?" — `TranscodingInfo` describes
    /// *how* a transcode is happening, but this says *whether* one is.
    #[serde(default)]
    pub play_method: Option<String>,
}

/// A track the server says is playing, already attributed and converted.
#[derive(Debug, Clone)]
pub struct MediaBrowserTrack {
    pub play: Play,
    pub quality: Option<AudioQuality>,
    /// Seconds into the track — both servers report a real playhead, so the deck
    /// gets a true position rather than one counted up from first sight.
    pub position: u64,
    pub session_id: String,
    /// The client this is playing on, so the listen can carry a device and
    /// therefore a signal chain. `None` only when the server reported no
    /// `DeviceId` at all.
    pub player: Option<MediaBrowserPlayer>,
}

#[derive(Debug, Default)]
pub struct MediaBrowserSessions {
    pub now_playing: Vec<MediaBrowserTrack>,
    pub ready_to_scrobble: Vec<MediaBrowserTrack>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn http_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| Client::new())
}

impl MediaBrowserSource {
    pub fn new(flavour: Flavour, url: String, api_key: String, account: String) -> Self {
        Self {
            flavour,
            url: url.trim_end_matches('/').to_string(),
            api_key,
            account,
            client: http_client(),
            tracker: SessionTracker::new(),
        }
    }

    pub fn jellyfin(url: String, api_key: String, account: String) -> Self {
        Self::new(Flavour::Jellyfin, url, api_key, account)
    }

    pub fn emby(url: String, api_key: String, account: String) -> Self {
        Self::new(Flavour::Emby, url, api_key, account)
    }

    pub fn flavour(&self) -> Flavour {
        self.flavour
    }

    /// Full URL for a route, with the flavour's base path applied.
    pub(crate) fn endpoint(&self, path: &str) -> String {
        format!("{}{}{}", self.url, self.flavour.base(), path)
    }

    /// Which account an API key belongs to.
    ///
    /// The same job `PlexSource::resolve_account` does, but answered by the
    /// server itself rather than a cloud service — so unlike Plex this works on
    /// a fully offline LAN install.
    pub async fn resolve_account(
        flavour: Flavour,
        url: &str,
        api_key: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Me {
            #[serde(default)]
            name: Option<String>,
        }

        let client = http_client();
        let base = format!("{}{}", url.trim_end_matches('/'), flavour.base());

        let resp = client
            .get(format!("{base}/Users/Me"))
            .header("X-Emby-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(format!("{} rejected that API key", flavour.label()).into());
        }

        // `/Users/Me` only answers for a token minted by a *user* login. An API
        // key created in the dashboard is server-scoped and belongs to nobody,
        // so the server cannot resolve "Me" and answers 400 — which is what
        // every connection test with an API key hit. Not an error to report: it
        // just means the account has to be named rather than inferred.
        if resp.status().is_success() {
            let me: Me = resp.json().await?;
            if let Some(name) = me.name.filter(|n| !n.trim().is_empty()) {
                return Ok(name);
            }
        }

        let users = Self::list_accounts(flavour, url, api_key).await?;
        match users.len() {
            0 => Err(format!("{} reported no users for that key", flavour.label()).into()),
            // One user is unambiguous, so don't make them pick.
            1 => Ok(users.into_iter().next().expect("checked").name),
            _ => Err(format!(
                "That API key isn't tied to a user, and this server has {}. \
                 Choose which account's listens to record.",
                users.len()
            )
            .into()),
        }
    }

    /// Every enabled account on the server, for the source-setup picker.
    ///
    /// Disabled users are dropped: they cannot play anything, so offering them
    /// would only be a way to configure a source that never scrobbles.
    pub async fn list_accounts(
        flavour: Flavour,
        url: &str,
        api_key: &str,
    ) -> Result<Vec<MediaBrowserAccount>, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Policy {
            #[serde(default)]
            is_disabled: bool,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct User {
            id: String,
            name: String,
            #[serde(default)]
            policy: Option<Policy>,
        }

        let client = http_client();
        let base = format!("{}{}", url.trim_end_matches('/'), flavour.base());

        let resp = client
            .get(format!("{base}/Users"))
            .header("X-Emby-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(format!("{} rejected that API key", flavour.label()).into());
        }
        if !resp.status().is_success() {
            return Err(
                format!("{} returned {} listing users", flavour.label(), resp.status()).into()
            );
        }

        let users: Vec<User> = resp.json().await?;
        Ok(users
            .into_iter()
            .filter(|u| !u.policy.as_ref().is_some_and(|p| p.is_disabled))
            .filter(|u| !u.name.trim().is_empty())
            .map(|u| MediaBrowserAccount { id: u.id, name: u.name })
            .collect())
    }

    /// Confirm the server answers and the key is good.
    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let resp = self
            .client
            .get(self.endpoint("/System/Info"))
            .header("X-Emby-Token", &self.api_key)
            .send()
            .await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(format!("{} rejected that API key", self.flavour.label()).into());
        }
        if !resp.status().is_success() {
            return Err(format!("{} returned {}", self.flavour.label(), resp.status()).into());
        }
        Ok(())
    }

    /// Poll `/Sessions`, returning what's playing and what has crossed the
    /// scrobble threshold since the last poll.
    pub async fn fetch_sessions(
        &mut self,
    ) -> Result<MediaBrowserSessions, Box<dyn std::error::Error>> {
        let resp = self
            .client
            .get(self.endpoint("/Sessions"))
            .header("X-Emby-Token", &self.api_key)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(
                format!("{} /Sessions returned {}", self.flavour.label(), resp.status()).into()
            );
        }
        let sessions: Vec<MbSession> = resp.json().await?;
        Ok(self.consume_sessions(sessions))
    }

    /// The pure half of `fetch_sessions`: decide what is playing and what has
    /// crossed the threshold, given a batch of sessions.
    ///
    /// Split out so the repeat and seek behaviour can be tested without a
    /// server — the same reason Plex has `process_live_track`.
    pub(crate) fn consume_sessions(&mut self, sessions: Vec<MbSession>) -> MediaBrowserSessions {
        let mut result = MediaBrowserSessions::default();
        let mut live_sessions = Vec::new();

        for session in sessions {
            // Other people's playback on a shared server is not this user's.
            if !session
                .user_name
                .as_deref()
                .map(|u| u.eq_ignore_ascii_case(&self.account))
                .unwrap_or(false)
            {
                continue;
            }
            // Borrowed rather than moved out: the session is still needed for
            // the client identity that becomes the listen's device.
            let Some(item) = session.now_playing_item.as_ref() else {
                continue;
            };
            // Sessions cover video too.
            if !item.item_type.eq_ignore_ascii_case("Audio") {
                continue;
            }

            let position = session
                .play_state
                .as_ref()
                .and_then(|p| p.position_ticks)
                .map(|t| t / TICKS_PER_SECOND)
                .unwrap_or(0);
            let duration = item.run_time_ticks.map(|t| t / TICKS_PER_SECOND);

            let track = MediaBrowserTrack {
                quality: Self::extract_quality(
                    item,
                    session.transcoding_info.as_ref(),
                    session.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
                ),
                play: Self::to_play(self.flavour, item, &session.device_name),
                position,
                session_id: session.id.clone(),
                player: Self::to_player(&session),
            };
            live_sessions.push(session.id.clone());

            // A paused track is still "now playing" for the deck, but its
            // position isn't advancing, so it shouldn't cross the threshold.
            let paused = session.play_state.as_ref().and_then(|p| p.is_paused).unwrap_or(false);

            if self.tracker.observe(&session.id, &item.id, &item.name, position, duration, paused) {
                // The banked copy gets a source_id unique to *this pass*. The
                // now-playing copy keeps the stable one, which is what the deck
                // wants. Without this the id was `<kind>-<item>-<device>` —
                // identical on every repeat, so `Database::exists` dropped the
                // second and third plays permanently, not merely within a
                // window. Plex has always put the scrobble time in its suffix
                // for exactly this reason.
                let mut banked = track.clone();
                banked.play.source_id =
                    format!("{}-{}-scrobble-{}", self.flavour.source_name(), item.id, now_secs());
                result.ready_to_scrobble.push(banked);
            }
            result.now_playing.push(track);
        }

        self.tracker.retain_live(&live_sessions);
        result
    }

    pub(crate) fn to_play(
        flavour: Flavour,
        item: &NowPlayingItem,
        device: &Option<String>,
    ) -> Play {
        let artist = item
            .artists
            .as_ref()
            .and_then(|a| a.first().cloned())
            .or_else(|| item.album_artist.clone())
            .unwrap_or_else(|| "Unknown".to_string());

        Play {
            // The item id alone would collide across repeat plays, so the
            // session and device disambiguate; the flush-time dedup handles the
            // rest.
            source_id: format!(
                "{}-{}-{}",
                flavour.source_name(),
                item.id,
                device.as_deref().unwrap_or("session")
            ),
            source_name: flavour.source_name().into(),
            title: item.name.clone(),
            artist,
            album: item.album.clone(),
            timestamp: now_secs(),
            duration: item.run_time_ticks.map(|t| t / TICKS_PER_SECOND),
            track_number: item.index_number.filter(|n| *n > 0),
            // Carries MusicBrainz ids when the library was tagged — another
            // listen that never needs a MusicBrainz lookup.
            mbid_recording: item.provider_ids.as_ref().and_then(|p| p.musicbrainz_track.clone()),
            mbid_release: item.provider_ids.as_ref().and_then(|p| p.musicbrainz_album.clone()),
            mbid_artist: item
                .provider_ids
                .as_ref()
                .and_then(|p| {
                    p.musicbrainz_artist.clone().or_else(|| p.musicbrainz_album_artist.clone())
                })
                .map(|id| vec![id]),
            artists: item.artists.clone(),
            mbid_release_group: item
                .provider_ids
                .as_ref()
                .and_then(|p| p.musicbrainz_release_group.clone()),
            caa_id: None,
            caa_release_mbid: None,
        }
    }

    /// The client this session is running on, in `devices` terms.
    ///
    /// `DeviceId` is the key: it is stable per installed client across restarts,
    /// where `DeviceName` is a label the user can change. A session without one
    /// can't be a device, so it stays `None` rather than being keyed on
    /// something that will drift.
    pub(crate) fn to_player(session: &MbSession) -> Option<MediaBrowserPlayer> {
        let machine_id = session.device_id.clone()?;
        Some(MediaBrowserPlayer {
            machine_id,
            // "Living Room" if the user named it, else the app name.
            name: session.device_name.clone().or_else(|| session.client.clone()),
            platform: session.device_type.clone(),
            // The app, which is what a chain is usually bound to — "Finamp on
            // the phone" is a different signal path from the web player.
            product: session.client.clone(),
        })
    }

    /// Quality, both halves: the file on disk from `MediaStreams`, and what was
    /// actually delivered from `TranscodingInfo` / `PlayMethod`.
    ///
    /// Keeping them apart is the whole point — a FLAC transcoded to 128k Opus
    /// for a phone is a lossless *source* and a lossy *listen*, and collapsing
    /// them loses the fact that the transcode happened at all.
    pub(crate) fn extract_quality(
        item: &NowPlayingItem,
        transcoding: Option<&TranscodingInfo>,
        play_method: Option<&str>,
    ) -> Option<AudioQuality> {
        let streams = item.media_streams.as_ref()?;
        let audio = streams.iter().find(|s| {
            let t = s.stream_type.as_deref().unwrap_or("");
            t.eq_ignore_ascii_case("Audio")
        })?;

        let codec_lower = audio.codec.as_deref().unwrap_or("").to_lowercase();
        let is_lossless = matches!(
            codec_lower.as_str(),
            "flac" | "alac" | "wav" | "aiff" | "ape" | "wavpack" | "pcm" | "dsd"
        );

        // `PlayMethod` is the authority; `TranscodingInfo` merely describes a
        // transcode that is running. Note `IsAudioDirect` — a video transcode
        // can leave the audio untouched, and calling that a transcoded listen
        // would be wrong.
        let audio_direct = transcoding.and_then(|t| t.is_audio_direct).unwrap_or(false);
        let is_transcoded = match play_method {
            Some(m) if m.eq_ignore_ascii_case("Transcode") => !audio_direct,
            // DirectStream remuxes the container without touching the audio;
            // DirectPlay touches nothing. Neither alters what was heard.
            Some(_) => false,
            // No PlayMethod at all: fall back to whether a transcode is running.
            None => transcoding.is_some() && !audio_direct,
        };

        let transcode_reason = transcoding
            .and_then(|t| t.transcode_reasons.as_ref())
            .filter(|r| !r.is_empty())
            .map(|r| r.join(", "));

        let mut q = AudioQuality {
            format_type: Some(if codec_lower == "dsd" { "dsd" } else { "pcm" }.to_string()),
            codec: audio.codec.clone(),
            bitrate: audio.bit_rate,
            sample_rate: audio.sample_rate,
            bit_depth: audio.bit_depth.map(|b| b as i16),
            channels: audio.channels.map(|c| c as i16),
            container: item.container.clone(),
            is_lossless: Some(is_lossless),
            is_transcoded: Some(is_transcoded),
            transcode_reason,
            ..Default::default()
        };

        // Only describe the delivery side when it really is a different thing
        // from the source, or `delivery_codec` would claim a Bluetooth-style
        // downgrade on every direct play — and `compute_score` docks points for
        // one.
        if is_transcoded {
            if let Some(t) = transcoding {
                q.delivery_codec = t.audio_codec.clone().map(|c| c.to_lowercase());
                // Reported in bits per second; Tapedeck's other sources and the
                // score thresholds are in kbps.
                q.delivery_bitrate = t.bitrate.map(|b| if b > 10_000 { b / 1000 } else { b });
            }
        }

        q.compute_score();
        Some(q)
    }

    /// Find an artist by name, returning their item id.
    ///
    /// `searchTerm` is a fuzzy match on the server's side, so the name is
    /// checked here the same way it is for Plex — a search hit is a suggestion,
    /// not an identification.
    pub async fn find_artist(
        &self,
        name: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Resp {
            #[serde(default)]
            items: Vec<Item>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Item {
            id: String,
            name: String,
            #[serde(default)]
            image_tags: HashMap<String, String>,
        }

        let resp = self
            .client
            .get(self.endpoint("/Artists"))
            .query(&[("searchTerm", name), ("limit", "10")])
            .header("X-Emby-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!(
                "{} returned {} searching artists",
                self.flavour.label(),
                resp.status()
            )
            .into());
        }
        let body: Resp = resp.json().await?;

        Ok(body
            .items
            .into_iter()
            // No Primary image means nothing to fetch, so a match without one is
            // not a match worth returning.
            .find(|i| {
                crate::sources::plex::artist_matches(&i.name, name)
                    && i.image_tags.contains_key("Primary")
            })
            .map(|i| i.id))
    }

    /// The item id of the account this source is pinned to.
    ///
    /// A playlist has an owner, and an API key minted in the dashboard belongs
    /// to nobody — so `/Users/Me` is no help and the account name the source
    /// already carries has to be resolved against `/Users`. Without this the
    /// playlist would either be refused or land on whichever account the server
    /// picked, which on a household server is somebody else's.
    pub async fn account_id(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let users = Self::list_accounts(self.flavour, &self.url, &self.api_key).await?;
        if self.account.trim().is_empty() {
            return match users.len() {
                1 => Ok(users.into_iter().next().expect("checked").id),
                _ => Err(format!(
                    "This {} source has no account set, and the server has several — \
                     set one before pushing a playlist",
                    self.flavour.label()
                )
                .into()),
            };
        }
        users
            .into_iter()
            .find(|u| u.name.eq_ignore_ascii_case(self.account.trim()))
            .map(|u| u.id)
            .ok_or_else(|| {
                format!("{} has no user called {}", self.flavour.label(), self.account).into()
            })
    }

    /// Tracks matching a free-text query, as candidates for a playlist push.
    ///
    /// `searchTerm` is fuzzy on the server's side — the same caution
    /// `find_artist` documents — so these are suggestions and
    /// `crate::playlists::pick` decides.
    pub async fn search_tracks(
        &self,
        query: &str,
    ) -> Result<Vec<crate::playlists::Candidate>, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Resp {
            #[serde(default)]
            items: Vec<Item>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Item {
            id: String,
            name: String,
            /// The performing credit. `Artists` is per-track and is what a
            /// listen's artist was recorded from; `AlbumArtist` is the fallback
            /// and is "Various Artists" on a compilation.
            #[serde(default)]
            artists: Vec<String>,
            #[serde(default)]
            album_artist: Option<String>,
            #[serde(default)]
            album: Option<String>,
        }

        // Emby scopes item queries to a user (`/emby/Users/<id>/Items`) where
        // Jellyfin accepts a bare `/Items`. Jellyfin tolerates the user-scoped
        // form too, but asking for the account id costs a round trip, so each
        // takes the cheapest route it has.
        let path = match self.flavour {
            Flavour::Jellyfin => "/Items".to_string(),
            Flavour::Emby => format!("/Users/{}/Items", self.account_id().await?),
        };

        let resp = self
            .client
            .get(self.endpoint(&path))
            .query(&[
                ("searchTerm", query),
                ("includeItemTypes", "Audio"),
                ("recursive", "true"),
                ("limit", "20"),
                ("fields", "Artists"),
            ])
            .header("X-Emby-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!(
                "{} returned {} searching tracks",
                self.flavour.label(),
                resp.status()
            )
            .into());
        }
        let body: Resp = resp.json().await?;

        Ok(body
            .items
            .into_iter()
            .map(|i| crate::playlists::Candidate {
                id: i.id,
                title: i.name,
                artist: i
                    .artists
                    .into_iter()
                    .next()
                    .filter(|s| !s.trim().is_empty())
                    .or(i.album_artist)
                    .unwrap_or_default(),
                album: i.album,
            })
            .collect())
    }

    /// Create an audio playlist holding `ids`, in that order.
    ///
    /// The two flavours disagree about how to say it. Jellyfin marks the query
    /// parameters deprecated on this endpoint and wants a JSON body with an
    /// array; Emby never deprecated them and takes a comma-joined `Ids` in the
    /// query string. Sending Jellyfin's shape to Emby creates an empty playlist
    /// rather than failing, which is why these are two calls and not one with a
    /// flag.
    pub async fn create_playlist(
        &self,
        name: &str,
        ids: &[String],
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if ids.is_empty() {
            return Err("Nothing to put in the playlist".into());
        }
        let user_id = self.account_id().await?;

        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Made {
            id: String,
        }

        let req = self
            .client
            .post(self.endpoint("/Playlists"))
            .header("X-Emby-Token", &self.api_key)
            .header("Accept", "application/json");

        let req = match self.flavour {
            Flavour::Jellyfin => req.json(&serde_json::json!({
                "Name": name,
                "Ids": ids,
                "UserId": user_id,
                "MediaType": "Audio",
            })),
            Flavour::Emby => req.query(&[
                ("Name", name.to_string()),
                ("Ids", ids.join(",")),
                ("UserId", user_id),
                ("MediaType", "Audio".to_string()),
            ]),
        };

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(format!(
                "{} playlist create returned {}",
                self.flavour.label(),
                resp.status()
            )
            .into());
        }
        let made: Made = resp.json().await?;
        Ok(made.id)
    }

    /// Cover art bytes, proxied for the same reason as Plex's and Navidrome's —
    /// the browser cannot reach a box on the user's LAN and must not hold its
    /// credentials.
    pub async fn fetch_cover_art(
        &self,
        item_id: &str,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error + Send + Sync>> {
        let resp = self
            .client
            .get(self.endpoint(&format!("/Items/{item_id}/Images/Primary")))
            .header("X-Emby-Token", &self.api_key)
            .query(&[("maxWidth", "500")])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(
                format!("{} artwork returned {}", self.flavour.label(), resp.status()).into()
            );
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
}

#[async_trait::async_trait]
impl crate::sources::MusicSource for MediaBrowserSource {
    fn name(&self) -> &str {
        self.flavour.label()
    }

    fn set_scrobble_threshold(&mut self, threshold: crate::models::ScrobbleThreshold) {
        self.tracker.set_threshold(threshold);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    /// Neither server records listens natively — Jellyfin needs the Playback
    /// Reporting plugin and Emby its own equivalent, and neither is guaranteed
    /// installed. Everything comes from the live session poll, so this returns
    /// nothing rather than pretending otherwise.
    async fn fetch_new_plays(
        &mut self,
        _last_checked: u64,
    ) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Type` must actually reach `stream_type`, and the audio stream must be
    /// found by it.
    ///
    /// This struct once declared *two* fields for this key — `type_`, which
    /// `rename_all = "PascalCase"` also maps to `Type`, beside an explicitly
    /// renamed `stream_type`. serde binds whichever is declared first, so
    /// `stream_type` was always `None` and `extract_quality` only worked
    /// because it fell through an `.or()` to the other one. Nothing errored;
    /// clippy's `unreachable pattern` was the only sign.
    ///
    /// The consequence if this regresses is not a missing label — it is
    /// `extract_quality` finding no audio stream at all and returning `None`,
    /// so every listen from both servers loses its codec, sample rate, bit
    /// depth and quality score together.
    #[test]
    fn the_type_key_binds_to_exactly_one_field() {
        let s: MediaStream =
            serde_json::from_str(r#"{"Type":"Audio","Codec":"flac","SampleRate":44100}"#)
                .expect("parses");
        assert_eq!(s.stream_type.as_deref(), Some("Audio"));

        // …and the stream is reachable through the real selector, which is what
        // the `.or()` chain was masking.
        let mut it = item("i1", 293);
        it.media_streams = Some(vec![s]);
        let q = MediaBrowserSource::extract_quality(&it, None, Some("DirectPlay"))
            .expect("the audio stream must be found");
        assert_eq!(q.codec.as_deref(), Some("flac"));
        assert_eq!(q.sample_rate, Some(44100));
        assert!(q.quality_score.is_some());
    }

    /// The base path is the single largest part of the Jellyfin/Emby diff, and
    /// getting it wrong is a 404 on every route rather than a silent wrong
    /// answer — but it is worth pinning because the two are otherwise
    /// interchangeable and a copied constructor would look correct.
    #[test]
    fn the_base_path_is_the_flavours_only_routing_difference() {
        let jf = MediaBrowserSource::jellyfin("http://jf:8096".into(), "k".into(), "a".into());
        let emby = MediaBrowserSource::emby("http://emby:8096".into(), "k".into(), "a".into());

        assert_eq!(jf.endpoint("/Sessions"), "http://jf:8096/Sessions");
        assert_eq!(emby.endpoint("/Sessions"), "http://emby:8096/emby/Sessions");
    }

    /// A listen must record which server it came off, or the two collide in
    /// `Database::exists` — the same item id can exist on both.
    #[test]
    fn each_flavour_stamps_its_own_source_name() {
        assert_eq!(Flavour::Jellyfin.source_name(), "jellyfin");
        assert_eq!(Flavour::Emby.source_name(), "emby");

        let jf = MediaBrowserSource::to_play(Flavour::Jellyfin, &item("i1", 293), &None);
        let emby = MediaBrowserSource::to_play(Flavour::Emby, &item("i1", 293), &None);
        assert_eq!(jf.source_name, "jellyfin");
        assert_eq!(emby.source_name, "emby");
        assert_ne!(jf.source_id, emby.source_id, "same item on both servers is two listens");
    }

    #[test]
    fn url_trailing_slash_is_normalised() {
        let s = MediaBrowserSource::jellyfin("http://jf:8096/".into(), "k".into(), "ashwin".into());
        assert_eq!(s.url, "http://jf:8096");
        let e = MediaBrowserSource::emby("http://emby:8096/".into(), "k".into(), "ashwin".into());
        assert_eq!(e.endpoint("/Users"), "http://emby:8096/emby/Users");
    }

    pub(super) fn item(id: &str, secs: u64) -> NowPlayingItem {
        NowPlayingItem {
            id: id.into(),
            name: "Xtal".into(),
            item_type: "Audio".into(),
            album: Some("SAW 85-92".into()),
            album_artist: Some("Aphex Twin".into()),
            artists: Some(vec!["Aphex Twin".into()]),
            run_time_ticks: Some(secs * TICKS_PER_SECOND),
            index_number: Some(1),
            container: Some("flac".into()),
            provider_ids: Some(ProviderIds {
                musicbrainz_track: Some("mb-track".into()),
                musicbrainz_album: Some("mb-album".into()),
                musicbrainz_artist: Some("mb-artist".into()),
                musicbrainz_album_artist: Some("mb-album-artist".into()),
                musicbrainz_release_group: Some("mb-rg".into()),
            }),
            media_streams: Some(vec![MediaStream {
                stream_type: Some("Audio".into()),
                codec: Some("flac".into()),
                bit_rate: Some(900_000),
                sample_rate: Some(44100),
                bit_depth: Some(16),
                channels: Some(2),
            }]),
        }
    }

    // ---- The state machine, which is flavour-independent ------------------

    /// Playing one track three times on repeat must bank three listens.
    ///
    /// This is the bug Plex had until v0.13.3 and Jellyfin still had after it:
    /// the session and the item are both unchanged on a repeat, so the
    /// `item_id` check never fires, `scrobbled` stays true, and only the first
    /// of N plays is ever recorded. Nothing errors — the listens are simply
    /// missing. Tested on the tracker directly so it covers every source built
    /// on it, present and future.
    #[test]
    fn a_looped_track_scrobbles_once_per_pass() {
        let mut t = SessionTracker::new();
        let mut banked = 0;
        for pass in 0..3 {
            for secs in [3u64, 120, 195] {
                if t.observe("sess-1", "item-1", "Combustible", secs, Some(200), false) {
                    banked += 1;
                }
            }
            assert_eq!(banked, pass + 1, "pass {pass} should have banked exactly one");
        }
    }

    /// Scrubbing back a few seconds inside one listen is not a new play, or
    /// fidgeting with the playhead would mint scrobbles.
    #[test]
    fn a_small_seek_back_is_not_a_repeat() {
        let mut t = SessionTracker::new();
        let mut banked = 0;
        for secs in [120u64, 115, 118, 160, 195] {
            if t.observe("sess-1", "item-1", "Combustible", secs, Some(200), false) {
                banked += 1;
            }
        }
        assert_eq!(banked, 1, "one listen, however much the playhead was nudged");
    }

    /// A rewind *before* the threshold has nothing to undo, so it must not arm
    /// a second scrobble for the same pass.
    #[test]
    fn a_rewind_before_the_threshold_banks_nothing_extra() {
        let mut t = SessionTracker::new();
        let mut banked = 0;
        for secs in [40u64, 5, 60, 110, 195] {
            if t.observe("sess-1", "item-1", "Combustible", secs, Some(200), false) {
                banked += 1;
            }
        }
        assert_eq!(banked, 1);
    }

    /// A new item in the same session resets the flag, or only the first song
    /// of a queue would ever count.
    #[test]
    fn a_queue_banks_every_track() {
        let mut t = SessionTracker::new();
        let mut banked = 0;
        for item in ["a", "b", "c"] {
            for secs in [3u64, 120] {
                if t.observe("sess-1", item, "track", secs, Some(200), false) {
                    banked += 1;
                }
            }
        }
        assert_eq!(banked, 3);
    }

    /// A paused track is still on the deck but its position is not advancing,
    /// so it must not cross the threshold.
    #[test]
    fn a_paused_track_never_crosses_the_threshold() {
        let mut t = SessionTracker::new();
        assert!(!t.observe("sess-1", "item-1", "track", 195, Some(200), true));
        // …and resuming past it still banks exactly once.
        assert!(t.observe("sess-1", "item-1", "track", 196, Some(200), false));
    }

    /// A source that reports no duration falls back to the four-minute rule
    /// rather than never scrobbling.
    #[test]
    fn an_unknown_duration_falls_back_to_four_minutes() {
        let mut t = SessionTracker::new();
        assert!(!t.observe("s", "i", "track", 239, None, false));
        assert!(t.observe("s", "i", "track", 240, None, false));
    }

    /// A session that goes away is forgotten, so a returning one starts clean
    /// rather than inheriting a stale `scrobbled` flag.
    #[test]
    fn a_departed_session_is_forgotten() {
        let mut t = SessionTracker::new();
        assert!(t.observe("sess-1", "item-1", "track", 195, Some(200), false));
        t.retain_live(&[]);
        // Same session and item returning is a fresh play, not a suppressed one.
        assert!(t.observe("sess-1", "item-1", "track", 195, Some(200), false));
    }
}
