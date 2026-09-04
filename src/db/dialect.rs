//! Which SQL dialect this build speaks, and the mechanical differences between
//! them.
//!
//! The backend is chosen at **compile time** by the `postgres` cargo feature —
//! one source tree, two builds:
//!
//! ```text
//! cargo build --release                      # SQLite  (default)
//! cargo build --release --features postgres  # Postgres
//! ```
//!
//! The feature picks nothing but the [`super::Db`] type alias. Both sqlx
//! drivers are compiled either way — they already were, because the optional
//! MusicBrainz mirror is Postgres regardless — which is what lets the Postgres
//! build open an old SQLite file to migrate out of it.
//!
//! **The SQLite path is frozen behaviour.** Everything here is either a no-op
//! for SQLite or is applied only under `postgres`. That is the whole safety
//! argument for a compile-time backend: a SQLite build has no Postgres code in
//! it and cannot be regressed by this work.
//!
//! # What [`rewrite`] does automatically
//!
//! | | SQLite | Postgres | why it is safe to automate |
//! |---|---|---|---|
//! | placeholders | `?` | `$1`, `$2`, … | positional either way; the tokeniser skips literals and comments |
//! | `LIKE` | ASCII case-**insensitive** | case-**sensitive** — so `ILIKE` | reproduces SQLite's semantics. Postgres folds full Unicode, so it matches a superset, which for a search box is right |
//! | `CHAR(n)` | the character function | `CHR(n)` | `CHAR(` never appears as a column type in this schema — checked — so there is nothing to mangle |
//!
//! # What it deliberately does **not** do
//!
//! These need a decision or a rewrite per site, not a textual substitution.
//! Recorded here because each one fails *silently* if it is missed.
//!
//! * **`LOWER()` folds differently, and this is the sharp one.** SQLite's is
//!   ASCII-only; Postgres's is Unicode-aware. 308 sites. Get it wrong and
//!   `entity_key`, `artist_key`, `loveKey`, `crateKey`, the chart groupings and
//!   the `fuzzy_exists` window all shift underneath a history written with the
//!   other fold — hearts stop filling, `Cœur de pirate` splits into two chart
//!   rows, and nothing errors. [`lower`] emits `LOWER(x COLLATE "C")` under
//!   Postgres to reproduce SQLite byte for byte, because a *migrated* database
//!   must keep answering the way it did. **Not yet verified against a live
//!   Postgres** — it is the first thing to check when one exists.
//!
//!   Worth knowing while you are in here: SQL and Rust already disagree on
//!   SQLite. `Database::artist_key` uses Rust's `to_lowercase`, which is full
//!   Unicode, while `LOWER()` beside it is ASCII-only. CLAUDE.md records this
//!   for `library_refs_for_tracks`, which deliberately keys on the string
//!   SQLite computed. Postgres would make the SQL side agree with the Rust
//!   side — arguably more correct, and still a behaviour change, which is why
//!   it is not being taken by accident.
//! * **`printf('%012d', …)`** in `COVER_PICK` is SQLite's. Postgres wants
//!   `lpad(x::text, 12, '0')`. One site, and it decides whether a grouped cover
//!   comes from one listen.
//! * **DDL**: `AUTOINCREMENT` (56), partial-index and CHECK-rebuild guards read
//!   out of `sqlite_master` (7), `PRAGMA` (10). All of `schema.rs` and
//!   `migrate.rs` — the reason those were split out first.
//! * **`json_each`** (13) → `jsonb_array_elements`.
//! * **`last_insert_rowid()`** (17) → `RETURNING`.
//! * **`INSERT OR IGNORE` / `OR REPLACE`** (16) → `ON CONFLICT`.
//! * **`GROUP_CONCAT`** (2) → `string_agg`, **`strftime`** (6), **`datetime(`** (6).
//! * **Bare columns beside an aggregate.** `loves_detailed` leans on SQLite's
//!   rule that one `min`/`max` makes bare columns come from that row. Postgres
//!   rejects it outright — loud, at least, but it is a rewrite rather than a
//!   translation.
//! * **`VACUUM INTO`** (8) is the whole backup endpoint and has no Postgres
//!   equivalent short of `pg_dump`.

use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
}

/// The dialect this binary was built for.
pub const ACTIVE: Dialect =
    if cfg!(feature = "postgres") { Dialect::Postgres } else { Dialect::Sqlite };

