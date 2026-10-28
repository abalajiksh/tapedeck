use serde::{Deserialize, Serialize};

// ── Auth ──

/// Authenticated user identity, resolved from a session cookie or API token.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i64,
    pub username: String,
    pub is_admin: bool,
}

// ── Core Scrobble ──

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Play {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub timestamp: u64,
    pub duration: Option<u64>,
    pub track_number: Option<i32>,
    pub source_id: String,
    pub source_name: String,
    pub mbid_recording: Option<String>,
    pub mbid_release: Option<String>,
    pub mbid_artist: Option<Vec<String>>,
    pub artists: Option<Vec<String>>,
    pub mbid_release_group: Option<String>,
    /// The *track* on a particular release, as distinct from the recording:
    /// the same recording appears as a different release-track on an album, a
    /// single and a compilation. A tagging client is the only thing that ever
    /// knows which one was played.
    pub mbid_release_track: Option<String>,
    /// The composition, not a performance of it. Where the rest of these are
    /// near-useless for classical — one work spans dozens of recordings by
    /// different performers — this is the identifier that holds it together.
    pub mbid_work: Option<String>,
    /// Whoever the *record* is credited to, which on a compilation or a
    /// soundtrack is not the performer of the track. Kept apart from
    /// `mbid_artist` for the same reason `artist_credit` is kept apart from
    /// `artist`.
    pub mbid_album_artist: Option<Vec<String>>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
    /// The recording's ISRC, when anything knew it. The only identifier that
    /// crosses between a library, a streaming catalogue and MusicBrainz, which
    /// is what makes it worth carrying on a listen at all.
    pub isrc: Option<String>,
}

impl Play {
    pub fn new(
        title: String,
        artist: String,
        timestamp: u64,
        source_id: String,
        source_name: String,
    ) -> Self {
        Self {
            title,
            artist,
            album: None,
            timestamp,
            duration: None,
            track_number: None,
            source_id,
            source_name,
            mbid_recording: None,
            mbid_release: None,
            mbid_artist: None,
            artists: None,
            mbid_release_group: None,
            mbid_release_track: None,
            mbid_work: None,
            mbid_album_artist: None,
            caa_id: None,
            caa_release_mbid: None,
            isrc: None,
        }
    }
}

// ── Phase 2: Audio Quality ──

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioQuality {
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i16>,
    pub channels: Option<i16>,
    pub container: Option<String>,
    pub is_lossless: Option<bool>,
    pub dsd_rate: Option<i64>,
    pub dsd_multiplier: Option<i16>,
    pub delivery_codec: Option<String>,
    pub delivery_bitrate: Option<i32>,
    pub delivery_sample_rate: Option<i32>,
    pub delivery_bit_depth: Option<i32>,
    /// What the player believes it was handed — "lossless", "hi-res", "cd",
    /// "lossy". The player's own claim about the file, kept apart from the
    /// measurements beside it because the two can disagree and the
    /// disagreement is the interesting part.
    pub source_quality: Option<String>,
    /// Was a DSD stream decoded to PCM before it reached the output?
    ///
    /// Deliberately separate from `is_transcoded`: a DSD→PCM conversion is not
    /// a bitrate compromise, it's a change of representation, and on a chain
    /// built around a DSD-capable DAC it is the difference between native
    /// playback and the thing you bought the DAC to avoid. It does **not**
    /// enter `compute_score` — see the note there.
    pub dsd_to_pcm_converted: Option<bool>,
    pub is_transcoded: Option<bool>,
    pub transcode_reason: Option<String>,
    pub quality_score: Option<f64>,
}

/// How a listen should be filed: is it a backfill, and was it skipped?
///
/// Both answers come out as `status = 'imported'`, which is what takes a row
/// out of `flush_pending`'s queue — so this is the one place that decides
/// whether a listen is ever forwarded to Last.fm / ListenBrainz / Libre.fm.
/// It exists as a struct rather than two more positional arguments because the
/// coupling is the point: forgetting either one puts a wrong scrobble on
/// someone's permanent public record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListenDisposition {
    /// The submission declared itself a backfill (`listen_type: "import"`, or
    /// a file import). A historical listen is not news — forwarding it
    /// rewrites the user's scrobble history at the far end.
    pub imported: bool,
    /// The client reported the track was skipped rather than played through.
    /// Stored so skip-rate analytics has something real to count, excluded
    /// from every count, and never forwarded.
    pub skipped: bool,
}

impl ListenDisposition {
    pub const LIVE: Self = Self { imported: false, skipped: false };

    /// `'pending'` only for a live, played-through listen; everything else is
    /// filed as already-delivered so the flush loop leaves it alone.
    pub fn status(&self) -> &'static str {
        if self.imported || self.skipped {
            "imported"
        } else {
            "pending"
        }
    }
}

/// What a pushing client observed about *how* a track played, as opposed to
/// what it was.
///
/// Deliberately **not** folded into [`ListenDisposition`], for the reason
/// `ImportProvenance` is not either: that struct decides whether a row is ever
/// forwarded to someone's permanent public record, and fields with no
/// consequences should not share a home with fields whose consequences are
/// irreversible. Nothing here changes what happens to the listen.
///
/// Every field is null for every source that cannot see it, which today is all
/// of them but a pushing client — a poll can report a playhead but not how much
/// of it the listener actually heard, and an import carries neither.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaybackReport {
    /// Milliseconds actually heard, paused time excluded. The half `skipped`
    /// could never express: a bool cannot separate "cut short three seconds in"
    /// from "cut short with thirty seconds left".
    pub listened_ms: Option<i64>,
    /// Whether shuffle chose the track rather than the listener.
    pub is_shuffle: Option<bool>,
    /// What it was played from, in the client's own vocabulary.
    pub queue_source: Option<String>,
}

impl PlaybackReport {
    /// Whether the client said anything at all. Callers use this to skip the
    /// write entirely, so a submission that reports none of this — which is
    /// every client in existence until one is built for it — costs no extra
    /// statement per listen.
    pub fn is_empty(&self) -> bool {
        self.listened_ms.is_none() && self.is_shuffle.is_none() && self.queue_source.is_none()
    }
}

/// When a listen counts — how much of a track has to play before it is banked.
///
/// The default is Last.fm's convention and the number every session-shaped
/// source here has always hardcoded: **half the track, or four minutes,
/// whichever comes first**. That is a convention, not a fact, and it fits a
/// three-minute pop song far better than it fits a forty-minute raga, a
/// nineteen-minute side-long piece, or a ninety-second hardcore track — which is
/// why it is a per-user setting rather than a constant.
///
/// Both halves are kept, and "whichever comes first" is the whole point of
/// having two: the fraction is what makes a short track count, and the flat
/// number is what stops a long one needing twenty minutes before it exists.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrobbleThreshold {
    /// Share of the track that must have played, 0.0–1.0.
    pub fraction: f64,
    /// Seconds after which it counts regardless of the fraction. Also the only
    /// rule available when the source reported no duration.
    pub after_secs: u64,
}

