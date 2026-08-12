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
    match state.db.list_sources_for_user(user.user_id).await {
        Ok(rows) => {
            let targets: Vec<_> = rows
                .iter()
                .filter(|s| s.enabled)
                .filter_map(|s| protocol_for(&s.kind).map(|p| target_view(s, p)))
                .collect();
            (StatusCode::OK, Json(serde_json::json!({ "targets": targets }))).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
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
    let name = body.name.trim();
    if name.is_empty() {
        return err(400, "A playlist needs a name");
    }
    // Long enough for "Tapedeck · Forgotten Favourites · March 2026", short
    // enough that no server truncates it into something unrecognisable.
    let name: String = name.chars().take(120).collect();

    if body.tracks.is_empty() {
        return err(400, "A playlist needs at least one track");
    }
    if body.tracks.len() > MAX_TRACKS {
        return err(400, format!("That's more than {MAX_TRACKS} tracks"));
    }

    // Scoped before anything else: sources are per user, and an id from someone
    // else's row would push through their credentials.
    let source = match state.db.get_source(user.user_id, body.source_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return err(404, "No such source"),
        Err(e) => return err(500, e.to_string()),
    };
    if !source.enabled {
        return err(400, "That source is switched off");
    }
    let Some(protocol) = protocol_for(&source.kind) else {
        return err(400, format!("A {} source cannot receive a playlist", source.kind));
    };
    let target = match build_target(&source) {
        Ok(t) => t,
        Err(msg) => return err(400, msg),
    };

    // Sequential on purpose, for the same reason the Rediscovery queries are:
    // this is someone's Raspberry Pi, and firing a hundred searches at it at
    // once is how a push turns into a timeout. It also keeps the order the
    // caller asked for, which is the order the playlist ends up in.
    let mut ids: Vec<String> = Vec::with_capacity(body.tracks.len());
    let mut missing: Vec<&PlaylistTrack> = Vec::new();
    let mut search_error: Option<String> = None;

    for want in &body.tracks {
        match target.search_tracks(want).await {
            Ok(candidates) => match playlists::pick(want, &candidates) {
                Some(id) => ids.push(id),
                None => missing.push(want),
            },
            Err(e) => {
                // One failed search is a miss; a server that has stopped
                // answering is a failed push. Remember the first reason so the
                // caller gets it rather than "nothing matched".
                warn!("Playlist push: search failed on {}: {e}", source.url);
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
        return match search_error {
            Some(e) => err(502, format!("{protocol} could not be reached: {e}")),
            None => err(
                404,
                format!("None of those {} tracks are on this {protocol}", body.tracks.len()),
            ),
        };
    }

    match target.create_playlist(&name, &ids).await {
        Ok(playlist_id) => {
            info!(
                "📀 Pushed \"{}\" to {} — {}/{} tracks",
                name,
                protocol,
                ids.len(),
                body.tracks.len()
            );
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "playlist_id": playlist_id,
                    "name": name,
                    "protocol": protocol,
                    "target": target_view(&source, protocol),
                    "requested": body.tracks.len(),
                    "matched": ids.len(),
                    "missing": missing_json,
                })),
            )
                .into_response()
        }
        Err(e) => err(502, format!("{protocol} refused the playlist: {e}")),
    }
}

pub fn create_playlists_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/playlist-targets", get(list_targets))
        .route("/api/v1/playlists", post(push))
}
