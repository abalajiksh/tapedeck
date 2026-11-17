//! What kind of listen a listen is: classical, film or game score, ambient,
//! or seasonal — and, for classical, which *piece* it belongs to and who
//! played it.
//!
//! Two kinds of answer, kept apart on purpose:
//!
//! - **Splits** (`CLASSICAL`, `SCORE`, `AMBIENT`) leave Top Charts for their own
//!   tabs, per listener (`split_classical`, `split_scores` preferences). Totals,
//!   Statistics, Dashboard and Reports keep counting them.
//! - **Lenses** (`CHRISTMAS`, `HALLOWEEN`, `EASTER`) take nothing out of
//!   anywhere. They add a view.
//!
//! Everything here is a pure function of facts. Fetching them is
//! `musicbrainz.rs`; storing them is `db::facets`; scheduling is [`backfill`].
//!
//! **Precision over recall, everywhere.** A track leaves Top Charts only on
//! positive evidence. A soundtrack song with no data stays a song; an album
//! that merely *looks* like score is a suggestion the listener confirms.

pub mod anniversary;
pub mod backfill;
pub mod canon;
pub mod season;
pub mod titles;

use serde::{Deserialize, Serialize};
use titles::Season;

pub const CLASSICAL: i64 = 1;
pub const SCORE: i64 = 2;
pub const AMBIENT: i64 = 4;
pub const CHRISTMAS: i64 = 8;
pub const HALLOWEEN: i64 = 16;
pub const EASTER: i64 = 32;
/// Not a split: "this album looks like score — confirm?" in the Scores tab.
pub const SCORE_SUGGESTED: i64 = 64;

/// Bumped whenever a rule or the canon changes, so every listen is decided
/// again. The facts behind them are cached, so a bump costs no lookups.
pub const VERSION: i64 = 1;

/// MusicBrainz work types that make a piece classical whoever wrote it — the
/// structure that pulls in Hildegard, Pärt or Ysaÿe without a list. "Song",
/// "Soundtrack", "Musical" and the like are deliberately absent.
pub const CLASSICAL_WORK_TYPES: &[&str] = &[
    "Aria",
    "Ballet",
    "Cantata",
    "Concerto",
    "Sonata",
    "Suite",
    "Madrigal",
    "Mass",
    "Motet",
    "Opera",
    "Oratorio",
    "Overture",
    "Partita",
    "Quartet",
    "Song-cycle",
    "Symphony",
    "Symphonic poem",
    "Zarzuela",
    "Étude",
    "Operetta",
];

const AMBIENT_GENRES: &[&str] = &[
    "ambient",
    "dark ambient",
    "drone",
    "space ambient",
    "tape music",
    "lowercase",
    "berlin school",
    "ambient electronic",
];
const CHRISTMAS_TAGS: &[&str] = &["christmas", "christmas music", "christmas carol", "xmas"];

pub fn season_flag(s: Season) -> i64 {
    match s {
        Season::Christmas => CHRISTMAS,
        Season::Halloween => HALLOWEEN,
        Season::Easter => EASTER,
    }
}

