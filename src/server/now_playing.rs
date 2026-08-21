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
/// A third state exists because "measured" and "measured *just now*" are not the
/// same claim, and only one of them may overrule our own arithmetic.
///
/// Every **polled** source reports what its player last uploaded on the player's
/// own cadence, so its number is a few seconds behind the music through no
/// fault of anyone's — that lag is what `MAX_PLAYHEAD_LAG_SECS` exists to
/// absorb. A **pushing** client sends the playhead at the moment it was true,
/// so it has no lag to absorb and the guard could only ever suppress a genuine
/// seek. A measurement taken now beats arithmetic; a measurement taken at an
/// unknown moment in the last few seconds does not.
#[derive(Clone, Copy, Debug)]
pub enum Progress {
    /// Seconds in, measured by the player and pushed the instant it was true.
    /// Believed outright, backwards steps included — a client that says the
    /// listener seeked back really is the authority on that.
    Exact(u64),
    /// Seconds in as of whenever this source last heard from its player. True
    /// of every polled source.
    Reported(u64),
    /// No playhead exists for this source. The number is our own arithmetic.
    Unknown,
}

impl Progress {
    fn seconds(self) -> u64 {
        match self {
            Progress::Exact(p) | Progress::Reported(p) => p,
            Progress::Unknown => 0,
        }
    }
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
    /// Whether the playhead was measured at the instant it was sent, and may
    /// therefore overrule our own extrapolation in either direction. Internal
    /// staleness policy rather than anything about the listen, so it stays off
    /// the wire: the deck already distinguishes a measurement from a guess with
    /// `position_known`, and both kinds of measurement draw the same.
    #[serde(skip)]
    pub position_exact: bool,
    /// The listener has paused. Only a pushing client can know this — a poll
    /// that simply stops arriving is indistinguishable from a client that
    /// crashed — so it is false for every polled source.
    ///
    /// Load-bearing rather than decorative: `position_at` counts forward from
    /// `updated_at`, so without this a paused track's position runs away while
    /// `position_known` asserts it is a measurement. That is the "confident and
    /// completely wrong position" this type was introduced to stop, made worse
    /// by now being stated with confidence.
    pub paused: bool,
    pub updated_at: i64,
    /// Which source reported this — also the key the pipeline clears by.
    pub source: String,
    pub listening_context: Option<String>,
    /// The signal chain this listen is attributed to, where the writer already
    /// knew it.
    ///
    /// Filled by the two sources for which a chain is a *deliberate statement*
    /// rather than a fallback: an ingest submission resolves one through the
    /// four-rung ladder, and a side off the shelf runs through the turntable
    /// the pressing remembers. A polled source falls through to the device's
    /// default — the coarsest rung — and does not set this, so the public
    /// payload omits the field rather than naming a chain nobody chose.
    pub chain_name: Option<String>,
    // Quality fields, named to match the frontend's `qualityLabel()` shape.
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bit_depth: Option<i16>,
    pub sample_rate: Option<i32>,
    pub dsd_multiplier: Option<i16>,
    pub dsd_to_pcm_converted: Option<bool>,
    pub delivery_codec: Option<String>,
    pub is_lossless: Option<bool>,
    /// A cover for what is on the deck, resolved lazily and at most once per
    /// track.
    ///
    /// Deliberately not filled by whoever writes the entry: the deck in the SPA
    /// has never drawn one, so paying for a lookup on the hot path would be a
    /// cost with no reader. The public endpoint is the first thing that wants
    /// it, so it resolves on first ask and hands the answer back through
    /// `set_artwork` — one query per track rather than one per request, which
    /// on an unauthenticated route is the difference that matters.
    pub artwork_url: Option<String>,
    /// Whether the lookup above has been *attempted*. A track nobody has a
    /// cover for must not be re-asked on every poll of every widget, so a
    /// resolved `None` is a tombstone — the same discipline as `caa_id = -1`
    /// and `artwork_url = ''` in the backfills.
    #[serde(skip)]
    pub artwork_resolved: bool,
    /// Whether a real listen for this track has been submitted.
    ///
    /// Off the wire — it says nothing about what is playing, only about what we
    /// have already banked. It exists to tell a **skip** from a track that
    /// simply finished: a client that scrobbles at end-of-track sends the
    /// listen and *then* announces the next one, so without this every
    /// completed track would look abandoned the moment its successor arrived.
    #[serde(skip)]
    pub banked: bool,
}

