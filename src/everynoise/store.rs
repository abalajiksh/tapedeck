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

/// Two spellings of the same genre.
///
/// **This is not a third artist normaliser.** `db::normalise_artist` answers
/// "are these the same performer?", which is a question about identity and where
/// being too aggressive invents a merge that destroys data. This answers "are
/// these the same genre label?", where the two vocabularies being joined —
/// MusicBrainz's folksonomy tags and Every Noise's Spotify-derived names —
/// simply punctuate differently, and the cost of a wrong match is one genre
/// landing a few pixels off on a map.
///
/// Measured against a real 21,390-listen history: matching on `LOWER(TRIM())`
/// alone loses `dance-pop` (Every Noise spells it `dance pop`, 3,509 plays),
/// `variété française` (`variete francaise`, 2,957), `synth-pop` (`synthpop`)
/// and `folk pop` (`folk-pop` — the hyphen goes the *other* way). Folding
/// punctuation and accents recovers about 10% more tags.
///
/// It does not change *coverage* — those artists already landed via some
/// broader tag — it changes **resolution**, which is the whole point of a map.
pub fn genre_key(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Does a column exist? Same shape as `db.rs::has_column` — this database has
/// no migration list of its own, since it is rebuildable reference data.
async fn column_exists(pool: &Pool<Sqlite>, table: &str, column: &str) -> bool {
    let rows: Vec<(i64, String, String, i64, Option<String>, i64)> =
        sqlx::query_as(&format!("PRAGMA table_info({})", table))
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    rows.iter().any(|r| r.1 == column)
}

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
                loose_key TEXT NOT NULL DEFAULT '',
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
        // Added after the first scrape shipped, so an existing copy has to gain
        // the column rather than be re-walked — that walk is two hours against
        // someone else's server and the data has not changed.
        if !column_exists(&self.pool, "genres", "loose_key").await {
            sqlx::query("ALTER TABLE genres ADD COLUMN loose_key TEXT NOT NULL DEFAULT ''")
                .execute(&self.pool)
                .await?;
        }
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_en_genres_loose ON genres(loose_key)")
            .execute(&self.pool)
            .await?;
        self.backfill_loose_keys().await?;

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
                "INSERT INTO genres (slug, name, name_key, loose_key, x, y, color, font_size,
                                     preview_track_id, example_title, scraped_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(slug) DO UPDATE SET
                    name = excluded.name, name_key = excluded.name_key,
                    loose_key = excluded.loose_key,
                    x = excluded.x, y = excluded.y, color = excluded.color,
                    font_size = excluded.font_size,
                    preview_track_id = excluded.preview_track_id,
                    example_title = excluded.example_title,
                    scraped_at = excluded.scraped_at",
            )
            .bind(&g.slug)
            .bind(&g.name)
            .bind(g.name.trim().to_lowercase())
            .bind(genre_key(&g.name))
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

    /// Coordinates for a set of genre names, keyed by `genre_key` — call that
    /// on a tag to look its answer up.
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
                "SELECT loose_key, x, y, color FROM genres WHERE loose_key IN ({})",
                placeholders
            );
            let mut q = sqlx::query_as::<_, (String, f64, f64, String)>(&sql);
            for n in chunk {
                q = q.bind(genre_key(n));
            }
            for (k, x, y, c) in q.fetch_all(&self.pool).await? {
                out.insert(k, (x, y, c));
            }
        }
        Ok(out)
    }

    /// Fill `loose_key` for rows written before the column existed.
    ///
    /// Bounded by the size of the genre map (~6,300 rows) and a no-op once
    /// done, so it can run unconditionally at open.
    async fn backfill_loose_keys(&self) -> Result<(), sqlx::Error> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT slug, name FROM genres WHERE loose_key = ''")
                .fetch_all(&self.pool)
                .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for (slug, name) in &rows {
            sqlx::query("UPDATE genres SET loose_key = ? WHERE slug = ?")
                .bind(genre_key(name))
                .bind(slug)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        tracing::info!("🗺️ Every Noise: keyed {} genres for loose matching", rows.len());
        Ok(())
    }

    /// The whole map, for the backdrop: every genre's name, position and
    /// colour. ~6,300 rows and identical for every user, so it is worth the
    /// client caching it.
    pub async fn all_points(&self) -> Result<Vec<(String, f64, f64, String)>, sqlx::Error> {
        sqlx::query_as("SELECT name, x, y, color FROM genres ORDER BY font_size DESC")
            .fetch_all(&self.pool)
            .await
    }

    /// The map's extent, so the client can set a viewBox without scanning
    /// every point itself.
    pub async fn extent(&self) -> Result<(f64, f64), sqlx::Error> {
        let row: (Option<f64>, Option<f64>) =
            sqlx::query_as("SELECT MAX(x), MAX(y) FROM genres").fetch_one(&self.pool).await?;
        Ok((row.0.unwrap_or(0.0), row.1.unwrap_or(0.0)))
    }

    /// Map genre names to slugs, keyed by `genre_key` — the entry point for
    /// anything that needs the adjacency graph, which is keyed by slug.
    pub async fn slugs_for(
        &self,
        names: &[String],
    ) -> Result<std::collections::HashMap<String, String>, sqlx::Error> {
        let mut out = std::collections::HashMap::new();
        for chunk in names.chunks(500) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql =
                format!("SELECT loose_key, slug FROM genres WHERE loose_key IN ({})", placeholders);
            let mut q = sqlx::query_as::<_, (String, String)>(&sql);
            for n in chunk {
                q = q.bind(genre_key(n));
            }
            for (k, slug) in q.fetch_all(&self.pool).await? {
                out.insert(k, slug);
            }
        }
        Ok(out)
    }

    /// The neighbours of a set of genres, in one relation.
    ///
    /// Returns `(from_slug, to_name, x, y, colour, position)`. `position` is the
    /// rank Every Noise itself gave the neighbour, so a caller can weight the
    /// closest ones higher rather than treating twenty neighbours as equal.
    pub async fn neighbours_of(
        &self,
        slugs: &[String],
        relation: &str,
    ) -> Result<Vec<(String, String, f64, f64, String, i64)>, sqlx::Error> {
        let mut out = Vec::new();
        for chunk in slugs.chunks(400) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT e.genre_slug, g.name, g.x, g.y, g.color, e.position
                   FROM genre_edges e JOIN genres g ON g.slug = e.related_slug
                  WHERE e.relation = ? AND e.genre_slug IN ({})",
                placeholders
            );
            let mut q = sqlx::query_as::<_, (String, String, f64, f64, String, i64)>(&sql);
            q = q.bind(relation);
            for s in chunk {
                q = q.bind(s);
            }
            out.extend(q.fetch_all(&self.pool).await?);
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
        assert_eq!(found.get(&genre_key("deep filthstep")).map(|c| (c.0, c.1)), Some((1.0, 2.0)));
    }

    /// The four spellings that actually cost coverage, measured against a real
    /// 21,390-listen history. Every Noise and MusicBrainz punctuate the same
    /// genre differently — and note `folk pop` goes the *other* way, so a rule
    /// that only stripped hyphens would still miss it.
    #[tokio::test]
    async fn the_two_vocabularies_punctuate_differently() {
        let s = store().await;
        s.put_genres(&[
            genre("dancepop", "dance pop"), // MusicBrainz: "dance-pop"
            genre("varietefrancaise", "variete francaise"), // MB: "variété française"
            genre("synthpop", "synthpop"),  // MB: "synth-pop"
            genre("folkpop", "folk-pop"),   // MB: "folk pop"
        ])
        .await
        .unwrap();

        let tags: Vec<String> = ["dance-pop", "variété française", "synth-pop", "folk pop"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let found = s.coordinates_for(&tags).await.unwrap();
        assert_eq!(
            found.len(),
            4,
            "all four must land; got {:?}",
            found.keys().collect::<Vec<_>>()
        );
    }

    /// A copy scraped before the column existed must gain it without a re-walk —
    /// that walk is two hours aimed at a frozen one-person site.
    #[tokio::test]
    async fn an_older_copy_gains_loose_keys_without_rescraping() {
        let s = store().await;
        s.put_genres(&[genre("dancepop", "dance pop")]).await.unwrap();
        // Put it back into the pre-migration state.
        sqlx::query("UPDATE genres SET loose_key = ''").execute(&s.pool).await.unwrap();
        assert!(s.coordinates_for(&["dance-pop".into()]).await.unwrap().is_empty());

        s.backfill_loose_keys().await.unwrap();
        assert_eq!(s.coordinates_for(&["dance-pop".into()]).await.unwrap().len(), 1);
    }

    /// Genre identity is not artist identity, and this must not quietly grow
    /// into a third normaliser: it folds punctuation and accents, nothing more.
    #[test]
    fn the_genre_key_folds_punctuation_and_accents_only() {
        assert_eq!(genre_key("Dance-Pop"), genre_key("dance pop"));
        assert_eq!(genre_key("variété française"), genre_key("variete francaise"));
        assert_eq!(genre_key("synth-pop"), "synthpop");
        // Distinct genres must stay distinct — the fold must not merge them.
        assert_ne!(genre_key("post-rock"), genre_key("post-punk"));
        assert_ne!(genre_key("hard rock"), genre_key("hardcore"));
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