// ── Facts, as MusicBrainz states them ──

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Performer {
    pub name: String,
    pub mbid: String,
    /// MusicBrainz's relationship: `conductor`, `performing orchestra`,
    /// `instrument`, `vocal`, `performer`.
    pub rel: String,
    /// The instrument or voice, when the relationship names one: `violin`.
    #[serde(default)]
    pub attribute: Option<String>,
    /// `Person`, `Orchestra`, `Choir`, `Group`…
    #[serde(default)]
    pub artist_type: Option<String>,
    /// ISO 3166-1, where MusicBrainz says the performer is from. Comes with
    /// the recording lookup at no extra cost.
    #[serde(default)]
    pub country: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParentWork {
    pub mbid: String,
    pub title: String,
    #[serde(default)]
    pub work_type: Option<String>,
}

/// A work the recording performs, with what the recording lookup says about it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkRef {
    pub mbid: String,
    pub title: String,
    #[serde(default)]
    pub work_type: Option<String>,
    /// ISO 639-3. `zxx` is "no linguistic content": instrumental.
    #[serde(default)]
    pub languages: Vec<String>,
    /// `(name, mbid)`.
    #[serde(default)]
    pub composers: Vec<(String, String)>,
    #[serde(default)]
    pub lyricist: bool,
    #[serde(default)]
    pub parent: Option<ParentWork>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordingFacts {
    pub title: String,
    #[serde(default)]
    pub performers: Vec<Performer>,
    #[serde(default)]
    pub works: Vec<WorkRef>,
    /// Tag name and vote count.
    #[serde(default)]
    pub tags: Vec<(String, i64)>,
}

/// The work a piece is filed under: the parent of a movement, or the work itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkFacts {
    pub mbid: String,
    pub title: String,
    #[serde(default)]
    pub work_type: Option<String>,
    #[serde(default)]
    pub tags: Vec<(String, i64)>,
    /// How many movements (direct `parts`). Zero for a work with none.
    #[serde(default)]
    pub part_count: i64,
    #[serde(default)]
    pub composers: Vec<(String, String)>,
    /// When it was composed: the year on the composer relationship, which
    /// MusicBrainz fills for about half of the canon's often-recorded works.
    #[serde(default)]
    pub composed: Option<i32>,
}

/// An artist's dates, from an artist lookup: a person's birth and death, a
/// group's formation and break-up. For anniversaries, and for composers
/// outside the canon, their era, gender and country.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArtistDates {
    pub name: String,
    /// `Person`, `Group`, `Orchestra`, `Choir`…
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    /// `YYYY[-MM[-DD]]`.
    #[serde(default)]
    pub begin: Option<String>,
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub ended: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReleaseFacts {
    pub title: String,
    #[serde(default)]
    pub group_mbid: Option<String>,
    #[serde(default)]
    pub group_title: String,
    #[serde(default)]
    pub primary_type: Option<String>,
    #[serde(default)]
    pub secondary_types: Vec<String>,
    /// The release's own credit, by MBID.
    #[serde(default)]
    pub artist_mbids: Vec<String>,
    #[serde(default)]
    pub track_count: i64,
    /// Tracks credited to the release's own artist(s) and nobody else — the
    /// album-level signal that tells a score album (≥80% on Western score
    /// albums) from a songs album (singers credited on most tracks).
    #[serde(default)]
    pub sole_tracks: i64,
}

