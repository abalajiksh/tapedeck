use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use crate::engine::enrichment::enrich_play;
use crate::models::{AudioQuality, Play};
use super::auth::AuthUser;
use super::models::*;
use super::AppState;

/// POST /1/submit-listens
pub async fn submit_listens(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SubmitListensRequest>,
) -> impl IntoResponse {
    let listen_count = body.payload.len();

    if listen_count == 0 {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { code: 400, error: "Payload must contain at least one listen".into() })).into_response();
    }
    if body.listen_type == ListenType::PlayingNow && listen_count > 1 {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { code: 400, error: "playing_now must contain exactly one listen".into() })).into_response();
    }
    if listen_count > 1000 {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { code: 400, error: "Maximum 1000 listens per request".into() })).into_response();
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let mut accepted = 0u32;

    for listen in &body.payload {
        let timestamp = match body.listen_type {
            ListenType::PlayingNow => now,
            _ => match listen.listened_at {
                Some(ts) => ts,
                None => { warn!("Listen missing listened_at, skipping"); continue; }
            },
        };

        let meta = &listen.track_metadata;
        let info_ref = meta.additional_info.as_ref();

        let submission_client = info_ref
            .and_then(|ai| ai.submission_client.as_deref())
            .unwrap_or("unknown");

        let source_id = format!("ingest-{}-{}-{}", user.user_id, timestamp, &meta.track_name);

        let duration = info_ref.and_then(|ai| ai.duration_ms).map(|ms| (ms / 1000) as u64);
        let track_number = info_ref.and_then(|ai| ai.track_number);

        // Pre-populate MBIDs from mbid_mapping
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
            mbid_recording, mbid_release, mbid_artist,
            artists: None, mbid_release_group: None,
            caa_id, caa_release_mbid,
        };

        // Enrich with MusicBrainz if no MBIDs were provided
        if play.mbid_recording.is_none() {
            let album_hint = play.album.clone();
            enrich_play(&state.mb_client, &mut play, album_hint.as_deref()).await;
        }

        // ── Phase 2: Extract quality metadata ──
        let mut quality = extract_audio_quality(info_ref);
        if quality.has_data() {
            quality.compute_score();
        }
        let quality_opt = if quality.has_data() { Some(&quality) } else { None };

        // ── Phase 2: Resolve device ──
        let device_id = if let Some(td) = info_ref.and_then(|ai| ai.tapedeck_device.as_ref()) {
            if let Some(ref mid) = td.machine_id {
                match state.db.upsert_device(
                    user.user_id, mid,
                    td.player_name.as_deref(),
                    td.platform.as_deref(),
                    td.player_name.as_deref(),
                ).await {
                    Ok(id) => Some(id),
                    Err(e) => { debug!("Device upsert failed: {}", e); None }
                }
            } else { None }
        } else { None };

        // ── Phase 2: Resolve chain ──
        let chain_id = info_ref
            .and_then(|ai| ai.tapedeck_chain.as_ref())
            .and_then(|tc| tc.chain_id.as_ref())
            .and_then(|name| {
                // Chain IDs from the client are names — we'd need to look them up.
                // For now, just log it. Full resolution requires async lookup per-listen.
                debug!("Client specified chain: {}", name);
                None::<i64>
            });

        // ── Phase 2: Listening context from chain default ──
        let listening_context = if let Some(cid) = chain_id {
            match state.db.get_chain(user.user_id, cid).await {
                Ok(Some(c)) => Some(c.listening_context.as_str().to_string()),
                _ => None,
            }
        } else { None };

        match body.listen_type {
            ListenType::PlayingNow => {
                debug!("Now playing (ingest): {} - {} [user: {}]", play.artist, play.title, user.username);
                for sink in state.sinks.iter() {
                    if let Some(lb_sink) = sink.as_any().downcast_ref::<crate::sinks::ListenBrainzSink>() {
                        if let Err(e) = lb_sink.submit_now_playing(&play).await {
                            error!("Failed to forward now playing to {}: {}", sink.name(), e);
                        }
                    }
                }
                accepted += 1;
            }
            ListenType::Single | ListenType::Import => {
                match state.db.save_scrobble(
                    user.user_id, &play, quality_opt,
                    device_id, chain_id,
                    listening_context.as_deref(),
                    Some(submission_client),
                ).await {
                    Ok(true) => {
                        // Assign to a session
                        let dur = play.duration.unwrap_or(180) as i64;
                        let _ = state.db.assign_session(
                            user.user_id, timestamp, dur,
                            device_id, chain_id,
                            quality.quality_score,
                            listening_context.as_deref().unwrap_or("unknown"),
                            1800,
                        ).await;

                        info!("📥 Ingested: {} - {} [user: {}, client: {}, quality: {}]",
                            play.artist, play.title, user.username, submission_client,
                            quality.quality_score.map(|s| format!("{:.0}", s)).unwrap_or_else(|| "n/a".into()),
                        );
                        accepted += 1;
                    }
                    Ok(false) => { accepted += 1; } // dupe
                    Err(e) => error!("Database error: {}", e),
                }
            }
        }
    }

    info!("Ingest complete: {}/{} listens accepted [user: {}, type: {:?}]",
        accepted, listen_count, user.username, body.listen_type);

    (StatusCode::OK, Json(SubmitListensResponse { status: "ok".into() })).into_response()
}

/// Extract audio quality from Tapedeck extension fields.
fn extract_audio_quality(info: Option<&AdditionalInfo>) -> AudioQuality {
    let Some(ai) = info else { return AudioQuality::default() };
    let Some(ta) = &ai.tapedeck_audio else { return AudioQuality::default() };

    AudioQuality {
        format_type: ta.format_type.clone(),
        codec: ta.codec.clone(),
        bitrate: ta.bitrate,
        sample_rate: ta.sample_rate,
        bit_depth: ta.bit_depth,
        channels: ta.channels,
        container: ta.container.clone(),
        is_lossless: ta.is_lossless,
        dsd_rate: ta.dsd_rate,
        dsd_multiplier: ta.dsd_multiplier,
        delivery_codec: ta.delivery_codec.clone(),
        delivery_bitrate: ta.delivery_bitrate,
        delivery_sample_rate: ta.delivery_sample_rate,
        delivery_bit_depth: ta.delivery_bit_depth,
        is_transcoded: ta.is_transcoded,
        transcode_reason: ta.transcode_reason.clone(),
        quality_score: None, // computed after
    }
}
