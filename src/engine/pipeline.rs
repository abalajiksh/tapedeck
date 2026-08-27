use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::db::Database;
use crate::models::{ListenDisposition, Play, PlaybackReport};
use crate::musicbrainz::MusicBrainzClient;
use crate::server::now_playing::{NowPlaying, NowPlayingRegistry, Progress};
use crate::sinks;
use crate::sources::{
    MediaBrowserSource, MusicSource, NavidromeSource, PlexFilters, PlexSource, RoonSource,
};

use super::enrichment::enrich_play;

/// How long a displaced ingest track waits before it is written off as a skip.
///
/// This is the whole mechanism for the submitting path: a client may announce
/// the next track before sending the scrobble for the last one, so the verdict
/// has to stay revocable long enough for that scrobble to land and cancel it.
/// Generous on purpose — a skip written in error sits on a permanent record,
/// while a skip written a minute late costs nothing at all.
const SKIP_GRACE_SECS: i64 = 90;

/// Longest a single MusicBrainz lookup may hold the flush.
///
/// Deliberately well under the flush budget: this is the one call in the loop
/// that can be slow without anything being wrong (the client is rate limited to
/// one request a second and its own timeout is thirty), and it is the one whose
/// result is optional. A listen forwards fine with no MBID and the backfill
/// fills it in later, off the tick entirely.
const ENRICH_BUDGET: Duration = Duration::from_secs(4);

/// Write a track that was playing and got replaced before it ever counted.
///
/// A skip is the one thing a session-shaped source can say about how a listen
/// *ended*; everything else in this file is about one beginning. Three
/// deliberate differences from banking a real listen:
///
/// * **No enrichment.** There is no point spending a rate-limited MusicBrainz
///   lookup on a track the listener just rejected, which is the rule the
///   enrichment backfills already follow by filtering `skipped = FALSE`.
/// * **`listened_ms` is written**, because that is the half that makes the flag
///   mean anything: bailing three seconds in and bailing with thirty seconds
///   left are opposite behaviours and the bool stores them identically.
/// * **The device and chain still resolve.** Which gear a track was rejected on
///   is exactly the kind of thing this application exists to record, and a skip
///   accrues no chain hours anyway — `CHAIN_COLUMNS` filters them out.
#[allow(clippy::too_many_arguments)]
async fn bank_skip(
    db: &Database,
    user_id: i64,
    play: &Play,
    // `None` when nothing measured a playhead — an ingest client that reports
    // no position. The row still records *that* it was skipped; it just does
    // not claim to know how far in, which is better than storing wall-clock
    // arithmetic as though it were a measurement.
    listened_secs: Option<u64>,
    device_id: Option<i64>,
    chain_id: Option<i64>,
    listening_context: Option<&str>,
    label: &str,
    row_id: i64,
) {
    // Like-for-like against stored skips only, so a skip cannot be confused
    // with a real listen of the same track in either direction.
    match already_banked(db, user_id, play, true).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            warn!("Could not check for a duplicate skip ({}) — not banking it this pass", e);
            return;
        }
    }
    let disposition = ListenDisposition { imported: false, skipped: true };
    match db
        .save_scrobble(
            user_id,
            play,
            None,
            device_id,
            chain_id,
            None,
            listening_context,
            Some(label),
            Some(row_id),
            disposition,
        )
        .await
    {
        Ok(true) => {
            // A follow-up write rather than another argument on `save_scrobble`,
            // which has 53 call sites and a documented history of positional
            // arguments landing in the wrong slot. The usual objection to
            // insert-then-update does not reach here: a skipped row is filed
            // `'imported'` from the first statement, so no flush tick can catch
            // it half-written and forward it.
            let report = PlaybackReport {
                listened_ms: listened_secs.map(|s| s as i64 * 1000),
                ..Default::default()
            };
            if !report.is_empty() {
                if let Err(e) = db
                    .set_listen_playback(user_id, &play.source_id, &play.source_name, &report)
                    .await
                {
                    warn!("Could not record how far into the skipped track we got: {}", e);
                }
            }
            match listened_secs {
                Some(s) => info!(
                    "⏭️  Skipped for user {}: {} - {} ({}s in)",
                    user_id, play.artist, play.title, s
                ),
                None => info!(
                    "⏭️  Skipped for user {}: {} - {} (position not reported)",
                    user_id, play.artist, play.title
                ),
            }
        }
        Ok(false) => {}
        Err(e) => error!("Database error recording a skip: {}", e),
    }
}

/// Has this listen already been banked?
///
/// Three answers, not two — the third is the point. Both halves used to be
/// `.unwrap_or(false)`, so a busy database answered *"no, this is new"* and the
/// caller banked a second copy. The two halves fail differently and only one of
/// them was ever safe:
///
/// * `exists` is backstopped by `UNIQUE(user_id, source_id, source_name)`, so
///   the worst a wrong answer costs is a refused insert.
/// * `fuzzy_exists` has **no constraint behind it**. It is the only thing
///   catching the live/history double-report — the class of bug v0.13.3 and
///   v0.65.0 were both about — so failing open here writes a duplicate listen
///   that nothing will ever reconcile, and `flush_pending` then forwards it to
///   the user's permanent public record.
///
/// So the error propagates and the caller skips this pass. Nothing is lost: the
/// play is still in the session or the history walk, and the next tick asks
/// again. A late listen is recoverable; a duplicate on Last.fm is not.
async fn already_banked(
    db: &Database,
    user_id: i64,
    play: &Play,
    like_for_like_skip: bool,
) -> Result<bool, sqlx::Error> {
    if db.exists(user_id, &play.source_id, &play.source_name).await? {
        return Ok(true);
    }
    db.fuzzy_exists(user_id, play, like_for_like_skip).await
}

pub struct ScrobbleEngine {
    /// Live sources, one per (user, kind) row in `user_sources`. Reconciled at
    /// the top of every tick against the database.
    sources: Vec<ActiveSource>,
    db: Arc<Database>,
    mb_client: Arc<MusicBrainzClient>,
    /// How often the tick itself runs. Individual sources are polled on their
    /// own schedule, which may be slower than this. Short, because a session
    /// source polled on this cadence is what makes the deck feel live — the
    /// expensive work is throttled separately rather than by slowing the tick.
    poll_interval: Duration,
    /// Next wall-clock second the enrichment backfills may run.
    next_backfill_at: i64,
    /// Which pass the next round starts from. See [`Self::run_backfills`].
    backfill_cursor: usize,
    /// Next wall-clock second the per-user scrobble thresholds are re-read.
    next_threshold_refresh_at: i64,
    /// ListenBrainz endpoint (self-hosted instances override the default).
    listenbrainz_url: String,
    /// Shared with the HTTP layer so the dashboard deck reflects Plex sessions.
    now_playing: NowPlayingRegistry,
    /// Artwork lookup chain, cheapest first. Built once — each provider holds a
    /// pooled HTTP client.
    artwork_providers: Vec<Box<dyn crate::metadata::ArtworkProvider>>,
    /// LRCLIB, for the language tier that reads a lyric's script. `None` when
    /// the cache database could not be opened, which is a degraded state rather
    /// than a fatal one — every other backfill still runs.
    lyrics: Option<Arc<crate::lyrics::LyricsClient>>,
    /// Rung by a push-shaped source that has something new to say, so the tick
    /// happens sooner than the interval.
    ///
    /// Only Roon uses it today. It is engine-level rather than Roon-specific
    /// because the *state* still comes from a poll — the bell only shortens the
    /// wait, so any future push source gets the same benefit without a second
    /// mechanism. A missed notify costs latency and nothing else, which is what
    /// makes it safe to treat as a hint.
    wake: Arc<tokio::sync::Notify>,
}

/// A built, polling source and the bookkeeping the engine needs around it.
struct ActiveSource {
    /// `user_sources.id` — for writing poll outcomes back.
    row_id: i64,
    /// The Tapedeck user this source belongs to. Listens from it are *theirs*;
    /// there is no name-matching and no fallback user any more.
    user_id: i64,
    kind: String,
    /// `updated_at` of the row this was built from; a newer value means rebuild.
    stamp: i64,
    source: Box<dyn MusicSource>,
    /// Staggered so N users × M sources don't all fire on the same tick.
    next_poll_at: i64,
    poll_interval: i64,
    /// Set after reporting a failure, so a source that stays down doesn't fill
    /// the log at every interval.
    error_logged: bool,
}

