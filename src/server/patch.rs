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
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use utoipa::{IntoParams, ToSchema};

use super::mcp::{is_private_host, public_origin};
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{
    Actor, ArtistDeck, ChainTally, DeckView, DiscoverDeck, Dub, FederationOps, HandedSitting,
    HandedSittingDetail, InstancePulse, PatchArtist, PatchPeer, ReelListen, SentSitting,
    SpoolCapture, SpoolDetail, SpoolSummary, SpoolView,
};
use crate::openapi::{Bytes as Binary, Unauthorized};
use crate::patch::{actor as doc, signatures};
use crate::sources::egress::Egress;

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

/// Everyone on the instance, and where you stand with them.
#[derive(Serialize, ToSchema)]
pub struct PatchDecks {
    /// Every deck you could patch into.
    pub decks: Vec<PatchPeer>,
    /// Decks you patched into, or asked to.
    pub following: Vec<PatchPeer>,
    /// Everything pointed at you, pending and settled — the inbox filters to
    /// `pending` itself.
    pub followers: Vec<PatchPeer>,
}

#[derive(Serialize, ToSchema)]
pub struct PatchRequested {
    pub uuid: String,
    /// As it now stands: `pending`, or `accepted` for a patch that already
    /// was.
    pub state: String,
}

#[derive(Serialize, ToSchema)]
pub struct PatchSettled {
    /// `accepted` or `rejected`.
    pub state: &'static str,
}

#[derive(Serialize, ToSchema)]
pub struct Unpatched {
    pub unpatched: bool,
}

#[derive(Serialize, ToSchema)]
pub struct ChainShared {
    pub shared: bool,
}

/// A page of the Reel, newest first.
#[derive(Serialize, ToSchema)]
pub struct ReelPage {
    pub listens: Vec<ReelListen>,
    /// Pass as `before` for the next page. Null when this page was the last.
    pub next: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct PatchArtists {
    pub artists: Vec<PatchArtist>,
    /// The window asked for; null is all time.
    pub days: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct DiscoverDecks {
    pub decks: Vec<DiscoverDeck>,
}

/// What this instance has been playing, through shared chains.
#[derive(Serialize, ToSchema)]
pub struct InstanceView {
    pub days: i64,
    pub pulse: InstancePulse,
}

/// The federation counters, and the vocabulary this instance speaks.
#[derive(Serialize, ToSchema)]
pub struct PatchOps {
    pub ops: FederationOps,
    pub vocabulary: Vocabulary,
    /// Whether outbound delivery is on. Not yet.
    pub delivery: bool,
}

/// Compiled in and read-only.
#[derive(Serialize, ToSchema)]
pub struct Vocabulary {
    pub verb: &'static str,
    pub object: &'static str,
    pub namespace: &'static str,
    pub frozen: bool,
}

#[derive(Serialize, ToSchema)]
pub struct DubFiled {
    pub id: i64,
}

#[derive(Serialize, ToSchema)]
pub struct Dubs {
    pub dubs: Vec<Dub>,
}

/// The sitting you are in, and what sittings have offered you.
#[derive(Serialize, ToSchema)]
pub struct SpoolNow {
    pub spool: Option<SpoolView>,
    /// Plays waiting for you to accept or decline.
    pub captures: Vec<SpoolCapture>,
}

#[derive(Serialize, ToSchema)]
pub struct SpoolStarted {
    pub uuid: String,
}

#[derive(Serialize, ToSchema)]
pub struct SpoolMembership {
    /// `joined`, `declined` or `left` — the row knows which.
    pub state: String,
    /// Plays joining has just put in your inbox. For a sitting that ended
    /// before you answered, that is the whole of it.
    pub offered: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SpoolEnded {
    pub ended: bool,
}

#[derive(Serialize, ToSchema)]
pub struct SpoolHistory {
    /// Newest first.
    pub sittings: Vec<SpoolSummary>,
}

#[derive(Serialize, ToSchema)]
pub struct PatchSaved {
    pub saved: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MembersAdded {
    pub added: i64,
}

#[derive(Serialize, ToSchema)]
pub struct PresenceSet {
    /// How many plays this has now put in your inbox.
    pub offered: i64,
}

#[derive(Serialize, ToSchema)]
pub struct CapturesSettled {
    pub settled: i64,
    /// How many of those will be relayed to your connections — which turns
    /// on a setting *and* on each play being recent enough.
    pub forwarded: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SpoolChain {
    /// Null files accepted plays under no chain.
    pub chain_id: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct ArtistContext {
    /// Decks on this instance that play them through a shared chain.
    pub decks: Vec<ArtistDeck>,
    /// Your own chains you hear them on.
    pub chains: Vec<ChainTally>,
}

#[derive(Serialize, ToSchema)]
pub struct LiveDecks {
    pub live: Vec<LiveDeck>,
}

/// A deck you are patched into that is playing now, through a chain you may
/// see.
#[derive(Serialize, ToSchema)]
pub struct LiveDeck {
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<u64>,
    /// Seconds into the track.
    pub position: u64,
    /// The position was measured rather than extrapolated.
    pub position_known: bool,
    pub paused: bool,
    /// Null with `has_chain` true is a private chain; null with it false is a
    /// listen through nothing anybody recorded.
    pub chain_name: Option<String>,
    pub has_chain: bool,
    pub artwork_url: Option<String>,
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bit_depth: Option<i16>,
    pub sample_rate: Option<i32>,
    pub is_lossless: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct InboxCount {
    /// Zero, rather than an error, when it cannot be read.
    pub waiting: i64,
}

#[derive(Serialize, ToSchema)]
pub struct HandedSittings {
    /// Newest first. `seen_at` is null for ones you have not opened.
    pub sittings: Vec<HandedSitting>,
}

#[derive(Serialize, ToSchema)]
pub struct SentSittings {
    pub sittings: Vec<SentSitting>,
}

#[derive(Serialize, ToSchema)]
pub struct HandedOver {
    /// `spool` or `recommendation`, as asked.
    pub kind: &'static str,
    /// The sitting's uuid for `spool`; the document's for `recommendation`.
    pub uuid: String,
    /// `spool` only: plays offered to them as captures…
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offered: Option<i64>,
    /// …and plays held back because they ran through a chain you do not
    /// share.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withheld: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct SittingWithdrawn {
    pub withdrawn: bool,
}

#[derive(Serialize, ToSchema)]
pub struct SittingSeen {
    /// False when it had already been seen.
    pub seen: bool,
}

/// Whether this account is discoverable, and the identity it publishes.
#[derive(Serialize, ToSchema)]
pub struct FederationSwitch {
    pub enabled: bool,
    /// What somebody on another server would type, `@name@host`.
    pub handle: String,
    /// The actor URI. Null until an identity has been minted — the first time
    /// federation is switched on.
    pub actor: Option<String>,
    /// The URI that would be minted, shown as a preview.
    pub would_be: String,
    /// Once set, the username is load-bearing for other servers and cannot be
    /// renamed.
    pub frozen_at: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct ShareAll {
    pub enabled: bool,
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    actor_document,
    outbox,
    inbox,
    followers,
    following,
    webfinger,
    get_federation,
    put_federation,
    get_share_all,
    put_share_all,
    decks,
    reel,
    request,
    settle,
    set_shared,
    deck,
    withdraw,
    artists,
    discover,
    instance,
    ops,
    dub,
    dubs,
    spool,
    start_spool,
    join_spool,
    end_spool,
    spool_history,
    spool_detail,
    edit_spool,
    spool_image,
    add_spool_members,
    set_presence,
    settle_captures,
    edit_capture,
    get_spool_chain,
    put_spool_chain,
    artist_context,
    live,
    inbox_count,
    sittings,
    hand_over,
    sittings_sent,
    sitting,
    withdraw_sitting,
    mark_seen
))]
pub struct Api;

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
        .route("/api/v1/profile/share-all", get(get_share_all).put(put_share_all))
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
        // A sitting is a thing you can come back to. `spool` (singular) is
        // what is happening now; these are what happened.
        .route("/api/v1/patch/spools", get(spool_history))
        .route("/api/v1/patch/spools/:uuid", get(spool_detail).patch(edit_spool))
        .route("/api/v1/patch/spools/:uuid/image", get(spool_image))
        .route("/api/v1/patch/spools/:uuid/members", post(add_spool_members))
        .route("/api/v1/patch/spools/:uuid/presence", axum::routing::put(set_presence))
        .route("/api/v1/patch/captures", post(settle_captures))
        .route("/api/v1/patch/captures/:id", axum::routing::patch(edit_capture))
        .route("/api/v1/profile/spool-chain", get(get_spool_chain).put(put_spool_chain))
        .route("/api/v1/patch/artist", get(artist_context))
        .route("/api/v1/patch/live", get(live))
        .route("/api/v1/patch/inbox-count", get(inbox_count))
        .route("/api/v1/patch/sittings", get(sittings).post(hand_over))
        // Before the `:uuid` route: matchit prefers a static segment, but
        // stating the order makes that independent of it.
        .route("/api/v1/patch/sittings/sent", get(sittings_sent))
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

#[derive(Deserialize, ToSchema)]
#[schema(as = PatchRequest)]
struct PatchRequestBody {
    /// The deck to patch into, by username.
    //
    // A username rather than an id, because that is what the person typed and
    // what a remote handle will resolve to in Step 3.
    username: String,
}

/// A yes or a no, said explicitly.
#[derive(Deserialize, ToSchema)]
#[schema(as = PatchAnswer)]
struct SettleBody {
    /// **Required** — an absent key is a 400, never a default.
    //
    // Not an `Option<bool>` with a default. A body that merely misspelled the
    // key must not be read as an answer — and here the two answers are "let
    // them read my listening" and "no".
    #[schema(required, value_type = bool)]
    accept: Option<bool>,
}

#[derive(Deserialize, ToSchema)]
#[schema(as = ChainShareSwitch)]
struct SharedBody {
    /// **Required** — an absent key is a 400.
    #[schema(required, value_type = bool)]
    shared: Option<bool>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ReelQuery {
    /// Default 40, at most 200.
    #[param(maximum = 200)]
    limit: Option<i64>,
    /// Keyset paging: the `next` from the previous page. Not an offset — the
    /// Reel grows from the top while it is being read.
    before: Option<i64>,
    /// Narrows on track, artist, album, chain or deck, server-side.
    //
    // **Server-side**, because the Reel is keyset-paged: a filter applied to
    // the loaded page would silently search only what has been scrolled to and
    // say nothing about it — which looks exactly like a search that found
    // nothing.
    q: Option<String>,
    /// One deck's username, for the filter chips.
    deck: Option<String>,
}

// `GET /api/v1/patch/decks` — everyone on the instance, and where you stand
// with them.
//
// One call rather than three, because the screen shows one list of people with
// a different button each: patched, pending, or not asked. Splitting it would
// have the UI stitch three lists back together and invent the states in
// between.
/// Everyone on the instance, and where you stand with them.
///
/// Returns `decks` (everyone but you, and not disabled), `following` (the
/// patches you have made or asked for) and `followers` (the ones pointed at
/// you, pending and settled).
///
/// One call rather than three, because the screen shows one list of people
/// with a different button each — patched, pending, or not asked. Splitting
/// it would have the client stitch three lists back together and invent the
/// states in between. A peer with a null `uuid` and `state` is one nobody
/// has asked about.
///
/// Each peer carries two fields describing what a patch into them would
/// actually show, and they have to be read together.
/// **`shares_everything`** is the gate: with it on, a patched deck sees
/// every listen they play, whatever chain it ran through or whether it ran
/// through one at all. **`shared_chains`** is how many chains they publish,
/// which is what decides visibility when that switch is off — and, either
/// way, whether the chain is *named* on a row. Zero chains **and** closed is
/// the empty patch that needs saying on screen rather than looking broken;
/// zero chains and open is a deck publishing everything it plays, unnamed.
#[utoipa::path(
    get,
    path = "/api/v1/patch/decks",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "The three lists.", body = PatchDecks),
        (status = 401, response = Unauthorized),
    )
)]
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
        Json(PatchDecks {
            decks: all,
            following: out,
            // Everything pointed at this user, pending and settled. The inbox
            // filters to `pending` itself rather than being served a
            // pre-filtered list, so "who is patched into me" and "who is
            // waiting" cannot disagree about the same row.
            followers: inbound,
        }),
    )
        .into_response()
}

// `POST /api/v1/patch` — ask to patch into a deck.
/// Ask to patch into a deck.
///
/// **A patch is a request, not a subscription.** It lands `pending` and
/// appears in the target's inbox; nothing is shared until they say yes.
///
/// Idempotent in the direction that matters: asking twice does not mint a
/// second edge, and asking again does **not** disturb a patch that has
/// already been accepted. Asking again after a rejection *does* reopen it —
/// a rejection is not a block, and this design has no block yet.
///
/// Not gated on the target having switched federation on: that switch is
/// about the wider fediverse, and members of a private invite-only instance
/// already know each other exist. What protects a deck here is that the
/// patch needs approval and that a chain publishes nothing until its owner
/// shares it.
#[utoipa::path(
    post,
    path = "/api/v1/patch",
    tag = "Patch",
    security(("sessionCookie" = [])),
    request_body = PatchRequestBody,
    responses(
        (status = 200, description = "The patch's `uuid` and its `state` as it now stands.", body = PatchRequested),
        (status = 400, description = "A deck cannot patch into itself.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such deck on this instance.", body = ErrorResponse),
    )
)]
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
                Json(PatchRequested {
                    uuid,
                    // The state is read back rather than assumed: an existing
                    // accepted patch is left alone by `request_patch`, so
                    // reporting "pending" here would be a lie about a patch
                    // that is already live.
                    state: state
                        .db
                        .patches_out(user.user_id)
                        .await
                        .ok()
                        .and_then(|v| v.into_iter().find(|p| p.user_id == target))
                        .and_then(|p| p.state)
                        .unwrap_or_else(|| "pending".into()),
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!("📡 patch request: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not record that")
        }
    }
}

