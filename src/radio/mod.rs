//! The radio tuner: tune in to a station, and log what it plays.
//!
//! **Where the track names come from.** Today, the station's own internet
//! stream: the tuner opens it, discards the audio and reads the ICY
//! `StreamTitle` lines (see [`icy`]). That works with no hardware, for any
//! station whose SI.xml lists a stream, which is every ARD station. It is a
//! stand-in for what the listener actually hears — the stream can run a few
//! seconds either side of the broadcast. An off-air feeder (RT+ over FM, DL+
//! over DAB, from an SDR on the Pi) is the later, exact source; the air log
//! already records which source each line came from so the two can coexist.
//!
//! **How Tapedeck knows you are listening.** It does not guess: you press
//! Tune in, and it logs until you press Off, tune elsewhere, or the auto-stop
//! you chose runs out. A tuner lives in memory, like now-playing — a restart
//! ends it, and `close_dangling_radio` tidies the rows it left open.
//!
//! **What becomes a listen.** A line shaped like `Artist - Title` that was
//! heard long enough (see [`heard_enough`]). Then the rules decide where it
//! goes: one MusicBrainz recognises is *tagged* and is written and forwarded
//! like any other listen; one it does not is a *guess*, and with "Forward
//! tagged tracks only" on it waits for a yes in the review queue rather than
//! reaching somebody's public Last.fm record on the strength of a station's
//! free text. Breaks and promo lines are logged, never listened.

pub mod icy;
pub mod machine;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::db::{NewAirLine, RadioPreset};
use crate::engine::enrichment::enrich_play;
use crate::models::{ListenDisposition, Play, ScrobbleThreshold};
use crate::radiodns::directory::{Directory, DirectoryStation};
use crate::radiodns::{self, spi, Bearer, ScheduleStatus};
use crate::server::now_playing::{NowPlaying, Progress};
use crate::server::AppState;
use crate::sources::egress::Egress;
use icy::{classify, stream_title, IcyDemux};
use machine::{BreakKind, Effect, Machine, Track};

/// The now-playing registry's source key for the tuner.
pub const RADIO_SOURCE: &str = "Radio";

/// Where a line came from, as the air log names it.
const ICY: &str = "ICY";

/// Below this, nothing is a listen — Last.fm's own floor.
const MIN_HEARD_SECS: i64 = 30;

/// No bytes for this long and the stream is treated as dropped.
const STALL: Duration = Duration::from_secs(20);

/// The listener's rules, stored in their preferences under `radio`. Every
/// field defaults the way the design draws it: all on, a three-hour stop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct RadioSettings {
    /// Skip ads, talk & jingles: such lines are logged as skipped. Off, they
    /// are logged as logged. Neither ever becomes a listen.
    pub skip_breaks: bool,
    /// Forward tagged tracks only: a guess waits rather than being written.
    pub tagged_only: bool,
    /// Guesses wait in the review queue. Off, they are held in the log.
    pub review: bool,
    /// Save radio listens with `listening_context = radio`.
    pub tag_radio: bool,
    /// End the session by itself after `auto_stop_hours`.
    pub auto_stop: bool,
    /// 1–12.
    pub auto_stop_hours: u8,
    /// File radio listens under `chain_id`.
    pub attach_chain: bool,
    pub chain_id: Option<i64>,
}

impl Default for RadioSettings {
    fn default() -> Self {
        RadioSettings {
            skip_breaks: true,
            tagged_only: true,
            review: true,
            tag_radio: true,
            auto_stop: true,
            auto_stop_hours: 3,
            attach_chain: true,
            chain_id: None,
        }
    }
}

impl RadioSettings {
    pub fn from_prefs(prefs: &serde_json::Value) -> Self {
        let mut s: RadioSettings = prefs
            .get("radio")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        s.auto_stop_hours = s.auto_stop_hours.clamp(1, 12);
        s
    }

    fn ends_at(&self, since: i64) -> Option<i64> {
        self.auto_stop.then(|| since + i64::from(self.auto_stop_hours) * 3600)
    }
}

