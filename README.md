# 📼 Tapedeck

A self-hosted scrobbler and music journal, written in Rust. Tapedeck collects
your listens from wherever you play music, enriches them with MusicBrainz, and
keeps a complete personal history: not only *what* you played, but *how*. It
records the signal chain from file to DAC to headphones, whether that FLAC
reached you as SBC over Bluetooth, and the sides of vinyl you put on by hand.

One binary, your data, DSD is first-class.

**[tapedeck.cc](https://tapedeck.cc)** · [Live demo](https://demo.tapedeck.cc) ·
[Features](https://tapedeck.cc/features/) · [Docs](https://tapedeck.cc/docs/) ·
[API reference](https://tapedeck.cc/api/)

## What it does

- **Collects listens from anywhere.** It has a ListenBrainz-compatible API
  (Pano Scrobbler, fooyin, browsers, scripts), it polls Plex, Navidrome,
  Jellyfin, Emby and Roon, and it imports Rockbox logs and exports from
  ListenBrainz, Last.fm and Spotify.
- **Forwards them.** It scrobbles on to Last.fm, Libre.fm and ListenBrainz,
  using the accounts each user connected.
- **Knows your gear.** It tracks signal chains, an audio quality score per
  listen, and hours on every piece of equipment.
- **Keeps a shelf.** Records, tapes and discs: catalogue them by barcode, then
  log a side as you play it, pauses and skips included.
- **Lets you write about it.** Liner notes, loves, and star ratings read from
  your media server.
- **Shows you your listening.** Reports by week, month and year, Rediscovery,
  Playlist Lab, a musical-personality profile, and lyrics with romanization.
- **Shares, if you want it to.** Patch lets you follow the other decks on your
  instance, and a now-playing widget can go on your own site.
- **Talks to AI assistants.** An MCP server lets one read your history and hand
  playlists back.
- **Runs on very little.** It serves several users on SQLite or Postgres, and
  the public demo runs on 512 MB of RAM.

## Install

**Linux packages** are on the openSUSE Build Service for openSUSE, Fedora,
Debian 13, Ubuntu 26.04 and Arch (x86_64, and aarch64 on most):
**[one-click install page](https://software.opensuse.org/download.html?project=home:abksh:tapedeck&package=tapedeck)**.
The service is installed but not enabled; set `MUSICBRAINZ_CONTACT` in
`/etc/tapedeck/tapedeck.env`, then run `sudo systemctl enable --now tapedeck`.

**Docker** images are multi-arch (amd64, arm64) and published for every
release. Tags: `latest`, `0.134`, `0.134.0`.

```bash
docker run -d --name tapedeck -p 8080:8080 -v tapedeck-data:/data \
  -e MUSICBRAINZ_CONTACT=you@your.domain ghcr.io/abalajiksh/tapedeck
```

Everything is written to `/data`, including the credential key, so keep that
volume. [`compose.yaml`](compose.yaml) is a starting point. The image has no
ffmpeg, so a report Mixdown plays in the browser but can't be exported as video.

**From source** you need Rust 1.88+ and [Bun](https://bun.sh); `build.rs`
builds the web UI into the binary.

```bash
git clone https://codeberg.org/abksh/tapedeck.git && cd tapedeck
cargo build --release                       # SQLite (default)
cargo build --release --features postgres   # or Postgres
```

## First run

Open `http://<host>:8080` and **create the admin account straight away**: until
it exists, the setup page answers anyone who can reach the port. Then, in
*Settings*:

- **Connections**: Last.fm, Libre.fm or ListenBrainz, if you want listens
  forwarded.
- **Sources**: Plex, Navidrome, Jellyfin, Emby or Roon.
- **API Tokens**: a `td_…` token for each scrobble client.

To connect Pano Scrobbler, or anything else that speaks ListenBrainz, add a
custom ListenBrainz server with URL `http://<host>:8080` and your token.
[The docs](https://tapedeck.cc/docs/) cover the other clients.

## Configuration

Almost everything is set in the UI. Files hold only infrastructure: copy
`.env.example` to `.env` (or use `tapedeck.toml`; environment variables win).
These are the ones you're likely to touch:

| Variable | What it's for |
|---|---|
| `MUSICBRAINZ_CONTACT` | Your email or URL. Without it, MusicBrainz and cover-art lookups stay off. |
| `PUBLIC_URL`, `TRUST_PROXY` | Set both behind an HTTPS reverse proxy. |
| `TAPEDECK_SOURCE_NETWORKS` | Private networks a member's media server may live on, e.g. `192.168.1.0/24`. |
| `DATABASE_URL` | SQLite file (default) or a Postgres URL for a Postgres build. |
| `PORT`, `HOST` | Default `8080` on `0.0.0.0`. |

Every setting is documented in `.env.example`. Stored credentials are encrypted
with a key kept outside the database (`tapedeck.key`, or `TAPEDECK_SECRET_KEY`).
Back it up, but not in the same place as your database backups.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) first: it covers how PRs are tested,
the house rules, and the architecture. The local loop is:

```bash
cargo fmt && cargo clippy
cargo test && cargo test --features postgres
scripts/opengrep.sh
```

Issues and pull requests go to [Codeberg](https://codeberg.org/abksh/tapedeck);
the GitHub copy is a mirror.

## License

[AGPL-3.0](LICENSE).

## Acknowledgments

[Pano Scrobbler](https://github.com/kawaiiDango/pano-scrobbler) (inspiration and
primary client) · [MusicBrainz](https://musicbrainz.org) ·
[ListenBrainz](https://listenbrainz.org) ·
[Cover Art Archive](https://coverartarchive.org) ·
[Deezer](https://developers.deezer.com) · [Discogs](https://www.discogs.com) ·
[Every Noise at Once](https://everynoise.com) (Glenn McDonald's genre map) ·
[DVinyl](https://github.com/Kyonew/DVinyl) ·
[AudioMuse-AI](https://github.com/NeptuneHub/AudioMuse-AI) ·
[uqr](https://github.com/unjs/uqr) · [Navidrome](https://www.navidrome.org) ·
[Jellyfin](https://jellyfin.org) · [Rockbox](https://www.rockbox.org).
Typefaces: [Figtree](https://github.com/erikdkennedy/figtree) and
[Caprasimo](https://github.com/docrepair-fonts/caprasimo-fonts), OFL 1.1, served by the
instance itself (licence at `/licenses/fonts.txt`).
