#!/usr/bin/env bash
# deploy.sh — Install nginx mainline on a target VM, copy config + cert +
#             download payloads, apply the API-SPEC.md env knobs, start nginx.
#
# Usage:
#   ./deploy.sh <user@host>
#
# Env knobs (API-SPEC.md §1, §3) applied to the remote nginx.conf:
#   BENCH_WORKERS   → worker_processes   (default: auto = all logical CPUs)
#   BENCH_PORT      → listen port        (default: 8443)
#   BENCH_API_TOKEN → bearer-token auth on every route except /health
#                     (token must not contain quotes or spaces)
#
# Prerequisites:
#   - Generate the cert first: bash benchmarks/shared/generate-cert.sh
#   - SSH key access to the target VM (Ubuntu)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CERT_DIR="$REPO_ROOT/benchmarks/shared"
REMOTE_DIR="/opt/bench"

BENCH_WORKERS="${BENCH_WORKERS:-}"
BENCH_PORT="${BENCH_PORT:-8443}"
BENCH_API_TOKEN="${BENCH_API_TOKEN:-}"

# ── Parse arguments ──────────────────────────────────────────────────────────
if [ $# -lt 1 ]; then
  echo "Usage: $0 <user@host>"
  exit 1
fi

TARGET="$1"; shift

# ── Validate local files ────────────────────────────────────────────────────
if [ ! -f "$CERT_DIR/cert.pem" ] || [ ! -f "$CERT_DIR/key.pem" ]; then
  echo "ERROR: TLS certificate not found in $CERT_DIR"
  echo "Generate first: bash benchmarks/shared/generate-cert.sh"
  exit 1
fi

# ── Install nginx mainline on remote ────────────────────────────────────────
# nginx.conf uses `http2 on` (1.25.1+) and `listen ... quic` (1.25+), so stock
# Ubuntu nginx (1.24, no QUIC) is not enough. This mirrors the orchestrator
# (deployer.rs) and install.sh, which also install mainline from nginx.org.
echo "==> Installing nginx mainline (1.25+) on $TARGET"
ssh "$TARGET" "if nginx -v 2>&1 | grep -qE 'nginx/1\\.2[5-9]|nginx/1\\.[3-9]'; then \
    echo 'nginx mainline already installed'; \
  else \
    sudo apt-get update -qq && sudo apt-get install -y -qq curl gnupg2 ca-certificates lsb-release && \
    curl -fsSL https://nginx.org/keys/nginx_signing.key | sudo gpg --dearmor -o /usr/share/keyrings/nginx-archive-keyring.gpg 2>/dev/null && \
    echo \"deb [signed-by=/usr/share/keyrings/nginx-archive-keyring.gpg] http://nginx.org/packages/mainline/ubuntu \$(lsb_release -cs) nginx\" \
      | sudo tee /etc/apt/sources.list.d/nginx.list > /dev/null && \
    sudo apt-get update -qq && sudo apt-get install -y -qq nginx; \
  fi" < /dev/null

# ── Create directories ───────────────────────────────────────────────────────
echo "==> Creating remote directories"
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR/download && sudo chown -R \$(whoami) $REMOTE_DIR" < /dev/null

# ── Copy certificates ────────────────────────────────────────────────────────
echo "==> Copying TLS certificates"
scp "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" "$TARGET:$REMOTE_DIR/"

# ── Copy nginx config ────────────────────────────────────────────────────────
echo "==> Copying nginx.conf"
scp "$SCRIPT_DIR/nginx.conf" "$TARGET:/tmp/bench-nginx.conf"
ssh "$TARGET" "sudo cp /tmp/bench-nginx.conf /etc/nginx/nginx.conf" < /dev/null

# ── Apply env knobs to the remote config (API-SPEC.md §1, §3) ────────────────
if [ -n "$BENCH_WORKERS" ]; then
  echo "==> Setting worker_processes $BENCH_WORKERS (BENCH_WORKERS)"
  ssh "$TARGET" "sudo sed -i 's/^worker_processes .*/worker_processes $BENCH_WORKERS;/' /etc/nginx/nginx.conf" < /dev/null
fi
if [ "$BENCH_PORT" != "8443" ]; then
  echo "==> Setting listen port $BENCH_PORT (BENCH_PORT)"
  ssh "$TARGET" "sudo sed -i 's/listen 8443/listen $BENCH_PORT/g; s/:8443\"/:$BENCH_PORT\"/g' /etc/nginx/nginx.conf" < /dev/null
fi
if [ -n "$BENCH_API_TOKEN" ]; then
  echo "==> Enabling bearer-token auth (BENCH_API_TOKEN)"
  ssh "$TARGET" "printf '\"Bearer %s\" 0;\n~^ 1;\n' '$BENCH_API_TOKEN' | sudo tee /etc/nginx/bench-auth-token.conf > /dev/null" < /dev/null
else
  ssh "$TARGET" "sudo rm -f /etc/nginx/bench-auth-token.conf" < /dev/null
fi

# ── Generate download files on remote ─────────────────────────────────────────
echo "==> Copying and running download-file generator"
scp "$SCRIPT_DIR/generate-download-files.sh" "$TARGET:$REMOTE_DIR/"
ssh "$TARGET" "chmod +x $REMOTE_DIR/generate-download-files.sh && $REMOTE_DIR/generate-download-files.sh $REMOTE_DIR/download" < /dev/null

# ── Restart nginx ─────────────────────────────────────────────────────────────
echo "==> Restarting nginx"
ssh "$TARGET" "sudo nginx -t && sudo systemctl restart nginx" < /dev/null

# ── Health check ──────────────────────────────────────────────────────────────
echo "==> Waiting for health check..."
sleep 2
if ssh "$TARGET" "curl -sk https://127.0.0.1:$BENCH_PORT/health" < /dev/null \
    | grep -q '"status":"ok"'; then
  echo "==> nginx baseline is running on $TARGET:$BENCH_PORT"
else
  echo "WARNING: Health check did not return status ok. Check nginx logs on $TARGET"
  echo "  sudo journalctl -u nginx --no-pager -n 20"
  echo "  sudo nginx -t"
fi
