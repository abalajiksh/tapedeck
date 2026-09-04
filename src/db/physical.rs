//! The shelf: pressings, sides, spins, scans — and, for now, the enrichment
//! backfill queues, which sit inside this region without a banner of their
//! own. **Splitting `enrich` out of here is the next cut.**

use super::*;

impl Database {
    pub async fn list_physical(
        &self,
        user_id: i64,
        kind: Option<&str>,
    ) -> Result<Vec<PhysicalRelease>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT r.*,
                    (SELECT COUNT(*) FROM physical_plays p WHERE p.release_id = r.id) AS play_count,
                    (SELECT COALESCE(SUM(p.duration_secs), 0) FROM physical_plays p
                      WHERE p.release_id = r.id) AS played_secs,
                    (SELECT MAX(p.played_at) FROM physical_plays p WHERE p.release_id = r.id)
                      AS last_played,
                    (SELECT COUNT(*) FROM physical_images i WHERE i.release_id = r.id)
                      AS image_count,
                    -- The front sleeve, built exactly the way `image_payload`
                    -- builds one so a card and the gallery cannot disagree
                    -- about which file that is.
                    --
                    -- Ordered so the user's *own* scan wins over a provider's:
                    -- an upload has a `file_path` and is a photograph of the
                    -- copy actually in the room, which is the same reason the
                    -- gallery badges an upload as the user's own.
                    (SELECT COALESCE(i.thumb_url, i.url,
                                     '/api/v1/physical/images/' || i.id || '/file')
                       FROM physical_images i
                      WHERE i.release_id = r.id AND i.kind = 'front'
                      ORDER BY (i.file_path IS NULL), i.position, i.id
                      LIMIT 1) AS cover_thumb
               FROM physical_releases r
              WHERE r.user_id = ? AND (? IS NULL OR r.kind = ?)
              ORDER BY r.artist COLLATE NOCASE, r.title COLLATE NOCASE"
        ))
        .bind(user_id)
        .bind(kind)
        .bind(kind)
        .fetch_all(&self.pool)
        .await
    }
    pub async fn get_physical(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<PhysicalRelease>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT r.*,
                    (SELECT COUNT(*) FROM physical_plays p WHERE p.release_id = r.id) AS play_count,
                    (SELECT COALESCE(SUM(p.duration_secs), 0) FROM physical_plays p
                      WHERE p.release_id = r.id) AS played_secs,
                    (SELECT MAX(p.played_at) FROM physical_plays p WHERE p.release_id = r.id)
                      AS last_played,
                    (SELECT COUNT(*) FROM physical_images i WHERE i.release_id = r.id)
                      AS image_count,
                    -- The front sleeve, built exactly the way `image_payload`
                    -- builds one so a card and the gallery cannot disagree
                    -- about which file that is.
                    --
                    -- Ordered so the user's *own* scan wins over a provider's:
                    -- an upload has a `file_path` and is a photograph of the
                    -- copy actually in the room, which is the same reason the
                    -- gallery badges an upload as the user's own.
                    (SELECT COALESCE(i.thumb_url, i.url,
                                     '/api/v1/physical/images/' || i.id || '/file')
                       FROM physical_images i
                      WHERE i.release_id = r.id AND i.kind = 'front'
                      ORDER BY (i.file_path IS NULL), i.position, i.id
                      LIMIT 1) AS cover_thumb
               FROM physical_releases r WHERE r.user_id = ? AND r.id = ?"
        ))
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn create_physical(
        &self,
        user_id: i64,
        kind: &str,
        artist: &str,
        title: &str,
        year: Option<i64>,
        mbid_release: Option<&str>,
        discogs_id: Option<&str>,
        barcode: Option<&str>,
        catalog_number: Option<&str>,
        artwork_url: Option<&str>,
        tracks_json: &str,
        notes: Option<&str>,
        purchased_at: Option<i64>,
        acquired_as: Option<&str>,
        acquired_from: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let res = sqlx::query_as::<_, (i64,)>(&crate::sql!(
            "INSERT INTO physical_releases
                (user_id, kind, artist, title, year, mbid_release, discogs_id, barcode,
                 catalog_number, artwork_url, tracks, notes, added_at, purchased_at,
                 acquired_as, acquired_from)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ))
        .bind(user_id)
        .bind(kind)
        .bind(artist)
        .bind(title)
        .bind(year)
        .bind(mbid_release)
        .bind(discogs_id)
        .bind(barcode)
        .bind(catalog_number)
        .bind(artwork_url)
        .bind(tracks_json)
        .bind(notes)
        .bind(Self::now_secs())
        .bind(purchased_at)
        .bind(acquired_as)
        .bind(acquired_from)
        .fetch_all(&self.pool)
        .await?;
        res.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }
    /// Edit the shelf-keeping fields of a pressing: when it was bought, what it
    /// is normally played on, and the note beside it.
    ///
    /// Every field is `Option<Option<_>>` for the reason
    /// `POST /scrobbles/bulk-chain` learned the hard way: a bare `Option` cannot
    /// tell *absent* from *null*, so a typo'd key would read as "clear the
    /// chain" and quietly wipe an attribution while answering 200. Absent leaves
    /// the column alone; an explicit null clears it, which is how the picker's
    /// "—" option works.
    pub async fn update_physical(
        &self,
        user_id: i64,
        id: i64,
        purchased_at: Option<Option<i64>>,
        chain_id: Option<Option<i64>>,
        equipment_id: Option<Option<i64>>,
        notes: Option<Option<&str>>,
        kind: Option<&str>,
        chain_variant_id: Option<Option<i64>>,
        mbid_release: Option<Option<&str>>,
        acquired_as: Option<Option<&str>>,
        acquired_from: Option<Option<&str>>,
    ) -> Result<bool, sqlx::Error> {
        let mut sets: Vec<&str> = Vec::new();
        // A plain `Option` rather than a nested one, unlike everything else
        // here: `kind` carries a CHECK constraint and has no "cleared" state to
        // tell apart from absent, so `None` can only mean "not mentioned".
        if kind.is_some() {
            sets.push("kind = ?");
        }
        if purchased_at.is_some() {
            sets.push("purchased_at = ?");
        }
        if chain_id.is_some() {
            sets.push("chain_id = ?");
        }
        if equipment_id.is_some() {
            sets.push("equipment_id = ?");
        }
        if chain_variant_id.is_some() {
            sets.push("chain_variant_id = ?");
        }
        if notes.is_some() {
            sets.push("notes = ?");
        }
        // Typed in by hand when no lookup found the pressing. It also makes the
        // running-time lookup a single request instead of a search, which is
        // the difference between answering and being cut off by a proxy.
        if mbid_release.is_some() {
            sets.push("mbid_release = ?");
        }
        // How it came to be yours, and from where. Both clearable: a wrong
        // answer is worse than a blank, and "I actually don't remember" is a
        // real thing to want to say afterwards.
        if acquired_as.is_some() {
            sets.push("acquired_as = ?");
        }
        if acquired_from.is_some() {
            sets.push("acquired_from = ?");
        }
        if sets.is_empty() {
            // Nothing asked for is not a failure — but it must not report a
            // 404 either, so confirm the row is the caller's and stop.
            return Ok(self.get_physical(user_id, id).await?.is_some());
        }
        let sql = format!(
            "UPDATE physical_releases SET {} WHERE user_id = ? AND id = ?",
            sets.join(", ")
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query(&stmt);
        // Bound in the same order the SET clauses were pushed.
        if let Some(v) = kind {
            q = q.bind(v.to_string());
        }
        if let Some(v) = purchased_at {
            q = q.bind(v);
        }
        if let Some(v) = chain_id {
            q = q.bind(v);
        }
        if let Some(v) = equipment_id {
            q = q.bind(v);
        }
        if let Some(v) = chain_variant_id {
            q = q.bind(v);
        }
        if let Some(v) = notes {
            q = q.bind(v.map(|s| s.to_string()));
        }
        if let Some(v) = mbid_release {
            q = q.bind(v.map(|s| s.to_string()));
        }
        if let Some(v) = acquired_as {
            q = q.bind(v.map(|s| s.to_string()));
        }
        if let Some(v) = acquired_from {
            q = q.bind(v.map(|s| s.to_string()));
        }
        let res = q.bind(user_id).bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }
    /// The side currently on this user's deck, if any.
    pub async fn get_spin(&self, user_id: i64) -> Result<Option<PhysicalSpin>, sqlx::Error> {
        sqlx::query_as::<_, PhysicalSpin>(&crate::sql!(
            "SELECT * FROM physical_spins WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
    }
    /// Put a side on the deck, or update the one that is on it.
    ///
    /// An upsert on `user_id` rather than an insert, because the primary key is
    /// the whole rule: one deck per person. Starting a second side replaces the
    /// first rather than erroring, which matches what actually happened in the
    /// room — you took the record off.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_spin(
        &self,
        user_id: i64,
        release_id: i64,
        side: &str,
        started_at: i64,
        paused_at: Option<i64>,
        paused_total: i64,
        outcomes_json: &str,
        chain_id: Option<i64>,
        equipment_id: Option<i64>,
        start_secs: i64,
    ) -> Result<(), sqlx::Error> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(&crate::sql!(
            "INSERT INTO physical_spins
               (user_id, release_id, side, started_at, paused_at, paused_total,
                outcomes, chain_id, equipment_id, start_secs, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET
               release_id = excluded.release_id,
               side = excluded.side,
               started_at = excluded.started_at,
               paused_at = excluded.paused_at,
               paused_total = excluded.paused_total,
               outcomes = excluded.outcomes,
               chain_id = excluded.chain_id,
               equipment_id = excluded.equipment_id,
               start_secs = excluded.start_secs,
               updated_at = excluded.updated_at"
        ))
        .bind(user_id)
        .bind(release_id)
        .bind(side)
        .bind(started_at)
        .bind(paused_at)
        .bind(paused_total)
        .bind(outcomes_json)
        .bind(chain_id)
        .bind(equipment_id)
        .bind(start_secs)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Remember where a tape was left — and refuse to for anything else.
    ///
    /// A cassette is sequential. Stop it halfway through side A and eject, and
    /// the tape is still sitting at that point: put it back in tomorrow, press
    /// play, and it carries on from there. Nothing else on this shelf works
    /// that way, and the asymmetry is the design rather than an omission:
    ///
    /// * A **record** has random access and no state. You lift the arm and it
    ///   goes back in its sleeve; there is no "where it was left" to remember,
    ///   and inventing one would be modelling physics the object does not have.
    /// * A **disc** is random access too, and every player on earth starts it
    ///   at track one.
    ///
    /// So this writes nothing unless the release is a cassette. The guard is
    /// here rather than at the call site because it is a fact about the medium,
    /// and a second caller would otherwise have to remember it.
    pub async fn park_tape(
        &self,
        user_id: i64,
        release_id: i64,
        side: &str,
        secs: i64,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE physical_releases SET parked_side = ?, parked_secs = ?
               WHERE user_id = ? AND id = ? AND kind = 'cassette'"
        ))
        .bind(side)
        .bind(secs.max(0))
        .bind(user_id)
        .bind(release_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Wind a tape back to the start.
    pub async fn rewind_tape(&self, user_id: i64, release_id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE physical_releases SET parked_side = NULL, parked_secs = NULL
               WHERE user_id = ? AND id = ?"
        ))
        .bind(user_id)
        .bind(release_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Take whatever is on the deck off it.
    pub async fn clear_spin(&self, user_id: i64) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!("DELETE FROM physical_spins WHERE user_id = ?"))
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
    pub async fn list_physical_images(
        &self,
        user_id: i64,
        release_id: i64,
    ) -> Result<Vec<PhysicalImage>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT * FROM physical_images
              WHERE user_id = ? AND release_id = ?
              ORDER BY position ASC, id ASC"
        ))
        .bind(user_id)
        .bind(release_id)
        .fetch_all(&self.pool)
        .await
    }
    /// One image, scoped to its owner — the guard that stops ids being walked
    /// to pull somebody else's scans through the file endpoint.
    pub async fn get_physical_image(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<PhysicalImage>, sqlx::Error> {
        sqlx::query_as(&crate::sql!("SELECT * FROM physical_images WHERE user_id = ? AND id = ?"))
            .bind(user_id)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map(Some)
            .or_else(|e| if matches!(e, sqlx::Error::RowNotFound) { Ok(None) } else { Err(e) })
    }
    /// Store one image. A provider's own id makes this idempotent: re-fetching
    /// a release updates the rows it already holds rather than stacking a
    /// second copy of all nineteen booklet pages on top of the first.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_physical_image(
        &self,
        user_id: i64,
        release_id: i64,
        kind: &str,
        provider: &str,
        url: Option<&str>,
        thumb_url: Option<&str>,
        file_path: Option<&str>,
        mime: Option<&str>,
        bytes: Option<i64>,
        caption: Option<&str>,
        position: i64,
        remote_id: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        sqlx::query(&crate::sql!(
            "INSERT INTO physical_images
                (user_id, release_id, kind, provider, url, thumb_url, file_path, mime,
                 bytes, caption, position, remote_id, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             -- The `WHERE` is part of the conflict target, not decoration:
             -- SQLite matches an upsert against a *partial* unique index only
             -- when the predicate is repeated here, and rejects the statement
             -- outright otherwise. It is also what leaves uploads alone — they
             -- carry no provider id, so two scans of the same page really are
             -- two scans.
             ON CONFLICT(release_id, provider, remote_id) WHERE remote_id IS NOT NULL
             DO UPDATE SET
                kind = excluded.kind, url = excluded.url, thumb_url = excluded.thumb_url,
                caption = excluded.caption, position = excluded.position"
        ))
        .bind(user_id)
        .bind(release_id)
        .bind(kind)
        .bind(provider)
        .bind(url)
        .bind(thumb_url)
        .bind(file_path)
        .bind(mime)
        .bind(bytes)
        .bind(caption)
        .bind(position)
        .bind(remote_id)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        // `last_insert_rowid` is meaningless after an upsert that updated, so
        // the id is read back by the key that decided it.
        let row: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT id FROM physical_images
              WHERE release_id = ? AND provider = ?
                AND (remote_id IS ? OR (remote_id IS NULL AND ? IS NULL))
              ORDER BY id DESC LIMIT 1"
        ))
        .bind(release_id)
        .bind(provider)
        .bind(remote_id)
        .bind(remote_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }
    pub async fn update_physical_image(
        &self,
        user_id: i64,
        id: i64,
        kind: Option<&str>,
        caption: Option<Option<&str>>,
        position: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let mut sets: Vec<&str> = Vec::new();
        if kind.is_some() {
            sets.push("kind = ?");
        }
        if caption.is_some() {
            sets.push("caption = ?");
        }
        if position.is_some() {
            sets.push("position = ?");
        }
        if sets.is_empty() {
            return Ok(self.get_physical_image(user_id, id).await?.is_some());
        }
        let sql =
            format!("UPDATE physical_images SET {} WHERE user_id = ? AND id = ?", sets.join(", "));
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query(&stmt);
        if let Some(v) = kind {
            q = q.bind(v.to_string());
        }
        if let Some(v) = caption {
            q = q.bind(v.map(|s| s.to_string()));
        }
        if let Some(v) = position {
            q = q.bind(v);
        }
        let res = q.bind(user_id).bind(id).execute(&self.pool).await?;
        Ok(res.rows_affected() > 0)
    }
    /// Delete an image row and hand back the file it was holding, if any, so
    /// the caller can remove it from disk. The row goes first: a file left
    /// behind is litter, whereas a row pointing at a file that is gone renders
    /// as a broken image on the page.
    pub async fn delete_physical_image(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<Option<String>>, sqlx::Error> {
        let Some(img) = self.get_physical_image(user_id, id).await? else {
            return Ok(None);
        };
        sqlx::query(&crate::sql!("DELETE FROM physical_images WHERE user_id = ? AND id = ?"))
            .bind(user_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(Some(img.file_path))
    }
    /// Replace the stored tracklist — how a hand-corrected side split is saved.
    /// Give the listens already recorded from a side the durations their tracks
    /// were finally found to have.
    ///
    /// A shelf listen banked before the tracklist had durations stored NULL (or,
    /// before v0.74.1, a three-minute placeholder), so the record's chain and
    /// stylus hours were missing or invented. Once the real running times
    /// arrive, the listens are the only place the correction has to land —
    /// every hours figure in the application derives from `scrobbles.duration`.
    ///
    /// **Timestamps are deliberately not touched.** `physical_plays` records
    /// when a side *started* and nothing about when it ended, so where the
    /// tracks actually fell is unrecoverable; re-spacing them from the start
    /// would assume the side ran uninterrupted, which is precisely the case
    /// that produced the wrong data in the first place.
    ///
    /// Keyed on `source_id`, which `play_side` builds as
    /// `physical-<release>-<side>-<start>-<position>`. That is exact: it names
    /// this pressing, this side and this track, so it cannot reach a listen of
    /// another record that happens to share a title.
    pub async fn set_physical_listen_durations(
        &self,
        user_id: i64,
        release_id: i64,
        updates: &[(String, i64, i64)],
    ) -> Result<u64, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let mut touched = 0u64;
        for (side, position, secs) in updates {
            if *secs <= 0 {
                continue;
            }
            let pattern = format!("physical-{release_id}-{side}-%-{position}");
            let r = sqlx::query(&crate::sql!(
                "UPDATE scrobbles SET duration = ?
                  WHERE user_id = ? AND source_name = 'physical' AND source_id LIKE ?"
            ))
            .bind(secs)
            .bind(user_id)
            .bind(&pattern)
            .execute(&mut *tx)
            .await?;
            touched += r.rows_affected();
        }
        tx.commit().await?;
        Ok(touched)
    }
    /// Bring a side's recorded *wear* back in line with its real running time.
    ///
    /// `physical_plays.duration_secs` is what the side card and the release
    /// total read, and it was banked from the same placeholder the listens
    /// were — so correcting the listens alone leaves the shelf saying one thing
    /// and the hours another.
    ///
    /// **Only whole-side plays are touched.** `track_count` is the one thing
    /// distinguishing a side that ran through from a needle dropped on a single
    /// song, and a partial play's wear is a subset nothing here can reconstruct:
    /// the row records how many tracks played, never which. Rewriting those from
    /// the full side would charge the stylus for music it did not play, which is
    /// the error this whole pass exists to remove.
    pub async fn recompute_side_play_wear(
        &self,
        user_id: i64,
        release_id: i64,
        side: &str,
        full_track_count: i64,
        side_secs: i64,
    ) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(&crate::sql!(
            "UPDATE physical_plays SET duration_secs = ?
              WHERE user_id = ? AND release_id = ? AND side = ? AND track_count = ?"
        ))
        .bind(side_secs)
        .bind(user_id)
        .bind(release_id)
        .bind(side)
        .bind(full_track_count)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }
    pub async fn set_physical_tracks(
        &self,
        user_id: i64,
        id: i64,
        tracks_json: &str,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE physical_releases SET tracks = ? WHERE user_id = ? AND id = ?",
        ))
        .bind(tracks_json)
        .bind(user_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Remove a pressing. Returns the uploaded scans it was holding so the
    /// caller can take them off disk — nothing else knows those files exist,
    /// so skipping this leaves them orphaned forever.
    pub async fn delete_physical(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<Vec<String>>, sqlx::Error> {
        let files: Vec<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT file_path FROM physical_images
              WHERE user_id = ? AND release_id = ? AND file_path IS NOT NULL"
        ))
        .bind(user_id)
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        // Plays and images both reference the release, and foreign keys are on.
        sqlx::query(&crate::sql!(
            "DELETE FROM physical_images WHERE user_id = ? AND release_id = ?"
        ))
        .bind(user_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        sqlx::query(&crate::sql!(
            "DELETE FROM physical_plays WHERE user_id = ? AND release_id = ?"
        ))
        .bind(user_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        let res =
            sqlx::query(&crate::sql!("DELETE FROM physical_releases WHERE user_id = ? AND id = ?"))
                .bind(user_id)
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok((res.rows_affected() > 0).then(|| files.into_iter().map(|(f,)| f).collect()))
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn record_physical_play(
        &self,
        user_id: i64,
        release_id: i64,
        side: &str,
        played_at: i64,
        duration_secs: i64,
        track_count: i64,
        equipment_id: Option<i64>,
        chain_id: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        let res = sqlx::query_as::<_, (i64,)>(&crate::sql!(
            "INSERT INTO physical_plays
                (user_id, release_id, side, played_at, duration_secs, track_count,
                 equipment_id, chain_id, recorded_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
        ))
        .bind(user_id)
        .bind(release_id)
        .bind(side)
        .bind(played_at)
        .bind(duration_secs)
        .bind(track_count)
        .bind(equipment_id)
        .bind(chain_id)
        .bind(Self::now_secs())
        .fetch_all(&self.pool)
        .await?;
        res.first().map(|r| r.0).ok_or(sqlx::Error::RowNotFound)
    }
    /// How often each side has been played, and for how long.
    ///
    /// Derived from `physical_plays` rather than kept as a counter, the same
    /// rule chain hours, gear hours and the release's own totals follow: a
    /// number that is incremented can drift from the events it claims to count
    /// and there is no way to tell afterwards which of the two is wrong. It
    /// also means deleting a side play takes its wear back, which an
    /// accumulator could never do.
    ///
    /// `secs` is the running time of the tracks that actually played — the
    /// same figure the stylus is charged for, so a side started at track four
    /// counts four tracks' worth and not the whole side. It is nominal, not
    /// wall clock: nothing here knows how long you left the arm parked between
    /// tracks.
    ///
    /// Returns only sides that have been played. A side with no plays is the
    /// interesting case and it is the *caller* that knows the tracklist, so
    /// zero-filling belongs there.
    pub async fn side_plays(
        &self,
        user_id: i64,
        release_id: i64,
    ) -> Result<Vec<SidePlays>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT side,
                    COUNT(*) AS plays,
                    COALESCE(SUM(duration_secs), 0) AS secs,
                    MAX(played_at) AS last_played
               FROM physical_plays
              WHERE user_id = ? AND release_id = ?
              GROUP BY side
              ORDER BY side"
        ))
        .bind(user_id)
        .bind(release_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Hours on a stylus or tape head, summed from the sides actually played
    /// through it.
    ///
    /// Derived rather than accumulated, for the same reason chain hours are:
    /// a counter that is incremented can drift from the events it claims to
    /// count, and there is no way to tell which is wrong afterwards.
    pub async fn equipment_playback_hours(
        &self,
        user_id: i64,
        equipment_id: i64,
    ) -> Result<(f64, i64), sqlx::Error> {
        let row: (Option<i64>, i64) = sqlx::query_as(&crate::sql!(
            "SELECT SUM(duration_secs), COUNT(*) FROM physical_plays
              WHERE user_id = ? AND equipment_id = ?"
        ))
        .bind(user_id)
        .bind(equipment_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((row.0.unwrap_or(0) as f64 / 3600.0, row.1))
    }
    /// Read an admin-set server setting, decrypted.
    /// The title of one track of a shelved record, by side and position.
    ///
    /// What turns the design's `A:3:12` coordinate into something a lyric
    /// lookup can use: the reference names a *place on a record*, and the
    /// tracklist is what knows which song is there. A record that is not on the
    /// shelf, or a side and position nothing sits at, resolves to nothing — and
    /// the reference then renders as readable prose, the way an unresolved
    /// `[[track:…]]` already does.
    pub async fn shelf_track_title(
        &self,
        user_id: i64,
        artist: &str,
        album: &str,
        side: &str,
        position: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(&crate::sql!(
            "SELECT tracks FROM physical_releases
              WHERE user_id = ? AND LOWER(TRIM(artist)) = ? AND LOWER(TRIM(title)) = ?
              ORDER BY id LIMIT 1"
        ))
        .bind(user_id)
        .bind(artist.trim().to_lowercase())
        .bind(album.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await?;
        let Some((json,)) = row else { return Ok(None) };
        let tracks: Vec<crate::metadata::release_lookup::PhysicalTrack> =
            serde_json::from_str(&json).unwrap_or_default();
        Ok(tracks
            .into_iter()
            .find(|t| t.side.eq_ignore_ascii_case(side.trim()) && t.position == position)
            .map(|t| t.title))
    }
    /// Stamp a pressing's own sleeve onto every listen it has ever produced.
    ///
    /// **A record has one cover, and the shelf is holding it.** A side written
    /// by `play_side` used to go in with no artwork at all, so
    /// `backfill_artwork` resolved each listen the way it resolves any other —
    /// by asking a provider about (artist, title, album) — and a provider
    /// answers per *song*. Eleven tracks off one side of *The Wall* came back
    /// with eleven different covers, several of them compilations the record
    /// has nothing to do with.
    ///
    /// That is the "your own library beats any provider" rule, in the one place
    /// where it is not even a preference: the object is on the shelf, the
    /// sleeve is the sleeve, and no lookup can be more right than that.
    ///
    /// Matched on the `source_id` prefix, which `play_side` builds
    /// deterministically as `physical-<release>-<side>-<start>-<position>`, so
    /// this reaches sides played before the cover was known as well as the one
    /// just recorded.
    pub async fn set_physical_artwork(
        &self,
        user_id: i64,
        release_id: i64,
        cover: &str,
    ) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET artwork_url = ?, caa_id = NULL, caa_release_mbid = NULL
              WHERE user_id = ? AND source_name = 'physical'
                AND source_id LIKE 'physical-' || ? || '-%'
                AND COALESCE(artwork_url, '') != ?",
        ))
        .bind(cover)
        .bind(user_id)
        .bind(release_id)
        .bind(cover)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }
    /// The sleeve a pressing should stamp on its listens: the user's own scan
    /// if they uploaded one, else whatever the lookup found.
    ///
    /// The same order `cover_thumb` uses on the shelf itself — an upload is a
    /// photograph of the copy actually in the room — so a listen and the record
    /// it came off cannot show different covers.
    pub async fn physical_cover(
        &self,
        user_id: i64,
        release_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(&crate::sql!(
            "SELECT COALESCE(
                      (SELECT COALESCE(i.thumb_url, i.url,
                                       '/api/v1/physical/images/' || i.id || '/file')
                         FROM physical_images i
                        WHERE i.release_id = r.id AND i.kind = 'front'
                        ORDER BY (i.file_path IS NULL), i.position, i.id
                        LIMIT 1),
                      r.artwork_url)
               FROM physical_releases r WHERE r.user_id = ? AND r.id = ?"
        ))
        .bind(user_id)
        .bind(release_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.0).filter(|s| !s.trim().is_empty()))
    }
    pub async fn get_app_setting(&self, key: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as(&crate::sql!("SELECT value FROM app_settings WHERE key = ?"))
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.and_then(|r| self.secrets.decrypt(&r.0).ok()).filter(|v| !v.trim().is_empty()))
    }
    /// Store (or clear) a server setting. Encrypted at rest like every other
    /// credential that has to be replayed.
    pub async fn set_app_setting(&self, key: &str, value: &str) -> Result<(), sqlx::Error> {
        if value.trim().is_empty() {
            sqlx::query(&crate::sql!("DELETE FROM app_settings WHERE key = ?"))
                .bind(key)
                .execute(&self.pool)
                .await?;
            return Ok(());
        }
        sqlx::query(&crate::sql!(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at"
        ))
        .bind(key)
        .bind(
            self.secrets
                .encrypt(value.trim())
                .map_err(|e| sqlx::Error::Protocol(format!("encrypting setting: {e}")))?,
        )
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Tracks recorded under two different artists on the same record.
    ///
    /// The shape this exists for is a film soundtrack: Plex files the album
    /// under the music director but tags each track with its singers, and until
    /// the ingest fix the live and history paths each picked a different one.
    /// The same play therefore landed twice under two names, and MusicBrainz
    /// then resolved those to two different recordings — so the plays, the
    /// artwork, the loves and the charts all split in half.
    ///
    /// The album artist wins, matched against the album's most common artist
    /// rather than assumed: it is how the record is filed, and picking the
    /// most-played spelling instead would hand the record to whichever singer
    /// happened to be on the tracks played most. Everything folded keeps its
    /// name in `artist_credit`, exactly as `fold_artist_credit` does.
    pub async fn tracks_with_split_attribution(
        &self,
        user_id: i64,
    ) -> Result<Vec<SplitAttribution>, sqlx::Error> {
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT title, album, artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
               AND album IS NOT NULL AND TRIM(album) != ''
             GROUP BY LOWER(TRIM(title)), LOWER(TRIM(album)), LOWER(TRIM(artist))"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        // The album's own artist is the one appearing on the most *distinct
        // tracks* of the record — not the one with the most plays.
        //
        // Breadth is the signal that separates a director from a singer: the
        // director is credited on every track, each singer on one or two. Play
        // count is not, and using it got this backwards on real data — on an
        // album where most tracks happened to be caught by the live path, one
        // singer's total outweighed the director's and the fold ran the wrong
        // way, moving "Harris Jayaraj" onto a singer. Total plays stays as the
        // tiebreak for artists of equal breadth.
        let mut album_tracks: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::new();
        let mut album_totals: HashMap<String, HashMap<String, i64>> = HashMap::new();
        for (title, album, artist, plays) in &rows {
            let key = album.trim().to_lowercase();
            album_tracks
                .entry(key.clone())
                .or_default()
                .entry(artist.clone())
                .or_default()
                .insert(title.trim().to_lowercase());
            *album_totals.entry(key).or_default().entry(artist.clone()).or_default() += plays;
        }

        let mut by_track: HashMap<(String, String), Vec<(String, i64)>> = HashMap::new();
        for (title, album, artist, plays) in rows {
            by_track
                .entry((title.trim().to_lowercase(), album.trim().to_lowercase()))
                .or_default()
                .push((artist, plays));
        }

        let mut out = Vec::new();
        for ((title_key, album_key), mut credits) in by_track {
            if credits.len() < 2 {
                continue;
            }
            let Some(totals) = album_totals.get(&album_key) else { continue };
            let breadth = |name: &str| -> usize {
                album_tracks.get(&album_key).and_then(|m| m.get(name)).map_or(0, |t| t.len())
            };
            // A compilation files *every* track under "Various Artists", so it
            // wins breadth on every record it appears on — and folding onto it
            // would replace real performers with a placeholder, which is the
            // exact damage this pass exists to undo. It can be folded *away
            // from*, never *onto*.
            if credits.iter().all(|(a, _)| is_various_artists(a)) {
                continue;
            }
            credits.retain(|(a, _)| !is_various_artists(a));
            if credits.len() < 2 {
                continue;
            }
            // Widest credit on the record first, then total plays, then plays
            // on this track.
            credits.sort_by(|a, b| {
                breadth(&b.0)
                    .cmp(&breadth(&a.0))
                    .then_with(|| {
                        totals.get(&b.0).unwrap_or(&0).cmp(totals.get(&a.0).unwrap_or(&0))
                    })
                    .then(b.1.cmp(&a.1))
            });
            let (primary, primary_plays) = credits[0].clone();
            let folding: Vec<(String, i64)> = credits[1..].to_vec();
            if folding.is_empty() {
                continue;
            }
            // Computed before the key is moved into the row.
            let primary_album_tracks = breadth(&primary) as i64;
            let primary_album_plays = *totals.get(&primary).unwrap_or(&0);
            out.push(SplitAttribution {
                title: title_key,
                album: album_key,
                primary_album_tracks,
                primary_album_plays,
                primary,
                primary_plays,
                folding,
            });
        }
        out.sort_by(|a, b| {
            let (x, y) = (
                b.folding.iter().map(|f| f.1).sum::<i64>(),
                a.folding.iter().map(|f| f.1).sum::<i64>(),
            );
            x.cmp(&y)
        });
        Ok(out)
    }
    /// Put one track's stray attribution back on the record's own artist.
    ///
    /// Matched case-insensitively on title+album+artist because that is how the
    /// preview grouped them. The folded name is kept in `artist_credit` — this
    /// is a credit being demoted, not a mistake being erased, and the singers
    /// are exactly the information the whole change exists to preserve.
    pub async fn merge_track_attribution(
        &self,
        user_id: i64,
        title: &str,
        album: &str,
        from_artist: &str,
        to_artist: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE scrobbles
                SET artist_credit = COALESCE(artist_credit, ? || '; ' || artist),
                    artist = ?,
                    mbid_recording = NULL,
                    artwork_url = NULL,
                    caa_id = NULL
              WHERE user_id = ?
                AND LOWER(TRIM(title)) = LOWER(TRIM(?))
                AND LOWER(TRIM(album)) = LOWER(TRIM(?))
                AND LOWER(TRIM(artist)) = LOWER(TRIM(?))"
        ))
        .bind(to_artist)
        .bind(to_artist)
        .bind(user_id)
        .bind(title)
        .bind(album)
        .bind(from_artist)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
    /// Merge one spelling of an artist into another.
    ///
    /// A plain rename, unlike `fold_artist_credit`: both strings name the same
    /// artist, so there is no lost credit to preserve in `artist_credit`.
    pub async fn rename_artist(
        &self,
        user_id: i64,
        from: &str,
        to: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET artist = ? WHERE user_id = ? AND LOWER(TRIM(artist)) = ?",
        ))
        .bind(to)
        .bind(user_id)
        .bind(from.trim().to_lowercase())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
    /// Rewrite one artist string to another across a user's listens.
    ///
    /// The full credit is kept in `artist_credit` so nothing is lost — the
    /// featured performer is still recorded, it just stops fragmenting every
    /// grouping query.
    pub async fn fold_artist_credit(
        &self,
        user_id: i64,
        from: &str,
        to: &str,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(&crate::sql!(
            "UPDATE scrobbles
                SET artist_credit = COALESCE(artist_credit, artist), artist = ?
              WHERE user_id = ? AND LOWER(TRIM(artist)) = ?"
        ))
        .bind(to)
        .bind(user_id)
        .bind(from.trim().to_lowercase())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
    /// (total listens, newest listened_at) for a user — the two numbers the
    /// ListenBrainz-compatible endpoints report alongside a listen page.
    pub async fn listen_count_and_latest(
        &self,
        user_id: i64,
    ) -> Result<(i64, Option<i64>), sqlx::Error> {
        let row: (i64, Option<i64>) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*), MAX(timestamp) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }
    /// Every listen a report needs, oldest first.
    ///
    /// One query and one pass, deliberately. A report card wants a dozen
    /// different aggregates per period — counts, distincts, top artist, an
    /// hour histogram, genre shares, repeat days — and the periods themselves
    /// depend on the user's timezone, which SQL here cannot bucket by. Doing it
    /// as a dozen queries per period would be hundreds of round trips for a
    /// year of months; doing it in Rust from one ordered scan is a few
    /// milliseconds.
    ///
    /// **Ascending**, unlike every other listing query, because "the first time
    /// you played this artist" is what makes a discovery a discovery — walking
    /// forwards means first-seen is simply the first time a name appears.
    pub async fn report_rows(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<ReportRow>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT timestamp, artist, artist_credit, title, album, duration, genres, chain_id,
                    company
             FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE
             ORDER BY timestamp ASC LIMIT ?",
        ))
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Every listen the Playlist Lab needs to build its vector space.
    ///
    /// One scan, folded per track in Rust, for the same reason `report_rows`
    /// is: the hour-of-day axis has to be bucketed in the **user's** timezone
    /// and SQL here cannot do that, and the genre list is a JSON array that has
    /// to be joined to the Every Noise map through `genre_key` — which SQLite
    /// cannot compute either. Grouping in SQL would mean giving up both.
    ///
    /// `skipped = FALSE` like every other counting query: a skip says nothing
    /// about whether you want a track on a playlist.
    pub async fn playlist_lab_rows(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<crate::playlist_lab::LabRow>, sqlx::Error> {
        let rows: Vec<(i64, String, String, Option<String>, Option<String>, Option<i64>)> =
            sqlx::query_as(&crate::sql!(
                "SELECT timestamp, artist, title, album, genres, release_year
                   FROM scrobbles
                  WHERE user_id = ? AND skipped = FALSE
                  ORDER BY timestamp DESC LIMIT ?"
            ))
            .bind(user_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|(timestamp, artist, title, album, genres, release_year)| {
                crate::playlist_lab::LabRow {
                    timestamp,
                    artist,
                    title,
                    album,
                    genres,
                    release_year,
                }
            })
            .collect())
    }
    /// Sides played by hand, for the "off the shelf" block.
    pub async fn physical_plays_in(
        &self,
        user_id: i64,
        from: i64,
        to: i64,
    ) -> Result<Vec<(String, String, String, i64)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT r.title, r.artist, p.side, COUNT(*) AS spins
             FROM physical_plays p JOIN physical_releases r ON r.id = p.release_id
             WHERE p.user_id = ? AND p.played_at >= ? AND p.played_at < ?
             GROUP BY r.id, p.side ORDER BY spins DESC, r.title"
        ))
        .bind(user_id)
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await
    }
    /// Every side ever put on, oldest first — what the shelf export carries.
    ///
    /// Ascending because an export is read as a log: a shelf's history is the
    /// order things were played in, and reversing it to match the history
    /// screen's newest-first habit would be the wrong shape for a file someone
    /// keeps.
    pub async fn all_physical_plays(
        &self,
        user_id: i64,
    ) -> Result<Vec<PhysicalPlayRow>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT p.id, p.release_id, r.artist, r.title, r.kind, p.side, p.played_at,
                    p.duration_secs, p.track_count,
                    e.name AS equipment_name, c.name AS chain_name
               FROM physical_plays p
               JOIN physical_releases r ON r.id = p.release_id
               LEFT JOIN equipment e ON e.id = p.equipment_id
               LEFT JOIN signal_chains c ON c.id = p.chain_id
              WHERE p.user_id = ?
              ORDER BY p.played_at ASC, p.id ASC",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Every scan the user holds, in release then page order.
    pub async fn all_physical_images(
        &self,
        user_id: i64,
    ) -> Result<Vec<PhysicalImage>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT * FROM physical_images WHERE user_id = ?
              ORDER BY release_id ASC, position ASC, id ASC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Minimal columns for analytics: (timestamp, artist, title, album, track_number).
    /// Capped to the most-recent `limit` rows to bound memory on large histories.
    pub async fn get_listen_rows(
        &self,
        user_id: i64,
        after: Option<i64>,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, Option<String>, Option<i32>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT timestamp, artist, title, album, track_number FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
             ORDER BY timestamp DESC LIMIT ?"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Listens still needing a language, newest first. (id, title, album, mbid_recording)
    pub async fn scrobbles_needing_language(
        &self,
        limit: i64,
    ) -> Result<Vec<(i64, String, Option<String>, Option<String>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT id, title, album, mbid_recording FROM scrobbles
             WHERE language IS NULL AND skipped = FALSE ORDER BY timestamp DESC LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Tracks whose language is unresolved, biggest first — the queue the
    /// lyrics pass works through.
    ///
    /// **Grouped by track rather than by listen**, which is the whole reason
    /// this is a separate query from `scrobbles_needing_language`. A lyric is a
    /// property of the song, so a track played fifty times is *one*
    /// rate-limited request; asking per listen would spend the budget fifty
    /// times over on an answer that cannot differ.
    ///
    /// "Unresolved" includes `DEFAULT_LANGUAGE`, which is the point: that value
    /// is a shrug rather than a measurement, and it is 45% of a real window.
    /// A row that already carries a real language is left alone.
    ///
    /// The duration is the **mode** rather than the max — LRCLIB matches within
    /// ±2s, so the length most of the plays agree on is the one most likely to
    /// find the record, where a single mis-tagged listen could push a max out
    /// of range.
    /// The `WHERE` the language queue is made of. One place, because the count
    /// and the listing must describe the same population — a progress readout
    /// that cannot reach the end of its own queue is the paired-counts trap.
    pub(super) fn language_queue_predicate() -> String {
        format!(
            "skipped = FALSE
               AND (language IS NULL OR language = '{}')
               AND TRIM(title) != '' AND TRIM(artist) != ''
               AND LOWER(TRIM(artist)) {NOT_VARIOUS}",
            crate::language::DEFAULT_LANGUAGE
        )
    }
    /// How many distinct tracks are still waiting on a language.
    ///
    /// A `COUNT` rather than `tracks_wanting_language(…).len()`, which is what
    /// the status endpoint used to do — it fetched a hundred thousand rows,
    /// with their albums and durations, to look at the length of the vector.
    pub async fn count_tracks_wanting_language(&self) -> Result<i64, sqlx::Error> {
        let (n,): (i64,) = sqlx::query_as(&crate::sql!(&format!(
            "SELECT COUNT(*) FROM (
               SELECT 1 FROM scrobbles WHERE {}
                GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist)))",
            Self::language_queue_predicate()
        )))
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }
    /// Tracks still wanting a language, most-played first, with the running
    /// time LRCLIB should be asked to match on.
    ///
    /// **Two bounded scans folded in Rust, never a lookup per row.** This was
    /// one statement whose duration came from a correlated subquery joined on
    /// `LOWER(TRIM(d.title)) = LOWER(TRIM(s.title))` — a join key that is a
    /// *function* of a column, so no index can serve it and SQLite scans the
    /// whole history once per candidate track. `ORDER BY plays DESC` then means
    /// every group is computed before the `LIMIT` applies, so there is no early
    /// exit either. Measured on the real instance: **357s average, 753s worst**,
    /// with five running at once against a pool of five — the dashboard, session
    /// validation and the engine all queued behind it for two days.
    ///
    /// Same shape as `loved_but_unplayed` (v0.56.0) and the same fix as
    /// `list_crate` and `playlist_tracks_detailed`: ask for the candidates,
    /// then ask one bounded question about them.
    ///
    /// **The grouping key is computed by SQLite in both passes and never
    /// reproduced here.** SQLite's `LOWER` folds ASCII only, so a Rust
    /// `to_lowercase` would disagree with it on every accented title — two
    /// normalisers, silently losing the duration for exactly the non-Latin
    /// catalogue this queue exists to serve. The key is an opaque string that
    /// came from the database, matched against itself.
    pub async fn tracks_wanting_language(
        &self,
        limit: i64,
    ) -> Result<Vec<LanguageCandidate>, sqlx::Error> {
        // Pass one: the candidates. One scan, no subquery.
        let rows: Vec<(String, String, String, Option<String>, i64)> =
            sqlx::query_as(&crate::sql!(&format!(
                "SELECT LOWER(TRIM(title)) || CHAR(31) || LOWER(TRIM(artist)) AS k,
                    title, artist, MAX(album) AS album, COUNT(*) AS plays
               FROM scrobbles
              WHERE {}
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
              ORDER BY plays DESC LIMIT ?",
                Self::language_queue_predicate()
            )))
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }

        // Pass two: the modal running time of just those tracks. Chunked so a
        // large limit cannot walk into SQLite's bound-parameter ceiling.
        let mut best: HashMap<String, (i64, i64)> = HashMap::new();
        for chunk in rows.chunks(DURATION_KEY_CHUNK) {
            let keys: Vec<&str> = chunk.iter().map(|r| r.0.as_str()).collect();
            let placeholders = vec!["?"; keys.len()].join(",");
            let sql = format!(
                "SELECT LOWER(TRIM(title)) || CHAR(31) || LOWER(TRIM(artist)) AS k,
                        duration, COUNT(*) AS n
                   FROM scrobbles
                  WHERE duration IS NOT NULL AND skipped = FALSE
                    AND LOWER(TRIM(title)) || CHAR(31) || LOWER(TRIM(artist))
                        IN ({placeholders})
                  GROUP BY k, duration"
            );
            let stmt = crate::sql!(&sql);
            let mut q = sqlx::query_as::<_, (String, i64, i64)>(&stmt);
            for k in keys {
                q = q.bind(k);
            }
            for (k, duration, n) in q.fetch_all(&self.pool).await? {
                // The mode, which is the statistic the Spotify import already
                // argues for: seeking forward shortens a play and seeking back
                // lengthens it, so the distribution is a spike with thin tails
                // and the median or the max would both sit off it. Ties break
                // on the shorter time rather than arbitrarily — the old
                // subquery left that to SQLite, so the same history could
                // answer differently between two runs.
                let e = best.entry(k).or_insert((n, duration));
                if n > e.0 || (n == e.0 && duration < e.1) {
                    *e = (n, duration);
                }
            }
        }

        Ok(rows
            .into_iter()
            .map(|(k, title, artist, album, plays)| LanguageCandidate {
                duration: best.get(&k).map(|(_, d)| *d),
                key: k,
                title,
                artist,
                album,
                plays,
            })
            .collect())
    }
    /// Where one track sits in a library, by name.
    ///
    /// The batch version above is what the backfill uses. This is for the
    /// reader opening on a track nothing is cached for — one query on a
    /// deliberate press, never per row.
    ///
    /// **Both sides are folded by SQLite**, so no Rust `to_lowercase` gets to
    /// disagree with its ASCII-only `LOWER` on an accented title.
    ///
    /// Deliberately no `ORDER BY`: it would force the whole scan, where
    /// `LIMIT 1` alone lets SQLite stop at the first match. Any listen of the
    /// track gives a usable item id, and if that one has since been removed
    /// from the library the fetch simply finds nothing — which is the same
    /// answer as the track being untagged.
    pub async fn library_ref_for_track(
        &self,
        title: &str,
        artist: &str,
    ) -> Result<Option<LibraryRef>, sqlx::Error> {
        sqlx::query_as::<_, (i64, i64, String, String, i64)>(&crate::sql!(
            "SELECT user_id, source_ref, source_id, source_name, timestamp
               FROM scrobbles
              WHERE source_ref IS NOT NULL
                AND LOWER(source_name) IN ('plex', 'jellyfin', 'emby')
                AND LOWER(TRIM(title)) = LOWER(TRIM(?))
                AND LOWER(TRIM(artist)) = LOWER(TRIM(?))
              LIMIT 1",
        ))
        .bind(title)
        .bind(artist)
        .fetch_optional(&self.pool)
        .await
        .map(|o| {
            o.map(|(user_id, source_ref, source_id, source_name, timestamp)| LibraryRef {
                user_id,
                source_ref,
                source_id,
                source_name,
                timestamp,
            })
        })
    }
    /// Where a track can be found in the user's own libraries, for the
    /// candidates in hand.
    ///
    /// **Keyed on the string SQLite computed**, never one folded again in Rust:
    /// `LOWER` here is ASCII-only, so a Rust `to_lowercase` disagrees with it on
    /// every accented title — and would silently miss exactly the non-Latin
    /// catalogue this queue exists to serve. `LanguageCandidate::key` came out
    /// of the query above, and goes straight back in.
    ///
    /// One scan for the whole batch, folded in Rust — never a lookup per track,
    /// which on a `LOWER(TRIM(…))` predicate is a full scan each and is the
    /// shape that took this instance down.
    ///
    /// The newest listen wins: a track re-ripped or re-tagged is most likely to
    /// be found under the id it carried last.
    pub async fn library_refs_for_tracks(
        &self,
        keys: &[String],
    ) -> Result<HashMap<String, LibraryRef>, sqlx::Error> {
        let mut out: HashMap<String, LibraryRef> = HashMap::new();
        if keys.is_empty() {
            return Ok(out);
        }
        for chunk in keys.chunks(DURATION_KEY_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT LOWER(TRIM(title)) || CHAR(31) || LOWER(TRIM(artist)) AS k,
                        user_id, source_ref, source_id, source_name, timestamp
                   FROM scrobbles
                  WHERE source_ref IS NOT NULL
                    AND LOWER(source_name) IN ('plex', 'jellyfin', 'emby')
                    AND LOWER(TRIM(title)) || CHAR(31) || LOWER(TRIM(artist))
                        IN ({placeholders})"
            );
            let stmt = crate::sql!(&sql);
            let mut q = sqlx::query_as::<_, (String, i64, i64, String, String, i64)>(&stmt);
            for k in chunk {
                q = q.bind(k);
            }
            for (k, user_id, source_ref, source_id, source_name, ts) in
                q.fetch_all(&self.pool).await?
            {
                let e = out.entry(k).or_insert(LibraryRef {
                    user_id,
                    source_ref,
                    source_id: source_id.clone(),
                    source_name: source_name.clone(),
                    timestamp: ts,
                });
                if ts > e.timestamp {
                    *e = LibraryRef { user_id, source_ref, source_id, source_name, timestamp: ts };
                }
            }
        }
        Ok(out)
    }
    /// Write a language onto every listen of one track, saying where it came
    /// from.
    ///
    /// `source` is not decoration. A language read off a lyric's script and one
    /// taken from MusicBrainz's work record are **different measurements** of
    /// the same thing, and a breakdown that mixed them without saying so would
    /// be reporting two populations as one — the trap the fidelity cards
    /// already cost this codebase once.
    pub async fn set_track_language(
        &self,
        artist: &str,
        title: &str,
        language: &str,
        source: &str,
    ) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET language = ?, language_source = ?
              WHERE LOWER(TRIM(artist)) = ? AND LOWER(TRIM(title)) = ? AND skipped = FALSE"
        ))
        .bind(language)
        .bind(source)
        .bind(artist.trim().to_lowercase())
        .bind(title.trim().to_lowercase())
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }
    /// `source` says which of the three ways this was decided — see
    /// `set_track_language`.
    pub async fn set_scrobble_language(
        &self,
        id: i64,
        language: &str,
        source: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET language = ?, language_source = ? WHERE id = ?"
        ))
        .bind(language)
        .bind(source)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Every genre the user has listened to, with play counts.
    ///
    /// One listen carries several tags, so the counts sum to more than the
    /// listen count — each tag is counted once per listen that carries it,
    /// which is what makes a weighted position meaningful.
    pub async fn genre_play_counts(
        &self,
        user_id: i64,
        after: Option<i64>,
    ) -> Result<Vec<(String, i64)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT g.value AS name, COUNT(*) AS plays
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE AND s.genres IS NOT NULL
                AND (? IS NULL OR s.timestamp >= ?)
              GROUP BY LOWER(g.value) ORDER BY plays DESC"
        ))
        .bind(user_id)
        .bind(after)
        .bind(after)
        .fetch_all(&self.pool)
        .await
    }
    /// `(genre, artist, plays)` for everything the user has listened to.
    ///
    /// The join to the genre map has to happen in Rust because matching is on
    /// `genre_key`, which SQLite cannot compute — so this hands over the raw
    /// triples. Bounded by distinct (artist, tag) pairs, a few thousand rows on
    /// a large history.
    ///
    /// It exists so "genres next door" can name music the user **already
    /// owns**. Pointing at anything else would make this a shopping list, which
    /// is explicitly not what Tapedeck is for.
    pub async fn artists_by_genre(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String, i64)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT g.value AS genre, s.artist, COUNT(*) AS plays
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE AND s.genres IS NOT NULL
              GROUP BY LOWER(g.value), s.artist
              ORDER BY plays DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Genre plays bucketed by year, for the listening trajectory.
    ///
    /// The year is computed in the caller's timezone rather than UTC — a listen
    /// at 00:30 on 1 January in CET belongs to the new year, and bucketing it
    /// in UTC would put it in the old one. Same reasoning as the dashboard's
    /// "today", which had exactly this bug.
    ///
    /// Returns `(period, genre, plays)`; the caller turns that into a position.
    /// `fmt` is a `strftime` pattern — `%Y` for years, `%Y-%m` for months.
    pub async fn genre_plays_by_period(
        &self,
        user_id: i64,
        tz_offset_hours: f64,
        fmt: &str,
    ) -> Result<Vec<(String, String, i64)>, sqlx::Error> {
        let shift = format!("{} hours", tz_offset_hours);
        sqlx::query_as(&crate::sql!(
            "SELECT strftime(?, s.timestamp, 'unixepoch', ?) AS yr, g.value AS name,
                    COUNT(*) AS plays
               FROM scrobbles s, json_each(s.genres) g
              WHERE s.user_id = ? AND s.skipped = FALSE AND s.genres IS NOT NULL
              GROUP BY yr, LOWER(g.value)
              ORDER BY yr",
        ))
        .bind(fmt)
        .bind(&shift)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Listens per year, and how many of them have genres resolved — so a
    /// trajectory can say which years are still filling in rather than drawing
    /// a confident line through a half-enriched history.
    pub async fn listens_by_period(
        &self,
        user_id: i64,
        tz_offset_hours: f64,
        fmt: &str,
    ) -> Result<Vec<(String, i64, i64)>, sqlx::Error> {
        let shift = format!("{} hours", tz_offset_hours);
        sqlx::query_as(&crate::sql!(
            "SELECT strftime(?, timestamp, 'unixepoch', ?) AS yr,
                    COUNT(*) AS listens,
                    SUM(CASE WHEN genres IS NOT NULL THEN 1 ELSE 0 END) AS enriched
               FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE
              GROUP BY yr ORDER BY yr"
        ))
        .bind(fmt)
        .bind(&shift)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// How many of the user's listens carry any genre at all, and how many
    /// exist — so a thin map can say "the backfill is still running" rather
    /// than looking like a small life.
    pub async fn genre_backfill_progress(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let done: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND genres IS NOT NULL"
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let total: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((done.0, total.0))
    }
    /// Listens with no genres resolved yet.
    ///
    /// `genres = '[]'` is the tombstone for "asked, nobody had any" — the same
    /// discipline as `artwork_url = ''` and `caa_id = -1`, and the reason this
    /// tests `IS NULL` rather than emptiness.
    pub async fn scrobbles_needing_genres(
        &self,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, Option<String>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT id, title, artist, album FROM scrobbles
             WHERE genres IS NULL AND skipped = FALSE ORDER BY timestamp DESC LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Store a listen's genres. An empty slice writes `[]`, which is the
    /// tombstone for "asked, MusicBrainz had no tags".
    pub async fn set_scrobble_genres(&self, id: i64, genres: &[String]) -> Result<(), sqlx::Error> {
        // A JSON array rather than a delimited string, so the breakdown can
        // `json_each` over it — the same tool `get_equipment` uses to sum hours
        // across a chain's components. A comma-joined string would need a
        // recursive CTE to split, for no gain.
        let json = serde_json::to_string(genres).unwrap_or_else(|_| "[]".into());
        sqlx::query(&crate::sql!("UPDATE scrobbles SET genres = ? WHERE id = ?"))
            .bind(&json)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Listens with an MBID but no resolved release year yet. Rows without an
    /// MBID are skipped: there's nothing to look them up with.
    /// The ISRCs the history holds for a set of tracks, keyed by folded
    /// title and artist.
    ///
    /// **One bounded scan, never a lookup per track.** A correlated subquery
    /// keyed on `LOWER(TRIM(...))` cannot use an index, so it is a full pass per
    /// row by construction — the quadratic that made Rediscovery take two
    /// minutes on the real host. The tracks' own artist keys become an `IN`
    /// filter instead, so the history is walked once and returns only rows that
    /// could match. Same shape as `list_crate` and `playlist_tracks_detailed`.
    ///
    /// The tombstone is excluded: `isrc = ''` means "asked, and this recording
    /// has none", which is not an ISRC to hand a streaming service.
    pub async fn isrc_map(
        &self,
        user_id: i64,
        tracks: &[(String, String)],
    ) -> Result<HashMap<(String, String), String>, sqlx::Error> {
        if tracks.is_empty() {
            return Ok(HashMap::new());
        }
        let keys: Vec<String> = tracks
            .iter()
            .map(|(_, artist)| Self::artist_key(artist))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let placeholders = vec!["?"; keys.len()].join(",");
        let sql = format!(
            "SELECT LOWER(TRIM(title)), LOWER(TRIM(artist)), isrc
               FROM scrobbles
              WHERE user_id = ? AND isrc IS NOT NULL AND isrc != ''
                AND LOWER(TRIM(artist)) IN ({placeholders})
              GROUP BY 1, 2"
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, (String, String, String)>(&stmt).bind(user_id);
        for k in &keys {
            q = q.bind(k);
        }
        Ok(q.fetch_all(&self.pool).await?.into_iter().map(|(t, a, i)| ((t, a), i)).collect())
    }
    /// Distinct tracks with no recording MBID, newest first.
    ///
    /// **Why this queue exists at all.** A recording MBID is written in exactly
    /// one place — `pipeline::flush_pending`, which processes `status =
    /// 'pending'`. An **imported** listen is `status = 'imported'` and never
    /// becomes pending, so it never reached `enrich_play` and never got one.
    /// Nothing else in this file writes the column. On a history that is 92%
    /// imported that left 57% of listens with no MBID *permanently*, and with
    /// it no ISRC, no work-language and no release-year lookup — every backfill
    /// that keys off a recording was quietly excluded from most of the history.
    ///
    /// Grouped by track, because a song played fifty times is fifty rows and
    /// one answer. `album` comes along as a hint for the search, taken from one
    /// of those rows: the same thing `flush_pending` passes.
    ///
    /// **Already-asked tracks are filtered by the caller, not here.** The
    /// negative cache belongs to `MusicBrainzClient` and lives in a *different
    /// database file* (`SQLITE_DB_PATH`, not `DATABASE_URL`), so a join to it
    /// from this file is not merely slow — the table is not there. That is why
    /// this over-fetches and `backfill_recording_mbids` does the filtering.
    pub async fn tracks_needing_recording_mbid(
        &self,
        limit: i64,
    ) -> Result<Vec<(String, String, Option<String>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT title, artist, MAX(album) FROM scrobbles
              WHERE mbid_recording IS NULL AND skipped = FALSE
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
              ORDER BY MAX(timestamp) DESC
              LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// The distinct tracks this user has no recording MBID for.
    ///
    /// Folded keys, so the caller can intersect them with the MusicBrainz
    /// client's negative cache — which is in another database and can only be
    /// joined in Rust.
    pub async fn unresolved_track_keys(
        &self,
        user_id: i64,
    ) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT LOWER(TRIM(title)), LOWER(TRIM(artist)) FROM scrobbles
              WHERE user_id = ? AND mbid_recording IS NULL AND skipped = FALSE
              GROUP BY 1, 2"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Write a resolved recording onto every listen of that track.
    ///
    /// Keyed on the folded name pair rather than on ids, because that is what
    /// the lookup was made with — and it means one rate-limited request settles
    /// all fifty plays at once. Only fills columns that are **empty**: a listen
    /// that already carried an MBID from an import's `mbid_mapping` knows
    /// better than a name search does.
    pub async fn set_recording_mbid_for_track(
        &self,
        title: &str,
        artist: &str,
        play: &Play,
    ) -> Result<u64, sqlx::Error> {
        let artists_json =
            play.mbid_artist.as_ref().map(|a| serde_json::to_string(a).unwrap_or_default());
        let r = sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET
                 mbid_recording   = COALESCE(mbid_recording, ?),
                 mbid_release     = COALESCE(mbid_release, ?),
                 mbid_artist      = COALESCE(mbid_artist, ?),
                 caa_id           = COALESCE(caa_id, ?),
                 caa_release_mbid = COALESCE(caa_release_mbid, ?)
              WHERE mbid_recording IS NULL
                AND LOWER(TRIM(title)) = ? AND LOWER(TRIM(artist)) = ?"
        ))
        .bind(&play.mbid_recording)
        .bind(&play.mbid_release)
        .bind(&artists_json)
        .bind(play.caa_id)
        .bind(&play.caa_release_mbid)
        .bind(title.trim().to_lowercase())
        .bind(artist.trim().to_lowercase())
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }
    /// How much of a history MusicBrainz has identified at recording level.
    ///
    /// Two numbers here. The third the readout wants — how many of the rest
    /// have already been asked about and come back empty — needs the client's
    /// negative cache, which is in a different database file, so it is folded
    /// in the handler from `unresolved_track_keys` and
    /// `MusicBrainzClient::negative_track_keys`.
    pub async fn recording_mbid_coverage(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN mbid_recording IS NOT NULL
                                       AND TRIM(mbid_recording) != ''
                                      THEN 1 ELSE 0 END), 0)
               FROM scrobbles WHERE user_id = ? AND skipped = FALSE"
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
    }
    /// Listens with no ISRC yet.
    ///
    /// `''` is the tombstone — "asked MusicBrainz, this recording has none" —
    /// so a track nobody has an ISRC for is not re-asked forever. Same
    /// discipline as `artwork_url = ''`, `caa_id = -1` and the language pass.
    ///
    /// **Grouped by recording**, because a track played fifty times is fifty
    /// rows sharing one recording and one ISRC — asking per row would spend
    /// fifty rate-limited lookups on one answer. Newest first, since the tracks
    /// somebody is likely to put on a playlist this week are the ones they have
    /// been playing.
    ///
    /// Only listens MusicBrainz has already identified. Without a recording
    /// MBID there is nothing to look up, and a name search is what the other
    /// backfills already do — this pass collects them once they have one.
    pub async fn scrobbles_needing_isrc(
        &self,
        limit: i64,
    ) -> Result<Vec<(String, String, String)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT mbid_recording, title, artist FROM scrobbles
              WHERE isrc IS NULL AND skipped = FALSE
                AND mbid_recording IS NOT NULL AND TRIM(mbid_recording) != ''
              GROUP BY mbid_recording
              ORDER BY MAX(timestamp) DESC
              LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Write an ISRC onto every listen of the same recording, not just the row
    /// that prompted the lookup.
    ///
    /// A track played fifty times is fifty rows, and they all share one
    /// recording — so resolving it once and writing it once would leave
    /// forty-nine of them queued for a rate-limited lookup that can only return
    /// the same answer. Matched on the folded title and artist, which is how
    /// every other grouping in this file decides two rows are the same track.
    pub async fn set_scrobble_isrc(
        &self,
        title: &str,
        artist: &str,
        isrc: &str,
    ) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET isrc = ?
              WHERE isrc IS NULL
                AND LOWER(TRIM(title)) = LOWER(TRIM(?))
                AND LOWER(TRIM(artist)) = LOWER(TRIM(?))"
        ))
        .bind(isrc)
        .bind(title)
        .bind(artist)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }
    pub async fn scrobbles_needing_release_year(
        &self,
        limit: i64,
    ) -> Result<Vec<(i64, String, String, Option<String>)>, sqlx::Error> {
        sqlx::query_as(&crate::sql!(
            "SELECT id, title, artist, album FROM scrobbles
             WHERE release_year IS NULL AND skipped = FALSE ORDER BY timestamp DESC LIMIT ?"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// `0` records "looked up, no date known" so the row isn't retried forever.
    ///
    /// Deliberately does **not** touch `caa_id`. It used to, on the reasoning
    /// that the same MusicBrainz lookup returned artwork anyway — but that
    /// silently pre-empted the artwork provider chain: setting `caa_id` removes
    /// the row from `scrobbles_needing_artwork`, so Deezer never got asked and
    /// the row ended up with a Cover Art Archive id but no `artwork_url`.
    /// Artwork is `backfill_artwork`'s job alone.
    pub async fn set_scrobble_release_year(&self, id: i64, year: i32) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!("UPDATE scrobbles SET release_year = ? WHERE id = ?"))
            .bind(year)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// (total, with cover art) — drives the Settings artwork progress readout.
    pub async fn artwork_coverage(&self, user_id: i64) -> Result<(i64, i64), sqlx::Error> {
        let total: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE",
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        // Must match the total's population exactly. It didn't: the total
        // excluded skips and this didn't, so a history with skipped listens
        // that happened to have artwork reported more resolved than existed.
        let with_art: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND artwork_url IS NOT NULL"
        ))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((total.0, with_art.0))
    }
    /// The artwork queue: listens whose artwork has never been resolved.
    ///
    /// Keyed purely on `artwork_url` being NULL. It deliberately does *not* look
    /// at `caa_id`, which several other paths write (ingest `mbid_mapping`,
    /// flush-time enrichment) — using it here meant a row that had merely been
    /// through MusicBrainz was treated as "artwork done" and never got a URL.
    /// `artwork_url = ''` is the tombstone for "looked, found nothing".
    ///
    /// Returns any Cover Art Archive ids already on the row so the backfill can
    /// build a URL from them for free rather than looking anything up.
    #[allow(clippy::type_complexity)]
    pub async fn scrobbles_needing_artwork(
        &self,
        limit: i64,
    ) -> Result<
        Vec<(i64, String, String, Option<String>, Option<i64>, Option<String>, String)>,
        sqlx::Error,
    > {
        sqlx::query_as(&crate::sql!(
            "SELECT id, title, artist, album, caa_id, caa_release_mbid, source_name
             FROM scrobbles
             WHERE artwork_url IS NULL AND skipped = FALSE
             ORDER BY timestamp DESC LIMIT ?",
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Record artwork for a listen. Pass `""` to tombstone a row whose artwork
    /// could not be found anywhere, so later passes skip it instead of re-asking
    /// every provider forever. `caa_id` is only written when a lookup produced
    /// one; it is provenance, not queue state.
    pub async fn set_scrobble_artwork(
        &self,
        id: i64,
        url: &str,
        caa_id: Option<i64>,
        caa_release_mbid: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET artwork_url = ?,
                caa_id = COALESCE(?, caa_id),
                caa_release_mbid = COALESCE(?, caa_release_mbid)
             WHERE id = ?"
        ))
        .bind(url)
        .bind(caa_id)
        .bind(caa_release_mbid)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// (total, with a language set) — for backfill progress.
    ///
    /// Both halves filter `skipped = FALSE` to match `scrobbles_needing_language`,
    /// which does. Counting rows the backfill will never process put them
    /// permanently in the denominator, so the progress readout could not reach
    /// 100% on any history containing a skip.
    pub async fn language_coverage(&self) -> Result<(i64, i64), sqlx::Error> {
        let total: (i64,) =
            sqlx::query_as(&crate::sql!("SELECT COUNT(*) FROM scrobbles WHERE skipped = FALSE"))
                .fetch_one(&self.pool)
                .await?;
        let done: (i64,) = sqlx::query_as(&crate::sql!(
            "SELECT COUNT(*) FROM scrobbles WHERE skipped = FALSE AND language IS NOT NULL",
        ))
        .fetch_one(&self.pool)
        .await?;
        Ok((total.0, done.0))
    }
    /// Dashboard counters, bounded in the *user's* local time.
    ///
    /// `tz` and `week_start_day` come from their preferences. Both used to be
    /// implicit and both were wrong: "today" was midnight UTC, so the day
    /// rolled over at 01:00 in CET and 02:00 in CEST, and "this week" was a
    /// rolling 7×86400 window rather than a calendar week starting anywhere in
    /// particular.
    ///
    /// The fidelity shares divide by [`FIDELITY_KNOWN`], not by every play —
    /// see that constant for why.
    pub async fn get_dashboard_stats(
        &self,
        user_id: i64,
        tz: chrono_tz::Tz,
        week_start_day: u32,
    ) -> Result<serde_json::Value, sqlx::Error> {
        let now = Self::now_secs();
        let today_start = crate::analytics::local_day_start(now, tz);
        let week_start = crate::analytics::local_week_start(now, tz, week_start_day);

        // ── One scan of the history, not seven ────────────────────────────
        //
        // This is the most-loaded page in the application, and it used to issue
        // **thirteen** queries of which **seven** scanned the user's whole
        // history: the total, the two fidelity counts, the running time, the
        // quality average, and three `COUNT(DISTINCT)`s that each built a temp
        // B-tree over every row — one of them concatenating two strings per row
        // to do it. On a Pi with the database on an SD card that is the exact
        // profile of the incidents in v0.58.0 and v0.70.1.
        //
        // Conditional aggregates give the same numbers from one pass. Each
        // preserves its predicate exactly: `SUM(CASE WHEN … THEN 1 ELSE 0 END)`
        // matches `COUNT(*) … WHERE`, and `SUM`/`AVG` already ignore NULLs, so
        // dropping the `IS NOT NULL` filters changes nothing.
        //
        // Over **zero rows every `SUM` and `AVG` is NULL**, including the ones
        // standing in for counts — hence the `Option`s. A fresh account reading
        // `null` where it used to read `0` is the one way this could regress.
        let whole: (i64, Option<i64>, Option<i64>, Option<i64>, Option<f64>) =
            sqlx::query_as(&crate::sql!(&format!(
                "SELECT COUNT(*),
                        SUM(CASE WHEN is_lossless = TRUE THEN 1 ELSE 0 END),
                        SUM(CASE WHEN {FIDELITY_KNOWN} THEN 1 ELSE 0 END),
                        SUM(duration),
                        AVG(quality_score)
                   FROM scrobbles WHERE user_id = ? AND skipped = FALSE"
            )))
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?;
        let (total, lossless_count, fidelity_known, total_duration, avg_quality) =
            (whole.0, whole.1.unwrap_or(0), whole.2.unwrap_or(0), whole.3.unwrap_or(0), whole.4);

        // Today and this week from one indexed range rather than four. Today is
        // inside this week, but the floor is the smaller of the two anyway —
        // the alternative is a silent hole on whichever boundary is wrong.
        let floor = week_start.min(today_start);
        let recent: (Option<i64>, Option<i64>, Option<i64>, Option<i64>) =
            sqlx::query_as(&crate::sql!(
                "SELECT SUM(CASE WHEN timestamp >= ?2 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN timestamp >= ?3 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN timestamp >= ?2 THEN duration END),
                    SUM(CASE WHEN timestamp >= ?3 THEN duration END)
               FROM scrobbles
              WHERE user_id = ?1 AND skipped = FALSE AND timestamp >= ?4"
            ))
            .bind(user_id)
            .bind(today_start)
            .bind(week_start)
            .bind(floor)
            .fetch_one(&self.pool)
            .await?;
        let today = recent.0.unwrap_or(0);
        let week = recent.1.unwrap_or(0);
        let today_hours = recent.2.unwrap_or(0) as f64 / 3600.0;
        let week_hours = recent.3.unwrap_or(0) as f64 / 3600.0;

        let lossless_pct = if fidelity_known > 0 {
            (lossless_count as f64 / fidelity_known as f64 * 100.0).round() as i64
        } else {
            0
        };
        let top_artist: Option<(String, i64)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT artist, COUNT(*) as cnt FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND timestamp >= ?
                AND LOWER(TRIM(artist)) {NOT_VARIOUS}
              GROUP BY artist ORDER BY cnt DESC LIMIT 1"
        )))
        .bind(user_id)
        .bind(now - 30 * 86400)
        .fetch_optional(&self.pool)
        .await?;
        let total_hours = total_duration as f64 / 3600.0;

        Ok(serde_json::json!({
            "today": today, "this_week": week, "total": total,
            "lossless_pct": lossless_pct,
            // What the share is *of*. The client shows the coverage instead of
            // the percentage when this is too small a slice to score — a 94%
            // computed from 558 of 21,390 listens is a fact about the 558.
            "fidelity_known": fidelity_known,
            "top_artist": top_artist.as_ref().map(|(a, _)| a.as_str()).unwrap_or("—"),
            "top_artist_count": top_artist.as_ref().map(|(_, c)| *c).unwrap_or(0),
            "total_hours": (total_hours * 10.0).round() / 10.0,
            "today_hours": (today_hours * 10.0).round() / 10.0,
            "week_hours": (week_hours * 10.0).round() / 10.0,
            "avg_quality": avg_quality.map(|q| (q * 10.0).round() / 10.0).unwrap_or(0.0),
        }))
    }
}
