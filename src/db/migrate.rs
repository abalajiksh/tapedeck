//! Hand-rolled forward migrations for databases that predate the current
//! shape in `schema.rs`.
//!
//! Every block is guarded — `has_column`, or the constraint text in
//! `sqlite_master` where SQLite cannot alter a CHECK in place and the table has
//! to be rebuilt. **To add a column: add it to the `CREATE TABLE` in
//! `schema.rs` *and* append a guarded block here**, or existing databases
//! break.
//!
//! An index naming a migrated column belongs here rather than in `schema.rs`,
//! because `init()` runs first and would fail on a database that has not yet
//! gained the column (v0.61.1).

use super::*;

impl Database {
    /// Walk a database forward to the shape [`super::schema`]'s `init` writes.
    ///
    /// **This is SQLite-only work, and not by omission.** Every block below
    /// exists to carry a database forward from a shape that only ever existed
    /// in SQLite — the `postgres` feature has never shipped, so no Postgres
    /// database of this application predates the current schema. A fresh one
    /// gets the whole shape from `init()`, and one filled by a future
    /// `migrate-db` tool lands on that same shape.
    ///
    /// **That stops being true the moment Postgres ships and the schema next
    /// changes.** The first Postgres migration goes here, in an arm of its own
    /// — `has_column` below is SQLite's `pragma_table_info` and is compiled out
    /// with the rest, so a Postgres arm needs its own guard against
    /// `information_schema.columns`. Do not translate the blocks below to get
    /// one: they are answers to questions a Postgres database never asked.
    ///
    /// The indexes are the part both backends need, and they are in
    /// [`Database::indexes`] rather than here for exactly that reason.
    pub(super) async fn migrate(&self) -> Result<(), sqlx::Error> {
        #[cfg(feature = "postgres")]
        return Ok(());
        #[cfg(not(feature = "postgres"))]
        self.migrate_sqlite().await
    }

