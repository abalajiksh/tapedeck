//! Admin endpoints for the Every Noise genre map.
//!
//! Admin-only throughout. This is instance-wide reference data rather than
//! anything belonging to a user, and it makes outbound requests to a third
//! party — neither is a decision an ordinary account should be making.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::error;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::everynoise::scrape::{self, Progress, Tier};
use crate::everynoise::store::Summary;
use crate::openapi::{Forbidden, Unauthorized};
use utoipa::{IntoParams, ToSchema};

/// What is held, and whether a walk is running.
#[derive(Serialize, ToSchema)]
pub struct EveryNoiseStatus {
    pub summary: Summary,
    /// Null when nothing has run since the process started.
    pub progress: Option<Progress>,
    /// The admin's address sent in the scraper's `User-Agent`. Echoed so the
    /// field can be pre-filled; it is not a credential.
    pub contact: Option<String>,
    pub source: &'static str,
}

#[derive(Serialize, ToSchema)]
pub struct ScrapeStarted {
    /// `started`.
    pub status: &'static str,
    pub tier: Tier,
}

/// Everything on the map, and where your listening sits on it.
#[derive(Serialize, ToSchema)]
pub struct GenreMap {
    pub map: MapBackdrop,
    /// Your genres that have a place on the map.
    pub mine: Vec<MyGenre>,
    /// Play-weighted average colour of `mine`, blended in linear light — what
    /// your listening sounds like.
    #[schema(example = "#a18430")]
    pub colour: Option<String>,
    pub placed_tags: usize,
    /// Plays on tags with no place on the map — instruments, nationalities,
    /// roles.
    pub unplaced_plays: i64,
    /// So a half-backfilled history says so rather than looking like a small
    /// life.
    pub listens_with_genres: i64,
    pub listens_total: i64,
}

#[derive(Serialize, ToSchema)]
pub struct MapBackdrop {
    pub width: f64,
    pub height: f64,
    #[schema(schema_with = map_points)]
    pub points: Vec<(String, f64, f64, String)>,
}

fn map_points() -> utoipa::openapi::schema::Array {
    use utoipa::openapi::schema::{ArrayBuilder, ObjectBuilder, OneOfBuilder, Type};
    let cell = OneOfBuilder::new()
        .item(ObjectBuilder::new().schema_type(Type::String))
        .item(ObjectBuilder::new().schema_type(Type::Number));
    ArrayBuilder::new()
        .description(Some(
            "`[name, x, y, colour]` per genre — positional to keep ~6,300 rows small.",
        ))
        .items(ArrayBuilder::new().items(cell).min_items(Some(4)).max_items(Some(4)))
        .build()
}

#[derive(Serialize, ToSchema)]
pub struct MyGenre {
    pub name: String,
    pub plays: i64,
    pub x: f64,
    pub y: f64,
    pub color: String,
}

