// musicbrainz.rs - MusicBrainz metadata with 3-tier caching
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Sqlite, Postgres};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicBrainzMetadata {
    pub track_mbid: Option<String>,
    pub artist_mbid: Option<String>,
    pub album_mbid: Option<String>,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub release_date: Option<String>,
    pub genres: Vec<String>,
    pub caa_id: Option<i64>,
    pub caa_release_mbid: Option<String>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone)]
pub struct MusicBrainzConfig {
    pub api_base_url: String,
    pub user_agent: String,
    pub rate_limit_per_second: u32,
    pub postgres_url: Option<String>,
    pub enable_postgres: bool,
}

impl Default for MusicBrainzConfig {
    fn default() -> Self {
        Self {
            api_base_url: "https://musicbrainz.org/ws/2".to_string(),
            user_agent: "Tapedeck/1.0 ( your-email@example.com )".to_string(),
            rate_limit_per_second: 1, // MusicBrainz rate limit
            postgres_url: None,
            enable_postgres: false,
        }
    }
}

pub struct MusicBrainzClient {
    config: MusicBrainzConfig,
    sqlite_pool: Pool<Sqlite>,
    postgres_pool: Option<Pool<Postgres>>,
    http_client: reqwest::Client,
    rate_limiter: Arc<RateLimiter>,
}

struct RateLimiter {
    semaphore: Semaphore,
    last_request: RwLock<Instant>,
    min_interval: Duration,
}

impl RateLimiter {
    fn new(requests_per_second: u32) -> Self {
        Self {
            semaphore: Semaphore::new(1),
            last_request: RwLock::new(Instant::now() - Duration::from_secs(10)),
            min_interval: Duration::from_millis(1000 / requests_per_second as u64),
        }
    }

    async fn acquire(&self) {
        let _permit = self.semaphore.acquire().await.unwrap();

        let mut last = self.last_request.write().await;
        let elapsed = last.elapsed();

        if elapsed < self.min_interval {
            let sleep_duration = self.min_interval - elapsed;
            tokio::time::sleep(sleep_duration).await;
        }

        *last = Instant::now();
    }
}

impl MusicBrainzClient {
    pub async fn new(config: MusicBrainzConfig, sqlite_pool: Pool<Sqlite>) -> Result<Self> {
        let postgres_pool = if config.enable_postgres {
            if let Some(ref pg_url) = config.postgres_url {
                Some(Pool::<Postgres>::connect(pg_url).await?)
            } else {
                None
            }
        } else {
            None
        };

        let rate_limiter = Arc::new(RateLimiter::new(config.rate_limit_per_second));

        Ok(Self {
            config,
            sqlite_pool,
            postgres_pool,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            rate_limiter,
        })
    }

    /// Main entry point - fetches metadata with 3-tier strategy
    pub async fn fetch_metadata(
        &self,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
    ) -> Result<MusicBrainzMetadata> {
        // Tier 1: Check SQLite cache
        if let Some(cached) = self.get_from_sqlite(track_title, artist_name, album_name).await? {
            tracing::debug!("Cache hit (SQLite) for: {} - {}", artist_name, track_title);
            return Ok(cached);
        }

        // Tier 2: Check PostgreSQL dump (if enabled)
        if self.postgres_pool.is_some() {
            if let Some(pg_data) = self.get_from_postgres(track_title, artist_name, album_name).await? {
                tracing::debug!("Cache hit (PostgreSQL) for: {} - {}", artist_name, track_title);
                // Store in SQLite for faster future access
                self.store_in_sqlite(&pg_data).await?;
                return Ok(pg_data);
            }
        }

        // Tier 3: Fetch from MusicBrainz API with rate limiting
        tracing::debug!("Fetching from MusicBrainz API: {} - {}", artist_name, track_title);
        let api_data = self.fetch_from_api(track_title, artist_name, album_name).await?;

        // Cache the result
        self.store_in_sqlite(&api_data).await?;

        Ok(api_data)
    }

