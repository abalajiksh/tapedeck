//! Sending a generated list of tracks to a media server as a playlist.
//!
//! Tapedeck has no playback and is not going to grow any, so a list it builds is
//! only useful if it can reach the thing that *does* play music. Every list in
//! the app is a candidate — Rediscovery's seven collections today, the genre-map
//! playlists later — which is why this is a general endpoint taking bare tracks
//! rather than something bolted onto one screen.
//!
//! Four rules it keeps:
//!
//! * **The targets are whatever the user already connected.** No new credential
//!   and no new settings page: `GET /playlist-targets` is their enabled
//!   `user_sources` rows, so the button offers Plex if they have Plex and
//!   nothing if they have nothing. A source that can be polled can be written
//!   to.
//! * **Session-only, like sources themselves.** Pushing spends the credential of
//!   a configured server, which puts it on the same side of the line as
//!   `/api/v1/sources` — and that line is the one `WriteAuth` deliberately does
//!   not cross. A token can love a track; it cannot reach the user's Plex.
//! * **A partial result is reported.** Not every listen in a history is on every
//!   server, so a push of twelve landing nine is the normal case, not a failure.
//!   The response names the three that did not.
//! * **Nothing is invented.** The tracks come from the caller's own history and
//!   a search hit is checked by `playlists::pick` before it is used. The one
//!   thing this must never do is put something on a playlist that isn't what was
//!   asked for.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};
use utoipa::ToSchema;

use super::auth::{AuthUser, ReadAuth};
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{PlaylistTrackView, SavedPlaylistRecord, UserSource};
use crate::openapi::{BadRequest, Unauthorized};
use crate::playlists::{self, Miss, PlaylistTarget, PlaylistTrack};
use crate::sources::{MediaBrowserSource, NavidromeSource, PlexFilters, PlexSource};

/// How many tracks one push may carry.
///
/// Each one costs a search against the user's own server, run in turn, so this
/// is a wall-clock bound as much as a politeness one: a few hundred would sit
/// past any reverse proxy's gateway timeout and come back as a 502 with the
/// playlist half made.
const MAX_TRACKS: usize = 100;

