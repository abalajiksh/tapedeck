use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use super::auth::IngestAuth;
use super::models::*;
use super::now_playing::NowPlaying;
use super::AppState;
use crate::engine::enrichment::enrich_play;
use crate::models::{AudioQuality, ListenDisposition, Play};

#[derive(Deserialize)]
pub struct TokenQuery {
    token: Option<String>,
}

/// GET /1/validate-token — the handshake ListenBrainz clients (Pano Scrobbler,
/// multi-scrobbler, Web Scrobbler…) use to check a token before submitting.
///
/// Deliberately mirrors ListenBrainz's contract rather than Tapedeck's own
/// conventions: the token may arrive in the `Authorization` header *or* as a
/// `?token=` query parameter, and an invalid token is reported as HTTP 200 with
/// `valid: false` — not a 401. Clients branch on that field, so returning an
/// auth error here makes them report a server fault instead of a bad token.
pub async fn validate_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<TokenQuery>,
) -> impl IntoResponse {
    let header_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Token ").or_else(|| h.strip_prefix("token ")))
        .map(str::trim);

    let token = header_token
        .map(str::to_string)
        .or_else(|| q.token.map(|t| t.trim().to_string()))
        .unwrap_or_default();

    if token.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "error": "You need to provide an Authorization header.",
            })),
        )
            .into_response();
    }

    match state.db.validate_token(&token).await {
        Ok(Some(t)) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "code": 200,
                "message": "Token valid.",
                "valid": true,
                "user_name": t.user.username,
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "code": 200,
                "message": "Token invalid.",
                "valid": false,
            })),
        )
            .into_response(),
        Err(e) => {
            error!("Token validation error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": 500,
                    "error": "Internal server error",
                })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct SubmitQuery {
    /// `?dry_run=1` — resolve everything, store nothing, forward nothing.
    dry_run: Option<String>,
}

/// Accepts `1`, `true`, `yes`, and a bare `?dry_run` — a plugin author trying
/// this by hand shouldn't have to guess which spelling we wanted.
fn truthy(v: &Option<String>) -> bool {
    match v {
        Some(s) => {
            let s = s.trim();
            s.is_empty() || matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
        }
        None => false,
    }
}

