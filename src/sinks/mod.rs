mod health;
mod lastfm;
mod listenbrainz;
pub use health::health;
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

    /// Mirror a love (or un-love) of one *recording* to the service.
    ///
    /// Only recordings: Last.fm's `track.love` and ListenBrainz's feedback both
    /// take a track, and neither has a notion of loving an album or an artist —
    /// so those stay local rather than being approximated. Defaults to a no-op.
    async fn set_loved(
        &self,
        _artist: &str,
        _title: &str,
        _mbid: Option<&str>,
        _loved: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
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
///
/// **Returns a `Result`, and the flush path must not paper over it.** This read
/// used to be `.unwrap_or_default()`, which made a transient `SQLITE_BUSY`
/// indistinguishable from "this user has connected nothing" — and in
/// `flush_pending` the consequence was not a skipped tick but a permanent one:
/// `enabled` came back empty, `enabled.iter().all(...)` was vacuously true, the
/// row was marked `synced`, and the listen was never forwarded to anything,
/// logged as a success. A caller that genuinely tolerates an empty answer (a
/// now-playing update, superseded within seconds) may say so explicitly; the
/// scrobble flush may not.
pub async fn sinks_for_user(
    db: &Database,
    user_id: i64,
    listenbrainz_url: &str,
) -> Result<Vec<Box<dyn ScrobbleSink>>, sqlx::Error> {
    // A replica holds the credentials — its snapshot came from an instance that
    // has them — and must not spend them. Note this is *not* on its own enough
    // to stop forwarding: `flush_pending` reads "every enabled sink took it" as
    // vacuously true over an empty list and would mark the row delivered having
    // sent it nowhere, which is the v0.91.0 defect. The flush gate is what makes
    // this one safe; this one is what catches every other caller.
    if crate::replica::is_replica() {
        return Ok(Vec::new());
    }
    let conns = db.get_connections_for_forwarding(user_id).await?;
    let mut sinks: Vec<Box<dyn ScrobbleSink>> = Vec::new();

    for (service, creds_json) in conns {
        // A blob that will not parse is a broken connection, not an absent one.
        // `unwrap_or_default()` here turned it into `Value::Null`, so the sink
        // was quietly dropped from the user's list with nothing said — the same
        // failed-read-as-absence shape as the read above.
        let creds: serde_json::Value = match serde_json::from_str(&creds_json) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(
                    "Stored {} credentials for user {} will not parse ({}) — \
                     that service is not being forwarded to",
                    service,
                    user_id,
                    e
                );
                continue;
            }
        };
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
    Ok(sinks)
}

/// Call one sink, with the breaker in front of it and the outcome recorded.
///
/// Every outbound call goes through here so that "is this service down?" has
/// one answer, kept in one place — see [`health`]. A skipped call is reported
/// as an error to the caller, which is right: nothing was delivered, so a
/// scrobble must stay `pending` rather than be counted as sent.
pub async fn guarded<F, T>(name: &str, call: F) -> Result<T, Box<dyn std::error::Error>>
where
    F: std::future::Future<Output = Result<T, Box<dyn std::error::Error>>>,
{
    if let Some(left) = health().skip_for(name) {
        return Err(format!("{} is being skipped for another {}s", name, left.as_secs()).into());
    }
    match call.await {
        Ok(v) => {
            health().record_ok(name);
            Ok(v)
        }
        Err(e) => {
            health().record_failure(name, &e.to_string());
            Err(e)
        }
    }
}

/// Forward a now-playing update to every one of a user's sinks.
pub async fn submit_now_playing(sinks: &[Box<dyn ScrobbleSink>], play: &Play) {
    if crate::replica::is_replica() {
        return;
    }
    for sink in sinks {
        // Through the breaker like everything else. Now-playing is the call
        // made most often — once per track change, per user — so a dead service
        // costs the most here, and it is the least worth waiting for: the next
        // track supersedes it regardless.
        if let Err(e) = guarded(sink.name(), sink.update_now_playing(play)).await {
            tracing::debug!("Did not forward now playing to {}: {}", sink.name(), e);
        }
    }
}

/// Forward a love to every one of a user's sinks.
///
/// Never fails the caller: the love is already stored locally, and a service
/// being unreachable shouldn't make the heart in the UI refuse to fill. This is
/// the one piece of forwarding with no retry queue — unlike a scrobble, a love
/// is idempotent and re-assertable, so the next toggle fixes a missed one.
pub async fn submit_love(
    sinks: &[Box<dyn ScrobbleSink>],
    artist: &str,
    title: &str,
    mbid: Option<&str>,
    loved: bool,
) {
    for sink in sinks {
        if let Err(e) = guarded(sink.name(), sink.set_loved(artist, title, mbid, loved)).await {
            tracing::error!("Failed to forward love to {}: {}", sink.name(), e);
        }
    }
}
