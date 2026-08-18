//! Listen import: ListenBrainz / Last.fm public history + file upload.
//!
//! Jobs run as detached tokio tasks. Live progress is tracked in an in-memory
//! registry (for the polling `/jobs/:id` endpoint), while each job is also
//! persisted to `import_jobs` at start and updated on completion — so history
//! survives restarts and an interrupted job is recorded (marked `interrupted`
//! on the next startup) rather than lost.

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
use std::time::Duration;
use tokio::time::sleep;
use tracing::warn;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::{Database, ImportOutcome};
use crate::models::Play;

pub type ImportRegistry = Arc<Mutex<HashMap<String, ImportJob>>>;

#[derive(Clone, Serialize)]
pub struct ImportJob {
    pub job_id: String,
    #[serde(skip)]
    pub user_id: i64, // owner (not serialized to clients)
    #[serde(skip)]
    pub db_id: i64, // row in import_jobs (persisted for durable history)
    pub source: String,  // "listenbrainz" | "lastfm" | "file"
    pub state: String,   // "running" | "done" | "error"
    pub fetched: usize,  // listens pulled from the source
    pub imported: usize, // new rows actually inserted (post-dedup)
    /// Listens already held that the incoming copy taught something to — an
    /// MBID, a duration, a cover-art id. Counted apart from `imported` because
    /// "0 new listens" reads as "nothing happened", and on a re-import of a
    /// history you already hold, a great deal just did.
    pub enriched: usize,
    pub total: Option<usize>, // known upper bound, when the source reports one
    pub status: String,       // human message / error detail
    pub date: i64,            // started, unix seconds
}

fn job_id() -> String {
    use rand::Rng;
    hex::encode(rand::rng().random::<[u8; 8]>())
}

fn update<F: FnOnce(&mut ImportJob)>(reg: &ImportRegistry, id: &str, f: F) {
    if let Ok(mut m) = reg.lock() {
        if let Some(j) = m.get_mut(id) {
            f(j);
        }
    }
}

/// Register a fresh running job (persisted to the DB at start) and return its id.
async fn start(reg: &ImportRegistry, db: &Database, source: &str, user_id: i64) -> String {
    let id = job_id();
    let db_id = db.create_import_job(user_id, source).await.unwrap_or(0);
    let job = ImportJob {
        job_id: id.clone(),
        user_id,
        db_id,
        source: source.into(),
        state: "running".into(),
        fetched: 0,
        imported: 0,
        enriched: 0,
        total: None,
        status: "started".into(),
        date: chrono_now(),
    };
    if let Ok(mut m) = reg.lock() {
        m.insert(id.clone(), job);
    }
    id
}

/// After a job finishes, update its persisted row so history survives restarts
/// (the live registry stays in-memory for progress).
async fn persist_job(reg: &ImportRegistry, id: &str, db: &Database) {
    let job = reg.lock().ok().and_then(|m| m.get(id).cloned());
    if let Some(j) = job {
        let _ = db
            .finish_import_job(j.db_id, &j.state, j.fetched as i64, j.imported as i64, &j.status)
            .await;
    }
}

/// What a finished job says it did.
///
/// Enrichment is reported *beside* the insert count rather than folded into it:
/// re-importing a history you already hold correctly inserts nothing, and
/// "Imported 0 new listens" read as though the whole job had been pointless
/// when it had in fact filled in MBIDs, durations and cover ids on hundreds of
/// rows. Silent when nothing was enriched, so an ordinary first import reads
/// exactly as it did before.
fn imported_line(imported: usize, enriched: usize, whence: &str) -> String {
    let mut out = format!("Imported {imported} new listen{}", if imported == 1 { "" } else { "s" });
    if !whence.is_empty() {
        out.push_str(whence);
    }
    if enriched > 0 {
        out.push_str(&format!(
            "; filled in metadata on {enriched} listen{} already held",
            if enriched == 1 { "" } else { "s" }
        ));
    }
    out
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

fn play(
    ts: i64,
    title: String,
    artist: String,
    album: Option<String>,
    mbid: Option<String>,
) -> Play {
    Play {
        source_id: source_id(ts, &artist, &title),
        source_name: "import".into(),
        title,
        artist,
        album,
        timestamp: ts.max(0) as u64,
        duration: None,
        track_number: None,
        mbid_recording: mbid,
        mbid_release: None,
        mbid_artist: None,
        artists: None,
        mbid_release_group: None,
        caa_id: None,
        caa_release_mbid: None,
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
    let id = start(&state.imports, &state.db, "listenbrainz", user.user_id).await;
    let (reg, db, uid) = (state.imports.clone(), state.db.clone(), user.user_id);
    let ret = id.clone();
    tokio::spawn(async move {
        if let Err(e) =
            run_listenbrainz(&reg, &id, &db, uid, &body.username, body.token.as_deref()).await
        {
            update(&reg, &id, |j| {
                j.state = "error".into();
                j.status = e;
            });
        }
        persist_job(&reg, &id, &db).await;
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "job_id": ret }))).into_response()
}

