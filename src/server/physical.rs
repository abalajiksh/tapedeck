//! Records and tapes: a shelf you catalogue, and sides you play.
//!
//! The unit here is a **side**, because that is the unit you physically handle —
//! you put one on, it plays through, you flip it. Playing a side writes one
//! ordinary `scrobbles` row per track, not one row for the side: every query in
//! this app counts listens, and a side-shaped row would be invisible to all of
//! them. The timestamps are *derived* from the side's running order, which is
//! the one thing here that is inferred rather than observed — see `play_side`.
//!
//! Alongside the listens, a `physical_plays` row records the physical act. That
//! is what stylus hours and a cassette counter are computed from, and it is
//! deliberately separate: a listen is about the music, this is about the object
//! and the equipment wearing out.

use axum::{
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

use super::auth::{AuthUser, ReadAuth, WriteAuth};
use super::models::ErrorResponse;
use super::now_playing::{NowPlaying, Progress};
use super::AppState;
use crate::db::PhysicalImage;
use crate::db::{PhysicalRelease, SidePlays};
use crate::metadata::release_lookup::ReleaseCandidate;
use crate::metadata::release_lookup::{self, PhysicalTrack};
use crate::models::{ListenDisposition, Play};
use crate::openapi::{BadRequest, Bytes, FileUpload, NotFound, Unauthorized};
use utoipa::{IntoParams, ToSchema};

/// Candidate pressings for what you are holding.
#[derive(Serialize, ToSchema)]
pub struct PressingLookup {
    /// Discogs first when a barcode and a credential are in hand, then
    /// MusicBrainz.
    pub candidates: Vec<ReleaseCandidate>,
    pub discogs_enabled: bool,
    /// A provider that failed, and why. The others still answered.
    pub notes: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DiscogsPressing {
    pub candidate: ReleaseCandidate,
}

#[derive(Serialize, ToSchema)]
pub struct Shelf {
    pub releases: Vec<PhysicalRelease>,
}

#[derive(Serialize, ToSchema)]
pub struct ShelfEntry {
    pub release: PhysicalRelease,
}

#[derive(Serialize, ToSchema)]
pub struct Shelved {
    pub id: i64,
    /// How many scans came back from the archive.
    pub images: usize,
    /// The shelf it went on.
    pub kind: String,
    /// Set when the release's own `format` overrode the `kind` asked for — a
    /// box of CDs looked up by barcode is filed as CDs whatever was picked.
    pub corrected_from: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ShelfItemSaved {
    /// `ok`.
    pub status: &'static str,
    /// The medium changed and the tracklist's sides were re-derived — a
    /// record's A/B break, or a disc's numbers.
    pub sides_rewritten: bool,
}

#[derive(Serialize, ToSchema)]
pub struct TracklistSaved {
    /// `ok`.
    pub status: &'static str,
    /// Running times that differ from what was stored.
    pub changed: usize,
    /// Listens already recorded from these sides that were corrected.
    pub listens_updated: u64,
    /// Whole-side plays whose recorded wear was brought back in line.
    pub plays_recomputed: u64,
}

/// What another pressing says the tracks run. Nothing is written.
#[derive(Serialize, ToSchema)]
pub struct DurationPreview {
    /// Every track already has a running time; there is nothing to ask.
    pub complete: bool,
    pub durations: Vec<DurationProposal>,
    /// The pressing the running times came from.
    pub source: Option<DurationSource>,
    /// The lookup could not be made — MusicBrainz was busy, throttling, or
    /// did not answer inside this endpoint's own 20-second deadline. **Still a
    /// 200**, deliberately: an upstream being slow is a normal outcome of
    /// asking, and a 5xx here is indistinguishable from a reverse proxy's own.
    /// An empty `durations` without this means the lookup succeeded and found
    /// nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DurationSource {
    pub title: String,
    pub artist: String,
    pub year: Option<i64>,
    pub format: Option<String>,
    pub country: Option<String>,
    pub mbid_release: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DurationsApplied {
    /// Tracks that gained a duration. One the shelf already had is never
    /// overwritten.
    pub filled: usize,
    /// Listens already recorded that were corrected.
    pub listens_updated: u64,
    /// Side plays whose recorded wear was brought back in line. **Whole-side
    /// plays only** — a partial play records how many tracks ran but never
    /// which, so its wear cannot be reconstructed and is left alone.
    pub plays_recomputed: u64,
}

#[derive(Serialize, ToSchema)]
pub struct ShelfDone {
    /// `ok`, `deleted` or `cleared`.
    pub status: &'static str,
}

#[derive(Serialize, ToSchema)]
pub struct Sides {
    /// Only sides that have been played, including any recorded under a label
    /// the tracklist no longer uses.
    pub sides: Vec<SidePlays>,
}

/// One scan or photograph of a pressing.
#[derive(Serialize, ToSchema)]
#[schema(as = ShelfImage)]
pub struct ImageView {
    pub id: i64,
    pub release_id: i64,
    /// `front`, `back`, `booklet`, `medium`, `spine`, … Free text rather than
    /// an enum: the archive's vocabulary grows, and a scan of something it has
    /// no word for is still a scan.
    pub kind: String,
    /// `caa`, `discogs` or `upload`.
    pub provider: String,
    /// What to render, whichever provider it came from.
    pub src: String,
    pub thumb: String,
    pub caption: Option<String>,
    pub position: i64,
    pub bytes: Option<i64>,
    pub mime: Option<String>,
    pub added_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ShelfImages {
    pub images: Vec<ImageView>,
}

#[derive(Serialize, ToSchema)]
pub struct ImagesFetched {
    /// Newly stored.
    pub stored: usize,
    /// Now held.
    pub total: usize,
    /// A provider with nothing to offer or that failed, and why.
    pub notes: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ScansStored {
    pub ids: Vec<i64>,
}

/// A scan of your own copy. Several `file` fields may be sent at once.
#[derive(ToSchema)]
#[allow(dead_code)] // describes a multipart body; never constructed
pub struct ScanUpload {
    /// JPEG, PNG, WebP, GIF, AVIF or TIFF, at most 24 MB.
    #[schema(value_type = String, format = Binary)]
    pub file: Vec<u8>,
    /// Defaults to `other`.
    pub kind: Option<String>,
    pub caption: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SideLogged {
    pub listens_added: usize,
    pub skipped: usize,
    pub tracks: usize,
    /// Running time credited as wear.
    pub duration_secs: i64,
    pub started_at: i64,
    /// The side to reach for next — only when this one ran to its end.
    pub next_side: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DeckReported {
    /// `ok`.
    pub status: &'static str,
    /// A different track from the last report.
    pub changed: bool,
}

#[derive(Serialize, ToSchema)]
pub struct Spin {
    /// Null when nothing is on the deck.
    pub spin: Option<SpinView>,
}

/// The side on the deck, with enough of the record to draw it.
#[derive(Serialize, ToSchema)]
pub struct SpinView {
    pub release_id: i64,
    pub kind: String,
    pub artist: String,
    pub title: String,
    pub artwork_url: Option<String>,
    pub mbid_release: Option<String>,
    pub side: String,
    pub started_at: i64,
    /// Null while running.
    pub paused_at: Option<i64>,
    /// Seconds spent stopped.
    pub paused_total: i64,
    /// Per-track `{position, started_at, skipped}`, as the deck wrote them.
    #[schema(value_type = Vec<Object>)]
    pub outcomes: serde_json::Value,
    pub chain_id: Option<i64>,
    pub equipment_id: Option<i64>,
    /// How far into the side this spin began — non-zero when the needle was
    /// dropped mid-side, or a tape resumed from where it was parked.
    pub start_secs: i64,
    pub updated_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct GearWear {
    /// To one decimal place.
    pub hours: f64,
    pub sides_played: i64,
}

/// Whether Discogs is configured, and never the credential.
#[derive(Serialize, ToSchema)]
pub struct DiscogsSettings {
    pub configured: bool,
    pub from_env: bool,
    pub stored: bool,
    /// Which credential shape is in use: `token`, `key_secret` or `none`.
    pub kind: &'static str,
}

#[derive(Serialize, ToSchema)]
pub struct DiscogsConfigured {
    pub configured: bool,
}

/// A rejection is a 200 with `ok: false`: the request succeeded and the answer
/// is no.
#[derive(Serialize, ToSchema)]
pub struct DiscogsCheck {
    pub ok: bool,
    /// Discogs' own words, which name the credential shape it rejected.
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct ShelfImported {
    /// Pressings added.
    pub releases: usize,
    /// Matched an existing pressing and left alone.
    pub already_on_the_shelf: usize,
    /// Entries that could not be read.
    pub skipped: usize,
    pub gear: usize,
    pub images: usize,
    /// Listed in the manifest but absent from the archive.
    pub missing_scans: usize,
    /// Sides played, restored.
    pub plays: usize,
    /// What a shelf export cannot carry.
    pub note: &'static str,
}

/// Where an admin-set Discogs token lives.
pub const DISCOGS_TOKEN_KEY: &str = "discogs_token";

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

/// Refuse a chain or a piece of gear that is not the caller's own.
///
/// Every shelf write that takes one in its body goes through this, because a
/// stored id is trusted everywhere downstream: `play_side` stamps it on every
/// listen the side writes, and from there it reaches chain and gear hours and
/// the Reel. An id naming another household member's chain would file your
/// listening under their gear, and nothing would ever error. `update_release`
/// always checked; `play_side` and `put_spin` did not — the output-binding bug
/// of v0.119.0 in a second place, found when the shelf was opened to tokens.
///
/// One copy, because the next shelf write that takes a chain should inherit
/// the check rather than have to know it exists.
async fn refuse_foreign_gear(
    state: &AppState,
    user_id: i64,
    chain_id: Option<i64>,
    equipment_id: Option<i64>,
) -> Option<axum::response::Response> {
    if let Some(cid) = chain_id {
        match state.db.get_chain(user_id, cid).await {
            Ok(Some(_)) => {}
            Ok(None) => return Some(err(400, "No such signal chain")),
            Err(e) => return Some(err(500, e.to_string())),
        }
    }
    if let Some(eid) = equipment_id {
        match state.db.get_equipment(user_id).await {
            Ok(gear) if gear.iter().any(|g| g.id == eid) => {}
            Ok(_) => return Some(err(400, "No such equipment")),
            Err(e) => return Some(err(500, e.to_string())),
        }
    }
    None
}

/// Where an admin-set consumer key/secret pair lives.
pub const DISCOGS_KEY_KEY: &str = "discogs_key";
pub const DISCOGS_SECRET_KEY: &str = "discogs_secret";

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// The configured Discogs credential, if there is one.
///
/// Database first, environment second — the env var is the deployment-time
/// answer (matching how the Apple Music token is supplied) and the setting is
/// the one an admin can paste in without a restart.
///
/// Two credential shapes are accepted because Discogs issues two. Registering
/// an application gives a **consumer key and secret**, which is what the
/// developer settings page leads with; generating a **personal access token**
/// is a separate button on the same page and easy to miss. Either works for
/// reading the public database, so refusing one of them would just be a way to
/// look broken.
///
/// A token wins when both are set: it is the narrower credential, scoped to one
/// account rather than to an application.
async fn discogs_auth(state: &AppState) -> Option<release_lookup::DiscogsAuth> {
    if let Ok(Some(t)) = state.db.get_app_setting(DISCOGS_TOKEN_KEY).await {
        if !t.trim().is_empty() {
            return Some(release_lookup::DiscogsAuth::Token(t.trim().to_string()));
        }
    }
    let stored_key = state.db.get_app_setting(DISCOGS_KEY_KEY).await.ok().flatten();
    let stored_secret = state.db.get_app_setting(DISCOGS_SECRET_KEY).await.ok().flatten();
    if let (Some(k), Some(s)) = (&stored_key, &stored_secret) {
        if !k.trim().is_empty() && !s.trim().is_empty() {
            return Some(release_lookup::DiscogsAuth::KeySecret {
                key: k.trim().to_string(),
                secret: s.trim().to_string(),
            });
        }
    }

    if let Some(t) = env_nonempty("TAPEDECK_DISCOGS_TOKEN") {
        return Some(release_lookup::DiscogsAuth::Token(t));
    }
    // The namespaced names first. `Consumer_Key`/`Consumer_Secret` are what
    // Discogs' own docs call them and what people therefore export, but they
    // are generic enough to belong to some other application on the same host —
    // so they are accepted as a fallback rather than looked for first.
    let key = env_nonempty("TAPEDECK_DISCOGS_KEY").or_else(|| env_nonempty("Consumer_Key"));
    let secret =
        env_nonempty("TAPEDECK_DISCOGS_SECRET").or_else(|| env_nonempty("Consumer_Secret"));
    match (key, secret) {
        (Some(key), Some(secret)) => Some(release_lookup::DiscogsAuth::KeySecret { key, secret }),
        _ => None,
    }
}

// ════════════════════════════════════════════════════════════
//  Looking up what you're holding
// ════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LookupQuery {
    /// Scanned off the sleeve or the spine.
    pub barcode: Option<String>,
    /// The label's catalogue number, which is on the spine when the barcode
    /// isn't — and is the only identifier on most pre-1980 pressings.
    pub catalog_number: Option<String>,
    /// With `release`, when there is neither.
    pub artist: Option<String>,
    pub release: Option<String>,
    /// `vinyl`, `cassette`, `cd` or `sacd`; narrows the format filter.
    pub kind: Option<String>,
}

// GET /api/v1/physical/lookup — find the pressing.
/// Resolve a barcode to a pressing.
///
/// Scope: `write` *(since v0.120)*, although it is a `GET`: it exists only to
/// add a record to the shelf, and it spends rate-limited Discogs and
/// MusicBrainz lookups doing it, so a token that can only read the shelf has
/// no use for it. `GET /api/v1/art/library` makes the same call.
///
/// Discogs first when configured — it catalogues *pressings*, so a barcode
/// usually resolves to the exact edition, often with real side letters that
/// MusicBrainz lacks. Falls back to MusicBrainz.
///
/// Barcodes may be sent with or without the printed spacing; the unspaced
/// form a scanner produces matches slightly **more** often.
#[utoipa::path(
    get,
    path = "/api/v1/physical/lookup",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(LookupQuery),
    responses(
        (status = 200, description = "OK.", body = PressingLookup),
        (status = 400, description = "Nothing to look up by.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
    )
)]
async fn lookup(
    State(state): State<Arc<AppState>>,
    WriteAuth { user: _user }: WriteAuth,
    Query(q): Query<LookupQuery>,
) -> impl IntoResponse {
    let barcode = q.barcode.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let catno = q.catalog_number.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let artist = q.artist.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let release = q.release.as_deref().map(str::trim).filter(|s| !s.is_empty());

    if barcode.is_none() && catno.is_none() && artist.is_none() && release.is_none() {
        return err(400, "Give a barcode, a catalogue number, or an artist and release");
    }

    let auth = discogs_auth(&state).await;
    let mut candidates = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // Discogs first when a barcode is in hand and a credential is configured:
    // it catalogues *pressings*, so a barcode usually lands on the exact
    // edition, where MusicBrainz will often only have "the album".
    if let (Some(auth), Some(barcode)) = (auth.as_ref(), barcode) {
        match release_lookup::discogs_search(auth, Some(barcode), None).await {
            Ok(mut hits) => candidates.append(&mut hits),
            Err(e) => {
                warn!("Discogs barcode lookup failed: {}", e);
                notes.push(format!("Discogs: {e}"));
            }
        }
    }

    // MusicBrainz needs no credential, so it always runs — it is the only path
    // on an instance with no Discogs token, which is the default state.
    let format_filter = match q.kind.as_deref() {
        Some("vinyl") => "format:Vinyl",
        Some("cassette") => "format:Cassette",
        // SACD is a CD-family format in MusicBrainz and most hybrid discs are
        // catalogued as both, so asking for either finds the pressing.
        Some("cd") => "(format:CD OR format:SACD)",
        Some("sacd") => "(format:SACD OR format:CD)",
        _ => "(format:Vinyl OR format:Cassette OR format:CD OR format:SACD)",
    };
    let mb_query = if let Some(b) = barcode {
        format!("barcode:{b}")
    } else if let Some(c) = catno {
        format!("catno:\"{c}\" AND {format_filter}")
    } else {
        let mut parts = Vec::new();
        if let Some(a) = artist {
            parts.push(format!("artist:\"{a}\""));
        }
        if let Some(r) = release {
            parts.push(format!("release:\"{r}\""));
        }
        parts.push(format_filter.to_string());
        parts.join(" AND ")
    };

    match release_lookup::musicbrainz_search(&mb_query, 5).await {
        Ok(mut hits) => candidates.append(&mut hits),
        Err(e) => {
            warn!("MusicBrainz lookup failed: {}", e);
            notes.push(format!("MusicBrainz: {e}"));
        }
    }

    (StatusCode::OK, Json(PressingLookup { candidates, discogs_enabled: auth.is_some(), notes }))
        .into_response()
}

// GET /api/v1/physical/discogs/:id — one Discogs release, with its tracklist.
//
// Separate from search because the search endpoint carries no tracklist:
// browsing ten results would otherwise be ten extra calls against a
// rate-limited API for nine releases nobody picked.
/// A Discogs release, with its tracklist positions.
///
/// Scope: `write` *(since v0.120)*, for the reason the barcode lookup is: it
/// is the second half of adding a record, and it spends a Discogs request.
#[utoipa::path(
    get,
    path = "/api/v1/physical/discogs/{id}",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = String, Path)),
    responses(
        (status = 200, description = "OK.", body = DiscogsPressing),
        (status = 400, description = "Discogs is not configured on this server.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 502, description = "Discogs failed to answer.", body = ErrorResponse),
    )
)]
async fn discogs_detail(
    State(state): State<Arc<AppState>>,
    WriteAuth { user: _user }: WriteAuth,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(auth) = discogs_auth(&state).await else {
        return err(400, "Discogs isn't configured on this server");
    };
    match release_lookup::discogs_release(&auth, &id).await {
        Ok(Some(c)) => (StatusCode::OK, Json(DiscogsPressing { candidate: c })).into_response(),
        Ok(None) => err(404, "No such Discogs release"),
        Err(e) => err(502, e.to_string()),
    }
}

// ════════════════════════════════════════════════════════════
//  The shelf
// ════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ShelfQuery {
    /// `vinyl`, `cassette`, `cd` or `sacd`. Anything else is ignored rather
    /// than rejected.
    pub kind: Option<String>,
}

/// Your shelf.
///
/// Scope: `read` *(since v0.120)*.
#[utoipa::path(
    get,
    path = "/api/v1/physical",
    tag = "Shelf",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(ShelfQuery),
    responses(
        (status = 200, description = "OK.", body = Shelf),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn list_shelf(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<ShelfQuery>,
) -> impl IntoResponse {
    let kind = q.kind.as_deref().filter(|k| matches!(*k, "vinyl" | "cassette" | "cd" | "sacd"));
    match state.db.list_physical(user.user_id, kind).await {
        Ok(releases) => (StatusCode::OK, Json(Shelf { releases })).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

/// One shelf item, with its sides.
///
/// Scope: `read` *(since v0.120)*.
#[utoipa::path(
    get,
    path = "/api/v1/physical/{id}",
    tag = "Shelf",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "OK.", body = ShelfEntry),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn get_one(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.get_physical(user.user_id, id).await {
        Ok(Some(release)) => (StatusCode::OK, Json(ShelfEntry { release })).into_response(),
        Ok(None) => err(404, "No such record"),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = NewShelfItem)]
pub struct NewRelease {
    /// `vinyl`, `cassette`, `cd` or `sacd`. A **disc has no sides**: its
    /// tracklist is one numbered unit per disc.
    pub kind: String,
    pub artist: String,
    pub title: String,
    pub year: Option<i64>,
    pub mbid_release: Option<String>,
    pub discogs_id: Option<String>,
    pub barcode: Option<String>,
    pub catalog_number: Option<String>,
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub tracks: Vec<PhysicalTrack>,
    pub notes: Option<String>,
    /// The medium the *release* declares — `"CD"`, `"12\" Vinyl"`, `"Hybrid
    /// SACD"` — as the lookup returned it. When present it **overrides `kind`**,
    /// because `kind` is what was picked before anyone looked at the record and
    /// this is what the record says it is.
    #[serde(default)]
    pub format: Option<String>,
    /// When you bought it, unix seconds. Optional — most of a shelf is
    /// catalogued long after the fact and guessing would be worse than blank.
    pub purchased_at: Option<i64>,
    /// How it came to be yours — `new`, `used`, `gift` or `inherited`.
    /// Optional, and absent means nobody said: a shelf catalogued before this
    /// existed must not be retroactively claimed as bought new.
    pub acquired_as: Option<String>,
    /// Where from — the shop, the record fair, the person who gave it to you.
    /// Free text, because that is the half that makes a thrift find a story
    /// rather than a flag.
    pub acquired_from: Option<String>,
    /// Pull the sleeve, labels and booklet from the Cover Art Archive as part
    /// of adding it. On by default. A failure does **not** fail the add, and
    /// it does nothing without `mbid_release`.
    #[serde(default = "yes")]
    #[schema(default = true)]
    pub fetch_images: bool,
}

fn yes() -> bool {
    true
}

/// Add a release to the shelf.
///
/// Scope: `write` *(since v0.120)*. Seed from a barcode lookup, or supply the tracklist yourself.
#[utoipa::path(
    post,
    path = "/api/v1/physical",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    request_body = NewRelease,
    responses(
        (status = 201, description = "Added.", body = Shelved),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
async fn add_release(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Json(body): Json<NewRelease>,
) -> impl IntoResponse {
    if !matches!(body.kind.as_str(), "vinyl" | "cassette" | "cd" | "sacd") {
        return err(400, "kind must be vinyl, cassette, cd or sacd");
    }
    if body.artist.trim().is_empty() || body.title.trim().is_empty() {
        return err(400, "An artist and a title are required");
    }
    let tracks = serde_json::to_string(&body.tracks).unwrap_or_else(|_| "[]".into());

    // What shelf it really goes on.
    //
    // The picker is what the user thought they were adding, chosen *before*
    // anyone looked at the record; `format` is what the release says it is, and
    // the release is the authority — the same reasoning `is_disc` already
    // follows for side splitting. A five-CD box set looked up by barcode could
    // otherwise be filed as vinyl with 76 tracks split into two sides, which is
    // exactly how one was.
    //
    // Silent when they agree, which is nearly always. When they disagree the
    // response says so, because quietly overriding what someone picked is its
    // own kind of wrong — they can move it back with the same picker.
    let declared = release_lookup::kind_from_format(body.format.as_deref());
    let kind = declared.unwrap_or(body.kind.as_str());
    let corrected = declared.filter(|k| *k != body.kind.as_str());

    let id = match state
        .db
        .create_physical(
            user.user_id,
            kind,
            body.artist.trim(),
            body.title.trim(),
            body.year,
            body.mbid_release.as_deref(),
            body.discogs_id.as_deref(),
            body.barcode.as_deref(),
            body.catalog_number.as_deref(),
            body.artwork_url.as_deref(),
            &tracks,
            body.notes.as_deref(),
            body.purchased_at,
            body.acquired_as.as_deref(),
            body.acquired_from.as_deref(),
        )
        .await
    {
        Ok(id) => id,
        Err(e) => return err(500, e.to_string()),
    };
    info!("💿 Added {} to the shelf: {} — {}", kind, body.artist, body.title);
    if let Some(k) = corrected {
        info!("   (filed as {k} rather than {} — the release says so)", body.kind);
    }

    // Awaited rather than spawned, and failing softly. Awaited because the next
    // thing the user does is open the record, and a gallery that fills in
    // thirty seconds later reads as broken; softly because a record you own is
    // still on the shelf whether or not an archive has scans of it.
    let mut images = 0usize;
    if body.fetch_images {
        if let Some(mbid) = body.mbid_release.as_deref() {
            match store_archive_images(&state, user.user_id, id, mbid).await {
                Ok(n) => images = n,
                Err(e) => warn!("Could not fetch cover art for {}: {}", mbid, e),
            }
        }
    }

    (
        StatusCode::CREATED,
        Json(Shelved {
            id,
            images,
            kind: kind.to_string(),
            // Only when it differs from what was asked for, so a client can say
            // "filed as CD — the release says so" rather than silently landing
            // it somewhere else.
            corrected_from: corrected.map(|_| body.kind.clone()),
        }),
    )
        .into_response()
}

/// The shelf-keeping fields. **An absent field is left alone and an explicit
/// `null` clears it** — a misspelled key is never read as a clear.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = ShelfItemUpdate)]
pub struct ReleaseUpdate {
    // `Option<Option<_>>` throughout, for the reason `bulk-chain` learned the
    // hard way: a bare `Option` reads a *misspelled* key as an explicit null,
    // so a typo would clear the field it was trying to set and answer 200.
    /// When you bought it, Unix seconds.
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<i64>)]
    pub purchased_at: Option<Option<i64>>,
    /// `new`, `used`, `gift` or `inherited`.
    //
    // Both clearable: a wrong answer is worse than a blank, and "I don't
    // actually remember" is a real thing to want to say afterwards.
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<String>)]
    pub acquired_as: Option<Option<String>>,
    /// The shop, the fair, the person.
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<String>)]
    pub acquired_from: Option<Option<String>>,
    /// What it is normally played on. Must be yours — a 400 otherwise.
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<i64>)]
    pub chain_id: Option<Option<i64>>,
    /// Must be yours — a 400 otherwise.
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<i64>)]
    pub equipment_id: Option<Option<i64>>,
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<String>)]
    pub notes: Option<Option<String>>,
    /// The variant of `chain_id` this pressing is normally played on — the
    /// cartridge you keep fitted for it.
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<i64>)]
    pub chain_variant_id: Option<Option<i64>>,
    /// Move it to another shelf: `vinyl`, `cassette`, `cd` or `sacd`. Not
    /// clearable. Changing it re-derives the tracklist's sides, because what a
    /// side *is* depends on the medium.
    //
    // A record has a break in it that `assign_sides` has to guess, and a disc
    // has none at all. Leaving the old split behind would have the shelf offer
    // to remind you to flip a CD.
    #[serde(default)]
    pub kind: Option<String>,
    /// The MusicBrainz release this pressing is, typed in by hand — also the
    /// cheapest way to ask for running times.
    //
    // Worth having for its own sake — a record catalogued from Discogs carries
    // no MusicBrainz id at all — and with one, the lookup is a single request
    // against a known release rather than a search that may walk several
    // candidates and be cut off by a proxy before it answers.
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<String>)]
    pub mbid_release: Option<Option<String>>,
}

