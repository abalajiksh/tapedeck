//! Spotify "Extended Streaming History" export.
//!
//! Requested from Spotify's privacy page and delivered as a zip holding
//! `Spotify Extended Streaming History/Streaming_History_Audio_<year>.json`
//! plus `Streaming_History_Video_<year>.json` — one JSON array per file, one
//! object per play:
//!
//! ```json
//! { "ts": "2026-04-06T01:32:10Z", "platform": "linux", "ms_played": 289921,
//!   "master_metadata_track_name": "Rettai Kathirae",
//!   "master_metadata_album_artist_name": "Harris Jayaraj",
//!   "master_metadata_album_album_name": "Maattrraan (…)",
//!   "spotify_track_uri": "spotify:track:51DhooaRxLNlzBIZnjZbOL",
//!   "reason_end": "trackdone", "skipped": false, "incognito_mode": false }
//! ```
//!
//! The `_Video_` files are **not** podcasts — they are music videos carrying an
//! ordinary `spotify_track_uri`, so they are read exactly like the audio ones.
//! Podcast episodes and audiobook chapters are identified by their own uri
//! fields instead, and are dropped here.
//!
//! Three things in this format are easy to get wrong, and all three are silent.
//!
//! **`ts` is when the track *stopped*.** Spotify's now-playing endpoint reports
//! a start time and this export reports an end time; nothing in the file says
//! so. Importing `ts` as-is shifts every listen forward by one track length,
//! which is invisible in a listen count and wrong in every session boundary,
//! hour-of-day heatmap and Reports clock — measured on a 32,575-row export,
//! 5.3% of listens land in a different hour. Verified rather than assumed: over
//! 22,959 consecutive gapless pairs, `ts` as an end time predicts the next
//! row's start to within 3s **90.7%** of the time, against 21.6% for `ts` as a
//! start time.
//!
//! **`ms_played` is not the track length, except when it is.** On a row whose
//! `reason_end` is `trackdone` the track ran to the end, so `ms_played` *is*
//! its duration. Taking the modal value per track across those rows is
//! essentially exact — for tracks played four or more times, the median track
//! has 100% of its `trackdone` plays on the same second — and it fills a
//! duration for 93.3% of rows, including the ones that were skipped. That is
//! the only duration this export will ever give up; see [`durations`].
//!
//! **Spotify's `skipped` flag does not mean what Tapedeck's does.** It records
//! that the track did not reach its end, which is a different question from
//! whether it counts as a listen: of 6,321 rows flagged skipped, 2,362 played
//! 30 seconds or more and 909 played over half the track. So the flag is
//! ignored and the disposition is derived from the same threshold the live
//! sources use — see [`SCROBBLE_FRACTION`].

use serde::Deserialize;
use std::collections::HashMap;

/// Half the track counts as a listen, as everywhere else in Tapedeck (and as
/// Last.fm and ListenBrainz define it). Kept in step with `sources::jellyfin`
/// deliberately: a Spotify listen and a Jellyfin listen should not need a
/// different amount of playing to earn the same row.
const SCROBBLE_FRACTION: f64 = 0.5;
/// …or four minutes, whichever comes first, so a long track doesn't need
/// half an hour before it counts.
const SCROBBLE_AFTER_MS: u64 = 240_000;
/// With no duration to compare against, fall back to Spotify's own definition
/// of a stream. Only ~6.7% of rows need this.
const MIN_PLAY_MS: u64 = 30_000;

/// One play, already corrected to a start time.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Unix seconds at which the track *started* — `ts` minus `ms_played`.
    pub timestamp: i64,
    /// Track length in seconds, when some `trackdone` row revealed it.
    pub duration: Option<u64>,
    /// The bare Spotify track id, without the `spotify:track:` prefix. Stored
    /// so a later pass can map these to MusicBrainz without re-reading the
    /// export; nothing in Tapedeck resolves it yet.
    pub track_id: String,
    /// `platform` — the OS the Spotify client was running on.
    pub platform: Option<String>,
    /// Derived from how much played, never from Spotify's own `skipped` field.
    pub skipped: bool,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub entries: Vec<Entry>,
    /// Podcast episodes and audiobook chapters, dropped. Reported rather than
    /// silently discarded — an export that is half podcasts should say so.
    pub non_music: usize,
    /// The same play listed twice in the export (identical start and track).
    pub duplicates: usize,
    /// Rows we could not read at all.
    pub malformed: usize,
    /// Files that parsed but held no rows we could use.
    pub files: usize,
}

