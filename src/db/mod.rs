use crate::crypto::SecretBox;
use crate::models::*;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Pool};
// Only the SQLite build opens a SQLite pool. The Postgres build still links the
// driver — that is what will let it read an old file to migrate out of — but it
// does not configure one here.
#[cfg(not(feature = "postgres"))]
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::collections::{HashMap, HashSet};
#[cfg(not(feature = "postgres"))]
use std::str::FromStr;
use std::sync::Arc;
#[cfg(not(feature = "postgres"))]
use std::time::Duration;
use tracing::{debug, error, info, warn};

#[derive(Debug, FromRow, serde::Serialize)]
pub struct ScrobbleRecord {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub timestamp: i64,
    pub duration: Option<i64>,
    pub source_id: String,
    pub source_name: String,
    pub status: String,
    pub mbid_recording: Option<String>,
    pub mbid_release: Option<String>,
    pub mbid_artist: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
    pub isrc: Option<String>,
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub channels: Option<i32>,
    pub container: Option<String>,
    pub source_quality: Option<String>,
    pub is_lossless: Option<bool>,
    pub dsd_rate: Option<i64>,
    pub dsd_multiplier: Option<i32>,
    pub dsd_to_pcm_converted: Option<bool>,
    pub delivery_codec: Option<String>,
    pub delivery_bitrate: Option<i32>,
    pub delivery_sample_rate: Option<i32>,
    pub delivery_bit_depth: Option<i32>,
    pub is_transcoded: Option<bool>,
    pub transcode_reason: Option<String>,
    pub quality_score: Option<f64>,
    pub device_id: Option<i64>,
    pub chain_id: Option<i64>,
    /// Which setup of that chain was fitted. Exposed so the History edit dialog
    /// can preselect it — without this the picker could offer setups but never
    /// show which one a listen is already on, and every edit would silently
    /// look like a change.
    pub chain_variant_id: Option<i64>,
    pub session_id: Option<i64>,
    pub listening_context: Option<String>,
    pub submission_client: Option<String>,
    pub track_number: Option<i32>,
    pub artwork_url: Option<String>,
    pub source_ref: Option<i64>,
    /// How much of the track was actually heard, in milliseconds, paused time
    /// excluded. Only a pushing client can report this, so it is null for
    /// everything polled and everything imported.
    pub listened_ms: Option<i64>,
    /// Whether shuffle chose this track rather than the listener.
    pub is_shuffle: Option<bool>,
    /// What it was played from, in the client's own words.
    pub queue_source: Option<String>,
    /// Who this was heard with, or null for not marked.
    ///
    /// **Null is "unmarked", not "alone"** — most of any history predates this
    /// existing. A count of marked listens is exact; a *share* of listening
    /// that was social is not, and anything presenting one has to say so.
    pub company: Option<String>,
    /// The Shared Spool this listen belonged to, if any.
    pub joint_session_id: Option<String>,
    #[serde(default)]
    pub delivered_sinks: Option<String>,
}

/// A deck on this instance that plays a given artist.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct ArtistDeck {
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub actor_uri: Option<String>,
    /// Their plays of this artist, through shared chains only.
    pub plays: i64,
    /// Whether you already have an accepted patch with them, either direction.
    /// Decides whether their deck can be opened or has to be asked for.
    pub patched: bool,
}

/// A deck you are patched into, without any of its listening.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct DeckPeer {
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// A sitting somebody handed you, as it appears in the inbox.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct HandedSitting {
    pub uuid: String,
    pub session_uuid: String,
    pub note: Option<String>,
    pub created_at: i64,
    pub seen_at: Option<i64>,
    pub from_username: String,
    pub from_display_name: Option<String>,
    /// The sitting's own name, and when it ran. Null when it has since been
    /// regrouped out of existence — a real state rather than an error.
    pub title: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// Whether a patched deck may see a listen, and whether it may be told what it
/// ran through.
///
/// Two answers rather than one because since 2026-08-26 they can differ: a deck
/// that has opened up shows every listen, and names the chain only on the ones
/// whose chain it shares.
#[derive(Debug, Clone, PartialEq)]
pub enum Visible {
    /// Not shown at all.
    No,
    /// Shown. `Some(name)` when the chain may be named, `None` when it may not
    /// — which the screen renders as "Private signal chain" rather than as a
    /// hole.
    Yes(Option<String>),
}

/// One sitting **you** passed on, for the sender's own list.
///
/// A separate type from [`HandedSitting`] rather than a nullable-both-ways
/// version of it: the recipient's row is about who sent it, this one is about
/// who got it, and a struct carrying `from_*` and `to_*` with one half always
/// null is how a reader ends up showing the wrong name.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct SentSitting {
    pub uuid: String,
    pub session_uuid: String,
    pub note: Option<String>,
    pub created_at: i64,
    /// When they opened it. `None` means it is still unread — which is the
    /// difference between withdrawing something nobody saw and taking back
    /// something already read, and the screen says which.
    pub seen_at: Option<i64>,
    pub to_username: String,
    pub to_display_name: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// One listen inside a handed-over sitting. **Not a listen of the reader's** —
/// this enters no history, count or chart, which is why it is its own type
/// rather than a `ScrobbleRecord`.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct HandedListen {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub timestamp: i64,
    pub duration: Option<i64>,
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
    pub chain_name: Option<String>,
}

/// The document a hand-over renders to.
#[derive(Debug, serde::Serialize)]
pub struct HandedSittingDetail {
    pub uuid: String,
    pub note: Option<String>,
    pub from_username: String,
    pub from_display_name: Option<String>,
    pub title: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub listens: Vec<HandedListen>,
    /// Listens in the sitting that ran through chains the owner does not
    /// share, and are therefore not shown. **Stated rather than silently
    /// dropped** — a document with invisible holes misrepresents the evening.
    pub omitted: i64,
    /// The sitting has been regrouped out of existence since it was handed
    /// over. The document is empty and says so.
    pub gone: bool,
}

/// A pending scrobble awaiting delivery, with the set of sinks it has already
/// been delivered to (so retries only hit sinks that haven't succeeded).
pub struct PendingScrobble {
    pub id: i64,
    pub user_id: i64,
    pub play: Play,
    pub delivered: Vec<String>,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct BindingRow {
    pub id: i64,
    pub user_id: i64,
    pub identifier: String,
    pub interface: Option<String>,
    pub chain_id: Option<i64>,
    pub last_seen: i64,
    pub hits: i64,
}

/// Something a note or a love can point at: a recording, a release, an artist.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct EntityRow {
    pub id: i64,
    pub kind: String,
    pub name: String,
    /// Empty for `kind = 'artist'`, where the name *is* the artist.
    pub artist_name: String,
    pub mbid: Option<String>,
    pub created_at: i64,
}

/// One row on the Loved page: the entity, plus what the user's own history
/// says about it. Loves span three kinds, so `album` and the artwork columns
/// are absent for an artist and present for the other two.
#[derive(Debug, serde::Serialize)]
pub struct LovedEntity {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub artist_name: String,
    pub mbid: Option<String>,
    pub loved_at: i64,
    pub plays: i64,
    pub last_played: Option<i64>,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
    pub note_id: Option<i64>,
    pub note_excerpt: Option<String>,
}

/// Aggregates behind the album and artist pages (UX 4). These are shapes for
/// one user's own history, not catalogue facts — `plays` is *your* plays.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct AlbumSummary {
    pub plays: i64,
    pub seconds: i64,
    pub track_count: i64,
    pub first_heard: Option<i64>,
    pub last_heard: Option<i64>,
    pub year: Option<i32>,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct ArtistSummary {
    pub plays: i64,
    pub seconds: i64,
    pub track_count: i64,
    pub album_count: i64,
    pub first_heard: Option<i64>,
    pub last_heard: Option<i64>,
}

