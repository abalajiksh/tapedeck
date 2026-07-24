use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub format: Option<String>,
}

/// GET /api/v1/export?format=json|csv — full listen dump for the session user.
async fn export(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ExportQuery>,
) -> impl IntoResponse {
    let rows = match state.db.get_all_scrobbles(user.user_id).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!(
            ErrorResponse { code: 500, error: e.to_string() }
        ))).into_response(),
    };

    if q.format.as_deref() == Some("csv") {
        let mut out = String::from("title,artist,album,format,chain,mbid,time\n");
        for r in &rows {
            out.push_str(&format!("{},{},{},{},{},{},{}\n",
                csv(&r.title), csv(&r.artist), csv(r.album.as_deref().unwrap_or("")),
                csv(r.format_type.as_deref().unwrap_or("")),
                r.chain_id.map(|c| c.to_string()).unwrap_or_default(),
                csv(r.mbid_recording.as_deref().unwrap_or("")),
                r.timestamp,
            ));
        }
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/csv"),
             (header::CONTENT_DISPOSITION, "attachment; filename=\"tapedeck-export.csv\"")],
            out,
        ).into_response();
    }

    // default: JSON
    let body = serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "count": rows.len(), "scrobbles": rows });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json"),
         (header::CONTENT_DISPOSITION, "attachment; filename=\"tapedeck-export.json\"")],
        serde_json::to_string(&body).unwrap_or_default(),
    ).into_response()
}

/// Minimal RFC-4180 field escaping: quote if it contains a comma, quote, or newline.
fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn create_export_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/export", get(export))
}
