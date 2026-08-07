//! Jellyfin source.
//!
//! Structurally Plex's twin: poll `/Sessions`, watch the playhead, scrobble a
//! track once it passes the threshold. Two differences that matter:
//!
//! * **Auth is an API key**, sent as `X-Emby-Token`. Jellyfin lets a user mint
//!   one per client, so it's per-user in the same way a Plex token is.
//! * **There is no play history to backfill.** Jellyfin doesn't record listens
//!   natively — that needs the Playback Reporting plugin, which not everyone
//!   installs. So this source only ever sees what's playing now, unlike
//!   Navidrome where `/api/scrobble/` hands over years of it.
//!
//! Ticks are in .NET "ticks" (100-nanosecond units), which is why every duration
//! and position here is divided by ten million rather than a thousand.

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::any::Any;
use std::collections::HashMap;
use std::time::Duration;
use tracing::debug;

use crate::models::{AudioQuality, Play};
use crate::sources::MusicSource;

/// .NET ticks per second — Jellyfin reports positions and durations in these.
const TICKS_PER_SECOND: u64 = 10_000_000;

/// Scrobble once half the track has played, or after four minutes, whichever
/// comes first. Matches the Plex source and the Last.fm convention.
const SCROBBLE_FRACTION: f64 = 0.5;
const SCROBBLE_AFTER_SECS: u64 = 240;

/// How far the playhead must jump backwards before it counts as the track
/// starting over rather than the user scrubbing.
///
/// A repeat always lands near zero, so this only has to clear ordinary
/// seek-back noise. Jellyfin reports an exact tick count rather than Plex's
/// coarse `viewOffset`, but the same 20 seconds is used for both — the number
/// is about what a *person* means by "I skipped back a bit", not about the
/// precision of the source.
const REPEAT_REWIND_SECS: u64 = 20;

/// An account on the Jellyfin server. Sessions are matched on `name`, which is
/// what `SessionInfo.UserName` carries.
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct JellyfinAccount {
    pub id: String,
    pub name: String,
}

pub struct JellyfinSource {
    url: String,
    api_key: String,
    /// Which Jellyfin account this source belongs to. Everything from any other
    /// account is discarded — a shared server must not leak between users.
    account: String,
    client: Client,
    /// Per-session progress, so a track is scrobbled once rather than every poll.
    seen: HashMap<String, SessionProgress>,
}

