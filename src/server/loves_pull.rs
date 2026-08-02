//! Pull loved tracks from the services the user has connected.
//!
//! The ListenBrainz *export* importer has done this since Phase 2 by reading
//! `feedback.jsonl` out of a zip (`import::import_feedback`); this is the live
//! API version of the same idea, so you don't have to export a file to get your
//! loves across.
//!
//! Both services are matched back to the user's own history and **a love whose
//! track isn't in that history is skipped, never invented**. Loves point at
//! entities, entities are things you have actually listened to, and manufacturing
//! rows for records that were never played would put things on the loves page
//! that have no listens behind them.
//!
//! The two services key differently, which is the whole reason this isn't one
//! code path:
//!
//! * **ListenBrainz** returns a recording MBID and nothing else — no names at
//!   all — so it can only match through `track_by_recording_mbid`. A track that
//!   was never enriched can't be matched and is counted as skipped.
//! * **Last.fm** returns artist and track *names* (and sometimes an MBID), so it
//!   matches on the MBID when there is one and falls back to the name.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::post,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

use super::auth::AuthUser;
use super::AppState;
use crate::db::Database;

/// What one service's pull did. Every field is a count of listens, not of
/// requests, so the numbers mean the same thing across services.
#[derive(Debug, Default, serde::Serialize)]
pub struct PullReport {
    pub service: String,
    /// Loves the service reported.
    pub found: usize,
    /// Newly stored — already-loved tracks are not counted twice.
    pub loved: usize,
    /// Reported by the service but not in this user's history.
    pub skipped: usize,
    pub error: Option<String>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_default()
}

// ── ListenBrainz ──

#[derive(Deserialize)]
struct LbFeedback {
    feedback: Vec<LbFeedbackItem>,
}

#[derive(Deserialize)]
struct LbFeedbackItem {
    recording_mbid: Option<String>,
    score: Option<i64>,
}

/// `GET /1/feedback/user/<user>/get-feedback?score=1`, paged.
///
/// Paged with `offset` because a long-standing account can have thousands of
/// loves and the endpoint caps a page at 100.
pub async fn pull_listenbrainz(
    db: &Database,
    user_id: i64,
    base_url: &str,
    lb_username: &str,
) -> PullReport {
    let mut report = PullReport { service: "listenbrainz".into(), ..Default::default() };
    let http = client();
    let mut offset = 0usize;
    const PAGE: usize = 100;

    loop {
        let url = format!(
            "{}/1/feedback/user/{}/get-feedback?score=1&count={}&offset={}",
            base_url.trim_end_matches('/'),
            lb_username,
            PAGE,
            offset
        );
        let resp = match http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                report.error = Some(e.to_string());
                break;
            }
        };
        if !resp.status().is_success() {
            report.error = Some(format!("ListenBrainz returned {}", resp.status()));
            break;
        }
        let page: LbFeedback = match resp.json().await {
            Ok(p) => p,
            Err(e) => {
                report.error = Some(e.to_string());
                break;
            }
        };

        let n = page.feedback.len();
        for item in page.feedback {
            if item.score != Some(1) {
                continue;
            }
            let Some(mbid) = item.recording_mbid.as_deref() else {
                continue;
            };
            report.found += 1;
            // No names in the payload — the MBID is the only key there is.
            match db.track_by_recording_mbid(user_id, mbid).await {
                Ok(Some((title, artist))) => {
                    if store_love(db, user_id, &title, &artist, Some(mbid)).await {
                        report.loved += 1;
                    }
                }
                _ => report.skipped += 1,
            }
        }

        // A short page is the last page.
        if n < PAGE {
            break;
        }
        offset += n;
        // Nobody has this many loves; the guard is against a cursor that never
        // advances, which is a failure mode this endpoint has had before.
        if offset > 50_000 {
            debug!("ListenBrainz feedback paging hit the safety cap at {}", offset);
            break;
        }
    }

    info!(
        "❤️ ListenBrainz loves: {} found, {} stored, {} not in history",
        report.found, report.loved, report.skipped
    );
    report
}

// ── Last.fm ──

#[derive(Deserialize)]
struct LfmLovedResp {
    lovedtracks: Option<LfmLoved>,
}
#[derive(Deserialize)]
struct LfmLoved {
    track: Option<Vec<LfmTrack>>,
    #[serde(rename = "@attr")]
    attr: Option<LfmAttr>,
}
#[derive(Deserialize)]
struct LfmAttr {
    #[serde(rename = "totalPages")]
    total_pages: Option<String>,
}
#[derive(Deserialize)]
struct LfmTrack {
    name: Option<String>,
    mbid: Option<String>,
    artist: Option<LfmArtistRef>,
}
#[derive(Deserialize)]
struct LfmArtistRef {
    name: Option<String>,
}

