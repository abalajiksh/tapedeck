use super::ScrobbleSink;
use crate::models::Play;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::json;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

/// Ceiling on one request. Without it a hung connection is bounded only by the
/// OS, and this call is awaited inline on the engine tick.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Attempts after a **rate limit**. ListenBrainz tells us when to come back, so
/// waiting is both cheap and the polite thing to do.
const RATE_LIMIT_RETRIES: u32 = 3;

/// Attempts after a **network error** — deliberately far fewer.
///
/// These were the same number (5, with 2/4/8/16/32s backoff) until 2026-08-24,
/// and conflating them is what turned an upstream outage into a dead deck: a
/// service that cannot be reached at all will not be reachable 62 seconds
/// later either, and the tick is 5 seconds long. One quick retry covers a
/// dropped connection; anything worse is the breaker's business, and the
/// listen simply stays `pending` for the next tick.
const NETWORK_RETRIES: u32 = 1;

/// No single wait may exceed this, whatever a header asks for. A service is
/// free to say "come back in an hour"; this call is not free to sit on the
/// engine tick for one.
const MAX_WAIT: u64 = 20;

pub struct ListenBrainzSink {
    base_url: String,
    token: String,
    client: Client,
}

impl ListenBrainzSink {
    pub fn new(base_url: String, token: String) -> Self {
        let sanitized_token =
            token.trim().chars().filter(|c| c.is_ascii() && !c.is_control()).collect::<String>();

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: sanitized_token,
            // `Client::new()` has **no timeout at all**, which is how a single
            // unreachable host held the engine tick open on 2026-08-24. Thirty
            // seconds is the figure `lb_get_page` already uses against the same
            // service on the import path; there is no reason for two.
            client: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .user_agent(concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Build track metadata with proper mbid_mapping structure
    fn build_track_metadata(play: &Play) -> serde_json::Map<String, serde_json::Value> {
        let mut track_meta = serde_json::Map::new();

        track_meta.insert("artist_name".to_string(), json!(play.artist));
        track_meta.insert("track_name".to_string(), json!(play.title));

        if let Some(ref album) = play.album {
            track_meta.insert("release_name".to_string(), json!(album));
        }

        // additional_info
        let mut additional_info = serde_json::Map::new();
        additional_info.insert("submission_client".to_string(), json!(env!("CARGO_PKG_NAME")));
        additional_info
            .insert("submission_client_version".to_string(), json!(env!("CARGO_PKG_VERSION")));

        if let Some(dur) = play.duration {
            additional_info.insert("duration_ms".to_string(), json!(dur * 1000));
        }
        if let Some(num) = play.track_number {
            additional_info.insert("track_number".to_string(), json!(num));
        }
        if !additional_info.is_empty() {
            track_meta
                .insert("additional_info".to_string(), serde_json::Value::Object(additional_info));
        }

        // mbid_mapping
        let mut has_mbid_data = false;
        let mut mbid_mapping = serde_json::Map::new();

        if let Some(ref mbid) = play.mbid_recording {
            mbid_mapping.insert("recording_mbid".to_string(), json!(mbid));
            mbid_mapping.insert("recording_name".to_string(), json!(play.title));
            has_mbid_data = true;
        }
        if let Some(ref mbid) = play.mbid_release {
            mbid_mapping.insert("release_mbid".to_string(), json!(mbid));
            has_mbid_data = true;
        }
        if let Some(caa_id) = play.caa_id {
            mbid_mapping.insert("caa_id".to_string(), json!(caa_id));
            has_mbid_data = true;
        }
        if let Some(ref caa_release_mbid) = play.caa_release_mbid {
            mbid_mapping.insert("caa_release_mbid".to_string(), json!(caa_release_mbid));
            has_mbid_data = true;
        }
        if let Some(ref mbids) = play.mbid_artist {
            if !mbids.is_empty() {
                mbid_mapping.insert("artist_mbids".to_string(), json!(mbids));
                has_mbid_data = true;

                let artists_array: Vec<serde_json::Value> = mbids
                    .iter()
                    .enumerate()
                    .map(|(idx, mbid)| {
                        let artist_name = if let Some(ref artists) = play.artists {
                            artists.get(idx).cloned().unwrap_or_else(|| play.artist.clone())
                        } else {
                            play.artist.clone()
                        };
                        json!({
                            "artist_credit_name": artist_name,
                            "join_phrase": "",
                            "artist_mbid": mbid
                        })
                    })
                    .collect();

                mbid_mapping.insert("artists".to_string(), json!(artists_array));
            }
        }

        if has_mbid_data {
            debug!("Adding mbid_mapping: recording={:?}, release={:?}, artist={:?}, caa_id={:?}, caa_release={:?}",
                play.mbid_recording, play.mbid_release, play.mbid_artist, play.caa_id, play.caa_release_mbid);
            track_meta.insert("mbid_mapping".to_string(), serde_json::Value::Object(mbid_mapping));
        } else {
            debug!("No MBIDs available for {} - {}", play.artist, play.title);
        }

        track_meta
    }

    /// Submit "now playing" status for a track
    pub async fn submit_now_playing(&self, play: &Play) -> Result<(), Box<dyn std::error::Error>> {
        debug!(
            "Submitting now playing: {} - {} (MBIDs: rec={:?}, rel={:?}, art={:?}, caa={:?})",
            play.artist,
            play.title,
            play.mbid_recording,
            play.mbid_release,
            play.mbid_artist,
            play.caa_id
        );

        let track_meta = Self::build_track_metadata(play);

        let body = json!({
            "listen_type": "playing_now",
            "payload": [{
                "track_metadata": track_meta
            }]
        });

        let endpoint = format!("{}/1/submit-listens", self.base_url);
        let auth_header_value = format!("Token {}", self.token);

        let mut retries = 0;

        loop {
            let resp = self
                .client
                .post(&endpoint)
                .header("Authorization", &auth_header_value)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await;

            match resp {
                Ok(response) => {
                    if response.status().is_success() {
                        debug!("✅ Now playing submitted: {} - {}", play.artist, play.title);
                        return Ok(());
                    } else if response.status() == 429 {
                        // One retry only. Now-playing is superseded by the next
                        // track anyway, so a queued stale one is worth less
                        // than the tick time it costs.
                        if retries >= 1 {
                            let error_text = response.text().await.unwrap_or_default();
                            error!("Now playing rate limited (gave up): {}", error_text);
                            return Ok(());
                        }
                        let retry_after = response
                            .headers()
                            .get("X-RateLimit-Reset-In")
                            .and_then(|h| h.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(2)
                            .min(MAX_WAIT);
                        debug!("Rate limited on now playing, waiting {}s", retry_after);
                        sleep(Duration::from_secs(retry_after + 1)).await;
                        retries += 1;
                        continue;
                    } else {
                        let error_text = response.text().await.unwrap_or_default();
                        error!("Now playing submission failed: {}", error_text);
                        return Ok(());
                    }
                }
                Err(e) => {
                    // Reported rather than swallowed: `sinks::guarded` is what
                    // opens the breaker, and it can only count what it is told.
                    // The caller still treats a failed now-playing as
                    // non-fatal — it just no longer looks like a success.
                    return Err(format!("Network error sending now playing: {}", e).into());
                }
            }
        }
    }
}

#[async_trait]
impl ScrobbleSink for ListenBrainzSink {
    fn name(&self) -> &str {
        "ListenBrainz"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn update_now_playing(&self, play: &Play) -> Result<(), Box<dyn std::error::Error>> {
        self.submit_now_playing(play).await
    }

    /// `POST /1/feedback/recording-feedback`, score 1 (love) or 0 (clear).
    ///
    /// Needs a recording MBID: the endpoint's only other key is a `recording_msid`,
    /// which is an id ListenBrainz assigns to listens *it* received and Tapedeck
    /// therefore doesn't have. A track that was never enriched is skipped rather
    /// than guessed at — locally the love still stands.
    async fn set_loved(
        &self,
        artist: &str,
        title: &str,
        mbid: Option<&str>,
        loved: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(mbid) = mbid.filter(|m| !m.trim().is_empty()) else {
            debug!("No recording MBID for {} - {}; not sending feedback", artist, title);
            return Ok(());
        };

        let body = json!({ "recording_mbid": mbid, "score": if loved { 1 } else { 0 } });
        let resp = self
            .client
            .post(format!("{}/1/feedback/recording-feedback", self.base_url))
            .header("Authorization", format!("Token {}", self.token))
            .json(&body)
            .send()
            .await?;

        if resp.status().is_success() {
            info!(
                "{} {} — {} on ListenBrainz",
                if loved { "❤️ Loved" } else { "Un-loved" },
                artist,
                title
            );
            Ok(())
        } else {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            Err(format!("ListenBrainz feedback failed (status {}): {}", status, text).into())
        }
    }

    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>> {
        if plays.is_empty() {
            return Ok(());
        }

        info!("Submitting {} plays to ListenBrainz...", plays.len());

        let payload_items: Vec<serde_json::Value> = plays.iter().map(|play| {
            debug!("Preparing ListenBrainz payload for: {} - {} (MBIDs: rec={:?}, rel={:?}, art={:?}, caa={:?})",
                play.artist, play.title, play.mbid_recording, play.mbid_release, play.mbid_artist, play.caa_id);

            let track_meta = Self::build_track_metadata(play);

            json!({
                "listened_at": play.timestamp,
                "track_metadata": track_meta
            })
        }).collect();

        for chunk in payload_items.chunks(100) {
            let body = json!({
                "listen_type": "import",
                "payload": chunk
            });

            let endpoint = if self.base_url.ends_with("submit-listens") {
                self.base_url.clone()
            } else {
                format!("{}/1/submit-listens", self.base_url)
            };

            let auth_header_value = format!("Token {}", self.token);

            // Two budgets, not one — see the constants. A rate limit is worth
            // waiting out; an unreachable host is not, and treating them alike
            // is what cost the engine ~18 hours of sleeping on 2026-08-24.
            let mut rate_limit_retries = 0;
            let mut network_retries = 0;
            let mut backoff = 2;

            loop {
                debug!("Submitting chunk of {} plays to ListenBrainz", chunk.len());

                let resp = self
                    .client
                    .post(&endpoint)
                    .header("Authorization", &auth_header_value)
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await;

                match resp {
                    Ok(response) => {
                        if response.status().is_success() {
                            info!(
                                "✅ Successfully submitted {} plays to ListenBrainz",
                                chunk.len()
                            );
                            break;
                        } else if response.status() == 429 {
                            let reset_in = response
                                .headers()
                                .get("X-RateLimit-Reset-In")
                                .and_then(|h| h.to_str().ok())
                                .and_then(|s| s.parse::<u64>().ok());

                            let remaining = response
                                .headers()
                                .get("X-RateLimit-Remaining")
                                .and_then(|h| h.to_str().ok())
                                .unwrap_or("?");

                            // Capped: the service may legitimately ask for a
                            // long wait, but this is awaited on the engine tick,
                            // so a longer one is left for the flush deadline to
                            // abandon and pick up again next tick.
                            let wait_time = reset_in.unwrap_or(backoff).min(MAX_WAIT);

                            warn!("ListenBrainz rate limit exceeded (remaining: {}). Waiting {}s before retry...", remaining, wait_time);

                            sleep(Duration::from_secs(wait_time + 1)).await;

                            rate_limit_retries += 1;
                            if rate_limit_retries > RATE_LIMIT_RETRIES {
                                let error_text = response.text().await.unwrap_or_default();
                                error!(
                                    "Max retries exceeded for ListenBrainz. Last error: {}",
                                    error_text
                                );
                                return Err(format!(
                                    "Rate limit exceeded after retries: {}",
                                    error_text
                                )
                                .into());
                            }

                            if reset_in.is_none() {
                                backoff *= 2;
                            }
                            continue;
                        } else {
                            let status = response.status();
                            let error_text = response.text().await.unwrap_or_default();
                            error!(
                                "ListenBrainz submission failed. Status: {}, Response: {}",
                                status, error_text
                            );
                            return Err(format!("ListenBrainz API Error: {}", error_text).into());
                        }
                    }
                    Err(e) => {
                        network_retries += 1;
                        if network_retries > NETWORK_RETRIES {
                            // Logged at debug because `sinks::guarded` reports
                            // this once when the breaker opens, and the one
                            // thing it must not do is write a line per listen:
                            // on 2026-08-24 this exact `error!` produced 6,096
                            // of the day's 7,317 error lines — one fact,
                            // repeated until nothing else could be found.
                            debug!("Network error sending to ListenBrainz: {}", e);
                            return Err(format!("Network error after retries: {}", e).into());
                        }
                        sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                }
            }
        }

        Ok(())
    }
}
