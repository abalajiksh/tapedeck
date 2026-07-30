//! Musical-personality analytics — a five-axis trait profile computed purely
//! from listen rows (no external calls). Ported from the standalone
//! `musical_personality` tool. All axes are explainable ratios/entropy/Jaccard.
//!
//! Hour-of-day and day-of-week are evaluated in the user's timezone (see the
//! `tz` argument). Curator↔Drifter uses strict track-number sequencing where
//! track numbers are present, and falls back to an "album block" proxy (runs of
//! consecutive same-album plays) where they aren't.

use chrono::{Datelike, TimeZone, Timelike, Weekday};
use chrono_tz::Tz;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

pub struct ListenRow {
    pub ts: i64,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub track_number: Option<i32>,
}

#[derive(Serialize)]
pub struct Trait {
    pub label: String,
    pub low: String,
    pub high: String,
    pub score: f64, // 0..100, 50 = balanced
    pub evidence: String,
}

#[derive(Serialize)]
pub struct LoyalArtist {
    pub artist: String,
    pub plays: i64,
    pub loyalty_score: f64,
}

#[derive(Serialize)]
pub struct Personality {
    pub verdict: String,
    pub traits: Vec<Trait>,
    pub loyalty: Vec<LoyalArtist>,
    pub total_plays: usize,
}

/// Parse an IANA timezone name (e.g. "Europe/Berlin"); falls back to UTC.
pub fn parse_tz(name: &str) -> Tz {
    name.parse().unwrap_or(chrono_tz::UTC)
}

fn local(ts: i64, tz: Tz) -> chrono::DateTime<Tz> {
    tz.timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(|| tz.timestamp_opt(0, 0).single().unwrap())
}
fn hour_of(ts: i64, tz: Tz) -> i64 {
    local(ts, tz).hour() as i64
}
fn is_weekend(ts: i64, tz: Tz) -> bool {
    matches!(local(ts, tz).weekday(), Weekday::Sat | Weekday::Sun)
}
/// Local calendar day as an ordinal (for grouping "days listened").
fn local_day(ts: i64, tz: Tz) -> i64 {
    local(ts, tz).date_naive().num_days_from_ce() as i64
}

fn entropy(counts: &[i64]) -> f64 {
    let total: i64 = counts.iter().sum();
    if total == 0 || counts.len() <= 1 {
        return 0.0;
    }
    let mut h = 0.0;
    for &c in counts {
        if c <= 0 {
            continue;
        }
        let p = c as f64 / total as f64;
        h -= p * p.log2();
    }
    let hmax = (counts.len() as f64).log2();
    if hmax > 0.0 {
        h / hmax
    } else {
        0.0
    }
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count();
    let union = a.union(b).count().max(1);
    inter as f64 / union as f64
}

fn top_n_artists(rows: &[&ListenRow], n: usize) -> HashSet<String> {
    let mut c: HashMap<&str, i64> = HashMap::new();
    for r in rows {
        *c.entry(r.artist.as_str()).or_default() += 1;
    }
    let mut v: Vec<_> = c.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.into_iter().take(n).map(|(a, _)| a.to_string()).collect()
}