/// Was enough of a track heard for it to count?
///
/// ICY gives no duration, so the user's fraction cannot apply. What can be
/// known is whether the track *started* while tuned in: a title that arrived
/// after another one, and ended by being replaced, was heard from its first
/// bar to its last — a whole play, whatever its length past the 30-second
/// floor. Anything caught part-way (already on at tune-in, or cut off by
/// Off) has to clear the user's flat threshold instead, the same rule every
/// durationless source in Tapedeck uses.
pub fn heard_enough(
    started_at: i64,
    ended_at: i64,
    seen_start: bool,
    natural_end: bool,
    threshold: &ScrobbleThreshold,
) -> bool {
    let heard = ended_at - started_at;
    heard >= MIN_HEARD_SECS && ((seen_start && natural_end) || heard >= threshold.after_secs as i64)
}

/// What the faceplate shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TunerState {
    /// Connecting, or connected and nothing said yet.
    Tuning,
    /// A track is on.
    Live,
    /// The station is saying something that is not a track.
    Unknown,
    /// News, traffic, weather or ads.
    Break,
    /// The stream went away; retrying.
    Dropped,
}

/// What the async half attaches to a track once it has logged it and asked
/// MusicBrainz about it.
#[derive(Debug, Clone)]
struct Heard {
    log_id: i64,
    play: Play,
    tagged: bool,
}

#[derive(Debug, Clone)]
struct Tuned {
    gen: u64,
    session_id: i64,
    preset: RadioPreset,
    station: DirectoryStation,
    settings: RadioSettings,
    /// The chain this session files listens under, already resolved against
    /// `attach_chain`.
    chain_id: Option<i64>,
    chain_name: Option<String>,
    since: i64,
    ends_at: Option<i64>,
    /// What is on air, and every rule about it. See [`machine`].
    machine: Machine<Heard>,
    /// The connection's own state when it overrides the broadcast's:
    /// `Tuning` while (re)connecting, `Dropped` while waiting to retry.
    transport: Option<TunerState>,
    retry_at: Option<i64>,
    egress: Egress,
}

/// What `GET /api/v1/radio/tuner` returns.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TunerStatus {
    pub tuned: bool,
    pub preset: Option<RadioPreset>,
    pub session_id: Option<i64>,
    /// Unix seconds.
    pub since: Option<i64>,
    /// When auto-stop will end it, if it will.
    pub ends_at: Option<i64>,
    pub state: Option<TunerState>,
    pub now: Option<OnAirNow>,
    /// The last thing the station said, verbatim.
    pub line: Option<String>,
    /// Where `line` came from: `ICY` today.
    pub source: Option<String>,
    /// When a dropped stream is next retried.
    pub retry_at: Option<i64>,
    /// 0–5, the faceplate's bars: connection health, not RF signal.
    pub signal: u8,
    pub chain_id: Option<i64>,
    pub chain_name: Option<String>,
    pub logo: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct OnAirNow {
    pub artist: String,
    pub title: String,
    pub since: i64,
    /// MusicBrainz recognised it.
    pub tagged: bool,
}

/// What `GET /api/v1/radio/schedule` returns.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ScheduleOut {
    pub station_id: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// `RadioDNS` — the only source today.
    pub source: String,
    pub status: ScheduleStatus,
    pub programmes: Vec<spi::Programme>,
}

/// Shared radio state: the station directory, every live tuner, and a short
/// cache of schedules.
#[derive(Default)]
pub struct Radio {
    pub directory: Directory,
    tuners: Mutex<HashMap<i64, Tuned>>,
    tasks: Mutex<HashMap<i64, tokio::task::AbortHandle>>,
    schedules: tokio::sync::Mutex<HashMap<String, (Instant, ScheduleOut)>>,
    next_gen: AtomicU64,
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn submission_client(band: &str) -> &'static str {
    if band == "dab" {
        "DAB+ radio"
    } else {
        "FM radio"
    }
}

impl Radio {
    fn with<R>(&self, user_id: i64, gen: u64, f: impl FnOnce(&mut Tuned) -> R) -> Option<R> {
        let mut map = self.tuners.lock().ok()?;
        map.get_mut(&user_id).filter(|t| t.gen == gen).map(f)
    }

