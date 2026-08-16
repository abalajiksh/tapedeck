use serde::{Deserialize, Serialize};

/// Top-level request body for `/1/submit-listens`
#[derive(Debug, Deserialize)]
pub struct SubmitListensRequest {
    pub listen_type: ListenType,
    pub payload: Vec<ListenPayload>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ListenType {
    Single,
    PlayingNow,
    Import,
}

#[derive(Debug, Deserialize)]
pub struct ListenPayload {
    /// Unix timestamp — required for `single` and `import`, absent for `playing_now`
    pub listened_at: Option<i64>,
    pub track_metadata: TrackMetadata,
}

#[derive(Debug, Deserialize)]
pub struct TrackMetadata {
    pub artist_name: String,
    pub track_name: String,
    pub release_name: Option<String>,
    #[serde(default)]
    pub additional_info: Option<AdditionalInfo>,
    #[serde(default)]
    pub mbid_mapping: Option<MbidMapping>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AdditionalInfo {
    #[serde(default, deserialize_with = "lenient")]
    pub submission_client: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub submission_client_version: Option<String>,
    /// Lenient because this is the field CLAUDE.md named as the obvious
    /// next casualty of a strict type in this bag — it escaped only because
    /// fooyin happens to send it as a number.
    #[serde(default, deserialize_with = "lenient_millis")]
    pub duration_ms: Option<i64>,
    /// ListenBrainz spells this `tracknumber`; Tapedeck's own submissions and
    /// some clients use `track_number`. A mismatch here is silent — the number
    /// simply goes missing, taking album-playthrough analytics and `[[track:3]]`
    /// with it — so accept both spellings.
    ///
    /// It also arrives as a **string** as often as a number: `additional_info`
    /// is free-form in ListenBrainz and a tag read straight off a file is text
    /// (fooyin sends `"tracknumber": "3"`, and vinyl-style `"A1"` exists too).
    /// A bare `Option<i32>` failed the whole body rather than this one field —
    /// serde has no per-field recovery, so axum's `Json` extractor rejected the
    /// request with a 422 before `submit_listens` ran and *every* listen in the
    /// batch was lost, with no `rejected` entry to say why. That is why this is
    /// hand-deserialised: an unusable track number must cost the track number,
    /// never the listen.
    #[serde(default, alias = "tracknumber", alias = "trackNumber")]
    #[serde(deserialize_with = "lenient_track_number")]
    pub track_number: Option<i32>,
    /// The client reporting that this track was skipped rather than played
    /// through. `scrobbles.skipped` has existed since Phase 2 but only Rockbox
    /// imports and the shelf's deck transport ever set it, so skip-rate
    /// analytics had no live source. A skipped listen is stored, excluded from
    /// every count, and never forwarded.
    #[serde(default)]
    pub skipped: Option<bool>,

    /// How much of the track was actually heard, in milliseconds, with paused
    /// time excluded.
    ///
    /// `skipped` is a bool, and a bool cannot tell "skipped three seconds in"
    /// from "skipped with thirty seconds left" — opposite listening behaviours
    /// filed identically. Until a pushing client sends this there is nothing in
    /// the system that could distinguish them: `skipped` had no live source at
    /// all beyond Rockbox imports and the shelf transport.
    ///
    /// Lenient for the same reason as `track_number`: this lives in
    /// `additional_info`, and a strict type in that bag fails the **whole
    /// submission** with a 422 before the handler runs. `duration_ms` was named
    /// as the next field likely to be bitten by that; this is a new one in the
    /// same position, so it does not get to repeat the mistake.
    #[serde(default, deserialize_with = "lenient_millis")]
    pub listened_ms: Option<i64>,

    /// MBIDs as a *submitting* client sends them. `mbid_mapping` is the shape
    /// ListenBrainz **returns** — it's what the export carries, which is why
    /// `import.rs` reads it — but on the way in the spec puts these in
    /// `additional_info`, and that is where every real client (fooyin, Pano,
    /// multi-scrobbler) writes them. Reading only `mbid_mapping` meant a client
    /// that had already done the identification paid for a rate-limited
    /// MusicBrainz lookup anyway, and nothing errored to say so. `openapi.yaml`
    /// has documented these as read since v0.39.0; now they are.
    #[serde(default, deserialize_with = "lenient")]
    pub recording_mbid: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub release_mbid: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub release_group_mbid: Option<String>,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub artist_mbids: Option<Vec<String>>,

    /// Every artist credited, where the client sends them separately.
    ///
    /// ListenBrainz's own field, and the only structured way a submission can
    /// say "this is Bach *and* Hilary Hahn". Without it a classical listen is
    /// one string and the performer is unrecoverable — which is what it was:
    /// ingest hardcoded `artists: None` and threw the credit away even when it
    /// was there to read.
    ///
    /// Typed loosely on purpose. This lives in `additional_info`, which has no
    /// schema and where one strict field costs the **whole submission** — the
    /// trap `lenient_track_number` exists for. Anything that is not an array of
    /// strings is ignored rather than fatal.
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub artist_names: Option<Vec<String>>,

    // ── Tapedeck extensions ──
    #[serde(default, deserialize_with = "lenient")]
    pub tapedeck_audio: Option<TapedeckAudio>,
    #[serde(default, deserialize_with = "lenient")]
    pub tapedeck_device: Option<TapedeckDevice>,
    #[serde(default, deserialize_with = "lenient")]
    pub tapedeck_chain: Option<TapedeckChain>,
    #[serde(default, deserialize_with = "lenient")]
    pub tapedeck_session: Option<TapedeckSession>,
    #[serde(default, deserialize_with = "lenient")]
    pub tapedeck_playback: Option<TapedeckPlayback>,
}

/// Accepts whatever a client put in a milliseconds field and yields a
/// non-negative number or nothing — never an error. Same contract and same
/// reasoning as [`lenient_track_number`].
fn lenient_millis<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Field {
        Int(i64),
        /// Some tag readers hand milliseconds over as text.
        Str(String),
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Field>::deserialize(deserializer)? {
        Some(Field::Int(n)) => Some(n),
        Some(Field::Str(s)) => s.trim().parse::<i64>().ok(),
        Some(Field::Other(_)) | None => None,
    }
    .filter(|n| *n >= 0))
}

/// Accepts an array of strings and yields nothing for anything else.
///
/// Same rule as `lenient_track_number` and for the same reason: this field
/// lives in `additional_info`, which ListenBrainz imposes no schema on, and a
/// strict type there fails the entire submission with a 422 before the handler
/// ever runs. A bad field costs the field, never the listen.
fn lenient_string_list<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Field {
        List(Vec<String>),
        /// A bare string, a number, an object — anything at all.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Field>::deserialize(deserializer)? {
        Some(Field::List(v)) => Some(v),
        Some(Field::Other(_)) | None => None,
    })
}

