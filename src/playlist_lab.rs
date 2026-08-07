//! The vector space the Playlist Lab moves through.
//!
//! ## What this is, and what it is not
//!
//! The design this was built from (`temp/Tapedeck-PlaylistLabs.dc.html`) labels
//! its ten axes Energy, Valence, Tempo, Acoustic, Danceable, Instrumental,
//! Warmth, Brightness, Density and Dynamics — Spotify's audio-feature
//! vocabulary — and calls the result "a 10-dimensional map of how they actually
//! sound". **Tapedeck has none of those numbers and cannot derive them.** In the
//! mockup, `plVec()` hashes the artist and album names through `sin()` to
//! produce plausible-looking values; that is honest work for a mockup and would
//! be a lie shipped, since the playlist would be ordered by a hash of somebody's
//! name while claiming to be ordered by how the music sounds.
//!
//! So the axes here are the ones that really exist, and they come from two
//! different places — which is the single most important thing to understand
//! about this module:
//!
//! * **Six are about the music** (`Group::Sound`), derived from Every Noise via
//!   the track's genres. Glenn McDonald's map is itself a dimensionality
//!   reduction of audio character, and `parse.rs` already documents what the
//!   axes mean: `x` runs dense/atmospheric to spiky/bouncy, `y` organic to
//!   mechanical/electric, and the colour's three channels encode audio
//!   character too (which is why `blend` exists at all). The sixth is how far
//!   apart a track's own genres sit.
//! * **Four are about the listener** (`Group::Habit`) — era, familiarity, when
//!   in the day it gets played, how lately. Real, from the user's own history,
//!   and *not* facts about the sound.
//!
//! Mixing the two in one distance is a deliberate choice made with Ashwin on
//! 2026-08-07, and it has a failure mode that must stay visible: without
//! weighting, "similar songs" quietly starts meaning "songs I play at the same
//! hour". That is what `sound_weight` is for, and why the UI names the two
//! groups rather than presenting ten interchangeable axes.
//!
//! ## The granularity ceiling, which cannot be engineered away
//!
//! Genres attach to **artists**, so every track by one artist shares its six
//! Sound coordinates exactly. This is the same constraint the genre trajectory
//! carries ("an artist-mix trajectory, not a track-level one") and it means
//! nearest-neighbour search in Sound space returns that artist's own tracks
//! first. The Habit axes are the only thing that separates them. Anything
//! describing this as track-level audio similarity is wrong.
//!
//! A real per-recording space — AcousticBrainz, keyed by recording MBID, which
//! 90.4% of a measured history carries — is the agreed next step. Note it
//! cannot be a *fallback chain* like artwork or bios: a distance between a track
//! in AcousticBrainz space and one in genre space is meaningless, so it has to
//! be a whole second space chosen per playlist, never a per-track substitution.

use std::collections::HashMap;

/// Which half of the space an axis belongs to. The distinction is load-bearing
/// — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    /// Derived from Every Noise, i.e. about the music.
    Sound,
    /// Derived from the user's own listening, i.e. about them.
    Habit,
}

pub struct Dim {
    pub key: &'static str,
    /// What the axis is called in a picker.
    pub label: &'static str,
    /// What each end means, so an axis is never an unexplained number.
    pub low: &'static str,
    pub high: &'static str,
    pub group: Group,
}

/// The ten axes, in vector order.
///
/// The three colour channels are named for what they are rather than for a
/// meaning invented here. Every Noise's colour is a real encoding of audio
/// character, but Glenn publishes no per-channel semantics, and making some up
/// would be the same error as a grid-size-dependent breadth metric: a
/// confident-looking way of saying nothing.
pub const DIMS: [Dim; 10] = [
    Dim {
        key: "bouncy",
        label: "Texture",
        low: "Atmospheric",
        high: "Bouncy",
        group: Group::Sound,
    },
    Dim {
        key: "electric",
        label: "Organic ↔ Electric",
        low: "Organic",
        high: "Electric",
        group: Group::Sound,
    },
    Dim { key: "col_r", label: "Colour · red", low: "Low", high: "High", group: Group::Sound },
    Dim { key: "col_g", label: "Colour · green", low: "Low", high: "High", group: Group::Sound },
    Dim { key: "col_b", label: "Colour · blue", low: "Low", high: "High", group: Group::Sound },
    Dim {
        key: "eclectic",
        label: "Breadth",
        low: "One genre",
        high: "Many genres",
        group: Group::Sound,
    },
    Dim { key: "era", label: "Era", low: "Older", high: "Newer", group: Group::Habit },
    Dim {
        key: "familiar",
        label: "Familiarity",
        low: "Barely played",
        high: "Worn out",
        group: Group::Habit,
    },
    Dim {
        key: "nocturnal",
        label: "Hour",
        low: "Daytime",
        high: "After dark",
        group: Group::Habit,
    },
    Dim { key: "recent", label: "Recency", low: "Long ago", high: "Lately", group: Group::Habit },
];

