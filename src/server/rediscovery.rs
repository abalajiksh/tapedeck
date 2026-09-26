//! Rediscovery: what you already chose and are not choosing now.
//!
//! Every list here is built from the user's own history. Nothing recommends
//! music they don't own — that needs a corpus this app deliberately doesn't
//! have, and a self-hosted journal suggesting things to go and buy is the wrong
//! shape. What it can do is notice what has slipped.
//!
//! The lists are served together because they are cheap and the page shows all
//! of them at once; the one exception is the per-gear list, which needs to be
//! told which piece of gear.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use super::auth::AuthUser;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::RediscoveryRow;
use crate::openapi::{NotFound, Unauthorized};

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RediscoverQuery {
    /// How long is "not lately", in days. Defaults to six months — long enough
    /// that a gap is meaningful rather than just a busy fortnight. Clamped to
    /// between a week and ten years.
    #[param(minimum = 7, maximum = 3650, default = 180)]
    pub quiet_days: Option<i64>,
    /// Rows per list. 12 by default, at most 50.
    #[param(minimum = 1, maximum = 50, default = 12)]
    pub limit: Option<i64>,
}

/// Five lists — seven, now — all built from your own history.
#[derive(Serialize, ToSchema)]
pub struct Rediscovery {
    pub quiet_days: i64,
    /// `quiet_days` in words — "six months".
    pub window: String,
    /// The collection with the most rows, for the hero card. Null when every
    /// list is empty.
    pub featured: Option<Collection>,
    /// Every collection, in the page's order, empty ones included.
    pub collections: Vec<Collection>,
    /// Your gear, for the per-gear list at `/api/v1/rediscovery/gear/{id}`.
    pub gear: Vec<GearChoice>,
}

/// One collection: rows, and the reason they are together.
#[derive(Clone, Serialize, ToSchema)]
pub struct Collection {
    /// `forgotten`, `highrated`, `onelisten`, `seasonal`, `latenight`,
    /// `deepcuts` or `dusty`.
    pub key: &'static str,
    pub title: &'static str,
    /// Why these rows are together.
    pub blurb: &'static str,
    /// A hex colour.
    pub accent: &'static str,
    /// The evidence, in words — "Last played 14 months ago".
    pub stat: String,
    pub count: usize,
    pub rows: Vec<RediscoveryRow>,
}

