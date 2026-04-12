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

CARGO_BUILD_FLAGS=()
if [ "$PROFILE" = "release" ]; then
    CARGO_BUILD_FLAGS=(--release)
fi

SMOKE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/networker_cli_smoke.XXXXXX")"
ENDPOINT_LOG="$SMOKE_DIR/endpoint.log"
ENDPOINT_PID=""

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
    if [ -n "${DASHBOARD_PID:-}" ] && kill -0 "$DASHBOARD_PID" 2>/dev/null; then
        kill "$DASHBOARD_PID" 2>/dev/null || true
        sleep 0.5
        kill -9 "$DASHBOARD_PID" 2>/dev/null || true
    fi
    rm -rf "$SMOKE_DIR" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# ─── Helpers ─────────────────────────────────────────────────────────────────

log() { printf '[smoke] %s\n' "$*"; }
hr()  { printf -- '─%.0s' $(seq 1 72); printf '\n'; }

record_pass() {
    PASSES=$((PASSES + 1))
    RESULTS+=("PASS"$'\t'"$1"$'\t'"$2")
    log "PASS  $1 — $2"
}

record_fail() {
    FAILS=$((FAILS + 1))
    RESULTS+=("FAIL"$'\t'"$1"$'\t'"$2")
    log "FAIL  $1 — $2"
}

record_skip() {
    SKIPS=$((SKIPS + 1))
    RESULTS+=("SKIP"$'\t'"$1"$'\t'"$2")
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
    cargo run ${CARGO_BUILD_FLAGS[@]+"${CARGO_BUILD_FLAGS[@]}"} --quiet -p networker-tester -- "$@" \
        >"$logfile" 2>&1
    return $?
}

