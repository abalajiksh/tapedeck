//! Long-form prose about an artist or a release, in preference order.
//!
//! MusicBrainz has no long-form text at all, so the album and artist pages had
//! nowhere to get a description. The chain, best coverage first:
//!
//! 1. **Last.fm** — `artist.getInfo` / `album.getInfo`. Widest coverage by far,
//!    and Tapedeck already ships an application key. Its text is **CC BY-SA**,
//!    so the attribution line is not optional; it is returned alongside the
//!    prose and every surface that shows one must show the other.
//! 2. **TheAudioDB** — keyed by the MBID we already store, so no name matching.
//!    The free key `2` is rate-limited but real; a Patreon key raises it.
//! 3. **Apple Music** — editorial notes, album only. Needs a developer token,
//!    so it reports "not configured" and steps aside rather than erroring.
//! 4. **MusicBrainz** — annotations and disambiguation. Rarely prose, but it is
//!    the only one that knows the obscure things, so it answers last.
//!
//! Server-side only, like the artwork chain: the keys would leak, the browser
//! would be blocked by CORS, and each user's page views would multiply the rate
//! limit by the number of open tabs.

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use tracing::debug;

/// What a provider found. `text` is plain prose — any markup the source wrapped
/// it in has already been stripped.
#[derive(Debug, Clone, PartialEq)]
pub struct Bio {
    pub text: String,
    /// Licence / credit line. **Mandatory** to display when present; Last.fm's
    /// text is CC BY-SA and showing it without credit breaks the licence.
    pub attribution: Option<String>,
    pub provider: &'static str,
    /// Where a reader can go for the full text, when the provider offers one.
    pub url: Option<String>,
}

/// Artist or release — the two things worth describing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BioKind {
    Artist,
    Album,
}

impl BioKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Album => "album",
        }
    }
}

/// What the chain is asked about. Both a name and an MBID are carried because
/// the providers disagree on which they can use: Last.fm matches on names,
/// TheAudioDB only on MBIDs.
#[derive(Debug, Clone)]
pub struct BioQuery {
    pub kind: BioKind,
    /// Album title, or artist name when `kind` is `Artist`.
    pub name: String,
    /// The artist. Empty for an artist query.
    pub artist: String,
    pub mbid: Option<String>,
}

#[async_trait]
pub trait BioProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// `Ok(None)` means "nothing here, try the next one". `Err` means this
    /// provider broke, and the chain still continues — one service being down
    /// must not deny the page a description another service has.
    async fn lookup(
        &self,
        q: &BioQuery,
    ) -> Result<Option<Bio>, Box<dyn std::error::Error + Send + Sync>>;
}

