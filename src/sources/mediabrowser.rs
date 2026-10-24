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

use super::egress::Egress;
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

/// How many items to ask for at a time when walking the library for ratings.
///
/// A manual, user-triggered walk against a server on the user's own network,
/// so the round trips cost more than the rows.
const RATING_PAGE_SIZE: usize = 500;

/// Where that walk gives up. A ceiling rather than an expectation, and the one
/// stopping condition that is a *loss*, so it is the one that logs.
const RATING_MAX_ITEMS: usize = 200_000;

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
#[derive(Debug, Clone, serde::Serialize, Deserialize, utoipa::ToSchema)]
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
    egress: Egress,
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
    /// Enough of the outgoing track to write a listen for it once it is
    /// displaced. Held because the *caller* cannot supply it at that moment:
    /// by the time a session reports a new item, the old item's metadata is
    /// gone from the payload, and the tracker is the only thing that still
    /// knows what was playing a poll ago.
    play: Play,
}

/// A track that was playing and was replaced before it ever counted.
///
/// This is a **skip**, and it is the one thing a polled source can say about
/// how a listen ended. Everything else here is about a listen beginning.
#[derive(Debug, Clone)]
pub struct Abandoned {
    pub play: Play,
    /// How far in the listener got, in seconds. Real, not inferred: it is the
    /// last playhead the server reported for that item.
    ///
    /// No `duration` beside it on purpose: the threshold decision has already
    /// been made here, so a running time downstream would only be there to look
    /// informative.
    pub listened_secs: u64,
}

/// What one pass of a session says.
#[derive(Debug, Clone, Default)]
pub struct Observed {
    /// This pass just crossed the threshold — bank the listen.
    pub ready: bool,
    /// Something else started before this one ever crossed.
    pub abandoned: Option<Abandoned>,
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
    ///
    /// It also reports the track this one *replaced*, when that track never
    /// crossed the threshold — see [`Abandoned`]. The information was always
    /// here and was discarded one line later: at the item-change branch the
    /// entry still holds the outgoing item, whether it was ever banked, and how
    /// far into it the listener got.
    pub fn observe(
        &mut self,
        session_id: &str,
        item_id: &str,
        play: &Play,
        position: u64,
        duration: Option<u64>,
        paused: bool,
    ) -> Observed {
        let entry = self.seen.entry(session_id.to_string()).or_insert_with(|| SessionProgress {
            item_id: item_id.to_string(),
            scrobbled: false,
            last_position: position,
            play: play.clone(),
        });

        let mut out = Observed::default();

        if entry.item_id != item_id {
            // The outgoing track never counted, and something else is playing
            // now — so the listener moved on from it deliberately. A track that
            // *did* cross is already banked and is simply over.
            if !entry.scrobbled {
                out.abandoned = Some(Abandoned {
                    play: entry.play.clone(),
                    listened_secs: entry.last_position,
                });
            }
            entry.item_id = item_id.to_string();
            entry.scrobbled = false;
            entry.play = play.clone();
        } else if entry.last_position > position
            && entry.last_position - position >= REPEAT_REWIND_SECS
        {
            if entry.scrobbled {
                debug!(
                    "Playhead rewound in session {} ({}s -> {}s): '{}' started over",
                    session_id, entry.last_position, position, play.title
                );
            }
            entry.scrobbled = false;
        }
        entry.last_position = position;

        let past_threshold = !paused && self.threshold.reached(position, duration);

        if past_threshold && !entry.scrobbled {
            entry.scrobbled = true;
            out.ready = true;
        }
        out
    }

    /// Forget sessions that have gone away, so the map doesn't grow forever and
    /// a returning session starts clean.
    ///
    /// **A session that disappears is deliberately not a skip.** An item change
    /// is evidence — something else started playing, so the listener moved on.
    /// A session simply ending is not: the player was closed, the listener
    /// walked away, the network blinked, the poll missed. Writing those as
    /// skips would file "went to bed" as a judgement about a record, and a
    /// skip is meant to mean something.
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
    /// The same playhead in milliseconds, for the deck: `PositionTicks` carries
    /// ten million to the second, and rounding it to whole seconds threw away
    /// what the hold rule compares.
    pub position_ms: u64,
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
    /// Tracks replaced before they ever counted. `position` is how far in the
    /// listener got, which is the whole point — bailing three seconds in and
    /// bailing thirty seconds from the end are opposite behaviours.
    pub skipped: Vec<MediaBrowserTrack>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn http_client(egress: Egress) -> Client {
    egress.client(Duration::from_secs(10))
}

impl MediaBrowserSource {
    pub fn new(
        flavour: Flavour,
        url: String,
        api_key: String,
        account: String,
        egress: Egress,
    ) -> Self {
        Self {
            flavour,
            url: url.trim_end_matches('/').to_string(),
            api_key,
            account,
            client: http_client(egress),
            egress,
            tracker: SessionTracker::new(),
        }
    }