#[derive(Debug, Clone)]
struct SessionProgress {
    item_id: String,
    scrobbled: bool,
    /// Last playhead seen, so a track starting over can be told from a seek.
    last_position: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinSession {
    #[serde(default)]
    id: String,
    #[serde(default)]
    user_name: Option<String>,
    #[serde(default)]
    now_playing_item: Option<NowPlayingItem>,
    #[serde(default)]
    play_state: Option<PlayState>,
    #[serde(default)]
    device_name: Option<String>,
    /// Stable per installed client — Jellyfin keeps it across restarts, which is
    /// what makes it usable as a `devices.machine_id`.
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    device_type: Option<String>,
    /// The app: "Jellyfin Media Player", "Finamp", "Jellyfin Web".
    #[serde(default)]
    client: Option<String>,
    /// Present **only while actually transcoding** — Jellyfin leaves it null for
    /// a direct play, so its absence is itself the answer.
    #[serde(default)]
    transcoding_info: Option<TranscodingInfo>,
}

/// The delivery half: what the server actually sent, as opposed to what is on
/// disk. Null unless a transcode is running.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
/// Only the fields Tapedeck has somewhere to put are parsed. `Container`,
/// `AudioChannels`, `Framerate` and the video ones are deliberately left out:
/// storing the delivery container or a downmix would need columns that don't
/// exist, and parsing a field with nowhere to put it is how the five dropped
/// quality fields happened in the first place. Add the column first.
struct TranscodingInfo {
    #[serde(default)]
    audio_codec: Option<String>,
    #[serde(default)]
    bitrate: Option<i32>,
    /// True when the audio is passed through untouched even though a transcode
    /// is running for some other reason.
    #[serde(default)]
    is_audio_direct: Option<bool>,
    /// An array of enum names in the 12.x schema, a comma-joined string in
    /// 10.8. Accepting one shape would be silently wrong on the other, and the
    /// symptom — an empty `transcode_reason` — looks exactly like a direct
    /// play.
    #[serde(default, deserialize_with = "string_or_seq")]
    transcode_reasons: Option<Vec<String>>,
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
pub struct JellyfinPlayer {
    pub machine_id: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub product: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct NowPlayingItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "Type", default)]
    item_type: String,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    album_artist: Option<String>,
    #[serde(default)]
    artists: Option<Vec<String>>,
    #[serde(default)]
    run_time_ticks: Option<u64>,
    #[serde(default)]
    index_number: Option<i32>,
    #[serde(default)]
    provider_ids: Option<ProviderIds>,
    #[serde(default)]
    media_streams: Option<Vec<MediaStream>>,
    /// The source container as it sits on disk — `flac`, `dsf`, `m4a`.
    #[serde(default)]
    container: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProviderIds {
    /// Jellyfin's own tag for the MusicBrainz recording id.
    #[serde(rename = "MusicBrainzTrack", default)]
    musicbrainz_track: Option<String>,
    #[serde(rename = "MusicBrainzAlbum", default)]
    musicbrainz_album: Option<String>,
    /// The performer. Jellyfin writes `MusicBrainzArtist` for the track artist
    /// and `MusicBrainzAlbumArtist` for the record's — prefer the former and
    /// fall back, since that is the order `Play.mbid_artist` means.
    #[serde(rename = "MusicBrainzArtist", default)]
    musicbrainz_artist: Option<String>,
    #[serde(rename = "MusicBrainzAlbumArtist", default)]
    musicbrainz_album_artist: Option<String>,
    #[serde(rename = "MusicBrainzReleaseGroup", default)]
    musicbrainz_release_group: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MediaStream {
    #[serde(default)]
    type_: Option<String>,
    #[serde(rename = "Type", default)]
    stream_type: Option<String>,
    #[serde(default)]
    codec: Option<String>,
    #[serde(default)]
    bit_rate: Option<i32>,
    #[serde(default)]
    sample_rate: Option<i32>,
    #[serde(default)]
    bit_depth: Option<i32>,
    #[serde(default)]
    channels: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PlayState {
    #[serde(default)]
    position_ticks: Option<u64>,
    #[serde(default)]
    is_paused: Option<bool>,
    /// `DirectPlay` | `DirectStream` | `Transcode`. The authoritative answer to
    /// "did this file reach the listener intact?" — `TranscodingInfo` describes
    /// *how* a transcode is happening, but this says *whether* one is.
    #[serde(default)]
    play_method: Option<String>,
}

/// A track Jellyfin says is playing, already attributed and converted.
#[derive(Debug, Clone)]
pub struct JellyfinTrack {
    pub play: Play,
    pub quality: Option<AudioQuality>,
    /// Seconds into the track — Jellyfin reports a real playhead, so the deck
    /// gets a true position rather than one counted up from first sight.
    pub position: u64,
    pub session_id: String,
    /// The client this is playing on, so the listen can carry a device and
    /// therefore a signal chain. `None` only when Jellyfin reported no
    /// `DeviceId` at all.
    pub player: Option<JellyfinPlayer>,
}

#[derive(Debug, Default)]
pub struct JellyfinSessions {
    pub now_playing: Vec<JellyfinTrack>,
    pub ready_to_scrobble: Vec<JellyfinTrack>,
}

impl JellyfinSource {
    pub fn new(url: String, api_key: String, account: String) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            api_key,
            account,
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|_| Client::new()),
            seen: HashMap::new(),
        }
    }

