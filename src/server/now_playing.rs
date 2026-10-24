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
/// own cadence, so its number is behind the music through no fault of anyone's
/// — `NowPlayingRegistry::set` is where that lag is absorbed. A **pushing**
/// client sends the playhead at the moment it was true, so it has no lag to
/// absorb and the guard could only ever suppress a genuine seek. A measurement
/// taken now beats arithmetic; a measurement taken at an unknown moment in the
/// last few seconds does not.
///
/// **Milliseconds**, and struct variants so that no caller can hand over
/// seconds by habit: the hold rule compares reports a few hundred milliseconds
/// apart, and whole seconds were part of what made the deck jump.
#[derive(Clone, Copy, Debug)]
pub enum Progress {
    /// Measured by the player and pushed the instant it was true. Believed
    /// outright, backwards steps included — a client that says the listener
    /// seeked back really is the authority on that.
    Exact { ms: u64 },
    /// As of whenever this source last heard from its player. True of every
    /// polled source.
    Reported { ms: u64 },
    /// No playhead exists for this source. The number is our own arithmetic.
    Unknown,
}

impl Progress {
    fn millis(self) -> u64 {
        match self {
            Progress::Exact { ms } | Progress::Reported { ms } => ms,
            Progress::Unknown => 0,
        }
    }
}