/// Accepts whatever a client put in `tracknumber` and yields a number or
/// nothing — never an error. See the field's own note for why an error here is
/// unaffordable.
///
/// `1/12` is a real tag spelling (the disc's total rides along in the same
/// string), so the part before the slash is taken. `A1` deliberately does not
/// parse: a vinyl side letter is a *position*, not an index into the release,
/// and guessing `1` from it would put every side's opener at track one.
fn lenient_track_number<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Field {
        Int(i64),
        Str(String),
        /// Anything else at all — a float, a bool, an object. Swallowed rather
        /// than failed, which is the whole point of this function.
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Field>::deserialize(deserializer)? {
        Some(Field::Int(n)) => i32::try_from(n).ok(),
        Some(Field::Str(s)) => {
            let s = s.trim();
            let head = s.split('/').next().unwrap_or(s).trim();
            head.parse::<i32>().ok()
        }
        Some(Field::Other(_)) | None => None,
    }
    .filter(|n| *n > 0))
}

#[derive(Debug, Deserialize)]
pub struct MbidMapping {
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub artist_mbids: Option<Vec<String>>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

// ── Tapedeck-specific extensions (roadmap 4.2) ──

#[derive(Debug, Deserialize)]
pub struct TapedeckAudio {
    pub format_type: Option<String>, // "pcm", "dsd", "mqa"
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i16>,
    pub channels: Option<i16>,
    pub container: Option<String>,
    pub is_lossless: Option<bool>,
    pub source_quality: Option<String>,
    // DSD fields
    pub dsd_rate: Option<i64>,
    pub dsd_multiplier: Option<i16>,
    // Delivery (BT / transcode)
    pub delivery_codec: Option<String>,
    pub delivery_bitrate: Option<i32>,
    pub delivery_sample_rate: Option<i32>,
    pub delivery_bit_depth: Option<i32>,
    pub dsd_to_pcm_converted: Option<bool>,
    pub is_transcoded: Option<bool>,
    pub transcode_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TapedeckDevice {
    pub player_name: Option<String>,
    pub player_version: Option<String>,
    pub platform: Option<String>,
    pub machine_id: Option<String>,
    pub output_device: Option<String>,
    pub output_type: Option<String>,
    pub interface: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TapedeckChain {
    pub chain_id: Option<String>,
    pub components: Option<Vec<String>>,
}

/// What the client was doing when the track came up.
///
/// Deliberately two fields where there were five. The struct existed since the
/// extensions were first sketched and **nothing anywhere read it** — it was
/// deserialised on every submission and dropped, so a client filling it in got
/// silence with no error to say so. Half-present is the worst of the available
/// states: it costs a client author real work and returns nothing.
///
/// The two kept are the two that answer a question nothing else in Tapedeck
/// can: *what did I put on deliberately, and what did shuffle hand me?* The
/// three dropped could not. `volume_level` at the moment of submission says
/// nothing about a listen — it is a fact about a slider, and one sample of it
/// at that. `skip_count` is per-session state the server derives from the
/// listens themselves, and better, since it does not reset when a client
/// restarts. `is_repeat` is a mode rather than an event: a repeat that actually
/// happened is already visible as two listens of the same track.
///
/// A dropped field costs nothing to reinstate if a use for it appears. Storing
/// one nothing reads is how this struct got here.
#[derive(Debug, Deserialize)]
pub struct TapedeckSession {
    /// Whether shuffle chose this track rather than the listener.
    pub is_shuffle: Option<bool>,
    /// What it was played from — a playlist name, an album, a queue, radio.
    /// Free text: it is the client's own vocabulary for its own containers, and
    /// there is no shared one to normalise it against.
    #[serde(default, deserialize_with = "lenient")]
    pub queue_source: Option<String>,
}

/// Where the playhead is, from a client that can actually see it.
///
/// `playing_now` carries no position — the ListenBrainz submission format has
/// no field for one — so a pushed now-playing was wall-clock arithmetic from
/// the moment the client spoke, and a paused player drew a confident and
/// completely wrong position. [`Progress`](super::now_playing::Progress) has
/// modelled the difference between a measurement and a guess for a while; this
/// is the missing half, the way for a client to supply the measurement.
///
/// Both fields are lenient, because this rides in `additional_info` where a
/// strict type costs the entire submission.
#[derive(Debug, Deserialize)]
pub struct TapedeckPlayback {
    /// Exact playhead at the instant of submission, in milliseconds.
    #[serde(default, deserialize_with = "lenient_millis")]
    pub position_ms: Option<i64>,
    /// `playing` or `paused`. Anything else is read as playing, which is the
    /// safe default: a wrongly-paused deck freezes and looks broken, where a
    /// wrongly-playing one is merely the behaviour of every other source.
    ///
    /// Lenient like everything else in this bag. A bare `Option<String>` here
    /// is a *strict* type — a client sending a number or an object for `state`
    /// would fail the whole submission with a 422 before the handler ran, and
    /// take every listen in the batch with it. Caught by
    /// `only_an_explicit_pause_pauses_the_deck`, which is the exact shape of
    /// the fooyin bug arriving in a new field.
    #[serde(default, deserialize_with = "lenient")]
    pub state: Option<String>,
}

/// Accepts a value of the expected type and yields nothing for anything else.
/// The [`lenient_track_number`] contract, generic.
///
/// This is the general form of a rule this file states three times: nothing in
/// `additional_info` may be strict, because ListenBrainz imposes no schema on
/// that bag and serde has no per-field recovery — one wrongly-typed key and
/// axum's `Json` extractor rejects the body with a 422 before the handler runs,
/// losing every listen in the batch with no `rejected` entry to say why. The
/// client then retries the same doomed request forever while its connection
/// test keeps passing, because `validate-token` parses no body.
///
/// On a nested extension object this costs the whole object rather than one
/// field, which is still the right trade: an unreadable `tapedeck_audio` should
/// lose the audio detail, not the listen it describes.
fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Field<T> {
        Value(T),
        Other(serde::de::IgnoredAny),
    }

    Ok(match Option::<Field<T>>::deserialize(deserializer)? {
        Some(Field::Value(v)) => Some(v),
        Some(Field::Other(_)) | None => None,
    })
}

impl TapedeckPlayback {
    /// Whether the listener has paused. Only an explicit `paused` counts.
    pub fn is_paused(&self) -> bool {
        self.state.as_deref().map(str::trim).is_some_and(|s| s.eq_ignore_ascii_case("paused"))
    }