// `POST /api/v1/patch/:uuid/settle` — say yes or no.
//
// Authorisation is inside `settle_patch`'s `WHERE`, not here — see its own
// note. This handler only reports what happened.
/// Say yes or no to a patch request.
///
/// Only the **target** of a patch may settle it, and that check lives in
/// the query's `WHERE` rather than in the handler — this is the one
/// operation that widens who may read somebody's listening, and an
/// authorisation check that lives in a handler is one a second handler can
/// forget.
///
/// A patch that is not yours, one that does not exist, and one already
/// settled are the same 404 on purpose: anything else would confirm that a
/// given patch id is real to somebody it does not concern.
///
/// Accepting **freezes the target's username** — from that point the
/// identity is load-bearing for another party, and a rename would be a
/// deletion and a stranger rather than a rename.
///
/// `accept` is required. A body that merely misspelled the key must not be
/// read as an answer, and here the two answers are "let them read my
/// listening" and "no".
#[utoipa::path(
    post,
    path = "/api/v1/patch/requests/{uuid}/settle",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    request_body = SettleBody,
    responses(
        (status = 200, description = "The new state, `accepted` or `rejected`.", body = PatchSettled),
        (status = 400, description = "`accept` was absent.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No patch request waiting on you with that id.", body = ErrorResponse),
    )
)]
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
                Json(PatchSettled { state: if accept { "accepted" } else { "rejected" } }),
            )
                .into_response()
        }
        Err(e) => {
            warn!("📡 settling a patch: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `DELETE /api/v1/patch/:username` — stop patching into someone.
//
// The follower's own operation, and it takes a username rather than the
// patch's uuid because that is what the row on the screen knows. Deliberately
// **no sweep** of anything already delivered: those listens were shared at the
// time under a switch their owner set, and a retroactive sweep would promise
// an unpatching that the protocol cannot deliver.
/// Stop patching into a deck.
///
/// The follower's own operation. Takes a username rather than the patch's
/// uuid, because that is what the row on the screen knows.
///
/// **No sweep of anything already delivered**, deliberately: those listens
/// were shared at the time under a switch their owner set, and a
/// retroactive sweep would promise an unpatching that the protocol cannot
/// actually deliver.
#[utoipa::path(
    delete,
    path = "/api/v1/patch/deck/{username}",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("username" = String, Path)),
    responses(
        (status = 200, description = "Unpatched.", body = Unpatched),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such deck, or you were not patched into it.", body = ErrorResponse),
    )
)]
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
        Ok(true) => (StatusCode::OK, Json(Unpatched { unpatched: true })).into_response(),
        Ok(false) => refused(StatusCode::NOT_FOUND, "You are not patched into that deck"),
        Err(e) => {
            warn!("📡 unpatch: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `PUT /api/v1/patch/chains/:id/shared` — publish a chain, or stop.
//
// **The sharing unit is the chain, and this is the only switch there is.** It
// lives here rather than on the chains router because it is a Patch decision
// rather than a piece of gear configuration — but the Signal Chains page calls
// exactly this endpoint, which is what makes "one state, two places" true
// rather than aspirational.
/// Publish a signal chain to the people patched into you, or stop.
///
/// **The sharing unit is the chain, and this is the only switch there is.**
/// A listen reaches the Reel because the chain it ran through was switched
/// on by its owner — there is no per-listen override and there should not
/// be one. It makes "what am I publishing?" answerable by looking at a list
/// of chains you already understand, and it makes the decision a standing
/// one about *how you listen* rather than a choice to make per play.
///
/// Off for every chain by default, including every chain that predates the
/// column.
///
/// The Signal Chains page calls this same endpoint, which is what makes
/// "one state, two places" true rather than aspirational.
///
/// `shared` is required; an absent field is a 400, never a default.
#[utoipa::path(
    put,
    path = "/api/v1/patch/chains/{id}/shared",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = SharedBody,
    responses(
        (status = 200, description = "The switch as it now stands.", body = ChainShared),
        (status = 400, description = "`shared` was absent.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such chain of yours.", body = ErrorResponse),
    )
)]
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
            (StatusCode::OK, Json(ChainShared { shared })).into_response()
        }
        Ok(false) => refused(StatusCode::NOT_FOUND, "No such chain"),
        Err(e) => {
            warn!("📡 sharing a chain: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `GET /api/v1/patch/reel` — what the decks you patched into have been playing.
//
// The one read in this application that returns another user's listens. Every
// narrowing is in `Database::reel`'s query rather than here, deliberately: a
// filter applied in a handler is one a second handler can forget.
/// What the decks you patched into have been playing.
///
/// **The one read in this application that returns another user's
/// listens**, and every narrowing is in the query rather than the handler:
/// the patch must be `accepted`, the listen's chain must be `shared`, the
/// listen must have a chain at all, and skips are excluded.
///
/// A listen with no chain is never shared — nobody switched anything on for
/// it, so the default is exclusion.
///
/// Paged by keyset on `before`, not by offset: the Reel grows from the top
/// while it is being read, and an offset would repeat or skip rows as it
/// shifted underneath. `next` is null when the page was short.
///
/// Rows carry the chain — its name, icon and components — because that is
/// what makes a Tapedeck listen a Tapedeck listen rather than a sentence.
/// They deliberately do not carry the source id, the delivery state or the
/// session, which are nobody else's business.
#[utoipa::path(
    get,
    path = "/api/v1/patch/reel",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(ReelQuery),
    responses(
        (status = 200, description = "`listens` and a `next` cursor.", body = ReelPage),
        (status = 401, response = Unauthorized),
    )
)]
async fn reel(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Query(q): Query<ReelQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(40).clamp(1, 200);
    let needle = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let deck = q.deck.as_deref().map(str::trim).filter(|s| !s.is_empty());
    match state.db.reel(user.user_id, limit, q.before, needle, deck).await {
        Ok(listens) => {
            // `next` is the oldest timestamp returned, so the client pages by
            // keyset. `None` when the page was short, which is how it knows to
            // stop rather than asking again forever.
            let next = (listens.len() as i64 == limit)
                .then(|| listens.last().map(|l| l.timestamp))
                .flatten();
            (StatusCode::OK, Json(ReelPage { listens, next })).into_response()
        }
        Err(e) => {
            warn!("📡 reel for {}: {e}", user.username);
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the Reel")
        }
    }
}

// ── Step 3: the screens, Dubs and Spools ─────────────────────

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SinceQuery {
    /// Days back.
    days: Option<i64>,
}