/// Default per-source cadence, by kind. This is what the deck's latency is
/// mostly made of, so it's set by how expensive one poll actually is rather
/// than by a single number for everything.
///
/// Plex and Jellyfin answer with one small "what is playing right now"
/// response, which is cheap enough to ask for every tick — and asking every 30s
/// meant up to half a minute of silence after pressing play. Navidrome's poll
/// also walks its history endpoint and writes a row back, so it stays slow; a
/// user who wants it snappier can raise it per source in the UI.
fn default_interval_secs(kind: &str) -> i64 {
    match kind {
        "plex" | "jellyfin" | "emby" | "roon" => 5,
        _ => 30,
    }
}

/// How often the MusicBrainz backfills and the artwork chain get a turn.
/// Deliberately decoupled from the tick: the tick is fast so the deck is fast,
/// and tripling the rate of rate-limited lookups is exactly what that must not
/// cost.
const BACKFILL_INTERVAL_SECS: i64 = 60;

/// Where the next backfill round begins, given how many passes this one got
/// through.
///
/// Pure and separate for the reason `resume_point` and `consume_sessions` are:
/// it decides whether a pass ever runs at all, and a rule like that must be
/// testable without an engine. A wrong modulo here does not fail — it starves
/// one pass forever, silently, and the symptom is artwork that never resolves.
fn advance_backfill_cursor(cursor: usize, ran: usize, passes: usize) -> usize {
    if passes == 0 {
        return 0;
    }
    (cursor + ran) % passes
}

/// How often the per-user scrobble thresholds are re-read from preferences.
///
/// On a clock of its own for the same reason the backfills are: the tick is 5s
/// because that is what makes the deck feel live, and a setting two numbers wide
/// that changes about once in the life of an install has no business being read
/// twelve times a minute per user. The visible cost is that moving the slider
/// takes effect on the next minute rather than the next second, which is well
/// inside the length of the track it would apply to.
///
/// It is applied to the *running* source rather than rebuilding it — see
/// `MusicSource::set_scrobble_threshold` for why that matters.
const THRESHOLD_REFRESH_SECS: i64 = 60;

