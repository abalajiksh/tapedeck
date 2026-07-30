//! Rockbox `.scrobbler.log` parsing (Audioscrobbler Log format 1.1).
//!
//! Rockbox and most DAPs running it write a tab-separated log as you listen and
//! leave it on the device. It's the only way listens from a player with no
//! network ever reach a scrobbler.
//!
//! ```text
//! #AUDIOSCROBBLER/1.1
//! #TZ/UNKNOWN
//! #CLIENT/Rockbox sansae200 $Revision$
//! Artist<TAB>Album<TAB>Title<TAB>TrackNum<TAB>Length<TAB>Rating<TAB>Timestamp<TAB>MBID
//! ```
//!
//! Two details do most of the work here.
//!
//! **`#TZ`.** The header declares whether the timestamps are UTC. Players with a
//! real-time clock but no network sync — which is most DAPs — write `UNKNOWN`,
//! meaning the numbers are seconds-since-epoch computed from a *local* wall
//! clock. Reading those as UTC shifts a whole device's history by the offset,
//! which quietly corrupts the listening-clock heatmap and the Night Owl trait.
//! So `UNKNOWN` requires the caller to say which timezone the device was set to.
//!
//! **`Rating`.** `L` means listened, `S` means skipped. Skips are kept (behind a
//! flag) rather than discarded — how much of an album you actually sit through
//! is worth knowing — but they are not listens and must never be forwarded.

use chrono::TimeZone;
use chrono_tz::Tz;

/// One parsed row. `skipped` mirrors the `S` rating.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub artist: String,
    pub album: Option<String>,
    pub title: String,
    pub track_number: Option<i32>,
    /// Track length in seconds, when the player recorded one.
    pub duration: Option<u64>,
    pub skipped: bool,
    /// Unix seconds, already normalised to UTC.
    pub timestamp: i64,
    /// Column 8 — free MusicBrainz recording ID, when the player had one. These
    /// listens skip MusicBrainz lookup entirely.
    pub mbid: Option<String>,
}

/// What the `#TZ` header said, which decides whether we need a timezone hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTimezone {
    /// `#TZ/UTC` — timestamps are already UTC and need no adjustment.
    Utc,
    /// `#TZ/UNKNOWN`, or no header at all. Timestamps are local to whatever the
    /// device's clock was set to, and the caller must supply that zone.
    Unknown,
}

#[derive(Debug)]
pub struct ParsedLog {
    pub entries: Vec<LogEntry>,
    pub declared_tz: LogTimezone,
    pub client: Option<String>,
    /// Rows we couldn't make sense of. Surfaced rather than silently dropped —
    /// a log that's half-garbage should say so.
    pub malformed: usize,
}

/// Does this look like an Audioscrobbler log? Used to route an upload to this
/// parser rather than the JSON/CSV one.
pub fn looks_like_scrobbler_log(text: &str) -> bool {
    text.lines()
        .take(5)
        .any(|l| l.trim_start().to_uppercase().starts_with("#AUDIOSCROBBLER"))
}

/// Read the `#TZ` header without parsing the whole file, so the UI can ask for a
/// timezone *before* committing to an import.
pub fn peek_timezone(text: &str) -> LogTimezone {
    for line in text.lines().take(10) {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("#TZ/") {
            return if rest.trim().eq_ignore_ascii_case("UTC") {
                LogTimezone::Utc
            } else {
                LogTimezone::Unknown
            };
        }
    }
    LogTimezone::Unknown
}

/// Parse a log, converting timestamps to UTC.
///
/// `device_tz` is only consulted when the header says the timezone is unknown;
/// a log that declares UTC is taken at its word regardless of what was passed.
pub fn parse(text: &str, device_tz: Tz) -> ParsedLog {
    let declared_tz = peek_timezone(text);
    let mut entries = Vec::new();
    let mut client = None;
    let mut malformed = 0usize;

    for line in text.lines() {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.trim_start().strip_prefix('#') {
            if let Some(c) = rest.strip_prefix("CLIENT/") {
                client = Some(c.trim().to_string());
            }
            continue;
        }

        match parse_row(line, declared_tz, device_tz) {
            Some(entry) => entries.push(entry),
            None => malformed += 1,
        }
    }

    ParsedLog {
        entries,
        declared_tz,
        client,
        malformed,
    }
}

