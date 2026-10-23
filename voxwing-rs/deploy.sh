#!/usr/bin/env bash
# Deploy the dedicated server to a Linux machine over SSH: sync the source, build it there and
# (re)start it in a tmux session named "voxwing". The machine needs a C compiler (build-essential)
# and Rust in the user's home (~/.cargo); its firewall must allow UDP 4433.
#
#   ./deploy.sh                          # logan@10.0.0.10, co-op
#   ./deploy.sh logan@10.0.0.10 --mode ffa --bots 6
set -euo pipefail

HOST="${1:-logan@10.0.0.10}"
shift || true
ARGS="${*:---mode coop --bots 4}"

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