/// Strip HTML tags and collapse whitespace.
///
/// Providers return prose wrapped in varying amounts of markup, and it is
/// rendered as text — never as `{@html}` — so anything tag-shaped is removed
/// here rather than trusted downstream.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Last.fm appends a "Read more on Last.fm" link to every bio. It's a link into
/// their site, not part of the description, and it reads as garbage once the
/// markup is stripped — so it goes, along with the trailing licence boilerplate
/// that sometimes comes with it.
fn strip_lastfm_tail(s: &str) -> String {
    let mut text = s.to_string();
    for marker in ["Read more on Last.fm", "<a href=\"https://www.last.fm"] {
        if let Some(pos) = text.find(marker) {
            text.truncate(pos);
        }
    }
    // Whitespace only. Trimming trailing punctuation here would eat the full
    // stop off the last real sentence, which is the far more common case.
    text.trim().to_string()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

// ── Last.fm ──

pub struct LastFmBio {
    api_key: String,
    http: reqwest::Client,
}

impl LastFmBio {
    pub fn new(api_key: String) -> Self {
        Self { api_key, http: client() }
    }
}

#[derive(Deserialize)]
struct LfmArtistResp {
    artist: Option<LfmArtist>,
}
#[derive(Deserialize)]
struct LfmArtist {
    url: Option<String>,
    bio: Option<LfmBioText>,
}
#[derive(Deserialize)]
struct LfmAlbumResp {
    album: Option<LfmAlbum>,
}
#[derive(Deserialize)]
struct LfmAlbum {
    url: Option<String>,
    wiki: Option<LfmBioText>,
}
#[derive(Deserialize)]
struct LfmBioText {
    content: Option<String>,
    summary: Option<String>,
}

#[async_trait]
impl BioProvider for LastFmBio {
    fn name(&self) -> &'static str {
        "lastfm"
    }

    async fn lookup(
        &self,
        q: &BioQuery,
    ) -> Result<Option<Bio>, Box<dyn std::error::Error + Send + Sync>> {
        let mut params: Vec<(&str, String)> = vec![
            ("api_key", self.api_key.clone()),
            ("format", "json".into()),
            ("autocorrect", "1".into()),
        ];

        let raw = match q.kind {
            BioKind::Artist => {
                params.push(("method", "artist.getinfo".into()));
                if let Some(m) = q.mbid.as_deref().filter(|m| !m.is_empty()) {
                    params.push(("mbid", m.to_string()));
                } else {
                    params.push(("artist", q.name.clone()));
                }
                let resp: LfmArtistResp = self
                    .http
                    .get("https://ws.audioscrobbler.com/2.0/")
                    .query(&params)
                    .send()
                    .await?
                    .json()
                    .await?;
                let a = resp.artist;
                let url = a.as_ref().and_then(|x| x.url.clone());
                let text = a.and_then(|x| x.bio).and_then(|b| b.content.or(b.summary));
                (text, url)
            }
            BioKind::Album => {
                params.push(("method", "album.getinfo".into()));
                if let Some(m) = q.mbid.as_deref().filter(|m| !m.is_empty()) {
                    params.push(("mbid", m.to_string()));
                } else {
                    params.push(("artist", q.artist.clone()));
                    params.push(("album", q.name.clone()));
                }
                let resp: LfmAlbumResp = self
                    .http
                    .get("https://ws.audioscrobbler.com/2.0/")
                    .query(&params)
                    .send()
                    .await?
                    .json()
                    .await?;
                let a = resp.album;
                let url = a.as_ref().and_then(|x| x.url.clone());
                let text = a.and_then(|x| x.wiki).and_then(|b| b.content.or(b.summary));
                (text, url)
            }
        };

        let (text, url) = raw;
        let Some(text) = text else { return Ok(None) };
        let cleaned = strip_html(&strip_lastfm_tail(&text));
        if cleaned.len() < 20 {
            return Ok(None);
        }
        Ok(Some(Bio {
            text: cleaned,
            // Not optional. See the module docs.
            attribution: Some("Description via Last.fm · CC BY-SA".into()),
            provider: "lastfm",
            url,
        }))
    }
}

// ── TheAudioDB ──

pub struct AudioDbBio {
    key: String,
    http: reqwest::Client,
}

impl AudioDbBio {
    /// `2` is TheAudioDB's public test key: rate-limited, but real and needing
    /// no signup. A Patreon key can be supplied instead.
    pub fn new(key: Option<String>) -> Self {
        Self { key: key.unwrap_or_else(|| "2".into()), http: client() }
    }
}

#[derive(Deserialize)]
struct AdbArtists {
    artists: Option<Vec<AdbArtist>>,
}
#[derive(Deserialize)]
struct AdbArtist {
    #[serde(rename = "strBiographyEN")]
    biography: Option<String>,
}
#[derive(Deserialize)]
struct AdbAlbums {
    album: Option<Vec<AdbAlbum>>,
}
#[derive(Deserialize)]
struct AdbAlbum {
    #[serde(rename = "strDescriptionEN")]
    description: Option<String>,
}