/// One distinct track. The quality columns are carried raw so the frontend's
/// existing `qualityLabel()` produces the same tag it does on a history row —
/// duplicating that logic server-side would let the two drift.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct TrackAggregate {
    pub title: String,
    pub plays: i64,
    pub duration: Option<i64>,
    pub track_number: Option<i32>,
    /// Set on artist top-tracks (where the album varies), null on an album page.
    pub album: Option<String>,
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bit_depth: Option<i32>,
    pub sample_rate: Option<i32>,
    pub dsd_multiplier: Option<i32>,
    pub delivery_codec: Option<String>,
    pub is_lossless: Option<bool>,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct AlbumAggregate {
    pub name: String,
    pub plays: i64,
    pub year: Option<i32>,
    /// Filled from `cover_pick` after the query, never selected directly —
    /// see [`COVER_PICK`] for why the three must come from one listen.
    #[sqlx(default)]
    pub artwork_url: Option<String>,
    #[sqlx(default)]
    pub caa_id: Option<i64>,
    #[sqlx(default)]
    pub caa_release_mbid: Option<String>,
    /// The packed pick, unpacked by `resolve_cover` before the row is served.
    #[serde(skip)]
    #[sqlx(default)]
    pub cover_pick: Option<String>,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct NoteRow {
    pub id: i64,
    pub user_id: i64,
    pub entity_id: Option<i64>,
    pub scrobble_id: Option<i64>,
    /// `<unit>:<key>` — `month:2026-08`, `week:2026-W32`, `year:2026`. See
    /// [`NoteTarget::Period`].
    pub period_key: Option<String>,
    /// A **pin**: the folded coordinate of one lyric line —
    /// `<artist_key>\x1f<folded title>\x1f<n>`. See [`NoteTarget::Line`].
    pub line_key: Option<String>,
    /// The same line's names as they were written — `<artist>\x1f<title>`.
    ///
    /// A display snapshot beside a folded key, which is the pair
    /// `scrobbles.artist_credit` and a spool's `company` already are. It exists
    /// because the identity fold has to happen in **Rust**: SQLite's `LOWER()`
    /// folds ASCII only, so an index built on it would disagree with
    /// [`Database::artist_key`] for every non-Latin name — and a list of the
    /// lines you have written about has to be able to say "Cœur de pirate"
    /// rather than the key it was filed under.
    pub line_label: Option<String>,
    pub body_md: String,
    pub visibility: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// What a note is about. Exactly one, which a CHECK on `notes` enforces.
///
/// An enum rather than three `Option` arguments on `upsert_note`, because three
/// nullable parameters where exactly one may be set is precisely the shape that
/// lets a caller pass none of them and find out from a constraint violation at
/// runtime. This makes the impossible states unwritable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteTarget {
    /// A record, an artist or a recording.
    Entity(i64),
    /// One particular playing of something — "heard this on the train at 2am"
    /// is a different thing from what you think of the record.
    Listen(i64),
    /// A week, a month or a year of listening: `<unit>:<key>`, matching
    /// `reports::period_of`.
    ///
    /// A string rather than a foreign key because a period is not a row
    /// anywhere — reports are folded out of the history on every request, so
    /// there is nothing to point at. Build it with [`Self::period`], which is
    /// what stops an unparseable key reaching the table.
    Period(String),
    /// One numbered line of a lyric — a **pin**.
    ///
    /// The design's footnote form: the pin *is* the note, targeted at the line,
    /// and it is the one rendering that can ever federate, because nothing of
    /// the provider's text is quoted into the prose — a patched-in reader sees
    /// your words and a stub.
    ///
    /// Two fields because identity and display are different jobs. `key` is
    /// folded in Rust and is what the unique index and the CHECK see; `label`
    /// is the spelling to show. Built only by [`Self::line`], so the two cannot
    /// be set apart — the same reason this is an enum rather than four
    /// nullable arguments.
    ///
    /// **Absolute, not relative.** `[[line:12]]` in a note's prose is read
    /// against whatever that note is about, because it sits inside a sentence
    /// that already has a subject. A pin has no other subject — it *is* the
    /// target — and it has to be findable from the reader and from a list of
    /// every line you have written about, neither of which has a note to read
    /// it against. So the artist and title are named outright.
    Line { key: String, label: String },
}

/// See [`NoteTarget::columns`].
type NoteColumns<'a> =
    (Option<i64>, Option<i64>, Option<&'a str>, Option<&'a str>, Option<&'a str>);

impl NoteTarget {
    /// The three units a report is bucketed by, matching `reports::Unit`.
    pub const PERIOD_UNITS: [&'static str; 3] = ["week", "month", "year"];

    /// A period target from a unit and one of `period_of`'s keys, validated.
    ///
    /// Validated at the door rather than trusted, because this key is the note's
    /// *identity*: a note filed under a malformed key is not findable again by
    /// the page that wrote it, and would look to its author like the note had
    /// silently failed to save. The shapes are exactly the three `period_of`
    /// produces — `2026`, `2026-08`, `2026-W32`.
    pub fn period(unit: &str, key: &str) -> Option<Self> {
        let unit = unit.trim().to_lowercase();
        let key = key.trim();
        if !Self::PERIOD_UNITS.contains(&unit.as_str()) {
            return None;
        }
        let shaped = match unit.as_str() {
            "year" => key.len() == 4 && key.bytes().all(|b| b.is_ascii_digit()),
            "month" => {
                let (y, m) = key.split_once('-')?;
                y.len() == 4
                    && m.len() == 2
                    && y.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit())
                    && (1..=12).contains(&m.parse::<u32>().ok()?)
            }
            _ => {
                let (y, w) = key.split_once("-W")?;
                y.len() == 4
                    && w.len() == 2
                    && y.bytes().chain(w.bytes()).all(|b| b.is_ascii_digit())
                    // ISO weeks run to 53 in a long year.
                    && (1..=53).contains(&w.parse::<u32>().ok()?)
            }
        };
        shaped.then(|| Self::Period(format!("{unit}:{key}")))
    }

    /// The separator inside a line key.
    ///
    /// ASCII 31, the unit separator — the same answer [`COVER_PICK`] gives to
    /// the same question, and for the same reason: a title may legitimately
    /// contain a pipe ("Intro | Outro") and a control character may not.
    pub const SEP: char = '\u{1f}';

    /// A pin on line `n` of a track, validated.
    ///
    /// Refuses an empty artist or title and a line number of zero, because the
    /// key *is* the note's identity: a pin filed under a malformed coordinate
    /// is one the reader can never find again, which reads to its author as a
    /// save that silently did nothing. Same rule [`Self::period`] follows.
    ///
    /// The fold is [`Database::artist_key`] — trim and lowercase — which is
    /// what [`crate::lyrics::LyricsClient::track_key`] folds with, so a pin and
    /// the lyric it points at agree about which track they mean. Deliberately
    /// *not* `normalise_artist`: that one strips punctuation and accents to
    /// answer "same performer?", and over-folding here would file one artist's
    /// lines under another's name.
    pub fn line(artist: &str, title: &str, n: usize) -> Option<Self> {
        let (artist, title) = (artist.trim(), title.trim());
        if artist.is_empty() || title.is_empty() || n == 0 {
            return None;
        }
        Some(Self::Line {
            key: format!(
                "{}{}{}{}{}",
                Database::artist_key(artist),
                Self::SEP,
                title.to_lowercase(),
                Self::SEP,
                n
            ),
            label: format!("{artist}{}{title}", Self::SEP),
        })
    }

    /// The line number out of a stored `line_key`.
    pub fn line_number(key: &str) -> Option<usize> {
        key.rsplit(Self::SEP).next()?.parse().ok()
    }

    /// The artist and title out of a stored `line_label`.
    pub fn line_names(label: &str) -> Option<(&str, &str)> {
        label.split_once(Self::SEP)
    }

    /// The five nullable columns a target writes, in the order `upsert_note`
    /// binds them: entity, listen, period, line key, line label.
    ///
    /// A named type because the tuple is now five wide and a caller binding it
    /// in the wrong order would compile perfectly and file every note against
    /// the wrong thing.
    fn columns(&self) -> NoteColumns<'_> {
        match self {
            Self::Entity(id) => (Some(*id), None, None, None, None),
            Self::Listen(id) => (None, Some(*id), None, None, None),
            Self::Period(key) => (None, None, Some(key.as_str()), None, None),
            Self::Line { key, label } => {
                (None, None, None, Some(key.as_str()), Some(label.as_str()))
            }
        }
    }
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct NoteRevisionRow {
    pub id: i64,
    pub note_id: i64,
    pub body_md: String,
    pub created_at: i64,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct ImportHistoryRow {
    pub id: i64,
    pub user_id: i64,
    pub source: String,
    pub state: String,
    pub fetched: i64,
    pub imported: i64,
    pub status: String,
    pub created_at: i64,
}

/// One suggestion on the Rediscovery screen.
///
/// Same shape whichever list produced it, so the page renders one row type and
/// the reason lives on the list rather than the item.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct RediscoveryRow {
    pub name: String,
    pub artist: String,
    pub album: Option<String>,
    pub plays: i64,
    pub last_played: Option<i64>,
    /// Filled from `cover_pick` after the query, never selected directly —
    /// see [`COVER_PICK`] for why the three must come from one listen.
    #[sqlx(default)]
    pub artwork_url: Option<String>,
    #[sqlx(default)]
    pub caa_id: Option<i64>,
    #[sqlx(default)]
    pub caa_release_mbid: Option<String>,
    /// The packed pick, unpacked by `resolve_cover` before the row is served.
    #[serde(skip)]
    #[sqlx(default)]
    pub cover_pick: Option<String>,
}

/// A record or tape you own.
///
/// Distinct from an `entity`, which is a work you have listened to: this is an
/// *object* with a pressing, a barcode and a side order. Two pressings of the
/// same album are two of these and one entity.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct PhysicalRelease {
    pub id: i64,
    pub kind: String,
    pub artist: String,
    pub title: String,
    pub year: Option<i64>,
    pub mbid_release: Option<String>,
    pub discogs_id: Option<String>,
    pub barcode: Option<String>,
    pub catalog_number: Option<String>,
    pub artwork_url: Option<String>,
    /// JSON array of `{side, position, title, duration_secs}`, served through
    /// as-is — the browser is the only thing that reads its shape.
    pub tracks: String,
    pub notes: Option<String>,
    pub added_at: i64,
    /// When you bought it — how long it has been on the shelf, which nothing
    /// else here can answer. Absent for everything shelved before the column.
    pub purchased_at: Option<i64>,
    /// How it came to be yours — `new`, `used`, `gift`, `inherited`. `None`
    /// means nobody said, never "new": an existing shelf must not be
    /// retroactively claimed as bought new.
    pub acquired_as: Option<String>,
    /// Where from — the shop, the fair, the person. Free text, because that is
    /// what makes a thrift find a story rather than a flag.
    pub acquired_from: Option<String>,
    /// What this normally gets played on. The deck pre-fills from these and
    /// writes the answer back, so a record with one turntable is asked once.
    pub chain_id: Option<i64>,
    pub equipment_id: Option<i64>,
    /// Which variant of that chain this pressing is normally played on — the
    /// cartridge you keep fitted for it. Pre-fills the deck the same way
    /// `chain_id` does.
    pub chain_variant_id: Option<i64>,
    /// Where the tape was left. Cassettes only — a record has random access and
    /// no state to remember, so these stay null for every other medium.
    pub parked_side: Option<String>,
    pub parked_secs: Option<i64>,
    /// How many sides have been played, and for how long. Derived in the
    /// query rather than kept as a counter, so they cannot drift from the
    /// plays they claim to count.
    pub play_count: i64,
    pub played_secs: i64,
    pub last_played: Option<i64>,
    /// Sleeves, booklet pages and labels held for this pressing. Derived the
    /// same way, so a card can say "29 scans" without a second request.
    pub image_count: i64,
    /// The front sleeve, when this pressing has one — so a shelf card wears the
    /// cover of the object rather than a lettered tile.
    ///
    /// `artwork_url` is what the *lookup* returned, and it is frequently empty:
    /// Discogs and MusicBrainz both answer plenty of releases without one, and
    /// nothing back-fills it. Meanwhile the add already pulls the Cover Art
    /// Archive into `physical_images`, so the sleeve was sitting one table away
    /// the whole time and the card was drawing a letter over it.
    pub cover_thumb: Option<String>,
}

/// One side played, with the gear it ran through resolved to names.
///
/// Names rather than ids because this is what leaves the building: an export
/// that says `equipment_id: 4` is unreadable anywhere except the database it
/// came from.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct PhysicalPlayRow {
    pub id: i64,
    pub release_id: i64,
    pub artist: String,
    pub title: String,
    pub kind: String,
    pub side: String,
    pub played_at: i64,
    pub duration_secs: i64,
    pub track_count: i64,
    pub equipment_name: Option<String>,
    pub chain_name: Option<String>,
}

/// One side of a pressing, and what it has been through.
///
/// Every field is derived from `physical_plays` on read — see `side_plays`.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct SidePlays {
    /// As it was recorded: "A", "B", or a disc number.
    pub side: String,
    pub plays: i64,
    /// Running time of what actually played, summed over every play.
    pub secs: i64,
    pub last_played: Option<i64>,
}

/// One scan or photograph of a physical release.
///
/// Either a remote URL (the Cover Art Archive, Discogs) or a file this server
/// holds (a scan of the user's own copy). `file_path` is deliberately **not**
/// serialised: the browser is handed a Tapedeck URL to fetch it through, so a
/// server path can never reach the page.
/// The side currently on a user's deck.
///
/// Server-side because the shelf is the one source where the browser is the
/// player, and "the browser holds the state" turned out to mean "only that
/// browser knows, and a crash loses it". The clock still runs in the page —
/// the server holds *what* is on and when it started, which is everything
/// needed to rebuild the countdown anywhere.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct PhysicalSpin {
    pub user_id: i64,
    pub release_id: i64,
    pub side: String,
    pub started_at: i64,
    /// Null while running; set to the moment it was paused.
    pub paused_at: Option<i64>,
    /// Seconds spent stopped, so elapsed is wall clock minus this. A paused
    /// record is not playing, and counting it puts every later track late.
    pub paused_total: i64,
    /// JSON array of `{position, started_at, skipped}` — written and read
    /// whole, never queried into.
    pub outcomes: String,
    pub chain_id: Option<i64>,
    pub equipment_id: Option<i64>,
    /// How far into the side this spin began — non-zero when the needle was
    /// dropped mid-side or a tape resumed from where it was parked.
    pub start_secs: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct PhysicalImage {
    pub id: i64,
    pub release_id: i64,
    /// `front` | `back` | `booklet` | `medium` | `spine` | `tray` | `obi` |
    /// `sticker` | `poster` | `liner` | `matrix` | `other`.
    pub kind: String,
    /// `caa` | `discogs` | `upload`.
    pub provider: String,
    pub url: Option<String>,
    pub thumb_url: Option<String>,
    #[serde(skip_serializing)]
    pub file_path: Option<String>,
    pub mime: Option<String>,
    pub bytes: Option<i64>,
    pub caption: Option<String>,
    pub position: i64,
    pub remote_id: Option<String>,
    pub added_at: i64,
}

/// One source's liveness, with nothing secret in it — see
/// [`Database::source_health`].
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct SourceHealth {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub kind: String,
    pub label: String,
    pub enabled: bool,
    pub poll_interval_secs: Option<i64>,
    pub last_polled_at: Option<i64>,
    pub last_error: Option<String>,
}

/// A configured source, credentials decrypted. `kind` is `plex` | `navidrome` |
/// `jellyfin`; `credentials` is a per-kind JSON blob (Plex: `{"token":…}`).
#[derive(Debug, Clone)]
pub struct UserSource {
    pub id: i64,
    pub user_id: i64,
    pub kind: String,
    /// User-supplied name, so two servers of the same kind are tellable apart.
    pub label: String,
    pub enabled: bool,
    pub url: String,
    pub credentials: String,
    /// The remote account the credentials resolve to — for Plex this is the Plex
    /// username, used to keep other people's sessions out of this user's history.
    pub remote_account: Option<String>,
    pub poll_interval_secs: Option<i64>,
    /// Per-source options as JSON — currently the library/device allow and block
    /// lists. A JSON blob rather than columns so a new source kind can carry its
    /// own knobs without a migration.
    pub options: String,
    pub last_polled_at: Option<i64>,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

impl UserSource {
    /// A comma-or-array option read as a lowercased list, which is the shape
    /// `PlexFilters` matches against.
    pub fn option_list(&self, key: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&self.options) else {
            return Vec::new();
        };
        match v.get(key) {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|i| i.as_str())
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            Some(serde_json::Value::String(s)) => {
                s.split(',').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Pull a field out of the credential blob.
    pub fn credential(&self, key: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(&self.credentials)
            .ok()?
            .get(key)?
            .as_str()
            .map(str::to_string)
    }

    pub fn is_usable(&self) -> bool {
        self.enabled && !self.url.trim().is_empty() && !self.credentials.trim().is_empty()
    }
}

/// Raw row, credentials still sealed. Never leaves `db.rs`.
#[derive(FromRow)]
struct UserSourceRow {
    id: i64,
    user_id: i64,
    kind: String,
    label: String,
    enabled: bool,
    url: String,
    credentials: String,
    remote_account: Option<String>,
    poll_interval_secs: Option<i64>,
    options: String,
    last_polled_at: Option<i64>,
    last_error: Option<String>,
    updated_at: i64,
}

/// Server-wide Plex source config. The list fields are comma-separated so the
/// row stays a flat, hand-editable shape like the rest of the schema.
#[derive(Debug, Clone, Default, FromRow)]
pub struct PlexSettings {
    pub enabled: bool,
    pub url: String,
    pub token: String,
    pub users_allow: String,
    pub users_block: String,
    pub devices_allow: String,
    pub devices_block: String,
    pub libraries_allow: String,
    pub libraries_block: String,
    pub updated_at: i64,
}

impl PlexSettings {
    /// Split a stored comma-separated field into the `PlexFilters` shape.
    pub fn list(field: &str) -> Vec<String> {
        field.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    }

