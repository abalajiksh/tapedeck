use crate::models::Play;
use crate::musicbrainz::MusicBrainzClient;
use tracing::{debug, info, warn};

/// Enrich a `Play` in-place with MusicBrainz metadata + Cover Art Archive info.
/// Returns `true` if enrichment succeeded, `false` if it failed (play is still usable).
pub async fn enrich_play(
    mb_client: &MusicBrainzClient,
    play: &mut Play,
    album_hint: Option<&str>,
) -> bool {
    debug!(
        "Fetching MusicBrainz metadata for: {} - {}",
        play.artist, play.title
    );

    match mb_client
        .fetch_metadata(&play.title, &play.artist, album_hint)
        .await
    {
        Ok(metadata) => {
            let track_mbid_str = metadata
                .track_mbid
                .as_deref()
                .unwrap_or("none")
                .to_string();
            let album_mbid_str = metadata
                .album_mbid
                .as_deref()
                .unwrap_or("none")
                .to_string();
            let artist_mbid_str = metadata
                .artist_mbid
                .as_deref()
                .unwrap_or("none")
                .to_string();
            let caa_id_str = metadata
                .caa_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string());

            play.mbid_recording = metadata.track_mbid;
            play.mbid_release = metadata.album_mbid;
            play.mbid_artist = metadata.artist_mbid.as_ref().map(|id| vec![id.clone()]);
            play.caa_id = metadata.caa_id;
            play.caa_release_mbid = metadata.caa_release_mbid;

            info!(
                "✓ Enriched: {} - {} [recording: {}, release: {}, artist: {}, caa: {}]",
                play.artist,
                play.title,
                track_mbid_str,
                album_mbid_str,
                artist_mbid_str,
                caa_id_str,
            );
            true
        }
        Err(e) => {
            warn!(
                "⚠ MusicBrainz lookup failed for {} - {}: {}",
                play.artist, play.title, e
            );
            false
        }
    }
}
