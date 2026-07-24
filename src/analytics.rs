//! Musical-personality analytics — a five-axis trait profile computed purely
//! from listen rows (no external calls). Ported from the standalone
//! `musical_personality` tool. All axes are explainable ratios/entropy/Jaccard.
//!
//! Curator↔Drifter uses an "album block" proxy (runs of consecutive same-album
//! plays) rather than strict track-number sequencing, because Tapedeck does not
//! store track numbers.

use std::collections::{HashMap, HashSet};
use serde::Serialize;

pub struct ListenRow {
    pub ts: i64,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
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

// ── time helpers (UTC, from raw unix seconds) ──
fn hour_of(ts: i64) -> i64 { (((ts % 86400) + 86400) % 86400) / 3600 }
fn day_index(ts: i64) -> i64 { ts.div_euclid(86400) }
fn weekday(ts: i64) -> i64 { ((day_index(ts) + 4) % 7 + 7) % 7 } // 0=Sun .. 6=Sat
fn is_weekend(ts: i64) -> bool { matches!(weekday(ts), 0 | 6) }

/// Normalised Shannon entropy (0..1) of a list of counts.
fn entropy(counts: &[i64]) -> f64 {
    let total: i64 = counts.iter().sum();
    if total == 0 || counts.len() <= 1 { return 0.0; }
    let mut h = 0.0;
    for &c in counts {
        if c <= 0 { continue; }
        let p = c as f64 / total as f64;
        h -= p * p.log2();
    }
    let hmax = (counts.len() as f64).log2();
    if hmax > 0.0 { h / hmax } else { 0.0 }
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() { return 1.0; }
    let inter = a.intersection(b).count();
    let union = a.union(b).count().max(1);
    inter as f64 / union as f64
}

fn top_n_artists(rows: &[&ListenRow], n: usize) -> HashSet<String> {
    let mut c: HashMap<&str, i64> = HashMap::new();
    for r in rows { *c.entry(r.artist.as_str()).or_default() += 1; }
    let mut v: Vec<_> = c.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.into_iter().take(n).map(|(a, _)| a.to_string()).collect()
}

pub fn compute_personality(rows: &[ListenRow]) -> Personality {
    let total = rows.len();
    if total == 0 {
        return Personality { verdict: "Not enough listens yet".into(), traits: vec![], loyalty: vec![], total_plays: 0 };
    }
    let total_f = total as f64;

    // ── Adventurer ↔ Loyalist: artist spread ──
    let mut artist_counts: HashMap<&str, i64> = HashMap::new();
    for r in rows { *artist_counts.entry(r.artist.as_str()).or_default() += 1; }
    let n_artists = artist_counts.len();
    let mut counts_vec: Vec<i64> = artist_counts.values().copied().collect();
    counts_vec.sort_unstable_by(|a, b| b.cmp(a));
    let top1pct_n = ((n_artists as f64 * 0.01).ceil() as usize).max(1);
    let top1pct_share = counts_vec.iter().take(top1pct_n).sum::<i64>() as f64 / total_f;
    let adv = 100.0 * (0.5 * (1.0 - top1pct_share) + 0.5 * entropy(&counts_vec));

    // ── Curator ↔ Drifter: fraction of listens inside album blocks ──
    let mut sorted: Vec<&ListenRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.ts);
    let mut in_run = 0usize;
    let mut i = 0;
    while i < sorted.len() {
        let album = sorted[i].album.as_deref().filter(|s| !s.is_empty());
        if album.is_none() { i += 1; continue; }
        let mut j = i + 1;
        while j < sorted.len()
            && sorted[j].album.as_deref() == album
            && sorted[j].artist == sorted[i].artist
            && sorted[j].ts - sorted[j - 1].ts <= 1800
        {
            j += 1;
        }
        if j - i >= 4 { in_run += j - i; }
        i = j.max(i + 1);
    }
    let cur_share = in_run as f64 / total_f;
    let cur = (cur_share * 300.0).min(100.0);

