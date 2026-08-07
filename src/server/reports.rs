//! Weekly / monthly / yearly reports — a feed of period cards, newest first.
//!
//! **Its own surface, not a tab under Statistics** (Ashwin's call, 2026-08-06).
//! The design package offered both and this is the Last.fm shape: its own
//! sidebar entry, its own period stepper. The cost is real and worth naming —
//! it is a second analytics surface whose range can drift out of step with
//! Statistics and Top Charts — and organising all four is a separate piece of
//! work that has been explicitly deferred.
//!
//! ## One pass, not a query per number
//!
//! A card carries a dozen aggregates and every one of them is bucketed in the
//! **user's** timezone, which the SQL layer here cannot do. So this reads the
//! history once, ascending, and folds it in Rust. A year of monthly cards is
//! one query rather than a few hundred.
//!
//! Walking forwards is what makes discoveries cheap: the first time an artist's
//! name appears *is* the first time they were played, so "new this month" and
//! the artist crush fall straight out of the scan with no extra work.
//!
//! ## What this deliberately does not report
//!
//! The design mocks "You out-listened 88% of Tapedeck listeners". That needs
//! other people's listening, and on a private invite-only instance it would
//! quietly expose how much the rest of the household plays. There is no
//! percentile here and there should not be one.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike};
use chrono_tz::Tz;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::auth::ReadAuth;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::ReportRow;

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(serde_json::json!(ErrorResponse { code, error: msg.into() }))).into_response()
}

#[derive(Debug, Deserialize)]
pub struct ReportQuery {
    /// `week`, `month` (default) or `year`.
    pub unit: Option<String>,
    /// How many periods to return, newest first.
    pub limit: Option<usize>,
    /// How many periods to skip before that, counting back from the newest.
    ///
    /// The stepper on the Reports page asks for one period at a time and walks
    /// this. Paging costs nothing extra: the handler already folds the *whole*
    /// history into buckets on every call, so an offset is a slice of work that
    /// has been done regardless — see `report_rows`.
    pub offset: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum Unit {
    Week,
    Month,
    Year,
}

impl Unit {
    fn parse(s: Option<&str>) -> Self {
        match s.unwrap_or("month") {
            "week" => Unit::Week,
            "year" => Unit::Year,
            _ => Unit::Month,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Unit::Week => "week",
            Unit::Month => "month",
            Unit::Year => "year",
        }
    }
}

/// The period a local datetime falls in, as a sortable key and a label.
///
/// Weeks start on Monday. That is deliberately *not* the user's week-start
/// preference: that setting exists so the dashboard counter and the heatmap
/// rows match how someone thinks about their week, whereas a report is a named
/// object — "24 Jul – 30 Jul" — and its boundary has to be stable, or last
/// month's report changes shape when the preference does.
fn period_of(dt: DateTime<Tz>, unit: Unit) -> (String, String) {
    match unit {
        Unit::Year => (format!("{:04}", dt.year()), format!("{}", dt.year())),
        Unit::Month => (
            format!("{:04}-{:02}", dt.year(), dt.month()),
            format!("{} {}", month_name(dt.month()), dt.year()),
        ),
        Unit::Week => {
            let iso = dt.iso_week();
            let monday = NaiveDate::from_isoywd_opt(iso.year(), iso.week(), chrono::Weekday::Mon)
                .unwrap_or_else(|| dt.date_naive());
            let sunday = monday + Duration::days(6);
            (
                format!("{:04}-W{:02}", iso.year(), iso.week()),
                format!(
                    "{} {} – {} {}",
                    monday.day(),
                    short_month(monday.month()),
                    sunday.day(),
                    short_month(sunday.month())
                ),
            )
        }
    }
}

fn month_name(m: u32) -> &'static str {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ][(m as usize).clamp(1, 12) - 1]
}

fn short_month(m: u32) -> &'static str {
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        [(m as usize).clamp(1, 12) - 1]
}

/// The sub-bucket a period is charted by: days across a week, weeks across a
/// month, months across a year. Always the granularity one level finer than
/// the period itself, so the bar chart says something the headline doesn't.
fn sub_bucket(dt: DateTime<Tz>, unit: Unit) -> (usize, String) {
    match unit {
        Unit::Week => {
            let d = dt.weekday().num_days_from_monday() as usize;
            (d, ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][d].to_string())
        }
        Unit::Month => {
            let w = ((dt.day() - 1) / 7) as usize;
            (w.min(4), format!("W{}", w.min(4) + 1))
        }
        Unit::Year => {
            let m = (dt.month() - 1) as usize;
            (m, short_month(dt.month()).to_string())
        }
    }
}