/// A server this user can push a playlist to.
#[derive(Clone, Serialize, ToSchema)]
#[schema(as = PushTarget)]
pub struct TargetView {
    /// What `source_id` takes. `-1` is TIDAL, which is a connection rather
    /// than a source.
    pub source_id: i64,
    /// `plex`, `jellyfin`, `navidrome` or `tidal`.
    pub kind: String,
    /// `Plex`, `Jellyfin`, `Subsonic` or `TIDAL`.
    pub protocol: &'static str,
    /// Falls back to the protocol name when the source has none.
    pub label: String,
    pub account: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PushTargets {
    /// Enabled, writable sources, and TIDAL when it is connected. Empty is a
    /// normal answer.
    pub targets: Vec<TargetView>,
}

/// What was made, and **every track that did not land**, with why.
#[derive(Serialize, ToSchema)]
pub struct PlaylistPushed {
    /// The server's own id. Empty when a Subsonic server answered `ok`
    /// without one.
    pub playlist_id: String,
    pub name: String,
    pub protocol: &'static str,
    pub target: TargetView,
    pub requested: usize,
    pub matched: usize,
    /// How many were answered from remembered matches without asking the far
    /// end. Nearly everything on a re-push.
    pub from_cache: usize,
    /// A partial match is the normal outcome — a listening history is not the
    /// same set as any one library.
    pub missing: Vec<MissingTrack>,
    /// A playlist of this name was pushed here before and was rewritten in
    /// place rather than made again.
    pub updated_existing: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MissingTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    #[serde(flatten)]
    pub why: Miss,
}

#[derive(Serialize, ToSchema)]
pub struct SavedPlaylists {
    pub playlists: Vec<SavedPlaylistRecord>,
}

#[derive(Serialize, ToSchema)]
pub struct SavedPlaylistDetail {
    pub playlist: SavedPlaylistRecord,
    /// With the plays and artwork the history already holds.
    pub tracks: Vec<PlaylistTrackView>,
}

#[derive(Serialize, ToSchema)]
pub struct PlaylistSaved {
    pub id: i64,
}

#[derive(Serialize, ToSchema)]
pub struct PlaylistDeleted {
    pub deleted: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MatchSet {
    /// `matched` or `cleared`.
    pub status: &'static str,
}

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

/// Which source kinds can receive a playlist, and what protocol that is.
///
/// A kind absent from here is not a bug — it is a source that polls and cannot
/// be written to, and the UI says so rather than offering a button that fails.
fn protocol_for(kind: &str) -> Option<&'static str> {
    match kind {
        "plex" => Some("Plex"),
        "jellyfin" => Some("Jellyfin"),
        // Named for the protocol: this is `search3` + `createPlaylist`, so
        // anything speaking OpenSubsonic works through the same two calls.
        "navidrome" => Some("Subsonic"),
        _ => None,
    }
}

/// Build the client for one source row, or say what is missing.
///
/// Mirrors `artwork.rs`'s dispatch deliberately — same rows, same credentials,
/// same three kinds. Kept separate rather than shared because the artwork proxy
/// answers 404 for every failure (the UI falls back to a lettered tile) while a
/// push has to report *why* it could not go.
fn build_target(source: &UserSource) -> Result<Box<dyn PlaylistTarget>, String> {
    let egress = source.egress();
    egress.check_url(&source.url).map_err(|e| e.to_string())?;
    match source.kind.as_str() {
        "plex" => {
            let token = source.credential("token").ok_or("Plex token is missing")?;
            Ok(Box::new(PlexSource::with_filters(
                source.url.clone(),
                token,
                PlexFilters::default(),
                egress,
            )))
        }
        "jellyfin" => {
            let key = source.credential("api_key").ok_or("Jellyfin API key is missing")?;
            Ok(Box::new(MediaBrowserSource::jellyfin(
                source.url.clone(),
                key,
                source.remote_account.clone().unwrap_or_default(),
                egress,
            )))
        }
        "navidrome" => {
            let (Some(u), Some(p)) = (source.credential("username"), source.credential("password"))
            else {
                return Err("Navidrome credentials are missing".into());
            };
            Ok(Box::new(NavidromeSource::new(source.url.clone(), u, p, egress)))
        }
        other => Err(format!("A {other} source cannot receive a playlist")),
    }
}

/// How a TIDAL target is addressed.
///
/// Every other target is a row in `user_sources` and is named by its id. TIDAL
/// is a *connection*, not a source — no URL, an expiring credential, and
/// write-only — so it has no such row. A reserved negative id lets it share the
/// existing `source_id` shape rather than making the endpoint, the MCP tool,
/// the spec and the UI all learn a second way to name a target. Row ids come
/// from `AUTOINCREMENT` and are always positive, so this can never collide.
pub const TIDAL_TARGET_ID: i64 = -1;

fn target_view(s: &UserSource, protocol: &'static str) -> TargetView {
    TargetView {
        source_id: s.id,
        kind: s.kind.clone(),
        protocol,
        // The label is what tells two Plex servers apart; without one, the
        // protocol name is a better button than a blank.
        label: if s.label.trim().is_empty() { protocol.to_string() } else { s.label.clone() },
        account: s.remote_account.clone(),
    }
}

fn tidal_target() -> TargetView {
    TargetView {
        source_id: TIDAL_TARGET_ID,
        kind: "tidal".into(),
        protocol: "TIDAL",
        label: "TIDAL".into(),
        account: None,
    }
}

// GET /api/v1/playlist-targets — the servers this user could push to.
//
// Enabled sources only. A disabled source keeps its credentials on purpose, but
// the user has said they don't want Tapedeck talking to it, and writing a
// playlist is talking to it.
/// Servers a generated list can be sent to.
///
/// Session-only, and permanently so: pushing spends the credential of a
/// configured source, which puts it on the same side of the line as
/// `/api/v1/sources` — the line API tokens do not cross.
///
/// The targets are the caller's **enabled** sources, filtered to the kinds
/// that can receive a playlist. `protocol` names the wire protocol rather
/// than the product, so a Navidrome source reads as `Subsonic`: the calls
/// are `search3` and `createPlaylist`, which any OpenSubsonic server
/// answers. A kind missing from the list is a source that polls and cannot
/// be written to, not a bug.
#[utoipa::path(
    get,
    path = "/api/v1/playlist-targets",
    tag = "Playlists",
    responses(
        (status = 200, description = "OK.", body = PushTargets),
        (status = 401, response = Unauthorized),
    )
)]
async fn list_targets(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let targets = targets_for(&state, user.user_id).await;
    (StatusCode::OK, Json(PushTargets { targets })).into_response()
}

