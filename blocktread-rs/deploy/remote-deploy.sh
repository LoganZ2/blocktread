#!/usr/bin/env bash
# Run on the game server by CI for each deploy:
#   remote-deploy.sh <release name> [max seconds to wait for players to leave]
# The new binary has been uploaded to ~/blocktread-server/releases/<name>/blocktread-server, and the
# service file next to this script. Waits until nobody is online, switches
# ~/blocktread-server/current to it and restarts; if the new version doesn't come up, switches back.
#
# A server still set up under the game's old name (~/voxwing-server, the voxwing-server service)
# moves over on its first deploy: args.env comes along, the new service takes the old one's place,
# and the old one comes back if the release doesn't start. ~/voxwing-server itself stays as it was.
set -euo pipefail
name="$1"; wait_max="${2:-1200}"
base="$HOME/blocktread-server"; status="$base/status.json"
old="$HOME/voxwing-server"
new="$base/releases/$name/blocktread-server"
chmod +x "$new"
"$new" --help >/dev/null  # it runs on this machine

active() { systemctl --user -q is-active "$1"; }
online() { grep -o '"connections":[0-9]*' "$1" 2>/dev/null | cut -d: -f2 || echo 0; }
# players on the server, whichever service is up (the old one, before the move)
players() {
  local n=0
  if active blocktread-server; then n=$((n + $(online "$status"))); fi
  if active voxwing-server; then n=$((n + $(online "$old/status.json"))); fi
  echo "$n"
}

# the service as this release has it, and its flags (from the old setup, the first time)
mkdir -p "$HOME/.config/systemd/user"
cp "$base/blocktread-server.service" "$HOME/.config/systemd/user/"
[ -f "$base/args.env" ] || cp "$old/args.env" "$base/args.env" 2>/dev/null || echo 'ARGS=' > "$base/args.env"
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
moved=false
if systemctl --user -q is-enabled voxwing-server 2>/dev/null || active voxwing-server; then
  echo "moving over from voxwing-server"
  systemctl --user disable --now voxwing-server
  moved=true
fi
systemctl --user enable blocktread-server
systemctl --user restart blocktread-server

# up: the service is active and has written a fresh status since the restart
for _ in $(seq 1 20); do
  sleep 1
  if active blocktread-server && [ -f "$status" ] && [ "$(stat -c %Y "$status")" -ge "$t0" ]; then
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
elif [ "$moved" = true ]; then
  # nothing to go back to under the new name: the old service as it was
  systemctl --user disable --now blocktread-server
  systemctl --user enable --now voxwing-server
fi
exit 1
