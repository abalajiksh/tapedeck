// musicbrainz.rs - MusicBrainz metadata with 3-tier caching
use crate::db::Db;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};

/// How long (seconds) to cache a "not found" result before retrying the API.
const NEGATIVE_CACHE_TTL_SECS: i64 = 7 * 24 * 3600; // 7 days

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
            user_agent: user_agent_for(None),
            rate_limit_per_second: 1, // MusicBrainz rate limit
            postgres_url: None,
            enable_postgres: false,
        }
    }
}

/// The contact string an unconfigured instance carries, and the one
/// [`contact_is_placeholder`] refuses.
pub const UNCONFIGURED_CONTACT: &str = "no contact configured";

/// Build the `User-Agent` MusicBrainz asks every client to send.
///
/// **Derived from `CARGO_PKG_*`, never written out**, because it had been
/// written out three times — `Tapedeck/1.0` here, `Tapedeck/0.5.0` in
/// `config.rs` and `Tapedeck/0.10.0` in `tapedeck.toml.example` — on an
/// application that was at 0.101.0. A version string a human has to remember
/// to bump is one that is always wrong, and this one is sent to a service that
/// judges us by it.
pub fn user_agent_for(contact: Option<&str>) -> String {
    let contact = contact.map(str::trim).filter(|c| !c.is_empty()).unwrap_or(UNCONFIGURED_CONTACT);
    format!("{}/{} ( {} )", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"), contact)
}

/// Does this `User-Agent` name somebody MusicBrainz could actually reach?
///
/// **This is what got the instance blocked** (2026-08-27). The shipped default
/// was `Tapedeck/1.0 ( your-email@example.com )` — a template nobody had
/// filled in — and MetaBrainz's policy is explicit that a client must identify
/// itself with real contact details and that ones which do not are blocked.
/// A placeholder is *worse* than a bare product name: it identifies an
/// unconfigured bot. Their edge then denied the address at the connection
/// level, which is why 48 hours of logs contain no HTTP status at all — the
/// server accepted each TCP connection and closed it without answering, on
/// port 80 as well as 443.
///
/// So this is the same rule the Every Noise scrape already follows — it
/// **refuses to start without an admin contact email**, on the grounds that a
/// made-up address is worse than none. That rule existed and this client did
/// not follow it.
///
/// Deliberately a check on the *contact*, not on the whole string: an operator
/// may set any product name they like, but `example.com` in it means nobody is
/// there.
/// Which service a URL belongs to, for the breaker's key.
///
/// Cover Art Archive is tracked **separately from MusicBrainz** even though
/// both are MetaBrainz: they are different hosts, and on 2026-08-27 exactly one
/// address was denied. Folding them together would have taken artwork down for
/// a fault that never touched it.
fn service_name(url: &str) -> &'static str {
    if url.contains("coverartarchive.org") {
        "Cover Art Archive"
    } else {
        "MusicBrainz"
    }
}

pub fn contact_is_placeholder(user_agent: &str) -> bool {
    let ua = user_agent.to_ascii_lowercase();

    // No contact at all. MusicBrainz wants a URL or an email; a bare product
    // name tells them nothing about who to ask to stop.
    let has_contact = ua.contains('@') || ua.contains("http://") || ua.contains("https://");
    if !has_contact {
        return true;
    }

    // The template values this project and the wider world ship.
    const TEMPLATES: &[&str] = &[
        "example.com",
        "example.org",
        "example.net",
        "your-email",
        "your_email",
        "youremail",
        "yourname",
        "user@host",
        "changeme",
        "todo@",
        "foo@bar",
    ];
    TEMPLATES.iter().any(|t| ua.contains(t))
}