    /// Whether the engine can actually poll with this config.
    pub fn is_usable(&self) -> bool {
        self.enabled && !self.url.trim().is_empty() && !self.token.trim().is_empty()
    }
}

/// The database this binary was built for, chosen by the `postgres` cargo
/// feature. See [`dialect`] for what the switch does and does not cover.
///
/// **All four schemas switch together.** The main store, the MusicBrainz cache
/// (`SQLITE_DB_PATH`), the lyrics cache and the Every Noise map
/// (`EVERYNOISE_DB_PATH`) are separate SQLite *files* only because
/// `VACUUM INTO` copies a whole file and cannot exclude a table — the one way
/// to keep bulk reference data out of the user's backup. Postgres has
/// `pg_dump --exclude-table`, so under it they share one database and
/// `DATABASE_URL` is the only one to configure. Choosing Postgres should not
/// leave stray SQLite files behind that nobody asked for.
#[cfg(feature = "postgres")]
pub type Db = sqlx::Postgres;
#[cfg(not(feature = "postgres"))]
pub type Db = sqlx::Sqlite;

/// Execute a DDL blob against `pool`, translated into the active dialect and
/// split into individual statements.
///
/// One runner for all four schemas — the main store, the Every Noise map, the
/// MusicBrainz cache and the lyrics cache — so the translation cannot be
/// applied in three places and forgotten in the fourth.
/// Translate a query into the active dialect.
///
/// Every `sqlx::query*` call in this module and its children goes through it,
/// which is what turns `?` binds into `$1` and `LIKE` into `ILIKE` under
/// Postgres. Under SQLite it is the identity and borrows, so the default build
/// pays nothing — see [`dialect::rewrite`].
///
/// **A macro rather than a function**, because `sql` is a common local variable
/// name in this file and a function by that name is shadowed at two dozen call
/// sites — silently, since a `String` is callable-looking enough to produce a
/// confusing error rather than the right one. A macro cannot be shadowed by a
/// binding, and it leaves somewhere to put a per-site cache later without
/// touching 578 call sites a second time.
///
/// `migrate.rs` and `tests.rs` are the deliberate exceptions: both are
/// SQLite-only by construction, so their queries can never reach Postgres.
#[macro_export]
macro_rules! sql {
    ($q:expr $(,)?) => {
        $crate::db::dialect::rewrite($q)
    };
}

pub async fn run_ddl(pool: &Pool<Db>, sql: &str) -> Result<(), sqlx::Error> {
    for stmt in dialect::statements(&dialect::rewrite_ddl(sql)) {
        sqlx::query(&stmt).execute(pool).await?;
    }
    Ok(())
}

/// Open a pool for one of the **auxiliary** stores — the MusicBrainz cache and
/// the Every Noise map.
///
/// Under SQLite these are separate database *files*, and the reason is
/// `VACUUM INTO`: it copies a whole file and cannot exclude a table, so the
/// only way to keep 6,300 rows of someone else's reference data out of the
/// user's backup is to keep it out of the user's file.
///
/// **Under Postgres that reason disappears.** `pg_dump --exclude-table` leaves
/// them out of a backup while they live in the same database, so a Postgres
/// deployment configures **one** `DATABASE_URL` and `SQLITE_DB_PATH` /
/// `EVERYNOISE_DB_PATH` are ignored. Choosing Postgres should not leave a
/// couple of SQLite files lying about that nobody asked for.
///
/// Sharing one database is only safe because no table name collides across the
/// three schemas — `the_auxiliary_schemas_do_not_collide_with_the_main_one`
/// asserts it, so a new table named `sessions` in the lyrics cache fails the
/// build rather than shadowing the real one.
pub async fn open_aux_pool(
    sqlite_url: &str,
    primary_url: &str,
    max_connections: u32,
) -> Result<Pool<Db>, sqlx::Error> {
    #[cfg(not(feature = "postgres"))]
    {
        let _ = primary_url;
        let options = SqliteConnectOptions::from_str(sqlite_url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10))
            .synchronous(SqliteSynchronous::Normal);
        SqlitePoolOptions::new().max_connections(max_connections).connect_with(options).await
    }
    #[cfg(feature = "postgres")]
    {
        let _ = sqlite_url;
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(primary_url)
            .await
    }
}

pub struct Database {
    pool: Pool<Db>,
    /// Encrypts the credential blobs Tapedeck must keep replayable. Held here so
    /// ciphertext never escapes this module — callers pass and receive plaintext.
    secrets: Arc<SecretBox>,
}

/// What moving a listen in time actually did, and what it could not do.
///
/// Three of these four fields exist because a timestamp edit has consequences
/// this application cannot reach, and saying so is the whole difference
/// between a correction and a quiet inconsistency.
#[derive(Debug, Default, serde::Serialize)]
pub struct TimeEdit {
    /// Listens whose timestamp moved.
    pub moved: i64,
    /// Of those, how many had already been forwarded to somebody's Last.fm or
    /// ListenBrainz.
    ///
    /// **Neither service has an edit API**, so their copy keeps the time it was
    /// sent with, permanently and silently. The edit is still the right thing
    /// to do — the local record is the one that has to be true — but a screen
    /// that did not say this would be letting somebody believe they had
    /// corrected a public record they had not.
    pub already_forwarded: i64,
    /// Sessions whose aggregates were recomputed to match.
    pub sessions_touched: i64,
    /// Whether a moved listen now sits far enough from its session's
    /// neighbours that the *grouping* is stale.
    ///
    /// The aggregates are fixed here; the grouping is not, and deliberately.
    /// Sessions are stored rather than computed (which is what lets a manual
    /// merge survive), `sessions.uuid` is what a handed-over sitting points at,
    /// and re-grouping silently on a metadata edit would move an identity
    /// somebody else is holding a reference to. So this reports, and Rebuild
    /// remains a thing the user asks for.
    pub grouping_stale: bool,
}

/// How long after a listen a second report of it is still assumed to be the
/// same play rather than a repeat. See `fuzzy_exists`.
const DUPLICATE_WINDOW_SECS: i64 = 60;

/// The silence that ends a session.
///
/// Was written out as a bare `1800` at both call sites. It is named here
/// because `shift_listen_times` has to ask the same question in a third place
/// — whether moving a listen has left its session holding a gap it was grouped
/// to exclude — and three copies of a rule about what counts as contiguous
/// listening is how two of them come to disagree. A rebuild still takes it as
/// a parameter, which is the honest place for it: changing it means regrouping.
pub const SESSION_GAP_SECS: i64 = 1800;

/// How far either side of a skip to look for the tracks that bracketed it.
///
/// Generous, because the neighbours are what make a skip legible — "it did not
/// sit between these two" — and a gap of a few minutes is still the same
/// sitting. Anything genuinely unrelated is filtered out by there being no
/// listen in the window at all, which reports as `None` rather than reaching.
const SKIP_CONTEXT_WINDOW_SECS: i64 = 1800;

/// One skipped listen, with what bracketed it.
#[derive(Debug, Clone)]
pub struct SkipRow {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub timestamp: i64,
    pub duration: Option<i64>,
    /// How far in they got. `None` when nothing measured a playhead — a
    /// submitting client that reports no position — and it stays `None` rather
    /// than being filled with wall-clock arithmetic.
    pub listened_ms: Option<i64>,
    pub source: String,
    pub chain: Option<String>,
    pub before: Option<(String, String)>,
    pub after: Option<(String, String)>,
}

/// How many listen ids one `UPDATE … WHERE id IN (…)` carries during a session
/// rebuild. Comfortably under SQLite's parameter limit, and large enough that a
/// 49k-listen history is about a hundred statements rather than 49,000.
const SESSION_LINK_CHUNK: usize = 500;

/// How many track keys one duration lookup may bind at a time.
///
/// Comfortably under SQLite's bound-parameter ceiling, which the language
/// queue could otherwise walk into: the status endpoint used to ask this for
/// a hundred thousand tracks at once.
const DURATION_KEY_CHUNK: usize = 400;

/// How long `musicbrainz_cache`'s "asked, nothing there" answer is trusted.
///
/// Mirrors `NEGATIVE_CACHE_TTL_SECS` in `musicbrainz.rs`, which is the client's
/// own rule — this is the queue reading the same answer so it does not spend
/// every batch on tracks the client would refuse to look up anyway. Kept as a
/// separate constant rather than reaching across the module because it is a
/// *policy* about the queue, and a queue that skipped for longer than the
/// client caches would leave tracks unasked forever.
const MB_NEGATIVE_TTL_SECS: i64 = 7 * 24 * 3600;

/// How many listens one transaction of the artist-credit backfill covers.
///
/// This runs at startup on an existing database, so the trade is between
/// holding a write lock and paying a commit: a few thousand rows per commit
/// keeps a 49k-listen history to a couple of dozen commits without blocking
/// anything for long.
const CREDIT_BACKFILL_CHUNK: i64 = 2_000;

/// "this listen credits this artist" — the fan-out predicate, in one place.
///
/// Takes one bind: the folded artist key. Requires the outer query to alias
/// `scrobbles` as `s`.
///
/// `EXISTS` rather than a join, and that is a correctness rule rather than a
/// style one: a join multiplies the outer row by its matching credits, so
/// `COUNT(*)` would stop meaning listens the moment anything matched twice.
/// It is the same trap `get_equipment` hit when a chain naming one piece of
/// gear at two steps counted its whole running time twice.
///
/// **Where this is deliberately not used**: anything scoped to an *album*
/// (`artist` + `album` together) identifies a record, and a record's artist is
/// its primary — fanning out there would fold Hilary Hahn's own albums into
/// Bach's. Nor the sanitiser, which operates on the stored column by design,
/// nor `entity_key`, since a love points at one artist.
pub const CREDITS_ARTIST: &str = "EXISTS (SELECT 1 FROM scrobble_artists sa \
     WHERE sa.scrobble_id = s.id AND sa.artist_key = ?)";

/// The cover for a grouped row, taken from **one listen**.
///
/// A listen's artwork is three columns — `artwork_url`, and the
/// `caa_id`/`caa_release_mbid` pair a client turns into a Cover Art Archive
/// link — and they only mean anything *together*. Every grouped query used to
/// select them as three independent `MAX()`es, which is three separate
/// questions asked of the same group and answered by whichever row won each
/// one.
///
/// That is not theoretical. Reported 2026-08-21 against a Cœur de pirate record
/// whose Top Charts card wore a *5-CD compilation's* sleeve while the album page
/// showed the right one. The chart row read:
///
/// ```text
/// caa_id            44286336686   → belongs to release 9e5838c1 (the album)
/// caa_release_mbid  ead839e9-…    → a different release (the compilation)
/// artwork_url       …/ead839e9-…/32013642354-250.jpg   → a third combination
/// ```
///
/// `MAX()` on text is **lexicographic**, and `"ead839e9…" > "9e5838c1…"` because
/// `e` > `9`, so the compilation won the string comparison while `MAX(caa_id)`
/// independently picked the real album's id. Nothing errors; the group simply
/// held listens of more than one release — a track that also appears on a
/// compilation is the ordinary way that happens — and the answer was assembled
/// from whichever row won each column.
///
/// So the pick is made **once**, over a packed string whose first field is a
/// zero-padded timestamp. Lexicographic max over that is the newest row, and
/// because it is a single value the three columns cannot come from different
/// listens however the group is shaped.
///
/// Two properties worth keeping. It considers **only rows that actually have
/// artwork**, so an album whose most recent listen arrived from an import with
/// no cover keeps the one an earlier listen carried — the same semantics
/// `album_artwork` gets from its `WHERE`, which is why the chart and the album
/// page now agree. And the separator is `CHAR(31)`, the ASCII unit separator,
/// which cannot occur in a URL or an MBID.
///
/// Twelve queries share it. That is the point: a per-query fix would have left
/// the next grouped listing free to get it wrong again.
pub const COVER_PICK: &str = "MAX(CASE WHEN (artwork_url IS NOT NULL AND artwork_url != '') \
        OR caa_id > 0 \
     THEN printf('%012d', timestamp) || CHAR(31) || COALESCE(artwork_url, '') || CHAR(31) \
          || COALESCE(caa_id, 0) || CHAR(31) || COALESCE(caa_release_mbid, '') END)";

/// Gives a row type the `resolve_cover` step every grouped listing owes its
/// caller. A macro rather than a trait because there is nothing polymorphic
/// here — three identical structs, and the point is that none of them can be
/// served without unpacking.
macro_rules! resolves_cover {
    ($t:ty) => {
        impl $t {
            fn resolve_cover(&mut self) {
                let c = Cover::parse(self.cover_pick.take().as_deref());
                self.artwork_url = c.artwork_url;
                self.caa_id = c.caa_id;
                self.caa_release_mbid = c.caa_release_mbid;
            }
        }
    };
}
resolves_cover!(ChartRow);
resolves_cover!(RediscoveryRow);
resolves_cover!(AlbumAggregate);

/// The three artwork columns of one listen, unpacked from [`COVER_PICK`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cover {
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

impl Cover {
    /// Unpack a `cover_pick`. Anything unreadable is "no cover", which renders
    /// the lettered tile — the one outcome that is never *wrong*, only plain.
    pub fn parse(packed: Option<&str>) -> Self {
        let Some(packed) = packed else { return Self::default() };
        let mut parts = packed.split('\u{1f}');
        // Field 0 is the timestamp the pick was made on; it has done its job.
        let _ = parts.next();
        let url = parts.next().unwrap_or_default();
        let caa_id: i64 = parts.next().unwrap_or_default().parse().unwrap_or(0);
        let mbid = parts.next().unwrap_or_default();
        Self {
            artwork_url: (!url.is_empty()).then(|| url.to_string()),
            // `-1` is the "asked, nobody has one" tombstone and 0 is the
            // packed absence, so anything non-positive is no artwork — the
            // rule `coverArtUrl` applies on the other side.
            caa_id: (caa_id > 0).then_some(caa_id),
            caa_release_mbid: (!mbid.is_empty()).then(|| mbid.to_string()),
        }
    }
}

/// Everyone credited on a listen, primary first, de-duplicated by key.
///
/// The primary is always `play.artist` — the name the rest of the row is
/// written with — so a query that fans out over the credits and one that reads
/// `scrobbles.artist` cannot disagree about who the record is by. Anything else
/// in `play.artists` follows.
///
/// MBIDs are positional in `play.mbid_artist`, which is how MusicBrainz and
/// ListenBrainz both send them, so they are zipped by index and simply run out
/// when there are fewer of them than names.
pub fn credited_artists(play: &Play) -> Vec<(String, Option<String>)> {
    let mut names: Vec<String> = Vec::new();
    let mut push = |n: &str| {
        let n = n.trim();
        // **The placeholder is not a credit.** "Various Artists" is a filing
        // convention meaning *look at the track* — unresolved metadata, not a
        // performer — so it never becomes a row in `scrobble_artists`, and the
        // readouts that group over that table (Top Charts, the ranged stats,
        // `artist_rank`) are clean at the source rather than each remembering
        // to filter.
        //
        // The consequence is deliberate: a compilation listen carrying no real
        // credit belongs to **no artist at all**. That is the honest answer —
        // nobody recorded who played it — and it costs nothing, because every
        // count is made of the listen's own row rather than of its credits.
        if n.is_empty() || is_various_artists(n) {
            return;
        }
        let key = Database::artist_key(n);
        if !names.iter().any(|existing| Database::artist_key(existing) == key) {
            names.push(n.to_string());
        }
    };

    push(&play.artist);
    for extra in play.artists.iter().flatten() {
        push(extra);
    }

    let mbids = play.mbid_artist.as_deref().unwrap_or(&[]);
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| (name, mbids.get(i).filter(|m| !m.trim().is_empty()).cloned()))
        .collect()
}