    pub fn status(&self, user_id: i64) -> TunerStatus {
        let t = self.tuners.lock().ok().and_then(|m| m.get(&user_id).cloned());
        let Some(t) = t else {
            return TunerStatus {
                tuned: false,
                preset: None,
                session_id: None,
                since: None,
                ends_at: None,
                state: None,
                now: None,
                line: None,
                source: None,
                retry_at: None,
                signal: 0,
                chain_id: None,
                chain_name: None,
                logo: None,
            };
        };
        let state = t.transport.or(t.machine.state).unwrap_or(TunerState::Tuning);
        let signal = match state {
            TunerState::Dropped => 0,
            TunerState::Tuning => 1,
            _ => 5,
        };
        TunerStatus {
            tuned: true,
            now: t.machine.current.as_ref().map(|c| OnAirNow {
                artist: c.artist.clone(),
                title: c.title.clone(),
                since: c.started_at,
                tagged: c.payload.as_ref().is_some_and(|h| h.tagged),
            }),
            source: t.machine.line.as_ref().map(|_| ICY.to_string()),
            preset: Some(t.preset),
            session_id: Some(t.session_id),
            since: Some(t.since),
            ends_at: t.ends_at,
            state: Some(state),
            line: t.machine.line,
            retry_at: t.retry_at,
            signal,
            chain_id: t.chain_id,
            chain_name: t.chain_name,
            logo: t.station.logo,
        }
    }

    /// New settings take effect on a running tuner: the auto-stop moves, and
    /// the next line is judged by the new rules. The chain a session files
    /// under does not change mid-session.
    pub fn apply_settings(&self, user_id: i64, settings: &RadioSettings) {
        if let Ok(mut map) = self.tuners.lock() {
            if let Some(t) = map.get_mut(&user_id) {
                t.ends_at = settings.ends_at(t.since);
                t.settings = settings.clone();
            }
        }
    }

    /// Today's schedule for a preset, cached for half an hour.
    pub async fn schedule(
        &self,
        preset: &RadioPreset,
        date: chrono::NaiveDate,
    ) -> Option<ScheduleOut> {
        let station = self.directory.find(&preset.station_id).await?;
        let key = format!(
            "{}|{}|{}|{}",
            preset.station_id,
            preset.band,
            preset.dial.as_deref().unwrap_or(""),
            date
        );
        {
            let cache = self.schedules.lock().await;
            if let Some((at, out)) = cache.get(&key) {
                if at.elapsed() < Duration::from_secs(1800) {
                    return Some(out.clone());
                }
            }
        }
        // The band the preset is on, when its bearer can be named; otherwise
        // the other one — it is one service, and one schedule.
        let fm =
            station.fm.as_deref().and_then(|f| Bearer::from_published(f, preset.dial.as_deref()));
        let dab = station.dab.first().and_then(|d| Bearer::from_published(d, None));
        let bearer = if preset.band == "fm" { fm.or(dab) } else { dab.or(fm) }?;
        let egress = Egress::Member;
        let (status, programmes) = match radiodns::spi_server(&station.broadcaster).await {
            Ok(base) => {
                radiodns::schedule(&radiodns::client(egress), egress, &base, &bearer, date).await
            }
            Err(e) => {
                debug!("📻 Schedule for {}: {e}", station.name);
                (ScheduleStatus::Unavailable, Vec::new())
            }
        };
        let out = ScheduleOut {
            station_id: station.id,
            date: date.format("%Y-%m-%d").to_string(),
            source: "RadioDNS".into(),
            status,
            programmes,
        };
        let mut cache = self.schedules.lock().await;
        cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(1800));
        cache.insert(key, (Instant::now(), out.clone()));
        Some(out)
    }
}

/// Why tuning in was refused. The text is what the listener sees.
#[derive(Debug)]
pub struct TuneError(pub u16, pub String);

