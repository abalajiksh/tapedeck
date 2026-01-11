# MusicBrainz Integration Guide

## Overview

Tapedeck now includes a sophisticated 3-tier MusicBrainz metadata enrichment system that automatically adds MBIDs (MusicBrainz Identifiers) to your scrobbles, improving accuracy and compatibility with services like ListenBrainz.

## Architecture

The MusicBrainz integration uses a **3-tier caching strategy** to minimize API calls and improve performance:

```
┌─────────────────────────────────────────────────────────────┐
│                      Playback Sources                        │
│                    (Plex, Jellyfin, etc.)                   │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│              MusicBrainz Metadata Enrichment                │
│                                                             │
│  Tier 1: SQLite Cache (Local, Instant)                     │
│     └─> Tier 2: PostgreSQL MB Dump (Optional, Fast)        │
│           └─> Tier 3: MusicBrainz API (Fallback, 1 req/s)  │
└─────────────────────┬───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│               Enriched Scrobbles with MBIDs                 │
│          (Saved to Database & Sent to Sinks)                │
└─────────────────────────────────────────────────────────────┘
```

### Tier 1: SQLite Cache

- **Purpose**: Fast local cache of previously fetched metadata
- **Performance**: Instant lookups (< 1ms)
- **Storage**: `tapedeck.db` (configured via `SQLITE_DB_PATH`)
- **Schema**: Automatic initialization on startup
- **Cache Policy**: Permanent storage, grows over time

**Table Structure**:
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
    genres TEXT,
    fetched_at INTEGER NOT NULL,
    UNIQUE(track_title, artist_name, album_name)
);
```

### Tier 2: PostgreSQL MusicBrainz Dump (Optional)

- **Purpose**: Offline access to complete MusicBrainz database
- **Performance**: Fast lookups (< 50ms)
- **Setup**: Requires local PostgreSQL with MB dump
- **Download**: https://musicbrainz.org/doc/MusicBrainz_Database
- **Size**: ~30GB (compressed), ~50GB (uncompressed)

**Benefits**:
- No API rate limits
- Works offline
- Complete metadata coverage
- Ideal for bulk imports

**Configuration**:
```env
MUSICBRAINZ_POSTGRES_URL=postgresql://musicbrainz:password@localhost:5432/musicbrainz_db
MUSICBRAINZ_POSTGRES_ENABLED=true
```

### Tier 3: MusicBrainz API

- **Purpose**: Fetch missing metadata from official MusicBrainz servers
- **Rate Limit**: 1 request/second (automatic throttling)
- **Performance**: 1-2 seconds per lookup
- **User Agent**: Required by MusicBrainz policy

**Rate Limiting**:
- Implemented via token bucket algorithm
- Automatic queuing and spacing
- Respects MusicBrainz Terms of Service

## Configuration

### Required Environment Variables

```env
# Contact email (required for MusicBrainz API)
CONTACT_EMAIL=your-email@example.com

# User agent (automatically generated if not set)
MUSICBRAINZ_USER_AGENT=Tapedeck/0.3.7 ( your-email@example.com )

# API rate limit (requests per second)
MUSICBRAINZ_RATE_LIMIT=1

# SQLite database path
SQLITE_DB_PATH=./tapedeck.db
```

### Optional PostgreSQL Configuration

```env
# PostgreSQL MusicBrainz database dump
MUSICBRAINZ_POSTGRES_URL=postgresql://user:pass@localhost:5432/musicbrainz_db
MUSICBRAINZ_POSTGRES_ENABLED=true
```

## Usage

### Automatic Enrichment

Metadata enrichment happens automatically for all playback events:

1. **Now Playing Updates**: Real-time MBID lookup (non-blocking)
2. **Scrobbles**: MBID lookup before saving to database
3. **Batch Processing**: Efficient handling of multiple tracks

### Manual Testing

You can test the MusicBrainz integration with:

```bash
# Enable debug logging
export RUST_LOG=debug

# Run Tapedeck
cargo run
```

**Expected output**:
```
✓ Enriched: The Beatles - Come Together [recording: abc123, release: def456, artist: ghi789]
```

## Performance

### Cache Hit Rates

**Typical Performance** (after initial run):
- SQLite Cache Hit: **90-95%** (< 1ms)
- PostgreSQL Hit: **3-5%** (< 50ms)
- API Fallback: **0-2%** (1-2 seconds)

### API Usage

**First Run** (1000 unique tracks):
- API Calls: ~1000 requests
- Time: ~17 minutes (1 req/sec)
- Cache: 100% populated

**Subsequent Runs**:
- API Calls: 0-20 requests (new tracks only)
- Time: < 1 second (cache hits)
- Cache: Grows incrementally

## MusicBrainz Database Setup (Optional)

For users who want offline metadata access:

### 1. Install PostgreSQL

```bash
# Debian/Ubuntu
sudo apt-get install postgresql postgresql-contrib