/// The play-weighted centre of each period's listening, on the map's
/// coordinates.
#[derive(Serialize, ToSchema)]
pub struct GenreTrajectory {
    pub years: Vec<TrajectoryPoint>,
    /// `year` or `month`.
    pub unit: &'static str,
    /// Periods with fewer placed plays than this are left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_plays: Option<i64>,
    /// Set when nothing could be drawn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

#[derive(Serialize, ToSchema)]
pub struct TrajectoryPoint {
    /// `2024`, or `2024-03` by month.
    pub year: String,
    pub x: f64,
    pub y: f64,
    pub plays: i64,
    pub colour: Option<String>,
    /// Listens in the period…
    pub listens: i64,
    /// …and how many of them have genres resolved. A thin period is labelled
    /// rather than drawn as if it were complete.
    pub enriched: i64,
}

/// Genres bordering your listening, split by whether you already own music
/// there.
#[derive(Serialize, ToSchema)]
pub struct GenreNeighbours {
    /// Bordering genres you already own music in and underplay. This half
    /// leads.
    pub next_door: Vec<NeighbourGenre>,
    /// Bordering genres you own nothing in — a region, never a suggestion.
    pub unexplored: Vec<NeighbourGenre>,
    /// Furthest from your listening. Suggests nothing by construction.
    pub opposite: Vec<NeighbourGenre>,
    /// How many of your top genres the walk started from.
    pub seeded_from: usize,
}

/// A genre bordering your listening. **`your_artists` and `exemplar_artists`
/// are never both populated**, so nothing can present an Every Noise exemplar
/// as something the listener owns.
#[derive(Serialize, ToSchema)]
pub struct NeighbourGenre {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub color: String,
    /// How many of your top genres border it.
    pub borders: usize,
    /// `plays / sqrt(rank + 1)`, summed: weighted by your play counts and
    /// discounted by Every Noise's own neighbour rank.
    pub affinity: f64,
    pub your_plays: i64,
    /// From **your own history**. Empty is what puts a genre in `unexplored`.
    pub your_artists: Vec<ArtistPlays>,
    /// **Every Noise's exemplars — not your music.** Set only where
    /// `your_artists` is empty. Requires a tier-2 scrape; empty otherwise.
    pub exemplar_artists: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ArtistPlays {
    pub artist: String,
    pub plays: i64,
}

#[derive(Serialize, ToSchema)]
pub struct EveryNoiseDone {
    /// `saved`, `stopping` or `cleared`.
    pub status: &'static str,
}

/// Where the admin's contact address for the scraper's `User-Agent` lives.
pub const CONTACT_KEY: &str = "everynoise_contact";

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

/// The configured contact address, database first and environment second —
/// the same order the Discogs token uses.
async fn contact(state: &AppState) -> Option<String> {
    if let Ok(Some(c)) = state.db.get_app_setting(CONTACT_KEY).await {
        let c = c.trim().to_string();
        if !c.is_empty() {
            return Some(c);
        }
    }
    std::env::var("TAPEDECK_EVERYNOISE_CONTACT").ok().filter(|c| !c.trim().is_empty())
}

/// Enough of an address to be a way of reaching someone.
///
/// Not a validity check — that is not decidable — just a guard against a
/// placeholder being sent to a stranger's server as if it meant something.
fn looks_like_an_address(s: &str) -> bool {
    let s = s.trim();
    match s.split_once('@') {
        Some((user, host)) => {
            !user.is_empty() && host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
        }
        None => false,
    }
}

// GET /admin/everynoise — what is held, whether a walk is running.
/// Genre-map scrape status.
///
/// Session-only, admin role.
#[utoipa::path(
    operation_id = "everynoise_status",
    get,
    path = "/admin/everynoise",
    tag = "Admin",
    responses(
        (status = 200, description = "OK.", body = EveryNoiseStatus),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
pub async fn status(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let summary = match state.everynoise.summary().await {
        Ok(s) => s,
        Err(e) => {
            error!("Every Noise summary failed: {}", e);
            return err(500, "Could not read the genre map database");
        }
    };
    let contact = contact(&state).await;
    (
        StatusCode::OK,
        Json(EveryNoiseStatus {
            summary,
            progress: state.everynoise_jobs.get().await,
            // Echoed back so the field can be pre-filled; it is the admin's own
            // address, not a credential.
            contact,
            source: "https://everynoise.com/",
        }),
    )
        .into_response()
}

#[derive(Deserialize, ToSchema)]
#[schema(as = EveryNoiseContact)]
pub struct ContactBody {
    /// An email address, sent in the scraper's `User-Agent` so the site's
    /// owner can reach whoever is fetching. Empty clears it.
    pub contact: String,
}

/// Set the admin contact email for the scraper's User-Agent.
///
/// Session-only, admin role. The scrape **refuses to start without one** —
/// a made-up address is worse than none.
#[utoipa::path(
    put,
    path = "/admin/everynoise/contact",
    tag = "Admin",
    request_body = ContactBody,
    responses(
        (status = 200, description = "Saved.", body = EveryNoiseDone),
        (status = 400, description = "Not a plausible address.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
pub async fn set_contact(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<ContactBody>,
) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    let value = body.contact.trim().to_string();
    if !value.is_empty() && !looks_like_an_address(&value) {
        return err(400, "That doesn't look like an email address");
    }
    match state.db.set_app_setting(CONTACT_KEY, &value).await {
        Ok(_) => (StatusCode::OK, Json(EveryNoiseDone { status: "saved" })).into_response(),
        Err(e) => {
            error!("Could not save the Every Noise contact: {}", e);
            err(500, "Could not save that")
        }
    }
}

#[derive(Deserialize, ToSchema)]
#[schema(as = ScrapeRequest)]
pub struct StartBody {
    /// `map` (one request) or `pages` (~6,300, about two hours). `pages`
    /// needs the map first.
    #[schema(example = "map")]
    pub tier: String,
}

/// Start a scrape.
///
/// Session-only, admin role. Runs when an admin asks and **never on a
/// schedule** — Every Noise has been frozen since December 2023, so there
/// is nothing to sync.
///
/// **Tier 1 is one HTTP request** and returns every genre with coordinates
/// and colour; genre-level features cost that alone. **Tier 2 walks ~6,300
/// genre pages at 1 req/s** — about two hours — for exemplar artists and
/// the adjacency graph. Build on tier 1 first.
///
/// A fetch that parses to **zero** genres is treated as a failure rather
/// than written, or a changed page would silently empty the table. Resume
/// is per page, written in the same transaction as the page's contents, so
/// a crash can never mark a page done with nothing in it. A second
/// concurrent start is refused.
#[utoipa::path(
    operation_id = "everynoise_start",
    post,
    path = "/admin/everynoise/scrape",
    tag = "Admin",
    request_body = StartBody,
    responses(
        (status = 202, description = "Started.", body = ScrapeStarted),
        (status = 400, description = "No admin contact set, or an unknown tier.", body = ErrorResponse),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 409, description = "A scrape is already running, or `pages` was asked for before the map.", body = ErrorResponse),
    )
)]
pub async fn start(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(body): Json<StartBody>,
) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }

    let tier = match body.tier.as_str() {
        "map" => Tier::Map,
        "pages" => Tier::Pages,
        other => return err(400, format!("Unknown tier '{}'", other)),
    };

    // One walk at a time. Two would contend for the same rows and double the
    // traffic aimed at a site that cannot benefit from either.
    if state.everynoise_jobs.is_running().await {
        return err(409, "A scrape is already running");
    }

    // Refuse without a contact address rather than substituting a default.
    // Sending an anonymous — or invented — identity to a frozen personal site
    // 6,300 times is the thing that gets a project blocked.
    let Some(contact) = contact(&state).await else {
        return err(
            400,
            "Set a contact email first — it identifies this instance to everynoise.com",
        );
    };

    if tier == Tier::Pages {
        match state.everynoise.summary().await {
            Ok(s) if s.genres == 0 => {
                return err(
                    409,
                    "Fetch the genre map first — the per-genre walk works from the list it produces",
                );
            }
            Err(e) => {
                error!("Every Noise summary failed: {}", e);
                return err(500, "Could not read the genre map database");
            }
            _ => {}
        }
    }

    let store = state.everynoise.clone();
    let registry = state.everynoise_jobs.clone();
    tokio::spawn(async move {
        let result = match tier {
            Tier::Map => scrape::run_map(store, registry, contact).await,
            Tier::Pages => scrape::run_pages(store, registry, contact).await,
        };
        if let Err(e) = result {
            error!("Every Noise scrape failed: {}", e);
        }
    });

    (StatusCode::ACCEPTED, Json(ScrapeStarted { status: "started", tier })).into_response()
}