/// Tune `user_id` in to a preset. Replaces whatever they were tuned to.
pub async fn tune_in(
    state: &Arc<AppState>,
    user_id: i64,
    is_admin: bool,
    preset: RadioPreset,
) -> Result<TunerStatus, TuneError> {
    let radio = &state.radio;
    let station = radio.directory.find(&preset.station_id).await.ok_or_else(|| {
        TuneError(
            409,
            "That station is not in the station list right now — try again in a minute".into(),
        )
    })?;
    if station.stream.is_none() {
        return Err(TuneError(
            409,
            format!(
                "{} publishes no internet stream, so there is nothing to read track names from",
                station.name
            ),
        ));
    }

    stop(state, user_id, "retuned").await;

    let prefs = state.db.get_preferences(user_id).await.unwrap_or_default();
    let settings = RadioSettings::from_prefs(&prefs);
    let mut chain_id = None;
    let mut chain_name = None;
    if settings.attach_chain {
        if let Some(cid) = settings.chain_id {
            // Owned by this user, or nothing: a chain id from preferences is
            // still only a number somebody stored.
            if let Ok(Some(chain)) = state.db.get_chain(user_id, cid).await {
                chain_id = chain.id;
                chain_name = Some(chain.name);
            }
        }
    }
    let since = now_secs();
    let session_id = state
        .db
        .start_radio_session(user_id, &preset, chain_id, since)
        .await
        .map_err(|e| TuneError(500, format!("Could not start the session: {e}")))?;

    let gen = radio.next_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let tuned = Tuned {
        gen,
        session_id,
        ends_at: settings.ends_at(since),
        preset,
        station,
        settings,
        chain_id,
        chain_name,
        since,
        machine: Machine::default(),
        transport: Some(TunerState::Tuning),
        retry_at: None,
        egress: Egress::for_owner(is_admin),
    };
    info!(
        "📻 Tuned in: user {user_id} → {} ({})",
        tuned.station.name,
        tuned.preset.band.to_uppercase()
    );
    if let Ok(mut map) = radio.tuners.lock() {
        map.insert(user_id, tuned);
    }
    let task = tokio::spawn(run(state.clone(), user_id, gen));
    if let Ok(mut tasks) = radio.tasks.lock() {
        tasks.insert(user_id, task.abort_handle());
    }
    Ok(radio.status(user_id))
}

/// Turn the tuner off: whatever was on is judged as cut short, and the
/// session closes. Does nothing when nothing is tuned.
pub async fn stop(state: &Arc<AppState>, user_id: i64, reason: &str) {
    if let Ok(mut tasks) = state.radio.tasks.lock() {
        if let Some(h) = tasks.remove(&user_id) {
            h.abort();
        }
    }
    finish(state, user_id, None, reason).await;
}

/// Remove the tuner and settle what it left. `gen` restricts it to one
/// particular tuning, which is how the task ends itself without ending a
/// newer one.
async fn finish(state: &Arc<AppState>, user_id: i64, gen: Option<u64>, reason: &str) {
    let removed = state.radio.tuners.lock().ok().and_then(|mut m| match (m.get(&user_id), gen) {
        (Some(t), Some(g)) if t.gen != g => None,
        (Some(_), _) => m.remove(&user_id),
        (None, _) => None,
    });
    let Some(mut t) = removed else { return };
    if gen.is_some() {
        // The task ending itself: its handle is its own, and aborting it here
        // would cancel the rest of this function.
        if let Ok(mut tasks) = state.radio.tasks.lock() {
            tasks.remove(&user_id);
        }
    }
    let now = now_secs();
    let effects = t.machine.off(now);
    apply(state, user_id, gen, &t, "", effects).await;
    if let Err(e) = state.db.end_radio_session(user_id, t.session_id, now, reason).await {
        warn!("📻 Could not close radio session {}: {e}", t.session_id);
    }
    state.now_playing.clear_user_source(user_id, RADIO_SOURCE);
    info!("📻 Tuner off: user {user_id} ({reason})");
}

async fn close_break(state: &Arc<AppState>, user_id: i64, t: &Tuned, id: i64, now: i64) {
    let status = if t.settings.skip_breaks { "skipped" } else { "logged" };
    if let Ok(Some(line)) = state.db.air_line(user_id, id).await {
        let _ =
            state.db.settle_air_line(user_id, id, &line.kind, status, Some(now), None, None).await;
    }
}

