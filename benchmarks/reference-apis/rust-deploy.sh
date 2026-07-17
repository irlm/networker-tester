#!/usr/bin/env bash
# rust-deploy.sh — Copy the networker-endpoint binary + TLS cert + shared
#                  benchmark dataset to a target VM and start it.
#
# Usage:
#   ./rust-deploy.sh <user@host> [--port 8443] [--http-port 8080]
#
# Env knobs (API-SPEC.md §1–§3):
#   BENCH_WORKERS   → exported as TOKIO_WORKER_THREADS on the remote (tokio
#                     sizes its multi-thread runtime from it; verified against
#                     tokio 1.52.4). Default: all logical CPUs.
#   BENCH_PORT      → default for --port (HTTPS), i.e. the spec's 8443 knob.
#   BENCH_API_TOKEN → forwarded to the remote process (bearer auth on every
#                     route except /health). Must not contain quotes/spaces.
#
# The shared dataset benchmarks/reference-apis/shared/bench-data.json is
# always deployed to /opt/bench/bench-data.json and BENCH_DATA_PATH is set —
# per API-SPEC.md §2 a set-but-unloadable BENCH_DATA_PATH is fatal at startup,
# so a bad deploy fails loudly instead of benchmarking PRNG data.
#
# Prerequisites:
#   - Build the binary first:  cargo build --release -p networker-endpoint
#   - SSH key access to the target VM
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BINARY="$REPO_ROOT/target/release/networker-endpoint"
DATASET="$SCRIPT_DIR/shared/bench-data.json"
REMOTE_DIR="/opt/bench"

HTTPS_PORT="${BENCH_PORT:-8443}"
HTTP_PORT=8080
BENCH_WORKERS="${BENCH_WORKERS:-}"
BENCH_API_TOKEN="${BENCH_API_TOKEN:-}"

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

if [ ! -f "$DATASET" ]; then
  echo "ERROR: Shared dataset not found at $DATASET"
  echo "It is committed to the repo (API-SPEC.md §2) — check your checkout."
  exit 1
fi

# ── Build the remote environment prefix (API-SPEC.md §1–§3) ─────────────────
REMOTE_ENV="BENCH_DATA_PATH=$REMOTE_DIR/bench-data.json"
if [ -n "$BENCH_WORKERS" ]; then
  REMOTE_ENV="$REMOTE_ENV TOKIO_WORKER_THREADS=$BENCH_WORKERS"
fi
if [ -n "$BENCH_API_TOKEN" ]; then
  REMOTE_ENV="$REMOTE_ENV BENCH_API_TOKEN=$BENCH_API_TOKEN"
fi

# ── Deploy ───────────────────────────────────────────────────────────────────
echo "==> Creating remote directory $REMOTE_DIR on $TARGET"
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR && sudo chown \$(whoami) $REMOTE_DIR" < /dev/null

echo "==> Copying binary and shared dataset"
scp "$BINARY" "$TARGET:$REMOTE_DIR/networker-endpoint"
scp "$DATASET" "$TARGET:$REMOTE_DIR/bench-data.json"

echo "==> Stopping any existing instance"
ssh "$TARGET" "pkill -f 'networker-endpoint' || true" < /dev/null

echo "==> Starting networker-endpoint (HTTPS=$HTTPS_PORT, HTTP=$HTTP_PORT, env: $REMOTE_ENV)"
ssh "$TARGET" "nohup env $REMOTE_ENV $REMOTE_DIR/networker-endpoint \
  --https-port $HTTPS_PORT \
  --http-port $HTTP_PORT \
  > $REMOTE_DIR/endpoint.log 2>&1 &" < /dev/null

echo "==> Waiting for health check..."
sleep 2
if ssh "$TARGET" "curl -sk https://127.0.0.1:$HTTPS_PORT/health" < /dev/null \
    | grep -q '"runtime":"rust"'; then
  echo "==> Rust endpoint is running on $TARGET:$HTTPS_PORT"
else
  echo "WARNING: Health check did not return the §5.1 contract body."
  echo "Check $REMOTE_DIR/endpoint.log on $TARGET (a broken bench-data.json is fatal by design)"
  exit 1
fi
