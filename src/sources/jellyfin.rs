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

use crate::models::{AudioQuality, Play};
use crate::sources::MusicSource;

/// .NET ticks per second — Jellyfin reports positions and durations in these.
const TICKS_PER_SECOND: u64 = 10_000_000;

/// Scrobble once half the track has played, or after four minutes, whichever
/// comes first. Matches the Plex source and the Last.fm convention.
const SCROBBLE_FRACTION: f64 = 0.5;
const SCROBBLE_AFTER_SECS: u64 = 240;

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
}

#[derive(Debug, Deserialize)]
struct ProviderIds {
    /// Jellyfin's own tag for the MusicBrainz recording id.
    #[serde(rename = "MusicBrainzTrack", default)]
    musicbrainz_track: Option<String>,
    #[serde(rename = "MusicBrainzAlbum", default)]
    musicbrainz_album: Option<String>,
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
        if !resp.status().is_success() {
            return Err(format!("Jellyfin returned {}", resp.status()).into());
        }
        let me: Me = resp.json().await?;
        me.name
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| "Jellyfin did not report a username for that key".into())
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
            let Some(item) = session.now_playing_item else {
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
                quality: Self::extract_quality(&item),
                play: Self::to_play(&item, &session.device_name),
                position,
                session_id: session.id.clone(),
            };
            live_sessions.push(session.id.clone());

            // A paused track is still "now playing" for the deck, but its
            // position isn't advancing, so it shouldn't cross the threshold.
            let paused = session.play_state.as_ref().and_then(|p| p.is_paused).unwrap_or(false);

            let entry = self
                .seen
                .entry(session.id.clone())
                .or_insert_with(|| SessionProgress { item_id: item.id.clone(), scrobbled: false });

            // A new track in the same session resets the scrobble state,
            // otherwise only the first song of a queue would ever count.
            if entry.item_id != item.id {
                entry.item_id = item.id.clone();
                entry.scrobbled = false;
            }

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
                result.ready_to_scrobble.push(track.clone());
            }
            result.now_playing.push(track);
        }

        // Forget sessions that have gone away, so the map doesn't grow forever
        // and a returning session starts clean.
        self.seen.retain(|id, _| live_sessions.contains(id));
        Ok(result)
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
            mbid_artist: None,
            artists: item.artists.clone(),
            mbid_release_group: None,
            caa_id: None,
            caa_release_mbid: None,
        }
    }

    fn extract_quality(item: &NowPlayingItem) -> Option<AudioQuality> {
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

        let mut q = AudioQuality {
            format_type: Some(if codec_lower == "dsd" { "dsd" } else { "pcm" }.to_string()),
            codec: audio.codec.clone(),
            bitrate: audio.bit_rate,
            sample_rate: audio.sample_rate,
            bit_depth: audio.bit_depth.map(|b| b as i16),
            channels: audio.channels.map(|c| c as i16),
            is_lossless: Some(is_lossless),
            ..Default::default()
        };
        q.compute_score();
        Some(q)
    }

    /// Cover art bytes, proxied for the same reason as Plex's and Navidrome's.
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
            provider_ids: Some(ProviderIds {
                musicbrainz_track: Some("mb-track".into()),
                musicbrainz_album: Some("mb-album".into()),
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
        let q = JellyfinSource::extract_quality(&item("i1", 293)).expect("has streams");
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

    #[test]
    fn url_trailing_slash_is_normalised() {
        let s = JellyfinSource::new("http://jf:8096/".into(), "k".into(), "ashwin".into());
        assert_eq!(s.url, "http://jf:8096");
    }
}
