use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::auth::ReadAuth;
use super::AppState;
use crate::models::{AudioQuality, Play};

/// Where the playhead is, and — the half that was missing — whether anything
/// actually knows.
///
/// Plex reports `view_offset`, Jellyfin/Emby `PositionTicks`, Roon
/// `seek_position`, and the shelf ran the clock itself. Those are measurements.
/// A ListenBrainz-style client submits `playing_now` **once at track start and
/// carries no position at all**, and Subsonic's `getNowPlaying` answers
/// `minutesAgo` rather than a playhead — for both of those the only thing
/// available is wall-clock since we were told, which is right until the listener
/// pauses, seeks, or was already halfway through when we first heard about it.
///
/// Both used to arrive as a bare `0` and be presented as a readout, so a paused
/// fooyin drew a confident and completely wrong position. The registry still
/// counts an unknown position forward — that is what expires the entry — but it
/// says so, and the deck draws it as an estimate rather than a measurement.
#[derive(Clone, Copy, Debug)]
pub enum Progress {
    /// Seconds in, from something that can see the playhead.
    Known(u64),
    /// No playhead exists for this source. The number is our own arithmetic.
    Unknown,
}

/// Nothing here is persisted: "now playing" is live state, so a restart
/// correctly shows an empty deck until a source reports again.
#[derive(Clone, Debug, Serialize)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Track length in seconds, when the source reports one.
    pub duration: Option<u64>,
    /// Playback position in seconds **as of `updated_at`**. The client
    /// extrapolates from there between polls rather than us ticking every second.
    pub progress: u64,
    /// False when `progress` is wall-clock arithmetic rather than a reported
    /// playhead — see `Progress`. The deck reads this to decide whether it is
    /// showing a position or a guess.
    pub position_known: bool,
    pub updated_at: i64,
    /// Which source reported this — also the key the pipeline clears by.
    pub source: String,
    pub listening_context: Option<String>,
    // Quality fields, named to match the frontend's `qualityLabel()` shape.
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bit_depth: Option<i16>,
    pub sample_rate: Option<i32>,
    pub dsd_multiplier: Option<i16>,
    pub dsd_to_pcm_converted: Option<bool>,
    pub delivery_codec: Option<String>,
    pub is_lossless: Option<bool>,
}

impl NowPlaying {
    pub fn from_play(
        play: &Play,
        source: &str,
        quality: Option<&AudioQuality>,
        progress: Progress,
    ) -> Self {
        Self {
            title: play.title.clone(),
            artist: play.artist.clone(),
            album: play.album.clone(),
            duration: play.duration,
            progress: match progress {
                Progress::Known(p) => p,
                Progress::Unknown => 0,
            },
            position_known: matches!(progress, Progress::Known(_)),
            updated_at: now_secs(),
            source: source.to_string(),
            listening_context: None,
            format_type: quality.and_then(|q| q.format_type.clone()),
            codec: quality.and_then(|q| q.codec.clone()),
            bit_depth: quality.and_then(|q| q.bit_depth),
            sample_rate: quality.and_then(|q| q.sample_rate),
            dsd_multiplier: quality.and_then(|q| q.dsd_multiplier),
            dsd_to_pcm_converted: quality.and_then(|q| q.dsd_to_pcm_converted),
            delivery_codec: quality.and_then(|q| q.delivery_codec.clone()),
            is_lossless: quality.and_then(|q| q.is_lossless),
        }
    }

    /// Where playback would be by `now`, assuming it kept running.
    fn position_at(&self, now: i64) -> u64 {
        self.progress + (now - self.updated_at).max(0) as u64
    }

