use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// The request side of `/1/submit-listens`. Every type here is published in the
// OpenAPI document, so `///` on them is written for a client author and notes
// for whoever edits this file are `//` comments.

/// A ListenBrainz-shaped submission.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubmitListensRequest {
    pub listen_type: ListenType,
    /// At least one listen and at most 1000; exactly one for `playing_now`.
    #[schema(min_items = 1, max_items = 1000)]
    pub payload: Vec<ListenPayload>,
}

/// **This decides forwarding.** `single` and `playing_now` are live;
/// `import` is stored only unless the token is marked `imports_are_live` —
/// see `POST /1/submit-listens`.
#[derive(Debug, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListenType {
    Single,
    PlayingNow,
    Import,
}

/// One listen in a ListenBrainz-shaped submission.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = SubmitListen)]
pub struct ListenPayload {
    /// Unix seconds, UTC, **start** of play. Required for `single` and
    /// `import`, and ignored for `playing_now`, which is always now.
    pub listened_at: Option<i64>,
    pub track_metadata: TrackMetadata,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TrackMetadata {
    /// The artist credit as one string. A **semicolon-separated** value
    /// (`Bach; Hilary Hahn`) is read as several credited artists when
    /// `additional_info.artist_names` is absent — see there.
    pub artist_name: String,
    pub track_name: String,
    pub release_name: Option<String>,
    #[serde(default)]
    pub additional_info: Option<AdditionalInfo>,
    /// The shape ListenBrainz **returns** its MBIDs in. Read as a fallback;
    /// a submitting client should put them in `additional_info`.
    #[serde(default)]
    pub mbid_mapping: Option<MbidMapping>,
}

/// ListenBrainz's free-form bag, and where Tapedeck's own extensions ride.
///
/// **No field in here can fail a submission.** A value of the wrong type
/// costs that field — or, for a `tapedeck_*` object, that object — and the
/// listen is still stored. Unknown keys are ignored.
// Every field is lenient for the reason `lenient` states: ListenBrainz imposes
// no schema on this bag and serde has no per-field recovery, so one strict
// field would fail the whole body with a 422 before the handler ran.
#[derive(Debug, Deserialize, Default, ToSchema)]
pub struct AdditionalInfo {
    #[serde(default, deserialize_with = "lenient")]
    pub submission_client: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub submission_client_version: Option<String>,
    /// The track's length in milliseconds. A number or a numeric string.
    // Lenient because this is the field CLAUDE.md named as the obvious
    // next casualty of a strict type in this bag — it escaped only because
    // fooyin happens to send it as a number.
    #[serde(default, deserialize_with = "lenient_millis")]
    #[schema(schema_with = integer_or_string)]
    pub duration_ms: Option<i64>,
    /// The recording's ISRC, when the client knows it — a tag read off the
    /// file usually does. It survives crossing between a library, a streaming
    /// catalogue and MusicBrainz, so a listen that arrives with one never needs
    /// a lookup to be identified.
    #[serde(default, deserialize_with = "lenient")]
    pub isrc: Option<String>,
    /// Also accepted as `tracknumber` (ListenBrainz's spelling) and
    /// `trackNumber`. A number, or the string a tag reader hands over — `"7"`
    /// and `"7/12"` both work. Anything that is neither (a vinyl `"A1"`, a
    /// bool, an object) costs the track number only.
    // It arrives as a **string** as often as a number: fooyin sends
    // `"tracknumber": "3"`, and vinyl-style `"A1"` exists too. A bare
    // `Option<i32>` failed the whole body rather than this one field — axum's
    // `Json` extractor rejected the request with a 422 before `submit_listens`
    // ran and *every* listen in the batch was lost, with no `rejected` entry to
    // say why. That is why this is hand-deserialised: an unusable track number
    // must cost the track number, never the listen.
    #[serde(default, alias = "tracknumber", alias = "trackNumber")]
    #[serde(deserialize_with = "lenient_track_number")]
    #[schema(schema_with = integer_or_string)]
    pub track_number: Option<i32>,
    /// The track was skipped rather than played through. Stored, excluded
    /// from every count, and **never forwarded** — a forwarded skip is a wrong
    /// scrobble on a permanent record. Top level of `additional_info`; there
    /// is no `tapedeck_skipped`.
    // `scrobbles.skipped` has existed since Phase 2 but only Rockbox imports
    // and the shelf's deck transport ever set it, so skip-rate analytics had
    // no live source until a client could say so here.
    #[serde(default)]
    pub skipped: Option<bool>,
    /// How much of the track was actually heard, in milliseconds, with
    /// paused time excluded. A number or a numeric string. This is what makes
    /// `skipped` worth having: a bool cannot separate "skipped three seconds
    /// in" from "skipped with thirty seconds left", and those are opposite
    /// behaviours.
    // Lenient for the same reason as `track_number`: a strict type in this bag
    // fails the **whole submission** with a 422 before the handler runs.
    #[serde(default, deserialize_with = "lenient_millis")]
    #[schema(schema_with = integer_or_string)]
    pub listened_ms: Option<i64>,