    pub fn jellyfin(url: String, api_key: String, account: String, egress: Egress) -> Self {
        Self::new(Flavour::Jellyfin, url, api_key, account, egress)
    }

    pub fn emby(url: String, api_key: String, account: String, egress: Egress) -> Self {
        Self::new(Flavour::Emby, url, api_key, account, egress)
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
        egress: Egress,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Me {
            #[serde(default)]
            name: Option<String>,
        }

        let client = http_client(egress);
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

        let users = Self::list_accounts(flavour, url, api_key, egress).await?;
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
        egress: Egress,
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

        let client = http_client(egress);
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

            let ticks = session.play_state.as_ref().and_then(|p| p.position_ticks).unwrap_or(0);
            let position = ticks / TICKS_PER_SECOND;
            let position_ms = ticks / (TICKS_PER_SECOND / 1000);
            let duration = item.run_time_ticks.map(|t| t / TICKS_PER_SECOND);

            let track = MediaBrowserTrack {
                quality: Self::extract_quality(
                    item,
                    session.transcoding_info.as_ref(),
                    session.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
                ),
                play: Self::to_play(self.flavour, item, &session.device_name),
                position,
                position_ms,
                session_id: session.id.clone(),
                player: Self::to_player(&session),
            };
            live_sessions.push(session.id.clone());

            // A paused track is still "now playing" for the deck, but its
            // position isn't advancing, so it shouldn't cross the threshold.
            let paused = session.play_state.as_ref().and_then(|p| p.is_paused).unwrap_or(false);

            let observed = self.tracker.observe(
                &session.id,
                &item.id,
                &track.play,
                position,
                duration,
                paused,
            );

            // The track this one replaced, if it never counted. It played on
            // this same session, so it carries this session's player — which is
            // what gives a skip a device and therefore a signal chain.
            if let Some(gone) = observed.abandoned {
                let mut skipped = track.clone();
                skipped.play = gone.play;
                skipped.position = gone.listened_secs;
                // Its own id, or `exists` would treat the skip and a later real
                // play of the same track as one row.
                skipped.play.source_id = format!("{}-skip-{}", skipped.play.source_id, now_secs());
                // Deliberately not carried over: `quality` describes the item
                // now playing, not the one that just went.
                skipped.quality = None;
                result.skipped.push(skipped);
            }

            if observed.ready {
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
            // No release-track or work id in `ProviderIds` — Jellyfin does not
            // surface either, so these stay unset rather than guessed at.
            mbid_release_track: None,
            mbid_work: None,
            // Also recorded now, rather than only as a fallback for the
            // performer above: the record's credit and the track's are
            // different facts, and on a compilation they disagree. The fallback
            // is deliberately left in place — it is a guess, but it is the
            // guess this source has always made, and narrowing it is a separate
            // change from storing something it was throwing away.
            mbid_album_artist: item
                .provider_ids
                .as_ref()
                .and_then(|p| p.musicbrainz_album_artist.clone())
                .map(|id| vec![id]),
            caa_id: None,
            caa_release_mbid: None,
            isrc: None,
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
            codec: audio.codec.clone(),
            bitrate: audio.bit_rate,
            sample_rate: audio.sample_rate,
            bit_depth: audio.bit_depth.map(|b| b as i16),
            channels: audio.channels.map(|c| c as i16),
            container: item.container.clone(),
            is_transcoded: Some(is_transcoded),
            transcode_reason,
            ..Default::default()
        };
        q.classify_codec();

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
        let users =
            Self::list_accounts(self.flavour, &self.url, &self.api_key, self.egress).await?;
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

    /// Every rated or favourited item in this server's music library.
    ///
    /// **`IsFavorite` is the signal here and the number usually is not.**
    /// `UserItemDataDto.Rating` is a nullable double inherited from
    /// MediaBrowser and, per Jellyfin's own OpenAPI document, its
    /// `POST /UserItems/{itemId}/Rating` takes **`likes` — a boolean — and no
    /// number at all**. So on a modern Jellyfin that field is typically empty
    /// and the heart is what people actually press. It is read anyway because
    /// Emby still writes one and an older library may hold them, and because a
    /// field that is read costs nothing where a field that is not is
    /// unrecoverable.
    ///
    /// A favourite carries no scale and therefore takes no threshold: it is
    /// already the same statement a love makes. See [`crate::ratings::LoveRule`].
    ///
    /// Three kinds in one walk (`Audio`, `MusicAlbum`, `MusicArtist`), because
    /// a love can point at any of them and this server will happily rate all
    /// three.
    ///
    /// **Not verified against a live server.** `Rating` and `IsFavorite` are
    /// pinned to the shapes the OpenAPI schema declares; whether either is
    /// populated on a real `/Items` response with `EnableUserData` has not been
    /// captured. Same caveat the rest of this module's Emby half carries.
    pub async fn fetch_ratings(
        &self,
    ) -> Result<Vec<crate::db::IncomingRating>, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Resp {
            #[serde(default)]
            items: Vec<Item>,
            #[serde(default)]
            total_record_count: Option<usize>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Item {
            id: String,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            #[serde(rename = "Type")]
            item_type: Option<String>,
            #[serde(default)]
            artists: Vec<String>,
            #[serde(default)]
            album_artist: Option<String>,
            #[serde(default)]
            album: Option<String>,
            #[serde(default)]
            user_data: Option<UserData>,
            #[serde(default)]
            provider_ids: HashMap<String, String>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct UserData {
            #[serde(default)]
            rating: Option<f32>,
            #[serde(default)]
            is_favorite: bool,
        }

        // Emby scopes item queries to a user where Jellyfin accepts a bare
        // `/Items` — the same split `search_tracks` makes, and the account is
        // needed here regardless because `UserData` is per user.
        let account = self.account_id().await?;
        let path = match self.flavour {
            Flavour::Jellyfin => "/Items".to_string(),
            Flavour::Emby => format!("/Users/{account}/Items"),
        };

        let mut out: Vec<crate::db::IncomingRating> = Vec::new();
        let mut start = 0usize;
        loop {
            let resp = self
                .client
                .get(self.endpoint(&path))
                .query(&[
                    ("userId", account.as_str()),
                    ("includeItemTypes", "Audio,MusicAlbum,MusicArtist"),
                    ("recursive", "true"),
                    ("fields", "Artists,ProviderIds"),
                    ("enableUserData", "true"),
                    ("startIndex", &start.to_string()),
                    ("limit", &RATING_PAGE_SIZE.to_string()),
                ])
                .header("X-Emby-Token", &self.api_key)
                .header("Accept", "application/json")
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(format!(
                    "{} returned {} listing rated items",
                    self.flavour.label(),
                    resp.status()
                )
                .into());
            }
            let body: Resp = resp.json().await?;
            let n = body.items.len();

            for it in body.items {
                let Some(name) = it.name.filter(|n| !n.trim().is_empty()) else { continue };
                let Some(ud) = it.user_data else { continue };
                let rating = ud.rating.filter(|v| *v > 0.0);
                if rating.is_none() && !ud.is_favorite {
                    continue;
                }

                let kind = match it.item_type.as_deref() {
                    Some("MusicAlbum") => "release",
                    Some("MusicArtist") => "artist",
                    _ => "recording",
                };
                let artist = match kind {
                    "artist" => String::new(),
                    _ => it
                        .artists
                        .into_iter()
                        .next()
                        .filter(|s| !s.trim().is_empty())
                        .or(it.album_artist)
                        .unwrap_or_default(),
                };
                if kind != "artist" && artist.trim().is_empty() {
                    continue;
                }

                out.push(crate::db::IncomingRating {
                    kind: kind.to_string(),
                    remote_id: it.id,
                    artist,
                    name,
                    album: if kind == "recording" { it.album } else { None },
                    // Free, and every one is a MusicBrainz lookup not made.
                    mbid: it
                        .provider_ids
                        .get("MusicBrainzTrack")
                        .or_else(|| it.provider_ids.get("MusicBrainzAlbum"))
                        .or_else(|| it.provider_ids.get("MusicBrainzArtist"))
                        .cloned(),
                    rating: rating.map(|v| {
                        crate::ratings::Rating::new(v, crate::ratings::SourceScale::MEDIABROWSER)
                    }),
                    starred: ud.is_favorite,
                    // Neither server records when a favourite was pressed.
                    rated_at: None,
                });
            }

            start += n;
            // A short page is the end, and so is running past the count the
            // server itself reported — belt and braces, because a server that
            // ignores `startIndex` would otherwise re-serve page one forever.
            if n == 0 || n < RATING_PAGE_SIZE {
                break;
            }
            if body.total_record_count.is_some_and(|t| start >= t) {
                break;
            }
            if start >= RATING_MAX_ITEMS {
                tracing::warn!(
                    "{}: stopped listing rated items at {} — raise the cap if a library really \
                     holds more than this",
                    self.flavour.label(),
                    start
                );
                break;
            }
        }
        Ok(out)
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

    /// An artist's item id, found by name, whatever pictures it has.
    ///
    /// `find_artist` insists on a Primary image because its callers want a
    /// portrait; the picker wants the artist whether or not one is held, since
    /// it lists backdrops and the server's online finds too.
    pub async fn artist_item(
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
            .find(|i| crate::sources::plex::artist_matches(&i.name, name))
            .map(|i| i.id))
    }

    /// Every picture the server holds for an artist, then the ones it can find
    /// online — Primary for a portrait, Backdrop for a wide one.
    ///
    /// Held pictures come from `/Items/{id}/Images` and are proxied. The online
    /// ones come from `/Items/{id}/RemoteImages`, which asks the server's image
    /// providers live; they are public URLs, so the browser loads them and
    /// nothing is downloaded into the library. A failed online search costs
    /// only the online half.
    ///
    /// `ImageInfo.Path` is the file's location on the server's disk and is
    /// deliberately not read.
    pub async fn artist_images(
        &self,
        item_id: &str,
        wide: bool,
    ) -> Result<Vec<super::LibraryImage>, Box<dyn std::error::Error + Send + Sync>> {
        let kind = if wide { "Backdrop" } else { "Primary" };
        let resp = self
            .client
            .get(self.endpoint(&format!("/Items/{item_id}/Images")))
            .header("X-Emby-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!(
                "{} returned {} listing images",
                self.flavour.label(),
                resp.status()
            )
            .into());
        }
        let held: Vec<MbImageInfo> = resp.json().await?;
        let mut out = held_images(item_id, kind, held);