// `GET /api/v1/patch/artists` — artists across the decks you patched into.
//
// `days` bounds the window the ranking is computed over; absent is all time.
// Server-side because the query cuts to the top N by plays, so narrowing in
// the browser would rank a week among artists chosen by their all-time totals
// — a list that looks right and is not.
/// Artists across the decks you've patched into.
///
/// Narrowed exactly like the Reel — accepted patch, shared chain, the
/// listen has a chain, no skips — because `Database::SHARED_LISTENS` is one
/// fragment every Patch reader shares. A filter omitted from one of them is
/// the shape this codebase keeps finding.
///
/// `mine` is your own plays of that artist: **the overlap is reported as
/// which artists, never as a similarity score.** A score invites a ranking
/// of people, which is not what this is for.
///
/// `days` bounds the window the ranking is computed over; absent is all
/// time. **Server-side on purpose**: the query cuts to the top N by plays,
/// so narrowing in the client would rank a week among artists chosen on
/// their all-time totals — a list that looks right and is not.
///
/// `genre` is the artist's own first tag from the cache the genre map
/// fills, and is null for anyone MusicBrainz has not tagged. `first_played`
/// is when they first appeared in the shared history.
#[utoipa::path(
    get,
    path = "/api/v1/patch/artists",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(SinceQuery),
    responses(
        (status = 200, description = "Artists with plays, decks, who, genre, and your own overlap.", body = PatchArtists),
        (status = 401, response = Unauthorized),
    )
)]
async fn artists(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Query(q): Query<SinceQuery>,
) -> Response {
    let since = q.days.filter(|d| *d > 0).map(|d| chrono::Utc::now().timestamp() - d * 86_400);
    match state.db.patch_artists(user.user_id, 120, since).await {
        Ok(artists) => {
            (StatusCode::OK, Json(PatchArtists { artists, days: q.days })).into_response()
        }
        Err(e) => {
            warn!("📡 patch artists: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

// `GET /api/v1/patch/discover` — everyone here, and what you have in common.
/// Everyone on the instance, and what you have in common.
///
/// `overlap` is artists you both play, unit-separated (`\x1f`), and it is
/// computed against **their shared chains only** — so this cannot become a
/// way to learn what somebody plays privately by watching the list change.
///
/// `state` is your patch with them (`pending` / `accepted` / `rejected`) or
/// null if you have never asked.
#[utoipa::path(
    get,
    path = "/api/v1/patch/discover",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Decks, newest first.", body = DiscoverDecks),
        (status = 401, response = Unauthorized),
    )
)]
async fn discover(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    match state.db.discover_decks(user.user_id).await {
        Ok(decks) => (StatusCode::OK, Json(DiscoverDecks { decks })).into_response(),
        Err(e) => {
            warn!("📡 patch discover: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

// `GET /api/v1/patch/instance` — what this instance has been playing.
//
// **Not narrowed by the caller's patches**, unlike everything else on this
// router. The Reel answers "what have the decks I follow played"; this answers
// "what is this instance listening to", which is a fact about the people who
// chose to publish a chain and is the same for everyone who asks.
/// What this instance has been playing.
///
/// Over **shared chains only**, across all decks — and deliberately *not*
/// narrowed by your own patches: the Reel answers "what have the decks I
/// follow played", this answers "what is this instance listening to", which
/// is a fact about the people who chose to publish a chain and reads the
/// same for everyone.
///
/// A considered reversal of the Reports rule that there is no cross-user
/// figure. That rule existed to stop the instance exposing how much other
/// members play without their consent; a per-chain opt-in **is** that
/// consent. It stays inside the instance.
///
/// `daily` is raw `(timestamp, hours)` pairs rather than pre-bucketed days,
/// because the day boundary belongs to the reader's timezone and the server
/// does not know it here.
#[utoipa::path(
    get,
    path = "/api/v1/patch/instance",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(SinceQuery),
    responses(
        (status = 200, description = "Decks, listens, hours, top artists, chains and daily hours.", body = InstanceView),
        (status = 401, response = Unauthorized),
    )
)]
async fn instance(
    State(state): State<Arc<AppState>>,
    _user: super::auth::AuthUser,
    Query(q): Query<SinceQuery>,
) -> Response {
    let days = q.days.unwrap_or(7).clamp(1, 365);
    let since = chrono::Utc::now().timestamp() - days * 86_400;
    match state.db.instance_pulse(since).await {
        Ok(pulse) => (StatusCode::OK, Json(InstanceView { days, pulse })).into_response(),
        Err(e) => {
            warn!("📡 instance pulse: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

// `GET /api/v1/patch/ops` — the admin Operations panel.
//
// Reports **only what is recorded**. The design lists signature failures;
// refusals go through `tracing` and are counted nowhere, and a tile showing an
// invented number would be worse than one that is honest about where to look.
/// The federation Operations panel. Admin only.
///
/// Reports **only what is recorded**: identities minted, accounts
/// published, usernames frozen, remote actors cached, inbox activities
/// received, deliveries attempted.
///
/// Signature refusals are logged through `tracing` and counted nowhere, so
/// there is no tile for them — an invented number would be worse than an
/// honest absence, and the screen says where to look instead.
///
/// `vocabulary` is read-only. The design offers a picker with a Freeze
/// control; three vocabularies would mean three things to maintain and a
/// migration path between them for a decision made once, so the verb is
/// compiled in and this only reports it.
#[utoipa::path(
    operation_id = "patch_ops",
    get,
    path = "/api/v1/patch/ops",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Counters, the frozen vocabulary, and whether delivery is on.", body = PatchOps),
        (status = 401, response = Unauthorized),
        (status = 403, description = "Not an admin.", body = ErrorResponse),
    )
)]
async fn ops(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    if !user.is_admin {
        return refused(StatusCode::FORBIDDEN, "Admins only");
    }
    match state.db.federation_ops().await {
        Ok(ops) => (
            StatusCode::OK,
            Json(PatchOps {
                ops,
                // Read-only, deliberately. The design offers a picker with a
                // Freeze control; three vocabularies would mean three things to
                // maintain and a migration path between them for a decision
                // made once, so the verb is compiled in and this only reports
                // it.
                vocabulary: Vocabulary {
                    verb: "Create",
                    object: "td:Listen",
                    namespace: crate::patch::actor::TD_NS,
                    frozen: true,
                },
                delivery: false,
            }),
        )
            .into_response(),
        Err(e) => {
            warn!("📡 federation ops: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

// `GET /api/v1/patch/deck/:username` — one deck, through an accepted patch.
/// One deck, through an accepted patch.
///
/// Session-only. The deck's profile, how much it has played, its shared
/// chains with their hours, and the artists you both play. A deck that does
/// not exist and one you are not patched into are the same 404.
#[utoipa::path(
    get,
    path = "/api/v1/patch/deck/{username}",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("username" = String, Path)),
    responses(
        (status = 200, description = "The deck.", body = DeckView),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such deck, or you are not patched into it.", body = ErrorResponse),
    )
)]
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

/// A record passed to somebody you are patched with, filed in their crate.
#[derive(Deserialize, ToSchema)]
#[schema(as = NewDub)]
struct DubBody {
    /// Their username.
    to: String,
    /// `artist`, `album` or `track`.
    kind: String,
    artist: String,
    /// The album or track title.
    name: Option<String>,
    mbid: Option<String>,
    reason: Option<String>,
    /// **Required** — what in *their* listening prompted this.
    #[schema(required, value_type = String)]
    because: Option<String>,
}

// `POST /api/v1/patch/dub` — pass a record to someone.
//
// Goes through `crate_shelf::file`'s own guards where it can: `because` is
// required, and a record already in their history is refused with the play
// count. "You already have this" is Rediscovery under another name, and the
// count is the half that tells the sender something.
/// Pass a record to someone.
///
/// **A dub is a crate entry that came from a person** — not a second table
/// and not a second page. It lands on the Crate, which is the page they
/// already visit for records they are still flipping through, and it is
/// scored the same way: `first_played_at` is derived from their listens, so
/// "you dubbed 12 and they played 5" falls out with nothing accumulated.
///
/// Three guards, two of them the Crate's own:
///
/// * **You must be patched with them**, in either direction. Dubbing at a
///   stranger would be a way to put text on somebody's screen without their
///   consent.
/// * **`because` is required** — what in *their* listening prompted this. A
///   suggestion with no grounding is free association.
/// * **A record already in their history is refused**, with the play count.
///   "You already have this" is Rediscovery under another name, and the
///   count is the half that tells the sender something.
#[utoipa::path(
    post,
    path = "/api/v1/patch/dub",
    tag = "Patch",
    security(("sessionCookie" = [])),
    request_body = DubBody,
    responses(
        (status = 200, description = "Filed in their crate.", body = DubFiled),
        (status = 400, description = "Missing `because`, a bad `kind`, or dubbing to yourself.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, description = "You are not patched with that deck.", body = ErrorResponse),
        (status = 404, description = "No such deck on this instance.", body = ErrorResponse),
        (status = 409, description = "They already have it. The message carries the play count.", body = ErrorResponse),
    )
)]
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
            (StatusCode::OK, Json(DubFiled { id })).into_response()
        }
        Err(sqlx::Error::Protocol(msg)) => refused(StatusCode::FORBIDDEN, msg),
        Err(e) => {
            warn!("📡 dub: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not record that")
        }
    }
}

// `GET /api/v1/patch/dubs` — records people handed you.
/// Records people handed you.
///
/// Crate entries whose origin is a person rather than a model —
/// `from_user_id IS NOT NULL` is the whole filter, on a page that
/// deliberately holds both kinds.
#[utoipa::path(
    get,
    path = "/api/v1/patch/dubs",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Dubs, open ones first.", body = Dubs),
        (status = 401, response = Unauthorized),
    )
)]
async fn dubs(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    match state.db.dubs_for(user.user_id).await {
        Ok(dubs) => (StatusCode::OK, Json(Dubs { dubs })).into_response(),
        Err(e) => {
            warn!("📡 dubs: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(as = NewSpool)]
struct SpoolBody {
    title: Option<String>,
    /// Usernames to invite.
    with: Option<Vec<String>>,
    /// Names of people in the room with no account on this instance.
    guests: Option<Vec<String>>,
}

/// Name a sitting, write about it, or change who else was there. An absent
/// field is left alone; an explicit `null` clears it.
//
// Every field is `Option<Option<_>>` so that an absent key and an explicit
// null stay distinguishable — this endpoint decides between "change this" and
// "leave it alone", which is exactly where a bare `Option` is the wrong type.
#[derive(Deserialize, ToSchema)]
#[schema(as = SpoolEdit)]
struct SpoolEditBody {
    #[serde(default, deserialize_with = "double_option_str")]
    #[schema(value_type = Option<String>)]
    title: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option_str")]
    #[schema(value_type = Option<String>)]
    notes: Option<Option<String>>,
    /// Names of people in the room with no account here. Replaces the list.
    guests: Option<Vec<String>>,
}

#[derive(Deserialize, ToSchema)]
#[schema(as = SpoolMembers)]
struct SpoolMembersBody {
    /// Usernames to add. Required.
    with: Option<Vec<String>>,
    /// When they arrived, or absent for "they were here all along".
    //
    // Which is the common case and the reason this is not derived from the
    // clock.
    present_from: Option<i64>,
}

/// When you were in the room. An absent field is left alone; `null` clears
/// it.
#[derive(Deserialize, ToSchema)]
#[schema(as = SpoolPresence)]
struct PresenceBody {
    #[serde(default, deserialize_with = "double_option_i64")]
    #[schema(value_type = Option<i64>)]
    present_from: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option_i64")]
    #[schema(value_type = Option<i64>)]
    present_until: Option<Option<i64>>,
}

#[derive(Deserialize, ToSchema)]
#[schema(as = CaptureTime)]
struct CaptureTimeBody {
    /// Unix seconds. Required.
    #[schema(required, value_type = i64)]
    played_at: Option<i64>,
}

#[derive(Deserialize, ToSchema)]
#[schema(as = SpoolChainChoice)]
struct SpoolChainBody {
    /// A chain of yours, or `null` to file accepted plays under nothing.
    /// **Required** — an absent key is a 400, never a clear.
    #[serde(default, deserialize_with = "double_option_i64")]
    #[schema(required, value_type = Option<i64>)]
    chain_id: Option<Option<i64>>,
}

fn double_option_i64<'de, D>(de: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i64>::deserialize(de).map(Some)
}

fn double_option_str<'de, D>(de: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(de).map(Some)
}

#[derive(Deserialize, ToSchema)]
#[schema(as = CaptureAnswer)]
struct SettleCapturesBody {
    /// The captures. Required.
    #[schema(required, value_type = Vec<i64>)]
    ids: Option<Vec<i64>>,
    /// **Required.** Accepted plays land as imported and are never forwarded
    /// from here.
    #[schema(required, value_type = bool)]
    accept: Option<bool>,
}

#[derive(Deserialize, ToSchema)]
#[schema(as = HandOver)]
struct HandOverBody {
    /// The sitting's uuid, from `GET /api/v1/sessions`.
    #[schema(required, value_type = String)]
    session: Option<String>,
    /// The username to hand it to.
    #[schema(required, value_type = String)]
    to: Option<String>,
    /// At most 500 characters.
    note: Option<String>,
    /// **Which of the two gestures this is, and it is required.**
    ///
    /// `spool` — they were in the room and neither of you started a sitting.
    /// The plays are offered as ordinary captures and enter their history when
    /// they accept, exactly as a live Shared Spool would have.
    ///
    /// `recommendation` — they were not there. They get a document of what you
    /// played, and nothing enters their history.
    //
    // No default. The two write different things to somebody else's permanent
    // record, so guessing from an absent field is the one thing this must not
    // do — the rule `bulk-chain` learned the expensive way.
    #[schema(required, value_type = String)]
    kind: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ArtistQuery {
    /// The artist. Required.
    #[param(required = true)]
    name: Option<String>,
}

// `GET /api/v1/patch/artist?name=` — who else here plays them, and which of
// your own chains you hear them on.
//
// **Instance-wide rather than narrowed by your patches**, the same deliberate
// reversal the Instance tab makes: the rule that stops a deck exposing
// somebody's listening without consent is the per-chain switch, and switching
// it on *is* that consent. Shared chains only, and it stays inside the
// instance.
//
// Sits beside the ordinary artist page rather than replacing it. The design
// draws a whole artist screen inside Patch, but its stats are the same ones
// `/artist/:id` already computes, and a second copy would drift — this is only
// the half that is genuinely about other decks.
/// Who else here plays an artist, and which of your chains you hear them on.
///
/// The Patch half of an artist page. **Not a separate artist screen**: the
/// design draws one inside Patch, but its statistics are the ones
/// `/api/v1/artists/{id}` already computes and a second copy would drift.
/// This is only the part that is genuinely about *other decks*.
///
/// `decks` is **instance-wide rather than narrowed by your patches** — the
/// same deliberate reversal the Instance tab makes, on the same reasoning:
/// the rule that stops a deck exposing somebody's listening without consent
/// is the per-chain switch, and switching it on *is* that consent. Shared
/// chains only, and it stays inside the instance. `plays` therefore counts
/// a deck's listens through shared chains and nothing else.
///
/// `patched` says whether an accepted patch already exists in either
/// direction, so a screen can offer to open the deck or to ask — rather
/// than a link that 404s, since reading a deck needs a patch.
///
/// `chains` is **your own** listening of that artist, grouped by chain, and
/// is deliberately *not* filtered on `shared`: what you heard them on is a
/// fact about your setup rather than a publication.
#[utoipa::path(
    get,
    path = "/api/v1/patch/artist",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(ArtistQuery),
    responses(
        (status = 200, description = "The decks, and your own chains.", body = ArtistContext),
        (status = 400, description = "name was missing.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn artist_context(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Query(q): Query<ArtistQuery>,
) -> Response {
    let Some(name) = q.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return refused(StatusCode::BAD_REQUEST, "`name` is required");
    };
    match state.db.patch_artist_context(user.user_id, name).await {
        Ok((decks, chains)) => {
            (StatusCode::OK, Json(ArtistContext { decks, chains })).into_response()
        }
        Err(e) => {
            warn!("📡 patch artist: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

// `GET /api/v1/patch/live` — the decks you are patched into that are playing
// something right now.
//
// **Gated by exactly the rule the Reel uses**, and that is the whole reason
// `NowPlaying` grew a `chain_id`: a live entry has to be checked against a
// *shared chain*, and `chain_name` could not do it — a name is not a key, and
// a polled source sets no name at all because its chain is the device's
// default rather than a choice.
//
// The alternative was relaxing the rule for live entries, so an unshared chain
// would still show a track and a playhead. It was rejected for the reason the
// Spool's capture rule was: it would be a **second way a listen becomes
// visible** while the settings page explains one, and somebody could hand over
// a chain they had deliberately kept private just by pressing play.
//
// Reads the in-memory registry, so it costs no history query — one chain
// lookup per live deck, which on a household instance is a handful.
/// Decks you're patched into that are playing something right now.
///
/// **Gated by exactly the rule the Reel uses** — an accepted patch, and the
/// listen running through a chain its owner switched to **shared**. A live
/// entry that could not be checked never appears, which is why `chain_name`
/// is always present on a row here.
///
/// That gate is what `NowPlaying.chain_id` exists for. A name could not do
/// it — a name is not a key, and a polled source sets no chain name at all,
/// because its chain is the device's default rather than a choice.
///
/// The alternative was relaxing the rule for live entries so an unshared
/// chain would still show a track and a playhead. Rejected for the reason
/// the Shared Spool's capture rule was: it would be a **second way a listen
/// becomes visible** while the settings page explains one, and somebody
/// could hand over a chain they had deliberately kept private just by
/// pressing play.
///
/// Read from the in-memory now-playing registry, so it costs no history
/// query — one chain lookup per live deck. Nothing is persisted, so a
/// restart correctly shows an empty strip until a source reports again.
///
/// `position_known` is false when the position is wall-clock arithmetic
/// rather than a reported playhead; draw it as an estimate, never as a
/// measurement.
#[utoipa::path(
    get,
    path = "/api/v1/patch/live",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "The decks currently playing, and what they are on.", body = LiveDecks),
        (status = 401, response = Unauthorized),
    )
)]
async fn live(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    let peers = match state.db.accepted_patch_targets(user.user_id).await {
        Ok(v) => v,
        Err(e) => {
            warn!("📡 live: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that");
        }
    };

    let mut out = Vec::new();
    for p in peers {
        let Some((np, position)) = state.now_playing.get(p.user_id) else {
            continue;
        };
        // A listen with no chain used to be excluded outright. It is offered
        // now when its owner has opened up, because "all the music they
        // scrobble" has to mean all of it — see `PATCH_VISIBLE_LISTENS`.
        match state.db.live_visibility(p.user_id, np.chain_id).await {
            Ok(crate::db::Visible::Yes(chain_name)) => out.push(LiveDeck {
                user_id: p.user_id,
                username: p.username,
                display_name: p.display_name,
                avatar_url: p.avatar_url,
                has_chain: np.chain_id.is_some(),
                title: np.title,
                artist: np.artist,
                album: np.album,
                duration: np.duration,
                position,
                // The deck already distinguishes a measurement from a guess and
                // so should this: drawing an extrapolation as a playhead is the
                // "confident and completely wrong position" `Progress` exists
                // to stop.
                position_known: np.position_known,
                paused: np.paused,
                // Null with `has_chain` true is "Private signal chain" on the
                // strip; null with it false is a listen that ran through
                // nothing anybody recorded. Different facts, drawn differently.
                chain_name,
                artwork_url: np.artwork_url,
                format_type: np.format_type,
                codec: np.codec,
                bit_depth: np.bit_depth,
                sample_rate: np.sample_rate,
                is_lossless: np.is_lossless,
            }),
            Ok(crate::db::Visible::No) => {}
            Err(e) => warn!("📡 live: {e}"),
        }
    }

    (StatusCode::OK, Json(LiveDecks { live: out })).into_response()
}

// `GET /api/v1/patch/inbox-count` — one number, for the sidebar badge.
//
// Its own endpoint rather than the sidebar assembling the four lists: that
// would be four requests on every page load, for a number, against a pool of
// five connections.
/// How many things are waiting on you in Patch.
///
/// One number, for the sidebar badge: patches awaiting a yes, unsettled
/// Shared Spool captures, open dubs, and sittings passed to you but not
/// opened.
///
/// Its own endpoint rather than the sidebar assembling the four lists —
/// that would be four requests on every page load, for a number, against a
/// pool of five connections.
///
/// **Answers `0` rather than an error** if the count cannot be read. A
/// badge is decoration, and a page that fails to render its sidebar
/// because a count failed is worse than one with no badge on it.
#[utoipa::path(
    get,
    path = "/api/v1/patch/inbox-count",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "The count.", body = InboxCount),
        (status = 401, response = Unauthorized),
    )
)]
async fn inbox_count(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    match state.db.patch_inbox_count(user.user_id).await {
        Ok(waiting) => (StatusCode::OK, Json(InboxCount { waiting })).into_response(),
        Err(e) => {
            warn!("📡 inbox count: {e}");
            // Zero rather than an error: a badge is decoration, and a page that
            // cannot render its sidebar because a count failed is worse than a
            // page with no badge on it.
            (StatusCode::OK, Json(InboxCount { waiting: 0 })).into_response()
        }
    }
}

