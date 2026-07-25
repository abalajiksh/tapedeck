use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info};

use crate::db::Database;
use crate::musicbrainz::MusicBrainzClient;
use crate::server::now_playing::{NowPlaying, NowPlayingRegistry};
use crate::sinks;
use crate::sources::{MusicSource, PlexFilters, PlexSource};

use super::enrichment::enrich_play;

pub struct ScrobbleEngine {
    sources: Vec<Box<dyn MusicSource>>,
    db: Arc<Database>,
    mb_client: Arc<MusicBrainzClient>,
    poll_interval: Duration,
    default_user_id: i64,
    /// ListenBrainz endpoint (self-hosted instances override the default).
    listenbrainz_url: String,
    /// Shared with the HTTP layer so the dashboard deck reflects Plex sessions.
    now_playing: NowPlayingRegistry,
    /// `plex_settings.updated_at` of the config the current source was built
    /// from. A newer stamp means an admin edited it and we rebuild.
    plex_config_stamp: i64,
    /// Set once we've reported an unreachable Plex, so the per-tick retry
    /// doesn't repeat the same error every poll interval.
    plex_unreachable_logged: bool,
}

impl ScrobbleEngine {
    pub fn new(
        sources: Vec<Box<dyn MusicSource>>,
        db: Arc<Database>,
        mb_client: Arc<MusicBrainzClient>,
        now_playing: NowPlayingRegistry,
        listenbrainz_url: String,
    ) -> Self {
        Self {
            sources, db, mb_client,
            poll_interval: Duration::from_secs(15),
            default_user_id: 1,
            now_playing,
            listenbrainz_url,
            plex_config_stamp: 0,
            plex_unreachable_logged: false,
        }
    }

    /// Bring the Plex source in line with the stored settings. Runs every tick,
    /// so an admin's save applies within one poll interval instead of at the
    /// next restart — and a server that was unreachable when we first tried
    /// (still booting, network not up yet) gets picked up when it comes back.
    async fn reconcile_plex_source(&mut self) {
        let settings = match self.db.get_plex_settings().await {
            Ok(Some(s)) => s,
            Ok(None) => return, // never configured; nothing to reconcile
            Err(e) => { error!("Failed to read Plex settings: {}", e); return; }
        };

        let config_changed = settings.updated_at != self.plex_config_stamp;

        if config_changed {
            self.plex_config_stamp = settings.updated_at;
            self.plex_unreachable_logged = false; // new config: report failures loudly again
            // Drop the old source either way — a disabled or reconfigured Plex
            // must not keep polling with stale credentials.
            self.sources.retain(|s| s.name() != "Plex");
            self.now_playing.clear_source("Plex");
        }

        if !settings.is_usable() {
            if config_changed { info!("⚙️ Plex source disabled"); }
            return;
        }

        // Already running on the current config — nothing to do. Otherwise we
        // either just applied a change or a previous build failed; retry.
        if self.sources.iter().any(|s| s.name() == "Plex") {
            return;
        }

        let filters = PlexFilters {
            users_allow: crate::db::PlexSettings::list(&settings.users_allow),
            users_block: crate::db::PlexSettings::list(&settings.users_block),
            devices_allow: crate::db::PlexSettings::list(&settings.devices_allow),
            devices_block: crate::db::PlexSettings::list(&settings.devices_block),
            libraries_allow: crate::db::PlexSettings::list(&settings.libraries_allow),
            libraries_block: crate::db::PlexSettings::list(&settings.libraries_block),
        };
        let mut plex = PlexSource::with_filters(settings.url.clone(), settings.token.clone(), filters);
        match plex.initialize().await {
            Ok(()) => {
                info!("✅ Plex source connected ({})", settings.url);
                self.plex_unreachable_logged = false;
                self.sources.push(Box::new(plex));
            }
            Err(e) => {
                // Retried every tick, so log the first failure and then stay
                // quiet rather than filling the log while a server is down.
                if !self.plex_unreachable_logged {
                    error!("❌ Plex unreachable ({}): {} — retrying every {}s",
                        settings.url, e, self.poll_interval.as_secs());
                    self.plex_unreachable_logged = true;
                } else {
                    debug!("Plex still unreachable: {}", e);
                }
            }
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
            if *shutdown.borrow() { break; }

            // Isolate each tick: a panic here would otherwise kill the engine
            // task while the HTTP server keeps running, silently stopping all
            // scrobbling. Catch it, log it, and carry on next tick.
            if let Err(p) = AssertUnwindSafe(self.run_tick()).catch_unwind().await {
                let msg = p.downcast_ref::<&str>().map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                error!("💥 Scrobble tick panicked (recovered): {}", msg);
            }

            // A completed tick reached a clean boundary — stop now if asked.
            if *shutdown.borrow() { break; }

            tokio::select! {
                _ = sleep(self.poll_interval) => {}
                _ = shutdown.changed() => {}
            }
        }
        info!("🛑 Scrobble engine stopped cleanly");
    }

