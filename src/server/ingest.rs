use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use crate::engine::enrichment::enrich_play;
use crate::models::Play;
use super::auth::AuthUser;
use super::models::*;
use super::AppState;

/// POST /1/submit-listens
///
/// Accepts the standard ListenBrainz payload format. Any existing LB-compatible
/// client (Pano Scrobbler, Web Scrobbler, multi-scrobbler, mpdscribble) works
/// out of the box.
pub async fn submit_listens(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SubmitListensRequest>,
) -> impl IntoResponse {
    let listen_count = body.payload.len();

    // Validate payload size
    if listen_count == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                code: 400,
                error: "Payload must contain at least one listen".into(),
            }),
        )
            .into_response();
    }

    if body.listen_type == ListenType::PlayingNow && listen_count > 1 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                code: 400,
                error: "playing_now must contain exactly one listen".into(),
            }),
        )
            .into_response();
    }

    if listen_count > 1000 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                code: 400,
                error: "Maximum 1000 listens per request".into(),
            }),
        )
            .into_response();
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let mut accepted = 0u32;

    for listen in &body.payload {
        // Resolve timestamp
        let timestamp = match body.listen_type {
            ListenType::PlayingNow => now,
            _ => match listen.listened_at {
                Some(ts) => ts,
                None => {
                    warn!("Listen missing listened_at for non-playing_now type, skipping");
                    continue;
                }
            },
        };

        let meta = &listen.track_metadata;
        let info_ref = meta.additional_info.as_ref();

        // Determine source_id — use submission client + timestamp for uniqueness
        let submission_client = info_ref
            .and_then(|ai| ai.submission_client.as_deref())
            .unwrap_or("unknown");
        let source_id = format!(
            "ingest-{}-{}-{}",
            user.user_id, timestamp, &meta.track_name
        );

        let duration = info_ref
            .and_then(|ai| ai.duration_ms)
            .map(|ms| (ms / 1000) as u64);

        let track_number = info_ref.and_then(|ai| ai.track_number);

        // Pre-populate MBIDs from mbid_mapping if the client provided them
        let mbid_mapping = meta.mbid_mapping.as_ref();
        let mbid_recording = mbid_mapping.and_then(|m| m.recording_mbid.clone());
        let mbid_release = mbid_mapping.and_then(|m| m.release_mbid.clone());
        let mbid_artist = mbid_mapping.and_then(|m| m.artist_mbids.clone());
        let caa_id = mbid_mapping.and_then(|m| m.caa_id);
        let caa_release_mbid = mbid_mapping.and_then(|m| m.caa_release_mbid.clone());

        let mut play = Play {
            title: meta.track_name.clone(),
            artist: meta.artist_name.clone(),
            album: meta.release_name.clone(),
            timestamp: timestamp as u64,
            duration,
            track_number,
            source_id,
            source_name: format!("ingest:{}", submission_client),
            mbid_recording,
            mbid_release,
            mbid_artist,
            artists: None,
            mbid_release_group: None,
            caa_id,
            caa_release_mbid,
        };

        // Enrich with MusicBrainz if no MBIDs were provided by the client
        if play.mbid_recording.is_none() {
            let album_hint = play.album.clone();
            enrich_play(&state.mb_client, &mut play, album_hint.as_deref()).await;
        }

        match body.listen_type {
            ListenType::PlayingNow => {
                // Now-playing is stateless — forward to sinks, don't store
                debug!(
                    "Now playing (ingest): {} - {} [user: {}]",
                    play.artist, play.title, user.username
                );
                for sink in state.sinks.iter() {
                    if let Some(lb_sink) = sink
                        .as_any()
                        .downcast_ref::<crate::sinks::ListenBrainzSink>()
                    {
                        if let Err(e) = lb_sink.submit_now_playing(&play).await {
                            error!("Failed to forward now playing to {}: {}", sink.name(), e);
                        }
                    }
                }
                accepted += 1;
            }
            ListenType::Single | ListenType::Import => {
                // Store for the engine's flush_pending to dispatch to all sinks
                match state.db.save_scrobble(user.user_id, &play).await {
                    Ok(true) => {
                        info!(
                            "📥 Ingested: {} - {} [user: {}, client: {}]",
                            play.artist, play.title, user.username, submission_client
                        );
                        accepted += 1;
                    }
                    Ok(false) => {
                        debug!(
                            "Duplicate skipped: {} - {} [user: {}]",
                            play.artist, play.title, user.username
                        );
                        accepted += 1; // LB counts dupes as accepted
                    }
                    Err(e) => {
                        error!("Database error saving ingested listen: {}", e);
                    }
                }
            }
        }
    }

    info!(
        "Ingest complete: {}/{} listens accepted [user: {}, type: {:?}]",
        accepted,
        listen_count,
        user.username,
        body.listen_type
    );

    (StatusCode::OK, Json(SubmitListensResponse { status: "ok".into() })).into_response()
}
