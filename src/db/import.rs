//! Listen edit, delete, import and export.
//!
//! `delete_scrobble` and `clear_scrobbles` must clear listen-attached notes
//! and `scrobble_artists` first — the pool opens with `foreign_keys(true)`.

use super::*;

impl Database {
    /// PATCH a listen. Every field is optional; only provided (`Some`) fields
    /// change (COALESCE keeps the existing value on NULL). Scoped by user_id.
    /// Returns false if no row matched (wrong id or not owned by user).
    #[allow(clippy::too_many_arguments)]
    pub async fn update_scrobble(
        &self,
        user_id: i64,
        id: i64,
        title: Option<&str>,
        artist: Option<&str>,
        album: Option<&str>,
        mbid_recording: Option<&str>,
        mbid_release: Option<&str>,
        chain_id: Option<i64>,
        chain_variant_id: Option<i64>,
        listening_context: Option<&str>,
        format_type: Option<&str>,
        codec: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        // Changing the release means the artwork we resolved is for the wrong
        // record, so the tombstones are cleared and the backfill picks the row
        // up again. This is the whole point of being able to fix an MBID by
        // hand: the cover is what made the mistake visible in the first place.
        let clear_artwork = mbid_release.is_some();

        let res = sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET
                title = COALESCE(?, title),
                artist = COALESCE(?, artist),
                album = COALESCE(?, album),
                mbid_recording = COALESCE(?, mbid_recording),
                mbid_release = COALESCE(?, mbid_release),
                chain_id = COALESCE(?, chain_id),
                -- **Not `COALESCE`.** The variant is rewritten whenever the
                -- chain is set, including to nothing: a listen moved from chain
                -- A to chain B must not keep a variant belonging to A, which
                -- would pair it with components that were never fitted
                -- together. `equipment_seconds` ignores such a pairing, so the
                -- hours stayed right and only the displayed setup was wrong —
                -- which is the kind of wrongness nobody notices. Sessions have
                -- rewritten both together since v0.54.0; listens did not.
                chain_variant_id = CASE WHEN ? IS NOT NULL THEN ? ELSE chain_variant_id END,
                listening_context = COALESCE(?, listening_context),
                format_type = COALESCE(?, format_type),
                codec = COALESCE(?, codec),
                artwork_url = CASE WHEN ? THEN NULL ELSE artwork_url END,
                caa_id = CASE WHEN ? THEN NULL ELSE caa_id END,
                caa_release_mbid = CASE WHEN ? THEN NULL ELSE caa_release_mbid END
             WHERE id = ? AND user_id = ?"
        ))
        .bind(title)
        .bind(artist)
        .bind(album)
        .bind(mbid_recording)
        .bind(mbid_release)
        .bind(chain_id)
        .bind(chain_id)
        .bind(chain_variant_id)
        .bind(listening_context)
        .bind(format_type)
        .bind(codec)
        .bind(clear_artwork)
        .bind(clear_artwork)
        .bind(clear_artwork)
        .bind(id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Delete a single listen owned by the user. Returns false if not found.
    ///
    /// Foreign keys are on, so a note attached to this listen has to go first —
    /// otherwise the delete fails outright. The note is about *this playing* of
    /// the track, so it has nothing to survive onto; notes about the record
    /// itself hang off an entity and are untouched.
    pub async fn delete_scrobble(&self, user_id: i64, id: i64) -> Result<bool, sqlx::Error> {
        self.delete_notes_for_scrobbles(user_id, Some(id)).await?;
        self.delete_artists_for_scrobbles(user_id, Some(id)).await?;
        let res = sqlx::query(&crate::sql!("DELETE FROM scrobbles WHERE id = ? AND user_id = ?"))
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }
    /// Delete ALL listens for the user. Returns the number removed.
    pub async fn clear_scrobbles(&self, user_id: i64) -> Result<u64, sqlx::Error> {
        self.delete_notes_for_scrobbles(user_id, None).await?;
        self.delete_artists_for_scrobbles(user_id, None).await?;
        let res = sqlx::query(&crate::sql!("DELETE FROM scrobbles WHERE user_id = ?"))
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }
    /// Clear the credit rows for one listen, or for every listen this user has.
    ///
    /// The pool opens with `foreign_keys(true)`, so `scrobble_artists.scrobble_id`
    /// makes this mandatory before any listen is deleted — exactly the rule
    /// listen-attached notes already follow. The `delete_user_cascades_their_data`
    /// test caught this the moment the table was added, which is what it is for.
    async fn delete_artists_for_scrobbles(
        &self,
        user_id: i64,
        id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        match id {
            Some(id) => {
                sqlx::query(&crate::sql!(
                    "DELETE FROM scrobble_artists WHERE scrobble_id IN
                         (SELECT id FROM scrobbles WHERE id = ? AND user_id = ?)"
                ))
                .bind(id)
                .bind(user_id)
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query(&crate::sql!(
                    "DELETE FROM scrobble_artists WHERE scrobble_id IN
                         (SELECT id FROM scrobbles WHERE user_id = ?)"
                ))
                .bind(user_id)
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }
    /// Clear listen-attached notes (and their revisions) for one listen, or for
    /// every listen this user has when `id` is None.
    async fn delete_notes_for_scrobbles(
        &self,
        user_id: i64,
        id: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        let ids: Vec<(i64,)> = match id {
            Some(id) => {
                sqlx::query_as(&crate::sql!(
                    "SELECT id FROM notes WHERE user_id = ? AND scrobble_id = ?"
                ))
                .bind(user_id)
                .bind(id)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(&crate::sql!(
                    "SELECT id FROM notes WHERE user_id = ? AND scrobble_id IS NOT NULL",
                ))
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?
            }
        };
        for (note_id,) in ids {
            sqlx::query(&crate::sql!("DELETE FROM note_revisions WHERE note_id = ?"))
                .bind(note_id)
                .execute(&self.pool)
                .await?;
            sqlx::query(&crate::sql!("DELETE FROM notes WHERE id = ?"))
                .bind(note_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
    /// A single listen by id, scoped to the user.
    pub async fn get_scrobble(
        &self,
        user_id: i64,
        id: i64,
    ) -> Result<Option<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>(&crate::sql!(
            "SELECT * FROM scrobbles WHERE id = ? AND user_id = ?",
        ))
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
    }
    /// Every listen for a user, newest first — for full-backup export.
    pub async fn get_all_scrobbles(
        &self,
        user_id: i64,
    ) -> Result<Vec<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>(&crate::sql!(
            "SELECT * FROM scrobbles WHERE user_id = ? ORDER BY timestamp DESC",
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }
    /// Insert a historical listen from an import. Deduped like `save_scrobble`,
    /// but inserted as `imported` (not `pending`) so the outbound sink engine
    /// never re-scrobbles a back-fill to Last.fm/ListenBrainz. Returns true if
    /// a new row was inserted.
    pub async fn import_listen(
        &self,
        user_id: i64,
        play: &Play,
    ) -> Result<ImportOutcome, sqlx::Error> {
        self.import_listen_flagged(user_id, play, false).await
    }
    /// Import a listen, optionally marking it a skip.
    ///
    /// Skips are stored but never counted: they are excluded from listen counts,
    /// analytics and the history listing, and because imports land as `imported`
    /// rather than `pending` they are never forwarded to Last.fm either — a
    /// forwarded skip would be a wrong scrobble on a permanent record.
    pub async fn import_listen_flagged(
        &self,
        user_id: i64,
        play: &Play,
        skipped: bool,
    ) -> Result<ImportOutcome, sqlx::Error> {
        self.import_listen_detailed(user_id, play, skipped, ImportProvenance::default()).await
    }
    /// Fill in what an already-held listen never knew, from a duplicate copy of
    /// it that knew more.
    ///
    /// An import used to drop a duplicate whole, which threw away every field
    /// the incoming copy carried. A ListenBrainz archive holds a full
    /// `mbid_mapping`, so re-importing a history you already have discarded
    /// free, authoritative metadata — and the enrichment backfill then went and
    /// bought it again through a rate-limited MusicBrainz search whose ranking
    /// is, by this codebase's own measurement, not even stable. Measured on a
    /// bare listen: the export's recording id was dropped and enrichment
    /// resolved a *different* one.
    ///
    /// **Only ever fills a blank; never overwrites.** That is what keeps an
    /// import from being able to rewrite history you already trust, which is the
    /// property that made dropping duplicates safe in the first place. Artist
    /// and title are not touchable at all — they are the identity the row was
    /// matched on.
    ///
    /// `caa_id = -1` counts as blank. It is the tombstone meaning "asked the
    /// Cover Art Archive, nothing there", and an import that carries a real id
    /// is precisely the answer that tombstone was standing in for. `artwork_url
    /// = ''` is the same idea but no import format carries one, so there is
    /// nothing to fill it with.
    ///
    /// Returns whether anything actually changed, so the caller can report
    /// enrichment separately from a plain duplicate.
    async fn enrich_existing(
        &self,
        id: i64,
        play: &Play,
        provenance: &ImportProvenance,
    ) -> Result<bool, sqlx::Error> {
        // Nothing to give: skip the read entirely. A Rockbox log carries no
        // MBIDs at all, so on that path this costs one comparison per listen
        // rather than a query.
        let incoming_mbid_artist = play
            .mbid_artist
            .as_ref()
            .filter(|a| !a.is_empty())
            .map(|a| serde_json::to_string(a).unwrap_or_else(|_| "[]".into()));
        let has_anything = play.album.as_deref().is_some_and(|a| !a.trim().is_empty())
            || play.duration.is_some()
            || play.mbid_recording.is_some()
            || play.mbid_release.is_some()
            || incoming_mbid_artist.is_some()
            || play.caa_id.is_some()
            || play.caa_release_mbid.is_some()
            || play.track_number.is_some()
            || provenance.spotify_track_id.is_some();
        if !has_anything {
            return Ok(false);
        }

        let Some(row) = sqlx::query_as::<_, SparseColumns>(&crate::sql!(
            "SELECT album, duration, mbid_recording, mbid_release, mbid_artist,
                    caa_id, caa_release_mbid, track_number, spotify_track_id
               FROM scrobbles WHERE id = ?",
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(false);
        };

        let mut sets: Vec<&str> = Vec::new();
        let blank = |s: &Option<String>| s.as_deref().map(str::trim).unwrap_or("").is_empty();

        let album = play
            .album
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .filter(|_| blank(&row.album));
        if album.is_some() {
            sets.push("album = ?");
        }
        let duration = play.duration.filter(|_| row.duration.is_none()).map(|d| d as i64);
        if duration.is_some() {
            sets.push("duration = ?");
        }
        let rec = play.mbid_recording.as_deref().filter(|_| blank(&row.mbid_recording));
        if rec.is_some() {
            sets.push("mbid_recording = ?");
        }
        let rel = play.mbid_release.as_deref().filter(|_| blank(&row.mbid_release));
        if rel.is_some() {
            sets.push("mbid_release = ?");
        }
        let art = incoming_mbid_artist.filter(|_| {
            // "[]" is a stored empty list, which knows no more than NULL does.
            row.mbid_artist
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .trim_matches(['[', ']', ' '])
                .is_empty()
        });
        if art.is_some() {
            sets.push("mbid_artist = ?");
        }
        // -1 is the "nothing in the archive" tombstone, which a real id supersedes.
        let caa = play.caa_id.filter(|_| row.caa_id.is_none_or(|c| c <= 0));
        if caa.is_some() {
            sets.push("caa_id = ?");
        }
        let caa_rel = play.caa_release_mbid.as_deref().filter(|_| blank(&row.caa_release_mbid));
        if caa_rel.is_some() {
            sets.push("caa_release_mbid = ?");
        }
        let track = play.track_number.filter(|_| row.track_number.is_none());
        if track.is_some() {
            sets.push("track_number = ?");
        }
        let spotify =
            provenance.spotify_track_id.as_deref().filter(|_| blank(&row.spotify_track_id));
        if spotify.is_some() {
            sets.push("spotify_track_id = ?");
        }

        // Everything it had, we had. One comparison and no write — which is the
        // common case on a re-import and must not cost a statement.
        if sets.is_empty() {
            return Ok(false);
        }

        let sql = format!("UPDATE scrobbles SET {} WHERE id = ?", sets.join(", "));
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query(&stmt);
        if let Some(v) = album {
            q = q.bind(v.to_string());
        }
        if let Some(v) = duration {
            q = q.bind(v);
        }
        if let Some(v) = rec {
            q = q.bind(v.to_string());
        }
        if let Some(v) = rel {
            q = q.bind(v.to_string());
        }
        if let Some(v) = art {
            q = q.bind(v);
        }
        if let Some(v) = caa {
            q = q.bind(v);
        }
        if let Some(v) = caa_rel {
            q = q.bind(v.to_string());
        }
        if let Some(v) = track {
            q = q.bind(v);
        }
        if let Some(v) = spotify {
            q = q.bind(v.to_string());
        }
        q.bind(id).execute(&self.pool).await?;
        Ok(true)
    }
    /// Import a listen, keeping what the export knew about *where* it happened.
    ///
    /// Additive over `import_listen_flagged`: every field here is something one
    /// export format carries and the others don't, so the plain call stays the
    /// one to use unless you have something to put in these.
    pub async fn import_listen_detailed(
        &self,
        user_id: i64,
        play: &Play,
        skipped: bool,
        provenance: ImportProvenance,
    ) -> Result<ImportOutcome, sqlx::Error> {
        // A duplicate is no longer the end of it. The incoming copy may know
        // things the stored one never did — a ListenBrainz archive carries a
        // full `mbid_mapping` — and dropping it whole meant the enrichment
        // backfill later bought the same facts through a rate-limited search.
        // Nothing is ever overwritten; see `enrich_existing`.
        let held = match self.existing_id(user_id, &play.source_id, &play.source_name).await? {
            Some(id) => Some(id),
            None => self.fuzzy_existing_id(user_id, play, skipped).await?,
        };
        if let Some(id) = held {
            return Ok(if self.enrich_existing(id, play, &provenance).await? {
                ImportOutcome::Enriched
            } else {
                ImportOutcome::Duplicate
            });
        }
        let mbid_artist_json =
            play.mbid_artist.as_ref().map(|arr| serde_json::to_string(arr).unwrap());
        sqlx::query(&crate::sql!(
            "INSERT INTO scrobbles (
                user_id, title, artist, album, timestamp, duration, source_id, source_name, status,
                mbid_recording, mbid_release, mbid_artist, caa_id, caa_release_mbid, isrc,
                submission_client, track_number, skipped, spotify_track_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(user_id)
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
        .bind(&play.isrc)
        .bind(provenance.submission_client.as_deref().unwrap_or("import"))
        .bind(play.track_number)
        .bind(skipped)
        .bind(&provenance.spotify_track_id)
        .execute(&self.pool)
        .await?;

        // An import writes its own INSERT rather than going through
        // `save_scrobble`, so it has to link its credits itself. Missing this
        // does not degrade gracefully: a listen with no row in
        // `scrobble_artists` is invisible to every artist page and every chart,
        // because those fan out over that table now. The startup backfill only
        // covers what was already stored, so an import run *after* the upgrade
        // would produce listens that simply do not appear anywhere.
        self.link_scrobble_artists(user_id, &play.source_id, &play.source_name, play).await?;

        Ok(ImportOutcome::Inserted)
    }
}
