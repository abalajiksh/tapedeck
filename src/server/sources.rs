use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::info;

use crate::db::PlexSettings;
use crate::sources::{PlexFilters, PlexSource};
use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

fn forbidden() -> axum::response::Response {
    (StatusCode::FORBIDDEN, Json(serde_json::json!(
        ErrorResponse { code: 403, error: "Admin access required".into() }
    ))).into_response()
}

fn server_error(e: impl std::fmt::Display) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
        ErrorResponse { code: 500, error: e.to_string() }
    ))).into_response()
}

/// The token is never sent back — same rule as user connections and API tokens.
/// The UI only needs to know whether one is stored.
fn public_view(s: &PlexSettings) -> serde_json::Value {
    serde_json::json!({
        "enabled": s.enabled,
        "url": s.url,
        "has_token": !s.token.is_empty(),
        "users_allow": PlexSettings::list(&s.users_allow),
        "users_block": PlexSettings::list(&s.users_block),
        "devices_allow": PlexSettings::list(&s.devices_allow),
        "devices_block": PlexSettings::list(&s.devices_block),
        "libraries_allow": PlexSettings::list(&s.libraries_allow),
        "libraries_block": PlexSettings::list(&s.libraries_block),
        "updated_at": s.updated_at,
    })
}

/// GET /api/v1/sources/plex — admin only.
async fn get_plex(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin { return forbidden(); }
    match state.db.get_plex_settings().await {
        Ok(Some(s)) => (StatusCode::OK, Json(public_view(&s))).into_response(),
        Ok(None) => (StatusCode::OK, Json(public_view(&PlexSettings::default()))).into_response(),
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
struct PlexUpdate {
    enabled: Option<bool>,
    url: Option<String>,
    /// Omitted or empty leaves the stored token untouched, so the UI can save
    /// other fields without having to re-enter it (it never receives it back).
    token: Option<String>,
    users_allow: Option<Vec<String>>,
    users_block: Option<Vec<String>>,
    devices_allow: Option<Vec<String>>,
    devices_block: Option<Vec<String>>,
    libraries_allow: Option<Vec<String>>,
    libraries_block: Option<Vec<String>>,
}

fn join(v: Option<Vec<String>>, current: &str) -> String {
    match v {
        Some(list) => list.iter().map(|s| s.trim()).filter(|s| !s.is_empty())
            .collect::<Vec<_>>().join(","),
        None => current.to_string(),
    }
}

/// PUT /api/v1/sources/plex — admin only. Applies on the engine's next tick.
async fn put_plex(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<PlexUpdate>,
) -> impl IntoResponse {
    if !user.is_admin { return forbidden(); }

    let current = match state.db.get_plex_settings().await {
        Ok(s) => s.unwrap_or_default(),
        Err(e) => return server_error(e),
    };

    let next = PlexSettings {
        enabled: body.enabled.unwrap_or(current.enabled),
        url: body.url.map(|u| u.trim().trim_end_matches('/').to_string()).unwrap_or(current.url),
        token: match body.token {
            Some(t) if !t.trim().is_empty() => t.trim().to_string(),
            _ => current.token,
        },
        users_allow: join(body.users_allow, &current.users_allow),
        users_block: join(body.users_block, &current.users_block),
        devices_allow: join(body.devices_allow, &current.devices_allow),
        devices_block: join(body.devices_block, &current.devices_block),
        libraries_allow: join(body.libraries_allow, &current.libraries_allow),
        libraries_block: join(body.libraries_block, &current.libraries_block),
        updated_at: 0, // set by the DB layer
    };

    if next.enabled && (next.url.is_empty() || next.token.is_empty()) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!(
            ErrorResponse { code: 400, error: "A server URL and token are required to enable Plex".into() }
        ))).into_response();
    }

    if let Err(e) = state.db.save_plex_settings(&next).await {
        return server_error(e);
    }
    info!("⚙️ Plex settings updated by {} (enabled: {})", user.username, next.enabled);

    match state.db.get_plex_settings().await {
        Ok(Some(s)) => (StatusCode::OK, Json(public_view(&s))).into_response(),
        Ok(None) => server_error("settings vanished after write"),
        Err(e) => server_error(e),
    }
}

#[derive(Deserialize)]
struct PlexTest {
    url: Option<String>,
    token: Option<String>,
}

/// POST /api/v1/sources/plex/test — admin only. Verifies a URL/token pair by
/// listing libraries, so the admin finds out here rather than from the logs.
/// Falls back to the stored values for whichever field the body omits.
async fn test_plex(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<PlexTest>,
) -> impl IntoResponse {
    if !user.is_admin { return forbidden(); }

    let stored = state.db.get_plex_settings().await.ok().flatten().unwrap_or_default();
    let url = body.url.map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty()).unwrap_or(stored.url);
    let token = body.token.filter(|t| !t.trim().is_empty())
        .map(|t| t.trim().to_string()).unwrap_or(stored.token);

    if url.is_empty() || token.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!(
            ErrorResponse { code: 400, error: "Enter a server URL and token first".into() }
        ))).into_response();
    }

    let mut probe = PlexSource::with_filters(url, token, PlexFilters::default());
    match probe.initialize().await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({
            "ok": true,
            "libraries": probe.library_names(),
        }))).into_response(),
        Err(e) => (StatusCode::OK, Json(serde_json::json!({
            "ok": false,
            "error": e.to_string(),
        }))).into_response(),
    }
}

pub fn create_sources_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/sources/plex", get(get_plex).put(put_plex))
        .route("/api/v1/sources/plex/test", post(test_plex))
}
