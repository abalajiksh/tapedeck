use crate::models::Play;
use async_trait::async_trait;

#[async_trait]
pub trait MusicSource {
    fn name(&self) -> &str;
    // Fetch plays newer than the given timestamp
    async fn fetch_new_plays(&self, last_checked_timestamp: u64) -> Result<Vec<Play>, Box<dyn std::error::Error>>;
}
