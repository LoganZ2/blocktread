#!/usr/bin/env bash
# Run on the game server by CI for each deploy:
#   remote-deploy.sh <release name> [max seconds to wait for players to leave]
# The new binary has been uploaded to ~/voxwing-server/releases/<name>/voxwing-server. Waits until
# nobody is online, switches ~/voxwing-server/current to it and restarts; if the new version doesn't
# come up, switches back.
set -euo pipefail
name="$1"; wait_max="${2:-1200}"
base="$HOME/voxwing-server"; status="$base/status.json"
new="$base/releases/$name/voxwing-server"
chmod +x "$new"
"$new" --help >/dev/null  # it runs on this machine

online() { grep -o '"connections":[0-9]*' "$status" 2>/dev/null | cut -d: -f2 || echo 0; }

# never restart under players: wait for the server to empty, up to wait_max
start=$SECONDS
while systemctl --user -q is-active voxwing-server && [ "$(online)" != 0 ]; do
  if [ $((SECONDS - start)) -ge "$wait_max" ]; then
    echo "::error::$(online) still online after ${wait_max}s; not restarting. Release $name is uploaded; run the deploy again later."
    exit 1
  fi
  echo "$(online) online, waiting for them to leave..."
  sleep 15
done

prev="$(readlink "$base/current" || true)"
ln -sfn "$new" "$base/current.new" && mv -Tf "$base/current.new" "$base/current"
t0=$(date +%s)
systemctl --user restart voxwing-server

# up: the service is active and has written a fresh status since the restart
for _ in $(seq 1 20); do
  sleep 1
  if systemctl --user -q is-active voxwing-server && [ -f "$status" ] && [ "$(stat -c %Y "$status")" -ge "$t0" ]; then
    echo "deployed $name: $(cat "$status")"
    # keep the last five releases
    ls -1dt "$base"/releases/*/ | tail -n +6 | xargs -r rm -rf
    exit 0
  fi
done

echo "::error::release $name did not come up; rolling back"
journalctl --user-unit voxwing-server -n 40 --no-pager || true
if [ -n "$prev" ]; then
  ln -sfn "$prev" "$base/current.new" && mv -Tf "$base/current.new" "$base/current"
  systemctl --user restart voxwing-server
fi
exit 1
