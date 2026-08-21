//! Finding a record or tape you are holding.
//!
//! Two providers, because they are good at different things. **MusicBrainz**
//! needs no credential and has reliable tracklists with durations.
//! **Discogs** is far better at *pressings* — it is a collector's database, so
//! a barcode usually resolves to the exact edition — but its API is
//! authenticated, so it stays switched off until a credential is configured.
//! Same shape as Apple Music in the bio chain: unconfigured is a normal state
//! that steps aside, not an error.
//!
//! **Sides are not reliably available anywhere.** MusicBrainz records a vinyl
//! track number as "A1" only when the editor bothered; most releases carry
//! plain 1..n, including the pressing of *The Dark Side of the Moon* this was
//! checked against. So `assign_sides` parses a side letter when one is there
//! and otherwise splits by playing time, and the result is presented as a guess
//! the user corrects rather than as fact.
//!
//! Discogs, notably, *does* have the side letters for that same pressing — its
//! tracklist positions come back `A1`, `A2`, `B1`… — which is the clearest
//! statement of what it is for.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::debug;

/// How to authenticate to Discogs.
///
/// Discogs has four auth tiers and only the bottom two are relevant here.
/// Unauthenticated works but is capped at 25 requests a minute and returns no
/// images. A **personal access token** or a **consumer key/secret pair** both
/// lift that to 60 and unlock images, and either is enough for everything
/// Tapedeck does — it only ever reads the public database.
///
/// The three-legged OAuth flow is deliberately **not** implemented. It exists
/// so an application can act *as a particular user* (their collection,
/// wantlist, marketplace orders); Tapedeck looks up pressings, which is not
/// user-specific. Registering an application on Discogs hands you a consumer
/// key and secret, and those work directly as a credential without any of the
/// request-token/authorize/access-token dance — which is why the pair is
/// accepted here rather than a flow being built.
#[derive(Debug, Clone)]
pub enum DiscogsAuth {
    /// A personal access token from discogs.com/settings/developers.
    Token(String),
    /// An application's consumer key and secret, sent directly.
    KeySecret { key: String, secret: String },
}

impl DiscogsAuth {
    /// The `Authorization` header value.
    ///
    /// The key/secret form is sent as a header rather than as `?key=…&secret=…`
    /// query parameters — Discogs documents both, but a secret in a query
    /// string ends up in proxy logs and any error message that echoes the URL.
    fn header(&self) -> String {
        match self {
            DiscogsAuth::Token(t) => format!("Discogs token={t}"),
            DiscogsAuth::KeySecret { key, secret } => {
                format!("Discogs key={key}, secret={secret}")
            }
        }
    }
}

/// One track on a physical side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalTrack {
    /// "A", "B", "C"… Cassettes have two, a double LP four.
    pub side: String,
    /// Position within the side, from 1.
    pub position: i64,
    pub title: String,
    pub artist: Option<String>,
    pub duration_secs: Option<i64>,
    /// Which physical disc of the set this track is on, from 1.
    ///
    /// Needs no migration: the tracklist is a JSON blob in
    /// `physical_releases.tracks`, so a record catalogued before this existed
    /// parses with `None` — which is honest. It means "nobody recorded which
    /// disc", not "disc 1", and the panel groups only when it actually knows.
    #[serde(default)]
    pub disc: Option<i64>,
}

/// A candidate pressing.
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseCandidate {
    pub source: &'static str,
    pub artist: String,
    pub title: String,
    pub year: Option<i64>,
    pub country: Option<String>,
    pub format: Option<String>,
    pub mbid_release: Option<String>,
    pub discogs_id: Option<String>,
    pub barcode: Option<String>,
    pub catalog_number: Option<String>,
    pub artwork_url: Option<String>,
    pub tracks: Vec<PhysicalTrack>,
}

fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(format!(
            "{}/{} ( https://codeberg.org/abksh/tapedeck )",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        ))
        .build()
}

/// Split a flat tracklist into sides.
///
/// A side letter in the track number is authoritative when present — that is an
/// editor stating it. Otherwise the tracks are split so the sides are as close
/// to equal *playing time* as possible, which is what actually determines a
/// side break on a record: a side holds around 20 minutes and a mastering
/// engineer balances them. Splitting by track count instead would put a 9-minute
/// closer on the wrong side of the break.
///
/// Only ever a starting point. The caller shows it and lets the user move the
/// break, because no amount of cleverness recovers what the pressing did.
pub fn assign_sides(numbers: &[String], durations: &[Option<i64>], sides: usize) -> Vec<String> {
    let n = numbers.len();
    if n == 0 {
        return Vec::new();
    }

    // Explicit side letters, when the data has them.
    let lettered: Vec<Option<char>> = numbers
        .iter()
        .map(|num| {
            num.trim()
                .chars()
                .next()
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_uppercase())
        })
        .collect();
    if lettered.iter().all(|c| c.is_some()) {
        return lettered.into_iter().map(|c| c.expect("checked").to_string()).collect();
    }

    let sides = sides.max(1);
    let total: i64 = durations.iter().map(|d| d.unwrap_or(0)).sum();
    // No durations at all — nothing better than an even split by count.
    if total == 0 {
        return (0..n).map(|i| side_letter(i * sides / n)).collect();
    }

    let target = total as f64 / sides as f64;
    let mut out = Vec::with_capacity(n);
    let (mut side, mut acc) = (0usize, 0i64);
    for (i, d) in durations.iter().enumerate() {
        let d = d.unwrap_or(0);
        // Start a new side when this track would take the current one further
        // past its share than leaving it out would — but never leave a side
        // empty, and never overflow the side count.
        let remaining_sides = sides - side;
        let remaining_tracks = n - i;
        if side + 1 < sides
            && acc > 0
            && (acc as f64 + d as f64 / 2.0) > target * (side + 1) as f64
            && remaining_tracks >= remaining_sides
        {
            side += 1;
        }
        out.push(side_letter(side));
        acc += d;
    }
    out
}

fn side_letter(i: usize) -> String {
    ((b'A' + (i.min(25) as u8)) as char).to_string()
}

/// Where a side letter sits in the alphabet. The inverse of `side_letter`.
fn letter_index(s: &str) -> usize {
    s.trim()
        .chars()
        .next()
        .filter(|c| c.is_ascii_alphabetic())
        .map_or(0, |c| (c.to_ascii_uppercase() as u8 - b'A') as usize)
}