    pub async fn run_tick(&mut self) {
        self.reconcile_plex_source().await;

        let mut has_active_now_playing = false;

        let mb_client = self.mb_client.clone();
        let lb_url = self.listenbrainz_url.clone();
        let db = self.db.clone();
        let default_user_id = self.default_user_id;

        for source in &mut self.sources {
            if source.name() == "Plex" {
                if let Some(plex) = source.as_any_mut().downcast_mut::<PlexSource>() {
                    match plex.fetch_sessions_extended(None).await {
                        Ok(session_result) => {
                            if !session_result.now_playing.is_empty() {
                                has_active_now_playing = true;
                                info!("🎧 {} active now playing session(s)", session_result.now_playing.len());
                            }

                            for plex_track in &session_result.now_playing {
                                let mut play = plex_track.to_play("np");
                                enrich_play(&mb_client, &mut play, plex_track.album.as_deref()).await;

                                // Feed the dashboard deck. Plex reports the real
                                // playhead, so the UI gets a true position rather
                                // than counting up from when we first saw it.
                                let user_id = match &plex_track.plex_user {
                                    Some(name) => db.get_user_id_by_username(name).await.ok().flatten().unwrap_or(default_user_id),
                                    None => default_user_id,
                                };
                                let progress = plex_track.view_offset.unwrap_or(0) / 1000;
                                self.now_playing.set(user_id, NowPlaying::from_play(
                                    &play, "Plex", plex_track.audio_quality.as_ref(), progress,
                                ));

                                // Forward to *that* user's connected accounts.
                                let user_sinks = sinks::sinks_for_user(&db, user_id, &lb_url).await;
                                sinks::submit_now_playing(&user_sinks, &play).await;
                            }

                            if !session_result.ready_to_scrobble.is_empty() {
                                info!("📀 Processing {} ready-to-scrobble track(s)", session_result.ready_to_scrobble.len());

                                let current_time = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

                                for plex_track in session_result.ready_to_scrobble {
                                    let mut play = plex_track.to_play(&format!("scrobble-{}", current_time));

                                    // Map the Plex account to a Tapedeck user by
                                    // username; fall back to the default user.
                                    let user_id = match &plex_track.plex_user {
                                        Some(name) => db.get_user_id_by_username(name).await.ok().flatten().unwrap_or(default_user_id),
                                        None => default_user_id,
                                    };

                                    // ── FIX: Check dedup BEFORE enrichment to avoid wasting MB API calls ──
                                    let already_exists = db.exists(user_id, &play.source_id, &play.source_name).await.unwrap_or(false)
                                        || db.fuzzy_exists(user_id, &play.title, &play.artist, play.timestamp as i64).await.unwrap_or(false);

                                    if already_exists {
                                        debug!("Skipping already-scrobbled: {} - {}", play.artist, play.title);
                                        continue;
                                    }

                                    // Only enrich new tracks
                                    enrich_play(&mb_client, &mut play, plex_track.album.as_deref()).await;

                                    // Pass Plex-extracted audio quality if available
                                    let quality_ref = plex_track.audio_quality.as_ref();
                                    let quality_log = quality_ref
                                        .and_then(|q| q.quality_score)
                                        .map(|s| format!("{:.0}", s))
                                        .unwrap_or_else(|| "n/a".into());

                                    match db.save_scrobble(
                                        user_id, &play, quality_ref,
                                        None, None, None, Some("Plex"),
                                    ).await {
                                        Ok(true) => info!("📥 Queued: {} - {} [quality: {}]",
                                            play.artist, play.title, quality_log),
                                        Ok(false) => {}
                                        Err(e) => error!("Database error: {}", e),
                                    }
                                }
                            }
                        }
                        Err(e) => error!("Error fetching from Plex: {}", e),
                    }
                }
            }
        }

        if !has_active_now_playing {
            // A Plex tick with no sessions is an authoritative "stopped" — clear
            // the deck now instead of letting the entry time out on its own.
            self.now_playing.clear_source("Plex");
            self.flush_pending().await;
        } else {
            debug!("⏸ Skipping pending scrobbles - active now playing session");
        }

        if let Some(plex) = self.sources.iter_mut()
            .find(|s| s.name() == "Plex")
            .and_then(|s| s.as_any_mut().downcast_mut::<PlexSource>())
        {
            plex.cleanup_sessions(3600);
        }

        // Incrementally classify listen languages (script instantly, a few
        // MusicBrainz lookups per tick to respect the rate limit).
        crate::enrich::backfill_languages(&self.db, &self.mb_client, 50, 8).await;

        // Same deal for release years (feeds the by-decade breakdown) — cache
        // hits are free, so the budget only bounds real API calls.
        crate::enrich::backfill_release_years(&self.db, &self.mb_client, 50, 8).await;
        crate::enrich::backfill_artwork(&self.db, &self.mb_client, 50, 8).await;
    }

    async fn flush_pending(&mut self) {
        let pending = match self.db.get_pending_scrobbles().await {
            Ok(p) => p,
            Err(e) => { error!("Failed to fetch pending scrobbles: {}", e); return; }
        };
        if pending.is_empty() { return; }
        info!("🔄 Processing {} pending scrobble(s)", pending.len());

        // Group by user so each user's listens forward to *their* connected
        // services — and only those. No server-wide fallback: a user who has
        // connected nothing keeps their listens local.
        let mut by_user: HashMap<i64, Vec<crate::db::PendingScrobble>> = HashMap::new();
        for ps in pending { by_user.entry(ps.user_id).or_default().push(ps); }

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
                    if ps.delivered.contains(&name) { continue; }
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