/// The shelf's formats. Mirrors the CHECK on `physical_releases.kind` — a value
/// outside this set is a 400 rather than a constraint violation surfaced as a
/// 500, and the CHECK stays as the backstop.
pub const SHELF_KINDS: [&str; 4] = ["vinyl", "cassette", "cd", "sacd"];

/// Keep an explicit `null` distinguishable from an omitted field.
fn explicit<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

// PATCH /api/v1/physical/:id — the shelf-keeping fields.
/// Edit the shelf-keeping fields.
///
/// Scope: `write` *(since v0.120)*. When it was bought, what it is normally played through,
/// and the note beside it.
///
/// Every field distinguishes **absent from null**: omitting one leaves the
/// column alone, and an explicit `null` clears it. That is not decoration —
/// a bare optional reads a *misspelled* key as an explicit null, so a typo
/// would clear the chain it was trying to set and answer 200. Same shape and
/// the same reason as `default_chain_id` on `PATCH /admin/tokens/{id}`.
///
/// `chain_id` and `equipment_id` are checked against the caller's own rows
/// before they are stored.
///
/// `kind` is the exception to the absent/null rule — a format has no
/// "cleared" state, so a plain optional is correct. Changing it **rewrites
/// the tracklist's sides**, because what a side is depends on the medium: a
/// record has a break `assign_sides` has to place by playing time, and a
/// disc has none at all. The response reports `sides_rewritten` so a client
/// knows the tracklist it is holding is stale.
#[utoipa::path(
    patch,
    path = "/api/v1/physical/{id}",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = ReleaseUpdate,
    responses(
        (status = 200, description = "Saved.", body = ShelfItemSaved),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn update_release(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    Json(body): Json<ReleaseUpdate>,
) -> impl IntoResponse {
    // A chain or a piece of gear arriving in a body is checked against the
    // *owner's* rows before it is stored, the same rule ingest follows for a
    // token's default chain.
    if let Some(refused) = refuse_foreign_gear(
        &state,
        user.user_id,
        body.chain_id.flatten(),
        body.equipment_id.flatten(),
    )
    .await
    {
        return refused;
    }

    let kind = body.kind.as_deref().map(|k| k.trim().to_ascii_lowercase());
    if let Some(ref k) = kind {
        if !SHELF_KINDS.contains(&k.as_str()) {
            return err(400, format!("kind must be one of {}", SHELF_KINDS.join(", ")));
        }
    }

    // What the sides were before, so we only rewrite them when the medium
    // actually changed. Re-splitting a record that is still a record would
    // discard a hand-edited side break for nothing.
    let previous_kind = match state.db.get_physical(user.user_id, id).await {
        Ok(Some(r)) => r.kind,
        Ok(None) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
    };

    match state
        .db
        .update_physical(
            user.user_id,
            id,
            body.purchased_at,
            body.chain_id,
            body.equipment_id,
            body.notes.as_ref().map(|n| n.as_deref()),
            kind.as_deref(),
            body.chain_variant_id,
            body.mbid_release.as_ref().map(|m| m.as_deref()),
            body.acquired_as.as_ref().map(|v| v.as_deref()),
            body.acquired_from.as_ref().map(|v| v.as_deref()),
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
    }

    let mut resplit = false;
    if let Some(ref k) = kind {
        if *k != previous_kind {
            match respread_sides(&state, user.user_id, id, k).await {
                Ok(did) => resplit = did,
                Err(e) => return err(500, e),
            }
        }
    }

    (StatusCode::OK, Json(ShelfItemSaved { status: "ok", sides_rewritten: resplit }))
        .into_response()
}

/// Re-derive a tracklist's sides after the medium changed.
///
/// A disc plays start to finish, so its "sides" are disc numbers; a record or a
/// tape has a break that `assign_sides` places by playing time. Moving a
/// release between the two without redoing this leaves A/B lettering on a CD,
/// which is what makes the shelf offer to remind you to flip it.
///
/// The tracks keep their order and their titles — only `side` is rewritten,
/// because the order is a fact about the release and the split is a property of
/// the medium.
async fn respread_sides(
    state: &Arc<AppState>,
    user_id: i64,
    id: i64,
    kind: &str,
) -> Result<bool, String> {
    let Some(release) = state.db.get_physical(user_id, id).await.map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let mut tracks: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();
    if tracks.is_empty() {
        return Ok(false);
    }

    // `is_disc` is written against a *format* string; "sacd" and "cd" are the
    // two shelf kinds it recognises, and both are things you play through.
    let sides: Vec<String> = if release_lookup::is_disc(Some(kind)) {
        release_lookup::disc_sides(&[tracks.len()])
    } else {
        // Nothing states a side letter here — a tracklist that had one lost it
        // when it was filed as a disc — so this is the playing-time guess, the
        // same one the lookup makes. It is editable afterwards.
        let numbers: Vec<String> = (1..=tracks.len()).map(|n| n.to_string()).collect();
        let durations: Vec<Option<i64>> = tracks.iter().map(|t| t.duration_secs).collect();
        release_lookup::assign_sides(&numbers, &durations, 2)
    };

    // Position restarts within each side, which is what the side letter means.
    let mut seen: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for (t, side) in tracks.iter_mut().zip(sides) {
        let n = seen.entry(side.clone()).or_insert(0);
        *n += 1;
        t.position = *n;
        t.side = side;
    }

    let json = serde_json::to_string(&tracks).map_err(|e| e.to_string())?;
    state.db.set_physical_tracks(user_id, id, &json).await.map_err(|e| e.to_string())?;
    Ok(true)
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = Tracklist)]
pub struct TracksUpdate {
    /// The whole tracklist. Not empty, no side and position twice, and no
    /// running time of zero or less.
    #[schema(min_items = 1)]
    pub tracks: Vec<PhysicalTrack>,
}

