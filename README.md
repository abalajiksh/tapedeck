# 📼 Tapedeck

A self-hosted music intelligence hub written in Rust. Tapedeck ingests scrobble
data from any source — Plex, Pano Scrobbler, Navidrome, web browsers, phones,
DAPs — enriches it with MusicBrainz metadata and audio quality context, and
stores a complete personal listening history with audiophile-grade analytics.

Single binary. Privacy-first. DSD is first-class.

## What makes Tapedeck different

Most scrobblers track *what* you listened to. Tapedeck tracks *how* — the full
signal chain from source file through DAC and amp to transducer, whether
your FLAC was degraded to SBC over Bluetooth, and how many hours are on your
HD 650 drivers. It's a scrobbler built by and for someone who cares about the
listening experience, not just the listening history.

## Features

### Universal Ingest API (Phase 1)

Tapedeck implements the ListenBrainz Core API endpoints a scrobble client
actually exercises — `validate-token`, `submit-listens`, `user/…/listens`,
`playing-now`, `listen-count`, `services`, `playing-now/delete` and
`latest-import` — so clients like Pano Scrobbler, Web Scrobbler,
multi-scrobbler and mpdscribble work out of the box. Point one at your Tapedeck
URL, paste a token, done.

Two deliberate differences from listenbrainz.org:

- **Reads need a token.** On ListenBrainz a user's listens are public. A
  self-hosted Tapedeck is usually private, so these endpoints require
  authentication and only return your own data (admins can read any user).
- **No MSIDs.** Tapedeck keys listens by source rather than assigning recording
  MSIDs, so `/1/delete-listen` isn't implemented — use
  `DELETE /api/v1/scrobbles/:id`. Playlists, `lb-radio` and similar-users aren't
  implemented either; they return a clean JSON 404 rather than a stub.

Beyond the LB spec, Tapedeck accepts extended fields for audio quality,
device identification, signal chain tagging, and session metadata — and reads
them back in `additional_info`, so a listen round-trips intact.

### Scrobble Proxy

Tapedeck stores listens locally *and* forwards them to Last.fm and
ListenBrainz simultaneously. You don't have to choose — Tapedeck sits in
front of both.

Forwarding is **strictly per user**: each account connects its own Last.fm and
Libre.fm (one-click OAuth) and ListenBrainz (paste a token) from Settings →
Scrobble Connections. There are no server-wide credentials, so a listen can only
ever reach accounts the listening user connected themselves — nothing leaks
between users on a shared instance. Connect nothing and your listens simply stay
local. Tapedeck ships its own registered Last.fm application key, so there's no
API registration step.

### Signal Chain Intelligence (Phase 2)

Define your audio signal chains — the ordered path from source to ears:

```
Desktop → Schiit Mimir (DAC) → Schiit Midgaard (Amp) → HD 650
```

Each chain carries a default listening context (active, passive, background).
When a scrobble arrives tagged with a chain, the context propagates
automatically. The gear choice reflects the intent — respect the gear,
respect the classification.

### Audio Quality Tracking (Phase 2)

Every scrobble can carry audio quality metadata: codec, sample rate, bit depth,
DSD rate, and delivery format. Tapedeck computes a 0–100 quality score and
tracks source vs. delivered quality, so you know when your FLAC was degraded
to AAC over Bluetooth.

Quality scores:
- DSD128: 95 · DSD64: 92
- FLAC 24/192: 90 · FLAC 24/96: 85 · FLAC 16/44.1 (CD): 80
- LDAC 990kbps: ~77 · AAC 256: 55
- Penalties: −10 if transcoded, −3 to −15 for BT codec

### Equipment Usage Tracking (Phase 2)

Track total hours on each piece of gear. Useful for driver burn-in tracking,
warranty records, and understanding how your listening habits distribute
across your equipment.

### Session Grouping (Phase 2)

Individual scrobbles are automatically grouped into listening sessions —
contiguous periods with gaps under 30 minutes. Each session records duration,
track count, quality stats, and listening context.

### Multi-User Support

Multiple users in the same household can each have their own token, their own
scrobble history, their own signal chains. Data is isolated per-user. First run
is a web onboarding flow — open the UI and it walks you through creating the
admin account (no config files, no console token hunting). Admins then manage
everyone from the **Users** screen: create accounts, change roles, reset
passwords, and remove users (with guards against deleting yourself or the last
admin).