/// Cancel a running scrape.
///
/// Session-only, admin role. Progress already written is kept — resume is per
/// page.
#[utoipa::path(
    operation_id = "everynoise_cancel",
    post,
    path = "/admin/everynoise/cancel",
    tag = "Admin",
    responses(
        (status = 200, description = "Cancelled.", body = EveryNoiseDone),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
pub async fn cancel(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    state.everynoise_jobs.cancel().await;
    (StatusCode::OK, Json(EveryNoiseDone { status: "stopping" })).into_response()
}

// DELETE /admin/everynoise — drop the local copy.
/// Drop the scraped genre map.
///
/// Session-only, admin role. The genre map lives in its **own database**
/// (`EVERYNOISE_DB_PATH`), kept out of `VACUUM INTO` backups — those should
/// hold the user's listening, not 6,300 rows of someone else's reference
/// data.
#[utoipa::path(
    operation_id = "everynoise_clear",
    delete,
    path = "/admin/everynoise",
    tag = "Admin",
    responses(
        (status = 200, description = "Cleared.", body = EveryNoiseDone),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
        (status = 409, description = "A scrape is running; stop it first.", body = ErrorResponse),
    )
)]
pub async fn clear(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    if state.everynoise_jobs.is_running().await {
        return err(409, "Stop the running scrape first");
    }
    match state.everynoise.clear().await {
        Ok(_) => (StatusCode::OK, Json(EveryNoiseDone { status: "cleared" })).into_response(),
        Err(e) => {
            error!("Could not clear the genre map: {}", e);
            err(500, "Could not clear it")
        }
    }
}

/// Average a set of `#rrggbb` colours, weighted by plays.
///
/// Every Noise encodes audio character in the three channels, so a
/// play-weighted average really is "what your listening sounds like" rather
/// than a decorative accent — which is why this is worth a swatch of its own.
///
/// Averaged in **linear light**, not in sRGB. Mixing gamma-encoded values
/// directly is the classic mistake: it drags everything toward mud, and with a
/// few hundred genres in the mix the result would be the same brown-grey for
/// everybody, which would make the feature look broken rather than personal.
pub(crate) fn blend(weighted: &[(&str, i64)]) -> Option<String> {
    let to_linear = |c: f64| {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let to_srgb = |c: f64| {
        let c = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (c.clamp(0.0, 1.0) * 255.0).round() as u8
    };

    let (mut r, mut g, mut b, mut total) = (0.0, 0.0, 0.0, 0i64);
    for (hex, plays) in weighted {
        let h = hex.trim_start_matches('#');
        if h.len() != 6 || *plays <= 0 {
            continue;
        }
        let ch = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f64);
        let (Some(cr), Some(cg), Some(cb)) = (ch(0), ch(2), ch(4)) else { continue };
        let w = *plays as f64;
        r += to_linear(cr) * w;
        g += to_linear(cg) * w;
        b += to_linear(cb) * w;
        total += plays;
    }
    (total > 0).then(|| {
        let t = total as f64;
        format!("#{:02x}{:02x}{:02x}", to_srgb(r / t), to_srgb(g / t), to_srgb(b / t))
    })
}

