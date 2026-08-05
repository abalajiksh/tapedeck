//! Admin endpoints for the Every Noise genre map.
//!
//! Admin-only throughout. This is instance-wide reference data rather than
//! anything belonging to a user, and it makes outbound requests to a third
//! party — neither is a decision an ordinary account should be making.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post, put},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::error;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::everynoise::scrape::{self, Tier};

/// Where the admin's contact address for the scraper's `User-Agent` lives.
pub const CONTACT_KEY: &str = "everynoise_contact";

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

/// The configured contact address, database first and environment second —
/// the same order the Discogs token uses.
async fn contact(state: &AppState) -> Option<String> {
    if let Ok(Some(c)) = state.db.get_app_setting(CONTACT_KEY).await {
        let c = c.trim().to_string();
        if !c.is_empty() {
            return Some(c);
        }
    }
    std::env::var("TAPEDECK_EVERYNOISE_CONTACT").ok().filter(|c| !c.trim().is_empty())
}

/// Enough of an address to be a way of reaching someone.
///
/// Not a validity check — that is not decidable — just a guard against a
/// placeholder being sent to a stranger's server as if it meant something.
fn looks_like_an_address(s: &str) -> bool {
    let s = s.trim();
    match s.split_once('@') {
        Some((user, host)) => {
            !user.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
        }
        None => false,
    }
}

/// GET /admin/everynoise — what is held, whether a walk is running.
pub async fn status(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let summary = match state.everynoise.summary().await {
        Ok(s) => s,
        Err(e) => {
            error!("Every Noise summary failed: {}", e);
            return err(500, "Could not read the genre map database");
        }
    };
    let contact = contact(&state).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "summary": summary,
            "progress": state.everynoise_jobs.get().await,
            // Echoed back so the field can be pre-filled; it is the admin's own
            // address, not a credential.
            "contact": contact,
            "source": "https://everynoise.com/",
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct ContactBody {
    pub contact: String,
}

/// PUT /admin/everynoise/contact
pub async fn set_contact(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<ContactBody>,
) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let value = body.contact.trim().to_string();
    if !value.is_empty() && !looks_like_an_address(&value) {
        return err(400, "That doesn't look like an email address");
    }
    match state.db.set_app_setting(CONTACT_KEY, &value).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "status": "saved" }))).into_response(),
        Err(e) => {
            error!("Could not save the Every Noise contact: {}", e);
            err(500, "Could not save that")
        }
    }
}

#[derive(Deserialize)]
pub struct StartBody {
    /// `map` (one request) or `pages` (~6,300, about two hours).
    pub tier: String,
}

/// POST /admin/everynoise/scrape
pub async fn start(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<StartBody>,
) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }

    let tier = match body.tier.as_str() {
        "map" => Tier::Map,
        "pages" => Tier::Pages,
        other => return err(400, format!("Unknown tier '{}'", other)),
    };

    // One walk at a time. Two would contend for the same rows and double the
    // traffic aimed at a site that cannot benefit from either.
    if state.everynoise_jobs.is_running().await {
        return err(409, "A scrape is already running");
    }

    // Refuse without a contact address rather than substituting a default.
    // Sending an anonymous — or invented — identity to a frozen personal site
    // 6,300 times is the thing that gets a project blocked.
    let Some(contact) = contact(&state).await else {
        return err(
            400,
            "Set a contact email first — it identifies this instance to everynoise.com",
        );
    };

    if tier == Tier::Pages {
        match state.everynoise.summary().await {
            Ok(s) if s.genres == 0 => {
                return err(
                    409,
                    "Fetch the genre map first — the per-genre walk works from the list it produces",
                );
            }
            Err(e) => {
                error!("Every Noise summary failed: {}", e);
                return err(500, "Could not read the genre map database");
            }
            _ => {}
        }
    }

    let store = state.everynoise.clone();
    let registry = state.everynoise_jobs.clone();
    tokio::spawn(async move {
        let result = match tier {
            Tier::Map => scrape::run_map(store, registry, contact).await,
            Tier::Pages => scrape::run_pages(store, registry, contact).await,
        };
        if let Err(e) = result {
            error!("Every Noise scrape failed: {}", e);
        }
    });

    (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "started", "tier": tier })))
        .into_response()
}

/// POST /admin/everynoise/cancel
pub async fn cancel(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    state.everynoise_jobs.cancel().await;
    (StatusCode::OK, Json(serde_json::json!({ "status": "stopping" }))).into_response()
}

/// DELETE /admin/everynoise — drop the local copy.
pub async fn clear(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    if state.everynoise_jobs.is_running().await {
        return err(409, "Stop the running scrape first");
    }
    match state.everynoise.clear().await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "status": "cleared" }))).into_response(),
        Err(e) => {
            error!("Could not clear the genre map: {}", e);
            err(500, "Could not clear it")
        }
    }
}

pub fn create_everynoise_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/everynoise", get(status).delete(clear))
        .route("/admin/everynoise/contact", put(set_contact))
        .route("/admin/everynoise/scrape", post(start))
        .route("/admin/everynoise/cancel", post(cancel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_is_not_accepted_as_a_contact() {
        assert!(looks_like_an_address("someone@example.com"));
        assert!(looks_like_an_address("a.b+c@sub.domain.co.uk"));

        // These would all be sent to a stranger's server as if they meant
        // something.
        assert!(!looks_like_an_address(""));
        assert!(!looks_like_an_address("nobody"));
        assert!(!looks_like_an_address("@example.com"));
        assert!(!looks_like_an_address("someone@localhost"));
        assert!(!looks_like_an_address("someone@.com"));
    }
}
