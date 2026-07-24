//! Per-user scrobble connections: each user connects their own Last.fm and
//! ListenBrainz accounts. Credentials live server-side; responses only ever
//! reveal the connected account name, never the token/session key.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let code_status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code_status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

/// GET /api/v1/connections — which services this user has connected.
async fn list_connections(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    match state.db.list_connections(user.user_id).await {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows.into_iter().map(|(service, username, at)| {
                serde_json::json!({ "service": service, "username": username, "connected_at": at })
            }).collect();
            // Also surface whether Last.fm connect is even possible (app creds present).
            (StatusCode::OK, Json(serde_json::json!({
                "connections": items,
                "lastfm_available": !state.lastfm_api_key.is_empty() && !state.lastfm_secret.is_empty(),
            }))).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

async fn disconnect(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(service): Path<String>,
) -> impl IntoResponse {
    if service != "lastfm" && service != "listenbrainz" {
        return err(400, "Unknown service");
    }
    match state.db.delete_connection(user.user_id, &service).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// ── ListenBrainz: paste a user token ──

#[derive(Deserialize)]
struct LbConnect { token: String }

async fn connect_listenbrainz(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<LbConnect>,
) -> impl IntoResponse {
    let token = body.token.trim().to_string();
    if token.is_empty() {
        return err(400, "Token required");
    }
    // Validate against ListenBrainz and capture the account name.
    let url = format!("{}/1/validate-token", state.listenbrainz_base_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Token {token}"))
        .send().await;
    let username = match resp {
        Ok(r) if r.status().is_success() => {
            let v: serde_json::Value = r.json().await.unwrap_or_default();
            if v["valid"].as_bool() != Some(true) {
                return err(400, "ListenBrainz rejected that token");
            }
            v["user_name"].as_str().map(str::to_string)
        }
        Ok(r) => return err(400, format!("ListenBrainz returned {}", r.status())),
        Err(e) => return err(502, format!("Could not reach ListenBrainz: {e}")),
    };

    let creds = serde_json::json!({ "token": token }).to_string();
    match state.db.set_connection(user.user_id, "listenbrainz", &creds, username.as_deref()).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "connected": true, "username": username }))).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// ── Last.fm: token → approve → session (button OAuth) ──

/// Last.fm signs requests with md5 of the sorted params + shared secret.
fn lastfm_sign(params: &BTreeMap<&str, &str>, secret: &str) -> String {
    let mut s = String::new();
    for (k, v) in params {
        s.push_str(k);
        s.push_str(v);
    }
    s.push_str(secret);
    format!("{:x}", md5::compute(s))
}

fn lastfm_ready(state: &AppState) -> bool {
    !state.lastfm_api_key.is_empty() && !state.lastfm_secret.is_empty()
}

/// POST /api/v1/connections/lastfm/start — get a request token + the approval URL.
async fn lastfm_start(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> impl IntoResponse {
    if !lastfm_ready(&state) {
        return err(400, "Last.fm is not configured on this server (set LASTFM_API_KEY and LASTFM_SECRET)");
    }
    let mut params = BTreeMap::new();
    params.insert("api_key", state.lastfm_api_key.as_str());
    params.insert("method", "auth.getToken");
    let sig = lastfm_sign(&params, &state.lastfm_secret);

    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=auth.getToken&api_key={}&api_sig={}&format=json",
        state.lastfm_api_key, sig,
    );
    let resp = reqwest::Client::new().get(&url).send().await;
    let token = match resp {
        Ok(r) => {
            let v: serde_json::Value = r.json().await.unwrap_or_default();
            match v["token"].as_str() {
                Some(t) => t.to_string(),
                None => return err(502, format!("Last.fm getToken failed: {}", v["message"].as_str().unwrap_or("unknown"))),
            }
        }
        Err(e) => return err(502, format!("Could not reach Last.fm: {e}")),
    };

    let auth_url = format!("https://www.last.fm/api/auth/?api_key={}&token={}", state.lastfm_api_key, token);
    (StatusCode::OK, Json(serde_json::json!({ "token": token, "auth_url": auth_url }))).into_response()
}

#[derive(Deserialize)]
struct LastfmComplete { token: String }

/// POST /api/v1/connections/lastfm/complete — exchange the approved token for a
/// session key. Fails if the user hasn't approved on Last.fm's site yet.
async fn lastfm_complete(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<LastfmComplete>,
) -> impl IntoResponse {
    if !lastfm_ready(&state) {
        return err(400, "Last.fm is not configured on this server");
    }
    let token = body.token.trim();
    if token.is_empty() {
        return err(400, "Missing token");
    }

    let mut params = BTreeMap::new();
    params.insert("api_key", state.lastfm_api_key.as_str());
    params.insert("method", "auth.getSession");
    params.insert("token", token);
    let sig = lastfm_sign(&params, &state.lastfm_secret);

    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=auth.getSession&api_key={}&token={}&api_sig={}&format=json",
        state.lastfm_api_key, token, sig,
    );
    let resp = reqwest::Client::new().get(&url).send().await;
    let (session_key, username) = match resp {
        Ok(r) => {
            let v: serde_json::Value = r.json().await.unwrap_or_default();
            match v["session"]["key"].as_str() {
                Some(k) => (k.to_string(), v["session"]["name"].as_str().map(str::to_string)),
                None => {
                    // error 14/15 = token not authorized yet
                    return err(400, format!("Not yet approved on Last.fm — click Connect again after approving ({})",
                        v["message"].as_str().unwrap_or("no session")));
                }
            }
        }
        Err(e) => return err(502, format!("Could not reach Last.fm: {e}")),
    };

    let creds = serde_json::json!({ "session_key": session_key }).to_string();
    match state.db.set_connection(user.user_id, "lastfm", &creds, username.as_deref()).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "connected": true, "username": username }))).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lastfm_sign_is_deterministic_and_secret_sensitive() {
        let mut p = BTreeMap::new();
        p.insert("api_key", "KEY");
        p.insert("method", "auth.getToken");
        // md5("api_keyKEYmethodauth.getTokenSECRET")
        let sig = lastfm_sign(&p, "SECRET");
        assert_eq!(sig.len(), 32);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(sig, lastfm_sign(&p, "SECRET"), "same input → same signature");
        assert_ne!(sig, lastfm_sign(&p, "OTHER"), "signature depends on the secret");
    }

    #[test]
    fn lastfm_sign_sorts_params() {
        // BTreeMap sorts keys, so insertion order doesn't change the signature.
        let mut a = BTreeMap::new();
        a.insert("b", "2"); a.insert("a", "1");
        let mut b = BTreeMap::new();
        b.insert("a", "1"); b.insert("b", "2");
        assert_eq!(lastfm_sign(&a, "s"), lastfm_sign(&b, "s"));
    }
}

pub fn create_connections_router() -> Router<Arc<AppState>> {
    use axum::routing::{delete, post, put};
    Router::new()
        .route("/api/v1/connections", get(list_connections))
        .route("/api/v1/connections/:service", delete(disconnect))
        .route("/api/v1/connections/listenbrainz", put(connect_listenbrainz))
        .route("/api/v1/connections/lastfm/start", post(lastfm_start))
        .route("/api/v1/connections/lastfm/complete", post(lastfm_complete))
}