// PUT /api/v1/physical/:id/tracks — save a corrected side split.
//
// The one edit that matters, because the split Tapedeck guessed is a guess:
// MusicBrainz only records a side letter when an editor entered one.
/// Replace the tracklist and side split.
///
/// Scope: `write` *(since v0.120)*. Sides are **not reliably available from any provider** —
/// MusicBrainz records a vinyl track number as "A1" only when an editor
/// entered one. When there is no explicit side letter the split is guessed
/// by **playing time**, not track count, because a mastering engineer
/// balances sides and a nine-minute closer would otherwise land on the
/// wrong one. It is still a guess, which is why this endpoint exists.
///
/// None of that applies to a **disc**. A CD or SACD plays start to finish,
/// so its tracks are numbered one unit per disc with no guessing at all —
/// splitting a CD by playing time would invent a break that is not there.
/// The medium's own format string decides which rule applies, not what was
/// searched for.
///
///
/// **It is also how a running time is typed in**, which for a pressing no
/// provider carries is the only way one is ever known. That is not
/// cosmetic: `scrobbles.duration` is what every hours readout sums and
/// `physical_plays.duration_secs` is what the stylus meter reads, so this
/// corrects **both** alongside the tracklist rather than leaving three
/// answers free to disagree. Whole-side plays only — a partial records how
/// many tracks ran and never which, so its wear cannot be reconstructed.
///
///
/// Unlike `POST /durations`, a value here **overwrites** a stored one. That
/// is the whole difference between the two: there the source is another
/// pressing and a hand-typed figure must survive it, here the source is the
/// person, and refusing their correction would make the field pointless.
#[utoipa::path(
    put,
    path = "/api/v1/physical/{id}/tracks",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = TracksUpdate,
    responses(
        (status = 200, description = "Saved.", body = TracklistSaved),
        (status = 400, description = "Empty tracklist, a duplicated side and position, or a running time of zero or less.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn set_tracks(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    Json(body): Json<TracksUpdate>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };

    // A whole-record replace of the one thing that makes a side playable, so
    // the obviously-wrong bodies are refused rather than stored. An empty list
    // is the sharpest: it would leave a record on the shelf that cannot be
    // played and offers no way back, and nothing legitimately asks for it.
    if body.tracks.is_empty() {
        return err(400, "A tracklist cannot be empty — re-add the record instead");
    }
    let mut seen = std::collections::HashSet::new();
    for t in &body.tracks {
        if !seen.insert((t.side.to_ascii_uppercase(), t.position)) {
            return err(400, format!("Two tracks share side {} position {}", t.side, t.position));
        }
        if t.duration_secs.is_some_and(|d| d <= 0) {
            return err(400, format!("'{}' has a running time of zero or less", t.title));
        }
    }

    // What actually changed, so the listens already recorded from these sides
    // can be corrected with it. **Every** difference counts here, including one
    // that overwrites a stored value — the opposite of `apply_durations`, and
    // deliberately: there the source is another pressing and a hand-typed
    // figure must survive it, here the source *is* the person, and refusing
    // their correction would make the field pointless.
    let before: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();
    let listen_updates: Vec<(String, i64, i64)> = body
        .tracks
        .iter()
        .filter_map(|t| {
            let secs = t.duration_secs?;
            let was = before
                .iter()
                .find(|b| b.position == t.position && b.side.eq_ignore_ascii_case(&t.side))
                .and_then(|b| b.duration_secs);
            (was != Some(secs)).then(|| (t.side.clone(), t.position, secs))
        })
        .collect();

    let json = serde_json::to_string(&body.tracks).unwrap_or_else(|_| "[]".into());
    match state.db.set_physical_tracks(user.user_id, id, &json).await {
        Ok(true) => {}
        Ok(false) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
    }

    // The half that makes this worth having. A running time typed in here is
    // not decoration: `scrobbles.duration` is what every hours readout sums and
    // `physical_plays.duration_secs` is what the stylus meter reads, and until
    // v0.75.0 nothing corrected either — so a record whose durations arrived
    // late contributed nothing to gear hours for ever.
    let (listens, sides_touched) =
        reconcile_durations(&state, user.user_id, id, &body.tracks, &listen_updates).await;

    (
        StatusCode::OK,
        Json(TracklistSaved {
            status: "ok",
            changed: listen_updates.len(),
            listens_updated: listens,
            plays_recomputed: sides_touched,
        }),
    )
        .into_response()
}

/// Push corrected running times through to the listens and the recorded wear.
///
/// Shared by the lookup's apply path and by a hand edit, because correcting one
/// and not the other leaves the side card and the stylus meter disagreeing
/// about the same play — and two copies of that rule would drift.
///
/// Neither failure is fatal to the caller: the tracklist is already saved and
/// correct, and a reconciliation that could not run is recoverable by editing
/// again, where a 500 would suggest the edit itself had not landed.
async fn reconcile_durations(
    state: &AppState,
    user_id: i64,
    id: i64,
    tracks: &[PhysicalTrack],
    listen_updates: &[(String, i64, i64)],
) -> (u64, u64) {
    let listens = match state.db.set_physical_listen_durations(user_id, id, listen_updates).await {
        Ok(n) => n,
        Err(e) => {
            warn!("Saved the tracklist but could not correct its listens: {}", e);
            0
        }
    };

    // Whole-side plays only: a partial records how many tracks ran and never
    // which, so its wear cannot be reconstructed and is left alone rather than
    // inflated.
    let mut sides_touched = 0u64;
    for side in tracks.iter().map(|t| t.side.clone()).collect::<std::collections::BTreeSet<_>>() {
        let on: Vec<&PhysicalTrack> = tracks.iter().filter(|t| t.side == side).collect();
        if !on.iter().all(|t| t.duration_secs.is_some()) {
            continue;
        }
        let secs: i64 = on.iter().filter_map(|t| t.duration_secs).sum();
        match state.db.recompute_side_play_wear(user_id, id, &side, on.len() as i64, secs).await {
            Ok(n) => sides_touched += n,
            Err(e) => warn!("Could not recompute the wear on side {}: {}", side, e),
        }
    }
    (listens, sides_touched)
}

/// How long the running-time lookup may take before this endpoint gives up and
/// says so itself.
///
/// Chosen to sit well inside any reverse proxy's patience rather than to be
/// generous: the worst thing this endpoint can do is fail to answer, because
/// then the proxy answers instead and its 502 says nothing about running times.
const LOOKUP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// One track's proposed running time, as previewed and as applied.
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct DurationProposal {
    pub side: String,
    pub position: i64,
    pub title: String,
    /// What the shelf holds now.
    pub current: Option<i64>,
    /// What the looked-up pressing says. Null where nothing matched — never a
    /// guess.
    pub proposed: Option<i64>,
}

/// The proposals exactly as the preview showed them.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ApplyDurations {
    pub durations: Vec<DurationProposal>,
}

// GET /api/v1/physical/:id/durations — what another pressing says the tracks run.
//
// **Preview only.** Nothing is written, and the apply below takes back exactly
// what was shown rather than searching again — a preview that says one thing
// and applies another is worse than no preview, which is the rule the metadata
// sanitiser already follows. It matters more here than usual: a wrong duration
// is invisible the moment it is stored, and there is no way afterwards to tell
// which figures were guessed.
//
// The lookup is a **search**, not a join. A record catalogued from Discogs
// often carries no MusicBrainz id at all — Fallen does not — so the barcode is
// the key, and whatever comes back is a *different pressing* that may be
// ordered differently or carry bonus tracks. `match_durations` therefore
// matches on titles; see its own note for why position matching is wrong.
/// What another pressing says this record's tracks run.
///
/// **Preview only — nothing is written.** Session-only.
///
/// Exists because a record catalogued from Discogs often has no running
/// times at all, which used to leave the deck showing three minutes a track
/// as though it were a length. The lookup is a *search* (MusicBrainz id if
/// the record has one, else barcode, else artist and title), so whatever
/// comes back is a **different pressing** and may be ordered differently or
/// carry bonus tracks — tracks are therefore matched by title, never by
/// position.
///
/// `complete: true` means every track already has a duration and there is
/// nothing to do. An empty `durations` with a null `source` means nothing
/// with running times was found, which is a normal answer.
#[utoipa::path(
    get,
    path = "/api/v1/physical/{id}/durations",
    tag = "Shelf",
    security(("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "OK.", body = DurationPreview),
        (status = 400, description = "That record has no tracklist.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such record.", body = ErrorResponse),
    )
)]
async fn preview_durations(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };
    let stored: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();
    if stored.is_empty() {
        return err(400, "That record has no tracklist to fill in");
    }
    if stored.iter().all(|t| t.duration_secs.is_some()) {
        return (
            StatusCode::OK,
            Json(DurationPreview {
                complete: true,
                durations: Vec::new(),
                source: None,
                unavailable: None,
                note: None,
            }),
        )
            .into_response();
    }

    // **This endpoint answers within its own deadline, always.**
    //
    // It reaches a third party over the network, and MusicBrainz is slow at
    // unpredictable moments. Held open long enough, the request stops being a
    // slow response and becomes *no response*: the reverse proxy in front of the
    // instance gives up and serves its own 502, which is unreadable HTML with
    // nothing in it about running times. That is what got reported, and it is
    // the third time here that a request outliving a proxy has been mistaken for
    // a fault in the thing it was asking about.
    //
    // Best key first, and each is cheaper than the last resort. An MBID names
    // the pressing outright and is one request. A barcode names the edition, so
    // there is exactly one answer worth fetching. Only the name search may walk
    // further, and it stops at the first pressing that can actually answer.
    let lookup = async {
        if let Some(mbid) = release.mbid_release.as_deref().filter(|m| !m.trim().is_empty()) {
            release_lookup::musicbrainz_release(mbid).await
        } else if let Some(bc) = release.barcode.as_deref().filter(|b| !b.trim().is_empty()) {
            let q = format!("barcode:{}", bc.replace([' ', '-'], ""));
            release_lookup::musicbrainz_first_with_durations(&q, 1).await
        } else {
            let q = format!(
                "artist:\"{}\" AND release:\"{}\"",
                release.artist.replace('"', ""),
                release.title.replace('"', "")
            );
            release_lookup::musicbrainz_first_with_durations(&q, 3).await
        }
    };

    // An upstream that is busy or slow is a **normal outcome of asking**, not a
    // failure of this server — so it comes back 200 with `unavailable` set,
    // rather than as a 5xx a proxy is free to replace with its own error page.
    // The distinction still travels: "asked, and there is nothing" and "could
    // not ask" remain different answers, carried in the body where nothing
    // between here and the browser can eat them.
    let unavailable = |note: String| {
        (
            StatusCode::OK,
            Json(DurationPreview {
                complete: false,
                durations: Vec::new(),
                source: None,
                unavailable: Some(true),
                note: Some(note),
            }),
        )
            .into_response()
    };

    let source = match tokio::time::timeout(LOOKUP_DEADLINE, lookup).await {
        Ok(Ok(Some(c))) => c,
        Ok(Ok(None)) => {
            return (
                StatusCode::OK,
                Json(DurationPreview {
                    complete: false,
                    durations: Vec::new(),
                    source: None,
                    unavailable: None,
                    note: Some("Nothing with running times was found for this record.".into()),
                }),
            )
                .into_response();
        }
        Ok(Err(e)) => return unavailable(format!("MusicBrainz: {e}")),
        Err(_) => {
            return unavailable(
                "MusicBrainz didn't answer in time. It throttles hard at busy \
                 moments — try again in a minute."
                    .into(),
            )
        }
    };

    let matched = release_lookup::match_durations(&stored, &source.tracks);
    let durations: Vec<DurationProposal> = stored
        .iter()
        .zip(matched)
        .map(|(t, proposed)| DurationProposal {
            side: t.side.clone(),
            position: t.position,
            title: t.title.clone(),
            current: t.duration_secs,
            proposed,
        })
        .collect();

    (
        StatusCode::OK,
        Json(DurationPreview {
            complete: false,
            durations,
            source: Some(DurationSource {
                title: source.title,
                artist: source.artist,
                year: source.year,
                format: source.format,
                country: source.country,
                mbid_release: source.mbid_release,
            }),
            unavailable: None,
            note: None,
        }),
    )
        .into_response()
}

// POST /api/v1/physical/:id/durations — write the running times that were shown.
//
// Takes the proposals back rather than re-running the search, so what lands is
// what was on the screen. Only fills a duration the shelf does **not** already
// have: a running time somebody typed in, or one an earlier lookup got right,
// is not something a later fetch may quietly overwrite.
//
// Then the listens already recorded from those sides are given the same
// figures, because every hours readout in the application derives from
// `scrobbles.duration` — filling the tracklist alone would leave the shelf
// saying one thing and the stylus meter another.
/// Write the running times that were previewed.
///
/// Session-only. Takes the proposals **back** rather than searching again,
/// so what lands is what was on the screen — a preview that applies
/// something else is worse than no preview.
///
/// Only fills a duration the record does not already have: one that was
/// typed in, or that an earlier lookup got right, is never overwritten by a
/// later fetch.
///
/// The listens already recorded from those sides are given the same figures,
/// because every hours readout derives from `scrobbles.duration`. Their
/// **timestamps are not touched** — `physical_plays` records when a side
/// started and nothing about when it ended, so where the tracks fell is
/// unrecoverable, and re-spacing them would assume the side ran
/// uninterrupted.
#[utoipa::path(
    post,
    path = "/api/v1/physical/{id}/durations",
    tag = "Shelf",
    security(("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = ApplyDurations,
    responses(
        (status = 200, description = "OK.", body = DurationsApplied),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such record.", body = ErrorResponse),
    )
)]
async fn apply_durations(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<ApplyDurations>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };
    let mut tracks: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();

    let mut filled = 0usize;
    let mut listen_updates: Vec<(String, i64, i64)> = Vec::new();
    for p in &body.durations {
        let Some(secs) = p.proposed.filter(|s| *s > 0) else { continue };
        let Some(t) = tracks
            .iter_mut()
            .find(|t| t.position == p.position && t.side.eq_ignore_ascii_case(&p.side))
        else {
            continue;
        };
        if t.duration_secs.is_some() {
            continue;
        }
        t.duration_secs = Some(secs);
        filled += 1;
        listen_updates.push((t.side.clone(), t.position, secs));
    }

    // Only writes when something changed, but the reconciliation below runs
    // either way: a record whose tracklist was filled in before the wear was
    // being corrected would otherwise have no way back into line.
    if filled > 0 {
        let json = serde_json::to_string(&tracks).unwrap_or_else(|_| "[]".into());
        if let Err(e) = state.db.set_physical_tracks(user.user_id, id, &json).await {
            return err(500, e.to_string());
        }
    }
    // The listens and the recorded wear, through the same path a hand edit
    // takes — see `reconcile_durations`. It runs even when nothing was filled:
    // a record whose tracklist was completed before the wear was being
    // corrected would otherwise have no way back into line.
    let (listens, sides_touched) =
        reconcile_durations(&state, user.user_id, id, &tracks, &listen_updates).await;

    (
        StatusCode::OK,
        Json(DurationsApplied {
            filled,
            listens_updated: listens,
            plays_recomputed: sides_touched,
        }),
    )
        .into_response()
}