    /// Which account an API key belongs to.
    ///
    /// The same job `PlexSource::resolve_account` does, but answered by the
    /// server itself rather than a cloud service — so unlike Plex this works on
    /// a fully offline LAN install.
    pub async fn resolve_account(
        url: &str,
        api_key: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Me {
            #[serde(default)]
            name: Option<String>,
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
            .build()?;

        let resp = client
            .get(format!("{}/Users/Me", url.trim_end_matches('/')))
            .header("X-Emby-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err("Jellyfin rejected that API key".into());
        }

        // `/Users/Me` only answers for a token minted by a *user* login. An API
        // key created in the dashboard is server-scoped and belongs to nobody,
        // so Jellyfin cannot resolve "Me" and answers 400 — which is what every
        // connection test with an API key hit. Not an error to report: it just
        // means the account has to be named rather than inferred.
        if resp.status().is_success() {
            let me: Me = resp.json().await?;
            if let Some(name) = me.name.filter(|n| !n.trim().is_empty()) {
                return Ok(name);
            }
        }

        let users = Self::list_accounts(url, api_key).await?;
        match users.len() {
            0 => Err("Jellyfin reported no users for that key".into()),
            // One user is unambiguous, so don't make them pick.
            1 => Ok(users.into_iter().next().expect("checked").name),
            _ => Err(format!(
                "That API key isn't tied to a user, and this server has {}.                  Choose which account's listens to record.",
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
        url: &str,
        api_key: &str,
    ) -> Result<Vec<JellyfinAccount>, Box<dyn std::error::Error + Send + Sync>> {
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

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
            .build()?;

        let resp = client
            .get(format!("{}/Users", url.trim_end_matches('/')))
            .header("X-Emby-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err("Jellyfin rejected that API key".into());
        }
        if !resp.status().is_success() {
            return Err(format!("Jellyfin returned {} listing users", resp.status()).into());
        }

        let users: Vec<User> = resp.json().await?;
        Ok(users
            .into_iter()
            .filter(|u| !u.policy.as_ref().is_some_and(|p| p.is_disabled))
            .filter(|u| !u.name.trim().is_empty())
            .map(|u| JellyfinAccount { id: u.id, name: u.name })
            .collect())
    }

    /// Confirm the server answers and the key is good.
    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let resp = self
            .client
            .get(format!("{}/System/Info", self.url))
            .header("X-Emby-Token", &self.api_key)
            .send()
            .await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err("Jellyfin rejected that API key".into());
        }
        if !resp.status().is_success() {
            return Err(format!("Jellyfin returned {}", resp.status()).into());
        }
        Ok(())
    }

    /// Poll `/Sessions`, returning what's playing and what has crossed the
    /// scrobble threshold since the last poll.
    pub async fn fetch_sessions(&mut self) -> Result<JellyfinSessions, Box<dyn std::error::Error>> {
        let resp = self
            .client
            .get(format!("{}/Sessions", self.url))
            .header("X-Emby-Token", &self.api_key)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Jellyfin /Sessions returned {}", resp.status()).into());
        }
        let sessions: Vec<JellyfinSession> = resp.json().await?;
        Ok(self.consume_sessions(sessions))
    }

    /// The pure half of `fetch_sessions`: decide what is playing and what has
    /// crossed the threshold, given a batch of sessions.
    ///
    /// Split out so the repeat and seek behaviour can be tested without a
    /// server — the same reason Plex has `process_live_track`. That behaviour
    /// is state held across calls, which is exactly the kind that goes wrong
    /// silently.
    pub fn consume_sessions(&mut self, sessions: Vec<JellyfinSession>) -> JellyfinSessions {
        let mut result = JellyfinSessions::default();
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
            // Jellyfin sessions cover video too.
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

            let track = JellyfinTrack {
                quality: Self::extract_quality(
                    item,
                    session.transcoding_info.as_ref(),
                    session.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
                ),
                play: Self::to_play(item, &session.device_name),
                position,
                session_id: session.id.clone(),
                player: Self::to_player(&session),
            };
            live_sessions.push(session.id.clone());

            // A paused track is still "now playing" for the deck, but its
            // position isn't advancing, so it shouldn't cross the threshold.
            let paused = session.play_state.as_ref().and_then(|p| p.is_paused).unwrap_or(false);

            let entry = self.seen.entry(session.id.clone()).or_insert_with(|| SessionProgress {
                item_id: item.id.clone(),
                scrobbled: false,
                last_position: position,
            });

            // A new track in the same session resets the scrobble state,
            // otherwise only the first song of a queue would ever count.
            if entry.item_id != item.id {
                entry.item_id = item.id.clone();
                entry.scrobbled = false;
            } else if entry.last_position > position
                && entry.last_position - position >= REPEAT_REWIND_SECS
            {
                // A track on repeat is invisible to the item check above:
                // Jellyfin keeps the same session *and* the same item, so
                // `scrobbled` stayed true forever and only the first of N plays
                // was ever banked — the same bug Plex had until v0.13.3, which
                // this mirrors. The playhead jumping backwards is the one
                // signal that the track started over.
                //
                // Requiring that we had *already* banked this pass is what
                // stops scrubbing around inside one listen minting extra
                // scrobbles: a rewind before the threshold changes nothing,
                // because there was nothing to undo.
                if entry.scrobbled {
                    debug!(
                        "Playhead rewound in session {} ({}s -> {}s): '{}' started over",
                        session.id, entry.last_position, position, item.name
                    );
                }
                entry.scrobbled = false;
            }
            entry.last_position = position;

            let past_threshold = !paused
                && match duration {
                    Some(d) if d > 0 => {
                        position as f64 >= d as f64 * SCROBBLE_FRACTION
                            || position >= SCROBBLE_AFTER_SECS
                    }
                    _ => position >= SCROBBLE_AFTER_SECS,
                };

            if past_threshold && !entry.scrobbled {
                entry.scrobbled = true;
                // The banked copy gets a source_id unique to *this pass*. The
                // now-playing copy keeps the stable one, which is what the deck
                // wants. Without this the id was `jellyfin-<item>-<device>` —
                // identical on every repeat, so `Database::exists` dropped the
                // second and third plays permanently, not merely within a
                // window. Plex has always put the scrobble time in its suffix
                // for exactly this reason.
                let mut banked = track.clone();
                banked.play.source_id = format!("jellyfin-{}-scrobble-{}", item.id, now_secs());
                result.ready_to_scrobble.push(banked);
            }
            result.now_playing.push(track);
        }

        // Forget sessions that have gone away, so the map doesn't grow forever
        // and a returning session starts clean.
        self.seen.retain(|id, _| live_sessions.contains(id));
        result
    }

