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

/// `ReadAuth` accepts a session cookie **or** an `Authorization: Token` header
/// whose token carries the `read` scope.
///
/// It exists so a scrobble client can *ask* rather than guess. A plugin that
/// wants to attribute a listen to a signal chain has to name one, and without
/// this it could only take unvalidatable free text from the user — there was no
/// way to fetch the list of chains that actually exist.
///
/// Three rules keep it narrow, and none of them should be relaxed casually:
///
/// * **`read` is an explicit grant**, never implied by `submit`. A client that
///   only posts listens does not silently gain the ability to enumerate the
///   user's gear because `tokens.scopes` happened to exist.
/// * **Reads only, and only the caller's own data.** Every handler using this
///   must still scope its queries by `user.user_id`, exactly as it did under
///   `AuthUser` — this widens *who may ask*, not *what may be seen*.
/// * **Nothing that writes, and nothing under `/admin/`.** Those stay on
///   `AuthUser`, which is session-only, so a leaked token cannot change
///   anything or reach user management.
pub struct ReadAuth {
    pub user: AuthUser,
}

/// The shared body of `ReadAuth` and `WriteAuth`: session cookie first, then a
/// bearer-style `Authorization: Token` that must carry `scope`.
///
/// Factored out rather than copied because the *rejection* behaviour is the
/// part worth having in one place — in particular that a valid token missing
/// the grant is a 403 and never a 401.
async fn scoped_token_user(
    parts: &Parts,
    state: &Arc<super::AppState>,
    scope: &'static str,
    granted: fn(&crate::db::TokenAuth) -> bool,
) -> Result<AuthUser, Rejection> {
    if let Some(user) = session_user(parts, state).await {
        return Ok(user);
    }

    let header = parts.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = header
        .strip_prefix("Token ")
        .or_else(|| header.strip_prefix("token "))
        .unwrap_or("")
        .trim();
    if token.is_empty() {
        return Err(unauthorized(
            "Authentication required. Provide a session cookie or Authorization: Token header.",
        ));
    }

    match state.db.validate_token(token).await {
        Ok(Some(t)) if granted(&t) => Ok(t.user),
        // A valid token without the grant is 403, not 401: the caller
        // authenticated fine and re-authenticating will not help. A 401
        // would send a client into a token-refresh loop it cannot win.
        Ok(Some(_)) => Err((
            StatusCode::FORBIDDEN,
            Json(super::models::ErrorResponse {
                code: 403,
                error: format!("This token does not have the '{}' scope", scope),
            }),
        )),
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
    }
}

#[axum::async_trait]
impl FromRequestParts<Arc<super::AppState>> for ReadAuth {
    type Rejection = Rejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<super::AppState>,
    ) -> Result<Self, Self::Rejection> {
        scoped_token_user(parts, state, "read", |t| t.can_read())
            .await
            .map(|user| ReadAuth { user })
    }
}

/// `WriteAuth` accepts a session cookie **or** an `Authorization: Token` header
/// whose token carries the `write` scope.
///
/// It exists because `/api/v1/` was session-only, which meant a paired client
/// could reach `/1/*` and five GETs and nothing else — no loves, no notes, no
/// manual listens, no chain edits. Documenting the API without this would have
/// described a promise the server does not keep.
///
/// The rules are `ReadAuth`'s, with one addition that must not be relaxed:
///
/// * **`write` is an explicit grant**, never implied by `submit` or by `read`.
/// * **Only the caller's own data.** Every handler must still scope its queries
///   and its ownership checks by `user.user_id`. This widens *who may ask*, not
///   *what may be touched* — a token must not become a way around a check that
///   a session is subject to.
/// * **Nothing under `/admin/`, ever.** User management, token issuance, source
///   credentials and the maintenance jobs stay on session-only `AuthUser`. A
///   leaked token is a bad afternoon; a leaked token that can mint more tokens
///   or read another household member's sources is a different category.
pub struct WriteAuth {
    pub user: AuthUser,
}

#[axum::async_trait]
impl FromRequestParts<Arc<super::AppState>> for WriteAuth {
    type Rejection = Rejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<super::AppState>,
    ) -> Result<Self, Self::Rejection> {
        scoped_token_user(parts, state, "write", |t| t.can_write())
            .await
            .map(|user| WriteAuth { user })
    }
}

/// `IngestAuth` accepts a session cookie **or** an `Authorization: Token`
/// header, whatever scopes that token carries.
///
/// Used by exactly two routes, and the pair is the boundary: `POST
/// /1/submit-listens`, and `GET /api/v1/scrobble-settings`, which is the rule
/// that submission is judged by. A plain `submit` token reaches both and
/// nothing else. The second is deliberately not behind `read` — that would make
/// every scrobble client carry read access to the whole listening history in
/// order to learn one number it is obliged to obey, where the grant should
/// match what the read is *for*.
///
/// Anything that is not "post a listen, or ask when a listen counts" belongs on
/// `ReadAuth`, `WriteAuth` or session-only `AuthUser`.
pub struct IngestAuth {
    pub user: AuthUser,
    /// The signal chain the authenticating *token* defaults to. `None` for a
    /// browser session, which has no token and therefore no default. Consumed
    /// by the ingest chain ladder; every other `IngestAuth` route ignores it.
    pub token_chain_id: Option<i64>,
    /// Which token authenticated, if one did. Sessions are split on it: a
    /// different app almost always means a different setup.
    pub token_id: Option<i64>,
    /// The operator's standing statement that this client's `import`
    /// submissions are live listens. **False for a browser session**, which is
    /// not a scrobble client and has no business relaying a backfill onward.
    pub imports_are_live: bool,
}

#[axum::async_trait]
impl FromRequestParts<Arc<super::AppState>> for IngestAuth {
    type Rejection = Rejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<super::AppState>,
    ) -> Result<Self, Self::Rejection> {
        // 1. Session cookie (browser).
        if let Some(user) = session_user(parts, state).await {
            return Ok(IngestAuth {
                user,
                token_chain_id: None,
                token_id: None,
                imports_are_live: false,
            });
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
                Ok(Some(t)) => Ok(IngestAuth {
                    user: t.user,
                    token_chain_id: t.default_chain_id,
                    token_id: Some(t.token_id),
                    imports_are_live: t.imports_are_live,
                }),
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