/// Remove a shelf item.
///
/// Session-only, deliberately, although the rest of the shelf takes a
/// `write` token: its plays and its images go with it, and any scan files the
/// user uploaded are removed from disk — nothing else knows those files
/// exist, so leaving them would orphan them permanently. Scans are not in
/// `GET /api/v1/backup`, so this is the one shelf operation a lost phone could
/// make unrecoverable, which is the line `DELETE /api/v1/scrobbles` sits on.
#[utoipa::path(
    delete,
    path = "/api/v1/physical/{id}",
    tag = "Shelf",
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "Deleted.", body = ShelfDone),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn delete_release(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_physical(user.user_id, id).await {
        Ok(Some(files)) => {
            // The rows are already gone, so a file that refuses to delete is
            // litter rather than a failure — reporting a 500 here would say the
            // record is still on the shelf, which it is not.
            for f in files {
                if let Err(e) = tokio::fs::remove_file(state.uploads_dir.join(&f)).await {
                    warn!("Could not remove the scan at {}: {}", f, e);
                }
            }
            (StatusCode::OK, Json(ShelfDone { status: "deleted" })).into_response()
        }
        Ok(None) => err(404, "No such record"),
        Err(e) => err(500, e.to_string()),
    }
}

// ════════════════════════════════════════════════════════════
//  Everything else in the sleeve
// ════════════════════════════════════════════════════════════

/// What the browser is told about one image.
///
/// Built by hand rather than serialised straight off the row so that
/// `file_path` cannot reach the page: an uploaded scan is addressed by a
/// Tapedeck URL, and the server's own layout is nobody's business. The row type
/// also skips the field, so this is belt and braces on the thing that matters.
fn image_payload(img: &PhysicalImage) -> ImageView {
    let own = format!("/api/v1/physical/images/{}/file", img.id);
    ImageView {
        id: img.id,
        release_id: img.release_id,
        kind: img.kind.clone(),
        provider: img.provider.clone(),
        // One field the page renders, whichever provenance it came from.
        src: img.url.clone().unwrap_or_else(|| own.clone()),
        // A remote provider's thumbnail, or the full file for an upload —
        // scans are the user's own and there is nothing else to serve.
        thumb: img.thumb_url.clone().or_else(|| img.url.clone()).unwrap_or(own),
        caption: img.caption.clone(),
        position: img.position,
        bytes: img.bytes,
        mime: img.mime.clone(),
        added_at: img.added_at,
    }
}

// GET /api/v1/physical/:id/sides — how often each side has been played.
//
// A shelf item's totals answer "how much has this record been played"; this
// answers the question the object actually raises, which is that a record has
// a side you reach for and a side you rarely turn over.
//
// Reports only the sides that have plays. The page holds the tracklist and
// zero-fills from it, which is what lets a side that has never been played
// say so — and it is also how a side recorded under a *different* label shows
// up: re-filing a release rewrites its sides (a vinyl A/B becomes a CD's 1),
// and the plays keep the letter they were recorded with. Dropping those here
// would leave the sides summing to less than the release's own total with
// nothing to say why.
/// How often each side has been played, and for how long.
///
/// Scope: `read` *(since v0.120)*. A record has a side you reach for and a side you rarely
/// turn over, and the release's own totals cannot say which is which.
///
/// Everything here is derived from the sides played on read — never a
/// counter — so it cannot drift from the plays it counts, and deleting a
/// play takes its wear back with it. `secs` is the running time of the
/// tracks that actually played, which is the same figure the stylus is
/// charged for: a side started at track four counts four tracks' worth.
/// It is nominal rather than wall clock.
///
/// **Only sides with plays are returned.** A side that has never been
/// played is not an entry with zeros — the caller holds the tracklist and
/// fills those in. That also means a side may appear that is not on the
/// tracklist at all: re-filing a release rewrites its sides (a vinyl A/B
/// becomes a CD's 1) while its plays keep the label they were recorded
/// with, and dropping those would leave the sides summing to less than the
/// release total with nothing to say why.
#[utoipa::path(
    get,
    path = "/api/v1/physical/{id}/sides",
    tag = "Shelf",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "OK.", body = Sides),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such record.", body = ErrorResponse),
    )
)]
async fn side_plays(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Ownership before the stats: scoping the query alone would answer an
    // empty list for somebody else's record, which reads as "never played"
    // rather than as "not yours".
    match state.db.get_physical(user.user_id, id).await {
        Ok(None) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
        Ok(Some(_)) => {}
    }
    match state.db.side_plays(user.user_id, id).await {
        Ok(sides) => (StatusCode::OK, Json(Sides { sides })).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

/// Every scan held for a pressing.
///
/// Scope: `read` *(since v0.120)*. Sleeves, labels, obi strips and booklet pages — the parts
/// of a record you actually hold, which is why they are rows rather than
/// one more `artwork_url` column. The Cover Art Archive carries 29 for a
/// typical CD, 19 of them booklet pages.
///
/// Each entry carries a single `src` and `thumb` whichever provenance it
/// came from: a provider's image is its own URL, an uploaded scan is a
/// Tapedeck URL that checks who is asking. The server's own file layout is
/// never in the response.
#[utoipa::path(
    get,
    path = "/api/v1/physical/{id}/images",
    tag = "Shelf",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "OK.", body = ShelfImages),
        (status = 401, response = Unauthorized),
    )
)]
async fn list_images(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.list_physical_images(user.user_id, id).await {
        Ok(rows) => {
            let images = rows.iter().map(image_payload).collect();
            (StatusCode::OK, Json(ShelfImages { images })).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

/// Pull everything the Cover Art Archive holds for a release and store it.
///
/// Idempotent by the archive's own image id, so re-running this on a release
/// that has gained a scan adds the one and leaves the other twenty-eight alone.
/// The error is flattened to a `String` the moment it arrives, and that is not
/// cosmetic: a `Box<dyn Error>` is not `Send`, and holding one across the
/// awaits below makes the whole handler futures fail axum's `Handler` bound
/// with an error message that names neither.
async fn store_archive_images(
    state: &AppState,
    user_id: i64,
    release_id: i64,
    mbid: &str,
) -> Result<usize, String> {
    let images = release_lookup::cover_art_archive(mbid).await.map_err(|e| e.to_string())?;
    let mut n = 0;
    for img in &images {
        if state
            .db
            .add_physical_image(
                user_id,
                release_id,
                &img.kind,
                "caa",
                Some(&img.url),
                img.thumb_url.as_deref(),
                None,
                None,
                None,
                img.caption.as_deref(),
                img.position,
                Some(&img.id),
            )
            .await
            .is_ok()
        {
            n += 1;
        }
    }
    Ok(n)
}

// POST /api/v1/physical/:id/images/fetch — ask the providers again.
//
// Runs when the user asks rather than on a clock: the archive is not going to
// change under a record that has been on a shelf for thirty years, and the one
// case that does — an editor uploading the booklet you were missing — is
// exactly the case where the user knows and Tapedeck cannot.
/// Pull the sleeve from the Cover Art Archive.
///
/// Scope: `write` *(since v0.120)*. Runs when the user asks rather than on a clock: the
/// archive is not going to change under a record that has been on a shelf
/// for thirty years, and the one case that does — an editor uploading the
/// booklet you were missing — is exactly the case where the user knows and
/// Tapedeck cannot.
///
/// **Idempotent** on the provider's own image id, so re-running this adds
/// what is new and leaves the rest alone rather than stacking a second copy
/// of all nineteen booklet pages on top of the first.
///
/// Discogs runs second and only when a credential is configured; it says
/// only "primary" or "secondary" about an image, so everything but the
/// cover lands in `other` for the user to file. A pressing with no
/// MusicBrainz id is **not** an error — it comes back with a note saying
/// there is nothing to look it up by.
#[utoipa::path(
    post,
    path = "/api/v1/physical/{id}/images/fetch",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "OK.", body = ImagesFetched),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn fetch_images(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };

    let mut added = 0usize;
    let mut notes: Vec<String> = Vec::new();

    match release.mbid_release.as_deref() {
        Some(mbid) => match store_archive_images(&state, user.user_id, id, mbid).await {
            Ok(n) => added += n,
            Err(e) => notes.push(format!("Cover Art Archive: {e}")),
        },
        // Not an error, and worth saying rather than showing an empty gallery:
        // a record typed in by hand has no MusicBrainz id to ask about.
        None => notes.push(
            "This pressing has no MusicBrainz id, so there is nothing to look its sleeve up by. \
             Uploaded scans still work."
                .into(),
        ),
    }

    // Discogs second and only if configured, because it serves images only to
    // an authenticated caller and says nothing useful about what they *are* —
    // "secondary" covers a back cover and a booklet page equally.
    if let (Some(auth), Some(did)) = (discogs_auth(&state).await, release.discogs_id.as_deref()) {
        match release_lookup::discogs_images(&auth, did).await.map_err(|e| e.to_string()) {
            Ok(images) => {
                for img in &images {
                    if state
                        .db
                        .add_physical_image(
                            user.user_id,
                            id,
                            &img.kind,
                            "discogs",
                            Some(&img.url),
                            img.thumb_url.as_deref(),
                            None,
                            None,
                            None,
                            None,
                            // After the archive's, so a gallery holding both
                            // does not interleave them.
                            1000 + img.position,
                            Some(&img.id),
                        )
                        .await
                        .is_ok()
                    {
                        added += 1;
                    }
                }
            }
            Err(e) => notes.push(format!("Discogs: {e}")),
        }
    }

    let total = state.db.list_physical_images(user.user_id, id).await.map(|r| r.len()).unwrap_or(0);
    (StatusCode::OK, Json(ImagesFetched { stored: added, total, notes })).into_response()
}

/// The largest a single scan may be. A 600dpi A4 page is a few megabytes; this
/// leaves room for a gatefold without letting one upload fill the disk.
const MAX_SCAN_BYTES: usize = 24 * 1024 * 1024;

/// Extension for a declared image type. The list is exhaustive on purpose —
/// the extension is part of a path this server writes, so it is chosen from a
/// fixed set here rather than taken from the client's filename.
pub(super) fn image_ext(mime: &str) -> Option<&'static str> {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/png" => Some("png"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/avif" => Some("avif"),
        "image/tiff" => Some("tif"),
        _ => None,
    }
}

// POST /api/v1/physical/:id/images — upload a scan of your own copy.
//
// The one image source that is not somebody else's database. A pressing
// nobody has photographed, a sleeve with your own handwriting on it, the
// receipt tucked inside — none of that exists anywhere but here.
/// Upload scans of your own copy.
///
/// Scope: `write` *(since v0.120)*. `multipart/form-data`: any number of file fields, plus
/// optional `kind` and `caption` text fields that apply to all of them —
/// which is what makes uploading a booklet in one go work, since the files
/// land in the order they are sent.
///
/// The declared content type decides the stored extension; a filename from
/// the client never reaches the path. Max 24 MB per image.
///
/// Uploads are files on the server rather than blobs in the database, so
/// `GET /api/v1/backup` does **not** carry them — `GET /api/v1/physical/export`
/// is what does.
#[utoipa::path(
    post,
    path = "/api/v1/physical/{id}/images",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body(content = ScanUpload, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Stored.", body = ScansStored),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 413, description = "Larger than 24 MB.", body = ErrorResponse),
        (status = 415, description = "Not an image type this server stores.", body = ErrorResponse),
    )
)]
async fn upload_image(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let Ok(Some(_)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };

    let mut kind = "other".to_string();
    let mut caption: Option<String> = None;
    let mut files: Vec<(Vec<u8>, String)> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "kind" => {
                if let Ok(v) = field.text().await {
                    let v = v.trim().to_lowercase();
                    if !v.is_empty() {
                        kind = v;
                    }
                }
            }
            "caption" => {
                caption =
                    field.text().await.ok().map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
            }
            _ => {
                // The declared type decides the extension, and a filename from
                // the client never touches the path.
                let declared = field.content_type().unwrap_or("").to_string();
                let named = field.file_name().unwrap_or("").to_string();
                let mime = if image_ext(&declared).is_some() {
                    declared
                } else {
                    mime_guess::from_path(&named).first_raw().unwrap_or("").to_string()
                };
                if let Ok(b) = field.bytes().await {
                    files.push((b.to_vec(), mime));
                }
            }
        }
    }

    if files.is_empty() {
        return err(400, "No image in the request");
    }

    // Where the next one goes. A booklet is uploaded a page at a time, and
    // appending keeps that order without asking the user to number them.
    let mut position = state
        .db
        .list_physical_images(user.user_id, id)
        .await
        .map(|r| r.iter().map(|i| i.position).max().unwrap_or(-1) + 1)
        .unwrap_or(0);

    let dir = state.uploads_dir.join("shelf").join(user.user_id.to_string()).join(id.to_string());
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return err(500, format!("Could not write to the uploads directory: {e}"));
    }

    let mut stored = Vec::new();
    for (bytes, mime) in files {
        if bytes.len() > MAX_SCAN_BYTES {
            return err(413, "That scan is larger than 24 MB");
        }
        let Some(ext) = image_ext(&mime) else {
            return err(415, "Only JPEG, PNG, WebP, GIF, AVIF and TIFF images can be uploaded");
        };
        let name = format!("{}.{}", random_name(), ext);
        // Stored relative to the uploads directory, so moving that directory
        // (or restoring it beside a different install) does not orphan a row.
        let rel = format!("shelf/{}/{}/{}", user.user_id, id, name);
        if let Err(e) = tokio::fs::write(dir.join(&name), &bytes).await {
            return err(500, format!("Could not save the scan: {e}"));
        }
        match state
            .db
            .add_physical_image(
                user.user_id,
                id,
                &kind,
                "upload",
                None,
                None,
                Some(&rel),
                Some(&mime),
                Some(bytes.len() as i64),
                caption.as_deref(),
                position,
                None,
            )
            .await
        {
            Ok(new_id) => {
                stored.push(new_id);
                position += 1;
            }
            Err(e) => {
                // The row is what makes the file reachable, so a file with no
                // row is unreferenced bytes — clean it up rather than leaving it.
                let _ = tokio::fs::remove_file(dir.join(&name)).await;
                return err(500, e.to_string());
            }
        }
    }

    info!("🖼️ Stored {} scan(s) for shelf item {}", stored.len(), id);
    (StatusCode::CREATED, Json(ScansStored { ids: stored })).into_response()
}

pub(super) fn random_name() -> String {
    use rand::Rng;
    let raw: [u8; 12] = rand::rng().random();
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

// GET /api/v1/physical/images/:id/file — serve an uploaded scan.
//
// Scoped to the caller by `(id, user_id)`, the same rule the artwork proxy
// keeps: these are photographs of someone's own possessions and ids must not
// be walkable. Cached hard — a scan never changes once uploaded — but
// `private`, because it is per-user and a shared cache holding it would serve
// it to the next person through the proxy.
/// An uploaded scan's bytes.
///
/// Scope: `read` *(since v0.120)*, and scoped to the caller by `(id, user_id)` — these are
/// photographs of someone's own possessions and ids must not be walkable,
/// the same rule the artwork proxy keeps.
///
/// Cached hard, because a scan never changes once uploaded, but `private`:
/// it is per-user, and a shared cache holding it would serve it to the next
/// person through the proxy. A provider-hosted image 404s here — it has a
/// URL of its own and this server is not in front of it.
#[utoipa::path(
    get,
    path = "/api/v1/physical/images/{id}/file",
    tag = "Shelf",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "The image.", body = Bytes, content_type = "image/*"),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn image_file(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let Ok(Some(img)) = state.db.get_physical_image(user.user_id, id).await else {
        return err(404, "No such image");
    };
    let Some(rel) = img.file_path else {
        return err(404, "That image lives at a provider, not on this server");
    };
    match tokio::fs::read(state.uploads_dir.join(&rel)).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    img.mime.unwrap_or_else(|| "application/octet-stream".into()),
                ),
                (header::CACHE_CONTROL, "private, max-age=31536000, immutable".into()),
            ],
            Body::from(bytes),
        )
            .into_response(),
        Err(e) => err(404, format!("The file is gone: {e}")),
    }
}

