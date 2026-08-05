# 📼 Tapedeck

A self-hosted music intelligence hub written in Rust. Tapedeck ingests scrobble
data from any source — Plex, Navidrome, Jellyfin, Pano Scrobbler, web browsers,
phones, DAPs — enriches it with MusicBrainz metadata and audio quality context,
and stores a complete personal listening history with audiophile-grade analytics.

It also records the listening nothing can report automatically: put a side of
vinyl on, press start, and every track on it is logged with the pauses and skips
you actually made.

Single binary. Privacy-first. DSD is first-class.

## What makes Tapedeck different

Most scrobblers track *what* you listened to. Tapedeck tracks *how* — the full
signal chain from source file through DAC and amp to transducer, whether your
FLAC was degraded to SBC over Bluetooth, and how many hours are on your HD 650
drivers. It's a scrobbler built by and for someone who cares about the listening
experience, not just the listening history.

It is meant to become a **personal musical autobiography**: not only what you
played, but how you played it, what you thought of it at the time, and how your
taste and your equipment shaped each other. That framing decides arguments —
your own library beats any metadata provider, nothing recommends music you don't
own, and anything personal is editable by hand.

## Features

### Universal Ingest API

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

### Adding a listen by hand

Paste one ListenBrainz-shaped listen — or an array, or a whole `submit-listens`
body — on the Import screen and it goes in. It takes the same path an external
client's does, so it gets the same validation, deduplication and chain
resolution; there is no parallel code path free to drift from the real one.

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

### Signal Chain Intelligence

Define your audio signal chains — the ordered path from source to ears:

```
Desktop → Schiit Mimir (DAC) → Schiit Midgaard (Amp) → HD 650
```

Each chain carries a default listening context (active, passive, background).
When a scrobble arrives tagged with a chain, the context propagates
automatically. The gear choice reflects the intent — respect the gear,
respect the classification.

Components are picked from your equipment list rather than typed, so a piece of
gear is one record whichever chains it appears in. A step can still be free text
for something you don't own.

**You never have to tag a listen by hand.** A chain is resolved per listen, first
match winning: an explicit chain named by the client, then the output device it
came out of (auto-learned and mapped once), then **the chain the submitting API
token defaults to**, then the submitting machine's default. Issuing one token per
app — `fooyin`, `Pano on Android` — and giving each a chain is the whole setup:
the app itself needs no Tapedeck-specific configuration.

### Audio Quality Tracking

Every scrobble can carry audio quality metadata: codec, sample rate, bit depth,
DSD rate, and delivery format. Tapedeck computes a 0–100 quality score and
tracks source vs. delivered quality, so you know when your FLAC was degraded
to AAC over Bluetooth.

Quality scores:
- DSD128: 95 · DSD64: 92
- FLAC 24/192: 90 · FLAC 24/96: 85 · FLAC 16/44.1 (CD): 80
- LDAC 990kbps: ~77 · AAC 256: 55
- Penalties: −10 if transcoded, −3 to −15 for BT codec

### Equipment Usage Tracking

Track total hours on each piece of gear. Useful for driver burn-in tracking,
warranty records, and understanding how your listening habits distribute
across your equipment.

### Session Grouping

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

### Metadata & Artwork

MBIDs come from a 3-tier lookup:

1. SQLite cache (~2ms, 85–95% hit rate)
2. Optional PostgreSQL MusicBrainz dump (~30ms, 60–80% coverage)
3. MusicBrainz API fallback (~500ms, rate-limited)

Artwork comes from an ordered provider chain, cheapest and most accurate first:

1. **Your own library** — a listen polled from Plex, Navidrome or Jellyfin uses
   the cover of the exact file that was played. Zero external calls. The browser
   can't reach a LAN media server (and must never hold its credentials), so
   Tapedeck proxies the image itself at `/api/v1/art/:id`, scoped to the caller.
2. **Deezer** — no auth, no one-request-per-second ceiling.
3. **MusicBrainz → Cover Art Archive** — for what nobody else has.

A track with no artwork anywhere is tombstoned rather than re-asked forever, and
providers return **URLs, not bytes** (Deezer's terms discourage persisting
images). Toggle art off in Settings for lettered tiles.

### Navidrome

