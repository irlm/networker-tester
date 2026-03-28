#!/usr/bin/env bash
# generate-download-files.sh — Pre-generate download files for the nginx baseline.
#
# Creates files named by byte count, filled with 0x42 ('B'), matching the
# pattern used by the application-level reference APIs.
#
# Usage:
#   ./generate-download-files.sh [output-dir]
#
# Default output: /opt/bench/download/
set -euo pipefail

OUTPUT_DIR="${1:-/opt/bench/download}"
SIZES=(1024 8192 65536 1048576)

mkdir -p "$OUTPUT_DIR"

for size in "${SIZES[@]}"; do
  echo "Generating ${size}-byte file..."
  # dd with bs=1 count=N reading from /dev/zero, then tr to convert 0x00 → 0x42
  dd if=/dev/zero bs=1 count="$size" 2>/dev/null | tr '\0' 'B' > "$OUTPUT_DIR/$size"
done

echo "Download files generated in $OUTPUT_DIR:"
ls -lh "$OUTPUT_DIR"
