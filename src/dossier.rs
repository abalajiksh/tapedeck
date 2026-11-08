//! One artist's place in a listening history — the artist page's "your
//! history with them" section. Ported from the standalone `swiftie_audit` tool,
//! which asked these questions of one artist; here they are asked of every one.
//!
//! Pure: it reads two flat lists the database hands over and makes no calls,
//! so every answer is testable without one. Days, hours and weeks are the
//! **user's local** ones (`tz`, `week_start`), never UTC.
//!
//! **Every share has its population beside it.** A count of this artist's
//! listens is always returned with the count of *all* listens over the same
//! window, both skip-free, because a numerator and denominator filtered
//! differently is the recurring bug in this codebase. The window is the whole
//! history unless the instance caps analytics reads, and `capped` says so.

use chrono::{Datelike, NaiveDate, TimeZone, Timelike};
use chrono_tz::Tz;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use utoipa::ToSchema;

/// The silence that ends a sitting: a run or a playthrough does not continue
/// across it. The same half hour sessions use.
const SITTING_GAP_SECS: i64 = 1800;

/// An album played through: at least this many consecutive tracks, in order.
const PLAYTHROUGH_MIN_TRACKS: usize = 4;

/// Below this, "when did they take off" has no curve to find a turn in.
const TOOK_OFF_MIN_PLAYS: i64 = 20;

/// Neither side of a step may be shorter than this, in days: a rate measured
/// over a weekend is a weekend, not a rate.
const STEP_MIN_DAYS: i64 = 14;

/// How much better two rates must explain the listening than one before a
/// step is called — a log-likelihood ratio. With a long history almost any
/// split beats one rate a little; this asks for a clear step.
const STEP_MIN_EVIDENCE: f64 = 10.0;

/// "What plays next" only counts the next listen when it came this soon:
/// the script's quarter hour, so it is the same sitting and not tomorrow.
const NEXT_GAP_SECS: i64 = 15 * 60;

/// Rivals drawn against this artist, and names listed under "what plays next".
const RIVALS: usize = 5;
const NEXT_ARTISTS: usize = 8;

/// One skip-free listen of the history, in time order.
pub struct Listen {
    pub ts: i64,
    /// Credits this artist — the same test the rest of the artist page uses.
    pub mine: bool,
    pub album: Option<String>,
    pub track_number: Option<i32>,
}

/// One credit of a skip-free listen: who it counts for in a chart.
pub struct Credit {
    pub ts: i64,
    pub artist_key: String,
    /// The name as credited, for showing. `artist_key` is what groups.
    pub name: String,
}

/// A day, as the local midnight it began at (Unix seconds).
#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct DayCount {
    pub day: i64,
    /// The same day as `YYYY-MM-DD` in your timezone. A browser elsewhere
    /// would put `day` on the wrong date; lay out by this.
    pub date: String,
    /// Their listens that day.
    pub plays: i64,
    /// Every listen that day — the population `plays` is a share of.
    pub all: i64,
}

#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct MonthShare {
    /// `YYYY-MM`, local.
    pub month: String,
    pub plays: i64,
    pub all: i64,
}

/// Their listens back to back, nobody else in between and no half-hour
/// silence.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct Run {
    pub length: usize,
    pub start: i64,
    pub end: i64,
}

/// One of their records played in order — track N, then N+1, … — in one
/// sitting.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct Playthrough {
    pub album: String,
    pub start: i64,
    /// Local `YYYY-MM-DD` of `start`.
    pub date: String,
    pub first_track: i32,
    pub last_track: i32,
}

/// A daily habit or a burst? The `musical_personality` loyalty score for this
/// one artist, with the three things it is made of.
#[derive(Debug, Serialize, ToSchema)]
pub struct Habit {
    /// Days they were played, over the days from their first listen to their
    /// last.
    pub return_rate: f64,
    /// How much of the whole window those first and last listens span.
    pub span_coverage: f64,
    /// The share of their listens that fell on their single biggest day.
    pub top_day_share: f64,
    /// 0–100; high is a habit, low a burst.
    pub score: f64,
}