Signs in as *you* and pulls your play history — Navidrome keeps its own, so this
backfills listens from before Tapedeck existed and then keeps up live. Uses both
of Navidrome's APIs: the native one for history (`/api/scrobble/?from=`) and
Subsonic for now-playing. Your library's MusicBrainz IDs come along, so those
listens never need a MusicBrainz lookup.

Subsonic hashes `password + salt` on every request, so Navidrome needs the
account password rather than a token. It's encrypted at rest like every other
stored credential.

### Rockbox / DAP imports

Upload a `.scrobbler.log` straight off a Rockbox player. Track numbers, lengths
and any MusicBrainz IDs the player recorded all come across.

Skips (`S` rows) are **kept but never counted** — excluded from listen counts,
analytics and forwarding, so they can power skip-rate analytics later without
ever becoming a wrong scrobble on Last.fm.

If the log's header says `#TZ/UNKNOWN` — normal for a player with no time sync —
Tapedeck asks which timezone the device was set to rather than guessing. Assuming
the server's zone would shift the whole device's history by the offset, and it
can't be recovered afterwards.

### Plex and Jellyfin

Both are polled directly — no webhooks, no Plex Pass — with accurate scrobble
thresholds (50% played or 4 minutes) and live Now Playing on the dashboard.
Jellyfin keeps no play history of its own, so its listens start from when you
connect; Plex is the same.

**Sources belong to a user, not to the server.** Each person connects their own
Plex token or Jellyfin API key in Settings → Sources; Tapedeck resolves it to a
remote account and records only that account's playback. Nobody's credential can
see anyone else's listening, and there's no fragile matching of display names
against Tapedeck usernames.

**As many as you like, of any kind.** Two Plex servers, a Navidrome and a
Jellyfin can coexist on one account — each with its own name, poll interval and
filters. Polling is staggered, so several household members don't all hit the
same server on the same tick, and every listen records which source produced it.

Credentials Tapedeck has to replay — Plex tokens, Jellyfin API keys, the
Navidrome password, Last.fm session keys, ListenBrainz tokens — are encrypted at
rest with a key held outside the database
(`tapedeck.key`, or `TAPEDECK_SECRET_KEY`). That protects a leaked backup, which
is a real risk given `GET /api/v1/backup` hands out the whole database; it does
not protect against host compromise, and isn't claimed to.

### Web UI

A SvelteKit single-page app is embedded directly in the binary (via
`rust-embed`) — no separate deployment. The "cassette deck" interface ships
with:

- **Light / dark / system theme**, chosen per user. The choice is applied
  before first paint (no flash), cached in `localStorage`, and persisted
  server-side so every device matches.
- **Dashboard** — a cassette hero whose reels turn only while something is
  actually playing, with the artist and album clickable and a heart for the
  track on the deck; quick stats and recent listens.
- **History** — grouped by day with inline edit (fix title/artist/album,
  recording MBID, signal chain, format), per-listen delete, and **Inspect
  listen**: the stored row verbatim, every column including the ones no screen
  renders, with copy-to-clipboard and MusicBrainz links.
- **Album & artist pages** — aggregations over your own listens, a provider bio
  with its attribution, and a portrait from your library or one you set yourself.
- **Top Charts** and **Statistics** — timeframe-ranged totals, top artists, and
  a listening-clock heatmap. First day of the week is a preference (Monday by
  default; Friday if you think in Last.fm chart weeks).
- **Search** across everything you've played, resolving to the album or artist
  page rather than a filtered list.
- **Liner Notes** — write about a track, a record, an artist, or one particular
  playing; see below.
- **Vinyl & Tape** — the shelf, and a deck with pause, skip and flip reminders.
- **Rediscovery** — smart collections built from your own history.
- **Import** — pull from ListenBrainz or Last.fm, upload a `.json`/`.csv` export
  or a Rockbox `.scrobbler.log` with live progress, or paste a single listen.
- **Signal Chains & Equipment**, plus **Settings** — split into sections
  (Appearance, Sources, Connections, API Tokens, Data, Sanitiser, Maintenance,
  About), each deep-linkable as `/settings#sources`.

### Liner Notes