    fn to_play(item: &NowPlayingItem, device: &Option<String>) -> Play {
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
            source_id: format!("jellyfin-{}-{}", item.id, device.as_deref().unwrap_or("session")),
            source_name: "jellyfin".into(),
            title: item.name.clone(),
            artist,
            album: item.album.clone(),
            timestamp: now_secs(),
            duration: item.run_time_ticks.map(|t| t / TICKS_PER_SECOND),
            track_number: item.index_number.filter(|n| *n > 0),
            // Jellyfin carries MusicBrainz ids when the library was tagged —
            // another listen that never needs a MusicBrainz lookup.
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
    /// `DeviceId` is the key: it is stable per installed client across
    /// restarts, where `DeviceName` is a label the user can change. A session
    /// without one can't be a device, so it stays `None` rather than being
    /// keyed on something that will drift.
    fn to_player(session: &JellyfinSession) -> Option<JellyfinPlayer> {
        let machine_id = session.device_id.clone()?;
        Some(JellyfinPlayer {
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
    fn extract_quality(
        item: &NowPlayingItem,
        transcoding: Option<&TranscodingInfo>,
        play_method: Option<&str>,
    ) -> Option<AudioQuality> {
        let streams = item.media_streams.as_ref()?;
        let audio = streams.iter().find(|s| {
            let t = s.stream_type.as_deref().or(s.type_.as_deref()).unwrap_or("");
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
                // Jellyfin reports this in bits per second; Tapedeck's other
                // sources and the score thresholds are in kbps.
                q.delivery_bitrate = t.bitrate.map(|b| if b > 10_000 { b / 1000 } else { b });
            }
        }

        q.compute_score();
        Some(q)
    }

    /// Cover art bytes, proxied for the same reason as Plex's and Navidrome's.
    /// Find an artist by name, returning their item id.
    ///
    /// `searchTerm` is a fuzzy match on Jellyfin's side, so the name is checked
    /// here the same way it is for Plex — a search hit is a suggestion, not an
    /// identification.
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
            image_tags: std::collections::HashMap<String, String>,
        }

        let resp = self
            .client
            .get(format!("{}/Artists", self.url))
            .query(&[("searchTerm", name), ("limit", "10")])
            .header("X-Emby-Token", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Jellyfin returned {} searching artists", resp.status()).into());
        }
        let body: Resp = resp.json().await?;

        Ok(body
            .items
            .into_iter()
            // No Primary image means nothing to fetch, so a match without one
            // is not a match worth returning.
            .find(|i| {
                crate::sources::plex::artist_matches(&i.name, name)
                    && i.image_tags.contains_key("Primary")
            })
            .map(|i| i.id))
    }

    /// The item id of the account this source is pinned to.
    ///
    /// A playlist has an owner on Jellyfin, and an API key minted in the
    /// dashboard belongs to nobody — so `/Users/Me` is no help and the account
    /// name the source already carries has to be resolved against `/Users`.
    /// Without this the playlist would either be refused or land on whichever
    /// account Jellyfin picked, which on a household server is somebody else's.
    pub async fn account_id(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let users = Self::list_accounts(&self.url, &self.api_key).await?;
        if self.account.trim().is_empty() {
            return match users.len() {
                1 => Ok(users.into_iter().next().expect("checked").id),
                _ => Err("This Jellyfin source has no account set, and the server has several — \
                          set one before pushing a playlist"
                    .into()),
            };
        }
        users
            .into_iter()
            .find(|u| u.name.eq_ignore_ascii_case(self.account.trim()))
            .map(|u| u.id)
            .ok_or_else(|| format!("Jellyfin has no user called {}", self.account).into())
    }

    /// Tracks matching a free-text query, as candidates for a playlist push.
    ///
    /// `searchTerm` is fuzzy on Jellyfin's side — the same caution `find_artist`
    /// documents — so these are suggestions and `crate::playlists::pick`
    /// decides.
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

        let resp = self
            .client
            .get(format!("{}/Items", self.url))
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
            return Err(format!("Jellyfin returned {} searching tracks", resp.status()).into());
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

        // The body, not the query parameters: Jellyfin marks every one of those
        // deprecated on this endpoint, and they carry the same fields.
        let resp = self
            .client
            .post(format!("{}/Playlists", self.url))
            .header("X-Emby-Token", &self.api_key)
            .header("Accept", "application/json")
            .json(&serde_json::json!({
                "Name": name,
                "Ids": ids,
                "UserId": user_id,
                "MediaType": "Audio",
            }))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Jellyfin playlist create returned {}", resp.status()).into());
        }
        let made: Made = resp.json().await?;
        Ok(made.id)
    }

