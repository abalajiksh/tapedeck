mod lastfm;
mod listenbrainz;
pub use lastfm::LastFmSink;
pub use listenbrainz::ListenBrainzSink;

use crate::models::Play;
use async_trait::async_trait;
use std::any::Any;

#[async_trait]
pub trait ScrobbleSink: Any + Send + Sync {
    fn name(&self) -> &str;
    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>>;
    fn as_any(&self) -> &dyn Any;
}