/// What is on the deck. **Not a `Listen`** — nothing here is persisted, so
/// there is no id, no timestamp, no status and no `source_id`. It carries the
/// quality fields a source could report plus the playhead.
// Nothing here is persisted: "now playing" is live state, so a restart
// correctly shows an empty deck until a source reports again.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[schema(as = NowPlayingTrack)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Track length in seconds, when the source reports one.
    pub duration: Option<u64>,
    /// Seconds in **as of `updated_at`**. Extrapolate from there rather than
    /// expecting a tick per second — but see `position_known` first, and stop
    /// extrapolating when `paused`. `progress_ms` is the same, unrounded.
    pub progress: u64,
    /// Milliseconds in as of `updated_at_ms`. The number the registry works in;
    /// `progress` is this in whole seconds.
    pub progress_ms: u64,
    /// **False when `progress` is the server's own wall-clock arithmetic rather
    /// than a reported playhead.** A ListenBrainz `playing_now` has no position
    /// field in the format and Subsonic answers `minutesAgo`, so for those there
    /// is nothing to report and the number is a guess. Draw it as one. Plex,
    /// Jellyfin, Emby, Roon and the shelf report a real playhead, and so does
    /// any client sending `tapedeck_playback`.
    pub position_known: bool,
    // Whether the playhead was measured at the instant it was sent, and may
    // therefore overrule our own extrapolation in either direction. Internal
    // staleness policy rather than anything about the listen, so it stays off
    // the wire: the deck already distinguishes a measurement from a guess with
    // `position_known`, and both kinds of measurement draw the same.
    #[serde(skip)]
    pub position_exact: bool,
    /// Only a pushing client can know this — a poll that stops arriving is
    /// indistinguishable from a client that crashed — so it is false for every
    /// polled source. A paused entry holds its position rather than counting
    /// forward, and expires after ten minutes of silence.
    // Load-bearing rather than decorative: `position_at` counts forward from
    // `updated_at`, so without this a paused track's position runs away while
    // `position_known` asserts it is a measurement. That is the "confident and
    // completely wrong position" this type was introduced to stop, made worse
    // by now being stated with confidence.
    pub paused: bool,
    /// Unix seconds when this was last reported.
    pub updated_at: i64,
    /// Unix milliseconds when this was last reported.
    pub updated_at_ms: i64,
    // The playhead exactly as the source last sent it, before the hold rule
    // touched it. A polled source re-sends the same number until its player
    // uploads a new one — Plexamp does so every ~15.6 s, measured 2026-10-24 —
    // so a report equal to this one is not new information, and comparing it
    // with our count is what dragged the deck backwards.
    #[serde(skip)]
    pub reported_ms: Option<u64>,
    /// `Plex`, `Roon`, `ingest`, `physical`…
    // Also the key the pipeline clears by.
    pub source: String,
    pub listening_context: Option<String>,
    /// The signal chain, **only where it was a deliberate statement** — an
    /// ingest submission's resolved chain, or the turntable a pressing on the
    /// shelf remembers. A polled source falls back to its device's default and
    /// leaves this null rather than naming a chain nobody chose.
    pub chain_name: Option<String>,
    // The chain this listen will be attributed to, resolved the way its
    // eventual scrobble will resolve it.
    //
    // **Internal, and `skip`ped off the wire on purpose.** It is a *gate
    // input* rather than anything about the listen: Patch's live strip has to
    // apply the same shared-chain rule the Reel does, and `chain_name` cannot
    // carry it — a name is not a key, and a polled source sets no name at all
    // because its chain is the device's default rather than a choice.
    #[serde(skip)]
    pub chain_id: Option<i64>,
    // Quality fields, named to match the frontend's `qualityLabel()` shape.
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bit_depth: Option<i16>,
    pub sample_rate: Option<i32>,
    pub dsd_multiplier: Option<i16>,
    pub dsd_to_pcm_converted: Option<bool>,
    pub delivery_codec: Option<String>,
    pub is_lossless: Option<bool>,
    /// A cover for what is on the deck, once something has asked for one — it
    /// is resolved lazily, at most once per track, by the public widget.
    // Deliberately not filled by whoever writes the entry: the deck in the SPA
    // has never drawn one, so paying for a lookup on the hot path would be a
    // cost with no reader. The public endpoint resolves on first ask and hands
    // the answer back through `set_artwork` — one query per track rather than
    // one per request, which on an unauthenticated route is what matters.
    pub artwork_url: Option<String>,
    // Whether the lookup above has been *attempted*. A track nobody has a
    // cover for must not be re-asked on every poll of every widget, so a
    // resolved `None` is a tombstone — the same discipline as `caa_id = -1`
    // and `artwork_url = ''` in the backfills.
    #[serde(skip)]
    pub artwork_resolved: bool,
    // Whether a real listen for this track has been submitted.
    //
    // Off the wire — it says nothing about what is playing, only about what we
    // have already banked. It exists to tell a **skip** from a track that
    // simply finished: a client that scrobbles at end-of-track sends the
    // listen and *then* announces the next one, so without this every
    // completed track would look abandoned the moment its successor arrived.
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
            progress: progress.millis() / 1000,
            progress_ms: progress.millis(),
            reported_ms: (!matches!(progress, Progress::Unknown)).then(|| progress.millis()),
            position_known: !matches!(progress, Progress::Unknown),
            position_exact: matches!(progress, Progress::Exact { .. }),
            paused: false,
            banked: false,
            artwork_url: None,
            artwork_resolved: false,
            updated_at: now_ms() / 1000,
            updated_at_ms: now_ms(),
            source: source.to_string(),
            listening_context: None,
            chain_name: None,
            chain_id: None,
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

    /// Where playback would be by `now` (unix ms), assuming it kept running.
    ///
    /// A paused track is not running, so it stays exactly where it was told to
    /// stop. Counting a pause forward is what made a paused deck drift to the
    /// end of a track nobody was playing.
    fn position_at_ms(&self, now_ms: i64) -> u64 {
        if self.paused {
            return self.progress_ms;
        }
        self.progress_ms + (now_ms - self.updated_at_ms).max(0) as u64
    }

    /// The same in whole seconds, for the callers that count in them.
    fn position_at(&self, now: i64) -> u64 {
        self.position_at_ms(now * 1000) / 1000
    }

    /// The playhead right now in milliseconds, held at the track's end — what
    /// the browser's smooth clock anchors to.
    pub fn position_ms_now(&self) -> u64 {
        let at = self.position_at_ms(now_ms());
        match self.duration {
            Some(d) => at.min(d * 1000),
            None => at,
        }
    }

    /// Move the playhead, keeping the whole-second fields on the wire in step.
    fn set_progress_ms(&mut self, ms: u64) {
        self.progress_ms = ms;
        self.progress = ms / 1000;
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

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

/// Room for a changed report to be older than the gap since the previous poll:
/// the request's own time, and a player whose upload landed just before we
/// asked last time but after Plex answered.
const FRESH_REPORT_SLACK_MS: i64 = 1_500;

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
        // Measured against Plexamp on 2026-10-24: its `viewOffset` moves on
        // every ~15.6 s and Plex hands back the same number in between, so a
        // 5 s poll reads a figure anywhere from 0 to ~15 s old. The old rule
        // held any report up to 12 s behind our count and believed the rest as
        // a seek, so whenever a poll landed just before Plexamp's next upload
        // the deck jumped back ~13 s and forward again at the next one. That
        // is "it keeps going backward and forward".
        //
        // The rule now, for a polled source on the same track:
        //
        // * **An unchanged report is no information.** Our own count carries
        //   on and the report is not compared with it at all.
        // * **A changed report was sent since the previous poll**, so it is at
        //   most that gap old (plus slack). Behind our count by more than that
        //   is the listener seeking back, and is believed. Behind by less is
        //   the report's own age, and our count holds.
        // * **Ahead of our count** is a fresher measurement than the one we
        //   were extrapolating from, and moves the clock forward.
        //
        // None of that applies to a client that measured the playhead at the
        // moment it sent it. It has no upload cadence to lag behind, so a
        // backward step from one is the listener seeking and nothing else.
        // A measurement taken now beats arithmetic; see `Progress`.
        let reported = np.progress_ms;
        let ours = previous.filter(|_| !changed).map(|p| p.position_at_ms(np.updated_at_ms));
        let verdict = match (previous.filter(|_| !changed), ours) {
            (Some(prev), Some(ours)) if !np.position_exact && np.position_known => {
                let window = (np.updated_at_ms - prev.updated_at_ms).max(0) + FRESH_REPORT_SLACK_MS;
                if prev.reported_ms == Some(reported) {
                    np.set_progress_ms(ours);
                    "unchanged, ours carries on"
                } else if reported >= ours {
                    "ahead of ours, moved forward"
                } else if (ours - reported) as i64 <= window {
                    np.set_progress_ms(ours);
                    "behind by its own age, held"
                } else {
                    "behind by more than its age, believed (seek)"
                }
            }
            (Some(_), Some(_)) if np.position_exact => "exact, believed",
            (Some(_), Some(_)) => "no playhead, ours",
            _ => "new track",
        };
        // The `playhead` trace's other half: what the deck made of the report.
        if tracing::enabled!(target: "playhead", tracing::Level::DEBUG) {
            tracing::debug!(
                target: "playhead",
                "deck {} reported={}ms ours={} → {}ms ({verdict})",
                np.source,
                reported,
                ours.map_or("-".to_string(), |o| format!("{o}ms")),
                np.progress_ms
            );
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

/// Never persisted. Held in memory only, so a restart correctly shows an empty
/// deck until a source reports again.
#[derive(Serialize, utoipa::ToSchema)]
#[schema(as = NowPlaying)]
pub struct NowPlayingResponse {
    pub playing: bool,
    /// Seconds into the track, extrapolated to now. Absent when nothing is
    /// playing. Read `track.position_known` before presenting this as a
    /// readout — for a source with no playhead it is the server's own
    /// wall-clock arithmetic, not a measurement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u64>,
    /// The same, in milliseconds. Anchor a running clock to this rather than
    /// ticking whole seconds: whole seconds and a 1 s timer were half of why
    /// the deck jumped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_ms: Option<u64>,
    /// Absent when nothing is playing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<NowPlaying>,
}

/// What is on the deck.
///
/// Scope: `read`. In-memory only. Entries expire on read — past `duration`
/// + 10s, or after 10 minutes when the source reported no duration.
#[utoipa::path(
    get,
    path = "/api/v1/now-playing",
    tag = "Listens",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK — `playing: false` when the deck is empty.",
            body = NowPlayingResponse),
        (status = 401, response = crate::openapi::Unauthorized),
        (status = 403, response = crate::openapi::Forbidden),
    )
)]
pub async fn now_playing(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.now_playing.get(user.user_id) {
        Some((track, position)) => (
            StatusCode::OK,
            Json(NowPlayingResponse {
                playing: true,
                position: Some(position),
                position_ms: Some(track.position_ms_now()),
                track: Some(track),
            }),
        )
            .into_response(),
        None => (
            StatusCode::OK,
            Json(NowPlayingResponse {
                playing: false,
                position: None,
                position_ms: None,
                track: None,
            }),
        )
            .into_response(),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(now_playing))]
