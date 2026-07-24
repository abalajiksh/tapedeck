use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info};

use crate::db::Database;
use crate::models::Play;
use crate::musicbrainz::MusicBrainzClient;
use crate::sinks::{ListenBrainzSink, ScrobbleSink};
use crate::sources::{MusicSource, PlexSource};

use super::enrichment::enrich_play;

pub struct ScrobbleEngine {
    sources: Vec<Box<dyn MusicSource>>,
    sinks: Arc<Vec<Box<dyn ScrobbleSink>>>,
    db: Arc<Database>,
    mb_client: Arc<MusicBrainzClient>,
    poll_interval: Duration,
    default_user_id: i64,
}

impl ScrobbleEngine {
    pub fn new(
        sources: Vec<Box<dyn MusicSource>>,
        sinks: Arc<Vec<Box<dyn ScrobbleSink>>>,
        db: Arc<Database>,
        mb_client: Arc<MusicBrainzClient>,
    ) -> Self {
        Self {
            sources, sinks, db, mb_client,
            poll_interval: Duration::from_secs(15),
            default_user_id: 1,
        }
    }

    pub async fn run(&mut self) -> ! {
        use futures_util::future::FutureExt;
        use std::panic::AssertUnwindSafe;

        info!("🎵 Starting scrobble loop with prioritized MusicBrainz metadata enrichment...");
        loop {
            // Isolate each tick: a panic here would otherwise kill the engine
            // task while the HTTP server keeps running, silently stopping all
            // scrobbling. Catch it, log it, and carry on next tick.
            if let Err(p) = AssertUnwindSafe(self.run_tick()).catch_unwind().await {
                let msg = p.downcast_ref::<&str>().map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                error!("💥 Scrobble tick panicked (recovered): {}", msg);
            }
            sleep(self.poll_interval).await;
        }
    }

    pub async fn run_tick(&mut self) {
        let mut has_active_now_playing = false;

        let sinks = self.sinks.clone();
        let mb_client = self.mb_client.clone();
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
                                Self::submit_now_playing_to(&sinks, &play).await;
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
    }

    async fn submit_now_playing_to(sinks: &[Box<dyn ScrobbleSink>], play: &Play) {
        for sink in sinks.iter() {
            if let Some(lb_sink) = sink.as_any().downcast_ref::<ListenBrainzSink>() {
                if let Err(e) = lb_sink.submit_now_playing(play).await {
                    error!("Failed to submit now playing to {}: {}", sink.name(), e);
                }
            }
        }
    }

    async fn flush_pending(&mut self) {
        let pending = match self.db.get_pending_scrobbles().await {
            Ok(p) => p,
            Err(e) => { error!("Failed to fetch pending scrobbles: {}", e); return; }
        };
        if pending.is_empty() { return; }
        info!("🔄 Processing {} pending scrobble(s)", pending.len());

        let enabled: Vec<String> = self.sinks.iter().map(|s| s.name().to_string()).collect();

        for mut ps in pending {
            if ps.play.mbid_recording.is_none() {
                let album_hint = ps.play.album.clone();
                enrich_play(&self.mb_client, &mut ps.play, album_hint.as_deref()).await;
            }

            // Only send to sinks that haven't already accepted this listen, so a
            // retry after a partial failure can't double-scrobble.
            for sink in self.sinks.iter() {
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

            // Fully synced once every enabled sink has accepted it (trivially
            // true when no sinks are configured — store-locally-only mode).
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
