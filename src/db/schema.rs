//! Every `CREATE TABLE` / `CREATE INDEX` in the application.
//!
//! `init()` is the current shape; `migrate()` (in `migrate.rs`) walks an
//! existing database forward to it. **Both halves are required for a new
//! column** — see the module docs on `migrate`.
//!
//! `init()` runs *first* and propagates with `?`, so anything here that
//! references a migrated column takes the whole process down on an existing
//! database. That is the v0.61.1 crash loop, and it is why an index naming a
//! migrated column belongs in `migrate.rs`, not here.

use super::*;

impl Database {
    /// Every index that is part of the current shape but cannot live in
    /// [`init`].
    ///
    /// These name columns that arrive by `ALTER TABLE` on an existing SQLite
    /// database, and `init()` runs *first* and propagates with `?` — so an
    /// index here placed in `init()` takes startup down on every database that
    /// has not yet gained the column. That is the v0.61.1 crash loop, 297
    /// restarts in 26 minutes.
    ///
    /// So they run **after** `migrate()`, and both backends need them: a fresh
    /// Postgres database reaches the current shape through `init()` alone and
    /// would otherwise have none of these.
    ///
    /// Not every index `migrate()` creates belongs here. `idx_scrobbles_user_source`
    /// deliberately does not: `init()` already declares
    /// `UNIQUE(user_id, source_id, source_name)` inline on the table, so the
    /// constraint `exists` dedup is backstopped by is present on any current
    /// database under whatever name the engine gives it, and the named index
    /// exists only for databases predating the `user_id` column. The guard in
    /// `db::guards` carries that as an exemption with its reason.
    ///
    /// **Two kinds of index here, and they fail differently on purpose.** A
    /// UNIQUE constraint carries correctness — `idx_scrobbles_user_source` is
    /// what `exists` dedup rests on — so failing to create one is fatal and
    /// says so. A performance index is only a degradation, and taking startup
    /// down over one is the v0.61.1 crash loop: an index naming a column some
    /// database does not have, run unconditionally, 297 restarts in 26
    /// minutes. Those are logged and stepped over. `migrate()` swallowed
    /// *both* kinds with `let _ =`, which is why the dedup constraint could
    /// have gone missing without a word.
    ///
    /// `migrate()` still creates its own copies inline, because a table
    /// rebuild drops the table and has to put them back in the same
    /// transaction. They are `IF NOT EXISTS`, so running both is free — and
    /// `every_index_migrate_creates_is_part_of_the_shared_shape` is what stops
    /// the two lists drifting.
    pub(super) async fn indexes(&self) -> Result<(), sqlx::Error> {
        // A UNIQUE constraint the application's correctness rests on.
        macro_rules! constraint {
            ($sql:expr) => {
                self.ddl($sql).await?
            };
        }
        // An index that only makes something faster. Report it and carry on.
        macro_rules! speed {
            ($sql:expr) => {
                if let Err(e) = self.ddl($sql).await {
                    warn!(
                        "⚠️ index not created, queries will be slower: {}",
                        crate::logging::error_chain(&e)
                    );
                }
            };
        }
        // One sitting per host, enforced by the database rather than by a
        // handler that could race a double-click.
        constraint!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_spools_one_per_sitting
                 ON spools(host_user_id, session_uuid) WHERE session_uuid IS NOT NULL"
        );