/// A week, by the local date it began on (your first day of the week).
#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct WeekShare {
    /// `YYYY-MM-DD`, local: the week's first day.
    pub week: String,
    /// Their listens that week.
    pub plays: i64,
    /// Every listen that week — the population `plays` is part of.
    pub all: i64,
}

/// One of your other most-played artists, week by week, to set this one
/// against.
#[derive(Debug, Serialize, ToSchema)]
pub struct Rival {
    pub name: String,
    /// Credited listens over the whole window.
    pub plays: i64,
    /// Credited listens per week, aligned with `Dossier::weeks`.
    pub weekly: Vec<i64>,
}

/// Who you go on to after them.
#[derive(Debug, Serialize, ToSchema)]
pub struct Next {
    /// Times one of their listens was followed, within a quarter of an hour,
    /// by somebody else's — the population each `plays` below is part of.
    pub total: i64,
    /// Most often first. A listen crediting two artists counts for both, so
    /// these can add up to more than `total`.
    pub artists: Vec<NextArtist>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NextArtist {
    pub name: String,
    pub plays: i64,
}

/// The day your rate of listening to them stepped up most.
///
/// One change point, fitted to every day from their first listen to the end
/// of the history: the split where "one rate before, a higher one after"
/// explains the days best. Absent when there is no clear step up — a steady
/// favourite, or one that has faded, has nothing to mark.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct Step {
    /// `YYYY-MM-DD`, local: the first day at the new rate.
    pub date: String,
    /// Their listens a week before that day.
    pub before_per_week: f64,
    /// And from that day on.
    pub after_per_week: f64,
}

/// Is somebody your number one, and how often.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct AtTop {
    /// Periods in which they had the most listens (ties included).
    pub periods: i64,
    /// Periods with any listening at all.
    pub of: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Dossier {
    /// Their skip-free listens in the window.
    pub plays: i64,
    /// Everyone's — the population every share here is of.
    pub all_plays: i64,
    /// Local days they were played on.
    pub days: i64,
    /// Local days with any listening.
    pub all_days: i64,
    pub first: Option<i64>,
    pub last: Option<i64>,
    /// The local day (`YYYY-MM-DD`) your listening to them stepped up —
    /// `step.date`, kept as its own field for the clients that read it.
    /// Absent under twenty listens, or without a clear step up.
    pub took_off: Option<String>,
    /// The step itself, with the rates either side. Absent like `took_off`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<Step>,
    /// Absent under five listens, where there is no pattern to score.
    pub habit: Option<Habit>,
    /// Every day they were played, oldest first.
    pub daily: Vec<DayCount>,
    /// Every month from their first to the last of the history, oldest first,
    /// including months they were not played in.
    pub months: Vec<MonthShare>,
    /// Every week of the history, oldest first, including weeks they were
    /// not played in.
    pub weeks: Vec<WeekShare>,
    /// Your five most-played other artists across the same weeks.
    pub rivals: Vec<Rival>,
    /// Who you play next after them.
    pub next: Next,
    /// 7 × 24, rows Sunday first like the Statistics heatmap; the page
    /// rotates them by `week_start`.
    pub clock: Vec<Vec<i64>>,
    /// First day of the week, 0 = Sunday.
    pub week_start: u32,
    /// Their five biggest days.
    pub top_days: Vec<DayCount>,
    /// The three longest runs, longest first.
    pub runs: Vec<Run>,
    /// Up to ten, most tracks first.
    pub playthroughs: Vec<Playthrough>,
    pub months_at_top: AtTop,
    pub weeks_at_top: AtTop,
    /// The instance caps analytics reads and this history is longer than the
    /// cap: everything above describes only the most recent part of it.
    pub capped: bool,
}