    #[cfg(not(feature = "postgres"))]
    async fn migrate_sqlite(&self) -> Result<(), sqlx::Error> {
        /// Does `table` already have `column`?
        ///
        /// The schema catalogue is the one thing no dialect layer can paper
        /// over — SQLite answers through `pragma_table_info` and Postgres
        /// through `information_schema`, and they are different queries rather
        /// than one query spelled twice.
        async fn has_column(pool: &Pool<Db>, table: &str, column: &str) -> bool {
            #[cfg(not(feature = "postgres"))]
            let sql = format!(
                "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='{}'",
                table, column
            );
            #[cfg(feature = "postgres")]
            let sql = format!(
                "SELECT COUNT(*) FROM information_schema.columns \
                 WHERE table_name = '{}' AND column_name = '{}'",
                table, column
            );
            let result: Result<(i64,), _> = sqlx::query_as(&sql).fetch_one(pool).await;
            matches!(result, Ok((count,)) if count > 0)
        }

        // Migration 1: MusicBrainz fields
        if !has_column(&self.pool, "scrobbles", "mbid_recording").await {
            info!("⚙️ Migrating: adding MusicBrainz fields...");
            for col in &[
                "mbid_recording TEXT",
                "mbid_release TEXT",
                "mbid_artist TEXT",
                "caa_id INTEGER",
                "caa_release_mbid TEXT",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {}", col))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration 2: user_id
        if !has_column(&self.pool, "scrobbles", "user_id").await {
            info!("⚙️ Migrating: adding user_id...");
            let _ =
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1")
                    .execute(&self.pool)
                    .await;
            let _ = sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_scrobbles_user_source ON scrobbles(user_id, source_id, source_name)")
                .execute(&self.pool).await;
        }

        // Migration 3: Phase 2 quality + context columns
        if !has_column(&self.pool, "scrobbles", "format_type").await {
            info!("⚙️ Migrating: adding Phase 2 quality and context fields...");
            for col in &[
                "format_type TEXT",
                "codec TEXT",
                "bitrate INTEGER",
                "sample_rate INTEGER",
                "bit_depth INTEGER",
                "channels INTEGER",
                "is_lossless BOOLEAN",
                "dsd_rate INTEGER",
                "dsd_multiplier INTEGER",
                "delivery_codec TEXT",
                "delivery_bitrate INTEGER",
                "is_transcoded BOOLEAN",
                "transcode_reason TEXT",
                "quality_score REAL",
                "device_id INTEGER",
                "chain_id INTEGER",
                "session_id INTEGER",
                "listening_context TEXT",
                "submission_client TEXT",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {}", col))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration 4: role column on users
        if !has_column(&self.pool, "users", "role").await {
            info!("⚙️ Migrating: adding role column to users...");
            let _ = sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user'")
                .execute(&self.pool)
                .await;
            // First user (id=1) is always admin
            let _ = sqlx::query("UPDATE users SET role = 'admin' WHERE id = 1")
                .execute(&self.pool)
                .await;
            info!("✅ Role migration complete");
        }

        // Migration 5: per-sink delivery tracking
        if !has_column(&self.pool, "scrobbles", "delivered_sinks").await {
            info!("⚙️ Migrating: adding delivered_sinks...");
            let _ = sqlx::query(
                "ALTER TABLE scrobbles ADD COLUMN delivered_sinks TEXT NOT NULL DEFAULT '[]'",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration 6: disabled flag on users
        if !has_column(&self.pool, "users", "disabled").await {
            info!("⚙️ Migrating: adding users.disabled...");
            let _ = sqlx::query("ALTER TABLE users ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0")
                .execute(&self.pool)
                .await;
        }

        // Migration 7: track_number on scrobbles (album-playthrough analytics)
        if !has_column(&self.pool, "scrobbles", "track_number").await {
            info!("⚙️ Migrating: adding scrobbles.track_number...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN track_number INTEGER")
                .execute(&self.pool)
                .await;
        }

        // Migration 8: language on scrobbles (language breakdown analytics)
        if !has_column(&self.pool, "scrobbles", "language").await {
            info!("⚙️ Migrating: adding scrobbles.language...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN language TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: which `user_sources` row a listen came from. Needed once a
        // user can have two Plex servers — `plex-55821-np` alone doesn't say
        // which one, so the artwork proxy would have to guess.
        if !has_column(&self.pool, "scrobbles", "source_ref").await {
            info!("⚙️ Migrating: adding scrobbles.source_ref...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN source_ref INTEGER")
                .execute(&self.pool)
                .await;
        }

        // Migration: several sources of the same kind per user. SQLite can't
        // drop a UNIQUE constraint in place, so the table is rebuilt. Guarded on
        // the `label` column, which only the new shape has.
        if !has_column(&self.pool, "user_sources", "label").await {
            info!("⚙️ Migrating: allowing multiple sources of the same kind...");
            let rebuild = [
                "CREATE TABLE IF NOT EXISTS user_sources_new (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL REFERENCES users(id),
                    kind TEXT NOT NULL,
                    label TEXT NOT NULL DEFAULT '',
                    enabled INTEGER NOT NULL DEFAULT 1,
                    url TEXT NOT NULL DEFAULT '',
                    credentials TEXT NOT NULL DEFAULT '',
                    remote_account TEXT,
                    poll_interval_secs INTEGER,
                    options TEXT NOT NULL DEFAULT '{}',
                    last_polled_at INTEGER,
                    last_error TEXT,
                    updated_at INTEGER NOT NULL DEFAULT 0
                )",
                "INSERT INTO user_sources_new
                    (id, user_id, kind, label, enabled, url, credentials, remote_account,
                     poll_interval_secs, options, last_polled_at, last_error, updated_at)
                 SELECT id, user_id, kind, '', enabled, url, credentials, remote_account,
                        poll_interval_secs, options, last_polled_at, last_error, updated_at
                 FROM user_sources",
                "DROP TABLE user_sources",
                "ALTER TABLE user_sources_new RENAME TO user_sources",
            ];
            for stmt in rebuild {
                if let Err(e) = sqlx::query(stmt).execute(&self.pool).await {
                    error!("user_sources rebuild failed: {} — leaving the old table in place", e);
                    break;
                }
            }
        }

        // Migration: resolved artwork URL. Supersedes deriving a Cover Art
        // Archive link from caa_id — artwork can now come from Deezer or the
        // user's own library, neither of which has a CAA id.
        if !has_column(&self.pool, "scrobbles", "artwork_url").await {
            info!("⚙️ Migrating: adding scrobbles.artwork_url...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN artwork_url TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: skipped listens (Rockbox logs mark rows L or S). Stored so
        // skip-rate analytics are possible, but excluded from every count.
        if !has_column(&self.pool, "scrobbles", "skipped").await {
            info!("⚙️ Migrating: adding scrobbles.skipped...");
            let _ = sqlx::query(
                "ALTER TABLE scrobbles ADD COLUMN skipped BOOLEAN NOT NULL DEFAULT FALSE",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: the Spotify track id a listen came in with. Written only by
        // the Spotify export importer, and read by nothing yet — it is kept
        // because it is the single external identifier that export carries, and
        // re-deriving it later would mean asking the user for the zip again.
        if !has_column(&self.pool, "scrobbles", "spotify_track_id").await {
            info!("⚙️ Migrating: adding scrobbles.spotify_track_id...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN spotify_track_id TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: per-source options (library/device filters). `user_sources`
        // is young enough that few databases will need this, but the table was
        // shipped without the column for one build.
        if !has_column(&self.pool, "user_sources", "options").await {
            info!("⚙️ Migrating: adding user_sources.options...");
            let _ = sqlx::query(
                "ALTER TABLE user_sources ADD COLUMN options TEXT NOT NULL DEFAULT '{}'",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: release year, for the by-decade breakdown.
        if !has_column(&self.pool, "scrobbles", "release_year").await {
            info!("⚙️ Migrating: adding scrobbles.release_year...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN release_year INTEGER")
                .execute(&self.pool)
                .await;
        }

        // Migration: a token's default signal chain. One token per app ("Pano
        // on Android", "fooyin") is how people actually organise these, so the
        // token is the natural place to say what the listens through it came
        // out of — no client-side config, nothing to type per listen.
        // Migration: sessions become a thing you can look at, name, merge and
        // split — so they need a stable identity that survives all of that.
        // `uuid` is added now rather than when Phase 6 shares one outward,
        // because retrofitting an id onto rows other instances already hold is
        // the migration that section warns about.
        if !has_column(&self.pool, "sessions", "uuid").await {
            info!("⚙️ Migrating: adding sessions.uuid/title/updated_at...");
            for col in ["uuid TEXT", "title TEXT", "updated_at INTEGER"] {
                let _ = sqlx::query(&format!("ALTER TABLE sessions ADD COLUMN {}", col))
                    .execute(&self.pool)
                    .await;
            }
            // Backfill: every existing session gets an id too, or the ones that
            // predate this are the only ones that can never be shared.
            let ids: Vec<(i64,)> = sqlx::query_as("SELECT id FROM sessions WHERE uuid IS NULL")
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default();
            for (id,) in ids {
                let _ = sqlx::query("UPDATE sessions SET uuid = ? WHERE id = ?")
                    .bind(new_uuid())
                    .bind(id)
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration: the full artist credit, kept when a `feat.` suffix is
        // folded out of `artist` so the featured performer is never lost —
        // it just stops fragmenting every grouping query.
        if !has_column(&self.pool, "scrobbles", "artist_credit").await {
            info!("⚙️ Migrating: adding scrobbles.artist_credit...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN artist_credit TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: which token a session's listens came in on. A change of
        // token means a change of app, which in practice means a change of
        // setup — so it ends the session even inside the 30-minute gap.
        if !has_column(&self.pool, "sessions", "token_id").await {
            info!("⚙️ Migrating: adding sessions.token_id...");
            let _ = sqlx::query(
                "ALTER TABLE sessions ADD COLUMN token_id INTEGER REFERENCES tokens(id)",
            )
            .execute(&self.pool)
            .await;
        }

        if !has_column(&self.pool, "tokens", "default_chain_id").await {
            info!("⚙️ Migrating: adding tokens.default_chain_id...");
            let _ = sqlx::query(
                "ALTER TABLE tokens ADD COLUMN default_chain_id INTEGER REFERENCES signal_chains(id)",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: whether a token's `import` submissions are live listens.
        // FALSE for every existing token, deliberately — an upgrade must not
        // start forwarding anything that wasn't being forwarded yesterday.
        if !has_column(&self.pool, "tokens", "imports_are_live").await {
            info!("⚙️ Migrating: adding tokens.imports_are_live...");
            let _ =
                sqlx::query("ALTER TABLE tokens ADD COLUMN imports_are_live BOOLEAN DEFAULT FALSE")
                    .execute(&self.pool)
                    .await;
        }

        // Migration: a chain's icon. Which setup a listen came out of is the
        // thing this app is about, and a name in a dropdown is a poor way to
        // recognise one at a glance.
        if !has_column(&self.pool, "signal_chains", "icon").await {
            info!("⚙️ Migrating: adding signal_chains.icon...");
            let _ = sqlx::query("ALTER TABLE signal_chains ADD COLUMN icon TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: Phase 6's sharing switch. FALSE for every chain that
        // existed before the column, which is the only safe direction — nobody
        // consents to publishing their listening by upgrading a binary. Same
        // rule `users.public_now_playing` follows.
        // Patch: a dub is a crate row that came from a *person*.
        //
        // `origin` already carries a name, but it is free text shared with
        // model connections — so the person is a real reference rather than a
        // string somebody could spell two ways. Deliberately not a second
        // table: a dub *is* a crate entry, and it lands on the page the user
        // already visits for records they are still flipping through.
        if !has_column(&self.pool, "recommendations", "from_user_id").await {
            info!("⚙️ Migrating: adding recommendations.from_user_id...");
            let _ = sqlx::query(
                "ALTER TABLE recommendations ADD COLUMN from_user_id INTEGER REFERENCES users(id)",
            )
            .execute(&self.pool)
            .await;
        }

        // Patch: which sitting a joint listen belonged to.
        //
        // The uuid of the spool, not its row id — it is the identity that
        // survives being handed to another instance, the same reason
        // `sessions.uuid` exists.
        if !has_column(&self.pool, "spools", "session_uuid").await {
            info!("⚙️ Migrating: adding spools.session_uuid...");
            let _ = sqlx::query("ALTER TABLE spools ADD COLUMN session_uuid TEXT")
                .execute(&self.pool)
                .await;
        }

        // **Here rather than in `init()`**, because it names a migrated column
        // and `init()` runs first — on a database predating that column the
        // CREATE fails and takes startup down with it. That is the v0.61.1
        // crash-loop exactly, and the rule it left behind: anything in `init()`
        // that *references* a migrated column has the same ordering problem as
        // the column itself. By this line a fresh database has it from `init()`
        // and a migrated one from the block above.
        //
        // One reconstructed spool per (host, sitting). Handing the same evening
        // to a *second* person adds them to the sitting that already exists
        // rather than minting a parallel one — a sitting is one event however
        // many people were in it.
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_spools_one_per_sitting
             ON spools(host_user_id, session_uuid) WHERE session_uuid IS NOT NULL",
        )
        .execute(&self.pool)
        .await?;

        if !has_column(&self.pool, "spool_captures", "company").await {
            info!("⚙️ Migrating: adding spool_captures.company...");
            let _ = sqlx::query("ALTER TABLE spool_captures ADD COLUMN company TEXT")
                .execute(&self.pool)
                .await;
        }

        if !has_column(&self.pool, "scrobbles", "company").await {
            info!("⚙️ Migrating: adding scrobbles.company...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN company TEXT")
                .execute(&self.pool)
                .await;
        }

        if !has_column(&self.pool, "scrobbles", "joint_session_id").await {
            info!("⚙️ Migrating: adding scrobbles.joint_session_id...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN joint_session_id TEXT")
                .execute(&self.pool)
                .await;
        }

        // Patch: an existing account is not published by an upgrade.
        if !has_column(&self.pool, "users", "federation_enabled").await {
            info!("⚙️ Migrating: adding users.federation_enabled...");
            let _ = sqlx::query(
                "ALTER TABLE users ADD COLUMN federation_enabled INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await;
        }

        // Nobody is published by an upgrade — the rule `public_now_playing`
        // and `federation_enabled` both follow, and the one that matters most
        // here: a patch accepted under the old rule was accepted on the
        // understanding that an unshared chain published nothing.
        if !has_column(&self.pool, "users", "share_all_with_patched").await {
            info!("⚙️ Migrating: adding users.share_all_with_patched...");
            let _ = sqlx::query(
                "ALTER TABLE users ADD COLUMN share_all_with_patched INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await;
        }

        // A side played before the shelf stamped its own sleeve got a cover per
        // *track* from a provider — eleven tracks off one record, eleven
        // different covers. Nothing was lost, so this simply rewrites them to
        // the sleeve the shelf is holding.
        //
        // Marker-guarded rather than run every start: it is a write per
        // release, and after the first pass there is nothing to do.
        if self.get_app_setting("physical_artwork_repaired").await.ok().flatten().is_none() {
            let releases: Vec<(i64, i64)> =
                sqlx::query_as("SELECT id, user_id FROM physical_releases")
                    .fetch_all(&self.pool)
                    .await
                    .unwrap_or_default();
            let mut fixed = 0u64;
            for (id, user_id) in releases {
                if let Ok(Some(cover)) = self.physical_cover(user_id, id).await {
                    fixed += self.set_physical_artwork(user_id, id, &cover).await.unwrap_or(0);
                }
            }
            if fixed > 0 {
                info!("⚙️ Migrating: put {fixed} shelf listen(s) back on their own sleeve...");
            }
            let _ = self.set_app_setting("physical_artwork_repaired", "1").await;
        }

        for (col, ty) in [("acquired_as", "TEXT"), ("acquired_from", "TEXT")] {
            if !has_column(&self.pool, "physical_releases", col).await {
                info!("⚙️ Migrating: adding physical_releases.{col}...");
                let _ =
                    sqlx::query(&format!("ALTER TABLE physical_releases ADD COLUMN {col} {ty}"))
                        .execute(&self.pool)
                        .await;
            }
        }

        if !has_column(&self.pool, "scrobbles", "language_source").await {
            info!("⚙️ Migrating: adding scrobbles.language_source...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN language_source TEXT")
                .execute(&self.pool)
                .await;
        }

        // The placeholder used to be credited like a performer, so an existing
        // history holds `scrobble_artists` rows for it — and those are what put
        // "Various Artists" third on Top Charts. `credited_artists` no longer
        // writes them; this clears the ones already there.
        //
        // Read first, delete only if there is something to delete: this runs on
        // every start, and an unconditional DELETE would take the write lock
        // each time to do nothing. The read is a seek on
        // `idx_scrobble_artists_key`.
        let stale: Option<(i64,)> = sqlx::query_as(&format!(
            "SELECT 1 FROM scrobble_artists WHERE artist_key {IS_VARIOUS} LIMIT 1"
        ))
        .fetch_optional(&self.pool)
        .await
        .unwrap_or(None);
        if stale.is_some() {
            info!("⚙️ Migrating: dropping 'Various Artists' credits — it is a filing convention, not a performer...");
            let _ =
                sqlx::query(&format!("DELETE FROM scrobble_artists WHERE artist_key {IS_VARIOUS}"))
                    .execute(&self.pool)
                    .await;
        }

        if !has_column(&self.pool, "signal_chains", "shared").await {
            info!("⚙️ Migrating: adding signal_chains.shared...");
            let _ = sqlx::query(
                "ALTER TABLE signal_chains ADD COLUMN shared BOOLEAN NOT NULL DEFAULT FALSE",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: artist-level genres. A genre is a property of an artist far
        // more than of one recording, and MusicBrainz tags it that way — so this
        // is where the useful answers actually are.
        if !has_column(&self.pool, "artists", "genres").await {
            info!("⚙️ Migrating: adding artists.genres...");
            let _ =
                sqlx::query("ALTER TABLE artists ADD COLUMN genres TEXT").execute(&self.pool).await;
        }

        // Migration: genres, which MusicBrainz has been handing over since the
        // cache existed. `mb_cache.genres` was written by every lookup and read
        // by nothing, while the ranged-stats payload shipped a hardcoded empty
        // list — the UI had a slot for this all along.
        if !has_column(&self.pool, "scrobbles", "genres").await {
            info!("⚙️ Migrating: adding scrobbles.genres...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN genres TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: the five quality fields that were parsed and thrown away.
        // `AudioQuality` has carried `container`, `delivery_sample_rate` and
        // `delivery_bit_depth` since Phase 2 and the ingest path filled them in,
        // but `save_scrobble` never had columns to bind them to — so the values
        // were built on every listen and dropped on the floor. `source_quality`
        // and `dsd_to_pcm_converted` arrived in the payload and weren't even
        // mapped. "Was this DSD converted to PCM before it reached me?" is a
        // question this app exists to answer and could not.
        if !has_column(&self.pool, "scrobbles", "container").await {
            info!("⚙️ Migrating: adding the remaining scrobble quality fields...");
            for col in &[
                "container TEXT",
                "source_quality TEXT",
                "dsd_to_pcm_converted BOOLEAN",
                "delivery_sample_rate INTEGER",
                "delivery_bit_depth INTEGER",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {}", col))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration: a hand-set cover for a release, the counterpart of the
        // artist portrait override that has existed since v0.21.0. An album's
        // cover is otherwise whichever artwork the newest listen of it happens
        // to carry, and there was no way to say "not that one" — which for a
        // journal is the wrong answer to have no answer to.
        if !has_column(&self.pool, "entities", "image_url").await {
            info!("⚙️ Migrating: adding entities.image_url...");
            let _ = sqlx::query("ALTER TABLE entities ADD COLUMN image_url TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: CD and SACD on the shelf.
        //
        // `physical_releases.kind` carries a CHECK, and SQLite cannot alter a
        // constraint in place — the only way is to build the table again and
        // copy. That is why this one is a whole block rather than an
        // `ADD COLUMN`, and why it is guarded on the *constraint text* in
        // `sqlite_master` rather than on a column existing: the column has
        // always been there, it is what it will accept that changed.
        //
        // `physical_plays.release_id` points at this table, so foreign keys are
        // suspended for the swap — the pool opens with them on. They go back on
        // immediately after, and the whole thing is one transaction, so a crash
        // mid-rebuild leaves the old table rather than a half-copied one.
        let needs_disc_kinds: Option<(String,)> = sqlx::query_as(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'physical_releases'",
        )
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        if needs_disc_kinds.is_some_and(|(sql,)| !sql.contains("'sacd'")) {
            info!("⚙️ Migrating: letting the shelf hold CDs and SACDs...");
            let _ = sqlx::query("PRAGMA foreign_keys = OFF").execute(&self.pool).await;
            let rebuild = async {
                let mut tx = self.pool.begin().await?;
                sqlx::query(
                    "CREATE TABLE physical_releases_new (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        user_id INTEGER NOT NULL REFERENCES users(id),
                        kind TEXT NOT NULL CHECK (kind IN ('vinyl', 'cassette', 'cd', 'sacd')),
                        artist TEXT NOT NULL, title TEXT NOT NULL,
                        year INTEGER,
                        mbid_release TEXT, discogs_id TEXT, barcode TEXT, catalog_number TEXT,
                        artwork_url TEXT,
                        tracks TEXT NOT NULL DEFAULT '[]',
                        notes TEXT,
                        added_at INTEGER NOT NULL
                    )",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO physical_releases_new
                     SELECT id, user_id, kind, artist, title, year, mbid_release, discogs_id,
                            barcode, catalog_number, artwork_url, tracks, notes, added_at
                       FROM physical_releases",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query("DROP TABLE physical_releases").execute(&mut *tx).await?;
                sqlx::query("ALTER TABLE physical_releases_new RENAME TO physical_releases")
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await
            }
            .await;
            if let Err(e) = rebuild {
                warn!("Could not widen the shelf's formats: {}", e);
            }
            let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(&self.pool).await;
        }

        // Migration: when you bought it, and what it gets played on.
        //
        // This has to come **after** the rebuild above, not before: that block
        // copies an explicit column list into a freshly created table, so any
        // column added ahead of it would be silently dropped on the one kind of
        // database that still needs it.
        if !has_column(&self.pool, "physical_releases", "purchased_at").await {
            info!("⚙️ Migrating: purchase dates and default gear on the shelf...");
            for col in &[
                "purchased_at INTEGER",
                "chain_id INTEGER REFERENCES signal_chains(id)",
                "equipment_id INTEGER REFERENCES equipment(id)",
            ] {
                let _ = sqlx::query(&format!("ALTER TABLE physical_releases ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }
        if !has_column(&self.pool, "equipment", "purchased_at").await {
            info!("⚙️ Migrating: adding equipment.purchased_at...");
            let _ = sqlx::query("ALTER TABLE equipment ADD COLUMN purchased_at INTEGER")
                .execute(&self.pool)
                .await;
        }
        if !has_column(&self.pool, "equipment", "status").await {
            info!("⚙️ Migrating: adding equipment.status...");
            let _ = sqlx::query("ALTER TABLE equipment ADD COLUMN status TEXT")
                .execute(&self.pool)
                .await;
        }

        // Migration: give every listen already stored its artist credits.
        //
        // Backfilled rather than left to new listens only, because the whole
        // point is a history that already exists — "does Hilary Hahn get credit
        // for the Bach I played last year" has to be yes, or the feature reads
        // as broken for everybody except new users.
        //
        // Two sources, in this order: the primary from `scrobbles.artist`, and
        // anyone else out of `artist_credit`, which has been collecting the
        // performers since v0.17.0 and being read by almost nothing. So an
        // existing database has the data already — it was only ever the join
        // that was missing.
        //
        // Guarded on the table being *empty* rather than on a column existing:
        // the table is created by `init()` on every start, so `has_column`
        // would say yes immediately and skip the backfill forever.
        let credits_done: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(*) FROM (SELECT 1 FROM scrobble_artists LIMIT 1)")
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        let any_listens: Option<(i64,)> =
            sqlx::query_as("SELECT COUNT(*) FROM (SELECT 1 FROM scrobbles LIMIT 1)")
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        if credits_done.map(|(n,)| n) == Some(0) && any_listens.map(|(n,)| n) == Some(1) {
            info!("⚙️ Migrating: crediting every performer on the listens already stored...");
            if let Err(e) = self.backfill_scrobble_artists().await {
                // Not fatal. Charts fall back to the primary artist, which is
                // exactly what they did before this table existed, so a failed
                // backfill loses a feature rather than breaking the app.
                warn!("Could not backfill artist credits: {}", e);
            }
        }

        // Migration: re-file the DSD listens that were stored as lossy PCM.
        //
        // Both session sources tested `codec == "dsd"` where ffmpeg says
        // `dsd_lsbf_planar`, so every DSD listen ever polled was written with
        // `format_type = 'pcm'`, `is_lossless = 0` and a score of 60 — see
        // `models::codec_is_dsd`. Fixing the classifier only helps listens
        // recorded *after* it, because `quality_score` is written once at ingest
        // and never recomputed, so without this pass the reported symptom (a
        // "Lossy" bucket made entirely of SACD rips) never goes away.
        //
        // This is deliberately **not** the general rescore that
        // `AudioQuality::compute_score`'s doc comment warns against. That
        // warning is about changing the scoring *formula*, which would make
        // listens either side of the change incomparable. Here the formula is
        // untouched and the input was misparsed: these rows are being given the
        // score they should always have had.
        //
        // No marker is needed to make it run once. The `WHERE` matches only
        // rows that are still misclassified, so a second run selects nothing.
        if let Err(e) = self.reclassify_dsd_listens().await {
            // Not fatal: the rows stay as they were, which is exactly the state
            // every release before this one shipped.
            warn!("Could not re-file misclassified DSD listens: {}", e);
        }

        // Migration: a note can hang off a report period.
        //
        // The second CHECK rebuild in this file, and for the same reason as the
        // shelf's: `notes` enforces "exactly one target" with a constraint, and
        // SQLite cannot alter one in place. Guarded on the constraint text, not
        // on the column — a `period_key` added by `ADD COLUMN` would sit under
        // the *old* CHECK, which reads `(entity_id IS NULL) <> (scrobble_id IS
        // NULL)` and would reject every period note as having no target at all.
        //
        // `note_revisions.note_id` references this table, so foreign keys come
        // off for the swap and the whole thing is one transaction. Note the
        // insert names its columns on both sides: the new table has one more,
        // and a bare `INSERT INTO … SELECT *` would shift `body_md` into it.
        let notes_sql: Option<(String,)> =
            sqlx::query_as("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'notes'")
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        if notes_sql.is_some_and(|(sql,)| !sql.contains("period_key")) {
            info!("⚙️ Migrating: letting a liner note hang off a report period...");
            let _ = sqlx::query("PRAGMA foreign_keys = OFF").execute(&self.pool).await;
            let rebuild = async {
                let mut tx = self.pool.begin().await?;
                sqlx::query(
                    "CREATE TABLE notes_new (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        user_id INTEGER NOT NULL REFERENCES users(id),
                        entity_id INTEGER REFERENCES entities(id),
                        scrobble_id INTEGER REFERENCES scrobbles(id),
                        period_key TEXT,
                        body_md TEXT NOT NULL,
                        visibility TEXT NOT NULL DEFAULT 'private',
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL,
                        CHECK ((entity_id IS NOT NULL) + (scrobble_id IS NOT NULL)
                               + (period_key IS NOT NULL) = 1)
                    )",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO notes_new
                            (id, user_id, entity_id, scrobble_id, body_md,
                             visibility, created_at, updated_at)
                     SELECT  id, user_id, entity_id, scrobble_id, body_md,
                             visibility, created_at, updated_at
                       FROM notes",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query("DROP TABLE notes").execute(&mut *tx).await?;
                sqlx::query("ALTER TABLE notes_new RENAME TO notes").execute(&mut *tx).await?;
                // Dropped with the old table, so they are rebuilt here rather
                // than left to `init()` — which has already run by now.
                for idx in &[
                    "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_entity ON notes(user_id, entity_id) WHERE entity_id IS NOT NULL",
                    "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_scrobble ON notes(user_id, scrobble_id) WHERE scrobble_id IS NOT NULL",
                    "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_period ON notes(user_id, period_key) WHERE period_key IS NOT NULL",
                ] {
                    sqlx::query(idx).execute(&mut *tx).await?;
                }
                tx.commit().await
            }
            .await;
            if let Err(e) = rebuild {
                warn!("Could not widen what a liner note can be about: {}", e);
            }
            let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(&self.pool).await;
        }

        // Migration: a note can hang off one line of a lyric — a **pin**.
        //
        // The third CHECK rebuild in this file, and the same shape as the two
        // above it: SQLite cannot alter a constraint in place, and this one has
        // to gain a fourth term. Guarded on the **constraint text** rather than
        // on a column, because a `line_key` added by `ADD COLUMN` would sit
        // under the old three-term CHECK, which reads `… = 1` over three
        // targets and would reject every pin as having no target at all.
        //
        // `note_revisions.note_id` references this table, so foreign keys come
        // off for the swap and the whole thing is one transaction. The insert
        // names its columns on both sides: the new table has two more, and a
        // bare `INSERT INTO … SELECT *` would shift `body_md` into one of them.
        let notes_sql: Option<(String,)> =
            sqlx::query_as("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'notes'")
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        if notes_sql.is_some_and(|(sql,)| !sql.contains("line_key")) {
            info!("⚙️ Migrating: letting a liner note be pinned to a lyric line...");
            let _ = sqlx::query("PRAGMA foreign_keys = OFF").execute(&self.pool).await;
            let rebuild = async {
                let mut tx = self.pool.begin().await?;
                sqlx::query(
                    "CREATE TABLE notes_new (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        user_id INTEGER NOT NULL REFERENCES users(id),
                        entity_id INTEGER REFERENCES entities(id),
                        scrobble_id INTEGER REFERENCES scrobbles(id),
                        period_key TEXT,
                        line_key TEXT,
                        line_label TEXT,
                        body_md TEXT NOT NULL,
                        visibility TEXT NOT NULL DEFAULT 'private',
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL,
                        CHECK ((entity_id IS NOT NULL) + (scrobble_id IS NOT NULL)
                               + (period_key IS NOT NULL) + (line_key IS NOT NULL) = 1),
                        CHECK ((line_key IS NULL) = (line_label IS NULL))
                    )",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO notes_new
                            (id, user_id, entity_id, scrobble_id, period_key, body_md,
                             visibility, created_at, updated_at)
                     SELECT  id, user_id, entity_id, scrobble_id, period_key, body_md,
                             visibility, created_at, updated_at
                       FROM notes",
                )
                .execute(&mut *tx)
                .await?;
                sqlx::query("DROP TABLE notes").execute(&mut *tx).await?;
                sqlx::query("ALTER TABLE notes_new RENAME TO notes").execute(&mut *tx).await?;
                // Dropped with the old table, so they are rebuilt here rather
                // than left to `init()`, which has already run by now.
                for idx in &[
                    "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_entity ON notes(user_id, entity_id) WHERE entity_id IS NOT NULL",
                    "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_scrobble ON notes(user_id, scrobble_id) WHERE scrobble_id IS NOT NULL",
                    "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_period ON notes(user_id, period_key) WHERE period_key IS NOT NULL",
                ] {
                    sqlx::query(idx).execute(&mut *tx).await?;
                }
                tx.commit().await
            }
            .await;
            if let Err(e) = rebuild {
                warn!("Could not let a liner note be pinned to a lyric line: {}", e);
            }
            let _ = sqlx::query("PRAGMA foreign_keys = ON").execute(&self.pool).await;
        }

        // Deliberately here rather than in `init()` alongside the other two note
        // indexes, and this is the trap: `init()` runs *before* `migrate()`, so
        // on an existing database `period_key` does not exist yet and creating
        // an index on it fails — and `init()` propagates with `?`, so the whole
        // process exits and systemd restarts it forever. Shipped exactly that
        // way in v0.61.0; `an_existing_database_opens_and_gains_period_notes`
        // is the check that was missing.
        //
        // A fresh database gets the column from `init()`, a migrated one from
        // the block above, so by this line it always exists.
        sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_period ON notes(user_id, period_key) WHERE period_key IS NOT NULL")
            .execute(&self.pool)
            .await?;

        // One pin per line per user, the rule every other note target follows —
        // a note is a living document, edited in place. In `migrate()` and not
        // `init()` for exactly the reason above: on a database that predates
        // the rebuild `line_key` does not exist when `init()` runs, and `init()`
        // propagates with `?`.
        sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_line ON notes(user_id, line_key) WHERE line_key IS NOT NULL")
            .execute(&self.pool)
            .await?;

        // Migration: a user's own profile.
        //
        // `avatar_url` is a URL rather than a blob for the same reason the
        // shelf's scans are files on disk — but unlike a booklet an avatar is
        // one small image, so an uploaded one is stored under the uploads dir
        // and this column holds the path that serves it.
        if !has_column(&self.pool, "users", "avatar_url").await {
            info!("⚙️ Migrating: adding user profiles...");
            for col in &["avatar_url TEXT", "bio TEXT"] {
                let _ = sqlx::query(&format!("ALTER TABLE users ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration: the publish switch for now-playing.
        //
        // Both columns default to "not published" for every account that
        // already exists, which is the only safe reading of an upgrade: nobody
        // consented to a public page by installing a new binary. `NOT NULL
        // DEFAULT 0` gives existing rows the off state without a backfill.
        // The one identifier that crosses between a library, a streaming
        // catalogue and MusicBrainz. Filled by ingest when a client sends one
        // and by `backfill_isrc` otherwise; read by the playlist push, which is
        // the only thing that can use an exact key instead of a name.
        if !has_column(&self.pool, "scrobbles", "isrc").await {
            info!("⚙️ Migrating: adding scrobbles.isrc...");
            let _ =
                sqlx::query("ALTER TABLE scrobbles ADD COLUMN isrc TEXT").execute(&self.pool).await;
        }

        if !has_column(&self.pool, "physical_plays", "recorded_at").await {
            info!("⚙️ Migrating: remembering when a side play was written...");
            let _ = sqlx::query("ALTER TABLE physical_plays ADD COLUMN recorded_at INTEGER")
                .execute(&self.pool)
                .await;
        }

        if !has_column(&self.pool, "users", "public_now_playing").await {
            info!("⚙️ Migrating: adding the public now-playing switch...");
            let _ = sqlx::query(
                "ALTER TABLE users ADD COLUMN public_now_playing INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await;
            let _ = sqlx::query("ALTER TABLE users ADD COLUMN public_np_fields TEXT")
                .execute(&self.pool)
                .await;
        }

        if !has_column(&self.pool, "users", "forward_spool_listens").await {
            info!("⚙️ Migrating: adding the Shared Spool forwarding switch...");
            let _ = sqlx::query(
                "ALTER TABLE users ADD COLUMN forward_spool_listens INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: where the tape is parked.
        //
        // Like every other `physical_releases` column this comes **after** the
        // disc-kinds rebuild, which copies an explicit column list into a fresh
        // table — anything added ahead of it is silently dropped on the one kind
        // of database that still needs the rebuild.
        //
        // Cassettes only, on purpose. See `park_tape`.
        if !has_column(&self.pool, "physical_releases", "parked_side").await {
            info!("⚙️ Migrating: remembering where a tape was left...");
            for col in &["parked_side TEXT", "parked_secs INTEGER"] {
                let _ = sqlx::query(&format!("ALTER TABLE physical_releases ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }
        // Migration: which variant of a chain a listen went through.
        //
        // Nullable and never backfilled: a listen recorded before variants
        // existed went through the chain as it was, which is exactly what a
        // null means here. Guessing a variant for it would invent wear on a
        // cartridge that may not have been fitted yet.
        // Migration: the forgiving entity key.
        //
        // Backfilled rather than left null, because it is what makes a love
        // recognised across two spellings and an existing database is precisely
        // the one holding the split entities. The index is **not** unique — see
        // the column comment.
        if !has_column(&self.pool, "entities", "match_key").await {
            info!("⚙️ Migrating: forgiving entity keys...");
            let _ = sqlx::query("ALTER TABLE entities ADD COLUMN match_key TEXT")
                .execute(&self.pool)
                .await;
            let rows: Vec<(i64, String, String, String)> =
                sqlx::query_as("SELECT id, kind, artist_name, name FROM entities")
                    .fetch_all(&self.pool)
                    .await
                    .unwrap_or_default();
            let mut tx = self.pool.begin().await?;
            for (id, kind, artist, name) in &rows {
                let _ = sqlx::query("UPDATE entities SET match_key = ? WHERE id = ?")
                    .bind(Self::match_key(kind, artist, name))
                    .bind(id)
                    .execute(&mut *tx)
                    .await;
            }
            tx.commit().await?;
            info!("⚙️ Migrated {} entities.", rows.len());
        }
        // Outside the guard and `IF NOT EXISTS`, for the reason v0.61.1 records:
        // anything in `init()` that references a migrated column fails on every
        // existing database, so it belongs here — after the column is certain to
        // be there on both paths.
        // **`scrobbles.session_id` had no index, and four queries look listens up
        // by it.** The worst is `list_sessions`, which runs a correlated
        // subquery per session row — so opening the History page's session view
        // was one full scan of the whole history *per session shown*. Measured
        // on the real host: a single `list_sessions` call took **630 seconds**,
        // holding one of the pool's five connections for ten and a half minutes
        // while every other request queued behind it. That is the quadratic
        // shape this codebase has now hit three times.
        //
        // A point lookup — "the listens in this session" — is exactly the
        // workload an index is for, which is the distinction v0.59.0 drew when
        // it dropped the *expression* indexes: those replaced sequential scans
        // for whole-history aggregations and lost. This one replaces a full
        // scan with a seek.
        //
        // Partial, because most rows have no session and there is no query that
        // wants the NULLs.
        let _ = sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_session
                 ON scrobbles(session_id) WHERE session_id IS NOT NULL",
        )
        .execute(&self.pool)
        .await;
        // ── The album page's "whole record" button, and the one expression
        //    index in this file ──────────────────────────────────────────────
        //
        // `album_release_mbid` asks which release a plurality of the listens
        // claim, folding **both** names in its `WHERE`. That is the
        // `LOWER(TRIM(` shape recorded three times above, and with nothing able
        // to serve it the plan was `SEARCH … USING INDEX idx_scrobbles_user_time
        // (user_id=?)` plus two temp B-trees — a walk of every row the user
        // has, with a rowid lookup each. On a single-user instance that is the
        // entire history. Measured on a seeded 50,000-listen database:
        // **400,247 VDBE steps** per call.
        //
        // Reported 2026-08-20: the first click returned Cloudflare's own 502
        // and the second worked. Nothing was cached at the app level — the
        // handler stores nothing on purpose — so what was warm the second time
        // was the OS page cache. A cold pass over the whole history off an SD
        // card ran past the tunnel's patience; a warm one did not.
        //
        // This is the exception to the v0.59.0 note rather than a repeat of it,
        // for the reason `idx_scrobbles_session` above is: it is a **point
        // lookup** — one artist, one album, a handful of rows — which that note
        // names as the workload an index is actually for. Those three lost
        // because they meant a rowid lookup *per row across the whole history*;
        // this one seeks straight to one record. 400,247 steps → **37**.
        //
        // `mbid_release` is the fourth column so the index *can* cover the
        // query, but do not lean on that: SQLite 3.51 reports `USING COVERING
        // INDEX` and the **3.44.0 bundled by libsqlite3-sys 0.27 does not**,
        // taking the rowid lookups instead. 41 steps against 37 — with the seek
        // there is almost nothing left to look up. The seek is the fix.
        //
        // **`WHERE mbid_release IS NOT NULL` is load-bearing, not a size
        // trim.** Without it this becomes a net loss exactly as v0.59.0
        // describes: the planner finds it selective and `top artists` and `top
        // albums` flip from a sequential `SCAN` to `SEARCH … USING INDEX
        // (user_id=?)`, a non-covering walk of everything with the random I/O
        // that costs on this hardware. Verified by building it both ways and
        // diffing `EXPLAIN QUERY PLAN` over five representative queries — the
        // partial form leaves every other plan byte-identical, the plain form
        // changes two. A partial index is only usable when the query's `WHERE`
        // implies the index's, and no aggregation here filters on
        // `mbid_release`, so the predicate is what confines this to its one
        // caller.
        //
        // In `migrate()` and not `init()` because it names `mbid_release`, and
        // `init()` runs first: on a database predating that column the whole
        // `CREATE INDEX` fails and takes startup with it. That is the v0.61.1
        // trap — *anything in `init()` referencing a migrated column has the
        // same ordering problem as the column* — and it was caught here by
        // `an_existing_database_gains_the_playback_columns`, which is what that
        // test exists for.
        //
        // ~3.2 MB on 50,000 listens. **Still to be confirmed on the Pi**
        // (`temp/measure-tapedeck.sh`): the step count is hardware-independent
        // evidence and a laptop's clock is not.
        let _ = sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_album_release
                 ON scrobbles(user_id, LOWER(TRIM(artist)), LOWER(TRIM(album)), mbid_release)
                WHERE mbid_release IS NOT NULL AND TRIM(mbid_release) != ''",
        )
        .execute(&self.pool)
        .await;
        let _ = sqlx::query("CREATE INDEX IF NOT EXISTS idx_entities_match ON entities(match_key)")
            .execute(&self.pool)
            .await;
        let _ = sqlx::query("CREATE INDEX IF NOT EXISTS idx_entities_mbid ON entities(kind, mbid)")
            .execute(&self.pool)
            .await;

        if !has_column(&self.pool, "scrobbles", "chain_variant_id").await {
            info!("⚙️ Migrating: chain variants...");
            let _ = sqlx::query(
                "ALTER TABLE scrobbles ADD COLUMN chain_variant_id INTEGER
                   REFERENCES chain_variants(id)",
            )
            .execute(&self.pool)
            .await;
        }
        if !has_column(&self.pool, "physical_plays", "chain_variant_id").await {
            let _ = sqlx::query(
                "ALTER TABLE physical_plays ADD COLUMN chain_variant_id INTEGER
                   REFERENCES chain_variants(id)",
            )
            .execute(&self.pool)
            .await;
        }
        if !has_column(&self.pool, "physical_releases", "chain_variant_id").await {
            let _ = sqlx::query(
                "ALTER TABLE physical_releases ADD COLUMN chain_variant_id INTEGER
                   REFERENCES chain_variants(id)",
            )
            .execute(&self.pool)
            .await;
        }
        if !has_column(&self.pool, "sessions", "notes").await {
            let _ =
                sqlx::query("ALTER TABLE sessions ADD COLUMN notes TEXT").execute(&self.pool).await;
        }

        // Migration: remove the expression indexes v0.56.0 added.
        //
        // Measured a net loss on the target hardware — see the note in `init`.
        // Dropped rather than left in place, because the planner will keep
        // choosing them: they exist, they look selective, and the cost model
        // has no idea the storage is an SD card.
        for ix in ["idx_scrobbles_artist_key", "idx_scrobbles_track_key", "idx_scrobbles_album_key"]
        {
            let present: Option<(i64,)> = sqlx::query_as(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?",
            )
            .bind(ix)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten();
            if present.map(|(n,)| n) == Some(1) {
                info!("⚙️ Migrating: dropping {} — a net loss on slow storage", ix);
                let _ =
                    sqlx::query(&format!("DROP INDEX IF EXISTS {ix}")).execute(&self.pool).await;
                // The statistics describe indexes that no longer exist.
                let _ = sqlx::query("ANALYZE").execute(&self.pool).await;
            }
        }

        if !has_column(&self.pool, "artists", "landscape_url").await {
            info!("⚙️ Migrating: a wide image for an artist...");
            let _ = sqlx::query("ALTER TABLE artists ADD COLUMN landscape_url TEXT")
                .execute(&self.pool)
                .await;
        }

        if !has_column(&self.pool, "physical_spins", "start_secs").await {
            let _ = sqlx::query(
                "ALTER TABLE physical_spins ADD COLUMN start_secs INTEGER NOT NULL DEFAULT 0",
            )
            .execute(&self.pool)
            .await;
        }

        // Migration: what a pushing client can say about *how* a track played.
        //
        // `listened_ms` is what makes `skipped` mean anything. The flag has
        // existed since Phase 2 and could only ever say *that* a track was cut
        // short, never how far in — so "skipped three seconds in" and "skipped
        // with thirty seconds left" were stored identically, and those are
        // opposite behaviours.
        //
        // `is_shuffle` and `queue_source` are the surviving half of
        // `TapedeckSession`, which was deserialised on every submission and read
        // by nothing. They answer what did I put on deliberately versus what did
        // shuffle hand me, which no other column in this schema can.
        //
        // All three are nullable with no default, and that is the honest shape:
        // nothing before this release recorded them, and no source but a pushing
        // client can. A zero would read as "heard none of it".
        //
        // Deliberately **not indexed**. Nothing queries by them yet, and this
        // app's reads are whole-history aggregations on slow sequential storage
        // where an index that only supplies a grouping key measured as a net
        // loss (v0.59.0). Also note there is no index on them in `init()`, which
        // is the ordering trap that crash-looped v0.61.0.
        if !has_column(&self.pool, "scrobbles", "listened_ms").await {
            info!("⚙️ Migrating: recording how much of a track was actually heard...");
            for col in &["listened_ms INTEGER", "is_shuffle BOOLEAN", "queue_source TEXT"] {
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }

        // Migration: a sitting is a thing you can come back to.
        //
        // Everything a Shared Spool needed for a page of its own was already
        // stored — a host, a span, a roster with join times, captures in three
        // states, and the listens tied by `joint_session_id`. What was missing
        // was any query that returned an *ended* one: `active_spool` was the
        // only reader of the table, so closing a sitting made the evening
        // disappear from every screen in the application. These columns are
        // what turn it from a transient into a record.
        if !has_column(&self.pool, "spools", "notes").await {
            info!("⚙️ Migrating: a Shared Spool can be written about...");
            for col in &["notes TEXT", "guests TEXT"] {
                let _ = sqlx::query(&format!("ALTER TABLE spools ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }

        // **No backfill, and that is the correct reading rather than a saving.**
        // NULL means "from the start of the sitting", which is exactly what
        // every member of every existing spool got: `offer_spool_backlog` has
        // always replayed the whole evening to whoever joined. Writing
        // `joined_at` into `present_from` here would retroactively cut those
        // people out of plays they were offered and in many cases accepted.
        if !has_column(&self.pool, "spool_members", "present_from").await {
            info!("⚙️ Migrating: when somebody was actually in the room...");
            for col in &["present_from INTEGER", "present_until INTEGER"] {
                let _ = sqlx::query(&format!("ALTER TABLE spool_members ADD COLUMN {col}"))
                    .execute(&self.pool)
                    .await;
            }
        }

        // What the clock said before a hand edit. Written once, so a row that
        // already carries one is not overwritten by a second correction — the
        // original observation is the thing worth keeping, not the last guess.
        if !has_column(&self.pool, "scrobbles", "timestamp_original").await {
            info!("⚙️ Migrating: keeping what the clock said before it was corrected...");
            let _ = sqlx::query("ALTER TABLE scrobbles ADD COLUMN timestamp_original INTEGER")
                .execute(&self.pool)
                .await;
        }

        // The chain an accepted Shared Spool play is filed under. Unset for
        // every account that predates it, which reproduces the old behaviour
        // exactly — those listens carried no chain at all.
        if !has_column(&self.pool, "users", "spool_chain_id").await {
            info!("⚙️ Migrating: a chain for the room you were in...");
            let _ = sqlx::query("ALTER TABLE users ADD COLUMN spool_chain_id INTEGER")
                .execute(&self.pool)
                .await;
        }

        // Migration: a stable id on every table a replica has to agree about.
        //
        // Nothing reads these yet, and that is the point. The argument is the
        // one Phase 6 already made and `sessions.uuid` already acted on:
        // deferring a *feature* is cheap, and retrofitting an id onto rows a
        // second instance already holds is not. A staging instance restored
        // from a snapshot needs none of this; a desktop replica that writes
        // offline and reconciles later needs all of it, and by then these rows
        // exist in two places.
        //
        // Deliberately absent: `scrobbles`, `loves` and `entities`. Each
        // already has a natural key that is *already* used across instances --
        // `(user_id, source_id, source_name)` for a listen, `entity_key` for
        // the other two, which is what `/loves/keys` is built on. A second
        // identity for a row that already has one is a second answer free to
        // disagree with the first.
        //
        // **This block must stay last.** Three migrations above rebuild a table
        // by copying an explicit column list into a fresh one -- the `notes`
        // CHECK widenings and the `physical_releases` disc-kinds rebuild -- and
        // a column added before them is silently dropped on exactly the
        // databases that still need the rebuild. That trap is recorded against
        // v0.44.0 and it costs nothing to avoid by appending.
        for table in [
            "notes",
            "note_revisions",
            "signal_chains",
            "chain_variants",
            "equipment",
            "physical_releases",
            "physical_plays",
            "physical_images",
        ] {
            if has_column(&self.pool, table, "uuid").await {
                continue;
            }
            info!("⚙️ Migrating: adding {}.uuid...", table);
            let _ = sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN uuid TEXT"))
                .execute(&self.pool)
                .await;

            // Backfill, or the rows that predate this are the only ones that
            // can never be reconciled -- which is the whole reason to add it
            // before there is a second copy rather than after.
            let ids: Vec<(i64,)> =
                sqlx::query_as(&format!("SELECT id FROM {table} WHERE uuid IS NULL"))
                    .fetch_all(&self.pool)
                    .await
                    .unwrap_or_default();
            if ids.is_empty() {
                continue;
            }
            // One transaction, not one per row. A shelf can hold thousands of
            // plays, and a commit each is the shape that made `rebuild_sessions`
            // run past a proxy timeout on the real host (v0.61.0): no statement
            // is slow, there are simply fifty thousand of them.
            let count = ids.len();
            match self.pool.begin().await {
                Ok(mut tx) => {
                    for (id,) in ids {
                        let _ = sqlx::query(&format!("UPDATE {table} SET uuid = ? WHERE id = ?"))
                            .bind(new_uuid())
                            .bind(id)
                            .execute(&mut *tx)
                            .await;
                    }
                    if let Err(e) = tx.commit().await {
                        warn!(
                            "⚠️ {}.uuid backfill did not commit: {}",
                            table,
                            crate::logging::error_chain(&e)
                        );
                    } else {
                        info!("✅ {} {} row(s) given a stable id", count, table);
                    }
                }
                Err(e) => warn!(
                    "⚠️ {}.uuid backfill could not start: {}",
                    table,
                    crate::logging::error_chain(&e)
                ),
            }
        }

        // ── One format, one spelling, for the rows already written ──
        //
        // `AudioQuality::normalise` folds this at every write path now, and
        // every source before it stored whatever its upstream said: fooyin
        // reports `FLAC` where ffmpeg-derived paths report `flac`, so a history
        // holds both and they are two grouping keys.
        //
        // Marker-guarded rather than run every start. The `WHERE` is
        // self-limiting — after one pass there is nothing left to match — but
        // asking costs a scan of the whole `scrobbles` table, and on the real
        // host that is 77,000 rows off an SD card for an answer that cannot
        // change. Same reasoning as the shelf artwork repair above.
        //
        // `format_type` is the one that was not merely cosmetic: `FIDELITY_KNOWN`
        // and the badge both compare it to a lowercase literal, so an uppercase
        // `DSD` was filed as neither DSD nor known.
        if self.get_app_setting("codec_casing_folded").await.ok().flatten().is_none() {
            let mut folded = 0u64;
            let mut all_ok = true;
            for col in ["codec", "container", "delivery_codec", "format_type"] {
                // `LOWER()` here is SQLite's, which is ASCII-only — and that is
                // exactly right for a codec name. Nothing in this vocabulary is
                // non-Latin, and a Rust-side fold would disagree with the
                // `LOWER()` the queries use.
                let r = sqlx::query(&format!(
                    "UPDATE scrobbles SET {col} = LOWER({col})
                      WHERE {col} IS NOT NULL AND {col} <> LOWER({col})"
                ))
                .execute(&self.pool)
                .await;
                match r {
                    Ok(done) => folded += done.rows_affected(),
                    // Reported rather than swallowed, and the marker is left
                    // unset so the pass runs again — but **`migrate` carries
                    // on**. Returning early here would take the column
                    // migrations below it with it, which is a far worse outcome
                    // than a codec name keeping its capital letters.
                    Err(e) => {
                        all_ok = false;
                        warn!(
                            "⚠️ Could not fold scrobbles.{} casing: {}",
                            col,
                            crate::logging::error_chain(&e)
                        );
                    }
                }
            }
            if folded > 0 {
                info!("⚙️ Migrating: folded {folded} codec/container spelling(s) to one case...");
            }
            if all_ok {
                let _ = self.set_app_setting("codec_casing_folded", "1").await;
            }
        }

        // ── The listens that reported a codec and were never classified ──
        //
        // **Below the casing fold on purpose**: `compute_score` docks by a
        // `match` on lowercase `delivery_codec` spellings, so an unfolded
        // `"AAC"` would take the `_` arm and dock 5 instead of 8.
        //
        // Marker-guarded for the same reason that one is — the `WHERE` is
        // self-limiting, but asking costs a scan of the whole table. A failure
        // leaves the marker unset and `migrate` carries on, because the column
        // migrations below matter more than a score.
        if self.get_app_setting("ingest_quality_classified").await.ok().flatten().is_none() {
            match self.reclassify_unclassified_listens().await {
                Ok(n) => {
                    if n > 0 {
                        info!("✅ Scored {n} listen(s) that had reported a codec and no format");
                    }
                    let _ = self.set_app_setting("ingest_quality_classified", "1").await;
                }
                Err(e) => warn!(
                    "⚠️ Could not score the unclassified listens: {}",
                    crate::logging::error_chain(&e)
                ),
            }
        }

        // ── The MBIDs only a tagging client can supply ──
        //
        // Appended after the uuid block for the reason that block is appended
        // after everything else: three migrations above **rebuild** their table
        // by copying an explicit column list, and a column added before one of
        // them is silently dropped on exactly the databases that still need the
        // rebuild (v0.44.0). New columns go at the bottom.
        //
        // `mbid_release_group` is the one that had been read and thrown away —
        // by ingest, by the ListenBrainz import and by Jellyfin — so an existing
        // history gains the column and starts filling it from the next listen.
        // Nothing backfills the rows already written: the id was never stored,
        // so there is nothing to recover, and asking MusicBrainz per track is
        // what `backfill_recording_mbids` is for.
        for col in ["mbid_release_group", "mbid_release_track", "mbid_work", "mbid_album_artist"] {
            if !has_column(&self.pool, "scrobbles", col).await {
                info!("⚙️ Migrating: adding scrobbles.{}...", col);
                let _ = sqlx::query(&format!("ALTER TABLE scrobbles ADD COLUMN {col} TEXT"))
                    .execute(&self.pool)
                    .await;
            }
        }

        // ── Where a love came from ──
        //
        // `library_ratings` is a new table and `init()` creates it on every
        // start, so only this column needs walking forward.
        //
        // **Every love that predates it reads NULL, and that is the honest
        // value rather than a convenience.** NULL means "not derived from a
        // rating", which is true of all of them — they were pressed by hand or
        // pulled from Last.fm or ListenBrainz — and it is what keeps
        // `withdraw_rating_loves` from ever touching one. Defaulting the column
        // to anything else would put somebody's hand-made loves inside the
        // reach of a threshold they later moved.
        if !has_column(&self.pool, "loves", "source").await {
            info!("⚙️ Migrating: recording where a love came from...");
            let _ =
                sqlx::query("ALTER TABLE loves ADD COLUMN source TEXT").execute(&self.pool).await;
        }

        Ok(())
    }
}
