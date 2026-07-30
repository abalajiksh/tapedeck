//! Navidrome source — talks both of Navidrome's APIs at once.
//!
//! Navidrome exposes two, and neither alone is enough:
//!
//! * **Native** (`/auth/login` → `x-nd-authorization`) is the only one that
//!   returns *historical* scrobbles, via `/api/scrobble/?from=<ts>`. That's what
//!   makes a real backfill possible rather than only catching live playback.
//!   The rows are just `{mediaFileId, submissionTime}`, so track details come
//!   from `/api/song`.
//! * **Subsonic** (`/rest/*`, salted-MD5 auth) is what everything else speaks.
//!   It answers `getNowPlaying`, and its `getArtist`/`getAlbum` carry MusicBrainz
//!   IDs and cover art — which is why Phase 4 can lean on a user's own library
//!   instead of the MusicBrainz API.
//!
//! Subsonic hashes `password + salt` per request, so the account password is
//! required rather than a token; it's held encrypted at rest like every other
//! stored credential. The native session token expires, so it's re-established
//! transparently on a 401.
//!
//! Ported from `temp/navidrome-navanalyze`, deliberately *without* its
//! `tls_danger_accept_invalid_certs(true)` — that disables certificate checking
//! entirely, which is not a trade a scrobbler should make on the user's behalf.

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::any::Any;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, warn};

use crate::models::Play;
use crate::sources::MusicSource;

const SUBSONIC_API_VERSION: &str = "1.16.1";

pub struct NavidromeSource {
    url: String,
    username: String,
    password: String,
    client: Client,
    /// Native-API bearer, re-fetched on expiry.
    nd_token: Option<String>,
    /// mediaFileId → track details, filled lazily from `/api/song`.
    songs: HashMap<String, NavidromeSong>,
    /// Highest `submissionTime` already returned, so a poll only asks for what's
    /// new. Zero means "we haven't looked yet" and triggers the initial backfill.
    last_submission: u64,
}

#[derive(Debug, Clone)]
pub struct NavidromeSong {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<u64>,
    pub track_number: Option<i32>,
    pub mbid: Option<String>,
}

// ── Native API wire types ──

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeScrobble {
    media_file_id: String,
    submission_time: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeSong {
    id: String,
    title: String,
    artist: String,
    album: Option<String>,
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    track_number: Option<i32>,
    /// Navidrome has spelled this several ways across versions, and getting it
    /// wrong is silent: the MBID is simply absent and every listen pays for a
    /// MusicBrainz lookup it didn't need. Accept the known spellings.
    #[serde(
        default,
        alias = "mbzRecordingID",
        alias = "mbzRecordingId",
        alias = "mbzTrackId",
        alias = "mbzTrackID",
        alias = "musicBrainzId"
    )]
    mbz_recording_id: Option<String>,
}

// ── Subsonic wire types ──

#[derive(Deserialize)]
struct SubsonicEnvelope {
    #[serde(rename = "subsonic-response")]
    response: SubsonicResponse,
}

#[derive(Deserialize)]
struct SubsonicResponse {
    status: String,
    #[serde(default)]
    error: Option<SubsonicError>,
    #[serde(rename = "nowPlaying", default)]
    now_playing: Option<NowPlayingContainer>,
}

#[derive(Deserialize)]
struct SubsonicError {
    message: Option<String>,
}