        let remote = self
            .client
            .get(self.endpoint(&format!("/Items/{item_id}/RemoteImages")))
            .query(&[("type", kind), ("limit", "24")])
            .header("X-Emby-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await;
        match remote {
            Ok(r) if r.status().is_success() => match r.json::<MbRemoteImages>().await {
                Ok(found) => out.extend(remote_images(found)),
                Err(e) => {
                    tracing::debug!("{} remote images unreadable: {}", self.flavour.label(), e)
                }
            },
            Ok(r) => {
                tracing::debug!("{} remote images returned {}", self.flavour.label(), r.status())
            }
            Err(e) => tracing::debug!("{} remote images failed: {}", self.flavour.label(), e),
        }
        Ok(out)
    }

    /// The bytes behind a held key `artist_images` returned.
    pub async fn fetch_image(
        &self,
        key: &str,
        max: u32,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error + Send + Sync>> {
        let Some((item, kind, index)) = parse_mb_image_key(key) else {
            return Err("not an image key".into());
        };
        let resp = self
            .client
            .get(self.endpoint(&format!("/Items/{item}/Images/{kind}/{index}")))
            .header("X-Emby-Token", &self.api_key)
            .query(&[("maxWidth", max.to_string())])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("{} image returned {}", self.flavour.label(), resp.status()).into());
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();
        if !content_type.starts_with("image/") {
            return Err("no image there".into());
        }
        Ok((resp.bytes().await?.to_vec(), content_type))
    }

