//! Rediscovery — every collection is built from the user's **own** history.
//! Nothing here may suggest music they do not own; that is the Crate's job.

use super::*;

impl Database {
    /// The far end's id for this track, if a previous push resolved it.
    ///
    /// **A hand-picked match never expires.** Somebody looked at the candidates
    /// and said which one it was; re-asking a search that already got it wrong
    /// once would throw that away on a timer. An automatic one is only a
    /// remembered guess and gets `MATCH_TTL_SECS`.
    pub async fn playlist_match(
        &self,
        user_id: i64,
        source_id: i64,
        artist: &str,
        title: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String, String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT remote_id, origin, resolved_at FROM playlist_matches
              WHERE user_id = ? AND source_id = ? AND artist_key = ? AND title_key = ?"
        ))
        .bind(user_id)
        .bind(source_id)
        .bind(normalise_artist(artist))
        .bind(normalise_artist(title))
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(id, origin, at)| {
            let fresh = origin == "manual" || Self::now_secs() - at < Self::MATCH_TTL_SECS;
            fresh.then_some(id)
        }))
    }
    /// Forget a mapping, so the next push searches for it again. The pick was
    /// wrong, or the far end has changed what it holds.
    pub async fn forget_playlist_match(
        &self,
        user_id: i64,
        source_id: i64,
        artist: &str,
        title: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "DELETE FROM playlist_matches
              WHERE user_id = ? AND source_id = ? AND artist_key = ? AND title_key = ?"
        ))
        .bind(user_id)
        .bind(source_id)
        .bind(normalise_artist(artist))
        .bind(normalise_artist(title))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Remember what this name resolved to. A manual pick replaces an automatic
    /// one; an automatic one **never** overwrites a manual one, or the next
    /// push would quietly undo the correction somebody just made.
    pub async fn remember_playlist_match(
        &self,
        user_id: i64,
        source_id: i64,
        artist: &str,
        title: &str,
        remote_id: &str,
        remote_label: Option<&str>,
        manual: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO playlist_matches
                 (user_id, source_id, artist_key, title_key, remote_id, remote_label,
                  origin, resolved_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, source_id, artist_key, title_key) DO UPDATE SET
                 remote_id    = excluded.remote_id,
                 remote_label = excluded.remote_label,
                 origin       = excluded.origin,
                 resolved_at  = excluded.resolved_at
             WHERE excluded.origin = 'manual' OR playlist_matches.origin = 'auto'"
        ))
        .bind(user_id)
        .bind(source_id)
        .bind(normalise_artist(artist))
        .bind(normalise_artist(title))
        .bind(remote_id)
        .bind(remote_label)
        .bind(if manual { "manual" } else { "auto" })
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// The remote playlist a previous push of this name created, if any.
    pub async fn pushed_playlist(
        &self,
        user_id: i64,
        source_id: i64,
        name: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT remote_id FROM pushed_playlists
              WHERE user_id = ? AND source_id = ? AND name_key = ?"
        ))
        .bind(user_id)
        .bind(source_id)
        .bind(name.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }
    /// Remember which remote playlist this push owns, replacing whatever was
    /// remembered before — a stale id that the far end has deleted must not
    /// outlive the playlist that replaced it.
    pub async fn remember_pushed_playlist(
        &self,
        user_id: i64,
        source_id: i64,
        name: &str,
        remote_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO pushed_playlists (user_id, source_id, name_key, remote_id, pushed_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(user_id, source_id, name_key)
             DO UPDATE SET remote_id = excluded.remote_id, pushed_at = excluded.pushed_at",
        ))
        .bind(user_id)
        .bind(source_id)
        .bind(name.trim().to_lowercase())
        .bind(remote_id)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Case- and padding-insensitive key, so "Aphex Twin" and "aphex twin " are
    /// one artist rather than two rows that each learn half the metadata.
    ///
    /// The same fold `entity_key` uses and the same one the charts' bare
    /// `LOWER(TRIM(artist))` performs, which is what lets a chart row, a heart
    /// and a `scrobble_artists` row agree about who an artist is.
    ///
    /// Deliberately **not** `normalise_artist`. That one strips punctuation and
    /// accents to answer "are these two spellings the same performer?" for the
    /// sanitiser's review preview, where a false pairing costs one line in a
    /// preview. Here a wrong fold silently merges two artists' listening, so it
    /// stays conservative. Two normalisers exist; there is no third.
    pub fn artist_key(name: &str) -> String {
        name.trim().to_lowercase()
    }
    /// Record what we've learned about an artist. Existing values are kept
    /// unless the caller has something better — a later lookup that finds no
    /// image must not erase one we already had.
    pub async fn upsert_artist(
        &self,
        name: &str,
        image_url: Option<&str>,
        mbid: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let key = Self::artist_key(name);
        if key.is_empty() {
            return Ok(());
        }
        sqlx::query(&crate::sql!(
            "INSERT INTO artists (name, name_key, mbid, image_url, updated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(name_key) DO UPDATE SET
                mbid = COALESCE(excluded.mbid, artists.mbid),
                image_url = COALESCE(excluded.image_url, artists.image_url),
                updated_at = excluded.updated_at"
        ))
        .bind(name.trim())
        .bind(&key)
        .bind(mbid)
        .bind(image_url)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Played a lot once, not lately.
    ///
    /// The bar is relative rather than absolute: "a lot" means it is in the
    /// upper half of what you play at all, so a modest library still produces
    /// a list. `quiet_since` is the cutoff for "not lately".
    pub async fn forgotten_favourites(
        &self,
        user_id: i64,
        quiet_since: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        let mut rows: Vec<RediscoveryRow> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    {COVER_PICK} AS cover_pick
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
             HAVING COUNT(*) >= 3 AND MAX(timestamp) < ?
              ORDER BY COUNT(*) DESC, MAX(timestamp) ASC
              LIMIT ?"
        )))
        .bind(user_id)
        .bind(quiet_since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }
    /// Loved, but hardly ever played.
    ///
    /// A love is the strongest signal this app collects — you went out of your
    /// way — so a loved track with two plays is the sharpest "you meant to come
    /// back to this" it can produce.
    pub async fn loved_but_unplayed(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        // One pass over the history, joined — **not** three correlated
        // subqueries per love, which is what this was.
        //
        // That shape scanned `scrobbles` three times *for every loved
        // recording*, and `ORDER BY plays` meant every one had to be evaluated
        // before the `LIMIT` could apply, so it was quadratic in
        // (loves x listens) with no early exit. It looked fine in testing
        // because a fresh instance has no loves: the whole query returns
        // instantly at zero, and the cost only appears once someone has used
        // the feature for a while.
        //
        // Measured on a 49,143-listen history with 450 loves: **122 seconds**
        // on a Raspberry Pi 4, 7.6s on an M3 laptop. As one grouped scan it is
        // ~0.2s and ~30ms. Nothing errored at any point; the page just took two
        // minutes, which read as "SQLite is slow" rather than as one bad query.
        //
        // No index can rescue the old shape — `LOWER(TRIM(title))` is not a
        // column — so this is the fix rather than a tuning question.
        let mut rows: Vec<RediscoveryRow> = sqlx::query_as(&crate::sql!(&format!(
            "WITH played AS (
                 SELECT LOWER(TRIM(title)) AS k_title,
                        LOWER(TRIM(artist)) AS k_artist,
                        MAX(album) AS album,
                        COUNT(*) AS plays,
                        MAX(timestamp) AS last_played,
                        {COVER_PICK} AS cover_pick
                   FROM scrobbles
                  WHERE user_id = ? AND skipped = FALSE
                  GROUP BY 1, 2
             )
             SELECT e.name AS name, e.artist_name AS artist,
                    p.album AS album,
                    COALESCE(p.plays, 0) AS plays,
                    p.last_played AS last_played,
                    p.artwork_url AS artwork_url,
                    p.caa_id AS caa_id,
                    p.caa_release_mbid AS caa_release_mbid
               FROM loves l
               JOIN entities e ON e.id = l.entity_id
               -- LEFT, because a love whose track is not in the history at all
               -- is the sharpest row this list can produce, not one to drop.
               LEFT JOIN played p
                      ON p.k_title = LOWER(TRIM(e.name))
                     AND p.k_artist = LOWER(TRIM(e.artist_name))
              WHERE l.user_id = ? AND e.kind = 'recording'
              ORDER BY plays ASC, l.created_at DESC
              LIMIT ?"
        )))
        .bind(user_id)
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }
    /// Played exactly once, and long enough ago to have been forgotten.
    ///
    /// A record you heard once and never returned to is either a miss or a
    /// thing you never got round to. This app can't tell which, which is the
    /// point of showing it to you.
    pub async fn heard_once(
        &self,
        user_id: i64,
        before: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        let mut rows: Vec<RediscoveryRow> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    {COVER_PICK} AS cover_pick
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
             HAVING COUNT(*) = 1 AND MAX(timestamp) < ?
              ORDER BY MAX(timestamp) DESC
              LIMIT ?"
        )))
        .bind(user_id)
        .bind(before)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }
    /// What you were playing this time of year, in years gone by.
    ///
    /// Month-of-year in the *user's* timezone would be more correct, but the
    /// window is ±3 weeks wide, so an hour of offset cannot move anything
    /// across it — and doing it in SQL keeps this one query rather than a scan.
    /// Excludes the current year, or it would just be "recently played".
    pub async fn seasonal(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        let mut rows: Vec<RediscoveryRow> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    {COVER_PICK} AS cover_pick
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND STRFTIME('%Y', timestamp, 'unixepoch') <> STRFTIME('%Y', 'now')
                AND ABS(
                      CAST(STRFTIME('%j', timestamp, 'unixepoch') AS INTEGER)
                    - CAST(STRFTIME('%j', 'now') AS INTEGER)
                    ) <= 21
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
              ORDER BY COUNT(*) DESC
              LIMIT ?"
        )))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }
    /// Records and tapes that haven't left the shelf.
    ///
    /// The one list here that isn't about listens at all — it reads
    /// `physical_plays`, so it only says something once the shelf is in use.
    /// A record never played is included: never is longer than 18 months.
    pub async fn dusty_shelf(
        &self,
        user_id: i64,
        quiet_since: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT r.title AS name, r.artist AS artist, r.title AS album,
                    (SELECT COUNT(*) FROM physical_plays p WHERE p.release_id = r.id) AS plays,
                    (SELECT MAX(p.played_at) FROM physical_plays p WHERE p.release_id = r.id)
                      AS last_played,
                    r.artwork_url AS artwork_url, NULL AS caa_id,
                    r.mbid_release AS caa_release_mbid
               FROM physical_releases r
              WHERE r.user_id = ?
                AND COALESCE(
                      (SELECT MAX(p.played_at) FROM physical_plays p WHERE p.release_id = r.id), 0
                    ) < ?
              ORDER BY last_played ASC NULLS FIRST, r.added_at ASC
              LIMIT ?"
        ))
        .bind(user_id)
        .bind(quiet_since)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Tracks you only ever reach for after midnight.
    ///
    /// Hour-of-day in the user's timezone, because "after midnight" in UTC is
    /// the evening in one place and the afternoon in another — the same bug
    /// that made the dashboard's day start at 01:00. The offset is applied to
    /// the timestamp before the hour is taken.
    pub async fn late_night(
        &self,
        user_id: i64,
        tz_offset_secs: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        let mut rows: Vec<RediscoveryRow> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    {COVER_PICK} AS cover_pick
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
             HAVING COUNT(*) >= 3
                AND SUM(
                      CASE WHEN CAST(STRFTIME('%H', timestamp + ?, 'unixepoch') AS INTEGER)
                                 >= 0
                            AND CAST(STRFTIME('%H', timestamp + ?, 'unixepoch') AS INTEGER) < 5
                           THEN 1 ELSE 0 END
                    ) * 1.0 / COUNT(*) >= 0.7
              ORDER BY COUNT(*) DESC
              LIMIT ?"
        )))
        .bind(user_id)
        .bind(tz_offset_secs)
        .bind(tz_offset_secs)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }
    /// Album tracks you play that are never the single.
    ///
    /// "Buried on side B" in the design. Approximated by track number, which is
    /// the only positional information a listen carries: a track from the back
    /// half of a record that you nonetheless keep returning to.
    pub async fn deep_cuts(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        let mut rows: Vec<RediscoveryRow> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    {COVER_PICK} AS cover_pick
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
                AND track_number IS NOT NULL AND track_number >= 6
                AND album IS NOT NULL AND TRIM(album) != ''
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
             HAVING COUNT(*) >= 3
              ORDER BY COUNT(*) DESC
              LIMIT ?"
        )))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }
    /// Things you like that have never been through a given piece of gear.
    ///
    /// The join is the reason `chain_id` on a listen matters: a chain names its
    /// components, so "never heard on the HD 600s" is answerable without any
    /// per-listen equipment field. Restricted to tracks you have played more
    /// than once, because the interesting question is what your *favourites*
    /// sound like on it, not your whole library.
    pub async fn never_on_gear(
        &self,
        user_id: i64,
        equipment_id: i64,
        limit: i64,
    ) -> Result<Vec<RediscoveryRow>, sqlx::Error> {
        let mut rows: Vec<RediscoveryRow> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT title AS name, artist, MAX(album) AS album, COUNT(*) AS plays,
                    MAX(timestamp) AS last_played,
                    {COVER_PICK} AS cover_pick
               FROM scrobbles s
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND NOT EXISTS (
                      SELECT 1 FROM scrobbles s2
                        JOIN signal_chains c ON c.id = s2.chain_id
                       WHERE s2.user_id = s.user_id AND s2.skipped = FALSE
                         AND LOWER(TRIM(s2.title)) = LOWER(TRIM(s.title))
                         AND LOWER(TRIM(s2.artist)) = LOWER(TRIM(s.artist))
                         AND EXISTS (
                               SELECT 1 FROM json_each(c.components) j
                                WHERE json_extract(j.value, '$.equipment_id') = ?
                             )
                    )
              GROUP BY LOWER(TRIM(s.title)), LOWER(TRIM(s.artist))
             HAVING COUNT(*) > 1
              ORDER BY COUNT(*) DESC
              LIMIT ?"
        )))
        .bind(user_id)
        .bind(equipment_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());
        Ok(rows)
    }
    /// The `artists` row id for a name, creating the row if it's new.
    ///
    /// Needed because an artist portrait is served through a proxy keyed by
    /// that id — the image lives on the user's own media server and the browser
    /// can't reach it, the same problem `server/artwork.rs` solves for covers.
    /// An artist's cached genre tags. `Some(vec![])` is the tombstone for
    /// "asked MusicBrainz, it had none"; `None` means not looked up yet.
    pub async fn artist_genres(&self, name: &str) -> Result<Option<Vec<String>>, sqlx::Error> {
        let key = Self::artist_key(name);
        if key.is_empty() {
            return Ok(None);
        }
        let row: Option<(Option<String>,)> =
            sqlx::query_as(&crate::sql!("SELECT genres FROM artists WHERE name_key = ?"))
                .bind(&key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row
            .and_then(|r| r.0)
            .map(|j| serde_json::from_str::<Vec<String>>(&j).unwrap_or_default()))
    }
    pub async fn set_artist_genres(
        &self,
        name: &str,
        genres: &[String],
    ) -> Result<(), sqlx::Error> {
        let key = Self::artist_key(name);
        if key.is_empty() {
            return Ok(());
        }
        let json = serde_json::to_string(genres).unwrap_or_else(|_| "[]".into());
        sqlx::query(&crate::sql!(
            "INSERT INTO artists (name, name_key, genres, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(name_key) DO UPDATE SET genres = excluded.genres",
        ))
        .bind(name.trim())
        .bind(&key)
        .bind(&json)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn artist_row_id(&self, name: &str) -> Result<Option<i64>, sqlx::Error> {
        let key = Self::artist_key(name);
        if key.is_empty() {
            return Ok(None);
        }
        sqlx::query(&crate::sql!(
            "INSERT INTO artists (name, name_key, updated_at) VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
        ))
        .bind(name.trim())
        .bind(&key)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        let row: Option<(i64,)> =
            sqlx::query_as(&crate::sql!("SELECT id FROM artists WHERE name_key = ?"))
                .bind(&key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.0))
    }
    pub async fn artist_by_id(
        &self,
        id: i64,
    ) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
        let row: Option<(String, Option<String>)> =
            sqlx::query_as(&crate::sql!("SELECT name, image_url FROM artists WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row)
    }
    /// Both of an artist's pictures, for the upload path.
    ///
    /// `artist_by_id` returns the portrait beside the name and nothing else, so
    /// replacing the landscape had no way to learn what it was replacing —
    /// which is what deletes the file a new upload supersedes. Without it a
    /// user swapping a picture ten times leaves ten files on disk.
    pub async fn artist_picture_urls(
        &self,
        id: i64,
    ) -> Result<Option<(Option<String>, Option<String>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!("SELECT image_url, landscape_url FROM artists WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }
    /// Set or clear an artist portrait.
    ///
    /// Unlike `upsert_artist` this *overwrites*, including with NULL — it backs
    /// the manual override, and "I don't want that picture" has to be sayable.
    /// An empty string is the tombstone the backfill reads as "asked, nothing
    /// found", the same convention `artwork_url` uses.
    pub async fn set_artist_image(
        &self,
        id: i64,
        image_url: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE artists SET image_url = ?, updated_at = ? WHERE id = ?"
        ))
        .bind(image_url)
        .bind(Self::now_secs())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Set or clear an artist's **wide** image.
    ///
    /// Its own column and its own setter rather than a second argument to the
    /// one above, because the two are set independently: a portrait usually
    /// arrives from the library or a provider, while a landscape is almost
    /// always chosen by hand — nothing Tapedeck can reach supplies one.
    pub async fn set_artist_landscape(
        &self,
        id: i64,
        url: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE artists SET landscape_url = ?, updated_at = ? WHERE id = ?"
        ))
        .bind(url)
        .bind(Self::now_secs())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// A wide image for each of these artists, keyed by folded name.
    ///
    /// Reports asks for its chapters' top artists in one go rather than one
    /// request per card — there are twelve of them on a year page.
    pub async fn artist_landscapes(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        let keys: Vec<String> = names.iter().map(|n| normalise_artist(n)).collect();
        let placeholders = vec!["?"; keys.len()].join(",");
        let sql = format!(
            "SELECT name_key, landscape_url FROM artists
              WHERE name_key IN ({placeholders}) AND landscape_url IS NOT NULL
                AND TRIM(landscape_url) != ''"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, (String, String)>(&stmt);
        for k in &keys {
            q = q.bind(k);
        }
        Ok(q.fetch_all(&self.pool).await?.into_iter().collect())
    }
    /// A hand-set cover for a release entity, if there is one.
    ///
    /// Kept as its own query rather than a field on `EntityRow`: only the album
    /// page asks, and widening the row would mean touching every one of its
    /// column lists for a value nothing else reads.
    pub async fn entity_image(&self, id: i64) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as(&crate::sql!("SELECT image_url FROM entities WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|r| r.0).filter(|u| !u.is_empty()))
    }
    /// Set or clear a release's cover by hand.
    ///
    /// Restricted to `kind = 'release'` at the query. An artist's picture lives
    /// on the `artists` row — shared by name, and what the library proxy and the
    /// portrait backfill both address — so letting this write one too would give
    /// the same artist two places to be pictured from, and no rule about which
    /// wins. Returns false when the id is not a release, which the handler
    /// reports rather than silently succeeding.
    pub async fn set_entity_image(
        &self,
        id: i64,
        image_url: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE entities SET image_url = ? WHERE id = ? AND kind = 'release'",
        ))
        .bind(image_url)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Artists this user has listened to that still have no portrait.
    ///
    /// Ordered by play count so the ones actually visible on the charts are
    /// resolved first — a backfill that starts alphabetically spends its first
    /// run on artists nobody sees.
    pub async fn artists_without_images(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String,)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT s.artist FROM scrobbles s
              LEFT JOIN artists a ON a.name_key = LOWER(TRIM(s.artist))
              WHERE s.user_id = ? AND s.skipped = FALSE
                AND TRIM(s.artist) != ''
                AND LOWER(TRIM(s.artist)) {NOT_VARIOUS}
                AND (a.image_url IS NULL)
              GROUP BY LOWER(TRIM(s.artist))
              ORDER BY COUNT(*) DESC
              LIMIT ?"
        )))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }
    pub async fn artist_images(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = vec!["?"; names.len()].join(",");
        let sql = format!(
            "SELECT name_key, image_url FROM artists
             WHERE image_url IS NOT NULL AND name_key IN ({placeholders})"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, (String, String)>(&stmt);
        for n in names {
            q = q.bind(Self::artist_key(n));
        }
        Ok(q.fetch_all(&self.pool).await?.into_iter().collect())
    }
    /// How many distinct artists in this user's history have an image yet.
    pub async fn artist_image_coverage(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let total: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(DISTINCT artist) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let with_image: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(DISTINCT s.artist) FROM scrobbles s
             JOIN artists a ON a.name_key = LOWER(TRIM(s.artist))
             WHERE s.user_id = ? AND s.skipped = FALSE AND a.image_url IS NOT NULL"
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((total.0, with_image.0))
    }
}