/// The population every fidelity readout divides by: listens whose format is
/// actually known.
///
/// `is_lossless` is nullable, and most of a real history has nothing in it —
/// measured on a 21,390-listen export, **97.4% of rows carry no format at all**,
/// because an import from Last.fm or ListenBrainz has none to carry. Dividing a
/// lossless count by *every* play therefore scores "we never knew" as "not
/// lossless", and the card reports the user's import ratio rather than their
/// listening: someone who has only ever played FLAC reads 2%.
///
/// This is the [paired counts must share a population] rule again, after
/// `artwork_coverage` and `language_coverage`. The fidelity mix on the
/// Statistics page already had it right — it calls the unlabelled rows
/// `Unknown` instead of dropping them — so this constant is that bucket's
/// complement, and the mix is written in terms of it so the two cannot drift.
/// `a_fidelity_share_ignores_listens_of_unknown_format` pins them together.
///
/// Note a windowed share (last 7 days, say) does **not** fix this: an imported
/// listen from yesterday still has no format, and a Plex listen from 2019 does.
/// The period was never the problem.
///
/// [paired counts must share a population]: the note in CLAUDE.md under "Skips
/// leaking into a count is now a three-time bug".
///
/// **Every comparison here is NULL-safe, and it has to be.** `format_type =
/// 'dsd'` against a NULL column is NULL, not false — so in a `WHERE` it behaves
/// like false and counts correctly, but under the `NOT` the fidelity mix uses
/// it stays NULL, the branch never fires, and every unlabelled listen lands in
/// some *other* bucket. The two readouts would then disagree while both looked
/// reasonable. `COALESCE` is what makes the predicate mean the same thing in
/// both positions.
pub const FIDELITY_KNOWN: &str = "(COALESCE(format_type, '') = 'dsd' \
     OR is_lossless IS NOT NULL \
     OR COALESCE(codec, '') != '')";

impl Database {
    pub async fn new(database_url: &str, secrets: Arc<SecretBox>) -> Result<Self, sqlx::Error> {
        // Refuse a URL this build cannot serve, in words rather than as a
        // driver error twenty frames down. Getting handed
        // "unsupported URL scheme" when the real answer is "you want the other
        // build" is the kind of message that costs an evening.
        if !dialect::ACTIVE.accepts(database_url) {
            return Err(sqlx::Error::Configuration(
                format!(
                    "this build serves {}, but DATABASE_URL names something else. \
                     Accepted schemes: {}. The other backend is a separate build \
                     of the same binary — see `cargo build --features postgres`.",
                    dialect::ACTIVE.name(),
                    dialect::ACTIVE.schemes().join(", "),
                )
                .into(),
            ));
        }
        let pool = Self::connect(database_url).await?;
        let db = Self { pool, secrets };
        // The current shape, then walking an existing database up to it, then
        // the indexes over whatever those two produced. `indexes()` is last
        // because it names columns `migrate()` may only just have added — see
        // its own docs, and v0.61.1.
        db.init().await?;
        db.migrate().await?;
        db.indexes().await?;
        db.encrypt_legacy_credentials().await?;
        db.plan_stats().await;
        Ok(db)
    }

    /// Open the pool. Every knob here is per-backend and none of it transfers:
    /// WAL, `busy_timeout` and the page-cache pragmas are answers to SQLite's
    /// single-writer model and to running off an SD card.
    #[cfg(not(feature = "postgres"))]
    async fn connect(database_url: &str) -> Result<Pool<Db>, sqlx::Error> {
        // WAL + a busy_timeout let concurrent writers (poll engine, HTTP
        // handlers, background imports) wait for the lock instead of erroring
        // out with SQLITE_BUSY — which would otherwise drop listens on ingest.
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10))
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            // Where a GROUP BY that cannot use an index puts its temp B-tree.
            //
            // The default is a file, which on the target hardware — a Pi with
            // its database on an SD card — is the difference between a sort
            // that happens in RAM and one that happens on the slowest storage
            // in the machine. Several of the analytics screens group by
            // `LOWER(TRIM(artist))`, which no index can serve directly, so this
            // is not a rare path.
            .pragma("temp_store", "MEMORY")
            // ~8 MB of page cache (negative = KiB). The default 2 MB makes a
            // repeated full scan of a 50k-listen history re-read from disk
            // every time; this is small enough for a 1 GB host and large enough
            // to hold the working set of the analytics queries.
            .pragma("cache_size", "-8000");
        SqlitePoolOptions::new().max_connections(5).connect_with(options).await
    }

    #[cfg(feature = "postgres")]
    async fn connect(database_url: &str) -> Result<Pool<Db>, sqlx::Error> {
        // Deliberately the same five connections as the SQLite build, which is
        // almost certainly too few for a server that has no single-writer
        // constraint. It is left alone because this codebase's two worst
        // incidents were pool exhaustion behind a slow query, and the honest
        // way to raise it is a measurement on the real host rather than a
        // number picked here.
        sqlx::postgres::PgPoolOptions::new().max_connections(5).connect(database_url).await
    }

    /// Give the query planner something to plan with.
    ///
    /// **This is not a nicety — without it the indexes are a net loss.** Added
    /// after the expression indexes shipped and made three endpoints *slower*
    /// on the real host: Statistics 3.9 → 6.4s, Reports 1.76 → 2.3s, dashboard
    /// 0.85 → 1.2s, while Rediscovery went 122s → 2.7s.
    ///
    /// The cause is visible in the plan and is hardware-independent. With no
    /// statistics SQLite guesses at selectivity, and it guessed that the new
    /// narrow `(user_id, LOWER(TRIM(artist)))` index served *everything* — so
    /// `report_rows`, which only wants every row in timestamp order, walked the
    /// **artist** index instead, paid a rowid lookup per row for the six columns
    /// it needed, and then sorted the result by timestamp. Two costs added where
    /// `idx_scrobbles_user_time` had been giving it the right order for free.
    /// On an SD card, 49k random reads is the whole regression.
    ///
    /// After `ANALYZE` the planner picks `idx_scrobbles_user_time` for that scan
    /// with no sort at all, and still picks the expression index for the
    /// groupings that genuinely want it.
    ///
    /// `PRAGMA optimize` is the maintained form — SQLite's own recommendation,
    /// it re-analyses only what has drifted — but it does nothing on a database
    /// that has never been analysed, so the first run is explicit. Measured at
    /// 64ms for `ANALYZE` on a 49k-listen history and 7ms for `optimize`
    /// thereafter; neither is worth deferring or making conditional on a flag.
    ///
    /// Failure is logged and ignored: stale statistics make queries slower,
    /// never wrong, and that is not a reason to refuse to start.
    ///
    /// **SQLite only.** `ANALYZE` and `PRAGMA optimize` are its vocabulary, and
    /// `sqlite_stat1` is its table. Postgres keeps its own statistics through
    /// autovacuum and needs nothing here — a Postgres build that ran this would
    /// log three failures on every start and change nothing.
    async fn plan_stats(&self) {
        #[cfg(feature = "postgres")]
        return;
        #[cfg(not(feature = "postgres"))]
        {
            let analysed: Option<(i64,)> = sqlx::query_as(&crate::sql!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'sqlite_stat1'",
            ))
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();

            if analysed.map(|(n,)| n) == Some(0) {
                info!("⚙️ Analysing the database so the query planner has statistics...");
                if let Err(e) = sqlx::query(&crate::sql!("ANALYZE")).execute(&self.pool).await {
                    warn!("Could not analyse the database: {}", e);
                    return;
                }
            }
            if let Err(e) = sqlx::query(&crate::sql!("PRAGMA optimize")).execute(&self.pool).await {
                warn!("Could not run PRAGMA optimize: {}", e);
            }
        }
    }

    /// How long an automatic resolution is trusted.
    ///
    /// Long, because a catalogue's id for a track it already has does not
    /// change often — but not forever, because a track can be pulled and a
    /// cached id would then be added to every future playlist and rejected by
    /// the far end with nothing saying why.
    pub const MATCH_TTL_SECS: i64 = 60 * 60 * 24 * 60;

    // ════════════════════════════════════════════════════════════
    //  Password Hashing (Argon2)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Auth: Setup & Login
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Web Sessions (browser cookies)
    // ════════════════════════════════════════════════════════════

    /// Validate a session token from a cookie. Returns AuthUser if valid.
    /// How stale an activity stamp may get before it is worth a write.
    ///
    /// **Every authenticated request used to issue one**, which on a page whose
    /// deck polls every three seconds is twenty writes a minute per open tab —
    /// each taking SQLite's write lock, and each turning a read request into a
    /// write one. Thirty-three of them queued behind the stuck query in the
    /// v0.70.1 incident, where they were read as a symptom; they were also an
    /// amplifier, since each holds a pool connection while it waits.
    ///
    /// Five minutes is far finer than anything that reads these needs: nothing
    /// reads `web_sessions.last_active_at` at all, and `tokens.last_used_at`
    /// is shown in Settings as a relative time.
    const ACTIVITY_STAMP_INTERVAL_SECS: i64 = 300;

    // ════════════════════════════════════════════════════════════
    //  Scrobble CRUD
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Scrobble Reads
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Album & artist pages (UX 4)
    //
    //  All of these aggregate over `scrobbles` matched by *name*, not MBID.
    //  Most of a library is un-enriched and those are exactly the records worth
    //  a page, so an MBID join would leave the interesting ones blank. Every
    //  count filters `skipped = FALSE` like the rest of the counting queries.
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Metadata sanitiser
    //
    //  Scrobble clients tag per *track*, not per album, so one listen out of
    //  twelve routinely arrives carrying a compilation's release MBID. Nothing
    //  is wrong with that listen on its own — but the album page groups by name
    //  and the artwork comes from the release, so a single outlier shows the
    //  wrong cover and skews the record's stats.
    //
    //  The fix is a majority vote *within one album*, never across the library,
    //  and it is always offered as a preview before it is applied. Metadata that
    //  arrived from a real MusicBrainz mapping is still data; overwriting it
    //  silently would be worse than the problem.
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Physical media — records and tapes
    // ════════════════════════════════════════════════════════════

    // ── What is on the deck ──

    // ── Scans: sleeves, booklets, labels ──

    // ── Server-wide settings ──

    // ── Language enrichment ──

    // ════════════════════════════════════════════════════════════
    //  Scrobble Edit / Delete / Import / Export
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Ranged analytics  (GET /api/v1/stats?range=...)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  User preferences  (stored in users.settings JSON — no new table)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Signal Chains
    // ════════════════════════════════════════════════════════════

    /// Every column of a chain, with `total_hours` **derived from the listens
    /// that carry it** rather than read from the column of the same name.
    ///
    /// `signal_chains.total_hours` was an accumulator, and it was wrong in three
    /// ways that all look the same from the outside — a chain that has plainly
    /// been used reading zero:
    ///
    /// * **Only `ingest.rs` ever added to it.** A listen polled from Plex,
    ///   Jellyfin or Navidrome goes through `pipeline.rs`, a side off the shelf
    ///   through `physical.rs`, and an import through `import.rs` — none of
    ///   which touched it. So for anyone whose listens come from a media server
    ///   rather than a scrobble client, chain hours never moved at all.
    /// * **Assigning a chain was retroactive to nothing.** Binding an output
    ///   device, pointing a token at a chain, or fixing a listen's chain by hand
    ///   all change which listens belong to which chain — and an accumulator
    ///   only ever counted what arrived *after* the change. This is what "I
    ///   assigned the chain and the hours didn't move" is.
    /// * **`equipment.total_hours` is derived from this**, so gear inherited
    ///   every one of those gaps. That column was fixed the same way in v0.20;
    ///   the level below it was left an accumulator, which is why the fix only
    ///   half worked.
    ///
    /// So the number now comes from the same place every other count in this
    /// app comes from: the listens. `skipped = FALSE` for the reason it is
    /// everywhere else — crediting the gear with a track's full running time for
    /// three seconds of audio is exactly the error that filter exists to stop.
    ///
    /// A listen with no `duration` contributes nothing, which is honest and
    /// matches what the accumulator did (it only ever added when the client sent
    /// one). The `CAST` is load-bearing for the same reason it is in
    /// `get_equipment`: SQLite types `SUM()` by what it actually summed, so a
    /// chain with no listens comes back INTEGER and sqlx refuses to decode it
    /// into `f64`.
    const CHAIN_COLUMNS: &'static str = "s.id, s.user_id, s.name, s.description, s.components,
                s.listening_context, s.icon, s.is_active, s.shared,
                CAST(COALESCE((
                    SELECT SUM(sc.duration) FROM scrobbles sc
                     WHERE sc.user_id = s.user_id AND sc.chain_id = s.id
                       AND sc.skipped = FALSE
                ), 0) AS REAL) / 3600.0 AS total_hours,
                s.created_at";

    // ════════════════════════════════════════════════════════════
    //  Output-device → chain bindings (Phase B)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Import history (persisted; the live job registry is in-memory)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Per-user scrobble connections
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Artists
    // ════════════════════════════════════════════════════════════

    /// Images for a set of artist names, keyed by the same normalised form the
    /// table uses. Returned as a map so a stats page can decorate its rows with
    /// one query rather than one per artist.
    // ════════════════════════════════════════════════════════════
    //  Rediscovery
    // ════════════════════════════════════════════════════════════
    //
    // Every list here answers "something you already chose, that you are not
    // choosing now". None of them recommend music you don't own — that is a
    // different product, and it needs a corpus this app deliberately doesn't
    // have. What it does have is your own history, which is enough to notice
    // what has slipped.
    //
    // All of them exclude skips, like every other counting query.

    // ════════════════════════════════════════════════════════════
    //  Entities, liner notes and loves (Phase 7)
    // ════════════════════════════════════════════════════════════

    /// The three things a note or a love can point at. Callers validate against
    /// this before writing — the column is a plain TEXT.
    pub const ENTITY_KINDS: [&'static str; 3] = ["recording", "release", "artist"];

    /// Title and artist of a recording this user has actually listened to,
    /// found by MusicBrainz recording id.
    ///
    /// A ListenBrainz `feedback.jsonl` row carries an MBID and a score but no
    /// names, so this is how a loved track becomes something nameable.
    // ════════════════════════════════════════════════════════════
    //  Sessions
    //
    //  `assign_session` has grouped listens since Phase 2, but nothing ever read
    //  the result. Making them visible also makes them **editable**, and that
    //  settles a question left open in the design notes: the grouping cannot be
    //  computed at read time, because a merge or a split is a decision no
    //  heuristic can re-derive. So `scrobbles.session_id` stays the source of
    //  truth and these operations rewrite it.
    //
    //  Every mutation recomputes the aggregates from the listens rather than
    //  adjusting them arithmetically — a session that disagrees with its own
    //  contents is the failure mode worth designing out.
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Per-user sources
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Plex source settings (legacy server-wide row, migrated away)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Devices
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Equipment
    // ════════════════════════════════════════════════════════════

    /// Gear with its hours **derived from the chains that use it**.
    ///
    /// The stored `equipment.total_hours` was only ever written at creation, so
    /// it sat at zero forever while the chains above it accumulated — a chain
    /// reading "1.7h logged" over gear reading "0min" was the visible symptom.
    /// Now that a chain component carries `equipment_id`, the number is
    /// derivable, and deriving it means it can't drift from the chain's own
    /// total. Gear that predates the link (free-text steps) still reports zero,
    /// which is honest — nothing records what it was attached to.
    ///
    /// Two things about the shape of this, both found later:
    ///
    /// * It sums the **derived** chain hours, not `signal_chains.total_hours` —
    ///   that column is a vestigial accumulator that undercounted every listen
    ///   not arriving through the ingest API. See `CHAIN_COLUMNS`. Deriving here
    ///   too is what stops the two levels disagreeing.
    /// * `EXISTS` rather than a join against `json_each`. A chain that names the
    ///   same piece of gear at two steps — a preamp used twice, a cable listed
    ///   either side — matched twice and had its whole running time counted
    ///   twice. The question is "does this chain use this gear", which is
    ///   existence, not a row per mention.
    ///
    /// The `CAST` is load-bearing: SQLite types `SUM()` by what it actually
    /// summed, so gear on no chain at all comes back INTEGER through the
    /// `COALESCE` and sqlx refuses to decode it into `f64`. It shows up the
    /// moment a user has gear with no hours on it, which is every new install.
    // ── Chain variants ──

    // ════════════════════════════════════════════════════════════
    //  Listening Sessions
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  User & Token Management
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Device-code pairing
    // ════════════════════════════════════════════════════════════

    /// How long a pairing request lives. Short on purpose: the user code is
    /// only eight characters, so its safety comes from the window being small,
    /// the code being single-use, and polling being rate limited — not from
    /// entropy.
    pub const PAIRING_TTL_SECS: i64 = 300;

    // ════════════════════════════════════════════════════════
    //  MCP connections
    // ════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════
    //  OAuth 2.1
    // ════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════
    //  The Crate
    // ════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════
    //  Saved playlists
    // ════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Patch — the actor layer (Phase 6, Step 1)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Patch — patches and the Reel (Phase 6, Step 2)
    // ════════════════════════════════════════════════════════════

    /// The other end of a patch, as both directions need it. One list so the
    /// two cannot drift into describing a person differently.
    const PATCH_PEER_COLUMNS: &'static str = "u.id AS user_id, u.username, u.display_name, \
         u.bio, u.avatar_url, u.share_all_with_patched AS shares_everything, \
         (SELECT COUNT(*) FROM signal_chains c \
           WHERE c.user_id = u.id AND c.shared = 1) AS shared_chains";

    // ════════════════════════════════════════════════════════════
    //  Patch — the screens, Dubs and Spools (Phase 6, Step 3)
    // ════════════════════════════════════════════════════════════

    /// Every listen this user is entitled to see from other decks.
    ///
    /// The Reel's `FROM` and `WHERE`, factored out because the Artists tab, the
    /// Instance figures and a deck page all have to narrow the *same* way. Four
    /// clauses, and none may be relaxed: an accepted patch, a shared chain, a
    /// listen that **has** a chain, and no skips. Written once so a second
    /// reader cannot quietly publish more than the Reel does.
    ///
    /// `?` binds the reader's own user id.
    /// What a deck you are patched with may **see**.
    ///
    /// **The sharing unit moved from the chain to the patch** (Ashwin,
    /// 2026-08-26), and this is where that lives. It used to `JOIN
    /// signal_chains … AND c.shared = 1`, so a listen was published by the
    /// *equipment* it ran through — which meant a deck you had deliberately
    /// let in still saw nothing of anything played on gear you had not
    /// switched on, including everything with no chain at all (Navidrome, an
    /// unattributed submission). A signal chain is a gear configuration; using
    /// it as an access-control list was always an odd proxy for consent, and a
    /// patch is a decision about a *person*, which is what consent is.
    ///
    /// So a listen is visible when **either**:
    ///
    /// * its owner has `share_all_with_patched` on — the switch is off by
    ///   default and off for every account that predates it, because a patch
    ///   accepted under the old rule was accepted on the understanding that an
    ///   unshared chain published nothing; or
    /// * it ran through a chain its owner shares, which is the old rule intact.
    ///
    /// **`LEFT JOIN`, and that is deliberate here** — the opposite of the
    /// warning on `reel`, which said an inner join was what stopped every
    /// unattributed listen being published. That warning was right *while the
    /// chain was the gate*. It is not the gate any more, so the join is there
    /// to name the chain rather than to decide, and the deciding is in the
    /// `WHERE`. `c.shared = 1` over a missing row is NULL, which is false — so
    /// a chainless listen is still excluded unless the switch is on.
    ///
    /// Three narrowings are **unchanged and must stay**: the patch is
    /// `accepted`, skips never appear, and this is not what the Instance tab
    /// counts — that stays chain-gated, because a patch is consent to one
    /// person and there is no patch behind a figure the whole instance reads.
    pub(crate) const PATCH_VISIBLE_LISTENS: &'static str = "FROM scrobbles s \
         JOIN users u ON u.id = s.user_id \
         LEFT JOIN signal_chains c ON c.id = s.chain_id \
         JOIN patches p ON p.target_user_id = s.user_id \
        WHERE p.follower_user_id = ? AND p.state = 'accepted' AND s.skipped = FALSE \
          AND (u.share_all_with_patched = 1 OR c.shared = 1)";

    /// How a chain is named to somebody who may see the listen.
    ///
    /// Shared chains are named; a chain its owner keeps private is present but
    /// unnamed, and the screen says *"Private signal chain"* rather than
    /// leaving a hole. A listen with no chain at all says nothing — those are
    /// different facts and the row draws them differently.
    pub(crate) const CHAIN_VISIBILITY: &'static str =
        "CASE WHEN c.shared = 1 THEN c.name END AS chain_name, \
         CASE WHEN s.chain_id IS NULL THEN 0 ELSE 1 END AS has_chain";

    // ── Dubs: a record passed hand to hand ──

    // ── Shared Spools ──

    // ── Handing over a past sitting ──
}