/// The push targets for one user.
///
/// Split out of the handler so the MCP surface offers exactly the same list.
/// An empty vec is a normal answer — plenty of instances have no writable
/// source at all — and both callers say so rather than showing a button or a
/// tool that cannot work.
pub async fn targets_for(state: &AppState, user_id: i64) -> Vec<TargetView> {
    let mut targets: Vec<TargetView> = match state.db.list_sources_for_user(user_id).await {
        Ok(rows) => rows
            .iter()
            .filter(|s| s.enabled)
            .filter_map(|s| protocol_for(&s.kind).map(|p| target_view(s, p)))
            .collect(),
        Err(e) => {
            warn!("Listing playlist targets failed: {e}");
            Vec::new()
        }
    };

    // Offered only when the account is actually connected — the same call the
    // rest of this list makes. A button that cannot work is worse than no
    // button, which is why the endpoint returns an empty list rather than a
    // disabled entry.
    if crate::server::tidal::credentials(state, user_id).await.is_some() {
        targets.push(tidal_target());
    }
    targets
}

#[derive(Deserialize, ToSchema)]
#[schema(as = PlaylistPush)]
struct PushRequest {
    /// From `/api/v1/playlist-targets`.
    source_id: i64,
    /// Trimmed, and truncated to 120 characters.
    name: String,
    /// In playlist order.
    #[schema(max_items = 100)]
    tracks: Vec<PlaylistTrack>,
}

// POST /api/v1/playlists — resolve every track against one server and create
// the playlist from whatever was found.
/// Create a playlist on one of your servers from a list of tracks.
///
/// Session-only — see `/api/v1/playlist-targets`.
///
/// Tapedeck stores **names, not the server's item ids**, for a generated
/// list, so every track is resolved by searching the target and every one of
/// those searches is fuzzy. A hit is checked on artist *and* title before it
/// is used; an exact title beats a `(Remastered)` one, and a hit for a
/// different artist is discarded rather than taken.
///
/// **A partial push is a success, not an error.** A listening history is not
/// the same set as any one library, so `matched` below `requested` is the
/// normal case and `missing` names every track that did not land. The push
/// only fails outright when nothing matched (`404`) or the server refused or
/// could not be reached (`502`) — the two need completely different things
/// from the user, so they are not collapsed.
///
/// Order is preserved; misses drop out. At most 100 tracks, which is a
/// wall-clock bound as much as a politeness one — the searches run in turn
/// against what is often a Raspberry Pi.
#[utoipa::path(
    post,
    path = "/api/v1/playlists",
    tag = "Playlists",
    request_body = PushRequest,
    responses(
        (status = 200, description = "Created — possibly with fewer tracks than asked for.", body = PlaylistPushed),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such source, or none of those tracks are on it.", body = ErrorResponse),
        (status = 502, description = "The server refused the playlist or could not be reached.", body = ErrorResponse),
        (status = 501, description = "TIDAL was named and is not configured on this instance.", body = ErrorResponse),
    )
)]
async fn push(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<PushRequest>,
) -> impl IntoResponse {
    match push_tracks(&state, user.user_id, body.source_id, &body.name, &body.tracks).await {
        Ok(pushed) => (StatusCode::OK, Json(pushed)).into_response(),
        Err(PushError { code, message }) => err(code, message),
    }
}

/// A push that could not go, with the status the HTTP layer should use.
///
/// The code matters and is not decoration: `404` is "none of this is in your
/// library" and `502` is "your server stopped answering". They need completely
/// different things from the user, and collapsing them into one failure is what
/// makes a push feel broken when it merely found nothing.
pub struct PushError {
    pub code: u16,
    pub message: String,
}

