# 📼 Tapedeck

A self-hosted music intelligence hub written in Rust. Tapedeck ingests scrobble
data from any source — Plex, Navidrome, Jellyfin, Emby, Roon, Pano Scrobbler, web browsers,
phones, DAPs — enriches it with MusicBrainz metadata and audio quality context,
and stores a complete personal listening history with audiophile-grade analytics.

It also records the listening nothing can report automatically: put a side of
vinyl on — or a tape, or a disc — press start, and every track is logged with
the pauses and skips you actually made.

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

`additional_info` is free-form in ListenBrainz, and real clients disagree about
what goes in it, so Tapedeck reads it forgivingly. A track number may be a
number or a string, `"7"` or `"7/12"`; anything unusable costs the track number
and never the listen, because a whole batch failing over one cosmetic field is
the worst possible trade. MusicBrainz IDs are read from where a *submitting*
client puts them as well as from the `mbid_mapping` an export carries, so a
client that already identified the recording isn't charged a lookup for it.

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

**Listens marked as an import are stored but not forwarded**, because an import
normally means a chunk of old history and pushing that onward rewrites a
permanent public record. Some players label *every* scrobble that way — fooyin
does — so their listens arrive and go no further, which looks like Tapedeck
ignoring them. Tick **Relay listens** on that client's token in Settings → API
Tokens and they are forwarded like any other. Only listens from the last day
qualify even then, so a genuine backfill from the same app still stays local:
the setting says how the client labels things, and cannot be used to overwrite
your history.

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

Hours are **derived from your listens**, not accumulated as they arrive — a chain
sums the running time of every listen that carries it, and a piece of gear sums
the chains that name it. So assigning a chain to a device, or fixing a listen's
chain by hand, moves the hours for listens you already have rather than only for
future ones. Skips don't count: crediting a pair of headphones with a track's
full running time for three seconds of audio is not a measurement.

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

### Plex, Jellyfin and Emby

All three are polled directly — no webhooks, no Plex Pass — with accurate scrobble
thresholds (50% played or 4 minutes) and live Now Playing on the dashboard.
Jellyfin keeps no play history of its own, so its listens start from when you
connect; Plex and Emby are the same.

**Emby is untested.** Jellyfin is a 2018 fork of Emby, so they are one protocol
with a `/emby/` prefix on it and Tapedeck drives both from one client. But Emby
has developed independently for seven years, and this source has never run
against a real Emby server — it is built from a working third-party reference
and Emby's published API docs. The fields at risk fail *silently* rather than
erroring: if Emby spells them differently you get listens with no codec, no
device (and therefore no signal chain) and no MusicBrainz IDs, with nothing in
the log to say so. If you run Emby, that is what to check first, and a captured
`/emby/Sessions` response is what would close it for good.

### Roon

Roon is a source of its own rather than a network-streamer driver: one Core owns
many zones, which makes it server-shaped like Plex. It is the only way to see
**RAAT** playback, which no other protocol exposes.

Two things to know before relying on it. **Roon's extension API carries no
signal path** — the Lossless / High Quality / Enhanced verdict and every stage
behind it stay inside Roon's own client, so those listens resolve their chain
from the endpoint's default like any other. And it carries **no MusicBrainz IDs
and no structured metadata**, only rendered display strings, so a Roon listen
holds *less* detail than a Plex or Jellyfin one and pays for a metadata lookup
it could have skipped. That is the opposite of what most people expect, and it
is a limit of the API rather than something Tapedeck can work around.

Setup has one step nothing else does: after saving the source, open **Roon →
Settings → Extensions** and enable Tapedeck. Roon issues its own credential at
that point, so there is nothing to paste; Tapedeck connects by itself once you
approve, with no restart.

**Roon is also untested** — built against the `roon-api` crate's declared types
and Roon's own vendored API docs, never run against a real Core.

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
- **Top Charts** and **Statistics** — timeframe-ranged totals, top artists,
  records and tracks side by side, and a listening-clock heatmap. Every figure
  that is drawn from a narrower population than "everything you played" says
  so: a lossless share is a fact about the listens whose format is known, and
  where that is too little of your history the card reports its coverage
  instead of a number rather than reading as a verdict on your library. First
  day of the week is a preference (Monday by default; Friday if you think in
  Last.fm chart weeks).
- **Search** across everything you've played, resolving to the album or artist
  page rather than a filtered list.
- **Liner Notes** — write about a track, a record, an artist, or one particular
  playing; see below.