    /// MBIDs as a submitting client sends them. A listen that arrives
    /// identified is never looked up again.
    // `mbid_mapping` is the shape ListenBrainz **returns** — it's what the
    // export carries, which is why `import.rs` reads it — but on the way in the
    // spec puts these in `additional_info`, and that is where every real client
    // (fooyin, Pano, multi-scrobbler) writes them. Reading only `mbid_mapping`
    // meant a client that had already done the identification paid for a
    // rate-limited MusicBrainz lookup anyway.
    #[serde(default, deserialize_with = "lenient")]
    pub recording_mbid: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub release_mbid: Option<String>,
    /// The release group, which is what relates a reissue, a remaster and a
    /// regional edition to one another.
    #[serde(default, deserialize_with = "lenient")]
    pub release_group_mbid: Option<String>,
    /// The track *on a particular release*, as distinct from the recording.
    /// Only a tagging client ever knows it.
    // Neither this nor `work_mbid` can come from any other source — a media
    // server surfaces neither, and MusicBrainz would have to be asked per
    // listen.
    #[serde(default, deserialize_with = "lenient")]
    pub release_track_mbid: Option<String>,
    /// The composition rather than a performance of it. Only a tagging client
    /// ever knows it.
    #[serde(default, deserialize_with = "lenient")]
    pub work_mbid: Option<String>,
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub artist_mbids: Option<Vec<String>>,
    /// Whoever the *record* is credited to. On a compilation or a soundtrack
    /// that is not the performer, which is why it is a field of its own rather
    /// than a fallback for `artist_mbids`.
    #[serde(default, deserialize_with = "lenient_string_list")]
    pub album_artist_mbids: Option<Vec<String>>,