// GET /api/v1/genre-map — the whole map, and where the caller lives on it.
//
// Not admin-only: the map itself is instance-wide reference data, but the
// weighting is entirely the caller's own listening, so this is scoped by
// `user_id` like every other read.
/// The map, your genres on it, and your average colour.
///
/// Session-only. The canvas is 1500 × 22,648 — fifteen times taller than
/// wide — so a client must compress the y axis **in the coordinates**, not
/// with `preserveAspectRatio`, which would squash the circles into ellipses
/// too.
///
/// `colour` is blended in **linear light**, not sRGB; mixing gamma-encoded
/// values drags every user toward the same brown-grey.
#[utoipa::path(
    get,
    path = "/api/v1/genre-map",
    tag = "Genre map",
    responses(
        (status = 200, description = "OK.", body = GenreMap),
        (status = 401, response = Unauthorized),
        (status = 409, description = "The map has not been fetched yet. Run the scrape in Settings.", body = ErrorResponse),
    )
)]
pub async fn genre_map(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    let backdrop = match state.everynoise.all_points().await {
        Ok(p) => p,
        Err(e) => {
            error!("Genre map read failed: {}", e);
            return err(500, "Could not read the genre map");
        }
    };
    if backdrop.is_empty() {
        return err(
            409,
            "The genre map hasn't been fetched yet — an admin can do it in Settings → Genre Map",
        );
    }

    let counts = state.db.genre_play_counts(user.user_id, None).await.unwrap_or_default();
    let names: Vec<String> = counts.iter().map(|(n, _)| n.clone()).collect();
    let coords = match state.everynoise.coordinates_for(&names).await {
        Ok(c) => c,
        Err(e) => {
            error!("Genre map join failed: {}", e);
            return err(500, "Could not place your listening on the map");
        }
    };

    // The caller's genres that have a position. Tags with no place on the map
    // (instruments, nationalities, roles — "saxophone", "french") simply drop
    // out; they are not genres and there is nowhere to put them.
    let mut mine = Vec::new();
    let mut unplaced = 0i64;
    for (name, plays) in &counts {
        match coords.get(&crate::everynoise::store::genre_key(name)) {
            Some((x, y, color)) => mine.push(MyGenre {
                name: name.clone(),
                plays: *plays,
                x: *x,
                y: *y,
                color: color.clone(),
            }),
            None => unplaced += plays,
        }
    }

    let weighted: Vec<(&str, i64)> = counts
        .iter()
        .filter_map(|(n, p)| {
            coords.get(&crate::everynoise::store::genre_key(n)).map(|(_, _, c)| (c.as_str(), *p))
        })
        .collect();
    let colour = blend(&weighted);
    let placed = weighted.len();

    let (w, h) = state.everynoise.extent().await.unwrap_or((1500.0, 22648.0));
    let (with_genres, total) =
        state.db.genre_backfill_progress(user.user_id).await.unwrap_or((0, 0));

    (
        StatusCode::OK,
        Json(GenreMap {
            // Compact tuples rather than objects: 6,300 of these, and the
            // difference between [n,x,y,c] and four keys apiece is most of the
            // payload.
            map: MapBackdrop { width: w, height: h, points: backdrop },
            mine,
            colour,
            placed_tags: placed,
            unplaced_plays: unplaced,
            // So a half-backfilled history says so rather than looking like a
            // small life.
            listens_with_genres: with_genres,
            listens_total: total,
        }),
    )
        .into_response()
}

/// Running totals for one period of the trajectory: the play-weighted sums of
/// x and y, the plays behind them, and the colours to blend.
#[derive(Default)]
struct PeriodAccumulator {
    x: f64,
    y: f64,
    plays: i64,
    colours: Vec<(String, i64)>,
}

/// A period needs this many placed plays before it gets a point.
///
/// A centroid computed from four listens is not a fact about a year, it is a
/// fact about four listens — and drawn as part of a path it reads as a real
/// movement in taste. Better to leave the period out and say so.
const MIN_PLAYS_PER_PERIOD: i64 = 25;

/// Below this many distinct years, bucket by month instead.
///
/// Measured on a real history: 21,390 listens spanning 2025–2026 give a yearly
/// trajectory of **two points**, which is a line segment rather than a story.
/// The same history by month is ~20 points and an actual path. Granularity has
/// to follow the data — a fixed choice is wrong at one end or the other.
const MONTHS_BELOW_YEARS: usize = 4;

/// GET /api/v1/genre-map/trajectory — where your listening sat, year by year.
///
/// The play-weighted centroid of each year's genres, which is the closest thing
/// the data supports to "how your taste moved". Two honesty constraints, both
/// of which matter more than they look:
///
/// * The granularity is really **artist-level**. Every track by one artist
///   carries that artist's genres and therefore sits at the same point, so this
///   is an *artist-mix* trajectory. That is still taste evolution — your mix
///   shifting is the thing — but it is not track-level and should not be
///   described as such.
/// * The genre backfill runs newest-first over hours, so a half-enriched
///   history has recent years populated and older ones empty. A trajectory
///   slices by time, which makes it the feature most distorted by that: it
///   would look like the listening only started recently. Each year therefore
///   reports its own enrichment coverage, and the client says so.
#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TrajectoryQuery {
    /// `year` or `month`. Omitted lets the server pick from the span: by
    /// month under a few years of history, by year otherwise.
    pub by: Option<String>,
}

