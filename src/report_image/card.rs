//! The facts a share image draws, lifted out of a report period.
//!
//! Typed rather than a `serde_json::Value` poked at from the drawing code, for
//! two reasons: the extraction happens in one place instead of a dozen, and the
//! composition becomes testable without a database behind it.
//!
//! **Nothing is computed here.** Every number comes from `reports()`, which the
//! image endpoint calls the way the MCP tools call handlers — so there is one
//! implementation of the fold and no second copy to drift from the page. A
//! share image that disagreed with the chapter it was shared from would be
//! worse than no share image.

use serde_json::Value;

/// A name and how many plays it got.
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub name: String,
    pub plays: i64,
}

/// A track or record: what it is called, who made it, how often it played.
#[derive(Clone, Debug, PartialEq)]
pub struct Titled {
    pub title: String,
    pub artist: String,
    pub plays: i64,
}

/// Everything a card can draw. Which of it actually gets drawn is the size's
/// business — see `month`.
#[derive(Clone, Debug, Default)]
pub struct Card {
    pub label: String,
    pub plays: i64,
    pub hours: f64,
    pub avg_daily: i64,
    pub rank: i64,
    pub rank_of: i64,
    pub artists: i64,
    pub albums: i64,
    pub tracks: i64,
    /// The period's blended colour, straight from the report. Validated before
    /// it is painted — see `theme::period_colour`.
    pub colour: Option<String>,
    pub top_artist: Option<Named>,
    pub top_album: Option<Titled>,
    /// Name and share, biggest first.
    pub genres: Vec<(String, i64)>,
    pub on_repeat: Vec<Titled>,
    pub crush: Option<Named>,
    pub top_chain: Option<String>,

    // ── What a week and a year draw, and a month does not ──
    /// The track of the period. The week chapter leads with this where the
    /// month leads with a record, which is the design's own call: seven days
    /// often do not contain a record played end to end.
    pub top_track: Option<Titled>,
    /// Plays per sub-bucket — a week's seven days, a year's twelve months.
    /// **The chart a week and a year are actually shared for.**
    pub sub: Vec<i64>,
    pub sub_labels: Vec<String>,
    /// Artists heard for the first time. A year's headline discovery figure.
    pub new_artists: i64,
    /// Sides played off the shelf. Zero for anyone without one, and the stat
    /// row drops a zero rather than drawing it.
    pub sides: i64,
    /// The three biggest genres by name, and their share in each sub-bucket.
    /// Tracked by *name* across the whole period — see `layout::bands`.
    pub genre_bands: Option<(Vec<String>, Vec<Vec<f64>>)>,
    /// Listens marked as heard with somebody, and the population that is a
    /// count *of*. **Both travel or neither does**: `company` is null on
    /// everything nobody has tagged, so a bare share would report how the
    /// listening was *recorded* while appearing to report how it happened.
    pub company: Option<(i64, i64)>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)?.as_str().map(str::to_string)
}

fn i(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

fn nums(v: Option<&Value>) -> Vec<i64> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(|n| n.as_i64().unwrap_or(0)).collect())
        .unwrap_or_default()
}

/// The genre bands, refused unless both halves are there and agree.
///
/// A names list with no shares would draw a key over nothing, and shares with
/// no names would draw stripes nobody can read. Either alone is an absence
/// rather than half a chart — the same call every other section here makes.
fn bands(v: Option<&Value>) -> Option<(Vec<String>, Vec<Vec<f64>>)> {
    let v = v?;
    let names: Vec<String> =
        v.get("names")?.as_array()?.iter().filter_map(|n| n.as_str().map(str::to_string)).collect();
    let shares: Vec<Vec<f64>> = v
        .get("shares")?
        .as_array()?
        .iter()
        .map(|col| {
            col.as_array()
                .map(|a| a.iter().map(|n| n.as_f64().unwrap_or(0.0)).collect())
                .unwrap_or_default()
        })
        .collect();
    (!names.is_empty() && shares.len() >= 2).then_some((names, shares))
}

fn named(v: Option<&Value>) -> Option<Named> {
    let v = v?;
    Some(Named { name: s(v, "name")?, plays: i(v, "plays") })
}

/// A track or record, which the report spells two ways: `top_album` carries
/// `name` while `on_repeat` carries `title`. Both are accepted here rather than
/// at each call site.
fn titled(v: Option<&Value>) -> Option<Titled> {
    let v = v?;
    let title = s(v, "title").or_else(|| s(v, "name"))?;
    Some(Titled { title, artist: s(v, "artist").unwrap_or_default(), plays: i(v, "plays") })
}