async fn import_lastfm(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<ServiceImport>,
) -> impl IntoResponse {
    if crate::sinks::LASTFM_API_KEY.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "Last.fm import requires LASTFM_API_KEY to be configured".into()
            })),
        )
            .into_response();
    }
    let id = start(&state.imports, &state.db, "lastfm", user.user_id).await;
    let (reg, db, uid, key) = (
        state.imports.clone(),
        state.db.clone(),
        user.user_id,
        crate::sinks::LASTFM_API_KEY.to_string(),
    );
    let ret = id.clone();
    tokio::spawn(async move {
        if let Err(e) = run_lastfm(&reg, &id, &db, uid, &body.username, &key).await {
            update(&reg, &id, |j| {
                j.state = "error".into();
                j.status = e;
            });
        }
        persist_job(&reg, &id, &db).await;
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
    // Rockbox logs whose header says `#TZ/UNKNOWN` carry local-clock timestamps,
    // so the client sends the device's timezone alongside the file.
    let mut device_tz: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "timezone" => {
                device_tz = field.text().await.ok().filter(|t| !t.trim().is_empty());
            }
            _ => {
                name = field.file_name().unwrap_or("").to_string();
                if let Ok(b) = field.bytes().await {
                    bytes = Some(b.to_vec());
                }
            }
        }
    }
    let Some(bytes) = bytes else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!(ErrorResponse {
                code: 400,
                error: "No file field in multipart body".into()
            })),
        )
            .into_response();
    };

    let id = start(&state.imports, &state.db, "file", user.user_id).await;
    let (reg, db, uid) = (state.imports.clone(), state.db.clone(), user.user_id);
    let ret = id.clone();
    tokio::spawn(async move {
        if let Err(e) = run_file(&reg, &id, &db, uid, &bytes, &name, device_tz.as_deref()).await {
            update(&reg, &id, |j| {
                j.state = "error".into();
                j.status = e;
            });
        }
        persist_job(&reg, &id, &db).await;
    });
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "job_id": ret }))).into_response()
}

async fn job_status(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(job): Path<String>,
) -> impl IntoResponse {
    let found = state
        .imports
        .lock()
        .ok()
        .and_then(|m| m.get(&job).cloned())
        .filter(|j| j.user_id == user.user_id);
    match found {
        Some(j) => (StatusCode::OK, Json(serde_json::json!(j))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!(ErrorResponse { code: 404, error: "Unknown job".into() })),
        )
            .into_response(),
    }
}

/// GET /api/v1/import/jobs — the caller's jobs that are still running.
///
/// The import page only learned a job id from the response to the POST that
/// started it, so navigating away and back lost the progress bar while the job
/// carried on in the background. This lets the page find its own work again.
async fn active_jobs(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let jobs: Vec<ImportJob> = state
        .imports
        .lock()
        .ok()
        .map(|m| {
            m.values()
                .filter(|j| j.user_id == user.user_id && j.state == "running")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    (StatusCode::OK, Json(serde_json::json!({ "jobs": jobs }))).into_response()
}

async fn import_history(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    match state.db.list_imports(user.user_id).await {
        Ok(rows) => {
            let imports: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
                "source": r.source, "date": r.created_at, "count": r.imported, "status": r.state,
            })).collect();
            (StatusCode::OK, Json(serde_json::json!({ "imports": imports }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!(ErrorResponse { code: 500, error: e.to_string() })),
        )
            .into_response(),
    }
}

// ════════════════════════════════════════════════════════════
//  Workers
// ════════════════════════════════════════════════════════════

/// One page of listens, retried through the failures a long walk actually hits.
///
/// A 260k-listen history is ~2,600 requests, and around the 800th ListenBrainz
/// starts closing connections on us — the import died at roughly 80k listens,
/// twice, on two accounts. There was nothing wrong with the paging: the first
/// read timeout propagated straight out and killed the job.
///
/// So: an explicit timeout (the default client has none, so a hung socket hangs
/// forever), exponential backoff on network errors and 5xx, and 429 handled via
/// `X-RateLimit-Reset-In` rather than a guess.
async fn lb_get_page(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    reg: &ImportRegistry,
    id: &str,
) -> Result<serde_json::Value, String> {
    const MAX_ATTEMPTS: u32 = 6;
    let mut attempt = 0u32;

    loop {
        attempt += 1;
        let mut req = client.get(url).header("User-Agent", "tapedeck-import");
        if let Some(t) = token.filter(|t| !t.is_empty()) {
            req = req.header("Authorization", format!("Token {t}"));
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                if attempt >= MAX_ATTEMPTS {
                    return Err(format!("giving up after {attempt} attempts: {e}"));
                }
                let wait = (1u64 << attempt).min(60);
                warn!("Import: {} — retrying in {}s (attempt {})", e, wait, attempt);
                update(reg, id, |j| {
                    j.status = format!("Network hiccup, retrying in {wait}s…");
                });
                sleep(Duration::from_secs(wait)).await;
                continue;
            }
        };

        // Rate limited: the service tells us exactly how long to wait.
        if resp.status().as_u16() == 429 {
            let wait = resp
                .headers()
                .get("X-RateLimit-Reset-In")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(5)
                + 1;
            update(reg, id, |j| j.status = format!("Rate limited, waiting {wait}s…"));
            sleep(Duration::from_secs(wait)).await;
            continue;
        }

        if resp.status().is_server_error() {
            if attempt >= MAX_ATTEMPTS {
                return Err(format!("ListenBrainz returned {}", resp.status()));
            }
            let wait = (1u64 << attempt).min(60);
            update(reg, id, |j| j.status = format!("ListenBrainz busy, retrying in {wait}s…"));
            sleep(Duration::from_secs(wait)).await;
            continue;
        }

        if !resp.status().is_success() {
            return Err(format!("ListenBrainz returned {}", resp.status()));
        }

        // Stay ahead of the limit rather than waiting to be told off — the
        // headers are on every response, so this costs nothing.
        let remaining = resp
            .headers()
            .get("X-RateLimit-Remaining")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok());
        let reset_in = resp
            .headers()
            .get("X-RateLimit-Reset-In")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        let body = resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())?;

        if remaining.is_some_and(|r| r < 3) {
            sleep(Duration::from_secs(reset_in.unwrap_or(1) + 1)).await;
        }
        return Ok(body);
    }
}

