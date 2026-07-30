use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::models::{AudioQuality, Play};
use super::auth::AuthUser;
use super::AppState;

/// Nothing here is persisted: "now playing" is live state, so a restart
/// correctly shows an empty deck until a source reports again.
#[derive(Clone, Debug, Serialize)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Track length in seconds, when the source reports one.
    pub duration: Option<u64>,
    /// Playback position in seconds **as of `updated_at`**. The client
    /// extrapolates from there between polls rather than us ticking every second.
    pub progress: u64,
    pub updated_at: i64,
    /// Which source reported this — also the key the pipeline clears by.
    pub source: String,
    pub listening_context: Option<String>,
    // Quality fields, named to match the frontend's `qualityLabel()` shape.
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bit_depth: Option<i16>,
    pub sample_rate: Option<i32>,
    pub dsd_multiplier: Option<i16>,
    pub delivery_codec: Option<String>,
    pub is_lossless: Option<bool>,
}

impl NowPlaying {
    pub fn from_play(play: &Play, source: &str, quality: Option<&AudioQuality>, progress: u64) -> Self {
        Self {
            title: play.title.clone(),
            artist: play.artist.clone(),
            album: play.album.clone(),
            duration: play.duration,
            progress,
            updated_at: now_secs(),
            source: source.to_string(),
            listening_context: None,
            format_type: quality.and_then(|q| q.format_type.clone()),
            codec: quality.and_then(|q| q.codec.clone()),
            bit_depth: quality.and_then(|q| q.bit_depth),
            sample_rate: quality.and_then(|q| q.sample_rate),
            dsd_multiplier: quality.and_then(|q| q.dsd_multiplier),
            delivery_codec: quality.and_then(|q| q.delivery_codec.clone()),
            is_lossless: quality.and_then(|q| q.is_lossless),
        }
    }

    /// Where playback would be by `now`, assuming it kept running.
    fn position_at(&self, now: i64) -> u64 {
        self.progress + (now - self.updated_at).max(0) as u64
    }

    /// A source that stops reporting has usually stopped playing, but sources
    /// differ: Plex re-reports every poll, while a ListenBrainz-style client
    /// sends `playing_now` once at track start. So expire by *running off the
    /// end of the track* when we know its length, and fall back to a flat
    /// timeout when we don't.
    fn is_live(&self, now: i64) -> bool {
        match self.duration {
            Some(d) if d > 0 => self.position_at(now) < d + 10,
            _ => now - self.updated_at < 600,
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

/// In-memory "what's playing right now", keyed by user. Shared by the HTTP
/// handlers and the poll engine, so it lives behind its own `Arc` rather than
/// inside `AppState` (the engine never sees `AppState`).
#[derive(Clone, Default)]
pub struct NowPlayingRegistry(Arc<RwLock<HashMap<i64, NowPlaying>>>);

impl NowPlayingRegistry {
    pub fn set(&self, user_id: i64, np: NowPlaying) {
        if let Ok(mut map) = self.0.write() {
            map.insert(user_id, np);
        }
    }

    /// Drop every entry reported by `source`. The poll engine calls this when a
    /// tick finds no active sessions — that is the authoritative "stopped"
    /// signal for a polled source, so we don't wait for the track to time out.
    pub fn clear_source(&self, source: &str) {
        if let Ok(mut map) = self.0.write() {
            map.retain(|_, np| np.source != source);
        }
    }

    /// Drop this user's entry only if `source` reported it.
    ///
    /// Needed once sources are per-user: an idle Plex must not wipe a
    /// now-playing that the user's phone is currently reporting over ingest.
    pub fn clear_user_source(&self, user_id: i64, source: &str) {
        if let Ok(mut map) = self.0.write() {
            if map.get(&user_id).is_some_and(|np| np.source == source) {
                map.remove(&user_id);
            }
        }
    }

    /// Drop this user's entry, whatever reported it. Backs
    /// `POST /1/playing-now/delete`.
    pub fn clear_user(&self, user_id: i64) {
        if let Ok(mut map) = self.0.write() {
            map.remove(&user_id);
        }
    }

    /// The user's current track, or `None` once it has run its course.
    /// Expired entries are dropped on read so the map can't grow unbounded.
    pub fn get(&self, user_id: i64) -> Option<(NowPlaying, u64)> {
        let now = now_secs();
        let mut map = self.0.write().ok()?;
        map.retain(|_, np| np.is_live(now));
        let np = map.get(&user_id)?;
        let position = match np.duration {
            Some(d) => np.position_at(now).min(d),
            None => np.position_at(now),
        };
        Some((np.clone(), position))
    }
}

/// GET /api/v1/now-playing → `{ playing, track?, position? }`
async fn now_playing(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.now_playing.get(user.user_id) {
        Some((track, position)) => (StatusCode::OK, Json(serde_json::json!({
            "playing": true,
            "position": position,
            "track": track,
        }))).into_response(),
        None => (StatusCode::OK, Json(serde_json::json!({ "playing": false }))).into_response(),
    }
}

pub fn create_now_playing_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/now-playing", get(now_playing))
}
