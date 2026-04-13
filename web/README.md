# Tapedeck Web UI

SvelteKit frontend for Tapedeck, built with TailwindCSS and the Rosé Pine color theme.

## Prerequisites

- [Bun](https://bun.sh) (v1.0+)

## Development

```bash
cd web/

# Install dependencies
bun install

# Start dev server (port 5173, proxies API to localhost:8080)
bun run dev
```

Run the Tapedeck Rust backend alongside:

```bash
# In the project root
cargo run
```

The Vite dev server proxies `/1/*`, `/api/*`, `/admin/*`, and `/health` to the
Rust backend on port 8080, so you get live reload on the frontend with real API
data.

## Production Build

```bash
# From the project root
./build-web.sh      # Builds SvelteKit → static/
cargo build --release  # Embeds static/ into the binary via rust-embed
```

The resulting `tapedeck` binary serves the UI at `/` — no separate web server needed.

## Structure

```
web/
├── package.json
├── svelte.config.js        adapter-static → ../static/
├── vite.config.ts          Dev proxy to Axum backend
├── tailwind.config.js      Rosé Pine palette
├── postcss.config.js
├── src/
│   ├── app.html            Shell (Roboto font, dark mode)
│   ├── app.css             Tailwind base + quality badge classes
│   ├── lib/
│   │   ├── api.ts          Typed API client
│   │   ├── utils.ts        Formatters (time, duration, quality)
│   │   └── components/
│   │       ├── Nav.svelte          Sidebar navigation
│   │       ├── NowPlaying.svelte   Now playing hero card
│   │       ├── ScrobbleRow.svelte  Single scrobble list item
│   │       ├── StatsCard.svelte    Metric card
│   │       └── ChainCard.svelte    Signal chain visualizer
│   └── routes/
│       ├── +layout.svelte          Root layout (sidebar + main)
│       ├── +page.svelte            Dashboard
│       ├── history/+page.svelte    Scrobble timeline
│       ├── stats/+page.svelte      Statistics overview
│       ├── chains/+page.svelte     Signal chain management
│       └── settings/+page.svelte   Token + connection config
```

## Color Theme

[Rosé Pine](https://rosepinetheme.com/) — the full palette is available via
Tailwind utilities:

| Token       | Color     | Usage                  |
|-------------|-----------|------------------------|
| `rp-base`   | `#191724` | Page background        |
| `rp-surface` | `#1f1d2e` | Cards, sidebar         |
| `rp-overlay` | `#26233a` | Elevated surfaces      |
| `rp-text`   | `#e0def4` | Primary text           |
| `rp-subtle` | `#908caa` | Secondary text         |
| `rp-muted`  | `#6e6a86` | Tertiary text          |
| `rp-love`   | `#eb6f92` | Errors, now playing    |
| `rp-gold`   | `#f6c177` | Warnings, lossy badge  |
| `rp-rose`   | `#ebbcba` | Accents                |
| `rp-pine`   | `#31748f` | Lossless badge         |
| `rp-foam`   | `#9ccfd8` | Success, lossless text |
| `rp-iris`   | `#c4a7e7` | Interactive, DSD badge |

## Quality Badges

Four tiers with distinct visual treatment:

- **DSD** — iris/purple (`badge-dsd`)
- **Lossless** — pine/teal (`badge-lossless`)
- **Lossy** — gold/amber (`badge-lossy`)
- **Bluetooth-degraded** — love/red (`badge-bt`)