#[async_trait]
impl BioProvider for AudioDbBio {
    fn name(&self) -> &'static str {
        "audiodb"
    }

    async fn lookup(
        &self,
        q: &BioQuery,
    ) -> Result<Option<Bio>, Box<dyn std::error::Error + Send + Sync>> {
        // Keyed by MBID only — without one there is nothing to ask.
        let Some(mbid) = q.mbid.as_deref().filter(|m| !m.trim().is_empty()) else {
            return Ok(None);
        };

        let text = match q.kind {
            BioKind::Artist => {
                let url = format!(
                    "https://www.theaudiodb.com/api/v1/json/{}/artist-mb.php?i={}",
                    self.key, mbid
                );
                let r: AdbArtists = self.http.get(url).send().await?.json().await?;
                r.artists.and_then(|v| v.into_iter().next()).and_then(|a| a.biography)
            }
            BioKind::Album => {
                let url = format!(
                    "https://www.theaudiodb.com/api/v1/json/{}/album-mb.php?i={}",
                    self.key, mbid
                );
                let r: AdbAlbums = self.http.get(url).send().await?.json().await?;
                r.album.and_then(|v| v.into_iter().next()).and_then(|a| a.description)
            }
        };

        let Some(text) = text else { return Ok(None) };
        let cleaned = strip_html(&text);
        if cleaned.len() < 20 {
            return Ok(None);
        }
        Ok(Some(Bio {
            text: cleaned,
            attribution: Some("Description via TheAudioDB".into()),
            provider: "audiodb",
            url: None,
        }))
    }
}

// ── Apple Music ──

/// Editorial notes from the Apple Music catalogue.
///
/// Needs a developer token (a signed JWT) and a storefront, neither of which
/// Tapedeck can generate on the user's behalf. Unconfigured it answers `None`
/// immediately — "not configured" is a normal state here, not an error, so the
/// chain simply moves on.
pub struct AppleMusicBio {
    token: Option<String>,
    storefront: String,
    http: reqwest::Client,
}

impl AppleMusicBio {
    pub fn new(token: Option<String>, storefront: Option<String>) -> Self {
        Self { token, storefront: storefront.unwrap_or_else(|| "us".into()), http: client() }
    }

    pub fn is_configured(&self) -> bool {
        self.token.as_deref().is_some_and(|t| !t.trim().is_empty())
    }
}

#[derive(Deserialize)]
struct AmSearch {
    results: Option<AmResults>,
}
#[derive(Deserialize)]
struct AmResults {
    albums: Option<AmAlbums>,
}
#[derive(Deserialize)]
struct AmAlbums {
    data: Option<Vec<AmAlbum>>,
}
#[derive(Deserialize)]
struct AmAlbum {
    attributes: Option<AmAttributes>,
}
#[derive(Deserialize)]
struct AmAttributes {
    #[serde(rename = "editorialNotes")]
    editorial_notes: Option<AmNotes>,
    url: Option<String>,
}
#[derive(Deserialize)]
struct AmNotes {
    standard: Option<String>,
    short: Option<String>,
}

#[async_trait]
impl BioProvider for AppleMusicBio {
    fn name(&self) -> &'static str {
        "apple"
    }

    async fn lookup(
        &self,
        q: &BioQuery,
    ) -> Result<Option<Bio>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(token) = self.token.as_deref().filter(|t| !t.trim().is_empty()) else {
            debug!("Apple Music not configured; skipping");
            return Ok(None);
        };
        // The free catalogue has editorial notes for albums only.
        if q.kind != BioKind::Album {
            return Ok(None);
        }

        let term = format!("{} {}", q.artist, q.name);
        let url = format!("https://api.music.apple.com/v1/catalog/{}/search", self.storefront);
        let resp: AmSearch = self
            .http
            .get(url)
            .bearer_auth(token)
            .query(&[("term", term.as_str()), ("types", "albums"), ("limit", "1")])
            .send()
            .await?
            .json()
            .await?;

        let attrs = resp
            .results
            .and_then(|r| r.albums)
            .and_then(|a| a.data)
            .and_then(|d| d.into_iter().next())
            .and_then(|a| a.attributes);
        let Some(attrs) = attrs else { return Ok(None) };
        let text = attrs.editorial_notes.and_then(|n| n.standard.or(n.short));
        let Some(text) = text else { return Ok(None) };

        let cleaned = strip_html(&text);
        if cleaned.len() < 20 {
            return Ok(None);
        }
        Ok(Some(Bio {
            text: cleaned,
            attribution: Some("Description via Apple Music".into()),
            provider: "apple",
            url: attrs.url,
        }))
    }
}

// ── MusicBrainz ──

