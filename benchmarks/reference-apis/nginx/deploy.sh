#!/usr/bin/env bash
# deploy.sh — Install nginx on a target VM, copy config + cert + static files,
#              generate download blobs, and start nginx.
#
# Usage:
#   ./deploy.sh <user@host>
#
# Prerequisites:
#   - Generate the cert first: bash benchmarks/shared/generate-cert.sh
#   - SSH key access to the target VM
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CERT_DIR="$REPO_ROOT/benchmarks/shared"
REMOTE_DIR="/opt/bench"

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

# ── Install nginx on remote ─────────────────────────────────────────────────
echo "==> Installing nginx on $TARGET"
ssh "$TARGET" "sudo apt-get update -qq && sudo apt-get install -y -qq nginx" < /dev/null

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

# ── Generate download files on remote ─────────────────────────────────────────
echo "==> Copying and running download-file generator"
scp "$SCRIPT_DIR/generate-download-files.sh" "$TARGET:$REMOTE_DIR/"
ssh "$TARGET" "chmod +x $REMOTE_DIR/generate-download-files.sh && $REMOTE_DIR/generate-download-files.sh $REMOTE_DIR/download" < /dev/null

# ── Create upload temp directory ──────────────────────────────────────────────
ssh "$TARGET" "sudo mkdir -p /tmp/nginx_uploads && sudo chown www-data:www-data /tmp/nginx_uploads" < /dev/null

# ── Restart nginx ─────────────────────────────────────────────────────────────
echo "==> Restarting nginx"
ssh "$TARGET" "sudo nginx -t && sudo systemctl restart nginx" < /dev/null

# ── Health check ──────────────────────────────────────────────────────────────
echo "==> Waiting for health check..."
sleep 2
if ssh "$TARGET" "curl -sk https://127.0.0.1:8443/health" < /dev/null | grep -q ok; then
  echo "==> Nginx baseline is running on $TARGET:8443"
else
  echo "WARNING: Health check did not return 'ok'. Check nginx logs on $TARGET"
  echo "  sudo journalctl -u nginx --no-pager -n 20"
  echo "  sudo nginx -t"
fi