fn local(ts: i64, tz: Tz) -> chrono::DateTime<Tz> {
    tz.timestamp_opt(ts, 0).single().unwrap_or_else(|| tz.timestamp_opt(0, 0).single().unwrap())
}

fn date(ts: i64, tz: Tz) -> NaiveDate {
    local(ts, tz).date_naive()
}

fn midnight(d: NaiveDate, tz: Tz) -> i64 {
    crate::analytics::local_day_start(
        tz.from_local_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
            .earliest()
            .map(|t| t.timestamp())
            .unwrap_or(0),
        tz,
    )
}

fn month_key(d: NaiveDate) -> String {
    format!("{:04}-{:02}", d.year(), d.month())
}

fn week_key(d: NaiveDate, week_start: u32) -> NaiveDate {
    let back = (d.weekday().num_days_from_sunday() + 7 - week_start) % 7;
    d - chrono::Duration::days(back as i64)
}

fn same_album(a: &str, b: &str) -> bool {
    crate::db::normalise_artist(a) == crate::db::normalise_artist(b)
}

/// Everything on the page, from the history in time order.
pub fn compute(
    listens: &[Listen],
    credits: &[Credit],
    artist_key: &str,
    tz: Tz,
    week_start: u32,
    capped: bool,
) -> Dossier {
    // ── Per day, and the clock ──
    let mut all_by_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    let mut mine_by_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    let mut clock = vec![vec![0i64; 24]; 7];
    let (mut plays, mut first, mut last) = (0i64, None, None);
    for l in listens {
        let dt = local(l.ts, tz);
        let d = dt.date_naive();
        *all_by_day.entry(d).or_default() += 1;
        if l.mine {
            plays += 1;
            *mine_by_day.entry(d).or_default() += 1;
            clock[dt.weekday().num_days_from_sunday() as usize][dt.hour() as usize] += 1;
            first.get_or_insert(l.ts);
            last = Some(l.ts);
        }
    }
    let day_count = |d: &NaiveDate, n: i64| DayCount {
        day: midnight(*d, tz),
        date: d.to_string(),
        plays: n,
        all: all_by_day[d],
    };
    let daily: Vec<DayCount> = mine_by_day.iter().map(|(d, n)| day_count(d, *n)).collect();

    let mut top_days: Vec<DayCount> = mine_by_day.iter().map(|(d, n)| day_count(d, *n)).collect();
    top_days.sort_by(|a, b| b.plays.cmp(&a.plays).then(a.day.cmp(&b.day)));
    top_days.truncate(5);

    // ── Months, from their first to the end of the history ──
    let mut months = Vec::new();
    if let (Some(first_day), Some(last_day)) = (mine_by_day.keys().next(), all_by_day.keys().last())
    {
        let mut all_m: HashMap<String, i64> = HashMap::new();
        let mut mine_m: HashMap<String, i64> = HashMap::new();
        for (d, n) in &all_by_day {
            *all_m.entry(month_key(*d)).or_default() += n;
        }
        for (d, n) in &mine_by_day {
            *mine_m.entry(month_key(*d)).or_default() += n;
        }
        let mut m = NaiveDate::from_ymd_opt(first_day.year(), first_day.month(), 1).unwrap();
        while m <= *last_day {
            let key = month_key(m);
            months.push(MonthShare {
                plays: mine_m.get(&key).copied().unwrap_or(0),
                all: all_m.get(&key).copied().unwrap_or(0),
                month: key,
            });
            m = m.checked_add_months(chrono::Months::new(1)).unwrap();
        }
    }

    // ── Runs and playthroughs, over the whole sequence ──
    let mut runs: Vec<Run> = Vec::new();
    let mut playthroughs: Vec<Playthrough> = Vec::new();
    let mut run: Option<Run> = None;
    let mut album_run: Vec<&Listen> = Vec::new();
    let flush_album = |album_run: &mut Vec<&Listen>, out: &mut Vec<Playthrough>| {
        if album_run.len() >= PLAYTHROUGH_MIN_TRACKS {
            out.push(Playthrough {
                album: album_run[0].album.clone().unwrap_or_default(),
                start: album_run[0].ts,
                date: date(album_run[0].ts, tz).to_string(),
                first_track: album_run[0].track_number.unwrap_or(0),
                last_track: album_run[album_run.len() - 1].track_number.unwrap_or(0),
            });
        }
        album_run.clear();
    };
    let mut prev_ts: Option<i64> = None;
    for l in listens {
        let continues = prev_ts.is_some_and(|p| l.ts - p < SITTING_GAP_SECS);
        prev_ts = Some(l.ts);
        if !l.mine {
            runs.extend(run.take());
            flush_album(&mut album_run, &mut playthroughs);
            continue;
        }
        run = match run.take() {
            Some(r) if continues => Some(Run { length: r.length + 1, start: r.start, end: l.ts }),
            other => {
                runs.extend(other);
                Some(Run { length: 1, start: l.ts, end: l.ts })
            }
        };
        let next_track = album_run.last().is_some_and(|p| {
            continues
                && matches!((&p.album, &l.album), (Some(a), Some(b)) if same_album(a, b))
                && matches!((p.track_number, l.track_number), (Some(a), Some(b)) if b == a + 1)
        });
        if !next_track {
            flush_album(&mut album_run, &mut playthroughs);
        }
        if l.album.as_deref().is_some_and(|a| !a.trim().is_empty()) && l.track_number.is_some() {
            album_run.push(l);
        }
    }
    runs.extend(run);
    flush_album(&mut album_run, &mut playthroughs);
    runs.retain(|r| r.length > 1);
    runs.sort_by(|a, b| b.length.cmp(&a.length).then(a.start.cmp(&b.start)));
    runs.truncate(3);
    playthroughs.sort_by(|a, b| {
        (b.last_track - b.first_track)
            .cmp(&(a.last_track - a.first_track))
            .then(a.start.cmp(&b.start))
    });
    playthroughs.truncate(10);

    // ── Habit or burst ──
    let habit = (plays >= 5).then(|| {
        let (window_first, window_last) =
            (all_by_day.keys().next().unwrap(), all_by_day.keys().last().unwrap());
        let (mine_first, mine_last) =
            (mine_by_day.keys().next().unwrap(), mine_by_day.keys().last().unwrap());
        let full_span = ((*window_last - *window_first).num_days() + 1).max(1) as f64;
        let span = ((*mine_last - *mine_first).num_days() + 1).max(1) as f64;
        let return_rate = mine_by_day.len() as f64 / span;
        let span_coverage = span / full_span;
        let top_day_share = *mine_by_day.values().max().unwrap() as f64 / plays as f64;
        let score = (100.0
            * (0.45 * return_rate + 0.35 * span_coverage + 0.20 * (1.0 - top_day_share)))
            .clamp(0.0, 100.0);
        Habit { return_rate, span_coverage, top_day_share, score }
    });

    // ── When they took off ──
    let step = (plays >= TOOK_OFF_MIN_PLAYS)
        .then(|| {
            let (start, end) = (*mine_by_day.keys().next()?, *all_by_day.keys().last()?);
            let n = (end - start).num_days() + 1;
            let series: Vec<i64> = (0..n)
                .map(|i| {
                    mine_by_day.get(&(start + chrono::Duration::days(i))).copied().unwrap_or(0)
                })
                .collect();
            step_up(&series).map(|(k, before, after)| Step {
                date: (start + chrono::Duration::days(k as i64)).to_string(),
                before_per_week: (before * 7.0 * 10.0).round() / 10.0,
                after_per_week: (after * 7.0 * 10.0).round() / 10.0,
            })
        })
        .flatten();
    let took_off = step.as_ref().map(|s| s.date.clone());

    // ── Weeks: theirs against everything, and against your other favourites ──
    let mut weeks: Vec<WeekShare> = Vec::new();
    let mut rivals: Vec<Rival> = Vec::new();
    if let (Some(first_day), Some(last_day)) = (all_by_day.keys().next(), all_by_day.keys().last())
    {
        let (w0, w1) = (week_key(*first_day, week_start), week_key(*last_day, week_start));
        let n = ((w1 - w0).num_days() / 7 + 1) as usize;
        let at = |d: NaiveDate| ((week_key(d, week_start) - w0).num_days() / 7) as usize;
        let (mut all_w, mut mine_w) = (vec![0i64; n], vec![0i64; n]);
        for (d, c) in &all_by_day {
            all_w[at(*d)] += c;
        }
        for (d, c) in &mine_by_day {
            mine_w[at(*d)] += c;
        }
        weeks = (0..n)
            .map(|i| WeekShare {
                week: (w0 + chrono::Duration::days(7 * i as i64)).to_string(),
                plays: mine_w[i],
                all: all_w[i],
            })
            .collect();

        // Your other most-credited artists over the same window.
        let mut tally: HashMap<&str, (i64, &str)> = HashMap::new();
        for c in credits {
            if c.artist_key == artist_key || crate::db::is_various_artists(&c.artist_key) {
                continue;
            }
            let e = tally.entry(c.artist_key.as_str()).or_insert((0, c.name.as_str()));
            e.0 += 1;
            e.1 = c.name.as_str();
        }
        let mut top: Vec<(&str, i64, &str)> =
            tally.into_iter().map(|(k, (n, name))| (k, n, name)).collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        top.truncate(RIVALS);
        let mut series: HashMap<&str, Vec<i64>> =
            top.iter().map(|(k, _, _)| (*k, vec![0i64; n])).collect();
        for c in credits {
            if let Some(v) = series.get_mut(c.artist_key.as_str()) {
                let d = date(c.ts, tz);
                if d >= *first_day && d <= *last_day {
                    v[at(d)] += 1;
                }
            }
        }
        rivals = top
            .into_iter()
            .map(|(k, plays, name)| Rival {
                name: name.to_string(),
                plays,
                weekly: series.remove(k).unwrap_or_default(),
            })
            .collect();
    }

    // ── What plays next ──
    let mut by_ts: HashMap<i64, Vec<&Credit>> = HashMap::new();
    for c in credits {
        by_ts.entry(c.ts).or_default().push(c);
    }
    let mut next_total = 0i64;
    let mut next_tally: HashMap<&str, (i64, &str)> = HashMap::new();
    for pair in listens.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if !a.mine || b.mine || b.ts - a.ts > NEXT_GAP_SECS {
            continue;
        }
        let names: Vec<&&Credit> = by_ts
            .get(&b.ts)
            .map(|v| v.iter().filter(|c| !crate::db::is_various_artists(&c.artist_key)).collect())
            .unwrap_or_default();
        if names.is_empty() {
            continue;
        }
        next_total += 1;
        for c in names {
            let e = next_tally.entry(c.artist_key.as_str()).or_insert((0, c.name.as_str()));
            e.0 += 1;
        }
    }
    let mut next_artists: Vec<NextArtist> = next_tally
        .into_values()
        .map(|(plays, name)| NextArtist { name: name.to_string(), plays })
        .collect();
    next_artists.sort_by(|a, b| b.plays.cmp(&a.plays).then(a.name.cmp(&b.name)));
    next_artists.truncate(NEXT_ARTISTS);
    let next = Next { total: next_total, artists: next_artists };

    // ── Number one ──
    let at_top = |key_of: &dyn Fn(NaiveDate) -> String| {
        let mut buckets: HashMap<String, HashMap<&str, i64>> = HashMap::new();
        for c in credits {
            *buckets
                .entry(key_of(date(c.ts, tz)))
                .or_default()
                .entry(c.artist_key.as_str())
                .or_default() += 1;
        }
        let periods = buckets
            .values()
            .filter(|counts| {
                let top = counts.values().copied().max().unwrap_or(0);
                counts.get(artist_key).is_some_and(|n| *n == top && top > 0)
            })
            .count() as i64;
        AtTop { periods, of: buckets.len() as i64 }
    };
    let months_at_top = at_top(&|d| month_key(d));
    let weeks_at_top = at_top(&|d| week_key(d, week_start).to_string());

    Dossier {
        plays,
        all_plays: listens.len() as i64,
        days: mine_by_day.len() as i64,
        all_days: all_by_day.len() as i64,
        first,
        last,
        took_off,
        step,
        habit,
        daily,
        months,
        weeks,
        rivals,
        next,
        clock,
        week_start,
        top_days,
        runs,
        playthroughs,
        months_at_top,
        weeks_at_top,
        capped,
    }
}