impl ReleaseFacts {
    fn is(&self, secondary: &str) -> bool {
        self.secondary_types.iter().any(|t| t.eq_ignore_ascii_case(secondary))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupFacts {
    #[serde(default)]
    pub genres: Vec<(String, i64)>,
    #[serde(default)]
    pub tags: Vec<(String, i64)>,
}

/// What the listen itself carries, for when MusicBrainz knows nothing.
#[derive(Debug, Clone, Default)]
pub struct ListenView {
    pub title: String,
    pub artist: String,
    /// Every credited name: `artist_credit` split, plus the raw `artist` split
    /// on commas — "London Symphony Orchestra, Pyotr Ilyich Tchaikovsky" is
    /// how Pano Scrobbler sends a credit.
    pub credit_names: Vec<String>,
    pub album: Option<String>,
    pub mbid_artist: Vec<String>,
}

impl ListenView {
    pub fn new(
        title: &str,
        artist: &str,
        artist_credit: Option<&str>,
        album: Option<&str>,
        mbid_artist: Option<&str>,
    ) -> Self {
        let mut names: Vec<String> = Vec::new();
        let mut push = |n: &str| {
            let n = n.trim();
            if !n.is_empty() && !names.iter().any(|m| m.eq_ignore_ascii_case(n)) {
                names.push(n.to_string());
            }
        };
        push(artist);
        for part in artist.split(", ") {
            push(part);
        }
        for credit in artist_credit.unwrap_or("").split("; ") {
            push(credit);
            for part in credit.split(", ") {
                push(part);
            }
        }
        ListenView {
            title: title.to_string(),
            artist: artist.to_string(),
            credit_names: names,
            album: album.map(str::to_string).filter(|a| !a.trim().is_empty()),
            mbid_artist: mbid_artist
                .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
                .unwrap_or_default(),
        }
    }
}

// ── The decision ──

/// A performer as the Classical tab ranks them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerformerRow {
    pub name: String,
    pub mbid: Option<String>,
    /// `conductor`, `orchestra`, `ensemble` or `soloist`.
    pub role: String,
    pub instrument: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub facets: i64,
    /// Groups performances of one piece, and covers of one song: `w:<work mbid>`
    /// when MusicBrainz knows the work, else a key built from the title.
    pub piece_key: String,
    pub piece_title: String,
    pub movement: Option<String>,
    pub composer_name: Option<String>,
    pub composer_mbid: Option<String>,
    /// The movement's own work, for counting complete hearings.
    pub leaf_key: Option<String>,
    /// Movements in the piece, when MusicBrainz lists them.
    pub part_count: Option<i64>,
    /// The year the piece was composed, when MusicBrainz knows it.
    pub composed_year: Option<i32>,
    /// The piece's form as MusicBrainz types it: Symphony, Concerto, Opera…
    pub form: Option<String>,
    pub performers: Vec<PerformerRow>,
    /// Why, in a few words each — shown on hover, kept for debugging rules.
    pub evidence: Vec<String>,
}

fn tagged(tags: &[(String, i64)], names: &[&str]) -> bool {
    tags.iter().any(|(t, n)| *n > 0 && names.iter().any(|w| t.eq_ignore_ascii_case(w)))
}

/// Ambient is the album's *top* genre, with at least two votes. Artist tags are
/// never used: Aphex Twin is tagged ambient by 25 people, and *Syro* is not.
pub fn ambient_album(group: &GroupFacts) -> bool {
    let is_amb = |g: &str| AMBIENT_GENRES.iter().any(|a| g.eq_ignore_ascii_case(a));
    let amb = group.genres.iter().filter(|(g, _)| is_amb(g)).map(|(_, n)| *n).max().unwrap_or(0);
    let other = group.genres.iter().filter(|(g, _)| !is_amb(g)).map(|(_, n)| *n).max().unwrap_or(0);
    amb >= 2 && amb >= other
}

pub fn classify(
    listen: &ListenView,
    rec: Option<&RecordingFacts>,
    piece_work: Option<&WorkFacts>,
    release: Option<&ReleaseFacts>,
    group: Option<&GroupFacts>,
) -> Outcome {
    let mut out = Outcome::default();
    let work = rec.and_then(|r| r.works.first());

    // ── Classical ──
    let mut composer: Option<(String, Option<String>)> = None;
    let mut why_classical: Option<String> = None;
    let mut title_for_piece = listen.title.clone();

    // 1. A listed composer wrote the work.
    let work_composers = work
        .map(|w| w.composers.clone())
        .unwrap_or_default()
        .into_iter()
        .chain(piece_work.map(|w| w.composers.clone()).unwrap_or_default());
    for (name, mbid) in work_composers {
        if let Some(c) = canon::by_mbid(&mbid) {
            composer = Some((c.name.to_string(), Some(c.mbid.to_string())));
            why_classical = Some(format!("work by {}", c.name));
            break;
        }
        if composer.is_none() {
            composer = Some((name, Some(mbid)));
        }
    }
    // 2. A listed composer is credited on the listen, by MBID or by name.
    if why_classical.is_none() {
        let found = listen
            .mbid_artist
            .iter()
            .find_map(|m| canon::by_mbid(m))
            .or_else(|| listen.credit_names.iter().find_map(|n| canon::by_name(n)));
        if let Some(c) = found {
            composer = Some((c.name.to_string(), Some(c.mbid.to_string())));
            why_classical = Some(format!("{} credited", c.name));
        }
    }
    // 3. The title or the album leads with a listed composer: "Bach: Cello Suites".
    if why_classical.is_none() {
        if let Some((c, rest)) = titles::strip_composer_prefix(&listen.title) {
            composer = Some((c.name.to_string(), Some(c.mbid.to_string())));
            why_classical = Some(format!("title names {}", c.name));
            title_for_piece = rest;
        } else if let Some((c, _)) = listen.album.as_deref().and_then(titles::strip_composer_prefix)
        {
            composer = Some((c.name.to_string(), Some(c.mbid.to_string())));
            why_classical = Some(format!("album names {}", c.name));
        }
    }
    // 4. MusicBrainz types the piece as a classical form, whoever wrote it.
    if why_classical.is_none() {
        let ty = piece_work
            .and_then(|w| w.work_type.clone())
            .or_else(|| work.and_then(|w| w.parent.as_ref()).and_then(|p| p.work_type.clone()))
            .or_else(|| work.and_then(|w| w.work_type.clone()));
        if let Some(t) =
            ty.filter(|t| CLASSICAL_WORK_TYPES.iter().any(|c| c.eq_ignore_ascii_case(t)))
        {
            why_classical = Some(format!("a {} in MusicBrainz", t.to_lowercase()));
        }
    }
    if let Some(why) = why_classical {
        out.facets |= CLASSICAL;
        out.evidence.push(why);
    }

    // ── The piece: what performances and covers group under ──
    if let Some(w) = work {
        let (key, title) = match (&w.parent, piece_work) {
            (_, Some(pw)) if !pw.mbid.is_empty() => (pw.mbid.clone(), pw.title.clone()),
            (Some(p), _) => (p.mbid.clone(), p.title.clone()),
            _ => (w.mbid.clone(), w.title.clone()),
        };
        out.piece_key = format!("w:{key}");
        out.piece_title = title.clone();
        if key != w.mbid {
            out.leaf_key = Some(format!("w:{}", w.mbid));
            let movement = w.title.strip_prefix(&title).unwrap_or(&w.title);
            out.movement =
                Some(movement.trim_start_matches([':', ' ', '-', '–']).trim().to_string())
                    .filter(|m| !m.is_empty());
        }
        out.part_count = piece_work.map(|p| p.part_count).filter(|&n| n > 0);
        out.composed_year = piece_work.and_then(|p| p.composed);
        out.form = piece_work
            .and_then(|p| p.work_type.clone())
            .or_else(|| w.parent.as_ref().and_then(|p| p.work_type.clone()))
            .or_else(|| w.work_type.clone());
    } else if out.facets & CLASSICAL != 0 {
        let (piece, movement) = titles::split_piece(&title_for_piece);
        let who = composer.as_ref().map(|(n, _)| canon::name_key(n)).unwrap_or_default();
        out.piece_key = format!("t:{}|{}", who, titles::piece_key(&piece));
        out.piece_title = piece;
        out.movement = movement;
    } else {
        out.piece_key =
            format!("t:{}|{}", titles::piece_key(&listen.title), canon::name_key(&listen.artist));
        out.piece_title = listen.title.clone();
    }
    if out.facets & CLASSICAL != 0 {
        if let Some((name, mbid)) = composer {
            out.composer_name = Some(name);
            out.composer_mbid = mbid;
        }
        out.performers = rec.map(performer_rows).unwrap_or_default();
    }

    // ── Score ──
    let lyrics = work
        .is_some_and(|w| w.lyricist || w.languages.iter().any(|l| !l.is_empty() && l != "zxx"))
        || rec.is_some_and(|r| {
            r.performers
                .iter()
                .any(|p| p.rel == "vocal" && p.artist_type.as_deref().is_none_or(|t| t == "Person"))
        });
    let instrumental =
        work.is_some_and(|w| !w.languages.is_empty() && w.languages.iter().all(|l| l == "zxx"));
    let album_name =
        release.map(|r| r.title.clone()).or_else(|| listen.album.clone()).unwrap_or_default();
    let group_title = release.map(|r| r.group_title.clone()).unwrap_or_default();
    match release {
        Some(rel) if rel.is("Soundtrack") && !rel.is("Remix") => {
            if lyrics {
                out.evidence.push("a song on a soundtrack".into());
            } else if titles::has_score_album_words(&album_name)
                || titles::has_score_album_words(&group_title)
            {
                out.facets |= SCORE;
                out.evidence.push("a score album".into());
            } else if instrumental {
                out.facets |= SCORE;
                out.evidence.push("instrumental, on a soundtrack".into());
            } else if titles::has_score_track_words(&listen.title) {
                out.facets |= SCORE;
                out.evidence.push("a cue title, on a soundtrack".into());
            } else if rel.track_count >= 4 && rel.sole_tracks * 5 >= rel.track_count * 4 {
                out.facets |= SCORE_SUGGESTED;
                out.evidence.push("the album credits its composer alone".into());
            }
        }
        Some(_) => {}
        // Names only: the album's own words are all there is.
        None if !lyrics && titles::has_score_album_words(&album_name) => {
            out.facets |= SCORE;
            out.evidence.push("a score album, by its name".into());
        }
        None if !lyrics
            && titles::has_soundtrack_album_words(&album_name)
            && titles::has_score_track_words(&listen.title) =>
        {
            out.facets |= SCORE;
            out.evidence.push("a cue title on a soundtrack, by name".into());
        }
        None => {}
    }

    // ── Ambient ──
    if group.is_some_and(ambient_album) && release.is_none_or(|r| !r.is("Remix")) {
        out.facets |= AMBIENT;
        out.evidence.push("ambient is the album's top genre".into());
    }

    // ── Seasons: tags on the work, the recording or the album; then words ──
    let all_tags: Vec<&(String, i64)> = rec
        .map(|r| r.tags.iter().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(piece_work.map(|w| w.tags.iter().collect::<Vec<_>>()).unwrap_or_default())
        .chain(group.map(|g| g.tags.iter().collect::<Vec<_>>()).unwrap_or_default())
        .collect();
    let owned: Vec<(String, i64)> = all_tags.into_iter().cloned().collect();
    if tagged(&owned, CHRISTMAS_TAGS) {
        out.facets |= CHRISTMAS;
        out.evidence.push("tagged Christmas".into());
    }
    if tagged(&owned, &["halloween"]) {
        out.facets |= HALLOWEEN;
        out.evidence.push("tagged Halloween".into());
    }
    if tagged(&owned, &["easter"]) {
        out.facets |= EASTER;
        out.evidence.push("tagged Easter".into());
    }
    // An album's name speaks for every track on it only for Christmas, where a
    // "Christmas" album is a Christmas album. Patti Smith's *Easter* is not.
    let album_season =
        listen.album.as_deref().filter(|a| titles::season_words(a) == Some(Season::Christmas));
    for text in [Some(listen.title.as_str()), Some(out.piece_title.as_str()), album_season]
        .into_iter()
        .flatten()
    {
        if let Some(s) = titles::season_words(text) {
            let flag = season_flag(s);
            if out.facets & flag == 0 {
                out.facets |= flag;
                out.evidence.push(format!("\u{201c}{text}\u{201d}"));
            }
        }
    }
    out
}

/// Bachtrack's seven periods, by the year a piece was composed — or, for an
/// undated piece, by its composer's birth year plus 35, roughly the middle of
/// a career. The fallback is checked against the era classes of Weiß et al.'s
/// Cross-Era dataset (Musicae Scientiae, 2019) in the tests.
pub fn period(year: i32) -> &'static str {
    match year {
        ..=1599 => "Early",
        1600..=1749 => "Baroque",
        1750..=1819 => "Classical",
        1820..=1899 => "Romantic",
        1900..=1944 => "Early 20th century",
        1945..=1974 => "Modern",
        _ => "Contemporary",
    }
}

pub const PERIODS: &[&str] =
    &["Early", "Baroque", "Classical", "Romantic", "Early 20th century", "Modern", "Contemporary"];

/// The year a piece stands at on the timeline and in the period chart: when
/// it was composed, else its composer's mid-career. `None` without either.
pub fn placed_year(composed: Option<i32>, composer_born: Option<i32>) -> Option<(i32, bool)> {
    match (composed, composer_born) {
        (Some(y), _) => Some((y, true)),
        (None, Some(b)) => Some((b + 35, false)),
        _ => None,
    }
}

/// The year in a `YYYY[-MM[-DD]]` date.
pub fn year_of(date: &str) -> Option<i32> {
    date.get(..4)?.parse().ok()
}

/// MusicBrainz's performance relationships, as the Classical tab ranks them.
pub fn performer_rows(rec: &RecordingFacts) -> Vec<PerformerRow> {
    let mut rows: Vec<PerformerRow> = Vec::new();
    for p in &rec.performers {
        let ty = p.artist_type.as_deref().unwrap_or("Person");
        let (role, instrument) = match (p.rel.as_str(), ty) {
            ("conductor", _) => ("conductor", None),
            ("performing orchestra", _) | (_, "Orchestra") => ("orchestra", None),
            ("instrument" | "vocal" | "performer", "Choir" | "Group") => ("ensemble", None),
            ("instrument", _) => ("soloist", p.attribute.clone()),
            ("vocal", _) => ("soloist", Some(voice(p.attribute.as_deref()))),
            ("performer", _) => ("soloist", None),
            _ => continue,
        };
        if !rows.iter().any(|r| r.mbid.as_deref() == Some(&p.mbid) && r.role == role) {
            rows.push(PerformerRow {
                name: p.name.clone(),
                mbid: Some(p.mbid.clone()).filter(|m| !m.is_empty()),
                role: role.to_string(),
                instrument,
                country: p.country.clone(),
            });
        }
    }
    rows
}

/// "soprano vocals" → "soprano"; "lead vocals" or nothing → "voice".
fn voice(attr: Option<&str>) -> String {
    match attr.map(|a| a.trim_end_matches(" vocals").trim()) {
        Some(v) if !v.is_empty() && v != "lead" && v != "vocals" && v != "background" => {
            v.to_string()
        }
        _ => "voice".to_string(),
    }
}

/// A performer's role from their name alone, for listens nobody described.
pub fn role_from_name(name: &str) -> Option<&'static str> {
    let p = format!(" {} ", name.to_lowercase());
    let has = |ws: &[&str]| ws.iter().any(|w| p.contains(w));
    if has(&[
        "orchestra",
        "orchestre",
        "orchester",
        "philharmoni",
        "symphony",
        "sinfonia",
        "staatskapelle",
        "kammerorchester",
        "filharmonien",
    ]) {
        Some("orchestra")
    } else if has(&[
        " quartet",
        " quartett",
        " quatuor",
        " trio",
        " ensemble",
        " consort",
        " choir",
        " chor ",
        " chorus",
        " singers",
        " sixteen",
        "baroque soloists",
    ]) {
        Some("ensemble")
    } else {
        None
    }
}

/// A listener's own corrections on top of a decision, broadest first so the
/// most specific wins: an artist ("everything by Gibran Alcocer is
/// classical"), then an album, then a piece ("but this one is not").
/// `artist_keys` are the listen's credited names, folded as
/// `Database::artist_key` folds them.
pub fn apply_overrides(
    facets: i64,
    piece_key: &str,
    album_key: Option<&str>,
    artist_keys: &[String],
    overrides: &[crate::db::FacetOverride],
) -> i64 {
    let mut out = facets;
    for kind in ["artist", "album", "work"] {
        for o in overrides.iter().filter(|o| o.target_kind == kind) {
            let hit = match kind {
                "artist" => artist_keys.contains(&o.target_key),
                "album" => album_key == Some(o.target_key.as_str()),
                _ => o.target_key == piece_key,
            };
            if hit {
                out = if o.value { out | o.facet } else { out & !o.facet };
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
