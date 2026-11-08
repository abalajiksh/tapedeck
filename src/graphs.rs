//! The Statistics graphs: eight charts folded from one pass over the history.
//!
//! Ported from the standalone `musical_personality_v2.py` (and its plots), with
//! three corrections the script could not make because it only had the
//! submitted strings:
//!
//! * **Artists are everyone credited.** The script grouped on the artist
//!   string, so "Harris Jayaraj, Unni Menon, Bombay Jayashri" was one artist
//!   with one song. Here a listen counts for each credited name — the same fan
//!   out the charts, the artist page and Reports do, so one question gets one
//!   answer on every screen. A share of *artist credits* is therefore not a
//!   share of listens; each figure says which it is.
//! * **Days are the user's days.** Bucketed in their IANA zone, never UTC.
//! * **Compilations count for the performer.** "Various Artists" is a filing
//!   convention, not a performer, and never appears here.
//!
//! Everything is pure: rows in, a serialisable struct out. The handler does the
//! I/O and runs this off the async workers.

use chrono::{Datelike, NaiveDate, TimeZone};
use chrono_tz::Tz;
use serde::Serialize;
use std::collections::HashMap;

use crate::db::GraphRow;

/// Artists drawn on the obsession-waves chart. Six is the categorical palette.
pub const WAVE_ARTISTS: usize = 6;
/// Artists on the first-listens timeline.
pub const FIRST_LISTEN_ARTISTS: usize = 20;
/// Artists on the habit-vs-burst scatter.
pub const HABIT_ARTISTS: usize = 40;
/// Below this an artist has no pattern to speak of — five plays on one day and
/// five plays over five years both read as noise. The script's threshold.
pub const HABIT_MIN_PLAYS: i64 = 5;
/// Artists on the album-diversity bars.
pub const ALBUM_ARTISTS: usize = 12;
/// Albums named per artist before the rest fold into "N more".
pub const ALBUM_SLICES: usize = 5;
/// An artist needs this many plays before "mostly one song" means anything.
pub const ANCHOR_MIN_PLAYS: i64 = 20;
/// The lowest top-song share that still makes a song an artist's anchor.
pub const ANCHOR_MIN_SHARE: f64 = 0.4;
/// Rows on the anchor-songs list.
pub const ANCHOR_ARTISTS: usize = 15;
/// A session's devotion is only measured from this many listens: a one-song
/// session is trivially "all one artist" and would swamp the answer.
pub const DEVOTION_MIN_LISTENS: usize = 3;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StatsGraphs {
    /// The range asked for: `week`, `month`, `last30`, `year` or `all`.
    pub range: String,
    /// The IANA zone every day below is bucketed in.
    pub timezone: String,
    /// True when the history was longer than this server reads in one pass
    /// (scaled to its memory). The oldest listens are the ones left out.
    pub capped: bool,
    /// First local day of the axis, `YYYY-MM-DD`. Absent when the range is
    /// empty. Every `daily` array below starts here, one entry per day.
    pub start_day: Option<String>,
    /// Listens per local day, from `start_day` to today, including days with
    /// none.
    pub daily: Vec<i64>,
    pub volume: Volume,
    /// The most-credited artists in the range, with their own daily counts on
    /// the same axis as `daily`.
    pub waves: Vec<Wave>,
    pub long_tail: LongTail,
    pub sessions: Sessions,
    /// The range's most-credited artists, ordered by when you **first** heard
    /// them — across the whole history, not the range.
    pub first_listens: Vec<FirstListen>,
    /// Daily habit or burst: how each artist's listening is spread over time.
    /// Only artists with five or more plays on at least two different days.
    pub habits: Vec<ArtistHabit>,
    /// How each top artist's plays split across their records.
    pub albums: Vec<AlbumSpread>,
    /// Artists you mostly play one song from, most anchored first.
    pub anchors: Vec<Anchor>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Volume {
    /// Listens in the range.
    pub plays: i64,
    /// Days in the range with at least one listen.
    pub active_days: i64,
    /// Median listens on a day you listened at all. Zero-listen days are left
    /// out on purpose: they would drag the median of a sparse history to 0.
    pub median_active_day: f64,
    /// The busiest day in the range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak: Option<PeakDay>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PeakDay {
    /// `YYYY-MM-DD`, local.
    pub day: String,
    pub plays: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Wave {
    pub name: String,
    /// Credited plays in the range.
    pub plays: i64,
    /// Credited plays per local day, aligned with `StatsGraphs::daily`.
    pub daily: Vec<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LongTail {
    /// Credited plays per artist, largest first. The curve is their running
    /// sum over `credits`.
    pub counts: Vec<i64>,
    /// The sum of `counts`: artist credits, **not** listens — a listen with
    /// two artists credited counts once for each.
    pub credits: i64,
    /// Artists heard exactly once in the range.
    pub once: i64,
    /// Artists heard 100 times or more in the range.
    pub hundred_plus: i64,
    /// How many of the top artists it takes to reach half of `credits`.
    pub half: i64,
    /// How many of the top artists it takes to reach 80% of `credits`.
    pub eighty: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Sessions {
    /// Sessions in the range. A gap longer than `gap_minutes` starts a new one.
    pub total: i64,
    pub gap_minutes: i64,
    /// Sessions by how many listens they held, in fixed buckets.
    pub lengths: Vec<LengthBucket>,
    pub devotion: Devotion,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LengthBucket {
    /// `1`, `2`, `3–4`, `5–9`, `10–19`, `20–49`, `50–99` or `100+`.
    pub label: String,
    pub sessions: i64,
}

/// How much of each session went to its most-played artist.
///
/// Measured over sessions of at least `min_listens`, and over the *primary*
/// artist of each listen — fanning credits out here would let one session's
/// shares add up to more than everything played in it.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Devotion {
    pub min_listens: i64,
    /// One artist took 80% or more of the session.
    pub devoted: i64,
    /// The top artist took 40–80%.
    pub mixed: i64,
    /// No artist took as much as 40%.
    pub scattered: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FirstListen {
    pub name: String,
    /// Credited plays in the range.
    pub plays: i64,
    /// First listen ever, unix seconds — not ranged.
    pub first: i64,
    /// Most recent listen, unix seconds — not ranged.
    pub last: i64,
    /// 0–100. High is steady, recurring listening; low is bursts.
    pub loyalty: f64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ArtistHabit {
    pub name: String,
    /// Credited plays in the range.
    pub plays: i64,
    /// Days in the range this artist was played.
    pub active_days: i64,
    /// Days from their first to their last listen in the range, inclusive.
    pub span_days: i64,
    /// `active_days / span_days`: how often they come back. 1 = every day.
    pub return_rate: f64,
    /// Share of their plays on their single biggest day. 1 = all on one day.
    pub top_day_share: f64,
    /// That biggest day, `YYYY-MM-DD`.
    pub top_day: String,
    /// 0–100. High is steady, recurring listening; low is bursts.
    pub loyalty: f64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AlbumSpread {
    pub name: String,
    /// Credited plays in the range.
    pub plays: i64,
    /// Distinct records those plays were on.
    pub album_count: i64,
    /// The most-played records, largest first, at most five.
    pub albums: Vec<AlbumSlice>,
    /// Plays on every other record.
    pub rest_plays: i64,
    /// Plays whose source named no record at all.
    pub no_album: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AlbumSlice {
    pub name: String,
    /// Who the record is filed under, which is what opens it.
    pub album_artist: String,
    pub plays: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Anchor {
    pub name: String,
    /// Credited plays in the range.
    pub plays: i64,
    /// The one song most of them are.
    pub title: String,
    pub title_plays: i64,
    /// `title_plays / plays`.
    pub share: f64,
    /// How many different songs of theirs were played at all.
    pub tracks: i64,
}

// ── Credits ─────────────────────────────────────────────────────────────────

/// Who actually played it: the record's artist, unless that is the "Various
/// Artists" placeholder, in which case the first credited name.
pub(crate) fn performer<'a>(artist: &'a str, credit: Option<&'a str>) -> &'a str {
    if crate::db::is_various_artists(artist) {
        credit
            .and_then(|c| c.split(';').next())
            .map(str::trim)
            .filter(|c| !c.is_empty() && !crate::db::is_various_artists(c))
            .unwrap_or(artist)
    } else {
        artist
    }
}

/// Every artist a listen credits, primary first, as `(key, display)`.
///
/// `artist_credit` is everyone credited joined with `"; "` — the shape
/// `save_scrobble` and `fold_artist_credit` both write — so splitting it is
/// reading our own format. The placeholder is dropped wherever it appears.
pub(crate) fn credited<'a>(primary: &'a str, credit: Option<&'a str>) -> Vec<(String, &'a str)> {
    let mut out: Vec<(String, &'a str)> = Vec::new();
    let mut push = |n: &'a str| {
        let n = n.trim();
        if n.is_empty() || crate::db::is_various_artists(n) {
            return;
        }
        let key = n.to_lowercase();
        if !out.iter().any(|(k, _)| *k == key) {
            out.push((key, n));
        }
    };
    push(primary);
    for name in credit.into_iter().flat_map(|c| c.split(';')) {
        push(name);
    }
    out
}

// ── Days ────────────────────────────────────────────────────────────────────

/// Local calendar day as an ordinal.
fn day_of(ts: i64, tz: Tz) -> i32 {
    tz.timestamp_opt(ts, 0)
        .single()
        .map(|d| d.date_naive().num_days_from_ce())
        .unwrap_or_else(|| (ts.div_euclid(86_400) + 719_163) as i32)
}

fn day_label(d: i32) -> String {
    NaiveDate::from_num_days_from_ce_opt(d)
        .map(|n| n.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// The script's loyalty score: recurring, spread across the window, and not
/// all on one day.
fn loyalty(return_rate: f64, span: i64, window: i64, top_day_share: f64) -> f64 {
    let coverage = span as f64 / window.max(1) as f64;
    (100.0 * (0.45 * return_rate + 0.35 * coverage + 0.20 * (1.0 - top_day_share)))
        .clamp(0.0, 100.0)
}

// ── The fold ────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Artist {
    name: String,
    plays: i64,
    days: HashMap<i32, i64>,
    /// `filed|album` → (album, filed under, plays)
    albums: HashMap<String, (String, String, i64)>,
    no_album: i64,
    /// lowercased title → (title, plays)
    titles: HashMap<String, (String, i64)>,
}

/// Fold the graphs from `rows` (oldest first, the whole history).
///
/// `after` is the range's lower bound (None = all time), `before` its
/// exclusive upper bound (None = up to now — a report's period has one), and
/// `now` is today, all unix seconds; `tz` buckets every day.
#[allow(clippy::too_many_arguments)]
pub fn compute(
    rows: &[GraphRow],
    range: &str,
    after: Option<i64>,
    before: Option<i64>,
    now: i64,
    tz: Tz,
    capped: bool,
) -> StatsGraphs {
    // All-time first and last listen per artist, before the range narrows.
    let mut ever: HashMap<String, (i64, i64)> = HashMap::new();
    for r in rows {
        let p = performer(&r.artist, r.artist_credit.as_deref());
        for (key, _) in credited(p, r.artist_credit.as_deref()) {
            let e = ever.entry(key).or_insert((r.timestamp, r.timestamp));
            e.0 = e.0.min(r.timestamp);
            e.1 = e.1.max(r.timestamp);
        }
    }

    let in_range: Vec<&GraphRow> = rows
        .iter()
        .filter(|r| {
            after.is_none_or(|a| r.timestamp >= a) && before.is_none_or(|b| r.timestamp < b)
        })
        .collect();

    // The axis: from the range's start (or the first listen) to today, or to
    // the period's last day when that came first.
    let today = before.map_or(day_of(now, tz), |b| day_of(b - 1, tz).min(day_of(now, tz)));
    let start = match (after, in_range.first()) {
        (Some(a), _) => Some(day_of(a, tz)),
        (None, Some(r)) => Some(day_of(r.timestamp, tz)),
        (None, None) => None,
    };
    let end = in_range.last().map(|r| day_of(r.timestamp, tz)).map_or(today, |d| d.max(today));
    let axis_len = start.map_or(0, |s| (end - s + 1).max(0) as usize);
    let idx = |d: i32| start.and_then(|s| usize::try_from(d - s).ok()).filter(|&i| i < axis_len);

    let mut daily = vec![0i64; axis_len];
    let mut artists: HashMap<String, Artist> = HashMap::new();

    // Sessions walk the range in order, so they fold here too.
    let gap = crate::db::SESSION_GAP_SECS;
    let mut lengths = [0i64; 8];
    let (mut devoted, mut mixed, mut scattered) = (0i64, 0i64, 0i64);
    let mut session: Vec<String> = Vec::new();
    let mut last_ts: Option<i64> = None;
    let mut close = |s: &mut Vec<String>| {
        if s.is_empty() {
            return;
        }
        let n = s.len();
        lengths[match n {
            1 => 0,
            2 => 1,
            3..=4 => 2,
            5..=9 => 3,
            10..=19 => 4,
            20..=49 => 5,
            50..=99 => 6,
            _ => 7,
        }] += 1;
        if n >= DEVOTION_MIN_LISTENS {
            let mut c: HashMap<&str, usize> = HashMap::new();
            for k in s.iter() {
                *c.entry(k.as_str()).or_default() += 1;
            }
            let share = *c.values().max().unwrap_or(&0) as f64 / n as f64;
            if share >= 0.8 {
                devoted += 1;
            } else if share >= 0.4 {
                mixed += 1;
            } else {
                scattered += 1;
            }
        }
        s.clear();
    };

    for r in &in_range {
        let d = day_of(r.timestamp, tz);
        if let Some(i) = idx(d) {
            daily[i] += 1;
        }

        let credit = r.artist_credit.as_deref();
        let p = performer(&r.artist, credit);
        if last_ts.is_some_and(|t| r.timestamp - t > gap) {
            close(&mut session);
        }
        last_ts = Some(r.timestamp);
        session.push(p.trim().to_lowercase());

        let album = r.album.as_deref().map(str::trim).filter(|a| !a.is_empty());
        let filed = r
            .album_filed_under
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .unwrap_or(p.trim());
        let title_key = r.title.trim().to_lowercase();
        for (key, display) in credited(p, credit) {
            let a = artists.entry(key).or_default();
            // The newest spelling wins: rows arrive oldest first.
            a.name = display.to_string();
            a.plays += 1;
            *a.days.entry(d).or_default() += 1;
            match album {
                Some(al) => {
                    let k = format!("{}|{}", filed.to_lowercase(), al.to_lowercase());
                    let e =
                        a.albums.entry(k).or_insert_with(|| (al.to_string(), filed.to_string(), 0));
                    e.2 += 1;
                }
                None => a.no_album += 1,
            }
            let t = a
                .titles
                .entry(title_key.clone())
                .or_insert_with(|| (r.title.trim().to_string(), 0));
            t.1 += 1;
        }
    }
    close(&mut session);

    // Ranked by credited plays; ties by name so a cut is stable.
    let mut ranked: Vec<(&String, &Artist)> = artists.iter().collect();
    ranked.sort_by(|a, b| b.1.plays.cmp(&a.1.plays).then_with(|| a.1.name.cmp(&b.1.name)));

    // Per-artist spread over days, shared by habits and first listens.
    let window = axis_len as i64;
    let spread = |a: &Artist| -> (i64, i64, f64, f64, i32) {
        let (mn, mx) =
            a.days.keys().fold((i32::MAX, i32::MIN), |(lo, hi), &d| (lo.min(d), hi.max(d)));
        let span = (mx - mn + 1).max(1) as i64;
        let active = a.days.len() as i64;
        let (top_day, top_n) = a
            .days
            .iter()
            .max_by(|x, y| x.1.cmp(y.1).then_with(|| y.0.cmp(x.0)))
            .map(|(d, n)| (*d, *n))
            .unwrap_or((mn, 0));
        let return_rate = active as f64 / span as f64;
        let top_share = top_n as f64 / a.plays.max(1) as f64;
        (active, span, return_rate, top_share, top_day)
    };

    // ── Volume ──
    let plays = in_range.len() as i64;
    let mut active: Vec<i64> = daily.iter().copied().filter(|&n| n > 0).collect();
    active.sort_unstable();
    let median_active_day = match active.len() {
        0 => 0.0,
        n if n % 2 == 1 => active[n / 2] as f64,
        n => (active[n / 2 - 1] + active[n / 2]) as f64 / 2.0,
    };
    let peak = daily
        .iter()
        .enumerate()
        .filter(|(_, &n)| n > 0)
        // Earliest of equal peaks, so the answer doesn't move between calls.
        .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(&a.0)))
        .and_then(|(i, &n)| start.map(|s| PeakDay { day: day_label(s + i as i32), plays: n }));

    // ── Waves ──
    let waves = ranked
        .iter()
        .take(WAVE_ARTISTS)
        .map(|(_, a)| {
            let mut series = vec![0i64; axis_len];
            for (&d, &n) in &a.days {
                if let Some(i) = idx(d) {
                    series[i] += n;
                }
            }
            Wave { name: a.name.clone(), plays: a.plays, daily: series }
        })
        .collect();

    // ── Long tail ──
    let counts: Vec<i64> = ranked.iter().map(|(_, a)| a.plays).collect();
    let credits: i64 = counts.iter().sum();
    let reach = |frac: f64| -> i64 {
        let target = credits as f64 * frac;
        let mut run = 0i64;
        for (i, c) in counts.iter().enumerate() {
            run += c;
            if run as f64 >= target {
                return i as i64 + 1;
            }
        }
        counts.len() as i64
    };
    let long_tail = LongTail {
        once: counts.iter().filter(|&&c| c == 1).count() as i64,
        hundred_plus: counts.iter().filter(|&&c| c >= 100).count() as i64,
        half: if credits > 0 { reach(0.5) } else { 0 },
        eighty: if credits > 0 { reach(0.8) } else { 0 },
        counts,
        credits,
    };

    // ── Sessions ──
    const LABELS: [&str; 8] = ["1", "2", "3–4", "5–9", "10–19", "20–49", "50–99", "100+"];
    let sessions = Sessions {
        total: lengths.iter().sum(),
        gap_minutes: gap / 60,
        lengths: LABELS
            .iter()
            .zip(lengths)
            .map(|(l, n)| LengthBucket { label: (*l).to_string(), sessions: n })
            .collect(),
        devotion: Devotion { min_listens: DEVOTION_MIN_LISTENS as i64, devoted, mixed, scattered },
    };

    // ── First listens ──
    let mut first_listens: Vec<FirstListen> = ranked
        .iter()
        .take(FIRST_LISTEN_ARTISTS)
        .filter_map(|(key, a)| {
            let (first, last) = *ever.get(*key)?;
            let (_, span, rr, tds, _) = spread(a);
            Some(FirstListen {
                name: a.name.clone(),
                plays: a.plays,
                first,
                last,
                loyalty: loyalty(rr, span, window, tds),
            })
        })
        .collect();
    first_listens.sort_by(|a, b| a.first.cmp(&b.first).then_with(|| a.name.cmp(&b.name)));

    // ── Habits ──
    let habits = ranked
        .iter()
        // Heard on one day only, an artist scores 100% on both axes — always
        // back, and all on one day — which says nothing about either.
        .filter(|(_, a)| a.plays >= HABIT_MIN_PLAYS && a.days.len() >= 2)
        .take(HABIT_ARTISTS)
        .map(|(_, a)| {
            let (active_days, span_days, return_rate, top_day_share, top_day) = spread(a);
            ArtistHabit {
                name: a.name.clone(),
                plays: a.plays,
                active_days,
                span_days,
                return_rate,
                top_day_share,
                top_day: day_label(top_day),
                loyalty: loyalty(return_rate, span_days, window, top_day_share),
            }
        })
        .collect();

    // ── Album diversity ──
    let albums = ranked
        .iter()
        .take(ALBUM_ARTISTS)
        .map(|(_, a)| {
            let mut slices: Vec<&(String, String, i64)> = a.albums.values().collect();
            slices.sort_by(|x, y| y.2.cmp(&x.2).then_with(|| x.0.cmp(&y.0)));
            let named: Vec<AlbumSlice> = slices
                .iter()
                .take(ALBUM_SLICES)
                .map(|(n, f, p)| AlbumSlice { name: n.clone(), album_artist: f.clone(), plays: *p })
                .collect();
            let named_plays: i64 = named.iter().map(|s| s.plays).sum();
            let on_records: i64 = slices.iter().map(|s| s.2).sum();
            AlbumSpread {
                name: a.name.clone(),
                plays: a.plays,
                album_count: slices.len() as i64,
                albums: named,
                rest_plays: on_records - named_plays,
                no_album: a.no_album,
            }
        })
        .collect();

    // ── Anchor songs ──
    let mut anchors: Vec<Anchor> = ranked
        .iter()
        .filter(|(_, a)| a.plays >= ANCHOR_MIN_PLAYS)
        .filter_map(|(_, a)| {
            let (title, n) =
                a.titles.values().max_by(|x, y| x.1.cmp(&y.1).then_with(|| y.0.cmp(&x.0)))?;
            let share = *n as f64 / a.plays as f64;
            (share >= ANCHOR_MIN_SHARE).then(|| Anchor {
                name: a.name.clone(),
                plays: a.plays,
                title: title.clone(),
                title_plays: *n,
                share,
                tracks: a.titles.len() as i64,
            })
        })
        .collect();
    anchors.sort_by(|a, b| {
        b.share
            .total_cmp(&a.share)
            .then_with(|| b.plays.cmp(&a.plays))
            .then_with(|| a.name.cmp(&b.name))
    });
    anchors.truncate(ANCHOR_ARTISTS);

    StatsGraphs {
        range: range.to_string(),
        timezone: tz.name().to_string(),
        capped,
        start_day: start.filter(|_| axis_len > 0).map(day_label),
        daily,
        volume: Volume { plays, active_days: active.len() as i64, median_active_day, peak },
        waves,
        long_tail,
        sessions,
        first_listens,
        habits,
        albums,
        anchors,
    }
}

// ── Reports: one period's window, and who topped each part of it ────────────

/// Who had the most listens in one part of a report's period: a week of a
/// month, or a month of a year.
#[derive(Debug, Serialize, utoipa::ToSchema, PartialEq)]
pub struct Throne {
    /// The part's first day, `YYYY-MM-DD`, local.
    pub start: String,
    /// `Jan`, or `1–7 Aug` — clipped to the period, so a month's first week
    /// starts on the 1st.
    pub label: String,
    /// Whoever was credited on the most listens. Absent for a part with none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    /// Their credited listens in that part.
    pub plays: i64,
    /// Every listen in that part — the population `plays` is part of.
    pub listens: i64,
}

/// How a period splits for its throne.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Parts {
    /// Monday-start weeks, clipped to the period — for a month.
    Weeks,
    /// Calendar months — for a year.
    Months,
}

const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Local midnight at the start of `d`, as unix seconds.
pub fn day_start(d: NaiveDate, tz: Tz) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .and_then(|n| tz.from_local_datetime(&n).earliest())
        .map(|t| t.timestamp())
        // A zone that skips midnight: the first instant of that day.
        .unwrap_or_else(|| {
            crate::analytics::local_day_start(day_start(d, chrono_tz::UTC) + 43_200, tz)
        })
}

/// The local `[start, end)` of a report period from its key — `2026`,
/// `2026-08` or `2026-W31`, the shapes `Period::key` takes. None for a key
/// that names no such period.
pub fn period_bounds(key: &str, tz: Tz) -> Option<(i64, i64)> {
    let (first, next) = if let Some((y, w)) = key.split_once("-W") {
        let monday =
            NaiveDate::from_isoywd_opt(y.parse().ok()?, w.parse().ok()?, chrono::Weekday::Mon)?;
        (monday, monday + chrono::Duration::days(7))
    } else if let Some((y, m)) = key.split_once('-') {
        let first = NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, 1)?;
        (first, first.checked_add_months(chrono::Months::new(1))?)
    } else {
        let y: i32 = key.parse().ok()?;
        (NaiveDate::from_ymd_opt(y, 1, 1)?, NaiveDate::from_ymd_opt(y + 1, 1, 1)?)
    };
    Some((day_start(first, tz), day_start(next, tz)))
}

/// Who topped each part of `[start, before)`, oldest first, up to today —
/// a running period has no throne for parts that haven't happened.
///
/// Counted over everyone credited, like every other artist figure; a part's
/// `listens` counts each listen once, so `plays` can never exceed it.
pub fn throne(
    rows: &[GraphRow],
    start: i64,
    before: i64,
    now: i64,
    tz: Tz,
    parts: Parts,
) -> Vec<Throne> {
    let first = NaiveDate::from_num_days_from_ce_opt(day_of(start, tz));
    let last = NaiveDate::from_num_days_from_ce_opt(day_of(before - 1, tz).min(day_of(now, tz)));
    let (Some(first), Some(last)) = (first, last) else { return Vec::new() };
    if last < first {
        return Vec::new();
    }
    let part_of = |d: NaiveDate| -> NaiveDate {
        match parts {
            Parts::Months => NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap_or(d),
            Parts::Weeks => {
                let monday = d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64);
                monday.max(first)
            }
        }
    };

    // Every part from the first to the last, empty ones included.
    let mut order: Vec<NaiveDate> = Vec::new();
    let mut d = first;
    while d <= last {
        let p = part_of(d);
        if order.last() != Some(&p) {
            order.push(p);
        }
        d += chrono::Duration::days(1);
    }
    // Per part: every listen, and credited plays per artist key → (plays, name).
    type Tally = (i64, HashMap<String, (i64, String)>);
    let mut tallies: HashMap<NaiveDate, Tally> = HashMap::new();
    for r in rows.iter().filter(|r| r.timestamp >= start && r.timestamp < before) {
        let Some(day) = NaiveDate::from_num_days_from_ce_opt(day_of(r.timestamp, tz)) else {
            continue;
        };
        let t = tallies.entry(part_of(day)).or_default();
        t.0 += 1;
        let p = performer(&r.artist, r.artist_credit.as_deref());
        for (key, name) in credited(p, r.artist_credit.as_deref()) {
            let e = t.1.entry(key).or_insert_with(|| (0, name.to_string()));
            e.0 += 1;
            e.1 = name.to_string();
        }
    }

    order
        .into_iter()
        .map(|p| {
            let (listens, artists) = tallies.remove(&p).unwrap_or_default();
            let top =
                artists.into_values().max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
            let label = match parts {
                Parts::Months => MONTHS[p.month0() as usize].to_string(),
                Parts::Weeks => {
                    let sunday =
                        p + chrono::Duration::days(6 - p.weekday().num_days_from_monday() as i64);
                    // Clipped at the period's last day (or today, while it runs).
                    let end = sunday.min(last);
                    if end == p {
                        format!("{} {}", p.day(), MONTHS[p.month0() as usize])
                    } else {
                        format!("{}–{} {}", p.day(), end.day(), MONTHS[p.month0() as usize])
                    }
                }
            };
            Throne {
                start: p.to_string(),
                label,
                plays: top.as_ref().map_or(0, |t| t.0),
                artist: top.map(|t| t.1),
                listens,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;
    /// 2026-01-05 12:00 UTC, a Monday.
    const T0: i64 = 1_767_614_400;

    fn row(ts: i64, artist: &str, title: &str, album: Option<&str>) -> GraphRow {
        GraphRow {
            timestamp: ts,
            artist: artist.into(),
            artist_credit: None,
            title: title.into(),
            album: album.map(Into::into),
            album_filed_under: Some(artist.into()),
        }
    }

    fn credit(mut r: GraphRow, c: &str) -> GraphRow {
        r.artist_credit = Some(c.into());
        r
    }

    fn run(rows: &[GraphRow], after: Option<i64>, now: i64) -> StatsGraphs {
        compute(rows, "all", after, None, now, chrono_tz::UTC, false)
    }

    #[test]
    fn every_credited_artist_counts_and_the_placeholder_never_does() {
        let rows = vec![
            credit(
                row(T0, "Harris Jayaraj", "Paartha Mudhal", None),
                "Harris Jayaraj; Unni Menon; Bombay Jayashri",
            ),
            credit(row(T0 + 60, "Various Artists", "Song", Some("Hits")), "Ava Max"),
        ];
        let g = run(&rows, None, T0 + 120);
        let names: Vec<&str> = g.waves.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(names.contains(&"Unni Menon") && names.contains(&"Ava Max"));
        assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("various artists")));
        // Credits, not listens: two listens, four credits.
        assert_eq!(g.volume.plays, 2);
        assert_eq!(g.long_tail.credits, 4);
    }

    #[test]
    fn days_are_the_users_days() {
        // 23:30 UTC on Monday is 00:30 Tuesday in Berlin.
        let late = T0 + 11 * 3600 + 1800;
        let rows = vec![row(T0, "A", "x", None), row(late, "A", "y", None)];
        let utc = compute(&rows, "all", None, None, late, chrono_tz::UTC, false);
        let berlin = compute(&rows, "all", None, None, late, chrono_tz::Europe::Berlin, false);
        assert_eq!(utc.daily, vec![2]);
        assert_eq!(berlin.daily, vec![1, 1]);
        assert_eq!(berlin.timezone, "Europe/Berlin");
    }

    #[test]
    fn the_axis_runs_to_today_with_empty_days_included() {
        let rows = vec![row(T0, "A", "x", None), row(T0 + 3 * DAY, "A", "x", None)];
        let g = run(&rows, None, T0 + 5 * DAY);
        assert_eq!(g.start_day.as_deref(), Some("2026-01-05"));
        assert_eq!(g.daily, vec![1, 0, 0, 1, 0, 0]);
        assert_eq!(g.volume.active_days, 2);
        assert_eq!(g.volume.median_active_day, 1.0);
        assert_eq!(g.volume.peak.as_ref().map(|p| p.day.as_str()), Some("2026-01-05"));
    }

    #[test]
    fn a_range_narrows_everything_but_the_first_listen() {
        let rows =
            vec![row(T0 - 400 * DAY, "Old Friend", "x", None), row(T0, "Old Friend", "x", None)];
        let g = run(&rows, Some(T0 - DAY), T0);
        assert_eq!(g.volume.plays, 1);
        assert_eq!(g.daily.len(), 2);
        assert_eq!(g.first_listens[0].first, T0 - 400 * DAY, "first listen is all-time");
        assert_eq!(g.first_listens[0].plays, 1, "plays are ranged");
    }

    #[test]
    fn long_tail_reach_counts_artists_to_the_threshold() {
        let mut rows = Vec::new();
        for i in 0..6 {
            rows.push(row(T0 + i, "Big", "x", None));
        }
        for (i, a) in ["B", "C", "D", "E"].iter().enumerate() {
            rows.push(row(T0 + 100 + i as i64, a, "x", None));
        }
        let g = run(&rows, None, T0 + 200);
        assert_eq!(g.long_tail.counts, vec![6, 1, 1, 1, 1]);
        assert_eq!(g.long_tail.half, 1, "6 of 10 already passes half");
        assert_eq!(g.long_tail.eighty, 3);
        assert_eq!(g.long_tail.once, 4);
    }

    #[test]
    fn a_long_gap_starts_a_new_session_and_short_ones_skip_devotion() {
        let gap = crate::db::SESSION_GAP_SECS;
        let mut rows = vec![
            row(T0, "A", "x", None),
            row(T0 + 60, "A", "y", None),
            row(T0 + 120, "A", "z", None),
        ];
        // A lone listen after a long gap: its own session, too short to judge.
        rows.push(row(T0 + 120 + gap + 1, "B", "x", None));
        // Then a scattered one.
        let s = T0 + 10 * gap;
        for (i, a) in ["C", "D", "E", "F"].iter().enumerate() {
            rows.push(row(s + i as i64 * 60, a, "x", None));
        }
        let g = run(&rows, None, s + 600);
        assert_eq!(g.sessions.total, 3);
        let by: HashMap<&str, i64> =
            g.sessions.lengths.iter().map(|b| (b.label.as_str(), b.sessions)).collect();
        assert_eq!((by["1"], by["3–4"]), (1, 2));
        assert_eq!((g.sessions.devotion.devoted, g.sessions.devotion.scattered), (1, 1));
    }

    #[test]
    fn habits_measure_return_rate_and_the_biggest_day() {
        let mut rows = Vec::new();
        // Burst: eight plays on one day, then one a week later.
        for i in 0..8 {
            rows.push(row(T0 + i * 60, "Burst", "x", None));
        }
        rows.push(row(T0 + 7 * DAY, "Burst", "x", None));
        // One day only: no pattern to place.
        for i in 0..6 {
            rows.push(row(T0 + 2 * DAY + i * 60, "One Day", "x", None));
        }
        let g = run(&rows, None, T0 + 7 * DAY);
        assert_eq!(g.habits.len(), 1, "a one-day artist has no habit to show");
        let h = &g.habits[0];
        assert_eq!((h.active_days, h.span_days), (2, 8));
        assert!((h.top_day_share - 8.0 / 9.0).abs() < 1e-9);
        assert_eq!(h.top_day, "2026-01-05");
    }

    #[test]
    fn anchors_need_enough_plays_and_a_dominant_song() {
        let mut rows = Vec::new();
        for i in 0..20 {
            rows.push(row(T0 + i, "Anchored", if i < 18 { "The One" } else { "Other" }, None));
            rows.push(row(T0 + 100 + i, "Spread", &format!("song {}", i % 10), None));
        }
        for i in 0..5 {
            rows.push(row(T0 + 300 + i, "Few", "Only", None));
        }
        let g = run(&rows, None, T0 + 400);
        assert_eq!(g.anchors.len(), 1, "{:?}", g.anchors);
        assert_eq!(g.anchors[0].title, "The One");
        assert_eq!((g.anchors[0].title_plays, g.anchors[0].tracks), (18, 2));
    }

    #[test]
    fn album_spread_folds_the_rest_and_counts_unnamed_records() {
        let mut rows = Vec::new();
        for (i, al) in ["A", "A", "A", "B", "B", "C", "D", "E", "F", "G"].iter().enumerate() {
            rows.push(row(T0 + i as i64, "X", "t", Some(al)));
        }
        rows.push(row(T0 + 50, "X", "t", None));
        let g = run(&rows, None, T0 + 100);
        let a = &g.albums[0];
        assert_eq!(a.album_count, 7);
        assert_eq!(a.albums.len(), ALBUM_SLICES);
        assert_eq!(a.albums[0].name, "A");
        assert_eq!((a.rest_plays, a.no_album), (2, 1));
        assert_eq!(
            a.albums.iter().map(|s| s.plays).sum::<i64>() + a.rest_plays + a.no_album,
            a.plays
        );
    }

    #[test]
    fn an_empty_history_is_empty_not_an_error() {
        let g = run(&[], None, T0);
        assert!(g.start_day.is_none() && g.daily.is_empty() && g.volume.peak.is_none());
        assert_eq!(g.long_tail.half, 0);
    }

    #[test]
    fn period_keys_are_local_calendar_windows() {
        let berlin = chrono_tz::Europe::Berlin;
        // 1 August 2026 00:00 CEST is 31 July 22:00 UTC.
        let (a, b) = period_bounds("2026-08", berlin).unwrap();
        assert_eq!((a, b), (1_785_535_200, 1_788_213_600));
        let (w0, w1) = period_bounds("2026-W31", chrono_tz::UTC).unwrap();
        assert_eq!(
            (day_label(day_of(w0, chrono_tz::UTC)), w1 - w0),
            ("2026-07-27".into(), 7 * DAY)
        );
        let (y0, _) = period_bounds("2026", chrono_tz::UTC).unwrap();
        assert_eq!(day_label(day_of(y0, chrono_tz::UTC)), "2026-01-01");
        for bad in ["2026-13", "2026-W60", "twenty", ""] {
            assert!(period_bounds(bad, chrono_tz::UTC).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_report_window_stops_at_its_last_day() {
        let (start, before) = period_bounds("2026-01", chrono_tz::UTC).unwrap();
        let rows = vec![
            row(start - DAY, "A", "x", None),
            row(start, "A", "x", None),
            row(before, "A", "x", None),
        ];
        let g = compute(
            &rows,
            "month",
            Some(start),
            Some(before),
            before + 40 * DAY,
            chrono_tz::UTC,
            false,
        );
        assert_eq!((g.volume.plays, g.daily.len()), (1, 31));
        // A running period stops at today instead.
        let g = compute(
            &rows,
            "month",
            Some(start),
            Some(before),
            start + 9 * DAY,
            chrono_tz::UTC,
            false,
        );
        assert_eq!(g.daily.len(), 10);
    }

    #[test]
    fn a_years_throne_is_every_month_with_empty_ones_kept() {
        let (start, before) = period_bounds("2026", chrono_tz::UTC).unwrap();
        let mar = start + 70 * DAY;
        let rows = vec![
            row(start + DAY, "Dua Lipa", "x", None),
            row(start + DAY + 60, "Dua Lipa", "y", None),
            credit(row(start + DAY + 120, "Madonna", "z", None), "Madonna; Dua Lipa"),
            row(mar, "Michael Jackson", "x", None),
        ];
        let t = throne(&rows, start, before, start + 80 * DAY, chrono_tz::UTC, Parts::Months);
        let got: Vec<(&str, Option<&str>, i64, i64)> =
            t.iter().map(|p| (p.label.as_str(), p.artist.as_deref(), p.plays, p.listens)).collect();
        assert_eq!(
            got,
            vec![
                ("Jan", Some("Dua Lipa"), 3, 3),
                ("Feb", None, 0, 0),
                ("Mar", Some("Michael Jackson"), 1, 1)
            ],
            "credits count, listens count once, months after today are not there"
        );
    }

    #[test]
    fn a_months_weeks_are_clipped_to_the_month() {
        // August 2026 starts on a Saturday and ends on a Monday.
        let (start, before) = period_bounds("2026-08", chrono_tz::UTC).unwrap();
        let t = throne(&[], start, before, before + DAY, chrono_tz::UTC, Parts::Weeks);
        let labels: Vec<&str> = t.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["1–2 Aug", "3–9 Aug", "10–16 Aug", "17–23 Aug", "24–30 Aug", "31 Aug"]
        );
        assert_eq!(t[0].start, "2026-08-01");
    }
}
