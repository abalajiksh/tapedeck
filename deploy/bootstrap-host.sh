#!/usr/bin/env bash
#
# Prepare a host to receive Tapedeck deploys. Run once per machine, before the
# first deploy — the pipeline installs a binary and restarts a service, and
# neither the directory nor the unit exists until this has run.
#
#   HOST=abalaji@192.168.178.63 ROLE=staging \
#   PUBLIC_URL=http://192.168.178.63:8080 \
#   ./deploy/bootstrap-host.sh
#
#   ROLE=staging     TAPEDECK_REPLICA=1 — forwards nothing, publishes no actor,
#                    asks MusicBrainz nothing. See src/replica.rs.
#   ROLE=production  the authoritative instance. No replica flag.
#   ROLE=demo        public, reset data. No replica flag either — the demo is
#                    its own instance rather than a copy of one.
#
# Idempotent, and deliberately **never overwrites an existing .env**. That file
# holds TAPEDECK_SECRET_KEY, which encrypts every stored credential in the
# database; regenerating it does not lock you out so much as make the rows
# undecryptable, with nothing to say so until something tries to use one.
#
# It does not start the service. There is no binary yet — that is the pipeline's
# job, and `systemctl --user restart` on an enabled, stopped unit is what the
# deploy step already does.

set -euo pipefail

HOST="${HOST:?set HOST, e.g. abalaji@192.168.178.63}"
ROLE="${ROLE:?set ROLE: staging | production | demo}"
PUBLIC_URL="${PUBLIC_URL:?set PUBLIC_URL, e.g. http://192.168.178.63:8080}"
DIR="${DIR:-tapedeck}"          # relative to the remote user's home
SERVICE="${SERVICE:-tapedeck}"
PORT="${PORT:-8080}"
SSHOPTS="-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10"

case "$ROLE" in
  staging)            REPLICA_LINE='TAPEDECK_REPLICA=1' ;;
  production|demo)    REPLICA_LINE='# TAPEDECK_REPLICA is unset — this instance acts on the world' ;;
  *) echo "ROLE must be staging, production or demo (got '$ROLE')"; exit 1 ;;
esac

UNIT="$(dirname "$0")/tapedeck-user.service"
test -f "$UNIT" || { echo "cannot find $UNIT"; exit 1; }

echo "→ ${HOST}: ${ROLE}, ~/${DIR}, port ${PORT}"

# ── The unit ────────────────────────────────────────────────────────────────
# A *user* service, so the deploy needs no root and no sudo — it only ever has
# SSH-key access as this account, which is the whole reason the CI deploy works
# the way it does.
scp $SSHOPTS "$UNIT" "${HOST}:/tmp/tapedeck-user.service"

ssh $SSHOPTS "$HOST" bash -s -- "$DIR" "$SERVICE" "$PORT" "$PUBLIC_URL" "$REPLICA_LINE" <<'REMOTE'
set -euo pipefail
DIR="$1"; SERVICE="$2"; PORT="$3"; PUBLIC_URL="$4"; REPLICA_LINE="$5"

mkdir -p "$HOME/$DIR" "$HOME/.config/systemd/user"
mv -f /tmp/tapedeck-user.service "$HOME/.config/systemd/user/${SERVICE}.service"

# ── Linger ──────────────────────────────────────────────────────────────────
# Without it the user manager stops when the last session ends, so the service
# dies whenever you log out and never starts at boot. It is also what creates
# /run/user/$(id -u), which `systemctl --user` over a non-login SSH needs — the
# deploy step exports XDG_RUNTIME_DIR pointing at exactly that.
# This runs over ssh with no TTY, so sudo cannot prompt for a password — it
# fails with "no tty present and no askpass program specified". So ask rather
# than assume, and say exactly what to run when we cannot do it ourselves.
NEEDS_ROOT=""
if [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null || echo no)" = yes ]; then
  echo "  linger: already enabled"
elif sudo -n true 2>/dev/null; then
  sudo -n loginctl enable-linger "$USER"
  echo "  linger: enabled"
else
  echo "  ❌ linger is off and sudo needs a password over this connection."
  NEEDS_ROOT="loginctl enable-linger $USER"
fi

export XDG_RUNTIME_DIR="/run/user/$(id -u)"
systemctl --user daemon-reload
systemctl --user enable "$SERVICE" >/dev/null
echo "  unit: installed and enabled (not started — no binary yet)"

# ── .env ────────────────────────────────────────────────────────────────────
ENV_FILE="$HOME/$DIR/.env"
if [ -f "$ENV_FILE" ]; then
  echo "  .env: already present, left alone"
  grep -q '^TAPEDECK_REPLICA=1' "$ENV_FILE" \
    && echo "        (replica mode is on)" \
    || echo "        (replica mode is OFF — check this is what you meant)"
else
  command -v openssl >/dev/null || { echo "  ❌ openssl is needed to generate the key"; exit 1; }
  KEY="$(openssl rand -base64 32)"
  cat > "$ENV_FILE" <<ENV
# Written by deploy/bootstrap-host.sh. Safe to edit; never regenerate the key
# below — it encrypts every stored credential in the database.
$REPLICA_LINE

PUBLIC_URL=$PUBLIC_URL
TAPEDECK_SECRET_KEY=$KEY
DATABASE_URL=scrobbles.db
PORT=$PORT
RUST_LOG=info
ENV
  chmod 600 "$ENV_FILE"
  echo "  .env: written, key generated, mode 600"
fi

# ── Reachability ────────────────────────────────────────────────────────────
# The deploy's health check curls 127.0.0.1, so it passes whether or not the
# port is open to the LAN — which means a firewall would let every deploy
# succeed while nobody could open the page. Fedora Server runs firewalld by
# default and blocks 8080.
command -v curl >/dev/null || echo "  ⚠ curl is missing — the deploy health check will fail"
if command -v firewall-cmd >/dev/null && systemctl is-active --quiet firewalld; then
  if sudo -n firewall-cmd --query-port="${PORT}/tcp" >/dev/null 2>&1; then
    echo "  firewall: ${PORT}/tcp already open"
  elif sudo -n true 2>/dev/null; then
    sudo -n firewall-cmd --add-port="${PORT}/tcp" --permanent >/dev/null
    sudo -n firewall-cmd --reload >/dev/null
    echo "  firewall: opened ${PORT}/tcp"
  else
    NEEDS_ROOT="${NEEDS_ROOT:+$NEEDS_ROOT
}firewall-cmd --add-port=${PORT}/tcp --permanent && firewall-cmd --reload"
  fi
fi

if [ -n "$NEEDS_ROOT" ]; then
  echo
  echo "  ⚠ Finish on the host as root, then re-run this script:"
  printf '      %s\n' "$NEEDS_ROOT"
  # Linger is not optional: without it the user manager stops with your session,
  # so the service never starts at boot, and /run/user/$(id -u) — which the
  # deploy step needs for `systemctl --user` — may not exist at all.
  case "$NEEDS_ROOT" in *loginctl*) exit 1 ;; esac
fi
REMOTE

echo "✅ ${HOST} is ready. Deploy to it, then it will start on its own."