/// The one split of `days` (listens per day) where a higher rate after it
/// best explains the listening, as `(first day after, rate before, rate after)`
/// in listens a day.
///
/// The textbook single change point for counts: for each split, the Poisson
/// log-likelihood of "two rates" over "one rate"; the largest wins, if it
/// steps *up* and clears `STEP_MIN_EVIDENCE`. Closed form, one pass of
/// prefix sums — no smoothing window to choose, which is what made the old
/// two-week climb jump to the sharpest fortnight rather than the real turn.
fn step_up(days: &[i64]) -> Option<(usize, f64, f64)> {
    let n = days.len() as i64;
    if n < 2 * STEP_MIN_DAYS {
        return None;
    }
    let xlogx = |x: f64| if x > 0.0 { x * x.ln() } else { 0.0 };
    let total: i64 = days.iter().sum();
    let one = xlogx(total as f64) - total as f64 * (n as f64).ln();
    let mut best: Option<(f64, usize, f64, f64)> = None;
    let mut before = 0i64;
    for k in 1..n {
        before += days[k as usize - 1];
        if k < STEP_MIN_DAYS || n - k < STEP_MIN_DAYS {
            continue;
        }
        let after = total - before;
        let (r1, r2) = (before as f64 / k as f64, after as f64 / (n - k) as f64);
        if r2 <= r1 {
            continue;
        }
        let two = xlogx(before as f64) - before as f64 * (k as f64).ln() + xlogx(after as f64)
            - after as f64 * ((n - k) as f64).ln();
        let gain = two - one;
        if best.is_none_or(|b| gain > b.0) {
            best = Some((gain, k as usize, r1, r2));
        }
    }
    best.filter(|b| b.0 >= STEP_MIN_EVIDENCE).map(|(_, k, r1, r2)| (k, r1, r2))
}

