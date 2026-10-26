#!/usr/bin/env bash
# One-time setup on the game server, run as the user the server runs under (not root):
#   bash setup.sh
# Installs the systemd user service and lets it run without anyone logged in (one sudo).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
base="$HOME/blocktread-server"
mkdir -p "$base/releases" "$HOME/.config/systemd/user"
cp "$here/blocktread-server.service" "$HOME/.config/systemd/user/"
[ -f "$base/args.env" ] || echo 'ARGS=' > "$base/args.env"
# keep user services running at boot and after logout
sudo loginctl enable-linger "$USER"
systemctl --user daemon-reload
systemctl --user enable blocktread-server
echo "ready: the first deploy puts a release in $base/releases and starts the service"
if command -v ufw >/dev/null && sudo ufw status | grep -q "Status: active"; then
  sudo ufw allow 4433/udp
fi