    /// The lyrics this server holds for an item, as `(plain, synced)`.
    ///
    /// `GET /Audio/{id}/Lyrics`, which is a first-class endpoint here rather
    /// than something dug out of a stream list — and it is **the user's own
    /// tagging**, so it beats any provider: the right pressing, the right edit,
    /// and no rate limit because it is their server.
    ///
    /// Deliberately *not* `/Audio/{id}/RemoteSearch/Lyrics`. That is Jellyfin
    /// asking a lyrics provider on our behalf, which is LRCLIB one hop further
    /// away and is precisely the half that is not the user's.
    ///
    /// **`Start` is in .NET ticks — 10,000,000 per second, not milliseconds.**
    /// Reading them as ms puts every cue four orders of magnitude out, the
    /// reader sits on line one forever, and nothing errors. `Offset` is ticks
    /// too, and is applied here rather than stored, because everything
    /// downstream of this expects cues that are already right.
    ///
    /// Returns `Ok(None)` when the server has none, which is an ordinary answer
    /// and not a failure — most libraries are only partly tagged.
    pub async fn fetch_lyrics(
        &self,
        item_id: &str,
    ) -> Result<Option<(Option<String>, Option<String>)>, Box<dyn std::error::Error + Send + Sync>>
    {
        let resp = self
            .client
            .get(self.endpoint(&format!("/Audio/{item_id}/Lyrics")))
            .header("X-Emby-Token", &self.api_key)
            .send()
            .await?;

        // 404 is "this item has no lyrics", which is the common case.
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(
                format!("{} lyrics returned {}", self.flavour.label(), resp.status()).into()
            );
        }

