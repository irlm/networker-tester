#!/usr/bin/env bash
# Generate a self-signed TLS certificate for AletheBench servers.
# Output: cert.pem + key.pem in the same directory as this script.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

openssl req -x509 -newkey rsa:2048 \
  -keyout "$SCRIPT_DIR/key.pem" \
  -out    "$SCRIPT_DIR/cert.pem" \
  -days 3650 -nodes \
  -subj "/CN=alethabench"

echo "Certificate written to $SCRIPT_DIR/cert.pem"
echo "Private key written to $SCRIPT_DIR/key.pem"