impl Default for ScrobbleThreshold {
    fn default() -> Self {
        Self { fraction: Self::DEFAULT_FRACTION, after_secs: Self::DEFAULT_AFTER_SECS }
    }
}

impl ScrobbleThreshold {
    pub const DEFAULT_FRACTION: f64 = 0.5;
    pub const DEFAULT_AFTER_SECS: u64 = 240;

    /// Read the user's stored preferences, clamped.
    ///
    /// Clamped rather than validated-and-rejected because this is read on the
    /// poll path, where there is nobody to report an error to — and the floors
    /// are doing real work. A fraction of zero would bank a listen the instant a
    /// track appeared in a session list, including one the listener skipped past
    /// in a second, and it would do it to their permanent public Last.fm record.
    pub fn from_prefs(prefs: &serde_json::Value) -> Self {
        let percent = prefs.get("scrobble_threshold_percent").and_then(|v| v.as_f64());
        let secs = prefs.get("scrobble_threshold_secs").and_then(|v| v.as_u64());
        Self {
            fraction: percent
                .map(|p| (p / 100.0).clamp(0.05, 1.0))
                .unwrap_or(Self::DEFAULT_FRACTION),
            after_secs: secs.map(|s| s.clamp(10, 3600)).unwrap_or(Self::DEFAULT_AFTER_SECS),
        }
    }

    /// Did the listener choose this, or is it the default?
    ///
    /// Either key alone is a choice — the settings screen writes both, but a
    /// client or a hand-edited preferences object may set one — so this is an
    /// OR, and `from_prefs` fills the other half from the default either way.
    ///
    /// Split out because two callers ask it and they must not disagree: the
    /// engine skips users who never set one so the default stays the single
    /// source of truth for everybody else, and
    /// `GET /api/v1/scrobble-settings` reports it so a client can say whether
    /// the number it is following was chosen or inherited.
    pub fn is_configured(prefs: &serde_json::Value) -> bool {
        prefs.get("scrobble_threshold_percent").is_some()
            || prefs.get("scrobble_threshold_secs").is_some()
    }

    /// The fraction as a percentage, which is how the setting is worded
    /// everywhere a person meets it.
    ///
    /// Rounded to the nearest hundredth because `0.05 * 100.0` is
    /// `5.000000000000001` in binary floating point, and a settings screen
    /// reading that back off the API would show it.
    pub fn percent(&self) -> f64 {
        (self.fraction * 10_000.0).round() / 100.0
    }

    /// Has this playhead earned the listen?
    ///
    /// One implementation shared by every source, so the case that used to be
    /// easy to get wrong — a source reporting no duration, where the fraction is
    /// unanswerable and only the flat number applies — cannot diverge between
    /// them.
    pub fn reached(&self, position_secs: u64, duration_secs: Option<u64>) -> bool {
        match duration_secs {
            Some(d) if d > 0 => {
                position_secs as f64 >= d as f64 * self.fraction || position_secs >= self.after_secs
            }
            _ => position_secs >= self.after_secs,
        }
    }
}

/// What an import knew about a listen beyond the listen itself.
///
/// Every field is something one export format carries and the others do not, so
/// this is `Default` and the plain import path fills in none of it. Separate
/// from `ListenDisposition` on purpose: that struct decides whether a row is
/// ever forwarded, which is a safety question, and burying provenance in it
/// would make a field with no consequences share a home with fields that have
/// permanent ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportProvenance {
    /// The app the listen was played in, when the export names it. Falls back
    /// to `"import"` — what every imported row said before this existed.
    pub submission_client: Option<String>,
    /// The bare Spotify track id (no `spotify:track:` prefix), from a Spotify
    /// Extended Streaming History export. Nothing reads it yet; it is stored
    /// because it is the only external identifier that export carries and the
    /// alternative to keeping it is asking for the zip a second time.
    pub spotify_track_id: Option<String>,
    /// The `source_id` an older importer gave this same listen, when it gave
    /// it a different one.
    ///
    /// A Spotify play relayed through ListenBrainz used to be stored at its
    /// *end* time under an `import` key; it is now stored at its start under
    /// the Spotify export's own key. Without this, re-importing an archive
    /// over a history imported before the change would match neither key —
    /// the old row sits a whole play away, outside the fuzzy window — and add
    /// the listen a second time.
    pub previous_source_id: Option<String>,
}

/// Does this codec string name a DSD stream?
///
/// **Not an equality test, and that is the whole point.** Plex and
/// Jellyfin/Emby both asked `codec == "dsd"`, but both are quoting ffmpeg, and
/// ffmpeg names the four bit-orderings separately: `dsd_lsbf`, `dsd_msbf` and
/// their `_planar` variants. So a real SACD rip arrived as `dsd_lsbf_planar`,
/// missed the test, and was filed as PCM — *and* as lossy, because the lossless
/// list was an equality test on the same string. `compute_score` then took the
/// bitrate branch and scored it **60**, which is what a 320 kbps MP3 gets.
///
/// Measured on a real history: this was the entire "Lossy" bucket on a library
/// whose owner plays DSD deliberately.
///
/// `dst` is DSD's own lossless compression — what is actually written on an
/// SACD — so it belongs to the same family.
pub fn codec_is_dsd(codec: &str) -> bool {
    let c = codec.trim().to_lowercase();
    c.starts_with("dsd") || c == "dst"
}

/// Does this codec string name a lossless format?
///
/// One list, shared. There used to be two and they disagreed: Plex's omitted
/// `ape` and `wavpack`, so the same file was lossless from one server and lossy
/// from the other — a difference in the *reporter*, presented as a fact about
/// the music.
pub fn codec_is_lossless(codec: &str) -> bool {
    let c = codec.trim().to_lowercase();
    // ffmpeg spells PCM per sample format — `pcm_s16le`, `pcm_s24be` — so this
    // is a prefix test for exactly the reason the DSD one is.
    codec_is_dsd(&c)
        || c.starts_with("pcm")
        || matches!(
            c.as_str(),
            "flac"
                | "alac"
                | "wav"
                | "wave"
                | "aiff"
                | "aif"
                | "ape"
                | "wavpack"
                | "wv"
                | "tta"
                | "tak"
                | "mlp"
                | "truehd"
        )
}

