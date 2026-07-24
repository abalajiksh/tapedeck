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

Tapedeck exposes a ListenBrainz-compatible `/1/submit-listens` endpoint. Any
client that speaks the LB protocol works out of the box — Pano Scrobbler,
Web Scrobbler, multi-scrobbler, mpdscribble. Point it at your Tapedeck URL,
set your token, done.

Beyond the LB spec, Tapedeck accepts extended fields for audio quality,
device identification, signal chain tagging, and session metadata.

### Scrobble Proxy

Tapedeck stores listens locally *and* forwards them to Last.fm and
ListenBrainz simultaneously. You don't have to choose — Tapedeck sits in
front of both.

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
scrobble history, their own signal chains. Data is isolated per-user. First
run creates an admin user; additional users are created via the API.

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
- **Crash-safe.** Pending scrobbles live in the DB, so a crash or restart resumes
  where it left off. The poll loop isolates per-tick panics, and the server drains
  in-flight requests on `SIGTERM`.
- **Backups.** `GET /api/v1/backup` (or the Settings screen) downloads a
  consistent `VACUUM INTO` snapshot of the whole database, safe to take while
  running. `GET /health` reports DB connectivity for uptime monitoring.

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
  recording MBID, signal chain, format) and per-listen delete.
- **Analytics** — timeframe-ranged totals, top artists, and a listening-clock
  heatmap.
- **Import** — pull from ListenBrainz or Last.fm, or upload a `.json`/`.csv`
  export, with live progress.
- **Signal Chains & Equipment**, plus **Settings** for appearance, API tokens,
  connection test, and data export / wipe.

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
- ListenBrainz token (optional)
- Last.fm API credentials (optional)

### Build

The web UI is compiled into `static/` and then baked into the binary via
`rust-embed`, so it must be built **before** `cargo build`:

```bash
git clone https://github.com/abalajiksh/tapedeck.git
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

Configure via environment variables, a `.env` file, or `tapedeck.toml`
(precedence: env > `.env` > `tapedeck.toml`). Copy `.env.example` to `.env`, or
`tapedeck.toml.example` to `tapedeck.toml`:

```bash
# ── General ──
RUST_LOG=info
PORT=8080

# ── Database ──
SQLITE_DB_PATH=./tapedeck.db
DATABASE_URL=sqlite:scrobbles.db

# ── MusicBrainz ──
CONTACT_EMAIL=your-email@example.com
MUSICBRAINZ_RATE_LIMIT=1

# ── Plex (optional) ──
PLEX_ENABLED=true
PLEX_URL=http://192.168.1.X:32400
PLEX_TOKEN=your-plex-token

# ── ListenBrainz (optional) ──
LISTENBRAINZ_ENABLED=true
LISTENBRAINZ_TOKEN=your-lb-token

# ── Last.fm (optional) ──
LASTFM_ENABLED=true
LASTFM_API_KEY=your-api-key
LASTFM_SECRET=your-secret
LASTFM_SESSION_KEY=your-session-key

# ── Filtering ──
PLEX_USERS_ALLOW=myusername
PLEX_LIBRARIES_ALLOW=music
```

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

Auth accepts either an `Authorization: Token td_…` header (external clients) or
the `td_session` cookie set by the web UI login.

### Ingest

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/1/submit-listens` | Token | LB-compatible listen submission |

### Admin

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/health` | None | Health check |
| GET/POST | `/log-level` | None | View/set runtime log level |
| GET | `/admin/users` | Token | List users |
| POST | `/admin/users` | Token | Create user (returns token) |
| GET | `/admin/tokens` | Token | List your tokens |
| POST | `/admin/tokens` | Token | Create new token |
| DELETE | `/admin/tokens/{id}` | Token | Revoke a token |

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

### Preferences, Export & Import

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET/PUT | `/api/v1/preferences` | Auth | Per-user settings (theme, …) |
| GET | `/api/v1/export?format=` | Auth | Listen dump — `json` or `csv` |
| GET | `/api/v1/backup` | Admin | Consistent full-database `.db` snapshot |
| POST | `/api/v1/import/listenbrainz` | Auth | Import by ListenBrainz username |
| POST | `/api/v1/import/lastfm` | Auth | Import by Last.fm username (needs `LASTFM_API_KEY`) |
| POST | `/api/v1/import/file` | Auth | Import an uploaded `.json`/`.csv` |
| GET | `/api/v1/import/jobs/{id}` | Auth | Import job progress |
| GET | `/api/v1/import/history` | Auth | Recent import jobs |

### Signal Chains & Gear

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/chains` | Token | List signal chains |
| POST | `/api/v1/chains` | Token | Create signal chain |
| GET | `/api/v1/chains/{id}` | Token | Get chain details |
| PATCH | `/api/v1/chains/{id}` | Token | Update a chain |
| DELETE | `/api/v1/chains/{id}` | Token | Delete a chain (unlinks listens, keeps them) |
| GET | `/api/v1/devices` | Token | List auto-discovered devices |
| GET | `/api/v1/equipment` | Token | List equipment with usage hours |
| POST | `/api/v1/equipment` | Token | Register equipment |
| PATCH | `/api/v1/equipment/{id}` | Token | Update equipment |
| DELETE | `/api/v1/equipment/{id}` | Token | Delete equipment |
| GET | `/api/v1/bindings` | Token | List output→chain bindings (mapped + auto-learned) |
| PUT | `/api/v1/bindings` | Token | Set/clear the chain for an output (`{identifier, chain_id}`) |
| DELETE | `/api/v1/bindings/{id}` | Token | Forget an output |
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
