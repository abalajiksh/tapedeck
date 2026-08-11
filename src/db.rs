use crate::crypto::SecretBox;
use crate::models::*;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    FromRow, Pool, Sqlite,
};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
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
    pub session_id: Option<i64>,
    pub listening_context: Option<String>,
    pub submission_client: Option<String>,
    pub track_number: Option<i32>,
    pub artwork_url: Option<String>,
    pub source_ref: Option<i64>,
    #[serde(default)]
    pub delivered_sinks: Option<String>,
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
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
}

#[derive(Debug, Clone, FromRow, serde::Serialize)]
pub struct NoteRow {
    pub id: i64,
    pub user_id: i64,
    pub entity_id: Option<i64>,
    pub scrobble_id: Option<i64>,
    pub body_md: String,
    pub visibility: String,
    pub created_at: i64,
    pub updated_at: i64,
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
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
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
    /// What this normally gets played on. The deck pre-fills from these and
    /// writes the answer back, so a record with one turntable is asked once.
    pub chain_id: Option<i64>,
    pub equipment_id: Option<i64>,
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

pub struct Database {
    pool: Pool<Sqlite>,
    /// Encrypts the credential blobs Tapedeck must keep replayable. Held here so
    /// ciphertext never escapes this module — callers pass and receive plaintext.
    secrets: Arc<SecretBox>,
}

/// How long after a listen a second report of it is still assumed to be the
/// same play rather than a repeat. See `fuzzy_exists`.
const DUPLICATE_WINDOW_SECS: i64 = 60;

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
        // WAL + a busy_timeout let concurrent writers (poll engine, HTTP
        // handlers, background imports) wait for the lock instead of erroring
        // out with SQLITE_BUSY — which would otherwise drop listens on ingest.
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10))
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await?;
        let db = Self { pool, secrets };
        db.init().await?;
        db.migrate().await?;
        db.encrypt_legacy_credentials().await?;
        Ok(db)
    }

    async fn init(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS scrobbles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL DEFAULT 1,
                title TEXT NOT NULL, artist TEXT NOT NULL, album TEXT,
                artist_credit TEXT,
                timestamp INTEGER NOT NULL, duration INTEGER,
                source_id TEXT NOT NULL, source_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                mbid_recording TEXT, mbid_release TEXT, mbid_artist TEXT,
                caa_id INTEGER, caa_release_mbid TEXT,
                format_type TEXT, codec TEXT, bitrate INTEGER,
                sample_rate INTEGER, bit_depth INTEGER, channels INTEGER,
                container TEXT, source_quality TEXT,
                is_lossless BOOLEAN, dsd_rate INTEGER, dsd_multiplier INTEGER,
                dsd_to_pcm_converted BOOLEAN,
                delivery_codec TEXT, delivery_bitrate INTEGER,
                delivery_sample_rate INTEGER, delivery_bit_depth INTEGER,
                is_transcoded BOOLEAN, transcode_reason TEXT, quality_score REAL,
                device_id INTEGER REFERENCES devices(id),
                chain_id INTEGER REFERENCES signal_chains(id),
                session_id INTEGER REFERENCES sessions(id),
                listening_context TEXT, submission_client TEXT,
                delivered_sinks TEXT NOT NULL DEFAULT '[]',
                track_number INTEGER,
                language TEXT,
                genres TEXT,
                release_year INTEGER,
                skipped BOOLEAN NOT NULL DEFAULT FALSE,
                artwork_url TEXT,
                source_ref INTEGER,
                spotify_track_id TEXT,
                UNIQUE(user_id, source_id, source_name)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT UNIQUE NOT NULL,
                display_name TEXT,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'user',
                created_at INTEGER NOT NULL,
                settings JSON DEFAULT '{}',
                disabled INTEGER NOT NULL DEFAULT 0,
                avatar_url TEXT,
                bio TEXT
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                token_hash TEXT UNIQUE NOT NULL,
                name TEXT NOT NULL,
                scopes TEXT DEFAULT 'submit',
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                expires_at INTEGER,
                default_chain_id INTEGER REFERENCES signal_chains(id),
                -- Whether this client's import-type submissions are
                -- live listens rather than a backfill. Off by default and never
                -- inferred: turning it on lets a client write to the owner's
                -- permanent Last.fm/ListenBrainz record, which is not a decision
                -- to make on their behalf.
                imports_are_live BOOLEAN DEFAULT FALSE
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS signal_chains (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                name TEXT NOT NULL, description TEXT,
                components JSON NOT NULL DEFAULT '[]',
                listening_context TEXT NOT NULL DEFAULT 'unknown',
                icon TEXT,
                is_active BOOLEAN DEFAULT TRUE,
                total_hours REAL DEFAULT 0,
                created_at INTEGER NOT NULL,
                UNIQUE(user_id, name)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS devices (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                machine_id TEXT NOT NULL, name TEXT, platform TEXT,
                product TEXT, device_type TEXT,
                default_chain_id INTEGER REFERENCES signal_chains(id),
                first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                total_listens INTEGER DEFAULT 0,
                UNIQUE(user_id, machine_id)
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Physical media ──
        //
        // A record or tape you own. Separate from `entities` on purpose: an
        // entity is a work you have listened to, this is an *object* with a
        // pressing, a barcode and a side order. Two pressings of the same album
        // are two rows here and one entity.
        //
        // `tracks` is a JSON array of {side, position, title, duration_secs}.
        // A blob rather than a table because it is only ever read and written
        // whole, and because side assignment is frequently hand-corrected —
        // MusicBrainz records vinyl track numbers as "A1" only when the editor
        // bothered, and as plain 1..n otherwise.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS physical_releases (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                kind TEXT NOT NULL CHECK (kind IN ('vinyl', 'cassette', 'cd', 'sacd')),
                artist TEXT NOT NULL, title TEXT NOT NULL,
                year INTEGER,
                mbid_release TEXT, discogs_id TEXT, barcode TEXT, catalog_number TEXT,
                artwork_url TEXT,
                tracks TEXT NOT NULL DEFAULT '[]',
                notes TEXT,
                added_at INTEGER NOT NULL,
                -- When you bought it. A shelf is a thing you accumulate, so how
                -- long something has been yours is a fact about the shelf that
                -- nothing else in the database can answer.
                purchased_at INTEGER,
                -- What this normally gets played on. A record lives with one
                -- turntable, so asking every time would be asking the same
                -- question forever; the deck pre-fills from here and the answer
                -- is written back.
                chain_id INTEGER REFERENCES signal_chains(id),
                equipment_id INTEGER REFERENCES equipment(id),
                -- Where the tape was left. A cassette does not rewind itself:
                -- stop it halfway and eject, and it is still parked there the
                -- next time you put it in. That is a fact about the object,
                -- which is what this shelf is for. Written for cassettes only —
                -- see `park_tape` for why a record deliberately has no memory.
                parked_side TEXT,
                parked_secs INTEGER
            )",
        )
        .execute(&self.pool)
        .await?;

        // What is on the deck right now, as a fact about the *shelf* rather
        // than about a browser.
        //
        // This started in localStorage on the reasoning that a side in progress
        // is a fact about the machine sitting in front of the turntable. That
        // is true of where the *clock* runs and false of everything else: a
        // record is on the platter whichever device you happen to be holding,
        // so the phone in your pocket could not see it, a second tab disagreed
        // with the first, and a browser crash lost the side outright with no
        // way to recover what had already played.
        //
        // `user_id` is the PRIMARY KEY, which is the constraint that matters:
        // one deck per person, because nobody listens to two records at once,
        // and it makes "resume what I was playing" a single row lookup with no
        // ambiguity about which spin is the real one.
        //
        // `outcomes` is the per-track record the browser accumulates (position,
        // real start, skipped) and is JSON because it is written and read
        // whole, never queried into.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS physical_spins (
                user_id INTEGER PRIMARY KEY REFERENCES users(id),
                release_id INTEGER NOT NULL REFERENCES physical_releases(id),
                side TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                paused_at INTEGER,
                paused_total INTEGER NOT NULL DEFAULT 0,
                outcomes TEXT NOT NULL DEFAULT '[]',
                chain_id INTEGER REFERENCES signal_chains(id),
                equipment_id INTEGER REFERENCES equipment(id),
                -- How far into the side this spin began. Non-zero when the
                -- needle was dropped mid-side, or when a tape resumed from
                -- where it was parked, so the countdown reads a position
                -- within the side rather than starting from nothing.
                start_secs INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // Sleeves, booklets, labels, obi strips — the parts of a record you
        // actually hold. A release has *many* of these (the Cover Art Archive
        // carries 29 for the pressing this was built against, 19 of them
        // booklet pages), which is why they are rows rather than a second
        // `artwork_url` column.
        //
        // Two provenances share the table because they answer the same question
        // and the difference matters only to the reader: `caa`/`discogs` hold a
        // remote **URL** — the same rule the artwork chain follows, providers
        // return URLs and never bytes — while `upload` holds a **file on this
        // server**, because a scan of your own copy exists nowhere else.
        //
        // `kind` is free text rather than a CHECK: the archive adds types, and a
        // scan of something it has no word for is still a scan.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS physical_images (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                release_id INTEGER NOT NULL REFERENCES physical_releases(id),
                kind TEXT NOT NULL DEFAULT 'other',
                provider TEXT NOT NULL,
                url TEXT, thumb_url TEXT,
                file_path TEXT, mime TEXT, bytes INTEGER,
                caption TEXT,
                -- Order within the release, which for a booklet is page order.
                position INTEGER NOT NULL DEFAULT 0,
                -- The provider's own id, so re-fetching a release updates the
                -- rows it already has instead of stacking a second copy of
                -- every page on top of the first.
                remote_id TEXT,
                added_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_physical_images_remote
                ON physical_images(release_id, provider, remote_id)
             WHERE remote_id IS NOT NULL",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_physical_images_release
                ON physical_images(release_id, position)",
        )
        .execute(&self.pool)
        .await?;

        // One row per *side played*, which is the unit you actually handle: you
        // put a side on, it plays through, you flip it. The listens themselves
        // are ordinary `scrobbles` rows — this is the record of the physical
        // act, and what stylus hours and a cassette counter are computed from.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS physical_plays (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                release_id INTEGER NOT NULL REFERENCES physical_releases(id),
                side TEXT NOT NULL,
                played_at INTEGER NOT NULL,
                duration_secs INTEGER NOT NULL DEFAULT 0,
                track_count INTEGER NOT NULL DEFAULT 0,
                equipment_id INTEGER REFERENCES equipment(id),
                chain_id INTEGER REFERENCES signal_chains(id)
            )",
        )
        .execute(&self.pool)
        .await?;

        // Server-wide settings an admin sets in the UI, as opposed to
        // `tapedeck.toml` (infrastructure) or per-user preferences. Values are
        // encrypted, because the first inhabitant is a Discogs token.
        // Device-code pairing. A client that cannot show a browser — a DAP, a
        // headless player, a TV app — asks for a code here and polls; the user
        // approves it in the web UI and the client collects its token.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pairing_requests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Hashed like a token: the device code is a bearer secret, and
                -- a leaked database should not hand over pending pairings.
                device_code_hash TEXT NOT NULL UNIQUE,
                -- Short and typeable, so it can be read off one screen and into
                -- another. Guessable by construction, which is why it lives for
                -- five minutes, is single-use, and is rate limited.
                user_code TEXT NOT NULL UNIQUE,
                client_name TEXT NOT NULL,
                scopes TEXT NOT NULL DEFAULT 'submit',
                user_id INTEGER REFERENCES users(id),
                default_chain_id INTEGER REFERENCES signal_chains(id),
                -- The minted token, held only until the client collects it.
                token TEXT,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                approved_at INTEGER
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS equipment (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                name TEXT NOT NULL, type TEXT NOT NULL,
                brand TEXT, model TEXT,
                total_hours REAL DEFAULT 0,
                first_used INTEGER, last_used INTEGER, notes TEXT,
                -- Distinct from `first_used`, which is when Tapedeck first saw
                -- it. Gear is usually owned for a while before it is logged,
                -- and burn-in hours are counted from the purchase, not from the
                -- day someone got round to typing it in.
                purchased_at INTEGER,
                UNIQUE(user_id, name)
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                device_id INTEGER REFERENCES devices(id),
                chain_id INTEGER REFERENCES signal_chains(id),
                started_at INTEGER NOT NULL, ended_at INTEGER,
                track_count INTEGER DEFAULT 0, total_duration INTEGER DEFAULT 0,
                skip_count INTEGER DEFAULT 0, avg_quality_score REAL,
                listening_context TEXT DEFAULT 'unknown',
                token_id INTEGER REFERENCES tokens(id),
                uuid TEXT,
                title TEXT,
                updated_at INTEGER
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Web Sessions (browser login) ──
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                session_hash TEXT UNIQUE NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                last_active_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Output-device → chain bindings (Phase B) ──
        // One row per distinct output the client reports (USB DAC name, BT
        // device, "wired", …). chain_id NULL = seen but unmapped, which the UI
        // surfaces for one-tap assignment (auto-learn).
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS chain_bindings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                identifier TEXT NOT NULL,
                interface TEXT,
                chain_id INTEGER REFERENCES signal_chains(id),
                last_seen INTEGER NOT NULL,
                hits INTEGER NOT NULL DEFAULT 0,
                UNIQUE(user_id, identifier)
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Per-user scrobble connections (Last.fm / ListenBrainz) ──
        // Credentials stay server-side; the API only ever exposes the connected
        // account name, never the token/session key.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_connections (
                user_id INTEGER NOT NULL REFERENCES users(id),
                service TEXT NOT NULL,
                credentials TEXT NOT NULL,
                connected_username TEXT,
                connected_at INTEGER NOT NULL,
                PRIMARY KEY (user_id, service)
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Import history (one row per finished import job) ──
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS import_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                source TEXT NOT NULL,
                state TEXT NOT NULL,
                fetched INTEGER NOT NULL,
                imported INTEGER NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // Artists as entities rather than bare strings on a listen. Needed for
        // anywhere an artist has properties of its own — an image today, MBIDs
        // and liner notes later. Global, not per-user: "Aphex Twin" is the same
        // artist for everyone on the instance.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS artists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                name_key TEXT NOT NULL UNIQUE,
                mbid TEXT,
                image_url TEXT,
                genres TEXT,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await?;

        // ── Entities, liner notes and loves (Phase 7) ──
        //
        // Anything you can write about: a recording, a release, an artist.
        // Deliberately *not* keyed by MBID — an un-enriched listen has none, so
        // notes would silently fail on exactly the obscure records worth
        // annotating. The MBID is one alias among others.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS entities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                artist_name TEXT NOT NULL DEFAULT '',
                entity_key TEXT NOT NULL UNIQUE,
                mbid TEXT,
                image_url TEXT,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        // Other names the same entity answers to. Today this carries MBIDs; it
        // is what makes merging two spellings of one record possible later
        // without rewriting every note that points at either.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS bio_cache (
                entity_id INTEGER PRIMARY KEY REFERENCES entities(id),
                bio TEXT,
                attribution TEXT,
                provider TEXT,
                url TEXT,
                fetched_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS entity_aliases (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_id INTEGER NOT NULL REFERENCES entities(id),
                alias_kind TEXT NOT NULL,
                value TEXT NOT NULL,
                UNIQUE(entity_id, alias_kind, value)
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_entity_aliases_lookup ON entity_aliases(alias_kind, value)")
            .execute(&self.pool).await?;

        // `scrobbles` carried no index at all beyond the UNIQUE it needs for
        // dedup, so every ranged query — history, stats, all seven rediscovery
        // collections — was a full scan of the user's rows plus a temp B-tree.
        // Fine at 21k rows on a laptop, and the reason Rediscovery could take
        // long enough on a small host to be killed by a gateway timeout.
        //
        // `CREATE INDEX IF NOT EXISTS` is idempotent, so this needs no
        // `migrate()` block — an existing database gains the index on next
        // open. That is the one schema change that does not need the
        // add-to-init-*and*-migrate dance.
        //
        // `(user_id, timestamp)` is the shape nearly every query wants: scoped
        // to the caller, then bounded or ordered by time. `skipped` is left out
        // deliberately — it is a low-cardinality flag, and putting it in front
        // of `timestamp` would help no range scan while making the index wider.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_user_time ON scrobbles(user_id, timestamp)",
        )
        .execute(&self.pool)
        .await?;
        // Grouping queries (top artists/albums/tracks, the variant pass) sort
        // by artist within a user before they aggregate.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_user_artist ON scrobbles(user_id, artist)",
        )
        .execute(&self.pool)
        .await?;

        // A note is a living document: one per user per target, edited in place.
        // Exactly one of entity_id / scrobble_id is set — a note about a record
        // and a note about one particular playing of it are different things.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                entity_id INTEGER REFERENCES entities(id),
                scrobble_id INTEGER REFERENCES scrobbles(id),
                body_md TEXT NOT NULL,
                visibility TEXT NOT NULL DEFAULT 'private',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                CHECK ((entity_id IS NULL) <> (scrobble_id IS NULL))
            )",
        )
        .execute(&self.pool)
        .await?;
        // Partial uniques: one note per target per user, enforced by the
        // database rather than by every write path remembering to check.
        sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_entity ON notes(user_id, entity_id) WHERE entity_id IS NOT NULL")
            .execute(&self.pool).await?;
        sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_scrobble ON notes(user_id, scrobble_id) WHERE scrobble_id IS NOT NULL")
            .execute(&self.pool).await?;

        // Every edit keeps what was there before. These are meant to be kept for
        // years; a rewrite in 2031 shouldn't erase what you thought in 2026.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS note_revisions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                note_id INTEGER NOT NULL REFERENCES notes(id),
                body_md TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_note_revisions_note ON note_revisions(note_id, created_at)")
            .execute(&self.pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS loves (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                entity_id INTEGER NOT NULL REFERENCES entities(id),
                created_at INTEGER NOT NULL,
                UNIQUE(user_id, entity_id)
            )",
        )
        .execute(&self.pool)
        .await?;

        // One row per user per source kind. Replaces the single-row
        // `plex_settings`: a source belongs to the person whose account it
        // reads, so nobody's token can see anybody else's listening.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_sources (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                kind TEXT NOT NULL,
                label TEXT NOT NULL DEFAULT '',
                enabled INTEGER NOT NULL DEFAULT 1,
                url TEXT NOT NULL DEFAULT '',
                credentials TEXT NOT NULL DEFAULT '',
                remote_account TEXT,
                poll_interval_secs INTEGER,
                options TEXT NOT NULL DEFAULT '{}',
                last_polled_at INTEGER,
                last_error TEXT,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await?;

        // Plex is a single server-wide source, not a per-user connection, so
        // this is deliberately a one-row table (the CHECK enforces it).
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS plex_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                enabled INTEGER NOT NULL DEFAULT 0,
                url TEXT NOT NULL DEFAULT '',
                token TEXT NOT NULL DEFAULT '',
                users_allow TEXT NOT NULL DEFAULT '',
                users_block TEXT NOT NULL DEFAULT '',
                devices_allow TEXT NOT NULL DEFAULT '',
                devices_block TEXT NOT NULL DEFAULT '',
                libraries_allow TEXT NOT NULL DEFAULT '',
                libraries_block TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn migrate(&self) -> Result<(), sqlx::Error> {
        async fn has_column(pool: &Pool<Sqlite>, table: &str, column: &str) -> bool {
            let result: Result<(i64,), _> = sqlx::query_as(&format!(
                "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='{}'",
                table, column
            ))
            .fetch_one(pool)
            .await;
            matches!(result, Ok((count,)) if count > 0)
        }

        // Migration 1: MusicBrainz fields
        if !has_column(&self.pool, "scrobbles", "mbid_recording").await {
            info!("⚙️ Migrating: adding MusicBrainz fields...");
            for col in &[
                "mbid_recording TEXT",
                "mbid_release TEXT",
                "mbid_artist TEXT",
                "caa_id INTEGER",
                "caa_release_mbid TEXT",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {}", col))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration 2: user_id
        if !has_column(&self.pool, "scrobbles", "user_id").await {
            info!("⚙️ Migrating: adding user_id...");
            let _ =
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1")
                    .execute(&self.pool)
                    .await;
            let _ = sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_scrobbles_user_source ON scrobbles(user_id, source_id, source_name)")
                .execute(&self.pool).await;
        }

        // Migration 3: Phase 2 quality + context columns
        if !has_column(&self.pool, "scrobbles", "format_type").await {
            info!("⚙️ Migrating: adding Phase 2 quality and context fields...");
            for col in &[
                "format_type TEXT",
                "codec TEXT",
                "bitrate INTEGER",
                "sample_rate INTEGER",
                "bit_depth INTEGER",
                "channels INTEGER",
                "is_lossless BOOLEAN",
                "dsd_rate INTEGER",
                "dsd_multiplier INTEGER",
                "delivery_codec TEXT",
                "delivery_bitrate INTEGER",
                "is_transcoded BOOLEAN",
                "transcode_reason TEXT",
                "quality_score REAL",
                "device_id INTEGER",
                "chain_id INTEGER",
                "session_id INTEGER",
                "listening_context TEXT",
                "submission_client TEXT",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {}", col))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration 4: role column on users
        if !has_column(&self.pool, "users", "role").await {
            info!("⚙️ Migrating: adding role column to users...");
            let _ = sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user'")
                .execute(&self.pool)
                .await;
            // First user (id=1) is always admin
            let _ = sqlx::query("UPDATE users SET role = 'admin' WHERE id = 1")
                .execute(&self.pool)
                .await;
            info!("✅ Role migration complete");
        }

        // Migration 5: per-sink delivery tracking
        if !has_column(&self.pool, "scrobbles", "delivered_sinks").await {
            info!("⚙️ Migrating: adding delivered_sinks...");
            let _ = sqlx::query(
                "ALTER TABLE scrobbles ADD COLUMN delivered_sinks TEXT NOT NULL DEFAULT '[]'",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration 6: disabled flag on users
        if !has_column(&self.pool, "users", "disabled").await {
            info!("⚙️ Migrating: adding users.disabled...");
            let _ = sqlx::query("ALTER TABLE users ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0")
                .execute(&self.pool)
                .await;
        }

        // Migration 7: track_number on scrobbles (album-playthrough analytics)
        if !has_column(&self.pool, "scrobbles", "track_number").await {
            info!("⚙️ Migrating: adding scrobbles.track_number...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN track_number INTEGER")
                .execute(&self.pool)
                .await;
        }

        // Migration 8: language on scrobbles (language breakdown analytics)
        if !has_column(&self.pool, "scrobbles", "language").await {
            info!("⚙️ Migrating: adding scrobbles.language...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN language TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: which `user_sources` row a listen came from. Needed once a
        // user can have two Plex servers — `plex-55821-np` alone doesn't say
        // which one, so the artwork proxy would have to guess.
        if !has_column(&self.pool, "scrobbles", "source_ref").await {
            info!("⚙️ Migrating: adding scrobbles.source_ref...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN source_ref INTEGER")
                .execute(&self.pool)
                .await;
        }

        // Migration: several sources of the same kind per user. SQLite can't
        // drop a UNIQUE constraint in place, so the table is rebuilt. Guarded on
        // the `label` column, which only the new shape has.
        if !has_column(&self.pool, "user_sources", "label").await {
            info!("⚙️ Migrating: allowing multiple sources of the same kind...");
            let rebuild = [
                "CREATE TABLE IF NOT EXISTS user_sources_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL REFERENCES users(id),
                    kind TEXT NOT NULL,
                    label TEXT NOT NULL DEFAULT '',
                    enabled INTEGER NOT NULL DEFAULT 1,
                    url TEXT NOT NULL DEFAULT '',
                    credentials TEXT NOT NULL DEFAULT '',
                    remote_account TEXT,
                    poll_interval_secs INTEGER,
                    options TEXT NOT NULL DEFAULT '{}',
                    last_polled_at INTEGER,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL DEFAULT 0
                )",
                "INSERT INTO user_sources_new
                    (id, user_id, kind, label, enabled, url, credentials, remote_account,
                     poll_interval_secs, options, last_polled_at, last_error, updated_at)
                 SELECT id, user_id, kind, '', enabled, url, credentials, remote_account,
                        poll_interval_secs, options, last_polled_at, last_error, updated_at
                 FROM user_sources",
                "DROP TABLE user_sources",
                "ALTER TABLE user_sources_new RENAME TO user_sources",
            ];
            for stmt in rebuild {
                if let Err(e) = sqlx::query(stmt).execute(&self.pool).await {
                    error!("user_sources rebuild failed: {} — leaving the old table in place", e);
                    break;
                }
            }
        }

        // Migration: resolved artwork URL. Supersedes deriving a Cover Art
        // Archive link from caa_id — artwork can now come from Deezer or the
        // user's own library, neither of which has a CAA id.
        if !has_column(&self.pool, "scrobbles", "artwork_url").await {
            info!("⚙️ Migrating: adding scrobbles.artwork_url...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN artwork_url TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: skipped listens (Rockbox logs mark rows L or S). Stored so
        // skip-rate analytics are possible, but excluded from every count.
        if !has_column(&self.pool, "scrobbles", "skipped").await {
            info!("⚙️ Migrating: adding scrobbles.skipped...");
            let _ = sqlx::query(
                "ALTER TABLE scrobbles ADD COLUMN skipped BOOLEAN NOT NULL DEFAULT FALSE",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: the Spotify track id a listen came in with. Written only by
        // the Spotify export importer, and read by nothing yet — it is kept
        // because it is the single external identifier that export carries, and
        // re-deriving it later would mean asking the user for the zip again.
        if !has_column(&self.pool, "scrobbles", "spotify_track_id").await {
            info!("⚙️ Migrating: adding scrobbles.spotify_track_id...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN spotify_track_id TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: per-source options (library/device filters). `user_sources`
        // is young enough that few databases will need this, but the table was
        // shipped without the column for one build.
        if !has_column(&self.pool, "user_sources", "options").await {
            info!("⚙️ Migrating: adding user_sources.options...");
            let _ = sqlx::query(
                "ALTER TABLE user_sources ADD COLUMN options TEXT NOT NULL DEFAULT '{}'",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: release year, for the by-decade breakdown.
        if !has_column(&self.pool, "scrobbles", "release_year").await {
            info!("⚙️ Migrating: adding scrobbles.release_year...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN release_year INTEGER")
                .execute(&self.pool)
                .await;
        }

        // Migration: a token's default signal chain. One token per app ("Pano
        // on Android", "fooyin") is how people actually organise these, so the
        // token is the natural place to say what the listens through it came
        // out of — no client-side config, nothing to type per listen.
        // Migration: sessions become a thing you can look at, name, merge and
        // split — so they need a stable identity that survives all of that.
        // `uuid` is added now rather than when Phase 6 shares one outward,
        // because retrofitting an id onto rows other instances already hold is
        // the migration that section warns about.
        if !has_column(&self.pool, "sessions", "uuid").await {
            info!("⚙️ Migrating: adding sessions.uuid/title/updated_at...");
            for col in ["uuid TEXT", "title TEXT", "updated_at INTEGER"] {
                let _ = sqlx::query(&format!("ALTER TABLE sessions ADD COLUMN {}", col))
                    .execute(&self.pool)
                    .await;
            }
            // Backfill: every existing session gets an id too, or the ones that
            // predate this are the only ones that can never be shared.
            let ids: Vec<(i64,)> = sqlx::query_as("SELECT id FROM sessions WHERE uuid IS NULL")
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default();
            for (id,) in ids {
                let _ = sqlx::query("UPDATE sessions SET uuid = ? WHERE id = ?")
                    .bind(new_uuid())
                    .bind(id)
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration: the full artist credit, kept when a `feat.` suffix is
        // folded out of `artist` so the featured performer is never lost —
        // it just stops fragmenting every grouping query.
        if !has_column(&self.pool, "scrobbles", "artist_credit").await {
            info!("⚙️ Migrating: adding scrobbles.artist_credit...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN artist_credit TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: which token a session's listens came in on. A change of
        // token means a change of app, which in practice means a change of
        // setup — so it ends the session even inside the 30-minute gap.
        if !has_column(&self.pool, "sessions", "token_id").await {
            info!("⚙️ Migrating: adding sessions.token_id...");
            let _ = sqlx::query(
                "ALTER TABLE sessions ADD COLUMN token_id INTEGER REFERENCES tokens(id)",
            )
            .execute(&self.pool)
            .await;
        }

        if !has_column(&self.pool, "tokens", "default_chain_id").await {
            info!("⚙️ Migrating: adding tokens.default_chain_id...");
            let _ = sqlx::query(
                "ALTER TABLE tokens ADD COLUMN default_chain_id INTEGER REFERENCES signal_chains(id)",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: whether a token's `import` submissions are live listens.
        // FALSE for every existing token, deliberately — an upgrade must not
        // start forwarding anything that wasn't being forwarded yesterday.
        if !has_column(&self.pool, "tokens", "imports_are_live").await {
            info!("⚙️ Migrating: adding tokens.imports_are_live...");
            let _ =
                sqlx::query("ALTER TABLE tokens ADD COLUMN imports_are_live BOOLEAN DEFAULT FALSE")
                    .execute(&self.pool)
                    .await;
        }

        // Migration: a chain's icon. Which setup a listen came out of is the
        // thing this app is about, and a name in a dropdown is a poor way to
        // recognise one at a glance.
        if !has_column(&self.pool, "signal_chains", "icon").await {
            info!("⚙️ Migrating: adding signal_chains.icon...");
            let _ = sqlx::query("ALTER TABLE signal_chains ADD COLUMN icon TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: artist-level genres. A genre is a property of an artist far
        // more than of one recording, and MusicBrainz tags it that way — so this
        // is where the useful answers actually are.
        if !has_column(&self.pool, "artists", "genres").await {
            info!("⚙️ Migrating: adding artists.genres...");
            let _ =
                sqlx::query("ALTER TABLE artists ADD COLUMN genres TEXT").execute(&self.pool).await;
        }

        // Migration: genres, which MusicBrainz has been handing over since the
        // cache existed. `mb_cache.genres` was written by every lookup and read
        // by nothing, while the ranged-stats payload shipped a hardcoded empty
        // list — the UI had a slot for this all along.
        if !has_column(&self.pool, "scrobbles", "genres").await {
            info!("⚙️ Migrating: adding scrobbles.genres...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN genres TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: the five quality fields that were parsed and thrown away.
        // `AudioQuality` has carried `container`, `delivery_sample_rate` and
        // `delivery_bit_depth` since Phase 2 and the ingest path filled them in,
        // but `save_scrobble` never had columns to bind them to — so the values
        // were built on every listen and dropped on the floor. `source_quality`
        // and `dsd_to_pcm_converted` arrived in the payload and weren't even
        // mapped. "Was this DSD converted to PCM before it reached me?" is a
        // question this app exists to answer and could not.
        if !has_column(&self.pool, "scrobbles", "container").await {
            info!("⚙️ Migrating: adding the remaining scrobble quality fields...");
            for col in &[
                "container TEXT",
                "source_quality TEXT",
                "dsd_to_pcm_converted BOOLEAN",
                "delivery_sample_rate INTEGER",
                "delivery_bit_depth INTEGER",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {}", col))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration: a hand-set cover for a release, the counterpart of the
        // artist portrait override that has existed since v0.21.0. An album's
        // cover is otherwise whichever artwork the newest listen of it happens
        // to carry, and there was no way to say "not that one" — which for a
        // journal is the wrong answer to have no answer to.
        if !has_column(&self.pool, "entities", "image_url").await {
            info!("⚙️ Migrating: adding entities.image_url...");
            let _ = sqlx::query("ALTER TABLE entities ADD COLUMN image_url TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: CD and SACD on the shelf.
        //
        // `physical_releases.kind` carries a CHECK, and SQLite cannot alter a
        // constraint in place — the only way is to build the table again and
        // copy. That is why this one is a whole block rather than an
        // `ADD COLUMN`, and why it is guarded on the *constraint text* in
        // `sqlite_master` rather than on a column existing: the column has
        // always been there, it is what it will accept that changed.
        //
        // `physical_plays.release_id` points at this table, so foreign keys are
        // suspended for the swap — the pool opens with them on. They go back on
        // immediately after, and the whole thing is one transaction, so a crash
        // mid-rebuild leaves the old table rather than a half-copied one.
        let needs_disc_kinds: Option<(String,)> = sqlx::query_as(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'physical_releases'",
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        if needs_disc_kinds.is_some_and(|(sql,)| !sql.contains("'sacd'")) {
            info!("⚙️ Migrating: letting the shelf hold CDs and SACDs...");
            let _ = sqlx::query("PRAGMA foreign_keys = OFF").execute(&self.pool).await;
            let rebuild = async {
                let mut tx = self.pool.begin().await?;
                sqlx::query(
                    "CREATE TABLE physical_releases_new (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        user_id INTEGER NOT NULL REFERENCES users(id),
                        kind TEXT NOT NULL CHECK (kind IN ('vinyl', 'cassette', 'cd', 'sacd')),
                        artist TEXT NOT NULL, title TEXT NOT NULL,
                        year INTEGER,
                        mbid_release TEXT, discogs_id TEXT, barcode TEXT, catalog_number TEXT,
                        artwork_url TEXT,
                        tracks TEXT NOT NULL DEFAULT '[]',
                        notes TEXT,
                        added_at INTEGER NOT NULL
                    )",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO physical_releases_new
                     SELECT id, user_id, kind, artist, title, year, mbid_release, discogs_id,
                            barcode, catalog_number, artwork_url, tracks, notes, added_at
                       FROM physical_releases",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query("DROP TABLE physical_releases").execute(&mut *tx).await?;
                sqlx::query("ALTER TABLE physical_releases_new RENAME TO physical_releases")
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await
            }
            .await;
            if let Err(e) = rebuild {
                warn!("Could not widen the shelf's formats: {}", e);
            }
            let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(&self.pool).await;
        }

        // Migration: when you bought it, and what it gets played on.
        //
        // This has to come **after** the rebuild above, not before: that block
        // copies an explicit column list into a freshly created table, so any
        // column added ahead of it would be silently dropped on the one kind of
        // database that still needs it.
        if !has_column(&self.pool, "physical_releases", "purchased_at").await {
            info!("⚙️ Migrating: purchase dates and default gear on the shelf...");
            for col in &[
                "purchased_at INTEGER",
                "chain_id INTEGER REFERENCES signal_chains(id)",
                "equipment_id INTEGER REFERENCES equipment(id)",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE physical_releases ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }
        if !has_column(&self.pool, "equipment", "purchased_at").await {
            info!("⚙️ Migrating: adding equipment.purchased_at...");
            let _ = sqlx::query("ALTER TABLE equipment ADD COLUMN purchased_at INTEGER")
                .execute(&self.pool)
                .await;
        }

        // Migration: a user's own profile.
        //
        // `avatar_url` is a URL rather than a blob for the same reason the
        // shelf's scans are files on disk — but unlike a booklet an avatar is
        // one small image, so an uploaded one is stored under the uploads dir
        // and this column holds the path that serves it.
        if !has_column(&self.pool, "users", "avatar_url").await {
            info!("⚙️ Migrating: adding user profiles...");
            for col in &["avatar_url TEXT", "bio TEXT"] {
                let _ = sqlx::query(&format!("ALTER TABLE users ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration: where the tape is parked.
        //
        // Like every other `physical_releases` column this comes **after** the
        // disc-kinds rebuild, which copies an explicit column list into a fresh
        // table — anything added ahead of it is silently dropped on the one kind
        // of database that still needs the rebuild.
        //
        // Cassettes only, on purpose. See `park_tape`.
        if !has_column(&self.pool, "physical_releases", "parked_side").await {
            info!("⚙️ Migrating: remembering where a tape was left...");
            for col in &["parked_side TEXT", "parked_secs INTEGER"] {
                let _ = sqlx::query(&format!("ALTER TABLE physical_releases ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }
        if !has_column(&self.pool, "physical_spins", "start_secs").await {
            let _ = sqlx::query(
                "ALTER TABLE physical_spins ADD COLUMN start_secs INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await;
        }

        Ok(())
    }

    // ════════════════════════════════════════════════════════════
    //  Password Hashing (Argon2)
    // ════════════════════════════════════════════════════════════

    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| format!("Password hashing failed: {}", e))
    }

    pub fn verify_password(password: &str, hash: &str) -> bool {
        if hash == "not-used-yet" {
            return false;
        }
        let Ok(parsed) = PasswordHash::new(hash) else {
            return false;
        };
        Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
    }

    // ════════════════════════════════════════════════════════════
    //  Auth: Setup & Login
    // ════════════════════════════════════════════════════════════

    /// Returns true if no user has a real password (first-run or pre-auth upgrade).
    pub async fn needs_setup(&self) -> Result<bool, sqlx::Error> {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM users WHERE password_hash != 'not-used-yet'")
                .fetch_one(&self.pool)
                .await?;
        Ok(count.0 == 0)
    }

    /// Verify username + password, return user info if valid.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<AuthUser>, sqlx::Error> {
        let row: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, username, password_hash, role FROM users WHERE username = ? AND disabled = 0"
        ).bind(username).fetch_optional(&self.pool).await?;

        if let Some((user_id, uname, hash, role)) = row {
            if Self::verify_password(password, &hash) {
                return Ok(Some(AuthUser { user_id, username: uname, is_admin: role == "admin" }));
            }
        }
        Ok(None)
    }

    /// Look up a user id by username (case-insensitive). Used to map a polled
    /// Plex account to the matching Tapedeck user.
    pub async fn get_user_id_by_username(
        &self,
        username: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM users WHERE LOWER(username) = LOWER(?)")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(id,)| id))
    }

    /// Update the password for user_id=1 during setup.
    pub async fn setup_admin(
        &self,
        username: &str,
        display_name: Option<&str>,
        password_hash: &str,
    ) -> Result<i64, sqlx::Error> {
        let has_users = self.has_users().await?;

        if has_users {
            // Update existing user_id=1
            sqlx::query(
                "UPDATE users SET username = ?, display_name = ?, password_hash = ?, role = 'admin' WHERE id = 1"
            )
            .bind(username).bind(display_name).bind(password_hash)
            .execute(&self.pool).await?;
            Ok(1)
        } else {
            // Create new admin
            self.create_user(username, display_name, password_hash, "admin").await
        }
    }

    // ════════════════════════════════════════════════════════════
    //  Web Sessions (browser cookies)
    // ════════════════════════════════════════════════════════════

    fn hash_session(token: &str) -> String {
        Self::hash_token(token)
    }

    /// Create a new web session. Returns the raw session token (for the cookie).
    pub async fn create_web_session(&self, user_id: i64) -> Result<String, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 32] = rand::rng().random();
        let session_token = hex::encode(raw);
        let session_hash = Self::hash_session(&session_token);
        let now = Self::now_secs();
        let expires_at = now + 7 * 86400; // 7 days

        sqlx::query(
            "INSERT INTO web_sessions (user_id, session_hash, created_at, expires_at, last_active_at)
             VALUES (?, ?, ?, ?, ?)"
        )
        .bind(user_id).bind(&session_hash).bind(now).bind(expires_at).bind(now)
        .execute(&self.pool).await?;

        Ok(session_token)
    }

    /// Validate a session token from a cookie. Returns AuthUser if valid.
    pub async fn validate_session(
        &self,
        session_token: &str,
    ) -> Result<Option<AuthUser>, sqlx::Error> {
        let session_hash = Self::hash_session(session_token);
        let now = Self::now_secs();

        let row: Option<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT ws.id, u.id, u.username, u.role FROM web_sessions ws
             JOIN users u ON ws.user_id = u.id
             WHERE ws.session_hash = ? AND ws.expires_at > ? AND u.disabled = 0",
        )
        .bind(&session_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((session_id, user_id, username, role)) = row {
            // Update last_active_at
            let _ = sqlx::query("UPDATE web_sessions SET last_active_at = ? WHERE id = ?")
                .bind(now)
                .bind(session_id)
                .execute(&self.pool)
                .await;
            Ok(Some(AuthUser { user_id, username, is_admin: role == "admin" }))
        } else {
            Ok(None)
        }
    }

    /// Delete a session by its raw token (logout).
    pub async fn delete_web_session(&self, session_token: &str) -> Result<(), sqlx::Error> {
        let session_hash = Self::hash_session(session_token);
        sqlx::query("DELETE FROM web_sessions WHERE session_hash = ?")
            .bind(&session_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ════════════════════════════════════════════════════════════
    //  Scrobble CRUD
    // ════════════════════════════════════════════════════════════

    /// Store one listen.
    ///
    /// `disposition` decides whether the row is ever forwarded. It used to be
    /// hardcoded to `'pending'`, which meant an ingest-path backfill
    /// (`listen_type: "import"`) was indistinguishable from a live listen and
    /// got scrobbled onward — rewriting the user's real Last.fm history. Pass
    /// `ListenDisposition::LIVE` only for something that just played.
    #[allow(clippy::too_many_arguments)]
    pub async fn save_scrobble(
        &self,
        user_id: i64,
        play: &Play,
        quality: Option<&AudioQuality>,
        device_id: Option<i64>,
        chain_id: Option<i64>,
        listening_context: Option<&str>,
        submission_client: Option<&str>,
        source_ref: Option<i64>,
        disposition: ListenDisposition,
    ) -> Result<bool, sqlx::Error> {
        if self.exists(user_id, &play.source_id, &play.source_name).await? {
            return Ok(false);
        }
        if self.fuzzy_exists(user_id, play).await? {
            debug!("Skipping duplicate play (fuzzy match): {} - {}", play.artist, play.title);
            return Ok(false);
        }
        let mbid_artist_json =
            play.mbid_artist.as_ref().map(|arr| serde_json::to_string(arr).unwrap());
        let q = quality.cloned().unwrap_or_default();

        // Everyone credited, when the source named more than one. `artist` is
        // the primary — for a film soundtrack the music director, which is how
        // the record is filed and what gets forwarded — and this keeps the
        // performers rather than discarding them. Same column and same meaning
        // as `fold_artist_credit`: the fuller form of the credit.
        let artist_credit = play.artists.as_ref().filter(|a| a.len() > 1).map(|a| a.join("; "));

        sqlx::query(
            "INSERT INTO scrobbles (
                user_id, title, artist, artist_credit, album, timestamp, duration,
                source_id, source_name, status, skipped,
                mbid_recording, mbid_release, mbid_artist, caa_id, caa_release_mbid,
                format_type, codec, bitrate, sample_rate, bit_depth, channels,
                container, source_quality, is_lossless,
                dsd_rate, dsd_multiplier, dsd_to_pcm_converted,
                delivery_codec, delivery_bitrate, delivery_sample_rate, delivery_bit_depth,
                is_transcoded, transcode_reason, quality_score,
                device_id, chain_id, listening_context, submission_client, track_number,
                source_ref
            ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?
            )",
        )
        .bind(user_id)
        .bind(&play.title)
        .bind(&play.artist)
        .bind(&artist_credit)
        .bind(&play.album)
        .bind(play.timestamp as i64)
        .bind(play.duration.map(|d| d as i64))
        .bind(&play.source_id)
        .bind(&play.source_name)
        .bind(disposition.status())
        .bind(disposition.skipped)
        .bind(&play.mbid_recording)
        .bind(&play.mbid_release)
        .bind(&mbid_artist_json)
        .bind(play.caa_id)
        .bind(&play.caa_release_mbid)
        .bind(&q.format_type)
        .bind(&q.codec)
        .bind(q.bitrate)
        .bind(q.sample_rate)
        .bind(q.bit_depth.map(|v| v as i32))
        .bind(q.channels.map(|v| v as i32))
        .bind(&q.container)
        .bind(&q.source_quality)
        .bind(q.is_lossless)
        .bind(q.dsd_rate)
        .bind(q.dsd_multiplier.map(|v| v as i32))
        .bind(q.dsd_to_pcm_converted)
        .bind(&q.delivery_codec)
        .bind(q.delivery_bitrate)
        .bind(q.delivery_sample_rate)
        .bind(q.delivery_bit_depth)
        .bind(q.is_transcoded)
        .bind(&q.transcode_reason)
        .bind(q.quality_score)
        .bind(device_id)
        .bind(chain_id)
        .bind(listening_context)
        .bind(submission_client)
        .bind(play.track_number)
        .bind(source_ref)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    pub async fn exists(
        &self,
        user_id: i64,
        source_id: &str,
        source_name: &str,
    ) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND source_id = ? AND source_name = ?"
        ).bind(user_id).bind(source_id).bind(source_name).fetch_one(&self.pool).await?;
        Ok(count.0 > 0)
    }

    /// Has this play already been recorded under a different `source_id`?
    ///
    /// The window is the track's own length rather than a flat ten minutes,
    /// because those are two different questions. A duplicate — the same play
    /// arriving from a second code path, such as Plex's history endpoint
    /// re-reporting what the live session already banked — lands within
    /// seconds. A track *on repeat* is a genuinely separate listen and cannot
    /// arrive sooner than one track length later. A fixed ten-minute window
    /// can't tell them apart and silently swallowed every repeat of anything
    /// shorter than ten minutes, which is nearly everything.
    ///
    /// Unknown duration falls back to a conservative minute: better a rare
    /// duplicate than a silently dropped listen.
    pub async fn fuzzy_exists(&self, user_id: i64, play: &Play) -> Result<bool, sqlx::Error> {
        // A duplicate is a *reporting lag* phenomenon and is therefore roughly
        // constant: the same play arriving from a second code path lands within
        // seconds (measured across a 21k-listen export: 0-51s, median 3s). A
        // repeat is one full track later. So the window is a constant, capped
        // below one track length so a short track's repeat still escapes it.
        //
        // It used to be the duration itself, which put a back-to-back repeat
        // *exactly* on the boundary — and `BETWEEN` is inclusive, so whether a
        // second play survived came down to poll jitter. That is why a track
        // played twice showed up once, intermittently.
        let window = play
            .duration
            .map(|d| (d as i64 * 4 / 5).clamp(20, DUPLICATE_WINDOW_SECS))
            .unwrap_or(DUPLICATE_WINDOW_SECS);
        let timestamp = play.timestamp as i64;
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND title = ? AND artist = ?
             AND timestamp BETWEEN ? - ? AND ? + ?",
        )
        .bind(user_id)
        .bind(&play.title)
        .bind(&play.artist)
        .bind(timestamp)
        .bind(window)
        .bind(timestamp)
        .bind(window)
        .fetch_one(&self.pool)
        .await?;
        Ok(count.0 > 0)
    }

    pub async fn get_pending_scrobbles(&self) -> Result<Vec<PendingScrobble>, sqlx::Error> {
        let records: Vec<ScrobbleRecord> = sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE status = 'pending' ORDER BY timestamp ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(records
            .into_iter()
            .map(|r| {
                let mbid_artist =
                    r.mbid_artist.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());
                let delivered = r
                    .delivered_sinks
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                    .unwrap_or_default();
                PendingScrobble {
                    id: r.id,
                    user_id: r.user_id,
                    delivered,
                    play: Play {
                        title: r.title,
                        artist: r.artist,
                        album: r.album,
                        timestamp: r.timestamp as u64,
                        duration: r.duration.map(|d| d as u64),
                        track_number: None,
                        source_id: r.source_id,
                        source_name: r.source_name,
                        mbid_recording: r.mbid_recording,
                        mbid_release: r.mbid_release,
                        mbid_artist,
                        artists: None,
                        mbid_release_group: None,
                        caa_id: r.caa_id,
                        caa_release_mbid: r.caa_release_mbid,
                    },
                }
            })
            .collect())
    }

    /// Append a sink to a scrobble's delivered set (idempotent). Prevents a
    /// retry from re-sending to a sink that already accepted the listen.
    pub async fn mark_sink_delivered(&self, id: i64, sink_name: &str) -> Result<(), sqlx::Error> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT delivered_sinks FROM scrobbles WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let mut set: Vec<String> =
            row.and_then(|(s,)| s).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if !set.iter().any(|s| s == sink_name) {
            set.push(sink_name.to_string());
            sqlx::query("UPDATE scrobbles SET delivered_sinks = ? WHERE id = ?")
                .bind(serde_json::to_string(&set).unwrap_or_else(|_| "[]".into()))
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Persist MusicBrainz IDs onto a stored listen after the engine enriches it
    /// (enrichment moved off the ingest request path — see server/ingest.rs).
    pub async fn update_scrobble_enrichment(
        &self,
        id: i64,
        play: &Play,
    ) -> Result<(), sqlx::Error> {
        let mbid_artist_json =
            play.mbid_artist.as_ref().map(|a| serde_json::to_string(a).unwrap_or_default());
        sqlx::query(
            "UPDATE scrobbles SET mbid_recording = ?, mbid_release = ?, mbid_artist = ?, caa_id = ?, caa_release_mbid = ?
             WHERE id = ?"
        )
        .bind(&play.mbid_recording).bind(&play.mbid_release).bind(&mbid_artist_json)
        .bind(play.caa_id).bind(&play.caa_release_mbid).bind(id)
        .execute(&self.pool).await?;
        Ok(())
    }

    /// Mark a scrobble fully synced (delivered to every enabled sink).
    pub async fn mark_scrobble_synced(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE scrobbles SET status = 'synced' WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Lightweight liveness check for the DB connection (used by /health).
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    /// Write a consistent, compacted snapshot of the whole database to `path`
    /// (`VACUUM INTO`), safe to run while the server is live — including under
    /// WAL. The path must be server-controlled; single quotes are escaped since
    /// VACUUM INTO takes a string literal, not a bound parameter.
    pub async fn backup_to(&self, path: &str) -> Result<(), sqlx::Error> {
        let safe = path.replace('\'', "''");
        sqlx::query(&format!("VACUUM INTO '{}'", safe)).execute(&self.pool).await?;
        Ok(())
    }

    // ════════════════════════════════════════════════════════════
    //  Scrobble Reads
    // ════════════════════════════════════════════════════════════

    pub async fn get_recent_scrobbles(
        &self,
        user_id: i64,
        limit: i64,
        offset: i64,
        artist_filter: Option<&str>,
        album_filter: Option<&str>,
        track_filter: Option<&str>,
        after: Option<i64>,
        before: Option<i64>,
    ) -> Result<Vec<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE user_id = ? AND skipped = FALSE
               AND (? IS NULL OR LOWER(artist) LIKE '%' || LOWER(?) || '%')
               AND (? IS NULL OR LOWER(album) LIKE '%' || LOWER(?) || '%')
               AND (? IS NULL OR LOWER(title) LIKE '%' || LOWER(?) || '%')
               AND (? IS NULL OR timestamp >= ?)
               AND (? IS NULL OR timestamp <= ?)
             ORDER BY timestamp DESC LIMIT ? OFFSET ?",
        )
        .bind(user_id)
        .bind(artist_filter)
        .bind(artist_filter)
        .bind(album_filter)
        .bind(album_filter)
        .bind(track_filter)
        .bind(track_filter)
        .bind(after)
        .bind(after)
        .bind(before)
        .bind(before)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    /// Search a user's own listening history by name.
    ///
    /// Deliberately over `scrobbles` rather than `entities`: entities only exist
    /// for things already annotated or loved, so searching them would find only
    /// what you've already been to. Grouped rather than a flat row list because
    /// "that Radiohead album" is an album, not fifty listens of it.
    #[allow(clippy::type_complexity)]
    pub async fn search_library(
        &self,
        user_id: i64,
        q: &str,
        limit: i64,
    ) -> Result<
        (
            Vec<(String, i64)>,
            Vec<(String, String, i64)>,
            Vec<(String, String, Option<String>, i64)>,
        ),
        sqlx::Error,
    > {
        let needle = q.trim().to_lowercase();
        if needle.is_empty() {
            return Ok((Vec::new(), Vec::new(), Vec::new()));
        }

        let artists: Vec<(String, i64)> = sqlx::query_as(
            "SELECT artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(artist) LIKE '%' || ? || '%'
             GROUP BY LOWER(artist) ORDER BY plays DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(&needle)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let albums: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT album, artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND album IS NOT NULL AND album != ''
               AND LOWER(album) LIKE '%' || ? || '%'
             GROUP BY LOWER(album), LOWER(artist) ORDER BY plays DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(&needle)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let tracks: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
            "SELECT title, artist, album, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(title) LIKE '%' || ? || '%'
             GROUP BY LOWER(title), LOWER(artist) ORDER BY plays DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(&needle)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok((artists, albums, tracks))
    }

    // ════════════════════════════════════════════════════════════
    //  Album & artist pages (UX 4)
    //
    //  All of these aggregate over `scrobbles` matched by *name*, not MBID.
    //  Most of a library is un-enriched and those are exactly the records worth
    //  a page, so an MBID join would leave the interesting ones blank. Every
    //  count filters `skipped = FALSE` like the rest of the counting queries.
    // ════════════════════════════════════════════════════════════

    /// Headline numbers for one album: plays, seconds, distinct tracks, first
    /// and last heard, and the newest release year seen.
    #[allow(clippy::type_complexity)]
    pub async fn album_summary(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<Option<AlbumSummary>, sqlx::Error> {
        let row: Option<AlbumSummary> = sqlx::query_as(
            "SELECT COUNT(*) AS plays,
                    COALESCE(SUM(duration), 0) AS seconds,
                    COUNT(DISTINCT LOWER(TRIM(title))) AS track_count,
                    MIN(timestamp) AS first_heard,
                    MAX(timestamp) AS last_heard,
                    MAX(release_year) AS year
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
               AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?",
        )
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.filter(|r| r.plays > 0))
    }

    /// One row per distinct track on the album, most played first.
    ///
    /// The quality columns come from the *most recent* listen of that track
    /// rather than an average: "what it sounds like when I play it now" is the
    /// useful answer, and averaging a codec is meaningless anyway.
    pub async fn album_tracks(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<Vec<TrackAggregate>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title,
                    COUNT(*) AS plays,
                    MAX(duration) AS duration,
                    MIN(track_number) AS track_number,
                    NULL AS album,
                    (SELECT s2.format_type FROM scrobbles s2 WHERE s2.user_id = s.user_id
                       AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                       AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                       AND s2.skipped = FALSE ORDER BY s2.timestamp DESC LIMIT 1) AS format_type,
                    (SELECT s2.codec FROM scrobbles s2 WHERE s2.user_id = s.user_id
                       AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                       AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                       AND s2.skipped = FALSE ORDER BY s2.timestamp DESC LIMIT 1) AS codec,
                    (SELECT s2.bit_depth FROM scrobbles s2 WHERE s2.user_id = s.user_id
                       AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                       AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                       AND s2.skipped = FALSE ORDER BY s2.timestamp DESC LIMIT 1) AS bit_depth,
                    (SELECT s2.sample_rate FROM scrobbles s2 WHERE s2.user_id = s.user_id
                       AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                       AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                       AND s2.skipped = FALSE ORDER BY s2.timestamp DESC LIMIT 1) AS sample_rate,
                    (SELECT s2.dsd_multiplier FROM scrobbles s2 WHERE s2.user_id = s.user_id
                       AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                       AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                       AND s2.skipped = FALSE ORDER BY s2.timestamp DESC LIMIT 1) AS dsd_multiplier,
                    (SELECT s2.delivery_codec FROM scrobbles s2 WHERE s2.user_id = s.user_id
                       AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                       AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                       AND s2.skipped = FALSE ORDER BY s2.timestamp DESC LIMIT 1) AS delivery_codec,
                    (SELECT s2.is_lossless FROM scrobbles s2 WHERE s2.user_id = s.user_id
                       AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                       AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                       AND s2.skipped = FALSE ORDER BY s2.timestamp DESC LIMIT 1) AS is_lossless
             FROM scrobbles s
             WHERE user_id = ? AND skipped = FALSE
               AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?
             GROUP BY LOWER(TRIM(title))
             ORDER BY plays DESC, track_number ASC",
        )
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .fetch_all(&self.pool)
        .await
    }

    /// This artist's albums, most played first. `exclude` drops the one you're
    /// already looking at, for the "More from …" strip.
    pub async fn artist_albums(
        &self,
        user_id: i64,
        artist: &str,
        exclude: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AlbumAggregate>, sqlx::Error> {
        sqlx::query_as(
            "SELECT album AS name, COUNT(*) AS plays, MAX(release_year) AS year,
                    MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                    MAX(caa_release_mbid) AS caa_release_mbid
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(TRIM(artist)) = ?
               AND album IS NOT NULL AND TRIM(album) != ''
               AND (? IS NULL OR LOWER(TRIM(album)) != ?)
             GROUP BY LOWER(TRIM(album))
             ORDER BY plays DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(exclude.map(|e| e.trim().to_lowercase()))
        .bind(exclude.map(|e| e.trim().to_lowercase()))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Headline numbers for one artist, plus their share of everything.
    pub async fn artist_summary(
        &self,
        user_id: i64,
        artist: &str,
    ) -> Result<Option<ArtistSummary>, sqlx::Error> {
        let row: Option<ArtistSummary> = sqlx::query_as(
            "SELECT COUNT(*) AS plays,
                    COALESCE(SUM(duration), 0) AS seconds,
                    COUNT(DISTINCT LOWER(TRIM(title))) AS track_count,
                    COUNT(DISTINCT CASE WHEN album IS NOT NULL AND TRIM(album) != ''
                          THEN LOWER(TRIM(album)) END) AS album_count,
                    MIN(timestamp) AS first_heard,
                    MAX(timestamp) AS last_heard
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(TRIM(artist)) = ?",
        )
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.filter(|r| r.plays > 0))
    }

    /// Where this artist sits in the user's all-time ranking, and how many
    /// artists they've heard at all (so the UI can say "#3 of 412").
    pub async fn artist_rank(&self, user_id: i64, artist: &str) -> Result<(i64, i64), sqlx::Error> {
        // Ranking groups case-insensitively on purpose: "LINKIN PARK" and
        // "Linkin Park" are one artist to the charts, so they must be one here.
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             GROUP BY LOWER(TRIM(artist)) ORDER BY plays DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let needle = artist.trim().to_lowercase();
        let rank = rows
            .iter()
            .position(|(a, _)| a.trim().to_lowercase() == needle)
            .map(|i| i as i64 + 1)
            .unwrap_or(0);
        Ok((rank, rows.len() as i64))
    }

    /// This artist's most played tracks, with the album each was heard on.
    pub async fn artist_top_tracks(
        &self,
        user_id: i64,
        artist: &str,
        limit: i64,
    ) -> Result<Vec<TrackAggregate>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title,
                    COUNT(*) AS plays,
                    MAX(duration) AS duration,
                    MIN(track_number) AS track_number,
                    MAX(album) AS album,
                    MAX(format_type) AS format_type,
                    MAX(codec) AS codec,
                    MAX(bit_depth) AS bit_depth,
                    MAX(sample_rate) AS sample_rate,
                    MAX(dsd_multiplier) AS dsd_multiplier,
                    MAX(delivery_codec) AS delivery_codec,
                    MAX(is_lossless) AS is_lossless
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(TRIM(artist)) = ?
             GROUP BY LOWER(TRIM(title))
             ORDER BY plays DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Plays per calendar year for the artist trend bars. Years are formatted
    /// with SQLite's own `strftime` so they follow the stored UTC timestamp;
    /// this is a shape, not a precise local-time histogram.
    pub async fn artist_trend(
        &self,
        user_id: i64,
        artist: &str,
    ) -> Result<Vec<(String, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT strftime('%Y', timestamp, 'unixepoch') AS year, COUNT(*) AS plays
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(TRIM(artist)) = ?
             GROUP BY year ORDER BY year ASC",
        )
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .fetch_all(&self.pool)
        .await
    }

    /// Total non-skipped listens, for "share of your listening".
    /// How many times this user has played anything by one artist.
    ///
    /// Grouped case-insensitively for the same reason `artist_rank` is: "LINKIN
    /// PARK" and "Linkin Park" are one artist to every chart, so a share
    /// computed against one spelling would be wrong for the other.
    pub async fn artist_plays(&self, user_id: i64, artist: &str) -> Result<i64, sqlx::Error> {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND LOWER(TRIM(artist)) = ?",
        )
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    pub async fn total_plays(&self, user_id: i64) -> Result<i64, sqlx::Error> {
        let (n,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(n)
    }

    /// Best artwork known for an album — used by the album hero and the cards.
    pub async fn album_artwork(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<Option<(Option<String>, Option<i64>, Option<String>)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT artwork_url, caa_id, caa_release_mbid FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
               AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?
               AND (artwork_url IS NOT NULL AND artwork_url != '' OR caa_id > 0)
             ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await
    }

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

    /// Albums where the listens disagree about which release they are.
    ///
    /// Returns (artist, album, total plays, distinct release MBIDs, the winning
    /// MBID, how many rows carry it).
    pub async fn albums_with_mixed_releases(
        &self,
        user_id: i64,
    ) -> Result<Vec<MixedAlbum>, sqlx::Error> {
        // One flat query, grouped in Rust. The previous version did the
        // majority vote in SQL with correlated subqueries and could only report
        // the winner; per-edit review needs every candidate and its play count,
        // because deciding whether a 4-of-29 plurality is trustworthy is
        // exactly the judgement being handed to the operator.
        let rows: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
            "SELECT artist, album, mbid_release, COUNT(*) AS n
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND album IS NOT NULL AND TRIM(album) != ''
              GROUP BY LOWER(TRIM(artist)), LOWER(TRIM(album)), mbid_release
              ORDER BY n DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        // Key on the case-folded pair; keep the first spelling seen for display.
        let mut groups: HashMap<(String, String), MixedAlbum> = HashMap::new();
        for (artist, album, mbid, n) in rows {
            let key = (artist.trim().to_lowercase(), album.trim().to_lowercase());
            let entry = groups.entry(key).or_insert_with(|| MixedAlbum {
                artist: artist.clone(),
                album: album.clone(),
                plays: 0,
                variants: 0,
                winner: None,
                winner_plays: 0,
                untagged: 0,
                candidates: Vec::new(),
            });
            entry.plays += n;
            match mbid.filter(|m| !m.trim().is_empty()) {
                Some(m) => {
                    entry.variants += 1;
                    // Rows arrive most-played first, so the first is the winner.
                    if entry.winner.is_none() {
                        entry.winner = Some(m.clone());
                        entry.winner_plays = n;
                    }
                    entry.candidates.push(ReleaseCandidate { mbid: m, plays: n });
                }
                // Listens with no release at all. Unifying stamps these too,
                // which is worth showing rather than burying in the total.
                None => entry.untagged += n,
            }
        }

        let mut out: Vec<MixedAlbum> = groups.into_values().filter(|g| g.variants > 1).collect();
        out.sort_by(|a, b| b.plays.cmp(&a.plays));
        Ok(out)
    }

    /// Force every listen of one album onto a single release MBID.
    ///
    /// Artwork is cleared alongside it — the resolved cover belongs to the
    /// release being replaced, and `artwork_url`/`caa_id` are the queue keys for
    /// the backfill, so nulling them is what re-fetches the right one.
    /// Returns how many rows changed.
    pub async fn unify_album_release(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
        mbid_release: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE scrobbles
                SET mbid_release = ?, artwork_url = NULL, caa_id = NULL, caa_release_mbid = NULL
              WHERE user_id = ?
                AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?
                AND (mbid_release IS NULL OR mbid_release != ?)",
        )
        .bind(mbid_release)
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .bind(mbid_release)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Artist strings that carry a featured credit, with the primary artist they
    /// would fold into and how many listens are affected.
    ///
    /// `Cœur de pirate feat. Loud` is a *different artist* to every query in the
    /// app, so one track drops out of its own album and off its own artist page.
    /// Only the explicit `feat.` / `ft.` / `featuring` / `with` forms are
    /// matched — `&` and `,` are left alone, because "Simon & Garfunkel" and
    /// "Earth, Wind & Fire" are names, not credits.
    pub async fn artists_with_featured_credits(
        &self,
        user_id: i64,
    ) -> Result<Vec<FeaturedArtist>, sqlx::Error> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             GROUP BY LOWER(TRIM(artist)) ORDER BY plays DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(artist, plays)| {
                let primary = split_featured(&artist)?;
                Some(FeaturedArtist { artist, primary, plays })
            })
            .collect())
    }

    /// Artist names that differ only in punctuation or spacing.
    ///
    /// "A.R. Rahman" and "A. R. Rahman" are one person but two rows in every
    /// grouping query, so they appear twice in the same top-50 and split their
    /// own play count. Grouped by a key with all punctuation and whitespace
    /// removed; the spelling with the most plays wins, on the theory that the
    /// one you've scrobbled most is the one your library actually uses.
    ///
    /// Only *variants of one name* are matched — this can't merge two artists
    /// who are genuinely different, because the normalised forms would differ.
    pub async fn artists_with_spelling_variants(
        &self,
        user_id: i64,
    ) -> Result<Vec<VariantArtist>, sqlx::Error> {
        // Grouped by the **exact** string, not by `LOWER(TRIM(artist))`.
        //
        // That was the bug: the Rust grouping below also lowercases, so
        // anything it would merge on case alone, the SQL had already merged —
        // SQLite returned one arbitrarily-chosen spelling per lowercased name
        // and the group came back with a single member. Case-only variants
        // were therefore invisible *by construction*: on a 21k-listen history
        // this reported nothing while thirteen real groups sat in the data.
        // The half that worked — punctuation and spacing, "A.R. Rahman"
        // against "A R Rahman" — survives the collapse, which is why it
        // looked correct.
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             GROUP BY artist ORDER BY plays DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut groups: HashMap<String, Vec<(String, i64)>> = HashMap::new();
        for (artist, plays) in rows {
            groups.entry(normalise_artist(&artist)).or_default().push((artist, plays));
        }

        let mut out: Vec<VariantArtist> = groups
            .into_values()
            .filter(|g| g.len() > 1)
            .map(|mut g| {
                // Already ordered by plays from the query; be explicit anyway.
                g.sort_by(|a, b| b.1.cmp(&a.1));
                let (primary, primary_plays) = g[0].clone();
                // Whether these differ by anything a reader would notice.
                // Most read paths already lowercase — `chart()` groups on
                // `LOWER(TRIM(artist))`, `entity_key` lowercases — so a
                // case-only group has correct counts and merely displays
                // inconsistently. Flagged rather than hidden: it is a real
                // variant, just a cosmetic one, and burying it with the
                // count-splitting kind would make the list unreadable.
                let case_only = g.iter().all(|(n, _)| n.to_lowercase() == primary.to_lowercase());
                VariantArtist {
                    primary,
                    primary_plays,
                    variants: g[1..].iter().map(|(n, p)| (n.clone(), *p)).collect(),
                    case_only,
                }
            })
            .collect();
        // The ones that actually split a play count first; cosmetic ones last.
        out.sort_by(|a, b| {
            a.case_only.cmp(&b.case_only).then(b.primary_plays.cmp(&a.primary_plays))
        });
        Ok(out)
    }

    // ════════════════════════════════════════════════════════════
    //  Physical media — records and tapes
    // ════════════════════════════════════════════════════════════

    pub async fn list_physical(
        &self,
        user_id: i64,
        kind: Option<&str>,
    ) -> Result<Vec<PhysicalRelease>, sqlx::Error> {
        sqlx::query_as(
            "SELECT r.*,
                    (SELECT COUNT(*) FROM physical_plays p WHERE p.release_id = r.id) AS play_count,
                    (SELECT COALESCE(SUM(p.duration_secs), 0) FROM physical_plays p
                      WHERE p.release_id = r.id) AS played_secs,
                    (SELECT MAX(p.played_at) FROM physical_plays p WHERE p.release_id = r.id)
                      AS last_played,
                    (SELECT COUNT(*) FROM physical_images i WHERE i.release_id = r.id)
                      AS image_count
               FROM physical_releases r
              WHERE r.user_id = ? AND (? IS NULL OR r.kind = ?)
              ORDER BY r.artist COLLATE NOCASE, r.title COLLATE NOCASE",
        )
        .bind(user_id)
        .bind(kind)
        .bind(kind)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_physical(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<PhysicalRelease>, sqlx::Error> {
        sqlx::query_as(
            "SELECT r.*,
                    (SELECT COUNT(*) FROM physical_plays p WHERE p.release_id = r.id) AS play_count,
                    (SELECT COALESCE(SUM(p.duration_secs), 0) FROM physical_plays p
                      WHERE p.release_id = r.id) AS played_secs,
                    (SELECT MAX(p.played_at) FROM physical_plays p WHERE p.release_id = r.id)
                      AS last_played,
                    (SELECT COUNT(*) FROM physical_images i WHERE i.release_id = r.id)
                      AS image_count
               FROM physical_releases r WHERE r.user_id = ? AND r.id = ?",
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_physical(
        &self,
        user_id: i64,
        kind: &str,
        artist: &str,
        title: &str,
        year: Option<i64>,
        mbid_release: Option<&str>,
        discogs_id: Option<&str>,
        barcode: Option<&str>,
        catalog_number: Option<&str>,
        artwork_url: Option<&str>,
        tracks_json: &str,
        notes: Option<&str>,
        purchased_at: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        let res = sqlx::query(
            "INSERT INTO physical_releases
                (user_id, kind, artist, title, year, mbid_release, discogs_id, barcode,
                 catalog_number, artwork_url, tracks, notes, added_at, purchased_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(kind)
        .bind(artist)
        .bind(title)
        .bind(year)
        .bind(mbid_release)
        .bind(discogs_id)
        .bind(barcode)
        .bind(catalog_number)
        .bind(artwork_url)
        .bind(tracks_json)
        .bind(notes)
        .bind(Self::now_secs())
        .bind(purchased_at)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// Edit the shelf-keeping fields of a pressing: when it was bought, what it
    /// is normally played on, and the note beside it.
    ///
    /// Every field is `Option<Option<_>>` for the reason
    /// `POST /scrobbles/bulk-chain` learned the hard way: a bare `Option` cannot
    /// tell *absent* from *null*, so a typo'd key would read as "clear the
    /// chain" and quietly wipe an attribution while answering 200. Absent leaves
    /// the column alone; an explicit null clears it, which is how the picker's
    /// "—" option works.
    pub async fn update_physical(
        &self,
        user_id: i64,
        id: i64,
        purchased_at: Option<Option<i64>>,
        chain_id: Option<Option<i64>>,
        equipment_id: Option<Option<i64>>,
        notes: Option<Option<&str>>,
        kind: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let mut sets: Vec<&str> = Vec::new();
        // A plain `Option` rather than a nested one, unlike everything else
        // here: `kind` carries a CHECK constraint and has no "cleared" state to
        // tell apart from absent, so `None` can only mean "not mentioned".
        if kind.is_some() {
            sets.push("kind = ?");
        }
        if purchased_at.is_some() {
            sets.push("purchased_at = ?");
        }
        if chain_id.is_some() {
            sets.push("chain_id = ?");
        }
        if equipment_id.is_some() {
            sets.push("equipment_id = ?");
        }
        if notes.is_some() {
            sets.push("notes = ?");
        }
        if sets.is_empty() {
            // Nothing asked for is not a failure — but it must not report a
            // 404 either, so confirm the row is the caller's and stop.
            return Ok(self.get_physical(user_id, id).await?.is_some());
        }
        let sql = format!(
            "UPDATE physical_releases SET {} WHERE user_id = ? AND id = ?",
            sets.join(", ")
        );
        let mut q = sqlx::query(&sql);
        // Bound in the same order the SET clauses were pushed.
        if let Some(v) = kind {
            q = q.bind(v.to_string());
        }
        if let Some(v) = purchased_at {
            q = q.bind(v);
        }
        if let Some(v) = chain_id {
            q = q.bind(v);
        }
        if let Some(v) = equipment_id {
            q = q.bind(v);
        }
        if let Some(v) = notes {
            q = q.bind(v.map(|s| s.to_string()));
        }
        let res = q.bind(user_id).bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    // ── What is on the deck ──

    /// The side currently on this user's deck, if any.
    pub async fn get_spin(&self, user_id: i64) -> Result<Option<PhysicalSpin>, sqlx::Error> {
        sqlx::query_as::<_, PhysicalSpin>("SELECT * FROM physical_spins WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Put a side on the deck, or update the one that is on it.
    ///
    /// An upsert on `user_id` rather than an insert, because the primary key is
    /// the whole rule: one deck per person. Starting a second side replaces the
    /// first rather than erroring, which matches what actually happened in the
    /// room — you took the record off.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_spin(
        &self,
        user_id: i64,
        release_id: i64,
        side: &str,
        started_at: i64,
        paused_at: Option<i64>,
        paused_total: i64,
        outcomes_json: &str,
        chain_id: Option<i64>,
        equipment_id: Option<i64>,
        start_secs: i64,
    ) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO physical_spins
               (user_id, release_id, side, started_at, paused_at, paused_total,
                outcomes, chain_id, equipment_id, start_secs, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET
               release_id = excluded.release_id,
               side = excluded.side,
               started_at = excluded.started_at,
               paused_at = excluded.paused_at,
               paused_total = excluded.paused_total,
               outcomes = excluded.outcomes,
               chain_id = excluded.chain_id,
               equipment_id = excluded.equipment_id,
               start_secs = excluded.start_secs,
               updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(release_id)
        .bind(side)
        .bind(started_at)
        .bind(paused_at)
        .bind(paused_total)
        .bind(outcomes_json)
        .bind(chain_id)
        .bind(equipment_id)
        .bind(start_secs)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remember where a tape was left — and refuse to for anything else.
    ///
    /// A cassette is sequential. Stop it halfway through side A and eject, and
    /// the tape is still sitting at that point: put it back in tomorrow, press
    /// play, and it carries on from there. Nothing else on this shelf works
    /// that way, and the asymmetry is the design rather than an omission:
    ///
    /// * A **record** has random access and no state. You lift the arm and it
    ///   goes back in its sleeve; there is no "where it was left" to remember,
    ///   and inventing one would be modelling physics the object does not have.
    /// * A **disc** is random access too, and every player on earth starts it
    ///   at track one.
    ///
    /// So this writes nothing unless the release is a cassette. The guard is
    /// here rather than at the call site because it is a fact about the medium,
    /// and a second caller would otherwise have to remember it.
    pub async fn park_tape(
        &self,
        user_id: i64,
        release_id: i64,
        side: &str,
        secs: i64,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE physical_releases SET parked_side = ?, parked_secs = ?
               WHERE user_id = ? AND id = ? AND kind = 'cassette'",
        )
        .bind(side)
        .bind(secs.max(0))
        .bind(user_id)
        .bind(release_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Wind a tape back to the start.
    pub async fn rewind_tape(&self, user_id: i64, release_id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE physical_releases SET parked_side = NULL, parked_secs = NULL
               WHERE user_id = ? AND id = ?",
        )
        .bind(user_id)
        .bind(release_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Take whatever is on the deck off it.
    pub async fn clear_spin(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM physical_spins WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ── Scans: sleeves, booklets, labels ──

    pub async fn list_physical_images(
        &self,
        user_id: i64,
        release_id: i64,
    ) -> Result<Vec<PhysicalImage>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM physical_images
              WHERE user_id = ? AND release_id = ?
              ORDER BY position ASC, id ASC",
        )
        .bind(user_id)
        .bind(release_id)
        .fetch_all(&self.pool)
        .await
    }

    /// One image, scoped to its owner — the guard that stops ids being walked
    /// to pull somebody else's scans through the file endpoint.
    pub async fn get_physical_image(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<PhysicalImage>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM physical_images WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map(Some)
            .or_else(|e| if matches!(e, sqlx::Error::RowNotFound) { Ok(None) } else { Err(e) })
    }

    /// Store one image. A provider's own id makes this idempotent: re-fetching
    /// a release updates the rows it already holds rather than stacking a
    /// second copy of all nineteen booklet pages on top of the first.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_physical_image(
        &self,
        user_id: i64,
        release_id: i64,
        kind: &str,
        provider: &str,
        url: Option<&str>,
        thumb_url: Option<&str>,
        file_path: Option<&str>,
        mime: Option<&str>,
        bytes: Option<i64>,
        caption: Option<&str>,
        position: i64,
        remote_id: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query(
            "INSERT INTO physical_images
                (user_id, release_id, kind, provider, url, thumb_url, file_path, mime,
                 bytes, caption, position, remote_id, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             -- The `WHERE` is part of the conflict target, not decoration:
             -- SQLite matches an upsert against a *partial* unique index only
             -- when the predicate is repeated here, and rejects the statement
             -- outright otherwise. It is also what leaves uploads alone — they
             -- carry no provider id, so two scans of the same page really are
             -- two scans.
             ON CONFLICT(release_id, provider, remote_id) WHERE remote_id IS NOT NULL
             DO UPDATE SET
                kind = excluded.kind, url = excluded.url, thumb_url = excluded.thumb_url,
                caption = excluded.caption, position = excluded.position",
        )
        .bind(user_id)
        .bind(release_id)
        .bind(kind)
        .bind(provider)
        .bind(url)
        .bind(thumb_url)
        .bind(file_path)
        .bind(mime)
        .bind(bytes)
        .bind(caption)
        .bind(position)
        .bind(remote_id)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        // `last_insert_rowid` is meaningless after an upsert that updated, so
        // the id is read back by the key that decided it.
        let row: (i64,) = sqlx::query_as(
            "SELECT id FROM physical_images
              WHERE release_id = ? AND provider = ?
                AND (remote_id IS ? OR (remote_id IS NULL AND ? IS NULL))
              ORDER BY id DESC LIMIT 1",
        )
        .bind(release_id)
        .bind(provider)
        .bind(remote_id)
        .bind(remote_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn update_physical_image(
        &self,
        user_id: i64,
        id: i64,
        kind: Option<&str>,
        caption: Option<Option<&str>>,
        position: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let mut sets: Vec<&str> = Vec::new();
        if kind.is_some() {
            sets.push("kind = ?");
        }
        if caption.is_some() {
            sets.push("caption = ?");
        }
        if position.is_some() {
            sets.push("position = ?");
        }
        if sets.is_empty() {
            return Ok(self.get_physical_image(user_id, id).await?.is_some());
        }
        let sql =
            format!("UPDATE physical_images SET {} WHERE user_id = ? AND id = ?", sets.join(", "));
        let mut q = sqlx::query(&sql);
        if let Some(v) = kind {
            q = q.bind(v.to_string());
        }
        if let Some(v) = caption {
            q = q.bind(v.map(|s| s.to_string()));
        }
        if let Some(v) = position {
            q = q.bind(v);
        }
        let res = q.bind(user_id).bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete an image row and hand back the file it was holding, if any, so
    /// the caller can remove it from disk. The row goes first: a file left
    /// behind is litter, whereas a row pointing at a file that is gone renders
    /// as a broken image on the page.
    pub async fn delete_physical_image(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<Option<String>>, sqlx::Error> {
        let Some(img) = self.get_physical_image(user_id, id).await? else {
            return Ok(None);
        };
        sqlx::query("DELETE FROM physical_images WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(Some(img.file_path))
    }

    /// Replace the stored tracklist — how a hand-corrected side split is saved.
    pub async fn set_physical_tracks(
        &self,
        user_id: i64,
        id: i64,
        tracks_json: &str,
    ) -> Result<bool, sqlx::Error> {
        let res =
            sqlx::query("UPDATE physical_releases SET tracks = ? WHERE user_id = ? AND id = ?")
                .bind(tracks_json)
                .bind(user_id)
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Remove a pressing. Returns the uploaded scans it was holding so the
    /// caller can take them off disk — nothing else knows those files exist,
    /// so skipping this leaves them orphaned forever.
    pub async fn delete_physical(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<Vec<String>>, sqlx::Error> {
        let files: Vec<(String,)> = sqlx::query_as(
            "SELECT file_path FROM physical_images
              WHERE user_id = ? AND release_id = ? AND file_path IS NOT NULL",
        )
        .bind(user_id)
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        // Plays and images both reference the release, and foreign keys are on.
        sqlx::query("DELETE FROM physical_images WHERE user_id = ? AND release_id = ?")
            .bind(user_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM physical_plays WHERE user_id = ? AND release_id = ?")
            .bind(user_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        let res = sqlx::query("DELETE FROM physical_releases WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok((res.rows_affected() > 0).then(|| files.into_iter().map(|(f,)| f).collect()))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_physical_play(
        &self,
        user_id: i64,
        release_id: i64,
        side: &str,
        played_at: i64,
        duration_secs: i64,
        track_count: i64,
        equipment_id: Option<i64>,
        chain_id: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        let res = sqlx::query(
            "INSERT INTO physical_plays
                (user_id, release_id, side, played_at, duration_secs, track_count,
                 equipment_id, chain_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(release_id)
        .bind(side)
        .bind(played_at)
        .bind(duration_secs)
        .bind(track_count)
        .bind(equipment_id)
        .bind(chain_id)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    /// Hours on a stylus or tape head, summed from the sides actually played
    /// through it.
    ///
    /// Derived rather than accumulated, for the same reason chain hours are:
    /// a counter that is incremented can drift from the events it claims to
    /// count, and there is no way to tell which is wrong afterwards.
    pub async fn equipment_playback_hours(
        &self,
        user_id: i64,
        equipment_id: i64,
    ) -> Result<(f64, i64), sqlx::Error> {
        let row: (Option<i64>, i64) = sqlx::query_as(
            "SELECT SUM(duration_secs), COUNT(*) FROM physical_plays
              WHERE user_id = ? AND equipment_id = ?",
        )
        .bind(user_id)
        .bind(equipment_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((row.0.unwrap_or(0) as f64 / 3600.0, row.1))
    }

    // ── Server-wide settings ──

    /// Read an admin-set server setting, decrypted.
    pub async fn get_app_setting(&self, key: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM app_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|r| self.secrets.decrypt(&r.0).ok()).filter(|v| !v.trim().is_empty()))
    }

    /// Store (or clear) a server setting. Encrypted at rest like every other
    /// credential that has to be replayed.
    pub async fn set_app_setting(&self, key: &str, value: &str) -> Result<(), sqlx::Error> {
        if value.trim().is_empty() {
            sqlx::query("DELETE FROM app_settings WHERE key = ?")
                .bind(key)
                .execute(&self.pool)
                .await?;
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(
            self.secrets
                .encrypt(value.trim())
                .map_err(|e| sqlx::Error::Protocol(format!("encrypting setting: {e}")))?,
        )
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Tracks recorded under two different artists on the same record.
    ///
    /// The shape this exists for is a film soundtrack: Plex files the album
    /// under the music director but tags each track with its singers, and until
    /// the ingest fix the live and history paths each picked a different one.
    /// The same play therefore landed twice under two names, and MusicBrainz
    /// then resolved those to two different recordings — so the plays, the
    /// artwork, the loves and the charts all split in half.
    ///
    /// The album artist wins, matched against the album's most common artist
    /// rather than assumed: it is how the record is filed, and picking the
    /// most-played spelling instead would hand the record to whichever singer
    /// happened to be on the tracks played most. Everything folded keeps its
    /// name in `artist_credit`, exactly as `fold_artist_credit` does.
    pub async fn tracks_with_split_attribution(
        &self,
        user_id: i64,
    ) -> Result<Vec<SplitAttribution>, sqlx::Error> {
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
            "SELECT title, album, artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
               AND album IS NOT NULL AND TRIM(album) != ''
             GROUP BY LOWER(TRIM(title)), LOWER(TRIM(album)), LOWER(TRIM(artist))",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        // The album's own artist is the one appearing on the most *distinct
        // tracks* of the record — not the one with the most plays.
        //
        // Breadth is the signal that separates a director from a singer: the
        // director is credited on every track, each singer on one or two. Play
        // count is not, and using it got this backwards on real data — on an
        // album where most tracks happened to be caught by the live path, one
        // singer's total outweighed the director's and the fold ran the wrong
        // way, moving "Harris Jayaraj" onto a singer. Total plays stays as the
        // tiebreak for artists of equal breadth.
        let mut album_tracks: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::new();
        let mut album_totals: HashMap<String, HashMap<String, i64>> = HashMap::new();
        for (title, album, artist, plays) in &rows {
            let key = album.trim().to_lowercase();
            album_tracks
                .entry(key.clone())
                .or_default()
                .entry(artist.clone())
                .or_default()
                .insert(title.trim().to_lowercase());
            *album_totals.entry(key).or_default().entry(artist.clone()).or_default() += plays;
        }

        let mut by_track: HashMap<(String, String), Vec<(String, i64)>> = HashMap::new();
        for (title, album, artist, plays) in rows {
            by_track
                .entry((title.trim().to_lowercase(), album.trim().to_lowercase()))
                .or_default()
                .push((artist, plays));
        }

        let mut out = Vec::new();
        for ((title_key, album_key), mut credits) in by_track {
            if credits.len() < 2 {
                continue;
            }
            let Some(totals) = album_totals.get(&album_key) else { continue };
            let breadth = |name: &str| -> usize {
                album_tracks.get(&album_key).and_then(|m| m.get(name)).map_or(0, |t| t.len())
            };
            // A compilation files *every* track under "Various Artists", so it
            // wins breadth on every record it appears on — and folding onto it
            // would replace real performers with a placeholder, which is the
            // exact damage this pass exists to undo. It can be folded *away
            // from*, never *onto*.
            if credits.iter().all(|(a, _)| is_various_artists(a)) {
                continue;
            }
            credits.retain(|(a, _)| !is_various_artists(a));
            if credits.len() < 2 {
                continue;
            }
            // Widest credit on the record first, then total plays, then plays
            // on this track.
            credits.sort_by(|a, b| {
                breadth(&b.0)
                    .cmp(&breadth(&a.0))
                    .then_with(|| {
                        totals.get(&b.0).unwrap_or(&0).cmp(totals.get(&a.0).unwrap_or(&0))
                    })
                    .then(b.1.cmp(&a.1))
            });
            let (primary, primary_plays) = credits[0].clone();
            let folding: Vec<(String, i64)> = credits[1..].to_vec();
            if folding.is_empty() {
                continue;
            }
            // Computed before the key is moved into the row.
            let primary_album_tracks = breadth(&primary) as i64;
            let primary_album_plays = *totals.get(&primary).unwrap_or(&0);
            out.push(SplitAttribution {
                title: title_key,
                album: album_key,
                primary_album_tracks,
                primary_album_plays,
                primary,
                primary_plays,
                folding,
            });
        }
        out.sort_by(|a, b| {
            let (x, y) = (
                b.folding.iter().map(|f| f.1).sum::<i64>(),
                a.folding.iter().map(|f| f.1).sum::<i64>(),
            );
            x.cmp(&y)
        });
        Ok(out)
    }

    /// Put one track's stray attribution back on the record's own artist.
    ///
    /// Matched case-insensitively on title+album+artist because that is how the
    /// preview grouped them. The folded name is kept in `artist_credit` — this
    /// is a credit being demoted, not a mistake being erased, and the singers
    /// are exactly the information the whole change exists to preserve.
    pub async fn merge_track_attribution(
        &self,
        user_id: i64,
        title: &str,
        album: &str,
        from_artist: &str,
        to_artist: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE scrobbles
                SET artist_credit = COALESCE(artist_credit, ? || '; ' || artist),
                    artist = ?,
                    mbid_recording = NULL,
                    artwork_url = NULL,
                    caa_id = NULL
              WHERE user_id = ?
                AND LOWER(TRIM(title)) = LOWER(TRIM(?))
                AND LOWER(TRIM(album)) = LOWER(TRIM(?))
                AND LOWER(TRIM(artist)) = LOWER(TRIM(?))",
        )
        .bind(to_artist)
        .bind(to_artist)
        .bind(user_id)
        .bind(title)
        .bind(album)
        .bind(from_artist)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Merge one spelling of an artist into another.
    ///
    /// A plain rename, unlike `fold_artist_credit`: both strings name the same
    /// artist, so there is no lost credit to preserve in `artist_credit`.
    pub async fn rename_artist(
        &self,
        user_id: i64,
        from: &str,
        to: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE scrobbles SET artist = ? WHERE user_id = ? AND LOWER(TRIM(artist)) = ?",
        )
        .bind(to)
        .bind(user_id)
        .bind(from.trim().to_lowercase())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Rewrite one artist string to another across a user's listens.
    ///
    /// The full credit is kept in `artist_credit` so nothing is lost — the
    /// featured performer is still recorded, it just stops fragmenting every
    /// grouping query.
    pub async fn fold_artist_credit(
        &self,
        user_id: i64,
        from: &str,
        to: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE scrobbles
                SET artist_credit = COALESCE(artist_credit, artist), artist = ?
              WHERE user_id = ? AND LOWER(TRIM(artist)) = ?",
        )
        .bind(to)
        .bind(user_id)
        .bind(from.trim().to_lowercase())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// (total listens, newest listened_at) for a user — the two numbers the
    /// ListenBrainz-compatible endpoints report alongside a listen page.
    pub async fn listen_count_and_latest(
        &self,
        user_id: i64,
    ) -> Result<(i64, Option<i64>), sqlx::Error> {
        let row: (i64, Option<i64>) = sqlx::query_as(
            "SELECT COUNT(*), MAX(timestamp) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Every listen a report needs, oldest first.
    ///
    /// One query and one pass, deliberately. A report card wants a dozen
    /// different aggregates per period — counts, distincts, top artist, an
    /// hour histogram, genre shares, repeat days — and the periods themselves
    /// depend on the user's timezone, which SQL here cannot bucket by. Doing it
    /// as a dozen queries per period would be hundreds of round trips for a
    /// year of months; doing it in Rust from one ordered scan is a few
    /// milliseconds.
    ///
    /// **Ascending**, unlike every other listing query, because "the first time
    /// you played this artist" is what makes a discovery a discovery — walking
    /// forwards means first-seen is simply the first time a name appears.
    pub async fn report_rows(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<ReportRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT timestamp, artist, title, album, duration, genres, chain_id
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             ORDER BY timestamp ASC LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Every listen the Playlist Lab needs to build its vector space.
    ///
    /// One scan, folded per track in Rust, for the same reason `report_rows`
    /// is: the hour-of-day axis has to be bucketed in the **user's** timezone
    /// and SQL here cannot do that, and the genre list is a JSON array that has
    /// to be joined to the Every Noise map through `genre_key` — which SQLite
    /// cannot compute either. Grouping in SQL would mean giving up both.
    ///
    /// `skipped = FALSE` like every other counting query: a skip says nothing
    /// about whether you want a track on a playlist.
    pub async fn playlist_lab_rows(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<crate::playlist_lab::LabRow>, sqlx::Error> {
        let rows: Vec<(i64, String, String, Option<String>, Option<String>, Option<i64>)> =
            sqlx::query_as(
                "SELECT timestamp, artist, title, album, genres, release_year
                   FROM scrobbles
                  WHERE user_id = ? AND skipped = FALSE
                  ORDER BY timestamp DESC LIMIT ?",
            )
            .bind(user_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|(timestamp, artist, title, album, genres, release_year)| {
                crate::playlist_lab::LabRow {
                    timestamp,
                    artist,
                    title,
                    album,
                    genres,
                    release_year,
                }
            })
            .collect())
    }

    /// Sides played by hand, for the "off the shelf" block.
    pub async fn physical_plays_in(
        &self,
        user_id: i64,
        from: i64,
        to: i64,
    ) -> Result<Vec<(String, String, String, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT r.title, r.artist, p.side, COUNT(*) AS spins
             FROM physical_plays p JOIN physical_releases r ON r.id = p.release_id
             WHERE p.user_id = ? AND p.played_at >= ? AND p.played_at < ?
             GROUP BY r.id, p.side ORDER BY spins DESC, r.title",
        )
        .bind(user_id)
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await
    }

    /// Every side ever put on, oldest first — what the shelf export carries.
    ///
    /// Ascending because an export is read as a log: a shelf's history is the
    /// order things were played in, and reversing it to match the history
    /// screen's newest-first habit would be the wrong shape for a file someone
    /// keeps.
    pub async fn all_physical_plays(
        &self,
        user_id: i64,
    ) -> Result<Vec<PhysicalPlayRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT p.id, p.release_id, r.artist, r.title, r.kind, p.side, p.played_at,
                    p.duration_secs, p.track_count,
                    e.name AS equipment_name, c.name AS chain_name
               FROM physical_plays p
               JOIN physical_releases r ON r.id = p.release_id
               LEFT JOIN equipment e ON e.id = p.equipment_id
               LEFT JOIN signal_chains c ON c.id = p.chain_id
              WHERE p.user_id = ?
              ORDER BY p.played_at ASC, p.id ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Every scan the user holds, in release then page order.
    pub async fn all_physical_images(
        &self,
        user_id: i64,
    ) -> Result<Vec<PhysicalImage>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM physical_images WHERE user_id = ?
              ORDER BY release_id ASC, position ASC, id ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Minimal columns for analytics: (timestamp, artist, title, album, track_number).
    /// Capped to the most-recent `limit` rows to bound memory on large histories.
    pub async fn get_listen_rows(
        &self,
        user_id: i64,
        after: Option<i64>,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, Option<String>, Option<i32>)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT timestamp, artist, title, album, track_number FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    // ── Language enrichment ──

    /// Listens still needing a language, newest first. (id, title, album, mbid_recording)
    pub async fn scrobbles_needing_language(
        &self,
        limit: i64,
    ) -> Result<Vec<(i64, String, Option<String>, Option<String>)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, title, album, mbid_recording FROM scrobbles
             WHERE language IS NULL AND skipped = FALSE ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn set_scrobble_language(&self, id: i64, language: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE scrobbles SET language = ? WHERE id = ?")
            .bind(language)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every genre the user has listened to, with play counts.
    ///
    /// One listen carries several tags, so the counts sum to more than the
    /// listen count — each tag is counted once per listen that carries it,
    /// which is what makes a weighted position meaningful.
    pub async fn genre_play_counts(
        &self,
        user_id: i64,
        after: Option<i64>,
    ) -> Result<Vec<(String, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT g.value AS name, COUNT(*) AS plays
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE AND s.genres IS NOT NULL
                AND (? IS NULL OR s.timestamp >= ?)
              GROUP BY LOWER(g.value) ORDER BY plays DESC",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await
    }

    /// `(genre, artist, plays)` for everything the user has listened to.
    ///
    /// The join to the genre map has to happen in Rust because matching is on
    /// `genre_key`, which SQLite cannot compute — so this hands over the raw
    /// triples. Bounded by distinct (artist, tag) pairs, a few thousand rows on
    /// a large history.
    ///
    /// It exists so "genres next door" can name music the user **already
    /// owns**. Pointing at anything else would make this a shopping list, which
    /// is explicitly not what Tapedeck is for.
    pub async fn artists_by_genre(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT g.value AS genre, s.artist, COUNT(*) AS plays
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE AND s.genres IS NOT NULL
              GROUP BY LOWER(g.value), s.artist
              ORDER BY plays DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Genre plays bucketed by year, for the listening trajectory.
    ///
    /// The year is computed in the caller's timezone rather than UTC — a listen
    /// at 00:30 on 1 January in CET belongs to the new year, and bucketing it
    /// in UTC would put it in the old one. Same reasoning as the dashboard's
    /// "today", which had exactly this bug.
    ///
    /// Returns `(period, genre, plays)`; the caller turns that into a position.
    /// `fmt` is a `strftime` pattern — `%Y` for years, `%Y-%m` for months.
    pub async fn genre_plays_by_period(
        &self,
        user_id: i64,
        tz_offset_hours: f64,
        fmt: &str,
    ) -> Result<Vec<(String, String, i64)>, sqlx::Error> {
        let shift = format!("{} hours", tz_offset_hours);
        sqlx::query_as(
            "SELECT strftime(?, s.timestamp, 'unixepoch', ?) AS yr, g.value AS name,
                    COUNT(*) AS plays
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE AND s.genres IS NOT NULL
              GROUP BY yr, LOWER(g.value)
              ORDER BY yr",
        )
        .bind(fmt)
        .bind(&shift)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Listens per year, and how many of them have genres resolved — so a
    /// trajectory can say which years are still filling in rather than drawing
    /// a confident line through a half-enriched history.
    pub async fn listens_by_period(
        &self,
        user_id: i64,
        tz_offset_hours: f64,
        fmt: &str,
    ) -> Result<Vec<(String, i64, i64)>, sqlx::Error> {
        let shift = format!("{} hours", tz_offset_hours);
        sqlx::query_as(
            "SELECT strftime(?, timestamp, 'unixepoch', ?) AS yr,
                    COUNT(*) AS listens,
                    SUM(CASE WHEN genres IS NOT NULL THEN 1 ELSE 0 END) AS enriched
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
              GROUP BY yr ORDER BY yr",
        )
        .bind(fmt)
        .bind(&shift)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// How many of the user's listens carry any genre at all, and how many
    /// exist — so a thin map can say "the backfill is still running" rather
    /// than looking like a small life.
    pub async fn genre_backfill_progress(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let done: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND genres IS NOT NULL",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let total: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;
        Ok((done.0, total.0))
    }

    /// Listens with no genres resolved yet.
    ///
    /// `genres = '[]'` is the tombstone for "asked, nobody had any" — the same
    /// discipline as `artwork_url = ''` and `caa_id = -1`, and the reason this
    /// tests `IS NULL` rather than emptiness.
    pub async fn scrobbles_needing_genres(
        &self,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, Option<String>)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, title, artist, album FROM scrobbles
             WHERE genres IS NULL AND skipped = FALSE ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Store a listen's genres. An empty slice writes `[]`, which is the
    /// tombstone for "asked, MusicBrainz had no tags".
    pub async fn set_scrobble_genres(&self, id: i64, genres: &[String]) -> Result<(), sqlx::Error> {
        // A JSON array rather than a delimited string, so the breakdown can
        // `json_each` over it — the same tool `get_equipment` uses to sum hours
        // across a chain's components. A comma-joined string would need a
        // recursive CTE to split, for no gain.
        let json = serde_json::to_string(genres).unwrap_or_else(|_| "[]".into());
        sqlx::query("UPDATE scrobbles SET genres = ? WHERE id = ?")
            .bind(&json)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Listens with an MBID but no resolved release year yet. Rows without an
    /// MBID are skipped: there's nothing to look them up with.
    pub async fn scrobbles_needing_release_year(
        &self,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, Option<String>)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, title, artist, album FROM scrobbles
             WHERE release_year IS NULL AND skipped = FALSE ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// `0` records "looked up, no date known" so the row isn't retried forever.
    ///
    /// Deliberately does **not** touch `caa_id`. It used to, on the reasoning
    /// that the same MusicBrainz lookup returned artwork anyway — but that
    /// silently pre-empted the artwork provider chain: setting `caa_id` removes
    /// the row from `scrobbles_needing_artwork`, so Deezer never got asked and
    /// the row ended up with a Cover Art Archive id but no `artwork_url`.
    /// Artwork is `backfill_artwork`'s job alone.
    pub async fn set_scrobble_release_year(&self, id: i64, year: i32) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE scrobbles SET release_year = ? WHERE id = ?")
            .bind(year)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// (total, with cover art) — drives the Settings artwork progress readout.
    pub async fn artwork_coverage(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let total: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;
        // Must match the total's population exactly. It didn't: the total
        // excluded skips and this didn't, so a history with skipped listens
        // that happened to have artwork reported more resolved than existed.
        let with_art: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND artwork_url IS NOT NULL",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((total.0, with_art.0))
    }

    /// The artwork queue: listens whose artwork has never been resolved.
    ///
    /// Keyed purely on `artwork_url` being NULL. It deliberately does *not* look
    /// at `caa_id`, which several other paths write (ingest `mbid_mapping`,
    /// flush-time enrichment) — using it here meant a row that had merely been
    /// through MusicBrainz was treated as "artwork done" and never got a URL.
    /// `artwork_url = ''` is the tombstone for "looked, found nothing".
    ///
    /// Returns any Cover Art Archive ids already on the row so the backfill can
    /// build a URL from them for free rather than looking anything up.
    #[allow(clippy::type_complexity)]
    pub async fn scrobbles_needing_artwork(
        &self,
        limit: i64,
    ) -> Result<
        Vec<(i64, String, String, Option<String>, Option<i64>, Option<String>, String)>,
        sqlx::Error,
    > {
        sqlx::query_as(
            "SELECT id, title, artist, album, caa_id, caa_release_mbid, source_name
             FROM scrobbles
             WHERE artwork_url IS NULL AND skipped = FALSE
             ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Record artwork for a listen. Pass `""` to tombstone a row whose artwork
    /// could not be found anywhere, so later passes skip it instead of re-asking
    /// every provider forever. `caa_id` is only written when a lookup produced
    /// one; it is provenance, not queue state.
    pub async fn set_scrobble_artwork(
        &self,
        id: i64,
        url: &str,
        caa_id: Option<i64>,
        caa_release_mbid: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE scrobbles SET artwork_url = ?,
                caa_id = COALESCE(?, caa_id),
                caa_release_mbid = COALESCE(?, caa_release_mbid)
             WHERE id = ?",
        )
        .bind(url)
        .bind(caa_id)
        .bind(caa_release_mbid)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// (total, with a language set) — for backfill progress.
    ///
    /// Both halves filter `skipped = FALSE` to match `scrobbles_needing_language`,
    /// which does. Counting rows the backfill will never process put them
    /// permanently in the denominator, so the progress readout could not reach
    /// 100% on any history containing a skip.
    pub async fn language_coverage(&self) -> Result<(i64, i64), sqlx::Error> {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE skipped = FALSE")
            .fetch_one(&self.pool)
            .await?;
        let done: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE skipped = FALSE AND language IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok((total.0, done.0))
    }

    /// Dashboard counters, bounded in the *user's* local time.
    ///
    /// `tz` and `week_start_day` come from their preferences. Both used to be
    /// implicit and both were wrong: "today" was midnight UTC, so the day
    /// rolled over at 01:00 in CET and 02:00 in CEST, and "this week" was a
    /// rolling 7×86400 window rather than a calendar week starting anywhere in
    /// particular.
    ///
    /// The fidelity shares divide by [`FIDELITY_KNOWN`], not by every play —
    /// see that constant for why.
    pub async fn get_dashboard_stats(
        &self,
        user_id: i64,
        tz: chrono_tz::Tz,
        week_start_day: u32,
    ) -> Result<serde_json::Value, sqlx::Error> {
        let now = Self::now_secs();
        let today_start = crate::analytics::local_day_start(now, tz);
        let week_start = crate::analytics::local_week_start(now, tz, week_start_day);

        let today: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND timestamp >= ?")
            .bind(user_id).bind(today_start).fetch_one(&self.pool).await?;
        let week: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND timestamp >= ?")
            .bind(user_id).bind(week_start).fetch_one(&self.pool).await?;
        let total: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;
        let lossless_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND is_lossless = TRUE")
            .bind(user_id).fetch_one(&self.pool).await?;
        let fidelity_known: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND {FIDELITY_KNOWN}"
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let lossless_pct = if fidelity_known.0 > 0 {
            (lossless_count.0 as f64 / fidelity_known.0 as f64 * 100.0).round() as i64
        } else {
            0
        };
        let top_artist: Option<(String, i64)> = sqlx::query_as(
            "SELECT artist, COUNT(*) as cnt FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND timestamp >= ?
              GROUP BY artist ORDER BY cnt DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(now - 30 * 86400)
        .fetch_optional(&self.pool)
        .await?;
        let unique_artists: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let unique_albums: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT album) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND album IS NOT NULL")
            .bind(user_id).fetch_one(&self.pool).await?;
        let unique_tracks: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT title || '|' || artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE")
            .bind(user_id).fetch_one(&self.pool).await?;
        let total_duration: (Option<i64>,) = sqlx::query_as("SELECT SUM(duration) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND duration IS NOT NULL")
            .bind(user_id).fetch_one(&self.pool).await?;
        let total_hours = total_duration.0.unwrap_or(0) as f64 / 3600.0;
        let today_duration: (Option<i64>,) = sqlx::query_as("SELECT SUM(duration) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND timestamp >= ? AND duration IS NOT NULL")
            .bind(user_id).bind(today_start).fetch_one(&self.pool).await?;
        let week_duration: (Option<i64>,) = sqlx::query_as("SELECT SUM(duration) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND timestamp >= ? AND duration IS NOT NULL")
            .bind(user_id).bind(week_start).fetch_one(&self.pool).await?;
        let today_hours = today_duration.0.unwrap_or(0) as f64 / 3600.0;
        let week_hours = week_duration.0.unwrap_or(0) as f64 / 3600.0;
        let avg_quality: (Option<f64>,) = sqlx::query_as("SELECT AVG(quality_score) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND quality_score IS NOT NULL")
            .bind(user_id).fetch_one(&self.pool).await?;

        Ok(serde_json::json!({
            "today": today.0, "this_week": week.0, "total": total.0,
            "lossless_pct": lossless_pct,
            // What the share is *of*. The client shows the coverage instead of
            // the percentage when this is too small a slice to score — a 94%
            // computed from 558 of 21,390 listens is a fact about the 558.
            "fidelity_known": fidelity_known.0,
            "top_artist": top_artist.as_ref().map(|(a, _)| a.as_str()).unwrap_or("—"),
            "top_artist_count": top_artist.as_ref().map(|(_, c)| *c).unwrap_or(0),
            "unique_artists": unique_artists.0, "unique_albums": unique_albums.0,
            "unique_tracks": unique_tracks.0,
            "total_hours": (total_hours * 10.0).round() / 10.0,
            "today_hours": (today_hours * 10.0).round() / 10.0,
            "week_hours": (week_hours * 10.0).round() / 10.0,
            "avg_quality": avg_quality.0.map(|q| (q * 10.0).round() / 10.0).unwrap_or(0.0),
        }))
    }

    // ════════════════════════════════════════════════════════════
    //  Scrobble Edit / Delete / Import / Export
    // ════════════════════════════════════════════════════════════

    /// PATCH a listen. Every field is optional; only provided (`Some`) fields
    /// change (COALESCE keeps the existing value on NULL). Scoped by user_id.
    /// Returns false if no row matched (wrong id or not owned by user).
    #[allow(clippy::too_many_arguments)]
    pub async fn update_scrobble(
        &self,
        user_id: i64,
        id: i64,
        title: Option<&str>,
        artist: Option<&str>,
        album: Option<&str>,
        mbid_recording: Option<&str>,
        mbid_release: Option<&str>,
        chain_id: Option<i64>,
        listening_context: Option<&str>,
        format_type: Option<&str>,
        codec: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        // Changing the release means the artwork we resolved is for the wrong
        // record, so the tombstones are cleared and the backfill picks the row
        // up again. This is the whole point of being able to fix an MBID by
        // hand: the cover is what made the mistake visible in the first place.
        let clear_artwork = mbid_release.is_some();

        let res = sqlx::query(
            "UPDATE scrobbles SET
                title = COALESCE(?, title),
                artist = COALESCE(?, artist),
                album = COALESCE(?, album),
                mbid_recording = COALESCE(?, mbid_recording),
                mbid_release = COALESCE(?, mbid_release),
                chain_id = COALESCE(?, chain_id),
                listening_context = COALESCE(?, listening_context),
                format_type = COALESCE(?, format_type),
                codec = COALESCE(?, codec),
                artwork_url = CASE WHEN ? THEN NULL ELSE artwork_url END,
                caa_id = CASE WHEN ? THEN NULL ELSE caa_id END,
                caa_release_mbid = CASE WHEN ? THEN NULL ELSE caa_release_mbid END
             WHERE id = ? AND user_id = ?",
        )
        .bind(title)
        .bind(artist)
        .bind(album)
        .bind(mbid_recording)
        .bind(mbid_release)
        .bind(chain_id)
        .bind(listening_context)
        .bind(format_type)
        .bind(codec)
        .bind(clear_artwork)
        .bind(clear_artwork)
        .bind(clear_artwork)
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete a single listen owned by the user. Returns false if not found.
    ///
    /// Foreign keys are on, so a note attached to this listen has to go first —
    /// otherwise the delete fails outright. The note is about *this playing* of
    /// the track, so it has nothing to survive onto; notes about the record
    /// itself hang off an entity and are untouched.
    pub async fn delete_scrobble(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        self.delete_notes_for_scrobbles(user_id, Some(id)).await?;
        let res = sqlx::query("DELETE FROM scrobbles WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete ALL listens for the user. Returns the number removed.
    pub async fn clear_scrobbles(&self, user_id: i64) -> Result<u64, sqlx::Error> {
        self.delete_notes_for_scrobbles(user_id, None).await?;
        let res = sqlx::query("DELETE FROM scrobbles WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// Clear listen-attached notes (and their revisions) for one listen, or for
    /// every listen this user has when `id` is None.
    async fn delete_notes_for_scrobbles(
        &self,
        user_id: i64,
        id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let ids: Vec<(i64,)> = match id {
            Some(id) => {
                sqlx::query_as("SELECT id FROM notes WHERE user_id = ? AND scrobble_id = ?")
                    .bind(user_id)
                    .bind(id)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query_as("SELECT id FROM notes WHERE user_id = ? AND scrobble_id IS NOT NULL")
                    .bind(user_id)
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        for (note_id,) in ids {
            sqlx::query("DELETE FROM note_revisions WHERE note_id = ?")
                .bind(note_id)
                .execute(&self.pool)
                .await?;
            sqlx::query("DELETE FROM notes WHERE id = ?").bind(note_id).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// A single listen by id, scoped to the user.
    pub async fn get_scrobble(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>("SELECT * FROM scrobbles WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Every listen for a user, newest first — for full-backup export.
    pub async fn get_all_scrobbles(
        &self,
        user_id: i64,
    ) -> Result<Vec<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE user_id = ? ORDER BY timestamp DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Insert a historical listen from an import. Deduped like `save_scrobble`,
    /// but inserted as `imported` (not `pending`) so the outbound sink engine
    /// never re-scrobbles a back-fill to Last.fm/ListenBrainz. Returns true if
    /// a new row was inserted.
    pub async fn import_listen(&self, user_id: i64, play: &Play) -> Result<bool, sqlx::Error> {
        self.import_listen_flagged(user_id, play, false).await
    }

    /// Import a listen, optionally marking it a skip.
    ///
    /// Skips are stored but never counted: they are excluded from listen counts,
    /// analytics and the history listing, and because imports land as `imported`
    /// rather than `pending` they are never forwarded to Last.fm either — a
    /// forwarded skip would be a wrong scrobble on a permanent record.
    pub async fn import_listen_flagged(
        &self,
        user_id: i64,
        play: &Play,
        skipped: bool,
    ) -> Result<bool, sqlx::Error> {
        self.import_listen_detailed(user_id, play, skipped, ImportProvenance::default()).await
    }

    /// Import a listen, keeping what the export knew about *where* it happened.
    ///
    /// Additive over `import_listen_flagged`: every field here is something one
    /// export format carries and the others don't, so the plain call stays the
    /// one to use unless you have something to put in these.
    pub async fn import_listen_detailed(
        &self,
        user_id: i64,
        play: &Play,
        skipped: bool,
        provenance: ImportProvenance,
    ) -> Result<bool, sqlx::Error> {
        if self.exists(user_id, &play.source_id, &play.source_name).await? {
            return Ok(false);
        }
        if self.fuzzy_exists(user_id, play).await? {
            return Ok(false);
        }
        let mbid_artist_json =
            play.mbid_artist.as_ref().map(|arr| serde_json::to_string(arr).unwrap());
        sqlx::query(
            "INSERT INTO scrobbles (
                user_id, title, artist, album, timestamp, duration, source_id, source_name, status,
                mbid_recording, mbid_release, mbid_artist, caa_id, caa_release_mbid,
                submission_client, track_number, skipped, spotify_track_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(&play.title)
        .bind(&play.artist)
        .bind(&play.album)
        .bind(play.timestamp as i64)
        .bind(play.duration.map(|d| d as i64))
        .bind(&play.source_id)
        .bind(&play.source_name)
        .bind(&play.mbid_recording)
        .bind(&play.mbid_release)
        .bind(&mbid_artist_json)
        .bind(play.caa_id)
        .bind(&play.caa_release_mbid)
        .bind(provenance.submission_client.as_deref().unwrap_or("import"))
        .bind(play.track_number)
        .bind(skipped)
        .bind(&provenance.spotify_track_id)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    // ════════════════════════════════════════════════════════════
    //  Ranged analytics  (GET /api/v1/stats?range=...)
    // ════════════════════════════════════════════════════════════

    /// `after` = None means all-time. Genres/languages/regions/decades need
    /// MusicBrainz enrichment we don't have yet, so they return empty arrays
    /// (the UI hides empty breakdowns). totals/top_artists/heatmap are real.
    pub async fn get_ranged_stats(
        &self,
        user_id: i64,
        after: Option<i64>,
        by_hours: bool,
    ) -> Result<serde_json::Value, sqlx::Error> {
        // The top lists are cut to 50 rows, so the sort has to happen in SQL:
        // re-sorting the returned 50 by hours in the browser would rank within
        // the top 50 *by plays*, and silently miss a long record played a few
        // times — exactly the row switching to Hours exists to surface.
        //
        // `SUM(duration)` is NULL for listens whose source reported no length,
        // so it is coalesced to 0; those sort last under Hours, which is right
        // — an unknown running time is not a large one.
        let order = if by_hours { "COALESCE(secs, 0) DESC, plays DESC" } else { "plays DESC" };
        let plays: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let total = plays.0;

        let secs: (Option<i64>,) = sqlx::query_as(
            "SELECT SUM(duration) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND duration IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let hours = secs.0.unwrap_or(0) as f64 / 3600.0;
        // Hours is a sum over the listens that *have* a length, so it is a floor
        // rather than a total — a source that reported no duration contributes
        // nothing and says nothing. Cheaper to state the coverage than to
        // pretend a listen of unknown length lasted zero seconds.
        let duration_known: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND duration IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;

        let artists: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let albums: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT album) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND album IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let tracks: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT title || '|' || artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let avg_quality: (Option<f64>,) = sqlx::query_as(
            "SELECT AVG(quality_score) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND quality_score IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        // The average's own denominator. Paired counts must share a population,
        // and this one is *not* `total` — see `quality_known` in the payload.
        let quality_known: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND quality_score IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let lossless: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND is_lossless = TRUE"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let dsd: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND format_type = 'dsd'"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let fidelity_known: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                AND {FIDELITY_KNOWN}"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_one(&self.pool)
        .await?;
        // ── The four "shape of the listening" figures ──
        //
        // Deliberately *not* filtered to `skipped = FALSE`: this is the one
        // query in the file whose whole subject is the skips. Everything that
        // counts listens still filters them out, and `plays` above already did.
        let skips: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = TRUE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;

        // "Since" is a fact about the whole history and is therefore *not*
        // ranged — asking it of a one-week window would answer with the start
        // of that window, which is a restatement of the control rather than an
        // answer. Skips are excluded: the first thing you skipped past is not
        // the day you started listening.
        let first_listen: (Option<i64>,) = sqlx::query_as(
            "SELECT MIN(timestamp) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        let loves: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM loves WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?;

        // Plays per **calendar** day, not per day you happened to listen — 846
        // plays over five active days reads as 169/day when the month's real
        // rate is 27. Same trap `reports::avg_daily` documents.
        //
        // `range_after` builds rolling windows (`now - 7 days`) rather than
        // calendar periods, so the span is simply how far back `after` reaches;
        // all-time measures from the first listen. Both are clamped to at least
        // a day, or a history a few hours old divides by nearly zero and
        // reports a spectacular rate.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let span_secs = match after {
            Some(a) => now - a,
            None => first_listen.0.map(|f| now - f).unwrap_or(0),
        };
        let days = (span_secs as f64 / 86_400.0).max(1.0);
        let per_day = if total > 0 { (total as f64 / days * 10.0).round() / 10.0 } else { 0.0 };

        let pct = |n: i64| {
            if total > 0 {
                (n as f64 / total as f64 * 100.0).round() as i64
            } else {
                0
            }
        };
        // The fidelity shares have their own denominator, and it is not `total`
        // — see `FIDELITY_KNOWN`. The mix below keeps `pct`, because it carries
        // an `Unknown` bucket and so really is a share of everything.
        let fpct = |n: i64| {
            if fidelity_known.0 > 0 {
                (n as f64 / fidelity_known.0 as f64 * 100.0).round() as i64
            } else {
                0
            }
        };

        let top_rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(&format!(
            "SELECT artist, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY artist ORDER BY {} LIMIT 50",
            order
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let names: Vec<String> = top_rows.iter().map(|(n, _, _)| n.clone()).collect();
        let images = self.artist_images(&names).await.unwrap_or_default();
        let top_artists: Vec<serde_json::Value> = top_rows
            .into_iter()
            .map(|(name, plays, secs)| {
                let image = images.get(&Self::artist_key(&name)).cloned();
                serde_json::json!({
                    "name": name, "plays": plays,
                    "hours": (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0,
                    "image_url": image,
                })
            })
            .collect();

        let album_rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(&format!(
            "SELECT album, artist, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND album IS NOT NULL AND album != ''
             GROUP BY album, artist ORDER BY {} LIMIT 50",
            order
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let top_albums: Vec<serde_json::Value> = album_rows
            .into_iter()
            .map(|(name, artist, plays, secs)| {
                serde_json::json!({ "name": name, "artist": artist, "plays": plays,
                "hours": (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0 })
            })
            .collect();

        // Grouped case-insensitively on title+artist, like every other listing
        // query — the same song submitted as "Xtal" and "XTAL" by two clients is
        // one track, not two entries competing for the same slot.
        let track_rows: Vec<(String, String, Option<String>, i64, Option<i64>)> = sqlx::query_as(
            &format!(            "SELECT title, artist, MAX(album) AS album, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist)) ORDER BY {} LIMIT 50",
                order
            ),
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let top_tracks: Vec<serde_json::Value> = track_rows
            .into_iter()
            .map(|(title, artist, album, plays, secs)| {
                serde_json::json!({ "title": title, "artist": artist, "album": album,
                "plays": plays,
                "hours": (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0 })
            })
            .collect();

        // Fidelity mix, bucketed the way the badges are: DSD > lossless > lossy,
        // with anything unlabelled called out rather than silently dropped.
        //
        // `Unknown` is written as the complement of `FIDELITY_KNOWN` rather than
        // as its own `ELSE`, so this bucket and the shares above are provably
        // the same split of the same rows. Written the other way round they were
        // *almost* the same, which is worse than different.
        let fidelity_rows: Vec<(String, i64)> = sqlx::query_as(&format!(
            "SELECT CASE
                    WHEN NOT {FIDELITY_KNOWN} THEN 'Unknown'
                    WHEN format_type = 'dsd' THEN 'DSD'
                    WHEN is_lossless = TRUE THEN 'Lossless'
                    ELSE 'Lossy'
                END AS tier, COUNT(*) as c
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY tier ORDER BY c DESC"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let fidelity: Vec<serde_json::Value> = fidelity_rows
            .into_iter()
            .map(|(name, c)| serde_json::json!({ "name": name, "plays": c, "pct": pct(c) }))
            .collect();

        // Release decades. Only listens whose release year has been resolved by
        // the MusicBrainz backfill count — the rest are simply absent.
        let decade_rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT (release_year / 10) * 10 AS decade, COUNT(*) as c
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND release_year IS NOT NULL AND release_year > 1000
             GROUP BY decade ORDER BY decade",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let decade_total: i64 = decade_rows.iter().map(|(_, c)| c).sum();
        let decades: Vec<serde_json::Value> = decade_rows.into_iter().map(|(d, c)| {
            serde_json::json!({ "name": format!("{}s", d), "plays": c,
                "pct": if decade_total > 0 { (c as f64 / decade_total as f64 * 100.0).round() as i64 } else { 0 } })
        }).collect();

        // Genre breakdown. One listen carries several tags, so `json_each`
        // fans the array out and each tag is counted once per listen that has
        // it — which means the percentages are a share of *tag mentions*, not
        // of listens, and cannot be read as "40% of what I played was rock".
        // Tombstoned rows (`[]`) contribute nothing and drop out naturally.
        let genre_rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT g.value AS name, COUNT(*) AS c
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND (? IS NULL OR s.timestamp >= ?) AND s.genres IS NOT NULL
              GROUP BY LOWER(g.value) ORDER BY c DESC LIMIT 12",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let genre_total: i64 = genre_rows.iter().map(|(_, c)| c).sum();
        let genres: Vec<serde_json::Value> = genre_rows.into_iter().map(|(name, c)| {
            serde_json::json!({ "name": name, "plays": c,
                "pct": if genre_total > 0 { (c as f64 / genre_total as f64 * 100.0).round() as i64 } else { 0 } })
        }).collect();

        // Language breakdown (only listens that have been classified).
        // `skipped = FALSE` was missing here: a skip is stored but must never be
        // counted, and this is a counting query like any other.
        let lang_rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT language, COUNT(*) as c FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND language IS NOT NULL
             GROUP BY language ORDER BY c DESC LIMIT 12",
        )
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        let lang_total: i64 = lang_rows.iter().map(|(_, c)| c).sum();
        let languages: Vec<serde_json::Value> = lang_rows.into_iter().map(|(name, c)| {
            serde_json::json!({ "name": name, "pct": if lang_total > 0 { (c as f64 / lang_total as f64 * 100.0).round() as i64 } else { 0 } })
        }).collect();

        Ok(serde_json::json!({
            "totals": {
                "plays": total, "hours": (hours * 10.0).round() / 10.0,
                // What that sum is over. A listen with no reported length adds
                // nothing to it, so the figure is a floor.
                "duration_known": duration_known.0,
                "artists": artists.0, "albums": albums.0, "tracks": tracks.0,
                "avg_quality": avg_quality.0.map(|q| (q * 10.0).round() / 10.0).unwrap_or(0.0),
                // How many listens that average is actually over. `compute_score`
                // only runs at ingest and in the Plex/Jellyfin sources, so an
                // imported listen has no score — the average has the same narrow
                // population the fidelity shares do, and the card must say so
                // rather than presenting a figure drawn from 5% of a history as
                // a fact about all of it.
                "quality_known": quality_known.0,
                "lossless_pct": fpct(lossless.0), "dsd_pct": fpct(dsd.0),
                // The two shares above are of this, not of `plays`.
                "fidelity_known": fidelity_known.0,
                "skips": skips.0,
                "per_day": per_day,
                "first_listen": first_listen.0,
                "loves": loves.0,
            },
            "top_artists": top_artists,
            "top_albums": top_albums,
            "top_tracks": top_tracks,
            "fidelity": fidelity,
            "decades": decades,
            "genres": genres, "languages": languages, "regions": [],
            // heatmap is filled in by the handler in the user's local timezone
            "heatmap": Vec::<Vec<i64>>::new(),
        }))
    }

    // ════════════════════════════════════════════════════════════
    //  User preferences  (stored in users.settings JSON — no new table)
    // ════════════════════════════════════════════════════════════

    /// Returns the user's `settings` JSON object (defaults to `{}`).
    pub async fn get_preferences(&self, user_id: i64) -> Result<serde_json::Value, sqlx::Error> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT settings FROM users WHERE id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        let json = row
            .and_then(|(s,)| s)
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .filter(|v| v.is_object())
            .unwrap_or_else(|| serde_json::json!({}));
        Ok(json)
    }

    /// Shallow-merge `patch` into the user's stored preferences and persist.
    /// Returns the merged object.
    pub async fn merge_preferences(
        &self,
        user_id: i64,
        patch: &serde_json::Value,
    ) -> Result<serde_json::Value, sqlx::Error> {
        let mut current = self.get_preferences(user_id).await?;
        if let (Some(obj), Some(p)) = (current.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                obj.insert(k.clone(), v.clone());
            }
        }
        sqlx::query("UPDATE users SET settings = ? WHERE id = ?")
            .bind(current.to_string())
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(current)
    }

    // ════════════════════════════════════════════════════════════
    //  Signal Chains
    // ════════════════════════════════════════════════════════════

    pub async fn create_chain(&self, chain: &SignalChain) -> Result<i64, sqlx::Error> {
        let components_json =
            serde_json::to_string(&chain.components).unwrap_or_else(|_| "[]".into());
        let now = Self::now_secs();
        let result = sqlx::query(
            "INSERT INTO signal_chains (user_id, name, description, components, listening_context, icon, is_active, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        ).bind(chain.user_id).bind(&chain.name).bind(&chain.description)
        .bind(&components_json).bind(chain.listening_context.as_str()).bind(&chain.icon)
        .bind(chain.is_active).bind(now)
        .execute(&self.pool).await?;
        Ok(result.last_insert_rowid())
    }

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
                s.listening_context, s.icon, s.is_active,
                CAST(COALESCE((
                    SELECT SUM(sc.duration) FROM scrobbles sc
                     WHERE sc.user_id = s.user_id AND sc.chain_id = s.id
                       AND sc.skipped = FALSE
                ), 0) AS REAL) / 3600.0 AS total_hours,
                s.created_at";

    pub async fn get_chains(&self, user_id: i64) -> Result<Vec<SignalChain>, sqlx::Error> {
        let rows: Vec<ChainRow> = sqlx::query_as(&format!(
            "SELECT {} FROM signal_chains s WHERE s.user_id = ? ORDER BY s.name ASC",
            Self::CHAIN_COLUMNS
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    pub async fn get_chain(
        &self,
        user_id: i64,
        chain_id: i64,
    ) -> Result<Option<SignalChain>, sqlx::Error> {
        let row: Option<ChainRow> = sqlx::query_as(&format!(
            "SELECT {} FROM signal_chains s WHERE s.id = ? AND s.user_id = ?",
            Self::CHAIN_COLUMNS
        ))
        .bind(chain_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.into()))
    }

    /// Resolve a chain *name* (as sent by clients in `tapedeck_chain.chain_id`)
    /// to its id for this user. Case-insensitive.
    pub async fn get_chain_by_name(
        &self,
        user_id: i64,
        name: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM signal_chains WHERE user_id = ? AND LOWER(name) = LOWER(?)",
        )
        .bind(user_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id,)| id))
    }

    /// A source device's configured default chain, if any (fallback for listens
    /// that don't carry an explicit chain or output binding).
    pub async fn get_device_default_chain(
        &self,
        user_id: i64,
        device_id: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT default_chain_id FROM devices WHERE id = ? AND user_id = ?")
                .bind(device_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|(c,)| c))
    }

    /// Point a source device at a chain (or clear it with `None`).
    pub async fn set_device_default_chain(
        &self,
        user_id: i64,
        device_id: i64,
        chain_id: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let result =
            sqlx::query("UPDATE devices SET default_chain_id = ? WHERE id = ? AND user_id = ?")
                .bind(chain_id)
                .bind(device_id)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_chain(
        &self,
        user_id: i64,
        chain_id: i64,
        name: &str,
        description: Option<&str>,
        components: &[ChainComponent],
        listening_context: &str,
        icon: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let components_json = serde_json::to_string(components).unwrap_or_else(|_| "[]".into());
        let res = sqlx::query(
            "UPDATE signal_chains SET name = ?, description = ?, components = ?, listening_context = ?, icon = ?
             WHERE id = ? AND user_id = ?"
        ).bind(name).bind(description).bind(&components_json).bind(listening_context).bind(icon)
         .bind(chain_id).bind(user_id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete a chain, first unlinking (not deleting) anything that references
    /// it — listens and sessions keep their data with `chain_id` cleared. Done
    /// in a transaction so we never leave dangling FKs (sqlx enables them).
    pub async fn delete_chain(&self, user_id: i64, chain_id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE scrobbles SET chain_id = NULL WHERE chain_id = ? AND user_id = ?")
            .bind(chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE sessions SET chain_id = NULL WHERE chain_id = ? AND user_id = ?")
            .bind(chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE devices SET default_chain_id = NULL WHERE default_chain_id = ? AND user_id = ?",
        )
        .bind(chain_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE chain_bindings SET chain_id = NULL WHERE chain_id = ? AND user_id = ?")
            .bind(chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        let res = sqlx::query("DELETE FROM signal_chains WHERE id = ? AND user_id = ?")
            .bind(chain_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    // ════════════════════════════════════════════════════════════
    //  Output-device → chain bindings (Phase B)
    // ════════════════════════════════════════════════════════════

    /// Record that we saw a listen come through this output device. Inserts a
    /// new unmapped row (chain_id NULL) the first time, else bumps hits/last_seen
    /// and refreshes the interface. Never overwrites an existing chain mapping.
    pub async fn record_output(
        &self,
        user_id: i64,
        identifier: &str,
        interface: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = Self::now_secs();
        sqlx::query(
            "INSERT INTO chain_bindings (user_id, identifier, interface, chain_id, last_seen, hits)
             VALUES (?, ?, ?, NULL, ?, 1)
             ON CONFLICT(user_id, identifier) DO UPDATE SET
                hits = hits + 1, last_seen = excluded.last_seen,
                interface = COALESCE(excluded.interface, chain_bindings.interface)",
        )
        .bind(user_id)
        .bind(identifier)
        .bind(interface)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Resolve an output-device identifier to its bound chain, if mapped.
    pub async fn resolve_binding(
        &self,
        user_id: i64,
        identifier: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT chain_id FROM chain_bindings WHERE user_id = ? AND identifier = ?",
        )
        .bind(user_id)
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(c,)| c))
    }

    pub async fn list_bindings(&self, user_id: i64) -> Result<Vec<BindingRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM chain_bindings WHERE user_id = ? ORDER BY chain_id IS NOT NULL, last_seen DESC")
            .bind(user_id).fetch_all(&self.pool).await
    }

    /// Assign (or clear, with None) the chain for an output identifier. Creates
    /// the binding if the identifier hasn't been seen yet (manual add).
    pub async fn set_binding(
        &self,
        user_id: i64,
        identifier: &str,
        chain_id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let now = Self::now_secs();
        sqlx::query(
            "INSERT INTO chain_bindings (user_id, identifier, chain_id, last_seen, hits)
             VALUES (?, ?, ?, ?, 0)
             ON CONFLICT(user_id, identifier) DO UPDATE SET chain_id = excluded.chain_id",
        )
        .bind(user_id)
        .bind(identifier)
        .bind(chain_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_binding(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM chain_bindings WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ════════════════════════════════════════════════════════════
    //  Import history (persisted; the live job registry is in-memory)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Per-user scrobble connections
    // ════════════════════════════════════════════════════════════

    pub async fn set_connection(
        &self,
        user_id: i64,
        service: &str,
        credentials: &str,
        username: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO user_connections (user_id, service, credentials, connected_username, connected_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(user_id, service) DO UPDATE SET
                credentials = excluded.credentials,
                connected_username = excluded.connected_username,
                connected_at = excluded.connected_at"
        )
        .bind(user_id).bind(service)
        .bind(self.seal(credentials)).bind(username).bind(Self::now_secs())
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn delete_connection(
        &self,
        user_id: i64,
        service: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM user_connections WHERE user_id = ? AND service = ?")
            .bind(user_id)
            .bind(service)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Public-facing list: service + connected account only, never credentials.
    pub async fn list_connections(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, Option<String>, i64)>, sqlx::Error> {
        sqlx::query_as("SELECT service, connected_username, connected_at FROM user_connections WHERE user_id = ?")
            .bind(user_id).fetch_all(&self.pool).await
    }

    /// Credentials for the engine to build this user's sinks. `(service, credentials_json)`.
    pub async fn get_connections_for_forwarding(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT service, credentials FROM user_connections WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        // A row we can't decrypt is skipped rather than fatal: one bad credential
        // shouldn't stop the user's other services from receiving listens.
        Ok(rows
            .into_iter()
            .filter_map(|(service, blob)| match self.secrets.decrypt(&blob) {
                Ok(plain) => Some((service, plain)),
                Err(e) => {
                    error!("Undecryptable {} credential for user {}: {}", service, user_id, e);
                    None
                }
            })
            .collect())
    }

    /// Wrap a credential for storage. Encryption failure is not silently
    /// tolerated — storing plaintext because the cipher errored would defeat the
    /// point, so the row keeps whatever it had and the caller sees the error via
    /// the write failing.
    fn seal(&self, plaintext: &str) -> String {
        match self.secrets.encrypt(plaintext) {
            Ok(sealed) => sealed,
            Err(e) => {
                error!("Credential encryption failed: {} — refusing to store plaintext", e);
                String::new()
            }
        }
    }

    fn unseal(&self, stored: &str) -> Option<String> {
        match self.secrets.decrypt(stored) {
            Ok(plain) => Some(plain),
            Err(e) => {
                error!("Could not decrypt stored credential: {}", e);
                None
            }
        }
    }

    /// One-time upgrade for rows written before encryption existed. Idempotent —
    /// already-encrypted values carry a prefix and are skipped, so this is safe
    /// to run on every boot.
    async fn encrypt_legacy_credentials(&self) -> Result<(), sqlx::Error> {
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT user_id, service, credentials FROM user_connections")
                .fetch_all(&self.pool)
                .await?;

        let mut upgraded = 0;
        for (user_id, service, blob) in rows {
            if SecretBox::is_encrypted(&blob) || blob.is_empty() {
                continue;
            }
            let sealed = self.seal(&blob);
            if sealed.is_empty() {
                continue;
            }
            sqlx::query(
                "UPDATE user_connections SET credentials = ? WHERE user_id = ? AND service = ?",
            )
            .bind(sealed)
            .bind(user_id)
            .bind(&service)
            .execute(&self.pool)
            .await?;
            upgraded += 1;
        }
        if upgraded > 0 {
            info!("🔐 Encrypted {} stored credential(s) at rest", upgraded);
        }
        Ok(())
    }

    // ════════════════════════════════════════════════════════════
    //  Artists
    // ════════════════════════════════════════════════════════════

    /// Case- and padding-insensitive key, so "Aphex Twin" and "aphex twin " are
    /// one artist rather than two rows that each learn half the metadata.
    fn artist_key(name: &str) -> String {
        name.trim().to_lowercase()
    }

    /// Record what we've learned about an artist. Existing values are kept
    /// unless the caller has something better — a later lookup that finds no
    /// image must not erase one we already had.
    pub async fn upsert_artist(
        &self,
        name: &str,
        image_url: Option<&str>,
        mbid: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let key = Self::artist_key(name);
        if key.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO artists (name, name_key, mbid, image_url, updated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(name_key) DO UPDATE SET
                mbid = COALESCE(excluded.mbid, artists.mbid),
                image_url = COALESCE(excluded.image_url, artists.image_url),
                updated_at = excluded.updated_at",
        )
        .bind(name.trim())
        .bind(&key)
        .bind(mbid)
        .bind(image_url)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

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

    /// Played a lot once, not lately.
    ///
    /// The bar is relative rather than absolute: "a lot" means it is in the
    /// upper half of what you play at all, so a modest library still produces
    /// a list. `quiet_since` is the cutoff for "not lately".
    pub async fn forgotten_favourites(
        &self,
        user_id: i64,
        quiet_since: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                    MAX(caa_release_mbid) AS caa_release_mbid
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
             HAVING COUNT(*) >= 3 AND MAX(timestamp) < ?
              ORDER BY COUNT(*) DESC, MAX(timestamp) ASC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(quiet_since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Loved, but hardly ever played.
    ///
    /// A love is the strongest signal this app collects — you went out of your
    /// way — so a loved track with two plays is the sharpest "you meant to come
    /// back to this" it can produce.
    pub async fn loved_but_unplayed(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT e.name AS name, e.artist_name AS artist,
                    (SELECT MAX(s.album) FROM scrobbles s
                      WHERE s.user_id = l.user_id AND s.skipped = FALSE
                        AND LOWER(TRIM(s.title)) = LOWER(TRIM(e.name))
                        AND LOWER(TRIM(s.artist)) = LOWER(TRIM(e.artist_name))) AS album,
                    (SELECT COUNT(*) FROM scrobbles s
                      WHERE s.user_id = l.user_id AND s.skipped = FALSE
                        AND LOWER(TRIM(s.title)) = LOWER(TRIM(e.name))
                        AND LOWER(TRIM(s.artist)) = LOWER(TRIM(e.artist_name))) AS plays,
                    (SELECT MAX(s.timestamp) FROM scrobbles s
                      WHERE s.user_id = l.user_id AND s.skipped = FALSE
                        AND LOWER(TRIM(s.title)) = LOWER(TRIM(e.name))
                        AND LOWER(TRIM(s.artist)) = LOWER(TRIM(e.artist_name))) AS last_played,
                    NULL AS artwork_url, NULL AS caa_id, NULL AS caa_release_mbid
               FROM loves l JOIN entities e ON e.id = l.entity_id
              WHERE l.user_id = ? AND e.kind = 'recording'
              ORDER BY plays ASC, l.created_at DESC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Played exactly once, and long enough ago to have been forgotten.
    ///
    /// A record you heard once and never returned to is either a miss or a
    /// thing you never got round to. This app can't tell which, which is the
    /// point of showing it to you.
    pub async fn heard_once(
        &self,
        user_id: i64,
        before: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                    MAX(caa_release_mbid) AS caa_release_mbid
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
             HAVING COUNT(*) = 1 AND MAX(timestamp) < ?
              ORDER BY MAX(timestamp) DESC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(before)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// What you were playing this time of year, in years gone by.
    ///
    /// Month-of-year in the *user's* timezone would be more correct, but the
    /// window is ±3 weeks wide, so an hour of offset cannot move anything
    /// across it — and doing it in SQL keeps this one query rather than a scan.
    /// Excludes the current year, or it would just be "recently played".
    pub async fn seasonal(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                    MAX(caa_release_mbid) AS caa_release_mbid
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND STRFTIME('%Y', timestamp, 'unixepoch') <> STRFTIME('%Y', 'now')
                AND ABS(
                      CAST(STRFTIME('%j', timestamp, 'unixepoch') AS INTEGER)
                    - CAST(STRFTIME('%j', 'now') AS INTEGER)
                    ) <= 21
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
              ORDER BY COUNT(*) DESC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Records and tapes that haven't left the shelf.
    ///
    /// The one list here that isn't about listens at all — it reads
    /// `physical_plays`, so it only says something once the shelf is in use.
    /// A record never played is included: never is longer than 18 months.
    pub async fn dusty_shelf(
        &self,
        user_id: i64,
        quiet_since: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT r.title AS name, r.artist AS artist, r.title AS album,
                    (SELECT COUNT(*) FROM physical_plays p WHERE p.release_id = r.id) AS plays,
                    (SELECT MAX(p.played_at) FROM physical_plays p WHERE p.release_id = r.id)
                      AS last_played,
                    r.artwork_url AS artwork_url, NULL AS caa_id,
                    r.mbid_release AS caa_release_mbid
               FROM physical_releases r
              WHERE r.user_id = ?
                AND COALESCE(
                      (SELECT MAX(p.played_at) FROM physical_plays p WHERE p.release_id = r.id), 0
                    ) < ?
              ORDER BY last_played ASC NULLS FIRST, r.added_at ASC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(quiet_since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Tracks you only ever reach for after midnight.
    ///
    /// Hour-of-day in the user's timezone, because "after midnight" in UTC is
    /// the evening in one place and the afternoon in another — the same bug
    /// that made the dashboard's day start at 01:00. The offset is applied to
    /// the timestamp before the hour is taken.
    pub async fn late_night(
        &self,
        user_id: i64,
        tz_offset_secs: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                    MAX(caa_release_mbid) AS caa_release_mbid
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
             HAVING COUNT(*) >= 3
                AND SUM(
                      CASE WHEN CAST(STRFTIME('%H', timestamp + ?, 'unixepoch') AS INTEGER)
                                 >= 0
                            AND CAST(STRFTIME('%H', timestamp + ?, 'unixepoch') AS INTEGER) < 5
                           THEN 1 ELSE 0 END
                    ) * 1.0 / COUNT(*) >= 0.7
              ORDER BY COUNT(*) DESC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(tz_offset_secs)
        .bind(tz_offset_secs)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Album tracks you play that are never the single.
    ///
    /// "Buried on side B" in the design. Approximated by track number, which is
    /// the only positional information a listen carries: a track from the back
    /// half of a record that you nonetheless keep returning to.
    pub async fn deep_cuts(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                    MAX(caa_release_mbid) AS caa_release_mbid
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND track_number IS NOT NULL AND track_number >= 6
                AND album IS NOT NULL AND TRIM(album) != ''
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
             HAVING COUNT(*) >= 3
              ORDER BY COUNT(*) DESC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Things you like that have never been through a given piece of gear.
    ///
    /// The join is the reason `chain_id` on a listen matters: a chain names its
    /// components, so "never heard on the HD 600s" is answerable without any
    /// per-listen equipment field. Restricted to tracks you have played more
    /// than once, because the interesting question is what your *favourites*
    /// sound like on it, not your whole library.
    pub async fn never_on_gear(
        &self,
        user_id: i64,
        equipment_id: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                    MAX(caa_release_mbid) AS caa_release_mbid
               FROM scrobbles s
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND NOT EXISTS (
                      SELECT 1 FROM scrobbles s2
                        JOIN signal_chains c ON c.id = s2.chain_id
                       WHERE s2.user_id = s.user_id AND s2.skipped = FALSE
                         AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                         AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                         AND EXISTS (
                               SELECT 1 FROM json_each(c.components) j
                                WHERE json_extract(j.value, '$.equipment_id') = ?
                             )
                    )
              GROUP BY LOWER(TRIM(s.title)), LOWER(TRIM(s.artist))
             HAVING COUNT(*) > 1
              ORDER BY COUNT(*) DESC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(equipment_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// The `artists` row id for a name, creating the row if it's new.
    ///
    /// Needed because an artist portrait is served through a proxy keyed by
    /// that id — the image lives on the user's own media server and the browser
    /// can't reach it, the same problem `server/artwork.rs` solves for covers.
    /// An artist's cached genre tags. `Some(vec![])` is the tombstone for
    /// "asked MusicBrainz, it had none"; `None` means not looked up yet.
    pub async fn artist_genres(&self, name: &str) -> Result<Option<Vec<String>>, sqlx::Error> {
        let key = Self::artist_key(name);
        if key.is_empty() {
            return Ok(None);
        }
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT genres FROM artists WHERE name_key = ?")
                .bind(&key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|r| r.0)
            .map(|j| serde_json::from_str::<Vec<String>>(&j).unwrap_or_default()))
    }

    pub async fn set_artist_genres(
        &self,
        name: &str,
        genres: &[String],
    ) -> Result<(), sqlx::Error> {
        let key = Self::artist_key(name);
        if key.is_empty() {
            return Ok(());
        }
        let json = serde_json::to_string(genres).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            "INSERT INTO artists (name, name_key, genres, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(name_key) DO UPDATE SET genres = excluded.genres",
        )
        .bind(name.trim())
        .bind(&key)
        .bind(&json)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn artist_row_id(&self, name: &str) -> Result<Option<i64>, sqlx::Error> {
        let key = Self::artist_key(name);
        if key.is_empty() {
            return Ok(None);
        }
        sqlx::query("INSERT OR IGNORE INTO artists (name, name_key, updated_at) VALUES (?, ?, ?)")
            .bind(name.trim())
            .bind(&key)
            .bind(Self::now_secs())
            .execute(&self.pool)
            .await?;
        let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM artists WHERE name_key = ?")
            .bind(&key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.0))
    }

    pub async fn artist_by_id(
        &self,
        id: i64,
    ) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT name, image_url FROM artists WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    /// Set or clear an artist portrait.
    ///
    /// Unlike `upsert_artist` this *overwrites*, including with NULL — it backs
    /// the manual override, and "I don't want that picture" has to be sayable.
    /// An empty string is the tombstone the backfill reads as "asked, nothing
    /// found", the same convention `artwork_url` uses.
    pub async fn set_artist_image(
        &self,
        id: i64,
        image_url: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE artists SET image_url = ?, updated_at = ? WHERE id = ?")
            .bind(image_url)
            .bind(Self::now_secs())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// A hand-set cover for a release entity, if there is one.
    ///
    /// Kept as its own query rather than a field on `EntityRow`: only the album
    /// page asks, and widening the row would mean touching every one of its
    /// column lists for a value nothing else reads.
    pub async fn entity_image(&self, id: i64) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT image_url FROM entities WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|r| r.0).filter(|u| !u.is_empty()))
    }

    /// Set or clear a release's cover by hand.
    ///
    /// Restricted to `kind = 'release'` at the query. An artist's picture lives
    /// on the `artists` row — shared by name, and what the library proxy and the
    /// portrait backfill both address — so letting this write one too would give
    /// the same artist two places to be pictured from, and no rule about which
    /// wins. Returns false when the id is not a release, which the handler
    /// reports rather than silently succeeding.
    pub async fn set_entity_image(
        &self,
        id: i64,
        image_url: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res =
            sqlx::query("UPDATE entities SET image_url = ? WHERE id = ? AND kind = 'release'")
                .bind(image_url)
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Artists this user has listened to that still have no portrait.
    ///
    /// Ordered by play count so the ones actually visible on the charts are
    /// resolved first — a backfill that starts alphabetically spends its first
    /// run on artists nobody sees.
    pub async fn artists_without_images(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT s.artist FROM scrobbles s
              LEFT JOIN artists a ON a.name_key = LOWER(TRIM(s.artist))
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND TRIM(s.artist) != ''
                AND (a.image_url IS NULL)
              GROUP BY LOWER(TRIM(s.artist))
              ORDER BY COUNT(*) DESC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    pub async fn artist_images(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = vec!["?"; names.len()].join(",");
        let sql = format!(
            "SELECT name_key, image_url FROM artists
             WHERE image_url IS NOT NULL AND name_key IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, String)>(&sql);
        for n in names {
            q = q.bind(Self::artist_key(n));
        }
        Ok(q.fetch_all(&self.pool).await?.into_iter().collect())
    }

    /// How many distinct artists in this user's history have an image yet.
    pub async fn artist_image_coverage(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let with_image: (i64,) = sqlx::query_as(
            "SELECT COUNT(DISTINCT s.artist) FROM scrobbles s
             JOIN artists a ON a.name_key = LOWER(TRIM(s.artist))
             WHERE s.user_id = ? AND s.skipped = FALSE AND a.image_url IS NOT NULL",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((total.0, with_image.0))
    }

    // ════════════════════════════════════════════════════════════
    //  Entities, liner notes and loves (Phase 7)
    // ════════════════════════════════════════════════════════════

    /// Identity of an entity: kind + artist + name, all case- and
    /// padding-insensitive. Two spellings of one record should be one row, and
    /// the same track title by two artists must not be.
    fn entity_key(kind: &str, artist_name: &str, name: &str) -> String {
        format!(
            "{}|{}|{}",
            kind.trim().to_lowercase(),
            artist_name.trim().to_lowercase(),
            name.trim().to_lowercase()
        )
    }

    /// The three things a note or a love can point at. Callers validate against
    /// this before writing — the column is a plain TEXT.
    pub const ENTITY_KINDS: [&'static str; 3] = ["recording", "release", "artist"];

    /// Get-or-create. An MBID is recorded when the caller has one, but never
    /// required and never the identity — `COALESCE` so a later lookup that
    /// finds nothing can't erase an MBID we already had.
    pub async fn upsert_entity(
        &self,
        kind: &str,
        name: &str,
        artist_name: &str,
        mbid: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let key = Self::entity_key(kind, artist_name, name);
        sqlx::query(
            "INSERT INTO entities (kind, name, artist_name, entity_key, mbid, created_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(entity_key) DO UPDATE SET mbid = COALESCE(excluded.mbid, entities.mbid)",
        )
        .bind(kind.trim().to_lowercase())
        .bind(name.trim())
        .bind(artist_name.trim())
        .bind(&key)
        .bind(mbid)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;

        let (id,): (i64,) = sqlx::query_as("SELECT id FROM entities WHERE entity_key = ?")
            .bind(&key)
            .fetch_one(&self.pool)
            .await?;

        if let Some(mbid) = mbid.filter(|m| !m.trim().is_empty()) {
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO entity_aliases (entity_id, alias_kind, value) VALUES (?, 'mbid', ?)"
            ).bind(id).bind(mbid.trim()).execute(&self.pool).await;
        }
        Ok(id)
    }

    pub async fn get_entity(&self, id: i64) -> Result<Option<EntityRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, kind, name, artist_name, mbid, created_at FROM entities WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Exact match on the identity key, or — when the artist isn't known — the
    /// oldest entity of that kind with the name.
    pub async fn find_entity(
        &self,
        kind: &str,
        name: &str,
        artist_name: Option<&str>,
    ) -> Result<Option<EntityRow>, sqlx::Error> {
        if let Some(artist) = artist_name.filter(|a| !a.trim().is_empty()) {
            let key = Self::entity_key(kind, artist, name);
            let hit: Option<EntityRow> = sqlx::query_as(
                "SELECT id, kind, name, artist_name, mbid, created_at FROM entities WHERE entity_key = ?"
            ).bind(key).fetch_optional(&self.pool).await?;
            if hit.is_some() {
                return Ok(hit);
            }
        }
        sqlx::query_as(
            "SELECT id, kind, name, artist_name, mbid, created_at FROM entities
             WHERE kind = ? AND LOWER(TRIM(name)) = ? ORDER BY id LIMIT 1",
        )
        .bind(kind.trim().to_lowercase())
        .bind(name.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await
    }

    /// What a `[[track:…]]` / `[[album:…]]` / `[[artist:…]]` shortcode points at.
    ///
    /// Resolution walks outward: an entity that already exists, then the user's
    /// own listening history. The history step **creates** the entity it finds,
    /// which is a write on a read path — deliberately, because the alternative
    /// is that every cross-reference renders dead until you happen to annotate
    /// the thing it points at. Entities are shared metadata, not user content,
    /// so minting one costs nothing and makes references work for anything you
    /// have actually played.
    ///
    /// `position` handles `[[track:3]]`: the third track of `ctx_album`,
    /// resolved through the listen history and rendered with its real title.
    pub async fn resolve_reference(
        &self,
        user_id: i64,
        kind: &str,
        target: &str,
        position: Option<i64>,
        ctx_artist: Option<&str>,
        ctx_album: Option<&str>,
    ) -> Result<Option<(i64, String)>, sqlx::Error> {
        let needle = target.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(None);
        }

        // "Track 3 of this record" — only answerable with an album in context.
        if let (Some(pos), Some(album)) = (position.filter(|_| kind == "recording"), ctx_album) {
            let row: Option<(String, String, Option<String>)> = sqlx::query_as(
                "SELECT title, artist, mbid_recording FROM scrobbles
                 WHERE user_id = ? AND LOWER(TRIM(album)) = ? AND track_number = ?
                 ORDER BY id DESC LIMIT 1",
            )
            .bind(user_id)
            .bind(album.trim().to_lowercase())
            .bind(pos)
            .fetch_optional(&self.pool)
            .await?;
            if let Some((title, artist, mbid)) = row {
                let id = self.upsert_entity("recording", &title, &artist, mbid.as_deref()).await?;
                return Ok(Some((id, title)));
            }
            return Ok(None);
        }
        if position.is_some() {
            return Ok(None);
        }

        if let Some(e) = self.find_entity(kind, target, ctx_artist).await? {
            return Ok(Some((e.id, e.name)));
        }

        // Nothing annotated yet — fall back to what this user has listened to.
        let found: Option<(String, String, Option<String>)> = match kind {
            "artist" => sqlx::query_as(
                "SELECT artist, '', mbid_artist FROM scrobbles
                 WHERE user_id = ? AND LOWER(TRIM(artist)) = ? ORDER BY id DESC LIMIT 1"
            ).bind(user_id).bind(&needle).fetch_optional(&self.pool).await?,

            // `ORDER BY (artist = ctx) DESC` prefers the record by the artist
            // this note is about when two of them share a title.
            "release" => sqlx::query_as(
                "SELECT album, artist, mbid_release FROM scrobbles
                 WHERE user_id = ? AND LOWER(TRIM(album)) = ?
                 ORDER BY (LOWER(TRIM(artist)) = ?) DESC, id DESC LIMIT 1"
            )
            .bind(user_id).bind(&needle)
            .bind(ctx_artist.unwrap_or_default().trim().to_lowercase())
            .fetch_optional(&self.pool).await?,

            _ => sqlx::query_as(
                "SELECT title, artist, mbid_recording FROM scrobbles
                 WHERE user_id = ? AND LOWER(TRIM(title)) = ?
                 ORDER BY (LOWER(TRIM(album)) = ?) DESC, (LOWER(TRIM(artist)) = ?) DESC, id DESC LIMIT 1"
            )
            .bind(user_id).bind(&needle)
            .bind(ctx_album.unwrap_or_default().trim().to_lowercase())
            .bind(ctx_artist.unwrap_or_default().trim().to_lowercase())
            .fetch_optional(&self.pool).await?,
        };

        let Some((name, artist, mbid)) = found else {
            return Ok(None);
        };
        if name.trim().is_empty() {
            return Ok(None);
        }
        let id = self.upsert_entity(kind, &name, &artist, mbid.as_deref()).await?;
        Ok(Some((id, name)))
    }

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

    /// Point a just-saved listen at its session.
    ///
    /// Keyed on `source_id`, which is the per-listen identity the dedup checks
    /// already rely on.
    pub async fn link_scrobble_session(
        &self,
        user_id: i64,
        source_id: &str,
        session_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE scrobbles SET session_id = ? WHERE user_id = ? AND source_id = ?")
            .bind(session_id)
            .bind(user_id)
            .bind(source_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Recalculate a session's aggregates from the listens that point at it.
    ///
    /// Deletes the session outright when nothing points at it any more, which
    /// is what makes merge and split self-cleaning.
    pub async fn recompute_session(&self, session_id: i64) -> Result<bool, sqlx::Error> {
        let agg: Option<(i64, Option<i64>, Option<i64>, Option<i64>, Option<f64>)> =
            sqlx::query_as(
                "SELECT COUNT(*), MIN(timestamp), MAX(timestamp + COALESCE(duration, 0)),
                        SUM(COALESCE(duration, 0)), AVG(quality_score)
                 FROM scrobbles WHERE session_id = ?",
            )
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await?;

        let Some((count, started, ended, total, avg)) = agg else {
            return Ok(false);
        };
        if count == 0 {
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(session_id)
                .execute(&self.pool)
                .await?;
            return Ok(false);
        }

        sqlx::query(
            "UPDATE sessions SET started_at = ?, ended_at = ?, track_count = ?,
                    total_duration = ?, avg_quality_score = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(started.unwrap_or(0))
        .bind(ended)
        .bind(count)
        .bind(total.unwrap_or(0))
        .bind(avg)
        .bind(Self::now_secs())
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    /// Rebuild every session for one user from their listens.
    ///
    /// Needed because `assign_session` maintained the session rows while nothing
    /// ever wrote `scrobbles.session_id`, so an existing database has sessions
    /// with track counts and no contents. It's also the only way to re-group
    /// after changing the gap, and the way an *imported* history — which never
    /// went through the ingest path — gets sessions at all.
    ///
    /// Destructive by design: manual merges and splits are stored in this same
    /// column, so a rebuild discards them. The caller is expected to say so.
    pub async fn rebuild_sessions(
        &self,
        user_id: i64,
        gap_seconds: i64,
    ) -> Result<(u64, u64), sqlx::Error> {
        // Clear the link first — the foreign key means sessions can't go while
        // anything still points at them.
        sqlx::query("UPDATE scrobbles SET session_id = NULL WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;

        // Ordered walk. Only the gap is available here: a listen doesn't record
        // which token submitted it, so the token-change rule that `assign_session`
        // applies live can't be reconstructed after the fact.
        let rows: Vec<(
            i64,
            i64,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<f64>,
            Option<String>,
        )> = sqlx::query_as(
            "SELECT id, timestamp, duration, device_id, chain_id, quality_score,
                        listening_context
                   FROM scrobbles WHERE user_id = ? AND skipped = FALSE
                  ORDER BY timestamp ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut sessions = 0u64;
        let mut linked = 0u64;
        let mut current: Option<i64> = None;
        let mut prev_end = i64::MIN;

        for (id, ts, dur, device_id, chain_id, quality, context) in rows {
            let start_new = current.is_none() || ts - prev_end > gap_seconds;
            if start_new {
                if let Some(sid) = current {
                    self.recompute_session(sid).await?;
                }
                let res = sqlx::query(
                    "INSERT INTO sessions (user_id, device_id, chain_id, started_at, ended_at,
                        track_count, total_duration, avg_quality_score, listening_context,
                        uuid, updated_at)
                     VALUES (?, ?, ?, ?, ?, 0, 0, ?, ?, ?, ?)",
                )
                .bind(user_id)
                .bind(device_id)
                .bind(chain_id)
                .bind(ts)
                .bind(ts)
                .bind(quality)
                .bind(context.clone().unwrap_or_else(|| "unknown".into()))
                .bind(new_uuid())
                .bind(Self::now_secs())
                .execute(&self.pool)
                .await?;
                current = Some(res.last_insert_rowid());
                sessions += 1;
            }

            if let Some(sid) = current {
                sqlx::query("UPDATE scrobbles SET session_id = ? WHERE id = ?")
                    .bind(sid)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                linked += 1;
            }
            prev_end = ts + dur.unwrap_or(0);
        }

        if let Some(sid) = current {
            self.recompute_session(sid).await?;
        }
        info!("🧵 Rebuilt {} sessions across {} listens for user {}", sessions, linked, user_id);
        Ok((sessions, linked))
    }

    /// A user's sessions, most recent first, with the chain's name joined so the
    /// list doesn't need a second request per row.
    pub async fn list_sessions(
        &self,
        user_id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<SessionRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT s.id, s.uuid, s.title, s.started_at, s.ended_at, s.track_count,
                    s.total_duration, s.avg_quality_score, s.listening_context,
                    s.chain_id, c.name AS chain_name, s.device_id, d.name AS device_name,
                    s.token_id, t.name AS token_name
               FROM sessions s
               LEFT JOIN signal_chains c ON c.id = s.chain_id
               LEFT JOIN devices d ON d.id = s.device_id
               LEFT JOIN tokens t ON t.id = s.token_id
              WHERE s.user_id = ? AND s.track_count > 0
              ORDER BY s.started_at DESC LIMIT ? OFFSET ?",
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_session(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<SessionRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT s.id, s.uuid, s.title, s.started_at, s.ended_at, s.track_count,
                    s.total_duration, s.avg_quality_score, s.listening_context,
                    s.chain_id, c.name AS chain_name, s.device_id, d.name AS device_name,
                    s.token_id, t.name AS token_name
               FROM sessions s
               LEFT JOIN signal_chains c ON c.id = s.chain_id
               LEFT JOIN devices d ON d.id = s.device_id
               LEFT JOIN tokens t ON t.id = s.token_id
              WHERE s.user_id = ? AND s.id = ?",
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// The listens in one session, oldest first — the order they were played.
    pub async fn session_scrobbles(
        &self,
        user_id: i64,
        session_id: i64,
    ) -> Result<Vec<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM scrobbles WHERE user_id = ? AND session_id = ?
             ORDER BY timestamp ASC",
        )
        .bind(user_id)
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Fold several sessions into one.
    ///
    /// The **earliest** session survives, because its id is the one already
    /// referenced by anything pointing at "that listening"; the rest hand over
    /// their listens and are deleted by the recompute. Ownership is checked for
    /// every id before anything moves, so a request naming someone else's
    /// session changes nothing at all rather than half-merging.
    pub async fn merge_sessions(
        &self,
        user_id: i64,
        ids: &[i64],
    ) -> Result<Option<i64>, sqlx::Error> {
        if ids.len() < 2 {
            return Ok(None);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let owned: Vec<(i64,)> = {
            let sql = format!(
                "SELECT id FROM sessions WHERE user_id = ? AND id IN ({}) ORDER BY started_at ASC",
                placeholders
            );
            let mut q = sqlx::query_as(&sql).bind(user_id);
            for id in ids {
                q = q.bind(id);
            }
            q.fetch_all(&self.pool).await?
        };
        if owned.len() != ids.len() {
            return Ok(None);
        }

        let target = owned[0].0;
        let others: Vec<i64> = owned[1..].iter().map(|(id,)| *id).collect();
        let others_ph = vec!["?"; others.len()].join(",");

        {
            let sql = format!(
                "UPDATE scrobbles SET session_id = ? WHERE user_id = ? AND session_id IN ({})",
                others_ph
            );
            let mut q = sqlx::query(&sql).bind(target).bind(user_id);
            for id in &others {
                q = q.bind(id);
            }
            q.execute(&self.pool).await?;
        }

        // Now empty; the recompute deletes each one.
        for id in &others {
            self.recompute_session(*id).await?;
        }
        self.recompute_session(target).await?;
        Ok(Some(target))
    }

    /// Cut a session in two at `at_scrobble_id`, which becomes the first listen
    /// of the new session.
    ///
    /// Returns the new session's id. Splitting at the very first listen is a
    /// no-op rather than an error: it would leave one empty half, which is just
    /// the session you already had.
    pub async fn split_session(
        &self,
        user_id: i64,
        session_id: i64,
        at_scrobble_id: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        let listens = self.session_scrobbles(user_id, session_id).await?;
        let Some(pos) = listens.iter().position(|s| s.id == at_scrobble_id) else {
            return Ok(None);
        };
        if pos == 0 {
            return Ok(None);
        }

        let old: Option<(i64, Option<i64>, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT started_at, device_id, chain_id, listening_context
               FROM sessions WHERE id = ? AND user_id = ?",
        )
        .bind(session_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((_, device_id, chain_id, context)) = old else {
            return Ok(None);
        };

        // The tail inherits the setup — a split says "this was a separate
        // sitting", not "this was different gear".
        let moving: Vec<i64> = listens[pos..].iter().map(|s| s.id).collect();
        let started = listens[pos].timestamp;

        let res = sqlx::query(
            "INSERT INTO sessions (user_id, device_id, chain_id, started_at, ended_at,
                track_count, total_duration, listening_context, uuid, updated_at)
             VALUES (?, ?, ?, ?, ?, 0, 0, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(device_id)
        .bind(chain_id)
        .bind(started)
        .bind(started)
        .bind(context.unwrap_or_else(|| "unknown".into()))
        .bind(new_uuid())
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        let new_id = res.last_insert_rowid();

        let ph = vec!["?"; moving.len()].join(",");
        let sql =
            format!("UPDATE scrobbles SET session_id = ? WHERE user_id = ? AND id IN ({})", ph);
        let mut q = sqlx::query(&sql).bind(new_id).bind(user_id);
        for id in &moving {
            q = q.bind(id);
        }
        q.execute(&self.pool).await?;

        self.recompute_session(session_id).await?;
        self.recompute_session(new_id).await?;
        Ok(Some(new_id))
    }

    /// Retag a whole session's listens onto a chain, and the session with them.
    ///
    /// "This whole sitting was on the desk rig" is the edit sessions exist to
    /// make cheap — the alternative is shift-selecting forty rows in history.
    pub async fn set_session_chain(
        &self,
        user_id: i64,
        session_id: i64,
        chain_id: Option<i64>,
    ) -> Result<u64, sqlx::Error> {
        let res =
            sqlx::query("UPDATE scrobbles SET chain_id = ? WHERE user_id = ? AND session_id = ?")
                .bind(chain_id)
                .bind(user_id)
                .bind(session_id)
                .execute(&self.pool)
                .await?;
        sqlx::query(
            "UPDATE sessions SET chain_id = ?, updated_at = ? WHERE id = ? AND user_id = ?",
        )
        .bind(chain_id)
        .bind(Self::now_secs())
        .bind(session_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    /// Name a session. `None` clears it back to the derived label.
    pub async fn set_session_title(
        &self,
        user_id: i64,
        session_id: i64,
        title: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE sessions SET title = ?, updated_at = ? WHERE id = ? AND user_id = ?",
        )
        .bind(title.map(str::trim).filter(|t| !t.is_empty()))
        .bind(Self::now_secs())
        .bind(session_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// One ranked chart: artists, albums or tracks over a window.
    ///
    /// Separate from `get_ranged_stats` because the charts page wants *one* long
    /// list and none of the breakdowns, heatmap or personality that query also
    /// computes — asking for all of it to render fifty rows would be wasteful on
    /// a Pi.
    pub async fn chart(
        &self,
        user_id: i64,
        kind: &str,
        after: Option<i64>,
        limit: i64,
        by_hours: bool,
    ) -> Result<Vec<ChartRow>, sqlx::Error> {
        // Same reasoning as `get_ranged_stats`: the chart is cut to `limit`
        // rows, so ranking by the chosen metric has to happen in SQL. It also
        // has to agree with Statistics — "See more" hands off from there
        // carrying its metric, and landing on a differently-ranked list would
        // be the exact inconsistency that handoff exists to remove.
        let order = if by_hours { "seconds DESC, plays DESC" } else { "plays DESC" };
        // Every branch selects the same shape so the caller doesn't care which
        // ran. `sub` is the secondary line (an album's artist), null for artists.
        let sql: String = match kind {
            "albums" => {
                format!(
                    "SELECT album AS name, artist AS sub, NULL AS album, COUNT(*) AS plays,
                        COALESCE(SUM(duration), 0) AS seconds,
                        MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                        MAX(caa_release_mbid) AS caa_release_mbid
                 FROM scrobbles
                 WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                   AND album IS NOT NULL AND TRIM(album) != ''
                 GROUP BY LOWER(TRIM(album)), LOWER(TRIM(artist))
                 ORDER BY {order} LIMIT ?"
                )
            }
            "tracks" => {
                format!(
                    "SELECT title AS name, artist AS sub, MAX(album) AS album, COUNT(*) AS plays,
                        COALESCE(SUM(duration), 0) AS seconds,
                        MAX(artwork_url) AS artwork_url, MAX(caa_id) AS caa_id,
                        MAX(caa_release_mbid) AS caa_release_mbid
                 FROM scrobbles
                 WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                 GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
                 ORDER BY {order} LIMIT ?"
                )
            }
            _ => {
                format!(
                    "SELECT artist AS name, NULL AS sub, NULL AS album, COUNT(*) AS plays,
                        COALESCE(SUM(duration), 0) AS seconds,
                        NULL AS artwork_url, NULL AS caa_id, NULL AS caa_release_mbid
                 FROM scrobbles
                 WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                 GROUP BY LOWER(TRIM(artist))
                 ORDER BY {order} LIMIT ?"
                )
            }
        };
        let mut rows: Vec<ChartRow> = sqlx::query_as(&sql)
            .bind(user_id)
            .bind(after)
            .bind(after)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        // An artist's portrait doesn't live on `scrobbles` — a listen carries
        // its *cover*, which is the album's. It lives on `artists`, filled by
        // the metadata chain, which is why the artists branch above selects a
        // literal NULL. Statistics has always filled it in this second step
        // (see `ranged_stats`); the charts screen never did, so every artist
        // row fell back to the lettered tile.
        if kind == "artists" {
            let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
            let images = self.artist_images(&names).await.unwrap_or_default();
            for row in &mut rows {
                row.artwork_url = images.get(&Self::artist_key(&row.name)).cloned();
            }
        }
        Ok(rows)
    }

    /// A track in this user's history matched by name.
    ///
    /// The fallback for services that report names rather than MBIDs (Last.fm).
    /// Returns the *stored* spelling so the entity key matches everything else
    /// keyed off this listen, rather than the remote service's capitalisation.
    pub async fn track_by_name(
        &self,
        user_id: i64,
        title: &str,
        artist: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title, artist FROM scrobbles
             WHERE user_id = ? AND LOWER(TRIM(title)) = ? AND LOWER(TRIM(artist)) = ?
             ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(title.trim().to_lowercase())
        .bind(artist.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn track_by_recording_mbid(
        &self,
        user_id: i64,
        mbid: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title, artist FROM scrobbles
             WHERE user_id = ? AND mbid_recording = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(mbid)
        .fetch_optional(&self.pool)
        .await
    }

    /// The album this user most recently heard a given track on. Supplies the
    /// album context for a note about a *track*, so `[[track:3]]` still means
    /// "the third track of the record this one is from".
    pub async fn album_for_track(
        &self,
        user_id: i64,
        title: &str,
        artist: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT album FROM scrobbles
             WHERE user_id = ? AND LOWER(TRIM(title)) = ? AND LOWER(TRIM(artist)) = ?
               AND album IS NOT NULL AND album <> ''
             ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(title.trim().to_lowercase())
        .bind(artist.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(album,)| album))
    }

    /// Write a note, keeping what it said before.
    ///
    /// One note per user per target: the second write edits the first rather
    /// than stacking up, and the previous body is archived first so an edit
    /// never destroys what was there. An unchanged body writes no revision —
    /// toggling visibility isn't a rewrite.
    pub async fn upsert_note(
        &self,
        user_id: i64,
        entity_id: Option<i64>,
        scrobble_id: Option<i64>,
        body_md: &str,
        visibility: &str,
    ) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let existing = match (entity_id, scrobble_id) {
            (Some(e), _) => self.note_for_entity(user_id, e).await?,
            (_, Some(s)) => self.note_for_scrobble(user_id, s).await?,
            _ => None,
        };

        if let Some(note) = existing {
            if note.body_md != body_md {
                sqlx::query(
                    "INSERT INTO note_revisions (note_id, body_md, created_at) VALUES (?, ?, ?)",
                )
                .bind(note.id)
                .bind(&note.body_md)
                .bind(note.updated_at)
                .execute(&self.pool)
                .await?;
            }
            sqlx::query("UPDATE notes SET body_md = ?, visibility = ?, updated_at = ? WHERE id = ? AND user_id = ?")
                .bind(body_md).bind(visibility).bind(now).bind(note.id).bind(user_id)
                .execute(&self.pool).await?;
            return Ok(note.id);
        }

        let result = sqlx::query(
            "INSERT INTO notes (user_id, entity_id, scrobble_id, body_md, visibility, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(user_id).bind(entity_id).bind(scrobble_id).bind(body_md).bind(visibility)
        .bind(now).bind(now)
        .execute(&self.pool).await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn get_note(&self, user_id: i64, id: i64) -> Result<Option<NoteRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM notes WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn note_for_entity(
        &self,
        user_id: i64,
        entity_id: i64,
    ) -> Result<Option<NoteRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM notes WHERE user_id = ? AND entity_id = ?")
            .bind(user_id)
            .bind(entity_id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn note_for_scrobble(
        &self,
        user_id: i64,
        scrobble_id: i64,
    ) -> Result<Option<NoteRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM notes WHERE user_id = ? AND scrobble_id = ?")
            .bind(user_id)
            .bind(scrobble_id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list_notes(&self, user_id: i64) -> Result<Vec<NoteRow>, sqlx::Error> {
        self.search_notes(user_id, "", "", "updated").await
    }

    /// Notes matching a query and a kind, in the requested order.
    ///
    /// Filtered here rather than in the handler because rendering one note costs
    /// four more queries (its entity, its listen, whether it's loved, how many
    /// revisions it has) — so narrowing a few hundred notes to a dozen before
    /// that loop is the difference between a search being instant and being the
    /// slowest screen in the app.
    ///
    /// `q` matches the note's **body and its subject both**: someone looking for
    /// what they wrote about Burial means either "the word Burial appears in a
    /// note" or "this is the Burial note", and being asked which is a worse
    /// answer than searching both. The joins are `LEFT` because exactly one of
    /// `entity_id` / `scrobble_id` is ever set.
    ///
    /// Case-insensitivity is SQLite's `LIKE`, which folds ASCII only — "BURIAL"
    /// finds "Burial", but "CŒUR" does not find "Cœur". Doing better means
    /// carrying a folded column, which is a bigger change than this earns.
    pub async fn search_notes(
        &self,
        user_id: i64,
        q: &str,
        kind: &str,
        sort: &str,
    ) -> Result<Vec<NoteRow>, sqlx::Error> {
        let order = match sort {
            "created" => "n.created_at DESC",
            // Alphabetical by subject. Notes on a listen have no entity name, so
            // they sort last rather than under an empty string at the top.
            "title" => "e.name IS NULL, LOWER(e.name) ASC, n.updated_at DESC",
            _ => "n.updated_at DESC",
        };
        // `kind` is one of the stored entity kinds, or `listen` for a note about
        // one particular playing. Anything else means "no filter" rather than an
        // error — a stale bookmark should show everything, not nothing.
        let kind_clause = match kind {
            "recording" | "release" | "artist" => "e.kind = ?",
            "listen" => "n.scrobble_id IS NOT NULL AND ? IS NOT NULL",
            _ => "? IS NOT NULL",
        };
        let kind_bind = match kind {
            "recording" | "release" | "artist" => kind.to_string(),
            _ => "1".to_string(),
        };

        // `%` and `_` are wildcards to LIKE, so a note about "50_50" must not
        // become a pattern matching everything. Escaped by hand — `str::replace`
        // has no capture groups, and reaching for a regex to quote three
        // characters would be its own mistake.
        let mut escaped = String::with_capacity(q.len());
        for c in q.trim().chars() {
            if matches!(c, '\\' | '%' | '_') {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        let like = format!("%{escaped}%");
        let sql = format!(
            "SELECT n.* FROM notes n
             LEFT JOIN entities e ON e.id = n.entity_id
             LEFT JOIN scrobbles s ON s.id = n.scrobble_id
             WHERE n.user_id = ?
               AND ({kind_clause})
               AND (? = ''
                    OR n.body_md LIKE ? ESCAPE '\\'
                    OR e.name LIKE ? ESCAPE '\\'
                    OR e.artist_name LIKE ? ESCAPE '\\'
                    OR s.title LIKE ? ESCAPE '\\'
                    OR s.artist LIKE ? ESCAPE '\\')
             ORDER BY {order}"
        );

        let mut query = sqlx::query_as(&sql).bind(user_id).bind(kind_bind).bind(q.trim());
        for _ in 0..5 {
            query = query.bind(like.clone());
        }
        query.fetch_all(&self.pool).await
    }

    /// Revisions newest-first, scoped through the note's owner so an id from
    /// another user's note reads as "no revisions" rather than as history.
    pub async fn note_revisions(
        &self,
        user_id: i64,
        note_id: i64,
    ) -> Result<Vec<NoteRevisionRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT r.id, r.note_id, r.body_md, r.created_at FROM note_revisions r
             JOIN notes n ON n.id = r.note_id
             WHERE r.note_id = ? AND n.user_id = ? ORDER BY r.created_at DESC, r.id DESC",
        )
        .bind(note_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Foreign keys are on, so the revisions have to go first.
    pub async fn delete_note(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        if self.get_note(user_id, id).await?.is_none() {
            return Ok(false);
        }
        sqlx::query("DELETE FROM note_revisions WHERE note_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        let result = sqlx::query("DELETE FROM notes WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn set_love(
        &self,
        user_id: i64,
        entity_id: i64,
        loved: bool,
    ) -> Result<(), sqlx::Error> {
        if loved {
            sqlx::query(
                "INSERT OR IGNORE INTO loves (user_id, entity_id, created_at) VALUES (?, ?, ?)",
            )
            .bind(user_id)
            .bind(entity_id)
            .bind(Self::now_secs())
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("DELETE FROM loves WHERE user_id = ? AND entity_id = ?")
                .bind(user_id)
                .bind(entity_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// A cached description, and how old it is.
    ///
    /// A row with `bio IS NULL` is a **tombstone**: every provider was asked and
    /// none had anything. Same discipline as `artwork_url = ''` and
    /// `caa_id = -1` — without it, the records nobody has written about get
    /// re-asked on every page view forever.
    pub async fn get_bio(&self, entity_id: i64) -> Result<Option<BioRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT entity_id, bio, attribution, provider, url, fetched_at
               FROM bio_cache WHERE entity_id = ?",
        )
        .bind(entity_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn set_bio(
        &self,
        entity_id: i64,
        bio: Option<&str>,
        attribution: Option<&str>,
        provider: Option<&str>,
        url: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO bio_cache (entity_id, bio, attribution, provider, url, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(entity_id) DO UPDATE SET
                bio = excluded.bio, attribution = excluded.attribution,
                provider = excluded.provider, url = excluded.url,
                fetched_at = excluded.fetched_at",
        )
        .bind(entity_id)
        .bind(bio)
        .bind(attribution)
        .bind(provider)
        .bind(url)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Identity keys of everything this user loves, for the row hearts.
    ///
    /// Built with the same `kind|artist|name` shape as `entity_key` so the
    /// client can derive a listen's key without a round-trip. `entity_key` is
    /// stored, so this is a plain column read rather than a recomputation that
    /// could drift from it.
    pub async fn loved_keys(&self, user_id: i64) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT e.entity_key FROM loves l JOIN entities e ON e.id = l.entity_id
             WHERE l.user_id = ?",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(k,)| k).collect())
    }

    pub async fn is_loved(&self, user_id: i64, entity_id: i64) -> Result<bool, sqlx::Error> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM loves WHERE user_id = ? AND entity_id = ?")
                .bind(user_id)
                .bind(entity_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }

    /// Loved entities, newest first, with the timestamp of the love.
    pub async fn list_loves(&self, user_id: i64) -> Result<Vec<(EntityRow, i64)>, sqlx::Error> {
        let rows: Vec<(i64, String, String, String, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT e.id, e.kind, e.name, e.artist_name, e.mbid, e.created_at, l.created_at
             FROM loves l JOIN entities e ON e.id = l.entity_id
             WHERE l.user_id = ? ORDER BY l.created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, kind, name, artist_name, mbid, created_at, loved_at)| {
                (EntityRow { id, kind, name, artist_name, mbid, created_at }, loved_at)
            })
            .collect())
    }

    /// Every love, with what the Loved page needs to render a row: how often
    /// you have played the thing, when you last did, its artwork, and whether
    /// there is a liner note on it.
    ///
    /// Four queries regardless of how many loves there are, rather than a
    /// per-row lookup. A love is an *entity* and entities come in three kinds
    /// that live in different columns of `scrobbles`, so there is one aggregate
    /// per kind and they are zipped in Rust on the same `entity_key` the loves
    /// themselves are keyed by — see `Database::entity_key`, and keep the two in
    /// step or hearts and rows stop matching with nothing to show for it.
    ///
    /// The bare `artwork_url` / `caa_id` columns beside `MAX(timestamp)` are
    /// deliberate: SQLite guarantees those come from the row that produced the
    /// max, so a row's cover is the one its *most recent* listen carried. That
    /// is a documented min/max special case, not an accident of grouping.
    pub async fn loves_detailed(&self, user_id: i64) -> Result<Vec<LovedEntity>, sqlx::Error> {
        let loves = self.list_loves(user_id).await?;
        if loves.is_empty() {
            return Ok(Vec::new());
        }

        type Agg = (String, i64, i64, Option<String>, Option<i64>, Option<String>, Option<String>);

        // Recordings and releases differ only in which column names the thing.
        let mut by_key: HashMap<
            String,
            (i64, i64, Option<String>, Option<i64>, Option<String>, Option<String>),
        > = HashMap::new();
        for (kind, name_col) in [("recording", "title"), ("release", "album")] {
            let rows: Vec<Agg> = sqlx::query_as(&format!(
                "SELECT LOWER(TRIM(artist)) || '|' || LOWER(TRIM({name_col})) AS k,
                        COUNT(*) AS plays, MAX(timestamp) AS last_played,
                        artwork_url, caa_id, caa_release_mbid, album
                   FROM scrobbles
                  WHERE user_id = ? AND skipped = FALSE
                    AND {name_col} IS NOT NULL AND TRIM({name_col}) != ''
                  GROUP BY k"
            ))
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
            for (k, plays, last, art, caa, caa_mbid, album) in rows {
                by_key.insert(format!("{kind}|{k}"), (plays, last, art, caa, caa_mbid, album));
            }
        }
        let artist_rows: Vec<(String, i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT LOWER(TRIM(s.artist)) AS k, COUNT(*) AS plays, MAX(s.timestamp) AS last_played,
                    (SELECT a.image_url FROM artists a WHERE a.name_key = LOWER(TRIM(s.artist)))
               FROM scrobbles s
              WHERE s.user_id = ? AND s.skipped = FALSE AND TRIM(s.artist) != ''
              GROUP BY k",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        for (k, plays, last, image) in artist_rows {
            by_key.insert(format!("artist||{k}"), (plays, last, image, None, None, None));
        }

        // Notes are per (user, entity); the excerpt is what the row shows.
        let note_rows: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT entity_id, id, body_md FROM notes
              WHERE user_id = ? AND entity_id IS NOT NULL",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let notes: HashMap<i64, (i64, String)> =
            note_rows.into_iter().map(|(e, id, body)| (e, (id, body))).collect();

        Ok(loves
            .into_iter()
            .map(|(e, loved_at)| {
                let key = Self::entity_key(&e.kind, &e.artist_name, &e.name);
                let agg = by_key.get(&key);
                let note = notes.get(&e.id);
                LovedEntity {
                    id: e.id,
                    kind: e.kind,
                    name: e.name,
                    artist_name: e.artist_name,
                    mbid: e.mbid,
                    loved_at,
                    plays: agg.map(|a| a.0).unwrap_or(0),
                    last_played: agg.map(|a| a.1),
                    artwork_url: agg.and_then(|a| a.2.clone()).filter(|u| !u.is_empty()),
                    caa_id: agg.and_then(|a| a.3),
                    caa_release_mbid: agg.and_then(|a| a.4.clone()),
                    album: agg.and_then(|a| a.5.clone()),
                    note_id: note.map(|n| n.0),
                    note_excerpt: note.map(|n| {
                        let body = n.1.trim().replace('\n', " ");
                        if body.chars().count() > 90 {
                            format!("{}…", body.chars().take(90).collect::<String>())
                        } else {
                            body
                        }
                    }),
                }
            })
            .collect())
    }

    /// Lowest-numbered admin — the migration target for config that used to be
    /// instance-wide and now needs an owner.
    pub async fn first_admin_id(&self) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM users WHERE role = 'admin' AND disabled = 0 ORDER BY id LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    // ════════════════════════════════════════════════════════════
    //  Per-user sources
    // ════════════════════════════════════════════════════════════

    /// Every enabled source across all users — what the engine reconciles its
    /// registry against each tick. Credentials come back decrypted; a row we
    /// can't decrypt is dropped rather than failing the whole tick.
    pub async fn list_enabled_sources(&self) -> Result<Vec<UserSource>, sqlx::Error> {
        let rows: Vec<UserSourceRow> = sqlx::query_as(
            "SELECT id, user_id, kind, label, enabled, url, credentials, remote_account,
                    poll_interval_secs, options, last_polled_at, last_error, updated_at
             FROM user_sources WHERE enabled = 1 ORDER BY user_id, kind",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().filter_map(|r| self.hydrate_source(r)).collect())
    }

    /// This user's sources, for the Settings screen. Credentials are decrypted
    /// here too — the handler decides what is safe to send to the browser.
    pub async fn list_sources_for_user(
        &self,
        user_id: i64,
    ) -> Result<Vec<UserSource>, sqlx::Error> {
        let rows: Vec<UserSourceRow> = sqlx::query_as(
            "SELECT id, user_id, kind, label, enabled, url, credentials, remote_account,
                    poll_interval_secs, options, last_polled_at, last_error, updated_at
             FROM user_sources WHERE user_id = ? ORDER BY kind",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().filter_map(|r| self.hydrate_source(r)).collect())
    }

    /// One source by id, scoped to its owner so an id from another user is a
    /// miss rather than a leak.
    pub async fn get_source(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<UserSource>, sqlx::Error> {
        let row: Option<UserSourceRow> = sqlx::query_as(
            "SELECT id, user_id, kind, label, enabled, url, credentials, remote_account,
                    poll_interval_secs, options, last_polled_at, last_error, updated_at
             FROM user_sources WHERE user_id = ? AND id = ?",
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| self.hydrate_source(r)))
    }

    fn hydrate_source(&self, row: UserSourceRow) -> Option<UserSource> {
        let credentials =
            if row.credentials.is_empty() { String::new() } else { self.unseal(&row.credentials)? };
        Some(UserSource {
            id: row.id,
            user_id: row.user_id,
            kind: row.kind,
            label: row.label,
            enabled: row.enabled,
            url: row.url,
            credentials,
            remote_account: row.remote_account,
            poll_interval_secs: row.poll_interval_secs,
            options: row.options,
            last_polled_at: row.last_polled_at,
            last_error: row.last_error,
            updated_at: row.updated_at,
        })
    }

    /// Create a source, returning its id. `updated_at` doubles as the change
    /// stamp the engine watches, so a save is picked up on the next tick.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_source(
        &self,
        user_id: i64,
        kind: &str,
        label: &str,
        enabled: bool,
        url: &str,
        credentials: &str,
        remote_account: Option<&str>,
        poll_interval_secs: Option<i64>,
        options: &str,
    ) -> Result<i64, sqlx::Error> {
        let sealed = if credentials.is_empty() { String::new() } else { self.seal(credentials) };
        let res = sqlx::query(
            "INSERT INTO user_sources
                (user_id, kind, label, enabled, url, credentials, remote_account,
                 poll_interval_secs, options, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(kind)
        .bind(label)
        .bind(enabled)
        .bind(url)
        .bind(sealed)
        .bind(remote_account)
        .bind(poll_interval_secs)
        .bind(options)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_source(
        &self,
        user_id: i64,
        id: i64,
        label: &str,
        enabled: bool,
        url: &str,
        credentials: &str,
        remote_account: Option<&str>,
        poll_interval_secs: Option<i64>,
        options: &str,
    ) -> Result<bool, sqlx::Error> {
        let sealed = if credentials.is_empty() { String::new() } else { self.seal(credentials) };
        let res = sqlx::query(
            "UPDATE user_sources SET
                label = ?, enabled = ?, url = ?, credentials = ?, remote_account = ?,
                poll_interval_secs = ?, options = ?, updated_at = ?
             WHERE id = ? AND user_id = ?",
        )
        .bind(label)
        .bind(enabled)
        .bind(url)
        .bind(sealed)
        .bind(remote_account)
        .bind(poll_interval_secs)
        .bind(options)
        .bind(Self::now_secs())
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn delete_source(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM user_sources WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Record the outcome of a poll. Deliberately does *not* touch
    /// `updated_at` — that would look like a config change and make the engine
    /// rebuild the source on every tick.
    pub async fn record_source_poll(
        &self,
        id: i64,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE user_sources SET last_polled_at = ?, last_error = ? WHERE id = ?")
            .bind(Self::now_secs())
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ════════════════════════════════════════════════════════════
    //  Plex source settings (legacy server-wide row, migrated away)
    // ════════════════════════════════════════════════════════════

    /// The stored Plex config, or `None` before first-run seeding.
    pub async fn get_plex_settings(&self) -> Result<Option<PlexSettings>, sqlx::Error> {
        sqlx::query_as(
            "SELECT enabled, url, token, users_allow, users_block, devices_allow,
                    devices_block, libraries_allow, libraries_block, updated_at
             FROM plex_settings WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await
    }

    /// Write the single settings row. `updated_at` doubles as the change stamp
    /// the engine watches to decide when to rebuild its Plex source.
    pub async fn save_plex_settings(&self, s: &PlexSettings) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO plex_settings
                (id, enabled, url, token, users_allow, users_block, devices_allow,
                 devices_block, libraries_allow, libraries_block, updated_at)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                enabled=excluded.enabled, url=excluded.url, token=excluded.token,
                users_allow=excluded.users_allow, users_block=excluded.users_block,
                devices_allow=excluded.devices_allow, devices_block=excluded.devices_block,
                libraries_allow=excluded.libraries_allow, libraries_block=excluded.libraries_block,
                updated_at=excluded.updated_at",
        )
        .bind(s.enabled)
        .bind(&s.url)
        .bind(&s.token)
        .bind(&s.users_allow)
        .bind(&s.users_block)
        .bind(&s.devices_allow)
        .bind(&s.devices_block)
        .bind(&s.libraries_allow)
        .bind(&s.libraries_block)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a new import as `running` and return its row id. Persisting at
    /// start (not just completion) means an interrupted import leaves a durable
    /// record instead of a job that vanishes with the process.
    pub async fn create_import_job(&self, user_id: i64, source: &str) -> Result<i64, sqlx::Error> {
        let res = sqlx::query(
            "INSERT INTO import_jobs (user_id, source, state, fetched, imported, status, created_at)
             VALUES (?, ?, 'running', 0, 0, 'started', ?)"
        ).bind(user_id).bind(source).bind(Self::now_secs()).execute(&self.pool).await?;
        Ok(res.last_insert_rowid())
    }

    /// Update a job to its final state on completion.
    pub async fn finish_import_job(
        &self,
        id: i64,
        state: &str,
        fetched: i64,
        imported: i64,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE import_jobs SET state = ?, fetched = ?, imported = ?, status = ? WHERE id = ?",
        )
        .bind(state)
        .bind(fetched)
        .bind(imported)
        .bind(status)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// On startup, any import still marked `running` died with the previous
    /// process — mark it interrupted so history is accurate and nothing lingers.
    pub async fn mark_running_imports_interrupted(&self) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE import_jobs SET state = 'interrupted', status = 'interrupted by shutdown' WHERE state = 'running'"
        ).execute(&self.pool).await?;
        Ok(res.rows_affected())
    }

    pub async fn list_imports(&self, user_id: i64) -> Result<Vec<ImportHistoryRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM import_jobs WHERE user_id = ? ORDER BY created_at DESC LIMIT 50",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Bulk-reassign the chain for a set of listens (History bulk edit).
    pub async fn bulk_set_scrobble_chain(
        &self,
        user_id: i64,
        ids: &[i64],
        chain_id: Option<i64>,
    ) -> Result<u64, sqlx::Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "UPDATE scrobbles SET chain_id = ? WHERE user_id = ? AND id IN ({})",
            placeholders
        );
        let mut q = sqlx::query(&sql).bind(chain_id).bind(user_id);
        for id in ids {
            q = q.bind(id);
        }
        let res = q.execute(&self.pool).await?;
        Ok(res.rows_affected())
    }

    // ════════════════════════════════════════════════════════════
    //  Devices
    // ════════════════════════════════════════════════════════════

    pub async fn upsert_device(
        &self,
        user_id: i64,
        machine_id: &str,
        name: Option<&str>,
        platform: Option<&str>,
        product: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM devices WHERE user_id = ? AND machine_id = ?")
                .bind(user_id)
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((id,)) = existing {
            sqlx::query("UPDATE devices SET last_seen = ?, total_listens = total_listens + 1,
                 name = COALESCE(?, name), platform = COALESCE(?, platform), product = COALESCE(?, product) WHERE id = ?")
            .bind(now).bind(name).bind(platform).bind(product).bind(id).execute(&self.pool).await?;
            Ok(id)
        } else {
            let result = sqlx::query("INSERT INTO devices (user_id, machine_id, name, platform, product, first_seen, last_seen, total_listens)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 1)")
            .bind(user_id).bind(machine_id).bind(name).bind(platform).bind(product).bind(now).bind(now)
            .execute(&self.pool).await?;
            Ok(result.last_insert_rowid())
        }
    }

    /// Look a device up without creating or touching it.
    ///
    /// `upsert_device` is the normal path and it *writes* — it bumps
    /// `last_seen` and `total_listens`. A dry-run submit has to resolve the
    /// chain ladder's device rung without leaving a trace, or "tell me what
    /// would happen" would itself change what happens.
    pub async fn find_device(
        &self,
        user_id: i64,
        machine_id: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM devices WHERE user_id = ? AND machine_id = ?")
                .bind(user_id)
                .bind(machine_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(id,)| id))
    }

    pub async fn get_devices(&self, user_id: i64) -> Result<Vec<DeviceRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM devices WHERE user_id = ? ORDER BY last_seen DESC")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
    }

    // ════════════════════════════════════════════════════════════
    //  Equipment
    // ════════════════════════════════════════════════════════════

    pub async fn upsert_equipment(
        &self,
        user_id: i64,
        name: &str,
        equipment_type: &str,
        brand: Option<&str>,
        model: Option<&str>,
        hours_to_add: f64,
        purchased_at: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM equipment WHERE user_id = ? AND name = ?")
                .bind(user_id)
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        if let Some((id,)) = existing {
            sqlx::query(
                "UPDATE equipment SET total_hours = total_hours + ?, last_used = ? WHERE id = ?",
            )
            .bind(hours_to_add)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(id)
        } else {
            let result = sqlx::query("INSERT INTO equipment (user_id, name, type, brand, model, total_hours, first_used, last_used, purchased_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(user_id).bind(name).bind(equipment_type).bind(brand).bind(model)
            .bind(hours_to_add).bind(now).bind(now).bind(purchased_at).execute(&self.pool).await?;
            Ok(result.last_insert_rowid())
        }
    }

    /// Resolve a `[[gear:…]]` shortcode to one of the user's equipment rows.
    ///
    /// Unlike `resolve_reference` this never *creates* anything: gear is
    /// something you own and catalogue deliberately, so a note naming a piece
    /// you haven't added should render as unresolved rather than conjuring an
    /// empty record with no type or brand. Matches the name first, then the
    /// model, so "[[gear:HD 650]]" finds it whichever field it was entered in.
    pub async fn find_equipment_by_name(
        &self,
        user_id: i64,
        needle: &str,
    ) -> Result<Option<(i64, String)>, sqlx::Error> {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(None);
        }
        sqlx::query_as(
            "SELECT id, name FROM equipment
             WHERE user_id = ? AND (LOWER(TRIM(name)) = ? OR LOWER(TRIM(model)) = ?)
             ORDER BY (LOWER(TRIM(name)) = ?) DESC, id ASC LIMIT 1",
        )
        .bind(user_id)
        .bind(&needle)
        .bind(&needle)
        .bind(&needle)
        .fetch_optional(&self.pool)
        .await
    }

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
    pub async fn get_equipment(&self, user_id: i64) -> Result<Vec<EquipmentRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT e.id, e.user_id, e.name, e.type, e.brand, e.model,
                    CAST(COALESCE((
                        SELECT SUM((
                            SELECT COALESCE(SUM(sc.duration), 0) FROM scrobbles sc
                             WHERE sc.user_id = c.user_id AND sc.chain_id = c.id
                               AND sc.skipped = FALSE
                        ) / 3600.0)
                          FROM signal_chains c
                         WHERE c.user_id = e.user_id
                           AND EXISTS (
                               SELECT 1 FROM json_each(c.components) j
                                WHERE json_extract(j.value, '$.equipment_id') = e.id
                           )
                    ), 0) AS REAL) AS total_hours,
                    e.first_used, e.last_used, e.notes, e.purchased_at
               FROM equipment e
              WHERE e.user_id = ?
              ORDER BY total_hours DESC, e.name ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_equipment(
        &self,
        user_id: i64,
        id: i64,
        name: &str,
        equipment_type: &str,
        brand: Option<&str>,
        model: Option<&str>,
        notes: Option<&str>,
        purchased_at: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE equipment SET name = ?, type = ?, brand = ?, model = ?, notes = ?,
                    purchased_at = ?
              WHERE id = ? AND user_id = ?",
        )
        .bind(name)
        .bind(equipment_type)
        .bind(brand)
        .bind(model)
        .bind(notes)
        .bind(purchased_at)
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn delete_equipment(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM equipment WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    // ════════════════════════════════════════════════════════════
    //  Listening Sessions
    // ════════════════════════════════════════════════════════════

    pub async fn assign_session(
        &self,
        user_id: i64,
        timestamp: i64,
        duration_secs: i64,
        device_id: Option<i64>,
        chain_id: Option<i64>,
        quality_score: Option<f64>,
        listening_context: &str,
        session_gap_seconds: i64,
        token_id: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        // A session is contiguous listening *through one setup*. The gap is the
        // usual test, but a change of submitting token ends it too: one token
        // per app is how these are organised, so "fooyin stopped, Pano started"
        // is a different setup even if it happened a minute later. A browser
        // submission carries no token and is treated as its own bucket.
        let recent: Option<(i64, i64, Option<i64>)> = sqlx::query_as(
            "SELECT id, ended_at, token_id FROM sessions WHERE user_id = ? AND ended_at >= ? - ? ORDER BY ended_at DESC LIMIT 1"
        ).bind(user_id).bind(timestamp).bind(session_gap_seconds).fetch_optional(&self.pool).await?;
        let ended_at = timestamp + duration_secs;
        let recent = recent.filter(|(_, _, prev_token)| *prev_token == token_id);
        if let Some((session_id, _, _)) = recent {
            sqlx::query(
                "UPDATE sessions SET ended_at = MAX(ended_at, ?), track_count = track_count + 1,
                    total_duration = total_duration + ?,
                    avg_quality_score = CASE WHEN ? IS NOT NULL THEN
                        (COALESCE(avg_quality_score, 0) * track_count + ?) / (track_count + 1)
                    ELSE avg_quality_score END WHERE id = ?",
            )
            .bind(ended_at)
            .bind(duration_secs)
            .bind(quality_score)
            .bind(quality_score)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
            Ok(session_id)
        } else {
            let result = sqlx::query(
                "INSERT INTO sessions (user_id, device_id, chain_id, started_at, ended_at,
                    track_count, total_duration, avg_quality_score, listening_context, token_id,
                    uuid, updated_at)
                 VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(device_id)
            .bind(chain_id)
            .bind(timestamp)
            .bind(ended_at)
            .bind(duration_secs)
            .bind(quality_score)
            .bind(listening_context)
            .bind(token_id)
            // Every session gets its stable id at birth — see the migration.
            .bind(new_uuid())
            .bind(Self::now_secs())
            .execute(&self.pool)
            .await?;
            Ok(result.last_insert_rowid())
        }
    }

    // ════════════════════════════════════════════════════════════
    //  User & Token Management
    // ════════════════════════════════════════════════════════════

    fn hash_token(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn now_secs() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    /// Resolve a raw API token to its owner **and** the signal chain that token
    /// defaults to. The chain rides along because the ingest path needs it on
    /// every submission and this is already the one query that touches the row.
    pub async fn validate_token(&self, token: &str) -> Result<Option<TokenAuth>, sqlx::Error> {
        let token_hash = Self::hash_token(token);
        let now = Self::now_secs();
        let row: Option<(i64, String, String, i64, Option<i64>, Option<String>, Option<bool>)> =
            sqlx::query_as(
                "SELECT u.id, u.username, u.role, t.id, t.default_chain_id, t.scopes,
                        t.imports_are_live
             FROM tokens t JOIN users u ON t.user_id = u.id
             WHERE t.token_hash = ? AND (t.expires_at IS NULL OR t.expires_at > ?) AND u.disabled = 0"
            ).bind(&token_hash).bind(now).fetch_optional(&self.pool).await?;

        if let Some((
            user_id,
            username,
            role,
            token_id,
            default_chain_id,
            scopes,
            imports_are_live,
        )) = row
        {
            let _ = sqlx::query("UPDATE tokens SET last_used_at = ? WHERE token_hash = ?")
                .bind(now)
                .bind(&token_hash)
                .execute(&self.pool)
                .await;
            Ok(Some(TokenAuth {
                user: AuthUser { user_id, username, is_admin: role == "admin" },
                token_id,
                default_chain_id,
                scopes: scopes.unwrap_or_default(),
                imports_are_live: imports_are_live.unwrap_or(false),
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn create_user(
        &self,
        username: &str,
        display_name: Option<&str>,
        password_hash: &str,
        role: &str,
    ) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let result = sqlx::query(
            "INSERT INTO users (username, display_name, password_hash, role, created_at) VALUES (?, ?, ?, ?, ?)"
        ).bind(username).bind(display_name).bind(password_hash).bind(role).bind(now)
        .execute(&self.pool).await?;
        Ok(result.last_insert_rowid())
    }

    /// Returns (role, disabled) for a user, if they exist.
    pub async fn get_user_role_disabled(
        &self,
        id: i64,
    ) -> Result<Option<(String, bool)>, sqlx::Error> {
        let row: Option<(String, bool)> =
            sqlx::query_as("SELECT role, disabled FROM users WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }

    /// Count admins who can actually log in (enabled). Used to guard against
    /// leaving the instance with no reachable admin.
    pub async fn count_active_admins(&self) -> Result<i64, sqlx::Error> {
        let c: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0")
                .fetch_one(&self.pool)
                .await?;
        Ok(c.0)
    }

    pub async fn update_user_role(&self, id: i64, role: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(role)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn set_user_disabled(&self, id: i64, disabled: bool) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE users SET disabled = ? WHERE id = ?")
            .bind(disabled)
            .bind(id)
            .execute(&self.pool)
            .await?;
        // Disabling a user invalidates their live web sessions immediately.
        if disabled {
            let _ = sqlx::query("DELETE FROM web_sessions WHERE user_id = ?")
                .bind(id)
                .execute(&self.pool)
                .await;
        }
        Ok(res.rows_affected() > 0)
    }

    pub async fn update_user_password(
        &self,
        id: i64,
        password_hash: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Drop every browser session a user holds.
    ///
    /// Called on a password change: that is normally a reaction to "someone
    /// else may have access", and a new password that leaves the old sessions
    /// logged in does not actually take anything away from them.
    pub async fn clear_user_sessions(&self, id: i64) -> Result<u64, sqlx::Error> {
        let res = sqlx::query("DELETE FROM web_sessions WHERE user_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// A user's own profile, for the account screen.
    pub async fn get_profile(&self, id: i64) -> Result<Option<UserProfile>, sqlx::Error> {
        sqlx::query_as::<_, UserProfile>(
            "SELECT id, username, display_name, role, created_at, avatar_url, bio, disabled
               FROM users WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Update the parts of a profile the owner controls.
    ///
    /// Each field is `Option<Option<..>>` for the reason recorded against
    /// `bulk-chain` and `default_chain_id`: on an endpoint where a field
    /// chooses between "change this" and "leave it alone", a bare `Option`
    /// cannot tell absent from null, and a misspelled key silently clears
    /// whatever it failed to name. Absent leaves alone; explicit null clears.
    ///
    /// Note what is *not* here: `role`, `disabled`, `username` and
    /// `password_hash`. Those are either admin decisions or need the current
    /// password, and neither belongs on the same call as a bio.
    pub async fn update_profile(
        &self,
        id: i64,
        display_name: Option<Option<String>>,
        bio: Option<Option<String>>,
        avatar_url: Option<Option<String>>,
    ) -> Result<bool, sqlx::Error> {
        let mut changed = false;
        if let Some(v) = display_name {
            sqlx::query("UPDATE users SET display_name = ? WHERE id = ?")
                .bind(v)
                .bind(id)
                .execute(&self.pool)
                .await?;
            changed = true;
        }
        if let Some(v) = bio {
            sqlx::query("UPDATE users SET bio = ? WHERE id = ?")
                .bind(v)
                .bind(id)
                .execute(&self.pool)
                .await?;
            changed = true;
        }
        if let Some(v) = avatar_url {
            sqlx::query("UPDATE users SET avatar_url = ? WHERE id = ?")
                .bind(v)
                .bind(id)
                .execute(&self.pool)
                .await?;
            changed = true;
        }
        Ok(changed)
    }

    /// Verify a password against the stored hash for one user.
    ///
    /// This is what makes "change my password" different from an admin reset:
    /// the caller already holds a session, so without it a borrowed browser is
    /// enough to lock the owner out of their own account.
    pub async fn check_user_password(&self, id: i64, password: &str) -> bool {
        let row: Result<(String,), _> =
            sqlx::query_as("SELECT password_hash FROM users WHERE id = ? AND disabled = 0")
                .bind(id)
                .fetch_one(&self.pool)
                .await;
        matches!(row, Ok((hash,)) if Self::verify_password(password, &hash))
    }

    /// Delete a user and everything they own, in one transaction. Every listen,
    /// token, chain, device, session, binding, and import record scoped to the
    /// user is removed first so foreign keys stay consistent.
    pub async fn delete_user(&self, id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Revisions reference notes, and notes reference scrobbles, so both have
        // to be cleared before the rows they point at. Entities are shared
        // metadata and stay; the loves and notes that pointed at them go.
        sqlx::query(
            "DELETE FROM note_revisions WHERE note_id IN (SELECT id FROM notes WHERE user_id = ?)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        for table in [
            "notes",
            "loves",
            "scrobbles",
            // The shelf goes before `equipment` and `signal_chains`, which both
            // of these point at — a play records the turntable it ran on, and a
            // pressing records what it is normally played through. Clearing the
            // gear first is a foreign-key violation, which with `physical_plays`
            // absent from this list is what deleting anyone with a shelf did.
            "physical_images",
            "physical_plays",
            // Points at a release, a chain and a piece of gear, so it clears
            // before all three — same trap `physical_plays` was missing from.
            "physical_spins",
            "physical_releases",
            "tokens",
            "signal_chains",
            "devices",
            "equipment",
            "sessions",
            "chain_bindings",
            "import_jobs",
            "web_sessions",
            "user_connections",
        ] {
            sqlx::query(&format!("DELETE FROM {} WHERE user_id = ?", table))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        let res = sqlx::query("DELETE FROM users WHERE id = ?").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    // ════════════════════════════════════════════════════════════
    //  Device-code pairing
    // ════════════════════════════════════════════════════════════

    /// How long a pairing request lives. Short on purpose: the user code is
    /// only eight characters, so its safety comes from the window being small,
    /// the code being single-use, and polling being rate limited — not from
    /// entropy.
    pub const PAIRING_TTL_SECS: i64 = 300;

    /// Start a pairing. Returns `(device_code, user_code)`.
    ///
    /// The device code is the bearer secret the client polls with and is stored
    /// hashed; the user code is what a human carries between two screens.
    pub async fn start_pairing(
        &self,
        client_name: &str,
        scopes: &str,
    ) -> Result<(String, String), sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let device_code = format!("tdp_{}", hex::encode(raw));

        // No vowels and no 0/O/1/I/L: the whole point is reading it off one
        // screen and typing it into another without a transcription error, and
        // excluding those also means it cannot accidentally spell anything.
        const ALPHABET: &[u8] = b"BCDFGHJKMNPQRSTVWXYZ23456789";
        let now = Self::now_secs();
        // Retry on the (vanishingly unlikely) collision rather than failing the
        // request — the unique index is what makes that safe.
        for _ in 0..8 {
            let code: String = (0..8)
                .map(|i| {
                    let c = ALPHABET[rand::rng().random_range(0..ALPHABET.len())] as char;
                    if i == 4 {
                        format!("-{c}")
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            let res = sqlx::query(
                "INSERT INTO pairing_requests
                    (device_code_hash, user_code, client_name, scopes, created_at, expires_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Self::hash_token(&device_code))
            .bind(&code)
            .bind(client_name)
            .bind(scopes)
            .bind(now)
            .bind(now + Self::PAIRING_TTL_SECS)
            .execute(&self.pool)
            .await;
            match res {
                Ok(_) => return Ok((device_code, code)),
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => continue,
                Err(e) => return Err(e),
            }
        }
        Err(sqlx::Error::Protocol("could not allocate a pairing code".into()))
    }

    /// What a user code refers to, for the approval screen. Expired requests
    /// are not returned.
    pub async fn pairing_by_user_code(
        &self,
        user_code: &str,
    ) -> Result<Option<(i64, String, String, bool)>, sqlx::Error> {
        let row: Option<(i64, String, String, Option<i64>)> = sqlx::query_as(
            "SELECT id, client_name, scopes, approved_at FROM pairing_requests
              WHERE user_code = ? AND expires_at > ?",
        )
        .bind(user_code.trim().to_uppercase())
        .bind(Self::now_secs())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(id, name, scopes, approved)| (id, name, scopes, approved.is_some())))
    }

    /// Approve a pairing and mint its token.
    ///
    /// The token is stored on the request for the client to collect exactly
    /// once. Returns `false` when the code is unknown, expired, or already
    /// approved — a second approval must not mint a second token.
    pub async fn approve_pairing(
        &self,
        user_code: &str,
        user_id: i64,
        default_chain_id: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let now = Self::now_secs();
        let row: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT id, client_name, scopes FROM pairing_requests
              WHERE user_code = ? AND expires_at > ? AND approved_at IS NULL",
        )
        .bind(user_code.trim().to_uppercase())
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, client_name, scopes)) = row else {
            return Ok(false);
        };

        // A paired client starts as a backfill source like any other; the
        // operator flips it live from Settings once they know what it is.
        let token =
            self.create_token(user_id, &client_name, &scopes, default_chain_id, false).await?;
        sqlx::query(
            "UPDATE pairing_requests
                SET user_id = ?, default_chain_id = ?, token = ?, approved_at = ?
              WHERE id = ? AND approved_at IS NULL",
        )
        .bind(user_id)
        .bind(default_chain_id)
        .bind(&token)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    /// What the polling client should be told.
    pub async fn poll_pairing(&self, device_code: &str) -> Result<PairingPoll, sqlx::Error> {
        let hash = Self::hash_token(device_code);
        let row: Option<(i64, Option<String>, i64)> = sqlx::query_as(
            "SELECT id, token, expires_at FROM pairing_requests WHERE device_code_hash = ?",
        )
        .bind(&hash)
        .fetch_optional(&self.pool)
        .await?;
        let Some((id, token, expires_at)) = row else {
            return Ok(PairingPoll::Unknown);
        };
        if let Some(token) = token {
            // Handed over once, then cleared: the request row must not remain a
            // second copy of a live credential.
            sqlx::query("DELETE FROM pairing_requests WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
            return Ok(PairingPoll::Approved(token));
        }
        if expires_at <= Self::now_secs() {
            return Ok(PairingPoll::Expired);
        }
        Ok(PairingPoll::Pending)
    }

    /// Drop expired, unapproved requests. Cheap, and keeps a guessable code
    /// from lingering.
    pub async fn purge_expired_pairings(&self) -> Result<u64, sqlx::Error> {
        let res =
            sqlx::query("DELETE FROM pairing_requests WHERE expires_at <= ? AND token IS NULL")
                .bind(Self::now_secs())
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected())
    }

    pub async fn create_token(
        &self,
        user_id: i64,
        name: &str,
        scopes: &str,
        default_chain_id: Option<i64>,
        imports_are_live: bool,
    ) -> Result<String, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let token = format!("td_{}", hex::encode(raw));
        let token_hash = Self::hash_token(&token);
        let now = Self::now_secs();
        sqlx::query("INSERT INTO tokens (user_id, token_hash, name, scopes, created_at, default_chain_id, imports_are_live) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(user_id).bind(&token_hash).bind(name).bind(scopes).bind(now)
            .bind(default_chain_id)
            .bind(imports_are_live)
            .execute(&self.pool).await?;
        info!("🔑 Created token '{}' for user_id {}", name, user_id);
        Ok(token)
    }

    /// Rename a token or repoint its default chain. The token itself is never
    /// re-issued — the whole point is that you can fix the chain after the fact
    /// without touching the client you already pasted it into.
    pub async fn update_token(
        &self,
        user_id: i64,
        token_id: i64,
        name: &str,
        default_chain_id: Option<i64>,
        imports_are_live: bool,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE tokens SET name = ?, default_chain_id = ?, imports_are_live = ?
             WHERE id = ? AND user_id = ?",
        )
        .bind(name)
        .bind(default_chain_id)
        .bind(imports_are_live)
        .bind(token_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Re-issue a token in place: same id, name, scopes and chain, new secret.
    /// Only the hash is stored, so a lost token can't be shown again — this is
    /// the recovery path instead. Everything referencing the token id (sessions,
    /// the default chain) keeps pointing at the same row.
    pub async fn rotate_token(
        &self,
        user_id: i64,
        token_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let token = format!("td_{}", hex::encode(raw));
        let result = sqlx::query("UPDATE tokens SET token_hash = ? WHERE id = ? AND user_id = ?")
            .bind(Self::hash_token(&token))
            .bind(token_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        info!("🔑 Rotated token {} for user_id {}", token_id, user_id);
        Ok(Some(token))
    }

    pub async fn delete_token(&self, user_id: i64, token_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM tokens WHERE id = ? AND user_id = ?")
            .bind(token_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn has_users(&self) -> Result<bool, sqlx::Error> {
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM users").fetch_one(&self.pool).await?;
        Ok(count.0 > 0)
    }

    pub async fn list_users(&self) -> Result<Vec<UserRecord>, sqlx::Error> {
        sqlx::query_as("SELECT id, username, display_name, role, created_at, disabled FROM users ORDER BY id ASC")
            .fetch_all(&self.pool).await
    }

    pub async fn list_tokens(&self, user_id: i64) -> Result<Vec<TokenRecord>, sqlx::Error> {
        sqlx::query_as(
            "SELECT t.id, t.name, t.scopes, t.created_at, t.last_used_at, t.default_chain_id,
                    t.imports_are_live, c.name AS default_chain_name
             FROM tokens t LEFT JOIN signal_chains c ON c.id = t.default_chain_id
             WHERE t.user_id = ? ORDER BY t.created_at ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
}

// ════════════════════════════════════════════════════════════
//  Row types
// ════════════════════════════════════════════════════════════

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
    pub artwork_url: Option<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
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
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub track_count: i64,
    pub total_duration: i64,
    pub avg_quality_score: Option<f64>,
    pub listening_context: Option<String>,
    pub chain_id: Option<i64>,
    pub chain_name: Option<String>,
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
fn new_uuid() -> String {
    use rand::Rng;
    let raw: [u8; 16] = rand::rng().random();
    hex::encode(raw)
}

/// A "no single artist" placeholder, not an artist.
///
/// Shared with the Plex source, which must not record a compilation's tracks
/// under it. Here it keeps the sanitiser from folding real performers onto a
/// placeholder: "Various Artists" is on every track of a compilation, so it
/// wins the breadth test outright.
pub fn is_various_artists(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "various artists" | "various artist" | "various" | "va" | "verschiedene interpreten"
    )
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
    pub title: String,
    pub album: Option<String>,
    pub duration: Option<i64>,
    /// JSON array of tag names, written by the genre backfill. Absent until
    /// that listen has been enriched.
    pub genres: Option<String>,
    pub chain_id: Option<i64>,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    async fn testdb() -> Database {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("td-test-{}-{}.db", std::process::id(), n));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
            .await
            .unwrap()
    }

    fn play(title: &str, artist: &str, ts: u64) -> Play {
        Play::new(title.into(), artist.into(), ts, format!("src-{ts}"), "test".into())
    }

    /// A skip must not reach *any* readout that counts listens.
    ///
    /// This class of bug has now been found three times — the language
    /// breakdown, the dashboard's top artist, and the artwork coverage
    /// numerator — each time because a new query was written next to filtered
    /// ones and quietly omitted the filter. Nothing errors; a number is simply
    /// wrong. So this asserts on the *readouts* rather than on the SQL, which
    /// A misspelled key must not be able to wipe an attribution.
    ///
    /// This is the fourth place the same shape has come up (`bulk-chain`,
    /// `default_chain_id`, `unify_album_release`) and the reason is always the
    /// same: a bare `Option` reads *absent* and *null* as the same thing, so a
    /// typo in a body reads as "clear this" and answers 200. On the shelf that
    /// means editing a purchase date silently unassigns the turntable.
    #[tokio::test]
    async fn absent_and_null_are_different_things_on_a_shelf_edit() {
        let db = testdb().await;
        let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
        let gear =
            db.upsert_equipment(uid, "SL-1200", "turntable", None, None, 0.0, None).await.unwrap();
        let id = db
            .create_physical(
                uid,
                "vinyl",
                "Burial",
                "Untrue",
                Some(2007),
                None,
                None,
                None,
                None,
                None,
                "[]",
                None,
                Some(1_700_000_000),
            )
            .await
            .unwrap();

        db.update_physical(uid, id, None, None, Some(Some(gear)), None, None).await.unwrap();
        let r = db.get_physical(uid, id).await.unwrap().unwrap();
        assert_eq!(r.equipment_id, Some(gear));
        assert_eq!(r.purchased_at, Some(1_700_000_000));

        // Editing one field leaves the others alone. This is the assertion a
        // bare `Option` fails.
        db.update_physical(uid, id, Some(Some(1_800_000_000)), None, None, None, None)
            .await
            .unwrap();
        let r = db.get_physical(uid, id).await.unwrap().unwrap();
        assert_eq!(r.purchased_at, Some(1_800_000_000));
        assert_eq!(r.equipment_id, Some(gear), "an untouched field must survive an edit");

        // An explicit null still clears — the picker's "— no deck —" needs it,
        // which is why the distinction has to survive deserialisation rather
        // than being designed away.
        db.update_physical(uid, id, None, None, Some(None), None, None).await.unwrap();
        assert_eq!(db.get_physical(uid, id).await.unwrap().unwrap().equipment_id, None);
    }

    /// Re-fetching a sleeve must update what is there, not stack a second copy
    /// of all nineteen booklet pages on top of the first. The archive's own
    /// image id is the key that makes that true.
    #[tokio::test]
    async fn refetching_a_sleeve_updates_rather_than_duplicates() {
        let db = testdb().await;
        let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
        let id = db
            .create_physical(
                uid,
                "cd",
                "Ólafur Arnalds",
                "re:member",
                None,
                None,
                None,
                None,
                None,
                None,
                "[]",
                None,
                None,
            )
            .await
            .unwrap();

        for pass in 0..2 {
            for (n, kind) in [(0, "front"), (1, "booklet"), (2, "booklet")] {
                db.add_physical_image(
                    uid,
                    id,
                    kind,
                    "caa",
                    Some(&format!("https://coverartarchive.org/x/{n}.jpg")),
                    None,
                    None,
                    None,
                    None,
                    Some(&format!("page {}", n + pass)),
                    n,
                    Some(&format!("{n}")),
                )
                .await
                .unwrap();
            }
        }
        let images = db.list_physical_images(uid, id).await.unwrap();
        assert_eq!(images.len(), 3, "a second fetch must not duplicate the sleeve");
        // And it is an update, not a no-op: an editor's corrected caption should
        // land rather than being discarded as "already have it".
        assert_eq!(images[0].caption.as_deref(), Some("page 1"));

        // An upload carries no provider id, so two scans of the same page are
        // two scans — the partial unique index is `WHERE remote_id IS NOT NULL`
        // for exactly this reason.
        for _ in 0..2 {
            db.add_physical_image(
                uid,
                id,
                "booklet",
                "upload",
                None,
                None,
                Some("shelf/1/1/a.jpg"),
                Some("image/jpeg"),
                Some(10),
                None,
                9,
                None,
            )
            .await
            .unwrap();
        }
        assert_eq!(db.list_physical_images(uid, id).await.unwrap().len(), 5);

        // The count the shelf card reads comes off the same rows.
        assert_eq!(db.get_physical(uid, id).await.unwrap().unwrap().image_count, 5);

        // And deleting the pressing hands back the files it was holding, which
        // is the only thing that knows they exist.
        let files = db.delete_physical(uid, id).await.unwrap().unwrap();
        assert_eq!(files, vec!["shelf/1/1/a.jpg".to_string(), "shelf/1/1/a.jpg".to_string()]);
        assert!(db.list_physical_images(uid, id).await.unwrap().is_empty());
    }

    /// is the only way a future query gets caught too.
    #[tokio::test]
    async fn a_skip_reaches_no_counting_readout() {
        let db = testdb().await;
        let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
        let now = Database::now_secs();

        // One real listen, and five skips of a different artist — enough that a
        // missing filter would put the skipped artist on top.
        let mut played = play("Played", "Real Artist", (now - 100) as u64);
        played.duration = Some(200);
        db.save_scrobble(uid, &played, None, None, None, None, None, None, ListenDisposition::LIVE)
            .await
            .unwrap();
        for i in 0..5 {
            let mut p = play("Skipped", "Skipped Artist", (now - 3000 - i * 400) as u64);
            p.duration = Some(200);
            p.source_id = format!("skip-{i}");
            db.save_scrobble(
                uid,
                &p,
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition { imported: false, skipped: true },
            )
            .await
            .unwrap();
        }

        assert_eq!(db.listen_count_and_latest(uid).await.unwrap().0, 1, "listen count");

        let stats = db.get_dashboard_stats(uid, chrono_tz::UTC, 1).await.unwrap();
        assert_eq!(
            stats["top_artist"].as_str(),
            Some("Real Artist"),
            "a skipped artist must never win the dashboard"
        );

        // Coverage readouts: both halves must count the same population, or the
        // progress bar reports a fraction of the wrong whole.
        let (art_total, _) = db.artwork_coverage(uid).await.unwrap();
        assert_eq!(art_total, 1, "artwork coverage counts only real listens");
        let (lang_total, _) = db.language_coverage().await.unwrap();
        assert_eq!(lang_total, 1, "language coverage counts only what the backfill will process");
    }

    /// The Statistics row's four "shape" figures, and the two rules that make
    /// them mean anything.
    ///
    /// `skips` is the one query in this file that deliberately counts skipped
    /// rows, sitting beside a dozen that must not — so it is exactly the shape
    /// of query that gets "fixed" by someone adding the usual filter.
    ///
    /// `per_day` divides by **calendar** days rather than by days that saw a
    /// listen. Written the obvious way, a burst of listening in one afternoon
    /// reports the afternoon's rate as the year's.
    #[tokio::test]
    async fn the_shape_figures_count_the_right_populations() {
        let db = testdb().await;
        let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
        let now = Database::now_secs();

        // Ten listens and four skips, all inside one hour a fortnight ago —
        // a burst, so "per active day" and "per calendar day" disagree loudly.
        for i in 0..10i64 {
            let mut p = play(&format!("Track {i}"), "Real Artist", (now - 14 * 86_400 + i) as u64);
            p.duration = Some(200);
            p.source_id = format!("listen-{i}");
            db.save_scrobble(uid, &p, None, None, None, None, None, None, ListenDisposition::LIVE)
                .await
                .unwrap();
        }
        for i in 0..4i64 {
            let mut p =
                play(&format!("Skipped {i}"), "Other", (now - 14 * 86_400 + 100 + i) as u64);
            p.duration = Some(200);
            p.source_id = format!("skip-{i}");
            db.save_scrobble(
                uid,
                &p,
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition { imported: false, skipped: true },
            )
            .await
            .unwrap();
        }

        let all = db.get_ranged_stats(uid, None, false).await.unwrap();
        let t = &all["totals"];

        assert_eq!(t["plays"].as_i64(), Some(10), "plays still exclude skips");
        assert_eq!(t["skips"].as_i64(), Some(4), "skips are counted, not filtered away");

        // The listening spans one hour but the history is a fortnight old, so
        // the honest rate is ten plays over fourteen days, not over one.
        let per_day = t["per_day"].as_f64().unwrap();
        assert!(
            (0.6..0.8).contains(&per_day),
            "per_day must divide by calendar days, got {per_day} (10 plays / ~14 days ≈ 0.71)"
        );

        // "Since" is about the whole history and ignores the range control, or
        // a one-week view answers with the start of that week.
        let first = t["first_listen"].as_i64().expect("first_listen");
        let week = db.get_ranged_stats(uid, Some(now - 7 * 86_400), false).await.unwrap();
        assert_eq!(
            week["totals"]["first_listen"].as_i64(),
            Some(first),
            "first_listen is not ranged"
        );
        assert_eq!(week["totals"]["plays"].as_i64(), Some(0), "the burst is outside this week");
        assert_eq!(week["totals"]["skips"].as_i64(), Some(0), "and so are its skips");

        // The average quality's denominator, which is the thing the card has to
        // report. Nothing here carried a quality score, so it is zero — and a
        // card reading "80.6/100" over zero listens is what this prevents.
        assert_eq!(t["quality_known"].as_i64(), Some(0));

        assert_eq!(t["loves"].as_i64(), Some(0));
        let e = db.upsert_entity("recording", "Track 1", "Real Artist", None).await.unwrap();
        db.set_love(uid, e, true).await.unwrap();
        let after = db.get_ranged_stats(uid, None, false).await.unwrap();
        assert_eq!(after["totals"]["loves"].as_i64(), Some(1));
    }

    /// Three consecutive plays of one track must store three listens.
    ///
    /// The window used to be the track's own duration, and a back-to-back
    /// repeat lands *exactly* one duration later — right on an inclusive
    /// `BETWEEN` boundary. Whether the second play survived came down to poll
    /// jitter, which is why a track played twice showed up once, sometimes.
    #[tokio::test]
    async fn a_track_on_repeat_is_not_deduplicated_away() {
        let db = testdb().await;
        let duration = 200u64;

        for pass in 0..3u64 {
            // Each pass is banked halfway through, so consecutive plays are
            // exactly one track length apart.
            let scrobbled_at = 1_700_000_000 + pass * duration + duration / 2;
            let mut p = play("Combustible", "Cœur de pirate", scrobbled_at);
            p.duration = Some(duration);
            // A per-pass id, as the live sources now produce.
            p.source_id = format!("jellyfin-item1-scrobble-{scrobbled_at}");
            assert!(
                db.save_scrobble(
                    1,
                    &p,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    ListenDisposition::LIVE
                )
                .await
                .unwrap(),
                "pass {pass} should have been stored"
            );
        }
        assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 3);
    }

    /// The same play arriving twice from two code paths — the live session and
    /// the history walk — is still one listen. This is what the window is for,
    /// and it must keep working now that the window is smaller.
    #[tokio::test]
    async fn the_same_play_reported_twice_is_still_one_listen() {
        let db = testdb().await;
        let mut first = play("Combustible", "Cœur de pirate", 1_700_000_000);
        first.duration = Some(200);
        assert!(db
            .save_scrobble(1, &first, None, None, None, None, None, None, ListenDisposition::LIVE)
            .await
            .unwrap());

        // Seconds later, under a different source_id, as the history endpoint
        // re-reports what the live session already banked.
        let mut echo = play("Combustible", "Cœur de pirate", 1_700_000_000 + 45);
        echo.duration = Some(200);
        echo.source_id = "jellyfin-item1-history-9".into();
        assert!(
            !db.save_scrobble(
                1,
                &echo,
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE
            )
            .await
            .unwrap(),
            "a re-report within the lag window is the same listen"
        );
        assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 1);
    }

    /// A pairing code mints exactly one token and hands it over exactly once.
    /// Both halves matter: a second approval would give an attacker who watched
    /// someone type a code a token of their own, and a request row left holding
    /// the token would be a second live copy of a credential.
    #[tokio::test]
    async fn a_pairing_code_is_single_use_in_both_directions() {
        let db = testdb().await;
        let uid = db.create_user("ash", None, "pw", "admin").await.unwrap();

        let (device_code, user_code) =
            db.start_pairing("Pano on Android", "submit read").await.unwrap();
        assert_eq!(user_code.len(), 9, "8 characters plus the separator");
        assert_eq!(db.poll_pairing(&device_code).await.unwrap(), PairingPoll::Pending);

        assert!(db.approve_pairing(&user_code, uid, None).await.unwrap());
        // A second approval must not mint a second token.
        assert!(!db.approve_pairing(&user_code, uid, None).await.unwrap());

        let token = match db.poll_pairing(&device_code).await.unwrap() {
            PairingPoll::Approved(t) => t,
            other => panic!("expected a token, got {other:?}"),
        };
        // Collected once; the row is gone, so polling again cannot re-serve it.
        assert_eq!(db.poll_pairing(&device_code).await.unwrap(), PairingPoll::Unknown);

        // The token really works, and carries the scopes that were asked for.
        let auth = db.validate_token(&token).await.unwrap().expect("valid");
        assert_eq!(auth.user.user_id, uid);
        assert!(auth.can_read());
    }

    /// `read` is an explicit grant. A token that only says `submit` must not be
    /// able to read, or the scope column would be decoration again.
    #[tokio::test]
    async fn read_is_not_implied_by_submit() {
        let db = testdb().await;
        let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

        let plain = db.create_token(uid, "scrobbler", "submit", None, false).await.unwrap();
        assert!(!db.validate_token(&plain).await.unwrap().unwrap().can_read());

        let reader = db.create_token(uid, "plugin", "submit read", None, false).await.unwrap();
        assert!(db.validate_token(&reader).await.unwrap().unwrap().can_read());
    }

    /// The same discipline for `write`, and the pair that matters most: `read`
    /// and `write` must not imply each other. A phone that may edit a listen is
    /// a different grant from one that may enumerate the history, and every
    /// scrobble client in the world holds `submit` — that one must carry
    /// neither.
    #[tokio::test]
    async fn write_is_its_own_grant_and_implies_nothing() {
        let db = testdb().await;
        let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

        let submit = db.create_token(uid, "scrobbler", "submit", None, false).await.unwrap();
        let t = db.validate_token(&submit).await.unwrap().unwrap();
        assert!(!t.can_write(), "submit must not imply write");
        assert!(!t.can_read(), "submit must not imply read");

        // read does not carry write...
        let reader = db.create_token(uid, "plugin", "submit read", None, false).await.unwrap();
        let t = db.validate_token(&reader).await.unwrap().unwrap();
        assert!(t.can_read());
        assert!(!t.can_write(), "read must not imply write");

        // ...and write does not carry read.
        let writer = db.create_token(uid, "phone", "submit write", None, false).await.unwrap();
        let t = db.validate_token(&writer).await.unwrap().unwrap();
        assert!(t.can_write());
        assert!(!t.can_read(), "write must not imply read");

        // `all` is the one grant that covers both.
        let every = db.create_token(uid, "trusted", "all", None, false).await.unwrap();
        let t = db.validate_token(&every).await.unwrap().unwrap();
        assert!(t.can_read() && t.can_write());

        // Substrings must not match: `rewrite` is not `write`.
        let sneaky =
            db.create_token(uid, "odd", "submit rewrite readonly", None, false).await.unwrap();
        let t = db.validate_token(&sneaky).await.unwrap().unwrap();
        assert!(!t.can_write(), "'rewrite' must not grant write");
        assert!(!t.can_read(), "'readonly' must not grant read");
    }

    /// `save_scrobble` hardcoded `status = 'pending'`, so a listen submitted as
    /// `listen_type: "import"` was indistinguishable from one that had just
    /// played and went out to Last.fm — rewriting a permanent public record
    /// with someone's backfill. The forwarding queue is the thing to assert on:
    /// a backfill must never appear in it.
    #[tokio::test]
    async fn a_backfilled_listen_is_never_queued_for_forwarding() {
        let db = testdb().await;

        db.save_scrobble(
            1,
            &play("Live", "A", 1000),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        db.save_scrobble(
            1,
            &play("Backfilled", "A", 2000),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: true, skipped: false },
        )
        .await
        .unwrap();

        let pending = db.get_pending_scrobbles().await.unwrap();
        assert_eq!(pending.len(), 1, "only the live listen may be forwarded");
        assert_eq!(pending[0].play.title, "Live");

        // Both are still *stored* — an import is history, not a discard.
        assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 2);
    }

    /// A skip is stored so skip-rate analytics has something real to count, and
    /// filed as `'imported'` so nothing forwards it: a skip on someone's
    /// Last.fm is a wrong scrobble on a permanent record.
    #[tokio::test]
    async fn a_skipped_listen_is_stored_uncounted_and_unforwarded() {
        let db = testdb().await;

        db.save_scrobble(
            1,
            &play("Played", "A", 1000),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        db.save_scrobble(
            1,
            &play("Skipped", "A", 2000),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped: true },
        )
        .await
        .unwrap();

        assert!(
            db.get_pending_scrobbles().await.unwrap().iter().all(|p| p.play.title != "Skipped"),
            "a skip must never reach a sink"
        );
        assert_eq!(
            db.listen_count_and_latest(1).await.unwrap().0,
            1,
            "a skip is excluded from every count"
        );

        let stored: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = 1 AND skipped = TRUE")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(stored.0, 1, "but it is kept, so skip-rate analytics can see it");
    }

    /// These five were built on every listen and dropped on the floor:
    /// `AudioQuality` carried them and `save_scrobble` had no columns to bind
    /// them to. The regression to guard is silent — a field that is simply
    /// always NULL.
    #[tokio::test]
    async fn the_full_quality_record_round_trips() {
        let db = testdb().await;
        let q = AudioQuality {
            format_type: Some("dsd".into()),
            container: Some("dsf".into()),
            source_quality: Some("hi-res".into()),
            dsd_rate: Some(2_822_400),
            dsd_multiplier: Some(64),
            dsd_to_pcm_converted: Some(true),
            delivery_codec: Some("ldac".into()),
            delivery_sample_rate: Some(96_000),
            delivery_bit_depth: Some(24),
            ..Default::default()
        };
        db.save_scrobble(
            1,
            &play("T", "A", 1000),
            Some(&q),
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();

        let row: (Option<String>, Option<String>, Option<bool>, Option<i32>, Option<i32>) =
            sqlx::query_as(
                "SELECT container, source_quality, dsd_to_pcm_converted,
                        delivery_sample_rate, delivery_bit_depth
                   FROM scrobbles WHERE user_id = 1",
            )
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("dsf"));
        assert_eq!(row.1.as_deref(), Some("hi-res"));
        assert_eq!(row.2, Some(true));
        assert_eq!(row.3, Some(96_000));
        assert_eq!(row.4, Some(24));
    }

    /// The bug this pass had from the start: its SQL grouped by
    /// `LOWER(TRIM(artist))` while the Rust grouping also lowercases, so a
    /// case-only variant was merged away before anything could report it. On a
    /// real 21k-listen history that meant thirteen groups went unreported and
    /// the screen said there was nothing to fix.
    #[tokio::test]
    async fn case_only_variants_are_reported() {
        let db = testdb().await;
        let mut ts = 1000u64;
        for (artist, n) in [("Linkin Park", 5), ("LINKIN PARK", 2), ("Kiss", 1), ("KISS", 3)] {
            for _ in 0..n {
                ts += 1000;
                db.save_scrobble(
                    1,
                    &play("T", artist, ts),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    ListenDisposition::LIVE,
                )
                .await
                .unwrap();
            }
        }

        let v = db.artists_with_spelling_variants(1).await.unwrap();
        let lp = v.iter().find(|g| g.primary.eq_ignore_ascii_case("linkin park")).expect("found");
        assert_eq!(lp.primary, "Linkin Park", "the most-played spelling wins");
        assert_eq!(lp.variants, vec![("LINKIN PARK".to_string(), 2)]);
        assert!(lp.case_only, "differs only in case");

        let kiss = v.iter().find(|g| g.primary.eq_ignore_ascii_case("kiss")).expect("found");
        assert_eq!(kiss.primary, "KISS", "and it is the count that decides, not the casing");
    }

    /// The other half: a ligature is not a case difference. These are two
    /// artists to every query — two chart rows, two entity keys — so they must
    /// be reported *and* not marked cosmetic.
    #[tokio::test]
    async fn ligature_variants_are_reported_as_real() {
        let db = testdb().await;
        let mut ts = 1000u64;
        for (artist, n) in [("Cœur de pirate", 4), ("Coeur de Pirate", 2)] {
            for _ in 0..n {
                ts += 1000;
                db.save_scrobble(
                    1,
                    &play("T", artist, ts),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    ListenDisposition::LIVE,
                )
                .await
                .unwrap();
            }
        }

        let v = db.artists_with_spelling_variants(1).await.unwrap();
        let g = v.iter().find(|g| g.primary.starts_with('C')).expect("grouped");
        assert_eq!(g.primary, "Cœur de pirate");
        assert_eq!(g.variants, vec![("Coeur de Pirate".to_string(), 2)]);
        assert!(!g.case_only, "a ligature splits the play count — not cosmetic");
    }

    /// Two genuinely different artists must never be grouped, whatever the
    /// normaliser folds.
    #[tokio::test]
    async fn different_artists_are_not_grouped() {
        let db = testdb().await;
        let mut ts = 1000u64;
        for artist in ["Burial", "Aphex Twin", "Karthik", "Karthika"] {
            ts += 1000;
            db.save_scrobble(
                1,
                &play("T", artist, ts),
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE,
            )
            .await
            .unwrap();
        }
        assert!(db.artists_with_spelling_variants(1).await.unwrap().is_empty());
    }

    /// The soundtrack case, in the exact shape it has in real data: an album
    /// where every track carries the music director and each track also carries
    /// its own singers.
    ///
    /// The director must win, and must win *on breadth*. Ranking by play count
    /// got this backwards on a real export — a singer whose one track had been
    /// played more than the director's spread outranked them, and the fold ran
    /// the wrong way, moving "Harris Jayaraj" onto a singer. Here the singers
    /// deliberately out-play the director on their own track.
    #[tokio::test]
    async fn the_album_artist_wins_on_breadth_not_plays() {
        let db = testdb().await;
        let mut ts = 1_000u64;

        // Written out rather than looped so the play counts are visible: the
        // director is on three tracks with 1 play each, one singer has 9 on a
        // single track. Play count would crown the singer.
        let rows: [(&str, &str, usize); 5] = [
            ("Vaarayo Vaarayo", "Harris Jayaraj", 1),
            ("Hasili Fisiliye", "Harris Jayaraj", 1),
            ("Yaaro Yaaro", "Harris Jayaraj", 1),
            ("Vaarayo Vaarayo", "Unnikrishnan, Chinmayi & Megha", 9),
            ("Hasili Fisiliye", "Karthik", 2),
        ];
        for (title, artist, n) in rows {
            for _ in 0..n {
                ts += 1000;
                let mut p = play(title, artist, ts);
                p.album = Some("Aadhavan".into());
                db.save_scrobble(
                    1,
                    &p,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    ListenDisposition::LIVE,
                )
                .await
                .unwrap();
            }
        }

        let splits = db.tracks_with_split_attribution(1).await.unwrap();
        let v = splits
            .iter()
            .find(|s| s.title == "vaarayo vaarayo")
            .expect("the split track is listed");

        assert_eq!(v.primary, "Harris Jayaraj", "the director wins despite fewer plays");
        assert_eq!(v.primary_album_tracks, 3, "and wins because they are on every track");
        assert_eq!(v.folding.len(), 1);
        assert_eq!(v.folding[0].0, "Unnikrishnan, Chinmayi & Megha");
        assert_eq!(v.folding[0].1, 9);

        // Applying it keeps the singers rather than erasing them.
        let n = db
            .merge_track_attribution(
                1,
                "vaarayo vaarayo",
                "aadhavan",
                "Unnikrishnan, Chinmayi & Megha",
                "Harris Jayaraj",
            )
            .await
            .unwrap();
        assert_eq!(n, 9, "every stray listen moves");

        let credit: (Option<String>,) = sqlx::query_as(
            "SELECT artist_credit FROM scrobbles
             WHERE user_id = 1 AND LOWER(title) = 'vaarayo vaarayo'
               AND artist_credit IS NOT NULL LIMIT 1",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        let credit = credit.0.expect("the folded name is kept");
        assert!(credit.contains("Unnikrishnan"), "singers survive in the credit: {credit}");
        assert!(credit.contains("Harris Jayaraj"), "and so does the director: {credit}");

        // Nothing is left split.
        let after = db.tracks_with_split_attribution(1).await.unwrap();
        assert!(!after.iter().any(|s| s.title == "vaarayo vaarayo"));
    }

    #[tokio::test]
    async fn dedup_rejects_duplicate_source() {
        let db = testdb().await;
        let p = play("Teardrop", "Massive Attack", 1000);
        assert!(db
            .save_scrobble(1, &p, None, None, None, None, None, None, ListenDisposition::LIVE)
            .await
            .unwrap());
        // same source_id + name → deduped
        assert!(!db
            .save_scrobble(1, &p, None, None, None, None, None, None, ListenDisposition::LIVE)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn per_sink_delivery_is_tracked() {
        let db = testdb().await;
        db.save_scrobble(
            1,
            &play("T", "A", 2000),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();

        let pending = db.get_pending_scrobbles().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].delivered.is_empty());
        let id = pending[0].id;

        db.mark_sink_delivered(id, "lastfm").await.unwrap();
        db.mark_sink_delivered(id, "lastfm").await.unwrap(); // idempotent
        let pending = db.get_pending_scrobbles().await.unwrap();
        assert_eq!(pending[0].delivered, vec!["lastfm".to_string()]);

        // still pending until marked synced
        db.mark_scrobble_synced(id).await.unwrap();
        assert!(db.get_pending_scrobbles().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn chain_name_and_output_binding_resolve() {
        let db = testdb().await;
        let uid = db.create_user("alice", None, "not-used-yet", "admin").await.unwrap();
        let chain = SignalChain {
            id: None,
            user_id: uid,
            name: "Desktop".into(),
            description: None,
            components: vec![],
            listening_context: ListeningContext::Active,
            icon: None,
            is_active: true,
            total_hours: 0.0,
            created_at: 0,
        };
        let cid = db.create_chain(&chain).await.unwrap();

        // name → id (case-insensitive)
        assert_eq!(db.get_chain_by_name(uid, "desktop").await.unwrap(), Some(cid));
        assert_eq!(db.get_chain_by_name(uid, "nope").await.unwrap(), None);

        // auto-learn an output, unmapped resolves to None, then bind it
        db.record_output(uid, "Schiit Mimir", Some("usb")).await.unwrap();
        assert_eq!(db.resolve_binding(uid, "Schiit Mimir").await.unwrap(), None);
        db.set_binding(uid, "Schiit Mimir", Some(cid)).await.unwrap();
        assert_eq!(db.resolve_binding(uid, "Schiit Mimir").await.unwrap(), Some(cid));
    }

    #[tokio::test]
    async fn bulk_set_chain_updates_only_targets() {
        let db = testdb().await;
        let uid = db.create_user("bob", None, "not-used-yet", "admin").await.unwrap();
        let cid = db
            .create_chain(&SignalChain {
                id: None,
                user_id: uid,
                name: "C".into(),
                description: None,
                components: vec![],
                listening_context: ListeningContext::Active,
                icon: None,
                is_active: true,
                total_hours: 0.0,
                created_at: 0,
            })
            .await
            .unwrap();
        db.save_scrobble(
            uid,
            &play("a", "x", 10),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        db.save_scrobble(
            uid,
            &play("b", "y", 20),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        let ids: Vec<i64> = db.get_all_scrobbles(uid).await.unwrap().iter().map(|r| r.id).collect();

        let n = db.bulk_set_scrobble_chain(uid, &ids[..1], Some(cid)).await.unwrap();
        assert_eq!(n, 1);
        let rows = db.get_all_scrobbles(uid).await.unwrap();
        assert_eq!(rows.iter().filter(|r| r.chain_id == Some(cid)).count(), 1);
    }

    /// Chain and gear hours follow the listens, whenever the listens were
    /// assigned.
    ///
    /// This was an accumulator written at one insert site, which meant three
    /// things looked identical from the outside — a chain that has plainly been
    /// used reading zero. Only ingested listens counted (a polled Plex listen
    /// added nothing), assigning a chain afterwards moved nothing, and
    /// `equipment.total_hours` derives from this so gear inherited all of it.
    /// The reported symptom was "assigning signal chains to devices doesn't
    /// update the usage hours", which is the second of those.
    #[tokio::test]
    async fn chain_and_gear_hours_follow_the_listens_however_late_they_are_assigned() {
        let db = testdb().await;
        let uid = db.create_user("ash", None, "pw", "admin").await.unwrap();

        let gear = db
            .upsert_equipment(uid, "Bifrost 2", "dac", Some("Schiit"), None, 0.0, None)
            .await
            .unwrap();
        let cid = db
            .create_chain(&SignalChain {
                id: None,
                user_id: uid,
                name: "Desktop".into(),
                description: None,
                // The same gear at two steps. Joining `json_each` counted the
                // chain's whole running time once per mention, so a preamp used
                // twice doubled its own hours.
                components: vec![
                    ChainComponent {
                        component_type: ComponentType::Dac,
                        name: "Bifrost 2".into(),
                        detail: None,
                        equipment_id: Some(gear),
                    },
                    ChainComponent {
                        component_type: ComponentType::Transport,
                        name: "Bifrost 2".into(),
                        detail: None,
                        equipment_id: Some(gear),
                    },
                ],
                listening_context: ListeningContext::Active,
                icon: None,
                is_active: true,
                total_hours: 0.0,
                created_at: 0,
            })
            .await
            .unwrap();

        // Two hours played, plus a skip of the same length. None of it is
        // assigned to the chain yet — which is the ordinary case, since a listen
        // polled from a media server carries no chain until one is bound.
        for (i, title) in ["a", "b"].iter().enumerate() {
            let mut p = play(title, "x", 1000 + i as u64);
            p.duration = Some(3600);
            db.save_scrobble(uid, &p, None, None, None, None, None, None, ListenDisposition::LIVE)
                .await
                .unwrap();
        }
        let mut skipped = play("c", "x", 3000);
        skipped.duration = Some(3600);
        db.save_scrobble(
            uid,
            &skipped,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped: true },
        )
        .await
        .unwrap();

        let hours = |c: &SignalChain| (c.total_hours * 100.0).round() / 100.0;
        assert_eq!(hours(&db.get_chain(uid, cid).await.unwrap().unwrap()), 0.0);

        // Now assign them, the way binding an output device or fixing a listen
        // by hand would. An accumulator counted nothing here; the derivation
        // picks up every listen that carries the chain.
        let ids: Vec<i64> = db.get_all_scrobbles(uid).await.unwrap().iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 3);
        db.bulk_set_scrobble_chain(uid, &ids, Some(cid)).await.unwrap();

        let chain = db.get_chain(uid, cid).await.unwrap().unwrap();
        assert_eq!(hours(&chain), 2.0, "two played hours; the skip is not one of them");
        assert_eq!(
            hours(&db.get_chains(uid).await.unwrap()[0]),
            2.0,
            "the listing and the single read must agree"
        );

        // Gear sums its chains, and the doubled component must not double it.
        let gear_row =
            db.get_equipment(uid).await.unwrap().into_iter().find(|g| g.id == gear).unwrap();
        assert_eq!((gear_row.total_hours * 100.0).round() / 100.0, 2.0);

        // Unassigning takes the hours back with it, which an accumulator could
        // never do.
        db.bulk_set_scrobble_chain(uid, &ids, None).await.unwrap();
        assert_eq!(hours(&db.get_chain(uid, cid).await.unwrap().unwrap()), 0.0);
    }

    /// A token per app, each with its own chain, is the whole point of
    /// `default_chain_id` — so validating a token must hand back the chain in
    /// the same breath as the identity, and editing must not re-issue it.
    /// Merge and split rewrite `session_id` across rows and then rebuild the
    /// aggregates from what's left — the failure worth designing out is a
    /// session whose counts disagree with its own listens.
    #[tokio::test]
    async fn sessions_merge_and_split_and_keep_their_aggregates_honest() {
        let db = testdb().await;
        let uid = db.create_user("hana", None, "x", "user").await.unwrap();

        // Two sittings an hour apart: the gap heuristic makes them separate.
        let mut ids = Vec::new();
        for (i, ts) in [1_000, 1_200, 1_400, 10_000, 10_200].iter().enumerate() {
            db.save_scrobble(
                uid,
                &play(&format!("t{i}"), "A", *ts),
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE,
            )
            .await
            .unwrap();
            let sid = db
                .assign_session(uid, *ts as i64, 180, None, None, None, "active", 1800, None)
                .await
                .unwrap();
            ids.push(sid);
        }
        // Point the listens at the sessions the assigner chose.
        let rows = db.get_all_scrobbles(uid).await.unwrap();
        for (row, sid) in rows.iter().rev().zip(ids.iter()) {
            sqlx::query("UPDATE scrobbles SET session_id = ? WHERE id = ?")
                .bind(sid)
                .bind(row.id)
                .execute(&db.pool)
                .await
                .unwrap();
        }
        for sid in &ids {
            db.recompute_session(*sid).await.unwrap();
        }

        let sessions = db.list_sessions(uid, 50, 0).await.unwrap();
        assert_eq!(sessions.len(), 2, "the hour gap should make two sittings");
        let (first, second) = (sessions[1].id, sessions[0].id);
        assert_eq!(sessions[1].track_count + sessions[0].track_count, 5);

        // Merge: the earlier session survives and absorbs everything.
        let target = db.merge_sessions(uid, &[first, second]).await.unwrap().unwrap();
        assert_eq!(target, first);
        let merged = db.list_sessions(uid, 50, 0).await.unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].track_count, 5, "aggregates rebuilt from the listens");

        // Split it back at the fourth listen.
        let listens = db.session_scrobbles(uid, target).await.unwrap();
        assert_eq!(listens.len(), 5);
        let new_id = db.split_session(uid, target, listens[3].id).await.unwrap().unwrap();
        let after = db.list_sessions(uid, 50, 0).await.unwrap();
        assert_eq!(after.len(), 2);
        assert_eq!(db.session_scrobbles(uid, target).await.unwrap().len(), 3);
        assert_eq!(db.session_scrobbles(uid, new_id).await.unwrap().len(), 2);

        // Splitting at the first listen is a no-op, not an empty session.
        let head = db.session_scrobbles(uid, target).await.unwrap()[0].id;
        assert!(db.split_session(uid, target, head).await.unwrap().is_none());
        assert_eq!(db.list_sessions(uid, 50, 0).await.unwrap().len(), 2);

        // Another user's session is untouchable.
        let other = db.create_user("ivan", None, "x", "user").await.unwrap();
        assert!(db.merge_sessions(other, &[target, new_id]).await.unwrap().is_none());
        assert_eq!(db.list_sessions(uid, 50, 0).await.unwrap().len(), 2);
    }

    /// Per-edit review hands the operator a choice, so the preview has to show
    /// the whole ballot — not just the winner — and count the listens that carry
    /// no release at all, since unifying stamps those too.
    #[tokio::test]
    async fn mixed_album_preview_reports_every_candidate_and_the_untagged() {
        let db = testdb().await;
        let uid = db.create_user("jo", None, "x", "user").await.unwrap();

        // Four listens on release A, one on B, two with no release at all.
        let mut plays = Vec::new();
        for i in 0..7 {
            let mut p = play(&format!("t{i}"), "Band", 100 + i as u64);
            p.album = Some("Record".into());
            p.mbid_release = match i {
                0..=3 => Some("aaaaaaaa-0000-0000-0000-000000000000".into()),
                4 => Some("bbbbbbbb-0000-0000-0000-000000000000".into()),
                _ => None,
            };
            plays.push(p);
        }
        for p in &plays {
            db.save_scrobble(uid, p, None, None, None, None, None, None, ListenDisposition::LIVE)
                .await
                .unwrap();
        }

        let mixed = db.albums_with_mixed_releases(uid).await.unwrap();
        assert_eq!(mixed.len(), 1, "{mixed:?}");
        let m = &mixed[0];
        assert_eq!(m.variants, 2);
        assert_eq!(m.winner_plays, 4);
        assert_eq!(m.untagged, 2, "listens with no release are counted separately");
        assert_eq!(m.candidates.len(), 2, "the operator sees the whole ballot");
        assert_eq!(m.candidates[0].plays, 4, "most-claimed first");

        // Unifying onto the *minority* release is allowed — that's the point of
        // reviewing — and it also stamps the untagged listens.
        let loser = m.candidates[1].mbid.clone();
        let n = db.unify_album_release(uid, "Band", "Record", &loser).await.unwrap();
        assert_eq!(n, 6, "everything except the one already on that release");
        assert!(db.albums_with_mixed_releases(uid).await.unwrap().is_empty());
    }

    /// The duplicate that started this: "A.R. Rahman" and "A. R. Rahman" split
    /// one artist across two rows of the same top-50.
    #[test]
    fn artist_spellings_normalise_to_one_identity() {
        assert_eq!(normalise_artist("A.R. Rahman"), normalise_artist("A. R. Rahman"));
        assert_eq!(normalise_artist("A.R. Rahman"), normalise_artist("AR Rahman"));
        assert_eq!(normalise_artist("Sigur Rós"), normalise_artist("sigur rós"));
        // Different artists must not collapse together.
        assert_ne!(normalise_artist("Bill Evans"), normalise_artist("Gil Evans"));
        assert_ne!(normalise_artist("The Beatles"), normalise_artist("Beatles"));
    }

    #[tokio::test]
    async fn spelling_variants_are_found_and_merged_into_the_most_played() {
        let db = testdb().await;
        let uid = db.create_user("gina", None, "x", "user").await.unwrap();
        for (i, artist) in
            ["A.R. Rahman", "A.R. Rahman", "A.R. Rahman", "A. R. Rahman"].iter().enumerate()
        {
            db.save_scrobble(
                uid,
                &play(&format!("t{i}"), artist, 100 + i as u64),
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE,
            )
            .await
            .unwrap();
        }

        let found = db.artists_with_spelling_variants(uid).await.unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        // The spelling with the most plays is the one that survives.
        assert_eq!(found[0].primary, "A.R. Rahman");
        assert_eq!(found[0].variants.len(), 1);

        db.rename_artist(uid, "A. R. Rahman", "A.R. Rahman").await.unwrap();
        assert!(db.artists_with_spelling_variants(uid).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_token_carries_its_default_chain_and_keeps_working_after_an_edit() {
        let db = testdb().await;
        let uid = db.create_user("erin", None, "x", "user").await.unwrap();
        let mk = |name: &str| SignalChain {
            id: None,
            user_id: uid,
            name: name.into(),
            description: None,
            components: vec![],
            listening_context: ListeningContext::Active,
            icon: None,
            is_active: true,
            total_hours: 0.0,
            created_at: 0,
        };
        let desk = db.create_chain(&mk("Desk")).await.unwrap();
        let couch = db.create_chain(&mk("Couch")).await.unwrap();

        let token = db.create_token(uid, "fooyin", "submit", Some(desk), false).await.unwrap();
        assert_eq!(db.validate_token(&token).await.unwrap().unwrap().default_chain_id, Some(desk));

        let id = db.list_tokens(uid).await.unwrap()[0].id;
        assert!(db.update_token(uid, id, "fooyin", Some(couch), false).await.unwrap());
        // Same token string, new chain — nothing to re-paste into the client.
        assert_eq!(db.validate_token(&token).await.unwrap().unwrap().default_chain_id, Some(couch));

        assert!(db.update_token(uid, id, "fooyin", None, false).await.unwrap());
        assert_eq!(db.validate_token(&token).await.unwrap().unwrap().default_chain_id, None);

        // Someone else's token id is not editable.
        let other = db.create_user("frank", None, "x", "user").await.unwrap();
        assert!(!db.update_token(other, id, "stolen", Some(desk), false).await.unwrap());
    }

    /// The relay switch has to survive the round trip, and — the half worth
    /// pinning — must be **off** for a token created without mentioning it.
    /// Every token that existed before this column did reads as not-live, which
    /// is what stops an upgrade forwarding anything it wasn't forwarding
    /// yesterday. The rail that bounds it lives in `ingest::import_is_live`.
    #[tokio::test]
    async fn a_token_relays_imports_only_once_it_is_told_to() {
        let db = testdb().await;
        let uid = db.create_user("gina", None, "x", "user").await.unwrap();

        let quiet = db.create_token(uid, "some plugin", "submit", None, false).await.unwrap();
        assert!(!db.validate_token(&quiet).await.unwrap().unwrap().imports_are_live);

        let live = db.create_token(uid, "fooyin", "submit", None, true).await.unwrap();
        assert!(db.validate_token(&live).await.unwrap().unwrap().imports_are_live);

        // Flipped after the fact, without re-issuing — the whole point of
        // editing a token in place rather than minting a new one.
        let id =
            db.list_tokens(uid).await.unwrap().iter().find(|t| t.name == "some plugin").unwrap().id;
        assert!(db.update_token(uid, id, "some plugin", None, true).await.unwrap());
        assert!(db.validate_token(&quiet).await.unwrap().unwrap().imports_are_live);
        assert!(
            db.list_tokens(uid)
                .await
                .unwrap()
                .iter()
                .find(|t| t.id == id)
                .unwrap()
                .imports_are_live
        );

        // And back off again.
        assert!(db.update_token(uid, id, "some plugin", None, false).await.unwrap());
        assert!(!db.validate_token(&quiet).await.unwrap().unwrap().imports_are_live);
    }

    #[tokio::test]
    async fn disabled_user_cannot_authenticate_by_password_token_or_session() {
        let db = testdb().await;
        let hash = Database::hash_password("password123").unwrap();
        let uid = db.create_user("carol", None, &hash, "user").await.unwrap();
        let token = db.create_token(uid, "t", "submit", None, false).await.unwrap();
        let session = db.create_web_session(uid).await.unwrap();

        // enabled: all three work
        assert!(db.authenticate("carol", "password123").await.unwrap().is_some());
        assert!(db.validate_token(&token).await.unwrap().is_some());
        assert!(db.validate_session(&session).await.unwrap().is_some());

        db.set_user_disabled(uid, true).await.unwrap();

        // disabled: every path is rejected
        assert!(db.authenticate("carol", "password123").await.unwrap().is_none());
        assert!(db.validate_token(&token).await.unwrap().is_none());
        assert!(db.validate_session(&session).await.unwrap().is_none());
    }

    /// A user must be able to change the password on an account somebody else
    /// created for them, and must not be able to do it without proving they
    /// know the current one.
    #[tokio::test]
    async fn a_user_can_change_their_own_password_but_only_with_the_old_one() {
        let db = testdb().await;
        let hash = Database::hash_password("original-pw").unwrap();
        let uid = db.create_user("colin", None, &hash, "user").await.unwrap();

        assert!(db.check_user_password(uid, "original-pw").await);
        assert!(!db.check_user_password(uid, "not-the-password").await);

        let next = Database::hash_password("a-better-one").unwrap();
        assert!(db.update_user_password(uid, &next).await.unwrap());
        assert!(db.authenticate("colin", "a-better-one").await.unwrap().is_some());
        assert!(db.authenticate("colin", "original-pw").await.unwrap().is_none());
    }

    /// Changing a password is normally a reaction to "someone else may have
    /// this", so the sessions opened with the old one must not survive it.
    #[tokio::test]
    async fn changing_a_password_drops_the_sessions_opened_with_the_old_one() {
        let db = testdb().await;
        let hash = Database::hash_password("original-pw").unwrap();
        let uid = db.create_user("colin", None, &hash, "user").await.unwrap();
        let elsewhere = db.create_web_session(uid).await.unwrap();
        assert!(db.validate_session(&elsewhere).await.unwrap().is_some());

        db.clear_user_sessions(uid).await.unwrap();
        assert!(db.validate_session(&elsewhere).await.unwrap().is_none());
    }

    /// A profile field distinguishes "leave alone" from "clear", so a request
    /// naming one field cannot blank the others.
    #[tokio::test]
    async fn a_profile_edit_touches_only_the_fields_it_names() {
        let db = testdb().await;
        let uid = db.create_user("colin", Some("Colin"), "x", "user").await.unwrap();
        db.update_profile(uid, None, Some(Some("Collects tape.".into())), None).await.unwrap();

        let p = db.get_profile(uid).await.unwrap().unwrap();
        assert_eq!(p.bio.as_deref(), Some("Collects tape."));
        assert_eq!(p.display_name.as_deref(), Some("Colin"), "an unnamed field must survive");

        // Explicit null clears, and still leaves the rest alone.
        db.update_profile(uid, None, Some(None), None).await.unwrap();
        let p = db.get_profile(uid).await.unwrap().unwrap();
        assert_eq!(p.bio, None);
        assert_eq!(p.display_name.as_deref(), Some("Colin"));
    }

    /// One deck per person, enforced by the primary key: putting a second side
    /// on replaces the first rather than erroring, because that is what
    /// happened in the room — you took the record off.
    #[tokio::test]
    async fn a_user_has_one_deck_and_the_side_on_it_survives() {
        let db = testdb().await;
        let uid = db.create_user("colin", None, "x", "user").await.unwrap();
        let a = db
            .create_physical(
                uid, "vinyl", "Burial", "Untrue", None, None, None, None, None, None, "[]", None,
                None,
            )
            .await
            .unwrap();
        let b = db
            .create_physical(
                uid,
                "cd",
                "Aphex Twin",
                "SAW 85-92",
                None,
                None,
                None,
                None,
                None,
                None,
                "[]",
                None,
                None,
            )
            .await
            .unwrap();

        assert!(db.get_spin(uid).await.unwrap().is_none(), "an empty deck to start");

        db.set_spin(uid, a, "A", 1_700_000_000, None, 0, "[]", None, None, 0).await.unwrap();
        let spin = db.get_spin(uid).await.unwrap().expect("a side is on");
        assert_eq!(spin.release_id, a);
        assert_eq!(spin.side, "A");
        assert_eq!(spin.paused_at, None);

        // Pausing keeps the same row rather than opening a second deck.
        db.set_spin(uid, a, "A", 1_700_000_000, Some(1_700_000_300), 0, "[]", None, None, 0)
            .await
            .unwrap();
        assert_eq!(db.get_spin(uid).await.unwrap().unwrap().paused_at, Some(1_700_000_300));

        // And a different record replaces it outright.
        db.set_spin(uid, b, "1", 1_700_001_000, None, 12, "[{}]", None, None, 0).await.unwrap();
        let spin = db.get_spin(uid).await.unwrap().unwrap();
        assert_eq!(spin.release_id, b);
        assert_eq!(spin.side, "1");
        assert_eq!(spin.paused_total, 12);

        assert!(db.clear_spin(uid).await.unwrap());
        assert!(db.get_spin(uid).await.unwrap().is_none());
    }

    /// Two people can each have a record on without seeing each other's.
    #[tokio::test]
    async fn one_persons_deck_is_not_anothers() {
        let db = testdb().await;
        let a = db.create_user("ashwin", None, "x", "admin").await.unwrap();
        let c = db.create_user("colin", None, "x", "user").await.unwrap();
        let mine = db
            .create_physical(
                a, "vinyl", "Burial", "Untrue", None, None, None, None, None, None, "[]", None,
                None,
            )
            .await
            .unwrap();
        db.set_spin(a, mine, "A", 1_700_000_000, None, 0, "[]", None, None, 0).await.unwrap();

        assert!(db.get_spin(a).await.unwrap().is_some());
        assert!(db.get_spin(c).await.unwrap().is_none());
        assert!(!db.clear_spin(c).await.unwrap(), "clearing an empty deck changes nothing");
        assert!(db.get_spin(a).await.unwrap().is_some(), "and leaves the other alone");
    }

    /// A tape remembers where it was left; a record has no such state.
    ///
    /// The asymmetry is the design, not an omission — a cassette is sequential
    /// and really is still parked halfway when you put it back in, while a
    /// record has random access and goes back in its sleeve. Writing a park
    /// point for vinyl would be modelling physics the object does not have.
    #[tokio::test]
    async fn a_tape_remembers_where_it_was_left_and_a_record_does_not() {
        let db = testdb().await;
        let uid = db.create_user("colin", None, "x", "user").await.unwrap();
        let tape = db
            .create_physical(
                uid, "cassette", "ABBA", "Gold", None, None, None, None, None, None, "[]", None,
                None,
            )
            .await
            .unwrap();
        let record = db
            .create_physical(
                uid, "vinyl", "Burial", "Untrue", None, None, None, None, None, None, "[]", None,
                None,
            )
            .await
            .unwrap();
        let disc = db
            .create_physical(
                uid,
                "cd",
                "Radiohead",
                "Pablo Honey",
                None,
                None,
                None,
                None,
                None,
                None,
                "[]",
                None,
                None,
            )
            .await
            .unwrap();

        assert!(db.park_tape(uid, tape, "A", 754).await.unwrap(), "a cassette parks");
        assert!(!db.park_tape(uid, record, "A", 754).await.unwrap(), "a record does not");
        assert!(!db.park_tape(uid, disc, "1", 754).await.unwrap(), "nor does a disc");

        let shelf = db.list_physical(uid, None).await.unwrap();
        let got = |id: i64| shelf.iter().find(|r| r.id == id).unwrap();
        assert_eq!(got(tape).parked_side.as_deref(), Some("A"));
        assert_eq!(got(tape).parked_secs, Some(754));
        assert_eq!(got(record).parked_side, None);
        assert_eq!(got(record).parked_secs, None);

        // Playing it through winds it back to the start.
        assert!(db.rewind_tape(uid, tape).await.unwrap());
        let shelf = db.list_physical(uid, None).await.unwrap();
        assert_eq!(shelf.iter().find(|r| r.id == tape).unwrap().parked_side, None);
    }

    #[tokio::test]
    async fn count_active_admins_excludes_disabled() {
        let db = testdb().await;
        let a1 = db.create_user("a1", None, "x", "admin").await.unwrap();
        db.create_user("a2", None, "x", "admin").await.unwrap();
        assert_eq!(db.count_active_admins().await.unwrap(), 2);
        db.set_user_disabled(a1, true).await.unwrap();
        assert_eq!(db.count_active_admins().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn delete_user_cascades_their_data() {
        let db = testdb().await;
        let uid = db.create_user("dave", None, "x", "user").await.unwrap();
        db.save_scrobble(
            uid,
            &play("s", "a", 5),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        db.create_token(uid, "t", "submit", None, false).await.unwrap();
        // A connection has an FK to users — deletion must clear it, or the whole
        // cascade rolls back (the bug this guards against).
        db.set_connection(uid, "listenbrainz", r#"{"token":"secret"}"#, Some("dave"))
            .await
            .unwrap();

        assert!(db.delete_user(uid).await.unwrap());
        assert!(db.get_all_scrobbles(uid).await.unwrap().is_empty());
        assert!(db.list_users().await.unwrap().iter().all(|u| u.id != uid));
        assert!(db.list_tokens(uid).await.unwrap().is_empty());
        assert!(db.list_connections(uid).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn interrupted_import_is_marked_on_startup() {
        let db = testdb().await;
        let uid = db.create_user("fay", None, "x", "admin").await.unwrap();
        // A job that started but never finished (process died) stays 'running'.
        let jid = db.create_import_job(uid, "file").await.unwrap();
        // A job that completed normally.
        let done = db.create_import_job(uid, "lastfm").await.unwrap();
        db.finish_import_job(done, "done", 10, 8, "ok").await.unwrap();

        let n = db.mark_running_imports_interrupted().await.unwrap();
        assert_eq!(n, 1, "only the unfinished job is touched");

        let imports = db.list_imports(uid).await.unwrap();
        let running = imports.iter().find(|r| r.id == jid).unwrap();
        assert_eq!(running.state, "interrupted");
        let finished = imports.iter().find(|r| r.id == done).unwrap();
        assert_eq!(finished.state, "done"); // untouched
    }

    #[tokio::test]
    async fn connections_round_trip_and_hide_creds() {
        let db = testdb().await;
        let uid = db.create_user("erin", None, "x", "user").await.unwrap();
        db.set_connection(uid, "listenbrainz", r#"{"token":"secret"}"#, Some("erin_lb"))
            .await
            .unwrap();

        // engine sees the credentials
        let fwd = db.get_connections_for_forwarding(uid).await.unwrap();
        assert_eq!(fwd, vec![("listenbrainz".to_string(), r#"{"token":"secret"}"#.to_string())]);

        // the public list exposes only service + account, never the token
        let list = db.list_connections(uid).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "listenbrainz");
        assert_eq!(list[0].1.as_deref(), Some("erin_lb"));

        assert!(db.delete_connection(uid, "listenbrainz").await.unwrap());
        assert!(db.list_connections(uid).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn connection_credentials_are_encrypted_at_rest() {
        let db = testdb().await;
        let uid = db.create_user("frank", None, "x", "user").await.unwrap();
        db.set_connection(uid, "listenbrainz", r#"{"token":"PLAINTEXT_MARKER"}"#, None)
            .await
            .unwrap();

        // The raw column must not contain the secret — a leaked backup is the
        // whole threat this defends against.
        let raw: (String,) =
            sqlx::query_as("SELECT credentials FROM user_connections WHERE user_id = ?")
                .bind(uid)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert!(!raw.0.contains("PLAINTEXT_MARKER"), "credential stored in the clear: {}", raw.0);
        assert!(SecretBox::is_encrypted(&raw.0));

        // …while the forwarding path still gets usable plaintext back.
        let fwd = db.get_connections_for_forwarding(uid).await.unwrap();
        assert_eq!(
            fwd,
            vec![("listenbrainz".to_string(), r#"{"token":"PLAINTEXT_MARKER"}"#.to_string())]
        );
    }

    #[tokio::test]
    async fn source_credentials_round_trip_and_stay_sealed() {
        let db = testdb().await;
        let uid = db.create_user("grace", None, "x", "user").await.unwrap();
        db.create_source(
            uid,
            "plex",
            "Main",
            true,
            "http://plex.local:32400",
            r#"{"token":"PLEX_SECRET"}"#,
            Some("grace_plex"),
            None,
            "{}",
        )
        .await
        .unwrap();

        let raw: (String,) =
            sqlx::query_as("SELECT credentials FROM user_sources WHERE user_id = ?")
                .bind(uid)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert!(!raw.0.contains("PLEX_SECRET"));

        let src = db.list_sources_for_user(uid).await.unwrap().remove(0);
        assert_eq!(src.credential("token").as_deref(), Some("PLEX_SECRET"));
        assert_eq!(src.remote_account.as_deref(), Some("grace_plex"));
        assert!(src.is_usable());
    }

    #[tokio::test]
    async fn sources_are_scoped_per_user() {
        let db = testdb().await;
        let a = db.create_user("heidi", None, "x", "user").await.unwrap();
        let b = db.create_user("ivan", None, "x", "user").await.unwrap();
        let a_id = db
            .create_source(
                a,
                "plex",
                "A",
                true,
                "http://a",
                r#"{"token":"ta"}"#,
                Some("heidi_plex"),
                None,
                "{}",
            )
            .await
            .unwrap();
        let b_id = db
            .create_source(
                b,
                "plex",
                "B",
                true,
                "http://b",
                r#"{"token":"tb"}"#,
                Some("ivan_plex"),
                None,
                "{}",
            )
            .await
            .unwrap();

        // Each user sees only their own row…
        assert_eq!(db.list_sources_for_user(a).await.unwrap().len(), 1);
        assert_eq!(db.get_source(a, a_id).await.unwrap().unwrap().url, "http://a");
        assert!(
            db.get_source(b, b_id).await.unwrap().unwrap().credential("token").as_deref()
                == Some("tb")
        );
        // An id belonging to someone else is a miss, not a leak.
        assert!(db.get_source(a, b_id).await.unwrap().is_none());

        // …while the engine sees both, each carrying its owner.
        let all = db.list_enabled_sources().await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|s| s.user_id == a && s.remote_account.as_deref() == Some("heidi_plex")));
        assert!(all
            .iter()
            .any(|s| s.user_id == b && s.remote_account.as_deref() == Some("ivan_plex")));

        // Disabling drops it from the engine's view but keeps the config.
        db.update_source(
            a,
            a_id,
            "A",
            false,
            "http://a",
            r#"{"token":"ta"}"#,
            Some("heidi_plex"),
            None,
            "{}",
        )
        .await
        .unwrap();
        assert_eq!(db.list_enabled_sources().await.unwrap().len(), 1);
        assert!(db.get_source(a, a_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn several_sources_of_one_kind_coexist() {
        let db = testdb().await;
        let uid = db.create_user("kate", None, "x", "user").await.unwrap();

        let living = db
            .create_source(
                uid,
                "plex",
                "Living Room",
                true,
                "http://plex-a",
                r#"{"token":"ta"}"#,
                Some("kate"),
                None,
                "{}",
            )
            .await
            .unwrap();
        let study = db
            .create_source(
                uid,
                "plex",
                "Study",
                true,
                "http://plex-b",
                r#"{"token":"tb"}"#,
                Some("kate"),
                None,
                "{}",
            )
            .await
            .unwrap();
        let nav = db
            .create_source(
                uid,
                "navidrome",
                "NAS",
                true,
                "http://nd",
                r#"{"username":"kate","password":"pw"}"#,
                Some("kate"),
                None,
                "{}",
            )
            .await
            .unwrap();

        // Two Plex servers used to be impossible: the table had UNIQUE(user, kind).
        assert_ne!(living, study);
        assert_eq!(db.list_sources_for_user(uid).await.unwrap().len(), 3);
        assert_eq!(db.list_enabled_sources().await.unwrap().len(), 3);

        // Each keeps its own credentials and label.
        assert_eq!(
            db.get_source(uid, living).await.unwrap().unwrap().credential("token").as_deref(),
            Some("ta")
        );
        assert_eq!(
            db.get_source(uid, study).await.unwrap().unwrap().credential("token").as_deref(),
            Some("tb")
        );
        assert_eq!(db.get_source(uid, study).await.unwrap().unwrap().label, "Study");

        // Deleting one leaves its twin alone.
        assert!(db.delete_source(uid, living).await.unwrap());
        let left = db.list_sources_for_user(uid).await.unwrap();
        assert_eq!(left.len(), 2);
        assert!(left.iter().any(|s| s.id == study));
        assert!(left.iter().any(|s| s.id == nav));
    }

    #[tokio::test]
    async fn a_listen_remembers_which_source_produced_it() {
        let db = testdb().await;
        let uid = db.create_user("liam", None, "x", "user").await.unwrap();
        let a = db
            .create_source(
                uid,
                "plex",
                "A",
                true,
                "http://a",
                r#"{"token":"ta"}"#,
                Some("liam"),
                None,
                "{}",
            )
            .await
            .unwrap();
        let b = db
            .create_source(
                uid,
                "plex",
                "B",
                true,
                "http://b",
                r#"{"token":"tb"}"#,
                Some("liam"),
                None,
                "{}",
            )
            .await
            .unwrap();

        db.save_scrobble(
            uid,
            &play("From A", "Artist", 100),
            None,
            None,
            None,
            None,
            Some("Plex"),
            Some(a),
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        db.save_scrobble(
            uid,
            &play("From B", "Artist", 200),
            None,
            None,
            None,
            None,
            Some("Plex"),
            Some(b),
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();

        // Without this the artwork proxy couldn't tell which of two Plex servers
        // to ask for a given listen's cover.
        let rows = db.get_all_scrobbles(uid).await.unwrap();
        let from_a = rows.iter().find(|r| r.title == "From A").unwrap();
        let from_b = rows.iter().find(|r| r.title == "From B").unwrap();
        assert_eq!(from_a.source_ref, Some(a));
        assert_eq!(from_b.source_ref, Some(b));
    }

    #[tokio::test]
    async fn recording_a_poll_does_not_look_like_a_config_change() {
        let db = testdb().await;
        let uid = db.create_user("judy", None, "x", "user").await.unwrap();
        let sid = db
            .create_source(
                uid,
                "plex",
                "P",
                true,
                "http://p",
                r#"{"token":"t"}"#,
                Some("judy_plex"),
                None,
                "{}",
            )
            .await
            .unwrap();
        let before = db.get_source(uid, sid).await.unwrap().unwrap();

        db.record_source_poll(before.id, Some("boom")).await.unwrap();
        let after = db.get_source(uid, sid).await.unwrap().unwrap();

        // If a poll bumped updated_at the engine would rebuild the source every
        // single tick, reconnecting endlessly.
        assert_eq!(before.updated_at, after.updated_at);
        assert_eq!(after.last_error.as_deref(), Some("boom"));
        assert!(after.last_polled_at.is_some());
    }

    // ── Liner notes (Phase 7) ──

    #[tokio::test]
    async fn one_entity_per_spelling_and_the_mbid_is_only_an_alias() {
        let db = testdb().await;
        let first = db.upsert_entity("recording", "Xtal", "Aphex Twin", None).await.unwrap();
        // Same record typed differently is the same row...
        let again =
            db.upsert_entity("recording", " xtal ", "aphex twin", Some("mb-1")).await.unwrap();
        assert_eq!(first, again);
        // ...and learning the MBID later doesn't create a second one.
        assert_eq!(db.get_entity(first).await.unwrap().unwrap().mbid.as_deref(), Some("mb-1"));

        // A later lookup that knows nothing must not erase what we had.
        db.upsert_entity("recording", "Xtal", "Aphex Twin", None).await.unwrap();
        assert_eq!(db.get_entity(first).await.unwrap().unwrap().mbid.as_deref(), Some("mb-1"));

        // Same title by someone else is a different record.
        let other = db.upsert_entity("recording", "Xtal", "Some Coverband", None).await.unwrap();
        assert_ne!(first, other);
    }

    #[tokio::test]
    async fn editing_a_note_keeps_what_it_said_before() {
        let db = testdb().await;
        let uid = db.create_user("ann", None, "x", "user").await.unwrap();
        let e = db.upsert_entity("release", "SAW 85-92", "Aphex Twin", None).await.unwrap();

        let id = db.upsert_note(uid, Some(e), None, "first thoughts", "private").await.unwrap();
        let again = db.upsert_note(uid, Some(e), None, "second thoughts", "public").await.unwrap();

        // One note per target — the second write edits rather than stacks.
        assert_eq!(id, again);
        assert_eq!(db.list_notes(uid).await.unwrap().len(), 1);
        let note = db.get_note(uid, id).await.unwrap().unwrap();
        assert_eq!(note.body_md, "second thoughts");
        assert_eq!(note.visibility, "public");

        // The earlier body survives the rewrite.
        let revs = db.note_revisions(uid, id).await.unwrap();
        assert_eq!(revs.len(), 1);
        assert_eq!(revs[0].body_md, "first thoughts");

        // Changing only the visibility is not a rewrite.
        db.upsert_note(uid, Some(e), None, "second thoughts", "private").await.unwrap();
        assert_eq!(db.note_revisions(uid, id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_record_and_one_playing_of_it_hold_separate_notes() {
        let db = testdb().await;
        let uid = db.create_user("bo", None, "x", "user").await.unwrap();
        db.save_scrobble(
            uid,
            &play("Xtal", "Aphex Twin", 1000),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        let listen = db
            .get_recent_scrobbles(uid, 1, 0, None, None, None, None, None)
            .await
            .unwrap()
            .remove(0);
        let e = db.upsert_entity("recording", "Xtal", "Aphex Twin", None).await.unwrap();

        db.upsert_note(uid, Some(e), None, "about the record", "private").await.unwrap();
        db.upsert_note(uid, None, Some(listen.id), "heard this on the train", "private")
            .await
            .unwrap();

        assert_eq!(db.list_notes(uid).await.unwrap().len(), 2);
        assert_eq!(db.note_for_entity(uid, e).await.unwrap().unwrap().body_md, "about the record");
        assert_eq!(
            db.note_for_scrobble(uid, listen.id).await.unwrap().unwrap().body_md,
            "heard this on the train"
        );

        // Deleting the listen takes its note with it — foreign keys are on, so
        // without that cleanup the delete would fail outright — and leaves the
        // note about the record alone.
        assert!(db.delete_scrobble(uid, listen.id).await.unwrap());
        assert_eq!(db.list_notes(uid).await.unwrap().len(), 1);
        assert!(db.note_for_entity(uid, e).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn searching_notes_looks_at_the_body_and_the_subject_both() {
        let db = testdb().await;
        let uid = db.create_user("kit", None, "x", "user").await.unwrap();
        let other = db.create_user("nell", None, "x", "user").await.unwrap();

        let record = db.upsert_entity("release", "Untrue", "Burial", None).await.unwrap();
        let artist = db.upsert_entity("artist", "Aphex Twin", "", None).await.unwrap();
        db.upsert_note(uid, Some(record), None, "the rain on this one", "private").await.unwrap();
        db.upsert_note(uid, Some(artist), None, "everything after Untrue owes this", "private")
            .await
            .unwrap();
        // Someone else's note mentioning the same word must never come back.
        let theirs = db.upsert_entity("release", "Untrue", "Burial", None).await.unwrap();
        db.upsert_note(other, Some(theirs), None, "not yours", "private").await.unwrap();

        // The subject matches for one, the body for the other — a search that
        // only did one of those would find half of what was asked for.
        let hits = db.search_notes(uid, "untrue", "", "updated").await.unwrap();
        assert_eq!(hits.len(), 2, "the record by name, the artist by its body");

        let by_body = db.search_notes(uid, "rain", "", "updated").await.unwrap();
        assert_eq!(by_body.len(), 1);
        assert_eq!(by_body[0].body_md, "the rain on this one");

        // ASCII case folding is what SQLite's LIKE gives, and it is the case
        // people actually type.
        assert_eq!(db.search_notes(uid, "BURIAL", "", "updated").await.unwrap().len(), 1);

        // The kind filter, and an unrecognised one meaning "no filter" rather
        // than "nothing" — a stale bookmark should not look like an empty
        // account.
        assert_eq!(db.search_notes(uid, "", "artist", "updated").await.unwrap().len(), 1);
        assert_eq!(db.search_notes(uid, "", "release", "updated").await.unwrap().len(), 1);
        assert_eq!(db.search_notes(uid, "", "listen", "updated").await.unwrap().len(), 0);
        assert_eq!(db.search_notes(uid, "", "nonsense", "updated").await.unwrap().len(), 2);

        // A wildcard typed into the box is a literal, not a pattern matching
        // everything.
        assert_eq!(db.search_notes(uid, "%", "", "updated").await.unwrap().len(), 0);
        assert_eq!(db.search_notes(uid, "_", "", "updated").await.unwrap().len(), 0);

        // Alphabetical sorts by subject, which is not the same order as by edit.
        let titled = db.search_notes(uid, "", "", "title").await.unwrap();
        assert_eq!(titled.len(), 2);
        assert_eq!(titled[0].entity_id, Some(artist), "Aphex Twin before Untrue");
    }

    #[tokio::test]
    async fn notes_and_revisions_are_scoped_to_their_owner() {
        let db = testdb().await;
        let ann = db.create_user("ann2", None, "x", "user").await.unwrap();
        let bob = db.create_user("bob2", None, "x", "user").await.unwrap();
        let e = db.upsert_entity("artist", "Aphex Twin", "", None).await.unwrap();

        let id = db.upsert_note(ann, Some(e), None, "mine", "public").await.unwrap();
        db.upsert_note(ann, Some(e), None, "mine, edited", "public").await.unwrap();

        // Even a `public` note isn't served to anyone else yet — patches don't
        // exist, so every read path is still self-only.
        assert!(db.get_note(bob, id).await.unwrap().is_none());
        assert!(db.note_for_entity(bob, e).await.unwrap().is_none());
        assert!(db.note_revisions(bob, id).await.unwrap().is_empty());
        assert!(!db.delete_note(bob, id).await.unwrap());
        assert!(db.get_note(ann, id).await.unwrap().is_some());

        // Both users can annotate the same entity without colliding.
        let bobs = db.upsert_note(bob, Some(e), None, "his", "private").await.unwrap();
        assert_ne!(id, bobs);
    }

    #[tokio::test]
    async fn shortcodes_resolve_through_the_listening_history() {
        let db = testdb().await;
        let uid = db.create_user("cy", None, "x", "user").await.unwrap();

        let mut p = play("Ageispolis", "Aphex Twin", 1000);
        p.album = Some("SAW 85-92".into());
        p.track_number = Some(3);
        db.save_scrobble(uid, &p, None, None, None, None, None, None, ListenDisposition::LIVE)
            .await
            .unwrap();

        // A named track with nothing annotated yet still resolves, because the
        // user has actually played it.
        let (id, label) = db
            .resolve_reference(uid, "recording", "ageispolis", None, Some("Aphex Twin"), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(label, "Ageispolis");

        // [[track:3]] means the third track of the record in context, and
        // renders with its real title rather than the number.
        let (pos_id, pos_label) = db
            .resolve_reference(
                uid,
                "recording",
                "3",
                Some(3),
                Some("Aphex Twin"),
                Some("SAW 85-92"),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pos_label, "Ageispolis");
        assert_eq!(pos_id, id);

        // A position with no album in scope, and a track nobody has heard,
        // resolve to nothing — which renders as plain text, not an error.
        assert!(db
            .resolve_reference(uid, "recording", "3", Some(3), None, None)
            .await
            .unwrap()
            .is_none());
        assert!(db
            .resolve_reference(uid, "recording", "Windowlicker", None, None, None)
            .await
            .unwrap()
            .is_none());

        // Another user's history is not a resolution source.
        let other = db.create_user("dee", None, "x", "user").await.unwrap();
        assert!(db
            .resolve_reference(other, "release", "SAW 85-92", None, None, None)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn loves_toggle_and_stay_per_user() {
        let db = testdb().await;
        let ann = db.create_user("ann3", None, "x", "user").await.unwrap();
        let bob = db.create_user("bob3", None, "x", "user").await.unwrap();
        let e = db.upsert_entity("recording", "Xtal", "Aphex Twin", None).await.unwrap();

        db.set_love(ann, e, true).await.unwrap();
        // Loving twice is not two loves.
        db.set_love(ann, e, true).await.unwrap();
        assert!(db.is_loved(ann, e).await.unwrap());
        assert!(!db.is_loved(bob, e).await.unwrap());
        assert_eq!(db.list_loves(ann).await.unwrap().len(), 1);
        assert!(db.list_loves(bob).await.unwrap().is_empty());

        db.set_love(ann, e, false).await.unwrap();
        assert!(!db.is_loved(ann, e).await.unwrap());
    }

    /// A fidelity share is of the listens whose format is known, and the mix's
    /// `Unknown` bucket is exactly the rest.
    ///
    /// The bug this pins: `is_lossless` is nullable and an imported listen has
    /// nothing in it — 97.4% of a real 21,390-listen export carries no format
    /// at all — so dividing by every play scored "never knew" as "not
    /// lossless". Someone who has only ever played FLAC read 2%. Nothing
    /// errored; the number was just a measure of how much history they had
    /// imported.
    ///
    /// It asserts on the *readouts* and on the two agreeing, not on the SQL,
    /// because the failure mode is a third fidelity query written later beside
    /// these two — the same reason `a_skip_reaches_no_counting_readout` is
    /// written the way it is.
    #[tokio::test]
    async fn a_fidelity_share_ignores_listens_of_unknown_format() {
        let db = testdb().await;
        let uid = db.create_user("fid", None, "pw", "user").await.unwrap();
        let now = Database::now_secs();

        let flac = AudioQuality {
            codec: Some("flac".into()),
            is_lossless: Some(true),
            format_type: Some("pcm".into()),
            ..Default::default()
        };
        let mp3 = AudioQuality {
            codec: Some("mp3".into()),
            is_lossless: Some(false),
            format_type: Some("pcm".into()),
            ..Default::default()
        };

        // Three listens Tapedeck saw the format of: two lossless, one not.
        // Separately titled and spaced an hour apart so `fuzzy_exists` reads
        // them as three plays rather than one play reported three times.
        for (i, q) in [(0i64, &flac), (1, &flac), (2, &mp3)] {
            let p = play(&format!("Known {i}"), "A", (now - 3600 * (i + 1)) as u64);
            db.save_scrobble(
                uid,
                &p,
                Some(q),
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE,
            )
            .await
            .unwrap();
        }
        // And ninety-seven with no format at all, which is roughly the ratio a
        // real imported history has.
        for i in 0..97i64 {
            let p = play(&format!("Imported {i}"), "A", (now - 400_000 - 3600 * i) as u64);
            db.save_scrobble(uid, &p, None, None, None, None, None, None, ListenDisposition::LIVE)
                .await
                .unwrap();
        }

        let dash = db.get_dashboard_stats(uid, chrono_tz::UTC, 1).await.unwrap();
        assert_eq!(dash["total"], 100);
        assert_eq!(dash["fidelity_known"], 3);
        // Two of the three known, not two of a hundred.
        assert_eq!(dash["lossless_pct"], 67);

        let ranged = db.get_ranged_stats(uid, None, false).await.unwrap();
        assert_eq!(ranged["totals"]["fidelity_known"], 3);
        assert_eq!(ranged["totals"]["lossless_pct"], 67);

        // The mix and the share must split the same rows the same way, or the
        // page contradicts itself: `Unknown` is defined as the complement of
        // the share's population, so everything else has to add up to it.
        let mix = ranged["fidelity"].as_array().unwrap();
        let named = |n: &str| {
            mix.iter().find(|r| r["name"] == n).map(|r| r["plays"].as_i64().unwrap()).unwrap_or(0)
        };
        assert_eq!(named("Unknown"), 97);
        assert_eq!(named("Lossless") + named("Lossy") + named("DSD"), 3);
    }

    /// A hand-set cover is for a *release*, and only a release.
    ///
    /// An artist's picture lives on the `artists` row — shared by name, and
    /// what the library proxy and the portrait backfill both address. If this
    /// query could write an artist entity's `image_url` too, the same artist
    /// would have two places to be pictured from and no rule about which wins,
    /// and nothing would error to say so.
    #[tokio::test]
    async fn a_cover_override_is_release_only_and_clearable() {
        let db = testdb().await;
        let album = db.upsert_entity("release", "Untrue", "Burial", None).await.unwrap();
        let artist = db.upsert_entity("artist", "Burial", "", None).await.unwrap();

        assert!(db.entity_image(album).await.unwrap().is_none());

        assert!(db.set_entity_image(album, Some("https://example.test/untrue.jpg")).await.unwrap());
        assert_eq!(
            db.entity_image(album).await.unwrap().as_deref(),
            Some("https://example.test/untrue.jpg")
        );

        // Refused, and reported as refused rather than quietly doing nothing.
        assert!(!db.set_entity_image(artist, Some("https://example.test/nope.jpg")).await.unwrap());
        assert!(db.entity_image(artist).await.unwrap().is_none());

        // Clearing restores "no override", which is what puts the cover derived
        // from the listens back on the page.
        assert!(db.set_entity_image(album, None).await.unwrap());
        assert!(db.entity_image(album).await.unwrap().is_none());
    }
}