/// DSD64 / DSD128 / … from the sample rate a server reported.
///
/// **Two forms arrive and both have to be read.** DSD64's true rate is
/// 2,822,400 Hz (44,100 × 64), but Plex reports **352,800** for the same file —
/// the rate in bytes rather than in bits, i.e. the true rate over eight.
///
/// They are told apart by magnitude, and the threshold is a property of the
/// formats rather than a number picked to make the arithmetic work: no PCM
/// sample rate reaches 1 MHz (768 kHz is the highest anything supports), and no
/// DSD rate falls below 2.8 MHz. So a DSD stream quoting a figure under 1 MHz
/// is quoting the byte form, and there is nothing else it could be.
///
/// Returns `None` rather than guessing when the result is not a real DSD
/// multiple — `compute_score` has a fallback for an unknown multiplier, and an
/// invented one would be worse than the fallback.
pub fn dsd_multiplier_from_rate(sample_rate: i64) -> Option<i16> {
    if sample_rate <= 0 {
        return None;
    }
    let bits = if sample_rate < 1_000_000 { sample_rate * 8 } else { sample_rate };
    let multiple = bits / 44_100;
    if bits % 44_100 != 0 {
        return None;
    }
    matches!(multiple, 64 | 128 | 256 | 512 | 1024).then_some(multiple as i16)
}

impl AudioQuality {
    /// Fill `format_type`, `is_lossless`, and the DSD rate fields from the
    /// codec and sample rate a source reported.
    ///
    /// Shared so the two session-shaped sources cannot answer differently about
    /// the same file; see `codec_is_dsd` for what they used to get wrong.
    pub fn classify_codec(&mut self) {
        // Recomputed from the codec, so whatever was there is discarded first
        // and `fill_classification` does the single implementation of the rule.
        self.format_type = None;
        self.is_lossless = None;
        self.fill_classification();
    }

    /// The same derivation, but **only where nothing said otherwise**.
    ///
    /// What a source *reports* outranks what its codec implies: a client that
    /// says `is_lossless: false` about a FLAC knows something we do not — it
    /// may be describing a lossy file in a FLAC container, or a transcode. So
    /// ingest fills the gaps and overrules nobody, where Plex and Jellyfin
    /// derive the lot because neither server reports either field.
    ///
    /// The two share one body rather than one being a copy of the other, which
    /// is the rule `codec_is_lossless` itself earned: there used to be two
    /// lossless lists and the same file was lossless from one server and lossy
    /// from the other.
    pub fn fill_classification(&mut self) {
        let codec = self.codec.clone().unwrap_or_default();
        if codec.trim().is_empty() {
            // Nothing to derive from. Leaving these unset is the honest answer
            // — `FIDELITY_KNOWN` treats a NULL `is_lossless` as "we never
            // knew", which is the population the fidelity cards confess to.
            return;
        }
        let dsd = codec_is_dsd(&codec);
        if self.format_type.is_none() {
            self.format_type = Some(if dsd { "dsd" } else { "pcm" }.to_string());
        }
        if self.is_lossless.is_none() {
            self.is_lossless = Some(codec_is_lossless(&codec));
        }
        if dsd {
            if let Some(sr) = self.sample_rate {
                if let Some(m) = dsd_multiplier_from_rate(sr as i64) {
                    if self.dsd_multiplier.is_none() {
                        self.dsd_multiplier = Some(m);
                    }
                    if self.dsd_rate.is_none() {
                        self.dsd_rate = Some(m as i64 * 44_100);
                    }
                }
            }
        }
    }

    /// **One format is one spelling.**
    ///
    /// Every source quotes whatever its upstream calls the format and they
    /// disagree about case: ffmpeg-derived paths say `flac`, fooyin says
    /// `FLAC`, and each source stored the string raw. Classification was never
    /// affected — `codec_is_lossless` and `codec_is_dsd` lowercase before
    /// testing — and neither was the badge, which uppercases for display. What
    /// splits is anything that **groups** on the column, which today is nothing
    /// and tomorrow is the first per-codec breakdown somebody writes.
    ///
    /// `format_type` is the one that was not merely latent: `FIDELITY_KNOWN`
    /// and the badge both compare it to a lowercase literal, so an uppercase
    /// `"DSD"` would have been filed as neither DSD nor known.
    ///
    /// Called from `save_scrobble`, which is the chokepoint every write path
    /// goes through — so no source can reintroduce the split by forgetting.
    /// Ingest calls it a second time, before `compute_score`, because a score
    /// is computed from `format_type` *before* the row is stored.
    pub fn normalise(&mut self) {
        fn fold(s: &mut Option<String>) {
            *s = s.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_lowercase);
        }
        fold(&mut self.format_type);
        fold(&mut self.codec);
        fold(&mut self.container);
        fold(&mut self.delivery_codec);
    }

    /// Note what is deliberately *not* in here: `dsd_to_pcm_converted` and
    /// `source_quality`. Both are stored and both are worth seeing, but the
    /// score is written once at ingest and never recomputed, so folding a new
    /// term in would silently make every listen recorded after the change
    /// incomparable with every listen before it. If these should count, the
    /// change is a backfill that rescores the whole history, not a new line
    /// here.
    pub fn compute_score(&mut self) {
        let base = match self.format_type.as_deref() {
            Some("dsd") => match self.dsd_multiplier {
                Some(m) if m >= 256 => 98.0,
                Some(128) => 95.0,
                Some(64) => 92.0,
                _ => 90.0,
            },
            Some("pcm") | None => {
                let sr = self.sample_rate.unwrap_or(0);
                let bd = self.bit_depth.unwrap_or(0);
                let lossless = self.is_lossless.unwrap_or(false);
                if !lossless {
                    match self.bitrate.unwrap_or(0) {
                        b if b >= 320 => 60.0,
                        b if b >= 256 => 55.0,
                        b if b >= 192 => 50.0,
                        b if b >= 128 => 40.0,
                        _ => 30.0,
                    }
                } else {
                    match (bd, sr) {
                        (24, s) if s >= 192000 => 90.0,
                        (24, s) if s >= 96000 => 85.0,
                        (24, _) => 82.0,
                        (16, 44100) => 80.0,
                        (16, s) if s >= 48000 => 81.0,
                        _ => 78.0,
                    }
                }
            }
            _ => 50.0,
        };
        let mut score: f64 = base;
        if self.is_transcoded.unwrap_or(false) {
            score -= 10.0;
        }
        if self.delivery_codec.is_some() {
            match self.delivery_codec.as_deref() {
                Some("ldac") => score -= 3.0,
                Some("aptx_hd") => score -= 5.0,
                Some("aac") => score -= 8.0,
                Some("sbc") => score -= 15.0,
                _ => score -= 5.0,
            }
        }
        self.quality_score = Some(score.clamp(0.0, 100.0));
    }

    pub fn has_data(&self) -> bool {
        self.format_type.is_some()
            || self.codec.is_some()
            || self.bitrate.is_some()
            || self.sample_rate.is_some()
    }
}

// ── Phase 2: Signal Chains ──

