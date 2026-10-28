//! A source scanner that keeps `skipped = FALSE` auditable now that the queries
//! live in twenty files instead of one.
//!
//! `db.rs` used to be a single file, and CLAUDE.md said plainly why that
//! mattered: it "keeps `skipped = FALSE` (and per-user scoping) auditable in
//! one file". Splitting it into `db/` bought readability and **spent that
//! property** — 143 occurrences spread across a directory are not auditable the
//! way 143 in one file were. A skip leaking into a count has been found four
//! separate times (the language breakdown, the dashboard's top artist, the
//! artwork coverage numerator, and the fidelity cards), and every one of them
//! was a query written beside filtered ones that quietly omitted the filter.
//! Nothing errors; a number is just wrong.
//!
//! So the property moves from "one file you can read" to "a test you cannot
//! ignore". Every `SELECT` that reads `scrobbles` must do one of:
//!
//! * carry the filter itself (`skipped = FALSE`);
//! * be explicitly *about* skips (`skipped = TRUE`) — the Skipped card and
//!   `most_skipped` are the whole population, not a leak;
//! * interpolate a fragment that carries the filter (`PATCH_VISIBLE_LISTENS`,
//!   `CHAIN_COLUMNS`, `SHARED`, `language_queue_predicate()`);
//! * or name its method in [`EXEMPT`] **with a reason**.
//!
//! Default deny, because the failure is silent and the cost of being asked is
//! one line. This follows the same technique as
//! `openapi::tests::the_spec_and_the_router_describe_the_same_api` and
//! `mcp::drift` — a dumb textual scan over our own source, which is exactly as
//! clever as it needs to be.
//!
//! A SQL-ish *test fixture* elsewhere under `src/db/` will trip this if it
//! names `scrobbles` — `dialect.rs` did, once. Rename the fixture's table
//! rather than excluding its file: an exclusion is permanent and silent,
//! where a fixture is neither.
//!
//! **Known limitation:** an exemption is keyed on the *method*, so a method in
//! the list that later grows a counting query is exempt without anyone
//! noticing. The `skipped = TRUE` rule above exists to keep the list small and
//! specifically to keep `get_ranged_stats` — which holds nineteen filtered
//! queries beside one deliberate skip count — out of it. Prefer adding the
//! filter over adding an entry.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Methods whose reads of `scrobbles` legitimately carry no skip filter.
///
/// A reason is mandatory. "It looked fine" is not one — every entry here is a
/// claim that the query is *not* about how much someone listened.
const EXEMPT: &[(&str, &str)] = &[
    // ── Dedup: a skipped listen still occupies its source_id ──────────────
    (
        "existing_id",
        "dedup by source_id; a skip must still dedup or it is \
                     re-inserted as a fresh listen",
    ),
    (
        "fuzzy_existing_id",
        "the live/history double-report window; disposition \
                           is a parameter so a skip cannot swallow a real \
                           listen, and neither can erase the other",
    ),
    (
        "relay_candidates",
        "finds Spotify plays held twice; a copy of a play the \
                          export calls a skip is the pair that was counting a \
                          skip as a listen, so skips must be read",
    ),
    // ── Identity lookups: which record is this, not how much was played ───
    (
        "get_scrobble",
        "fetches one listen by id for editing; the caller \
                      already knows which row it means",
    ),
    (
        "album_for_track",
        "which record a track sits on; a skipped play names \
                         the same album",
    ),
    ("track_by_name", "matches a pulled love to a listen by name"),
    ("track_by_recording_mbid", "matches a pulled love to a listen by MBID"),
    (
        "resolve_reference",
        "resolves a shortcode against the history; having \
                           skipped a track still proves you have the record",
    ),
    ("isrc_map", "maps a track to its ISRC — a property of the recording"),
    ("library_ref_for_track", "maps a track to an item on the user's own server"),
    ("library_refs_for_tracks", "the batched form of the same lookup"),
    (
        "album_release_mbid",
        "the plurality vote for an album's release; the tag \
                            is the file's regardless of whether it finished",
    ),
    (
        "search_notes",
        "searches note bodies; a note attached to a skipped \
                      listen is still the user's own writing",
    ),
    // ── Delivery and repair: keyed on status or id, never on listening ────
    (
        "get_pending_scrobbles",
        "the flush queue, selected on status='pending'. \
                               A skip is always stored 'imported', so it can \
                               never appear here — see ListenDisposition",
    ),
    ("mark_sink_delivered", "reads delivered_sinks for one listen by id"),
    ("link_scrobble_artists", "writes credit rows for one listen by id"),
    (
        "backfill_scrobble_artists",
        "one-off pass writing credit rows for every \
                                   listen; the filter belongs on the readers, \
                                   and a skip still has a performer",
    ),
    (
        "album_art_held",
        "answers whether a client should bother offering \
                                a cover; a skipped listen carries artwork like \
                                any other and excluding them would have the \
                                same sleeve re-uploaded on every run",
    ),
    (
        "reclassify_unclassified_listens",
        "one-off pass giving a listen that reported a \
                                codec the classification ingest never ran; a \
                                skip's stored fidelity is as much a fact as a \
                                play's, and the filter belongs on the readers",
    ),
    (
        "reclassify_dsd_listens",
        "repairs format_type/quality_score on rows the \
                                old classifier misparsed; leaving a skip wrong \
                                would make two stored columns disagree",
    ),
    ("sparse_columns", "reads the blanks of a listen already matched by identity, to merge into"),
    ("get_all_scrobbles", "the backup: it must carry everything, skips included"),
    // ── Sessions: selected by session_id, which a skip never has ──────────
    (
        "get_session",
        "reads one session's listens by session_id; rebuild_sessions \
                     filters skips, and since v0.71.0 ingest no longer assigns \
                     a session to one",
    ),
    ("list_sessions", "the session list, joined on session_id — same reason"),
    ("recompute_session", "recomputes a session's aggregates from its own rows"),
    ("session_scrobbles", "one session's listens by session_id"),
    (
        "session_has_a_gap",
        "one session's listens by session_id, to ask whether a \
                          time edit has left the stored grouping stale",
    ),
    // ── Editing a listen, which is about the row and not about listening ──
    (
        "shift_listen_times",
        "moves listens named by id. A skip has a timestamp \
                            that can be as wrong as any other, and correcting \
                            it is not counting it; the duplicate check beside \
                            it is dedup-shaped and unfiltered for the reason \
                            `fuzzy_exists` is — a skip still occupies its slot \
                            and a listen must not be moved on top of one",
    ),
    // ── Not about listening at all ────────────────────────────────────────
    ("has_column", "probes the schema; the row is never read"),
];

