use sqlx::{sqlite::{SqlitePoolOptions, SqliteConnectOptions}, Pool, Sqlite, FromRow};
use std::str::FromStr;
use crate::models::*;
use tracing::{info, debug};
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
}

pub struct Database {
    pool: Pool<Sqlite>,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options).await?;
        let db = Self { pool };
        db.init().await?;
        db.migrate().await?;
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
                settings JSON DEFAULT '{}'
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
            "SELECT id, username, password_hash, role FROM users WHERE username = ?"
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
             WHERE ws.session_hash = ? AND ws.expires_at > ?"
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

    /// Clean up expired sessions.
    pub async fn cleanup_expired_sessions(&self) -> Result<u64, sqlx::Error> {
        let now = Self::now_secs();
        let result = sqlx::query("DELETE FROM web_sessions WHERE expires_at <= ?")
            .bind(now).execute(&self.pool).await?;
        Ok(result.rows_affected())
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
                device_id, chain_id, listening_context, submission_client
            ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, 'pending',
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
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

    pub async fn get_pending_scrobbles(&self) -> Result<Vec<(i64, Play)>, sqlx::Error> {
        let records: Vec<ScrobbleRecord> = sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE status = 'pending' ORDER BY timestamp ASC"
        ).fetch_all(&self.pool).await?;

        Ok(records.into_iter().map(|r| {
            let user_id = r.user_id;
            let mbid_artist = r.mbid_artist.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());
            (user_id, Play {
                title: r.title, artist: r.artist, album: r.album,
                timestamp: r.timestamp as u64, duration: r.duration.map(|d| d as u64),
                track_number: None, source_id: r.source_id, source_name: r.source_name,
                mbid_recording: r.mbid_recording, mbid_release: r.mbid_release,
                mbid_artist, artists: None, mbid_release_group: None,
                caa_id: r.caa_id, caa_release_mbid: r.caa_release_mbid,
            })
        }).collect())
    }

    pub async fn mark_as_scrobbled(&self, user_id: i64, source_id: &str, source_name: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE scrobbles SET status = 'synced' WHERE user_id = ? AND source_id = ? AND source_name = ?")
            .bind(user_id).bind(source_id).bind(source_name).execute(&self.pool).await?;
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
            "SELECT * FROM scrobbles WHERE user_id = ?
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

    pub async fn get_dashboard_stats(&self, user_id: i64) -> Result<serde_json::Value, sqlx::Error> {
        let now = Self::now_secs();
        let today_start = now - (now % 86400);
        let week_start = now - (7 * 86400);

        let today: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND timestamp >= ?")
            .bind(user_id).bind(today_start).fetch_one(&self.pool).await?;
        let week: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND timestamp >= ?")
            .bind(user_id).bind(week_start).fetch_one(&self.pool).await?;
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ?")
            .bind(user_id).fetch_one(&self.pool).await?;
        let lossless_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND is_lossless = TRUE")
            .bind(user_id).fetch_one(&self.pool).await?;
        let lossless_pct = if total.0 > 0 { (lossless_count.0 as f64 / total.0 as f64 * 100.0).round() as i64 } else { 0 };
        let top_artist: Option<(String, i64)> = sqlx::query_as(
            "SELECT artist, COUNT(*) as cnt FROM scrobbles WHERE user_id = ? AND timestamp >= ? GROUP BY artist ORDER BY cnt DESC LIMIT 1"
        ).bind(user_id).bind(now - 30 * 86400).fetch_optional(&self.pool).await?;
        let unique_artists: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT artist) FROM scrobbles WHERE user_id = ?")
            .bind(user_id).fetch_one(&self.pool).await?;
        let unique_albums: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT album) FROM scrobbles WHERE user_id = ? AND album IS NOT NULL")
            .bind(user_id).fetch_one(&self.pool).await?;
        let unique_tracks: (i64,) = sqlx::query_as("SELECT COUNT(DISTINCT title || '|' || artist) FROM scrobbles WHERE user_id = ?")
            .bind(user_id).fetch_one(&self.pool).await?;
        let total_duration: (Option<i64>,) = sqlx::query_as("SELECT SUM(duration) FROM scrobbles WHERE user_id = ? AND duration IS NOT NULL")
            .bind(user_id).fetch_one(&self.pool).await?;
        let total_hours = total_duration.0.unwrap_or(0) as f64 / 3600.0;
        let avg_quality: (Option<f64>,) = sqlx::query_as("SELECT AVG(quality_score) FROM scrobbles WHERE user_id = ? AND quality_score IS NOT NULL")
            .bind(user_id).fetch_one(&self.pool).await?;

        Ok(serde_json::json!({
            "today": today.0, "this_week": week.0, "total": total.0,
            "lossless_pct": lossless_pct,
            "top_artist": top_artist.as_ref().map(|(a, _)| a.as_str()).unwrap_or("—"),
            "top_artist_count": top_artist.as_ref().map(|(_, c)| *c).unwrap_or(0),
            "unique_artists": unique_artists.0, "unique_albums": unique_albums.0,
            "unique_tracks": unique_tracks.0,
            "total_hours": (total_hours * 10.0).round() / 10.0,
            "avg_quality": avg_quality.0.map(|q| (q * 10.0).round() / 10.0).unwrap_or(0.0),
        }))
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
             WHERE t.token_hash = ? AND (t.expires_at IS NULL OR t.expires_at > ?)"
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
        sqlx::query_as("SELECT id, username, display_name, role, created_at FROM users ORDER BY id ASC")
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