/// A signal chain: the gear a listen ran through, in order.
///
/// **There is no `is_default`.** A chain is not default for anything by
/// itself; *being the default* is a property of the thing pointing at it, and
/// there are two: a **device** (`PATCH /api/v1/devices/{id}`) and an **API
/// token**, whose `default_chain_id` is rung 3 of the chain ladder and is
/// reported by `GET /1/validate-token`.
///
/// Absent is not null here. `icon`, and a component's `equipment_id` and
/// `since`, are **omitted entirely** when unset rather than sent as null — a
/// strict deserialiser wants those optional, not nullable. `description`,
/// `detail` and `status` really are present-and-null.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = Chain)]
pub struct SignalChain {
    pub id: Option<i64>,
    pub user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub components: Vec<ChainComponent>,
    pub listening_context: ListeningContext,
    /// A name from the UI's `CHAIN_ICONS` set. **Omitted when unset**, not
    /// null. Unvalidated on purpose — an unknown value falls back to a generic
    /// mark rather than erroring, and rejecting a save because the front end
    /// learned a new icon first would be the worse failure.
    // An unrecognised value falling back is also what keeps this from being a
    // way to inject anything into the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub is_active: bool,
    /// *(since v0.76.1)* Whether this chain may be **named** to a deck patched
    /// into yours, and whether its listens reach the parts of Patch that have
    /// no patch behind them — the Instance tab, Discover, and Shared Spool
    /// captures. **Not the sharing unit** since v0.93.0: what a patched deck
    /// sees is `share_all_with_patched` on the profile.
    // It was the sharing unit, and the argument for it was that "what am I
    // publishing?" is answerable by looking at a list of chains you already
    // understand. What that missed is that a chain is a *gear configuration*,
    // and using one as an access-control list is an odd proxy for consent — a
    // patch is a decision about a person, which is what consent is.
    #[serde(default)]
    pub shared: bool,
    /// Derived from the listens (`SUM(duration)` where `chain_id` matches and
    /// the listen is not a skip), never accumulated — so assigning a chain to
    /// an old listen moves the hours, and un-assigning takes them back. A
    /// source that reported no length contributes nothing, so this is a floor.
    pub total_hours: f64,
    /// Unix seconds.
    pub created_at: i64,
}

/// One step of a chain.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChainComponent {
    #[serde(rename = "type")]
    pub component_type: ComponentType,
    /// Always written, so deleting the gear leaves the chain readable.
    pub name: String,
    /// Free text about this step. Always present; null when there is none.
    /// Guessed from the gear type when a step is first picked and never
    /// re-applied, so an override sticks.
    pub detail: Option<String>,
    /// The `equipment` row this step *is*, set when it was picked from your
    /// gear. **Omitted** for free text — a chain can name something you do not
    /// own (a friend's amp, a rental).
    // Old chains predate the link entirely. `name` stays a snapshot so a
    // deleted gear row leaves the chain readable instead of blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equipment_id: Option<i64>,
    /// Unix seconds from which this box was in the path. **Omitted means "for
    /// as long as the chain has existed"**, which is what every step written
    /// before this field meant.
    ///
    /// Chain and gear hours are derived from the listens, so without it a box
    /// added to a chain that already has history is credited with all of it.
    /// Stamped automatically on gear a chain did not name before; send one to
    /// correct the date, or `null` to say it was always there.
    // Stamped by [`Database::update_chain`], and editable, because the honest
    // date is usually a few days before anybody got round to recording it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
}

/// One step of a chain, replaced.
///
/// `position` indexes into the parent chain's `components`. Only the steps that
/// differ are stored, which is what makes a variant an override rather than a
/// copy: editing the shared part of a chain updates every variant of it, and
/// two cartridges on one turntable stay one chain.
///
/// A `position` past the end of the base components *adds* a step, so a chain
/// that never named a cartridge can gain one without being rewritten.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct VariantOverride {
    /// Index into the chain's `components`. One past the end *adds* a step.
    pub position: i64,
    /// The gear fitted at this step. Null removes the step for this variant —
    /// a chain run without its amp, say.
    #[serde(default)]
    pub equipment_id: Option<i64>,
    /// Snapshot of the name, so a deleted gear row leaves the variant readable
    /// instead of blank.
    #[serde(default)]
    pub name: Option<String>,
}

/// What kind of box a step is.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComponentType {
    Source,
    Transport,
    Dac,
    Amp,
    Transducer,
    Network,
    Bluetooth,
}

/// How you listen through a chain. **Attention, not company** — the two are
/// orthogonal, and who you were with is `company` on the listen.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListeningContext {
    Active,
    // `active-mobile`, with a hyphen: the spelling `as_str` stores, the web UI
    // offers and the published document has always named. It was serialised
    // `active_mobile` by `rename_all`, so a chain saved as "active-mobile"
    // was refused with a 422 and one stored that way came back under the
    // other spelling, which the chains page could not match. The underscore
    // form is still accepted, since it is what this used to send.
    #[serde(rename = "active-mobile", alias = "active_mobile")]
    ActiveMobile,
    Passive,
    Background,
    #[default]
    Unknown,
}

impl ListeningContext {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::ActiveMobile => "active-mobile",
            Self::Passive => "passive",
            Self::Background => "background",
            Self::Unknown => "unknown",
        }
    }
    pub fn from_str_loose(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "active-mobile" | "active_mobile" => Self::ActiveMobile,
            "passive" => Self::Passive,
            "background" => Self::Background,
            _ => Self::Unknown,
        }
    }
}

/// Do these two steps name the same thing?
///
/// Keyed on `equipment_id` when both sides have one — that is the identity the
/// hours are attributed against, and two rows of gear with the same name are
/// still two rows. Only when neither side is linked to a piece of gear does
/// this fall back to the free-text name, folded the way every other name
/// comparison in this codebase folds one.
///
/// A linked step and a free-text step are deliberately **never** equal, even
/// when the text matches: one credits an equipment row and the other credits
/// nothing, so treating them as the same step would fold a chain that logs
/// hours into one that does not.
pub fn component_matches(a: &ChainComponent, b: &ChainComponent) -> bool {
    match (a.equipment_id, b.equipment_id) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.name.trim().eq_ignore_ascii_case(b.name.trim()),
        _ => false,
    }
}

