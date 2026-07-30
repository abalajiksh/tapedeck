use sqlx::{sqlite::{SqlitePoolOptions, SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous}, Pool, Sqlite, FromRow};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use crate::crypto::SecretBox;
use crate::models::*;
use tracing::{error, info, debug};
use sha2::{Sha256, Digest};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

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
    pub is_lossless: Option<bool>,
    pub dsd_rate: Option<i64>,
    pub dsd_multiplier: Option<i32>,
    pub delivery_codec: Option<String>,
    pub delivery_bitrate: Option<i32>,
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

/// A configured source, credentials decrypted. `kind` is `plex` | `navidrome` |
/// `jellyfin`; `credentials` is a per-kind JSON blob (Plex: `{"token":…}`).
#[derive(Debug, Clone)]
pub struct UserSource {
    pub id: i64,
    pub user_id: i64,
    pub kind: String,
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
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&self.options) else { return Vec::new() };
        match v.get(key) {
            Some(serde_json::Value::Array(items)) => items.iter()
                .filter_map(|i| i.as_str())
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            Some(serde_json::Value::String(s)) => s.split(',')
                .map(|p| p.trim().to_lowercase())
                .filter(|p| !p.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Pull a field out of the credential blob.
    pub fn credential(&self, key: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(&self.credentials).ok()?
            .get(key)?.as_str().map(str::to_string)
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
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options).await?;
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
                timestamp INTEGER NOT NULL, duration INTEGER,
                source_id TEXT NOT NULL, source_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                mbid_recording TEXT, mbid_release TEXT, mbid_artist TEXT,
                caa_id INTEGER, caa_release_mbid TEXT,
                format_type TEXT, codec TEXT, bitrate INTEGER,
                sample_rate INTEGER, bit_depth INTEGER, channels INTEGER,
                is_lossless BOOLEAN, dsd_rate INTEGER, dsd_multiplier INTEGER,
                delivery_codec TEXT, delivery_bitrate INTEGER,
                is_transcoded BOOLEAN, transcode_reason TEXT, quality_score REAL,
                device_id INTEGER REFERENCES devices(id),
                chain_id INTEGER REFERENCES signal_chains(id),
                session_id INTEGER REFERENCES sessions(id),
                listening_context TEXT, submission_client TEXT,
                delivered_sinks TEXT NOT NULL DEFAULT '[]',
                track_number INTEGER,
                language TEXT,
                release_year INTEGER,
                skipped BOOLEAN NOT NULL DEFAULT FALSE,
                artwork_url TEXT,
                UNIQUE(user_id, source_id, source_name)
            )"
        ).execute(&self.pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT UNIQUE NOT NULL,
                display_name TEXT,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'user',
                created_at INTEGER NOT NULL,
                settings JSON DEFAULT '{}',
                disabled INTEGER NOT NULL DEFAULT 0
            )"
        ).execute(&self.pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                token_hash TEXT UNIQUE NOT NULL,
                name TEXT NOT NULL,
                scopes TEXT DEFAULT 'submit',
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                expires_at INTEGER
            )"
        ).execute(&self.pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS signal_chains (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                name TEXT NOT NULL, description TEXT,
                components JSON NOT NULL DEFAULT '[]',
                listening_context TEXT NOT NULL DEFAULT 'unknown',
                is_active BOOLEAN DEFAULT TRUE,
                total_hours REAL DEFAULT 0,
                created_at INTEGER NOT NULL,
                UNIQUE(user_id, name)
            )"
        ).execute(&self.pool).await?;

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
            )"
        ).execute(&self.pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS equipment (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                name TEXT NOT NULL, type TEXT NOT NULL,
                brand TEXT, model TEXT,
                total_hours REAL DEFAULT 0,
                first_used INTEGER, last_used INTEGER, notes TEXT,
                UNIQUE(user_id, name)
            )"
        ).execute(&self.pool).await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                device_id INTEGER REFERENCES devices(id),
                chain_id INTEGER REFERENCES signal_chains(id),
                started_at INTEGER NOT NULL, ended_at INTEGER,
                track_count INTEGER DEFAULT 0, total_duration INTEGER DEFAULT 0,
                skip_count INTEGER DEFAULT 0, avg_quality_score REAL,
                listening_context TEXT DEFAULT 'unknown'
            )"
        ).execute(&self.pool).await?;

        // ── Web Sessions (browser login) ──
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS web_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                session_hash TEXT UNIQUE NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                last_active_at INTEGER NOT NULL
            )"
        ).execute(&self.pool).await?;

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
            )"
        ).execute(&self.pool).await?;

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
            )"
        ).execute(&self.pool).await?;

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
            )"
        ).execute(&self.pool).await?;

        // One row per user per source kind. Replaces the single-row
        // `plex_settings`: a source belongs to the person whose account it
        // reads, so nobody's token can see anybody else's listening.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS user_sources (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                kind TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                url TEXT NOT NULL DEFAULT '',
                credentials TEXT NOT NULL DEFAULT '',
                remote_account TEXT,
                poll_interval_secs INTEGER,
                options TEXT NOT NULL DEFAULT '{}',
                last_polled_at INTEGER,
                last_error TEXT,
                updated_at INTEGER NOT NULL DEFAULT 0,
                UNIQUE(user_id, kind)
            )"
        ).execute(&self.pool).await?;

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
            )"
        ).execute(&self.pool).await?;

        Ok(())
    }

    async fn migrate(&self) -> Result<(), sqlx::Error> {
        async fn has_column(pool: &Pool<Sqlite>, table: &str, column: &str) -> bool {
            let result: Result<(i64,), _> = sqlx::query_as(
                &format!("SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='{}'", table, column)
            ).fetch_one(pool).await;
            matches!(result, Ok((count,)) if count > 0)
        }

        // Migration 1: MusicBrainz fields
        if !has_column(&self.pool, "scrobbles", "mbid_recording").await {
            info!("⚙️ Migrating: adding MusicBrainz fields...");
            for col in &["mbid_recording TEXT", "mbid_release TEXT", "mbid_artist TEXT",
                         "caa_id INTEGER", "caa_release_mbid TEXT"] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {}", col))
                    .execute(&self.pool).await;
            }
        }

        // Migration 2: user_id
        if !has_column(&self.pool, "scrobbles", "user_id").await {
            info!("⚙️ Migrating: adding user_id...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1")
                .execute(&self.pool).await;
            let _ = sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_scrobbles_user_source ON scrobbles(user_id, source_id, source_name)")
                .execute(&self.pool).await;
        }

        // Migration 3: Phase 2 quality + context columns
        if !has_column(&self.pool, "scrobbles", "format_type").await {
            info!("⚙️ Migrating: adding Phase 2 quality and context fields...");
            for col in &[
                "format_type TEXT", "codec TEXT", "bitrate INTEGER",
                "sample_rate INTEGER", "bit_depth INTEGER", "channels INTEGER",
                "is_lossless BOOLEAN", "dsd_rate INTEGER", "dsd_multiplier INTEGER",
                "delivery_codec TEXT", "delivery_bitrate INTEGER",
                "is_transcoded BOOLEAN", "transcode_reason TEXT", "quality_score REAL",
                "device_id INTEGER", "chain_id INTEGER", "session_id INTEGER",
                "listening_context TEXT", "submission_client TEXT",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {}", col))
                    .execute(&self.pool).await;
            }
        }

        // Migration 4: role column on users
        if !has_column(&self.pool, "users", "role").await {
            info!("⚙️ Migrating: adding role column to users...");
            let _ = sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user'")
                .execute(&self.pool).await;
            // First user (id=1) is always admin
            let _ = sqlx::query("UPDATE users SET role = 'admin' WHERE id = 1")
                .execute(&self.pool).await;
            info!("✅ Role migration complete");
        }

        // Migration 5: per-sink delivery tracking
        if !has_column(&self.pool, "scrobbles", "delivered_sinks").await {
            info!("⚙️ Migrating: adding delivered_sinks...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN delivered_sinks TEXT NOT NULL DEFAULT '[]'")
                .execute(&self.pool).await;
        }

        // Migration 6: disabled flag on users
        if !has_column(&self.pool, "users", "disabled").await {
            info!("⚙️ Migrating: adding users.disabled...");
            let _ = sqlx::query("ALTER TABLE users ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0")
                .execute(&self.pool).await;
        }

        // Migration 7: track_number on scrobbles (album-playthrough analytics)
        if !has_column(&self.pool, "scrobbles", "track_number").await {
            info!("⚙️ Migrating: adding scrobbles.track_number...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN track_number INTEGER")
                .execute(&self.pool).await;
        }

        // Migration 8: language on scrobbles (language breakdown analytics)
        if !has_column(&self.pool, "scrobbles", "language").await {
            info!("⚙️ Migrating: adding scrobbles.language...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN language TEXT")
                .execute(&self.pool).await;
        }

        // Migration: resolved artwork URL. Supersedes deriving a Cover Art
        // Archive link from caa_id — artwork can now come from Deezer or the
        // user's own library, neither of which has a CAA id.
        if !has_column(&self.pool, "scrobbles", "artwork_url").await {
            info!("⚙️ Migrating: adding scrobbles.artwork_url...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN artwork_url TEXT")
                .execute(&self.pool).await;
        }

        // Migration: skipped listens (Rockbox logs mark rows L or S). Stored so
        // skip-rate analytics are possible, but excluded from every count.
        if !has_column(&self.pool, "scrobbles", "skipped").await {
            info!("⚙️ Migrating: adding scrobbles.skipped...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN skipped BOOLEAN NOT NULL DEFAULT FALSE")
                .execute(&self.pool).await;
        }

        // Migration: per-source options (library/device filters). `user_sources`
        // is young enough that few databases will need this, but the table was
        // shipped without the column for one build.
        if !has_column(&self.pool, "user_sources", "options").await {
            info!("⚙️ Migrating: adding user_sources.options...");
            let _ = sqlx::query("ALTER TABLE user_sources ADD COLUMN options TEXT NOT NULL DEFAULT '{}'")
                .execute(&self.pool).await;
        }

        // Migration: release year, for the by-decade breakdown.
        if !has_column(&self.pool, "scrobbles", "release_year").await {
            info!("⚙️ Migrating: adding scrobbles.release_year...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN release_year INTEGER")
                .execute(&self.pool).await;
        }

        Ok(())
    }

    // ════════════════════════════════════════════════════════════
    //  Password Hashing (Argon2)
    // ════════════════════════════════════════════════════════════

    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2.hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| format!("Password hashing failed: {}", e))
    }

    pub fn verify_password(password: &str, hash: &str) -> bool {
        if hash == "not-used-yet" { return false; }
        let Ok(parsed) = PasswordHash::new(hash) else { return false };
        Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
    }

    // ════════════════════════════════════════════════════════════
    //  Auth: Setup & Login
    // ════════════════════════════════════════════════════════════

    /// Returns true if no user has a real password (first-run or pre-auth upgrade).
    pub async fn needs_setup(&self) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM users WHERE password_hash != 'not-used-yet'"
        ).fetch_one(&self.pool).await?;
        Ok(count.0 == 0)
    }

    /// Verify username + password, return user info if valid.
    pub async fn authenticate(&self, username: &str, password: &str) -> Result<Option<AuthUser>, sqlx::Error> {
        let row: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, username, password_hash, role FROM users WHERE username = ? AND disabled = 0"
        ).bind(username).fetch_optional(&self.pool).await?;

        if let Some((user_id, uname, hash, role)) = row {
            if Self::verify_password(password, &hash) {
                return Ok(Some(AuthUser {
                    user_id, username: uname, is_admin: role == "admin",
                }));
            }
        }
        Ok(None)
    }

    /// Look up a user id by username (case-insensitive). Used to map a polled
    /// Plex account to the matching Tapedeck user.
    pub async fn get_user_id_by_username(&self, username: &str) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE LOWER(username) = LOWER(?)")
            .bind(username).fetch_optional(&self.pool).await?;
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
    pub async fn validate_session(&self, session_token: &str) -> Result<Option<AuthUser>, sqlx::Error> {
        let session_hash = Self::hash_session(session_token);
        let now = Self::now_secs();

        let row: Option<(i64, i64, String, String)> = sqlx::query_as(
            "SELECT ws.id, u.id, u.username, u.role FROM web_sessions ws
             JOIN users u ON ws.user_id = u.id
             WHERE ws.session_hash = ? AND ws.expires_at > ? AND u.disabled = 0"
        )
        .bind(&session_hash).bind(now)
        .fetch_optional(&self.pool).await?;

        if let Some((session_id, user_id, username, role)) = row {
            // Update last_active_at
            let _ = sqlx::query("UPDATE web_sessions SET last_active_at = ? WHERE id = ?")
                .bind(now).bind(session_id).execute(&self.pool).await;
            Ok(Some(AuthUser { user_id, username, is_admin: role == "admin" }))
        } else {
            Ok(None)
        }
    }

    /// Delete a session by its raw token (logout).
    pub async fn delete_web_session(&self, session_token: &str) -> Result<(), sqlx::Error> {
        let session_hash = Self::hash_session(session_token);
        sqlx::query("DELETE FROM web_sessions WHERE session_hash = ?")
            .bind(&session_hash).execute(&self.pool).await?;
        Ok(())
    }

    // ════════════════════════════════════════════════════════════
    //  Scrobble CRUD
    // ════════════════════════════════════════════════════════════

    pub async fn save_scrobble(
        &self, user_id: i64, play: &Play, quality: Option<&AudioQuality>,
        device_id: Option<i64>, chain_id: Option<i64>,
        listening_context: Option<&str>, submission_client: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        if self.exists(user_id, &play.source_id, &play.source_name).await? { return Ok(false); }
        if self.fuzzy_exists(user_id, &play.title, &play.artist, play.timestamp as i64).await? {
            debug!("Skipping duplicate play (fuzzy match): {} - {}", play.artist, play.title);
            return Ok(false);
        }
        let mbid_artist_json = play.mbid_artist.as_ref().map(|arr| serde_json::to_string(arr).unwrap());
        let q = quality.cloned().unwrap_or_default();

        sqlx::query(
            "INSERT INTO scrobbles (
                user_id, title, artist, album, timestamp, duration, source_id, source_name, status,
                mbid_recording, mbid_release, mbid_artist, caa_id, caa_release_mbid,
                format_type, codec, bitrate, sample_rate, bit_depth, channels, is_lossless,
                dsd_rate, dsd_multiplier, delivery_codec, delivery_bitrate,
                is_transcoded, transcode_reason, quality_score,
                device_id, chain_id, listening_context, submission_client, track_number
            ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, 'pending',
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
            )"
        )
        .bind(user_id).bind(&play.title).bind(&play.artist).bind(&play.album)
        .bind(play.timestamp as i64).bind(play.duration.map(|d| d as i64))
        .bind(&play.source_id).bind(&play.source_name)
        .bind(&play.mbid_recording).bind(&play.mbid_release).bind(&mbid_artist_json)
        .bind(play.caa_id).bind(&play.caa_release_mbid)
        .bind(&q.format_type).bind(&q.codec).bind(q.bitrate).bind(q.sample_rate)
        .bind(q.bit_depth.map(|v| v as i32)).bind(q.channels.map(|v| v as i32))
        .bind(q.is_lossless).bind(q.dsd_rate).bind(q.dsd_multiplier.map(|v| v as i32))
        .bind(&q.delivery_codec).bind(q.delivery_bitrate)
        .bind(q.is_transcoded).bind(&q.transcode_reason).bind(q.quality_score)
        .bind(device_id).bind(chain_id).bind(listening_context).bind(submission_client)
        .bind(play.track_number)
        .execute(&self.pool).await?;
        Ok(true)
    }

    pub async fn exists(&self, user_id: i64, source_id: &str, source_name: &str) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND source_id = ? AND source_name = ?"
        ).bind(user_id).bind(source_id).bind(source_name).fetch_one(&self.pool).await?;
        Ok(count.0 > 0)
    }

    pub async fn fuzzy_exists(&self, user_id: i64, title: &str, artist: &str, timestamp: i64) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND title = ? AND artist = ?
             AND timestamp BETWEEN ? - 600 AND ? + 600"
        ).bind(user_id).bind(title).bind(artist).bind(timestamp).bind(timestamp)
        .fetch_one(&self.pool).await?;
        Ok(count.0 > 0)
    }

    pub async fn get_pending_scrobbles(&self) -> Result<Vec<PendingScrobble>, sqlx::Error> {
        let records: Vec<ScrobbleRecord> = sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE status = 'pending' ORDER BY timestamp ASC"
        ).fetch_all(&self.pool).await?;

        Ok(records.into_iter().map(|r| {
            let mbid_artist = r.mbid_artist.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());
            let delivered = r.delivered_sinks.as_deref()
                .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                .unwrap_or_default();
            PendingScrobble {
                id: r.id,
                user_id: r.user_id,
                delivered,
                play: Play {
                    title: r.title, artist: r.artist, album: r.album,
                    timestamp: r.timestamp as u64, duration: r.duration.map(|d| d as u64),
                    track_number: None, source_id: r.source_id, source_name: r.source_name,
                    mbid_recording: r.mbid_recording, mbid_release: r.mbid_release,
                    mbid_artist, artists: None, mbid_release_group: None,
                    caa_id: r.caa_id, caa_release_mbid: r.caa_release_mbid,
                },
            }
        }).collect())
    }

    /// Append a sink to a scrobble's delivered set (idempotent). Prevents a
    /// retry from re-sending to a sink that already accepted the listen.
    pub async fn mark_sink_delivered(&self, id: i64, sink_name: &str) -> Result<(), sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as("SELECT delivered_sinks FROM scrobbles WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?;
        let mut set: Vec<String> = row.and_then(|(s,)| s)
            .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if !set.iter().any(|s| s == sink_name) {
            set.push(sink_name.to_string());
            sqlx::query("UPDATE scrobbles SET delivered_sinks = ? WHERE id = ?")
                .bind(serde_json::to_string(&set).unwrap_or_else(|_| "[]".into())).bind(id)
                .execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Persist MusicBrainz IDs onto a stored listen after the engine enriches it
    /// (enrichment moved off the ingest request path — see server/ingest.rs).
    pub async fn update_scrobble_enrichment(&self, id: i64, play: &Play) -> Result<(), sqlx::Error> {
        let mbid_artist_json = play.mbid_artist.as_ref().map(|a| serde_json::to_string(a).unwrap_or_default());
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
            .bind(id).execute(&self.pool).await?;
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
        &self, user_id: i64, limit: i64, offset: i64,
        artist_filter: Option<&str>, album_filter: Option<&str>,
        after: Option<i64>, before: Option<i64>,
    ) -> Result<Vec<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE user_id = ? AND skipped = FALSE
               AND (? IS NULL OR LOWER(artist) LIKE '%' || LOWER(?) || '%')
               AND (? IS NULL OR LOWER(album) LIKE '%' || LOWER(?) || '%')
               AND (? IS NULL OR timestamp >= ?)
               AND (? IS NULL OR timestamp <= ?)
             ORDER BY timestamp DESC LIMIT ? OFFSET ?"
        )
        .bind(user_id)
        .bind(artist_filter).bind(artist_filter)
        .bind(album_filter).bind(album_filter)
        .bind(after).bind(after).bind(before).bind(before)
        .bind(limit).bind(offset)
        .fetch_all(&self.pool).await
    }

    /// (total listens, newest listened_at) for a user — the two numbers the
    /// ListenBrainz-compatible endpoints report alongside a listen page.
    pub async fn listen_count_and_latest(&self, user_id: i64) -> Result<(i64, Option<i64>), sqlx::Error> {
        let row: (i64, Option<i64>) = sqlx::query_as(
            "SELECT COUNT(*), MAX(timestamp) FROM scrobbles WHERE user_id = ? AND skipped = FALSE"
        ).bind(user_id).fetch_one(&self.pool).await?;
        Ok(row)
    }

    /// Minimal columns for analytics: (timestamp, artist, title, album, track_number).
    /// Capped to the most-recent `limit` rows to bound memory on large histories.
    pub async fn get_listen_rows(&self, user_id: i64, after: Option<i64>, limit: i64)
        -> Result<Vec<(i64, String, String, Option<String>, Option<i32>)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT timestamp, artist, title, album, track_number FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             ORDER BY timestamp DESC LIMIT ?"
        ).bind(user_id).bind(after).bind(after).bind(limit).fetch_all(&self.pool).await
    }

    // ── Language enrichment ──

    /// Listens still needing a language, newest first. (id, title, album, mbid_recording)
    pub async fn scrobbles_needing_language(&self, limit: i64) -> Result<Vec<(i64, String, Option<String>, Option<String>)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, title, album, mbid_recording FROM scrobbles
             WHERE language IS NULL AND skipped = FALSE ORDER BY timestamp DESC LIMIT ?"
        ).bind(limit).fetch_all(&self.pool).await
    }

    pub async fn set_scrobble_language(&self, id: i64, language: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE scrobbles SET language = ? WHERE id = ?")
            .bind(language).bind(id).execute(&self.pool).await?;
        Ok(())
    }

    /// Listens with an MBID but no resolved release year yet. Rows without an
    /// MBID are skipped: there's nothing to look them up with.
    pub async fn scrobbles_needing_release_year(&self, limit: i64) -> Result<Vec<(i64, String, String, Option<String>)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, title, artist, album FROM scrobbles
             WHERE release_year IS NULL AND skipped = FALSE ORDER BY timestamp DESC LIMIT ?"
        ).bind(limit).fetch_all(&self.pool).await
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
            .bind(year).bind(id).execute(&self.pool).await?;
        Ok(())
    }

    /// (total, with cover art) — drives the Settings artwork progress readout.
    pub async fn artwork_coverage(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE")
            .bind(user_id).fetch_one(&self.pool).await?;
        let with_art: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND artwork_url IS NOT NULL"
        ).bind(user_id).fetch_one(&self.pool).await?;
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
    pub async fn scrobbles_needing_artwork(&self, limit: i64)
        -> Result<Vec<(i64, String, String, Option<String>, Option<i64>, Option<String>)>, sqlx::Error>
    {
        sqlx::query_as(
            "SELECT id, title, artist, album, caa_id, caa_release_mbid FROM scrobbles
             WHERE artwork_url IS NULL AND skipped = FALSE
             ORDER BY timestamp DESC LIMIT ?"
        ).bind(limit).fetch_all(&self.pool).await
    }

    /// Record artwork for a listen. Pass `""` to tombstone a row whose artwork
    /// could not be found anywhere, so later passes skip it instead of re-asking
    /// every provider forever. `caa_id` is only written when a lookup produced
    /// one; it is provenance, not queue state.
    pub async fn set_scrobble_artwork(
        &self, id: i64, url: &str, caa_id: Option<i64>, caa_release_mbid: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE scrobbles SET artwork_url = ?,
                caa_id = COALESCE(?, caa_id),
                caa_release_mbid = COALESCE(?, caa_release_mbid)
             WHERE id = ?"
        )
        .bind(url).bind(caa_id).bind(caa_release_mbid).bind(id)
        .execute(&self.pool).await?;
        Ok(())
    }

    /// (total, with a language set) — for backfill progress.
    pub async fn language_coverage(&self) -> Result<(i64, i64), sqlx::Error> {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles").fetch_one(&self.pool).await?;
        let done: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE language IS NOT NULL").fetch_one(&self.pool).await?;
        Ok((total.0, done.0))
    }

    pub async fn get_dashboard_stats(&self, user_id: i64) -> Result<serde_json::Value, sqlx::Error> {
        let now = Self::now_secs();
        let today_start = now - (now % 86400);
        let week_start = now - (7 * 86400);

        let today: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND timestamp >= ?")
            .bind(user_id).bind(today_start).fetch_one(&self.pool).await?;
        let week: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND timestamp >= ?")
            .bind(user_id).bind(week_start).fetch_one(&self.pool).await?;
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE")
            .bind(user_id).fetch_one(&self.pool).await?;
        let lossless_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND is_lossless = TRUE")
            .bind(user_id).fetch_one(&self.pool).await?;
        let lossless_pct = if total.0 > 0 { (lossless_count.0 as f64 / total.0 as f64 * 100.0).round() as i64 } else { 0 };
        let top_artist: Option<(String, i64)> = sqlx::query_as(
            "SELECT artist, COUNT(*) as cnt FROM scrobbles WHERE user_id = ? AND timestamp >= ? GROUP BY artist ORDER BY cnt DESC LIMIT 1"
        ).bind(user_id).bind(now - 30 * 86400).fetch_optional(&self.pool).await?;
        let unique_artists: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE")
            .bind(user_id).fetch_one(&self.pool).await?;
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
        &self, user_id: i64, id: i64,
        title: Option<&str>, artist: Option<&str>, album: Option<&str>,
        mbid_recording: Option<&str>, chain_id: Option<i64>,
        listening_context: Option<&str>, format_type: Option<&str>, codec: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE scrobbles SET
                title = COALESCE(?, title),
                artist = COALESCE(?, artist),
                album = COALESCE(?, album),
                mbid_recording = COALESCE(?, mbid_recording),
                chain_id = COALESCE(?, chain_id),
                listening_context = COALESCE(?, listening_context),
                format_type = COALESCE(?, format_type),
                codec = COALESCE(?, codec)
             WHERE id = ? AND user_id = ?"
        )
        .bind(title).bind(artist).bind(album)
        .bind(mbid_recording).bind(chain_id)
        .bind(listening_context).bind(format_type).bind(codec)
        .bind(id).bind(user_id)
        .execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete a single listen owned by the user. Returns false if not found.
    pub async fn delete_scrobble(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM scrobbles WHERE id = ? AND user_id = ?")
            .bind(id).bind(user_id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete ALL listens for the user. Returns the number removed.
    pub async fn clear_scrobbles(&self, user_id: i64) -> Result<u64, sqlx::Error> {
        let res = sqlx::query("DELETE FROM scrobbles WHERE user_id = ?")
            .bind(user_id).execute(&self.pool).await?;
        Ok(res.rows_affected())
    }

    /// A single listen by id, scoped to the user.
    pub async fn get_scrobble(&self, user_id: i64, id: i64) -> Result<Option<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>("SELECT * FROM scrobbles WHERE id = ? AND user_id = ?")
            .bind(id).bind(user_id).fetch_optional(&self.pool).await
    }

    /// Every listen for a user, newest first — for full-backup export.
    pub async fn get_all_scrobbles(&self, user_id: i64) -> Result<Vec<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE user_id = ? ORDER BY timestamp DESC"
        ).bind(user_id).fetch_all(&self.pool).await
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
    pub async fn import_listen_flagged(&self, user_id: i64, play: &Play, skipped: bool) -> Result<bool, sqlx::Error> {
        if self.exists(user_id, &play.source_id, &play.source_name).await? { return Ok(false); }
        if self.fuzzy_exists(user_id, &play.title, &play.artist, play.timestamp as i64).await? {
            return Ok(false);
        }
        let mbid_artist_json = play.mbid_artist.as_ref().map(|arr| serde_json::to_string(arr).unwrap());
        sqlx::query(
            "INSERT INTO scrobbles (
                user_id, title, artist, album, timestamp, duration, source_id, source_name, status,
                mbid_recording, mbid_release, mbid_artist, caa_id, caa_release_mbid,
                submission_client, track_number, skipped
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, ?, ?, ?, 'import', ?, ?)"
        )
        .bind(user_id).bind(&play.title).bind(&play.artist).bind(&play.album)
        .bind(play.timestamp as i64).bind(play.duration.map(|d| d as i64))
        .bind(&play.source_id).bind(&play.source_name)
        .bind(&play.mbid_recording).bind(&play.mbid_release).bind(&mbid_artist_json)
        .bind(play.caa_id).bind(&play.caa_release_mbid)
        .bind(play.track_number).bind(skipped)
        .execute(&self.pool).await?;
        Ok(true)
    }

    // ════════════════════════════════════════════════════════════
    //  Ranged analytics  (GET /api/v1/stats?range=...)
    // ════════════════════════════════════════════════════════════

    /// `after` = None means all-time. Genres/languages/regions/decades need
    /// MusicBrainz enrichment we don't have yet, so they return empty arrays
    /// (the UI hides empty breakdowns). totals/top_artists/heatmap are real.
    pub async fn get_ranged_stats(&self, user_id: i64, after: Option<i64>) -> Result<serde_json::Value, sqlx::Error> {
        let plays: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let total = plays.0;

        let secs: (Option<i64>,) = sqlx::query_as(
            "SELECT SUM(duration) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND duration IS NOT NULL"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let hours = secs.0.unwrap_or(0) as f64 / 3600.0;

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
        let lossless: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND is_lossless = TRUE"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let dsd: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?) AND format_type = 'dsd'"
        ).bind(user_id).bind(after).bind(after).fetch_one(&self.pool).await?;
        let pct = |n: i64| if total > 0 { (n as f64 / total as f64 * 100.0).round() as i64 } else { 0 };

        let top_rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT artist, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY artist ORDER BY plays DESC LIMIT 10"
        ).bind(user_id).bind(after).bind(after).fetch_all(&self.pool).await?;
        let top_artists: Vec<serde_json::Value> = top_rows.into_iter().map(|(name, plays, secs)| {
            serde_json::json!({ "name": name, "plays": plays, "hours": (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0 })
        }).collect();

        let album_rows: Vec<(String, String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT album, artist, COUNT(*) as plays, SUM(duration) as secs
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND album IS NOT NULL AND album != ''
             GROUP BY album, artist ORDER BY plays DESC LIMIT 10"
        ).bind(user_id).bind(after).bind(after).fetch_all(&self.pool).await?;
        let top_albums: Vec<serde_json::Value> = album_rows.into_iter().map(|(name, artist, plays, secs)| {
            serde_json::json!({ "name": name, "artist": artist, "plays": plays,
                "hours": (secs.unwrap_or(0) as f64 / 3600.0 * 10.0).round() / 10.0 })
        }).collect();

        // Fidelity mix, bucketed the way the badges are: DSD > lossless > lossy,
        // with anything unlabelled called out rather than silently dropped.
        let fidelity_rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT CASE
                    WHEN format_type = 'dsd' THEN 'DSD'
                    WHEN is_lossless = TRUE THEN 'Lossless'
                    WHEN codec IS NOT NULL AND codec != '' THEN 'Lossy'
                    ELSE 'Unknown'
                END AS tier, COUNT(*) as c
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             GROUP BY tier ORDER BY c DESC"
        ).bind(user_id).bind(after).bind(after).fetch_all(&self.pool).await?;
        let fidelity: Vec<serde_json::Value> = fidelity_rows.into_iter().map(|(name, c)| {
            serde_json::json!({ "name": name, "plays": c, "pct": pct(c) })
        }).collect();

        // Release decades. Only listens whose release year has been resolved by
        // the MusicBrainz backfill count — the rest are simply absent.
        let decade_rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT (release_year / 10) * 10 AS decade, COUNT(*) as c
             FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
               AND release_year IS NOT NULL AND release_year > 1000
             GROUP BY decade ORDER BY decade"
        ).bind(user_id).bind(after).bind(after).fetch_all(&self.pool).await?;
        let decade_total: i64 = decade_rows.iter().map(|(_, c)| c).sum();
        let decades: Vec<serde_json::Value> = decade_rows.into_iter().map(|(d, c)| {
            serde_json::json!({ "name": format!("{}s", d), "plays": c,
                "pct": if decade_total > 0 { (c as f64 / decade_total as f64 * 100.0).round() as i64 } else { 0 } })
        }).collect();

        // Language breakdown (only listens that have been classified).
        let lang_rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT language, COUNT(*) as c FROM scrobbles
             WHERE user_id = ? AND (? IS NULL OR timestamp >= ?) AND language IS NOT NULL
             GROUP BY language ORDER BY c DESC LIMIT 12"
        ).bind(user_id).bind(after).bind(after).fetch_all(&self.pool).await?;
        let lang_total: i64 = lang_rows.iter().map(|(_, c)| c).sum();
        let languages: Vec<serde_json::Value> = lang_rows.into_iter().map(|(name, c)| {
            serde_json::json!({ "name": name, "pct": if lang_total > 0 { (c as f64 / lang_total as f64 * 100.0).round() as i64 } else { 0 } })
        }).collect();

        Ok(serde_json::json!({
            "totals": {
                "plays": total, "hours": (hours * 10.0).round() / 10.0,
                "artists": artists.0, "albums": albums.0, "tracks": tracks.0,
                "avg_quality": avg_quality.0.map(|q| (q * 10.0).round() / 10.0).unwrap_or(0.0),
                "lossless_pct": pct(lossless.0), "dsd_pct": pct(dsd.0),
            },
            "top_artists": top_artists,
            "top_albums": top_albums,
            "fidelity": fidelity,
            "decades": decades,
            "genres": [], "languages": languages, "regions": [],
            // heatmap is filled in by the handler in the user's local timezone
            "heatmap": Vec::<Vec<i64>>::new(),
        }))
    }

    // ════════════════════════════════════════════════════════════
    //  User preferences  (stored in users.settings JSON — no new table)
    // ════════════════════════════════════════════════════════════

    /// Returns the user's `settings` JSON object (defaults to `{}`).
    pub async fn get_preferences(&self, user_id: i64) -> Result<serde_json::Value, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as("SELECT settings FROM users WHERE id = ?")
            .bind(user_id).fetch_optional(&self.pool).await?;
        let json = row.and_then(|(s,)| s)
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .filter(|v| v.is_object())
            .unwrap_or_else(|| serde_json::json!({}));
        Ok(json)
    }

    /// Shallow-merge `patch` into the user's stored preferences and persist.
    /// Returns the merged object.
    pub async fn merge_preferences(&self, user_id: i64, patch: &serde_json::Value) -> Result<serde_json::Value, sqlx::Error> {
        let mut current = self.get_preferences(user_id).await?;
        if let (Some(obj), Some(p)) = (current.as_object_mut(), patch.as_object()) {
            for (k, v) in p { obj.insert(k.clone(), v.clone()); }
        }
        sqlx::query("UPDATE users SET settings = ? WHERE id = ?")
            .bind(current.to_string()).bind(user_id).execute(&self.pool).await?;
        Ok(current)
    }

    // ════════════════════════════════════════════════════════════
    //  Signal Chains
    // ════════════════════════════════════════════════════════════

    pub async fn create_chain(&self, chain: &SignalChain) -> Result<i64, sqlx::Error> {
        let components_json = serde_json::to_string(&chain.components).unwrap_or_else(|_| "[]".into());
        let now = Self::now_secs();
        let result = sqlx::query(
            "INSERT INTO signal_chains (user_id, name, description, components, listening_context, is_active, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        ).bind(chain.user_id).bind(&chain.name).bind(&chain.description)
        .bind(&components_json).bind(chain.listening_context.as_str()).bind(chain.is_active).bind(now)
        .execute(&self.pool).await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn get_chains(&self, user_id: i64) -> Result<Vec<SignalChain>, sqlx::Error> {
        let rows: Vec<ChainRow> = sqlx::query_as("SELECT * FROM signal_chains WHERE user_id = ? ORDER BY name ASC")
            .bind(user_id).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    pub async fn get_chain(&self, user_id: i64, chain_id: i64) -> Result<Option<SignalChain>, sqlx::Error> {
        let row: Option<ChainRow> = sqlx::query_as("SELECT * FROM signal_chains WHERE id = ? AND user_id = ?")
            .bind(chain_id).bind(user_id).fetch_optional(&self.pool).await?;
        Ok(row.map(|r| r.into()))
    }

    pub async fn update_chain_hours(&self, chain_id: i64, hours_to_add: f64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE signal_chains SET total_hours = total_hours + ? WHERE id = ?")
            .bind(hours_to_add).bind(chain_id).execute(&self.pool).await?;
        Ok(())
    }

    /// Resolve a chain *name* (as sent by clients in `tapedeck_chain.chain_id`)
    /// to its id for this user. Case-insensitive.
    pub async fn get_chain_by_name(&self, user_id: i64, name: &str) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM signal_chains WHERE user_id = ? AND LOWER(name) = LOWER(?)"
        ).bind(user_id).bind(name).fetch_optional(&self.pool).await?;
        Ok(row.map(|(id,)| id))
    }

    /// A source device's configured default chain, if any (fallback for listens
    /// that don't carry an explicit chain or output binding).
    pub async fn get_device_default_chain(&self, user_id: i64, device_id: i64) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT default_chain_id FROM devices WHERE id = ? AND user_id = ?"
        ).bind(device_id).bind(user_id).fetch_optional(&self.pool).await?;
        Ok(row.and_then(|(c,)| c))
    }

    pub async fn update_chain(&self, user_id: i64, chain_id: i64, name: &str, description: Option<&str>,
        components: &[ChainComponent], listening_context: &str) -> Result<bool, sqlx::Error> {
        let components_json = serde_json::to_string(components).unwrap_or_else(|_| "[]".into());
        let res = sqlx::query(
            "UPDATE signal_chains SET name = ?, description = ?, components = ?, listening_context = ?
             WHERE id = ? AND user_id = ?"
        ).bind(name).bind(description).bind(&components_json).bind(listening_context)
         .bind(chain_id).bind(user_id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete a chain, first unlinking (not deleting) anything that references
    /// it — listens and sessions keep their data with `chain_id` cleared. Done
    /// in a transaction so we never leave dangling FKs (sqlx enables them).
    pub async fn delete_chain(&self, user_id: i64, chain_id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE scrobbles SET chain_id = NULL WHERE chain_id = ? AND user_id = ?")
            .bind(chain_id).bind(user_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE sessions SET chain_id = NULL WHERE chain_id = ? AND user_id = ?")
            .bind(chain_id).bind(user_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE devices SET default_chain_id = NULL WHERE default_chain_id = ? AND user_id = ?")
            .bind(chain_id).bind(user_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE chain_bindings SET chain_id = NULL WHERE chain_id = ? AND user_id = ?")
            .bind(chain_id).bind(user_id).execute(&mut *tx).await?;
        let res = sqlx::query("DELETE FROM signal_chains WHERE id = ? AND user_id = ?")
            .bind(chain_id).bind(user_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    // ════════════════════════════════════════════════════════════
    //  Output-device → chain bindings (Phase B)
    // ════════════════════════════════════════════════════════════

    /// Record that we saw a listen come through this output device. Inserts a
    /// new unmapped row (chain_id NULL) the first time, else bumps hits/last_seen
    /// and refreshes the interface. Never overwrites an existing chain mapping.
    pub async fn record_output(&self, user_id: i64, identifier: &str, interface: Option<&str>) -> Result<(), sqlx::Error> {
        let now = Self::now_secs();
        sqlx::query(
            "INSERT INTO chain_bindings (user_id, identifier, interface, chain_id, last_seen, hits)
             VALUES (?, ?, ?, NULL, ?, 1)
             ON CONFLICT(user_id, identifier) DO UPDATE SET
                hits = hits + 1, last_seen = excluded.last_seen,
                interface = COALESCE(excluded.interface, chain_bindings.interface)"
        ).bind(user_id).bind(identifier).bind(interface).bind(now)
         .execute(&self.pool).await?;
        Ok(())
    }

    /// Resolve an output-device identifier to its bound chain, if mapped.
    pub async fn resolve_binding(&self, user_id: i64, identifier: &str) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT chain_id FROM chain_bindings WHERE user_id = ? AND identifier = ?"
        ).bind(user_id).bind(identifier).fetch_optional(&self.pool).await?;
        Ok(row.and_then(|(c,)| c))
    }

    pub async fn list_bindings(&self, user_id: i64) -> Result<Vec<BindingRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM chain_bindings WHERE user_id = ? ORDER BY chain_id IS NOT NULL, last_seen DESC")
            .bind(user_id).fetch_all(&self.pool).await
    }

    /// Assign (or clear, with None) the chain for an output identifier. Creates
    /// the binding if the identifier hasn't been seen yet (manual add).
    pub async fn set_binding(&self, user_id: i64, identifier: &str, chain_id: Option<i64>) -> Result<(), sqlx::Error> {
        let now = Self::now_secs();
        sqlx::query(
            "INSERT INTO chain_bindings (user_id, identifier, chain_id, last_seen, hits)
             VALUES (?, ?, ?, ?, 0)
             ON CONFLICT(user_id, identifier) DO UPDATE SET chain_id = excluded.chain_id"
        ).bind(user_id).bind(identifier).bind(chain_id).bind(now)
         .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn delete_binding(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM chain_bindings WHERE id = ? AND user_id = ?")
            .bind(id).bind(user_id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    // ════════════════════════════════════════════════════════════
    //  Import history (persisted; the live job registry is in-memory)
    // ════════════════════════════════════════════════════════════

    // ════════════════════════════════════════════════════════════
    //  Per-user scrobble connections
    // ════════════════════════════════════════════════════════════

    pub async fn set_connection(&self, user_id: i64, service: &str, credentials: &str, username: Option<&str>) -> Result<(), sqlx::Error> {
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

    pub async fn delete_connection(&self, user_id: i64, service: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM user_connections WHERE user_id = ? AND service = ?")
            .bind(user_id).bind(service).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Public-facing list: service + connected account only, never credentials.
    pub async fn list_connections(&self, user_id: i64) -> Result<Vec<(String, Option<String>, i64)>, sqlx::Error> {
        sqlx::query_as("SELECT service, connected_username, connected_at FROM user_connections WHERE user_id = ?")
            .bind(user_id).fetch_all(&self.pool).await
    }

    /// Credentials for the engine to build this user's sinks. `(service, credentials_json)`.
    pub async fn get_connections_for_forwarding(&self, user_id: i64) -> Result<Vec<(String, String)>, sqlx::Error> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT service, credentials FROM user_connections WHERE user_id = ?"
        ).bind(user_id).fetch_all(&self.pool).await?;
        // A row we can't decrypt is skipped rather than fatal: one bad credential
        // shouldn't stop the user's other services from receiving listens.
        Ok(rows.into_iter().filter_map(|(service, blob)| {
            match self.secrets.decrypt(&blob) {
                Ok(plain) => Some((service, plain)),
                Err(e) => { error!("Undecryptable {} credential for user {}: {}", service, user_id, e); None }
            }
        }).collect())
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
            Err(e) => { error!("Could not decrypt stored credential: {}", e); None }
        }
    }

    /// One-time upgrade for rows written before encryption existed. Idempotent —
    /// already-encrypted values carry a prefix and are skipped, so this is safe
    /// to run on every boot.
    async fn encrypt_legacy_credentials(&self) -> Result<(), sqlx::Error> {
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT user_id, service, credentials FROM user_connections"
        ).fetch_all(&self.pool).await?;

        let mut upgraded = 0;
        for (user_id, service, blob) in rows {
            if SecretBox::is_encrypted(&blob) || blob.is_empty() { continue; }
            let sealed = self.seal(&blob);
            if sealed.is_empty() { continue; }
            sqlx::query("UPDATE user_connections SET credentials = ? WHERE user_id = ? AND service = ?")
                .bind(sealed).bind(user_id).bind(&service)
                .execute(&self.pool).await?;
            upgraded += 1;
        }
        if upgraded > 0 {
            info!("🔐 Encrypted {} stored credential(s) at rest", upgraded);
        }
        Ok(())
    }

    /// Lowest-numbered admin — the migration target for config that used to be
    /// instance-wide and now needs an owner.
    pub async fn first_admin_id(&self) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM users WHERE role = 'admin' AND disabled = 0 ORDER BY id LIMIT 1"
        ).fetch_optional(&self.pool).await?;
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
            "SELECT id, user_id, kind, enabled, url, credentials, remote_account,
                    poll_interval_secs, options, last_polled_at, last_error, updated_at
             FROM user_sources WHERE enabled = 1 ORDER BY user_id, kind"
        ).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().filter_map(|r| self.hydrate_source(r)).collect())
    }

    /// This user's sources, for the Settings screen. Credentials are decrypted
    /// here too — the handler decides what is safe to send to the browser.
    pub async fn list_sources_for_user(&self, user_id: i64) -> Result<Vec<UserSource>, sqlx::Error> {
        let rows: Vec<UserSourceRow> = sqlx::query_as(
            "SELECT id, user_id, kind, enabled, url, credentials, remote_account,
                    poll_interval_secs, options, last_polled_at, last_error, updated_at
             FROM user_sources WHERE user_id = ? ORDER BY kind"
        ).bind(user_id).fetch_all(&self.pool).await?;
        Ok(rows.into_iter().filter_map(|r| self.hydrate_source(r)).collect())
    }

    pub async fn get_source(&self, user_id: i64, kind: &str) -> Result<Option<UserSource>, sqlx::Error> {
        let row: Option<UserSourceRow> = sqlx::query_as(
            "SELECT id, user_id, kind, enabled, url, credentials, remote_account,
                    poll_interval_secs, options, last_polled_at, last_error, updated_at
             FROM user_sources WHERE user_id = ? AND kind = ?"
        ).bind(user_id).bind(kind).fetch_optional(&self.pool).await?;
        Ok(row.and_then(|r| self.hydrate_source(r)))
    }

    fn hydrate_source(&self, row: UserSourceRow) -> Option<UserSource> {
        let credentials = if row.credentials.is_empty() {
            String::new()
        } else {
            self.unseal(&row.credentials)?
        };
        Some(UserSource {
            id: row.id,
            user_id: row.user_id,
            kind: row.kind,
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

    /// Upsert a source. `updated_at` doubles as the change stamp the engine
    /// watches, so any save causes a rebuild on the next tick.
    #[allow(clippy::too_many_arguments)]
    pub async fn save_source(
        &self, user_id: i64, kind: &str, enabled: bool, url: &str,
        credentials: &str, remote_account: Option<&str>, poll_interval_secs: Option<i64>,
        options: &str,
    ) -> Result<(), sqlx::Error> {
        let sealed = if credentials.is_empty() { String::new() } else { self.seal(credentials) };
        sqlx::query(
            "INSERT INTO user_sources
                (user_id, kind, enabled, url, credentials, remote_account,
                 poll_interval_secs, options, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, kind) DO UPDATE SET
                enabled = excluded.enabled,
                url = excluded.url,
                credentials = excluded.credentials,
                remote_account = excluded.remote_account,
                poll_interval_secs = excluded.poll_interval_secs,
                options = excluded.options,
                updated_at = excluded.updated_at"
        )
        .bind(user_id).bind(kind).bind(enabled).bind(url)
        .bind(sealed).bind(remote_account).bind(poll_interval_secs)
        .bind(options).bind(Self::now_secs())
        .execute(&self.pool).await?;
        Ok(())
    }

    pub async fn delete_source(&self, user_id: i64, kind: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM user_sources WHERE user_id = ? AND kind = ?")
            .bind(user_id).bind(kind).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Record the outcome of a poll. Deliberately does *not* touch
    /// `updated_at` — that would look like a config change and make the engine
    /// rebuild the source on every tick.
    pub async fn record_source_poll(&self, id: i64, error: Option<&str>) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE user_sources SET last_polled_at = ?, last_error = ? WHERE id = ?")
            .bind(Self::now_secs()).bind(error).bind(id)
            .execute(&self.pool).await?;
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
             FROM plex_settings WHERE id = 1"
        ).fetch_optional(&self.pool).await
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
                updated_at=excluded.updated_at"
        )
        .bind(s.enabled).bind(&s.url).bind(&s.token)
        .bind(&s.users_allow).bind(&s.users_block)
        .bind(&s.devices_allow).bind(&s.devices_block)
        .bind(&s.libraries_allow).bind(&s.libraries_block)
        .bind(Self::now_secs())
        .execute(&self.pool).await?;
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
    pub async fn finish_import_job(&self, id: i64, state: &str, fetched: i64, imported: i64, status: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE import_jobs SET state = ?, fetched = ?, imported = ?, status = ? WHERE id = ?")
            .bind(state).bind(fetched).bind(imported).bind(status).bind(id).execute(&self.pool).await?;
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
        sqlx::query_as("SELECT * FROM import_jobs WHERE user_id = ? ORDER BY created_at DESC LIMIT 50")
            .bind(user_id).fetch_all(&self.pool).await
    }

    /// Bulk-reassign the chain for a set of listens (History bulk edit).
    pub async fn bulk_set_scrobble_chain(&self, user_id: i64, ids: &[i64], chain_id: Option<i64>) -> Result<u64, sqlx::Error> {
        if ids.is_empty() { return Ok(0); }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!("UPDATE scrobbles SET chain_id = ? WHERE user_id = ? AND id IN ({})", placeholders);
        let mut q = sqlx::query(&sql).bind(chain_id).bind(user_id);
        for id in ids { q = q.bind(id); }
        let res = q.execute(&self.pool).await?;
        Ok(res.rows_affected())
    }

    // ════════════════════════════════════════════════════════════
    //  Devices
    // ════════════════════════════════════════════════════════════

    pub async fn upsert_device(&self, user_id: i64, machine_id: &str, name: Option<&str>,
        platform: Option<&str>, product: Option<&str>) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM devices WHERE user_id = ? AND machine_id = ?")
            .bind(user_id).bind(machine_id).fetch_optional(&self.pool).await?;
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

    pub async fn get_devices(&self, user_id: i64) -> Result<Vec<DeviceRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM devices WHERE user_id = ? ORDER BY last_seen DESC")
            .bind(user_id).fetch_all(&self.pool).await
    }

    // ════════════════════════════════════════════════════════════
    //  Equipment
    // ════════════════════════════════════════════════════════════

    pub async fn upsert_equipment(&self, user_id: i64, name: &str, equipment_type: &str,
        brand: Option<&str>, model: Option<&str>, hours_to_add: f64) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM equipment WHERE user_id = ? AND name = ?")
            .bind(user_id).bind(name).fetch_optional(&self.pool).await?;
        if let Some((id,)) = existing {
            sqlx::query("UPDATE equipment SET total_hours = total_hours + ?, last_used = ? WHERE id = ?")
                .bind(hours_to_add).bind(now).bind(id).execute(&self.pool).await?;
            Ok(id)
        } else {
            let result = sqlx::query("INSERT INTO equipment (user_id, name, type, brand, model, total_hours, first_used, last_used)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(user_id).bind(name).bind(equipment_type).bind(brand).bind(model)
            .bind(hours_to_add).bind(now).bind(now).execute(&self.pool).await?;
            Ok(result.last_insert_rowid())
        }
    }

    pub async fn get_equipment(&self, user_id: i64) -> Result<Vec<EquipmentRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM equipment WHERE user_id = ? ORDER BY total_hours DESC")
            .bind(user_id).fetch_all(&self.pool).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_equipment(&self, user_id: i64, id: i64, name: &str, equipment_type: &str,
        brand: Option<&str>, model: Option<&str>, notes: Option<&str>) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE equipment SET name = ?, type = ?, brand = ?, model = ?, notes = ? WHERE id = ? AND user_id = ?"
        ).bind(name).bind(equipment_type).bind(brand).bind(model).bind(notes)
         .bind(id).bind(user_id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn delete_equipment(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("DELETE FROM equipment WHERE id = ? AND user_id = ?")
            .bind(id).bind(user_id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    // ════════════════════════════════════════════════════════════
    //  Listening Sessions
    // ════════════════════════════════════════════════════════════

    pub async fn assign_session(&self, user_id: i64, timestamp: i64, duration_secs: i64,
        device_id: Option<i64>, chain_id: Option<i64>, quality_score: Option<f64>,
        listening_context: &str, session_gap_seconds: i64) -> Result<i64, sqlx::Error> {
        let recent: Option<(i64, i64)> = sqlx::query_as(
            "SELECT id, ended_at FROM sessions WHERE user_id = ? AND ended_at >= ? - ? ORDER BY ended_at DESC LIMIT 1"
        ).bind(user_id).bind(timestamp).bind(session_gap_seconds).fetch_optional(&self.pool).await?;
        let ended_at = timestamp + duration_secs;
        if let Some((session_id, _)) = recent {
            sqlx::query("UPDATE sessions SET ended_at = MAX(ended_at, ?), track_count = track_count + 1,
                    total_duration = total_duration + ?,
                    avg_quality_score = CASE WHEN ? IS NOT NULL THEN
                        (COALESCE(avg_quality_score, 0) * track_count + ?) / (track_count + 1)
                    ELSE avg_quality_score END WHERE id = ?")
            .bind(ended_at).bind(duration_secs).bind(quality_score).bind(quality_score).bind(session_id)
            .execute(&self.pool).await?;
            Ok(session_id)
        } else {
            let result = sqlx::query("INSERT INTO sessions (user_id, device_id, chain_id, started_at, ended_at,
                    track_count, total_duration, avg_quality_score, listening_context)
                 VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?)")
            .bind(user_id).bind(device_id).bind(chain_id).bind(timestamp).bind(ended_at)
            .bind(duration_secs).bind(quality_score).bind(listening_context)
            .execute(&self.pool).await?;
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

    pub async fn validate_token(&self, token: &str) -> Result<Option<AuthUser>, sqlx::Error> {
        let token_hash = Self::hash_token(token);
        let now = Self::now_secs();
        let row: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT u.id, u.username, u.role FROM tokens t JOIN users u ON t.user_id = u.id
             WHERE t.token_hash = ? AND (t.expires_at IS NULL OR t.expires_at > ?) AND u.disabled = 0"
        ).bind(&token_hash).bind(now).fetch_optional(&self.pool).await?;

        if let Some((user_id, username, role)) = row {
            let _ = sqlx::query("UPDATE tokens SET last_used_at = ? WHERE token_hash = ?")
                .bind(now).bind(&token_hash).execute(&self.pool).await;
            Ok(Some(AuthUser { user_id, username, is_admin: role == "admin" }))
        } else {
            Ok(None)
        }
    }

    pub async fn create_user(&self, username: &str, display_name: Option<&str>,
        password_hash: &str, role: &str) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let result = sqlx::query(
            "INSERT INTO users (username, display_name, password_hash, role, created_at) VALUES (?, ?, ?, ?, ?)"
        ).bind(username).bind(display_name).bind(password_hash).bind(role).bind(now)
        .execute(&self.pool).await?;
        Ok(result.last_insert_rowid())
    }

    /// Returns (role, disabled) for a user, if they exist.
    pub async fn get_user_role_disabled(&self, id: i64) -> Result<Option<(String, bool)>, sqlx::Error> {
        let row: Option<(String, bool)> = sqlx::query_as("SELECT role, disabled FROM users WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?;
        Ok(row)
    }

    /// Count admins who can actually log in (enabled). Used to guard against
    /// leaving the instance with no reachable admin.
    pub async fn count_active_admins(&self) -> Result<i64, sqlx::Error> {
        let c: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE role = 'admin' AND disabled = 0")
            .fetch_one(&self.pool).await?;
        Ok(c.0)
    }

    pub async fn update_user_role(&self, id: i64, role: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(role).bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn set_user_disabled(&self, id: i64, disabled: bool) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE users SET disabled = ? WHERE id = ?")
            .bind(disabled).bind(id).execute(&self.pool).await?;
        // Disabling a user invalidates their live web sessions immediately.
        if disabled {
            let _ = sqlx::query("DELETE FROM web_sessions WHERE user_id = ?").bind(id).execute(&self.pool).await;
        }
        Ok(res.rows_affected() > 0)
    }

    pub async fn update_user_password(&self, id: i64, password_hash: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(password_hash).bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }

    /// Delete a user and everything they own, in one transaction. Every listen,
    /// token, chain, device, session, binding, and import record scoped to the
    /// user is removed first so foreign keys stay consistent.
    pub async fn delete_user(&self, id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for table in ["scrobbles", "tokens", "signal_chains", "devices",
                      "equipment", "sessions", "chain_bindings", "import_jobs",
                      "web_sessions", "user_connections"] {
            sqlx::query(&format!("DELETE FROM {} WHERE user_id = ?", table))
                .bind(id).execute(&mut *tx).await?;
        }
        let res = sqlx::query("DELETE FROM users WHERE id = ?").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn create_token(&self, user_id: i64, name: &str, scopes: &str) -> Result<String, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let token = format!("td_{}", hex::encode(raw));
        let token_hash = Self::hash_token(&token);
        let now = Self::now_secs();
        sqlx::query("INSERT INTO tokens (user_id, token_hash, name, scopes, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(user_id).bind(&token_hash).bind(name).bind(scopes).bind(now)
            .execute(&self.pool).await?;
        info!("🔑 Created token '{}' for user_id {}", name, user_id);
        Ok(token)
    }

    pub async fn delete_token(&self, user_id: i64, token_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM tokens WHERE id = ? AND user_id = ?")
            .bind(token_id).bind(user_id).execute(&self.pool).await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn has_users(&self) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users").fetch_one(&self.pool).await?;
        Ok(count.0 > 0)
    }

    pub async fn list_users(&self) -> Result<Vec<UserRecord>, sqlx::Error> {
        sqlx::query_as("SELECT id, username, display_name, role, created_at, disabled FROM users ORDER BY id ASC")
            .fetch_all(&self.pool).await
    }

    pub async fn list_tokens(&self, user_id: i64) -> Result<Vec<TokenRecord>, sqlx::Error> {
        sqlx::query_as("SELECT id, name, scopes, created_at, last_used_at FROM tokens WHERE user_id = ? ORDER BY created_at ASC")
            .bind(user_id).fetch_all(&self.pool).await
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

#[derive(Debug, FromRow, serde::Serialize)]
pub struct TokenRecord {
    pub id: i64,
    pub name: String,
    pub scopes: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, FromRow)]
pub struct ChainRow {
    pub id: i64, pub user_id: i64, pub name: String,
    pub description: Option<String>, pub components: String,
    pub listening_context: String, pub is_active: bool,
    pub total_hours: f64, pub created_at: i64,
}

impl From<ChainRow> for SignalChain {
    fn from(r: ChainRow) -> Self {
        let components = serde_json::from_str(&r.components).unwrap_or_default();
        SignalChain {
            id: Some(r.id), user_id: r.user_id, name: r.name,
            description: r.description, components,
            listening_context: ListeningContext::from_str_loose(&r.listening_context),
            is_active: r.is_active, total_hours: r.total_hours, created_at: r.created_at,
        }
    }
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct DeviceRow {
    pub id: i64, pub user_id: i64, pub machine_id: String,
    pub name: Option<String>, pub platform: Option<String>,
    pub product: Option<String>, pub device_type: Option<String>,
    pub default_chain_id: Option<i64>,
    pub first_seen: i64, pub last_seen: i64, pub total_listens: i64,
}

#[derive(Debug, FromRow, serde::Serialize)]
pub struct EquipmentRow {
    pub id: i64, pub user_id: i64, pub name: String,
    #[sqlx(rename = "type")]
    pub equipment_type: String,
    pub brand: Option<String>, pub model: Option<String>,
    pub total_hours: f64, pub first_used: Option<i64>,
    pub last_used: Option<i64>, pub notes: Option<String>,
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
        Database::new(
            &format!("sqlite:{}", path.display()),
            Arc::new(SecretBox::test_key()),
        ).await.unwrap()
    }

    fn play(title: &str, artist: &str, ts: u64) -> Play {
        Play::new(title.into(), artist.into(), ts, format!("src-{ts}"), "test".into())
    }

    #[tokio::test]
    async fn dedup_rejects_duplicate_source() {
        let db = testdb().await;
        let p = play("Teardrop", "Massive Attack", 1000);
        assert!(db.save_scrobble(1, &p, None, None, None, None, None).await.unwrap());
        // same source_id + name → deduped
        assert!(!db.save_scrobble(1, &p, None, None, None, None, None).await.unwrap());
    }

    #[tokio::test]
    async fn per_sink_delivery_is_tracked() {
        let db = testdb().await;
        db.save_scrobble(1, &play("T", "A", 2000), None, None, None, None, None).await.unwrap();

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
            id: None, user_id: uid, name: "Desktop".into(), description: None,
            components: vec![], listening_context: ListeningContext::Active,
            is_active: true, total_hours: 0.0, created_at: 0,
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
        let cid = db.create_chain(&SignalChain {
            id: None, user_id: uid, name: "C".into(), description: None, components: vec![],
            listening_context: ListeningContext::Active, is_active: true, total_hours: 0.0, created_at: 0,
        }).await.unwrap();
        db.save_scrobble(uid, &play("a", "x", 10), None, None, None, None, None).await.unwrap();
        db.save_scrobble(uid, &play("b", "y", 20), None, None, None, None, None).await.unwrap();
        let ids: Vec<i64> = db.get_all_scrobbles(uid).await.unwrap().iter().map(|r| r.id).collect();

        let n = db.bulk_set_scrobble_chain(uid, &ids[..1], Some(cid)).await.unwrap();
        assert_eq!(n, 1);
        let rows = db.get_all_scrobbles(uid).await.unwrap();
        assert_eq!(rows.iter().filter(|r| r.chain_id == Some(cid)).count(), 1);
    }

    #[tokio::test]
    async fn disabled_user_cannot_authenticate_by_password_token_or_session() {
        let db = testdb().await;
        let hash = Database::hash_password("password123").unwrap();
        let uid = db.create_user("carol", None, &hash, "user").await.unwrap();
        let token = db.create_token(uid, "t", "submit").await.unwrap();
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
        db.save_scrobble(uid, &play("s", "a", 5), None, None, None, None, None).await.unwrap();
        db.create_token(uid, "t", "submit").await.unwrap();
        // A connection has an FK to users — deletion must clear it, or the whole
        // cascade rolls back (the bug this guards against).
        db.set_connection(uid, "listenbrainz", r#"{"token":"secret"}"#, Some("dave")).await.unwrap();

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
        db.set_connection(uid, "listenbrainz", r#"{"token":"secret"}"#, Some("erin_lb")).await.unwrap();

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
        db.set_connection(uid, "listenbrainz", r#"{"token":"PLAINTEXT_MARKER"}"#, None).await.unwrap();

        // The raw column must not contain the secret — a leaked backup is the
        // whole threat this defends against.
        let raw: (String,) = sqlx::query_as("SELECT credentials FROM user_connections WHERE user_id = ?")
            .bind(uid).fetch_one(&db.pool).await.unwrap();
        assert!(!raw.0.contains("PLAINTEXT_MARKER"), "credential stored in the clear: {}", raw.0);
        assert!(SecretBox::is_encrypted(&raw.0));

        // …while the forwarding path still gets usable plaintext back.
        let fwd = db.get_connections_for_forwarding(uid).await.unwrap();
        assert_eq!(fwd, vec![("listenbrainz".to_string(), r#"{"token":"PLAINTEXT_MARKER"}"#.to_string())]);
    }

    #[tokio::test]
    async fn source_credentials_round_trip_and_stay_sealed() {
        let db = testdb().await;
        let uid = db.create_user("grace", None, "x", "user").await.unwrap();
        db.save_source(uid, "plex", true, "http://plex.local:32400",
            r#"{"token":"PLEX_SECRET"}"#, Some("grace_plex"), None, "{}").await.unwrap();

        let raw: (String,) = sqlx::query_as("SELECT credentials FROM user_sources WHERE user_id = ?")
            .bind(uid).fetch_one(&db.pool).await.unwrap();
        assert!(!raw.0.contains("PLEX_SECRET"));

        let src = db.get_source(uid, "plex").await.unwrap().unwrap();
        assert_eq!(src.credential("token").as_deref(), Some("PLEX_SECRET"));
        assert_eq!(src.remote_account.as_deref(), Some("grace_plex"));
        assert!(src.is_usable());
    }

    #[tokio::test]
    async fn sources_are_scoped_per_user() {
        let db = testdb().await;
        let a = db.create_user("heidi", None, "x", "user").await.unwrap();
        let b = db.create_user("ivan", None, "x", "user").await.unwrap();
        db.save_source(a, "plex", true, "http://a", r#"{"token":"ta"}"#, Some("heidi_plex"), None, "{}").await.unwrap();
        db.save_source(b, "plex", true, "http://b", r#"{"token":"tb"}"#, Some("ivan_plex"), None, "{}").await.unwrap();

        // Each user sees only their own row…
        assert_eq!(db.list_sources_for_user(a).await.unwrap().len(), 1);
        assert_eq!(db.get_source(a, "plex").await.unwrap().unwrap().url, "http://a");
        assert!(db.get_source(b, "plex").await.unwrap().unwrap().credential("token").as_deref() == Some("tb"));

        // …while the engine sees both, each carrying its owner.
        let all = db.list_enabled_sources().await.unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|s| s.user_id == a && s.remote_account.as_deref() == Some("heidi_plex")));
        assert!(all.iter().any(|s| s.user_id == b && s.remote_account.as_deref() == Some("ivan_plex")));

        // Disabling drops it from the engine's view but keeps the config.
        db.save_source(a, "plex", false, "http://a", r#"{"token":"ta"}"#, Some("heidi_plex"), None, "{}").await.unwrap();
        assert_eq!(db.list_enabled_sources().await.unwrap().len(), 1);
        assert!(db.get_source(a, "plex").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn recording_a_poll_does_not_look_like_a_config_change() {
        let db = testdb().await;
        let uid = db.create_user("judy", None, "x", "user").await.unwrap();
        db.save_source(uid, "plex", true, "http://p", r#"{"token":"t"}"#, Some("judy_plex"), None, "{}").await.unwrap();
        let before = db.get_source(uid, "plex").await.unwrap().unwrap();

        db.record_source_poll(before.id, Some("boom")).await.unwrap();
        let after = db.get_source(uid, "plex").await.unwrap().unwrap();

        // If a poll bumped updated_at the engine would rebuild the source every
        // single tick, reconnecting endlessly.
        assert_eq!(before.updated_at, after.updated_at);
        assert_eq!(after.last_error.as_deref(), Some("boom"));
        assert!(after.last_polled_at.is_some());
    }
}