/// Page https://api.listenbrainz.org public listens newest→oldest via max_ts.
async fn run_listenbrainz(
    reg: &ImportRegistry,
    id: &str,
    db: &Database,
    uid: i64,
    username: &str,
    token: Option<&str>,
) -> Result<(), String> {
    // The default client has *no* timeout: a stalled read would hang the job
    // forever with no way to tell it from slow progress.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let mut max_ts = chrono_now();
    let mut pages = 0u32;
    loop {
        let url = format!(
            "https://api.listenbrainz.org/1/user/{}/listens?count=100&max_ts={}",
            urlencoding::encode(username),
            max_ts,
        );
        let v = lb_get_page(&client, &url, token, reg, id).await?;
        let listens = v["payload"]["listens"].as_array().cloned().unwrap_or_default();
        if listens.is_empty() {
            break;
        }
        pages += 1;

        let mut min_ts = max_ts;
        for l in &listens {
            let ts = l["listened_at"].as_i64().unwrap_or(0);
            if ts > 0 && ts < min_ts {
                min_ts = ts;
            }
            let tm = &l["track_metadata"];
            let title = tm["track_name"].as_str().unwrap_or("").to_string();
            let artist = tm["artist_name"].as_str().unwrap_or("").to_string();
            if title.is_empty() || artist.is_empty() {
                continue;
            }
            let album = tm["release_name"].as_str().map(str::to_string);
            let mbid = tm["mbid_mapping"]["recording_mbid"].as_str().map(str::to_string);
            let outcome = db
                .import_listen(uid, &play(ts, title, artist, album, mbid))
                .await
                .map_err(|e| e.to_string())?;
            update(reg, id, |j| {
                j.fetched += 1;
                if outcome == ImportOutcome::Enriched {
                    j.enriched += 1;
                }
                if outcome == ImportOutcome::Inserted {
                    j.imported += 1;
                }
            });
        }
        if listens.len() < 100 || min_ts >= max_ts {
            break;
        }
        max_ts = min_ts - 1;

        update(reg, id, |j| {
            j.status = format!("Fetched {} listens ({} pages)…", j.fetched, pages);
        });
        // Deliberate pacing. Without it a 260k history is 2,600 back-to-back
        // requests, which is what got the connection closed on us in the first
        // place; the wall-clock cost is ~17 minutes of waiting across an import
        // that already takes far longer than that.
        sleep(Duration::from_millis(400)).await;
    }
    update(reg, id, |j| {
        j.state = "done".into();
        j.status = imported_line(j.imported, j.enriched, "");
    });
    Ok(())
}

