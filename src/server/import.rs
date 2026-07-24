//! Listen import: ListenBrainz / Last.fm public history + file upload.
//!
//! Jobs run as detached tokio tasks and report progress through an in-memory
//! registry. ponytail: registry is in-memory, so job status + import history
//! reset on restart — fine for a single-binary self-host; persist to a table if
//! durable history is ever needed.

use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::db::Database;
use crate::models::Play;
use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;

pub type ImportRegistry = Arc<Mutex<HashMap<String, ImportJob>>>;

#[derive(Clone, Serialize)]
pub struct ImportJob {
    pub job_id: String,
    pub source: String,           // "listenbrainz" | "lastfm" | "file"
    pub state: String,            // "running" | "done" | "error"
    pub fetched: usize,           // listens pulled from the source
    pub imported: usize,          // new rows actually inserted (post-dedup)
    pub total: Option<usize>,     // known upper bound, when the source reports one
    pub status: String,           // human message / error detail
    pub date: i64,                // started, unix seconds
}

fn job_id() -> String {
    use rand::Rng;
    hex::encode(rand::rng().random::<[u8; 8]>())
}

fn update<F: FnOnce(&mut ImportJob)>(reg: &ImportRegistry, id: &str, f: F) {
    if let Ok(mut m) = reg.lock() {
        if let Some(j) = m.get_mut(id) { f(j); }
    }
}

/// Register a fresh running job and return its id.
fn start(reg: &ImportRegistry, source: &str) -> String {
    let id = job_id();
    let job = ImportJob {
        job_id: id.clone(), source: source.into(), state: "running".into(),
        fetched: 0, imported: 0, total: None, status: "started".into(),
        date: chrono_now(),
    };
    if let Ok(mut m) = reg.lock() { m.insert(id.clone(), job); }
    id
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// Stable per-listen key so re-imports dedupe on the exact-match path too
/// (`db.import_listen` also does a ±10min fuzzy check as a backstop).
fn source_id(ts: i64, artist: &str, title: &str) -> String {
    let digest = md5::compute(format!("{artist}\u{1}{title}"));
    format!("{ts}-{:x}", digest)
}

fn play(ts: i64, title: String, artist: String, album: Option<String>, mbid: Option<String>) -> Play {
    Play {
        source_id: source_id(ts, &artist, &title),
        source_name: "import".into(),
        title, artist, album,
        timestamp: ts.max(0) as u64,
        duration: None, track_number: None,
        mbid_recording: mbid, mbid_release: None, mbid_artist: None,
        artists: None, mbid_release_group: None, caa_id: None, caa_release_mbid: None,
    }
}

// ── Request bodies ──

#[derive(Deserialize)]
pub struct ServiceImport {
    pub username: String,
    #[serde(default)]
    pub token: Option<String>,
}

// ════════════════════════════════════════════════════════════
//  Endpoints
// ════════════════════════════════════════════════════════════

async fn import_listenbrainz(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<ServiceImport>,
) -> impl IntoResponse {
    let id = start(&state.imports, "listenbrainz");
    let (reg, db, uid) = (state.imports.clone(), state.db.clone(), user.user_id);
    let ret = id.clone();
    tokio::spawn(async move {
        if let Err(e) = run_listenbrainz(&reg, &id, &db, uid, &body.username, body.token.as_deref()).await {
            update(&reg, &id, |j| { j.state = "error".into(); j.status = e; });
        }
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "job_id": ret }))).into_response()
}

async fn import_lastfm(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<ServiceImport>,
) -> impl IntoResponse {
    if state.lastfm_api_key.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!(
            ErrorResponse { code: 400, error: "Last.fm import requires LASTFM_API_KEY to be configured".into() }
        ))).into_response();
    }
    let id = start(&state.imports, "lastfm");
    let (reg, db, uid, key) = (state.imports.clone(), state.db.clone(), user.user_id, state.lastfm_api_key.clone());
    let ret = id.clone();
    tokio::spawn(async move {
        if let Err(e) = run_lastfm(&reg, &id, &db, uid, &body.username, &key).await {
            update(&reg, &id, |j| { j.state = "error".into(); j.status = e; });
        }
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "job_id": ret }))).into_response()
}

async fn import_file(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut bytes: Option<Vec<u8>> = None;
    let mut name = String::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        name = field.file_name().unwrap_or("").to_string();
        if let Ok(b) = field.bytes().await { bytes = Some(b.to_vec()); }
    }
    let Some(bytes) = bytes else {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!(
            ErrorResponse { code: 400, error: "No file field in multipart body".into() }
        ))).into_response();
    };

    let id = start(&state.imports, "file");
    let (reg, db, uid) = (state.imports.clone(), state.db.clone(), user.user_id);
    let ret = id.clone();
    tokio::spawn(async move {
        if let Err(e) = run_file(&reg, &id, &db, uid, &bytes, &name).await {
            update(&reg, &id, |j| { j.state = "error".into(); j.status = e; });
        }
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "job_id": ret }))).into_response()
}