- **The Shelf** — records, tapes and discs, with a deck drawn for whichever is
  in hand: pause, skip and flip reminders.
- **Loved** — every heart in one place, across tracks, records and artists.
- **Reports** — your listening in chapters, one week, month or year at a time.
- **Rediscovery** — smart collections built from your own history.
- **Playlist Lab** — build a playlist by sound and habit from a seed you already
  play, and push it to your own server.
- **Import** — pull from ListenBrainz or Last.fm, drop in a ListenBrainz or
  Spotify export `.zip`, upload a `.json`/`.csv` or a Rockbox `.scrobbler.log`
  with live progress, or paste a single listen.
- **Genre Map** — your listening placed on Every Noise at Once, with a taste
  trajectory over time and the regions bordering what you already play.
- **Signal Chains & Equipment**, plus **Settings** — split into sections
  (Appearance, Time, Sources, Connections, API Tokens, Data, Sanitiser,
  Maintenance, Genre Map, About), each deep-linkable as `/settings#sources`.

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

The notes list is **searchable** across both what you wrote and what you wrote it
about, filterable by track / record / artist / listen, and sortable by edit, by
date written or alphabetically. A collapsible legend on the editor covers every
shortcode, including the two that were previously undiscoverable —
`[[gear:Bifrost 2]]` and positional `[[track:3]]`.

### The Shelf — records, tapes and discs

A shelf you catalogue and sides you play, for the listening nothing can report
automatically. Vinyl, cassette, CD and SACD. Look a pressing up by **barcode**, catalogue number, or artist and
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
nine-minute closer on the wrong side of the break. A multi-disc set is split
**one record at a time**, by each disc's own running time, so a double LP whose
first record runs long does not have side C opening with tracks that are
physically on disc 1 — and the panel groups its sides under *Record 1* and
*Record 2*.

**Running times are often missing, and Tapedeck says so rather than guessing.**
Discogs frequently lists none, and a tracklist of blanks used to be filled in at
three minutes a track and the sum shown as a side length — so an 18:00 side
"finished" while the record was still playing. Now a side with unknown running
times shows no length and no progress bar, the platter keeps turning, and its
listens carry no duration, so they add nothing to your gear hours rather than a
made-up figure. **Find running times** looks the pressing up by barcode, matches
tracks by title (not by position — what comes back is usually a different
pressing), shows you what it found beside what you have, and writes only when
you accept. Applying corrects the tracklist, the listens already recorded, and
the wear those plays banked. A running time you already have is never
overwritten.

You can also play **just one track**: drop the needle on a single song and
nothing after it is recorded. It still counts as wear — the needle was in the
groove — and it is marked partial, so it does not read as a whole side you never
played.

A **disc is the smaller problem**, and none of that guessing applies: a CD or
SACD plays start to finish, so its tracks come off the disc as one numbered
unit and the UI says *Disc 1* rather than *Side A*. Which rule applies is
decided by the medium's own format string, not by what you searched for. This
is also why discs are a shelf format rather than a source: no disc player of
any brand or vintage will tell you the artist and title of what it is playing,
but the shelf already holds the tracklist — so it works today with no driver at
all.

The deck is drawn for the medium in hand. A turntable's arm swings down onto
the record when a side starts and lifts when it stops; a cassette turns both
reels behind the window; a disc spins at its own much faster rate with a sheen,
because the silhouette is the same circle as a record. All of it stops dead
when the deck is paused, and none of it moves under `prefers-reduced-motion`.

### Loved

Every heart in one place — tracks, records *and* artists, since a love in
Tapedeck attaches to an entity rather than only to a song. Search it, sort by
when you loved it / most played / title / artist (each sort brings the divider
it implies), and filter by kind or to the ones carrying a liner note.

A love of a **recording** is mirrored outward to Last.fm and ListenBrainz;
albums and artists stay local because neither service has the concept. Loves
you already have on those services can be pulled in from Settings → Connections.

### Reports