/// Page Last.fm user.getRecentTracks (public scrobbles).
async fn run_lastfm(
    reg: &ImportRegistry,
    id: &str,
    db: &Database,
    uid: i64,
    username: &str,
    api_key: &str,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let mut page = 1u32;
    loop {
        let url = format!(
            "https://ws.audioscrobbler.com/2.0/?method=user.getrecenttracks&user={}&api_key={}&format=json&limit=200&page={}",
            urlencoding::encode(username), api_key, page,
        );
        let resp = client
            .get(&url)
            .header("User-Agent", "tapedeck-import")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Last.fm returned {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(msg) = v["message"].as_str() {
            return Err(format!("Last.fm: {msg}"));
        }

        let rt = &v["recenttracks"];
        let total_pages: u32 =
            rt["@attr"]["totalPages"].as_str().and_then(|s| s.parse().ok()).unwrap_or(1);
        if let Some(t) = rt["@attr"]["total"].as_str().and_then(|s| s.parse::<usize>().ok()) {
            update(reg, id, |j| j.total = Some(t));
        }
        let tracks = rt["track"].as_array().cloned().unwrap_or_default();
        for t in &tracks {
            // Skip the "now playing" row (no date, @attr.nowplaying=true).
            let Some(ts) = t["date"]["uts"].as_str().and_then(|s| s.parse::<i64>().ok()) else {
                continue;
            };
            let title = t["name"].as_str().unwrap_or("").to_string();
            let artist = t["artist"]["#text"]
                .as_str()
                .or(t["artist"]["name"].as_str())
                .unwrap_or("")
                .to_string();
            if title.is_empty() || artist.is_empty() {
                continue;
            }
            let album = t["album"]["#text"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            let mbid = t["mbid"].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            let outcome = db
                .import_listen(uid, &play(ts, title, artist, album, mbid))
                .await
                .map_err(|e| e.to_string())?;
            update(reg, id, |j| {
                j.fetched += 1;
                if outcome == ImportOutcome::Enriched {
                    j.enriched += 1;
                }
                if outcome == ImportOutcome::Inserted {
                    j.imported += 1;
                }
            });
        }
        if page >= total_pages {
            break;
        }
        page += 1;
    }
    update(reg, id, |j| {
        j.state = "done".into();
        j.status = imported_line(j.imported, j.enriched, "");
    });
    Ok(())
}

/// Parse an uploaded backup: a ListenBrainz or Spotify export (`.zip` or a
/// single `.json`/`.jsonl`), our own CSV, or a Rockbox `.scrobbler.log`.
async fn run_file(
    reg: &ImportRegistry,
    id: &str,
    db: &Database,
    uid: i64,
    bytes: &[u8],
    name: &str,
    device_tz: Option<&str>,
) -> Result<(), String> {
    // Sniff the archive by its magic bytes rather than the filename: people
    // rename downloads, and a mislabelled `.json` that is really a zip should
    // still import rather than fail on the first byte.
    //
    // Two archive formats arrive here now, and they are told apart by what is
    // *inside* rather than by a second magic number — both are plain zips. A
    // Spotify export is unmistakable from its entry names alone, so that check
    // is cheap and needs no decompression.
    if bytes.starts_with(b"PK\x03\x04") {
        let names = zip_entry_names(bytes)?;
        if crate::spotify::looks_like_export(&names) {
            return run_spotify_archive(reg, id, db, uid, bytes).await;
        }
        return run_listenbrainz_archive(reg, id, db, uid, bytes).await;
    }

    let text = String::from_utf8_lossy(bytes);

    if crate::rockbox::looks_like_scrobbler_log(&text) {
        return run_scrobbler_log(reg, id, db, uid, &text, device_tz).await;
    }

    let is_csv =
        name.to_lowercase().ends_with(".csv") || (!text.trim_start().starts_with(['{', '[']));
    let plays: Vec<Play> = if is_csv { parse_csv(&text) } else { parse_listens(&text)? };
    update(reg, id, |j| j.total = Some(plays.len()));
    for p in &plays {
        let outcome = db.import_listen(uid, p).await.map_err(|e| e.to_string())?;
        update(reg, id, |j| {
            j.fetched += 1;
            if outcome == ImportOutcome::Enriched {
                j.enriched += 1;
            }
            if outcome == ImportOutcome::Inserted {
                j.imported += 1;
            }
        });
    }
    update(reg, id, |j| {
        j.state = "done".into();
        j.status = imported_line(j.imported, j.enriched, "");
    });
    Ok(())
}

/// The entry names of a zip, without decompressing anything.
///
/// Used only to decide which archive parser to route to — the central directory
/// carries the names, so this reads no member bodies.
fn zip_entry_names(bytes: &[u8]) -> Result<Vec<String>, String> {
    let reader = std::io::Cursor::new(bytes);
    let zip = zip::ZipArchive::new(reader).map_err(|e| format!("Unreadable zip: {e}"))?;
    Ok(zip.file_names().map(str::to_string).collect())
}

/// A Spotify "Extended Streaming History" archive.
///
/// Everything that makes this format treacherous — `ts` being an *end* time,
/// the track-length table hiding in the `trackdone` rows, Spotify's `skipped`
/// meaning something other than ours — lives in `crate::spotify`, which is pure
/// and tested. This half does the walking and the inserting.
///
/// The opposite of the ListenBrainz archive in one respect worth knowing: those
/// listens arrive with MBIDs and cover art and cost nothing afterwards, while
/// these carry no MusicBrainz identifiers at all, so every distinct track here
/// is a rate-limited lookup for the enrichment backfill to work through later.
async fn run_spotify_archive(
    reg: &ImportRegistry,
    id: &str,
    db: &Database,
    uid: i64,
    bytes: &[u8],
) -> Result<(), String> {
    use std::io::Read;

    let reader = std::io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| format!("Unreadable zip: {e}"))?;

    // Read every history file before parsing any of it: the duration table is
    // built across the whole export, so a track played to the end once in 2023
    // gives its length to a truncated play of it in 2025.
    let mut files: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("Unreadable zip entry: {e}"))?;
        if !entry.is_file() {
            continue;
        }
        let path = entry.name().to_string();
        // macOS zips carry a __MACOSX shadow tree of AppleDouble files.
        if path.starts_with("__MACOSX/") || path.contains("/.") || path.starts_with('.') {
            continue;
        }
        if !crate::spotify::is_history_file(&path) {
            continue;
        }
        let mut text = String::new();
        entry.read_to_string(&mut text).map_err(|e| format!("Could not read {path}: {e}"))?;
        files.push(text);
    }

    update(reg, id, |j| j.status = "Reading the export…".into());
    let parsed = crate::spotify::parse(&files);

    if parsed.entries.is_empty() {
        return Err("No music listens found in that archive — is it a Spotify \
                    Extended Streaming History export?"
            .into());
    }

    update(reg, id, |j| j.total = Some(parsed.entries.len()));

    let mut skips = 0usize;
    for e in &parsed.entries {
        let mut p = play(e.timestamp, e.title.clone(), e.artist.clone(), e.album.clone(), None);
        // Stable and unique per play. The track id alone would collide on every
        // repeat, and the timestamp alone on anything played simultaneously on
        // two devices — which this export does contain.
        p.source_id = format!("spotify-{}-{}", e.track_id, e.timestamp);
        p.source_name = "spotify".into();
        p.duration = e.duration;

        let provenance = crate::models::ImportProvenance {
            // "Spotify (android)" rather than a bare OS name: the row's client
            // really was Spotify, and the platform is what distinguishes one of
            // these listens from another.
            submission_client: Some(match &e.platform {
                Some(plat) => format!("Spotify ({plat})"),
                None => "Spotify".into(),
            }),
            spotify_track_id: Some(e.track_id.clone()),
        };

        let outcome = db
            .import_listen_detailed(uid, &p, e.skipped, provenance)
            .await
            .map_err(|e| e.to_string())?;
        let inserted = outcome == ImportOutcome::Inserted;
        if inserted && e.skipped {
            skips += 1;
        }
        update(reg, id, |j| {
            j.fetched += 1;
            if outcome == ImportOutcome::Enriched {
                j.enriched += 1;
            }
            // Skips are stored but are not listens, so they don't inflate the
            // count — same rule as the Rockbox path.
            if inserted && !e.skipped {
                j.imported += 1;
            }
        });
    }

    let (non_music, duplicates, malformed) =
        (parsed.non_music, parsed.duplicates, parsed.malformed);
    update(reg, id, |j| {
        j.state = "done".into();
        let mut parts = vec![imported_line(j.imported, j.enriched, " from Spotify")];
        if skips > 0 {
            parts.push(format!("{skips} skips recorded"));
        }
        if non_music > 0 {
            parts.push(format!("{non_music} podcast/audiobook plays ignored"));
        }
        if duplicates > 0 {
            parts.push(format!("{duplicates} duplicate rows collapsed"));
        }
        if malformed > 0 {
            parts.push(format!("{malformed} unreadable rows ignored"));
        }
        j.status = parts.join(" · ");
    });
    Ok(())
}

