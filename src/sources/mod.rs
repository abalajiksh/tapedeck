pub mod plex;
// pub mod navidrome;
// pub mod jellyfin;

use async_trait::async_trait;
use crate::models::Play;

#[async_trait]
pub trait MusicSource: Send + Sync {
    fn name(&self) -> &str;
    async fn fetch_new_plays(&mut self, last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>>;

    // Helper for downcasting
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

// Re-export Plex types
pub use plex::{PlexSource, PlexFilters};
