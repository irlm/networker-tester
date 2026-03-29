#!/bin/bash
# AletheBench Local Test — run each available server and benchmark it
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CERT="$REPO_ROOT/benchmarks/shared/cert.pem"
KEY="$REPO_ROOT/benchmarks/shared/key.pem"
TESTER="$REPO_ROOT/target/release/networker-tester"
PORT=8443
RUNS=100
RESULTS=()

G='\033[0;32m'
R='\033[0;31m'
Y='\033[0;33m'
C='\033[0;36m'
N='\033[0m'

benchmark() {
    local name="$1"
    echo -e "\n${C}━━━ Benchmarking: $name ━━━${N}"

    # Wait for server to be ready
    for i in $(seq 1 15); do
        if curl -sk --max-time 2 https://localhost:$PORT/health > /dev/null 2>&1; then
            break
        fi
        sleep 1
    done

    local health=$(curl -sk https://localhost:$PORT/health 2>/dev/null)
    if [ -z "$health" ]; then
        echo -e "${R}  FAILED — server not responding${N}"
        RESULTS+=("$name|FAILED|—|—|—")
        return
    fi
    echo -e "  Health: $health"

    # Run benchmark
    local output=$("$TESTER" \
        --target https://localhost:$PORT/health \
        --modes http1 \
        --runs $RUNS \
        --timeout 5 \
        --insecure 2>&1)

    # Extract metrics from output
    local total_line=$(echo "$output" | grep "http1.*Total ms" | head -1)
    if [ -n "$total_line" ]; then
        local mean=$(echo "$total_line" | awk -F'│' '{print $4}' | tr -d ' ')
        local p50=$(echo "$total_line" | awk -F'│' '{print $5}' | tr -d ' ')
        local p99=$(echo "$total_line" | awk -F'│' '{print $7}' | tr -d ' ')
        local success=$(echo "$output" | grep "Run complete" | grep -o 'success=[0-9]*' | cut -d= -f2)
        echo -e "  ${G}Mean: ${mean}ms  p50: ${p50}ms  p99: ${p99}ms  Success: ${success}/${RUNS}${N}"
        RESULTS+=("$name|OK|$mean|$p50|$p99")
    else
        local success=$(echo "$output" | grep "Run complete" | grep -o 'success=[0-9]*' | cut -d= -f2)
        local failure=$(echo "$output" | grep "Run complete" | grep -o 'failure=[0-9]*' | cut -d= -f2)
        echo -e "${R}  Success: ${success:-0}  Failure: ${failure:-?}${N}"
        RESULTS+=("$name|PARTIAL|—|—|—")
    fi
}

kill_server() {
    lsof -ti :$PORT 2>/dev/null | xargs kill -9 2>/dev/null || true
    sleep 1
}

echo "╔══════════════════════════════════════════════════════╗"
echo "║  AletheBench Local Test Suite                       ║"
echo "║  $RUNS requests per language · HTTP/1.1 · localhost ║"
echo "╚══════════════════════════════════════════════════════╝"

# 1. Rust
kill_server
echo -e "\n${Y}Starting Rust...${N}"
"$REPO_ROOT/target/release/networker-endpoint" --https-port $PORT 2>/dev/null &
sleep 2
benchmark "Rust (hyper)"
kill_server

# 2. Go
kill_server
echo -e "\n${Y}Building and starting Go...${N}"
cd "$REPO_ROOT/benchmarks/reference-apis/go"
go build -o /tmp/alethabench-go . 2>&1
BENCH_CERT_DIR="$REPO_ROOT/benchmarks/shared" /tmp/alethabench-go &
sleep 2
benchmark "Go (net/http)"
kill_server

# 3. Node.js
kill_server
echo -e "\n${Y}Starting Node.js...${N}"
cd "$REPO_ROOT/benchmarks/reference-apis/nodejs"
BENCH_CERT_DIR="$REPO_ROOT/benchmarks/shared" node server.js &
sleep 2
benchmark "Node.js (http2)"
kill_server

# 4. Python
kill_server
echo -e "\n${Y}Starting Python...${N}"
cd "$REPO_ROOT/benchmarks/reference-apis/python"
pip3 install -q uvicorn starlette 2>/dev/null || true
uvicorn server:app --host 0.0.0.0 --port $PORT \
  --ssl-keyfile "$KEY" --ssl-certfile "$CERT" \
  --log-level error &
sleep 3
benchmark "Python (uvicorn)"
kill_server

# 5. Java
kill_server
echo -e "\n${Y}Building and starting Java...${N}"
cd "$REPO_ROOT/benchmarks/reference-apis/java"
javac Server.java 2>&1
CERT_DIR="$REPO_ROOT/benchmarks/shared" java Server &
sleep 3
benchmark "Java (JDK HttpServer)"
kill_server

# 6. C# .NET 10
kill_server
echo -e "\n${Y}Building and starting C# .NET 10...${N}"
cd "$REPO_ROOT/benchmarks/reference-apis/csharp-net10"
dotnet build -c Release -q 2>&1 | tail -1
ASPNETCORE_URLS="https://0.0.0.0:$PORT" \
ASPNETCORE_Kestrel__Certificates__Default__Path="$CERT" \
ASPNETCORE_Kestrel__Certificates__Default__KeyPath="$KEY" \
dotnet run -c Release --no-build 2>/dev/null &
sleep 4
benchmark "C# .NET 10 (Kestrel)"
kill_server

# Print summary
echo ""
echo "╔══════════════════════════════════════════════════════╗"
echo "║  RESULTS SUMMARY                                    ║"
echo "╠══════════════════════════════════════════════════════╣"
printf "║  %-25s │ %8s │ %8s │ %8s ║\n" "Language" "Mean" "p50" "p99"
echo "╠══════════════════════════════════════════════════════╣"
for r in "${RESULTS[@]}"; do
    IFS='|' read -r name status mean p50 p99 <<< "$r"
    if [ "$status" = "OK" ]; then
        printf "║  %-25s │ %7sms │ %7sms │ %7sms ║\n" "$name" "$mean" "$p50" "$p99"
    else
        printf "║  %-25s │ %8s │ %8s │ %8s ║\n" "$name" "$status" "—" "—"
    fi
done
echo "╚══════════════════════════════════════════════════════╝"