Notes are Markdown kept next to the music. A note is a **living document** about
one thing — a track, a release, an artist, or a single listen — edited in place,
with every earlier version archived, so a rewrite years later never erases what
you thought at the time.

Notes cross-reference each other with shortcodes:

```markdown
Bought on a whim in 2011. [[track:3]] is the one that got me — nothing else
by [[artist:Aphex Twin]] sounds like it, and [[track:Windowlicker]] came later.
```

`[[track:3]]` means the third track of the record you're annotating, and renders
with its real title. References resolve against things you've annotated *and*
against your own listen history, so they work for anything you've actually
played. A reference that matches nothing stays readable prose with a dotted
underline — never an error, because most of a real library is un-enriched and
those are exactly the records worth writing about.

**Love** works on a track, a release or an artist (not on a single playing of
one). Markdown is rendered and sanitised **server-side**, so notes are safe to
display anywhere and shortcode resolution happens in the same pass.

Notes carry a visibility (private / patched-in / public) for the coming social
phase. Today every read path is self-only: nothing is served to another user
whatever the setting says.

### Vinyl & Cassette

A shelf you catalogue and sides you play, for the listening nothing can report
automatically. Look a pressing up by **barcode**, catalogue number, or artist and
release — MusicBrainz always, Discogs first for a barcode when an admin has
configured a token, because it catalogues *pressings* and usually lands on the
exact edition.

Playing a side writes **one listen per track**, not one per side: every counting
query in Tapedeck is over listens, so a side-shaped row would be invisible to all
of them. Press **Start** when the needle drops and the platter turns at a real
33⅓ with a countdown; the browser runs the clock, so pause and skip are recorded
as they happen and each track lands where it actually played. Press *Already
played* instead and the running order is laid out backwards from the sleeve
times — right ordering and gaps, but no one saw the clock, and the UI says so.

A skipped track is stored and marked, so it stays out of every count and is never
forwarded — a skip on your Last.fm is a wrong scrobble on a permanent record.
Stylus hours and the cassette counter are **derived** from the sides played,
never accumulated, so they cannot drift from the plays they claim to count.

Sides are not reliably available from any provider — MusicBrainz records a vinyl
track number as "A1" only when an editor entered one — so Tapedeck splits by
**playing time** when it has to. A side holds around twenty minutes and a
mastering engineer balances them; splitting by track count would put a
nine-minute closer on the wrong side of the break. It's a guess, and the
tracklist is editable.

### Rediscovery

Smart collections built entirely from your own history. Nothing here recommends
music you don't own — that needs a corpus this app deliberately doesn't have.

Forgotten favourites (played properly once, not lately — the window is yours),
loved but barely played, one-listen records, what you played this time of year,
deep cuts, the 3 AM club (hour-of-day in *your* timezone), dusty vinyl, and
**never heard on a particular pair of headphones** — which is what
`scrobbles.chain_id` was always for: a chain names its components, so the
question is answerable with no per-listen equipment field at all.

### Musical Personality

The Statistics screen profiles *how* you listen across five explainable axes —
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

- **Import** the `.zip` from ListenBrainz's *Export your data* — listens **and**
  loved tracks, with the MusicBrainz IDs and cover-art references already in it,
  so a 20,000-listen history costs no lookups at all. Or pull public listens from
  ListenBrainz / Last.fm by username, or upload a single `.json`/`.jsonl`, a
  `.csv`, or a Rockbox `.scrobbler.log`. Imports run as background jobs, are
  deduplicated against existing listens, and are stored so they are never
  re-forwarded to your scrobble sinks.
