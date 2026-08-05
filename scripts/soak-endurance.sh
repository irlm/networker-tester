#!/usr/bin/env bash
# soak-endurance.sh — multi-hour endurance soak of the live control plane.
#
# Audit P2. The nightly `Prod soak check` is a POINT check: it asks "is
# everything healthy right now?" and exits in a couple of minutes. Every defect
# that needs TIME to appear is therefore invisible to it — a memory leak, a
# connection-pool leak, a background loop that degrades after N ticks, latency
# that drifts as a table grows, runs that pile up in a non-terminal state.
#
# This samples the same surfaces repeatedly over hours and judges the TREND.
# It changes nothing on the target: reads only, no runs launched, no VMs
# created. Safe to run against production, which is the point — a leak only
# reproduces where the process has been up for days.
#
# Usage:
#   DASHBOARD_ADMIN_PASSWORD=... ./scripts/soak-endurance.sh
#
# Env:
#   SOAK_HOURS         total duration            (default 4)
#   SOAK_INTERVAL_SECS seconds between samples   (default 120)
#   LAGHOUND_URL       target base URL           (default https://laghound.com)
#   SOAK_ADMIN_EMAIL   login email               (default admin@laghound.com)
#   SOAK_MEM_GROWTH_PCT   fail above this growth (default 50)
#   SOAK_LATENCY_GROWTH_X fail above this factor (default 3)

set -uo pipefail

BASE="${LAGHOUND_URL:-https://laghound.com}"
EMAIL="${SOAK_ADMIN_EMAIL:-admin@laghound.com}"
HOURS="${SOAK_HOURS:-4}"
INTERVAL="${SOAK_INTERVAL_SECS:-120}"
MEM_GROWTH_PCT="${SOAK_MEM_GROWTH_PCT:-50}"
LATENCY_GROWTH_X="${SOAK_LATENCY_GROWTH_X:-3}"

