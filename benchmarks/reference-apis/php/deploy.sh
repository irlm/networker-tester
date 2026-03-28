#!/usr/bin/env bash
# Deploy the PHP (Swoole) reference API to a remote VM.
#
# Usage:
#   ./deploy.sh <user@host> [--cert-dir /path/to/certs]
#
# Expects:
#   - Certificate files (cert.pem, key.pem) either in --cert-dir or /opt/bench/
#   - Linux target (Swoole does not support macOS/Windows)
#
# The server listens on port 8443 HTTPS (HTTP/1.1).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REMOTE_DIR="/opt/bench/php"
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

echo "==> Installing PHP 8.3 + Swoole on $TARGET"
ssh "$TARGET" "sudo apt-get update -qq && \
    sudo apt-get install -y -qq php8.3-cli php8.3-dev php8.3-curl phpize libssl-dev < /dev/null && \
    sudo pecl install swoole < /dev/null && \
    echo 'extension=swoole.so' | sudo tee /etc/php/8.3/cli/conf.d/20-swoole.ini"

echo "==> Deploying to $TARGET:$REMOTE_DIR"

# Create remote directory and stop any existing instance
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR && sudo mkdir -p $CERT_DIR && sudo pkill -f 'php.*server.php' || true"

# Copy application files
ssh "$TARGET" "mkdir -p /tmp/php-deploy"
scp "$SCRIPT_DIR/server.php" "$TARGET:/tmp/php-deploy/"
ssh "$TARGET" "sudo mv /tmp/php-deploy/* $REMOTE_DIR/ && rm -rf /tmp/php-deploy"

# Copy certs if they exist locally
if [[ -f "$CERT_DIR/cert.pem" && -f "$CERT_DIR/key.pem" ]]; then
    scp "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" "$TARGET:/tmp/"
    ssh "$TARGET" "sudo mv /tmp/cert.pem /tmp/key.pem $CERT_DIR/"
fi

# Start server
ssh "$TARGET" "cd $REMOTE_DIR && \
    sudo nohup php server.php \
        > /var/log/php-bench.log 2>&1 &"

echo "==> Server started on $TARGET:8443"
echo "    Verify: curl -k https://$TARGET:8443/health"