    pub async fn fetch_cover_art(
        &self,
        item_id: &str,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error + Send + Sync>> {
        let resp = self
            .client
            .get(format!("{}/Items/{}/Images/Primary", self.url, item_id))
            .header("X-Emby-Token", &self.api_key)
            .query(&[("maxWidth", "500")])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Jellyfin artwork returned {}", resp.status()).into());
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

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

#[async_trait]
impl MusicSource for JellyfinSource {
    fn name(&self) -> &str {
        "Jellyfin"
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    /// Jellyfin has no history to page through — everything comes from the live
    /// session poll, so this returns nothing rather than pretending otherwise.
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

    /// `/Users` is the fallback that makes a dashboard API key usable at all —
    /// `/Users/Me` answers 400 for one, because a server-scoped key belongs to
    /// no user. Pinned against a real 10.11.11 response: these names are
    /// PascalCase and a mismatch would be silent, leaving the picker empty and
    /// the source unconfigurable with no error to explain it.
    #[test]
    fn the_users_list_parses_a_real_response() {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Policy {
            #[serde(default)]
            is_disabled: bool,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct User {
            id: String,
            name: String,
            #[serde(default)]
            policy: Option<Policy>,
        }

        let body = r#"[
            {
                "Name": "abalaji",
                "ServerId": "f61075a77ace44edb6a28a67eb9cfef0",
                "Id": "2ed208b8f7314587915b25f0602d8df8",
                "HasPassword": true,
                "Policy": { "IsAdministrator": true, "IsHidden": true, "IsDisabled": false }
            },
            {
                "Name": "retired",
                "Id": "0000",
                "Policy": { "IsAdministrator": false, "IsDisabled": true }
            }
        ]"#;

        let users: Vec<User> = serde_json::from_str(body).expect("parses");
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].name, "abalaji");
        assert_eq!(users[0].id, "2ed208b8f7314587915b25f0602d8df8");

        // A disabled account can't play anything, so offering it would only be
        // a way to configure a source that never scrobbles.
        let live: Vec<&User> =
            users.iter().filter(|u| !u.policy.as_ref().is_some_and(|p| p.is_disabled)).collect();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].name, "abalaji");
    }