        let dto: LyricDto = resp.json().await?;
        Ok(lyric_dto_to_pair(&dto))
    }
}

/// `LyricDto` — what `GET /Audio/{id}/Lyrics` returns.
///
/// PascalCase on the wire like every other Jellyfin/Emby response. Pinned to
/// the 12.0.0 schema; the field that fails silently if it is wrong is `Start`,
/// which is ticks.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LyricDto {
    #[serde(default)]
    pub metadata: Option<LyricMetadata>,
    #[serde(default)]
    pub lyrics: Vec<LyricLine>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LyricMetadata {
    #[serde(default)]
    pub is_synced: Option<bool>,
    /// Ticks, and signed: a negative offset pulls the words earlier.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LyricLine {
    #[serde(default)]
    pub text: Option<String>,
    /// **Ticks — 10,000,000 per second.** Not milliseconds.
    #[serde(default)]
    pub start: Option<i64>,
}

/// Turn a `LyricDto` into `(plain, synced)`.
///
/// Split out of the fetch for the reason `consume_sessions` is: **the ticks
/// conversion fails silently.** A `Start` read as milliseconds is four orders
/// of magnitude out, so every cue lands in the first thousandth of a second,
/// the reader sits on line one forever, and nothing anywhere errors. That has
/// to be testable without a server.
///
/// `Offset` is applied here rather than stored, because everything downstream
/// expects cues that are already right, and it is signed — a negative offset
/// legitimately pulls the words earlier.
pub fn lyric_dto_to_pair(dto: &LyricDto) -> Option<(Option<String>, Option<String>)> {
    /// .NET ticks in one second. The whole trap, named.
    const TICKS_PER_SEC: f64 = 10_000_000.0;

    let offset = dto.metadata.as_ref().and_then(|m| m.offset).unwrap_or(0);
    let lines: Vec<&LyricLine> =
        dto.lyrics.iter().filter(|l| l.text.is_some() || l.start.is_some()).collect();
    if lines.is_empty() {
        return None;
    }

    let plain: String =
        lines.iter().map(|l| l.text.clone().unwrap_or_default()).collect::<Vec<_>>().join("\n");

    // Timed when the cues are actually there. `IsSynced` is the server's
    // opinion of the file; the lines are the evidence, and a file claiming to
    // be synced with no cues would otherwise produce an unusable timed lyric.
    let synced = lines.iter().any(|l| l.start.is_some()).then(|| {
        lines
            .iter()
            .map(|l| {
                let t = (l.start.unwrap_or(0) + offset).max(0);
                let secs = t as f64 / TICKS_PER_SEC;
                let m = (secs / 60.0).floor() as i64;
                let rest = secs - (m as f64) * 60.0;
                format!("[{:02}:{:05.2}] {}", m, rest, l.text.clone().unwrap_or_default())
            })
            .collect::<Vec<_>>()
            .join("\n")
    });

    let plain = (!plain.trim().is_empty()).then_some(plain);
    if plain.is_none() && synced.is_none() {
        return None;
    }
    Some((plain, synced))
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

/// One entry of `/Items/{id}/Images` — Jellyfin's `ImageInfo`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct MbImageInfo {
    #[serde(default)]
    image_type: String,
    #[serde(default)]
    image_index: Option<i64>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
}

/// `/Items/{id}/RemoteImages` — Jellyfin's `RemoteImageResult`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct MbRemoteImages {
    #[serde(default)]
    images: Vec<MbRemoteImage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct MbRemoteImage {
    #[serde(default)]
    provider_name: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    thumbnail_url: Option<String>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
}

/// The held pictures of one type, keyed `item/Type/index`.
pub(crate) fn held_images(
    item_id: &str,
    kind: &str,
    held: Vec<MbImageInfo>,
) -> Vec<super::LibraryImage> {
    held.into_iter()
        .filter(|i| i.image_type == kind)
        .map(|i| super::LibraryImage {
            key: format!("{item_id}/{kind}/{}", i.image_index.unwrap_or(0)),
            remote: false,
            provider: None,
            // The first held one is the one the server shows.
            selected: i.image_index.unwrap_or(0) == 0,
            width: i.width,
            height: i.height,
            thumb: None,
        })
        .collect()
}

