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

/// Central orchestrator that owns all runtime resources and drives the
/// poll → enrich → dedup → store → dispatch pipeline.
pub struct ScrobbleEngine {
    sources: Vec<Box<dyn MusicSource>>,
    sinks: Arc<Vec<Box<dyn ScrobbleSink>>>,
    db: Arc<Database>,
    mb_client: Arc<MusicBrainzClient>,
    poll_interval: Duration,
    /// User ID used for scrobbles from polling sources (Plex, Navidrome, etc.)
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
            sources,
            sinks,
            db,
            mb_client,
            poll_interval: Duration::from_secs(15),
            default_user_id: 1,
        }
    }

    /// Run the scrobble loop forever.
    pub async fn run(&mut self) -> ! {
        info!("🎵 Starting scrobble loop with prioritized MusicBrainz metadata enrichment...");
        loop {
            self.run_tick().await;
            sleep(self.poll_interval).await;
        }
    }

    /// Execute one iteration of the poll/enrich/dispatch cycle.
    pub async fn run_tick(&mut self) {
        let mut has_active_now_playing = false;

        // Grab shared refs to non-source fields before the mutable source borrow.
        // This avoids the "cannot borrow `*self` as immutable" error.
        let sinks = self.sinks.clone();
        let mb_client = self.mb_client.clone();
        let db = self.db.clone();
        let default_user_id = self.default_user_id;

        for source in &mut self.sources {
            if source.name() == "Plex" {
                if let Some(plex) = source.as_any_mut().downcast_mut::<PlexSource>() {
                    match plex.fetch_sessions_extended(None).await {
                        Ok(session_result) => {
                            // A. Now Playing
                            if !session_result.now_playing.is_empty() {
                                has_active_now_playing = true;
                                info!(
                                    "🎧 {} active now playing session(s) detected",
                                    session_result.now_playing.len()
                                );
                            }

                            for plex_track in &session_result.now_playing {
                                let mut play = plex_track.to_play("np");
                                enrich_play(
                                    &mb_client,
                                    &mut play,
                                    plex_track.album.as_deref(),
                                )
                                .await;
                                Self::submit_now_playing_to(&sinks, &play).await;
                            }

                            // B. Scrobble candidates
                            if !session_result.ready_to_scrobble.is_empty() {
                                info!(
                                    "📀 Processing {} ready-to-scrobble track(s)",
                                    session_result.ready_to_scrobble.len()
                                );

                                let current_time = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs();

                                for plex_track in session_result.ready_to_scrobble {
                                    let mut play = plex_track
                                        .to_play(&format!("scrobble-{}", current_time));
                                    enrich_play(
                                        &mb_client,
                                        &mut play,
                                        plex_track.album.as_deref(),
                                    )
                                    .await;

                                    match db.save_scrobble(default_user_id, &play).await {
                                        Ok(true) => {
                                            info!("📥 Queued new play: {} - {}", play.artist, play.title)
                                        }
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
            debug!("⏸ Skipping pending scrobbles processing - active now playing session detected");
        }

        if let Some(plex) = self
            .sources
            .iter_mut()
            .find(|s| s.name() == "Plex")
            .and_then(|s| s.as_any_mut().downcast_mut::<PlexSource>())
        {
            plex.cleanup_sessions(3600);
        }
    }

    // ── Helpers ──

    /// Submit now-playing to sinks. Takes sinks by ref to avoid borrowing self.
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
        match self.db.get_pending_scrobbles().await {
            Ok(pending_plays) => {
                if pending_plays.is_empty() {
                    return;
                }
                info!(
                    "🔄 No active sessions - processing {} pending scrobble(s) from history",
                    pending_plays.len()
                );

                for (user_id, mut play) in pending_plays {
                    if play.mbid_recording.is_none() {
                        let album_hint = play.album.clone();
                        enrich_play(&self.mb_client, &mut play, album_hint.as_deref()).await;
                    }

                    let mut all_succeeded = true;
                    for sink in self.sinks.iter() {
                        match sink.scrobble(&[play.clone()]).await {
                            Ok(_) => debug!("Sent to {}", sink.name()),
                            Err(e) => {
                                error!("Failed to send to {}: {}", sink.name(), e);
                                all_succeeded = false;
                            }
                        }
                    }

                    if all_succeeded {
                        if let Err(e) =
                            self.db.mark_as_scrobbled(user_id, &play.source_id, &play.source_name).await
                        {
                            error!("Failed to mark as scrobbled: {}", e);
                        } else {
                            info!("✅ Synced: {} - {}", play.artist, play.title);
                        }
                    }
                }
            }
            Err(e) => error!("Failed to fetch pending scrobbles: {}", e),
        }
    }
}
