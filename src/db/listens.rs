//! Writing and reading listens.
//!
//! `exists` and `fuzzy_exists` deliberately carry **no** `skipped = FALSE`
//! filter: a skipped listen still occupies its `source_id` and must still
//! dedup. Everything that *counts* listens must filter; these two must not.

use super::*;

impl Database {
    /// Store one listen.
    ///
    /// `disposition` decides whether the row is ever forwarded. It used to be
    /// hardcoded to `'pending'`, which meant an ingest-path backfill
    /// (`listen_type: "import"`) was indistinguishable from a live listen and
    /// got scrobbled onward — rewriting the user's real Last.fm history. Pass
    /// `ListenDisposition::LIVE` only for something that just played.
    #[allow(clippy::too_many_arguments)]
    pub async fn save_scrobble(
        &self,
        user_id: i64,
        play: &Play,
        quality: Option<&AudioQuality>,
        device_id: Option<i64>,
        chain_id: Option<i64>,
        // Which variant of that chain was fitted. Sits immediately beside
        // `chain_id` because the two only mean anything together: a variant
        // belongs to exactly one chain, and pairing one with a different chain
        // resolves to components that were never fitted at the same time.
        //
        // Almost always `None` at write time, and that is correct — a scrobble
        // client has no idea which stylus is in the headshell. The paths that
        // legitimately know are the shelf (you told it) and retagging a sitting
        // afterwards.
        chain_variant_id: Option<i64>,
        listening_context: Option<&str>,
        submission_client: Option<&str>,
        source_ref: Option<i64>,
        disposition: ListenDisposition,
    ) -> Result<bool, sqlx::Error> {
        if self.exists(user_id, &play.source_id, &play.source_name).await? {
            return Ok(false);
        }
        if self.fuzzy_exists(user_id, play, disposition.skipped).await? {
            debug!("Skipping duplicate play (fuzzy match): {} - {}", play.artist, play.title);
            return Ok(false);
        }
        let mbid_artist_json =
            play.mbid_artist.as_ref().map(|arr| serde_json::to_string(arr).unwrap());
        let mbid_album_artist_json =
            play.mbid_album_artist.as_ref().map(|arr| serde_json::to_string(arr).unwrap());
        let mut q = quality.cloned().unwrap_or_default();
        // **One format is one spelling**, folded here because this is the one
        // place every write path goes through: ingest, Plex, Jellyfin/Emby,
        // Navidrome, Roon, the shelf and the importers. Each of those quoted
        // whatever its upstream called the format and stored it raw, so `FLAC`
        // and `flac` were two grouping keys. Doing it per source is six places
        // to remember and one to forget.
        q.normalise();

        // Everyone credited, when the source named more than one. `artist` is
        // the primary — for a film soundtrack the music director, which is how
        // the record is filed and what gets forwarded — and this keeps the
        // performers rather than discarding them. Same column and same meaning
        // as `fold_artist_credit`: the fuller form of the credit.
        let artist_credit = play.artists.as_ref().filter(|a| a.len() > 1).map(|a| a.join("; "));

        sqlx::query(&crate::sql!(
            "INSERT INTO scrobbles (
                user_id, title, artist, artist_credit, album, timestamp, duration,
                source_id, source_name, status, skipped,
                mbid_recording, mbid_release, mbid_artist,
                mbid_release_group, mbid_release_track, mbid_work, mbid_album_artist,
                caa_id, caa_release_mbid, isrc,
                format_type, codec, bitrate, sample_rate, bit_depth, channels,
                container, source_quality, is_lossless,
                dsd_rate, dsd_multiplier, dsd_to_pcm_converted,
                delivery_codec, delivery_bitrate, delivery_sample_rate, delivery_bit_depth,
                is_transcoded, transcode_reason, quality_score,
                device_id, chain_id, chain_variant_id, listening_context, submission_client,
                track_number, source_ref
            ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?
            )"
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
        .bind(disposition.status())
        .bind(disposition.skipped)
        .bind(&play.mbid_recording)
        .bind(&play.mbid_release)
        .bind(&mbid_artist_json)
        .bind(&play.mbid_release_group)
        .bind(&play.mbid_release_track)
        .bind(&play.mbid_work)
        .bind(&mbid_album_artist_json)
        .bind(play.caa_id)
        .bind(&play.caa_release_mbid)
        .bind(&play.isrc)
        .bind(&q.format_type)
        .bind(&q.codec)
        .bind(q.bitrate)
        .bind(q.sample_rate)
        .bind(q.bit_depth.map(|v| v as i32))
        .bind(q.channels.map(|v| v as i32))
        .bind(&q.container)
        .bind(&q.source_quality)
        .bind(q.is_lossless)
        .bind(q.dsd_rate)
        .bind(q.dsd_multiplier.map(|v| v as i32))
        .bind(q.dsd_to_pcm_converted)
        .bind(&q.delivery_codec)
        .bind(q.delivery_bitrate)
        .bind(q.delivery_sample_rate)
        .bind(q.delivery_bit_depth)
        .bind(q.is_transcoded)
        .bind(&q.transcode_reason)
        .bind(q.quality_score)
        .bind(device_id)
        .bind(chain_id)
        .bind(chain_variant_id)
        .bind(listening_context)
        .bind(submission_client)
        .bind(play.track_number)
        .bind(source_ref)
        .execute(&self.pool)
        .await?;

        // Who actually played it. Written here rather than at each source,
        // because there are six write paths and a source that forgot would
        // produce a listen whose performers are simply absent from every chart
        // — with nothing erroring to say so, which is how the credit was lost
        // in the first place.
        self.link_scrobble_artists(user_id, &play.source_id, &play.source_name, play).await?;

        // Offer it to anyone else in the sitting, if there is one.
        //
        // **Live and unskipped only**, and the guard is here rather than inside
        // `capture_into_spool` so the cost is visible at the call site: an
        // import must not fan out, or twenty thousand backfilled rows become
        // twenty thousand offers in somebody's inbox — a backfill is not a
        // sitting. A skip is not offered either, for the reason a skip is never
        // counted anywhere else.
        //
        // Costs one indexed lookup returning no rows when nothing is running,
        // which is almost always. That is affordable per *live* listen — a
        // handful an hour — and would not be per imported one.
        if !disposition.imported && !disposition.skipped {
            self.capture_into_spool(user_id, play, chain_id).await?;
        }

        Ok(true)
    }
    /// Record what a pushing client observed about how the track played.
    ///
    /// A follow-up write rather than three more arguments on `save_scrobble`,
    /// and both halves of that are deliberate.
    ///
    /// `save_scrobble` has 53 call sites. Threading positional `Option`s
    /// through all of them is the edit this codebase has already got wrong
    /// once — a variant id landed in `source_ref`'s slot, compiled cleanly
    /// because both are `Option<i64>`, and silently did nothing. Three more
    /// nullable parameters of three different types is the same trap with more
    /// surface.
    ///
    /// The usual objection to insert-then-update is the one `physical.rs` hit:
    /// a row that is briefly `'pending'` can be caught mid-flight by a flush
    /// tick and forwarded. That objection does not reach here, and the reason
    /// is exactly why these fields live apart from [`ListenDisposition`] —
    /// nothing reads them, nothing forwards on them, and no tick is racing for
    /// them. They are a note about the listen, not a decision about it.
    ///
    /// Keyed on `(user_id, source_id, source_name)`, the unique key
    /// `link_scrobble_artists` already uses, rather than a
    /// `last_insert_rowid()`.
    pub async fn set_listen_playback(
        &self,
        user_id: i64,
        source_id: &str,
        source_name: &str,
        report: &PlaybackReport,
    ) -> Result<(), sqlx::Error> {
        if report.is_empty() {
            return Ok(());
        }
        // COALESCE so a client that sends one field does not blank the others.
        sqlx::query(&crate::sql!(
            "UPDATE scrobbles SET
                listened_ms  = COALESCE(?, listened_ms),
                is_shuffle   = COALESCE(?, is_shuffle),
                queue_source = COALESCE(?, queue_source)
             WHERE user_id = ? AND source_id = ? AND source_name = ?"
        ))
        .bind(report.listened_ms)
        .bind(report.is_shuffle)
        .bind(report.queue_source.as_deref())
        .bind(user_id)
        .bind(source_id)
        .bind(source_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Record every artist credited on a listen, primary first.
    ///
    /// Looks the row back up by its `(user_id, source_id, source_name)` unique
    /// key rather than taking a `last_insert_rowid()`: the insert above is
    /// `INSERT OR IGNORE`-shaped in effect (dedup has already run), and reading
    /// the key that is guaranteed unique is what keeps this correct if the
    /// insert ever becomes an upsert.
    pub(super) async fn link_scrobble_artists(
        &self,
        user_id: i64,
        source_id: &str,
        source_name: &str,
        play: &Play,
    ) -> Result<(), sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(&crate::sql!(
            "SELECT id FROM scrobbles WHERE user_id = ? AND source_id = ? AND source_name = ?",
        ))
        .bind(user_id)
        .bind(source_id)
        .bind(source_name)
        .fetch_optional(&self.pool)
        .await?;
        let Some((scrobble_id,)) = row else {
            return Ok(());
        };

        for (position, (name, mbid)) in credited_artists(play).into_iter().enumerate() {
            // `OR IGNORE` rather than a check: the primary key already says one
            // row per artist per listen, and a tag crediting somebody twice is
            // a real thing rather than an error worth failing a listen over.
            sqlx::query(&crate::sql!(
                "INSERT INTO scrobble_artists (scrobble_id, artist_key, name, position, mbid)
                 VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            ))
            .bind(scrobble_id)
            .bind(Database::artist_key(&name))
            .bind(&name)
            .bind(position as i64)
            .bind(mbid)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
    pub async fn exists(
        &self,
        user_id: i64,
        source_id: &str,
        source_name: &str,
    ) -> Result<bool, sqlx::Error> {
        Ok(self.existing_id(user_id, source_id, source_name).await?.is_some())
    }
    /// The same question, answered with the row so a caller can do something
    /// with what is already there. `exists` is the bool form.
    pub async fn existing_id(
        &self,
        user_id: i64,
        source_id: &str,
        source_name: &str,
    ) -> Result<Option<i64>, sqlx::Error> {
        let hit: Option<(i64,)> = sqlx::query_as(&crate::sql!(
            "SELECT id FROM scrobbles WHERE user_id = ? AND source_id = ? AND source_name = ?",
        ))
        .bind(user_id)
        .bind(source_id)
        .bind(source_name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(hit.map(|(id,)| id))
    }
    /// Has this play already been recorded under a different `source_id`?
    ///
    /// The window is the track's own length rather than a flat ten minutes,
    /// because those are two different questions. A duplicate — the same play
    /// arriving from a second code path, such as Plex's history endpoint
    /// re-reporting what the live session already banked — lands within
    /// seconds. A track *on repeat* is a genuinely separate listen and cannot
    /// arrive sooner than one track length later. A fixed ten-minute window
    /// can't tell them apart and silently swallowed every repeat of anything
    /// shorter than ten minutes, which is nearly everything.
    ///
    /// Unknown duration falls back to a conservative minute: better a rare
    /// duplicate than a silently dropped listen.
    /// `skipped` is the **incoming** row's disposition, and a candidate only
    /// matches one of the same kind.
    ///
    /// Skips used to be rare and historical — Rockbox and Spotify imports, and
    /// the shelf — so "no skip filter here" was right and is recorded as a rule
    /// above `exists`. Live sources write them now, which introduces a pair
    /// that must not be confused: a skip and a real play of the same track,
    /// close together. Matching across the two breaks in both directions.
    ///
    /// Dropping the filter entirely would let a stored skip swallow a genuine
    /// listen — skip a two-minute track ten seconds in, start it again and let
    /// it run, and the real listen lands inside the same 60-second window and
    /// is silently discarded off a permanent record. Filtering skips out
    /// entirely breaks the other way: re-running a Spotify import relies on
    /// this to dedup its skips, and that history carried 5,630 of them.
    ///
    /// So it matches like for like. An incoming skip sees stored skips, an
    /// incoming listen sees stored listens, and neither can erase the other.
    pub async fn fuzzy_existing_id(
        &self,
        user_id: i64,
        play: &Play,
        skipped: bool,
    ) -> Result<Option<i64>, sqlx::Error> {
        // A duplicate is a *reporting lag* phenomenon and is therefore roughly
        // constant: the same play arriving from a second code path lands within
        // seconds (measured across a 21k-listen export: 0-51s, median 3s). A
        // repeat is one full track later. So the window is a constant, capped
        // below one track length so a short track's repeat still escapes it.
        //
        // It used to be the duration itself, which put a back-to-back repeat
        // *exactly* on the boundary — and `BETWEEN` is inclusive, so whether a
        // second play survived came down to poll jitter. That is why a track
        // played twice showed up once, intermittently.
        let window = play
            .duration
            .map(|d| (d as i64 * 4 / 5).clamp(20, DUPLICATE_WINDOW_SECS))
            .unwrap_or(DUPLICATE_WINDOW_SECS);
        let timestamp = play.timestamp as i64;
        // **The artist is not a reliable join key, and requiring it to match
        // exactly is what let Plex double-scrobble compilations.**
        //
        // Plex has two paths into this function and they identify the performer
        // differently: the live path takes the *track* artist (`originalTitle`)
        // and the history walk takes the *album* artist. On a normal record
        // those agree. On a compilation the album artist is "Various Artists",
        // so the same listen arrived one second apart as "Jon Pardi" and as
        // "Various Artists", neither row could see the other, and both were
        // banked. Measured on a real history: "Close To Me" from one OST stood
        // at 75 plays under its singer and 21 under the placeholder, which is
        // also why Various Artists read as the nominal top artist.
        //
        // So a row in the window is the same listen when the titles match and
        // the artists are not *known* to be different. Three ways to be sure:
        //
        //   1. the artists fold to the same key — `LOWER(TRIM(…))`, the same
        //      fold `artist_key` and the charts use, so "System of a Down" and
        //      "System of A Down" stop being two listens as well;
        //   2. either side is a Various Artists placeholder, which per
        //      `is_various_artists` means *look at the track* and carries no
        //      performer information at all — comparing it to a real name is
        //      comparing against a null;
        //   3. both carry the same recording MBID, which settles it outright
        //      whatever the two servers chose to call the performer.
        //
        // What this deliberately does **not** do is drop the artist entirely.
        // Two different songs sharing a title within the same minute is rare
        // but real (two covers back to back), and silently swallowing the
        // second is a listen lost off a permanent record.
        //
        // `LOWER(TRIM(...))` in a `WHERE` normally means an unindexable scan —
        // the shape that made Rediscovery quadratic. It is safe here because
        // `user_id` and the timestamp range are tested first and
        // `idx_scrobbles_user_time` serves them, so the fold only ever runs
        // over the handful of rows inside a two-minute slice of one user's
        // history.
        let placeholders = VARIOUS_ARTISTS_KEYS.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "SELECT id FROM scrobbles
              WHERE user_id = ?
                AND skipped = ?
                AND timestamp BETWEEN ? - ? AND ? + ?
                AND LOWER(TRIM(title)) = LOWER(TRIM(?))
                AND (
                      LOWER(TRIM(artist)) = LOWER(TRIM(?))
                   OR LOWER(TRIM(artist)) IN ({ph})
                   OR ? IN ({ph})
                   OR (? IS NOT NULL AND mbid_recording = ?)
                )
              ORDER BY id LIMIT 1",
            ph = placeholders
        );
        let stmt = crate::sql!(&sql);
        let mut q = sqlx::query_as::<_, (i64,)>(&stmt)
            .bind(user_id)
            .bind(skipped)
            .bind(timestamp)
            .bind(window)
            .bind(timestamp)
            .bind(window)
            .bind(&play.title)
            .bind(&play.artist);
        for k in VARIOUS_ARTISTS_KEYS {
            q = q.bind(*k);
        }
        q = q.bind(play.artist.trim().to_lowercase());
        for k in VARIOUS_ARTISTS_KEYS {
            q = q.bind(*k);
        }
        let hit: Option<(i64,)> = q
            .bind(&play.mbid_recording)
            .bind(&play.mbid_recording)
            .fetch_optional(&self.pool)
            .await?;
        Ok(hit.map(|(id,)| id))
    }
    /// The bool form, for the callers that only need to know *whether*.
    pub async fn fuzzy_exists(
        &self,
        user_id: i64,
        play: &Play,
        skipped: bool,
    ) -> Result<bool, sqlx::Error> {
        Ok(self.fuzzy_existing_id(user_id, play, skipped).await?.is_some())
    }
    pub async fn get_pending_scrobbles(&self) -> Result<Vec<PendingScrobble>, sqlx::Error> {
        let records: Vec<ScrobbleRecord> = sqlx::query_as::<_, ScrobbleRecord>(&crate::sql!(
            "SELECT * FROM scrobbles WHERE status = 'pending' ORDER BY timestamp ASC",
        ))
        .fetch_all(&self.pool)
        .await?;

        Ok(records
            .into_iter()
            .map(|r| {
                let mbid_artist =
                    r.mbid_artist.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());
                let delivered = r
                    .delivered_sinks
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                    .unwrap_or_default();
                PendingScrobble {
                    id: r.id,
                    user_id: r.user_id,
                    delivered,
                    play: Play {
                        title: r.title,
                        artist: r.artist,
                        album: r.album,
                        timestamp: r.timestamp as u64,
                        duration: r.duration.map(|d| d as u64),
                        track_number: None,
                        source_id: r.source_id,
                        source_name: r.source_name,
                        mbid_recording: r.mbid_recording,
                        mbid_release: r.mbid_release,
                        mbid_artist,
                        artists: None,
                        mbid_release_group: None,
                        mbid_release_track: None,
                        mbid_work: None,
                        mbid_album_artist: None,
                        caa_id: r.caa_id,
                        caa_release_mbid: r.caa_release_mbid,
                        isrc: r.isrc,
                    },
                }
            })
            .collect())
    }
    /// Append a sink to a scrobble's delivered set (idempotent). Prevents a
    /// retry from re-sending to a sink that already accepted the listen.
    pub async fn mark_sink_delivered(&self, id: i64, sink_name: &str) -> Result<(), sqlx::Error> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as(&crate::sql!("SELECT delivered_sinks FROM scrobbles WHERE id = ?"))
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        let mut set: Vec<String> =
            row.and_then(|(s,)| s).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        if !set.iter().any(|s| s == sink_name) {
            set.push(sink_name.to_string());
            sqlx::query(&crate::sql!("UPDATE scrobbles SET delivered_sinks = ? WHERE id = ?"))
                .bind(serde_json::to_string(&set).unwrap_or_else(|_| "[]".into()))
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }
    /// Persist MusicBrainz IDs onto a stored listen after the engine enriches it
    /// (enrichment moved off the ingest request path — see server/ingest.rs).
    pub async fn update_scrobble_enrichment(
        &self,
        id: i64,
        play: &Play,
    ) -> Result<(), sqlx::Error> {
        let mbid_artist_json =
            play.mbid_artist.as_ref().map(|a| serde_json::to_string(a).unwrap_or_default());
        sqlx::query(&crate::sql!("UPDATE scrobbles SET mbid_recording = ?, mbid_release = ?, mbid_artist = ?, caa_id = ?, caa_release_mbid = ?
             WHERE id = ?"))
        .bind(&play.mbid_recording).bind(&play.mbid_release).bind(&mbid_artist_json)
        .bind(play.caa_id).bind(&play.caa_release_mbid).bind(id)
        .execute(&self.pool).await?;
        Ok(())
    }
    /// Mark a scrobble fully synced (delivered to every enabled sink).
    pub async fn mark_scrobble_synced(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!("UPDATE scrobbles SET status = 'synced' WHERE id = ?"))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Lightweight liveness check for the DB connection (used by /health).
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query(&crate::sql!("SELECT 1")).execute(&self.pool).await?;
        Ok(())
    }
    /// Write a consistent, compacted snapshot of the whole database to `path`
    /// (`VACUUM INTO`), safe to run while the server is live — including under
    /// WAL. The path must be server-controlled; single quotes are escaped since
    /// VACUUM INTO takes a string literal, not a bound parameter.
    pub async fn backup_to(&self, path: &str) -> Result<(), sqlx::Error> {
        let safe = path.replace('\'', "''");
        sqlx::query(&crate::sql!(&format!("VACUUM INTO '{}'", safe))).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn get_recent_scrobbles(
        &self,
        user_id: i64,
        limit: i64,
        offset: i64,
        artist_filter: Option<&str>,
        album_filter: Option<&str>,
        track_filter: Option<&str>,
        after: Option<i64>,
        before: Option<i64>,
        social: Option<bool>,
    ) -> Result<Vec<ScrobbleRecord>, sqlx::Error> {
        sqlx::query_as::<_, ScrobbleRecord>(&crate::sql!(
            "SELECT * FROM scrobbles WHERE user_id = ? AND skipped = FALSE
               AND (? IS NULL OR LOWER(artist) LIKE '%' || LOWER(?) || '%')
               AND (? IS NULL OR LOWER(album) LIKE '%' || LOWER(?) || '%')
               AND (? IS NULL OR LOWER(title) LIKE '%' || LOWER(?) || '%')
               AND (? IS NULL OR timestamp >= ?)
               AND (? IS NULL OR timestamp <= ?)
               AND (? IS NULL OR (company IS NOT NULL) = ?)
             ORDER BY timestamp DESC LIMIT ? OFFSET ?",
        ))
        .bind(user_id)
        .bind(artist_filter)
        .bind(artist_filter)
        .bind(album_filter)
        .bind(album_filter)
        .bind(track_filter)
        .bind(track_filter)
        .bind(after)
        .bind(after)
        .bind(before)
        .bind(before)
        .bind(social)
        .bind(social)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }
    /// Search a user's own listening history by name.
    ///
    /// Deliberately over `scrobbles` rather than `entities`: entities only exist
    /// for things already annotated or loved, so searching them would find only
    /// what you've already been to. Grouped rather than a flat row list because
    /// "that Radiohead album" is an album, not fifty listens of it.
    #[allow(clippy::type_complexity)]
    pub async fn search_library(
        &self,
        user_id: i64,
        q: &str,
        limit: i64,
    ) -> Result<
        (
            Vec<(String, i64)>,
            Vec<(String, String, i64)>,
            Vec<(String, String, Option<String>, i64)>,
        ),
        sqlx::Error,
    > {
        let needle = q.trim().to_lowercase();
        if needle.is_empty() {
            return Ok((Vec::new(), Vec::new(), Vec::new()));
        }

        let artists: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(&format!(
            "SELECT artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(artist) LIKE '%' || ? || '%'
               AND LOWER(TRIM(artist)) {NOT_VARIOUS}
             GROUP BY LOWER(artist) ORDER BY plays DESC LIMIT ?"
        )))
        .bind(user_id)
        .bind(&needle)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let albums: Vec<(String, String, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT album, artist, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND album IS NOT NULL AND album != ''
               AND LOWER(album) LIKE '%' || ? || '%'
             GROUP BY LOWER(album), LOWER(artist) ORDER BY plays DESC LIMIT ?"
        ))
        .bind(user_id)
        .bind(&needle)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let tracks: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(&crate::sql!(
            "SELECT title, artist, album, COUNT(*) AS plays FROM scrobbles
             WHERE user_id = ? AND skipped = FALSE AND LOWER(title) LIKE '%' || ? || '%'
             GROUP BY LOWER(title), LOWER(artist) ORDER BY plays DESC LIMIT ?"
        ))
        .bind(user_id)
        .bind(&needle)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok((artists, albums, tracks))
    }

    /// Move listens in time.
    ///
    /// **An offset over a selection is the shape that matters**, and the
    /// reason is that it is the shape of the actual failure. A timestamp is
    /// almost never wrong on its own: a phone's clock was an hour out, an
    /// evening was reconstructed from a session whose times were laid out by
    /// arithmetic, or a capture arrived carrying the host's clock rather than
    /// yours. Every one of those moves a *run* of listens by a *constant*, and
    /// an offset cannot scramble an ordering the way a set of hand-typed
    /// absolute times can. A single listen is the same operation over one row.
    ///
    /// Four things it takes care of, in the order they bite:
    ///
    /// * **`timestamp_original` is written once**, so a second correction does
    ///   not overwrite the observation the first one replaced. What is worth
    ///   keeping is what the clock said, not the previous guess at what it
    ///   should have said.
    /// * **Nothing is moved onto another listen of the same track.** The unique
    ///   key is `(user_id, source_id, source_name)`, so the database will
    ///   happily hold two rows a second apart that `fuzzy_exists` would have
    ///   collapsed on the way in. Dedup runs at ingest and never again, so a
    ///   duplicate created here is one nothing will ever reconcile.
    /// * **Sessions are recomputed**, but only their aggregates — see
    ///   [`TimeEdit::grouping_stale`].
    /// * **Already-forwarded rows are counted and reported**, not refused.
    ///
    /// One transaction, because a half-applied shift leaves an ordering that
    /// was never true at any point.
    pub async fn shift_listen_times(
        &self,
        user_id: i64,
        ids: &[i64],
        offset: i64,
    ) -> Result<TimeEdit, sqlx::Error> {
        if ids.is_empty() || offset == 0 {
            return Ok(TimeEdit::default());
        }
        let marks = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");

        let sql = format!(
            "SELECT id, timestamp, session_id, status, title, artist
               FROM scrobbles WHERE user_id = ? AND id IN ({marks})"
        );
        let stmt = crate::sql!(&sql);
        #[allow(clippy::type_complexity)]
        let mut q = sqlx::query_as::<_, (i64, i64, Option<i64>, String, String, String)>(&stmt)
            .bind(user_id);
        for id in ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(&self.pool).await?;
        if rows.is_empty() {
            return Ok(TimeEdit::default());
        }

        let mut edit = TimeEdit::default();
        let mut sessions: Vec<i64> = Vec::new();
        let mut tx = self.pool.begin().await?;

        for (id, timestamp, session_id, status, title, artist) in &rows {
            let moved_to = timestamp + offset;

            // Would this land on top of another listen of the same track? The
            // window is the one `fuzzy_exists` uses, and this is the only place
            // that question gets asked after ingest.
            let clash: i64 = sqlx::query_scalar(&crate::sql!(
                "SELECT COUNT(*) FROM scrobbles
                  WHERE user_id = ? AND id != ?
                    AND LOWER(TRIM(title)) = LOWER(TRIM(?))
                    AND LOWER(TRIM(artist)) = LOWER(TRIM(?))
                    AND timestamp BETWEEN ? AND ?"
            ))
            .bind(user_id)
            .bind(id)
            .bind(title)
            .bind(artist)
            .bind(moved_to - DUPLICATE_WINDOW_SECS)
            .bind(moved_to + DUPLICATE_WINDOW_SECS)
            .fetch_one(&mut *tx)
            .await?;
            if clash > 0 {
                continue;
            }

            sqlx::query(&crate::sql!(
                "UPDATE scrobbles
                    SET timestamp = ?, timestamp_original = COALESCE(timestamp_original, ?)
                  WHERE id = ? AND user_id = ?"
            ))
            .bind(moved_to)
            .bind(timestamp)
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

            edit.moved += 1;
            if status == "synced" {
                edit.already_forwarded += 1;
            }
            if let Some(sid) = session_id {
                if !sessions.contains(sid) {
                    sessions.push(*sid);
                }
            }
        }
        tx.commit().await?;

        for sid in &sessions {
            self.recompute_session(*sid).await?;
            if self.session_has_a_gap(*sid).await? {
                edit.grouping_stale = true;
            }
        }
        edit.sessions_touched = sessions.len() as i64;
        Ok(edit)
    }
    /// Whether a session now holds a gap wider than the one it was grouped by.
    ///
    /// Cheap and exact: one ordered read of the session's own listens, which
    /// `idx_scrobbles_session` makes a seek. It answers only "does the stored
    /// grouping still describe this run" — never what the new grouping would
    /// be, which is `rebuild_sessions`' job and the user's decision.
    async fn session_has_a_gap(&self, session_id: i64) -> Result<bool, sqlx::Error> {
        let times: Vec<i64> = sqlx::query_scalar(&crate::sql!(
            "SELECT timestamp FROM scrobbles WHERE session_id = ? ORDER BY timestamp"
        ))
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(times.windows(2).any(|w| w[1] - w[0] > SESSION_GAP_SECS))
    }
}