/// Where your listening sat, period by period.
///
/// Session-only. The play-weighted centroid of each period.
///
/// Three honesty constraints, all load-bearing. It is an **artist-mix**
/// trajectory, not a track-level one — every track by an artist carries
/// that artist's genres and sits at the same point. A period under 25
/// placed plays is **omitted, not drawn**, because a centroid from four
/// listens reads as a real movement inside a path. And each period reports
/// its own enrichment coverage, to be drawn faintly below 90% — the genre
/// backfill runs newest-first, so a time-sliced view is the one most
/// distorted by a half-done pass.
///
/// The server picks month or year by how much history there is and says
/// which it used. Buckets are in the user's own timezone.
#[utoipa::path(
    get,
    path = "/api/v1/genre-map/trajectory",
    tag = "Genre map",
    params(TrajectoryQuery),
    responses(
        (status = 200, description = "OK.", body = GenreTrajectory),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn genre_trajectory(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    axum::extract::Query(q): axum::extract::Query<TrajectoryQuery>,
) -> impl IntoResponse {
    let tz = super::scrobbles::user_tz_for(&state, user.user_id).await;
    // SQLite's `strftime` modifier wants a signed hour offset; derive it from
    // the zone's *current* offset, which is right to the year boundary except
    // for a listen inside the DST hour of New Year — which no zone has.
    let offset_hours = {
        use chrono::Offset;
        let now = chrono::Utc::now().with_timezone(&tz);
        now.offset().fix().local_minus_utc() as f64 / 3600.0
    };

    // How many distinct years are there? Cheap, and it decides the granularity.
    let by_year = state.db.listens_by_period(user.user_id, offset_hours, "%Y").await;
    let year_count = by_year.as_ref().map(|v| v.len()).unwrap_or(0);
    let unit = match q.by.as_deref() {
        Some("month") => "month",
        Some("year") => "year",
        _ if year_count < MONTHS_BELOW_YEARS => "month",
        _ => "year",
    };
    let fmt = if unit == "month" { "%Y-%m" } else { "%Y" };

    let rows = match state.db.genre_plays_by_period(user.user_id, offset_hours, fmt).await {
        Ok(r) => r,
        Err(e) => {
            error!("Trajectory query failed: {}", e);
            return err(500, "Could not read your listening over time");
        }
    };
    if rows.is_empty() {
        return (
            StatusCode::OK,
            Json(GenreTrajectory {
                years: Vec::new(),
                unit,
                min_plays: None,
                note: Some("no genres resolved yet"),
            }),
        )
            .into_response();
    }

    let names: Vec<String> = {
        let mut n: Vec<String> = rows.iter().map(|(_, g, _)| g.clone()).collect();
        n.sort();
        n.dedup();
        n
    };
    let coords = match state.everynoise.coordinates_for(&names).await {
        Ok(c) => c,
        Err(e) => {
            error!("Trajectory join failed: {}", e);
            return err(500, "Could not place your listening on the map");
        }
    };

    // Coverage per year, so a thin year can be labelled rather than drawn as if
    // it were complete.
    let coverage: std::collections::HashMap<String, (i64, i64)> = state
        .db
        .listens_by_period(user.user_id, offset_hours, fmt)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(y, listens, enriched)| (y, (listens, enriched)))
        .collect();

    // Accumulate a weighted centroid per year, and the colours that go with it.
    let mut per_year: std::collections::BTreeMap<String, PeriodAccumulator> = Default::default();
    for (year, genre, plays) in &rows {
        let Some((x, y, colour)) = coords.get(&crate::everynoise::store::genre_key(genre)) else {
            continue;
        };
        let e = per_year.entry(year.clone()).or_default();
        e.x += x * *plays as f64;
        e.y += y * *plays as f64;
        e.plays += plays;
        e.colours.push((colour.clone(), *plays));
    }

    let years: Vec<TrajectoryPoint> = per_year
        .into_iter()
        .filter(|(_, a)| a.plays >= MIN_PLAYS_PER_PERIOD)
        .map(|(year, a)| {
            let (listens, enriched) = coverage.get(&year).copied().unwrap_or((0, 0));
            let weighted: Vec<(&str, i64)> =
                a.colours.iter().map(|(c, p)| (c.as_str(), *p)).collect();
            TrajectoryPoint {
                colour: blend(&weighted),
                year,
                x: a.x / a.plays as f64,
                y: a.y / a.plays as f64,
                plays: a.plays,
                listens,
                enriched,
            }
        })
        .collect();

    (
        StatusCode::OK,
        Json(GenreTrajectory { years, unit, min_plays: Some(MIN_PLAYS_PER_PERIOD), note: None }),
    )
        .into_response()
}

/// How many of the caller's top genres seed the neighbour walk.
const SEED_GENRES: usize = 25;

/// How many Every Noise exemplars a genre the caller owns nothing in carries.
/// Kept small on purpose: enough to give a bare genre name a texture, few
/// enough that the card cannot be mistaken for a list of things to go and get.
const EXEMPLARS_PER_GENRE: usize = 4;

