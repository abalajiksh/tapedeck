//! Settings → Operations: what the engine is doing, and the log, from inside
//! the running application.
//!
//! **This exists because of 2026-08-24.** One unreachable upstream stalled the
//! engine tick from five seconds to ninety minutes; Plex stopped being polled,
//! the deck went dark, and there was no way to learn any of that without SSH
//! and 3 MB of log. Three force restarts were the only tool available, and they
//! bought a minute each.
//!
//! Everything here is **admin-only and session-only**. It is instance-wide
//! rather than per-user, so it is not an ordinary account's business; and it
//! carries a log containing usernames, source labels and request paths, which
//! is not something a scrobble client's token should ever reach. A token gets
//! **401** — these endpoints do not accept one at all, so the scope never
//! enters into it, which is the line `/api/v1/sources` already sits on.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

fn err(code: u16, msg: impl Into<String>) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

/// Exit code used when the operator asks for a restart.
///
/// **Non-zero on purpose, and it is load-bearing.**
/// `deploy/tapedeck-user.service` is `Restart=on-failure`, so exiting `0` would
/// stop the service rather than restart it — the button would be a very
/// convincing off switch. 75 is `EX_TEMPFAIL`, which is honest about what this
/// is: a deliberate, temporary stop.
pub const RESTART_EXIT_CODE: i32 = 75;

static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Has an admin asked for a restart? Read by `main` once the drain finishes.
pub fn restart_requested() -> bool {
    RESTART_REQUESTED.load(Ordering::SeqCst)
}

/// Wakes when an admin asks for a restart, so `shutdown_signal` can select on
/// it alongside Ctrl-C and SIGTERM.
pub fn restart_notify() -> &'static tokio::sync::Notify {
    static N: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

fn secs(t: SystemTime) -> i64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn create_ops_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/ops", get(ops))
        .route("/admin/logs", get(log_tail))
        .route("/admin/logs/files", get(log_files))
        .route("/admin/logs/download", get(log_download))
        .route("/admin/restart", post(restart))
}

/// One request answering "is this instance healthy?".
///
/// One request rather than four because this is polled while a screen is open,
/// and four connections out of a pool of five is how a diagnostics page becomes
/// the incident — the reasoning `patch/inbox-count` already carries.
async fn ops(State(state): State<Arc<AppState>>, user: AuthUser) -> Response {
    if !user.is_admin {
        return err(403, "Admin only");
    }

    let tick = crate::engine::health::snapshot();
    // Late relative to its **own** interval, which is the only comparison that
    // means anything: 40 seconds is fine at a 30-second interval and is an
    // outage at five.
    let behind = tick.interval_secs > 0 && tick.last_duration_ms / 1000 > tick.interval_secs;

    let sinks: Vec<serde_json::Value> = crate::sinks::health()
        .snapshot()
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "failures": s.failures,
                "total_failures": s.total_failures,
                "total_successes": s.total_successes,
                "last_ok": s.last_ok.map(secs),
                "last_failure": s.last_failure.map(secs),
                "last_error": s.last_error,
                // While this is set the breaker is open and the sink is being
                // skipped outright — the state that turns a day-long outage
                // into a handful of requests instead of one per listen.
                "skipping_for": s.open_for,
            })
        })
        .collect();

    let sources = match state.db.source_health().await {
        Ok(s) => s,
        Err(e) => return err(500, format!("Could not read sources: {e}")),
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_secs": state.started_at.elapsed().as_secs(),
            "tick": {
                "interval_secs": tick.interval_secs,
                "ticks": tick.ticks,
                "last_started": tick.last_started.map(secs),
                "last_duration_ms": tick.last_duration_ms,
                "longest_ms": tick.longest_ms,
                "overruns": tick.overruns,
                "deferred_listens": tick.deferred_listens,
                "behind": behind,
            },
            "sinks": sinks,
            "sources": sources,
            "logs_enabled": !state.log_dir.is_empty(),
        })),
    )
        .into_response()
}

/// The log files this instance is holding, newest first.
///
/// The names come from the directory rather than from the caller, which is what
/// makes [`log_download`] safe: a request names one of *these*, never a path.
fn log_file_names(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("tapedeck.") && n.ends_with(".log"))
        .collect();
    names.sort();
    names.reverse();
    names
}

async fn log_files(State(state): State<Arc<AppState>>, user: AuthUser) -> Response {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    if state.log_dir.is_empty() {
        return err(404, "File logging is switched off on this instance");
    }
    let files: Vec<serde_json::Value> = log_file_names(&state.log_dir)
        .into_iter()
        .map(|name| {
            let bytes = std::fs::metadata(std::path::Path::new(&state.log_dir).join(&name))
                .map(|m| m.len())
                .unwrap_or(0);
            serde_json::json!({ "name": name, "bytes": bytes })
        })
        .collect();
    (StatusCode::OK, Json(serde_json::json!({ "files": files }))).into_response()
}

#[derive(Deserialize)]
pub struct TailQuery {
    /// Newest N lines. Clamped — this reads into memory.
    lines: Option<usize>,
    /// `error` | `warn` | `info` | `debug`, meaning "this and worse".
    level: Option<String>,
    /// Substring filter, applied after the level.
    q: Option<String>,
    /// Which day's file. Defaults to the one being written.
    file: Option<String>,
}

const MAX_TAIL: usize = 2_000;

/// Severity ranking, so `level=warn` means warnings *and* errors rather than
/// warnings alone — which is what anybody typing it into a filter means.
fn rank(line: &str) -> u8 {
    if line.contains(" ERROR ") {
        3
    } else if line.contains(" WARN ") {
        2
    } else if line.contains(" INFO ") {
        1
    } else {
        0
    }
}