impl From<PushError> for String {
    fn from(e: PushError) -> String {
        e.message
    }
}

/// Resolve every track against one server and create the playlist from what was
/// found.
///
/// Shared by the HTTP endpoint and the MCP `push_playlist` tool. The rule this
/// exists to keep in one place is the partial result: a listening history is not
/// the same set as any one library, so `matched < requested` is the normal case
/// and **every track that did not land is named**. Sending nine of twelve and
/// calling it done is the failure this codebase keeps finding in other forms.
pub async fn push_tracks(
    state: &AppState,
    user_id: i64,
    source_id: i64,
    name: &str,
    tracks: &[PlaylistTrack],
) -> Result<PlaylistPushed, PushError> {
    let fail = |code: u16, message: String| PushError { code, message };

    let name = name.trim();
    if name.is_empty() {
        return Err(fail(400, "A playlist needs a name".into()));
    }
    // Long enough for "Tapedeck · Forgotten Favourites · March 2026", short
    // enough that no server truncates it into something unrecognisable.
    let name: String = name.chars().take(120).collect();

    if tracks.is_empty() {
        return Err(fail(400, "A playlist needs at least one track".into()));
    }
    if tracks.len() > MAX_TRACKS {
        return Err(fail(400, format!("That's more than {MAX_TRACKS} tracks")));
    }

    // TIDAL is a connection rather than a source, so it is resolved before the
    // `user_sources` lookup that every other target goes through. Held as an
    // `Arc` as well as a trait object because a push can outlive its access
    // token, and the refreshed pair has to be persisted afterwards.
    let mut tidal_handle: Option<std::sync::Arc<crate::tidal::TidalClient>> = None;
    // Built inside whichever branch resolves the target, so the response can
    // describe it without the source row being in scope afterwards.
    let mut target_desc = tidal_target();
    let (protocol, target): (&'static str, Box<dyn playlists::PlaylistTarget>) = if source_id
        == TIDAL_TARGET_ID
    {
        let Some(app) = crate::tidal::TidalApp::resolve(&state.db).await else {
            return Err(fail(501, "TIDAL is not configured on this instance".into()));
        };
        let Some(creds) = crate::server::tidal::credentials(state, user_id).await else {
            return Err(fail(400, "No TIDAL account is connected".into()));
        };
        let client = crate::tidal::TidalClient::new(app, creds)
            .map_err(|e| fail(500, e))
            .map(std::sync::Arc::new)?;
        tidal_handle = Some(client.clone());
        ("TIDAL", Box::new(client))
    } else {
        // Scoped before anything else: sources are per user, and an id from
        // someone else's row would push through their credentials.
        let source = match state.db.get_source(user_id, source_id).await {
            Ok(Some(s)) => s,
            Ok(None) => return Err(fail(404, "No such source".into())),
            Err(e) => return Err(fail(500, e.to_string())),
        };
        if !source.enabled {
            return Err(fail(400, "That source is switched off".into()));
        }
        let Some(protocol) = protocol_for(&source.kind) else {
            return Err(fail(400, format!("A {} source cannot receive a playlist", source.kind)));
        };
        let target = match build_target(&source) {
            Ok(t) => t,
            Err(msg) => return Err(fail(400, msg)),
        };
        target_desc = target_view(&source, protocol);
        (protocol, target)
    };
    // The ISRCs the history holds for these tracks, in one bounded pass. Every
    // miss below is ultimately a name that did not agree, and an ISRC is the
    // one key that sidesteps names entirely — so it is worth resolving even
    // though only some targets can use it.
    let isrcs = {
        let pairs: Vec<(String, String)> =
            tracks.iter().map(|t| (t.title.clone(), t.artist.clone())).collect();
        state.db.isrc_map(user_id, &pairs).await.unwrap_or_default()
    };
    let with_isrc: Vec<PlaylistTrack> = tracks
        .iter()
        .map(|t| {
            let mut t = t.clone();
            if t.isrc.is_none() {
                let key = (t.title.trim().to_lowercase(), t.artist.trim().to_lowercase());
                t.isrc = isrcs.get(&key).cloned();
            }
            t
        })
        .collect();
    let tracks: &[PlaylistTrack] = &with_isrc;

    // Sequential on purpose, for the same reason the Rediscovery queries are:
    // this is someone's Raspberry Pi, and firing a hundred searches at it at
    // once is how a push turns into a timeout. It also keeps the order the
    // caller asked for, which is the order the playlist ends up in.
    let mut ids: Vec<String> = Vec::with_capacity(tracks.len());
    let mut missing: Vec<(&PlaylistTrack, playlists::Miss)> = Vec::new();
    let mut search_error: Option<String> = None;

    let mut from_cache = 0usize;

    for want in tracks {
        // **The cheapest search is the one not made.** A re-push of a list
        // where most tracks already resolved should cost almost nothing, which
        // is the difference between the second push of an evening working and
        // being throttled into reporting real tracks as absent.
        if let Ok(Some(id)) =
            state.db.playlist_match(user_id, source_id, &want.artist, &want.title).await
        {
            ids.push(id);
            from_cache += 1;
            continue;
        }
        match target.search_tracks(want).await {
            Ok(candidates) => match playlists::pick(want, &candidates) {
                Some(id) => {
                    // Remembered as a guess: `manual` is reserved for a person
                    // choosing from the candidates, and must not be overwritten
                    // by whatever the matcher decides next time.
                    let label = candidates
                        .iter()
                        .find(|c| c.id == id)
                        .map(|c| format!("{} — {}", c.title, c.artist));
                    if let Err(e) = state
                        .db
                        .remember_playlist_match(
                            user_id,
                            source_id,
                            &want.artist,
                            &want.title,
                            &id,
                            label.as_deref(),
                            false,
                        )
                        .await
                    {
                        debug!("Could not remember a playlist match: {e}");
                    }
                    ids.push(id);
                }
                // **Why** it missed, not just that it did. A server that
                // returned nothing has a search problem; one that returned the
                // right song and had it refused has a matching problem. Both
                // used to read as the same bare miss, which is no help at all
                // to whoever has to go and fix it.
                None => missing.push((want, playlists::explain(want, &candidates))),
            },
            Err(e) => {
                // One failed search is a miss; a server that has stopped
                // answering is a failed push. Remember the first reason so the
                // caller gets it rather than "nothing matched".
                warn!("Playlist push: search failed on {protocol}: {e}");
                search_error.get_or_insert(e);
                missing.push((want, playlists::Miss::SearchFailed));
            }
        }
    }

    let missing: Vec<MissingTrack> = missing
        .into_iter()
        .map(|(t, why)| MissingTrack {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            why,
        })
        .collect();

    if ids.is_empty() {
        // Distinguish "your server said no" from "none of this is in your
        // library" — they need completely different things from the user.
        return Err(match search_error {
            Some(e) => fail(502, format!("{protocol} could not be reached: {e}")),
            None => {
                fail(404, format!("None of those {} tracks are on this {protocol}", tracks.len()))
            }
        });
    }

    // **Update in place where we made it before.**
    //
    // Pushing the same list three times used to make three playlists on the far
    // end, cleaned up by hand — and a retrying agent is exactly the caller that
    // does that. `save_playlist` has been idempotent since v0.63.0 through its
    // `idempotency_key`; this is the same property for the push.
    //
    // A target that cannot rewrite a playlist answers `Ok(false)` and falls
    // through to creating one, which is also what happens when the remembered
    // playlist has been deleted on the far end. An `Err` is different and is
    // **not** papered over by creating a second playlist: the list was found and
    // the rewrite went wrong, so there is a playlist out there in a state the
    // user needs told about rather than a duplicate sitting beside it.
    let remembered = state.db.pushed_playlist(user_id, source_id, &name).await.unwrap_or(None);
    let rewritten: Option<Result<String, String>> = match remembered.as_deref() {
        Some(existing) => match target.replace_playlist(existing, &ids).await {
            Ok(true) => Some(Ok(existing.to_string())),
            // "I cannot rewrite", or the remembered playlist is gone. Both mean
            // fall through and make one.
            Ok(false) => None,
            Err(e) => Some(Err(e)),
        },
        None => None,
    };
    let reused = matches!(rewritten, Some(Ok(_)));
    let created = match rewritten {
        Some(outcome) => outcome,
        None => target.create_playlist(&name, &ids).await,
    };

    // A push makes one request per track, so a token can legitimately expire
    // partway through one. The client refreshes itself and hands the new pair
    // back here, because it cannot write to the database — `Database` owns the
    // encryption and ciphertext never leaves `db.rs`. Persisted whether or not
    // the playlist itself succeeded: the old token is dead either way, and
    // dropping the new one would make the next push fail for no reason.
    if let Some(handle) = tidal_handle {
        if let Some(fresh) = handle.take_refreshed() {
            if let Err(e) = crate::server::tidal::store(state, user_id, &fresh, None).await {
                warn!("Could not persist the refreshed TIDAL token: {e}");
            }
        }
    }

    match created {
        Ok(playlist_id) => {
            info!(
                "📀 {} \"{}\" on {} — {}/{} tracks",
                if reused { "Updated" } else { "Pushed" },
                name,
                protocol,
                ids.len(),
                tracks.len()
            );
            // Remembered whether it was made now or reused, so a playlist the
            // user renamed on the far end does not strand the mapping.
            if let Err(e) =
                state.db.remember_pushed_playlist(user_id, source_id, &name, &playlist_id).await
            {
                warn!("Could not remember the pushed playlist: {e}");
            }
            Ok(PlaylistPushed {
                playlist_id,
                name,
                protocol,
                target: target_desc,
                requested: tracks.len(),
                matched: ids.len(),
                // How much of that was answered without asking the far end.
                // Worth reporting rather than hiding: on a re-push it should be
                // nearly everything, and if it is not, the cache is not working.
                from_cache,
                missing,
                // So a caller can tell "I made you one" from "I rewrote the one
                // you already had", which are different things to say to
                // somebody looking at their library.
                updated_existing: reused,
            })
        }
        Err(e) => Err(fail(502, format!("{protocol} refused the playlist: {e}"))),
    }
}

// ════════════════════════════════════════════════════════════
//  Playlists that live in Tapedeck
// ════════════════════════════════════════════════════════════
//
// Until now every list this application built went straight out to a media
// server, because every one of them was computed on demand and could be
// computed again. A list handed back by a chatbot is the first that cannot: it
// is the product of a conversation, nobody has looked at it yet, and the point
// of storing it is that somebody does before it reaches the household's Plex.

/// Playlists kept in Tapedeck.
///
/// Scope: `read` *(since v0.120)*. `origin` is `user` for one you built, or the name of the
/// MCP connection that wrote it — a list an assistant produced stays
/// distinguishable from one you made, the same provenance rule notes
/// follow.
#[utoipa::path(
    get,
    path = "/api/v1/saved-playlists",
    tag = "Playlists",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "The playlists.", body = SavedPlaylists),
        (status = 401, response = Unauthorized),
    )
)]
async fn list_saved(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    match state.db.list_playlists(user.user_id).await {
        Ok(rows) => (StatusCode::OK, Json(SavedPlaylists { playlists: rows })).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

/// One playlist and its tracks.
///
/// Scope: `read` *(since v0.120)*.
#[utoipa::path(
    get,
    path = "/api/v1/saved-playlists/{id}",
    tag = "Playlists",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "The playlist.", body = SavedPlaylistDetail),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such playlist.", body = ErrorResponse),
    )
)]
async fn get_saved(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    let Ok(all) = state.db.list_playlists(user.user_id).await else {
        return err(500, "Could not read playlists");
    };
    let Some(meta) = all.into_iter().find(|p| p.id == id) else {
        return err(404, "No such playlist");
    };
    // Enriched with the artwork and play counts the history already holds, so
    // this page reads like the rest of the application rather than like a text
    // file sitting next to Rediscovery's sleeves.
    match state.db.playlist_tracks_detailed(user.user_id, id).await {
        Ok(tracks) => {
            (StatusCode::OK, Json(SavedPlaylistDetail { playlist: meta, tracks })).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(as = PlaylistToSave)]
struct SavePlaylistRequest {
    name: String,
    description: Option<String>,
    /// In playlist order. At least one.
    tracks: Vec<crate::db::SavedPlaylistTrack>,
}

/// Save a playlist.
///
/// Session-only. Tracks are **names, not file paths** — Tapedeck stores no
/// server item ids for a generated list and resolves them against the real
/// library at push time.
#[utoipa::path(
    post,
    path = "/api/v1/saved-playlists",
    tag = "Playlists",
    request_body = SavePlaylistRequest,
    responses(
        (status = 200, description = "Saved.", body = PlaylistSaved),
        (status = 400, description = "No name, or no tracks.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn save_saved(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SavePlaylistRequest>,
) -> impl IntoResponse {
    let name = body.name.trim();
    if name.is_empty() {
        return err(400, "A playlist needs a name");
    }
    if body.tracks.is_empty() {
        return err(400, "A playlist needs at least one track");
    }
    // `origin: "user"` — this one they built themselves, which is what the
    // playlists page distinguishes from one a connection wrote.
    match state
        .db
        .save_playlist(
            user.user_id,
            name,
            body.description.as_deref(),
            "user",
            None,
            None,
            &body.tracks,
        )
        .await
    {
        Ok((id, _)) => (StatusCode::OK, Json(PlaylistSaved { id })).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

/// Delete a playlist.
#[utoipa::path(
    delete,
    path = "/api/v1/saved-playlists/{id}",
    tag = "Playlists",
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "Deleted.", body = PlaylistDeleted),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such playlist.", body = ErrorResponse),
    )
)]
async fn delete_saved(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_playlist(user.user_id, id).await {
        Ok(true) => (StatusCode::OK, Json(PlaylistDeleted { deleted: true })).into_response(),
        Ok(false) => err(404, "No such playlist"),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(as = PlaylistDestination)]
struct PushSavedRequest {
    /// From `/api/v1/playlist-targets`.
    source_id: i64,
}

// POST /api/v1/saved-playlists/:id/push — send a stored playlist onward.
//
// The review step the whole design turns on: a list an LLM wrote is saved
// first and reaches a media server only when the person whose server it is
// says so.
/// Send a stored playlist to a media server.
///
/// Session-only — pushing spends the credential of a configured source,
/// which is the line `WriteAuth` deliberately does not cross.
///
/// **This is the review step the MCP design turns on**: a list an assistant
/// built is saved first and reaches a media server only when the person
/// whose server it is says so.
///
/// A partial match is the normal outcome — a listening history is not the
/// same set as any one library — and every track that did not land is
/// named. `404` means nothing matched; `502` means the server could not be
/// reached. They need completely different things from the user.
#[utoipa::path(
    post,
    path = "/api/v1/saved-playlists/{id}/push",
    tag = "Playlists",
    params(("id" = i64, Path)),
    request_body = PushSavedRequest,
    responses(
        (status = 200, description = "Sent, wholly or partly.", body = PlaylistPushed),
        (status = 400, description = "The source is off, or cannot receive a playlist.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such playlist, or none of the tracks are on that server.", body = ErrorResponse),
        (status = 502, description = "The server could not be reached, or refused the playlist.", body = ErrorResponse),
        (status = 501, description = "TIDAL was named and is not configured on this instance.", body = ErrorResponse),
    )
)]
async fn push_saved(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(body): Json<PushSavedRequest>,
) -> impl IntoResponse {
    let Ok(all) = state.db.list_playlists(user.user_id).await else {
        return err(500, "Could not read playlists");
    };
    let Some(meta) = all.into_iter().find(|p| p.id == id) else {
        return err(404, "No such playlist");
    };
    let tracks = match state.db.playlist_tracks(user.user_id, id).await {
        Ok(t) => t,
        Err(e) => return err(500, e.to_string()),
    };
    let wanted: Vec<PlaylistTrack> = tracks
        .iter()
        .map(|t| PlaylistTrack {
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            // Resolved centrally in `push_tracks`, so every caller gets it and
            // none has to know where an ISRC lives.
            isrc: None,
        })
        .collect();

    match push_tracks(&state, user.user_id, body.source_id, &meta.name, &wanted).await {
        Ok(pushed) => {
            let _ = state.db.mark_playlist_pushed(user.user_id, id, pushed.protocol).await;
            (StatusCode::OK, Json(pushed)).into_response()
        }
        Err(PushError { code, message }) => err(code, message),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_targets,
    push,
    list_saved,
    save_saved,
    get_saved,
    delete_saved,
    push_saved,
    set_match,
    clear_match
))]
pub struct Api;