    /// Batch fetch for multiple tracks with rate limiting
    pub async fn fetch_metadata_batch(
        &self,
        tracks: Vec<(&str, &str, Option<&str>)>,
    ) -> Vec<Result<MusicBrainzMetadata>> {
        let mut results = Vec::new();

        for (track_title, artist_name, album_name) in tracks {
            let result = self.fetch_metadata(track_title, artist_name, album_name).await;
            results.push(result);
        }

        results
    }

    // ========== TIER 1: SQLite Cache ==========

    async fn get_from_sqlite(
        &self,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
    ) -> Result<Option<MusicBrainzMetadata>> {
        let query = if let Some(album) = album_name {
            sqlx::query_as::<_, MusicBrainzMetadataRow>(
                r#"
                SELECT * FROM musicbrainz_cache
                WHERE LOWER(track_title) = LOWER($1)
                  AND LOWER(artist_name) = LOWER($2)
                  AND LOWER(album_name) = LOWER($3)
                ORDER BY fetched_at DESC
                LIMIT 1
                "#
            )
                .bind(track_title)
                .bind(artist_name)
                .bind(album)
        } else {
            sqlx::query_as::<_, MusicBrainzMetadataRow>(
                r#"
                SELECT * FROM musicbrainz_cache
                WHERE LOWER(track_title) = LOWER($1)
                  AND LOWER(artist_name) = LOWER($2)
                  AND album_name IS NULL
                ORDER BY fetched_at DESC
                LIMIT 1
                "#
            )
                .bind(track_title)
                .bind(artist_name)
        };

        match query.fetch_optional(&self.sqlite_pool).await? {
            Some(row) => Ok(Some(row.into())),
            None => Ok(None),
        }
    }

