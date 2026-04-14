use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::Json,
};
use std::sync::Arc;

pub use crate::models::AuthUser;

/// Extract the `td_session` cookie value from the Cookie header.
pub fn extract_session_cookie(parts: &Parts) -> Option<String> {
    parts.headers.get("cookie")?
        .to_str().ok()?
        .split(';')
        .find_map(|c| c.trim().strip_prefix("td_session=").map(|v| v.to_string()))
}

/// Axum extractor: try session cookie first, then `Authorization: Token`, else 401.
#[axum::async_trait]
impl FromRequestParts<Arc<super::AppState>> for AuthUser {
    type Rejection = (StatusCode, Json<super::models::ErrorResponse>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<super::AppState>,
    ) -> Result<Self, Self::Rejection> {
        // ── 1. Try session cookie ──
        if let Some(session_token) = extract_session_cookie(parts) {
            if !session_token.is_empty() {
                match state.db.validate_session(&session_token).await {
                    Ok(Some(user)) => return Ok(user),
                    Ok(None) => { /* expired / invalid — fall through to token */ }
                    Err(e) => {
                        tracing::error!("Session validation error: {}", e);
                        // Fall through — don't block token auth because of a DB hiccup
                    }
                }
            }
        }

        // ── 2. Try Authorization: Token header ──
        let header = parts.headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let token = header
            .strip_prefix("Token ")
            .or_else(|| header.strip_prefix("token "))
            .unwrap_or("")
            .trim();

        if !token.is_empty() {
            return match state.db.validate_token(token).await {
                Ok(Some(user)) => Ok(user),
                Ok(None) => Err((
                    StatusCode::UNAUTHORIZED,
                    Json(super::models::ErrorResponse { code: 401, error: "Invalid token".into() }),
                )),
                Err(e) => {
                    tracing::error!("Token validation error: {}", e);
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(super::models::ErrorResponse { code: 500, error: "Internal server error".into() }),
                    ))
                }
            };
        }

        // ── 3. No credentials ──
        Err((
            StatusCode::UNAUTHORIZED,
            Json(super::models::ErrorResponse {
                code: 401,
                error: "Authentication required. Provide a session cookie or Authorization: Token header.".into(),
            }),
        ))
    }
}