- **Export** your entire history as JSON (full dump) or CSV.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                       Tapedeck Binary                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌───────────────────────┐   ┌──────────────────┐   ┌────────┐ │
│  │ Poll Sources          │   │  Ingest API      │   │ Import │ │
│  │ (Plex/Navidrome/      │   │  (LB-compatible) │   │ (LB/fm/│ │
│  │  Jellyfin, per user)  │   │                  │   │ files) │ │
│  └───────────┬───────────┘   └────────┬─────────┘   └───┬────┘ │
│              └────────────────────────┼─────────────────┘      │
│                                       ▼                        │
│                          ┌─────────────────────┐               │
│                          │  Scrobble Engine    │               │
│                          │  - Dedup            │               │
│                          │  - MB enrich        │               │
│                          │  - Artwork chain    │               │
│                          │  - Quality score    │               │
│                          │  - Session group    │               │
│                          └──────────┬──────────┘               │
│                                     │                          │
│              ┌──────────────────────┼──────────────────┐       │
│              ▼                      ▼                  ▼       │
│     ┌──────────────────┐   ┌────────────────┐  ┌────────────┐ │
│     │  SQLite          │   │  Sinks         │  │  REST API  │ │
│     │  (listens,       │   │  (per user:    │  │  + embedded│ │
│     │   entities/notes,│   │   Last.fm,     │  │    SPA     │ │
│     │   chains, gear,  │   │   Libre.fm,    │  │            │ │
│     │   sessions)      │   │   ListenBrainz)│  │            │ │
│     └──────────────────┘   └────────────────┘  └────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

## Installation

### Prerequisites