impl Dialect {
    /// The name to show a human — in a startup line, or when refusing a
    /// `DATABASE_URL` this build cannot serve.
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Sqlite => "SQLite",
            Dialect::Postgres => "Postgres",
        }
    }

    /// The URL schemes this build accepts.
    pub fn schemes(self) -> &'static [&'static str] {
        match self {
            Dialect::Sqlite => &["sqlite"],
            Dialect::Postgres => &["postgres", "postgresql"],
        }
    }

    /// Whether this build can serve `url` as its primary store.
    ///
    /// A bare path with no scheme is SQLite's historical spelling and stays
    /// accepted, so an existing `DATABASE_URL=scrobbles.db` keeps working.
    pub fn accepts(self, url: &str) -> bool {
        match url.split_once(':') {
            Some((scheme, _)) if scheme.chars().all(|c| c.is_ascii_alphabetic()) => {
                self.schemes().contains(&scheme)
            }
            _ => self == Dialect::Sqlite,
        }
    }
}

/// A lowercase fold that means the same thing in both dialects.
///
/// See the module docs — Postgres folds full Unicode where SQLite folds only
/// ASCII, so `COLLATE "C"` is what keeps a migrated database answering the way
/// it did rather than silently regrouping every accented artist.
pub fn lower(expr: &str) -> String {
    match ACTIVE {
        Dialect::Sqlite => format!("LOWER({expr})"),
        Dialect::Postgres => format!("LOWER({expr} COLLATE \"C\")"),
    }
}

/// Rewrite a query written in SQLite's spelling into the active dialect.
///
/// Under SQLite this is the identity and borrows, so it costs nothing.
pub fn rewrite(sql: &str) -> Cow<'_, str> {
    match ACTIVE {
        Dialect::Sqlite => Cow::Borrowed(sql),
        Dialect::Postgres => Cow::Owned(to_postgres(sql)),
    }
}

