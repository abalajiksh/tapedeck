# Contributing to Tapedeck

Thanks for wanting to help. This page covers what to expect, from setting up a
checkout to getting a pull request merged. The short version:

1. Open a PR against `main` from a branch.
2. An automated **PR guard** tests it, comments on it, and either passes it or
   closes it with the reasons.
3. A green PR is merged by the maintainer. Nobody, the maintainer included,
   pushes to `main` directly.

The rest explains each part, and why it works the way it does.

- [Before you start](#before-you-start)
- [Setting up](#setting-up)
- [Before you open a PR](#before-you-open-a-pr)
- [What happens when you open a PR](#what-happens-when-you-open-a-pr)
- [The database migration report](#the-database-migration-report)
- [Writing a migration](#writing-a-migration)
- [Code conventions](#code-conventions)
- [Commits, versions and releases](#commits-versions-and-releases)
- [Security issues](#security-issues)

## Before you start

- **Tapedeck lives on Codeberg**: <https://codeberg.org/abksh/tapedeck>. Issues and
  PRs go there.
- **The licence is AGPL-3.0** (see `LICENSE`). By opening a PR you agree that your
  contribution is released under it.
- **For anything bigger than a fix, open an issue first** and say what you have in
  mind. Tapedeck has opinions (see [Code conventions](#code-conventions)), and it is
  cheaper to agree on a shape before the code exists.
- `README.md` is the feature tour and the architecture overview. Read its
  *Architecture* and *Project Structure* sections before a large change.

## Setting up

You need:

- **Rust 1.88 or newer.** CI builds with 1.98.
- **[Bun](https://bun.sh).** `build.rs` builds the SvelteKit UI into `static/`
  before rustc runs, and `rust-embed` bakes it into the binary. So bun is needed
  for **every** cargo command, `cargo check` and `cargo clippy` included.

```bash
git clone https://codeberg.org/abksh/tapedeck.git
cd tapedeck
cargo build                                  # SQLite (default)
cargo build --features postgres              # the same tree, against Postgres
TAPEDECK_SKIP_WEB_BUILD=1 cargo check        # Rust-only work, against an existing static/
```

For UI work with hot reload:

```bash
cargo run                 # backend on :8080
cd web && bun run dev     # UI on :5173, proxies /api /1 /admin /health
```

### Running a throwaway instance safely

**Never run the binary bare inside your checkout.** With no configuration it uses
`./scrobbles.db`, `./tapedeck.db`, `./logs`, `./.env` and `./tapedeck.toml` relative
to the current directory, and it migrates whatever database it finds there. Give it
a scratch directory and replica mode, which forwards nothing to Last.fm or
ListenBrainz, asks MusicBrainz nothing, and publishes no ActivityPub actor:

```bash
T=$(mktemp -d)
env -i HOME="$T" PATH=/usr/bin:/bin TAPEDECK_CONFIG="$T/none.toml" TAPEDECK_REPLICA=1 \
  HOST=127.0.0.1 PORT=18080 \
  SQLITE_DB_PATH="$T/tapedeck.db" DATABASE_URL="sqlite:$T/scrobbles.db?mode=rwc" \
  EVERYNOISE_DB_PATH="sqlite:$T/everynoise.db?mode=rwc" TAPEDECK_UPLOADS_DIR="$T/uploads" \
  TAPEDECK_SECRET_KEY_PATH="$T/tapedeck.key" LOG_DIR="$T/logs" RUST_LOG=info \
  ./target/debug/tapedeck
```

Then `POST /api/v1/auth/setup` with `{"username": …, "password": …}` (8+ characters)
creates the first admin and returns a session cookie and an API token.

## Before you open a PR

Run what the guard will run, so its first comment is not a surprise:

```bash
cargo fmt --check                                         # hard requirement
cargo clippy --bin tapedeck                               # advisory, but don't add warnings
cargo test --bin tapedeck                                 # SQLite suite (also builds the UI)
TAPEDECK_SKIP_WEB_BUILD=1 cargo test --bin tapedeck --features postgres
(cd web && bun run check)                                 # svelte-check, advisory
scripts/opengrep.sh                                       # house rules: what your branch adds
```

`scripts/opengrep.sh` downloads the pinned Opengrep release the first time (checked
against its SHA-256) unless that version is already on your `PATH`.

The two suites are different sets, not the same set run twice: the feature decides
which code compiles. There are also **live Postgres checks**, which are the only thing
that proves the SQL translation against a real server. They are opt-in. Point them at
a throwaway Postgres:

```bash
podman run --rm -d --name td-pg -p 55432:5432 -e POSTGRES_HOST_AUTH_METHOD=trust postgres:16
TAPEDECK_TEST_POSTGRES_URL=postgres://postgres@127.0.0.1:55432/postgres \
  TAPEDECK_SKIP_WEB_BUILD=1 cargo test --bin tapedeck --features postgres -- --ignored pg_live
podman stop td-pg
```

Each test creates a schema of its own and drops it, so nothing else in that database is
touched.

**If you changed an API handler**, regenerate the published spec and commit it:

```bash
TAPEDECK_BLESS_OPENAPI=1 cargo test openapi   # rewrites openapi.yaml
cargo test openapi                            # must pass on the committed file
```

**There are no HTTP-layer tests.** Verify a handler by running a throwaway instance (see
[Setting up](#setting-up)) and calling it with `curl`. If your change depends on the shape
of a remote API, pin that shape with a test against a **real captured response**. Field-name
mismatches compile fine and produce a silent `None` forever, and several of this
codebase's worst bugs were exactly that.

## What happens when you open a PR

A Jenkins pipeline called the **PR guard** tests every PR. It starts within a minute of
you opening the PR, pushing to it, or a maintainer labelling it, and **a run takes about
15 minutes**. While it runs, all three statuses show as pending, with a link to the build.
Runs are one at a time, so with several PRs open yours may wait in the queue first.

### Who gets tested

- **Branches in `abksh/tapedeck`** are tested automatically.
- **PRs from forks** are tested once a maintainer adds the **`ok-to-test`** label.
  Your PR's `build.rs` and tests are code the CI runs, and someone has to have looked
  at them first. Until then the PR has no status and cannot be merged. Once labelled,
  every later push is tested automatically.

### Where it runs

Your code is built and tested in a **throwaway container**:

- It exists for one build and has no credentials.
- It can reach the public internet (crates.io, the npm registry, Codeberg) and
  nothing on the maintainer's network.
- It comes with a throwaway Postgres 16 server of its own, for the live checks.

Commenting on your PR, posting its status and fetching private test data all happen
outside that container, before or after your code runs.

### What it tests

It tests **the merge of your branch into the current `main`**, not your branch alone.
What lands is the merge, so the merge is what gets checked.

| Check | Blocks the merge? |
|---|---|
| Merges cleanly onto `main` | yes |
| `cargo fmt --check` | yes |
| SQLite test suite, which also builds the UI | yes |
| The UI bundle boots (`static/index.html`, the auth gate present) | yes |
| Postgres test suite | yes |
| Live Postgres checks (`pg_live`), against the container's own server | yes |
| No AI-tool attribution in commit messages (see [Commits](#commits-versions-and-releases)) | yes |
| No private or generated files committed (see [Commits](#commits-versions-and-releases)) | yes |
| The database migration check (see the next section) | yes |
| No stored data lost on upgrade, unless the PR says it means to | yes |
| clippy: a warning only if your change *adds* warnings compared with `main` | no |
| `svelte-check` | no |
| No new **error** finding from the house rules in `.opengrep/` (see [below](#the-house-rules-opengrep)) | yes, but never closes the PR |
| No new **warning** from the house rules | no |
| The version was bumped when code changed | no |
| No binary over 5 MB added | no |

### What you get back

Three **commit statuses** appear on your PR:

- `tapedeck-pr/tests`: the build, formatting, both suites, the live checks and commit
  hygiene.
- `tapedeck-pr/migrations`: what your change does to an existing database.
- `tapedeck-pr/static`: the house rules, checked by reading the code.

`main` requires all three before anything can be merged.

And **one comment** carries the full report. It is edited in place on each push, so a
PR with twenty pushes has one report, and its title names the commit it describes. It
links the Jenkins build, its console log and the archived results (every check's log),
all readable without an account.

### The house rules (Opengrep)

Some of this project's rules are written down as [Opengrep](https://github.com/opengrep/opengrep)
rules in `.opengrep/`, so a tool holds every change to them rather than a reviewer's
memory: an HTTP client without a timeout, a source reaching the network outside its
`Egress`, a request value spliced into SQL text, a query outside the database layer, a
sink built outside `sinks_for_user`, the `log` crate, `history.replaceState`. Each rule's
message says why it exists.

- **Only what your PR adds counts.** Findings already on `main` are never yours.
- **The rules come from `main`, not from your branch**, so a PR cannot loosen the rule
  it is being checked against. If your PR changes `.opengrep/`, its own rules are run
  and tested too, and reported, but the verdict uses `main`'s.
- **ERROR findings fail `tapedeck-pr/static`. WARNINGs are listed and block nothing.**
- **A deliberate exception** says so on the line, with the reason on the line above
  (anything after the rule id would be read as another rule id):

  ```rust
  // plex.tv, a fixed public host — not the server URL anyone stored.
  let client = Client::builder() // nosemgrep: source-client-outside-egress
  ```

- **A false positive** is a bug in the rule: add the case to `.opengrep/tapedeck.rs` as
  an `ok:` case and fix the rule (`opengrep test .opengrep`).

Opengrep does not read `.svelte` files, so the frontend rules see `.ts` only.

### If it fails: your PR is closed

When a required check fails, the guard **closes the PR** with a comment listing every
reason. Fix them on your branch and **open a new PR**. This keeps the open PR list to
work that can actually be merged. A closed PR is not a rejection: it is a to-do list.

What does **not** close your PR:

- **An infrastructure problem.** The CI machine is down, a download times out, or the
  guard itself crashes. The status then says `error`, which closes nothing; a maintainer
  re-runs the build.
- **A problem `main` or the last release already has.** The migration check also runs
  against `main`, and against the last release itself on an empty database. Anything
  found there is listed under *"Already on main or in vX.Y.Z, not this PR's doing"*,
  and those findings never fail your PR. If neither the release nor your change can
  start on a backend at all, that backend is reported as *not checked* rather than
  failed.
- **A house-rule finding.** `tapedeck-pr/static` fails and blocks the merge, and the PR
  stays open: fix it on the branch and push.
- **A title starting with `WIP:` or `[WIP]`.** The PR is tested and reported on, but
  left open. Remove the prefix when it's ready.

### Getting merged

`main` is protected:

- **No direct pushes**, by anyone.
- **All three statuses green.**
- **Your branch must be up to date with `main`.** If `main` has moved since your last
  push, merge or rebase it in. That is a new push, so it is tested again, and the green
  check covers what will actually land.
- **Only the maintainer merges.**

## The database migration report

Tapedeck runs on people's own machines with years of listening history in them. An
upgrade that loses some of it can't be taken back, so every PR is checked for what it
does to a database made by **the last release**, by running both versions rather than
reading the diff. This is most of the 15 minutes. For each backend, SQLite and Postgres:

1. **fresh**: your change starts on an empty database. This is the shape a new install
   gets.
2. **seed**: the last release starts on an empty database. It is filled through its own
   API: a real listening history for branches in this repository, synthetic listens for
   forks. Then synthetic rows go into every table and column still empty, so that a loss
   anywhere can be seen. The report says how many tables and columns held data.
3. **upgrade**: your change opens a copy. The two are compared table by table, row by
   row, column by column.
4. **drift**: the upgraded schema against the fresh one. A column added to `schema.rs`
   without a migration shows up here.
5. **restart**: your change starts again. A migration whose guard never becomes true
   runs on every start, and this catches it.
6. **rollback**: the last release starts on the upgraded database. Can someone go back
   to the previous binary without restoring a backup?

How to read the comparison:

- **Lost** (blocks the merge): rows deleted; tables or columns dropped while they held
  values; values set to NULL. Values that turn up elsewhere are reported as **moved**,
  not lost. That covers another column of the same row, another table, or a row re-keyed
  under the same parent.
- **Rewritten** (a warning): a value replaced by a different one. The previous value
  then exists only in a backup taken before the upgrade.
- **Backfill** (fine): NULL becomes a value.
- **Already on main**: found for `main` too, so it is not your PR's doing.

**The report never shows data values**, only counts, and table and column names. Part of
the seed is a real person's history.

**A PR that is meant to lose data**, such as dropping a retired table, carries the label
**`migration: destructive-ok`**. Add it **when you open the PR**: without it, the guard
closes the PR before there is a chance to add it later. The data loss is still reported
in full; the label only stops it failing the PR.

The report also lists what was added to `migrate.rs` since the release, and flags lines
worth a second look, each linked to its line. These are `DROP`, `DELETE`, `UPDATE` and
`RENAME` statements, table rebuilds, errors swallowed with `let _ =`, and destructive
statements outside a transaction.

## Writing a migration

There is no migrations directory. `src/db/schema.rs` is the current shape, and
`src/db/migrate.rs` is a run of guarded blocks that walk an older SQLite database up to
it. Read the top of `migrate.rs` before you add one.

- **To add a column, add it to the `CREATE TABLE` in `schema.rs` *and* append a guarded
  block (`if !has_column(...)`) to `migrate.rs`.** With only the first, every existing
  install breaks. The drift check catches this.
- **Append new blocks at the end.** Earlier blocks rebuild tables by copying an explicit
  column list, and a column added before them is silently dropped on exactly the
  databases that still need the rebuild.
- **An index that names a migrated column goes in `migrate.rs` or `Database::indexes()`,
  never in `init()`.** `init()` runs first, and on an existing database the column does
  not exist yet. That was a crash loop in v0.61.1.
- **Never throw data away.** Store every MBID and credit a source hands over. Fill blanks
  rather than overwrite. When something must be rewritten, keep what it replaced, the
  way `artist_display` and `previous_artist_credit` do.
- **Postgres:** `migrate()` is a no-op under the `postgres` feature today, because no
  Postgres database predated the current schema. The first Postgres migration arm needs
  its own guard against `information_schema.columns`. Don't translate the SQLite blocks.
- **Test the upgrade path, not just a fresh database.** A test that builds the old table
  shape by hand, opens it through `Database::new`, and asserts the old data survived is
  the kind that would have caught past crash loops. `src/db/tests.rs` has examples.

## Code conventions

The longer list, with the reason behind each rule, is in the code itself: module docs
and `//` comments say why, `///` comments are published API docs. The ones a PR most
often trips on:

### HTTP and auth
- **axum 0.7 route syntax:** `/:id` and `/*path`. The 0.8 form `{id}` compiles and never
  matches.
- **Authentication is not authorisation.** `AuthUser` means "signed in". Check
  `user.is_admin` explicitly where it matters, and **scope every query by
  `user.user_id`**.
- Token scopes: `ReadAuth` / `WriteAuth` need an explicit `read` / `write` scope. A
  valid token without the scope gets **403, not 401**.
- Handlers build `(StatusCode, Json(ErrorResponse { code, error }))` by hand. There is
  no central error enum, on purpose.
- Where a field means "change this" versus "leave it alone", use `Option<Option<T>>`,
  not `Option<T>`.
- Every handler has a `#[utoipa::path]` and is listed in its module's `Api`. Run
  `cargo test openapi`.

### Database
- **Every `sqlx::query*` goes through `crate::sql!`**, which translates for Postgres.
  `db::guards` fails the build otherwise.
- **Skips are stored but never counted.** Every counting query filters
  `skipped = FALSE`, and `db::guards` enforces that too. A numerator and a denominator
  must be filtered the same way.
- Timestamps are bucketed in the user's own time zone, never a fixed offset.
- Count, rank and search artists over `scrobble_artists`, never `scrobbles.artist`.
  Albums are grouped by `COALESCE(album_filed_under, artist)`.

### Listens and sources
- **Forwarding is strictly per user**, through `sinks::sinks_for_user`. Never add a
  server-wide sink.
- Imports and skips are never forwarded.
- Every source client takes an `Egress`, never a bare `reqwest::Client`, so that a
  member's source URL cannot reach the server's own network.

### Frontend
- A pure SPA (`ssr = false`, no prerendering).
- **Svelte 4 idiom:** `export let`, `on:click`, `$:`, `<slot />`. Not runes.
- All API calls go through `web/src/lib/api.ts`. URL state goes through `syncUrl()`,
  never `history.replaceState`.
- Colours are the Rosé Pine `rp-*` utilities, never hex. Icons are inline SVG in
  `currentColor`, never service logos.

### General
- `tracing` for logging, never `println!` or `log`.
- Configuration is infrastructure only. User-facing settings live in the UI and the
  database.
- **Never present music someone doesn't own as theirs.** Suggestions from outside a
  person's history are labelled as such.

## Commits, versions and releases

### Commits
- **Conventional prefixes with a scope:** `feat(chains): …`, `fix(plex): …`,
  `refactor(db): …`, `docs: …`, `chore: …`. Branches are named the same way:
  `feat/output-setups`, `fix/split-joined-artists`.
- **Messages explain why.** A subject line, then a body that says what changed, why,
  what was traded off, and how it was verified. `git log` on `main` has plenty of
  examples.
- **One logical change per commit**, where you can.
- **No AI-tool attribution.** Commit messages must not carry `Co-Authored-By:` lines
  naming an AI assistant, or "Generated with …" footers. The guard rejects them. To fix
  one, reword the commit (`git rebase -i`, then `reword`) and force-push your branch.
- **Never commit** `CLAUDE.md`, `.claude/`, `temp/`, `design/`, `.env`, database files,
  credential keys, or `static/` (a build output). The guard rejects these too.
  `.gitignore` covers most of them but not `.claude/` or every database file, so look at
  `git status` before you commit.

### Versions
- **A feature bumps the minor version, a fix bumps the patch.** The bump is its own
  `chore: vX.Y.Z` commit, touching `Cargo.toml`, `Cargo.lock` and `openapi.yaml`.
  `info.version` there follows the crate, so regenerate it with
  `TAPEDECK_BLESS_OPENAPI=1 cargo test openapi`. Also sweep the version strings in
  `README.md`, `.env.example` and `tapedeck.toml.example`.
- The guard reminds you (without blocking) when code changed and the version did not.

### Releases
- Releases are `vX.Y.Z` tags, created by the maintainer only.
- A tag builds the release binaries and Linux packages, publishes the API reference,
  and updates the public demo.

## Security issues

**Please do not describe a vulnerability in a public issue or PR.** Open an issue that
says only that you have a security report and how to reach you, and the maintainer will
follow up privately. Fixes for reported issues are tracked privately and land with a
reference in the commit message.
