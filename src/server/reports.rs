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
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use utoipa::ToSchema;

use super::auth::ReadAuth;
use super::models::ErrorResponse;
use super::AppState;
use crate::db::ReportRow;
use crate::openapi::{Forbidden, Unauthorized};

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReportQuery {
    /// `week`, `month` (default) or `year`.
    #[param(pattern = "^(week|month|year)$", default = "month")]
    pub unit: Option<String>,
    /// How many periods to return, newest first. 12 by default, at most 60.
    #[param(minimum = 1, maximum = 60, default = 12)]
    pub limit: Option<usize>,
    /// Periods to skip, counting back from the newest. The stepper on the
    /// Reports page walks this one period at a time. Costs nothing extra — the
    /// whole history is folded on every call regardless.
    // Paging costs nothing extra: the handler already folds the *whole*
    // history into buckets on every call, so an offset is a slice of work that
    // has been done regardless — see `report_rows`.
    #[param(minimum = 0, default = 0)]
    pub offset: Option<usize>,
}

/// A feed of period cards, newest first.
#[derive(Serialize, ToSchema)]
pub struct Reports {
    /// `week`, `month` or `year`.
    pub unit: &'static str,
    /// The IANA zone the buckets were cut in.
    pub timezone: String,
    pub periods: Vec<Period>,
    /// Where this page sits, counting back from the newest.
    pub offset: usize,
    /// How many periods exist at this bucket size — what tells a stepper where
    /// the ends are.
    pub total_periods: usize,
    /// The scan hit the analytics row cap, so the oldest periods are partial
    /// and must not be presented as final. Absent with no listening at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capped: Option<bool>,
}

/// One week, month or year.
#[derive(Serialize, ToSchema)]
pub struct Period {
    /// `2026-W31`, `2026-08`, `2026`.
    #[schema(example = "2026-08")]
    pub key: String,
    #[schema(example = "August 2026")]
    pub label: String,
    /// Unix seconds — the first listen in the period.
    pub start: i64,
    /// Unix seconds — the last listen in the period.
    pub end: i64,
    pub plays: i64,
    pub hours: f64,
    /// Plays per **calendar** day — a finished period by its full length, one
    /// still running by the days it has actually had.
    pub avg_daily: i64,
    pub artists: usize,
    pub albums: usize,
    pub tracks: usize,
    /// Artists with no earlier listen anywhere in the history.
    pub new_artists: usize,
    /// Never "Various Artists", which is a filing convention, not an artist.
    pub top_artist: Option<PeriodArtist>,
    pub top_album: Option<PeriodAlbum>,
    pub top_track: Option<PeriodTrack>,
    /// 24 counts, local hours.
    pub clock: [i64; 24],
    /// One level finer than the period — days across a week, weeks across a
    /// month, months across a year.
    pub sub: Vec<i64>,
    pub sub_labels: Vec<String>,
    /// The six biggest genres. Shares of **tag mentions**, not of listens.
    pub genres: Vec<GenreShare>,
    pub genre_bands: GenreBands,
    /// Where this period sits among *your own* periods by plays, 1 being the
    /// busiest. This is what fills the slot the design gave to a cross-user
    /// percentile — there is no such thing here: it needs other people's
    /// listening, and on a private instance it would leak how much the rest of
    /// the household plays.
    pub rank: usize,
    /// How many periods `rank` is out of.
    pub rank_of: usize,
    /// Play-weighted blend of the period's genre colours, in linear light.
    /// Null until the genre map is fetched.
    pub colour: Option<String>,
    /// The heaviest single days of single tracks — three plays or more.
    pub on_repeat: Vec<PeriodTrack>,
    /// Most-played artist never played before, with at least five plays.
    /// Null for the oldest period, where every artist is new by construction
    /// and it would just be the top artist relabelled.
    pub crush: Option<Crush>,
    /// Sides played off the shelf.
    pub shelf: ShelfSummary,
    /// The chain most listens went through.
    pub top_chain: Option<String>,
    pub gear: PeriodGear,
    pub company: PeriodCompany,
    pub fingerprint: Fingerprint,
    /// Sentences worth a line of their own — "your 10,000th listen".
    pub milestones: Vec<String>,
    /// The period before, for deltas. Null for the oldest.
    pub prev: Option<PrevPeriod>,
}

#[derive(Serialize, ToSchema)]
pub struct PeriodArtist {
    /// The latest spelling actually submitted.
    pub name: String,
    pub plays: i64,
    pub hours: f64,
    /// A wide image, when you chose one for this artist. Absent otherwise —
    /// nothing Tapedeck can reach supplies one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landscape_url: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PeriodAlbum {
    pub name: String,
    pub artist: String,
    pub plays: i64,
}

#[derive(Serialize, ToSchema)]
pub struct PeriodTrack {
    pub title: String,
    pub artist: String,
    pub plays: i64,
}

#[derive(Serialize, ToSchema)]
pub struct Crush {
    /// The first spelling it arrived under.
    pub name: String,
    pub plays: i64,
    /// Unix seconds of the first listen.
    pub since: i64,
}

#[derive(Serialize, ToSchema)]
pub struct GenreShare {
    pub name: String,
    /// A whole percentage of tag mentions.
    pub pct: i64,
}

/// The period's three biggest genres **by name**, fixed across every
/// sub-bucket, plus `other` as the remainder. Drawn as stacked bands by the
/// year chapter. A sub-bucket with no tags at all is all zeroes rather than
/// 100% `other` — an empty month is an absence, not a genre.
///
/// Shares of tag *mentions*, like `genres`.
#[derive(Serialize, ToSchema)]
pub struct GenreBands {
    /// At most three.
    pub names: Vec<String>,
    /// One row per sub-bucket; each row is the named shares then `other`.
    pub shares: Vec<Vec<f64>>,
}

#[derive(Serialize, ToSchema)]
pub struct ShelfSummary {
    pub sides: i64,
    /// The five most-played sides.
    pub rows: Vec<ShelfRow>,
}

#[derive(Serialize, ToSchema)]
pub struct ShelfRow {
    pub title: String,
    pub artist: String,
    pub side: String,
    pub spins: i64,
}

/// What the period was listened *through*. `unattributed` and `no_duration`
/// are stated rather than folded away: on most histories the first is nearly
/// everything, and the hours are a floor without the second.
#[derive(Serialize, ToSchema)]
pub struct PeriodGear {
    pub chains: Vec<GearChain>,
    /// The eight pieces of gear with the most hours, each counted once per
    /// chain even when a chain names it twice.
    pub items: Vec<GearItem>,
    /// Listens with no chain at all.
    pub unattributed: i64,
    /// Chained listens whose source reported no length.
    pub no_duration: i64,
}

#[derive(Serialize, ToSchema)]
pub struct GearChain {
    pub name: String,
    pub icon: Option<String>,
    pub plays: i64,
    pub hours: f64,
}

