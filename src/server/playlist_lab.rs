//! The Playlist Lab's vector space, served once per visit.
//!
//! **The server owns the space; the browser owns the geometry.** Everything
//! that is easy to get wrong lives here — the join to Every Noise through
//! `genre_key`, the hour-of-day bucketing in the user's own timezone, the
//! normalisation against the pool's own extremes — and it is all pure and
//! tested in `crate::playlist_lab`. What the browser does with the result is
//! nearest-neighbour search and path-walking over an array of ten floats, which
//! has to be interactive: the design drives it from sliders, and a round trip
//! per slider tick would make the whole screen feel broken.
//!
//! The browser needs the pool anyway — the phase-space plot draws every
//! candidate as a background dot — so sending it once and generating locally is
//! also one request instead of one per drag.
//!
//! Read the module docs on `crate::playlist_lab` before changing anything here.
//! The short version: the design's ten audio-feature axes do not exist and its
//! mockup faked them with a hash of the artist's name; these ten are real, six
//! are about the music and four are about the listener, and every track by one
//! artist shares its six Sound coordinates.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::error;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::openapi::Unauthorized;
use crate::playlist_lab::{self, DIMS};

/// How much history one build reads.
///
/// The scan is the expensive half and it is bounded rather than open: a listen
/// from eight years ago contributes to a track's play count and nothing else,
/// and the pool is capped long before that matters.
const SCAN_LIMIT: i64 = 120_000;

/// Default ceiling on pool size. Generous — the whole point of a candidate pool
/// is that it is bigger than the playlist — but bounded, because this is JSON
/// going over the wire to a browser.
const DEFAULT_CAP: usize = 3_000;

/// A track needs to have been played this many times to be a candidate, unless
/// it is loved.
///
/// One play is not evidence you want it again; a repeat is the cheapest signal
/// there is that a track is actually yours. Loved tracks bypass it entirely —
/// they are the design's own starting point.
const DEFAULT_MIN_PLAYS: i64 = 2;

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SpaceQuery {
    /// Plays a track needs to be in the pool. Lower it to reach further into
    /// the tail of the history.
    #[param(minimum = 1, maximum = 50, default = 2)]
    pub min_plays: Option<i64>,
    /// The most tracks to return, most-played first.
    #[param(minimum = 50, maximum = 10000)]
    pub cap: Option<usize>,
}

/// The pool, where each track sits, and what the axes mean.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct LabSpace {
    /// The ten axes, in the order every track's vector lists them.
    pub dims: Vec<LabDim>,
    /// One array per track rather than an object, to keep a large pool small:
    /// `[title, artist, album, plays, vector]`, the vector's coordinates
    /// rounded to three places.
    #[schema(value_type = Vec<Vec<Object>>)]
    pub tracks: Vec<serde_json::Value>,
    /// Indexes into `tracks` of the ones you love.
    pub loved: Vec<usize>,
    /// Tracks in the pool.
    pub pool: usize,
    /// Tracks considered before the cut.
    pub considered: usize,
    /// Tracks left out for having no genre the map places.
    pub unplaced: usize,
    pub min_plays: i64,
    pub cap: usize,
    /// Always `artist`: the Sound axes are reached through each track's
    /// artist, so every track by one artist shares them. Present so no client
    /// can present this as per-recording analysis.
    pub granularity: &'static str,
    pub note: &'static str,
}

/// One axis.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct LabDim {
    pub key: &'static str,
    pub label: &'static str,
    /// What the low end means.
    pub low: &'static str,
    /// What the high end means.
    pub high: &'static str,
    pub group: crate::playlist_lab::Group,
}