/// Split a multi-disc release into sides **one disc at a time**.
///
/// `assign_sides` on its own is given the whole release flattened, and splits
/// it into `media_count * 2` sides of even playing time. That is right for a
/// single LP and quietly wrong for an unbalanced set: a 2-LP where the first
/// record runs 46 minutes and the second 22 gets four sides of 17, so side C
/// opens with tracks that are physically on disc 1. Nothing errors — the
/// tracklist simply describes an object that does not exist, and the shelf then
/// tells you to flip to a side that is not there.
///
/// MusicBrainz already says which medium each track came off; the caller used
/// to discard it in a `flat_map`. Given it back, each disc is split into its own
/// two sides by *its own* running time, which is what physically happened, and
/// the letters continue across discs (disc 2 is C and D).
///
/// A disc whose tracks carry real side letters keeps them and is **not**
/// offset — `C1`…`D5` is already absolute. That is per disc rather than
/// per release, because a set can have one medium an editor lettered and
/// another they numbered.
pub fn assign_sides_by_disc(per_disc: &[(Vec<String>, Vec<Option<i64>>)]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, (numbers, durations)) in per_disc.iter().enumerate() {
        let lettered = !numbers.is_empty()
            && numbers
                .iter()
                .all(|n| n.trim().chars().next().is_some_and(|c| c.is_ascii_alphabetic()));
        let sides = assign_sides(numbers, durations, 2);
        if lettered {
            out.extend(sides);
        } else {
            out.extend(sides.iter().map(|s| side_letter(letter_index(s) + i * 2)));
        }
    }
    out
}

/// Is this medium one you play straight through, rather than one you get up
/// and flip?
///
/// The whole of `assign_sides` exists because a record has a break in it that
/// nobody reliably records. A disc has no break: it plays start to finish, so
/// splitting its tracklist by playing time would invent one, and the shelf
/// would then offer to remind you to flip to a Side B that does not exist.
/// This is the *smaller* problem the roadmap predicted — real track boundaries
/// come straight off the disc.
///
/// Matched on the medium's own format string rather than on what the user
/// searched for, because the release is the authority on what it is.
pub fn is_disc(format: Option<&str>) -> bool {
    let f = format.unwrap_or("").to_ascii_lowercase();
    // "cassette" has no "cd" in it and neither does any vinyl spelling, so the
    // substring test is safe and catches CD-R, Hybrid SACD, Enhanced CD…
    f.contains("cd") || f.contains("dvd") || f.contains("blu-ray") || f.contains("minidisc")
}

/// What shelf `kind` a medium's own format string describes.
///
/// The kind picker in the UI is what the user *thought* they were adding; this
/// is what the release says it is, and the release is the authority — the same
/// reasoning `is_disc` already follows for side splitting. Without it a box set
/// looked up by barcode could be filed as vinyl while its five media all say
/// `CD`, which is how "Original Album Classics" ended up on the wrong shelf
/// with 76 tracks and no way to move it.
///
/// Returns `None` when the format is missing or is something the shelf has no
/// kind for (digital media, a format nobody typed) — in which case the user's
/// own choice stands rather than being overridden by a guess.
///
/// **Order matters and is the whole trap: `"sacd"` contains `"cd"`.** So does
/// `"Hybrid SACD"`. Testing for CD first would file every SACD as a CD and the
/// mistake would be invisible, because a CD is a plausible thing to own.
pub fn kind_from_format(format: Option<&str>) -> Option<&'static str> {
    let f = format?.to_ascii_lowercase();
    if f.is_empty() {
        return None;
    }
    if f.contains("sacd") {
        return Some("sacd");
    }
    if f.contains("cassette") {
        return Some("cassette");
    }
    // "Vinyl", "12\" Vinyl", "LP", "7\" Single". Checked before the disc test
    // because a "DVD-Video" is not vinyl but a "Vinyl" never contains "cd".
    if f.contains("vinyl") || f.contains("lp") || f.contains("shellac") {
        return Some("vinyl");
    }
    if is_disc(Some(&f)) {
        return Some("cd");
    }
    None
}

/// One "side" per disc, numbered — the counterpart of `assign_sides` for media
/// that have no sides. A single CD is `["1", "1", …]`; a two-disc set numbers
/// the second disc's tracks `2`.
pub fn disc_sides(per_disc_counts: &[usize]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, n) in per_disc_counts.iter().enumerate() {
        for _ in 0..*n {
            out.push((i + 1).to_string());
        }
    }
    out
}

/// Match a fetched tracklist onto the one already on the shelf, by title.
///
/// The point of this function is that **position matching is wrong**. A record
/// on the shelf has no durations because the source that catalogued it had
/// none — Discogs frequently does — so the fix is to look the release up
/// somewhere else, and "somewhere else" is a *different pressing*: it may carry
/// bonus tracks, run to a different number of media, or order a side
/// differently. Lining the two lists up by index would silently assign one
/// track's running time to another, which is the same class of invisible error
/// as the placeholder durations this exists to replace.
///
/// So titles decide, using the fold `playlists::pick` already applies to the
/// same question — `db::normalise_artist` for punctuation and accents, then a
/// second pass with any trailing bracketed qualifier removed, so a shelf's
/// "Haunted" matches a MusicBrainz "Haunted (Album Version)". That fold is not
/// a third normaliser: it is the existing one asked about a different field.
///
/// Matching is **one-to-one** and in stored order. A release with two tracks of
/// the same name — a reprise, a hidden track — gives its first unclaimed copy
/// to the first asker rather than the same duration to both.
///
/// Returns one entry per stored track, `None` where nothing matched. A caller
/// must show that to somebody before writing it: a wrong duration is invisible
/// once stored, and re-deriving which ones were guessed is impossible.
pub fn match_durations(stored: &[PhysicalTrack], found: &[PhysicalTrack]) -> Vec<Option<i64>> {
    let fold = |t: &str| crate::db::normalise_artist(t);
    let stem = |t: &str| crate::db::normalise_artist(crate::playlists::without_qualifier(t));

    let mut claimed = vec![false; found.len()];
    let mut out = Vec::with_capacity(stored.len());

    for t in stored {
        // Exact fold first, so a qualifier-stripped near-match can never beat a
        // title that agrees outright.
        let exact = found.iter().enumerate().position(|(i, f)| {
            !claimed[i] && f.duration_secs.is_some() && fold(&f.title) == fold(&t.title)
        });
        let hit = exact.or_else(|| {
            found.iter().enumerate().position(|(i, f)| {
                !claimed[i] && f.duration_secs.is_some() && stem(&f.title) == stem(&t.title)
            })
        });
        match hit {
            Some(i) => {
                claimed[i] = true;
                out.push(found[i].duration_secs);
            }
            None => out.push(None),
        }
    }
    out
}

