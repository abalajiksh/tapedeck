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

    /// A row written straight to its table, returning its id.
    ///
    /// For `signal_chains` and `chain_variants`: `create_chain` and
    /// `create_variant` bind their JSON columns as text, which Postgres refuses
    /// (`column "components" is of type json but expression is of type
    /// text`). A gap of its own, recorded in the PR rather than fixed here.
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
}
