//! Where artwork and track metadata come from, in preference order.
//!
//! MusicBrainz used to be the only answer, which made it the bottleneck: its API
//! allows one request per second, so resolving artwork for a twenty-thousand
//! listen history takes the better part of a day, and most of those calls are
//! asking a public service for something the user already has on their own disk.
//!
//! The chain, cheapest and most accurate first:
//!
//! 1. **The user's own library** (Navidrome, Plex). Zero external calls, and it
//!    returns the artwork of *the actual file that was played* rather than a
//!    best guess at the release.
//! 2. **Local cache** — already handled inside the MusicBrainz client.
//! 3. **Deezer.** No auth, generous limits, good coverage of mainstream releases.
//! 4. **MusicBrainz / Cover Art Archive.** Last, for the things only it knows.
//!
//! Providers return *URLs*, never image bytes. Deezer's terms discourage
//! persisting their artwork, and library art needs the user's credentials to
//! fetch — so it's proxied on demand instead (`server/artwork.rs`).

pub mod bio;
pub mod deezer;
pub mod release_lookup;

use async_trait::async_trait;

/// What a provider can tell us about one track.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TrackArtwork {
    /// Cover image, ready to put in an `<img src>`. Either an external URL or a
    /// Tapedeck proxy path for library art the browser can't reach directly.
    pub image_url: Option<String>,
    /// Artist image, when the provider has one. MusicBrainz never does.
    pub artist_image_url: Option<String>,
    /// Release year, if the provider knows it — saves a separate lookup.
    pub release_year: Option<i32>,
    /// Which provider answered, for debugging and the inspect view.
    pub provider: &'static str,
}

impl TrackArtwork {
    pub fn is_empty(&self) -> bool {
        self.image_url.is_none() && self.artist_image_url.is_none() && self.release_year.is_none()
    }
}

#[async_trait]
pub trait ArtworkProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Look up one track. `Ok(None)` means "not found here, try the next
    /// provider"; `Err` means the provider itself failed and the chain should
    /// still continue — a Deezer outage must not stop MusicBrainz answering.
    async fn lookup(
        &self,
        artist: &str,
        title: &str,
        album: Option<&str>,
    ) -> Result<Option<TrackArtwork>, Box<dyn std::error::Error + Send + Sync>>;
}

/// Try each provider in order, returning the first real answer.
///
/// A provider erroring is logged and skipped rather than aborting: the whole
/// point of a chain is that one source being down degrades quality, not
/// availability.
pub async fn resolve(
    providers: &[Box<dyn ArtworkProvider>],
    artist: &str,
    title: &str,
    album: Option<&str>,
) -> Option<TrackArtwork> {
    for p in providers {
        match p.lookup(artist, title, album).await {
            Ok(Some(found)) if !found.is_empty() => return Some(found),
            Ok(_) => continue,
            Err(e) => {
                tracing::debug!("{} lookup failed for {} - {}: {}", p.name(), artist, title, e);
                continue;
            }
        }
    }
    None
}