/// Is this file one of the export's history files?
///
/// Matched on the leaf name so a zip that keeps the `Spotify Extended Streaming
/// History/` directory (which is how Spotify ships it) and one that doesn't
/// both work.
pub fn is_history_file(path: &str) -> bool {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    leaf.starts_with("Streaming_History_") && leaf.ends_with(".json")
}

/// Does this archive look like a Spotify export rather than a ListenBrainz one?
pub fn looks_like_export<S: AsRef<str>>(paths: &[S]) -> bool {
    paths.iter().any(|p| is_history_file(p.as_ref()))
}

/// One row of the export. Everything absent is optional: Spotify has added
/// fields between export vintages (audiobooks arrived in 2023), and a 2021
/// export must still import.
#[derive(Debug, Deserialize)]
struct Row {
    ts: String,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    ms_played: u64,
    #[serde(default)]
    master_metadata_track_name: Option<String>,
    #[serde(default)]
    master_metadata_album_artist_name: Option<String>,
    #[serde(default)]
    master_metadata_album_album_name: Option<String>,
    #[serde(default)]
    spotify_track_uri: Option<String>,
    #[serde(default)]
    spotify_episode_uri: Option<String>,
    #[serde(default)]
    audiobook_uri: Option<String>,
    #[serde(default)]
    reason_end: Option<String>,
}

/// `spotify:track:51DhooaRxLNlzBIZnjZbOL` → `51DhooaRxLNlzBIZnjZbOL`.
///
/// Anything that isn't a track uri (an episode's, a local file's) returns None,
/// which is also what keeps non-music out of the import.
fn track_id(uri: &str) -> Option<String> {
    uri.strip_prefix("spotify:track:").filter(|id| !id.is_empty()).map(str::to_string)
}

/// Track length in **milliseconds** per track id, learned from the rows that
/// ran to the end.
///
/// The modal value rather than the median or the maximum: seeking forward makes
/// a `trackdone` row shorter than the track and seeking back makes it longer,
/// so the distribution has a hard spike at the true length with thin tails
/// either side. Measured, the mode sits within 0.5s of the maximum for 90% of
/// tracks, and the median track has *every* one of its `trackdone` plays on the
/// same second.
///
/// Public so a caller can pin the behaviour; the parser calls it itself.
pub fn durations(rows: &[(String, u64, bool)]) -> HashMap<String, u64> {
    let mut seen: HashMap<String, HashMap<u64, usize>> = HashMap::new();
    for (id, ms, finished) in rows {
        if *finished && *ms > 0 {
            *seen.entry(id.clone()).or_default().entry(*ms).or_insert(0) += 1;
        }
    }
    seen.into_iter()
        .filter_map(|(id, counts)| {
            // Ties break toward the longer reading: a tie means two lengths were
            // seen equally often, and the shorter one is the one a seek could
            // have produced.
            counts.into_iter().max_by_key(|&(ms, n)| (n, ms)).map(|(ms, _)| (id, ms))
        })
        .collect()
}

/// Did this play earn a listen, or is it a skip?
fn counts_as_listen(ms_played: u64, duration_ms: Option<u64>) -> bool {
    match duration_ms {
        Some(d) if d > 0 => {
            ms_played as f64 >= d as f64 * SCROBBLE_FRACTION || ms_played >= SCROBBLE_AFTER_MS
        }
        _ => ms_played >= MIN_PLAY_MS,
    }
}

