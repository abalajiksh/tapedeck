//! Storage for the Every Noise genre map, in a database of its own.
//!
//! Kept out of the main scrobble database deliberately, following the same
//! reasoning as the MusicBrainz cache: this is bulk reference data acquired
//! from somewhere else, it is rebuildable at any time by scraping again, and it
//! is roughly the same size as a small listening history. Mixing it in would
//! bloat every `VACUUM INTO` backup with data that is not the user's.
//!
//! It is also **not** derived from anything the user did, which is why nothing
//! here is scoped by `user_id`.

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Pool, Sqlite};
use std::str::FromStr;
use std::time::Duration;

use super::parse::{Artist, Genre, Neighbour};

#[derive(Clone)]
pub struct EveryNoiseStore {
    pool: Pool<Sqlite>,
}

/// What the local copy currently holds.
#[derive(Debug, Default, serde::Serialize)]
pub struct Summary {
    pub genres: i64,
    pub artists: i64,
    pub edges: i64,
    /// Genre pages walked, out of `genres` — the tier 2 progress marker that
    /// survives a restart.
    pub pages_scraped: i64,
    /// Unix seconds of the most recent write, or `None` when empty.
    pub last_scraped_at: Option<i64>,
}

impl EveryNoiseStore {
    pub async fn open(path: &str) -> Result<Self, sqlx::Error> {
        let options = SqliteConnectOptions::from_str(path)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(10))
            .synchronous(SqliteSynchronous::Normal);
        // Two connections is plenty: one scrape job writes, handlers read.
        let pool = SqlitePoolOptions::new().max_connections(2).connect_with(options).await?;
        let store = Self { pool };
        store.init().await?;
        Ok(store)
    }

    async fn init(&self) -> Result<(), sqlx::Error> {
        // `name_key` is the join column, not `name`: genre names are compared
        // case-insensitively against MusicBrainz tags, and doing that with
        // LOWER() at query time would give up the index.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS genres (
                slug TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                name_key TEXT NOT NULL,
                x REAL NOT NULL,
                y REAL NOT NULL,
                color TEXT NOT NULL,
                font_size REAL NOT NULL,
                preview_track_id TEXT,
                example_title TEXT,
                scraped_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_en_genres_key ON genres(name_key)")
            .execute(&self.pool)
            .await?;

        // `norm_name` is `db::normalise_artist` — the *one* artist identity
        // function in this codebase. A second one here would be the third, and
        // CLAUDE.md is explicit that the answer to needing a different notion
        // of artist identity is a conversation, not another normaliser.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS genre_artists (
                genre_slug TEXT NOT NULL,
                name TEXT NOT NULL,
                norm_name TEXT NOT NULL,
                spotify_id TEXT,
                x REAL NOT NULL,
                y REAL NOT NULL,
                color TEXT NOT NULL,
                font_size REAL NOT NULL,
                PRIMARY KEY (genre_slug, name)
            )",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_en_artists_norm ON genre_artists(norm_name)")
            .execute(&self.pool)
            .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS genre_edges (
                genre_slug TEXT NOT NULL,
                related_slug TEXT NOT NULL,
                relation TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (genre_slug, related_slug, relation)
            )",
        )
        .execute(&self.pool)
        .await?;

        // The resume marker. A tier 2 walk is ~6,300 requests over about two
        // hours; losing it to a restart would mean starting over, and starting
        // over means asking a frozen one-person site for everything twice.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS scraped_pages (
                slug TEXT PRIMARY KEY,
                scraped_at INTEGER NOT NULL,
                artists INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Replace the genre map. Tier 1 is one page, so it is written as one
    /// transaction — a partial map is worse than the previous one.
    pub async fn put_genres(&self, genres: &[Genre]) -> Result<u64, sqlx::Error> {
        let now = Self::now();
        let mut tx = self.pool.begin().await?;
        for g in genres {
            sqlx::query(
                "INSERT INTO genres (slug, name, name_key, x, y, color, font_size,
                                     preview_track_id, example_title, scraped_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(slug) DO UPDATE SET
                    name = excluded.name, name_key = excluded.name_key,
                    x = excluded.x, y = excluded.y, color = excluded.color,
                    font_size = excluded.font_size,
                    preview_track_id = excluded.preview_track_id,
                    example_title = excluded.example_title,
                    scraped_at = excluded.scraped_at",
            )
            .bind(&g.slug)
            .bind(&g.name)
            .bind(g.name.trim().to_lowercase())
            .bind(g.x)
            .bind(g.y)
            .bind(&g.color)
            .bind(g.font_size)
            .bind(&g.preview_track_id)
            .bind(&g.example_title)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(genres.len() as u64)
    }

    /// Store one genre's artists and neighbour edges, and mark the page done.
    ///
    /// All in one transaction with the `scraped_pages` marker, so a crash
    /// mid-write can never leave a page recorded as scraped while its contents
    /// are missing — which resume would then skip forever.
    pub async fn put_genre_page(
        &self,
        slug: &str,
        artists: &[Artist],
        neighbours: &[Neighbour],
    ) -> Result<(), sqlx::Error> {
        let now = Self::now();
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM genre_artists WHERE genre_slug = ?")
            .bind(slug)
            .execute(&mut *tx)
            .await?;
        for a in artists {
            sqlx::query(
                "INSERT OR REPLACE INTO genre_artists
                    (genre_slug, name, norm_name, spotify_id, x, y, color, font_size)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(slug)
            .bind(&a.name)
            .bind(crate::db::normalise_artist(&a.name))
            .bind(&a.spotify_id)
            .bind(a.x)
            .bind(a.y)
            .bind(&a.color)
            .bind(a.font_size)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query("DELETE FROM genre_edges WHERE genre_slug = ?")
            .bind(slug)
            .execute(&mut *tx)
            .await?;
        for n in neighbours {
            sqlx::query(
                "INSERT OR REPLACE INTO genre_edges (genre_slug, related_slug, relation, position)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(slug)
            .bind(&n.slug)
            .bind(n.relation.as_str())
            .bind(n.position)
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT OR REPLACE INTO scraped_pages (slug, scraped_at, artists) VALUES (?, ?, ?)",
        )
        .bind(slug)
        .bind(now)
        .bind(artists.len() as i64)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Genre slugs with no page walked yet, in map order — the tier 2 queue.
    pub async fn pending_pages(&self) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT g.slug FROM genres g
              LEFT JOIN scraped_pages p ON p.slug = g.slug
              WHERE p.slug IS NULL
              ORDER BY g.font_size DESC, g.slug",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(s,)| s).collect())
    }

    pub async fn summary(&self) -> Result<Summary, sqlx::Error> {
        let genres: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM genres").fetch_one(&self.pool).await?;
        let artists: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM genre_artists").fetch_one(&self.pool).await?;
        let edges: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM genre_edges").fetch_one(&self.pool).await?;
        let pages: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scraped_pages").fetch_one(&self.pool).await?;
        let last: (Option<i64>,) = sqlx::query_as(
            "SELECT MAX(t) FROM (SELECT MAX(scraped_at) AS t FROM genres
                                 UNION ALL SELECT MAX(scraped_at) FROM scraped_pages)",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(Summary {
            genres: genres.0,
            artists: artists.0,
            edges: edges.0,
            pages_scraped: pages.0,
            last_scraped_at: last.0,
        })
    }

    /// Coordinates for a set of genre names, keyed by the lowercased name.
    ///
    /// This is the join the whole exercise is for: MusicBrainz gives Tapedeck
    /// genre *names* per recording, and this turns them into positions.
    ///
    /// Not called yet — `mb_cache.genres` is populated and nothing reads it, so
    /// there is no genre vocabulary to join against until that backfill lands.
    /// Kept rather than deferred because it is the seam that decides whether
    /// any of this is worth building on: the overlap between Spotify's very
    /// fine-grained genre names and MusicBrainz's broader tags is the open
    /// question, and this is the function that answers it.
    #[allow(dead_code)]
    pub async fn coordinates_for(
        &self,
        names: &[String],
    ) -> Result<std::collections::HashMap<String, (f64, f64, String)>, sqlx::Error> {
        let mut out = std::collections::HashMap::new();
        // Chunked rather than one giant IN list: SQLite's parameter limit is
        // 999 by default and a big vocabulary would blow straight through it.
        for chunk in names.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT name_key, x, y, color FROM genres WHERE name_key IN ({})",
                placeholders
            );
            let mut q = sqlx::query_as::<_, (String, f64, f64, String)>(&sql);
            for n in chunk {
                q = q.bind(n.trim().to_lowercase());
            }
            for (k, x, y, c) in q.fetch_all(&self.pool).await? {
                out.insert(k, (x, y, c));
            }
        }
        Ok(out)
    }

    /// Wipe everything. The data is re-acquirable, so this is a reset rather
    /// than a destructive act — but it does mean another full walk.
    pub async fn clear(&self) -> Result<(), sqlx::Error> {
        for t in ["genre_edges", "genre_artists", "scraped_pages", "genres"] {
            sqlx::query(&format!("DELETE FROM {}", t)).execute(&self.pool).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::everynoise::parse::Relation;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    async fn store() -> EveryNoiseStore {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("td-en-{}-{}.db", std::process::id(), n));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        EveryNoiseStore::open(&format!("sqlite:{}", path.display())).await.unwrap()
    }

    fn genre(slug: &str, name: &str) -> Genre {
        Genre {
            name: name.into(),
            slug: slug.into(),
            x: 1.0,
            y: 2.0,
            color: "#abcdef".into(),
            font_size: 100.0,
            preview_track_id: None,
            example_title: None,
        }
    }

    #[tokio::test]
    async fn the_queue_is_what_has_not_been_walked_yet() {
        let s = store().await;
        s.put_genres(&[genre("pop", "pop"), genre("soul", "soul")]).await.unwrap();
        assert_eq!(s.pending_pages().await.unwrap().len(), 2);

        s.put_genre_page(
            "soul",
            &[Artist {
                name: "Al Green".into(),
                spotify_id: Some("abc".into()),
                x: 1.0,
                y: 1.0,
                color: "#fff".into(),
                font_size: 100.0,
            }],
            &[Neighbour {
                slug: "southernsoul".into(),
                name: "southern soul".into(),
                relation: Relation::Nearby,
                position: 0,
            }],
        )
        .await
        .unwrap();

        // Resume must skip what is done, or a restart re-asks the site for
        // everything it already has.
        assert_eq!(s.pending_pages().await.unwrap(), vec!["pop".to_string()]);

        let sum = s.summary().await.unwrap();
        assert_eq!((sum.genres, sum.artists, sum.edges, sum.pages_scraped), (2, 1, 1, 1));
    }

    /// The point of the whole exercise: a MusicBrainz genre name has to land on
    /// a position. Matching is case-insensitive because tag capitalisation is
    /// not meaningful.
    #[tokio::test]
    async fn genre_names_resolve_to_coordinates_regardless_of_case() {
        let s = store().await;
        s.put_genres(&[genre("deepfilthstep", "Deep Filthstep")]).await.unwrap();
        let found = s
            .coordinates_for(&["deep filthstep".into(), "nonexistent genre".into()])
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found.get("deep filthstep").map(|c| (c.0, c.1)), Some((1.0, 2.0)));
    }

    /// Re-scraping is how the data is refreshed, so a second write of the same
    /// genre must update it rather than fail or duplicate it.
    #[tokio::test]
    async fn rescraping_replaces_rather_than_duplicates() {
        let s = store().await;
        s.put_genres(&[genre("pop", "pop")]).await.unwrap();
        let mut moved = genre("pop", "pop");
        moved.x = 999.0;
        s.put_genres(&[moved]).await.unwrap();

        let sum = s.summary().await.unwrap();
        assert_eq!(sum.genres, 1);
        let c = s.coordinates_for(&["pop".into()]).await.unwrap();
        assert_eq!(c["pop"].0, 999.0);
    }
}
