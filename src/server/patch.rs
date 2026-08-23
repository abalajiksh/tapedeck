//! Patch — the actor layer (Phase 6, Step 1).
//!
//! Four unauthenticated endpoints that let another server find out who somebody
//! here is, and say something to them. Nothing is delivered outbound and there
//! is no UI: **local-first is the shipping order**, because the identity these
//! endpoints publish is the one thing in Phase 6 that other people's databases
//! end up holding copies of, and a copy cannot be corrected later.
//!
//! Everything is off until a user turns it on (`users.federation_enabled`,
//! Ashwin's call 2026-08-22), and an account that has not is indistinguishable
//! from one that does not exist — the membership-oracle rule `public_np` and
//! `pairing` already follow.
//!
//! The rules about *believing* a request live in `crate::patch::signatures` and
//! are pure. What is here is the impure half: resolving a key over the network,
//! the cache in front of that, and the shapes of the four responses.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::mcp::{is_private_host, public_origin};
use super::models::ErrorResponse;
use super::AppState;
use crate::db::Actor;
use crate::patch::{actor as doc, signatures};

/// A whole inbox request's budget for reaching a third party, not one HTTP
/// call's. **The handler's own deadline**, which is the rule this codebase
/// arrived at three times over: N times a per-request timeout is what actually
/// reaches the reverse proxy, and a request that outlives the proxy is answered
/// by the proxy's error page instead of by ours.
const FETCH_DEADLINE: Duration = Duration::from_secs(10);

/// One call inside that budget. Two fetches at most happen — the cached key
/// failing, then one refetch — so this leaves room for both.
const FETCH_TIMEOUT: Duration = Duration::from_secs(4);

/// An actor document is a page of JSON. Anything larger is either a mistake or
/// somebody feeding us a stream, and we are reading it because a stranger asked
/// us to.
const MAX_ACTOR_BYTES: usize = 256 * 1024;

/// How long a cached key is used without asking again. A key that has rotated
/// is *also* caught by the refetch-on-failure path below, so this is about
/// staleness rather than correctness — the failure path is what makes rotation
/// survivable, and this only stops us noticing a week late.
const ACTOR_CACHE_SECS: i64 = 7 * 24 * 3600;

pub fn create_patch_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users/:name", get(actor_document))
        .route("/users/:name/outbox", get(outbox))
        // Registered `post` only, so a GET here is a 405 from axum rather than
        // something this file has to remember to write.
        .route("/users/:name/inbox", post(inbox))
        .route("/users/:name/followers", get(followers))
        .route("/users/:name/following", get(following))
        .route("/.well-known/webfinger", get(webfinger))
        .route("/api/v1/profile/federation", get(get_federation).put(put_federation))
        // Step 2 — local Patch. Session-only, all of it.
        .route("/api/v1/patch/decks", get(decks))
        .route("/api/v1/patch/reel", get(reel))
        .route("/api/v1/patch", post(request))
        // `requests/:uuid` rather than `:uuid`, so every route under
        // `/api/v1/patch/` has a **literal** fourth segment. A parameter there
        // would sit beside `decks`, `reel` and `chains`, and matchit resolves
        // that ambiguity by its own rules rather than by anything obvious from
        // reading this list.
        .route("/api/v1/patch/requests/:uuid/settle", post(settle))
        .route("/api/v1/patch/chains/:id/shared", axum::routing::put(set_shared))
        // One registration, two methods. Two `.route()` calls on the same path
        // panic at startup rather than failing to compile.
        .route("/api/v1/patch/deck/:username", get(deck).delete(withdraw))
        // Step 3 — the screens.
        .route("/api/v1/patch/artists", get(artists))
        .route("/api/v1/patch/discover", get(discover))
        .route("/api/v1/patch/instance", get(instance))
        .route("/api/v1/patch/ops", get(ops))
        // Dubs.
        .route("/api/v1/patch/dub", post(dub))
        .route("/api/v1/patch/dubs", get(dubs))
        // Shared Spools.
        .route("/api/v1/patch/spool", get(spool).post(start_spool))
        .route("/api/v1/patch/spool/:uuid/join", post(join_spool))
        .route("/api/v1/patch/spool/:uuid/end", post(end_spool))
        .route("/api/v1/patch/captures", post(settle_captures))
        .route("/api/v1/patch/sittings", get(sittings).post(hand_over))
        .route("/api/v1/patch/sittings/:uuid", get(sitting).delete(withdraw_sitting))
        .route("/api/v1/patch/sittings/:uuid/seen", post(mark_seen))
}

// ── Step 2: patches, the inbox, and the Reel ─────────────────
//
// Local-only. Both ends of every patch here are rows in `users`, so no
// signature is involved and nothing is delivered — the remote half arrives with
// outbound delivery in Step 3, and `patches` already has the columns for it.
//
// **Session-only, all of it.** A token is held by a scrobble client, and no
// scrobble client has business approving who may read its owner's listening.
// Same line `/api/v1/sources`, the password endpoints and the two sharing
// switches sit on: `AuthUser`, so a token gets 401 and scopes never arise.

#[derive(Deserialize)]
struct PatchRequestBody {
    /// Who to patch into. A username rather than an id, because that is what
    /// the person typed and what a remote handle will resolve to in Step 3.
    username: String,
}

#[derive(Deserialize)]
struct SettleBody {
    /// **Required, not an `Option<bool>` with a default.** A body that merely
    /// misspelled the key must not be read as an answer — and here the two
    /// answers are "let them read my listening" and "no".
    accept: Option<bool>,
}

#[derive(Deserialize)]
struct SharedBody {
    shared: Option<bool>,
}

#[derive(Deserialize)]
struct ReelQuery {
    limit: Option<i64>,
    /// Keyset paging on the timestamp. Not an offset: the Reel grows from the
    /// top while it is being read, and an offset would repeat or skip rows as
    /// it shifted underneath.
    before: Option<i64>,
}

