//! Per-user polling sources (`user_sources`).
//!
//! Every user configures their own — there is no admin-managed server-wide
//! source any more. For Plex that means a user supplies *their own* token, which
//! we resolve to a Plex account and pin the source to; other people's sessions on
//! the same server never become their listens.
//!
//! A user may configure **several servers of the same kind** — two Plex boxes, a
//! Navidrome and a Jellyfin — so sources are addressed by id rather than by kind,
//! and each carries a `label` to tell them apart. Every listen records which
//! source produced it (`scrobbles.source_ref`), which is what lets the artwork
//! proxy know which of two Plex servers to ask.
//!
//! Stored credentials are never returned to the browser (`has_token` instead),
//! matching how API tokens and scrobble connections already behave. An omitted
//! secret on update leaves the stored one alone, so a user can change the URL or
//! poll interval without re-pasting it.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::info;

use crate::db::UserSource;
use crate::sources::{JellyfinSource, NavidromeSource, PlexFilters, PlexSource};
use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

/// Kinds we can actually build. Kept explicit so a typo'd path can't create a
/// row the engine will never poll.
const KINDS: [&str; 3] = ["plex", "navidrome", "jellyfin"];

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

fn public_view(s: &UserSource) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "kind": s.kind,
        "label": s.label,
        "enabled": s.enabled,
        "url": s.url,
        "has_token": !s.credentials.is_empty(),
        "remote_account": s.remote_account,
        "username": s.credential("username"),
        "poll_interval_secs": s.poll_interval_secs,
        "libraries_allow": s.option_list("libraries_allow"),
        "libraries_block": s.option_list("libraries_block"),
        "devices_allow": s.option_list("devices_allow"),
        "devices_block": s.option_list("devices_block"),
        "last_polled_at": s.last_polled_at,
        "last_error": s.last_error,
        "updated_at": s.updated_at,
    })
}

/// GET /api/v1/sources — the caller's own sources.
async fn list_sources(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.db.list_sources_for_user(user.user_id).await {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows.iter().map(public_view).collect();
            (StatusCode::OK, Json(serde_json::json!({
                "sources": items,
                "kinds": KINDS,
            }))).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize)]
struct SourceUpdate {
    /// Only honoured on create; a source can't change kind afterwards.
    kind: Option<String>,
    label: Option<String>,
    enabled: Option<bool>,
    url: Option<String>,
    /// Omitted or empty keeps the stored credential — it's never sent back, so
    /// the UI can't echo it even if it wanted to.
    token: Option<String>,
    /// Navidrome logs in as a user, so it needs a username too. Subsonic hashes
    /// password+salt per request, which is why the password itself is stored
    /// (encrypted) rather than a token.
    username: Option<String>,
    password: Option<String>,
    /// Jellyfin mints a per-client API key, so it's per-user like a Plex token.
    api_key: Option<String>,
    poll_interval_secs: Option<i64>,
    libraries_allow: Option<Vec<String>>,
    libraries_block: Option<Vec<String>>,
    devices_allow: Option<Vec<String>>,
    devices_block: Option<Vec<String>>,
}

/// Everything a save needs, resolved from the body and whatever is stored.
struct Resolved {
    url: String,
    credentials: String,
    remote_account: Option<String>,
    options: String,
    complete: bool,
}