#[derive(Deserialize)]
struct NowPlayingContainer {
    #[serde(default)]
    entry: Vec<NowPlayingEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NowPlayingEntry {
    id: String,
    title: String,
    artist: String,
    album: Option<String>,
    #[serde(default)]
    duration: Option<u64>,
    #[serde(default)]
    track: Option<i32>,
    /// Which Navidrome account is playing — used to keep a shared server's other
    /// users out of this source's owner's history.
    #[serde(default)]
    username: Option<String>,
    /// Seconds since this entry was last updated by the player.
    #[serde(default)]
    minutes_ago: Option<u64>,
}

/// One live track, already attributed to the account that's playing it.
#[derive(Debug, Clone)]
pub struct NavidromeNowPlaying {
    pub play: Play,
    pub username: Option<String>,
    pub minutes_ago: u64,
}

impl NavidromeSource {
    pub fn new(url: String, username: String, password: String) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            username,
            password,
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|_| Client::new()),
            nd_token: None,
            songs: HashMap::new(),
            last_submission: 0,
        }
    }

    /// Resume from a known high-water mark so a restart doesn't re-walk history.
    pub fn resume_from(&mut self, submission_time: u64) {
        self.last_submission = submission_time;
    }

    pub fn last_submission(&self) -> u64 {
        self.last_submission
    }

    /// The account these credentials belong to. Unlike Plex there's no lookup
    /// needed — you log in *as* the user, so the username is the identity.
    pub fn username_owned(&self) -> String {
        self.username.clone()
    }

    /// Log in and confirm both APIs answer. Also the connection test.
    pub async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.login().await?;
        self.subsonic_ping().await?;
        Ok(())
    }

    // ── Native API ──

    async fn login(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let resp = self.client
            .post(format!("{}/auth/login", self.url))
            .json(&serde_json::json!({ "username": self.username, "password": self.password }))
            .send().await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err("Navidrome rejected those credentials".into());
        }
        if !resp.status().is_success() {
            return Err(format!("Navidrome login returned {}", resp.status()).into());
        }
        let body: LoginResponse = resp.json().await?;
        self.nd_token = Some(body.token);
        Ok(())
    }

    /// GET against the native API, logging in again once if the token expired.
    async fn native_get(&mut self, path: &str, query: &[(&str, String)])
        -> Result<reqwest::Response, Box<dyn std::error::Error>>
    {
        for attempt in 0..2 {
            if self.nd_token.is_none() {
                self.login().await?;
            }
            let token = self.nd_token.clone().unwrap_or_default();
            let resp = self.client
                .get(format!("{}{}", self.url, path))
                .header("x-nd-authorization", format!("Bearer {token}"))
                .query(query)
                .send().await?;

            if resp.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                debug!("Navidrome native token expired; re-authenticating");
                self.nd_token = None;
                continue;
            }
            if !resp.status().is_success() {
                return Err(format!("Navidrome {} returned {}", path, resp.status()).into());
            }
            return Ok(resp);
        }
        Err("Navidrome authentication kept failing".into())
    }

    /// Scrobbles submitted after `from`. This is the backfill: Navidrome keeps
    /// its own play history, so a new Tapedeck install can pull years of it.
    pub async fn fetch_scrobbles(&mut self, from: u64)
        -> Result<Vec<Play>, Box<dyn std::error::Error>>
    {
        let resp = self.native_get("/api/scrobble/", &[
            ("from", from.to_string()),
            ("_start", "0".into()),
            ("_end", "10000".into()),
        ]).await?;
        let rows: Vec<NativeScrobble> = resp.json().await?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }

        // Only load the catalogue when we actually have rows referencing it.
        if rows.iter().any(|r| !self.songs.contains_key(&r.media_file_id)) {
            self.load_songs().await?;
        }

        let mut plays = Vec::new();
        let mut newest = self.last_submission;
        for row in rows {
            newest = newest.max(row.submission_time);
            let Some(song) = self.songs.get(&row.media_file_id) else {
                debug!("Navidrome scrobble references unknown song {}", row.media_file_id);
                continue;
            };
            plays.push(song.to_play(row.submission_time, &row.media_file_id));
        }
        self.last_submission = newest;
        Ok(plays)
    }

    async fn load_songs(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let resp = self.native_get("/api/song", &[
            ("_start", "0".into()),
            ("_end", "100000".into()),
        ]).await?;
        let songs: Vec<NativeSong> = resp.json().await?;
        self.songs = songs.into_iter().map(|s| (s.id.clone(), NavidromeSong {
            title: s.title,
            artist: s.artist,
            album: s.album,
            duration: (s.duration > 0.0).then_some(s.duration as u64),
            track_number: s.track_number.filter(|n| *n > 0),
            mbid: s.mbz_recording_id.filter(|m| !m.is_empty()),
        })).collect();
        debug!("Navidrome: cached {} songs", self.songs.len());
        Ok(())
    }

    // ── Subsonic API ──

    /// Salted-MD5 auth, computed per request as the protocol requires.
    fn subsonic_params(&self) -> Vec<(String, String)> {
        use rand::Rng;
        let salt: String = (0..12)
            .map(|_| rand::rng().sample(rand::distr::Alphanumeric) as char)
            .collect();
        let token = format!("{:x}", md5::compute(format!("{}{}", self.password, salt)));
        vec![
            ("u".into(), self.username.clone()),
            ("t".into(), token),
            ("s".into(), salt),
            ("v".into(), SUBSONIC_API_VERSION.into()),
            ("c".into(), env!("CARGO_PKG_NAME").into()),
            ("f".into(), "json".into()),
        ]
    }

    async fn subsonic_get(&self, endpoint: &str) -> Result<SubsonicResponse, Box<dyn std::error::Error>> {
        let resp = self.client
            .get(format!("{}/rest/{}", self.url, endpoint))
            .query(&self.subsonic_params())
            .send().await?;
        if !resp.status().is_success() {
            return Err(format!("Subsonic {} returned {}", endpoint, resp.status()).into());
        }
        let env: SubsonicEnvelope = resp.json().await?;
        if env.response.status != "ok" {
            let msg = env.response.error.and_then(|e| e.message)
                .unwrap_or_else(|| "unknown Subsonic error".into());
            return Err(format!("Subsonic {endpoint}: {msg}").into());
        }
        Ok(env.response)
    }

    async fn subsonic_ping(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.subsonic_get("ping").await.map(|_| ())
    }

    /// Cover art bytes for one library item.
    ///
    /// Streamed through Tapedeck rather than linked directly: the browser can't
    /// reach a LAN Navidrome, and it must never be handed the credentials that
    /// would let it. Returns the bytes plus the content type so the proxy can
    /// pass both through.
    pub async fn fetch_cover_art(&self, item_id: &str)
        -> Result<(Vec<u8>, String), Box<dyn std::error::Error + Send + Sync>>
    {
        let mut params = self.subsonic_params();
        params.push(("id".into(), item_id.to_string()));
        params.push(("size".into(), "500".into()));

        let resp = self.client
            .get(format!("{}/rest/getCoverArt", self.url))
            .query(&params)
            .send().await?;

        if !resp.status().is_success() {
            return Err(format!("Navidrome cover art returned {}", resp.status()).into());
        }
        let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_string();

        // Subsonic reports "no artwork" as a JSON error body with HTTP 200, so a
        // non-image response means there's nothing to show.
        if content_type.contains("json") || content_type.contains("xml") {
            return Err("no cover art for that item".into());
        }
        Ok((resp.bytes().await?.to_vec(), content_type))
    }

    /// What's playing right now, per account.
    ///
    /// Navidrome keeps reporting a finished track for a while, so entries that
    /// haven't been touched in a few minutes are dropped — otherwise the deck
    /// would show a stale song long after playback stopped.
    pub async fn fetch_now_playing(&self)
        -> Result<Vec<NavidromeNowPlaying>, Box<dyn std::error::Error>>
    {
        let resp = self.subsonic_get("getNowPlaying").await?;
        let entries = resp.now_playing.map(|n| n.entry).unwrap_or_default();

        Ok(entries.into_iter()
            .filter(|e| e.minutes_ago.unwrap_or(0) < 5)
            .map(|e| {
                let minutes_ago = e.minutes_ago.unwrap_or(0);
                NavidromeNowPlaying {
                    play: Play {
                        source_id: format!("navidrome-{}-np", e.id),
                        source_name: "navidrome".into(),
                        title: e.title,
                        artist: e.artist,
                        album: e.album,
                        timestamp: now_secs(),
                        duration: e.duration,
                        track_number: e.track,
                        mbid_recording: None, mbid_release: None, mbid_artist: None,
                        artists: None, mbid_release_group: None,
                        caa_id: None, caa_release_mbid: None,
                    },
                    username: e.username,
                    minutes_ago,
                }
            })
            .collect())
    }
}