pub fn compute_personality(rows: &[ListenRow], tz: Tz) -> Personality {
    let total = rows.len();
    if total == 0 {
        return Personality {
            verdict: "Not enough listens yet".into(),
            traits: vec![],
            loyalty: vec![],
            total_plays: 0,
        };
    }
    let total_f = total as f64;

    // ── Adventurer ↔ Loyalist: artist spread ──
    let mut artist_counts: HashMap<&str, i64> = HashMap::new();
    for r in rows {
        *artist_counts.entry(r.artist.as_str()).or_default() += 1;
    }
    let n_artists = artist_counts.len();
    let mut counts_vec: Vec<i64> = artist_counts.values().copied().collect();
    counts_vec.sort_unstable_by(|a, b| b.cmp(a));
    let top1pct_n = ((n_artists as f64 * 0.01).ceil() as usize).max(1);
    let top1pct_share = counts_vec.iter().take(top1pct_n).sum::<i64>() as f64 / total_f;
    let adv = 100.0 * (0.5 * (1.0 - top1pct_share) + 0.5 * entropy(&counts_vec));

    // ── Curator ↔ Drifter: fraction of listens inside album playthroughs ──
    // A run extends while it's the same album+artist within 30 min; if BOTH
    // rows carry a track number we also require them to be sequential.
    let mut sorted: Vec<&ListenRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.ts);
    let mut in_run = 0usize;
    let mut i = 0;
    while i < sorted.len() {
        let album = sorted[i].album.as_deref().filter(|s| !s.is_empty());
        if album.is_none() {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < sorted.len()
            && sorted[j].album.as_deref() == album
            && sorted[j].artist == sorted[i].artist
            && sorted[j].ts - sorted[j - 1].ts <= 1800
            && match (sorted[j - 1].track_number, sorted[j].track_number) {
                (Some(prev), Some(cur)) => cur == prev + 1, // strict where known
                _ => true,                                  // proxy otherwise
            }
        {
            j += 1;
        }
        if j - i >= 4 {
            in_run += j - i;
        }
        i = j.max(i + 1);
    }
    let cur_share = in_run as f64 / total_f;
    let cur = (cur_share * 300.0).min(100.0);

    // ── Casual ↔ Obsessive: replay ratio ──
    let mut track_counts: HashMap<(&str, &str), i64> = HashMap::new();
    for r in rows {
        *track_counts
            .entry((r.artist.as_str(), r.title.as_str()))
            .or_default() += 1;
    }
    let distinct = track_counts.len();
    let repeat_ratio = 1.0 - (distinct as f64 / total_f);
    let top_track_plays = track_counts.values().copied().max().unwrap_or(0);
    let obs = repeat_ratio * 100.0;

    // ── Lark ↔ Night Owl (local time) ──
    let night = rows
        .iter()
        .filter(|r| {
            let h = hour_of(r.ts, tz);
            h >= 21 || h <= 3
        })
        .count();
    let morning = rows
        .iter()
        .filter(|r| (6..=11).contains(&hour_of(r.ts, tz)))
        .count();
    let owl = 100.0 * night as f64 / (night + morning).max(1) as f64;

    // ── Mood-Driven ↔ Identity: how stable the rotation is by context ──
    let weekday_rows: Vec<&ListenRow> = rows.iter().filter(|r| !is_weekend(r.ts, tz)).collect();
    let weekend_rows: Vec<&ListenRow> = rows.iter().filter(|r| is_weekend(r.ts, tz)).collect();
    let morning_rows: Vec<&ListenRow> = rows
        .iter()
        .filter(|r| (6..=11).contains(&hour_of(r.ts, tz)))
        .collect();
    let evening_rows: Vec<&ListenRow> = rows
        .iter()
        .filter(|r| (18..=23).contains(&hour_of(r.ts, tz)))
        .collect();
    let jac_wd = jaccard(
        &top_n_artists(&weekday_rows, 10),
        &top_n_artists(&weekend_rows, 10),
    );
    let jac_me = jaccard(
        &top_n_artists(&morning_rows, 10),
        &top_n_artists(&evening_rows, 10),
    );
    let identity = 100.0 * (jac_wd + jac_me) / 2.0;

    let traits = vec![
        Trait {
            label: "Adventurer ↔ Loyalist".into(),
            low: "Loyalist".into(),
            high: "Adventurer".into(),
            score: adv.clamp(0.0, 100.0),
            evidence: format!(
                "{} artists · top 1% own {:.0}% of listens",
                n_artists,
                100.0 * top1pct_share
            ),
        },
        Trait {
            label: "Curator ↔ Drifter".into(),
            low: "Drifter".into(),
            high: "Curator".into(),
            score: cur.clamp(0.0, 100.0),
            evidence: format!("{:.0}% of listens in album runs", 100.0 * cur_share),
        },
        Trait {
            label: "Casual ↔ Obsessive".into(),
            low: "Casual".into(),
            high: "Obsessive".into(),
            score: obs.clamp(0.0, 100.0),
            evidence: format!(
                "{:.0}% replays · top track {}×",
                100.0 * repeat_ratio,
                top_track_plays
            ),
        },
        Trait {
            label: "Lark ↔ Night Owl".into(),
            low: "Morning Lark".into(),
            high: "Night Owl".into(),
            score: owl.clamp(0.0, 100.0),
            evidence: format!(
                "night {:.0}% · morning {:.0}% of listens",
                100.0 * night as f64 / total_f,
                100.0 * morning as f64 / total_f
            ),
        },
        Trait {
            label: "Mood-Driven ↔ Identity".into(),
            low: "Mood-Driven".into(),
            high: "Identity".into(),
            score: identity.clamp(0.0, 100.0),
            evidence: format!("weekday/weekend top-10 overlap {:.0}%", 100.0 * jac_wd),
        },
    ];

    let verdict = synthesise_verdict(&traits);
    let loyalty = compute_loyalty(rows, tz);

    Personality {
        verdict,
        traits,
        loyalty,
        total_plays: total,
    }
}