fn floor_for(level: Option<&str>) -> u8 {
    match level.unwrap_or("").to_ascii_lowercase().as_str() {
        "error" => 3,
        "warn" => 2,
        "info" => 1,
        _ => 0,
    }
}

/// The tail of a log file, filtered.
///
/// Reads the whole file and keeps the last N matching lines. A day's log is a
/// few megabytes, and the alternative — seeking backwards through a file whose
/// lines are being appended as it is read — is a great deal of machinery for a
/// page an admin opens occasionally. If these ever grow past that, stream it
/// rather than raising [`MAX_TAIL`].
async fn log_tail(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<TailQuery>,
) -> Response {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    if state.log_dir.is_empty() {
        return err(404, "File logging is switched off on this instance");
    }
    let available = log_file_names(&state.log_dir);
    // The requested name has to *be* one of the names we listed. Anything else
    // — including a perfectly innocent-looking relative path — is refused
    // rather than sanitised, because this handler reads whatever it is given
    // and an allow-list cannot be got wrong the way an escaping rule can.
    let name = match q.file {
        Some(f) => match available.iter().find(|n| **n == f) {
            Some(n) => n.clone(),
            None => return err(404, "No such log file"),
        },
        None => match available.first() {
            Some(n) => n.clone(),
            None => return err(404, "No log files yet"),
        },
    };

    let path = std::path::Path::new(&state.log_dir).join(&name);
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(b) => b,
        Err(e) => return err(500, format!("Could not read the log: {e}")),
    };

    let want = q.lines.unwrap_or(300).clamp(1, MAX_TAIL);
    let floor = floor_for(q.level.as_deref());
    let needle = q.q.as_deref().map(|s| s.to_lowercase()).filter(|s| !s.is_empty());

    let mut matched = 0usize;
    let mut kept: std::collections::VecDeque<&str> =
        std::collections::VecDeque::with_capacity(want);
    for line in body.lines() {
        if rank(line) < floor {
            continue;
        }
        if let Some(n) = &needle {
            if !line.to_lowercase().contains(n) {
                continue;
            }
        }
        matched += 1;
        if kept.len() == want {
            kept.pop_front();
        }
        kept.push_back(line);
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "file": name,
            "files": available,
            // What the filter *found*, against what is being shown. Without it
            // a capped tail looks like the whole answer — the same reason the
            // notes search reports an unfiltered total.
            "matched": matched,
            "shown": kept.len(),
            "truncated": matched > kept.len(),
            "lines": kept.into_iter().collect::<Vec<_>>(),
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    file: Option<String>,
}

async fn log_download(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<DownloadQuery>,
) -> Response {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    if state.log_dir.is_empty() {
        return err(404, "File logging is switched off on this instance");
    }
    let available = log_file_names(&state.log_dir);
    let name = match q.file {
        Some(f) => match available.iter().find(|n| **n == f) {
            Some(n) => n.clone(),
            None => return err(404, "No such log file"),
        },
        None => match available.first() {
            Some(n) => n.clone(),
            None => return err(404, "No log files yet"),
        },
    };
    let path = std::path::Path::new(&state.log_dir).join(&name);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) => return err(500, format!("Could not read the log: {e}")),
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
            // A log is somebody's activity. Never a shared cache.
            (header::CACHE_CONTROL, "private, no-store".to_string()),
        ],
        Body::from(bytes),
    )
        .into_response()
}

/// Ask the process to stop so its supervisor brings it back.
///
/// **It does not shell out to `systemctl`.** The unit is `Restart=on-failure`,
/// so draining and exiting [`RESTART_EXIT_CODE`] is enough — which needs no
/// privileges, makes no assumption about the init system beyond the one already
/// documented in `deploy/`, and reuses the graceful shutdown that already
/// exists. A host with no supervisor gets a clean stop and says so below rather
/// than a restart that silently does not happen.
///
/// The drain is the whole point: in-flight requests finish and the engine
/// completes its current tick, so this cannot cut a write in half. That is
/// strictly better than the SIGKILL an operator reaches for otherwise.
async fn restart(user: AuthUser) -> Response {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    tracing::warn!("🔁 Restart requested by {} — draining", user.username);
    RESTART_REQUESTED.store(true, Ordering::SeqCst);
    restart_notify().notify_waiters();
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "restarting",
            "note": "In-flight requests drain and the engine finishes its current tick first. \
                     The process comes back only if something is supervising it — under the \
                     supplied systemd unit it does.",
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A level filter means "this and worse". Somebody asking for warnings
    /// wants to see the errors too — showing warnings *alone* would hide the
    /// severe half of exactly the situation they opened the page for.
    #[test]
    fn a_level_filter_includes_everything_worse_than_it() {
        let e = "2026-08-24T00:01:32Z ERROR ThreadId(01) boom";
        let w = "2026-08-24T00:01:32Z  WARN ThreadId(01) hmm";
        let i = "2026-08-24T00:01:32Z  INFO ThreadId(01) fine";
        assert!(rank(e) >= floor_for(Some("warn")));
        assert!(rank(w) >= floor_for(Some("warn")));
        assert!(rank(i) < floor_for(Some("warn")));
        // And no filter keeps everything, including untagged continuation lines.
        assert!(rank("    at some::frame") >= floor_for(None));
    }

    /// The exit code has to be non-zero or the restart button is an off switch:
    /// the unit is `Restart=on-failure`, and a clean exit is not a failure.
    #[test]
    fn the_restart_exit_code_is_a_failure_so_systemd_brings_it_back() {
        assert_ne!(RESTART_EXIT_CODE, 0);
    }
}