/// Annotations and disambiguation comments. Rarely a real description, but it
/// is the only source that knows the obscure records, so it answers last rather
/// than not at all.
pub struct MusicBrainzBio {
    http: reqwest::Client,
}

impl Default for MusicBrainzBio {
    fn default() -> Self {
        Self::new()
    }
}

impl MusicBrainzBio {
    pub fn new() -> Self {
        Self { http: client() }
    }
}

#[derive(Deserialize)]
struct MbEntity {
    annotation: Option<String>,
    disambiguation: Option<String>,
}

#[async_trait]
impl BioProvider for MusicBrainzBio {
    fn name(&self) -> &'static str {
        "musicbrainz"
    }

    async fn lookup(
        &self,
        q: &BioQuery,
    ) -> Result<Option<Bio>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(mbid) = q.mbid.as_deref().filter(|m| !m.trim().is_empty()) else {
            return Ok(None);
        };
        let entity = match q.kind {
            BioKind::Artist => "artist",
            BioKind::Album => "release",
        };
        let url =
            format!("https://musicbrainz.org/ws/2/{}/{}?fmt=json&inc=annotation", entity, mbid);
        let e: MbEntity = self.http.get(url).send().await?.json().await?;

        let text = e
            .annotation
            .filter(|a| !a.trim().is_empty())
            .or_else(|| e.disambiguation.filter(|d| d.trim().len() > 20));
        let Some(text) = text else { return Ok(None) };

        let cleaned = strip_html(&text);
        if cleaned.len() < 20 {
            return Ok(None);
        }
        Ok(Some(Bio {
            text: cleaned,
            attribution: Some("Description via MusicBrainz · CC0".into()),
            provider: "musicbrainz",
            url: Some(format!("https://musicbrainz.org/{}/{}", entity, mbid)),
        }))
    }
}

// ── The chain ──

/// Boxed provider, aliased because it appears in `AppState` and in every
/// signature that walks the chain.
pub type BioProviderBox = Box<dyn BioProvider>;

/// The providers in preference order, built from whatever is configured.
pub fn default_chain(
    audiodb_key: Option<String>,
    apple_token: Option<String>,
    apple_storefront: Option<String>,
) -> Vec<BioProviderBox> {
    vec![
        Box::new(LastFmBio::new(crate::sinks::LASTFM_API_KEY.to_string())),
        Box::new(AudioDbBio::new(audiodb_key)),
        Box::new(AppleMusicBio::new(apple_token, apple_storefront)),
        Box::new(MusicBrainzBio::new()),
    ]
}

/// First real answer wins. A provider erroring is logged and skipped — one
/// service being down must not deny the page a description another one has.
pub async fn fetch_bio(providers: &[BioProviderBox], q: &BioQuery) -> Option<Bio> {
    for p in providers {
        match p.lookup(q).await {
            Ok(Some(bio)) => {
                debug!("Bio for {} '{}' from {}", q.kind.as_str(), q.name, p.name());
                return Some(bio);
            }
            Ok(None) => continue,
            Err(e) => {
                debug!("Bio provider {} failed for '{}': {}", p.name(), q.name, e);
                continue;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trailing "Read more on Last.fm" anchor is theirs, not the
    /// description's, and it reads as garbage once tags are stripped.
    #[test]
    fn lastfm_tail_and_markup_are_removed() {
        let raw = "A <b>seventh</b> album, released in 2007. \
                   <a href=\"https://www.last.fm/music/Radiohead\">Read more on Last.fm</a>.";
        let cleaned = strip_html(&strip_lastfm_tail(raw));
        assert_eq!(cleaned, "A seventh album, released in 2007.");
        assert!(!cleaned.contains("Last.fm"));
    }

    #[test]
    fn entities_and_whitespace_are_normalised() {
        assert_eq!(strip_html("Simon &amp; Garfunkel\n\n  live"), "Simon & Garfunkel live");
    }

    /// Apple Music without a token is a normal state, not a failure.
    #[test]
    fn apple_music_reports_unconfigured() {
        assert!(!AppleMusicBio::new(None, None).is_configured());
        assert!(!AppleMusicBio::new(Some("  ".into()), None).is_configured());
        assert!(AppleMusicBio::new(Some("tok".into()), None).is_configured());
    }
}
