//! Deezer as an artwork and artist-image provider.
//!
//! Chosen because its public search API needs no authentication, no registered
//! application, and no per-user setup — and because its limits are generous
//! enough to backfill a large history in minutes rather than the hours
//! MusicBrainz's one-request-per-second would take.
//!
//! **We store URLs, not images.** Deezer's terms discourage persisting their
//! artwork, and their CDN URLs are stable and content-addressed, so keeping the
//! link is both lighter and better behaved than mirroring the file.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use super::{ArtworkProvider, TrackArtwork};

const SEARCH_URL: &str = "https://api.deezer.com/search";

pub struct DeezerProvider {
    client: Client,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<SearchHit>,
    /// Deezer reports quota problems in-band with HTTP 200.
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct SearchHit {
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: Option<DeezerArtist>,
    #[serde(default)]
    album: Option<DeezerAlbum>,
}

#[derive(Deserialize)]
struct DeezerArtist {
    #[serde(default)]
    name: String,
    #[serde(default)]
    picture_medium: Option<String>,
    #[serde(default)]
    picture_big: Option<String>,
}

#[derive(Deserialize)]
struct DeezerAlbum {
    #[serde(default)]
    cover_medium: Option<String>,
    #[serde(default)]
    cover_big: Option<String>,
}

impl Default for DeezerProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DeezerProvider {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent(format!("{}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Deezer's fielded search. Quoting each term stops a title containing
    /// `artist:` or a stray quote from changing the query's meaning.
    fn query(artist: &str, title: &str, album: Option<&str>) -> String {
        let esc = |s: &str| s.replace('"', " ");
        let mut q = format!(r#"artist:"{}" track:"{}""#, esc(artist), esc(title));
        // Album narrows a common title, but Deezer's album names often differ in
        // punctuation or edition, so it's only added when present.
        if let Some(a) = album.filter(|a| !a.trim().is_empty()) {
            q.push_str(&format!(r#" album:"{}""#, esc(a)));
        }
        q
    }

    /// Prefer a hit whose artist actually matches — a fielded search can still
    /// return covers and tributes, and silently attaching the wrong artwork is
    /// worse than showing none.
    fn best_hit(hits: Vec<SearchHit>, artist: &str) -> Option<SearchHit> {
        let wanted = artist.trim().to_lowercase();
        hits.into_iter().find(|h| {
            h.artist.as_ref().map(|a| a.name.trim().to_lowercase() == wanted).unwrap_or(false)
        })
    }
}

#[async_trait]
impl ArtworkProvider for DeezerProvider {
    fn name(&self) -> &'static str {
        "deezer"
    }

    async fn lookup(
        &self,
        artist: &str,
        title: &str,
        album: Option<&str>,
    ) -> Result<Option<TrackArtwork>, Box<dyn std::error::Error + Send + Sync>> {
        if artist.trim().is_empty() || title.trim().is_empty() {
            return Ok(None);
        }

        let resp = self
            .client
            .get(SEARCH_URL)
            .query(&[("q", Self::query(artist, title, album)), ("limit", "10".into())])
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err("Deezer rate limit reached".into());
        }
        if !resp.status().is_success() {
            return Err(format!("Deezer returned {}", resp.status()).into());
        }

        let body: SearchResponse = resp.json().await?;
        if let Some(e) = body.error {
            // Quota exhaustion arrives as a 200 with an error object, so it has
            // to be checked explicitly or it looks like "no results".
            return Err(format!("Deezer error: {e}").into());
        }

        let Some(hit) = Self::best_hit(body.data, artist) else {
            return Ok(None);
        };

        let image_url =
            hit.album.as_ref().and_then(|a| a.cover_big.clone().or_else(|| a.cover_medium.clone()));
        let artist_image_url = hit
            .artist
            .as_ref()
            .and_then(|a| a.picture_big.clone().or_else(|| a.picture_medium.clone()));

        let found = TrackArtwork {
            image_url,
            artist_image_url,
            release_year: None, // search hits carry no date; the album endpoint would
            provider: "deezer",
        };
        Ok((!found.is_empty()).then_some(found))
    }
}

/// `/search/artist`'s body.
#[derive(Deserialize)]
struct ArtistSearch {
    #[serde(default)]
    data: Vec<ArtistHit>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ArtistHit {
    #[serde(default)]
    name: String,
    #[serde(default)]
    picture_xl: Option<String>,
    #[serde(default)]
    picture_big: Option<String>,
}

impl DeezerProvider {
    /// Deezer's picture of an artist, as a candidate for the picker.
    ///
    /// Its own search rather than the track search the artwork chain uses: the
    /// picker knows a name and nothing else. Hits are checked with
    /// `artist_matches`, and Deezer's placeholder — a URL with an empty hash,
    /// `/images/artist//…` — is not a picture of anybody.
    pub async fn artist_pictures(
        &self,
        name: &str,
    ) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
        let resp = self
            .client
            .get("https://api.deezer.com/search/artist")
            .query(&[("q", name), ("limit", "10")])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(format!("Deezer returned {}", resp.status()).into());
        }
        let body: ArtistSearch = resp.json().await?;
        if let Some(e) = body.error {
            return Err(format!("Deezer error: {e}").into());
        }
        Ok(artist_picture_urls(body.data, name))
    }
}

fn artist_picture_urls(hits: Vec<ArtistHit>, name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for hit in hits {
        if !crate::sources::plex::artist_matches(&hit.name, name) {
            continue;
        }
        if let Some(url) = hit.picture_xl.or(hit.picture_big) {
            if !url.contains("/artist//") && !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/search/artist`: a near miss is not the artist, and Deezer's
    /// placeholder (an empty hash in the path) is not a picture.
    #[test]
    fn an_artist_search_keeps_real_pictures_of_the_right_artist() {
        let body: ArtistSearch = serde_json::from_value(serde_json::json!({ "data": [
            { "id": 1, "name": "The Marías",
              "picture_xl": "https://e-cdns-images.dzcdn.net/images/artist/abc/1000x1000-000000-80-0-0.jpg" },
            { "id": 2, "name": "The Marias",
              "picture_xl": "https://e-cdns-images.dzcdn.net/images/artist//1000x1000-000000-80-0-0.jpg" },
            { "id": 3, "name": "María Zardoya",
              "picture_xl": "https://e-cdns-images.dzcdn.net/images/artist/zzz/1000x1000.jpg" }
        ], "total": 3 }))
        .unwrap();
        let got = artist_picture_urls(body.data, "The Marías");
        assert_eq!(
            got,
            vec!["https://e-cdns-images.dzcdn.net/images/artist/abc/1000x1000-000000-80-0-0.jpg"
                .to_string()]
        );
    }

    #[test]
    fn query_is_fielded_and_quote_safe() {
        let q = DeezerProvider::query("Aphex Twin", "Xtal", Some("SAW 85-92"));
        assert!(q.contains(r#"artist:"Aphex Twin""#));
        assert!(q.contains(r#"track:"Xtal""#));
        assert!(q.contains(r#"album:"SAW 85-92""#));

        // A quote in the title would otherwise terminate the field early and
        // change what we're searching for.
        let q = DeezerProvider::query(r#"Guns N" Roses"#, r#"Don"t Cry"#, None);
        assert_eq!(q.matches('"').count() % 2, 0, "unbalanced quotes: {q}");
        assert!(!q.contains(r#"N" Roses"#));
    }

    #[test]
    fn album_is_omitted_when_absent_or_blank() {
        assert!(!DeezerProvider::query("A", "T", None).contains("album:"));
        assert!(!DeezerProvider::query("A", "T", Some("   ")).contains("album:"));
    }

    fn hit(artist: &str) -> SearchHit {
        SearchHit {
            title: "T".into(),
            artist: Some(DeezerArtist {
                name: artist.into(),
                picture_medium: Some("pic".into()),
                picture_big: None,
            }),
            album: Some(DeezerAlbum { cover_medium: Some("cov".into()), cover_big: None }),
        }
    }

    #[test]
    fn picks_the_matching_artist_not_the_first_result() {
        // Deezer happily returns tributes and karaoke versions above the real
        // thing; attaching their artwork would be silently wrong.
        let hits = vec![hit("Tribute Band"), hit("Aphex Twin")];
        let best = DeezerProvider::best_hit(hits, "Aphex Twin").expect("should match");
        assert_eq!(best.artist.unwrap().name, "Aphex Twin");
    }

    #[test]
    fn no_artist_match_means_no_answer() {
        // Better to fall through to the next provider than to guess.
        let hits = vec![hit("Someone Else")];
        assert!(DeezerProvider::best_hit(hits, "Aphex Twin").is_none());
    }

    #[test]
    fn artist_match_ignores_case_and_padding() {
        let hits = vec![hit("  aphex TWIN ")];
        assert!(DeezerProvider::best_hit(hits, "Aphex Twin").is_some());
    }
}
