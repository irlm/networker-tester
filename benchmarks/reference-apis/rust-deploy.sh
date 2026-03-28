#!/usr/bin/env bash
# deploy.sh — Copy the networker-endpoint binary + TLS cert to a target VM and start it.
#
# Usage:
#   ./deploy.sh <user@host> [--port 8443] [--http-port 8080]
#
# Prerequisites:
#   - Build the binary first:  cargo build --release -p networker-endpoint
#   - Generate the cert first: ./benchmarks/shared/generate-cert.sh
#   - SSH key access to the target VM
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BINARY="$REPO_ROOT/target/release/networker-endpoint"
CERT_DIR="$REPO_ROOT/benchmarks/shared"
REMOTE_DIR="/opt/alethabench"

HTTPS_PORT=8443
HTTP_PORT=8080

# ── Parse arguments ──────────────────────────────────────────────────────────
if [ $# -lt 1 ]; then
  echo "Usage: $0 <user@host> [--port 8443] [--http-port 8080]"
  exit 1
fi

TARGET="$1"; shift
while [ $# -gt 0 ]; do
  case "$1" in
    --port)       HTTPS_PORT="$2"; shift 2 ;;
    --http-port)  HTTP_PORT="$2";  shift 2 ;;
    *)            echo "Unknown option: $1"; exit 1 ;;
  esac
done

# ── Validate local files ────────────────────────────────────────────────────
if [ ! -f "$BINARY" ]; then
  echo "ERROR: Binary not found at $BINARY"
  echo "Build first: cargo build --release -p networker-endpoint"
  exit 1
fi

if [ ! -f "$CERT_DIR/cert.pem" ] || [ ! -f "$CERT_DIR/key.pem" ]; then
  echo "ERROR: TLS certificate not found in $CERT_DIR"
  echo "Generate first: bash benchmarks/shared/generate-cert.sh"
  exit 1
fi

# ── Deploy ───────────────────────────────────────────────────────────────────
echo "==> Creating remote directory $REMOTE_DIR on $TARGET"
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR && sudo chown \$(whoami) $REMOTE_DIR" < /dev/null

echo "==> Copying binary and certificates"
scp "$BINARY" "$TARGET:$REMOTE_DIR/networker-endpoint"
scp "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" "$TARGET:$REMOTE_DIR/"

echo "==> Stopping any existing instance"
ssh "$TARGET" "pkill -f 'networker-endpoint' || true" < /dev/null

echo "==> Starting networker-endpoint (HTTPS=$HTTPS_PORT, HTTP=$HTTP_PORT)"
ssh "$TARGET" "nohup $REMOTE_DIR/networker-endpoint \
  --https-port $HTTPS_PORT \
  --http-port $HTTP_PORT \
  > $REMOTE_DIR/endpoint.log 2>&1 &" < /dev/null

echo "==> Waiting for health check..."
sleep 2
if ssh "$TARGET" "curl -sk https://127.0.0.1:$HTTPS_PORT/health" < /dev/null | grep -q ok; then
  echo "==> Rust endpoint is running on $TARGET:$HTTPS_PORT"
else
  echo "WARNING: Health check did not return 'ok'. Check $REMOTE_DIR/endpoint.log on $TARGET"
fi