/// A track that was on the deck and was replaced before anything banked it.
///
/// Held rather than written immediately because a submitting client can send
/// the scrobble for a track either side of announcing the next one — both
/// orderings are legal and both happen in the wild. Resolving on a delay is
/// what lets a late scrobble cancel it.
#[derive(Debug, Clone)]
pub struct PendingSkip {
    pub play: Play,
    /// Where the deck had got to. `None` when the client never reported a
    /// playhead, in which case the bound below is wall-clock since it announced
    /// the track — good enough to *decide* with, never good enough to store.
    pub listened_secs: Option<u64>,
    /// The figure the decision is made on, measured or not.
    pub bound_secs: u64,
    pub duration: Option<u64>,
    pub displaced_at: i64,
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
            progress: progress.seconds(),
            position_known: !matches!(progress, Progress::Unknown),
            position_exact: matches!(progress, Progress::Exact(_)),
            paused: false,
            banked: false,
            artwork_url: None,
            artwork_resolved: false,
            updated_at: now_secs(),
            source: source.to_string(),
            listening_context: None,
            chain_name: None,
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

    /// Rebuild a listen from what the deck was told, for writing a skip.
    ///
    /// The timestamp is when the track **started**, not when it was displaced.
    /// `scrobbles.timestamp` is start-of-play everywhere in this application —
    /// history, sessions, the 30-minute session gap, every ranged query — and a
    /// skip filed at its end time would sit a track length late in all of them.
    /// That is the same trap the Spotify export's `ts` carries.
    pub fn to_play(&self) -> Play {
        let started = (self.updated_at - self.progress as i64).max(0);
        // Unique per skip: the start second alone would collide for two tracks
        // announced in the same second, and a constant id would let `exists`
        // drop every skip after the first permanently.
        let source_id = format!("ingest-skip-{}-{}-{}", started, self.artist, self.title);
        let mut play = Play::new(
            self.title.clone(),
            self.artist.clone(),
            started as u64,
            source_id,
            "ingest".into(),
        );
        play.album = self.album.clone();
        play.duration = self.duration;
        play
    }

    /// Where playback would be by `now`, assuming it kept running.
    ///
    /// A paused track is not running, so it stays exactly where it was told to
    /// stop. Counting a pause forward is what made a paused deck drift to the
    /// end of a track nobody was playing.
    fn position_at(&self, now: i64) -> u64 {
        if self.paused {
            return self.progress;
        }
        self.progress + (now - self.updated_at).max(0) as u64
    }