/// Decide what a finished track was, write the listen if it is one, and say
/// so in the air log. A track whose lookup had not come back yet has no row
/// to settle; [`start_track`] marks that one itself when its attach fails.
async fn settle(
    state: &Arc<AppState>,
    user_id: i64,
    t: &Tuned,
    track: &Track<Heard>,
    ended_at: i64,
    natural_end: bool,
) {
    let Some(heard) = &track.payload else { return };
    let prefs = state.db.get_preferences(user_id).await.unwrap_or_default();
    let threshold = ScrobbleThreshold::from_prefs(&prefs);
    let kind = if heard.tagged { "track" } else { "guess" };
    let mbid = heard.play.mbid_recording.clone();
    if !heard_enough(track.started_at, ended_at, track.seen_start, natural_end, &threshold) {
        let _ = state
            .db
            .settle_air_line(
                user_id,
                heard.log_id,
                kind,
                "partial",
                Some(ended_at),
                None,
                mbid.as_deref(),
            )
            .await;
        return;
    }
    let status = if heard.tagged || !t.settings.tagged_only {
        match write_listen(
            state,
            user_id,
            &heard.play,
            t.chain_id,
            &t.preset.band,
            t.settings.tag_radio,
        )
        .await
        {
            true => "scrobbled",
            // A duplicate: the same track from another source inside the
            // dedup window. Logged as held rather than claimed as a listen.
            false => "held",
        }
    } else if t.settings.review {
        "review"
    } else {
        "held"
    };
    let source_id = (status == "scrobbled").then(|| heard.play.source_id.clone());
    let _ = state
        .db
        .settle_air_line(
            user_id,
            heard.log_id,
            kind,
            status,
            Some(ended_at),
            source_id.as_deref(),
            mbid.as_deref(),
        )
        .await;
}

/// Write a radio listen through the one path every listen takes. `true` when
/// it was new.
pub async fn write_listen(
    state: &Arc<AppState>,
    user_id: i64,
    play: &Play,
    chain_id: Option<i64>,
    band: &str,
    tag_radio: bool,
) -> bool {
    match state
        .db
        .save_scrobble(
            user_id,
            play,
            // No quality: what was heard came off a tuner, and the stream's
            // bitrate says nothing about it.
            None,
            None,
            chain_id,
            None,
            tag_radio.then_some("radio"),
            Some(submission_client(band)),
            None,
            ListenDisposition { imported: false, skipped: false },
        )
        .await
    {
        Ok(new) => new,
        Err(e) => {
            warn!("📻 Could not store a radio listen: {e}");
            false
        }
    }
}

/// The station's own names, which a line made only of them is not a track by.
fn station_names(t: &Tuned) -> Vec<String> {
    [Some(&t.station.name), t.station.short_name.as_ref(), Some(&t.preset.name)]
        .into_iter()
        .flatten()
        .cloned()
        .collect()
}

/// A line arrived from the station. The decision is [`Machine::step`]'s;
/// this carries it out.
async fn on_line(state: &Arc<AppState>, user_id: i64, gen: u64, raw: String, first: bool) {
    let now = now_secs();
    let Some((t, effects)) = state.radio.with(user_id, gen, |t| {
        // Any line means the connection is up.
        t.transport = None;
        t.retry_at = None;
        let names = station_names(t);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let effects = t.machine.step(now, &raw, classify(&raw, &names), first);
        (t.clone(), effects)
    }) else {
        return;
    };
    apply(state, user_id, Some(gen), &t, &raw, effects).await;
}

/// End a track that station text paused long ago. Called from the read loop,
/// because a station that falls silent sends no line to end it with.
async fn expire_suspended(state: &Arc<AppState>, user_id: i64, gen: u64) {
    let now = now_secs();
    let Some((t, effects)) = state.radio.with(user_id, gen, |t| {
        let effects = t.machine.tick(now);
        (t.clone(), effects)
    }) else {
        return;
    };
    apply(state, user_id, Some(gen), &t, "", effects).await;
}