/// POST /1/submit-listens
pub async fn submit_listens(
    State(state): State<Arc<AppState>>,
    IngestAuth { user, token_chain_id, token_id }: IngestAuth,
    Query(q): Query<SubmitQuery>,
    Json(body): Json<SubmitListensRequest>,
) -> impl IntoResponse {
    let dry_run = truthy(&q.dry_run);
    let listen_count = body.payload.len();

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
            Json(ErrorResponse { code: 400, error: "Maximum 1000 listens per request".into() }),
        )
            .into_response();
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let mut accepted = 0u32;
    let mut duplicate = 0u32;
    let mut rejected: Vec<RejectedListen> = Vec::new();
    let mut previews: Vec<DryRunListen> = Vec::new();
    let mut failed = 0u32; // listens we could not persist (DB error) — triggers a retryable response

    for (index, listen) in body.payload.iter().enumerate() {
        let timestamp = match body.listen_type {
            ListenType::PlayingNow => now,
            _ => match listen.listened_at {
                Some(ts) => ts,
                None => {
                    warn!("Listen missing listened_at, skipping");
                    rejected.push(RejectedListen { index, reason: "missing listened_at".into() });
                    continue;
                }
            },
        };

        let meta = &listen.track_metadata;

        // Reject malformed listens (no retry — it's bad client data, not our fault)
        if meta.artist_name.trim().is_empty() || meta.track_name.trim().is_empty() {
            warn!("Listen missing artist/track, skipping");
            rejected
                .push(RejectedListen { index, reason: "missing artist_name or track_name".into() });
            continue;
        }

        let info_ref = meta.additional_info.as_ref();

        let submission_client =
            info_ref.and_then(|ai| ai.submission_client.as_deref()).unwrap_or("unknown");

        let source_id = format!("ingest-{}-{}-{}", user.user_id, timestamp, meta.track_name);

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
            mbid_recording,
            mbid_release,
            mbid_artist,
            artists: None,
            mbid_release_group: None,
            caa_id,
            caa_release_mbid,
        };

        // MusicBrainz enrichment is NOT done inline for stored listens — it's a
        // rate-limited network call, so doing it per-listen would make a bulk
        // submit take minutes and risk being killed on shutdown. Stored listens
        // are saved immediately and enriched by the engine's flush loop, which
        // writes the MBIDs back. (Now-playing is enriched below since it's a
        // single track forwarded right away.)

        // ── Phase 2: Extract quality metadata ──
        let mut quality = extract_audio_quality(info_ref);
        if quality.has_data() {
            quality.compute_score();
        }
        let quality_opt = if quality.has_data() { Some(&quality) } else { None };

        // ── Phase 2: Resolve device ──
        // `upsert_device` writes (it bumps `last_seen` and `total_listens`), so
        // a dry run looks the device up instead. Asking what would happen must
        // not itself be a listen.
        let device_id = if let Some(td) = info_ref.and_then(|ai| ai.tapedeck_device.as_ref()) {
            if let Some(ref mid) = td.machine_id {
                if dry_run {
                    state.db.find_device(user.user_id, mid).await.ok().flatten()
                } else {
                    match state
                        .db
                        .upsert_device(
                            user.user_id,
                            mid,
                            td.player_name.as_deref(),
                            td.platform.as_deref(),
                            td.player_name.as_deref(),
                        )
                        .await
                    {
                        Ok(id) => Some(id),
                        Err(e) => {
                            debug!("Device upsert failed: {}", e);
                            None
                        }
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // ── Phase B: Record the output device (auto-learn bindings) ──
        // Every distinct output we see is recorded so the UI can surface
        // unmapped ones for one-tap assignment.
        let output_dev = info_ref
            .and_then(|ai| ai.tapedeck_device.as_ref())
            .and_then(|td| td.output_device.as_ref());
        let output_iface = info_ref
            .and_then(|ai| ai.tapedeck_device.as_ref())
            .and_then(|td| td.interface.as_ref().or(td.output_type.as_ref()));
        if let Some(dev) = output_dev {
            if !dry_run {
                let _ = state
                    .db
                    .record_output(user.user_id, dev, output_iface.map(|s| s.as_str()))
                    .await;
            }
        }

        // ── Resolve chain (layered, first match wins) ──
        //   1. explicit chain name from the client
        //   2. output-device binding (auto-learned / user-set)
        //   3. the submitting token's default chain
        //   4. source device's default chain
        //
        // The token layer sits below the binding because a binding is evidence
        // about *this* listen (what it actually came out of) while the token is
        // a standing assumption about the app. It sits above the device default
        // because one machine can run several players — a token per app is the
        // finer-grained statement, and the more deliberate one: you picked the
        // chain when you issued the token.
        let explicit_chain_name =
            info_ref.and_then(|ai| ai.tapedeck_chain.as_ref()).and_then(|tc| tc.chain_id.as_ref());
        // `chain_source` names the rung that won, which is only reported by the
        // dry run — but it is the single most useful thing to see there, since
        // a chain arriving from the wrong rung looks identical to one arriving
        // from the right one.
        let (chain_id, chain_source): (Option<i64>, &'static str) =
            if let Some(name) = explicit_chain_name {
                match state.db.get_chain_by_name(user.user_id, name).await {
                    Ok(Some(id)) => (Some(id), "explicit"),
                    Ok(None) => {
                        debug!("Unknown chain name from client: {}", name);
                        (None, "none")
                    }
                    Err(e) => {
                        debug!("Chain lookup failed: {}", e);
                        (None, "none")
                    }
                }
            } else if let Some(binding) = match output_dev {
                Some(dev) => state.db.resolve_binding(user.user_id, dev).await.ok().flatten(),
                None => None,
            } {
                (Some(binding), "output_binding")
            } else if let Some(tc) = token_chain_id {
                (Some(tc), "token_default")
            } else if let Some(dev) = device_id {
                match state.db.get_device_default_chain(user.user_id, dev).await.ok().flatten() {
                    Some(cid) => (Some(cid), "device_default"),
                    None => (None, "none"),
                }
            } else {
                (None, "none")
            };

        // ── Phase 2: Listening context from chain default ──
        // Fetched once: the context and the chain's name both come off the same
        // row, and the dry run wants the name.
        let chain = match chain_id {
            Some(cid) => state.db.get_chain(user.user_id, cid).await.ok().flatten(),
            None => None,
        };
        let listening_context = chain.as_ref().map(|c| c.listening_context.as_str().to_string());
        let chain_name = chain.as_ref().map(|c| c.name.clone());

        if dry_run {
            let disposition = ListenDisposition {
                imported: body.listen_type == ListenType::Import,
                skipped: info_ref.and_then(|ai| ai.skipped).unwrap_or(false),
            };
            // Both dedup rules are read-only, so the preview can answer the
            // question a client most often has — "will this be swallowed?" —
            // rather than making them submit and find out.
            let duplicate =
                matches!(
                    state.db.exists(user.user_id, &play.source_id, &play.source_name).await,
                    Ok(true)
                ) || matches!(state.db.fuzzy_exists(user.user_id, &play).await, Ok(true));

            previews.push(DryRunListen {
                index,
                artist: play.artist.clone(),
                title: play.title.clone(),
                album: play.album.clone(),
                timestamp,
                quality_score: quality.quality_score,
                chain_id,
                chain_name,
                chain_source,
                device_id,
                listening_context,
                status_if_stored: disposition.status(),
                skipped: disposition.skipped,
                duplicate,
            });
            continue;
        }

        match body.listen_type {
            ListenType::PlayingNow => {
                // Enrich the single now-playing track inline so the forwarded
                // metadata is complete (it isn't stored, so nothing back-fills it).
                if play.mbid_recording.is_none() {
                    let album_hint = play.album.clone();
                    enrich_play(&state.mb_client, &mut play, album_hint.as_deref()).await;
                }
                debug!(
                    "Now playing (ingest): {} - {} [user: {}]",
                    play.artist, play.title, user.username
                );

                // Feed the deck. Clients submit `playing_now` at track start
                // and carry no position, so we start at 0 and let the registry
                // extrapolate from `duration`.
                let mut np = NowPlaying::from_play(&play, "ingest", quality_opt, 0);
                np.listening_context = listening_context.clone();
                state.now_playing.set(user.user_id, np);

                // Forward to the submitting user's own connected accounts only.
                let user_sinks =
                    crate::sinks::sinks_for_user(&state.db, user.user_id, &state.listenbrainz_url)
                        .await;
                crate::sinks::submit_now_playing(&user_sinks, &play).await;
                accepted += 1;
            }
            ListenType::Single | ListenType::Import => {
                // What this listen *is*, and therefore whether it is ever
                // forwarded. `import` is a backfill: the client is telling us
                // about something that already happened, and pushing that to
                // Last.fm rewrites a permanent public record. This used to be
                // hardcoded `'pending'` in `save_scrobble`, so every
                // plugin-driven backfill was scrobbled onward as if live.
                let disposition = ListenDisposition {
                    imported: body.listen_type == ListenType::Import,
                    skipped: info_ref.and_then(|ai| ai.skipped).unwrap_or(false),
                };

                match state
                    .db
                    .save_scrobble(
                        user.user_id,
                        &play,
                        quality_opt,
                        device_id,
                        chain_id,
                        listening_context.as_deref(),
                        Some(submission_client),
                        None, // pushed by a client, not pulled from a configured source
                        disposition,
                    )
                    .await
                {
                    Ok(true) => {
                        // Assign to a session, and — the part that was missing —
                        // point the listen back at it. `assign_session` has
                        // maintained `sessions.track_count` since Phase 2 while
                        // every `scrobbles.session_id` stayed NULL, so the
                        // sessions existed but had no contents.
                        let dur = play.duration.unwrap_or(180) as i64;
                        if let Ok(session_id) = state
                            .db
                            .assign_session(
                                user.user_id,
                                timestamp,
                                dur,
                                device_id,
                                chain_id,
                                quality.quality_score,
                                listening_context.as_deref().unwrap_or("unknown"),
                                1800,
                                token_id,
                            )
                            .await
                        {
                            let _ = state
                                .db
                                .link_scrobble_session(user.user_id, &play.source_id, session_id)
                                .await;
                        }

                        // Chain hours used to be accumulated here, and *only*
                        // here — so a listen polled from Plex, taken off the
                        // shelf or imported added nothing, and re-assigning a
                        // chain later moved no hours at all because the count
                        // had already been banked against whatever the chain was
                        // at the time. They are derived from the listens now
                        // (`db::CHAIN_COLUMNS`), which answers all of that and
                        // keeps the skip rule in one place rather than needing
                        // it repeated at every insert site.

                        info!(
                            "📥 Ingested{}: {} - {} [user: {}, client: {}, quality: {}]",
                            if disposition.skipped {
                                " (skipped)"
                            } else if disposition.imported {
                                " (backfill)"
                            } else {
                                ""
                            },
                            play.artist,
                            play.title,
                            user.username,
                            submission_client,
                            quality
                                .quality_score
                                .map(|s| format!("{:.0}", s))
                                .unwrap_or_else(|| "n/a".into()),
                        );
                        accepted += 1;
                    }
                    // Already held — by `source_id` or by the fuzzy window.
                    // Counted apart from `accepted` now: a client resending
                    // after a dropped connection is normal, but a client whose
                    // every listen comes back `duplicate` has a bug, and
                    // folding the two together hid that.
                    Ok(false) => {
                        duplicate += 1;
                    }
                    Err(e) => {
                        error!("Database error persisting listen: {}", e);
                        failed += 1;
                    }
                }
            }
        }
    }

    if dry_run {
        info!(
            "🔍 Dry run: {} listens resolved, {} rejected [user: {}, type: {:?}]",
            previews.len(),
            rejected.len(),
            user.username,
            body.listen_type
        );
        return (
            StatusCode::OK,
            Json(DryRunResponse {
                status: "ok".into(),
                dry_run: true,
                listens: previews,
                rejected,
            }),
        )
            .into_response();
    }

    info!(
        "Ingest complete: {} accepted, {} duplicate, {} rejected, {} failed of {} [user: {}, type: {:?}]",
        accepted, duplicate, rejected.len(), failed, listen_count, user.username, body.listen_type
    );

    // If any listen failed to persist, return a retryable error so the client
    // resends the batch — our dedup (source_id + fuzzy match) makes retries safe,
    // and this prevents silently losing listens on a transient DB error.
    //
    // Note this is separate from `rejected`, which is the *don't* retry list:
    // a rejected listen is malformed and resending it changes nothing.
    if failed > 0 {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                code: 503,
                error: format!(
                    "{} of {} listens could not be stored; please retry",
                    failed, listen_count
                ),
            }),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(SubmitListensResponse { status: "ok".into(), accepted, duplicate, rejected }),
    )
        .into_response()
}

/// Extract audio quality from Tapedeck extension fields.
fn extract_audio_quality(info: Option<&AdditionalInfo>) -> AudioQuality {
    let Some(ai) = info else {
        return AudioQuality::default();
    };
    let Some(ta) = &ai.tapedeck_audio else {
        return AudioQuality::default();
    };

    AudioQuality {
        format_type: ta.format_type.clone(),
        codec: ta.codec.clone(),
        bitrate: ta.bitrate,
        sample_rate: ta.sample_rate,
        bit_depth: ta.bit_depth,
        channels: ta.channels,
        container: ta.container.clone(),
        source_quality: ta.source_quality.clone(),
        is_lossless: ta.is_lossless,
        dsd_rate: ta.dsd_rate,
        dsd_multiplier: ta.dsd_multiplier,
        dsd_to_pcm_converted: ta.dsd_to_pcm_converted,
        delivery_codec: ta.delivery_codec.clone(),
        delivery_bitrate: ta.delivery_bitrate,
        delivery_sample_rate: ta.delivery_sample_rate,
        delivery_bit_depth: ta.delivery_bit_depth,
        is_transcoded: ta.is_transcoded,
        transcode_reason: ta.transcode_reason.clone(),
        quality_score: None, // computed after
    }
}