fn sub_bucket_count(unit: Unit) -> usize {
    match unit {
        Unit::Week => 7,
        Unit::Month => 5,
        Unit::Year => 12,
    }
}

/// Everything accumulated for one period while scanning.
#[derive(Default)]
struct Bucket {
    label: String,
    plays: i64,
    seconds: i64,
    start: i64,
    end: i64,
    artists: HashMap<String, (i64, i64)>, // name -> (plays, seconds)
    albums: HashMap<String, (String, i64)>, // "artist|album" -> (display, plays)
    tracks: HashMap<String, (String, String, i64)>, // key -> (title, artist, plays)
    clock: [i64; 24],
    sub: Vec<i64>,
    genres: HashMap<String, i64>,
    chains: HashMap<i64, i64>,
    /// (track key, local day) -> plays, for "on repeat".
    per_day: HashMap<(String, i32), (String, String, i64)>,
    /// Artists whose very first listen anywhere falls inside this period.
    new_artists: HashMap<String, i64>,
    first_seen_at: HashMap<String, i64>,
    days: HashSet<i32>,
}

/// GET /api/v1/reports
async fn reports(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<ReportQuery>,
) -> impl IntoResponse {
    let unit = Unit::parse(q.unit.as_deref());
    let limit = q.limit.unwrap_or(12).clamp(1, 60);
    let offset = q.offset.unwrap_or(0);

    let rows: Vec<ReportRow> =
        match state.db.report_rows(user.user_id, state.analytics_row_cap).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Report rows failed: {}", e);
                return err(500, "Could not read your listening");
            }
        };
    if rows.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "unit": unit.as_str(), "periods": [], "timezone": "UTC",
                "offset": 0, "total_periods": 0,
            })),
        )
            .into_response();
    }

    let tz = crate::server::scrobbles::user_tz_for(&state, user.user_id).await;

    // ── One pass ──────────────────────────────────────────────────────────
    let mut buckets: HashMap<String, Bucket> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    // First-ever sighting of an artist, across the whole history. Because the
    // scan is ascending, the first time we see a name is the real first time.
    let mut seen_artists: HashSet<String> = HashSet::new();
    let mut running_total: i64 = 0;
    // Round-number listens, attributed to whichever period crossed them.
    let mut milestones: HashMap<String, Vec<String>> = HashMap::new();

    for r in &rows {
        let dt = tz.from_utc_datetime(
            &DateTime::from_timestamp(r.timestamp, 0).unwrap_or_default().naive_utc(),
        );
        let (key, label) = period_of(dt, unit);
        let b = buckets.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Bucket {
                label,
                start: r.timestamp,
                sub: vec![0; sub_bucket_count(unit)],
                ..Default::default()
            }
        });

        b.plays += 1;
        b.end = r.timestamp;
        b.seconds += r.duration.unwrap_or(0).max(0);
        b.clock[dt.hour() as usize] += 1;
        let (si, _) = sub_bucket(dt, unit);
        if si < b.sub.len() {
            b.sub[si] += 1;
        }
        b.days.insert(dt.num_days_from_ce());

        let akey = r.artist.trim().to_lowercase();
        let e = b.artists.entry(akey.clone()).or_insert((0, 0));
        e.0 += 1;
        e.1 += r.duration.unwrap_or(0).max(0);

        if let Some(al) = r.album.as_deref().filter(|a| !a.trim().is_empty()) {
            let k = format!("{}|{}", akey, al.trim().to_lowercase());
            let e = b.albums.entry(k).or_insert((al.trim().to_string(), 0));
            e.1 += 1;
        }

        let tkey = format!("{}|{}", akey, r.title.trim().to_lowercase());
        let e = b.tracks.entry(tkey.clone()).or_insert((r.title.clone(), r.artist.clone(), 0));
        e.2 += 1;

        let day = dt.num_days_from_ce();
        let e = b.per_day.entry((tkey, day)).or_insert((r.title.clone(), r.artist.clone(), 0));
        e.2 += 1;

        if let Some(cid) = r.chain_id {
            *b.chains.entry(cid).or_insert(0) += 1;
        }

        // A track carries several tags; each is counted once. This is a share
        // of *tag mentions*, not of listens, and every consumer must say so.
        if let Some(g) = r.genres.as_deref() {
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(g) {
                for t in tags {
                    let t = t.trim().to_lowercase();
                    if !t.is_empty() {
                        *b.genres.entry(t).or_insert(0) += 1;
                    }
                }
            }
        }

        if seen_artists.insert(akey.clone()) {
            *b.new_artists.entry(akey.clone()).or_insert(0) += 1;
            b.first_seen_at.insert(akey.clone(), r.timestamp);
        } else if b.new_artists.contains_key(&akey) {
            *b.new_artists.get_mut(&akey).unwrap() += 1;
        }

        running_total += 1;
        for step in [1000, 5000, 10_000, 20_000, 25_000, 50_000, 100_000] {
            if running_total == step {
                milestones
                    .entry(key.clone())
                    .or_default()
                    .push(format!("{}th listen", thousands(step)));
            }
        }
    }

    // Newest first, and only as many as were asked for — plus one extra kept
    // aside so the oldest card still has a previous period to compare against.
    order.sort();
    order.reverse();
    let total_periods = order.len();
    let wanted: Vec<String> = order.iter().skip(offset).take(limit).cloned().collect();

    let chains = state.db.get_chains(user.user_id).await.unwrap_or_default();
    let chain_name: HashMap<i64, String> =
        chains.iter().filter_map(|c| c.id.map(|id| (id, c.name.clone()))).collect();

    let mut periods = Vec::new();
    for (i, key) in wanted.iter().enumerate() {
        let b = &buckets[key];
        // The previous period in the *ordered* list, which is the one before it
        // in time — not "the key minus one", which would be wrong across a
        // month with no listening.
        //
        // `i` indexes `wanted`, which starts at `offset` — so the offset has to
        // come back in here. Without it every paged-to period would be compared
        // against whatever sat one past the *window*, and "up 14% on July" would
        // name a real month and the wrong number.
        let prev = order.get(offset + i + 1).and_then(|k| buckets.get(k));

        let top_artist = b.artists.iter().max_by_key(|(_, v)| v.0).map(|(k, v)| {
            // Display the spelling actually used, not the lowercased key.
            let display = rows
                .iter()
                .rev()
                .find(|r| r.artist.trim().to_lowercase() == *k)
                .map(|r| r.artist.clone())
                .unwrap_or_else(|| k.clone());
            serde_json::json!({
                "name": display,
                "plays": v.0,
                "hours": (v.1 as f64 / 3600.0 * 10.0).round() / 10.0,
            })
        });
        let top_album = b.albums.iter().max_by_key(|(_, v)| v.1).map(|(k, v)| {
            // The key is lowercased for grouping; show the spelling that was
            // actually submitted, or the card reads "epitone project".
            let akey = k.split('|').next().unwrap_or("");
            let artist = rows
                .iter()
                .rev()
                .find(|r| r.artist.trim().to_lowercase() == akey)
                .map(|r| r.artist.clone())
                .unwrap_or_else(|| akey.to_string());
            serde_json::json!({ "name": v.0, "artist": artist, "plays": v.1 })
        });
        let top_track = b
            .tracks
            .values()
            .max_by_key(|v| v.2)
            .map(|v| serde_json::json!({ "title": v.0, "artist": v.1, "plays": v.2 }));

        // "On repeat": the heaviest single day of a single track.
        let mut repeat: Vec<_> = b.per_day.values().filter(|v| v.2 >= 3).collect();
        repeat.sort_by(|a, b| b.2.cmp(&a.2));
        let on_repeat: Vec<serde_json::Value> = repeat
            .iter()
            .take(4)
            .map(|v| serde_json::json!({ "title": v.0, "artist": v.1, "plays": v.2 }))
            .collect();

        // The crush: the most-played artist you had never played before.
        // Only where there was a "before". In the oldest period every artist is
        // new by construction, so the crush would just be the top artist wearing
        // a different label.
        let crush = prev
            .and(b.new_artists.iter().max_by_key(|(_, p)| **p))
            .filter(|(_, p)| **p >= 5)
            .map(|(k, p)| {
                let display = rows
                    .iter()
                    .find(|r| r.artist.trim().to_lowercase() == *k)
                    .map(|r| r.artist.clone())
                    .unwrap_or_else(|| k.clone());
                serde_json::json!({
                    "name": display,
                    "plays": p,
                    "since": b.first_seen_at.get(k).copied().unwrap_or(0),
                })
            });

        let tag_total: i64 = b.genres.values().sum();
        let mut gs: Vec<_> = b.genres.iter().collect();
        gs.sort_by(|a, b| b.1.cmp(a.1));
        let genres: Vec<serde_json::Value> = gs
            .iter()
            .take(6)
            .map(|(n, c)| {
                serde_json::json!({
                    "name": n,
                    "pct": if tag_total > 0 { (**c as f64 / tag_total as f64 * 100.0).round() as i64 } else { 0 },
                })
            })
            .collect();

        // The period's colour, if the genre map has been fetched. Same linear
        // blend the map itself uses, so the two agree.
        let colour = {
            let names: Vec<String> = gs.iter().take(60).map(|(n, _)| (*n).clone()).collect();
            match state.everynoise.coordinates_for(&names).await {
                Ok(coords) if !coords.is_empty() => {
                    let weighted: Vec<(&str, i64)> = gs
                        .iter()
                        .filter_map(|(n, c)| {
                            coords
                                .get(&crate::everynoise::store::genre_key(n))
                                .map(|p| (p.2.as_str(), **c))
                        })
                        .collect();
                    crate::server::everynoise::blend(&weighted)
                }
                _ => None,
            }
        };

        let shelf =
            state.db.physical_plays_in(user.user_id, b.start, b.end + 1).await.unwrap_or_default();
        let sides: i64 = shelf.iter().map(|(_, _, _, n)| *n).sum();

        let top_chain = b
            .chains
            .iter()
            .max_by_key(|(_, n)| **n)
            .and_then(|(cid, _)| chain_name.get(cid).cloned());

        // Calendar days, not days you happened to play something. Dividing by
        // listening days answers "how much on the days you listened", which is
        // a different and much larger number — 846 plays over five active days
        // reads as 169/day when the month's real rate is 27.
        //
        // For a period still in progress it counts days *elapsed*, so the
        // current month is not quietly halved by dividing through a full 31.
        let days = elapsed_days(b, unit, tz) as f64;
        let prev_json = prev.map(|p| {
            let pd = elapsed_days(p, unit, tz) as f64;
            serde_json::json!({
                "label": p.label,
                "plays": p.plays,
                "hours": (p.seconds as f64 / 3600.0 * 10.0).round() / 10.0,
                "avg_daily": (p.plays as f64 / pd).round() as i64,
                "artists": p.artists.len(),
                "albums": p.albums.len(),
                "tracks": p.tracks.len(),
                "sub": p.sub,
            })
        });

        periods.push(serde_json::json!({
            "key": key,
            "label": b.label,
            "start": b.start,
            "end": b.end,
            "plays": b.plays,
            "hours": (b.seconds as f64 / 3600.0 * 10.0).round() / 10.0,
            "avg_daily": (b.plays as f64 / days).round() as i64,
            "artists": b.artists.len(),
            "albums": b.albums.len(),
            "tracks": b.tracks.len(),
            "new_artists": b.new_artists.len(),
            "top_artist": top_artist,
            "top_album": top_album,
            "top_track": top_track,
            "clock": b.clock,
            "sub": b.sub,
            "sub_labels": sub_labels(unit),
            "genres": genres,
            "colour": colour,
            "on_repeat": on_repeat,
            "crush": crush,
            "shelf": {
                "sides": sides,
                "rows": shelf.iter().take(5).map(|(title, artist, side, spins)| serde_json::json!({
                    "title": title, "artist": artist, "side": side, "spins": spins
                })).collect::<Vec<_>>(),
            },
            "top_chain": top_chain,
            "milestones": milestones.get(key).cloned().unwrap_or_default(),
            "prev": prev_json,
        }));
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "unit": unit.as_str(),
            "timezone": tz.name(),
            "periods": periods,
            // What the stepper needs to know where the ends are: how many
            // periods exist at this bucket size, and where this page sits.
            "offset": offset,
            "total_periods": total_periods,
            // The scan is capped like every other analytics read; say so rather
            // than silently reporting on a truncated history.
            "capped": rows.len() as i64 >= state.analytics_row_cap,
        })),
    )
        .into_response()
}