/// Carry out what the machine decided, in order. `t` is the tuner as it was
/// right after the decision; `gen` is `None` once it has been removed (Off),
/// when nothing can be attached to it any more.
async fn apply(
    state: &Arc<AppState>,
    user_id: i64,
    gen: Option<u64>,
    t: &Tuned,
    raw: &str,
    effects: Vec<Effect<Heard>>,
) {
    for effect in effects {
        match effect {
            Effect::Refresh => {
                let Some(heard) = t.machine.current.as_ref().and_then(|c| c.payload.as_ref())
                else {
                    continue;
                };
                let now = now_secs();
                let stale =
                    state.now_playing.get(user_id).is_none_or(|(np, _)| now - np.updated_at > 60);
                if stale {
                    publish_now_playing(state, user_id, t, heard, false).await;
                }
            }
            Effect::EndTrack { track, ended_at, natural } => {
                settle(state, user_id, t, &track, ended_at, natural).await;
            }
            Effect::StartTrack { id, artist, title, started_at } => {
                if let Some(gen) = gen {
                    start_track(state, user_id, gen, t, raw, id, artist, title, started_at).await;
                }
            }
            Effect::OpenBreak { id, kind, at } => {
                let status = if t.settings.skip_breaks { "skipped" } else { "logged" };
                let kind = match kind {
                    BreakKind::Break => "break",
                    BreakKind::Other => "other",
                };
                let line = air_line(t, raw, at, None, None, kind, status);
                let Ok(log) = state.db.add_air_line(user_id, &line).await else { continue };
                let kept = gen
                    .and_then(|g| {
                        state.radio.with(user_id, g, |tt| tt.machine.attach_break(id, log))
                    })
                    .unwrap_or(false);
                if !kept {
                    // The run ended while its row was being written.
                    close_break(state, user_id, t, log, at).await;
                }
            }
            Effect::CloseBreak { log, at } => {
                if let Some(log) = log {
                    close_break(state, user_id, t, log, at).await;
                }
            }
            Effect::ClearNowPlaying => state.now_playing.clear_user_source(user_id, RADIO_SOURCE),
        }
    }
}

fn air_line<'a>(
    t: &'a Tuned,
    raw: &'a str,
    started_at: i64,
    artist: Option<&'a str>,
    title: Option<&'a str>,
    kind: &'a str,
    status: &'a str,
) -> NewAirLine<'a> {
    NewAirLine {
        session_id: t.session_id,
        station_id: &t.station.id,
        station_name: &t.station.name,
        band: &t.preset.band,
        started_at,
        raw,
        artist,
        title,
        kind,
        source: ICY,
        status,
    }
}

/// A track began: look it up, log it as playing, and attach both to it — or,
/// when it ended while MusicBrainz was being asked, mark its row cut short.
#[allow(clippy::too_many_arguments)]
async fn start_track(
    state: &Arc<AppState>,
    user_id: i64,
    gen: u64,
    t: &Tuned,
    raw: &str,
    id: u64,
    artist: String,
    title: String,
    started_at: i64,
) {
    let mut play = Play::new(
        title.clone(),
        artist.clone(),
        started_at as u64,
        format!("radio-{}-{}", t.station.id, started_at),
        "radio".into(),
    );
    let tagged =
        enrich_play(&state.mb_client, &mut play, None).await && play.mbid_recording.is_some();
    let kind = if tagged { "track" } else { "guess" };
    let line = air_line(t, raw, started_at, Some(&artist), Some(&title), kind, "playing");
    let log_id = match state.db.add_air_line(user_id, &line).await {
        Ok(id) => id,
        Err(e) => {
            warn!("📻 Could not log a line: {e}");
            return;
        }
    };
    let heard = Heard { log_id, play, tagged };
    let attached = state
        .radio
        .with(user_id, gen, |tt| tt.machine.attach(id, heard.clone()).is_ok().then(|| tt.clone()))
        .flatten();
    match attached {
        Some(tt) => publish_now_playing(state, user_id, &tt, &heard, true).await,
        None => {
            let now = now_secs();
            let _ = state
                .db
                .settle_air_line(user_id, log_id, kind, "partial", Some(now), None, None)
                .await;
        }
    }
}

async fn publish_now_playing(
    state: &Arc<AppState>,
    user_id: i64,
    t: &Tuned,
    heard: &Heard,
    announce: bool,
) {
    let mut np = NowPlaying::from_play(&heard.play, RADIO_SOURCE, None, Progress::Unknown);
    np.listening_context = Some(t.station.name.clone());
    np.chain_name = t.chain_name.clone();
    np.chain_id = t.chain_id;
    let changed = state.now_playing.set(user_id, np);
    // Somebody else's servers hear about a track only when it would also be
    // forwarded as a listen — a guess waiting for review is not announced.
    if announce && changed && (heard.tagged || !t.settings.tagged_only) {
        let sinks = crate::sinks::sinks_for_user(&state.db, user_id, &state.listenbrainz_url)
            .await
            .unwrap_or_default();
        crate::sinks::submit_now_playing(&sinks, &heard.play).await;
    }
}