/// `user.getLovedTracks`, paged.
///
/// Read-only and unsigned — it only needs the application key and the username,
/// not the session key, which is why this works for a connection that was set up
/// purely to scrobble.
pub async fn pull_lastfm(
    db: &Database,
    user_id: i64,
    api_key: &str,
    lastfm_username: &str,
) -> PullReport {
    let mut report = PullReport { service: "lastfm".into(), ..Default::default() };
    let http = client();
    let mut page = 1u32;

    loop {
        let resp = match http
            .get("https://ws.audioscrobbler.com/2.0/")
            .query(&[
                ("method", "user.getlovedtracks"),
                ("user", lastfm_username),
                ("api_key", api_key),
                ("format", "json"),
                ("limit", "200"),
                ("page", &page.to_string()),
            ])
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                report.error = Some(e.to_string());
                break;
            }
        };
        if !resp.status().is_success() {
            report.error = Some(format!("Last.fm returned {}", resp.status()));
            break;
        }
        let body: LfmLovedResp = match resp.json().await {
            Ok(b) => b,
            Err(e) => {
                report.error = Some(e.to_string());
                break;
            }
        };

        let Some(loved) = body.lovedtracks else { break };
        let total_pages = loved
            .attr
            .as_ref()
            .and_then(|a| a.total_pages.as_deref())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(1);
        let tracks = loved.track.unwrap_or_default();
        if tracks.is_empty() {
            break;
        }

        for t in tracks {
            let (Some(title), Some(artist)) =
                (t.name.as_deref(), t.artist.as_ref().and_then(|a| a.name.as_deref()))
            else {
                continue;
            };
            report.found += 1;
            let mbid = t.mbid.as_deref().filter(|m| !m.trim().is_empty());

            // Names are present here, so the MBID is a preference and not a
            // requirement — unlike the ListenBrainz side.
            let matched = match mbid {
                Some(m) => db.track_by_recording_mbid(user_id, m).await.ok().flatten(),
                None => None,
            };
            let (title, artist) = match matched {
                Some(pair) => pair,
                None => match db.track_by_name(user_id, title, artist).await {
                    Ok(Some(pair)) => pair,
                    _ => {
                        report.skipped += 1;
                        continue;
                    }
                },
            };
            if store_love(db, user_id, &title, &artist, mbid).await {
                report.loved += 1;
            }
        }

        if page >= total_pages {
            break;
        }
        page += 1;
        if page > 200 {
            debug!("Last.fm loved-tracks paging hit the safety cap at page {}", page);
            break;
        }
    }

    info!(
        "❤️ Last.fm loves: {} found, {} stored, {} not in history",
        report.found, report.loved, report.skipped
    );
    report
}

/// Upsert the entity and love it. Returns whether this was a *new* love, so a
/// second pull doesn't report every track again.
async fn store_love(
    db: &Database,
    user_id: i64,
    title: &str,
    artist: &str,
    mbid: Option<&str>,
) -> bool {
    let Ok(entity) = db.upsert_entity("recording", title, artist, mbid).await else {
        return false;
    };
    let already = db.is_loved(user_id, entity).await.unwrap_or(false);
    if already {
        return false;
    }
    db.set_love(user_id, entity, true).await.is_ok()
}

// ── Endpoint ──

/// POST /api/v1/loves/pull — fetch loves from every connected service.
///
/// Runs inline rather than as a background job: a loves list is thousands of
/// rows at most, not the hundreds of thousands a listen import walks, so there
/// is nothing here worth the progress-tracking machinery. Each service reports
/// separately, and one failing doesn't stop the other.
async fn pull_loves(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let conns = state.db.list_connections(user.user_id).await.unwrap_or_default();
    let mut reports: Vec<PullReport> = Vec::new();

    for (service, username, _) in conns {
        let Some(username) = username.filter(|u| !u.trim().is_empty()) else {
            // Without the remote account name there is nothing to ask for; the
            // connection still works for scrobbling, so this isn't an error.
            reports.push(PullReport {
                service: service.clone(),
                error: Some("No connected account name on file".into()),
                ..Default::default()
            });
            continue;
        };
        match service.as_str() {
            "listenbrainz" => reports.push(
                pull_listenbrainz(&state.db, user.user_id, &state.listenbrainz_url, &username)
                    .await,
            ),
            "lastfm" => reports.push(
                pull_lastfm(&state.db, user.user_id, crate::sinks::LASTFM_API_KEY, &username).await,
            ),
            // Libre.fm's GNU FM does not implement `user.getLovedTracks`.
            _ => {}
        }
    }

    let loved: usize = reports.iter().map(|r| r.loved).sum();
    (StatusCode::OK, Json(serde_json::json!({ "loved": loved, "services": reports })))
        .into_response()
}

pub fn create_loves_pull_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/loves/pull", post(pull_loves))
}