/// A ListenBrainz "export your data" archive.
///
/// Shape (as of 2026): `user.json`, `feedback.jsonl`, and `listens/<year>/<month>.jsonl`
/// with one listen per line. Every `.jsonl` under `listens/` is read; anything
/// else in the archive is ignored, so a future export can add files without
/// breaking this.
///
/// The listens carry a full `mbid_mapping`, so this is the one import path where
/// recording/release/artist MBIDs and cover art come for free — those listens
/// never need a MusicBrainz lookup afterwards.
async fn run_listenbrainz_archive(
    reg: &ImportRegistry,
    id: &str,
    db: &Database,
    uid: i64,
    bytes: &[u8],
) -> Result<(), String> {
    use std::io::Read;

    let reader = std::io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| format!("Unreadable zip: {e}"))?;

    // Read everything first so `total` is known before the slow part starts —
    // an import with no upper bound shows a progress bar that never fills.
    let mut plays: Vec<Play> = Vec::new();
    let mut feedback = String::new();
    let mut listen_files = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("Unreadable zip entry: {e}"))?;
        if !entry.is_file() {
            continue;
        }
        let path = entry.name().to_string();
        // macOS zips carry a __MACOSX shadow tree of AppleDouble files.
        if path.starts_with("__MACOSX/") || path.contains("/.") || path.starts_with('.') {
            continue;
        }
        let mut text = String::new();
        if path.ends_with("feedback.jsonl") {
            let _ = entry.read_to_string(&mut feedback);
            continue;
        }
        if !path.ends_with(".jsonl") && !path.ends_with(".json") {
            continue;
        }
        if path.ends_with("user.json") {
            continue;
        }
        entry.read_to_string(&mut text).map_err(|e| format!("Could not read {path}: {e}"))?;
        listen_files += 1;
        plays.extend(parse_listens(&text)?);
    }

    if plays.is_empty() {
        return Err("No listens found in that archive — is it a ListenBrainz export?".into());
    }

    // Oldest first, so an interrupted import leaves a contiguous history rather
    // than a hole in the middle.
    plays.sort_by_key(|p| p.timestamp);
    update(reg, id, |j| j.total = Some(plays.len()));

    for p in &plays {
        let outcome = db.import_listen(uid, p).await.map_err(|e| e.to_string())?;
        update(reg, id, |j| {
            j.fetched += 1;
            if outcome == ImportOutcome::Enriched {
                j.enriched += 1;
            }
            if outcome == ImportOutcome::Inserted {
                j.imported += 1;
            }
        });
    }

    let loved = import_feedback(db, uid, &feedback).await;

    update(reg, id, |j| {
        j.state = "done".into();
        let mut parts = vec![imported_line(
            j.imported,
            j.enriched,
            &format!(" from {listen_files} file{}", if listen_files == 1 { "" } else { "s" }),
        )];
        if loved > 0 {
            parts.push(format!("{loved} loved tracks"));
        }
        j.status = parts.join(" · ");
    });
    Ok(())
}