/// The tuner's task: hold the stream open, feed its lines to [`on_line`],
/// reconnect when it drops, and end the session when auto-stop says so.
async fn run(state: Arc<AppState>, user_id: i64, gen: u64) {
    let radio = &state.radio;
    let mut backoff = 5u64;
    loop {
        let Some((url, egress, ends_at)) = radio.with(user_id, gen, |t| {
            (t.station.stream.as_ref().map(|s| s.url.clone()), t.egress, t.ends_at)
        }) else {
            return;
        };
        if ends_at.is_some_and(|e| now_secs() >= e) {
            finish(&state, user_id, Some(gen), "auto_stop").await;
            return;
        }
        let Some(url) = url else { return };
        radio.with(user_id, gen, |t| {
            if t.transport == Some(TunerState::Dropped) {
                t.transport = Some(TunerState::Tuning);
            }
        });

        match read_stream(&state, user_id, gen, &url, egress).await {
            Ok(()) => return, // ended by auto-stop or replaced
            Err(e) => {
                debug!("📻 Stream for user {user_id} dropped: {e}");
                let retry_at = now_secs() + backoff as i64;
                if radio
                    .with(user_id, gen, |t| {
                        t.transport = Some(TunerState::Dropped);
                        t.retry_at = Some(retry_at);
                    })
                    .is_none()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(60);
            }
        }
    }
}

/// One connection. `Ok` means stop for good; `Err` means reconnect.
async fn read_stream(
    state: &Arc<AppState>,
    user_id: i64,
    gen: u64,
    url: &str,
    egress: Egress,
) -> Result<(), String> {
    egress.check_url(url).map_err(|e| e.to_string())?;
    // A total timeout would cut a stream that is supposed to run all evening;
    // the stall check below is the real one.
    let client = egress.client(Duration::from_secs(24 * 3600));
    let mut resp = tokio::time::timeout(STALL, client.get(url).header("Icy-MetaData", "1").send())
        .await
        .map_err(|_| "connecting timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("the stream answered {}", resp.status()));
    }
    let metaint = resp
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0);
    let Some(metaint) = metaint else {
        // Nothing to read, ever, on this stream: say so and sit out the
        // session rather than reconnecting every minute.
        drop(resp);
        state.radio.with(user_id, gen, |t| {
            t.transport = None;
            t.machine.state = Some(TunerState::Unknown);
            t.machine.line = Some("This station's stream carries no track names".into());
        });
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            let Some(ends_at) = state.radio.with(user_id, gen, |t| t.ends_at) else {
                return Ok(());
            };
            if ends_at.is_some_and(|e| now_secs() >= e) {
                finish(state, user_id, Some(gen), "auto_stop").await;
                return Ok(());
            }
        }
    };

    let mut demux = IcyDemux::new(metaint);
    let mut lines_seen = 0usize;
    let mut last_check = Instant::now();
    loop {
        if last_check.elapsed() > Duration::from_secs(10) {
            last_check = Instant::now();
            expire_suspended(state, user_id, gen).await;
        }
        let Some(ends_at) = state.radio.with(user_id, gen, |t| t.ends_at) else { return Ok(()) };
        if ends_at.is_some_and(|e| now_secs() >= e) {
            finish(state, user_id, Some(gen), "auto_stop").await;
            return Ok(());
        }
        let chunk = match tokio::time::timeout(STALL, resp.chunk()).await {
            Err(_) => return Err("no data for 20 seconds".into()),
            Ok(Err(e)) => return Err(e.to_string()),
            Ok(Ok(None)) => return Err("the server closed the stream".into()),
            Ok(Ok(Some(c))) => c,
        };
        for block in demux.feed(&chunk) {
            if let Some(title) = stream_title(&block) {
                // The first line of a connection was already playing when we
                // arrived; every later one is a change we watched happen.
                on_line(state, user_id, gen, title, lines_seen == 0).await;
                lines_seen += 1;
            }
        }
    }
}

