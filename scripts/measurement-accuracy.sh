#!/usr/bin/env bash
# ==============================================================================
# Measurement-accuracy benchmark (audit P0-8)
#
# THE PRODUCT'S CORE CLAIM IS THAT ITS NUMBERS ARE TRUE — and until this
# script existed, nothing anywhere validated a reported figure against a link
# with KNOWN ground truth. Every other test proves the code runs; this proves
# the code MEASURES.
#
# Method: shape the loopback with `tc netem` (a kernel qdisc — the same
# mechanism used to validate iperf/netperf), run the tester against a local
# networker-endpoint over that shaped path, and require the reported RTT and
# throughput to land inside a tolerance band around the imposed values.
#
# Run:  sudo scripts/measurement-accuracy.sh
# CI:   .github/workflows/ci.yml → measurement-accuracy job (ubuntu-latest)
#
# Exit non-zero if any measurement falls outside its band.
# ==============================================================================
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IFACE="${NETEM_IFACE:-lo}"
HTTP_PORT="${NETEM_HTTP_PORT:-18080}"
HTTPS_PORT="${NETEM_HTTPS_PORT:-18443}"
UDP_PORT="${NETEM_UDP_PORT:-19999}"
UDP_TP_PORT="${NETEM_UDP_TP_PORT:-19998}"
OUT_DIR="${NETEM_OUT_DIR:-/tmp/netem-accuracy}"
BASELINE_FILE="${ROOT}/benchmarks/baselines/measurement-accuracy.json"

# Imposed conditions and the tolerance each measurement must land within.
#   delay: one-way ms → RTT is 2× this on loopback (both directions shaped)
#   rate:  link ceiling in mbit
IMPOSED_DELAY_MS="${NETEM_DELAY_MS:-25}"
IMPOSED_RATE_MBIT="${NETEM_RATE_MBIT:-100}"
RTT_TOLERANCE_MS="${NETEM_RTT_TOLERANCE_MS:-8}"
THROUGHPUT_TOLERANCE_PCT="${NETEM_THROUGHPUT_TOLERANCE_PCT:-20}"

ENDPOINT_PID=""
NETEM_APPLIED=0