#[cfg(test)]
mod tests {
    use super::*;
    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    /// 2026-03-27 00:00 in Berlin (CET, UTC+1).
    const DAY: i64 = 1774566000;

    fn mine(ts: i64, album: &str, track: i32) -> Listen {
        Listen { ts, mine: true, album: Some(album.into()), track_number: Some(track) }
    }
    fn other(ts: i64) -> Listen {
        Listen { ts, mine: false, album: Some("Elsewhere".into()), track_number: Some(1) }
    }

    /// Every share comes with the population it is a share of, over the same
    /// listens: a day's `all` counts this artist's listens too.
    #[test]
    fn shares_carry_their_population() {
        let l = vec![mine(DAY + 3600, "Lover", 1), other(DAY + 7200), other(DAY + 86400 + 60)];
        let d = compute(&l, &[], "taylor swift", BERLIN, 1, false);
        assert_eq!((d.plays, d.all_plays, d.days, d.all_days), (1, 3, 1, 2));
        assert_eq!(
            d.daily,
            vec![DayCount { day: DAY, date: "2026-03-27".into(), plays: 1, all: 2 }]
        );
        assert_eq!(d.months, vec![MonthShare { month: "2026-03".into(), plays: 1, all: 3 }]);
    }

    /// The day is Berlin's: 23:30 UTC on the 26th is already the 27th there.
    #[test]
    fn days_are_local() {
        let l = vec![mine(DAY - 1800 + 3600, "Lover", 1)]; // 00:30 Berlin, 23:30 UTC the day before
        let d = compute(&l, &[], "x", BERLIN, 1, false);
        assert_eq!((d.daily[0].day, d.daily[0].date.as_str()), (DAY, "2026-03-27"));
        assert_eq!(d.clock[5][0], 1, "a Friday at midnight hour, Sunday-first rows");
    }