/// The online finds that are real `http(s)` URLs, which is all the browser
/// will be handed.
pub(crate) fn remote_images(found: MbRemoteImages) -> Vec<super::LibraryImage> {
    let web =
        |u: &str| reqwest::Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
    found
        .images
        .into_iter()
        .filter_map(|i| {
            let url = i.url.filter(|u| web(u))?;
            Some(super::LibraryImage {
                key: url,
                remote: true,
                provider: i.provider_name.filter(|p| !p.is_empty()),
                selected: false,
                width: i.width,
                height: i.height,
                thumb: i.thumbnail_url.filter(|u| web(u)),
            })
        })
        .collect()
}

/// Split and check a held key. It comes back from the browser when a picture
/// is chosen, so every part is constrained to what `held_images` writes: an id
/// of letters, digits and dashes, one of the image types the picker lists, and
/// a number.
pub(crate) fn parse_mb_image_key(key: &str) -> Option<(&str, &str, u32)> {
    let mut parts = key.split('/');
    let (item, kind, index) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some()
        || item.is_empty()
        || !item.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !matches!(kind, "Primary" | "Backdrop")
    {
        return None;
    }
    Some((item, kind, index.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/Items/{id}/Images` and `/Items/{id}/RemoteImages`, in the shape
    /// Jellyfin's published OpenAPI gives `ImageInfo` and `RemoteImageResult`.
    #[test]
    fn held_and_online_images_read_into_tiles() {
        let held: Vec<MbImageInfo> = serde_json::from_value(serde_json::json!([
            { "ImageType": "Primary", "ImageTag": "a", "Path": "/srv/music/x/folder.jpg",
              "Height": 1000, "Width": 1000, "Size": 1 },
            { "ImageType": "Backdrop", "ImageIndex": 0, "Width": 1920, "Height": 1080 },
            { "ImageType": "Backdrop", "ImageIndex": 1, "Width": 1920, "Height": 1080 },
            { "ImageType": "Logo", "ImageIndex": 0 }
        ]))
        .unwrap();
        let portrait = held_images("8f2a", "Primary", held);
        assert_eq!(portrait.len(), 1);
        assert_eq!(portrait[0].key, "8f2a/Primary/0", "an absent index is the first");
        assert_eq!((portrait[0].width, portrait[0].height), (Some(1000), Some(1000)));
        assert!(portrait[0].selected && !portrait[0].remote);

        let remote: MbRemoteImages = serde_json::from_value(serde_json::json!({
            "Images": [
                { "ProviderName": "TheAudioDB", "Url": "https://www.theaudiodb.com/a.jpg",
                  "ThumbnailUrl": "https://www.theaudiodb.com/a.jpg/preview", "Type": "Primary",
                  "Width": 700, "Height": 700 },
                { "ProviderName": "Odd", "Url": "file:///etc/passwd", "Type": "Primary" },
                { "ProviderName": "", "Url": "https://assets.fanart.tv/b.jpg", "Type": "Primary" }
            ],
            "TotalRecordCount": 3, "Providers": ["TheAudioDB", "FanArt"]
        }))
        .unwrap();
        let found = remote_images(remote);
        assert_eq!(found.len(), 2, "only web URLs reach the browser");
        assert!(found[0].remote);
        assert_eq!(found[0].provider.as_deref(), Some("TheAudioDB"));
        assert_eq!(found[0].thumb.as_deref(), Some("https://www.theaudiodb.com/a.jpg/preview"));
        assert_eq!(found[1].provider, None);
    }

    #[test]
    fn an_image_key_is_only_what_held_images_writes() {
        assert_eq!(parse_mb_image_key("8f2a-11/Backdrop/3"), Some(("8f2a-11", "Backdrop", 3)));
        assert_eq!(parse_mb_image_key("8f2a/Primary/x"), None);
        assert_eq!(parse_mb_image_key("8f2a/Primary/0/extra"), None);
        assert_eq!(parse_mb_image_key("8f2a/Logo/0"), None);
        assert_eq!(parse_mb_image_key("../Primary/0"), None);
        assert_eq!(parse_mb_image_key("/Primary/0"), None);
    }

    /// A stub answering several requests, choosing a body by request path.
    async fn routed_stub(
        routes: Vec<(&'static str, String)>,
        serve: usize,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = asked.clone();
        tokio::spawn(async move {
            for _ in 0..serve {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let line =
                    String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
                seen.lock().unwrap().push(line.clone());
                let payload = routes
                    .iter()
                    .find(|(pat, _)| line.contains(*pat))
                    .map(|(_, b)| b.clone())
                    .unwrap_or_else(|| "{}".to_string());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}"), asked)
    }

    /// A favourite is the signal, and the number — where there is one — is out
    /// of ten.
    ///
    /// `UserItemDataDto.Rating` is a nullable double inherited from
    /// MediaBrowser, and Jellyfin's own `POST /UserItems/{itemId}/Rating` takes
    /// **`likes` (a boolean) and no number at all**, so on a modern Jellyfin
    /// that field is usually empty and `IsFavorite` carries everything. Both
    /// are read: Emby still writes a number and an old library may hold them,
    /// and a field that is read costs nothing where a field that is not is
    /// unrecoverable.
    ///
    /// The three item types map to the three kinds a love can point at, and an
    /// item with neither a rating nor a heart is not a nought-star verdict.
    #[tokio::test]
    async fn a_favourite_and_a_ten_point_rating_both_arrive_and_nothing_else_does() {
        let items = r#"{"Items":[
            {"Id":"j1","Name":"Teardrop","Type":"Audio","Artists":["Massive Attack"],
             "Album":"Mezzanine","UserData":{"IsFavorite":true}},
            {"Id":"j2","Name":"Angel","Type":"Audio","Artists":["Massive Attack"],
             "Album":"Mezzanine","UserData":{"Rating":6.0,"IsFavorite":false}},
            {"Id":"j3","Name":"Mezzanine","Type":"MusicAlbum","Artists":["Massive Attack"],
             "UserData":{"Rating":10.0,"IsFavorite":false}},
            {"Id":"j4","Name":"Massive Attack","Type":"MusicArtist",
             "UserData":{"IsFavorite":true}},
            {"Id":"j5","Name":"Untouched","Type":"Audio","Artists":["Massive Attack"],
             "UserData":{"IsFavorite":false}}
        ],"TotalRecordCount":5}"#;
        let users = r#"[{"Id":"u1","Name":"ash"}]"#;
        let (url, _asked) =
            routed_stub(vec![("/Users", users.to_string()), ("/Items", items.to_string())], 4)
                .await;

        let src = MediaBrowserSource::jellyfin(url, "k".into(), "ash".into(), Egress::Open);
        let got = src.fetch_ratings().await.expect("the walk");

        let teardrop = got.iter().find(|r| r.name == "Teardrop").expect("Teardrop");
        assert!(teardrop.starred, "a heart is the statement a love makes");
        assert!(teardrop.rating.is_none(), "and carries no number to apply a threshold to");

        let angel = got.iter().find(|r| r.name == "Angel").expect("Angel");
        let rating = angel.rating.expect("a rating");
        assert_eq!(rating.value, 6.0);
        assert_eq!(rating.scale.max, 10.0, "ten, not five — this is a MediaBrowser double");

        // All three kinds, because a love can point at any of them.
        assert_eq!(got.iter().find(|r| r.name == "Mezzanine").unwrap().kind, "release");
        let artist = got.iter().find(|r| r.kind == "artist").expect("the artist");
        assert_eq!(artist.name, "Massive Attack");
        assert_eq!(artist.artist, "", "an artist *is* the name — the shape entity_key expects");

        // Neither rated nor hearted is not a verdict of nought.
        assert!(!got.iter().any(|r| r.name == "Untouched"));
        assert_eq!(got.len(), 4);
    }

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
        let jf = MediaBrowserSource::jellyfin(
            "http://jf:8096".into(),
            "k".into(),
            "a".into(),
            Egress::Open,
        );
        let emby = MediaBrowserSource::emby(
            "http://emby:8096".into(),
            "k".into(),
            "a".into(),
            Egress::Open,
        );

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
        let s = MediaBrowserSource::jellyfin(
            "http://jf:8096/".into(),
            "k".into(),
            "ashwin".into(),
            Egress::Open,
        );
        assert_eq!(s.url, "http://jf:8096");
        let e = MediaBrowserSource::emby(
            "http://emby:8096/".into(),
            "k".into(),
            "ashwin".into(),
            Egress::Open,
        );
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

    /// A minimal `Play` for driving the tracker. Only the identity fields
    /// matter here — the tracker keys on the `item_id` it is handed, and holds
    /// the play only so it can give it back when the track is displaced.
    fn named(title: &str) -> Play {
        Play::new(title.into(), "Test Artist".into(), 1_700_000_000, "src".into(), "test".into())
    }

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
                if t.observe("sess-1", "item-1", &named("Combustible"), secs, Some(200), false)
                    .ready
                {
                    banked += 1;
                }
            }
            assert_eq!(banked, pass + 1, "pass {pass} should have banked exactly one");
        }
    }

    /// A track replaced before it ever counted is a skip, and says how far in.
    ///
    /// This is the one thing a polled source can say about how a listen
    /// *ended*; everything else in this tracker is about one beginning. The
    /// information was always here and was thrown away one line later.
    #[test]
    fn a_track_replaced_before_it_counted_is_reported_as_skipped() {
        let mut t = SessionTracker::new();
        // Twelve seconds into the first track, then something else starts.
        t.observe("sess-1", "item-1", &named("Archangel"), 5, Some(200), false);
        t.observe("sess-1", "item-1", &named("Archangel"), 12, Some(200), false);
        let o = t.observe("sess-1", "item-2", &named("Near Dark"), 0, Some(200), false);

        let gone = o.abandoned.expect("the displaced track should be reported");
        assert_eq!(gone.play.title, "Archangel");
        assert_eq!(gone.listened_secs, 12, "how far in is the whole point");
        assert!(!o.ready);
    }

    /// A track that *did* cross the threshold is simply over, not skipped.
    ///
    /// Without this every completed track in a queue would be filed as a skip
    /// the moment the next one started — which is every track anyone ever
    /// plays, and would make the flag meaningless.
    #[test]
    fn a_track_that_counted_is_not_also_a_skip() {
        let mut t = SessionTracker::new();
        assert!(t.observe("sess-1", "item-1", &named("Archangel"), 195, Some(200), false).ready);
        let o = t.observe("sess-1", "item-2", &named("Near Dark"), 0, Some(200), false);
        assert!(o.abandoned.is_none(), "a banked listen is over, not abandoned");
    }

    /// A session going away is not evidence of anything.
    ///
    /// An item change means something else started, so the listener moved on.
    /// A session ending means the player closed, or they walked away, or the
    /// network blinked. Filing that as a skip would record "went to bed" as a
    /// judgement about a record.
    #[test]
    fn a_session_that_disappears_is_not_a_skip() {
        let mut t = SessionTracker::new();
        t.observe("sess-1", "item-1", &named("Archangel"), 12, Some(200), false);
        t.retain_live(&[]);
        // Nothing to report, and the slate is clean for a returning session.
        let o = t.observe("sess-1", "item-1", &named("Archangel"), 3, Some(200), false);
        assert!(o.abandoned.is_none());
    }

    /// A track put on repeat is not skipping itself.
    ///
    /// The repeat path resets `scrobbled` through the *rewind* branch, not the
    /// item-change branch, so it must never produce an abandoned track — a
    /// listener who loves a song enough to loop it would otherwise accrue a
    /// skip per pass.
    #[test]
    fn a_looped_track_never_reports_a_skip() {
        let mut t = SessionTracker::new();
        for _ in 0..3 {
            for secs in [3u64, 120, 195] {
                let o = t.observe("sess-1", "item-1", &named("Archangel"), secs, Some(200), false);
                assert!(o.abandoned.is_none(), "a repeat is not a skip");
            }
        }
    }

    /// Scrubbing back a few seconds inside one listen is not a new play, or
    /// fidgeting with the playhead would mint scrobbles.
    #[test]
    fn a_small_seek_back_is_not_a_repeat() {
        let mut t = SessionTracker::new();
        let mut banked = 0;
        for secs in [120u64, 115, 118, 160, 195] {
            if t.observe("sess-1", "item-1", &named("Combustible"), secs, Some(200), false).ready {
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
            if t.observe("sess-1", "item-1", &named("Combustible"), secs, Some(200), false).ready {
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
                if t.observe("sess-1", item, &named("track"), secs, Some(200), false).ready {
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
        assert!(!t.observe("sess-1", "item-1", &named("track"), 195, Some(200), true).ready);
        // …and resuming past it still banks exactly once.
        assert!(t.observe("sess-1", "item-1", &named("track"), 196, Some(200), false).ready);
    }

    /// A source that reports no duration falls back to the four-minute rule
    /// rather than never scrobbling.
    #[test]
    fn an_unknown_duration_falls_back_to_four_minutes() {
        let mut t = SessionTracker::new();
        assert!(!t.observe("s", "i", &named("track"), 239, None, false).ready);
        assert!(t.observe("s", "i", &named("track"), 240, None, false).ready);
    }

    /// A session that goes away is forgotten, so a returning one starts clean
    /// rather than inheriting a stale `scrobbled` flag.
    #[test]
    fn a_departed_session_is_forgotten() {
        let mut t = SessionTracker::new();
        assert!(t.observe("sess-1", "item-1", &named("track"), 195, Some(200), false).ready);
        t.retain_live(&[]);
        // Same session and item returning is a fresh play, not a suppressed one.
        assert!(t.observe("sess-1", "item-1", &named("track"), 195, Some(200), false).ready);
    }
}
