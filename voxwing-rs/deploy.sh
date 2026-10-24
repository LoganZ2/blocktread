#!/usr/bin/env bash
# Deploy the dedicated server to a Linux machine over SSH: sync the source, build it there and
# (re)start it in a tmux session named "voxwing". The machine needs a C compiler (build-essential)
# and Rust in the user's home (~/.cargo); its firewall must allow UDP 4433.
#
#   ./deploy.sh user@host
#   ./deploy.sh user@host --queue-wait 30 --match-secs 600
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 user@host [server args: --queue-wait S --match-secs S --kill-limit N]" >&2
  exit 2
fi
HOST="$1"
shift
ARGS="$*"

cd "$(dirname "$0")"
echo "sync → $HOST:~/voxwing-rs"
rsync -az --delete --exclude 'target/' ./ "$HOST:voxwing-rs/"

echo "build and restart on $HOST ($ARGS)"
ssh "$HOST" "set -e
cd ~/voxwing-rs
~/.cargo/bin/cargo build --release --locked -p voxwing-server
tmux kill-session -t voxwing 2>/dev/null || true
tmux new-session -d -s voxwing 'cd ~/voxwing-rs && ./target/release/voxwing-server $ARGS --bind 0.0.0.0 --port 4433 2>&1 | tee -a ~/voxwing-server.log'
sleep 1
tail -3 ~/voxwing-server.log"