// GET /api/v1/playlist-lab — the pool, its coordinates, and what the axes mean.
/// The vector space the Playlist Lab generates from.
///
/// Session-only. Returns the candidate pool with a position for each track,
/// plus what every axis means.
///
/// **Read `granularity` and `note` before using this.** The space is ten
/// real dimensions, and they come from two different places:
///
/// * **Six are about the music** (`group: sound`), from the Every Noise
///   genre map — `x` (dense/atmospheric ↔ spiky/bouncy), `y` (organic ↔
///   mechanical/electric), the colour's three channels, and how far apart a
///   track's own genres sit.
/// * **Four are about the listener** (`group: habit`) — era, familiarity,
///   hour of day, recency — from their own history.
///
/// These are **not** per-recording audio features. Genres attach to
/// artists, so `granularity` is `artist` and every track by one artist
/// shares its six `sound` coordinates exactly; the `habit` axes are the
/// only thing separating them. Anything describing this as track-level
/// audio similarity is wrong. A client mixing the two groups without
/// weighting will find "similar" quietly means "played at the same hour".
///
/// Values are normalised 0–1 against **the pool's own extremes**, so
/// "newer" means newer than the rest of this collection. A track whose
/// genres have no place on the map is **dropped, not centred** — placing it
/// in the middle would invent a similarity it does not have — and the count
/// comes back as `unplaced`.
///
/// `tracks` is compact — `[title, artist, album, plays, [v0…v9]]` — because
/// on a 3,000-track pool the repeated key names would be most of the
/// payload. Answers `409` when the genre map has not been fetched, when
/// there is no history, or when nothing has been placed yet.
#[utoipa::path(
    get,
    path = "/api/v1/playlist-lab",
    tag = "Playlists",
    params(SpaceQuery),
    responses(
        (status = 200, description = "OK.", body = LabSpace),
        (status = 401, response = Unauthorized),
        (status = 409, description = "No genre map yet, no history, or nothing placed yet — the message says which.", body = ErrorResponse),
    )
)]
async fn space(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<SpaceQuery>,
) -> impl IntoResponse {
    let min_plays = q.min_plays.unwrap_or(DEFAULT_MIN_PLAYS).clamp(1, 50);
    let cap = q.cap.unwrap_or(DEFAULT_CAP).clamp(50, 10_000);

    // Without the map there is no Sound half of the space, and a lab with only
    // the Habit axes would be a different feature wearing this one's name. Say
    // so rather than serving four-dimensional vectors under a ten-dimensional
    // description.
    let map_ready = state.everynoise.summary().await.map(|s| s.genres > 0).unwrap_or(false);
    if !map_ready {
        return err(
            409,
            "The genre map hasn't been fetched yet — an admin can do it in Settings → Genre Map. \
             Without it there are no coordinates to place your listening on.",
        );
    }

    let rows = match state.db.playlist_lab_rows(user.user_id, SCAN_LIMIT).await {
        Ok(r) => r,
        Err(e) => {
            error!("Playlist lab scan failed: {e}");
            return err(500, "Could not read your listening history");
        }
    };
    if rows.is_empty() {
        return err(409, "No listening history yet — there is nothing to build a playlist from");
    }

    // Every genre name in play, joined to the map in one go. `coordinates_for`
    // keys on `genre_key`, which folds punctuation and accents — the difference
    // between "dance-pop" and "dance pop", and worth 10% more tags.
    let counts = state.db.genre_play_counts(user.user_id, None).await.unwrap_or_default();
    let names: Vec<String> = counts.into_iter().map(|(n, _)| n).collect();
    let coords = match state.everynoise.coordinates_for(&names).await {
        Ok(c) => c,
        Err(e) => {
            error!("Playlist lab map join failed: {e}");
            return err(500, "Could not read the genre map");
        }
    };

    // Loved recordings, as the same `artist\x01title` key the builder folds on.
    // `entity_key` is `kind|artist|name`, already lowercased — see the note on
    // `loveKey()` in the frontend, which must stay in step with it.
    let keep: Vec<String> = state
        .db
        .loved_keys(user.user_id)
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|k| k.strip_prefix("recording|"))
        .filter_map(|rest| rest.split_once('|'))
        .map(|(artist, title)| format!("{artist}\u{1}{title}"))
        .collect();

    let map_spread = state.everynoise.spread().await.unwrap_or(0.0);
    let tz = crate::server::scrobbles::user_tz_for(&state, user.user_id).await;
    let now = chrono::Utc::now().timestamp();

    let space = playlist_lab::build(&rows, &coords, map_spread, tz, now, min_plays, &keep, cap);

    if space.tracks.is_empty() {
        return err(
            409,
            "None of your listening has a genre with a place on the map yet — the genre backfill \
             runs newest-first over a few hours, so try again once it has caught up",
        );
    }

    // Compact, the same idiom the genre map's backdrop uses: an array per track
    // rather than an object, and coordinates rounded to three places. On a
    // 3,000-track pool that is the difference between a fast page and a
    // megabyte of repeated key names.
    let loved: std::collections::HashSet<String> = keep.into_iter().collect();
    let mut loved_idx: Vec<usize> = Vec::new();
    let tracks: Vec<serde_json::Value> = space
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let key =
                format!("{}\u{1}{}", t.artist.trim().to_lowercase(), t.title.trim().to_lowercase());
            if loved.contains(&key) {
                loved_idx.push(i);
            }
            let v: Vec<f64> =
                t.vector.iter().map(|x| (*x as f64 * 1000.0).round() / 1000.0).collect();
            serde_json::json!([t.title, t.artist, t.album, t.plays, v])
        })
        .collect();

    let dims: Vec<LabDim> = DIMS
        .iter()
        .map(|d| LabDim { key: d.key, label: d.label, low: d.low, high: d.high, group: d.group })
        .collect();

    (
        StatusCode::OK,
        Json(LabSpace {
            dims,
            tracks,
            loved: loved_idx,
            pool: space.tracks.len(),
            considered: space.considered,
            unplaced: space.unplaced,
            min_plays,
            cap,
            // Stated in the payload rather than only in the UI, so a client
            // reading this API cannot present it as something it is not.
            granularity: "artist",
            note: "Six axes come from the Every Noise genre map, reached through each track's \
                   artist — so every track by one artist shares them. Four come from your own \
                   listening. Nothing here is a per-recording audio analysis.",
        }),
    )
        .into_response()
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(space))]
pub struct Api;

pub fn create_playlist_lab_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/playlist-lab", get(space))
}
