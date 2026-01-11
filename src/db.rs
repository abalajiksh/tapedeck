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
        // Parse the connection string options
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
                UNIQUE(source_id, source_name)
            )"
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn migrate(&self) -> Result<(), sqlx::Error> {
        // Check if mbid_recording column exists
        let column_check: Result<(i64,), _> = sqlx::query_as(
            "SELECT COUNT(*) FROM pragma_table_info('scrobbles') WHERE name='mbid_recording'"
        )
        .fetch_one(&self.pool)
        .await;

        if let Ok((count,)) = column_check {
            if count == 0 {
                info!("⚙️ Migrating scrobbles database to add MusicBrainz and CAA fields...");
                
                // Add MBID columns
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN mbid_recording TEXT")
                    .execute(&self.pool)
                    .await?;
                
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN mbid_release TEXT")
                    .execute(&self.pool)
                    .await?;
                
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN mbid_artist TEXT")
                    .execute(&self.pool)
                    .await?;
                
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN caa_id INTEGER")
                    .execute(&self.pool)
                    .await?;
                
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN caa_release_mbid TEXT")
                    .execute(&self.pool)
                    .await?;
                
                info!("✅ Database migration complete");
            }
        }

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

        // Convert Vec<String> to JSON string for mbid_artist
        let mbid_artist_json = play.mbid_artist.as_ref()
            .map(|arr| serde_json::to_string(arr).unwrap());

        sqlx::query(
            "INSERT INTO scrobbles (
                title, artist, album, timestamp, duration, source_id, source_name, status,
                mbid_recording, mbid_release, mbid_artist, caa_id, caa_release_mbid
            )
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?)"
        )
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

        let plays = records.into_iter().map(|r| {
            // Parse mbid_artist JSON back to Vec<String>
            let mbid_artist = r.mbid_artist.and_then(|json_str| {
                serde_json::from_str::<Vec<String>>(&json_str).ok()
            });

            Play {
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
            }
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