/// The two most-pronounced traits name the listener, e.g. "The Obsessive Night Owl".
fn synthesise_verdict(traits: &[Trait]) -> String {
    let mut rated: Vec<&Trait> = traits.iter().collect();
    rated.sort_by(|a, b| {
        (b.score - 50.0)
            .abs()
            .partial_cmp(&(a.score - 50.0).abs())
            .unwrap()
    });
    let mut descriptors = Vec::new();
    for t in rated.iter().take(2) {
        if (t.score - 50.0).abs() < 15.0 {
            continue;
        }
        descriptors.push(if t.score > 50.0 {
            t.high.clone()
        } else {
            t.low.clone()
        });
    }
    match descriptors.len() {
        0 => "The Balanced Listener".into(),
        1 => format!("The {}", descriptors[0]),
        _ => format!("The {} {}", descriptors[0], descriptors[1]),
    }
}

/// Per-artist loyalty: high = sustained/recurring, low = one-off bursts.
fn compute_loyalty(rows: &[ListenRow], tz: Tz) -> Vec<LoyalArtist> {
    let (min_day, max_day) = rows.iter().fold((i64::MAX, i64::MIN), |(mn, mx), r| {
        let d = local_day(r.ts, tz);
        (mn.min(d), mx.max(d))
    });
    let full_span = (max_day - min_day + 1).max(1) as f64;

    let mut by_artist: HashMap<&str, Vec<i64>> = HashMap::new();
    for r in rows {
        by_artist
            .entry(r.artist.as_str())
            .or_default()
            .push(local_day(r.ts, tz));
    }

    let mut out: Vec<LoyalArtist> = by_artist
        .iter()
        .filter(|(_, days)| days.len() >= 5)
        .map(|(artist, days)| {
            let plays = days.len() as i64;
            let mut day_counts: HashMap<i64, i64> = HashMap::new();
            let (mut mn, mut mx) = (i64::MAX, i64::MIN);
            for &d in days.iter() {
                *day_counts.entry(d).or_default() += 1;
                mn = mn.min(d);
                mx = mx.max(d);
            }
            let span = (mx - mn + 1).max(1) as f64;
            let return_rate = day_counts.len() as f64 / span;
            let span_coverage = span / full_span;
            let top_day_share =
                day_counts.values().copied().max().unwrap_or(1) as f64 / plays as f64;
            let loyalty_score = (100.0
                * (0.45 * return_rate + 0.35 * span_coverage + 0.20 * (1.0 - top_day_share)))
                .clamp(0.0, 100.0);
            LoyalArtist {
                artist: (*artist).to_string(),
                plays,
                loyalty_score,
            }
        })
        .collect();
    out.sort_by(|a, b| b.plays.cmp(&a.plays));
    out.truncate(10);
    out
}