    /// Track 1, 2, 3, 4 of one record in one sitting is a playthrough; an
    /// interruption, a skipped number or a half-hour silence ends it.
    #[test]
    fn a_playthrough_is_four_tracks_in_order_in_one_sitting() {
        let mut l: Vec<Listen> = (1..=5).map(|t| mine(DAY + t as i64 * 200, "Lover", t)).collect();
        l.push(other(DAY + 1300));
        l.extend((1..=3).map(|t| mine(DAY + 1400 + t as i64 * 200, "Lover", t)));
        l.extend([
            mine(DAY + 9000, "Red", 1),
            mine(DAY + 9200, "Red", 2),
            mine(DAY + 9400, "Red", 4),
            mine(DAY + 9600, "Red", 5),
        ]);
        let d = compute(&l, &[], "x", BERLIN, 1, false);
        assert_eq!(
            d.playthroughs,
            vec![Playthrough {
                album: "Lover".into(),
                start: DAY + 200,
                date: "2026-03-27".into(),
                first_track: 1,
                last_track: 5
            }]
        );
    }

    /// A run is broken by anyone else and by a half-hour silence.
    #[test]
    fn runs_break_on_other_artists_and_silence() {
        let mut l: Vec<Listen> = (0..4).map(|i| mine(DAY + i * 200, "A", 1)).collect();
        l.push(other(DAY + 900));
        l.extend((0..3).map(|i| mine(DAY + 1000 + i * 200, "A", 1)));
        l.extend((0..2).map(|i| mine(DAY + 1000 + 400 + SITTING_GAP_SECS + i * 200, "A", 1)));
        let d = compute(&l, &[], "x", BERLIN, 1, false);
        assert_eq!(d.runs.iter().map(|r| r.length).collect::<Vec<_>>(), vec![4, 3, 2]);
    }

