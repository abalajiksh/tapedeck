mod emby;
mod jellyfin;
pub mod mediabrowser;
mod navidrome;
pub mod plex;
pub mod roon;

use crate::models::Play;
use async_trait::async_trait;
use std::any::Any;

#[async_trait]
pub trait MusicSource: Send + Sync {
    fn name(&self) -> &str;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    async fn fetch_new_plays(
        &mut self,
        last_checked: u64,
    ) -> Result<Vec<Play>, Box<dyn std::error::Error>>;

    /// Whether this source has been told something changed since the last ask,
    /// and should be polled now rather than at its next interval.
    ///
    /// Defaults to false, which is every polled source: they have no way to
    /// know anything until they ask. Roon overrides it — its zone subscription
    /// rings the engine's bell and raises this, so a track change reaches the
    /// deck in about a second instead of up to five.
    ///
    /// A trait method with a default rather than a downcast, for the reason
    /// `ScrobbleSink::update_now_playing` is one: any push-shaped source has
    /// this concept, and the `Any` downcasting elsewhere is reserved for
    /// capabilities the traits genuinely cannot express. Consuming (`take`)
    /// rather than peeking is what stops one notification polling forever.
    fn take_wakeup(&mut self) -> bool {
        false
    }
}

/// Jellyfin and Emby share one client — they are one protocol with a prefix on
/// it, distinguished by [`mediabrowser::Flavour`]. They are deliberately *not*
/// two Rust types: the pipeline already dispatches on `user_sources.kind`
/// before it downcasts, so a second type would buy nothing and would mean two
/// copies of a state machine that took three attempts to get right.
pub use mediabrowser::{Flavour, MediaBrowserSource};
pub use navidrome::NavidromeSource;
pub use plex::PlexFilters;
pub use plex::PlexSource;
pub use roon::RoonSource;
