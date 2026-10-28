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
    /// it that knew more. See [`fill_blanks`].
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
        let has_anything = play.album.as_deref().is_some_and(|a| !a.trim().is_empty())
            || play.duration.is_some()
            || play.mbid_recording.is_some()
            || play.mbid_release.is_some()
            || play.mbid_artist.as_ref().is_some_and(|a| !a.is_empty())
            || play.caa_id.is_some()
            || play.caa_release_mbid.is_some()
            || play.track_number.is_some()
            || play.isrc.is_some()
            || play.artists.as_ref().is_some_and(|a| a.len() > 1)
            || provenance.spotify_track_id.is_some();
        if !has_anything {
            return Ok(false);
        }
        let mut conn = self.pool.acquire().await?;
        fill_blanks(&mut conn, id, play, provenance).await
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
        let mut held = self.existing_id(user_id, &play.source_id, &play.source_name).await?;
        if held.is_none() {
            if let Some(previous) = provenance.previous_source_id.as_deref() {
                // Every key an importer wrote before `ImportProvenance` existed
                // was filed under `import`.
                held = self.existing_id(user_id, previous, "import").await?;
            }
        }
        if held.is_none() {
            held = self.fuzzy_existing_id(user_id, play, skipped).await?;
        }
        if let Some(id) = held {
            return Ok(if self.enrich_existing(id, play, &provenance).await? {
                ImportOutcome::Enriched
            } else {
                ImportOutcome::Duplicate
            });
        }
        let mbid_artist_json =
            play.mbid_artist.as_ref().map(|arr| serde_json::to_string(arr).unwrap());
        // The same column and meaning `save_scrobble` writes: everyone credited,
        // when there was more than one. An import never wrote it, so a
        // ListenBrainz listen with a structured credit lost all but its primary.
        let artist_credit = play.artists.as_ref().filter(|a| a.len() > 1).map(|a| a.join("; "));
        sqlx::query(&crate::sql!(
            "INSERT INTO scrobbles (
                user_id, title, artist, artist_credit, album, timestamp, duration, source_id,
                source_name, status, mbid_recording, mbid_release, mbid_release_group, mbid_artist,
                caa_id, caa_release_mbid, isrc, submission_client, track_number, skipped,
                spotify_track_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        ))
        .bind(user_id)
        .bind(&play.title)
        .bind(&play.artist)
        .bind(&artist_credit)
        .bind(&play.album)
        .bind(play.timestamp as i64)
        .bind(play.duration.map(|d| d as i64))
        .bind(&play.source_id)
        .bind(&play.source_name)
        .bind(&play.mbid_recording)
        .bind(&play.mbid_release)
        .bind(&play.mbid_release_group)
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

