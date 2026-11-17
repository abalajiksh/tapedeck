//! Live checks against a real Postgres server.
//!
//! Opt-in and `#[ignore]`d, the way `tidal_live_search` is: they need a server
//! nobody else has, and a suite that silently skips them is worse than one that
//! says why it did not run.
//!
//! ```text
//! TAPEDECK_TEST_POSTGRES_URL=postgres://user:pass@host/db \
//!   cargo test --features postgres --bin tapedeck -- --ignored pg_live
//! ```
//!
//! Each run drops and recreates a **schema** of its own and works only inside
//! it, so a run starts from a genuinely fresh shape and `public` is never
//! touched — the account needs `CREATE` on the database and nothing more, no
//! `CREATEDB` and no superuser. That is also the isolation a CI job would want,
//! since a schema is cheap where a database needs privileges somebody has to
//! grant.
//!
//! This is the only thing that can answer whether the DDL translation works.
//! Everything else about it is a claim from reading a specification.

#[cfg(test)]
mod tests {
    use crate::crypto::SecretBox;
    use crate::db::Database;
    use sqlx::{postgres::PgPoolOptions, Row};
    use std::sync::Arc;

    /// Schemas are named per test — `cargo test` runs them in parallel, and two
    /// tests dropping and recreating one schema race each other.
    const SCHEMA_PREFIX: &str = "tapedeck_pg_check";

    fn base_url() -> String {
        std::env::var("TAPEDECK_TEST_POSTGRES_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .expect("set TAPEDECK_TEST_POSTGRES_URL, e.g. postgres://user:pass@host/db")
    }

    /// Kept for the probe below, which asks what the account may do.
    fn admin_url() -> String {
        base_url()
    }

    /// The same database, with `search_path` pinned to the scratch schema — so
    /// every `CREATE TABLE` lands there and nothing reaches `public`.
    fn scoped_url(base: &str, schema: &str) -> String {
        let sep = if base.contains('?') { '&' } else { '?' };
        format!("{base}{sep}options=-c%20search_path%3D{schema}")
    }

    /// Drop and recreate the scratch schema, returning a URL scoped to it.
    async fn fresh(name: &str) -> String {
        let schema = format!("{SCHEMA_PREFIX}_{name}");
        let base = base_url();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&base)
            .await
            .unwrap_or_else(|e| panic!("connect: {}", crate::logging::error_chain(&e)));
        for sql in
            [format!("DROP SCHEMA IF EXISTS {schema} CASCADE"), format!("CREATE SCHEMA {schema}")]
        {
            sqlx::query(&sql)
                .execute(&pool)
                .await
                .unwrap_or_else(|e| panic!("{sql}: {}", crate::logging::error_chain(&e)));
        }
        pool.close().await;
        scoped_url(&base, &schema)
    }

    /// The schema existing is not the application working. This is the first
    /// thing that actually reads and writes.
    ///
    /// **Expected to fail today, and it is kept because of what it fails on.**
    /// `dialect::rewrite` — the placeholder rewriter — has no call sites: every
    /// one of the ~848 query sites still hands `sqlx::query` raw SQL with `?`
    /// binds, which Postgres answers with `syntax error at or near ","`. Wiring
    /// it is the next piece, and this is the marker for where the work stops.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn a_listen_survives_a_round_trip_through_postgres() {
        let url = fresh("roundtrip").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(|e| panic!("Database::new: {}", crate::logging::error_chain(&e)));

        let uid = db
            .setup_admin("tester", None, "not-a-real-hash")
            .await
            .unwrap_or_else(|e| panic!("setup_admin: {}", crate::logging::error_chain(&e)));

        let play = crate::models::Play::new(
            "Xtal".into(),
            "Aphex Twin".into(),
            1_700_000_000,
            "pg-1".into(),
            "test".into(),
        );
        db.save_scrobble(uid, &play, None, None, None, None, None, None, None, Default::default())
            .await
            .unwrap_or_else(|e| panic!("save_scrobble: {}", crate::logging::error_chain(&e)));

        // The same listen twice must not become two.
        let _ = db
            .save_scrobble(uid, &play, None, None, None, None, None, None, None, Default::default())
            .await;

        // Read it back through the pool rather than through a listing helper,
        // so this test stays about the round trip and not about one signature.
        let (n, title): (i64, String) =
            sqlx::query_as("SELECT COUNT(*), MAX(title) FROM scrobbles WHERE user_id = $1")
                .bind(uid)
                .fetch_one(&db.pool)
                .await
                .unwrap_or_else(|e| panic!("read back: {}", crate::logging::error_chain(&e)));
        assert_eq!(n, 1, "the same listen twice became {n} rows — dedup is not holding");
        assert_eq!(title, "Xtal");

