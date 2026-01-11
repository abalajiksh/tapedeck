# MusicBrainz Metadata Architecture

## Overview

Tapedeck now uses a sophisticated 3-tier caching system for MusicBrainz metadata enrichment, separating concerns between playback tracking and metadata resolution.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Tapedeck                             │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌──────────────┐         ┌─────────────────────────┐      │
│  │  Plex Source │────────▶│  PlexTrack (Raw Data)   │      │
│  └──────────────┘         └───────────┬─────────────┘      │
│                                        │                    │
│                                        ▼                    │
│                          ┌──────────────────────────┐       │
│                          │  MusicBrainz Client      │       │
│                          │  (3-Tier Enrichment)     │       │
│                          └───────────┬──────────────┘       │
│                                      │                      │
│                    ┌─────────────────┼─────────────────┐    │
│                    │                 │                 │    │
│                    ▼                 ▼                 ▼    │
│              ┌─────────┐       ┌──────────┐     ┌─────────┐│
│              │ SQLite  │       │PostgreSQL│     │   MB    ││
│              │  Cache  │       │   Dump   │     │   API   ││
│              │ (Tier 1)│       │ (Tier 2) │     │(Tier 3) ││
│              └─────────┘       └──────────┘     └─────────┘│
│                    │                 │                 │    │
│                    └─────────────────┴─────────────────┘    │
│                                      │                      │
│                                      ▼                      │
│                          ┌──────────────────────────┐       │
│                          │  Play (Enriched)         │       │
│                          │  + MBIDs                 │       │
│                          └───────────┬──────────────┘       │
│                                      │                      │
│                                      ▼                      │
│                          ┌──────────────────────────┐       │
│                          │  Scrobble Sinks          │       │
│                          │  (Last.fm, ListenBrainz) │       │
│                          └──────────────────────────┘       │
└─────────────────────────────────────────────────────────────┘
```

## Components

### 1. Plex Source (`src/sources/plex.rs`)

**Responsibility**: Track playback data only

- Fetches current playing sessions
- Retrieves playback history
- Manages session state (scrobble thresholds)
- Returns `PlexTrack` objects with basic metadata
- **Does NOT** fetch MusicBrainz IDs

**Key Methods**:
- `get_playback_data()` - Returns `SessionResult` with `PlexTrack` objects
- `fetch_sessions_extended()` - Gets live sessions + history
- `PlexTrack::to_play()` - Converts to `Play` object (MBIDs = None)

### 2. MusicBrainz Client (`src/musicbrainz.rs`)

**Responsibility**: Metadata enrichment with 3-tier caching

#### Tier 1: SQLite Cache (Local, Fast)
- Stores successful MBID lookups
- Case-insensitive matching
- Automatic deduplication
- **Latency**: ~1-5ms

#### Tier 2: PostgreSQL Dump (Optional, Fast)
- Full MusicBrainz database mirror
- Offline-capable
- Requires ~30GB disk space
- **Latency**: ~10-50ms

#### Tier 3: MusicBrainz API (Fallback, Slow)
- Live API queries
- Rate limited: 1 request/second
- Automatic rate limiting with token bucket
- **Latency**: ~200-500ms

**Key Methods**:
- `fetch_metadata(title, artist, album)` - Main entry point
- `fetch_metadata_batch(tracks)` - Batch processing
- `initialize_schema()` - Creates SQLite tables

### 3. Configuration (`src/config.rs`)

**Environment Variables**:

```bash
# Required
CONTACT_EMAIL=your-email@example.com
MUSICBRAINZ_USER_AGENT=Tapedeck/0.3.7 ( your-email@example.com )
MUSICBRAINZ_RATE_LIMIT=1

# Database
SQLITE_DB_PATH=./tapedeck.db

# Optional: PostgreSQL Dump
MUSICBRAINZ_POSTGRES_URL=postgresql://user:pass@localhost/musicbrainz_db
MUSICBRAINZ_POSTGRES_ENABLED=true
```

## Data Flow

### Now Playing Flow

```
1. Plex API → PlexTrack (basic metadata)
2. PlexTrack → MusicBrainz Client
3. Check SQLite cache → HIT? Return MBIDs
4. Check PostgreSQL → HIT? Store in SQLite, return MBIDs
5. Query MB API (rate limited) → Store in SQLite, return MBIDs
6. PlexTrack + MBIDs → Play object
7. Play → Submit to sinks (Now Playing)
```

### Scrobble Flow

```
1. Plex API → PlexTrack (basic metadata)
2. PlexTrack → MusicBrainz Client (3-tier lookup)
3. PlexTrack + MBIDs → Play object
4. Play → Save to scrobble database
5. Background worker → Fetch pending scrobbles
6. Play → Submit to sinks (Last.fm, ListenBrainz)
7. On success → Mark as scrobbled
```

## Performance Characteristics

### Cache Hit Rates (Expected)

- **SQLite Cache**: 85-95% (for repeat listens)
- **PostgreSQL Dump**: 60-80% (for new tracks)
- **API Fallback**: 5-20% (obscure/new releases)

### Throughput

- **Tier 1 (SQLite)**: ~1000 lookups/second
- **Tier 2 (PostgreSQL)**: ~100 lookups/second
- **Tier 3 (API)**: 1 lookup/second (rate limited)

### Storage

- **SQLite Cache**: ~100KB per 1000 tracks
- **PostgreSQL Dump**: ~30GB (full MusicBrainz)

## Error Handling

### Graceful Degradation

1. **SQLite Failure**: Falls back to PostgreSQL or API
2. **PostgreSQL Failure**: Falls back to API
3. **API Failure**: Scrobble with basic metadata (no MBIDs)
4. **Rate Limit**: Automatic backoff and retry

### Logging

```rust
// Cache hit (fastest)
log::debug!("Cache hit (SQLite) for: {} - {}", artist, title);