// ════════════════════════════════════════════════════════════
//  Row types
// ════════════════════════════════════════════════════════════

/// A local user's outward-facing identity, with the private key unsealed.
///
/// **Deliberately not `Serialize`.** Every other row type in this file derives
/// it, and this one must not: `private_key` is the whole of the identity, and a
/// struct that can be handed to `Json(...)` is one refactor away from being
/// handed to `Json(...)`. The handlers build their responses from
/// `patch::actor::document`, which takes only the public half.
#[derive(Debug, Clone)]
pub struct Actor {
    pub user_id: i64,
    pub username: String,
    /// Written once at creation. Never re-derived — see `actor_for_user`.
    pub uri: String,
    /// SPKI PEM, as published in the actor document.
    pub public_key: String,
    /// PKCS#8 PEM, plaintext. Sealed in the column it came from.
    pub private_key: String,
    /// Non-null once the username is load-bearing for another server.
    pub frozen_at: Option<i64>,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub federation_enabled: bool,
}

/// The stored shape, before unsealing. Private to this file so no caller can
/// get hold of a ciphertext and treat it as a key.
#[derive(Debug, FromRow)]
struct ActorRow {
    user_id: i64,
    username: String,
    uri: String,
    public_key: String,
    private_key: String,
    frozen_at: Option<i64>,
    display_name: Option<String>,
    bio: Option<String>,
    avatar_url: Option<String>,
    federation_enabled: i64,
}

/// The other end of a patch — or a deck that could become one.
///
/// One type for three lists (out, in, and everyone patchable) so the three
/// cannot drift into describing a person differently. `uuid` and `state` are
/// null for a deck nobody has asked about yet, which is what tells "not
/// patched" apart from "asked and waiting".
#[derive(Debug, FromRow, serde::Serialize)]
pub struct PatchPeer {
    /// The patch's own id, `None` when no edge exists yet.
    pub uuid: Option<String>,
    /// pending | accepted | rejected, or `None` for no edge.
    pub state: Option<String>,
    pub created_at: Option<i64>,
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    /// How many chains this person publishes. **Zero is no longer the
    /// interesting case on its own** — read it with `shares_everything`, which
    /// is the gate. A deck sharing no chains and open to patched decks shows
    /// everything it plays; one sharing no chains and *closed* is the empty
    /// patch that needs saying on screen rather than looking broken.
    pub shared_chains: i64,
    /// Whether they let decks they have patched in see **everything** they
    /// play, not only listens through a chain they publish.
    ///
    /// Carried so the screen can say what a patch will actually show, which it
    /// could not do from `shared_chains` alone — and it leaks nothing: this
    /// only ever describes the far end of an *edge that exists*, and whoever
    /// can read it can already read the listens it is about.
    pub shares_everything: bool,
}

/// A listen on somebody else's deck, reaching this user through a patch.
///
/// Deliberately **not** `ScrobbleRecord`. That type is the shape of the
/// caller's own history and carries things nobody else is entitled to — the
/// source id, the delivery state, the chain id, the session. This one carries
/// what a Reel row draws and nothing more, so a field cannot leak by being
/// added to a struct somebody else also serialises.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct ReelListen {
    pub id: i64,
    pub timestamp: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
    pub format_type: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i64>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub is_lossless: Option<bool>,
    pub submission_client: Option<String>,
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// This deck's actor URI, or null when they have not switched federation
    /// on.
    ///
    /// The **handle is derived from this**, never from today's origin — the bug
    /// v0.78.0 found on the settings page, where a handle built from the
    /// current host sat beside a stored actor that disagreed with it, and the
    /// larger of the two was the one that did not work. A deck with no actor
    /// has no published address, and the row shows none rather than inventing
    /// one that resolves nowhere.
    pub actor_uri: Option<String>,
    /// The chain, **when its owner shares it**.
    ///
    /// `None` has two meanings and the row must not conflate them, which is
    /// what `has_chain` is for: a chain kept private reads *"Private signal
    /// chain"*, and a listen that ran through no chain at all says nothing.
    /// Before 2026-08-26 this was a non-`Option` `String`, because the chain
    /// *was* the gate and a row could not exist without one.
    pub chain_name: Option<String>,
    pub chain_icon: Option<String>,
    pub chain_components: Option<String>,
    /// Whether there was a chain, whatever its owner shares.
    pub has_chain: bool,
}

/// An artist seen across the decks a user is patched into.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct PatchArtist {
    pub name: String,
    pub plays: i64,
    /// How many decks play them.
    pub decks: i64,
    /// Comma-joined usernames, for the little avatar stack on the card.
    pub who: Option<String>,
    pub last_played: Option<i64>,
    /// When this artist first appeared in the shared history — what "newest
    /// arrival" sorts on.
    pub first_played: Option<i64>,
    /// **The reader's own plays of this artist — the overlap.** Shown as
    /// *which artists* rather than as a similarity score: a score invites a
    /// ranking of people, which is not what this is for.
    pub mine: i64,
    /// The artist's own tags, from `artists.genres` — the same cache the genre
    /// map reads, so this costs no lookup. Absent for anyone MusicBrainz has
    /// not tagged, which is a normal state and draws nothing rather than a
    /// placeholder.
    pub genre: Option<String>,
}