    /// A source that stops reporting has usually stopped playing, but sources
    /// differ: Plex re-reports every poll, while a ListenBrainz-style client
    /// sends `playing_now` once at track start. So expire by *running off the
    /// end of the track* when we know its length, and fall back to a flat
    /// timeout when we don't.
    fn is_live(&self, now: i64) -> bool {
        match self.duration {
            Some(d) if d > 0 => self.position_at(now) < d + 10,
            _ => now - self.updated_at < 600,
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

/// How far a freshly reported playhead may sit *behind* our own extrapolation
/// before we believe it rather than treating it as a stale report.
///
/// Sized against Plex, which is the worst of them: the offset in `/status/sessions`
/// is whatever the player last uploaded on its own timeline cadence, so a few
/// seconds of lag is normal and is not the listener doing anything. Deliberately
/// well under the shortest deliberate seek anyone makes — a rewind bigger than
/// this passes straight through.
const MAX_PLAYHEAD_LAG_SECS: u64 = 12;

/// In-memory "what's playing right now", keyed by user. Shared by the HTTP
/// handlers and the poll engine, so it lives behind its own `Arc` rather than
/// inside `AppState` (the engine never sees `AppState`).
#[derive(Clone, Default)]
pub struct NowPlayingRegistry(Arc<RwLock<HashMap<i64, NowPlaying>>>);

impl NowPlayingRegistry {
    /// Record what this user is playing. Returns **true when the track itself
    /// changed** (or nothing was playing), false when this is the same track
    /// moving along.
    ///
    /// Polled sources re-report the same track every few seconds, and the deck
    /// wants every one of those — that's how `progress` stays accurate. What
    /// must *not* happen every few seconds is telling Last.fm and ListenBrainz
    /// about it: their now-playing is a one-shot that expires on its own after
    /// the track's duration, so re-sending is pure noise aimed at someone
    /// else's servers. Callers forward on `true` only.
    pub fn set(&self, user_id: i64, mut np: NowPlaying) -> bool {
        let Ok(mut map) = self.0.write() else {
            return false;
        };
        let previous = map.get(&user_id);
        let changed = previous.is_none_or(|prev| {
            prev.title != np.title || prev.artist != np.artist || prev.album != np.album
        });

        // A reported playhead is not the same thing as the current playhead.
        // Plex's `view_offset` is whatever its client last uploaded, which lags
        // the music by a few seconds and lands unevenly — so every engine tick
        // handed us a position *behind* the one we had already extrapolated to,
        // the deck snapped back, then walked forward again until the next tick.
        // That sawtooth is what "it keeps going backward and forward" is.
        //
        // So a small backward step is treated as the report being stale rather
        // than the music having moved, and we hold our own count. A step bigger
        // than the lag is a real seek or a restart and is believed — which is
        // also what keeps a track put on repeat from being pinned at the end.
        if !changed {
            if let Some(prev) = previous {
                let ours = prev.position_at(np.updated_at);
                if np.progress < ours && ours - np.progress <= MAX_PLAYHEAD_LAG_SECS {
                    np.progress = ours;
                }
            }
        }

        map.insert(user_id, np);
        changed
    }

    /// Drop every entry reported by `source`. The poll engine calls this when a
    /// tick finds no active sessions — that is the authoritative "stopped"
    /// signal for a polled source, so we don't wait for the track to time out.
    pub fn clear_source(&self, source: &str) {
        if let Ok(mut map) = self.0.write() {
            map.retain(|_, np| np.source != source);
        }
    }

    /// Drop this user's entry only if `source` reported it.
    ///
    /// Needed once sources are per-user: an idle Plex must not wipe a
    /// now-playing that the user's phone is currently reporting over ingest.
    pub fn clear_user_source(&self, user_id: i64, source: &str) {
        if let Ok(mut map) = self.0.write() {
            if map.get(&user_id).is_some_and(|np| np.source == source) {
                map.remove(&user_id);
            }
        }
    }

    /// Drop this user's entry, whatever reported it. Backs
    /// `POST /1/playing-now/delete`.
    pub fn clear_user(&self, user_id: i64) {
        if let Ok(mut map) = self.0.write() {
            map.remove(&user_id);
        }
    }

    /// The user's current track, or `None` once it has run its course.
    /// Expired entries are dropped on read so the map can't grow unbounded.
    pub fn get(&self, user_id: i64) -> Option<(NowPlaying, u64)> {
        let now = now_secs();
        let mut map = self.0.write().ok()?;
        map.retain(|_, np| np.is_live(now));
        let np = map.get(&user_id)?;
        let position = match np.duration {
            Some(d) => np.position_at(now).min(d),
            None => np.position_at(now),
        };
        Some((np.clone(), position))
    }
}

/// GET /api/v1/now-playing → `{ playing, track?, position? }`
async fn now_playing(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.now_playing.get(user.user_id) {
        Some((track, position)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "playing": true,
                "position": position,
                "track": track,
            })),
        )
            .into_response(),
        None => (StatusCode::OK, Json(serde_json::json!({ "playing": false }))).into_response(),
    }
}