#[derive(Serialize, ToSchema)]
pub struct GearChoice {
    pub id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// Favourites a piece of gear has never played.
#[derive(Serialize, ToSchema)]
pub struct GearRows {
    pub rows: Vec<RediscoveryRow>,
}

/// One collection on the Rediscovery screen.
///
/// A card, not a bare list: the design gives each an accent colour, a one-line
/// blurb explaining *why* these rows are together, and a stat that names the
/// evidence. The rows travel with it so the page renders in one request — none
/// of these queries is expensive and the screen shows them all at once.
///
/// Deliberately **no play or queue action**, though the design has both.
/// Tapedeck has no playback, and a button that does nothing is worse than an
/// absent one — the same call the album/artist pages already made.
fn collection(
    key: &'static str,
    title: &'static str,
    blurb: &'static str,
    accent: &'static str,
    stat: String,
    rows: Vec<crate::db::RediscoveryRow>,
) -> Collection {
    Collection { key, title, blurb, accent, stat, count: rows.len(), rows }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The Rediscovery lists, all built from your own history.
///
/// Session-only. Nothing here points at music you do not own — that is the
/// design, not an implementation detail.
///
/// "Not lately" is a **parameter, not a constant**: six months is a long
/// gap for a daily listener and no gap at all otherwise. The seasonal list
/// excludes the current year, or it would just be "recently played".
#[utoipa::path(
    get,
    path = "/api/v1/rediscovery",
    tag = "Rediscovery",
    params(RediscoverQuery),
    responses(
        (status = 200, description = "OK.", body = Rediscovery),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn rediscover(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Query(q): Query<RediscoverQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(12).clamp(1, 50);
    let quiet_days = q.quiet_days.unwrap_or(180).clamp(7, 3650);
    let quiet_since = now() - quiet_days * 86_400;
    // A one-listen record needs longer to count as forgotten than a favourite
    // does — you haven't had the chance to come back to it yet.
    let heard_once_before = now() - (quiet_days.max(90)) * 86_400;
    // A record is dusty on a much longer clock than a track: putting a side on
    // is an occasion, and 18 months is the design's own figure.
    let dusty_since = now() - 548 * 86_400;

    // "After midnight" has to be local or it means nothing — the same mistake
    // that had the dashboard's day starting at 01:00.
    let tz = crate::server::scrobbles::user_tz_for(&state, user.user_id).await;
    let offset = {
        use chrono::{Offset, TimeZone};
        // Taken for *now* rather than as a fixed number, so a zone on summer
        // time gets its summer offset — the same reason the day boundary goes
        // through chrono-tz instead of arithmetic.
        tz.from_utc_datetime(&chrono::Utc::now().naive_utc()).offset().fix().local_minus_utc()
            as i64
    };

    // Sequential, deliberately — this used to be a `tokio::join!` of all seven.
    //
    // The join looked like free parallelism and was the opposite. The pool
    // holds five connections, so seven concurrent queries immediately queue two
    // of them, and every one of these is an unindexed `GROUP BY` over the
    // user's whole history. Worse, they compete with the poll engine and the
    // MusicBrainz backfills for the same five connections, and sqlx will wait
    // 30s to acquire one on top of the 10s `busy_timeout` — so on a small host
    // behind a proxy this screen, and only this screen, could exceed the
    // gateway timeout and come back as a 502 with nothing in the log.
    //
    // SQLite serialises these anyway. Measured on a 21,390-listen history the
    // whole set is ~140ms either way, so the join bought nothing and cost the
    // one endpoint that fires seven queries its reliability.
    let forgotten = state.db.forgotten_favourites(user.user_id, quiet_since, limit).await;
    let loved = state.db.loved_but_unplayed(user.user_id, limit).await;
    let once = state.db.heard_once(user.user_id, heard_once_before, limit).await;
    let seasonal = state.db.seasonal(user.user_id, limit).await;
    let dusty = state.db.dusty_shelf(user.user_id, dusty_since, limit).await;
    let late = state.db.late_night(user.user_id, offset, limit).await;
    let deep = state.db.deep_cuts(user.user_id, limit).await;

    let window = match quiet_days {
        d if d <= 90 => "three months".to_string(),
        d if d <= 180 => "six months".to_string(),
        d if d <= 365 => "a year".to_string(),
        d => format!("{} years", d / 365),
    };

    let forgotten = forgotten.unwrap_or_default();
    let loved = loved.unwrap_or_default();
    let once = once.unwrap_or_default();
    let seasonal = seasonal.unwrap_or_default();
    let dusty = dusty.unwrap_or_default();
    let late = late.unwrap_or_default();
    let deep = deep.unwrap_or_default();

    // Stats name the evidence rather than repeating the blurb — the design's
    // "Last played 14 months ago" rather than "tracks you used to like".
    let oldest = |rows: &[crate::db::RediscoveryRow]| -> String {
        match rows.iter().filter_map(|r| r.last_played).min() {
            Some(ts) => {
                let months = ((now() - ts) / 2_592_000).max(0);
                if months >= 12 {
                    format!(
                        "Last played {} year{} ago",
                        months / 12,
                        if months >= 24 { "s" } else { "" }
                    )
                } else {
                    format!("Last played {months} months ago")
                }
            }
            None => "Never played".into(),
        }
    };

    let collections = vec![
        collection(
            "forgotten",
            "Forgotten Favourites",
            "Tracks you once wore out on repeat, untouched since.",
            "#c67139",
            oldest(&forgotten),
            forgotten,
        ),
        collection(
            "highrated",
            "Loved, Rarely Played",
            "You went out of your way to love these, then didn't come back.",
            "#7a8a5e",
            loved
                .first()
                .map(|r| {
                    format!(
                        "{} play{} each on average",
                        loved.iter().map(|r| r.plays).sum::<i64>() / loved.len().max(1) as i64,
                        if r.plays == 1 { "" } else { "s" }
                    )
                })
                .unwrap_or_else(|| "No loves yet".into()),
            loved,
        ),
        collection(
            "onelisten",
            "One-Listen Masterpieces",
            "Played once, and never returned to.",
            "#8c491a",
            oldest(&once),
            once,
        ),
        collection(
            "seasonal",
            "Seasonal Favourites",
            "What you had on heavy rotation this time in years gone by.",
            "#aebf92",
            plural(seasonal.len(), "track", "tracks") + " around this date",
            seasonal,
        ),
        collection(
            "latenight",
            "The 3 AM Club",
            "Tracks you only ever reach for well after midnight.",
            "#56633f",
            "Mostly played between midnight and 5am".into(),
            late,
        ),
        collection(
            "deepcuts",
            "Deep Cuts",
            "Album tracks you keep returning to that were never the single.",
            "#b2622d",
            "Track six or later, played more than twice".into(),
            deep,
        ),
        collection(
            "dusty",
            "Dusty Vinyl",
            "Records and tapes that haven't left the shelf.",
            "#645c50",
            if dusty.is_empty() {
                "Nothing gathering dust".into()
            } else {
                plural(dusty.len(), "record", "records") + " unplayed for 18 months"
            },
            dusty,
        ),
    ];

    // The featured card is whichever collection has the most to say, rather
    // than a fixed one — an empty hero on a new instance would be the worst
    // thing on the screen.
    let featured = collections.iter().filter(|c| c.count > 0).max_by_key(|c| c.count).cloned();

    // Gear the page offers for the per-gear list, so it needs no second trip.
    let gear = state.db.get_equipment(user.user_id).await.unwrap_or_default();
    let gear_json: Vec<GearChoice> = gear
        .iter()
        .map(|g| GearChoice { id: g.id, name: g.name.clone(), kind: g.equipment_type.clone() })
        .collect();

    (
        StatusCode::OK,
        Json(Rediscovery { quiet_days, window, featured, collections, gear: gear_json }),
    )
        .into_response()
}

// GET /api/v1/rediscovery/gear/:id — favourites this gear has never played.
/// Favourites never heard on a piece of gear.
///
/// Session-only. Answered by joining `chain_id` to the chains whose
/// components reference the gear — no per-listen equipment field needed.
/// Restricted to tracks played more than once: the interesting question is
/// how your *favourites* sound on it.
#[utoipa::path(
    get,
    path = "/api/v1/rediscovery/gear/{id}",
    tag = "Rediscovery",
    params(("id" = i64, Path, description = "One of your pieces of gear."), RediscoverQuery),
    responses(
        (status = 200, description = "OK.", body = GearRows),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
async fn never_on_gear(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Path(id): Path<i64>,
    Query(q): Query<RediscoverQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(12).clamp(1, 50);

    // Scoped before the query rather than trusting the id: equipment is
    // per-user, and the list would otherwise describe someone else's shelf.
    match state.db.get_equipment(user.user_id).await {
        Ok(list) if list.iter().any(|g| g.id == id) => {}
        Ok(_) => return err(404, "No such gear"),
        Err(e) => return err(500, e.to_string()),
    }

    match state.db.never_on_gear(user.user_id, id, limit).await {
        Ok(rows) => (StatusCode::OK, Json(GearRows { rows })).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(rediscover, never_on_gear))]
pub struct Api;

pub fn create_rediscovery_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/rediscovery", get(rediscover))
        .route("/api/v1/rediscovery/gear/:id", get(never_on_gear))
}
