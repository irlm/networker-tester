#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Application Benchmark JSON API Validation
#
# Validates reference API servers against the frozen contract in
# benchmarks/shared/API-SPEC.md. Two failure tiers:
#
#   HARD  — server unreachable or /health violates the orchestrator contract
#           (status/runtime/version, constant body). Always fatal (exit 1).
#   CONF  — spec-conformance failures: endpoint status, per-endpoint JSON
#           shape (exact keys + types on non-canonical requests, §5),
#           canonical checksums (§7), byte-exact downloads at 1024 and 65536
#           (length + sha256 of 0x42*N), benchmark headers. Exit 2 unless
#           --require-conformance (then exit 1). Conformance is required for
#           all languages since wave-2 convergence (2026-07-17).
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

# CONF tier: GET /download/{size} → exactly {size} bytes, all 0x42 (§5.2).
# Byte-exact: length AND sha256(body) == sha256(0x42 * size), so a wrong fill
# byte, truncation, or padding on non-default sizes is caught.
check_download_bytes() {
    local name="$1"
    local base="$2"
    local size="${3:-1024}"

    local tmp curl_err
    tmp=$(mktemp)
    # -sS: keep curl quiet but surface transfer errors (e.g. "transfer closed
    # with N bytes remaining" — exactly the failure mode this check hunts).
    if ! curl_err=$(curl -sSk --max-time 15 ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -o "$tmp" "$base/download/$size" 2>&1); then
        local got_bytes
        got_bytes=$(wc -c < "$tmp" 2>/dev/null | tr -d ' ' || echo '?')
        conf_fail "$name /download/$size" "request failed after $got_bytes/$size bytes: ${curl_err:-unknown curl error}"
        rm -f "$tmp"; return 1
    fi
    local detail
    if detail=$(python3 -c "
import hashlib
size = $size
b = open('$tmp','rb').read()
assert len(b) == size, f'{len(b)} bytes (want {size})'
want = hashlib.sha256(b'\x42' * size).hexdigest()
got = hashlib.sha256(b).hexdigest()
assert got == want, f'sha256 {got[:16]}… != 0x42*{size} ({want[:16]}…)'
" 2>&1); then
        pass "$name /download/$size" "$size x 0x42 (sha256 exact)"
    else
        conf_fail "$name /download/$size" "$(echo "$detail" | grep -o 'AssertionError.*' || echo "wrong size or content (spec: $size bytes of 0x42)")"
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

# CONF tier: per-endpoint JSON *shape* assertions (API-SPEC.md §5) on
# NON-canonical requests. The §7 checksums already pin four exact requests;
# this catches an implementation that returns extra or missing FIELDS (or
# wrong types) on requests the checksums never see — e.g. family-B wrapper
# objects, a stray compression_ratio, floats where ints belong. Deliberately
# not full JSON Schema machinery: a small required key:type table per
# endpoint, exact key sets (no extras allowed except where §5 permits).
check_endpoint_shapes() {
    local name="$1"
    local base="$2"

    local out line
    out=$(python3 - "$base" "$TOKEN" <<'PYEOF' 2>&1
import hashlib, json, ssl, sys, urllib.request

base, token = sys.argv[1], sys.argv[2]
ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE

def fetch(path, body=None, raw=False):
    headers = {}
    if body is not None:
        headers["Content-Type"] = "application/octet-stream" if raw else "application/json"
    if token:
        headers["Authorization"] = "Bearer " + token
    req = urllib.request.Request(base + path, data=body, headers=headers)
    with urllib.request.urlopen(req, context=ctx, timeout=10) as r:
        return json.loads(r.read())

# type predicates — note bool is a subclass of int in Python, exclude it
is_int = lambda v: type(v) is int
is_num = lambda v: type(v) in (int, float)
is_str = lambda v: isinstance(v, str)
is_list = lambda v: isinstance(v, list)
is_dict = lambda v: isinstance(v, dict)
def is_hex(n):
    return lambda v: isinstance(v, str) and len(v) == n and all(c in "0123456789abcdef" for c in v)

class Bad(Exception):
    pass

def exact(obj, spec, where):
    """obj must be a dict with EXACTLY spec's keys, each passing its type check."""
    if not is_dict(obj):
        raise Bad(f"{where}: expected object, got {type(obj).__name__}")
    got, want = set(obj), set(spec)
    if got != want:
        parts = []
        if got - want:
            parts.append("extra keys: " + ",".join(sorted(got - want)))
        if want - got:
            parts.append("missing keys: " + ",".join(sorted(want - got)))
        raise Bad(f"{where}: " + "; ".join(parts))
    for k, chk in spec.items():
        if not chk(obj[k]):
            raise Bad(f"{where}.{k}: wrong type ({type(obj[k]).__name__})")

USER = {"id": is_int, "name": is_str, "email": is_str, "score": is_num, "created_at": is_str}
CATEGORY = {"category": is_str, "count": is_int, "mean": is_num, "min": is_num, "max": is_num}
RESULT = {"rank": is_int, "item": is_str, "match_position": is_int}
UPLOAD_BODY = b"shape-check-payload-0123456789"

def users():
    d = fetch("/api/users?page=1&sort=score&order=desc")
    if not is_list(d):
        raise Bad(f"top level: expected bare array (no wrapper), got {type(d).__name__}")
    if not (0 < len(d) <= 20):
        raise Bad(f"expected 1..20 users, got {len(d)}")
    for i, u in enumerate(d):
        exact(u, USER, f"[{i}]")

def transform():
    body = json.dumps({"seed": 7, "fields": ["shape-check"], "values": [9, 8, 7]}).encode()
    d = fetch("/api/transform", body)
    exact(d, {"seed": is_int, "hashed_fields": is_list, "reversed_values": is_list}, "top level")
    for i, h in enumerate(d["hashed_fields"]):
        if not is_hex(64)(h):
            raise Bad(f"hashed_fields[{i}]: not 64-char lowercase hex")

def aggregate():
    d = fetch("/api/aggregate?range=1,100")  # range accepted + ignored (§5.6)
    exact(d, {"total_points": is_int, "mean": is_num, "p50": is_num,
              "p95": is_num, "max": is_num, "categories": is_list}, "top level")
    if len(d["categories"]) != 5:
        raise Bad(f"categories: expected 5 quintiles, got {len(d['categories'])}")
    for i, c in enumerate(d["categories"]):
        exact(c, CATEGORY, f"categories[{i}]")

def search():
    d = fetch("/api/search?q=test&limit=5")
    exact(d, {"query": is_str, "total_matches": is_int,
              "returned": is_int, "results": is_list}, "top level")
    if len(d["results"]) > 5:
        raise Bad(f"results: limit=5 not honored ({len(d['results'])} returned)")
    for i, r in enumerate(d["results"]):
        exact(r, RESULT, f"results[{i}]")

def upload_process():
    d = fetch("/api/upload/process", UPLOAD_BODY, raw=True)
    exact(d, {"original_size": is_int, "compressed_size": is_int,
              "crc32": is_hex(8), "sha256": is_hex(64)}, "top level")
    if d["original_size"] != len(UPLOAD_BODY):
        raise Bad(f"original_size {d['original_size']} != {len(UPLOAD_BODY)}")
    if d["sha256"] != hashlib.sha256(UPLOAD_BODY).hexdigest():
        raise Bad("sha256 does not match request body")

def delayed():
    d = fetch("/api/delayed?ms=5")
    exact(d, {"requested_ms": is_int, "actual_ms": is_num}, "top level")

def validate():
    d = fetch("/api/validate?seed=7")
    exact(d, {"seed": is_int, "checksums": is_dict}, "top level")
    exact(d["checksums"], {k: is_hex(64) for k in
          ("users_page1", "aggregate_default", "search_network_top10", "transform_input0")},
          "checksums")

for ep, fn in [("/api/users", users), ("/api/transform", transform),
               ("/api/aggregate", aggregate), ("/api/search", search),
               ("/api/upload/process", upload_process), ("/api/delayed", delayed),
               ("/api/validate", validate)]:
    try:
        fn()
        print(f"OK {ep}")
    except Bad as e:
        print(f"BAD {ep}: {e}")
    except Exception as e:
        print(f"BAD {ep}: request failed: {e}")
PYEOF
    ) || true

    local saw=0
    while IFS= read -r line; do
        case "$line" in
            "OK "*)
                pass "$name shape ${line#OK }" "exact keys+types"
                saw=1 ;;
            "BAD "*)
                local rest="${line#BAD }"
                conf_fail "$name shape ${rest%%:*}" "${rest#*: }"
                saw=1 ;;
        esac
    done <<< "$out"
    if [[ "$saw" -eq 0 ]]; then
        conf_fail "$name shape assertions" "checker produced no results: $(echo "$out" | head -1)"
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

    # CONF tier: transport workload — default size plus one non-default size,
    # both byte-exact (length + sha256 of 0x42*N), so size-dependent bugs
    # (chunking, truncation, wrong fill on the non-1024 path) are caught.
    check_download_bytes "$name" "$base" 1024
    check_download_bytes "$name" "$base" 65536

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

    # CONF tier: JSON shape assertions on non-canonical requests (§5)
    check_endpoint_shapes "$name" "$base"

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
