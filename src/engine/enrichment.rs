use crate::models::Play;
use crate::musicbrainz::MusicBrainzClient;
use tracing::{debug, info};

/// Enrich a `Play` in-place with MusicBrainz metadata + Cover Art Archive info.
/// Returns `true` if enrichment succeeded, `false` if it failed (play is still usable).
pub async fn enrich_play(
    mb_client: &MusicBrainzClient,
    play: &mut Play,
    album_hint: Option<&str>,
) -> bool {
    debug!("Fetching MusicBrainz metadata for: {} - {}", play.artist, play.title);

    match mb_client.fetch_metadata(&play.title, &play.artist, album_hint).await {
        Ok(metadata) => {
            let track_mbid_str = metadata.track_mbid.as_deref().unwrap_or("none").to_string();
            let album_mbid_str = metadata.album_mbid.as_deref().unwrap_or("none").to_string();
            let artist_mbid_str = metadata.artist_mbid.as_deref().unwrap_or("none").to_string();
            let caa_id_str =
                metadata.caa_id.map(|id| id.to_string()).unwrap_or_else(|| "none".to_string());

            // **Blanks only.** A source's own ids came off the file's tags and
            // outrank a name search's best guess; overwriting them threw away
            // exactly the metadata Tapedeck runs on. The search keeps only the
            // first credited artist, so it never replaces a fuller list either —
            // the full credit comes from the recording lookup
            // (`fetch_recording_credits`) once the recording MBID is known.
            let same_release =
                play.mbid_release.is_none() || play.mbid_release == metadata.album_mbid;
            if play.mbid_recording.is_none() {
                play.mbid_recording = metadata.track_mbid;
            }
            if play.mbid_release.is_none() {
                play.mbid_release = metadata.album_mbid;
            }
            if play.mbid_artist.as_ref().is_none_or(|a| a.is_empty()) {
                play.mbid_artist = metadata.artist_mbid.as_ref().map(|id| vec![id.clone()]);
            }
            // The cover belongs to the release the search found; only where
            // that is the listen's release too.
            if same_release && play.caa_id.is_none() {
                play.caa_id = metadata.caa_id;
                play.caa_release_mbid = metadata.caa_release_mbid;
            }

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
            // **Debug, not warn, and that is a fix rather than a demotion.**
            // On 2026-08-27 this line ran 912 times in 2h18m for 39 distinct
            // tracks — one query 155 times — because a track MusicBrainz has
            // never heard of is re-asked on every flush, and a *transport*
            // failure cannot be tombstoned at all so it is re-asked forever.
            // At warn that is the same outage the ListenBrainz spam was: 6,096
            // copies of one sentence, and nothing else in the file findable.
            //
            // The signal did not go away, it moved somewhere it is said once:
            // the breaker reports an unreachable service in Settings →
            // Operations, and Maintenance → MusicBrainz Recordings reports how
            // many tracks they simply do not have.
            debug!(
                "MusicBrainz lookup failed for {} - {}: {}",
                play.artist,
                play.title,
                // `{:#}` is anyhow's own cause chain — the same thing
                // `logging::error_chain` does for a plain `std` error, which is
                // what the reqwest error inside `api_get` is.
                format_args!("{e:#}")
            );
            false
        }
    }
}