/// Turn `feedback.jsonl` into loves.
///
/// The rows carry only a `recording_mbid` and a score, so the track has to be
/// matched against a listen we now hold to learn its title and artist. That's
/// why this runs *after* the listens are in. A love whose recording never
/// appears in the history is skipped rather than guessed at, and `score: -1`
/// (a "hate" on ListenBrainz) has no equivalent here, so it's ignored.
///
/// Never fails the import: the listens are the point, and losing a love is not
/// worth failing a 50,000-row import over.
async fn import_feedback(db: &Database, uid: i64, jsonl: &str) -> usize {
    let mut loved = 0usize;
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["score"].as_i64() != Some(1) {
            continue;
        }
        let Some(mbid) = v["recording_mbid"].as_str() else { continue };
        let Ok(Some((title, artist))) = db.track_by_recording_mbid(uid, mbid).await else {
            continue;
        };
        let Ok(entity) = db.upsert_entity("recording", &title, &artist, Some(mbid)).await else {
            continue;
        };
        if db.set_love(uid, entity, true).await.is_ok() {
            loved += 1;
        }
    }
    loved
}

/// Rockbox `.scrobbler.log`.
///
/// Refuses rather than guesses when the log declares `#TZ/UNKNOWN` and no
/// timezone was supplied: assuming the server's zone would silently shift a whole
/// device's history, and the original offset can't be recovered afterwards.
async fn run_scrobbler_log(
    reg: &ImportRegistry,
    id: &str,
    db: &Database,
    uid: i64,
    text: &str,
    device_tz: Option<&str>,
) -> Result<(), String> {
    use crate::rockbox::{parse, peek_timezone, LogTimezone};

    let tz = match (peek_timezone(text), device_tz) {
        (LogTimezone::Utc, _) => chrono_tz::UTC,
        (LogTimezone::Unknown, Some(name)) => name.parse::<chrono_tz::Tz>()
            .map_err(|_| format!("Unknown timezone: {name}"))?,
        (LogTimezone::Unknown, None) => return Err(
            "This log's header says #TZ/UNKNOWN, so its timestamps are in the player's              local time. Pick the timezone the device was set to and import again."
                .to_string()),
    };

    let parsed = parse(text, tz);
    update(reg, id, |j| j.total = Some(parsed.entries.len()));

    let mut skips = 0usize;
    for e in &parsed.entries {
        let p = Play {
            source_id: source_id(e.timestamp, &e.artist, &e.title),
            source_name: "rockbox".into(),
            title: e.title.clone(),
            artist: e.artist.clone(),
            album: e.album.clone(),
            timestamp: e.timestamp.max(0) as u64,
            duration: e.duration,
            track_number: e.track_number,
            // Column 8 when the player wrote one — these skip MusicBrainz entirely.
            mbid_recording: e.mbid.clone(),
            mbid_release: None,
            mbid_artist: None,
            artists: None,
            mbid_release_group: None,
            caa_id: None,
            caa_release_mbid: None,
        };
        let outcome =
            db.import_listen_flagged(uid, &p, e.skipped).await.map_err(|e| e.to_string())?;
        let inserted = outcome == ImportOutcome::Inserted;
        if inserted && e.skipped {
            skips += 1;
        }
        update(reg, id, |j| {
            j.fetched += 1;
            if outcome == ImportOutcome::Enriched {
                j.enriched += 1;
            }
            // Skips are stored but are not listens, so they don't inflate the count.
            if inserted && !e.skipped {
                j.imported += 1;
            }
        });
    }

    let client = parsed.client.clone().unwrap_or_else(|| "Rockbox".into());
    let malformed = parsed.malformed;
    update(reg, id, |j| {
        j.state = "done".into();
        let mut parts = vec![imported_line(j.imported, j.enriched, &format!(" from {client}"))];
        if skips > 0 {
            parts.push(format!("{skips} skips recorded"));
        }
        if malformed > 0 {
            parts.push(format!("{malformed} unreadable rows ignored"));
        }
        j.status = parts.join(" · ");
    });
    Ok(())
}