/// The overrides that turn `base`'s components into `target`'s.
///
/// This is what makes an existing near-duplicate chain foldable into a variant
/// rather than only offering variants to chains built from today on. Every
/// position where the two disagree becomes one override, and the three cases
/// the override format already supports line up exactly with the three ways two
/// chains can differ:
///
/// * **Replaced** — both have a step at this position and they differ.
/// * **Added** — `target` is longer, and a `position` past the end of the base
///   components adds a step (which `equipment_seconds` handles explicitly).
/// * **Removed** — `target` is shorter, so the trailing positions get an
///   override with no `equipment_id`, which is what removing a step means.
///
/// Positions the two agree on emit nothing, which is the whole point of an
/// override: editing the shared part of a chain must keep updating every
/// variant of it.
pub fn derive_overrides(
    base: &[ChainComponent],
    target: &[ChainComponent],
) -> Vec<VariantOverride> {
    let mut out = Vec::new();
    for i in 0..base.len().max(target.len()) {
        match (base.get(i), target.get(i)) {
            (Some(b), Some(t)) if component_matches(b, t) => {}
            (_, Some(t)) => out.push(VariantOverride {
                position: i as i64,
                equipment_id: t.equipment_id,
                name: Some(t.name.clone()),
            }),
            // The base runs on past the end of the target: this variant does
            // without those steps.
            (Some(_), None) => {
                out.push(VariantOverride { position: i as i64, equipment_id: None, name: None })
            }
            (None, None) => {}
        }
    }
    out
}

/// How many steps two chains disagree on — 0 when they are the same wiring.
///
/// Used only to rank and filter the fold suggestions; it decides what gets
/// offered, never what gets written.
pub fn chain_distance(a: &[ChainComponent], b: &[ChainComponent]) -> usize {
    (0..a.len().max(b.len()))
        .filter(|&i| match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) => !component_matches(x, y),
            _ => true,
        })
        .count()
}

// Device, Equipment, and ListeningSession rows are read back as ad-hoc tuples
// or the DeviceRow/EquipmentRow types in db.rs — no shared model struct needed.

// ── The publish switch ──

/// Which parts of a now-playing entry a user has agreed to make public.
///
/// Everything else in this application is self-only, so the public endpoint is
/// the one place where a field leaving the instance is a decision rather than a
/// consequence. That decision is per field, not per endpoint: title, artist and
/// album are the whole of the base payload, and each richer thing — the cover,
/// how far in you are, what the file was, which chain it ran through — is its
/// own answer.
///
/// **Absent means off.** A JSON array that is missing, null, malformed or names
/// something this build has never heard of yields a struct of `false`, so the
/// failure mode of a corrupt value is publishing *less* than intended rather
/// than more. That direction is not arbitrary: a lost toggle costs a widget a
/// line, and a gained one puts something on the internet nobody asked to put
/// there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicNpFields {
    /// A cover image URL for whatever is on the deck.
    pub artwork: bool,
    /// Position and duration, which is what lets a widget draw a progress bar —
    /// and also what reveals a pause and how far into a track someone is.
    pub position: bool,
    /// Codec, bit depth, sample rate, lossless/DSD.
    pub fidelity: bool,
    /// The signal chain the listen is attributed to.
    pub gear: bool,
}

