use sqlx::{sqlite::{SqlitePoolOptions, SqliteConnectOptions}, Pool, Sqlite, FromRow};
use std::str::FromStr;
use crate::models::Play;
use log::{info, debug};

#[derive(Debug, FromRow)]
pub struct ScrobbleRecord {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub timestamp: i64,
    pub duration: Option<i64>,
    pub source_id: String,
    pub source_name: String,
    pub status: String,
}

pub struct Database {
    pool: Pool<Sqlite>,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        // Parse the connection string options
        let options = SqliteConnectOptions::from_str(database_url)?
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options).await?;

        let db = Self { pool };
        db.init().await?;
        Ok(db)
    }

    async fn init(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS scrobbles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT,
                timestamp INTEGER NOT NULL,
                duration INTEGER,
                source_id TEXT NOT NULL,
                source_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                UNIQUE(source_id, source_name)
            )"
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_scrobble(&self, play: &Play) -> Result<bool, sqlx::Error> {
        // 1. Exact match check
        if self.exists(&play.source_id, &play.source_name).await? {
            return Ok(false);
        }

        // 2. Fuzzy match check (Title + Artist + Time Delta)
        // This prevents duplicates where one source is "live session" and other is "history"
        if self.fuzzy_exists(&play.title, &play.artist, play.timestamp as i64).await? {
            debug!("Skipping duplicate play (fuzzy match): {} - {}", play.artist, play.title);
            return Ok(false);
        }

        sqlx::query(
            "INSERT INTO scrobbles (title, artist, album, timestamp, duration, source_id, source_name, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending')"
        )
        .bind(&play.title)
        .bind(&play.artist)
        .bind(&play.album)
        .bind(play.timestamp as i64)
        .bind(play.duration.map(|d| d as i64))
        .bind(&play.source_id)
        .bind(&play.source_name)
        .execute(&self.pool)
        .await?;

        Ok(true)
    }

    pub async fn exists(&self, source_id: &str, source_name: &str) -> Result<bool, sqlx::Error> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles WHERE source_id = ? AND source_name = ?"
        )
        .bind(source_id)
        .bind(source_name)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0 > 0)
    }

    pub async fn fuzzy_exists(&self, title: &str, artist: &str, timestamp: i64) -> Result<bool, sqlx::Error> {
        // Check for same song within +/- 10 minutes (600 seconds)
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM scrobbles 
             WHERE title = ? AND artist = ? 
             AND timestamp BETWEEN ? - 600 AND ? + 600"
        )
        .bind(title)
        .bind(artist)
        .bind(timestamp)
        .bind(timestamp)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0 > 0)
    }

    pub async fn get_pending_scrobbles(&self) -> Result<Vec<Play>, sqlx::Error> {
        let records: Vec<ScrobbleRecord> = sqlx::query_as::<_, ScrobbleRecord>(
            "SELECT * FROM scrobbles WHERE status = 'pending' ORDER BY timestamp ASC"
        )
        .fetch_all(&self.pool)
        .await?;

        let plays = records.into_iter().map(|r| Play {
            title: r.title,
            artist: r.artist,
            album: r.album,
            timestamp: r.timestamp as u64,
            duration: r.duration.map(|d| d as u64),
            track_number: None,
            source_id: r.source_id,
            source_name: r.source_name,
            mbid_recording: None,
            mbid_release: None,
            mbid_artist: None,
            artists: None,
            mbid_release_group: None,
        }).collect();

        Ok(plays)
    }

    pub async fn mark_as_scrobbled(&self, source_id: &str, source_name: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE scrobbles SET status = 'synced' WHERE source_id = ? AND source_name = ?"
        )
        .bind(source_id)
        .bind(source_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
