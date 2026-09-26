use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};

use super::auth::IngestAuth;
use super::models::*;
use super::now_playing::{NowPlaying, Progress};
use super::AppState;
use crate::engine::enrichment::enrich_play;
use crate::models::{AudioQuality, ListenDisposition, Play, PlaybackReport};
use crate::openapi::{BadRequest, Unauthorized};
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TokenQuery {
    /// The token, for a client that cannot set a header. The
    /// `Authorization: Token …` header wins when both are sent.
    token: Option<String>,
}

/// ListenBrainz's answer, plus what this token may do when it is valid.
#[derive(Serialize, ToSchema)]
pub struct TokenValidation {
    /// Always 200.
    pub code: u16,
    pub message: &'static str,
    pub valid: bool,
    /// Present only when `valid`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// As granted, with `all` expanded: some of `submit`, `read`, `write`.
    /// Present only when `valid`. Matching is exact and **none implies
    /// another** — `submit` does not carry `read`, and `write` does not carry
    /// `read`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<&'static str>>,
    /// Rung 3 of the chain ladder. Show the user which chain their listens
    /// will be attributed to rather than leaving them to find out from their
    /// history. Present only when `valid`, and then null when the token has
    /// no default.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<i64>)]
    pub default_chain_id: Option<Option<i64>>,
    /// Whether this token's `listen_type: "import"` submissions are relayed
    /// onward — see `POST /1/submit-listens` for the two gates that still
    /// apply. Invisible from the client side and silent in its absence, which
    /// matters most for players that label every scrobble `import`. Present
    /// only when `valid`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imports_are_live: Option<bool>,
    /// So a client shipped against one version can tell which extensions this
    /// instance understands without parsing this document to find out. Present
    /// only when `valid`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_version: Option<&'static str>,
}

