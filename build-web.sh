#!/usr/bin/env bash
# build-web.sh — Build the SvelteKit frontend into static/ for rust-embed
#
# NO LONGER REQUIRED BEFORE `cargo build`. Since v0.14.0 build.rs does this
# during the build, so a fresh clone compiles and static/ cannot go stale.
#
# This script is kept for frontend-only iteration — rebuilding the UI without
# invoking cargo at all. It is the same sequence build.rs runs.
#
# The bundle smoke test at the bottom is deliberately duplicated in
# build.rs::verify_bundle. If you change one, change the other: it guards a
# failure mode that is completely silent (see the comment there).
#
# Requires: bun (https://bun.sh)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB_DIR="$SCRIPT_DIR/web"
OUT_DIR="$SCRIPT_DIR/static"

echo "══════════════════════════════════════════"
echo "  📼 Tapedeck — Building frontend"
echo "══════════════════════════════════════════"

# Check for bun
if ! command -v bun &> /dev/null; then
    echo "❌ bun is required but not installed."
    echo "   Install: curl -fsSL https://bun.sh/install | bash"
    exit 1
fi

cd "$WEB_DIR"

# Install dependencies. bun.lock IS committed — a frozen install keeps the
# toolchain pinned. Drifting off it is how vite/vite-plugin-svelte once ended up
# on incompatible majors, which silently compiled every onMount out of the app.
echo "📦 Installing dependencies..."
if ! bun install --frozen-lockfile; then
    echo "⚠️  Lockfile is out of date with package.json — re-resolving."
    echo "   Commit the updated web/bun.lock afterwards."
    bun install
fi

# Build SvelteKit with adapter-static
echo "🔨 Building SvelteKit..."
bun run build

# Smoke-test the bundle. The root layout's onMount is what boots the whole SPA
# (auth gate → /setup or /login redirect); if a broken toolchain drops lifecycle
# callbacks, the build still "succeeds" and ships a page that renders a spinner
# forever. Assert that call survived rather than finding out in the browser.
echo "🔎 Verifying bundle..."
if ! grep -rq "auth/status" "$OUT_DIR/_app"; then
    echo "❌ Built bundle never calls /api/v1/auth/status."
    echo "   The root layout's onMount was compiled away — the SPA would boot"
    echo "   to a blank page. Check svelte / vite / @sveltejs/vite-plugin-svelte"
    echo "   are on mutually compatible majors (see 'bun pm ls')."
    exit 1
fi

echo "✅ Frontend built → $OUT_DIR"
echo ""
echo "Now run: cargo build --release"
