#!/usr/bin/env bash
# deploy.sh — Install dependencies, copy source, build on VM, and start the C++ server.
#
# Usage:
#   ./deploy.sh <user@host> [--port 8443]
#
# Prerequisites:
#   - SSH key access to the target VM
#   - TLS cert/key at /opt/bench/cert.pem and /opt/bench/key.pem on the VM
#     (or run benchmarks/shared/generate-cert.sh and scp them first)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REMOTE_DIR="/opt/alethabench/cpp"
PORT=8443

if [ $# -lt 1 ]; then
    echo "Usage: $0 <user@host> [--port 8443]"
    exit 1
fi

TARGET="$1"; shift
while [ $# -gt 0 ]; do
    case "$1" in
        --port) PORT="$2"; shift 2 ;;
        *)      echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "==> Installing build dependencies on $TARGET"
ssh "$TARGET" "sudo apt-get update -qq && sudo apt-get install -y -qq \
    build-essential cmake libboost-system-dev libboost-dev libssl-dev" < /dev/null

echo "==> Creating remote directory $REMOTE_DIR"
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR && sudo chown \$(whoami) $REMOTE_DIR" < /dev/null

echo "==> Copying source files"
scp "$SCRIPT_DIR/server.cpp" "$SCRIPT_DIR/CMakeLists.txt" "$SCRIPT_DIR/build.sh" "$TARGET:$REMOTE_DIR/"

echo "==> Building on target (Release -O3)"
ssh "$TARGET" "cd $REMOTE_DIR && bash build.sh" < /dev/null

echo "==> Stopping any existing instance"
ssh "$TARGET" "pkill -f '$REMOTE_DIR/build/server' || true" < /dev/null
sleep 1

echo "==> Starting C++ server (port $PORT)"
ssh "$TARGET" "BENCH_PORT=$PORT nohup $REMOTE_DIR/build/server \
    > $REMOTE_DIR/server.log 2>&1 &" < /dev/null

echo "==> Waiting for health check..."
sleep 2
if ssh "$TARGET" "curl -sk https://127.0.0.1:$PORT/health" < /dev/null | grep -q ok; then
    echo "==> C++ Boost.Beast server is running on $TARGET:$PORT"
else
    echo "WARNING: Health check did not return 'ok'. Check $REMOTE_DIR/server.log on $TARGET"
fi
