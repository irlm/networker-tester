#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Application Benchmark JSON API Validation
#
# Validates reference API servers against the frozen contract in
# benchmarks/shared/API-SPEC.md. Two failure tiers:
#
#   HARD  — server unreachable or /health violates the orchestrator contract
#           (status/runtime/version, constant body). Always fatal (exit 1).
#   CONF  — spec-conformance failures: endpoint status/shape, canonical
#           checksums (§7), download fill bytes, benchmark headers. Exit 2
#           unless --require-conformance (then exit 1). Languages not yet
#           ported to the canonical family C contract fail this tier until
#           wave 2 lands — the CI workflow tracks them per language.
#
# Usage:
#   ./run-validation.sh                          # Docker Compose (all languages)
#   ./run-validation.sh --url=https://localhost:8443 --name=Go   # Single server
#   ./run-validation.sh --rust-only              # Canonical Rust endpoint (no Docker)
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

PASS=0
FAIL=0        # hard failures
CONF_FAIL=0   # spec-conformance failures
SKIP=0
ERRORS=""

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Argument parsing ─────────────────────────────────────────────────────────
MODE="docker"
SINGLE_URL=""
SINGLE_NAME="Server"
SEED=42
TOKEN=""
DATA_PATH="$SCRIPT_DIR/../reference-apis/shared/bench-data.json"
REQUIRE_CONF=0

for arg in "$@"; do
    case "$arg" in
        --url=*) SINGLE_URL="${arg#*=}"; MODE="single" ;;
        --name=*) SINGLE_NAME="${arg#*=}" ;;
        --rust-only) MODE="rust" ;;
        --seed=*) SEED="${arg#*=}" ;;
        --token=*) TOKEN="${arg#*=}" ;;
        --data=*) DATA_PATH="${arg#*=}" ;;
        --require-conformance) REQUIRE_CONF=1 ;;
        --help)
            echo "Usage: $0 [--url=URL [--name=NAME] | --rust-only] [--seed=N]" \
                 "[--token=TOKEN] [--data=bench-data.json] [--require-conformance]"
            exit 0 ;;
    esac
done

# Build auth header args for curl when a token is provided
AUTH_ARGS=()
if [[ -n "$TOKEN" ]]; then
    AUTH_ARGS=(-H "Authorization: Bearer $TOKEN")
fi

# ── Test helpers ─────────────────────────────────────────────────────────────

pass() { printf "  ${GREEN}PASS${NC} %-48s %s\n" "$1" "$2"; PASS=$((PASS + 1)); }
hard_fail() {
    printf "  ${RED}FAIL${NC} %-48s %s\n" "$1" "$2"
    FAIL=$((FAIL + 1)); ERRORS+="  [hard] $1: $2\n"
}
conf_fail() {
    printf "  ${YELLOW}CONF${NC} %-48s %s\n" "$1" "$2"
    CONF_FAIL=$((CONF_FAIL + 1)); ERRORS+="  [conf] $1: $2\n"
}

# Conformance-tier endpoint check: HTTP < 400, valid JSON, optional field.
check() {
    local label="$1"
    local url="$2"
    local method="${3:-GET}"
    local body="${4:-}"
    local expect_field="${5:-}"

    local curl_args=(-sk --max-time 10 -X "$method" ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"})
    if [[ -n "$body" ]]; then
        curl_args+=(-H "Content-Type: application/json" -d "$body")
    fi

    local response
    response=$(curl "${curl_args[@]}" -w "\n%{http_code}" "$url" 2>/dev/null) || {
        conf_fail "$label" "connection refused"
        return 1
    }

    local http_code
    http_code=$(echo "$response" | tail -1)
    local body_text
    body_text=$(echo "$response" | sed '$d')

    if [[ "$http_code" -ge 400 ]]; then
        conf_fail "$label" "HTTP $http_code"
        return 1
    fi

    if ! echo "$body_text" | python3 -m json.tool > /dev/null 2>&1; then
        conf_fail "$label" "invalid JSON"
        return 1
    fi

    if [[ -n "$expect_field" ]]; then
        if ! echo "$body_text" | python3 -c "import sys,json; d=json.load(sys.stdin); assert '$expect_field' in (d if isinstance(d,dict) else {}), 'missing $expect_field'" 2>/dev/null; then
            if ! echo "$body_text" | python3 -c "import sys,json; d=json.load(sys.stdin); assert isinstance(d,list) and len(d)>0, 'empty'" 2>/dev/null; then
                conf_fail "$label" "missing field: $expect_field"
                return 1
            fi
        fi
    fi

    pass "$label" "HTTP $http_code"
    return 0
}

check_header() {
    local label="$1"
    local url="$2"
    local header="$3"

    local headers
    headers=$(curl -sk --max-time 10 -I ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} "$url" 2>/dev/null) || {
        conf_fail "$label" "connection refused"
        return 1
    }

    if echo "$headers" | grep -qi "$header"; then
        pass "$label" "present"
    else
        conf_fail "$label" "MISSING header '$header'"
    fi
}

