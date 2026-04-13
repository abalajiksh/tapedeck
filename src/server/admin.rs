use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::logging::LogLevelHandle;

#[derive(Clone)]
pub struct AdminState {
    pub log_handle: LogLevelHandle,
}

#[derive(Serialize)]
struct LogLevelResponse {
    current_level: String,
    message: String,
}

#[derive(Deserialize)]
struct SetLogLevelRequest {
    level: String,
}

async fn get_log_level(State(state): State<Arc<AdminState>>) -> impl IntoResponse {
    let level = state.log_handle.get_level().await;
    Json(LogLevelResponse {
        current_level: level.clone(),
        message: format!("Current log level: {}", level),
    })
}

async fn set_log_level(
    State(state): State<Arc<AdminState>>,
    Json(payload): Json<SetLogLevelRequest>,
) -> impl IntoResponse {
    match state.log_handle.set_level(&payload.level).await {
        Ok(_) => (
            StatusCode::OK,
            Json(LogLevelResponse {
                current_level: payload.level.to_uppercase(),
                message: format!("Log level updated to {}", payload.level.to_uppercase()),
            }),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(LogLevelResponse {
                current_level: "ERROR".to_string(),
                message: e,
            }),
        ),
    }
}

async fn health_check() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "tapedeck",
	"version": env!("CARGO_PKG_VERSION"),
    }))
}

pub fn create_admin_router(log_handle: LogLevelHandle) -> Router {
    let state = Arc::new(AdminState { log_handle });

    Router::new()
        .route("/health", get(health_check))
        .route("/log-level", get(get_log_level))
        .route("/log-level", post(set_log_level))
        .with_state(state)
}
