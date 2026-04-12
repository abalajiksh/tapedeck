use sqlx::{sqlite::{SqlitePoolOptions, SqliteConnectOptions}, Pool, Sqlite, FromRow};
use std::str::FromStr;
use crate::models::{AuthUser, Play};
use tracing::{info, debug};
use sha2::{Sha256, Digest};

#[derive(Debug, FromRow)]
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
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT,
                timestamp INTEGER NOT NULL,
                duration INTEGER,
                source_id TEXT NOT NULL,
                source_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                mbid_recording TEXT,
                mbid_release TEXT,
                mbid_artist TEXT,
                caa_id INTEGER,
                caa_release_mbid TEXT,
                UNIQUE(user_id, source_id, source_name)
            )"
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT UNIQUE NOT NULL,
                display_name TEXT,
                password_hash TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                settings JSON DEFAULT '{}'
            )"
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
                expires_at INTEGER
            )"
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn migrate(&self) -> Result<(), sqlx::Error> {
        let column_check: Result<(i64,), _> = sqlx::query_as(
            "SELECT COUNT(*) FROM pragma_table_info('scrobbles') WHERE name='mbid_recording'"
        )
        .fetch_one(&self.pool)
        .await;

        if let Ok((count,)) = column_check {
            if count == 0 {
                info!("⚙️ Migrating scrobbles database to add MusicBrainz and CAA fields...");

                sqlx::query("ALTER TABLE scrobbles ADD COLUMN mbid_recording TEXT")
                    .execute(&self.pool).await?;
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN mbid_release TEXT")
                    .execute(&self.pool).await?;
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN mbid_artist TEXT")
                    .execute(&self.pool).await?;
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN caa_id INTEGER")
                    .execute(&self.pool).await?;
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN caa_release_mbid TEXT")
                    .execute(&self.pool).await?;

                info!("✅ Database migration complete");
            }
        }

        // Migration: add user_id column if missing (v2 multi-user support)
        let user_id_check: Result<(i64,), _> = sqlx::query_as(
            "SELECT COUNT(*) FROM pragma_table_info('scrobbles') WHERE name='user_id'"
        )
        .fetch_one(&self.pool)
        .await;

        if let Ok((count,)) = user_id_check {
            if count == 0 {
                info!("⚙️ Migrating scrobbles database to add user_id...");
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1")
                    .execute(&self.pool).await?;
                // Recreate unique index to include user_id
                sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_scrobbles_user_source ON scrobbles(user_id, source_id, source_name)")
                    .execute(&self.pool).await?;
                info!("✅ user_id migration complete");
            }
        }

        Ok(())
    }

    pub async fn save_scrobble(&self, user_id: i64, play: &Play) -> Result<bool, sqlx::Error> {
        if self.exists(user_id, &play.source_id, &play.source_name).await? {
            return Ok(false);
        }

        if self.fuzzy_exists(user_id, &play.title, &play.artist, play.timestamp as i64).await? {
            debug!("Skipping duplicate play (fuzzy match): {} - {}", play.artist, play.title);
            return Ok(false);
        }

        let mbid_artist_json = play.mbid_artist.as_ref()
            .map(|arr| serde_json::to_string(arr).unwrap());

        sqlx::query(
            "INSERT INTO scrobbles (
                user_id, title, artist, album, timestamp, duration, source_id, source_name, status,
                mbid_recording, mbid_release, mbid_artist, caa_id, caa_release_mbid
            )
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?)"
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
        .execute(&self.pool)
        .await?;

        Ok(true)
    }

    pub async fn exists(&self, user_id: i64, source_id: &str, source_name: &str) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND source_id = ? AND source_name = ?"
        )
        .bind(user_id)
        .bind(source_id)
        .bind(source_name)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0 > 0)
    }

    pub async fn fuzzy_exists(&self, user_id: i64, title: &str, artist: &str, timestamp: i64) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles
             WHERE user_id = ? AND title = ? AND artist = ?
             AND timestamp BETWEEN ? - 600 AND ? + 600"
        )
        .bind(user_id)
        .bind(title)
        .bind(artist)
        .bind(timestamp)
        .bind(timestamp)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0 > 0)
    }

    /// Returns `(user_id, Play)` tuples for all pending scrobbles across all users.
    pub async fn get_pending_scrobbles(&self) -> Result<Vec<(i64, Play)>, sqlx::Error> {
        let records: Vec<ScrobbleRecord> = sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE status = 'pending' ORDER BY timestamp ASC"
        )
        .fetch_all(&self.pool)
        .await?;

        let plays = records.into_iter().map(|r| {
            let user_id = r.user_id;
            let mbid_artist = r.mbid_artist.and_then(|json_str| {
                serde_json::from_str::<Vec<String>>(&json_str).ok()
            });

            (user_id, Play {
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
            })
        }).collect();

        Ok(plays)
    }

    pub async fn mark_as_scrobbled(&self, user_id: i64, source_id: &str, source_name: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE scrobbles SET status = 'synced' WHERE user_id = ? AND source_id = ? AND source_name = ?"
        )
        .bind(user_id)
        .bind(source_id)
        .bind(source_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── User & Token Management (roadmap 4.3) ──

    fn hash_token(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Validate a token and return the associated user.
    /// Also updates `last_used_at`.
    pub async fn validate_token(&self, token: &str) -> Result<Option<AuthUser>, sqlx::Error> {
        let token_hash = Self::hash_token(token);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let row: Option<(i64, String)> = sqlx::query_as(
            "SELECT u.id, u.username
             FROM tokens t
             JOIN users u ON t.user_id = u.id
             WHERE t.token_hash = ?
               AND (t.expires_at IS NULL OR t.expires_at > ?)"
        )
        .bind(&token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        if let Some((user_id, username)) = &row {
            // Fire-and-forget last_used update
            let _ = sqlx::query("UPDATE tokens SET last_used_at = ? WHERE token_hash = ?")
                .bind(now)
                .bind(&token_hash)
                .execute(&self.pool)
                .await;

            Ok(Some(AuthUser {
                user_id: *user_id,
                username: username.clone(),
            }))
        } else {
            Ok(None)
        }
    }

    /// Create a new user. Returns the user id.
    pub async fn create_user(
        &self,
        username: &str,
        display_name: Option<&str>,
        password_hash: &str,
    ) -> Result<i64, sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let result = sqlx::query(
            "INSERT INTO users (username, display_name, password_hash, created_at)
             VALUES (?, ?, ?, ?)"
        )
        .bind(username)
        .bind(display_name)
        .bind(password_hash)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// Create a new API token. Returns the raw token string (`td_<hex>`).
    pub async fn create_token(
        &self,
        user_id: i64,
        name: &str,
        scopes: &str,
    ) -> Result<String, sqlx::Error> {
        use rand::Rng;
        let raw: [u8; 24] = rand::rng().random();
        let token = format!("td_{}", hex::encode(raw));
        let token_hash = Self::hash_token(&token);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        sqlx::query(
            "INSERT INTO tokens (user_id, token_hash, name, scopes, created_at)
             VALUES (?, ?, ?, ?, ?)"
        )
        .bind(user_id)
        .bind(&token_hash)
        .bind(name)
        .bind(scopes)
        .bind(now)
        .execute(&self.pool)
        .await?;

        info!("🔑 Created token '{}' for user_id {}", name, user_id);
        Ok(token)
    }

    /// Check if any users exist (for first-run setup).
    pub async fn has_users(&self) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(count.0 > 0)
    }

    /// List all users.
    pub async fn list_users(&self) -> Result<Vec<UserRecord>, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(
            "SELECT id, username, display_name, created_at FROM users ORDER BY id ASC"
        )
        .fetch_all(&self.pool)
        .await
    }

    /// List all tokens for a given user (without exposing the hash).
    pub async fn list_tokens(&self, user_id: i64) -> Result<Vec<TokenRecord>, sqlx::Error> {
        sqlx::query_as::<_, TokenRecord>(
            "SELECT id, name, scopes, created_at, last_used_at
             FROM tokens WHERE user_id = ? ORDER BY created_at ASC"
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
}

// ── Supporting record types ──

#[derive(Debug, FromRow, serde::Serialize)]
pub struct UserRecord {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
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
