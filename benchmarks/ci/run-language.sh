#!/usr/bin/env bash
# run-language.sh — Build, start, benchmark, and clean up a single language server.
#
# Usage: run-language.sh <language> [runs]
#
# Environment:
#   REPO_ROOT     — repository root (default: auto-detected)
#   CERT_DIR      — directory containing cert.pem + key.pem
#   TESTER        — path to networker-tester binary
#   PORT          — server listen port (default: 8443)
#   RESULTS_DIR   — directory for JSON output files
set -euo pipefail

LANG="${1:?Usage: run-language.sh <language> [runs]}"
RUNS="${2:-100}"
REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
CERT_DIR="${CERT_DIR:-$REPO_ROOT/benchmarks/shared}"
TESTER="${TESTER:-$REPO_ROOT/target/release/networker-tester}"
PORT="${PORT:-8443}"
RESULTS_DIR="${RESULTS_DIR:-$REPO_ROOT/benchmarks/results}"
API_DIR="$REPO_ROOT/benchmarks/reference-apis"

SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    # Belt-and-suspenders: kill anything left on the port
    if command -v lsof >/dev/null 2>&1; then
        lsof -ti :"$PORT" 2>/dev/null | xargs kill -9 2>/dev/null || true
    fi
}
trap cleanup EXIT

wait_for_server() {
    local max_wait="${1:-30}"
    for i in $(seq 1 "$max_wait"); do
        if curl -sk --max-time 2 "https://localhost:$PORT/health" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    echo "ERROR: Server did not respond within ${max_wait}s" >&2
    return 1
}

start_server() {
    case "$LANG" in
        rust)
            "$REPO_ROOT/target/release/networker-endpoint" --https-port "$PORT" &
            SERVER_PID=$!
            ;;
        go)
            cd "$API_DIR/go"
            go build -o /tmp/alethabench-go .
            BENCH_CERT_DIR="$CERT_DIR" /tmp/alethabench-go &
            SERVER_PID=$!
            ;;
        cpp)
            cd "$API_DIR/cpp"
            mkdir -p build && cd build
            cmake .. -DCMAKE_BUILD_TYPE=Release 2>&1
            make -j"$(nproc)" 2>&1
            BENCH_CERT_DIR="$CERT_DIR" ./server &
            SERVER_PID=$!
            ;;
        nodejs|node)
            cd "$API_DIR/nodejs"
            npm install --silent 2>/dev/null || true
            BENCH_CERT_DIR="$CERT_DIR" node server.js &
            SERVER_PID=$!
            ;;
        python)
            cd "$API_DIR/python"
            pip3 install -q uvicorn starlette 2>/dev/null || true
            uvicorn server:app --host 0.0.0.0 --port "$PORT" \
                --ssl-keyfile "$CERT_DIR/key.pem" \
                --ssl-certfile "$CERT_DIR/cert.pem" \
                --log-level error &
            SERVER_PID=$!
            ;;
        java)
            cd "$API_DIR/java"
            javac Server.java 2>&1
            BENCH_CERT_DIR="$CERT_DIR" java Server &
            SERVER_PID=$!
            ;;
        ruby)
            cd "$API_DIR/ruby"
            bundle install --quiet 2>/dev/null || gem install puma 2>/dev/null
            BENCH_CERT_DIR="$CERT_DIR" bundle exec puma -C puma.rb &
            SERVER_PID=$!
            ;;
        php)
            cd "$API_DIR/php"
            BENCH_CERT_DIR="$CERT_DIR" php server.php &
            SERVER_PID=$!
            ;;
        nginx)
            # Nginx requires config file substitution for cert paths
            local conf="/tmp/alethabench-nginx.conf"
            sed \
                -e "s|/opt/bench/cert.pem|$CERT_DIR/cert.pem|g" \
                -e "s|/opt/bench/key.pem|$CERT_DIR/key.pem|g" \
                "$API_DIR/nginx/nginx.conf" > "$conf"
            nginx -c "$conf" &
            SERVER_PID=$!
            ;;
        csharp-net6|csharp-net7|csharp-net8|csharp-net8-aot|\
        csharp-net9|csharp-net9-aot|csharp-net10|csharp-net10-aot)
            cd "$API_DIR/$LANG"
            dotnet build -c Release -q 2>&1 | tail -1
            BENCH_CERT_DIR="$CERT_DIR" dotnet run -c Release --no-build &
            SERVER_PID=$!
            ;;
        *)
            echo "ERROR: Unknown language '$LANG'" >&2
            echo "Supported: rust, go, cpp, nodejs, python, java, ruby, php, nginx, csharp-net*" >&2
            exit 1
            ;;
    esac
}

run_benchmark() {
    mkdir -p "$RESULTS_DIR"
    local outfile="$RESULTS_DIR/${LANG}.json"

    "$TESTER" \
        --target "https://localhost:$PORT/health" \
        --modes http1 \
        --runs "$RUNS" \
        --timeout 5 \
        --insecure \
        --json-stdout \
        > "$outfile" 2>/dev/null

    echo "$outfile"
}

# ── Main ─────────────────────────────────────────────────────────────────────

echo "=== AletheBench CI: $LANG ($RUNS runs) ==="

echo "Starting $LANG server on port $PORT..."
start_server

echo "Waiting for health check..."
if ! wait_for_server 30; then
    echo "FATAL: $LANG server failed to start" >&2
    exit 1
fi

# Print health response for logging
curl -sk "https://localhost:$PORT/health" 2>/dev/null || true
echo ""

echo "Running benchmark..."
RESULT_FILE=$(run_benchmark)
echo "Results written to $RESULT_FILE"

echo "=== Done: $LANG ==="
