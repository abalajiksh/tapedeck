#!/usr/bin/env bash
# build-web.sh — Build the SvelteKit frontend into static/ for rust-embed
#
# Run this before `cargo build` or wire it into your CI pipeline.
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

# Install dependencies
echo "📦 Installing dependencies..."
bun install --frozen-lockfile 2>/dev/null || bun install

# Build SvelteKit with adapter-static
echo "🔨 Building SvelteKit..."
bun run build

echo "✅ Frontend built → $OUT_DIR"
echo ""
echo "Now run: cargo build --release"