/// Parse every history file of an export together.
///
/// They are read as one batch rather than file by file because the duration
/// table is built across the whole export: a track played to the end once in
/// 2023 gives its length to every truncated play of it in 2025.
///
/// `incognito_mode` is deliberately not filtered. A private session is a
/// request not to affect Spotify's recommendations, not a request to be
/// forgotten — Spotify recorded those plays in this very file, and they are the
/// user's own listening.
pub fn parse<S: AsRef<str>>(files: &[S]) -> Parsed {
    let mut out = Parsed::default();
    let mut rows: Vec<Row> = Vec::new();

    for text in files {
        match serde_json::from_str::<Vec<Row>>(text.as_ref()) {
            Ok(mut r) => {
                out.files += 1;
                rows.append(&mut r);
            }
            Err(_) => out.malformed += 1,
        }
    }

    // Everything playable, with its start time worked out. Non-music is counted
    // out here so `non_music` means episodes and chapters rather than "rows we
    // didn't like".
    let mut playable: Vec<(Row, String)> = Vec::new();
    for row in rows {
        if row.spotify_episode_uri.is_some() || row.audiobook_uri.is_some() {
            out.non_music += 1;
            continue;
        }
        match row.spotify_track_uri.as_deref().and_then(track_id) {
            Some(id) => playable.push((row, id)),
            None => out.malformed += 1,
        }
    }

    let lengths = durations(
        &playable
            .iter()
            .map(|(r, id)| (id.clone(), r.ms_played, r.reason_end.as_deref() == Some("trackdone")))
            .collect::<Vec<_>>(),
    );

    let mut entries: Vec<Entry> = Vec::new();
    for (row, id) in playable {
        let Some(end) = parse_ts(&row.ts) else {
            out.malformed += 1;
            continue;
        };
        let (Some(title), Some(artist)) =
            (row.master_metadata_track_name, row.master_metadata_album_artist_name)
        else {
            out.malformed += 1;
            continue;
        };
        if title.trim().is_empty() || artist.trim().is_empty() {
            out.malformed += 1;
            continue;
        }

        let duration_ms = lengths.get(&id).copied();
        // Round rather than truncate: the export's seconds-resolution `ts` is
        // already up to a second away from the real boundary, and rounding
        // halves the error rather than always leaning one way.
        let played_secs = ((row.ms_played + 500) / 1000) as i64;

        entries.push(Entry {
            title,
            artist,
            album: row.master_metadata_album_album_name.filter(|a| !a.trim().is_empty()),
            timestamp: end - played_secs,
            duration: duration_ms.map(|ms| ((ms + 500) / 1000).max(1)),
            track_id: id,
            platform: row.platform.filter(|p| !p.trim().is_empty() && p != "unknown"),
            skipped: !counts_as_listen(row.ms_played, duration_ms),
        });
    }

    // Oldest first, so an interrupted import leaves a contiguous history rather
    // than a hole in the middle — the same reason the ListenBrainz archive
    // sorts.
    entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp).then_with(|| a.track_id.cmp(&b.track_id)));
    // The export lists some plays twice, identically (183 in a 32,575-row
    // sample). Those are artifacts of the export rather than repeats — a real
    // repeat is a track length later — so they are dropped here instead of
    // being left for the database's fuzzy window, which costs a query each.
    let before = entries.len();
    entries.dedup_by(|a, b| a.timestamp == b.timestamp && a.track_id == b.track_id);
    out.duplicates = before - entries.len();

    out.entries = entries;
    out
}