/// `GET /api/v1/patch/decks` — everyone on the instance, and where you stand
/// with them.
///
/// One call rather than three, because the screen shows one list of people with
/// a different button each: patched, pending, or not asked. Splitting it would
/// have the UI stitch three lists back together and invent the states in
/// between.
async fn decks(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    // **Sequential, not `try_join!`.** Running them concurrently takes three
    // pool connections for one request, and the pool has five — so two
    // simultaneous callers can hold two each and wait forever for a third.
    // Concurrency buys nothing here anyway: it is one SQLite file and three
    // fast reads.
    //
    // An amplifier in the 2026-08-23 outage rather than its cause — the cause
    // was three quadratic queries — but it is exactly the shape that turns
    // pressure into deadlock, and there is no reason to keep it.
    let all = match state.db.patchable_decks(user.user_id).await {
        Ok(v) => v,
        Err(e) => {
            warn!("📡 patch decks for {}: {e}", user.username);
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the decks");
        }
    };
    let out = match state.db.patches_out(user.user_id).await {
        Ok(v) => v,
        Err(e) => {
            warn!("📡 patch decks for {}: {e}", user.username);
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the decks");
        }
    };
    let inbound = match state.db.patches_in(user.user_id).await {
        Ok(v) => v,
        Err(e) => {
            warn!("📡 patch decks for {}: {e}", user.username);
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the decks");
        }
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "decks": all,
            "following": out,
            // Everything pointed at this user, pending and settled. The inbox
            // filters to `pending` itself rather than being served a
            // pre-filtered list, so "who is patched into me" and "who is
            // waiting" cannot disagree about the same row.
            "followers": inbound,
        })),
    )
        .into_response()
}

/// `POST /api/v1/patch` — ask to patch into a deck.
async fn request(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Json(body): Json<PatchRequestBody>,
) -> Response {
    let target = match state.db.patchable_user_id(&body.username).await {
        Ok(Some(id)) => id,
        Ok(None) => return refused(StatusCode::NOT_FOUND, "No such deck on this instance"),
        Err(e) => {
            warn!("📡 patch request: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not look that deck up");
        }
    };

    if target == user.user_id {
        return refused(StatusCode::BAD_REQUEST, "A deck cannot patch into itself");
    }

    match state.db.request_patch(user.user_id, target).await {
        Ok(uuid) => {
            info!("📡 {} asked to patch into {}", user.username, body.username);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "uuid": uuid,
                    // The state is read back rather than assumed: an existing
                    // accepted patch is left alone by `request_patch`, so
                    // reporting "pending" here would be a lie about a patch
                    // that is already live.
                    "state": state
                        .db
                        .patches_out(user.user_id)
                        .await
                        .ok()
                        .and_then(|v| v.into_iter().find(|p| p.user_id == target))
                        .and_then(|p| p.state)
                        .unwrap_or_else(|| "pending".into()),
                })),
            )
                .into_response()
        }
        Err(e) => {
            warn!("📡 patch request: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not record that")
        }
    }
}

