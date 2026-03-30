#!/usr/bin/env bash
# Build a self-contained, trimmed binary for linux-x64.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

dotnet publish -c Release -r linux-x64 --self-contained -o ./publish

echo "Build complete. Binary: ./publish/csharp-net7"
ls -lh ./publish/csharp-net7
