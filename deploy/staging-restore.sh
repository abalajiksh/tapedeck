#!/usr/bin/env bash
#
# Refresh a staging replica from the authoritative instance.
#
# Staging exists so `main` can be exercised against *real* data without four
# household users noticing. This script is the whole sync: take a snapshot,
# strip the credentials, install it, restart. There is no delta protocol and
# deliberately so — see design note 0004. A nightly restore re-runs every
# migration against a production-shaped database, which an incrementally-synced
# replica seeded once would never do, and that is the single thing most worth
# testing here (v0.61.1: 297 restarts in 26 minutes from a migration that could
# not run against an existing database, while the whole suite stayed green).
#
#   PROD_HOST=abalaji@192.168.178.132 \
#   STAGING_HOST=abalaji@192.168.178.140 \
#   ./deploy/staging-restore.sh
#
# Safe to re-run. It never writes to production: the only thing it does there is
# `VACUUM INTO`, which reads.

set -euo pipefail

PROD_HOST="${PROD_HOST:?set PROD_HOST, e.g. abalaji@192.168.178.132}"
STAGING_HOST="${STAGING_HOST:?set STAGING_HOST, e.g. abalaji@192.168.178.140}"
PROD_DIR="${PROD_DIR:-/home/abalaji/tapedeck}"
STAGING_DIR="${STAGING_DIR:-/home/abalaji/tapedeck}"
DB_NAME="${DB_NAME:-scrobbles.db}"
SERVICE="${SERVICE:-tapedeck}"
HEALTH_URL="${HEALTH_URL:-http://127.0.0.1:8080/health}"
SSHOPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SNAP="$WORK/snapshot.db"

echo "→ snapshotting ${PROD_HOST}:${PROD_DIR}/${DB_NAME}"
# VACUUM INTO rather than copying the file: it is the one form that is safe
# against a live WAL, and it is what /api/v1/backup already does. Copying
# scrobbles.db alone would miss whatever is still in the -wal.
ssh $SSHOPTS "$PROD_HOST" "
  set -e
  rm -f /tmp/td-snap.db
  sqlite3 '${PROD_DIR}/${DB_NAME}' \"VACUUM INTO '/tmp/td-snap.db'\"
"
scp $SSHOPTS "$PROD_HOST:/tmp/td-snap.db" "$SNAP"
ssh $SSHOPTS "$PROD_HOST" "rm -f /tmp/td-snap.db"
echo "  $(du -h "$SNAP" | cut -f1)"

# ── Sanitise ────────────────────────────────────────────────────────────────
#
# TAPEDECK_REPLICA=1 in staging's .env is what actually enforces all of this at
# runtime, and it is the guarantee that matters — a mode cannot be forgotten on
# the one night somebody restores by hand. This pass is defence in depth: it
# means the *file* is not worth stealing either, and that staging cannot spend a
# credential even if it is started without the flag.
echo "→ stripping credentials"
sqlite3 "$SNAP" <<'SQL'
PRAGMA foreign_keys = OFF;
-- Forwarding credentials: Last.fm, ListenBrainz, Libre.fm, TIDAL.
DELETE FROM user_connections;
-- Anything that could push listens in, or read them out, with prod's tokens.
DELETE FROM tokens;
DELETE FROM mcp_connections;
DELETE FROM oauth_codes;
DELETE FROM oauth_clients;
DELETE FROM web_sessions;
DELETE FROM pairing_requests;
-- Source polling. Reads are harmless and dedup is deterministic, but a second
-- poller is a second set of requests at somebody's Plex for no benefit.
UPDATE user_sources SET enabled = 0;
-- Two switches that publish. `federation_enabled` matters most: actors.uri is
-- written once and never re-derived, so a restored snapshot would otherwise
-- serve production's actor URI from the wrong host under a second keypair.
UPDATE users SET federation_enabled = 0, public_now_playing = 0;
-- Delivery state that is not this instance's to hold.
DELETE FROM activity_deliveries;
VACUUM;
SQL

echo "→ installing on ${STAGING_HOST}:${STAGING_DIR}"
scp $SSHOPTS "$SNAP" "$STAGING_HOST:${STAGING_DIR}/${DB_NAME}.new"

ssh $SSHOPTS "$STAGING_HOST" "
  set -e
  export XDG_RUNTIME_DIR=/run/user/\$(id -u)

  grep -q '^TAPEDECK_REPLICA=1' '${STAGING_DIR}/.env' 2>/dev/null || {
    echo '❌ ${STAGING_DIR}/.env does not set TAPEDECK_REPLICA=1.'
    echo '   Refusing to install production data on an instance that would forward it.'
    rm -f '${STAGING_DIR}/${DB_NAME}.new'
    exit 1
  }

  systemctl --user stop ${SERVICE} || true
  # The -wal and -shm belong to the database being replaced. Leaving them puts
  # a fresh file next to another database's journal.
  rm -f '${STAGING_DIR}/${DB_NAME}-wal' '${STAGING_DIR}/${DB_NAME}-shm'
  mv -f '${STAGING_DIR}/${DB_NAME}.new' '${STAGING_DIR}/${DB_NAME}'
  systemctl --user start ${SERVICE}

  # Poll rather than sleep. RestartSec=5 makes a crash loop and a slow start
  # indistinguishable through a single check — the v0.61.1 lesson the deploy
  # step already carries.
  for i in \$(seq 1 30); do
    state=\$(systemctl --user is-active ${SERVICE} || true)
    [ \"\$state\" = active ] && break
    sleep 1
  done
  if [ \"\$state\" != active ]; then
    echo \"service is '\$state' after 30s — last 40 log lines:\"
    journalctl --user -u ${SERVICE} -n 40 --no-pager || true
    exit 1
  fi

  # Active is not healthy. And this is where a migration that cannot run
  # against a real database shows itself, which is most of why staging exists.
  health=\$(curl -fsS --max-time 15 ${HEALTH_URL})
  echo \"   \$health\"
  echo \"\$health\" | grep -q '\"replica\":true' || {
    echo '❌ the instance came up but does not report replica mode'
    exit 1
  }
"
echo "✅ staging restored and healthy, in replica mode"