pub const N_DIMS: usize = DIMS.len();

/// One listen, as the pool builder needs it.
#[derive(Debug, Clone)]
pub struct LabRow {
    pub timestamp: i64,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    /// JSON array of genre names, as `scrobbles.genres` stores it.
    pub genres: Option<String>,
    pub release_year: Option<i64>,
}

/// A track in the pool.
#[derive(Debug, Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub plays: i64,
    pub last_played: i64,
    pub vector: [f32; N_DIMS],
}

pub struct Space {
    pub tracks: Vec<Track>,
    /// Tracks dropped because none of their genres has a place on the map.
    ///
    /// Reported rather than hidden, and dropped rather than parked at the
    /// centre: a track with no position is not a track at the middle of the
    /// map, and putting it there would invent a similarity it does not have.
    pub unplaced: usize,
    /// Distinct tracks seen before the pool rules were applied.
    pub considered: usize,
}

/// A genre's place on the map: position and colour, as `coordinates_for` gives.
pub type Coords = HashMap<String, (f64, f64, String)>;

fn hex_channels(hex: &str) -> Option<(f64, f64, f64)> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let ch = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f64);
    match (ch(0), ch(2), ch(4)) {
        (Some(r), Some(g), Some(b)) => Some((r, g, b)),
        _ => None,
    }
}

/// Everything accumulated for one track before the pool is normalised.
#[derive(Default)]
struct Fold {
    title: String,
    artist: String,
    album: Option<String>,
    plays: i64,
    last_played: i64,
    year: Option<i64>,
    after_dark: i64,
    /// Distinct genre positions this track's listens named, and their colours.
    points: Vec<(f64, f64, f64, f64, f64)>,
    seen_genres: Vec<String>,
}

