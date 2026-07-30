use super::ScrobbleSink;
use crate::models::Play;
use async_trait::async_trait;
use reqwest::Client;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

/// Tapedeck's registered Last.fm application credentials.
///
/// These are baked in on purpose. Last.fm issues one key per *application*, not
/// per user — every desktop scrobbler (fooyin, Audacious, Clementine…) ships its
/// key the same way, because a client-side app has nowhere to hide one. They
/// identify Tapedeck; the thing that authorises a scrobble is the per-user
/// session key obtained through the auth flow and stored in `user_connections`.
/// So this is not a secret worth protecting — it just means users don't have to
/// register their own app before connecting Last.fm.
pub const LASTFM_API_KEY: &str = "f90c2a1009b9e296ad339bd65e059ec7";
pub const LASTFM_SECRET: &str = "7f5ef26759ce73de771845e65375050e";

/// A scrobble sink for Last.fm-compatible APIs (Last.fm, Libre.fm, or any GNU FM instance).
pub struct LastFmSink {
    api_key: String,
    secret: String,
    session_key: String,
    client: Client,
    base_url: String,
    service_name: String,
}

impl LastFmSink {
    /// Create a sink for Last.fm (api.audioscrobbler.com).
    pub fn new(api_key: String, secret: String, session_key: String) -> Self {
        Self::with_url(
            api_key,
            secret,
            session_key,
            "https://ws.audioscrobbler.com/2.0/".to_string(),
            "Last.fm".to_string(),
        )
    }

    /// Create a sink for Libre.fm (libre.fm) or any GNU FM-compatible server.
    pub fn libre_fm(
        api_key: String,
        secret: String,
        session_key: String,
        base_url: Option<String>,
    ) -> Self {
        Self::with_url(
            api_key,
            secret,
            session_key,
            base_url.unwrap_or_else(|| "https://libre.fm/2.0/".to_string()),
            "Libre.fm".to_string(),
        )
    }

    /// Create a sink with a custom API URL and service name.
    pub fn with_url(
        api_key: String,
        secret: String,
        session_key: String,
        base_url: String,
        service_name: String,
    ) -> Self {
        let app_name = env!("CARGO_PKG_NAME");
        let app_version = env!("CARGO_PKG_VERSION");
        let user_agent = format!("{}/{}", app_name, app_version);

        let client = Client::builder()
            .user_agent(user_agent)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            api_key,
            secret,
            session_key,
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            service_name,
        }
    }

    fn api_endpoint(&self) -> &str {
        &self.base_url
    }

    fn generate_signature(&self, params: &[(String, String)]) -> String {
        let mut sorted_params: Vec<_> = params.iter().collect();
        sorted_params.sort_by(|a, b| a.0.cmp(&b.0));

        let mut sig_base = String::new();
        for (k, v) in sorted_params {
            sig_base.push_str(k);
            sig_base.push_str(v);
        }
        sig_base.push_str(&self.secret);

        let digest = md5::compute(sig_base.as_bytes());
        format!("{:x}", digest)
    }
}

#[async_trait]
impl ScrobbleSink for LastFmSink {
    fn name(&self) -> &str {
        &self.service_name
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>> {
        if plays.is_empty() {
            return Ok(());
        }

        info!(
            "Submitting {} plays to {}...",
            plays.len(),
            self.service_name
        );

        for chunk in plays.chunks(50) {
            let mut params: Vec<(String, String)> = Vec::new();
            params.push(("method".into(), "track.scrobble".into()));
            params.push(("api_key".into(), self.api_key.clone()));
            params.push(("sk".into(), self.session_key.clone()));

            for (i, play) in chunk.iter().enumerate() {
                debug!(
                    "Preparing {} scrobble for: {} - {}",
                    self.service_name, play.artist, play.title
                );

                params.push((format!("artist[{}]", i), play.artist.clone()));
                params.push((format!("track[{}]", i), play.title.clone()));
                params.push((format!("timestamp[{}]", i), play.timestamp.to_string()));

                if let Some(ref album) = play.album {
                    params.push((format!("album[{}]", i), album.clone()));
                }

                if let Some(ref artists) = play.artists {
                    if let Some(album_artist) = artists.first() {
                        if album_artist != &play.artist {
                            params.push((format!("albumArtist[{}]", i), album_artist.clone()));
                        }
                    }
                }

                if let Some(duration) = play.duration {
                    params.push((format!("duration[{}]", i), duration.to_string()));
                }

                if let Some(track_num) = play.track_number {
                    params.push((format!("trackNumber[{}]", i), track_num.to_string()));
                }

                if let Some(ref mbid) = play.mbid_recording {
                    params.push((format!("mbid[{}]", i), mbid.clone()));
                    debug!("Including recording MBID for '{}': {}", play.title, mbid);
                }
            }

            let signature = self.generate_signature(&params);
            params.push(("api_sig".into(), signature));
            params.push(("format".into(), "json".into()));

            let form_data: HashMap<String, String> = params.into_iter().collect();

            // Retry transient failures with exponential backoff so a flaky
            // network or a brief service outage doesn't fail the scrobble.
            const MAX_RETRIES: u32 = 3;
            let mut backoff = 2u64;
            let mut attempt = 0u32;
            loop {
                attempt += 1;
                match self
                    .client
                    .post(self.api_endpoint())
                    .form(&form_data)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        debug!(
                            "Successfully submitted chunk of {} plays to {}",
                            chunk.len(),
                            self.service_name
                        );
                        break;
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let error_text = resp.text().await.unwrap_or_default();
                        if attempt >= MAX_RETRIES {
                            error!(
                                "{} submission failed after {} attempts (status {}): {}",
                                self.service_name, attempt, status, error_text
                            );
                            return Err(
                                format!("{} API Error: {}", self.service_name, error_text).into()
                            );
                        }
                        warn!(
                            "{} submission failed (status {}), retry {}/{} in {}s",
                            self.service_name, status, attempt, MAX_RETRIES, backoff
                        );
                        sleep(Duration::from_secs(backoff)).await;
                        backoff *= 2;
                    }
                    Err(e) => {
                        if attempt >= MAX_RETRIES {
                            error!(
                                "{} request failed after {} attempts: {}",
                                self.service_name, attempt, e
                            );
                            return Err(Box::new(e));
                        }
                        warn!(
                            "{} request error, retry {}/{} in {}s: {}",
                            self.service_name, attempt, MAX_RETRIES, backoff, e
                        );
                        sleep(Duration::from_secs(backoff)).await;
                        backoff *= 2;
                    }
                }
            }
        }

        info!(
            "✅ Successfully submitted {} plays to {}",
            plays.len(),
            self.service_name
        );
        Ok(())
    }
}