/// ListenBrainz listens as JSON or JSONL.
///
/// Accepts a top-level array, `{ "listens": [...] }`, or — which is what the
/// export archive actually contains — one JSON object per line. JSONL is
/// detected by the whole-document parse failing, so a file that is valid JSON is
/// never re-read line by line.
fn parse_listens(text: &str) -> Result<Vec<Play>, String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(arr) = v
            .as_array()
            .cloned()
            .or_else(|| v["listens"].as_array().cloned())
            .or_else(|| v["scrobbles"].as_array().cloned())
        {
            return Ok(arr.iter().filter_map(listen_to_play).collect());
        }
        // A month with exactly one listen in it is a single JSON object, which
        // parses whole and would otherwise be rejected for not being an array.
        if let Some(p) = listen_to_play(&v) {
            return Ok(vec![p]);
        }
        return Err("Expected a JSON array of listens or a { listens: [...] } object".into());
    }

    let mut out = Vec::new();
    let mut lines = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        lines += 1;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(p) = listen_to_play(&v) {
                out.push(p);
            }
        }
    }
    if lines == 0 {
        return Err("Empty file".into());
    }
    if out.is_empty() {
        return Err("Not valid JSON, and no listens found reading it line by line".into());
    }
    Ok(out)
}

/// One listen object → a `Play`, keeping everything the export knows.
///
/// The archive's `mbid_mapping` is the whole reason this import is cheap:
/// recording/release/artist MBIDs and the cover-art ids come with it, so these
/// listens skip MusicBrainz enrichment *and* the artwork chain entirely. Losing
/// them would mean re-looking-up a decade of history one rate-limited request at
/// a time.
fn listen_to_play(l: &serde_json::Value) -> Option<Play> {
    let ts = l["listened_at"].as_i64().or_else(|| l["timestamp"].as_i64())?;
    let tm = if l["track_metadata"].is_object() { &l["track_metadata"] } else { l };
    let title = tm["track_name"].as_str().or(tm["title"].as_str())?.to_string();
    let artist = tm["artist_name"].as_str().or(tm["artist"].as_str())?.to_string();
    let album = tm["release_name"].as_str().or(tm["album"].as_str()).map(str::to_string);

    let mapping = &tm["mbid_mapping"];
    let info = &tm["additional_info"];
    let str_of = |v: &serde_json::Value| v.as_str().map(str::to_string);

    let mut p = play(
        ts,
        title,
        artist,
        album,
        str_of(&mapping["recording_mbid"])
            .or_else(|| str_of(&tm["mbid_recording"]))
            .or_else(|| str_of(&info["recording_mbid"])),
    );
    p.mbid_release = str_of(&mapping["release_mbid"]).or_else(|| str_of(&info["release_mbid"]));
    p.mbid_release_group = str_of(&mapping["release_group_mbid"]);
    p.mbid_artist = mapping["artist_mbids"]
        .as_array()
        .map(|a| a.iter().filter_map(|m| m.as_str().map(str::to_string)).collect::<Vec<_>>())
        .filter(|v: &Vec<String>| !v.is_empty());
    p.caa_id = mapping["caa_id"].as_i64();
    p.caa_release_mbid = str_of(&mapping["caa_release_mbid"]);
    p.duration = info["duration_ms"]
        .as_i64()
        .map(|ms| (ms / 1000).max(0) as u64)
        .or_else(|| info["duration"].as_i64().map(|s| s.max(0) as u64));
    p.track_number =
        info["tracknumber"].as_i64().or_else(|| info["track_number"].as_i64()).map(|n| n as i32);
    Some(p)
}