/// `user_sources.kind` → the label the now-playing registry stores. They differ
/// because the registry's label is user-visible and predates the kind column.
fn source_label(kind: &str) -> &'static str {
    match kind {
        "plex" => "Plex",
        "navidrome" => "Navidrome",
        "jellyfin" => "Jellyfin",
        "emby" => "Emby",
        "roon" => "Roon",
        _ => "unknown",
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

impl ScrobbleEngine {
    pub fn new(
        db: Arc<Database>,
        mb_client: Arc<MusicBrainzClient>,
        now_playing: NowPlayingRegistry,
        listenbrainz_url: String,
    ) -> Self {
        Self {
            sources: Vec::new(),
            db,
            mb_client,
            poll_interval: Duration::from_secs(5),
            next_backfill_at: 0,
            backfill_cursor: 0,
            // Zero so the first tick reads them, rather than every source
            // running a minute on the default before the owner's setting lands.
            next_threshold_refresh_at: 0,
            now_playing,
            listenbrainz_url,
            artwork_providers: vec![Box::new(crate::metadata::deezer::DeezerProvider::new())],
            lyrics: None,
            wake: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Hand the engine a lyrics client. Separate from `new` because the cache
    /// database may fail to open and that must not stop the engine — the
    /// language tier is the only thing lost, and `None` simply skips it.
    pub fn with_lyrics(mut self, lyrics: Arc<crate::lyrics::LyricsClient>) -> Self {
        self.lyrics = Some(lyrics);
        self
    }

    /// Push each user's scrobble threshold down into their running sources.
    ///
    /// Pushed rather than pulled because the decision lives inside the source's
    /// state machine — `SessionTracker::observe` and Plex's equivalent are what
    /// know whether this pass has already been banked, and they answer in the
    /// same breath as deciding whether it should be. Handing them the number is
    /// the only way to change the rule without moving that logic out.
    ///
    /// A source whose owner has never touched the setting is told the default
    /// explicitly, so turning the setting *back* to standard applies as
    /// immediately as setting it did.
    /// Write off the ingest tracks whose grace period has run out.
    ///
    /// The threshold decides, and it is the **listener's own** — the same
    /// number their sources use, so "what counts as played" has one answer
    /// across the application rather than one per code path.
    ///
    /// A track that got past it is *not* a skip even though nothing banked it:
    /// that is a client which reports now-playing and never scrobbles, and
    /// filing its whole history as skips would be a fact about the client
    /// rather than about the listening.
    async fn resolve_pending_skips(&self) {
        let due = self.now_playing.take_due_skips(now_secs(), SKIP_GRACE_SECS);
        if due.is_empty() {
            return;
        }
        let thresholds = self.db.scrobble_thresholds().await.unwrap_or_default();
        for (user_id, pending) in due {
            let threshold = thresholds.get(&user_id).copied().unwrap_or_default();
            if threshold.reached(pending.bound_secs, pending.duration) {
                continue;
            }
            bank_skip(
                &self.db,
                user_id,
                &pending.play,
                // `listened_secs` is `None` when the client never reported a
                // playhead. The decision above still used the wall-clock bound,
                // because a bound is enough to answer "did this get far enough
                // to count"; storing it as `listened_ms` would state arithmetic
                // as a measurement, which is what `Progress` exists to prevent.
                pending.listened_secs,
                None,
                None,
                None,
                "ingest",
                0,
            )
            .await;
        }
    }

    async fn apply_scrobble_thresholds(&mut self) {
        let configured = match self.db.scrobble_thresholds().await {
            Ok(map) => map,
            Err(e) => {
                // Leave every source on whatever it already holds. A failed read
                // must not quietly reset everybody to the default mid-track.
                error!("Failed to read scrobble thresholds: {}", e);
                return;
            }
        };
        for active in &mut self.sources {
            let threshold = configured.get(&active.user_id).copied().unwrap_or_default();
            active.source.set_scrobble_threshold(threshold);
        }
    }

    /// Bring the live source registry in line with `user_sources`.
    ///
    /// Runs at the top of every tick, so a user's save applies within a tick
    /// instead of at the next restart, and a server that was unreachable when we
    /// first tried gets picked up when it comes back. One code path for startup
    /// and for later edits.
    async fn reconcile_sources(&mut self) {
        let mut added = false;
        let configured = match self.db.list_enabled_sources().await {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to read user sources: {}", e);
                return;
            }
        };

        // Drop anything that was disabled, deleted, or reconfigured. A stale
        // token must stop polling immediately, not linger until the next edit.
        let live: Vec<(i64, i64)> = configured.iter().map(|c| (c.id, c.updated_at)).collect();
        self.sources.retain(|active| {
            let still_current =
                live.iter().any(|(id, stamp)| *id == active.row_id && *stamp == active.stamp);
            if !still_current {
                self.now_playing.clear_user_source(active.user_id, source_label(&active.kind));
                info!(
                    "⚙️ Source {} for user {} stopped (changed or disabled)",
                    active.kind, active.user_id
                );
            }
            still_current
        });

        for cfg in configured {
            if self.sources.iter().any(|a| a.row_id == cfg.id) {
                continue;
            }
            if !cfg.is_usable() {
                continue;
            }

            match self.build_source(&cfg).await {
                Ok(source) => {
                    info!("✅ {} connected for user {} ({})", cfg.kind, cfg.user_id, cfg.url);
                    let _ = self.db.record_source_poll(cfg.id, None).await;
                    let interval = cfg
                        .poll_interval_secs
                        .filter(|s| *s > 0)
                        .unwrap_or_else(|| default_interval_secs(&cfg.kind));
                    self.sources.push(ActiveSource {
                        row_id: cfg.id,
                        user_id: cfg.user_id,
                        kind: cfg.kind.clone(),
                        stamp: cfg.updated_at,
                        source,
                        // Stagger the first poll so several sources coming up at
                        // once don't all fire on the same tick.
                        next_poll_at: now_secs() + (self.sources.len() as i64 % interval.max(1)),
                        poll_interval: interval,
                        error_logged: false,
                    });
                    added = true;
                }
                Err(e) => {
                    // Not added, so the next tick retries. Record it so the
                    // Settings screen can show why nothing is arriving.
                    let msg = e.to_string();
                    error!(
                        "❌ {} unreachable for user {} ({}): {}",
                        cfg.kind, cfg.user_id, cfg.url, msg
                    );
                    let _ = self.db.record_source_poll(cfg.id, Some(&msg)).await;
                }
            }
        }

        // A source built between refreshes would otherwise run on the default
        // for up to a minute — which is a whole track, and exactly the track
        // somebody who just changed the setting is testing it with.
        if added {
            self.apply_scrobble_thresholds().await;
        }
    }

    /// Construct and connect one source from its stored config.
    ///
    /// For Plex this is where per-user isolation is enforced: the source is
    /// pinned to the Plex account the user's own token belongs to, so other
    /// people's sessions on the same server are filtered out rather than
    /// attributed by guesswork.
    async fn build_source(
        &self,
        cfg: &crate::db::UserSource,
    ) -> Result<Box<dyn MusicSource>, Box<dyn std::error::Error>> {
        match cfg.kind.as_str() {
            "plex" => {
                let token = cfg.credential("token").ok_or("no Plex token stored")?;
                let account = cfg
                    .remote_account
                    .clone()
                    .ok_or("Plex account not resolved — reconnect the source")?;

                let filters = PlexFilters {
                    // The allow-list *is* the isolation. Empty would mean
                    // "everyone on the server", which is what we're moving away from.
                    users_allow: vec![account.to_lowercase()],
                    // Resolved from the name by `initialize`; the history
                    // endpoint identifies a listener by id and never by name.
                    accounts_allow: Vec::new(),
                    users_block: Vec::new(),
                    libraries_allow: cfg.option_list("libraries_allow"),
                    libraries_block: cfg.option_list("libraries_block"),
                    devices_allow: cfg.option_list("devices_allow"),
                    devices_block: cfg.option_list("devices_block"),
                };
                let mut plex = PlexSource::with_filters(cfg.url.clone(), token, filters);
                plex.initialize().await?;
                Ok(Box::new(plex))
            }
            "navidrome" => {
                let username = cfg.credential("username").ok_or("no Navidrome username stored")?;
                let password = cfg.credential("password").ok_or("no Navidrome password stored")?;
                let mut nd = NavidromeSource::new(cfg.url.clone(), username, password);
                nd.initialize().await?;
                // Navidrome keeps its own play history, so without a high-water
                // mark a restart would re-walk all of it. Dedup would absorb the
                // duplicates, but only after thousands of pointless inserts.
                if let Some(ts) = cfg.last_polled_at {
                    nd.resume_from(ts.max(0) as u64);
                }
                Ok(Box::new(nd))
            }
            // One protocol with a prefix on it — see `sources::mediabrowser`.
            // The account filter is the whole of the isolation for both: a
            // shared server must not leak one housemate's listens onto another.
            kind @ ("jellyfin" | "emby") => {
                let emby = kind == "emby";
                let label = if emby { "Emby" } else { "Jellyfin" };
                let key = cfg
                    .credential("api_key")
                    .ok_or_else(|| format!("no {label} API key stored"))?;
                let account = cfg.remote_account.clone().ok_or_else(|| {
                    format!("{label} account not resolved — reconnect the source")
                })?;
                let mut mb = if emby {
                    MediaBrowserSource::emby(cfg.url.clone(), key, account)
                } else {
                    MediaBrowserSource::jellyfin(cfg.url.clone(), key, account)
                };
                mb.initialize().await?;
                Ok(Box::new(mb))
            }
            "roon" => {
                // No URL to type — SOOD finds the Core — but a host may be set
                // when discovery is blocked across subnets, which is common
                // enough on a VLANed home network to be worth allowing.
                let host = if cfg.url.trim().is_empty() {
                    return Err("no Roon Core address — discovery is not wired yet".into());
                } else {
                    cfg.url.trim().trim_start_matches("http://").to_string()
                };
                let (host, port) = match host.rsplit_once(':') {
                    Some((h, p)) => (h.to_string(), p.parse::<u16>().ok()),
                    None => (host, None),
                };
                let mut roon = crate::sources::RoonSource::new(
                    host,
                    port,
                    cfg.credential("pairing").as_deref(),
                    cfg.option_list("zones_allow"),
                    self.wake.clone(),
                )?;
                // A first run legitimately fails here: Roon will not talk to an
                // extension the user has not approved in Settings → Extensions.
                // The engine retries every tick, so approval is picked up
                // without anyone restarting anything.
                roon.initialize().await?;
                Ok(Box::new(roon))
            }
            other => Err(format!("unknown source kind: {other}").into()),
        }
    }

    /// Runs the poll loop until `shutdown` flips to `true`. A tick always runs
    /// to completion (never interrupted mid-write); shutdown is checked at the
    /// tick boundary, and the between-ticks sleep is interruptible so we stop
    /// promptly. Returning cleanly guarantees we exit at a consistent point.
    pub async fn run(&mut self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        use futures_util::future::FutureExt;
        use std::panic::AssertUnwindSafe;

        info!("🎵 Starting scrobble loop with prioritized MusicBrainz metadata enrichment...");
        crate::engine::health::set_interval(self.poll_interval);
        loop {
            if *shutdown.borrow() {
                break;
            }

            // Timed so Settings → Operations can say whether the engine is
            // keeping its own schedule. A tick longer than its interval is the
            // one number that would have made 2026-08-24 visible from inside
            // the running application rather than from 3 MB of log over SSH.
            let tick_at = Instant::now();
            crate::engine::health::tick_started();

            // Isolate each tick: a panic here would otherwise kill the engine
            // task while the HTTP server keeps running, silently stopping all
            // scrobbling. Catch it, log it, and carry on next tick.
            if let Err(p) = AssertUnwindSafe(self.run_tick()).catch_unwind().await {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                error!("💥 Scrobble tick panicked (recovered): {}", msg);
            }
            crate::engine::health::tick_finished(tick_at.elapsed());
            if tick_at.elapsed() > self.poll_interval {
                debug!(
                    "Tick took {:?} against a {:?} interval — sources and the deck are that late",
                    tick_at.elapsed(),
                    self.poll_interval
                );
            }

            // A completed tick reached a clean boundary — stop now if asked.
            if *shutdown.borrow() {
                break;
            }

            // The bell only ever shortens this wait. A source that rings it
            // still has its state read by the ordinary poll on the next tick,
            // so a dropped or spurious notify is a latency change and never a
            // correctness one.
            tokio::select! {
                _ = sleep(self.poll_interval) => {}
                _ = self.wake.notified() => {}
                _ = shutdown.changed() => {}
            }
        }
        info!("🛑 Scrobble engine stopped cleanly");
    }

    /// Poll one Navidrome source: new scrobbles from its own history, plus
    /// whatever the account is playing right now.
    #[allow(clippy::too_many_arguments)]
    async fn poll_navidrome(
        &mut self,
        i: usize,
        db: &Arc<Database>,
        mb_client: &Arc<MusicBrainzClient>,
        lb_url: &str,
        now_playing: &NowPlayingRegistry,
    ) {
        let (row_id, user_id) = (self.sources[i].row_id, self.sources[i].user_id);
        let Some(nd) = self.sources[i].source.as_any_mut().downcast_mut::<NavidromeSource>() else {
            return;
        };

        // Only this source's own account counts — a shared Navidrome reports
        // every user's playback, same as Plex.
        let account = nd.username_owned();

        match nd.fetch_now_playing().await {
            Ok(entries) => {
                let mine: Vec<_> = entries
                    .into_iter()
                    .filter(|e| {
                        e.username
                            .as_deref()
                            .map(|u| u.eq_ignore_ascii_case(&account))
                            .unwrap_or(true)
                    })
                    .collect();
                if mine.is_empty() {
                    now_playing.clear_user_source(user_id, "Navidrome");
                }
                for entry in mine {
                    let mut play = entry.play.clone();
                    // Deck first, network second: the registry needs nothing
                    // from MusicBrainz, so enriching before writing it just
                    // spent a rate-limited round trip on the user's latency.
                    let changed = now_playing.set(
                        user_id,
                        // Subsonic's `getNowPlaying` answers `minutesAgo`,
                        // not a playhead — there is nothing here to report.
                        NowPlaying::from_play(&play, "Navidrome", None, Progress::Unknown),
                    );
                    if changed {
                        let album = play.album.clone();
                        enrich_play(mb_client, &mut play, album.as_deref()).await;
                        let sinks =
                            sinks::sinks_for_user(db, user_id, lb_url).await.unwrap_or_default();
                        sinks::submit_now_playing(&sinks, &play).await;
                    }
                }
            }
            Err(e) => debug!("Navidrome now-playing failed for user {}: {}", user_id, e),
        }

        let from = nd.last_submission();
        match nd.fetch_scrobbles(from).await {
            Ok(plays) => {
                if self.sources[i].error_logged {
                    info!("✅ Navidrome recovered for user {}", user_id);
                    self.sources[i].error_logged = false;
                }
                let _ = db.record_source_poll(row_id, None).await;
                if !plays.is_empty() {
                    info!("📀 {} Navidrome listen(s) for user {}", plays.len(), user_id);
                }
                for mut play in plays {
                    let dupe = match already_banked(db, user_id, &play, false).await {
                        Ok(d) => d,
                        Err(e) => {
                            warn!(
                                "Could not check for a duplicate ({}) — leaving it for the \
                                 next tick",
                                e
                            );
                            continue;
                        }
                    };
                    if dupe {
                        continue;
                    }
                    // A library MBID means enrichment is already done.
                    if play.mbid_recording.is_none() {
                        let album = play.album.clone();
                        enrich_play(mb_client, &mut play, album.as_deref()).await;
                    }
                    match db
                        .save_scrobble(
                            user_id,
                            &play,
                            None,
                            None,
                            None,
                            None,
                            None,
                            Some("Navidrome"),
                            Some(row_id),
                            ListenDisposition::LIVE,
                        )
                        .await
                    {
                        Ok(true) => info!(
                            "📥 Queued for user {}: {} - {}",
                            user_id, play.artist, play.title
                        ),
                        Ok(false) => {}
                        Err(e) => error!("Database error: {}", e),
                    }
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if !self.sources[i].error_logged {
                    error!("Navidrome fetch failed for user {}: {}", user_id, msg);
                    self.sources[i].error_logged = true;
                } else {
                    debug!("Navidrome still failing for user {}: {}", user_id, msg);
                }
                let _ = db.record_source_poll(row_id, Some(&msg)).await;
            }
        }
    }

    /// Poll one Jellyfin **or Emby** source — they are one protocol, so one
    /// poller. Session-shaped like Plex: neither records listens natively, so
    /// there is no history endpoint to backfill from and everything comes from
    /// what is playing now.
    ///
    /// The label is read off the source's own flavour rather than hardcoded, so
    /// the now-playing registry key and every log line name the server actually
    /// being polled — otherwise an Emby tick would clear a Jellyfin deck entry.
    #[allow(clippy::too_many_arguments)]
    async fn poll_media_browser(
        &mut self,
        i: usize,
        db: &Arc<Database>,
        mb_client: &Arc<MusicBrainzClient>,
        lb_url: &str,
        now_playing: &NowPlayingRegistry,
    ) {
        let (row_id, user_id) = (self.sources[i].row_id, self.sources[i].user_id);
        let Some(jf) = self.sources[i].source.as_any_mut().downcast_mut::<MediaBrowserSource>()
        else {
            return;
        };
        let label = jf.flavour().label();

        let sessions = match jf.fetch_sessions().await {
            Ok(s) => {
                if self.sources[i].error_logged {
                    info!("✅ {} recovered for user {}", label, user_id);
                    self.sources[i].error_logged = false;
                }
                let _ = db.record_source_poll(row_id, None).await;
                s
            }
            Err(e) => {
                let msg = e.to_string();
                if !self.sources[i].error_logged {
                    error!("{} fetch failed for user {}: {}", label, user_id, msg);
                    self.sources[i].error_logged = true;
                } else {
                    debug!("{} still failing for user {}: {}", label, user_id, msg);
                }
                let _ = db.record_source_poll(row_id, Some(&msg)).await;
                return;
            }
        };

        if sessions.now_playing.is_empty() {
            now_playing.clear_user_source(user_id, label);
        }

        for track in &sessions.now_playing {
            let mut play = track.play.clone();
            let mut np = NowPlaying::from_play(
                &play,
                label,
                track.quality.as_ref(),
                Progress::Reported(track.position),
            );
            // The chain this listen *will* be attributed to, resolved the way
            // the scrobble branch below resolves it — so Patch's live strip can
            // apply the same shared-chain rule the Reel does, rather than a
            // second rule invented for live entries.
            //
            // Read-only (`device_chain_by_key`, not `upsert_device`): creating
            // a device row is right once per banked listen and wrong every five
            // seconds for something still playing.
            np.chain_id = match &track.player {
                Some(p) => db.device_chain_by_key(user_id, &p.machine_id).await.ok().flatten(),
                None => None,
            };
            let changed = now_playing.set(user_id, np);
            if changed {
                if play.mbid_recording.is_none() {
                    let album = play.album.clone();
                    enrich_play(mb_client, &mut play, album.as_deref()).await;
                }
                let sinks = sinks::sinks_for_user(db, user_id, lb_url).await.unwrap_or_default();
                sinks::submit_now_playing(&sinks, &play).await;
            }
        }

        for track in sessions.ready_to_scrobble {
            let mut play = track.play;
            let dupe = match already_banked(db, user_id, &play, false).await {
                Ok(d) => d,
                Err(e) => {
                    warn!("Could not check for a duplicate ({}) — leaving it for the next tick", e);
                    continue;
                }
            };
            if dupe {
                continue;
            }
            if play.mbid_recording.is_none() {
                let album = play.album.clone();
                enrich_play(mb_client, &mut play, album.as_deref()).await;
            }

            // Register the client and let it carry the chain — the same fix
            // Plex got in v0.22.0, and it was missing here for the same reason.
            // A Jellyfin listen has no token, so the ladder's first three rungs
            // can never match; with no device it fell through all four and
            // every Jellyfin listen was stored with no signal chain at all.
            let (device_id, chain_id) = match &track.player {
                Some(p) => match db
                    .upsert_device(
                        user_id,
                        &p.machine_id,
                        p.name.as_deref(),
                        p.platform.as_deref(),
                        p.product.as_deref(),
                    )
                    .await
                {
                    Ok(dev) => {
                        let chain = db.get_device_default_chain(user_id, dev).await.ok().flatten();
                        (Some(dev), chain)
                    }
                    Err(e) => {
                        warn!("Could not record {} client as a device: {}", label, e);
                        (None, None)
                    }
                },
                None => (None, None),
            };

            let listening_context = match chain_id {
                Some(cid) => db
                    .get_chain(user_id, cid)
                    .await
                    .ok()
                    .flatten()
                    .map(|c| c.listening_context.as_str().to_string()),
                None => None,
            };

            match db
                .save_scrobble(
                    user_id,
                    &play,
                    track.quality.as_ref(),
                    device_id,
                    chain_id,
                    None,
                    listening_context.as_deref(),
                    Some(label),
                    Some(row_id),
                    ListenDisposition::LIVE,
                )
                .await
            {
                Ok(true) => {
                    info!("📥 Queued for user {}: {} - {}", user_id, play.artist, play.title)
                }
                Ok(false) => {}
                Err(e) => error!("Database error: {}", e),
            }
        }

        // Tracks the listener moved on from before they ever counted. Same
        // session, so the same client and therefore the same chain.
        for track in sessions.skipped {
            let (device_id, chain_id) = match &track.player {
                Some(p) => match db
                    .upsert_device(
                        user_id,
                        &p.machine_id,
                        p.name.as_deref(),
                        p.platform.as_deref(),
                        p.product.as_deref(),
                    )
                    .await
                {
                    Ok(dev) => {
                        let chain = db.get_device_default_chain(user_id, dev).await.ok().flatten();
                        (Some(dev), chain)
                    }
                    Err(_) => (None, None),
                },
                None => (None, None),
            };
            let listening_context = match chain_id {
                Some(cid) => db
                    .get_chain(user_id, cid)
                    .await
                    .ok()
                    .flatten()
                    .map(|c| c.listening_context.as_str().to_string()),
                None => None,
            };
            bank_skip(
                db,
                user_id,
                &track.play,
                Some(track.position),
                device_id,
                chain_id,
                listening_context.as_deref(),
                label,
                row_id,
            )
            .await;
        }
    }

    /// Poll one Roon source.
    ///
    /// Session-shaped like Plex, Jellyfin and Emby — Roon's transport API has
    /// no history — but with two differences worth knowing:
    ///
    /// * **The pairing token can change under us.** Roon issues one when the
    ///   user approves the extension, and may reissue it. It is written back
    ///   only when it actually changed, so this is not a row rewrite every
    ///   tick.
    /// * **A Roon listen has no MBIDs at all**, so enrichment always runs. That
    ///   is the honest cost of the source, not something to work around.
    #[allow(clippy::too_many_arguments)]
    async fn poll_roon(
        &mut self,
        i: usize,
        db: &Arc<Database>,
        mb_client: &Arc<MusicBrainzClient>,
        lb_url: &str,
        now_playing: &NowPlayingRegistry,
    ) {
        let (row_id, user_id) = (self.sources[i].row_id, self.sources[i].user_id);
        let Some(roon) = self.sources[i].source.as_any_mut().downcast_mut::<RoonSource>() else {
            return;
        };

        let zones = match roon.fetch_zones().await {
            Ok(z) => {
                if self.sources[i].error_logged {
                    info!("✅ Roon recovered for user {}", user_id);
                    self.sources[i].error_logged = false;
                }
                let _ = db.record_source_poll(row_id, None).await;
                z
            }
            Err(e) => {
                let msg = e.to_string();
                if !self.sources[i].error_logged {
                    error!("Roon fetch failed for user {}: {}", user_id, msg);
                    self.sources[i].error_logged = true;
                } else {
                    debug!("Roon still failing for user {}: {}", user_id, msg);
                }
                let _ = db.record_source_poll(row_id, Some(&msg)).await;
                return;
            }
        };

        // Only when Roon actually issued a new one — see `take_if_changed`.
        if let Some(state) = self.sources[i]
            .source
            .as_any_mut()
            .downcast_mut::<RoonSource>()
            .and_then(|r| r.take_pairing_state())
        {
            if let Err(e) = db.set_source_credential(row_id, "pairing", &state).await {
                warn!("Could not persist the Roon pairing token: {}", e);
            }
        }

        if zones.now_playing.is_empty() {
            now_playing.clear_user_source(user_id, "Roon");
        }

        for track in &zones.now_playing {
            let mut play = track.play.clone();
            let mut np =
                NowPlaying::from_play(&play, "Roon", None, Progress::Reported(track.position));
            // Roon's endpoint is the device — the only rung of the ladder a
            // Roon listen ever reaches.
            np.chain_id = match &track.output {
                Some(o) => db.device_chain_by_key(user_id, &o.machine_id).await.ok().flatten(),
                None => None,
            };
            let changed = now_playing.set(user_id, np);
            if changed {
                if play.mbid_recording.is_none() {
                    let album = play.album.clone();
                    enrich_play(mb_client, &mut play, album.as_deref()).await;
                }
                let sinks = sinks::sinks_for_user(db, user_id, lb_url).await.unwrap_or_default();
                sinks::submit_now_playing(&sinks, &play).await;
            }
        }

        for track in zones.ready_to_scrobble {
            // A Core has several zones and the listen only says which endpoint
            // it went to, so the zone is what makes a log line locatable.
            let zone_id = track.zone_id;
            let mut play = track.play;
            let dupe = match already_banked(db, user_id, &play, false).await {
                Ok(d) => d,
                Err(e) => {
                    warn!("Could not check for a duplicate ({}) — leaving it for the next tick", e);
                    continue;
                }
            };
            if dupe {
                continue;
            }
            if play.mbid_recording.is_none() {
                let album = play.album.clone();
                enrich_play(mb_client, &mut play, album.as_deref()).await;
            }

            // The zone's output is the device, which is the only rung of the
            // chain ladder a Roon listen can reach — there is no token and no
            // signal path. Without it every Roon listen would land with no
            // chain, the bug Plex had before v0.22.0.
            let (device_id, chain_id) = match &track.output {
                Some(o) => match db
                    .upsert_device(user_id, &o.machine_id, o.name.as_deref(), None, Some("Roon"))
                    .await
                {
                    Ok(dev) => {
                        let chain = db.get_device_default_chain(user_id, dev).await.ok().flatten();
                        (Some(dev), chain)
                    }
                    Err(e) => {
                        warn!("Could not record the Roon endpoint as a device: {}", e);
                        (None, None)
                    }
                },
                None => (None, None),
            };

            let listening_context = match chain_id {
                Some(cid) => db
                    .get_chain(user_id, cid)
                    .await
                    .ok()
                    .flatten()
                    .map(|c| c.listening_context.as_str().to_string()),
                None => None,
            };

            match db
                .save_scrobble(
                    user_id,
                    &play,
                    // Roon reports no format, sample rate or bit depth — the
                    // signal path is a client feature and is not in the API.
                    // Better unset than asserted on no evidence.
                    None,
                    device_id,
                    chain_id,
                    None,
                    listening_context.as_deref(),
                    Some("Roon"),
                    Some(row_id),
                    ListenDisposition::LIVE,
                )
                .await
            {
                Ok(true) => {
                    info!(
                        "📥 Queued for user {} from Roon zone {}: {} - {}",
                        user_id, zone_id, play.artist, play.title
                    )
                }
                Ok(false) => {}
                Err(e) => error!("Database error: {}", e),
            }
        }

        // Zones the listener moved on from before the track ever counted.
        for track in zones.skipped {
            let (device_id, chain_id) = match &track.output {
                Some(o) => match db
                    .upsert_device(user_id, &o.machine_id, o.name.as_deref(), None, Some("Roon"))
                    .await
                {
                    Ok(dev) => {
                        let chain = db.get_device_default_chain(user_id, dev).await.ok().flatten();
                        (Some(dev), chain)
                    }
                    Err(_) => (None, None),
                },
                None => (None, None),
            };
            let listening_context = match chain_id {
                Some(cid) => db
                    .get_chain(user_id, cid)
                    .await
                    .ok()
                    .flatten()
                    .map(|c| c.listening_context.as_str().to_string()),
                None => None,
            };
            bank_skip(
                db,
                user_id,
                &track.play,
                Some(track.position),
                device_id,
                chain_id,
                listening_context.as_deref(),
                "Roon",
                row_id,
            )
            .await;
        }
    }

    pub async fn run_tick(&mut self) {
        self.reconcile_sources().await;

        let db = self.db.clone();
        let mb_client = self.mb_client.clone();
        let lb_url = self.listenbrainz_url.clone();
        let now_playing = self.now_playing.clone();
        let now = now_secs();

        for i in 0..self.sources.len() {
            // A push-shaped source that has been told something changed is
            // polled now; everything else waits for its interval. `take_wakeup`
            // consumes the flag, so one notification cannot poll forever.
            let rung = self.sources[i].source.take_wakeup();
            if self.sources[i].next_poll_at > now && !rung {
                continue;
            }
            self.sources[i].next_poll_at = now + self.sources[i].poll_interval;

            let (row_id, user_id) = (self.sources[i].row_id, self.sources[i].user_id);

            if matches!(self.sources[i].kind.as_str(), "jellyfin" | "emby") {
                self.poll_media_browser(i, &db, &mb_client, &lb_url, &now_playing).await;
                continue;
            }
            if self.sources[i].kind == "navidrome" {
                self.poll_navidrome(i, &db, &mb_client, &lb_url, &now_playing).await;
                continue;
            }
            if self.sources[i].kind == "roon" {
                self.poll_roon(i, &db, &mb_client, &lb_url, &now_playing).await;
                continue;
            }

            let Some(plex) = self.sources[i].source.as_any_mut().downcast_mut::<PlexSource>()
            else {
                continue;
            };

            let session_result = match plex.fetch_sessions_extended(None).await {
                Ok(r) => {
                    if self.sources[i].error_logged {
                        info!("✅ Plex recovered for user {}", user_id);
                        self.sources[i].error_logged = false;
                    }
                    let _ = db.record_source_poll(row_id, None).await;
                    r
                }
                Err(e) => {
                    // Logged once per outage, then quietly, so a server that is
                    // off overnight doesn't produce hundreds of identical lines.
                    let msg = e.to_string();
                    if !self.sources[i].error_logged {
                        error!("Error fetching Plex for user {}: {}", user_id, msg);
                        self.sources[i].error_logged = true;
                    } else {
                        debug!("Plex still failing for user {}: {}", user_id, msg);
                    }
                    let _ = db.record_source_poll(row_id, Some(&msg)).await;
                    continue;
                }
            };

            if session_result.now_playing.is_empty() {
                // Authoritative "stopped" for this user's Plex — but only for
                // Plex, so an ingest client reporting for the same user survives.
                now_playing.clear_user_source(user_id, "Plex");
            } else {
                // Debug, not info: a track is now reported every tick for its
                // whole length, so this fires once per poll while music plays.
                debug!(
                    "🎧 {} active Plex session(s) for user {}",
                    session_result.now_playing.len(),
                    user_id
                );
            }

            for plex_track in &session_result.now_playing {
                let mut play = plex_track.to_play("np");

                // The owning user comes from the source row, not from matching
                // the Plex account name against Tapedeck usernames.
                let progress = plex_track.view_offset.unwrap_or(0) / 1000;
                // Deck first. Enrichment is a rate-limited MusicBrainz round
                // trip that the registry has no use for — only the sink payload
                // does — so doing it first put a network wait between the user
                // pressing play and the deck showing anything.
                let mut np = NowPlaying::from_play(
                    &play,
                    "Plex",
                    plex_track.audio_quality.as_ref(),
                    Progress::Reported(progress),
                );
                // See the note on the Jellyfin/Emby writer: the live entry
                // carries the chain its eventual listen will get, resolved
                // read-only, so Patch can apply one sharing rule rather than
                // two.
                np.chain_id = match &plex_track.player {
                    Some(p) => db.device_chain_by_key(user_id, &p.machine_id).await.ok().flatten(),
                    None => None,
                };
                let changed = now_playing.set(user_id, np);

                if changed {
                    enrich_play(&mb_client, &mut play, plex_track.album.as_deref()).await;
                    let user_sinks =
                        sinks::sinks_for_user(&db, user_id, &lb_url).await.unwrap_or_default();
                    sinks::submit_now_playing(&user_sinks, &play).await;
                }
            }

            if session_result.ready_to_scrobble.is_empty() {
                continue;
            }
            info!(
                "📀 Processing {} ready-to-scrobble track(s) for user {}",
                session_result.ready_to_scrobble.len(),
                user_id
            );

            let current_time = now_secs() as u64;
            for plex_track in session_result.ready_to_scrobble {
                let mut play = plex_track.to_play(&format!("scrobble-{}", current_time));

                // Dedup before enrichment, so a duplicate doesn't cost a
                // MusicBrainz call.
                let already_exists = match already_banked(&db, user_id, &play, false).await {
                    Ok(d) => d,
                    Err(e) => {
                        warn!(
                            "Could not check for a duplicate ({}) — leaving it for the next tick",
                            e
                        );
                        continue;
                    }
                };
                if already_exists {
                    debug!("Skipping already-scrobbled: {} - {}", play.artist, play.title);
                    continue;
                }

                enrich_play(&mb_client, &mut play, plex_track.album.as_deref()).await;

                let quality_ref = plex_track.audio_quality.as_ref();
                let quality_log = quality_ref
                    .and_then(|q| q.quality_score)
                    .map(|s| format!("{:.0}", s))
                    .unwrap_or_else(|| "n/a".into());

                // Register the player and let it carry the chain. A Plex listen
                // has no token, so the ladder's first three rungs can never
                // match; without a device it fell through all four and every
                // Plex listen was stored with no signal chain at all. Binding a
                // chain to "Plexamp on fedora" in the UI now attributes every
                // listen from it, with nothing to configure per listen.
                let (device_id, chain_id) = match &plex_track.player {
                    Some(p) => {
                        match db
                            .upsert_device(
                                user_id,
                                &p.machine_id,
                                p.name.as_deref(),
                                p.platform.as_deref(),
                                p.product.as_deref(),
                            )
                            .await
                        {
                            Ok(dev) => {
                                let chain =
                                    db.get_device_default_chain(user_id, dev).await.ok().flatten();
                                (Some(dev), chain)
                            }
                            Err(e) => {
                                warn!("Could not record Plex player as a device: {}", e);
                                (None, None)
                            }
                        }
                    }
                    // History rows have no player — nothing is playing by then.
                    None => (None, None),
                };

                let listening_context = match chain_id {
                    Some(cid) => db
                        .get_chain(user_id, cid)
                        .await
                        .ok()
                        .flatten()
                        .map(|c| c.listening_context.as_str().to_string()),
                    None => None,
                };

                match db
                    .save_scrobble(
                        user_id,
                        &play,
                        quality_ref,
                        device_id,
                        chain_id,
                        None,
                        listening_context.as_deref(),
                        Some("Plex"),
                        Some(row_id),
                        ListenDisposition::LIVE,
                    )
                    .await
                {
                    Ok(true) => info!(
                        "📥 Queued for user {}: {} - {} [quality: {}]",
                        user_id, play.artist, play.title, quality_log
                    ),
                    Ok(false) => {}
                    Err(e) => error!("Database error: {}", e),
                }
            }

            // Tracks the listener moved on from before they ever counted.
            // Plex keeps its own copy of the session state machine, so this
            // mirrors the Jellyfin and Roon loops rather than sharing one.
            for plex_track in session_result.skipped {
                let listened_secs = plex_track.view_offset.unwrap_or(0) / 1000;
                // The suffix carries the time, or skipping the same track
                // twice would produce one `plex-<rk>-skip` both times and
                // `exists` would drop the second permanently — the constant
                // `source_id` bug Jellyfin repeats had before v0.30.0.
                let play = plex_track.to_play(&format!("skip-{}", now_secs()));
                let (device_id, chain_id) = match &plex_track.player {
                    Some(p) => match db
                        .upsert_device(
                            user_id,
                            &p.machine_id,
                            p.name.as_deref(),
                            p.platform.as_deref(),
                            p.product.as_deref(),
                        )
                        .await
                    {
                        Ok(dev) => {
                            let chain =
                                db.get_device_default_chain(user_id, dev).await.ok().flatten();
                            (Some(dev), chain)
                        }
                        Err(_) => (None, None),
                    },
                    None => (None, None),
                };
                let listening_context = match chain_id {
                    Some(cid) => db
                        .get_chain(user_id, cid)
                        .await
                        .ok()
                        .flatten()
                        .map(|c| c.listening_context.as_str().to_string()),
                    None => None,
                };
                bank_skip(
                    &db,
                    user_id,
                    &play,
                    Some(listened_secs),
                    device_id,
                    chain_id,
                    listening_context.as_deref(),
                    "Plex",
                    row_id,
                )
                .await;
            }
        }

        // Deliberately not gated on "is anything playing". Now that a track is
        // reported for its whole length rather than only its first half, that
        // gate would hold every pending scrobble until the music stopped.
        // Forwarding a listen while the same track is still on the deck is what
        // every scrobble client does, and `delivered_sinks` stops a retry from
        // sending it twice.
        self.flush_pending().await;

        // Tracks that left a pushing client's deck with nothing banked for
        // them. Polled sources report their own skips directly from the session
        // state machine; this is only for clients that submit.
        self.resolve_pending_skips().await;

        // Likewise on its own clock — see THRESHOLD_REFRESH_SECS.
        if now >= self.next_threshold_refresh_at {
            self.next_threshold_refresh_at = now_secs() + THRESHOLD_REFRESH_SECS;
            self.apply_scrobble_thresholds().await;
        }

        // On their own clock, not the tick's. The tick is short so the deck is
        // live; these are rate-limited lookups whose budgets were sized for a
        // slow tick, and letting them ride it would just aim more traffic at
        // MusicBrainz for no gain.
        if now >= self.next_backfill_at {
            self.next_backfill_at = now_secs() + BACKFILL_INTERVAL_SECS;
            self.run_backfills().await;
        }
    }

    /// One round of the incremental metadata backfills, bounded.
    ///
    /// **Bounded, and rotated.** The bound is [`Self::backfill_budget`] and the
    /// reason for it is written there. The rotation is the half that is easy to
    /// leave out and wrong to: with a fixed order and a budget, the passes at
    /// the front eat it and the ones at the back — artwork and artist
    /// portraits, which are also the two whose absence is most visible — would
    /// never run again on any instance with a real backlog. Starting each round
    /// where the last one stopped means every pass gets its turn.
    ///
    /// That does relax "recording MBIDs first, because everything below keys
    /// off what it produces". It is the right trade: that ordering is about the
    /// long run, and these run every minute forever, so what matters is that
    /// each pass gets time at all — which is precisely what a fixed order under
    /// a budget takes away.
    ///
    /// The breaker does the rest. When MusicBrainz is unreachable its passes
    /// now fail instantly instead of sleeping a second per lookup, so they stop
    /// consuming the budget and the passes that do not need it — artwork via
    /// Deezer, portraits from the user's own library — keep running through the
    /// outage. Before, they were the first thing to be starved by it.
    async fn run_backfills(&mut self) {
        let deadline = Instant::now() + self.backfill_budget();

        // Named so a skipped round can say what it skipped. The order is the
        // one they were written in; `backfill_cursor` decides where it starts.
        const PASSES: usize = 8;
        let mut ran = 0usize;

        for step in 0..PASSES {
            if Instant::now() >= deadline {
                break;
            }
            let which = (self.backfill_cursor + step) % PASSES;
            ran += 1;

            // **Around each pass, not only between them.** A deadline checked
            // between passes bounds how many *start*, not how long one takes —
            // and one pass is eight lookups, each of which can sit for the
            // client's own timeout. Measured against an upstream answering in
            // three seconds: a round meant to stop at ten ran to 28.6, all of
            // it inside a single pass. This is the same correction
            // `flush_pending` already applies around `enrich_play`, for the
            // same reason.
            //
            // Cancelling here is safe: every pass writes one row at a time, so
            // being dropped at an await point loses at most the item in flight
            // and never half of one. It comes back round next time.
            let left = deadline.saturating_duration_since(Instant::now());
            let pass = async {
                match which {
                    // Incrementally classify listen languages (script instantly, a
                    // few MusicBrainz lookups per pass to respect the rate limit).
                    0 => {
                        crate::enrich::backfill_languages(&self.db, &self.mb_client, 50, 8).await;
                    }

                    // Same deal for release years (feeds the by-decade breakdown) —
                    // cache hits are free, so the budget only bounds real API calls.
                    1 => {
                        crate::enrich::backfill_release_years(&self.db, &self.mb_client, 50, 8)
                            .await;
                    }

                    // A recording MBID is otherwise only ever written by
                    // `flush_pending`, which sees `status = 'pending'` listens alone
                    // — so an imported listen never got one, and the ISRC and
                    // work-language passes were structurally excluded from most of a
                    // mostly-imported history. Given its own budget rather than
                    // sharing, because it is the pass that unblocks the others.
                    2 => {
                        crate::enrich::backfill_recording_mbids(&self.db, &self.mb_client, 25, 8)
                            .await;
                    }

                    // Genres come off the same `fetch_metadata` call the two above
                    // use, so on an already-enriched history this resolves entirely
                    // out of the cache and spends none of its budget.
                    3 => {
                        crate::enrich::backfill_genres(&self.db, &self.mb_client, 50, 8).await;
                    }

                    // And ISRCs, off that same call. It is the only identifier a
                    // playlist push can hand a streaming service instead of a name,
                    // so it is worth having before somebody needs it rather than
                    // fetching a hundred of them while they wait.
                    4 => {
                        crate::enrich::backfill_isrc(&self.db, &self.mb_client, 50, 8).await;
                    }

                    // Lyrics, for the language a transliterated title cannot give.
                    // A small budget: this is one request per *track* rather than
                    // per listen, and LRCLIB is a free service that asks callers to
                    // be considerate — the docs' words, and it has no API key to
                    // ration with, so the rationing is ours to do.
                    5 => {
                        if let Some(lyrics) = self.lyrics.clone() {
                            crate::enrich::backfill_lyrics(&self.db, &lyrics, 8).await;
                        }
                    }

                    6 => {
                        crate::enrich::backfill_artwork(
                            &self.db,
                            &self.mb_client,
                            &self.artwork_providers,
                            50,
                            8,
                        )
                        .await;
                    }

                    _ => self.backfill_artist_images().await,
                }
            };
            if tokio::time::timeout(left, pass).await.is_err() {
                break;
            }
        }

        self.backfill_cursor = advance_backfill_cursor(self.backfill_cursor, ran, PASSES);

        if ran < PASSES {
            debug!(
                "Backfills used their {}s budget after {} of {} passes — the rest run next round",
                self.backfill_budget().as_secs(),
                ran,
                PASSES
            );
        }
    }

    /// Fill in artist portraits from the users' own libraries.
    ///
    /// A separate pass because portraits used to arrive only as a side effect
    /// of the artwork chain — Deezer hands one back alongside a cover — so an
    /// artist whose covers were already resolved never got a picture at all.
    /// That is why a well-played artist could sit on a lettered tile forever.
    ///
    /// Deliberately small per pass. Each miss is a search against someone's own
    /// media server, and the row is only claimed on a *hit*: a miss stays NULL
    /// and is retried later, because the answer changes when they add the
    /// artist to their library.
    async fn backfill_artist_images(&mut self) {
        const PER_PASS: i64 = 5;

        let users = match self.db.list_users().await {
            Ok(u) => u,
            Err(e) => {
                warn!("Artist-portrait backfill could not list users: {}", e);
                return;
            }
        };
        for user in users {
            let names = match self.db.artists_without_images(user.id, PER_PASS).await {
                Ok(n) if !n.is_empty() => n,
                Ok(_) => continue,
                Err(e) => {
                    warn!("Artist-portrait backfill query failed: {}", e);
                    continue;
                }
            };
            debug!("🖼️ Looking for portraits: {:?}", names);
            for name in names {
                let Ok(Some(id)) = self.db.artist_row_id(&name).await else { continue };
                // The stored value is a path to our own proxy rather than the
                // library URL: the browser cannot reach a LAN server, and the
                // credentials that could must not leave the backend.
                if self.artist_is_in_a_library(user.id, &name).await {
                    let path = format!("/api/v1/artists/img/{id}");
                    if self.db.set_artist_image(id, Some(&path)).await.is_ok() {
                        debug!("🖼️ Portrait for {} from the library", name);
                    }
                }
            }
        }
    }

    /// Is this artist in any of the user's connected libraries?
    async fn artist_is_in_a_library(&self, user_id: i64, name: &str) -> bool {
        let Ok(sources) = self.db.list_sources_for_user(user_id).await else { return false };
        for source in sources.iter().filter(|s| s.enabled) {
            match source.kind.as_str() {
                "plex" => {
                    let Some(token) = source.credential("token") else { continue };
                    let plex =
                        PlexSource::with_filters(source.url.clone(), token, PlexFilters::default());
                    if matches!(plex.find_artist(name).await, Ok(Some(_))) {
                        return true;
                    }
                }
                "jellyfin" => {
                    let Some(key) = source.credential("api_key") else { continue };
                    let jf = MediaBrowserSource::jellyfin(source.url.clone(), key, String::new());
                    if matches!(jf.find_artist(name).await, Ok(Some(_))) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// Forward everything still `pending`, **bounded against the tick**.
    ///
    /// This is awaited inline at the tail of `run_tick`, so however long it
    /// takes is how late every source poll and every deck update is. That was
    /// only theoretical until 2026-08-24, when one unreachable upstream turned
    /// a 5-second tick into 45–92 minutes and the deck went dark for a day.
    ///
    /// So it now stops at [`Self::flush_budget`] and leaves the rest
    /// `pending` for the next tick. Nothing is lost by stopping early: that is
    /// the queue's normal state, and `delivered_sinks` already means a retry
    /// cannot double-send a listen a partial flush did deliver. Being late is
    /// recoverable; being stopped is not.
    async fn flush_pending(&mut self) {
        let deadline = Instant::now() + self.flush_budget();
        let pending = match self.db.get_pending_scrobbles().await {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to fetch pending scrobbles: {}", e);
                return;
            }
        };
        if pending.is_empty() {
            return;
        }
        info!("🔄 Processing {} pending scrobble(s)", pending.len());

        // Group by user so each user's listens forward to *their* connected
        // services — and only those. No server-wide fallback: a user who has
        // connected nothing keeps their listens local.
        let mut by_user: HashMap<i64, Vec<crate::db::PendingScrobble>> = HashMap::new();
        for ps in pending {
            by_user.entry(ps.user_id).or_default().push(ps);
        }

        let mut ran_out = 0usize;
        for (uid, items) in by_user {
            if Instant::now() >= deadline {
                ran_out += items.len();
                continue;
            }
            // Rebuilt each flush (fresh reqwest client) — fine at self-host
            // volume; cache per user if throughput ever matters.
            //
            // A failed read here is **not** an empty connection list, and the
            // difference is the whole reason this returns a `Result`: `enabled`
            // would come back empty, the all-delivered test below would be
            // vacuously true, and the listen would be marked `synced` having
            // been sent nowhere — permanently, and logged as a success. Skip
            // the user; the row stays `pending` and next tick tries again.
            let targets = match sinks::sinks_for_user(&self.db, uid, &self.listenbrainz_url).await {
                Ok(t) => t,
                Err(e) => {
                    error!(
                        "Could not read connections for user {} ({}) — leaving {} listen(s) \
                         pending rather than marking them sent",
                        uid,
                        e,
                        items.len()
                    );
                    continue;
                }
            };
            let enabled: Vec<String> = targets.iter().map(|s| s.name().to_string()).collect();

            for mut ps in items {
                if Instant::now() >= deadline {
                    ran_out += 1;
                    continue;
                }
                if ps.play.mbid_recording.is_none() {
                    let album_hint = ps.play.album.clone();
                    // **Bounded, because enrichment is opportunistic and the
                    // sink call after it is not.**
                    //
                    // The deadline above limits how many listens are *started*,
                    // not how long one takes — and a rate-limited MusicBrainz
                    // lookup with a 30s client timeout is easily the longest
                    // thing in this loop. Measured against a dead sink: a tick
                    // that should have been bounded at 10s ran to 41, and all
                    // of the excess was one lookup.
                    //
                    // It is also the exact compounding that made 2026-08-24
                    // worse than it had to be: MusicBrainz and ListenBrainz are
                    // both MetaBrainz, both were unreachable, and every pending
                    // listen paid for each of them in turn.
                    //
                    // Giving up costs nothing — the listen forwards perfectly
                    // well without an MBID, and `backfill_recording_mbids`
                    // exists to fill it in later, off the tick.
                    let enriched = tokio::time::timeout(
                        ENRICH_BUDGET.min(deadline.saturating_duration_since(Instant::now())),
                        enrich_play(&self.mb_client, &mut ps.play, album_hint.as_deref()),
                    )
                    .await;
                    if enriched.is_err() {
                        debug!(
                            "Enrichment for {} - {} outran the flush budget — forwarding \
                             unenriched and leaving it to the backfill",
                            ps.play.artist, ps.play.title
                        );
                    }
                    // Persist the resolved MBIDs so language/analytics can use them.
                    if ps.play.mbid_recording.is_some() {
                        if let Err(e) = self.db.update_scrobble_enrichment(ps.id, &ps.play).await {
                            error!("Failed to persist enrichment for scrobble {}: {}", ps.id, e);
                        }
                    }
                }

                // Only send to sinks that haven't already accepted this listen, so
                // a retry after a partial failure can't double-scrobble.
                for sink in targets.iter() {
                    let name = sink.name().to_string();
                    if ps.delivered.contains(&name) {
                        continue;
                    }
                    // Through the breaker, so a service already known to be
                    // down costs nothing rather than a full retry budget per
                    // listen — the 978-listens-×-66s arithmetic that took the
                    // engine out. A skipped sink returns `Err`, which is right:
                    // nothing was delivered, so the row stays pending.
                    match sinks::guarded(&name, sink.scrobble(&[ps.play.clone()])).await {
                        Ok(_) => {
                            ps.delivered.push(name.clone());
                            if let Err(e) = self.db.mark_sink_delivered(ps.id, &name).await {
                                error!("Failed to record delivery to {}: {}", name, e);
                            }
                        }
                        Err(e) => debug!("Did not send to {}: {}", name, e),
                    }
                }

                // Fully synced once every target sink has accepted it (trivially
                // true when there are no sinks — store-locally-only mode).
                if enabled.iter().all(|n| ps.delivered.contains(n)) {
                    if let Err(e) = self.db.mark_scrobble_synced(ps.id).await {
                        error!("Failed to mark synced: {}", e);
                    } else {
                        info!("✅ Synced: {} - {}", ps.play.artist, ps.play.title);
                    }
                }
            }
        }

        if ran_out > 0 {
            crate::engine::health::deferred(ran_out);
            info!(
                "⏳ Flush hit its {}s budget — {} listen(s) still pending, picked up next tick",
                self.flush_budget().as_secs(),
                ran_out
            );
        }
    }

    /// How long the flush may hold the tick.
    ///
    /// Twice the poll interval: long enough that a healthy flush is never cut
    /// short (a successful submission is one request), short enough that a sick
    /// one delays the deck by seconds rather than by hours. Floored so a very
    /// fast tick cannot starve forwarding entirely.
    fn flush_budget(&self) -> Duration {
        (self.poll_interval * 2).max(Duration::from_secs(10))
    }

    /// How long a round of metadata backfills may hold the tick.
    ///
    /// **This is the 2026-08-24 rule arriving where it was missed.** That
    /// release bounded `flush_pending` because a dead sink was stopping the
    /// engine, and left the eight passes immediately above it awaited inline
    /// with no bound at all. On 2026-08-27 that cost the same outage a second
    /// time: Plex polling fell from 1,440 reads an hour to 45 — a tick every
    /// four minutes instead of every five seconds — with Plex itself perfectly
    /// healthy the whole time.
    ///
    /// The floor of the *healthy* case is the surprise, and it is why no fixed
    /// number works: the passes budget about 89 lookups between them, and the
    /// MusicBrainz rate limiter sleeps a second per lookup **whether or not it
    /// succeeds**. So a round with real work takes ninety seconds when nothing
    /// is wrong. The tick is five. There was never a version of this that fit.
    ///
    /// Same shape as [`Self::flush_budget`] and the same reasoning, so the
    /// whole tick is bounded at roughly four times its own interval however
    /// badly a third party is behaving. Deliberately **not** generous: these
    /// are a background trickle, and the latency that matters is at *track
    /// start*, which is exactly what a long round delays. A real backlog is
    /// not this loop's job — Settings → Maintenance has a button that walks it
    /// off the tick, which is where the throughput belongs.
    fn backfill_budget(&self) -> Duration {
        (self.poll_interval * 2).max(Duration::from_secs(10))
    }
}

#[cfg(test)]
mod backfill_rotation_tests {
    use super::advance_backfill_cursor;

    /// **Every pass gets its turn, however few fit in a round.**
    ///
    /// The bound on the backfill round is what stops a slow upstream holding
    /// the tick, and rotation is what stops that bound turning into a
    /// permanent starvation: with a fixed order, the passes at the front eat
    /// the budget and artwork and artist portraits — the two whose absence is
    /// most visible on screen — would never run again on any instance with a
    /// real backlog.
    ///
    /// Asserted as *coverage over rounds* rather than on the arithmetic,
    /// because that is the property; an off-by-one in the modulo is a pass that
    /// silently never runs, not an error anybody sees.
    #[test]
    fn a_starved_round_still_reaches_every_pass_eventually() {
        const PASSES: usize = 8;
        for fits in 1..=PASSES {
            let mut cursor = 0usize;
            let mut seen = [false; PASSES];
            // Enough rounds that a fair rotation must have covered everything.
            for _ in 0..PASSES {
                for step in 0..fits {
                    seen[(cursor + step) % PASSES] = true;
                }
                cursor = advance_backfill_cursor(cursor, fits, PASSES);
            }
            assert!(
                seen.iter().all(|s| *s),
                "with only {fits} of {PASSES} passes per round, some pass never ran: {seen:?}"
            );
        }
    }

    /// A round that ran nothing must not advance past anything. Otherwise a
    /// spell of the tick being busy would skip passes that were never even
    /// attempted — losing work precisely when the instance is least healthy.
    #[test]
    fn a_round_that_ran_nothing_does_not_skip_a_pass() {
        assert_eq!(advance_backfill_cursor(3, 0, 8), 3);
    }

    /// A full round comes back to where it started, so the fixed order is what
    /// a healthy instance actually sees — rotation is a response to starvation,
    /// not a reshuffle for its own sake.
    #[test]
    fn a_complete_round_leaves_the_order_alone() {
        assert_eq!(advance_backfill_cursor(0, 8, 8), 0);
        assert_eq!(advance_backfill_cursor(5, 8, 8), 5);
    }
}