impl Card {
    /// Reads one element of the report's `periods` array.
    ///
    /// Every field is optional in the sense that matters: a period with no
    /// albums, no genres and no chain is an ordinary early month, not a broken
    /// payload, so a missing key is an absence and never an error. The card
    /// leaves the section out rather than drawing a heading over nothing.
    pub fn from_period(p: &Value) -> Card {
        Card {
            label: s(p, "label").unwrap_or_default(),
            plays: i(p, "plays"),
            hours: p.get("hours").and_then(Value::as_f64).unwrap_or(0.0),
            avg_daily: i(p, "avg_daily"),
            rank: i(p, "rank"),
            rank_of: i(p, "rank_of"),
            artists: i(p, "artists"),
            albums: i(p, "albums"),
            tracks: i(p, "tracks"),
            colour: s(p, "colour"),
            top_artist: named(p.get("top_artist")),
            top_album: titled(p.get("top_album")),
            genres: p
                .get("genres")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|g| Some((s(g, "name")?, i(g, "pct"))))
                        .filter(|(_, pct)| *pct > 0)
                        .collect()
                })
                .unwrap_or_default(),
            on_repeat: p
                .get("on_repeat")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|r| titled(Some(r))).collect())
                .unwrap_or_default(),
            crush: named(p.get("crush")),
            top_chain: s(p, "top_chain").filter(|c| !c.trim().is_empty()),
            top_track: titled(p.get("top_track")),
            sub: nums(p.get("sub")),
            sub_labels: p
                .get("sub_labels")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            new_artists: i(p, "new_artists"),
            sides: p.get("shelf").map(|sh| i(sh, "sides")).unwrap_or(0),
            genre_bands: bands(p.get("genre_bands")),
            company: p.get("company").and_then(|co| {
                let marked = i(co, "marked_plays");
                (marked > 0).then(|| (marked, i(co, "of_plays")))
            }),
        }
    }

    /// The letter that stands in for a picture.
    ///
    /// The report payload carries **no artwork** and that is not an omission to
    /// work around: portraits exist only for artists found in the user's own
    /// library, and Reports already decided the giant initial is the *design*
    /// rather than a placeholder for a photo that never arrives. Drawing it
    /// here keeps the share consistent with the chapter and keeps the render
    /// free of any network call.
    pub fn initial(name: &str) -> String {
        name.chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "·".to_string())
    }
}