impl NavidromeSong {
    fn to_play(&self, submission_time: u64, media_file_id: &str) -> Play {
        Play {
            // Keyed by the Navidrome id *and* the submission time: the same track
            // played twice is two listens, and re-polling the same window must
            // not create a third.
            source_id: format!("navidrome-{}-{}", media_file_id, submission_time),
            source_name: "navidrome".into(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            timestamp: submission_time,
            duration: self.duration,
            track_number: self.track_number,
            mbid_recording: self.mbid.clone(),
            mbid_release: None, mbid_artist: None,
            artists: None, mbid_release_group: None,
            caa_id: None, caa_release_mbid: None,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

#[async_trait]
impl MusicSource for NavidromeSource {
    fn name(&self) -> &str {
        "Navidrome"
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    async fn fetch_new_plays(&mut self, last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>> {
        let from = self.last_submission.max(last_checked);
        match self.fetch_scrobbles(from).await {
            Ok(plays) => Ok(plays),
            Err(e) => {
                warn!("Navidrome scrobble fetch failed: {}", e);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> NavidromeSong {
        NavidromeSong {
            title: "Xtal".into(),
            artist: "Aphex Twin".into(),
            album: Some("Selected Ambient Works".into()),
            duration: Some(293),
            track_number: Some(1),
            mbid: Some("mbid-xtal".into()),
        }
    }

    #[test]
    fn source_id_separates_repeat_plays_of_one_track() {
        let s = song();
        let first = s.to_play(1700000000, "abc");
        let second = s.to_play(1700000400, "abc");
        // Same track, two listens — they must not dedup into one.
        assert_ne!(first.source_id, second.source_id);
        // …but re-polling the same window yields the identical id, so the
        // existing (user, source_id, source_name) uniqueness absorbs it.
        assert_eq!(first.source_id, s.to_play(1700000000, "abc").source_id);
    }

    #[test]
    fn carries_navidrome_metadata_onto_the_play() {
        let p = song().to_play(1700000000, "abc");
        assert_eq!(p.timestamp, 1700000000);
        assert_eq!(p.duration, Some(293));
        assert_eq!(p.track_number, Some(1));
        // A library MBID means this listen never needs a MusicBrainz lookup.
        assert_eq!(p.mbid_recording.as_deref(), Some("mbid-xtal"));
        assert_eq!(p.source_name, "navidrome");
    }

    #[test]
    fn subsonic_params_are_salted_per_call() {
        let src = NavidromeSource::new("http://nd".into(), "u".into(), "pw".into());
        let a = src.subsonic_params();
        let b = src.subsonic_params();
        let salt = |p: &Vec<(String, String)>| p.iter().find(|(k, _)| k == "s").unwrap().1.clone();
        let tok = |p: &Vec<(String, String)>| p.iter().find(|(k, _)| k == "t").unwrap().1.clone();
        // A fixed salt would make the token a replayable password equivalent.
        assert_ne!(salt(&a), salt(&b));
        assert_ne!(tok(&a), tok(&b));
        // The password itself is never a query parameter.
        assert!(a.iter().all(|(_, v)| v != "pw"));
    }

    #[test]
    fn accepts_navidrome_mbid_field_spellings() {
        // Each of these has been the field name at some point; a mismatch is
        // invisible at runtime, so pin them all.
        for key in ["mbzRecordingID", "mbzRecordingId", "mbzTrackId", "musicBrainzId"] {
            let json = format!(
                r#"{{"id":"1","title":"T","artist":"A","duration":100.0,"{key}":"mb-123"}}"#
            );
            let song: NativeSong = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{key} failed to parse: {e}"));
            assert_eq!(song.mbz_recording_id.as_deref(), Some("mb-123"), "{key} did not map");
        }
    }

    #[test]
    fn url_trailing_slash_is_normalised() {
        let src = NavidromeSource::new("http://nd:4533/".into(), "u".into(), "p".into());
        assert_eq!(src.url, "http://nd:4533");
    }
}
