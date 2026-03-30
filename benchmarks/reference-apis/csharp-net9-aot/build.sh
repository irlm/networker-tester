#!/usr/bin/env bash
# Build the C# .NET 9 AOT reference API.
# IMPORTANT: Native AOT does not support cross-compilation.
# This MUST be run on the target OS (e.g., linux-x64 build on Linux).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

dotnet publish -c Release -r linux-x64 -o ./publish

echo "Published AOT binary to $SCRIPT_DIR/publish/"
ls -lh ./publish/csharp-net9-aot