/// One full card per unit, for the tests and for the images the ignored
/// `render_the_cards_to_look_at` writes out.
///
/// **One fixture rather than one per test module.** The pictures somebody looks
/// at and the cards the guards assert on have to be the same cards, or the
/// thing that was eyeballed is not the thing that is protected.
#[cfg(test)]
impl Card {
    pub(crate) fn sample(unit: super::layout::Unit) -> Card {
        use super::layout::Unit;
        let base = Card {
            colour: Some("#a18430".into()),
            top_artist: Some(Named { name: "Cœur de pirate".into(), plays: 612 }),
            top_album: Some(Titled {
                title: "Blonde".into(),
                artist: "Cœur de pirate".into(),
                plays: 96,
            }),
            top_track: Some(Titled {
                title: "Comme des enfants".into(),
                artist: "Cœur de pirate".into(),
                plays: 41,
            }),
            crush: Some(Named { name: "Clara Luciani".into(), plays: 63 }),
            top_chain: Some("Desk2 · HD 650".into()),
            ..Default::default()
        };
        match unit {
            Unit::Week => Card {
                label: "Week of 17 August".into(),
                plays: 312,
                hours: 16.4,
                avg_daily: 45,
                rank: 3,
                rank_of: 61,
                artists: 48,
                albums: 61,
                tracks: 190,
                sub: vec![41, 62, 18, 0, 77, 58, 56],
                sub_labels: ["M", "T", "W", "T", "F", "S", "S"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                top_track: Some(Titled {
                    title: "Comme des enfants".into(),
                    artist: "Cœur de pirate".into(),
                    plays: 9,
                }),
                // A week's top artist cannot out-play the week. The fixture is
                // what the pictures are eyeballed against, so an incoherent one
                // teaches the wrong lesson about the layout.
                top_artist: Some(Named { name: "Cœur de pirate".into(), plays: 44 }),
                crush: Some(Named { name: "Clara Luciani".into(), plays: 21 }),
                ..base
            },
            Unit::Month => Card {
                label: "August 2026".into(),
                plays: 4182,
                hours: 214.6,
                avg_daily: 135,
                rank: 2,
                rank_of: 14,
                artists: 214,
                albums: 318,
                tracks: 902,
                genres: vec![
                    ("dance-pop".into(), 41),
                    ("chanson".into(), 22),
                    ("french pop".into(), 14),
                    ("indie poptimism".into(), 8),
                ],
                on_repeat: vec![
                    Titled {
                        title: "Comme des enfants".into(),
                        artist: "Cœur de pirate".into(),
                        plays: 9,
                    },
                    Titled {
                        title: "Ne m'appelle pas Fanny".into(),
                        artist: "Clara Luciani".into(),
                        plays: 7,
                    },
                    Titled {
                        title: "Place de la République".into(),
                        artist: "Cœur de pirate".into(),
                        plays: 6,
                    },
                ],
                sub: vec![980, 1140, 760, 1002, 300],
                sub_labels: ["W1", "W2", "W3", "W4", "W5"].iter().map(|s| s.to_string()).collect(),
                ..base
            },
            Unit::Year => Card {
                label: "2026".into(),
                plays: 21_390,
                hours: 1_204.5,
                avg_daily: 71,
                rank: 1,
                rank_of: 4,
                artists: 812,
                albums: 1_144,
                tracks: 4_902,
                new_artists: 214,
                sides: 63,
                company: Some((418, 21_390)),
                sub: vec![1400, 1810, 2100, 1520, 1980, 2210, 1740, 1650, 1900, 2050, 1780, 1250],
                sub_labels: ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                genre_bands: Some((
                    vec!["dance-pop".into(), "chanson".into(), "french pop".into()],
                    (0..12).map(|i| vec![40.0 + i as f64, 22.0, 14.0, 24.0]).collect(),
                )),
                top_artist: Some(Named { name: "Cœur de pirate".into(), plays: 2_870 }),
                top_album: Some(Titled {
                    title: "Blonde".into(),
                    artist: "Cœur de pirate".into(),
                    plays: 412,
                }),
                crush: Some(Named { name: "Clara Luciani".into(), plays: 631 }),
                ..base
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The report spells a record's name `name` and a track's `title`, and the
    /// card reads both. Getting this wrong is silent — the title comes out
    /// empty and the row renders as a play count with nothing beside it.
    #[test]
    fn a_record_and_a_track_are_both_read_though_the_report_spells_them_differently() {
        let p = json!({
            "top_album": { "name": "Blonde", "artist": "Cœur de pirate", "plays": 96 },
            "on_repeat": [{ "title": "Comme des enfants", "artist": "Cœur de pirate", "plays": 9 }],
        });
        let c = Card::from_period(&p);
        assert_eq!(c.top_album.unwrap().title, "Blonde");
        assert_eq!(c.on_repeat[0].title, "Comme des enfants");
    }

    /// An early month with nothing but listens is an ordinary period, not a
    /// broken payload — the card must come back empty-handed rather than
    /// inventing zeros to draw headings over.
    #[test]
    fn a_period_with_nothing_in_it_is_an_absence_and_not_an_error() {
        let c = Card::from_period(&json!({ "label": "March 2019", "plays": 12 }));
        assert_eq!(c.label, "March 2019");
        assert_eq!(c.plays, 12);
        assert!(c.top_artist.is_none() && c.top_album.is_none());
        assert!(c.genres.is_empty() && c.on_repeat.is_empty());
        assert!(c.colour.is_none() && c.top_chain.is_none());
    }

    /// A genre at 0% is dropped rather than drawn as a bar with no width and a
    /// name beside it, which reads as a rendering fault.
    #[test]
    fn a_genre_nobody_played_is_left_out_rather_than_drawn_flat() {
        let p = json!({ "genres": [
            { "name": "dance-pop", "pct": 41 },
            { "name": "chanson",   "pct": 0  },
        ]});
        assert_eq!(Card::from_period(&p).genres, vec![("dance-pop".to_string(), 41)]);
    }

    /// The initial skips punctuation, so a record called "…And Justice for All"
    /// gets an A rather than an ellipsis, and never panics on a name that has
    /// no letters in it at all.
    #[test]
    fn the_initial_is_the_first_letter_and_not_the_first_character() {
        assert_eq!(Card::initial("Burial"), "B");
        assert_eq!(Card::initial("…And Justice for All"), "A");
        assert_eq!(Card::initial("cœur de pirate"), "C");
        assert_eq!(Card::initial("!!!"), "·");
        assert_eq!(Card::initial(""), "·");
    }
}