        // One note per user per target, and a note hangs off exactly one of the
        // four. Partial, so the three NULL targets do not collide with each
        // other.
        constraint!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_entity
                 ON notes(user_id, entity_id) WHERE entity_id IS NOT NULL"
        );
        constraint!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_scrobble
                 ON notes(user_id, scrobble_id) WHERE scrobble_id IS NOT NULL"
        );
        constraint!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_period
                 ON notes(user_id, period_key) WHERE period_key IS NOT NULL"
        );
        constraint!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_line
                 ON notes(user_id, line_key) WHERE line_key IS NOT NULL"
        );

        // A point lookup — "the listens in this session" — which is the
        // workload an index is actually for, and the distinction v0.59.0 drew
        // when it dropped the *expression* indexes: those replaced sequential
        // scans of whole-history aggregations and lost on an SD card.
        speed!(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_session
                 ON scrobbles(session_id) WHERE session_id IS NOT NULL"
        );

        // The same shape one step over: "the listens in this sitting", which is
        // what a Shared Spool's page and its history list both ask, and what
        // `offer_spool_backlog` asks every time somebody joins late.
        //
        // Partial, and here it is barely an index at all — a joint listen is a
        // handful an evening against a history of tens of thousands, so the
        // rows it covers round to nothing while the scan it replaces is the
        // whole table. The v0.59.0 objection does not reach it for the reason
        // it does not reach `idx_scrobbles_session`: this supplies a *seek*,
        // not a grouping key for an aggregation that was going to read
        // everything anyway.
        speed!(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_joint
                 ON scrobbles(joint_session_id) WHERE joint_session_id IS NOT NULL"
        );

        // The album page's "whole record" button: 400,247 VDBE steps to 37.
        // The `WHERE` is load-bearing rather than a size trim — without it the
        // planner takes this index for the top-artist and top-album
        // aggregations too and flips them back to a non-covering walk, which
        // is exactly the v0.59.0 regression.
        speed!(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_album_release
                 ON scrobbles(user_id, LOWER(TRIM(artist)), LOWER(TRIM(album)), mbid_release)
                WHERE mbid_release IS NOT NULL AND TRIM(mbid_release) != ''"
        );

        // Matching a library rating to something in the history, which is a
        // point lookup per rating rather than a grouping — the workload
        // v0.59.0 names as the one an index is actually for. On a table holding
        // a few thousand rows written by a manual pull.
        speed!(
            "CREATE INDEX IF NOT EXISTS idx_library_ratings_key
                 ON library_ratings(user_id, entity_key)"
        );
        speed!(
            "CREATE INDEX IF NOT EXISTS idx_library_ratings_match
                 ON library_ratings(user_id, match_key) WHERE match_key IS NOT NULL"
        );

        speed!("CREATE INDEX IF NOT EXISTS idx_entities_match ON entities(match_key)");
        speed!("CREATE INDEX IF NOT EXISTS idx_entities_mbid ON entities(kind, mbid)");

        // The stable ids a replica reconciles on.
        //
        // Justified where v0.59.0 dropped three indexes as a net loss, by that
        // note's own test: those replaced *sequential scans of whole-history
        // aggregations*, where an index scan costs a rowid lookup per row on an
        // SD card. These serve a **point lookup** — "the row with this id" —
        // which is the workload it names as the one an index is actually for,
        // and they sit on tables holding hundreds of rows written a handful of
        // times a day rather than on `scrobbles`.
        //
        // UNIQUE rather than a plain index because the uniqueness is the
        // property, not the speed: two rows sharing an id is a reconciliation
        // silently applying one write over another. NULL is allowed many times
        // over in both engines, so a database mid-backfill is fine.
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
            constraint!(&format!(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_{table}_uuid ON {table}(uuid)"
            ));
        }
        Ok(())
    }

    /// Run one DDL statement, translated into the active dialect.
    ///
    /// Every `CREATE` in this file is written in SQLite's spelling and mapped
    /// by [`dialect::rewrite_ddl`] — **the schema is translated, never written
    /// twice.** A hand-maintained second copy of 48 tables would be free to
    /// drift from the first, and a schema that disagrees with itself between
    /// backends is the worst thing this application could own.
    async fn ddl(&self, sql: &str) -> Result<(), sqlx::Error> {
        crate::db::run_ddl(&self.pool, sql).await
    }

    pub(super) async fn init(&self) -> Result<(), sqlx::Error> {
        self.ddl(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT UNIQUE NOT NULL,
                display_name TEXT,
                password_hash TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'user',
                created_at INTEGER NOT NULL,
                settings JSON DEFAULT '{}',
                disabled INTEGER NOT NULL DEFAULT 0,
                avatar_url TEXT,
                bio TEXT,
                -- The one deliberately public read path in this application.
                -- Off by default and never inferred: everything else here is
                -- self-only, so publishing has to be an act, not a default.
                public_now_playing INTEGER NOT NULL DEFAULT 0,
                -- Which *extra* fields the publish switch carries, as a JSON
                -- array drawn from `PublicNpFields::KNOWN`. Title, artist and
                -- album are the whole of the base payload; everything richer
                -- than that is opted into one field at a time.
                public_np_fields TEXT,
                -- Whether this account answers to the rest of the fediverse at
                -- all: an actor document, a WebFinger record, an inbox that
                -- takes deliveries.
                --
                -- Off by default and off for every account that predates the
                -- column, for the reason `public_now_playing` is: serving an
                -- actor publishes a handle, a display name, a bio and an avatar
                -- to any stranger who asks, and nobody consents to that by
                -- their operator upgrading a binary. Ashwin's call, 2026-08-22.
                --
                -- Note this is *not* the same question as whether the deck is
                -- public. It is the question of whether the person can be found
                -- and patched into at all -- following happens while private in
                -- this design, so being discoverable and being readable are two
                -- switches and always will be.
                federation_enabled INTEGER NOT NULL DEFAULT 0,
                -- Whether a deck you have patched with sees **all** your
                -- listens, or only those through chains you share.
                --
                -- Off by default, which is the old behaviour exactly. See
                -- `PATCH_VISIBLE_LISTENS` for what it opens up and, just as
                -- importantly, what it does not.
                share_all_with_patched INTEGER NOT NULL DEFAULT 0,
                -- Whether a play accepted out of a Shared Spool is forwarded on
                -- to this account's Last.fm / ListenBrainz, or stored here
                -- only.
                --
                -- Off by default and off for every account that predates the
                -- column, for the reason the two switches above are: turning it
                -- on writes to somebody's permanent public record, and nobody
                -- consents to that by their operator upgrading a binary.
                --
                -- It is a *user* setting rather than a per-connection one
                -- because the question is about the listening, not about any
                -- one service: either somebody else's deck playing a record you
                -- were in the room for counts as your listen or it does not,
                -- and that answer cannot differ between Last.fm and
                -- ListenBrainz without meaning two different things at once.
                forward_spool_listens INTEGER NOT NULL DEFAULT 0,
                -- Which chain an accepted Shared Spool play is filed under —
                -- the room you were in rather than a rig of your own.
                --
                -- A capture used to land with no chain at all, so those listens
                -- earned no gear hours, carried no fidelity and could never
                -- reach the Reel. That is defensible (it was not your gear) and
                -- it is not the whole truth either: you were in the room
                -- hearing real speakers, and somebody who has built a
                -- Noor's-living-room chain should be able to say so. Unset is the
                -- default and reproduces the old behaviour exactly.
                --
                -- **Deliberately no `REFERENCES signal_chains(id)`**, and this
                -- is the forward-reference trap rather than an oversight:
                -- `users` is created before `signal_chains` and `signal_chains`
                -- references `users`, so the graph has a cycle and Postgres —
                -- which resolves a FK target eagerly where SQLite does not —
                -- would refuse the whole `init()`. `tokens.default_chain_id`
                -- gets the constraint only because `tokens` is created after
                -- both. What actually matters here is ownership, which no
                -- foreign key would have checked anyway: `set_spool_chain`
                -- scopes the lookup by `user_id`.
                spool_chain_id INTEGER
            )",
        )
        .await?;
        self.ddl(
            "CREATE TABLE IF NOT EXISTS signal_chains (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Stable id for replicas; see the note on `notes.uuid`.
                -- referenced by scrobbles.chain_id, so this gets worse with every listen.
                uuid TEXT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                name TEXT NOT NULL, description TEXT,
                components JSON NOT NULL DEFAULT '[]',
                listening_context TEXT NOT NULL DEFAULT 'unknown',
                icon TEXT,
                is_active BOOLEAN DEFAULT TRUE,
                total_hours REAL DEFAULT 0,
                created_at INTEGER NOT NULL,
                -- Phase 6: the sharing unit is the chain, not the listen.
                --
                -- Off by default, and it is what decides whether a listen ever
                -- reaches the Reel, the instance figures or an outbox. There is
                -- no per-listen override and there should not be one: this is a
                -- standing decision about *how you listen*, which is a thing
                -- somebody can hold in their head, where a choice per play is
                -- not. It is the reason private-by-default is not also useless.
                shared BOOLEAN NOT NULL DEFAULT FALSE,
                UNIQUE(user_id, name)
            )",
        )
        .await?;
        self.ddl(
            "CREATE TABLE IF NOT EXISTS tokens (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                token_hash TEXT UNIQUE NOT NULL,
                name TEXT NOT NULL,
                scopes TEXT DEFAULT 'submit',
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                expires_at INTEGER,
                default_chain_id INTEGER REFERENCES signal_chains(id),
                -- Whether this client's import-type submissions are
                -- live listens rather than a backfill. Off by default and never
                -- inferred: turning it on lets a client write to the owner's
                -- permanent Last.fm/ListenBrainz record, which is not a decision
                -- to make on their behalf.
                imports_are_live BOOLEAN DEFAULT FALSE,
                -- The `user_sources` row this token was made for, when it was
                -- made on a source (a Navidrome plugin token). Its existence is
                -- what puts that source in plugin mode: the engine stops
                -- polling it and listens arrive through the token instead.
                source_ref INTEGER
            )",
        )
        .await?;
        self.ddl(
            "CREATE TABLE IF NOT EXISTS devices (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                machine_id TEXT NOT NULL, name TEXT, platform TEXT,
                product TEXT, device_type TEXT,
                default_chain_id INTEGER REFERENCES signal_chains(id),
                first_seen INTEGER NOT NULL, last_seen INTEGER NOT NULL,
                total_listens INTEGER DEFAULT 0,
                UNIQUE(user_id, machine_id)
            )",
        )
        .await?;
        self.ddl(
            "CREATE TABLE IF NOT EXISTS equipment (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Stable id for replicas; see the note on `notes.uuid`.
                -- chains name it, and a replica has to agree which box that is.
                uuid TEXT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                name TEXT NOT NULL, type TEXT NOT NULL,
                brand TEXT, model TEXT,
                total_hours REAL DEFAULT 0,
                first_used INTEGER, last_used INTEGER, notes TEXT,
                -- Distinct from `first_used`, which is when Tapedeck first saw
                -- it. Gear is usually owned for a while before it is logged,
                -- and burn-in hours are counted from the purchase, not from the
                -- day someone got round to typing it in.
                purchased_at INTEGER,
                -- Why a piece of gear is not in a chain: 'new', 'spare',
                -- 'lent'. Free text rather than a CHECK, for the reason
                -- `equipment_type` is — the UI owns the vocabulary, and a
                -- constraint here would mean a rebuild to add a word. NULL is
                -- the usual state and means nothing has been said.
                status TEXT,
                UNIQUE(user_id, name)
            )",
        )
        .await?;
        self.ddl(
            "CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                device_id INTEGER REFERENCES devices(id),
                chain_id INTEGER REFERENCES signal_chains(id),
                started_at INTEGER NOT NULL, ended_at INTEGER,
                track_count INTEGER DEFAULT 0, total_duration INTEGER DEFAULT 0,
                skip_count INTEGER DEFAULT 0, avg_quality_score REAL,
                listening_context TEXT DEFAULT 'unknown',
                token_id INTEGER REFERENCES tokens(id),
                uuid TEXT,
                title TEXT,
                -- A note about the sitting itself, distinct from a liner note
                -- about a record, e.g. a first listen on a new cartridge.
                notes TEXT,
                updated_at INTEGER
            )",
        )
        .await?;

        // A chain with one step swapped.
        //
        // Two cartridges on one turntable, or the same commute with a different
        // pair of IEMs, are not two signal chains — they are one chain and a
        // choice. Modelling them as separate chains is what made a shelf of
        // eight chains unreadable: "Commute Phone" and "Summer Commute"
        // differed by exactly one component, and every gear swap forever adds
        // another near-duplicate row.
        //
        // `overrides` is a JSON array of `{position, equipment_id, name}`, where
        // `position` indexes into the parent chain's `components`. Only the
        // steps that differ are stored, so editing the shared part of a chain
        // updates every variant of it — which is the entire reason this is an
        // override rather than a copy.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS chain_variants (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Stable id for replicas; see the note on `notes.uuid`.
                -- referenced by scrobbles.chain_variant_id.
                uuid TEXT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                chain_id INTEGER NOT NULL REFERENCES signal_chains(id),
                name TEXT NOT NULL,
                overrides JSON NOT NULL DEFAULT '[]',
                created_at INTEGER NOT NULL,
                UNIQUE(chain_id, name)
            )",
        )
        .await?;
        self.ddl(
            "CREATE TABLE IF NOT EXISTS scrobbles (
                -- which variant of that chain was fitted. `migrate()` adds it too, for databases predating it.
                chain_variant_id INTEGER REFERENCES chain_variants(id),
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL DEFAULT 1,
                title TEXT NOT NULL, artist TEXT NOT NULL, album TEXT,
                artist_credit TEXT,
                timestamp INTEGER NOT NULL, duration INTEGER,
                source_id TEXT NOT NULL, source_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                mbid_recording TEXT, mbid_release TEXT, mbid_artist TEXT,
                -- The release group, the release-track and the work.
                --
                -- The first was **parsed and discarded for months**: ingest read
                -- it out of `additional_info`, the ListenBrainz import read it
                -- out of `mbid_mapping`, Jellyfin read `MusicBrainzReleaseGroup`
                -- off the session — and `Play` carried it to an insert that had
                -- no column for it. Three sources collecting a field that went
                -- nowhere, with nothing erroring to say so.
                --
                -- The other two only ever come from a tagging client, so there
                -- is no mapping and no backfill behind them: not stored here is
                -- not stored anywhere. `mbid_work` is the one that matters for
                -- classical, where a single work spans dozens of recordings by
                -- different performers and nothing else in this schema relates
                -- them.
                mbid_release_group TEXT, mbid_release_track TEXT, mbid_work TEXT,
                -- A JSON array, mirroring `mbid_artist`: a record can be
                -- credited to several people.
                mbid_album_artist TEXT,
                caa_id INTEGER, caa_release_mbid TEXT,
                -- The recording's International Standard Recording Code. The
                -- only identifier that survives crossing between a library, a
                -- streaming catalogue and MusicBrainz, so it is what lets a
                -- playlist push ask for a track *exactly* rather than searching
                -- by name and hoping the spellings agree.
                isrc TEXT,
                format_type TEXT, codec TEXT, bitrate INTEGER,
                sample_rate INTEGER, bit_depth INTEGER, channels INTEGER,
                container TEXT, source_quality TEXT,
                is_lossless BOOLEAN, dsd_rate INTEGER, dsd_multiplier INTEGER,
                dsd_to_pcm_converted BOOLEAN,
                delivery_codec TEXT, delivery_bitrate INTEGER,
                delivery_sample_rate INTEGER, delivery_bit_depth INTEGER,
                is_transcoded BOOLEAN, transcode_reason TEXT, quality_score REAL,
                device_id INTEGER REFERENCES devices(id),
                chain_id INTEGER REFERENCES signal_chains(id),
                session_id INTEGER REFERENCES sessions(id),
                listening_context TEXT, submission_client TEXT,
                delivered_sinks TEXT NOT NULL DEFAULT '[]',
                track_number INTEGER,
                language TEXT,
                -- Where that language came from: 'script' (the title's own
                -- script, certain), 'lyrics' (the script of the lyric, which
                -- resolves a transliterated title), 'work' (MusicBrainz's
                -- editorial work language) or 'default' (a shrug). Different
                -- measurements of the same thing, and a breakdown that mixed
                -- them silently would be reporting two populations as one.
                language_source TEXT,
                genres TEXT,
                release_year INTEGER,
                skipped BOOLEAN NOT NULL DEFAULT FALSE,
                artwork_url TEXT,
                source_ref INTEGER,
                spotify_track_id TEXT,
                listened_ms INTEGER,
                is_shuffle BOOLEAN,
                queue_source TEXT,
                -- The Shared Spool this listen belonged to, as the spool's
                -- uuid. A joint listen is N rows sharing this, one per
                -- listener — never one row with several owners, because every
                -- query here scopes by `user_id`.
                joint_session_id TEXT,
                -- Who you were listening with, as names, or NULL for not
                -- marked. **On the listen and not on the session**, which is
                -- not a preference: `rebuild_sessions` deletes every session
                -- and re-inserts it, so anything held there is erased by a
                -- regroup with nothing to say so. A spool fills this in
                -- automatically; everything else is marked by hand, usually
                -- afterwards.
                --
                -- Free text on purpose. The people you listen with mostly do
                -- not have accounts on this instance, and a feature that can
                -- only record the ones who do would miss the common case.
                company TEXT,
                -- What the clock said before anybody corrected it, written
                -- **once** on the first hand edit and never again.
                --
                -- Several paths produce a timestamp that is only approximately
                -- right: a capture accepted out of a Shared Spool carries the
                -- *host's* clock, a reconstructed evening replays an old one,
                -- and the shelf lays a side out by arithmetic from the moment
                -- the needle went down. Correcting those is the point of the
                -- edit; keeping what was there is what separates a listen
                -- that is at 9pm from one that is at 9pm and was recorded at
                -- 8pm. `note_revisions` is the precedent — an edit here never
                -- destroys the observation it replaces.
                timestamp_original INTEGER,
                UNIQUE(user_id, source_id, source_name)
            )",
        )
        .await?;

        // ── Physical media ──
        //
        // A record or tape you own. Separate from `entities` on purpose: an
        // entity is a work you have listened to, this is an *object* with a
        // pressing, a barcode and a side order. Two pressings of the same album
        // are two rows here and one entity.
        //
        // `tracks` is a JSON array of {side, position, title, duration_secs}.
        // A blob rather than a table because it is only ever read and written
        // whole, and because side assignment is frequently hand-corrected —
        // MusicBrainz records vinyl track numbers as "A1" only when the editor
        // bothered, and as plain 1..n otherwise.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS physical_releases (
                -- the variant this pressing usually runs through. `migrate()` adds it too, for databases predating it.
                chain_variant_id INTEGER REFERENCES chain_variants(id),
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Stable id for replicas; see the note on `notes.uuid`.
                -- the shelf's own rows reference it.
                uuid TEXT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                kind TEXT NOT NULL CHECK (kind IN ('vinyl', 'cassette', 'cd', 'sacd')),
                artist TEXT NOT NULL, title TEXT NOT NULL,
                year INTEGER,
                mbid_release TEXT, discogs_id TEXT, barcode TEXT, catalog_number TEXT,
                artwork_url TEXT,
                tracks TEXT NOT NULL DEFAULT '[]',
                notes TEXT,
                added_at INTEGER NOT NULL,
                -- When you bought it. A shelf is a thing you accumulate, so how
                -- long something has been yours is a fact about the shelf that
                -- nothing else in the database can answer.
                purchased_at INTEGER,
                -- How it came to be yours: 'new', 'used', 'gift', 'inherited'.
                -- Nullable, and null means nobody said — never 'new', because
                -- an existing shelf must not be retroactively claimed as bought
                -- new. Deliberately *not* condition grading (M/NM/VG+): that
                -- describes the object's wear rather than where it came from,
                -- and it is the field people abandon filling in.
                acquired_as TEXT,
                -- The shop, the fair, the person. Free text, because that is
                -- what makes a thrift find a story rather than a flag.
                acquired_from TEXT,
                -- What this normally gets played on. A record lives with one
                -- turntable, so asking every time would be asking the same
                -- question forever; the deck pre-fills from here and the answer
                -- is written back.
                chain_id INTEGER REFERENCES signal_chains(id),
                equipment_id INTEGER REFERENCES equipment(id),
                -- Where the tape was left. A cassette does not rewind itself:
                -- stop it halfway and eject, and it is still parked there the
                -- next time you put it in. That is a fact about the object,
                -- which is what this shelf is for. Written for cassettes only —
                -- see `park_tape` for why a record deliberately has no memory.
                parked_side TEXT,
                parked_secs INTEGER,
                -- Where it lives: a room, a shelf, the car. Free text,
                -- because a shelf of three hundred records is only useful if
                -- you can find the one you want on it.
                location TEXT,
                -- The edition as the sleeve or the sticker says it: a colour,
                -- a weight, Record Store Day. Free text on purpose — a
                -- vocabulary would be out of date by the next pressing.
                edition TEXT,
                country TEXT,
                -- The record label. `catalog_number` without it is half a
                -- citation.
                label TEXT
            )",
        )
        .await?;

        // What is on the deck right now, as a fact about the *shelf* rather
        // than about a browser.
        //
        // This started in localStorage on the reasoning that a side in progress
        // is a fact about the machine sitting in front of the turntable. That
        // is true of where the *clock* runs and false of everything else: a
        // record is on the platter whichever device you happen to be holding,
        // so the phone in your pocket could not see it, a second tab disagreed
        // with the first, and a browser crash lost the side outright with no
        // way to recover what had already played.
        //
        // `user_id` is the PRIMARY KEY, which is the constraint that matters:
        // one deck per person, because nobody listens to two records at once,
        // and it makes "resume what I was playing" a single row lookup with no
        // ambiguity about which spin is the real one.
        //
        // `outcomes` is the per-track record the browser accumulates (position,
        // real start, skipped) and is JSON because it is written and read
        // whole, never queried into.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS physical_spins (
                user_id INTEGER PRIMARY KEY REFERENCES users(id),
                release_id INTEGER NOT NULL REFERENCES physical_releases(id),
                side TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                paused_at INTEGER,
                paused_total INTEGER NOT NULL DEFAULT 0,
                outcomes TEXT NOT NULL DEFAULT '[]',
                chain_id INTEGER REFERENCES signal_chains(id),
                equipment_id INTEGER REFERENCES equipment(id),
                -- How far into the side this spin began. Non-zero when the
                -- needle was dropped mid-side, or when a tape resumed from
                -- where it was parked, so the countdown reads a position
                -- within the side rather than starting from nothing.
                start_secs INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )",
        )
        .await?;

        // Sleeves, booklets, labels, obi strips — the parts of a record you
        // actually hold. A release has *many* of these (the Cover Art Archive
        // carries 29 for the pressing this was built against, 19 of them
        // booklet pages), which is why they are rows rather than a second
        // `artwork_url` column.
        //
        // Two provenances share the table because they answer the same question
        // and the difference matters only to the reader: `caa`/`discogs` hold a
        // remote **URL** — the same rule the artwork chain follows, providers
        // return URLs and never bytes — while `upload` holds a **file on this
        // server**, because a scan of your own copy exists nowhere else.
        //
        // `kind` is free text rather than a CHECK: the archive adds types, and a
        // scan of something it has no word for is still a scan.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS physical_images (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Stable id for replicas; see the note on `notes.uuid`.
                -- an upload is the one image a provider cannot re-supply.
                uuid TEXT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                release_id INTEGER NOT NULL REFERENCES physical_releases(id),
                kind TEXT NOT NULL DEFAULT 'other',
                provider TEXT NOT NULL,
                url TEXT, thumb_url TEXT,
                file_path TEXT, mime TEXT, bytes INTEGER,
                caption TEXT,
                -- Order within the release, which for a booklet is page order.
                position INTEGER NOT NULL DEFAULT 0,
                -- The provider's own id, so re-fetching a release updates the
                -- rows it already has instead of stacking a second copy of
                -- every page on top of the first.
                remote_id TEXT,
                added_at INTEGER NOT NULL
            )",
        )
        .await?;
        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_physical_images_remote
                ON physical_images(release_id, provider, remote_id)
             WHERE remote_id IS NOT NULL",
        )
        .await?;
        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_physical_images_release
                ON physical_images(release_id, position)",
        )
        .await?;

        // One row per *side played*, which is the unit you actually handle: you
        // put a side on, it plays through, you flip it. The listens themselves
        // are ordinary `scrobbles` rows — this is the record of the physical
        // act, and what stylus hours and a cassette counter are computed from.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS physical_plays (
                -- which variant of that chain was fitted. `migrate()` adds it too, for databases predating it.
                chain_variant_id INTEGER REFERENCES chain_variants(id),
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Stable id for replicas; see the note on `notes.uuid`.
                -- a side played is a fact, and it is deletable.
                uuid TEXT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                release_id INTEGER NOT NULL REFERENCES physical_releases(id),
                side TEXT NOT NULL,
                played_at INTEGER NOT NULL,
                duration_secs INTEGER NOT NULL DEFAULT 0,
                track_count INTEGER NOT NULL DEFAULT 0,
                equipment_id INTEGER REFERENCES equipment(id),
                chain_id INTEGER REFERENCES signal_chains(id),
                -- When the play was *written*, as opposed to when the side
                -- started. The pair is the only record of how long the record
                -- was actually on: `played_at` alone cannot say whether a side
                -- ran straight through or was left half-played over an
                -- afternoon, which is what made a set of listens land on top of
                -- everything else heard in between. Nullable, because for every
                -- play recorded before this column there is no answer, and
                -- inventing one would be the same mistake again.
                recorded_at INTEGER
            )",
        )
        .await?;

        // Server-wide settings an admin sets in the UI, as opposed to
        // `tapedeck.toml` (infrastructure) or per-user preferences. Values are
        // encrypted, because the first inhabitant is a Discogs token.
        // Device-code pairing. A client that cannot show a browser — a DAP, a
        // headless player, a TV app — asks for a code here and polls; the user
        // approves it in the web UI and the client collects its token.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS pairing_requests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Hashed like a token: the device code is a bearer secret, and
                -- a leaked database should not hand over pending pairings.
                device_code_hash TEXT NOT NULL UNIQUE,
                -- Short and typeable, so it can be read off one screen and into
                -- another. Guessable by construction, which is why it lives for
                -- five minutes, is single-use, and is rate limited.
                user_code TEXT NOT NULL UNIQUE,
                client_name TEXT NOT NULL,
                scopes TEXT NOT NULL DEFAULT 'submit',
                user_id INTEGER REFERENCES users(id),
                default_chain_id INTEGER REFERENCES signal_chains(id),
                -- The minted token, held only until the client collects it.
                token TEXT,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                approved_at INTEGER
            )",
        )
        .await?;

        self.ddl(
            "CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            )",
        )
        .await?;

        // ── Web Sessions (browser login) ──
        self.ddl(
            "CREATE TABLE IF NOT EXISTS web_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                session_hash TEXT UNIQUE NOT NULL,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                last_active_at INTEGER NOT NULL
            )",
        )
        .await?;

        // ── Output-device → chain bindings (Phase B) ──
        // One row per distinct output the client reports (USB DAC name, BT
        // device, "wired", …). chain_id NULL = seen but unmapped, which the UI
        // surfaces for one-tap assignment (auto-learn).
        self.ddl(
            "CREATE TABLE IF NOT EXISTS chain_bindings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                identifier TEXT NOT NULL,
                interface TEXT,
                chain_id INTEGER REFERENCES signal_chains(id),
                last_seen INTEGER NOT NULL,
                hits INTEGER NOT NULL DEFAULT 0,
                UNIQUE(user_id, identifier)
            )",
        )
        .await?;

        // ── Per-user scrobble connections (Last.fm / ListenBrainz) ──
        // Credentials stay server-side; the API only ever exposes the connected
        // account name, never the token/session key.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS user_connections (
                user_id INTEGER NOT NULL REFERENCES users(id),
                service TEXT NOT NULL,
                credentials TEXT NOT NULL,
                connected_username TEXT,
                connected_at INTEGER NOT NULL,
                PRIMARY KEY (user_id, service)
            )",
        )
        .await?;

        // ── Import history (one row per finished import job) ──
        self.ddl(
            "CREATE TABLE IF NOT EXISTS import_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                source TEXT NOT NULL,
                state TEXT NOT NULL,
                fetched INTEGER NOT NULL,
                imported INTEGER NOT NULL,
                status TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .await?;

        // Artists as entities rather than bare strings on a listen. Needed for
        // anywhere an artist has properties of its own — an image today, MBIDs
        // and liner notes later. Global, not per-user: "Aphex Twin" is the same
        // artist for everyone on the instance.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS artists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                name_key TEXT NOT NULL UNIQUE,
                mbid TEXT,
                -- The square one: a portrait, used wherever an artist appears
                -- in a list or beside a listen.
                image_url TEXT,
                -- The wide one. A Reports chapter gives its top artist a whole
                -- tile, and a square crop either pillarboxes or loses the head;
                -- a giant initial was the placeholder standing in for this.
                -- Kept as its own column rather than a crop of the portrait,
                -- because the two are different photographs, not two framings.
                landscape_url TEXT,
                genres TEXT,
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .await?;

        // ── Entities, liner notes and loves (Phase 7) ──
        //
        // Anything you can write about: a recording, a release, an artist.
        // Deliberately *not* keyed by MBID — an un-enriched listen has none, so
        // notes would silently fail on exactly the obscure records worth
        // annotating. The MBID is one alias among others.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS entities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                artist_name TEXT NOT NULL DEFAULT '',
                entity_key TEXT NOT NULL UNIQUE,
                -- The forgiving key. Deliberately **not** UNIQUE: the whole
                -- reason it exists is that entities already exist which fold
                -- together, and a unique index would refuse to create it on
                -- exactly the databases that need it. See `Database::match_key`.
                match_key TEXT,
                mbid TEXT,
                image_url TEXT,
                created_at INTEGER NOT NULL
            )",
        )
        .await?;

        // Other names the same entity answers to. Today this carries MBIDs; it
        // is what makes merging two spellings of one record possible later
        // without rewriting every note that points at either.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS bio_cache (
                entity_id INTEGER PRIMARY KEY REFERENCES entities(id),
                bio TEXT,
                attribution TEXT,
                provider TEXT,
                url TEXT,
                fetched_at INTEGER NOT NULL
            )",
        )
        .await?;

        self.ddl(
            "CREATE TABLE IF NOT EXISTS entity_aliases (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_id INTEGER NOT NULL REFERENCES entities(id),
                alias_kind TEXT NOT NULL,
                value TEXT NOT NULL,
                UNIQUE(entity_id, alias_kind, value)
            )",
        )
        .await?;
        self.ddl("CREATE INDEX IF NOT EXISTS idx_entity_aliases_lookup ON entity_aliases(alias_kind, value)").await?;

        // `scrobbles` carried no index at all beyond the UNIQUE it needs for
        // dedup, so every ranged query — history, stats, all seven rediscovery
        // collections — was a full scan of the user's rows plus a temp B-tree.
        // Fine at 21k rows on a laptop, and the reason Rediscovery could take
        // long enough on a small host to be killed by a gateway timeout.
        //
        // `CREATE INDEX IF NOT EXISTS` is idempotent, so this needs no
        // `migrate()` block — an existing database gains the index on next
        // open. That is the one schema change that does not need the
        // add-to-init-*and*-migrate dance.
        //
        // `(user_id, timestamp)` is the shape nearly every query wants: scoped
        // to the caller, then bounded or ordered by time. `skipped` is left out
        // deliberately — it is a low-cardinality flag, and putting it in front
        // of `timestamp` would help no range scan while making the index wider.
        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_user_time ON scrobbles(user_id, timestamp)",
        )
        .await?;
        // Grouping queries (top artists/albums/tracks, the variant pass) sort
        // by artist within a user before they aggregate.
        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_scrobbles_user_artist ON scrobbles(user_id, artist)",
        )
        .await?;

        // **Almost no expression indexes on the folded name** — the single
        // exception is `idx_scrobbles_album_release`, which lives in
        // `migrate()` because it names a migrated column, and carries the
        // argument for why it is not a repeat of what follows.
        //
        // Three were
        // added in v0.56.0 — `(user_id, LOWER(TRIM(artist)))` and the
        // title/album pairs — because they take a bare `GROUP BY
        // LOWER(TRIM(artist))` from ~50ms to under 10ms on a laptop, and the
        // plan stops saying `USE TEMP B-TREE FOR GROUP BY`. They were removed
        // in v0.59.0 after measuring on the hardware that runs this.
        //
        // On a Raspberry Pi with the database on an SD card they were a **net
        // loss**: Statistics went 3.9s → 6.4s, dashboard 0.85s → 1.26s, and
        // adding `ANALYZE` recovered almost none of it. The reason is a cost
        // model that does not know this storage: an index scan means a rowid
        // lookup per row, which is *random* I/O, while the full scan it
        // replaced is sequential. For a query that touches most rows and reads
        // several columns — which every analytics query here does — sequential
        // wins by more than the sort costs.
        //
        // The lesson worth keeping is not "indexes are bad". It is that this
        // application's reads are **whole-history aggregations on slow
        // sequential storage**, which is close to the opposite of the
        // point-lookup workload an index is for. `idx_scrobbles_user_time`
        // earns its place because it also supplies an *order* the queries want;
        // an index that only supplies a grouping key does not.
        //
        // Before adding one here, measure on the Pi — `temp/measure-tapedeck.sh`
        // — and compare `EXPLAIN QUERY PLAN` either side. A laptop cannot see
        // this cost at all.

        // ── Who played it ────────────────────────────────────────────────
        //
        // `scrobbles.artist` holds **one** name, and every chart in this
        // application groups by it — so a performer got no credit anywhere. Play
        // "Hilary Hahn plays Bach" and Bach takes the listen; Hilary Hahn was
        // written into `artist_credit` as text and counted by nothing. Same for
        // Pascal Rogé on Debussy, and for every soloist, conductor and ensemble
        // in a classical library. `artist_credit` was read in exactly one place
        // — the Various Artists fallback in Reports.
        //
        // One row per credited artist per listen. The primary is `position = 0`
        // and is the same name `scrobbles.artist` holds, so a query that fans
        // out over this table and one that does not still agree about who the
        // record is by.
        //
        // `artist_key` is `LOWER(TRIM(name))` — **the same fold the charts and
        // `entity_key` already use**, deliberately not `normalise_artist`. That
        // one answers "same performer?" for the sanitiser's review preview,
        // where over-folding invents a merge; this has to agree with the keys
        // hearts and entities are stored under or the two silently disagree
        // about who an artist is. There is no third normaliser here.
        //
        // The primary key is `(scrobble_id, artist_key)`, so a listen crediting
        // the same artist twice — which real tags do, "Bach; J.S. Bach" — counts
        // once. Same rule as the `fitted` set in `equipment_seconds`.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS scrobble_artists (
                scrobble_id INTEGER NOT NULL REFERENCES scrobbles(id),
                artist_key TEXT NOT NULL,
                name TEXT NOT NULL,
                position INTEGER NOT NULL DEFAULT 0,
                mbid TEXT,
                PRIMARY KEY (scrobble_id, artist_key)
            ) WITHOUT ROWID",
        )
        .await?;
        // Unlike the expression indexes the note above rejects, this one is for
        // a **point lookup** — "every listen crediting this artist", which is
        // what an artist page asks — and without it that question is a full scan
        // of the join table per visit. It is the workload an index is actually
        // for. The whole-history direction (top artists) still scans, which is
        // correct on this storage.
        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_scrobble_artists_key
                 ON scrobble_artists(artist_key, scrobble_id)",
        )
        .await?;

        // A note is a living document: one per user per target, edited in place.
        // Exactly one of entity_id / scrobble_id / period_key is set. A note
        // about a record, a note about one particular playing of it, and a note
        // about the month you spent listening to it are three different things.
        //
        // `period_key` is `<unit>:<key>` — `month:2026-08`, `week:2026-W32`,
        // `year:2026` — the same key `reports::period_of` builds. It is a text
        // key rather than a foreign key because a period is not a row anywhere:
        // reports are folded from the history on every request, so there is
        // nothing to point at. That key is stable by construction, which is
        // what makes it safe to write down — including for weeks, which
        // deliberately ignore the user's week-start preference so a named
        // period cannot change shape under a note about it.
        //
        // `line_key` is the fourth target: a **pin**, one numbered line of a
        // lyric. Also a text key rather than a foreign key, and for a sharper
        // version of the same reason — the lyric it points into lives in a
        // *different database file* (the metadata cache, deliberately outside
        // `VACUUM INTO`), so there is nothing here to reference even in
        // principle. `line_label` rides beside it because the fold has to
        // happen in Rust; see [`NoteRow::line_label`].
        self.ddl(
            "CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- A stable id that survives leaving this database.
                --
                -- Nothing reads it yet. It is here now because retrofitting an
                -- id onto rows another instance already holds is the migration
                -- Phase 6 warns about, and it is the same call `sessions.uuid`
                -- made before anything shared one outward. Tables with a
                -- natural key -- `scrobbles(user_id, source_id, source_name)`,
                -- `loves`/`entities` on `entity_key` -- deliberately have none:
                -- a second identity for a row that already has one is a second
                -- answer free to disagree with the first.
                --
                -- the note itself is the thing another replica holds.
                uuid TEXT,
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
                -- Exactly one target. Written as CASE rather than by summing the
                -- predicates: SQLite treats a boolean as an integer and adds them
                -- happily, where Postgres refuses with `operator does not exist:
                -- boolean + boolean`. CASE is standard SQL and means the same in both.
                CHECK ((CASE WHEN entity_id   IS NOT NULL THEN 1 ELSE 0 END)
                     + (CASE WHEN scrobble_id IS NOT NULL THEN 1 ELSE 0 END)
                     + (CASE WHEN period_key  IS NOT NULL THEN 1 ELSE 0 END)
                     + (CASE WHEN line_key    IS NOT NULL THEN 1 ELSE 0 END) = 1),
                CHECK ((line_key IS NULL) = (line_label IS NULL))
            )",
        )
        .await?;
        // Partial uniques: one note per target per user, enforced by the
        // database rather than by every write path remembering to check.
        self.ddl("CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_entity ON notes(user_id, entity_id) WHERE entity_id IS NOT NULL").await?;
        self.ddl("CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_user_scrobble ON notes(user_id, scrobble_id) WHERE scrobble_id IS NOT NULL").await?;

        // Every edit keeps what was there before. These are meant to be kept for
        // years; a rewrite in 2031 shouldn't erase what you thought in 2026.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS note_revisions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Stable id for replicas; see the note on `notes.uuid`.
                -- a revision is how a concurrent edit survives as a choice rather than a loss.
                uuid TEXT,
                note_id INTEGER NOT NULL REFERENCES notes(id),
                body_md TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .await?;
        self.ddl("CREATE INDEX IF NOT EXISTS idx_note_revisions_note ON note_revisions(note_id, created_at)").await?;

        self.ddl(
            "CREATE TABLE IF NOT EXISTS loves (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                entity_id INTEGER NOT NULL REFERENCES entities(id),
                -- Where this love came from, and it exists to protect the ones
                -- that came from a person. `rating` marks a love derived from a
                -- library star rating under a threshold, so raising that
                -- threshold can withdraw it again. NULL is everything else —
                -- pressed by hand, or pulled from Last.fm or ListenBrainz — and
                -- **nothing may ever remove one of those automatically**. A
                -- love is a statement somebody made; only the ones this
                -- instance derived are this instance\'s to take back.
                source TEXT,
                created_at INTEGER NOT NULL,
                UNIQUE(user_id, entity_id)
            )",
        )
        .await?;

        // One row per user per source kind. Replaces the single-row
        // `plex_settings`: a source belongs to the person whose account it
        // reads, so nobody's token can see anybody else's listening.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS user_sources (
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
        )
        .await?;

        // Star ratings lifted off a media server, exactly as that server gave
        // them.
        //
        // **The raw number and what it was out of, never a normalised one.**
        // Plex counts to ten, Subsonic to five, and a bare `7` means nothing
        // without the second column — so `rating` is verbatim and `scale_max`
        // says what it is a rating out of. Everything downstream compares the
        // fraction of the two, which is what lets one threshold mean the same
        // thing on every server. Normalising at the door would throw away the
        // only thing that makes this answerable if the reading of a server's
        // field ever turns out to be wrong, which is the same call
        // `artist_credit`, `timestamp_original` and `spotify_track_id` all
        // make.
        //
        // A table of its own rather than columns on `loves`, because a rating
        // and a love are different statements: a rating is what the server
        // holds, and a love is what this instance decided about it under a
        // threshold the user can move. Keeping them apart is what makes the
        // threshold a knob rather than a ratchet — re-applying it can withdraw
        // the loves it made without touching one made by hand.
        //
        // `entity_key` and `match_key` are stamped at write time with the same
        // functions `entities` uses, so matching a rating to something in the
        // history is an indexed join rather than a fold per row.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS library_ratings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                source_id INTEGER NOT NULL REFERENCES user_sources(id),
                source_kind TEXT NOT NULL,
                kind TEXT NOT NULL,
                remote_id TEXT NOT NULL,
                artist TEXT NOT NULL DEFAULT '',
                name TEXT NOT NULL DEFAULT '',
                album TEXT,
                entity_key TEXT NOT NULL,
                match_key TEXT,
                mbid TEXT,
                rating REAL,
                scale_max REAL NOT NULL,
                starred INTEGER NOT NULL DEFAULT 0,
                rated_at INTEGER,
                fetched_at INTEGER NOT NULL,
                UNIQUE(user_id, source_id, kind, remote_id)
            )",
        )
        .await?;

        // Plex is a single server-wide source, not a per-user connection, so
        // this is deliberately a one-row table (the CHECK enforces it).
        self.ddl(
            "CREATE TABLE IF NOT EXISTS plex_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                enabled INTEGER NOT NULL DEFAULT 0,
                url TEXT NOT NULL DEFAULT '',
                token TEXT NOT NULL DEFAULT '',
                users_allow TEXT NOT NULL DEFAULT '',
                users_block TEXT NOT NULL DEFAULT '',
                devices_allow TEXT NOT NULL DEFAULT '',
                devices_block TEXT NOT NULL DEFAULT '',
                libraries_allow TEXT NOT NULL DEFAULT '',
                libraries_block TEXT NOT NULL DEFAULT '',
                updated_at INTEGER NOT NULL DEFAULT 0
            )",
        )
        .await?;

        self.init_mcp().await?;
        self.init_saved_playlists().await?;
        self.init_crate().await?;
        self.init_radio().await?;
        self.init_artist_pictures().await?;

        Ok(())
    }
    /// The picture each person chose for an artist.
    ///
    /// `artists.image_url` and `landscape_url` are shared by name, and until
    /// v0.124.0 the hand override wrote straight into them — so whoever last
    /// pressed *Change picture* chose the face everyone on the instance saw.
    /// The shared row now holds only what the backfill found, and a choice is
    /// a row here, read back as `COALESCE(pick.url, artists.image_url)`.
    ///
    /// `url` is what the browser loads, worked out once when the pick is made,
    /// so every reader is that one `COALESCE` rather than a rule per source.
    /// For a library picture it is this server's proxy with a version on it;
    /// the proxy reads `source_ref` and `image_key` to find the bytes.
    ///
    /// `source_ref` is a plain column rather than a foreign key, the same call
    /// `radio_air_log.source_id` makes: `delete_source` clears the picks that
    /// pointed at it, and a pick left dangling by anything else falls back to
    /// the shared picture when the proxy cannot reach it.
    async fn init_artist_pictures(&self) -> Result<(), sqlx::Error> {
        self.ddl(
            "CREATE TABLE IF NOT EXISTS artist_pictures (
                user_id INTEGER NOT NULL REFERENCES users(id),
                artist_id INTEGER NOT NULL REFERENCES artists(id),
                -- 'portrait' is artists.image_url's job, 'wide' landscape_url's.
                kind TEXT NOT NULL CHECK (kind IN ('portrait','wide')),
                -- 'upload', 'link', 'deezer', 'plex', 'jellyfin', 'emby',
                -- 'navidrome'. Shown on the tile, so the person can see where a
                -- face came from.
                source TEXT NOT NULL,
                -- The user_sources row, for a picture held by a library.
                source_ref INTEGER,
                -- What the proxy asks that library for, validated per kind of
                -- server; for every other source, the URL itself.
                image_key TEXT NOT NULL,
                -- The outside provider a library found it through
                -- (TheAudioDB, fanart.tv, a Plex agent), when it says.
                provider TEXT,
                url TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (user_id, artist_id, kind)
            )",
        )
        .await?;
        Ok(())
    }
    /// The MCP surface's own tables.
    ///
    /// **An MCP connection is deliberately not a row in `tokens`.** It is a
    /// standing grant to a third-party AI service, which is a different kind of
    /// object from "a token I pasted into fooyin", and three differences make
    /// sharing the table wrong rather than merely untidy:
    ///
    /// * `tokens.scopes` treats `all` as granting everything, so adding an
    ///   `mcp` grant there would hand every existing `all` token the power to
    ///   talk to a chatbot — a capability its owner never approved. Existing
    ///   credentials must not gain reach from an upgrade.
    /// * An OAuth-minted credential **expires and refreshes**. `tokens` has no
    ///   concept of either, and bolting one on would put expiry logic in the
    ///   path every scrobble client authenticates through.
    /// * Revoking "the chatbot I connected last month" should not mean reading
    ///   past six players in the same list.
    async fn init_mcp(&self) -> Result<(), sqlx::Error> {
        self.ddl(
            "CREATE TABLE IF NOT EXISTS mcp_connections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                -- What the user sees on the connections screen: 'Claude
                -- Desktop', 'claude.ai', 'my local agent'.
                name TEXT NOT NULL,
                -- Hashed like every other bearer secret in this file.
                token_hash TEXT NOT NULL UNIQUE,
                -- Space-separated grants from `mcp::scopes::ALL`. Never 'all':
                -- the consent screen names what it is granting, and a wildcard
                -- cannot be named.
                scopes TEXT NOT NULL DEFAULT '',
                -- Which OAuth client the grant came through, when it did.
                -- Null for a connection minted from Settings by hand.
                client_id TEXT,
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                -- OAuth access tokens expire; one created by hand does not.
                expires_at INTEGER,
                refresh_hash TEXT UNIQUE,
                revoked_at INTEGER
            )",
        )
        .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_mcp_connections_user
             ON mcp_connections(user_id)",
        )
        .await?;

        // Every call, kept. This is what makes "I connected a chatbot to my
        // listening history" reviewable rather than a matter of trust: the user
        // can see which tool ran, when, with what, and how much data left the
        // building. `rows` is the last of those and the one worth having — a
        // tool that answered with 12 rows and a tool that answered with 20,000
        // are different events however identical the call looks.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS mcp_audit (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                connection_id INTEGER REFERENCES mcp_connections(id),
                user_id INTEGER NOT NULL,
                at INTEGER NOT NULL,
                method TEXT NOT NULL,
                tool TEXT,
                args TEXT,
                ok INTEGER NOT NULL,
                rows_returned INTEGER,
                ms INTEGER
            )",
        )
        .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_mcp_audit_user_at
             ON mcp_audit(user_id, at DESC)",
        )
        .await?;

        // Registered OAuth clients. `redirect_uris` and `grant_types` are JSON
        // arrays; `secret_hash` is null for a public client, which is what
        // every MCP client is (PKCE, no back channel).
        self.ddl(
            "CREATE TABLE IF NOT EXISTS oauth_clients (
                client_id TEXT PRIMARY KEY,
                secret_hash TEXT,
                client_name TEXT NOT NULL DEFAULT '',
                redirect_uris TEXT NOT NULL DEFAULT '[]',
                grant_types TEXT NOT NULL DEFAULT '[\"authorization_code\"]',
                -- A client registered through RFC 7591 is anonymous; one
                -- identified by a Client ID Metadata Document is its own URL.
                registered_by_url INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            )",
        )
        .await?;

        // Authorization codes. Single use, short lived, and bound to both the
        // PKCE challenge and the RFC 8707 resource the client asked for — the
        // audience the minted credential is then pinned to.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS oauth_codes (
                code_hash TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                user_id INTEGER NOT NULL REFERENCES users(id),
                redirect_uri TEXT NOT NULL,
                scopes TEXT NOT NULL DEFAULT '',
                code_challenge TEXT NOT NULL,
                code_challenge_method TEXT NOT NULL DEFAULT 'S256',
                resource TEXT,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            )",
        )
        .await?;

        Ok(())
    }
    /// The Crate — things somebody told you about that you do not own yet.
    ///
    /// **Tapedeck does not recommend. It records recommendations made to you.**
    /// That distinction is the whole reason this table is allowed to exist next
    /// to a vision rule that says nothing here points at music the user does
    /// not have. The rule governs what Tapedeck *generates*; this stores what an
    /// assistant, or later a person, said — the same relationship liner notes
    /// have to the user's own writing.
    ///
    /// So the shape enforces two things. `origin` is general rather than
    /// MCP-specific, because a tip from a friend in Phase 6 is the same row.
    /// And `because` is not optional decoration: it is what in the user's own
    /// history prompted the suggestion, which is the vision doc's pipeline —
    /// analytics, then candidate selection, then explanation — held to at the
    /// schema rather than in a prompt.
    ///
    /// `first_played_at` is deliberately **not** a column. It is derived from
    /// the listens on read, for the reason chain hours and gear hours are: an
    /// accumulator that nothing recomputes drifts from the thing it summarises,
    /// and this codebase has now found that bug at three different levels.
    async fn init_crate(&self) -> Result<(), sqlx::Error> {
        self.ddl(
            "CREATE TABLE IF NOT EXISTS recommendations (
                -- the deck a dub came from. `migrate()` adds it too, for databases predating it.
                from_user_id INTEGER REFERENCES users(id),
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                kind TEXT NOT NULL CHECK (kind IN ('artist','album','track')),
                artist TEXT NOT NULL,
                -- The album or track title. Null for an artist, whose name is
                -- already in `artist` — the same rule `entities` follows.
                name TEXT,
                mbid TEXT,
                -- Why, in the recommender's own words.
                reason TEXT,
                -- What in *this user's* listening prompted it. Required by the
                -- tool: a suggestion with no grounding is free association.
                because TEXT,
                -- 'user', or the name of the connection that wrote it.
                origin TEXT NOT NULL DEFAULT 'user',
                connection_id INTEGER REFERENCES mcp_connections(id),
                status TEXT NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open','acquired','dismissed')),
                created_at INTEGER NOT NULL,
                acted_at INTEGER
            )",
        )
        .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_recommendations_user
             ON recommendations(user_id, status, created_at DESC)",
        )
        .await?;

        // One row per thing, per user. A second assistant suggesting the same
        // record should not produce a duplicate card — the interesting fact is
        // that two of them did, which the UI can show from the audit trail
        // rather than by stacking rows.
        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_recommendations_target
             ON recommendations(user_id, kind, LOWER(TRIM(artist)), LOWER(TRIM(COALESCE(name,''))))",
        )
        .await?;

        Ok(())
    }
    /// The Radio page's own tables: the listener's presets, each time they
    /// tuned in, and the air log of what was heard.
    ///
    /// **A listen is still a row in `scrobbles`** — the air log is not a second
    /// history. It is the record of what the station *said*, including what
    /// was never a listen (a news break, a promo line, a track caught for ten
    /// seconds) and what waits for a yes. `source_id` is how a logged line
    /// points at the listen it became; a plain column rather than a foreign
    /// key, so deleting a listen never needs to know the air log exists.
    ///
    /// `chain_id` is plain for the same reason: deleting a chain must not fail
    /// because somebody once tuned in through it.
    async fn init_radio(&self) -> Result<(), sqlx::Error> {
        self.ddl(
            "CREATE TABLE IF NOT EXISTS radio_presets (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                -- `<gcc>.<pi>` from the station directory, e.g. `de0.d3c3`.
                station_id TEXT NOT NULL,
                name TEXT NOT NULL,
                short_name TEXT,
                band TEXT NOT NULL CHECK (band IN ('fm','dab')),
                -- Where it sits on the dial: MHz for FM (`95.8`), the channel
                -- block for DAB (`5C`). The listener's to type — SI.xml carries
                -- neither — and optional: a preset without one still tunes.
                dial TEXT,
                city TEXT,
                position INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .await?;
        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_radio_presets_station
             ON radio_presets(user_id, station_id, band)",
        )
        .await?;

        self.ddl(
            "CREATE TABLE IF NOT EXISTS radio_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                station_id TEXT NOT NULL,
                station_name TEXT NOT NULL,
                band TEXT NOT NULL,
                dial TEXT,
                city TEXT,
                chain_id INTEGER,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                -- 'off', 'auto_stop', 'retuned', 'restart'.
                end_reason TEXT
            )",
        )
        .await?;
        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_radio_sessions_user
             ON radio_sessions(user_id, started_at DESC)",
        )
        .await?;

        self.ddl(
            "CREATE TABLE IF NOT EXISTS radio_air_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                session_id INTEGER NOT NULL REFERENCES radio_sessions(id),
                station_id TEXT NOT NULL,
                station_name TEXT NOT NULL,
                band TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                ended_at INTEGER,
                -- Exactly what the station sent.
                raw TEXT NOT NULL,
                artist TEXT,
                title TEXT,
                -- 'track': named, and MusicBrainz knows it. 'guess': named,
                -- and nothing confirmed it. 'break': news, traffic, weather,
                -- ads. 'other': anything else the station said.
                kind TEXT NOT NULL CHECK (kind IN ('track','guess','break','other')),
                -- Where the line came from: 'ICY' today; 'RT+', 'DL+' and
                -- 'RadioText' once an off-air feeder exists.
                source TEXT NOT NULL,
                -- 'playing', 'scrobbled', 'review', 'kept', 'dropped', 'held',
                -- 'partial', 'skipped', 'logged'. Free text, the UI owns it.
                status TEXT NOT NULL,
                -- The listen this became: `scrobbles.source_id`.
                source_id TEXT,
                mbid_recording TEXT
            )",
        )
        .await?;
        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_radio_air_log_user
             ON radio_air_log(user_id, started_at DESC)",
        )
        .await?;
        Ok(())
    }
    /// Playlists that live in Tapedeck rather than on a media server.
    ///
    /// Tapedeck has never had a playlist object — every list it builds
    /// (Rediscovery, the genre map, Playlist Lab) is computed on the way to
    /// being pushed somewhere else. A list an LLM hands back is the first one
    /// that has to survive the request that made it: nobody has looked at it
    /// yet, and the whole point of storing it is that somebody does before it
    /// reaches the household's Plex.
    async fn init_saved_playlists(&self) -> Result<(), sqlx::Error> {
        self.ddl(
            "CREATE TABLE IF NOT EXISTS playlists (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL REFERENCES users(id),
                name TEXT NOT NULL,
                description TEXT,
                -- Who made it: 'user', or the MCP connection's name. A list a
                -- chatbot wrote must stay distinguishable from one you built,
                -- for the same reason a note does.
                origin TEXT NOT NULL DEFAULT 'user',
                connection_id INTEGER REFERENCES mcp_connections(id),
                -- Caller-supplied, so a retrying agent updates its playlist
                -- instead of leaving five copies.
                idempotency_key TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                pushed_at INTEGER,
                pushed_to TEXT
            )",
        )
        .await?;

        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_playlists_idempotency
             ON playlists(user_id, idempotency_key)
             WHERE idempotency_key IS NOT NULL",
        )
        .await?;

        // Names, not ids. A generated list is titles and artists — the same
        // shape `playlists::pick` resolves against a real library at push time,
        // and the only shape that survives the user not owning the track on
        // every server they have.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS playlist_tracks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                playlist_id INTEGER NOT NULL REFERENCES playlists(id),
                position INTEGER NOT NULL,
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT,
                -- Why this track is here, in the words of whatever chose it.
                -- Optional, and never generated by Tapedeck itself.
                note TEXT
            )",
        )
        .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_playlist_tracks_playlist
             ON playlist_tracks(playlist_id, position)",
        )
        .await?;

        // Which remote playlist a push owns.
        //
        // Keyed on the **name**, not on a local playlist id, because most of
        // what gets pushed has no local id at all: a Rediscovery collection is
        // computed on demand, and the name is the only stable thing about it —
        // and it is also what identifies the playlist on the far end, since
        // that is what the playlist is called there. Re-pushing "Dusty Vinyl"
        // therefore rewrites the same remote playlist, which is what anyone
        // pressing the button twice expects.
        //
        // `source_id` is part of the key so the same list pushed to Plex and to
        // TIDAL remembers both, and `-1` is the reserved TIDAL target.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS pushed_playlists (
                user_id INTEGER NOT NULL REFERENCES users(id),
                source_id INTEGER NOT NULL,
                name_key TEXT NOT NULL,
                remote_id TEXT NOT NULL,
                pushed_at INTEGER NOT NULL,
                PRIMARY KEY (user_id, source_id, name_key)
            )",
        )
        .await?;

        // Which track on the far end a name resolved to, last time.
        //
        // A push is one search per track and the far end meters them, so the
        // cheapest search is the one not made: on a re-push of a 30-track list
        // where 24 matched, only the 6 that missed need asking about again.
        // That alone would have prevented the incident this exists for — three
        // pushes in quick succession, the second and third throttled into
        // reporting real tracks as absent.
        //
        // Keyed on the same fold `playlists::pick` matches with, so a hit here
        // and a match there cannot disagree about whether two spellings are the
        // same track.
        //
        // **Misses are deliberately not cached.** A catalogue gains tracks, and
        // remembering "not on TIDAL" would make that permanent for a listener
        // who would never find out why. The win is on hits and that is enough.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS playlist_matches (
                user_id INTEGER NOT NULL REFERENCES users(id),
                source_id INTEGER NOT NULL,
                artist_key TEXT NOT NULL,
                title_key TEXT NOT NULL,
                remote_id TEXT NOT NULL,
                -- What the far end calls it, so a reconciliation screen can
                -- show what was actually chosen rather than just an opaque id.
                remote_label TEXT,
                -- 'auto' — this is what the search found and the matcher
                -- accepted. 'manual' — a person picked it from the candidates
                -- offered. The two expire differently; see `playlist_match`.
                origin TEXT NOT NULL DEFAULT 'auto',
                resolved_at INTEGER NOT NULL,
                PRIMARY KEY (user_id, source_id, artist_key, title_key)
            )",
        )
        .await?;

        self.init_patch().await?;

        Ok(())
    }
    /// Phase 6 — Patch, and the actor side of Phase 8.
    ///
    /// Split into its own function because it is a coherent subsystem rather
    /// than more of the same, and because it is the only schema in this file
    /// whose rows **another server will hold copies of**. That is the whole
    /// reason the stable ids here are not an afterthought: a listen can be
    /// re-derived, and an id another instance already stored cannot.
    ///
    /// Local-first is the shipping order. Everything here serves from day one;
    /// outbound delivery stays dark until Patch works inside one instance.
    async fn init_patch(&self) -> Result<(), sqlx::Error> {
        // One row per local user, holding the half of an identity that faces
        // outward. Deliberately *not* columns on `users`: the private key is a
        // credential, and `users` is the row every authentication query in this
        // application selects.
        //
        // `uri` is stored rather than derived on read. It is derived from
        // `PUBLIC_URL` and the username at creation, but the moment a remote
        // server holds it, it is a fact about the past — recomputing it from
        // today's config would silently rename an actor other people follow.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS actors (
                user_id INTEGER PRIMARY KEY REFERENCES users(id),
                uri TEXT NOT NULL UNIQUE,
                -- PEM. The private half goes through `Database::seal` like every
                -- other replayed secret, so ciphertext never leaves db.rs.
                public_key TEXT NOT NULL,
                private_key TEXT NOT NULL,
                -- When this identity first left the building: the first
                -- accepted patch, or the first signed activity, whichever came
                -- first. Non-null means the username is frozen.
                --
                -- The obvious rule -- 'renameable while the deck is private' --
                -- is wrong here, because following happens *while* private: a
                -- private deck holds patches in the inbox for approval, so a
                -- user can have followers and never be public. Renaming after
                -- this point breaks the identity on every server holding it.
                -- ActivityPub's way out is `Move` + `alsoKnownAs`; that is a
                -- deliberate migration and it is not built.
                frozen_at INTEGER,
                created_at INTEGER NOT NULL
            )",
        )
        .await?;

        // Everyone we have resolved who is not us. Cached rather than fetched
        // per request -- an actor document is the key material a signature is
        // checked against, and re-fetching it per inbox delivery would make
        // every remote server's availability our own.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS remote_actors (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uri TEXT NOT NULL UNIQUE,
                -- 'ash@deck.example', as WebFinger resolves it. Display only:
                -- `uri` is the identity, and a handle can be reused after a
                -- `Move` where a URI cannot.
                handle TEXT,
                name TEXT,
                inbox TEXT NOT NULL,
                -- A shared inbox is one POST for every follower on that host.
                -- Optional, and the only reason delivery to a large instance is
                -- not one request per person.
                shared_inbox TEXT,
                public_key TEXT,
                icon_url TEXT,
                fetched_at INTEGER NOT NULL
            )",
        )
        .await?;

        // A patch is a follow edge, and it carries state because an
        // ActivityPub follow is a request that needs an explicit Accept -- one
        // of the four things recorded as must-not-defer since 2026-07-30.
        //
        // Either end may be local or remote, which is why there are four
        // columns and not two. The CHECKs are what stop a row that names both
        // or neither: exactly one side of each end is set, always.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS patches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid TEXT NOT NULL UNIQUE,
                follower_user_id INTEGER REFERENCES users(id),
                follower_actor_id INTEGER REFERENCES remote_actors(id),
                target_user_id INTEGER REFERENCES users(id),
                target_actor_id INTEGER REFERENCES remote_actors(id),
                -- pending | accepted | rejected. Never a boolean: a rejected
                -- patch and a patch nobody has looked at are different things,
                -- and only one of them should be offered again.
                state TEXT NOT NULL DEFAULT 'pending',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                CHECK ((follower_user_id IS NULL) <> (follower_actor_id IS NULL)),
                CHECK ((target_user_id IS NULL) <> (target_actor_id IS NULL))
            )",
        )
        .await?;

        // One edge per pair, whichever shape the ends take. Two partial indexes
        // rather than one, because a UNIQUE over four mostly-null columns
        // treats every NULL as distinct and would enforce nothing at all.
        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_patches_local_pair
             ON patches(follower_user_id, target_user_id)
             WHERE follower_user_id IS NOT NULL AND target_user_id IS NOT NULL",
        )
        .await?;
        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_patches_remote_out
             ON patches(follower_user_id, target_actor_id)
             WHERE follower_user_id IS NOT NULL AND target_actor_id IS NOT NULL",
        )
        .await?;
        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_patches_remote_in
             ON patches(follower_actor_id, target_user_id)
             WHERE follower_actor_id IS NOT NULL AND target_user_id IS NOT NULL",
        )
        .await?;

        // Every social action, with a stable id and a timestamp.
        //
        // The verb is settled and frozen: a listen is a `Create` wrapping a
        // `td:Listen` object, so a server that understands nothing renders the
        // content line and one that understands `td:` gets the chain and the
        // format. There is no picker -- three vocabularies would mean three
        // things to maintain and a migration path between them for a decision
        // made once.
        //
        // `payload` is the JSON-LD **as signed**, kept rather than rebuilt.
        // Rebuilding it later would produce a different document once anything
        // upstream of it changes, and a signature is over bytes.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS activities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid TEXT NOT NULL UNIQUE,
                user_id INTEGER NOT NULL REFERENCES users(id),
                -- Create | Follow | Accept | Reject | Undo | Delete | Like
                kind TEXT NOT NULL,
                -- 'td:Listen', 'Note', 'td:Dub'. Null for the bare verbs.
                object_type TEXT,
                -- The local row this is about, as 'scrobble:41822'. Kept so a
                -- delete can find the activity that announced the thing being
                -- deleted -- and note the ordering that forces: the Delete is
                -- emitted while the row still exists, because it carries its id.
                object_ref TEXT,
                payload TEXT NOT NULL,
                published INTEGER NOT NULL,
                -- public | patched | private. The enum from the first
                -- migration, never a boolean.
                visibility TEXT NOT NULL DEFAULT 'private'
            )",
        )
        .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_activities_user_published
             ON activities(user_id, published)",
        )
        .await?;
        self.ddl("CREATE INDEX IF NOT EXISTS idx_activities_object ON activities(object_ref)")
            .await?;

        // Delivery is per recipient, for the reason `delivered_sinks` is per
        // scrobble: one inbox being down must not redeliver to the twenty that
        // already took it. Retries only touch rows that are not yet 'sent'.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS activity_deliveries (
                activity_id INTEGER NOT NULL REFERENCES activities(id),
                actor_id INTEGER NOT NULL REFERENCES remote_actors(id),
                -- pending | sent | failed
                state TEXT NOT NULL DEFAULT 'pending',
                attempts INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                delivered_at INTEGER,
                PRIMARY KEY (activity_id, actor_id)
            )",
        )
        .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_activity_deliveries_pending
             ON activity_deliveries(state, attempts) WHERE state <> 'sent'",
        )
        .await?;

        // What other servers have said to us, kept apart from what we have
        // said.
        //
        // The obvious economy is to reuse `activities` with a direction flag,
        // and it is wrong: `activities.user_id` is the *author*, and the outbox
        // serves that table filtered only by visibility. An inbound activity
        // filed there would be one `WHERE` clause away from this server
        // publishing a stranger's activity as its user's own -- and it would
        // look completely reasonable in review. Two tables make that
        // impossible rather than merely unlikely, which is the same call the
        // Crate's separate table makes about unowned music.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS inbox_activities (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                -- Whose inbox it arrived at.
                user_id INTEGER NOT NULL REFERENCES users(id),
                -- Who sent it, verified -- not who it claimed to be from. This
                -- row exists only after a signature checked out.
                actor_id INTEGER NOT NULL REFERENCES remote_actors(id),
                -- The activity's own id on the sending server. UNIQUE, because
                -- senders retry: a redelivery is the same activity, and acting
                -- on one Follow twice is how one follower becomes two.
                activity_uri TEXT NOT NULL UNIQUE,
                kind TEXT,
                object_ref TEXT,
                -- Kept as received. A signature is over bytes, and rebuilding
                -- this later would produce a different document.
                payload TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                -- pending | handled | ignored. Nothing acts on these yet; Step
                -- 2 is what turns a pending Follow into a `patches` row.
                state TEXT NOT NULL DEFAULT 'pending'
            )",
        )
        .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_inbox_activities_user
             ON inbox_activities(user_id, received_at)",
        )
        .await?;

        // ── Shared Spools ──
        //
        // A sitting several people are in at once. The decision from
        // 2026-08-02 stands and decides the shape: a joint listen is **N rows
        // sharing a `joint_session_id`, one per listener** — not one row with N
        // owners. Every query in this codebase scopes by `user_id` and
        // `delivered_sinks` is per scrobble; one row with several owners would
        // mean revisiting all of it. The consequence to keep in mind is that a
        // joint listen really is duplicated storage, and each copy forwards to
        // its own owner's services — which is correct, because each listener
        // really did listen.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS spools (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid TEXT NOT NULL UNIQUE,
                -- Whose deck is playing. Captures flow outward from here.
                host_user_id INTEGER NOT NULL REFERENCES users(id),
                -- Set at the start, when you know least about the evening, and
                -- editable afterwards — you name a sitting for what it turned
                -- out to be.
                title TEXT,
                -- What the evening was. A sitting is the one thing in this
                -- application that has an ending, and an ending is when
                -- somebody writes about it.
                --
                -- Not a `notes` row: those hang off an entity, a listen, a
                -- period or a line, and every one of them is about *music*.
                -- This is about an occasion, and the sitting is already the
                -- thing that owns it.
                notes TEXT,
                -- Everybody else who was in the room, as names — the friend on
                -- the sofa with no account on this instance.
                --
                -- Free text for the reason `scrobbles.company` is free text: a
                -- feature that could only record the people who have accounts
                -- would miss the common case. They get no captures and no
                -- inbox, because there is nobody to offer anything to; what
                -- they get is being named in the company of every play the
                -- sitting fans out.
                guests TEXT,
                started_at INTEGER NOT NULL,
                -- Null while it is running. One running spool per host, which
                -- the partial index below enforces rather than the handler.
                ended_at INTEGER,
                -- Set only for a sitting reconstructed after the fact — the
                -- spool you were both in and neither of you started. Null for
                -- one opened live, which is what tells the two apart.
                session_uuid TEXT
            )",
        )
        .await?;

        // Nobody hosts two sittings at once, and saying so here means no
        // handler has to remember it.
        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_spools_one_running
             ON spools(host_user_id) WHERE ended_at IS NULL",
        )
        .await?;

        self.ddl(
            "CREATE TABLE IF NOT EXISTS spool_members (
                spool_id INTEGER NOT NULL REFERENCES spools(id),
                user_id INTEGER NOT NULL REFERENCES users(id),
                -- invited | joined | left. An invitation is not attendance:
                -- captures are only made for members who actually joined.
                state TEXT NOT NULL DEFAULT 'invited',
                joined_at INTEGER,
                -- When they were **in the room** from, which is not when they
                -- pressed the button. NULL means from the start of the sitting.
                --
                -- These are two different facts and conflating them is what
                -- makes late joining wrong in one direction or the other.
                -- Somebody added an hour in is usually somebody who was there
                -- all along and the host forgot — that is NULL, and they get
                -- the whole evening. Somebody who genuinely walked in at ten
                -- did not hear the eight o'clock records, and saying so is what
                -- this column is for. The host says which when adding and the
                -- joiner can correct it.
                present_from INTEGER,
                -- When they left, or NULL for stayed to the end. The mirror of
                -- the above, and the reason `state = 'left'` is not enough on
                -- its own: leaving is a fact with a time on it.
                present_until INTEGER,
                PRIMARY KEY (spool_id, user_id)
            )",
        )
        .await?;

        // What somebody else's deck played while you were in the room.
        //
        // **Nothing here is a listen.** A capture is an offer, and it enters no
        // history until its owner accepts it — the rule the design adds to the
        // 2026-08-02 decision, and the reason this is a table of its own rather
        // than rows in `scrobbles` with a flag. A flag would put unaccepted
        // plays one forgotten `WHERE` away from every count in the
        // application; a separate table makes that impossible, the same call
        // the Crate and `inbox_activities` both make.
        self.ddl(
            "CREATE TABLE IF NOT EXISTS spool_captures (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                spool_id INTEGER NOT NULL REFERENCES spools(id),
                -- Whose inbox it is waiting in.
                user_id INTEGER NOT NULL REFERENCES users(id),
                -- Whose deck actually played it.
                from_user_id INTEGER NOT NULL REFERENCES users(id),
                title TEXT NOT NULL,
                artist TEXT NOT NULL,
                album TEXT,
                duration INTEGER,
                played_at INTEGER NOT NULL,
                -- pending | accepted | declined
                state TEXT NOT NULL DEFAULT 'pending',
                settled_at INTEGER,
                -- Who else was in the room when this played, snapshotted at
                -- fan-out. Carried on the capture rather than recomputed when
                -- it is accepted, because those can be hours apart and somebody
                -- who joined in between was not there for this play.
                company TEXT,
                -- One offer per person per play. A host whose deck re-reports a
                -- listen must not fill somebody's inbox with copies.
                UNIQUE (user_id, spool_id, played_at, title)
            )",
        )
        .await?;

        // ── Handing over a past sitting ──
        //
        // **A handed-over sitting is a document, not captures.** Spool captures
        // may enter your history because you were in the room; for a past
        // sitting the recipient was not, and writing "you listened to this"
        // onto somebody's permanent record when they demonstrably did not is
        // the error `ListenDisposition` exists to prevent. So this table holds
        // a *reference* and nothing is ever copied into `scrobbles`.
        //
        // A reference rather than a snapshot, which has consequences that are
        // correct locally: regroup the sitting or un-share a chain and what the
        // recipient sees changes, because the document is rendered from the
        // listens on every read. That diverges from "no delete sweep on
        // un-patching" by physics rather than preference — that rule is about
        // bytes already on somebody else's server, which cannot be recalled.
        // **When this federates it has to become a snapshot.**
        self.ddl(
            "CREATE TABLE IF NOT EXISTS shared_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uuid TEXT NOT NULL UNIQUE,
                -- `sessions.uuid`, which is stable across merges and splits
                -- deliberately — this is what that stability was built for.
                session_uuid TEXT NOT NULL,
                from_user_id INTEGER NOT NULL REFERENCES users(id),
                -- Exactly one recipient, and the remote column exists now so
                -- that federating this needs no migration. The same shape
                -- `patches` uses, and the discipline that made Phase 6 cheap.
                to_user_id INTEGER REFERENCES users(id),
                to_actor_id INTEGER REFERENCES remote_actors(id),
                note TEXT,
                created_at INTEGER NOT NULL,
                seen_at INTEGER,
                CHECK ((to_user_id IS NULL) <> (to_actor_id IS NULL))
            )",
        )
        .await?;

        // Handing the same sitting to the same person twice updates the note
        // rather than filling their inbox with copies.
        self.ddl(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_shared_sessions_local
             ON shared_sessions(from_user_id, session_uuid, to_user_id)
             WHERE to_user_id IS NOT NULL",
        )
        .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_shared_sessions_inbox
             ON shared_sessions(to_user_id, created_at)",
        )
        .await?;

        // Every live listen now asks "am I in a sitting?", which walks
        // `spool_members` by user. The primary key is (spool_id, user_id), so
        // it cannot serve that prefix. A point lookup is exactly the workload
        // v0.59.0 says an index is for, unlike the whole-history aggregations
        // its expression indexes were dropped from.
        self.ddl("CREATE INDEX IF NOT EXISTS idx_spool_members_user ON spool_members(user_id)")
            .await?;

        self.ddl(
            "CREATE INDEX IF NOT EXISTS idx_spool_captures_pending
             ON spool_captures(user_id, state, played_at)",
        )
        .await?;

        Ok(())
    }
}
