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
use serde::Deserialize;
use std::sync::Arc;
use tracing::error;

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::everynoise::scrape::{self, Tier};

/// Where the admin's contact address for the scraper's `User-Agent` lives.
pub const CONTACT_KEY: &str = "everynoise_contact";

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
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

/// GET /admin/everynoise — what is held, whether a walk is running.
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
        Json(serde_json::json!({
            "summary": summary,
            "progress": state.everynoise_jobs.get().await,
            // Echoed back so the field can be pre-filled; it is the admin's own
            // address, not a credential.
            "contact": contact,
            "source": "https://everynoise.com/",
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct ContactBody {
    pub contact: String,
}

/// PUT /admin/everynoise/contact
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
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "status": "saved" }))).into_response(),
        Err(e) => {
            error!("Could not save the Every Noise contact: {}", e);
            err(500, "Could not save that")
        }
    }
}

#[derive(Deserialize)]
pub struct StartBody {
    /// `map` (one request) or `pages` (~6,300, about two hours).
    pub tier: String,
}

/// POST /admin/everynoise/scrape
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

    (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "started", "tier": tier })))
        .into_response()
}

/// POST /admin/everynoise/cancel
pub async fn cancel(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    state.everynoise_jobs.cancel().await;
    (StatusCode::OK, Json(serde_json::json!({ "status": "stopping" }))).into_response()
}

/// DELETE /admin/everynoise — drop the local copy.
pub async fn clear(State(state): State<Arc<AppState>>, user: AuthUser) -> impl IntoResponse {
    if !user.is_admin {
        return err(403, "Admin only");
    }
    if state.everynoise_jobs.is_running().await {
        return err(409, "Stop the running scrape first");
    }
    match state.everynoise.clear().await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "status": "cleared" }))).into_response(),
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
fn blend(weighted: &[(&str, i64)]) -> Option<String> {
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

/// GET /api/v1/genre-map — the whole map, and where the caller lives on it.
///
/// Not admin-only: the map itself is instance-wide reference data, but the
/// weighting is entirely the caller's own listening, so this is scoped by
/// `user_id` like every other read.
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

    let counts = state.db.genre_play_counts(user.user_id).await.unwrap_or_default();
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
            Some((x, y, color)) => mine.push(serde_json::json!({
                "name": name, "plays": plays, "x": x, "y": y, "color": color,
            })),
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

    let (w, h) = state.everynoise.extent().await.unwrap_or((1500.0, 22648.0));
    let (with_genres, total) =
        state.db.genre_backfill_progress(user.user_id).await.unwrap_or((0, 0));

    (
        StatusCode::OK,
        Json(serde_json::json!({
            // Compact tuples rather than objects: 6,300 of these, and the
            // difference between [n,x,y,c] and four keys apiece is most of the
            // payload.
            "map": {
                "width": w, "height": h,
                "points": backdrop.iter()
                    .map(|(n, x, y, c)| serde_json::json!([n, x, y, c]))
                    .collect::<Vec<_>>(),
            },
            "mine": mine,
            "colour": colour,
            "placed_tags": weighted.len(),
            "unplaced_plays": unplaced,
            // So a half-backfilled history says so rather than looking like a
            // small life.
            "listens_with_genres": with_genres,
            "listens_total": total,
        })),
    )
        .into_response()
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
#[derive(Deserialize)]
pub struct TrajectoryQuery {
    /// `year` or `month`. Omitted lets the server pick from the span.
    pub by: Option<String>,
}

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
            Json(
                serde_json::json!({ "years": [], "unit": unit, "note": "no genres resolved yet" }),
            ),
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
    let mut per_year: std::collections::BTreeMap<String, (f64, f64, i64, Vec<(String, i64)>)> =
        Default::default();
    for (year, genre, plays) in &rows {
        let Some((x, y, colour)) = coords.get(&crate::everynoise::store::genre_key(genre)) else {
            continue;
        };
        let e = per_year.entry(year.clone()).or_insert((0.0, 0.0, 0, Vec::new()));
        e.0 += x * *plays as f64;
        e.1 += y * *plays as f64;
        e.2 += plays;
        e.3.push((colour.clone(), *plays));
    }

    let years: Vec<serde_json::Value> = per_year
        .into_iter()
        .filter(|(_, (_, _, n, _))| *n >= MIN_PLAYS_PER_PERIOD)
        .map(|(year, (sx, sy, n, colours))| {
            let (listens, enriched) = coverage.get(&year).copied().unwrap_or((0, 0));
            let weighted: Vec<(&str, i64)> =
                colours.iter().map(|(c, p)| (c.as_str(), *p)).collect();
            serde_json::json!({
                "year": year,
                "x": sx / n as f64,
                "y": sy / n as f64,
                "plays": n,
                "colour": blend(&weighted),
                "listens": listens,
                "enriched": enriched,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "years": years,
            "unit": unit,
            "min_plays": MIN_PLAYS_PER_PERIOD,
        })),
    )
        .into_response()
}

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