    /// Everyone credited, where the client can separate them — the only
    /// structured way to say "Bach *and* Hilary Hahn". All of them are recorded
    /// and the artist pages and charts aggregate over every one, so a soloist
    /// appears under their own name rather than vanishing into the composer's.
    ///
    /// Failing this, a **semicolon-separated** `artist_name` is split on `;`
    /// and nothing else. `&` and `,` appear inside real names — "Simon &
    /// Garfunkel", "Earth, Wind & Fire" — so splitting on those would invent
    /// artists who do not exist.
    // Ingest used to hardcode `artists: None` and threw the credit away even
    // when it was there to read. Anything that is not an array of strings is
    // ignored rather than fatal — the trap `lenient_track_number` exists for.
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
    #[serde(default, deserialize_with = "lenient")]
    pub tapedeck_source: Option<TapedeckSource>,
}

/// The schema of a field that takes a number or a numeric string.
fn integer_or_string() -> utoipa::openapi::schema::Object {
    use utoipa::openapi::schema::{ObjectBuilder, SchemaType, Type};
    ObjectBuilder::new()
        .schema_type(SchemaType::from_iter([Type::Integer, Type::String, Type::Null]))
        .build()
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

/// ListenBrainz's read-side MBID block, accepted as a fallback for the
/// fields in `additional_info`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MbidMapping {
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub artist_mbids: Option<Vec<String>>,
    /// Cover Art Archive id.
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

// ── Tapedeck-specific extensions (roadmap 4.2) ──

/// How the audio actually sounded, as a **submitting client** reports it in
/// `additional_info.tapedeck_audio`. Every field is optional; a plugin that
/// can see its decoder reports most, a bare scrobble client none.
///
/// This is the write-side shape. A stored listen carries the same information
/// **flat** on the `Listen` object rather than nested.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TapedeckAudio {
    /// `pcm`, `dsd`, or `mqa`. Not the codec.
    pub format_type: Option<String>,
    /// The codec as your decoder names it — `flac`, `mp3`, `dsd_lsbf_planar`.
    /// Tapedeck matches DSD and PCM by **prefix**, so ffmpeg's per-ordering and
    /// per-sample-format spellings all classify correctly.
    pub codec: Option<String>,
    /// kbps.
    pub bitrate: Option<i32>,
    /// Hz.
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i16>,
    pub channels: Option<i16>,
    /// The file container, e.g. `ogg`, `m4a`.
    pub container: Option<String>,
    pub is_lossless: Option<bool>,
    /// What the source claims to be, where that differs from what was measured.
    pub source_quality: Option<String>,
    // DSD fields
    /// Hz.
    pub dsd_rate: Option<i64>,
    /// 64, 128, 256… Derived from `dsd_rate` when absent, and left unset rather
    /// than guessed when the rate is not a real DSD multiple.
    pub dsd_multiplier: Option<i16>,
    // Delivery (BT / transcode)
    /// Set **only** when the audio was genuinely transcoded. Remuxing a
    /// container without touching the audio is not a transcode, nor is a video
    /// transcode that left the audio alone. Scoring docks points whenever this
    /// is set, so filling it on a direct play quietly penalises the best
    /// listens on the system.
    pub delivery_codec: Option<String>,
    /// kbps.
    pub delivery_bitrate: Option<i32>,
    /// Hz.
    pub delivery_sample_rate: Option<i32>,
    pub delivery_bit_depth: Option<i32>,
    /// Usually derived from the signal chain rather than reported. SACD's DSD
    /// layer cannot leave a player over coax or optical, so nothing downstream
    /// will ever tell you this directly.
    pub dsd_to_pcm_converted: Option<bool>,
    pub is_transcoded: Option<bool>,
    pub transcode_reason: Option<String>,
}

/// What was playing it. Resolves to a `devices` row, which is rung 4 of the
/// chain ladder — without it a listen from a client with no token default and
/// no output binding gets no signal chain at all.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TapedeckDevice {
    pub player_name: Option<String>,
    pub player_version: Option<String>,
    pub platform: Option<String>,
    /// **The key.** Stable per installation and never the display name, which
    /// the user edits — keying on a name re-registers the device as a new one
    /// the moment it is renamed. Nothing is recorded without this.
    pub machine_id: Option<String>,
    /// What the audio came out of. Recorded whether or not it is mapped, so the
    /// UI can offer unmapped ones for one-tap assignment, and it drives rung 2
    /// of the chain ladder.
    pub output_device: Option<String>,
    pub output_type: Option<String>,
    /// USB, Bluetooth, analog… Preferred over `output_type`.
    pub interface: Option<String>,
}

/// Signal chain, rung 1 of the four-rung ladder: explicit name →
/// output-device binding → the submitting token's `default_chain_id` → the
/// source device's default. An unknown name resolves to no chain rather than
/// erroring.
///
/// Note this is an **object**. It was documented as a bare string until
/// v0.66.0 and never was one.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TapedeckChain {
    /// The chain's **name**, despite the field being called `chain_id` — a
    /// client knows what the user called it, not its row id. Use `?dry_run=1`
    /// to check it resolves.
    pub chain_id: Option<String>,
    /// Accepted and not yet stored.
    pub components: Option<Vec<String>>,
}

/// What the client was doing when the track came up — specifically, whether
/// the listener chose it.
// Deliberately two fields where there were five. The struct existed since the
// extensions were first sketched and **nothing anywhere read it** — it was
// deserialised on every submission and dropped, so a client filling it in got
// silence with no error to say so. Half-present is the worst of the available
// states: it costs a client author real work and returns nothing.
//
// The two kept are the two that answer a question nothing else in Tapedeck
// can: *what did I put on deliberately, and what did shuffle hand me?* The
// three dropped could not. `volume_level` at the moment of submission says
// nothing about a listen — it is a fact about a slider, and one sample of it
// at that. `skip_count` is per-session state the server derives from the
// listens themselves, and better, since it does not reset when a client
// restarts. `is_repeat` is a mode rather than an event: a repeat that actually
// happened is already visible as two listens of the same track.
//
// A dropped field costs nothing to reinstate if a use for it appears. Storing
// one nothing reads is how this struct got here.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TapedeckSession {
    /// Whether shuffle chose this track rather than the listener.
    pub is_shuffle: Option<bool>,
    /// What it was played from — a playlist name, an album, a queue, radio.
    /// Free text: it is your vocabulary for your own containers and there is
    /// no shared one to normalise against.
    #[serde(default, deserialize_with = "lenient")]
    pub queue_source: Option<String>,
}

