#!/usr/bin/env bash
# Build the C++ Boost.Beast reference server.
# Must run on the target OS (native compilation).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

mkdir -p build
cd build
cmake .. -DCMAKE_BUILD_TYPE=Release
make -j"$(nproc)"

echo "Build complete. Binary: ./build/server"
ls -lh server