// `GET /api/v1/patch/sittings` — sittings other people have handed you.
/// Sittings other people have handed you.
#[utoipa::path(
    get,
    path = "/api/v1/patch/sittings",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Newest first. `seen_at` is null for ones you have not opened.", body = HandedSittings),
        (status = 401, response = Unauthorized),
    )
)]
async fn sittings(State(state): State<Arc<AppState>>, user: super::auth::AuthUser) -> Response {
    match state.db.sittings_for(user.user_id).await {
        Ok(sittings) => (StatusCode::OK, Json(HandedSittings { sittings })).into_response(),
        Err(e) => {
            warn!("📡 sittings: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read those")
        }
    }
}

// `GET /api/v1/patch/sittings/sent` — sittings you have passed on.
//
// Its absence is why `withdraw_sitting` had no caller: you could hand one over
// and never see it again, so there was nowhere to take one back from.
/// Sittings you have passed on, and whether they have been read.
///
/// Session-only. The counterpart of `GET /api/v1/patch/sittings`, which lists
/// only what was passed *to* you — without this a hand-over could be sent and
/// never seen again, so there was nowhere to withdraw one from.
///
/// **Only documents appear here.** A reconstruction (`kind: "spool"`) writes
/// real captures into the other person's inbox and no `shared_sessions` row, so
/// there is nothing to take back — those are plays they accept or decline for
/// themselves.
#[utoipa::path(
    get,
    path = "/api/v1/patch/sittings/sent",
    tag = "Patch",
    responses(
        (status = 200, description = "OK.", body = SentSittings),
        (status = 401, response = Unauthorized),
        (status = 401, response = Unauthorized),
    )
)]
async fn sittings_sent(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
) -> Response {
    match state.db.sittings_sent(user.user_id).await {
        Ok(sittings) => (StatusCode::OK, Json(SentSittings { sittings })).into_response(),
        Err(e) => {
            warn!("📡 sittings sent: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read those")
        }
    }
}

// `POST /api/v1/patch/sittings` — hand one of your sittings to somebody.
//
// **This copies nothing.** The recipient gets a document rendered from your
// listens, which enters no history of theirs — they were not in the room, and
// writing plays onto somebody's permanent record when they demonstrably did
// not hear them is the error `ListenDisposition` exists to prevent.
/// Pass a past sitting to somebody you are patched with.
///
/// **Two different gestures, and `kind` chooses between them. There is no
/// default** — they write different things to another person's permanent
/// record, so an absent field is a `400` rather than a guess.
///
/// `kind: "spool"` — **the Shared Spool neither of you started.** They were
/// in the room; you just forgot to press the button. The sitting is
/// reconstructed, its plays are offered as ordinary `spool_captures`, and
/// they **enter the recipient's history** when accepted — arriving in the
/// same inbox section and on the same accept a live sitting uses. Returns
/// `offered` and `withheld`. Idempotent: doing it again offers nothing new,
/// and naming a second person adds them to the same reconstructed sitting
/// rather than minting a parallel one.
///
/// `kind: "recommendation"` — **they were not there.** They get a
/// *document* rendered from your listens, which enters no history, count or
/// chart of theirs. `note` applies to this kind only; a set of captures has
/// nowhere to carry a sentence. Handing the same sitting to the same person
/// again updates the note rather than duplicating it.
///
/// You must be patched with them in either direction, and the sitting must
/// be yours.
///
/// **Both kinds pass on only listens through shared chains** — a sitting
/// made late captures exactly what one made on time would have, and this
/// phase has one switch for publishing rather than two. What is withheld is
/// counted either way (`withheld` here, `omitted` on the document), never
/// silently dropped. Unlike a live sitting there is a recovery: share the
/// chain and do it again.
#[utoipa::path(
    post,
    path = "/api/v1/patch/sittings",
    tag = "Patch",
    security(("sessionCookie" = [])),
    request_body = HandOverBody,
    responses(
        (status = 200, description = "For `spool`: the sitting's uuid plus `offered` and `withheld`. For `recommendation`: the document's uuid.", body = HandedOver),
        (status = 400, description = "session, to or kind was missing, kind was not one of the two, or the note was too long.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, description = "That deck is not patched with you.", body = ErrorResponse),
        (status = 404, description = "No such deck, or no sitting of yours with that id.", body = ErrorResponse),
    )
)]
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
                HandedOver { kind: "spool", uuid, offered: Some(offered), withheld: Some(withheld) }
            },
        )
    } else {
        state.db.hand_over_session(user.user_id, session, to_id, note).await.map(|uuid| {
            info!("📡 {} passed a sitting to {}", user.username, to);
            HandedOver { kind: "recommendation", uuid, offered: None, withheld: None }
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

// `GET /api/v1/patch/sittings/:uuid` — the document.
/// A handed-over sitting, as a document.
///
/// Readable by the recipient **or** by whoever handed it over, so you can
/// see what you actually passed on, holes included. Any other caller gets
/// the same 404 a nonexistent id does — the alternative would confirm a
/// uuid is real to somebody it does not concern.
///
/// Rendered **live** rather than snapshotted: un-sharing a chain, or
/// regrouping the sitting, really does change what the recipient sees.
/// That is correct while both ends are on one instance, where the document
/// is served from the owner's own database on every read. It has to become
/// a snapshot when this federates, because bytes already on somebody
/// else's server cannot be recalled.
///
/// `omitted` is how many listens in the sitting are not being shown
/// because they ran through chains the owner does not share. `gone` means
/// the sitting has since been regrouped out of existence — an honest empty
/// document rather than a broken link.
#[utoipa::path(
    get,
    path = "/api/v1/patch/sittings/{uuid}",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    responses(
        (status = 200, description = "The document.", body = HandedSittingDetail),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No sitting with that id, or not yours to read.", body = ErrorResponse),
    )
)]
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

// `DELETE /api/v1/patch/sittings/:uuid` — withdraw one you handed over.
//
// A real withdrawal, unlike un-patching: nothing was ever copied, so removing
// the row removes the access. That stops being true the moment this
// federates, which is why the storage note says the remote case must be a
// snapshot.
/// Withdraw a sitting you handed over.
///
/// A real withdrawal, unlike un-patching: nothing was ever copied, so
/// removing the row removes the access. That stops being true once this
/// federates.
#[utoipa::path(
    delete,
    path = "/api/v1/patch/sittings/{uuid}",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    responses(
        (status = 200, description = "Withdrawn.", body = SittingWithdrawn),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No sitting of yours with that id.", body = ErrorResponse),
    )
)]
async fn withdraw_sitting(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
) -> Response {
    match state.db.withdraw_session(&uuid, user.user_id).await {
        Ok(true) => (StatusCode::OK, Json(SittingWithdrawn { withdrawn: true })).into_response(),
        Ok(false) => refused(StatusCode::NOT_FOUND, "No sitting of yours with that id"),
        Err(e) => {
            warn!("📡 withdraw sitting: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not withdraw that")
        }
    }
}