/// SQL fragments that carry the filter themselves, so a query interpolating one
/// is filtered even though its own literal is not.
///
/// Collected from the source rather than hardcoded, so a constant that *loses*
/// the filter stops exempting its callers instead of silently continuing to.
fn filter_bearing_fragments() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (_, src) in db_sources() {
        let mut rest = src.as_str();
        while let Some(i) = rest.find("const ") {
            rest = &rest[i + "const ".len()..];
            let Some(colon) = rest.find(':') else { break };
            let name = rest[..colon].trim();
            if name.is_empty()
                || !name.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
            {
                continue;
            }
            let Some(semi) = rest.find(";\n") else { break };
            if filters_skips(&norm(&rest[..semi])) {
                out.insert(name.to_string());
            }
        }
    }
    // `language_queue_predicate()` is a function rather than a const, so it
    // cannot be found by the scan above. It is pinned by its own test below.
    out.insert("language_queue_predicate".to_string());
    out
}

/// Every `.rs` file under `src/db/`, except this one.
///
/// **This file is excluded deliberately.** The scanner reads its own source,
/// where SQL appears inside the test fixtures below — exactly the trap the
/// OpenAPI drift test records, which had to filter out `.route(` occurrences in
/// its own string literals.
///
/// `tests.rs` and `pg_live.rs` go with it: both are `#[cfg(test)]`-only and
/// hold fixtures rather than production queries, so scanning them reports the
/// test's own SQL as an application defect.
fn db_sources() -> Vec<(PathBuf, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("read src/db").flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "rs")
            && !matches!(
                p.file_name().and_then(|n| n.to_str()),
                Some("guards.rs") | Some("tests.rs") | Some("pg_live.rs")
            )
        {
            let src = std::fs::read_to_string(&p).expect("read db source");
            out.push((p, src));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Whitespace-collapsed, lowercased, with `=` given room either side so
/// `skipped=FALSE` and `skipped = FALSE` read the same.
fn norm(sql: &str) -> String {
    let spaced: String =
        sql.chars().flat_map(|c| if c == '=' { vec![' ', '=', ' '] } else { vec![c] }).collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn tokens(n: &str) -> Vec<&str> {
    n.split(' ').collect()
}

/// `FROM scrobbles` or `JOIN scrobbles` — the table itself, never
/// `scrobble_artists`.
fn reads_scrobbles(n: &str) -> bool {
    let t = tokens(n);
    t.windows(2).any(|w| {
        matches!(w[0], "from" | "join")
            && w[1].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_') == "scrobbles"
    })
}

fn skipped_is(n: &str, values: &[&str]) -> bool {
    let t = tokens(n);
    t.windows(3).any(|w| {
        (w[0] == "skipped" || w[0].ends_with(".skipped"))
            && w[1] == "="
            && values.contains(&w[2].trim_matches(|c: char| !c.is_ascii_alphanumeric()))
    })
}

fn filters_skips(n: &str) -> bool {
    skipped_is(n, &["false", "0"])
}

/// A query whose *subject* is the skipped rows. Not a leak — the population.
fn about_skips(n: &str) -> bool {
    skipped_is(n, &["true", "1"])
}

fn is_read(n: &str) -> bool {
    n.starts_with("select") || n.starts_with("with")
}

/// Rust string literals in `src`, as `(start, end, contents)` byte offsets.
///
/// Handles line and block comments, raw strings, and `\`-escapes. A `\` at the
/// end of a line inside a normal string is Rust's line continuation, so a
/// multi-line SQL statement is one literal.
pub(super) fn literals(src: &str) -> Vec<(usize, usize, String)> {
    let b = src.as_bytes();
    let (mut i, n, mut out) = (0usize, b.len(), Vec::new());
    while i < n {
        match b[i] {
            b'/' if i + 1 < n && b[i + 1] == b'/' => {
                i = src[i..].find('\n').map_or(n, |j| i + j + 1);
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                i = src[i..].find("*/").map_or(n, |j| i + j + 2);
            }
            b'r' if i + 1 < n && (b[i + 1] == b'"' || b[i + 1] == b'#') => {
                let mut j = i + 1;
                let mut hashes = 0;
                while j < n && b[j] == b'#' {
                    hashes += 1;
                    j += 1;
                }
                if j < n && b[j] == b'"' {
                    let term = format!("\"{}", "#".repeat(hashes));
                    match src[j + 1..].find(&term) {
                        Some(k) => {
                            let end = j + 1 + k;
                            out.push((i, end, src[j + 1..end].to_string()));
                            i = end + term.len();
                        }
                        None => i += 1,
                    }
                } else {
                    i += 1;
                }
            }
            b'"' => {
                let mut j = i + 1;
                let mut buf = String::new();
                while j < n {
                    if b[j] == b'\\' {
                        // Line continuation: the newline and the indent vanish.
                        if j + 1 < n && b[j + 1] == b'\n' {
                            j += 2;
                            while j < n && (b[j] == b' ' || b[j] == b'\t') {
                                j += 1;
                            }
                            buf.push(' ');
                            continue;
                        }
                        if j + 1 < n {
                            buf.push(b[j + 1] as char);
                        }
                        j += 2;
                        continue;
                    }
                    if b[j] == b'"' {
                        break;
                    }
                    buf.push(src[j..].chars().next().unwrap());
                    j += src[j..].chars().next().unwrap().len_utf8();
                }
                out.push((i, j, buf));
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// The name of the `fn` whose body contains `off`.
fn enclosing_fn(src: &str, off: usize) -> String {
    let mut best = "?".to_string();
    let mut at = 0usize;
    for line in src.lines() {
        if at > off {
            break;
        }
        if let Some(rest) = line.trim_start().strip_prefix("pub ").or(Some(line.trim_start())) {
            let rest = rest.strip_prefix("pub ").unwrap_or(rest);
            let rest = rest
                .strip_prefix("pub(crate) ")
                .or_else(|| rest.strip_prefix("pub(super) "))
                .unwrap_or(rest);
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            if let Some(sig) = rest.strip_prefix("fn ") {
                let name: String =
                    sig.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() {
                    best = name;
                }
            }
        }
        at += line.len() + 1;
    }
    best
}

/// Every read of `scrobbles` that carries no skip filter, as
/// `(file, line, method, sql)`.
fn unfiltered_reads() -> Vec<(String, usize, String, String)> {
    let frags = filter_bearing_fragments();
    let mut out = Vec::new();
    for (path, src) in db_sources() {
        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        for (start, end, lit) in literals(&src) {
            let n = norm(&lit);
            if !(is_read(&n) && reads_scrobbles(&n)) {
                continue;
            }
            if filters_skips(&n) || about_skips(&n) {
                continue;
            }
            // The filter may arrive through an interpolated fragment.
            let window = &src[end..(end + 600).min(src.len())];
            if frags.iter().any(|f| window.contains(f.as_str())) {
                continue;
            }
            let line = src[..start].matches('\n').count() + 1;
            out.push((file.clone(), line, enclosing_fn(&src, start), n));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The guard.** Every read of `scrobbles` filters skips, is about skips,
    /// or is named in `EXEMPT` with a reason.
    #[test]
    fn every_listen_read_filters_skips_or_says_why() {
        let exempt: BTreeSet<&str> = EXEMPT.iter().map(|(m, _)| *m).collect();
        let offenders: Vec<_> = unfiltered_reads()
            .into_iter()
            .filter(|(_, _, m, _)| !exempt.contains(m.as_str()))
            .collect();
        assert!(
            offenders.is_empty(),
            "these reads of `scrobbles` neither filter skips nor are exempt.\n\
             Add `skipped = FALSE`, or add the method to db::guards::EXEMPT with \
             a reason saying why the query is not about how much someone \
             listened:\n{}",
            offenders
                .iter()
                .map(|(f, l, m, s)| format!("  {f}:{l}  {m}\n      {}", &s[..s.len().min(120)]))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// A scanner that finds nothing passes everything. This is what stops the
    /// guard above from going quietly green because the parser broke — the same
    /// companion the OpenAPI drift test carries.
    #[test]
    fn the_scanner_finds_the_queries_it_is_meant_to_guard() {
        let mut reads = 0usize;
        let mut filtered = 0usize;
        for (_, src) in db_sources() {
            for (_, _, lit) in literals(&src) {
                let n = norm(&lit);
                if is_read(&n) && reads_scrobbles(&n) {
                    reads += 1;
                    if filters_skips(&n) {
                        filtered += 1;
                    }
                }
            }
        }
        assert!(
            reads > 100,
            "only {reads} reads of `scrobbles` found — the literal scanner is \
             probably broken, and a broken scanner makes the guard pass by \
             examining nothing"
        );
        assert!(
            filtered > 80,
            "only {filtered} of {reads} reads carry the filter — the filter \
             detector is probably broken"
        );
    }

    /// And it must actually catch the thing it exists for. Verified against a
    /// synthetic query rather than trusting that the real tree is clean.
    #[test]
    fn the_scanner_catches_a_query_that_forgets_the_filter() {
        let bad = norm("SELECT COUNT(*) FROM scrobbles WHERE user_id = ?");
        assert!(is_read(&bad) && reads_scrobbles(&bad));
        assert!(!filters_skips(&bad) && !about_skips(&bad));

        let good = norm("SELECT COUNT(*) FROM scrobbles WHERE user_id = ? AND skipped = FALSE");
        assert!(filters_skips(&good));

        // A join, a table alias, and the qualified spelling all count.
        let joined = norm(
            "SELECT s.id FROM notes n JOIN scrobbles s ON s.id = n.scrobble_id \
             WHERE s.skipped = FALSE",
        );
        assert!(reads_scrobbles(&joined) && filters_skips(&joined));

        // `skipped = TRUE` is the skips' own population, not a leak.
        let skips = norm("SELECT COUNT(*) FROM scrobbles WHERE skipped = TRUE");
        assert!(about_skips(&skips) && !filters_skips(&skips));

        // A different table that merely starts the same way is not this one.
        let other = norm("SELECT 1 FROM scrobble_artists WHERE scrobble_id = ?");
        assert!(!reads_scrobbles(&other));
    }

    /// `tracks_wanting_language` and its counterpart are exempt from the list
    /// only because they interpolate this predicate. If it ever loses the
    /// filter, those queries silently start counting skips — so the claim is
    /// pinned here rather than trusted.
    #[test]
    fn the_language_queue_predicate_carries_the_filter() {
        assert!(
            filters_skips(&norm(&crate::db::Database::language_queue_predicate())),
            "language_queue_predicate() no longer filters skips — the language \
             backfill and its coverage readout both depend on it"
        );
    }

    /// An exemption without a reason is a hole with a name on it.
    #[test]
    fn every_exemption_carries_a_reason() {
        for (method, reason) in EXEMPT {
            assert!(reason.len() > 20, "{method} is exempt with no real reason given");
        }
        let names: BTreeSet<&str> = EXEMPT.iter().map(|(m, _)| *m).collect();
        assert_eq!(names.len(), EXEMPT.len(), "duplicate entry in EXEMPT");
    }

    /// Every query goes through the dialect, or Postgres cannot parse it.
    ///
    /// A `sqlx::query*` call handed raw SQL keeps its `?` binds, which Postgres
    /// answers with `syntax error at or near ","` — and only on the build
    /// nobody may be running, so nothing else in the tree notices. This is a
    /// textual scan for the same reason the OpenAPI route drift test is one.
    ///
    /// `migrate.rs` and `tests.rs` are exempt because both are compiled out
    /// under `--features postgres`, so their queries cannot reach it.
    #[test]
    fn every_query_goes_through_the_dialect() {
        const EXEMPT_FILES: &[(&str, &str)] = &[
            ("migrate.rs", "SQLite-only: the whole body is cfg'd out under --features postgres"),
            ("tests.rs", "SQLite-backed fixtures, compiled out under --features postgres"),
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("read src/").flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        walk(&root, &mut files);
        files.sort();

        let exempt: BTreeSet<&str> = EXEMPT_FILES.iter().map(|(f, _)| *f).collect();
        for (f, reason) in EXEMPT_FILES {
            assert!(reason.len() > 30, "{f} is exempt with no real reason given");
        }

        let (mut checked, mut raw) = (0usize, Vec::new());
        for path in &files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if exempt.contains(name) || matches!(name, "dialect.rs" | "guards.rs" | "pg_live.rs") {
                continue;
            }
            let src = std::fs::read_to_string(path).expect("read source");
            for (idx, _) in src.match_indices("sqlx::query") {
                let after = &src[idx + "sqlx::query".len()..];
                // `_as` / `_scalar`, then an optional turbofish, then the call.
                let after = after
                    .strip_prefix("_scalar")
                    .or_else(|| after.strip_prefix("_as"))
                    .unwrap_or(after);
                let after = if let Some(t) = after.strip_prefix("::<") {
                    // Angle-matched, because a turbofish holds `(i64,)` and
                    // looking for the next `(` finds the tuple's, not the call's.
                    let (mut depth, mut end) = (1usize, 0usize);
                    for (k, c) in t.char_indices() {
                        match c {
                            '<' => depth += 1,
                            '>' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = k + 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    if end == 0 {
                        continue;
                    }
                    &t[end..]
                } else {
                    after
                };
                let Some(arg) = after.strip_prefix('(') else { continue };
                let arg = arg.trim_start();
                checked += 1;
                if !arg.starts_with("&crate::sql!") && !arg.starts_with("&stmt") {
                    let line = src[..idx].matches('\n').count() + 1;
                    raw.push(format!(
                        "  {}:{line}  {}",
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
                        arg.chars().take(60).collect::<String>()
                    ));
                }
            }
        }
        assert!(checked > 400, "only {checked} query sites found — the scanner is broken");
        assert!(
            raw.is_empty(),
            "these queries do not go through `crate::sql!`, so Postgres will not parse \
             their `?` binds:\n{}",
            raw.join("\n")
        );
    }

    /// Every index `migrate()` creates must also be created where **both**
    /// backends will run it.
    ///
    /// `migrate()` is SQLite-only, so an index that exists only there is an
    /// index a Postgres database silently does without. That is not a
    /// performance question for all of them:
    /// `idx_scrobbles_user_source` is the UNIQUE constraint `exists` dedup
    /// rests on, so its absence means the same listen banked twice, with
    /// nothing erroring.
    ///
    /// `migrate()` keeps its own copies on purpose — a table rebuild drops the
    /// table and has to put them back in the same transaction — so the rule is
    /// containment rather than exclusivity.
    #[test]
    fn every_index_migrate_creates_is_part_of_the_shared_shape() {
        fn index_names(file: &str) -> BTreeSet<String> {
            let src = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db").join(file),
            )
            .unwrap_or_else(|e| panic!("read {file}: {e}"));
            let mut out = BTreeSet::new();
            for (_, _, lit) in literals(&src) {
                let flat = norm(&lit);
                let Some(rest) = flat.strip_prefix("create ") else { continue };
                let rest = rest.strip_prefix("unique ").unwrap_or(rest);
                let Some(rest) = rest.strip_prefix("index if not exists ") else { continue };
                let name: String =
                    rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if !name.is_empty() {
                    out.insert(name);
                }
            }
            out
        }
        // Created by `migrate()` and deliberately not repeated in the shared
        // set, with the reason — the shape of `EXEMPT` above.
        const ONLY_IN_MIGRATE: &[(&str, &str)] = &[(
            "idx_scrobbles_user_source",
            "`init()` declares UNIQUE(user_id, source_id, source_name) inline on the \
             table, so every current database already has the constraint under the \
             engine's own name. This named index exists only to give it to a database \
             that predates the user_id column, which can only be a SQLite one.",
        )];

        let in_migrate = index_names("migrate.rs");
        let shared = index_names("schema.rs");

        // A scanner that finds nothing would pass this by examining an empty
        // set — the trap the OpenAPI drift test records.
        assert!(
            in_migrate.len() >= 8 && shared.len() >= 8,
            "the index scanner found {} in migrate.rs and {} in schema.rs — it is broken",
            in_migrate.len(),
            shared.len()
        );

        for (name, reason) in ONLY_IN_MIGRATE {
            assert!(reason.len() > 40, "{name} is exempt with no real reason given");
            assert!(
                in_migrate.contains(*name),
                "{name} is listed as migrate-only and migrate() no longer creates it"
            );
        }
        let exempt: BTreeSet<&str> = ONLY_IN_MIGRATE.iter().map(|(n, _)| *n).collect();

        let missing: Vec<&str> = in_migrate
            .iter()
            .filter(|n| !shared.contains(*n) && !exempt.contains(n.as_str()))
            .map(|s| s.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "these indexes are created only by migrate(), which does not run under \
             --features postgres, so a Postgres database would not have them: {missing:?}.\n\
             Add them to Database::indexes() in schema.rs — keeping migrate()'s own copy \
             is fine and expected where a table rebuild has to recreate it."
        );
    }

    /// Under Postgres all four schemas share one database, so a table name
    /// used twice would have one module silently reading the other's rows.
    ///
    /// Under SQLite they are separate files and a collision is invisible — so
    /// this only fails on the build nobody may be running, which is exactly why
    /// it is a test rather than a note.
    #[test]
    fn the_auxiliary_schemas_do_not_collide_with_the_main_one() {
        use std::collections::BTreeMap;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let files = [
            ("main", "src/db/schema.rs"),
            ("everynoise", "src/everynoise/store.rs"),
            ("lyrics", "src/lyrics.rs"),
            ("musicbrainz", "src/musicbrainz.rs"),
        ];
        let mut owner: BTreeMap<String, &str> = BTreeMap::new();
        let mut clashes: Vec<String> = Vec::new();
        let mut found = 0usize;
        for (module, rel) in files {
            let src = std::fs::read_to_string(root.join(rel)).expect("read schema source");
            for (_, _, lit) in literals(&src) {
                let mut rest = lit.as_str();
                while let Some(i) = rest.find("CREATE TABLE IF NOT EXISTS ") {
                    rest = &rest[i + "CREATE TABLE IF NOT EXISTS ".len()..];
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if name.is_empty() {
                        continue;
                    }
                    found += 1;
                    match owner.get(&name) {
                        Some(prev) if *prev != module => {
                            clashes.push(format!("`{name}` in both {prev} and {module}"))
                        }
                        _ => {
                            owner.insert(name, module);
                        }
                    }
                }
            }
        }
        assert!(found > 40, "only {found} tables found — the scanner is broken");
        assert!(
            clashes.is_empty(),
            "these table names are used by more than one schema, which under \
             Postgres means one module reading another's rows: {clashes:?}"
        );
    }

    /// A stale exemption is how this guard rots: the query gets its filter (or
    /// is deleted) and the entry stays, quietly exempting whatever is written
    /// under that name next.
    #[test]
    fn no_exemption_is_stale() {
        let live: BTreeSet<String> = unfiltered_reads().into_iter().map(|(_, _, m, _)| m).collect();
        let stale: Vec<&str> =
            EXEMPT.iter().map(|(m, _)| *m).filter(|m| !live.contains(*m)).collect();
        assert!(
            stale.is_empty(),
            "these methods no longer contain an unfiltered read and should be \
             removed from EXEMPT: {stale:?}"
        );
    }
}