        let stats = db
            .get_dashboard_stats(uid, chrono_tz::Tz::UTC, 1)
            .await
            .unwrap_or_else(|e| panic!("get_dashboard_stats: {}", crate::logging::error_chain(&e)));
        println!("dashboard: {}", serde_json::to_value(&stats).unwrap());
    }

    fn die<T>(what: &str) -> impl FnOnce(sqlx::Error) -> T + '_ {
        move |e| panic!("{what}: {}", crate::logging::error_chain(&e))
    }

    /// A row written straight to its table, returning its id — for fixtures
    /// whose own writer would drag in more than the check is about.
    /// (`create_chain` / `create_variant` work on Postgres since v0.132.0,
    /// when the JSON columns became TEXT; `duration_sums_decode_on_postgres`
    /// goes through them.)
    async fn insert_returning_id(db: &Database, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&db.pool).await.unwrap_or_else(die(sql))
    }

    async fn insert_chain(db: &Database, user_id: i64, name: &str) -> i64 {
        insert_returning_id(
            db,
            &format!(
                "INSERT INTO signal_chains (user_id, name, created_at)
                 VALUES ({user_id}, '{name}', 0) RETURNING id"
            ),
        )
        .await
    }

    /// What `main` does before it serves anything, against a database nobody
    /// has opened before.
    ///
    /// v0.126.7 and v0.128.0 both died here: `migrate_global_plex` seeds the
    /// legacy Plex row with a Rust `bool` bound into `plex_settings.enabled`,
    /// which is `INTEGER` in the schema and therefore `BIGINT` on Postgres, and
    /// Postgres refuses the bind outright. Reading it back as a `bool` failed
    /// the same way. No suite noticed because none of them start the binary.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn the_startup_path_runs_on_a_fresh_postgres_database() {
        let url = fresh("startup").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new"));

        // A config with Plex switched off: the shape every install without the
        // legacy `[sources.plex]` block has.
        let plex = crate::config::PlexConfig {
            enabled: false,
            url: String::new(),
            token: String::new(),
            users_allow: vec![],
            users_block: vec![],
            devices_allow: vec![],
            devices_block: vec![],
            libraries_allow: vec![],
            libraries_block: vec![],
        };
        crate::migrate_global_plex(&db, &plex)
            .await
            .unwrap_or_else(|e| panic!("migrate_global_plex: {e}"));
        // And a second start, which reads the row the first one wrote.
        crate::migrate_global_plex(&db, &plex)
            .await
            .unwrap_or_else(|e| panic!("migrate_global_plex, second start: {e}"));

        let seeded = db.get_plex_settings().await.unwrap_or_else(die("get_plex_settings"));
        let seeded = seeded.expect("the first start seeds the legacy row");
        assert!(!seeded.enabled);

        // Both values of the flag, not only the one a fresh install writes.
        let on = crate::db::PlexSettings {
            enabled: true,
            url: "http://plex.test:32400".into(),
            ..seeded
        };
        db.save_plex_settings(&on).await.unwrap_or_else(die("save_plex_settings"));
        let back = db.get_plex_settings().await.unwrap_or_else(die("get_plex_settings")).unwrap();
        assert!(back.enabled, "saved on, read back off");
        assert_eq!(back.url, "http://plex.test:32400");
    }

    /// Every on/off flag the schema keeps as `INTEGER`, written and read back
    /// through the functions that own it.
    ///
    /// Each one is the bug the startup test found, in a place startup does not
    /// reach: SQLite takes a `bool` into an `INTEGER` column and hands one back
    /// without a murmur, Postgres does neither. Plus `signal_chains.shared`,
    /// the reverse case — a real `BOOLEAN` compared with `= 1`, which Postgres
    /// has no operator for.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn every_on_off_flag_round_trips_through_postgres() {
        use crate::db::IncomingRating;

        let url = fresh("flags").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new"));
        let me = db
            .setup_admin("tester", None, "not-a-real-hash")
            .await
            .unwrap_or_else(die("setup_admin"));
        let bob = db
            .create_user("bob", None, "not-a-real-hash", "user")
            .await
            .unwrap_or_else(die("create_user"));

        // users.disabled
        db.set_user_disabled(bob, true).await.unwrap_or_else(die("set_user_disabled"));
        let (role, disabled) = db
            .get_user_role_disabled(bob)
            .await
            .unwrap_or_else(die("get_user_role_disabled"))
            .unwrap();
        assert_eq!((role.as_str(), disabled), ("user", true));
        let listed = db.list_users().await.unwrap_or_else(die("list_users"));
        assert!(listed.iter().any(|u| u.id == bob && u.disabled));
        assert!(listed.iter().any(|u| u.id == me && !u.disabled));
        let profile = db.get_profile(bob).await.unwrap_or_else(die("get_profile")).unwrap();
        assert!(profile.disabled);
        db.set_user_disabled(bob, false).await.unwrap_or_else(die("set_user_disabled"));
        assert!(!db.get_user_role_disabled(bob).await.unwrap().unwrap().1);

        // users.disabled through an MCP token, and mcp_audit.ok
        let cred = db
            .create_mcp_connection(me, "pg check", "read", None, None)
            .await
            .unwrap_or_else(die("create_mcp_connection"));
        assert!(
            db.validate_mcp_token(&cred.token)
                .await
                .unwrap_or_else(die("validate_mcp_token"))
                .is_some(),
            "a live token on an enabled account validates"
        );
        db.log_mcp_call(
            Some(cred.id),
            me,
            "tools/call",
            Some("now_playing"),
            None,
            true,
            Some(1),
            3,
        )
        .await;
        db.log_mcp_call(Some(cred.id), me, "tools/call", Some("lyrics"), None, false, None, 4)
            .await;
        let audit = db.list_mcp_audit(me, 10).await.unwrap_or_else(die("list_mcp_audit"));
        assert_eq!(audit.len(), 2, "both audit writes landed");
        assert_eq!(
            audit.iter().map(|a| (a.tool.as_deref(), a.ok)).collect::<Vec<_>>(),
            vec![(Some("lyrics"), false), (Some("now_playing"), true)]
        );

        // oauth_clients.registered_by_url
        db.register_oauth_client("https://client.test/meta.json", "Client", "[]", true)
            .await
            .unwrap_or_else(die("register_oauth_client"));
        let client = db
            .get_oauth_client("https://client.test/meta.json")
            .await
            .unwrap_or_else(die("get_oauth_client"))
            .unwrap();
        assert!(client.registered_by_url);

        // user_sources.enabled, every read and both writes
        let sid = db
            .create_source(me, "navidrome", "Nav", true, "http://nav.test", "", None, None, "{}")
            .await
            .unwrap_or_else(die("create_source"));
        let mine = db.list_sources_for_user(me).await.unwrap_or_else(die("list_sources_for_user"));
        assert!(mine.iter().any(|s| s.id == sid && s.enabled));
        let polled = db.list_enabled_sources().await.unwrap_or_else(die("list_enabled_sources"));
        assert!(polled.iter().any(|s| s.id == sid));
        let health = db.source_health().await.unwrap_or_else(die("source_health"));
        assert!(health.iter().any(|h| h.id == sid && h.enabled && !h.plugin));
        db.update_source(me, sid, "Nav", false, "http://nav.test", "", None, None, "{}")
            .await
            .unwrap_or_else(die("update_source"));
        assert!(!db.get_source(me, sid).await.unwrap_or_else(die("get_source")).unwrap().enabled);

        // library_ratings.starred
        let heart = IncomingRating {
            kind: "recording".into(),
            remote_id: "nd-1".into(),
            artist: "Massive Attack".into(),
            name: "Teardrop".into(),
            album: None,
            mbid: None,
            rating: None,
            starred: true,
            rated_at: None,
        };
        db.upsert_library_ratings(me, sid, "navidrome", &[heart])
            .await
            .unwrap_or_else(die("upsert_library_ratings"));
        let held =
            db.list_library_ratings(me, 10).await.unwrap_or_else(die("list_library_ratings"));
        assert!(held.len() == 1 && held[0].starred);
        let sources = db.rating_sources(me).await.unwrap_or_else(die("rating_sources"));
        assert_eq!(sources.iter().map(|s| s.starred).sum::<i64>(), 1);
        // `apply_rating_loves` reads `history_keys`, which puts bare columns
        // beside an aggregate — a gap `dialect.rs` already lists, not this
        // one. Reported, not asserted.
        let applied =
            db.apply_rating_loves(me, crate::ratings::LoveRule::new(1.0, true), true).await;
        println!(
            "apply_rating_loves: {:?}",
            applied.map(|r| r.qualifying).map_err(|e| e.to_string())
        );

        // users.share_all_with_patched, and signal_chains.shared = TRUE
        db.set_share_all_with_patched(me, true)
            .await
            .unwrap_or_else(die("set_share_all_with_patched"));
        assert!(db.share_all_with_patched(me).await.unwrap_or_else(die("share_all_with_patched")));
        let chain = insert_chain(&db, me, "Desk").await;
        db.set_chain_shared(me, chain, true).await.unwrap_or_else(die("set_chain_shared"));
        db.live_visibility(me, Some(chain)).await.unwrap_or_else(die("live_visibility"));
        // `instance_pulse` also counts `shared = TRUE`, but stops first on
        // `GROUP_CONCAT`, another gap `dialect.rs` lists. Reported only.
        let pulse = db.instance_pulse(0).await;
        println!("instance_pulse: {:?}", pulse.map(|_| ()).map_err(|e| e.to_string()));
        db.request_patch(bob, me).await.unwrap_or_else(die("request_patch"));
        // A new account starts open (`create_user` writes 1), so close it
        // first to see both values come back.
        db.set_share_all_with_patched(bob, false).await.unwrap();
        let inbox = db.patches_in(me).await.unwrap_or_else(die("patches_in"));
        assert_eq!(inbox.len(), 1);
        // `PATCH_PEER_COLUMNS`: the far end's flag decoded, and its shared
        // chains counted through `c.shared = TRUE`.
        assert!(!inbox[0].shares_everything);
        assert_eq!(inbox[0].shared_chains, 0);
        db.set_share_all_with_patched(bob, true).await.unwrap();
        let bobs = insert_chain(&db, bob, "Bob desk").await;
        db.set_chain_shared(bob, bobs, true).await.unwrap();
        let inbox = db.patches_in(me).await.unwrap_or_else(die("patches_in"));
        assert!(inbox[0].shares_everything);
        assert_eq!(inbox[0].shared_chains, 1);
        let peers = db.patches_out(bob).await;
        // `patches_out` still orders by `COLLATE NOCASE`, which Postgres does
        // not have. Recorded rather than fixed here; see the PR.
        println!("patches_out: {:?}", peers.as_ref().map(|p| p.len()).map_err(|e| e.to_string()));
    }

    /// A database v0.126.7 created gains what v0.127.0 added to it.
    ///
    /// The table is rebuilt by hand in the exact shape v0.126.7's `init()`
    /// gave it on Postgres (its DDL through `rewrite_ddl`), with a row in it,
    /// then opened through the normal path. The result is compared against a
    /// fresh database's table rather than against a list written here, so a
    /// migration that adds the column with the wrong type or no foreign key
    /// fails too.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn a_v0_126_postgres_database_gains_chain_bindings_chain_variant_id() {
        let url = fresh("upgrade").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new"));
        let me = db.setup_admin("tester", None, "not-a-real-hash").await.unwrap();
        let chain = insert_chain(&db, me, "Desk").await;
        drop(db);

        let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
        for sql in [
            "DROP TABLE chain_bindings".to_string(),
            // v0.126.7's schema.rs, as `rewrite_ddl` spelled it for Postgres.
            "CREATE TABLE chain_bindings (
                id BIGSERIAL PRIMARY KEY,
                user_id BIGINT NOT NULL REFERENCES users(id),
                identifier TEXT NOT NULL,
                interface TEXT,
                chain_id BIGINT REFERENCES signal_chains(id),
                last_seen BIGINT NOT NULL,
                hits BIGINT NOT NULL DEFAULT 0,
                UNIQUE(user_id, identifier)
            )"
            .to_string(),
            format!(
                "INSERT INTO chain_bindings (user_id, identifier, interface, chain_id, last_seen, hits)
                 VALUES ({me}, 'Topping E30', 'usb', {chain}, 1700000000, 7)"
            ),
        ] {
            sqlx::query(&sql).execute(&pool).await.unwrap_or_else(die("build v0.126.7 shape"));
        }
        pool.close().await;

        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new over a v0.126.7 database"));

        // The row survived and reads through the code that wants the column.
        let bindings = db.list_bindings(me).await.unwrap_or_else(die("list_bindings"));
        assert_eq!(bindings.len(), 1);
        assert_eq!((bindings[0].chain_id, bindings[0].chain_variant_id), (Some(chain), None));
        assert_eq!(bindings[0].hits, 7);
        let variant = insert_returning_id(
            &db,
            &format!(
                "INSERT INTO chain_variants (user_id, chain_id, name, created_at)
                 VALUES ({me}, {chain}, 'Other stylus', 0) RETURNING id"
            ),
        )
        .await;
        db.set_binding(me, "Topping E30", Some(chain), Some(variant))
            .await
            .unwrap_or_else(die("set_binding"));
        assert_eq!(
            db.resolve_binding(me, "Topping E30").await.unwrap_or_else(die("resolve_binding")),
            Some((chain, Some(variant)))
        );

        // Opening it again is a no-op, not a second ALTER.
        drop(db);
        Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new, third start"));

        // Same shape as a database that never needed migrating.
        let fresh_url = fresh("upgrade_ref").await;
        drop(Database::new(&fresh_url, Arc::new(SecretBox::test_key())).await.unwrap());
        let upgraded = binding_shape(&url, "upgrade").await;
        let reference = binding_shape(&fresh_url, "upgrade_ref").await;
        assert!(
            upgraded.0.iter().any(|(c, _, _)| c == "chain_variant_id"),
            "no chain_variant_id after the upgrade: {:?}",
            upgraded.0
        );
        assert_eq!(upgraded, reference, "an upgraded chain_bindings differs from a fresh one");
    }

    /// A v0.131 Postgres database — no facet columns on `scrobbles` — opened
    /// by v0.132.0 gains them, shaped exactly as a fresh install's, with its
    /// listens intact and the classifier's work list reading them. Fresh
    /// schemas never see this, which is how the first PR for the facets
    /// shipped without a Postgres arm.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn a_v0_131_postgres_database_gains_the_facet_columns() {
        let url = fresh("facets_upgrade").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new"));
        let me = db.setup_admin("tester", None, "not-a-real-hash").await.unwrap();
        let play = crate::models::Play::new(
            "Xtal".into(),
            "Aphex Twin".into(),
            1_700_000_000,
            "pg-upgrade-1".into(),
            "test".into(),
        );
        db.save_scrobble(me, &play, None, None, None, None, None, None, None, Default::default())
            .await
            .unwrap_or_else(die("save_scrobble"));
        drop(db);

        let columns = |schema: &'static str| async move {
            let pool = PgPoolOptions::new().max_connections(1).connect(&base_url()).await.unwrap();
            let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
                "SELECT column_name::text, data_type::text, is_nullable::text, column_default::text
                   FROM information_schema.columns
                  WHERE table_schema = $1 AND table_name = 'scrobbles'
                    AND column_name IN ('facets', 'facets_version', 'fact_key')
                  ORDER BY column_name",
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .unwrap_or_else(die("read columns"));
            pool.close().await;
            rows
        };
        let reference = columns("tapedeck_pg_check_facets_upgrade").await;
        assert_eq!(reference.len(), 3, "a fresh install has all three: {reference:?}");

        // Back to v0.131.3's scrobbles: the indexes on the columns go with them.
        let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
        for sql in [
            "DROP INDEX IF EXISTS idx_scrobbles_fact_key",
            "DROP INDEX IF EXISTS idx_scrobbles_facets_version",
            "ALTER TABLE scrobbles DROP COLUMN facets, DROP COLUMN facets_version, DROP COLUMN fact_key",
        ] {
            sqlx::query(sql).execute(&pool).await.unwrap_or_else(die("build v0.131.3 shape"));
        }
        pool.close().await;
        assert!(columns("tapedeck_pg_check_facets_upgrade").await.is_empty());

        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new over a v0.131.3 database"));
        assert_eq!(
            columns("tapedeck_pg_check_facets_upgrade").await,
            reference,
            "upgraded differs from fresh"
        );
        let todo = db
            .listens_needing_facets(crate::classify::VERSION, Some(me), 10)
            .await
            .unwrap_or_else(die("listens_needing_facets"));
        assert_eq!(todo.len(), 1, "the old listen is on the work list");

        // A second start is a no-op.
        drop(db);
        Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new, second start"));
    }

    /// A Postgres database made before v0.132.0 has `json` columns for
    /// `users.settings`, `signal_chains.components` and
    /// `chain_variants.overrides`, which refused every text bind and would not
    /// decode into a `String`. Opened by v0.132.0 they become TEXT — shaped as
    /// a fresh install's, defaults included — and keep what they held.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn json_columns_become_text_on_an_older_postgres_database() {
        let url = fresh("json_upgrade").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new"));
        let me = db.setup_admin("tester", None, "not-a-real-hash").await.unwrap();
        drop(db);

        let shape = |schema: &'static str| async move {
            let pool = PgPoolOptions::new().max_connections(1).connect(&base_url()).await.unwrap();
            let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
                "SELECT table_name::text, column_name::text, data_type::text, is_nullable::text,
                        column_default::text
                   FROM information_schema.columns
                  WHERE table_schema = $1
                    AND (table_name, column_name) IN (('users', 'settings'),
                         ('signal_chains', 'components'), ('chain_variants', 'overrides'))
                  ORDER BY 1, 2",
            )
            .bind(schema)
            .fetch_all(&pool)
            .await
            .unwrap_or_else(die("read columns"));
            pool.close().await;
            rows
        };
        let reference = shape("tapedeck_pg_check_json_upgrade").await;
        assert_eq!(reference.len(), 3);
        assert!(reference.iter().all(|r| r.2 == "text"), "a fresh install is TEXT: {reference:?}");

        // Back to the shape `JSON` used to translate to, with a chain in it.
        let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
        for sql in [
            "ALTER TABLE users ALTER COLUMN settings DROP DEFAULT".to_string(),
            "ALTER TABLE users ALTER COLUMN settings TYPE JSON USING settings::json".to_string(),
            "ALTER TABLE users ALTER COLUMN settings SET DEFAULT '{}'".to_string(),
            "ALTER TABLE signal_chains ALTER COLUMN components DROP DEFAULT".to_string(),
            "ALTER TABLE signal_chains ALTER COLUMN components TYPE JSON USING components::json"
                .to_string(),
            "ALTER TABLE signal_chains ALTER COLUMN components SET DEFAULT '[]'".to_string(),
            "ALTER TABLE chain_variants ALTER COLUMN overrides DROP DEFAULT".to_string(),
            "ALTER TABLE chain_variants ALTER COLUMN overrides TYPE JSON USING overrides::json"
                .to_string(),
            "ALTER TABLE chain_variants ALTER COLUMN overrides SET DEFAULT '[]'".to_string(),
            format!(
                "INSERT INTO signal_chains (user_id, name, components, created_at)
                 VALUES ({me}, 'Desk', '[{{\"type\":\"dac\",\"name\":\"E30\",\"detail\":null}}]', 0)"
            ),
        ] {
            sqlx::query(&sql).execute(&pool).await.unwrap_or_else(die("build the json shape"));
        }
        pool.close().await;
        assert!(shape("tapedeck_pg_check_json_upgrade").await.iter().all(|r| r.2 == "json"));

        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new over the json shape"));
        assert_eq!(
            shape("tapedeck_pg_check_json_upgrade").await,
            reference,
            "upgraded differs from fresh"
        );
        let chains = db.get_chains(me).await.unwrap_or_else(die("get_chains"));
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].components.len(), 1, "the chain kept its components");
        assert_eq!(chains[0].components[0].name, "E30");
        db.create_variant(me, chains[0].id.expect("an id"), "Other", &[])
            .await
            .unwrap_or_else(die("create_variant"));

        // A second start is a no-op.
        drop(db);
        Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new, second start"));
    }

    /// `COLLATE NOCASE` — Patch's username lookups and the Shelf's sort —
    /// which Postgres has no collation for. The dialect rewrites it to
    /// `LOWER(x COLLATE "C")`, and this holds it to SQLite's answers: ASCII
    /// case folds, nothing else does (`ünal` is not `Ünal`), and the Shelf
    /// sorts by the folded bytes.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn nocase_lookups_and_sorts_answer_as_sqlite_does() {
        let url = fresh("nocase").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new"));
        let me = db
            .setup_admin("tester", None, "not-a-real-hash")
            .await
            .unwrap_or_else(die("setup_admin"));
        let bob =
            db.create_user("Bob", None, "x", "member").await.unwrap_or_else(die("create_user Bob"));
        let unal = db
            .create_user("Ünal", None, "x", "member")
            .await
            .unwrap_or_else(die("create_user Ünal"));

        assert_eq!(db.patchable_user_id("TESTER").await.unwrap_or_else(die("patchable")), Some(me));
        assert_eq!(db.patchable_user_id("bob").await.unwrap_or_else(die("patchable")), Some(bob));
        assert_eq!(db.patchable_user_id("Ünal").await.unwrap_or_else(die("patchable")), Some(unal));
        assert_eq!(
            db.patchable_user_id("ünal").await.unwrap_or_else(die("patchable")),
            None,
            "SQLite's NOCASE folds ASCII only, and so must this"
        );
        db.discoverable_user_id("BOB").await.unwrap_or_else(die("discoverable_user_id"));
        db.patches_out(me).await.unwrap_or_else(die("patches_out"));
        db.patchable_decks(me).await.unwrap_or_else(die("patchable_decks"));
        db.deck_view(me, "BOB").await.unwrap_or_else(die("deck_view"));
        db.deck_i_am_patched_into(me, "bob").await.unwrap_or_else(die("deck_i_am_patched_into"));

        for (artist, title) in
            [("beta", "b"), ("Ångström", "c"), ("Alpha", "Zed"), ("alpha", "apple")]
        {
            db.create_physical(
                me, "vinyl", artist, title, None, None, None, None, None, None, "[]", None, None,
                None, None,
            )
            .await
            .unwrap_or_else(die("create_physical"));
        }
        let shelf = db.list_physical(me, None).await.unwrap_or_else(die("list_physical"));
        let order: Vec<(&str, &str)> =
            shelf.iter().map(|r| (r.artist.as_str(), r.title.as_str())).collect();
        // Folded bytes: "alpha" twice (then by title, folded), "beta", then
        // "Å", whose UTF-8 sorts after every ASCII letter.
        assert_eq!(order, [("alpha", "apple"), ("Alpha", "Zed"), ("beta", "b"), ("Ångström", "c")]);
    }

    /// The genre trajectory's periods on Postgres, which has no `strftime`:
    /// the same answers `db::tests::trajectory_periods_follow_the_listeners_offset`
    /// pins for SQLite.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn trajectory_periods_answer_as_sqlite_does() {
        let url = fresh("periods").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(die("Database::new"));
        let me = db.setup_admin("tester", None, "x").await.unwrap_or_else(die("setup_admin"));
        for (i, ts) in [1_735_687_800u64, 1_749_988_800, 1_677_664_800].into_iter().enumerate() {
            let play = crate::models::Play::new(
                format!("t{i}"),
                "Artist".into(),
                ts,
                format!("src-{ts}"),
                "test".into(),
            );
            db.save_scrobble(
                me,
                &play,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Default::default(),
            )
            .await
            .unwrap_or_else(die("save_scrobble"));
        }
        sqlx::query("UPDATE scrobbles SET genres = '[\"Ambient\",\"idm\"]' WHERE user_id = $1")
            .bind(me)
            .execute(&db.pool)
            .await
            .unwrap_or_else(die("genres"));

        let years =
            db.listens_by_period(me, 1.0, false).await.unwrap_or_else(die("listens_by_period"));
        assert_eq!(years, [("2023".to_string(), 1, 1), ("2025".to_string(), 2, 2)]);
        let utc = db.listens_by_period(me, 0.0, false).await.unwrap_or_else(die("utc"));
        assert_eq!(utc.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["2023", "2024", "2025"]);
        let months = db.listens_by_period(me, 1.0, true).await.unwrap_or_else(die("months"));
        assert_eq!(
            months.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ["2023-03", "2025-01", "2025-06"]
        );
        let behind = db.listens_by_period(me, -5.5, true).await.unwrap_or_else(die("behind"));
        assert_eq!(
            behind.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ["2023-03", "2024-12", "2025-06"]
        );

        let genres = db
            .genre_plays_by_period(me, 1.0, false)
            .await
            .unwrap_or_else(die("genre_plays_by_period"));
        assert_eq!(
            genres,
            [
                ("2023".to_string(), "Ambient".to_string(), 1),
                ("2023".to_string(), "idm".to_string(), 1),
                ("2025".to_string(), "Ambient".to_string(), 2),
                ("2025".to_string(), "idm".to_string(), 2),
            ]
        );
    }

    /// `chain_bindings`' columns (name, type, nullability) and foreign keys
    /// (name, definition), both sorted — column order differs after an
    /// `ADD COLUMN` and means nothing.
    async fn binding_shape(
        url: &str,
        name: &str,
    ) -> (Vec<(String, String, String)>, Vec<(String, String)>) {
        let schema = format!("{SCHEMA_PREFIX}_{name}");
        let pool = PgPoolOptions::new().max_connections(1).connect(url).await.unwrap();
        let cols: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT column_name::text, data_type::text, is_nullable::text
               FROM information_schema.columns
              WHERE table_schema = $1 AND table_name = 'chain_bindings'
              ORDER BY column_name",
        )
        .bind(&schema)
        .fetch_all(&pool)
        .await
        .unwrap();
        let fks: Vec<(String, String)> = sqlx::query_as(
            "SELECT c.conname::text, pg_get_constraintdef(c.oid)
               FROM pg_constraint c
               JOIN pg_class t ON t.oid = c.conrelid
               JOIN pg_namespace n ON n.oid = t.relnamespace
              WHERE n.nspname = $1 AND t.relname = 'chain_bindings' AND c.contype = 'f'
              ORDER BY c.conname",
        )
        .bind(&schema)
        .fetch_all(&pool)
        .await
        .unwrap();
        pool.close().await;
        (cols, fks)
    }

    /// What can this account actually do? Diagnostic, not a guard.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL"]
    async fn what_the_test_account_can_do() {
        let pool =
            PgPoolOptions::new().max_connections(1).connect(&admin_url()).await.expect("connect");
        for (label, sql) in [
            ("version", "SELECT version()"),
            ("current_user", "SELECT current_user"),
            ("current_database", "SELECT current_database()"),
            ("databases", "SELECT string_agg(datname, ', ' ORDER BY datname) FROM pg_database WHERE NOT datistemplate"),
            ("createdb", "SELECT rolcreatedb::text FROM pg_roles WHERE rolname = current_user"),
            ("superuser", "SELECT rolsuper::text FROM pg_roles WHERE rolname = current_user"),
            ("can create schema here", "SELECT has_database_privilege(current_database(), 'CREATE')::text"),
            ("schemas", "SELECT string_agg(nspname, ', ' ORDER BY nspname) FROM pg_namespace WHERE nspname !~ '^pg_'"),
            ("extensions", "SELECT string_agg(extname, ', ' ORDER BY extname) FROM pg_extension"),
        ] {
            let got: Result<Option<String>, _> =
                sqlx::query_scalar(sql).fetch_one(&pool).await;
            match got {
                Ok(v) => println!("  {label:24} {}", v.unwrap_or_else(|| "(null)".into())),
                Err(e) => println!("  {label:24} ERROR {}", crate::logging::error_chain(&e)),
            }
        }
    }

    /// Open a fresh Postgres database and report exactly where it stops.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn a_fresh_postgres_database_reaches_the_current_shape() {
        let url = fresh("shape").await;
        let db = match Database::new(&url, Arc::new(SecretBox::test_key())).await {
            Ok(db) => db,
            Err(e) => panic!("Database::new: {}", crate::logging::error_chain(&e)),
        };

        let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
        let tables: Vec<String> =
            sqlx::query("SELECT tablename FROM pg_tables WHERE schemaname = $1 ORDER BY tablename")
                .bind(format!("{SCHEMA_PREFIX}_shape"))
                .fetch_all(&pool)
                .await
                .expect("list tables")
                .into_iter()
                .map(|r| r.get::<String, _>(0))
                .collect();
        let indexes: Vec<String> = sqlx::query(
            "SELECT indexname FROM pg_indexes WHERE schemaname = $1 ORDER BY indexname",
        )
        .bind(format!("{SCHEMA_PREFIX}_shape"))
        .fetch_all(&pool)
        .await
        .expect("list indexes")
        .into_iter()
        .map(|r| r.get::<String, _>(0))
        .collect();

        println!("{} tables, {} indexes", tables.len(), indexes.len());
        for want in ["scrobbles", "users", "notes", "signal_chains", "spools"] {
            assert!(tables.contains(&want.to_string()), "no `{want}` table; got {tables:?}");
        }
        for want in ["idx_notes_user_entity", "idx_scrobbles_session", "idx_entities_match"] {
            assert!(indexes.contains(&want.to_string()), "no `{want}`; got {indexes:?}");
        }
        drop(db);
    }

    /// Every query behind Classical, Scores & Ambient, the seasonal lenses,
    /// B-Sides, the split Top Charts and anniversaries, run on Postgres with
    /// rows behind it — written for v0.132.0, whose SQL was otherwise only
    /// ever executed on SQLite. A shape Postgres refuses (a GROUP BY it
    /// considers ungrouped, an untyped NULL, a SQLite-only function) fails
    /// here; the asserted numbers keep it about the answers too.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn the_insight_facets_run_on_postgres() {
        use crate::classify::{self, ArtistDates, Outcome, PerformerRow};
        use crate::db::InsightFilter;

        let url = fresh("facets").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(|e| panic!("Database::new: {}", crate::logging::error_chain(&e)));
        let uid = db
            .setup_admin("tester", None, "not-a-real-hash")
            .await
            .unwrap_or_else(die("setup_admin"));

        const MENDELSSOHN: &str = "c70d12a2-24fe-4f83-a6e6-57d84f8efb51";
        const CONCERTO: &str = "w:0b0b0b0b-0000-4000-8000-000000000001";
        let base = 1_766_000_000u64; // December 2025
                                     // (title, artist, album, recording mbid, artist mbid, offset)
        let listens = [
            (
                "Violin Concerto in E minor: I. Allegro molto appassionato",
                "Hilary Hahn",
                "Mendelssohn / Shostakovich",
                "rec-1",
                MENDELSSOHN,
                0u64,
            ),
            (
                "Violin Concerto in E minor: II. Andante",
                "Hilary Hahn",
                "Mendelssohn / Shostakovich",
                "rec-2",
                MENDELSSOHN,
                700,
            ),
            (
                "Violin Concerto in E minor: III. Allegretto non troppo",
                "Hilary Hahn",
                "Mendelssohn / Shostakovich",
                "rec-3",
                MENDELSSOHN,
                1200,
            ),
            (
                "Derezzed",
                "Daft Punk",
                "TRON: Legacy",
                "rec-4",
                "056e4f3e-d505-4dad-8ec1-d04f521cbb56",
                9_000,
            ),
            (
                "Arrival of the Birds",
                "The Cinematic Orchestra",
                "The Crimson Wing",
                "rec-5",
                "a1b2c3d4-0000-4000-8000-000000000005",
                12_000,
            ),
            (
                "1/1",
                "Brian Eno",
                "Ambient 1: Music for Airports",
                "rec-6",
                "ff95eb47-41c4-4f7f-a104-cdc30f02e872",
                15_000,
            ),
            (
                "Last Christmas",
                "Wham!",
                "Last Christmas",
                "rec-7",
                "5f7d0f1c-0000-4000-8000-000000000007",
                18_000,
            ),
            (
                "Last Christmas",
                "Taylor Swift",
                "Sounds of the Season",
                "rec-8",
                "20244d07-534f-4eff-b4d4-930878889970",
                18_400,
            ),
            (
                "Everything In Its Right Place",
                "Radiohead",
                "Kid A",
                "rec-9",
                "a74b1b7f-71a5-4011-9441-d0b5e4122711",
                21_000,
            ),
        ];
        for (i, (title, artist, album, rec, artist_mbid, off)) in listens.iter().enumerate() {
            let mut play = crate::models::Play::new(
                title.to_string(),
                artist.to_string(),
                base + off,
                format!("pg-facets-{i}"),
                "test".into(),
            );
            play.album = Some(album.to_string());
            play.duration = Some(300);
            play.mbid_recording = Some(format!("00000000-0000-4000-8000-00000000000{i}"));
            play.mbid_release = Some(format!("rel-{rec}"));
            play.mbid_artist = Some(vec![artist_mbid.to_string()]);
            db.save_scrobble(
                uid,
                &play,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Default::default(),
            )
            .await
            .unwrap_or_else(die("save_scrobble"));
        }
        db.rebuild_sessions(uid, 1800).await.unwrap_or_else(die("rebuild_sessions"));

        let todo = db
            .listens_needing_facets(classify::VERSION, Some(uid), 100)
            .await
            .unwrap_or_else(die("listens_needing_facets"));
        assert_eq!(todo.len(), listens.len());
        assert_eq!(
            db.facets_pending(uid, classify::VERSION).await.unwrap_or_else(die("facets_pending")),
            listens.len() as i64
        );

        let hahn = PerformerRow {
            name: "Hilary Hahn".into(),
            mbid: None,
            role: "soloist".into(),
            instrument: Some("violin".into()),
            country: Some("US".into()),
        };
        let conductor = PerformerRow {
            name: "Hugh Wolff".into(),
            mbid: None,
            role: "conductor".into(),
            instrument: None,
            country: Some("US".into()),
        };
        let orchestra = PerformerRow {
            name: "Oslo Philharmonic Orchestra".into(),
            mbid: None,
            role: "orchestra".into(),
            instrument: None,
            country: Some("NO".into()),
        };
        for l in &todo {
            let key = l.fact_key();
            let movement = l.title.split(": ").nth(1).map(str::to_string);
            let o = match l.artist.as_str() {
                "Hilary Hahn" => Outcome {
                    facets: classify::CLASSICAL,
                    piece_key: CONCERTO.into(),
                    piece_title: "Violin Concerto in E minor, Op. 64".into(),
                    movement: movement.clone(),
                    composer_name: Some("Felix Mendelssohn".into()),
                    composer_mbid: Some(MENDELSSOHN.into()),
                    leaf_key: Some(format!("w:leaf-{}", movement.unwrap_or_default())),
                    part_count: Some(3),
                    composed_year: Some(1844),
                    form: Some("Concerto".into()),
                    performers: vec![conductor.clone(), orchestra.clone()],
                    evidence: vec!["canon composer".into()],
                },
                "Daft Punk" => Outcome {
                    facets: classify::SCORE,
                    piece_key: "t:derezzed".into(),
                    piece_title: l.title.clone(),
                    ..Default::default()
                },
                "The Cinematic Orchestra" => Outcome {
                    facets: classify::SCORE_SUGGESTED,
                    piece_key: "t:arrival of the birds".into(),
                    piece_title: l.title.clone(),
                    ..Default::default()
                },
                "Brian Eno" => Outcome {
                    facets: classify::AMBIENT,
                    piece_key: "t:1/1".into(),
                    piece_title: l.title.clone(),
                    ..Default::default()
                },
                "Wham!" | "Taylor Swift" => Outcome {
                    facets: classify::CHRISTMAS,
                    piece_key: "w:last-christmas".into(),
                    piece_title: "Last Christmas".into(),
                    ..Default::default()
                },
                _ => Outcome {
                    facets: 0,
                    piece_key: "t:everything in its right place".into(),
                    piece_title: l.title.clone(),
                    ..Default::default()
                },
            };
            let inferred = if l.artist == "Hilary Hahn" { vec![hahn.clone()] } else { vec![] };
            db.store_fact(&key, classify::VERSION, &o, &inferred)
                .await
                .unwrap_or_else(die("store_fact"));
            // Storing twice is the upsert path a re-run takes.
            db.store_fact(&key, classify::VERSION, &o, &inferred)
                .await
                .unwrap_or_else(die("store_fact again"));
            let back = db
                .stored_fact(&key, classify::VERSION)
                .await
                .unwrap_or_else(die("stored_fact"))
                .expect("a stored fact reads back");
            assert_eq!(back.facets, o.facets);
            db.set_listen_facets(l.id, &key, o.facets, classify::VERSION)
                .await
                .unwrap_or_else(die("set_listen_facets"));
        }
        assert_eq!(
            db.facets_pending(uid, classify::VERSION).await.unwrap_or_else(die("pending")),
            0
        );
        let role = db.stated_role("hugh wolff").await.unwrap_or_else(die("stated_role"));
        println!("stated role of Hugh Wolff: {role:?}");

        // ── Classical ──
        let all = InsightFilter::default();
        for rank in [
            "pieces",
            "movements",
            "composers",
            "performances",
            "conductor",
            "orchestra",
            "ensemble",
            "soloist",
        ] {
            for by_hours in [false, true] {
                let rows =
                    db.classical_chart(uid, rank, &all, by_hours, 20).await.unwrap_or_else(|e| {
                        panic!("classical_chart {rank}: {}", crate::logging::error_chain(&e))
                    });
                println!(
                    "classical {rank} (hours={by_hours}): {:?}",
                    rows.iter().map(|r| (&r.name, r.plays)).collect::<Vec<_>>()
                );
                match rank {
                    "pieces" | "composers" | "performances" => {
                        assert_eq!(rows[0].plays, 3, "{rank}")
                    }
                    "movements" => assert_eq!(rows.len(), 3),
                    "soloist" | "conductor" | "orchestra" => assert_eq!(rows.len(), 1, "{rank}"),
                    _ => assert!(rows.is_empty(), "{rank}"),
                }
            }
        }
        let soloists = db
            .classical_chart(uid, "soloist", &all, false, 20)
            .await
            .unwrap_or_else(die("soloist"));
        assert_eq!(soloists[0].inferred, Some(1), "Hahn's role was inferred");
        let filtered = InsightFilter {
            after: Some(base as i64 - 1),
            before: Some(base as i64 + 100_000),
            composer: Some(MENDELSSOHN.into()),
            piece: Some(CONCERTO.into()),
            performer: Some("hilary hahn".into()),
            instrument: Some("violin".into()),
        };
        for rank in ["pieces", "movements", "composers", "performances", "soloist"] {
            let rows =
                db.classical_chart(uid, rank, &filtered, false, 20).await.unwrap_or_else(|e| {
                    panic!("filtered classical_chart {rank}: {}", crate::logging::error_chain(&e))
                });
            assert!(!rows.is_empty(), "filtered {rank} came back empty");
        }
        let hearings = db
            .classical_hearing_rows(uid, &all)
            .await
            .unwrap_or_else(die("classical_hearing_rows"));
        assert_eq!(hearings.len(), 3);
        assert!(
            hearings.iter().all(|h| h.0.is_some() && h.0 == hearings[0].0),
            "one sitting: {hearings:?}"
        );
        let instruments =
            db.classical_instruments(uid, &all).await.unwrap_or_else(die("classical_instruments"));
        assert_eq!(instruments, vec![("violin".to_string(), 3)]);
        let coverage = db
            .classical_role_coverage(uid, &all)
            .await
            .unwrap_or_else(die("classical_role_coverage"));
        println!("role coverage: {coverage:?}");
        assert_eq!(coverage.seconds, 900);
        let stats =
            db.classical_stat_rows(uid, &filtered).await.unwrap_or_else(die("classical_stat_rows"));
        assert_eq!(stats.len(), 3);
        let perf = db
            .classical_performer_rows(uid, &all)
            .await
            .unwrap_or_else(die("classical_performer_rows"));
        assert!(!perf.is_empty());
        let timeline =
            db.classical_timeline(uid, &all).await.unwrap_or_else(die("classical_timeline"));
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].composed_year, Some(1844));

        // ── Scores, lenses, B-Sides ──
        for flag in [classify::SCORE, classify::AMBIENT, classify::CHRISTMAS, classify::HALLOWEEN] {
            for by_hours in [false, true] {
                let rows = db.lens_songs(uid, flag, &all, by_hours, 20).await.unwrap_or_else(|e| {
                    panic!("lens_songs {flag}: {}", crate::logging::error_chain(&e))
                });
                if flag == classify::CHRISTMAS {
                    assert_eq!(rows.len(), 1, "two covers, one song");
                    assert_eq!(rows[0].versions, Some(2));
                }
            }
        }
        let windowed = InsightFilter {
            after: Some(base as i64),
            before: Some(base as i64 + 19_000),
            ..Default::default()
        };
        db.lens_songs(uid, classify::CHRISTMAS, &windowed, false, 20)
            .await
            .unwrap_or_else(die("lens_songs window"));
        let versions = db
            .piece_versions(uid, "w:last-christmas", 20)
            .await
            .unwrap_or_else(die("piece_versions"));
        assert_eq!(versions.len(), 2);
        let suggestions =
            db.score_suggestions(uid, 20).await.unwrap_or_else(die("score_suggestions"));
        assert_eq!(suggestions.len(), 1, "{suggestions:?}");
        let tl =
            db.facet_timeline(uid, None, None, 100).await.unwrap_or_else(die("facet_timeline"));
        println!("facet timeline: {tl:?}");
        db.facet_timeline(uid, Some(base as i64), Some(base as i64 + 86_400), 100)
            .await
            .unwrap_or_else(die("facet_timeline window"));
        let ts = db
            .facet_timestamps(uid, classify::CHRISTMAS)
            .await
            .unwrap_or_else(die("facet_timestamps"));
        assert_eq!(ts.len(), 2);
        let counts = db.facet_counts(uid).await.unwrap_or_else(die("facet_counts"));
        println!("facet counts: {counts:?}");
        assert_eq!(counts[0], 3, "classical");

        // ── Top Charts with the split ──
        let split = classify::CLASSICAL | classify::SCORE | classify::AMBIENT;
        for kind in ["artists", "albums", "tracks"] {
            for by_hours in [false, true] {
                let rows =
                    db.top_chart(uid, kind, None, 50, by_hours, split).await.unwrap_or_else(|e| {
                        panic!("top_chart {kind}: {}", crate::logging::error_chain(&e))
                    });
                assert!(
                    rows.iter().all(|r| r.name != "Hilary Hahn" && r.name != "Brian Eno"),
                    "{kind}"
                );
            }
            db.chart_filtered(
                uid,
                kind,
                Some(0),
                Some(i64::MAX),
                50,
                false,
                0,
                classify::CHRISTMAS,
            )
            .await
            .unwrap_or_else(|e| {
                panic!("chart_filtered {kind}: {}", crate::logging::error_chain(&e))
            });
        }
        assert_eq!(db.split_out(uid, None, split).await.unwrap_or_else(die("split_out")), 5);
        assert_eq!(
            db.split_out(uid, Some(base as i64 + 10_000), split)
                .await
                .unwrap_or_else(die("split_out after")),
            1
        );
        let spread = db
            .artist_album_plays(uid, "Hilary Hahn")
            .await
            .unwrap_or_else(die("artist_album_plays"));
        assert_eq!(spread.len(), 1);

        // ── Overrides: artist, album, work; set, read, clear ──
        let n = db
            .set_facet_override(
                uid,
                "artist",
                "the cinematic orchestra",
                classify::SCORE,
                Some(true),
            )
            .await
            .unwrap_or_else(die("set_facet_override artist"));
        println!("artist override requeued {n}");
        db.set_facet_override(uid, "album", "kid a\u{1f}radiohead", classify::AMBIENT, Some(false))
            .await
            .unwrap_or_else(die("set_facet_override album"));
        db.set_facet_override(uid, "work", "t:derezzed", classify::SCORE, Some(false))
            .await
            .unwrap_or_else(die("set_facet_override work"));
        // The upsert path.
        db.set_facet_override(uid, "work", "t:derezzed", classify::SCORE, Some(true))
            .await
            .unwrap_or_else(die("set_facet_override again"));
        let ov = db.facet_overrides(uid).await.unwrap_or_else(die("facet_overrides"));
        assert_eq!(ov.len(), 3);
        let on = db
            .facet_overrides_on(uid, "work", "t:derezzed")
            .await
            .unwrap_or_else(die("facet_overrides_on"));
        assert_eq!(on, vec![(classify::SCORE, true)]);
        db.set_facet_override(uid, "work", "t:derezzed", classify::SCORE, None)
            .await
            .unwrap_or_else(die("clear override"));
        assert_eq!(db.facet_overrides(uid).await.unwrap_or_else(die("facet_overrides")).len(), 2);

        // ── Dates and anniversaries ──
        let needing =
            db.artists_needing_dates(50).await.unwrap_or_else(die("artists_needing_dates"));
        println!("artists needing dates: {}", needing.len());
        db.store_artist_dates(
            MENDELSSOHN,
            &ArtistDates {
                name: "Felix Mendelssohn".into(),
                kind: Some("Person".into()),
                gender: Some("Male".into()),
                country: Some("DE".into()),
                begin: Some("1809-02-03".into()),
                end: Some("1847-11-04".into()),
                ended: true,
            },
        )
        .await
        .unwrap_or_else(die("store_artist_dates"));
        db.store_artist_dates(
            "a74b1b7f-71a5-4011-9441-d0b5e4122711",
            &ArtistDates {
                name: "Radiohead".into(),
                kind: Some("Group".into()),
                begin: Some("1985".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_or_else(die("store_artist_dates again"));
        let dated = db.dated_artists(uid, 1).await.unwrap_or_else(die("dated_artists"));
        println!(
            "dated artists: {:?}",
            dated.iter().map(|d| (&d.name, &d.begin, d.plays)).collect::<Vec<_>>()
        );
        // Enrichment fills `release_year`; written straight here.
        sqlx::query(
            "UPDATE scrobbles SET release_year = 2000 WHERE user_id = $1 AND album = 'Kid A'",
        )
        .bind(uid)
        .execute(&db.pool)
        .await
        .unwrap_or_else(die("release_year"));
        let albums = db.dated_albums(uid, 1).await.unwrap_or_else(die("dated_albums"));
        println!("dated albums: {albums:?}");
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].2, 2000);
        let radiohead = db
            .artist_dates_for(Some("a74b1b7f-71a5-4011-9441-d0b5e4122711"), "Radiohead")
            .await
            .unwrap_or_else(die("artist_dates_for"));
        assert_eq!(radiohead.map(|r| r.begin), Some(Some("1985".to_string())));
        db.artist_dates_for(None, "Felix Mendelssohn")
            .await
            .unwrap_or_else(die("artist_dates_for by name"));
        let stats = db
            .classical_stat_rows(uid, &all)
            .await
            .unwrap_or_else(die("classical_stat_rows dated"));
        assert!(
            stats.iter().all(|r| r.born.as_deref() == Some("1809-02-03")),
            "the composer's dates join"
        );

        // ── A member who leaves takes their overrides ──
        let member =
            db.create_user("leaver", None, "x", "member").await.unwrap_or_else(die("create_user"));
        db.set_facet_override(member, "artist", "radiohead", classify::CLASSICAL, Some(true))
            .await
            .unwrap_or_else(die("member override"));
        assert!(db.delete_user(member).await.unwrap_or_else(die("delete_user")));
        drop(db);
    }

    /// The older readers that sum a duration into an integer: the Dashboard,
    /// Statistics, album and artist pages, sessions, chains, gear and the
    /// Shelf. On Postgres `SUM(bigint)` is NUMERIC and will not decode into an
    /// `i64`; `a_listen_survives_a_round_trip_through_postgres` never saw it
    /// because its one listen has no duration, so every sum was NULL and NULL
    /// is never type-checked. Every listen here has a length.
    #[tokio::test]
    #[ignore = "needs TAPEDECK_TEST_POSTGRES_URL and a reachable server"]
    async fn duration_sums_decode_on_postgres() {
        let url = fresh("sums").await;
        let db = Database::new(&url, Arc::new(SecretBox::test_key()))
            .await
            .unwrap_or_else(|e| panic!("Database::new: {}", crate::logging::error_chain(&e)));
        let uid = db
            .setup_admin("tester", None, "not-a-real-hash")
            .await
            .unwrap_or_else(die("setup_admin"));
        let chain = insert_chain(&db, uid, "Desk").await;
        let now = chrono::Utc::now().timestamp();
        // The app's own writers bind `components` and `overrides` as text,
        // which a Postgres `json` column refused before v0.132.0.
        let equipment = insert_returning_id(
            &db,
            &format!("INSERT INTO equipment (user_id, name, type) VALUES ({uid}, 'Stylus', 'cartridge') RETURNING id"),
        )
        .await;
        let components: Vec<crate::models::ChainComponent> = serde_json::from_str(&format!(
            r#"[{{"type":"source","name":"Turntable","detail":null}},
                {{"type":"transducer","name":"Stylus","detail":null,"equipment_id":{equipment}}}]"#
        ))
        .expect("components parse");
        assert!(db
            .update_chain(uid, chain, "Desk", None, &components, "unknown", None)
            .await
            .unwrap_or_else(die("update_chain")));
        db.create_variant(uid, chain, "Other stylus", &[])
            .await
            .unwrap_or_else(die("create_variant"));

        for (i, title) in ["Xtal", "Tha", "Xtal", "Pulsewidth"].iter().enumerate() {
            let mut play = crate::models::Play::new(
                title.to_string(),
                "Aphex Twin".into(),
                (now - 3_600 + i as i64 * 300) as u64,
                format!("pg-sums-{i}"),
                "test".into(),
            );
            play.album = Some("Selected Ambient Works 85-92".into());
            play.duration = Some(240);
            db.save_scrobble(
                uid,
                &play,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Default::default(),
            )
            .await
            .unwrap_or_else(die("save_scrobble"));
        }
        sqlx::query(
            "UPDATE scrobbles SET chain_id = $1, quality_score = 0.75, genres = '[\"ambient\",\"idm\"]'
              WHERE user_id = $2",
        )
            .bind(chain)
            .bind(uid)
            .execute(&db.pool)
            .await
            .unwrap_or_else(die("chain and quality"));
        db.rebuild_sessions(uid, 1800).await.unwrap_or_else(die("rebuild_sessions"));
        let session: i64 =
            sqlx::query_scalar("SELECT MAX(session_id) FROM scrobbles WHERE user_id = $1")
                .bind(uid)
                .fetch_one(&db.pool)
                .await
                .unwrap_or_else(die("session id"));
        assert!(db.recompute_session(session).await.unwrap_or_else(die("recompute_session")));

        let dash = db
            .get_dashboard_stats(uid, chrono_tz::Tz::UTC, 1)
            .await
            .unwrap_or_else(|e| panic!("get_dashboard_stats: {}", crate::logging::error_chain(&e)));
        println!("dashboard: {}", serde_json::to_value(&dash).unwrap());
        for (after, by_hours) in [(None, false), (Some(0), true)] {
            let ranged = db.get_ranged_stats(uid, after, by_hours).await.unwrap_or_else(|e| {
                panic!("get_ranged_stats {by_hours}: {}", crate::logging::error_chain(&e))
            });
            let v = serde_json::to_value(&ranged).unwrap();
            println!(
                "ranged (hours={by_hours}): {}",
                v.to_string().chars().take(400).collect::<String>()
            );
        }
        let album = db
            .album_summary(uid, "Aphex Twin", "Selected Ambient Works 85-92")
            .await
            .unwrap_or_else(die("album_summary"))
            .expect("the album has listens");
        assert_eq!((album.plays, album.seconds), (4, 960));
        let artist = db
            .artist_summary(uid, "Aphex Twin")
            .await
            .unwrap_or_else(die("artist_summary"))
            .expect("the artist has listens");
        assert_eq!((artist.plays, artist.seconds), (4, 960));
        let genres = db.genre_play_counts(uid, None).await.unwrap_or_else(die("genre_play_counts"));
        assert_eq!(genres.len(), 2, "{genres:?}");
        assert_eq!(genres[0].1, 4);
        db.artists_by_genre(uid).await.unwrap_or_else(die("artists_by_genre"));
        db.listening_coverage(uid).await.unwrap_or_else(die("listening_coverage"));
        db.recording_mbid_coverage(uid).await.unwrap_or_else(die("recording_mbid_coverage"));

        // ── Chains, gear and the Shelf ──
        let chains = db.get_chains(uid).await.unwrap_or_else(die("get_chains"));
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].components.len(), 2, "components read back through TEXT");
        assert!(
            (chains[0].total_hours - 960.0 / 3_600.0).abs() < 1e-9,
            "{}",
            chains[0].total_hours
        );
        let variants = db.list_variants(uid).await.unwrap_or_else(die("list_variants"));
        assert_eq!(variants.len(), 1);
        db.near_duplicate_chains(uid).await.unwrap_or_else(die("near_duplicate_chains"));
        db.equipment_seconds(uid).await.unwrap_or_else(die("equipment_seconds"));
        let gear = db.get_equipment(uid).await.unwrap_or_else(die("get_equipment"));
        assert_eq!(gear.len(), 1);

        let release = db
            .create_physical(
                uid,
                "vinyl",
                "Aphex Twin",
                "Selected Ambient Works 85-92",
                Some(1992),
                None,
                None,
                None,
                None,
                None,
                "[]",
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap_or_else(die("create_physical"));
        db.record_physical_play(
            uid,
            release,
            "A",
            now - 600,
            1_200,
            5,
            Some(equipment),
            Some(chain),
        )
        .await
        .unwrap_or_else(die("record_physical_play"));
        db.list_physical(uid, None).await.unwrap_or_else(die("list_physical"));
        db.get_physical(uid, release).await.unwrap_or_else(die("get_physical"));
        let sides = db.side_plays(uid, release).await.unwrap_or_else(die("side_plays"));
        assert_eq!(sides.len(), 1);
        let (hours, n) = db
            .equipment_playback_hours(uid, equipment)
            .await
            .unwrap_or_else(die("equipment_playback_hours"));
        assert_eq!(n, 1);
        assert!((hours - 1_200.0 / 3_600.0).abs() < 1e-9, "{hours}");
        drop(db);
    }
}