/// CSV: header row `title,artist,album,format,chain,mbid,time`.
fn parse_csv(text: &str) -> Vec<Play> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 || line.trim().is_empty() {
            continue;
        } // skip header
        let f = split_csv(line);
        if f.len() < 7 {
            continue;
        }
        let (title, artist, album, mbid, time) = (&f[0], &f[1], &f[2], &f[5], &f[6]);
        let Ok(ts) = time.trim().parse::<i64>() else {
            continue;
        };
        if title.is_empty() || artist.is_empty() {
            continue;
        }
        out.push(play(
            ts,
            title.clone(),
            artist.clone(),
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
            '"' if in_q && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_q = !in_q,
            ',' if !in_q => {
                fields.push(std::mem::take(&mut cur));
            }
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
        .route("/api/v1/import/jobs", get(active_jobs))
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
        let plays = parse_listens(js).unwrap();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].artist, "A");
        assert_eq!(plays[0].album.as_deref(), Some("R"));
    }

    /// One JSON object per line — the shape inside a ListenBrainz export.
    #[test]
    fn parse_jsonl_one_listen_per_line() {
        let jsonl = "{\"listened_at\":1700000000,\"track_metadata\":{\"track_name\":\"A\",\"artist_name\":\"X\"}}\n\
                     \n\
                     {\"listened_at\":1700000100,\"track_metadata\":{\"track_name\":\"B\",\"artist_name\":\"X\"}}\n";
        let plays = parse_listens(jsonl).unwrap();
        assert_eq!(plays.len(), 2);
        assert_eq!(plays[1].title, "B");
    }

    /// A single unreadable line must not lose the rest of the month.
    #[test]
    fn jsonl_tolerates_one_bad_line() {
        let jsonl = "{\"listened_at\":1,\"track_metadata\":{\"track_name\":\"A\",\"artist_name\":\"X\"}}\n\
                     {not json at all\n\
                     {\"listened_at\":2,\"track_metadata\":{\"track_name\":\"B\",\"artist_name\":\"X\"}}\n";
        assert_eq!(parse_listens(jsonl).unwrap().len(), 2);
        // But a file with nothing readable in it is an error, not silence.
        assert!(parse_listens("not json\nstill not json\n").is_err());
    }

    /// The export's `mbid_mapping` is the reason this import is cheap — every
    /// field dropped here is a rate-limited MusicBrainz lookup later.
    #[test]
    fn export_mbid_mapping_is_kept() {
        let line = r#"{"listened_at":1759320594,"track_metadata":{
            "track_name":"Up!","artist_name":"Shania Twain","release_name":"Up!",
            "mbid_mapping":{"caa_id":3827741944,
              "artist_mbids":["faabb55d-3c9e-4c23-8779-732ac2ee2c0d"],
              "release_mbid":"480bdf50-a119-49cb-b3d1-54e1e811b4fd",
              "recording_mbid":"1cecdf15-8596-4985-a865-6cbd624b483f",
              "caa_release_mbid":"480bdf50-a119-49cb-b3d1-54e1e811b4fd"},
            "additional_info":{"duration_ms":213376,"tracknumber":3}}}"#;
        let p = &parse_listens(line).unwrap()[0];
        assert_eq!(p.mbid_recording.as_deref(), Some("1cecdf15-8596-4985-a865-6cbd624b483f"));
        assert_eq!(p.mbid_release.as_deref(), Some("480bdf50-a119-49cb-b3d1-54e1e811b4fd"));
        assert_eq!(p.mbid_artist.as_deref().map(|v| v.len()), Some(1));
        assert_eq!(p.caa_id, Some(3827741944));
        assert_eq!(p.duration, Some(213));
        assert_eq!(p.track_number, Some(3));
    }

    /// A listen with no mapping at all still imports — most of anyone's older
    /// history is un-enriched, and dropping it would be the wrong trade.
    #[test]
    fn a_listen_with_no_mapping_still_imports() {
        let line = r#"{"listened_at":1,"track_metadata":{"track_name":"T","artist_name":"A"}}"#;
        let p = &parse_listens(line).unwrap()[0];
        assert!(p.mbid_recording.is_none() && p.caa_id.is_none());
        assert_eq!(p.title, "T");
    }
}