    /// A source that stops reporting has usually stopped playing, but sources
    /// differ: Plex re-reports every poll, while a ListenBrainz-style client
    /// sends `playing_now` once at track start. So expire by *running off the
    /// end of the track* when we know its length, and fall back to a flat
    /// timeout when we don't.
    ///
    /// A paused entry can never run off the end — that is the whole point of
    /// `position_at` holding it still — so it takes the flat timeout too.
    /// Otherwise a client that paused once and then went away would leave the
    /// deck asserting a paused record forever.
    fn is_live(&self, now: i64) -> bool {
        match self.duration {
            Some(d) if d > 0 && !self.paused => self.position_at(now) < d + 10,
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
/// Skips awaiting a verdict, and the deck itself. Two locks rather than one
/// struct because they are taken independently and the deck is read on every
/// browser poll.
#[derive(Clone, Default)]
pub struct NowPlayingRegistry(
    Arc<RwLock<HashMap<i64, NowPlaying>>>,
    Arc<RwLock<Vec<(i64, PendingSkip)>>>,
);

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
        //
        // None of that applies to a client that measured the playhead at the
        // moment it sent it. It has no upload cadence to lag behind, so a
        // backward step from one is the listener seeking and nothing else —
        // holding our own count there would suppress the very thing the client
        // is better placed than us to know. A measurement taken now beats
        // arithmetic; see `Progress`.
        if !changed && !np.position_exact {
            if let Some(prev) = previous {
                let ours = prev.position_at(np.updated_at);
                if np.progress < ours && ours - np.progress <= MAX_PLAYHEAD_LAG_SECS {
                    np.progress = ours;
                }
            }
        }

        // A track leaving the deck with nothing banked for it is a skip —
        // **but only for a pushing source**. A polled source's own state
        // machine sees the item change directly and reports it with a real
        // playhead (`SessionTracker::observe`), so recording it here as well
        // would file every Plex and Jellyfin skip twice.
        if changed && np.source == "ingest" {
            if let Some(prev) = previous {
                if !prev.banked {
                    let bound = prev.position_at(np.updated_at);
                    if let Ok(mut pending) = self.1.write() {
                        pending.push((
                            user_id,
                            PendingSkip {
                                play: prev.to_play(),
                                // Only a reported playhead is worth storing.
                                // Wall-clock arithmetic is a bound, and writing
                                // it into `listened_ms` would state a guess as a
                                // measurement — the exact thing `Progress` was
                                // introduced to stop.
                                listened_secs: prev.position_known.then_some(bound),
                                bound_secs: bound,
                                duration: prev.duration,
                                displaced_at: np.updated_at,
                            },
                        ));
                    }
                }
            }
        }

        map.insert(user_id, np);
        changed
    }

    /// Note that a real listen for what is on the deck has been submitted.
    ///
    /// Called when a listen is stored, so the track it describes is not later
    /// mistaken for one the listener walked out on. Matching is on the same
    /// three fields `set` compares, so it agrees with what counts as a change.
    pub fn mark_banked(&self, user_id: i64, title: &str, artist: &str) {
        if let Ok(mut map) = self.0.write() {
            if let Some(np) = map.get_mut(&user_id) {
                if np.title == title && np.artist == artist {
                    np.banked = true;
                }
            }
        }
    }

    /// Record the cover that was resolved for what is on the deck.
    ///
    /// `None` is a real answer and is stored as one — see `artwork_resolved`.
    /// Guarded on the title and artist still matching for the same reason
    /// `mark_banked` is: a lookup started for one track must not land on its
    /// successor if the listener moved on while it was in flight.
    pub fn set_artwork(&self, user_id: i64, title: &str, artist: &str, url: Option<String>) {
        if let Ok(mut map) = self.0.write() {
            if let Some(np) = map.get_mut(&user_id) {
                if np.title == title && np.artist == artist {
                    np.artwork_url = url;
                    np.artwork_resolved = true;
                }
            }
        }
    }

    /// Withdraw a pending skip because its listen turned up after all.
    ///
    /// The other half of `mark_banked`, and it is needed because the two
    /// orderings are both legal: a client may announce the next track before
    /// sending the scrobble for the last one, in which case the displacement
    /// has already been recorded by the time the listen arrives.
    pub fn cancel_pending_skip(&self, user_id: i64, title: &str, artist: &str) {
        if let Ok(mut pending) = self.1.write() {
            pending.retain(|(uid, p)| {
                !(*uid == user_id && p.play.title == title && p.play.artist == artist)
            });
        }
    }

    /// Take the skips that have waited out the grace period.
    ///
    /// The delay is the whole mechanism: it is how long a late scrobble has to
    /// arrive and cancel the verdict. Removed as they are taken, so a caller
    /// that fails to write one loses a skip rather than writing it forever.
    pub fn take_due_skips(&self, now: i64, grace_secs: i64) -> Vec<(i64, PendingSkip)> {
        let Ok(mut pending) = self.1.write() else {
            return Vec::new();
        };
        let (due, waiting) =
            pending.drain(..).partition(|(_, p)| now - p.displaced_at >= grace_secs);
        *pending = waiting;
        due
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
pub async fn now_playing(
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

    /// A track pushed off the deck with nothing banked for it is a skip.
    #[test]
    fn displacing_an_unbanked_ingest_track_queues_a_skip() {
        let reg = NowPlayingRegistry::default();
        let mut first = np("Xtal", "ingest");
        first.progress = 20;
        reg.set(1, first);
        reg.set(1, np("Ageispolis", "ingest"));

        let due = reg.take_due_skips(now_secs() as i64 + 1000, 90);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1.play.title, "Xtal");
        assert_eq!(due[0].1.listened_secs, Some(20));
    }

    /// A track whose listen was submitted is finished, not abandoned.
    ///
    /// A client that scrobbles at end-of-track sends the listen and *then*
    /// announces the next one. Without `mark_banked` every completed track in
    /// everyone's history would be filed as a skip.
    #[test]
    fn a_banked_track_is_not_a_skip_when_the_next_one_starts() {
        let reg = NowPlayingRegistry::default();
        reg.set(1, np("Xtal", "ingest"));
        reg.mark_banked(1, "Xtal", "Aphex Twin");
        reg.set(1, np("Ageispolis", "ingest"));

        assert!(reg.take_due_skips(now_secs() as i64 + 1000, 90).is_empty());
    }

    /// The other ordering: the scrobble arrives *after* the next track was
    /// announced. Both are legal, so both have to be survivable.
    #[test]
    fn a_late_scrobble_withdraws_the_skip() {
        let reg = NowPlayingRegistry::default();
        reg.set(1, np("Xtal", "ingest"));
        reg.set(1, np("Ageispolis", "ingest"));
        // ... and only now does the listen for the first one turn up.
        reg.cancel_pending_skip(1, "Xtal", "Aphex Twin");

        assert!(reg.take_due_skips(now_secs() as i64 + 1000, 90).is_empty());
    }

    /// The grace period is the mechanism, so it has to actually hold them back.
    #[test]
    fn a_skip_waits_out_its_grace_period() {
        let reg = NowPlayingRegistry::default();
        reg.set(1, np("Xtal", "ingest"));
        reg.set(1, np("Ageispolis", "ingest"));

        let now = now_secs() as i64;
        assert!(reg.take_due_skips(now, 90).is_empty(), "not yet — a scrobble may still arrive");
        assert_eq!(reg.take_due_skips(now + 90, 90).len(), 1);
        // Taken means taken: a second pass must not write it again.
        assert!(reg.take_due_skips(now + 1000, 90).is_empty());
    }

    /// A polled source must not produce skips here.
    ///
    /// Plex, Jellyfin, Emby and Roon see the item change in their own session
    /// state machine and report it there with a real playhead. Recording it
    /// again on displacement would file every one of their skips twice.
    #[test]
    fn a_polled_source_does_not_queue_skips_here() {
        let reg = NowPlayingRegistry::default();
        reg.set(1, np("Xtal", "Plex"));
        reg.set(1, np("Ageispolis", "Plex"));

        assert!(reg.take_due_skips(now_secs() as i64 + 1000, 90).is_empty());
    }

    /// A skip is stamped when the track *started*, like every other listen.
    ///
    /// `scrobbles.timestamp` is start-of-play throughout this application, so a
    /// skip filed at its displacement time would sit a track length late in the
    /// history, the session gap and every ranged query.
    #[test]
    fn a_skip_is_timestamped_at_the_start_of_the_track() {
        let mut entry = np("Xtal", "ingest");
        entry.progress = 45;
        let play = entry.to_play();
        assert_eq!(play.timestamp, (entry.updated_at - 45) as u64);
        assert_eq!(play.album.as_deref(), Some("Selected Ambient Works 85-92"));
        assert_eq!(play.duration, Some(300));
    }

    fn np(title: &str, source: &str) -> NowPlaying {
        NowPlaying {
            title: title.into(),
            artist: "Aphex Twin".into(),
            album: Some("Selected Ambient Works 85-92".into()),
            duration: Some(300),
            progress: 0,
            position_known: true,
            // The polled shape, which is what these tests are about — a report
            // that may lag. `position_exact` is set per-test where a pushing
            // client is what is being exercised.
            position_exact: false,
            paused: false,
            banked: false,
            artwork_url: None,
            artwork_resolved: false,
            updated_at: now_secs(),
            source: source.into(),
            listening_context: None,
            chain_name: None,
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

        let measured = NowPlaying::from_play(&play, "Plex", None, Progress::Reported(44));
        assert!(measured.position_known);
        assert_eq!(measured.progress, 44);

        // Both kinds of measurement are a measurement to the deck; the
        // difference between them is only about whose number wins a
        // disagreement, which is `set`'s business and not the browser's.
        let pushed = NowPlaying::from_play(&play, "ingest", None, Progress::Exact(44));
        assert!(pushed.position_known);
        assert!(pushed.position_exact);
        assert!(!measured.position_exact);
    }

    /// The other side of `a_lagging_report_does_not_drag_the_playhead_backwards`,
    /// and the reason `Progress` grew a third state. That guard absorbs the
    /// upload lag a *polled* source cannot help; a client that measured the
    /// playhead at the instant it sent it has no such lag, so the same backward
    /// step is the listener seeking and must be believed. Suppressing it would
    /// have the server overrule the only party that can actually see the player.
    #[test]
    fn a_pushed_playhead_outranks_our_own_arithmetic_in_both_directions() {
        let reg = NowPlayingRegistry::default();

        let mut first = np("Xtal", "ingest");
        first.progress = 100;
        first.position_exact = true;
        reg.set(1, first);

        // Five seconds back — well inside MAX_PLAYHEAD_LAG_SECS, so a polled
        // source would have this held at 100.
        let mut seeked = np("Xtal", "ingest");
        seeked.progress = 95;
        seeked.position_exact = true;
        reg.set(1, seeked);

        let (_, position) = reg.get(1).expect("still playing");
        assert!(position < 100, "a measured seek was suppressed as lag: {position}");
    }

    /// A paused track stays where it stopped. Without this the position counts
    /// forward from `updated_at` regardless, so a paused deck walks to the end
    /// of a record nobody is playing — while `position_known` asserts the whole
    /// way that it is a measurement rather than a guess.
    #[test]
    fn a_paused_deck_does_not_run_away() {
        let reg = NowPlayingRegistry::default();

        let mut paused = np("Xtal", "ingest");
        paused.progress = 90;
        paused.paused = true;
        paused.position_exact = true;
        // Reported a minute ago and not heard from since.
        paused.updated_at = now_secs() - 60;
        reg.set(1, paused);

        let (track, position) = reg.get(1).expect("a paused record is still on the deck");
        assert!(track.paused);
        assert_eq!(position, 90, "a pause counted forward to {position}");
    }

    /// …but not forever. `is_live` expires an entry by running it past the end
    /// of the track, and a paused entry never gets there — so without the flat
    /// timeout a client that paused once and then went away would leave the deck
    /// asserting a paused record indefinitely.
    #[test]
    fn a_pause_nobody_ever_resumed_still_expires() {
        let reg = NowPlayingRegistry::default();

        let mut abandoned = np("Xtal", "ingest");
        abandoned.progress = 30;
        abandoned.paused = true;
        abandoned.updated_at = now_secs() - 601;
        reg.set(1, abandoned);

        assert!(reg.get(1).is_none(), "a pause held the deck past its timeout");
    }
}
