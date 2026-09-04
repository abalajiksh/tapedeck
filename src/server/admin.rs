use crate::db::Database;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct AdminState {
    pub db: Arc<Database>,
}

async fn health_check(State(state): State<Arc<AdminState>>) -> impl IntoResponse {
    let db_ok = state.db.ping().await.is_ok();
    let code = if db_ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (
        code,
        Json(serde_json::json!({
            "status": if db_ok { "ok" } else { "degraded" },
            "service": "tapedeck",
            "database": if db_ok { "ok" } else { "unreachable" },
            "version": env!("CARGO_PKG_VERSION"),
            // Which instance is this? A replica is a byte-identical copy
            // serving identical screens, so nothing else on the page tells an
            // operator with two tabs open which one forwards listens. Always
            // present rather than only when true, so its absence is an old
            // binary rather than an authoritative instance.
            "replica": crate::replica::is_replica(),
        })),
    )
}

/// Only `/health` lives here (intentionally unauthenticated). `/log-level` moved
/// to the authenticated, admin-gated router.
pub fn create_admin_router(db: Arc<Database>) -> Router {
    let state = Arc::new(AdminState { db });
    Router::new().route("/health", get(health_check)).with_state(state)
}