async fn job_status(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
    Path(job): Path<String>,
) -> impl IntoResponse {
    let found = state.imports.lock().ok().and_then(|m| m.get(&job).cloned());
    match found {
        Some(j) => (StatusCode::OK, Json(serde_json::json!(j))).into_response(),
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!(
            ErrorResponse { code: 404, error: "Unknown job".into() }
        ))).into_response(),
    }
}

async fn import_history(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> impl IntoResponse {
    let mut jobs: Vec<ImportJob> = state.imports.lock().map(|m| m.values().cloned().collect()).unwrap_or_default();
    jobs.sort_by_key(|j| std::cmp::Reverse(j.date));
    let imports: Vec<serde_json::Value> = jobs.iter().map(|j| serde_json::json!({
        "source": j.source, "date": j.date, "count": j.imported, "status": j.state,
    })).collect();
    (StatusCode::OK, Json(serde_json::json!({ "imports": imports }))).into_response()
}

// ════════════════════════════════════════════════════════════
//  Workers
// ════════════════════════════════════════════════════════════

/// Page https://api.listenbrainz.org public listens newest→oldest via max_ts.
async fn run_listenbrainz(
    reg: &ImportRegistry, id: &str, db: &Database, uid: i64,
    username: &str, token: Option<&str>,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let mut max_ts = chrono_now();
    loop {
        let url = format!(
            "https://api.listenbrainz.org/1/user/{}/listens?count=100&max_ts={}",
            urlencoding::encode(username), max_ts,
        );
        let mut req = client.get(&url).header("User-Agent", "tapedeck-import");
        if let Some(t) = token { if !t.is_empty() { req = req.header("Authorization", format!("Token {t}")); } }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("ListenBrainz returned {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let listens = v["payload"]["listens"].as_array().cloned().unwrap_or_default();
        if listens.is_empty() { break; }

        let mut min_ts = max_ts;
        for l in &listens {
            let ts = l["listened_at"].as_i64().unwrap_or(0);
            if ts > 0 && ts < min_ts { min_ts = ts; }
            let tm = &l["track_metadata"];
            let title = tm["track_name"].as_str().unwrap_or("").to_string();
            let artist = tm["artist_name"].as_str().unwrap_or("").to_string();
            if title.is_empty() || artist.is_empty() { continue; }
            let album = tm["release_name"].as_str().map(str::to_string);
            let mbid = tm["mbid_mapping"]["recording_mbid"].as_str().map(str::to_string);
            let inserted = db.import_listen(uid, &play(ts, title, artist, album, mbid)).await.map_err(|e| e.to_string())?;
            update(reg, id, |j| { j.fetched += 1; if inserted { j.imported += 1; } });
        }
        if listens.len() < 100 || min_ts >= max_ts { break; }
        max_ts = min_ts - 1;
    }
    update(reg, id, |j| { j.state = "done".into(); j.status = format!("Imported {} new listens", j.imported); });
    Ok(())
}

/// Page Last.fm user.getRecentTracks (public scrobbles).
async fn run_lastfm(
    reg: &ImportRegistry, id: &str, db: &Database, uid: i64,
    username: &str, api_key: &str,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let mut page = 1u32;
    loop {
        let url = format!(
            "https://ws.audioscrobbler.com/2.0/?method=user.getrecenttracks&user={}&api_key={}&format=json&limit=200&page={}",
            urlencoding::encode(username), api_key, page,
        );
        let resp = client.get(&url).header("User-Agent", "tapedeck-import").send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() { return Err(format!("Last.fm returned {}", resp.status())); }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(msg) = v["message"].as_str() { return Err(format!("Last.fm: {msg}")); }

        let rt = &v["recenttracks"];
        let total_pages: u32 = rt["@attr"]["totalPages"].as_str().and_then(|s| s.parse().ok()).unwrap_or(1);
        if let Some(t) = rt["@attr"]["total"].as_str().and_then(|s| s.parse::<usize>().ok()) {
            update(reg, id, |j| j.total = Some(t));
        }
        let tracks = rt["track"].as_array().cloned().unwrap_or_default();
        for t in &tracks {
            // Skip the "now playing" row (no date, @attr.nowplaying=true).
            let Some(ts) = t["date"]["uts"].as_str().and_then(|s| s.parse::<i64>().ok()) else { continue };
            let title = t["name"].as_str().unwrap_or("").to_string();
            let artist = t["artist"]["#text"].as_str().or(t["artist"]["name"].as_str()).unwrap_or("").to_string();
            if title.is_empty() || artist.is_empty() { continue; }
            let album = t["album"]["#text"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            let mbid = t["mbid"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            let inserted = db.import_listen(uid, &play(ts, title, artist, album, mbid)).await.map_err(|e| e.to_string())?;
            update(reg, id, |j| { j.fetched += 1; if inserted { j.imported += 1; } });
        }
        if page >= total_pages { break; }
        page += 1;
    }
    update(reg, id, |j| { j.state = "done".into(); j.status = format!("Imported {} new listens", j.imported); });
    Ok(())
}

/// Parse an uploaded backup: ListenBrainz JSON export or a CSV
/// (`title,artist,album,format,chain,mbid,time`, our own export shape).
async fn run_file(
    reg: &ImportRegistry, id: &str, db: &Database, uid: i64,
    bytes: &[u8], name: &str,
) -> Result<(), String> {
    let text = String::from_utf8_lossy(bytes);
    let is_csv = name.to_lowercase().ends_with(".csv") || (!text.trim_start().starts_with(['{', '[']));

    let plays: Vec<Play> = if is_csv { parse_csv(&text) } else { parse_json(&text)? };
    update(reg, id, |j| j.total = Some(plays.len()));
    for p in &plays {
        let inserted = db.import_listen(uid, p).await.map_err(|e| e.to_string())?;
        update(reg, id, |j| { j.fetched += 1; if inserted { j.imported += 1; } });
    }
    update(reg, id, |j| { j.state = "done".into(); j.status = format!("Imported {} new listens", j.imported); });
    Ok(())
}

/// ListenBrainz export: top-level array of listens, or `{ "listens": [...] }`.
fn parse_json(text: &str) -> Result<Vec<Play>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {e}"))?;
    let arr = v.as_array().cloned()
        .or_else(|| v["listens"].as_array().cloned())
        .or_else(|| v["scrobbles"].as_array().cloned())
        .ok_or("Expected a JSON array of listens or a { listens: [...] } object")?;
    Ok(arr.iter().filter_map(|l| {
        let ts = l["listened_at"].as_i64().or_else(|| l["timestamp"].as_i64())?;
        let tm = if l["track_metadata"].is_object() { &l["track_metadata"] } else { l };
        let title = tm["track_name"].as_str().or(tm["title"].as_str())?.to_string();
        let artist = tm["artist_name"].as_str().or(tm["artist"].as_str())?.to_string();
        let album = tm["release_name"].as_str().or(tm["album"].as_str()).map(str::to_string);
        let mbid = tm["mbid_mapping"]["recording_mbid"].as_str().or(tm["mbid_recording"].as_str()).map(str::to_string);
        Some(play(ts, title, artist, album, mbid))
    }).collect())
}

/// CSV: header row `title,artist,album,format,chain,mbid,time`.
fn parse_csv(text: &str) -> Vec<Play> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 || line.trim().is_empty() { continue; } // skip header
        let f = split_csv(line);
        if f.len() < 7 { continue; }
        let (title, artist, album, mbid, time) = (&f[0], &f[1], &f[2], &f[5], &f[6]);
        let Ok(ts) = time.trim().parse::<i64>() else { continue };
        if title.is_empty() || artist.is_empty() { continue; }
        out.push(play(
            ts, title.clone(), artist.clone(),
            (!album.is_empty()).then(|| album.clone()),
            (!mbid.is_empty()).then(|| mbid.clone()),
        ));
    }
    out
}

/// Minimal RFC-4180 splitter: handles quoted fields and `""` escapes.
fn split_csv(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_q && chars.peek() == Some(&'"') => { cur.push('"'); chars.next(); }
            '"' => in_q = !in_q,
            ',' if !in_q => { fields.push(std::mem::take(&mut cur)); }
            _ => cur.push(c),
        }
    }
    fields.push(cur);
    fields
}

