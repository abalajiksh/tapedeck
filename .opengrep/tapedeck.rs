// Cases for tapedeck.yml: `opengrep test .opengrep`. Not compiled — Cargo
// never sees this directory. Every false positive the guard reports should end
// up here as an "ok" case, and every bug a rule exists for as a "ruleid" one.
//
// Rules limited by `paths:` (source-client-outside-egress, sql-outside-a-store,
// sink-outside-sinks-for-user) live in boundaries.yml and are not checked
// here: `opengrep test` ignores `paths:`, so their cases would fire wherever
// this file lives. It ignores `nosemgrep` too, so exceptions are not cases.

use reqwest::Client;

async fn clients() {
    // ruleid: reqwest-client-without-timeout
    let a = reqwest::Client::new();
    // ruleid: reqwest-client-without-timeout
    let b = reqwest::Client::builder().user_agent("x").build()?;
    // ok: reqwest-client-without-timeout
    let c = reqwest::Client::builder().timeout(T).user_agent("x").build()?;
    // ok: reqwest-client-without-timeout
    let c2 = reqwest::Client::builder().user_agent("x").timeout(T).build()?;
    // ruleid: reqwest-client-without-timeout
    let d = reqwest::get("https://example.org").await?;
    // ruleid: reqwest-client-without-timeout
    let e = Client::new();
    // ok: reqwest-client-without-timeout
    let f = Client::builder().timeout(T).build().expect("reqwest client");
}

async fn sql_text(Query(q): Query<Params>, db: &Db) {
    let rows = sqlx::query(&crate::sql!(&format!(
        "SELECT * FROM artists WHERE name = '{}' AND {}",
        // ruleid: sql-format-interpolates-a-value
        q.artist,
        // ok: sql-format-interpolates-a-value
        FIDELITY_KNOWN
    )))
    .fetch_all(&db.pool)
    .await?;
    // ok: sql-format-interpolates-a-value
    let fine = sqlx::query(&crate::sql!(&format!("SELECT {} FROM x WHERE {}", CHAIN_COLUMNS, FIDELITY_KNOWN)));

    let t = q.sort.clone();
    let s = format!("SELECT * FROM x ORDER BY {}", t);
    // ruleid: request-value-into-sql-text
    let r = sqlx::query(&crate::sql!(&s)).fetch_all(&db.pool).await?;

    let col = match q.sort.as_str() { "plays" => "plays", _ => "artist" };
    let s2 = format!("SELECT * FROM x ORDER BY {}", col);
    // ok: request-value-into-sql-text
    let r2 = sqlx::query(&crate::sql!(&s2)).fetch_all(&db.pool).await?;
}

async fn counting(db: &Db) {
    // ruleid: count-in-rust
    let n = sqlx::query(&crate::sql!("SELECT * FROM scrobbles WHERE user_id = ?")).fetch_all(&db.pool).await?.len();

    let rows: Vec<(String, i64)> = sqlx::query_as(&crate::sql!(
        "SELECT sa.artist_key, COUNT(*) FROM scrobble_artists sa GROUP BY sa.artist_key"
    ))
    .fetch_all(&db.pool)
    .await?;
    // ruleid: count-in-rust
    let total = rows.len();

    // ok: count-in-rust
    let k: i64 = sqlx::query_scalar(&crate::sql!("SELECT COUNT(*) FROM scrobbles")).fetch_one(&db.pool).await?;

    // A capped probe, not a whole-history read.
    let capped: Vec<(i64,)> = sqlx::query_as(&crate::sql!("SELECT id FROM scrobbles WHERE user_id = ? LIMIT ?"))
        .fetch_all(&db.pool)
        .await?;
    // ok: count-in-rust
    let more = capped.len() > cap;

    // A small per-user table, not the history.
    let devices: Vec<(i64,)> = sqlx::query_as(&crate::sql!("SELECT id FROM devices WHERE user_id = ?"))
        .fetch_all(&db.pool)
        .await?;
    // ok: count-in-rust
    let nd = devices.len();

    let found: Vec<(String,)> = sqlx::query_as(&crate::sql!("SELECT album FROM scrobbles WHERE user_id = ?"))
        .fetch_all(&db.pool)
        .await?;
    // ok: count-in-rust
    let mut out = Vec::with_capacity(found.len());
}

fn logging() {
    // ruleid: log-crate-not-tracing
    log::info!("hi");
    // ok: log-crate-not-tracing
    tracing::info!("hi");
    // ruleid: print-outside-tests
    println!("hi");
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn t() {
        // ok: print-outside-tests
        println!("{}", 1);
        // ok: reqwest-client-without-timeout
        let c = reqwest::Client::new();
    }
}