/// Calendar days the period covers — the denominator for "average daily".
///
/// Two cases, and both were wrong in the obvious version. A **finished** period
/// divides by its full length: someone who played nothing between the 5th and
/// the 20th still had a 31-day month, and dividing by the span between their
/// first and last listen would report the rate of their busiest fortnight as
/// the month's average.
///
/// A period **still running** divides by the days it has actually had, or the
/// current month reads as a slump for its first three weeks. It counts to
/// *today*, not to the last listen — stopping two days ago is part of the
/// average, not an exclusion from it.
fn elapsed_days(b: &Bucket, unit: Unit, tz: Tz) -> i64 {
    let local = |ts: i64| {
        tz.from_utc_datetime(&DateTime::from_timestamp(ts, 0).unwrap_or_default().naive_utc())
    };
    let start = local(b.start);
    let full = match unit {
        Unit::Week => 7,
        Unit::Month => days_in_month(start.year(), start.month()),
        Unit::Year => {
            if is_leap(start.year()) {
                366
            } else {
                365
            }
        }
    };

    // The first day of the period this listen sits in, not the day of the
    // listen itself — otherwise a month whose first play was on the 9th would
    // only ever count 23 days.
    let period_start_day = match unit {
        Unit::Week => start.num_days_from_ce() - start.weekday().num_days_from_monday() as i32,
        Unit::Month => start.num_days_from_ce() - (start.day() as i32 - 1),
        Unit::Year => start.num_days_from_ce() - (start.ordinal() as i32 - 1),
    };

    let today = local(chrono::Utc::now().timestamp());
    let (today_key, _) = period_of(today, unit);
    let (this_key, _) = period_of(start, unit);
    if today_key != this_key {
        return full;
    }
    ((today.num_days_from_ce() - period_start_day + 1) as i64).clamp(1, full)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i32, m: u32) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 30,
    }
}