/// One column a duplicate may write onto the row already held.
#[derive(Debug, PartialEq)]
pub(super) enum Fill {
    Text(&'static str, String),
    Int(&'static str, i64),
}

/// The sparse columns of one stored listen, read on whatever connection the
/// caller holds — the repair runs this inside its transaction.
pub(super) async fn sparse_columns(
    conn: &mut <Db as sqlx::Database>::Connection,
    id: i64,
) -> Result<Option<SparseColumns>, sqlx::Error> {
    sqlx::query_as::<_, SparseColumns>(&crate::sql!(
        "SELECT artist, artist_credit, album, duration, mbid_recording, mbid_release,
                mbid_release_group, mbid_artist, caa_id, caa_release_mbid, track_number,
                isrc, spotify_track_id
           FROM scrobbles WHERE id = ?",
    ))
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
}

/// Merge what `play` knows into the held row `id`, without overwriting
/// anything. Returns whether anything changed.
pub(super) async fn fill_blanks(
    conn: &mut <Db as sqlx::Database>::Connection,
    id: i64,
    play: &Play,
    provenance: &ImportProvenance,
) -> Result<bool, sqlx::Error> {
    let Some(held) = sparse_columns(conn, id).await? else {
        return Ok(false);
    };
    let (fills, credits) = blanks_to_fill(&held, play, provenance);
    if fills.is_empty() {
        return Ok(false);
    }
    let sets: Vec<String> = fills
        .iter()
        .map(|f| match f {
            Fill::Text(c, _) | Fill::Int(c, _) => format!("{c} = ?"),
        })
        .collect();
    let sql = format!("UPDATE scrobbles SET {} WHERE id = ?", sets.join(", "));
    let stmt = crate::sql!(&sql);
    let mut q = sqlx::query(&stmt);
    for f in &fills {
        q = match f {
            Fill::Text(_, v) => q.bind(v.clone()),
            Fill::Int(_, v) => q.bind(*v),
        };
    }
    q.bind(id).execute(&mut *conn).await?;

    // The same rows `link_scrobble_artists` writes, for the credits the held
    // listen did not have. The primary is already there and is skipped by the
    // key.
    for (position, name, mbid) in credits {
        sqlx::query(&crate::sql!(
            "INSERT INTO scrobble_artists (scrobble_id, artist_key, name, position, mbid)
             VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        ))
        .bind(id)
        .bind(Database::artist_key(&name))
        .bind(&name)
        .bind(position as i64)
        .bind(mbid)
        .execute(&mut *conn)
        .await?;
    }
    Ok(true)
}

/// What a duplicate copy may fill on the row already held, and which credits it
/// adds — the one merge rule for an import that meets a listen it has, and for
/// the repair that removes a copy.
///
/// **Only ever fills a blank; never overwrites.** That is what keeps an import
/// from being able to rewrite history you already trust. Artist and title are
/// not touchable at all — they are the identity the row was matched on.
///
/// **A fact goes only where it is true.** Two copies of one play can disagree
/// about the record it was on: Spotify's export said *Sainte-Victoire* where
/// ListenBrainz mapped the same play to the compilation *25 Ans Ultratop*. What
/// describes the *recording* — its MBID, ISRC, length, artists, Spotify id — is
/// true of the play whichever record it came from and always fills a blank.
/// What describes the *release* — its MBID and group, cover art, track number
/// — is filled only when both copies name the same album (or the held one names
/// none, and takes the copy's; or neither names one). Filling them across albums would put a
/// compilation's cover on the studio album and a 31 on its track number.
///
/// **Credits merge when the copy credits the held artist.** The export names
/// the primary ("A.R. Rahman"), ListenBrainz everyone ("A.R. Rahman, Lady
/// Kash"); when the held artist is one of the copy's credited names, the held
/// listen gains the others. A copy crediting different people adds nothing.
///
/// `caa_id = -1` counts as blank: it is the tombstone for "asked the Cover Art
/// Archive, nothing there", and a real id is the answer it was standing in for.
pub(super) fn blanks_to_fill(
    held: &SparseColumns,
    play: &Play,
    provenance: &ImportProvenance,
) -> (Vec<Fill>, Vec<(usize, String, Option<String>)>) {
    let blank = |s: &Option<String>| s.as_deref().map(str::trim).unwrap_or("").is_empty();
    let given = |s: &Option<String>| {
        s.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
    };
    let mut fills = Vec::new();

    // ── The recording: true whichever record it was played from ──
    if blank(&held.mbid_recording) {
        if let Some(v) = given(&play.mbid_recording) {
            fills.push(Fill::Text("mbid_recording", v));
        }
    }
    if blank(&held.isrc) {
        if let Some(v) = given(&play.isrc) {
            fills.push(Fill::Text("isrc", v));
        }
    }
    if held.duration.is_none() {
        if let Some(d) = play.duration {
            fills.push(Fill::Int("duration", d as i64));
        }
    }
    // Positional, like the credits it belongs to: element N is the Nth credited
    // artist. So it crosses only between rows naming the same artist — filled
    // from a copy credited "Boyz II Men, Mariah Carey", a "Mariah Carey"
    // listen would carry Boyz II Men's id as hers. "[]" is a stored empty
    // list, which knows no more than NULL does.
    let held_artists_blank = artist_matches(&play.artist, &held.artist)
        && held
            .mbid_artist
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .trim_matches(['[', ']', ' '])
            .is_empty();
    if held_artists_blank {
        if let Some(a) = play.mbid_artist.as_ref().filter(|a| !a.is_empty()) {
            fills.push(Fill::Text(
                "mbid_artist",
                serde_json::to_string(a).unwrap_or_else(|_| "[]".into()),
            ));
        }
    }
    if blank(&held.spotify_track_id) {
        if let Some(v) = given(&provenance.spotify_track_id) {
            fills.push(Fill::Text("spotify_track_id", v));
        }
    }

    // ── The release: only with the album it describes ──
    let incoming_album = given(&play.album);
    // Neither naming an album leaves nothing for a release id to contradict;
    // a copy naming none against a held album does not say which it meant.
    let same_album = match (&incoming_album, given(&held.album)) {
        (Some(a), Some(h)) => normalise_artist(a) == normalise_artist(&h),
        (Some(_), None) | (None, None) => true,
        (None, Some(_)) => false,
    };
    if blank(&held.album) {
        if let Some(a) = incoming_album {
            fills.push(Fill::Text("album", a));
        }
    }
    if same_album {
        if blank(&held.mbid_release) {
            if let Some(v) = given(&play.mbid_release) {
                fills.push(Fill::Text("mbid_release", v));
            }
        }
        if blank(&held.mbid_release_group) {
            if let Some(v) = given(&play.mbid_release_group) {
                fills.push(Fill::Text("mbid_release_group", v));
            }
        }
        if held.caa_id.is_none_or(|c| c <= 0) {
            if let Some(c) = play.caa_id.filter(|c| *c > 0) {
                fills.push(Fill::Int("caa_id", c));
            }
        }
        if blank(&held.caa_release_mbid) {
            if let Some(v) = given(&play.caa_release_mbid) {
                fills.push(Fill::Text("caa_release_mbid", v));
            }
        }
        if held.track_number.is_none() {
            if let Some(n) = play.track_number {
                fills.push(Fill::Int("track_number", n as i64));
            }
        }
    }

    // ── Who else is credited ──
    let mut credits = Vec::new();
    if blank(&held.artist_credit) {
        if let Some(names) = play.artists.as_ref().filter(|a| a.len() > 1) {
            if names.iter().any(|n| artist_matches(n, &held.artist)) {
                fills.push(Fill::Text("artist_credit", names.join("; ")));
                let mbids = play.mbid_artist.as_deref().unwrap_or(&[]);
                credits = names
                    .iter()
                    .enumerate()
                    .map(|(i, n)| {
                        (i, n.clone(), mbids.get(i).filter(|m| !m.trim().is_empty()).cloned())
                    })
                    .collect();
            }
        }
    }
    (fills, credits)
}

/// A stored listen as a `Play`, carrying the columns a merge reads — so the
/// repair can hand a copy it is about to delete to the same rule an import
/// uses.
pub(super) fn play_from_sparse(title: &str, s: &SparseColumns) -> Play {
    let mut p = Play::new(title.into(), s.artist.clone(), 0, String::new(), String::new());
    p.album = s.album.clone();
    p.duration = s.duration.map(|d| d.max(0) as u64);
    p.mbid_recording = s.mbid_recording.clone();
    p.mbid_release = s.mbid_release.clone();
    p.mbid_release_group = s.mbid_release_group.clone();
    p.mbid_artist = s.mbid_artist.as_deref().and_then(|j| serde_json::from_str(j).ok());
    p.caa_id = s.caa_id;
    p.caa_release_mbid = s.caa_release_mbid.clone();
    p.track_number = s.track_number.map(|n| n as i32);
    p.isrc = s.isrc.clone();
    p.artists = s
        .artist_credit
        .as_deref()
        .map(|c| {
            c.split(';').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect::<Vec<_>>()
        })
        .filter(|a| a.len() > 1);
    p
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    /// The play as the Spotify export holds it: its own album, no MBIDs.
    fn held() -> SparseColumns {
        SparseColumns {
            artist: "Clara Luciani".into(),
            artist_credit: None,
            album: Some("Sainte-Victoire".into()),
            duration: Some(194),
            mbid_recording: None,
            mbid_release: None,
            mbid_release_group: None,
            mbid_artist: None,
            caa_id: None,
            caa_release_mbid: None,
            track_number: None,
            isrc: None,
            spotify_track_id: Some("7ixCRBD0FZMRBeOBhTu2KD".into()),
        }
    }

    /// The same play as ListenBrainz mapped it — to a compilation.
    fn relayed() -> Play {
        let mut p =
            Play::new("La grenade".into(), "Clara Luciani".into(), 0, String::new(), String::new());
        p.album = Some("25 Ans Ultratop".into());
        p.duration = Some(194);
        p.mbid_recording = Some("68dfb551-6462-4f77-a2cb-71271f4733d2".into());
        p.mbid_release = Some("e653264c-420a-4116-8552-1f1f1f70c3d1".into());
        p.mbid_artist = Some(vec!["a6386935-e6ee-4aaa-aaa8-70f56b79bc29".into()]);
        p.caa_id = Some(25015720640);
        p.caa_release_mbid = Some("4aeeff6b-104c-41c7-9f96-854ea0718be0".into());
        p.track_number = Some(31);
        p
    }

    fn columns(fills: &[Fill]) -> Vec<&'static str> {
        fills
            .iter()
            .map(|f| match f {
                Fill::Text(c, _) | Fill::Int(c, _) => *c,
            })
            .collect()
    }

    /// The recording is the same whichever record it was played from; the
    /// compilation's release, cover and track 31 belong to the compilation.
    #[test]
    fn recording_facts_cross_albums_and_release_facts_do_not() {
        let (fills, credits) = blanks_to_fill(&held(), &relayed(), &ImportProvenance::default());
        assert_eq!(columns(&fills), vec!["mbid_recording", "mbid_artist"]);
        assert!(credits.is_empty());
    }

    /// The same album however it is spelled, or none held at all: the release
    /// facts come too.
    #[test]
    fn release_facts_come_with_their_album() {
        let mut h = held();
        h.album = Some("25 ans ULTRATOP".into());
        let (fills, _) = blanks_to_fill(&h, &relayed(), &ImportProvenance::default());
        assert_eq!(
            columns(&fills),
            vec![
                "mbid_recording",
                "mbid_artist",
                "mbid_release",
                "caa_id",
                "caa_release_mbid",
                "track_number"
            ]
        );

        h.album = None;
        let (fills, _) = blanks_to_fill(&h, &relayed(), &ImportProvenance::default());
        assert!(fills.contains(&Fill::Text("album", "25 Ans Ultratop".into())));
        assert!(fills.contains(&Fill::Int("track_number", 31)));
    }

    /// The export names the primary, ListenBrainz everyone; the held listen
    /// gains the rest. A copy crediting other people gives nothing.
    #[test]
    fn credits_merge_only_onto_an_artist_they_include() {
        let mut h = held();
        h.artist = "A.R. Rahman".into();
        let mut p = relayed();
        p.artists = Some(vec!["A.R. Rahman".into(), "Lady Kash".into()]);
        p.mbid_artist = Some(vec!["rahman-mbid".into(), "kash-mbid".into()]);
        let (fills, credits) = blanks_to_fill(&h, &p, &ImportProvenance::default());
        assert!(fills.contains(&Fill::Text("artist_credit", "A.R. Rahman; Lady Kash".into())));
        assert_eq!(credits[1], (1, "Lady Kash".into(), Some("kash-mbid".into())));

        h.artist = "Mariah Carey".into();
        p.artists = Some(vec!["Boyz II Men".into(), "New York Symphonic Arts Ensemble".into()]);
        let (fills, credits) = blanks_to_fill(&h, &p, &ImportProvenance::default());
        assert!(!columns(&fills).contains(&"artist_credit") && credits.is_empty());
    }

    /// Artist ids are positional; a copy crediting someone else keeps them.
    #[test]
    fn artist_ids_cross_only_between_the_same_artist() {
        let mut h = held();
        h.artist = "Mariah Carey".into();
        let mut p = relayed();
        p.artist = "Boyz II Men, Mariah Carey".into();
        let (fills, _) = blanks_to_fill(&h, &p, &ImportProvenance::default());
        assert!(!columns(&fills).contains(&"mbid_artist"));
    }

    /// Nothing held is ever replaced, and a tombstoned cover is a blank.
    #[test]
    fn nothing_held_is_overwritten() {
        let mut h = held();
        h.album = Some("25 Ans Ultratop".into());
        h.mbid_recording = Some("other-recording".into());
        h.mbid_release = Some("other-release".into());
        h.mbid_artist = Some(r#"["someone"]"#.into());
        h.caa_id = Some(-1);
        h.caa_release_mbid = Some("x".into());
        h.track_number = Some(2);
        let (fills, _) = blanks_to_fill(&h, &relayed(), &ImportProvenance::default());
        assert_eq!(fills, vec![Fill::Int("caa_id", 25015720640)]);
    }
}
