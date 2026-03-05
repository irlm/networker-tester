#!/usr/bin/env bash
# Entrypoint for networker integration tests.
# Usage: ./run.sh [azure|aws|all]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TARGET="${1:-all}"

run_suite() {
    local suite="$1"
    echo ""
    echo "=== Running $suite integration tests ==="
    if compgen -G "${SCRIPT_DIR}/${suite}/*.bats" > /dev/null 2>&1; then
        bats "${SCRIPT_DIR}/${suite}/"*.bats
    else
        echo "  (no .bats files in ${suite}/ yet)"
    fi
}

case "$TARGET" in
    azure) run_suite azure ;;
    aws)   run_suite aws   ;;
    all)   run_suite azure; run_suite aws ;;
    *)
        echo "Usage: $0 [azure|aws|all]" >&2
        exit 1
        ;;
esac