    // ── Casual ↔ Obsessive: replay ratio ──
    let mut track_counts: HashMap<(&str, &str), i64> = HashMap::new();
    for r in rows { *track_counts.entry((r.artist.as_str(), r.title.as_str())).or_default() += 1; }
    let distinct = track_counts.len();
    let repeat_ratio = 1.0 - (distinct as f64 / total_f);
    let top_track_plays = track_counts.values().copied().max().unwrap_or(0);
    let obs = repeat_ratio * 100.0;

    // ── Lark ↔ Night Owl ──
    let night = rows.iter().filter(|r| { let h = hour_of(r.ts); h >= 21 || h <= 3 }).count();
    let morning = rows.iter().filter(|r| (6..=11).contains(&hour_of(r.ts))).count();
    let owl = 100.0 * night as f64 / (night + morning).max(1) as f64;

    // ── Mood-Driven ↔ Identity: how stable the rotation is by context ──
    let weekday_rows: Vec<&ListenRow> = rows.iter().filter(|r| !is_weekend(r.ts)).collect();
    let weekend_rows: Vec<&ListenRow> = rows.iter().filter(|r| is_weekend(r.ts)).collect();
    let morning_rows: Vec<&ListenRow> = rows.iter().filter(|r| (6..=11).contains(&hour_of(r.ts))).collect();
    let evening_rows: Vec<&ListenRow> = rows.iter().filter(|r| (18..=23).contains(&hour_of(r.ts))).collect();
    let jac_wd = jaccard(&top_n_artists(&weekday_rows, 10), &top_n_artists(&weekend_rows, 10));
    let jac_me = jaccard(&top_n_artists(&morning_rows, 10), &top_n_artists(&evening_rows, 10));
    let identity = 100.0 * (jac_wd + jac_me) / 2.0;

    let traits = vec![
        Trait { label: "Adventurer ↔ Loyalist".into(), low: "Loyalist".into(), high: "Adventurer".into(),
            score: adv.clamp(0.0, 100.0),
            evidence: format!("{} artists · top 1% own {:.0}% of listens", n_artists, 100.0 * top1pct_share) },
        Trait { label: "Curator ↔ Drifter".into(), low: "Drifter".into(), high: "Curator".into(),
            score: cur.clamp(0.0, 100.0),
            evidence: format!("{:.0}% of listens in album runs", 100.0 * cur_share) },
        Trait { label: "Casual ↔ Obsessive".into(), low: "Casual".into(), high: "Obsessive".into(),
            score: obs.clamp(0.0, 100.0),
            evidence: format!("{:.0}% replays · top track {}×", 100.0 * repeat_ratio, top_track_plays) },
        Trait { label: "Lark ↔ Night Owl".into(), low: "Morning Lark".into(), high: "Night Owl".into(),
            score: owl.clamp(0.0, 100.0),
            evidence: format!("night {:.0}% · morning {:.0}% of listens", 100.0 * night as f64 / total_f, 100.0 * morning as f64 / total_f) },
        Trait { label: "Mood-Driven ↔ Identity".into(), low: "Mood-Driven".into(), high: "Identity".into(),
            score: identity.clamp(0.0, 100.0),
            evidence: format!("weekday/weekend top-10 overlap {:.0}%", 100.0 * jac_wd) },
    ];

    let verdict = synthesise_verdict(&traits);
    let loyalty = compute_loyalty(rows);

    Personality { verdict, traits, loyalty, total_plays: total }
}

/// The two most-pronounced traits name the listener, e.g. "The Obsessive Night Owl".
fn synthesise_verdict(traits: &[Trait]) -> String {
    let mut rated: Vec<&Trait> = traits.iter().collect();
    rated.sort_by(|a, b| (b.score - 50.0).abs().partial_cmp(&(a.score - 50.0).abs()).unwrap());
    let mut descriptors = Vec::new();
    for t in rated.iter().take(2) {
        if (t.score - 50.0).abs() < 15.0 { continue; }
        descriptors.push(if t.score > 50.0 { t.high.clone() } else { t.low.clone() });
    }
    match descriptors.len() {
        0 => "The Balanced Listener".into(),
        1 => format!("The {}", descriptors[0]),
        _ => format!("The {} {}", descriptors[0], descriptors[1]),
    }
}