fn parse_row(line: &str, declared: LogTimezone, device_tz: Tz) -> Option<LogEntry> {
    let f: Vec<&str> = line.split('\t').collect();
    // artist, album, title, tracknum, length, rating, timestamp — MBID optional.
    if f.len() < 7 {
        return None;
    }

    let artist = f[0].trim();
    let title = f[2].trim();
    if artist.is_empty() || title.is_empty() {
        return None;
    }

    let raw_ts: i64 = f[6].trim().parse().ok()?;
    if raw_ts <= 0 {
        return None;
    }

    let skipped = match f[5].trim().to_uppercase().as_str() {
        "S" => true,
        "L" => false,
        // Some forks write an empty rating; treat it as a listen rather than
        // dropping the row, since the timestamp and track are still good.
        "" => false,
        _ => return None,
    };

    let timestamp = match declared {
        LogTimezone::Utc => raw_ts,
        LogTimezone::Unknown => local_epoch_to_utc(raw_ts, device_tz)?,
    };

    Some(LogEntry {
        artist: artist.to_string(),
        album: Some(f[1].trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        title: title.to_string(),
        track_number: f[3].trim().parse().ok().filter(|n| *n > 0),
        duration: f[4].trim().parse().ok().filter(|d| *d > 0),
        skipped,
        timestamp,
        mbid: f
            .get(7)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

/// Reinterpret a timestamp that was built from a local wall clock.
///
/// The device computed "seconds since epoch" as if its local time *were* UTC, so
/// we read the number back as a UTC calendar date, then re-anchor those same
/// wall-clock fields in the real zone. A DST-ambiguous hour picks the earlier
/// offset; a nonexistent one (spring-forward gap) is dropped rather than guessed.
fn local_epoch_to_utc(raw: i64, tz: Tz) -> Option<i64> {
    let naive = chrono::DateTime::from_timestamp(raw, 0)?.naive_utc();
    tz.from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_UNKNOWN: &str =
        "#AUDIOSCROBBLER/1.1\n#TZ/UNKNOWN\n#CLIENT/Rockbox sansae200 $Revision$\n";
    const HEADER_UTC: &str = "#AUDIOSCROBBLER/1.1\n#TZ/UTC\n#CLIENT/Rockbox ipod6g\n";

    #[test]
    fn detects_the_format() {
        assert!(looks_like_scrobbler_log(HEADER_UNKNOWN));
        assert!(!looks_like_scrobbler_log(r#"{"listens":[]}"#));
        assert!(!looks_like_scrobbler_log("title,artist,album\na,b,c"));
    }

    #[test]
    fn parses_a_listen_with_every_column() {
        let log = format!(
            "{HEADER_UTC}Boards of Canada\tMusic Has the Right\tRoygbiv\t4\t140\tL\t1700000000\tmbid-123\n"
        );
        let p = parse(&log, chrono_tz::UTC);
        assert_eq!(p.malformed, 0);
        assert_eq!(p.client.as_deref(), Some("Rockbox ipod6g"));
        let e = &p.entries[0];
        assert_eq!(e.artist, "Boards of Canada");
        assert_eq!(e.album.as_deref(), Some("Music Has the Right"));
        assert_eq!(e.title, "Roygbiv");
        assert_eq!(e.track_number, Some(4));
        assert_eq!(e.duration, Some(140));
        assert_eq!(e.mbid.as_deref(), Some("mbid-123"));
        assert!(!e.skipped);
        assert_eq!(e.timestamp, 1700000000);
    }

    #[test]
    fn marks_skips_without_dropping_them() {
        let log = format!(
            "{HEADER_UTC}A\tAl\tListened\t1\t100\tL\t1700000000\n\
                                       B\tAl\tSkipped\t2\t100\tS\t1700000100\n"
        );
        let p = parse(&log, chrono_tz::UTC);
        assert_eq!(p.entries.len(), 2);
        assert!(!p.entries[0].skipped);
        assert!(p.entries[1].skipped);
    }

    #[test]
    fn unknown_tz_is_read_as_device_local_time() {
        // 1700000000 UTC is 2023-11-14 22:13:20. A device set to Asia/Kolkata
        // (+5:30) that wrote this number meant 22:13 *local*, which is 16:43 UTC.
        let log = format!("{HEADER_UNKNOWN}A\tAl\tT\t1\t100\tL\t1700000000\n");
        let p = parse(&log, chrono_tz::Asia::Kolkata);
        assert_eq!(p.declared_tz, LogTimezone::Unknown);
        assert_eq!(p.entries[0].timestamp, 1700000000 - (5 * 3600 + 30 * 60));
    }

    #[test]
    fn declared_utc_ignores_the_device_hint() {
        // A log that says UTC is already correct; applying an offset would break it.
        let log = format!("{HEADER_UTC}A\tAl\tT\t1\t100\tL\t1700000000\n");
        let p = parse(&log, chrono_tz::Asia::Kolkata);
        assert_eq!(p.entries[0].timestamp, 1700000000);
    }

    #[test]
    fn counts_malformed_rows_instead_of_silently_dropping() {
        let log = format!(
            "{HEADER_UTC}A\tAl\tT\t1\t100\tL\t1700000000\n\
                                       too\tfew\tcolumns\n\
                                       B\tAl\tT2\t1\t100\tX\t1700000100\n\
                                       \tAl\tNoArtist\t1\t100\tL\t1700000200\n"
        );
        let p = parse(&log, chrono_tz::UTC);
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.malformed, 3); // short row, bad rating, missing artist
    }

    #[test]
    fn tolerates_missing_mbid_and_optional_fields() {
        let log = format!("{HEADER_UTC}A\t\tT\t\t\tL\t1700000000\n");
        let p = parse(&log, chrono_tz::UTC);
        let e = &p.entries[0];
        assert_eq!(e.album, None);
        assert_eq!(e.track_number, None);
        assert_eq!(e.duration, None);
        assert_eq!(e.mbid, None);
    }

    #[test]
    fn a_log_with_no_tz_header_is_treated_as_unknown() {
        // Safer default: assuming UTC on a device that wasn't would shift
        // everything, and the offset is unrecoverable after import.
        let log = "#AUDIOSCROBBLER/1.1\nA\tAl\tT\t1\t100\tL\t1700000000\n";
        assert_eq!(peek_timezone(log), LogTimezone::Unknown);
    }
}
