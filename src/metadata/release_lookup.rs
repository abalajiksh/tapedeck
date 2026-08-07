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

    // A double LP is four sides, so the side count comes from the number of
    // media rather than being assumed to be two.
    let media_count = r.media.len().max(1);
    let flat: Vec<&Track> = r.media.iter().flat_map(|m| m.tracks.iter()).collect();
    let numbers: Vec<String> = flat.iter().map(|t| t.number.clone().unwrap_or_default()).collect();
    let durations: Vec<Option<i64>> = flat.iter().map(|t| t.length.map(|ms| ms / 1000)).collect();
    // A disc is numbered by disc, not lettered by side — see `is_disc`.
    let side_of = if is_disc(format.as_deref()) {
        disc_sides(&r.media.iter().map(|m| m.tracks.len()).collect::<Vec<_>>())
    } else {
        assign_sides(&numbers, &durations, media_count * 2)
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
}