    /// Number one in a period, ties included; periods without them still count
    /// in `of`.
    #[test]
    fn months_at_top_count_ties_and_every_month() {
        let c = |ts: i64, a: &str| Credit { ts, artist_key: a.into(), name: a.into() };
        let march = DAY;
        let april = DAY + 10 * 86400;
        let credits = vec![
            c(march, "x"),
            c(march + 1, "y"),
            c(april, "y"),
            c(april + 1, "y"),
            c(april + 2, "x"),
        ];
        let d = compute(&[mine(march, "A", 1)], &credits, "x", BERLIN, 1, false);
        assert_eq!(d.months_at_top, AtTop { periods: 1, of: 2 });
    }

    /// A trickle and then a jump: the step is the day of the jump, with the
    /// rates either side.
    #[test]
    fn took_off_is_the_step_up() {
        let mut l: Vec<Listen> =
            (0..60).step_by(10).map(|i| mine(DAY + i * 86400 + 3600, "A", 1)).collect();
        let jump = DAY + 60 * 86400;
        for day in 0..20 {
            l.extend((0..5).map(|k| mine(jump + day * 86400 + 3600 + k * 300, "A", 1)));
        }
        l.sort_by_key(|x| x.ts);
        let d = compute(&l, &[], "x", BERLIN, 1, false);
        let step = d.step.expect("a clear step up");
        assert_eq!(step.date, date(jump, BERLIN).to_string());
        assert_eq!(d.took_off.as_deref(), Some(step.date.as_str()));
        assert_eq!((step.before_per_week, step.after_per_week), (0.7, 35.0));
    }

