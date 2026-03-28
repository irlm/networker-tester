#!/usr/bin/env bash
# Deploy Go reference API to a benchmark target host.
# Usage: ./deploy.sh <host> [ssh-key]
set -euo pipefail

HOST="${1:?Usage: deploy.sh <host> [ssh-key]}"
SSH_KEY="${2:-}"
SSH_OPTS=(-o StrictHostKeyChecking=no -o ConnectTimeout=10)
[[ -n "$SSH_KEY" ]] && SSH_OPTS+=(-i "$SSH_KEY")

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SHARED_DIR="$(cd "$SCRIPT_DIR/../../shared" && pwd)"

# Build if binary doesn't exist
if [[ ! -f "$SCRIPT_DIR/server" ]]; then
    echo "Building Go binary..."
    bash "$SCRIPT_DIR/build.sh"
fi

echo "Deploying Go reference API to $HOST..."

# Ensure target directory and certs exist
ssh "${SSH_OPTS[@]}" "root@$HOST" "mkdir -p /opt/bench"

# Copy binary
scp "${SSH_OPTS[@]}" "$SCRIPT_DIR/server" "root@$HOST:/opt/bench/go-server"

# Generate and copy TLS certs if not already present
ssh "${SSH_OPTS[@]}" "root@$HOST" "test -f /opt/bench/cert.pem" 2>/dev/null || {
    echo "Generating TLS certificates on host..."
    scp "${SSH_OPTS[@]}" "$SHARED_DIR/generate-cert.sh" "root@$HOST:/opt/bench/"
    ssh "${SSH_OPTS[@]}" "root@$HOST" "bash /opt/bench/generate-cert.sh"
}

# Stop any existing instance and start fresh
ssh "${SSH_OPTS[@]}" "root@$HOST" bash <<'REMOTE'
pkill -f /opt/bench/go-server || true
sleep 1
chmod +x /opt/bench/go-server
nohup /opt/bench/go-server > /opt/bench/go-server.log 2>&1 &
sleep 1
if pgrep -f /opt/bench/go-server > /dev/null; then
    echo "Go reference API running on $HOSTNAME:8443"
else
    echo "ERROR: Go server failed to start" >&2
    cat /opt/bench/go-server.log >&2
    exit 1
fi
REMOTE
