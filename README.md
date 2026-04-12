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
- A Plex Media Server (optional — ingest API works standalone)
- ListenBrainz token (optional)
- Last.fm API credentials (optional)

### Build

```bash
git clone https://github.com/abalajiksh/tapedeck.git
cd tapedeck
cargo build --release
```

## Configuration

Copy `.env.example` to `.env` and configure:

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

### Signal Chains & Gear

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/chains` | Token | List signal chains |
| POST | `/api/v1/chains` | Token | Create signal chain |
| GET | `/api/v1/chains/{id}` | Token | Get chain details |
| GET | `/api/v1/devices` | Token | List auto-discovered devices |
| GET | `/api/v1/equipment` | Token | List equipment with usage hours |
| POST | `/api/v1/equipment` | Token | Register equipment |

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
├── engine/
│   ├── pipeline.rs          ScrobbleEngine (poll → enrich → store → dispatch)
│   └── enrichment.rs        MusicBrainz enrichment helper
├── server/
│   ├── mod.rs               AppState, build_app()
│   ├── admin.rs             Health + log level endpoints
│   ├── auth.rs              Token auth extractor
│   ├── ingest.rs            /1/submit-listens (LB-compatible)
│   ├── models.rs            Request/response types + Tapedeck extensions
│   ├── users.rs             User + token management
│   └── chains.rs            Signal chain, device, equipment endpoints
├── sinks/
│   ├── lastfm.rs            Last.fm scrobble sink
│   └── listenbrainz.rs      ListenBrainz scrobble + now-playing sink
└── sources/
    └── plex.rs              Plex polling source
```

## Roadmap

### Completed

- [x] **Phase 1** — Universal ingest API, multi-user, scrobble proxy
- [x] **Phase 2** — Signal chains, audio quality model, device tracking, sessions
- [x] **Tech debt** — Engine extraction, thiserror, log→tracing, Box::leak fix

### Upcoming

- [ ] **Phase 3** — SvelteKit UI embedded in the binary (dark mode, album art, quality badges)
- [ ] **Phase 4** — Analytics engine (materialized stats, patterns, year-in-review)
- [ ] **Phase 5** — AI-powered features (recommendations, profile compression, smart corrections)
- [ ] **Phase 6** — Discord rich presence, outbound webhooks, import/export (Spotify, Rockbox)
- [ ] TOML configuration (replace .env)
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