/// Per-artist loyalty: high = sustained/recurring, low = one-off bursts.
fn compute_loyalty(rows: &[ListenRow]) -> Vec<LoyalArtist> {
    let (min_ts, max_ts) = rows.iter().fold((i64::MAX, i64::MIN), |(mn, mx), r| (mn.min(r.ts), mx.max(r.ts)));
    let full_span = ((max_ts - min_ts) / 86400 + 1).max(1) as f64;

    let mut by_artist: HashMap<&str, Vec<i64>> = HashMap::new();
    for r in rows { by_artist.entry(r.artist.as_str()).or_default().push(r.ts); }

    let mut out: Vec<LoyalArtist> = by_artist.iter()
        .filter(|(_, ts)| ts.len() >= 5)
        .map(|(artist, ts)| {
            let plays = ts.len() as i64;
            let mut day_counts: HashMap<i64, i64> = HashMap::new();
            let (mut mn, mut mx) = (i64::MAX, i64::MIN);
            for &t in ts.iter() {
                *day_counts.entry(t.div_euclid(86400)).or_default() += 1;
                mn = mn.min(t); mx = mx.max(t);
            }
            let span = ((mx - mn) / 86400 + 1).max(1) as f64;
            let return_rate = day_counts.len() as f64 / span;
            let span_coverage = span / full_span;
            let top_day_share = day_counts.values().copied().max().unwrap_or(1) as f64 / plays as f64;
            let loyalty_score = (100.0 * (0.45 * return_rate + 0.35 * span_coverage + 0.20 * (1.0 - top_day_share)))
                .clamp(0.0, 100.0);
            LoyalArtist { artist: (*artist).to_string(), plays, loyalty_score }
        })
        .collect();
    out.sort_by(|a, b| b.plays.cmp(&a.plays));
    out.truncate(10);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ts: i64, a: &str, t: &str, album: Option<&str>) -> ListenRow {
        ListenRow { ts, artist: a.into(), title: t.into(), album: album.map(str::to_string) }
    }

    #[test]
    fn empty_is_safe() {
        let p = compute_personality(&[]);
        assert_eq!(p.total_plays, 0);
        assert!(p.traits.is_empty());
    }

    #[test]
    fn obsessive_listener_scores_high_on_replays() {
        // Same track 20×, one day → heavy replay, single artist.
        let rows: Vec<ListenRow> = (0..20).map(|i| row(1_700_000_000 + i * 200, "A", "Song", Some("Alb"))).collect();
        let p = compute_personality(&rows);
        let obs = p.traits.iter().find(|t| t.high == "Obsessive").unwrap();
        assert!(obs.score > 80.0, "expected obsessive, got {}", obs.score);
        // one artist owns everything → low adventurer
        let adv = p.traits.iter().find(|t| t.high == "Adventurer").unwrap();
        assert!(adv.score < 30.0, "expected loyalist, got {}", adv.score);
        assert!(p.verdict.starts_with("The "));
    }

    #[test]
    fn album_block_lifts_curator() {
        // A 6-track album played in sequence, 5 min apart → an album run.
        let rows: Vec<ListenRow> = (0..6).map(|i| row(1_700_000_000 + i * 300, "A", &format!("t{i}"), Some("LP"))).collect();
        let p = compute_personality(&rows);
        let cur = p.traits.iter().find(|t| t.high == "Curator").unwrap();
        assert!(cur.score > 50.0, "expected curator, got {}", cur.score);
    }

    #[test]
    fn loyalty_prefers_spread_over_burst() {
        // Burst artist: 10 plays all one day. Loyal artist: 10 plays across 10 days.
        let mut rows = Vec::new();
        for i in 0..10 { rows.push(row(1_700_000_000 + i * 100, "Burst", "s", None)); }
        for d in 0..10 { rows.push(row(1_700_000_000 + d * 86400, "Loyal", "s", None)); }
        let p = compute_personality(&rows);
        let burst = p.loyalty.iter().find(|l| l.artist == "Burst").unwrap();
        let loyal = p.loyalty.iter().find(|l| l.artist == "Loyal").unwrap();
        assert!(loyal.loyalty_score > burst.loyalty_score);
    }
}
