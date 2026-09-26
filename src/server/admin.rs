use crate::db::Database;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Clone)]
pub struct AdminState {
    pub db: Arc<Database>,
}

/// What `/health` answers.
#[derive(Serialize, ToSchema)]
pub struct Health {
    /// `ok`, or `degraded` when the database cannot be reached.
    pub status: &'static str,
    /// Always `tapedeck`.
    pub service: &'static str,
    /// `ok` or `unreachable`.
    pub database: &'static str,
    /// The running binary's version.
    pub version: &'static str,
    /// Whether this instance is a **replica** — a full copy that forwards no
    /// listens, publishes no actor and makes no MusicBrainz request. A staging
    /// instance restored from a production snapshot is byte-identical on every
    /// other screen, so this is the only thing that tells them apart. Always
    /// present; its absence means a binary older than 0.109.0.
    pub replica: bool,
}

/// Liveness and database reachability.
#[utoipa::path(
    get,
    path = "/health",
    tag = "Ops",
    security(),
    responses(
        (status = 200, description = "Up, and the database answers.", body = Health,
            example = json!({ "status": "ok", "service": "tapedeck", "database": "ok",
                              "version": "0.109.0", "replica": false })),
        (status = 503, description = "Up, but the database does not answer. Same body, \
            `status: degraded`.", body = Health),
    )
)]
async fn health_check(State(state): State<Arc<AdminState>>) -> impl IntoResponse {
    let db_ok = state.db.ping().await.is_ok();
    let code = if db_ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (
        code,
        Json(Health {
            status: if db_ok { "ok" } else { "degraded" },
            service: "tapedeck",
            database: if db_ok { "ok" } else { "unreachable" },
            version: env!("CARGO_PKG_VERSION"),
            // Which instance is this? A replica is a byte-identical copy
            // serving identical screens, so nothing else on the page tells an
            // operator with two tabs open which one forwards listens. Always
            // present rather than only when true, so its absence is an old
            // binary rather than an authoritative instance.
            replica: crate::replica::is_replica(),
        }),
    )
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(health_check))]
pub struct Api;

/// Only `/health` lives here (intentionally unauthenticated). `/log-level` moved
/// to the authenticated, admin-gated router.
pub fn create_admin_router(db: Arc<Database>) -> Router {
    let state = Arc::new(AdminState { db });
    Router::new().route("/health", get(health_check)).with_state(state)
}