/// Where the playhead is, from a client that can actually see it.
///
/// The ListenBrainz submission format carries no position, so a `playing_now`
/// without this is wall-clock arithmetic from the moment you spoke — right
/// until the listener pauses or seeks, wrong from then on, and reported as an
/// estimate (`position_known: false`) rather than dressed up as a measurement.
/// Send this and the deck shows a real position.
///
/// A playhead sent here is believed **outright**, including a step backwards:
/// a polled source's report may lag its player by a few seconds and is smoothed
/// for it, but a client measuring at the instant it sends has no such lag, so
/// a backward step is the listener seeking and you are the authority on that.
// [`Progress`](super::now_playing::Progress) has modelled the difference
// between a measurement and a guess for a while; this is the missing half, the
// way for a client to supply the measurement. Both fields are lenient, because
// this rides in `additional_info` where a strict type costs the entire
// submission.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TapedeckPlayback {
    /// Exact playhead at the instant of submission, in milliseconds. A number
    /// or a numeric string.
    #[serde(default, deserialize_with = "lenient_millis")]
    #[schema(schema_with = integer_or_string)]
    pub position_ms: Option<i64>,
    /// `playing` or `paused`. Anything other than `paused` is read as playing,
    /// which is the safe way to fall: a wrongly-paused deck freezes and reads
    /// as broken.
    ///
    /// **Send `paused` when the listener pauses.** Without it the position
    /// counts forward through the pause while `position_known` asserts it is a
    /// measurement — a confident and completely wrong readout. A paused entry
    /// holds still and expires after ten minutes of silence, so a heartbeat
    /// keeps it alive and stopping altogether clears it.
    // Lenient like everything else in this bag. A bare `Option<String>` here
    // is a *strict* type — a client sending a number or an object for `state`
    // would fail the whole submission with a 422 before the handler ran, and
    // take every listen in the batch with it. Caught by
    // `only_an_explicit_pause_pauses_the_deck`, which is the exact shape of
    // the fooyin bug arriving in a new field.
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
}

/// `additional_info.tapedeck_source` — which library item was played, from a
/// plugin running inside the media server (the Navidrome plugin today).
///
/// Only read when the submitting token was made on a Navidrome source, or the
/// user has exactly one: the id means nothing without knowing which server it
/// belongs to. The listen is then stored as if that source had polled it, so
/// the cover proxy and everything else that reads a library id works as-is.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TapedeckSource {
    /// `navidrome`. Anything else is ignored.
    pub kind: Option<String>,
    /// The library's own id for the played track (Navidrome's media file id).
    pub song_id: Option<String>,
}

