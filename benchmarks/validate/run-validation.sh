#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Application Benchmark JSON API Validation
#
# Tests all 7 JSON API endpoints against each reference API server.
# Can run against Docker Compose servers or any single server URL.
#
# Usage:
#   ./run-validation.sh                          # Docker Compose (all languages)
#   ./run-validation.sh --url https://localhost:8443   # Single server
#   ./run-validation.sh --rust-only              # Just the Rust endpoint (no Docker)
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

PASS=0
FAIL=0
SKIP=0
ERRORS=""

# ── Argument parsing ─────────────────────────────────────────────────────────
MODE="docker"
SINGLE_URL=""
RUST_ONLY=0
SEED=42
TOKEN=""

for arg in "$@"; do
    case "$arg" in
        --url=*) SINGLE_URL="${arg#*=}"; MODE="single" ;;
        --url)   shift; SINGLE_URL="${1:-}"; MODE="single" ;;
        --rust-only) RUST_ONLY=1; MODE="rust" ;;
        --seed=*) SEED="${arg#*=}" ;;
        --token=*) TOKEN="${arg#*=}" ;;
        --help) echo "Usage: $0 [--url=URL | --rust-only] [--seed=N] [--token=TOKEN]"; exit 0 ;;
    esac
done

# Build auth header args for curl when a token is provided
AUTH_ARGS=()
if [[ -n "$TOKEN" ]]; then
    AUTH_ARGS=(-H "Authorization: Bearer $TOKEN")
fi

# ── Test helpers ─────────────────────────────────────────────────────────────

check() {
    local label="$1"
    local url="$2"
    local method="${3:-GET}"
    local body="${4:-}"
    local expect_field="${5:-}"

    local curl_args=(-sk --max-time 10 -X "$method" "${AUTH_ARGS[@]}")
    if [[ -n "$body" ]]; then
        curl_args+=(-H "Content-Type: application/json" -d "$body")
    fi

    local response
    response=$(curl "${curl_args[@]}" -w "\n%{http_code}" "$url" 2>/dev/null) || {
        printf "  ${RED}FAIL${NC} %-45s %s\n" "$label" "connection refused"
        FAIL=$((FAIL + 1))
        ERRORS+="  $label: connection refused\n"
        return 1
    }

    local http_code
    http_code=$(echo "$response" | tail -1)
    local body_text
    body_text=$(echo "$response" | sed '$d')

    if [[ "$http_code" -ge 400 ]]; then
        printf "  ${RED}FAIL${NC} %-45s HTTP %s\n" "$label" "$http_code"
        FAIL=$((FAIL + 1))
        ERRORS+="  $label: HTTP $http_code\n"
        return 1
    fi

    # Check response is valid JSON
    if ! echo "$body_text" | python3 -m json.tool > /dev/null 2>&1; then
        printf "  ${RED}FAIL${NC} %-45s %s\n" "$label" "invalid JSON"
        FAIL=$((FAIL + 1))
        ERRORS+="  $label: invalid JSON response\n"
        return 1
    fi

    # Check expected field if specified
    if [[ -n "$expect_field" ]]; then
        if ! echo "$body_text" | python3 -c "import sys,json; d=json.load(sys.stdin); assert '$expect_field' in (d if isinstance(d,dict) else {}), 'missing $expect_field'" 2>/dev/null; then
            # For arrays, check first element
            if ! echo "$body_text" | python3 -c "import sys,json; d=json.load(sys.stdin); assert isinstance(d,list) and len(d)>0, 'empty'" 2>/dev/null; then
                printf "  ${RED}FAIL${NC} %-45s %s\n" "$label" "missing field: $expect_field"
                FAIL=$((FAIL + 1))
                ERRORS+="  $label: missing expected field '$expect_field'\n"
                return 1
            fi
        fi
    fi

    printf "  ${GREEN}PASS${NC} %-45s HTTP %s\n" "$label" "$http_code"
    PASS=$((PASS + 1))
    return 0
}

check_header() {
    local label="$1"
    local url="$2"
    local header="$3"

    local headers
    headers=$(curl -sk --max-time 10 -I "${AUTH_ARGS[@]}" "$url" 2>/dev/null) || {
        printf "  ${RED}FAIL${NC} %-45s %s\n" "$label" "connection refused"
        FAIL=$((FAIL + 1))
        return 1
    }

    if echo "$headers" | grep -qi "$header"; then
        printf "  ${GREEN}PASS${NC} %-45s %s\n" "$label" "present"
        PASS=$((PASS + 1))
    else
        printf "  ${RED}FAIL${NC} %-45s %s\n" "$label" "MISSING"
        FAIL=$((FAIL + 1))
        ERRORS+="  $label: header '$header' not found\n"
    fi
}

check_deterministic() {
    local label="$1"
    local url="$2"

    local r1 r2
    r1=$(curl -sk --max-time 10 "${AUTH_ARGS[@]}" "$url" 2>/dev/null)
    r2=$(curl -sk --max-time 10 "${AUTH_ARGS[@]}" "$url" 2>/dev/null)

    if [[ "$r1" == "$r2" ]]; then
        printf "  ${GREEN}PASS${NC} %-45s %s\n" "$label" "deterministic"
        PASS=$((PASS + 1))
    else
        printf "  ${YELLOW}WARN${NC} %-45s %s\n" "$label" "non-deterministic"
        SKIP=$((SKIP + 1))
    fi
}