/// How broad is a listening history, in two numbers that mean different things.
///
/// **Not a grid.** The obvious measure — bin the map into cells and count how
/// many you occupy — was built and rejected: on a real history it returns
/// anywhere from 5% to 34% depending purely on the cell size, which has no
/// principled value. A number dominated by a parameter someone invented is a
/// confident-looking way of saying nothing, and the naive alternative
/// (`genres played / 6,291`) is worse still: 1.8%, which is both meaningless
/// and demoralising, because Every Noise's genres are hyper-granular and nobody
/// covers a meaningful share of them.
///
/// Both numbers below are scale-free and parameter-free, and both degenerate
/// correctly: someone who only ever played one genre scores 1.0 and 0.0.
///
/// * **Effective genres** — `exp(H)` where `H` is Shannon entropy over play
///   counts. The standard diversity measure, and it reads directly: "as varied
///   as playing N genres equally". It answers *how evenly you listen*, and
///   needs no map at all.
/// * **Reach** — your play-weighted RMS distance from your own centre, over the
///   map's RMS distance from its centre. It answers *how far apart in genre
///   space* your listening is, which is a different question: a person can play
///   forty genres that all sit in one corner.
pub(crate) fn breadth(
    counts: &[(String, i64)],
    coords: &std::collections::HashMap<String, (f64, f64, String)>,
    map_spread: f64,
) -> Breadth {
    let total: i64 = counts.iter().map(|(_, p)| p).sum();
    if total == 0 {
        return Breadth {
            genres_played: 0,
            genres_placed: None,
            effective_genres: None,
            evenness: None,
            reach: None,
        };
    }

    // Evenness is over *everything* played, map or not — an untagged-by-Every-
    // Noise genre is still a genre you listened to.
    let entropy: f64 = counts
        .iter()
        .map(|(_, p)| *p as f64 / total as f64)
        .filter(|w| *w > 0.0)
        .map(|w| -w * w.ln())
        .sum();
    let effective = entropy.exp();

    // Reach needs positions, so it is over the placed genres only.
    let placed: Vec<(f64, f64, f64)> = counts
        .iter()
        .filter_map(|(name, plays)| {
            coords
                .get(&crate::everynoise::store::genre_key(name))
                .map(|(x, y, _)| (*x, *y, *plays as f64))
        })
        .collect();
    let placed_total: f64 = placed.iter().map(|(_, _, w)| w).sum();

    let reach = (placed_total > 0.0 && map_spread > 0.0).then(|| {
        let cx: f64 = placed.iter().map(|(x, _, w)| x * w).sum::<f64>() / placed_total;
        let cy: f64 = placed.iter().map(|(_, y, w)| y * w).sum::<f64>() / placed_total;
        let var: f64 =
            placed.iter().map(|(x, y, w)| w * ((x - cx).powi(2) + (y - cy).powi(2))).sum::<f64>()
                / placed_total;
        (var.sqrt() / map_spread * 1000.0).round() / 1000.0
    });

    Breadth {
        genres_played: counts.len(),
        genres_placed: Some(placed.len()),
        effective_genres: Some((effective * 10.0).round() / 10.0),
        // 1.0 would mean every genre played exactly equally.
        evenness: Some(((effective / counts.len() as f64) * 1000.0).round() / 1000.0),
        reach: Some(reach),
    }
}

/// How widely you listen across the genre map. Every field but
/// `genres_played` is absent when nothing was played.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Breadth {
    pub genres_played: usize,
    /// Of those, the ones the map places.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genres_placed: Option<usize>,
    /// `exp(H)` over play counts — as varied as playing N genres equally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_genres: Option<f64>,
    /// `effective_genres` over `genres_played`, 0–1. 1.0 would mean every
    /// genre played exactly equally.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evenness: Option<f64>,
    /// Your play-weighted spread over the map's own spread, 0–1. Null when
    /// nothing played is on the map.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<f64>)]
    pub reach: Option<Option<f64>>,
}