pub struct MusicBrainzClient {
    config: MusicBrainzConfig,
    sqlite_pool: Pool<Db>,
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
    pub async fn new(config: MusicBrainzConfig, sqlite_pool: Pool<Db>) -> Result<Self> {
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
            // `timeout` is the whole request; `connect_timeout` bounds getting
            // off the ground. Without the second, a blackholed route holds a
            // lookup for the full thirty seconds — and these run inside the
            // engine tick, so that is thirty seconds of the deck being stale
            // for one unreachable host.
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .build()?,
            rate_limiter,
        })
    }

    /// The one place an outbound MetaBrainz request is made.
    ///
    /// Five call sites used to each do `rate_limiter.acquire()` then build
    /// their own `get(...).header("User-Agent", ...).send()`, which meant the
    /// politeness contract had to be remembered five times and the breaker
    /// would have had to be added five times. It is a chokepoint now, for the
    /// reason `credited_artists` is one: a rule that decides whether we may
    /// talk to somebody at all belongs where the talking happens, so the sixth
    /// call site inherits it instead of having to know.
    ///
    /// Three things happen here, in this order, and the order matters:
    ///
    /// 1. **Refuse to call at all with a placeholder contact.** Costs nothing
    ///    and is the only one that prevents the block rather than surviving it.
    /// 2. **Ask the breaker.** An unreachable service is learned about once,
    ///    not once per track — on 2026-08-27 a single query was re-sent 155
    ///    times in 2h18m, and 39 distinct tracks accounted for 912 lookups.
    /// 3. **Only then pay the rate limiter's second.** That sleep is paid per
    ///    lookup whether or not it succeeds, so sleeping before a call we were
    ///    always going to skip is the pure waste that made the backfill block
    ///    take four minutes.
    ///
    /// `Ok` is recorded on the breaker for any *answer*, including a 404 or a
    /// search that matched nothing: the service is plainly alive, and counting
    /// "MusicBrainz has never heard of this track" as an outage would open the
    /// breaker on a library it simply does not cover.
    async fn api_get(&self, url: &str) -> Result<reqwest::Response> {
        // A replica shares a public address with the instance it copied, and
        // MetaBrainz blocks by IP at the connection level with no status code
        // to say so — the 2026-08-27 outage. The cache still answers; the
        // network does not get asked twice for the same history.
        if crate::replica::is_replica() {
            return Err(anyhow!(
                "replica mode: not asking {} — the authoritative instance owns \
                 the enrichment budget for this address",
                service_name(url)
            ));
        }
        if contact_is_placeholder(&self.config.user_agent) {
            return Err(anyhow!(
                "refusing to call {} with a placeholder User-Agent ({}) — \
                 set MUSICBRAINZ_CONTACT to an email or URL they can reach",
                service_name(url),
                self.config.user_agent
            ));
        }

        let service = service_name(url);
        if let Some(left) = crate::sinks::health().skip_for(service) {
            return Err(anyhow!("{} is being skipped for another {}s", service, left.as_secs()));
        }

        self.rate_limiter.acquire().await;

        match self.http_client.get(url).header("User-Agent", &self.config.user_agent).send().await {
            Ok(resp) => {
                crate::sinks::health().record_ok(service);
                Ok(resp)
            }
            Err(e) => {
                // The cause chain, not just `{}`. Bare Display on a reqwest
                // error is `error sending request for url (...)` for a DNS
                // failure, a refused connection and a TLS handshake that was
                // cut off alike — and it was the third of those all along.
                // Two days of logs said nothing that could tell them apart.
                let detail = crate::logging::error_chain(&e);
                crate::sinks::health().record_failure(service, &detail);
                Err(anyhow!("{}", detail))
            }
        }
    }

    /// Main entry point - fetches metadata with 3-tier strategy
    /// Track/artist pairs this client has already asked about and found
    /// nothing for, while that answer is still current.
    ///
    /// **Lives here because the cache does.** `musicbrainz_cache` is in the
    /// client's own database — `SQLITE_DB_PATH`, default `./tapedeck.db` —
    /// which is a *different file* from the listens (`DATABASE_URL`,
    /// `scrobbles.db`). A query in `db.rs` joining `scrobbles` to this table
    /// cannot work and does not fail loudly: the table simply is not there.
    ///
    /// Returned as a set for the caller to fold against its own rows, which is
    /// the only shape a two-database question has.
    pub async fn negative_track_keys(&self) -> std::collections::HashSet<(String, String)> {
        let cutoff = chrono::Utc::now().timestamp() - NEGATIVE_CACHE_TTL_SECS;
        let rows: Vec<(String, String)> = sqlx::query_as(&crate::sql!(
            "SELECT LOWER(TRIM(track_title)), LOWER(TRIM(artist_name))
               FROM musicbrainz_cache
              WHERE track_mbid IS NULL AND fetched_at > ?",
        ))
        .bind(cutoff)
        .fetch_all(&self.sqlite_pool)
        .await
        .unwrap_or_default();
        rows.into_iter().collect()
    }

    pub async fn fetch_metadata(
        &self,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
    ) -> Result<MusicBrainzMetadata> {
        // Tier 1: Check SQLite cache (includes negative cache entries)
        if let Some(cached) = self.get_from_sqlite(track_title, artist_name, album_name).await? {
            if cached.track_mbid.is_none() {
                // Negative cache hit — return the error without hitting the API
                tracing::debug!(
                    "Skipping API call (negative cache): {} - {}",
                    artist_name,
                    track_title
                );
                return Err(anyhow!(
                    "No results found for {} - {} (cached)",
                    artist_name,
                    track_title
                ));
            }
            tracing::debug!("Cache hit (SQLite) for: {} - {}", artist_name, track_title);
            return Ok(cached);
        }

        // Tier 2: Check PostgreSQL dump (if enabled)
        if self.postgres_pool.is_some() {
            if let Some(pg_data) =
                self.get_from_postgres(track_title, artist_name, album_name).await?
            {
                tracing::debug!("Cache hit (PostgreSQL) for: {} - {}", artist_name, track_title);
                // Store in SQLite for faster future access
                self.store_in_sqlite(&pg_data, (track_title, artist_name, album_name)).await?;
                return Ok(pg_data);
            }
        }

        // Tier 3: Fetch from MusicBrainz API with rate limiting
        tracing::debug!("Fetching from MusicBrainz API: {} - {}", artist_name, track_title);
        let api_data = self.fetch_from_api(track_title, artist_name, album_name).await?;

        // Cache the result
        self.store_in_sqlite(&api_data, (track_title, artist_name, album_name)).await?;

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
        // The statement is chosen first and bound to a name, because a query
        // built inside an `if` borrows a translated `Cow` that dies with the
        // branch — see `crate::sql!`.
        let stmt = if album_name.is_some() {
            crate::sql!(
                r#"
                SELECT * FROM musicbrainz_cache
                WHERE LOWER(track_title) = LOWER($1)
                  AND LOWER(artist_name) = LOWER($2)
                  AND LOWER(album_name) = LOWER($3)
                ORDER BY fetched_at DESC
                LIMIT 1
                "#
            )
        } else {
            crate::sql!(
                r#"
                SELECT * FROM musicbrainz_cache
                WHERE LOWER(track_title) = LOWER($1)
                  AND LOWER(artist_name) = LOWER($2)
                  AND album_name IS NULL
                ORDER BY fetched_at DESC
                LIMIT 1
                "#
            )
        };
        let mut query =
            sqlx::query_as::<_, MusicBrainzMetadataRow>(&stmt).bind(track_title).bind(artist_name);
        if let Some(album) = album_name {
            query = query.bind(album);
        }

        match query.fetch_optional(&self.sqlite_pool).await? {
            Some(row) => {
                let metadata: MusicBrainzMetadata = row.into();
                // Check if this is a negative cache entry (no track MBID found)
                if metadata.track_mbid.is_none() {
                    let now = chrono::Utc::now().timestamp();
                    if now - metadata.fetched_at > NEGATIVE_CACHE_TTL_SECS {
                        // Stale negative entry — allow a retry
                        tracing::debug!(
                            "Negative cache expired for: {} - {}",
                            artist_name,
                            track_title
                        );
                        return Ok(None);
                    }
                    // Fresh negative entry — return it to skip the API call
                    tracing::debug!("Negative cache hit for: {} - {}", artist_name, track_title);
                }
                Ok(Some(metadata))
            }
            None => Ok(None),
        }
    }

    /// Cache a lookup **under the key it was asked for**.
    ///
    /// This used to key on `metadata.track_title/artist_name/album_name`, which
    /// `fetch_from_api` fills with MusicBrainz's *canonical* strings — so a
    /// successful lookup was filed under a name the caller had never used and
    /// could not produce again. Every call therefore missed its own cache and
    /// re-queried the API: submitting "Selected Ambient Works 85-92" cached it
    /// under "Selected Ambient Works 85–92" (en dash), and the next lookup for
    /// the same listen went back out to the network.
    ///
    /// Nothing reads the canonical names back out — consumers want the MBIDs,
    /// the release date and the genres — so keying on the query loses nothing
    /// and makes the cache actually cache.
    async fn store_in_sqlite(
        &self,
        metadata: &MusicBrainzMetadata,
        key: (&str, &str, Option<&str>),
    ) -> Result<()> {
        let genres_json = serde_json::to_string(&metadata.genres)?;

        sqlx::query(&crate::sql!(
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
            "#,
        ))
        .bind(&metadata.track_mbid)
        .bind(&metadata.artist_mbid)
        .bind(&metadata.album_mbid)
        .bind(key.0)
        .bind(key.1)
        .bind(key.2)
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
        // The Postgres dump query matches on recording + artist only; kept for
        // signature parity with the other tiers.
        _album_name: Option<&str>,
    ) -> Result<Option<MusicBrainzMetadata>> {
        let pool = match &self.postgres_pool {
            Some(p) => p,
            None => return Ok(None),
        };

        let result = sqlx::query_as::<_, PostgresMBRow>(&crate::sql!(
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
            "#,
        ))
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
        // Build search query
        let query = format!(r#"recording:"{}" AND artist:"{}""#, track_title, artist_name);

        let url = format!(
            "{}/recording?query={}&fmt=json&limit=1&inc=releases+artist-credits",
            self.config.api_base_url,
            urlencoding::encode(&query)
        );

        let response = self.api_get(&url).await?;

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
                artist_mbid: recording
                    .artist_credit
                    .first()
                    .and_then(|ac| ac.artist.as_ref())
                    .map(|a| a.id.clone()),
                album_mbid,
                track_title: recording.title.clone(),
                artist_name: recording
                    .artist_credit
                    .first()
                    .map(|ac| ac.name.clone())
                    .unwrap_or_else(|| artist_name.to_string()),
                album_name: recording.releases.first().map(|r| r.title.clone()),
                release_date: recording.releases.first().and_then(|r| r.date.clone()),
                genres: recording.tags.iter().map(|t| t.name.clone()).collect(),
                caa_id,
                caa_release_mbid,
                fetched_at: chrono::Utc::now().timestamp(),
            })
        } else {
            // Store a negative-cache sentinel so we don't re-query this every cycle
            let negative = MusicBrainzMetadata {
                track_mbid: None,
                artist_mbid: None,
                album_mbid: None,
                track_title: track_title.to_string(),
                artist_name: artist_name.to_string(),
                album_name: _album_name.map(|s| s.to_string()),
                release_date: None,
                genres: Vec::new(),
                caa_id: None,
                caa_release_mbid: None,
                fetched_at: chrono::Utc::now().timestamp(),
            };
            // Best-effort cache — don't fail the whole call if this errors
            if let Err(e) =
                self.store_in_sqlite(&negative, (track_title, artist_name, _album_name)).await
            {
                tracing::warn!("Failed to store negative cache entry: {}", e);
            }
            Err(anyhow!("No results found for {} - {}", artist_name, track_title))
        }
    }

    /// Fetch Cover Art Archive info for a release
    async fn fetch_cover_art_info(
        &self,
        release_mbid: &str,
    ) -> Result<(Option<i64>, Option<String>)> {
        let url = format!("https://coverartarchive.org/release/{}", release_mbid);

        let response = self.api_get(&url).await?;

        if !response.status().is_success() {
            return Ok((None, None));
        }

        let caa_response: CAAResponse = response.json().await?;

        let front_image = caa_response
            .images
            .iter()
            .find(|img| img.front)
            .or_else(|| caa_response.images.first());

        if let Some(image) = front_image {
            let release_mbid_clean = Self::extract_mbid_from_url(&caa_response.release);
            Ok((Some(image.id), Some(release_mbid_clean)))
        } else {
            Ok((None, None))
        }
    }

    fn extract_mbid_from_url(url_or_mbid: &str) -> String {
        if url_or_mbid.starts_with("http://") || url_or_mbid.starts_with("https://") {
            url_or_mbid.rsplit('/').next().unwrap_or(url_or_mbid).to_string()
        } else {
            url_or_mbid.to_string()
        }
    }

    pub async fn initialize_schema(&self) -> Result<()> {
        crate::db::run_ddl(
            &self.sqlite_pool,
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

            -- ISRCs are keyed by recording and come from the **lookup**
            -- endpoint, not the search: `inc=` is silently ignored on
            -- `/recording?query=`, so asking there returns nothing and a
            -- backfill built on it would tombstone every track as "no ISRC"
            -- without ever really asking. Same shape as the language cache
            -- below, and for the same reason.
            CREATE TABLE IF NOT EXISTS mb_isrc_cache (
                recording_mbid TEXT PRIMARY KEY,
                isrcs TEXT,             -- JSON array; '[]' = looked up, none
                fetched_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS mb_language_cache (
                recording_mbid TEXT PRIMARY KEY,
                language TEXT,          -- NULL/'' = looked up, none found
                fetched_at INTEGER NOT NULL
            );
            "#,
        )
        .await?;

        Ok(())
    }

    /// Resolve the language of a recording via its linked MusicBrainz *work*.
    /// Postgres dump first (instant, local), then the API (rate-limited), cached
    /// by recording MBID. Returns `Ok(None)` when no work-language is available;
    /// the negative result is cached too so we don't re-query.
    /// An artist's genre tags from MusicBrainz.
    ///
    /// Recording-level tags are the more precise answer and are tried first by
    /// the caller, but they are **sparse and unstable**: the recording search
    /// takes `limit=1` against a ranking that is not deterministic, so for a
    /// track with many recordings (originals, remasters, live versions) which
    /// one comes back — and therefore whether it carries tags at all — varies
    /// between identical calls. Measured: "Xtal" by Aphex Twin returned seven
    /// tags on one run and none on the next.
    ///
    /// Artist tags do not have that problem. There is one artist, the search
    /// finds it, and MusicBrainz's taggers work at this level far more than at
    /// the recording level — every artist checked came back with six to eight.
    /// A genre is more an artist's property than one recording's anyway.
    ///
    /// One call per *artist*, and the caller caches it, so this costs a handful
    /// of lookups for a whole library rather than one per listen.
    pub async fn fetch_artist_tags(&self, artist_name: &str) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            artists: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            #[serde(default)]
            tags: Vec<Tag>,
        }
        #[derive(Deserialize)]
        struct Tag {
            name: String,
            #[serde(default)]
            count: i64,
        }

        let query = format!(r#"artist:"{}""#, artist_name.replace('"', ""));
        let url = format!(
            "{}/artist?query={}&fmt=json&limit=1",
            self.config.api_base_url,
            urlencoding::encode(&query)
        );
        let resp = self.api_get(&url).await?;
        if !resp.status().is_success() {
            return Err(anyhow!("MusicBrainz artist search returned {}", resp.status()));
        }
        let body: Resp = resp.json().await?;
        let Some(entry) = body.artists.first() else {
            return Ok(Vec::new());
        };

        // Tags are folksonomy, so a single downvoted tag is noise. Anything with
        // a non-positive count is dropped, and the rest are ordered by how many
        // people agreed.
        let mut tags: Vec<(i64, String)> = entry
            .tags
            .iter()
            .filter(|t| t.count > 0)
            .map(|t| (t.count, t.name.trim().to_lowercase()))
            .filter(|(_, n)| !n.is_empty())
            .collect();
        tags.sort_by(|a, b| b.0.cmp(&a.0));
        tags.dedup_by(|a, b| a.1 == b.1);
        Ok(tags.into_iter().take(6).map(|(_, n)| n).collect())
    }

    /// Every ISRC MusicBrainz holds for a recording.
    ///
    /// **The lookup endpoint, not the search.** `inc=` is silently ignored on
    /// `/recording?query=`: the response simply has no `isrcs` key and parses
    /// perfectly, so a backfill built on the search writes "this recording has
    /// no ISRC" for everything without ever having asked. That was caught by
    /// checking a track that demonstrably has one, and it is the same shape as
    /// every other silent-field bug in this codebase — nothing errors, the
    /// answer is just always empty.
    ///
    /// Keyed and cached by recording MBID, so a track played fifty times costs
    /// one request. `[]` is stored when there genuinely is none, which is a
    /// common and legitimate answer — the tombstone that stops it being asked
    /// forever against a rate-limited service.
    pub async fn fetch_recording_isrcs(&self, recording_mbid: &str) -> Result<Vec<String>> {
        let cached: Option<(Option<String>,)> = sqlx::query_as(&crate::sql!(
            "SELECT isrcs FROM mb_isrc_cache WHERE recording_mbid = ?",
        ))
        .bind(recording_mbid)
        .fetch_optional(&self.sqlite_pool)
        .await
        .unwrap_or(None);
        if let Some((raw,)) = cached {
            return Ok(raw
                .as_deref()
                .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
                .unwrap_or_default());
        }

        let url = format!(
            "{}/recording/{}?inc=isrcs&fmt=json",
            self.config.api_base_url.trim_end_matches('/'),
            recording_mbid
        );
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            isrcs: Vec<String>,
        }
        let resp = self.api_get(&url).await?;
        if !resp.status().is_success() {
            return Err(anyhow!("MusicBrainz API error: {}", resp.status()));
        }
        let found: Resp = resp.json().await?;
        // Normalised here so nothing downstream has to: an ISRC is defined
        // uppercase and clients send it either way.
        let isrcs: Vec<String> =
            found.isrcs.iter().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect();

        let _ = sqlx::query(&crate::sql!(
            "INSERT INTO mb_isrc_cache (recording_mbid, isrcs, fetched_at)
             VALUES (?, ?, ?)
             ON CONFLICT (recording_mbid) DO UPDATE SET
                isrcs = excluded.isrcs, fetched_at = excluded.fetched_at",
        ))
        .bind(recording_mbid)
        .bind(serde_json::to_string(&isrcs).unwrap_or_else(|_| "[]".into()))
        .bind(chrono::Utc::now().timestamp())
        .execute(&self.sqlite_pool)
        .await;

        Ok(isrcs)
    }

    pub async fn fetch_recording_language(&self, recording_mbid: &str) -> Result<Option<String>> {
        let cached: Option<(Option<String>,)> = sqlx::query_as(&crate::sql!(
            "SELECT language FROM mb_language_cache WHERE recording_mbid = ?",
        ))
        .bind(recording_mbid)
        .fetch_optional(&self.sqlite_pool)
        .await
        .unwrap_or(None);
        if let Some((lang,)) = cached {
            return Ok(lang.filter(|s| !s.is_empty()));
        }

        let mut resolved: Option<String> = None;

        // Tier 1: Postgres MB dump — l_recording_work → work_language → language.
        if let Some(pool) = &self.postgres_pool {
            let row: Option<(String,)> = sqlx::query_as(&crate::sql!(
                r#"
                SELECT lang.name
                FROM recording r
                JOIN l_recording_work lrw ON lrw.entity0 = r.id
                JOIN work w ON w.id = lrw.entity1
                JOIN work_language wl ON wl.work = w.id
                JOIN language lang ON lang.id = wl.language
                WHERE r.gid = $1::uuid
                LIMIT 1
                "#,
            ))
            .bind(recording_mbid)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
            if let Some((name,)) = row {
                resolved = Some(name); // Postgres `language.name` is already a display name
            }
        }

        // Tier 2: API — recording?inc=work-rels → work.languages[0] (ISO 639-3).
        if resolved.is_none() {
            let url = format!(
                "{}/recording/{}?inc=work-rels&fmt=json",
                self.config.api_base_url, recording_mbid
            );
            if let Ok(resp) = self.api_get(&url).await {
                if resp.status().is_success() {
                    if let Ok(v) = resp.json::<serde_json::Value>().await {
                        resolved = v["relations"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|rel| rel["target-type"] == "work")
                            .find_map(|rel| {
                                let w = &rel["work"];
                                w["languages"]
                                    .as_array()
                                    .and_then(|ls| ls.first())
                                    .and_then(|l| l.as_str())
                                    .or_else(|| w["language"].as_str())
                                    .map(crate::language::iso_to_display)
                            });
                    }
                }
            }
        }

        let _ = sqlx::query(&crate::sql!(
            "INSERT INTO mb_language_cache (recording_mbid, language, fetched_at) VALUES (?, ?, ?)
             ON CONFLICT (recording_mbid) DO UPDATE SET
                language = excluded.language, fetched_at = excluded.fetched_at",
        ))
        .bind(recording_mbid)
        .bind(resolved.clone().unwrap_or_default())
        .bind(chrono::Utc::now().timestamp())
        .execute(&self.sqlite_pool)
        .await;

        Ok(resolved)
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
    #[serde(default)]
    releases: Vec<MBRelease>,
    #[serde(default)]
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
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MBTag {
    name: String,
    #[serde(default)]
    count: Option<i32>,
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

#[cfg(all(test, not(feature = "postgres")))]
mod identity_tests {
    use super::*;

    /// **The bug that got this instance blocked**, pinned so it cannot ship
    /// again. `Tapedeck/1.0 ( your-email@example.com )` was the shipped default
    /// for the entire life of the project, and MetaBrainz denied the address at
    /// the connection level on 2026-08-27 — no HTTP status, on port 80 as well
    /// as 443, while every other host on the same machine was reachable.
    #[test]
    fn the_shipped_placeholders_are_all_refused() {
        for ua in [
            "Tapedeck/1.0 ( your-email@example.com )",
            "Tapedeck/0.5.0 ( contact@example.com )",
            "Tapedeck/0.10.0 ( your-email@example.com )",
            "Tapedeck/0.101.0 ( https://example.org/tapedeck )",
            "Tapedeck/0.101.0 ( changeme@somewhere.net )",
        ] {
            assert!(contact_is_placeholder(ua), "should have been refused: {ua}");
        }
    }

    /// A client that names no way to reach anybody is refused too. This is the
    /// half a check on `example.com` alone would miss, and MusicBrainz's policy
    /// asks for contact details rather than merely for a product name.
    #[test]
    fn a_user_agent_with_no_contact_at_all_is_refused() {
        assert!(contact_is_placeholder("Tapedeck/0.101.0"));
        assert!(contact_is_placeholder("Tapedeck/0.101.0 ( )"));
        assert!(contact_is_placeholder(&user_agent_for(None)));
        assert!(contact_is_placeholder(&user_agent_for(Some("   "))));
    }

    /// And a real one passes, by either of the two forms MusicBrainz accepts.
    /// Without this the check could be "return true" and every test above would
    /// still pass — which is the shape that silently disables the API forever.
    #[test]
    fn a_real_contact_is_accepted() {
        assert!(!contact_is_placeholder(&user_agent_for(Some("someone@example-host.test"))));
        assert!(!contact_is_placeholder("Tapedeck/0.101.0 ( https://codeberg.org/x/tapedeck )"));
    }

    /// The version is read from the crate, not typed out. It had been typed out
    /// three times, in three different and equally wrong values, on an
    /// application at 0.101.0 — and it is the string a rate limiter judges.
    #[test]
    fn the_user_agent_carries_the_version_the_binary_actually_is() {
        let ua = user_agent_for(Some("someone@example-host.test"));
        assert!(ua.contains(env!("CARGO_PKG_VERSION")), "{ua}");
        assert!(ua.starts_with(env!("CARGO_PKG_NAME")), "{ua}");
    }

    /// Cover Art Archive is a different host from MusicBrainz and gets its own
    /// breaker. On 2026-08-27 exactly one address was denied; folding the two
    /// together would have taken artwork down for a fault it never had.
    #[test]
    fn cover_art_is_tracked_apart_from_musicbrainz() {
        assert_eq!(service_name("https://coverartarchive.org/release/abc"), "Cover Art Archive");
        assert_eq!(service_name("https://musicbrainz.org/ws/2/recording?query=x"), "MusicBrainz");
    }
}

#[cfg(all(test, not(feature = "postgres")))]
mod refusal_tests {
    use super::*;

    async fn client(user_agent: &str, api_base_url: &str) -> MusicBrainzClient {
        let pool: Pool<Db> = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let c = MusicBrainzClient::new(
            MusicBrainzConfig {
                api_base_url: api_base_url.to_string(),
                user_agent: user_agent.to_string(),
                rate_limit_per_second: 1000, // no pacing; this test makes no calls
                postgres_url: None,
                enable_postgres: false,
            },
            pool,
        )
        .await
        .unwrap();
        c.initialize_schema().await.unwrap();
        c
    }

    /// **The refusal happens before the request, not instead of reading the
    /// reply.** Proven by pointing the client at an address nothing can be
    /// reached at: if a call were made the error would be a transport failure,
    /// and it is only because none is made that the message is about the
    /// User-Agent. That distinction is the whole feature — an instance with no
    /// contact configured must generate *no traffic at all*, since traffic is
    /// what got the last one blocked.
    #[tokio::test]
    async fn a_placeholder_contact_makes_no_request_at_all() {
        // Nothing listens on port 1, so a request that *is* made fails
        // immediately and loudly rather than making this test wait out a
        // timeout — the failure has to be legible, not merely present.
        let c = client("Tapedeck/1.0 ( your-email@example.com )", "http://127.0.0.1:1/ws/2").await;

        let err = c.fetch_metadata("Xtal", "Aphex Twin", None).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("placeholder User-Agent"), "should name the real reason: {msg}");
        assert!(
            !msg.contains("error sending request"),
            "a request was attempted, which is exactly what must not happen: {msg}"
        );
    }

    /// And a configured instance is *not* refused — it gets as far as trying,
    /// which against an unroutable address is a transport error. Without this
    /// the guard could refuse everything and the test above would still pass,
    /// which is the shape that silently disables enrichment forever.
    #[tokio::test]
    async fn a_configured_contact_is_allowed_through_to_the_network() {
        let c =
            client(&user_agent_for(Some("someone@example-host.test")), "http://127.0.0.1:1/ws/2")
                .await;

        let err = c.fetch_metadata("Xtal", "Aphex Twin", None).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(!msg.contains("placeholder"), "should not have been refused locally: {msg}");
    }
}