/// Search MusicBrainz for a pressing.
///
/// `query` is a Lucene expression; the caller composes it so barcode, catalogue
/// number and artist+title searches all come through here. Restricted to the
/// physical formats — a CD or a digital release is not something you flip.
pub async fn musicbrainz_search(
    query: &str,
    limit: usize,
) -> Result<Vec<ReleaseCandidate>, Box<dyn std::error::Error + Send + Sync>> {
    #[derive(Deserialize)]
    struct SearchResp {
        #[serde(default)]
        releases: Vec<SearchRelease>,
    }
    #[derive(Deserialize)]
    struct SearchRelease {
        id: String,
    }

    let http = client()?;
    let resp = http
        .get("https://musicbrainz.org/ws/2/release/")
        .query(&[("query", query), ("fmt", "json"), ("limit", &limit.to_string())])
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        return Err("MusicBrainz is rate limiting us — try again in a moment".into());
    }
    if !resp.status().is_success() {
        return Err(format!("MusicBrainz returned {}", resp.status()).into());
    }
    let found: SearchResp = resp.json().await?;

    // The search index carries no tracklist, so each candidate is a second
    // request. Bounded hard and paced: this is an unauthenticated shared
    // service and the user is standing at a turntable, not running a crawl.
    let mut out = Vec::new();
    let mut last_err = None;
    for r in found.releases.into_iter().take(limit.min(5)) {
        match musicbrainz_release(&r.id).await {
            Ok(Some(c)) => out.push(c),
            Ok(None) => {}
            Err(e) => {
                debug!("MusicBrainz release {} failed: {}", r.id, e);
                last_err = Some(e);
            }
        }
        tokio::time::sleep(Duration::from_millis(1100)).await;
    }

    // Every candidate failing is not "nothing found" — it is usually the rate
    // limiter, and reporting it as an empty shelf sends the user hunting for a
    // record that is right there. Only raised when nothing at all came back,
    // so one bad release among five still returns the other four.
    if out.is_empty() {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(out)
}

/// One release with its tracklist.
pub async fn musicbrainz_release(
    mbid: &str,
) -> Result<Option<ReleaseCandidate>, Box<dyn std::error::Error + Send + Sync>> {
    #[derive(Deserialize)]
    struct Release {
        id: String,
        title: String,
        #[serde(default)]
        date: Option<String>,
        #[serde(default)]
        country: Option<String>,
        #[serde(default)]
        barcode: Option<String>,
        #[serde(rename = "artist-credit", default)]
        artist_credit: Vec<ArtistCredit>,
        #[serde(default)]
        media: Vec<Medium>,
        #[serde(rename = "label-info", default)]
        label_info: Vec<LabelInfo>,
    }
    #[derive(Deserialize)]
    struct ArtistCredit {
        name: String,
        #[serde(default)]
        joinphrase: Option<String>,
    }
    #[derive(Deserialize)]
    struct LabelInfo {
        #[serde(rename = "catalog-number", default)]
        catalog_number: Option<String>,
    }
    #[derive(Deserialize)]
    struct Medium {
        #[serde(default)]
        format: Option<String>,
        #[serde(default)]
        tracks: Vec<Track>,
    }
    #[derive(Deserialize)]
    struct Track {
        #[serde(default)]
        number: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        length: Option<i64>,
    }

    let http = client()?;
    let resp = http
        .get(format!("https://musicbrainz.org/ws/2/release/{mbid}"))
        .query(&[("inc", "recordings+artist-credits+labels"), ("fmt", "json")])
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        return Err("MusicBrainz is rate limiting us — try again in a moment".into());
    }
    if !resp.status().is_success() {
        return Err(format!("MusicBrainz returned {}", resp.status()).into());
    }
    let r: Release = resp.json().await?;

    let artist = r
        .artist_credit
        .iter()
        .map(|a| format!("{}{}", a.name, a.joinphrase.clone().unwrap_or_default()))
        .collect::<String>();
    let format = r.media.first().and_then(|m| m.format.clone());

    // Which disc each track came off. This used to be discarded by a `flat_map`
    // straight into `assign_sides`, which then split the *whole release* into
    // even sides — fine for one LP, and wrong for an unbalanced set, where a
    // side letter would be printed over tracks that are on the other record.
    // MusicBrainz tells us; there is no reason to throw it away.
    let flat: Vec<&Track> = r.media.iter().flat_map(|m| m.tracks.iter()).collect();
    let durations: Vec<Option<i64>> = flat.iter().map(|t| t.length.map(|ms| ms / 1000)).collect();
    let disc_of: Vec<i64> = r
        .media
        .iter()
        .enumerate()
        .flat_map(|(i, m)| std::iter::repeat_n(i as i64 + 1, m.tracks.len()))
        .collect();
    // A disc is numbered by disc, not lettered by side — see `is_disc`.
    let side_of = if is_disc(format.as_deref()) {
        disc_sides(&r.media.iter().map(|m| m.tracks.len()).collect::<Vec<_>>())
    } else {
        let per_disc: Vec<(Vec<String>, Vec<Option<i64>>)> = r
            .media
            .iter()
            .map(|m| {
                (
                    m.tracks.iter().map(|t| t.number.clone().unwrap_or_default()).collect(),
                    m.tracks.iter().map(|t| t.length.map(|ms| ms / 1000)).collect(),
                )
            })
            .collect();
        assign_sides_by_disc(&per_disc)
    };

    let mut tracks = Vec::with_capacity(flat.len());
    let mut per_side_pos: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for (i, t) in flat.iter().enumerate() {
        let side = side_of.get(i).cloned().unwrap_or_else(|| "A".into());
        let pos = per_side_pos.entry(side.clone()).or_insert(0);
        *pos += 1;
        tracks.push(PhysicalTrack {
            side,
            position: *pos,
            title: t.title.clone().unwrap_or_else(|| "Untitled".into()),
            artist: None,
            duration_secs: durations[i],
            disc: disc_of.get(i).copied(),
        });
    }

    Ok(Some(ReleaseCandidate {
        source: "musicbrainz",
        artist,
        title: r.title,
        year: r.date.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()),
        country: r.country,
        format,
        mbid_release: Some(r.id),
        discogs_id: None,
        barcode: r.barcode,
        catalog_number: r.label_info.into_iter().find_map(|l| l.catalog_number),
        artwork_url: None,
        tracks,
    }))
}