/// Absent fields are left alone; a `null` caption clears it.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = ShelfImageUpdate)]
pub struct ImageUpdate {
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "explicit")]
    #[schema(value_type = Option<String>)]
    pub caption: Option<Option<String>>,
    pub position: Option<i64>,
}

// PATCH /api/v1/physical/images/:id — file it, name it, move it.
/// Re-file, caption or reorder a scan.
///
/// Scope: `write` *(since v0.120)*.
#[utoipa::path(
    patch,
    path = "/api/v1/physical/images/{id}",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = ImageUpdate,
    responses(
        (status = 200, description = "Saved.", body = ShelfDone),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn update_image(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    Json(body): Json<ImageUpdate>,
) -> impl IntoResponse {
    let kind = body.kind.as_deref().map(str::trim).filter(|k| !k.is_empty());
    match state
        .db
        .update_physical_image(
            user.user_id,
            id,
            kind,
            body.caption.as_ref().map(|c| c.as_deref()),
            body.position,
        )
        .await
    {
        Ok(true) => (StatusCode::OK, Json(ShelfDone { status: "ok" })).into_response(),
        Ok(false) => err(404, "No such image"),
        Err(e) => err(500, e.to_string()),
    }
}

/// Remove a scan.
///
/// Session-only, deliberately, for the reason removing a shelf item is: an
/// uploaded file is deleted from disk with the row and cannot be recovered,
/// and no backup holds it. A provider's image is only unlinked and can be
/// fetched again.
#[utoipa::path(
    delete,
    path = "/api/v1/physical/images/{id}",
    tag = "Shelf",
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "Deleted.", body = ShelfDone),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn delete_image(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.delete_physical_image(user.user_id, id).await {
        Ok(Some(file)) => {
            if let Some(f) = file {
                if let Err(e) = tokio::fs::remove_file(state.uploads_dir.join(&f)).await {
                    warn!("Could not remove the scan at {}: {}", f, e);
                }
            }
            (StatusCode::OK, Json(ShelfDone { status: "deleted" })).into_response()
        }
        Ok(None) => err(404, "No such image"),
        Err(e) => err(500, e.to_string()),
    }
}

// ════════════════════════════════════════════════════════════
//  Playing a side
// ════════════════════════════════════════════════════════════

/// What actually happened to one track on the side, as the deck saw it.
//
// The client sends this because it is the only thing that *knows*: it ran the
// clock, so it saw the pauses and the skips. Deriving it server-side from a
// start time and a running order can only describe an uninterrupted play.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TrackOutcome {
    pub position: i64,
    /// When this track actually started, unix seconds.
    pub started_at: i64,
    /// Skipped rather than heard. Stored, but excluded from every count and
    /// never forwarded.
    //
    // `scrobbles.skipped` exists for exactly this — the rule the Rockbox
    // import already follows. A skip forwarded to Last.fm would be a wrong
    // scrobble on a permanent record.
    #[serde(default)]
    pub skipped: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = SidePlay)]
pub struct PlaySide {
    #[schema(example = "A")]
    pub side: String,
    /// Per-track outcomes from the deck. Authoritative where present — the
    /// browser ran the clock, so it is the only thing that saw the pauses and
    /// the skips. Without it the running order is laid out from `started_at`.
    #[serde(default)]
    pub tracks: Vec<TrackOutcome>,
    /// When the side *started*, unix seconds. Defaults to now minus its running
    /// time, which is what you want when you press the button as it finishes.
    pub started_at: Option<i64>,
    /// The turntable, cartridge or deck. Its playback hours are summed from
    /// these rows.
    pub equipment_id: Option<i64>,
    pub chain_id: Option<i64>,
    /// The last track that played, when the side was not run to its end. With
    /// `from_position` this selects a range; the two equal is a single track.
    /// Tracks after it are not written at all — not as skips either. The play
    /// still counts as wear.
    //
    // A track nobody reached is not a track they skipped, and filing it as one
    // would put noise into the single query in `db.rs` whose subject *is* the
    // skipped rows. `track_count` is what tells a partial apart from a whole
    // side afterwards, which is why no new column was needed.
    pub to_position: Option<i64>,
    /// The first track that actually played, when the side did not start at
    /// its beginning. **Load-bearing**: without it, starting at track 4
    /// records tracks 1–3 as played. They are not written at all — and not as
    /// *skips*, since a track you never reached is not one you skipped. Wear
    /// is credited only for the part that played.
    //
    // `play_side` writes a listen for **every** track on the side, and a skip
    // feeds the skip-rate analytics `scrobbles.skipped` exists for.
    #[serde(default)]
    pub from_position: Option<i64>,
    /// Where a tape was left when it came off, in seconds into the side.
    /// **Cassettes only**, and clamped to the side's length. Absent means the
    /// side ran through and the tape is wound back.
    //
    // `Database::park_tape` enforces the medium, since it is a fact about the
    // medium rather than about this request.
    #[serde(default)]
    pub parked_secs: Option<i64>,
    /// Which variant of the chain was fitted — the cartridge in the headshell.
    /// The shelf is the one write path that legitimately knows this, because
    /// you told it when you dropped the needle.
    #[serde(default)]
    pub chain_variant_id: Option<i64>,
}

// POST /api/v1/physical/:id/play — record a side.
//
// **One listen per track**, not one per side. Every counting query in this app
// is over listens, so a side-shaped row would be invisible to all of them.
//
// The timestamps are the only inferred thing here, and worth being honest
// about: you observed when the side started, not when track three did. They
// are spaced by each track's running time from the pressing's tracklist, so
// the ordering and the gaps are right even though no individual timestamp was
// measured. A track with no known duration is given three minutes, which is
// wrong but bounded — the alternative is stacking every track on one instant,
// which would break session grouping and the hour-of-day heatmap outright.
/// Record a side as played.
///
/// Scope: `write` *(since v0.120)*. **Writes one listen per track, not one per side** — every
/// counting query in Tapedeck is over listens, so a side-shaped row would
/// be invisible to all of them.
///
/// Timestamps are the one inferred thing here: you observed when the side
/// *started*, so tracks are spaced by their running times. A track of
/// unknown length gets three minutes — wrong but bounded, where stacking
/// them on one instant would break session grouping and the hour-of-day
/// heatmap outright.
///
/// Per-track outcomes from the browser deck are taken as authoritative: a
/// side laid out from a start time can only describe an uninterrupted play.
/// A skipped track is stored, excluded from every count, and never
/// forwarded — and does not accrue gear hours.
#[utoipa::path(
    post,
    path = "/api/v1/physical/{id}/play",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = PlaySide,
    responses(
        (status = 200, description = "Logged.", body = SideLogged),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn play_side(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    Json(body): Json<PlaySide>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };
    // Before anything is written: these land on every listen below and are
    // then remembered on the pressing.
    if let Some(refused) =
        refuse_foreign_gear(&state, user.user_id, body.chain_id, body.equipment_id).await
    {
        return refused;
    }

    let all: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();
    let side = body.side.trim().to_uppercase();
    let mut tracks: Vec<&PhysicalTrack> =
        all.iter().filter(|t| t.side.eq_ignore_ascii_case(&side)).collect();
    tracks.sort_by_key(|t| t.position);

    if tracks.is_empty() {
        return err(400, format!("Side {side} has no tracks on it"));
    }

    // Everything before the needle went down never played. This has to happen
    // **before** `total`, which feeds both the derived start time and the wear
    // recorded below — crediting the stylus with a whole side for the half of
    // it you actually played is the same error `skipped` guards everywhere.
    if let Some(from) = body.from_position {
        tracks.retain(|t| t.position >= from);
        if tracks.is_empty() {
            return err(400, format!("Side {side} has no track at or after {from}"));
        }
    }
    if let Some(to) = body.to_position {
        tracks.retain(|t| t.position <= to);
        if tracks.is_empty() {
            return err(400, format!("Side {side} has no track at or before {to}"));
        }
    }

    const ASSUMED_SECS: i64 = 180;
    let total: i64 = tracks.iter().map(|t| t.duration_secs.unwrap_or(ASSUMED_SECS)).sum();
    let now = chrono::Utc::now().timestamp();
    let start = body.started_at.unwrap_or(now - total);

    // A chain makes this listen attributable the same way any other is. There
    // is no rung below it here: `equipment` carries no default chain (only
    // `devices` do), and a turntable is not a device that submits anything.
    //
    // What there *is* is a rung above the nothing there used to be — the
    // pressing's own default. A record lives with one turntable, so the deck
    // asks once and the answer is written back below; without that, every
    // shelf listen arrived with `chain_id = NULL` and the chain hours of the
    // gear it plainly ran through never moved.
    let chain_id = body.chain_id.or(release.chain_id);
    let equipment_id = body.equipment_id.or(release.equipment_id);
    // A variant only means anything paired with its own chain, so one naming a
    // different chain is dropped rather than stored — `equipment_seconds` would
    // ignore the pairing anyway, and a listen carrying an id nothing resolves
    // is worse than one carrying none.
    let variant_id = match (chain_id, body.chain_variant_id.or(release.chain_variant_id)) {
        (Some(cid), Some(vid)) => match state.db.variant_fits_chain(user.user_id, vid, cid).await {
            Ok(true) => Some(vid),
            Ok(false) => None,
            Err(e) => return err(500, e.to_string()),
        },
        _ => None,
    };

    // Outcomes the deck reported, by track position.
    let reported: std::collections::HashMap<i64, &TrackOutcome> =
        body.tracks.iter().map(|o| (o.position, o)).collect();

    let mut written = 0usize;
    let mut skipped_count = 0usize;
    let mut at = start;
    for t in &tracks {
        // Two different numbers that used to be one. `spacing` is the bounded
        // guess the derived clock walks on, which is what `ASSUMED_SECS` was
        // always for — "wrong but bounded" beats stacking a side on one
        // instant. The listen's own `duration` is a *measurement*, and there is
        // none: writing three minutes there put an invented figure into every
        // hours readout in the application, including the stylus wear this
        // shelf exists to track. A record whose lookup returned no durations —
        // Discogs often has none — was quietly under-counting the cartridge.
        let spacing = t.duration_secs.unwrap_or(ASSUMED_SECS);
        let dur = t.duration_secs.filter(|d| *d > 0).map(|d| d as u64);
        // The deck's own timing wins wherever it reported any — it ran the
        // clock, so it is the only thing that saw the pauses.
        let outcome = reported.get(&t.position);
        let ts = outcome.map(|o| o.started_at).unwrap_or(at);
        let was_skipped = outcome.is_some_and(|o| o.skipped);
        let play = Play {
            title: t.title.clone(),
            artist: t.artist.clone().unwrap_or_else(|| release.artist.clone()),
            artists: None,
            album: Some(release.title.clone()),
            timestamp: ts.max(0) as u64,
            duration: dur,
            track_number: Some(t.position as i32),
            mbid_recording: None,
            mbid_release: release.mbid_release.clone(),
            mbid_release_group: None,
            mbid_release_track: None,
            mbid_work: None,
            mbid_album_artist: None,
            mbid_artist: None,
            caa_id: None,
            caa_release_mbid: None,
            isrc: None,
            // Stable per side play, so pressing the button twice by accident
            // is caught by the same dedup every other source relies on.
            source_id: format!("physical-{}-{}-{}-{}", release.id, side, start, t.position),
            source_name: "physical".into(),
        };

        match state
            .db
            .save_scrobble(
                user.user_id,
                &play,
                None,
                None,
                chain_id,
                // Which cartridge was fitted. The shelf is the one write path that
                // legitimately knows — you told it when you dropped the needle.
                variant_id,
                None,
                Some(medium_label(&release.kind)),
                None,
                // Kept so skip-rate analytics has something real to count, and
                // filed so nothing else does. This used to be an UPDATE after
                // the insert, which left the row briefly `'pending'` — long
                // enough for a flush tick to forward a skip.
                ListenDisposition { imported: false, skipped: was_skipped },
            )
            .await
        {
            Ok(true) => {
                if was_skipped {
                    skipped_count += 1;
                } else {
                    written += 1;
                }
            }
            Ok(false) => {}
            Err(e) => warn!("Could not store a physical listen: {}", e),
        }
        // Only advances the derived clock; a reported track carries its own.
        at = ts + spacing;
    }

    // Where the tape ends up. A cassette does not rewind itself, so stopping
    // halfway leaves it there — `park_tape` writes nothing for any other
    // medium, since a record has no such state and inventing one would be
    // modelling physics the object does not have.
    //
    // An explicit `parked_secs` is the deck reporting where it stopped; a side
    // played to the end winds through and is left at the start of the *other*
    // side, which is what an absent value with a `next_side` means.
    if let Some(secs) = body.parked_secs {
        // A tape cannot be parked past the end of its own side. Clamped here
        // rather than trusted, because the position is arithmetic done in the
        // browser and getting it wrong is silent — the first version of this
        // added the needle-drop offset twice and parked a 12:00 side at 12:14,
        // which then reads as "the tape ran out" on a side that had four
        // minutes left. The whole side, not the part that played.
        let side_secs: i64 = all
            .iter()
            .filter(|t| t.side.eq_ignore_ascii_case(&side))
            .map(|t| t.duration_secs.unwrap_or(ASSUMED_SECS))
            .sum();
        let parked = secs.clamp(0, side_secs);
        if let Err(e) = state.db.park_tape(user.user_id, release.id, &side, parked).await {
            warn!("Could not remember where the tape was left: {}", e);
        }
    } else if let Err(e) = state.db.rewind_tape(user.user_id, release.id).await {
        warn!("Could not wind the tape back: {}", e);
    }

    // Only when something was actually recorded. A duplicate submission — the
    // same side at the same start time, which is what a double-tap produces —
    // dedups every listen, and counting stylus hours for it would wear the
    // cartridge down on paper for a play that did not happen twice. Skips
    // count towards wear: the needle was still in the groove to get there.
    if written + skipped_count > 0 {
        if let Err(e) = state
            .db
            .record_physical_play(
                user.user_id,
                release.id,
                &side,
                start,
                total,
                tracks.len() as i64,
                equipment_id,
                chain_id,
            )
            .await
        {
            warn!("Could not record the side play: {}", e);
        }
    }

    // The side is off the deck, whether it ran through or was stopped early.
    // Without this the registry keeps extrapolating the last track it was told
    // about and the dashboard shows a record still playing.
    state.now_playing.clear_user_source(user.user_id, SHELF_SOURCE);
    // And the stored spin goes with it, or reopening the page on any device
    // would offer to resume a side that has already been recorded — which is
    // how the same side gets logged twice.
    if let Err(e) = state.db.clear_spin(user.user_id).await {
        warn!("Could not clear the spin after recording a side: {}", e);
    }

    // The sleeve, onto every listen this record has produced.
    //
    // Written here rather than left to `backfill_artwork`, which resolves a
    // listen by asking a provider about (artist, title, album) — and a provider
    // answers per *song*, so one side of a record came back with a different
    // cover per track, several of them compilations. The shelf is holding the
    // object; there is nothing to look up.
    match state.db.physical_cover(user.user_id, release.id).await {
        Ok(Some(cover)) => {
            if let Err(e) = state.db.set_physical_artwork(user.user_id, release.id, &cover).await {
                warn!("Could not stamp the sleeve on {}'s listens: {}", release.title, e);
            }
        }
        // No cover yet is a normal state — a pressing typed in by hand, or one
        // the archive had nothing for. The listens keep the lettered tile,
        // which is honest, rather than a provider's guess per track.
        Ok(None) => {}
        Err(e) => warn!("Could not read the sleeve for {}: {}", release.title, e),
    }

    // Remember what it was played on, so the next side of the same record —
    // and the next time it comes off the shelf — is pre-filled. Only when the
    // caller stated something: a play that fell through to the stored default
    // has nothing new to say.
    if body.chain_id.is_some() || body.equipment_id.is_some() || body.chain_variant_id.is_some() {
        if let Err(e) = state
            .db
            .update_physical(
                user.user_id,
                release.id,
                None,
                body.chain_id.map(Some),
                body.equipment_id.map(Some),
                None,
                None,
                // The cartridge you keep fitted for this record, so the next
                // side does not ask again. `variant_id` rather than the raw
                // body: a variant that failed the chain check is not stored.
                body.chain_variant_id.map(|_| variant_id),
                None,
                None,
                None,
            )
            .await
        {
            warn!("Could not remember what {} is played on: {}", release.title, e);
        }
    }

    info!("🎚️ Side {} of {} — {}: {} listen(s)", side, release.artist, release.title, written);

    // Which side to reach for next — a double LP is A/B/C/D, so this is the
    // next side that exists rather than always "the other one".
    let mut sides: Vec<String> = all.iter().map(|t| t.side.to_uppercase()).collect();
    sides.sort();
    sides.dedup();
    // Only when the side actually ran to its end. Lifting the needle after one
    // song and being told to turn the record over is the reminder firing on
    // something that did not happen — the flip prompt is for a side that has
    // just run out, which is the whole reason it exists.
    let ran_out = tracks.last().map(|t| t.position)
        == all.iter().filter(|t| t.side.eq_ignore_ascii_case(&side)).map(|t| t.position).max();
    let next = ran_out
        .then(|| {
            sides.iter().position(|s| *s == side).map(|i| sides[(i + 1) % sides.len()].clone())
        })
        .flatten();

    (
        StatusCode::OK,
        Json(SideLogged {
            listens_added: written,
            skipped: skipped_count,
            tracks: tracks.len(),
            duration_secs: total,
            started_at: start,
            next_side: next,
        }),
    )
        .into_response()
}