pub struct Api;

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
        first.set_progress_ms(20_000);
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
        entry.set_progress_ms(45_000);
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
            progress_ms: 0,
            reported_ms: None,
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
            updated_at_ms: now_ms(),
            source: source.into(),
            listening_context: None,
            chain_name: None,
            chain_id: None,
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
        later.set_progress_ms(120_000);
        assert!(!reg.set(1, later));

        let (_, position) = reg.get(1).expect("still playing");
        assert!(position >= 120, "expected the newer playhead, got {position}");
    }

    /// A polled report as it arrives: `ms` exactly as the source sent it, at
    /// `at` (unix ms).
    fn report(ms: u64, at: i64) -> NowPlaying {
        let mut n = np("Xtal", "Plex");
        n.set_progress_ms(ms);
        n.reported_ms = Some(ms);
        n.updated_at_ms = at;
        n.updated_at = at / 1000;
        n
    }

    fn deck_ms(reg: &NowPlayingRegistry) -> u64 {
        reg.0.read().unwrap().get(&1).unwrap().progress_ms
    }

    /// The sawtooth, as measured against Plexamp on 2026-10-24: its playhead
    /// moves on every ~15.6 s and Plex hands back the same number in between,
    /// so a 5 s poll reads it 0, 5, 10 and sometimes 15 s old. The old rule
    /// held up to 12 s and believed the rest as a seek, so the 15 s case threw
    /// the deck back. A repeated number is no information and is not compared.
    #[test]
    fn an_unchanged_report_is_not_information() {
        let reg = NowPlayingRegistry::default();
        let t = now_ms() - 60_000;
        reg.set(1, report(108_970, t));
        for (n, gap) in [(1, 5_000), (2, 10_000), (3, 15_055)] {
            reg.set(1, report(108_970, t + gap));
            assert_eq!(
                deck_ms(&reg),
                108_970 + gap as u64,
                "repeat {n}, {gap} ms on: the deck should have carried on counting"
            );
        }
        // And the whole seconds on the wire stay in step with it.
        let held = reg.0.read().unwrap().get(&1).unwrap().clone();
        assert_eq!(held.progress, held.progress_ms / 1000);
    }

    /// A changed report was sent some time since the previous poll, so it can
    /// be up to that gap old. Behind our count by no more than that is its own
    /// age, and our count holds.
    #[test]
    fn a_fresh_report_behind_by_its_own_age_is_held() {
        let reg = NowPlayingRegistry::default();
        let t = now_ms() - 60_000;
        reg.set(1, report(46_750, t));
        reg.set(1, report(46_750, t + 5_000));
        // Plexamp's next upload happened ~3 s before this poll.
        reg.set(1, report(58_750, t + 15_000));
        assert_eq!(deck_ms(&reg), 61_750, "a report 3 s old dragged the deck back");
    }

    /// Ahead of our count is a fresher measurement than the one we were
    /// counting from — the first number for a track may already have been
    /// stale — and moves the clock forward.
    #[test]
    fn a_fresh_report_ahead_moves_the_clock_forward() {
        let reg = NowPlayingRegistry::default();
        let t = now_ms() - 60_000;
        reg.set(1, report(130, t));
        reg.set(1, report(15_730, t + 5_000));
        assert_eq!(deck_ms(&reg), 15_730);
    }

    /// The other half of the rule, and the reason it is a window rather than a
    /// floor: a real rewind — scrubbing back, or putting the track on again —
    /// is further behind than any report can be old, and is believed.
    #[test]
    fn a_real_rewind_is_believed() {
        let reg = NowPlayingRegistry::default();
        let t = now_ms() - 60_000;
        reg.set(1, report(200_000, t));
        reg.set(1, report(3_000, t + 5_000));
        assert_eq!(deck_ms(&reg), 3_000);

        // A short one too: 10 s back is past a 5 s poll gap plus slack.
        reg.set(1, report(90_000, t + 10_000));
        reg.set(1, report(85_000, t + 15_000));
        assert_eq!(deck_ms(&reg), 85_000);
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

        let measured =
            NowPlaying::from_play(&play, "Plex", None, Progress::Reported { ms: 44_000 });
        assert!(measured.position_known);
        assert_eq!(measured.progress, 44);

        // Both kinds of measurement are a measurement to the deck; the
        // difference between them is only about whose number wins a
        // disagreement, which is `set`'s business and not the browser's.
        let pushed = NowPlaying::from_play(&play, "ingest", None, Progress::Exact { ms: 44_000 });
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
        first.set_progress_ms(100_000);
        first.position_exact = true;
        reg.set(1, first);

        // Five seconds back, reported at once — a changed polled report would
        // be judged against the poll gap; a pushed one is simply believed.
        let mut seeked = np("Xtal", "ingest");
        seeked.set_progress_ms(95_000);
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
        paused.set_progress_ms(90_000);
        paused.paused = true;
        paused.position_exact = true;
        // Reported a minute ago and not heard from since.
        paused.updated_at = now_secs() - 60;
        paused.updated_at_ms = now_ms() - 60_000;
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
        abandoned.set_progress_ms(30_000);
        abandoned.paused = true;
        abandoned.updated_at = now_secs() - 601;
        abandoned.updated_at_ms = now_ms() - 601_000;
        reg.set(1, abandoned);

        assert!(reg.get(1).is_none(), "a pause held the deck past its timeout");
    }
}
