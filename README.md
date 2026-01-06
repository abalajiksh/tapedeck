📼 Tapedeck
Tapedeck is a lightweight, high-performance scrobbler written in Rust. It monitors your Plex Media Server and automatically syncs your playback history to Last.fm and ListenBrainz.

Designed for speed and reliability, it fills the gap for users who want dual-scrobbling support without heavy dependencies or complex webhooks.

🚀 Features
Multi-Sink Support: Scrobble to both Last.fm and ListenBrainz simultaneously.

Plex Integration: Polls Plex history directly (no webhooks required), making it perfect for users behind CGNAT or without Plex Pass.

Smart Filtering:

Ignores short tracks (< 30 seconds).

Deduplicates plays based on timestamp.

Handles "offline" syncs by checking deep history (last 200 items).

Rust Native: Fast, low memory footprint, and single binary deployment.

Rich Metadata: Submits artist, track, album, and MusicBrainz IDs (where available).

🛠️ Installation
Prerequisites
Rust Toolchain (for building from source)

A Plex Media Server

ListenBrainz User Token

Last.fm API Key & Session Key

Build from Source
bash
git clone https://github.com/ashwinbalaji/tapedeck.git
cd tapedeck
cargo build --release
The binary will be available at ./target/release/tapedeck.

⚙️ Configuration
Tapedeck uses a .env file for configuration. Create a file named .env in the same directory as the binary:

text
# --- Plex Source ---
PLEX_URL=http://192.168.1.X:32400
PLEX_TOKEN=your-plex-token-here

# --- ListenBrainz Sink ---
LISTENBRAINZ_TOKEN=your-lb-token-here
LISTENBRAINZ_URL=https://api.listenbrainz.org/1/submit-listens

# --- Last.fm Sink ---
LASTFM_API_KEY=your-api-key
LASTFM_SECRET=your-shared-secret
LASTFM_SESSION_KEY=your-session-key

# --- System ---
# Check interval in seconds (default: 300)
CHECK_INTERVAL=300
# Logging Level (error, warn, info, debug, trace)
RUST_LOG=info
Getting Tokens
Plex Token: See Plex Support Article.

ListenBrainz: Get it from your Profile Settings.

Last.fm: You need to generate a session key. Use a tool like Last.fm API Auth or a simple script to exchange a token for a session key.

🏃 Usage
Run the binary directly. It will look for the .env file in the current working directory.

bash
# Standard run
./tapedeck

# Run with debug logging (overrides .env)
RUST_LOG=debug ./tapedeck
Docker (Optional)
Coming soon...

🤝 Contributing
Pull requests are welcome! Please ensure your code formats with cargo fmt and passes cargo clippy before submitting.

