#!/usr/bin/env bats
# Integration tests for networker-endpoint.
#
# Builds the binary (if not already built) then starts it on high-numbered
# test ports so these tests can run alongside a real instance.
#
# Requires: cargo, curl

HTTP_PORT=18080
HTTPS_PORT=18443
UDP_PORT=19999
UDP_TP_PORT=19998

BASE="http://localhost:${HTTP_PORT}"

# ── Fixture ──────────────────────────────────────────────────────────────────

setup_file() {
    local root
    root="$(cd "${BATS_TEST_DIRNAME}/.." && pwd)"
    local bin="${root}/target/debug/networker-endpoint"

    # Build if the binary is missing (skip rebuild when already present).
    if [[ ! -x "$bin" ]]; then
        cargo build -p networker-endpoint \
            --manifest-path "${root}/Cargo.toml" \
            2>&1 | tail -5
    fi

    # Start on test ports to avoid conflicting with a running instance.
    "$bin" \
        --http-port  "$HTTP_PORT" \
        --https-port "$HTTPS_PORT" \
        --udp-port   "$UDP_PORT" \
        --udp-throughput-port "$UDP_TP_PORT" \
        &>/dev/null &
    echo "$!" > "${BATS_SUITE_TMPDIR}/endpoint.pid"

    # Wait up to 5 s for the HTTP server to accept connections.
    local i=0
    until curl -sf --max-time 1 "${BASE}/health" &>/dev/null; do
        sleep 0.2
        i=$((i + 1))
        if [[ $i -ge 25 ]]; then
            echo "networker-endpoint did not start within 5 s" >&2
            exit 1
        fi
    done
}

teardown_file() {
    local pid_file="${BATS_SUITE_TMPDIR}/endpoint.pid"
    if [[ -f "$pid_file" ]]; then
        kill "$(cat "$pid_file")" 2>/dev/null || true
    fi
}

# ── Landing page (GET /) ──────────────────────────────────────────────────────

@test "GET / returns 200" {
    run curl -sf "${BASE}/"
    [ "$status" -eq 0 ]
}

@test "GET / content-type is text/html" {
    run curl -sI "${BASE}/"
    [ "$status" -eq 0 ]
    [[ "$output" == *"text/html"* ]]
}

@test "GET / contains service name" {
    run curl -sf "${BASE}/"
    [ "$status" -eq 0 ]
    [[ "$output" =~ "networker-endpoint" ]]
}

@test "GET / lists /health in endpoint table" {
    run curl -sf "${BASE}/"
    [ "$status" -eq 0 ]
    [[ "$output" =~ "/health" ]]
}

@test "GET / shows HTTP port" {
    run curl -sf "${BASE}/"
    [ "$status" -eq 0 ]
    [[ "$output" =~ ":${HTTP_PORT}" ]]
}

@test "GET / shows uptime" {
    run curl -sf "${BASE}/"
    [ "$status" -eq 0 ]
    [[ "$output" =~ "running" ]]
}

# ── Health ────────────────────────────────────────────────────────────────────

@test "GET /health returns 200" {
    run curl -sf "${BASE}/health"
    [ "$status" -eq 0 ]
}

@test "GET /health body contains status ok" {
    run curl -sf "${BASE}/health"
    [ "$status" -eq 0 ]
    [[ "$output" =~ '"status"' ]]
    [[ "$output" =~ '"ok"' ]]
}

@test "GET /health has x-networker-server-version header" {
    run curl -sI "${BASE}/health"
    [ "$status" -eq 0 ]
    [[ "$output" == *"x-networker-server-version"* ]]
}

@test "GET /health has x-networker-server-timestamp header" {
    run curl -sI "${BASE}/health"
    [ "$status" -eq 0 ]
    [[ "$output" == *"x-networker-server-timestamp"* ]]
}

# ── Info ──────────────────────────────────────────────────────────────────────

@test "GET /info returns JSON with service field" {
    run curl -sf "${BASE}/info"
    [ "$status" -eq 0 ]
    [[ "$output" =~ '"service"' ]]
    [[ "$output" =~ "networker-endpoint" ]]
}

@test "GET /info returns JSON with version field" {
    run curl -sf "${BASE}/info"
    [ "$status" -eq 0 ]
    [[ "$output" =~ '"version"' ]]
}