pub fn create_playlists_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/playlist-targets", get(list_targets))
        .route("/api/v1/playlists", post(push))
        .route("/api/v1/saved-playlists", get(list_saved).post(save_saved))
        .route("/api/v1/saved-playlists/:id", get(get_saved).delete(delete_saved))
        .route("/api/v1/saved-playlists/:id/push", post(push_saved))
        .route("/api/v1/playlist-matches", post(set_match).delete(clear_match))
}

/// A track the search got wrong, and which of its candidates it really is.
#[derive(serde::Deserialize, ToSchema)]
#[schema(as = PlaylistMatch)]
pub struct MatchRequest {
    /// The target this resolution is for. `-1` is TIDAL.
    pub source_id: i64,
    pub artist: String,
    pub title: String,
    /// An `id` from a miss's `offered` list. Required to record a match;
    /// ignored by the `DELETE`, which forgets the mapping instead.
    #[serde(default)]
    pub remote_id: Option<String>,
    /// What the far end calls it, so the mapping can be shown back as something
    /// a person recognises rather than an opaque id.
    #[serde(default)]
    pub remote_label: Option<String>,
}

// Pin a track to one of the candidates the far end offered.
//
// **This is what makes the resolution cache self-improving.** A push reports
// every miss with the candidates that were offered — which is how SE SO NEON,
// Kim Jong Wan and Lee Moon Sae were found — and until now that list was a
// dead end you read and then went and fixed by hand somewhere else. Choosing
// from it writes a `manual` entry, which never expires and is never overwritten
// by a later automatic guess: somebody looked at the candidates and settled it,
// and a search that already got it wrong once must not be given another vote.
//
// Session-only, like the push it serves.
/// Pin a track to one of the candidates a push offered.
///
/// Session-only, like the push it serves.
///
/// A push reports every miss with the candidates the far end offered.
/// Until this existed that list was a dead end — plausible names you could
/// read and not act on. Choosing one records a **manual** resolution, which
/// never expires and is never overwritten by a later automatic guess: a
/// person read the candidates and settled it, and a search that already got
/// it wrong once does not get another vote.
///
/// This is also what makes the resolution cache self-improving. Every
/// successful match is remembered, so a re-push costs almost no searches at
/// all — which matters because the far end meters them.
#[utoipa::path(
    post,
    path = "/api/v1/playlist-matches",
    tag = "Playlists",
    request_body = MatchRequest,
    responses(
        (status = 200, description = "Recorded.", body = MatchSet),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn set_match(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<MatchRequest>,
) -> impl IntoResponse {
    let Some(remote_id) = body.remote_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return err(400, "A match needs the id of the track it resolves to");
    };
    if body.artist.trim().is_empty() || body.title.trim().is_empty() {
        return err(400, "A match needs the artist and title it is a match for");
    }
    match state
        .db
        .remember_playlist_match(
            user.user_id,
            body.source_id,
            &body.artist,
            &body.title,
            remote_id,
            body.remote_label.as_deref(),
            true,
        )
        .await
    {
        Ok(()) => Json(MatchSet { status: "matched" }).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// Forget a mapping — the pick was wrong, or the far end has changed. The next
// push searches for it again from scratch.
/// Forget a resolution.
///
/// Session-only. The pick was wrong, or the far end has changed what it
/// holds; the next push searches for the track again from scratch. Takes
/// the same body without `remote_id`.
#[utoipa::path(
    delete,
    path = "/api/v1/playlist-matches",
    tag = "Playlists",
    request_body = MatchRequest,
    responses(
        (status = 200, description = "Forgotten.", body = MatchSet),
        (status = 401, response = Unauthorized),
    )
)]
async fn clear_match(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<MatchRequest>,
) -> impl IntoResponse {
    match state
        .db
        .forget_playlist_match(user.user_id, body.source_id, &body.artist, &body.title)
        .await
    {
        Ok(()) => Json(MatchSet { status: "cleared" }).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}