pub fn create_now_playing_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/now-playing", get(now_playing))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn np(title: &str, source: &str) -> NowPlaying {
        NowPlaying {
            title: title.into(),
            artist: "Aphex Twin".into(),
            album: Some("Selected Ambient Works 85-92".into()),
            duration: Some(300),
            progress: 0,
            position_known: true,
            updated_at: now_secs(),
            source: source.into(),
            listening_context: None,
            format_type: None,
            codec: None,
            bit_depth: None,
            sample_rate: None,
            dsd_multiplier: None,
            dsd_to_pcm_converted: None,
            delivery_codec: None,
            is_lossless: None,
        }
    }

    /// The deck wants every poll; Last.fm and ListenBrainz want one per track.
    /// A polled source calls `set` every few seconds, so if this reported a
    /// change each time we would aim a now-playing request at both services
    /// every tick for the whole length of every track.
    #[test]
    fn only_a_new_track_counts_as_a_change() {
        let reg = NowPlayingRegistry::default();

        assert!(reg.set(1, np("Xtal", "Plex")), "first track is a change");
        assert!(!reg.set(1, np("Xtal", "Plex")), "same track re-reported is not");
        assert!(reg.set(1, np("Tha", "Plex")), "a different track is");

        // Another user's deck is its own thing.
        assert!(reg.set(2, np("Tha", "Plex")), "first track for user 2");

        // Stopping and starting the same track again is a change: the previous
        // now-playing was cleared, so nothing downstream knows about it.
        reg.clear_user_source(1, "Plex");
        assert!(reg.set(1, np("Tha", "Plex")));
    }

    /// Progress must still advance on every call, change or not — that's what
    /// keeps the position bar honest between the browser's own ticks.
    #[test]
    fn position_tracks_the_latest_report_even_without_a_change() {
        let reg = NowPlayingRegistry::default();
        reg.set(1, np("Xtal", "Plex"));

        let mut later = np("Xtal", "Plex");
        later.progress = 120;
        assert!(!reg.set(1, later));

        let (_, position) = reg.get(1).expect("still playing");
        assert!(position >= 120, "expected the newer playhead, got {position}");
    }

    /// The sawtooth. Plex uploads its playhead on its own cadence, so an engine
    /// tick routinely reports a position a few seconds behind where the music
    /// actually is — and the deck, which had already extrapolated past it,
    /// jumped backwards every time. A stale report must not move the playhead
    /// back.
    #[test]
    fn a_lagging_report_does_not_drag_the_playhead_backwards() {
        let reg = NowPlayingRegistry::default();

        let mut first = np("Xtal", "Plex");
        first.progress = 100;
        reg.set(1, first);

        // The same track re-reported a little behind: Plex's client had not
        // uploaded the intervening seconds yet.
        let mut lagging = np("Xtal", "Plex");
        lagging.progress = 95;
        reg.set(1, lagging);

        let (_, position) = reg.get(1).expect("still playing");
        assert!(position >= 100, "playhead went backwards to {position}");
    }

    /// The other half of the same rule, and the reason it is a tolerance rather
    /// than a floor: a real rewind — scrubbing back, or putting the track on
    /// again — has to be believed, or the deck pins itself at the far end of a
    /// track that has started over.
    #[test]
    fn a_real_rewind_is_believed() {
        let reg = NowPlayingRegistry::default();

        let mut first = np("Xtal", "Plex");
        first.progress = 200;
        reg.set(1, first);

        let mut rewound = np("Xtal", "Plex");
        rewound.progress = 3;
        reg.set(1, rewound);

        let (_, position) = reg.get(1).expect("still playing");
        assert!(position < 20, "expected the rewind to land, got {position}");
    }

    /// A source that cannot see a playhead must not be presented as if it can.
    /// fooyin submits `playing_now` once at track start with no position in the
    /// format at all, and Subsonic answers `minutesAgo` — both used to arrive as
    /// a bare `0` and be drawn as a readout, so a paused player showed a
    /// confident and completely wrong position.
    #[test]
    fn a_source_with_no_playhead_says_so() {
        let mut play = Play::new(
            "Giorgio by Moroder".into(),
            "Daft Punk".into(),
            0,
            "np".into(),
            "ingest".into(),
        );
        play.album = Some("Random Access Memories".into());
        play.duration = Some(544);

        let guessed = NowPlaying::from_play(&play, "ingest", None, Progress::Unknown);
        assert!(!guessed.position_known);
        assert_eq!(guessed.progress, 0);

        let measured = NowPlaying::from_play(&play, "Plex", None, Progress::Known(44));
        assert!(measured.position_known);
        assert_eq!(measured.progress, 44);
    }
}
