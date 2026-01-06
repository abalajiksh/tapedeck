// src/sources/mod.rs
use async_trait::async_trait;
use crate::models::Play;

#[async_trait]
pub trait MusicSource {
    fn name(&self) -> &str;
    async fn fetch_new_plays(&self, last_checked: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>>;
}

pub mod plex;
pub use plex::PlexSource; // Re-export!