impl PublicNpFields {
    /// The whole vocabulary. Anything outside it is ignored on the way in, so a
    /// value written by a newer build cannot silently grant a field an older one
    /// does not know how to withhold.
    pub const KNOWN: [&'static str; 4] = ["artwork", "position", "fidelity", "gear"];

    /// Parse the stored JSON array. Any failure is "nothing extra is public".
    pub fn from_json(raw: Option<&str>) -> Self {
        let Some(raw) = raw else { return Self::default() };
        let Ok(serde_json::Value::Array(items)) = serde_json::from_str(raw) else {
            return Self::default();
        };
        let has = |name: &str| items.iter().any(|v| v.as_str() == Some(name));
        Self {
            artwork: has("artwork"),
            position: has("position"),
            fidelity: has("fidelity"),
            gear: has("gear"),
        }
    }

    /// Serialise back to the stored form, in `KNOWN` order.
    pub fn to_json(self) -> String {
        let names: Vec<&str> = Self::KNOWN.iter().copied().filter(|n| self.has(n)).collect();
        serde_json::to_string(&names).unwrap_or_else(|_| "[]".into())
    }

    pub fn has(self, name: &str) -> bool {
        match name {
            "artwork" => self.artwork,
            "position" => self.position,
            "fidelity" => self.fidelity,
            "gear" => self.gear,
            _ => false,
        }
    }

    /// Build from whatever a request body offered, keeping only known names.
    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = Self::default();
        for n in names {
            match n.as_ref() {
                "artwork" => out.artwork = true,
                "position" => out.position = true,
                "fidelity" => out.fidelity = true,
                "gear" => out.gear = true,
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `active-mobile` is one spelling everywhere: what is stored, what is
    /// served, and what a save may send — and the underscore form a client
    /// may have learned from an older server still reads.
    #[test]
    fn active_mobile_is_spelled_the_way_it_is_stored() {
        let c: ListeningContext = serde_json::from_str(r#""active-mobile""#).unwrap();
        assert_eq!(c, ListeningContext::ActiveMobile);
        let c: ListeningContext = serde_json::from_str(r#""active_mobile""#).unwrap();
        assert_eq!(c, ListeningContext::ActiveMobile);
        assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!(c.as_str()));
        for c in [
            ListeningContext::Active,
            ListeningContext::ActiveMobile,
            ListeningContext::Passive,
            ListeningContext::Background,
            ListeningContext::Unknown,
        ] {
            assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!(c.as_str()));
            assert_eq!(ListeningContext::from_str_loose(c.as_str()), c);
        }
    }

    fn step(name: &str, gear: Option<i64>) -> ChainComponent {
        ChainComponent {
            component_type: ComponentType::Transducer,
            name: name.into(),
            detail: None,
            since: None,
            equipment_id: gear,
        }
    }

    /// The case the whole fold exists for, and the one measured on the real
    /// page: two chains that differ by exactly one box.
    #[test]
    fn one_swapped_step_derives_one_override() {
        let commute = [step("Xperia", Some(1)), step("HD 569", Some(2))];
        let summer = [step("Xperia", Some(1)), step("Hexa", Some(3))];

        assert_eq!(chain_distance(&commute, &summer), 1);
        let o = derive_overrides(&commute, &summer);
        assert_eq!(o.len(), 1, "only the step that differs is stored");
        assert_eq!(o[0].position, 1);
        assert_eq!(o[0].equipment_id, Some(3));
        assert_eq!(o[0].name.as_deref(), Some("Hexa"));
    }

    /// Identical wiring produces no overrides at all. A variant with an empty
    /// override list is the same rig under a second name, which is exactly what
    /// folding a pure duplicate should leave behind.
    #[test]
    fn identical_chains_derive_nothing() {
        let a = [step("Xperia", Some(1)), step("HD 569", Some(2))];
        let b = [step("Xperia", Some(1)), step("HD 569", Some(2))];
        assert_eq!(chain_distance(&a, &b), 0);
        assert!(derive_overrides(&a, &b).is_empty());
    }

    /// A longer target adds a step, a shorter one removes it. Both are
    /// representable, and `equipment_seconds` reads both — a `position` past
    /// the end adds, and an override with no `equipment_id` credits nothing.
    #[test]
    fn a_longer_or_shorter_chain_is_still_representable() {
        let base = [step("Xperia", Some(1))];
        let longer = [step("Xperia", Some(1)), step("Hexa", Some(3))];
        let added = derive_overrides(&base, &longer);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].position, 1, "past the end of the base — this adds a step");
        assert_eq!(added[0].equipment_id, Some(3));

        let removed = derive_overrides(&longer, &base);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].position, 1);
        assert_eq!(removed[0].equipment_id, None, "no gear at this step — it is dropped");
    }

    /// A step linked to a piece of gear and a free-text step that happens to
    /// spell the same words are **not** the same step. One accrues hours
    /// against an equipment row and the other accrues none, so folding them
    /// together would silently stop crediting the gear.
    #[test]
    fn a_linked_step_never_matches_a_free_text_one() {
        assert!(!component_matches(&step("Hexa", Some(3)), &step("Hexa", None)));
        assert!(component_matches(&step("hexa", None), &step("Hexa ", None)));
        assert!(!component_matches(&step("Hexa", Some(3)), &step("Hexa", Some(4))));
    }

    /// The bug this exists for: ffmpeg names the four DSD bit-orderings
    /// separately, so an equality test on `"dsd"` missed every real file and
    /// filed an SACD rip as lossy PCM scoring 60.
    #[test]
    fn every_spelling_ffmpeg_uses_for_dsd_is_recognised() {
        for c in
            ["dsd", "dsd_lsbf", "dsd_msbf", "dsd_lsbf_planar", "dsd_msbf_planar", "DSD_MSBF", "dst"]
        {
            assert!(codec_is_dsd(c), "{c} is DSD");
            assert!(codec_is_lossless(c), "{c} is not lossy");
        }
        for c in ["mp3", "aac", "opus", "vorbis", "flac"] {
            assert!(!codec_is_dsd(c), "{c} is not DSD");
        }
    }

    /// PCM has the same shape of problem — ffmpeg spells it per sample format,
    /// so `pcm_s24le` would have failed an equality test on `"pcm"` and been
    /// scored as a lossy file too.
    #[test]
    fn the_lossless_list_is_shared_and_covers_both_servers() {
        for c in ["flac", "alac", "wav", "aiff", "ape", "wavpack", "pcm", "pcm_s16le", "pcm_s24be"]
        {
            assert!(codec_is_lossless(c), "{c} is lossless");
        }
        for c in ["mp3", "aac", "opus", "vorbis", "wma", "ac3"] {
            assert!(!codec_is_lossless(c), "{c} is lossy");
        }
    }

    /// Both forms of the rate arrive: Plex quotes DSD64 as 352,800 (bytes) and
    /// the true rate is 2,822,400 (bits). Anything that is not a real DSD
    /// multiple is left unset rather than guessed at.
    #[test]
    fn a_dsd_rate_is_read_in_either_form() {
        assert_eq!(dsd_multiplier_from_rate(352_800), Some(64));
        assert_eq!(dsd_multiplier_from_rate(2_822_400), Some(64));
        assert_eq!(dsd_multiplier_from_rate(705_600), Some(128));
        assert_eq!(dsd_multiplier_from_rate(5_644_800), Some(128));
        assert_eq!(dsd_multiplier_from_rate(11_289_600), Some(256));
        assert_eq!(dsd_multiplier_from_rate(44_100), None);
        assert_eq!(dsd_multiplier_from_rate(96_000), None);
        assert_eq!(dsd_multiplier_from_rate(0), None);
    }

    /// The whole path, on the bytes a real Plex session reported for a DSF:
    /// what used to score 60 as lossy PCM is DSD64 and scores 92.
    #[test]
    fn a_real_dsf_session_is_scored_as_dsd_not_as_a_320k_mp3() {
        let mut q = AudioQuality {
            codec: Some("dsd_lsbf_planar".into()),
            container: Some("dsf".into()),
            sample_rate: Some(352_800),
            ..Default::default()
        };
        q.classify_codec();
        q.compute_score();
        assert_eq!(q.format_type.as_deref(), Some("dsd"));
        assert_eq!(q.is_lossless, Some(true));
        assert_eq!(q.dsd_multiplier, Some(64));
        assert_eq!(q.dsd_rate, Some(2_822_400));
        assert_eq!(q.quality_score, Some(92.0));
    }

    /// The two halves are an OR, and both directions matter: the fraction is
    /// what lets a ninety-second track count at all, and the flat number is what
    /// stops a nineteen-minute side needing nine and a half minutes first.
    #[test]
    fn either_half_of_the_threshold_is_enough() {
        let t = ScrobbleThreshold::default();

        assert!(!t.reached(40, Some(180)), "40s of a 3:00 track is not half of it");
        assert!(t.reached(90, Some(180)), "half of a 3:00 track counts");

        // 20 minutes long, so half is ten — but four minutes is the other rule.
        assert!(t.reached(240, Some(1200)));
        assert!(!t.reached(239, Some(1200)));
    }

    /// A source that reported no duration leaves the fraction unanswerable, so
    /// only the flat number applies. This lived separately in each source
    /// before; it lives here now so the four cannot diverge.
    #[test]
    fn with_no_duration_only_the_flat_rule_applies() {
        let t = ScrobbleThreshold::default();
        assert!(!t.reached(239, None));
        assert!(t.reached(240, None));
        assert!(t.reached(240, Some(0)), "a zero duration is no duration");
    }

    /// The listener's numbers, not ours.
    #[test]
    fn a_configured_threshold_is_the_one_that_applies() {
        let prefs = serde_json::json!({
            "scrobble_threshold_percent": 80,
            "scrobble_threshold_secs": 600
        });
        let t = ScrobbleThreshold::from_prefs(&prefs);

        assert!(!t.reached(90, Some(180)), "half is no longer enough");
        assert!(t.reached(144, Some(180)), "80% is");
        assert!(!t.reached(240, Some(3600)), "and four minutes no longer is");
    }

    /// Read on the poll path, where there is nobody to hand an error to — so a
    /// nonsense value is clamped rather than rejected. The floor is load-bearing
    /// rather than tidy: a fraction of zero would bank a listen the instant a
    /// track appeared in a session list, and forward it to somebody's permanent
    /// public record.
    #[test]
    fn a_nonsense_threshold_is_clamped_not_obeyed() {
        let zero = ScrobbleThreshold::from_prefs(&serde_json::json!({
            "scrobble_threshold_percent": 0,
            "scrobble_threshold_secs": 0
        }));
        assert!(!zero.reached(0, Some(300)), "nothing counts at zero seconds played");
        assert!(zero.fraction >= 0.05);
        assert!(zero.after_secs >= 10);

        let silly = ScrobbleThreshold::from_prefs(&serde_json::json!({
            "scrobble_threshold_percent": 5000,
            "scrobble_threshold_secs": 999_999
        }));
        assert_eq!(silly.fraction, 1.0, "at most the whole track");
        assert_eq!(silly.after_secs, 3600);

        // An empty settings object is the overwhelmingly common case.
        assert_eq!(
            ScrobbleThreshold::from_prefs(&serde_json::json!({})),
            ScrobbleThreshold::default()
        );
    }

    /// The number a client is handed has to be the number the server applies.
    ///
    /// `GET /api/v1/scrobble-settings` exists so a scrobble client submits at
    /// the same point the engine would have banked at — and the same point it
    /// writes an unbanked track off as a skip. Reporting the *stored*
    /// preferences instead of the clamped ones would hand a client a rule
    /// nothing here obeys, and the disagreement is silent at both ends.
    #[test]
    fn the_reported_threshold_is_the_one_the_server_applies() {
        let prefs = serde_json::json!({
            "scrobble_threshold_percent": 0,
            "scrobble_threshold_secs": 0
        });
        let t = ScrobbleThreshold::from_prefs(&prefs);

        assert_eq!(t.percent(), 5.0, "the clamped fraction, not the stored zero");
        assert_eq!(t.after_secs, 10);
        // The reported percent and the fraction the server multiplies by are
        // the same number, or a client's arithmetic diverges from ours.
        assert!((t.percent() / 100.0 - t.fraction).abs() < f64::EPSILON);

        assert_eq!(ScrobbleThreshold::default().percent(), 50.0);

        // Eight of the settings screen's ninety-six whole percentages do not
        // survive the round trip through a fraction — `7 / 100.0 * 100.0` is
        // `7.000000000000001` — and a settings screen reading that back off
        // the API would show it. 7 and 58 are two of the eight; 50 is not,
        // which is why the default alone proves nothing here.
        for p in [7, 14, 28, 29, 55, 56, 57, 58] {
            let t = ScrobbleThreshold::from_prefs(
                &serde_json::json!({ "scrobble_threshold_percent": p }),
            );
            assert_eq!(t.percent(), f64::from(p), "{p}% must read back as itself");
        }
    }

    /// Chosen or inherited is a different question from what the numbers are,
    /// and two callers ask it: the engine skips users who never set one, and
    /// the settings endpoint reports it so a client can say which.
    #[test]
    fn either_key_alone_counts_as_having_chosen() {
        assert!(!ScrobbleThreshold::is_configured(&serde_json::json!({})));
        assert!(!ScrobbleThreshold::is_configured(&serde_json::json!({ "theme": "dark" })));
        assert!(ScrobbleThreshold::is_configured(
            &serde_json::json!({ "scrobble_threshold_percent": 50 })
        ));
        assert!(ScrobbleThreshold::is_configured(
            &serde_json::json!({ "scrobble_threshold_secs": 240 })
        ));
    }

    /// The publish switch has to fail in the direction that publishes *less*.
    ///
    /// A lost toggle costs a widget a line. A gained one puts something on the
    /// internet nobody asked to put there, and there is no taking it back once
    /// a reader has seen it — so every unreadable value is "nothing extra",
    /// including the ones that look like near-misses.
    #[test]
    fn an_unreadable_publish_setting_grants_nothing() {
        for raw in [
            None,                              // never set
            Some(""),                          // written empty
            Some("null"),                      // valid JSON, wrong shape
            Some("{\"artwork\":true}"),        // an object, not an array
            Some("[\"artwork\""),              // truncated
            Some("[\"ARTWORK\"]"),             // right word, wrong case
            Some("[\"listens\",\"history\"]"), // names this build has never heard of
        ] {
            assert_eq!(
                PublicNpFields::from_json(raw),
                PublicNpFields::default(),
                "{raw:?} must grant nothing"
            );
        }

        // And the round trip is stable, in `KNOWN` order rather than whatever
        // order a client happened to send.
        let picked = PublicNpFields::from_names(["gear", "artwork", "nonsense"]);
        assert_eq!(picked.to_json(), r#"["artwork","gear"]"#);
        assert_eq!(PublicNpFields::from_json(Some(&picked.to_json())), picked);
    }
}

