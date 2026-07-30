//! Per-user polling sources (`user_sources`).
//!
//! Every user configures their own — there is no admin-managed server-wide
//! source any more. For Plex that means a user supplies *their own* token, which
//! we resolve to a Plex account and pin the source to; other people's sessions on
//! the same server never become their listens.
//!
//! Stored credentials are never returned to the browser (`has_token` instead),
//! matching how API tokens and scrobble connections already behave. An omitted
//! token on `PUT` leaves the stored one alone, so a user can change the URL or
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
use crate::sources::{PlexFilters, PlexSource};
use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

/// Kinds we can actually build. Kept explicit so a typo'd path can't create a
/// row the engine will never poll.
const KINDS: [&str; 1] = ["plex"];

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

fn public_view(s: &UserSource) -> serde_json::Value {
    serde_json::json!({
        "kind": s.kind,
        "enabled": s.enabled,
        "url": s.url,
        "has_token": !s.credentials.is_empty(),
        "remote_account": s.remote_account,
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
    enabled: Option<bool>,
    url: Option<String>,
    /// Omitted or empty keeps the stored credential — it's never sent back, so
    /// the UI can't echo it even if it wanted to.
    token: Option<String>,
    poll_interval_secs: Option<i64>,
    libraries_allow: Option<Vec<String>>,
    libraries_block: Option<Vec<String>>,
    devices_allow: Option<Vec<String>>,
    devices_block: Option<Vec<String>>,
}

/// PUT /api/v1/sources/:kind
async fn put_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(kind): Path<String>,
    Json(body): Json<SourceUpdate>,
) -> impl IntoResponse {
    if !KINDS.contains(&kind.as_str()) {
        return err(400, format!("Unknown source kind: {kind}"));
    }

    let current = match state.db.get_source(user.user_id, &kind).await {
        Ok(c) => c,
        Err(e) => return err(500, e.to_string()),
    };

    let url = body.url
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .or_else(|| current.as_ref().map(|c| c.url.clone()))
        .unwrap_or_default();

    let token = match body.token.filter(|t| !t.trim().is_empty()) {
        Some(t) => t.trim().to_string(),
        None => current.as_ref().and_then(|c| c.credential("token")).unwrap_or_default(),
    };

    let enabled = body.enabled.or_else(|| current.as_ref().map(|c| c.enabled)).unwrap_or(false);
    let interval = body.poll_interval_secs.or_else(|| current.as_ref().and_then(|c| c.poll_interval_secs));

    if enabled && (url.is_empty() || token.is_empty()) {
        return err(400, "A server URL and token are required to enable a source");
    }

    // Resolving the account is what makes this source *this user's*. Do it on
    // save so the failure is visible here rather than as silence later.
    let mut remote_account = current.as_ref().and_then(|c| c.remote_account.clone());
    if !url.is_empty() && !token.is_empty() {
        let changed = current.as_ref().map(|c| c.url != url).unwrap_or(true)
            || current.as_ref().and_then(|c| c.credential("token")).as_deref() != Some(token.as_str())
            || remote_account.is_none();
        if changed {
            match PlexSource::resolve_account(&url, &token).await {
                Ok(name) => remote_account = Some(name),
                Err(e) => return err(400, format!(
                    "Could not confirm which Plex account that token belongs to: {e}. \
                     Tapedeck needs this to keep other people's sessions out of your history."
                )),
            }
        }
    }

    let credentials = if token.is_empty() {
        String::new()
    } else {
        serde_json::json!({ "token": token }).to_string()
    };

    // Any list the body omits keeps its stored value, so saving a URL doesn't
    // wipe someone's podcast exclusions.
    let keep = |incoming: Option<Vec<String>>, key: &str| -> Vec<String> {
        incoming.map(|v| v.into_iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_else(|| current.as_ref().map(|c| c.option_list(key)).unwrap_or_default())
    };
    let options = serde_json::json!({
        "libraries_allow": keep(body.libraries_allow, "libraries_allow"),
        "libraries_block": keep(body.libraries_block, "libraries_block"),
        "devices_allow": keep(body.devices_allow, "devices_allow"),
        "devices_block": keep(body.devices_block, "devices_block"),
    }).to_string();

    if let Err(e) = state.db.save_source(
        user.user_id, &kind, enabled, &url, &credentials, remote_account.as_deref(), interval,
        &options,
    ).await {
        return err(500, e.to_string());
    }
    info!("⚙️ {} source updated by {} (enabled: {})", kind, user.username, enabled);

    match state.db.get_source(user.user_id, &kind).await {
        Ok(Some(s)) => (StatusCode::OK, Json(public_view(&s))).into_response(),
        Ok(None) => err(500, "source vanished after write"),
        Err(e) => err(500, e.to_string()),
    }
}

/// DELETE /api/v1/sources/:kind
async fn delete_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(kind): Path<String>,
) -> impl IntoResponse {
    match state.db.delete_source(user.user_id, &kind).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => err(404, "No such source"),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Deserialize)]
struct SourceTest {
    url: Option<String>,
    token: Option<String>,
}

/// POST /api/v1/sources/:kind/test — verify a URL/token pair, report which
/// account it belongs to and which libraries it can see. Falls back to the
/// stored values for whichever field the body omits.
async fn test_source(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(kind): Path<String>,
    Json(body): Json<SourceTest>,
) -> impl IntoResponse {
    if !KINDS.contains(&kind.as_str()) {
        return err(400, format!("Unknown source kind: {kind}"));
    }

    let stored = state.db.get_source(user.user_id, &kind).await.ok().flatten();
    let url = body.url
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| stored.as_ref().map(|s| s.url.clone()))
        .unwrap_or_default();
    let token = body.token
        .filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().to_string())
        .or_else(|| stored.as_ref().and_then(|s| s.credential("token")))
        .unwrap_or_default();

    if url.is_empty() || token.is_empty() {
        return err(400, "Enter a server URL and token first");
    }

    // Report both halves separately: reaching the server but failing to identify
    // the account is a different problem from an unreachable server, and the fix
    // differs too. Flattened to String/String immediately — `Box<dyn Error>` is
    // not `Send`, so holding it across the next await makes this un-handler-able.
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
            "ok": false,
            "error": e.to_string(),
        }))).into_response(),
    }
}

pub fn create_sources_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/sources", get(list_sources))
        .route("/api/v1/sources/:kind", axum::routing::put(put_source).delete(delete_source))
        .route("/api/v1/sources/:kind/test", post(test_source))
}
