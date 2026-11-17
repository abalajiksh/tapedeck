//! The Insights tabs that split listening by kind: Classical, Scores &
//! Ambient, the seasonal lenses (Christmas, Halloween, Easter) and B-Sides,
//! plus the Statistics composition chart and the overrides behind all of them.
//!
//! What a listen *is* was decided by `classify` in the background; everything
//! here reads that decision (`scrobbles.facets`). Classical and score/ambient
//! leave Top Charts per listener; the seasons take nothing out of anywhere.

use crate::classify::{self, season, titles::Season};
use crate::db::{InsightFilter, InsightRow, RoleCoverage, SavedPlaylistTrack};
use crate::logging::OrWarn;
use crate::report_image::mixdown::{href_ok, Art, Mixdown};
use crate::report_image::{card::Named, card::Titled, Card, SeasonCard, Size, Theme};
use crate::report_video;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use chrono::{Datelike, Duration, NaiveDate, TimeZone};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

use super::auth::{ReadAuth, WriteAuth};
use super::models::ErrorResponse;
use super::report_mixdown::{fetch_art, MixdownResponse, MixdownVideo};
use super::AppState;
use crate::openapi::{BadRequest, NotFound, Unauthorized};

fn err(code: u16, msg: impl Into<String>) -> axum::response::Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorResponse { code, error: msg.into() })).into_response()
}

/// The flags this listener keeps out of Top Charts: classical unless
/// `split_classical` is false, score and ambient unless `split_scores` is.
pub async fn split_mask(state: &AppState, user_id: i64) -> i64 {
    let prefs = state
        .db
        .get_preferences(user_id)
        .await
        .or_warn("split_mask: preferences")
        .unwrap_or_default();
    let on = |k: &str| prefs.get(k).and_then(|v| v.as_bool()).unwrap_or(true);
    let mut mask = 0;
    if on("split_classical") {
        mask |= classify::CLASSICAL;
    }
    if on("split_scores") {
        mask |= classify::SCORE | classify::AMBIENT;
    }
    mask
}

fn metric_hours(m: Option<&str>) -> bool {
    m == Some("hours")
}

