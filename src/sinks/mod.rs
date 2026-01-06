mod lastfm;
mod listenbrainz;
pub use lastfm::LastFmSink;
pub use listenbrainz::ListenBrainzSink;

use crate::models::Play;
use async_trait::async_trait;

#[async_trait]
pub trait ScrobbleSink {
    fn name(&self) -> &str;
    async fn scrobble(&self, plays: &[Play]) -> Result<(), Box<dyn std::error::Error>>;
}
