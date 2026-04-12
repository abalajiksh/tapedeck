mod plex;

use async_trait::async_trait;
use crate::models::Play;
use std::any::Any;

#[async_trait]
pub trait MusicSource: Send + Sync {
    fn name(&self) -> &str;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    async fn fetch_new_plays(&mut self, last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>>;
}

pub use plex::PlexSource;
pub use plex::PlexFilters;