// ── Promoting a variant to be the chain itself ────────────────
//
// "Two cartridges on one turntable, and the one I actually use is the variant."
// Making that the chain and demoting the old base to a variant is a
// *relabelling*: which gear was fitted for any given listen must not change,
// and neither must anybody's hours.
//
// That is harder than it looks, and the reason is the other variants. An
// override is `{position, equipment_id}` **relative to the base**, so a variant
// that swaps position 2 means "the base, but with X at 2". Change the base and
// that sentence now names different gear at position 1 — silently, with hours
// moving to a box that was never fitted. So promotion resolves every variant to
// an absolute list of steps *first*, and re-expresses each one against the new
// base afterwards.

/// Apply a variant's overrides to a chain's components.
///
/// Mirrors `equipment_seconds`' own loop exactly, which is what makes the
/// promotion below a relabelling rather than a change — if the two ever
/// disagree about what a variant means, hours move.
///
/// * An override at a position that exists **replaces** that step, keeping the
///   slot's `type`: the slot is what the chain says this position *is*, and the
///   gear type is only a guess at it. Same rule the swap dialog follows.
/// * An override past the end **adds** a step.
/// * An override with no `equipment_id` **removes** the step. Nothing in the UI
///   creates one, and `promote_variant` refuses to promote a variant that has
///   one — see there.
pub fn resolve_variant(
    base: &[ChainComponent],
    overrides: &[VariantOverride],
) -> Vec<ChainComponent> {
    let mut out: Vec<ChainComponent> = Vec::new();

    for (i, c) in base.iter().enumerate() {
        match overrides.iter().find(|o| o.position == i as i64) {
            Some(o) => {
                if o.equipment_id.is_none() && o.name.is_none() {
                    // Removed for this variant: the step is simply not there.
                    continue;
                }
                out.push(ChainComponent {
                    component_type: c.component_type.clone(),
                    name: o.name.clone().unwrap_or_else(|| c.name.clone()),
                    // The detail described the box that *was* here, not the
                    // position — keeping it under different gear is worse than
                    // nothing, and nothing errors.
                    detail: if o.equipment_id == c.equipment_id { c.detail.clone() } else { None },
                    // And `since` describes the box rather than the slot, for
                    // exactly the reason `detail` does: carried onto different
                    // gear it would assert a date that box was never there.
                    since: if o.equipment_id == c.equipment_id { c.since } else { None },
                    equipment_id: o.equipment_id,
                });
            }
            None => out.push(c.clone()),
        }
    }

    // Additions, in position order so two of them keep their relative order.
    let mut added: Vec<&VariantOverride> =
        overrides.iter().filter(|o| o.position as usize >= base.len()).collect();
    added.sort_by_key(|o| o.position);
    for o in added {
        if o.equipment_id.is_none() && o.name.is_none() {
            continue;
        }
        out.push(ChainComponent {
            // A step the base never had has no slot to ask. `Transducer` is
            // the same fallback the swap dialog uses for an added step, and the
            // caller corrects it from the gear's own type where it knows one.
            component_type: ComponentType::Transducer,
            name: o.name.clone().unwrap_or_default(),
            detail: None,
            // A variant's own steps are bounded by the listens that name the
            // variant, so there is nothing for a date to add here.
            since: None,
            equipment_id: o.equipment_id,
        });
    }

    out
}

