//! When a season is on. Dates are the listener's own calendar days (their IANA
//! zone, as everywhere else), so the Christmas tab appears at local midnight on
//! 1 September, not at UTC's.
//!
//! - **Christmas**: 1 September – 6 January. Glenn McDonald's Philippine start
//!   date, and Epiphany as the end, so the January tail of a season stays in it.
//! - **Halloween**: 1 October – 2 November.
//! - **Easter**: Palm Sunday – Easter Monday, Western (Gregorian) date.

use super::titles::Season;
use chrono::{Datelike, Duration, NaiveDate};

/// Western Easter Sunday — the anonymous Gregorian algorithm (Meeus/Jones/Butcher).
pub fn easter_sunday(year: i32) -> NaiveDate {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = ((h + l - 7 * m + 114) % 31) + 1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("valid Easter date")
}

/// The season that starts in `year`, first and last day inclusive. A
/// Christmas season is named for the year its September falls in.
pub fn bounds(season: Season, year: i32) -> (NaiveDate, NaiveDate) {
    let ymd = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).expect("valid date");
    match season {
        Season::Christmas => (ymd(year, 9, 1), ymd(year + 1, 1, 6)),
        Season::Halloween => (ymd(year, 10, 1), ymd(year, 11, 2)),
        Season::Easter => {
            let e = easter_sunday(year);
            (e - Duration::days(7), e + Duration::days(1))
        }
    }
}

/// The season `day` falls in, by the year it started in.
pub fn season_of(season: Season, day: NaiveDate) -> Option<i32> {
    // A January day can belong to the Christmas season that began last year.
    [day.year(), day.year() - 1].into_iter().find(|&y| {
        let (start, end) = bounds(season, y);
        day >= start && day <= end
    })
}

/// The season to show on `today`: the one running, else the most recent one.
pub fn current_or_last(season: Season, today: NaiveDate) -> i32 {
    if let Some(y) = season_of(season, today) {
        return y;
    }
    let (start, _) = bounds(season, today.year());
    if today < start {
        today.year() - 1
    } else {
        today.year()
    }
}

/// The days the season curve is drawn over: a few days of run-up before the
/// season, so the rise is visible against what came before. McDonald's chart
/// started on 28 August, the last day before Christmas listening lifts.
pub fn curve_bounds(season: Season, year: i32) -> (NaiveDate, NaiveDate) {
    let (start, end) = bounds(season, year);
    let lead = match season {
        Season::Christmas => 4,
        Season::Halloween => 14,
        Season::Easter => 14,
    };
    (start - Duration::days(lead), end)
}

/// The day a listener's season opened: the first day in the season from which
/// three days running were each at least `max(3 × baseline, 5%)` the season's
/// music, with at least one of its listens a day. `days` is
/// `(day, plays, the season's plays)` in date order; `baseline` is the share
/// in the months before, which is McDonald's "ambient minimum" — some
/// Christmas songs are played every day of the year.
pub fn opened(
    days: &[(NaiveDate, i64, i64)],
    baseline: f64,
    start: NaiveDate,
) -> Option<NaiveDate> {
    let floor = (baseline * 3.0).max(0.05);
    let lifted = |&(_, plays, flagged): &(NaiveDate, i64, i64)| {
        flagged > 0 && plays > 0 && flagged as f64 / plays as f64 >= floor
    };
    days.windows(3).find(|w| w[0].0 >= start && w.iter().all(lifted)).map(|w| w[0].0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn easter_dates_match_the_published_tables() {
        assert_eq!(easter_sunday(2024), d(2024, 3, 31));
        assert_eq!(easter_sunday(2025), d(2025, 4, 20));
        assert_eq!(easter_sunday(2026), d(2026, 4, 5));
        assert_eq!(easter_sunday(2027), d(2027, 3, 28));
        assert_eq!(easter_sunday(2038), d(2038, 4, 25));
    }

    #[test]
    fn the_christmas_season_runs_from_september_to_epiphany() {
        assert_eq!(season_of(Season::Christmas, d(2026, 8, 31)), None);
        assert_eq!(season_of(Season::Christmas, d(2026, 9, 1)), Some(2026));
        assert_eq!(season_of(Season::Christmas, d(2027, 1, 6)), Some(2026));
        assert_eq!(season_of(Season::Christmas, d(2027, 1, 7)), None);
        assert_eq!(current_or_last(Season::Christmas, d(2027, 3, 1)), 2026);
        assert_eq!(current_or_last(Season::Christmas, d(2026, 11, 17)), 2026);
    }

    #[test]
    fn a_season_opens_on_the_first_of_three_days_above_the_ambient_level() {
        let start = d(2026, 9, 1);
        let mut days: Vec<(NaiveDate, i64, i64)> =
            (0..40).map(|i| (start + Duration::days(i), 20, 0)).collect();
        days[10].2 = 5; // one day is not a season
        for day in days.iter_mut().skip(20).take(5) {
            day.2 = 4;
        }
        assert_eq!(opened(&days, 0.01, start), Some(d(2026, 9, 21)));
        assert_eq!(opened(&days, 0.2, start), None, "under three times a high baseline");
    }

    #[test]
    fn halloween_and_easter() {
        assert_eq!(season_of(Season::Halloween, d(2026, 11, 2)), Some(2026));
        assert_eq!(season_of(Season::Halloween, d(2026, 11, 3)), None);
        // Easter 2026 is 5 April: Palm Sunday 29 March to Easter Monday 6 April.
        assert_eq!(bounds(Season::Easter, 2026), (d(2026, 3, 29), d(2026, 4, 6)));
        assert_eq!(current_or_last(Season::Easter, d(2026, 2, 1)), 2025);
    }
}
