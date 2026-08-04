//! Prometheus exposition at `/metrics`.
//!
//! Wanted for one concrete reason: a ListenBrainz import stalled twice on a Pi
//! Zero and there was no way to tell a stall from a slowdown, or a network
//! problem from the host running out of memory. So the metrics that matter most
//! here are the live import gauges and resident memory, not request counters.
//!
//! **Auth.** Prometheus doesn't carry a session cookie, so this accepts either a
//! logged-in session *or* `Authorization: Bearer <TAPEDECK_METRICS_TOKEN>` when
//! that variable is set. It is never open: listen counts and usernames are the
//! user's business, and this instance is reachable from the internet whenever
//! it's behind a tunnel. Set the variable to scrape it.

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::fmt::Write as _;
use std::sync::Arc;

use super::AppState;

/// Resident set size, from `/proc/self/statm`.
///
/// Linux only — that is where Tapedeck is deployed, and where the 512 MB host
/// that may or may not be OOM-killing an import lives. Elsewhere the metric is
/// simply absent rather than faked.
#[cfg(target_os = "linux")]
fn resident_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    // field 2 is resident pages
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

#[cfg(not(target_os = "linux"))]
fn resident_bytes() -> Option<u64> {
    None
}

fn metric(out: &mut String, name: &str, help: &str, kind: &str, value: impl std::fmt::Display) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    let _ = writeln!(out, "{name} {value}");
}

/// Escape a Prometheus label value (backslash, quote, newline).
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ")
}

async fn metrics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    // ── auth ──
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::trim);
    let configured = std::env::var("TAPEDECK_METRICS_TOKEN").ok().filter(|t| !t.trim().is_empty());
    let token_ok = match (&configured, bearer) {
        (Some(want), Some(got)) => want == got,
        _ => false,
    };

    if !token_ok {
        // Fall back to a browser session so an admin can eyeball it — but it
        // has to actually be an admin. Every gauge below is instance-wide
        // (`tapedeck_users`, `tapedeck_listens`, and every other user's import
        // progress), so merely holding a valid session is not enough: on a
        // household instance that is one member reading the others' totals.
        let session = extract_session_cookie_from(&headers);
        let is_admin = match session {
            Some(t) => state
                .db
                .validate_session(&t)
                .await
                .ok()
                .flatten()
                .is_some_and(|u| u.is_admin),
            None => false,
        };
        if !is_admin {
            return (
                StatusCode::UNAUTHORIZED,
                "Set TAPEDECK_METRICS_TOKEN and scrape with a Bearer token, or sign in as an admin.\n",
            )
                .into_response();
        }
    }

    let mut out = String::with_capacity(2048);

    let _ = writeln!(out, "# HELP tapedeck_build_info Build information.");
    let _ = writeln!(out, "# TYPE tapedeck_build_info gauge");
    let _ =
        writeln!(out, "tapedeck_build_info{{version=\"{}\"}} 1", esc(env!("CARGO_PKG_VERSION")));

    if let Some(rss) = resident_bytes() {
        metric(
            &mut out,
            "process_resident_memory_bytes",
            "Resident memory of the Tapedeck process.",
            "gauge",
            rss,
        );
    }

    metric(
        &mut out,
        "tapedeck_uptime_seconds",
        "Seconds since this process started serving.",
        "gauge",
        state.started_at.elapsed().as_secs(),
    );

    // ── database ──
    if let Ok(users) = state.db.list_users().await {
        metric(&mut out, "tapedeck_users", "Registered users.", "gauge", users.len());

        let _ = writeln!(out, "# HELP tapedeck_listens Stored listens per user.");
        let _ = writeln!(out, "# TYPE tapedeck_listens gauge");
        for u in &users {
            if let Ok((count, _)) = state.db.listen_count_and_latest(u.id).await {
                let _ =
                    writeln!(out, "tapedeck_listens{{user=\"{}\"}} {}", esc(&u.username), count);
            }
        }
    }

    if let Ok(pending) = state.db.get_pending_scrobbles().await {
        metric(
            &mut out,
            "tapedeck_scrobbles_pending",
            "Listens awaiting delivery to a sink.",
            "gauge",
            pending.len(),
        );
    }

    // ── imports: the reason this endpoint exists ──
    // Per-job gauges make a stall visible as a flat line rather than something
    // you have to infer from the absence of a completion.
    let jobs: Vec<_> =
        state.imports.lock().ok().map(|m| m.values().cloned().collect()).unwrap_or_default();

    metric(
        &mut out,
        "tapedeck_import_jobs_running",
        "Import jobs currently running.",
        "gauge",
        jobs.iter().filter(|j| j.state == "running").count(),
    );

    if !jobs.is_empty() {
        let _ = writeln!(out, "# HELP tapedeck_import_fetched Listens pulled from the source.");
        let _ = writeln!(out, "# TYPE tapedeck_import_fetched gauge");
        for j in &jobs {
            let _ = writeln!(
                out,
                "tapedeck_import_fetched{{job=\"{}\",source=\"{}\",state=\"{}\"}} {}",
                esc(&j.job_id),
                esc(&j.source),
                esc(&j.state),
                j.fetched
            );
        }
        let _ = writeln!(out, "# HELP tapedeck_import_imported New listens actually stored.");
        let _ = writeln!(out, "# TYPE tapedeck_import_imported gauge");
        for j in &jobs {
            let _ = writeln!(
                out,
                "tapedeck_import_imported{{job=\"{}\",source=\"{}\"}} {}",
                esc(&j.job_id),
                esc(&j.source),
                j.imported
            );
        }
    }

    (StatusCode::OK, [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], out)
        .into_response()
}

/// The same cookie the `AuthUser` extractor reads. `auth::extract_session_cookie`
/// takes request `Parts`; a plain header map is all this handler has.
fn extract_session_cookie_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get("cookie")?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|c| c.trim().strip_prefix("td_session=").map(|v| v.to_string()))
}

pub fn create_metrics_router() -> Router<Arc<AppState>> {
    Router::new().route("/metrics", get(metrics))
}
