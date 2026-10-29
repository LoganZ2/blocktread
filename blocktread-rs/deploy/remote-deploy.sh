#!/usr/bin/env bash
# Run on the game server by CI for each deploy:
#   remote-deploy.sh <release name> [max seconds to wait for players to leave]
# The new binary has been uploaded to ~/blocktread-server/releases/<name>/blocktread-server, and the
# service file next to this script. Waits until nobody is online, switches
# ~/blocktread-server/current to it and restarts; if the new version doesn't come up, switches back.
set -euo pipefail
name="$1"; wait_max="${2:-1200}"
base="$HOME/blocktread-server"; status="$base/status.json"
new="$base/releases/$name/blocktread-server"
chmod +x "$new"
"$new" --help >/dev/null  # it runs on this machine

active() { systemctl --user -q is-active blocktread-server; }
# players on the server (none while it isn't running)
players() {
  local n=""
  if active; then n=$(grep -o '"connections":[0-9]*' "$status" 2>/dev/null | cut -d: -f2); fi
  echo "${n:-0}"
}

# the service as this release has it, and its flags
mkdir -p "$HOME/.config/systemd/user"
cp "$base/blocktread-server.service" "$HOME/.config/systemd/user/"
[ -f "$base/args.env" ] || echo 'ARGS=' > "$base/args.env"
systemctl --user daemon-reload

# never restart under players: wait for the server to empty, up to wait_max
start=$SECONDS
while [ "$(players)" != 0 ]; do
  if [ $((SECONDS - start)) -ge "$wait_max" ]; then
    echo "::error::$(players) still online after ${wait_max}s; not restarting. Release $name is uploaded; run the deploy again later."
    exit 1
  fi
  echo "$(players) online, waiting for them to leave..."
  sleep 15
done

prev="$(readlink "$base/current" || true)"
ln -sfn "$new" "$base/current.new" && mv -Tf "$base/current.new" "$base/current"
t0=$(date +%s)
systemctl --user enable blocktread-server
systemctl --user restart blocktread-server

# up: the service is active and has written a fresh status since the restart
for _ in $(seq 1 20); do
  sleep 1
  if active && [ -f "$status" ] && [ "$(stat -c %Y "$status")" -ge "$t0" ]; then
    echo "deployed $name: $(cat "$status")"
    # keep the last five releases
    ls -1dt "$base"/releases/*/ | tail -n +6 | xargs -r rm -rf
    exit 0
  fi
done

echo "::error::release $name did not come up; rolling back"
journalctl --user-unit blocktread-server -n 40 --no-pager || true
if [ -n "$prev" ]; then
  ln -sfn "$prev" "$base/current.new" && mv -Tf "$base/current.new" "$base/current"
  systemctl --user restart blocktread-server
fi
exit 1
