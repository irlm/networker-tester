#!/bin/sh
# generate-download-files.sh — Pre-generate /download/{size} payload files for
# the nginx baseline.
#
# API-SPEC.md §5.2: every byte is 0x42 ('B'). Files are named by byte count
# and served by nginx via sendfile (root /opt/bench + URI /download/<size>).
#
# POSIX sh on purpose: runs under dash, busybox (Alpine Docker image), and
# bash (orchestrator/install.sh invoke it with `bash`).
#
# Usage:
#   ./generate-download-files.sh [output-dir]
#
# Default output: /opt/bench/download/
set -eu

OUTPUT_DIR="${1:-/opt/bench/download}"
# 1024 + 65536 are validated byte-exactly by the orchestrator (§5.2);
# 1048576 is the browser-benchmark payload; 0 covers the lower clamp bound;
# 10485760 gives a 10 MiB throughput tier. Add sizes here as needed.
SIZES="0 1024 8192 65536 1048576 10485760"

mkdir -p "$OUTPUT_DIR"

for size in $SIZES; do
  echo "Generating ${size}-byte file..."
  if [ "$size" -eq 0 ]; then
    # BSD head rejects `-c 0`
    : > "$OUTPUT_DIR/$size"
  else
    head -c "$size" /dev/zero | tr '\0' 'B' > "$OUTPUT_DIR/$size"
  fi
done

echo "Download files generated in $OUTPUT_DIR:"
ls -l "$OUTPUT_DIR"