/// Work out the final credential set and confirm which remote account it
/// belongs to. Shared by create and update so both validate identically.
async fn resolve_save(
    kind: &str, body: &mut SourceUpdate, current: Option<&UserSource>, enabled: bool,
) -> Result<Resolved, axum::response::Response> {
    let url = body.url.take()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .or_else(|| current.map(|c| c.url.clone()))
        .unwrap_or_default();

    let keep = |incoming: Option<String>, key: &str| -> String {
        incoming.filter(|v| !v.trim().is_empty())
            .map(|v| v.trim().to_string())
            .or_else(|| current.and_then(|c| c.credential(key)))
            .unwrap_or_default()
    };
    let token = keep(body.token.take(), "token");
    let nd_user = keep(body.username.take(), "username");
    let nd_pass = keep(body.password.take(), "password");
    let jf_key = keep(body.api_key.take(), "api_key");

    let complete = match kind {
        "plex" => !url.is_empty() && !token.is_empty(),
        "navidrome" => !url.is_empty() && !nd_user.is_empty() && !nd_pass.is_empty(),
        "jellyfin" => !url.is_empty() && !jf_key.is_empty(),
        _ => false,
    };
    if enabled && !complete {
        return Err(err(400, match kind {
            "navidrome" => "A server URL, username and password are required to enable Navidrome",
            "jellyfin" => "A server URL and API key are required to enable Jellyfin",
            _ => "A server URL and token are required to enable Plex",
        }));
    }

    // Establishing *which account* the credentials belong to is what keeps one
    // user's listens out of another's. Done on save so a bad credential fails
    // here rather than as an unexplained empty history later.
    let mut remote_account = current.and_then(|c| c.remote_account.clone());
    let credentials = match kind {
        "plex" => {
            let changed = current.map(|c| c.url != url).unwrap_or(true)
                || current.and_then(|c| c.credential("token")).as_deref() != Some(token.as_str())
                || remote_account.is_none();
            if complete && changed {
                match PlexSource::resolve_account(&url, &token).await {
                    Ok(name) => remote_account = Some(name),
                    Err(e) => return Err(err(400, format!(
                        "Could not confirm which Plex account that token belongs to: {e}. \
                         Tapedeck needs this to keep other people's sessions out of your history."
                    ))),
                }
            }
            if token.is_empty() { String::new() }
            else { serde_json::json!({ "token": token }).to_string() }
        }
        "jellyfin" => {
            let changed = current.map(|c| c.url != url).unwrap_or(true)
                || current.and_then(|c| c.credential("api_key")).as_deref() != Some(jf_key.as_str())
                || remote_account.is_none();
            if complete && changed {
                match JellyfinSource::resolve_account(&url, &jf_key).await {
                    Ok(name) => remote_account = Some(name),
                    Err(e) => return Err(err(400, format!(
                        "Could not confirm which Jellyfin account that key belongs to: {e}"
                    ))),
                }
            }
            if jf_key.is_empty() { String::new() }
            else { serde_json::json!({ "api_key": jf_key }).to_string() }
        }
        _ => {
            if complete && enabled {
                let mut probe = NavidromeSource::new(url.clone(), nd_user.clone(), nd_pass.clone());
                if let Err(e) = probe.initialize().await {
                    return Err(err(400, format!("Could not sign in to Navidrome: {e}")));
                }
            }
            remote_account = (!nd_user.is_empty()).then(|| nd_user.clone());
            if nd_user.is_empty() { String::new() }
            else { serde_json::json!({ "username": nd_user, "password": nd_pass }).to_string() }
        }
    };

    // A list the body omits keeps its stored value, so saving a URL doesn't wipe
    // someone's podcast exclusions.
    let keep_list = |incoming: Option<Vec<String>>, key: &str| -> Vec<String> {
        incoming.map(|v| v.into_iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_else(|| current.map(|c| c.option_list(key)).unwrap_or_default())
    };
    let options = serde_json::json!({
        "libraries_allow": keep_list(body.libraries_allow.take(), "libraries_allow"),
        "libraries_block": keep_list(body.libraries_block.take(), "libraries_block"),
        "devices_allow": keep_list(body.devices_allow.take(), "devices_allow"),
        "devices_block": keep_list(body.devices_block.take(), "devices_block"),
    }).to_string();

    Ok(Resolved { url, credentials, remote_account, options, complete })
}

/// POST /api/v1/sources — add another server.
async fn create_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(mut body): Json<SourceUpdate>,
) -> impl IntoResponse {
    let kind = body.kind.clone().unwrap_or_default();
    if !KINDS.contains(&kind.as_str()) {
        return err(400, format!("Unknown source kind: {kind}"));
    }
    let enabled = body.enabled.unwrap_or(false);
    let label = body.label.clone().unwrap_or_default().trim().to_string();
    let interval = body.poll_interval_secs;

    let resolved = match resolve_save(&kind, &mut body, None, enabled).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match state.db.create_source(
        user.user_id, &kind, &label, enabled && resolved.complete, &resolved.url,
        &resolved.credentials, resolved.remote_account.as_deref(), interval, &resolved.options,
    ).await {
        Ok(id) => {
            info!("⚙️ {} source added by {}", kind, user.username);
            match state.db.get_source(user.user_id, id).await {
                Ok(Some(s)) => (StatusCode::CREATED, Json(public_view(&s))).into_response(),
                _ => err(500, "source vanished after write"),
            }
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// PUT /api/v1/sources/:id
async fn update_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(mut body): Json<SourceUpdate>,
) -> impl IntoResponse {
    let current = match state.db.get_source(user.user_id, id).await {
        Ok(Some(c)) => c,
        Ok(None) => return err(404, "No such source"),
        Err(e) => return err(500, e.to_string()),
    };

    let enabled = body.enabled.unwrap_or(current.enabled);
    let label = body.label.clone().unwrap_or_else(|| current.label.clone()).trim().to_string();
    let interval = body.poll_interval_secs.or(current.poll_interval_secs);

    let resolved = match resolve_save(&current.kind, &mut body, Some(&current), enabled).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    match state.db.update_source(
        user.user_id, id, &label, enabled, &resolved.url,
        &resolved.credentials, resolved.remote_account.as_deref(), interval, &resolved.options,
    ).await {
        Ok(true) => {
            info!("⚙️ {} source updated by {} (enabled: {})", current.kind, user.username, enabled);
            match state.db.get_source(user.user_id, id).await {
                Ok(Some(s)) => (StatusCode::OK, Json(public_view(&s))).into_response(),
                _ => err(500, "source vanished after write"),
            }
        }
        Ok(false) => err(404, "No such source"),
        Err(e) => err(500, e.to_string()),
    }
}

/// DELETE /api/v1/sources/:id
async fn delete_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_source(user.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(404, "No such source"),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize)]
struct SourceTest {
    kind: Option<String>,
    /// Optional: test an existing source's stored credentials by id.
    id: Option<i64>,
    url: Option<String>,
    token: Option<String>,
    username: Option<String>,
    password: Option<String>,
    api_key: Option<String>,
}

/// POST /api/v1/sources/test — verify credentials before or after saving.
///
/// Takes `kind` in the body rather than the path so it can be called while
/// adding a server that doesn't exist yet; pass `id` instead to re-test one
/// that does, reusing its stored secret.
async fn test_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<SourceTest>,
) -> impl IntoResponse {
    let stored = match body.id {
        Some(id) => match state.db.get_source(user.user_id, id).await {
            Ok(Some(s)) => Some(s),
            Ok(None) => return err(404, "No such source"),
            Err(e) => return err(500, e.to_string()),
        },
        None => None,
    };

    let kind = body.kind.clone()
        .or_else(|| stored.as_ref().map(|s| s.kind.clone()))
        .unwrap_or_default();
    if !KINDS.contains(&kind.as_str()) {
        return err(400, format!("Unknown source kind: {kind}"));
    }

    let url = body.url.clone()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| stored.as_ref().map(|s| s.url.clone()))
        .unwrap_or_default();
    let cred = |incoming: Option<String>, key: &str| -> String {
        incoming.filter(|v| !v.trim().is_empty())
            .map(|v| v.trim().to_string())
            .or_else(|| stored.as_ref().and_then(|s| s.credential(key)))
            .unwrap_or_default()
    };

    match kind.as_str() {
        "jellyfin" => {
            let key = cred(body.api_key.clone(), "api_key");
            if url.is_empty() || key.is_empty() {
                return err(400, "Enter a server URL and API key first");
            }
            match JellyfinSource::resolve_account(&url, &key).await {
                Ok(name) => (StatusCode::OK, Json(serde_json::json!({ "ok": true, "account": name }))).into_response(),
                Err(e) => (StatusCode::OK, Json(serde_json::json!({ "ok": false, "error": e.to_string() }))).into_response(),
            }
        }
        "navidrome" => {
            let (u, p) = (cred(body.username.clone(), "username"), cred(body.password.clone(), "password"));
            if url.is_empty() || u.is_empty() || p.is_empty() {
                return err(400, "Enter a server URL, username and password first");
            }
            let mut probe = NavidromeSource::new(url, u.clone(), p);
            match probe.initialize().await {
                Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "ok": true, "account": u }))).into_response(),
                Err(e) => (StatusCode::OK, Json(serde_json::json!({ "ok": false, "error": e.to_string() }))).into_response(),
            }
        }
        _ => {
            let token = cred(body.token.clone(), "token");
            if url.is_empty() || token.is_empty() {
                return err(400, "Enter a server URL and token first");
            }
            // Reaching the server but failing to identify the account is a
            // different problem from an unreachable server, so report both.
            // Flattened immediately — `Box<dyn Error>` isn't `Send`, so holding
            // it across the next await would make this un-handler-able.
            let (account, account_error) = match PlexSource::resolve_account(&url, &token).await {
                Ok(name) => (Some(name), None),
                Err(e) => (None, Some(e.to_string())),
            };
            let mut probe = PlexSource::with_filters(url, token, PlexFilters::default());
            match probe.initialize().await {
                Ok(()) => (StatusCode::OK, Json(serde_json::json!({
                    "ok": true,
                    "libraries": probe.library_names(),
                    "account": account,
                    "account_error": account_error,
                }))).into_response(),
                Err(e) => (StatusCode::OK, Json(serde_json::json!({
                    "ok": false, "error": e.to_string(),
                }))).into_response(),
            }
        }
    }
}

pub fn create_sources_router() -> Router<Arc<AppState>> {
    Router::new()
        // `/test` is registered before `/:id` so it isn't swallowed by the
        // parameterised route.
        .route("/api/v1/sources/test", post(test_source))
        .route("/api/v1/sources", get(list_sources).post(create_source))
        .route("/api/v1/sources/:id", axum::routing::put(update_source).delete(delete_source))
}