/// Group listens into tracks and place them in the space.
///
/// `now` and `tz` are passed rather than read here so this stays pure and the
/// hour bucketing can be asserted. The timezone matters for the same reason it
/// does everywhere else in this codebase: a listen at 00:30 in CET is after
/// dark on the right day only if the zone is applied.
pub fn build(
    rows: &[LabRow],
    coords: &Coords,
    map_spread: f64,
    tz: chrono_tz::Tz,
    now: i64,
    min_plays: i64,
    keep: &[String],
    cap: usize,
) -> Space {
    use chrono::TimeZone;

    // Keyed the way every grouping query in this codebase is: case-folded and
    // trimmed, so "Xtal" and "xtal " are one track rather than two.
    let key = |artist: &str, title: &str| {
        format!("{}\u{1}{}", artist.trim().to_lowercase(), title.trim().to_lowercase())
    };

    let mut folds: HashMap<String, Fold> = HashMap::new();
    for row in rows {
        if row.title.trim().is_empty() || row.artist.trim().is_empty() {
            continue;
        }
        let f = folds.entry(key(&row.artist, &row.title)).or_insert_with(|| Fold {
            title: row.title.clone(),
            artist: row.artist.clone(),
            ..Default::default()
        });
        f.plays += 1;
        f.last_played = f.last_played.max(row.timestamp);
        if f.album.is_none() {
            f.album = row.album.clone().filter(|a| !a.trim().is_empty());
        }
        // The newest release year wins the same way `MAX(release_year)` does
        // elsewhere — a compilation appearance should not age the track.
        if let Some(y) = row.release_year.filter(|y| *y > 1000) {
            f.year = Some(f.year.map_or(y, |c: i64| c.max(y)));
        }

        let hour = tz.timestamp_opt(row.timestamp, 0).single().map(|d| {
            use chrono::Timelike;
            d.hour()
        });
        if matches!(hour, Some(h) if !(5..22).contains(&h)) {
            f.after_dark += 1;
        }

        // Genres are per listen, but a track's genres barely change between
        // listens — so each distinct name is placed once rather than weighted by
        // how often it recurred, which would just re-weight by play count and
        // double-count what the familiarity axis already carries.
        if let Some(raw) = row.genres.as_deref() {
            if let Ok(names) = serde_json::from_str::<Vec<String>>(raw) {
                for name in names {
                    let k = crate::everynoise::store::genre_key(&name);
                    if k.is_empty() || f.seen_genres.contains(&k) {
                        continue;
                    }
                    f.seen_genres.push(k.clone());
                    if let Some((x, y, colour)) = coords.get(&k) {
                        if let Some((r, g, b)) = hex_channels(colour) {
                            f.points.push((*x, *y, r, g, b));
                        }
                    }
                }
            }
        }
    }

    let considered = folds.len();
    let keep: std::collections::HashSet<String> =
        keep.iter().map(|k| k.to_lowercase()).collect::<std::collections::HashSet<_>>();

    // Raw coordinates first; normalisation needs the pool's own extremes.
    struct Raw {
        f: Fold,
        x: f64,
        y: f64,
        r: f64,
        g: f64,
        b: f64,
        spread: f64,
    }

    let mut unplaced = 0usize;
    let mut raws: Vec<Raw> = Vec::new();
    for (k, f) in folds {
        // A loved track is kept whatever its play count — it is the one thing
        // the user has explicitly said is theirs, and the design starts from
        // exactly those.
        if f.plays < min_plays && !keep.contains(&k) {
            continue;
        }
        if f.points.is_empty() {
            unplaced += 1;
            continue;
        }
        let n = f.points.len() as f64;
        let (mut x, mut y, mut r, mut g, mut b) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (px, py, pr, pg, pb) in &f.points {
            x += px;
            y += py;
            r += pr;
            g += pg;
            b += pb;
        }
        let (x, y) = (x / n, y / n);
        // How far this track's own genres sit from their centre — "one genre"
        // versus "many, and far apart". Zero for a single-genre track by
        // construction, which is the honest answer rather than a missing value.
        let spread = if f.points.len() < 2 {
            0.0
        } else {
            (f.points.iter().map(|(px, py, ..)| (px - x).powi(2) + (py - y).powi(2)).sum::<f64>()
                / n)
                .sqrt()
        };
        raws.push(Raw { f, x, y, r: r / n, g: g / n, b: b / n, spread });
    }

    // The busiest tracks first, then trimmed. A pool is the candidate set for a
    // playlist of a dozen or two, and the long tail of once-played rows is
    // mostly noise — but the cap is generous and loved tracks are never in the
    // tail, because they were kept above.
    raws.sort_by(|a, b| b.f.plays.cmp(&a.f.plays).then_with(|| a.f.title.cmp(&b.f.title)));
    raws.truncate(cap);

    // Normalised against the *pool*, not against absolutes: "newer" means newer
    // than the rest of this collection, which is the only reading that means
    // anything to the person who owns it.
    let range = |vals: &[f64]| -> (f64, f64) {
        let lo = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if !lo.is_finite() || !hi.is_finite() || (hi - lo).abs() < f64::EPSILON {
            (0.0, 1.0)
        } else {
            (lo, hi)
        }
    };
    let norm = |v: f64, (lo, hi): (f64, f64)| ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;

    let xs: Vec<f64> = raws.iter().map(|r| r.x).collect();
    let ys: Vec<f64> = raws.iter().map(|r| r.y).collect();
    let x_range = range(&xs);
    let y_range = range(&ys);
    let years: Vec<f64> =
        raws.iter().filter_map(|r| r.f.year).map(|y| y as f64).collect::<Vec<_>>();
    let year_range = range(&years);
    let plays: Vec<f64> = raws.iter().map(|r| (r.f.plays as f64).ln_1p()).collect();
    let play_range = range(&plays);
    let ages: Vec<f64> = raws.iter().map(|r| (now - r.f.last_played) as f64).collect();
    let age_range = range(&ages);
    // Spread is normalised against the *map's* own spread, like `reach` on the
    // Statistics page, so "broad" means broad relative to the whole map rather
    // than to whichever tracks happen to be in the pool.
    let spread_ref = if map_spread > 0.0 { map_spread } else { 1.0 };

    let tracks = raws
        .into_iter()
        .map(|r| {
            let mut v = [0.0f32; N_DIMS];
            v[0] = norm(r.x, x_range);
            v[1] = norm(r.y, y_range);
            v[2] = (r.r / 255.0) as f32;
            v[3] = (r.g / 255.0) as f32;
            v[4] = (r.b / 255.0) as f32;
            v[5] = (r.spread / spread_ref).clamp(0.0, 1.0) as f32;
            // No release year is the middle rather than the oldest: unknown is
            // not "1900", and putting it at an extreme would drag every
            // un-enriched track to one end of the axis.
            v[6] = r.f.year.map_or(0.5, |y| norm(y as f64, year_range));
            v[7] = norm((r.f.plays as f64).ln_1p(), play_range);
            v[8] = (r.f.after_dark as f32 / r.f.plays.max(1) as f32).clamp(0.0, 1.0);
            // Inverted: the axis reads "long ago → lately", so a small age is a
            // high value.
            v[9] = 1.0 - norm((now - r.f.last_played) as f64, age_range);
            Track {
                title: r.f.title,
                artist: r.f.artist,
                album: r.f.album,
                plays: r.f.plays,
                last_played: r.f.last_played,
                vector: v,
            }
        })
        .collect();

    Space { tracks, unplaced, considered }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, artist: &str, ts: i64, genres: &[&str]) -> LabRow {
        LabRow {
            timestamp: ts,
            artist: artist.into(),
            title: title.into(),
            album: Some("An Album".into()),
            genres: Some(serde_json::to_string(genres).unwrap()),
            release_year: Some(2010),
        }
    }

    fn coords() -> Coords {
        let mut c = Coords::new();
        c.insert("ambient".into(), (100.0, 200.0, "#204080".to_string()));
        c.insert("techno".into(), (900.0, 1800.0, "#c08040".to_string()));
        c
    }

    const UTC: chrono_tz::Tz = chrono_tz::UTC;

    #[test]
    fn a_track_with_no_placeable_genre_is_dropped_and_counted() {
        // The alternative — parking it at the centre — would invent a
        // similarity it does not have.
        let rows = vec![
            row("Placed", "A", 1_700_000_000, &["ambient"]),
            row("Placed", "A", 1_700_000_100, &["ambient"]),
            row("Nowhere", "B", 1_700_000_000, &["saxophone"]),
            row("Nowhere", "B", 1_700_000_100, &["saxophone"]),
        ];
        let s = build(&rows, &coords(), 1000.0, UTC, 1_700_100_000, 2, &[], 100);
        assert_eq!(s.tracks.len(), 1);
        assert_eq!(s.tracks[0].title, "Placed");
        assert_eq!(s.unplaced, 1);
        assert_eq!(s.considered, 2);
    }

    #[test]
    fn a_loved_track_survives_the_play_threshold() {
        let rows = vec![
            row("Once", "A", 1_700_000_000, &["ambient"]),
            row("Twice", "A", 1_700_000_000, &["ambient"]),
            row("Twice", "A", 1_700_000_100, &["ambient"]),
        ];
        // Without the keep list, a single play is below the threshold.
        let bare = build(&rows, &coords(), 1000.0, UTC, 1_700_100_000, 2, &[], 100);
        assert_eq!(bare.tracks.len(), 1);

        let keep = vec!["a\u{1}once".to_string()];
        let kept = build(&rows, &coords(), 1000.0, UTC, 1_700_100_000, 2, &keep, 100);
        assert_eq!(kept.tracks.len(), 2, "a loved track is kept however little it was played");
    }

    #[test]
    fn every_track_by_one_artist_shares_its_sound_coordinates() {
        // The granularity ceiling, pinned. If this ever stops being true it is
        // because a real per-recording space landed, and that is a deliberate
        // change rather than a drift.
        let rows = vec![
            row("One", "A", 1_700_000_000, &["ambient"]),
            row("One", "A", 1_700_000_100, &["ambient"]),
            row("Two", "A", 1_700_000_200, &["ambient"]),
            row("Two", "A", 1_700_000_300, &["ambient"]),
        ];
        let s = build(&rows, &coords(), 1000.0, UTC, 1_700_100_000, 2, &[], 100);
        assert_eq!(s.tracks.len(), 2);
        let sound = |t: &Track| t.vector[..6].to_vec();
        assert_eq!(sound(&s.tracks[0]), sound(&s.tracks[1]));
    }

    #[test]
    fn a_two_genre_track_reads_broader_than_a_one_genre_track() {
        let rows = vec![
            row("Narrow", "A", 1_700_000_000, &["ambient"]),
            row("Narrow", "A", 1_700_000_100, &["ambient"]),
            row("Broad", "B", 1_700_000_000, &["ambient", "techno"]),
            row("Broad", "B", 1_700_000_100, &["ambient", "techno"]),
        ];
        let s = build(&rows, &coords(), 2000.0, UTC, 1_700_100_000, 2, &[], 100);
        let get = |t: &str| s.tracks.iter().find(|x| x.title == t).unwrap().vector[5];
        assert_eq!(get("Narrow"), 0.0, "one genre has no spread, which is the honest answer");
        assert!(get("Broad") > 0.0);
    }

    #[test]
    fn the_hour_axis_is_read_in_the_users_timezone() {
        // 22:10 UTC is after dark, and so is 23:10 in Berlin — that one agrees
        // either way. 04:30 UTC is still after dark, but it is 05:30 in Berlin,
        // which is morning. So the same two listens give a different answer in
        // the two zones, which is the whole point: the same bug the dashboard's
        // "today" had, in a different readout.
        let late = 1_699_999_800; // 2023-11-14T22:10:00Z → 23:10 Berlin
        let early = 1_700_022_600; // 2023-11-15T04:30:00Z → 05:30 Berlin

        let rows = vec![
            LabRow { timestamp: late, ..row("T", "A", late, &["ambient"]) },
            LabRow { timestamp: early, ..row("T", "A", early, &["ambient"]) },
        ];

        let utc = build(&rows, &coords(), 1000.0, UTC, 1_700_100_000, 2, &[], 100);
        assert_eq!(utc.tracks[0].vector[8], 1.0, "22:10 and 04:30 UTC are both after dark");

        let berlin =
            build(&rows, &coords(), 1000.0, chrono_tz::Europe::Berlin, 1_700_100_000, 2, &[], 100);
        assert_eq!(
            berlin.tracks[0].vector[8], 0.5,
            "05:30 in Berlin is morning, so only one of the two counts"
        );
    }

    #[test]
    fn an_unknown_release_year_sits_in_the_middle_rather_than_at_an_extreme() {
        let mut old = row("Old", "A", 1_700_000_000, &["ambient"]);
        old.release_year = Some(1970);
        let mut new = row("New", "B", 1_700_000_000, &["techno"]);
        new.release_year = Some(2020);
        let mut unknown = row("Unknown", "C", 1_700_000_000, &["ambient"]);
        unknown.release_year = None;

        let rows: Vec<LabRow> = [old, new, unknown]
            .into_iter()
            .flat_map(|r| {
                let mut second = r.clone();
                second.timestamp += 100;
                [r, second]
            })
            .collect();

        let s = build(&rows, &coords(), 1000.0, UTC, 1_700_100_000, 2, &[], 100);
        let era = |t: &str| s.tracks.iter().find(|x| x.title == t).unwrap().vector[6];
        assert_eq!(era("Old"), 0.0);
        assert_eq!(era("New"), 1.0);
        assert_eq!(era("Unknown"), 0.5, "unknown is not 1900");
    }

    #[test]
    fn the_pool_is_capped_by_plays_and_keeps_the_busiest() {
        let mut rows = Vec::new();
        for i in 0..5 {
            // Track i gets i + 2 plays, so "T4" is the busiest.
            for p in 0..(i + 2) {
                rows.push(row(
                    &format!("T{i}"),
                    "A",
                    1_700_000_000 + (i * 1000 + p) as i64,
                    &["ambient"],
                ));
            }
        }
        let s = build(&rows, &coords(), 1000.0, UTC, 1_700_100_000, 2, &[], 2);
        assert_eq!(s.tracks.len(), 2);
        assert_eq!(s.tracks[0].title, "T4");
        assert_eq!(s.tracks[1].title, "T3");
        assert_eq!(s.considered, 5, "considered counts before the cap");
    }

    #[test]
    fn the_two_groups_are_declared_and_add_up() {
        // The UI weights by group, so a dimension added without a group would
        // silently fall out of the weighting.
        assert_eq!(DIMS.iter().filter(|d| d.group == Group::Sound).count(), 6);
        assert_eq!(DIMS.iter().filter(|d| d.group == Group::Habit).count(), 4);
        assert_eq!(DIMS.len(), N_DIMS);
    }
}