start_endpoint() {
    log "Starting networker-endpoint on http=$HTTP_PORT https=$HTTPS_PORT udp=$UDP_PORT"
    (
        cargo run ${CARGO_BUILD_FLAGS[@]+"${CARGO_BUILD_FLAGS[@]}"} --quiet -p networker-endpoint -- \
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

# ── Dashboard helpers (shared by scenarios 7 + 8) ───────────────────────────

DASHBOARD_PID=""
DASHBOARD_PORT_USED="3000"
DASHBOARD_ADMIN_EMAIL_USED="admin@example.com"
DASHBOARD_ADMIN_PASSWORD_USED="smokesmoke1!"
DASHBOARD_TOKEN=""

stop_dashboard() {
    if [ -n "$DASHBOARD_PID" ] && kill -0 "$DASHBOARD_PID" 2>/dev/null; then
        log "Stopping dashboard (pid=$DASHBOARD_PID)"
        kill "$DASHBOARD_PID" 2>/dev/null || true
        sleep 0.5
        kill -9 "$DASHBOARD_PID" 2>/dev/null || true
    fi
    DASHBOARD_PID=""
}

# Verify postgres (docker-compose.dashboard.yml) is up on 127.0.0.1:5432.
# Returns 0 if reachable, 1 otherwise. Does NOT start it — operator-owned.
check_postgres_up() {
    wait_for_port 127.0.0.1 5432 4
}

# Start dashboard in background. Assumes postgres is reachable.
# On success: $DASHBOARD_PID is set, port 3000 is listening.
start_dashboard() {
    local log_file="$SMOKE_DIR/dashboard.log"
    log "Starting networker-dashboard on port $DASHBOARD_PORT_USED (log: $log_file)"
    (
        DASHBOARD_DB_URL="postgres://networker:networker@127.0.0.1:5432/networker_core" \
        DASHBOARD_ADMIN_EMAIL="$DASHBOARD_ADMIN_EMAIL_USED" \
        DASHBOARD_ADMIN_PASSWORD="$DASHBOARD_ADMIN_PASSWORD_USED" \
        DASHBOARD_JWT_SECRET="smoke-test-secret-not-for-production-use-only" \
        DASHBOARD_PORT="$DASHBOARD_PORT_USED" \
        cargo run ${CARGO_BUILD_FLAGS[@]+"${CARGO_BUILD_FLAGS[@]}"} --quiet -p networker-dashboard \
            >"$log_file" 2>&1 \
            </dev/null
    ) &
    DASHBOARD_PID=$!

    if ! wait_for_port 127.0.0.1 "$DASHBOARD_PORT_USED" 480; then
        log "dashboard failed to bind :$DASHBOARD_PORT_USED"
        log "---- dashboard log (tail) ----"
        tail -n 60 "$log_file" 2>/dev/null || true
        log "------------------------------"
        return 1
    fi
    return 0
}

# POST /auth/login; sets $DASHBOARD_TOKEN on success.
dashboard_login() {
    local resp="$SMOKE_DIR/login.json"
    local payload
    payload=$(printf '{"email":"%s","password":"%s"}' \
        "$DASHBOARD_ADMIN_EMAIL_USED" "$DASHBOARD_ADMIN_PASSWORD_USED")
    if ! curl -fsS -X POST \
        -H 'Content-Type: application/json' \
        -d "$payload" \
        "http://127.0.0.1:$DASHBOARD_PORT_USED/api/auth/login" \
        -o "$resp" 2>>"$SMOKE_DIR/dashboard.log"; then
        log "login failed (see $resp and dashboard.log)"
        return 1
    fi
    DASHBOARD_TOKEN=$(python3 -c \
        'import json,sys; print(json.load(open(sys.argv[1]))["token"])' \
        "$resp" 2>/dev/null || true)
    if [ -z "$DASHBOARD_TOKEN" ]; then
        log "login response did not contain a token: $(cat "$resp")"
        return 1
    fi
    return 0
}

scenario_7_dashboard_phases() {
    hr
    log "Scenario 7: CLI reports phases to dashboard"
    if [ "${SMOKE_TEST_DASHBOARD:-0}" != "1" ]; then
        record_skip "s7-dashboard-phases" "gated (set SMOKE_TEST_DASHBOARD=1 to enable)"
        return
    fi

    if ! command -v curl >/dev/null 2>&1; then
        record_skip "s7-dashboard-phases" "curl not available"
        return
    fi
    if ! command -v python3 >/dev/null 2>&1; then
        record_skip "s7-dashboard-phases" "python3 not available (needed for JSON parsing)"
        return
    fi

    if ! check_postgres_up; then
        record_skip "s7-dashboard-phases" \
            "postgres :5432 unreachable — run: docker compose -f docker-compose.dashboard.yml up -d postgres"
        return
    fi

    if ! start_dashboard; then
        record_fail "s7-dashboard-phases" "dashboard failed to start"
        stop_dashboard
        return
    fi

    if ! dashboard_login; then
        record_fail "s7-dashboard-phases" "failed to obtain JWT via /api/auth/login"
        stop_dashboard
        return
    fi
    log "Obtained JWT (len=${#DASHBOARD_TOKEN})"

    # The CLI does not currently expose --dashboard-url / --dashboard-token
    # flags (verified in crates/networker-tester/src/cli.rs as of commit
    # c74a2ab). Once added, this scenario should:
    #   1. Run:
    #        cargo run -p networker-tester -- \
    #          --target http://127.0.0.1:$HTTP_PORT \
    #          --modes http1 --runs 1 \
    #          --dashboard-url http://127.0.0.1:$DASHBOARD_PORT_USED \
    #          --dashboard-token "$DASHBOARD_TOKEN"
    #   2. Query benchmark_config and assert
    #        current_phase='done' AND outcome='success'
    #
    # For now we exercise the dashboard+login boot path and record a skip
    # for the verification half so the reason is actionable.
    stop_dashboard
    record_skip "s7-dashboard-phases" \
        "TODO: networker-tester CLI lacks --dashboard-url/--dashboard-token flags; boot + login paths verified"
}

scenario_8_e2e_persistent_tester() {
    hr
    log "Scenario 8: E2E persistent-tester flow"
    if [ "${SMOKE_TEST_AZURE:-0}" != "1" ]; then
        record_skip "s8-e2e-persistent-tester" "gated (set SMOKE_TEST_AZURE=1 to enable)"
        return
    fi

    if ! command -v curl >/dev/null 2>&1; then
        record_skip "s8-e2e-persistent-tester" "curl not available"
        return
    fi
    if ! command -v python3 >/dev/null 2>&1; then
        record_skip "s8-e2e-persistent-tester" "python3 not available"
        return
    fi
    if ! command -v az >/dev/null 2>&1; then
        record_skip "s8-e2e-persistent-tester" "az CLI not installed"
        return
    fi
    if ! az account show >/dev/null 2>&1; then
        record_skip "s8-e2e-persistent-tester" "az account show failed — not logged in"
        return
    fi
    if ! check_postgres_up; then
        record_skip "s8-e2e-persistent-tester" \
            "postgres :5432 unreachable — run: docker compose -f docker-compose.dashboard.yml up -d postgres"
        return
    fi

    if ! start_dashboard; then
        record_fail "s8-e2e-persistent-tester" "dashboard failed to start"
        stop_dashboard
        return
    fi
    if ! dashboard_login; then
        record_fail "s8-e2e-persistent-tester" "failed to obtain JWT"
        stop_dashboard
        return
    fi

    local api_base="http://127.0.0.1:$DASHBOARD_PORT_USED/api"
    # The smoke run assumes a default project slug exists. An operator with a
    # non-default setup can override with SMOKE_PROJECT_ID.
    local pid="${SMOKE_PROJECT_ID:-default}"
    local auth_header="Authorization: Bearer $DASHBOARD_TOKEN"
    local ct_header="Content-Type: application/json"

    # 1) Create a persistent tester.
    local create_body='{"name":"smoke-s8","cloud":"azure","region":"eastus","auto_probe_enabled":false}'
    local create_resp="$SMOKE_DIR/s8_create.json"
    if ! curl -fsS -X POST \
        -H "$auth_header" -H "$ct_header" \
        -d "$create_body" \
        "$api_base/projects/$pid/testers" \
        -o "$create_resp"; then
        record_fail "s8-e2e-persistent-tester" "POST /projects/$pid/testers failed"
        stop_dashboard
        return
    fi
    local tid
    tid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$create_resp" 2>/dev/null || true)
    if [ -z "$tid" ]; then
        record_fail "s8-e2e-persistent-tester" "no id in create-tester response"
        stop_dashboard
        return
    fi
    log "Created tester id=$tid"

    # 2) Poll until power_state='running' AND allocation='idle' (15 min max).
    local deadline=$(( $(date +%s) + 900 ))
    local tester_json="$SMOKE_DIR/s8_tester.json"
    local power_state="" allocation=""
    while :; do
        if [ "$(date +%s)" -gt "$deadline" ]; then
            record_fail "s8-e2e-persistent-tester" "tester did not reach running+idle within 15m"
            curl -fsS -X DELETE -H "$auth_header" "$api_base/projects/$pid/testers/$tid" >/dev/null 2>&1 || true
            stop_dashboard
            return
        fi
        if curl -fsS -H "$auth_header" "$api_base/projects/$pid/testers/$tid" -o "$tester_json"; then
            power_state=$(python3 -c \
                'import json,sys; print(json.load(open(sys.argv[1])).get("power_state",""))' \
                "$tester_json" 2>/dev/null || true)
            allocation=$(python3 -c \
                'import json,sys; print(json.load(open(sys.argv[1])).get("allocation",""))' \
                "$tester_json" 2>/dev/null || true)
            log "tester state: power=$power_state alloc=$allocation"
            if [ "$power_state" = "running" ] && [ "$allocation" = "idle" ]; then
                break
            fi
        fi
        sleep 10
    done

    # 3) Create a benchmark_config bound to this tester.
    local cfg_body
    cfg_body=$(printf '{"name":"smoke-s8","benchmark_type":"application","tester_id":"%s","testbeds":[{"cloud":"azure","region":"eastus","proxies":["nginx"]}]}' "$tid")
    local cfg_resp="$SMOKE_DIR/s8_cfg.json"
    if ! curl -fsS -X POST \
        -H "$auth_header" -H "$ct_header" \
        -d "$cfg_body" \
        "$api_base/projects/$pid/benchmark-configs" \
        -o "$cfg_resp"; then
        record_fail "s8-e2e-persistent-tester" "POST benchmark-configs failed: $(cat "$cfg_resp" 2>/dev/null)"
        curl -fsS -X DELETE -H "$auth_header" "$api_base/projects/$pid/testers/$tid" >/dev/null 2>&1 || true
        stop_dashboard
        return
    fi
    local cid
    cid=$(python3 -c \
        'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("config_id") or d.get("id",""))' \
        "$cfg_resp" 2>/dev/null || true)
    if [ -z "$cid" ]; then
        record_fail "s8-e2e-persistent-tester" "no config_id in create-config response"
        curl -fsS -X DELETE -H "$auth_header" "$api_base/projects/$pid/testers/$tid" >/dev/null 2>&1 || true
        stop_dashboard
        return
    fi
    log "Created benchmark_config id=$cid"

    # 4) Launch it.
    if ! curl -fsS -X POST \
        -H "$auth_header" -H "$ct_header" \
        -d '{}' \
        "$api_base/projects/$pid/benchmark-configs/$cid/launch" \
        -o "$SMOKE_DIR/s8_launch.json"; then
        record_fail "s8-e2e-persistent-tester" "POST benchmark-configs/$cid/launch failed"
        curl -fsS -X DELETE -H "$auth_header" "$api_base/projects/$pid/testers/$tid" >/dev/null 2>&1 || true
        stop_dashboard
        return
    fi
    log "Launched benchmark_config id=$cid"

    # 5) Poll benchmark_config status (20 min max).
    deadline=$(( $(date +%s) + 1200 ))
    local cfg_status=""
    local cfg_state_json="$SMOKE_DIR/s8_cfg_state.json"
    while :; do
        if [ "$(date +%s)" -gt "$deadline" ]; then
            record_fail "s8-e2e-persistent-tester" "benchmark did not reach terminal state within 20m"
            curl -fsS -X DELETE -H "$auth_header" "$api_base/projects/$pid/testers/$tid" >/dev/null 2>&1 || true
            stop_dashboard
            return
        fi
        if curl -fsS -H "$auth_header" \
            "$api_base/projects/$pid/benchmark-configs/$cid" \
            -o "$cfg_state_json"; then
            cfg_status=$(python3 -c \
                'import json,sys; d=json.load(open(sys.argv[1])); c=d.get("config",d); print(c.get("status",""))' \
                "$cfg_state_json" 2>/dev/null || true)
            log "benchmark status: $cfg_status"
            case "$cfg_status" in
                completed|completed_with_errors|failed|cancelled)
                    break
                    ;;
            esac
        fi
        sleep 15
    done

    if [ "$cfg_status" != "completed" ] && [ "$cfg_status" != "completed_with_errors" ]; then
        record_fail "s8-e2e-persistent-tester" "benchmark terminal status=$cfg_status (expected completed*)"
        curl -fsS -X DELETE -H "$auth_header" "$api_base/projects/$pid/testers/$tid" >/dev/null 2>&1 || true
        stop_dashboard
        return
    fi

    # 6) Tester persistence check: it should still exist and be idle again.
    if ! curl -fsS -H "$auth_header" \
        "$api_base/projects/$pid/testers/$tid" -o "$tester_json"; then
        record_fail "s8-e2e-persistent-tester" "tester GET after benchmark failed (tester destroyed?)"
        stop_dashboard
        return
    fi
    allocation=$(python3 -c \
        'import json,sys; print(json.load(open(sys.argv[1])).get("allocation",""))' \
        "$tester_json" 2>/dev/null || true)
    if [ "$allocation" != "idle" ]; then
        record_fail "s8-e2e-persistent-tester" "tester allocation after benchmark=$allocation (expected idle)"
        curl -fsS -X DELETE -H "$auth_header" "$api_base/projects/$pid/testers/$tid" >/dev/null 2>&1 || true
        stop_dashboard
        return
    fi

    # 7) Cleanup — destroy the VM so we don't bleed Azure cost.
    log "Cleaning up Azure tester $tid"
    if ! curl -fsS -X DELETE -H "$auth_header" \
        "$api_base/projects/$pid/testers/$tid" -o /dev/null; then
        record_fail "s8-e2e-persistent-tester" "DELETE tester failed — MANUAL CLEANUP REQUIRED for tid=$tid"
        stop_dashboard
        return
    fi

    stop_dashboard
    record_pass "s8-e2e-persistent-tester" "persistent tester round-trip ok (status=$cfg_status)"
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

if [ "$FAILS" -gt 0 ]; then
    log "Workspace build gate failed — skipping endpoint-based scenarios."
else
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
fi

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
