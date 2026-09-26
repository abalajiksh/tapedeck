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