fn sub_labels(unit: Unit) -> Vec<String> {
    match unit {
        Unit::Week => ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Unit::Month => (1..=5).map(|w| format!("W{w}")).collect(),
        Unit::Year => (1..=12).map(short_month).map(|s| s.to_string()).collect(),
    }
}

fn thousands(n: i64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn create_reports_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/reports", get(reports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(tz: Tz, y: i32, m: u32, d: u32, h: u32) -> DateTime<Tz> {
        tz.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn periods_are_named_the_way_someone_would_say_them() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        assert_eq!(period_of(at(tz, 2026, 8, 15, 12), Unit::Month).1, "August 2026");
        assert_eq!(period_of(at(tz, 2026, 8, 15, 12), Unit::Year).1, "2026");
        // 24–30 July 2026 is a Friday-to-Thursday span; the ISO week it sits in
        // runs Monday 20 to Sunday 26.
        let (key, label) = period_of(at(tz, 2026, 7, 24, 12), Unit::Week);
        assert!(key.contains("-W"), "week key should carry the ISO week: {key}");
        assert!(label.contains('–'), "week label should be a range: {label}");
    }

    /// The bucket a listen lands in is decided in the user's zone, not UTC.
    /// A listen at 00:30 on 1 August in Berlin belongs to August, and reporting
    /// it under July would be the same bug the dashboard's "today" had.
    #[test]
    fn a_listen_just_after_midnight_belongs_to_the_local_month() {
        let berlin: Tz = "Europe/Berlin".parse().unwrap();
        let local = at(berlin, 2026, 8, 1, 0); // 00:00 local == 22:00 UTC on 31 Jul
        assert_eq!(period_of(local, Unit::Month).0, "2026-08");
        let utc_view = chrono_tz::UTC.from_utc_datetime(&local.naive_utc());
        assert_eq!(period_of(utc_view, Unit::Month).0, "2026-07", "UTC would have said July");
    }

    #[test]
    fn sub_buckets_are_one_level_finer_than_the_period() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        assert_eq!(sub_bucket(at(tz, 2026, 8, 3, 9), Unit::Week).1, "Mon");
        assert_eq!(sub_bucket(at(tz, 2026, 8, 3, 9), Unit::Month).1, "W1");
        assert_eq!(sub_bucket(at(tz, 2026, 8, 3, 9), Unit::Year).1, "Aug");
        assert_eq!(sub_bucket(at(tz, 2026, 8, 29, 9), Unit::Month).1, "W5");
    }

    #[test]
    fn milestones_are_grouped_thousands() {
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(20_000), "20,000");
        assert_eq!(thousands(100_000), "100,000");
    }
}