impl TapedeckSource {
    /// The Navidrome song id, when this names one that is safe to hand back to
    /// Navidrome as a Subsonic id. A client-supplied string, so it is checked.
    pub fn navidrome_song_id(&self) -> Option<&str> {
        if self.kind.as_deref().map(str::trim) != Some("navidrome") {
            return None;
        }
        let id = self.song_id.as_deref()?.trim();
        (!id.is_empty()
            && id.len() <= 128
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        .then_some(id)
    }
}

// ── Response types ──

/// Everything past `status` is a Tapedeck extension and purely additive —
/// ListenBrainz clients branch on `status` alone, so the extra keys cannot
/// break them. They exist because a batch submit was otherwise all-or-nothing
/// from the client's side: "ok" said nothing about which listens were stored,
/// which were already held, and which were malformed.
#[derive(Debug, Serialize, ToSchema)]
pub struct SubmitListensResponse {
    /// Always `ok`.
    #[schema(example = "ok")]
    pub status: String,
    /// Newly stored.
    pub accepted: u32,
    /// **Singular.** Recognised as already held, by `source_id` or by the
    /// fuzzy window. Not an error — a client resending after a dropped
    /// connection is the normal way this happens. But a client whose every
    /// listen comes back here has a bug, which is why it is counted apart from
    /// `accepted` rather than folded in.
    pub duplicate: u32,
    /// Could not be stored and **must not be retried** — malformed client
    /// data, so resending changes nothing. Always present, empty when nothing
    /// was rejected; an absent key would be ambiguous.
    pub rejected: Vec<RejectedListen>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RejectedListen {
    /// Index into the submitted `payload`, so the client can map it back to
    /// what it sent.
    pub index: usize,
    pub reason: String,
}

/// The reply to `?dry_run` — what *would* have happened.
#[derive(Debug, Serialize, ToSchema)]
pub struct DryRunResponse {
    /// Always `ok`.
    #[schema(example = "ok")]
    pub status: String,
    /// Always `true`.
    pub dry_run: bool,
    pub listens: Vec<DryRunListen>,
    pub rejected: Vec<RejectedListen>,
}

/// One listen's resolved state, computed and thrown away.
#[derive(Debug, Serialize, ToSchema)]
pub struct DryRunListen {
    /// Index into the submitted `payload`.
    pub index: usize,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub timestamp: i64,
    /// What your `tapedeck_audio` fields scored, 0–100 — the main thing worth
    /// getting right and otherwise invisible.
    // The score `AudioQuality::compute_score` derives from the submitted fields.
    pub quality_score: Option<f64>,
    pub chain_id: Option<i64>,
    pub chain_name: Option<String>,
    /// **Which rung of the ladder won:** `explicit`, `output_binding`,
    /// `token_default`, `device_default` or `none`. The single most useful
    /// field here: a chain arriving from the wrong rung looks identical to one
    /// arriving from the right one, so without this a misconfiguration is
    /// invisible until someone reads their history months later.
    #[schema(
        value_type = String,
        pattern = "^(explicit|output_binding|token_default|device_default|none)$"
    )]
    pub chain_source: &'static str,
    pub device_id: Option<i64>,
    pub listening_context: Option<String>,
    /// `pending` will be forwarded onward; `imported` is stored only. The
    /// distinction a backfill has to get right, answered before it is too late
    /// to change.
    #[schema(value_type = String, pattern = "^(pending|imported)$")]
    pub status_if_stored: &'static str,
    pub skipped: bool,
    /// Whether dedup would have swallowed it, checked read-only against the
    /// real rules.
    pub duplicate: bool,
}

/// A submit answers one of these: the outcome, or — with `?dry_run` — the
/// preview.
#[derive(Serialize, ToSchema)]
#[serde(untagged)]
#[allow(dead_code)] // describes the two bodies `submit_listens` sends
pub enum SubmitOutcome {
    Stored(SubmitListensResponse),
    DryRun(DryRunResponse),
}

/// Every error body in the API has this shape.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = Error)]
pub struct ErrorResponse {
    /// Mirrors the HTTP status.
    pub code: u16,
    /// Human-readable. Not a stable identifier — do not branch on it.
    pub error: String,
}

/// An acknowledgement with nothing else to report.
///
/// `status` names what happened — `ok`, `updated`, `deleted`, `cleared`,
/// `started` and so on; the operation says which it sends. A client needs only
/// the HTTP status to know it worked.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = Ok)]
pub struct OkResponse {
    pub status: &'static str,
}

impl OkResponse {
    pub fn new(status: &'static str) -> axum::Json<Self> {
        axum::Json(Self { status })
    }
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
        assert_eq!(pb.position_ms, Some(84210));
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

    /// The song id goes back to Navidrome as a Subsonic id, so only the shapes
    /// Navidrome issues get through, and only under `kind: navidrome`.
    #[test]
    fn a_tapedeck_source_names_a_song_only_when_it_is_a_plain_navidrome_id() {
        let src = |kind: &str, id: &str| TapedeckSource {
            kind: Some(kind.into()),
            song_id: Some(id.into()),
        };
        assert_eq!(src("navidrome", " 2f8aB_c-1 ").navidrome_song_id(), Some("2f8aB_c-1"));
        assert_eq!(src("plex", "abc").navidrome_song_id(), None);
        assert_eq!(src("navidrome", "").navidrome_song_id(), None);
        assert_eq!(src("navidrome", "../rest/deleteUser").navidrome_song_id(), None);
        assert_eq!(src("navidrome", "a&u=admin").navidrome_song_id(), None);
        assert_eq!(src("navidrome", &"a".repeat(129)).navidrome_song_id(), None);
    }
}
