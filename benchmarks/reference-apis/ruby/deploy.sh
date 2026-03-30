#!/usr/bin/env bash
# Deploy the Ruby reference API to a remote VM.
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
REMOTE_DIR="/opt/bench/ruby"
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

echo "==> Installing Ruby 3.3 on $TARGET"
ssh "$TARGET" "sudo apt-get update -qq && \
    sudo apt-get install -y -qq ruby3.3 ruby3.3-dev build-essential libssl-dev < /dev/null && \
    sudo gem install bundler --no-document < /dev/null"

echo "==> Deploying to $TARGET:$REMOTE_DIR"

# Create remote directory and stop any existing instance
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR && sudo mkdir -p $CERT_DIR && sudo pkill -f 'puma.*config.ru' || true"

# Copy application files
ssh "$TARGET" "mkdir -p /tmp/ruby-deploy"
scp "$SCRIPT_DIR/config.ru" "$SCRIPT_DIR/Gemfile" "$SCRIPT_DIR/puma.rb" "$TARGET:/tmp/ruby-deploy/"
ssh "$TARGET" "sudo mv /tmp/ruby-deploy/* $REMOTE_DIR/ && rm -rf /tmp/ruby-deploy"

# Install dependencies
ssh "$TARGET" "cd $REMOTE_DIR && sudo bundle install --quiet"

# Copy certs if they exist locally
if [[ -f "$CERT_DIR/cert.pem" && -f "$CERT_DIR/key.pem" ]]; then
    scp "$CERT_DIR/cert.pem" "$CERT_DIR/key.pem" "$TARGET:/tmp/"
    ssh "$TARGET" "sudo mv /tmp/cert.pem /tmp/key.pem $CERT_DIR/"
fi

# Start server
ssh "$TARGET" "cd $REMOTE_DIR && \
    sudo nohup bundle exec puma -C puma.rb config.ru \
        > /var/log/ruby-bench.log 2>&1 &"

echo "==> Server started on $TARGET:8443"
echo "    Verify: curl -k https://$TARGET:8443/health"