/// Keep a line from the review queue: write it as a listen now.
pub async fn keep(
    state: &Arc<AppState>,
    user_id: i64,
    id: i64,
) -> Result<crate::db::AirLogEntry, TuneError> {
    let line = state
        .db
        .air_line(user_id, id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| TuneError(404, "No such line".into()))?;
    if !matches!(line.status.as_str(), "review" | "held") {
        return Err(TuneError(409, format!("That line is already {}", line.status)));
    }
    let (Some(artist), Some(title)) = (line.artist.clone(), line.title.clone()) else {
        return Err(TuneError(409, "Only a line naming a track can be kept".into()));
    };
    let session = state.db.radio_session(user_id, line.session_id).await.ok().flatten();
    let prefs = state.db.get_preferences(user_id).await.unwrap_or_default();
    let settings = RadioSettings::from_prefs(&prefs);
    let mut play = Play::new(
        title,
        artist,
        line.started_at.max(0) as u64,
        format!("radio-{}-{}", line.station_id, line.started_at),
        "radio".into(),
    );
    enrich_play(&state.mb_client, &mut play, None).await;
    let chain = session.as_ref().and_then(|s| s.chain_id);
    let new = write_listen(state, user_id, &play, chain, &line.band, settings.tag_radio).await;
    let status = if new { "kept" } else { "held" };
    let source_id = new.then(|| play.source_id.clone());
    state
        .db
        .settle_air_line(
            user_id,
            id,
            &line.kind,
            status,
            None,
            source_id.as_deref(),
            play.mbid_recording.as_deref(),
        )
        .await
        .map_err(|e| TuneError(500, e.to_string()))?;
    if !new {
        return Err(TuneError(
            409,
            "Already in your history — the same track from another source".into(),
        ));
    }
    state
        .db
        .air_line(user_id, id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| TuneError(404, "No such line".into()))
}

/// Drop a line from the review queue. It stays in the log, marked dropped.
pub async fn drop_line(
    state: &Arc<AppState>,
    user_id: i64,
    id: i64,
) -> Result<crate::db::AirLogEntry, TuneError> {
    let line = state
        .db
        .air_line(user_id, id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| TuneError(404, "No such line".into()))?;
    if !matches!(line.status.as_str(), "review" | "held") {
        return Err(TuneError(409, format!("That line is already {}", line.status)));
    }
    state
        .db
        .settle_air_line(user_id, id, &line.kind, "dropped", None, None, None)
        .await
        .map_err(|e| TuneError(500, e.to_string()))?;
    state
        .db
        .air_line(user_id, id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| TuneError(404, "No such line".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> ScrobbleThreshold {
        ScrobbleThreshold::default()
    }

    #[test]
    fn a_track_watched_from_start_to_end_counts_whatever_its_length() {
        assert!(heard_enough(0, 180, true, true, &t()), "a 3-minute song heard whole");
        assert!(heard_enough(0, 31, true, true, &t()), "a short one past the floor");
        assert!(!heard_enough(0, 29, true, true, &t()), "Last.fm's 30-second floor holds");
    }

    #[test]
    fn a_track_caught_part_way_needs_the_flat_threshold() {
        // Already on at tune-in.
        assert!(!heard_enough(0, 200, false, true, &t()));
        assert!(heard_enough(0, 240, false, true, &t()));
        // Cut short by Off.
        assert!(!heard_enough(0, 200, true, false, &t()));
        let quick = ScrobbleThreshold { fraction: 0.5, after_secs: 120 };
        assert!(
            heard_enough(0, 130, true, false, &quick),
            "the user's own threshold, not a constant"
        );
    }

    #[test]
    fn settings_default_to_the_design_and_survive_junk() {
        let s = RadioSettings::from_prefs(&serde_json::json!({}));
        assert_eq!(s, RadioSettings::default());
        assert!(
            s.skip_breaks
                && s.tagged_only
                && s.review
                && s.tag_radio
                && s.auto_stop
                && s.attach_chain
        );
        assert_eq!(s.auto_stop_hours, 3);

        let s = RadioSettings::from_prefs(
            &serde_json::json!({"radio": {"review": false, "auto_stop_hours": 40}}),
        );
        assert!(!s.review && s.tagged_only, "a missing field keeps its default");
        assert_eq!(s.auto_stop_hours, 12);
        assert_eq!(s.ends_at(1000), Some(1000 + 12 * 3600));

        let s = RadioSettings::from_prefs(&serde_json::json!({"radio": "nonsense"}));
        assert_eq!(s, RadioSettings::default());
        assert_eq!(RadioSettings { auto_stop: false, ..s }.ends_at(1000), None);
    }
}
