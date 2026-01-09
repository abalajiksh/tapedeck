use async_trait::async_trait;
use reqwest::{Client, Response};
use crate::models::Play;
use super::ScrobbleSink;
use log::{info, error, debug};
use std::collections::HashMap;

pub struct LastFmSink {
    api_key: String,
    secret: String,
    session_key: String,
    client: Client,
}

impl LastFmSink {
    pub fn new(api_key: String, secret: String, session_key: String) -> Self {
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
        }
    }

    fn generate_signature(&self, params: &Vec<(&str, String)>) -> String {
        let mut sorted_params = params.clone();
        sorted_params.sort_by(|a, b| a.0.cmp(b.0));

        let mut sig_base = String::new();
        for (k, v) in sorted_params {
            sig_base.push_str(k);
            sig_base.push_str(&v);
        }
        sig_base.push_str(&self.secret);

        let digest = md5::compute(sig_base.as_bytes());
        format!("{:x}", digest)
    }
}

#[async_trait]
impl ScrobbleSink for LastFmSink {
    fn name(&self) -> &str {
        "Last.fm"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>> {
        if plays.is_empty() {
            return Ok(());
        }

        info!("Submitting {} plays to Last.fm...", plays.len());

        // Last.fm allows up to 50 scrobbles per request
        for chunk in plays.chunks(50) {
            let mut params: Vec<(&str, String)> = Vec::new();
            params.push(("method", "track.scrobble".to_string()));
            params.push(("api_key", self.api_key.clone()));
            params.push(("sk", self.session_key.clone()));

            for (i, play) in chunk.iter().enumerate() {
                debug!("Preparing Last.fm scrobble for: {} - {}", play.artist, play.title);
                
                // Required fields
                params.push((Box::leak(format!("artist[{}]", i).into_boxed_str()), play.artist.clone()));
                params.push((Box::leak(format!("track[{}]", i).into_boxed_str()), play.title.clone()));
                params.push((Box::leak(format!("timestamp[{}]", i).into_boxed_str()), play.timestamp.to_string()));

                // Optional fields
                if let Some(ref album) = play.album {
                    params.push((Box::leak(format!("album[{}]", i).into_boxed_str()), album.clone()));
                }

                // Album artist (if available from artists field)
                if let Some(ref artists) = play.artists {
                    if let Some(album_artist) = artists.first() {
                        if album_artist != &play.artist {
                            params.push((Box::leak(format!("albumArtist[{}]", i).into_boxed_str()), album_artist.clone()));
                        }
                    }
                }

                // Duration in seconds
                if let Some(duration) = play.duration {
                    params.push((Box::leak(format!("duration[{}]", i).into_boxed_str()), duration.to_string()));
                }

                // Track number
                if let Some(track_num) = play.track_number {
                    params.push((Box::leak(format!("trackNumber[{}]", i).into_boxed_str()), track_num.to_string()));
                }

                // MusicBrainz Track ID (recording MBID)
                if let Some(ref mbid) = play.mbid_recording {
                    params.push((Box::leak(format!("mbid[{}]", i).into_boxed_str()), mbid.clone()));
                    debug!("Including recording MBID for '{}': {}", play.title, mbid);
                }
            }

            let signature = self.generate_signature(&params);
            params.push(("api_sig", signature));
            params.push(("format", "json".to_string()));

            let form_data: HashMap<&str, String> = params.into_iter().collect();

            let resp: Response = self.client.post("https://ws.audioscrobbler.com/2.0/")
                .form(&form_data)
                .send()
                .await?;

            if !resp.status().is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                error!("Last.fm submission failed: {}", error_text);
                return Err(format!("Last.fm API Error: {}", error_text).into());
            }

            debug!("Successfully submitted chunk of {} plays to Last.fm", chunk.len());
        }

        info!("✅ Successfully submitted {} plays to Last.fm", plays.len());
        Ok(())
    }
}
