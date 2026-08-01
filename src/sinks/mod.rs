mod lastfm;
mod listenbrainz;
pub use lastfm::{LastFmSink, LASTFM_API_KEY, LASTFM_SECRET};
pub use listenbrainz::ListenBrainzSink;

use crate::db::Database;
use crate::models::Play;
use async_trait::async_trait;
use std::any::Any;

/// Default ListenBrainz endpoint. Overridable via config (`LISTENBRAINZ_URL`)
/// because self-hosted ListenBrainz instances are a real deployment, unlike the
/// Last.fm app credentials which are fixed to Tapedeck-the-application.
pub const LISTENBRAINZ_DEFAULT_URL: &str = "https://api.listenbrainz.org";

#[async_trait]
pub trait ScrobbleSink: Any + Send + Sync {
    fn name(&self) -> &str;
    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>>;
    fn as_any(&self) -> &dyn Any;

    /// Tell the service what's playing right now.
    ///
    /// Defaults to a no-op so a sink that has no such concept doesn't have to
    /// say so. This is a trait method rather than a downcast because every
    /// service Tapedeck forwards to *does* have the concept — it's ordinary
    /// sink behaviour, unlike the source-specific capabilities the pipeline
    /// downcasts for.
    async fn update_now_playing(&self, _play: &Play) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}

/// Build the sinks for one user from their stored connections.
///
/// There is deliberately no server-wide fallback: a listen is only ever
/// forwarded to accounts *that user* connected. A user who has connected
/// nothing gets an empty vec and their listens stay local. Used by both the
/// HTTP layer (now-playing on ingest) and the poll engine (scrobble flush,
/// now-playing from Plex), so the rule can't drift between them.
pub async fn sinks_for_user(
    db: &Database,
    user_id: i64,
    listenbrainz_url: &str,
) -> Vec<Box<dyn ScrobbleSink>> {
    let conns = db.get_connections_for_forwarding(user_id).await.unwrap_or_default();
    let mut sinks: Vec<Box<dyn ScrobbleSink>> = Vec::new();

    for (service, creds_json) in conns {
        let creds: serde_json::Value = serde_json::from_str(&creds_json).unwrap_or_default();
        match service.as_str() {
            "listenbrainz" => {
                if let Some(t) = creds["token"].as_str() {
                    sinks.push(Box::new(ListenBrainzSink::new(
                        listenbrainz_url.to_string(),
                        t.to_string(),
                    )));
                }
            }
            "lastfm" => {
                if let Some(sk) = creds["session_key"].as_str() {
                    sinks.push(Box::new(LastFmSink::new(
                        LASTFM_API_KEY.to_string(),
                        LASTFM_SECRET.to_string(),
                        sk.to_string(),
                    )));
                }
            }
            "librefm" => {
                if let Some(sk) = creds["session_key"].as_str() {
                    sinks.push(Box::new(LastFmSink::libre_fm(
                        LASTFM_API_KEY.to_string(),
                        LASTFM_SECRET.to_string(),
                        sk.to_string(),
                        None,
                    )));
                }
            }
            _ => {}
        }
    }
    sinks
}

/// Forward a now-playing update to every one of a user's sinks.
pub async fn submit_now_playing(sinks: &[Box<dyn ScrobbleSink>], play: &Play) {
    for sink in sinks {
        if let Err(e) = sink.update_now_playing(play).await {
            tracing::error!("Failed to forward now playing to {}: {}", sink.name(), e);
        }
    }
}
