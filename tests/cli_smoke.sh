#!/usr/bin/env bash
#
# tests/cli_smoke.sh — CLI smoke gate for the persistent-testers work.
#
# Purpose
#   Exercise the existing networker-tester CLI in a fixed set of scenarios so
#   we have a known-good baseline BEFORE the persistent-testers feature lands.
#   Re-run after every meaningful change to catch regressions.
#
# Usage
#   bash tests/cli_smoke.sh
#
# Env toggles
#   SMOKE_TEST_DASHBOARD=1   Enable scenario 7 (deferred to Task 35).
#   SMOKE_TEST_AZURE=1       Enable scenario 8 (deferred to Task 35).
#   SMOKE_ENDPOINT_HTTP_PORT Override endpoint HTTP port    (default 18080).
#   SMOKE_ENDPOINT_HTTPS_PORT Override endpoint HTTPS port  (default 18443).
#   SMOKE_ENDPOINT_UDP_PORT  Override endpoint UDP port     (default 19999).
#   SMOKE_CARGO_PROFILE      "debug" (default) or "release".
#
# Exit status
#   Number of failed scenarios (0 == all good).
#
# Notes
#   - Uses `set -u` but NOT `set -e`; we want to keep running and count fails.
#   - Scenario 6 (workspace builds/clippy/test) is the hard gate.
#   - Scenarios 1-5 should pass on any healthy checkout; some require
#     outbound network (DNS, cloudflare.com, public domains).

set -u

# ─── Config ──────────────────────────────────────────────────────────────────

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR" || { echo "FATAL: cannot cd to $ROOT_DIR" >&2; exit 255; }

HTTP_PORT="${SMOKE_ENDPOINT_HTTP_PORT:-18080}"
HTTPS_PORT="${SMOKE_ENDPOINT_HTTPS_PORT:-18443}"
UDP_PORT="${SMOKE_ENDPOINT_UDP_PORT:-19999}"
UDP_TP_PORT="${SMOKE_ENDPOINT_UDP_TP_PORT:-19998}"
PROFILE="${SMOKE_CARGO_PROFILE:-debug}"

CARGO_BUILD_FLAG=""
if [ "$PROFILE" = "release" ]; then
    CARGO_BUILD_FLAG="--release"
fi

SMOKE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/networker_cli_smoke.XXXXXX")"
ENDPOINT_LOG="$SMOKE_DIR/endpoint.log"
ENDPOINT_PID=""
SQLITE_DB="$SMOKE_DIR/cli_smoke.db"

RESULTS=()   # lines of "PASS|FAIL|SKIP\t<scenario>\t<detail>"
FAILS=0
PASSES=0
SKIPS=0

# ─── Cleanup ─────────────────────────────────────────────────────────────────