/// 7 days × 24 hours listening grid in local time (rows Sun..Sat).
pub fn heatmap_local(rows: &[ListenRow], tz: Tz) -> Vec<Vec<i64>> {
    let mut grid = vec![vec![0i64; 24]; 7];
    for r in rows {
        let dt = local(r.ts, tz);
        let dow = dt.weekday().num_days_from_sunday() as usize; // 0=Sun
        let hr = dt.hour() as usize;
        grid[dow][hr] += 1;
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    const UTC: Tz = chrono_tz::UTC;

    fn row(ts: i64, a: &str, t: &str, album: Option<&str>, tn: Option<i32>) -> ListenRow {
        ListenRow {
            ts,
            artist: a.into(),
            title: t.into(),
            album: album.map(str::to_string),
            track_number: tn,
        }
    }

    #[test]
    fn empty_is_safe() {
        let p = compute_personality(&[], UTC);
        assert_eq!(p.total_plays, 0);
        assert!(p.traits.is_empty());
    }

    #[test]
    fn obsessive_listener_scores_high_on_replays() {
        let rows: Vec<ListenRow> = (0..20)
            .map(|i| row(1_700_000_000 + i * 200, "A", "Song", Some("Alb"), None))
            .collect();
        let p = compute_personality(&rows, UTC);
        let obs = p.traits.iter().find(|t| t.high == "Obsessive").unwrap();
        assert!(obs.score > 80.0, "expected obsessive, got {}", obs.score);
        let adv = p.traits.iter().find(|t| t.high == "Adventurer").unwrap();
        assert!(adv.score < 30.0, "expected loyalist, got {}", adv.score);
    }

    #[test]
    fn sequential_track_numbers_count_as_a_playthrough() {
        let rows: Vec<ListenRow> = (0..6)
            .map(|i| {
                row(
                    1_700_000_000 + i * 300,
                    "A",
                    &format!("t{i}"),
                    Some("LP"),
                    Some(i as i32 + 1),
                )
            })
            .collect();
        let p = compute_personality(&rows, UTC);
        let cur = p.traits.iter().find(|t| t.high == "Curator").unwrap();
        assert!(cur.score > 50.0, "expected curator, got {}", cur.score);
    }

    #[test]
    fn shuffled_track_numbers_do_not_count() {
        // Same album, within time gaps, but track numbers jump around → not a run.
        let order = [1, 7, 3, 9, 2, 8];
        let rows: Vec<ListenRow> = order
            .iter()
            .enumerate()
            .map(|(i, &tn)| {
                row(
                    1_700_000_000 + i as i64 * 300,
                    "A",
                    &format!("t{tn}"),
                    Some("LP"),
                    Some(tn),
                )
            })
            .collect();
        let p = compute_personality(&rows, UTC);
        let cur = p.traits.iter().find(|t| t.high == "Curator").unwrap();
        assert!(
            cur.score < 20.0,
            "shuffled shouldn't be curator, got {}",
            cur.score
        );
    }

    #[test]
    fn timezone_shifts_night_owl() {
        // Listens at 02:00 UTC. In UTC → night. In UTC-5 (America/New_York would be
        // 21:00 prev day, still night); use Asia/Kolkata (+5:30 → 07:30 → morning).
        let rows: Vec<ListenRow> = (0..10)
            .map(|i| {
                row(
                    1_700_000_000 + i * 90_000 + 2 * 3600,
                    "A",
                    &format!("s{i}"),
                    None,
                    None,
                )
            })
            .collect();
        let owl_utc = compute_personality(&rows, UTC)
            .traits
            .iter()
            .find(|t| t.high == "Night Owl")
            .unwrap()
            .score;
        let owl_ist = compute_personality(&rows, chrono_tz::Asia::Kolkata)
            .traits
            .iter()
            .find(|t| t.high == "Night Owl")
            .unwrap()
            .score;
        assert!(
            owl_utc > owl_ist,
            "UTC 02:00 is more 'night' than IST 07:30 ({owl_utc} vs {owl_ist})"
        );
    }
}