/// `POST /api/v1/patch/:uuid/settle` — say yes or no.
///
/// Authorisation is inside `settle_patch`'s `WHERE`, not here — see its own
/// note. This handler only reports what happened.
async fn settle(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
    Json(body): Json<SettleBody>,
) -> Response {
    let Some(accept) = body.accept else {
        return refused(
            StatusCode::BAD_REQUEST,
            "`accept` is required — say true or false explicitly.",
        );
    };

    match state.db.settle_patch(&uuid, user.user_id, accept).await {
        // A patch that is not theirs, does not exist, or has already been
        // settled are one answer on purpose: the first would otherwise
        // confirm that a given patch id is real to somebody it does not
        // concern.
        Ok(false) => refused(StatusCode::NOT_FOUND, "No patch request waiting on you with that id"),
        Ok(true) => {
            info!("📡 {} {} a patch", user.username, if accept { "accepted" } else { "declined" });
            (
                StatusCode::OK,
                Json(serde_json::json!({ "state": if accept { "accepted" } else { "rejected" } })),
            )
                .into_response()
        }
        Err(e) => {
            warn!("📡 settling a patch: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

/// `DELETE /api/v1/patch/:username` — stop patching into someone.
///
/// The follower's own operation, and it takes a username rather than the
/// patch's uuid because that is what the row on the screen knows. Deliberately
/// **no sweep** of anything already delivered: those listens were shared at the
/// time under a switch their owner set, and a retroactive sweep would promise
/// an unpatching that the protocol cannot deliver.
async fn withdraw(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(username): Path<String>,
) -> Response {
    let target = match state.db.patchable_user_id(&username).await {
        Ok(Some(id)) => id,
        Ok(None) => return refused(StatusCode::NOT_FOUND, "No such deck on this instance"),
        Err(e) => {
            warn!("📡 unpatch: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not look that deck up");
        }
    };

    match state.db.unpatch(user.user_id, target).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "unpatched": true }))).into_response()
        }
        Ok(false) => refused(StatusCode::NOT_FOUND, "You are not patched into that deck"),
        Err(e) => {
            warn!("📡 unpatch: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

/// `PUT /api/v1/patch/chains/:id/shared` — publish a chain, or stop.
///
/// **The sharing unit is the chain, and this is the only switch there is.** It
/// lives here rather than on the chains router because it is a Patch decision
/// rather than a piece of gear configuration — but the Signal Chains page calls
/// exactly this endpoint, which is what makes "one state, two places" true
/// rather than aspirational.
async fn set_shared(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(chain_id): Path<i64>,
    Json(body): Json<SharedBody>,
) -> Response {
    let Some(shared) = body.shared else {
        return refused(
            StatusCode::BAD_REQUEST,
            "`shared` is required — say true or false explicitly.",
        );
    };

    match state.db.set_chain_shared(user.user_id, chain_id, shared).await {
        Ok(true) => {
            info!("📡 {} {} a chain", user.username, if shared { "shared" } else { "unshared" });
            (StatusCode::OK, Json(serde_json::json!({ "shared": shared }))).into_response()
        }
        Ok(false) => refused(StatusCode::NOT_FOUND, "No such chain"),
        Err(e) => {
            warn!("📡 sharing a chain: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

/// `GET /api/v1/patch/reel` — what the decks you patched into have been playing.
///
/// The one read in this application that returns another user's listens. Every
/// narrowing is in `Database::reel`'s query rather than here, deliberately: a
/// filter applied in a handler is one a second handler can forget.
async fn reel(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Query(q): Query<ReelQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(40).clamp(1, 200);
    match state.db.reel(user.user_id, limit, q.before).await {
        Ok(listens) => {
            // `next` is the oldest timestamp returned, so the client pages by
            // keyset. `None` when the page was short, which is how it knows to
            // stop rather than asking again forever.
            let next = (listens.len() as i64 == limit)
                .then(|| listens.last().map(|l| l.timestamp))
                .flatten();
            (StatusCode::OK, Json(serde_json::json!({ "listens": listens, "next": next })))
                .into_response()
        }
        Err(e) => {
            warn!("📡 reel for {}: {e}", user.username);
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the Reel")
        }
    }
}

// ── Step 3: the screens, Dubs and Spools ─────────────────────

#[derive(Deserialize)]
struct SinceQuery {
    /// Days back. The Instance tab's only control.
    days: Option<i64>,
}

/// `GET /api/v1/patch/artists` — artists across the decks you patched into.
async fn artists(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    match state.db.patch_artists(user.user_id, 120).await {
        Ok(artists) => {
            (StatusCode::OK, Json(serde_json::json!({ "artists": artists }))).into_response()
        }
        Err(e) => {
            warn!("📡 patch artists: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

/// `GET /api/v1/patch/discover` — everyone here, and what you have in common.
async fn discover(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    match state.db.discover_decks(user.user_id).await {
        Ok(decks) => (StatusCode::OK, Json(serde_json::json!({ "decks": decks }))).into_response(),
        Err(e) => {
            warn!("📡 patch discover: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

/// `GET /api/v1/patch/instance` — what this instance has been playing.
///
/// **Not narrowed by the caller's patches**, unlike everything else on this
/// router. The Reel answers "what have the decks I follow played"; this answers
/// "what is this instance listening to", which is a fact about the people who
/// chose to publish a chain and is the same for everyone who asks.
async fn instance(
    State(state): State<Arc<AppState>>,
    _user: super::auth::AuthUser,
    Query(q): Query<SinceQuery>,
) -> Response {
    let days = q.days.unwrap_or(7).clamp(1, 365);
    let since = chrono::Utc::now().timestamp() - days * 86_400;
    match state.db.instance_pulse(since).await {
        Ok(pulse) => (StatusCode::OK, Json(serde_json::json!({ "days": days, "pulse": pulse })))
            .into_response(),
        Err(e) => {
            warn!("📡 instance pulse: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

/// `GET /api/v1/patch/ops` — the admin Operations panel.
///
/// Reports **only what is recorded**. The design lists signature failures;
/// refusals go through `tracing` and are counted nowhere, and a tile showing an
/// invented number would be worse than one that is honest about where to look.
async fn ops(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    if !user.is_admin {
        return refused(StatusCode::FORBIDDEN, "Admins only");
    }
    match state.db.federation_ops().await {
        Ok(ops) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "ops": ops,
                // Read-only, deliberately. The design offers a picker with a
                // Freeze control; three vocabularies would mean three things to
                // maintain and a migration path between them for a decision
                // made once, so the verb is compiled in and this only reports
                // it.
                "vocabulary": {
                    "verb": "Create",
                    "object": "td:Listen",
                    "namespace": crate::patch::actor::TD_NS,
                    "frozen": true,
                },
                "delivery": false,
            })),
        )
            .into_response(),
        Err(e) => {
            warn!("📡 federation ops: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

/// `GET /api/v1/patch/deck/:username` — one deck, through an accepted patch.
async fn deck(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(username): Path<String>,
) -> Response {
    match state.db.deck_view(user.user_id, &username).await {
        // A deck that does not exist and one you are not patched into are the
        // same answer — the rule the whole of Patch follows.
        Ok(None) => refused(StatusCode::NOT_FOUND, "No such deck, or you are not patched into it"),
        Ok(Some(deck)) => (StatusCode::OK, Json(deck)).into_response(),
        Err(e) => {
            warn!("📡 deck view: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that deck")
        }
    }
}

#[derive(Deserialize)]
struct DubBody {
    to: String,
    kind: String,
    artist: String,
    name: Option<String>,
    mbid: Option<String>,
    reason: Option<String>,
    because: Option<String>,
}

/// `POST /api/v1/patch/dub` — pass a record to someone.
///
/// Goes through `crate_shelf::file`'s own guards where it can: `because` is
/// required, and a record already in their history is refused with the play
/// count. "You already have this" is Rediscovery under another name, and the
/// count is the half that tells the sender something.
async fn dub(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Json(body): Json<DubBody>,
) -> Response {
    if !matches!(body.kind.as_str(), "artist" | "album" | "track") {
        return refused(StatusCode::BAD_REQUEST, "kind must be artist, album or track");
    }
    let Some(because) = body.because.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return refused(
            StatusCode::BAD_REQUEST,
            "`because` is required — say what in their listening prompted this.",
        );
    };

    let to = match state.db.patchable_user_id(&body.to).await {
        Ok(Some(id)) => id,
        Ok(None) => return refused(StatusCode::NOT_FOUND, "No such deck on this instance"),
        Err(e) => {
            warn!("📡 dub: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not look that deck up");
        }
    };
    if to == user.user_id {
        return refused(StatusCode::BAD_REQUEST, "Dub it to somebody else");
    }

    // Already theirs? The crate's rule, applied to a person's suggestion.
    match state.db.already_heard(to, &body.kind, &body.artist, body.name.as_deref()).await {
        Ok(plays) if plays > 0 => {
            return refused(
                StatusCode::CONFLICT,
                format!(
                    "They already have that — {plays} {}. Dub them something they don't.",
                    if plays == 1 { "play" } else { "plays" }
                ),
            )
        }
        Ok(_) => {}
        Err(e) => {
            warn!("📡 dub: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not check their history");
        }
    }

    let rec = crate::db::NewRecommendation {
        kind: body.kind.clone(),
        artist: body.artist.clone(),
        name: body.name,
        mbid: body.mbid,
        reason: body.reason,
        because: Some(because.to_string()),
    };
    match state.db.dub(user.user_id, to, &rec, &user.username).await {
        Ok(id) => {
            info!("📡 {} dubbed {} to {}", user.username, rec.artist, body.to);
            (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(sqlx::Error::Protocol(msg)) => refused(StatusCode::FORBIDDEN, msg),
        Err(e) => {
            warn!("📡 dub: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not record that")
        }
    }
}

/// `GET /api/v1/patch/dubs` — records people handed you.
async fn dubs(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    match state.db.dubs_for(user.user_id).await {
        Ok(dubs) => (StatusCode::OK, Json(serde_json::json!({ "dubs": dubs }))).into_response(),
        Err(e) => {
            warn!("📡 dubs: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

#[derive(Deserialize)]
struct SpoolBody {
    title: Option<String>,
    /// Usernames to invite.
    with: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SettleCapturesBody {
    ids: Option<Vec<i64>>,
    accept: Option<bool>,
}

#[derive(Deserialize)]
struct HandOverBody {
    /// The sitting's uuid, from `GET /api/v1/sessions`.
    session: Option<String>,
    /// The username to hand it to.
    to: Option<String>,
    note: Option<String>,
    /// **Which of the two gestures this is, and it is required.**
    ///
    /// `spool` — they were in the room and neither of you started a sitting.
    /// The plays are offered as ordinary captures and enter their history when
    /// they accept, exactly as a live Shared Spool would have.
    ///
    /// `recommendation` — they were not there. They get a document of what you
    /// played, and nothing enters their history.
    ///
    /// No default. The two write different things to somebody else's permanent
    /// record, so guessing from an absent field is the one thing this must not
    /// do — the rule `bulk-chain` learned the expensive way.
    kind: Option<String>,
}

/// `GET /api/v1/patch/sittings` — sittings other people have handed you.
async fn sittings(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    match state.db.sittings_for(user.user_id).await {
        Ok(v) => (StatusCode::OK, Json(serde_json::json!({ "sittings": v }))).into_response(),
        Err(e) => {
            warn!("📡 sittings: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read those")
        }
    }
}

/// `POST /api/v1/patch/sittings` — hand one of your sittings to somebody.
///
/// **This copies nothing.** The recipient gets a document rendered from your
/// listens, which enters no history of theirs — they were not in the room, and
/// writing plays onto somebody's permanent record when they demonstrably did
/// not hear them is the error `ListenDisposition` exists to prevent.
async fn hand_over(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Json(body): Json<HandOverBody>,
) -> Response {
    let Some(session) = body.session.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return refused(StatusCode::BAD_REQUEST, "`session` is required");
    };
    let Some(to) = body.to.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return refused(StatusCode::BAD_REQUEST, "`to` is required — name a deck to hand it to");
    };
    let note = body.note.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if note.is_some_and(|n| n.chars().count() > 500) {
        return refused(StatusCode::BAD_REQUEST, "That note is too long — 500 characters at most");
    }
    let kind = match body.kind.as_deref() {
        Some("spool") | Some("recommendation") => body.kind.as_deref().unwrap(),
        _ => {
            return refused(
                StatusCode::BAD_REQUEST,
                "`kind` is required — \"spool\" if they were listening with you and the plays \
                 should reach their history, \"recommendation\" if they were not there and this \
                 is just something to look at.",
            )
        }
    };

    let to_id = match state.db.patchable_user_id(to).await {
        Ok(Some(id)) => id,
        Ok(None) => return refused(StatusCode::NOT_FOUND, "No such deck on this instance"),
        Err(e) => {
            warn!("📡 hand over: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not look that deck up");
        }
    };

    let result = if kind == "spool" {
        state.db.retro_spool_from_session(user.user_id, session, to_id).await.map(
            |(uuid, offered, withheld)| {
                info!(
                    "📡 {} reconstructed a sitting for {} — {offered} offered, {withheld} withheld",
                    user.username, to
                );
                serde_json::json!({
                    "kind": "spool", "uuid": uuid,
                    "offered": offered, "withheld": withheld,
                })
            },
        )
    } else {
        state.db.hand_over_session(user.user_id, session, to_id, note).await.map(|uuid| {
            info!("📡 {} passed a sitting to {}", user.username, to);
            serde_json::json!({ "kind": "recommendation", "uuid": uuid })
        })
    };

    match result {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(sqlx::Error::Protocol(msg)) => refused(StatusCode::FORBIDDEN, msg),
        Err(sqlx::Error::RowNotFound) => {
            refused(StatusCode::NOT_FOUND, "No sitting of yours with that id")
        }
        Err(e) => {
            warn!("📡 hand over: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not hand that over")
        }
    }
}

/// `GET /api/v1/patch/sittings/:uuid` — the document.
async fn sitting(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
) -> Response {
    match state.db.handed_sitting(&uuid, user.user_id).await {
        Ok(Some(d)) => (StatusCode::OK, Json(d)).into_response(),
        // Not yours to read and does not exist are the same answer, or the
        // first would confirm a uuid is real to somebody it does not concern.
        Ok(None) => refused(StatusCode::NOT_FOUND, "No sitting with that id"),
        Err(e) => {
            warn!("📡 sitting: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

/// `DELETE /api/v1/patch/sittings/:uuid` — withdraw one you handed over.
///
/// A real withdrawal, unlike un-patching: nothing was ever copied, so removing
/// the row removes the access. That stops being true the moment this
/// federates, which is why the storage note says the remote case must be a
/// snapshot.
async fn withdraw_sitting(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
) -> Response {
    match state.db.withdraw_session(&uuid, user.user_id).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "withdrawn": true }))).into_response()
        }
        Ok(false) => refused(StatusCode::NOT_FOUND, "No sitting of yours with that id"),
        Err(e) => {
            warn!("📡 withdraw sitting: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not withdraw that")
        }
    }
}

/// `POST /api/v1/patch/sittings/:uuid/seen` — clear it from the inbox.
///
/// Not an accept. A handed-over sitting writes nothing to a history, so there
/// is nothing to consent to — only a notification to stop showing.
async fn mark_seen(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
) -> Response {
    match state.db.mark_sitting_seen(&uuid, user.user_id).await {
        Ok(seen) => (StatusCode::OK, Json(serde_json::json!({ "seen": seen }))).into_response(),
        Err(e) => {
            warn!("📡 sitting seen: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

/// `GET /api/v1/patch/spool` — the sitting you are in, and what it has offered.
async fn spool(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    // Sequential — see `decks`. Two connections for one request against a pool
    // of five is pressure nobody is buying anything with.
    let active = match state.db.active_spool(user.user_id).await {
        Ok(v) => v,
        Err(e) => {
            warn!("📡 spool: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that");
        }
    };
    let captures = match state.db.spool_captures(user.user_id).await {
        Ok(v) => v,
        Err(e) => {
            warn!("📡 spool: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that");
        }
    };
    (StatusCode::OK, Json(serde_json::json!({ "spool": active, "captures": captures })))
        .into_response()
}

/// `POST /api/v1/patch/spool` — open a sitting.
async fn start_spool(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Json(body): Json<SpoolBody>,
) -> Response {
    let mut ids = Vec::new();
    for name in body.with.unwrap_or_default() {
        match state.db.patchable_user_id(&name).await {
            Ok(Some(id)) => ids.push(id),
            Ok(None) => return refused(StatusCode::NOT_FOUND, format!("No deck called {name}")),
            Err(e) => {
                warn!("📡 spool start: {e}");
                return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not look that deck up");
            }
        }
    }

    match state.db.start_spool(user.user_id, body.title.as_deref(), &ids).await {
        Ok(uuid) => {
            info!("📡 {} started a spool with {} others", user.username, ids.len());
            (StatusCode::OK, Json(serde_json::json!({ "uuid": uuid }))).into_response()
        }
        // Inviting somebody you are not patched with. The guard is in `db`, so
        // this arm reports it rather than deciding it — the same shape `dub`
        // uses for the identical rule.
        Err(sqlx::Error::Protocol(msg)) => refused(StatusCode::FORBIDDEN, msg),
        // The partial unique index, not a check this handler races.
        Err(e) if e.to_string().contains("UNIQUE") => {
            refused(StatusCode::CONFLICT, "You already have a sitting running")
        }
        Err(e) => {
            warn!("📡 spool start: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not start that")
        }
    }
}

/// `POST /api/v1/patch/spool/:uuid/join` — accept an invitation, or leave.
async fn join_spool(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
    Json(body): Json<SettleBody>,
) -> Response {
    let Some(join) = body.accept else {
        return refused(
            StatusCode::BAD_REQUEST,
            "`accept` is required — say true or false explicitly.",
        );
    };
    let state_name = if join { "joined" } else { "left" };
    match state.db.set_spool_membership(&uuid, user.user_id, state_name).await {
        Ok(true) => {
            (StatusCode::OK, Json(serde_json::json!({ "state": state_name }))).into_response()
        }
        Ok(false) => {
            refused(StatusCode::NOT_FOUND, "No running sitting you were invited to with that id")
        }
        Err(e) => {
            warn!("📡 spool join: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

/// `POST /api/v1/patch/spool/:uuid/end` — close it. The host's only.
async fn end_spool(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
) -> Response {
    match state.db.end_spool(&uuid, user.user_id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "ended": true }))).into_response(),
        Ok(false) => refused(StatusCode::NOT_FOUND, "No running sitting of yours with that id"),
        Err(e) => {
            warn!("📡 spool end: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

/// `POST /api/v1/patch/captures` — accept or decline what a sitting offered.
///
/// **The only path by which another deck can write a listen into your
/// history**, which is why it is explicit and per capture. Accepted plays land
/// as `imported`, so they are never forwarded onward: somebody else's deck
/// played it, and putting it on your permanent public record is not a decision
/// this feature has standing to make for you.
async fn settle_captures(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Json(body): Json<SettleCapturesBody>,
) -> Response {
    let Some(accept) = body.accept else {
        return refused(
            StatusCode::BAD_REQUEST,
            "`accept` is required — say true or false explicitly.",
        );
    };
    let ids = body.ids.unwrap_or_default();
    if ids.is_empty() {
        return refused(StatusCode::BAD_REQUEST, "`ids` is required — name the captures.");
    }

    match state.db.settle_captures(user.user_id, &ids, accept).await {
        Ok(n) => (StatusCode::OK, Json(serde_json::json!({ "settled": n }))).into_response(),
        Err(e) => {
            warn!("📡 settling captures: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// ── the switch ───────────────────────────────────────────────

#[derive(Deserialize)]
struct FederationRequest {
    /// **Not `Option<bool>` with a default.** A body that merely misspelled the
    /// key must not be read as an answer either way — this is the
    /// `bulk-chain` rule, in the one place where guessing wrong is *published*.
    enabled: Option<bool>,
}

/// What the owner sees about their own switch.
///
/// Reports the handle and the actor URI so the settings screen can show what
/// other people would actually type, rather than leaving them to assemble it.
/// The URI comes from the stored actor once one exists, and is otherwise the
/// one that *would* be minted — a preview, and it says so by being absent from
/// the `actor` field until it is real.
fn federation_body(actor: Option<&Actor>, username: &str, origin: &str, on: bool) -> Value {
    // **The handle's host comes from the stored actor when there is one, not
    // from today's origin.** Found by looking at the page: an instance whose
    // `PUBLIC_URL` had changed since the keypair was minted showed
    // `@ash@127.0.0.1:8791` above an actor at `deck.example` — two addresses,
    // disagreeing, with the one drawn larger being the one that does not work.
    // The actor URI is the address other servers actually hold, so the handle
    // has to be derived from it; today's origin is only the preview shown
    // before anything has been minted.
    let host = actor
        .and_then(|a| a.uri.split("://").nth(1).and_then(|rest| rest.split('/').next()))
        .map(str::to_string)
        .unwrap_or_else(|| origin.split("://").nth(1).unwrap_or_default().to_string());

    serde_json::json!({
        "enabled": on,
        "handle": format!("@{username}@{host}"),
        // Null until a keypair has been minted, which happens the first time
        // somebody asks. Shown as a preview rather than asserted as an address.
        "actor": actor.map(|a| a.uri.clone()),
        "would_be": doc::uri_for(origin, username),
        // Once this is set the username is load-bearing for another server and
        // must not be renamed.
        "frozen_at": actor.and_then(|a| a.frozen_at),
    })
}

async fn get_federation(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    headers: HeaderMap,
) -> Response {
    let origin = public_origin(&state, &headers);
    let on = state.db.discoverable_user_id(&user.username).await.ok().flatten().is_some();
    // Deliberately does **not** mint a keypair: looking at a settings page is
    // not a decision to federate.
    let actor = state.db.stored_actor(user.user_id).await.ok().flatten();
    (StatusCode::OK, Json(federation_body(actor.as_ref(), &user.username, &origin, on)))
        .into_response()
}

/// Turn discoverability on or off.
///
/// **Session-only, and permanently.** A token is held by a scrobble client, and
/// no scrobble client has business deciding that its owner is findable on the
/// fediverse. Same line `/api/v1/sources`, the password endpoints and the
/// now-playing switch sit on: `AuthUser`, so a token gets 401 and the question
/// of scopes never arises.
///
/// Switching **on** mints the keypair then and there rather than leaving it to
/// the first stranger's fetch. It costs a moment on a screen where somebody is
/// waiting anyway, and it means the settings page can show the real actor URI
/// immediately instead of a promise.
///
/// Switching **off** stops answering and deletes nothing. The keypair stays,
/// because destroying it is destroying the identity: anyone who has already
/// patched in holds a key that would never verify again, and turning the switch
/// back on would present them with a stranger wearing the same name.
async fn put_federation(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    headers: HeaderMap,
    Json(body): Json<FederationRequest>,
) -> Response {
    let Some(enabled) = body.enabled else {
        return refused(
            StatusCode::BAD_REQUEST,
            "`enabled` is required — say true or false explicitly.",
        );
    };

    let origin = public_origin(&state, &headers);

    if let Err(e) = state.db.set_federation_enabled(user.user_id, enabled).await {
        warn!("📡 could not set federation for {}: {e}", user.username);
        return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that");
    }

    let actor = if enabled {
        match state.db.actor_for_user(user.user_id, &origin).await {
            Ok(a) => {
                info!("📡 {} is now discoverable as {}", user.username, a.uri);
                Some(a)
            }
            Err(e) => {
                warn!("📡 could not mint an actor for {}: {e}", user.username);
                return refused(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not create an identity for this account",
                );
            }
        }
    } else {
        info!("📡 {} is no longer discoverable", user.username);
        state.db.stored_actor(user.user_id).await.ok().flatten()
    };

    (StatusCode::OK, Json(federation_body(actor.as_ref(), &user.username, &origin, enabled)))
        .into_response()
}

// ── responses ────────────────────────────────────────────────

/// JSON-LD, served as JSON-LD.
///
/// A server handed `application/json` here is entitled to treat the document as
/// untyped JSON rather than as an actor, and several do — with the result that
/// the actor "does not exist" while this server returns 200 and logs a success.
fn ld(content_type: &str, body: Value) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            // These are fetched by strangers and are per-actor, not per-caller.
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        Json(body),
    )
        .into_response()
}

/// The one 404, used for every "not here" in this file.
///
/// Written once so the shapes cannot drift apart: an unknown username, a user
/// who has not switched federation on and a disabled account must be the same
/// answer down to the byte, or the difference between them is readable from
/// outside.
fn nope() -> Response {
    (StatusCode::NOT_FOUND, Json(ErrorResponse { code: 404, error: "No such actor".into() }))
        .into_response()
}

fn refused(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ErrorResponse { code: status.as_u16(), error: message.into() })).into_response()
}

/// Does this caller want the app or the document?
///
/// A browser navigating here sends `text/html,…` and should get the SPA; a
/// remote server sends `application/activity+json` or `application/ld+json` and
/// must get the document. Anything else — `*/*` from curl, an absent header —
/// gets the document, because there is no `/users/:name` page in the app yet
/// and JSON is the useful answer to a bare probe.
///
/// The activity types are checked **first**: a client that sends both (and some
/// do, listing `text/html` as a low-priority fallback) is a server, not a
/// browser.
fn wants_html(headers: &HeaderMap) -> bool {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if accept.contains("activity+json") || accept.contains("ld+json") {
        return false;
    }
    accept.contains("text/html")
}

// ── GET /users/:name ─────────────────────────────────────────

async fn actor_document(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    if wants_html(&headers) {
        // The person, not a server. There is no deck page yet, so this is the
        // app's own 404 rather than ours — which is the right one to show a
        // human and the wrong one to show a machine.
        return crate::web_assets::serve_index();
    }

    let Some(actor) = resolve_local(&state, &name, &headers).await else {
        return nope();
    };

    ld(
        doc::ACTIVITY_JSON,
        doc::document(
            &actor.uri,
            &actor.username,
            actor.display_name.as_deref(),
            actor.bio.as_deref(),
            actor.avatar_url.as_deref(),
            &actor.public_key,
        ),
    )
}

// ── GET /users/:name/outbox ──────────────────────────────────

/// Empty until delivery exists, and served anyway.
///
/// Remote servers probe an outbox to decide whether an actor is worth
/// following, and a 404 here reads as a broken actor rather than a quiet one.
/// The access control is in the query — `visibility = 'public'` — because this
/// collection is fetched with no credential at all.
async fn outbox(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(actor) = resolve_local(&state, &name, &headers).await else {
        return nope();
    };

    let (total, payloads) = match state.db.public_outbox(actor.user_id, 40).await {
        Ok(v) => v,
        Err(e) => {
            warn!("📡 outbox for {}: {e}", actor.uri);
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the outbox");
        }
    };

    // Each row is the JSON-LD **as signed**. Reparsed rather than rebuilt: a
    // row that will not parse is a bug worth seeing as a gap in the collection
    // rather than as a 500 that hides the other thirty-nine.
    let items: Vec<Value> = payloads.iter().filter_map(|p| serde_json::from_str(p).ok()).collect();

    ld(doc::ACTIVITY_JSON, doc::outbox(&actor.uri, total, items))
}

// ── GET /users/:name/followers | /following ──────────────────

/// Who has patched into this deck, and who it has patched into.
///
/// **The count, not the roster.** ActivityPub allows a collection to state its
/// size without enumerating it, and that is the right default for a private
/// invite-only instance: who somebody has accepted is a fact about *other
/// people*, and this endpoint answers to anyone at all. Remote software uses
/// the number; the list is what would leak the shape of a household.
///
/// Both are served now rather than left out, because the actor document
/// advertises them — and advertising an endpoint that 404s is the thing Step 1
/// deliberately would not do.
async fn followers(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    collection(state, name, headers, true).await
}

async fn following(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    collection(state, name, headers, false).await
}

async fn collection(
    state: Arc<AppState>,
    name: String,
    headers: HeaderMap,
    followers: bool,
) -> Response {
    let Some(actor) = resolve_local(&state, &name, &headers).await else {
        return nope();
    };

    let total = match state.db.patch_count(actor.user_id, followers).await {
        Ok(n) => n,
        Err(e) => {
            warn!("📡 collection for {}: {e}", actor.uri);
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that collection");
        }
    };

    let id =
        if followers { doc::followers_uri(&actor.uri) } else { doc::following_uri(&actor.uri) };
    // Empty `items`: only the count leaves. `actor_collection` omits
    // `orderedItems` entirely rather than serving an empty array, which would
    // read as "this deck has followers and will not say who" — a different and
    // less honest claim than not enumerating at all.
    ld(doc::ACTIVITY_JSON, doc::actor_collection(&id, total, vec![]))
}

// ── GET /.well-known/webfinger ───────────────────────────────

#[derive(Deserialize)]
struct WebfingerQuery {
    resource: Option<String>,
}

/// The entry point for the whole protocol: it is what somebody typing a handle
/// into another server's search box hits first.
async fn webfinger(
    State(state): State<Arc<AppState>>,
    Query(q): Query<WebfingerQuery>,
    headers: HeaderMap,
) -> Response {
    // A missing or malformed `resource` is the caller's mistake and gets a 400.
    // A well-formed one naming somebody we do not publish is a 404 — the same
    // 404 an unknown name gets. The two statuses draw the line between "you
    // asked wrong" and "there is nobody here", and only the second one is a
    // fact about our users.
    let Some(resource) = q.resource.as_deref().filter(|r| !r.trim().is_empty()) else {
        return refused(StatusCode::BAD_REQUEST, "A `resource` parameter is required");
    };
    let Some((username, host)) = doc::parse_resource(resource) else {
        return refused(StatusCode::BAD_REQUEST, "Could not read that `resource`");
    };

    let origin = public_origin(&state, &headers);
    let our_host = origin.split("://").nth(1).unwrap_or_default();

    // A resource on somebody else's host is not ours to answer, and answering
    // it anyway would make this server claim actors it does not host.
    if !host.eq_ignore_ascii_case(our_host) {
        return nope();
    }

    let Some(actor) = resolve_local(&state, &username, &headers).await else {
        return nope();
    };

    ld(doc::JRD_JSON, doc::webfinger(&format!("{}@{}", actor.username, our_host), &actor.uri))
}

// ── POST /users/:name/inbox ──────────────────────────────────

/// Take a delivery, having established that it is really from who it says.
///
/// Nothing acts on what arrives yet — Step 2 is what turns a pending `Follow`
/// into a `patches` row. What this does is the part that cannot be added later
/// without a gap in the record: verify, and file.
async fn inbox(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(actor) = resolve_local(&state, &name, &headers).await else {
        return nope();
    };

    let Some(sig) =
        headers.get("signature").and_then(|v| v.to_str().ok()).and_then(signatures::parse)
    else {
        return refused(StatusCode::UNAUTHORIZED, signatures::Refusal::NoSignature.to_string());
    };

    // The key's URI is the actor's URI with the fragment cut off. Everything
    // this request is believed to be rests on the document at that address.
    let key_owner = sig.key_id.split('#').next().unwrap_or_default().to_string();
    if key_owner.is_empty() {
        return refused(StatusCode::UNAUTHORIZED, "The signature names no key");
    }

    let deadline = tokio::time::Instant::now() + FETCH_DEADLINE;
    let Some(remote) = resolve_remote_actor(&state, &key_owner, deadline, false).await else {
        return refused(StatusCode::UNAUTHORIZED, "Could not fetch the actor the signature names");
    };

    let path_and_query = format!("/users/{name}/inbox");
    let lookup = |want: &str| headers.get(want).and_then(|v| v.to_str().ok()).map(str::to_string);
    let now = chrono::Utc::now().timestamp();

    let mut verified = remote.public_key.as_deref().map(|pem| {
        signatures::verify_inbound("POST", &path_and_query, &body, now, lookup, pem, &sig)
    });

    // **A key that has rotated fails exactly like a forgery**, so one refetch
    // is what stops a peer becoming permanently unable to talk to us after they
    // rotate — a failure that would look like our bug and be invisible in their
    // logs. Only on `BadSignature`, and only once: a stale clock or a thin
    // coverage set is not going to be fixed by asking again, and retrying those
    // would turn any malformed request into an outbound fetch.
    if matches!(verified, Some(Err(signatures::Refusal::BadSignature)) | None) {
        if let Some(fresh) = resolve_remote_actor(&state, &key_owner, deadline, true).await {
            if let Some(pem) = fresh.public_key.as_deref() {
                verified = Some(signatures::verify_inbound(
                    "POST",
                    &path_and_query,
                    &body,
                    now,
                    lookup,
                    pem,
                    &sig,
                ));
            }
        }
    }

    match verified {
        Some(Ok(())) => {}
        Some(Err(refusal)) => {
            debug!("📡 inbox {}: refused — {refusal}", actor.uri);
            return refused(StatusCode::UNAUTHORIZED, refusal.to_string());
        }
        None => {
            return refused(
                StatusCode::UNAUTHORIZED,
                "That actor publishes no key we can check against",
            )
        }
    }

    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return refused(StatusCode::BAD_REQUEST, "The body is not JSON");
    };

    // **The signature says who *sent* this; `actor` claims who *wrote* it.**
    // They are different claims, and a delivery where they disagree is one
    // server relaying an activity attributed to somebody on another server —
    // which is how an impersonated Follow or Delete gets in. Filing it under
    // the sender would be wrong and filing it under the claim would be worse,
    // so it is refused.
    let claimed = payload.get("actor").and_then(as_uri);
    match claimed {
        Some(a) if a == remote.uri => {}
        Some(a) => {
            warn!("📡 inbox {}: signed by {} but attributed to {a}", actor.uri, remote.uri);
            return refused(
                StatusCode::UNAUTHORIZED,
                "The signing key does not belong to the actor this activity is attributed to",
            );
        }
        None => return refused(StatusCode::BAD_REQUEST, "The activity names no actor"),
    }

    // An activity with no `id` cannot be deduplicated, cannot be referred to by
    // a later `Undo` or `Delete`, and cannot be checked against a redelivery.
    // Minting one here would paper over all three.
    let Some(activity_uri) = payload.get("id").and_then(as_uri) else {
        return refused(StatusCode::BAD_REQUEST, "The activity has no `id`");
    };

    let kind = payload.get("type").and_then(|v| v.as_str());
    let object_ref = payload.get("object").and_then(as_uri);

    match state
        .db
        .record_inbox_activity(
            actor.user_id,
            remote.id,
            &activity_uri,
            kind,
            object_ref.as_deref(),
            &String::from_utf8_lossy(&body),
        )
        .await
    {
        // A redelivery is the *same* activity — senders retry — so this is 202
        // either way. Reporting a duplicate as an error is what makes a sender
        // retry harder.
        Ok(fresh) => {
            if fresh {
                info!(
                    "📡 {} → {}: {} from {}",
                    kind.unwrap_or("activity"),
                    actor.username,
                    activity_uri,
                    remote.uri
                );
            }
            StatusCode::ACCEPTED.into_response()
        }
        Err(e) => {
            warn!("📡 inbox {}: could not file — {e}", actor.uri);
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not file that activity")
        }
    }
}

/// ActivityStreams lets an object be a bare URI string *or* an embedded object
/// with an `id`. Both are on the wire and reading only the first is how a
/// perfectly ordinary `Undo` arrives looking malformed.
fn as_uri(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::to_string)
        .or_else(|| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
        .filter(|s| !s.is_empty())
}

// ── resolution ───────────────────────────────────────────────

/// The local actor for a username, minted on first need.
///
/// One helper for all four endpoints, so the discoverability rule is applied in
/// one place rather than remembered four times.
async fn resolve_local(state: &Arc<AppState>, name: &str, headers: &HeaderMap) -> Option<Actor> {
    let user_id = state.db.discoverable_user_id(name).await.ok().flatten()?;
    let origin = public_origin(state, headers);

    match state.db.actor_for_user(user_id, &origin).await {
        Ok(actor) => Some(actor),
        Err(e) => {
            warn!("📡 could not mint an actor for {name}: {e}");
            None
        }
    }
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            // A redirect chain is somebody else's server telling us where to
            // look. Two hops is enough for a host that has moved and few enough
            // that it cannot be walked somewhere interesting.
            .redirect(reqwest::redirect::Policy::limited(2))
            .user_agent(concat!("Tapedeck/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

/// Find a remote actor, from cache or from their server.
///
/// `force` skips the cache — used only for the single refetch after a signature
/// failed, which is what makes key rotation survivable.
async fn resolve_remote_actor(
    state: &Arc<AppState>,
    uri: &str,
    deadline: tokio::time::Instant,
    force: bool,
) -> Option<crate::db::RemoteActor> {
    if !force {
        if let Ok(Some(cached)) = state.db.remote_actor_by_uri(uri).await {
            let fresh = chrono::Utc::now().timestamp() - cached.fetched_at < ACTOR_CACHE_SECS;
            if fresh && cached.public_key.is_some() {
                return Some(cached);
            }
        }
    }

    let fetched = fetch_actor(uri, deadline).await?;

    let inbox = fetched.get("inbox").and_then(as_uri)?;
    let shared = fetched.get("endpoints").and_then(|e| e.get("sharedInbox")).and_then(as_uri);
    let key = fetched.get("publicKey").and_then(|k| k.get("publicKeyPem")).and_then(|v| v.as_str());
    let name = fetched.get("name").and_then(|v| v.as_str());
    let icon = fetched.get("icon").and_then(|i| i.get("url")).and_then(|v| v.as_str());

    // `preferredUsername` plus the host the document actually came from — never
    // a `handle` the document claims, which is not the document's to assert.
    let handle = fetched
        .get("preferredUsername")
        .and_then(|v| v.as_str())
        .and_then(|u| host_of(uri).map(|h| format!("{u}@{h}")));

    state
        .db
        .upsert_remote_actor(uri, handle.as_deref(), name, &inbox, shared.as_deref(), key, icon)
        .await
        .ok()
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let host = rest.split('/').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// Fetch an actor document from a URL a **stranger chose**.
///
/// That is the whole hazard: the `keyId` on an unauthenticated POST decides
/// what this server connects to, which is a server-side request forgery in
/// exactly the shape they are usually found in. Three guards:
///
/// * **Absolute `https` only.** A scheme-relative or `http` URL is refused
///   outright — an actor document is key material and fetching it in the clear
///   means whoever is on the path chooses the key.
/// * **No private or loopback hosts**, via the same `is_private_host` the MCP
///   origin check uses. This is what stops `keyId` being pointed at
///   `http://127.0.0.1:8080/admin/...` or at a cloud metadata endpoint.
/// * **A size cap and the caller's deadline**, so neither a slow server nor an
///   endless one can hold a request open past the reverse proxy in front of us.
async fn fetch_actor(uri: &str, deadline: tokio::time::Instant) -> Option<Value> {
    let Some(host) = uri.strip_prefix("https://").and_then(|_| host_of(uri)) else {
        debug!("📡 refusing to fetch a non-https actor: {uri}");
        return None;
    };
    if is_private_host(&host) {
        warn!("📡 refusing to fetch an actor on a private host: {uri}");
        return None;
    }

    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return None;
    }

    let request = http()
        .get(uri)
        .header(header::ACCEPT, "application/activity+json, application/ld+json")
        .send();

    let response = match tokio::time::timeout(remaining, request).await {
        Ok(Ok(r)) if r.status().is_success() => r,
        Ok(Ok(r)) => {
            debug!("📡 actor fetch {uri}: HTTP {}", r.status());
            return None;
        }
        Ok(Err(e)) => {
            debug!("📡 actor fetch {uri}: {e}");
            return None;
        }
        Err(_) => {
            debug!("📡 actor fetch {uri}: out of time");
            return None;
        }
    };

    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let body = tokio::time::timeout(remaining, response.bytes()).await.ok()?.ok()?;
    if body.len() > MAX_ACTOR_BYTES {
        warn!("📡 actor document at {uri} is {} bytes; refusing", body.len());
        return None;
    }

    serde_json::from_slice(&body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that lists `text/html` as a fallback is still a server. Getting
    /// this backwards serves the app's HTML shell to a remote instance, which
    /// reads as an actor that does not exist.
    #[test]
    fn a_server_gets_the_document_and_a_browser_gets_the_app() {
        let accept = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::ACCEPT, v.parse().unwrap());
            h
        };

        assert!(!wants_html(&accept("application/activity+json")));
        assert!(!wants_html(&accept(
            "application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\""
        )));
        assert!(!wants_html(&accept("application/activity+json, text/html;q=0.1")));

        assert!(wants_html(&accept(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        )));

        // A bare probe gets the document: there is no `/users/:name` page yet,
        // and JSON is the useful answer to curl.
        assert!(!wants_html(&accept("*/*")));
        assert!(!wants_html(&HeaderMap::new()));
    }

    /// An object is a bare URI or an embedded object, and both are ordinary.
    #[test]
    fn an_object_is_read_in_either_shape() {
        assert_eq!(as_uri(&serde_json::json!("https://h/a/1")), Some("https://h/a/1".into()));
        assert_eq!(
            as_uri(&serde_json::json!({"id": "https://h/a/1", "type": "Follow"})),
            Some("https://h/a/1".into())
        );
        assert_eq!(as_uri(&serde_json::json!({"type": "Follow"})), None);
        assert_eq!(as_uri(&serde_json::json!("")), None);
        assert_eq!(as_uri(&serde_json::json!(null)), None);
    }

    #[test]
    fn a_host_is_read_off_a_url() {
        assert_eq!(host_of("https://deck.example/users/ash"), Some("deck.example".into()));
        assert_eq!(host_of("https://deck.example"), Some("deck.example".into()));
        assert_eq!(host_of("ftp://deck.example/x"), None);
    }

    /// The `keyId` on an unauthenticated POST decides what this server
    /// connects to. Every one of these is a request forgery that must not
    /// leave the box, and each is refused before any socket is opened.
    #[tokio::test]
    async fn an_actor_fetch_will_not_be_pointed_at_our_own_network() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        for hostile in [
            "http://127.0.0.1:8080/admin/users",
            "https://127.0.0.1/users/ash",
            "https://localhost/users/ash",
            "https://10.0.0.4/users/ash",
            "https://192.168.1.9/users/ash",
            "https://172.16.4.4/users/ash",
            // The cloud metadata endpoint — the highest-value target of the
            // whole class, and the reason `is_private_host` gained 169.254/16.
            "https://169.254.169.254/latest/meta-data",
            "https://100.79.1.2/users/ash",
            "https://[::1]/users/ash",
            "https://nas/users/ash",
            "https://pi.local/users/ash",
            "http://deck.example/users/ash",
            "file:///etc/passwd",
            "/users/ash",
            "",
        ] {
            assert!(
                fetch_actor(hostile, deadline).await.is_none(),
                "must refuse to fetch {hostile}"
            );
        }
    }
}