- Rust toolchain (1.75+)
- [Bun](https://bun.sh) (to build the embedded web UI)
- A Plex, Navidrome or Jellyfin server (optional — the ingest API works standalone)
- A ListenBrainz / Last.fm / Libre.fm account (optional — connected per user in the UI)

### Build

`cargo build` is the whole build. `build.rs` compiles the SvelteKit UI into
`static/` before rustc runs, and `rust-embed` bakes it into the binary:

```bash
git clone https://codeberg.org/abksh/tapedeck.git
cd tapedeck
cargo build --release
```

This means **bun is required for every cargo invocation**, including `cargo
check` and `cargo clippy`. For Rust-only work against an existing `static/`:

```bash
TAPEDECK_SKIP_WEB_BUILD=1 cargo check
```

For frontend iteration with hot reload (proxies the API to `localhost:8080`):

```bash
cargo run                 # backend on :8080
cd web && bun run dev     # UI on :5173
```

## Configuration

**Almost nothing is configured in files.** Scrobble connections
(ListenBrainz / Last.fm / Libre.fm) and media sources (Plex / Navidrome /
Jellyfin) are per user, set in *Settings → Connections* and *Settings → Sources*.
Source edits apply within a poll interval without a restart, and "Test
connection" verifies the credentials by naming the account they belong to.
`PLEX_*` variables and the `[sources.plex]` TOML block are legacy: they **only
seed the database on first run**, and the row is migrated onto the first admin as
a personal source. After that the database is authoritative and editing them does
nothing.

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
MUSICBRAINZ_USER_AGENT=Tapedeck/0.25.0 ( your-email@example.com )
MUSICBRAINZ_RATE_LIMIT=1

# ── ListenBrainz endpoint (only for a self-hosted instance) ──
# LISTENBRAINZ_URL=https://api.listenbrainz.org

# ── Optional service credentials ──
# All optional. Unconfigured is a normal state: the provider steps aside
# rather than erroring.
# TAPEDECK_METRICS_TOKEN=...     # bearer token for scraping /metrics
# TAPEDECK_DISCOGS_TOKEN=...     # barcode lookup; also settable in the UI
# TAPEDECK_APPLE_MUSIC_TOKEN=... # one provider in the artist/album bio chain
# TAPEDECK_SECRET_KEY=...        # credential encryption; generated on first run
#                                # into a keyfile if unset. Keep it OUT of
#                                # backups — a backup plus this key is the same
#                                # as storing credentials in the clear.
```

There are **no server-wide scrobble credentials.** A listen is only ever
forwarded to accounts the listening user connected themselves; a user who has
connected nothing keeps their listens local. Tapedeck ships its own registered
Last.fm application key, so nobody needs to register one.

## Usage

```bash
./tapedeck
```

On first run there is nothing to copy out of the console. Tapedeck creates the
admin row without a password and waits:

```
════════════════════════════════════════════════════════
📋 Visit the web UI to complete setup and set your password.
════════════════════════════════════════════════════════
```

Open `http://your-server:8080`, and the UI redirects you to `/setup` to name the
admin account and set a password. Tokens for scrobble clients are minted
afterwards in *Settings → API Tokens*.

Credentials are encrypted at rest with a key kept **outside** the database
(`./tapedeck.key`, generated on first run, or `TAPEDECK_SECRET_KEY`). Back it up
separately — and never inside a database snapshot, which would defeat the point.
Lose it and every stored token has to be reconnected.

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

# Create additional tokens — one per app, each with the chain it plays through
curl -X POST http://localhost:8080/admin/tokens \
  -H "Authorization: Token td_your_admin_token" \
  -d '{"user_id": 2, "name": "walkman", "default_chain_id": 3}'
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
      {"type": "transducer", "name": "Sennheiser HD 650", "equipment_id": 4}
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

### Sources & Ingest

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/sources` | Auth | The caller's own polling sources |
| POST | `/api/v1/sources` | Auth | Add one (`kind` = `plex`/`navidrome`/`jellyfin`) |
| PUT/DELETE | `/api/v1/sources/:id` | Auth | Update or remove one; an omitted credential keeps the stored one |
| POST | `/api/v1/sources/test` | Auth | Verify credentials before saving (`kind`) or re-test a stored source (`id`) |
| GET | `/api/v1/art/:scrobble_id` | Auth | Cover art proxied from the media server that produced the listen |
| GET | `/api/v1/artists/img/{id}` | Auth | Artist portrait, proxied from your own library |
| PUT | `/api/v1/artists/img/{id}` | Auth | Set the portrait by hand, or clear it |
| GET/POST | `/api/v1/sanitize` | Auth | Metadata sanitiser: preview, then apply the edits you ticked |
| GET | `/api/v1/resolve` | Auth | Name → entity id, creating it on first visit |
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
| GET | `/admin/tokens` | Session | List your tokens (with their default chain) |
| POST | `/admin/tokens` | Session | Create new token (`{name, scopes?, default_chain_id?}`) |
| PATCH | `/admin/tokens/{id}` | Session | Rename or repoint a token (`{name?, default_chain_id?}`) |
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
| GET | `/api/v1/scrobbles` | Auth | List listens (`limit`, `offset`, `artist`, `album`, `track`, `after`, `before`) |
| GET | `/api/v1/search` | Auth | Search your own history (`q`, `limit`) — grouped artists / albums / tracks |
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
| GET | `/api/v1/now-playing` | Auth | What this user is playing right now |
| DELETE | `/api/v1/connections/{service}` | Auth | Disconnect a service |
| POST | `/api/v1/import/listenbrainz` | Auth | Import by ListenBrainz username |
| POST | `/api/v1/import/lastfm` | Auth | Import by Last.fm username |
| POST | `/api/v1/import/file` | Auth | Import an uploaded `.json`/`.csv`/`.scrobbler.log` (multipart; send `timezone` with a `#TZ/UNKNOWN` log) |
| GET | `/api/v1/import/jobs/{id}` | Auth | Import job progress |
| GET | `/api/v1/import/history` | Auth | Recent import jobs |

### Liner Notes & Loves

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/notes` | Auth | Every note the caller has written |
| POST | `/api/v1/notes` | Auth | Write the note for a target, creating or editing it |
| GET/DELETE | `/api/v1/notes/{id}` | Auth | One note (with rendered HTML) / delete it and its revisions |
| GET | `/api/v1/notes/{id}/revisions` | Auth | Earlier versions, newest first |
| GET | `/api/v1/notes/for` | Auth | The note for a target, if any, plus the resolved target |
| POST | `/api/v1/notes/preview` | Auth | Render Markdown + shortcodes without saving |
| GET/POST | `/api/v1/loves` | Auth | List loves / love or un-love an entity |
| GET | `/api/v1/loves/keys` | Auth | Loved entity keys, so listen rows render a heart without a request each |
| GET | `/api/v1/albums/{id}` | Auth | Album page — your plays, tracklist, other records by that artist |
| GET | `/api/v1/artists/{id}` | Auth | Artist page — rank, top albums and tracks, plays by year |
| GET | `/api/v1/resolve` | Auth | Name → entity id (`kind`, `name`, `artist`), created on first use |
| GET | `/api/v1/sessions` | Auth | Listening sessions, newest first |
| GET | `/api/v1/sessions/{id}` | Auth | One session and the listens in it |
| PATCH | `/api/v1/sessions/{id}` | Auth | Retag the whole sitting (`chain_id`) or name it (`title`) |
| POST | `/api/v1/sessions/merge` | Auth | Fold sessions together (`{ids}`) — earliest survives |
| POST | `/api/v1/sessions/{id}/split` | Auth | Cut at a listen (`{at_scrobble_id}`) |
| POST | `/api/v1/sessions/rebuild` | Auth | Regroup all listens (`{gap_seconds}`) — discards hand edits |
| GET | `/api/v1/charts` | Auth | Ranked chart (`kind`=artists\|albums\|tracks, `range`, `limit`) |
| POST | `/api/v1/loves/pull` | Auth | Fetch loved tracks from every connected service |
| GET | `/metrics` | Admin/Token | Prometheus exposition. Bearer `TAPEDECK_METRICS_TOKEN`, or an **admin** session — every gauge is instance-wide |
| GET | `/api/v1/sanitize` | Auth | Preview metadata fixes (mixed album releases, featured credits) |
| POST | `/api/v1/sanitize` | Auth | Apply them — per-edit (`{album_edits, credit_edits, variant_edits}`) or whole-category (`{albums, artists, variants}`) |
| GET | `/api/v1/entities/{id}` | Auth | An entity with the caller's note and love state |
| GET | `/api/v1/scrobbles/{id}/annotations` | Auth | Notes and loves around one listen (read-only — creates nothing) |

A note target is one of: `entity_id`, `scrobble_id`, `from_scrobble` + `kind`
(the track / album / artist *of* that listen, created on demand), or `kind` +
`name` (+ `artist`) spelled out. `kind` is `track`, `album` or `artist`.

### Vinyl, Cassette & Rediscovery

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/physical` | Auth | The shelf; `?kind=vinyl\|cassette` |
| POST | `/api/v1/physical` | Auth | Add a record or tape |
| GET | `/api/v1/physical/{id}` | Auth | One release, with derived play counts |
| DELETE | `/api/v1/physical/{id}` | Auth | Remove it (listens are kept) |
| PUT | `/api/v1/physical/{id}/tracks` | Auth | Save a corrected side split |
| POST | `/api/v1/physical/{id}/play` | Auth | Record a side — one listen per track |
| GET | `/api/v1/physical/lookup` | Auth | `?barcode=` / `?catalog_number=` / `?artist=&release=` |
| GET | `/api/v1/physical/discogs/{id}` | Auth | One Discogs release, with tracklist |
| GET | `/api/v1/physical/equipment/{id}/wear` | Auth | Stylus hours and sides played |
| GET/PUT | `/api/v1/physical/settings/discogs` | Admin | Discogs token (never returned) |
| GET | `/api/v1/rediscovery` | Auth | All collections; `?quiet_days=` |
| GET | `/api/v1/rediscovery/gear/{id}` | Auth | Favourites never played through that gear |

`POST /physical/{id}/play` takes per-track outcomes from the client, which is the
only thing that saw the pauses and skips:

```json
{
  "side": "A",
  "started_at": 1785858587,
  "tracks": [
    { "position": 1, "started_at": 1785858587, "skipped": false },
    { "position": 2, "started_at": 1785858655, "skipped": true }
  ]
}
```

Omit `tracks` and the running order is derived from `started_at` — which can only
ever describe an uninterrupted side.

### Signal Chains & Gear

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/v1/chains` | Session | List signal chains |
| POST | `/api/v1/chains` | Session | Create signal chain |
| GET | `/api/v1/chains/{id}` | Session | Get chain details |
| PATCH | `/api/v1/chains/{id}` | Session | Update a chain |
| DELETE | `/api/v1/chains/{id}` | Session | Delete a chain (unlinks listens, keeps them) |
| GET | `/api/v1/devices` | Session | List auto-discovered devices |
| PATCH | `/api/v1/devices/{id}` | Session | Set/clear a device's default chain (`{chain_id}`) |
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
├── config.rs                Infrastructure config (env > .env > tapedeck.toml)
├── models.rs                Play, AudioQuality, SignalChain, Device, Session
├── db.rs                    SQLite: all tables, migrations, every query
├── crypto.rs                XChaCha20-Poly1305 credential encryption at rest
├── musicbrainz.rs           3-tier metadata client
├── enrich.rs                Background backfills (language, release year, artwork)
├── analytics.rs             Listening personality + per-artist loyalty
├── language.rs              Script/language classification
├── notes.rs                 Liner-note shortcodes + Markdown rendering
├── rockbox.rs               .scrobbler.log parsing (Audioscrobbler Log 1.1)
├── logging.rs               tracing + file output + live log level
├── web_assets.rs            serves the embedded SvelteKit SPA
├── engine/
│   ├── pipeline.rs          ScrobbleEngine (poll → enrich → store → dispatch)
│   └── enrichment.rs        MusicBrainz enrichment helper
├── metadata/
│   ├── mod.rs               ArtworkProvider trait + the ordered chain
│   ├── deezer.rs            Deezer artwork provider
│   ├── bio.rs               BioProvider chain (Last.fm → TheAudioDB → …)
│   └── release_lookup.rs    Pressing lookup for vinyl/tape (MusicBrainz, Discogs)
├── server/
│   ├── mod.rs               AppState, build_app()
│   ├── admin.rs             Health + log level endpoints
│   ├── auth.rs              Session/token auth extractor
│   ├── ingest.rs            /1/submit-listens, /1/validate-token
│   ├── lb_api.rs            The read side of the ListenBrainz Core API
│   ├── models.rs            Request/response types + Tapedeck extensions
│   ├── users.rs             Auth, user + token management
│   ├── scrobbles.rs         Listen list/edit/delete + stats endpoints
│   ├── notes.rs             Liner notes, loves, entities
│   ├── sources.rs           Per-user media sources (CRUD + test)
│   ├── artwork.rs           Cover-art proxy for library-sourced listens
│   ├── connections.rs       Last.fm / Libre.fm / ListenBrainz connect flows
│   ├── now_playing.rs       In-memory now-playing registry
│   ├── preferences.rs       Per-user settings (theme, timezone, …)
│   ├── export.rs            JSON/CSV export + full DB backup
│   ├── import.rs            ListenBrainz/Last.fm/file import + jobs
│   ├── ratelimit.rs         Login throttle
│   ├── chains.rs            Signal chain, device, equipment, binding endpoints
│   ├── library.rs           Album/artist pages, charts, entity resolve
│   ├── sessions.rs          Listening sessions: list, merge, split, rebuild
│   ├── physical.rs          Records and tapes: the shelf, and playing a side
│   ├── rediscovery.rs       Smart collections over your own history
│   ├── loves_pull.rs        Pull loves back from Last.fm / ListenBrainz
│   └── metrics.rs           Prometheus exposition (admin only)
├── sinks/
│   ├── mod.rs               sinks_for_user() — the only place sinks are built
│   ├── lastfm.rs            Last.fm sink (also drives Libre.fm)
│   └── listenbrainz.rs      ListenBrainz scrobble + now-playing sink
└── sources/
    ├── plex.rs              Plex polling source
    ├── navidrome.rs         Navidrome (native history + Subsonic)
    └── jellyfin.rs          Jellyfin session polling

web/                         SvelteKit SPA (built into static/, embedded)
deploy/                      Sample hardened systemd unit
```

## Roadmap

### Done

- [x] **Universal ingest** — ListenBrainz-compatible API, multi-user, per-user
      scrobble forwarding to Last.fm / Libre.fm / ListenBrainz
- [x] **Signal chains** — audio quality model, device tracking, sessions,
      output→chain bindings
- [x] **Embedded SvelteKit UI** — light/dark theme, inline edit, inspect listen
- [x] **Analytics** — ranged stats, top artists, listening heatmap, musical
      personality, language and decade breakdowns
- [x] **Import / export** — ListenBrainz, Last.fm, `.json`/`.csv`, Rockbox
      `.scrobbler.log`; JSON/CSV export and full-DB backup
- [x] **Per-user sources + credential encryption** — `user_sources` keyed by
      user, credentials encrypted at rest with an out-of-database key
- [x] **Rockbox / DAP import** — skips stored but never counted, device timezone
      asked for rather than assumed
- [x] **Navidrome source** — native history API + Subsonic
- [x] **Metadata providers** — artwork chain (your library → Deezer →
      MusicBrainz/CAA), artwork proxy, artist entities
- [x] **Jellyfin source**, and several sources of any kind per user
- [x] **Liner notes & loves** — Markdown notes with `[[track:…]]` shortcodes,
      revision history, notes on a listen as well as on a record
- [x] **Album and artist pages** — aggregations over your own listens, provider
      bios with attribution, artist portraits from your library or set by hand
- [x] **Metadata sanitiser** — per-edit review for mixed releases, featured
      credits, artist spellings and tracks credited two ways on one record
- [x] **Sessions as objects** — merge, split, rebuild; stable UUIDs
- [x] **Vinyl & cassette** — catalogue by barcode, catalogue number or name;
      play a side and it records every track on it, with a real 33⅓ platter,
      flip reminders, pause and skip, and derived stylus hours
- [x] **Rediscovery** — forgotten favourites, loved-but-unplayed, one-listen
      records, seasonal, deep cuts, the 3 AM club, dusty vinyl, and "never heard
      on this pair of headphones"
- [x] **`cargo build` builds everything** — `build.rs` compiles the UI, so a
      fresh clone compiles
- [x] **Prometheus `/metrics`** — admin-only, per-job import gauges
- [x] **Plex audio quality** from session metadata, plus device → signal chain

### Next

- [ ] **Patch (social)** — follow other users on the instance ("patch into
      someone's deck"), with follow state, stable actor URIs, and an activity
      log designed so federation later isn't a migration
- [ ] **ActivityPub** — actor endpoints, WebFinger, inbox/outbox, HTTP
      signatures. Open question first: ActivityStreams has no verb for
      "listened to a track", so the vocabulary has to be settled before the
      social schema freezes
- [ ] **Spotify source** — OAuth, `recently-played` backfill. Note its history
      endpoint reports when a track *ended*, while `scrobbles.timestamp` is
      start-of-play everywhere, so the duration has to be subtracted on read
- [ ] **Listening timeline** — monthly and yearly chapters, milestones,
      equipment history, "this day in music"
- [ ] Docker container
- [ ] Notes: full-text search, tags, and Markdown export for Obsidian/Logseq
- [ ] Mobile layout: the quality badge crowds the track title on narrow screens

### Known gaps

Things that look finished and aren't:

- **Discogs lookup is implemented but untested against the live API** — no token
  was available. The MusicBrainz path is fully verified.
- **Artist portraits from Navidrome** aren't wired. Plex and Jellyfin are;
  Subsonic can answer it, but an untested path that silently returns nothing is
  worse than a stated gap.
- **`listen_type: "import"` submitted through the ingest API is still
  forwarded** to your connected services. The *file* importer marks its rows
  `imported` and they are skipped; the ingest path has no equivalent yet. Don't
  build a backfill client against it until that lands.
- **Token scopes are stored and never enforced.** A token can submit listens but
  cannot read any `/api/v1/` route, so an external client can't offer a chain
  picker.

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
cargo test          # 119 tests
```

`static/` is gitignored but baked into the binary; `build.rs` builds it, so a
fresh clone compiles. `build-web.sh` still exists for rebuilding the UI without
invoking cargo.

The test suite is unit-level — pure functions and a few `sqlx` tests against a
temp database. There are **no HTTP-layer tests**, so handlers and anything
touching a remote service are verified by running the binary and hitting it with
`curl`. Where a remote API's shape matters, pin it against a real captured
response: several of this codebase's worst bugs were field-name mismatches that
compiled perfectly and silently produced `None` forever.

## License

See LICENSE file for details.

## Acknowledgments

- [Pano Scrobbler](https://github.com/kawaiiDango/pano-scrobbler) — inspiration and primary ingest client
- [MusicBrainz](https://musicbrainz.org) — comprehensive music metadata
- [ListenBrainz](https://listenbrainz.org) — open scrobble protocol
- [Cover Art Archive](https://coverartarchive.org) and [Deezer](https://developers.deezer.com) — artwork
- [Navidrome](https://www.navidrome.org), [Jellyfin](https://jellyfin.org) and [Rockbox](https://www.rockbox.org) — the other places listens come from
- Built with Rust's async ecosystem (tokio, axum, sqlx, reqwest)
