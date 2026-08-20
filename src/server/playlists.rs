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
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::UserSource;
use crate::playlists::{self, PlaylistTarget, PlaylistTrack};
use crate::sources::{MediaBrowserSource, NavidromeSource, PlexFilters, PlexSource};

/// How many tracks one push may carry.
///
/// Each one costs a search against the user's own server, run in turn, so this
/// is a wall-clock bound as much as a politeness one: a few hundred would sit
/// past any reverse proxy's gateway timeout and come back as a 502 with the
/// playlist half made.
const MAX_TRACKS: usize = 100;

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
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
    match source.kind.as_str() {
        "plex" => {
            let token = source.credential("token").ok_or("Plex token is missing")?;
            Ok(Box::new(PlexSource::with_filters(
                source.url.clone(),
                token,
                PlexFilters::default(),
            )))
        }
        "jellyfin" => {
            let key = source.credential("api_key").ok_or("Jellyfin API key is missing")?;
            Ok(Box::new(MediaBrowserSource::jellyfin(
                source.url.clone(),
                key,
                source.remote_account.clone().unwrap_or_default(),
            )))
        }
        "navidrome" => {
            let (Some(u), Some(p)) = (source.credential("username"), source.credential("password"))
            else {
                return Err("Navidrome credentials are missing".into());
            };
            Ok(Box::new(NavidromeSource::new(source.url.clone(), u, p)))
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

fn target_view(s: &UserSource, protocol: &str) -> serde_json::Value {
    serde_json::json!({
        "source_id": s.id,
        "kind": s.kind,
        "protocol": protocol,
        // The label is what tells two Plex servers apart; without one, the
        // protocol name is a better button than a blank.
        "label": if s.label.trim().is_empty() { protocol.to_string() } else { s.label.clone() },
        "account": s.remote_account,
    })
}

/// GET /api/v1/playlist-targets — the servers this user could push to.
///
/// Enabled sources only. A disabled source keeps its credentials on purpose, but
/// the user has said they don't want Tapedeck talking to it, and writing a
/// playlist is talking to it.
async fn list_targets(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let targets = targets_for(&state, user.user_id).await;
    (StatusCode::OK, Json(serde_json::json!({ "targets": targets }))).into_response()
}

/// The push targets for one user, as JSON.
///
/// Split out of the handler so the MCP surface offers exactly the same list.
/// An empty vec is a normal answer — plenty of instances have no writable
/// source at all — and both callers say so rather than showing a button or a
/// tool that cannot work.
pub async fn targets_for(state: &AppState, user_id: i64) -> Vec<serde_json::Value> {
    let mut targets: Vec<serde_json::Value> = match state.db.list_sources_for_user(user_id).await {
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
        targets.push(serde_json::json!({
            "source_id": TIDAL_TARGET_ID,
            "kind": "tidal",
            "protocol": "TIDAL",
            "label": "TIDAL",
            "account": null,
        }));
    }
    targets
}

#[derive(Deserialize)]
struct PushRequest {
    source_id: i64,
    name: String,
    tracks: Vec<PlaylistTrack>,
}

/// POST /api/v1/playlists — resolve every track against one server and create
/// the playlist from whatever was found.
async fn push(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<PushRequest>,
) -> impl IntoResponse {
    match push_tracks(&state, user.user_id, body.source_id, &body.name, &body.tracks).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
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
) -> Result<serde_json::Value, PushError> {
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
    let mut target_desc = serde_json::json!({
        "source_id": TIDAL_TARGET_ID,
        "kind": "tidal",
        "protocol": "TIDAL",
        "label": "TIDAL",
        "account": null,
    });
    let (protocol, target): (&str, Box<dyn playlists::PlaylistTarget>) = if source_id
        == TIDAL_TARGET_ID
    {
        let Some(app) = crate::tidal::TidalApp::from_env() else {
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
    // Sequential on purpose, for the same reason the Rediscovery queries are:
    // this is someone's Raspberry Pi, and firing a hundred searches at it at
    // once is how a push turns into a timeout. It also keeps the order the
    // caller asked for, which is the order the playlist ends up in.
    let mut ids: Vec<String> = Vec::with_capacity(tracks.len());
    let mut missing: Vec<&PlaylistTrack> = Vec::new();
    let mut search_error: Option<String> = None;

    for want in tracks {
        match target.search_tracks(want).await {
            Ok(candidates) => match playlists::pick(want, &candidates) {
                Some(id) => ids.push(id),
                None => missing.push(want),
            },
            Err(e) => {
                // One failed search is a miss; a server that has stopped
                // answering is a failed push. Remember the first reason so the
                // caller gets it rather than "nothing matched".
                warn!("Playlist push: search failed on {protocol}: {e}");
                search_error.get_or_insert(e);
                missing.push(want);
            }
        }
    }

    let missing_json: Vec<_> = missing
        .iter()
        .map(|t| serde_json::json!({ "title": t.title, "artist": t.artist, "album": t.album }))
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

    let created = target.create_playlist(&name, &ids).await;

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
            info!("📀 Pushed \"{}\" to {} — {}/{} tracks", name, protocol, ids.len(), tracks.len());
            Ok(serde_json::json!({
                "playlist_id": playlist_id,
                "name": name,
                "protocol": protocol,
                "target": target_desc,
                "requested": tracks.len(),
                "matched": ids.len(),
                "missing": missing_json,
            }))
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

/// GET /api/v1/saved-playlists
async fn list_saved(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.list_playlists(user.user_id).await {
        Ok(rows) => {
            (StatusCode::OK, Json(serde_json::json!({ "playlists": rows }))).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// GET /api/v1/saved-playlists/:id
async fn get_saved(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
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
            (StatusCode::OK, Json(serde_json::json!({ "playlist": meta, "tracks": tracks })))
                .into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize)]
struct SavePlaylistRequest {
    name: String,
    description: Option<String>,
    tracks: Vec<crate::db::SavedPlaylistTrack>,
}

/// POST /api/v1/saved-playlists
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
        Ok((id, _)) => (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

/// DELETE /api/v1/saved-playlists/:id
async fn delete_saved(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_playlist(user.user_id, id).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "deleted": true }))).into_response(),
        Ok(false) => err(404, "No such playlist"),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize)]
struct PushSavedRequest {
    source_id: i64,
}

/// POST /api/v1/saved-playlists/:id/push — send a stored playlist onward.
///
/// The review step the whole design turns on: a list an LLM wrote is saved
/// first and reaches a media server only when the person whose server it is
/// says so.
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
        })
        .collect();

    match push_tracks(&state, user.user_id, body.source_id, &meta.name, &wanted).await {
        Ok(value) => {
            let target = value
                .get("protocol")
                .and_then(|v| v.as_str())
                .unwrap_or("media server")
                .to_string();
            let _ = state.db.mark_playlist_pushed(user.user_id, id, &target).await;
            (StatusCode::OK, Json(value)).into_response()
        }
        Err(PushError { code, message }) => err(code, message),
    }
}

pub fn create_playlists_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/playlist-targets", get(list_targets))
        .route("/api/v1/playlists", post(push))
        .route("/api/v1/saved-playlists", get(list_saved).post(save_saved))
        .route("/api/v1/saved-playlists/:id", get(get_saved).delete(delete_saved))
        .route("/api/v1/saved-playlists/:id/push", post(push_saved))
}
