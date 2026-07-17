#!/usr/bin/env bash
# Deploy the Python reference API to a remote VM.
#
# Usage:
#   ./deploy.sh <user@host> [--cert-dir /path/to/certs]
#
# Expects:
#   - Certificate files (cert.pem, key.pem) either in --cert-dir or /opt/bench/
#
# The server listens on port 8443 HTTPS (HTTP/1.1).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REMOTE_DIR="/opt/bench/python"
CERT_DIR="/opt/bench"

if [[ $# -lt 1 ]]; then
    echo "Usage: $0 <user@host> [--cert-dir /path/to/certs]"
    exit 1
fi

TARGET="$1"
shift

while [[ $# -gt 0 ]]; do
    case "$1" in
        --cert-dir) CERT_DIR="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

echo "==> Installing Python 3.12 on $TARGET"
ssh "$TARGET" "sudo apt-get update -qq && sudo apt-get install -y -qq python3.12 python3.12-venv python3-pip < /dev/null"

echo "==> Deploying to $TARGET:$REMOTE_DIR"

# Create remote directory and stop any existing instance
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR && sudo mkdir -p $CERT_DIR && sudo pkill -f 'uvicorn server:app' || true"

# Copy application files
scp "$SCRIPT_DIR/server.py" "$SCRIPT_DIR/requirements.txt" "$TARGET:/tmp/python-deploy/"
ssh "$TARGET" "sudo mv /tmp/python-deploy/* $REMOTE_DIR/ && rm -rf /tmp/python-deploy"

# Create venv and install dependencies
ssh "$TARGET" "cd $REMOTE_DIR && \
    sudo python3.12 -m venv venv && \
    sudo venv/bin/pip install --quiet -r requirements.txt"

# Copy certs if they exist locally
if [[ -f "$CERT_DIR/cert.pem" && -f "$CERT_DIR/key.pem" ]]; then
    scp "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" "$TARGET:/tmp/"
    ssh "$TARGET" "sudo mv /tmp/cert.pem /tmp/key.pem $CERT_DIR/"
fi

# Start server. BENCH_WORKERS maps to uvicorn --workers (API-SPEC.md §3,
# default = logical CPUs); --no-access-log so prod matches CI (audit F11).
# Cert-absent on the target → plain HTTP on the same port (application mode
# behind a TLS-terminating reverse proxy, audit F8).
ssh "$TARGET" "cd $REMOTE_DIR && \
    if [ -f $CERT_DIR/cert.pem ] && [ -f $CERT_DIR/key.pem ]; then \
        SSL_OPTS='--ssl-keyfile $CERT_DIR/key.pem --ssl-certfile $CERT_DIR/cert.pem'; \
    else \
        echo 'no TLS certs in $CERT_DIR - starting plain HTTP on port 8443 (application mode)'; \
        SSL_OPTS=''; \
    fi && \
    sudo nohup venv/bin/uvicorn server:app \
        --host 0.0.0.0 \
        --port 8443 \
        --workers \"\${BENCH_WORKERS:-\$(nproc)}\" \
        --no-access-log \
        \$SSL_OPTS \
        > /var/log/python-bench.log 2>&1 &"

echo "==> Server started on $TARGET:8443"
echo "    Verify: curl -k https://$TARGET:8443/health"
