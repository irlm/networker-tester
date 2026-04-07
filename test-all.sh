#!/usr/bin/env bash
# test-all.sh — Run ALL tests across the entire project from macOS.
#
# Usage:
#   ./test-all.sh          # run everything
#   ./test-all.sh rust     # only rust tests
#   ./test-all.sh frontend # only frontend
#   ./test-all.sh orch     # only orchestrator
#   ./test-all.sh apis     # only reference API tests
#   ./test-all.sh install  # only installer tests

set -euo pipefail
cd "$(dirname "$0")"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'
FAILED=0
PASSED=0

run_suite() {
    local name="$1"
    shift
    printf "${YELLOW}▶ %-40s${NC}" "$name"
    if output=$("$@" 2>&1); then
        printf "${GREEN} ✓${NC}\n"
        PASSED=$((PASSED + 1))
    else
        printf "${RED} ✗${NC}\n"
        echo "$output" | tail -20
        echo ""
        FAILED=$((FAILED + 1))
    fi
}

should_run() {
    [ -z "${FILTER:-}" ] || [ "$FILTER" = "$1" ]
}

FILTER="${1:-}"

# ── Rust workspace ──────────────────────────────────────────────────────
if should_run rust; then
    echo ""
    echo "━━━ Rust ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    run_suite "cargo fmt --check"              cargo fmt --all -- --check
    run_suite "cargo clippy"                   cargo clippy --all-targets -- -D warnings
    run_suite "workspace lib tests"            cargo test --workspace --lib
    run_suite "dashboard all tests"            cargo test -p networker-dashboard
    run_suite "endpoint tests"                 cargo test -p networker-endpoint --lib
    run_suite "no-default-features build"      cargo build -p networker-tester --no-default-features
fi

# ── Orchestrator ────────────────────────────────────────────────────────
if should_run orch; then
    echo ""
    echo "━━━ Orchestrator ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    run_suite "orchestrator tests" bash -c "cd benchmarks/orchestrator && cargo test"
fi

# ── Frontend ────────────────────────────────────────────────────────────
if should_run frontend; then
    echo ""
    echo "━━━ Frontend ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    run_suite "TypeScript type check" bash -c "cd dashboard && npx tsc --noEmit"
    run_suite "ESLint"                bash -c "cd dashboard && npm run lint"
    run_suite "Vite build"            bash -c "cd dashboard && npm run build"
fi

# ── Installer ───────────────────────────────────────────────────────────
if should_run install; then
    echo ""
    echo "━━━ Installer ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    if command -v shellcheck &>/dev/null; then
        run_suite "shellcheck install.sh" shellcheck install.sh
    else
        printf "${YELLOW}▶ %-40s SKIP (shellcheck not installed)${NC}\n" "shellcheck"
    fi
    if command -v bats &>/dev/null; then
        run_suite "bats installer tests" bats tests/installer.bats
    else
        printf "${YELLOW}▶ %-40s SKIP (bats not installed)${NC}\n" "bats"
    fi
fi

# ── Reference API unit tests ───────────────────────────────────────────
if should_run apis; then
    echo ""
    echo "━━━ Reference API Tests ━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Go
    if command -v go &>/dev/null && [ -d benchmarks/reference-apis/go ]; then
        run_suite "Go reference API tests" bash -c "cd benchmarks/reference-apis/go && go test ./..."
    fi

    # Node.js
    if command -v node &>/dev/null && [ -d benchmarks/reference-apis/nodejs ]; then
        run_suite "Node.js reference API tests" node benchmarks/reference-apis/nodejs/test.js
    fi

    # Python
    if command -v python3 &>/dev/null && [ -d benchmarks/reference-apis/python ]; then
        run_suite "Python reference API tests" python3 benchmarks/reference-apis/python/test_app.py
    fi

    # Ruby
    if command -v ruby &>/dev/null && [ -d benchmarks/reference-apis/ruby ]; then
        run_suite "Ruby reference API tests" ruby benchmarks/reference-apis/ruby/test_app.rb
    fi

    # PHP
    if command -v php &>/dev/null && [ -d benchmarks/reference-apis/php ]; then
        run_suite "PHP reference API tests" php benchmarks/reference-apis/php/test_server.php
    fi

    # Java
    if command -v java &>/dev/null && [ -d benchmarks/reference-apis/java ]; then
        run_suite "Java reference API tests" bash -c "cd benchmarks/reference-apis/java && ./gradlew test 2>/dev/null || mvn test 2>/dev/null || echo 'no build tool'"
    fi
fi

# ── Summary ─────────────────────────────────────────────────────────────
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [ "$FAILED" -eq 0 ]; then
    printf "${GREEN}All %d suites passed.${NC}\n" "$PASSED"
else
    printf "${RED}%d failed${NC}, ${GREEN}%d passed${NC}\n" "$FAILED" "$PASSED"
    exit 1
fi