/// Express an absolute list of steps as overrides against a base.
///
/// The inverse of `resolve_variant`, and the half that makes promotion safe: a
/// variant's *meaning* is recomputed against the new base so it names the same
/// gear it always did.
///
/// Two steps are the same when they name the same equipment row — or, for a
/// free-text step that names no gear at all, the same text. Comparing whole
/// components would emit an override for a changed `detail`, which is not a
/// change of what was fitted.
pub fn overrides_against(
    base: &[ChainComponent],
    resolved: &[ChainComponent],
) -> Vec<VariantOverride> {
    let same = |a: &ChainComponent, b: &ChainComponent| match (a.equipment_id, b.equipment_id) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.name == b.name,
        _ => false,
    };

    let mut out = Vec::new();
    for i in 0..base.len().max(resolved.len()) {
        match (base.get(i), resolved.get(i)) {
            (Some(b), Some(r)) if same(b, r) => {}
            (_, Some(r)) => out.push(VariantOverride {
                position: i as i64,
                equipment_id: r.equipment_id,
                name: Some(r.name.clone()),
            }),
            // The new base has a step this variant does not: it has to say so,
            // or the variant would silently inherit gear it never used.
            (Some(_), None) => {
                out.push(VariantOverride { position: i as i64, equipment_id: None, name: None })
            }
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod variant_promotion_tests {
    use super::*;

    fn step(name: &str, id: Option<i64>) -> ChainComponent {
        ChainComponent {
            component_type: ComponentType::Transducer,
            name: name.into(),
            detail: None,
            since: None,
            equipment_id: id,
        }
    }

    fn over(pos: i64, name: &str, id: i64) -> VariantOverride {
        VariantOverride { position: pos, equipment_id: Some(id), name: Some(name.into()) }
    }

    #[test]
    fn an_override_replaces_a_step_and_keeps_the_slot_s_type() {
        let base = vec![step("Node", Some(1)), step("2M Red", Some(2))];
        let got = resolve_variant(&base, &[over(1, "Microlinear", 3)]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "Node");
        assert_eq!(got[1].name, "Microlinear");
        assert_eq!(got[1].equipment_id, Some(3));
        assert_eq!(
            got[1].component_type, base[1].component_type,
            "the slot says what the position is"
        );
    }

    #[test]
    fn an_override_past_the_end_adds_a_step() {
        let base = vec![step("Node", Some(1))];
        let got = resolve_variant(&base, &[over(1, "Cartridge", 9)]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].name, "Cartridge");
    }

    /// The round trip, which is the whole safety argument: resolve a variant,
    /// re-express it against the *same* base, and get the overrides back.
    #[test]
    fn resolving_and_re_expressing_is_a_round_trip() {
        let base = vec![step("Node", Some(1)), step("2M Red", Some(2)), step("LS50", Some(4))];
        let overrides = vec![over(1, "Microlinear", 3)];
        let resolved = resolve_variant(&base, &overrides);
        assert_eq!(overrides_against(&base, &resolved), overrides);
    }

    /// **The trap promotion exists to avoid.**
    ///
    /// Chain: Node → 2M Red → LS50. Variant A swaps the cartridge, variant B
    /// swaps the speakers. Promote A, and B's override — "position 2 is
    /// Harbeth" — now sits on a base whose position 1 is the Microlinear. B
    /// used to mean *2M Red* + Harbeth; naively it would now mean *Microlinear*
    /// + Harbeth, and the 2M Red's hours would move to a stylus that was never
    /// in the headshell.
    #[test]
    fn promoting_one_variant_does_not_change_what_another_means() {
        let base = vec![step("Node", Some(1)), step("2M Red", Some(2)), step("LS50", Some(4))];
        let a = vec![over(1, "Microlinear", 3)];
        let b = vec![over(2, "Harbeth", 5)];

        // What B meant before.
        let b_before = resolve_variant(&base, &b);
        assert_eq!(
            b_before.iter().map(|c| c.equipment_id).collect::<Vec<_>>(),
            [Some(1), Some(2), Some(5)]
        );

        // A becomes the chain.
        let new_base = resolve_variant(&base, &a);
        // B, re-expressed.
        let b_after_overrides = overrides_against(&new_base, &b_before);
        let b_after = resolve_variant(&new_base, &b_after_overrides);

        assert_eq!(
            b_after.iter().map(|c| c.equipment_id).collect::<Vec<_>>(),
            b_before.iter().map(|c| c.equipment_id).collect::<Vec<_>>(),
            "B must still name the gear it always named"
        );
        // And it had to grow an override to say so.
        assert!(
            b_after_overrides.iter().any(|o| o.position == 1 && o.equipment_id == Some(2)),
            "B now has to pin the cartridge it was relying on the base for"
        );
    }

    /// The old base becomes a variant that restores it, and means what the base
    /// meant.
    #[test]
    fn the_old_base_survives_as_a_variant() {
        let base = vec![step("Node", Some(1)), step("2M Red", Some(2))];
        let new_base = resolve_variant(&base, &[over(1, "Microlinear", 3)]);

        let old = overrides_against(&new_base, &base);
        assert_eq!(old, vec![over(1, "2M Red", 2)]);
        assert_eq!(
            resolve_variant(&new_base, &old).iter().map(|c| c.equipment_id).collect::<Vec<_>>(),
            base.iter().map(|c| c.equipment_id).collect::<Vec<_>>()
        );
    }

    /// A variant with fewer steps than the new base has to say so, or it would
    /// silently inherit gear it never used.
    #[test]
    fn a_shorter_variant_pins_the_step_it_does_not_have() {
        let base = vec![step("Node", Some(1))];
        let new_base = vec![step("Node", Some(1)), step("Cartridge", Some(9))];
        let o = overrides_against(&new_base, &base);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].position, 1);
        assert_eq!(o[0].equipment_id, None, "the added step is removed for this variant");
        assert_eq!(resolve_variant(&new_base, &o).len(), 1);
    }
}