cleanup() {
    if [ -n "$ENDPOINT_PID" ] && kill -0 "$ENDPOINT_PID" 2>/dev/null; then
        kill "$ENDPOINT_PID" 2>/dev/null || true
        # Give it a beat, then hard-kill
        sleep 0.5
        kill -9 "$ENDPOINT_PID" 2>/dev/null || true
    fi
    # Also clean any stray networker-endpoint we may have accidentally orphaned
    # (only processes bound to our ports — be conservative).
    rm -rf "$SMOKE_DIR" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# ─── Helpers ─────────────────────────────────────────────────────────────────

log() { printf '[smoke] %s\n' "$*"; }
hr()  { printf -- '─%.0s' $(seq 1 72); printf '\n'; }

record_pass() {
    PASSES=$((PASSES + 1))
    RESULTS+=("PASS	$1	$2")
    log "PASS  $1 — $2"
}

record_fail() {
    FAILS=$((FAILS + 1))
    RESULTS+=("FAIL	$1	$2")
    log "FAIL  $1 — $2"
}

record_skip() {
    SKIPS=$((SKIPS + 1))
    RESULTS+=("SKIP	$1	$2")
    log "SKIP  $1 — $2"
}

# Wait until a TCP port accepts connections (or timeout). Returns 0/1.
wait_for_port() {
    local host="$1" port="$2" tries="${3:-40}"
    local i=0
    while [ $i -lt "$tries" ]; do
        if (exec 3<>/dev/tcp/"$host"/"$port") 2>/dev/null; then
            exec 3<&- 3>&- 2>/dev/null || true
            return 0
        fi
        sleep 0.25
        i=$((i + 1))
    done
    return 1
}

# Run networker-tester with args; capture combined output to file.
# Usage: run_tester <logfile> <args...>
run_tester() {
    local logfile="$1"
    shift
    cargo run $CARGO_BUILD_FLAG --quiet -p networker-tester -- "$@" \
        >"$logfile" 2>&1
    return $?
}

start_endpoint() {
    log "Starting networker-endpoint on http=$HTTP_PORT https=$HTTPS_PORT udp=$UDP_PORT"
    (
        cargo run $CARGO_BUILD_FLAG --quiet -p networker-endpoint -- \
            --http-port "$HTTP_PORT" \
            --https-port "$HTTPS_PORT" \
            --udp-port "$UDP_PORT" \
            --udp-throughput-port "$UDP_TP_PORT" \
            >"$ENDPOINT_LOG" 2>&1
    ) &
    ENDPOINT_PID=$!

    # Endpoint may need to compile on first run; wait generously.
    if ! wait_for_port 127.0.0.1 "$HTTP_PORT" 240; then
        log "endpoint failed to bind $HTTP_PORT within timeout"
        log "---- endpoint log (tail) ----"
        tail -n 40 "$ENDPOINT_LOG" 2>/dev/null || true
        log "-----------------------------"
        return 1
    fi
    # HTTPS comes up right after HTTP — best effort.
    wait_for_port 127.0.0.1 "$HTTPS_PORT" 40 || true
    return 0
}

# ─── Scenarios ───────────────────────────────────────────────────────────────

scenario_1_local_http2() {
    hr
    log "Scenario 1: Local HTTP/2 probe against networker-endpoint"
    local out="$SMOKE_DIR/s1.log"
    if run_tester "$out" \
        --target "https://127.0.0.1:$HTTPS_PORT/health" \
        --modes http2 \
        --runs 1 \
        --insecure \
        --json-stdout; then
        record_pass "s1-local-http2" "HTTP/2 probe against local endpoint ok"
    else
        record_fail "s1-local-http2" "see $out"
        tail -n 20 "$out" >&2 || true
    fi
}

scenario_2_local_http3() {
    hr
    log "Scenario 2: HTTP/3 (QUIC) probe against networker-endpoint"
    local out="$SMOKE_DIR/s2.log"
    if run_tester "$out" \
        --target "https://127.0.0.1:$HTTPS_PORT/health" \
        --modes http3 \
        --runs 1 \
        --insecure \
        --json-stdout; then
        record_pass "s2-local-http3" "HTTP/3 probe against local endpoint ok"
    else
        record_fail "s2-local-http3" "see $out"
        tail -n 20 "$out" >&2 || true
    fi
}

scenario_3_public_dns() {
    hr
    log "Scenario 3: DNS probe against a public domain"
    local out="$SMOKE_DIR/s3.log"
    if run_tester "$out" \
        --target "https://www.cloudflare.com/" \
        --modes dns \
        --runs 1 \
        --json-stdout; then
        record_pass "s3-public-dns" "DNS probe vs cloudflare.com ok"
    else
        record_fail "s3-public-dns" "see $out (needs outbound DNS)"
        tail -n 20 "$out" >&2 || true
    fi
}

scenario_4_all_modes_public() {
    hr
    log "Scenario 4: Mixed-mode probe against https://www.cloudflare.com"
    local out="$SMOKE_DIR/s4.log"
    # "All modes" in practice = the core transport/HTTP set.
    # Avoid webdownload/webupload/pageload/browser: they need our endpoint
    # or extra features.
    if run_tester "$out" \
        --target "https://www.cloudflare.com/" \
        --modes dns,tcp,tls,http1,http2,http3 \
        --runs 1 \
        --json-stdout; then
        record_pass "s4-all-modes-public" "dns+tcp+tls+http1/2/3 vs cloudflare.com ok"
    else
        record_fail "s4-all-modes-public" "see $out (needs outbound network)"
        tail -n 20 "$out" >&2 || true
    fi
}

scenario_5_file_persistence() {
    hr
    log "Scenario 5: File persistence (JSON+HTML artifact)"
    # NOTE: the plan mentions '--features sqlite --output-db', but the
    # networker-tester crate does not expose a sqlite feature (only db-mssql
    # and db-postgres). We substitute the equivalent persistence surface that
    # the CLI actually supports today: --output-dir, which writes a TestRun
    # JSON artifact + HTML report. Task 35 can revisit if sqlite lands.
    local out="$SMOKE_DIR/s5.log"
    local out_dir="$SMOKE_DIR/s5_out"
    mkdir -p "$out_dir"
    if run_tester "$out" \
        --target "https://127.0.0.1:$HTTPS_PORT/health" \
        --modes http1 \
        --runs 1 \
        --insecure \
        --output-dir "$out_dir"; then
        # Verify at least one JSON file was written.
        if ls "$out_dir"/*.json >/dev/null 2>&1; then
            record_pass "s5-file-persistence" "artifact written to $out_dir"
        else
            record_fail "s5-file-persistence" "tester exited 0 but no JSON in $out_dir"
        fi
    else
        record_fail "s5-file-persistence" "see $out"
        tail -n 20 "$out" >&2 || true
    fi
    # Also make sure the sqlite db path the plan referenced is clean.
    rm -f "$SQLITE_DB"
}

scenario_6_workspace_builds() {
    hr
    log "Scenario 6: Workspace build matrix + clippy + lib tests"
    local step_log="$SMOKE_DIR/s6.log"
    local failed=0
    local steps=(
        "cargo build --workspace"
        "cargo build -p networker-tester --no-default-features"
        "cargo build -p networker-tester --all-features"
        "cargo clippy --all-targets -- -D warnings"
        "cargo test --workspace --lib"
    )
    for step in "${steps[@]}"; do
        log "  » $step"
        if ! eval "$step" >>"$step_log" 2>&1; then
            log "    FAILED: $step (see $step_log)"
            failed=$((failed + 1))
        fi
    done
    if [ "$failed" -eq 0 ]; then
        record_pass "s6-workspace-build" "build/clippy/test matrix clean"
    else
        record_fail "s6-workspace-build" "$failed step(s) failed; see $step_log"
        tail -n 40 "$step_log" >&2 || true
    fi
}

scenario_7_dashboard_phases() {
    hr
    log "Scenario 7: CLI reports phases to dashboard"
    if [ "${SMOKE_TEST_DASHBOARD:-0}" != "1" ]; then
        record_skip "s7-dashboard-phases" "deferred to Task 35 (set SMOKE_TEST_DASHBOARD=1 to enable)"
        return
    fi
    record_skip "s7-dashboard-phases" "implementation deferred to Task 35"
}

scenario_8_e2e_persistent_tester() {
    hr
    log "Scenario 8: E2E persistent-tester flow"
    if [ "${SMOKE_TEST_AZURE:-0}" != "1" ]; then
        record_skip "s8-e2e-persistent-tester" "deferred to Task 35 (set SMOKE_TEST_AZURE=1 to enable)"
        return
    fi
    record_skip "s8-e2e-persistent-tester" "implementation deferred to Task 35"
}

# ─── Driver ──────────────────────────────────────────────────────────────────

hr
log "networker-tester CLI smoke gate"
log "Root: $ROOT_DIR"
log "Tmp:  $SMOKE_DIR"
log "Profile: $PROFILE"
hr

# Scenario 6 (build) runs first so the rest of the scenarios can reuse the
# compiled binaries and start fast. If builds are broken, everything else is
# meaningless anyway.
scenario_6_workspace_builds

if ! start_endpoint; then
    record_fail "endpoint-boot" "networker-endpoint failed to start; scenarios 1,2,5 will fail"
fi

scenario_1_local_http2
scenario_2_local_http3
scenario_3_public_dns
scenario_4_all_modes_public
scenario_5_file_persistence
scenario_7_dashboard_phases
scenario_8_e2e_persistent_tester

# ─── Summary ─────────────────────────────────────────────────────────────────

hr
log "Summary"
for row in "${RESULTS[@]}"; do
    printf '  %s\n' "$row"
done
hr
TOTAL=${#RESULTS[@]}
log "$PASSES/$TOTAL passed, $FAILS/$TOTAL failed ($SKIPS deferred)"
hr

exit "$FAILS"
