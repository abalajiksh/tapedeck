use axum::{
    body::Body,
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

/// Streaming phase for the paged export (keeps memory O(page), not O(history)).
enum Phase {
    Open,
    Rows(i64),
    Close,
    End,
}

/// GET /api/v1/export?format=json|csv — full listen dump, streamed in pages so a
/// huge history can't blow up memory. JSON is a plain array (import-compatible).
async fn export(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<ExportQuery>,
) -> impl IntoResponse {
    let csv_fmt = q.format.as_deref() == Some("csv");
    let db = state.db.clone();
    let uid = user.user_id;
    const PAGE: i64 = 5000;

    let stream = futures_util::stream::unfold((Phase::Open, true), move |(phase, first)| {
        let db = db.clone();
        async move {
            match phase {
                Phase::Open => {
                    let chunk = if csv_fmt {
                        "title,artist,album,format,chain,mbid,time\n".to_string()
                    } else {
                        "[".to_string()
                    };
                    Some((Ok::<String, std::io::Error>(chunk), (Phase::Rows(0), first)))
                }
                Phase::Rows(off) => {
                    let page = db
                        .get_recent_scrobbles(uid, PAGE, off, None, None, None, None)
                        .await
                        .unwrap_or_default();
                    if page.is_empty() {
                        return Some((Ok(String::new()), (Phase::Close, first)));
                    }
                    let mut buf = String::new();
                    let mut first = first;
                    for r in &page {
                        if csv_fmt {
                            buf.push_str(&format!(
                                "{},{},{},{},{},{},{}\n",
                                csv(&r.title),
                                csv(&r.artist),
                                csv(r.album.as_deref().unwrap_or("")),
                                csv(r.format_type.as_deref().unwrap_or("")),
                                r.chain_id.map(|c| c.to_string()).unwrap_or_default(),
                                csv(r.mbid_recording.as_deref().unwrap_or("")),
                                r.timestamp
                            ));
                        } else {
                            if first {
                                first = false;
                            } else {
                                buf.push(',');
                            }
                            buf.push_str(&serde_json::to_string(r).unwrap_or_default());
                        }
                    }
                    let n = page.len() as i64;
                    Some((Ok(buf), (Phase::Rows(off + n), first)))
                }
                Phase::Close => {
                    let chunk = if csv_fmt { String::new() } else { "]".to_string() };
                    Some((Ok(chunk), (Phase::End, first)))
                }
                Phase::End => None,
            }
        }
    });

    let (ct, disp) = if csv_fmt {
        ("text/csv", "attachment; filename=\"tapedeck-export.csv\"")
    } else {
        ("application/json", "attachment; filename=\"tapedeck-export.json\"")
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, ct), (header::CONTENT_DISPOSITION, disp)],
        Body::from_stream(stream),
    )
        .into_response()
}

/// GET /api/v1/backup — admin-only consistent SQLite snapshot download.
async fn backup(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!(ErrorResponse { code: 403, error: "Admin only".into() })),
        )
            .into_response();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let tmp = std::env::temp_dir().join(format!("tapedeck-backup-{now}.db"));
    let tmp_str = tmp.to_string_lossy().to_string();

    if let Err(e) = state.db.backup_to(&tmp_str).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse {
                code: 500,
                error: format!("Backup failed: {e}")
            })),
        )
            .into_response();
    }
    let bytes = match tokio::fs::read(&tmp).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!(ErrorResponse {
                    code: 500,
                    error: format!("Backup read failed: {e}")
                })),
            )
                .into_response()
        }
    };
    let _ = tokio::fs::remove_file(&tmp).await;

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-sqlite3"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"tapedeck-backup.db\""),
        ],
        bytes,
    )
        .into_response()
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
    Router::new().route("/api/v1/export", get(export)).route("/api/v1/backup", get(backup))
}