Your listening in chapters — one week, month or year at a time, with a stepper
to walk back through the whole history. The three are genuinely different
shapes rather than one card resized: a month gets the full treatment (clock,
genres, on repeat, off the shelf), a week the compact one (seven days make a
thin hour-of-day distribution, so it isn't drawn), and a year is a bento of
tiles built around a twelve-month line with the prior year behind it and
genres as stacked bands.

Everything is folded from **one pass** over the history, bucketed in your own
timezone. There is no "you out-listened 88% of listeners" — that needs other
people's listening, and on a household instance it quietly reports how much
everyone else plays. Where the design put a percentile, Tapedeck ranks the
period against *your own*: "your 2nd busiest month of 8".

### Rediscovery

Smart collections built entirely from your own history. Nothing here recommends
music you don't own — that needs a corpus this app deliberately doesn't have.

Forgotten favourites (played properly once, not lately — the window is yours),
loved but barely played, one-listen records, what you played this time of year,
deep cuts, the 3 AM club (hour-of-day in *your* timezone), dusty vinyl, and
**never heard on a particular pair of headphones** — which is what
`scrobbles.chain_id` was always for: a chain names its components, so the
question is answerable with no per-listen equipment field at all.

**Send any of them to your server.** Tapedeck has no playback and isn't going to
grow any, so every collection carries a *Send to…* button that creates the
playlist on the Plex, Jellyfin or OpenSubsonic/Navidrome server you already
connected — no extra credential, and the options are exactly the servers you set
up. Tracks are matched by artist and title and every match is checked, so a
fuzzy search result for the wrong artist is discarded rather than added; a
library that doesn't have everything gets what it has, and the tracks that
didn't land are named.

### Playlist Lab

Grow a playlist by moving through the space your listening actually lives in.
Three ways: **similar songs** (nearest neighbours of your seeds), **dimensional
crawl** (a path traced between two or more seeds, one step at a time) and
**rollercoaster** (a trajectory that climbs to peaks and dives to lows along
whichever axis you pick). A phase-space plot shows any two of the ten
dimensions, with your playlist drawn on it.

**What the ten dimensions are, precisely.** Six come from the Every Noise genre
map, reached through each track's artist — dense/atmospheric ↔ spiky/bouncy,
organic ↔ mechanical/electric, the three colour channels (Every Noise encodes
audio character in them), and how far apart a track's own genres sit. Four come
from your own listening: era, familiarity, hour of day, recency. A *Judge by*
slider weights the two halves, because without it "similar" drifts toward
"played at the same hour".

**This is not a per-recording audio analysis, and nothing here pretends
otherwise.** Genres attach to artists, so every track by one artist shares its
six sound coordinates; the listening axes are what separate them. Tracks whose
genres have no place on the map are dropped rather than parked in the middle,
and the count is reported. Everything is built from music you already own.

Needs the genre map fetched (Settings → Genre Map — one request) and the genre
backfill to have run. Finished playlists export as M3U, JSON or plain text, or
go straight to your Plex, Jellyfin or Navidrome with **Send to…**.

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
- **Import your Spotify extended streaming history** — the `.zip` from
  *Download your data*, dropped in whole. Three things in that format are wrong
  in ways nothing errors on, and Tapedeck corrects all three: its `ts` is when
  the track **stopped**, so every listen is re-anchored to when it started
  (importing it raw puts 5% of listens in the wrong hour and breaks dedup
  against a history you already hold); track lengths are recovered from the
  plays that ran to the end, filling a duration for ~93% of rows; and Spotify's
  own `skipped` flag means "didn't reach the end", which over a third of the
  time is a track you played most of the way through, so disposition is decided
  by the same threshold every other source uses. Music videos come in with the
  music; podcasts and audiobooks are dropped and counted rather than silently
  swallowed. Unlike the ListenBrainz archive this carries no MusicBrainz IDs at
  all, so a large history has real enrichment work to do afterwards — the
  Import screen says so rather than letting it look broken.
- **Export** your entire history as JSON (full dump) or CSV.
- **Export your liner notes as Markdown** — a folder of one file per note with
  YAML frontmatter, or a single document. Drop it in an Obsidian or Logseq vault
  and your `[[track:…]]` references arrive as working `[[wikilinks]]`; a
  reference that never resolved is left exactly as you wrote it. Nothing in the
  export needs Tapedeck running to be readable.

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
- A Plex, Navidrome, Jellyfin, Emby or Roon server (optional — the ingest API works standalone)
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

### Connecting fooyin

1. fooyin → Settings → Integrations → Scrobbling → Services
2. Add a custom ListenBrainz service
3. URL: `http://your-server:8080`
4. Token: your `td_xxx` token

fooyin labels every scrobble it sends as an import, so its listens will arrive
in Tapedeck but not be forwarded to Last.fm or ListenBrainz until you tick
**Relay listens** on that token in Settings → API Tokens. Leave it off if
fooyin is already scrobbling to those services directly, or you'll get each
listen twice.

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

## Ask a chatbot about your listening

Tapedeck speaks the **Model Context Protocol**, so an AI assistant — running on
your own machine or hosted — can read your history and hand playlists back. The
server runs for every user on the instance, and reaches nobody's data until that
person authorises a connection.

**Two ways in.** For a local client, mint a credential in Settings → AI
Connections and point it at `https://your-tapedeck/mcp`. For a hosted one, give
it the same address and approve the OAuth request when it appears.

**You choose what it can see.** Six grants, all off by default:

| Grant | What it allows |
|---|---|
| `listening:read` | History, statistics, reports, sessions, the genre map |
| `library:read` | Gear, signal chains, the shelf, loves, **liner notes** |
| `playlists:write` | Save playlists into Tapedeck for you to review |
| `playlists:push` | Send them on to your media server |
| `loves:write` | Love and unlove |
| `notes:write` | Write liner notes, recorded as written by that connection |

Your statistics and your journal are separate grants on purpose.

**Twenty-three tools**, all of them the same queries the web UI uses. The one to
know about is `listening_profile`: it reports how much of your history carries
each kind of metadata, and the assistant is told to call it first. That is what
stops "4% lossless" being said about a history where the format is simply
unknown for 97% of the listens — the number that would otherwise measure your
import rather than your listening.

A playlist an assistant builds is **saved into Tapedeck first**, on the
Playlists page, and reaches a media server only when you send it. Every call any
connection makes is recorded — which tool, when, with what, and how many rows
came back — and a connection can be revoked at any time.

Tapedeck also tells the assistant what *not* to do: never suggest music you do
not own, never quote a share without its population, and never treat a genre
position as finer than it is (genres attach to artists, so every track by an
artist sits at the same point).

**Tested against**: a **Claude Pro** subscription (hosted, over OAuth) and
**Mistral Chat on the free tier**. Both drive the same endpoint — one after the
`initialize` handshake the older revisions use, one on the stateless 2026-07-28
shape — which is the pair the dual-era handling exists for. Any other client
speaking MCP should work; none has been tried.

## What you're listening to, on your own site

Everything else in Tapedeck answers only to you. There is one exception, and it
is off until you switch it on: **`GET /public/np/<username>`** returns whatever
is on your deck right now, with no credential.

It exists because a static site renders long before anyone visits it, so a live
"now playing" badge has to be fetched from the *reader's* browser — where none of
your credentials can safely go.

Settings → **Sharing** turns it on and decides what it carries. Title, artist and
record are the whole of it by default; the cover, how far in you are, the audio
format and the signal chain are four separate switches, each off until you say
otherwise. A progress bar tells people when you paused, and a chain says what is
in your listening room, so they are four decisions rather than one.

Two things it will not do. It **never reads your history** — the deck is one
track held in memory, and there is no route from it to a listen, a count, a note
or a love. And a username that does not exist and one that has not published
return the *same* 404, so nobody can use it to find out who has an account on
your instance.

A **Hugo shortcode** ships in `integrations/hugo/`. Drop it in your site and:

```
{{< tapedeck-now-playing src="https://tapedeck.example.com/public/np/you" >}}
```

It degrades to nothing — a server that is down, a deck with nothing on it, a
mistyped name all render an empty element rather than a broken box on your
homepage. An estimated playhead is drawn faded rather than as a confident bar,
because a scrobble client that reports no position is guessed at rather than
measured, and that distinction should survive onto your website.

## API

The whole API is described by **`openapi.yaml`** at the repo root, and every
running instance serves its own copy at:

```
GET /api/openapi.yaml
```

That endpoint is deliberately unauthenticated — it documents shapes, not data,
and a client has to read it *before* it has a credential, since the login and
device-pairing flows are in it. Serving it from the binary is the one thing a
separately hosted docs site cannot do: it describes **that** instance at **that**
version, which for a self-hosted app beats any version dropdown.

A drift test walks every `.route("…")` in the source and compares both
directions — routed but undocumented, and documented but not routed — so the
build fails rather than sending you to an endpoint that 404s.

Endpoint tables used to live here and were removed: a second copy of the API,
maintained by hand, is a copy that goes stale. The spec is the one to read.

**Auth model.** Two credentials, and they are not interchangeable.

A **session** (the `td_session` cookie from the web UI) can do everything.

An **API token** (`Authorization: Token td_…`) carries explicit scopes, and
none of them implies another:

| Scope | What it allows |
|-------|----------------|
| `submit` | `POST /1/submit-listens` and nothing else. What every scrobble client holds. |
| `read` | History, stats, loves, notes, now-playing, chains, gear. |
| `write` | Loves, notes, and editing or deleting a listen. |
| `all` | `read` + `write`. |

`submit` deliberately does not imply `read`: the token in a scrobble client on
a phone that gets lost must not be able to read the history it is appending to.
Matching is exact, so a scope of `rewrite` does not grant `write`.

A valid token missing the needed scope gets **403, not 401** — it authenticated
fine, and a 401 would send a client into a refresh loop it cannot win.

**Permanently session-only**, whatever scopes a token has: everything under
`/admin/`, plus sources, service connections, Discogs settings, export and
backup, the metadata sanitiser, the enrichment jobs, and
`DELETE /api/v1/scrobbles` (clear everything) — the one listen operation a lost
phone could make unrecoverable.

## How a listen gets its chain

Attribution happens at the source, but a chain lives downstream — so the server
resolves the chain on a **four-rung ladder**, first match winning. The order
encodes what each rung *is*: a binding is evidence about this listen, a token is
a standing statement about one app, a device is the coarsest fallback.

1. **Explicit chain name** — the client sends it in `tapedeck_chain.chain_id`
   (a name); the server looks it up.
2. **Output-device binding** — the client reports the OS output device in
   `tapedeck_device.output_device` (a USB DAC name, a Bluetooth device, "wired",
   …). Every distinct output is **auto-learned** and shown in the UI's *Outputs*
   panel; you map it to a chain **once**, and every future listen through it
   attributes automatically.
3. **The submitting token's default chain** — one token per app ("fooyin",
   "Pano on Android") is how people actually organise these, so a token can
   carry a chain and attribution costs the client no configuration at all. It
   can be repointed later **without re-issuing the token**, so fixing a chain
   never means pasting a new credential into a player.