### Reliability

Built to be trusted as your system of record:

- **Durable ingest.** Listens are written to SQLite (WAL mode + busy-timeout so
  concurrent writers wait rather than error) before the API returns. If a listen
  can't be persisted, `/1/submit-listens` returns a retryable `503` so the client
  resends — dedup makes retries safe, so nothing is silently dropped.
- **At-least-once forwarding, no double-scrobbles.** Each pending listen tracks
  which sinks have accepted it; a retry only re-sends to the sinks that haven't,
  so a flaky Last.fm won't get duplicate scrobbles when ListenBrainz was down.
  Both sinks retry with backoff.
- **Graceful, data-safe shutdown.** On `SIGTERM` the HTTP server drains in-flight
  requests, then the poll engine finishes its current tick and exits — no work is
  cut mid-write. Even a hard `SIGKILL` can't corrupt the DB (WAL + atomic writes);
  pending scrobbles resume on restart, and an interrupted import is recorded as
  such rather than left hanging. Metadata enrichment runs off the request path,
  so a bulk submit returns in milliseconds and can't stall shutdown. A sample
  hardened `systemd` unit (with a sensible `TimeoutStopSec`) is in `deploy/`.
- **Backups.** `GET /api/v1/backup` (or the Settings screen) downloads a
  consistent `VACUUM INTO` snapshot of the whole database, safe to take while
  running. `GET /health` reports DB connectivity for uptime monitoring.
- **Login throttling.** Repeated failed logins from an IP are rate-limited
  (8 tries / 5 min, `Retry-After` on block) to blunt online password guessing;
  Argon2 hashing already makes each attempt expensive.

### MusicBrainz Enrichment

3-tier metadata lookup:
1. SQLite cache (~2ms, 85–95% hit rate)
2. Optional PostgreSQL MusicBrainz dump (~30ms, 60–80% coverage)
3. MusicBrainz API fallback (~500ms, rate-limited)

Every scrobble gets recording, release, and artist MBIDs plus Cover Art
Archive artwork references.

### Plex Integration

Polls Plex history and active sessions directly — no webhooks required, no
Plex Pass needed. Session tracking with accurate scrobble thresholds
(50% played or 4 minutes). Now Playing support for ListenBrainz.

### Web UI (Phase 3)

A SvelteKit single-page app is embedded directly in the binary (via
`rust-embed`) — no separate deployment. The "cassette deck" interface ships
with:

- **Light / dark / system theme**, chosen per user. The choice is applied
  before first paint (no flash), cached in `localStorage`, and persisted
  server-side so every device matches.
- **Dashboard** — a now-rolling cassette hero, quick stats, and recent listens.
- **History** — grouped by day with inline edit (fix title/artist/album,
  recording MBID, signal chain, format), per-listen delete, and **Inspect
  listen**: the stored row verbatim, every column including the ones no screen
  renders, with copy-to-clipboard and MusicBrainz links.
- **Album art** from the Cover Art Archive, matched via MusicBrainz and
  backfilled in the background; toggle it off in Settings for lettered tiles.
- **Analytics** — timeframe-ranged totals, top artists, and a listening-clock
  heatmap.
- **Import** — pull from ListenBrainz or Last.fm, or upload a `.json`/`.csv`
  export, with live progress.
- **Signal Chains & Equipment**, plus **Settings** for appearance, API tokens,
  connection test, and data export / wipe.

### Musical Personality

The Analytics screen profiles *how* you listen across five explainable axes —
**Adventurer ↔ Loyalist** (artist spread), **Curator ↔ Drifter** (album-block
listening), **Casual ↔ Obsessive** (replays), **Lark ↔ Night Owl** (hours), and
**Mood-Driven ↔ Identity** (how much your rotation shifts by context) — and names
you (e.g. "The Obsessive Night Owl"). It also scores each artist's *loyalty*,
separating long-haul favourites from one-week bursts. All computed locally from
your own listen history — no external calls. Time-of-day and weekday axes (and
the listening heatmap) use your timezone, auto-detected from the browser and
editable in Settings; album-playthrough detection uses stored track numbers.

### Import & Export

Backfill your history from elsewhere and take it with you:

- **Import** public listens from ListenBrainz or Last.fm by username, or upload
  a ListenBrainz `.json` dump or a `.csv` export. Imports run as background
  jobs, are deduplicated against existing listens, and are stored so they are
  never re-forwarded to your scrobble sinks.
