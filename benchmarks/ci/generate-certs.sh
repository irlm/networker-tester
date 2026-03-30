#!/usr/bin/env bash
# Generate self-signed TLS certificates for CI benchmark runs.
# Produces cert.pem + key.pem in the specified output directory.
set -euo pipefail

OUT_DIR="${1:-$(pwd)}"
mkdir -p "$OUT_DIR"

openssl req -x509 -newkey rsa:2048 \
  -keyout "$OUT_DIR/key.pem" \
  -out    "$OUT_DIR/cert.pem" \
  -days 1 -nodes \
  -subj "/CN=localhost" \
  -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" \
  2>/dev/null

echo "Certificates written to $OUT_DIR/"
