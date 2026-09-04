//! Entities, liner notes, loves and listening sessions.
//!
//! A note hangs off exactly one of four targets (entity, scrobble, period,
//! line) — a CHECK enforces it. `resolve_reference` *creates* the entity it
//! finds in the history; `[[gear:…]]` deliberately does not.

use super::*;

impl Database {
    /// Identity of an entity: kind + artist + name, all case- and
    /// padding-insensitive. Two spellings of one record should be one row, and
    /// the same track title by two artists must not be.
    fn entity_key(kind: &str, artist_name: &str, name: &str) -> String {
        format!(
            "{}|{}|{}",
            kind.trim().to_lowercase(),
            artist_name.trim().to_lowercase(),
            name.trim().to_lowercase()
        )
    }
    /// The *forgiving* identity of an entity — what makes a love survive being
    /// played from somewhere else.
    ///
    /// `entity_key` is an exact match on the lowercased strings, which is right
    /// for storage and wrong for recognition: two sources describing the same
    /// song rarely spell it identically. Plex files a listen under the record's
    /// artist (`grandparentTitle`) while a scrobble client reports what the
    /// player displays, so anything with a feature or a collaboration arrives
    /// under two names — and an apostrophe, an accent or a ligature does the
    /// same. Loving a track in one place then left the heart empty in the other,
    /// with nothing erroring.
    ///
    /// Built from the two normalisers this codebase already has rather than a
    /// third: `normalise_artist` folds punctuation, accents and the ligatures
    /// Unicode does not decompose, and `split_featured` takes the primary of an
    /// explicit `feat.` credit. Both are already trusted to decide "are these
    /// two spellings of one name?" — the sanitiser asks it to group candidates
    /// for review, and this asks it to fill a heart. Neither rewrites data.
    ///
    /// **The title is folded but never *shortened*.** `normalise_artist` runs
    /// over it, so punctuation stops mattering, but a trailing qualifier is
    /// left alone: "Archangel" and "Archangel (Remastered)" stay apart, because
    /// a remaster genuinely may be a different recording and MusicBrainz ids
    /// tell them apart correctly. Folding them together is a further decision
    /// than this one.
    pub fn match_key(kind: &str, artist_name: &str, name: &str) -> String {
        let artist = split_featured(artist_name).unwrap_or_else(|| artist_name.to_string());
        format!(
            "{}|{}|{}",
            kind.trim().to_lowercase(),
            normalise_artist(&artist),
            normalise_artist(name)
        )
    }
    /// The key a *loved* entity is recognised by when its MusicBrainz id is
    /// known. Its own namespace so it can never collide with a folded key.
    pub fn mbid_key(kind: &str, mbid: &str) -> String {
        format!("mbid:{}:{}", kind.trim().to_lowercase(), mbid.trim().to_lowercase())
    }
    /// Get-or-create. An MBID is recorded when the caller has one, but never
    /// required and never the identity — `COALESCE` so a later lookup that
    /// finds nothing can't erase an MBID we already had.
    pub async fn upsert_entity(
        &self,
        kind: &str,
        name: &str,
        artist_name: &str,
        mbid: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        let key = Self::entity_key(kind, artist_name, name);
        let folded = Self::match_key(kind, artist_name, name);
        let kind_lc = kind.trim().to_lowercase();

        // Three ways to recognise something that already exists, tried in
        // descending order of certainty. Only if none of them finds anything is
        // a new entity created — which is what stops one song becoming two
        // because two players spell its artist differently.
        //
        // 1. The exact key. Unchanged, and still the UNIQUE column.
        // 2. Its **MusicBrainz id**. This is the authority on what a recording
        //    is, and it was already stored on every entity and never once read
        //    back: a Plex listen and a scrobble-client listen of the same track
        //    carried the identical recording id and still became two rows with
        //    two separate loves.
        // 3. The folded key, for everything with no id — which is most of a real
        //    library, and all of a listen a scrobble client never enriched.
        //
        // Ordered by id so the answer is stable: a database that already holds
        // two rows folding together keeps resolving to the first one made,
        // rather than moving between them as rows are added.
        let existing: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM entities WHERE entity_key = ?")
                .bind(&key)
                .fetch_optional(&self.pool)
                .await?;
        let existing =
            match existing {
                Some(hit) => Some(hit),
                None => match mbid.map(str::trim).filter(|m| !m.is_empty()) {
                    Some(m) => sqlx::query_as(
                        "SELECT id FROM entities WHERE kind = ? AND mbid = ? ORDER BY id LIMIT 1",
                    )
                    .bind(&kind_lc)
                    .bind(m)
                    .fetch_optional(&self.pool)
                    .await?,
                    None => None,
                },
            };
        let existing =
            match existing {
                Some(hit) => Some(hit),
                None => sqlx::query_as(
                    "SELECT id FROM entities WHERE kind = ? AND match_key = ? ORDER BY id LIMIT 1",
                )
                .bind(&kind_lc)
                .bind(&folded)
                .fetch_optional(&self.pool)
                .await?,
            };

        let id = match existing {
            Some((id,)) => {
                // Never overwrite what is there — the row keeps the spelling it
                // was created with, and only gains an id it did not have.
                // `COALESCE` for the same reason the insert used it: a later
                // lookup that finds nothing must not erase one we already had.
                let _ = sqlx::query(
                    "UPDATE entities SET mbid = COALESCE(mbid, ?),
                            match_key = COALESCE(match_key, ?) WHERE id = ?",
                )
                .bind(mbid)
                .bind(&folded)
                .bind(id)
                .execute(&self.pool)
                .await;
                id
            }
            None => {
                sqlx::query(
                    "INSERT INTO entities
                        (kind, name, artist_name, entity_key, match_key, mbid, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?)
                     ON CONFLICT(entity_key) DO UPDATE SET
                        mbid = COALESCE(excluded.mbid, entities.mbid),
                        match_key = COALESCE(excluded.match_key, entities.match_key)",
                )
                .bind(&kind_lc)
                .bind(name.trim())
                .bind(artist_name.trim())
                .bind(&key)
                .bind(&folded)
                .bind(mbid)
                .bind(Self::now_secs())
                .execute(&self.pool)
                .await?;
                let (id,): (i64,) = sqlx::query_as("SELECT id FROM entities WHERE entity_key = ?")
                    .bind(&key)
                    .fetch_one(&self.pool)
                    .await?;
                id
            }
        };

        if let Some(mbid) = mbid.filter(|m| !m.trim().is_empty()) {
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO entity_aliases (entity_id, alias_kind, value) VALUES (?, 'mbid', ?)"
            ).bind(id).bind(mbid.trim()).execute(&self.pool).await;
        }
        Ok(id)
    }
    pub async fn get_entity(&self, id: i64) -> Result<Option<EntityRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, kind, name, artist_name, mbid, created_at FROM entities WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }
    /// Exact match on the identity key, or — when the artist isn't known — the
    /// oldest entity of that kind with the name.
    pub async fn find_entity(
        &self,
        kind: &str,
        name: &str,
        artist_name: Option<&str>,
    ) -> Result<Option<EntityRow>, sqlx::Error> {
        if let Some(artist) = artist_name.filter(|a| !a.trim().is_empty()) {
            let key = Self::entity_key(kind, artist, name);
            let hit: Option<EntityRow> = sqlx::query_as(
                "SELECT id, kind, name, artist_name, mbid, created_at FROM entities WHERE entity_key = ?"
            ).bind(key).fetch_optional(&self.pool).await?;
            if hit.is_some() {
                return Ok(hit);
            }
        }
        sqlx::query_as(
            "SELECT id, kind, name, artist_name, mbid, created_at FROM entities
             WHERE kind = ? AND LOWER(TRIM(name)) = ? ORDER BY id LIMIT 1",
        )
        .bind(kind.trim().to_lowercase())
        .bind(name.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await
    }
    /// What a `[[track:…]]` / `[[album:…]]` / `[[artist:…]]` shortcode points at.
    ///
    /// Resolution walks outward: an entity that already exists, then the user's
    /// own listening history. The history step **creates** the entity it finds,
    /// which is a write on a read path — deliberately, because the alternative
    /// is that every cross-reference renders dead until you happen to annotate
    /// the thing it points at. Entities are shared metadata, not user content,
    /// so minting one costs nothing and makes references work for anything you
    /// have actually played.
    ///
    /// `position` handles `[[track:3]]`: the third track of `ctx_album`,
    /// resolved through the listen history and rendered with its real title.
    pub async fn resolve_reference(
        &self,
        user_id: i64,
        kind: &str,
        target: &str,
        position: Option<i64>,
        ctx_artist: Option<&str>,
        ctx_album: Option<&str>,
    ) -> Result<Option<(i64, String)>, sqlx::Error> {
        let needle = target.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(None);
        }

        // "Track 3 of this record" — only answerable with an album in context.
        if let (Some(pos), Some(album)) = (position.filter(|_| kind == "recording"), ctx_album) {
            let row: Option<(String, String, Option<String>)> = sqlx::query_as(
                "SELECT title, artist, mbid_recording FROM scrobbles
                 WHERE user_id = ? AND LOWER(TRIM(album)) = ? AND track_number = ?
                 ORDER BY id DESC LIMIT 1",
            )
            .bind(user_id)
            .bind(album.trim().to_lowercase())
            .bind(pos)
            .fetch_optional(&self.pool)
            .await?;
            if let Some((title, artist, mbid)) = row {
                let id = self.upsert_entity("recording", &title, &artist, mbid.as_deref()).await?;
                return Ok(Some((id, title)));
            }
            return Ok(None);
        }
        if position.is_some() {
            return Ok(None);
        }

        if let Some(e) = self.find_entity(kind, target, ctx_artist).await? {
            return Ok(Some((e.id, e.name)));
        }

        // Nothing annotated yet — fall back to what this user has listened to.
        let found: Option<(String, String, Option<String>)> = match kind {
            "artist" => sqlx::query_as(
                "SELECT artist, '', mbid_artist FROM scrobbles
                 WHERE user_id = ? AND LOWER(TRIM(artist)) = ? ORDER BY id DESC LIMIT 1"
            ).bind(user_id).bind(&needle).fetch_optional(&self.pool).await?,

            // `ORDER BY (artist = ctx) DESC` prefers the record by the artist
            // this note is about when two of them share a title.
            "release" => sqlx::query_as(
                "SELECT album, artist, mbid_release FROM scrobbles
                 WHERE user_id = ? AND LOWER(TRIM(album)) = ?
                 ORDER BY (LOWER(TRIM(artist)) = ?) DESC, id DESC LIMIT 1"
            )
            .bind(user_id).bind(&needle)
            .bind(ctx_artist.unwrap_or_default().trim().to_lowercase())
            .fetch_optional(&self.pool).await?,

            _ => sqlx::query_as(
                "SELECT title, artist, mbid_recording FROM scrobbles
                 WHERE user_id = ? AND LOWER(TRIM(title)) = ?
                 ORDER BY (LOWER(TRIM(album)) = ?) DESC, (LOWER(TRIM(artist)) = ?) DESC, id DESC LIMIT 1"
            )
            .bind(user_id).bind(&needle)
            .bind(ctx_album.unwrap_or_default().trim().to_lowercase())
            .bind(ctx_artist.unwrap_or_default().trim().to_lowercase())
            .fetch_optional(&self.pool).await?,
        };

        let Some((name, artist, mbid)) = found else {
            return Ok(None);
        };
        if name.trim().is_empty() {
            return Ok(None);
        }
        let id = self.upsert_entity(kind, &name, &artist, mbid.as_deref()).await?;
        Ok(Some((id, name)))
    }
    /// Point a just-saved listen at its session.
    ///
    /// Keyed on `source_id`, which is the per-listen identity the dedup checks
    /// already rely on.
    pub async fn link_scrobble_session(
        &self,
        user_id: i64,
        source_id: &str,
        session_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE scrobbles SET session_id = ? WHERE user_id = ? AND source_id = ?")
            .bind(session_id)
            .bind(user_id)
            .bind(source_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// The skips, with what was playing either side of each one.
    ///
    /// The second-most-useful column here is `listened_ms` and the most useful
    /// is the **context**: bailing out is a judgement about a track *in a
    /// sequence*, and "what came before and after" is the only part of that
    /// this application can honestly record. Why somebody skipped is not
    /// derivable and is not guessed at anywhere.
    ///
    /// **Two queries regardless of how many skips are asked for.** The obvious
    /// shape — a correlated subquery per row for the previous and next listen —
    /// is the quadratic that made Rediscovery take two minutes on the real
    /// host, and it cannot be indexed because the join key is a comparison
    /// against each row's own timestamp. Instead the neighbours for the whole
    /// page are fetched in one bounded range scan and matched in Rust, which is
    /// the same pattern `list_crate` and `playlist_tracks_detailed` use.
    pub async fn skips(
        &self,
        user_id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<(i64, Vec<SkipRow>), sqlx::Error> {
        let (total,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = TRUE")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;

        let rows: Vec<(
            i64,
            String,
            String,
            Option<String>,
            i64,
            Option<i64>,
            Option<i64>,
            String,
            Option<String>,
        )> = sqlx::query_as(
            "SELECT s.id, s.title, s.artist, s.album, s.timestamp, s.duration,
                        s.listened_ms, s.source_name, c.name
                   FROM scrobbles s
                   LEFT JOIN signal_chains c ON c.id = s.chain_id
                  WHERE s.user_id = ? AND s.skipped = TRUE
                  ORDER BY s.timestamp DESC
                  LIMIT ? OFFSET ?",
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok((total, Vec::new()));
        }

        // One range scan covering every skip on this page, widened enough to
        // catch the track either side of each. `idx_scrobbles_user_time` serves
        // it, so this is a seek and a short walk rather than a scan.
        let oldest = rows.iter().map(|r| r.4).min().unwrap_or(0) - SKIP_CONTEXT_WINDOW_SECS;
        let newest = rows.iter().map(|r| r.4).max().unwrap_or(0) + SKIP_CONTEXT_WINDOW_SECS;
        let neighbours: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT timestamp, title, artist FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND timestamp BETWEEN ? AND ?
              ORDER BY timestamp ASC",
        )
        .bind(user_id)
        .bind(oldest)
        .bind(newest)
        .fetch_all(&self.pool)
        .await?;

        let out = rows
            .into_iter()
            .map(|(id, title, artist, album, timestamp, duration, listened_ms, source, chain)| {
                // Nearest real listen either side. A skip at the very start or
                // end of a sitting legitimately has none, and `None` is the
                // honest answer rather than reaching further for something
                // unrelated.
                let before = neighbours
                    .iter()
                    .filter(|(ts, _, _)| *ts < timestamp)
                    .next_back()
                    .map(|(_, t, a)| (t.clone(), a.clone()));
                let after = neighbours
                    .iter()
                    .find(|(ts, _, _)| *ts > timestamp)
                    .map(|(_, t, a)| (t.clone(), a.clone()));
                SkipRow {
                    id,
                    title,
                    artist,
                    album,
                    timestamp,
                    duration,
                    listened_ms,
                    source,
                    chain,
                    before,
                    after,
                }
            })
            .collect();
        Ok((total, out))
    }
    /// What gets skipped most, by track.
    ///
    /// Its own query because a page of recent skips cannot answer it, and the
    /// question a person actually has ("what do I keep bailing on?") is about
    /// the whole history.
    pub async fn most_skipped(
        &self,
        user_id: i64,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title, artist, COUNT(*) AS n FROM scrobbles
              WHERE user_id = ? AND skipped = TRUE
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
              ORDER BY n DESC, title ASC
              LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Recalculate a session's aggregates from the listens that point at it.
    ///
    /// Deletes the session outright when nothing points at it any more, which
    /// is what makes merge and split self-cleaning.
    pub async fn recompute_session(&self, session_id: i64) -> Result<bool, sqlx::Error> {
        let agg: Option<(i64, Option<i64>, Option<i64>, Option<i64>, Option<f64>)> =
            sqlx::query_as(
                "SELECT COUNT(*), MIN(timestamp), MAX(timestamp + COALESCE(duration, 0)),
                        SUM(COALESCE(duration, 0)), AVG(quality_score)
                 FROM scrobbles WHERE session_id = ?",
            )
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await?;

        let Some((count, started, ended, total, avg)) = agg else {
            return Ok(false);
        };
        if count == 0 {
            sqlx::query("DELETE FROM sessions WHERE id = ?")
                .bind(session_id)
                .execute(&self.pool)
                .await?;
            return Ok(false);
        }

        sqlx::query(
            "UPDATE sessions SET started_at = ?, ended_at = ?, track_count = ?,
                    total_duration = ?, avg_quality_score = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(started.unwrap_or(0))
        .bind(ended)
        .bind(count)
        .bind(total.unwrap_or(0))
        .bind(avg)
        .bind(Self::now_secs())
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }
    /// Re-file listens whose codec names a DSD stream but which were stored as
    /// lossy PCM, and give them the score they should always have had.
    ///
    /// Returns how many rows were corrected. Idempotent by construction: the
    /// `WHERE` selects only rows that are still wrong.
    ///
    /// The rescore goes through `AudioQuality::compute_score` rather than being
    /// expressed in SQL, because a second copy of that formula in a `CASE` is
    /// free to drift from the real one — and this is the query that decides
    /// what the fidelity readouts say.
    pub async fn reclassify_dsd_listens(&self) -> Result<u64, sqlx::Error> {
        // `codec LIKE 'dsd%'` is the cheap half of `codec_is_dsd`; `dst` is
        // named explicitly. The container is *not* consulted — a DSF holding
        // PCM would be a lie, and the codec is the authority either way.
        let rows: Vec<(
            i64,
            String,
            Option<i32>,
            Option<i32>,
            Option<i16>,
            Option<bool>,
            Option<String>,
            Option<i32>,
        )> = sqlx::query_as(
            "SELECT id, codec, sample_rate, bitrate, bit_depth, is_transcoded,
                        delivery_codec, delivery_bitrate
                   FROM scrobbles
                  WHERE (LOWER(codec) LIKE 'dsd%' OR LOWER(codec) = 'dst')
                    AND COALESCE(format_type, '') <> 'dsd'",
        )
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(0);
        }
        info!("⚙️ Migrating: re-filing {} DSD listens stored as lossy PCM...", rows.len());

        let mut tx = self.pool.begin().await?;
        let mut done = 0u64;
        for (
            id,
            codec,
            sample_rate,
            bitrate,
            bit_depth,
            is_transcoded,
            delivery_codec,
            delivery_bitrate,
        ) in rows
        {
            let mut q = AudioQuality {
                codec: Some(codec),
                sample_rate,
                bitrate,
                bit_depth,
                is_transcoded,
                delivery_codec,
                delivery_bitrate,
                ..Default::default()
            };
            q.classify_codec();
            q.compute_score();
            sqlx::query(
                "UPDATE scrobbles
                    SET format_type = ?, is_lossless = ?, dsd_rate = COALESCE(?, dsd_rate),
                        dsd_multiplier = COALESCE(?, dsd_multiplier), quality_score = ?
                  WHERE id = ?",
            )
            .bind(&q.format_type)
            .bind(q.is_lossless)
            .bind(q.dsd_rate)
            .bind(q.dsd_multiplier)
            .bind(q.quality_score)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            done += 1;
        }
        tx.commit().await?;
        Ok(done)
    }
    /// Fill `scrobble_artists` from the listens already stored.
    ///
    /// Batched into one transaction per chunk for the reason
    /// `rebuild_sessions` is: a per-row round trip over a 49k-listen history is
    /// ~50,000 commits, which on a Pi with the database on an SD card is
    /// minutes rather than seconds — and this one runs at **startup**, where
    /// that is the difference between a restart and an outage.
    pub async fn backfill_scrobble_artists(&self) -> Result<u64, sqlx::Error> {
        let mut done = 0u64;
        let mut after = 0i64;

        loop {
            let rows: Vec<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT id, artist, artist_credit, mbid_artist
                   FROM scrobbles WHERE id > ? ORDER BY id LIMIT ?",
            )
            .bind(after)
            .bind(CREDIT_BACKFILL_CHUNK)
            .fetch_all(&self.pool)
            .await?;
            if rows.is_empty() {
                break;
            }

            let mut tx = self.pool.begin().await?;
            for (id, artist, credit, mbid_json) in &rows {
                after = *id;
                // `artist_credit` is everyone credited joined with "; ", primary
                // first — the shape `save_scrobble` and `fold_artist_credit`
                // both write. Splitting it back out is reading our own format,
                // not guessing at a tag: a name containing a semicolon would
                // already have been mangled on the way in.
                let mut names: Vec<String> = vec![artist.clone()];
                if let Some(c) = credit {
                    names.extend(c.split(';').map(|n| n.trim().to_string()));
                }
                let mbids: Vec<String> = mbid_json
                    .as_deref()
                    .and_then(|j| serde_json::from_str::<Vec<String>>(j).ok())
                    .unwrap_or_default();

                let mut seen: Vec<String> = Vec::new();
                for name in names {
                    let name = name.trim();
                    if name.is_empty() {
                        continue;
                    }
                    let key = Database::artist_key(name);
                    if seen.contains(&key) {
                        continue;
                    }
                    let position = seen.len() as i64;
                    seen.push(key.clone());
                    sqlx::query(
                        "INSERT OR IGNORE INTO scrobble_artists
                             (scrobble_id, artist_key, name, position, mbid)
                         VALUES (?, ?, ?, ?, ?)",
                    )
                    .bind(id)
                    .bind(&key)
                    .bind(name)
                    .bind(position)
                    .bind(mbids.get(position as usize))
                    .execute(&mut *tx)
                    .await?;
                }
                done += 1;
            }
            tx.commit().await?;

            if rows.len() < CREDIT_BACKFILL_CHUNK as usize {
                break;
            }
        }

        info!("🎻 Credited the performers on {} listens", done);
        Ok(done)
    }
    /// Rebuild every session for one user from their listens.
    ///
    /// Needed because `assign_session` maintained the session rows while nothing
    /// ever wrote `scrobbles.session_id`, so an existing database has sessions
    /// with track counts and no contents. It's also the only way to re-group
    /// after changing the gap, and the way an *imported* history — which never
    /// went through the ingest path — gets sessions at all.
    ///
    /// Destructive by design: manual merges and splits are stored in this same
    /// column, so a rebuild discards them. The caller is expected to say so.
    pub async fn rebuild_sessions(
        &self,
        user_id: i64,
        gap_seconds: i64,
    ) -> Result<(u64, u64), sqlx::Error> {
        // Ordered walk. Only the gap is available here: a listen doesn't record
        // which token submitted it, so the token-change rule that `assign_session`
        // applies live can't be reconstructed after the fact.
        let rows: Vec<(
            i64,
            i64,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<f64>,
            Option<String>,
            Option<i64>,
        )> = sqlx::query_as(
            "SELECT id, timestamp, duration, device_id, chain_id, quality_score,
                        listening_context, session_id
                   FROM scrobbles WHERE user_id = ? AND skipped = FALSE
                  ORDER BY timestamp ASC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        // **What a sitting is called, and the id other people hold, survive a
        // regroup.**
        //
        // This used to mint a fresh `new_uuid()` for every group and carry
        // neither `title` nor `notes`, so a rebuild silently erased every name
        // and note anybody had written — and, since v0.83.0, broke every
        // handed-over sitting, because `sessions.uuid` is what a hand-over
        // points at and the whole reason that column is documented as stable
        // across merges and splits.
        //
        // A sitting is identified by the listen it opens with. Where a regroup
        // produces a group starting on the same listen, it *is* that sitting
        // and keeps its identity; anything else is new. That rule falls out the
        // same way `merge_sessions` already resolves this — the earliest
        // session's identity is the one that survives — so a larger gap folding
        // two sittings together keeps the first one's name, and a smaller gap
        // splitting one keeps it on the opening half.
        let mut first_listen_of: HashMap<i64, i64> = HashMap::new();
        for (id, _, _, _, _, _, _, session_id) in &rows {
            if let Some(sid) = session_id {
                first_listen_of.entry(*sid).or_insert(*id);
            }
        }
        let prior: Vec<(i64, Option<String>, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT id, uuid, title, notes FROM sessions WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        // Keyed by the opening listen, which is what a group can be matched on.
        let mut identity: HashMap<i64, (Option<String>, Option<String>, Option<String>)> =
            HashMap::new();
        for (sid, uuid, title, notes) in prior {
            if let Some(first) = first_listen_of.get(&sid) {
                identity.insert(*first, (uuid, title, notes));
            }
        }

        // Grouped in Rust first, written afterwards. The previous version did a
        // round trip *per listen* — an INSERT at each boundary, an `UPDATE
        // scrobbles SET session_id` for every single row, and a two-statement
        // `recompute_session` per session, every one of them its own
        // autocommitted transaction.
        //
        // That is ~50,000 separate commits on a 49k-listen history. On an NVMe
        // laptop it is a couple of seconds and looks fine; on a Pi with the
        // database on an SD card each commit is a WAL write and the whole thing
        // ran past Cloudflare's 100s origin timeout, so the request died with a
        // 524 while the rebuild carried on in the background. Same blind spot
        // that hid the Rediscovery quadratic: the cost is storage latency
        // multiplied by statement count, and the dev machine hides both.
        struct Pending {
            device_id: Option<i64>,
            chain_id: Option<i64>,
            context: Option<String>,
            started_at: i64,
            ended_at: i64,
            total_duration: i64,
            /// Summed and counted rather than averaged as we go, because
            /// `AVG(quality_score)` ignores NULLs and most listens have none —
            /// treating those as zero would score an imported history at nearly
            /// nothing.
            quality_sum: f64,
            quality_n: i64,
            listens: Vec<i64>,
        }

        let mut groups: Vec<Pending> = Vec::new();
        let mut prev_end = i64::MIN;

        for (id, ts, dur, device_id, chain_id, quality, context, _) in rows {
            if groups.is_empty() || ts - prev_end > gap_seconds {
                groups.push(Pending {
                    device_id,
                    chain_id,
                    context,
                    started_at: ts,
                    ended_at: ts,
                    total_duration: 0,
                    quality_sum: 0.0,
                    quality_n: 0,
                    listens: Vec::new(),
                });
            }
            // Safe: the branch above guarantees there is one.
            let g = groups.last_mut().expect("a session was just started");
            let d = dur.unwrap_or(0);
            g.ended_at = g.ended_at.max(ts + d);
            g.total_duration += d;
            if let Some(q) = quality {
                g.quality_sum += q;
                g.quality_n += 1;
            }
            g.listens.push(id);
            prev_end = ts + d;
        }

        let sessions = groups.len() as u64;
        let linked: u64 = groups.iter().map(|g| g.listens.len() as u64).sum();

        // One transaction for the whole rebuild. This is a destructive
        // regroup — every listen loses its session before any of them gets a
        // new one — so a crash halfway through used to leave a history with no
        // sessions at all and no way to tell that had happened.
        let mut tx = self.pool.begin().await?;

        // Clear the link first — the foreign key means sessions can't go while
        // anything still points at them.
        sqlx::query("UPDATE scrobbles SET session_id = NULL WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

        let now = Self::now_secs();
        for g in &groups {
            // The sitting this group opens with, if it is one we already had.
            // A group whose first listen matches nothing is genuinely new and
            // gets a fresh identity.
            let carried = g.listens.first().and_then(|first| identity.get(first));
            let (uuid, title, notes) = match carried {
                Some((u, t, n)) => (u.clone().unwrap_or_else(new_uuid), t.clone(), n.clone()),
                None => (new_uuid(), None, None),
            };
            let res = sqlx::query(
                "INSERT INTO sessions (user_id, device_id, chain_id, started_at, ended_at,
                    track_count, total_duration, avg_quality_score, listening_context,
                    uuid, title, notes, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(g.device_id)
            .bind(g.chain_id)
            .bind(g.started_at)
            .bind(g.ended_at)
            .bind(g.listens.len() as i64)
            .bind(g.total_duration)
            .bind((g.quality_n > 0).then(|| g.quality_sum / g.quality_n as f64))
            .bind(g.context.clone().unwrap_or_else(|| "unknown".into()))
            .bind(uuid)
            .bind(title)
            .bind(notes)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            let sid = res.last_insert_rowid();

            // Chunked rather than one statement per listen. The cap is well
            // under SQLite's parameter limit and keeps a long session from
            // building a statement thousands of placeholders wide.
            for chunk in g.listens.chunks(SESSION_LINK_CHUNK) {
                let placeholders =
                    std::iter::repeat_n("?", chunk.len()).collect::<Vec<_>>().join(",");
                let sql = format!(
                    "UPDATE scrobbles SET session_id = ? WHERE user_id = ? AND id IN ({placeholders})"
                );
                let mut q = sqlx::query(&sql).bind(sid).bind(user_id);
                for id in chunk {
                    q = q.bind(id);
                }
                q.execute(&mut *tx).await?;
            }
        }

        tx.commit().await?;
        info!("🧵 Rebuilt {} sessions across {} listens for user {}", sessions, linked, user_id);
        Ok((sessions, linked))
    }
    /// A user's sessions, most recent first, with the chain's name joined so the
    /// list doesn't need a second request per row.
    pub async fn list_sessions(
        &self,
        user_id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<SessionRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT s.id, s.uuid, s.title, s.notes, s.started_at, s.ended_at, s.track_count,
                    s.total_duration, s.avg_quality_score, s.listening_context,
                    s.chain_id, c.name AS chain_name,
                    -- One correlated subquery, not three. The variant *name*
                    -- used to re-nest the id lookup inside itself, so every
                    -- session row scanned `scrobbles` twice over to answer one
                    -- question; joining `chain_variants` on the id instead asks
                    -- it once. With `idx_scrobbles_session` that one is a seek.
                    sv.chain_variant_id AS chain_variant_id,
                    v.name AS chain_variant_name,
                    s.device_id, d.name AS device_name,
                    s.token_id, t.name AS token_name
               FROM sessions s
               LEFT JOIN signal_chains c ON c.id = s.chain_id
               LEFT JOIN devices d ON d.id = s.device_id
               LEFT JOIN tokens t ON t.id = s.token_id
               LEFT JOIN (
                    SELECT session_id, MIN(chain_variant_id) AS chain_variant_id
                      FROM scrobbles
                     WHERE chain_variant_id IS NOT NULL AND session_id IS NOT NULL
                     GROUP BY session_id
               ) sv ON sv.session_id = s.id
               LEFT JOIN chain_variants v ON v.id = sv.chain_variant_id
              WHERE s.user_id = ? AND s.track_count > 0
              ORDER BY s.started_at DESC LIMIT ? OFFSET ?",
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }
    pub async fn get_session(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<SessionRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT s.id, s.uuid, s.title, s.notes, s.started_at, s.ended_at, s.track_count,
                    s.total_duration, s.avg_quality_score, s.listening_context,
                    s.chain_id, c.name AS chain_name,
                    -- One correlated subquery, not three. The variant *name*
                    -- used to re-nest the id lookup inside itself, so every
                    -- session row scanned `scrobbles` twice over to answer one
                    -- question; joining `chain_variants` on the id instead asks
                    -- it once. With `idx_scrobbles_session` that one is a seek.
                    sv.chain_variant_id AS chain_variant_id,
                    v.name AS chain_variant_name,
                    s.device_id, d.name AS device_name,
                    s.token_id, t.name AS token_name
               FROM sessions s
               LEFT JOIN signal_chains c ON c.id = s.chain_id
               LEFT JOIN devices d ON d.id = s.device_id
               LEFT JOIN tokens t ON t.id = s.token_id
               -- One row, so this is a seek on `idx_scrobbles_session` rather
               -- than the grouped pass `list_sessions` uses — that one answers
               -- for a whole page at once, this one for a single session.
               LEFT JOIN (
                    SELECT MIN(chain_variant_id) AS chain_variant_id
                      FROM scrobbles
                     WHERE session_id = ?2 AND chain_variant_id IS NOT NULL
               ) sv
               LEFT JOIN chain_variants v ON v.id = sv.chain_variant_id
              WHERE s.user_id = ?1 AND s.id = ?2",
        )
        .bind(user_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }
    /// The listens in one session, oldest first — the order they were played.
    pub async fn session_scrobbles(
        &self,
        user_id: i64,
        session_id: i64,
    ) -> Result<Vec<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM scrobbles WHERE user_id = ? AND session_id = ?
             ORDER BY timestamp ASC",
        )
        .bind(user_id)
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Fold several sessions into one.
    ///
    /// The **earliest** session survives, because its id is the one already
    /// referenced by anything pointing at "that listening"; the rest hand over
    /// their listens and are deleted by the recompute. Ownership is checked for
    /// every id before anything moves, so a request naming someone else's
    /// session changes nothing at all rather than half-merging.
    pub async fn merge_sessions(
        &self,
        user_id: i64,
        ids: &[i64],
    ) -> Result<Option<i64>, sqlx::Error> {
        if ids.len() < 2 {
            return Ok(None);
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let owned: Vec<(i64,)> = {
            let sql = format!(
                "SELECT id FROM sessions WHERE user_id = ? AND id IN ({}) ORDER BY started_at ASC",
                placeholders
            );
            let mut q = sqlx::query_as(&sql).bind(user_id);
            for id in ids {
                q = q.bind(id);
            }
            q.fetch_all(&self.pool).await?
        };
        if owned.len() != ids.len() {
            return Ok(None);
        }

        let target = owned[0].0;
        let others: Vec<i64> = owned[1..].iter().map(|(id,)| *id).collect();
        let others_ph = vec!["?"; others.len()].join(",");

        {
            let sql = format!(
                "UPDATE scrobbles SET session_id = ? WHERE user_id = ? AND session_id IN ({})",
                others_ph
            );
            let mut q = sqlx::query(&sql).bind(target).bind(user_id);
            for id in &others {
                q = q.bind(id);
            }
            q.execute(&self.pool).await?;
        }

        // Now empty; the recompute deletes each one.
        for id in &others {
            self.recompute_session(*id).await?;
        }
        self.recompute_session(target).await?;
        Ok(Some(target))
    }
    /// Cut a session in two at `at_scrobble_id`, which becomes the first listen
    /// of the new session.
    ///
    /// Returns the new session's id. Splitting at the very first listen is a
    /// no-op rather than an error: it would leave one empty half, which is just
    /// the session you already had.
    pub async fn split_session(
        &self,
        user_id: i64,
        session_id: i64,
        at_scrobble_id: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        let listens = self.session_scrobbles(user_id, session_id).await?;
        let Some(pos) = listens.iter().position(|s| s.id == at_scrobble_id) else {
            return Ok(None);
        };
        if pos == 0 {
            return Ok(None);
        }

        let old: Option<(i64, Option<i64>, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT started_at, device_id, chain_id, listening_context
               FROM sessions WHERE id = ? AND user_id = ?",
        )
        .bind(session_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((_, device_id, chain_id, context)) = old else {
            return Ok(None);
        };

        // The tail inherits the setup — a split says "this was a separate
        // sitting", not "this was different gear".
        let moving: Vec<i64> = listens[pos..].iter().map(|s| s.id).collect();
        let started = listens[pos].timestamp;

        let res = sqlx::query(
            "INSERT INTO sessions (user_id, device_id, chain_id, started_at, ended_at,
                track_count, total_duration, listening_context, uuid, updated_at)
             VALUES (?, ?, ?, ?, ?, 0, 0, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(device_id)
        .bind(chain_id)
        .bind(started)
        .bind(started)
        .bind(context.unwrap_or_else(|| "unknown".into()))
        .bind(new_uuid())
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        let new_id = res.last_insert_rowid();

        let ph = vec!["?"; moving.len()].join(",");
        let sql =
            format!("UPDATE scrobbles SET session_id = ? WHERE user_id = ? AND id IN ({})", ph);
        let mut q = sqlx::query(&sql).bind(new_id).bind(user_id);
        for id in &moving {
            q = q.bind(id);
        }
        q.execute(&self.pool).await?;

        self.recompute_session(session_id).await?;
        self.recompute_session(new_id).await?;
        Ok(Some(new_id))
    }
    /// Retag a whole session's listens onto a chain, and the session with them.
    ///
    /// "This whole sitting was on the desk rig" is the edit sessions exist to
    /// make cheap — the alternative is shift-selecting forty rows in history.
    /// Retag a whole sitting.
    ///
    /// `variant_id` is what makes "I had the other IEMs in for this one" a
    /// one-field edit rather than a second chain — the chain is the same, one
    /// step of it was different. Setting the chain always rewrites the variant
    /// too: a variant belongs to one chain, so leaving a stale one behind would
    /// pair a listen with components that were never fitted, which
    /// `equipment_seconds` then has to ignore.
    pub async fn set_session_chain(
        &self,
        user_id: i64,
        session_id: i64,
        chain_id: Option<i64>,
        variant_id: Option<i64>,
    ) -> Result<u64, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE scrobbles SET chain_id = ?, chain_variant_id = ?
              WHERE user_id = ? AND session_id = ?",
        )
        .bind(chain_id)
        .bind(variant_id)
        .bind(user_id)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE sessions SET chain_id = ?, updated_at = ? WHERE id = ? AND user_id = ?",
        )
        .bind(chain_id)
        .bind(Self::now_secs())
        .bind(session_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }
    /// Name a session. `None` clears it back to the derived label.
    /// A note about the sitting itself — distinct from a liner note about a
    /// record, which is what `notes` is for.
    pub async fn set_session_notes(
        &self,
        user_id: i64,
        session_id: i64,
        notes: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE sessions SET notes = ?, updated_at = ? WHERE id = ? AND user_id = ?",
        )
        .bind(notes)
        .bind(Self::now_secs())
        .bind(session_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    pub async fn set_session_title(
        &self,
        user_id: i64,
        session_id: i64,
        title: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE sessions SET title = ?, updated_at = ? WHERE id = ? AND user_id = ?",
        )
        .bind(title.map(str::trim).filter(|t| !t.is_empty()))
        .bind(Self::now_secs())
        .bind(session_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// One ranked chart: artists, albums or tracks over a window.
    ///
    /// Separate from `get_ranged_stats` because the charts page wants *one* long
    /// list and none of the breakdowns, heatmap or personality that query also
    /// computes — asking for all of it to render fifty rows would be wasteful on
    /// a Pi.
    pub async fn chart(
        &self,
        user_id: i64,
        kind: &str,
        after: Option<i64>,
        limit: i64,
        by_hours: bool,
    ) -> Result<Vec<ChartRow>, sqlx::Error> {
        // Same reasoning as `get_ranged_stats`: the chart is cut to `limit`
        // rows, so ranking by the chosen metric has to happen in SQL. It also
        // has to agree with Statistics — "See more" hands off from there
        // carrying its metric, and landing on a differently-ranked list would
        // be the exact inconsistency that handoff exists to remove.
        let order = if by_hours { "seconds DESC, plays DESC" } else { "plays DESC" };
        // Every branch selects the same shape so the caller doesn't care which
        // ran. `sub` is the secondary line (an album's artist), null for artists.
        let sql: String = match kind {
            "albums" => {
                format!(
                    "SELECT album AS name, artist AS sub, NULL AS album, COUNT(*) AS plays,
                        COALESCE(SUM(duration), 0) AS seconds,
                        {COVER_PICK} AS cover_pick
                 FROM scrobbles
                 WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                   AND album IS NOT NULL AND TRIM(album) != ''
                 GROUP BY LOWER(TRIM(album)), LOWER(TRIM(artist))
                 ORDER BY {order} LIMIT ?"
                )
            }
            "tracks" => {
                format!(
                    "SELECT title AS name, artist AS sub, MAX(album) AS album, COUNT(*) AS plays,
                        COALESCE(SUM(duration), 0) AS seconds,
                        {COVER_PICK} AS cover_pick
                 FROM scrobbles
                 WHERE user_id = ? AND skipped = FALSE AND (? IS NULL OR timestamp >= ?)
                 GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
                 ORDER BY {order} LIMIT ?"
                )
            }
            _ => {
                // Over `scrobble_artists`, so a performer is on the chart at
                // all: "Hilary Hahn plays Bach" credits both, where grouping by
                // `scrobbles.artist` gave the listen to Bach alone.
                //
                // `MAX(sa.name)` picks a spelling to display, since the key is
                // folded — the same trade every other grouping here makes.
                format!(
                    "SELECT MAX(sa.name) AS name, NULL AS sub, NULL AS album, COUNT(*) AS plays,
                        COALESCE(SUM(s.duration), 0) AS seconds,
                        NULL AS cover_pick
                 FROM scrobble_artists sa
                 JOIN scrobbles s ON s.id = sa.scrobble_id
                 WHERE s.user_id = ? AND s.skipped = FALSE
                   AND (? IS NULL OR s.timestamp >= ?)
                 GROUP BY sa.artist_key
                 ORDER BY {order} LIMIT ?"
                )
            }
        };
        let mut rows: Vec<ChartRow> = sqlx::query_as(&sql)
            .bind(user_id)
            .bind(after)
            .bind(after)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.iter_mut().for_each(|r| r.resolve_cover());

        // An artist's portrait doesn't live on `scrobbles` — a listen carries
        // its *cover*, which is the album's. It lives on `artists`, filled by
        // the metadata chain, which is why the artists branch above selects a
        // literal NULL. Statistics has always filled it in this second step
        // (see `ranged_stats`); the charts screen never did, so every artist
        // row fell back to the lettered tile.
        if kind == "artists" {
            let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
            let images = self.artist_images(&names).await.unwrap_or_default();
            for row in &mut rows {
                row.artwork_url = images.get(&Self::artist_key(&row.name)).cloned();
            }
        }
        Ok(rows)
    }
    /// A track in this user's history matched by name.
    ///
    /// The fallback for services that report names rather than MBIDs (Last.fm).
    /// Returns the *stored* spelling so the entity key matches everything else
    /// keyed off this listen, rather than the remote service's capitalisation.
    pub async fn track_by_name(
        &self,
        user_id: i64,
        title: &str,
        artist: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title, artist FROM scrobbles
             WHERE user_id = ? AND LOWER(TRIM(title)) = ? AND LOWER(TRIM(artist)) = ?
             ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(title.trim().to_lowercase())
        .bind(artist.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await
    }
    pub async fn track_by_recording_mbid(
        &self,
        user_id: i64,
        mbid: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT title, artist FROM scrobbles
             WHERE user_id = ? AND mbid_recording = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(mbid)
        .fetch_optional(&self.pool)
        .await
    }
    /// The album this user most recently heard a given track on. Supplies the
    /// album context for a note about a *track*, so `[[track:3]]` still means
    /// "the third track of the record this one is from".
    pub async fn album_for_track(
        &self,
        user_id: i64,
        title: &str,
        artist: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT album FROM scrobbles
             WHERE user_id = ? AND LOWER(TRIM(title)) = ? AND LOWER(TRIM(artist)) = ?
               AND album IS NOT NULL AND album <> ''
             ORDER BY id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(title.trim().to_lowercase())
        .bind(artist.trim().to_lowercase())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(album,)| album))
    }
    /// Write a note, keeping what it said before.
    ///
    /// One note per user per target: the second write edits the first rather
    /// than stacking up, and the previous body is archived first so an edit
    /// never destroys what was there. An unchanged body writes no revision —
    /// toggling visibility isn't a rewrite.
    pub async fn upsert_note(
        &self,
        user_id: i64,
        target: &NoteTarget,
        body_md: &str,
        visibility: &str,
    ) -> Result<i64, sqlx::Error> {
        let now = Self::now_secs();
        let existing = match target {
            NoteTarget::Entity(e) => self.note_for_entity(user_id, *e).await?,
            NoteTarget::Listen(s) => self.note_for_scrobble(user_id, *s).await?,
            NoteTarget::Period(k) => self.note_for_period(user_id, k).await?,
            NoteTarget::Line { key, .. } => self.note_for_line_key(user_id, key).await?,
        };
        let (entity_id, scrobble_id, period_key, line_key, line_label) = target.columns();

        if let Some(note) = existing {
            if note.body_md != body_md {
                sqlx::query(
                    "INSERT INTO note_revisions (note_id, body_md, created_at) VALUES (?, ?, ?)",
                )
                .bind(note.id)
                .bind(&note.body_md)
                .bind(note.updated_at)
                .execute(&self.pool)
                .await?;
            }
            sqlx::query("UPDATE notes SET body_md = ?, visibility = ?, updated_at = ? WHERE id = ? AND user_id = ?")
                .bind(body_md).bind(visibility).bind(now).bind(note.id).bind(user_id)
                .execute(&self.pool).await?;
            return Ok(note.id);
        }

        let result = sqlx::query(
            "INSERT INTO notes (user_id, entity_id, scrobble_id, period_key, line_key, line_label, body_md, visibility, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(user_id).bind(entity_id).bind(scrobble_id).bind(period_key)
        .bind(line_key).bind(line_label)
        .bind(body_md).bind(visibility)
        .bind(now).bind(now)
        .execute(&self.pool).await?;
        Ok(result.last_insert_rowid())
    }
    /// The pin on one lyric line, if there is one.
    ///
    /// Takes the folded key rather than the names so there is one place the
    /// fold happens — [`NoteTarget::line`] — and no caller can look a pin up
    /// under a spelling it could never have been filed under.
    pub async fn note_for_line_key(
        &self,
        user_id: i64,
        line_key: &str,
    ) -> Result<Option<NoteRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM notes WHERE user_id = ? AND line_key = ?")
            .bind(user_id)
            .bind(line_key)
            .fetch_optional(&self.pool)
            .await
    }
    /// Every pin on one track, newest edit first.
    ///
    /// One query for the whole reader rather than one per line: a lyric is a
    /// couple of hundred lines and asking per line would be a couple of hundred
    /// round trips for a page that usually has one or two pins on it. The
    /// caller matches them to lines by [`NoteTarget::line_number`].
    ///
    /// The `LIKE` is anchored on the track's folded key plus the separator, so
    /// it is a prefix match and cannot spill onto another track whose key
    /// happens to start the same way.
    pub async fn pins_for_track(
        &self,
        user_id: i64,
        artist: &str,
        title: &str,
    ) -> Result<Vec<NoteRow>, sqlx::Error> {
        let prefix = format!(
            "{}{}{}{}",
            Self::artist_key(artist),
            NoteTarget::SEP,
            title.trim().to_lowercase(),
            NoteTarget::SEP
        );
        // `%` and `_` are wildcards to LIKE and a title may hold either, so the
        // prefix is escaped by hand — the rule `search_notes` already follows.
        let mut escaped = String::with_capacity(prefix.len());
        for c in prefix.chars() {
            if matches!(c, '\\' | '%' | '_') {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        sqlx::query_as(
            "SELECT * FROM notes
              WHERE user_id = ? AND line_key LIKE ? ESCAPE '\\'
              ORDER BY updated_at DESC",
        )
        .bind(user_id)
        .bind(format!("{escaped}%"))
        .fetch_all(&self.pool)
        .await
    }
    /// Every line this user has pinned, newest edit first.
    ///
    /// Two callers, and they want it for opposite reasons: the design's "Lines
    /// you've written about" sidebar, which is deliberately **across
    /// everything** so it is a way back into writing you had forgotten; and the
    /// footnote roll-up on a record's own note, which filters this down to the
    /// record in hand. Folding one bounded read in Rust beats a prefix query
    /// per track — a person has tens of pins, and an album has tens of tracks.
    pub async fn line_notes(&self, user_id: i64, limit: i64) -> Result<Vec<NoteRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM notes
              WHERE user_id = ? AND line_key IS NOT NULL
              ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
    /// Every note whose body quotes a line, for counting what is quoted where.
    ///
    /// Narrowed in SQL to bodies that actually contain the shortcode, because
    /// the caller has to `scan` each one and resolve it against what that note
    /// is about — which costs a query or two per note. Most notes contain no
    /// line reference at all, and this is what keeps them out of that loop.
    pub async fn notes_quoting_lines(&self, user_id: i64) -> Result<Vec<NoteRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT * FROM notes
              WHERE user_id = ? AND (body_md LIKE '%[[line:%' OR body_md LIKE '%[[quote:%')
              ORDER BY updated_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// The note about a week, a month or a year, if there is one.
    pub async fn note_for_period(
        &self,
        user_id: i64,
        period_key: &str,
    ) -> Result<Option<NoteRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM notes WHERE user_id = ? AND period_key = ?")
            .bind(user_id)
            .bind(period_key)
            .fetch_optional(&self.pool)
            .await
    }
    pub async fn get_note(&self, user_id: i64, id: i64) -> Result<Option<NoteRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM notes WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
    }
    pub async fn note_for_entity(
        &self,
        user_id: i64,
        entity_id: i64,
    ) -> Result<Option<NoteRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM notes WHERE user_id = ? AND entity_id = ?")
            .bind(user_id)
            .bind(entity_id)
            .fetch_optional(&self.pool)
            .await
    }
    pub async fn note_for_scrobble(
        &self,
        user_id: i64,
        scrobble_id: i64,
    ) -> Result<Option<NoteRow>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM notes WHERE user_id = ? AND scrobble_id = ?")
            .bind(user_id)
            .bind(scrobble_id)
            .fetch_optional(&self.pool)
            .await
    }
    pub async fn list_notes(&self, user_id: i64) -> Result<Vec<NoteRow>, sqlx::Error> {
        self.search_notes(user_id, "", "", "updated").await
    }
    /// Notes matching a query and a kind, in the requested order.
    ///
    /// Filtered here rather than in the handler because rendering one note costs
    /// four more queries (its entity, its listen, whether it's loved, how many
    /// revisions it has) — so narrowing a few hundred notes to a dozen before
    /// that loop is the difference between a search being instant and being the
    /// slowest screen in the app.
    ///
    /// `q` matches the note's **body and its subject both**: someone looking for
    /// what they wrote about Burial means either "the word Burial appears in a
    /// note" or "this is the Burial note", and being asked which is a worse
    /// answer than searching both. The joins are `LEFT` because exactly one of
    /// `entity_id` / `scrobble_id` / `period_key` is ever set — and a note about
    /// a period has neither join to offer, so its own key is what `q` matches
    /// against, which is what makes "2026-08" find August's note.
    ///
    /// Case-insensitivity is SQLite's `LIKE`, which folds ASCII only — "BURIAL"
    /// finds "Burial", but "CŒUR" does not find "Cœur". Doing better means
    /// carrying a folded column, which is a bigger change than this earns.
    pub async fn search_notes(
        &self,
        user_id: i64,
        q: &str,
        kind: &str,
        sort: &str,
    ) -> Result<Vec<NoteRow>, sqlx::Error> {
        let order = match sort {
            "created" => "n.created_at DESC",
            // Alphabetical by subject. Notes on a listen have no entity name, so
            // they sort last rather than under an empty string at the top.
            // Alphabetical by subject. A note on a listen or a period has no
            // entity name, so it sorts last rather than under an empty string
            // at the top; period notes then fall together by their own key,
            // which is chronological because the key was built to sort.
            "title" => {
                "e.name IS NULL, LOWER(e.name) ASC, n.period_key ASC, LOWER(n.line_label) ASC, n.updated_at DESC"
            }
            _ => "n.updated_at DESC",
        };
        // `kind` is one of the stored entity kinds, or `listen` for a note about
        // one particular playing. Anything else means "no filter" rather than an
        // error — a stale bookmark should show everything, not nothing.
        let kind_clause = match kind {
            "recording" | "release" | "artist" => "e.kind = ?",
            "listen" => "n.scrobble_id IS NOT NULL AND ? IS NOT NULL",
            "period" => "n.period_key IS NOT NULL AND ? IS NOT NULL",
            "line" => "n.line_key IS NOT NULL AND ? IS NOT NULL",
            _ => "? IS NOT NULL",
        };
        let kind_bind = match kind {
            "recording" | "release" | "artist" => kind.to_string(),
            _ => "1".to_string(),
        };

        // `%` and `_` are wildcards to LIKE, so a note about "50_50" must not
        // become a pattern matching everything. Escaped by hand — `str::replace`
        // has no capture groups, and reaching for a regex to quote three
        // characters would be its own mistake.
        let mut escaped = String::with_capacity(q.len());
        for c in q.trim().chars() {
            if matches!(c, '\\' | '%' | '_') {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        let like = format!("%{escaped}%");
        let sql = format!(
            "SELECT n.* FROM notes n
             LEFT JOIN entities e ON e.id = n.entity_id
             LEFT JOIN scrobbles s ON s.id = n.scrobble_id
             WHERE n.user_id = ?
               AND ({kind_clause})
               AND (? = ''
                    OR n.body_md LIKE ? ESCAPE '\\'
                    OR e.name LIKE ? ESCAPE '\\'
                    OR e.artist_name LIKE ? ESCAPE '\\'
                    OR s.title LIKE ? ESCAPE '\\'
                    OR s.artist LIKE ? ESCAPE '\\'
                    OR n.period_key LIKE ? ESCAPE '\\'
                    OR n.line_label LIKE ? ESCAPE '\\')
             ORDER BY {order}"
        );

        let mut query = sqlx::query_as(&sql).bind(user_id).bind(kind_bind).bind(q.trim());
        for _ in 0..7 {
            query = query.bind(like.clone());
        }
        query.fetch_all(&self.pool).await
    }
    /// Revisions newest-first, scoped through the note's owner so an id from
    /// another user's note reads as "no revisions" rather than as history.
    pub async fn note_revisions(
        &self,
        user_id: i64,
        note_id: i64,
    ) -> Result<Vec<NoteRevisionRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT r.id, r.note_id, r.body_md, r.created_at FROM note_revisions r
             JOIN notes n ON n.id = r.note_id
             WHERE r.note_id = ? AND n.user_id = ? ORDER BY r.created_at DESC, r.id DESC",
        )
        .bind(note_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Foreign keys are on, so the revisions have to go first.
    pub async fn delete_note(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        if self.get_note(user_id, id).await?.is_none() {
            return Ok(false);
        }
        sqlx::query("DELETE FROM note_revisions WHERE note_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        let result = sqlx::query("DELETE FROM notes WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
    pub async fn set_love(
        &self,
        user_id: i64,
        entity_id: i64,
        loved: bool,
    ) -> Result<(), sqlx::Error> {
        if loved {
            sqlx::query(
                "INSERT OR IGNORE INTO loves (user_id, entity_id, created_at) VALUES (?, ?, ?)",
            )
            .bind(user_id)
            .bind(entity_id)
            .bind(Self::now_secs())
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("DELETE FROM loves WHERE user_id = ? AND entity_id = ?")
                .bind(user_id)
                .bind(entity_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
    /// A cached description, and how old it is.
    ///
    /// A row with `bio IS NULL` is a **tombstone**: every provider was asked and
    /// none had anything. Same discipline as `artwork_url = ''` and
    /// `caa_id = -1` — without it, the records nobody has written about get
    /// re-asked on every page view forever.
    pub async fn get_bio(&self, entity_id: i64) -> Result<Option<BioRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT entity_id, bio, attribution, provider, url, fetched_at
               FROM bio_cache WHERE entity_id = ?",
        )
        .bind(entity_id)
        .fetch_optional(&self.pool)
        .await
    }
    pub async fn set_bio(
        &self,
        entity_id: i64,
        bio: Option<&str>,
        attribution: Option<&str>,
        provider: Option<&str>,
        url: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO bio_cache (entity_id, bio, attribution, provider, url, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(entity_id) DO UPDATE SET
                bio = excluded.bio, attribution = excluded.attribution,
                provider = excluded.provider, url = excluded.url,
                fetched_at = excluded.fetched_at",
        )
        .bind(entity_id)
        .bind(bio)
        .bind(attribution)
        .bind(provider)
        .bind(url)
        .bind(Self::now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Identity keys of everything this user loves, for the row hearts.
    ///
    /// Built with the same `kind|artist|name` shape as `entity_key` so the
    /// client can derive a listen's key without a round-trip. `entity_key` is
    /// stored, so this is a plain column read rather than a recomputation that
    /// could drift from it.
    /// Every way a loved entity can be recognised on a listen row.
    ///
    /// Up to three per love rather than one: the exact key it was stored under,
    /// the folded key (so a different spelling of the same name still fills the
    /// heart), and an `mbid:` key when the entity has a MusicBrainz id. The
    /// client holds the union and asks whether *any* of a row's candidates is
    /// in it.
    ///
    /// More than one form is what makes this work across sources at all: Plex
    /// files a listen under the record's artist and a scrobble client reports
    /// what the player showed, so the exact key alone matched only when the two
    /// happened to agree.
    pub async fn loved_keys(&self, user_id: i64) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<(String, Option<String>, String, Option<String>)> = sqlx::query_as(
            "SELECT e.entity_key, e.match_key, e.kind, e.mbid
               FROM loves l JOIN entities e ON e.id = l.entity_id
              WHERE l.user_id = ?",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut keys: Vec<String> = Vec::with_capacity(rows.len() * 3);
        for (exact, folded, kind, mbid) in rows {
            keys.push(exact);
            if let Some(f) = folded.filter(|f| !f.is_empty()) {
                keys.push(f);
            }
            if let Some(m) = mbid.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()) {
                keys.push(Self::mbid_key(&kind, &m));
            }
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }
    pub async fn is_loved(&self, user_id: i64, entity_id: i64) -> Result<bool, sqlx::Error> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM loves WHERE user_id = ? AND entity_id = ?")
                .bind(user_id)
                .bind(entity_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.is_some())
    }
    /// Loved entities, newest first, with the timestamp of the love.
    pub async fn list_loves(&self, user_id: i64) -> Result<Vec<(EntityRow, i64)>, sqlx::Error> {
        let rows: Vec<(i64, String, String, String, Option<String>, i64, i64)> = sqlx::query_as(
            "SELECT e.id, e.kind, e.name, e.artist_name, e.mbid, e.created_at, l.created_at
             FROM loves l JOIN entities e ON e.id = l.entity_id
             WHERE l.user_id = ? ORDER BY l.created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, kind, name, artist_name, mbid, created_at, loved_at)| {
                (EntityRow { id, kind, name, artist_name, mbid, created_at }, loved_at)
            })
            .collect())
    }
    /// Every love, with what the Loved page needs to render a row: how often
    /// you have played the thing, when you last did, its artwork, and whether
    /// there is a liner note on it.
    ///
    /// Four queries regardless of how many loves there are, rather than a
    /// per-row lookup. A love is an *entity* and entities come in three kinds
    /// that live in different columns of `scrobbles`, so there is one aggregate
    /// per kind and they are zipped in Rust on the same `entity_key` the loves
    /// themselves are keyed by — see `Database::entity_key`, and keep the two in
    /// step or hearts and rows stop matching with nothing to show for it.
    ///
    /// The bare `artwork_url` / `caa_id` columns beside `MAX(timestamp)` are
    /// deliberate: SQLite guarantees those come from the row that produced the
    /// max, so a row's cover is the one its *most recent* listen carried. That
    /// is a documented min/max special case, not an accident of grouping.
    pub async fn loves_detailed(&self, user_id: i64) -> Result<Vec<LovedEntity>, sqlx::Error> {
        let loves = self.list_loves(user_id).await?;
        if loves.is_empty() {
            return Ok(Vec::new());
        }

        type Agg = (String, i64, i64, Option<String>, Option<i64>, Option<String>, Option<String>);

        // Recordings and releases differ only in which column names the thing.
        let mut by_key: HashMap<
            String,
            (i64, i64, Option<String>, Option<i64>, Option<String>, Option<String>),
        > = HashMap::new();
        for (kind, name_col) in [("recording", "title"), ("release", "album")] {
            let rows: Vec<Agg> = sqlx::query_as(&format!(
                "SELECT LOWER(TRIM(artist)) || '|' || LOWER(TRIM({name_col})) AS k,
                        COUNT(*) AS plays, MAX(timestamp) AS last_played,
                        artwork_url, caa_id, caa_release_mbid, album
                   FROM scrobbles
                  WHERE user_id = ? AND skipped = FALSE
                    AND {name_col} IS NOT NULL AND TRIM({name_col}) != ''
                  GROUP BY k"
            ))
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
            for (k, plays, last, art, caa, caa_mbid, album) in rows {
                by_key.insert(format!("{kind}|{k}"), (plays, last, art, caa, caa_mbid, album));
            }
        }
        let artist_rows: Vec<(String, i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT LOWER(TRIM(s.artist)) AS k, COUNT(*) AS plays, MAX(s.timestamp) AS last_played,
                    (SELECT a.image_url FROM artists a WHERE a.name_key = LOWER(TRIM(s.artist)))
               FROM scrobbles s
              WHERE s.user_id = ? AND s.skipped = FALSE AND TRIM(s.artist) != ''
              GROUP BY k",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        for (k, plays, last, image) in artist_rows {
            by_key.insert(format!("artist||{k}"), (plays, last, image, None, None, None));
        }

        // Notes are per (user, entity); the excerpt is what the row shows.
        let note_rows: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT entity_id, id, body_md FROM notes
              WHERE user_id = ? AND entity_id IS NOT NULL",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let notes: HashMap<i64, (i64, String)> =
            note_rows.into_iter().map(|(e, id, body)| (e, (id, body))).collect();

        Ok(loves
            .into_iter()
            .map(|(e, loved_at)| {
                let key = Self::entity_key(&e.kind, &e.artist_name, &e.name);
                let agg = by_key.get(&key);
                let note = notes.get(&e.id);
                LovedEntity {
                    id: e.id,
                    kind: e.kind,
                    name: e.name,
                    artist_name: e.artist_name,
                    mbid: e.mbid,
                    loved_at,
                    plays: agg.map(|a| a.0).unwrap_or(0),
                    last_played: agg.map(|a| a.1),
                    artwork_url: agg.and_then(|a| a.2.clone()).filter(|u| !u.is_empty()),
                    caa_id: agg.and_then(|a| a.3),
                    caa_release_mbid: agg.and_then(|a| a.4.clone()),
                    album: agg.and_then(|a| a.5.clone()),
                    note_id: note.map(|n| n.0),
                    note_excerpt: note.map(|n| {
                        let body = n.1.trim().replace('\n', " ");
                        if body.chars().count() > 90 {
                            format!("{}…", body.chars().take(90).collect::<String>())
                        } else {
                            body
                        }
                    }),
                }
            })
            .collect())
    }
    /// Lowest-numbered admin — the migration target for config that used to be
    /// instance-wide and now needs an owner.
    pub async fn first_admin_id(&self) -> Result<Option<i64>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM users WHERE role = 'admin' AND disabled = 0 ORDER BY id LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }
}
