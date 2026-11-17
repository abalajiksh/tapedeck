//! Anniversaries of what is in somebody's history: composers' and artists'
//! births and deaths, bands' formations and break-ups, albums' releases.
//!
//! Only round numbers — 10, 20, 25, 30, 40 … 100, 75, and every 50 — so it
//! reads "Ravel was born 150 years ago", never "37 years". The wording follows
//! the subject: the living turn an age, the dead were born or died so long
//! ago, a band formed or split. Never "would have turned".

use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use utoipa::ToSchema;

/// Whether `n` years is worth marking.
pub fn round(n: i32) -> bool {
    n > 0 && (n == 25 || n == 75 || (n % 10 == 0 && n <= 100) || n % 50 == 0)
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
pub struct Anniversary {
    /// `born`, `died`, `formed`, `split` or `released`.
    pub kind: String,
    /// The artist, or the album.
    pub name: String,
    /// For an album: who it is filed under.
    pub artist: Option<String>,
    /// `YYYY[-MM[-DD]]`, as MusicBrainz has it.
    pub date: String,
    pub years: i32,
    /// For a person: alive, as far as MusicBrainz knows.
    pub living: bool,
    /// Falls on today's date (month and day), not only in this year.
    pub today: bool,
    /// Your plays of them, all time.
    pub plays: i64,
    /// The sentence to show.
    pub label: String,
}

fn md(date: &str) -> Option<(u32, u32)> {
    let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some((d.month(), d.day()))
}

/// An artist's anniversaries in `year`. `kind` is MusicBrainz's artist type:
/// a `Group`, `Orchestra` or `Choir` forms and splits; anyone else is born
/// and dies. On `today`, a birthday or deathday counts whether or not the
/// number is round — "Today: Ravel was born in 1875".
#[allow(clippy::too_many_arguments)]
pub fn of_artist(
    name: &str,
    kind: Option<&str>,
    begin: Option<&str>,
    end: Option<&str>,
    ended: bool,
    plays: i64,
    year: i32,
    today: NaiveDate,
) -> Vec<Anniversary> {
    let group = matches!(kind, Some("Group" | "Orchestra" | "Choir"));
    let mut out = Vec::new();
    let begin_year = begin.and_then(super::year_of);
    // A person MusicBrainz gives no death for, born more than 110 years ago,
    // is not "turning" anything — the record is simply incomplete.
    let living = !ended && end.is_none() && begin_year.is_some_and(|b| year - b <= 110);
    let today_md = (today.month(), today.day());

    if let (Some(date), Some(by)) = (begin, begin_year) {
        let n = year - by;
        let on_day = md(date) == Some(today_md) && year == today.year();
        if round(n) || on_day {
            let label = match (group, living, on_day) {
                (true, _, _) => format!("{n} years since {name} formed"),
                (false, true, true) => format!("{name} turns {n} today"),
                (false, true, false) => format!("{name} turns {n}"),
                (false, false, true) => format!("Today: {name} was born in {by}"),
                (false, false, false) => format!("{name} was born {n} years ago"),
            };
            out.push(Anniversary {
                kind: if group { "formed" } else { "born" }.into(),
                name: name.to_string(),
                artist: None,
                date: date.to_string(),
                years: n,
                living,
                today: on_day,
                plays,
                label,
            });
        }
    }
    if let (Some(date), Some(ey)) = (end, end.and_then(super::year_of)) {
        let n = year - ey;
        let on_day = md(date) == Some(today_md) && year == today.year();
        if round(n) || (on_day && n > 0) {
            let label = match (group, on_day) {
                (true, _) => format!("{n} years since {name} split up"),
                (false, true) => format!("Today: {name} died in {ey}"),
                (false, false) => format!("{n} years since {name} died"),
            };
            out.push(Anniversary {
                kind: if group { "split" } else { "died" }.into(),
                name: name.to_string(),
                artist: None,
                date: date.to_string(),
                years: n,
                living: false,
                today: on_day,
                plays,
                label,
            });
        }
    }
    out
}

/// An album's release anniversary in `year`, when round.
pub fn of_album(
    album: &str,
    artist: &str,
    released: i32,
    plays: i64,
    year: i32,
) -> Option<Anniversary> {
    let n = year - released;
    round(n).then(|| Anniversary {
        kind: "released".into(),
        name: album.to_string(),
        artist: Some(artist.to_string()),
        date: released.to_string(),
        years: n,
        living: false,
        today: false,
        plays,
        label: format!("{album} turns {n}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn only_round_numbers() {
        for n in [10, 20, 25, 30, 40, 50, 75, 90, 100, 150, 200, 250] {
            assert!(round(n), "{n}");
        }
        for n in [0, 5, 15, 37, 110, 120, 125, 130] {
            assert!(!round(n), "{n}");
        }
    }

    #[test]
    fn the_dead_were_born_the_living_turn_and_bands_form() {
        let ravel = of_artist(
            "Maurice Ravel",
            Some("Person"),
            Some("1875-03-07"),
            Some("1937-12-28"),
            true,
            40,
            2025,
            d(2025, 6, 1),
        );
        assert_eq!(ravel[0].label, "Maurice Ravel was born 150 years ago");
        assert_eq!(ravel.len(), 1, "1937 + 88 is not round");

        let shostakovich = of_artist(
            "Dmitri Shostakovich",
            Some("Person"),
            Some("1906-09-25"),
            Some("1975-08-09"),
            true,
            9,
            2025,
            d(2025, 6, 1),
        );
        assert_eq!(shostakovich[0].label, "50 years since Dmitri Shostakovich died");

        let part = of_artist(
            "Arvo Pärt",
            Some("Person"),
            Some("1935-09-11"),
            None,
            false,
            12,
            2025,
            d(2025, 6, 1),
        );
        assert_eq!(part[0].label, "Arvo Pärt turns 90");
        assert!(part[0].living);

        let band = of_artist(
            "Radiohead",
            Some("Group"),
            Some("1991"),
            None,
            false,
            500,
            2031,
            d(2031, 2, 1),
        );
        assert_eq!(band[0].label, "40 years since Radiohead formed");
    }

    #[test]
    fn a_birthday_today_counts_whatever_the_number() {
        let today = of_artist(
            "Maurice Ravel",
            Some("Person"),
            Some("1875-03-07"),
            Some("1937-12-28"),
            true,
            40,
            2026,
            d(2026, 3, 7),
        );
        assert_eq!(today[0].label, "Today: Maurice Ravel was born in 1875");
        assert!(today[0].today);
    }

    #[test]
    fn an_old_record_without_a_death_is_not_a_living_person() {
        let a = of_artist(
            "Some Composer",
            Some("Person"),
            Some("1775"),
            None,
            false,
            3,
            2025,
            d(2025, 1, 1),
        );
        assert_eq!(a[0].label, "Some Composer was born 250 years ago");
        assert!(!a[0].living);
    }

    #[test]
    fn albums() {
        assert_eq!(
            of_album("OK Computer", "Radiohead", 1997, 80, 2027).unwrap().label,
            "OK Computer turns 30"
        );
        assert!(of_album("Kid A", "Radiohead", 2000, 80, 2027).is_none());
    }
}
