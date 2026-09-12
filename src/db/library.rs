//! Album and artist pages — aggregation over `scrobbles` matched by *name*,
//! never by MBID, because most of a library is un-enriched and those are
//! exactly the records worth a page.

use super::*;

impl Database {
    /// Headline numbers for one album: plays, seconds, distinct tracks, first
    /// and last heard, and the newest release year seen.
    #[allow(clippy::type_complexity)]
    pub async fn album_summary(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<Option<AlbumSummary>, sqlx::Error> {
        let row: Option<AlbumSummary> = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) AS plays,
                    COALESCE(SUM(duration), 0) AS seconds,
                    COUNT(DISTINCT LOWER(TRIM(title))) AS track_count,
                    MIN(timestamp) AS first_heard,
                    MAX(timestamp) AS last_heard,
                    MAX(release_year) AS year
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
               AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?"
        ))
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.filter(|r| r.plays > 0))
    }
    /// The tracklist beside the reader: every distinct track of one record, in
    /// the order the record plays.
    ///
    /// Deliberately *not* [`Self::album_tracks`], which answers a different
    /// question and pays for it — that one carries the whole quality record per
    /// track through a dozen correlated subqueries, and the reader wants a
    /// title, a running time and how often it has been played. One grouped scan.
    ///
    /// Ordered by track number where the tagger supplied one and alphabetically
    /// where it did not, because a reader's sidebar is a *place in a record*
    /// rather than a chart — sorting it by plays would move the row under the
    /// cursor as the listening changes.
    pub async fn album_track_list(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<Vec<(String, i64, Option<i64>, Option<i64>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT title, COUNT(*) AS plays, MAX(duration) AS duration,
                    MIN(track_number) AS track_number
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(COALESCE(album, ''))) = ?
              GROUP BY LOWER(TRIM(title))
              ORDER BY track_number IS NULL, track_number ASC, LOWER(TRIM(title)) ASC"
        ))
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .fetch_all(&self.pool)
        .await
    }
    /// How many times this user has played one track, skips excluded as ever.
    pub async fn track_play_count(
        &self,
        user_id: i64,
        artist: &str,
        title: &str,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(title)) = ?"
        ))
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(title.trim().to_lowercase())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
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
        sqlx::query_as(&crate::sql!(
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
             ORDER BY plays DESC, track_number ASC"
        ))
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
        let mut rows: Vec<AlbumAggregate> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT album AS name, COUNT(*) AS plays, MAX(release_year) AS year,
                    {COVER_PICK} AS cover_pick
             FROM scrobbles s
             WHERE user_id = ? AND skipped = FALSE AND {CREDITS_ARTIST}
               AND album IS NOT NULL AND TRIM(album) != ''
               AND (? IS NULL OR LOWER(TRIM(album)) != ?)
             GROUP BY LOWER(TRIM(album))
             ORDER BY plays DESC LIMIT ?"
        )))
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(exclude.map(|e| e.trim().to_lowercase()))
        .bind(exclude.map(|e| e.trim().to_lowercase()))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }
    /// Headline numbers for one artist, plus their share of everything.
    pub async fn artist_summary(
        &self,
        user_id: i64,
        artist: &str,
    ) -> Result<Option<ArtistSummary>, sqlx::Error> {
        let row: Option<ArtistSummary> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT COUNT(*) AS plays,
                    COALESCE(SUM(duration), 0) AS seconds,
                    COUNT(DISTINCT LOWER(TRIM(title))) AS track_count,
                    COUNT(DISTINCT CASE WHEN album IS NOT NULL AND TRIM(album) != ''
                          THEN LOWER(TRIM(album)) END) AS album_count,
                    MIN(timestamp) AS first_heard,
                    MAX(timestamp) AS last_heard
             FROM scrobbles s
             WHERE user_id = ? AND skipped = FALSE AND {CREDITS_ARTIST}"
        )))
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
        // Over the credits, not over `scrobbles.artist` — a performer who is
        // never the primary would otherwise have no rank at all, and the total
        // would be a count of record artists rather than of artists heard.
        let rows: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT sa.artist_key, COUNT(*) AS plays
               FROM scrobble_artists sa
               JOIN scrobbles s ON s.id = sa.scrobble_id
              WHERE s.user_id = ? AND s.skipped = FALSE
              GROUP BY sa.artist_key ORDER BY plays DESC"
        ))
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
        sqlx::query_as(&crate::sql!(&format!(
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
             FROM scrobbles s
             WHERE user_id = ? AND skipped = FALSE AND {CREDITS_ARTIST}
             GROUP BY LOWER(TRIM(title))
             ORDER BY plays DESC LIMIT ?"
        )))
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
        sqlx::query_as(&crate::sql!(&format!(
            "SELECT strftime('%Y', timestamp, 'unixepoch') AS year, COUNT(*) AS plays
             FROM scrobbles s
             WHERE user_id = ? AND skipped = FALSE AND {CREDITS_ARTIST}
             GROUP BY year ORDER BY year ASC"
        )))
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
        let (n,): (i64,) = sqlx::query_as(&crate::sql!(&format!(
            "SELECT COUNT(*) FROM scrobbles s
              WHERE user_id = ? AND skipped = FALSE AND {CREDITS_ARTIST}"
        )))
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }
    pub async fn total_plays(&self, user_id: i64) -> Result<i64, sqlx::Error> {
        let (n,): (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }
    /// Best artwork known for an album — used by the album hero and the cards.
    /// Does this album already have a cover, from any source?
    ///
    /// The question a client asks **before** reading a megabyte off disk and
    /// offering it, so the common case costs one small `GET` instead.
    ///
    /// **Deliberately not `album_artwork(..).is_some()`**, which is the obvious
    /// reuse and is subtly wrong here. That one picks a cover to *display* and
    /// therefore excludes skips — so an album whose only listens are skips
    /// would answer "nothing held" for ever, and a client that trusts the
    /// answer re-uploads the same sleeve on every run. The two questions differ:
    /// one is "what should I show", this is "do you need this from me", and a
    /// skipped listen carrying a cover is a fact either way.
    ///
    /// A hand-set cover counts. `entities.image_url` is what the album page
    /// prefers over everything the listens carry, so answering `false` while
    /// one is set would invite a client to send a picture that would never be
    /// shown. Matched on `entity_key` rather than folded columns, so it is an
    /// indexed lookup — and **nothing is created**: a client asking a question
    /// must not mint an entity as a side effect, which is why `resolve` is no
    /// use here.
    pub async fn album_art_held(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<bool, sqlx::Error> {
        let (a, al) = (artist.trim().to_lowercase(), album.trim().to_lowercase());
        let held: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT
               CASE WHEN EXISTS(
                 SELECT 1 FROM scrobbles
                  WHERE user_id = ? AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?
                    AND (artwork_url IS NOT NULL AND artwork_url != '' OR caa_id > 0)
               ) OR EXISTS(
                 SELECT 1 FROM entities
                  WHERE entity_key = ? AND image_url IS NOT NULL AND image_url != ''
               ) THEN 1 ELSE 0 END"
        ))
        .bind(user_id)
        .bind(&a)
        .bind(&al)
        .bind(Self::entity_key("release", artist, album))
        .fetch_one(&self.pool)
        .await?;
        Ok(held.0 == 1)
    }

    /// Stamp a cover a client read off its own files onto that album's listens.
    ///
    /// Writes `artwork_url` and nothing else, because that column **is** the
    /// artwork backfill's queue and `album_artwork` reads it back — so a cover
    /// that arrived with the listens is exactly what a Plex-sourced listen
    /// carries, and it is served by the path that already exists. A
    /// `cover_override` would say somebody set this by hand, and nobody did.
    ///
    /// **Only rows that have none**, which includes the `artwork_url = ''`
    /// tombstone — and overwriting that one is the point rather than an
    /// oversight. It means "asked every provider, nobody had a picture", and
    /// the whole premise here is that the listener's own file does.
    ///
    /// Skips are filled too. Artwork is not a count, and a skipped listen
    /// showing a lettered tile beside its neighbours would be the odd one out
    /// for no reason anybody could see.
    pub async fn set_album_artwork_from_library(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
        url: &str,
    ) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET artwork_url = ?
              WHERE user_id = ? AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?
                AND COALESCE(artwork_url, '') = ''",
        ))
        .bind(url)
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    pub async fn album_artwork(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
    ) -> Result<Option<(Option<String>, Option<i64>, Option<String>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT artwork_url, caa_id, caa_release_mbid FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
               AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(album)) = ?
               AND (artwork_url IS NOT NULL AND artwork_url != '' OR caa_id > 0)
             ORDER BY timestamp DESC LIMIT 1"
        ))
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await
    }
}