check_deterministic() {
    local label="$1"
    local url="$2"

    local r1 r2
    r1=$(curl -sk --max-time 10 ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} "$url" 2>/dev/null)
    r2=$(curl -sk --max-time 10 ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} "$url" 2>/dev/null)

    if [[ "$r1" == "$r2" ]]; then
        pass "$label" "deterministic"
    else
        conf_fail "$label" "non-deterministic"
    fi
}

# HARD tier: /health must satisfy the orchestrator contract
# (API-SPEC.md §5.1): 200, status=="ok", non-empty runtime + version,
# byte-constant body across two requests.
check_health_contract() {
    local name="$1"
    local base="$2"

    local h1 h2
    h1=$(curl -sk --max-time 10 "$base/health" 2>/dev/null) || {
        hard_fail "$name /health" "connection refused"
        return 1
    }
    h2=$(curl -sk --max-time 10 "$base/health" 2>/dev/null) || true

    if ! echo "$h1" | python3 -c '
import sys, json
d = json.load(sys.stdin)
assert d.get("status") == "ok", "status != ok"
assert isinstance(d.get("runtime"), str) and d["runtime"], "missing runtime"
assert isinstance(d.get("version"), str) and d["version"], "missing version"
' 2>/dev/null; then
        hard_fail "$name /health contract" "needs status=ok + runtime + version — got: ${h1:0:120}"
        return 1
    fi
    pass "$name /health contract" "status/runtime/version"

    if [[ "$h1" == "$h2" ]]; then
        pass "$name /health constant-work" "byte-constant"
    else
        conf_fail "$name /health constant-work" "body varies per request"
    fi
    return 0
}

# CONF tier: GET /download/1024 → exactly 1024 bytes, all 0x42 (§5.2).
check_download_bytes() {
    local name="$1"
    local base="$2"

    local tmp
    tmp=$(mktemp)
    if ! curl -sk --max-time 10 ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -o "$tmp" "$base/download/1024" 2>/dev/null; then
        conf_fail "$name /download/1024" "request failed"
        rm -f "$tmp"; return 1
    fi
    if python3 -c "
import sys
b = open('$tmp','rb').read()
assert len(b) == 1024, f'{len(b)} bytes'
assert set(b) == {0x42}, 'fill byte != 0x42'
" 2>/dev/null; then
        pass "$name /download/1024" "1024 x 0x42"
    else
        conf_fail "$name /download/1024" "wrong size or fill byte (spec: 1024 bytes of 0x42)"
    fi
    rm -f "$tmp"
}

# CONF tier: the four canonical checksums from API-SPEC.md §7.
check_spec_checksums() {
    local name="$1"
    local base="$2"

    if [[ ! -f "$DATA_PATH" ]]; then
        printf "  ${YELLOW}SKIP${NC} %-48s %s\n" "$name spec checksums" "no bench-data.json at $DATA_PATH"
        SKIP=$((SKIP + 1))
        return 0
    fi

    local out
    if out=$(python3 - "$base" "$DATA_PATH" "$TOKEN" <<'PYEOF' 2>&1
import hashlib, json, ssl, sys, urllib.request

base, data_path, token = sys.argv[1], sys.argv[2], sys.argv[3]
ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

data = json.load(open(data_path))
expected = data["expected_checksums"]

def canon(obj):
    return json.dumps(obj, sort_keys=True, separators=(",", ":")).encode()

def fetch(path, body=None):
    headers = {"Content-Type": "application/json"} if body else {}
    if token:
        headers["Authorization"] = "Bearer " + token
    req = urllib.request.Request(base + path, data=body, headers=headers)
    with urllib.request.urlopen(req, context=ctx, timeout=15) as r:
        return json.loads(r.read())

requests = {
    "users_page1": ("/api/users?page=1&sort=name&order=asc", None),
    "aggregate_default": ("/api/aggregate", None),
    "search_network_top10": ("/api/search?q=network&limit=10", None),
    "transform_input0": ("/api/transform", canon(data["transform_inputs"][0])),
}

bad = 0
for key, (path, body) in requests.items():
    try:
        got = hashlib.sha256(canon(fetch(path, body))).hexdigest()
    except Exception as e:
        print(f"    {key}: request failed: {e}")
        bad += 1
        continue
    if got != expected[key]:
        print(f"    {key}: MISMATCH got {got[:16]}… want {expected[key][:16]}…")
        bad += 1
sys.exit(1 if bad else 0)
PYEOF
    ); then
        pass "$name spec checksums" "4/4 canonical (§7)"
    else
        conf_fail "$name spec checksums" "response(s) diverge from API-SPEC.md"
        [[ -n "$out" ]] && printf "%s\n" "$out"
    fi
}

# ── Validate one server ──────────────────────────────────────────────────────