// PostgreSQL hit
log::debug!("Cache hit (PostgreSQL) for: {} - {}", artist, title);

// API query
log::info!("Fetching from MusicBrainz API: {} - {}", artist, title);

// Enrichment success
log::info!("✓ Enriched: {} - {} [recording: {}, release: {}, artist: {}]",
    play.artist, play.title, recording_mbid, release_mbid, artist_mbid);

// Lookup failure (non-fatal)
log::warn!("⚠ MusicBrainz lookup failed for {} - {}: {}", artist, title, err);
```

## Database Schema

### SQLite Cache (`musicbrainz_cache`)

```sql
CREATE TABLE musicbrainz_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    track_mbid TEXT,
    artist_mbid TEXT,
    album_mbid TEXT,
    track_title TEXT NOT NULL,
    artist_name TEXT NOT NULL,
    album_name TEXT,
    release_date TEXT,
    genres TEXT,  -- JSON array
    fetched_at INTEGER NOT NULL,
    UNIQUE(track_title, artist_name, album_name)
);

CREATE INDEX idx_mb_cache_lookup 
    ON musicbrainz_cache(track_title, artist_name, album_name);

CREATE INDEX idx_mb_cache_mbid 
    ON musicbrainz_cache(track_mbid, artist_mbid);
```

## Usage Example

```rust
use tapedeck::musicbrainz::MusicBrainzClient;
use sqlx::SqlitePool;

// Initialize
let pool = SqlitePool::connect("sqlite:tapedeck.db").await?;
let config = MusicBrainzConfig::default();
let client = MusicBrainzClient::new(config, pool).await?;
client.initialize_schema().await?;

// Single lookup
let metadata = client.fetch_metadata(
    "Bohemian Rhapsody",
    "Queen",
    Some("A Night at the Opera")
).await?;

println!("Recording MBID: {:?}", metadata.track_mbid);
println!("Release MBID: {:?}", metadata.album_mbid);
println!("Artist MBID: {:?}", metadata.artist_mbid);

// Batch lookup
let tracks = vec![
    ("track1", "artist1", None),
    ("track2", "artist2", Some("album2")),
];

let results = client.fetch_metadata_batch(tracks).await;
for result in results {
    match result {
        Ok(metadata) => println!("Found: {}", metadata.track_title),
        Err(e) => eprintln!("Error: {}", e),
    }
}
```

## Setting Up PostgreSQL Dump (Optional)

### 1. Download MusicBrainz Database

```bash
# Download latest dump (30GB+)
wget http://ftp.musicbrainz.org/pub/musicbrainz/data/fullexport/LATEST
```

### 2. Import to PostgreSQL

```bash
# Create database
createdb musicbrainz_db

# Import (takes 2-4 hours)
psql musicbrainz_db < mbdump.sql
```

### 3. Configure Tapedeck

```bash
export MUSICBRAINZ_POSTGRES_URL="postgresql://localhost/musicbrainz_db"
export MUSICBRAINZ_POSTGRES_ENABLED=true
```

### 4. Verify

Check logs for:
```
✅ MusicBrainz client initialized with PostgreSQL dump support
```

## Migration from Old Architecture

### Before (Coupled)

```rust
// plex.rs handled everything
let play = plex.fetch_sessions().await?; // Includes MBIDs
scrobbler.submit(play).await?;
```

### After (Decoupled)

```rust
// plex.rs: playback data only
let plex_tracks = plex.get_playback_data().await?.ready_to_scrobble;

// Separate enrichment
for track in plex_tracks {
    let mut play = track.to_play("scrobble");
    
    // Add MBIDs via dedicated client
    if let Ok(metadata) = mb_client.fetch_metadata(&track.title, &track.artist, track.album.as_deref()).await {
        play.mbid_recording = metadata.track_mbid;
        play.mbid_release = metadata.album_mbid;
        play.mbid_artist = metadata.artist_mbid.map(|id| vec![id]);
    }
    
    scrobbler.submit(play).await?;
}
```

## Testing

```bash
# Run tests
cargo test

# Test with debug logging
RUST_LOG=debug cargo run

# Test MusicBrainz API (rate limited)
RUST_LOG=tapedeck::musicbrainz=trace cargo run
```

## Troubleshooting

### Issue: Rate Limited by MusicBrainz

**Solution**: Increase cache usage or set up PostgreSQL dump

```bash
MUSICBRAINZ_POSTGRES_ENABLED=true
```

### Issue: Slow Metadata Lookups

**Check**:
1. SQLite cache hit rate in logs
2. Consider PostgreSQL dump for offline operation

### Issue: Missing MBIDs

**Causes**:
- Track not in MusicBrainz database
- Artist/title mismatch (spelling, punctuation)
- API timeout/error

**Solution**: Scrobbles still work with basic metadata

## Benefits

1. **Separation of Concerns**: Plex ≠ MusicBrainz
2. **Reusability**: MusicBrainz client works with any source
3. **Performance**: 3-tier caching reduces API calls by 90%+
4. **Offline Capable**: Optional PostgreSQL dump
5. **Rate Limit Compliance**: Built-in 1 req/sec throttling
6. **Graceful Degradation**: Works without MBIDs if needed
7. **Type Safety**: Rust prevents invalid states at compile time

## Future Enhancements

- [ ] AcoustID fingerprinting for better matching
- [ ] Bulk import from listening history
- [ ] MusicBrainz recording submission
- [ ] Genre tagging from MusicBrainz tags
- [ ] Album art URL caching