// ════════════════════════════════════════════════════════════
//  What's on the deck
// ════════════════════════════════════════════════════════════

/// The name every shelf entry is filed under in the registry, and therefore the
/// key `clear_user_source` uses. One constant so a stopped side cannot fail to
/// clear the entry a started side wrote.
const SHELF_SOURCE: &str = "Shelf";

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeckState {
    #[schema(example = "A")]
    pub side: String,
    /// The track's position on the side.
    //
    // The deck knows which one it is inside, because it is the thing running
    // the clock.
    pub position: i64,
    /// Seconds into that track.
    #[serde(default)]
    pub elapsed: u64,
    /// A paused record is not playing, but it is still on the deck. The entry
    /// stays and stops advancing, which is what the still platter shows.
    #[serde(default)]
    pub paused: bool,
}

// POST /api/v1/physical/:id/now-playing — a side is on.
//
// The shelf is the one source where the **browser** is the player: the clock
// runs in front of the turntable, in localStorage, and the server cannot
// derive a playhead from anything it holds. So the deck pushes, rather than
// the engine polling — which is why this endpoint exists at all when Plex,
// Jellyfin and Navidrome need nothing of the kind.
//
// Called on every track change, on pause and resume, and on a heartbeat while
// a side runs. The heartbeat is not optional: `NowPlaying::is_live` expires an
// entry by *running past the end of the track*, so a paused record would drop
// off the dashboard one track length after it was paused.
/// A side is on the deck.
///
/// Scope: `write` *(since v0.120)*. The shelf is the one source where the **browser** is the
/// player: the clock runs in front of the turntable, in localStorage, and
/// nothing server-side can derive a playhead from a stack of rows. So the
/// deck pushes here, where Plex, Jellyfin and Navidrome are polled.
///
/// Call it on every track change, on pause and resume, and on a heartbeat
/// of about ten seconds. The heartbeat is **not optional**: the registry
/// expires an entry by running past the end of the track it knows about, so
/// a paused record would drop off the dashboard one track length after it
/// was paused.
///
/// Now-playing is forwarded to the user's Last.fm and ListenBrainz only
/// when the track actually **changed**, and never while paused — their
/// now-playing is a one-shot that expires on its own, so re-sending it on
/// every heartbeat is noise aimed at someone else's servers. The listens
/// themselves are written by `/play` when the side comes off; this endpoint
/// stores nothing.
#[utoipa::path(
    post,
    path = "/api/v1/physical/{id}/now-playing",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    request_body = DeckState,
    responses(
        (status = 200, description = "OK.", body = DeckReported),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn set_now_playing(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<i64>,
    Json(body): Json<DeckState>,
) -> impl IntoResponse {
    let Ok(Some(release)) = state.db.get_physical(user.user_id, id).await else {
        return err(404, "No such record");
    };
    let all: Vec<PhysicalTrack> = serde_json::from_str(&release.tracks).unwrap_or_default();
    let side = body.side.trim().to_uppercase();
    let Some(track) =
        all.iter().find(|t| t.side.eq_ignore_ascii_case(&side) && t.position == body.position)
    else {
        return err(400, "No such track on that side");
    };

    let play = Play {
        title: track.title.clone(),
        artist: track.artist.clone().unwrap_or_else(|| release.artist.clone()),
        artists: None,
        album: Some(release.title.clone()),
        timestamp: 0,
        duration: track.duration_secs.map(|d| d.max(0) as u64),
        track_number: Some(track.position as i32),
        mbid_recording: None,
        mbid_release: release.mbid_release.clone(),
        mbid_release_group: None,
        mbid_release_track: None,
        mbid_work: None,
        mbid_album_artist: None,
        mbid_artist: None,
        caa_id: None,
        caa_release_mbid: None,
        isrc: None,
        source_id: format!("physical-{}-{}-np", release.id, side),
        source_name: "physical".into(),
    };

    // The browser sitting in front of the turntable ran the clock, so
    // unlike every other push source this really is a measured position.
    let mut np = NowPlaying::from_play(
        &play,
        SHELF_SOURCE,
        None,
        Progress::Exact { ms: body.elapsed * 1000 },
    );
    // `DeckState::paused` says the entry "stays and stops advancing", and until
    // the registry learned about pause only the first half was true: the
    // position counted forward regardless and the 10s heartbeat was the only
    // thing dragging it back to the truth. It really stops now.
    np.paused = body.paused;
    // The medium, which is the only thing resembling a format a turntable can
    // report — there is no codec here and never will be. Note the listen itself
    // files the same string under `submission_client` rather than
    // `listening_context`: "off a CD" describes where it came from, and that is
    // the column `play_side` has always used.
    np.listening_context = Some(medium_label(&release.kind).to_string());
    // A record lives with one turntable, which is why the pressing remembers
    // its chain rather than being asked every time — so the deck can say what
    // this side is running through. One indexed lookup on a heartbeat that
    // already does several, and it is the only rung a shelf listen ever uses.
    if let Some(cid) = release.chain_id {
        if let Ok(Some(chain)) = state.db.get_chain(user.user_id, cid).await {
            np.chain_name = Some(chain.name);
            np.chain_id = chain.id;
        }
    }
    let changed = state.now_playing.set(user.user_id, np);

    // Only on a real change of track, for the reason the registry returns the
    // flag at all: Last.fm's and ListenBrainz's now-playing is a one-shot that
    // expires on its own, so re-sending it on every heartbeat is noise aimed at
    // someone else's servers. A paused side re-reports the same track, so this
    // is false for the whole time it stands still.
    if changed && !body.paused {
        // Now-playing, so an empty answer on a failed read costs one heartbeat
        // and is corrected ten seconds later by the next one.
        let sinks = crate::sinks::sinks_for_user(&state.db, user.user_id, &state.listenbrainz_url)
            .await
            .unwrap_or_default();
        crate::sinks::submit_now_playing(&sinks, &play).await;
    }

    (StatusCode::OK, Json(DeckReported { status: "ok", changed })).into_response()
}

// DELETE /api/v1/physical/now-playing — nothing is on the deck.
//
// Its own route rather than a field on the one above because it is also what
// *cancelling* a side means, and that has no track to name.
/// Nothing is on the deck.
///
/// Scope: `write` *(since v0.120)*. Its own route rather than a field on the push, because it
/// is also what *cancelling* a side means and that has no track to name.
/// Clears only entries this shelf reported — an idle turntable must not
/// wipe a now-playing the user's phone is currently sending over ingest.
#[utoipa::path(
    delete,
    path = "/api/v1/physical/now-playing",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "Cleared.", body = ShelfDone),
        (status = 401, response = Unauthorized),
    )
)]
async fn clear_now_playing(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
) -> impl IntoResponse {
    state.now_playing.clear_user_source(user.user_id, SHELF_SOURCE);
    (StatusCode::OK, Json(ShelfDone { status: "cleared" })).into_response()
}

// ═══════════════════════════════════════════════════════════
//  What is on the deck, as a fact about the shelf
// ═══════════════════════════════════════════════════════════
//
//  The clock still runs in the browser — nothing else can watch a turntable —
//  but *what is on* and *when it started* live here. That is the difference
//  between a side that survives a crash and one that does not, and it is what
//  lets the phone in your pocket see the record that is actually playing.

/// The whole state of the deck, every time.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SpinUpdate {
    /// Must be yours — a 404 otherwise.
    pub release_id: i64,
    pub side: String,
    pub started_at: i64,
    /// Null while running.
    #[serde(default)]
    pub paused_at: Option<i64>,
    /// Seconds spent stopped.
    #[serde(default)]
    pub paused_total: i64,
    /// Per-track `{position, started_at, skipped}`, stored whole.
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub outcomes: Vec<serde_json::Value>,
    #[serde(default)]
    pub chain_id: Option<i64>,
    #[serde(default)]
    pub equipment_id: Option<i64>,
    /// How far into the side this spin began — the needle dropped mid-side, or
    /// a tape resumed from where it was parked.
    #[serde(default)]
    pub start_secs: i64,
}

// GET /api/v1/physical/spin — the side on the deck, with enough of the record
// to draw it without a second request.
/// The side currently on the deck.
///
/// Scope: `read` *(since v0.120)*. Returns `{ "spin": null }` when nothing is on.
///
/// The shelf is the one source where the *browser* is the player, so the
/// countdown runs in the page — but what is on and when it started live
/// here. That is what lets a second device see the record, and what makes a
/// side survive a browser crash.
///
/// A spin whose release has since been deleted is cleared and reported as
/// nothing on, rather than returned as a dangling id.
#[utoipa::path(
    get,
    path = "/api/v1/physical/spin",
    tag = "Shelf",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = Spin),
        (status = 401, response = Unauthorized),
    )
)]
async fn get_spin(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    let spin = match state.db.get_spin(user.user_id).await {
        Ok(s) => s,
        Err(e) => return err(500, e.to_string()),
    };
    let Some(spin) = spin else {
        return (StatusCode::OK, Json(Spin { spin: None })).into_response();
    };

    // A spin whose record has since been deleted is not a spin. Clearing it
    // here rather than returning a dangling id keeps the page from having to
    // reason about a release it cannot fetch.
    let release = match state.db.get_physical(user.user_id, spin.release_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            let _ = state.db.clear_spin(user.user_id).await;
            return (StatusCode::OK, Json(Spin { spin: None })).into_response();
        }
        Err(e) => return err(500, e.to_string()),
    };

    let outcomes: serde_json::Value =
        serde_json::from_str(&spin.outcomes).unwrap_or_else(|_| serde_json::json!([]));
    (
        StatusCode::OK,
        Json(Spin {
            spin: Some(SpinView {
                release_id: spin.release_id,
                kind: release.kind,
                artist: release.artist,
                title: release.title,
                artwork_url: release.artwork_url,
                mbid_release: release.mbid_release,
                side: spin.side,
                started_at: spin.started_at,
                paused_at: spin.paused_at,
                paused_total: spin.paused_total,
                outcomes,
                chain_id: spin.chain_id,
                equipment_id: spin.equipment_id,
                start_secs: spin.start_secs,
                updated_at: spin.updated_at,
            }),
        }),
    )
        .into_response()
}

// PUT /api/v1/physical/spin — put a side on, or update the one that is on.
//
// The whole state every time rather than a patch per field: it is half a dozen
// small numbers, it changes only on real events (start, pause, skip, next),
// and a partial update would let two devices interleave into a state neither
// of them meant.
/// Put a side on the deck, or update the one that is on.
///
/// Scope: `write` *(since v0.120)*. One deck per user — the table's primary key is the user,
/// because nobody listens to two records at once, so starting a second side
/// replaces the first rather than erroring.
///
/// The whole state is sent every time rather than a patch per field: it is
/// half a dozen small numbers, it changes only on real events (start,
/// pause, skip, next track), and a partial update would let two devices
/// interleave into a state neither of them meant.
#[utoipa::path(
    put,
    path = "/api/v1/physical/spin",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    request_body = SpinUpdate,
    responses(
        (status = 200, description = "Stored.", body = ShelfDone),
        (status = 401, response = Unauthorized),
        (status = 404, description = "No such record.", body = ErrorResponse),
    )
)]
async fn put_spin(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Json(body): Json<SpinUpdate>,
) -> impl IntoResponse {
    // Every id in the body names something owned, and each must be the
    // caller's. The chain and the gear are not decoration: the deck hands them
    // straight back to `play_side` when the side is recorded.
    match state.db.get_physical(user.user_id, body.release_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return err(404, "No such record"),
        Err(e) => return err(500, e.to_string()),
    }
    if let Some(refused) =
        refuse_foreign_gear(&state, user.user_id, body.chain_id, body.equipment_id).await
    {
        return refused;
    }

    let outcomes = serde_json::to_string(&body.outcomes).unwrap_or_else(|_| "[]".into());
    match state
        .db
        .set_spin(
            user.user_id,
            body.release_id,
            &body.side,
            body.started_at,
            body.paused_at,
            body.paused_total,
            &outcomes,
            body.chain_id,
            body.equipment_id,
            body.start_secs.max(0),
        )
        .await
    {
        Ok(()) => (StatusCode::OK, Json(ShelfDone { status: "ok" })).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// DELETE /api/v1/physical/spin — take it off the deck.
//
// Also clears the now-playing entry, so the two cannot disagree: a deck
// showing a record that came off twenty minutes ago is the failure the
// now-playing design already calls out.
/// Take the side off the deck.
///
/// Scope: `write` *(since v0.120)*. Clears the now-playing entry as well, so the two cannot
/// disagree. Recording a side (`POST /api/v1/physical/{id}/play`) clears it
/// too — otherwise reopening the page would offer to resume a side that has
/// already been logged, which is how one gets logged twice.
#[utoipa::path(
    delete,
    path = "/api/v1/physical/spin",
    tag = "Shelf",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "Cleared.", body = ShelfDone),
        (status = 401, response = Unauthorized),
    )
)]
async fn delete_spin(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
) -> impl IntoResponse {
    if let Err(e) = state.db.clear_spin(user.user_id).await {
        return err(500, e.to_string());
    }
    state.now_playing.clear_user_source(user.user_id, SHELF_SOURCE);
    (StatusCode::OK, Json(ShelfDone { status: "cleared" })).into_response()
}

/// How a medium is written on a listen. One place, because it is written by the
/// side play, the now-playing push and the export.
fn medium_label(kind: &str) -> &'static str {
    match kind {
        "vinyl" => "Vinyl",
        "cd" => "CD",
        "sacd" => "SACD",
        _ => "Cassette",
    }
}