// `POST /api/v1/patch/sittings/:uuid/seen` — clear it from the inbox.
//
// Not an accept. A handed-over sitting writes nothing to a history, so there
// is nothing to consent to — only a notification to stop showing.
/// Clear a handed-over sitting from your inbox.
///
/// **Not an accept.** A handed-over sitting writes nothing to a history,
/// so there is nothing to consent to — only a notification to stop
/// showing. Contrast `/api/v1/patch/captures`, where accepting really does
/// put a play into your listening.
#[utoipa::path(
    post,
    path = "/api/v1/patch/sittings/{uuid}/seen",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    responses(
        (status = 200, description = "Whether it had not already been seen.", body = SittingSeen),
        (status = 401, response = Unauthorized),
    )
)]
async fn mark_seen(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
) -> Response {
    match state.db.mark_sitting_seen(&uuid, user.user_id).await {
        Ok(seen) => (StatusCode::OK, Json(SittingSeen { seen })).into_response(),
        Err(e) => {
            warn!("📡 sitting seen: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `GET /api/v1/patch/spool` — the sitting you are in, and what it has offered.
/// The sitting you are in, and what it has offered you.
///
/// `spool` is null when you are not in one. `captures` are plays from other
/// people's decks waiting on you — **they are not listens**, and enter no
/// history, count or chart until accepted.
///
/// `spool` also carries an **invitation you have not answered yet, even
/// when the sitting has ended** — `ended_at` is non-null in that case. An
/// invitation is outstanding until it is answered, and the host closing
/// the room is not an answer. A running sitting wins over a stale
/// invitation.
#[utoipa::path(
    get,
    path = "/api/v1/patch/spool",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "The sitting and its captures.", body = SpoolNow),
        (status = 401, response = Unauthorized),
    )
)]
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
    (StatusCode::OK, Json(SpoolNow { spool: active, captures })).into_response()
}