/// Ask Discogs whether it actually accepts a credential.
///
/// Harder than it should be, and worth the paragraph because **most of the API
/// does not reject a bad credential at all**. Measured rather than assumed:
/// with `key=bogus, secret=bogus`, both `/releases/1` and `/database/search`
/// answer a normal `200` with real data — and the rate limit header even reads
/// the authenticated `60` rather than the anonymous `25`, because Discogs
/// raises it for the *presence* of an `Authorization` header without checking
/// what is in it. So neither the status code, nor the body, nor the headers
/// distinguish a working credential from a typo on the paths Tapedeck uses.
///
/// `/users/{username}` is the endpoint that does verify, for both shapes: a bad
/// pair gets `401 Invalid consumer key/secret`, a bad token `401 Invalid
/// consumer token`. `/oauth/identity` is not usable for this — it wants
/// *user*-level auth, so it 401s even for a perfectly good consumer pair.
///
/// The subject is Discogs' own account, because the request needs some user and
/// that one is as stable as the API itself. A 404 would mean it had gone, which
/// says nothing about the credential — so that is reported as inconclusive
/// rather than as a failure.
pub async fn discogs_verify(
    auth: &DiscogsAuth,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let http = client()?;
    let resp = http
        .get("https://api.discogs.com/users/discogs")
        .header("Authorization", auth.header())
        .send()
        .await?;

    match resp.status() {
        s if s.is_success() => Ok(()),
        reqwest::StatusCode::UNAUTHORIZED => {
            // Discogs words this precisely enough to be worth passing through:
            // it names which of the two credential shapes it rejected.
            let msg = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v.get("message").and_then(|m| m.as_str().map(str::to_string)))
                .unwrap_or_else(|| "Discogs rejected that credential".into());
            Err(msg.into())
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => {
            Err("Discogs rate limit reached — try again in a minute".into())
        }
        reqwest::StatusCode::NOT_FOUND => {
            Err("Could not verify — the account used for the check no longer exists".into())
        }
        other => Err(format!("Discogs returned {other}").into()),
    }
}

/// Look a barcode up on Discogs.
///
/// Requires a credential, so this is only called when one is configured.
/// Discogs is worth the extra step for physical media specifically: it
/// catalogues *pressings*, so a barcode generally lands on the exact edition in
/// your hands, where MusicBrainz will often only have "the album".
///
/// A scanned barcode is passed through as-is. Discogs stores them with the
/// printed spacing (`5 099902 987613`) but normalises on its side, so the
/// unspaced form a scanner produces matches — and in practice matches slightly
/// *more* than the spaced one.
pub async fn discogs_search(
    auth: &DiscogsAuth,
    barcode: Option<&str>,
    query: Option<&str>,
) -> Result<Vec<ReleaseCandidate>, Box<dyn std::error::Error + Send + Sync>> {
    #[derive(Deserialize)]
    struct SearchResp {
        #[serde(default)]
        results: Vec<Hit>,
    }
    #[derive(Deserialize)]
    struct Hit {
        id: i64,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        year: Option<serde_json::Value>,
        #[serde(default)]
        country: Option<String>,
        #[serde(default)]
        format: Vec<String>,
        #[serde(default)]
        barcode: Vec<String>,
        #[serde(default)]
        catno: Option<String>,
        #[serde(default)]
        cover_image: Option<String>,
    }

    let http = client()?;
    let mut req = http
        .get("https://api.discogs.com/database/search")
        .header("Authorization", auth.header())
        .query(&[("type", "release"), ("per_page", "10")]);
    if let Some(b) = barcode {
        req = req.query(&[("barcode", b)]);
    }
    if let Some(q) = query {
        req = req.query(&[("q", q)]);
    }

    let resp = req.send().await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Discogs rejected that credential".into());
    }
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("Discogs rate limit reached — try again in a minute".into());
    }
    if !resp.status().is_success() {
        return Err(format!("Discogs returned {}", resp.status()).into());
    }
    let found: SearchResp = resp.json().await?;

    Ok(found
        .results
        .into_iter()
        // Only things you can physically flip.
        .filter(|h| {
            h.format.iter().any(|f| {
                let f = f.to_lowercase();
                f.contains("vinyl") || f.contains("lp") || f.contains("cassette")
            })
        })
        .map(|h| {
            // Discogs packs "Artist - Title" into one string.
            let raw = h.title.unwrap_or_default();
            let (artist, title) = match raw.split_once(" - ") {
                Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
                None => (String::new(), raw),
            };
            ReleaseCandidate {
                source: "discogs",
                artist,
                title,
                year: h.year.as_ref().and_then(|y| match y {
                    serde_json::Value::Number(n) => n.as_i64(),
                    serde_json::Value::String(s) => s.get(..4).and_then(|y| y.parse().ok()),
                    _ => None,
                }),
                country: h.country,
                format: Some(h.format.join(", ")),
                mbid_release: None,
                discogs_id: Some(h.id.to_string()),
                barcode: h.barcode.into_iter().next(),
                catalog_number: h.catno,
                artwork_url: h.cover_image,
                // The search endpoint carries no tracklist. Fetched separately
                // only once a candidate is chosen, so browsing costs one call.
                tracks: Vec::new(),
            }
        })
        .collect())
}

/// A chosen Discogs release, with its tracklist.
pub async fn discogs_release(
    auth: &DiscogsAuth,
    id: &str,
) -> Result<Option<ReleaseCandidate>, Box<dyn std::error::Error + Send + Sync>> {
    #[derive(Deserialize)]
    struct Release {
        id: i64,
        title: String,
        #[serde(default)]
        year: Option<i64>,
        #[serde(default)]
        country: Option<String>,
        #[serde(default)]
        artists: Vec<Artist>,
        #[serde(default)]
        tracklist: Vec<Tr>,
        #[serde(default)]
        formats: Vec<Format>,
        #[serde(default)]
        identifiers: Vec<Ident>,
        #[serde(default)]
        labels: Vec<Label>,
        #[serde(default)]
        images: Vec<Image>,
    }
    #[derive(Deserialize)]
    struct Artist {
        name: String,
    }
    #[derive(Deserialize)]
    struct Tr {
        #[serde(default)]
        position: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        duration: Option<String>,
        #[serde(rename = "type_", default)]
        kind: Option<String>,
    }
    #[derive(Deserialize)]
    struct Format {
        #[serde(default)]
        name: Option<String>,
    }
    #[derive(Deserialize)]
    struct Ident {
        #[serde(rename = "type", default)]
        kind: Option<String>,
        #[serde(default)]
        value: Option<String>,
    }
    #[derive(Deserialize)]
    struct Label {
        #[serde(default)]
        catno: Option<String>,
    }
    #[derive(Deserialize)]
    struct Image {
        #[serde(default)]
        uri: Option<String>,
    }

    let http = client()?;
    let resp = http
        .get(format!("https://api.discogs.com/releases/{id}"))
        .header("Authorization", auth.header())
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("Discogs returned {}", resp.status()).into());
    }
    let r: Release = resp.json().await?;

    // Discogs marks side headings as `type_: "heading"` — they are not tracks
    // and would otherwise become entries with no duration.
    let real: Vec<&Tr> =
        r.tracklist.iter().filter(|t| t.kind.as_deref().unwrap_or("track") == "track").collect();

    let numbers: Vec<String> =
        real.iter().map(|t| t.position.clone().unwrap_or_default()).collect();
    let durations: Vec<Option<i64>> =
        real.iter().map(|t| parse_duration(t.duration.as_deref())).collect();
    let sides = r.formats.len().max(1) * 2;
    // Discogs names the medium in `formats[].name`; a disc gets one numbered
    // side per disc rather than a guessed A/B break.
    let side_of = if is_disc(r.formats.first().and_then(|f| f.name.as_deref())) {
        disc_sides(&[real.len()])
    } else {
        assign_sides(&numbers, &durations, sides)
    };

    let mut tracks = Vec::with_capacity(real.len());
    let mut per_side_pos: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for (i, t) in real.iter().enumerate() {
        let side = side_of.get(i).cloned().unwrap_or_else(|| "A".into());
        let pos = per_side_pos.entry(side.clone()).or_insert(0);
        *pos += 1;
        tracks.push(PhysicalTrack {
            side,
            position: *pos,
            title: t.title.clone().unwrap_or_else(|| "Untitled".into()),
            artist: None,
            duration_secs: durations[i],
            // Discogs' tracklist is flat and names no medium per track, so
            // there is nothing to record here. `None` says that, rather than
            // claiming disc 1.
            disc: None,
        });
    }

    Ok(Some(ReleaseCandidate {
        source: "discogs",
        artist: r.artists.first().map(|a| a.name.clone()).unwrap_or_default(),
        title: r.title,
        year: r.year.filter(|y| *y > 0),
        country: r.country,
        format: r.formats.into_iter().filter_map(|f| f.name).next(),
        mbid_release: None,
        discogs_id: Some(r.id.to_string()),
        barcode: r
            .identifiers
            .into_iter()
            .find(|i| i.kind.as_deref() == Some("Barcode"))
            .and_then(|i| i.value),
        catalog_number: r.labels.into_iter().find_map(|l| l.catno),
        artwork_url: r.images.into_iter().find_map(|i| i.uri),
        tracks,
    }))
}