    /// The playhead in whole seconds, when one was sent.
    pub fn position_secs(&self) -> Option<u64> {
        self.position_ms.map(|ms| (ms / 1000) as u64)
    }
}

// ── Response types ──

/// The response to `submit-listens`.
///
/// Everything past `status` is a Tapedeck extension and purely additive:
/// ListenBrainz clients branch on `status` alone, so extra keys cannot break
/// them. It exists because a batch submit was previously all-or-nothing from
/// the client's point of view — "ok" told a plugin author nothing about which
/// listens were stored, which were already known, and which were malformed.
#[derive(Debug, Serialize)]
pub struct SubmitListensResponse {
    pub status: String,
    /// Newly stored.
    pub accepted: u32,
    /// Recognised as already held, by `source_id` or by the fuzzy window.
    /// Not an error — a client re-sending after a dropped connection is the
    /// normal way this happens.
    pub duplicate: u32,
    /// Listens that could not be stored and should **not** be retried, each
    /// with its index into the submitted payload. Always present, empty when
    /// nothing was rejected — an absent key would be ambiguous.
    pub rejected: Vec<RejectedListen>,
}

#[derive(Debug, Serialize)]
pub struct RejectedListen {
    /// Index into the submitted `payload`, so the client can map it back to
    /// what it sent.
    pub index: usize,
    pub reason: String,
}

/// The response to `submit-listens?dry_run=1`: what *would* have happened.
#[derive(Debug, Serialize)]
pub struct DryRunResponse {
    pub status: String,
    pub dry_run: bool,
    pub listens: Vec<DryRunListen>,
    pub rejected: Vec<RejectedListen>,
}

/// One listen's resolved state, computed and thrown away.
#[derive(Debug, Serialize)]
pub struct DryRunListen {
    pub index: usize,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub timestamp: i64,
    /// The score `AudioQuality::compute_score` derives from the submitted
    /// fields — the main thing a plugin author is trying to get right.
    pub quality_score: Option<f64>,
    /// The winner of the four-rung ladder, and which rung won it.
    pub chain_id: Option<i64>,
    pub chain_name: Option<String>,
    pub chain_source: &'static str,
    pub device_id: Option<i64>,
    pub listening_context: Option<String>,
    /// What the row would be filed as: `pending` (will be forwarded onward) or
    /// `imported` (stored only). The distinction a backfill has to get right.
    pub status_if_stored: &'static str,
    pub skipped: bool,
    /// Whether the dedup would have swallowed it — checked read-only against
    /// the same two rules the real path uses.
    pub duplicate: bool,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: u16,
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body fooyin's ListenBrainz service actually sends, reduced to the
    /// fields that decide whether it deserialises. Its `tracknumber` is a
    /// `QString` — so this arrived as a *string*, the whole body failed, axum
    /// answered 422 before any handler ran, and every listen in the batch was
    /// dropped with nothing in `rejected` to explain it. The connection test
    /// passed throughout, because `validate-token` parses no body.
    #[test]
    fn a_client_sending_a_string_track_number_still_submits_its_listens() {
        let body = r#"{
          "listen_type": "import",
          "token": "tapedeck-token",
          "payload": [{
            "listened_at": 1754000000,
            "track_metadata": {
              "artist_name": "Aphex Twin",
              "track_name": "Xtal",
              "release_name": "Selected Ambient Works 85-92",
              "additional_info": {
                "duration_ms": 294000,
                "tracknumber": "1",
                "recording_mbid": "2f3f0b3f-9d1a-4f9a-8d3e-1c0b5b6a7c8d",
                "release_mbid": "3a4b5c6d-7e8f-4a0b-9c1d-2e3f4a5b6c7d",
                "media_player": "fooyin",
                "submission_client": "fooyin"
              }
            }
          }]
        }"#;