- **Export** your entire history as JSON (full dump) or CSV.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                       Tapedeck Binary                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────┐   ┌──────────────────┐                       │
│  │ Poll Sources  │   │  Ingest API      │                       │
│  │ (Plex)        │   │  (LB-compatible) │                       │
│  └──────┬───────┘   └────────┬─────────┘                       │
│         └────────────────────┤                                  │
│                              ▼                                  │
│                    ┌─────────────────┐                          │
│                    │  Scrobble Engine │                          │
│                    │  - Dedup         │                          │
│                    │  - MB Enrich     │                          │
│                    │  - Quality Score │                          │
│                    │  - Session Group │                          │
│                    └────────┬────────┘                          │
│                             │                                   │
│              ┌──────────────┼──────────────┐                   │
│              ▼              ▼              ▼                    │
│     ┌──────────────┐ ┌───────────┐ ┌────────────┐             │
│     │  SQLite      │ │  Sinks    │ │  REST API  │             │
│     │  (scrobbles, │ │  (Last.fm,│ │  (chains,  │             │
│     │   chains,    │ │   LB)     │ │   devices, │             │
│     │   devices,   │ │           │ │   gear)    │             │
│     │   sessions)  │ └───────────┘ └────────────┘             │
│     └──────────────┘                                           │
└─────────────────────────────────────────────────────────────────┘
```

## Installation

### Prerequisites

- Rust toolchain (1.75+)
- [Bun](https://bun.sh) (to build the embedded web UI)
- A Plex Media Server (optional — ingest API works standalone)
- A ListenBrainz / Last.fm / Libre.fm account (optional — connected per user in the UI)

### Build

The web UI is compiled into `static/` and then baked into the binary via
`rust-embed`, so it must be built **before** `cargo build`:

```bash
git clone https://codeberg.org/abksh/tapedeck.git
cd tapedeck
./build-web.sh            # bun install + vite build → static/  (required first)
cargo build --release
```

For frontend iteration with hot reload (proxies the API to `localhost:8080`):

```bash
cargo run                 # backend on :8080
cd web && bun run dev     # UI on :5173
```

## Configuration

**Almost nothing is configured in files.** Scrobble connections
(ListenBrainz / Last.fm / Libre.fm) are per user, set in *Settings → Scrobble
Connections*. Plex is set in *Settings → Plex* (admin-only) and applies within a
poll interval without a restart; "Test connection" verifies a URL/token pair by
listing the libraries it can see. `PLEX_*` variables and the `[sources.plex]`
TOML block **only seed the database on first run** — after that the database is
authoritative and editing them does nothing.

What is left in files is infrastructure that has to exist before the UI can be
served. Copy `.env.example` to `.env`, or `tapedeck.toml.example` to
`tapedeck.toml` (precedence: env > `.env` > `tapedeck.toml`):

```bash
# ── Server ──
PORT=8080
HOST=0.0.0.0
TRUST_PROXY=false

# ── Database ──
DATABASE_URL=sqlite:scrobbles.db
SQLITE_DB_PATH=./tapedeck.db

# ── Logging ──
RUST_LOG=info
LOG_DIR=./logs

# ── MusicBrainz ──
MUSICBRAINZ_USER_AGENT=Tapedeck/0.7.0 ( your-email@example.com )
MUSICBRAINZ_RATE_LIMIT=1