// GET /api/v1/physical/equipment/:id/wear — hours on a stylus or head.
/// Stylus hours or head wear for a piece of gear.
///
/// Scope: `read` *(since v0.120)*.
///
/// Derived from `physical_plays`. A duplicate submission is not counted as
/// wear.
#[utoipa::path(
    get,
    path = "/api/v1/physical/equipment/{id}/wear",
    tag = "Shelf",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = i64, Path)),
    responses(
        (status = 200, description = "OK.", body = GearWear),
        (status = 401, response = Unauthorized),
    )
)]
async fn equipment_wear(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match state.db.equipment_playback_hours(user.user_id, id).await {
        Ok((hours, sides)) => (
            StatusCode::OK,
            Json(GearWear { hours: (hours * 10.0).round() / 10.0, sides_played: sides }),
        )
            .into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// ════════════════════════════════════════════════════════════
//  Discogs configuration (admin)
// ════════════════════════════════════════════════════════════

/// Whether Discogs is configured.
///
/// Session-only. Never returns the credential itself.
#[utoipa::path(
    get,
    path = "/api/v1/physical/settings/discogs",
    tag = "Shelf",
    responses(
        (status = 200, description = "OK.", body = DiscogsSettings),
        (status = 401, response = Unauthorized),
        (status = 403, description = "Admin only.", body = ErrorResponse),
    )
)]
async fn get_discogs(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let token_env = env_nonempty("TAPEDECK_DISCOGS_TOKEN").is_some();
    let key_env = env_nonempty("TAPEDECK_DISCOGS_KEY").or_else(|| env_nonempty("Consumer_Key"));
    let secret_env =
        env_nonempty("TAPEDECK_DISCOGS_SECRET").or_else(|| env_nonempty("Consumer_Secret"));
    let pair_env = key_env.is_some() && secret_env.is_some();

    let stored_token = state
        .db
        .get_app_setting(DISCOGS_TOKEN_KEY)
        .await
        .unwrap_or(None)
        .is_some_and(|t| !t.trim().is_empty());
    let stored_pair = matches!(
        (
            state.db.get_app_setting(DISCOGS_KEY_KEY).await.unwrap_or(None),
            state.db.get_app_setting(DISCOGS_SECRET_KEY).await.unwrap_or(None)
        ),
        (Some(k), Some(s)) if !k.trim().is_empty() && !s.trim().is_empty()
    );

    let from_env = token_env || pair_env;
    let stored = stored_token || stored_pair;
    (
        StatusCode::OK,
        Json(DiscogsSettings {
            // Never the credential itself, the same rule source credentials follow.
            configured: stored || from_env,
            from_env,
            stored,
            // Which shape is in use, so the UI can say so rather than leaving
            // someone guessing which of the two fields took effect.
            kind: if stored_token || (!stored && token_env) {
                "token"
            } else if stored_pair || (!stored && pair_env) {
                "key_secret"
            } else {
                "none"
            },
        }),
    )
        .into_response()
}

/// Each field is written only when present; an empty string clears it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DiscogsUpdate {
    /// A personal access token. Empty clears it.
    #[serde(default)]
    pub token: Option<String>,
    /// An application's consumer key and secret. Either empty clears the pair.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
}

/// Store a Discogs credential.
///
/// Session-only. Accepts **either** a personal access token **or** a
/// consumer key/secret pair — either is enough, since Tapedeck only reads
/// the public database. The three-legged OAuth flow is deliberately not
/// implemented: it exists so an app can act *as a user*, and registering an
/// app hands you a key and secret that work directly. Stored encrypted.
#[utoipa::path(
    put,
    path = "/api/v1/physical/settings/discogs",
    tag = "Shelf",
    request_body = DiscogsUpdate,
    responses(
        (status = 200, description = "Saved.", body = DiscogsConfigured),
        (status = 401, response = Unauthorized),
        (status = 403, description = "Admin only.", body = ErrorResponse),
    )
)]
async fn set_discogs(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<DiscogsUpdate>,
) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    // Each field is written only when present, so saving a key/secret pair does
    // not silently wipe a token that was already working — and clearing one is
    // an explicit empty string rather than an omission.
    for (key, value) in [
        (DISCOGS_TOKEN_KEY, &body.token),
        (DISCOGS_KEY_KEY, &body.key),
        (DISCOGS_SECRET_KEY, &body.secret),
    ] {
        if let Some(v) = value {
            if let Err(e) = state.db.set_app_setting(key, v.trim()).await {
                return err(500, e.to_string());
            }
        }
    }
    let configured = discogs_auth(&state).await.is_some();
    (StatusCode::OK, Json(DiscogsConfigured { configured })).into_response()
}

// POST /api/v1/physical/settings/discogs/test — does the credential work?
//
// Separate from saving it because Discogs answers a bad key/secret with a
// normal 200, serving the request as anonymous. Without this, a typo looks
// exactly like success until someone notices barcode lookups are quietly
// worse.
/// Verify the stored Discogs credential.
///
/// Session-only, and **necessary rather than a nicety**: Discogs does not
/// reject a bad credential. With a bogus key and secret, `/releases/1` and
/// `/database/search` both return a normal 200 with real data, and the
/// rate-limit header reads the *authenticated* 60 — the limit is raised for
/// the presence of an `Authorization` header without checking its contents.
/// `/users/{username}` is the one endpoint that actually verifies, and is
/// what this calls. Without it, a typo is indistinguishable from a working
/// credential.
#[utoipa::path(
    post,
    path = "/api/v1/physical/settings/discogs/test",
    tag = "Shelf",
    responses(
        (status = 200, description = "Result of the check.", body = DiscogsCheck),
        (status = 400, description = "Discogs is not configured on this server.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, description = "Admin only.", body = ErrorResponse),
    )
)]
async fn test_discogs(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let Some(auth) = discogs_auth(&state).await else {
        return err(400, "Discogs isn't configured on this server");
    };
    match release_lookup::discogs_verify(&auth).await {
        Ok(()) => (
            StatusCode::OK,
            Json(DiscogsCheck {
                ok: true,
                message: "Working — Discogs accepted the credential.".into(),
            }),
        )
            .into_response(),
        // A rejection is a 200 with `ok: false`, not an HTTP error: the request
        // to Tapedeck succeeded and the answer is "no". The message is
        // Discogs' own, which names which credential shape it rejected.
        Err(e) => (StatusCode::OK, Json(DiscogsCheck { ok: false, message: e.to_string() }))
            .into_response(),
    }
}

// ════════════════════════════════════════════════════════════
//  Taking the shelf with you
// ════════════════════════════════════════════════════════════

/// What the zip says about itself.
const EXPORT_README: &str = "\
# Your shelf

Everything Tapedeck holds about the objects you own — the pressings, every side
you have put on, and the gear they ran through.

* `shelf.json` — one entry per pressing: what it is, when you bought it, its
  tracklist as sides, what it is normally played on, and its scans.
* `plays.csv` — every side ever played, oldest first, with the gear named.
* `gear.json` — your equipment and signal chains, with purchase dates.
* `scans/` — the images **you uploaded**, one directory per pressing.

Scans that live at the Cover Art Archive or Discogs are listed in `shelf.json`
with their URLs rather than copied in here. They are not yours to redistribute
and they are still there to fetch; your own scans exist nowhere else, which is
why those are the ones in the file.

That is also why this export exists separately from the database backup:
`GET /api/v1/backup` is a snapshot of the database, and uploaded scans are files
beside it rather than rows inside it. A backup and this export together are the
whole shelf.
";