cleanup() {
    if [[ "$NETEM_APPLIED" == "1" ]]; then
        tc qdisc del dev "$IFACE" root 2>/dev/null || true
        echo ">> netem removed from ${IFACE}"
    fi
    if [[ -n "$ENDPOINT_PID" ]]; then
        kill "$ENDPOINT_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

require_root() {
    if [[ "$(id -u)" != "0" ]]; then
        echo "ERROR: tc netem requires root. Re-run with sudo." >&2
        exit 2
    fi
}

start_endpoint() {
    local bin="${ROOT}/target/release/networker-endpoint"
    [[ -x "$bin" ]] || bin="${ROOT}/target/debug/networker-endpoint"
    if [[ ! -x "$bin" ]]; then
        echo "ERROR: networker-endpoint not built (cargo build -p networker-endpoint)" >&2
        exit 2
    fi
    "$bin" --http-port "$HTTP_PORT" --https-port "$HTTPS_PORT" \
           --udp-port "$UDP_PORT" --udp-throughput-port "$UDP_TP_PORT" \
           > "${OUT_DIR}/endpoint.log" 2>&1 &
    ENDPOINT_PID=$!
    for _ in $(seq 1 40); do
        if curl -sf --max-time 2 "http://127.0.0.1:${HTTP_PORT}/health" >/dev/null 2>&1; then
            echo ">> endpoint healthy (pid ${ENDPOINT_PID})"
            return 0
        fi
        sleep 0.5
    done
    echo "ERROR: endpoint did not become healthy" >&2
    tail -20 "${OUT_DIR}/endpoint.log" >&2 || true
    exit 2
}

apply_netem() {
    tc qdisc del dev "$IFACE" root 2>/dev/null || true
    # netem delay applies per direction on loopback, so a request's RTT sees
    # it twice. rate caps the egress the same way a link ceiling would.
    if ! tc qdisc add dev "$IFACE" root netem \
            delay "${IMPOSED_DELAY_MS}ms" rate "${IMPOSED_RATE_MBIT}mbit"; then
        echo "ERROR: failed to apply netem (is sch_netem available?)" >&2
        exit 2
    fi
    NETEM_APPLIED=1
    echo ">> netem on ${IFACE}: delay=${IMPOSED_DELAY_MS}ms rate=${IMPOSED_RATE_MBIT}mbit"
}

run_tester() {
    local modes="$1" out="$2" runs="${3:-5}"
    local bin="${ROOT}/target/release/networker-tester"
    [[ -x "$bin" ]] || bin="${ROOT}/target/debug/networker-tester"
    "$bin" \
        --target "http://127.0.0.1:${HTTP_PORT}" \
        --modes "$modes" \
        --runs "$runs" \
        --payload-size 2000000 \
        --json-stdout \
        > "$out" 2>"${out}.err" || true
    if [[ ! -s "$out" ]]; then
        echo "ERROR: tester produced no JSON for modes=${modes}" >&2
        tail -20 "${out}.err" >&2 || true
        exit 2
    fi
}

main() {
    require_root
    mkdir -p "$OUT_DIR" "$(dirname "$BASELINE_FILE")"

    start_endpoint
    apply_netem

    echo ">> measuring latency (tcp) under the shaped path…"
    run_tester "tcp" "${OUT_DIR}/latency.json" 10

    echo ">> measuring throughput (download) under the shaped path…"
    run_tester "download" "${OUT_DIR}/throughput.json" 3

    python3 - "$OUT_DIR" "$IMPOSED_DELAY_MS" "$IMPOSED_RATE_MBIT" \
                "$RTT_TOLERANCE_MS" "$THROUGHPUT_TOLERANCE_PCT" "$BASELINE_FILE" <<'PY'
import json, sys, statistics, pathlib, datetime

out_dir, delay_ms, rate_mbit, rtt_tol, thr_tol_pct, baseline_path = sys.argv[1:7]
delay_ms = float(delay_ms); rate_mbit = float(rate_mbit)
rtt_tol = float(rtt_tol); thr_tol_pct = float(thr_tol_pct)

def load(name):
    with open(f"{out_dir}/{name}") as f:
        return json.load(f)

def walk_attempts(doc):
    """Yield every attempt dict regardless of nesting shape."""
    stack = [doc]
    while stack:
        cur = stack.pop()
        if isinstance(cur, dict):
            if "attempts" in cur and isinstance(cur["attempts"], list):
                for a in cur["attempts"]:
                    if isinstance(a, dict):
                        yield a
            stack.extend(v for v in cur.values() if isinstance(v, (dict, list)))
        elif isinstance(cur, list):
            stack.extend(v for v in cur if isinstance(v, (dict, list)))

def collect(doc, key):
    vals = []
    for a in walk_attempts(doc):
        v = a.get(key)
        if isinstance(v, (int, float)) and v > 0:
            vals.append(float(v))
    return vals

failures, results = [], {}

# ── Latency: netem delays BOTH directions, so a TCP connect RTT ≈ 2×delay ──
lat_doc = load("latency.json")
tcp_ms = collect(lat_doc, "tcp_ms") or collect(lat_doc, "total_ms")
if not tcp_ms:
    failures.append("no tcp timing samples parsed from latency.json")
else:
    measured = statistics.median(tcp_ms)
    expected = 2 * delay_ms
    delta = abs(measured - expected)
    results["rtt"] = {"expected_ms": expected, "measured_ms": round(measured, 3),
                      "delta_ms": round(delta, 3), "tolerance_ms": rtt_tol,
                      "samples": len(tcp_ms)}
    print(f"RTT: expected ~{expected:.1f}ms  measured {measured:.2f}ms  "
          f"(delta {delta:.2f}ms, tolerance ±{rtt_tol}ms, n={len(tcp_ms)})")
    if delta > rtt_tol:
        failures.append(f"RTT off by {delta:.2f}ms (>{rtt_tol}ms): "
                        f"expected ~{expected:.1f}, measured {measured:.2f}")

# ── Throughput: reported Mbps must approach, and never exceed, the link cap ──
thr_doc = load("throughput.json")
mbps = collect(thr_doc, "throughput_mbps")
if not mbps:
    failures.append("no throughput samples parsed from throughput.json")
else:
    measured = statistics.median(mbps)
    lo = rate_mbit * (1 - thr_tol_pct / 100.0)
    hi = rate_mbit * 1.05  # a measurement ABOVE the imposed cap means the
                           # shaping was bypassed or the math is wrong
    results["throughput"] = {"cap_mbit": rate_mbit, "measured_mbps": round(measured, 3),
                             "band": [round(lo, 2), round(hi, 2)],
                             "samples": len(mbps)}
    print(f"Throughput: cap {rate_mbit:.0f}Mbit  measured {measured:.2f}Mbps  "
          f"(band {lo:.1f}–{hi:.1f}, n={len(mbps)})")
    if not (lo <= measured <= hi):
        failures.append(f"throughput {measured:.2f}Mbps outside {lo:.1f}–{hi:.1f} "
                        f"for a {rate_mbit:.0f}Mbit link")

record = {
    "recorded_at": datetime.datetime.now(datetime.UTC).isoformat(),
    "imposed": {"delay_ms": delay_ms, "rate_mbit": rate_mbit},
    "results": results,
    "verdict": "fail" if failures else "pass",
}
pathlib.Path(baseline_path).write_text(json.dumps(record, indent=2) + "\n")
print(f"\nbaseline written: {baseline_path}")

if failures:
    print("\nMEASUREMENT ACCURACY FAILURES:")
    for f in failures:
        print(f"  - {f}")
    sys.exit(1)
print("\nAll measurements within tolerance of the known-truth link.")
PY
}

main "$@"