// ── Classical ──

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ClassicalQuery {
    /// `pieces` (default), `movements` (needs `piece`), `composers`,
    /// `performances`, `conductor`, `orchestra`, `ensemble` or `soloist`.
    pub rank: Option<String>,
    /// `today`, `d7`, `month`, `year` or `all` (default), as on Top Charts.
    pub range: Option<String>,
    /// `hours` (default), `plays` or `hearings` — complete hearings, every
    /// movement of a piece in one sitting; pieces and composers only.
    pub metric: Option<String>,
    /// Narrow to one composer: a key from a `composers` row.
    pub composer: Option<String>,
    /// Narrow to one piece: a key from a `pieces` row.
    pub piece: Option<String>,
    /// Narrow to one performer: a key from a performer row.
    pub performer: Option<String>,
    /// With `rank=soloist`: one instrument or voice, e.g. `violin`.
    pub instrument: Option<String>,
    #[param(minimum = 1, maximum = 500, default = 100)]
    pub limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct NamedCount {
    pub name: String,
    pub count: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ClassicalChart {
    pub rank: String,
    pub metric: String,
    pub rows: Vec<InsightRow>,
    /// Instruments and voices the soloists in this window played, most heard
    /// first — the chips under "Soloists".
    pub instruments: Vec<NamedCount>,
    /// For a performer ranking: how much of the window rests on roles
    /// MusicBrainz states, and how much on roles Tapedeck inferred.
    pub coverage: Option<RoleCoverage>,
    /// Listens the classifier has not reached yet. They are in Top Charts
    /// until it does.
    pub pending: i64,
}

/// Complete hearings per piece and per composer: a multi-movement piece
/// counts once per sitting in which every movement was played; a piece
/// MusicBrainz records as one work with no movements counts every listen.
#[allow(clippy::type_complexity)]
fn hearings(
    rows: &[(Option<i64>, String, Option<String>, Option<i64>, Option<String>)],
) -> (HashMap<String, i64>, HashMap<String, i64>) {
    let mut by_piece: HashMap<String, i64> = HashMap::new();
    let mut by_composer: HashMap<String, i64> = HashMap::new();
    let mut sittings: HashMap<(i64, String), (HashSet<String>, i64, Option<String>)> =
        HashMap::new();
    for (session, piece, leaf, parts, composer) in rows {
        match (leaf, session) {
            (None, _) => {
                *by_piece.entry(piece.clone()).or_default() += 1;
                if let Some(c) = composer {
                    *by_composer.entry(c.clone()).or_default() += 1;
                }
            }
            (Some(leaf), Some(session)) if parts.unwrap_or(0) > 1 => {
                let e = sittings
                    .entry((*session, piece.clone()))
                    .or_insert_with(|| (HashSet::new(), parts.unwrap_or(0), composer.clone()));
                e.0.insert(leaf.clone());
            }
            _ => {}
        }
    }
    for ((_, piece), (heard, parts, composer)) in sittings {
        if heard.len() as i64 >= parts {
            *by_piece.entry(piece).or_default() += 1;
            if let Some(c) = composer {
                *by_composer.entry(c).or_default() += 1;
            }
        }
    }
    (by_piece, by_composer)
}

/// The Classical tab.
///
/// Scope: `read` *(since v0.132)*. Pieces roll movements up to the whole work
/// (Ciaccona → Partita No. 2); an opera is one piece and `rank=movements`
/// with its `piece` opens it. A composer is the *piece's* composer, whoever
/// the listen was credited to. Performers come from MusicBrainz's recording
/// credits, or are inferred (`inferred: 1`) for listens that carry names
/// only; a concerto counts in full for its soloist, conductor and orchestra,
/// so performer shares do not sum to 100%.
#[utoipa::path(
    get,
    path = "/api/v1/insights/classical",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(ClassicalQuery),
    responses(
        (status = 200, description = "OK.", body = ClassicalChart),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn classical(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<ClassicalQuery>,
) -> impl IntoResponse {
    let rank = q.rank.as_deref().unwrap_or("pieces");
    if !matches!(
        rank,
        "pieces"
            | "movements"
            | "composers"
            | "performances"
            | "conductor"
            | "orchestra"
            | "ensemble"
            | "soloist"
    ) {
        return err(400, "rank must be pieces, movements, composers, performances, conductor, orchestra, ensemble or soloist");
    }
    if rank == "movements" && q.piece.is_none() {
        return err(400, "rank=movements needs a piece");
    }
    let metric = q.metric.as_deref().unwrap_or("hours");
    if !matches!(metric, "hours" | "plays" | "hearings") {
        return err(400, "metric must be hours, plays or hearings");
    }
    if metric == "hearings" && !matches!(rank, "pieces" | "composers") {
        return err(400, "hearings are counted for pieces and composers");
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let f = InsightFilter {
        after: super::scrobbles::range_after(q.range.as_deref()),
        before: None,
        composer: q.composer.clone(),
        piece: q.piece.clone(),
        performer: q.performer.clone(),
        instrument: q.instrument.clone(),
    };
    // Hearings rank in Rust, so ask SQL for the whole list first.
    let fetch = if metric == "hearings" { 2000 } else { limit };
    let mut rows =
        match state.db.classical_chart(user.user_id, rank, &f, metric != "plays", fetch).await {
            Ok(r) => r,
            Err(e) => return err(500, e.to_string()),
        };
    if matches!(rank, "pieces" | "composers") {
        let raw = state
            .db
            .classical_hearing_rows(user.user_id, &f)
            .await
            .or_warn("classical: hearings")
            .unwrap_or_default();
        let (by_piece, by_composer) = hearings(&raw);
        let source = if rank == "pieces" { &by_piece } else { &by_composer };
        for r in &mut rows {
            r.hearings = Some(source.get(&r.key).copied().unwrap_or(0));
        }
        if metric == "hearings" {
            rows.sort_by(|a, b| b.hearings.cmp(&a.hearings).then(b.seconds.cmp(&a.seconds)));
            rows.truncate(limit as usize);
        }
    }
    let instruments = state
        .db
        .classical_instruments(user.user_id, &InsightFilter { instrument: None, ..f.clone() })
        .await
        .or_warn("classical: instruments")
        .unwrap_or_default()
        .into_iter()
        .map(|(name, count)| NamedCount { name, count })
        .collect();
    let coverage = if matches!(rank, "conductor" | "orchestra" | "ensemble" | "soloist") {
        state.db.classical_role_coverage(user.user_id, &f).await.or_warn("classical: coverage")
    } else {
        None
    };
    let pending = state
        .db
        .facets_pending(user.user_id, classify::VERSION)
        .await
        .or_warn("facets pending")
        .unwrap_or(0);
    (
        StatusCode::OK,
        Json(ClassicalChart {
            rank: rank.to_string(),
            metric: metric.to_string(),
            rows,
            instruments,
            coverage,
            pending,
        }),
    )
        .into_response()
}

// ── Classical statistics (after Bachtrack's annual report, refitted to listens) ──

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StatsRangeQuery {
    /// `today`, `d7`, `month`, `year` or `all` (default).
    pub range: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[schema(as = ClassicalShare)]
pub struct Share {
    pub name: String,
    pub seconds: i64,
    pub plays: i64,
    /// Composers or performers behind it, where that is counted.
    pub count: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct ComposerYears {
    pub key: String,
    pub name: String,
    /// Plays per year, aligned with `ClassicalYears::years`.
    pub plays: Vec<i64>,
    /// Round anniversaries inside those years.
    pub anniversaries: Vec<YearMark>,
}

#[derive(Serialize, ToSchema)]
pub struct YearMark {
    pub year: i32,
    pub label: String,
}

#[derive(Serialize, ToSchema)]
pub struct ClassicalYears {
    pub years: Vec<i32>,
    pub composers: Vec<ComposerYears>,
}

#[derive(Serialize, ToSchema)]
pub struct Subset {
    /// Seconds in the subset, of `known` seconds whose composer is described.
    pub seconds: i64,
    pub known: i64,
    pub composers: Vec<Share>,
}

#[derive(Serialize, ToSchema)]
pub struct RangeRow {
    pub key: String,
    pub name: String,
    pub role: String,
    /// Distinct composers you heard them play — Bachtrack's "countries
    /// visited", for listens.
    pub composers: i64,
    pub pieces: i64,
    pub plays: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ClassicalStats {
    pub seconds: i64,
    /// Seconds by Bachtrack's seven periods, by composition year, else the
    /// composer's mid-career. `dated` is how many of those seconds were placed
    /// by a composition year.
    pub periods: Vec<Share>,
    pub dated: i64,
    pub unplaced: i64,
    pub forms: Vec<Share>,
    pub years: ClassicalYears,
    pub living: Subset,
    /// Women and non-binary composers.
    pub women: Subset,
    pub composer_countries: Vec<Share>,
    pub performer_countries: Vec<Share>,
    pub range: Vec<RangeRow>,
}

/// Statistics of your classical listening.
///
/// Scope: `read` *(since v0.132)*. After Bachtrack's annual classical music
/// statistics, refitted from concerts to listens: by period and form, your
/// top composers year by year with their anniversaries, how much is by living
/// composers and by women, where composers and performers are from, and the
/// performers who took you across the most composers. Composer facts come
/// from MusicBrainz; `known` says how much of the window they cover.
#[utoipa::path(
    get,
    path = "/api/v1/insights/classical/stats",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(StatsRangeQuery),
    responses(
        (status = 200, description = "OK.", body = ClassicalStats),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn classical_stats(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<StatsRangeQuery>,
) -> impl IntoResponse {
    let f = InsightFilter {
        after: super::scrobbles::range_after(q.range.as_deref()),
        ..Default::default()
    };
    let rows = match state.db.classical_stat_rows(user.user_id, &f).await {
        Ok(r) => r,
        Err(e) => return err(500, e.to_string()),
    };
    let perf = state
        .db
        .classical_performer_rows(user.user_id, &f)
        .await
        .or_warn("classical stats: performers")
        .unwrap_or_default();
    let tz = super::scrobbles::user_tz_for(&state, user.user_id).await;
    let this_year = today(tz).year();
    Json(fold_stats(&rows, &perf, tz, this_year)).into_response()
}

fn share_list(map: HashMap<String, (i64, i64, HashSet<String>)>, limit: usize) -> Vec<Share> {
    let mut v: Vec<Share> = map
        .into_iter()
        .map(|(name, (seconds, plays, who))| Share {
            name,
            seconds,
            plays,
            count: (!who.is_empty()).then_some(who.len() as i64),
        })
        .collect();
    v.sort_by(|a, b| b.seconds.cmp(&a.seconds).then(b.plays.cmp(&a.plays)));
    v.truncate(limit);
    v
}

fn fold_stats(
    rows: &[crate::db::ClassicalStatRow],
    perf: &[crate::db::PerformerStatRow],
    tz: chrono_tz::Tz,
    this_year: i32,
) -> ClassicalStats {
    let mut seconds = 0;
    let (mut dated, mut unplaced) = (0, 0);
    let mut periods: HashMap<String, (i64, i64, HashSet<String>)> = HashMap::new();
    let mut forms: HashMap<String, (i64, i64, HashSet<String>)> = HashMap::new();
    let mut composer_countries: HashMap<String, (i64, i64, HashSet<String>)> = HashMap::new();
    let (mut living, mut women) = (
        Subset { seconds: 0, known: 0, composers: vec![] },
        Subset { seconds: 0, known: 0, composers: vec![] },
    );
    let mut living_who: HashMap<String, (i64, i64, HashSet<String>)> = HashMap::new();
    let mut women_who: HashMap<String, (i64, i64, HashSet<String>)> = HashMap::new();
    // Name, plays per year, born, died.
    type ComposerYearly = (String, BTreeMap<i32, i64>, Option<String>, Option<String>);
    let mut by_composer_year: HashMap<String, ComposerYearly> = HashMap::new();
    let mut composer_totals: HashMap<String, i64> = HashMap::new();
    let mut years: BTreeSet<i32> = BTreeSet::new();

    for r in rows {
        seconds += r.seconds;
        let born = r.born.as_deref().and_then(classify::year_of);
        match classify::placed_year(r.composed_year.map(|y| y as i32), born) {
            Some((y, was_dated)) => {
                if was_dated {
                    dated += r.seconds;
                }
                let e = periods.entry(classify::period(y).to_string()).or_default();
                e.0 += r.seconds;
                e.1 += 1;
            }
            None => unplaced += r.seconds,
        }
        if let Some(form) = &r.form {
            let e = forms.entry(form.clone()).or_default();
            e.0 += r.seconds;
            e.1 += 1;
        }
        if let (Some(key), Some(name)) = (&r.composer_key, &r.composer_name) {
            if let Some(c) = &r.country {
                let e = composer_countries.entry(c.clone()).or_default();
                e.0 += r.seconds;
                e.1 += 1;
                e.2.insert(key.clone());
            }
            if r.born.is_some() || r.gender.is_some() {
                living.known += r.seconds;
                women.known += r.seconds;
                let alive = r.ended != Some(1)
                    && r.died.is_none()
                    && born.is_some_and(|b| this_year - b <= 110);
                if alive {
                    living.seconds += r.seconds;
                    let e = living_who.entry(name.clone()).or_default();
                    e.0 += r.seconds;
                    e.1 += 1;
                }
                if matches!(r.gender.as_deref(), Some("female" | "non-binary")) {
                    women.seconds += r.seconds;
                    let e = women_who.entry(name.clone()).or_default();
                    e.0 += r.seconds;
                    e.1 += 1;
                }
            }
            let y = local_day(tz, r.timestamp).year();
            years.insert(y);
            let e = by_composer_year
                .entry(key.clone())
                .or_insert_with(|| (name.clone(), BTreeMap::new(), r.born.clone(), r.died.clone()));
            *e.1.entry(y).or_default() += 1;
            *composer_totals.entry(key.clone()).or_default() += 1;
        }
    }
    living.composers = share_list(living_who, 8);
    women.composers = share_list(women_who, 8);

    // The five composers you played most in the window, year by year.
    let mut top: Vec<(&String, &i64)> = composer_totals.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let year_list: Vec<i32> = match (years.first(), years.last()) {
        (Some(&a), Some(&b)) => (a..=b).collect(),
        _ => vec![],
    };
    let composers = top
        .into_iter()
        .take(5)
        .filter_map(|(key, _)| {
            let (name, per, born, died) = by_composer_year.get(key)?;
            let anniversaries = year_list
                .iter()
                .flat_map(|&y| {
                    classify::anniversary::of_artist(
                        name,
                        Some("Person"),
                        born.as_deref(),
                        died.as_deref(),
                        died.is_some(),
                        0,
                        y,
                        NaiveDate::from_ymd_opt(y, 1, 1).expect("1 January"),
                    )
                    .into_iter()
                    .map(move |a| YearMark { year: y, label: a.label })
                })
                .collect();
            Some(ComposerYears {
                key: key.clone(),
                name: name.clone(),
                plays: year_list.iter().map(|y| per.get(y).copied().unwrap_or(0)).collect(),
                anniversaries,
            })
        })
        .collect();

    // Performers: where they are from, and who took you across most composers.
    let mut performer_countries: HashMap<String, (i64, i64, HashSet<String>)> = HashMap::new();
    // Name, composers, pieces, plays — per performer and role.
    type Reach = (String, HashSet<String>, HashSet<String>, i64);
    let mut reach: HashMap<(String, String), Reach> = HashMap::new();
    for (key, name, role, country, composer, piece, plays) in perf {
        if let Some(c) = country {
            let e = performer_countries.entry(c.clone()).or_default();
            e.1 += plays;
            e.2.insert(key.clone());
        }
        let e = reach
            .entry((key.clone(), role.clone()))
            .or_insert_with(|| (name.clone(), HashSet::new(), HashSet::new(), 0));
        if let Some(c) = composer {
            e.1.insert(c.clone());
        }
        e.2.insert(piece.clone());
        e.3 += plays;
    }
    let mut range: Vec<RangeRow> = reach
        .into_iter()
        .map(|((key, role), (name, comps, pieces, plays))| RangeRow {
            key,
            name,
            role,
            composers: comps.len() as i64,
            pieces: pieces.len() as i64,
            plays,
        })
        .filter(|r| r.composers > 1)
        .collect();
    range.sort_by(|a, b| {
        b.composers.cmp(&a.composers).then(b.pieces.cmp(&a.pieces)).then(b.plays.cmp(&a.plays))
    });
    range.truncate(10);
    let mut performer_countries = share_list(performer_countries, 12);
    performer_countries.sort_by(|a, b| b.plays.cmp(&a.plays));

    let mut periods_v: Vec<Share> = classify::PERIODS
        .iter()
        .map(|p| {
            let (s, n, _) = periods.remove(*p).unwrap_or_default();
            Share { name: p.to_string(), seconds: s, plays: n, count: None }
        })
        .collect();
    periods_v.retain(|p| p.plays > 0 || seconds > 0);
    ClassicalStats {
        seconds,
        periods: periods_v,
        dated,
        unplaced,
        forms: share_list(forms, 10),
        years: ClassicalYears { years: year_list, composers },
        living,
        women,
        composer_countries: share_list(composer_countries, 12),
        performer_countries,
        range,
    }
}

#[derive(Serialize, ToSchema)]
pub struct TimelineComposer {
    pub key: String,
    pub name: String,
    pub born: Option<i32>,
    pub died: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct TimelinePiece {
    pub key: String,
    pub title: String,
    pub composer: String,
    /// Where it stands: the year composed, or the composer's mid-career.
    pub year: i32,
    /// True when `year` is the composition year MusicBrainz records.
    pub dated: bool,
    pub plays: i64,
    pub seconds: i64,
}

#[derive(Serialize, ToSchema)]
pub struct Timeline {
    /// Columns, by birth.
    pub composers: Vec<TimelineComposer>,
    pub pieces: Vec<TimelinePiece>,
    /// Pieces with neither a composition year nor a composer's birth year.
    pub unplaced: i64,
}

/// Your classical listening on a timeline — Chronologue's grid, drawn from
/// your history.
///
/// Scope: `read` *(since v0.132)*. One column per composer, in order of
/// birth, and each piece you heard at the year it was composed. A piece
/// MusicBrainz gives no year stands at its composer's mid-career, with
/// `dated: false`.
#[utoipa::path(
    get,
    path = "/api/v1/insights/classical/timeline",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(StatsRangeQuery),
    responses(
        (status = 200, description = "OK.", body = Timeline),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn classical_timeline(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<StatsRangeQuery>,
) -> impl IntoResponse {
    let f = InsightFilter {
        after: super::scrobbles::range_after(q.range.as_deref()),
        ..Default::default()
    };
    let rows = match state.db.classical_timeline(user.user_id, &f).await {
        Ok(r) => r,
        Err(e) => return err(500, e.to_string()),
    };
    let mut composers: HashMap<String, TimelineComposer> = HashMap::new();
    let mut pieces = Vec::new();
    let mut unplaced = 0;
    for r in rows {
        let (Some(key), Some(name)) = (r.composer_key.clone(), r.composer_name.clone()) else {
            unplaced += 1;
            continue;
        };
        let born = r.born.as_deref().and_then(classify::year_of);
        let Some((year, dated)) = classify::placed_year(r.composed_year.map(|y| y as i32), born)
        else {
            unplaced += 1;
            continue;
        };
        composers.entry(key.clone()).or_insert_with(|| TimelineComposer {
            key: key.clone(),
            name,
            born,
            died: None,
        });
        pieces.push(TimelinePiece {
            key: r.piece_key,
            title: r.piece_title,
            composer: key,
            year,
            dated,
            plays: r.plays,
            seconds: r.seconds,
        });
    }
    let mut composers: Vec<TimelineComposer> = composers.into_values().collect();
    composers.sort_by(|a, b| {
        a.born.unwrap_or(9999).cmp(&b.born.unwrap_or(9999)).then(a.name.cmp(&b.name))
    });
    Json(Timeline { composers, pieces, unplaced }).into_response()
}

// ── Anniversaries ──

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AnniversaryQuery {
    /// The year to mark; this year, in your timezone, when absent.
    pub year: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct Anniversaries {
    pub year: i32,
    /// Round anniversaries this year of artists, composers and albums in your
    /// history, most played first.
    pub items: Vec<classify::anniversary::Anniversary>,
    /// Birthdays and deathdays falling on today's date.
    pub today: Vec<classify::anniversary::Anniversary>,
    /// Albums with a round anniversary this year that you have not played
    /// for half a year — for Rediscovery.
    pub revisit: Vec<classify::anniversary::Anniversary>,
}

/// Anniversaries among what you listen to.
///
/// Scope: `read` *(since v0.132)*. Births and deaths of artists and composers,
/// bands forming and splitting, and albums' releases — round numbers only (10,
/// 20, 25, 30 … 100, 75, every 50), from MusicBrainz's dates. Only artists
/// and albums in your own history.
#[utoipa::path(
    get,
    path = "/api/v1/insights/anniversaries",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(AnniversaryQuery),
    responses(
        (status = 200, description = "OK.", body = Anniversaries),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn anniversaries(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<AnniversaryQuery>,
) -> impl IntoResponse {
    let tz = super::scrobbles::user_tz_for(&state, user.user_id).await;
    let day = today(tz);
    let year = q.year.unwrap_or(day.year());
    let artists = match state.db.dated_artists(user.user_id, 5).await {
        Ok(a) => a,
        Err(e) => return err(500, e.to_string()),
    };
    let albums = state
        .db
        .dated_albums(user.user_id, 5)
        .await
        .or_warn("anniversaries: albums")
        .unwrap_or_default();
    let mut items = Vec::new();
    let mut on_day = Vec::new();
    for a in &artists {
        for ann in classify::anniversary::of_artist(
            &a.name,
            a.kind.as_deref(),
            a.begin.as_deref(),
            a.end_date.as_deref(),
            a.ended != 0,
            a.plays,
            year,
            day,
        ) {
            if ann.today {
                if a.plays >= 20 {
                    on_day.push(ann.clone());
                }
                if !classify::anniversary::round(ann.years) {
                    continue;
                }
            }
            items.push(ann);
        }
    }
    let half_year_ago = chrono::Utc::now().timestamp() - 182 * 86400;
    let mut revisit = Vec::new();
    for (album, artist, released, plays, last) in albums {
        if let Some(ann) =
            classify::anniversary::of_album(&album, &artist, released as i32, plays, year)
        {
            if plays >= 10 && last < half_year_ago {
                revisit.push(ann.clone());
            }
            items.push(ann);
        }
    }
    items.sort_by(|a, b| b.plays.cmp(&a.plays));
    items.truncate(40);
    on_day.sort_by(|a, b| b.plays.cmp(&a.plays));
    revisit.sort_by(|a, b| b.plays.cmp(&a.plays));
    revisit.truncate(12);
    Json(Anniversaries { year, items, today: on_day, revisit }).into_response()
}

// ── Scores & Ambient ──

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ScoresQuery {
    /// `all` (default), `score` or `ambient`.
    pub section: Option<String>,
    /// `tracks` (default), `albums` or `artists`.
    pub kind: Option<String>,
    pub range: Option<String>,
    /// `plays` (default) or `hours`.
    pub metric: Option<String>,
    #[param(minimum = 1, maximum = 500, default = 100)]
    pub limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct ScoresChart {
    pub section: String,
    pub kind: String,
    pub rows: Vec<crate::db::ChartRow>,
    /// Albums that look like score — a soundtrack credited to its composer
    /// alone — but are not moved without your word. Confirm one with an
    /// `album` override setting `score`; dismiss it by clearing
    /// `score_suggested`.
    pub suggestions: Vec<InsightRow>,
    pub pending: i64,
}

/// The Scores & Ambient tab.
///
/// Scope: `read` *(since v0.132)*. Score is instrumental music on a
/// soundtrack — a song on a soundtrack stays a song, and in Top Charts. Ambient
/// is an album whose top genre on MusicBrainz is ambient.
#[utoipa::path(
    get,
    path = "/api/v1/insights/scores",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(ScoresQuery),
    responses(
        (status = 200, description = "OK.", body = ScoresChart),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn scores(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<ScoresQuery>,
) -> impl IntoResponse {
    let section = q.section.as_deref().unwrap_or("all");
    let flag = match section {
        "all" => classify::SCORE | classify::AMBIENT,
        "score" => classify::SCORE,
        "ambient" => classify::AMBIENT,
        _ => return err(400, "section must be all, score or ambient"),
    };
    let kind = q.kind.as_deref().unwrap_or("tracks");
    if !matches!(kind, "tracks" | "albums" | "artists") {
        return err(400, "kind must be tracks, albums or artists");
    }
    let after = super::scrobbles::range_after(q.range.as_deref());
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let rows = match state
        .db
        .chart_filtered(
            user.user_id,
            kind,
            after,
            None,
            limit,
            metric_hours(q.metric.as_deref()),
            0,
            flag,
        )
        .await
    {
        Ok(r) => r,
        Err(e) => return err(500, e.to_string()),
    };
    let suggestions = state
        .db
        .score_suggestions(user.user_id, 20)
        .await
        .or_warn("scores: suggestions")
        .unwrap_or_default();
    let pending = state
        .db
        .facets_pending(user.user_id, classify::VERSION)
        .await
        .or_warn("facets pending")
        .unwrap_or(0);
    (
        StatusCode::OK,
        Json(ScoresChart {
            section: section.to_string(),
            kind: kind.to_string(),
            rows,
            suggestions,
            pending,
        }),
    )
        .into_response()
}

// ── Lenses ──

struct Lens {
    id: &'static str,
    label: &'static str,
    season: Season,
    flag: i64,
}

const LENSES: &[Lens] = &[
    Lens {
        id: "christmas",
        label: "Christmas",
        season: Season::Christmas,
        flag: classify::CHRISTMAS,
    },
    Lens {
        id: "halloween",
        label: "Halloween",
        season: Season::Halloween,
        flag: classify::HALLOWEEN,
    },
    Lens { id: "easter", label: "Easter", season: Season::Easter, flag: classify::EASTER },
];

fn lens(id: &str) -> Option<&'static Lens> {
    LENSES.iter().find(|l| l.id == id)
}

fn today(tz: chrono_tz::Tz) -> NaiveDate {
    chrono::Utc::now().with_timezone(&tz).date_naive()
}

/// Local midnight at the start of `day`, as a Unix time.
fn midnight(tz: chrono_tz::Tz, day: NaiveDate) -> i64 {
    tz.from_local_datetime(&day.and_hms_opt(0, 0, 0).expect("midnight"))
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or_else(|| day.and_hms_opt(0, 0, 0).expect("midnight").and_utc().timestamp())
}

fn local_day(tz: chrono_tz::Tz, ts: i64) -> NaiveDate {
    tz.timestamp_opt(ts, 0).single().map(|t| t.date_naive()).unwrap_or_default()
}

fn fmt_day(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

#[derive(Serialize, ToSchema)]
pub struct LensSummary {
    /// `christmas`, `halloween` or `easter`.
    pub id: String,
    pub label: String,
    /// Whether today, in your timezone, is in its season — when a tab shows.
    pub in_season: bool,
    /// The season running, or the last one: the year it began.
    pub season: i32,
    pub starts: String,
    pub ends: String,
    /// All-time listens carrying it.
    pub listens: i64,
}

#[derive(Serialize, ToSchema)]
pub struct LensList {
    pub lenses: Vec<LensSummary>,
    /// All-time listens in the two split tabs, for their cards on B-Sides.
    pub classical: i64,
    pub score: i64,
    pub ambient: i64,
    pub pending: i64,
}

/// The seasonal lenses, and which are in season today.
///
/// Scope: `read` *(since v0.132)*. Christmas runs 1 September – 6 January,
/// Halloween 1 October – 2 November, Easter Palm Sunday – Easter Monday
/// (Western date), all by your own calendar. A lens takes nothing out of Top
/// Charts; it adds a view.
#[utoipa::path(
    get,
    path = "/api/v1/insights/lenses",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    responses(
        (status = 200, description = "OK.", body = LensList),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn lenses(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
) -> impl IntoResponse {
    let tz = super::scrobbles::user_tz_for(&state, user.user_id).await;
    let day = today(tz);
    let counts = match state.db.facet_counts(user.user_id).await {
        Ok(c) => c,
        Err(e) => return err(500, e.to_string()),
    };
    let lenses = LENSES
        .iter()
        .zip([counts[3], counts[4], counts[5]])
        .map(|(l, listens)| {
            let year = season::current_or_last(l.season, day);
            let (s, e) = season::bounds(l.season, year);
            LensSummary {
                id: l.id.to_string(),
                label: l.label.to_string(),
                in_season: season::season_of(l.season, day).is_some(),
                season: year,
                starts: fmt_day(s),
                ends: fmt_day(e),
                listens,
            }
        })
        .collect();
    let pending = state
        .db
        .facets_pending(user.user_id, classify::VERSION)
        .await
        .or_warn("facets pending")
        .unwrap_or(0);
    Json(LensList { lenses, classical: counts[0], score: counts[1], ambient: counts[2], pending })
        .into_response()
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LensQuery {
    /// The season, by the year it began. The running or most recent one when
    /// absent.
    pub season: Option<i32>,
    /// `season` (default), `year` or `all`.
    pub range: Option<String>,
    /// `songs` (default; covers of one song are one row), `artists` or `albums`.
    pub rank: Option<String>,
    /// `plays` (default) or `hours`.
    pub metric: Option<String>,
    #[param(minimum = 1, maximum = 200, default = 50)]
    pub limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct CurveDay {
    pub date: String,
    /// Everything played that day.
    pub plays: i64,
    /// The season's music among it.
    pub season_plays: i64,
    pub share: f64,
}

#[derive(Serialize, ToSchema)]
pub struct LensDetail {
    pub id: String,
    pub label: String,
    pub season: i32,
    pub starts: String,
    pub ends: String,
    pub in_season: bool,
    /// Every season you have listens in, newest first.
    pub seasons: Vec<i32>,
    pub range: String,
    pub rank: String,
    pub rows: Vec<InsightRow>,
    /// The season's own listens and seconds, and their share of everything
    /// you played in it — the same population, both skip-free.
    pub plays: i64,
    pub seconds: i64,
    pub share_pct: Option<i64>,
    /// The day your season opened: the first of three days running when it
    /// was at least three times its level in the months before.
    pub opened: Option<String>,
    /// One point per day from just before the season to its end (or today).
    pub curve: Vec<CurveDay>,
    /// Listens carrying it that fell outside every season — "Christmas in July".
    pub out_of_season: i64,
    pub pending: i64,
}

struct SeasonFacts {
    year: i32,
    start: NaiveDate,
    end: NaiveDate,
    in_season: bool,
    seasons: Vec<i32>,
    plays: i64,
    seconds: i64,
    share_pct: Option<i64>,
    opened: Option<NaiveDate>,
    curve: Vec<CurveDay>,
    out_of_season: i64,
    by_month: Vec<(String, i64)>,
}

async fn season_facts(
    state: &AppState,
    user_id: i64,
    l: &Lens,
    wanted: Option<i32>,
) -> Result<SeasonFacts, String> {
    let tz = super::scrobbles::user_tz_for(state, user_id).await;
    let day = today(tz);
    let year = wanted.unwrap_or_else(|| season::current_or_last(l.season, day));
    let (start, end) = season::bounds(l.season, year);
    let (from, to) = season::curve_bounds(l.season, year);
    let to = to.min(day);
    let cap = state.analytics_row_cap;

    let rows = state
        .db
        .facet_timeline(
            user_id,
            Some(midnight(tz, from)),
            Some(midnight(tz, to + Duration::days(1))),
            cap,
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut days: BTreeMap<NaiveDate, (i64, i64)> = BTreeMap::new();
    let mut d = from;
    while d <= to {
        days.insert(d, (0, 0));
        d += Duration::days(1);
    }
    let (mut plays, mut seconds, mut total) = (0i64, 0i64, 0i64);
    let mut by_month: BTreeMap<(i32, u32), i64> = BTreeMap::new();
    for (ts, secs, facets) in &rows {
        let day = local_day(tz, *ts);
        let hit = facets & l.flag != 0;
        if let Some(e) = days.get_mut(&day) {
            e.0 += 1;
            if hit {
                e.1 += 1;
            }
        }
        if day >= start && day <= end {
            total += 1;
            if hit {
                plays += 1;
                seconds += secs;
                *by_month.entry((day.year(), day.month())).or_default() += 1;
            }
        }
    }

    // The level before: the four months leading up to the run-up.
    let before = state
        .db
        .facet_timeline(
            user_id,
            Some(midnight(tz, from - Duration::days(120))),
            Some(midnight(tz, from)),
            cap,
        )
        .await
        .map_err(|e| e.to_string())?;
    let baseline = if before.is_empty() {
        0.0
    } else {
        before.iter().filter(|r| r.2 & l.flag != 0).count() as f64 / before.len() as f64
    };
    let series: Vec<(NaiveDate, i64, i64)> = days.iter().map(|(d, (p, f))| (*d, *p, *f)).collect();
    let opened = season::opened(&series, baseline, start);

    let stamps = state.db.facet_timestamps(user_id, l.flag).await.map_err(|e| e.to_string())?;
    let mut seasons: BTreeSet<i32> = BTreeSet::new();
    let mut out_of_season = 0;
    for ts in stamps {
        match season::season_of(l.season, local_day(tz, ts)) {
            Some(y) => {
                seasons.insert(y);
            }
            None => out_of_season += 1,
        }
    }
    if season::season_of(l.season, day).is_some() {
        seasons.insert(season::current_or_last(l.season, day));
    }

    Ok(SeasonFacts {
        year,
        start,
        end,
        in_season: season::season_of(l.season, day) == Some(year),
        seasons: seasons.into_iter().rev().collect(),
        plays,
        seconds,
        share_pct: (total > 0).then(|| ((plays as f64 / total as f64) * 100.0).round() as i64),
        opened,
        curve: series
            .into_iter()
            .map(|(d, p, f)| CurveDay {
                date: fmt_day(d),
                plays: p,
                season_plays: f,
                share: if p > 0 { f as f64 / p as f64 } else { 0.0 },
            })
            .collect(),
        out_of_season,
        by_month: by_month
            .into_iter()
            .map(|((y, m), n)| {
                (
                    NaiveDate::from_ymd_opt(y, m, 1)
                        .map(|d| d.format("%b").to_string())
                        .unwrap_or_default(),
                    n,
                )
            })
            .collect(),
    })
}

fn chart_to_insight(r: crate::db::ChartRow) -> InsightRow {
    InsightRow {
        key: r.name.trim().to_lowercase(),
        name: r.name,
        sub: r.sub,
        album: r.album,
        album_artist: r.album_artist,
        plays: r.plays,
        seconds: r.seconds,
        artwork_url: r.artwork_url,
        caa_id: r.caa_id,
        caa_release_mbid: r.caa_release_mbid,
        ..Default::default()
    }
}

async fn lens_rows(
    state: &AppState,
    user_id: i64,
    l: &Lens,
    window: (Option<i64>, Option<i64>),
    rank: &str,
    by_hours: bool,
    limit: i64,
) -> Result<Vec<InsightRow>, sqlx::Error> {
    match rank {
        "artists" | "albums" => Ok(state
            .db
            .chart_filtered(user_id, rank, window.0, window.1, limit, by_hours, 0, l.flag)
            .await?
            .into_iter()
            .map(chart_to_insight)
            .collect()),
        _ => {
            let f = InsightFilter { after: window.0, before: window.1, ..Default::default() };
            state.db.lens_songs(user_id, l.flag, &f, by_hours, limit).await
        }
    }
}

/// One seasonal lens: its charts, its curve, and when your season opened.
///
/// Scope: `read` *(since v0.132)*. `range=season` (the default) charts the
/// chosen season; `year` and `all` chart wider windows by the same flag.
#[utoipa::path(
    get,
    path = "/api/v1/insights/lenses/{id}",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = String, Path, description = "`christmas`, `halloween` or `easter`."), LensQuery),
    responses(
        (status = 200, description = "OK.", body = LensDetail),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
pub async fn lens_detail(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<String>,
    Query(q): Query<LensQuery>,
) -> impl IntoResponse {
    let Some(l) = lens(&id) else { return err(404, "No such lens") };
    let range = q.range.as_deref().unwrap_or("season");
    let rank = q.rank.as_deref().unwrap_or("songs");
    if !matches!(range, "season" | "year" | "all") {
        return err(400, "range must be season, year or all");
    }
    if !matches!(rank, "songs" | "artists" | "albums") {
        return err(400, "rank must be songs, artists or albums");
    }
    let s = match season_facts(&state, user.user_id, l, q.season).await {
        Ok(s) => s,
        Err(e) => return err(500, e),
    };
    let tz = super::scrobbles::user_tz_for(&state, user.user_id).await;
    let window = match range {
        "season" => (Some(midnight(tz, s.start)), Some(midnight(tz, s.end + Duration::days(1)))),
        "year" => (super::scrobbles::range_after(Some("year")), None),
        _ => (None, None),
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let rows = match lens_rows(
        &state,
        user.user_id,
        l,
        window,
        rank,
        metric_hours(q.metric.as_deref()),
        limit,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return err(500, e.to_string()),
    };
    let pending = state
        .db
        .facets_pending(user.user_id, classify::VERSION)
        .await
        .or_warn("facets pending")
        .unwrap_or(0);
    Json(LensDetail {
        id: l.id.to_string(),
        label: l.label.to_string(),
        season: s.year,
        starts: fmt_day(s.start),
        ends: fmt_day(s.end),
        in_season: s.in_season,
        seasons: s.seasons,
        range: range.to_string(),
        rank: rank.to_string(),
        rows,
        plays: s.plays,
        seconds: s.seconds,
        share_pct: s.share_pct,
        opened: s.opened.map(fmt_day),
        curve: s.curve,
        out_of_season: s.out_of_season,
        pending,
    })
    .into_response()
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct VersionsQuery {
    /// A `key` from a `songs` row.
    pub piece: String,
}

#[derive(Serialize, ToSchema)]
pub struct Versions {
    pub rows: Vec<InsightRow>,
}

/// Every artist's version of one song you heard — the covers behind a row.
///
/// Scope: `read` *(since v0.132)*.
#[utoipa::path(
    get,
    path = "/api/v1/insights/versions",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(VersionsQuery),
    responses(
        (status = 200, description = "OK.", body = Versions),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn versions(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<VersionsQuery>,
) -> impl IntoResponse {
    match state.db.piece_versions(user.user_id, &q.piece, 50).await {
        Ok(rows) => Json(Versions { rows }).into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

// ── Sharing a season ──

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ShareQuery {
    pub season: Option<i32>,
    /// `story` (default), `square` or `wide`. The image only.
    pub size: Option<String>,
    /// `dark` (default) or `light`.
    pub theme: Option<String>,
}

fn span_words(start: NaiveDate, end: NaiveDate) -> String {
    format!("{} – {}", start.format("%-d %B"), end.format("%-d %B"))
}

async fn top_songs(
    state: &AppState,
    user_id: i64,
    l: &Lens,
    tz: chrono_tz::Tz,
    s: &SeasonFacts,
    n: i64,
) -> Vec<crate::db::ChartRow> {
    state
        .db
        .chart_filtered(
            user_id,
            "tracks",
            Some(midnight(tz, s.start)),
            Some(midnight(tz, s.end + Duration::days(1))),
            n,
            false,
            0,
            l.flag,
        )
        .await
        .or_warn("lens share: top songs")
        .unwrap_or_default()
}

async fn top_artist(
    state: &AppState,
    user_id: i64,
    l: &Lens,
    tz: chrono_tz::Tz,
    s: &SeasonFacts,
) -> Option<crate::db::ChartRow> {
    state
        .db
        .chart_filtered(
            user_id,
            "artists",
            Some(midnight(tz, s.start)),
            Some(midnight(tz, s.end + Duration::days(1))),
            1,
            false,
            0,
            l.flag,
        )
        .await
        .or_warn("lens share: top artist")
        .unwrap_or_default()
        .into_iter()
        .next()
}

/// A season as a share card (PNG).
///
/// Scope: `read` *(since v0.132)*. The season's plays and share, the day it
/// opened, its daily curve, the artist you played most and what was on repeat.
#[utoipa::path(
    get,
    path = "/api/v1/insights/lenses/{id}/image",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = String, Path, description = "`christmas`, `halloween` or `easter`."), ShareQuery),
    responses(
        (status = 200, description = "A PNG.", content_type = "image/png"),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
pub async fn lens_image(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<String>,
    Query(q): Query<ShareQuery>,
) -> impl IntoResponse {
    let Some(l) = lens(&id) else { return err(404, "No such lens") };
    let Some(size) = Size::parse(q.size.as_deref()) else {
        return err(400, "size must be story, square or wide");
    };
    let theme = Theme::parse(q.theme.as_deref());
    let s = match season_facts(&state, user.user_id, l, q.season).await {
        Ok(s) => s,
        Err(e) => return err(500, e),
    };
    if s.plays == 0 {
        return err(404, "Nothing from that season to share yet");
    }
    let tz = super::scrobbles::user_tz_for(&state, user.user_id).await;
    let songs = top_songs(&state, user.user_id, l, tz, &s, 6).await;
    let artist = top_artist(&state, user.user_id, l, tz, &s).await;
    let card = SeasonCard {
        eyebrow: l.label.to_uppercase(),
        title: format!("{} {}", l.label, s.year),
        span: span_words(s.start, s.end),
        plays: s.plays,
        seconds: s.seconds,
        share_pct: s.share_pct,
        opened: s.opened.map(|d| format!("Your season opened on {}", d.format("%-d %B"))),
        top_artist: artist.map(|a| (a.name, a.plays)),
        songs: songs.into_iter().map(|r| (r.name, r.sub.unwrap_or_default(), r.plays)).collect(),
        curve: s.curve.iter().map(|c| c.share).collect(),
    };
    let rendered = match tokio::task::spawn_blocking(move || {
        crate::report_image::render_season(&card, size, theme)
    })
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return err(500, e.to_string()),
        Err(e) => return err(500, e.to_string()),
    };
    let file =
        super::report_image::slug(&format!("tapedeck-{}-{}-{}", l.id, s.year, size.as_str()));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png".to_string()),
            (header::CACHE_CONTROL, "private, max-age=60".to_string()),
            (header::CONTENT_DISPOSITION, format!("inline; filename=\"{file}.png\"")),
        ],
        rendered.png,
    )
        .into_response()
}

async fn season_mixdown(
    state: &AppState,
    user_id: i64,
    l: &Lens,
    q: &ShareQuery,
) -> Result<(Mixdown, String), axum::response::Response> {
    let s = season_facts(state, user_id, l, q.season).await.map_err(|e| err(500, e))?;
    if s.plays == 0 {
        return Err(err(404, "Nothing from that season to play yet"));
    }
    let tz = super::scrobbles::user_tz_for(state, user_id).await;
    let songs = top_songs(state, user_id, l, tz, &s, 5).await;
    let artist = top_artist(state, user_id, l, tz, &s).await;
    let mut art = Art::default();
    if let Some(cover) = songs.first().and_then(|r| r.artwork_url.clone()).filter(|u| href_ok(u)) {
        art.track = Some(cover);
    }
    let card = Card {
        label: format!("{} {}", l.label, s.year),
        plays: s.plays,
        hours: s.seconds as f64 / 3600.0,
        top_artist: artist.map(|a| Named { name: a.name, plays: a.plays }),
        top_track: songs.first().map(|r| Titled {
            title: r.name.clone(),
            artist: r.sub.clone().unwrap_or_default(),
            plays: r.plays,
        }),
        on_repeat: songs
            .iter()
            .map(|r| Titled {
                title: r.name.clone(),
                artist: r.sub.clone().unwrap_or_default(),
                plays: r.plays,
            })
            .collect(),
        sub: s.by_month.iter().map(|(_, n)| *n).collect(),
        sub_labels: s.by_month.iter().map(|(m, _)| m.clone()).collect(),
        ..Default::default()
    };
    let theme = Theme::parse(q.theme.as_deref());
    let mixdown = tokio::task::spawn_blocking(move || Mixdown::build_season(&card, theme, &art))
        .await
        .map_err(|_| err(500, "Could not build the mixdown"))?;
    Ok((mixdown, super::report_image::slug(&format!("tapedeck-{}-{}", l.id, s.year))))
}

/// A season as a Mixdown — the year's scenes, told for the season.
///
/// Scope: `read` *(since v0.132)*. Played exactly as
/// `GET /api/v1/reports/mixdown` describes.
#[utoipa::path(
    get,
    path = "/api/v1/insights/lenses/{id}/mixdown",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = String, Path, description = "`christmas`, `halloween` or `easter`."), ShareQuery),
    responses(
        (status = 200, description = "The mixdown.", body = MixdownResponse),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
pub async fn lens_mixdown(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Path(id): Path<String>,
    Query(q): Query<ShareQuery>,
) -> impl IntoResponse {
    let Some(l) = lens(&id) else { return err(404, "No such lens") };
    let (mixdown, _) = match season_mixdown(&state, user.user_id, l, &q).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let video_export = tokio::task::spawn_blocking(report_video::available).await.unwrap_or(false);
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "private, max-age=60")],
        Json(MixdownResponse { mixdown, video_export }),
    )
        .into_response()
}

/// Make a video of a season's Mixdown.
///
/// Scope: `read` *(since v0.132)*. The same job as
/// `POST /api/v1/reports/mixdown/video`; poll and download it there.
#[utoipa::path(
    post,
    path = "/api/v1/insights/lenses/{id}/mixdown/video",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(("id" = String, Path, description = "`christmas`, `halloween` or `easter`."), ShareQuery),
    responses(
        (status = 202, description = "Started.", body = MixdownVideo),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
        (status = 409, description = "You already have a video being made.", body = ErrorResponse),
        (status = 501, description = "This server has no video encoder.", body = ErrorResponse),
    )
)]
pub async fn lens_mixdown_video(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ShareQuery>,
) -> impl IntoResponse {
    let Some(l) = lens(&id) else { return err(404, "No such lens") };
    if !tokio::task::spawn_blocking(report_video::available).await.unwrap_or(false) {
        return err(501, "This server cannot make videos: ffmpeg is not installed");
    }
    if state.video_jobs.active(user.user_id).is_some() {
        return err(409, "You already have a video being made");
    }
    let (mixdown, stem) = match season_mixdown(&state, user.user_id, l, &q).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    let images = fetch_art(&state, &user, &headers, &mixdown).await;
    let job = state.video_jobs.start(user.user_id, stem, mixdown, images);
    (StatusCode::ACCEPTED, Json(MixdownVideo::from(&job))).into_response()
}

#[derive(Serialize, ToSchema)]
pub struct PlaylistMade {
    pub id: i64,
    /// False when this season's playlist already existed and was returned.
    pub created: bool,
    pub tracks: usize,
}

/// Save a season's most-played songs as a playlist.
///
/// Scope: `write` *(since v0.132)*. Your own history only — up to fifty songs
/// you played that season, most played first. Saving the same season again
/// returns the playlist already made. Push it to a media server from
/// Playlists like any other.
#[utoipa::path(
    post,
    path = "/api/v1/insights/lenses/{id}/playlist",
    tag = "Statistics",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    params(("id" = String, Path, description = "`christmas`, `halloween` or `easter`."), ShareQuery),
    responses(
        (status = 200, description = "Saved.", body = PlaylistMade),
        (status = 401, response = Unauthorized),
        (status = 404, response = NotFound),
    )
)]
pub async fn lens_playlist(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Path(id): Path<String>,
    Query(q): Query<ShareQuery>,
) -> impl IntoResponse {
    let Some(l) = lens(&id) else { return err(404, "No such lens") };
    let s = match season_facts(&state, user.user_id, l, q.season).await {
        Ok(s) => s,
        Err(e) => return err(500, e),
    };
    let tz = super::scrobbles::user_tz_for(&state, user.user_id).await;
    let songs = top_songs(&state, user.user_id, l, tz, &s, 50).await;
    if songs.is_empty() {
        return err(404, "Nothing from that season to put in a playlist yet");
    }
    let tracks: Vec<SavedPlaylistTrack> = songs
        .into_iter()
        .map(|r| SavedPlaylistTrack {
            title: r.name,
            artist: r.sub.unwrap_or_default(),
            album: r.album,
            note: None,
        })
        .collect();
    let name = format!("{} {} — most played", l.label, s.year);
    let key = format!("lens:{}:{}", l.id, s.year);
    match state
        .db
        .save_playlist(
            user.user_id,
            &name,
            Some("Your most-played songs of the season, from your own history."),
            "user",
            None,
            Some(&key),
            &tracks,
        )
        .await
    {
        // `save_playlist` says whether it *replaced* one with this key.
        Ok((id, replaced)) => {
            Json(PlaylistMade { id, created: !replaced, tracks: tracks.len() }).into_response()
        }
        Err(e) => err(500, e.to_string()),
    }
}

// ── Composition ──

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CompositionQuery {
    /// `year` (default) or `all`.
    pub range: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct CompositionMonth {
    /// `YYYY-MM`, in your timezone.
    pub month: String,
    /// Every second played. The three bands below add up to it.
    pub seconds: i64,
    pub classical: i64,
    /// Score and ambient, minus anything already counted as classical.
    pub score: i64,
    pub other: i64,
    /// Overlays: also inside the bands above.
    pub christmas: i64,
    pub halloween: i64,
    pub easter: i64,
}

#[derive(Serialize, ToSchema)]
pub struct Composition {
    pub months: Vec<CompositionMonth>,
    /// True when the window holds more listens than this instance reads for
    /// analytics, and the oldest months are missing.
    pub capped: bool,
    pub pending: i64,
}

/// What your listening is made of, month by month.
///
/// Scope: `read` *(since v0.132)*. Seconds of classical, score & ambient and
/// everything else — each listen counted once, classical first — with the
/// seasonal lenses as overlays inside them. Statistics keeps every listen in
/// its own figures; this is the one chart that says what share was which.
#[utoipa::path(
    get,
    path = "/api/v1/insights/composition",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(CompositionQuery),
    responses(
        (status = 200, description = "OK.", body = Composition),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn composition(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<CompositionQuery>,
) -> impl IntoResponse {
    let after = match q.range.as_deref().unwrap_or("year") {
        "all" => None,
        _ => super::scrobbles::range_after(Some("year")),
    };
    let cap = state.analytics_row_cap;
    let rows = match state.db.facet_timeline(user.user_id, after, None, cap).await {
        Ok(r) => r,
        Err(e) => return err(500, e.to_string()),
    };
    let capped = rows.len() as i64 >= cap;
    let tz = super::scrobbles::user_tz_for(&state, user.user_id).await;
    let mut months: BTreeMap<String, CompositionMonth> = BTreeMap::new();
    for (ts, secs, f) in rows {
        let month = local_day(tz, ts).format("%Y-%m").to_string();
        let m = months.entry(month.clone()).or_insert_with(|| CompositionMonth {
            month,
            seconds: 0,
            classical: 0,
            score: 0,
            other: 0,
            christmas: 0,
            halloween: 0,
            easter: 0,
        });
        m.seconds += secs;
        if f & classify::CLASSICAL != 0 {
            m.classical += secs;
        } else if f & (classify::SCORE | classify::AMBIENT) != 0 {
            m.score += secs;
        } else {
            m.other += secs;
        }
        if f & classify::CHRISTMAS != 0 {
            m.christmas += secs;
        }
        if f & classify::HALLOWEEN != 0 {
            m.halloween += secs;
        }
        if f & classify::EASTER != 0 {
            m.easter += secs;
        }
    }
    let pending = state
        .db
        .facets_pending(user.user_id, classify::VERSION)
        .await
        .or_warn("facets pending")
        .unwrap_or(0);
    Json(Composition { months: months.into_values().collect(), capped, pending }).into_response()
}

// ── Overrides ──

const FACET_NAMES: &[(&str, i64)] = &[
    ("classical", classify::CLASSICAL),
    ("score", classify::SCORE),
    ("ambient", classify::AMBIENT),
    ("christmas", classify::CHRISTMAS),
    ("halloween", classify::HALLOWEEN),
    ("easter", classify::EASTER),
    ("score_suggested", classify::SCORE_SUGGESTED),
];

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OverrideTarget {
    /// `work`, `album` or `artist`.
    pub target_kind: String,
    pub target_key: String,
}

#[derive(Serialize, ToSchema)]
pub struct OverrideState {
    pub facet: String,
    pub value: bool,
}

#[derive(Serialize, ToSchema)]
pub struct OverrideStates {
    /// Your corrections on that target. Empty means Tapedeck decides.
    pub overrides: Vec<OverrideState>,
}

/// Your corrections on one piece, album or artist.
///
/// Scope: `read` *(since v0.132)*.
#[utoipa::path(
    get,
    path = "/api/v1/insights/overrides",
    tag = "Statistics",
    security(("apiToken" = ["read"]), ("sessionCookie" = [])),
    params(OverrideTarget),
    responses(
        (status = 200, description = "OK.", body = OverrideStates),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn get_overrides(
    State(state): State<Arc<AppState>>,
    ReadAuth { user }: ReadAuth,
    Query(q): Query<OverrideTarget>,
) -> impl IntoResponse {
    let key = if q.target_kind == "artist" {
        crate::db::Database::artist_key(&q.target_key)
    } else {
        q.target_key.clone()
    };
    match state.db.facet_overrides_on(user.user_id, &q.target_kind, &key).await {
        Ok(rows) => Json(OverrideStates {
            overrides: rows
                .into_iter()
                .filter_map(|(f, value)| {
                    FACET_NAMES
                        .iter()
                        .find(|(_, b)| *b == f)
                        .map(|(n, _)| OverrideState { facet: n.to_string(), value })
                })
                .collect(),
        })
        .into_response(),
        Err(e) => err(500, e.to_string()),
    }
}

fn double_option<'de, D: Deserializer<'de>>(de: D) -> Result<Option<Option<bool>>, D::Error> {
    Option::<bool>::deserialize(de).map(Some)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OverrideBody {
    /// `work` — a piece or song, every version of it (key: a row's `key`
    /// from a lens or Classical, which starts `w:` or `t:`); `album` (key: a
    /// Scores suggestion's `key`); or `artist` (key: the artist's name) —
    /// everything they are credited on. Broadest first, most specific wins:
    /// artist, then album, then piece.
    pub target_kind: String,
    pub target_key: String,
    /// `classical`, `score`, `ambient`, `christmas`, `halloween`, `easter`
    /// or `score_suggested`.
    pub facet: String,
    /// `true` sets it, `false` clears it, `null` removes your override and
    /// returns to what Tapedeck decided. **Required**: an absent `value` is a
    /// 400, so a client that forgot it cannot quietly undo a correction.
    #[serde(default, deserialize_with = "double_option")]
    #[schema(value_type = Option<bool>)]
    pub value: Option<Option<bool>>,
}

#[derive(Serialize, ToSchema)]
pub struct OverrideResult {
    /// Listens the change touched, already re-applied.
    pub listens: u64,
}

/// Correct what kind of listen something is.
///
/// Scope: `write` *(since v0.132)*. Yours only: overrides never change what
/// another member of this instance sees. Applied at once, from what Tapedeck
/// already knows — nothing is looked up.
#[utoipa::path(
    post,
    path = "/api/v1/insights/overrides",
    tag = "Statistics",
    security(("apiToken" = ["write"]), ("sessionCookie" = [])),
    request_body = OverrideBody,
    responses(
        (status = 200, description = "Applied.", body = OverrideResult),
        (status = 400, response = BadRequest),
        (status = 401, response = Unauthorized),
    )
)]
pub async fn set_override(
    State(state): State<Arc<AppState>>,
    WriteAuth { user }: WriteAuth,
    Json(body): Json<OverrideBody>,
) -> impl IntoResponse {
    let Some(value) = body.value else { return err(400, "value is required: true, false or null") };
    if !matches!(body.target_kind.as_str(), "work" | "album" | "artist") {
        return err(400, "target_kind must be work, album or artist");
    }
    if body.target_key.trim().is_empty() {
        return err(400, "target_key is required");
    }
    // An artist is keyed the way `scrobble_artists` keys them, whatever
    // spelling the page sent.
    let target_key = if body.target_kind == "artist" {
        crate::db::Database::artist_key(&body.target_key)
    } else {
        body.target_key.clone()
    };
    let Some(&(_, facet)) = FACET_NAMES.iter().find(|(n, _)| *n == body.facet) else {
        return err(400, "facet must be classical, score, ambient, christmas, halloween, easter or score_suggested");
    };
    let listens = match state
        .db
        .set_facet_override(user.user_id, &body.target_kind, &target_key, facet, value)
        .await
    {
        Ok(n) => n,
        Err(e) => return err(500, e.to_string()),
    };
    classify::backfill::refresh_user(&state.db, &state.mb_client, user.user_id).await;
    Json(OverrideResult { listens }).into_response()
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    classical,
    scores,
    lenses,
    lens_detail,
    versions,
    lens_image,
    lens_mixdown,
    lens_mixdown_video,
    lens_playlist,
    composition,
    classical_stats,
    classical_timeline,
    anniversaries,
    get_overrides,
    set_override
))]
pub struct Api;

pub fn create_insights_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/insights/classical", get(classical))
        .route("/api/v1/insights/classical/stats", get(classical_stats))
        .route("/api/v1/insights/classical/timeline", get(classical_timeline))
        .route("/api/v1/insights/anniversaries", get(anniversaries))
        .route("/api/v1/insights/scores", get(scores))
        .route("/api/v1/insights/lenses", get(lenses))
        .route("/api/v1/insights/lenses/:id", get(lens_detail))
        .route("/api/v1/insights/lenses/:id/image", get(lens_image))
        .route("/api/v1/insights/lenses/:id/mixdown", get(lens_mixdown))
        .route("/api/v1/insights/lenses/:id/mixdown/video", post(lens_mixdown_video))
        .route("/api/v1/insights/lenses/:id/playlist", post(lens_playlist))
        .route("/api/v1/insights/versions", get(versions))
        .route("/api/v1/insights/composition", get(composition))
        .route("/api/v1/insights/overrides", get(get_overrides).post(set_override))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hearing_needs_every_movement_in_one_sitting() {
        let w = |s: i64, leaf: &str| {
            (Some(s), "w:p".to_string(), Some(leaf.to_string()), Some(3), Some("bach".to_string()))
        };
        let rows = vec![
            w(1, "a"),
            w(1, "b"),
            w(1, "c"), // whole partita in sitting 1
            w(2, "a"),
            w(2, "b"), // two of three in sitting 2
            (Some(3), "w:single".to_string(), None, None, Some("bach".to_string())),
        ];
        let (by_piece, by_composer) = hearings(&rows);
        assert_eq!(by_piece.get("w:p"), Some(&1));
        assert_eq!(by_piece.get("w:single"), Some(&1));
        assert_eq!(by_composer.get("bach"), Some(&2));
    }
}
