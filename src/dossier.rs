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

/// The smoothing and the look-back of the take-off curve, in days.
const TOOK_OFF_WINDOW_DAYS: i64 = 14;

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
    /// The local day (`YYYY-MM-DD`) their two-week average climbed fastest.
    /// Absent under twenty listens, or if it never climbed.
    pub took_off: Option<String>,
    /// Absent under five listens, where there is no pattern to score.
    pub habit: Option<Habit>,
    /// Every day they were played, oldest first.
    pub daily: Vec<DayCount>,
    /// Every month from their first to the last of the history, oldest first,
    /// including months they were not played in.
    pub months: Vec<MonthShare>,
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
    // The day the two-week average rose most against the two weeks before:
    // the audit's "awakening", over every day from their first listen to their
    // last, including the silent ones.
    let took_off = (plays >= TOOK_OFF_MIN_PLAYS)
        .then(|| {
            let (start, end) = (*mine_by_day.keys().next()?, *mine_by_day.keys().last()?);
            let n = (end - start).num_days() + 1;
            let series: Vec<f64> = (0..n)
                .map(|i| {
                    mine_by_day.get(&(start + chrono::Duration::days(i))).copied().unwrap_or(0)
                        as f64
                })
                .collect();
            let w = TOOK_OFF_WINDOW_DAYS as usize;
            let smooth: Vec<f64> = (0..series.len())
                .map(|i| {
                    let from = (i + 1).saturating_sub(w);
                    series[from..=i].iter().sum::<f64>() / (i + 1 - from) as f64
                })
                .collect();
            (w..smooth.len())
                .map(|i| (smooth[i] - smooth[i - w], i))
                .filter(|(rise, _)| *rise > 0.0)
                .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)))
                .map(|(_, i)| (start + chrono::Duration::days(i as i64)).to_string())
        })
        .flatten();

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
        habit,
        daily,
        months,
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
        let c = |ts: i64, a: &str| Credit { ts, artist_key: a.into() };
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

    /// A steady trickle and then a jump: the take-off is the jump.
    #[test]
    fn took_off_finds_the_climb() {
        let mut l: Vec<Listen> =
            (0..60).step_by(10).map(|i| mine(DAY + i * 86400 + 3600, "A", 1)).collect();
        let jump = DAY + 60 * 86400;
        for day in 0..10 {
            l.extend((0..5).map(|k| mine(jump + day * 86400 + 3600 + k * 300, "A", 1)));
        }
        l.sort_by_key(|x| x.ts);
        let d = compute(&l, &[], "x", BERLIN, 1, false);
        let took = NaiveDate::parse_from_str(&d.took_off.expect("a climb"), "%Y-%m-%d").unwrap();
        let jumped = date(jump, BERLIN);
        assert!(
            took >= jumped && took <= jumped + chrono::Duration::days(14),
            "{took} vs {jumped}"
        );
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