// GET /api/v1/genre-map/neighbours — what sits next to your listening.
//
// Every Noise precomputes, per genre, its ~20 nearest and ~20 maximally
// opposite genres. That graph is the tier-2 walk's real payoff — it is a
// *global* signal, unlike the artist coordinates from the same pages, which
// are page-local and cannot be compared across genres.
//
// **This does not recommend music.** The vision rule is that Tapedeck never
// points at things the listener does not own, and a genre map makes it very
// easy to do so accidentally. So the response is split by whether the caller
// *already has music there*, and the two halves carry artists in **two
// different fields that must never be merged**:
//
// * `your_artists` — from the caller's own listening history. Only ever set on
//   `next_door`, which is the half that leads, because "you have 243 plays of
//   boy band and it is not in your top 25" is the actionable finding.
// * `exemplar_artists` — Every Noise's own exemplars for a genre the caller
//   owns *nothing* in. These name music they do not have, so the field is
//   separate rather than shared: any consumer that renders them without
//   saying where they came from is doing it wrong, and a distinct field name
//   is what makes that visible at the call site rather than a convention to
//   remember. Empty until a tier-2 walk has run.
//
// The original design left unowned genres with no artist field at all, which
// enforced the rule by shape. Labelling replaced that (2026-08-06, at
// Ashwin's direction): a bordering region reduced to one word on a pill said
// less than the rule was protecting, and the honest fix is provenance, not
// omission. The shape still enforces the half that matters — nothing can
// present an Every Noise exemplar as something the listener owns.
/// What sits next to your listening.
///
/// Session-only. Walks Every Noise's precomputed adjacency, seeded from
/// your top 25 genres.
///
/// **This does not recommend music**, and the response shape is what
/// enforces that. Two artist fields that are never both populated on the
/// same card:
///
/// * `your_artists` — from **your own history**. Only ever set on
///   `next_door`.
/// * `exemplar_artists` — **Every Noise's own exemplars**, for a genre you
///   own nothing in. These name music you do not have. Any client showing
///   them owes the reader that provenance, in words and in styling.
///
/// `exemplar_artists` is empty until a tier-2 scrape has run — a
/// tier-1-only instance has adjacency but no exemplars. Say so rather than
/// rendering bare cards that look like a bug.
#[utoipa::path(
    get,
    path = "/api/v1/genre-map/neighbours",
    tag = "Genre map",
    responses(
        (status = 200, description = "OK.", body = GenreNeighbours),
        (status = 401, response = Unauthorized),
        (status = 409, description = "None of your genres are on the map yet.", body = ErrorResponse),
    )
)]
pub async fn genre_neighbours(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
) -> impl IntoResponse {
    let counts = state.db.genre_play_counts(user.user_id, None).await.unwrap_or_default();
    if counts.is_empty() {
        // The full shape, empty. This used to be `{nearby, opposite}` — a key
        // the real answer has never had — so the map page read
        // `next_door.length` off `undefined` for anybody with no genres yet.
        return (
            StatusCode::OK,
            Json(GenreNeighbours {
                next_door: Vec::new(),
                unexplored: Vec::new(),
                opposite: Vec::new(),
                seeded_from: 0,
            }),
        )
            .into_response();
    }

    let all_names: Vec<String> = counts.iter().map(|(n, _)| n.clone()).collect();
    let slugs = match state.everynoise.slugs_for(&all_names).await {
        Ok(s) => s,
        Err(e) => {
            error!("Neighbour slug lookup failed: {}", e);
            return err(500, "Could not read the genre map");
        }
    };
    if slugs.is_empty() {
        return err(409, "None of your genres are on the map yet");
    }

    // Plays per genre, and the slug each maps to. Seeded from the top genres
    // only: walking all of them would return most of the map and mean nothing.
    let mut plays_by_slug: std::collections::HashMap<String, i64> = Default::default();
    for (name, plays) in &counts {
        if let Some(slug) = slugs.get(&crate::everynoise::store::genre_key(name)) {
            *plays_by_slug.entry(slug.clone()).or_default() += plays;
        }
    }
    let mut seeds: Vec<(String, i64)> =
        plays_by_slug.iter().map(|(s, p)| (s.clone(), *p)).collect();
    seeds.sort_by(|a, b| b.1.cmp(&a.1));
    seeds.truncate(SEED_GENRES);
    let seed_slugs: Vec<String> = seeds.iter().map(|(s, _)| s.clone()).collect();

    // The caller's own artists, keyed by genre — the only source an artist name
    // in this response may come from.
    let mut mine_by_genre: std::collections::HashMap<String, Vec<(String, i64)>> =
        Default::default();
    for (genre, artist, plays) in state.db.artists_by_genre(user.user_id).await.unwrap_or_default()
    {
        mine_by_genre
            .entry(crate::everynoise::store::genre_key(&genre))
            .or_default()
            .push((artist, plays));
    }

    let nearby_edges =
        state.everynoise.neighbours_of(&seed_slugs, "nearby").await.unwrap_or_default();
    let opposite_edges =
        state.everynoise.neighbours_of(&seed_slugs, "opposite").await.unwrap_or_default();

    // Every Noise's own exemplars, fetched once for both relations. Keyed by
    // the neighbour's slug rather than its name, because a genre the caller
    // owns nothing in — which is exactly the set this exists for — never
    // appears in `slugs`, built from their own vocabulary.
    let reachable: Vec<String> = {
        let mut v: Vec<String> = nearby_edges
            .iter()
            .chain(opposite_edges.iter())
            .map(|(_, slug, ..)| slug.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let exemplars =
        state.everynoise.exemplars_for(&reachable, EXEMPLARS_PER_GENRE).await.unwrap_or_default();

    let collect = |edges: Vec<(String, String, String, f64, f64, String, i64)>,
                   seeds: &[(String, i64)],
                   plays_by_slug: &std::collections::HashMap<String, i64>,
                   mine: &std::collections::HashMap<String, Vec<(String, i64)>>|
     -> Vec<NeighbourGenre> {
        let seed_plays: std::collections::HashMap<&str, i64> =
            seeds.iter().map(|(s, p)| (s.as_str(), *p)).collect();

        // Affinity: a genre bordering several of your biggest genres, closely,
        // is more "next door" than one bordering a single small genre at rank
        // 19. Weighting by the seed's plays and discounting by rank says that.
        let mut score: std::collections::HashMap<String, (f64, f64, f64, String, usize, String)> =
            Default::default();
        for (from, slug, name, x, y, colour, position) in edges {
            let w = *seed_plays.get(from.as_str()).unwrap_or(&0) as f64
                / (position as f64 + 1.0).sqrt();
            let e = score.entry(name).or_insert((0.0, x, y, colour, 0, slug));
            e.0 += w;
            e.4 += 1;
        }

        let mut out: Vec<_> = score.into_iter().collect();
        out.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap_or(std::cmp::Ordering::Equal));
        out.into_iter()
            .filter_map(|(name, (affinity, x, y, colour, borders, slug))| {
                let key = crate::everynoise::store::genre_key(&name);
                // Genres you already play a lot are not "next door", they are
                // where you live — the map above already shows those.
                let slug_plays =
                    slugs.get(&key).and_then(|s| plays_by_slug.get(s)).copied().unwrap_or(0);
                let mut yours: Vec<&(String, i64)> =
                    mine.get(&key).map(|v| v.iter().collect()).unwrap_or_default();
                yours.sort_by(|a, b| b.1.cmp(&a.1));

                // Exemplars fill in exactly where the caller's own history has
                // nothing to say, and never alongside it — a card showing both
                // would be the one place a reader could confuse the two.
                let exemplar_artists: Vec<String> = if yours.is_empty() {
                    exemplars.get(&slug).cloned().unwrap_or_default()
                } else {
                    Vec::new()
                };

                Some(NeighbourGenre {
                    name,
                    x,
                    y,
                    color: colour,
                    borders,
                    affinity: (affinity * 10.0).round() / 10.0,
                    your_plays: slug_plays,
                    // Strictly the caller's own artists. Empty is a real answer
                    // and it is what splits the response — see `exemplar_artists`.
                    your_artists: yours
                        .iter()
                        .take(4)
                        .map(|(a, p)| ArtistPlays { artist: a.clone(), plays: *p })
                        .collect(),
                    // Every Noise's exemplars. **Not the caller's music.** Any
                    // consumer rendering these owes the reader that provenance.
                    exemplar_artists,
                })
            })
            .collect()
    };

    let mut nearby = collect(nearby_edges, &seeds, &plays_by_slug, &mine_by_genre);
    // Drop the genres that *are* your listening — bordering yourself is not a
    // neighbourhood.
    nearby.retain(|g| {
        let k = crate::everynoise::store::genre_key(&g.name);
        !seed_slugs.iter().any(|s| slugs.get(&k) == Some(s))
    });
    // Split rather than sorted, because the two halves answer different
    // questions and ranking them together buries the useful one. Sorting by
    // affinity alone puts genres you own *nothing* in at the top — the least
    // actionable rows, since the vision rule means there is nothing to point at
    // in them. "You have 243 plays of boy band and it is not in your top 25" is
    // the finding; everything else is context.
    let (owned, unexplored): (Vec<_>, Vec<_>) =
        nearby.into_iter().partition(|g| !g.your_artists.is_empty());
    let mut owned = owned;
    let mut unexplored = unexplored;
    owned.truncate(16);
    unexplored.truncate(16);

    let mut opposite = collect(opposite_edges, &seeds, &plays_by_slug, &mine_by_genre);
    opposite.truncate(12);

    (
        StatusCode::OK,
        Json(GenreNeighbours {
            // Bordering genres you already have music in, underplayed.
            next_door: owned,
            // Bordering genres you own nothing in. `your_artists` is empty by
            // construction here; `exemplar_artists` is Every Noise's, and has
            // to be presented as such.
            unexplored,
            opposite,
            seeded_from: seeds.len(),
        }),
    )
        .into_response()
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    status,
    clear,
    set_contact,
    start,
    cancel,
    genre_map,
    genre_trajectory,
    genre_neighbours
))]
pub struct Api;

pub fn create_everynoise_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/everynoise", get(status).delete(clear))
        .route("/admin/everynoise/contact", put(set_contact))
        .route("/admin/everynoise/scrape", post(start))
        .route("/admin/everynoise/cancel", post(cancel))
        // Not admin: the map is shared reference data, the weighting is the
        // caller's own listening.
        .route("/api/v1/genre-map", get(genre_map))
        .route("/api/v1/genre-map/trajectory", get(genre_trajectory))
        .route("/api/v1/genre-map/neighbours", get(genre_neighbours))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_is_not_accepted_as_a_contact() {
        assert!(looks_like_an_address("someone@example.com"));
        assert!(looks_like_an_address("a.b+c@sub.domain.co.uk"));

        // These would all be sent to a stranger's server as if they meant
        // something.
        assert!(!looks_like_an_address(""));
        assert!(!looks_like_an_address("nobody"));
        assert!(!looks_like_an_address("@example.com"));
        assert!(!looks_like_an_address("someone@localhost"));
        assert!(!looks_like_an_address("someone@.com"));
    }
}
