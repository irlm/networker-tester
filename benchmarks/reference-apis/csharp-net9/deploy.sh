#!/usr/bin/env bash
# Deploy the C# .NET 9 reference API to a remote VM.
#
# Usage:
#   ./deploy.sh <user@host> [--cert-dir /path/to/certs]
#
# Expects:
#   - ./publish/ directory (run build.sh first)
#   - Certificate files (cert.pem, key.pem) either in --cert-dir or /opt/bench/
#
# The server listens on port 8443 HTTPS (HTTP/1.1 + HTTP/2).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REMOTE_DIR="/opt/bench/csharp-net9"
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

if [[ ! -d "$SCRIPT_DIR/publish" ]]; then
    echo "Error: ./publish/ not found. Run build.sh first."
    exit 1
fi

echo "==> Deploying to $TARGET:$REMOTE_DIR"

# Create remote directory and stop any existing instance
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR && sudo mkdir -p $CERT_DIR && sudo pkill -f 'csharp-net9' || true"

# Copy published binary
scp -r "$SCRIPT_DIR/publish/"* "$TARGET:/tmp/csharp-net9-deploy/"
ssh "$TARGET" "sudo mv /tmp/csharp-net9-deploy/* $REMOTE_DIR/ && rm -rf /tmp/csharp-net9-deploy"

# Copy certs if they exist locally
if [[ -f "$CERT_DIR/cert.pem" && -f "$CERT_DIR/key.pem" ]]; then
    scp "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" "$TARGET:/tmp/"
    ssh "$TARGET" "sudo mv /tmp/cert.pem /tmp/key.pem $CERT_DIR/"
fi

# Start server
ssh "$TARGET" "sudo chmod +x $REMOTE_DIR/csharp-net9 && \
    sudo BENCH_CERT_PATH=$CERT_DIR/cert.pem \
         BENCH_KEY_PATH=$CERT_DIR/key.pem \
         BENCH_PORT=8443 \
         nohup $REMOTE_DIR/csharp-net9 > /var/log/csharp-net9.log 2>&1 &"

echo "==> Server started on $TARGET:8443"
echo "    Verify: curl -k https://$TARGET:8443/health"