4. **Source-device default** — the machine's default chain, if set.

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
├── openapi.rs               Serves openapi.yaml, plus the router drift test
├── rockbox.rs               .scrobbler.log parsing (Audioscrobbler Log 1.1)
├── spotify.rs               Spotify extended streaming history (pure, tested)
├── playlist_lab.rs          The ten-axis space a playlist is generated in
├── playlists.rs             Track matching + the push-to-server target trait
├── logging.rs               tracing + file output + live log level
├── web_assets.rs            serves the embedded SvelteKit SPA
├── engine/
│   ├── pipeline.rs          ScrobbleEngine (poll → enrich → store → dispatch)
│   └── enrichment.rs        MusicBrainz enrichment helper
├── metadata/
│   ├── mod.rs               ArtworkProvider trait + the ordered chain
│   ├── deezer.rs            Deezer artwork provider
│   ├── bio.rs               BioProvider chain (Last.fm → TheAudioDB → …)
│   └── release_lookup.rs    Pressing lookup for the shelf (MusicBrainz, Discogs)
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
│   ├── physical.rs          The shelf — records, tapes and discs — and playing one
│   ├── rediscovery.rs       Smart collections over your own history
│   ├── playlist_lab.rs      Serves the playlist space for the browser to work in
│   ├── playlists.rs         Push a generated list to Plex / Jellyfin / Subsonic
│   ├── reports.rs           Week/month/year chapters, one pass over the history
│   ├── everynoise.rs        The Every Noise genre map and the genre-map views
│   ├── pairing.rs           Device-code pairing for browserless clients
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

