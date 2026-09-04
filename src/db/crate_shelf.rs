//! The Crate and saved playlists.
//!
//! A recommendation's `first_played_at` is **derived on read**, never stored —
//! that is what makes the hit rate honest and needs no backfill job.

use super::*;

impl Database {
    /// Add something to the crate, or update what is already there.
    ///
    /// Returns whether the row was new. A second recommender naming the same
    /// record updates the reason rather than stacking a duplicate card — the
    /// unique index is on the *target*, not on who suggested it.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_recommendation(
        &self,
        user_id: i64,
        rec: &NewRecommendation,
        origin: &str,
        connection_id: Option<i64>,
    ) -> Result<(i64, bool), sqlx::Error> {
        let now = Self::now_secs();
        let existing: Option<(i64,)> = sqlx::query_as(&crate::sql!(
            "SELECT id FROM recommendations
             WHERE user_id = ? AND kind = ? AND LOWER(TRIM(artist)) = ?
               AND LOWER(TRIM(COALESCE(name,''))) = ?"
        ))
        .bind(user_id)
        .bind(&rec.kind)
        .bind(Self::artist_key(&rec.artist))
        .bind(Self::artist_key(rec.name.as_deref().unwrap_or("")))
        .fetch_optional(&self.pool)
        .await?;

        if let Some((id,)) = existing {
            sqlx::query(&crate::sql!(
                "UPDATE recommendations SET reason = ?, because = ?, origin = ?,
                        connection_id = ?, mbid = COALESCE(?, mbid)
                 WHERE id = ?"
            ))
            .bind(&rec.reason)
            .bind(&rec.because)
            .bind(origin)
            .bind(connection_id)
            .bind(&rec.mbid)
            .bind(id)
            .execute(&self.pool)
            .await?;
            return Ok((id, false));
        }

        // `fetch_all` — see `create_mcp_connection`. `fetch_one` here would
        // leave the row invisible to the next read on another pool connection.
        let row: Vec<(i64,)> = sqlx::query_as(&crate::sql!(
            "INSERT INTO recommendations
                (user_id, kind, artist, name, mbid, reason, because, origin,
                 connection_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ))
        .bind(user_id)
        .bind(&rec.kind)
        .bind(&rec.artist)
        .bind(&rec.name)
        .bind(&rec.mbid)
        .bind(&rec.reason)
        .bind(&rec.because)
        .bind(origin)
        .bind(connection_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        Ok((row.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)?, true))
    }
    /// The crate, with each entry's first listen derived from the history.
    ///
    /// **The derivation is one bounded scan, not a subquery per row.** A
    /// correlated subquery whose join key is `LOWER(TRIM(artist))` can never be
    /// indexed, so it is a full scan *per crate entry* by construction — which
    /// is exactly the quadratic that made Rediscovery take two minutes on the
    /// real host. Here the crate's own artist keys are collected first and used
    /// as an `IN` filter, so the history is walked once and returns only rows
    /// that could possibly match.
    pub async fn list_crate(&self, user_id: i64) -> Result<Vec<CrateEntry>, sqlx::Error> {
        let rows: Vec<RecommendationRow> = sqlx::query_as(&crate::sql!(
            "SELECT r.id, r.kind, r.artist, r.name, r.mbid, r.reason, r.because,
                    r.origin, r.status, r.created_at, r.acted_at
             FROM recommendations r
             WHERE r.user_id = ? ORDER BY r.created_at DESC",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(Vec::new());
        }

        let keys: Vec<String> = rows
            .iter()
            .map(|r| Self::artist_key(&r.artist))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let placeholders = vec!["?"; keys.len()].join(",");
        let sql = format!(
            "SELECT LOWER(TRIM(artist)) AS ak,
                    LOWER(TRIM(COALESCE(album,''))) AS bk,
                    LOWER(TRIM(title)) AS tk,
                    MIN(timestamp) AS first_at,
                    COUNT(*) AS plays
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(TRIM(artist)) IN ({placeholders})
             GROUP BY ak, bk, tk"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, (String, String, String, i64, i64)>(&stmt).bind(user_id);
        for k in &keys {
            q = q.bind(k);
        }
        let heard = q.fetch_all(&self.pool).await?;

        let out = rows
            .into_iter()
            .map(|r| {
                let ak = Self::artist_key(&r.artist);
                let nk = Self::artist_key(r.name.as_deref().unwrap_or(""));
                // Fold the track-level scan up to whatever granularity this
                // entry is about.
                let matches = heard.iter().filter(|(a, b, t, _, _)| {
                    *a == ak
                        && match r.kind.as_str() {
                            "album" => *b == nk,
                            "track" => *t == nk,
                            _ => true,
                        }
                });
                let mut first: Option<i64> = None;
                let mut plays = 0i64;
                for (_, _, _, at, n) in matches {
                    first = Some(first.map_or(*at, |f: i64| f.min(*at)));
                    plays += n;
                }
                CrateEntry { rec: r, first_played_at: first, plays }
            })
            .collect();
        Ok(out)
    }
    /// Whether the user already has this, and how much. The guard that stops a
    /// "recommendation" of a record with 43 plays — that is not a tip, it is
    /// Rediscovery, and the tool says so rather than filing it.
    pub async fn already_heard(
        &self,
        user_id: i64,
        kind: &str,
        artist: &str,
        name: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let ak = Self::artist_key(artist);
        let nk = Self::artist_key(name.unwrap_or(""));
        let sql = match kind {
            "album" => {
                "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE \
                        AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(COALESCE(album,''))) = ?"
            }
            "track" => {
                "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE \
                        AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(title)) = ?"
            }
            _ => {
                "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE \
                  AND LOWER(TRIM(artist)) = ?"
            }
        };
        let stmt = crate::sql!(sql);
        let mut q = sqlx::query_as::<_, (i64,)>(&stmt).bind(user_id).bind(&ak);
        if kind == "album" || kind == "track" {
            q = q.bind(&nk);
        }
        Ok(q.fetch_one(&self.pool).await?.0)
    }
    pub async fn set_recommendation_status(
        &self,
        user_id: i64,
        id: i64,
        status: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE recommendations SET status = ?, acted_at = ? WHERE id = ? AND user_id = ?",
        ))
        .bind(status)
        .bind(Self::now_secs())
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    pub async fn delete_recommendation(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let res =
            sqlx::query(&crate::sql!("DELETE FROM recommendations WHERE id = ? AND user_id = ?"))
                .bind(id)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Save a playlist, replacing one with the same idempotency key.
    ///
    /// The replace is what makes a retrying agent safe: a network blip between
    /// "created" and "the client heard about it" would otherwise leave a second
    /// copy every time, and an agent that retries three times leaves three.
    pub async fn save_playlist(
        &self,
        user_id: i64,
        name: &str,
        description: Option<&str>,
        origin: &str,
        connection_id: Option<i64>,
        idempotency_key: Option<&str>,
        tracks: &[SavedPlaylistTrack],
    ) -> Result<(i64, bool), sqlx::Error> {
        let now = Self::now_secs();
        let mut tx = self.pool.begin().await?;

        let existing: Option<(i64,)> = match idempotency_key {
            Some(key) => {
                sqlx::query_as(&crate::sql!(
                    "SELECT id FROM playlists WHERE user_id = ? AND idempotency_key = ?",
                ))
                .bind(user_id)
                .bind(key)
                .fetch_optional(&mut *tx)
                .await?
            }
            None => None,
        };

        let (id, replaced) = match existing {
            Some((id,)) => {
                sqlx::query(&crate::sql!(
                    "UPDATE playlists SET name = ?, description = ?, updated_at = ? WHERE id = ?",
                ))
                .bind(name)
                .bind(description)
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(&crate::sql!("DELETE FROM playlist_tracks WHERE playlist_id = ?"))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                (id, true)
            }
            None => {
                let row: (i64,) = sqlx::query_as(&crate::sql!(
                    "INSERT INTO playlists
                        (user_id, name, description, origin, connection_id, idempotency_key,
                         created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
                ))
                .bind(user_id)
                .bind(name)
                .bind(description)
                .bind(origin)
                .bind(connection_id)
                .bind(idempotency_key)
                .bind(now)
                .bind(now)
                .fetch_one(&mut *tx)
                .await?;
                (row.0, false)
            }
        };

        for (i, t) in tracks.iter().enumerate() {
            sqlx::query(&crate::sql!(
                "INSERT INTO playlist_tracks (playlist_id, position, title, artist, album, note)
                 VALUES (?, ?, ?, ?, ?, ?)",
            ))
            .bind(id)
            .bind(i as i64)
            .bind(&t.title)
            .bind(&t.artist)
            .bind(&t.album)
            .bind(&t.note)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok((id, replaced))
    }
    pub async fn list_playlists(
        &self,
        user_id: i64,
    ) -> Result<Vec<SavedPlaylistRecord>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT p.id, p.name, p.description, p.origin, p.created_at, p.updated_at,
                    p.pushed_at, p.pushed_to, c.name AS connection_name,
                    (SELECT COUNT(*) FROM playlist_tracks t WHERE t.playlist_id = p.id) AS tracks
             FROM playlists p LEFT JOIN mcp_connections c ON c.id = p.connection_id
             WHERE p.user_id = ? ORDER BY p.updated_at DESC",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    pub async fn playlist_tracks(
        &self,
        user_id: i64,
        playlist_id: i64,
    ) -> Result<Vec<SavedPlaylistTrack>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT t.title, t.artist, t.album, t.note
             FROM playlist_tracks t JOIN playlists p ON p.id = t.playlist_id
             WHERE t.playlist_id = ? AND p.user_id = ? ORDER BY t.position ASC"
        ))
        .bind(playlist_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// A playlist's tracks with the artwork and play count the listening
    /// history already holds for them.
    ///
    /// A stored playlist carries names and nothing else — that is deliberate,
    /// since it is resolved against a real library at push time and Tapedeck
    /// stores no server item ids for a generated list. But every track in one
    /// came *from* this user's listening, so the covers are already here, and a
    /// list of bare text beside a Rediscovery panel full of sleeves looks like
    /// two different applications.
    ///
    /// **One bounded scan, not a lookup per track.** Matching on
    /// `LOWER(TRIM(artist))` cannot use an index, so a correlated subquery would
    /// be a full scan per row — the quadratic that made Rediscovery take two
    /// minutes on the real host. The playlist's own artist keys become an `IN`
    /// filter instead, so the history is walked once and returns only rows that
    /// could match. Same shape as `list_crate`.
    pub async fn playlist_tracks_detailed(
        &self,
        user_id: i64,
        playlist_id: i64,
    ) -> Result<Vec<PlaylistTrackView>, sqlx::Error> {
        let tracks = self.playlist_tracks(user_id, playlist_id).await?;
        if tracks.is_empty() {
            return Ok(Vec::new());
        }

        let keys: Vec<String> = tracks
            .iter()
            .map(|t| Self::artist_key(&t.artist))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let placeholders = vec!["?"; keys.len()].join(",");
        let sql = format!(
            "SELECT LOWER(TRIM(artist)) AS ak, LOWER(TRIM(title)) AS tk,
                    COUNT(*) AS plays,
                    {COVER_PICK} AS cover_pick
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(TRIM(artist)) IN ({placeholders})
             GROUP BY ak, tk"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, (String, String, i64, Option<String>)>(&stmt).bind(user_id);
        for k in &keys {
            q = q.bind(k);
        }
        let heard = q.fetch_all(&self.pool).await?;

        Ok(tracks
            .into_iter()
            .map(|t| {
                let ak = Self::artist_key(&t.artist);
                let tk = Self::artist_key(&t.title);
                let hit = heard.iter().find(|(a, k, _, _)| *a == ak && *k == tk);
                let cover = Cover::parse(hit.and_then(|h| h.3.as_deref()));
                PlaylistTrackView {
                    // A track the user has never played is a normal outcome —
                    // it keeps its lettered tile and no count, rather than
                    // being hidden or marked as a problem.
                    plays: hit.map(|h| h.2).unwrap_or(0),
                    artwork_url: cover.artwork_url,
                    caa_id: cover.caa_id,
                    caa_release_mbid: cover.caa_release_mbid,
                    track: t,
                }
            })
            .collect())
    }
    pub async fn delete_playlist(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Tracks reference the playlist and foreign keys are on, so they go
        // first — the same rule `delete_chain` follows.
        sqlx::query(&crate::sql!(
            "DELETE FROM playlist_tracks WHERE playlist_id IN
                (SELECT id FROM playlists WHERE id = ? AND user_id = ?)"
        ))
        .bind(id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        let res = sqlx::query(&crate::sql!("DELETE FROM playlists WHERE id = ? AND user_id = ?"))
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }
    pub async fn mark_playlist_pushed(
        &self,
        user_id: i64,
        id: i64,
        target: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE playlists SET pushed_at = ?, pushed_to = ? WHERE id = ? AND user_id = ?",
        ))
        .bind(Self::now_secs())
        .bind(target)
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