pub fn create_import_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/import/listenbrainz", post(import_listenbrainz))
        .route("/api/v1/import/lastfm", post(import_lastfm))
        .route("/api/v1/import/file", post(import_file))
        .route("/api/v1/import/jobs/:job_id", get(job_status))
        .route("/api/v1/import/history", get(import_history))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_roundtrip_and_quotes() {
        // Field as our exporter escapes `Song "Live"` → `"Song ""Live"""`.
        let f = split_csv(r#""Song ""Live""",Artist,A,pcm,3,mbid-x,1700000000"#);
        assert_eq!(f[0], r#"Song "Live""#);
        assert_eq!(f[1], "Artist");
        assert_eq!(f[6], "1700000000");
    }

    #[test]
    fn parse_csv_skips_header_and_bad_rows() {
        let csv = "title,artist,album,format,chain,mbid,time\n\
                   Track,Me,Album,pcm,,,1700000000\n\
                   Bad,Row,,,,,notanumber\n";
        let plays = parse_csv(csv);
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].title, "Track");
        assert_eq!(plays[0].timestamp, 1700000000);
    }

    #[test]
    fn parse_json_listenbrainz_shape() {
        let js = r#"{"listens":[{"listened_at":1700000000,"track_metadata":{"track_name":"T","artist_name":"A","release_name":"R"}}]}"#;
        let plays = parse_json(js).unwrap();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].artist, "A");
        assert_eq!(plays[0].album.as_deref(), Some("R"));
    }
}