src/everynoise/              Every Noise map: parse, store, scrape
openapi.yaml                 The whole API; served at /api/openapi.yaml
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
- [x] **Genre map** — Every Noise at Once scraped into its own database, your
      listening placed on it, plus taste trajectory, neighbourhoods and two
      parameter-free breadth numbers
- [x] **Scoped API tokens** — `submit` / `read` / `write` / `all`, none
      implying another, with `/admin/` and anything destructive permanently
      session-only
- [x] **OpenAPI spec** — the whole API in `openapi.yaml`, served by the binary
      at `/api/openapi.yaml`, with a drift test in both directions
- [x] **Device-code pairing** — RFC 8628-shaped, for clients that cannot show a
      browser
- [x] **Reports** — week, month and year chapters with a stepper through the
      whole history, each a different layout rather than one card resized
- [x] **Loved** — every heart in one place, across tracks, records and artists
- [x] **Discs on the shelf** — CD and SACD alongside vinyl and cassette, with a
      deck drawn for the medium in hand
- [x] **Playlist Lab and pushing playlists out** — build a list from your own
      history, then send it to Plex, Jellyfin, Emby, Subsonic/Navidrome or
      TIDAL. A partial match is normal and every track that did not land is
      named. **TIDAL's write half is unverified** — sign-in, search and the
      token refresh are confirmed against the live API, but creating a playlist
      needs a completed user sign-in and the two `POST` shapes are still the
      ones TIDAL's spec declares rather than ones observed working