/// One chain a deck publishes.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct DeckChain {
    pub name: String,
    pub icon: Option<String>,
    pub components: String,
    pub hours: f64,
}

/// Somebody else's deck, as the person patched into it may see it.
#[derive(Debug, serde::Serialize)]
pub struct DeckView {
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: i64,
    /// Listens a patch into this deck can see — everything they play when they
    /// have opened up, and otherwise only what ran through a chain they
    /// publish. Not, either way, their whole history: skips never appear.
    pub listens: i64,
    pub chains: Vec<DeckChain>,
    /// Overlap only — no score.
    pub both_play: Vec<String>,
    /// Their actor URI, when they have an identity at all. `None` is a normal
    /// state, because federation is opt-in, and the page says so rather than
    /// printing endpoints that would 404.
    pub actor: Option<String>,
}

/// A deck on the instance, with what the viewer has in common with it.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct DiscoverDeck {
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: i64,
    pub shared_chains: i64,
    /// `pending` | `accepted` | `rejected`, or `None` if never asked.
    pub state: Option<String>,
    /// Artists you both play, unit-separated. **Computed against their shared
    /// chains only**, so this cannot become a way to learn what somebody plays
    /// privately by watching the list change.
    pub overlap: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ChainTally {
    pub name: String,
    pub plays: i64,
}

/// One listen's contribution to a day's hours. Sent as raw timestamps rather
/// than pre-bucketed, because the day boundary belongs to the *reader's*
/// timezone and the server does not know it here.
#[derive(Debug, serde::Serialize)]
pub struct DailyHour {
    pub at: i64,
    pub hours: f64,
}

/// What the instance has been playing, over shared chains only.
#[derive(Debug, serde::Serialize)]
pub struct InstancePulse {
    pub decks: i64,
    /// How many of them publish at least one chain.
    pub publishing: i64,
    pub shared_chains: i64,
    pub listens: i64,
    pub hours: f64,
    /// Distinct artists heard across shared chains in the window. **Not
    /// `artists.len()`** — that list is cut to the top 8 for the panel, so
    /// counting it would report 8 forever.
    pub artists_heard: i64,
    pub artists: Vec<PatchArtist>,
    pub chains: Vec<ChainTally>,
    pub daily: Vec<DailyHour>,
}

/// The admin Operations panel. Only what is actually recorded.
#[derive(Debug, serde::Serialize)]
pub struct FederationOps {
    pub identities: i64,
    pub published: i64,
    pub frozen: i64,
    pub remote_actors: i64,
    pub inbox_activities: i64,
    pub deliveries: i64,
}

/// A record somebody handed you.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct Dub {
    pub id: i64,
    pub kind: String,
    pub artist: String,
    pub name: Option<String>,
    pub reason: Option<String>,
    pub because: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub from_username: String,
    pub from_display_name: Option<String>,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct SpoolMember {
    pub user_id: i64,
    pub username: String,
    pub display_name: Option<String>,
    /// invited | joined | left.
    pub state: String,
    /// When they were in the room from, or `None` for the whole sitting.
    ///
    /// Not `joined_at`, which is when they pressed the button — somebody added
    /// an hour in was usually there all along, and the host says which.
    pub present_from: Option<i64>,
    /// When they left, or `None` for stayed to the end.
    pub present_until: Option<i64>,
}

/// One sitting on the history list.
///
/// Deliberately not [`SpoolView`]: that type answers *what is happening now*
/// and carries a full roster and the caller's own membership state, where this
/// answers *what happened* and has to stay cheap across a page of them. The
/// same call [`SentSitting`] makes against [`HandedSitting`].
#[derive(Debug, serde::Serialize)]
pub struct SpoolSummary {
    pub uuid: String,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub hosting: bool,
    pub host_username: String,
    pub host_display_name: Option<String>,
    /// Everyone who was actually in the room, the caller included — the
    /// screen decides whether to leave the reader out, because only it knows
    /// whether it is writing "with Noor" or listing attendance.
    pub present: Vec<String>,
    /// Names with no account on this instance.
    pub guests: Vec<String>,
    /// Plays that reached this sitting's timeline, from every deck in it.
    pub plays: i64,
    /// Offers still sitting in the caller's inbox from this evening.
    pub pending: i64,
}

/// One play on a sitting's timeline, whoever's deck it came off.
///
/// **This is the first row type in the application that mixes several people's
/// listening into one sequence.** Everything else — the Reel included — keeps
/// one deck per row.
#[derive(Debug, serde::Serialize)]
pub struct SpoolPlay {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<i64>,
    pub played_at: i64,
    pub from_username: String,
    pub from_display_name: Option<String>,
    /// `mine` | `accepted` | `declined` | `pending` | `theirs`.
    ///
    /// Four of those are facts about the caller and the fifth is the absence of
    /// one. Declined and never-answered are told apart deliberately: both mean
    /// "not in my history" and they are not the same decision.
    pub standing: String,
    /// The capture this row came from, where there is one to act on.
    pub capture_id: Option<i64>,
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

/// A sitting, in full.
#[derive(Debug, serde::Serialize)]
pub struct SpoolDetail {
    pub uuid: String,
    pub viewer_id: i64,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub host_user_id: i64,
    pub hosting: bool,
    pub my_state: String,
    pub members: Vec<SpoolMember>,
    pub guests: Vec<String>,
    pub plays: Vec<SpoolPlay>,
    /// Plays that happened in this sitting and are not on the timeline, because
    /// they ran through chains their owner does not share.
    ///
    /// **Counted and stated rather than silently dropped.** A document with
    /// invisible holes misrepresents the evening and the reader could never
    /// tell — the rule `handed_sitting` already carries, in the one other place
    /// this application shows somebody an evening.
    pub withheld: i64,
}

/// A sitting several people are in at once.
#[derive(Debug, serde::Serialize)]
pub struct SpoolView {
    pub uuid: String,
    /// Who is asking. The members list includes the caller, and a screen saying
    /// "with Noor and Tobias" has to leave the reader out of it — `hosting`
    /// cannot do that, since it only identifies one of the two cases.
    pub viewer_id: i64,
    pub title: Option<String>,
    pub started_at: i64,
    /// Null while it is running. An **ended** sitting still reaches this view
    /// when the caller was invited and never answered — an invitation you can
    /// no longer see is one you can no longer honour, and forgetting to press
    /// Join before the host closed the room is the ordinary case rather than
    /// the odd one.
    pub ended_at: Option<i64>,
    pub host_user_id: i64,
    pub hosting: bool,
    /// The *caller's* membership: `invited` until they join.
    pub my_state: String,
    pub members: Vec<SpoolMember>,
    /// Names with no account here, so the banner can say who else is in the
    /// room rather than only who else has a deck.
    pub guests: Vec<String>,
    /// How many offers this sitting has put in the caller's inbox.
    pub captured: i64,
}

/// A play from somebody else's deck, waiting to be accepted.
///
/// **Not a listen.** It enters no history, no count and no chart until its
/// owner says yes — which is why it lives in its own table rather than in
/// `scrobbles` behind a flag.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct SpoolCapture {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<i64>,
    pub played_at: i64,
    pub state: String,
    pub from_username: String,
    pub from_display_name: Option<String>,
    pub spool_uuid: String,
    pub spool_title: Option<String>,
}

/// Somebody else's actor, as we last fetched it.
#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct RemoteActor {
    pub id: i64,
    pub uri: String,
    /// `ash@deck.example`. Display only — `uri` is the identity, because a
    /// handle can be reused after a `Move` where a URI cannot.
    pub handle: Option<String>,
    pub name: Option<String>,
    pub inbox: String,
    pub shared_inbox: Option<String>,
    pub public_key: Option<String>,
    pub icon_url: Option<String>,
    pub fetched_at: i64,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct UserRecord {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub created_at: i64,
    pub disabled: bool,
}

/// A user's own account, as they see it on the profile screen.
///
/// Deliberately not `UserRecord` with two fields bolted on: that one is the
/// admin's view of *another* account and is rendered in a list, while this is
/// the owner's view of their own and carries the parts only they can set.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct UserProfile {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub created_at: i64,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub disabled: bool,
}

/// One row of a chart. `sub` is the secondary line — an album's or track's
/// artist — and is null for an artist chart, where the name says it all.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct ChartRow {
    pub name: String,
    pub sub: Option<String>,
    /// Only set on a tracks chart — the record the track is on, so the row can
    /// open the album like every other track listing in the app.
    pub album: Option<String>,
    pub plays: i64,
    pub seconds: i64,
    /// Filled from `cover_pick` after the query, never selected directly —
    /// see [`COVER_PICK`] for why the three must come from one listen.
    #[sqlx(default)]
    pub artwork_url: Option<String>,
    #[sqlx(default)]
    pub caa_id: Option<i64>,
    #[sqlx(default)]
    pub caa_release_mbid: Option<String>,
    /// The packed pick, unpacked by `resolve_cover` before the row is served.
    #[serde(skip)]
    #[sqlx(default)]
    pub cover_pick: Option<String>,
}

/// A cached description. `bio IS NULL` means every provider was asked and none
/// had anything — a tombstone, not a miss to retry on the next page view.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct BioRow {
    pub entity_id: i64,
    pub bio: Option<String>,
    pub attribution: Option<String>,
    pub provider: Option<String>,
    pub url: Option<String>,
    pub fetched_at: i64,
}

/// One release an album's listens claim to be, and how many say so.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReleaseCandidate {
    pub mbid: String,
    pub plays: i64,
}

/// An album whose listens disagree about which release they belong to.
#[derive(Debug, serde::Serialize)]
pub struct MixedAlbum {
    pub artist: String,
    pub album: String,
    pub plays: i64,
    /// How many distinct release MBIDs the listens carry. Always > 1 here.
    pub variants: i64,
    /// The MBID the most listens agree on — the *proposed* fix, not a verdict.
    pub winner: Option<String>,
    pub winner_plays: i64,
    /// Listens of this album carrying no release MBID at all. Unifying stamps
    /// these with the chosen release too, so they're reported separately.
    pub untagged: i64,
    /// Every release the listens claim, most-claimed first, so the operator can
    /// pick a different one than the plurality.
    pub candidates: Vec<ReleaseCandidate>,
}

/// An artist string carrying a featured credit, and the primary artist it would
/// fold into.
#[derive(Debug, serde::Serialize)]
pub struct FeaturedArtist {
    pub artist: String,
    pub primary: String,
    pub plays: i64,
}

/// One track carrying two different artists on the same record.
#[derive(Debug, serde::Serialize)]
pub struct SplitAttribution {
    /// Lowercased grouping keys — the same normalisation the query grouped on.
    pub title: String,
    pub album: String,
    /// The record's own artist, and what everything else folds into.
    pub primary: String,
    /// This artist's plays *on this track*.
    pub primary_plays: i64,
    /// This artist's plays across the whole album.
    pub primary_album_plays: i64,
    /// How many distinct tracks of the album carry this artist. *This* is why
    /// they win, and it is shown so a weak case is visible as one: a director
    /// credited on twelve tracks is obvious, a winner with two is a guess.
    pub primary_album_tracks: i64,
    /// `(artist, plays)` for every other attribution on this track.
    pub folding: Vec<(String, i64)>,
}

/// An artist name with other spellings of itself in the same history.
#[derive(Debug, serde::Serialize)]
pub struct VariantArtist {
    /// The most-played spelling — the one the others fold into.
    pub primary: String,
    pub primary_plays: i64,
    /// `(spelling, plays)` for every other form of the same name.
    pub variants: Vec<(String, i64)>,
    /// These differ only in capitalisation or spacing.
    ///
    /// Worth separating because the consequences differ. A case-only variant
    /// costs nothing but display consistency — every read path that counts
    /// already lowercases. Anything else ("Cœur de pirate" against "Coeur de
    /// pirate") is genuinely two artists to every query, and splits the plays.
    pub case_only: bool,
}

/// A listening session: contiguous listening through one setup.
///
/// `chain_name` / `device_name` / `token_name` are joined so a list of sessions
/// renders without a request per row. `title` is a user label; when it's null
/// the UI derives one from the span.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct SessionRow {
    pub id: i64,
    /// Stable across merges and splits, and the id a shared session will carry.
    pub uuid: Option<String>,
    pub title: Option<String>,
    /// A note about the sitting itself. Distinct from a liner note, which is
    /// about a record and hangs off an entity or a single listen.
    pub notes: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub track_count: i64,
    pub total_duration: i64,
    pub avg_quality_score: Option<f64>,
    pub listening_context: Option<String>,
    pub chain_id: Option<i64>,
    pub chain_name: Option<String>,
    /// Which variant of that chain the sitting's listens carry. Read off the
    /// listens rather than stored on the session, so it cannot disagree with
    /// them — the same rule `recompute_session` follows for the aggregates.
    pub chain_variant_id: Option<i64>,
    pub chain_variant_name: Option<String>,
    pub device_id: Option<i64>,
    pub device_name: Option<String>,
    pub token_id: Option<i64>,
    pub token_name: Option<String>,
}