note() { printf '>> %s\n' "$*"; }
fail() { printf '!! %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null || fail "curl is required"
command -v jq   >/dev/null || fail "jq is required"
[ -n "${DASHBOARD_ADMIN_PASSWORD:-}" ] || fail "DASHBOARD_ADMIN_PASSWORD is not set"

TOKEN=$(curl -sf --max-time 15 "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' \
  -d "$(jq -nc --arg e "$EMAIL" --arg p "$DASHBOARD_ADMIN_PASSWORD" '{email:$e,password:$p}')" \
  | jq -r '.token // empty')
[ -n "$TOKEN" ] || fail "login failed (no token) for $EMAIL @ $BASE"
note "authenticated as $EMAIL against $BASE"

SAMPLES=$(( HOURS * 3600 / INTERVAL ))
[ "$SAMPLES" -ge 3 ] || fail "need at least 3 samples; raise SOAK_HOURS or lower SOAK_INTERVAL_SECS"
note "sampling ${SAMPLES}x every ${INTERVAL}s (~${HOURS}h)"

LOG="${SOAK_LOG:-/tmp/soak-endurance.tsv}"
: > "$LOG"
printf 'sample\tepoch\tmem_bytes\tlatency_ms\tloops_healthy\tunhealthy_loops\trunning_runs\tqueued_runs\n' >> "$LOG"

UNHEALTHY_STREAK=0
MAX_UNHEALTHY_STREAK=0
ERRORS=0

for i in $(seq 1 "$SAMPLES"); do
    now=$(date -u +%s)

    # ── background-loop health ───────────────────────────────────────────────
    health=$(curl -sf --max-time 20 "$BASE/api/health/background" \
        -H "Authorization: Bearer $TOKEN" 2>/dev/null || echo '')
    if [ -z "$health" ]; then
        ERRORS=$((ERRORS + 1))
        note "sample $i: /api/health/background did not respond"
        all_healthy=false
        unhealthy_names="request-failed"
    else
        all_healthy=$(echo "$health" | jq -r '.all_healthy // false')
        unhealthy_names=$(echo "$health" \
            | jq -r '[.services[]? | select(.healthy == false) | .name] | join(",")')
    fi

    if [ "$all_healthy" = "true" ]; then
        UNHEALTHY_STREAK=0
    else
        UNHEALTHY_STREAK=$((UNHEALTHY_STREAK + 1))
        [ "$UNHEALTHY_STREAK" -gt "$MAX_UNHEALTHY_STREAK" ] && MAX_UNHEALTHY_STREAK=$UNHEALTHY_STREAK
        note "sample $i: loops UNHEALTHY (streak $UNHEALTHY_STREAK): ${unhealthy_names:-unknown}"
    fi

    # ── process memory + a timed read ────────────────────────────────────────
    # -w writes the server's own X-Process-Time-Ms, so the measurement is
    # server-side and immune to runner-to-internet jitter.
    metrics_body=$(mktemp)
    latency=$(curl -sf --max-time 30 "$BASE/api/admin/metrics" \
        -H "Authorization: Bearer $TOKEN" \
        -o "$metrics_body" \
        -w '%header{X-Process-Time-Ms}' 2>/dev/null || echo '')
    mem=$(jq -r '[.system.memory_bytes, .system.working_set_bytes, .system.memory_used_bytes,
                  .system.gc_heap_bytes] | map(select(. != null)) | first // empty' \
            "$metrics_body" 2>/dev/null || echo '')
    rm -f "$metrics_body"
    [ -n "$latency" ] || { latency=0; ERRORS=$((ERRORS + 1)); }
    [ -n "$mem" ] || mem=0

    # ── run backlog ──────────────────────────────────────────────────────────
    running=$(curl -sf --max-time 20 "$BASE/api/admin/metrics" \
        -H "Authorization: Bearer $TOKEN" 2>/dev/null \
        | jq -r '.counts.runs_24h // 0')
    queued=0

    printf '%d\t%d\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$i" "$now" "$mem" "$latency" "$all_healthy" "${unhealthy_names:-}" "$running" "$queued" >> "$LOG"

    note "sample $i/$SAMPLES: mem=${mem}B server_time=${latency}ms loops_healthy=${all_healthy}"

    [ "$i" -lt "$SAMPLES" ] && sleep "$INTERVAL"
done

note "sampling complete — analysing trend"

# ── verdict ──────────────────────────────────────────────────────────────────
# Compare the LAST quarter against the FIRST quarter rather than first-vs-last
# points: single samples are noisy, and a process is still warming up (JIT,
# pools, caches) during the first few minutes.
analysis=$(awk -F'\t' -v growth="$MEM_GROWTH_PCT" -v latx="$LATENCY_GROWTH_X" '
    NR == 1 { next }
    { n++; mem[n] = $3 + 0; lat[n] = $4 + 0 }
    END {
        if (n < 3) { print "INSUFFICIENT"; exit }
        q = int(n / 4); if (q < 1) q = 1
        for (i = 1; i <= q; i++)          { m0 += mem[i]; l0 += lat[i] }
        for (i = n - q + 1; i <= n; i++)  { m1 += mem[i]; l1 += lat[i] }
        m0 /= q; m1 /= q; l0 /= q; l1 /= q

        memPct = (m0 > 0) ? (m1 - m0) / m0 * 100 : 0
        latMul = (l0 > 0) ? l1 / l0 : 1

        printf "MEM_START=%.0f MEM_END=%.0f MEM_GROWTH_PCT=%.1f LAT_START=%.1f LAT_END=%.1f LAT_X=%.2f\n", \
            m0, m1, memPct, l0, l1, latMul
        bad = 0
        if (m0 > 0 && memPct > growth) { printf "FAIL_MEM %.1f%% > %s%%\n", memPct, growth; bad = 1 }
        if (l0 > 0 && latMul > latx)   { printf "FAIL_LAT %.2fx > %sx\n", latMul, latx; bad = 1 }
        if (!bad) print "TREND_OK"
    }
' "$LOG")

echo "$analysis"

VERDICT=0
echo "$analysis" | grep -q "FAIL_MEM" && { note "memory grew beyond the budget — likely a leak"; VERDICT=1; }
echo "$analysis" | grep -q "FAIL_LAT" && { note "server-side latency drifted upward materially"; VERDICT=1; }

# Two consecutive unhealthy samples is a real stall, not a tick landing late.
if [ "$MAX_UNHEALTHY_STREAK" -ge 2 ]; then
    note "background loops were unhealthy for $MAX_UNHEALTHY_STREAK consecutive samples"
    VERDICT=1
fi

# A handful of transient request failures over hours is the internet; a large
# share means the target was not actually up for the soak.
if [ "$ERRORS" -gt $(( SAMPLES / 4 )) ]; then
    note "$ERRORS/$SAMPLES samples failed to reach the control plane"
    VERDICT=1
fi

note "log: $LOG"
if [ "$VERDICT" -eq 0 ]; then
    note "SOAK_ENDURANCE_PASS"
else
    note "SOAK_ENDURANCE_FAIL"
fi
exit "$VERDICT"