// GET /1/validate-token — the handshake ListenBrainz clients (Pano Scrobbler,
// multi-scrobbler, Web Scrobbler…) use to check a token before submitting.
//
// Deliberately mirrors ListenBrainz's contract rather than Tapedeck's own
// conventions: the token may arrive in the `Authorization` header *or* as a
// `?token=` query parameter, and an invalid token is reported as HTTP 200 with
// `valid: false` — not a 401. Clients branch on that field, so returning an
// auth error here makes them report a server fault instead of a bad token.
//
// **The Tapedeck keys past `user_name` are introspection**, and they exist
// because a client had no way to learn what its own token may do. Without them
// a settings screen has two bad options — hide features that might work, or
// offer features that 403 at the moment of use — in a screen whose entire job
// is telling the user their setup is right. Extending this endpoint rather
// than adding a second one is deliberate: real ListenBrainz clients branch on
// `valid` alone, so extra keys cannot break them, and it costs no round trip.
//
// They appear **only on the valid branch**. This endpoint answers whoever
// asks, so returning the server version beside `valid: false` would report the
// build to anyone who guessed at a token. A caller holding a real token is
// already inside; a caller without one learns nothing.
//
// Note this reports what the token *may* do, never any of the user's data —
// the scope grants themselves, not what they unlock. It widens no boundary:
// `submit` still reaches nothing but this endpoint and `submit-listens`.
/// Check a token, and ask what it may do.
///
/// An invalid token is **`200 {valid:false}`, never 401** — LB clients read
/// the field, and a 401 sends them into a refresh loop.
///
/// The token may arrive in the `Authorization` header **or** as `?token=`.
///
/// Everything past `user_name` is Tapedeck introspection, and it is
/// additive: real ListenBrainz clients branch on `valid` alone, so the
/// extra keys cannot break them, and putting them here rather than on a
/// second endpoint costs no extra round trip. They let a settings screen
/// say what this token can do instead of choosing between hiding features
/// that might work and offering features that 403 at the moment of use.
///
/// They appear **only when `valid` is true.** This endpoint answers whoever
/// asks, so reporting the build beside `valid: false` would hand it to
/// anyone guessing at tokens.
///
/// Note this reports the grants themselves, never any of the user's data.
/// It widens nothing: `submit` still reaches only this endpoint and
/// `submit-listens`.
#[utoipa::path(
    get,
    path = "/1/validate-token",
    tag = "ListenBrainz",
    security(("apiToken" = [])),
    params(TokenQuery),
    responses(
        (status = 200, description = "Valid or not — read `valid`.", body = TokenValidation),
        (status = 400, description = "No token at all, in the header or the query.",
            body = ErrorResponse),
    )
)]
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
            Json(ErrorResponse {
                code: 400,
                error: "You need to provide an Authorization header.".into(),
            }),
        )
            .into_response();
    }

    match state.db.validate_token(&token).await {
        Ok(Some(t)) => (
            StatusCode::OK,
            Json(TokenValidation {
                code: 200,
                message: "Token valid.",
                valid: true,
                user_name: Some(t.user.username.clone()),

                // ── Tapedeck introspection; see this function's note. ──
                scopes: Some(t.granted_scopes()),
                // Rung 3 of the four-rung chain ladder. A client can show which
                // chain its listens will be attributed to, and say so rather
                // than leaving the user to discover it from the history.
                // Nested so a token with no default still sends the key, as
                // `null`, rather than dropping it.
                default_chain_id: Some(t.default_chain_id),
                // Whether this client's `listen_type: "import"` is relayed
                // onward. Invisible from the client side and silent in its
                // absence, which matters most for the players that hardcode
                // `import` for every scrobble they send.
                imports_are_live: Some(t.imports_are_live),
                // So a plugin shipped against one version can tell which
                // extensions this instance understands without parsing the
                // OpenAPI document to find out.
                server_version: Some(env!("CARGO_PKG_VERSION")),
            }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(TokenValidation {
                code: 200,
                message: "Token invalid.",
                valid: false,
                user_name: None,
                scopes: None,
                default_chain_id: None,
                imports_are_live: None,
                server_version: None,
            }),
        )
            .into_response(),
        Err(e) => {
            error!("Token validation error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { code: 500, error: "Internal server error".into() }),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SubmitQuery {
    /// Resolve everything, **store nothing and forward nothing**, and report
    /// what would have happened. `1`, `true`, `yes`, `on` and a bare
    /// `?dry_run` all count.
    ///
    /// This is the endpoint to point a client at while it is being set up: it
    /// names the winning rung of the chain ladder, the quality score your
    /// fields produced, whether the listen would be forwarded or stored only,
    /// and whether dedup would swallow it — none of which is visible from a
    /// successful submit. The preview and the real path share one
    /// implementation, so it cannot promise something the real path would not
    /// do.
    ///
    /// Note it is genuinely read-only: a dry run looks a device up rather than
    /// upserting it, because asking what would happen must not itself be a
    /// listen.
    dry_run: Option<String>,
}

/// How recent a listen must be for a client trusted with `imports_are_live` to
/// have it forwarded onward. 24 hours.
///
/// The number separates two things that no field in the payload distinguishes:
/// a *live listen that arrived late* — a laptop that was offline, a client
/// flushing a queue after a restart — and a *chunk of someone's history*, which
/// is months or years old. A day is far more than any player's queue needs and
/// far less than any backfill, and Last.fm accepts scrobbles up to 14 days old
/// regardless, so nothing inside this window is anomalous to the sink either.
pub const IMPORT_LIVE_WINDOW_SECS: i64 = 24 * 60 * 60;

/// Whether an `import` submission is being stored only, or relayed onward.
///
/// This is the whole of the relay rule and it is one function because the dry
/// run and the real path must not be able to disagree — a preview that says
/// "will be forwarded" about a listen that then isn't is worse than no preview.
///
/// Two gates, and each covers the other's blind spot. The **token** gate is the
/// operator saying "this client batches its live scrobbles"; it exists because
/// nothing in the request can be trusted to say so — `submission_client` is
/// free text the client picks, and fooyin hardcodes `listen_type: "import"` for
/// every scrobble it has ever sent, so the wire is genuinely silent on the
/// question. The **freshness** gate then bounds how wrong that statement can be:
/// if a client marked live ever does a real backfill, its old timestamps fail
/// here and the history is not rewritten. Neither gate alone is enough — the
/// token alone re-opens the exact bug `ListenDisposition` was added to close.
fn import_is_live(listen_type: &ListenType, timestamp: i64, now: i64, token_is_live: bool) -> bool {
    if *listen_type != ListenType::Import {
        return false; // `single` is already live; `playing_now` is never stored.
    }
    // A future timestamp is a clock skew, not a backfill, so it is allowed —
    // but not unboundedly, or a client with a badly wrong clock could park a
    // scrobble years ahead of everything else on a public record.
    token_is_live && (now - timestamp) <= IMPORT_LIVE_WINDOW_SECS && (timestamp - now) <= 3600
}

/// Everyone a submission credits, or `None` when it names only one.
///
/// Two sources, and the order matters. `additional_info.artist_names` is
/// ListenBrainz's own structured field and is believed outright. Failing that,
/// a **semicolon-separated** `artist_name` is unpacked: `;` is the multi-value
/// separator in Vorbis comments and ID3, it is the separator Tapedeck's own
/// `artist_credit` already uses, and it is what a tagged classical library
/// actually contains — "Bach; Hilary Hahn".
///
/// Nothing else is split, and that restraint is the whole safety of it. `&` and
/// `,` appear inside real names — "Simon & Garfunkel", "Earth, Wind & Fire" —
/// so splitting on them would invent artists who do not exist and scatter one
/// band's listening across three chart rows. The sanitiser refuses to touch
/// those forms for exactly the same reason.
fn submitted_artists(artist_name: &str, info: Option<&AdditionalInfo>) -> Option<Vec<String>> {
    if let Some(names) = info.and_then(|i| i.artist_names.as_ref()) {
        let cleaned: Vec<String> =
            names.iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
        if cleaned.len() > 1 {
            return Some(cleaned);
        }
    }

    let split: Vec<String> =
        artist_name.split(';').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
    // One name is not a credit list. Leaving it `None` keeps `artist_credit`
    // unwritten, which is the existing rule for a single-artist listen.
    (split.len() > 1).then_some(split)
}

/// Accepts `1`, `true`, `yes`, `on` and a bare `?dry_run` — a plugin author
/// trying this by hand shouldn't have to guess which spelling we wanted.
fn truthy(v: &Option<String>) -> bool {
    match v {
        Some(s) => {
            let s = s.trim();
            s.is_empty() || matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
        }
        None => false,
    }
}

/// Submit listens.
///
/// Accepts a session cookie or any valid token — this is the one endpoint a
/// `submit`-scoped token reaches.
///
/// **`listen_type` decides forwarding**, and getting it wrong rewrites
/// someone's permanent public history. `single` and `playing_now` are live
/// and will be forwarded.
///
/// `import` is stored only and **not** forwarded *(since v0.26 — before
/// that, an import was forwarded like a live listen)* — unless the token
/// used to submit it is marked **`imports_are_live`**, which exists because
/// some players label every scrobble `import` and nothing on the wire
/// distinguishes that from a backfill. Even then two gates apply and both
/// must pass:
///
/// * the listen is **no more than 24h old**, so a genuine history import
///   from the same client still stays local; and
/// * its timestamp is **no more than 1h in the future**, so ordinary clock
///   skew is tolerated but a broken clock cannot park a scrobble years
///   ahead of everything else.
///
/// The flag is per token and invisible from the client side, which is why
/// `GET /1/validate-token` reports it — read it there rather than guessing
/// from whether listens appear at the far end.
///
/// Dedup is on `(user_id, source_id, source_name)` plus a fuzzy window, so
/// re-sending the same listen is safe. Max 1000 listens per request.
///
/// `additional_info` is free-form and **no field in it can fail a
/// submission**: an unusable value costs that field and the listen is still
/// stored. Unknown keys are ignored.
#[utoipa::path(
    post,
    path = "/1/submit-listens",
    tag = "ListenBrainz",
    security(("apiToken" = []), ("sessionCookie" = [])),
    params(SubmitQuery),
    request_body = SubmitListensRequest,
    responses(
        (status = 200, description = "Accepted, or — with `?dry_run` — resolved and discarded. \
            Per-listen results are reported rather than the whole batch failing: one bad row \
            must not lose the other 999.", body = SubmitOutcome),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 503, description = "One or more listens could not be stored. **Retryable** — \
            resend the batch; dedup makes that safe. Distinct from `rejected`, which is the \
            do-not-retry list.", body = ErrorResponse),
    )
)]
pub async fn submit_listens(
    State(state): State<Arc<AppState>>,
    IngestAuth { user, token_chain_id, token_id, imports_are_live }: IngestAuth,
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

        let source_id = ingest_source_id(
            user.user_id,
            timestamp,
            &meta.track_name,
            info_ref.and_then(|ai| ai.skipped).unwrap_or(false),
        );

        let duration = info_ref.and_then(|ai| ai.duration_ms).map(|ms| (ms / 1000) as u64);
        let track_number = info_ref.and_then(|ai| ai.track_number);

        // Pre-populate MBIDs. A submitting client puts them in `additional_info`
        // (that is what the LB submit spec says and what fooyin, Pano and
        // multi-scrobbler all do); `mbid_mapping` is the read-side shape, which
        // arrives when something replays an LB export through this endpoint.
        // Read both, `additional_info` first — it is the client speaking about
        // the file it just played, where a mapping is LB's own guess.
        //
        // Precedence makes blankness dangerous: a client that *sets the key* to
        // `""` would otherwise outrank a real MBID sitting in the mapping, and
        // an empty string is worse than a missing one — it satisfies every
        // `IS NOT NULL` the enrichment backfills queue on, so the row is filed
        // as identified and never looked up again. `present` is what stops a
        // key that carries nothing from counting as an answer.
        let present = |s: &Option<String>| {
            s.as_ref().map(|v| v.trim()).filter(|v| !v.is_empty()).map(str::to_string)
        };
        let mbid_mapping = meta.mbid_mapping.as_ref();
        let mbid_recording = info_ref
            .and_then(|ai| present(&ai.recording_mbid))
            .or_else(|| mbid_mapping.and_then(|m| present(&m.recording_mbid)));
        let mbid_release = info_ref
            .and_then(|ai| present(&ai.release_mbid))
            .or_else(|| mbid_mapping.and_then(|m| present(&m.release_mbid)));
        let mbid_artist =
            info_ref.and_then(|ai| ai.artist_mbids.clone()).filter(|v| !v.is_empty()).or_else(
                || mbid_mapping.and_then(|m| m.artist_mbids.clone()).filter(|v| !v.is_empty()),
            );
        let mbid_release_group = info_ref.and_then(|ai| present(&ai.release_group_mbid));
        // Only a tagging client ever knows these two, so there is no mapping or
        // backfill to fall back on: unread here is unread for good.
        let mbid_release_track = info_ref.and_then(|ai| present(&ai.release_track_mbid));
        let mbid_work = info_ref.and_then(|ai| present(&ai.work_mbid));
        let mbid_album_artist =
            info_ref.and_then(|ai| ai.album_artist_mbids.clone()).filter(|v| !v.is_empty());
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
            artists: submitted_artists(&meta.artist_name, info_ref),
            mbid_release_group,
            mbid_release_track,
            mbid_work,
            mbid_album_artist,
            caa_id,
            caa_release_mbid,
            // Taken straight from the bag when the client knew it. A tag read
            // off a file usually carries one, and a listen that arrives with an
            // ISRC never needs the rate-limited lookup `backfill_isrc` would
            // otherwise spend on it.
            isrc: info_ref
                .and_then(|ai| ai.isrc.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_uppercase),
        };

        // MusicBrainz enrichment is NOT done inline for stored listens — it's a
        // rate-limited network call, so doing it per-listen would make a bulk
        // submit take minutes and risk being killed on shutdown. Stored listens
        // are saved immediately and enriched by the engine's flush loop, which
        // writes the MBIDs back. (Now-playing is enriched below since it's a
        // single track forwarded right away.)

        // ── Phase 2: Extract quality metadata ──
        let quality = prepare_quality(info_ref);
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
                imported: body.listen_type == ListenType::Import
                    && !import_is_live(&body.listen_type, timestamp, now, imports_are_live),
                skipped: info_ref.and_then(|ai| ai.skipped).unwrap_or(false),
            };
            // Both dedup rules are read-only, so the preview can answer the
            // question a client most often has — "will this be swallowed?" —
            // rather than making them submit and find out.
            let duplicate = matches!(
                state.db.exists(user.user_id, &play.source_id, &play.source_name).await,
                Ok(true)
            ) || matches!(
                state.db.fuzzy_exists(user.user_id, &play, disposition.skipped).await,
                Ok(true)
            );

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

                // Feed the deck.
                //
                // The ListenBrainz submission format carries no position, so a
                // generic client's `playing_now` is wall-clock from the moment
                // it spoke — right until the listener pauses or seeks, wrong
                // from then on. That stays `Progress::Unknown` and is drawn as
                // the estimate it is.
                //
                // `tapedeck_playback` is the way out of that for a client that
                // can actually see its playhead. It is `Exact` rather than
                // `Reported`: a pushing client measures at the instant it
                // sends, so it has no upload lag to absorb and the registry's
                // staleness guard must not hold its own count against it — a
                // backward step from this source is the listener seeking.
                let playback = info_ref.and_then(|ai| ai.tapedeck_playback.as_ref());
                let progress = match playback.and_then(|p| p.position_secs()) {
                    Some(secs) => Progress::Exact(secs),
                    None => Progress::Unknown,
                };
                let mut np = NowPlaying::from_play(&play, "ingest", quality_opt, progress);
                // Without this the position counts forward through a pause and
                // `position_known` asserts it is a measurement — the "confident
                // and completely wrong position" made worse by being stated
                // with confidence. Pause is not decoration on this feature.
                np.paused = playback.is_some_and(|p| p.is_paused());
                np.listening_context = listening_context.clone();
                // The chain the ladder actually picked for this listen. Free
                // here — it was resolved above to attribute the scrobble — and
                // it is the only rung that represents a decision the listener
                // made rather than a fallback.
                np.chain_name = chain_name.clone();
                // The key as well as the name: Patch's live strip gates on the
                // chain being shared, and a name is not a key.
                np.chain_id = chain_id;
                let paused = np.paused;
                let changed = state.now_playing.set(user.user_id, np);

                // Forward to the submitting user's own connected accounts only,
                // and only on a real change of track.
                //
                // This used to forward unconditionally, which was harmless while
                // the only thing a client could say was "a new track started" —
                // one submission per track, so every call was a change anyway.
                // A position field invites the heartbeat that makes it useful,
                // and an unconditional forward would then aim a request at
                // Last.fm and ListenBrainz every few seconds for the whole
                // length of every track. Their now-playing is a one-shot that
                // expires on its own after the track's duration, so re-sending
                // is pure noise at someone else's servers — the reason
                // `NowPlayingRegistry::set` returns this flag at all.
                //
                // A paused deck is not now-playing either, and the same gate is
                // what the shelf's own heartbeat uses.
                if changed && !paused {
                    // A failed read costs one now-playing update, which the
                    // next track supersedes within minutes — so an empty answer
                    // is tolerable here in a way it is not on the flush path.
                    let user_sinks = crate::sinks::sinks_for_user(
                        &state.db,
                        user.user_id,
                        &state.listenbrainz_url,
                    )
                    .await
                    .unwrap_or_default();
                    crate::sinks::submit_now_playing(&user_sinks, &play).await;
                }
                accepted += 1;
            }
            ListenType::Single | ListenType::Import => {
                // What this listen *is*, and therefore whether it is ever
                // forwarded. `import` is a backfill by default: the client is
                // telling us about something that already happened, and pushing
                // that to Last.fm rewrites a permanent public record. This used
                // to be hardcoded `'pending'` in `save_scrobble`, so every
                // plugin-driven backfill was scrobbled onward as if live.
                //
                // `import_is_live` is the narrow exception — a client the
                // operator has marked as batching its *live* scrobbles, sending
                // something that happened today. See the function for why both
                // halves of that are required.
                let disposition = ListenDisposition {
                    imported: body.listen_type == ListenType::Import
                        && !import_is_live(&body.listen_type, timestamp, now, imports_are_live),
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
                        // A scrobble client has no idea which stylus is fitted; a
                        // variant is set by the shelf or by retagging afterwards.
                        None,
                        listening_context.as_deref(),
                        Some(submission_client),
                        None, // pushed by a client, not pulled from a configured source
                        disposition,
                    )
                    .await
                {
                    Ok(true) => {
                        // This track is accounted for, so it is not one the
                        // listener walked out on. Both halves are needed
                        // because a client may send the scrobble either side of
                        // announcing the next track, and both orderings are
                        // legal: `mark_banked` covers the scrobble arriving
                        // first, `cancel_pending_skip` the displacement having
                        // already been recorded.
                        if !disposition.skipped {
                            state.now_playing.mark_banked(user.user_id, &play.title, &play.artist);
                            state.now_playing.cancel_pending_skip(
                                user.user_id,
                                &play.title,
                                &play.artist,
                            );
                        }

                        // What the client saw of the playing itself. Written
                        // separately, and skipped entirely when the client said
                        // none of it — which is every client until one is built
                        // for this — so it costs no statement per listen in the
                        // ordinary case. See `set_listen_playback` for why it is
                        // not three more arguments on `save_scrobble`.
                        let session_info = info_ref.and_then(|ai| ai.tapedeck_session.as_ref());
                        let report = PlaybackReport {
                            listened_ms: info_ref.and_then(|ai| ai.listened_ms),
                            is_shuffle: session_info.and_then(|s| s.is_shuffle),
                            queue_source: session_info
                                .and_then(|s| s.queue_source.as_ref())
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty()),
                        };
                        if let Err(e) = state
                            .db
                            .set_listen_playback(
                                user.user_id,
                                &play.source_id,
                                &play.source_name,
                                &report,
                            )
                            .await
                        {
                            // The listen is stored and correct; this is a note
                            // about it. Losing the note must not fail the
                            // submission and have the client resend a listen we
                            // already hold.
                            debug!("Could not record playback detail: {}", e);
                        }

                        // Assign to a session, and — the part that was missing —
                        // point the listen back at it. `assign_session` has
                        // maintained `sessions.track_count` since Phase 2 while
                        // every `scrobbles.session_id` stayed NULL, so the
                        // sessions existed but had no contents.
                        //
                        // **A skip is not part of a session**, and this was the
                        // one path that put one there. `rebuild_sessions`
                        // filters `skipped = FALSE`, so a client-submitted skip
                        // got a `session_id` here and silently lost it at the
                        // next rebuild — the session's `track_count` and
                        // `total_duration` disagreeing with themselves either
                        // side of that. It mattered little while skips arrived
                        // only from imports, which never take this path; live
                        // sources write them now.
                        let dur = play.duration.unwrap_or(180) as i64;
                        if !disposition.skipped {
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
                                    crate::db::SESSION_GAP_SECS,
                                    token_id,
                                )
                                .await
                            {
                                let _ = state
                                    .db
                                    .link_scrobble_session(
                                        user.user_id,
                                        &play.source_id,
                                        session_id,
                                    )
                                    .await;
                            }
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

/// The id a submitted listen is deduplicated on.
///
/// **A skip and a listen of one track at one second are two events**, and this
/// used to be `ingest-<user>-<ts>-<track>` with nothing to tell them apart — so
/// they collided on `UNIQUE(user_id, source_id, source_name)` and whichever
/// arrived second was reported `duplicate` with **no row written**. Reported by
/// the Tapeout plugin's author as `Accepted 1 duplicate 1` about a batch
/// holding one of each; the guess there was that a skip writes no listen row,
/// which it does, and the cause is the id.
///
/// Plex has carried the same marker since v0.71.0. This differs in one way that
/// matters: Plex appends `skip-<now>` because *it* observed the skip and nothing
/// else can identify it, where a client supplies `listened_at` — so the suffix
/// here is a **constant**, and a resent skip still dedups on its own id rather
/// than minting a second listen on every retry.
///
/// A function rather than a `format!` at the call site for the reason
/// `consume_sessions` and `own_history_tracks` are functions: it decides
/// whether a listen is ever stored, and a rule like that has to be testable
/// without a server.
fn ingest_source_id(user_id: i64, timestamp: i64, track_name: &str, skipped: bool) -> String {
    format!("ingest-{}-{}-{}{}", user_id, timestamp, track_name, if skipped { "-skip" } else { "" })
}

/// What the client said about the audio, made fit to store and to score.
///
/// Three steps that were inline in the handler, which is where the untested
/// half of this lives — `submit_listens` has no test suite, so every statement
/// left loose in it is a statement a guard cannot reach. Folded into one
/// function the tests call, leaving exactly one line of wiring that the
/// compiler holds: delete it and `quality` does not exist.
///
/// * **Fold the spelling**, here as well as in `save_scrobble`, because the
///   score is computed from `format_type` *before* the row is stored and
///   `compute_score` compares against a lowercase literal — an uppercase
///   `"DSD"` would otherwise be scored as PCM and then stored correctly, which
///   is the worst of both.
/// * **Derive what the client did not say.** Plex and Jellyfin both call
///   `classify_codec`, so a listen off a media server arrives with a
///   `format_type` and an `is_lossless`; a listen off a scrobble client arrived
///   with a codec and neither, so a FLAC submission was stored as being of
///   unknown fidelity — outside the population the fidelity cards report on —
///   and scored as if the format were unknown.
/// * **Score it**, but only where there is anything to score.
fn prepare_quality(info: Option<&AdditionalInfo>) -> AudioQuality {
    let mut q = extract_audio_quality(info);
    q.normalise();
    q.fill_classification();
    if q.has_data() {
        q.compute_score();
    }
    q
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

#[cfg(test)]
mod relay_tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 24 * 60 * 60;

    /// The default, and the one that must never regress: an `import` from a
    /// client nobody has vouched for is stored and never forwarded, however
    /// recent it is. This is the whole of `ListenDisposition`'s reason to exist
    /// — a plugin backfill once rewrote a real Last.fm history.
    #[test]
    fn an_untrusted_import_is_never_forwarded_however_fresh() {
        for age in [0, 60, DAY - 1, DAY * 400] {
            assert!(
                !import_is_live(&ListenType::Import, NOW - age, NOW, false),
                "age {age} forwarded without the token being marked live"
            );
        }
    }

    /// fooyin's shape: `import` for a live scrobble, timestamped at track start,
    /// submitted seconds later.
    #[test]
    fn a_trusted_clients_recent_import_is_relayed() {
        assert!(import_is_live(&ListenType::Import, NOW - 300, NOW, true));
        assert!(import_is_live(&ListenType::Import, NOW, NOW, true));
        // A queue flushed after a few hours offline is still this listening.
        assert!(import_is_live(&ListenType::Import, NOW - 6 * 3600, NOW, true));
    }

    /// The rail. Marking a client live is a statement about how it batches, not
    /// a promise it will never import history — so its old timestamps must
    /// still fail closed.
    #[test]
    fn a_trusted_client_backfilling_history_is_still_not_forwarded() {
        assert!(!import_is_live(&ListenType::Import, NOW - DAY - 1, NOW, true));
        assert!(!import_is_live(&ListenType::Import, NOW - DAY * 30, NOW, true));
        assert!(!import_is_live(&ListenType::Import, NOW - DAY * 3650, NOW, true));
        // A wildly future timestamp is a broken clock, not a listen; a small
        // one is ordinary skew between two machines and is allowed.
        assert!(import_is_live(&ListenType::Import, NOW + 120, NOW, true));
        assert!(!import_is_live(&ListenType::Import, NOW + DAY * 365, NOW, true));
    }

    /// The flag only ever *widens* `import`. It must not touch the other two:
    /// `single` is already forwarded on its own merits and must not start
    /// depending on this, and `playing_now` is never stored at all.
    #[test]
    fn the_relay_switch_changes_nothing_about_the_other_listen_types() {
        for lt in [ListenType::Single, ListenType::PlayingNow] {
            for trusted in [true, false] {
                assert!(!import_is_live(&lt, NOW - 60, NOW, trusted));
            }
        }
    }

    /// **What a client says about the audio, as it is actually stored.**
    ///
    /// Against `prepare_quality` rather than its three steps, because the steps
    /// were inline in `submit_listens` and a test that called them itself
    /// proved nothing about the handler — removing one of them from the
    /// handler left this green, which is how the weakness was found. The
    /// remaining wiring is a single line the compiler holds.
    ///
    /// Three rules in one place:
    ///
    /// * **One format is one spelling.** fooyin reports `FLAC` where
    ///   ffmpeg-derived sources report `flac`, and the column stores the
    ///   literal. `format_type` is the one that was never merely cosmetic —
    ///   `FIDELITY_KNOWN`, the badge and `compute_score` all compare it to a
    ///   lowercase literal, so an uppercase `DSD` was filed as neither DSD nor
    ///   known **and scored as PCM**.
    /// * **A codec alone is enough to classify.** Ingest called neither
    ///   `classify_codec` nor anything like it, so a FLAC submission was stored
    ///   as being of unknown fidelity, outside the population the fidelity
    ///   cards report on.
    /// * **The client still wins.** One saying `is_lossless: false` about a
    ///   FLAC knows something the codec does not — a lossy file in a FLAC
    ///   container, or a transcode — so the derivation fills gaps and overrules
    ///   nobody.
    #[test]
    fn one_format_is_one_spelling_and_a_codec_alone_is_enough_to_classify() {
        // Built from the wire shape rather than a struct literal, so the field
        // names are exercised too — this is a deserialisation path, and a
        // `tapedeck_audio` key nothing reads is the failure mode next door.
        let q = prepare_quality(Some(
            &serde_json::from_value(serde_json::json!({
                "tapedeck_audio": {
                    "format_type": "DSD",
                    "codec": "  DSD64  ",
                    "container": "Ogg",
                    "delivery_codec": "AAC"
                }
            }))
            .unwrap(),
        ));
        assert_eq!(q.format_type.as_deref(), Some("dsd"));
        assert_eq!(q.codec.as_deref(), Some("dsd64"), "trimmed as well as folded");
        assert_eq!(q.container.as_deref(), Some("ogg"));
        assert_eq!(q.delivery_codec.as_deref(), Some("aac"));

        // **And the score was computed from the folded value.** A DSD64 direct
        // play scores 92; an unfolded `"DSD"` matches neither arm of
        // `compute_score` and falls through. Kept on its own fixture with no
        // `delivery_codec`, because that field means the audio was transcoded
        // and is correctly docked — the first version of this assertion mixed
        // the two and read as a scoring bug when it was a fixture describing an
        // AAC transcode of a DSD file.
        let q = prepare_quality(Some(
            &serde_json::from_value(serde_json::json!({
                "tapedeck_audio": {
                    "format_type": "DSD",
                    "codec": "dsd64",
                    "sample_rate": 2_822_400
                }
            }))
            .unwrap(),
        ));
        assert_eq!(q.dsd_multiplier, Some(64), "derived from the rate");
        assert_eq!(q.quality_score, Some(92.0), "an uppercase format scored down some other arm");

        // A codec and nothing else: both derived.
        let q = prepare_quality(Some(
            &serde_json::from_value(serde_json::json!({"tapedeck_audio": {"codec": "FLAC"}}))
                .unwrap(),
        ));
        assert_eq!(q.codec.as_deref(), Some("flac"));
        assert_eq!(q.format_type.as_deref(), Some("pcm"));
        assert_eq!(q.is_lossless, Some(true), "derived, where it used to be left unknown");

        // Stated by the client, and therefore kept.
        let q = prepare_quality(Some(
            &serde_json::from_value(serde_json::json!({
                "tapedeck_audio": {"codec": "flac", "is_lossless": false}
            }))
            .unwrap(),
        ));
        assert_eq!(q.is_lossless, Some(false), "the client knows something the codec does not");

        // An empty string is not a codec, and storing one would be a third
        // spelling of "we do not know" beside NULL and absent. With nothing to
        // derive from, both stay unset — `FIDELITY_KNOWN` reads a NULL
        // `is_lossless` as "we never knew", which is the population the
        // fidelity cards confess to.
        let q = prepare_quality(Some(
            &serde_json::from_value(serde_json::json!({"tapedeck_audio": {"codec": "   "}}))
                .unwrap(),
        ));
        assert_eq!(q.codec, None);
        assert_eq!(q.is_lossless, None);
        assert_eq!(q.format_type, None);
    }

    /// **A skip does not swallow the listen it sat next to.**
    ///
    /// The two are separate events, and with one id between them the second to
    /// arrive was answered `duplicate` and never stored. Idempotence is the
    /// other half: a client resending its batch must still dedup, which is why
    /// the marker is a constant rather than Plex's `-<now>`.
    #[test]
    fn a_skip_and_a_listen_of_one_track_at_one_second_are_two_ids() {
        let listen = ingest_source_id(1, 1_700_000_000, "LA Girls", false);
        let skip = ingest_source_id(1, 1_700_000_000, "LA Girls", true);
        assert_ne!(listen, skip, "one id between them loses whichever arrives second");

        // Resending either is the same id, so `exists` still catches a retry.
        assert_eq!(listen, ingest_source_id(1, 1_700_000_000, "LA Girls", false));
        assert_eq!(skip, ingest_source_id(1, 1_700_000_000, "LA Girls", true));

        // And the parts that always distinguished listens still do.
        assert_ne!(listen, ingest_source_id(2, 1_700_000_000, "LA Girls", false));
        assert_ne!(listen, ingest_source_id(1, 1_700_000_001, "LA Girls", false));
        assert_ne!(listen, ingest_source_id(1, 1_700_000_000, "Cheating", false));
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(submit_listens, validate_token))]
pub struct Api;