/// A stable, opaque id for a session.
///
/// Not a real UUID library — a hex random is enough for something that only has
/// to be unique across instances and never parsed. It exists so a session keeps
/// one identity through merges, splits and (later) being handed to someone
/// else's Tapedeck.
/// How recent a Shared Spool play must be for accepting it to relay it onward.
/// Fourteen days.
///
/// **The number is Last.fm's, not one picked to feel about right.** That is
/// their own limit on how old a scrobble may be, so nothing inside this window
/// is anomalous to the sink and nothing outside it would land properly anyway.
/// Having a real anchor matters here for the reason a grid-size-dependent
/// breadth metric was rejected elsewhere: a threshold somebody invented, sitting
/// in front of an irreversible write to a public record, is a confident-looking
/// way of deciding nothing.
///
/// It is much wider than [`crate::server::ingest::IMPORT_LIVE_WINDOW_SECS`],
/// deliberately. That one bounds the lag between a play and a client flushing
/// its queue, which is hours. This one bounds the lag between a play and a
/// **person opening their inbox and pressing Accept**, which is legitimately
/// days — the whole design is that a capture waits until its owner answers it.
pub const SPOOL_FORWARD_WINDOW_SECS: i64 = 14 * 24 * 60 * 60;

/// Whether an accepted capture is fresh enough to relay.
///
/// The switch and this gate cover each other's blind spot, exactly as the token
/// and freshness gates do in `ingest::import_is_live`. The **switch** is the
/// person saying "plays I accept out of a sitting are my listens"; nothing else
/// can say it, since a capture looks identical whether it came from tonight or
/// from a sitting reconstructed out of 2019. The **window** then bounds how
/// wrong that statement can be: `retro_spool_from_session` can offer an evening
/// of any age, and `offer_spool_backlog` can hand over a sitting whose
/// invitation went unanswered for a month, so without it one Accept could
/// rewrite a year of somebody's public history in a single click.
///
/// A future timestamp is clock skew rather than a backfill and is allowed, but
/// only to an hour — the same bound and the same reason as the ingest rule.
pub fn spool_forward_is_live(played_at: i64, now: i64) -> bool {
    (now - played_at) <= SPOOL_FORWARD_WINDOW_SECS && (played_at - now) <= 3600
}

/// Split a stored free-text name list into names.
///
/// The same comma-joined shape `scrobbles.company` uses, so a sitting's guests
/// and a listen's company are the one format rather than two — which matters
/// because the first is written into the second on every fan-out.
///
/// Empty entries are dropped rather than kept as blanks: a trailing comma is
/// what somebody typing a list produces, not a person with no name.
pub fn split_names(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| s.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Join names back into the stored form, or `None` where there are none.
///
/// `None` rather than an empty string, so "nobody said" and "nobody was there"
/// stay the same absence they are everywhere else in this schema.
pub fn join_names(names: &[String]) -> Option<String> {
    let cleaned: Vec<&str> = names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()).collect();
    (!cleaned.is_empty()).then(|| cleaned.join(", "))
}

fn new_uuid() -> String {
    use rand::Rng;
    let raw: [u8; 16] = rand::rng().random();
    hex::encode(raw)
}

/// Every spelling of the "no single artist" placeholder, already folded to the
/// `LOWER(TRIM(…))` form so it can be compared in Rust or bound into SQL.
///
/// A constant rather than a list inline in `is_various_artists`, because
/// `fuzzy_exists` has to ask the same question in SQL — and two copies of this
/// list would be free to disagree about which listens are duplicates.
pub const VARIOUS_ARTISTS_KEYS: &[&str] =
    &["various artists", "various artist", "various", "va", "verschiedene interpreten"];

/// The `WHERE` fragment that keeps the placeholder out of an artist-shaped
/// readout.
///
/// Appended to a column already folded to the artist key
/// (`sa.artist_key {NOT_VARIOUS}`) or to an expression folded the same way
/// (`LOWER(TRIM(s.artist)) {NOT_VARIOUS}`). Inlining the keys as literals is
/// safe because they are a compile-time constant with no quote in them, which
/// `the_sql_guard_lists_exactly_the_names_the_rust_one_knows` pins along with
/// the fact that it lists *all* of them.
///
/// **A guard on the rows, never on the totals.** Every one of these plays is
/// real listening and still counts in the play total, the hours, the heatmap
/// and its session. What it must not do is occupy a slot that means "an artist
/// you listen to". Excluding it from a *count of listens* would be the
/// paired-counts trap in reverse.
pub const NOT_VARIOUS: &str =
    "NOT IN ('various artists','various artist','various','va','verschiedene interpreten')";

/// The same test the other way round, for the one place that wants the
/// placeholder rows rather than everything else — the migration that clears
/// them. Spelled out rather than written as `NOT (… {NOT_VARIOUS})`, because a
/// double negative on a `DELETE` is the wrong place to be clever.
/// `the_sql_guard_lists_exactly_the_names_the_rust_one_knows` pins both.
pub const IS_VARIOUS: &str =
    "IN ('various artists','various artist','various','va','verschiedene interpreten')";

/// A "no single artist" placeholder, not an artist.
///
/// Shared with the Plex source, which must not record a compilation's tracks
/// under it. Here it keeps the sanitiser from folding real performers onto a
/// placeholder: "Various Artists" is on every track of a compilation, so it
/// wins the breadth test outright. `fuzzy_exists` reads it the other way round
/// — as the absence of a performer, which is why it cannot be trusted to tell
/// two listens apart.
pub fn is_various_artists(name: &str) -> bool {
    VARIOUS_ARTISTS_KEYS.contains(&name.trim().to_lowercase().as_str())
}

/// Collapse an artist name to its identity for duplicate detection.
///
/// **The one artist-identity function in the codebase.** There used to be two —
/// this and `sources::plex::artist_matches`, written for portrait lookup — and
/// they disagreed about whether "Cœur de pirate" and "Coeur de pirate" were the
/// same person. The portrait code said yes and the sanitiser said no, so a
/// history could hold both spellings as separate artists while sharing one
/// photograph. Two answers to one question is worse than either answer.
///
/// Three steps, each earning its place:
///
/// 1. **Ligature expansion.** `œ`, `æ` and `ß` have no Unicode decomposition —
///    they are letters, not accented forms — so they need naming explicitly.
///    This is the case that actually split 2,958 plays from 3 in a real
///    history.
/// 2. **NFKD, then drop combining marks.** Turns `ó` into `o`, `ü` into `u`.
///    Handles every accented Latin form without a table to maintain, and folds
///    the ligatures Unicode *does* decompose (`ﬁ`) for free.
/// 3. **Keep alphanumerics, lowercase.** So "A.R. Rahman", "A. R. Rahman" and
///    "AR Rahman" all reduce to `arrahman`.
///
/// Aggressive on purpose. It groups *candidates for review* and never rewrites
/// on its own, so a false pairing costs a line in a preview rather than
/// corrupted data — with one exception worth knowing: `artist_matches` uses it
/// to decide which artist a portrait belongs to, where a false pairing shows
/// the wrong face. Accent folding is safe there because an accent has never
/// distinguished two different artists; it is the same person, spelled by a
/// tagger who couldn't be bothered.
/// What an import did with one listen.
///
/// Three outcomes rather than a bool, because "already had it" and "already had
/// it and learned something from it" are different events and the job reports
/// them separately — `imported: 0` used to read as "nothing happened" when a
/// re-import had in fact filled in a great deal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportOutcome {
    /// A new listen.
    Inserted,
    /// Already held, and the incoming copy knew things this one did not.
    Enriched,
    /// Already held, with nothing to add.
    Duplicate,
}

/// The sparse columns an import can fill in, as they stand on the stored row.
#[derive(Debug, sqlx::FromRow)]
struct SparseColumns {
    album: Option<String>,
    duration: Option<i64>,
    mbid_recording: Option<String>,
    mbid_release: Option<String>,
    mbid_artist: Option<String>,
    caa_id: Option<i64>,
    caa_release_mbid: Option<String>,
    track_number: Option<i64>,
    spotify_track_id: Option<String>,
}

