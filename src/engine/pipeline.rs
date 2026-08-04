use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::db::Database;
use crate::musicbrainz::MusicBrainzClient;
use crate::server::now_playing::{NowPlaying, NowPlayingRegistry};
use crate::sinks;
use crate::sources::{JellyfinSource, MusicSource, NavidromeSource, PlexFilters, PlexSource};

use super::enrichment::enrich_play;

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
    /// ListenBrainz endpoint (self-hosted instances override the default).
    listenbrainz_url: String,
    /// Shared with the HTTP layer so the dashboard deck reflects Plex sessions.
    now_playing: NowPlayingRegistry,
    /// Artwork lookup chain, cheapest first. Built once — each provider holds a
    /// pooled HTTP client.
    artwork_providers: Vec<Box<dyn crate::metadata::ArtworkProvider>>,
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
        "plex" | "jellyfin" => 5,
        _ => 30,
    }
}

/// How often the MusicBrainz backfills and the artwork chain get a turn.
/// Deliberately decoupled from the tick: the tick is fast so the deck is fast,
/// and tripling the rate of rate-limited lookups is exactly what that must not
/// cost.
const BACKFILL_INTERVAL_SECS: i64 = 60;

/// `user_sources.kind` → the label the now-playing registry stores. They differ
/// because the registry's label is user-visible and predates the kind column.
fn source_label(kind: &str) -> &'static str {
    match kind {
        "plex" => "Plex",
        "navidrome" => "Navidrome",
        "jellyfin" => "Jellyfin",
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
            now_playing,
            listenbrainz_url,
            artwork_providers: vec![Box::new(crate::metadata::deezer::DeezerProvider::new())],
        }
    }

    /// Bring the live source registry in line with `user_sources`.
    ///
    /// Runs at the top of every tick, so a user's save applies within a tick
    /// instead of at the next restart, and a server that was unreachable when we
    /// first tried gets picked up when it comes back. One code path for startup
    /// and for later edits.
    async fn reconcile_sources(&mut self) {
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
            "jellyfin" => {
                let key = cfg.credential("api_key").ok_or("no Jellyfin API key stored")?;
                let account = cfg
                    .remote_account
                    .clone()
                    .ok_or("Jellyfin account not resolved — reconnect the source")?;
                let mut jf = JellyfinSource::new(cfg.url.clone(), key, account);
                jf.initialize().await?;
                Ok(Box::new(jf))
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
        loop {
            if *shutdown.borrow() {
                break;
            }

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

            // A completed tick reached a clean boundary — stop now if asked.
            if *shutdown.borrow() {
                break;
            }

            tokio::select! {
                _ = sleep(self.poll_interval) => {}
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
                    let changed = now_playing
                        .set(user_id, NowPlaying::from_play(&play, "Navidrome", None, 0));
                    if changed {
                        let album = play.album.clone();
                        enrich_play(mb_client, &mut play, album.as_deref()).await;
                        let sinks = sinks::sinks_for_user(db, user_id, lb_url).await;
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
                    let dupe = db
                        .exists(user_id, &play.source_id, &play.source_name)
                        .await
                        .unwrap_or(false)
                        || db.fuzzy_exists(user_id, &play).await.unwrap_or(false);
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
                            Some("Navidrome"),
                            Some(row_id),
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

    /// Poll one Jellyfin source. Session-shaped like Plex — there is no history
    /// endpoint to backfill from, so everything comes from what's playing now.
    #[allow(clippy::too_many_arguments)]
    async fn poll_jellyfin(
        &mut self,
        i: usize,
        db: &Arc<Database>,
        mb_client: &Arc<MusicBrainzClient>,
        lb_url: &str,
        now_playing: &NowPlayingRegistry,
    ) {
        let (row_id, user_id) = (self.sources[i].row_id, self.sources[i].user_id);
        let Some(jf) = self.sources[i].source.as_any_mut().downcast_mut::<JellyfinSource>() else {
            return;
        };

        let sessions = match jf.fetch_sessions().await {
            Ok(s) => {
                if self.sources[i].error_logged {
                    info!("✅ Jellyfin recovered for user {}", user_id);
                    self.sources[i].error_logged = false;
                }
                let _ = db.record_source_poll(row_id, None).await;
                s
            }
            Err(e) => {
                let msg = e.to_string();
                if !self.sources[i].error_logged {
                    error!("Jellyfin fetch failed for user {}: {}", user_id, msg);
                    self.sources[i].error_logged = true;
                } else {
                    debug!("Jellyfin still failing for user {}: {}", user_id, msg);
                }
                let _ = db.record_source_poll(row_id, Some(&msg)).await;
                return;
            }
        };

        if sessions.now_playing.is_empty() {
            now_playing.clear_user_source(user_id, "Jellyfin");
        }

        for track in &sessions.now_playing {
            let mut play = track.play.clone();
            let changed = now_playing.set(
                user_id,
                NowPlaying::from_play(&play, "Jellyfin", track.quality.as_ref(), track.position),
            );
            if changed {
                if play.mbid_recording.is_none() {
                    let album = play.album.clone();
                    enrich_play(mb_client, &mut play, album.as_deref()).await;
                }
                let sinks = sinks::sinks_for_user(db, user_id, lb_url).await;
                sinks::submit_now_playing(&sinks, &play).await;
            }
        }

        for track in sessions.ready_to_scrobble {
            let mut play = track.play;
            let dupe =
                db.exists(user_id, &play.source_id, &play.source_name).await.unwrap_or(false)
                    || db.fuzzy_exists(user_id, &play).await.unwrap_or(false);
            if dupe {
                continue;
            }
            if play.mbid_recording.is_none() {
                let album = play.album.clone();
                enrich_play(mb_client, &mut play, album.as_deref()).await;
            }
            match db
                .save_scrobble(
                    user_id,
                    &play,
                    track.quality.as_ref(),
                    None,
                    None,
                    None,
                    Some("Jellyfin"),
                    Some(row_id),
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
    }

    pub async fn run_tick(&mut self) {
        self.reconcile_sources().await;

        let db = self.db.clone();
        let mb_client = self.mb_client.clone();
        let lb_url = self.listenbrainz_url.clone();
        let now_playing = self.now_playing.clone();
        let now = now_secs();

        for i in 0..self.sources.len() {
            if self.sources[i].next_poll_at > now {
                continue;
            }
            self.sources[i].next_poll_at = now + self.sources[i].poll_interval;

            let (row_id, user_id) = (self.sources[i].row_id, self.sources[i].user_id);

            if self.sources[i].kind == "jellyfin" {
                self.poll_jellyfin(i, &db, &mb_client, &lb_url, &now_playing).await;
                continue;
            }
            if self.sources[i].kind == "navidrome" {
                self.poll_navidrome(i, &db, &mb_client, &lb_url, &now_playing).await;
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
                let changed = now_playing.set(
                    user_id,
                    NowPlaying::from_play(
                        &play,
                        "Plex",
                        plex_track.audio_quality.as_ref(),
                        progress,
                    ),
                );

                if changed {
                    enrich_play(&mb_client, &mut play, plex_track.album.as_deref()).await;
                    let user_sinks = sinks::sinks_for_user(&db, user_id, &lb_url).await;
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
                let already_exists =
                    db.exists(user_id, &play.source_id, &play.source_name).await.unwrap_or(false)
                        || db.fuzzy_exists(user_id, &play).await.unwrap_or(false);
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
                        listening_context.as_deref(),
                        Some("Plex"),
                        Some(row_id),
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
        }

        // Deliberately not gated on "is anything playing". Now that a track is
        // reported for its whole length rather than only its first half, that
        // gate would hold every pending scrobble until the music stopped.
        // Forwarding a listen while the same track is still on the deck is what
        // every scrobble client does, and `delivered_sinks` stops a retry from
        // sending it twice.
        self.flush_pending().await;

        // On their own clock, not the tick's. The tick is short so the deck is
        // live; these are rate-limited lookups whose budgets were sized for a
        // slow tick, and letting them ride it would just aim more traffic at
        // MusicBrainz for no gain.
        if now >= self.next_backfill_at {
            self.next_backfill_at = now_secs() + BACKFILL_INTERVAL_SECS;

            // Incrementally classify listen languages (script instantly, a few
            // MusicBrainz lookups per pass to respect the rate limit).
            crate::enrich::backfill_languages(&self.db, &self.mb_client, 50, 8).await;

            // Same deal for release years (feeds the by-decade breakdown) —
            // cache hits are free, so the budget only bounds real API calls.
            crate::enrich::backfill_release_years(&self.db, &self.mb_client, 50, 8).await;
            crate::enrich::backfill_artwork(
                &self.db,
                &self.mb_client,
                &self.artwork_providers,
                50,
                8,
            )
            .await;
        }
    }

    async fn flush_pending(&mut self) {
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

        for (uid, items) in by_user {
            // Rebuilt each flush (fresh reqwest client) — fine at self-host
            // volume; cache per user if throughput ever matters.
            let targets = sinks::sinks_for_user(&self.db, uid, &self.listenbrainz_url).await;
            let enabled: Vec<String> = targets.iter().map(|s| s.name().to_string()).collect();

            for mut ps in items {
                if ps.play.mbid_recording.is_none() {
                    let album_hint = ps.play.album.clone();
                    enrich_play(&self.mb_client, &mut ps.play, album_hint.as_deref()).await;
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
                    match sink.scrobble(&[ps.play.clone()]).await {
                        Ok(_) => {
                            ps.delivered.push(name.clone());
                            if let Err(e) = self.db.mark_sink_delivered(ps.id, &name).await {
                                error!("Failed to record delivery to {}: {}", name, e);
                            }
                        }
                        Err(e) => error!("Failed to send to {}: {}", name, e),
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
    }
}