#[derive(Serialize, ToSchema)]
pub struct GearItem {
    pub name: String,
    /// The component type, lowercased — `dac`, `amp`, `transducer`…
    #[serde(rename = "type")]
    pub kind: String,
    pub plays: i64,
    pub hours: f64,
}

/// Listens **marked** as heard with somebody. `company` is null on
/// everything nobody has tagged, so `of_plays` travels as the denominator —
/// never present a bare share, and never read the complement as "alone".
#[derive(Serialize, ToSchema)]
pub struct PeriodCompany {
    pub marked_plays: i64,
    pub marked_hours: f64,
    pub of_plays: i64,
    /// The eight people heard with most.
    pub people: Vec<Person>,
}

#[derive(Serialize, ToSchema)]
pub struct Person {
    pub name: String,
    pub plays: i64,
}

/// Five ratios between 0 and 1, and the same five for *you, usually* — every
/// one answerable from any history, and **none of them a comparison against
/// another person**. The baseline is the mean of your own periods, not one
/// shape over the whole history: `variety` shrinks as plays grow and
/// `discovery` is pinned at 1.0 over a whole history, so a whole-history
/// baseline would report every period as more varied and less exploratory
/// than usual.
#[derive(Serialize, ToSchema)]
pub struct Fingerprint {
    /// `Variety`, `Repeat`, `Discovery`, `Spread`, `Runs`.
    pub axes: [&'static str; 5],
    pub you: [f64; 5],
    pub usual: [f64; 5],
    /// How many periods the baseline rests on. One is not a habit — a radar
    /// comparing a month against itself draws two identical shapes and means
    /// nothing by it, so decline to draw the baseline below two.
    pub usual_periods: usize,
}

/// The period before, for deltas.
#[derive(Serialize, ToSchema)]
pub struct PrevPeriod {
    pub label: String,
    pub plays: i64,
    pub hours: f64,
    pub avg_daily: i64,
    pub artists: usize,
    pub albums: usize,
    pub tracks: usize,
    pub sub: Vec<i64>,
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

/// Every artist a row credits, folded, primary first.
///
/// `artist_credit` is everyone credited joined with `"; "` — the shape
/// `save_scrobble` and `fold_artist_credit` both write — so splitting it is
/// reading our own format rather than guessing at a tag.
///
/// The primary comes in already decided by the caller, because on a compilation
/// it is *not* `scrobbles.artist`: that column holds the "Various Artists"
/// placeholder and the real performer is the first of the credit. The
/// placeholder is dropped wherever it appears here for the same reason — it is
/// a filing convention, and letting it into the tally puts it back on top of
/// every superlative it was removed from in v0.57.0.
fn credited_names(primary: &str, credit: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |n: &str| {
        let n = n.trim();
        if n.is_empty() || crate::db::is_various_artists(n) {
            return;
        }
        let key = n.to_lowercase();
        if !out.contains(&key) {
            out.push(key);
        }
    };
    push(primary);
    for name in credit.into_iter().flat_map(|c| c.split(';')) {
        push(name);
    }
    out
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

/// How many sub-buckets a period is drawn in.
///
/// Worth knowing before reaching for another distribution chart: a **month is
/// five**, not thirty. `genre_bands` is emitted for every unit and is drawn
/// only on the year, and that is deliberate — twelve points show a mix moving,
/// five sit under a ranked list that already says the same thing better. The
/// design's own argument against giving the week a genre split ("seven days
/// make a thin genre split") applies to a month at five just as well.
fn sub_bucket_count(unit: Unit) -> usize {
    match unit {
        Unit::Week => 7,
        Unit::Month => 5,
        Unit::Year => 12,
    }
}

/// Below this a period is left out of the "usually" average.
///
/// The same reasoning the genre trajectory's own floor carries: a shape from a
/// handful of listens is a fact about a handful of listens, and inside an
/// average it reads as a habit. It does not stop such a period being *drawn* —
/// only from distorting what it is drawn against.
const MIN_PLAYS_FOR_SHAPE: i64 = 20;

/// The five axes, in the order the radar draws them.
pub const FINGERPRINT_AXES: [&str; 5] = ["Variety", "Repeat", "Discovery", "Spread", "Runs"];

/// A period's shape, as five numbers between 0 and 1.
///
/// **Every axis is answerable from `timestamp`, `artist`, `title` and `album`
/// alone**, and that is the whole of why these five and not others. The
/// tempting axes are the Tapedeck-only ones — signal chain, format, attention —
/// and each of those is *unknown* for most of a real history: a shape drawn
/// from them would be a picture of how the listening was **recorded** wearing
/// the clothes of a picture of the listening, which is the fidelity-card
/// failure exactly. Those live in "Through what" instead, where a panel can
/// state its own coverage; a radar cannot legibly carry five different
/// denominators.
///
/// **Compared against the reader's own history, never against other people.**
/// The Last.fm chart this borrows its form from plots you against a "global
/// average"; the equivalent here needs other members' listening and quietly
/// exposes how much they play — the rule Reports settled in v0.41.0 when it
/// refused the cross-user percentile and used a rank against your own periods
/// instead. So the second shape is *you, usually*.
///
/// Each axis is a ratio with no dial on it except [`RUN_LEN`], for the reason
/// the breadth metric records: a number dominated by a parameter somebody
/// invented is a confident-looking way of saying nothing.
fn fingerprint(b: &Bucket) -> [f64; 5] {
    let plays = b.plays.max(0) as f64;
    if plays <= 0.0 {
        return [0.0; 5];
    }

    // Variety — effective artists as a share of plays, via `exp(H)`. 1.0 is a
    // different artist every time; low is a few on heavy rotation. The same
    // measure the genre breadth uses, and parameter-free for the same reason.
    let h: f64 = b
        .artists
        .values()
        .map(|(n, _)| *n as f64 / plays)
        .filter(|p| *p > 0.0)
        .map(|p| -p * p.ln())
        .sum();
    let variety = (h.exp() / plays).clamp(0.0, 1.0);

    // Repeat — how much of the period was hearing something again.
    let repeat = (1.0 - (b.tracks.len() as f64 / plays)).clamp(0.0, 1.0);

    // Discovery — of the artists played, how many were new *anywhere*, not
    // merely new to this period. `new_artists` is keyed on the first sighting
    // across the whole history, which is what makes it mean anything.
    let discovery = if b.artists.is_empty() {
        0.0
    } else {
        (b.new_artists.len() as f64 / b.artists.len() as f64).clamp(0.0, 1.0)
    };

    // Spread — how evenly the listening falls across the hours of the day,
    // as a share of the most even it could be. Bucketed in the reader's own
    // timezone upstream, so this is not quietly UTC.
    let ch: f64 =
        b.clock.iter().map(|n| *n as f64 / plays).filter(|p| *p > 0.0).map(|p| -p * p.ln()).sum();
    // `+ 0.0` after the clamp: a period entirely inside one hour has an
    // entropy of `-1 * ln(1)`, which is **negative zero** — `clamp` keeps it
    // and the axis then renders as "-0". Harmless arithmetically, and it reads
    // as a bug, which for a number nobody can check by eye is the same thing as
    // being one.
    //
    // Not `.max(0.0)`, which is what this was: IEEE 754 leaves the sign of
    // `max(-0.0, 0.0)` to the implementation, and current Rust hands back the
    // negative zero — the guard compiled, did nothing, and its own test failed.
    // Adding positive zero is defined to give positive zero in round-to-nearest.
    let spread = (ch / 24f64.ln()).clamp(0.0, 1.0) + 0.0;

    // Runs — the share of plays inside a run of one album. The "did you put a
    // record on, or did you shuffle" axis, and the one nothing else here says.
    let runs = (b.run_plays as f64 / plays).clamp(0.0, 1.0);

    [variety, repeat, discovery, spread, runs]
}

/// How many consecutive listens from one album make a *run*.
///
/// A stated parameter, and the only one in the fingerprint — the rest are
/// ratios with no dial on them, for the reason the breadth metric records: a
/// number dominated by a value somebody invented is a confident-looking way of
/// saying nothing. Three is the smallest count that is not an accident: two in
/// a row happens on shuffle often enough to mean little, and a side of vinyl or
/// an album played through is many more than three.
const RUN_LEN: i64 = 3;

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
    /// Genre tag counts per sub-bucket, for the year chapter's band chart.
    /// Same fan-out as `genres` and therefore the same caveat: a share of tag
    /// *mentions*, not of listens.
    sub_genres: Vec<HashMap<String, i64>>,
    /// chain id -> (plays, seconds). Seconds as well as plays because "what
    /// did I listen *through*" is a question about time, and a chain used for
    /// three long records is not the one used for thirty short tracks.
    chains: HashMap<i64, (i64, i64)>,
    /// Listens whose duration nobody reported. The gear hours are a **floor**
    /// without them, and the card says so rather than implying a total.
    chain_seconds_unknown: i64,
    /// Listens with no chain at all. Named rather than folded away: on most
    /// histories this is nearly everything, and a gear panel that quietly
    /// ignored it would look like a complete account of the period.
    chainless: i64,
    /// Listens **marked** as heard with somebody, and who.
    ///
    /// Marked, not "social" — `company` is null on everything nobody has
    /// tagged, which is almost all listening, and reading null as "alone"
    /// is the fidelity-card mistake exactly. `plays` above is the denominator
    /// the payload carries so nothing can present a share without it.
    company_plays: i64,
    company_seconds: i64,
    company_people: HashMap<String, i64>,
    /// Plays that fall inside a run of [`RUN_LEN`] or more consecutive listens
    /// from one album — the "did you put a record on, or shuffle" axis.
    run_plays: i64,
    /// Transient, for the ascending scan that computes the above.
    run_album: Option<String>,
    run_len: i64,
    /// (track key, local day) -> plays, for "on repeat".
    per_day: HashMap<(String, i32), (String, String, i64)>,
    /// Artists whose very first listen anywhere falls inside this period.
    new_artists: HashMap<String, i64>,
    first_seen_at: HashMap<String, i64>,
    days: HashSet<i32>,
}

/// Period reports — a feed of week / month / year cards, newest first.
///
/// Scope: `read` *(since v0.41)*.
///
/// Reads the history **once** and folds it in Rust. A card carries a dozen
/// aggregates and every one is bucketed in the caller's own timezone, which
/// SQL here cannot do — a year of monthly cards would otherwise be hundreds
/// of round trips.
///
/// Weeks are ISO (Monday-start), deliberately **not** the caller's
/// week-start preference: that setting shapes the dashboard counter and the
/// heatmap rows, whereas a report is a named object and its boundary has to
/// stay put, or last month's report changes shape when the preference does.
///
/// `avg_daily` divides by **calendar** days — a finished period by its full
/// length, one still running by the days it has actually had. Dividing by
/// days-with-listens would report the rate of your busiest fortnight as the
/// month's average.
///
/// Two things deliberately absent. There is **no percentile against other
/// users** ("you out-listened 88% of listeners"): that needs other people's
/// listening, and on a private instance it would leak how much the rest of
/// the household plays. And `crush` is omitted for the oldest period, where
/// every artist is new by construction and it would just be the top artist
/// relabelled.
#[utoipa::path(
    get,
    path = "/api/v1/reports",
    tag = "Statistics",
    security(("sessionCookie" = []), ("apiToken" = ["read"])),
    params(ReportQuery),
    responses(
        (status = 200, description = "OK.", body = Reports),
        (status = 401, response = Unauthorized),
        (status = 403, response = Forbidden),
    )
)]
pub async fn reports(
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
            Json(Reports {
                unit: unit.as_str(),
                periods: Vec::new(),
                timezone: "UTC".into(),
                offset: 0,
                total_periods: 0,
                capped: None,
            }),
        )
            .into_response();
    }

    let tz = crate::server::scrobbles::user_tz_for(&state, user.user_id).await;

    // Taken before the rows move: the response says whether the scan hit the
    // analytics cap, and that is a fact about what was read rather than about
    // what the fold produced.
    let scanned = rows.len() as i64;

    // ── One pass, off the async worker ─────────────────────────────────────
    //
    // The fold walks the **whole history** and builds a dense map per period.
    // It is ordinary synchronous work, and doing it in an async handler pins a
    // tokio worker for its full duration — one of four on a Pi — so several
    // concurrent report requests can starve everything else the runtime has to
    // do, including the poll tick and health checks.
    //
    // `rows` and `tz` are not read again after this, so the whole thing moves.
    let folded = match tokio::task::spawn_blocking(move || fold_periods(rows, tz, unit)).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Report fold panicked: {}", e);
            return err(500, "Could not build your report");
        }
    };
    let Folded { buckets, mut order, milestones, display } = folded;

    // "You, usually" — the shape every period is held against.
    //
    // **The mean of the period shapes, not one shape over the whole history**,
    // and the difference is not cosmetic. `variety` is `exp(H) / plays`, which
    // shrinks as plays grow: a whole-history figure would sit systematically
    // below any single month's, so *every* card would report itself as more
    // varied than usual. `discovery` is worse — over the whole history every
    // artist is new exactly once, so it would be pinned at 1.0 and every period
    // would look like a drought. Averaging the periods compares like with like.
    //
    // Periods with almost nothing in them are left out for the reason the genre
    // trajectory omits them: a shape from four listens is a fact about four
    // listens, and inside an average it reads as a habit.
    // Two periods are left out of the average, each for its own reason.
    //
    // A **thin** one, because a shape from a handful of listens is a fact about
    // a handful of listens and inside an average it reads as a habit — the
    // floor the genre trajectory already carries.
    //
    // The **oldest** one, for the reason `crush` is omitted there: the history
    // begins inside it, so every artist in it is new by construction and its
    // `discovery` is pinned at 1.0. That is a fact about where the records
    // start, not about the listener, and in the baseline it would make every
    // later period read as a discovery drought — the same bias averaging the
    // periods was chosen to avoid.
    //
    // `order` is still ascending here; it is reversed below.
    let oldest = order.first().cloned();
    let (usual, usual_periods): ([f64; 5], usize) = {
        let shapes: Vec<[f64; 5]> = buckets
            .iter()
            .filter(|(k, b)| b.plays >= MIN_PLAYS_FOR_SHAPE && Some(*k) != oldest.as_ref())
            .map(|(_, b)| fingerprint(b))
            .collect();
        let mut m = [0.0; 5];
        for sh in &shapes {
            for (i, v) in sh.iter().enumerate() {
                m[i] += v;
            }
        }
        if !shapes.is_empty() {
            for v in m.iter_mut() {
                *v /= shapes.len() as f64;
            }
        }
        (m, shapes.len())
    };

    // Newest first, and only as many as were asked for — plus one extra kept
    // aside so the oldest card still has a previous period to compare against.
    order.sort();
    order.reverse();
    let total_periods = order.len();

    // Busiest period first, so a card can say "your 3rd busiest month". Ranked
    // over *every* period, not the requested page — the page is a window, and a
    // rank that only counted the window would change as you stepped.
    let rank_of: HashMap<&String, usize> = {
        let mut by_plays: Vec<&String> = order.iter().collect();
        by_plays.sort_by_key(|k| std::cmp::Reverse(buckets[*k].plays));
        by_plays.into_iter().enumerate().map(|(i, k)| (k, i + 1)).collect()
    };
    let wanted: Vec<String> = order.iter().skip(offset).take(limit).cloned().collect();

    let chains = state.db.get_chains(user.user_id).await.unwrap_or_default();
    let chain_name: HashMap<i64, String> =
        chains.iter().filter_map(|c| c.id.map(|id| (id, c.name.clone()))).collect();
    // The chains themselves, for the gear panel — it needs the icon and the
    // components, not only the name.
    let chain_by_id: HashMap<i64, &crate::models::SignalChain> =
        chains.iter().filter_map(|c| c.id.map(|id| (id, c))).collect();

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

        // "Various Artists" is not an artist. It is a filing convention meaning
        // *look at the track*, which is why `is_various_artists` already guards
        // the Plex source and the sanitiser — and why a month headed "your top
        // artist was Various Artists" says nothing about that month. It wins
        // easily on a shelf with compilations on it, because every track of
        // every compilation credits it.
        //
        // Excluded from the *superlative* only. The plays are still real
        // listening and still count towards the totals, the artist tally and
        // everything else; what they cannot do is name the month.
        let top_artist = b
            .artists
            .iter()
            .filter(|(k, _)| !crate::db::is_various_artists(k))
            .max_by_key(|(_, v)| v.0)
            .map(|(k, v)| {
                // Display the spelling actually used, not the lowercased key —
                // the trap `top_album` hit once and shipped as "epitone project".
                //
                // Both columns are checked, because the key may have come from
                // `artist_credit` on a compilation track, in which case no row's
                // `artist` matches it and the fallback would print the folded key.
                // The latest spelling seen, which is what the reverse scan
                // this replaced was picking.
                let display =
                    display.get(k).map(|(_, last)| last.clone()).unwrap_or_else(|| k.clone());
                PeriodArtist {
                    name: display,
                    plays: v.0,
                    hours: (v.1 as f64 / 3600.0 * 10.0).round() / 10.0,
                    landscape_url: None,
                }
            });
        let top_album = b.albums.iter().max_by_key(|(_, v)| v.1).map(|(k, v)| {
            // The key is lowercased for grouping; show the spelling that was
            // actually submitted, or the card reads "epitone project".
            let akey = k.split('|').next().unwrap_or("");
            let artist =
                display.get(akey).map(|(_, last)| last.clone()).unwrap_or_else(|| akey.to_string());
            PeriodAlbum { name: v.0.clone(), artist, plays: v.1 }
        });
        let top_track = b.tracks.values().max_by_key(|v| v.2).map(|v| PeriodTrack {
            title: v.0.clone(),
            artist: v.1.clone(),
            plays: v.2,
        });

        // "On repeat": the heaviest single day of a single track.
        let mut repeat: Vec<_> = b.per_day.values().filter(|v| v.2 >= 3).collect();
        repeat.sort_by(|a, b| b.2.cmp(&a.2));
        let on_repeat: Vec<PeriodTrack> = repeat
            .iter()
            .take(4)
            .map(|v| PeriodTrack { title: v.0.clone(), artist: v.1.clone(), plays: v.2 })
            .collect();

        // The crush: the most-played artist you had never played before.
        // Only where there was a "before". In the oldest period every artist is
        // new by construction, so the crush would just be the top artist wearing
        // a different label.
        let crush = prev
            .and(b.new_artists.iter().max_by_key(|(_, p)| **p))
            .filter(|(_, p)| **p >= 5)
            .map(|(k, p)| {
                // The *first* spelling here, not the latest — the forward scan
                // this replaces took the oldest, and a crush is about how an
                // artist first appeared.
                let display =
                    display.get(k).map(|(first, _)| first.clone()).unwrap_or_else(|| k.clone());
                Crush {
                    name: display,
                    plays: *p,
                    since: b.first_seen_at.get(k).copied().unwrap_or(0),
                }
            });

        let tag_total: i64 = b.genres.values().sum();
        let mut gs: Vec<_> = b.genres.iter().collect();
        gs.sort_by(|a, b| b.1.cmp(a.1));
        let genres: Vec<GenreShare> = gs
            .iter()
            .take(6)
            .map(|(n, c)| GenreShare {
                name: (*n).clone(),
                pct: if tag_total > 0 {
                    (**c as f64 / tag_total as f64 * 100.0).round() as i64
                } else {
                    0
                },
            })
            .collect();

        // Genre share per sub-bucket, for the year chapter's band chart. The
        // three biggest genres *of the period* are tracked by name across every
        // bucket and everything else is folded into "other", so the bands stay
        // the same three colours across the year instead of renaming themselves
        // month to month — a stacked area whose series change identity partway
        // along is unreadable.
        let top_names: Vec<String> = gs.iter().take(3).map(|(n, _)| (*n).clone()).collect();
        let sub_genres: Vec<Vec<f64>> = b
            .sub_genres
            .iter()
            .map(|counts| {
                let total: i64 = counts.values().sum();
                let mut shares: Vec<f64> = top_names
                    .iter()
                    .map(|n| {
                        if total > 0 {
                            counts.get(n).copied().unwrap_or(0) as f64 / total as f64
                        } else {
                            0.0
                        }
                    })
                    .collect();
                // A bucket with no tags at all is *empty*, not "100% other" —
                // emitting the remainder there would draw a full-height band
                // across every month you did not listen in, which reads as a
                // genre you played rather than as an absence.
                if total == 0 {
                    shares.push(0.0);
                } else {
                    // "Other" is the remainder rather than its own sum, so the
                    // bands stack to exactly 1 and never overshoot the plot.
                    let named: f64 = shares.iter().sum();
                    shares.push((1.0 - named).max(0.0));
                }
                shares
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
            .max_by_key(|(_, (n, _))| *n)
            .and_then(|(cid, _)| chain_name.get(cid).cloned());

        // ── What you listened *through* ──────────────────────────────────
        //
        // The pillar the rest of this page cannot show: every other card is
        // about *what* played, and this is the only one about how. Hours rather
        // than plays alone, because a chain used for three long records is not
        // the one used for thirty short tracks.
        let mut chain_rows: Vec<(&crate::models::SignalChain, i64, i64)> = b
            .chains
            .iter()
            .filter_map(|(cid, (plays, secs))| chain_by_id.get(cid).map(|c| (*c, *plays, *secs)))
            .collect();
        chain_rows.sort_by(|a, z| z.2.cmp(&a.2).then(z.1.cmp(&a.1)));

        // The gear behind those chains. **A set per chain**, because a chain
        // naming the same piece at two steps must not be charged twice for the
        // same hour — the trap `get_equipment` hit once with a `json_each`
        // join. Keyed on the equipment row where the step was picked from the
        // user's gear, and on the name where it was typed.
        let mut gear: HashMap<String, (String, String, i64, i64)> = HashMap::new();
        for (chain, plays, secs) in &chain_rows {
            let mut seen: HashSet<String> = HashSet::new();
            for step in &chain.components {
                let key = step
                    .equipment_id
                    .map(|i| format!("e{i}"))
                    .unwrap_or_else(|| step.name.trim().to_lowercase());
                if key.is_empty() || !seen.insert(key.clone()) {
                    continue;
                }
                let e = gear.entry(key).or_insert_with(|| {
                    (step.name.clone(), format!("{:?}", step.component_type).to_lowercase(), 0, 0)
                });
                e.2 += plays;
                e.3 += secs;
            }
        }
        let mut gear_rows: Vec<_> = gear.into_values().collect();
        gear_rows.sort_by(|a, z| z.3.cmp(&a.3).then(z.2.cmp(&a.2)));

        let gear_json = PeriodGear {
            chains: chain_rows
                .iter()
                .map(|(c, plays, secs)| GearChain {
                    name: c.name.clone(),
                    icon: c.icon.clone(),
                    plays: *plays,
                    hours: (*secs as f64 / 3600.0 * 10.0).round() / 10.0,
                })
                .collect(),
            items: gear_rows
                .iter()
                .take(8)
                .map(|(name, kind, plays, secs)| GearItem {
                    name: name.clone(),
                    kind: kind.clone(),
                    plays: *plays,
                    hours: (*secs as f64 / 3600.0 * 10.0).round() / 10.0,
                })
                .collect(),
            // Both stated, never implied. Most listens on most histories have
            // no chain, and a panel that folded those away would read as a
            // complete account of the period rather than of the part of it
            // anybody attributed.
            unattributed: b.chainless,
            // The hours are a **floor**: a listen whose source reported no
            // length contributes plays but no time, the same confession
            // `total_hours` makes on the dashboard.
            no_duration: b.chain_seconds_unknown,
        };

        // ── Who you heard it with ────────────────────────────────────────
        //
        // **Marked, never "social".** `company` is null on everything nobody
        // has tagged, which is almost all listening — so `plays` travels beside
        // it as the denominator, and nothing here presents a share. Reading
        // null as "alone" is the fidelity-card mistake exactly: a number that
        // looks like it is about listening and is actually about how the
        // listening was recorded.
        let mut people: Vec<(&String, &i64)> = b.company_people.iter().collect();
        people.sort_by(|a, z| z.1.cmp(a.1).then(a.0.cmp(z.0)));
        let company_json = PeriodCompany {
            marked_plays: b.company_plays,
            marked_hours: (b.company_seconds as f64 / 3600.0 * 10.0).round() / 10.0,
            of_plays: b.plays,
            people: people
                .iter()
                .take(8)
                .map(|(name, plays)| Person { name: (*name).clone(), plays: **plays })
                .collect(),
        };

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
            PrevPeriod {
                label: p.label.clone(),
                plays: p.plays,
                hours: (p.seconds as f64 / 3600.0 * 10.0).round() / 10.0,
                avg_daily: (p.plays as f64 / pd).round() as i64,
                artists: p.artists.len(),
                albums: p.albums.len(),
                tracks: p.tracks.len(),
                sub: p.sub.clone(),
            }
        });

        periods.push(Period {
            key: key.clone(),
            label: b.label.clone(),
            start: b.start,
            end: b.end,
            plays: b.plays,
            hours: (b.seconds as f64 / 3600.0 * 10.0).round() / 10.0,
            avg_daily: (b.plays as f64 / days).round() as i64,
            artists: b.artists.len(),
            albums: b.albums.len(),
            tracks: b.tracks.len(),
            new_artists: b.new_artists.len(),
            top_artist,
            top_album,
            top_track,
            clock: b.clock,
            sub: b.sub.clone(),
            sub_labels: sub_labels(unit),
            genres,
            // The three biggest genres by name plus "other", and their share in
            // each sub-bucket. Only the year chapter draws these.
            genre_bands: GenreBands { names: top_names, shares: sub_genres },
            // Where this period sits among *your own* periods by play count,
            // 1 being the busiest. This is what fills the slot the design gave
            // to "you out-listened 88% of Tapedeck listeners" — a cross-user
            // percentile needs other people's listening and, on a private
            // household instance, quietly reports how much everyone else plays.
            // A rank against your own history says something comparable and
            // needs nobody else.
            rank: rank_of.get(key).copied().unwrap_or(0),
            rank_of: total_periods,
            colour,
            on_repeat,
            crush,
            shelf: ShelfSummary {
                sides,
                rows: shelf
                    .iter()
                    .take(5)
                    .map(|(title, artist, side, spins)| ShelfRow {
                        title: title.clone(),
                        artist: artist.clone(),
                        side: side.clone(),
                        spins: *spins,
                    })
                    .collect(),
            },
            top_chain,
            gear: gear_json,
            company: company_json,
            // Five numbers between 0 and 1, and the same five for "you,
            // usually" — every one answerable from any history, and none of
            // them a comparison against another person. See `fingerprint`.
            fingerprint: Fingerprint {
                axes: FINGERPRINT_AXES,
                you: fingerprint(b),
                usual,
                // Whether the baseline rests on anything. One period is not a
                // habit, and a radar that silently compared a month against
                // itself would draw two identical shapes and mean nothing by it.
                usual_periods,
            },
            milestones: milestones.get(key).cloned().unwrap_or_default(),
            prev: prev_json,
        });
    }

    // Wide images for the artists that head a chapter.
    //
    // One query for the whole page rather than one per card — a year view has
    // twelve of them. Asked for *after* the periods are built, since only then
    // is it known which artists actually won; the alternative is fetching an
    // image for every artist in the history to use twelve.
    //
    // A missing one is normal and not a gap to fill: nothing Tapedeck can reach
    // supplies a landscape, so it is there only if the user chose one. The card
    // falls back to the initial, which is the design rather than a placeholder
    // for a photo that never arrives.
    let top_names: Vec<String> =
        periods.iter().filter_map(|p| p.top_artist.as_ref().map(|a| a.name.clone())).collect();
    let landscapes = state.db.artist_landscapes(&top_names).await.unwrap_or_default();
    for p in &mut periods {
        let Some(a) = p.top_artist.as_mut() else {
            continue;
        };
        if let Some(url) = landscapes.get(&crate::db::normalise_artist(&a.name)) {
            a.landscape_url = Some(url.clone());
        }
    }

    (
        StatusCode::OK,
        Json(Reports {
            unit: unit.as_str(),
            timezone: tz.name().to_string(),
            periods,
            // What the stepper needs to know where the ends are: how many
            // periods exist at this bucket size, and where this page sits.
            offset,
            total_periods,
            // The scan is capped like every other analytics read; say so rather
            // than silently reporting on a truncated history.
            capped: Some(scanned >= state.analytics_row_cap),
        }),
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

/// `4182` → `4,182`. Shared with the share image, so a card cannot group a
/// number differently from the chapter it was rendered from.
pub fn thousands(n: i64) -> String {
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

#[derive(utoipa::OpenApi)]
#[openapi(paths(reports))]
pub struct Api;

pub fn create_reports_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/reports", get(reports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ts: i64, artist: &str, credit: Option<&str>, album: &str) -> crate::db::ReportRow {
        crate::db::ReportRow {
            timestamp: ts,
            artist: artist.into(),
            artist_credit: credit.map(str::to_string),
            title: "a track".into(),
            album: Some(album.into()),
            duration: Some(240),
            genres: None,
            chain_id: None,
            company: None,
        }
    }

    /// A period entirely inside one hour reports **zero** spread, not "-0".
    ///
    /// `-p * ln(p)` with `p == 1` is a negative zero, and `clamp` keeps it —
    /// so the axis rendered as "-0" on the page. Harmless arithmetically and it
    /// reads as a bug, which for a number nobody can check by eye is the same
    /// thing as being one.
    #[test]
    fn a_single_hour_of_listening_is_no_spread_rather_than_minus_zero() {
        let rows: Vec<_> =
            (0..5).map(|i| row(1_700_000_000 + i * 60, "Burial", None, "Untrue")).collect();
        let f = fold_periods(rows, Tz::UTC, Unit::Month);
        let spread = fingerprint(&f.buckets[&f.order[0]])[3];
        assert_eq!(spread, 0.0);
        assert!(spread.is_sign_positive(), "a negative zero renders as \"-0\"");
    }

    /// A run is three in a row from one album, and the play that completes it
    /// brings the earlier two with it.
    ///
    /// The axis exists to separate "put a record on" from "shuffled", so a run
    /// that is counted from its third play onwards would under-report every
    /// record ever played by exactly two listens.
    #[test]
    fn a_run_counts_the_plays_that_made_it_not_only_the_ones_after() {
        let album = |ts: i64, a: &str| {
            let mut r = row(ts, "Burial", None, a);
            r.album = Some(a.into());
            r
        };
        // Three from one record, then one from another.
        let rows = vec![
            album(1_700_000_000, "Untrue"),
            album(1_700_000_100, "Untrue"),
            album(1_700_000_200, "Untrue"),
            album(1_700_000_300, "Kid A"),
        ];
        let f = fold_periods(rows, Tz::UTC, Unit::Month);
        assert_eq!(f.buckets[&f.order[0]].run_plays, 3, "all three, not one");

        // Two in a row is not a run — it happens on shuffle often enough to
        // mean nothing, which is what `RUN_LEN` is for.
        let rows = vec![album(1_700_000_000, "Untrue"), album(1_700_000_100, "Untrue")];
        let f = fold_periods(rows, Tz::UTC, Unit::Month);
        assert_eq!(f.buckets[&f.order[0]].run_plays, 0);

        // A listen with no album **breaks** a run rather than extending it: it
        // is not evidence of a record being played through.
        let mut broken = vec![album(1_700_000_000, "Untrue"), album(1_700_000_100, "Untrue")];
        broken.push({
            let mut r = row(1_700_000_150, "Burial", None, "");
            r.album = None;
            r
        });
        broken.push(album(1_700_000_200, "Untrue"));
        let f = fold_periods(broken, Tz::UTC, Unit::Month);
        assert_eq!(f.buckets[&f.order[0]].run_plays, 0, "the gap ends the run");
    }

    /// Every axis is answerable from a listen that carries nothing but a time,
    /// an artist and a title — and that is the point of choosing these five.
    ///
    /// The tempting axes are the Tapedeck-only ones (chain, format, attention)
    /// and each is *unknown* for most of a real history, so a shape drawn from
    /// them would picture how the listening was **recorded** while appearing to
    /// picture the listening. This pins that a metadata-free history still
    /// produces a real shape rather than a collapsed one.
    #[test]
    fn the_fingerprint_needs_no_metadata_a_history_might_be_missing() {
        let bare = |ts: i64, artist: &str, title: &str| crate::db::ReportRow {
            timestamp: ts,
            artist: artist.into(),
            artist_credit: None,
            title: title.into(),
            album: None,
            duration: None,
            genres: None,
            chain_id: None,
            company: None,
        };
        let rows = vec![
            bare(1_700_000_000, "Burial", "Archangel"),
            bare(1_700_003_600, "Aphex Twin", "Xtal"),
            bare(1_700_007_200, "Burial", "Archangel"),
        ];
        let f = fold_periods(rows, Tz::UTC, Unit::Month);
        let fp = fingerprint(&f.buckets[&f.order[0]]);
        assert_eq!(fp.len(), 5);
        assert!(fp.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "got {fp:?}");
        // Three plays, two distinct tracks — one of them was a repeat.
        assert!((fp[1] - 1.0 / 3.0).abs() < 1e-9, "repeat axis, got {}", fp[1]);
        // Nothing carried an album, so nothing can have been a run.
        assert_eq!(fp[4], 0.0);
    }

    /// An empty period is a flat shape, not a division by zero.
    #[test]
    fn a_period_with_nothing_in_it_has_no_shape_rather_than_a_wrong_one() {
        let b = Bucket::default();
        assert_eq!(fingerprint(&b), [0.0; 5]);
    }

    /// Gear hours charge a chain once per piece, however often it names one.
    ///
    /// A chain can legitimately list the same box at two steps — a
    /// preamp/amp pair from one integrated unit, a cable back through the same
    /// interface — and charging it twice for the same hour is the trap
    /// `get_equipment` hit once with a `json_each` join, where a chain naming
    /// the same gear at two steps counted its whole running time twice.
    #[test]
    fn a_chain_charges_each_piece_of_gear_once_however_often_it_names_it() {
        use crate::models::{ChainComponent, ComponentType};
        let dup = |name: &str, t: ComponentType| ChainComponent {
            component_type: t,
            name: name.into(),
            detail: None,
            since: None,
            equipment_id: None,
        };
        // The fold the emit does, in miniature.
        let components =
            vec![dup("Bifrost", ComponentType::Dac), dup("Bifrost", ComponentType::Amp)];
        let mut seen = std::collections::HashSet::new();
        let mut charged = 0;
        for step in &components {
            if seen.insert(step.name.trim().to_lowercase()) {
                charged += 1;
            }
        }
        assert_eq!(charged, 1, "one box, one charge — not one per step naming it");
    }

    /// Company is **counted**, never turned into a share here.
    ///
    /// `company` is null on everything nobody has tagged, which is almost all
    /// listening. A bare percentage would therefore be answering "how much of
    /// this was *recorded* as social" while appearing to answer "how much of
    /// this *was* social" — the fidelity-card failure exactly, and the reason
    /// the History filter is three states rather than two.
    ///
    /// So the payload carries `marked_plays` **and** `of_plays`, and this pins
    /// that the denominator travels: without it a client can only guess, and
    /// the guess is always the wrong one.
    #[test]
    fn the_company_figure_carries_what_it_is_a_figure_of() {
        let rows = vec![
            {
                let mut r = row(1_700_000_000, "Burial", None, "Untrue");
                r.company = Some("noor, tobias".into());
                r
            },
            {
                let mut r = row(1_700_000_100, "Burial", None, "Untrue");
                r.company = Some("noor".into());
                r
            },
            // Unmarked. Not "alone" — nobody has said either way.
            row(1_700_000_200, "Aphex Twin", None, "SAW"),
        ];
        let f = fold_periods(rows, Tz::UTC, Unit::Month);
        let b = &f.buckets[&f.order[0]];
        assert_eq!(b.company_plays, 2, "only what is marked");
        assert_eq!(b.plays, 3, "and the whole period, so a share is answerable honestly");
        assert_eq!(b.company_people.get("noor"), Some(&2));
        assert_eq!(b.company_people.get("tobias"), Some(&1), "the comma splits into people");
        assert!(
            !b.company_people.contains_key("noor, tobias"),
            "one oddly named person is what not splitting looks like"
        );
    }

    /// A listen with no chain is counted as such rather than folded away.
    ///
    /// On most histories this is nearly everything — a gear panel that quietly
    /// ignored it would read as a complete account of the period instead of an
    /// account of the part somebody attributed. Same for a listen whose source
    /// reported no length: the hours are a floor and the card says so.
    #[test]
    fn the_gear_panel_says_what_it_cannot_account_for() {
        let rows = vec![
            {
                let mut r = row(1_700_000_000, "Burial", None, "Untrue");
                r.chain_id = Some(7);
                r
            },
            {
                let mut r = row(1_700_000_100, "Burial", None, "Untrue");
                r.chain_id = Some(7);
                r.duration = None;
                r
            },
            row(1_700_000_200, "Aphex Twin", None, "SAW"),
        ];
        let f = fold_periods(rows, Tz::UTC, Unit::Month);
        let b = &f.buckets[&f.order[0]];
        assert_eq!(b.chains.get(&7), Some(&(2, 240)), "two plays, one of them timed");
        assert_eq!(b.chain_seconds_unknown, 1, "and the untimed one is admitted");
        assert_eq!(b.chainless, 1, "as is the one that ran through nothing");
    }

    /// The superlatives print the spelling somebody actually submitted, and the
    /// three of them do not agree about *which* one — the top artist and top
    /// album take the newest, the crush takes the oldest. That used to fall out
    /// of scanning `rows` forwards or backwards per period; the fold records
    /// both ends so the same answers survive the scan being removed.
    #[test]
    fn the_fold_keeps_both_ends_of_every_spelling() {
        let mut m = HashMap::new();
        note_spelling(&mut m, "linkin park", "LINKIN PARK");
        note_spelling(&mut m, "linkin park", "Linkin Park");
        assert_eq!(m["linkin park"], ("LINKIN PARK".to_string(), "Linkin Park".to_string()));
        // An empty spelling is not a spelling — it must not become the answer a
        // card prints.
        note_spelling(&mut m, "linkin park", "");
        assert_eq!(m["linkin park"].1, "Linkin Park");
    }

    /// A performer who is only ever a *credit* still gets a real name.
    ///
    /// On a compilation `artist` is the placeholder and the performer is in
    /// `artist_credit` (v0.22.1, v0.57.0). Two of the three lookups this
    /// replaced consulted `artist` alone, so such a key fell back to printing
    /// the lowercased grouping key — which is the "epitone project" bug wearing
    /// different clothes.
    #[test]
    fn a_performer_who_is_only_ever_credited_is_still_named() {
        let rows = vec![
            row(
                1_700_000_000,
                "Various Artists",
                Some("Punch; Kim Na Young"),
                "Do You Like Brahms?",
            ),
            row(
                1_700_003_600,
                "Various Artists",
                Some("Punch; Kim Na Young"),
                "Do You Like Brahms?",
            ),
        ];
        let f = fold_periods(rows, Tz::UTC, Unit::Month);
        assert_eq!(f.display.get("punch").map(|(_, l)| l.as_str()), Some("Punch"));
        assert_eq!(f.display.get("kim na young").map(|(_, l)| l.as_str()), Some("Kim Na Young"));
        // And the placeholder is recorded too — it is a real string on the row.
        // What it must not do is *win* a superlative, which the handler filters.
        assert!(f.display.contains_key("various artists"));
    }

    /// The fold is the whole of the pass: one period here, with its plays and
    /// its ordering. It runs on `spawn_blocking`, so it has to be a plain
    /// function over rows — this is the guard that it stayed one.
    #[test]
    fn the_fold_buckets_by_period_in_the_readers_timezone() {
        let rows = vec![
            // 2023-12-31 23:30 UTC is already 2024-01-01 in CET, the trap the
            // trajectory buckets already document.
            row(1_704_065_400, "Burial", None, "Untrue"),
            row(1_704_100_000, "Burial", None, "Untrue"),
        ];
        let f = fold_periods(rows, Tz::Europe__Berlin, Unit::Month);
        assert_eq!(f.order.len(), 1, "both listens are the same local month");
        assert!(f.order[0].starts_with("2024-01"), "got {}", f.order[0]);
        assert_eq!(f.buckets[&f.order[0]].plays, 2);
    }

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

/// What one pass over the history leaves behind.
struct Folded {
    buckets: HashMap<String, Bucket>,
    /// Folded artist key → the spellings actually submitted, as
    /// `(first seen, last seen)`.
    ///
    /// Built here because the fold is already walking every row. The three
    /// superlatives below used to recover this by re-scanning `rows` **per
    /// period** — a scan of the whole history for each card on the page, which
    /// is the correlated-scan shape this codebase keeps finding. It also kept
    /// `rows` alive past the fold, which is what stopped the whole pass moving
    /// off the async worker.
    ///
    /// Both ends are kept because the three callers genuinely disagreed: the
    /// top artist and top album took the newest spelling and the crush took the
    /// oldest, and quietly unifying them would change what a card says.
    display: HashMap<String, (String, String)>,
    /// Period keys in the order they were first seen — i.e. ascending, since
    /// `report_rows` is the one listing query that scans oldest-first.
    order: Vec<String>,
    milestones: HashMap<String, Vec<String>>,
}

/// Remember a spelling for a folded key, keeping the first and the latest.
fn note_spelling(map: &mut HashMap<String, (String, String)>, key: &str, spelling: &str) {
    if spelling.is_empty() {
        return;
    }
    match map.get_mut(key) {
        Some(e) => e.1 = spelling.to_string(),
        None => {
            map.insert(key.to_string(), (spelling.to_string(), spelling.to_string()));
        }
    }
}

/// Bucket every listen into its period, in the reader's timezone.
///
/// Split out so it can run on [`tokio::task::spawn_blocking`] — see the call
/// site — and because a pure function over a slice of rows is testable, where a
/// hundred and thirty lines inside a handler is not.
fn fold_periods(rows: Vec<ReportRow>, tz: Tz, unit: Unit) -> Folded {
    let mut buckets: HashMap<String, Bucket> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    // First-ever sighting of an artist, across the whole history. Because the
    // scan is ascending, the first time we see a name is the real first time.
    let mut seen_artists: HashSet<String> = HashSet::new();
    let mut running_total: i64 = 0;
    // Round-number listens, attributed to whichever period crossed them.
    let mut milestones: HashMap<String, Vec<String>> = HashMap::new();
    let mut display: HashMap<String, (String, String)> = HashMap::new();

    for r in &rows {
        // The submitted spellings, for the superlatives. Credits are recorded
        // too, not only the row's own `artist`: a key can belong to a performer
        // who is never the primary, and without this the card falls back to
        // printing the lowercased grouping key.
        note_spelling(&mut display, &r.artist.trim().to_lowercase(), r.artist.trim());
        for c in r.artist_credit.as_deref().into_iter().flat_map(|c| c.split(';')).map(str::trim) {
            note_spelling(&mut display, &c.to_lowercase(), c);
        }
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
                sub_genres: vec![HashMap::new(); sub_bucket_count(unit)],
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
        let sub_index = si.min(b.sub.len().saturating_sub(1));
        b.days.insert(dt.num_days_from_ce());

        // Runs of one album, counted as the ascending scan goes past them.
        // Ascending is what makes this possible at all — `report_rows` is the
        // one listing query that scans oldest-first, and a run is a fact about
        // order.
        match r.album.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            Some(album) if b.run_album.as_deref() == Some(album) => {
                b.run_len += 1;
                // Nothing counts until a run exists. The play that *completes*
                // one brings the earlier ones with it; every play after that is
                // one more. Three branches, not two — an `else { 1 }` scores the
                // second play of a pair that never becomes a run, which is
                // exactly what shuffle produces all day.
                b.run_plays += match b.run_len.cmp(&RUN_LEN) {
                    std::cmp::Ordering::Less => 0,
                    std::cmp::Ordering::Equal => RUN_LEN,
                    std::cmp::Ordering::Greater => 1,
                };
            }
            Some(album) => {
                b.run_album = Some(album.to_string());
                b.run_len = 1;
            }
            // A listen with no album breaks the run rather than extending it —
            // it is not evidence of a record being played through.
            None => {
                b.run_album = None;
                b.run_len = 0;
            }
        }

        // Who actually played it.
        //
        // On a compilation the `artist` column is "Various Artists" — a filing
        // convention, not a performer — and the real name is in `artist_credit`,
        // which is exactly what v0.22.1 kept it for. Preferring it here means a
        // compilation track counts towards the person who played it rather than
        // piling up under a placeholder that then wins every superlative on the
        // page. Falls back to `artist` when there is no credit, which is the
        // overwhelmingly common case.
        // `artist_credit` is everyone credited, joined with "; " and the
        // primary first — the shape `save_scrobble` and `fold_artist_credit`
        // both write. The primary is the one to count; the whole joined string
        // would be its own key and no more useful than the placeholder.
        let performer = if crate::db::is_various_artists(&r.artist) {
            r.artist_credit
                .as_deref()
                .and_then(|c| c.split(';').next())
                .map(str::trim)
                .filter(|c| !c.is_empty() && !crate::db::is_various_artists(c))
                .unwrap_or(&r.artist)
        } else {
            &r.artist
        };
        let akey = performer.trim().to_lowercase();

        // Everyone credited, not only the one whose name the row carries.
        // "Hilary Hahn plays Bach" is a listen for both of them, and a report
        // that named only Bach would disagree with the charts and the artist
        // page, which fan out over `scrobble_artists`. Same question, three
        // screens, one answer.
        //
        // The tally fans out; **everything keyed on `akey` below does not** —
        // an album belongs to one artist, and the crush is about the artist a
        // record introduced you to. Those are facts about the primary.
        let seconds = r.duration.unwrap_or(0).max(0);
        for name in credited_names(performer, r.artist_credit.as_deref()) {
            let e = b.artists.entry(name).or_insert((0, 0));
            e.0 += 1;
            e.1 += seconds;
        }

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
            let e = b.chains.entry(cid).or_insert((0, 0));
            e.0 += 1;
            match r.duration {
                Some(d) if d > 0 => e.1 += d,
                _ => b.chain_seconds_unknown += 1,
            }
        } else {
            b.chainless += 1;
        }

        // Who it was heard with. Split on the comma the writer joins with, so
        // "noor F, tobias F" is two people rather than one oddly named one.
        if let Some(with) = r.company.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            b.company_plays += 1;
            b.company_seconds += r.duration.unwrap_or(0).max(0);
            for name in with.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                *b.company_people.entry(name.to_string()).or_insert(0) += 1;
            }
        }

        // A track carries several tags; each is counted once. This is a share
        // of *tag mentions*, not of listens, and every consumer must say so.
        if let Some(g) = r.genres.as_deref() {
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(g) {
                for t in tags {
                    let t = t.trim().to_lowercase();
                    if !t.is_empty() {
                        *b.genres.entry(t.clone()).or_insert(0) += 1;
                        if let Some(sg) = b.sub_genres.get_mut(sub_index) {
                            *sg.entry(t).or_insert(0) += 1;
                        }
                    }
                }
            }
        }

        if seen_artists.insert(akey.clone()) {
            *b.new_artists.entry(akey.clone()).or_insert(0) += 1;
            b.first_seen_at.insert(akey.clone(), r.timestamp);
        } else if let Some(n) = b.new_artists.get_mut(&akey) {
            // `Entry`-shaped rather than `contains_key` + `get_mut().unwrap()`:
            // correct as written and one edit to the condition away from
            // panicking a whole report request.
            *n += 1;
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
    Folded { buckets, order, milestones, display }
}