// `POST /api/v1/patch/spool` — open a sitting.
/// Open a Shared Spool.
///
/// A sitting several people are in at once. You are in it immediately;
/// everyone named in `with` is **invited**, and an invitation is not
/// attendance — nothing is captured onto their deck until they join.
///
/// One running sitting per host, enforced by a partial unique index rather
/// than by a check this handler could race, so a second start is a 409.
///
/// **You may only invite a deck you are patched with**, in either
/// direction — an invitation puts a banner on somebody's Patch page, so
/// being able to aim one at a stranger would be a way to reach them
/// without their consent. One stranger in `with` refuses the whole
/// sitting rather than opening it without them.
#[utoipa::path(
    post,
    path = "/api/v1/patch/spool",
    tag = "Patch",
    security(("sessionCookie" = [])),
    request_body = SpoolBody,
    responses(
        (status = 200, description = "The sitting's uuid.", body = SpoolStarted),
        (status = 401, response = Unauthorized),
        (status = 403, description = "One of the named decks is not patched with you. Nothing was started.", body = ErrorResponse),
        (status = 404, description = "One of the named decks does not exist.", body = ErrorResponse),
        (status = 409, description = "You already have a sitting running.", body = ErrorResponse),
    )
)]
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

    let guests = body.guests.unwrap_or_default();
    match state.db.start_spool(user.user_id, body.title.as_deref(), &ids, &guests).await {
        Ok(uuid) => {
            info!("📡 {} started a spool with {} others", user.username, ids.len());
            (StatusCode::OK, Json(SpoolStarted { uuid })).into_response()
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

// `POST /api/v1/patch/spool/:uuid/join` — accept an invitation, or leave.
//
// **An invitation to a sitting the host has already ended is still
// answerable**, and joining one offers the plays it already held — see
// `set_spool_membership`. The room closing is not an answer, and the common
// reason somebody has not joined by then is that they forgot to press the
// button, not that they were never there.
/// Accept an invitation to a sitting, or leave one.
///
/// `accept: true` joins, `false` leaves. Required — an absent field is a
/// 400, never a default, because joining is what starts plays being
/// captured onto your deck.
///
/// **A sitting the host has already ended can still be joined**, and doing
/// so offers the plays it already held — the response's `offered` says how
/// many. Forgetting to press Join before the room closed is the ordinary
/// case, not the odd one, and refusing it there would throw away an
/// evening with no way to ask for it again. Nothing is weakened: joining
/// still decides nothing on its own, since every play arrives as a capture
/// you have to accept.
///
/// Only plays through the player's **shared** chains are offered, exactly
/// as for a live sitting, and nothing already offered to you in any
/// sitting is offered twice.
#[utoipa::path(
    post,
    path = "/api/v1/patch/spool/{uuid}/join",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    request_body = SettleBody,
    responses(
        (status = 200, description = "Your membership as it now stands, and the backlog joining offered you.", body = SpoolMembership),
        (status = 400, description = "`accept` was absent.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No sitting you were invited to with that id.", body = ErrorResponse),
    )
)]
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
    match state.db.set_spool_membership(&uuid, user.user_id, join).await {
        // `offered` is the backlog joining has just put in their inbox, which
        // for a sitting that ended before they answered is the whole of it. The
        // screen says so rather than leaving a join that plainly did something
        // looking like a join that did nothing.
        // `state` comes back from the database rather than being decided here,
        // because saying no to an invitation and going home from a sitting you
        // were in are different answers and only the row knows which one this
        // was.
        Ok(Some((state_name, offered))) => {
            (StatusCode::OK, Json(SpoolMembership { state: state_name, offered })).into_response()
        }
        Ok(None) => refused(StatusCode::NOT_FOUND, "No sitting you were invited to with that id"),
        Err(e) => {
            warn!("📡 spool join: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `POST /api/v1/patch/spool/:uuid/end` — close it. The host's only.
/// Close a sitting. The host's only.
///
/// Captures already made are untouched — they are offers nobody has
/// answered yet, and ending the room does not withdraw what was played in
/// it.
#[utoipa::path(
    post,
    path = "/api/v1/patch/spool/{uuid}/end",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    responses(
        (status = 200, description = "Ended.", body = SpoolEnded),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No running sitting of yours with that id.", body = ErrorResponse),
    )
)]
async fn end_spool(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
) -> Response {
    match state.db.end_spool(&uuid, user.user_id).await {
        Ok(true) => (StatusCode::OK, Json(SpoolEnded { ended: true })).into_response(),
        Ok(false) => refused(StatusCode::NOT_FOUND, "No running sitting of yours with that id"),
        Err(e) => {
            warn!("📡 spool end: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `GET /api/v1/patch/spools` — every sitting you were in, newest first.
//
// **Membership is what admits you, not a patch** — see
// [`Database::spool_history`] for why that widening is deliberate and what it
// does not widen.
/// Every sitting you were in, newest first.
///
/// **Membership is the gate, not the patch.** A sitting is an evening you
/// were in the room for, and un-patching from the host afterwards has no
/// standing to un-happen it — the same argument as "no delete sweep on
/// un-patching". Your own accepted listens from that evening stay in your
/// history whatever happens to the patch, and a history row whose sitting
/// has vanished is a row with nothing to explain it.
///
/// That widening does **not** widen what any of it shows: a sitting still
/// only ever renders the plays the per-chain sharing switch allows, and
/// `withheld` on the detail below says how many it did not.
///
/// A sitting you declined does not appear. You were not there.
#[utoipa::path(
    get,
    path = "/api/v1/patch/spools",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(HistoryQuery),
    responses(
        (status = 200, description = "The sittings.", body = SpoolHistory),
        (status = 401, response = Unauthorized),
    )
)]
async fn spool_history(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Query(q): Query<HistoryQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    match state.db.spool_history(user.user_id, limit).await {
        Ok(sittings) => (StatusCode::OK, Json(SpoolHistory { sittings })).into_response(),
        Err(e) => {
            warn!("📡 spool history: {}", crate::logging::error_chain(&e));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

// `GET /api/v1/patch/spools/:uuid` — one sitting, in full.
/// One sitting, in full.
///
/// The roster with each person's span, the joint timeline, and a count of
/// what is not on it.
///
/// **The timeline is one row per play, not one per listen.** A joint listen
/// is N rows sharing a `joint_session_id`, one per person in the room, so
/// the naive read shows an evening at twice its length as soon as somebody
/// accepts their half. Each row's `standing` is the caller's own relation
/// to that play — `mine`, `accepted`, `declined`, `pending`, or `theirs`
/// for a play from a deck in the room that was never offered to you.
/// Declined and never-answered are told apart deliberately: both mean "not
/// in my history" and they are not the same decision.
///
/// `withheld` counts plays that happened in this sitting and are not shown,
/// because they ran through chains their owner does not share. Stated
/// rather than dropped — a document with invisible holes misrepresents the
/// evening and the reader could never tell.
#[utoipa::path(
    get,
    path = "/api/v1/patch/spools/{uuid}",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    responses(
        (status = 200, description = "The sitting.", body = SpoolDetail),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No sitting of yours with that id. Same answer as one somebody else was in.", body = ErrorResponse),
    )
)]
async fn spool_detail(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
) -> Response {
    match state.db.spool_detail(&uuid, user.user_id).await {
        Ok(Some(v)) => (StatusCode::OK, Json(v)).into_response(),
        // A sitting somebody else was in is the same answer as one that does
        // not exist, or the first confirms an id is real to somebody it does
        // not concern.
        Ok(None) => refused(StatusCode::NOT_FOUND, "No sitting of yours with that id"),
        Err(e) => {
            warn!("📡 spool detail: {}", crate::logging::error_chain(&e));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

// `PATCH /api/v1/patch/spools/:uuid` — name it, write about it, say who was
// there. The host's.
/// Name a sitting, write about it, or say who else was there.
///
/// **The host's, and editable after the fact on purpose.** `title` used to
/// be settable once, at the start — which is the moment you know least
/// about an evening.
///
/// `title` and `notes` distinguish absent from null: an absent field is
/// left alone and an explicit null clears it. A body naming none of the
/// three is a 400 rather than a no-op reported as success.
///
/// Changing `guests` does **not** rewrite the company on plays already
/// fanned out. Those snapshots were accurate when they were taken.
#[utoipa::path(
    patch,
    path = "/api/v1/patch/spools/{uuid}",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    request_body = SpoolEditBody,
    responses(
        (status = 200, description = "Saved.", body = PatchSaved),
        (status = 400, description = "No field named.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No sitting of yours with that id.", body = ErrorResponse),
    )
)]
async fn edit_spool(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
    Json(body): Json<SpoolEditBody>,
) -> Response {
    if body.title.is_none() && body.notes.is_none() && body.guests.is_none() {
        return refused(
            StatusCode::BAD_REQUEST,
            "Name a field to change — `title`, `notes` or `guests`.",
        );
    }
    let result = state
        .db
        .update_spool(
            &uuid,
            user.user_id,
            body.title.as_ref().map(|t| t.as_deref()),
            body.notes.as_ref().map(|n| n.as_deref()),
            body.guests.as_deref(),
        )
        .await;
    match result {
        Ok(true) => (StatusCode::OK, Json(PatchSaved { saved: true })).into_response(),
        Ok(false) => refused(StatusCode::NOT_FOUND, "No sitting of yours with that id"),
        Err(e) => {
            warn!("📡 spool edit: {}", crate::logging::error_chain(&e));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `POST /api/v1/patch/spools/:uuid/members` — add people to a sitting that
// has already started, or already ended.
/// Add people to a sitting that has already started, or ended.
///
/// The guest list used to be frozen at the moment the sitting opened, which
/// is wrong about how an evening goes: somebody arrives, somebody's deck
/// comes on, or the host simply forgot one of the people in front of them.
///
/// `present_from` answers "from when", and it is stated rather than
/// inferred from the clock:
///
/// * **absent** — they were here all along, and are offered the whole
///   evening. This is the common case by a distance.
/// * **a timestamp** — they arrived then, and the backlog is bounded by it.
///
/// **Host only.** The rule that you may only invite a deck *you* are
/// patched with is what stops a guest bringing somebody the host has never
/// patched with into a room where the host's plays are being published.
/// One stranger in `with` refuses the whole call.
///
/// Somebody already in the sitting is left exactly as they are — re-adding
/// must not walk a `declined` back to `invited`.
#[utoipa::path(
    post,
    path = "/api/v1/patch/spools/{uuid}/members",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    request_body = SpoolMembersBody,
    responses(
        (status = 200, description = "How many were added.", body = MembersAdded),
        (status = 400, description = "No decks named.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, description = "One of the named decks is not patched with you. Nothing was added.", body = ErrorResponse),
        (status = 404, description = "No sitting of yours with that id, or no deck by that name.", body = ErrorResponse),
    )
)]
async fn add_spool_members(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
    Json(body): Json<SpoolMembersBody>,
) -> Response {
    let names = body.with.unwrap_or_default();
    if names.is_empty() {
        return refused(StatusCode::BAD_REQUEST, "`with` is required — name the decks.");
    }
    let mut ids = Vec::new();
    for name in &names {
        match state.db.patchable_user_id(name).await {
            Ok(Some(id)) => ids.push(id),
            Ok(None) => return refused(StatusCode::NOT_FOUND, format!("No deck called {name}")),
            Err(e) => {
                warn!("📡 spool members: {}", crate::logging::error_chain(&e));
                return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not look that deck up");
            }
        }
    }

    match state.db.add_spool_members(&uuid, user.user_id, &ids, body.present_from).await {
        Ok(-1) => refused(StatusCode::NOT_FOUND, "No sitting of yours with that id"),
        Ok(added) => (StatusCode::OK, Json(MembersAdded { added })).into_response(),
        // One stranger refuses the whole list rather than the list being
        // quietly filtered — the rule `start_spool` and `dub` both carry.
        Err(sqlx::Error::Protocol(msg)) => refused(StatusCode::FORBIDDEN, msg),
        Err(e) => {
            warn!("📡 spool members: {}", crate::logging::error_chain(&e));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `PUT /api/v1/patch/spools/:uuid/presence` — correct when you were in the
// room.
//
// **Yours to set**, because you are the one who knows. Everything it decides
// is about your own inbox.
/// Correct when you were in the room.
///
/// Yours to set, because you are the one who knows. Everything it decides
/// is about your own inbox — how much of the evening you are offered — so
/// there is nothing here to overreach with.
///
/// Both fields distinguish absent from null; clearing `present_from` back
/// to null means "I was here the whole time".
///
/// Widening your presence offers you the plays you have just said you
/// heard. Narrowing it withdraws no offer already made: an offer is a
/// question somebody asked you, and it stays yours to answer.
#[utoipa::path(
    put,
    path = "/api/v1/patch/spools/{uuid}/presence",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path)),
    request_body = PresenceBody,
    responses(
        (status = 200, description = "How many plays this has now put in your inbox.", body = PresenceSet),
        (status = 400, description = "No field named.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No sitting of yours with that id.", body = ErrorResponse),
    )
)]
async fn set_presence(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
    Json(body): Json<PresenceBody>,
) -> Response {
    if body.present_from.is_none() && body.present_until.is_none() {
        return refused(
            StatusCode::BAD_REQUEST,
            "Name a field to change — `present_from` or `present_until`.",
        );
    }
    match state
        .db
        .set_member_presence(&uuid, user.user_id, body.present_from, body.present_until)
        .await
    {
        Ok(Some(offered)) => (StatusCode::OK, Json(PresenceSet { offered })).into_response(),
        Ok(None) => refused(StatusCode::NOT_FOUND, "No sitting of yours with that id"),
        Err(e) => {
            warn!("📡 spool presence: {}", crate::logging::error_chain(&e));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `PATCH /api/v1/patch/captures/:id` — fix when a capture was played, before
// it becomes a listen.
//
// The cheapest correction in the application: nothing is in a history yet, so
// none of the traps that make editing a *listen's* timestamp delicate apply.
/// Fix when a capture was played, before it becomes a listen.
///
/// **The cheapest correction in the application.** A capture is an offer
/// and not a record, so nothing is in a history, nothing has been forwarded
/// to anybody's Last.fm, no session has been grouped around it and no dedup
/// window has been drawn against it. Every trap that makes editing a
/// *listen's* timestamp delicate arrives only once it has been accepted.
///
/// A capture carries the **host's** clock, which is the ordinary way one
/// comes to be wrong.
///
/// Pending only. A settled capture has either become a listen — where
/// `POST /api/v1/scrobbles/bulk-time` is the thing to reach for — or been
/// declined, and re-timing a refusal changes nothing.
#[utoipa::path(
    patch,
    path = "/api/v1/patch/captures/{id}",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = CaptureTimeBody,
    responses(
        (status = 200, description = "Saved.", body = PatchSaved),
        (status = 400, description = "No `played_at`.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No offer of yours with that id still waiting.", body = ErrorResponse),
    )
)]
async fn edit_capture(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<CaptureTimeBody>,
) -> Response {
    let Some(played_at) = body.played_at else {
        return refused(StatusCode::BAD_REQUEST, "`played_at` is required.");
    };
    match state.db.set_capture_time(user.user_id, id, played_at).await {
        Ok(true) => (StatusCode::OK, Json(PatchSaved { saved: true })).into_response(),
        Ok(false) => refused(StatusCode::NOT_FOUND, "No offer of yours with that id still waiting"),
        Err(e) => {
            warn!("📡 capture time: {}", crate::logging::error_chain(&e));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// `GET|PUT /api/v1/profile/spool-chain` — the chain an accepted Shared Spool
// play is filed under.
/// The chain an accepted Shared Spool play is filed under.
#[utoipa::path(
    get,
    path = "/api/v1/profile/spool-chain",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "`chain_id`, or null.", body = SpoolChain),
        (status = 401, response = Unauthorized),
    )
)]
async fn get_spool_chain(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
) -> Response {
    match state.db.spool_chain_id(user.user_id).await {
        Ok(chain_id) => (StatusCode::OK, Json(SpoolChain { chain_id })).into_response(),
        Err(e) => {
            warn!("📡 spool chain: {}", crate::logging::error_chain(&e));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

/// Name the chain a room you sat in is filed under.
///
/// A capture used to land with no chain at all, so those listens earned no
/// gear hours, carried no fidelity and could never reach the Reel. That was
/// defensible — it was not your gear — and it was not the whole truth
/// either: you were in the room hearing real speakers, and if you have
/// built a chain that says so it should get the hours.
///
/// Unset is the default and reproduces the old behaviour exactly.
///
/// `chain_id` is required and an absent field is a **400**: this decides
/// where every play you accept out of a room lands, so a misspelled key
/// must not read as "clear it". Explicit null clears it.
///
/// Session-only. A scrobble client has no business deciding this.
#[utoipa::path(
    put,
    path = "/api/v1/profile/spool-chain",
    tag = "Patch",
    security(("sessionCookie" = [])),
    request_body = SpoolChainBody,
    responses(
        (status = 200, description = "Saved.", body = PatchSaved),
        (status = 400, description = "`chain_id` absent.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No chain of yours with that id.", body = ErrorResponse),
    )
)]
async fn put_spool_chain(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Json(body): Json<SpoolChainBody>,
) -> Response {
    // Absent is a 400 rather than a guess. This decides where every play you
    // accept out of a room lands, so a misspelled key must not read as "clear
    // it" — `bulk-chain`'s lesson, which cost data the one time it was learned.
    let Some(chain_id) = body.chain_id else {
        return refused(
            StatusCode::BAD_REQUEST,
            "`chain_id` is required — a chain id, or null to file these under nothing.",
        );
    };
    match state.db.set_spool_chain(user.user_id, chain_id).await {
        Ok(true) => (StatusCode::OK, Json(PatchSaved { saved: true })).into_response(),
        Ok(false) => refused(StatusCode::NOT_FOUND, "No chain of yours with that id"),
        Err(e) => {
            warn!("📡 spool chain: {}", crate::logging::error_chain(&e));
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct HistoryQuery {
    /// Default 50, at most 200.
    #[param(maximum = 200)]
    limit: Option<i64>,
}

// `GET /api/v1/patch/spools/:uuid/image` — an evening as a card.
//
// **The ending a sitting never had.** Ending one used to just stop it; this is
// the thing you look at afterwards and hand to the people who were in it.
//
// It computes nothing — every figure comes from `spool_detail`, so a card and
// the page it was shared from cannot disagree — and it makes no network call,
// which is what keeps it free of the deadline every other outbound-from-a-
// handler path in this codebase has had to grow.
/// An evening as a card.
///
/// **The ending a sitting never had.** Ending one used to just stop it;
/// this is the thing you look at afterwards and hand to the people who
/// were in it.
///
/// A fourth composition rather than a report card with a different label. A
/// sitting has no period, so no rank, no per-day rate and no bucket chart —
/// what it has instead is who was in the room, which is the subject, and
/// which no report card has anywhere to put.
///
/// It **computes nothing**: every figure comes from
/// `GET /api/v1/patch/spools/{uuid}`, so a card and the page it was shared
/// from cannot disagree. And it makes no network call, so it needs none of
/// the deadline machinery every other outbound-from-a-handler path here has
/// had to grow.
///
/// `X-Tapedeck-Glyph-Coverage` is `full` or `partial`. Partial is **not** a
/// failure and the image is still returned: the embedded faces are Latin,
/// the host's own fonts sit behind them and may well cover the rest. It
/// means this server cannot *promise* the image is free of tofu, which is a
/// thing to say rather than let the reader discover from the picture.
#[utoipa::path(
    get,
    path = "/api/v1/patch/spools/{uuid}/image",
    tag = "Patch",
    security(("sessionCookie" = [])),
    params(("uuid" = String, Path), ImageQuery),
    responses(
        (status = 200, description = "The card. `X-Tapedeck-Glyph-Coverage` is `partial` when a glyph had to be substituted.", content((Binary = "image/png"), (String = "image/svg+xml"))),
        (status = 400, description = "An unknown size or format.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No sitting of yours with that id.", body = ErrorResponse),
    )
)]
async fn spool_image(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Path(uuid): Path<String>,
    Query(q): Query<ImageQuery>,
) -> Response {
    let Some(size) = crate::report_image::Size::parse(q.size.as_deref()) else {
        return refused(StatusCode::BAD_REQUEST, "size must be story, square or wide");
    };
    // Asked at export time rather than inherited: a share lands on somebody
    // else's screen, so the reader's own theme preference is not the question.
    // An unrecognised value is the default rather than a 400 — this is
    // presentation, and refusing to draw over a misspelling is worse than
    // drawing the default. `size` and `format` are 400s because neither has a
    // sensible default shape.
    let theme = crate::report_image::Theme::parse(q.theme.as_deref());
    let svg_only = matches!(q.format.as_deref(), Some("svg"));
    if !matches!(q.format.as_deref(), None | Some("png") | Some("svg")) {
        return refused(StatusCode::BAD_REQUEST, "format must be png or svg");
    }

    let sitting = match state.db.spool_detail(&uuid, user.user_id).await {
        Ok(Some(v)) => v,
        Ok(None) => return refused(StatusCode::NOT_FOUND, "No sitting of yours with that id"),
        Err(e) => {
            warn!("📡 spool image: {}", crate::logging::error_chain(&e));
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that");
        }
    };

    let zone = crate::server::scrobbles::user_tz_for(&state, user.user_id).await;
    let card = sitting_card(&sitting, zone);

    // **Off the async worker.** A 1080×1920 pixmap is ~8 MB of RGBA and
    // rasterising it in the handler pins a tokio worker, one of four on a Pi.
    let rendered = match tokio::task::spawn_blocking(move || {
        crate::report_image::render_sitting(&card, size, theme)
    })
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            warn!("📡 spool image: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not draw the image");
        }
        Err(e) => {
            warn!("📡 spool image render panicked: {e}");
            return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not draw the image");
        }
    };

    let coverage = if rendered.fully_covered { "full" } else { "partial" };
    let stem = format!("tapedeck-sitting-{}-{}", size.as_str(), theme.as_str());
    let (body, mime, ext) = if svg_only {
        (rendered.svg.into_bytes(), "image/svg+xml; charset=utf-8", "svg")
    } else {
        (rendered.png, "image/png", "png")
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.to_string()),
            // One evening of somebody's listening, so never a shared cache.
            (header::CACHE_CONTROL, "private, max-age=300".to_string()),
            (header::CONTENT_DISPOSITION, format!("inline; filename=\"{stem}.{ext}\"")),
            (header::HeaderName::from_static("x-tapedeck-glyph-coverage"), coverage.to_string()),
        ],
        body,
    )
        .into_response()
}

/// Turn a sitting into the card's own model.
///
/// Pure, and separate from the handler for the reason `public_payload` is: it
/// decides what an evening looks like to somebody who was not necessarily in
/// it, and a rule like that should be readable without a server around it.
fn sitting_card(
    s: &crate::db::SpoolDetail,
    zone: chrono_tz::Tz,
) -> crate::report_image::SittingCard {
    use chrono::TimeZone;

    // **Everybody else**, decks and guests alike — the reader is left out, the
    // way every "with …" line in this application leaves them out.
    let mut with: Vec<String> = s
        .members
        .iter()
        .filter(|m| m.user_id != s.viewer_id && matches!(m.state.as_str(), "joined" | "left"))
        .map(|m| m.display_name.clone().unwrap_or_else(|| m.username.clone()))
        .collect();
    with.extend(s.guests.iter().cloned());

    // Bucketed in the **reader's** timezone, not UTC. A sitting that ran to
    // half past midnight in CET belongs to the evening it started in, and the
    // date on the card is the one thing everybody in the room would agree on.
    let started = zone.timestamp_opt(s.started_at, 0).single();
    let date = started.map(|d| d.format("%A, %-d %B %Y").to_string()).unwrap_or_default();
    let span = match (started, s.ended_at.and_then(|e| zone.timestamp_opt(e, 0).single())) {
        (Some(a), Some(b)) => {
            // Rounded, not truncated — the page rounds, and a card saying
            // "9 min" beside a page saying "10 min" about one evening is the
            // kind of small disagreement that makes a reader doubt both.
            let secs = (s.ended_at.unwrap_or(0) - s.started_at).max(0);
            let mins = (secs + 30) / 60;
            let ran = if mins < 60 {
                format!("{mins} min")
            } else if mins % 60 == 0 {
                format!("{}h", mins / 60)
            } else {
                format!("{}h {}m", mins / 60, mins % 60)
            };
            format!("{} – {} · {ran}", a.format("%H:%M"), b.format("%H:%M"))
        }
        (Some(a), None) => format!("from {} · still running", a.format("%H:%M")),
        _ => String::new(),
    };

    crate::report_image::SittingCard {
        title: s.title.clone().unwrap_or_else(|| "A sitting".into()),
        date,
        span,
        with,
        plays: s.figures.plays,
        seconds: s.figures.seconds,
        duration_known: s.figures.duration_known,
        artists: s.figures.artists,
        albums: s.figures.albums,
        top_artist: s.figures.top_artist.clone(),
        top_album: s.figures.top_album.clone(),
        decks: s.figures.decks.clone(),
        // In the order they went on, capped well above anything a card can
        // draw — the composition truncates to the room it has, and it is the
        // only thing that knows how much that is.
        played: s
            .plays
            .iter()
            .filter_map(|p| {
                p.album
                    .as_deref()
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .map(|a| (a.to_string(), p.artist.clone()))
            })
            .fold(Vec::new(), |mut acc, row| {
                // Consecutive plays off one record are one record going on,
                // not several — a side is several listens and the card is
                // describing the evening rather than the scrobbles.
                if acc.last() != Some(&row) {
                    acc.push(row);
                }
                acc
            })
            .into_iter()
            .take(12)
            .collect(),
        withheld: s.withheld,
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct ImageQuery {
    /// `story` (1080×1920), `square` or `wide`.
    size: Option<String>,
    /// `light` or `dark`. Anything else is the default rather than a 400.
    theme: Option<String>,
    /// `png` (the default) or `svg`.
    format: Option<String>,
}

// `POST /api/v1/patch/captures` — accept or decline what a sitting offered.
//
// **The only path by which another deck can write a listen into your
// history**, which is why it is explicit and per capture. Accepted plays land
// as `imported`, so they are never forwarded onward: somebody else's deck
// played it, and putting it on your permanent public record is not a decision
// this feature has standing to make for you.
/// Accept or decline plays a sitting captured for you.
///
/// **The only path by which another deck can write a listen into your
/// history**, which is why it is explicit and per capture rather than a
/// standing setting.
///
/// Accepted plays land carrying the sitting's `joint_session_id` — the
/// N-rows-one-per-listener shape decided on 2026-08-02.
///
/// **Whether they are forwarded on to Last.fm and ListenBrainz is the
/// user's own setting** — `GET|PUT /api/v1/connections/spool-forwarding`,
/// off by default — bounded by a freshness window so an evening
/// reconstructed out of years ago is stored and never relayed. `forwarded`
/// in the response says how many of the settled plays the flush loop will
/// actually pick up; it is not derivable from `settled`.
///
/// Both `ids` and `accept` are required.
#[utoipa::path(
    post,
    path = "/api/v1/patch/captures",
    tag = "Patch",
    security(("sessionCookie" = [])),
    request_body = SettleCapturesBody,
    responses(
        (status = 200, description = "How many were settled, and how many of those will be relayed.", body = CapturesSettled),
        (status = 400, description = "`ids` or `accept` was absent.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
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
        // `forwarded` is how many of those the flush loop will relay onward,
        // which is not derivable from `settled` — it turns on a user setting
        // *and* on each play being inside the freshness window. A screen that
        // could not say it would be leaving the reader to guess whether a click
        // just wrote to their public Last.fm history.
        Ok((settled, forwarded)) => {
            (StatusCode::OK, Json(CapturesSettled { settled, forwarded })).into_response()
        }
        Err(e) => {
            warn!("📡 settling captures: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that")
        }
    }
}

// ── the switch ───────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
#[schema(as = SharingSwitch)]
struct FederationRequest {
    /// **Required** — an absent key is a 400, never a default.
    //
    // Not `Option<bool>` with a default. A body that merely misspelled the key
    // must not be read as an answer either way — this is the `bulk-chain`
    // rule, in the one place where guessing wrong is *published*.
    #[schema(required, value_type = bool)]
    enabled: Option<bool>,
}

/// What the owner sees about their own switch.
///
/// Reports the handle and the actor URI so the settings screen can show what
/// other people would actually type, rather than leaving them to assemble it.
/// The URI comes from the stored actor once one exists, and is otherwise the
/// one that *would* be minted — a preview, and it says so by being absent from
/// the `actor` field until it is real.
fn federation_body(
    actor: Option<&Actor>,
    username: &str,
    origin: &str,
    on: bool,
) -> FederationSwitch {
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

    FederationSwitch {
        enabled: on,
        handle: format!("@{username}@{host}"),
        // Null until a keypair has been minted, which happens the first time
        // somebody asks. Shown as a preview rather than asserted as an address.
        actor: actor.map(|a| a.uri.clone()),
        would_be: doc::uri_for(origin, username),
        // Once this is set the username is load-bearing for another server and
        // must not be renamed.
        frozen_at: actor.and_then(|a| a.frozen_at),
    }
}

/// Whether your deck answers to the fediverse.
///
/// Your own switch. Reports `enabled`, the `handle` other people would
/// type, the `actor` URI once a keypair has been minted (`null` before
/// that), the `would_be` URI it will get, and `frozen_at` — non-null once
/// the username is load-bearing for another server.
///
/// Reading this **does not mint a keypair**. Opening a settings page is not
/// a decision to federate.
#[utoipa::path(
    get,
    path = "/api/v1/profile/federation",
    tag = "Patch",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "The switch and the identity it publishes.", body = FederationSwitch),
        (status = 401, response = Unauthorized),
    )
)]
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

// What a patched deck sees of this account.
/// Whether patched decks see all your listens.
///
/// Session-only.
#[utoipa::path(
    get,
    path = "/api/v1/profile/share-all",
    tag = "Patch",
    responses(
        (status = 200, description = "OK.", body = ShareAll),
        (status = 401, response = Unauthorized),
        (status = 401, response = Unauthorized),
    )
)]
async fn get_share_all(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
) -> Response {
    match state.db.share_all_with_patched(user.user_id).await {
        Ok(enabled) => (StatusCode::OK, Json(ShareAll { enabled })).into_response(),
        Err(e) => {
            warn!("📡 share-all: {e}");
            refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not read that")
        }
    }
}