// ════════════════════════════════════════════════════════════
//  Everything else in the sleeve
// ════════════════════════════════════════════════════════════

/// One image the Cover Art Archive holds for a release.
///
/// Distinct from the single `artwork_url` a candidate carries: that is the
/// cover, and this is *everything else* — the back, the spine, the label, the
/// obi strip, and every page of the booklet. The archive holds 29 for the
/// pressing this was built against, 19 of them booklet pages, which is why the
/// shelf stores rows rather than one more column.
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveImage {
    /// Tapedeck's own vocabulary — see `image_kind`.
    pub kind: String,
    /// Full-size. Always present; the archive's `image` field is the file.
    pub url: String,
    /// A 250px edge for the gallery grid. Absent on very old uploads that
    /// predate thumbnail generation, which is why it is optional rather than
    /// derived from `url` by string surgery.
    pub thumb_url: Option<String>,
    /// The editor's own note — "inner left panel", "page 3". Worth keeping:
    /// nineteen booklet pages are indistinguishable without it.
    pub caption: Option<String>,
    /// The archive's image id, as a string. It is a 64-bit number in the JSON
    /// and there is no arithmetic to do on it, so it is carried as text — which
    /// is also what makes one storage column serve Discogs' ids too.
    pub id: String,
    /// The archive's own ordering, which puts the cover first and the booklet
    /// in page order. Kept rather than re-sorted: it is editor-curated, and no
    /// rule invented here would beat it.
    pub position: i64,
    /// Whether a MusicBrainz editor has approved the upload.
    pub approved: bool,
}

/// The type priority when an image claims several.
///
/// The archive lets one file be `["Front", "Sticker"]` (a sealed copy) or
/// `["Medium", "Matrix/Runout"]` (a label shot that happens to show the
/// runout), so a first-wins rule files the cover under "sticker". The most
/// structural description wins instead.
const KIND_PRIORITY: &[(&str, &str)] = &[
    ("Front", "front"),
    ("Back", "back"),
    ("Booklet", "booklet"),
    ("Medium", "medium"),
    ("Tray", "tray"),
    ("Obi", "obi"),
    ("Spine", "spine"),
    ("Liner", "liner"),
    ("Track", "track"),
    ("Sticker", "sticker"),
    ("Poster", "poster"),
    ("Matrix/Runout", "matrix"),
];

/// Which of Tapedeck's kinds an archive image is.
///
/// An unrecognised type is **slugified rather than dumped into `other`**: the
/// archive's vocabulary grows — `Panel` is on the very release this was checked
/// against and is not in any of the documentation — and "inner left panel" is a
/// more useful shelf heading than "other". Only a type with nothing usable in
/// it falls all the way through.
pub fn image_kind(types: &[String]) -> String {
    for (archive, ours) in KIND_PRIORITY {
        if types.iter().any(|t| t.eq_ignore_ascii_case(archive)) {
            return (*ours).to_string();
        }
    }
    types
        .first()
        .map(|t| {
            t.chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "other".to_string())
}

/// Parse a Cover Art Archive index. Split out from the fetch so the shapes can
/// be pinned against a captured response rather than the live service.
pub fn parse_cover_art(body: &str) -> Result<Vec<ArchiveImage>, Box<dyn std::error::Error>> {
    #[derive(Deserialize)]
    struct Index {
        #[serde(default)]
        images: Vec<Img>,
    }
    #[derive(Deserialize)]
    struct Img {
        #[serde(default)]
        types: Vec<String>,
        image: Option<String>,
        #[serde(default)]
        thumbnails: std::collections::HashMap<String, String>,
        #[serde(default)]
        comment: String,
        // The archive has served this as a number and as a string across its
        // lifetime, and a strict type here would fail the whole document — the
        // same class of bug `lenient_track_number` exists for.
        id: Option<serde_json::Value>,
        #[serde(default)]
        approved: bool,
    }

    let index: Index = serde_json::from_str(body)?;
    Ok(index
        .images
        .into_iter()
        .enumerate()
        .filter_map(|(i, img)| {
            let url = img.image?;
            let id = match img.id {
                Some(serde_json::Value::String(s)) => s,
                Some(serde_json::Value::Number(n)) => n.to_string(),
                // No id means nothing to key an update on, so the row would
                // duplicate on every re-fetch. The URL is unique per file and
                // stable, which is exactly what is needed.
                _ => url.clone(),
            };
            Some(ArchiveImage {
                kind: image_kind(&img.types),
                thumb_url: img
                    .thumbnails
                    .get("250")
                    .or_else(|| img.thumbnails.get("small"))
                    .or_else(|| img.thumbnails.get("500"))
                    .cloned(),
                url,
                caption: Some(img.comment.trim().to_string()).filter(|c| !c.is_empty()),
                id,
                position: i as i64,
                approved: img.approved,
            })
        })
        .collect())
}

/// Fetch everything the Cover Art Archive holds for a release MBID.
///
/// No credential, no rate limit worth pacing for, and one request per release —
/// so this runs when the user asks rather than on a backfill clock. A release
/// with no art at all answers **404**, which is a normal outcome and comes back
/// as an empty list rather than an error: most pressings have a cover and
/// nothing else.
pub async fn cover_art_archive(
    mbid: &str,
) -> Result<Vec<ArchiveImage>, Box<dyn std::error::Error>> {
    let url = format!("https://coverartarchive.org/release/{mbid}");
    let resp = client()?.get(&url).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        debug!("No cover art archived for {}", mbid);
        return Ok(Vec::new());
    }
    if !resp.status().is_success() {
        return Err(format!("Cover Art Archive returned {}", resp.status()).into());
    }
    parse_cover_art(&resp.text().await?)
}

