#!/usr/bin/env bash
# ============================================================================
# obs_smoke_build.sh — prove an OBS source tarball builds and boots by itself
#
# OBS will build the tarball with no network, so this does the same on a
# Linux host before anything is uploaded:
#
#   1. extract it OUTSIDE any git checkout
#   2. `cargo build --release --frozen` with an EMPTY CARGO_HOME, so no
#      registry cache on the host can quietly supply a crate the tarball
#      forgot to vendor (or that the Linux-only stubbing emptied), inside an
#      empty network namespace so no build script can download either
#   3. check the embedded UI made it into the binary
#   4. start the binary as a replica on a free loopback port, in an empty
#      state directory, and require /health to answer "ok" with the version
#      the tarball is named for — then stop it with SIGTERM and require a
#      clean exit inside the unit's 45 s stop window
#
# Replica mode, an empty MUSICBRAINZ_CONTACT and loopback-only binding mean
# the boot talks to nothing outside this host.
#
# Usage:
#   scripts/obs_smoke_build.sh TARBALL
# Environment:
#   CARGO_TARGET_DIR  reuse a target dir across runs (default: a temp dir).
#                     The vendored crates sit at the same relative paths in
#                     every release, so a persistent one rebuilds only what
#                     changed.
# ============================================================================

set -euo pipefail

TARBALL="${1:?usage: $0 TARBALL}"
TARBALL="$(cd "$(dirname "$TARBALL")" && pwd)/$(basename "$TARBALL")"
NAME="$(basename "$TARBALL" .tar.gz)"
VERSION="${NAME#tapedeck-}"
[ "$VERSION" != "$NAME" ] || { echo "error: $TARBALL is not a tapedeck-<version>.tar.gz" >&2; exit 1; }

WORK="$(mktemp -d)"
PID=""
cleanup() {
    [ -n "$PID" ] && kill -KILL "$PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

tar -xzf "$TARBALL" -C "$WORK"
SRC="$WORK/$NAME"
[ -f "$SRC/.cargo/config.toml" ] || { echo "error: no vendored-source config in the tarball" >&2; exit 1; }

echo ">> building $NAME offline"
mkdir -p "$WORK/cargo-home"
(
    cd "$SRC"
    export CARGO_HOME="$WORK/cargo-home" CARGO_NET_OFFLINE=true
    export TAPEDECK_SKIP_WEB_BUILD=1
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$WORK/target}"
    # CARGO_NET_OFFLINE stops cargo, not a build script: lindera's fetched its
    # dictionaries over HTTP right through it, so this passed for v0.121.0 and
    # OBS then failed everywhere. A network namespace with nothing in it is
    # what OBS actually gives a build.
    if unshare --user --map-root-user --net true 2>/dev/null; then
        unshare --user --map-root-user --net cargo build --release --frozen
    else
        echo "warning: cannot unshare a network namespace here; build scripts" \
             "still have network, so this does not prove an offline build" >&2
        cargo build --release --frozen
    fi
    install -m755 "$CARGO_TARGET_DIR/release/tapedeck" "$WORK/tapedeck"
)
BIN="$WORK/tapedeck"

grep -qF 'auth/status' "$BIN" \
    || { echo "error: the embedded UI is missing from the binary" >&2; exit 1; }

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
STATE="$WORK/state"
mkdir -p "$STATE"
echo ">> booting it on 127.0.0.1:$PORT"
(
    cd "$STATE"
    exec env -i PATH=/usr/bin:/bin HOME="$STATE" \
        HOST=127.0.0.1 PORT="$PORT" TAPEDECK_REPLICA=1 MUSICBRAINZ_CONTACT= \
        ENABLE_FILE_LOGGING=false ENABLE_CONSOLE_LOGGING=true \
        "$BIN"
) > "$WORK/run.log" 2>&1 &
PID=$!

HEALTH=""
for _ in $(seq 1 60); do
    if ! kill -0 "$PID" 2>/dev/null; then
        cat "$WORK/run.log" >&2
        echo "error: tapedeck exited during startup" >&2
        exit 1
    fi
    HEALTH="$(curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" 2>/dev/null || true)"
    [ -n "$HEALTH" ] && break
    sleep 1
done
[ -n "$HEALTH" ] || { cat "$WORK/run.log" >&2; echo "error: /health never answered" >&2; exit 1; }
echo ">> /health: $HEALTH"
python3 - "$HEALTH" "$VERSION" <<'PY'
import json, sys
h, want = json.loads(sys.argv[1]), sys.argv[2]
problems = []
if h.get("status") != "ok":
    problems.append(f"status is {h.get('status')!r}")
if h.get("version") != want:
    problems.append(f"version is {h.get('version')!r}, the tarball says {want!r}")
if h.get("replica") is not True:
    problems.append("not running as a replica, so it may have talked to the network")
if problems:
    sys.exit("error: " + "; ".join(problems))
PY

kill -TERM "$PID"
for _ in $(seq 1 45); do
    kill -0 "$PID" 2>/dev/null || break
    sleep 1
done
if kill -0 "$PID" 2>/dev/null; then
    echo "error: still running 45 s after SIGTERM — systemd would have killed it" >&2
    exit 1
fi
wait "$PID" || { cat "$WORK/run.log" >&2; echo "error: exited non-zero after SIGTERM" >&2; exit 1; }
PID=""
echo ">> $NAME builds offline, boots, reports $VERSION, and stops cleanly"
