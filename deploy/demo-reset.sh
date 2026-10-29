#!/usr/bin/env bash
#
# Nightly reset of the public demo. Runs on the demo host from a user timer
# (deploy/tapedeck-demo-reset.{service,timer}), as the account that owns the
# `tapedeck` user service.
#
# Lives in ~/tapedeck/demo/ beside its inputs:
#   golden.db     a scrobble database holding only the `demo` admin, no listens
#                 and no tokens — taken with GET /api/v1/backup. Restoring it
#                 rather than re-running /setup means there is never a window in
#                 which a stranger can claim the instance.
#   history.json  tools/demo-data output, one listen per line
#   password      the demo password (it is published on the website; 0600 anyway)
#
# Only scrobbles.db is replaced. tapedeck.db holds the MusicBrainz/artwork
# caches, so after the first night enrichment is cache hits, not API calls.
#
# The container has no python3, so the generator cannot run here. Instead the
# history is shifted by whole days to end yesterday (UTC) — the same thing
# `generate.py --end yesterday` would produce, minus re-rolling the dice.

set -euo pipefail
cd "$HOME/tapedeck"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
B=http://127.0.0.1:8080
JAR=$(mktemp); HIST=$(mktemp); trap 'rm -f "$JAR" "$HIST"' EXIT

last=$(awk -F'"listened_at":' 'NF>1 {split($2,a,","); if (a[1]>m) m=a[1]} END {print m}' demo/history.json)
shift=$(( ($(date -u +%s) / 86400 - 1 - last / 86400) * 86400 ))
awk -v s="$shift" -F'"listened_at":' -v OFS='"listened_at":' \
  'NF>1 {n=$2+0; sub(/^[0-9]+/, sprintf("%.0f", n+s), $2)} {print}' demo/history.json > "$HIST"

systemctl --user stop tapedeck
rm -f scrobbles.db-wal scrobbles.db-shm
cp demo/golden.db scrobbles.db
systemctl --user start tapedeck
for _ in $(seq 60); do curl -sf "$B/health" >/dev/null && break; sleep 2; done

curl -sf -c "$JAR" -H 'content-type: application/json' \
  -d "{\"username\":\"demo\",\"password\":\"$(cat demo/password)\"}" "$B/api/v1/auth/login" >/dev/null

job=$(curl -sf -b "$JAR" -F "file=@$HIST;filename=demo-history.json" "$B/api/v1/import/file" | grep -o '"job_id":"[^"]*"' | cut -d'"' -f4)
for _ in $(seq 120); do
  curl -sf -b "$JAR" "$B/api/v1/import/jobs/$job" | grep -q '"state":"running"' || break
  sleep 5
done

# Imports bypass assign_session, so the listens arrive ungrouped.
curl -sf -b "$JAR" -X POST -H 'content-type: application/json' -d '{}' "$B/api/v1/sessions/rebuild" >/dev/null
echo "🔄 demo reset: history shifted $((shift / 86400)) days, import $job"