- [x] **Chain variants** — one chain with a step swapped, so two cartridges on
      one turntable each accrue their own hours and the other wears nothing
- [x] **Your own account** — change your own password (the current one is
      required), display name, bio and picture, without an admin
- [x] **MCP server** — an AI assistant can read your listening and hand
      playlists back, over six grants that are all off by default
- [x] **The Crate** — records somebody *else* suggested, kept apart from
      everything you own, each with what in your own listening prompted it and
      whether you ever played it
- [x] **Skips are readable** — stored from live sources as well as imports, with
      what played either side of them
- [x] **Public now-playing** — one opt-in address, four separate field switches,
      and a Hugo shortcode for your own site

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
- [ ] **Network streamers** — one discovery-and-negotiation layer over
      SSDP/UPnP AVTransport, OpenHome, Google Cast and vendor HTTP APIs, so a
      streamer works because open protocols are supported rather than because
      someone wrote a driver for that exact box. A source declares its own blind
      spots: UPnP cannot see Spotify Connect or AirPlay, and the UI has to say so
- [ ] **Android app** — browse the history, scan a barcode onto the shelf, and
      optionally scrobble what the phone is playing. `AudioDeviceInfo` is the one
      thing only Android can contribute: USB DAC vs Bluetooth vs speaker, straight
      onto the chain ladder's device rung
- [ ] **Sharing a chapter as an image** — a Reports card as a PNG for a story
- [ ] Docker container
- [ ] Notes: **tags**. Full-text search and Markdown export for Obsidian/Logseq
      already shipped
- [ ] **Typing a track's running time in by hand** — a pressing MusicBrainz does
      not have stays unknown, because nothing in the UI writes a tracklist
- [ ] Organising the four analytics screens — Statistics, Top Charts, Reports and
      the genre map answer overlapping questions with their own period controls

### Known gaps

Things that look finished and aren't:

- **Artist portraits from Navidrome** aren't wired. Plex and Jellyfin are;
  Subsonic can answer it, but an untested path that silently returns nothing is
  worse than a stated gap.
- **Audio quality is only known for listens a live source reported.** An import
  from Last.fm or ListenBrainz carries no format at all — on a real 21,000-listen
  history that is 97% of it — so the fidelity readouts divide by the listens
  whose format is actually known and say how many that was. Below a quarter
  coverage they report the coverage instead of a percentage, because a share
  drawn from a twentieth of your listening is a fact about the twentieth.
- **Entity pages don't scope by user.** Play counts are correctly the caller's,
  but an entity's *name* comes from a shared table, so ids can be walked to learn
  what another household member has annotated. Low severity on an invite-only
  instance, and parked as a design call because the fix interacts with Phase 6
  visibility.
- **The SACD layer isn't recorded.** Stereo and multichannel are genuinely
  different listens and `AudioQuality.channels` has somewhere to put it, but
  `physical_plays` has no column for which one you played.
- **A running time can't be typed in.** Tapedeck will go and look one up — by
  MusicBrainz release ID if you paste one, else by barcode, matched on titles and
  previewed before it writes anything — but if no pressing anywhere has the
  figures, the tracklist stays unknown. `PUT /api/v1/physical/:id/tracks` exists
  and nothing in the UI calls it. A side with
  no running times shows no length and no progress bar rather than inventing
  one, and its listens carry no duration, so they add nothing to your gear hours
  until the numbers arrive.
- **Where a side's tracks fell in time can't be corrected after the fact.** A
  side recorded before v0.74.1 laid its tracks out end to end from the start
  time, so one played across an interruption sits on top of whatever you
  listened to in between. Only the start was ever stored, so there is nothing to
  reconstruct the real spacing from. New plays stamp each track as it starts, and
  `physical_plays.recorded_at` now keeps the other end.
- **Sides may be split by track count.** When a lookup returns no durations,
  `assign_sides` splits by number of tracks instead of playing time, which can
  put the break in the wrong place. Filling the running times in afterwards does
  not move it; re-filing the record through the medium picker does.

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