/// Every image Discogs holds for a release, in its own order.
///
/// Discogs serves images only to an authenticated caller, which is why this
/// takes the credential and why an instance with none simply gets the archive's
/// set. Its images carry no per-image id, so the URL is the key.
pub async fn discogs_images(
    auth: &DiscogsAuth,
    id: &str,
) -> Result<Vec<ArchiveImage>, Box<dyn std::error::Error>> {
    #[derive(Deserialize)]
    struct Release {
        #[serde(default)]
        images: Vec<Image>,
    }
    #[derive(Deserialize)]
    struct Image {
        #[serde(rename = "type")]
        kind: Option<String>,
        uri: Option<String>,
        uri150: Option<String>,
    }

    let url = format!("https://api.discogs.com/releases/{id}");
    let resp =
        client()?.get(&url).header(reqwest::header::AUTHORIZATION, auth.header()).send().await?;
    if !resp.status().is_success() {
        return Err(format!("Discogs returned {}", resp.status()).into());
    }
    let release: Release = resp.json().await?;
    Ok(release
        .images
        .into_iter()
        .enumerate()
        .filter_map(|(i, img)| {
            let url = img.uri?;
            Some(ArchiveImage {
                // Discogs says only "primary" or "secondary". Primary is the
                // cover; secondary could be anything, and claiming to know
                // which would be inventing a fact — so it is left unsorted and
                // the user files it if they care.
                kind: if img.kind.as_deref() == Some("primary") {
                    "front".to_string()
                } else {
                    "other".to_string()
                },
                thumb_url: img.uri150,
                caption: None,
                id: url.clone(),
                url,
                position: i as i64,
                approved: true,
            })
        })
        .collect())
}