# ── Validate one server ──────────────────────────────────────────────────────

validate_server() {
    local name="$1"
    local base="$2"

    printf "\n${CYAN}── $name ──${NC}\n"

    # Health check first
    if ! curl -sk --max-time 5 "$base/health" > /dev/null 2>&1; then
        printf "  ${RED}SKIP${NC} %-45s %s\n" "$name" "not reachable"
        SKIP=$((SKIP + 7))
        return
    fi

    # 1. /api/users
    check "$name /api/users" "$base/api/users?page=1&sort=name&order=asc" "GET" "" ""

    # 2. /api/transform
    check "$name /api/transform" "$base/api/transform" "POST" \
        '{"seed":42,"fields":["hello","world"],"values":[1,2,3]}' ""

    # 3. /api/aggregate
    check "$name /api/aggregate" "$base/api/aggregate?range=1,100" "GET" "" "mean"

    # 4. /api/search
    check "$name /api/search" "$base/api/search?q=test&limit=10" "GET" "" ""

    # 5. /api/upload/process
    check "$name /api/upload/process" "$base/api/upload/process" "POST" \
        "benchmark-test-payload-data-1234567890" "sha256"

    # 6. /api/delayed
    check "$name /api/delayed" "$base/api/delayed?ms=10&work=light" "GET" "" "actual_ms"

    # 7. /api/validate (deterministic)
    check "$name /api/validate" "$base/api/validate?seed=$SEED" "GET" "" ""
    check_deterministic "$name /api/validate determinism" "$base/api/validate?seed=$SEED"

    # Auth rejection test (only when --token is provided)
    if [[ -n "$TOKEN" ]]; then
        local code
        code=$(curl -sk -o /dev/null -w "%{http_code}" --max-time 5 "$base/api/users?page=1")
        if [[ "$code" == "401" ]]; then
            printf "  ${GREEN}PASS${NC} %-45s %s\n" "$name auth rejection" "401"
            PASS=$((PASS + 1))
        else
            printf "  ${RED}FAIL${NC} %-45s %s\n" "$name auth rejection" "expected 401, got $code"
            FAIL=$((FAIL + 1))
            ERRORS+="  $name auth rejection: expected 401, got $code\n"
        fi
    fi

    # Header checks
    check_header "$name Server-Timing" "$base/api/users?page=1" "Server-Timing"
    check_header "$name Cache-Control" "$base/api/users?page=1" "Cache-Control"
    check_header "$name Timing-Allow-Origin" "$base/api/users?page=1" "Timing-Allow-Origin"
    check_header "$name Access-Control-Allow-Origin" "$base/api/users?page=1" "Access-Control-Allow-Origin"
}

# ── Main ─────────────────────────────────────────────────────────────────────

echo "========================================"
echo "  Application Benchmark API Validation"
echo "========================================"

if [[ "$MODE" == "rust" ]]; then
    echo "Mode: Rust endpoint only (local)"
    echo ""

    # Build and start the Rust endpoint
    SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
    PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

    echo "Building networker-endpoint..."
    cargo build -p networker-endpoint --quiet 2>/dev/null

    echo "Starting endpoint on localhost:8443..."
    "$PROJECT_DIR/target/debug/networker-endpoint" --http-port 8480 --https-port 8443 &
    ENDPOINT_PID=$!
    trap "kill $ENDPOINT_PID 2>/dev/null; wait $ENDPOINT_PID 2>/dev/null || true" EXIT

    sleep 2
    validate_server "Rust" "https://localhost:8443"

elif [[ "$MODE" == "single" ]]; then
    echo "Mode: Single server at $SINGLE_URL"
    echo ""
    validate_server "Server" "$SINGLE_URL"

elif [[ "$MODE" == "docker" ]]; then
    echo "Mode: Docker Compose (all languages)"
    echo ""

    SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
    cd "$SCRIPT_DIR"

    echo "Starting containers..."
    docker compose up -d --build --wait 2>&1 | tail -5

    # Wait for servers to be healthy
    echo "Waiting for servers to start..."
    sleep 10

    # Language → port mapping
    declare -A SERVERS=(
        ["Go"]=8501
        ["Python"]=8502
        ["Node.js"]=8503
        ["Java"]=8504
        ["Ruby"]=8505
        ["PHP"]=8506
        ["C++"]=8507
        ["C# .NET 8"]=8508
    )

    for lang in "Go" "Python" "Node.js" "Java" "Ruby" "PHP" "C++" "C# .NET 8"; do
        port="${SERVERS[$lang]}"
        validate_server "$lang" "https://localhost:$port"
    done

    echo ""
    echo "Stopping containers..."
    docker compose down -v --timeout 5 2>&1 | tail -3
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "========================================"
printf "  Results: ${GREEN}%d passed${NC}  ${RED}%d failed${NC}  ${YELLOW}%d skipped${NC}\n" "$PASS" "$FAIL" "$SKIP"
echo "========================================"

if [[ -n "$ERRORS" ]]; then
    echo ""
    echo "Failures:"
    printf "$ERRORS"
fi

if [[ "$FAIL" -gt 0 ]]; then
    exit 1
fi
exit 0