/// The tokeniser. Walks `sql` once, copying through string literals, quoted
/// identifiers and comments untouched, and translating everything else.
///
/// Skipping literals is the load-bearing part: a `?` inside `'why?'` is data,
/// and turning it into `$1` corrupts the query while leaving it perfectly valid
/// SQL — which is exactly the shape of bug this codebase keeps finding.
fn to_postgres(sql: &str) -> String {
    let b = sql.as_bytes();
    let (mut out, mut i, n) = (String::with_capacity(sql.len() + 16), 0usize, b.len());
    let mut param = 0u32;
    while i < n {
        match b[i] {
            // '…' string literal; '' is an escaped quote and stays inside it.
            b'\'' => {
                let start = i;
                i += 1;
                while i < n {
                    if b[i] == b'\'' {
                        if i + 1 < n && b[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                out.push_str(&sql[start..i]);
            }
            // "…" quoted identifier.
            b'"' => {
                let start = i;
                i += 1;
                while i < n && b[i] != b'"' {
                    i += 1;
                }
                i = (i + 1).min(n);
                out.push_str(&sql[start..i]);
            }
            // -- line comment
            b'-' if i + 1 < n && b[i + 1] == b'-' => {
                let start = i;
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
                out.push_str(&sql[start..i]);
            }
            // /* block comment */
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                let start = i;
                i = sql[i..].find("*/").map_or(n, |j| i + j + 2);
                out.push_str(&sql[start..i]);
            }
            b'?' => {
                param += 1;
                out.push('$');
                out.push_str(&param.to_string());
                i += 1;
            }
            _ => {
                // Keyword substitutions, only on a word boundary.
                if let Some(rest) = word_at(sql, i, "LIKE") {
                    out.push_str("ILIKE");
                    i = rest;
                    continue;
                }
                if let Some(rest) = call_at(sql, i, "CHAR") {
                    out.push_str("CHR");
                    i = rest;
                    continue;
                }
                let c = sql[i..].chars().next().unwrap();
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

fn is_word_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `sql[i..]` starts with `word` (case-insensitively) as a whole word.
fn word_at(sql: &str, i: usize, word: &str) -> Option<usize> {
    let b = sql.as_bytes();
    let end = i + word.len();
    if end > b.len() || !sql[i..end].eq_ignore_ascii_case(word) {
        return None;
    }
    if i > 0 && is_word_byte(b[i - 1]) {
        return None;
    }
    if end < b.len() && is_word_byte(b[end]) {
        return None;
    }
    Some(end)
}

/// As [`word_at`], but only when the word is immediately followed by `(` — so
/// the `CHAR` *function* is matched and a `CHAR` column type would not be.
fn call_at(sql: &str, i: usize, word: &str) -> Option<usize> {
    let end = word_at(sql, i, word)?;
    (sql.as_bytes().get(end) == Some(&b'(')).then_some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: a `?` that is *data* must survive.
    #[test]
    fn a_question_mark_inside_a_literal_is_not_a_placeholder() {
        assert_eq!(
            to_postgres("SELECT * FROM t WHERE title = 'why?' AND user_id = ?"),
            "SELECT * FROM t WHERE title = 'why?' AND user_id = $1"
        );
        // '' is an escaped quote, not the end of the literal.
        assert_eq!(
            to_postgres("SELECT * FROM t WHERE s = 'it''s ? really' AND a = ?"),
            "SELECT * FROM t WHERE s = 'it''s ? really' AND a = $1"
        );
        assert_eq!(to_postgres("SELECT '' , ?"), "SELECT '' , $1");
    }

    #[test]
    fn placeholders_are_numbered_in_order() {
        assert_eq!(
            to_postgres("WHERE a = ? AND b = ? AND c IN (?, ?)"),
            "WHERE a = $1 AND b = $2 AND c IN ($3, $4)"
        );
    }

    #[test]
    fn comments_and_quoted_identifiers_are_left_alone() {
        assert_eq!(
            to_postgres("SELECT 1 -- is it ?\nWHERE a = ?"),
            "SELECT 1 -- is it ?\nWHERE a = $1"
        );
        assert_eq!(to_postgres("/* ? */ WHERE a = ?"), "/* ? */ WHERE a = $1");
        assert_eq!(
            to_postgres("SELECT \"weird?col\" WHERE a = ?"),
            "SELECT \"weird?col\" WHERE a = $1"
        );
    }

    /// SQLite's LIKE is ASCII case-insensitive and Postgres's is not, so a
    /// search that silently stops matching is the failure being prevented.
    #[test]
    fn like_becomes_ilike_but_only_as_a_whole_word() {
        assert_eq!(
            to_postgres("WHERE body_md LIKE ? ESCAPE '\\'"),
            "WHERE body_md ILIKE $1 ESCAPE '\\'"
        );
        assert_eq!(to_postgres("WHERE a like ?"), "WHERE a ILIKE $1");
        // Not a substring of an identifier, and not inside a literal.
        assert_eq!(to_postgres("SELECT unlike, likeness FROM t"), "SELECT unlike, likeness FROM t");
        assert_eq!(to_postgres("WHERE s = 'LIKE'"), "WHERE s = 'LIKE'");
    }

    #[test]
    fn char_the_function_becomes_chr_and_a_type_would_not() {
        assert_eq!(to_postgres("SELECT a || CHAR(31) || b"), "SELECT a || CHR(31) || b");
        // No `(` — a column type, left alone. (This schema has none, but the
        // rule is what makes the substitution safe to automate.)
        assert_eq!(to_postgres("name CHAR NOT NULL"), "name CHAR NOT NULL");
    }

    #[test]
    fn sqlite_rewriting_is_the_identity_and_does_not_allocate() {
        // Guards the frozen-behaviour promise: under the default build the
        // rewriter must not touch a single byte.
        // Deliberately not `scrobbles`: db::guards scans this file too, and a
        // fixture naming the real table would trip a guard that is working
        // correctly. Renaming the fixture beats excluding the file, which
        // would make it a permanent blind spot.
        let sql = "SELECT * FROM t WHERE user_id = ? AND title LIKE ? AND s = 'a?b'";
        if ACTIVE == Dialect::Sqlite {
            assert!(matches!(rewrite(sql), Cow::Borrowed(_)));
            assert_eq!(rewrite(sql), sql);
        }
    }

    #[test]
    fn a_build_accepts_only_the_urls_it_can_serve() {
        assert!(Dialect::Sqlite.accepts("sqlite:scrobbles.db"));
        assert!(Dialect::Sqlite.accepts("sqlite:/data/scrobbles.db?mode=rwc"));
        // The historical bare-path spelling still works.
        assert!(Dialect::Sqlite.accepts("scrobbles.db"));
        assert!(Dialect::Sqlite.accepts("/data/scrobbles.db"));
        assert!(!Dialect::Sqlite.accepts("postgres://u:p@host/db"));

        assert!(Dialect::Postgres.accepts("postgres://u:p@host/db"));
        assert!(Dialect::Postgres.accepts("postgresql://u:p@host/db"));
        assert!(!Dialect::Postgres.accepts("sqlite:scrobbles.db"));
        assert!(!Dialect::Postgres.accepts("scrobbles.db"));
    }

    #[test]
    fn the_lower_fold_is_pinned_to_sqlites_so_a_migration_keeps_its_groupings() {
        match ACTIVE {
            Dialect::Sqlite => assert_eq!(lower("TRIM(artist)"), "LOWER(TRIM(artist))"),
            Dialect::Postgres => {
                assert_eq!(lower("TRIM(artist)"), "LOWER(TRIM(artist) COLLATE \"C\")")
            }
        }
    }
}