    async fn store_in_sqlite(&self, metadata: &MusicBrainzMetadata) -> Result<()> {
        let genres_json = serde_json::to_string(&metadata.genres)?;

        sqlx::query(
            r#"
            INSERT INTO musicbrainz_cache (
                track_mbid, artist_mbid, album_mbid,
                track_title, artist_name, album_name,
                release_date, genres, caa_id, caa_release_mbid, fetched_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT(track_title, artist_name, album_name)
            DO UPDATE SET
                track_mbid = EXCLUDED.track_mbid,
                artist_mbid = EXCLUDED.artist_mbid,
                album_mbid = EXCLUDED.album_mbid,
                release_date = EXCLUDED.release_date,
                genres = EXCLUDED.genres,
                caa_id = EXCLUDED.caa_id,
                caa_release_mbid = EXCLUDED.caa_release_mbid,
                fetched_at = EXCLUDED.fetched_at
            "#
        )
            .bind(&metadata.track_mbid)
            .bind(&metadata.artist_mbid)
            .bind(&metadata.album_mbid)
            .bind(&metadata.track_title)
            .bind(&metadata.artist_name)
            .bind(&metadata.album_name)
            .bind(&metadata.release_date)
            .bind(genres_json)
            .bind(metadata.caa_id)
            .bind(&metadata.caa_release_mbid)
            .bind(metadata.fetched_at)
            .execute(&self.sqlite_pool)
            .await?;

        Ok(())
    }

    // ========== TIER 2: PostgreSQL Dump ==========

    async fn get_from_postgres(
        &self,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
    ) -> Result<Option<MusicBrainzMetadata>> {
        let pool = match &self.postgres_pool {
            Some(p) => p,
            None => return Ok(None),
        };

        // Query the MusicBrainz PostgreSQL dump
        // Adjust table/column names based on your actual MB schema
        let result = sqlx::query_as::<_, PostgresMBRow>(
            r#"
            SELECT
                r.gid as recording_gid,
                r.name as recording_name,
                ar.gid as artist_gid,
                ar.name as artist_name,
                rel.gid as release_gid,
                rel.name as release_name
            FROM recording r
            JOIN artist_credit ac ON r.artist_credit = ac.id
            JOIN artist_credit_name acn ON ac.id = acn.artist_credit
            JOIN artist ar ON acn.artist = ar.id
            LEFT JOIN track t ON t.recording = r.id
            LEFT JOIN medium m ON t.medium = m.id
            LEFT JOIN release rel ON m.release = rel.id
            WHERE LOWER(r.name) = LOWER($1)
              AND LOWER(ar.name) = LOWER($2)
            LIMIT 1
            "#
        )
            .bind(track_title)
            .bind(artist_name)
            .fetch_optional(pool)
            .await?;

        match result {
            Some(row) => Ok(Some(MusicBrainzMetadata {
                track_mbid: Some(row.recording_gid),
                artist_mbid: Some(row.artist_gid),
                album_mbid: row.release_gid,
                track_title: row.recording_name,
                artist_name: row.artist_name,
                album_name: row.release_name,
                release_date: None,
                genres: Vec::new(),
                caa_id: None,
                caa_release_mbid: None,
                fetched_at: chrono::Utc::now().timestamp(),
            })),
            None => Ok(None),
        }
    }

    // ========== TIER 3: MusicBrainz API ==========

    async fn fetch_from_api(
        &self,
        track_title: &str,
        artist_name: &str,
        _album_name: Option<&str>,
    ) -> Result<MusicBrainzMetadata> {
        // Rate limiting
        self.rate_limiter.acquire().await;

        // Build search query
        let query = format!(r#"recording:"{}" AND artist:"{}""#, track_title, artist_name);

        let url = format!(
            "{}/recording?query={}&fmt=json&limit=1&inc=releases+artist-credits",
            self.config.api_base_url,
            urlencoding::encode(&query)
        );

        let response = self.http_client
            .get(&url)
            .header("User-Agent", &self.config.user_agent)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("MusicBrainz API error: {}", response.status()));
        }

        let api_response: MBApiResponse = response.json().await?;

        if let Some(recording) = api_response.recordings.first() {
            let album_mbid = recording.releases.first().map(|r| r.id.clone());
            
            // Fetch cover art info if we have a release MBID
            let (caa_id, caa_release_mbid) = if let Some(ref release_mbid) = album_mbid {
                self.fetch_cover_art_info(release_mbid).await.unwrap_or((None, None))
            } else {
                (None, None)
            };

            Ok(MusicBrainzMetadata {
                track_mbid: Some(recording.id.clone()),
                artist_mbid: recording.artist_credit.first()
                    .and_then(|ac| ac.artist.as_ref())
                    .map(|a| a.id.clone()),
                album_mbid,
                track_title: recording.title.clone(),
                artist_name: recording.artist_credit.first()
                    .map(|ac| ac.name.clone())
                    .unwrap_or_else(|| artist_name.to_string()),
                album_name: recording.releases.first().map(|r| r.title.clone()),
                release_date: recording.releases.first()
                    .and_then(|r| r.date.clone()),
                genres: recording.tags.iter().map(|t| t.name.clone()).collect(),
                caa_id,
                caa_release_mbid,
                fetched_at: chrono::Utc::now().timestamp(),
            })
        } else {
            Err(anyhow!("No results found for {} - {}", artist_name, track_title))
        }
    }

    /// Fetch Cover Art Archive info for a release
    /// Returns (caa_id, release_mbid) - extracts just the MBID from the URL
    async fn fetch_cover_art_info(&self, release_mbid: &str) -> Result<(Option<i64>, Option<String>)> {
        // Rate limiting
        self.rate_limiter.acquire().await;

        let url = format!(
            "https://coverartarchive.org/release/{}",
            release_mbid
        );

        let response = self.http_client
            .get(&url)
            .header("User-Agent", &self.config.user_agent)
            .send()
            .await?;

        if !response.status().is_success() {
            // No cover art available
            return Ok((None, None));
        }

        let caa_response: CAAResponse = response.json().await?;

        // Get the front cover or first image
        let front_image = caa_response.images.iter()
            .find(|img| img.front)
            .or_else(|| caa_response.images.first());

        if let Some(image) = front_image {
            // Extract MBID from URL if needed
            // CAA returns "https://musicbrainz.org/release/{mbid}" but we only want the MBID
            let release_mbid_clean = Self::extract_mbid_from_url(&caa_response.release);
            Ok((Some(image.id), Some(release_mbid_clean)))
        } else {
            Ok((None, None))}
    }