/// `2026-04-06T01:32:10Z` → unix seconds. Always UTC in this export; the `Z` is
/// literal in every row Spotify has ever written, but rfc3339 is parsed
/// properly rather than sliced so an offset would not be read as UTC.
fn parse_ts(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|d| d.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real row, byte for byte out of a 2026 export, with the fields this
    /// parser reads. Pins the spellings — every one of them is snake_case and
    /// long enough to typo, and a mismatch deserialises to `None` in silence.
    const REAL_ROW: &str = r#"[{
      "ts": "2026-04-06T01:32:10Z",
      "platform": "linux",
      "ms_played": 289921,
      "conn_country": "DE",
      "ip_addr": "2a02:8071:7191:5a0:acc4:ba84:c09e:a19b",
      "master_metadata_track_name": "Rettai Kathirae",
      "master_metadata_album_artist_name": "Harris Jayaraj",
      "master_metadata_album_album_name": "Maattrraan (Original Motion Picture Soundtrack)",
      "spotify_track_uri": "spotify:track:51DhooaRxLNlzBIZnjZbOL",
      "episode_name": null,
      "episode_show_name": null,
      "spotify_episode_uri": null,
      "audiobook_title": null,
      "audiobook_uri": null,
      "audiobook_chapter_uri": null,
      "audiobook_chapter_title": null,
      "reason_start": "remote",
      "reason_end": "trackdone",
      "shuffle": false,
      "skipped": false,
      "offline": false,
      "offline_timestamp": 1775438837,
      "incognito_mode": false
    }]"#;

    #[test]
    fn a_real_row_parses_with_every_field_we_read() {
        let p = parse(&[REAL_ROW]);
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.malformed, 0);
        let e = &p.entries[0];
        assert_eq!(e.title, "Rettai Kathirae");
        assert_eq!(e.artist, "Harris Jayaraj");
        assert_eq!(e.album.as_deref(), Some("Maattrraan (Original Motion Picture Soundtrack)"));
        assert_eq!(e.track_id, "51DhooaRxLNlzBIZnjZbOL");
        assert_eq!(e.platform.as_deref(), Some("linux"));
        assert!(!e.skipped);
    }

    /// The one that shifts a whole history by a track length if it regresses.
    #[test]
    fn the_timestamp_is_the_start_of_play_not_the_end() {
        let p = parse(&[REAL_ROW]);
        // 2026-04-06T01:32:10Z is when it *stopped*, 289.921s after it began.
        let end = parse_ts("2026-04-06T01:32:10Z").unwrap();
        assert_eq!(p.entries[0].timestamp, end - 290);
        // And the row itself supplies the length, since it ran to the end.
        assert_eq!(p.entries[0].duration, Some(290));
    }

    #[test]
    fn a_trackdone_row_teaches_every_truncated_play_of_that_track_its_length() {
        let json = r#"[
          {"ts":"2025-01-01T00:03:20Z","ms_played":200000,"reason_end":"trackdone",
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA"},
          {"ts":"2025-01-02T00:00:50Z","ms_played":50000,"reason_end":"fwdbtn",
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA"}
        ]"#;
        let p = parse(&[json]);
        assert_eq!(p.entries.len(), 2);
        for e in &p.entries {
            assert_eq!(e.duration, Some(200), "both plays know the track is 200s");
        }
        // 50s of a 200s track is a quarter — a skip, whatever Spotify's own
        // flag would have said.
        assert!(p.entries.iter().any(|e| e.skipped));
        assert!(p.entries.iter().any(|e| !e.skipped));
    }

    /// Spotify's `skipped` is "didn't reach the end", which is not the same
    /// question. This row played 80% of the track with the flag set.
    #[test]
    fn spotifys_own_skipped_flag_is_not_the_disposition() {
        let json = r#"[
          {"ts":"2025-01-01T00:03:20Z","ms_played":200000,"reason_end":"trackdone","skipped":false,
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA"},
          {"ts":"2025-01-02T00:02:40Z","ms_played":160000,"reason_end":"fwdbtn","skipped":true,
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA"}
        ]"#;
        let p = parse(&[json]);
        assert!(p.entries.iter().all(|e| !e.skipped), "80% of a track is a listen");
    }

    #[test]
    fn without_a_duration_the_threshold_falls_back_to_thirty_seconds() {
        let json = r#"[
          {"ts":"2025-01-01T00:00:29Z","ms_played":29000,"reason_end":"fwdbtn",
           "master_metadata_track_name":"A","master_metadata_album_artist_name":"X",
           "spotify_track_uri":"spotify:track:AAA"},
          {"ts":"2025-01-02T00:00:31Z","ms_played":31000,"reason_end":"fwdbtn",
           "master_metadata_track_name":"B","master_metadata_album_artist_name":"X",
           "spotify_track_uri":"spotify:track:BBB"}
        ]"#;
        let p = parse(&[json]);
        assert_eq!(p.entries.iter().filter(|e| e.skipped).count(), 1);
        assert_eq!(p.entries.iter().find(|e| !e.skipped).unwrap().title, "B");
    }

    #[test]
    fn a_long_track_counts_after_four_minutes_rather_than_half() {
        let json = r#"[
          {"ts":"2025-01-01T00:20:00Z","ms_played":1200000,"reason_end":"trackdone",
           "master_metadata_track_name":"Untrue","master_metadata_album_artist_name":"Burial",
           "spotify_track_uri":"spotify:track:AAA"},
          {"ts":"2025-01-02T00:04:10Z","ms_played":250000,"reason_end":"fwdbtn",
           "master_metadata_track_name":"Untrue","master_metadata_album_artist_name":"Burial",
           "spotify_track_uri":"spotify:track:AAA"}
        ]"#;
        let p = parse(&[json]);
        // 250s is a fifth of a 20-minute track, but it is over four minutes.
        assert!(p.entries.iter().all(|e| !e.skipped));
    }

    #[test]
    fn podcasts_and_audiobooks_are_dropped_and_counted() {
        let json = r#"[
          {"ts":"2025-01-01T00:30:00Z","ms_played":1800000,"reason_end":"trackdone",
           "episode_name":"Some show","spotify_episode_uri":"spotify:episode:EEE"},
          {"ts":"2025-01-02T00:30:00Z","ms_played":1800000,"reason_end":"trackdone",
           "audiobook_title":"A book","audiobook_uri":"spotify:show:BBB"},
          {"ts":"2025-01-03T00:03:20Z","ms_played":200000,"reason_end":"trackdone",
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA"}
        ]"#;
        let p = parse(&[json]);
        assert_eq!(p.non_music, 2);
        assert_eq!(p.malformed, 0, "an episode is not a malformed row");
        assert_eq!(p.entries.len(), 1);
    }

    /// The `_Video_` files are music videos, not podcasts — same shape, same
    /// `spotify_track_uri`. Reading them as anything else loses 687 listens out
    /// of a 32,575-row export.
    #[test]
    fn a_video_row_is_an_ordinary_music_listen() {
        let json = r#"[{
          "ts":"2023-10-03T20:33:30Z","platform":"android","ms_played":3447,
          "master_metadata_track_name":"Loyal Brave True - From \"Mulan\"",
          "master_metadata_album_artist_name":"Christina Aguilera",
          "master_metadata_album_album_name":"Loyal Brave True",
          "spotify_track_uri":"spotify:track:3gy8PSkBSyvPyWZazgQn0F",
          "spotify_episode_uri":null,"reason_end":"endplay","skipped":true
        }]"#;
        let p = parse(&[json]);
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].track_id, "3gy8PSkBSyvPyWZazgQn0F");
        assert!(p.entries[0].skipped, "3.4s is not a listen");
    }

    #[test]
    fn the_same_play_listed_twice_is_dropped_once() {
        let row = r#"{"ts":"2025-01-01T00:03:20Z","ms_played":200000,"reason_end":"trackdone",
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA"}"#;
        let p = parse(&[format!("[{row},{row}]")]);
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.duplicates, 1);
    }

    /// A track played twice back to back is *not* a duplicate — it has a
    /// different start time, one track length apart.
    #[test]
    fn a_genuine_repeat_survives() {
        let json = r#"[
          {"ts":"2025-01-01T00:03:20Z","ms_played":200000,"reason_end":"trackdone",
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA"},
          {"ts":"2025-01-01T00:06:40Z","ms_played":200000,"reason_end":"trackdone",
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA"}
        ]"#;
        let p = parse(&[json]);
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.duplicates, 0);
        assert_eq!(p.entries[1].timestamp - p.entries[0].timestamp, 200);
    }

    /// An incognito play is still the user's own listening, and Spotify put it
    /// in the file. Dropping it would lose 538 listens out of 32,575 for no
    /// benefit the user asked for.
    #[test]
    fn an_incognito_play_is_imported_like_any_other() {
        let json = r#"[{"ts":"2025-01-01T00:03:20Z","ms_played":200000,"reason_end":"trackdone",
           "master_metadata_track_name":"Xtal","master_metadata_album_artist_name":"Aphex Twin",
           "spotify_track_uri":"spotify:track:AAA","incognito_mode":true}]"#;
        assert_eq!(parse(&[json]).entries.len(), 1);
    }

    #[test]
    fn an_export_is_recognised_by_its_history_files() {
        assert!(looks_like_export(&[
            "Spotify Extended Streaming History/Streaming_History_Audio_2024.json",
            "Spotify Extended Streaming History/ReadMeFirst_ExtendedStreamingHistory.pdf",
        ]));
        assert!(looks_like_export(&["Streaming_History_Video_2024.json"]));
        // A ListenBrainz archive must not be routed here.
        assert!(!looks_like_export(&["listens/2024/1.jsonl", "feedback.jsonl", "user.json"]));
    }

    #[test]
    fn a_local_file_play_has_no_track_id_and_is_not_imported() {
        let json = r#"[{"ts":"2025-01-01T00:03:20Z","ms_played":200000,"reason_end":"trackdone",
           "master_metadata_track_name":"Demo","master_metadata_album_artist_name":"Me",
           "spotify_track_uri":"spotify:local:::Demo:200"}]"#;
        let p = parse(&[json]);
        assert!(p.entries.is_empty());
        assert_eq!(p.malformed, 1);
    }

    /// Run the parser over a real export and print what it made of it.
    ///
    /// Ignored by default and pointed at a file by environment variable,
    /// because a real export is 27 MB of somebody's listening and does not
    /// belong in the tree. The tests above pin the behaviour; this one is for
    /// looking at a new export vintage before trusting it:
    ///
    /// ```sh
    /// TAPEDECK_SPOTIFY_EXPORT=~/my_spotify_data.zip \
    ///   cargo test spotify::tests::a_real_export -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a real export; set TAPEDECK_SPOTIFY_EXPORT"]
    fn a_real_export_reads_end_to_end() {
        use std::io::Read;
        let Ok(path) = std::env::var("TAPEDECK_SPOTIFY_EXPORT") else {
            panic!("set TAPEDECK_SPOTIFY_EXPORT to a Spotify export zip");
        };
        let bytes = std::fs::read(&path).expect("could not read the export");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("not a zip");
        let mut files = Vec::new();
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            if e.is_file() && is_history_file(e.name()) {
                let mut s = String::new();
                e.read_to_string(&mut s).unwrap();
                files.push(s);
            }
        }
        assert!(!files.is_empty(), "no Streaming_History_*.json in that zip");
        let p = parse(&files);

        let listens = p.entries.iter().filter(|e| !e.skipped).count();
        let with_duration = p.entries.iter().filter(|e| e.duration.is_some()).count();
        println!(
            "{} files · {} plays ({} listens, {} skips) · {} with a duration ({:.1}%) \
             · {} non-music · {} duplicates · {} malformed",
            p.files,
            p.entries.len(),
            listens,
            p.entries.len() - listens,
            with_duration,
            100.0 * with_duration as f64 / p.entries.len() as f64,
            p.non_music,
            p.duplicates,
            p.malformed,
        );
        let first = p.entries.first().unwrap();
        let last = p.entries.last().unwrap();
        println!("  {} … {}", first.timestamp, last.timestamp);

        assert!(p.entries.iter().all(|e| e.timestamp > 1_000_000_000), "no pre-2001 timestamps");
        assert!(p.entries.windows(2).all(|w| w[0].timestamp <= w[1].timestamp), "sorted");
        assert!(
            p.malformed * 100 < p.entries.len(),
            "over 1% of rows unreadable — has the format changed?"
        );
    }

    #[test]
    fn the_duration_table_takes_the_mode_not_the_extremes() {
        // A seek forward cut one play short; a seek back stretched another.
        let rows: Vec<(String, u64, bool)> = vec![
            ("A".into(), 200_000, true),
            ("A".into(), 200_000, true),
            ("A".into(), 200_000, true),
            ("A".into(), 140_000, true),
            ("A".into(), 260_000, true),
            // Not finished — says nothing about the length.
            ("A".into(), 999_000, false),
        ];
        assert_eq!(durations(&rows).get("A"), Some(&200_000));
    }
}