// `PUT /api/v1/profile/share-all` — let patched decks see everything, or not.
//
// **The third sharing switch, and it answers a third question.**
// `public_now_playing` decides whether a stranger may read the deck;
// `federation_enabled` decides whether this account can be *found*; this one
// decides how much a person you have already let in can see. They are
// deliberately separate because letting one friend in is not the same act as
// being listed publicly, and the settings copy has to carry that difference.
//
// **Off by default and off for every account that predates it.** A patch
// accepted under the old rule was accepted on the understanding that an
// unshared chain published nothing, and nobody consents to more by their
// operator upgrading a binary — the rule `public_now_playing` and
// `federation_enabled` both follow.
//
// It widens what a **patched** deck sees and nothing else. The Instance tab
// still counts shared-chain listens only, because a patch is consent to one
// person and there is no patch behind a figure the whole instance reads; and
// a Shared Spool still captures only through shared chains, because a capture
// enters somebody's history rather than merely being visible.
//
// Session-only, like the other two: a token is held by a scrobble client, and
// no scrobble client has business widening who may read its owner's listening.
/// Let patched decks see every listen, or only shared chains.
///
/// Session-only. **The third sharing switch, and it answers a third
/// question.** `public_now_playing` decides whether a stranger may read
/// the deck; `/profile/federation` decides whether this account can be
/// *found*; this one decides how much a person you have already let in can
/// see.
///
///
/// **Off by default and off for every account that predates it.** A patch
/// accepted under the old rule was accepted on the understanding that a
/// listen through an unshared chain published nothing, and nobody consents
/// to more by their operator upgrading a binary.
///
///
/// On, a patched deck sees every listen — including those through chains
/// you keep private and those with no chain at all — and the chain is
/// named only where you share it, rendering as "Private signal chain"
/// otherwise. It widens what a **patched** deck sees and nothing else: the
/// Instance tab still counts shared-chain listens only, because a patch is
/// consent to one person and there is no patch behind a figure the whole
/// instance reads; and a Shared Spool still captures only through shared
/// chains, because a capture enters somebody's history rather than merely
/// being visible.
///
///
/// `enabled` is required — a body that merely misspelled the key must not
/// be read as an answer.
#[utoipa::path(
    put,
    path = "/api/v1/profile/share-all",
    tag = "Patch",
    request_body = FederationRequest,
    responses(
        (status = 200, description = "Saved.", body = ShareAll),
        (status = 400, description = "`enabled` was absent.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn put_share_all(
    State(state): State<Arc<AppState>>,
    user: super::auth::AuthUser,
    Json(body): Json<FederationRequest>,
) -> Response {
    // Required, not an `Option<bool>` with a default: a body that merely
    // misspelled the key must not be read as an answer, and here the two
    // answers are "show them everything I play" and "no".
    let Some(enabled) = body.enabled else {
        return refused(
            StatusCode::BAD_REQUEST,
            "`enabled` is required — say true or false explicitly.",
        );
    };
    if let Err(e) = state.db.set_share_all_with_patched(user.user_id, enabled).await {
        warn!("📡 could not set share-all for {}: {e}", user.username);
        return refused(StatusCode::INTERNAL_SERVER_ERROR, "Could not save that");
    }
    info!(
        "📡 {} now shows patched decks {}",
        user.username,
        if enabled { "every listen" } else { "only shared chains" }
    );
    (StatusCode::OK, Json(ShareAll { enabled })).into_response()
}

// Turn discoverability on or off.
//
// **Session-only, and permanently.** A token is held by a scrobble client, and
// no scrobble client has business deciding that its owner is findable on the
// fediverse. Same line `/api/v1/sources`, the password endpoints and the
// now-playing switch sit on: `AuthUser`, so a token gets 401 and the question
// of scopes never arises.
//
// Switching **on** mints the keypair then and there rather than leaving it to
// the first stranger's fetch. It costs a moment on a screen where somebody is
// waiting anyway, and it means the settings page can show the real actor URI
// immediately instead of a promise.
//
// Switching **off** stops answering and deletes nothing. The keypair stays,
// because destroying it is destroying the identity: anyone who has already
// patched in holds a key that would never verify again, and turning the switch
// back on would present them with a stranger wearing the same name.
/// Turn discoverability on or off.
///
/// **Session-only, permanently.** A token is held by a scrobble client, and
/// no scrobble client has business deciding that its owner is findable on
/// the fediverse — so a token gets 401 and scopes never enter into it, the
/// same line `/api/v1/sources` and the password endpoints sit on.
///
/// Switching on mints the RSA-2048 keypair then and there, so the screen
/// can show the real actor URI rather than a promise.
///
/// Switching off stops answering and **deletes nothing**. Destroying the
/// keypair would destroy the identity: anyone already patched in holds a
/// key that would never verify again, and switching back on would present
/// them with a stranger wearing the same name.
///
/// `enabled` is required. A body that merely misspelled the key is a 400,
/// never a default — this is the one switch where guessing wrong publishes
/// something.
#[utoipa::path(
    put,
    path = "/api/v1/profile/federation",
    tag = "Patch",
    security(("sessionCookie" = [])),
    request_body = FederationRequest,
    responses(
        (status = 200, description = "The switch as it now stands.", body = FederationSwitch),
        (status = 400, description = "`enabled` was absent.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
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

/// The actor document for a deck.
///
/// **Content-negotiated.** A caller whose `Accept` names
/// `application/activity+json` or `application/ld+json` gets the actor
/// document, served as `application/activity+json` — a server handed plain
/// `application/json` is entitled to treat it as untyped JSON, with the
/// result that the actor "does not exist" while this returns 200. Anything
/// else that names `text/html` gets the SPA shell, because it is a person.
/// A bare `*/*` gets the document.
///
/// The document carries `publicKey`, `inbox`, `outbox`,
/// `preferredUsername`, `name`, `type: Person` and
/// `manuallyApprovesFollowers: true` — private by default, stated to the
/// network rather than only enforced at home.
///
/// **Three things are deliberately absent**, because advertising an
/// endpoint that 404s is worse than not having it: `endpoints.sharedInbox`
/// (there is no `POST /inbox`), and the `followers` / `following`
/// collections (Step 2 serves those, when `patches` rows exist to fill
/// them).
///
/// The `id` is written once, when the keypair is minted, and never
/// re-derived from today's `PUBLIC_URL` — recomputing it would silently
/// rename an actor that remote servers have cached.
#[utoipa::path(
    get,
    path = "/users/{name}",
    tag = "Patch",
    security(),
    params(("name" = String, Path)),
    responses(
        (status = 200, description = "The actor document, or the SPA shell for a browser.", content((serde_json::Value = "application/activity+json"), (String = "text/html"))),
        (status = 404, description = "No such user, or they have not switched federation on. Identical answers — anything that told them apart would make this a membership oracle for a private, invite-only instance.", body = ErrorResponse),
    )
)]
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

// Empty until delivery exists, and served anyway.
//
// Remote servers probe an outbox to decide whether an actor is worth
// following, and a 404 here reads as a broken actor rather than a quiet one.
// The access control is in the query — `visibility = 'public'` — because this
// collection is fetched with no credential at all.
/// An actor's public activities.
///
/// An `OrderedCollection`, **empty until outbound delivery exists** and
/// served anyway: remote servers probe an outbox to decide whether an actor
/// is real, and a 404 here reads as a broken actor rather than a quiet one.
///
/// Only activities stored `visibility = 'public'` appear. That filter is in
/// the query rather than in the handler because this collection is fetched
/// with no credential at all — a private or patched-in activity reaching it
/// is published to the internet, and nothing downstream would notice.
///
/// No `first` / `next` paging yet. An empty collection has no page to point
/// at, and inventing one would advertise a third endpoint that does not
/// exist; paging arrives with delivery.
#[utoipa::path(
    get,
    path = "/users/{name}/outbox",
    tag = "Patch",
    security(),
    params(("name" = String, Path)),
    responses(
        (status = 200, description = "An OrderedCollection with `totalItems` and `orderedItems`.", content_type = "application/activity+json", body = serde_json::Value),
        (status = 404, description = "No such user, or not discoverable. Identical answers.", body = ErrorResponse),
    )
)]
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