/// Discogs writes durations as "4:33", and leaves them blank often enough that
/// a missing one has to be a normal answer rather than a parse failure.
fn parse_duration(raw: Option<&str>) -> Option<i64> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let mut secs = 0i64;
    for part in raw.split(':') {
        secs = secs * 60 + part.parse::<i64>().ok()?;
    }
    Some(secs).filter(|s| *s > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The format strings MusicBrainz and Discogs actually emit.
    ///
    /// `"SACD"` and `"Hybrid SACD"` are the reason this function tests for SACD
    /// first: both contain the substring `"cd"`, so a naive order files every
    /// SACD as a CD — a wrong answer that looks entirely reasonable on the
    /// shelf and would never be questioned.
    #[test]
    fn a_medium_declares_its_own_kind() {
        for (format, want) in [
            ("CD", Some("cd")),
            ("Enhanced CD", Some("cd")),
            ("CD-R", Some("cd")),
            ("SACD", Some("sacd")),
            ("Hybrid SACD", Some("sacd")),
            ("12\" Vinyl", Some("vinyl")),
            ("Vinyl", Some("vinyl")),
            ("LP", Some("vinyl")),
            ("Cassette", Some("cassette")),
            ("Digital Media", None),
            ("", None),
        ] {
            assert_eq!(kind_from_format(Some(format)), want, "format {format:?}");
        }
        assert_eq!(kind_from_format(None), None);
    }

    /// The real case that prompted this: a five-CD box set filed as vinyl.
    /// MusicBrainz reports every medium of 940dce49… as `CD`.
    #[test]
    fn a_five_cd_box_set_is_not_vinyl() {
        assert_eq!(kind_from_format(Some("CD")), Some("cd"));
        // And the shelf must not then try to split it into sides.
        assert!(is_disc(Some("CD")));
    }

    /// An explicit side letter is an editor stating the answer, so it wins
    /// outright — no guessing on top of known data.
    #[test]
    fn explicit_side_letters_are_used_as_is() {
        let numbers: Vec<String> = ["A1", "A2", "B1", "B2"].iter().map(|s| s.to_string()).collect();
        let durations = vec![Some(200), Some(200), Some(200), Some(200)];
        assert_eq!(assign_sides(&numbers, &durations, 2), vec!["A", "A", "B", "B"]);
    }

    /// Most vinyl releases number tracks 1..n with no side at all — including
    /// the pressing of The Dark Side of the Moon this was checked against — so
    /// the fallback is the common path, not the edge case.
    #[test]
    fn a_flat_tracklist_splits_on_playing_time() {
        let numbers: Vec<String> = (1..=6).map(|i| i.to_string()).collect();
        // One long closer: splitting by *count* would put three tracks a side
        // and leave side A at 16 minutes against side B's 26.
        let durations = vec![Some(300), Some(300), Some(300), Some(300), Some(300), Some(900)];
        let sides = assign_sides(&numbers, &durations, 2);

        // The property that matters is balanced *time*, not an equal number of
        // tracks: five short ones a side against one long one is what a
        // mastering engineer would actually do here.
        let secs = |want: &str| -> i64 {
            sides
                .iter()
                .zip(&durations)
                .filter(|(s, _)| s.as_str() == want)
                .map(|(_, d)| d.unwrap_or(0))
                .sum()
        };
        assert_eq!(secs("A"), 1200);
        assert_eq!(secs("B"), 1200);
        // Splitting by count would have given 900s against 1500s.
        assert_ne!(
            sides.iter().filter(|s| *s == "A").count(),
            sides.iter().filter(|s| *s == "B").count()
        );
    }

    /// A double LP is four sides, not two.
    #[test]
    fn more_than_two_sides_are_supported() {
        let numbers: Vec<String> = (1..=8).map(|i| i.to_string()).collect();
        let durations = vec![Some(300); 8];
        let sides = assign_sides(&numbers, &durations, 4);
        assert_eq!(sides.first().map(String::as_str), Some("A"));
        assert_eq!(sides.last().map(String::as_str), Some("D"));
        // Every side used, none empty.
        for want in ["A", "B", "C", "D"] {
            assert!(sides.iter().any(|s| s == want), "side {want} missing");
        }
    }

    fn t(side: &str, pos: i64, title: &str, secs: Option<i64>) -> PhysicalTrack {
        PhysicalTrack {
            side: side.into(),
            position: pos,
            title: title.into(),
            artist: None,
            duration_secs: secs,
            disc: None,
        }
    }

    /// Durations are matched by title, never by position.
    ///
    /// The whole reason to look a release up again is that the source which
    /// catalogued it had no durations — so the answer comes from a *different*
    /// pressing, which may carry a bonus track or order a side differently.
    /// Lining the lists up by index would hand one track's running time to
    /// another, invisibly.
    #[test]
    fn durations_are_matched_by_title_not_by_position() {
        let shelf = [
            t("A", 1, "Going Under", None),
            t("A", 2, "Bring Me To Life", None),
            t("A", 3, "Tourniquet", None),
        ];
        // The found pressing opens with a track the shelf copy does not have,
        // so every index is shifted by one.
        let found = [
            t("A", 1, "Intro", Some(60)),
            t("A", 2, "Going Under", Some(214)),
            t("A", 3, "Bring Me To Life", Some(236)),
            t("A", 4, "Tourniquet", Some(278)),
        ];
        assert_eq!(
            match_durations(&shelf, &found),
            vec![Some(214), Some(236), Some(278)],
            "a shifted tracklist must not shift the durations with it"
        );
    }

    /// A trailing qualifier is a labelling difference, not a different song —
    /// but it must never beat a title that agrees outright.
    #[test]
    fn a_qualifier_is_matched_through_but_never_preferred() {
        let shelf = [t("A", 1, "Haunted", None), t("A", 2, "My Immortal", None)];
        let found = [
            t("A", 1, "My Immortal (Band Version)", Some(263)),
            t("A", 2, "Haunted", Some(247)),
            // The exact title exists further down; it has to win over the
            // qualifier-stripped match above it.
            t("A", 3, "My Immortal", Some(255)),
        ];
        assert_eq!(match_durations(&shelf, &found), vec![Some(247), Some(255)]);
    }

    /// Two tracks of one name — a reprise — must not both take the same entry,
    /// and a track nothing matches comes back unknown rather than guessed.
    #[test]
    fn one_found_track_is_claimed_once_and_a_miss_stays_unknown() {
        let shelf = [
            t("A", 1, "Whisper", None),
            t("A", 2, "Whisper", None),
            t("A", 3, "Not On That Pressing", None),
        ];
        let found = [t("A", 1, "Whisper", Some(320)), t("A", 2, "Whisper", Some(99))];
        assert_eq!(match_durations(&shelf, &found), vec![Some(320), Some(99), None]);
    }

    /// A found track with no duration of its own is no answer at all — it must
    /// be skipped rather than claimed, or it would block the real match behind
    /// it and report a miss.
    #[test]
    fn a_durationless_match_does_not_claim_the_slot() {
        let shelf = [t("A", 1, "Imaginary", None)];
        let found = [t("A", 1, "Imaginary", None), t("B", 1, "Imaginary", Some(275))];
        assert_eq!(match_durations(&shelf, &found), vec![Some(275)]);
    }

    /// An unbalanced double LP must break where the *records* break.
    ///
    /// This is the bug the whole-release split had: flattened, a 46-minute
    /// first record and a 22-minute second come to 68 minutes, so four even
    /// sides are 17 minutes each and side C opens with tracks that are
    /// physically on disc 1. Nothing errors — the tracklist just describes an
    /// object that does not exist, and the shelf then offers to remind you to
    /// flip to a side that is not there.
    ///
    /// Asserted by counting how many of the first record's tracks escaped onto
    /// a second-record side, because that is the symptom rather than any
    /// particular letter.
    #[test]
    fn an_unbalanced_double_lp_breaks_where_the_records_do() {
        // Disc 1: six 7:40 tracks (46:00). Disc 2: three 7:20 tracks (22:00).
        let d1: Vec<String> = (1..=6).map(|i| i.to_string()).collect();
        let d2: Vec<String> = (1..=3).map(|i| i.to_string()).collect();
        let per_disc = vec![(d1.clone(), vec![Some(460); 6]), (d2.clone(), vec![Some(440); 3])];

        let sides = assign_sides_by_disc(&per_disc);
        assert_eq!(sides.len(), 9);

        // The first record is sides A and B; the second is C and D. Nothing
        // from disc 1 may land on C or D, which is precisely what the
        // whole-release split got wrong.
        let (first, second) = sides.split_at(6);
        assert!(
            first.iter().all(|s| s == "A" || s == "B"),
            "disc 1 spilled onto the second record: {first:?}"
        );
        assert!(
            second.iter().all(|s| s == "C" || s == "D"),
            "disc 2 was lettered as though it were the first record: {second:?}"
        );

        // And the old behaviour really did get it wrong, so this test would
        // pass for the wrong reason if that ever stopped being true.
        let flat: Vec<String> = d1.iter().chain(d2.iter()).cloned().collect();
        let mut flat_durations = vec![Some(460); 6];
        flat_durations.extend(vec![Some(440); 3]);
        let whole = assign_sides(&flat, &flat_durations, 4);
        assert!(
            whole[..6].iter().any(|s| s == "C" || s == "D"),
            "the flat split was expected to spill disc 1 onto the second record"
        );
    }

    /// A disc an editor actually lettered keeps its letters and is **not**
    /// shifted along. `C1`…`D4` is already absolute, and offsetting it would
    /// push a correctly-described second record onto sides E and F.
    #[test]
    fn a_lettered_disc_is_not_offset_again() {
        let per_disc = vec![
            (vec!["A1".into(), "A2".into(), "B1".into(), "B2".into()], vec![Some(300); 4]),
            (vec!["C1".into(), "C2".into(), "D1".into(), "D2".into()], vec![Some(300); 4]),
        ];
        assert_eq!(assign_sides_by_disc(&per_disc), vec!["A", "A", "B", "B", "C", "C", "D", "D"]);
    }

    /// A single LP has to come out exactly as it always did — this change is
    /// about multi-disc sets and must be invisible to everything else.
    #[test]
    fn one_record_is_unchanged_by_the_per_disc_split() {
        let numbers: Vec<String> = (1..=8).map(|i| i.to_string()).collect();
        let durations = vec![Some(300); 8];
        assert_eq!(
            assign_sides_by_disc(&[(numbers.clone(), durations.clone())]),
            assign_sides(&numbers, &durations, 2)
        );
    }

    /// No durations anywhere is normal on Discogs, and must still produce
    /// something usable rather than putting everything on side A.
    #[test]
    fn a_tracklist_with_no_durations_still_splits() {
        let numbers: Vec<String> = (1..=4).map(|i| i.to_string()).collect();
        let durations = vec![None, None, None, None];
        let sides = assign_sides(&numbers, &durations, 2);
        assert_eq!(sides, vec!["A", "A", "B", "B"]);
    }

    /// Hits the real MusicBrainz API — run by hand:
    /// `cargo test --bin tapedeck mb_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn mb_live_lookup() {
        let c = musicbrainz_release("c7bb40b0-ec9e-36bd-a35e-8495243e288b")
            .await
            .expect("request ok")
            .expect("release found");
        println!("\n{} — {} ({:?}) {:?}", c.artist, c.title, c.year, c.format);
        println!("barcode={:?} catno={:?}", c.barcode, c.catalog_number);
        for t in &c.tracks {
            println!(
                "  {}{}  {:>4}s  {}",
                t.side,
                t.position,
                t.duration_secs.unwrap_or(0),
                t.title
            );
        }
        assert!(!c.tracks.is_empty());
        assert!(c.tracks.iter().any(|t| t.side == "A"));
        assert!(c.tracks.iter().any(|t| t.side == "B"));
    }

    #[test]
    fn discogs_durations_parse() {
        assert_eq!(parse_duration(Some("4:33")), Some(273));
        assert_eq!(parse_duration(Some("1:02:03")), Some(3723));
        assert_eq!(parse_duration(Some("")), None);
        assert_eq!(parse_duration(None), None);
        assert_eq!(parse_duration(Some("0:00")), None);
    }

    /// Five images lifted verbatim out of the live archive's answer for
    /// `6940fcdf-6e70-4764-8782-18e2c1e600de` on 2026-08-09 — the release that
    /// prompted this, which carries 29 images of which 19 are booklet pages.
    /// Chosen for the four shapes that are easy to get wrong rather than for
    /// coverage: a plain front, a front that is *also* a sticker, a type
    /// (`Panel`) that appears in no documentation, and a label shot that also
    /// claims `Matrix/Runout`.
    const CAA_INDEX: &str = r#"{"images": [
  {"approved": true, "back": false, "comment": "", "edit": 137479011, "front": true, "id": 43788051983, "image": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788051983.jpg", "thumbnails": {"1200": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788051983-1200.jpg", "250": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788051983-250.jpg", "500": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788051983-500.jpg", "large": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788051983-500.jpg", "small": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788051983-250.jpg"}, "types": ["Front"]},
  {"approved": true, "back": false, "comment": "", "edit": 137479198, "front": false, "id": 43788065411, "image": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788065411.jpg", "thumbnails": {"1200": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788065411-1200.jpg", "250": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788065411-250.jpg", "500": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788065411-500.jpg", "large": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788065411-500.jpg", "small": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788065411-250.jpg"}, "types": ["Front", "Sticker"]},
  {"approved": true, "back": false, "comment": "inner left panel", "edit": 137479013, "front": false, "id": 43788052682, "image": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788052682.jpg", "thumbnails": {"1200": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788052682-1200.jpg", "250": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788052682-250.jpg", "500": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788052682-500.jpg", "large": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788052682-500.jpg", "small": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788052682-250.jpg"}, "types": ["Panel"]},
  {"approved": true, "back": false, "comment": "disc 1: AN92540-01 manufactured by optimal media GmbH", "edit": 137479046, "front": false, "id": 43788054709, "image": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788054709.jpg", "thumbnails": {"1200": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788054709-1200.jpg", "250": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788054709-250.jpg", "500": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788054709-500.jpg", "large": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788054709-500.jpg", "small": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788054709-250.jpg"}, "types": ["Medium", "Matrix/Runout"]},
  {"approved": true, "back": false, "comment": "page 1 of 20", "edit": 137479075, "front": false, "id": 43788056657, "image": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788056657.jpg", "thumbnails": {"1200": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788056657-1200.jpg", "250": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788056657-250.jpg", "500": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788056657-500.jpg", "large": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788056657-500.jpg", "small": "https://coverartarchive.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de/43788056657-250.jpg"}, "types": ["Booklet"]}
], "release": "https://musicbrainz.org/release/6940fcdf-6e70-4764-8782-18e2c1e600de"}"#;

    #[test]
    fn a_real_cover_art_index_parses() {
        let images = parse_cover_art(CAA_INDEX).expect("the archive's own JSON");
        assert_eq!(images.len(), 5);

        let kinds: Vec<&str> = images.iter().map(|i| i.kind.as_str()).collect();
        assert_eq!(kinds, vec!["front", "front", "panel", "medium", "booklet"]);

        // A 64-bit id must survive as text. Parsed as an i32 this would
        // overflow; parsed as a float it would come back as 43788051983.0 and
        // never match the row it wrote.
        assert_eq!(images[0].id, "43788051983");
        assert!(images[0].thumb_url.as_deref().unwrap().ends_with("-250.jpg"));

        // The editor's note is the only thing telling nineteen booklet pages
        // apart, so it has to survive; an empty one must not become "".
        assert_eq!(images[0].caption, None);
        assert_eq!(images[4].caption.as_deref(), Some("page 1 of 20"));

        // The archive's own order is kept — it puts the cover first.
        assert_eq!(images.iter().map(|i| i.position).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    }

    /// The two multi-type cases, stated on their own because a first-wins rule
    /// passes every other assertion in the test above.
    #[test]
    fn the_most_structural_type_wins_when_an_image_claims_several() {
        assert_eq!(image_kind(&["Front".into(), "Sticker".into()]), "front");
        assert_eq!(image_kind(&["Sticker".into(), "Front".into()]), "front");
        assert_eq!(image_kind(&["Medium".into(), "Matrix/Runout".into()]), "medium");
        assert_eq!(image_kind(&["Matrix/Runout".into()]), "matrix");
    }

    /// The archive's vocabulary grows — `Panel` is on the release this was
    /// built against and appears in none of the documentation. An unknown type
    /// is slugified rather than swallowed into "other", because "inner left
    /// panel" filed under *panel* is a usable shelf heading and filed under
    /// *other* is not.
    #[test]
    fn an_unknown_type_keeps_its_name() {
        assert_eq!(image_kind(&["Panel".into()]), "panel");
        assert_eq!(image_kind(&["Raw/Unedited".into()]), "raw");
        assert_eq!(image_kind(&[]), "other");
        assert_eq!(image_kind(&["!!".into()]), "other");
    }

    /// A release with no art at all is a 404 from the archive and the common
    /// case for anything obscure, so it must read as "nothing here" rather than
    /// as a failure. This pins the parse half; the status half is in
    /// `cover_art_archive`.
    #[test]
    fn an_empty_index_is_not_an_error() {
        assert!(parse_cover_art(r#"{"images": []}"#).unwrap().is_empty());
        assert!(parse_cover_art("{}").unwrap().is_empty());
    }

    /// Live, ignored by default like the MusicBrainz lookup beside it.
    #[tokio::test]
    #[ignore]
    async fn caa_live_lookup() {
        let images = cover_art_archive("6940fcdf-6e70-4764-8782-18e2c1e600de").await.unwrap();
        assert!(images.len() > 20, "expected the full sleeve, got {}", images.len());
        assert!(images.iter().any(|i| i.kind == "booklet"));
        assert_eq!(
            cover_art_archive("00000000-0000-0000-0000-000000000000").await.unwrap().len(),
            0
        );
    }
}
