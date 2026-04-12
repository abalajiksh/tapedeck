use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::Json,
};
use std::sync::Arc;

// Re-export from models so downstream code can use server::auth::AuthUser
pub use crate::models::AuthUser;

/// Axum extractor that validates `Authorization: Token td_xxx` and resolves to an `AuthUser`.
#[axum::async_trait]
impl FromRequestParts<Arc<super::AppState>> for AuthUser {
    type Rejection = (StatusCode, Json<super::models::ErrorResponse>);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<super::AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let token = header
            .strip_prefix("Token ")
            .or_else(|| header.strip_prefix("token "))
            .unwrap_or("")
            .trim();

        if token.is_empty() {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(super::models::ErrorResponse {
                    code: 401,
                    error: "Missing or invalid Authorization header. Expected: Token <your_token>".into(),
                }),
            ));
        }

        match state.db.validate_token(token).await {
            Ok(Some(user)) => Ok(user),
            Ok(None) => Err((
                StatusCode::UNAUTHORIZED,
                Json(super::models::ErrorResponse {
                    code: 401,
                    error: "Invalid token".into(),
                }),
            )),
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
        }
    }
}