# macOS
brew install postgresql
```

### 2. Download MusicBrainz Dump

```bash
# Create database
creatdb musicbrainz_db

# Download and import (this takes several hours)
wget http://ftp.musicbrainz.org/pub/musicbrainz/data/fullexport/latest/mbdump.tar.bz2
tar -xjf mbdump.tar.bz2
psql musicbrainz_db < mbdump/CreateTables.sql
psql musicbrainz_db < mbdump/ImportData.sql
```

### 3. Configure Tapedeck

```env
MUSICBRAINZ_POSTGRES_URL=postgresql://localhost/musicbrainz_db
MUSICBRAINZ_POSTGRES_ENABLED=true
```

## Troubleshooting

### Issue: Rate Limit Errors

**Symptom**: `MusicBrainz API error: 503`

**Solution**:
- Reduce `MUSICBRAINZ_RATE_LIMIT` to `0.5`
- Wait a few minutes before retrying
- Consider setting up PostgreSQL dump

### Issue: Invalid User Agent

**Symptom**: `MusicBrainz API error: 403`

**Solution**:
- Ensure `CONTACT_EMAIL` is set
- Update `MUSICBRAINZ_USER_AGENT` format
- Follow MusicBrainz naming guidelines

### Issue: PostgreSQL Connection Failed

**Symptom**: `Failed to connect to PostgreSQL`

**Solution**:
- Verify PostgreSQL is running
- Check `MUSICBRAINZ_POSTGRES_URL` connection string
- Test with: `psql <connection_string>`
- Set `MUSICBRAINZ_POSTGRES_ENABLED=false` to disable

### Issue: SQLite Lock Errors

**Symptom**: `database is locked`

**Solution**:
- Ensure no other processes are using the database
- Check file permissions on `tapedeck.db`
- Use separate database files if running multiple instances

## Best Practices

### 1. Set a Valid Contact Email

MusicBrainz requires contact information in the User-Agent:

```env
CONTACT_EMAIL=your-actual-email@example.com
```

### 2. Respect Rate Limits

Never set `MUSICBRAINZ_RATE_LIMIT` above `1` (1 request/second)

### 3. Use PostgreSQL for Bulk Imports

If importing large libraries (> 10,000 tracks), set up PostgreSQL:
- Eliminates API rate limiting
- Significantly faster imports
- Works offline

### 4. Monitor Cache Performance

Watch for cache efficiency in logs:
```
Cache hit (SQLite) for: The Beatles - Come Together
```

High API usage indicates cache misses - consider PostgreSQL.

### 5. Handle Metadata Gracefully

Tapedeck continues working even if MusicBrainz is unavailable:
- Scrobbles without MBIDs still work
- Basic metadata from Plex/Jellyfin is used
- Metadata can be enriched later

## API Reference

### MusicBrainzClient

```rust
use tapedeck::musicbrainz::{MusicBrainzClient, MusicBrainzConfig};

// Initialize client
let config = MusicBrainzConfig {
    api_base_url: "https://musicbrainz.org/ws/2".to_string(),
    user_agent: "Tapedeck/0.3.7 ( email@example.com )".to_string(),
    rate_limit_per_second: 1,
    postgres_url: None,
    enable_postgres: false,
};

let client = MusicBrainzClient::new(config, sqlite_pool).await?;
client.initialize_schema().await?;

// Fetch metadata
let metadata = client.fetch_metadata(
    "Come Together",
    "The Beatles",
    Some("Abbey Road"),
).await?;

println!("Track MBID: {:?}", metadata.track_mbid);
println!("Artist MBID: {:?}", metadata.artist_mbid);
println!("Album MBID: {:?}", metadata.album_mbid);
```

## MusicBrainz Identifier (MBID) Benefits

### For Users

- **Improved Accuracy**: Disambiguates artists with same name
- **Better Stats**: More accurate ListenBrainz statistics
- **Enhanced Discovery**: Better music recommendations
- **Universal IDs**: Works across all MusicBrainz-enabled services

### For Services

- **ListenBrainz**: Required for accurate statistics and recommendations
- **Last.fm**: Optional but improves matching accuracy
- **Funkwhale**: Enhanced metadata integration
- **Maloja**: Better duplicate detection

## Resources

- [MusicBrainz API Documentation](https://musicbrainz.org/doc/MusicBrainz_API)
- [MusicBrainz Database Download](https://musicbrainz.org/doc/MusicBrainz_Database)
- [ListenBrainz MBID Guidelines](https://listenbrainz.org/)
- [Rate Limiting Policy](https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting)

## License

MusicBrainz data is licensed under CC0 (public domain).
Tapedeck integration code is licensed under GPL-3.0.
