#!/usr/bin/env bash
# Cross-compile Go reference API to a static Linux binary.
# Works from macOS, Windows (Git Bash), or Linux.
set -euo pipefail

cd "$(dirname "$0")"

GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -o server .

echo "Built: server ($(du -h server | cut -f1) static linux/amd64)"