pub fn normalise_artist(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.chars()
        .flat_map(|c| match c {
            'œ' | 'Œ' => "oe".chars().collect::<Vec<_>>(),
            'æ' | 'Æ' => "ae".chars().collect(),
            'ß' => "ss".chars().collect(),
            // Not decomposable either — a stroke through the letter.
            'ø' | 'Ø' => "o".chars().collect(),
            'đ' | 'Đ' | 'ð' | 'Ð' => "d".chars().collect(),
            'ł' | 'Ł' => "l".chars().collect(),
            'þ' | 'Þ' => "th".chars().collect(),
            c => vec![c],
        })
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Are these two strings the same artist?
///
/// Both sides go through `normalise_artist`, so this and the sanitiser can
/// never disagree again.
pub fn artist_matches(a: &str, b: &str) -> bool {
    let (a, b) = (normalise_artist(a), normalise_artist(b));
    !a.is_empty() && a == b
}

/// Split `Artist feat. Someone` into its primary artist.
///
/// Only the explicit credit forms are matched. `&` and `,` are deliberately
/// left alone: "Simon & Garfunkel" and "Earth, Wind & Fire" are names, and
/// folding them would invent artists that never existed. Case-insensitive, and
/// the separator has to be surrounded by whitespace so a track by a band called
/// "Feature" survives.
pub fn split_featured(artist: &str) -> Option<String> {
    const MARKERS: [&str; 6] = [" feat. ", " feat ", " ft. ", " ft ", " featuring ", " with "];
    let lower = artist.to_lowercase();
    for m in MARKERS {
        if let Some(pos) = lower.find(m) {
            let primary = artist[..pos].trim();
            if !primary.is_empty() && primary.len() != artist.trim().len() {
                return Some(primary.to_string());
            }
        }
    }
    None
}

/// What an API token resolves to. The id is carried because a session is now
/// split when the submitting token changes — a different app means a different
/// setup far more often than not.
/// What a polling client is told about its pairing request.
#[derive(Debug, PartialEq, Eq)]
pub enum PairingPoll {
    /// Not approved yet — keep polling.
    Pending,
    /// Approved; here is the token, once.
    Approved(String),
    /// The window closed. Start again.
    Expired,
    /// No such request. Deliberately distinct from `Expired` only to the
    /// server — the handler reports both the same way, so polling cannot be
    /// used to learn whether a device code was ever real.
    Unknown,
}

/// One listen, as a report reads it.
#[derive(Debug, FromRow)]
pub struct ReportRow {
    pub timestamp: i64,
    pub artist: String,
    /// Everyone credited on the track, when that differs from the record's
    /// artist. On a compilation the `artist` column is the placeholder
    /// "Various Artists" and this is where the real performer is — which is
    /// what lets a report name someone rather than a filing convention.
    pub artist_credit: Option<String>,
    pub title: String,
    pub album: Option<String>,
    pub duration: Option<i64>,
    /// JSON array of tag names, written by the genre backfill. Absent until
    /// that listen has been enriched.
    pub genres: Option<String>,
    pub chain_id: Option<i64>,
    /// Who this was heard with, as free text — a comma-joined list of names.
    ///
    /// **`None` means unmarked, and it must never be read as "alone".** Only a
    /// Shared Spool and a hand edit ever set it, so a report that presented a
    /// bare share would be answering "how much of this was recorded as social"
    /// while appearing to answer "how much of this was social". Anything using
    /// it states what has been marked — see `Bucket::company_plays`.
    pub company: Option<String>,
}

/// What an API token resolves to. The id is carried because a session is now
/// split when the submitting token changes — a different app means a different
/// setup far more often than not.
#[derive(Debug)]
pub struct TokenAuth {
    pub user: AuthUser,
    pub token_id: i64,
    pub default_chain_id: Option<i64>,
    /// Space- or comma-separated grants from `tokens.scopes`. Written since
    /// tokens existed and, until now, never read — so every token was
    /// effectively `submit`-only by virtue of nothing else accepting one.
    pub scopes: String,
    /// Whether this client's `listen_type: "import"` means "a live listen,
    /// batched" rather than "a chunk of history". It is a property of the
    /// *client*, not of the listen — fooyin hardcodes `import` for every
    /// scrobble it has ever sent, and nothing in the payload distinguishes that
    /// from a backfill — so the operator states it once per token, and
    /// `IMPORT_LIVE_WINDOW_SECS` bounds how far they can be wrong.
    pub imports_are_live: bool,
}

impl TokenAuth {
    /// May this token read the caller's own data?
    ///
    /// Deliberately narrow: `read` is an explicit grant, not something `submit`
    /// implies. A scrobble client that only ever posts listens should not
    /// silently gain the ability to enumerate the user's gear because the
    /// scopes column happened to exist.
    pub fn can_read(&self) -> bool {
        self.has_scope("read")
    }

    /// May this token change the caller's own data?
    ///
    /// Same discipline as `can_read`, and for a sharper reason: `write` is the
    /// scope that lets a paired client edit a listen, love a track or rewrite a
    /// signal chain. `submit` deliberately does not imply it — posting listens
    /// is what every scrobble client on the internet does, and it must not
    /// quietly carry the power to delete the history it is appending to.
    ///
    /// `write` does **not** imply `read` either. They are separate grants
    /// because they are separately dangerous, and a client that needs both asks
    /// for both.
    pub fn can_write(&self) -> bool {
        self.has_scope("write")
    }

    fn has_scope(&self, want: &str) -> bool {
        self.scopes
            .split(|c: char| c == ',' || c.is_whitespace())
            .any(|s| s.trim() == want || s.trim() == "all")
    }

    /// Every grant this token actually carries, `all` expanded.
    ///
    /// Backs the introspection on `/1/validate-token`, so that a client can
    /// show its user what their token may do instead of finding out from a 403
    /// mid-use.
    ///
    /// Derived by asking `has_scope` rather than by parsing `scopes` a second
    /// time, and that is the whole point of it being here: a hand-written
    /// expansion would be a second answer to "what does this token grant",
    /// free to drift from the one the extractors enforce. Reporting `write` to
    /// a client that is then refused it is worse than reporting nothing.
    pub fn granted_scopes(&self) -> Vec<&'static str> {
        Self::SCOPES.iter().copied().filter(|s| self.has_scope(s)).collect()
    }

    /// The grants that exist. `all` is a wildcard over these, not one of them.
    const SCOPES: [&'static str; 3] = ["submit", "read", "write"];
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct TokenRecord {
    pub id: i64,
    pub name: String,
    pub scopes: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub default_chain_id: Option<i64>,
    /// Nullable in the column (it was added by migration), but a token that has
    /// never been told is not live — so it reads as a plain bool out here.
    #[sqlx(default)]
    pub imports_are_live: bool,
    /// Joined for display so the token list doesn't need a second round-trip.
    pub default_chain_name: Option<String>,
}

/// How big a history is and how much of it is actually populated.
///
/// Every `with_*` count is over the same population as `plays`, so they divide
/// into it directly. Nullable because a history with no listens at all has no
/// first or last one — `0` would be a date in 1970 and a model would say so.
#[derive(Debug, FromRow, serde::Serialize)]
pub struct ListeningCoverage {
    pub plays: i64,
    pub artists: i64,
    pub albums: i64,
    pub first_listen: Option<i64>,
    pub last_listen: Option<i64>,
    pub with_mbid: i64,
    pub with_isrc: i64,
    pub with_format: i64,
    pub with_duration: i64,
    pub with_chain: i64,
    pub with_quality: i64,
}

/// Something to put in the crate.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct NewRecommendation {
    /// `artist`, `album` or `track`.
    pub kind: String,
    pub artist: String,
    /// The album or track title. `None` for an artist.
    pub name: Option<String>,
    pub mbid: Option<String>,
    pub reason: Option<String>,
    /// What in the user's own listening prompted this.
    pub because: Option<String>,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct RecommendationRow {
    pub id: i64,
    pub kind: String,
    pub artist: String,
    pub name: Option<String>,
    pub mbid: Option<String>,
    pub reason: Option<String>,
    pub because: Option<String>,
    /// `user`, or the name of the connection that suggested it.
    pub origin: String,
    pub status: String,
    pub created_at: i64,
    pub acted_at: Option<i64>,
}

/// A crate entry with the bit only Tapedeck can supply: whether the
/// recommendation actually landed.
///
/// A recommendation is a *prediction*, and a scrobbler is the only thing in the
/// room that can score it. `first_played_at` is derived from the listens rather
/// than stored, so it cannot drift from them.
#[derive(Debug, serde::Serialize)]
pub struct CrateEntry {
    #[serde(flatten)]
    pub rec: RecommendationRow,
    /// When this first turned up in the history, if it ever did.
    pub first_played_at: Option<i64>,
    pub plays: i64,
}

/// A freshly minted MCP credential. Plaintext, returned once, stored hashed.
pub struct McpCredential {
    pub id: i64,
    pub token: String,
    /// Only ever set for an OAuth grant — see `create_mcp_connection`.
    pub refresh: Option<String>,
    pub expires_in: Option<i64>,
}

/// An authenticated MCP caller: which connection, whose data, what it may do.
///
/// The scope check lives on this type rather than in each tool for the reason
/// `TokenAuth::can_read` does — a grant that has to be remembered at 20 call
/// sites is a grant that will be forgotten at one of them.
#[derive(Debug, Clone)]
pub struct McpAuth {
    pub connection_id: i64,
    pub user: AuthUser,
    pub scopes: String,
}

impl McpAuth {
    /// Exact-token match, and **no wildcard**.
    ///
    /// `TokenAuth::has_scope` accepts `all`; this deliberately does not. An MCP
    /// consent screen names the things it is granting so the user can read
    /// them, and "everything, including whatever gets added later" is not
    /// something a person can meaningfully consent to. Splitting on both commas
    /// and whitespace mirrors the token parser, so an operator who writes the
    /// column by hand gets the same behaviour in both places.
    pub fn has_scope(&self, want: &str) -> bool {
        self.scopes.split(|c: char| c == ',' || c.is_whitespace()).any(|s| s.trim() == want)
    }
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct McpConnectionRecord {
    pub id: i64,
    pub name: String,
    pub scopes: String,
    pub client_id: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub calls: i64,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct McpAuditRow {
    pub id: i64,
    pub at: i64,
    pub method: String,
    pub tool: Option<String>,
    pub args: Option<String>,
    pub ok: bool,
    pub rows_returned: Option<i64>,
    pub ms: Option<i64>,
    /// Null once the connection has been deleted outright rather than revoked —
    /// which nothing does, but the join has to allow for it.
    pub connection_name: Option<String>,
}

#[derive(Debug, FromRow)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_name: String,
    /// A JSON array, kept as text — this is read once per authorization and
    /// parsed there rather than being a shape the database has to know.
    pub redirect_uris: String,
    pub registered_by_url: bool,
}

#[derive(Debug, FromRow)]
pub struct OAuthCode {
    pub client_id: String,
    pub user_id: i64,
    pub redirect_uri: String,
    pub scopes: String,
    pub code_challenge: String,
    pub resource: Option<String>,
    pub expires_at: i64,
}

#[derive(Debug, Clone, FromRow, serde::Serialize, serde::Deserialize)]
pub struct SavedPlaylistTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Why this track is here, in the words of whatever chose it. Never
    /// generated by Tapedeck.
    pub note: Option<String>,
}

/// A playlist track with what the listening history already knows about it.
///
/// Separate from `SavedPlaylistTrack` because that one is also the *input*
/// shape — an assistant sends title, artist and album — and an input struct
/// carrying derived fields invites somebody to try setting them.
#[derive(Debug, serde::Serialize)]
pub struct PlaylistTrackView {
    #[serde(flatten)]
    pub track: SavedPlaylistTrack,
    /// Plays in this user's history. Zero for a track they have never played,
    /// which is a normal outcome rather than a problem.
    pub plays: i64,
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct SavedPlaylistRecord {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub origin: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub pushed_at: Option<i64>,
    pub pushed_to: Option<String>,
    pub connection_name: Option<String>,
    pub tracks: i64,
}

/// A variant as stored, before its overrides are parsed.
#[derive(Debug, FromRow)]
pub struct ChainVariantRow {
    pub id: i64,
    pub user_id: i64,
    pub chain_id: i64,
    pub name: String,
    pub overrides: String,
    pub created_at: i64,
    pub total_hours: f64,
}

/// A chain with one or more steps swapped.
///
/// `total_hours` here is the listening that went through *this variant*, which
/// is a different question from a chain's total — a chain's hours include every
/// variant of it, since they all ran through the same chain.

/// What a fold moved, so the caller can say it rather than claim success.
///
/// Reported per table because they are different kinds of thing: listens
/// are the hours, a token or a device default is *future* attribution, and
/// a record on the shelf is the turntable it lives with.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct FoldOutcome {
    pub variant_id: i64,
    pub variant_name: String,
    pub variant_created: bool,
    pub listens: u64,
    pub sides: u64,
    pub sessions: u64,
    pub releases: u64,
    pub devices: u64,
    pub bindings: u64,
    pub tokens: u64,
}

/// Which variant a fold lands on: one that already exists, or one to derive.
///
/// A borrow rather than an owned name so the handler can hand its request
/// straight through — and two cases rather than an `Option<i64>` because
/// "create one called this" and "use that one" are different intents, not a
/// present-or-absent field.
#[derive(Debug, Clone, Copy)]
pub enum FoldTarget<'a> {
    Existing(i64),
    New(&'a str),
}

/// One pair of chains that look like the same rig, and what folding would do.
///
/// Reported, never acted on — the same preview-then-apply discipline the
/// metadata sanitiser follows, and for the same reason: a fold is not
/// reversible, and a plurality of one step is a weak signal the operator should
/// see before agreeing to it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FoldSuggestion {
    pub base_chain_id: i64,
    pub base_name: String,
    pub base_hours: f64,
    pub fold_chain_id: i64,
    pub fold_name: String,
    pub fold_hours: f64,
    /// How many steps disagree. 0 is the same wiring under two names.
    pub distance: i64,
    /// What the folded chain becomes as a variant of the base.
    pub overrides: Vec<VariantOverride>,
    /// A variant of the base that is *already* wired this way — the state
    /// someone is in when they built the variant and found the old chain still
    /// sitting in the list beside it.
    pub matching_variant: Option<(i64, String)>,
    /// This one carries variants of its own, so it cannot be the side that
    /// folds. Surfaced here rather than left for the fold to refuse.
    pub fold_blocked_by_variants: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChainVariant {
    pub id: i64,
    pub user_id: i64,
    pub chain_id: i64,
    pub name: String,
    pub overrides: Vec<VariantOverride>,
    pub created_at: i64,
    pub total_hours: f64,
}

impl From<ChainVariantRow> for ChainVariant {
    fn from(r: ChainVariantRow) -> Self {
        ChainVariant {
            id: r.id,
            user_id: r.user_id,
            chain_id: r.chain_id,
            name: r.name,
            overrides: serde_json::from_str(&r.overrides).unwrap_or_default(),
            created_at: r.created_at,
            total_hours: r.total_hours,
        }
    }
}

#[derive(Debug, FromRow)]
pub struct ChainRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub components: String,
    pub listening_context: String,
    pub icon: Option<String>,
    pub is_active: bool,
    pub shared: bool,
    pub total_hours: f64,
    pub created_at: i64,
}

impl From<ChainRow> for SignalChain {
    fn from(r: ChainRow) -> Self {
        let components = serde_json::from_str(&r.components).unwrap_or_default();
        SignalChain {
            id: Some(r.id),
            user_id: r.user_id,
            name: r.name,
            description: r.description,
            components,
            listening_context: ListeningContext::from_str_loose(&r.listening_context),
            icon: r.icon,
            is_active: r.is_active,
            shared: r.shared,
            total_hours: r.total_hours,
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct DeviceRow {
    pub id: i64,
    pub user_id: i64,
    pub machine_id: String,
    pub name: Option<String>,
    pub platform: Option<String>,
    pub product: Option<String>,
    pub device_type: Option<String>,
    pub default_chain_id: Option<i64>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub total_listens: i64,
}

/// Where one piece of gear's hours came from.
///
/// The gear total is a sum and a sum is not checkable — "Hexa: 1.9h" is a
/// number you either believe or you do not, and with a variant in play the
/// listening genuinely arrives from more than one chain. This is the same
/// arithmetic `equipment_seconds` already does internally, kept instead of
/// thrown away, so the total can be read as the addition it is.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GearHours {
    pub chain_id: i64,
    pub chain_name: String,
    /// Set when the listening went through a *variant* of that chain — which is
    /// the case the breakdown exists for, since that is when one piece of gear
    /// accrues hours through a chain that does not name it.
    pub variant_id: Option<i64>,
    pub variant_name: Option<String>,
    pub hours: f64,
}

/// A track still wanting a language, and what is known about it.
///
/// `key` is the grouping key **as SQLite computed it** — carried rather than
/// recomputed so a second pass can join on it without a Rust fold disagreeing
/// with SQLite's ASCII-only `LOWER`.
#[derive(Debug, Clone)]
pub struct LanguageCandidate {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// The modal running time across this track's listens, for the lyric match.
    pub duration: Option<i64>,
    pub plays: i64,
}

/// Where a track sits in one of the user's own media servers.
///
/// `source_ref` is which server, which is what makes this work when somebody
/// runs two of the same kind — the same reason the artwork proxy reads it.
#[derive(Debug, Clone)]
pub struct LibraryRef {
    pub user_id: i64,
    pub source_ref: i64,
    pub source_id: String,
    pub source_name: String,
    pub timestamp: i64,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct EquipmentRow {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    #[sqlx(rename = "type")]
    pub equipment_type: String,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub total_hours: f64,
    pub first_used: Option<i64>,
    pub last_used: Option<i64>,
    pub notes: Option<String>,
    /// When it was bought, as opposed to `first_used`, which is when Tapedeck
    /// first saw it. Gear is usually owned well before it is catalogued.
    pub purchased_at: Option<i64>,
    /// Which chains and variants those hours came from, largest first. Empty
    /// for gear nothing has logged time through.
    #[sqlx(skip)]
    pub sources: Vec<GearHours>,
    /// Why this piece is not in a chain — `new`, `spare`, `lent`. Only ever
    /// meaningful for gear no chain names; the Signal Chains page shows it in
    /// the "Not in a chain yet" table and nowhere else.
    pub status: Option<String>,
}

mod auth;
mod chains;
mod connections;
mod crate_shelf;
pub mod dialect;
mod gear;
#[cfg(test)]
pub(crate) mod guards;
mod import;
mod library;
mod listens;
mod migrate;
mod notes;
mod patch;
mod physical;
mod rediscovery;
mod sanitise;
mod schema;
mod sources;
mod stats;
mod users;

// These build a temp SQLite file and drive it through the real `Database`.
// Under `--features postgres` that type is `Pool<Postgres>`, so they cannot run
// without a live server and are compiled out rather than left to fail.
//
// **That is a real gap, not a tidy-up.** The Postgres build's data layer has no
// test coverage at all today — 153 tests' worth. Pointing this module at a
// throwaway Postgres schema (a `TAPEDECK_TEST_DATABASE_URL`, a schema per test,
// dropped afterwards) is the work that closes it, and it should land before
// anyone points a Postgres build at data they care about.
#[cfg(all(test, feature = "postgres"))]
mod pg_live;

#[cfg(all(test, not(feature = "postgres")))]
mod tests;
