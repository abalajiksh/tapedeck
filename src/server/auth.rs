use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::Json,
};
use std::sync::Arc;

pub use crate::models::AuthUser;

type Rejection = (StatusCode, Json<super::models::ErrorResponse>);

fn unauthorized(msg: &str) -> Rejection {
    (StatusCode::UNAUTHORIZED, Json(super::models::ErrorResponse { code: 401, error: msg.into() }))
}

/// Extract the `td_session` cookie value from the Cookie header.
pub fn extract_session_cookie(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|c| c.trim().strip_prefix("td_session=").map(|v| v.to_string()))
}

/// Resolve a valid `td_session` cookie to a user, if present and valid.
async fn session_user(parts: &Parts, state: &Arc<super::AppState>) -> Option<AuthUser> {
    let token = extract_session_cookie(parts).filter(|t| !t.is_empty())?;
    match state.db.validate_session(&token).await {
        Ok(user) => user,
        Err(e) => {
            tracing::error!("Session validation error: {}", e);
            None
        }
    }
}

/// `AuthUser` authenticates via the **session cookie only** — API tokens are
/// deliberately rejected here. Tokens can only submit listens (see `IngestAuth`),
/// so every read/mutate/admin endpoint that uses `AuthUser` is unreachable with a
/// token. This is enforced by the type, not per-handler checks.
#[axum::async_trait]
impl FromRequestParts<Arc<super::AppState>> for AuthUser {
    type Rejection = Rejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<super::AppState>,
    ) -> Result<Self, Self::Rejection> {
        session_user(parts, state)
            .await
            .ok_or_else(|| unauthorized("Authentication required. Log in to access this endpoint."))
    }
}

/// `IngestAuth` accepts a session cookie **or** an `Authorization: Token` header.
/// Used by exactly one route — `POST /1/submit-listens` — so a token can
/// authenticate nowhere else.
pub struct IngestAuth(pub AuthUser);

#[axum::async_trait]
impl FromRequestParts<Arc<super::AppState>> for IngestAuth {
    type Rejection = Rejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<super::AppState>,
    ) -> Result<Self, Self::Rejection> {
        // 1. Session cookie (browser).
        if let Some(user) = session_user(parts, state).await {
            return Ok(IngestAuth(user));
        }

        // 2. Authorization: Token (external scrobble clients).
        let header = parts.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
        let token = header
            .strip_prefix("Token ")
            .or_else(|| header.strip_prefix("token "))
            .unwrap_or("")
            .trim();
        if !token.is_empty() {
            return match state.db.validate_token(token).await {
                Ok(Some(user)) => Ok(IngestAuth(user)),
                Ok(None) => Err(unauthorized("Invalid token")),
                Err(e) => {
                    tracing::error!("Token validation error: {}", e);
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(super::models::ErrorResponse {
                            code: 500,
                            error: "Internal server error".into(),
                        }),
                    ))
                }
            };
        }

        Err(unauthorized(
            "Authentication required. Provide a session cookie or Authorization: Token header.",
        ))
    }
}