# ── Download ──────────────────────────────────────────────────────────────────

@test "GET /download returns requested byte count" {
    local bytes
    bytes="$(curl -sf "${BASE}/download?bytes=1024" | wc -c)"
    [ "$bytes" -eq 1024 ]
}

@test "GET /download has server-timing header" {
    run curl -sI "${BASE}/download?bytes=64"
    [ "$status" -eq 0 ]
    [[ "$output" == *"server-timing"* ]]
}

@test "GET /download has x-download-bytes header" {
    run curl -sI "${BASE}/download?bytes=64"
    [ "$status" -eq 0 ]
    [[ "$output" == *"x-download-bytes"* ]]
}

# ── Upload ────────────────────────────────────────────────────────────────────

@test "POST /upload returns received_bytes in body" {
    run curl -sf -X POST --data "hello world" "${BASE}/upload"
    [ "$status" -eq 0 ]
    [[ "$output" =~ '"received_bytes"' ]]
}

@test "POST /upload has x-networker-received-bytes header" {
    run curl -sf -D - -o /dev/null -X POST --data "hello" "${BASE}/upload"
    [ "$status" -eq 0 ]
    [[ "$output" == *"x-networker-received-bytes"* ]]
}

@test "POST /upload echoes x-networker-request-id" {
    run curl -sf -D - -o /dev/null -X POST --data "data" \
        -H "x-networker-request-id: bats-test-123" "${BASE}/upload"
    [ "$status" -eq 0 ]
    [[ "$output" == *"x-networker-request-id"* ]]
    [[ "$output" == *"bats-test-123"* ]]
}

# ── Echo ──────────────────────────────────────────────────────────────────────

@test "GET /echo returns method GET" {
    run curl -sf "${BASE}/echo"
    [ "$status" -eq 0 ]
    [[ "$output" =~ '"GET"' ]]
}

@test "POST /echo echoes request body exactly" {
    run curl -sf -X POST --data "ping" "${BASE}/echo"
    [ "$status" -eq 0 ]
    [ "$output" = "ping" ]
}

# ── Delay ─────────────────────────────────────────────────────────────────────

@test "GET /delay returns delayed_ms field" {
    run curl -sf "${BASE}/delay?ms=0"
    [ "$status" -eq 0 ]
    [[ "$output" =~ '"delayed_ms"' ]]
}

# ── Headers ───────────────────────────────────────────────────────────────────

@test "GET /headers echoes custom header" {
    run curl -sf -H "x-bats-test: networker" "${BASE}/headers"
    [ "$status" -eq 0 ]
    [[ "$output" =~ "x-bats-test" ]]
    [[ "$output" =~ "networker" ]]
}

# ── Status ────────────────────────────────────────────────────────────────────

@test "GET /status/404 returns 404" {
    run curl -s -o /dev/null -w "%{http_code}" "${BASE}/status/404"
    [ "$output" = "404" ]
}

@test "GET /status/200 returns 200" {
    run curl -s -o /dev/null -w "%{http_code}" "${BASE}/status/200"
    [ "$output" = "200" ]
}

@test "GET /status/503 returns 503" {
    run curl -s -o /dev/null -w "%{http_code}" "${BASE}/status/503"
    [ "$output" = "503" ]
}

# ── HTTP version ──────────────────────────────────────────────────────────────

@test "GET /http-version returns version field" {
    run curl -sf "${BASE}/http-version"
    [ "$status" -eq 0 ]
    [[ "$output" =~ '"version"' ]]
    [[ "$output" =~ "HTTP" ]]
}

# ── Page load ─────────────────────────────────────────────────────────────────

@test "GET /page returns asset_count matching request" {
    run curl -sf "${BASE}/page?assets=5&bytes=64"
    [ "$status" -eq 0 ]
    [[ "$output" =~ '"asset_count"' ]]
    [[ "$output" =~ "5" ]]
}

@test "GET /browser-page returns HTML with img tags" {
    run curl -sf "${BASE}/browser-page?assets=3&bytes=64"
    [ "$status" -eq 0 ]
    [[ "$output" =~ "<img" ]]
}

@test "GET /asset returns exact byte count" {
    local bytes
    bytes="$(curl -sf "${BASE}/asset?id=0&bytes=256" | wc -c)"
    [ "$bytes" -eq 256 ]
}