# ── ListenBrainz endpoint (only for a self-hosted instance) ──
# LISTENBRAINZ_URL=https://api.listenbrainz.org
```

There are **no server-wide scrobble credentials.** A listen is only ever
forwarded to accounts the listening user connected themselves; a user who has
connected nothing keeps their listens local. Tapedeck ships its own registered
Last.fm application key, so nobody needs to register one.

## Usage

```bash
./tapedeck
```

On first run, Tapedeck creates an admin user and prints an API token:

```
════════════════════════════════════════════════════════
🔑 Admin API token (save this — it won't be shown again!):
   td_a1b2c3d4e5f6...
════════════════════════════════════════════════════════
```

### Connecting Pano Scrobbler

1. Open Pano Scrobbler → Settings → Scrobble services
2. Add a custom ListenBrainz server
3. URL: `http://your-server:8080`
4. Token: your `td_xxx` token

This covers all your devices — Walkman, Android phone, desktop Linux via MPRIS.

### Managing Users

```bash
# Create a new user (returns a ready-to-use token)
curl -X POST http://localhost:8080/admin/users \
  -H "Authorization: Token td_your_admin_token" \
  -H "Content-Type: application/json" \
  -d '{"username": "partner", "display_name": "Partner"}'

# List users
curl http://localhost:8080/admin/users \
  -H "Authorization: Token td_your_admin_token"

# Create additional tokens
curl -X POST http://localhost:8080/admin/tokens \
  -H "Authorization: Token td_your_admin_token" \
  -d '{"user_id": 2, "name": "walkman"}'
```

### Managing Signal Chains

```bash
# Create a signal chain
curl -X POST http://localhost:8080/api/v1/chains \
  -H "Authorization: Token td_your_token" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Desktop Reference",
    "description": "Primary desktop listening setup",
    "listening_context": "active",
    "components": [
      {"type": "source", "name": "Fooyin", "detail": "Fedora Sway"},
      {"type": "dac", "name": "Schiit Mimir", "detail": "USB input"},
      {"type": "amp", "name": "Schiit Midgaard"},
      {"type": "transducer", "name": "Sennheiser HD 650", "detail": "balanced cable"}
    ]
  }'

# List your chains
curl http://localhost:8080/api/v1/chains \
  -H "Authorization: Token td_your_token"

# List equipment usage
curl http://localhost:8080/api/v1/equipment \
  -H "Authorization: Token td_your_token"
```

### Submitting Listens with Quality Data

```bash
curl -X POST http://localhost:8080/1/submit-listens \
  -H "Authorization: Token td_your_token" \
  -H "Content-Type: application/json" \
  -d '{
    "listen_type": "single",
    "payload": [{
      "listened_at": 1712000000,
      "track_metadata": {
        "artist_name": "Simon & Garfunkel",
        "track_name": "The Sound of Silence",
        "release_name": "Wednesday Morning, 3 A.M.",
        "additional_info": {
          "submission_client": "tapedeck-test",
          "duration_ms": 210000,
          "tapedeck_audio": {
            "format_type": "pcm",
            "codec": "FLAC",
            "sample_rate": 44100,
            "bit_depth": 16,
            "channels": 2,
            "is_lossless": true
          },
          "tapedeck_device": {
            "player_name": "Fooyin",
            "platform": "linux",
            "machine_id": "desktop-001"
          },
          "tapedeck_chain": {
            "chain_id": "desktop-reference"
          }
        }
      }
    }]
  }'
```

## API Reference

**Auth model.** API tokens (`Authorization: Token td_…`) can do exactly one
thing: **submit listens** to `/1/submit-listens`. Everything else — reading,
editing, deleting, and all admin actions — requires a logged-in **session**
(the `td_session` cookie from the web UI). This is enforced at the extractor
level, so a leaked device token can only *add* listens, never read or destroy
your data. The tables below mark `Token` only where tokens are accepted; `Auth`
means session-only.

### Ingest

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/artwork/status` | Auth | Cover-art coverage for the caller |
| POST | `/api/v1/artwork/backfill` | Admin | Resolve missing cover art in the background |
| GET | `/1/validate-token` | Token | LB token check (`{valid, user_name}`) |
| POST | `/1/submit-listens` | Token | LB listen submission |
| GET | `/1/user/:user/listens` | Token | LB listen history (`max_ts`/`min_ts`/`count`) |
| GET | `/1/user/:user/playing-now` | Token | LB now-playing |
| GET | `/1/user/:user/listen-count` | Token | LB total listen count |
| GET | `/1/user/:user/services` | Token | LB connected-services list |
| POST | `/1/playing-now/delete` | Token | Clear your now-playing |
| GET/POST | `/1/latest-import` | Token | LB import high-water mark |

### Admin

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/health` | None | Health check |
| GET/POST | `/log-level` | None | View/set runtime log level |
| GET | `/admin/users` | Admin | List users |
| POST | `/admin/users` | Admin | Create user (returns token) |
| PATCH | `/admin/users/{id}` | Admin | Change role / reset password |
| DELETE | `/admin/users/{id}` | Admin | Delete user + all their data |
| GET | `/admin/tokens` | Session | List your tokens |
| POST | `/admin/tokens` | Session | Create new token |
| DELETE | `/admin/tokens/{id}` | Session | Revoke a token |

### Auth

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/auth/status` | None | Whether setup is needed / session is valid |
| POST | `/api/v1/auth/setup` | None | First-run: create the admin user |
| POST | `/api/v1/auth/login` | None | Log in, set session cookie |
| POST | `/api/v1/auth/logout` | Session | Log out |
| GET | `/api/v1/auth/me` | Auth | Current user info |

### Scrobbles & Stats

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/scrobbles` | Auth | List listens (`limit`, `offset`, `artist`, `album`, `after`, `before`) |
| PATCH | `/api/v1/scrobbles/{id}` | Auth | Edit a listen (metadata, MBID, chain, format) |
| DELETE | `/api/v1/scrobbles/{id}` | Auth | Delete one listen |
| DELETE | `/api/v1/scrobbles?confirm=true` | Auth | Clear all listens for the user |
| GET | `/api/v1/stats/dashboard` | Auth | Dashboard summary counts |
| GET | `/api/v1/stats?range=` | Auth | Ranged analytics (`week`/`month`/`last30`/`year`/`all`) |
| GET | `/api/v1/personality?range=` | Auth | Five-axis listening personality + per-artist loyalty |
| GET | `/api/v1/languages/status` | Auth | Language-classification coverage |
| POST | `/api/v1/languages/backfill` | Admin | Run a full language-classification pass |

### Preferences, Export & Import

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET/PUT | `/api/v1/preferences` | Auth | Per-user settings (theme, …) |
| GET | `/api/v1/export?format=` | Auth | Listen dump — `json` or `csv` |
| GET | `/api/v1/backup` | Admin | Consistent full-database `.db` snapshot |
| GET | `/api/v1/connections` | Auth | Your connected scrobble services |
| PUT | `/api/v1/connections/listenbrainz` | Auth | Connect ListenBrainz (`{token}`) |
| POST | `/api/v1/connections/lastfm/start` | Auth | Begin Last.fm OAuth → `{auth_url, token}` |
| POST | `/api/v1/connections/lastfm/complete` | Auth | Finish Last.fm OAuth after approval |
| POST | `/api/v1/connections/librefm/start` | Auth | Begin Libre.fm OAuth → `{auth_url, token}` |
| POST | `/api/v1/connections/librefm/complete` | Auth | Finish Libre.fm OAuth after approval |
| GET/PUT | `/api/v1/sources/plex` | Admin | Read / update the Plex source settings |
| POST | `/api/v1/sources/plex/test` | Admin | Verify a Plex URL + token |
| GET | `/api/v1/now-playing` | Auth | What this user is playing right now |
| DELETE | `/api/v1/connections/{service}` | Auth | Disconnect a service |
| POST | `/api/v1/import/listenbrainz` | Auth | Import by ListenBrainz username |
| POST | `/api/v1/import/lastfm` | Auth | Import by Last.fm username |
| POST | `/api/v1/import/file` | Auth | Import an uploaded `.json`/`.csv` |
| GET | `/api/v1/import/jobs/{id}` | Auth | Import job progress |
| GET | `/api/v1/import/history` | Auth | Recent import jobs |

### Signal Chains & Gear

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/chains` | Session | List signal chains |
| POST | `/api/v1/chains` | Session | Create signal chain |
| GET | `/api/v1/chains/{id}` | Session | Get chain details |
| PATCH | `/api/v1/chains/{id}` | Session | Update a chain |
| DELETE | `/api/v1/chains/{id}` | Session | Delete a chain (unlinks listens, keeps them) |
| GET | `/api/v1/devices` | Session | List auto-discovered devices |
| GET | `/api/v1/equipment` | Session | List equipment with usage hours |
| POST | `/api/v1/equipment` | Session | Register equipment |
| PATCH | `/api/v1/equipment/{id}` | Session | Update equipment |
| DELETE | `/api/v1/equipment/{id}` | Session | Delete equipment |
| GET | `/api/v1/bindings` | Session | List output→chain bindings (mapped + auto-learned) |
| PUT | `/api/v1/bindings` | Session | Set/clear the chain for an output (`{identifier, chain_id}`) |
| DELETE | `/api/v1/bindings/{id}` | Session | Forget an output |
| POST | `/api/v1/scrobbles/bulk-chain` | Auth | Reassign many listens' chain (`{ids, signal_chain_id}`) |

### How a listen gets its chain

Attribution happens at the source, but a chain lives downstream — so the server
resolves the chain in layers (first match wins):

1. **Explicit chain name** — the client sends it in `tapedeck_chain.chain_id`
   (a name); the server looks it up.
2. **Output-device binding** — the client reports the OS output device in
   `tapedeck_device.output_device` (a USB DAC name, a Bluetooth device, "wired",
   …). Every distinct output is **auto-learned** and shown in the UI's *Outputs*
   panel; you map it to a chain **once**, and every future listen through it
   attributes automatically.
3. **Source-device default** — the machine's default chain, if set.

Once resolved, the chain tags the listen and its session, propagates the chain's
listening context, and accumulates hours. Ambiguous cases (one output feeding
two rigs) are fixed retroactively with History's bulk chain-reassign.

## Project Structure

```
src/
├── main.rs                  Init, wire everything, run
├── config.rs                Environment-based configuration
├── models.rs                Play, AudioQuality, SignalChain, Device, Session
├── error.rs                 thiserror enum
├── db.rs                    SQLite: all tables, migrations, CRUD
├── musicbrainz.rs           3-tier metadata client
├── logging.rs               tracing + file output
├── web_assets.rs            serves the embedded SvelteKit SPA
├── engine/
│   ├── pipeline.rs          ScrobbleEngine (poll → enrich → store → dispatch)
│   └── enrichment.rs        MusicBrainz enrichment helper
├── server/
│   ├── mod.rs               AppState, build_app()
│   ├── admin.rs             Health + log level endpoints
│   ├── auth.rs              Session/token auth extractor
│   ├── ingest.rs            /1/submit-listens (LB-compatible)
│   ├── models.rs            Request/response types + Tapedeck extensions
│   ├── users.rs             Auth, user + token management
│   ├── scrobbles.rs         Listen list/edit/delete + stats endpoints
│   ├── preferences.rs       Per-user settings (theme, …)
│   ├── export.rs            JSON/CSV backup export
│   ├── import.rs            ListenBrainz/Last.fm/file import + jobs
│   └── chains.rs            Signal chain, device, equipment endpoints
├── sinks/
│   ├── lastfm.rs            Last.fm scrobble sink
│   └── listenbrainz.rs      ListenBrainz scrobble + now-playing sink
└── sources/
    └── plex.rs              Plex polling source

web/                         SvelteKit SPA (built into static/, embedded)
```

## Roadmap

### Completed

- [x] **Phase 1** — Universal ingest API, multi-user, scrobble proxy
- [x] **Phase 2** — Signal chains, audio quality model, device tracking, sessions
- [x] **Phase 3** — Embedded SvelteKit UI (light/dark theme, quality badges, inline edit)
- [x] **Analytics** — Dashboard + ranged stats, top artists, listening heatmap
- [x] **Import / Export** — ListenBrainz/Last.fm/file import, JSON/CSV export
- [x] TOML configuration (env > `.env` > `tapedeck.toml`)

### Upcoming

- [ ] **Phase 4** — Materialized stats, listening patterns, year-in-review
- [ ] **Phase 5** — AI-powered features (recommendations, profile compression, smart corrections)
- [ ] **Phase 6** — Discord rich presence, outbound webhooks
- [ ] MusicBrainz-driven genre/language/region breakdowns in Analytics
- [ ] Docker container
- [ ] Jellyfin + Navidrome source support
- [ ] Plex audio quality extraction from session metadata
- [ ] Prometheus /metrics endpoint

## Performance

| Operation | Latency | Notes |
|-----------|---------|-------|
| SQLite MB cache hit | ~2ms | 85–95% hit rate |
| PostgreSQL MB lookup | ~30ms | Optional, offline-capable |
| MusicBrainz API | ~500ms | Rate-limited 1/s, cached |
| Ingest API (per listen) | <5ms | Excluding MB enrichment |
| Quality score computation | <1µs | In-memory |

## Contributing

```bash
cargo fmt
cargo clippy
cargo test
```

## License

See LICENSE file for details.

## Acknowledgments

- [Pano Scrobbler](https://github.com/kawaiiDango/pano-scrobbler) — inspiration and primary ingest client
- [MusicBrainz](https://musicbrainz.org) — comprehensive music metadata
- [ListenBrainz](https://listenbrainz.org) — open scrobble protocol
- Built with Rust's async ecosystem (tokio, axum, sqlx, reqwest)