// Who has patched into this deck, and who it has patched into.
//
// **The count, not the roster.** ActivityPub allows a collection to state its
// size without enumerating it, and that is the right default for a private
// invite-only instance: who somebody has accepted is a fact about *other
// people*, and this endpoint answers to anyone at all. Remote software uses
// the number; the list is what would leak the shape of a household.
//
// Both are served now rather than left out, because the actor document
// advertises them — and advertising an endpoint that 404s is the thing Step 1
// deliberately would not do.
/// How many decks have patched into this one.
///
/// **The count, not the roster.** ActivityStreams allows a collection to
/// state its size without enumerating it, and that is the right default
/// here: who somebody has accepted is a fact about *other people*, and this
/// endpoint answers to anyone at all. Remote software uses `totalItems`;
/// the list is what would leak the shape of a household.
///
/// Served — and therefore advertised in the actor document — as of Step 2.
/// It was deliberately absent before that, because advertising an endpoint
/// that 404s is worse than not having one.
#[utoipa::path(
    get,
    path = "/users/{name}/followers",
    tag = "Patch",
    security(),
    params(("name" = String, Path)),
    responses(
        (status = 200, description = "An OrderedCollection carrying `totalItems` and no items.", content_type = "application/activity+json", body = serde_json::Value),
        (status = 404, description = "No such user, or not discoverable. Identical answers.", body = ErrorResponse),
    )
)]
async fn followers(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    collection(state, name, headers, true).await
}

/// How many decks this one has patched into.
///
/// The mirror of `/followers`, and the same rule: `totalItems` only.
#[utoipa::path(
    get,
    path = "/users/{name}/following",
    tag = "Patch",
    security(),
    params(("name" = String, Path)),
    responses(
        (status = 200, description = "An OrderedCollection carrying `totalItems` and no items.", content_type = "application/activity+json", body = serde_json::Value),
        (status = 404, description = "No such user, or not discoverable. Identical answers.", body = ErrorResponse),
    )
)]
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

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct WebfingerQuery {
    /// `acct:name@host`, or the actor URI.
    #[param(required = true, example = "acct:ash@deck.example")]
    resource: Option<String>,
}

// The entry point for the whole protocol: it is what somebody typing a handle
// into another server's search box hits first.
/// Resolve a handle to an actor.
///
/// RFC 7033. The entry point for the whole protocol — it is what somebody
/// typing a handle into another server's search box hits first. Served as
/// `application/jrd+json`.
///
/// `resource` is accepted as `acct:ash@host`, a bare `ash@host`,
/// `@ash@host`, or the actor URI itself; clients send all of these, and
/// refusing the unfashionable ones makes the instance undiscoverable from
/// software that is otherwise fine.
///
/// A missing or unreadable `resource` is a **400** — the caller asked
/// wrong. A well-formed one naming a host that is not ours, or a person we
/// do not publish, is a **404**. Only the second is a fact about our users,
/// which is why the two statuses are drawn apart here and never within the
/// 404.
#[utoipa::path(
    get,
    path = "/.well-known/webfinger",
    tag = "Patch",
    security(),
    params(WebfingerQuery),
    responses(
        (status = 200, description = "A JRD with a `self` link to the actor document.", content_type = "application/jrd+json", body = serde_json::Value),
        (status = 400, description = "No `resource`, or one that names no actor.", body = ErrorResponse),
        (status = 404, description = "Another host, or somebody we do not publish. Identical answers.", body = ErrorResponse),
    )
)]
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

// Take a delivery, having established that it is really from who it says.
//
// Nothing acts on what arrives yet — Step 2 is what turns a pending `Follow`
// into a `patches` row. What this does is the part that cannot be added later
// without a gap in the record: verify, and file.
/// Accept a signed delivery.
///
/// Verifies an HTTP signature and files the activity. **Nothing acts on
/// what arrives yet** — Step 2 is what turns a pending `Follow` into a
/// patch.
///
/// Requires a draft-cavage `Signature:` header, `rsa-sha256`. The signature
/// must cover `(request-target)`, `host` and `date`, plus `digest` whenever
/// there is a body — checked *before* any cryptography, because a signature
/// that verifies over the wrong set of headers is worse than none: it looks
/// like it proved something. `Date` must be within five minutes.
///
/// The signing key is fetched from the actor the `keyId` names and cached.
/// A signature that fails against a cached key triggers exactly one
/// refetch, which is what makes key rotation survivable.
///
/// **The signature says who sent this; the activity's `actor` says who
/// wrote it.** A delivery where they disagree is refused rather than filed
/// under either — that disagreement is how an impersonated `Follow` or
/// `Delete` gets in.
///
/// An activity with no `id` is refused: it could not be deduplicated,
/// referred to by a later `Undo`, or checked against a redelivery.
///
/// `GET` and `DELETE` here are 405 — both belonged to older revisions of
/// the protocol.
#[utoipa::path(
    post,
    path = "/users/{name}/inbox",
    tag = "Patch",
    security(),
    params(("name" = String, Path)),
    request_body(content = serde_json::Value, content_type = "application/activity+json"),
    responses(
        (status = 202, description = "Filed. Also the answer to a redelivery — senders retry, and reporting a duplicate as an error is what makes one retry harder."),
        (status = 400, description = "Not JSON, or the activity names no `actor` or no `id`.", body = ErrorResponse),
        (status = 401, description = "Unsigned, stale, badly covered, the digest does not match the body, the signature does not verify, or the signing key does not belong to the actor the activity is attributed to. The message says which.", body = ErrorResponse),
        (status = 404, description = "No such user, or not discoverable. Identical answers.", body = ErrorResponse),
    )
)]
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

// The client a stranger's URL is fetched with. It used to be a plain client
// with `Policy::limited(2)`, and `fetch_actor` checked only the first URL: a
// `keyId` on a public https host could answer 302 to http://127.0.0.1/ or to
// the cloud metadata address, and the body came back as the actor's key
// (td-security#8). `Egress::Stranger` judges every hop — https only, and the
// *resolved* address public — which also closes a public name whose DNS points
// inward, something `is_private_host` (a check on spelling) never could.
fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| Egress::Stranger.client(FETCH_TIMEOUT))
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
    if let Err(e) = Egress::Stranger.check_url(uri) {
        warn!("📡 refusing to fetch an actor at {uri}: {e}");
        return None;
    }

    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return None;
    }

    let request = http()
        .get(uri)
        .header(header::ACCEPT, "application/activity+json, application/ld+json")
        .header(header::USER_AGENT, concat!("Tapedeck/", env!("CARGO_PKG_VERSION")))
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