    /// A steady favourite and a faded one have no step up to mark.
    #[test]
    fn steady_or_fading_listening_has_no_step() {
        let steady: Vec<Listen> = (0..90).map(|i| mine(DAY + i * 86400 + 3600, "A", 1)).collect();
        assert!(compute(&steady, &[], "x", BERLIN, 1, false).step.is_none());
        let mut fading: Vec<Listen> = Vec::new();
        for day in 0..20 {
            fading.extend((0..5).map(|k| mine(DAY + day * 86400 + 3600 + k * 300, "A", 1)));
        }
        fading.extend((0..6).map(|i| mine(DAY + (30 + i * 10) * 86400, "A", 1)));
        assert!(compute(&fading, &[], "x", BERLIN, 1, false).step.is_none());
    }

    /// Weeks run from the history's first to its last, empty ones included,
    /// and rivals line up with them; this artist is never their own rival.
    #[test]
    fn weeks_and_rivals_share_one_axis() {
        let c = |ts: i64, a: &str| Credit { ts, artist_key: a.to_lowercase(), name: a.into() };
        // Friday 27 March, then Monday 13 April: three Monday-start weeks.
        let later = DAY + 17 * 86400;
        let l = vec![mine(DAY + 3600, "A", 1), other(DAY + 7200), other(later + 3600)];
        let credits = vec![c(DAY + 3600, "X"), c(DAY + 7200, "Rival"), c(later + 3600, "Rival")];
        let d = compute(&l, &credits, "x", BERLIN, 1, false);
        let weeks: Vec<(&str, i64, i64)> =
            d.weeks.iter().map(|w| (w.week.as_str(), w.plays, w.all)).collect();
        assert_eq!(
            weeks,
            vec![
                ("2026-03-23", 1, 2),
                ("2026-03-30", 0, 0),
                ("2026-04-06", 0, 0),
                ("2026-04-13", 0, 1)
            ]
        );
        assert_eq!(d.rivals.len(), 1);
        assert_eq!(
            (d.rivals[0].name.as_str(), d.rivals[0].weekly.clone()),
            ("Rival", vec![1, 0, 0, 1])
        );
    }

    /// Only the next listen in the same quarter hour counts, only when it is
    /// somebody else, and each artist it credits counts once.
    #[test]
    fn next_counts_who_follows_within_a_quarter_hour() {
        let c = |ts: i64, a: &str| Credit { ts, artist_key: a.to_lowercase(), name: a.into() };
        let l = vec![
            mine(DAY, "A", 1),
            other(DAY + 200), // follows: counts
            mine(DAY + 400, "A", 2),
            mine(DAY + 600, "A", 3), // followed by themselves: not a transition
            other(DAY + 800),        // follows: counts, a duet
            mine(DAY + 900, "A", 4),
            other(DAY + 900 + NEXT_GAP_SECS + 1), // too late
        ];
        let credits = vec![
            c(DAY + 200, "Madonna"),
            c(DAY + 800, "Madonna"),
            c(DAY + 800, "Clara Luciani"),
            c(DAY + 900 + NEXT_GAP_SECS + 1, "Late"),
        ];
        let d = compute(&l, &credits, "x", BERLIN, 1, false);
        assert_eq!(d.next.total, 2);
        let got: Vec<(&str, i64)> =
            d.next.artists.iter().map(|a| (a.name.as_str(), a.plays)).collect();
        assert_eq!(got, vec![("Madonna", 2), ("Clara Luciani", 1)]);
    }

    /// Under twenty listens there is no curve to find a turn in.
    #[test]
    fn a_handful_of_listens_has_no_take_off_and_five_have_a_habit() {
        let l: Vec<Listen> = (0..5).map(|i| mine(DAY + i * 86400, "A", 1)).collect();
        let d = compute(&l, &[], "x", BERLIN, 1, false);
        assert!(d.took_off.is_none());
        let h = d.habit.expect("five listens");
        assert_eq!(h.return_rate, 1.0);
        assert!((h.top_day_share - 0.2).abs() < 1e-9);
    }
}