validate_server() {
    local name="$1"
    local base="$2"

    printf "\n${CYAN}── %s ──${NC}\n" "$name"

    # HARD tier: reachability + /health orchestrator contract.
    if ! curl -sk --max-time 5 "$base/health" > /dev/null 2>&1; then
        hard_fail "$name" "not reachable"
        return
    fi
    check_health_contract "$name" "$base" || return

    # CONF tier: transport workload
    check_download_bytes "$name" "$base"

    # CONF tier: the 7 JSON API endpoints
    check "$name /api/users" "$base/api/users?page=1&sort=name&order=asc" "GET" "" ""
    check "$name /api/transform" "$base/api/transform" "POST" \
        '{"seed":42,"fields":["hello","world"],"values":[1,2,3]}' "hashed_fields"
    check "$name /api/aggregate" "$base/api/aggregate" "GET" "" "mean"
    check "$name /api/search" "$base/api/search?q=test&limit=10" "GET" "" "results"
    check "$name /api/upload/process" "$base/api/upload/process" "POST" \
        "benchmark-test-payload-data-1234567890" "sha256"
    check "$name /api/delayed" "$base/api/delayed?ms=10&work=light" "GET" "" "actual_ms"
    check "$name /api/validate" "$base/api/validate?seed=$SEED" "GET" "" "checksums"
    check_deterministic "$name /api/validate determinism" "$base/api/validate?seed=$SEED"

    # CONF tier: canonical response checksums (the real cross-language check)
    check_spec_checksums "$name" "$base"

    # Auth rejection test (only when --token is provided)
    if [[ -n "$TOKEN" ]]; then
        local code
        code=$(curl -sk -o /dev/null -w "%{http_code}" --max-time 5 "$base/api/users?page=1")
        if [[ "$code" == "401" ]]; then
            pass "$name auth rejection" "401"
        else
            conf_fail "$name auth rejection" "expected 401, got $code"
        fi
    fi

    # CONF tier: benchmark headers (API-SPEC.md §1)
    check_header "$name Server-Timing" "$base/api/users?page=1" "Server-Timing"
    check_header "$name Cache-Control" "$base/api/users?page=1" "Cache-Control"
    check_header "$name Timing-Allow-Origin" "$base/api/users?page=1" "Timing-Allow-Origin"
    check_header "$name Access-Control-Allow-Origin" "$base/api/users?page=1" "Access-Control-Allow-Origin"
}

# ── Main ─────────────────────────────────────────────────────────────────────

echo "========================================"
echo "  Application Benchmark API Validation"
echo "  Contract: benchmarks/shared/API-SPEC.md"
echo "========================================"

if [[ "$MODE" == "rust" ]]; then
    echo "Mode: Rust endpoint only (local, canonical baseline)"
    echo ""

    PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
    # The canonical baseline must be validated against the shared dataset.
    export BENCH_DATA_PATH="$DATA_PATH"
    REQUIRE_CONF=1

    echo "Building networker-endpoint..."
    cargo build -p networker-endpoint --quiet

    echo "Starting endpoint on localhost:8443..."
    "$PROJECT_DIR/target/debug/networker-endpoint" --http-port 8480 --https-port 8443 &
    ENDPOINT_PID=$!
    trap 'kill $ENDPOINT_PID 2>/dev/null; wait $ENDPOINT_PID 2>/dev/null || true' EXIT

    sleep 4
    validate_server "Rust" "https://localhost:8443"

elif [[ "$MODE" == "single" ]]; then
    echo "Mode: Single server '$SINGLE_NAME' at $SINGLE_URL"
    echo ""
    validate_server "$SINGLE_NAME" "$SINGLE_URL"

elif [[ "$MODE" == "docker" ]]; then
    echo "Mode: Docker Compose (all languages)"
    echo ""

    cd "$SCRIPT_DIR"

    echo "Starting containers..."
    docker compose up -d --build --wait 2>&1 | tail -5 || true

    echo "Waiting for servers to start..."
    sleep 10

    # Language → port mapping (parallel arrays: macOS ships bash 3.2, which
    # has no associative arrays).
    LANG_NAMES=("Go" "Python" "Node.js" "Java" "Ruby" "PHP" "C++" "C# .NET 8")
    LANG_PORTS=(8501 8502 8503 8504 8505 8506 8507 8508)

    i=0
    while [[ $i -lt ${#LANG_NAMES[@]} ]]; do
        validate_server "${LANG_NAMES[$i]}" "https://localhost:${LANG_PORTS[$i]}"
        i=$((i + 1))
    done

    echo ""
    echo "Stopping containers..."
    docker compose down -v --timeout 5 2>&1 | tail -3
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "========================================"
printf "  Results: ${GREEN}%d passed${NC}  ${RED}%d hard-failed${NC}  ${YELLOW}%d conformance-failed${NC}  %d skipped\n" \
    "$PASS" "$FAIL" "$CONF_FAIL" "$SKIP"
echo "========================================"

if [[ -n "$ERRORS" ]]; then
    echo ""
    echo "Failures:"
    printf "%b" "$ERRORS"
fi

if [[ "$FAIL" -gt 0 ]]; then
    exit 1
fi
if [[ "$CONF_FAIL" -gt 0 ]]; then
    if [[ "$REQUIRE_CONF" -eq 1 ]]; then
        exit 1
    fi
    exit 2
fi
exit 0