    /// Extract MBID from a MusicBrainz URL or return as-is if already just an MBID
    fn extract_mbid_from_url(url_or_mbid: &str) -> String {
        // If it's a URL like "https://musicbrainz.org/release/{mbid}", extract the MBID
        if url_or_mbid.starts_with("http://") || url_or_mbid.starts_with("https://") {
            url_or_mbid
                .split('/')
                .last()
                .unwrap_or(url_or_mbid)
                .to_string()
        } else {
            // Already just an MBID
            url_or_mbid.to_string()
        }
    }

    pub async fn initialize_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS musicbrainz_cache (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                track_mbid TEXT,
                artist_mbid TEXT,
                album_mbid TEXT,
                track_title TEXT NOT NULL,
                artist_name TEXT NOT NULL,
                album_name TEXT,
                release_date TEXT,
                genres TEXT,
                caa_id INTEGER,
                caa_release_mbid TEXT,
                fetched_at INTEGER NOT NULL,
                UNIQUE(track_title, artist_name, album_name)
            );

            CREATE INDEX IF NOT EXISTS idx_mb_cache_lookup
            ON musicbrainz_cache(track_title, artist_name, album_name);

            CREATE INDEX IF NOT EXISTS idx_mb_cache_mbid
            ON musicbrainz_cache(track_mbid, artist_mbid);
            "#
        )
            .execute(&self.sqlite_pool)
            .await?;

        Ok(())
    }
}

// ========== Supporting Types ==========

#[derive(sqlx::FromRow)]
struct MusicBrainzMetadataRow {
    track_mbid: Option<String>,
    artist_mbid: Option<String>,
    album_mbid: Option<String>,
    track_title: String,
    artist_name: String,
    album_name: Option<String>,
    release_date: Option<String>,
    genres: String,
    caa_id: Option<i64>,
    caa_release_mbid: Option<String>,
    fetched_at: i64,
}

impl From<MusicBrainzMetadataRow> for MusicBrainzMetadata {
    fn from(row: MusicBrainzMetadataRow) -> Self {
        let genres: Vec<String> = serde_json::from_str(&row.genres).unwrap_or_default();
        Self {
            track_mbid: row.track_mbid,
            artist_mbid: row.artist_mbid,
            album_mbid: row.album_mbid,
            track_title: row.track_title,
            artist_name: row.artist_name,
            album_name: row.album_name,
            release_date: row.release_date,
            genres,
            caa_id: row.caa_id,
            caa_release_mbid: row.caa_release_mbid,
            fetched_at: row.fetched_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct PostgresMBRow {
    recording_gid: String,
    recording_name: String,
    artist_gid: String,
    artist_name: String,
    release_gid: Option<String>,
    release_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MBApiResponse {
    recordings: Vec<MBRecording>,
}

#[derive(Debug, Deserialize)]
struct MBRecording {
    id: String,
    title: String,
    #[serde(rename = "artist-credit")]
    artist_credit: Vec<MBArtistCredit>,
    releases: Vec<MBRelease>,
    tags: Vec<MBTag>,
}

#[derive(Debug, Deserialize)]
struct MBArtistCredit {
    name: String,
    artist: Option<MBArtist>,
}

#[derive(Debug, Deserialize)]
struct MBArtist {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct MBRelease {
    id: String,
    title: String,
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MBTag {
    name: String,
}

#[derive(Debug, Deserialize)]
struct CAAResponse {
    release: String,
    images: Vec<CAAImage>,
}

#[derive(Debug, Deserialize)]
struct CAAImage {
    id: i64,
    front: bool,
}