    fn item(id: &str, secs: u64) -> NowPlayingItem {
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
                type_: None,
                stream_type: Some("Audio".into()),
                codec: Some("flac".into()),
                bit_rate: Some(900_000),
                sample_rate: Some(44100),
                bit_depth: Some(16),
                channels: Some(2),
            }]),
        }
    }

    /// A `/Sessions` element carrying the transcode and client fields.
    ///
    /// **Built from the 12.0.0 OpenAPI schema, not captured from a live
    /// server** — unlike `the_users_list_parses_a_real_response` above, which
    /// is a real 10.11.11 body. It pins the PascalCase spellings and the shapes
    /// the schema declares, which is the half that fails silently; it cannot
    /// prove Jellyfin actually populates these on a session, and that is worth
    /// confirming against a real server before relying on the transcode data.
    const SESSION_TRANSCODING: &str = r#"{
        "Id": "sess-1",
        "UserName": "abalaji",
        "Client": "Finamp",
        "DeviceName": "Pixel 8",
        "DeviceId": "0f1e2d3c4b5a",
        "DeviceType": "Android",
        "ApplicationVersion": "0.9.2",
        "PlayState": {
            "PositionTicks": 1230000000,
            "IsPaused": false,
            "PlayMethod": "Transcode"
        },
        "TranscodingInfo": {
            "AudioCodec": "opus",
            "Container": "ogg",
            "Bitrate": 128000,
            "AudioChannels": 2,
            "IsAudioDirect": false,
            "TranscodeReasons": ["ContainerNotSupported", "AudioCodecNotSupported"]
        },
        "NowPlayingItem": {
            "Id": "item-1",
            "Name": "Xtal",
            "Type": "Audio",
            "Album": "SAW 85-92",
            "Artists": ["Aphex Twin"],
            "Container": "flac",
            "RunTimeTicks": 2930000000,
            "IndexNumber": 1,
            "ProviderIds": { "MusicBrainzTrack": "mb-track" },
            "MediaStreams": [
                { "Type": "Audio", "Codec": "flac", "BitRate": 900000,
                  "SampleRate": 44100, "BitDepth": 16, "Channels": 2 }
            ]
        }
    }"#;

    #[test]
    fn a_session_parses_its_client_and_transcode_fields() {
        let s: JellyfinSession = serde_json::from_str(SESSION_TRANSCODING).expect("parses");
        let player = JellyfinSource::to_player(&s).expect("has a device id");
        // DeviceId is the key, not DeviceName — the name is user-editable and
        // would re-register the client as a new device when changed.
        assert_eq!(player.machine_id, "0f1e2d3c4b5a");
        assert_eq!(player.name.as_deref(), Some("Pixel 8"));
        assert_eq!(player.product.as_deref(), Some("Finamp"));
        assert_eq!(player.platform.as_deref(), Some("Android"));

        let item = s.now_playing_item.as_ref().unwrap();
        let q = JellyfinSource::extract_quality(
            item,
            s.transcoding_info.as_ref(),
            s.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
        )
        .expect("has streams");

        // The source half is the file on disk and must not be overwritten by
        // what was delivered.
        assert_eq!(q.codec.as_deref(), Some("flac"));
        assert_eq!(q.container.as_deref(), Some("flac"));
        assert_eq!(q.is_lossless, Some(true));
        assert_eq!(q.sample_rate, Some(44100));

        // The delivery half.
        assert_eq!(q.is_transcoded, Some(true));
        assert_eq!(q.delivery_codec.as_deref(), Some("opus"));
        assert_eq!(q.delivery_bitrate, Some(128), "bits per second → kbps");
        assert_eq!(
            q.transcode_reason.as_deref(),
            Some("ContainerNotSupported, AudioCodecNotSupported")
        );
    }

    /// Jellyfin 10.8 sent `TranscodeReasons` as a comma-joined string; 12.x
    /// sends an array. Reading only one shape leaves `transcode_reason` empty
    /// on the other — which looks exactly like a direct play, so nothing
    /// errors and the data is quietly wrong.
    #[test]
    fn transcode_reasons_parse_as_a_string_or_an_array() {
        let as_array: JellyfinSession = serde_json::from_str(SESSION_TRANSCODING).unwrap();
        let as_string: JellyfinSession = serde_json::from_str(&SESSION_TRANSCODING.replace(
            r#"["ContainerNotSupported", "AudioCodecNotSupported"]"#,
            r#""ContainerNotSupported, AudioCodecNotSupported""#,
        ))
        .expect("the 10.8 spelling parses too");

        let reasons = |s: &JellyfinSession| {
            s.transcoding_info.as_ref().unwrap().transcode_reasons.clone().unwrap()
        };
        assert_eq!(reasons(&as_array), reasons(&as_string));
        assert_eq!(reasons(&as_array).len(), 2);
    }

    /// A direct play must not claim a delivery codec. `compute_score` docks
    /// points whenever `delivery_codec` is set, so reporting one for an
    /// untouched file would quietly penalise the best listens on the system.
    #[test]
    fn a_direct_play_reports_no_delivery_downgrade() {
        let body = SESSION_TRANSCODING
            .replace(r#""PlayMethod": "Transcode""#, r#""PlayMethod": "DirectPlay""#);
        let body = body[..body.find(r#""TranscodingInfo""#).unwrap()].to_string()
            + &body[body.find(r#""NowPlayingItem""#).unwrap()..];
        let s: JellyfinSession =
            serde_json::from_str(&body).expect("parses without TranscodingInfo");

        let q = JellyfinSource::extract_quality(
            s.now_playing_item.as_ref().unwrap(),
            s.transcoding_info.as_ref(),
            s.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
        )
        .unwrap();
        assert_eq!(q.is_transcoded, Some(false));
        assert_eq!(q.delivery_codec, None);
        assert_eq!(q.transcode_reason, None);
    }

    /// A video transcode can leave the audio untouched. Calling that a
    /// transcoded listen would be wrong, and `IsAudioDirect` is the only field
    /// that says so.
    #[test]
    fn a_passthrough_audio_stream_is_not_a_transcoded_listen() {
        let s: JellyfinSession = serde_json::from_str(
            &SESSION_TRANSCODING.replace(r#""IsAudioDirect": false"#, r#""IsAudioDirect": true"#),
        )
        .unwrap();
        let q = JellyfinSource::extract_quality(
            s.now_playing_item.as_ref().unwrap(),
            s.transcoding_info.as_ref(),
            s.play_state.as_ref().and_then(|p| p.play_method.as_deref()),
        )
        .unwrap();
        assert_eq!(q.is_transcoded, Some(false));
        assert_eq!(q.delivery_codec, None);
    }

    #[test]
    fn converts_dotnet_ticks_to_seconds() {
        // 293s expressed in 100-nanosecond units; reading these as milliseconds
        // would make every track absurdly long and break the scrobble threshold.
        let p = JellyfinSource::to_play(&item("i1", 293), &Some("Living Room".into()));
        assert_eq!(p.duration, Some(293));
    }

    #[test]
    fn carries_library_mbids_and_track_number() {
        let p = JellyfinSource::to_play(&item("i1", 293), &None);
        assert_eq!(p.mbid_recording.as_deref(), Some("mb-track"));
        assert_eq!(p.mbid_release.as_deref(), Some("mb-album"));
        assert_eq!(p.track_number, Some(1));
        assert_eq!(p.artist, "Aphex Twin");
        assert_eq!(p.source_name, "jellyfin");
    }

    #[test]
    fn reads_quality_from_the_audio_stream() {
        let q = JellyfinSource::extract_quality(&item("i1", 293), None, Some("DirectPlay"))
            .expect("has streams");
        assert_eq!(q.codec.as_deref(), Some("flac"));
        assert_eq!(q.sample_rate, Some(44100));
        assert_eq!(q.bit_depth, Some(16));
        assert_eq!(q.is_lossless, Some(true));
        assert!(q.quality_score.is_some());
    }

    #[test]
    fn source_id_is_stable_for_one_play_of_one_item() {
        let a = JellyfinSource::to_play(&item("i1", 293), &Some("Desk".into()));
        let b = JellyfinSource::to_play(&item("i1", 293), &Some("Desk".into()));
        assert_eq!(a.source_id, b.source_id);
        // A different device is a different playback.
        let c = JellyfinSource::to_play(&item("i1", 293), &Some("Phone".into()));
        assert_ne!(a.source_id, c.source_id);
    }

    /// Build a `/Sessions` body for one track at a given playhead, always the
    /// same session and the same item — which is exactly what repeat looks
    /// like from here.
    fn session_at(position_secs: u64) -> String {
        format!(
            r#"[{{
                "Id": "sess-1", "UserName": "abalaji", "DeviceId": "dev-1",
                "PlayState": {{ "PositionTicks": {}, "IsPaused": false, "PlayMethod": "DirectPlay" }},
                "NowPlayingItem": {{
                    "Id": "item-1", "Name": "Combustible", "Type": "Audio",
                    "Album": "Blonde", "Artists": ["Cœur de pirate"],
                    "RunTimeTicks": {}, "IndexNumber": 1
                }}
            }}]"#,
            position_secs * TICKS_PER_SECOND,
            200 * TICKS_PER_SECOND
        )
    }

    /// Playing one track three times on repeat must bank three listens.
    ///
    /// This is the bug Plex had until v0.13.3 and Jellyfin still had after it:
    /// the session and the item are both unchanged on a repeat, so the
    /// `item_id` check never fired, `scrobbled` stayed true, and only the first
    /// of N plays was ever recorded. Nothing errored — the listens were simply
    /// missing.
    #[tokio::test]
    async fn a_looped_track_scrobbles_once_per_pass() {
        let mut src = JellyfinSource::new("http://jf:8096".into(), "k".into(), "abalaji".into());
        let mut banked = 0usize;

        for pass in 0..3 {
            // Each pass walks the playhead from near zero, past the halfway
            // threshold, to the end. Only the offset says a new play began.
            for secs in [3u64, 120, 195] {
                let sessions: Vec<JellyfinSession> =
                    serde_json::from_str(&session_at(secs)).unwrap();
                banked += src.consume_sessions(sessions).ready_to_scrobble.len();
            }
            assert_eq!(banked, pass + 1, "pass {pass} should have banked exactly one");
        }
    }

    /// Scrubbing back a few seconds inside one listen is not a new play, or
    /// fidgeting with the playhead would mint scrobbles.
    #[tokio::test]
    async fn a_small_seek_back_is_not_a_repeat() {
        let mut src = JellyfinSource::new("http://jf:8096".into(), "k".into(), "abalaji".into());
        let mut banked = 0usize;
        for secs in [120u64, 115, 118, 160, 195] {
            let sessions: Vec<JellyfinSession> = serde_json::from_str(&session_at(secs)).unwrap();
            banked += src.consume_sessions(sessions).ready_to_scrobble.len();
        }
        assert_eq!(banked, 1, "one listen, however much the playhead was nudged");
    }

    #[test]
    fn url_trailing_slash_is_normalised() {
        let s = JellyfinSource::new("http://jf:8096/".into(), "k".into(), "ashwin".into());
        assert_eq!(s.url, "http://jf:8096");
    }
}