// POST /api/v1/physical/import — put a shelf back.
//
// The counterpart of the export above, and deliberately **not** a database
// restore. It brings back the objects: pressings, their tracklists, the gear,
// the sides you played, and your own scans.
//
// What it cannot bring back is the *listening* — a shelf export holds no
// `scrobbles`, so the listens a side generated are not in the file and the
// chain hours derived from them stay where they are. That is stated in the
// response rather than papered over, because a shelf that came back with its
// play counts but no listens would make two screens disagree.
//
// **Everything is matched before it is inserted**, so importing the same zip
// twice changes nothing the second time. That matters more here than for a
// listens import: a duplicated pressing is a visible mess on a page you look
// at, and there is no dedup elsewhere that would catch it.
/// Put a shelf back from an export.
///
/// Session-only, `multipart/form-data`, 256 MB maximum. The counterpart of
/// `GET /api/v1/physical/export`, and deliberately **not** a database
/// restore: it brings back the *objects* — pressings, tracklists, gear,
/// the sides played, and your own scans.
///
/// What it cannot bring back is the **listening**. A shelf export holds no
/// `scrobbles`, so the listens a side generated are not in the file, and
/// the chain hours derived from them do not return with the objects. The
/// response says so rather than leaving it to be discovered.
///
/// **Everything is matched before it is inserted**, so importing the same
/// archive twice changes nothing the second time. A pressing is matched on
/// its barcode or release MBID when it has one, and otherwise on artist,
/// title and medium together; a side played is matched on its side and
/// timestamp, so re-importing cannot double the wear on a stylus.
///
/// Signal chains are **matched by name, never created** — the same rule
/// `[[gear:…]]` follows. A chain is something you build step by step, and
/// conjuring an empty one from a name in a file would silently attribute
/// listens to a chain with no components. Gear *is* created, since a piece
/// of equipment is fully described by the file.
///
/// Nothing is written until `shelf.json` parses, so an unrelated or
/// truncated zip fails before it has half-populated a shelf.
#[utoipa::path(
    post,
    path = "/api/v1/physical/import",
    tag = "Shelf",
    request_body(content = FileUpload, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "Imported.", body = ShelfImported),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 413, description = "Larger than 256 MB.", body = ErrorResponse),
    )
)]
async fn import_shelf(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut body: Option<Vec<u8>> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if let Ok(b) = field.bytes().await {
            if !b.is_empty() {
                body = Some(b.to_vec());
                break;
            }
        }
    }
    let Some(body) = body else { return err(400, "No file in the request") };
    if body.len() > MAX_IMPORT_BYTES {
        return err(413, "That archive is larger than 256 MB");
    }

    let reader = std::io::Cursor::new(body);
    let mut zip = match zip::ZipArchive::new(reader) {
        Ok(z) => z,
        Err(e) => return err(400, format!("That is not a zip archive: {e}")),
    };

    // Read the manifest first — nothing is written until it parses, so a
    // truncated or unrelated zip fails before it has half-populated a shelf.
    let shelf: Vec<serde_json::Value> = {
        let Ok(mut f) = zip.by_name("shelf.json") else {
            return err(400, "No shelf.json in that archive — is it a Tapedeck shelf export?");
        };
        let mut text = String::new();
        use std::io::Read;
        if f.read_to_string(&mut text).is_err() {
            return err(400, "shelf.json could not be read");
        }
        match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => return err(400, format!("shelf.json is not valid JSON: {e}")),
        }
    };

    let mut report = ImportReport::default();

    // ── Gear first: a pressing names its chain and deck by *name*, so they
    // have to exist before the releases that point at them.
    if let Ok(mut f) = zip.by_name("gear.json") {
        use std::io::Read;
        let mut text = String::new();
        if f.read_to_string(&mut text).is_ok() {
            if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) {
                for g in doc.get("equipment").and_then(|v| v.as_array()).into_iter().flatten() {
                    let Some(name) = g.get("name").and_then(|v| v.as_str()) else { continue };
                    let kind = g.get("type").and_then(|v| v.as_str()).unwrap_or("other");
                    // `upsert_equipment` matches on the name, so re-importing
                    // updates rather than duplicating. `hours` is deliberately
                    // not restored: it is derived from the listens, and writing
                    // it here would be a second answer that drifts.
                    match state
                        .db
                        .upsert_equipment(
                            user.user_id,
                            name,
                            kind,
                            g.get("brand").and_then(|v| v.as_str()),
                            g.get("model").and_then(|v| v.as_str()),
                            0.0,
                            g.get("purchased_at").and_then(|v| v.as_i64()),
                        )
                        .await
                    {
                        Ok(_) => report.gear += 1,
                        Err(e) => warn!("shelf import: gear {name}: {e}"),
                    }
                }
            }
        }
    }

    let gear = state.db.get_equipment(user.user_id).await.unwrap_or_default();
    let chains = state.db.get_chains(user.user_id).await.unwrap_or_default();
    // Chains are **matched, never created** — the same rule `[[gear:…]]`
    // follows. A chain is something you build step by step; conjuring an empty
    // one from a name in a file would produce a chain with no components that
    // silently attributes listens to nothing.
    let chain_by_name = |n: Option<&str>| {
        n.and_then(|n| chains.iter().find(|c| c.name.eq_ignore_ascii_case(n))).and_then(|c| c.id)
    };
    let gear_by_name = |n: Option<&str>| {
        n.and_then(|n| gear.iter().find(|g| g.name.eq_ignore_ascii_case(n))).map(|g| g.id)
    };

    let existing = state.db.list_physical(user.user_id, None).await.unwrap_or_default();

    for entry in &shelf {
        let (Some(artist), Some(title), Some(kind)) = (
            entry.get("artist").and_then(|v| v.as_str()),
            entry.get("title").and_then(|v| v.as_str()),
            entry.get("kind").and_then(|v| v.as_str()),
        ) else {
            report.skipped += 1;
            continue;
        };
        if !SHELF_KINDS.contains(&kind) {
            report.skipped += 1;
            continue;
        }

        // Already on the shelf? A barcode or an MBID identifies a *pressing*
        // exactly; without either, the artist, title and medium together are
        // the best available key — two copies of the same record on the same
        // shelf is a thing that happens, but importing the same file twice is
        // far more likely than owning a duplicate.
        let barcode = entry.get("barcode").and_then(|v| v.as_str());
        let mbid = entry.get("mbid_release").and_then(|v| v.as_str());
        let already = existing.iter().find(|r| {
            (barcode.is_some() && r.barcode.as_deref() == barcode)
                || (mbid.is_some() && r.mbid_release.as_deref() == mbid)
                || (r.kind == kind
                    && r.artist.eq_ignore_ascii_case(artist)
                    && r.title.eq_ignore_ascii_case(title))
        });
        if already.is_some() {
            report.already += 1;
            continue;
        }

        let tracks = entry
            .get("tracks")
            .map(|t| serde_json::to_string(t).unwrap_or_else(|_| "[]".into()))
            .unwrap_or_else(|| "[]".into());

        let id = match state
            .db
            .create_physical(
                user.user_id,
                kind,
                artist,
                title,
                entry.get("year").and_then(|v| v.as_i64()),
                mbid,
                entry.get("discogs_id").and_then(|v| v.as_str()),
                barcode,
                entry.get("catalog_number").and_then(|v| v.as_str()),
                None,
                &tracks,
                entry.get("notes").and_then(|v| v.as_str()),
                entry.get("purchased_at").and_then(|v| v.as_i64()),
                entry.get("acquired_as").and_then(|v| v.as_str()),
                entry.get("acquired_from").and_then(|v| v.as_str()),
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                warn!("shelf import: {artist} — {title}: {e}");
                report.skipped += 1;
                continue;
            }
        };
        report.releases += 1;

        // What it is normally played on, resolved from the names the export
        // wrote. Missing either is normal — the chain may not exist here.
        let chain = chain_by_name(entry.get("played_on_chain").and_then(|v| v.as_str()));
        let deck = gear_by_name(entry.get("played_on_equipment").and_then(|v| v.as_str()));
        if chain.is_some() || deck.is_some() {
            let _ = state
                .db
                .update_physical(
                    user.user_id,
                    id,
                    None,
                    chain.map(Some),
                    deck.map(Some),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
        }

        // Images. A provider image is a URL the archive still serves, so it
        // comes back as a row pointing at it; an upload is a file in the zip
        // and has to be written back to disk before the row means anything.
        for img in entry.get("images").and_then(|v| v.as_array()).into_iter().flatten() {
            let ikind = img.get("kind").and_then(|v| v.as_str()).unwrap_or("other");
            let provider = img.get("provider").and_then(|v| v.as_str()).unwrap_or("upload");
            let caption = img.get("caption").and_then(|v| v.as_str());
            let position = img.get("position").and_then(|v| v.as_i64()).unwrap_or(0);

            if let Some(url) = img.get("url").and_then(|v| v.as_str()) {
                let _ = state
                    .db
                    .add_physical_image(
                        user.user_id,
                        id,
                        ikind,
                        provider,
                        Some(url),
                        None,
                        None,
                        None,
                        None,
                        caption,
                        position,
                        None,
                    )
                    .await;
                report.images += 1;
                continue;
            }

            let Some(name) = img.get("file").and_then(|v| v.as_str()) else { continue };
            let entry_name = format!("scans/{name}");
            let bytes = {
                use std::io::Read;
                match zip.by_name(&entry_name) {
                    Ok(mut f) => {
                        let mut b = Vec::new();
                        if f.read_to_end(&mut b).is_ok() {
                            Some(b)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            };
            let Some(bytes) = bytes else {
                report.missing_scans += 1;
                continue;
            };
            if bytes.len() > MAX_SCAN_BYTES {
                report.missing_scans += 1;
                continue;
            }
            // The extension comes from the stored name, but the *path* is built
            // here — a filename out of an archive never reaches the filesystem,
            // the same rule the upload handler follows for a client's filename.
            let ext = std::path::Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or("jpg")
                .to_ascii_lowercase();
            let dir =
                state.uploads_dir.join("shelf").join(user.user_id.to_string()).join(id.to_string());
            if tokio::fs::create_dir_all(&dir).await.is_err() {
                report.missing_scans += 1;
                continue;
            }
            let fname = format!("{}.{}", random_name(), ext);
            let rel = format!("shelf/{}/{}/{}", user.user_id, id, fname);
            if tokio::fs::write(dir.join(&fname), &bytes).await.is_err() {
                report.missing_scans += 1;
                continue;
            }
            let mime = mime_guess::from_path(&fname).first_raw().unwrap_or("image/jpeg");
            let _ = state
                .db
                .add_physical_image(
                    user.user_id,
                    id,
                    ikind,
                    "upload",
                    None,
                    None,
                    Some(&rel),
                    Some(mime),
                    Some(bytes.len() as i64),
                    caption,
                    position,
                    None,
                )
                .await;
            report.images += 1;
        }

        // The sides played. `plays.csv` is keyed by artist and title rather
        // than by id, so it is matched back here while this pressing is the one
        // in hand — which also means a play whose pressing was skipped as a
        // duplicate is not re-added against the wrong row.
        report.plays += restore_plays(&state, user.user_id, id, artist, title, &mut zip).await;
    }

    (
        StatusCode::OK,
        Json(ShelfImported {
            releases: report.releases,
            already_on_the_shelf: report.already,
            skipped: report.skipped,
            gear: report.gear,
            images: report.images,
            missing_scans: report.missing_scans,
            plays: report.plays,
            // Said plainly rather than left to be discovered: the listens a
            // side generated live in `scrobbles`, which a shelf export does not
            // carry. Chain hours are derived from those, so they do not return
            // with the objects.
            note: concat!(
                "Pressings, gear, scans and sides are back. The listens those sides ",
                "generated are not in a shelf export — restore a database backup for ",
                "the history and the chain hours derived from it."
            ),
        }),
    )
        .into_response()
}

/// The biggest shelf archive we will read. A hundred records with booklets is
/// large; a quarter of a gigabyte is somebody's photo library.
const MAX_IMPORT_BYTES: usize = 256 * 1024 * 1024;

#[derive(Default)]
struct ImportReport {
    releases: usize,
    already: usize,
    skipped: usize,
    gear: usize,
    images: usize,
    missing_scans: usize,
    plays: usize,
}

/// Put back the sides played for one pressing, from `plays.csv`.
///
/// Returns how many were written. Anything unparseable is skipped rather than
/// failing the import — a shelf coming back matters more than one malformed
/// row, and the count in the response says how many landed.
async fn restore_plays<R: std::io::Read + std::io::Seek>(
    state: &Arc<AppState>,
    user_id: i64,
    release_id: i64,
    artist: &str,
    title: &str,
    zip: &mut zip::ZipArchive<R>,
) -> usize {
    use std::io::Read;
    let mut text = String::new();
    let Ok(mut f) = zip.by_name("plays.csv") else { return 0 };
    if f.read_to_string(&mut text).is_err() {
        return 0;
    }

    let existing = state.db.plays_for_release(user_id, release_id).await.unwrap_or_default();
    let mut written = 0;
    for line in text.lines().skip(1) {
        let cols = parse_csv_row(line);
        if cols.len() < 7 {
            continue;
        }
        let (Ok(played_at), Some(row_artist), Some(row_title)) =
            (cols[0].parse::<i64>(), cols.get(1), cols.get(2))
        else {
            continue;
        };
        if !row_artist.eq_ignore_ascii_case(artist) || !row_title.eq_ignore_ascii_case(title) {
            continue;
        }
        let side = cols[4].clone();
        // Re-importing the same file must not double the wear on a stylus.
        if existing
            .iter()
            .any(|(s, at): &(String, i64)| *at == played_at && s.eq_ignore_ascii_case(&side))
        {
            continue;
        }
        let tracks = cols[5].parse::<i64>().unwrap_or(0);
        let secs = cols[6].parse::<i64>().unwrap_or(0);
        let deck = cols.get(7).filter(|v| !v.is_empty());
        let chain = cols.get(8).filter(|v| !v.is_empty());
        let gear_id = match deck {
            Some(n) => {
                state.db.find_equipment_by_name(user_id, n).await.unwrap_or(None).map(|(id, _)| id)
            }
            None => None,
        };
        let chain_id = match chain {
            Some(n) => state.db.get_chain_by_name(user_id, n).await.unwrap_or(None),
            None => None,
        };
        if state
            .db
            .record_physical_play(
                user_id, release_id, &side, played_at, secs, tracks, gear_id, chain_id,
            )
            .await
            .is_ok()
        {
            written += 1;
        }
    }
    written
}

/// Split one CSV row, honouring the quoting the export writes.
fn parse_csv_row(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

// GET /api/v1/physical/export — the shelf as a zip.
//
// Built in memory rather than streamed, unlike the listen export: a zip's
// central directory is written after the entries anyway, and a shelf is
// bounded by how many records fit in a room.
/// The whole shelf as a zip.
///
/// Session-only. `shelf.json` (pressings, purchase dates, tracklists and an
/// image manifest), `plays.csv` (every side played, oldest first, gear
/// named), `gear.json` (equipment and chains with purchase dates) and
/// `scans/` — the images the **user uploaded**, one directory per pressing.
///
/// Images that live at the Cover Art Archive or Discogs are listed with
/// their URLs rather than copied in: they are not the user's to
/// redistribute and they are still there to fetch. Their own scans exist
/// nowhere else, which is why those are the ones in the file.
///
/// This exists separately from `GET /api/v1/backup` for a concrete reason:
/// a backup is a `VACUUM INTO` snapshot of the database, and uploaded scans
/// are files beside it rather than rows inside it. A backup and this export
/// together are the whole shelf.
#[utoipa::path(
    get,
    path = "/api/v1/physical/export",
    tag = "Shelf",
    responses(
        (status = 200, description = "A zip archive.", body = Bytes, content_type = "application/zip"),
        (status = 401, response = Unauthorized),
    )
)]
async fn export_shelf(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    use std::io::Write;

    let uid = user.user_id;
    let releases = match state.db.list_physical(uid, None).await {
        Ok(r) => r,
        Err(e) => return err(500, e.to_string()),
    };
    let images = state.db.all_physical_images(uid).await.unwrap_or_default();
    let plays = state.db.all_physical_plays(uid).await.unwrap_or_default();
    let gear = state.db.get_equipment(uid).await.unwrap_or_default();
    let chains = state.db.get_chains(uid).await.unwrap_or_default();

    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // Images are already compressed; deflating a JPEG spends CPU to make it
        // very slightly bigger.
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        let mut fail = false;
        let mut write = |zip: &mut zip::ZipWriter<_>, name: String, body: &[u8], o| {
            if zip.start_file(name, o).is_err() || zip.write_all(body).is_err() {
                fail = true;
            }
        };

        write(&mut zip, "README.md".into(), EXPORT_README.as_bytes(), opts);

        // One entry per pressing, with its tracklist parsed back out of the
        // stored blob — an export that made the reader parse a JSON string
        // inside a JSON document would be passing the work on rather than
        // doing it.
        let shelf: Vec<serde_json::Value> = releases
            .iter()
            .map(|r| {
                let tracks: serde_json::Value =
                    serde_json::from_str(&r.tracks).unwrap_or_else(|_| serde_json::json!([]));
                let mine: Vec<serde_json::Value> = images
                    .iter()
                    .filter(|i| i.release_id == r.id)
                    .map(|i| {
                        serde_json::json!({
                            "kind": i.kind,
                            "provider": i.provider,
                            "caption": i.caption,
                            "position": i.position,
                            // Exactly one of these is meaningful, and which one
                            // is what `provider` says.
                            "url": i.url,
                            "file": i.file_path.as_ref().map(|_| scan_name(r, i)),
                        })
                    })
                    .collect();
                serde_json::json!({
                    "kind": r.kind,
                    "artist": r.artist,
                    "title": r.title,
                    "year": r.year,
                    "mbid_release": r.mbid_release,
                    "discogs_id": r.discogs_id,
                    "barcode": r.barcode,
                    "catalog_number": r.catalog_number,
                    "purchased_at": r.purchased_at,
                    // Carried so a round trip does not lose them — the import
                    // reads both back, which is what makes the export a backup
                    // of the shelf rather than most of one.
                    "acquired_as": r.acquired_as,
                    "acquired_from": r.acquired_from,
                    "added_at": r.added_at,
                    "notes": r.notes,
                    "played_sides": r.play_count,
                    "played_secs": r.played_secs,
                    "last_played": r.last_played,
                    "played_on_chain": chains.iter().find(|c| c.id == r.chain_id).map(|c| c.name.clone()),
                    "played_on_equipment": gear.iter().find(|g| Some(g.id) == r.equipment_id).map(|g| g.name.clone()),
                    "tracks": tracks,
                    "images": mine,
                })
            })
            .collect();
        write(
            &mut zip,
            "shelf.json".into(),
            serde_json::to_string_pretty(&shelf).unwrap_or_default().as_bytes(),
            opts,
        );

        let mut csv_body = String::from(
            "played_at,artist,title,format,side,tracks,duration_secs,equipment,chain\n",
        );
        for p in &plays {
            csv_body.push_str(&format!(
                "{},{},{},{},{},{},{},{},{}\n",
                p.played_at,
                csv(&p.artist),
                csv(&p.title),
                medium_label(&p.kind),
                csv(&p.side),
                p.track_count,
                p.duration_secs,
                csv(p.equipment_name.as_deref().unwrap_or("")),
                csv(p.chain_name.as_deref().unwrap_or("")),
            ));
        }
        write(&mut zip, "plays.csv".into(), csv_body.as_bytes(), opts);

        let gear_doc = serde_json::json!({
            "equipment": gear.iter().map(|g| serde_json::json!({
                "name": g.name,
                "type": g.equipment_type,
                "brand": g.brand,
                "model": g.model,
                "purchased_at": g.purchased_at,
                "first_used": g.first_used,
                "last_used": g.last_used,
                // Derived from the listens, not an accumulator — so this is
                // the same number the app shows rather than a second answer.
                "hours": (g.total_hours * 10.0).round() / 10.0,
                "notes": g.notes,
            })).collect::<Vec<_>>(),
            "chains": chains.iter().map(|c| serde_json::json!({
                "name": c.name,
                "description": c.description,
                "icon": c.icon,
                "listening_context": c.listening_context,
                "components": c.components,
                "hours": (c.total_hours * 10.0).round() / 10.0,
            })).collect::<Vec<_>>(),
        });
        write(
            &mut zip,
            "gear.json".into(),
            serde_json::to_string_pretty(&gear_doc).unwrap_or_default().as_bytes(),
            opts,
        );

        for r in &releases {
            for img in images.iter().filter(|i| i.release_id == r.id) {
                let Some(rel) = &img.file_path else { continue };
                match tokio::fs::read(state.uploads_dir.join(rel)).await {
                    Ok(bytes) => {
                        write(&mut zip, format!("scans/{}", scan_name(r, img)), &bytes, stored)
                    }
                    // A missing file is not worth failing a whole export over;
                    // it is already listed in `shelf.json`, so the gap is
                    // visible rather than silent.
                    Err(e) => warn!("Scan {} could not be read for export: {}", rel, e),
                }
            }
        }

        if fail || zip.finish().is_err() {
            return err(500, "Could not build the export");
        }
    }

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"tapedeck-shelf.zip\""),
        ],
        buf.into_inner(),
    )
        .into_response()
}

/// Where one scan lands in the zip: `<artist> — <title>/<kind>-<n>.<ext>`.
///
/// Named after the record rather than after the row id, because the point of an
/// export is that it is readable somewhere that has never heard of this
/// database. The id is still in the stem, so two pressings of the same album
/// cannot collide.
fn scan_name(release: &crate::db::PhysicalRelease, img: &PhysicalImage) -> String {
    let ext = img
        .file_path
        .as_ref()
        .and_then(|p| p.rsplit('.').next())
        .filter(|e| e.len() <= 5)
        .unwrap_or("jpg");
    format!(
        "{} - {} [{}]/{}-{:02}.{}",
        safe(&release.artist),
        safe(&release.title),
        release.id,
        safe(&img.kind),
        img.position + 1,
        ext
    )
}

/// A path segment a filesystem will accept. Replaces rather than drops, so
/// `AC/DC` reads as `AC DC` rather than `ACDC` — the same rule the liner-note
/// export's filenames follow.
fn safe(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim().to_string();
    if trimmed.is_empty() {
        "Untitled".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

/// Minimal RFC-4180 field escaping, matching the listen export's.
fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    lookup,
    export_shelf,
    import_shelf,
    discogs_detail,
    equipment_wear,
    get_discogs,
    set_discogs,
    test_discogs,
    clear_now_playing,
    get_spin,
    put_spin,
    delete_spin,
    update_image,
    delete_image,
    image_file,
    list_shelf,
    add_release,
    get_one,
    update_release,
    delete_release,
    set_tracks,
    preview_durations,
    apply_durations,
    play_side,
    set_now_playing,
    side_plays,
    list_images,
    upload_image,
    fetch_images
))]
pub struct Api;

pub fn create_physical_router() -> Router<Arc<AppState>> {
    Router::new()
        // Registered before `/:id` so they aren't swallowed by the
        // parameterised route — axum 0.7 / matchit 0.7 matches in order.
        .route("/api/v1/physical/lookup", get(lookup))
        .route("/api/v1/physical/export", get(export_shelf))
        .route("/api/v1/physical/import", post(import_shelf))
        .route("/api/v1/physical/discogs/:id", get(discogs_detail))
        .route("/api/v1/physical/equipment/:id/wear", get(equipment_wear))
        .route("/api/v1/physical/settings/discogs", get(get_discogs).put(set_discogs))
        .route("/api/v1/physical/settings/discogs/test", post(test_discogs))
        .route("/api/v1/physical/now-playing", axum::routing::delete(clear_now_playing))
        // Before "/:id", or matchit reads "spin" as a release id.
        .route("/api/v1/physical/spin", get(get_spin).put(put_spin).delete(delete_spin))
        .route(
            "/api/v1/physical/images/:id",
            axum::routing::patch(update_image).delete(delete_image),
        )
        .route("/api/v1/physical/images/:id/file", get(image_file))
        .route("/api/v1/physical", get(list_shelf).post(add_release))
        .route("/api/v1/physical/:id", get(get_one).patch(update_release).delete(delete_release))
        .route("/api/v1/physical/:id/tracks", axum::routing::put(set_tracks))
        .route("/api/v1/physical/:id/durations", get(preview_durations).post(apply_durations))
        .route("/api/v1/physical/:id/play", post(play_side))
        .route("/api/v1/physical/:id/now-playing", post(set_now_playing))
        .route("/api/v1/physical/:id/sides", get(side_plays))
        .route("/api/v1/physical/:id/images", get(list_images).post(upload_image))
        .route("/api/v1/physical/:id/images/fetch", post(fetch_images))
}
