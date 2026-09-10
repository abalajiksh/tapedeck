use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

static N: AtomicU32 = AtomicU32::new(0);

async fn testdb() -> Database {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-test-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }
    Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
        .await
        .unwrap()
}

/// A fresh database really ends up with every index, wherever it is created.
///
/// The drift guard in `db::guards` reads the *source*; this reads the
/// *database*. They catch different things: that one fails if a name is missing
/// from the shared list, this one fails if the list is never run, runs in the
/// wrong order, or silently errors — which is how `idx_scrobbles_user_source`
/// would go missing and `save_scrobble` start accepting the same listen twice.
#[tokio::test]
async fn a_fresh_database_has_every_index_the_shape_calls_for() {
    let db = testdb().await;
    let names: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'index' ORDER BY name")
            .fetch_all(&db.pool)
            .await
            .expect("read the index list");
    let have: std::collections::BTreeSet<String> = names.into_iter().map(|(n,)| n).collect();

    for want in [
        // Correctness first — each of these is a rule, not a speed-up.
        "idx_spools_one_per_sitting",
        "idx_notes_user_entity",
        "idx_notes_user_scrobble",
        "idx_notes_user_period",
        "idx_notes_user_line",
        // Then the ones that only make something fast.
        "idx_scrobbles_session",
        "idx_scrobbles_album_release",
        "idx_entities_match",
        "idx_entities_mbid",
        // And one from `init` itself, so this fails loudly if the whole
        // schema stopped being applied rather than only the new step.
        "idx_scrobbles_user_time",
    ] {
        assert!(have.contains(want), "a fresh database is missing {want}; it has {have:?}");
    }
}

/// The dedup backstop is a *constraint*, so it is tested as one.
///
/// `save_scrobble` asks `exists` before inserting, and the UNIQUE behind it is
/// what makes that safe against a race rather than merely usually right. It is
/// declared inline in `init()`'s `CREATE TABLE`, so it carries whatever name
/// the engine chooses — which is why this asserts the *behaviour* and not an
/// index called `idx_scrobbles_user_source`. That name is a historical one
/// `migrate()` gives to databases predating the `user_id` column.
#[tokio::test]
async fn a_fresh_database_refuses_the_same_listen_twice_at_the_constraint() {
    let db = testdb().await;
    let insert =
        "INSERT INTO scrobbles (user_id, title, artist, timestamp, source_id, source_name, status)
                  VALUES (1, 'Xtal', 'Aphex Twin', 100, 'src-1', 'test', 'imported')";
    sqlx::query(insert).execute(&db.pool).await.expect("the first insert is fine");
    let again = sqlx::query(insert).execute(&db.pool).await;
    assert!(
        again.is_err(),
        "a second listen with the same (user_id, source_id, source_name) was accepted — \
         the dedup constraint is missing, and `exists` is on its own"
    );
}

/// Open a **real** existing database and prove the migration path survives it.
///
/// Opt-in and `#[ignore]`d: it needs a database nobody else has.
///
/// ```text
/// TAPEDECK_TEST_EXISTING_DB=/path/to/a/COPY/of/scrobbles.db \
///   cargo test --bin tapedeck -- --ignored an_existing_production_database
/// ```
///
/// **Point it at a copy.** It opens the file for writing and runs `init()`,
/// `migrate()` and `indexes()` against it, which is the whole thing being
/// tested.
///
/// Every other migration test in this file builds a pre-version schema by hand,
/// which is exactly the blind spot v0.61.1 fell into: a migration that could not
/// run against a real database crash-looped 297 times while the suite stayed
/// green, because every test starts from an empty file.
#[tokio::test]
#[ignore = "needs TAPEDECK_TEST_EXISTING_DB pointing at a copy of a real database"]
async fn an_existing_production_database_opens_and_keeps_its_listens() {
    let Ok(path) = std::env::var("TAPEDECK_TEST_EXISTING_DB") else {
        panic!("set TAPEDECK_TEST_EXISTING_DB to a COPY of a real scrobbles.db");
    };
    let url = format!("sqlite:{path}");

    async fn count(db: &Database) -> i64 {
        let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM scrobbles")
            .fetch_one(&db.pool)
            .await
            .expect("count listens");
        n.0
    }

    let started = std::time::Instant::now();
    let db = Database::new(&url, Arc::new(SecretBox::test_key()))
        .await
        .unwrap_or_else(|e| panic!("an existing database must open: {e}"));
    let opened_in = started.elapsed();

    let listens = count(&db).await;
    println!("opened in {opened_in:?}, {listens} listens");
    assert!(listens > 0, "the database came back empty");

    // The four columns that used to exist only in `migrate()`. On this database
    // they arrived by ALTER long ago; what is being checked is that adding them
    // to `init()` did not disturb anything.
    for (table, col) in [
        ("scrobbles", "chain_variant_id"),
        ("physical_plays", "chain_variant_id"),
        ("physical_releases", "chain_variant_id"),
        ("recommendations", "from_user_id"),
    ] {
        let n: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{col}'"
        ))
        .fetch_one(&db.pool)
        .await
        .expect("read the column list");
        assert_eq!(n.0, 1, "{table}.{col} is missing after opening");
    }

    // Opening twice must be a no-op, or the migration is not idempotent — which
    // matters because it runs on every single start.
    drop(db);
    let db2 = Database::new(&url, Arc::new(SecretBox::test_key()))
        .await
        .unwrap_or_else(|e| panic!("a second open must also work: {e}"));
    assert_eq!(count(&db2).await, listens, "reopening changed the listen count");
    println!("reopened cleanly, still {listens} listens");

    // Opening exercises perhaps fifty of the 577 rewritten query sites. These
    // are the heavy readouts, on real data with real edge cases — the thing a
    // synthetic fixture cannot stand in for.
    let uid: (i64,) =
        sqlx::query_as("SELECT MIN(id) FROM users").fetch_one(&db2.pool).await.expect("a user");
    let uid = uid.0;

    let t = std::time::Instant::now();
    let dash = db2.get_dashboard_stats(uid, chrono_tz::Tz::UTC, 1).await.expect("dashboard");
    println!("  dashboard        {:>7.0?}  total={}", t.elapsed(), dash["total"]);

    let t = std::time::Instant::now();
    let ranged = db2.get_ranged_stats(uid, None, false).await.expect("ranged stats");
    let totals = &ranged["totals"];
    println!(
        "  stats?range=all  {:>7.0?}  plays={} hours={}",
        t.elapsed(),
        totals["plays"],
        totals["hours"]
    );
    assert!(totals["plays"].as_i64().unwrap_or(0) > 0, "ranged stats came back empty");

    for kind in ["artists", "albums", "tracks"] {
        let t = std::time::Instant::now();
        let rows = db2.chart(uid, kind, None, 10, false).await.expect("chart");
        println!(
            "  chart {kind:<8}   {:>7.0?}  {} rows, top: {}",
            t.elapsed(),
            rows.len(),
            rows.first().map(|r| r.name.as_str()).unwrap_or("—")
        );
    }

    let t = std::time::Instant::now();
    let variants = db2.artists_with_spelling_variants(uid).await.expect("variants");
    println!("  spelling variants{:>7.0?}  {} groups", t.elapsed(), variants.len());
}

/// A build refuses a URL it cannot serve, in words, before it tries to connect.
///
/// The failure this prevents is a driver error twenty frames down when the real
/// answer is "you want the other build". Only checkable in the SQLite
/// direction here, because the Postgres build compiles this module out.
#[tokio::test]
async fn a_build_refuses_a_url_for_the_other_backend() {
    // `Database` is not Debug, so match rather than `expect_err`.
    let msg =
        match Database::new("postgres://u:p@localhost/tapedeck", Arc::new(SecretBox::test_key()))
            .await
        {
            Ok(_) => panic!("a SQLite build must not accept a postgres URL"),
            Err(e) => e.to_string(),
        };
    assert!(msg.contains("SQLite"), "should name the build it is: {msg}");
    assert!(msg.contains("sqlite"), "should name the scheme it accepts: {msg}");
    assert!(msg.contains("--features postgres"), "should point at the other build: {msg}");
}

fn play(title: &str, artist: &str, ts: u64) -> Play {
    Play::new(title.into(), artist.into(), ts, format!("src-{ts}"), "test".into())
}

/// A grouped row's cover has to come from **one listen**.
///
/// Reported 2026-08-21: a Cœur de pirate record wore a 5-CD compilation's
/// sleeve on Top Charts while the album page showed the right one. The
/// group held listens of two releases — ordinary, since one of its tracks
/// also appears on that compilation — and the three artwork columns were
/// three independent `MAX()`es, so each was answered by whichever row won
/// its own comparison.
///
/// The numbers below are the real ones, and they are chosen so the old
/// query is *wrong in every column at once*: `MAX(artwork_url)` and
/// `MAX(caa_release_mbid)` both pick the compilation, because `MAX` on text
/// is lexicographic and `"ead839e9…" > "9e5838c1…"`, while `MAX(caa_id)`
/// picks the album, because its id is numerically larger. A test built on
/// tidier values would pass against the bug.
/// The dashboard's folded aggregates answer exactly what the thirteen
/// separate queries did.
///
/// Written against the **old SQL run side by side**, not against numbers
/// typed out by hand: this collapsed seven whole-history scans into one,
/// and a faster query that answers differently is not a fix. Every
/// predicate here is one the conditional aggregates had to reproduce —
/// nulls in `duration` and `quality_score`, a skip that must not count, and
/// listens either side of both the day and the week boundary.
#[tokio::test]
async fn the_dashboard_folds_its_scans_without_changing_the_answers() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let tz = chrono_tz::Europe::Berlin;
    let now = Database::now_secs();
    let today_start = crate::analytics::local_day_start(now, tz);
    let week_start = crate::analytics::local_week_start(now, tz, 1);

    // A history spanning both boundaries, with the awkward rows in it: a
    // listen with no duration, one with no quality score, and a skip.
    let rows: [(&str, &str, i64, Option<i64>, Option<i64>, bool); 6] = [
        ("Aphex Twin", "Xtal", today_start + 60, Some(300), Some(90), false),
        ("Aphex Twin", "Xtal", today_start + 120, None, Some(70), false),
        ("Burial", "Archangel", week_start + 60, Some(240), None, false),
        ("Burial", "Archangel", week_start - 86_400, Some(240), Some(50), false),
        ("Massive Attack", "Teardrop", now - 400 * 86_400, Some(330), Some(60), false),
        // Skipped: excluded from every one of these figures.
        ("Massive Attack", "Angel", today_start + 300, Some(370), Some(99), true),
    ];
    for (i, (artist, title, ts, dur, q, skipped)) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO scrobbles
                   (user_id, title, artist, album, timestamp, duration, source_id, source_name,
                    status, skipped, quality_score, is_lossless, codec)
                 VALUES (?, ?, ?, 'an album', ?, ?, ?, 'test', 'synced', ?, ?, ?, ?)",
        )
        .bind(uid)
        .bind(title)
        .bind(artist)
        .bind(ts)
        .bind(dur)
        .bind(format!("s{i}"))
        .bind(skipped)
        .bind(q)
        .bind(i % 2 == 0)
        .bind(if i % 3 == 0 { "flac" } else { "" })
        .execute(&db.pool)
        .await
        .unwrap();
    }

    let got = db.get_dashboard_stats(uid, tz, 1).await.unwrap();

    // The originals, run verbatim.
    let one = |sql: &'static str| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_as::<_, (Option<i64>,)>(sql)
                .bind(uid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .0
                .unwrap_or(0)
        }
    };
    let ranged = |sql: &'static str, floor: i64| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_as::<_, (Option<i64>,)>(sql)
                .bind(uid)
                .bind(floor)
                .fetch_one(&pool)
                .await
                .unwrap()
                .0
                .unwrap_or(0)
        }
    };

    assert_eq!(
        got["total"].as_i64().unwrap(),
        one("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE").await
    );
    assert_eq!(
        got["fidelity_known"].as_i64().unwrap(),
        one(concat!(
            "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE AND ",
            "(COALESCE(format_type, '') = 'dsd' OR is_lossless IS NOT NULL ",
            "OR COALESCE(codec, '') != '')"
        ))
        .await
    );
    assert_eq!(
        got["today"].as_i64().unwrap(),
        ranged(
            "SELECT COUNT(*) FROM scrobbles
                  WHERE user_id = ? AND skipped = FALSE AND timestamp >= ?",
            today_start
        )
        .await
    );
    assert_eq!(
        got["this_week"].as_i64().unwrap(),
        ranged(
            "SELECT COUNT(*) FROM scrobbles
                  WHERE user_id = ? AND skipped = FALSE AND timestamp >= ?",
            week_start
        )
        .await
    );

    let secs = one("SELECT SUM(duration) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND duration IS NOT NULL")
    .await;
    assert_eq!(got["total_hours"].as_f64().unwrap(), (secs as f64 / 3600.0 * 10.0).round() / 10.0);
    let today_secs = ranged(
        "SELECT SUM(duration) FROM scrobbles
              WHERE user_id = ? AND skipped = FALSE AND timestamp >= ? AND duration IS NOT NULL",
        today_start,
    )
    .await;
    assert_eq!(
        got["today_hours"].as_f64().unwrap(),
        (today_secs as f64 / 3600.0 * 10.0).round() / 10.0
    );

    // The skip is the one that is easiest to lose in a rewrite: it is the
    // busiest listen in the fixture on every axis.
    assert_eq!(got["total"].as_i64().unwrap(), 5, "the skip counts nowhere");
    // Derived from the fixture rather than hardcoded, because **on a Monday
    // `week_start` *is* `today_start`** and the "earlier this week" row
    // falls inside today — so a literal `2` was right six days a week and
    // failed on the seventh. Found by running the suite on a Monday, which
    // is the only way it ever announces itself.
    //
    // It still asserts the thing it was written for: the filter excludes
    // the skip, so a rewrite that started counting skipped listens in
    // "today" fails here whatever the day.
    let today_expected =
        rows.iter().filter(|(_, _, ts, _, _, skipped)| !skipped && *ts >= today_start).count();
    assert_eq!(got["today"].as_i64().unwrap(), today_expected as i64);
    assert!(got["avg_quality"].as_f64().unwrap() > 0.0, "nulls are skipped, not zeroed");

    // Dropped deliberately: three `COUNT(DISTINCT)` scans that nothing
    // rendered and that Statistics already shows against a range control.
    for gone in ["unique_artists", "unique_albums", "unique_tracks"] {
        assert!(got.get(gone).is_none(), "{gone} should not be computed any more");
    }
}

/// And it does it in one pass, which the equivalence test above cannot see.
///
/// The plan is the hardware-independent evidence — this project's own rule,
/// after v0.58.0 found a regression a laptop clock could not. Splitting
/// these back into separate queries would keep every number correct and
/// quietly restore seven whole-history scans to the busiest page in the
/// application, which is how v0.70.1 became an outage rather than a slow
/// screen.
#[tokio::test]
async fn the_dashboard_reads_the_history_once_and_builds_no_temp_btree() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let whole = format!(
        "SELECT COUNT(*),
                    SUM(CASE WHEN is_lossless = TRUE THEN 1 ELSE 0 END),
                    SUM(CASE WHEN {FIDELITY_KNOWN} THEN 1 ELSE 0 END),
                    SUM(duration),
                    AVG(quality_score)
               FROM scrobbles WHERE user_id = ? AND skipped = FALSE"
    );
    let recent = "SELECT SUM(CASE WHEN timestamp >= ?2 THEN 1 ELSE 0 END),
                             SUM(CASE WHEN timestamp >= ?3 THEN duration END)
                        FROM scrobbles
                       WHERE user_id = ?1 AND skipped = FALSE AND timestamp >= ?4"
        .to_string();

    for (name, sql, binds) in [("whole history", whole, 1usize), ("today and this week", recent, 4)]
    {
        let explain = format!("EXPLAIN QUERY PLAN {sql}");
        let mut q = sqlx::query_as::<_, (i64, i64, i64, String)>(&explain);
        for _ in 0..binds {
            q = q.bind(uid);
        }
        let plan: Vec<String> =
            q.fetch_all(&db.pool).await.unwrap().into_iter().map(|r| r.3).collect();

        // A `SEARCH … USING INDEX` is the wanted shape; a bare `SCAN` is
        // not, and neither is a sort structure built over every row.
        assert!(
            !plan.iter().any(|l| l.contains("TEMP B-TREE")),
            "the {name} read builds a temp B-tree — that is what the three dropped \
                 COUNT(DISTINCT) queries did, over the whole history, per dashboard load. \
                 Plan:\n{}",
            plan.join("\n")
        );
        assert!(
            plan.iter().any(|l| l.contains("USING INDEX") || l.contains("USING COVERING")),
            "the {name} read is not using an index at all. Plan:\n{}",
            plan.join("\n")
        );
    }
}

/// Authenticating does not write to the database every time.
///
/// It used to. `validate_session` stamped `last_active_at` on **every**
/// authenticated request, and with the deck polling every three seconds
/// that is twenty writes a minute per open tab — each taking SQLite's write
/// lock, and each turning a read request into a write one. Thirty-three of
/// them queued behind the stuck query in the v0.70.1 incident, recorded
/// there as a symptom when they were also an amplifier.
///
/// Asserted through `validate_session` itself rather than on the constant,
/// because what matters is that repeated authentication leaves the stamp
/// alone — and that it still moves once the interval has passed, or this
/// would be "we stopped recording it" wearing a performance fix's clothes.
#[tokio::test]
async fn authenticating_does_not_write_on_every_request() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let token = db.create_web_session(uid).await.unwrap();

    let stamp = || {
        let pool = db.pool.clone();
        async move {
            sqlx::query_as::<_, (i64,)>("SELECT last_active_at FROM web_sessions")
                .fetch_one(&pool)
                .await
                .unwrap()
                .0
        }
    };

    // Backdate it so the first call is due, then let it write.
    let before = Database::now_secs() - Database::ACTIVITY_STAMP_INTERVAL_SECS - 1;
    sqlx::query("UPDATE web_sessions SET last_active_at = ?")
        .bind(before)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(db.validate_session(&token).await.unwrap().is_some());
    let after_due = stamp().await;
    assert!(after_due > before, "a stamp older than the interval must still be refreshed");

    // Now the half that matters, and it needs a *distinguishable* fresh
    // stamp: setting it to `now` would be rewritten as `now`, so writing
    // every time and not writing at all look identical inside one test run.
    // Five seconds ago is fresh by any reading of the interval and is not a
    // value an unconditional write could produce.
    let fresh = Database::now_secs() - 5;
    sqlx::query("UPDATE web_sessions SET last_active_at = ?")
        .bind(fresh)
        .execute(&db.pool)
        .await
        .unwrap();

    // Twenty requests — a minute of the deck polling one tab.
    for _ in 0..20 {
        assert!(db.validate_session(&token).await.unwrap().is_some());
    }
    assert_eq!(
        stamp().await,
        fresh,
        "a fresh stamp was rewritten — that is a write, and the write lock, on every \
             authenticated request"
    );
}

#[tokio::test]
async fn a_grouped_cover_comes_from_one_listen() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let now = Database::now_secs() as u64;

    const ALBUM: &str = "en cas de tempête, ce jardin sera fermé.";
    const RIGHT_MBID: &str = "9e5838c1-f4ef-4948-95a0-b201d64f33cb";
    const RIGHT_CAA: i64 = 44_286_336_686;
    const COMP_MBID: &str = "ead839e9-66cb-4ecf-a5e0-bc3a1a0691e2";
    const COMP_CAA: i64 = 32_013_642_354;

    // The compilation-tagged listen is older; the album's own is newest.
    for (title, ts, mbid, caa) in [
        ("Dans la nuit (feat. Loud)", now - 5_000, COMP_MBID, COMP_CAA),
        ("Combustible", now - 100, RIGHT_MBID, RIGHT_CAA),
    ] {
        let mut p = play(title, "Cœur de pirate", ts);
        p.album = Some(ALBUM.into());
        p.caa_id = Some(caa);
        p.caa_release_mbid = Some(mbid.into());
        let source_id = p.source_id.clone();
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        // `artwork_url` is written by the backfill rather than at insert,
        // so it is set here the way the chain would.
        sqlx::query("UPDATE scrobbles SET artwork_url = ? WHERE user_id = ? AND source_id = ?")
            .bind(format!("https://coverartarchive.org/release/{mbid}/{caa}-250.jpg"))
            .bind(uid)
            .bind(&source_id)
            .execute(&db.pool)
            .await
            .unwrap();
    }

    let rows = db.chart(uid, "albums", None, 10, false).await.unwrap();
    let row = rows.iter().find(|r| r.name == ALBUM).expect("the album charted");

    assert_eq!(row.plays, 2, "both listens still count — a cover fix, not a filter");
    assert_eq!(
        row.caa_release_mbid.as_deref(),
        Some(RIGHT_MBID),
        "the release must be the newest listen's, not the lexicographic max"
    );
    assert_eq!(row.caa_id, Some(RIGHT_CAA));
    assert_eq!(
        row.artwork_url.as_deref(),
        Some(
            format!("https://coverartarchive.org/release/{RIGHT_MBID}/{RIGHT_CAA}-250.jpg")
                .as_str()
        ),
        "and all three have to name the same release"
    );

    // The old query really was incoherent on this exact data — asserted so
    // the test cannot start passing for the wrong reason if the fixture
    // drifts to values where three independent maxes happen to agree.
    let (old_url, old_caa, old_mbid): (Option<String>, Option<i64>, Option<String>) =
        sqlx::query_as(
            "SELECT MAX(artwork_url), MAX(caa_id), MAX(caa_release_mbid)
                   FROM scrobbles WHERE user_id = ? AND album = ?",
        )
        .bind(uid)
        .bind(ALBUM)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(old_caa, Some(RIGHT_CAA), "the old MAX picked the album's id…");
    assert_eq!(old_mbid.as_deref(), Some(COMP_MBID), "…the compilation's release…");
    assert!(old_url.unwrap().contains(COMP_MBID), "…and the compilation's cover");

    // The album page is the surface that was always right. The point of the
    // fix is that the two now agree, not merely that the chart moved.
    let (page_url, page_caa, page_mbid) =
        db.album_artwork(uid, "Cœur de pirate", ALBUM).await.unwrap().expect("a cover");
    assert_eq!(page_mbid.as_deref(), row.caa_release_mbid.as_deref());
    assert_eq!(page_caa, row.caa_id);
    assert_eq!(page_url.as_deref(), row.artwork_url.as_deref());
}

/// A listen with no cover must not blank a group that has one.
///
/// The naive "take the newest row" would: an import carries no artwork at
/// all, and one arriving today would drop the sleeve off a record played
/// for years. The pick only ever considers rows that actually have
/// artwork — the same semantics `album_artwork` gets from its `WHERE`.
#[tokio::test]
async fn a_coverless_listen_does_not_blank_the_group() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let now = Database::now_secs() as u64;

    for (title, ts, art) in [
        ("Archangel", now - 9_000, true),
        // Newer, and carrying nothing — the shape an import lands in.
        ("Near Dark", now - 10, false),
    ] {
        let mut p = play(title, "Burial", ts);
        p.album = Some("Untrue".into());
        if art {
            p.caa_id = Some(123_456);
            p.caa_release_mbid = Some("11111111-1111-1111-1111-111111111111".into());
        }
        let source_id = p.source_id.clone();
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        if art {
            sqlx::query("UPDATE scrobbles SET artwork_url = ? WHERE user_id = ? AND source_id = ?")
                .bind("https://example.invalid/untrue.jpg")
                .bind(uid)
                .bind(&source_id)
                .execute(&db.pool)
                .await
                .unwrap();
        }
    }

    let rows = db.chart(uid, "albums", None, 10, false).await.unwrap();
    let row = rows.iter().find(|r| r.name == "Untrue").expect("charted");
    assert_eq!(row.plays, 2);
    assert_eq!(row.artwork_url.as_deref(), Some("https://example.invalid/untrue.jpg"));
    assert_eq!(row.caa_id, Some(123_456));
}

/// The one unauthenticated route on the instance must not become a
/// membership oracle.
///
/// A username that does not exist and one that has not published have to be
/// indistinguishable, or a stranger can type names at a private,
/// invite-only server and learn who has an account on it. Phrased about the
/// *answer* rather than about the SQL, because a later rewrite that splits
/// the two cases apart would be a correct-looking query and a leak.
#[tokio::test]
async fn an_unknown_user_and_an_unpublished_one_are_the_same_answer() {
    let db = testdb().await;
    let quiet = db.create_user("quiet", None, "pw", "user").await.unwrap();
    let loud = db.create_user("loud", None, "pw", "user").await.unwrap();

    // Nobody publishes until they say so — including accounts that predate
    // the column, which is what the `NOT NULL DEFAULT 0` is for.
    assert!(db.public_np_target("quiet").await.unwrap().is_none());
    assert!(
        db.public_np_target("nobody-here").await.unwrap().is_none(),
        "an unknown username answers exactly as an unpublished one does"
    );

    db.set_public_np_settings(loud, true, PublicNpFields::from_names(["position"])).await.unwrap();
    let (id, _, fields) = db.public_np_target("LOUD").await.unwrap().expect("published");
    assert_eq!(id, loud, "matched case-insensitively, like every other username lookup");
    assert!(fields.position);
    assert!(!fields.artwork, "one tick grants one field");

    // Turning it off is immediate and total, and it does not have to also
    // clear the ticks: the master switch is what the public route reads.
    db.set_public_np_settings(loud, false, fields).await.unwrap();
    assert!(db.public_np_target("loud").await.unwrap().is_none());

    // A disabled account never publishes, whatever its own switch says —
    // an admin turning someone off must not leave a public page running.
    db.set_public_np_settings(quiet, true, PublicNpFields::default()).await.unwrap();
    assert!(db.public_np_target("quiet").await.unwrap().is_some());
    sqlx::query("UPDATE users SET disabled = 1 WHERE id = ?")
        .bind(quiet)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(db.public_np_target("quiet").await.unwrap().is_none());
}

/// The album page's "whole record" button must not scan the whole history.
///
/// `album_release_mbid` folds both names in its `WHERE`, which no ordinary
/// index can serve — so before `idx_scrobbles_album_release` the plan was a
/// full pass over every listen the user has. On a Pi with the database on
/// an SD card the first click ran long enough for the tunnel to answer
/// Cloudflare's own 502, while the second, with the pages warm, was fine.
///
/// Asserted on the **plan**, not on a timing, because the timing is a fact
/// about the machine running the test and the plan is not — the rule
/// v0.58.0 arrived at after a laptop failed to reproduce a Pi regression.
/// Nothing about the answer changes when the index is dropped, so a test
/// phrased only about the result would keep passing while the endpoint went
/// back to being unusable.
#[tokio::test]
async fn the_whole_record_lookup_seeks_instead_of_scanning() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let now = Database::now_secs() as u64;

    // Three listens claiming one release and two claiming another, so the
    // answer is a plurality rather than whatever happens to come first.
    for (i, mbid) in ["rel-a", "rel-a", "rel-a", "rel-b", "rel-b"].iter().enumerate() {
        let mut p = play("Archangel", "Burial", now - 1000 + i as u64);
        p.album = Some("Untrue".into());
        p.mbid_release = Some((*mbid).into());
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    // A listen with no release MBID at all is outside the partial index,
    // and must not become the answer.
    let mut bare = play("Archangel", "Burial", now - 10);
    bare.album = Some("Untrue".into());
    db.save_scrobble(uid, &bare, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap();

    // The spelling is folded, so the caller's casing is not the stored one.
    let got = db.album_release_mbid(uid, "  burial ", "UNTRUE").await.unwrap();
    assert_eq!(got.as_deref(), Some("rel-a"), "the plurality release wins");

    async fn plan(db: &Database, sql: &str) -> String {
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {sql}")).fetch_all(&db.pool).await.unwrap();
        rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join(" | ")
    }

    // The fix itself: a **seek**, not a scan. Deliberately not asserting on
    // the word "COVERING" — SQLite 3.51 says it and the 3.44 bundled into
    // the binary does not, and the difference is 41 steps against 37. What
    // must never come back is `SCAN scrobbles`.
    let p = plan(
        &db,
        "SELECT mbid_release FROM scrobbles
              WHERE user_id = 1 AND LOWER(TRIM(artist)) = LOWER(TRIM('Burial'))
                AND LOWER(TRIM(album)) = LOWER(TRIM('Untrue'))
                AND mbid_release IS NOT NULL AND TRIM(mbid_release) != ''
              GROUP BY mbid_release ORDER BY COUNT(*) DESC LIMIT 1",
    )
    .await;
    assert!(
        p.contains("SEARCH scrobbles USING") && p.contains("idx_scrobbles_album_release"),
        "the whole-record lookup should seek idx_scrobbles_album_release, got: {p}"
    );
    assert!(!p.contains("SCAN scrobbles"), "it must not scan the history, got: {p}");

    // And the half that keeps it from becoming v0.59.0 all over again: the
    // partial predicate must keep this index away from the whole-history
    // aggregations, which want a sequential scan on slow storage. If the
    // `WHERE mbid_release IS NOT NULL` is ever dropped, the planner takes
    // it for these too and Statistics gets slower on the hardware that
    // actually runs this.
    for agg in [
        "SELECT LOWER(TRIM(artist)), COUNT(*) FROM scrobbles
              WHERE user_id = 1 AND skipped = FALSE GROUP BY 1 ORDER BY 2 DESC LIMIT 50",
        "SELECT LOWER(TRIM(artist)), LOWER(TRIM(album)), COUNT(*) FROM scrobbles
              WHERE user_id = 1 AND skipped = FALSE GROUP BY 1, 2 ORDER BY 3 DESC LIMIT 50",
    ] {
        let p = plan(&db, agg).await;
        assert!(
            !p.contains("idx_scrobbles_album_release"),
            "an aggregation must not be lured onto the partial index, got: {p}"
        );
    }
}

/// A skip reports what bracketed it, which is the whole point of the row.
///
/// Bailing out is a judgement about a track *in a sequence*, so the
/// neighbours are the only context this application can honestly record —
/// and they come from one bounded range scan rather than a subquery per
/// row, which is the quadratic shape recorded three times in this file.
#[tokio::test]
async fn a_skip_carries_what_played_either_side_of_it() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let base = Database::now_secs() as u64 - 10_000;

    for (title, offset, skipped) in
        [("Archangel", 0u64, false), ("Near Dark", 300, true), ("Endorphin", 600, false)]
    {
        let mut p = play(title, "Burial", base + offset);
        p.duration = Some(240);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped },
        )
        .await
        .unwrap();
        if skipped {
            let report = PlaybackReport { listened_ms: Some(30_000), ..Default::default() };
            db.set_listen_playback(uid, &p.source_id, &p.source_name, &report).await.unwrap();
        }
    }

    let (total, rows) = db.skips(uid, 25, 0).await.unwrap();
    assert_eq!(total, 1, "only the skip is in here");
    assert_eq!(rows.len(), 1);
    let s = &rows[0];
    assert_eq!(s.title, "Near Dark");
    assert_eq!(s.listened_ms, Some(30_000));
    assert_eq!(
        s.before.as_ref().map(|(t, _)| t.as_str()),
        Some("Archangel"),
        "the track it did not sit after"
    );
    assert_eq!(
        s.after.as_ref().map(|(t, _)| t.as_str()),
        Some("Endorphin"),
        "and the one that followed it"
    );

    // The whole-history question a page of recent skips cannot answer.
    let most = db.most_skipped(uid, 10).await.unwrap();
    assert_eq!(most.len(), 1);
    assert_eq!(most[0].0, "Near Dark");
    assert_eq!(most[0].2, 1);
}

/// A skip at the edge of a sitting has no neighbour, and says so.
///
/// `None` rather than reaching further for something unrelated — a track
/// from yesterday is not what this one failed to sit between.
#[tokio::test]
async fn a_skip_with_nothing_around_it_reports_no_context() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let mut p = play("Near Dark", "Burial", Database::now_secs() as u64 - 100);
    p.duration = Some(240);
    db.save_scrobble(
        uid,
        &p,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition { imported: false, skipped: true },
    )
    .await
    .unwrap();

    let (_, rows) = db.skips(uid, 25, 0).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].before.is_none());
    assert!(rows[0].after.is_none());
    assert!(rows[0].listened_ms.is_none(), "nothing measured a playhead here");
}

/// A skip written by a live source is stored, measured, and never sent.
///
/// Skips used to arrive only from imports and the shelf; Plex, Jellyfin,
/// Emby, Roon and a submitting client all write them now, which makes the
/// three properties below load-bearing rather than theoretical. The one
/// that would be worst to get wrong is the forwarding: a skip pushed to
/// Last.fm is a listen on someone's permanent public record that never
/// happened.
#[tokio::test]
async fn a_live_skip_is_measured_stored_and_never_forwarded() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let now = Database::now_secs();

    let mut p = play("Archangel", "Burial", (now - 60) as u64);
    p.duration = Some(240);
    assert!(db
        .save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped: true },
        )
        .await
        .unwrap());

    // How far in they got — the half a bool could never express.
    let report = PlaybackReport { listened_ms: Some(12_000), ..Default::default() };
    db.set_listen_playback(uid, &p.source_id, &p.source_name, &report).await.unwrap();

    let stored: ScrobbleRecord =
        sqlx::query_as("SELECT * FROM scrobbles WHERE user_id = ? AND title = ?")
            .bind(uid)
            .bind("Archangel")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    // `ScrobbleRecord` does not carry the flag, so ask the column.
    let (is_skipped,): (bool,) = sqlx::query_as("SELECT skipped FROM scrobbles WHERE id = ?")
        .bind(stored.id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(is_skipped, "stored as a skip");
    assert_eq!(stored.listened_ms, Some(12_000), "and it says how far in");
    assert_eq!(stored.status, "imported", "never queued for a sink");
    assert!(stored.session_id.is_none(), "a skip is not part of a sitting");

    // The flush loop only ever picks up `'pending'`, so this is the
    // property that keeps it off a permanent public record.
    let pending = db.get_pending_scrobbles().await.unwrap();
    assert!(!pending.iter().any(|s| s.play.title == "Archangel"), "a skip must never be forwarded");

    // And it still does not count.
    assert_eq!(db.listen_count_and_latest(uid).await.unwrap().0, 0);
}

/// A skip must not reach *any* readout that counts listens.
///
/// This class of bug has now been found three times — the language
/// breakdown, the dashboard's top artist, and the artwork coverage
/// numerator — each time because a new query was written next to filtered
/// ones and quietly omitted the filter. Nothing errors; a number is simply
/// wrong. So this asserts on the *readouts* rather than on the SQL, which
/// A misspelled key must not be able to wipe an attribution.
///
/// This is the fourth place the same shape has come up (`bulk-chain`,
/// `default_chain_id`, `unify_album_release`) and the reason is always the
/// same: a bare `Option` reads *absent* and *null* as the same thing, so a
/// typo in a body reads as "clear this" and answers 200. On the shelf that
/// means editing a purchase date silently unassigns the turntable.
#[tokio::test]
async fn absent_and_null_are_different_things_on_a_shelf_edit() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let gear =
        db.upsert_equipment(uid, "SL-1200", "turntable", None, None, 0.0, None).await.unwrap();
    let id = db
        .create_physical(
            uid,
            "vinyl",
            "Burial",
            "Untrue",
            Some(2007),
            None,
            None,
            None,
            None,
            None,
            "[]",
            None,
            Some(1_700_000_000),
            None,
            None,
        )
        .await
        .unwrap();

    db.update_physical(uid, id, None, None, Some(Some(gear)), None, None, None, None, None, None)
        .await
        .unwrap();
    let r = db.get_physical(uid, id).await.unwrap().unwrap();
    assert_eq!(r.equipment_id, Some(gear));
    assert_eq!(r.purchased_at, Some(1_700_000_000));

    // Editing one field leaves the others alone. This is the assertion a
    // bare `Option` fails.
    db.update_physical(
        uid,
        id,
        Some(Some(1_800_000_000)),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let r = db.get_physical(uid, id).await.unwrap().unwrap();
    assert_eq!(r.purchased_at, Some(1_800_000_000));
    assert_eq!(r.equipment_id, Some(gear), "an untouched field must survive an edit");

    // An explicit null still clears — the picker's "— no deck —" needs it,
    // which is why the distinction has to survive deserialisation rather
    // than being designed away.
    db.update_physical(uid, id, None, None, Some(None), None, None, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(db.get_physical(uid, id).await.unwrap().unwrap().equipment_id, None);
}

/// Every track off one record shows that record's sleeve.
///
/// A side used to go in with no artwork, so `backfill_artwork` resolved
/// each listen the way it resolves any other — by asking a provider about
/// (artist, title, album) — and a provider answers per *song*. Eleven
/// tracks off one side of *The Wall* came back with eleven different
/// covers, several of them compilations. The shelf is holding the object;
/// there is nothing to look up.
///
/// Asserted on the listens rather than on the update, because the failure
/// is one cover per track and only a readout can see that.
#[tokio::test]
async fn every_track_off_one_record_wears_that_record_s_sleeve() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let id = db
        .create_physical(
            uid,
            "vinyl",
            "Pink Floyd",
            "The Wall",
            Some(1979),
            None,
            None,
            None,
            None,
            Some("https://example.test/the-wall.jpg"),
            "[]",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    // Four tracks off one side, each having been given a different cover by
    // a provider — which is exactly the reported state.
    for (i, wrong) in ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].iter().enumerate() {
        let mut p = play(&format!("Track {i}"), "Pink Floyd", 1_760_000_000 + i as u64);
        p.album = Some("The Wall".into());
        p.source_id = format!("physical-{id}-A-1760000000-{}", i + 1);
        p.source_name = "physical".into();
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE scrobbles SET artwork_url = ? WHERE user_id = ? AND source_id = ?")
            .bind(format!("https://provider.test/{wrong}"))
            .bind(uid)
            .bind(&p.source_id)
            .execute(&db.pool)
            .await
            .unwrap();
    }

    let cover = db.physical_cover(uid, id).await.unwrap().expect("the pressing has a sleeve");
    assert_eq!(cover, "https://example.test/the-wall.jpg");
    let n = db.set_physical_artwork(uid, id, &cover).await.unwrap();
    assert_eq!(n, 4, "every listen of the record, not only the newest side");

    let covers: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT artwork_url FROM scrobbles WHERE user_id = ? AND source_name = 'physical'",
    )
    .bind(uid)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(covers.len(), 4);
    assert!(
        covers.iter().all(|c| c.as_deref() == Some(cover.as_str())),
        "one record, one cover — got {covers:?}"
    );

    // Idempotent: nothing to change is not a write.
    assert_eq!(db.set_physical_artwork(uid, id, &cover).await.unwrap(), 0);

    // And a listen from anywhere else is not touched.
    let mut other = play("Elsewhere", "Pink Floyd", 1_760_009_999);
    other.source_id = "plex-99-1760009999".into();
    other.source_name = "Plex".into();
    db.save_scrobble(
        uid,
        &other,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    assert_eq!(db.set_physical_artwork(uid, id, &cover).await.unwrap(), 0);
}
/// A side's plays and its hours are derived from the plays themselves, so
/// they cannot drift from them and un-recording a side takes its wear back.
///
/// The zero case is the one worth pinning: a side nobody has turned over
/// is *absent* from this, not a row of zeros, because the tracklist is what
/// knows a side exists. The caller fills those in — and if this ever starts
/// zero-filling on its own, the page would draw every side twice.
#[tokio::test]
async fn a_side_reports_the_plays_it_actually_had() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let id = db
        .create_physical(
            uid,
            "vinyl",
            "Pink Floyd",
            "The Dark Side of the Moon",
            Some(1973),
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
        .unwrap();

    // Side A twice, side B once, and a half side — started partway, so it
    // is charged for what played rather than for the whole side.
    db.record_physical_play(uid, id, "A", 1_700_000_000, 1_200, 5, None, None).await.unwrap();
    db.record_physical_play(uid, id, "A", 1_700_100_000, 1_200, 5, None, None).await.unwrap();
    db.record_physical_play(uid, id, "B", 1_700_200_000, 900, 4, None, None).await.unwrap();
    let half =
        db.record_physical_play(uid, id, "B", 1_700_300_000, 400, 2, None, None).await.unwrap();

    let sides = db.side_plays(uid, id).await.unwrap();
    assert_eq!(sides.len(), 2, "only sides that were played, and one row each");
    assert_eq!(sides[0].side, "A");
    assert_eq!((sides[0].plays, sides[0].secs), (2, 2_400));
    assert_eq!(sides[0].last_played, Some(1_700_100_000));
    assert_eq!(sides[1].side, "B");
    assert_eq!(
        (sides[1].plays, sides[1].secs),
        (2, 1_300),
        "a side started partway counts the part that played"
    );

    // Derived, not accumulated: removing a play takes its time back. A
    // counter could not do this, which is the whole reason for the rule.
    sqlx::query("DELETE FROM physical_plays WHERE id = ?")
        .bind(half)
        .execute(&db.pool)
        .await
        .unwrap();
    let sides = db.side_plays(uid, id).await.unwrap();
    assert_eq!((sides[1].plays, sides[1].secs), (1, 900));
    assert_eq!(sides[1].last_played, Some(1_700_200_000));

    // Somebody else's shelf is not in here.
    let other = db.create_user("sam", None, "pw", "user").await.unwrap();
    assert!(db.side_plays(other, id).await.unwrap().is_empty());
}

/// Re-fetching a sleeve must update what is there, not stack a second copy
/// of all nineteen booklet pages on top of the first. The archive's own
/// image id is the key that makes that true.
#[tokio::test]
async fn refetching_a_sleeve_updates_rather_than_duplicates() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let id = db
        .create_physical(
            uid,
            "cd",
            "Ólafur Arnalds",
            "re:member",
            None,
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
        .unwrap();

    for pass in 0..2 {
        for (n, kind) in [(0, "front"), (1, "booklet"), (2, "booklet")] {
            db.add_physical_image(
                uid,
                id,
                kind,
                "caa",
                Some(&format!("https://coverartarchive.org/x/{n}.jpg")),
                None,
                None,
                None,
                None,
                Some(&format!("page {}", n + pass)),
                n,
                Some(&format!("{n}")),
            )
            .await
            .unwrap();
        }
    }
    let images = db.list_physical_images(uid, id).await.unwrap();
    assert_eq!(images.len(), 3, "a second fetch must not duplicate the sleeve");
    // And it is an update, not a no-op: an editor's corrected caption should
    // land rather than being discarded as "already have it".
    assert_eq!(images[0].caption.as_deref(), Some("page 1"));

    // An upload carries no provider id, so two scans of the same page are
    // two scans — the partial unique index is `WHERE remote_id IS NOT NULL`
    // for exactly this reason.
    for _ in 0..2 {
        db.add_physical_image(
            uid,
            id,
            "booklet",
            "upload",
            None,
            None,
            Some("shelf/1/1/a.jpg"),
            Some("image/jpeg"),
            Some(10),
            None,
            9,
            None,
        )
        .await
        .unwrap();
    }
    assert_eq!(db.list_physical_images(uid, id).await.unwrap().len(), 5);

    // The count the shelf card reads comes off the same rows.
    assert_eq!(db.get_physical(uid, id).await.unwrap().unwrap().image_count, 5);

    // And deleting the pressing hands back the files it was holding, which
    // is the only thing that knows they exist.
    let files = db.delete_physical(uid, id).await.unwrap().unwrap();
    assert_eq!(files, vec!["shelf/1/1/a.jpg".to_string(), "shelf/1/1/a.jpg".to_string()]);
    assert!(db.list_physical_images(uid, id).await.unwrap().is_empty());
}

/// The SQL guard and the Rust one cannot drift.
///
/// Two copies of the same list is exactly how `fuzzy_exists` and
/// `is_various_artists` would come to disagree about which listens are
/// duplicates, so the fragments are checked against the constant they are
/// written from — including that they list *all* of it, which is what a
/// later addition to `VARIOUS_ARTISTS_KEYS` would otherwise silently miss.
#[tokio::test]
async fn the_sql_guard_lists_exactly_the_names_the_rust_one_knows() {
    for frag in [NOT_VARIOUS, IS_VARIOUS] {
        let inner = frag.split_once('(').unwrap().1.trim_end_matches(')');
        let listed: Vec<&str> = inner.split(',').map(|k| k.trim().trim_matches('\'')).collect();
        assert_eq!(listed, VARIOUS_ARTISTS_KEYS, "{frag} has drifted from the constant");
    }
    // Inlined as literals rather than bound, so a key carrying a quote
    // would end the string and change the statement.
    for k in VARIOUS_ARTISTS_KEYS {
        assert!(!k.contains('\''), "{k:?} cannot be inlined into SQL");
        assert_eq!(*k, k.trim().to_lowercase(), "{k:?} must already be folded");
    }
}

/// "Various Artists" is a filing convention, not a performer — so it
/// reaches no readout shaped like an artist, anywhere.
///
/// Asserted on the **readouts** rather than on the SQL, because that is the
/// only form that catches a query written later. It was third on Top Charts
/// with 170 plays when this was reported, and it had been excluded from the
/// Reports superlative since v0.57.0 — one screen fixed at a time is how it
/// survived on the others.
///
/// The second half is the half that keeps this honest: the plays are real
/// listening and **still count**. Dropping them from the totals would be
/// the paired-counts trap in reverse — a play total that silently omits
/// part of the history is worse than a chart row nobody wanted.
#[tokio::test]
async fn the_placeholder_reaches_no_artist_shaped_readout() {
    let db = testdb().await;
    // Two decks from the start, so the Patch readouts below have somebody
    // to be about without a second "ash".
    let (uid, them, chain) = patched_pair(&db).await;
    let now = Database::now_secs();

    // Enough compilation plays to win every artist ranking outright, and
    // one real artist to prove the readouts are not simply empty.
    for i in 0..12 {
        let mut p = play(&format!("Comp {i}"), "Various Artists", (now - 100 - i * 60) as u64);
        p.duration = Some(200);
        p.source_id = format!("va-{i}");
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }
    for i in 0..3 {
        let mut p = play(&format!("Song {i}"), "Burial", (now - 2000 - i * 60) as u64);
        p.duration = Some(200);
        p.source_id = format!("real-{i}");
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    let names = |rows: Vec<ChartRow>| -> Vec<String> { rows.into_iter().map(|r| r.name).collect() };
    let va = |v: &[String]| v.iter().any(|n| is_various_artists(n));

    // ── Top Charts ──
    let chart = names(db.chart(uid, "artists", None, 50, false).await.unwrap());
    assert!(!va(&chart), "Top Charts: {chart:?}");
    assert!(chart.iter().any(|n| n == "Burial"), "a real artist still charts");

    // ── Statistics ──
    let ranged = db.get_ranged_stats(uid, None, false).await.unwrap();
    let top: Vec<String> = ranged["top_artists"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str().map(str::to_string))
        .collect();
    assert!(!va(&top), "Statistics: {top:?}");
    assert!(top.iter().any(|n| n == "Burial"), "a real artist still ranks");

    // ── Dashboard ──
    let dash = db.get_dashboard_stats(uid, chrono_tz::UTC, 1).await.unwrap();
    let dash_top = dash["top_artist"].as_str().map(str::to_string);
    assert!(!dash_top.as_deref().is_some_and(is_various_artists), "Dashboard: {dash_top:?}");

    // ── Search ──
    let (found, _, _) = db.search_library(uid, "various", 20).await.unwrap();
    let found: Vec<String> = found.into_iter().map(|a| a.0).collect();
    assert!(!va(&found), "Search: {found:?}");

    // ── The portrait backfill, which would spend a lookup on it ──
    let wanted = db.artists_without_images(uid, 20).await.unwrap();
    assert!(!va(&wanted), "portrait queue: {wanted:?}");

    // ── And the plays are still every bit as real ──
    assert_eq!(db.listen_count_and_latest(uid).await.unwrap().0, 15, "every play counts");
    assert_eq!(dash["total"], 15, "the dashboard total is the whole history");

    // ── Patch: another deck's artists, and who you both play ──
    //
    // Same fragment as the four above, but asserted separately because
    // "one screen fixed at a time" is exactly how the placeholder survived
    // on Top Charts after the Reports superlative had already dropped it.
    let me = uid;
    for i in 0..6 {
        listen(&db, them, Some(chain), &format!("Comp {i}"), "Various Artists", 1_760_000_100 + i)
            .await;
    }
    listen(&db, them, Some(chain), "Archangel", "Burial", 1_760_000_300).await;
    // The viewer plays the placeholder too, so an overlap query with no
    // guard would report it as something they have in common.
    listen(&db, me, Some(chain), "Mine", "Various Artists", 1_760_000_400).await;
    listen(&db, me, Some(chain), "Shell", "Burial", 1_760_000_500).await;

    let tab: Vec<String> =
        db.patch_artists(me, 50, None).await.unwrap().into_iter().map(|a| a.name).collect();
    assert!(!va(&tab), "Patch artists: {tab:?}");
    assert!(tab.iter().any(|n| n == "Burial"), "a real artist still appears");

    let deck = db.deck_view(me, "kim").await.unwrap().expect("deck");
    assert!(!va(&deck.both_play), "you both play: {:?}", deck.both_play);
    assert!(deck.both_play.iter().any(|n| n == "Burial"), "a real overlap survives");

    let pulse = db.instance_pulse(0).await.unwrap();
    let heard: Vec<String> = pulse.artists.into_iter().map(|a| a.name).collect();
    assert!(!va(&heard), "instance pulse: {heard:?}");
}

/// is the only way a future query gets caught too.
#[tokio::test]
async fn a_skip_reaches_no_counting_readout() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let now = Database::now_secs();

    // One real listen, and five skips of a different artist — enough that a
    // missing filter would put the skipped artist on top.
    let mut played = play("Played", "Real Artist", (now - 100) as u64);
    played.duration = Some(200);
    db.save_scrobble(
        uid,
        &played,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    for i in 0..5 {
        let mut p = play("Skipped", "Skipped Artist", (now - 3000 - i * 400) as u64);
        p.duration = Some(200);
        p.source_id = format!("skip-{i}");
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped: true },
        )
        .await
        .unwrap();
    }

    assert_eq!(db.listen_count_and_latest(uid).await.unwrap().0, 1, "listen count");

    let stats = db.get_dashboard_stats(uid, chrono_tz::UTC, 1).await.unwrap();
    assert_eq!(
        stats["top_artist"].as_str(),
        Some("Real Artist"),
        "a skipped artist must never win the dashboard"
    );

    // Coverage readouts: both halves must count the same population, or the
    // progress bar reports a fraction of the wrong whole.
    let (art_total, _) = db.artwork_coverage(uid).await.unwrap();
    assert_eq!(art_total, 1, "artwork coverage counts only real listens");
    let (lang_total, _) = db.language_coverage().await.unwrap();
    assert_eq!(lang_total, 1, "language coverage counts only what the backfill will process");
}

/// The Statistics row's four "shape" figures, and the two rules that make
/// them mean anything.
///
/// `skips` is the one query in this file that deliberately counts skipped
/// rows, sitting beside a dozen that must not — so it is exactly the shape
/// of query that gets "fixed" by someone adding the usual filter.
///
/// `per_day` divides by **calendar** days rather than by days that saw a
/// listen. Written the obvious way, a burst of listening in one afternoon
/// reports the afternoon's rate as the year's.
#[tokio::test]
async fn the_shape_figures_count_the_right_populations() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let now = Database::now_secs();

    // Ten listens and four skips, all inside one hour a fortnight ago —
    // a burst, so "per active day" and "per calendar day" disagree loudly.
    for i in 0..10i64 {
        let mut p = play(&format!("Track {i}"), "Real Artist", (now - 14 * 86_400 + i) as u64);
        p.duration = Some(200);
        p.source_id = format!("listen-{i}");
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }
    for i in 0..4i64 {
        let mut p = play(&format!("Skipped {i}"), "Other", (now - 14 * 86_400 + 100 + i) as u64);
        p.duration = Some(200);
        p.source_id = format!("skip-{i}");
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped: true },
        )
        .await
        .unwrap();
    }

    let all = db.get_ranged_stats(uid, None, false).await.unwrap();
    let t = &all["totals"];

    assert_eq!(t["plays"].as_i64(), Some(10), "plays still exclude skips");
    assert_eq!(t["skips"].as_i64(), Some(4), "skips are counted, not filtered away");

    // The listening spans one hour but the history is a fortnight old, so
    // the honest rate is ten plays over fourteen days, not over one.
    let per_day = t["per_day"].as_f64().unwrap();
    assert!(
        (0.6..0.8).contains(&per_day),
        "per_day must divide by calendar days, got {per_day} (10 plays / ~14 days ≈ 0.71)"
    );

    // "Since" is about the whole history and ignores the range control, or
    // a one-week view answers with the start of that week.
    let first = t["first_listen"].as_i64().expect("first_listen");
    let week = db.get_ranged_stats(uid, Some(now - 7 * 86_400), false).await.unwrap();
    assert_eq!(week["totals"]["first_listen"].as_i64(), Some(first), "first_listen is not ranged");
    assert_eq!(week["totals"]["plays"].as_i64(), Some(0), "the burst is outside this week");
    assert_eq!(week["totals"]["skips"].as_i64(), Some(0), "and so are its skips");

    // The average quality's denominator, which is the thing the card has to
    // report. Nothing here carried a quality score, so it is zero — and a
    // card reading "80.6/100" over zero listens is what this prevents.
    assert_eq!(t["quality_known"].as_i64(), Some(0));

    assert_eq!(t["loves"].as_i64(), Some(0));
    let e = db.upsert_entity("recording", "Track 1", "Real Artist", None).await.unwrap();
    db.set_love(uid, e, true).await.unwrap();
    let after = db.get_ranged_stats(uid, None, false).await.unwrap();
    assert_eq!(after["totals"]["loves"].as_i64(), Some(1));
}

/// Three consecutive plays of one track must store three listens.
///
/// The window used to be the track's own duration, and a back-to-back
/// repeat lands *exactly* one duration later — right on an inclusive
/// `BETWEEN` boundary. Whether the second play survived came down to poll
/// jitter, which is why a track played twice showed up once, sometimes.
#[tokio::test]
async fn a_track_on_repeat_is_not_deduplicated_away() {
    let db = testdb().await;
    let duration = 200u64;

    for pass in 0..3u64 {
        // Each pass is banked halfway through, so consecutive plays are
        // exactly one track length apart.
        let scrobbled_at = 1_700_000_000 + pass * duration + duration / 2;
        let mut p = play("Combustible", "Cœur de pirate", scrobbled_at);
        p.duration = Some(duration);
        // A per-pass id, as the live sources now produce.
        p.source_id = format!("jellyfin-item1-scrobble-{scrobbled_at}");
        assert!(
            db.save_scrobble(
                1,
                &p,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE
            )
            .await
            .unwrap(),
            "pass {pass} should have been stored"
        );
    }
    assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 3);
}

/// The same play arriving twice from two code paths — the live session and
/// the history walk — is still one listen. This is what the window is for,
/// and it must keep working now that the window is smaller.
#[tokio::test]
async fn the_same_play_reported_twice_is_still_one_listen() {
    let db = testdb().await;
    let mut first = play("Combustible", "Cœur de pirate", 1_700_000_000);
    first.duration = Some(200);
    assert!(db
        .save_scrobble(1, &first, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap());

    // Seconds later, under a different source_id, as the history endpoint
    // re-reports what the live session already banked.
    let mut echo = play("Combustible", "Cœur de pirate", 1_700_000_000 + 45);
    echo.duration = Some(200);
    echo.source_id = "jellyfin-item1-history-9".into();
    assert!(
        !db.save_scrobble(
            1,
            &echo,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE
        )
        .await
        .unwrap(),
        "a re-report within the lag window is the same listen"
    );
    assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 1);
}

/// The Plex compilation double-scrobble: the live path banks the *track*
/// artist and the history walk banks the *album* artist, so the same listen
/// arrived seconds apart under two names and neither row could see the
/// other.
///
/// This is what put "Various Artists" at the top of a real history's artist
/// chart, so the assertion is on the listen count rather than on the SQL —
/// it is the count that was wrong.
#[tokio::test]
async fn a_compilation_reported_under_two_names_is_still_one_listen() {
    let db = testdb().await;
    let mut live = play("Wherever I May Roam", "Jon Pardi", 1_700_000_000);
    live.duration = Some(300);
    assert!(db
        .save_scrobble(1, &live, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap());

    // One second later, off `/status/sessions/history/all`, which carries
    // the album artist and no track artist at all.
    let mut hist = play("Wherever I May Roam", "Various Artists", 1_700_000_001);
    hist.duration = Some(300);
    hist.source_id = "plex-hist-15194".into();
    assert!(
        !db.save_scrobble(
            1,
            &hist,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE
        )
        .await
        .unwrap(),
        "the placeholder carries no performer, so it cannot make this a second listen"
    );
    assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 1);
}

/// The placeholder is treated as *unknown*, never as a wildcard that
/// swallows anything nearby. Two genuinely different songs sharing a title
/// inside the window — two covers back to back — are two listens, and
/// losing one of those is a listen gone off a permanent record.
#[tokio::test]
async fn two_different_songs_sharing_a_title_stay_two_listens() {
    let db = testdb().await;
    let mut first = play("Hallelujah", "Jeff Buckley", 1_700_000_000);
    first.duration = Some(120);
    assert!(db
        .save_scrobble(1, &first, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap());

    let mut second = play("Hallelujah", "k.d. lang", 1_700_000_030);
    second.duration = Some(120);
    second.source_id = "plex-999-scrobble-1700000030".into();
    assert!(
        db.save_scrobble(
            1,
            &second,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE
        )
        .await
        .unwrap(),
        "two named artists that differ are two listens, however close together"
    );
    assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 2);
}

/// A recording MBID settles it whatever the two servers called the
/// performer — the composer-vs-singer case, where neither side is the
/// Various Artists placeholder.
#[tokio::test]
async fn a_shared_recording_mbid_collapses_two_spellings() {
    let db = testdb().await;
    let mbid = "b1a9c0de-0000-4000-8000-000000000001";
    let mut live = play("Ennodu Nee Irundhaal", "Sid Sriram", 1_700_000_000);
    live.duration = Some(300);
    live.mbid_recording = Some(mbid.into());
    assert!(db
        .save_scrobble(1, &live, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap());

    let mut hist = play("Ennodu Nee Irundhaal", "A.R. Rahman", 1_700_000_004);
    hist.duration = Some(300);
    hist.source_id = "plex-hist-4242".into();
    hist.mbid_recording = Some(mbid.into());
    assert!(
        !db.save_scrobble(
            1,
            &hist,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE
        )
        .await
        .unwrap(),
        "same recording, same moment — one listen"
    );
    assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 1);
}

/// Every DSD listen polled before v0.65.0 was written as lossy PCM with a
/// score of 60. The classifier fix alone does not touch them, because
/// `quality_score` is written once at ingest — so this pass is the only
/// thing that makes the reported symptom go away.
///
/// Also pins that it is idempotent: a second run must correct nothing.
#[tokio::test]
async fn the_dsd_listens_stored_as_lossy_are_re_filed_once() {
    let db = testdb().await;
    let mut p = play("Speak to Me", "Pink Floyd", 1_700_000_000);
    p.duration = Some(90);
    db.save_scrobble(1, &p, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap();

    // Exactly what a Plex DSF session used to land as.
    sqlx::query(
        "UPDATE scrobbles SET codec = 'dsd_lsbf_planar', container = 'dsf',
                    sample_rate = 352800, format_type = 'pcm', is_lossless = 0,
                    quality_score = 60.0",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    assert_eq!(db.reclassify_dsd_listens().await.unwrap(), 1);
    let (ft, lossless, mult, score): (String, bool, Option<i64>, f64) = sqlx::query_as(
        "SELECT format_type, is_lossless, dsd_multiplier, quality_score FROM scrobbles",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ft, "dsd");
    assert!(lossless, "a DSD stream is not lossy");
    assert_eq!(mult, Some(64), "352800 is DSD64 quoted in bytes");
    assert_eq!(score, 92.0);

    assert_eq!(
        db.reclassify_dsd_listens().await.unwrap(),
        0,
        "the WHERE selects only rows still misclassified, so a second run is a no-op"
    );
}

/// A pairing code mints exactly one token and hands it over exactly once.
/// Both halves matter: a second approval would give an attacker who watched
/// someone type a code a token of their own, and a request row left holding
/// the token would be a second live copy of a credential.
#[tokio::test]
async fn a_pairing_code_is_single_use_in_both_directions() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "admin").await.unwrap();

    let (device_code, user_code) =
        db.start_pairing("Pano on Android", "submit read").await.unwrap();
    assert_eq!(user_code.len(), 9, "8 characters plus the separator");
    assert_eq!(db.poll_pairing(&device_code).await.unwrap(), PairingPoll::Pending);

    assert!(db.approve_pairing(&user_code, uid, None).await.unwrap());
    // A second approval must not mint a second token.
    assert!(!db.approve_pairing(&user_code, uid, None).await.unwrap());

    let token = match db.poll_pairing(&device_code).await.unwrap() {
        PairingPoll::Approved(t) => t,
        other => panic!("expected a token, got {other:?}"),
    };
    // Collected once; the row is gone, so polling again cannot re-serve it.
    assert_eq!(db.poll_pairing(&device_code).await.unwrap(), PairingPoll::Unknown);

    // The token really works, and carries the scopes that were asked for.
    let auth = db.validate_token(&token).await.unwrap().expect("valid");
    assert_eq!(auth.user.user_id, uid);
    assert!(auth.can_read());
}

/// `read` is an explicit grant. A token that only says `submit` must not be
/// able to read, or the scope column would be decoration again.
#[tokio::test]
async fn read_is_not_implied_by_submit() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let plain = db.create_token(uid, "scrobbler", "submit", None, false).await.unwrap();
    assert!(!db.validate_token(&plain).await.unwrap().unwrap().can_read());

    let reader = db.create_token(uid, "plugin", "submit read", None, false).await.unwrap();
    assert!(db.validate_token(&reader).await.unwrap().unwrap().can_read());
}

/// The same discipline for `write`, and the pair that matters most: `read`
/// and `write` must not imply each other. A phone that may edit a listen is
/// a different grant from one that may enumerate the history, and every
/// scrobble client in the world holds `submit` — that one must carry
/// neither.
#[tokio::test]
async fn write_is_its_own_grant_and_implies_nothing() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let submit = db.create_token(uid, "scrobbler", "submit", None, false).await.unwrap();
    let t = db.validate_token(&submit).await.unwrap().unwrap();
    assert!(!t.can_write(), "submit must not imply write");
    assert!(!t.can_read(), "submit must not imply read");

    // read does not carry write...
    let reader = db.create_token(uid, "plugin", "submit read", None, false).await.unwrap();
    let t = db.validate_token(&reader).await.unwrap().unwrap();
    assert!(t.can_read());
    assert!(!t.can_write(), "read must not imply write");

    // ...and write does not carry read.
    let writer = db.create_token(uid, "phone", "submit write", None, false).await.unwrap();
    let t = db.validate_token(&writer).await.unwrap().unwrap();
    assert!(t.can_write());
    assert!(!t.can_read(), "write must not imply read");

    // `all` is the one grant that covers both.
    let every = db.create_token(uid, "trusted", "all", None, false).await.unwrap();
    let t = db.validate_token(&every).await.unwrap().unwrap();
    assert!(t.can_read() && t.can_write());

    // Substrings must not match: `rewrite` is not `write`.
    let sneaky = db.create_token(uid, "odd", "submit rewrite readonly", None, false).await.unwrap();
    let t = db.validate_token(&sneaky).await.unwrap().unwrap();
    assert!(!t.can_write(), "'rewrite' must not grant write");
    assert!(!t.can_read(), "'readonly' must not grant read");
}

/// `save_scrobble` hardcoded `status = 'pending'`, so a listen submitted as
/// `listen_type: "import"` was indistinguishable from one that had just
/// played and went out to Last.fm — rewriting a permanent public record
/// with someone's backfill. The forwarding queue is the thing to assert on:
/// a backfill must never appear in it.
#[tokio::test]
async fn a_backfilled_listen_is_never_queued_for_forwarding() {
    let db = testdb().await;

    db.save_scrobble(
        1,
        &play("Live", "A", 1000),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    db.save_scrobble(
        1,
        &play("Backfilled", "A", 2000),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition { imported: true, skipped: false },
    )
    .await
    .unwrap();

    let pending = db.get_pending_scrobbles().await.unwrap();
    assert_eq!(pending.len(), 1, "only the live listen may be forwarded");
    assert_eq!(pending[0].play.title, "Live");

    // Both are still *stored* — an import is history, not a discard.
    assert_eq!(db.listen_count_and_latest(1).await.unwrap().0, 2);
}

/// A skip is stored so skip-rate analytics has something real to count, and
/// filed as `'imported'` so nothing forwards it: a skip on someone's
/// Last.fm is a wrong scrobble on a permanent record.
#[tokio::test]
async fn a_skipped_listen_is_stored_uncounted_and_unforwarded() {
    let db = testdb().await;

    db.save_scrobble(
        1,
        &play("Played", "A", 1000),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    db.save_scrobble(
        1,
        &play("Skipped", "A", 2000),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition { imported: false, skipped: true },
    )
    .await
    .unwrap();

    assert!(
        db.get_pending_scrobbles().await.unwrap().iter().all(|p| p.play.title != "Skipped"),
        "a skip must never reach a sink"
    );
    assert_eq!(
        db.listen_count_and_latest(1).await.unwrap().0,
        1,
        "a skip is excluded from every count"
    );

    let stored: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM scrobbles WHERE user_id = 1 AND skipped = TRUE")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(stored.0, 1, "but it is kept, so skip-rate analytics can see it");
}

/// These five were built on every listen and dropped on the floor:
/// `AudioQuality` carried them and `save_scrobble` had no columns to bind
/// them to. The regression to guard is silent — a field that is simply
/// always NULL.
#[tokio::test]
async fn the_full_quality_record_round_trips() {
    let db = testdb().await;
    let q = AudioQuality {
        format_type: Some("dsd".into()),
        container: Some("dsf".into()),
        source_quality: Some("hi-res".into()),
        dsd_rate: Some(2_822_400),
        dsd_multiplier: Some(64),
        dsd_to_pcm_converted: Some(true),
        delivery_codec: Some("ldac".into()),
        delivery_sample_rate: Some(96_000),
        delivery_bit_depth: Some(24),
        ..Default::default()
    };
    db.save_scrobble(
        1,
        &play("T", "A", 1000),
        Some(&q),
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();

    let row: (Option<String>, Option<String>, Option<bool>, Option<i32>, Option<i32>) =
        sqlx::query_as(
            "SELECT container, source_quality, dsd_to_pcm_converted,
                        delivery_sample_rate, delivery_bit_depth
                   FROM scrobbles WHERE user_id = 1",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("dsf"));
    assert_eq!(row.1.as_deref(), Some("hi-res"));
    assert_eq!(row.2, Some(true));
    assert_eq!(row.3, Some(96_000));
    assert_eq!(row.4, Some(24));
}

/// The bug this pass had from the start: its SQL grouped by
/// `LOWER(TRIM(artist))` while the Rust grouping also lowercases, so a
/// case-only variant was merged away before anything could report it. On a
/// real 21k-listen history that meant thirteen groups went unreported and
/// the screen said there was nothing to fix.
#[tokio::test]
async fn case_only_variants_are_reported() {
    let db = testdb().await;
    let mut ts = 1000u64;
    for (artist, n) in [("Linkin Park", 5), ("LINKIN PARK", 2), ("Kiss", 1), ("KISS", 3)] {
        for _ in 0..n {
            ts += 1000;
            db.save_scrobble(
                1,
                &play("T", artist, ts),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE,
            )
            .await
            .unwrap();
        }
    }

    let v = db.artists_with_spelling_variants(1).await.unwrap();
    let lp = v.iter().find(|g| g.primary.eq_ignore_ascii_case("linkin park")).expect("found");
    assert_eq!(lp.primary, "Linkin Park", "the most-played spelling wins");
    assert_eq!(lp.variants, vec![("LINKIN PARK".to_string(), 2)]);
    assert!(lp.case_only, "differs only in case");

    let kiss = v.iter().find(|g| g.primary.eq_ignore_ascii_case("kiss")).expect("found");
    assert_eq!(kiss.primary, "KISS", "and it is the count that decides, not the casing");
}

/// The other half: a ligature is not a case difference. These are two
/// artists to every query — two chart rows, two entity keys — so they must
/// be reported *and* not marked cosmetic.
#[tokio::test]
async fn ligature_variants_are_reported_as_real() {
    let db = testdb().await;
    let mut ts = 1000u64;
    for (artist, n) in [("Cœur de pirate", 4), ("Coeur de Pirate", 2)] {
        for _ in 0..n {
            ts += 1000;
            db.save_scrobble(
                1,
                &play("T", artist, ts),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE,
            )
            .await
            .unwrap();
        }
    }

    let v = db.artists_with_spelling_variants(1).await.unwrap();
    let g = v.iter().find(|g| g.primary.starts_with('C')).expect("grouped");
    assert_eq!(g.primary, "Cœur de pirate");
    assert_eq!(g.variants, vec![("Coeur de Pirate".to_string(), 2)]);
    assert!(!g.case_only, "a ligature splits the play count — not cosmetic");
}

/// Two genuinely different artists must never be grouped, whatever the
/// normaliser folds.
#[tokio::test]
async fn different_artists_are_not_grouped() {
    let db = testdb().await;
    let mut ts = 1000u64;
    for artist in ["Burial", "Aphex Twin", "Karthik", "Karthika"] {
        ts += 1000;
        db.save_scrobble(
            1,
            &play("T", artist, ts),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }
    assert!(db.artists_with_spelling_variants(1).await.unwrap().is_empty());
}

/// The soundtrack case, in the exact shape it has in real data: an album
/// where every track carries the music director and each track also carries
/// its own singers.
///
/// The director must win, and must win *on breadth*. Ranking by play count
/// got this backwards on a real export — a singer whose one track had been
/// played more than the director's spread outranked them, and the fold ran
/// the wrong way, moving "Harris Jayaraj" onto a singer. Here the singers
/// deliberately out-play the director on their own track.
#[tokio::test]
async fn the_album_artist_wins_on_breadth_not_plays() {
    let db = testdb().await;
    let mut ts = 1_000u64;

    // Written out rather than looped so the play counts are visible: the
    // director is on three tracks with 1 play each, one singer has 9 on a
    // single track. Play count would crown the singer.
    let rows: [(&str, &str, usize); 5] = [
        ("Vaarayo Vaarayo", "Harris Jayaraj", 1),
        ("Hasili Fisiliye", "Harris Jayaraj", 1),
        ("Yaaro Yaaro", "Harris Jayaraj", 1),
        ("Vaarayo Vaarayo", "Unnikrishnan, Chinmayi & Megha", 9),
        ("Hasili Fisiliye", "Karthik", 2),
    ];
    for (title, artist, n) in rows {
        for _ in 0..n {
            ts += 1000;
            let mut p = play(title, artist, ts);
            p.album = Some("Aadhavan".into());
            db.save_scrobble(
                1,
                &p,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                ListenDisposition::LIVE,
            )
            .await
            .unwrap();
        }
    }

    let splits = db.tracks_with_split_attribution(1).await.unwrap();
    let v =
        splits.iter().find(|s| s.title == "vaarayo vaarayo").expect("the split track is listed");

    assert_eq!(v.primary, "Harris Jayaraj", "the director wins despite fewer plays");
    assert_eq!(v.primary_album_tracks, 3, "and wins because they are on every track");
    assert_eq!(v.folding.len(), 1);
    assert_eq!(v.folding[0].0, "Unnikrishnan, Chinmayi & Megha");
    assert_eq!(v.folding[0].1, 9);

    // Applying it keeps the singers rather than erasing them.
    let n = db
        .merge_track_attribution(
            1,
            "vaarayo vaarayo",
            "aadhavan",
            "Unnikrishnan, Chinmayi & Megha",
            "Harris Jayaraj",
        )
        .await
        .unwrap();
    assert_eq!(n, 9, "every stray listen moves");

    let credit: (Option<String>,) = sqlx::query_as(
        "SELECT artist_credit FROM scrobbles
             WHERE user_id = 1 AND LOWER(title) = 'vaarayo vaarayo'
               AND artist_credit IS NOT NULL LIMIT 1",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let credit = credit.0.expect("the folded name is kept");
    assert!(credit.contains("Unnikrishnan"), "singers survive in the credit: {credit}");
    assert!(credit.contains("Harris Jayaraj"), "and so does the director: {credit}");

    // Nothing is left split.
    let after = db.tracks_with_split_attribution(1).await.unwrap();
    assert!(!after.iter().any(|s| s.title == "vaarayo vaarayo"));
}

#[tokio::test]
async fn dedup_rejects_duplicate_source() {
    let db = testdb().await;
    let p = play("Teardrop", "Massive Attack", 1000);
    assert!(db
        .save_scrobble(1, &p, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap());
    // same source_id + name → deduped
    assert!(!db
        .save_scrobble(1, &p, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap());
}

#[tokio::test]
async fn per_sink_delivery_is_tracked() {
    let db = testdb().await;
    db.save_scrobble(
        1,
        &play("T", "A", 2000),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();

    let pending = db.get_pending_scrobbles().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].delivered.is_empty());
    let id = pending[0].id;

    db.mark_sink_delivered(id, "lastfm").await.unwrap();
    db.mark_sink_delivered(id, "lastfm").await.unwrap(); // idempotent
    let pending = db.get_pending_scrobbles().await.unwrap();
    assert_eq!(pending[0].delivered, vec!["lastfm".to_string()]);

    // still pending until marked synced
    db.mark_scrobble_synced(id).await.unwrap();
    assert!(db.get_pending_scrobbles().await.unwrap().is_empty());
}

#[tokio::test]
async fn chain_name_and_output_binding_resolve() {
    let db = testdb().await;
    let uid = db.create_user("alice", None, "not-used-yet", "admin").await.unwrap();
    let chain = SignalChain {
        id: None,
        user_id: uid,
        name: "Desktop".into(),
        description: None,
        components: vec![],
        listening_context: ListeningContext::Active,
        icon: None,
        is_active: true,
        shared: false,
        total_hours: 0.0,
        created_at: 0,
    };
    let cid = db.create_chain(&chain).await.unwrap();

    // name → id (case-insensitive)
    assert_eq!(db.get_chain_by_name(uid, "desktop").await.unwrap(), Some(cid));
    assert_eq!(db.get_chain_by_name(uid, "nope").await.unwrap(), None);

    // auto-learn an output, unmapped resolves to None, then bind it
    db.record_output(uid, "Schiit Mimir", Some("usb")).await.unwrap();
    assert_eq!(db.resolve_binding(uid, "Schiit Mimir").await.unwrap(), None);
    db.set_binding(uid, "Schiit Mimir", Some(cid)).await.unwrap();
    assert_eq!(db.resolve_binding(uid, "Schiit Mimir").await.unwrap(), Some(cid));
}

#[tokio::test]
async fn bulk_set_chain_updates_only_targets() {
    let db = testdb().await;
    let uid = db.create_user("bob", None, "not-used-yet", "admin").await.unwrap();
    let cid = db
        .create_chain(&SignalChain {
            id: None,
            user_id: uid,
            name: "C".into(),
            description: None,
            components: vec![],
            listening_context: ListeningContext::Active,
            icon: None,
            is_active: true,
            shared: false,
            total_hours: 0.0,
            created_at: 0,
        })
        .await
        .unwrap();
    db.save_scrobble(
        uid,
        &play("a", "x", 10),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    db.save_scrobble(
        uid,
        &play("b", "y", 20),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    let ids: Vec<i64> = db.get_all_scrobbles(uid).await.unwrap().iter().map(|r| r.id).collect();

    let n = db.bulk_set_scrobble_chain(uid, &ids[..1], Some(cid), None).await.unwrap();
    assert_eq!(n, 1);
    let rows = db.get_all_scrobbles(uid).await.unwrap();
    assert_eq!(rows.iter().filter(|r| r.chain_id == Some(cid)).count(), 1);
}

/// Chain and gear hours follow the listens, whenever the listens were
/// assigned.
///
/// This was an accumulator written at one insert site, which meant three
/// things looked identical from the outside — a chain that has plainly been
/// used reading zero. Only ingested listens counted (a polled Plex listen
/// added nothing), assigning a chain afterwards moved nothing, and
/// `equipment.total_hours` derives from this so gear inherited all of it.
/// The reported symptom was "assigning signal chains to devices doesn't
/// update the usage hours", which is the second of those.
#[tokio::test]
async fn chain_and_gear_hours_follow_the_listens_however_late_they_are_assigned() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "admin").await.unwrap();

    let gear = db
        .upsert_equipment(uid, "Bifrost 2", "dac", Some("Schiit"), None, 0.0, None)
        .await
        .unwrap();
    let cid = db
        .create_chain(&SignalChain {
            id: None,
            user_id: uid,
            name: "Desktop".into(),
            description: None,
            // The same gear at two steps. Joining `json_each` counted the
            // chain's whole running time once per mention, so a preamp used
            // twice doubled its own hours.
            components: vec![
                ChainComponent {
                    component_type: ComponentType::Dac,
                    name: "Bifrost 2".into(),
                    detail: None,
                    equipment_id: Some(gear),
                },
                ChainComponent {
                    component_type: ComponentType::Transport,
                    name: "Bifrost 2".into(),
                    detail: None,
                    equipment_id: Some(gear),
                },
            ],
            listening_context: ListeningContext::Active,
            icon: None,
            is_active: true,
            shared: false,
            total_hours: 0.0,
            created_at: 0,
        })
        .await
        .unwrap();

    // Two hours played, plus a skip of the same length. None of it is
    // assigned to the chain yet — which is the ordinary case, since a listen
    // polled from a media server carries no chain until one is bound.
    for (i, title) in ["a", "b"].iter().enumerate() {
        let mut p = play(title, "x", 1000 + i as u64);
        p.duration = Some(3600);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }
    let mut skipped = play("c", "x", 3000);
    skipped.duration = Some(3600);
    db.save_scrobble(
        uid,
        &skipped,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition { imported: false, skipped: true },
    )
    .await
    .unwrap();

    let hours = |c: &SignalChain| (c.total_hours * 100.0).round() / 100.0;
    assert_eq!(hours(&db.get_chain(uid, cid).await.unwrap().unwrap()), 0.0);

    // Now assign them, the way binding an output device or fixing a listen
    // by hand would. An accumulator counted nothing here; the derivation
    // picks up every listen that carries the chain.
    let ids: Vec<i64> = db.get_all_scrobbles(uid).await.unwrap().iter().map(|r| r.id).collect();
    assert_eq!(ids.len(), 3);
    db.bulk_set_scrobble_chain(uid, &ids, Some(cid), None).await.unwrap();

    let chain = db.get_chain(uid, cid).await.unwrap().unwrap();
    assert_eq!(hours(&chain), 2.0, "two played hours; the skip is not one of them");
    assert_eq!(
        hours(&db.get_chains(uid).await.unwrap()[0]),
        2.0,
        "the listing and the single read must agree"
    );

    // Gear sums its chains, and the doubled component must not double it.
    let gear_row = db.get_equipment(uid).await.unwrap().into_iter().find(|g| g.id == gear).unwrap();
    assert_eq!((gear_row.total_hours * 100.0).round() / 100.0, 2.0);

    // Unassigning takes the hours back with it, which an accumulator could
    // never do.
    db.bulk_set_scrobble_chain(uid, &ids, None, None).await.unwrap();
    assert_eq!(hours(&db.get_chain(uid, cid).await.unwrap().unwrap()), 0.0);
}

/// A token per app, each with its own chain, is the whole point of
/// `default_chain_id` — so validating a token must hand back the chain in
/// the same breath as the identity, and editing must not re-issue it.
/// Merge and split rewrite `session_id` across rows and then rebuild the
/// aggregates from what's left — the failure worth designing out is a
/// session whose counts disagree with its own listens.
/// A rebuild has to produce the same aggregates the per-session recompute
/// did, because it no longer calls it: the walk now folds every session in
/// Rust and writes it once, which is what took a 49k-listen rebuild from
/// ~50,000 autocommitted round trips down to one transaction. On the dev
/// machine the old version merely looked slow; on a Pi with the database on
/// an SD card it ran past a reverse proxy's 100s timeout and the request
/// died with a 524 while the rebuild carried on unseen.
///
/// The quality average is the part whose mechanics changed and the part
/// most easily got wrong: `AVG(quality_score)` ignores NULLs, and most
/// listens have none, so summing over every row and dividing by the row
/// count would score an imported history at nearly zero.
#[tokio::test]
async fn a_rebuild_groups_by_the_gap_and_keeps_the_aggregates_honest() {
    let db = testdb().await;
    let uid = db.create_user("ida", None, "x", "user").await.unwrap();

    // Three listens close together, then a long gap, then two more.
    // Durations and scores are deliberately uneven, and two listens carry
    // no score at all.
    let listens: [(u64, u64, Option<f64>); 5] = [
        (1_000, 180, Some(80.0)),
        (1_200, 180, None),
        (1_400, 200, Some(60.0)),
        (10_000, 240, None),
        (10_300, 120, Some(90.0)),
    ];
    for (i, (ts, dur, score)) in listens.iter().enumerate() {
        let mut p = play(&format!("t{i}"), "A", *ts);
        p.duration = Some(*dur);
        let quality = AudioQuality { quality_score: *score, ..Default::default() };
        db.save_scrobble(
            uid,
            &p,
            Some(&quality),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    let (sessions, linked) = db.rebuild_sessions(uid, 1800).await.unwrap();
    assert_eq!(sessions, 2, "the two-and-a-half-hour gap makes two sittings");
    assert_eq!(linked, 5, "every listen belongs to one of them");

    let rows = db.list_sessions(uid, 50, 0).await.unwrap();
    assert_eq!(rows.len(), 2);
    // `list_sessions` is newest first.
    let (early, late) = (&rows[1], &rows[0]);

    assert_eq!(early.track_count, 3);
    assert_eq!(early.started_at, 1_000);
    assert_eq!(early.ended_at, Some(1_600), "the last listen's start plus its length");
    assert_eq!(early.total_duration, 560);
    assert_eq!(late.track_count, 2);
    assert_eq!(late.total_duration, 360);

    // 80 and 60 averaged over the *two* listens that had a score, not over
    // all three.
    let early_q = early.avg_quality_score.expect("two of the three were scored");
    assert!((early_q - 70.0).abs() < 0.001, "expected 70, got {early_q}");
    let late_q = late.avg_quality_score.expect("one of the two was scored");
    assert!((late_q - 90.0).abs() < 0.001, "expected 90, got {late_q}");

    // Every listen actually carries its link — the bug `rebuild_sessions`
    // was written to fix in the first place was sessions with track counts
    // and no contents.
    let linked_rows: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND session_id IS NOT NULL",
    )
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(linked_rows.0, 5);

    // Idempotent: running it again regroups to the same shape rather than
    // stacking a second set of sessions on top.
    let (again, _) = db.rebuild_sessions(uid, 1800).await.unwrap();
    assert_eq!(again, 2);
    assert_eq!(db.list_sessions(uid, 50, 0).await.unwrap().len(), 2);
}

#[tokio::test]
async fn sessions_merge_and_split_and_keep_their_aggregates_honest() {
    let db = testdb().await;
    let uid = db.create_user("hana", None, "x", "user").await.unwrap();

    // Two sittings an hour apart: the gap heuristic makes them separate.
    let mut ids = Vec::new();
    for (i, ts) in [1_000, 1_200, 1_400, 10_000, 10_200].iter().enumerate() {
        db.save_scrobble(
            uid,
            &play(&format!("t{i}"), "A", *ts),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
        let sid = db
            .assign_session(uid, *ts as i64, 180, None, None, None, "active", 1800, None)
            .await
            .unwrap();
        ids.push(sid);
    }
    // Point the listens at the sessions the assigner chose.
    let rows = db.get_all_scrobbles(uid).await.unwrap();
    for (row, sid) in rows.iter().rev().zip(ids.iter()) {
        sqlx::query("UPDATE scrobbles SET session_id = ? WHERE id = ?")
            .bind(sid)
            .bind(row.id)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    for sid in &ids {
        db.recompute_session(*sid).await.unwrap();
    }

    let sessions = db.list_sessions(uid, 50, 0).await.unwrap();
    assert_eq!(sessions.len(), 2, "the hour gap should make two sittings");
    let (first, second) = (sessions[1].id, sessions[0].id);
    assert_eq!(sessions[1].track_count + sessions[0].track_count, 5);

    // Merge: the earlier session survives and absorbs everything.
    let target = db.merge_sessions(uid, &[first, second]).await.unwrap().unwrap();
    assert_eq!(target, first);
    let merged = db.list_sessions(uid, 50, 0).await.unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].track_count, 5, "aggregates rebuilt from the listens");

    // Split it back at the fourth listen.
    let listens = db.session_scrobbles(uid, target).await.unwrap();
    assert_eq!(listens.len(), 5);
    let new_id = db.split_session(uid, target, listens[3].id).await.unwrap().unwrap();
    let after = db.list_sessions(uid, 50, 0).await.unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(db.session_scrobbles(uid, target).await.unwrap().len(), 3);
    assert_eq!(db.session_scrobbles(uid, new_id).await.unwrap().len(), 2);

    // Splitting at the first listen is a no-op, not an empty session.
    let head = db.session_scrobbles(uid, target).await.unwrap()[0].id;
    assert!(db.split_session(uid, target, head).await.unwrap().is_none());
    assert_eq!(db.list_sessions(uid, 50, 0).await.unwrap().len(), 2);

    // Another user's session is untouchable.
    let other = db.create_user("ivan", None, "x", "user").await.unwrap();
    assert!(db.merge_sessions(other, &[target, new_id]).await.unwrap().is_none());
    assert_eq!(db.list_sessions(uid, 50, 0).await.unwrap().len(), 2);
}

/// Per-edit review hands the operator a choice, so the preview has to show
/// the whole ballot — not just the winner — and count the listens that carry
/// no release at all, since unifying stamps those too.
#[tokio::test]
async fn mixed_album_preview_reports_every_candidate_and_the_untagged() {
    let db = testdb().await;
    let uid = db.create_user("jo", None, "x", "user").await.unwrap();

    // Four listens on release A, one on B, two with no release at all.
    let mut plays = Vec::new();
    for i in 0..7 {
        let mut p = play(&format!("t{i}"), "Band", 100 + i as u64);
        p.album = Some("Record".into());
        p.mbid_release = match i {
            0..=3 => Some("aaaaaaaa-0000-0000-0000-000000000000".into()),
            4 => Some("bbbbbbbb-0000-0000-0000-000000000000".into()),
            _ => None,
        };
        plays.push(p);
    }
    for p in &plays {
        db.save_scrobble(uid, p, None, None, None, None, None, None, None, ListenDisposition::LIVE)
            .await
            .unwrap();
    }

    let mixed = db.albums_with_mixed_releases(uid).await.unwrap();
    assert_eq!(mixed.len(), 1, "{mixed:?}");
    let m = &mixed[0];
    assert_eq!(m.variants, 2);
    assert_eq!(m.winner_plays, 4);
    assert_eq!(m.untagged, 2, "listens with no release are counted separately");
    assert_eq!(m.candidates.len(), 2, "the operator sees the whole ballot");
    assert_eq!(m.candidates[0].plays, 4, "most-claimed first");

    // Unifying onto the *minority* release is allowed — that's the point of
    // reviewing — and it also stamps the untagged listens.
    let loser = m.candidates[1].mbid.clone();
    let n = db.unify_album_release(uid, "Band", "Record", &loser).await.unwrap();
    assert_eq!(n, 6, "everything except the one already on that release");
    assert!(db.albums_with_mixed_releases(uid).await.unwrap().is_empty());
}

/// The duplicate that started this: "A.R. Rahman" and "A. R. Rahman" split
/// one artist across two rows of the same top-50.
#[test]
fn artist_spellings_normalise_to_one_identity() {
    assert_eq!(normalise_artist("A.R. Rahman"), normalise_artist("A. R. Rahman"));
    assert_eq!(normalise_artist("A.R. Rahman"), normalise_artist("AR Rahman"));
    assert_eq!(normalise_artist("Sigur Rós"), normalise_artist("sigur rós"));
    // Different artists must not collapse together.
    assert_ne!(normalise_artist("Bill Evans"), normalise_artist("Gil Evans"));
    assert_ne!(normalise_artist("The Beatles"), normalise_artist("Beatles"));
}

#[tokio::test]
async fn spelling_variants_are_found_and_merged_into_the_most_played() {
    let db = testdb().await;
    let uid = db.create_user("gina", None, "x", "user").await.unwrap();
    for (i, artist) in
        ["A.R. Rahman", "A.R. Rahman", "A.R. Rahman", "A. R. Rahman"].iter().enumerate()
    {
        db.save_scrobble(
            uid,
            &play(&format!("t{i}"), artist, 100 + i as u64),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    let found = db.artists_with_spelling_variants(uid).await.unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    // The spelling with the most plays is the one that survives.
    assert_eq!(found[0].primary, "A.R. Rahman");
    assert_eq!(found[0].variants.len(), 1);

    db.rename_artist(uid, "A. R. Rahman", "A.R. Rahman").await.unwrap();
    assert!(db.artists_with_spelling_variants(uid).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_token_carries_its_default_chain_and_keeps_working_after_an_edit() {
    let db = testdb().await;
    let uid = db.create_user("erin", None, "x", "user").await.unwrap();
    let mk = |name: &str| SignalChain {
        id: None,
        user_id: uid,
        name: name.into(),
        description: None,
        components: vec![],
        listening_context: ListeningContext::Active,
        icon: None,
        is_active: true,
        shared: false,
        total_hours: 0.0,
        created_at: 0,
    };
    let desk = db.create_chain(&mk("Desk")).await.unwrap();
    let couch = db.create_chain(&mk("Couch")).await.unwrap();

    let token = db.create_token(uid, "fooyin", "submit", Some(desk), false).await.unwrap();
    assert_eq!(db.validate_token(&token).await.unwrap().unwrap().default_chain_id, Some(desk));

    let id = db.list_tokens(uid).await.unwrap()[0].id;
    assert!(db.update_token(uid, id, "fooyin", Some(couch), false).await.unwrap());
    // Same token string, new chain — nothing to re-paste into the client.
    assert_eq!(db.validate_token(&token).await.unwrap().unwrap().default_chain_id, Some(couch));

    assert!(db.update_token(uid, id, "fooyin", None, false).await.unwrap());
    assert_eq!(db.validate_token(&token).await.unwrap().unwrap().default_chain_id, None);

    // Someone else's token id is not editable.
    let other = db.create_user("frank", None, "x", "user").await.unwrap();
    assert!(!db.update_token(other, id, "stolen", Some(desk), false).await.unwrap());
}

/// The relay switch has to survive the round trip, and — the half worth
/// pinning — must be **off** for a token created without mentioning it.
/// Every token that existed before this column did reads as not-live, which
/// is what stops an upgrade forwarding anything it wasn't forwarding
/// yesterday. The rail that bounds it lives in `ingest::import_is_live`.
#[tokio::test]
async fn a_token_relays_imports_only_once_it_is_told_to() {
    let db = testdb().await;
    let uid = db.create_user("gina", None, "x", "user").await.unwrap();

    let quiet = db.create_token(uid, "some plugin", "submit", None, false).await.unwrap();
    assert!(!db.validate_token(&quiet).await.unwrap().unwrap().imports_are_live);

    let live = db.create_token(uid, "fooyin", "submit", None, true).await.unwrap();
    assert!(db.validate_token(&live).await.unwrap().unwrap().imports_are_live);

    // Flipped after the fact, without re-issuing — the whole point of
    // editing a token in place rather than minting a new one.
    let id =
        db.list_tokens(uid).await.unwrap().iter().find(|t| t.name == "some plugin").unwrap().id;
    assert!(db.update_token(uid, id, "some plugin", None, true).await.unwrap());
    assert!(db.validate_token(&quiet).await.unwrap().unwrap().imports_are_live);
    assert!(
        db.list_tokens(uid).await.unwrap().iter().find(|t| t.id == id).unwrap().imports_are_live
    );

    // And back off again.
    assert!(db.update_token(uid, id, "some plugin", None, false).await.unwrap());
    assert!(!db.validate_token(&quiet).await.unwrap().unwrap().imports_are_live);
}

#[tokio::test]
async fn disabled_user_cannot_authenticate_by_password_token_or_session() {
    let db = testdb().await;
    let hash = Database::hash_password("password123").unwrap();
    let uid = db.create_user("carol", None, &hash, "user").await.unwrap();
    let token = db.create_token(uid, "t", "submit", None, false).await.unwrap();
    let session = db.create_web_session(uid).await.unwrap();

    // enabled: all three work
    assert!(db.authenticate("carol", "password123").await.unwrap().is_some());
    assert!(db.validate_token(&token).await.unwrap().is_some());
    assert!(db.validate_session(&session).await.unwrap().is_some());

    db.set_user_disabled(uid, true).await.unwrap();

    // disabled: every path is rejected
    assert!(db.authenticate("carol", "password123").await.unwrap().is_none());
    assert!(db.validate_token(&token).await.unwrap().is_none());
    assert!(db.validate_session(&session).await.unwrap().is_none());
}

/// A user must be able to change the password on an account somebody else
/// created for them, and must not be able to do it without proving they
/// know the current one.
#[tokio::test]
async fn a_user_can_change_their_own_password_but_only_with_the_old_one() {
    let db = testdb().await;
    let hash = Database::hash_password("original-pw").unwrap();
    let uid = db.create_user("colin", None, &hash, "user").await.unwrap();

    assert!(db.check_user_password(uid, "original-pw").await);
    assert!(!db.check_user_password(uid, "not-the-password").await);

    let next = Database::hash_password("a-better-one").unwrap();
    assert!(db.update_user_password(uid, &next).await.unwrap());
    assert!(db.authenticate("colin", "a-better-one").await.unwrap().is_some());
    assert!(db.authenticate("colin", "original-pw").await.unwrap().is_none());
}

/// Changing a password is normally a reaction to "someone else may have
/// this", so the sessions opened with the old one must not survive it.
#[tokio::test]
async fn changing_a_password_drops_the_sessions_opened_with_the_old_one() {
    let db = testdb().await;
    let hash = Database::hash_password("original-pw").unwrap();
    let uid = db.create_user("colin", None, &hash, "user").await.unwrap();
    let elsewhere = db.create_web_session(uid).await.unwrap();
    assert!(db.validate_session(&elsewhere).await.unwrap().is_some());

    db.clear_user_sessions(uid).await.unwrap();
    assert!(db.validate_session(&elsewhere).await.unwrap().is_none());
}

/// A profile field distinguishes "leave alone" from "clear", so a request
/// naming one field cannot blank the others.
#[tokio::test]
async fn a_profile_edit_touches_only_the_fields_it_names() {
    let db = testdb().await;
    let uid = db.create_user("colin", Some("Colin"), "x", "user").await.unwrap();
    db.update_profile(uid, None, Some(Some("Collects tape.".into())), None).await.unwrap();

    let p = db.get_profile(uid).await.unwrap().unwrap();
    assert_eq!(p.bio.as_deref(), Some("Collects tape."));
    assert_eq!(p.display_name.as_deref(), Some("Colin"), "an unnamed field must survive");

    // Explicit null clears, and still leaves the rest alone.
    db.update_profile(uid, None, Some(None), None).await.unwrap();
    let p = db.get_profile(uid).await.unwrap().unwrap();
    assert_eq!(p.bio, None);
    assert_eq!(p.display_name.as_deref(), Some("Colin"));
}

/// One deck per person, enforced by the primary key: putting a second side
/// on replaces the first rather than erroring, because that is what
/// happened in the room — you took the record off.
#[tokio::test]
async fn a_user_has_one_deck_and_the_side_on_it_survives() {
    let db = testdb().await;
    let uid = db.create_user("colin", None, "x", "user").await.unwrap();
    let a = db
        .create_physical(
            uid, "vinyl", "Burial", "Untrue", None, None, None, None, None, None, "[]", None, None,
            None, None,
        )
        .await
        .unwrap();
    let b = db
        .create_physical(
            uid,
            "cd",
            "Aphex Twin",
            "SAW 85-92",
            None,
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
        .unwrap();

    assert!(db.get_spin(uid).await.unwrap().is_none(), "an empty deck to start");

    db.set_spin(uid, a, "A", 1_700_000_000, None, 0, "[]", None, None, 0).await.unwrap();
    let spin = db.get_spin(uid).await.unwrap().expect("a side is on");
    assert_eq!(spin.release_id, a);
    assert_eq!(spin.side, "A");
    assert_eq!(spin.paused_at, None);

    // Pausing keeps the same row rather than opening a second deck.
    db.set_spin(uid, a, "A", 1_700_000_000, Some(1_700_000_300), 0, "[]", None, None, 0)
        .await
        .unwrap();
    assert_eq!(db.get_spin(uid).await.unwrap().unwrap().paused_at, Some(1_700_000_300));

    // And a different record replaces it outright.
    db.set_spin(uid, b, "1", 1_700_001_000, None, 12, "[{}]", None, None, 0).await.unwrap();
    let spin = db.get_spin(uid).await.unwrap().unwrap();
    assert_eq!(spin.release_id, b);
    assert_eq!(spin.side, "1");
    assert_eq!(spin.paused_total, 12);

    assert!(db.clear_spin(uid).await.unwrap());
    assert!(db.get_spin(uid).await.unwrap().is_none());
}

/// Two people can each have a record on without seeing each other's.
#[tokio::test]
async fn one_persons_deck_is_not_anothers() {
    let db = testdb().await;
    let a = db.create_user("ashwin", None, "x", "admin").await.unwrap();
    let c = db.create_user("colin", None, "x", "user").await.unwrap();
    let mine = db
        .create_physical(
            a, "vinyl", "Burial", "Untrue", None, None, None, None, None, None, "[]", None, None,
            None, None,
        )
        .await
        .unwrap();
    db.set_spin(a, mine, "A", 1_700_000_000, None, 0, "[]", None, None, 0).await.unwrap();

    assert!(db.get_spin(a).await.unwrap().is_some());
    assert!(db.get_spin(c).await.unwrap().is_none());
    assert!(!db.clear_spin(c).await.unwrap(), "clearing an empty deck changes nothing");
    assert!(db.get_spin(a).await.unwrap().is_some(), "and leaves the other alone");
}

/// A tape remembers where it was left; a record has no such state.
///
/// The asymmetry is the design, not an omission — a cassette is sequential
/// and really is still parked halfway when you put it back in, while a
/// record has random access and goes back in its sleeve. Writing a park
/// point for vinyl would be modelling physics the object does not have.
#[tokio::test]
async fn a_tape_remembers_where_it_was_left_and_a_record_does_not() {
    let db = testdb().await;
    let uid = db.create_user("colin", None, "x", "user").await.unwrap();
    let tape = db
        .create_physical(
            uid, "cassette", "ABBA", "Gold", None, None, None, None, None, None, "[]", None, None,
            None, None,
        )
        .await
        .unwrap();
    let record = db
        .create_physical(
            uid, "vinyl", "Burial", "Untrue", None, None, None, None, None, None, "[]", None, None,
            None, None,
        )
        .await
        .unwrap();
    let disc = db
        .create_physical(
            uid,
            "cd",
            "Radiohead",
            "Pablo Honey",
            None,
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
        .unwrap();

    assert!(db.park_tape(uid, tape, "A", 754).await.unwrap(), "a cassette parks");
    assert!(!db.park_tape(uid, record, "A", 754).await.unwrap(), "a record does not");
    assert!(!db.park_tape(uid, disc, "1", 754).await.unwrap(), "nor does a disc");

    let shelf = db.list_physical(uid, None).await.unwrap();
    let got = |id: i64| shelf.iter().find(|r| r.id == id).unwrap();
    assert_eq!(got(tape).parked_side.as_deref(), Some("A"));
    assert_eq!(got(tape).parked_secs, Some(754));
    assert_eq!(got(record).parked_side, None);
    assert_eq!(got(record).parked_secs, None);

    // Playing it through winds it back to the start.
    assert!(db.rewind_tape(uid, tape).await.unwrap());
    let shelf = db.list_physical(uid, None).await.unwrap();
    assert_eq!(shelf.iter().find(|r| r.id == tape).unwrap().parked_side, None);
}

/// The whole reason variants exist: two cartridges on one turntable, and
/// the hours have to land on the one that was actually fitted.
///
/// A variant overriding a step means the gear it replaced accrues **no**
/// wear for those listens — an elliptical stylus does not wear down while
/// the microlinear is in the headshell. Everything else in the chain is
/// shared and keeps accruing from both.
#[tokio::test]
async fn a_variant_moves_the_wear_onto_the_gear_that_was_fitted() {
    let db = testdb().await;
    let uid = db.create_user("ashwin", None, "x", "user").await.unwrap();

    let deck = db.upsert_equipment(uid, "LP5X", "turntable", None, None, 0.0, None).await.unwrap();
    let ell =
        db.upsert_equipment(uid, "Elliptical", "cartridge", None, None, 0.0, None).await.unwrap();
    let micro =
        db.upsert_equipment(uid, "Microlinear", "cartridge", None, None, 0.0, None).await.unwrap();

    let chain = db
        .create_chain(&SignalChain {
            id: None,
            user_id: uid,
            name: "Vinyl".into(),
            description: None,
            components: vec![
                ChainComponent {
                    component_type: ComponentType::Source,
                    name: "LP5X".into(),
                    detail: None,
                    equipment_id: Some(deck),
                },
                ChainComponent {
                    component_type: ComponentType::Source,
                    name: "Elliptical".into(),
                    detail: None,
                    equipment_id: Some(ell),
                },
            ],
            listening_context: ListeningContext::Active,
            icon: None,
            is_active: true,
            shared: false,
            total_hours: 0.0,
            created_at: 0,
        })
        .await
        .unwrap();

    let variant = db
        .create_variant(
            uid,
            chain,
            "Microlinear",
            &[VariantOverride { position: 1, equipment_id: Some(micro), name: None }],
        )
        .await
        .unwrap();

    // An hour on the base chain, then an hour on the variant.
    for (i, v) in [(1u64, None), (2u64, Some(variant))] {
        let mut play = play("t", "a", 1_700_000_000 + i * 10_000);
        play.duration = Some(3600);
        play.source_id = format!("s{i}");
        db.save_scrobble(
            uid,
            &play,
            None,
            None,
            Some(chain),
            v,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped: false },
        )
        .await
        .unwrap();
    }

    let (secs, _) = db.equipment_seconds(uid).await.unwrap();
    // The turntable is in both, so it gets both hours.
    assert_eq!(secs.get(&deck), Some(&7200), "the deck ran for both");
    // Each cartridge only for the listens it was fitted for.
    assert_eq!(secs.get(&ell), Some(&3600), "the elliptical only for its own");
    assert_eq!(secs.get(&micro), Some(&3600), "the microlinear only for its own");
}

/// "Various Artists" is a filing convention, not a performer.
///
/// Every track of every compilation credits it, so it wins any superlative
/// by construction on a shelf with compilations on it — which is how a
/// month's report came to be headed by a string that says nothing about
/// that month. The real name is in `artist_credit`, which is what v0.22.1
/// kept it for.
#[tokio::test]
async fn a_compilation_credits_the_performer_not_the_placeholder() {
    let db = testdb().await;
    let uid = db.create_user("ashwin", None, "x", "user").await.unwrap();

    // Three compilation tracks by one performer, and two by someone real.
    for (i, credit) in [Some("Punch"), Some("Punch"), Some("Punch")].iter().enumerate() {
        let mut p = play("t", "Various Artists", 1_700_000_000 + i as u64 * 100);
        p.source_id = format!("va{i}");
        // Two names, because `artist_credit` is only written when the
        // source named more than one — a single artist needs no credit.
        p.artists = credit.map(|c| vec![c.to_string(), "Kim Na Young".into()]);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped: false },
        )
        .await
        .unwrap();
    }
    for i in 0..2 {
        let mut p = play("u", "Burial", 1_700_100_000 + i as u64 * 100);
        p.source_id = format!("b{i}");
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: false, skipped: false },
        )
        .await
        .unwrap();
    }

    let rows = db.report_rows(uid, 1000).await.unwrap();
    assert_eq!(rows.len(), 5);
    // The credit survives the round trip — without it there is nothing to
    // prefer, and the whole fix has no input.
    let va: Vec<_> = rows.iter().filter(|r| is_various_artists(&r.artist)).collect();
    assert_eq!(va.len(), 3);
    assert!(
        va.iter().all(|r| r.artist_credit.as_deref() == Some("Punch; Kim Na Young")),
        "the performer has to reach the report, or there is nothing to credit"
    );
    // The primary is what a report counts — the whole joined credit would
    // be its own key and no more useful than the placeholder it replaced.
    assert_eq!(
        va[0].artist_credit.as_deref().and_then(|c| c.split(';').next()).map(str::trim),
        Some("Punch")
    );
}

#[tokio::test]
async fn count_active_admins_excludes_disabled() {
    let db = testdb().await;
    let a1 = db.create_user("a1", None, "x", "admin").await.unwrap();
    db.create_user("a2", None, "x", "admin").await.unwrap();
    assert_eq!(db.count_active_admins().await.unwrap(), 2);
    db.set_user_disabled(a1, true).await.unwrap();
    assert_eq!(db.count_active_admins().await.unwrap(), 1);
}

#[tokio::test]
async fn delete_user_cascades_their_data() {
    let db = testdb().await;
    let uid = db.create_user("dave", None, "x", "user").await.unwrap();
    db.save_scrobble(
        uid,
        &play("s", "a", 5),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    db.create_token(uid, "t", "submit", None, false).await.unwrap();
    // A connection has an FK to users — deletion must clear it, or the whole
    // cascade rolls back (the bug this guards against).
    db.set_connection(uid, "listenbrainz", r#"{"token":"secret"}"#, Some("dave")).await.unwrap();

    assert!(db.delete_user(uid).await.unwrap());
    assert!(db.get_all_scrobbles(uid).await.unwrap().is_empty());
    assert!(db.list_users().await.unwrap().iter().all(|u| u.id != uid));
    assert!(db.list_tokens(uid).await.unwrap().is_empty());
    assert!(db.list_connections(uid).await.unwrap().is_empty());
}

#[tokio::test]
async fn interrupted_import_is_marked_on_startup() {
    let db = testdb().await;
    let uid = db.create_user("fay", None, "x", "admin").await.unwrap();
    // A job that started but never finished (process died) stays 'running'.
    let jid = db.create_import_job(uid, "file").await.unwrap();
    // A job that completed normally.
    let done = db.create_import_job(uid, "lastfm").await.unwrap();
    db.finish_import_job(done, "done", 10, 8, "ok").await.unwrap();

    let n = db.mark_running_imports_interrupted().await.unwrap();
    assert_eq!(n, 1, "only the unfinished job is touched");

    let imports = db.list_imports(uid).await.unwrap();
    let running = imports.iter().find(|r| r.id == jid).unwrap();
    assert_eq!(running.state, "interrupted");
    let finished = imports.iter().find(|r| r.id == done).unwrap();
    assert_eq!(finished.state, "done"); // untouched
}

#[tokio::test]
async fn connections_round_trip_and_hide_creds() {
    let db = testdb().await;
    let uid = db.create_user("erin", None, "x", "user").await.unwrap();
    db.set_connection(uid, "listenbrainz", r#"{"token":"secret"}"#, Some("erin_lb")).await.unwrap();

    // engine sees the credentials
    let fwd = db.get_connections_for_forwarding(uid).await.unwrap();
    assert_eq!(fwd, vec![("listenbrainz".to_string(), r#"{"token":"secret"}"#.to_string())]);

    // the public list exposes only service + account, never the token
    let list = db.list_connections(uid).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0, "listenbrainz");
    assert_eq!(list[0].1.as_deref(), Some("erin_lb"));

    assert!(db.delete_connection(uid, "listenbrainz").await.unwrap());
    assert!(db.list_connections(uid).await.unwrap().is_empty());
}

#[tokio::test]
async fn connection_credentials_are_encrypted_at_rest() {
    let db = testdb().await;
    let uid = db.create_user("frank", None, "x", "user").await.unwrap();
    db.set_connection(uid, "listenbrainz", r#"{"token":"PLAINTEXT_MARKER"}"#, None).await.unwrap();

    // The raw column must not contain the secret — a leaked backup is the
    // whole threat this defends against.
    let raw: (String,) =
        sqlx::query_as("SELECT credentials FROM user_connections WHERE user_id = ?")
            .bind(uid)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(!raw.0.contains("PLAINTEXT_MARKER"), "credential stored in the clear: {}", raw.0);
    assert!(SecretBox::is_encrypted(&raw.0));

    // …while the forwarding path still gets usable plaintext back.
    let fwd = db.get_connections_for_forwarding(uid).await.unwrap();
    assert_eq!(
        fwd,
        vec![("listenbrainz".to_string(), r#"{"token":"PLAINTEXT_MARKER"}"#.to_string())]
    );
}

#[tokio::test]
async fn source_credentials_round_trip_and_stay_sealed() {
    let db = testdb().await;
    let uid = db.create_user("grace", None, "x", "user").await.unwrap();
    db.create_source(
        uid,
        "plex",
        "Main",
        true,
        "http://plex.local:32400",
        r#"{"token":"PLEX_SECRET"}"#,
        Some("grace_plex"),
        None,
        "{}",
    )
    .await
    .unwrap();

    let raw: (String,) = sqlx::query_as("SELECT credentials FROM user_sources WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(!raw.0.contains("PLEX_SECRET"));

    let src = db.list_sources_for_user(uid).await.unwrap().remove(0);
    assert_eq!(src.credential("token").as_deref(), Some("PLEX_SECRET"));
    assert_eq!(src.remote_account.as_deref(), Some("grace_plex"));
    assert!(src.is_usable());
}

#[tokio::test]
async fn sources_are_scoped_per_user() {
    let db = testdb().await;
    let a = db.create_user("heidi", None, "x", "user").await.unwrap();
    let b = db.create_user("ivan", None, "x", "user").await.unwrap();
    let a_id = db
        .create_source(
            a,
            "plex",
            "A",
            true,
            "http://a",
            r#"{"token":"ta"}"#,
            Some("heidi_plex"),
            None,
            "{}",
        )
        .await
        .unwrap();
    let b_id = db
        .create_source(
            b,
            "plex",
            "B",
            true,
            "http://b",
            r#"{"token":"tb"}"#,
            Some("ivan_plex"),
            None,
            "{}",
        )
        .await
        .unwrap();

    // Each user sees only their own row…
    assert_eq!(db.list_sources_for_user(a).await.unwrap().len(), 1);
    assert_eq!(db.get_source(a, a_id).await.unwrap().unwrap().url, "http://a");
    assert!(
        db.get_source(b, b_id).await.unwrap().unwrap().credential("token").as_deref() == Some("tb")
    );
    // An id belonging to someone else is a miss, not a leak.
    assert!(db.get_source(a, b_id).await.unwrap().is_none());

    // …while the engine sees both, each carrying its owner.
    let all = db.list_enabled_sources().await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(all
        .iter()
        .any(|s| s.user_id == a && s.remote_account.as_deref() == Some("heidi_plex")));
    assert!(all.iter().any(|s| s.user_id == b && s.remote_account.as_deref() == Some("ivan_plex")));

    // Disabling drops it from the engine's view but keeps the config.
    db.update_source(
        a,
        a_id,
        "A",
        false,
        "http://a",
        r#"{"token":"ta"}"#,
        Some("heidi_plex"),
        None,
        "{}",
    )
    .await
    .unwrap();
    assert_eq!(db.list_enabled_sources().await.unwrap().len(), 1);
    assert!(db.get_source(a, a_id).await.unwrap().is_some());
}

#[tokio::test]
async fn several_sources_of_one_kind_coexist() {
    let db = testdb().await;
    let uid = db.create_user("kate", None, "x", "user").await.unwrap();

    let living = db
        .create_source(
            uid,
            "plex",
            "Living Room",
            true,
            "http://plex-a",
            r#"{"token":"ta"}"#,
            Some("kate"),
            None,
            "{}",
        )
        .await
        .unwrap();
    let study = db
        .create_source(
            uid,
            "plex",
            "Study",
            true,
            "http://plex-b",
            r#"{"token":"tb"}"#,
            Some("kate"),
            None,
            "{}",
        )
        .await
        .unwrap();
    let nav = db
        .create_source(
            uid,
            "navidrome",
            "NAS",
            true,
            "http://nd",
            r#"{"username":"kate","password":"pw"}"#,
            Some("kate"),
            None,
            "{}",
        )
        .await
        .unwrap();

    // Two Plex servers used to be impossible: the table had UNIQUE(user, kind).
    assert_ne!(living, study);
    assert_eq!(db.list_sources_for_user(uid).await.unwrap().len(), 3);
    assert_eq!(db.list_enabled_sources().await.unwrap().len(), 3);

    // Each keeps its own credentials and label.
    assert_eq!(
        db.get_source(uid, living).await.unwrap().unwrap().credential("token").as_deref(),
        Some("ta")
    );
    assert_eq!(
        db.get_source(uid, study).await.unwrap().unwrap().credential("token").as_deref(),
        Some("tb")
    );
    assert_eq!(db.get_source(uid, study).await.unwrap().unwrap().label, "Study");

    // Deleting one leaves its twin alone.
    assert!(db.delete_source(uid, living).await.unwrap());
    let left = db.list_sources_for_user(uid).await.unwrap();
    assert_eq!(left.len(), 2);
    assert!(left.iter().any(|s| s.id == study));
    assert!(left.iter().any(|s| s.id == nav));
}

#[tokio::test]
async fn a_listen_remembers_which_source_produced_it() {
    let db = testdb().await;
    let uid = db.create_user("liam", None, "x", "user").await.unwrap();
    let a = db
        .create_source(
            uid,
            "plex",
            "A",
            true,
            "http://a",
            r#"{"token":"ta"}"#,
            Some("liam"),
            None,
            "{}",
        )
        .await
        .unwrap();
    let b = db
        .create_source(
            uid,
            "plex",
            "B",
            true,
            "http://b",
            r#"{"token":"tb"}"#,
            Some("liam"),
            None,
            "{}",
        )
        .await
        .unwrap();

    db.save_scrobble(
        uid,
        &play("From A", "Artist", 100),
        None,
        None,
        None,
        None,
        None,
        Some("Plex"),
        Some(a),
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    db.save_scrobble(
        uid,
        &play("From B", "Artist", 200),
        None,
        None,
        None,
        None,
        None,
        Some("Plex"),
        Some(b),
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();

    // Without this the artwork proxy couldn't tell which of two Plex servers
    // to ask for a given listen's cover.
    let rows = db.get_all_scrobbles(uid).await.unwrap();
    let from_a = rows.iter().find(|r| r.title == "From A").unwrap();
    let from_b = rows.iter().find(|r| r.title == "From B").unwrap();
    assert_eq!(from_a.source_ref, Some(a));
    assert_eq!(from_b.source_ref, Some(b));
}

#[tokio::test]
async fn recording_a_poll_does_not_look_like_a_config_change() {
    let db = testdb().await;
    let uid = db.create_user("judy", None, "x", "user").await.unwrap();
    let sid = db
        .create_source(
            uid,
            "plex",
            "P",
            true,
            "http://p",
            r#"{"token":"t"}"#,
            Some("judy_plex"),
            None,
            "{}",
        )
        .await
        .unwrap();
    let before = db.get_source(uid, sid).await.unwrap().unwrap();

    db.record_source_poll(before.id, Some("boom")).await.unwrap();
    let after = db.get_source(uid, sid).await.unwrap().unwrap();

    // If a poll bumped updated_at the engine would rebuild the source every
    // single tick, reconnecting endlessly.
    assert_eq!(before.updated_at, after.updated_at);
    assert_eq!(after.last_error.as_deref(), Some("boom"));
    assert!(after.last_polled_at.is_some());
}

// ── Liner notes (Phase 7) ──

#[tokio::test]
async fn one_entity_per_spelling_and_the_mbid_is_only_an_alias() {
    let db = testdb().await;
    let first = db.upsert_entity("recording", "Xtal", "Aphex Twin", None).await.unwrap();
    // Same record typed differently is the same row...
    let again = db.upsert_entity("recording", " xtal ", "aphex twin", Some("mb-1")).await.unwrap();
    assert_eq!(first, again);
    // ...and learning the MBID later doesn't create a second one.
    assert_eq!(db.get_entity(first).await.unwrap().unwrap().mbid.as_deref(), Some("mb-1"));

    // A later lookup that knows nothing must not erase what we had.
    db.upsert_entity("recording", "Xtal", "Aphex Twin", None).await.unwrap();
    assert_eq!(db.get_entity(first).await.unwrap().unwrap().mbid.as_deref(), Some("mb-1"));

    // Same title by someone else is a different record.
    let other = db.upsert_entity("recording", "Xtal", "Some Coverband", None).await.unwrap();
    assert_ne!(first, other);
}

#[tokio::test]
async fn editing_a_note_keeps_what_it_said_before() {
    let db = testdb().await;
    let uid = db.create_user("ann", None, "x", "user").await.unwrap();
    let e = db.upsert_entity("release", "SAW 85-92", "Aphex Twin", None).await.unwrap();

    let id =
        db.upsert_note(uid, &NoteTarget::Entity(e), "first thoughts", "private").await.unwrap();
    let again =
        db.upsert_note(uid, &NoteTarget::Entity(e), "second thoughts", "public").await.unwrap();

    // One note per target — the second write edits rather than stacks.
    assert_eq!(id, again);
    assert_eq!(db.list_notes(uid).await.unwrap().len(), 1);
    let note = db.get_note(uid, id).await.unwrap().unwrap();
    assert_eq!(note.body_md, "second thoughts");
    assert_eq!(note.visibility, "public");

    // The earlier body survives the rewrite.
    let revs = db.note_revisions(uid, id).await.unwrap();
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].body_md, "first thoughts");

    // Changing only the visibility is not a rewrite.
    db.upsert_note(uid, &NoteTarget::Entity(e), "second thoughts", "private").await.unwrap();
    assert_eq!(db.note_revisions(uid, id).await.unwrap().len(), 1);
}

/// A week, a month or a year is the third thing a note can be about, and
/// the CHECK on `notes` is what keeps the three from being confused for one
/// another. The key is the note's *identity*, so a malformed one has to be
/// refused at the door rather than stored — a note filed under a key the
/// Reports page cannot produce is a note its author can never find again,
/// which looks from the outside like a save that silently did nothing.
/// The whole point: a performer gets credit.
///
/// Play "Hilary Hahn plays Bach" and both of them have played it. Before
/// `scrobble_artists`, every chart in the application grouped by the single
/// `scrobbles.artist` column, so Bach took the listen and Hilary Hahn was
/// written into `artist_credit` as text that exactly one query read.
///
/// Asserts on the *readouts* rather than on the SQL, for the same reason
/// `a_skip_reaches_no_counting_readout` does: that is the only form that
/// catches a query written later.
#[tokio::test]
async fn a_performer_is_credited_alongside_the_composer() {
    let db = testdb().await;
    let uid = db.create_user("kit", None, "x", "user").await.unwrap();

    // Three violin sonatas, credited to both. Plus one listen of something
    // else, so "share of your listening" has a denominator worth dividing.
    for (i, ts) in [1_000u64, 2_000, 3_000].iter().enumerate() {
        let mut p = play(&format!("Partita {i}"), "Johann Sebastian Bach", *ts);
        p.album = Some("Bach: Violin Concertos".into());
        p.duration = Some(300);
        p.artists = Some(vec!["Johann Sebastian Bach".into(), "Hilary Hahn".into()]);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }
    let mut other = play("Xtal", "Aphex Twin", 9_000);
    other.duration = Some(300);
    db.save_scrobble(
        uid,
        &other,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();

    // The artist page, for somebody who is never the primary.
    let hh = db.artist_summary(uid, "Hilary Hahn").await.unwrap().expect("she has a page");
    assert_eq!(hh.plays, 3, "every sonata she played counts as hers");
    assert_eq!(db.artist_plays(uid, "Hilary Hahn").await.unwrap(), 3);
    assert_eq!(db.artist_top_tracks(uid, "Hilary Hahn", 10).await.unwrap().len(), 3);
    assert_eq!(db.artist_albums(uid, "Hilary Hahn", None, 10).await.unwrap().len(), 1);
    assert_eq!(
        db.artist_trend(uid, "Hilary Hahn").await.unwrap().iter().map(|(_, n)| n).sum::<i64>(),
        3
    );

    // And the composer keeps his, undiminished — this adds a credit rather
    // than moving one.
    assert_eq!(db.artist_plays(uid, "Johann Sebastian Bach").await.unwrap(), 3);

    // The chart lists both, and `COUNT(*)` still means listens: a listen
    // crediting two artists must not count as two plays for either of them.
    let chart = db.chart(uid, "artists", None, 10, false).await.unwrap();
    let by_name = |n: &str| chart.iter().find(|r| r.name.eq_ignore_ascii_case(n));
    let bach = by_name("Johann Sebastian Bach").expect("Bach charts");
    let hahn = by_name("Hilary Hahn").expect("Hilary Hahn charts");
    assert_eq!(bach.plays, 3);
    assert_eq!(hahn.plays, 3);
    assert_eq!(by_name("Aphex Twin").unwrap().plays, 1);

    // Ranked among artists *heard*, not among record artists.
    let (rank, total) = db.artist_rank(uid, "Hilary Hahn").await.unwrap();
    assert!(rank > 0, "a performer has a rank at all");
    assert_eq!(total, 3, "Bach, Hilary Hahn and Aphex Twin");

    // An album is still the record's own, not the soloist's: fanning out
    // there would fold her albums into his.
    let album = db
        .album_summary(uid, "Johann Sebastian Bach", "Bach: Violin Concertos")
        .await
        .unwrap()
        .expect("the record");
    assert_eq!(album.plays, 3);
}

/// What a pushing client observed survives the round trip and reaches the
/// wire, and a client that says nothing costs nothing.
///
/// The reaching-the-wire half is the one worth pinning. These columns are
/// only useful if they come back out, and `ScrobbleRecord` is what
/// `/api/v1/scrobbles` serialises — a column added to the table but not to
/// that struct is written, stored, and invisible forever, with nothing
/// erroring to say so.
#[tokio::test]
async fn what_a_client_saw_of_the_playing_round_trips() {
    let db = testdb().await;
    let p = play("Xtal", "Aphex Twin", 1_700_000_000);
    assert!(db
        .save_scrobble(1, &p, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap());

    // A listen nobody reported this about stays null — not zero, which
    // would read as "heard none of it".
    let before =
        db.get_recent_scrobbles(1, 10, 0, None, None, None, None, None, None).await.unwrap();
    assert_eq!(before[0].listened_ms, None);
    assert_eq!(before[0].is_shuffle, None);
    assert_eq!(before[0].queue_source, None);

    db.set_listen_playback(
        1,
        &p.source_id,
        &p.source_name,
        &PlaybackReport {
            listened_ms: Some(3200),
            is_shuffle: Some(true),
            queue_source: Some("Late night".into()),
        },
    )
    .await
    .unwrap();

    let after =
        db.get_recent_scrobbles(1, 10, 0, None, None, None, None, None, None).await.unwrap();
    assert_eq!(after[0].listened_ms, Some(3200));
    assert_eq!(after[0].is_shuffle, Some(true));
    assert_eq!(after[0].queue_source.as_deref(), Some("Late night"));

    // A later report naming one field must not blank the other two —
    // that is what the COALESCE is for.
    db.set_listen_playback(
        1,
        &p.source_id,
        &p.source_name,
        &PlaybackReport { listened_ms: Some(9000), ..Default::default() },
    )
    .await
    .unwrap();
    let again =
        db.get_recent_scrobbles(1, 10, 0, None, None, None, None, None, None).await.unwrap();
    assert_eq!(again[0].listened_ms, Some(9000));
    assert_eq!(
        again[0].queue_source.as_deref(),
        Some("Late night"),
        "a partial report blanked the rest"
    );

    // An empty report issues no statement at all, which is what keeps this
    // free for every client that reports none of it.
    db.set_listen_playback(1, &p.source_id, &p.source_name, &PlaybackReport::default())
        .await
        .unwrap();
}

/// The same check `an_existing_database_opens_and_gains_period_notes`
/// exists for, aimed at the three columns this release adds. Every other
/// test here starts from an empty file and therefore never runs `migrate()`
/// against a real prior schema, which is exactly how a migration that
/// cannot run on an existing database passes the whole suite.
#[tokio::test]
async fn an_existing_database_gains_the_playback_columns() {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-oldplay-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }

    // A `scrobbles` table with a listen in it and none of the new columns.
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for ddl in [
            // The v0.65 shape, minus only the three columns this release
            // adds. Everything `migrate()` fills in on its own is left out
            // so those paths run too.
            "CREATE TABLE scrobbles (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER,
                     title TEXT NOT NULL, artist TEXT NOT NULL, artist_credit TEXT,
                     album TEXT, timestamp INTEGER NOT NULL, duration INTEGER,
                     source_id TEXT NOT NULL, source_name TEXT NOT NULL,
                     status TEXT NOT NULL DEFAULT 'pending',
                     skipped BOOLEAN NOT NULL DEFAULT FALSE,
                     caa_id INTEGER, caa_release_mbid TEXT,
                     language TEXT, genres TEXT, release_year INTEGER,
                     artwork_url TEXT, source_ref INTEGER, spotify_track_id TEXT,
                     container TEXT, source_quality TEXT,
                     dsd_to_pcm_converted BOOLEAN,
                     delivery_sample_rate INTEGER, delivery_bit_depth INTEGER,
                     chain_variant_id INTEGER,
                     UNIQUE(user_id, source_id, source_name))",
            "INSERT INTO scrobbles (user_id, title, artist, timestamp, source_id, source_name)
                     VALUES (1, 'Xtal', 'Aphex Twin', 1700000000, 'old-1', 'test')",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    let db = Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
        .await
        .expect("an existing database must open");

    // The listen that was already there is untouched and readable — which
    // also proves `SELECT *` still maps onto `ScrobbleRecord`.
    let rows = db.get_recent_scrobbles(1, 10, 0, None, None, None, None, None, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Xtal");
    assert_eq!(rows[0].listened_ms, None, "an old listen cannot have reported this");

    // And the columns the migration was for actually work afterwards.
    db.set_listen_playback(
        1,
        "old-1",
        "test",
        &PlaybackReport { listened_ms: Some(1500), ..Default::default() },
    )
    .await
    .unwrap();
    let rows = db.get_recent_scrobbles(1, 10, 0, None, None, None, None, None, None).await.unwrap();
    assert_eq!(rows[0].listened_ms, Some(1500));
}

/// The MBID queue asks once per track, not once per play.
///
/// A song played fifty times is fifty rows and **one** answer, and this is
/// a rate-limited lookup — asking per row would spend a whole batch on one
/// track. Also pins that an imported listen is in the queue at all, which
/// is the gap this exists for: only `flush_pending` ever wrote a recording
/// MBID, and it never sees a listen that arrived as `'imported'`.
#[tokio::test]
async fn the_mbid_queue_asks_once_per_track_and_includes_imports() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let now = Database::now_secs() as u64;

    for i in 0..5 {
        let mut p = Play::new(
            "Xtal".into(),
            "Aphex Twin".into(),
            now - i * 300,
            format!("s-{i}"),
            "import".into(),
        );
        p.album = Some("Selected Ambient Works 85-92".into());
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition { imported: true, skipped: false },
        )
        .await
        .unwrap();
    }
    // A track that already has one must not be queued again.
    let mut known =
        Play::new("Ageispolis".into(), "Aphex Twin".into(), now, "k".into(), "import".into());
    known.mbid_recording = Some("already-known".into());
    db.save_scrobble(
        uid,
        &known,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition { imported: true, skipped: false },
    )
    .await
    .unwrap();

    let queue = db.tracks_needing_recording_mbid(50).await.unwrap();
    assert_eq!(queue.len(), 1, "five plays of one track are one question: {queue:?}");
    assert_eq!(queue[0].0, "Xtal");
    assert_eq!(
        queue[0].2.as_deref(),
        Some("Selected Ambient Works 85-92"),
        "album rides along as a search hint"
    );

    // Coverage counts *listens*, and the population excludes nothing but skips.
    let (total, resolved) = db.recording_mbid_coverage(uid).await.unwrap();
    assert_eq!((total, resolved), (6, 1));

    // Resolving it once settles every play of it.
    let mut found = Play::new("Xtal".into(), "Aphex Twin".into(), now, String::new(), "mb".into());
    found.mbid_recording = Some("the-recording".into());
    let n = db.set_recording_mbid_for_track("Xtal", "Aphex Twin", &found).await.unwrap();
    assert_eq!(n, 5, "one lookup, five listens");

    let (total, resolved) = db.recording_mbid_coverage(uid).await.unwrap();
    assert_eq!((total, resolved), (6, 6));
    assert!(db.tracks_needing_recording_mbid(50).await.unwrap().is_empty());
}

/// The resolution cache, and the one rule that makes it worth having.
///
/// A push is one search per track and the far end meters them, so a
/// re-push should cost almost nothing. But the half that has to hold is
/// **a hand-picked match outranking a guess**: somebody read the candidates
/// and settled it, and the search that already got it wrong once must not
/// be given another vote on the next push.
#[tokio::test]
async fn a_hand_picked_match_outlives_every_guess_after_it() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let get = |a: &'static str, t: &'static str| db.playlist_match(uid, -1, a, t);

    assert_eq!(get("SE SO NEON", "A Long Dream").await.unwrap(), None);

    // What the matcher found. Remembered, so the next push does not ask.
    db.remember_playlist_match(uid, -1, "SE SO NEON", "A Long Dream", "t-1", None, false)
        .await
        .unwrap();
    assert_eq!(get("SE SO NEON", "A Long Dream").await.unwrap().as_deref(), Some("t-1"));

    // The same fold `playlists::pick` matches with, so a cache hit and a
    // match cannot disagree about whether two spellings are one track.
    assert_eq!(get("se so neon", "a  long dream").await.unwrap().as_deref(), Some("t-1"));

    // A person corrects it from the offered candidates.
    db.remember_playlist_match(uid, -1, "SE SO NEON", "A Long Dream", "t-2", Some("x"), true)
        .await
        .unwrap();
    assert_eq!(get("SE SO NEON", "A Long Dream").await.unwrap().as_deref(), Some("t-2"));

    // …and the next push's guess must not quietly undo it.
    db.remember_playlist_match(uid, -1, "SE SO NEON", "A Long Dream", "t-3", None, false)
        .await
        .unwrap();
    assert_eq!(
        get("SE SO NEON", "A Long Dream").await.unwrap().as_deref(),
        Some("t-2"),
        "an automatic guess overwrote a hand-picked match"
    );

    // Forgetting it sends the next push back to searching.
    db.forget_playlist_match(uid, -1, "SE SO NEON", "A Long Dream").await.unwrap();
    assert_eq!(get("SE SO NEON", "A Long Dream").await.unwrap(), None);
}

/// A guess goes stale; a decision does not.
///
/// A catalogue can pull a track, and a cached id would then be added to
/// every future playlist and rejected by the far end with nothing saying
/// why — so an automatic entry expires. A hand-picked one does not, because
/// re-asking a search that was already corrected throws the correction away
/// on a timer.
#[tokio::test]
async fn a_guess_goes_stale_and_a_decision_does_not() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let long_ago = Database::now_secs() - Database::MATCH_TTL_SECS - 1;

    for (artist, origin) in [("Guessed", "auto"), ("Chosen", "manual")] {
        sqlx::query(
            "INSERT INTO playlist_matches
                     (user_id, source_id, artist_key, title_key, remote_id, origin, resolved_at)
                 VALUES (?, -1, ?, ?, 'old', ?, ?)",
        )
        .bind(uid)
        .bind(normalise_artist(artist))
        .bind(normalise_artist("Song"))
        .bind(origin)
        .bind(long_ago)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    assert_eq!(db.playlist_match(uid, -1, "Guessed", "Song").await.unwrap(), None);
    assert_eq!(db.playlist_match(uid, -1, "Chosen", "Song").await.unwrap().as_deref(), Some("old"));
}

/// A re-push rewrites the playlist it made last time.
///
/// Keyed on the **name** rather than a local playlist id, because most of
/// what gets pushed has none — a Rediscovery collection is computed on
/// demand. Three pushes of one list used to make three playlists on the far
/// end, cleaned up by hand.
#[tokio::test]
async fn a_second_push_of_one_name_owns_the_same_remote_playlist() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    assert_eq!(db.pushed_playlist(uid, -1, "Dusty Vinyl").await.unwrap(), None);

    db.remember_pushed_playlist(uid, -1, "Dusty Vinyl", "tidal-abc").await.unwrap();
    assert_eq!(
        db.pushed_playlist(uid, -1, "Dusty Vinyl").await.unwrap().as_deref(),
        Some("tidal-abc")
    );
    // The name is matched the way a person types it, not byte for byte.
    assert_eq!(
        db.pushed_playlist(uid, -1, "  dusty vinyl ").await.unwrap().as_deref(),
        Some("tidal-abc")
    );

    // The same list sent to a different target remembers its own, or
    // pushing to Plex would rewrite the TIDAL one.
    assert_eq!(db.pushed_playlist(uid, 7, "Dusty Vinyl").await.unwrap(), None);
    db.remember_pushed_playlist(uid, 7, "Dusty Vinyl", "plex-99").await.unwrap();
    assert_eq!(
        db.pushed_playlist(uid, -1, "Dusty Vinyl").await.unwrap().as_deref(),
        Some("tidal-abc")
    );

    // A playlist deleted on the far end and remade must not leave the old
    // id behind — the mapping is replaced, never accumulated.
    db.remember_pushed_playlist(uid, -1, "Dusty Vinyl", "tidal-xyz").await.unwrap();
    assert_eq!(
        db.pushed_playlist(uid, -1, "Dusty Vinyl").await.unwrap().as_deref(),
        Some("tidal-xyz")
    );
}

/// An existing database gains the Patch schema and the sharing switch.
///
/// Two things this pins, and both are the v0.61.1 trap in waiting.
/// `signal_chains.shared` is a migrated column, so anything in `init()`
/// that *references* it — an index, a partial index, a view — fails on
/// every database that has data in it while passing on a fresh one. And
/// the Patch tables are created in `init()` with `IF NOT EXISTS`, which is
/// only safe because they are wholly new: none of them may ever come to
/// depend on a column `migrate()` adds.
///
/// The default is the half worth asserting. `shared` must come back
/// **false** for a chain that predates the column: nobody consents to
/// publishing their listening by upgrading a binary.
#[tokio::test]
async fn an_existing_database_gains_the_patch_schema() {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-oldpatch-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }

    // A chain from before any of this existed, with hours on it.
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     username TEXT UNIQUE NOT NULL, display_name TEXT,
                     password_hash TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'user',
                     created_at INTEGER NOT NULL)",
            "INSERT INTO users (id, username, password_hash, created_at)
                     VALUES (1, 'ash', 'x', 1700000000)",
            "CREATE TABLE signal_chains (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     user_id INTEGER NOT NULL REFERENCES users(id),
                     name TEXT NOT NULL, description TEXT,
                     components JSON NOT NULL DEFAULT '[]',
                     listening_context TEXT NOT NULL DEFAULT 'unknown',
                     is_active BOOLEAN DEFAULT TRUE, total_hours REAL DEFAULT 0,
                     created_at INTEGER NOT NULL, UNIQUE(user_id, name))",
            "INSERT INTO signal_chains (user_id, name, created_at)
                     VALUES (1, 'Desk Reference', 1700000000)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    let db = Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
        .await
        .expect("an existing database must open");

    // The chain that was already there kept its name and is **not** shared.
    let (name, shared): (String, bool) =
        sqlx::query_as("SELECT name, shared FROM signal_chains WHERE user_id = 1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(name, "Desk Reference");
    assert!(!shared, "a chain that predates the column must not be published by an upgrade");

    // Nobody is published by an upgrade. This is the half that only a
    // migrated database can prove: on a fresh one the column comes from
    // `init()` and its default is never tested against a row that already
    // existed.
    let (published,): (i64,) = sqlx::query_as("SELECT federation_enabled FROM users WHERE id = 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        published, 0,
        "an account that predates the column must not become discoverable by an upgrade"
    );

    // And every Patch table is there to be written to.
    for table in [
        "actors",
        "remote_actors",
        "patches",
        "activities",
        "activity_deliveries",
        "inbox_activities",
    ] {
        let (n,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
                .bind(table)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(n, 1, "{} must exist on a migrated database", table);
    }

    // A patch with both ends named on one side is refused by the CHECK,
    // which is what stops an edge that means two different things at once.
    let bad = sqlx::query(
        "INSERT INTO patches (uuid, follower_user_id, follower_actor_id, target_user_id,
                                  state, created_at, updated_at)
             VALUES ('u1', 1, 1, 1, 'pending', 1, 1)",
    )
    .execute(&db.pool)
    .await;
    assert!(bad.is_err(), "a patch cannot have a local and a remote follower");
}

// ── Patch: the actor layer ───────────────────────────────

/// The identity is minted once and then it is a fact about the past.
///
/// The trap this pins is the reason `actors.uri` is a stored column rather
/// than something derived on read: the origin an actor was minted under is
/// the address remote servers have cached, so recomputing it from today's
/// `PUBLIC_URL` would silently rename an actor other people follow — and to
/// them a rename is not a rename, it is the old actor vanishing and a
/// stranger appearing.
#[tokio::test]
async fn an_actor_is_minted_once_and_keeps_the_uri_it_was_minted_with() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let first = db.actor_for_user(uid, "https://deck.example").await.unwrap();
    assert_eq!(first.uri, "https://deck.example/users/ash");
    assert!(first.public_key.starts_with("-----BEGIN PUBLIC KEY-----"));
    assert!(
        first.private_key.starts_with("-----BEGIN PRIVATE KEY-----"),
        "unsealed on the way out"
    );
    assert!(first.frozen_at.is_none(), "a fresh identity has not left the building");

    // The operator changes PUBLIC_URL. The actor does not move.
    let again = db.actor_for_user(uid, "https://somewhere-else.example").await.unwrap();
    assert_eq!(again.uri, first.uri, "the URI is written once, never re-derived");
    assert_eq!(again.public_key, first.public_key, "and the key is not regenerated");
    assert_eq!(again.private_key, first.private_key);
}

/// The private half is a credential and goes through `seal` like every
/// other replayed secret, so ciphertext never leaves this file. Read
/// straight off the column to prove it — going through `actor_for_user`
/// would only prove the round trip works.
#[tokio::test]
async fn the_private_key_is_sealed_in_the_column() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let actor = db.actor_for_user(uid, "https://deck.example").await.unwrap();

    let stored: String = sqlx::query_scalar("SELECT private_key FROM actors WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    assert!(stored.starts_with("tdenc:v1:"), "the key at rest is ciphertext");
    assert!(!stored.contains("BEGIN PRIVATE KEY"), "and no part of the PEM survives in it");
    assert_ne!(stored, actor.private_key);

    // The published half is *not* sealed: it is published.
    let public: String = sqlx::query_scalar("SELECT public_key FROM actors WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(public, actor.public_key);
}

/// **Phrased about the answer, not about the query.**
///
/// These are the only unauthenticated read paths on the instance besides
/// the now-playing widget, so anything that tells "no such person" apart
/// from "that person is not published" turns a private, invite-only server
/// into a membership oracle. A later rewrite that splits the two cases
/// apart is a reasonable-looking change and a leak, which is why the
/// assertion is that the three outcomes are *equal* rather than that each
/// is `None`.
#[tokio::test]
async fn an_undiscoverable_actor_and_an_unknown_one_are_the_same_answer() {
    let db = testdb().await;
    let quiet = db.create_user("ash", None, "pw", "user").await.unwrap();
    let gone = db.create_user("kim", None, "pw", "user").await.unwrap();
    db.set_federation_enabled(gone, true).await.unwrap();
    sqlx::query("UPDATE users SET disabled = 1 WHERE id = ?")
        .bind(gone)
        .execute(&db.pool)
        .await
        .unwrap();

    let unknown = db.discoverable_user_id("nobody-here").await.unwrap();
    let not_published = db.discoverable_user_id("ash").await.unwrap();
    let disabled = db.discoverable_user_id("kim").await.unwrap();

    assert_eq!(not_published, unknown, "an account that has not switched federation on must be indistinguishable from one that does not exist");
    assert_eq!(disabled, unknown, "and so must a disabled one");

    // And the switch really is the only thing standing between them.
    db.set_federation_enabled(quiet, true).await.unwrap();
    assert_eq!(db.discoverable_user_id("ash").await.unwrap(), Some(quiet));
    // Case-insensitive, because a handle typed into another server's search
    // box arrives however the person typed it.
    assert_eq!(db.discoverable_user_id("ASH").await.unwrap(), Some(quiet));
}

/// Nobody is published by their operator upgrading a binary.
#[tokio::test]
async fn federation_is_off_until_someone_turns_it_on() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let on: i64 = sqlx::query_scalar("SELECT federation_enabled FROM users WHERE id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(on, 0, "a new account is not discoverable");
    assert_eq!(db.discoverable_user_id("ash").await.unwrap(), None);

    db.set_federation_enabled(uid, true).await.unwrap();
    assert_eq!(db.discoverable_user_id("ash").await.unwrap(), Some(uid));
    db.set_federation_enabled(uid, false).await.unwrap();
    assert_eq!(db.discoverable_user_id("ash").await.unwrap(), None, "and it can be taken back");
}

/// The first stamp is the true one. Re-stamping would move a date that
/// other servers' copies of this identity are dated against.
#[tokio::test]
async fn the_username_freeze_records_the_first_moment_and_not_the_latest() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    db.actor_for_user(uid, "https://deck.example").await.unwrap();

    assert!(!db.username_is_frozen(uid).await.unwrap());

    db.freeze_actor_username(uid).await.unwrap();
    assert!(db.username_is_frozen(uid).await.unwrap());
    let first: Option<i64> = sqlx::query_scalar("SELECT frozen_at FROM actors WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    db.freeze_actor_username(uid).await.unwrap();
    let after: Option<i64> = sqlx::query_scalar("SELECT frozen_at FROM actors WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(after, first, "freezing twice must not move the date");

    // A user with no actor at all is not frozen, and asking does not error.
    let other = db.create_user("kim", None, "pw", "user").await.unwrap();
    assert!(!db.username_is_frozen(other).await.unwrap());
}

/// Senders retry. A redelivery is the *same* activity, and acting on one
/// Follow twice is how one follower becomes two — so the second arrival is
/// a no-op rather than a second row or an error.
#[tokio::test]
async fn a_redelivered_activity_is_filed_once() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let remote = db
        .upsert_remote_actor(
            "https://them.example/users/kim",
            Some("kim@them.example"),
            Some("Kim"),
            "https://them.example/users/kim/inbox",
            None,
            Some("-----BEGIN PUBLIC KEY-----\nx\n-----END PUBLIC KEY-----\n"),
            None,
        )
        .await
        .unwrap();

    let file = || {
        db.record_inbox_activity(
            uid,
            remote.id,
            "https://them.example/activities/1",
            Some("Follow"),
            Some("https://deck.example/users/ash"),
            r#"{"type":"Follow"}"#,
        )
    };

    assert!(file().await.unwrap(), "the first delivery is new");
    assert!(!file().await.unwrap(), "a redelivery is not");

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox_activities")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

/// Re-fetching an actor updates it; it does not accumulate copies. The
/// interesting half is the key: this is the path a rotation arrives on, and
/// leaving the old key would make the peer permanently unable to talk to us.
#[tokio::test]
async fn refetching_an_actor_updates_it_including_the_key() {
    let db = testdb().await;
    let put = |key: &'static str, name: &'static str| {
        db.upsert_remote_actor(
            "https://them.example/users/kim",
            Some("kim@them.example"),
            Some(name),
            "https://them.example/users/kim/inbox",
            Some("https://them.example/inbox"),
            Some(key),
            None,
        )
    };

    let first = put("KEY-ONE", "Kim").await.unwrap();
    let second = put("KEY-TWO", "Kim Renamed").await.unwrap();

    assert_eq!(first.id, second.id, "one row per actor URI");
    assert_eq!(second.public_key.as_deref(), Some("KEY-TWO"), "a rotated key replaces the old one");
    assert_eq!(second.name.as_deref(), Some("Kim Renamed"));
    assert_eq!(second.shared_inbox.as_deref(), Some("https://them.example/inbox"));

    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM remote_actors").fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1);
}

/// **An outbox is fetched by strangers with no credential**, so the
/// visibility filter is the whole of the access control — and it is in the
/// query rather than in the handler because nothing downstream would notice
/// a private activity arriving in a public collection.
#[tokio::test]
async fn an_outbox_publishes_only_what_is_public() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let other = db.create_user("kim", None, "pw", "user").await.unwrap();

    for (owner, uuid, visibility, published) in [
        (uid, "a-public", "public", 300),
        (uid, "a-patched", "patched", 200),
        (uid, "a-private", "private", 100),
        (other, "somebody-elses", "public", 400),
    ] {
        sqlx::query(
            "INSERT INTO activities (uuid, user_id, kind, payload, published, visibility)
                 VALUES (?, ?, 'Create', ?, ?, ?)",
        )
        .bind(uuid)
        .bind(owner)
        .bind(format!(r#"{{"id":"{uuid}"}}"#))
        .bind(published)
        .bind(visibility)
        .execute(&db.pool)
        .await
        .unwrap();
    }

    let (total, items) = db.public_outbox(uid, 40).await.unwrap();
    assert_eq!(total, 1, "patched-in and private activities are not published");
    assert_eq!(items.len(), 1);
    assert!(items[0].contains("a-public"));
    assert!(!items.iter().any(|i| i.contains("somebody-elses")), "and neither is anyone else's");

    // Newest first, and empty is a normal state rather than an error.
    assert_eq!(db.public_outbox(other, 40).await.unwrap().0, 1);
    let nobody = db.create_user("sam", None, "pw", "user").await.unwrap();
    assert_eq!(db.public_outbox(nobody, 40).await.unwrap(), (0, vec![]));
}

// ── Patch: patches and the Reel ──────────────────────────

/// Helper: a user with one chain, and a listen through it.
async fn deck_with_listen(
    db: &Database,
    name: &str,
    chain: &str,
    title: &str,
    ts: u64,
) -> (i64, i64) {
    let uid = db.create_user(name, None, "pw", "user").await.unwrap();
    // Closed, stated rather than inherited — see `patched_pair`. This deck
    // exists to test the chain gate, so it must not take whatever
    // `create_user` defaults to.
    db.set_share_all_with_patched(uid, false).await.unwrap();
    let chain_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(uid)
    .bind(chain)
    .bind(Database::now_secs())
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let p = play(title, "Massive Attack", ts);
    db.save_scrobble(
        uid,
        &p,
        None,
        None,
        Some(chain_id),
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    (uid, chain_id)
}

/// The whole request → accept → read cycle, and the states it passes
/// through. **A patch is a request, not a subscription**, so a pending one
/// must read nothing at all.
#[tokio::test]
async fn a_patch_reads_nothing_until_it_is_accepted() {
    let db = testdb().await;
    let (them, chain) =
        deck_with_listen(&db, "kim", "Study Chain", "Teardrop", 1_760_000_000).await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    db.set_chain_shared(them, chain, true).await.unwrap();

    // No patch at all.
    assert!(db.reel(me, 40, None, None, None).await.unwrap().is_empty());

    let uuid = db.request_patch(me, them).await.unwrap();
    assert!(
        db.reel(me, 40, None, None, None).await.unwrap().is_empty(),
        "a pending patch reads nothing — that is what makes it a request"
    );

    assert!(db.settle_patch(&uuid, them, true).await.unwrap());
    let reel = db.reel(me, 40, None, None, None).await.unwrap();
    assert_eq!(reel.len(), 1);
    assert_eq!(reel[0].title, "Teardrop");
    assert_eq!(reel[0].username, "kim");
    assert_eq!(reel[0].chain_name.as_deref(), Some("Study Chain"), "a shared chain is still named");

    // And withdrawing takes it away again.
    assert!(db.unpatch(me, them).await.unwrap());
    assert!(db.reel(me, 40, None, None, None).await.unwrap().is_empty());
}

/// **The sharing unit is the chain.** A patch into a deck that publishes
/// nothing is accepted and reads empty, which is correct — and a listen
/// through an unshared chain never appears however friendly the patch.
#[tokio::test]
async fn only_a_shared_chain_reaches_the_reel() {
    let db = testdb().await;
    let (them, shared) =
        deck_with_listen(&db, "kim", "Study Chain", "Teardrop", 1_760_000_100).await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();

    // A second chain on the same deck, left private.
    let private = sqlx::query_scalar::<_, i64>(
            "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Bedroom', ?) RETURNING id",
        )
        .bind(them)
        .bind(Database::now_secs())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let p = play("Angel", "Massive Attack", 1_760_000_200);
    db.save_scrobble(
        them,
        &p,
        None,
        None,
        Some(private),
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();

    let uuid = db.request_patch(me, them).await.unwrap();
    db.settle_patch(&uuid, them, true).await.unwrap();

    assert!(
        db.reel(me, 40, None, None, None).await.unwrap().is_empty(),
        "nothing is shared until a chain is switched on, even with the patch accepted"
    );

    db.set_chain_shared(them, shared, true).await.unwrap();
    let reel = db.reel(me, 40, None, None, None).await.unwrap();
    assert_eq!(reel.len(), 1, "only the shared chain's listen");
    assert_eq!(reel[0].title, "Teardrop");

    // Switching it back off takes it out again — the switch is the whole
    // control, and it works in both directions.
    db.set_chain_shared(them, shared, false).await.unwrap();
    assert!(db.reel(me, 40, None, None, None).await.unwrap().is_empty());
}

/// **A listen with no chain is never shared**, and this is the clause most
/// likely to be "tidied" into a LEFT JOIN by somebody wondering why
/// unattributed listens are missing. The default has to be exclusion:
/// nobody switched anything on for a listen that ran through nothing.
///
/// A skip is not shown either — stored, never counted, and certainly not
/// published to another person.
#[tokio::test]
async fn a_chainless_listen_and_a_skip_never_reach_the_reel() {
    let db = testdb().await;
    let (them, chain) =
        deck_with_listen(&db, "kim", "Study Chain", "Teardrop", 1_760_000_000).await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    db.set_chain_shared(them, chain, true).await.unwrap();

    // No chain at all.
    let orphan = play("Unattributed", "Massive Attack", 1_760_000_300);
    db.save_scrobble(
        them,
        &orphan,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();

    // A skip, through the shared chain.
    let skipped = play("Bailed On", "Massive Attack", 1_760_000_400);
    db.save_scrobble(
        them,
        &skipped,
        None,
        None,
        Some(chain),
        None,
        None,
        None,
        None,
        ListenDisposition { imported: false, skipped: true },
    )
    .await
    .unwrap();

    let uuid = db.request_patch(me, them).await.unwrap();
    db.settle_patch(&uuid, them, true).await.unwrap();

    let titles: Vec<_> =
        db.reel(me, 40, None, None, None).await.unwrap().into_iter().map(|r| r.title).collect();
    assert_eq!(titles, ["Teardrop"], "no chain means not shared; a skip is never published");
}

/// Asking twice must not mint a second edge or disturb an accepted one —
/// and this is the test that catches the partial-index `ON CONFLICT`,
/// which is a runtime failure that compiles perfectly.
#[tokio::test]
async fn asking_again_neither_duplicates_nor_un_approves() {
    let db = testdb().await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    let them = db.create_user("kim", None, "pw", "user").await.unwrap();

    let first = db.request_patch(me, them).await.unwrap();
    let again = db.request_patch(me, them).await.unwrap();
    assert_eq!(first, again, "one edge per pair, whoever asks how often");

    db.settle_patch(&first, them, true).await.unwrap();
    db.request_patch(me, them).await.unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM patches WHERE uuid = ?")
        .bind(&first)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(state, "accepted", "asking again must not un-approve a patch already said yes to");

    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM patches").fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1);
}

/// **Only the target may settle a patch**, and the check is in the `WHERE`
/// rather than in the handler — this is the one operation that widens who
/// may read somebody's listening, and an authorisation check that lives in
/// a handler is one a second handler can forget.
#[tokio::test]
async fn only_the_target_can_accept_a_patch() {
    let db = testdb().await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    let them = db.create_user("kim", None, "pw", "user").await.unwrap();
    let bystander = db.create_user("sam", None, "pw", "user").await.unwrap();

    let uuid = db.request_patch(me, them).await.unwrap();

    assert!(
        !db.settle_patch(&uuid, bystander, true).await.unwrap(),
        "a stranger cannot approve it"
    );
    assert!(
        !db.settle_patch(&uuid, me, true).await.unwrap(),
        "and neither can the person who asked"
    );

    let state: String = sqlx::query_scalar("SELECT state FROM patches WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(state, "pending", "nothing moved");

    assert!(db.settle_patch(&uuid, them, true).await.unwrap());
    assert!(!db.settle_patch(&uuid, them, false).await.unwrap(), "and it settles once");
}

/// Accepting is the moment the identity becomes load-bearing for somebody
/// else, which is the trigger `frozen_at` exists for — *not* going public,
/// because in this design following happens while private.
#[tokio::test]
async fn the_first_accepted_patch_freezes_the_username() {
    let db = testdb().await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    let them = db.create_user("kim", None, "pw", "user").await.unwrap();
    db.actor_for_user(them, "https://deck.example").await.unwrap();

    let uuid = db.request_patch(me, them).await.unwrap();
    assert!(!db.username_is_frozen(them).await.unwrap(), "a request is not a commitment");

    db.settle_patch(&uuid, them, true).await.unwrap();
    assert!(db.username_is_frozen(them).await.unwrap());

    // And a deck that gains its identity *after* being patched into is
    // frozen at that moment — otherwise the rule would depend on the
    // unrelated question of when they switched federation on.
    let late = db.create_user("bo", None, "pw", "user").await.unwrap();
    let yes = db.request_patch(me, late).await.unwrap();
    db.settle_patch(&yes, late, true).await.unwrap();
    assert!(!db.username_is_frozen(late).await.unwrap(), "no actor yet, nothing to freeze");
    db.actor_for_user(late, "https://deck.example").await.unwrap();
    assert!(
        db.username_is_frozen(late).await.unwrap(),
        "an identity minted for a deck that already has followers is born frozen"
    );

    // Rejecting does not freeze: nobody ended up holding the identity.
    let other = db.create_user("sam", None, "pw", "user").await.unwrap();
    db.actor_for_user(other, "https://deck.example").await.unwrap();
    let no = db.request_patch(me, other).await.unwrap();
    db.settle_patch(&no, other, false).await.unwrap();
    assert!(!db.username_is_frozen(other).await.unwrap());
}

/// Both directions of the list, and the shape that tells "not patched"
/// apart from "asked and waiting".
#[tokio::test]
async fn the_two_lists_describe_the_same_people_the_same_way() {
    let db = testdb().await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    let them = db.create_user("kim", None, "pw", "user").await.unwrap();
    let uuid = db.request_patch(me, them).await.unwrap();

    let out = db.patches_out(me).await.unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].username, "kim");
    assert_eq!(out[0].state.as_deref(), Some("pending"));

    let inbound = db.patches_in(them).await.unwrap();
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].username, "ash");
    assert_eq!(inbound[0].state.as_deref(), Some("pending"));
    assert_eq!(inbound[0].uuid.as_deref(), Some(uuid.as_str()));

    // A deck nobody has asked about carries no edge at all, which is what
    // the UI reads to offer "Patch in" rather than "Pending".
    let decks = db.patchable_decks(me).await.unwrap();
    assert!(decks.iter().all(|d| d.username != "ash"), "you cannot patch into yourself");
    let kim = decks.iter().find(|d| d.username == "kim").expect("kim is patchable");
    assert!(kim.uuid.is_none() && kim.state.is_none());
}

/// A deck cannot patch into itself, and the refusal is at the door rather
/// than left to a UNIQUE constraint to notice.
#[tokio::test]
async fn a_deck_cannot_patch_into_itself() {
    let db = testdb().await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    assert!(db.request_patch(me, me).await.is_err());
}

/// What the actor's collections count.
///
/// **Accepted patches, not actors.** The obvious version joins `actors` to
/// collect each end's URI and counts those — and a local follower who never
/// switched federation on has no actor row, so a deck with a follower
/// reported zero. That would make the number a fact about who enabled
/// federation rather than about followers, which is meaningless to the
/// remote software that reads it. Caught on a running instance, not here.
#[tokio::test]
async fn the_actor_collections_count_patches_rather_than_identities() {
    let db = testdb().await;
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    let them = db.create_user("kim", None, "pw", "user").await.unwrap();
    let quiet = db.create_user("sam", None, "pw", "user").await.unwrap();
    db.actor_for_user(them, "https://deck.example").await.unwrap();
    // Neither `me` nor `quiet` has an actor, on purpose.

    let a = db.request_patch(me, them).await.unwrap();
    db.settle_patch(&a, them, true).await.unwrap();
    let b = db.request_patch(quiet, them).await.unwrap();
    db.settle_patch(&b, them, true).await.unwrap();
    // A third, left pending.
    db.request_patch(me, quiet).await.unwrap();

    assert_eq!(
        db.patch_count(them, true).await.unwrap(),
        2,
        "both followers count, whether or not they have an identity of their own"
    );
    assert_eq!(db.patch_count(me, false).await.unwrap(), 1, "the pending one is not following");
    assert_eq!(db.patch_count(quiet, true).await.unwrap(), 0, "pending is not accepted");
}

/// Editing a chain must not un-publish it.
///
/// `update_chain` is a whole-record replace of everything the editor owns,
/// and sharing is not the editor's — it is switched from the drawer or from
/// Patch → Decks. Adding `shared` to that statement would take a chain off
/// the Reel as a side effect of renaming it, which is exactly the shape
/// that silently erased gear notes in v0.50.0. Nothing errors; a chain just
/// stops publishing.
#[tokio::test]
async fn renaming_a_chain_does_not_unshare_it() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let chain_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Desk', ?) RETURNING id",
    )
    .bind(uid)
    .bind(Database::now_secs())
    .fetch_one(&db.pool)
    .await
    .unwrap();

    db.set_chain_shared(uid, chain_id, true).await.unwrap();
    db.update_chain(uid, chain_id, "Desk Reference", None, &[], "focused", None).await.unwrap();

    let chains = db.get_chains(uid).await.unwrap();
    let c = chains.iter().find(|c| c.id == Some(chain_id)).expect("the chain");
    assert_eq!(c.name, "Desk Reference");
    assert!(c.shared, "renaming a chain must not take it off the Reel");

    // And a brand new chain publishes nothing. Through the real API rather
    // than a raw INSERT: `create_chain` is the path a user takes, and a raw
    // `INSERT … RETURNING` consumed with `fetch_one` is invisible to the
    // read that follows it — which is how this test found the pool
    // visibility bug in the first place.
    let fresh = db
        .create_chain(&SignalChain {
            id: None,
            user_id: uid,
            name: "New".into(),
            description: None,
            components: vec![],
            listening_context: ListeningContext::Unknown,
            icon: None,
            is_active: true,
            shared: false,
            total_hours: 0.0,
            created_at: 0,
        })
        .await
        .unwrap();
    let chains = db.get_chains(uid).await.unwrap();
    assert!(!chains.iter().find(|c| c.id == Some(fresh)).unwrap().shared);
}

/// **A row written with `RETURNING` must be visible to the next read.**
///
/// sqlx steps a statement only as far as the rows the caller asks for, so
/// `fetch_one` / `fetch_optional` on an `INSERT … RETURNING` stops at the
/// first row and never reaches `SQLITE_DONE`. The implicit write
/// transaction stays open until that pooled connection is next used, and
/// until then the row is invisible to every *other* connection in the pool.
///
/// Measured before the fix: **226 of 300** inserts were missing from a read
/// issued immediately afterwards. Nothing is lost — it commits when that
/// connection is reused — but "create it, then read it back" is exactly the
/// shape that misses, and for a freshly minted credential that is a
/// client's first request getting a spurious 401.
///
/// This is phrased about *visibility* rather than about which method is
/// called, so a future `RETURNING` written the tempting way fails here
/// rather than becoming an intermittent bug somebody chases on the Pi.
#[tokio::test]
async fn a_row_written_with_returning_is_visible_immediately() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    // Interleaved on purpose: the read has to land on a *different* pool
    // connection than the write for this to bite, and alternating is what
    // makes that happen.
    let mut missing = Vec::new();
    for i in 0..40 {
        let name = format!("con{i}");
        let cred =
            db.create_mcp_connection(uid, &name, "listening:read", None, None).await.unwrap();
        if db.validate_mcp_token(&cred.token).await.unwrap().is_none() {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "{} of 40 credentials could not be used immediately after being minted: {:?}",
        missing.len(),
        missing
    );
}

// ── Patch: Step 3 ────────────────────────────────────────

/// Two decks, patched, with one shared chain each.
async fn patched_pair(db: &Database) -> (i64, i64, i64) {
    let me = db.create_user("ash", None, "pw", "user").await.unwrap();
    let them = db.create_user("kim", None, "pw", "user").await.unwrap();
    let chain = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Study', ?) RETURNING id",
    )
    .bind(them)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];
    db.set_chain_shared(them, chain, true).await.unwrap();
    // **Closed, stated rather than inherited.** These fixtures are about
    // the gate, and `create_user` now opens a new account to its patched
    // decks — so a fixture that took the default would quietly change
    // meaning with it, and every "nothing unshared reaches this screen"
    // test would start passing for the wrong reason. A test that wants the
    // switch on turns it on.
    db.set_share_all_with_patched(them, false).await.unwrap();
    let uuid = db.request_patch(me, them).await.unwrap();
    db.settle_patch(&uuid, them, true).await.unwrap();
    (me, them, chain)
}

async fn listen(db: &Database, uid: i64, chain: Option<i64>, title: &str, artist: &str, ts: u64) {
    let p = Play::new(title.into(), artist.into(), ts, format!("s-{uid}-{ts}"), "test".into());
    db.save_scrobble(uid, &p, None, None, chain, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap();
}

/// The Artists tab, and the thing that makes it worth opening: the overlap
/// is reported as *your own plays of that artist*, not as a score.
#[tokio::test]
async fn the_artists_tab_shows_overlap_rather_than_a_score() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_100).await;
    listen(&db, them, Some(chain), "Angel", "Massive Attack", 1_760_000_200).await;
    listen(&db, them, Some(chain), "Hallogallo", "Neu!", 1_760_000_300).await;
    // My own history — never published, only counted as overlap.
    listen(&db, me, None, "Risingson", "Massive Attack", 1_760_000_400).await;

    let artists = db.patch_artists(me, 20, None).await.unwrap();
    let ma = artists.iter().find(|a| a.name == "Massive Attack").expect("massive attack");
    assert_eq!(ma.plays, 2, "their plays through a shared chain");
    assert_eq!(ma.mine, 1, "and mine, which is the overlap");
    assert_eq!(ma.decks, 1);
    assert_eq!(ma.who.as_deref(), Some("kim"));

    let neu = artists.iter().find(|a| a.name == "Neu!").expect("neu");
    assert_eq!(neu.mine, 0, "an artist only they play still appears, with no overlap");
}

/// Every Step 3 reader narrows exactly like the Reel. A listen through an
/// unshared chain, one with no chain, and a skip are invisible to all of
/// them — and that has to be asserted on the *readouts*, because a filter
/// omitted from one query is the shape this codebase keeps finding.
#[tokio::test]
async fn nothing_unshared_reaches_any_of_the_step_three_screens() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    let private = sqlx::query_scalar::<_, i64>(
            "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Bedroom', ?) RETURNING id",
        )
        .bind(them)
        .bind(Database::now_secs())
        .fetch_all(&db.pool)
        .await
        .unwrap()[0];

    listen(&db, them, Some(chain), "Shared", "Public Artist", 1_760_000_100).await;
    listen(&db, them, Some(private), "Hidden", "Private Artist", 1_760_000_200).await;
    listen(&db, them, None, "Orphan", "Chainless Artist", 1_760_000_300).await;
    let skip = Play::new(
        "Bailed".into(),
        "Skipped Artist".into(),
        1_760_000_400,
        "sk-1".into(),
        "test".into(),
    );
    db.save_scrobble(
        them,
        &skip,
        None,
        None,
        Some(chain),
        None,
        None,
        None,
        None,
        ListenDisposition { imported: false, skipped: true },
    )
    .await
    .unwrap();

    let names: Vec<String> =
        db.patch_artists(me, 50, None).await.unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Public Artist"], "artists tab");

    let deck = db.deck_view(me, "kim").await.unwrap().expect("the deck");
    assert_eq!(deck.listens, 1, "deck page counts only shared listens");
    assert_eq!(deck.chains.len(), 1, "and lists only shared chains");
    assert_eq!(deck.chains[0].name, "Study");

    let pulse = db.instance_pulse(0).await.unwrap();
    let pulse_names: Vec<String> = pulse.artists.iter().map(|a| a.name.clone()).collect();
    assert_eq!(pulse_names, ["Public Artist"], "instance figures");
    assert_eq!(pulse.listens, 1);
}

/// The switch opens the deck up, and the things it must **not** open.
///
/// The sharing unit moved from the chain to the patch on 2026-08-26: a
/// signal chain is a gear configuration, and using it as an access-control
/// list was an odd proxy for consent, where a patch is a decision about a
/// person. So a deck that has opened up shows everything to those it has
/// let in — including listens with no chain at all, which the chain gate
/// could never have shown however many switches were on.
///
/// Three narrowings survive it, and each is here because it protects
/// somebody who did not agree to this:
///
/// * **The Instance tab stays chain-gated.** A patch is consent to one
///   person; there is no patch behind a figure the whole instance reads.
/// * **Skips still never appear**, anywhere, ever.
/// * **The switch is per deck.** Opening yours says nothing about mine.
/// A new account starts open to the decks it lets in; one that predates the
/// switch does not.
///
/// The asymmetry is the whole point and it is easy to "tidy" away in either
/// direction. Flipping the column default would open every existing
/// account, whose patches were accepted on the understanding that an
/// unshared chain published nothing. Leaving the insert alone would keep
/// every *new* account closed for an obligation it never incurred, which is
/// what made the switch look like a feature nobody uses.
#[tokio::test]
async fn a_new_account_starts_open_to_the_decks_it_lets_in() {
    let db = testdb().await;
    let fresh = db.create_user("newcomer", None, "pw", "user").await.unwrap();
    assert!(
        db.share_all_with_patched(fresh).await.unwrap(),
        "an account created under this rule starts open"
    );

    // An account that predates the column: the migration gives it 0, which
    // is what `create_user` is deliberately not relying on.
    sqlx::query(
        "INSERT INTO users (username, password_hash, role, created_at) VALUES \
             ('oldtimer', 'pw', 'user', 0)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let old: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'oldtimer'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        !db.share_all_with_patched(old).await.unwrap(),
        "an account that predates the switch is left exactly as it was"
    );
}

#[tokio::test]
async fn opening_a_deck_up_shows_every_listen_and_still_spares_the_instance() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let private = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Bedroom', ?) \
             RETURNING id",
    )
    .bind(them)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];

    listen(&db, them, Some(chain), "Shared", "Public Artist", 1_760_000_100).await;
    listen(&db, them, Some(private), "Hidden", "Private Artist", 1_760_000_200).await;
    listen(&db, them, None, "Orphan", "Chainless Artist", 1_760_000_300).await;
    let skip = Play::new(
        "Bailed".into(),
        "Skipped Artist".into(),
        1_760_000_400,
        "sk-1".into(),
        "test".into(),
    );
    db.save_scrobble(
        them,
        &skip,
        None,
        None,
        Some(private),
        None,
        None,
        None,
        None,
        ListenDisposition { imported: false, skipped: true },
    )
    .await
    .unwrap();

    db.set_share_all_with_patched(them, true).await.unwrap();

    let reel = db.reel(me, 50, None, None, None).await.unwrap();
    let titles: Vec<&str> = reel.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Orphan", "Hidden", "Shared"],
        "everything they played, newest first — and never the skip"
    );

    // The chain is named only where they share it, and "private" is told
    // apart from "there was none".
    let by_title = |t: &str| reel.iter().find(|r| r.title == t).expect("row");
    assert_eq!(by_title("Shared").chain_name.as_deref(), Some("Study"));
    assert!(by_title("Shared").has_chain);
    assert_eq!(by_title("Hidden").chain_name, None, "a private chain is not named");
    assert!(by_title("Hidden").has_chain, "but the screen must know there *was* one");
    assert_eq!(by_title("Orphan").chain_name, None);
    assert!(!by_title("Orphan").has_chain, "and that this one ran through nothing");

    // The rest of the patched screens follow.
    let names: Vec<String> =
        db.patch_artists(me, 50, None).await.unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names.len(), 3, "artists tab, got {names:?}");
    let deck = db.deck_view(me, "kim").await.unwrap().expect("the deck");
    assert_eq!(deck.listens, 3, "deck page");
    assert_eq!(deck.chains.len(), 1, "but it still lists only the chains they share");

    // And the instance does not. This is the assertion that matters most:
    // it is the one place a person who never agreed to any of this is read.
    let pulse = db.instance_pulse(0).await.unwrap();
    assert_eq!(pulse.listens, 1, "instance figures stay chain-gated");
    let pulse_names: Vec<String> = pulse.artists.iter().map(|a| a.name.clone()).collect();
    assert_eq!(pulse_names, ["Public Artist"]);
}

/// The switch is one deck's own, and closing it closes everything again.
#[tokio::test]
async fn the_switch_is_per_deck_and_reverses() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let mine = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Mine', ?) \
             RETURNING id",
    )
    .bind(me)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];

    listen(&db, them, None, "Theirs", "Their Artist", 1_760_000_100).await;
    listen(&db, me, Some(mine), "Mine", "My Artist", 1_760_000_200).await;

    // I open *mine*. Theirs is untouched, so their chainless listen stays
    // hidden from me — the switch is not a setting on the relationship.
    db.set_share_all_with_patched(me, true).await.unwrap();
    assert!(db.reel(me, 50, None, None, None).await.unwrap().is_empty(), "mine is not theirs");

    db.set_share_all_with_patched(them, true).await.unwrap();
    assert_eq!(db.reel(me, 50, None, None, None).await.unwrap().len(), 1);

    // Nothing was ever handed over, so closing it takes it back completely.
    db.set_share_all_with_patched(them, false).await.unwrap();
    assert!(db.reel(me, 50, None, None, None).await.unwrap().is_empty(), "and it reverses");
    assert_eq!(chain, chain);
}

/// A deck page is readable only through an accepted patch, and the check
/// is in the query rather than the handler.
#[tokio::test]
async fn a_deck_page_needs_an_accepted_patch() {
    let db = testdb().await;
    let (me, them, _) = patched_pair(&db).await;
    let stranger = db.create_user("sam", None, "pw", "user").await.unwrap();

    assert!(db.deck_view(me, "kim").await.unwrap().is_some());
    assert!(db.deck_view(stranger, "kim").await.unwrap().is_none(), "no patch, no page");

    // Withdrawing closes it again.
    db.unpatch(me, them).await.unwrap();
    assert!(db.deck_view(me, "kim").await.unwrap().is_none());
}

/// A dub is a crate row that came from a person, and it carries the two
/// guards the crate already has.
#[tokio::test]
async fn a_dub_travels_along_a_patch_and_keeps_the_crate_s_guards() {
    let db = testdb().await;
    let (me, them, _) = patched_pair(&db).await;
    let stranger = db.create_user("sam", None, "pw", "user").await.unwrap();

    let rec = NewRecommendation {
        kind: "album".into(),
        artist: "Talk Talk".into(),
        name: Some("Laughing Stock".into()),
        mbid: None,
        reason: Some("the one to start with".into()),
        because: Some("you play a lot of Bark Psychosis".into()),
    };

    // Not patched with a stranger.
    assert!(db.dub(them, stranger, &rec, "kim").await.is_err());

    // Along the patch, either direction.
    let id = db.dub(them, me, &rec, "kim").await.unwrap();
    assert!(id > 0);

    let dubs = db.dubs_for(me).await.unwrap();
    assert_eq!(dubs.len(), 1);
    assert_eq!(dubs[0].from_username, "kim");
    assert_eq!(dubs[0].artist, "Talk Talk");
    assert_eq!(dubs[0].because.as_deref(), Some("you play a lot of Bark Psychosis"));

    // It is a crate row, on the page they already visit — not a second
    // table and not a second page.
    let crate_rows = db.list_crate(me).await.unwrap();
    assert_eq!(crate_rows.len(), 1);
    assert_eq!(crate_rows[0].rec.artist, "Talk Talk");

    // And a model suggestion is still distinguishable from a person's.
    assert!(db.dubs_for(them).await.unwrap().is_empty());
}

/// A sitting may only be opened onto decks you are patched with.
///
/// An invitation puts a banner on somebody's Patch page, so being able to
/// aim one at a stranger is a way to reach them without their consent —
/// the rule `dub` already carries. Phrased about the *refusal and its
/// consequence*: the sitting must not half-start with the stranger
/// quietly dropped, which is the version somebody would reach for when
/// "fixing" this by filtering the list instead of refusing it.
#[tokio::test]
async fn a_sitting_cannot_be_opened_onto_a_deck_you_are_not_patched_with() {
    let db = testdb().await;
    let (me, them, _) = patched_pair(&db).await;
    let stranger = db.create_user("sam", None, "pw", "user").await.unwrap();

    // Along the patch, either direction.
    assert!(db.start_spool(me, None, &[them], &[]).await.is_ok());
    db.end_spool(&db.active_spool(me).await.unwrap().unwrap().uuid, me).await.unwrap();

    assert!(db.start_spool(me, None, &[stranger], &[]).await.is_err());
    // And nothing was written on the way to refusing.
    assert!(db.active_spool(me).await.unwrap().is_none());
    assert!(db.active_spool(stranger).await.unwrap().is_none());

    // One stranger in the guest list refuses the whole sitting rather than
    // opening it without them.
    assert!(db.start_spool(me, None, &[them, stranger], &[]).await.is_err());
    assert!(db.active_spool(me).await.unwrap().is_none());
}

/// "Who else here plays them" is instance-wide, shared-chain only, and
/// says whether you can open their deck.
///
/// The instance-wide part is the deliberate reversal the Instance tab
/// makes, so the thing worth pinning is the *limiter*: an unshared chain
/// contributes nothing, however much of the artist it holds.
#[tokio::test]
async fn who_else_plays_them_counts_shared_chains_only() {
    let db = testdb().await;
    let (me, them, shared) = patched_pair(&db).await;
    let stranger = db.create_user("sam", None, "pw", "user").await.unwrap();
    async fn chain(db: &Database, uid: i64, name: &str, shared: bool) -> i64 {
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, ?, ?) \
                 RETURNING id",
        )
        .bind(uid)
        .bind(name)
        .bind(Database::now_secs())
        .fetch_all(&db.pool)
        .await
        .unwrap()[0];
        if shared {
            db.set_chain_shared(uid, id, true).await.unwrap();
        }
        id
    }
    let sam_open = chain(&db, stranger, "Sam open", true).await;
    let sam_shut = chain(&db, stranger, "Sam shut", false).await;
    let mine = chain(&db, me, "Mine", false).await;

    listen(&db, them, Some(shared), "Angel", "Massive Attack", 1_760_000_100).await;
    listen(&db, stranger, Some(sam_open), "Angel", "Massive Attack", 1_760_000_200).await;
    // Three more from a chain Sam does not share — none of them may count.
    for t in 0..3 {
        listen(&db, stranger, Some(sam_shut), "Teardrop", "Massive Attack", 1_760_000_300 + t)
            .await;
    }
    listen(&db, me, Some(mine), "Angel", "Massive Attack", 1_760_000_400).await;

    let (decks, chains) = db.patch_artist_context(me, "Massive Attack").await.unwrap();

    // Instance-wide: Sam appears without a patch. But only their shared
    // chain's single play counts, not the three behind the private one.
    let sam = decks.iter().find(|d| d.username == "sam").expect("instance-wide");
    assert_eq!(sam.plays, 1, "an unshared chain contributes nothing");
    assert!(!sam.patched, "and the screen must know it cannot open that deck");

    let kim = decks.iter().find(|d| d.username == "kim").expect("the deck I am patched with");
    assert_eq!(kim.plays, 1);
    assert!(kim.patched);

    // Never yourself — the panel is *who else*.
    assert!(!decks.iter().any(|d| d.user_id == me));

    // And your own chains are your own listening, so a private one still
    // counts: what you heard them on is a fact about your setup rather than
    // a publication.
    assert_eq!(chains.len(), 1);
    assert_eq!(chains[0].name, "Mine");
    assert_eq!(chains[0].plays, 1);
}

/// Searching the Reel narrows it and **never widens it**.
///
/// The filters are extra `AND`s on a query whose four narrowing clauses are
/// the whole of what makes the Reel safe, so the thing worth asserting is
/// that a search cannot reach past them — a listen through an unshared
/// chain must stay invisible however precisely it is asked for by name.
#[tokio::test]
async fn searching_the_reel_narrows_it_and_never_widens_it() {
    let db = testdb().await;
    let (me, them, shared) = patched_pair(&db).await;
    let private = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Private', ?) \
             RETURNING id",
    )
    .bind(them)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];

    listen(&db, them, Some(shared), "Ascension Day", "Talk Talk", 1_760_000_100).await;
    listen(&db, them, Some(shared), "Spoon", "Can", 1_760_000_200).await;
    listen(&db, them, Some(private), "Hidden", "Talk Talk", 1_760_000_300).await;

    let titles = |v: Vec<ReelListen>| -> Vec<String> { v.into_iter().map(|r| r.title).collect() };

    assert_eq!(
        titles(db.reel(me, 40, None, None, None).await.unwrap()).len(),
        2,
        "the unshared listen is not on the Reel to begin with"
    );

    // By track, by artist, by chain, by deck — all narrow.
    assert_eq!(titles(db.reel(me, 40, None, Some("spoon"), None).await.unwrap()), ["Spoon"]);
    assert_eq!(
        titles(db.reel(me, 40, None, Some("talk talk"), None).await.unwrap()),
        ["Ascension Day"]
    );
    assert_eq!(titles(db.reel(me, 40, None, Some("study"), None).await.unwrap()).len(), 2);
    assert_eq!(titles(db.reel(me, 40, None, None, Some("kim")).await.unwrap()).len(), 2);
    assert!(titles(db.reel(me, 40, None, None, Some("nobody")).await.unwrap()).is_empty());

    // **The one that matters.** Naming the unshared listen exactly still
    // finds nothing: a search is an extra narrowing, never a way past the
    // four clauses that decide what the Reel may publish.
    assert!(
        titles(db.reel(me, 40, None, Some("Hidden"), None).await.unwrap()).is_empty(),
        "a search must not reach a chain its owner does not share"
    );

    // And a wildcard typed into the box is a literal, not a match-all.
    assert!(titles(db.reel(me, 40, None, Some("%"), None).await.unwrap()).is_empty());
}

/// A live deck is gated exactly as a banked listen is.
///
/// With the switch **off** — the default, and every account that predates
/// it — that is the shared chain, unchanged. `live_visibility` returns the
/// chain's *name* rather than a boolean so one round trip answers both
/// questions, which is also what stops a caller showing the name of a
/// chain it never checked.
#[tokio::test]
async fn a_live_deck_is_gated_on_the_same_shared_chain_a_listen_is() {
    let db = testdb().await;
    let (me, them, shared) = patched_pair(&db).await;
    let private = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Private', ?) \
             RETURNING id",
    )
    .bind(them)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];

    assert_eq!(
        db.live_visibility(them, Some(shared)).await.unwrap(),
        Visible::Yes(Some("Study".into()))
    );
    assert_eq!(
        db.live_visibility(them, Some(private)).await.unwrap(),
        Visible::No,
        "with the switch off an unshared chain shows nothing at all"
    );
    assert_eq!(
        db.live_visibility(them, None).await.unwrap(),
        Visible::No,
        "and neither does a source that reports no chain"
    );
    // Somebody else's chain is not yours to publish, whatever its switch
    // says. `me` is open to their patched decks, so the *listen* is visible
    // — what must not happen is their chain's name coming back on it.
    assert_eq!(
        db.live_visibility(me, Some(shared)).await.unwrap(),
        Visible::Yes(None),
        "a chain that is not theirs is never named on their deck"
    );

    // With the switch on, the track shows and the private chain is simply
    // not named — which is the whole of what changed.
    db.set_share_all_with_patched(them, true).await.unwrap();
    assert_eq!(
        db.live_visibility(them, Some(shared)).await.unwrap(),
        Visible::Yes(Some("Study".into())),
        "a shared chain is still named"
    );
    assert_eq!(
        db.live_visibility(them, Some(private)).await.unwrap(),
        Visible::Yes(None),
        "a private chain shows the track and withholds the name"
    );
    assert_eq!(
        db.live_visibility(them, None).await.unwrap(),
        Visible::Yes(None),
        "and a chainless listen shows too — 'all the music they scrobble' means all of it"
    );

    // And the strip only ever looks at decks you actually patched into.
    let peers = db.accepted_patch_targets(me).await.unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].username, "kim");
    assert!(db.accepted_patch_targets(them).await.unwrap().is_empty(), "a patch has a direction");
}

/// A regroup keeps what a sitting was called, what was written about it,
/// and **the id other people hold**.
///
/// The rebuild used to mint a fresh uuid per group and carry neither title
/// nor notes, so it silently erased every name anybody had written — and
/// since hand-overs point at `sessions.uuid`, it broke those too. Asserted
/// through a hand-over rather than only on the column, because that is the
/// consequence somebody would actually notice.
#[tokio::test]
async fn a_regroup_keeps_what_a_sitting_was_called_and_the_id_others_hold() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    listen(&db, them, Some(chain), "One", "Talk Talk", 1_760_000_100).await;
    listen(&db, them, Some(chain), "Two", "Talk Talk", 1_760_000_200).await;
    // A second sitting, hours later, so the gap really separates them.
    listen(&db, them, Some(chain), "Later", "Bark Psychosis", 1_760_090_000).await;
    db.rebuild_sessions(them, 1800).await.unwrap();

    let before: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, uuid FROM sessions WHERE user_id = ? ORDER BY started_at")
            .bind(them)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(before.len(), 2);
    let (first_id, first_uuid) = before[0].clone();

    db.set_session_title(them, first_id, Some("Sunday morning")).await.unwrap();
    db.set_session_notes(them, first_id, Some("new cartridge")).await.unwrap();
    let handed = db.hand_over_session(them, &first_uuid, me, Some("have a look")).await.unwrap();
    assert_eq!(db.handed_sitting(&handed, me).await.unwrap().unwrap().listens.len(), 2);

    // The thing that used to wipe all of it.
    db.rebuild_sessions(them, 1800).await.unwrap();

    let after: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT uuid, title, notes FROM sessions WHERE user_id = ? ORDER BY started_at",
    )
    .bind(them)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].0, first_uuid, "the id other people hold survives a regroup");
    assert_eq!(after[0].1.as_deref(), Some("Sunday morning"));
    assert_eq!(after[0].2.as_deref(), Some("new cartridge"));

    // And the hand-over still resolves, rather than reading as a sitting
    // that has been regrouped out of existence.
    let doc = db.handed_sitting(&handed, me).await.unwrap().unwrap();
    assert!(!doc.gone, "a regroup must not break a sitting somebody was handed");
    assert_eq!(doc.listens.len(), 2);

    // A widened gap folds the two together: the earliest one's identity is
    // what survives, the same rule `merge_sessions` follows.
    db.rebuild_sessions(them, 200_000).await.unwrap();
    let folded: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT uuid, title FROM sessions WHERE user_id = ?")
            .bind(them)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(folded.len(), 1);
    assert_eq!(folded[0].0, first_uuid);
    assert_eq!(folded[0].1.as_deref(), Some("Sunday morning"));
}

/// **The spool you forgot to start.** The other gesture entirely.
///
/// They were in the room, so the plays are offered as ordinary captures and
/// reach their history on the same accept a live sitting uses. Asserted
/// against the *recipient's history*, because that is the whole difference
/// from a recommendation and the thing the original design got backwards.
#[tokio::test]
async fn a_sitting_reconstructed_late_reaches_the_history_a_live_one_would_have() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let stranger = db.create_user("sam", None, "pw", "user").await.unwrap();
    let private = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Private', ?) \
             RETURNING id",
    )
    .bind(them)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];

    listen(&db, them, Some(chain), "Together", "Talk Talk", 1_760_000_100).await;
    listen(&db, them, Some(private), "Withheld", "Talk Talk", 1_760_000_200).await;
    db.rebuild_sessions(them, 1800).await.unwrap();
    let sitting: String = sqlx::query_scalar("SELECT uuid FROM sessions WHERE user_id = ? LIMIT 1")
        .bind(them)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    assert!(db.retro_spool_from_session(them, &sitting, stranger).await.is_err());

    let (spool, offered, withheld) = db.retro_spool_from_session(them, &sitting, me).await.unwrap();
    assert_eq!(offered, 1);
    assert_eq!(withheld, 1, "a sitting made late captures what one made on time would have");

    // It arrives in the ordinary capture inbox — indistinguishable, from
    // the recipient's side, from a live sitting.
    let caps = db.spool_captures(me).await.unwrap();
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0].title, "Together");
    assert_eq!(caps[0].state, "pending");

    // And still nothing until they say yes.
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scrobbles WHERE user_id = ?")
        .bind(me)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(before, 0);

    assert_eq!(db.settle_captures(me, &[caps[0].id], true).await.unwrap(), (1, 0));
    let landed: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT title, status, joint_session_id, company FROM scrobbles WHERE user_id = ?",
    )
    .bind(me)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(landed.len(), 1, "this one really does enter their history");
    assert_eq!(landed[0].0, "Together");
    assert_eq!(landed[0].1, "imported", "and is never forwarded onward");
    assert_eq!(landed[0].2.as_deref(), Some(spool.as_str()));
    assert_eq!(landed[0].3.as_deref(), Some("kim"), "heard with the person who played it");

    // The host's own copy learns it too.
    let hosts: Option<String> = sqlx::query_scalar(
        "SELECT company FROM scrobbles WHERE user_id = ? AND title = 'Together'",
    )
    .bind(them)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(hosts.as_deref(), Some("ash"));

    // Doing it again is idempotent, and adding a second person joins the
    // sitting that exists rather than minting a parallel one.
    let (again, offered2, _) = db.retro_spool_from_session(them, &sitting, me).await.unwrap();
    assert_eq!(again, spool, "one sitting, however many people were in it");
    assert_eq!(offered2, 0, "and no duplicate offers");
}

/// A play already offered by a live sitting is not offered a second time by
/// a reconstruction of the same evening.
///
/// Found on a running instance rather than here: `spool_captures`' unique
/// constraint is per spool, so two sittings covering the same plays each
/// insert happily and the track appears twice in one inbox.
#[tokio::test]
async fn reconstructing_an_evening_does_not_re_offer_what_a_live_sitting_already_did() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    let live = db.start_spool(them, None, &[me], &[]).await.unwrap();
    assert!(db.set_spool_membership(&live, me, true).await.unwrap().is_some());
    listen(&db, them, Some(chain), "Both", "Talk Talk", 1_760_000_100).await;
    assert_eq!(db.spool_captures(me).await.unwrap().len(), 1, "the live sitting offered it");

    db.rebuild_sessions(them, 1800).await.unwrap();
    let sitting: String = sqlx::query_scalar("SELECT uuid FROM sessions WHERE user_id = ? LIMIT 1")
        .bind(them)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    let (_, offered, _) = db.retro_spool_from_session(them, &sitting, me).await.unwrap();
    assert_eq!(offered, 0, "already offered by the live sitting");
    assert_eq!(
        db.spool_captures(me).await.unwrap().len(),
        1,
        "one play, one offer, whatever route it arrived by"
    );
}

/// Handing over a past sitting: a document, never captures.
///
/// Asserts the three things that separate it from a Shared Spool — nothing
/// reaches the recipient's history, only shared-chain listens are shown,
/// and what is withheld is **counted and reported** rather than silently
/// dropped. That last one is the version somebody would "tidy up" by just
/// filtering, which would leave a document misrepresenting the evening
/// with no way for the reader to tell.
#[tokio::test]
async fn a_handed_over_sitting_is_a_document_and_says_what_it_is_not_showing() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let stranger = db.create_user("sam", None, "pw", "user").await.unwrap();
    let private = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Private', ?) \
             RETURNING id",
    )
    .bind(them)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];

    listen(&db, them, Some(chain), "Shown", "Talk Talk", 1_760_000_100).await;
    listen(&db, them, Some(private), "Withheld", "Talk Talk", 1_760_000_200).await;
    db.rebuild_sessions(them, 1800).await.unwrap();
    let sitting: String = sqlx::query_scalar("SELECT uuid FROM sessions WHERE user_id = ? LIMIT 1")
        .bind(them)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    // A stranger cannot be handed one — the guard `dub` and `start_spool`
    // both carry, for the same reason: it puts a document on their screen.
    assert!(db.hand_over_session(them, &sitting, stranger, None).await.is_err());
    // Nor can somebody hand over a sitting that is not theirs.
    assert!(db.hand_over_session(me, &sitting, them, None).await.is_err());

    let handed =
        db.hand_over_session(them, &sitting, me, Some("the good half of Sunday")).await.unwrap();

    // Nothing entered my history.
    let mine: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scrobbles WHERE user_id = ?")
        .bind(me)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(mine, 0, "a handed-over sitting is a document, not captures");

    let doc = db.handed_sitting(&handed, me).await.unwrap().expect("readable by the recipient");
    let titles: Vec<&str> = doc.listens.iter().map(|l| l.title.as_str()).collect();
    assert_eq!(titles, ["Shown"], "only shared chains are published, hand-over or not");
    assert_eq!(doc.omitted, 1, "and what is not shown is said, never silently dropped");
    assert_eq!(doc.note.as_deref(), Some("the good half of Sunday"));

    // A stranger cannot read it even holding the uuid.
    assert!(db.handed_sitting(&handed, stranger).await.unwrap().is_none());

    // It is a reference, not a snapshot: un-sharing takes the listen back
    // out on the next read. That is right locally and must become a
    // snapshot once this federates.
    db.set_chain_shared(them, chain, false).await.unwrap();
    let after = db.handed_sitting(&handed, me).await.unwrap().unwrap();
    assert!(after.listens.is_empty(), "un-sharing a chain really does withdraw it");
    assert_eq!(after.omitted, 2);

    // Withdrawing is real, because nothing was ever copied.
    assert!(db.withdraw_session(&handed, me).await.unwrap() == false, "not mine to withdraw");
    assert!(db.withdraw_session(&handed, them).await.unwrap());
    assert!(db.handed_sitting(&handed, me).await.unwrap().is_none());
}

/// **Being in company is a fact about your own listening, not a
/// publication.**
///
/// The stamp used to sit inside the gated block, so a host who shared no
/// chain had none of their own listens marked as social — though they were
/// demonstrably hosting a sitting. Asserted about both halves at once,
/// because the fix is only right if the *fan-out* stays gated: the tag
/// widens and the publishing does not.
#[tokio::test]
async fn an_unshared_chain_still_knows_you_were_not_alone() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    // `chain` belongs to `them` and is shared. Give them a private one too.
    let private = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Private', ?) \
             RETURNING id",
    )
    .bind(them)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];

    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    assert!(db.set_spool_membership(&spool, me, true).await.unwrap().is_some());

    listen(&db, them, Some(private), "Unshared", "Talk Talk", 1_760_000_100).await;
    listen(&db, them, Some(chain), "Shared", "Talk Talk", 1_760_000_200).await;

    let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT title, joint_session_id, company FROM scrobbles WHERE user_id = ? \
             ORDER BY timestamp",
    )
    .bind(them)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for (title, joint, company) in &rows {
        assert_eq!(
            joint.as_deref(),
            Some(spool.as_str()),
            "{title}: the sitting is a fact about this listening whatever it publishes"
        );
        assert_eq!(company.as_deref(), Some("ash"), "{title}: and it names who was there");
    }

    // The publishing half is untouched: only the shared chain was offered.
    let offered: Vec<String> =
        db.spool_captures(me).await.unwrap().into_iter().map(|c| c.title).collect();
    assert_eq!(offered, ["Shared"], "an unshared chain still publishes nothing");
}

/// Company is stored per listen, names who was actually in the room, and
/// survives the one operation that rewrites every session.
#[tokio::test]
async fn company_is_marked_on_the_listen_and_survives_a_session_rebuild() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    listen(&db, uid, None, "Alone", "Burial", 1_760_000_100).await;
    listen(&db, uid, None, "Together", "Burial", 1_760_000_200).await;

    let id: i64 = sqlx::query_scalar("SELECT id FROM scrobbles WHERE title = 'Together'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.bulk_set_scrobble_company(uid, &[id], Some("Kim")).await.unwrap(), 1);

    // The filter reads the mark, and unmarked listening is not claimed as
    // solitary anywhere — it is simply the other side of the filter.
    let social = db
        .get_recent_scrobbles(uid, 10, 0, None, None, None, None, None, Some(true))
        .await
        .unwrap();
    assert_eq!(social.len(), 1);
    assert_eq!(social[0].title, "Together");
    assert_eq!(social[0].company.as_deref(), Some("Kim"));
    let rest = db
        .get_recent_scrobbles(uid, 10, 0, None, None, None, None, None, Some(false))
        .await
        .unwrap();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].title, "Alone");
    let all =
        db.get_recent_scrobbles(uid, 10, 0, None, None, None, None, None, None).await.unwrap();
    assert_eq!(all.len(), 2, "no filter means no filter");

    // **The reason it is not on `sessions`.** A rebuild deletes every
    // session row and re-inserts it, so anything held there is erased by a
    // regroup with nothing to say so.
    db.rebuild_sessions(uid, 1800).await.unwrap();
    let after: Option<String> =
        sqlx::query_scalar("SELECT company FROM scrobbles WHERE title = 'Together'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(after.as_deref(), Some("Kim"), "a regroup must not erase who you were with");

    // Unmarking is explicit, and does not invent a sitting on the way.
    assert_eq!(db.bulk_set_scrobble_company(uid, &[id], None).await.unwrap(), 1);
    let (company, joint): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT company, joint_session_id FROM scrobbles WHERE id = ?")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(company.is_none());
    assert!(joint.is_none(), "a hand edit must never mint a joint listen");
}

/// The whole sitting: start, join, play, offer, accept.
///
/// **Nothing enters a history until it is accepted** — the rule that makes
/// captures a separate table rather than rows in `scrobbles` behind a flag.
#[tokio::test]
async fn a_spool_offers_plays_and_nothing_lands_until_it_is_accepted() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    let spool = db.start_spool(them, Some("Sunday listening"), &[me], &[]).await.unwrap();

    // Invited, not joined: a sitting must not write onto somebody's deck
    // before they have said they are in it.
    listen(&db, them, Some(chain), "Before", "Massive Attack", 1_760_000_100).await;
    assert!(db.spool_captures(me).await.unwrap().is_empty(), "an invitation is not attendance");

    // **Joining offers the sitting's backlog, not only what follows.**
    // This assertion used to be the other way round, and that was the bug
    // behind "I forgot to press Join and lost the evening": nothing on the
    // wire distinguishes somebody who was in the room all along and only
    // answered late — which is the common case, and the whole of it when
    // the sitting has already ended — from somebody who genuinely walked in
    // partway. Offering the earlier plays costs the second person a decline
    // and gives the first person their evening back; withholding them gave
    // the first person nothing and no way to ask again.
    //
    // The invariant is untouched: nothing was offered until the yes, and
    // nothing enters a history until each play is accepted on its own.
    assert_eq!(
        db.set_spool_membership(&spool, me, true).await.unwrap(),
        Some(("joined".into(), 1))
    );
    listen(&db, them, Some(chain), "During", "Massive Attack", 1_760_000_200).await;

    let caps = db.spool_captures(me).await.unwrap();
    let titles: Vec<&str> = caps.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["During", "Before"], "newest first, and the backlog is in it");
    assert_eq!(caps[0].from_username, "kim");
    assert_eq!(caps[0].state, "pending");

    // Still nothing in my history.
    let mine: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scrobbles WHERE user_id = ?")
        .bind(me)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(mine, 0, "a capture is an offer, not a listen");

    assert_eq!(db.settle_captures(me, &[caps[0].id], true).await.unwrap(), (1, 0));

    let landed: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT title, status, joint_session_id FROM scrobbles WHERE user_id = ?")
            .bind(me)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].0, "During");
    assert_eq!(
        landed[0].1, "imported",
        "somebody else's deck played it — it must never be forwarded to a public record"
    );
    assert_eq!(
        landed[0].2.as_deref(),
        Some(spool.as_str()),
        "N rows sharing one joint_session_id is what makes it a joint listen"
    );

    // The host's own copy carries the same sitting.
    let hosts: Option<String> = sqlx::query_scalar(
        "SELECT joint_session_id FROM scrobbles WHERE user_id = ? AND title = 'During'",
    )
    .bind(them)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(hosts.as_deref(), Some(spool.as_str()));

    // Settling twice does nothing, and declining leaves no listen.
    assert_eq!(db.settle_captures(me, &[caps[0].id], true).await.unwrap(), (0, 0));
}

/// An import must not fan out. A backfill is not a sitting, and twenty
/// thousand rows would become twenty thousand offers.
#[tokio::test]
async fn an_import_and_a_skip_are_never_captured() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();

    for (i, disp) in [
        ListenDisposition { imported: true, skipped: false },
        ListenDisposition { imported: false, skipped: true },
    ]
    .into_iter()
    .enumerate()
    {
        let p = Play::new(
            format!("T{i}"),
            "Massive Attack".into(),
            1_760_000_100 + i as u64,
            format!("x-{i}"),
            "test".into(),
        );
        db.save_scrobble(them, &p, None, None, Some(chain), None, None, None, None, disp)
            .await
            .unwrap();
    }

    assert!(
        db.spool_captures(me).await.unwrap().is_empty(),
        "neither an imported listen nor a skip is offered to the room"
    );
}

/// **Forgetting to press Join does not lose the evening.**
///
/// Reported by Ashwin: somebody was invited, never answered, and the host
/// ended the sitting — at which point the invitation vanished off their
/// screen and the plays were gone with it. Three separate refusals were
/// stacked on top of each other, so this asserts on the *outcome a person
/// gets* rather than on any one of them: the invitation is still there, it
/// can still be answered, and answering it offers what the sitting held.
///
/// The shared-chain limiter is asserted in the same test on purpose. A
/// backlog read out of `joint_session_id` sees the private-chain listens
/// too — that column is stamped before the gate — so the fix is only
/// correct if joining late publishes no more than joining on time would
/// have, and a version of this test that only counted rows would pass
/// against a backfill that leaked every one of them.
#[tokio::test]
async fn an_unanswered_invitation_outlives_the_sitting_and_still_carries_its_plays() {
    let db = testdb().await;
    let (me, them, shared) = patched_pair(&db).await;
    let private = sqlx::query_scalar::<_, i64>(
            "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Bedroom', ?) RETURNING id",
        )
        .bind(them)
        .bind(Database::now_secs())
        .fetch_all(&db.pool)
        .await
        .unwrap()[0];

    let spool = db.start_spool(them, Some("Sunday"), &[me], &[]).await.unwrap();

    // The whole evening plays out with the guest never having answered, so
    // not one capture is written for them as it happens.
    listen(&db, them, Some(shared), "Shared", "Talk Talk", 1_760_000_100).await;
    listen(&db, them, Some(private), "Private", "Talk Talk", 1_760_000_200).await;
    listen(&db, them, None, "Chainless", "Talk Talk", 1_760_000_300).await;
    assert!(db.spool_captures(me).await.unwrap().is_empty(), "nothing offered before joining");

    assert!(db.end_spool(&spool, them).await.unwrap());

    // The invitation survives the room closing, because closing it is not
    // an answer.
    let view = db.active_spool(me).await.unwrap().expect("the invitation is still there");
    assert_eq!(view.uuid, spool);
    assert_eq!(view.my_state, "invited");
    assert!(view.ended_at.is_some(), "and the screen can tell it is over");

    // And answering it now is what it always should have been.
    assert_eq!(
        db.set_spool_membership(&spool, me, true).await.unwrap(),
        Some(("joined".into(), 1)),
        "joining late offers the sitting's plays, and only the shareable ones"
    );
    let titles: Vec<String> =
        db.spool_captures(me).await.unwrap().into_iter().map(|c| c.title).collect();
    assert_eq!(titles, ["Shared"], "a private chain is no more published late than on time");

    // Answered, so it stops asking.
    assert!(db.active_spool(me).await.unwrap().is_none());

    // Joining twice does not offer the same play twice — the rule
    // `retro_spool_from_session` already carries, which this shares.
    assert_eq!(
        db.set_spool_membership(&spool, me, true).await.unwrap(),
        Some(("joined".into(), 0))
    );
    assert_eq!(db.spool_captures(me).await.unwrap().len(), 1);
}

/// **A play out of a sitting is forwarded only if its owner asked for it,
/// and only if it is recent.**
///
/// Both gates, in one test, because each covers the other's blind spot and
/// a version asserting only the switch would pass against a build that
/// relayed a sitting reconstructed out of 2019 — one Accept rewriting a
/// year of somebody's public Last.fm history, which is not recoverable.
///
/// Phrased about `status`, which is the thing that actually decides:
/// `flush_pending` reads `'pending'` and nothing else, so this asserts on
/// what the flush loop will pick up rather than on the flag that led there.
#[tokio::test]
async fn a_spool_play_is_forwarded_only_if_asked_for_and_only_if_recent() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();

    let status_of = |title: &'static str| {
        let db = &db;
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM scrobbles WHERE user_id = ? AND title = ?",
            )
            .bind(me)
            .bind(title)
            .fetch_one(&db.pool)
            .await
            .unwrap()
        }
    };
    // Each play gets its own second: `listen` derives its `source_id` from
    // (user, timestamp), so two at the same instant are one listen as far
    // as `exists` is concerned — which reads as the capture never having
    // been made.
    let offer = |title: &'static str, at: i64| {
        let db = &db;
        async move {
            listen(db, them, Some(chain), title, "Talk Talk", at as u64).await;
            let caps = db.spool_captures(me).await.unwrap();
            caps.iter().find(|c| c.title == title).expect("offered").id
        }
    };

    let now = Database::now_secs();

    // Off by default: it shipped never-forwarded, and an upgrade must not
    // start writing to somebody's public record on its own.
    assert!(!db.forward_spool_listens(me).await.unwrap());
    let a = offer("Default off", now - 60).await;
    assert_eq!(db.settle_captures(me, &[a], true).await.unwrap(), (1, 0));
    assert_eq!(status_of("Default off").await, "imported");

    db.set_forward_spool_listens(me, true).await.unwrap();

    // On, and tonight: relayed.
    let b = offer("Tonight", now - 120).await;
    assert_eq!(db.settle_captures(me, &[b], true).await.unwrap(), (1, 1));
    assert_eq!(status_of("Tonight").await, "pending");

    // On, but a sitting reconstructed out of years ago: stored, never
    // relayed. This is the gate that stops one click rewriting a history.
    let c = offer("Long ago", now - 400 * 86_400).await;
    assert_eq!(db.settle_captures(me, &[c], true).await.unwrap(), (1, 0));
    assert_eq!(status_of("Long ago").await, "imported");

    // And a clock running badly fast is not a way past it.
    let d = offer("Next year", now + 400 * 86_400).await;
    assert_eq!(db.settle_captures(me, &[d], true).await.unwrap(), (1, 0));
    assert_eq!(status_of("Next year").await, "imported");
}

/// The window is what separates a sitting from a reconstruction, so its
/// edges are pinned rather than left to the constant nobody re-reads.
#[test]
fn the_spool_forward_window_admits_a_late_accept_and_refuses_a_backfill() {
    let now = 1_760_000_000;
    assert!(spool_forward_is_live(now, now), "tonight");
    assert!(spool_forward_is_live(now - 13 * 86_400, now), "an inbox nobody opened for a week");
    assert!(!spool_forward_is_live(now - 15 * 86_400, now), "past what Last.fm would take");
    assert!(spool_forward_is_live(now + 600, now), "ten minutes of clock skew is not a backfill");
    assert!(!spool_forward_is_live(now + 7200, now), "two hours ahead is a broken clock");
}

/// Declining is what makes an invitation go away.
#[tokio::test]
async fn no_thanks_dismisses_an_invitation_for_good() {
    let db = testdb().await;
    let (me, them, _) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    assert!(db.end_spool(&spool, them).await.unwrap());

    assert!(db.active_spool(me).await.unwrap().is_some());
    // Saying no to an invitation is `declined`, not `left` — nobody who was
    // never in the room went home from it.
    assert_eq!(
        db.set_spool_membership(&spool, me, false).await.unwrap(),
        Some(("declined".into(), 0))
    );
    assert!(db.active_spool(me).await.unwrap().is_none());
}

/// Leaving a sitting that is still going takes it off the screen.
///
/// The reported bug, and it read as *"I cannot leave a Shared Spool"* because
/// the leaving worked perfectly: `set_spool_membership` wrote `left`, and the
/// banner — which narrowed on the sitting being open and never on the member's
/// own state — came straight back with its Leave button. Pressing it again
/// wrote `left` over `left`.
///
/// Note where its sibling above could not have caught this: that test declines
/// an invitation into a sitting the host has already **ended**, which is the
/// one case where `m.state = 'invited'` was the only reason the banner
/// appeared, so answering removed it whatever the rest of the clause said.
/// Both ways out are asserted here against a room that is still open.
#[tokio::test]
async fn answering_a_live_sitting_takes_its_banner_away() {
    let db = testdb().await;
    let (me, them, _) = patched_pair(&db).await;

    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();
    assert!(db.active_spool(me).await.unwrap().is_some(), "in the room, so the banner is right");

    assert_eq!(db.set_spool_membership(&spool, me, false).await.unwrap(), Some(("left".into(), 0)));
    assert!(db.active_spool(me).await.unwrap().is_none(), "went home, and the room is still open");

    // The host is still in it, and leaving is one person's decision.
    assert!(db.active_spool(them).await.unwrap().is_some());

    // And the same for somebody who never joined: declining an invitation into
    // a room that is still open is an answer, so it dismisses it too.
    let third = db.create_user("noor", None, "pw", "user").await.unwrap();
    let uuid = db.request_patch(third, them).await.unwrap();
    db.settle_patch(&uuid, them, true).await.unwrap();
    db.add_spool_members(&spool, them, &[third], None).await.unwrap();
    assert!(db.active_spool(third).await.unwrap().is_some(), "invited");
    assert_eq!(
        db.set_spool_membership(&spool, third, false).await.unwrap(),
        Some(("declined".into(), 0))
    );
    assert!(db.active_spool(third).await.unwrap().is_none());
}

/// **A sitting does not bypass the sharing switch.**
///
/// The other reading is defensible — a spool is a bounded event both people
/// opted into — and it was rejected: the rule this phase rests on is that a
/// listen through an unshared chain is not published, not counted and not
/// delivered, with no per-listen override. A capture is publishing, and
/// allowing it here would let somebody start a sitting and hand over a
/// chain they had deliberately kept private.
#[tokio::test]
async fn a_sitting_captures_only_from_shared_chains() {
    let db = testdb().await;
    let (me, them, shared) = patched_pair(&db).await;
    let private = sqlx::query_scalar::<_, i64>(
            "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Bedroom', ?) RETURNING id",
        )
        .bind(them)
        .bind(Database::now_secs())
        .fetch_all(&db.pool)
        .await
        .unwrap()[0];

    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();

    listen(&db, them, Some(private), "Private", "Talk Talk", 1_760_000_100).await;
    listen(&db, them, None, "Chainless", "Talk Talk", 1_760_000_200).await;
    listen(&db, them, Some(shared), "Shared", "Talk Talk", 1_760_000_300).await;

    let titles: Vec<String> =
        db.spool_captures(me).await.unwrap().into_iter().map(|c| c.title).collect();
    assert_eq!(
        titles,
        ["Shared"],
        "an unshared chain and a chainless listen are not offered to the room either"
    );
}

/// One running sitting per host, enforced by the index rather than by a
/// check somebody could race.
#[tokio::test]
async fn a_deck_hosts_one_sitting_at_a_time() {
    let db = testdb().await;
    let (me, them, _) = patched_pair(&db).await;

    let first = db.start_spool(them, Some("One"), &[me], &[]).await.unwrap();
    assert!(db.start_spool(them, Some("Two"), &[me], &[]).await.is_err(), "two rooms at once");

    assert!(db.end_spool(&first, me).await.unwrap() == false, "only the host may end it");
    assert!(db.end_spool(&first, them).await.unwrap());
    assert!(db.active_spool(them).await.unwrap().is_none());

    // And now a second one is fine.
    assert!(db.start_spool(them, Some("Two"), &[me], &[]).await.is_ok());
}

/// **Promotion is a relabelling, and this is how you know.**
///
/// Every piece of gear's hours are compared either side. If the other
/// variants were not re-expressed against the new base, the 2M Red's hours
/// would move to the Microlinear — a stylus that was never in the headshell
/// for those listens — and nothing would error.
#[tokio::test]
async fn promoting_a_variant_moves_no_hours() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    // Four boxes: a source, two cartridges, two sets of speakers.
    let mut gear = std::collections::HashMap::new();
    for (name, kind) in [
        ("Node", "streamer"),
        ("2M Red", "cartridge"),
        ("Microlinear", "cartridge"),
        ("LS50", "speakers"),
        ("Harbeth", "speakers"),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO equipment (user_id, name, type) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(uid)
        .bind(name)
        .bind(kind)
        .fetch_all(&db.pool)
        .await
        .unwrap()[0];
        gear.insert(name, id);
    }
    let g = |n: &str| *gear.get(n).unwrap();

    let comp = |name: &str, id: i64| ChainComponent {
        component_type: ComponentType::Transducer,
        name: name.into(),
        detail: None,
        equipment_id: Some(id),
    };

    let chain_id = db
        .create_chain(&SignalChain {
            id: None,
            user_id: uid,
            name: "Vinyl".into(),
            description: None,
            components: vec![
                comp("Node", g("Node")),
                comp("2M Red", g("2M Red")),
                comp("LS50", g("LS50")),
            ],
            listening_context: ListeningContext::Unknown,
            icon: None,
            is_active: true,
            shared: false,
            total_hours: 0.0,
            created_at: 0,
        })
        .await
        .unwrap();

    let micro = db
        .create_variant(
            uid,
            chain_id,
            "Microlinear",
            &[VariantOverride {
                position: 1,
                equipment_id: Some(g("Microlinear")),
                name: Some("Microlinear".into()),
            }],
        )
        .await
        .unwrap();
    let harbeth = db
        .create_variant(
            uid,
            chain_id,
            "Harbeths",
            &[VariantOverride {
                position: 2,
                equipment_id: Some(g("Harbeth")),
                name: Some("Harbeth".into()),
            }],
        )
        .await
        .unwrap();

    // An hour on each of the three setups.
    for (i, variant) in [None, Some(micro), Some(harbeth)].into_iter().enumerate() {
        let mut p = play(&format!("t{i}"), "Burial", 1_760_000_000 + i as u64 * 10_000);
        p.duration = Some(3600);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            Some(chain_id),
            variant,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    let before = db.equipment_seconds(uid).await.unwrap().0;
    assert_eq!(before.get(&g("2M Red")), Some(&7200), "base + Harbeths ran the 2M Red");
    assert_eq!(before.get(&g("Microlinear")), Some(&3600));
    assert_eq!(before.get(&g("LS50")), Some(&7200));
    assert_eq!(before.get(&g("Harbeth")), Some(&3600));
    assert_eq!(before.get(&g("Node")), Some(&10800), "every setup ran the Node");

    db.promote_variant(uid, micro, "2M Red days").await.unwrap();

    let after = db.equipment_seconds(uid).await.unwrap().0;
    assert_eq!(after, before, "promotion must not move a single second");

    // And the chain really did change.
    let chain =
        db.get_chains(uid).await.unwrap().into_iter().find(|c| c.id == Some(chain_id)).unwrap();
    assert_eq!(
        chain.components.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Node", "Microlinear", "LS50"]
    );

    // The promoted variant is gone — it is the chain now — and the old base
    // is a variant that restores it.
    let variants = db.list_variants(uid).await.unwrap();
    assert!(!variants.iter().any(|v| v.id == micro), "the promoted setup is the chain");
    let old = variants.iter().find(|v| v.name == "2M Red days").expect("the old base");
    assert_eq!(old.overrides.len(), 1);
    assert_eq!(old.overrides[0].position, 1);
    assert_eq!(old.overrides[0].equipment_id, Some(g("2M Red")));

    // **The other variant had to grow an override**, or it would have
    // silently inherited the Microlinear.
    let h = variants.iter().find(|v| v.id == harbeth).expect("the Harbeths");
    assert!(
        h.overrides.iter().any(|o| o.position == 1 && o.equipment_id == Some(g("2M Red"))),
        "the Harbeths setup must now pin the cartridge it was relying on the base for"
    );
}

/// The listens move with it: what was on the chain is now on the old-base
/// variant, and what was on the promoted variant is now on the chain.
#[tokio::test]
async fn promoting_repoints_the_listens_both_ways() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let chain_id = db
        .create_chain(&SignalChain {
            id: None,
            user_id: uid,
            name: "Desk".into(),
            description: None,
            components: vec![ChainComponent {
                component_type: ComponentType::Transducer,
                name: "HD 650".into(),
                detail: None,
                equipment_id: None,
            }],
            listening_context: ListeningContext::Unknown,
            icon: None,
            is_active: true,
            shared: false,
            total_hours: 0.0,
            created_at: 0,
        })
        .await
        .unwrap();
    let v = db
        .create_variant(
            uid,
            chain_id,
            "Hexa",
            &[VariantOverride { position: 0, equipment_id: None, name: Some("Hexa".into()) }],
        )
        .await
        .unwrap();

    for (i, variant) in [None, Some(v)].into_iter().enumerate() {
        let p = play(&format!("t{i}"), "Burial", 1_760_000_000 + i as u64 * 10_000);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            Some(chain_id),
            variant,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    let old_id = db.promote_variant(uid, v, "HD 650").await.unwrap();

    let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT title, chain_variant_id FROM scrobbles WHERE user_id = ? ORDER BY title",
    )
    .bind(uid)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(rows[0].1, Some(old_id), "the listen that was on the chain is on the old base");
    assert_eq!(rows[1].1, None, "and the one on the promoted setup is now on the chain");
}

/// Retagging a listen's chain must rewrite its variant, or the listen keeps
/// a setup belonging to a chain it is no longer on — components that were
/// never fitted together. `equipment_seconds` ignores such a pairing, so
/// the hours stay right and only the displayed setup is wrong, which is the
/// kind of wrongness nobody notices.
#[tokio::test]
async fn retagging_a_listen_s_chain_does_not_leave_a_stale_setup() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let mk = |name: &str| SignalChain {
        id: None,
        user_id: uid,
        name: name.into(),
        description: None,
        components: vec![ChainComponent {
            component_type: ComponentType::Transducer,
            name: "Cans".into(),
            detail: None,
            equipment_id: None,
        }],
        listening_context: ListeningContext::Unknown,
        icon: None,
        is_active: true,
        shared: false,
        total_hours: 0.0,
        created_at: 0,
    };
    let a = db.create_chain(&mk("A")).await.unwrap();
    let b = db.create_chain(&mk("B")).await.unwrap();
    let va = db
        .create_variant(
            uid,
            a,
            "A's setup",
            &[VariantOverride { position: 0, equipment_id: None, name: Some("Other".into()) }],
        )
        .await
        .unwrap();

    let p = play("t", "Burial", 1_760_000_000);
    db.save_scrobble(
        uid,
        &p,
        None,
        None,
        Some(a),
        Some(va),
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM scrobbles WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    // Move it to chain B, naming no setup.
    db.update_scrobble(uid, id, None, None, None, None, None, Some(b), None, None, None, None)
        .await
        .unwrap();
    let variant: Option<i64> =
        sqlx::query_scalar("SELECT chain_variant_id FROM scrobbles WHERE id = ?")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(variant, None, "A's setup must not survive a move to chain B");

    // The bulk path has to do the same.
    db.bulk_set_scrobble_chain(uid, &[id], Some(a), Some(va)).await.unwrap();
    db.bulk_set_scrobble_chain(uid, &[id], Some(b), None).await.unwrap();
    let variant: Option<i64> =
        sqlx::query_scalar("SELECT chain_variant_id FROM scrobbles WHERE id = ?")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(variant, None, "and neither may it survive a bulk retag");
}

/// **The Patch screens must not scan the history once per row.**
///
/// Written after v0.80.0 took the real instance down. `patch_artists`,
/// `discover_decks` and `deck_view` each asked, per outer row, for a count
/// or an `EXISTS` keyed on `LOWER(TRIM(artist))` — a join key that is a
/// function of a column, which no index can serve, so each one is a full
/// scan of the whole history *per artist* or *per user*. On a laptop with
/// three listens they were instant. On a 50,000-listen instance they held a
/// pool connection for minutes, and once every connection was held the
/// whole application stopped: sessions could not be validated, the engine
/// could not read its sources, and `auth_status` reported the instance as
/// having no users at all.
///
/// This asserts on the **query plan**, which is hardware-independent, where
/// a timing is not — the rule v0.58.0 arrived at. One `SCAN scrobbles` is
/// the honest cost of a grouped aggregation; two or more means something is
/// being re-scanned per row.
/// The language queue answers the same thing, without scanning per track.
///
/// The old statement got its duration from a correlated subquery joined on
/// `LOWER(TRIM(d.title)) = LOWER(TRIM(s.title))`, which no index can serve
/// — a full scan of the history per candidate. On the real instance that
/// was 357s average and 753s worst, five at a time against a pool of five,
/// for two days. A faster query that answers differently is not a fix, so
/// this runs **the old SQL beside the new method** and compares.
#[tokio::test]
async fn the_language_queue_reads_the_history_twice_and_answers_the_same() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    // Three tracks wanting a language, one already resolved, one skipped,
    // and one by the placeholder artist — the four exclusions in the
    // predicate, so a rewrite that drops one is caught here.
    let seed: &[(&str, &str, &str, Option<i64>, Option<&str>, i64)] = &[
        ("Star", "Heize", "Late Autumn", Some(211), None, 0),
        ("Star", "Heize", "Late Autumn", Some(211), None, 0),
        ("Star", "Heize", "Late Autumn", Some(999), None, 0),
        ("Adieu", "Cœur de pirate", "Blonde", Some(184), None, 0),
        ("Adieu", "Cœur de pirate", "Blonde", None, None, 0),
        ("Xtal", "Aphex Twin", "SAW 85-92", None, Some("English/Other"), 0),
        ("Teardrop", "Massive Attack", "Mezzanine", Some(330), Some("English"), 0),
        ("Skipped", "Some Artist", "X", Some(120), None, 1),
        ("Comp", "Various Artists", "X", Some(120), None, 0),
    ];
    for (i, (title, artist, album, dur, lang, skipped)) in seed.iter().enumerate() {
        sqlx::query(
            "INSERT INTO scrobbles
                   (user_id, title, artist, album, duration, language, skipped,
                    timestamp, source_id, source_name, status)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'test', 'imported')",
        )
        .bind(uid)
        .bind(title)
        .bind(artist)
        .bind(album)
        .bind(dur)
        .bind(lang)
        .bind(skipped)
        .bind(1_760_000_000i64 + i as i64)
        .bind(format!("seed-{i}"))
        .execute(&db.pool)
        .await
        .unwrap();
    }

    // The statement exactly as it was before the rewrite.
    let old_sql = format!(
        "SELECT title, artist, MAX(album) AS album,
                    (SELECT duration FROM scrobbles d
                      WHERE LOWER(TRIM(d.title)) = LOWER(TRIM(s.title))
                        AND LOWER(TRIM(d.artist)) = LOWER(TRIM(s.artist))
                        AND d.duration IS NOT NULL
                      GROUP BY d.duration ORDER BY COUNT(*) DESC LIMIT 1) AS duration,
                    COUNT(*) AS plays
               FROM scrobbles s
              WHERE skipped = FALSE
                AND (language IS NULL OR language = '{}')
                AND TRIM(title) != '' AND TRIM(artist) != ''
                AND LOWER(TRIM(artist)) {NOT_VARIOUS}
              GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
              ORDER BY plays DESC LIMIT ?",
        crate::language::DEFAULT_LANGUAGE
    );
    let mut old: Vec<(String, String, Option<String>, Option<i64>, i64)> =
        sqlx::query_as(&old_sql).bind(50i64).fetch_all(&db.pool).await.unwrap();
    // Flattened to the shape the old statement returned, so the comparison
    // is about the answer rather than about the struct that now carries it.
    let mut new: Vec<(String, String, Option<String>, Option<i64>, i64)> = db
        .tracks_wanting_language(50)
        .await
        .unwrap()
        .into_iter()
        .map(|c| (c.title, c.artist, c.album, c.duration, c.plays))
        .collect();
    old.sort();
    new.sort();
    assert_eq!(new, old, "the rewrite must return what the old statement returned");

    // And it really is answering, not returning an empty set that happens
    // to match a broken `old`.
    assert_eq!(new.len(), 3, "three tracks want a language: {new:?}");
    let star = new.iter().find(|r| r.0 == "Star").unwrap();
    assert_eq!(star.3, Some(211), "the modal duration, not the outlier");
    assert_eq!(star.4, 3, "plays");
    let adieu = new.iter().find(|r| r.0 == "Adieu").unwrap();
    assert_eq!(adieu.3, Some(184), "a track with one known duration keeps it");
    assert!(
        new.iter().all(|r| r.1 != "Various Artists" && r.0 != "Skipped"),
        "the placeholder artist and skips stay out"
    );

    // The count must describe the same population as the listing, or the
    // Settings card cannot reach the end of its own queue.
    assert_eq!(
        db.count_tracks_wanting_language().await.unwrap(),
        new.len() as i64,
        "the count and the listing must agree"
    );

    // And the plan: no correlated subquery, and the history scanned once
    // per statement rather than once per candidate.
    for (name, sql) in [
        (
            "candidates",
            format!(
                "SELECT LOWER(TRIM(title)) || CHAR(31) || LOWER(TRIM(artist)) AS k,
                        title, artist, MAX(album), COUNT(*) AS plays
                   FROM scrobbles WHERE {}
                  GROUP BY LOWER(TRIM(title)), LOWER(TRIM(artist))
                  ORDER BY plays DESC LIMIT 50",
                Database::language_queue_predicate()
            ),
        ),
        (
            "durations",
            "SELECT LOWER(TRIM(title)) || CHAR(31) || LOWER(TRIM(artist)) AS k,
                        duration, COUNT(*) AS n
                   FROM scrobbles
                  WHERE duration IS NOT NULL AND skipped = FALSE
                    AND LOWER(TRIM(title)) || CHAR(31) || LOWER(TRIM(artist)) IN ('a')
                  GROUP BY k, duration"
                .to_string(),
        ),
    ] {
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {sql}"))
                .fetch_all(&db.pool)
                .await
                .unwrap_or_default();
        let plan: Vec<String> = rows.into_iter().map(|r| r.3).collect();
        assert!(
            !plan.iter().any(|l| l.contains("CORRELATED")),
            "{name} has a correlated subquery — that is the bug. Plan:\n{}",
            plan.join("\n")
        );
        let scans = plan.iter().filter(|l| l.contains("SCAN scrobbles")).count();
        assert!(scans <= 1, "{name} scans scrobbles {scans} times. Plan:\n{}", plan.join("\n"));
    }
}

#[tokio::test]
async fn the_patch_screens_scan_the_history_once() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_100).await;
    listen(&db, me, None, "Risingson", "Massive Attack", 1_760_000_200).await;

    // The three queries as the handlers issue them, by plan.
    let plans: Vec<(&str, String)> = vec![
        (
            "patch_artists",
            format!(
                "SELECT s.artist, COUNT(*) {} GROUP BY LOWER(TRIM(s.artist)) LIMIT 50",
                Database::PATCH_VISIBLE_LISTENS
            ),
        ),
        (
            "my_artist_plays",
            "SELECT LOWER(TRIM(artist)), COUNT(*) FROM scrobbles
                   WHERE user_id = ? AND skipped = FALSE GROUP BY LOWER(TRIM(artist))"
                .into(),
        ),
        (
            "discover overlap",
            "SELECT s.user_id, s.artist, COUNT(*) FROM scrobbles s
                   JOIN signal_chains c ON c.id = s.chain_id AND c.shared = 1
                  WHERE s.skipped = FALSE GROUP BY s.user_id, LOWER(TRIM(s.artist))"
                .into(),
        ),
    ];

    for (name, sql) in plans {
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(&format!("EXPLAIN QUERY PLAN {sql}"))
                .bind(me)
                .bind(me)
                .fetch_all(&db.pool)
                .await
                .unwrap_or_default();
        let plan: Vec<String> = rows.into_iter().map(|r| r.3).collect();
        let scans = plan.iter().filter(|l| l.contains("SCAN scrobbles")).count();
        assert!(
            scans <= 1,
            "{name} scans `scrobbles` {scans} times — a correlated subquery keyed on a \
                 function of a column is a full scan per row, which is what took the instance \
                 down. Plan:\n{}",
            plan.join("\n")
        );
        assert!(
            !plan.iter().any(|l| l.contains("CORRELATED")),
            "{name} has a correlated subquery. Plan:\n{}",
            plan.join("\n")
        );
    }
}

/// The overlap has to keep *working*, not just be fast — the rewrite folds
/// two grouped scans in Rust instead of asking SQLite, and a fold that
/// silently matches nothing would look exactly like "you have nothing in
/// common".
#[tokio::test]
async fn the_overlap_still_finds_what_you_both_play() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_100).await;
    listen(&db, them, Some(chain), "Hallogallo", "Neu!", 1_760_000_200).await;
    // Mine, spelled differently — the fold is the point.
    listen(&db, me, None, "Risingson", "  massive attack ", 1_760_000_300).await;

    let artists = db.patch_artists(me, 50, None).await.unwrap();
    let ma = artists.iter().find(|a| a.name == "Massive Attack").expect("massive attack");
    assert_eq!(ma.mine, 1, "case and spacing must not stop the overlap being found");
    assert_eq!(artists.iter().find(|a| a.name == "Neu!").unwrap().mine, 0);

    let deck = db.deck_view(me, "kim").await.unwrap().expect("deck");
    assert_eq!(deck.both_play, ["Massive Attack"]);

    let decks = db.discover_decks(me).await.unwrap();
    let kim = decks.iter().find(|d| d.username == "kim").expect("kim");
    assert_eq!(kim.overlap, "Massive Attack");
    assert!(!decks.iter().any(|d| d.user_id == me), "you are not a deck to discover");
}

/// Opening a database that an *older* Tapedeck wrote.
///
/// Every test above starts from `Database::new` on an empty file, which
/// takes the `init()` path and never exercises `migrate()` against a real
/// prior schema — so a migration that cannot run on an existing database
/// passes the whole suite and fails on every install that has data in it.
/// v0.61.0 shipped exactly that: an index on `notes(period_key)` in
/// `init()`, which runs *before* `migrate()` adds the column, propagating
/// with `?` so the process exited and systemd restarted it forever.
#[tokio::test]
async fn an_existing_database_opens_and_gains_period_notes() {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-old-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }

    // The pre-v0.61.0 shape of every table the notes migration touches,
    // including the CHECK that the rebuild exists to widen.
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, username TEXT NOT NULL,
                     email TEXT, password_hash TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'user',
                     created_at INTEGER NOT NULL DEFAULT 0, settings TEXT)",
            "CREATE TABLE entities (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL,
                     name TEXT NOT NULL, artist_name TEXT NOT NULL DEFAULT '', mbid TEXT)",
            "CREATE TABLE scrobbles (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER,
                     title TEXT NOT NULL, artist TEXT NOT NULL, timestamp INTEGER NOT NULL,
                     source_id TEXT NOT NULL, source_name TEXT NOT NULL)",
            "CREATE TABLE notes (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     user_id INTEGER NOT NULL REFERENCES users(id),
                     entity_id INTEGER REFERENCES entities(id),
                     scrobble_id INTEGER REFERENCES scrobbles(id),
                     body_md TEXT NOT NULL,
                     visibility TEXT NOT NULL DEFAULT 'private',
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL,
                     CHECK ((entity_id IS NULL) <> (scrobble_id IS NULL)))",
            "CREATE TABLE note_revisions (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     note_id INTEGER NOT NULL REFERENCES notes(id), body_md TEXT NOT NULL,
                     created_at INTEGER NOT NULL)",
            "INSERT INTO users (id, username, password_hash) VALUES (1, 'ashwin', 'x')",
            "INSERT INTO entities (id, kind, name) VALUES (1, 'release', 'Untrue')",
            "INSERT INTO notes (user_id, entity_id, body_md, visibility, created_at, updated_at)
                     VALUES (1, 1, 'what I thought in 2026', 'private', 100, 100)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    // The whole point: this is what systemd was running in a loop.
    let db = Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
        .await
        .expect("an existing database must open");

    // The note that was already there survived the table rebuild.
    let kept = db.note_for_entity(1, 1).await.unwrap().expect("the old note is still there");
    assert_eq!(kept.body_md, "what I thought in 2026");
    assert!(kept.period_key.is_none());

    // And the thing the migration was for actually works afterwards.
    let target = NoteTarget::period("month", "2026-08").unwrap();
    db.upsert_note(1, &target, "the month Bach took over", "private").await.unwrap();
    let found = db.note_for_period(1, "month:2026-08").await.unwrap().expect("period note");
    assert_eq!(found.body_md, "the month Bach took over");
}

/// A database on the previous release opens and gains pins.
///
/// The third CHECK rebuild, and the same check
/// `an_existing_database_opens_and_gains_period_notes` exists for — every
/// other test in this file starts from an empty file, takes the `init()`
/// path, and therefore never runs `migrate()` against a real prior schema.
/// That is exactly how a migration that cannot run on an existing database
/// passes the whole suite, which is what v0.61.1 was.
///
/// The prior schema here is the **three-target** one, CHECK and all: a
/// `line_key` added by `ALTER TABLE` would sit under a constraint that
/// counts three targets and would reject every pin as having none.
#[tokio::test]
async fn an_existing_database_opens_and_gains_pinned_lines() {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-oldpin-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }

    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, username TEXT NOT NULL,
                     email TEXT, password_hash TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'user',
                     created_at INTEGER NOT NULL DEFAULT 0, settings TEXT)",
            "CREATE TABLE entities (id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL,
                     name TEXT NOT NULL, artist_name TEXT NOT NULL DEFAULT '', mbid TEXT)",
            "CREATE TABLE scrobbles (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER,
                     title TEXT NOT NULL, artist TEXT NOT NULL, timestamp INTEGER NOT NULL,
                     source_id TEXT NOT NULL, source_name TEXT NOT NULL)",
            "CREATE TABLE notes (
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
                            + (period_key IS NOT NULL) = 1))",
            "CREATE TABLE note_revisions (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     note_id INTEGER NOT NULL REFERENCES notes(id), body_md TEXT NOT NULL,
                     created_at INTEGER NOT NULL)",
            "INSERT INTO users (id, username, password_hash) VALUES (1, 'ashwin', 'x')",
            "INSERT INTO entities (id, kind, name) VALUES (1, 'release', 'Untrue')",
            "INSERT INTO notes (user_id, entity_id, body_md, visibility, created_at, updated_at)
                     VALUES (1, 1, 'what I thought in 2026', 'private', 100, 100)",
            "INSERT INTO notes (user_id, period_key, body_md, visibility, created_at, updated_at)
                     VALUES (1, 'month:2026-08', 'the month Bach took over', 'private', 100, 100)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    let db = Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
        .await
        .expect("an existing database must open");

    // Both older targets survived the rebuild, with their own targets
    // intact — the insert names its columns on both sides precisely so a
    // period note does not come back as an entity one.
    let kept = db.note_for_entity(1, 1).await.unwrap().expect("the old note is still there");
    assert_eq!(kept.body_md, "what I thought in 2026");
    assert!(kept.line_key.is_none());
    let month = db.note_for_period(1, "month:2026-08").await.unwrap().expect("period note");
    assert_eq!(month.body_md, "the month Bach took over");

    // And the thing the migration was for actually works afterwards.
    let target = NoteTarget::line("Burial", "Archangel", 12).unwrap();
    db.upsert_note(1, &target, "the whole record turns here", "private").await.unwrap();
    let pins = db.pins_for_track(1, "burial", "  ARCHANGEL ").await.unwrap();
    assert_eq!(pins.len(), 1, "a pin is found under any spelling of the fold");
    assert_eq!(pins[0].body_md, "the whole record turns here");
    assert_eq!(
        NoteTarget::line_number(pins[0].line_key.as_deref().unwrap()),
        Some(12),
        "the line number survives the round trip"
    );
    assert_eq!(
        NoteTarget::line_names(pins[0].line_label.as_deref().unwrap()),
        Some(("Burial", "Archangel")),
        "and so does the spelling, which the folded key cannot carry"
    );
}

/// A pin is one note per line, edited in place — and it does not collide
/// with a pin on a *different* line of the same track, or with the same
/// line of a different track.
///
/// The prefix match in `pins_for_track` is the part worth pinning: keying
/// on the track without the trailing separator would let "Archangel" match
/// "Archangel (Live)", which is a different recording with different words.
#[tokio::test]
async fn a_pin_is_one_note_per_line_and_does_not_bleed_between_tracks() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let twelve = NoteTarget::line("Burial", "Archangel", 12).unwrap();
    db.upsert_note(uid, &twelve, "first thought", "private").await.unwrap();
    db.upsert_note(uid, &twelve, "second thought", "private").await.unwrap();

    let pins = db.pins_for_track(uid, "Burial", "Archangel").await.unwrap();
    assert_eq!(pins.len(), 1, "the second write edits the first, as every other target does");
    assert_eq!(pins[0].body_md, "second thought");
    assert_eq!(
        db.note_revisions(uid, pins[0].id).await.unwrap().len(),
        1,
        "and what it said before is kept"
    );

    // A second line of the same track is a second pin.
    db.upsert_note(uid, &NoteTarget::line("Burial", "Archangel", 24).unwrap(), "later", "private")
        .await
        .unwrap();
    assert_eq!(db.pins_for_track(uid, "Burial", "Archangel").await.unwrap().len(), 2);

    // The same line number on a track whose title merely *starts* the same
    // is not this track's pin.
    db.upsert_note(
        uid,
        &NoteTarget::line("Burial", "Archangel (Live)", 12).unwrap(),
        "the live one",
        "private",
    )
    .await
    .unwrap();
    let pins = db.pins_for_track(uid, "Burial", "Archangel").await.unwrap();
    assert_eq!(pins.len(), 2, "a prefix of a title is a different track");

    // And nothing malformed reaches the table at all.
    assert!(NoteTarget::line("Burial", "Archangel", 0).is_none(), "line zero is not a line");
    assert!(NoteTarget::line("", "Archangel", 3).is_none());
    assert!(NoteTarget::line("Burial", "   ", 3).is_none());
}

/// A pin rolls up under the record it is on, and under nothing else.
///
/// The footnote form is the design's third rendering, and the thing worth
/// pinning is its **scope**: a record's note gathers the pins on tracks of
/// that record, and a pin on some other record must not appear under it.
/// Getting that wrong is not a visual bug — it puts a footnote about one
/// evening's listening under an unrelated piece of writing.
#[tokio::test]
async fn a_pin_rolls_up_under_its_own_record_and_no_other() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    // Two records, each with a track that has been played — the tracklist a
    // note's footnotes are gathered from is the *listening*, not a provider.
    for (title, album, ts) in [
        ("Archangel", "Untrue", 1_700_000_100),
        ("Shell of Light", "Untrue", 1_700_000_200),
        ("Ghost Hardware", "Burial", 1_700_000_300),
    ] {
        let mut p = play(title, "Burial", ts);
        p.album = Some(album.into());
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    for (title, n) in [("Archangel", 12), ("Shell of Light", 4), ("Ghost Hardware", 7)] {
        let t = NoteTarget::line("Burial", title, n).unwrap();
        db.upsert_note(uid, &t, &format!("about {title} line {n}"), "private").await.unwrap();
    }

    // Everything is in the one place the roll-up reads from...
    let all = db.line_notes(uid, 500).await.unwrap();
    assert_eq!(all.len(), 3);

    // ...and the scope is what narrows it. Folded here the way the handler
    // folds it, which is the half that decides where a footnote lands.
    let untrue: Vec<String> = db
        .album_track_list(uid, "Burial", "Untrue")
        .await
        .unwrap()
        .into_iter()
        .map(|(t, ..)| t.trim().to_lowercase())
        .collect();
    let under_untrue: Vec<&str> = all
        .iter()
        .filter(|n| {
            let label = n.line_label.as_deref().unwrap();
            let (_, title) = NoteTarget::line_names(label).unwrap();
            untrue.contains(&title.trim().to_lowercase())
        })
        .map(|n| n.body_md.as_str())
        .collect();

    assert_eq!(under_untrue.len(), 2, "both Untrue pins, and only those");
    assert!(under_untrue.iter().all(|b| !b.contains("Ghost Hardware")));
}

/// The same check for `equipment.status`, and for the same reason: every
/// other test in this file starts from an empty file, which takes the
/// `init()` path and never exercises `migrate()` against a real prior
/// schema. A column added only to `init()` opens fine here and crash-loops
/// on every database that already exists.
#[tokio::test]
async fn an_existing_database_gains_the_gear_status_column() {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-oldgear-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }

    // The pre-status shape, with a row already in it.
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, username TEXT NOT NULL,
                     email TEXT, password_hash TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'user',
                     created_at INTEGER NOT NULL DEFAULT 0, settings TEXT)",
            "CREATE TABLE equipment (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     user_id INTEGER NOT NULL REFERENCES users(id),
                     name TEXT NOT NULL, type TEXT NOT NULL, brand TEXT, model TEXT,
                     total_hours REAL DEFAULT 0, first_used INTEGER, last_used INTEGER,
                     notes TEXT, purchased_at INTEGER, UNIQUE(user_id, name))",
            "INSERT INTO users (id, username, password_hash) VALUES (1, 'ashwin', 'x')",
            "INSERT INTO equipment (user_id, name, type, brand, notes)
                     VALUES (1, 'HD 650', 'headphones', 'Sennheiser', 'the good pads')",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    let db = Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
        .await
        .expect("an existing database must open");

    let gear = db.get_equipment(1).await.unwrap();
    assert_eq!(gear.len(), 1);
    assert_eq!(gear[0].name, "HD 650");
    assert!(gear[0].status.is_none(), "nothing has been said about it yet");

    // And the column is writable, which is the half the migration is for.
    db.update_equipment(
        1,
        gear[0].id,
        "HD 650",
        "headphones",
        Some("Sennheiser"),
        None,
        Some("the good pads"),
        None,
        Some("spare"),
    )
    .await
    .unwrap();
    let after = db.get_equipment(1).await.unwrap();
    assert_eq!(after[0].status.as_deref(), Some("spare"));
    assert_eq!(after[0].notes.as_deref(), Some("the good pads"));
}

/// Every path that writes a listen has to link its credits, and an import
/// is the one that does not go through `save_scrobble`.
///
/// This does not degrade gracefully. A listen with no row in
/// `scrobble_artists` is invisible to every artist page and every chart,
/// because those fan out over that table — and the startup backfill only
/// covers what was already stored, so an import run after the upgrade would
/// produce listens that appear nowhere at all.
#[tokio::test]
async fn an_imported_listen_is_credited_too() {
    let db = testdb().await;
    let uid = db.create_user("mira", None, "x", "user").await.unwrap();

    let mut p = play("Gymnopédie No. 1", "Erik Satie", 1_000);
    p.artists = Some(vec!["Erik Satie".into(), "Aldo Ciccolini".into()]);
    db.import_listen_flagged(uid, &p, false).await.unwrap();

    assert_eq!(db.artist_plays(uid, "Erik Satie").await.unwrap(), 1);
    assert_eq!(db.artist_plays(uid, "Aldo Ciccolini").await.unwrap(), 1);
    assert!(db.artist_summary(uid, "Aldo Ciccolini").await.unwrap().is_some());

    // And it reaches the chart, which is the readout that would silently
    // have been short a listen.
    let chart = db.chart(uid, "artists", None, 10, false).await.unwrap();
    assert!(chart.iter().any(|r| r.name == "Erik Satie"));
}

/// The credit has been collected in `artist_credit` since v0.17.0 and read
/// by almost nothing, so an existing history already holds the answer — the
/// join was all that was missing. A backfill that only worked for new
/// listens would read as broken for everybody except a new user.
#[tokio::test]
async fn the_backfill_credits_a_history_that_already_exists() {
    let db = testdb().await;
    let uid = db.create_user("lena", None, "x", "user").await.unwrap();

    let mut p = play("Clair de lune", "Claude Debussy", 1_000);
    p.artists = Some(vec!["Claude Debussy".into(), "Pascal Rogé".into()]);
    db.save_scrobble(uid, &p, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap();

    // Wind it back to the state a pre-v0.61 database is in: the credit
    // string is there, the join rows are not.
    sqlx::query("DELETE FROM scrobble_artists").execute(&db.pool).await.unwrap();
    assert_eq!(db.artist_plays(uid, "Pascal Rogé").await.unwrap(), 0);

    db.backfill_scrobble_artists().await.unwrap();

    assert_eq!(db.artist_plays(uid, "Pascal Rogé").await.unwrap(), 1);
    assert_eq!(db.artist_plays(uid, "Claude Debussy").await.unwrap(), 1);

    // Idempotent — it runs at startup, and a second pass must not double
    // anybody's listening.
    db.backfill_scrobble_artists().await.unwrap();
    assert_eq!(db.artist_plays(uid, "Pascal Rogé").await.unwrap(), 1);
}

#[tokio::test]
async fn a_period_is_the_third_thing_a_note_can_be_about() {
    let db = testdb().await;
    let uid = db.create_user("juno", None, "x", "user").await.unwrap();

    let august = NoteTarget::period("month", "2026-08").expect("a real month");
    let id = db.upsert_note(uid, &august, "the month of Untrue", "private").await.unwrap();

    let found = db.note_for_period(uid, "month:2026-08").await.unwrap().expect("stored");
    assert_eq!(found.id, id);
    assert_eq!(found.body_md, "the month of Untrue");
    assert!(found.entity_id.is_none() && found.scrobble_id.is_none());

    // One note per period per user: the second write edits the first.
    db.upsert_note(uid, &august, "the month of Untrue, mostly", "private").await.unwrap();
    assert_eq!(db.list_notes(uid).await.unwrap().len(), 1);
    assert_eq!(db.note_revisions(uid, id).await.unwrap().len(), 1, "the edit was archived");

    // The three units do not collide, even where their keys could be read
    // as one another.
    let year = NoteTarget::period("year", "2026").expect("a real year");
    db.upsert_note(uid, &year, "the year of it", "private").await.unwrap();
    let week = NoteTarget::period("week", "2026-W32").expect("a real week");
    db.upsert_note(uid, &week, "that week", "private").await.unwrap();
    assert_eq!(db.list_notes(uid).await.unwrap().len(), 3);

    // Its own key is searchable, which is what a note with no entity and no
    // listen has to be found by.
    let hits = db.search_notes(uid, "2026-W32", "", "updated").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].period_key.as_deref(), Some("week:2026-W32"));

    // And the kind filter separates them from records and listens.
    assert_eq!(db.search_notes(uid, "", "period", "updated").await.unwrap().len(), 3);
    assert_eq!(db.search_notes(uid, "", "release", "updated").await.unwrap().len(), 0);

    // Shapes Reports does not produce are refused rather than stored.
    for (unit, key) in [
        ("month", "2026-13"), // no such month
        ("month", "2026-8"),  // unpadded
        ("week", "2026-W54"), // past the longest ISO year
        ("week", "2026-32"),  // missing the W
        ("year", "26"),       // not four digits
        ("decade", "2020"),   // not a unit Reports has
    ] {
        assert!(NoteTarget::period(unit, key).is_none(), "{unit} {key} should not be a period");
    }
}

#[tokio::test]
async fn a_record_and_one_playing_of_it_hold_separate_notes() {
    let db = testdb().await;
    let uid = db.create_user("bo", None, "x", "user").await.unwrap();
    db.save_scrobble(
        uid,
        &play("Xtal", "Aphex Twin", 1000),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
    let listen = db
        .get_recent_scrobbles(uid, 1, 0, None, None, None, None, None, None)
        .await
        .unwrap()
        .remove(0);
    let e = db.upsert_entity("recording", "Xtal", "Aphex Twin", None).await.unwrap();

    db.upsert_note(uid, &NoteTarget::Entity(e), "about the record", "private").await.unwrap();
    db.upsert_note(uid, &NoteTarget::Listen(listen.id), "heard this on the train", "private")
        .await
        .unwrap();

    assert_eq!(db.list_notes(uid).await.unwrap().len(), 2);
    assert_eq!(db.note_for_entity(uid, e).await.unwrap().unwrap().body_md, "about the record");
    assert_eq!(
        db.note_for_scrobble(uid, listen.id).await.unwrap().unwrap().body_md,
        "heard this on the train"
    );

    // Deleting the listen takes its note with it — foreign keys are on, so
    // without that cleanup the delete would fail outright — and leaves the
    // note about the record alone.
    assert!(db.delete_scrobble(uid, listen.id).await.unwrap());
    assert_eq!(db.list_notes(uid).await.unwrap().len(), 1);
    assert!(db.note_for_entity(uid, e).await.unwrap().is_some());
}

#[tokio::test]
async fn searching_notes_looks_at_the_body_and_the_subject_both() {
    let db = testdb().await;
    let uid = db.create_user("kit", None, "x", "user").await.unwrap();
    let other = db.create_user("nell", None, "x", "user").await.unwrap();

    let record = db.upsert_entity("release", "Untrue", "Burial", None).await.unwrap();
    let artist = db.upsert_entity("artist", "Aphex Twin", "", None).await.unwrap();
    db.upsert_note(uid, &NoteTarget::Entity(record), "the rain on this one", "private")
        .await
        .unwrap();
    db.upsert_note(
        uid,
        &NoteTarget::Entity(artist),
        "everything after Untrue owes this",
        "private",
    )
    .await
    .unwrap();
    // Someone else's note mentioning the same word must never come back.
    let theirs = db.upsert_entity("release", "Untrue", "Burial", None).await.unwrap();
    db.upsert_note(other, &NoteTarget::Entity(theirs), "not yours", "private").await.unwrap();

    // The subject matches for one, the body for the other — a search that
    // only did one of those would find half of what was asked for.
    let hits = db.search_notes(uid, "untrue", "", "updated").await.unwrap();
    assert_eq!(hits.len(), 2, "the record by name, the artist by its body");

    let by_body = db.search_notes(uid, "rain", "", "updated").await.unwrap();
    assert_eq!(by_body.len(), 1);
    assert_eq!(by_body[0].body_md, "the rain on this one");

    // ASCII case folding is what SQLite's LIKE gives, and it is the case
    // people actually type.
    assert_eq!(db.search_notes(uid, "BURIAL", "", "updated").await.unwrap().len(), 1);

    // The kind filter, and an unrecognised one meaning "no filter" rather
    // than "nothing" — a stale bookmark should not look like an empty
    // account.
    assert_eq!(db.search_notes(uid, "", "artist", "updated").await.unwrap().len(), 1);
    assert_eq!(db.search_notes(uid, "", "release", "updated").await.unwrap().len(), 1);
    assert_eq!(db.search_notes(uid, "", "listen", "updated").await.unwrap().len(), 0);
    assert_eq!(db.search_notes(uid, "", "nonsense", "updated").await.unwrap().len(), 2);

    // A wildcard typed into the box is a literal, not a pattern matching
    // everything.
    assert_eq!(db.search_notes(uid, "%", "", "updated").await.unwrap().len(), 0);
    assert_eq!(db.search_notes(uid, "_", "", "updated").await.unwrap().len(), 0);

    // Alphabetical sorts by subject, which is not the same order as by edit.
    let titled = db.search_notes(uid, "", "", "title").await.unwrap();
    assert_eq!(titled.len(), 2);
    assert_eq!(titled[0].entity_id, Some(artist), "Aphex Twin before Untrue");
}

#[tokio::test]
async fn notes_and_revisions_are_scoped_to_their_owner() {
    let db = testdb().await;
    let ann = db.create_user("ann2", None, "x", "user").await.unwrap();
    let bob = db.create_user("bob2", None, "x", "user").await.unwrap();
    let e = db.upsert_entity("artist", "Aphex Twin", "", None).await.unwrap();

    let id = db.upsert_note(ann, &NoteTarget::Entity(e), "mine", "public").await.unwrap();
    db.upsert_note(ann, &NoteTarget::Entity(e), "mine, edited", "public").await.unwrap();

    // Even a `public` note isn't served to anyone else yet — patches don't
    // exist, so every read path is still self-only.
    assert!(db.get_note(bob, id).await.unwrap().is_none());
    assert!(db.note_for_entity(bob, e).await.unwrap().is_none());
    assert!(db.note_revisions(bob, id).await.unwrap().is_empty());
    assert!(!db.delete_note(bob, id).await.unwrap());
    assert!(db.get_note(ann, id).await.unwrap().is_some());

    // Both users can annotate the same entity without colliding.
    let bobs = db.upsert_note(bob, &NoteTarget::Entity(e), "his", "private").await.unwrap();
    assert_ne!(id, bobs);
}

#[tokio::test]
async fn shortcodes_resolve_through_the_listening_history() {
    let db = testdb().await;
    let uid = db.create_user("cy", None, "x", "user").await.unwrap();

    let mut p = play("Ageispolis", "Aphex Twin", 1000);
    p.album = Some("SAW 85-92".into());
    p.track_number = Some(3);
    db.save_scrobble(uid, &p, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap();

    // A named track with nothing annotated yet still resolves, because the
    // user has actually played it.
    let (id, label) = db
        .resolve_reference(uid, "recording", "ageispolis", None, Some("Aphex Twin"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(label, "Ageispolis");

    // [[track:3]] means the third track of the record in context, and
    // renders with its real title rather than the number.
    let (pos_id, pos_label) = db
        .resolve_reference(uid, "recording", "3", Some(3), Some("Aphex Twin"), Some("SAW 85-92"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pos_label, "Ageispolis");
    assert_eq!(pos_id, id);

    // A position with no album in scope, and a track nobody has heard,
    // resolve to nothing — which renders as plain text, not an error.
    assert!(db
        .resolve_reference(uid, "recording", "3", Some(3), None, None)
        .await
        .unwrap()
        .is_none());
    assert!(db
        .resolve_reference(uid, "recording", "Windowlicker", None, None, None)
        .await
        .unwrap()
        .is_none());

    // Another user's history is not a resolution source.
    let other = db.create_user("dee", None, "x", "user").await.unwrap();
    assert!(db
        .resolve_reference(other, "release", "SAW 85-92", None, None, None)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn loves_toggle_and_stay_per_user() {
    let db = testdb().await;
    let ann = db.create_user("ann3", None, "x", "user").await.unwrap();
    let bob = db.create_user("bob3", None, "x", "user").await.unwrap();
    let e = db.upsert_entity("recording", "Xtal", "Aphex Twin", None).await.unwrap();

    db.set_love(ann, e, true).await.unwrap();
    // Loving twice is not two loves.
    db.set_love(ann, e, true).await.unwrap();
    assert!(db.is_loved(ann, e).await.unwrap());
    assert!(!db.is_loved(bob, e).await.unwrap());
    assert_eq!(db.list_loves(ann).await.unwrap().len(), 1);
    assert!(db.list_loves(bob).await.unwrap().is_empty());

    db.set_love(ann, e, false).await.unwrap();
    assert!(!db.is_loved(ann, e).await.unwrap());
}

/// A fidelity share is of the listens whose format is known, and the mix's
/// `Unknown` bucket is exactly the rest.
///
/// The bug this pins: `is_lossless` is nullable and an imported listen has
/// nothing in it — 97.4% of a real 21,390-listen export carries no format
/// at all — so dividing by every play scored "never knew" as "not
/// lossless". Someone who has only ever played FLAC read 2%. Nothing
/// errored; the number was just a measure of how much history they had
/// imported.
///
/// It asserts on the *readouts* and on the two agreeing, not on the SQL,
/// because the failure mode is a third fidelity query written later beside
/// these two — the same reason `a_skip_reaches_no_counting_readout` is
/// written the way it is.
#[tokio::test]
async fn a_fidelity_share_ignores_listens_of_unknown_format() {
    let db = testdb().await;
    let uid = db.create_user("fid", None, "pw", "user").await.unwrap();
    let now = Database::now_secs();

    let flac = AudioQuality {
        codec: Some("flac".into()),
        is_lossless: Some(true),
        format_type: Some("pcm".into()),
        ..Default::default()
    };
    let mp3 = AudioQuality {
        codec: Some("mp3".into()),
        is_lossless: Some(false),
        format_type: Some("pcm".into()),
        ..Default::default()
    };

    // Three listens Tapedeck saw the format of: two lossless, one not.
    // Separately titled and spaced an hour apart so `fuzzy_exists` reads
    // them as three plays rather than one play reported three times.
    for (i, q) in [(0i64, &flac), (1, &flac), (2, &mp3)] {
        let p = play(&format!("Known {i}"), "A", (now - 3600 * (i + 1)) as u64);
        db.save_scrobble(
            uid,
            &p,
            Some(q),
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }
    // And ninety-seven with no format at all, which is roughly the ratio a
    // real imported history has.
    for i in 0..97i64 {
        let p = play(&format!("Imported {i}"), "A", (now - 400_000 - 3600 * i) as u64);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    let dash = db.get_dashboard_stats(uid, chrono_tz::UTC, 1).await.unwrap();
    assert_eq!(dash["total"], 100);
    assert_eq!(dash["fidelity_known"], 3);
    // Two of the three known, not two of a hundred.
    assert_eq!(dash["lossless_pct"], 67);

    let ranged = db.get_ranged_stats(uid, None, false).await.unwrap();
    assert_eq!(ranged["totals"]["fidelity_known"], 3);
    assert_eq!(ranged["totals"]["lossless_pct"], 67);

    // The mix and the share must split the same rows the same way, or the
    // page contradicts itself: `Unknown` is defined as the complement of
    // the share's population, so everything else has to add up to it.
    let mix = ranged["fidelity"].as_array().unwrap();
    let named = |n: &str| {
        mix.iter().find(|r| r["name"] == n).map(|r| r["plays"].as_i64().unwrap()).unwrap_or(0)
    };
    assert_eq!(named("Unknown"), 97);
    assert_eq!(named("Lossless") + named("Lossy") + named("DSD"), 3);
}

/// A hand-set cover is for a *release*, and only a release.
///
/// An artist's picture lives on the `artists` row — shared by name, and
/// what the library proxy and the portrait backfill both address. If this
/// query could write an artist entity's `image_url` too, the same artist
/// would have two places to be pictured from and no rule about which wins,
/// and nothing would error to say so.
#[tokio::test]
async fn a_cover_override_is_release_only_and_clearable() {
    let db = testdb().await;
    let album = db.upsert_entity("release", "Untrue", "Burial", None).await.unwrap();
    let artist = db.upsert_entity("artist", "Burial", "", None).await.unwrap();

    assert!(db.entity_image(album).await.unwrap().is_none());

    assert!(db.set_entity_image(album, Some("https://example.test/untrue.jpg")).await.unwrap());
    assert_eq!(
        db.entity_image(album).await.unwrap().as_deref(),
        Some("https://example.test/untrue.jpg")
    );

    // Refused, and reported as refused rather than quietly doing nothing.
    assert!(!db.set_entity_image(artist, Some("https://example.test/nope.jpg")).await.unwrap());
    assert!(db.entity_image(artist).await.unwrap().is_none());

    // Clearing restores "no override", which is what puts the cover derived
    // from the listens back on the page.
    assert!(db.set_entity_image(album, None).await.unwrap());
    assert!(db.entity_image(album).await.unwrap().is_none());
}
/// Build a chain of `(name, equipment_id)` steps.
async fn chain_of(db: &Database, uid: i64, name: &str, steps: &[(&str, i64)]) -> i64 {
    db.create_chain(&SignalChain {
        id: None,
        user_id: uid,
        name: name.into(),
        description: None,
        components: steps
            .iter()
            .map(|(n, g)| ChainComponent {
                component_type: ComponentType::Transducer,
                name: (*n).into(),
                detail: None,
                equipment_id: Some(*g),
            })
            .collect(),
        listening_context: ListeningContext::Active,
        icon: None,
        is_active: true,
        shared: false,
        total_hours: 0.0,
        created_at: 0,
    })
    .await
    .unwrap()
}

/// An hour of listening on a chain.
async fn hour_on(db: &Database, uid: i64, chain: i64, ts: u64) {
    let mut p = play(&format!("t{ts}"), "x", ts);
    p.duration = Some(3600);
    db.save_scrobble(
        uid,
        &p,
        None,
        None,
        Some(chain),
        None,
        None,
        None,
        None,
        ListenDisposition::LIVE,
    )
    .await
    .unwrap();
}

/// The complaint this exists for: two chains differing by one box, each
/// holding real hours, with no way to add them up.
///
/// After the fold there is **one** chain with a variant, the listening is
/// all on it, and — the part that matters — the gear hours resolve against
/// what was actually fitted. The shared phone gets both hours; each pair of
/// IEMs gets only its own.
#[tokio::test]
async fn folding_a_near_duplicate_chain_adds_its_hours_up() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let phone = db.upsert_equipment(uid, "Xperia", "dap", None, None, 0.0, None).await.unwrap();
    let hd569 =
        db.upsert_equipment(uid, "HD 569", "headphones", None, None, 0.0, None).await.unwrap();
    let hexa = db.upsert_equipment(uid, "Hexa", "iem", None, None, 0.0, None).await.unwrap();

    let commute =
        chain_of(&db, uid, "Commute Phone", &[("Xperia", phone), ("HD 569", hd569)]).await;
    let summer = chain_of(&db, uid, "Summer Commute", &[("Xperia", phone), ("Hexa", hexa)]).await;

    hour_on(&db, uid, commute, 1_000).await;
    hour_on(&db, uid, commute, 2_000).await;
    hour_on(&db, uid, summer, 3_000).await;

    // Before: two rows, and the phone's hours split across them with no
    // way to see that it is one phone.
    let (secs, _) = db.equipment_seconds(uid).await.unwrap();
    assert_eq!(secs.get(&phone), Some(&10_800), "3h through the phone either way");
    assert_eq!(secs.get(&hd569), Some(&7_200));
    assert_eq!(secs.get(&hexa), Some(&3_600));
    assert_eq!(db.get_chains(uid).await.unwrap().len(), 2);

    let out = db
        .fold_chain_into_variant(uid, summer, commute, FoldTarget::New("Hexa"))
        .await
        .unwrap()
        .expect("the fold is allowed");

    assert!(out.variant_created);
    assert_eq!(out.listens, 1, "the duplicate's listening moved");

    // One chain now, and it carries the whole three hours.
    let chains = db.get_chains(uid).await.unwrap();
    assert_eq!(chains.len(), 1, "the near-duplicate is gone");
    assert_eq!(chains[0].id, Some(commute));
    assert!(
        (chains[0].total_hours - 3.0).abs() < 1e-6,
        "hours added up: {:?}",
        chains[0].total_hours
    );

    // And the gear still reads correctly: the swap really was a swap, so
    // the HD 569 accrues nothing while the Hexa is in.
    let (secs, _) = db.equipment_seconds(uid).await.unwrap();
    assert_eq!(secs.get(&phone), Some(&10_800), "the phone was in the path throughout");
    assert_eq!(secs.get(&hd569), Some(&7_200), "no wear while the other IEMs were in");
    assert_eq!(secs.get(&hexa), Some(&3_600));

    // The variant carries its own share, which is what makes the two
    // readable side by side instead of as two unrelated chains.
    let variants = db.list_variants(uid).await.unwrap();
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0].name, "Hexa");
    assert!((variants[0].total_hours - 1.0).abs() < 1e-6);
}

/// Folding onto a variant the user built themselves is the case they are
/// actually in — the variant already exists beside the chain it duplicates.
///
/// Also pins the two refusals that keep a fold from crediting gear that was
/// never fitted: a variant belonging to some other chain, and folding a
/// chain into itself.
#[tokio::test]
async fn a_fold_lands_on_an_existing_variant_and_refuses_a_foreign_one() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let phone = db.upsert_equipment(uid, "Xperia", "dap", None, None, 0.0, None).await.unwrap();
    let hd569 =
        db.upsert_equipment(uid, "HD 569", "headphones", None, None, 0.0, None).await.unwrap();
    let hexa = db.upsert_equipment(uid, "Hexa", "iem", None, None, 0.0, None).await.unwrap();

    let commute =
        chain_of(&db, uid, "Commute Phone", &[("Xperia", phone), ("HD 569", hd569)]).await;
    let summer = chain_of(&db, uid, "Summer Commute", &[("Xperia", phone), ("Hexa", hexa)]).await;
    let desk = chain_of(&db, uid, "Desk", &[("Xperia", phone)]).await;

    // The variant they already built on the chain they mean to keep.
    let v = db
        .create_variant(
            uid,
            commute,
            "Summer",
            &[VariantOverride { position: 1, equipment_id: Some(hexa), name: Some("Hexa".into()) }],
        )
        .await
        .unwrap();
    // A variant of a *different* chain, to be refused.
    let foreign = db.create_variant(uid, desk, "Elsewhere", &[]).await.unwrap();

    hour_on(&db, uid, summer, 1_000).await;

    assert!(db
        .fold_chain_into_variant(uid, summer, commute, FoldTarget::Existing(foreign))
        .await
        .unwrap()
        .is_err());
    assert!(
        db.fold_chain_into_variant(uid, summer, summer, FoldTarget::New("x"))
            .await
            .unwrap()
            .is_err(),
        "a chain cannot be folded into itself"
    );
    // Neither refusal moved anything.
    assert_eq!(db.get_chains(uid).await.unwrap().len(), 3);

    let out = db
        .fold_chain_into_variant(uid, summer, commute, FoldTarget::Existing(v))
        .await
        .unwrap()
        .expect("their own variant of the target chain");
    assert!(!out.variant_created, "it reused the variant they built");
    assert_eq!(out.variant_id, v);
    assert_eq!(out.listens, 1);

    // The hour landed on the Hexa, via the variant they wired.
    let (secs, _) = db.equipment_seconds(uid).await.unwrap();
    assert_eq!(secs.get(&hexa), Some(&3_600));
    assert_eq!(secs.get(&hd569), None, "the base transducer was not fitted");
}

/// A chain carrying variants of its own cannot be the side that folds:
/// their `position`s index into components that folding replaces, so
/// re-deriving them would be a guess about which box was fitted.
///
/// Refused rather than guessed — and `near_duplicate_chains` says so up
/// front, so the choice is never offered in the first place.
#[tokio::test]
async fn a_chain_with_its_own_variants_is_refused_and_flagged_before_it_is_offered() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let phone = db.upsert_equipment(uid, "Xperia", "dap", None, None, 0.0, None).await.unwrap();
    let hd569 =
        db.upsert_equipment(uid, "HD 569", "headphones", None, None, 0.0, None).await.unwrap();
    let hexa = db.upsert_equipment(uid, "Hexa", "iem", None, None, 0.0, None).await.unwrap();

    let commute =
        chain_of(&db, uid, "Commute Phone", &[("Xperia", phone), ("HD 569", hd569)]).await;
    let summer = chain_of(&db, uid, "Summer Commute", &[("Xperia", phone), ("Hexa", hexa)]).await;

    // The *smaller* chain is the one carrying a variant, so the suggestion
    // would otherwise pick it as the side to fold.
    db.create_variant(uid, summer, "Its own", &[]).await.unwrap();
    hour_on(&db, uid, commute, 1_000).await;

    let err = db
        .fold_chain_into_variant(uid, summer, commute, FoldTarget::New("Hexa"))
        .await
        .unwrap()
        .expect_err("refused");
    assert!(err.contains("variant"), "the message says why: {err}");
    assert_eq!(db.get_chains(uid).await.unwrap().len(), 2, "nothing moved");

    let s = db.near_duplicate_chains(uid).await.unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].distance, 1, "one box differs");
    assert_eq!(s[0].base_chain_id, commute, "the bigger history is the base");
    assert_eq!(s[0].fold_chain_id, summer);
    assert!(s[0].fold_blocked_by_variants, "flagged before it is offered");
}

/// The state that prompted all of this: the variant is built, and the old
/// chain is still sitting in the list beside it. The suggestion has to
/// *recognise* that variant rather than offer to create a second one
/// wired identically.
#[tokio::test]
async fn a_variant_that_already_wires_it_that_way_is_recognised() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let phone = db.upsert_equipment(uid, "Xperia", "dap", None, None, 0.0, None).await.unwrap();
    let hd569 =
        db.upsert_equipment(uid, "HD 569", "headphones", None, None, 0.0, None).await.unwrap();
    let hexa = db.upsert_equipment(uid, "Hexa", "iem", None, None, 0.0, None).await.unwrap();

    let commute =
        chain_of(&db, uid, "Commute Phone", &[("Xperia", phone), ("HD 569", hd569)]).await;
    let summer = chain_of(&db, uid, "Summer Commute", &[("Xperia", phone), ("Hexa", hexa)]).await;
    hour_on(&db, uid, commute, 1_000).await;

    // No variant yet: nothing to recognise.
    let s = db.near_duplicate_chains(uid).await.unwrap();
    assert!(s[0].matching_variant.is_none());
    assert_eq!(s[0].overrides.len(), 1, "what it would become");

    let v = db
        .create_variant(
            uid,
            commute,
            "Summer",
            &[VariantOverride { position: 1, equipment_id: Some(hexa), name: Some("Hexa".into()) }],
        )
        .await
        .unwrap();

    let s = db.near_duplicate_chains(uid).await.unwrap();
    assert_eq!(
        s[0].matching_variant.as_ref().map(|(id, n)| (*id, n.as_str())),
        Some((v, "Summer")),
        "the variant they already built is the one to fold onto"
    );
    assert!(!s[0].fold_blocked_by_variants, "a variant on the *base* does not block");
}

/// Deleting a chain that has a variant used to fail outright with a foreign
/// key violation — `chain_variants.chain_id` is NOT NULL, and `delete_chain`
/// cleared four of the ten tables that reference a chain. So the moment
/// anyone used the feature v0.54.0 shipped, that chain became undeletable,
/// and the error surfaced as a 500 naming nothing.
///
/// The other five references are pinned here too, because each one is the
/// same bug waiting and none of them errors until someone happens to own
/// that combination.
#[tokio::test]
async fn a_chain_can_be_deleted_whatever_still_points_at_it() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let hexa = db.upsert_equipment(uid, "Hexa", "iem", None, None, 0.0, None).await.unwrap();
    let cid = chain_of(&db, uid, "Commute Phone", &[("Hexa", hexa)]).await;

    let v = db.create_variant(uid, cid, "Summer", &[]).await.unwrap();
    hour_on(&db, uid, cid, 1_000).await;
    // A token pointing at it, and a record on the shelf that lives with it.
    db.create_token(uid, "phone", "submit", Some(cid), false).await.unwrap();
    let rid = db
        .create_physical(
            uid,
            "vinyl",
            "Burial",
            "Untrue",
            Some(2007),
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
        .unwrap();
    db.update_physical(uid, rid, None, Some(Some(cid)), None, None, None, None, None, None, None)
        .await
        .unwrap();

    assert!(db.delete_chain(uid, cid).await.unwrap(), "it deletes");
    assert!(db.get_chain(uid, cid).await.unwrap().is_none());
    // The variant went with it — overrides indexed into components that no
    // longer exist are not a degraded row, they are a meaningless one.
    assert!(db.list_variants(uid).await.unwrap().iter().all(|x| x.id != v));
    // The listening survives, having lost only the attribution.
    assert_eq!(
        db.get_recent_scrobbles(uid, 10, 0, None, None, None, None, None, None)
            .await
            .unwrap()
            .len(),
        1
    );
}
/// The reported bug, end to end: a track loved from a Plex listen and then
/// played through a scrobble client, which names its artist differently.
///
/// Plex files a listen under the **record's** artist (`grandparentTitle`)
/// while a client reports what the player displayed, so anything with a
/// feature arrives under two names. The love used to land on one of them and
/// the heart stayed empty on the other, with nothing erroring.
#[tokio::test]
async fn a_love_survives_the_same_song_arriving_from_another_source() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    const MBID: &str = "b1a9c0e9-d987-4042-ae91-78d6a3267d69";

    // Loved from the Plex-shaped listen.
    let plex = db.upsert_entity("recording", "Archangel", "Burial", Some(MBID)).await.unwrap();
    db.set_love(uid, plex, true).await.unwrap();

    // The same recording, as a scrobble client names it. Same MusicBrainz
    // id — which was on both rows all along and never once read back.
    let client = db
        .upsert_entity("recording", "Archangel", "Burial feat. Someone", Some(MBID))
        .await
        .unwrap();
    assert_eq!(client, plex, "one recording is one entity, whatever it was called");
    assert!(db.is_loved(uid, client).await.unwrap(), "the heart is filled from either source");

    // And with no MusicBrainz id anywhere, which is most of a real library:
    // the folded key still recognises it.
    let bare =
        db.upsert_entity("recording", "Archangel", "Burial  feat. Someone", None).await.unwrap();
    assert_eq!(bare, plex);
}

/// The spellings two players disagree about. Every one of these is the same
/// song, and every one of them used to be a separate entity with its own
/// love.
///
/// Built from the two normalisers this codebase already has rather than a
/// third — `normalise_artist` for punctuation, accents and ligatures, and
/// `split_featured` for the credit.
#[tokio::test]
async fn the_ways_two_players_spell_one_song_all_fold_together() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let base =
        db.upsert_entity("recording", "Don't Look Back", "Cœur de pirate", None).await.unwrap();
    db.set_love(uid, base, true).await.unwrap();

    for (artist, title, why) in [
        ("Coeur de pirate", "Don't Look Back", "ligature expanded"),
        ("Cœur de pirate", "Don’t Look Back", "curly apostrophe"),
        ("cœur de pirate", "DON'T LOOK BACK", "case"),
        ("Cœur de pirate feat. Someone", "Don't Look Back", "featured credit"),
        ("  Cœur de pirate  ", " Don't Look Back ", "whitespace"),
    ] {
        let id = db.upsert_entity("recording", title, artist, None).await.unwrap();
        assert_eq!(id, base, "{why}: {artist} — {title}");
        assert!(db.is_loved(uid, id).await.unwrap(), "{why}");
    }
}

/// Folding must not merge things that are genuinely different, or a heart
/// becomes a lie in the other direction.
///
/// A remaster is the deliberate omission: the title is folded but never
/// *shortened*, so a trailing qualifier still separates two recordings —
/// they may really be different, and MusicBrainz ids tell them apart
/// correctly. Merging those is a further decision than this one.
#[tokio::test]
async fn folding_stops_short_of_merging_different_songs() {
    let db = testdb().await;

    let a = db.upsert_entity("recording", "Archangel", "Burial", None).await.unwrap();
    for (artist, title) in [
        ("Burial", "Archangel (Remastered)"),
        ("Burial", "Archangel - Radio Edit"),
        ("Burial", "Arcangel"),
        ("Four Tet", "Archangel"),
        // `&` and `,` are names, not credits — the rule `split_featured`
        // already keeps.
        ("Simon & Garfunkel", "Archangel"),
        ("Simon and Garfunkel", "Archangel"),
    ] {
        assert_ne!(
            db.upsert_entity("recording", title, artist, None).await.unwrap(),
            a,
            "{artist} — {title} is not the same recording"
        );
    }
}

/// A love is published in every form a row could recognise it by, because
/// the client holds this set and matches locally — one request rather than
/// one per listen.
#[tokio::test]
async fn a_love_is_published_in_every_form_a_row_can_match() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    const MBID: &str = "b1a9c0e9-d987-4042-ae91-78d6a3267d69";
    let id = db
        .upsert_entity("recording", "Don't Look Back", "Cœur de pirate", Some(MBID))
        .await
        .unwrap();
    db.set_love(uid, id, true).await.unwrap();

    let keys = db.loved_keys(uid).await.unwrap();
    assert!(keys.contains(&"recording|cœur de pirate|don't look back".to_string()), "{keys:?}");
    assert!(keys.contains(&"recording|coeurdepirate|dontlookback".to_string()), "{keys:?}");
    assert!(keys.contains(&format!("mbid:recording:{MBID}")), "{keys:?}");
}

/// The strings `web/src/lib/loves.ts` has to produce, character for
/// character.
///
/// The client folds a listen row itself — asking the server per row would be
/// one request per listen — so the two implementations have to agree, and if
/// they drift the hearts silently stop filling. This pins the Rust side of
/// that contract so a change here is visible; the mirror is listed in the
/// same order in that file.
#[test]
fn the_folded_key_is_exactly_what_the_client_must_reproduce() {
    for (kind, artist, name, want) in [
        ("recording", "Burial", "Archangel", "recording|burial|archangel"),
        ("recording", "Burial feat. Someone", "Archangel", "recording|burial|archangel"),
        ("recording", "Cœur de pirate", "Don’t Look Back", "recording|coeurdepirate|dontlookback"),
        ("recording", "Beyoncé", "Halo", "recording|beyonce|halo"),
        ("release", "A.R. Rahman", "Roja", "release|arrahman|roja"),
        ("artist", "", "Simon & Garfunkel", "artist||simongarfunkel"),
        ("recording", "Sigur Rós", "Hoppípolla", "recording|sigurros|hoppipolla"),
    ] {
        assert_eq!(Database::match_key(kind, artist, name), want, "{artist} — {name}");
    }
    assert_eq!(
        Database::mbid_key("recording", "B1A9C0E9-D987-4042-AE91-78D6A3267D69"),
        "mbid:recording:b1a9c0e9-d987-4042-ae91-78d6a3267d69"
    );
}

/// An existing database has entities with no `match_key`, and it is the one
/// that needs it most — the split loves this release exists to reunite are
/// already sitting in it.
///
/// The same check `an_existing_database_opens_and_gains_period_notes` exists
/// for. Every other test starts from an empty file and so never runs
/// `migrate()` against a real prior schema, which is precisely how a
/// migration that cannot run on an existing database passes the whole suite
/// (v0.61.1).
#[tokio::test]
async fn an_existing_database_gains_the_forgiving_entity_key() {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-oldent-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }

    // The pre-v0.69 `entities` shape: no `match_key`, and two rows that are
    // the same recording under two spellings — the reported bug, already in
    // the database.
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE entities (id INTEGER PRIMARY KEY AUTOINCREMENT,
                     kind TEXT NOT NULL, name TEXT NOT NULL,
                     artist_name TEXT NOT NULL DEFAULT '',
                     entity_key TEXT NOT NULL UNIQUE, mbid TEXT, image_url TEXT,
                     created_at INTEGER NOT NULL)",
            "INSERT INTO entities (kind, name, artist_name, entity_key, created_at)
                     VALUES ('recording','Archangel','Burial','recording|burial|archangel',1)",
            "INSERT INTO entities (kind, name, artist_name, entity_key, created_at)
                     VALUES ('recording','Archangel','Burial feat. Someone',
                             'recording|burial feat. someone|archangel',1)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    let db = Database::new(&format!("sqlite:{}", path.display()), Arc::new(SecretBox::test_key()))
        .await
        .expect("an existing database must open");

    // Backfilled, not left null — the rows that were already there are the
    // ones the fold has to recognise.
    let keys: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, match_key FROM entities ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(keys.len(), 2);
    for (id, k) in &keys {
        assert_eq!(
            k.as_deref(),
            Some("recording|burial|archangel"),
            "entity {id} folds to the same key"
        );
    }

    // The index is non-unique on purpose: both rows above carry the same
    // folded key, so a unique one would refuse to be created on exactly the
    // databases that need it.
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    db.set_love(uid, 1, true).await.unwrap();

    // A third spelling now resolves onto the *first* row rather than making
    // a fourth, and inherits its love.
    let again =
        db.upsert_entity("recording", "Archangel", "BURIAL  FEAT. SOMEONE", None).await.unwrap();
    assert_eq!(again, 1, "resolved to the earliest of the two, deterministically");
    assert!(db.is_loved(uid, again).await.unwrap());
}
/// A re-import used to throw away everything the incoming copy knew.
///
/// The stored listen is bare — the way a simple scrobble client reports one
/// — and the archive carries a full mapping. Dropping the duplicate whole
/// meant that metadata was lost and the enrichment backfill then bought the
/// same facts back through a rate-limited MusicBrainz search.
#[tokio::test]
async fn a_duplicate_import_fills_in_what_the_stored_listen_never_knew() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    // Bare: no album, no duration, no ids.
    let mut bare = play("Archangel", "Burial", 1_750_000_000);
    assert_eq!(db.import_listen(uid, &bare).await.unwrap(), ImportOutcome::Inserted);

    // The same listen, as an export describes it.
    bare.source_id = "somewhere-else".into();
    let mut rich = bare.clone();
    rich.album = Some("Untrue".into());
    rich.duration = Some(230);
    rich.track_number = Some(2);
    rich.mbid_recording = Some("216ccc30-7b3a-440d-8e6f-1ad304507b87".into());
    rich.mbid_release = Some("4c010fec-e436-4583-a2c8-067b1e15462c".into());
    rich.mbid_artist = Some(vec!["9ddce51c-2b75-4b3e-ac8c-1db09e7c89c6".into()]);
    rich.caa_id = Some(43_788_051_983);
    rich.caa_release_mbid = Some("4c010fec-e436-4583-a2c8-067b1e15462c".into());

    assert_eq!(
        db.import_listen(uid, &rich).await.unwrap(),
        ImportOutcome::Enriched,
        "recognised as already held, and it had something to add"
    );

    let rows =
        db.get_recent_scrobbles(uid, 10, 0, None, None, None, None, None, None).await.unwrap();
    assert_eq!(rows.len(), 1, "still one listen — nothing was duplicated");
    let r = &rows[0];
    assert_eq!(r.album.as_deref(), Some("Untrue"));
    assert_eq!(r.duration, Some(230));
    assert_eq!(r.track_number, Some(2));
    assert_eq!(r.mbid_recording.as_deref(), Some("216ccc30-7b3a-440d-8e6f-1ad304507b87"));
    assert_eq!(r.caa_id, Some(43_788_051_983));

    // Running it a second time has nothing left to do, and must not cost a
    // write per row on a re-import of a whole history.
    assert_eq!(db.import_listen(uid, &rich).await.unwrap(), ImportOutcome::Duplicate);
}

/// **Only ever fills a blank.** An import must not be able to rewrite
/// history you already trust — that is the property which made dropping
/// duplicates safe, and it has to survive them being merged instead.
#[tokio::test]
async fn an_import_can_never_overwrite_what_is_already_there() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let mut held = play("Archangel", "Burial", 1_750_000_000);
    held.album = Some("Untrue".into());
    held.duration = Some(230);
    held.track_number = Some(2);
    held.mbid_recording = Some("the-one-we-already-had".into());
    held.caa_id = Some(111);
    db.import_listen(uid, &held).await.unwrap();

    let mut other = held.clone();
    other.source_id = "elsewhere".into();
    other.album = Some("A Different Album".into());
    other.duration = Some(999);
    other.track_number = Some(9);
    other.mbid_recording = Some("a-different-recording".into());
    other.caa_id = Some(222);

    assert_eq!(
        db.import_listen(uid, &other).await.unwrap(),
        ImportOutcome::Duplicate,
        "nothing to add — every field was already populated"
    );

    let r =
        &db.get_recent_scrobbles(uid, 10, 0, None, None, None, None, None, None).await.unwrap()[0];
    assert_eq!(r.album.as_deref(), Some("Untrue"));
    assert_eq!(r.duration, Some(230));
    assert_eq!(r.track_number, Some(2));
    assert_eq!(r.mbid_recording.as_deref(), Some("the-one-we-already-had"));
    assert_eq!(r.caa_id, Some(111));
}

/// `caa_id = -1` is the "asked the Cover Art Archive, nothing there"
/// tombstone, so it is a blank rather than an answer — an import carrying a
/// real id is exactly what it was standing in for. Getting this wrong is
/// invisible: the row keeps its lettered tile forever.
#[tokio::test]
async fn a_cover_art_tombstone_counts_as_blank() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let mut held = play("Archangel", "Burial", 1_750_000_000);
    held.caa_id = Some(-1);
    db.import_listen(uid, &held).await.unwrap();

    let mut rich = held.clone();
    rich.source_id = "elsewhere".into();
    rich.caa_id = Some(43_788_051_983);
    assert_eq!(db.import_listen(uid, &rich).await.unwrap(), ImportOutcome::Enriched);

    let r =
        &db.get_recent_scrobbles(uid, 10, 0, None, None, None, None, None, None).await.unwrap()[0];
    assert_eq!(r.caa_id, Some(43_788_051_983), "a real id supersedes the tombstone");
}

/// An import that knows nothing extra must cost no read and no write.
///
/// A Rockbox log carries no MBIDs at all, so on a re-import of one this is
/// every single row — and a query per row is the shape that made a session
/// rebuild time out on the real host.
#[tokio::test]
async fn a_duplicate_with_nothing_to_add_is_still_just_a_duplicate() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();

    let mut p = play("Archangel", "Burial", 1_750_000_000);
    db.import_listen(uid, &p).await.unwrap();
    p.source_id = "elsewhere".into();
    assert_eq!(db.import_listen(uid, &p).await.unwrap(), ImportOutcome::Duplicate);
    assert_eq!(
        db.get_recent_scrobbles(uid, 10, 0, None, None, None, None, None, None)
            .await
            .unwrap()
            .len(),
        1
    );
}
/// The session list still reports which variant was fitted, after the
/// query that answers it was rewritten.
///
/// It used to run a correlated subquery per session row — and the *name*
/// re-nested the id lookup inside itself, so one question cost two full
/// scans of the whole history per row. With no index on
/// `scrobbles.session_id` that was a measured **630 seconds** on the real
/// host, holding one of five pool connections for ten minutes while every
/// other request queued.
///
/// A wrong join here returns NULL rather than erroring, so the session card
/// would simply stop saying which IEMs were in. Hence asserting on the
/// value rather than on the plan.
#[tokio::test]
async fn a_session_still_reports_the_variant_that_was_fitted() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let gear = db.upsert_equipment(uid, "Hexa", "iem", None, None, 0.0, None).await.unwrap();
    let cid = chain_of(&db, uid, "Commute", &[("Xperia", gear)]).await;
    let vid = db
        .create_variant(
            uid,
            cid,
            "Summer",
            &[VariantOverride { position: 0, equipment_id: Some(gear), name: Some("Hexa".into()) }],
        )
        .await
        .unwrap();

    for i in 0..3u64 {
        let mut p = play(&format!("t{i}"), "Burial", 1_750_000_000 + i * 300);
        p.duration = Some(200);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            Some(cid),
            Some(vid),
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }
    db.rebuild_sessions(uid, 1800).await.unwrap();

    let listed = db.list_sessions(uid, 20, 0).await.unwrap();
    assert_eq!(listed.len(), 1, "one sitting");
    assert_eq!(listed[0].chain_variant_id, Some(vid));
    assert_eq!(
        listed[0].chain_variant_name.as_deref(),
        Some("Summer"),
        "the name comes back, not just the id"
    );

    // The single-session query answers the same way — it takes a different
    // shape (a seek rather than a grouped pass) and could disagree.
    let one = db.get_session(uid, listed[0].id).await.unwrap().expect("the session");
    assert_eq!(one.chain_variant_id, Some(vid));
    assert_eq!(one.chain_variant_name.as_deref(), Some("Summer"));

    // A sitting with no variant reports none rather than borrowing another
    // session's — the failure a badly-scoped join would produce.
    let uid2 = db.create_user("bo", None, "pw", "user").await.unwrap();
    let mut p = play("solo", "Burial", 1_750_100_000);
    p.duration = Some(200);
    db.save_scrobble(uid2, &p, None, None, None, None, None, None, None, ListenDisposition::LIVE)
        .await
        .unwrap();
    db.rebuild_sessions(uid2, 1800).await.unwrap();
    let other = db.list_sessions(uid2, 20, 0).await.unwrap();
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].chain_variant_id, None, "no variant, and not someone else's");
    assert_eq!(other[0].chain_variant_name, None);
}
/// A gear total has to be readable as the addition it is.
///
/// Reported as "I think the total hours added up for using the Hexa with my
/// Xperia, but there is no way of knowing it" — the number was right and
/// unverifiable, which for derived arithmetic is most of the way to being
/// useless. The breakdown is the same walk `equipment_seconds` already did
/// and threw away.
///
/// Asserts the parts sum to the whole, because a breakdown that disagrees
/// with the figure above it is worse than none.
#[tokio::test]
async fn a_gear_total_breaks_down_into_the_chains_that_earned_it() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    let phone = db.upsert_equipment(uid, "Xperia", "dap", None, None, 0.0, None).await.unwrap();
    let hd = db.upsert_equipment(uid, "HD 569", "headphones", None, None, 0.0, None).await.unwrap();
    let hexa = db.upsert_equipment(uid, "Hexa", "iem", None, None, 0.0, None).await.unwrap();

    // One chain with a Hexa variant, and a second chain naming the Hexa
    // outright — so the Hexa earns hours two different ways, which is
    // exactly when a bare total stops being checkable.
    let commute = chain_of(&db, uid, "Commute Phone", &[("Xperia", phone), ("HD 569", hd)]).await;
    let summer = chain_of(&db, uid, "Summer Commute", &[("Xperia", phone), ("Hexa", hexa)]).await;
    let v = db
        .create_variant(
            uid,
            commute,
            "Summer",
            &[VariantOverride { position: 1, equipment_id: Some(hexa), name: Some("Hexa".into()) }],
        )
        .await
        .unwrap();

    // 2h base chain, 1h through its variant, 3h through the other chain.
    for (i, (chain, variant)) in [
        (commute, None),
        (commute, None),
        (commute, Some(v)),
        (summer, None),
        (summer, None),
        (summer, None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut p = play(&format!("t{i}"), "Burial", 1_750_000_000 + i as u64 * 4000);
        p.duration = Some(3600);
        db.save_scrobble(
            uid,
            &p,
            None,
            None,
            Some(chain),
            variant,
            None,
            None,
            None,
            ListenDisposition::LIVE,
        )
        .await
        .unwrap();
    }

    let gear = db.get_equipment(uid).await.unwrap();
    let find = |id: i64| gear.iter().find(|g| g.id == id).unwrap();

    // The Hexa: one hour through the variant, three through its own chain.
    let h = find(hexa);
    assert!((h.total_hours - 4.0).abs() < 1e-6, "{}", h.total_hours);
    let summed: f64 = h.sources.iter().map(|s| s.hours).sum();
    assert!((summed - h.total_hours).abs() < 1e-6, "the parts add to the whole");
    assert_eq!(h.sources.len(), 2, "two chains earned it");
    assert_eq!(h.sources[0].chain_name, "Summer Commute", "largest first");
    assert!((h.sources[0].hours - 3.0).abs() < 1e-6);
    assert_eq!(h.sources[0].variant_name, None, "its own chain names it directly");
    assert_eq!(h.sources[1].chain_name, "Commute Phone");
    assert_eq!(
        h.sources[1].variant_name.as_deref(),
        Some("Summer"),
        "and it says which setup, or the row cannot be checked against the chain"
    );

    // The HD 569 accrues nothing while the Hexa is fitted — the whole point
    // of a variant — so its breakdown shows only the base chain.
    let d = find(hd);
    assert!((d.total_hours - 2.0).abs() < 1e-6, "{}", d.total_hours);
    assert_eq!(d.sources.len(), 1);
    assert_eq!(d.sources[0].variant_name, None);

    // The phone was in the path throughout, from both chains.
    let x = find(phone);
    assert!((x.total_hours - 6.0).abs() < 1e-6, "{}", x.total_hours);
    let summed: f64 = x.sources.iter().map(|s| s.hours).sum();
    assert!((summed - x.total_hours).abs() < 1e-6);

    // Gear nothing has logged through says so with an empty list rather
    // than a missing field.
    let spare = db.upsert_equipment(uid, "Spare", "dac", None, None, 0.0, None).await.unwrap();
    let gear = db.get_equipment(uid).await.unwrap();
    assert!(gear.iter().find(|g| g.id == spare).unwrap().sources.is_empty());
}

/// An existing database gains the stable ids a replica reconciles on — and so
/// does every row written afterwards.
///
/// The second half is the one that matters. The columns are easy; what fails
/// silently is an `INSERT` site that was not updated to bind one, because the
/// column is nullable and nothing errors — the row is simply invisible to any
/// future reconciliation, and it stays that way. So this asserts a *new* row of
/// every kind carries an id, not just a migrated one.
///
/// Same shape and the same reason as `an_existing_database_gains_the_gear_status_column`:
/// every other test starts from an empty file, takes the `init()` path and never
/// exercises `migrate()` against a real prior schema. That is the v0.61.1 blind
/// spot — 297 restarts in 26 minutes while the suite stayed green.
#[tokio::test]
async fn an_existing_database_gains_stable_ids_and_keeps_minting_them() {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-olduuid-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }

    // The pre-uuid shape, with a row in each table that is getting one.
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, username TEXT NOT NULL,
                 password_hash TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'user',
                 created_at INTEGER NOT NULL DEFAULT 0, settings TEXT)",
            "CREATE TABLE equipment (id INTEGER PRIMARY KEY AUTOINCREMENT,
                 user_id INTEGER NOT NULL REFERENCES users(id),
                 name TEXT NOT NULL, type TEXT NOT NULL, brand TEXT, model TEXT,
                 total_hours REAL DEFAULT 0, first_used INTEGER, last_used INTEGER,
                 notes TEXT, purchased_at INTEGER, UNIQUE(user_id, name))",
            "CREATE TABLE signal_chains (id INTEGER PRIMARY KEY AUTOINCREMENT,
                 user_id INTEGER NOT NULL REFERENCES users(id),
                 name TEXT NOT NULL, description TEXT,
                 components TEXT NOT NULL DEFAULT '[]',
                 listening_context TEXT NOT NULL DEFAULT 'unknown', icon TEXT,
                 is_active BOOLEAN DEFAULT TRUE, total_hours REAL DEFAULT 0,
                 created_at INTEGER NOT NULL, UNIQUE(user_id, name))",
            "INSERT INTO users (id, username, password_hash) VALUES (1, 'ashwin', 'x')",
            "INSERT INTO equipment (user_id, name, type, brand, notes)
                 VALUES (1, 'HD 650', 'headphones', 'Sennheiser', 'the good pads')",
            "INSERT INTO signal_chains (user_id, name, created_at)
                 VALUES (1, 'Desk', 0)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    let url = format!("sqlite:{}", path.display());
    let db = Database::new(&url, Arc::new(SecretBox::test_key()))
        .await
        .expect("an existing database must open");

    /// Every row's id, in insertion order, with NULLs kept so a gap is visible.
    async fn ids(db: &Database, table: &str) -> Vec<Option<String>> {
        sqlx::query_scalar::<_, Option<String>>(&format!("SELECT uuid FROM {table} ORDER BY id"))
            .fetch_all(&db.pool)
            .await
            .unwrap()
    }

    // The migration half: the rows that predate the column have one now.
    for table in ["equipment", "signal_chains"] {
        let got = ids(&db, table).await;
        assert_eq!(got.len(), 1, "{table}: the row survived the migration");
        assert!(got[0].is_some(), "{table}: a row predating the column was backfilled");
    }
    // And the data came with it, rather than a fresh table wearing the name.
    let gear = db.get_equipment(1).await.unwrap();
    assert_eq!(gear[0].name, "HD 650");
    assert_eq!(gear[0].notes.as_deref(), Some("the good pads"));

    // The half that fails silently: a row written *after* the migration.
    db.upsert_equipment(1, "Bifrost", "dac", Some("Schiit"), None, 0.0, None).await.unwrap();
    db.create_chain(&SignalChain {
        id: None,
        user_id: 1,
        name: "Vinyl".into(),
        description: None,
        components: vec![],
        listening_context: ListeningContext::Active,
        icon: None,
        is_active: true,
        shared: false,
        total_hours: 0.0,
        created_at: 0,
    })
    .await
    .unwrap();

    for table in ["equipment", "signal_chains"] {
        let got = ids(&db, table).await;
        assert_eq!(got.len(), 2, "{table}: the new row landed");
        assert!(
            got.iter().all(|u| u.is_some()),
            "{table}: an INSERT site is not binding an id — the column is nullable, so \
             nothing errors and the row is simply unreconcilable for ever"
        );
        assert_ne!(got[0], got[1], "{table}: two rows must not share an id");
    }

    // Opening runs on every start, so being a no-op the second time is the
    // property that matters — a backfill that re-ran would mint new ids and
    // orphan whatever already referred to the old ones.
    let before = ids(&db, "equipment").await;
    drop(db);
    let db = Database::new(&url, Arc::new(SecretBox::test_key()))
        .await
        .expect("reopening must be idempotent");
    assert_eq!(before, ids(&db, "equipment").await, "a reopen must not re-mint ids");
}

/// The timeline is one row per **play**, not one per listen.
///
/// A joint listen is N rows sharing a `joint_session_id` — one per person in
/// the room — so the naive read reports an evening at twice its real length the
/// moment somebody accepts their half of it. This asserts on the *timeline*
/// rather than on the query, because that doubling is not an error anywhere: it
/// is a page confidently describing a two-record evening as four records.
#[tokio::test]
async fn a_sitting_is_one_row_per_play_however_many_decks_were_in_it() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    let spool = db.start_spool(them, Some("Sunday"), &[me], &[]).await.unwrap();
    assert_eq!(
        db.set_spool_membership(&spool, me, true).await.unwrap(),
        Some(("joined".into(), 0))
    );

    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_000).await;
    listen(&db, them, Some(chain), "Angel", "Massive Attack", 1_760_000_400).await;

    let before = db.spool_detail(&spool, me).await.unwrap().unwrap();
    assert_eq!(before.plays.len(), 2, "two records played, so two rows");
    assert!(
        before.plays.iter().all(|p| p.standing == "pending"),
        "they are offers until they are answered"
    );

    // Accepting writes a real listen of mine carrying the same sitting id. The
    // evening did not get longer.
    let ids: Vec<i64> = before.plays.iter().filter_map(|p| p.capture_id).collect();
    assert_eq!(ids.len(), 2);
    db.settle_captures(me, &ids, true).await.unwrap();

    let after = db.spool_detail(&spool, me).await.unwrap().unwrap();
    assert_eq!(
        after.plays.len(),
        2,
        "accepting a play must not put a second copy of it on the timeline — a joint \
         listen is N rows by design, and counting all of them reports the evening at \
         twice the length it was"
    );
    assert!(after.plays.iter().all(|p| p.standing == "accepted"));

    // And the history list counts it the same way.
    let hist = db.spool_history(me, 50).await.unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].plays, 2);
    assert_eq!(hist[0].pending, 0);
    assert!(hist[0].present.iter().any(|n| n == "kim"), "the other deck was in the room");
    assert!(
        !hist[0].present.iter().any(|n| n == "ash"),
        "and the summary leaves the reader out — it exists to write 'with Kim'"
    );
}

/// A sitting shows what the sharing switch allows and **says how much it is not
/// showing**.
///
/// The withheld count is the half that matters. A page with invisible holes
/// misrepresents the evening and the reader could never tell — the rule
/// `handed_sitting` already carries, in the one other place this application
/// shows somebody an evening.
#[tokio::test]
async fn a_sitting_says_what_it_is_not_showing() {
    let db = testdb().await;
    let (me, them, shared) = patched_pair(&db).await;
    let private = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Private', ?) RETURNING id",
    )
    .bind(them)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];

    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();

    listen(&db, them, Some(shared), "Teardrop", "Massive Attack", 1_760_000_000).await;
    listen(&db, them, Some(private), "Karmacoma", "Massive Attack", 1_760_000_400).await;

    let view = db.spool_detail(&spool, me).await.unwrap().unwrap();
    let titles: Vec<&str> = view.plays.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["Teardrop"],
        "a play through an unshared chain does not reach somebody else's screen"
    );
    assert_eq!(view.withheld, 1, "and the evening says so rather than quietly omitting it");
}

/// A sitting stays visible after the patch that made it possible is gone.
///
/// The decision, and it is a deliberate widening rather than an inherited rule:
/// you were in the room, and un-patching afterwards is a choice about what you
/// want to keep seeing *of theirs*. It has no standing to un-happen an evening
/// you attended — and your own accepted listens from it stay in your history
/// regardless, so a vanished sitting would leave rows with nothing to explain
/// them.
#[tokio::test]
async fn un_patching_does_not_un_happen_an_evening_you_were_in() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    let spool = db.start_spool(them, Some("Before"), &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_000).await;
    assert_eq!(db.spool_history(me, 50).await.unwrap().len(), 1);

    db.unpatch(me, them).await.unwrap();

    let hist = db.spool_history(me, 50).await.unwrap();
    assert_eq!(hist.len(), 1, "the evening happened whatever the patch says now");
    assert!(db.spool_detail(&spool, me).await.unwrap().is_some());

    // The Reel is the thing the patch does gate, and it is untouched by any of
    // this — which is what makes the widening narrow.
    assert!(
        db.reel(me, 50, None, None, None).await.unwrap().is_empty(),
        "un-patching still stops their listening reaching your Reel"
    );
}

/// Saying no to an invitation and going home from a sitting are different
/// answers, and until they were told apart neither could be recorded.
///
/// Both used to write `left`, so the history could not say who had actually
/// been in a sitting and `present_until` had nothing to hang on.
#[tokio::test]
async fn declining_an_invitation_is_not_leaving_a_sitting() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    let declined = db.start_spool(them, Some("Not this one"), &[me], &[]).await.unwrap();
    assert_eq!(
        db.set_spool_membership(&declined, me, false).await.unwrap(),
        Some(("declined".into(), 0))
    );
    db.end_spool(&declined, them).await.unwrap();

    let attended = db.start_spool(them, Some("This one"), &[me], &[]).await.unwrap();
    db.set_spool_membership(&attended, me, true).await.unwrap();
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_000).await;
    assert_eq!(
        db.set_spool_membership(&attended, me, false).await.unwrap(),
        Some(("left".into(), 0)),
        "leaving a sitting you were in is `left`"
    );

    let hist = db.spool_history(me, 50).await.unwrap();
    let titles: Vec<&str> = hist.iter().filter_map(|s| s.title.as_deref()).collect();
    assert_eq!(titles, vec!["This one"], "an evening you said no to is not an evening you were at");

    // Leaving has a time on it. Declining does not — there is no moment you
    // stopped being in a room you were never in.
    let left_at: Option<i64> = sqlx::query_scalar(
        "SELECT present_until FROM spool_members m JOIN spools sp ON sp.id = m.spool_id
          WHERE sp.uuid = ? AND m.user_id = ?",
    )
    .bind(&attended)
    .bind(me)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(left_at.is_some());
    let never: Option<i64> = sqlx::query_scalar(
        "SELECT present_until FROM spool_members m JOIN spools sp ON sp.id = m.spool_id
          WHERE sp.uuid = ? AND m.user_id = ?",
    )
    .bind(&declined)
    .bind(me)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(never.is_none());
}

/// Somebody added to a sitting after it started gets the part of the evening
/// they were actually in the room for.
///
/// **Absent `present_from` is the whole evening**, which is the common case: a
/// host adding somebody an hour in has almost always just remembered the person
/// sitting in front of them. A real mid-evening arrival is what the column is
/// for, and it is said rather than inferred from a join time that means
/// something else entirely.
#[tokio::test]
async fn somebody_added_late_gets_the_evening_they_were_there_for() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[], &[]).await.unwrap();

    listen(&db, them, Some(chain), "Early", "Massive Attack", 1_760_000_000).await;
    listen(&db, them, Some(chain), "Late", "Massive Attack", 1_760_004_000).await;

    // Arrived between the two.
    assert_eq!(db.add_spool_members(&spool, them, &[me], Some(1_760_002_000)).await.unwrap(), 1);
    assert_eq!(
        db.set_spool_membership(&spool, me, true).await.unwrap(),
        Some(("joined".into(), 1)),
        "only the play they were in the room for"
    );
    let waiting: Vec<String> =
        db.spool_captures(me).await.unwrap().into_iter().map(|c| c.title).collect();
    assert_eq!(waiting, vec!["Late".to_string()]);

    // Correcting it to "I was here all along" offers the rest, and offers
    // nothing twice.
    assert_eq!(
        db.set_member_presence(&spool, me, Some(None), None).await.unwrap(),
        Some(1),
        "widening presence offers the plays you have just said you heard"
    );
    let mut waiting: Vec<String> =
        db.spool_captures(me).await.unwrap().into_iter().map(|c| c.title).collect();
    waiting.sort();
    assert_eq!(waiting, vec!["Early".to_string(), "Late".to_string()]);
}

/// Adding somebody to a sitting carries the consent guard `start_spool` has,
/// and one stranger refuses the whole list rather than being dropped from it.
#[tokio::test]
async fn a_stranger_cannot_be_added_to_a_sitting_either() {
    let db = testdb().await;
    let (me, them, _chain) = patched_pair(&db).await;
    let stranger = db.create_user("noor", None, "pw", "user").await.unwrap();
    let spool = db.start_spool(them, None, &[], &[]).await.unwrap();

    assert!(db.add_spool_members(&spool, them, &[stranger], None).await.is_err());
    assert!(
        db.add_spool_members(&spool, them, &[me, stranger], None).await.is_err(),
        "a mixed list is refused whole"
    );
    let members: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM spool_members m JOIN spools sp ON sp.id = m.spool_id
          WHERE sp.uuid = ?",
    )
    .bind(&spool)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(members, 1, "nothing is written on the way to refusing — only the host");

    // And a guest cannot add anybody at all: it is the host's sitting.
    assert_eq!(db.add_spool_members(&spool, me, &[them], None).await.unwrap(), -1);
}

/// A play accepted out of one sitting is not a play in another.
///
/// Naming a spool chain is what makes this reachable: with a chain set, an
/// accepted capture passes the chain gate, so without the guard, answering last
/// week's inbox while a sitting happens to be running would fan those plays out
/// into tonight's room and stamp them with tonight's uuid.
#[tokio::test]
async fn answering_an_old_inbox_does_not_publish_into_tonights_room() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;

    // My own shared chain, and I file rooms I sit in under it.
    let mine = sqlx::query_scalar::<_, i64>(
        "INSERT INTO signal_chains (user_id, name, created_at) VALUES (?, 'Sofa', ?) RETURNING id",
    )
    .bind(me)
    .bind(Database::now_secs())
    .fetch_all(&db.pool)
    .await
    .unwrap()[0];
    db.set_chain_shared(me, mine, true).await.unwrap();
    assert!(db.set_spool_chain(me, Some(mine)).await.unwrap());

    // **Forwarding on, and a play from this week.** Without both, accepting
    // files the listen as `imported` and the fan-out is never reached at all —
    // so this would be asserting an empty inbox that was empty for a reason
    // having nothing to do with the guard. It was written that way first, and
    // breaking the guard did not fail it.
    db.set_forward_spool_listens(me, true).await.unwrap();
    let recent = Database::now_secs() as u64 - 3600;

    let last_week = db.start_spool(them, Some("Last week"), &[me], &[]).await.unwrap();
    db.set_spool_membership(&last_week, me, true).await.unwrap();
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", recent).await;
    let waiting: Vec<i64> =
        db.spool_captures(me).await.unwrap().into_iter().map(|c| c.id).collect();
    assert_eq!(waiting.len(), 1);
    db.end_spool(&last_week, them).await.unwrap();

    // Tonight, I am hosting.
    let tonight = db.start_spool(me, Some("Tonight"), &[them], &[]).await.unwrap();
    db.set_spool_membership(&tonight, them, true).await.unwrap();

    let (settled, forwarded) = db.settle_captures(me, &waiting, true).await.unwrap();
    assert_eq!((settled, forwarded), (1, 1), "it relayed, so it was not filed as imported");

    assert_eq!(
        db.spool_captures(them).await.unwrap().len(),
        0,
        "answering an old inbox must not put last week's plays in tonight's room"
    );
    let stamped: Option<String> = sqlx::query_scalar(
        "SELECT joint_session_id FROM scrobbles WHERE user_id = ? AND source_name = 'Shared Spool'",
    )
    .bind(me)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        stamped.as_deref(),
        Some(last_week.as_str()),
        "it belongs to the evening it was played in"
    );

    // And the chain really did land, which is the whole point of naming one.
    let filed: Option<i64> = sqlx::query_scalar(
        "SELECT chain_id FROM scrobbles WHERE user_id = ? AND source_name = 'Shared Spool'",
    )
    .bind(me)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(filed, Some(mine), "an accepted play is filed under the room you named");
}

/// A chain named for accepted captures has to be one of yours.
#[tokio::test]
async fn the_room_you_name_has_to_be_a_chain_of_yours() {
    let db = testdb().await;
    let (me, _them, theirs) = patched_pair(&db).await;
    assert!(!db.set_spool_chain(me, Some(theirs)).await.unwrap());
    assert_eq!(db.spool_chain_id(me).await.unwrap(), None);
}

/// Moving a listen keeps what the clock said, and says what it could not reach.
#[tokio::test]
async fn a_time_edit_keeps_the_original_and_states_its_limits() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    listen(&db, uid, None, "Teardrop", "Massive Attack", 1_760_000_000).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM scrobbles WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE scrobbles SET status = 'synced' WHERE id = ?")
        .bind(id)
        .execute(&db.pool)
        .await
        .unwrap();

    let edit = db.shift_listen_times(uid, &[id], -3600).await.unwrap();
    assert_eq!(edit.moved, 1);
    assert_eq!(
        edit.already_forwarded, 1,
        "neither Last.fm nor ListenBrainz has an edit API, so a screen that could not \
         say this would let somebody believe they had corrected a public record"
    );

    let (now, was): (i64, Option<i64>) =
        sqlx::query_as("SELECT timestamp, timestamp_original FROM scrobbles WHERE id = ?")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(now, 1_760_000_000 - 3600);
    assert_eq!(was, Some(1_760_000_000), "what the clock said is kept");

    // A second correction does not overwrite the first one's record. What is
    // worth keeping is the observation, not the previous guess at it.
    db.shift_listen_times(uid, &[id], 60).await.unwrap();
    let was: Option<i64> =
        sqlx::query_scalar("SELECT timestamp_original FROM scrobbles WHERE id = ?")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(was, Some(1_760_000_000));
}

/// A listen is never moved on top of another of the same track.
///
/// Dedup runs at ingest and never again, so a duplicate created by a hand edit
/// is one nothing in the application will ever reconcile — `fuzzy_exists` has
/// already had its chance.
#[tokio::test]
async fn a_time_edit_never_creates_a_duplicate_nothing_will_reconcile() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    listen(&db, uid, None, "Teardrop", "Massive Attack", 1_760_000_000).await;
    listen(&db, uid, None, "Teardrop", "Massive Attack", 1_760_003_600).await;
    listen(&db, uid, None, "Angel", "Massive Attack", 1_760_007_200).await;

    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM scrobbles WHERE user_id = ? ORDER BY timestamp")
            .bind(uid)
            .fetch_all(&db.pool)
            .await
            .unwrap();

    // Move the second Teardrop back onto the first.
    let edit = db.shift_listen_times(uid, &[ids[1]], -3600).await.unwrap();
    assert_eq!(edit.moved, 0, "it would have landed on the other play of the same track");
    let still: i64 = sqlx::query_scalar("SELECT timestamp FROM scrobbles WHERE id = ?")
        .bind(ids[1])
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(still, 1_760_003_600);

    // A different track over the same ground is fine — it is not a duplicate of
    // anything.
    let edit = db.shift_listen_times(uid, &[ids[2]], -7200).await.unwrap();
    assert_eq!(edit.moved, 1);
}

/// Moving a listen out of its session reports that the *grouping* is stale
/// rather than silently regrouping.
///
/// Sessions are stored rather than computed, which is what lets a manual merge
/// survive — and `sessions.uuid` is what a handed-over sitting points at, so
/// regrouping on a metadata edit would move an identity somebody else holds.
#[tokio::test]
async fn a_time_edit_reports_a_stale_grouping_rather_than_regrouping() {
    let db = testdb().await;
    let uid = db.create_user("ash", None, "pw", "user").await.unwrap();
    for (i, ts) in [1_760_000_000u64, 1_760_000_300, 1_760_000_600].iter().enumerate() {
        listen(&db, uid, None, &format!("T{i}"), "Massive Attack", *ts).await;
    }
    db.rebuild_sessions(uid, crate::db::SESSION_GAP_SECS).await.unwrap();

    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM scrobbles WHERE user_id = ? ORDER BY timestamp")
            .bind(uid)
            .fetch_all(&db.pool)
            .await
            .unwrap();
    let before: Option<String> = sqlx::query_scalar("SELECT uuid FROM sessions WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    // Nudging one listen a minute leaves the run contiguous.
    let edit = db.shift_listen_times(uid, &[ids[2]], 60).await.unwrap();
    assert_eq!(edit.sessions_touched, 1);
    assert!(!edit.grouping_stale);

    // Throwing it a day forward does not.
    let edit = db.shift_listen_times(uid, &[ids[2]], 86_400).await.unwrap();
    assert!(
        edit.grouping_stale,
        "the stored grouping no longer describes this run, and only a rebuild the \
         user asks for may change it"
    );
    let after: Option<String> = sqlx::query_scalar("SELECT uuid FROM sessions WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(before, after, "nothing regrouped, so no identity moved under anybody");
}

/// A sitting can be named and written about afterwards, and the people with no
/// account here are named too.
#[tokio::test]
async fn a_sitting_can_be_named_for_what_it_turned_out_to_be() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &["Tobias".into(), "  ".into()]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();

    let view = db.spool_detail(&spool, me).await.unwrap().unwrap();
    assert_eq!(view.guests, vec!["Tobias".to_string()], "a blank is not a person");
    assert_eq!(view.title, None);

    assert!(db
        .update_spool(&spool, them, Some(Some("The long one")), Some(Some("Ran to 3am.")), None)
        .await
        .unwrap());
    let view = db.spool_detail(&spool, me).await.unwrap().unwrap();
    assert_eq!(view.title.as_deref(), Some("The long one"));
    assert_eq!(view.notes.as_deref(), Some("Ran to 3am."));

    // Not the guests' — only the host names a sitting.
    assert!(!db.update_spool(&spool, me, Some(Some("Mine now")), None, None).await.unwrap());

    // A guest with no account is company like anybody else.
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_000).await;
    let company: Option<String> = sqlx::query_scalar(
        "SELECT company FROM scrobbles WHERE user_id = ? AND title = 'Teardrop'",
    )
    .bind(them)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let company = company.unwrap();
    assert!(company.contains("Tobias"), "got {company}");
    assert!(company.contains("ash"), "got {company}");
}

/// Correcting a capture's time is the cheapest fix in the application, and it
/// is only ever available while it is still an offer.
#[tokio::test]
async fn a_capture_can_be_re_timed_while_it_is_still_an_offer() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_000).await;

    let cap = db.spool_captures(me).await.unwrap().remove(0);
    assert_eq!(cap.played_at, 1_760_000_000);
    assert!(db.set_capture_time(me, cap.id, 1_760_003_600).await.unwrap());

    // And it is the corrected time that becomes the listen.
    db.settle_captures(me, &[cap.id], true).await.unwrap();
    let ts: i64 = sqlx::query_scalar(
        "SELECT timestamp FROM scrobbles WHERE user_id = ? AND source_name = 'Shared Spool'",
    )
    .bind(me)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(ts, 1_760_003_600);

    // Settled is past re-timing: it is a listen now, and `shift_listen_times`
    // is what moves one of those.
    assert!(!db.set_capture_time(me, cap.id, 1_760_007_200).await.unwrap());
}

/// An existing database gains a sittings history, and keeps writing one.
///
/// The half that fails silently is the second: every other test in this file
/// starts from `Database::new` on an empty file, which takes the `init()` path
/// and never exercises `migrate()` against a real prior schema — so a migration
/// that cannot run on an existing database passes the whole suite. v0.61.1 is
/// what that costs.
#[tokio::test]
async fn an_existing_database_gains_a_sittings_history() {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("td-oldspool-{}-{}.db", std::process::id(), n));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
    }

    // The pre-history shape: spools with no notes and no guests, members with
    // no presence, and a member row already in it.
    {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path.display()))
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, username TEXT NOT NULL,
                 password_hash TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'user',
                 created_at INTEGER NOT NULL DEFAULT 0, settings TEXT, display_name TEXT)",
            "CREATE TABLE spools (id INTEGER PRIMARY KEY AUTOINCREMENT, uuid TEXT NOT NULL UNIQUE,
                 host_user_id INTEGER NOT NULL, title TEXT, started_at INTEGER NOT NULL,
                 ended_at INTEGER, session_uuid TEXT)",
            "CREATE TABLE spool_members (spool_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
                 state TEXT NOT NULL DEFAULT 'invited', joined_at INTEGER,
                 PRIMARY KEY (spool_id, user_id))",
            "INSERT INTO users (id, username, password_hash) VALUES (1, 'ashwin', 'x')",
            "INSERT INTO spools (uuid, host_user_id, title, started_at, ended_at)
                 VALUES ('old', 1, 'An old evening', 100, 200)",
            "INSERT INTO spool_members (spool_id, user_id, state, joined_at)
                 VALUES (1, 1, 'joined', 100)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    let url = format!("sqlite:{}", path.display());
    let db = Database::new(&url, Arc::new(SecretBox::test_key()))
        .await
        .expect("an existing database must open");

    // The migration half: the evening survived, and it has a page now.
    let hist = db.spool_history(1, 50).await.unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].title.as_deref(), Some("An old evening"));
    assert_eq!(hist[0].notes, None);

    // **No backfill of `present_from`, and that is the correct reading rather
    // than a saving.** NULL means the whole sitting, which is exactly what
    // every member of every existing spool was offered — writing `joined_at`
    // into it would retroactively cut people out of plays they had accepted.
    let view = db.spool_detail("old", 1).await.unwrap().unwrap();
    assert_eq!(view.members.len(), 1);
    assert_eq!(view.members[0].present_from, None);

    // The half that fails silently: a sitting written *after* the migration.
    let them = db.create_user("kim", None, "pw", "user").await.unwrap();
    // Adding somebody carries the consent guard, so the fixture has to be a
    // real patch rather than two accounts that merely exist.
    let patch = db.request_patch(them, 1).await.unwrap();
    db.settle_patch(&patch, 1, true).await.unwrap();
    let uuid = db.start_spool(1, Some("A new one"), &[], &["Tobias".into()]).await.unwrap();
    assert!(db.update_spool(&uuid, 1, None, Some(Some("wrote about it")), None).await.unwrap());
    assert_eq!(db.add_spool_members(&uuid, 1, &[them], Some(500)).await.unwrap(), 1);

    let view = db.spool_detail(&uuid, 1).await.unwrap().unwrap();
    assert_eq!(view.notes.as_deref(), Some("wrote about it"));
    assert_eq!(view.guests, vec!["Tobias".to_string()]);
    let added = view.members.iter().find(|m| m.user_id == them).unwrap();
    assert_eq!(added.present_from, Some(500));
}

/// A capture re-timed still belongs to the play it came from.
///
/// The timeline matched captures to plays on `(played_at, title)`, and
/// `set_capture_time` exists precisely so a host's wrong clock can be
/// corrected — so the moment anybody used it, the offer stopped matching its
/// own play and the row read "not offered" while a Keep button should have
/// been on it. Found by re-timing one on a running instance, which is the only
/// place it could have been found.
#[tokio::test]
async fn re_timing_an_offer_does_not_orphan_it_from_its_play() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_000).await;

    let cap = db.spool_captures(me).await.unwrap().remove(0);
    assert!(db.set_capture_time(me, cap.id, 1_760_003_600).await.unwrap());

    let view = db.spool_detail(&spool, me).await.unwrap().unwrap();
    assert_eq!(view.plays.len(), 1);
    assert_eq!(
        view.plays[0].standing, "pending",
        "the offer is still yours to answer — correcting its clock said nothing about that"
    );
    assert_eq!(view.plays[0].capture_id, Some(cap.id));
}

/// Two plays of one track in an evening take one offer each.
///
/// Nearest-time-wins is what separates them, and claiming each capture once is
/// what stops the second play taking the first's.
#[tokio::test]
async fn a_track_played_twice_gets_an_offer_each() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_000).await;
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_007_200).await;

    let view = db.spool_detail(&spool, me).await.unwrap().unwrap();
    assert_eq!(view.plays.len(), 2);
    let ids: Vec<Option<i64>> = view.plays.iter().map(|p| p.capture_id).collect();
    assert!(ids.iter().all(Option::is_some), "both were offered");
    assert_ne!(ids[0], ids[1], "and the second play did not take the first's offer");
}

/// An offer re-timed and then accepted is still one row, not two.
///
/// The copy `settle_captures` writes carries the *corrected* time, so it no
/// longer shares a timestamp with the original — and matching the two on time
/// drew both, which is the doubling the whole timeline exists to avoid, in the
/// one case where it is least obvious. Matched on the offer's id instead, which
/// `settle_captures` writes into `source_id`.
#[tokio::test]
async fn a_re_timed_offer_accepted_is_still_one_row() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();
    listen(&db, them, Some(chain), "Teardrop", "Massive Attack", 1_760_000_000).await;

    let cap = db.spool_captures(me).await.unwrap().remove(0);
    db.set_capture_time(me, cap.id, 1_760_009_000).await.unwrap();
    db.settle_captures(me, &[cap.id], true).await.unwrap();

    // Two listens, by design — one per person in the room, and they now
    // disagree about the clock because one of them was corrected.
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scrobbles WHERE joint_session_id = ?")
        .bind(&spool)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rows, 2);

    let view = db.spool_detail(&spool, me).await.unwrap().unwrap();
    assert_eq!(
        view.plays.len(),
        1,
        "one play happened, so one row — the copy is the same play under a corrected clock"
    );
    assert_eq!(view.plays[0].standing, "accepted");
}

/// Saying when you left stops the room going on without you.
///
/// `present_until` was recorded and nothing narrowed on it: `state` said
/// whether you had answered the invitation and the window said when you were
/// actually in the room, and only the first was read. So somebody who had said
/// they left at ten went on being offered plays at eleven, and went on being
/// named in the company of the people still there.
#[tokio::test]
async fn saying_when_you_left_stops_the_captures_and_the_company() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();

    let start = Database::now_secs();
    listen(&db, them, Some(chain), "While I was there", "Massive Attack", start as u64 - 60).await;

    // "I went home a moment ago." State is untouched — leaving through the
    // banner is a different gesture, and this is a correction to the record.
    db.set_member_presence(&spool, me, None, Some(Some(start))).await.unwrap();

    listen(&db, them, Some(chain), "After I left", "Massive Attack", start as u64 + 600).await;

    let waiting: Vec<String> =
        db.spool_captures(me).await.unwrap().into_iter().map(|c| c.title).collect();
    assert_eq!(
        waiting,
        vec!["While I was there".to_string()],
        "a play from after you went home is not yours to answer"
    );

    let company: Option<String> = sqlx::query_scalar(
        "SELECT company FROM scrobbles WHERE user_id = ? AND title = 'After I left'",
    )
    .bind(them)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        !company.unwrap_or_default().contains("ash"),
        "and you are not named in the company of a play you were not there for"
    );

    // The play from while you *were* there still names you, because it was
    // true when it was taken — a company snapshot is a record of the room at
    // the time, not a view onto a list that keeps moving.
    let earlier: Option<String> = sqlx::query_scalar(
        "SELECT company FROM scrobbles WHERE user_id = ? AND title = 'While I was there'",
    )
    .bind(them)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(earlier.unwrap_or_default().contains("ash"));
}

/// And your own plays stop belonging to a sitting you have left.
#[tokio::test]
async fn a_play_after_you_went_home_is_not_part_of_the_sitting() {
    let db = testdb().await;
    let (me, them, chain) = patched_pair(&db).await;
    let spool = db.start_spool(them, None, &[me], &[]).await.unwrap();
    db.set_spool_membership(&spool, me, true).await.unwrap();

    let start = Database::now_secs();
    db.set_member_presence(&spool, me, None, Some(Some(start))).await.unwrap();
    listen(&db, me, Some(chain), "On my own now", "Burial", start as u64 + 600).await;

    let stamped: Option<String> = sqlx::query_scalar(
        "SELECT joint_session_id FROM scrobbles WHERE user_id = ? AND title = 'On my own now'",
    )
    .bind(me)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(stamped, None, "you were not in the room, so it was not a joint listen");
}