        let req: SubmitListensRequest = serde_json::from_str(body).expect("fooyin body must parse");
        assert_eq!(req.listen_type, ListenType::Import);

        let info = req.payload[0].track_metadata.additional_info.as_ref().unwrap();
        assert_eq!(info.track_number, Some(1));
        // The other half of the same submission: a client that has already
        // identified the recording must not be charged a MusicBrainz lookup for
        // it. These live in `additional_info`, not in `mbid_mapping`.
        assert_eq!(info.recording_mbid.as_deref(), Some("2f3f0b3f-9d1a-4f9a-8d3e-1c0b5b6a7c8d"));
        assert_eq!(info.release_mbid.as_deref(), Some("3a4b5c6d-7e8f-4a0b-9c1d-2e3f4a5b6c7d"));
    }

    /// Every other spelling a tag can carry. The rule is that an unusable track
    /// number costs the track number and never the listen, so none of these may
    /// return an error.
    #[test]
    fn an_unusable_track_number_costs_the_track_number_and_not_the_listen() {
        fn parse(raw: &str) -> Option<i32> {
            let body = format!(
                r#"{{"artist_name":"A","track_name":"T","additional_info":{{"tracknumber":{raw}}}}}"#
            );
            serde_json::from_str::<TrackMetadata>(&body)
                .expect("a bad track number must never fail the listen")
                .additional_info
                .unwrap()
                .track_number
        }

        assert_eq!(parse("3"), Some(3));
        assert_eq!(parse(r#""3""#), Some(3));
        assert_eq!(parse(r#""  3 ""#), Some(3));
        // `1/12` carries the disc's total in the same string.
        assert_eq!(parse(r#""1/12""#), Some(1));
        // A vinyl side letter is a position, not an index into the release —
        // reading `1` out of it would put every side's opener at track one.
        assert_eq!(parse(r#""A1""#), None);
        assert_eq!(parse(r#""""#), None);
        assert_eq!(parse("null"), None);
        assert_eq!(parse("0"), None);
        assert_eq!(parse("-2"), None);
        assert_eq!(parse("1.5"), None);
        assert_eq!(parse("true"), None);
        assert_eq!(parse(r#"{"n":1}"#), None);
        // Out of range for i32 rather than malformed — still not worth the batch.
        assert_eq!(parse("99999999999999"), None);
    }

    /// The playhead, the pause, how much was heard, and what handed the track
    /// over — everything a pushing client can say that a poll cannot.
    #[test]
    fn a_pushing_client_can_report_the_playhead_and_what_it_was_playing_from() {
        let body = r#"{
          "artist_name": "Aphex Twin", "track_name": "Xtal",
          "additional_info": {
            "listened_ms": 3200,
            "tapedeck_playback": { "position_ms": 84210, "state": "paused" },
            "tapedeck_session": { "is_shuffle": true, "queue_source": "Late night" }
          }
        }"#;
        let info = serde_json::from_str::<TrackMetadata>(body).unwrap().additional_info.unwrap();

        assert_eq!(info.listened_ms, Some(3200));

        let pb = info.tapedeck_playback.unwrap();
        // Seconds, because that is what `Progress` and the deck deal in.
        assert_eq!(pb.position_secs(), Some(84));
        assert!(pb.is_paused());

        let s = info.tapedeck_session.unwrap();
        assert_eq!(s.is_shuffle, Some(true));
        assert_eq!(s.queue_source.as_deref(), Some("Late night"));
    }

    /// Only an explicit `paused` pauses the deck. A wrongly-paused deck freezes
    /// and reads as broken; a wrongly-playing one behaves like every other
    /// source, so an unrecognised state falls the safe way.
    #[test]
    fn only_an_explicit_pause_pauses_the_deck() {
        fn paused(state: &str) -> bool {
            let body = format!(
                r#"{{"artist_name":"A","track_name":"T","additional_info":{{"tapedeck_playback":{{"state":{state}}}}}}}"#
            );
            serde_json::from_str::<TrackMetadata>(&body)
                .expect("a playback state must never fail the listen")
                .additional_info
                .unwrap()
                .tapedeck_playback
                .unwrap()
                .is_paused()
        }

        assert!(paused(r#""paused""#));
        assert!(paused(r#""Paused""#));
        assert!(paused(r#"" paused ""#));
        assert!(!paused(r#""playing""#));
        assert!(!paused(r#""stopped""#));
        assert!(!paused(r#""""#));
        assert!(!paused("null"));
        assert!(!paused("42"));
    }

    /// Both new millisecond fields ride in `additional_info`, which has no
    /// schema and where a strict type costs the **entire submission** — serde
    /// has no per-field recovery, so axum's `Json` extractor rejects the body
    /// with a 422 before any handler runs and every listen in the batch is lost
    /// with nothing in `rejected` to explain it. That is the fooyin bug, and a
    /// new field in the same bag does not get to repeat it.
    #[test]
    fn an_unusable_millisecond_field_costs_the_field_and_not_the_listen() {
        fn parse(raw: &str) -> Option<i64> {
            let body = format!(
                r#"{{"artist_name":"A","track_name":"T","additional_info":{{"listened_ms":{raw}}}}}"#
            );
            serde_json::from_str::<TrackMetadata>(&body)
                .expect("a bad listened_ms must never fail the listen")
                .additional_info
                .unwrap()
                .listened_ms
        }

        assert_eq!(parse("3200"), Some(3200));
        assert_eq!(parse(r#""3200""#), Some(3200));
        assert_eq!(parse(r#""  3200 ""#), Some(3200));
        assert_eq!(parse("0"), Some(0));
        // Not a length. Silently dropped rather than stored as nonsense.
        assert_eq!(parse("-1"), None);
        assert_eq!(parse(r#""about a minute""#), None);
        assert_eq!(parse("1.5"), None);
        assert_eq!(parse("true"), None);
        assert_eq!(parse(r#"{"ms":10}"#), None);
        assert_eq!(parse("null"), None);
    }

    /// The general form of the rule, across the whole bag rather than one
    /// field. `additional_info` has no schema, so **every** typed key in it is
    /// a 422 waiting for a client that spells it differently — and a 422 here
    /// costs the entire batch, silently, forever, while the connection test
    /// keeps reporting success.
    ///
    /// `duration_ms` is the one CLAUDE.md named as the obvious next casualty;
    /// it escaped only because fooyin happens to send a number. The nested
    /// extension objects are the same bug with more surface: a client sending
    /// `"tapedeck_audio": "flac"` should lose its audio detail, not its music.
    #[test]
    fn a_wrongly_typed_extension_costs_the_extension_and_not_the_listen() {
        let body = r#"{
          "artist_name": "Aphex Twin", "track_name": "Xtal",
          "additional_info": {
            "duration_ms": "294000",
            "submission_client": 12345,
            "recording_mbid": { "id": "nope" },
            "tapedeck_audio": "flac",
            "tapedeck_device": [],
            "tapedeck_chain": "Desk",
            "tapedeck_session": 7,
            "tapedeck_playback": "playing",
            "listened_ms": "12000"
          }
        }"#;
        let info = serde_json::from_str::<TrackMetadata>(body)
            .expect("a bagful of wrong types must never fail the listen")
            .additional_info
            .unwrap();

        // The readable ones survive…
        assert_eq!(info.duration_ms, Some(294_000));
        assert_eq!(info.listened_ms, Some(12_000));
        // …and every unreadable one costs itself alone.
        assert_eq!(info.submission_client, None);
        assert_eq!(info.recording_mbid, None);
        assert!(info.tapedeck_audio.is_none());
        assert!(info.tapedeck_device.is_none());
        assert!(info.tapedeck_chain.is_none());
        assert!(info.tapedeck_session.is_none());
        assert!(info.tapedeck_playback.is_none());
    }

    /// `mbid_mapping` is the read-side shape and still has to work: an LB export
    /// replayed through this endpoint carries one.
    #[test]
    fn the_read_side_mbid_mapping_is_still_accepted() {
        let body = r#"{
          "artist_name": "A", "track_name": "T",
          "mbid_mapping": { "recording_mbid": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "caa_id": 42 }
        }"#;
        let meta: TrackMetadata = serde_json::from_str(body).unwrap();
        let m = meta.mbid_mapping.unwrap();
        assert_eq!(m.recording_mbid.as_deref(), Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"));
        assert_eq!(m.caa_id, Some(42));
    }
}
