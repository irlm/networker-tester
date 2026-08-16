#!/usr/bin/env bash
# ─── Lab validation — the local twin of scripts/soak-canary.sh ────────────────
# Drives REAL runs through the control plane's public API against the docker
# runners/targets `lab.sh up` started, and asserts what health checks can't see:
#
#   phase 1  network probe (kind=network → target-1 rust endpoint :8443, insecure)
#            → run reaches `completed`, attempts persisted, successes > 0
#   phase 2  proxy mode matrix — for EVERY proxy target (nginx/caddy/…): the
#            deterministic HTTP/TCP/UDP modes through the stack install.sh set
#            up (kind=proxy → dispatcher resolves ip:stack-port + insecure);
#            every mode must have ≥1 successful attempt; `native` must be DROPPED
#            by dispatch (0 attempts), never failed.
#   phase 3  fan-out — launch 2×runners runs at once; all complete and ≥2
#            distinct workers execute them (dispatch spreads across agents).
#   phase 4  cancel — launch a long run, cancel it, assert terminal `cancelled`.
#
# Exit non-zero on the first failed assertion, printing the run/attempt detail
# needed to debug it (and `lab.sh logs runner-N` for the rest).
#
# Flags:
#   --modes a,b,c     phase-2 matrix override (default below)
#   --runs N          runs per mode (default 2)
#   --timeout SECS    per-run wall-clock budget (default 300)
#   --skip PHASES     comma list of phase numbers to skip (e.g. --skip 3,4)
#   --only PHASES     comma list of phase numbers to run
set -uo pipefail

LAB_DIR="$(cd "$(dirname "$0")" && pwd)"
STATE_ENV="$LAB_DIR/.state/lab.env"
[ -f "$STATE_ENV" ] || { echo "no lab state — run ./lab/lab.sh up first" >&2; exit 2; }
# shellcheck disable=SC1090
. "$STATE_ENV"
BASE="${LAB_BASE_URL:-http://127.0.0.1:5030}"
PID="${LAB_PROJECT_ID:?}"
TOKEN="${LAB_TOKEN:?}"
NET_PREFIX="${LAB_NET_PREFIX:-172.31.100}"
RUNNERS="${LAB_RUNNERS:-1}"
TARGETS="${LAB_TARGETS:-rust}"

RUNS=2
RUN_TIMEOUT=300
POLL=3
SKIP=""; ONLY=""
# Deterministic + proxy-reachable modes. UDP modes (udp/stamp) go straight to
# the endpoint's UDP ports on the same container, like the opened cloud ports.
MATRIX_DEFAULT="tcp,dns,tls,tlsresume,http1,http2,http3,curl,download,upload,pageload,pageload2,pageload3,websocket,udp,stamp"
MATRIX="$MATRIX_DEFAULT"
NETWORK_MODES="tcp,dns,tls,tlsresume,http1,http2,http3,curl,ping"
while [ $# -gt 0 ]; do
  case "$1" in
    --modes) shift; MATRIX="$1" ;;
    --runs) shift; RUNS="$1" ;;
    --timeout) shift; RUN_TIMEOUT="$1" ;;
    --skip) shift; SKIP=",$1," ;;
    --only) shift; ONLY=",$1," ;;
    -h|--help) sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac; shift
done

if [ -t 1 ]; then G=$'\033[32m'; R=$'\033[31m'; Y=$'\033[33m'; C=$'\033[36m'; D=$'\033[2m'; N=$'\033[0m'; else G=""; R=""; Y=""; C=""; D=""; N=""; fi
note() { printf '%s▸%s %s\n' "$C" "$N" "$*"; }
pass() { printf '%s  ✓ %s%s\n' "$G" "$*" "$N"; }
FAILS=0
fail() { printf '%s  ✗ FAIL: %s%s\n' "$R" "$*" "$N" >&2; FAILS=$((FAILS + 1)); }
die()  { fail "$@"; summary; exit 1; }
run_phase() { # run_phase N
  case ",$SKIP," in *",$1,"*) return 1;; esac
  if [ -n "$ONLY" ]; then case "$ONLY" in *",$1,"*) return 0;; *) return 1;; esac; fi
  return 0
}

api() { # api METHOD PATH [json]
  local m="$1" p="$2" b="${3:-}"
  if [ -n "$b" ]; then curl -sS --max-time 60 -X "$m" -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d "$b" "$BASE$p"
  else curl -sS --max-time 60 -X "$m" -H "Authorization: Bearer $TOKEN" "$BASE$p"; fi
}
# Token may have expired since `up` — re-login transparently.
if ! api GET /api/projects | grep -q project_id; then
  TOKEN="$(curl -sS "$BASE/api/auth/login" -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg e "${LAB_ADMIN_EMAIL:-admin@lab.local}" --arg p "${LAB_ADMIN_PASSWORD:-LabAdmin-Pass1!}" '{email:$e,password:$p}')" | jq -r '.token // empty')"
  [ -n "$TOKEN" ] || { echo "login failed" >&2; exit 2; }
fi

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
STACKS_JSON="$LAB_DIR/../shared/http-stacks.json"
stack_h3() { # stack_h3 STACK → 0 (true) / 1 (false); unknown → assume true
  # (not `// true`: jq's // treats false as missing)
  local v; v="$(jq -r --arg s "$1" '([.stacks[]|select(.id==$s)][0].h3) | if . == null then true else . end' "$STACKS_JSON" 2>/dev/null || echo true)"
  [ "$v" = "true" ]
}
# HTTP/3 modes only make sense on stacks the installer configures with QUIC
# (shared/http-stacks.json h3=true); on the others they are dropped from the
# expected matrix rather than counted as regressions.
H3_MODES="http3,pageload3,browser3"
strip_h3_modes() { echo "$1" | tr ',' '\n' | grep -vxF -e http3 -e pageload3 -e browser3 | paste -sd, -; }
stack_of() { echo "$TARGETS" | tr ',' '\n' | sed -n "${1}p"; }
target_count() { echo "$TARGETS" | tr ',' '\n' | grep -c .; }
target_ip() { echo "${NET_PREFIX}.$((100 + $1))"; }
list_of() { jq -c 'if type=="array" then . else (.attempts // .items // .data // .configs // .runs // .test_runs // []) end' <<<"$1"; }

create_config() { # create_config NAME ENDPOINT_JSON WORKLOAD_JSON → id
  local resp id
  resp="$(api POST "/api/v2/projects/$PID/test-configs" "$(jq -nc --arg n "$1" --argjson e "$2" --argjson w "$3" '{name:$n,endpoint:$e,workload:$w}')")"
  id="$(jq -r '.id // empty' <<<"$resp")"
  [ -n "$id" ] || die "config create failed ($1): $(head -c 300 <<<"$resp")"
  echo "$id"
}
launch() { # launch CFG_ID → run id
  local resp id
  resp="$(api POST "/api/v2/test-configs/$1/launch" '{}')"
  id="$(jq -r '.run_id // .id // empty' <<<"$resp")"
  [ -n "$id" ] || die "launch failed for config $1: $(head -c 300 <<<"$resp")"
  echo "$id"
}
wait_run() { # wait_run RUN_ID [timeout] → prints final status; sets R_OK/R_FAIL/R_STATUS
  local id="$1" t="${2:-$RUN_TIMEOUT}" deadline r
  deadline=$((SECONDS + t))
  while :; do
    r="$(api GET "/api/v2/test-runs/$id")"
    R_STATUS="$(jq -r '.status // empty' <<<"$r")"
    R_OK="$(jq -r '.success_count // 0' <<<"$r")"
    R_FAIL="$(jq -r '.failure_count // 0' <<<"$r")"
    R_WORKER="$(jq -r '.worker_id // empty' <<<"$r")"
    R_ERR="$(jq -r '.error_message // empty' <<<"$r")"
    case "$R_STATUS" in completed|failed|partial|cancelled|error) return 0;; esac
    if [ "$SECONDS" -ge "$deadline" ]; then R_STATUS="timeout(${R_STATUS:-?})"; return 1; fi
    sleep "$POLL"
  done
}
attempts_of() { list_of "$(api GET "/api/v2/test-runs/$1/attempts?limit=1000")"; }
per_mode_stats() { # per_mode_stats ATTEMPTS_JSON → "mode ok/n · …" and sets BROKEN
  jq -r 'group_by(.protocol) | map({m:.[0].protocol, ok:(map(select(.success==true))|length), n:length}) | map("\(.m) \(.ok)/\(.n)") | join(" · ")' <<<"$1"
}
broken_modes() { jq -r 'group_by(.protocol) | map({m:.[0].protocol, ok:(map(select(.success==true))|length)}) | [.[]|select(.ok==0)|.m] | join(",")' <<<"$1"; }
first_errors() { jq -r '[.[]|select(.success==false)|"\(.protocol): \(.error_message // "?")"] | unique | .[0:6][]' <<<"$1"; }

RESULTS=""
summary() {
  echo
  printf '%s══ lab validation summary (%s) ══%s\n' "$C" "$STAMP" "$N"
  printf '%b' "$RESULTS"
  if [ "$FAILS" -eq 0 ]; then printf '%s  ALL PHASES PASSED%s\n' "$G" "$N"; else printf '%s  %d FAILURE(S)%s\n' "$R" "$FAILS" "$N"; fi
}
record() { RESULTS="${RESULTS}  $*\n"; }

# ── preflight ────────────────────────────────────────────────────────────────
note "lab: $BASE  project $PID  runners=$RUNNERS targets=$TARGETS"
ONLINE="$(api GET "/api/projects/$PID/agents" | jq -r '(.agents // .) | [.[]|select(.status=="online")] | length')"
[ "${ONLINE:-0}" -ge 1 ] || die "no online agent in project $PID — run ./lab/lab.sh status"
pass "$ONLINE agent(s) online"

# ── phase 1: network probe against the direct rust endpoint ──────────────────
if run_phase 1; then
  note "phase 1 — network probe (kind=network) against target-1 ($(stack_of 1)) at $(target_ip 1):8443"
  CFG1="$(create_config "lab-p1-network-$STAMP" \
    "$(jq -nc --arg h "$(target_ip 1)" '{kind:"network",host:$h,port:8443}')" \
    "$(jq -nc --arg m "$NETWORK_MODES" --argjson r "$RUNS" '{modes:($m|split(",")),runs:$r,concurrency:1,timeout_ms:8000,insecure:true}')")" || exit 1
  RUN1="$(launch "$CFG1")" || exit 1
  note "  run $RUN1 launched — waiting"
  wait_run "$RUN1" || fail "phase 1: run $RUN1 did not reach a terminal state in ${RUN_TIMEOUT}s (last=$R_STATUS)"
  ATT1="$(attempts_of "$RUN1")"
  N1="$(jq length <<<"$ATT1")"
  note "  status=$R_STATUS ok=$R_OK fail=$R_FAIL attempts=$N1 worker=${R_WORKER:-?}"
  echo "    $(per_mode_stats "$ATT1")"
  [ "$R_STATUS" = "completed" ] || fail "phase 1: status '$R_STATUS' (expected completed) ${R_ERR:+— $R_ERR}"
  [ "$N1" -gt 0 ] || fail "phase 1: 0 attempts persisted (attempt persistence broken?)"
  [ "${R_OK:-0}" -gt 0 ] || fail "phase 1: 0 successful attempts"
  B1="$(broken_modes "$ATT1")"
  if [ -n "$B1" ]; then fail "phase 1: mode(s) with zero successes: $B1"; first_errors "$ATT1" | sed 's/^/      /'; fi
  record "phase 1  network probe → $R_STATUS ok=$R_OK fail=$R_FAIL ($(per_mode_stats "$ATT1"))"
  [ "$FAILS" -eq 0 ] && pass "phase 1"
fi

# ── phase 2: proxy mode matrix through every proxy target ────────────────────
if run_phase 2; then
  i=1; total="$(target_count)"; any_proxy=0
  while [ "$i" -le "$total" ]; do
    st="$(stack_of "$i")"
    if [ "$st" != "rust" ]; then
      any_proxy=1
      DEP="$(api GET "/api/projects/$PID/deployments?limit=100" | jq -r --arg n "lab-target-${i}-${st}" \
        '(if type=="array" then . else (.deployments // .items // []) end) | [.[]|select(.name==$n)][0] | (.id // .deployment_id) // empty')"
      [ -n "$DEP" ] || { fail "phase 2: deployment for target-$i ($st) not found (lab.sh up registers it)"; i=$((i+1)); continue; }
      note "phase 2 — mode matrix through target-$i ($st) deployment ${DEP:0:8} at $(target_ip "$i")"
      STACK_MATRIX="$MATRIX"
      if ! stack_h3 "$st"; then STACK_MATRIX="$(strip_h3_modes "$MATRIX")"; note "  ($st has no HTTP/3 per shared/http-stacks.json — h3 modes excluded)"; fi
      MODES_JSON="$(jq -nc --arg m "$STACK_MATRIX" '($m|split(",")) + ["native"]')"
      CFG2="$(create_config "lab-p2-${st}-t${i}-$STAMP" \
        "$(jq -nc --arg d "$DEP" --arg s "$st" '{kind:"proxy",proxy_endpoint_id:$d,proxy_stack:$s}')" \
        "$(jq -nc --argjson modes "$MODES_JSON" --argjson r "$RUNS" '{modes:$modes,runs:$r,concurrency:1,timeout_ms:15000,capture_mode:"headers-only",payload_sizes:[]}')")" || exit 1
      RUN2="$(launch "$CFG2")" || exit 1
      note "  run $RUN2 launched — waiting (matrix: $STACK_MATRIX + native)"
      wait_run "$RUN2" || fail "phase 2 ($st): run $RUN2 did not finish in ${RUN_TIMEOUT}s (last=$R_STATUS)"
      ATT2="$(attempts_of "$RUN2")"
      note "  status=$R_STATUS ok=$R_OK fail=$R_FAIL attempts=$(jq length <<<"$ATT2") worker=${R_WORKER:-?}"
      echo "    $(per_mode_stats "$ATT2")"
      case "$R_STATUS" in completed|partial) ;; *) fail "phase 2 ($st): status '$R_STATUS' ${R_ERR:+— $R_ERR}";; esac
      B2="$(broken_modes "$ATT2")"
      if [ -n "$B2" ]; then fail "phase 2 ($st): mode(s) with ZERO successes through $st: $B2"; first_errors "$ATT2" | sed 's/^/      /'; fi
      NATIVE="$(jq '[.[]|select(.protocol=="native")]|length' <<<"$ATT2")"
      [ "$NATIVE" -eq 0 ] || fail "phase 2 ($st): 'native' produced $NATIVE attempt(s) — dispatch must DROP it (v0.28.120 filter)"
      NOTFOUND="$(jq '[.[]|select(.success==false and ((.error_message // "")|test("404")))]|length' <<<"$ATT2")"
      [ "$NOTFOUND" -eq 0 ] || fail "phase 2 ($st): $NOTFOUND attempt(s) got HTTP 404 through $st — the proxy is not forwarding a route (v0.28.112 class)"
      record "phase 2  $st matrix → $R_STATUS ok=$R_OK fail=$R_FAIL${B2:+ BROKEN=$B2}"
      [ -z "$B2" ] && [ "$NATIVE" -eq 0 ] && [ "$NOTFOUND" -eq 0 ] && pass "phase 2 ($st)"
    fi
    i=$((i + 1))
  done
  [ "$any_proxy" = 1 ] || { note "phase 2 — no proxy targets (all rust) — skipped; use: lab.sh up --targets rust,nginx"; record "phase 2  (skipped: no proxy targets)"; }
fi

# ── phase 3: fan-out across runners ──────────────────────────────────────────
if run_phase 3; then
  K=$((RUNNERS * 2)); [ "$K" -lt 2 ] && K=2
  note "phase 3 — fan-out: launching $K runs at once (runners=$RUNNERS)"
  CFG3="$(create_config "lab-p3-fanout-$STAMP" \
    "$(jq -nc --arg h "$(target_ip 1)" '{kind:"network",host:$h,port:8443}')" \
    "$(jq -nc '{modes:["tcp","http1"],runs:3,concurrency:1,timeout_ms:5000,insecure:true}')")" || exit 1
  RUNS3=""
  k=0; while [ "$k" -lt "$K" ]; do r="$(launch "$CFG3")" || exit 1; RUNS3="$RUNS3 $r"; k=$((k + 1)); done
  WORKERS=""; DONE=0
  for rid in $RUNS3; do
    wait_run "$rid" || fail "phase 3: run $rid did not finish (last=$R_STATUS)"
    [ "$R_STATUS" = "completed" ] && DONE=$((DONE + 1)) || fail "phase 3: run $rid ended '$R_STATUS'"
    WORKERS="$WORKERS $R_WORKER"
  done
  DISTINCT="$(echo "$WORKERS" | tr ' ' '\n' | grep . | sort -u | wc -l | tr -d ' ')"
  note "  $DONE/$K completed; distinct workers: $DISTINCT"
  if [ "$RUNNERS" -ge 2 ] && [ "$DISTINCT" -lt 2 ]; then fail "phase 3: $RUNNERS runners online but all runs went to $DISTINCT worker — dispatch is not spreading"; fi
  record "phase 3  fan-out → $DONE/$K completed on $DISTINCT worker(s)"
  [ "$DONE" -eq "$K" ] && pass "phase 3"
fi

# ── phase 4: cancel ──────────────────────────────────────────────────────────
if run_phase 4; then
  note "phase 4 — cancel a long-running run"
  CFG4="$(create_config "lab-p4-cancel-$STAMP" \
    "$(jq -nc --arg h "$(target_ip 1)" '{kind:"network",host:$h,port:8443}')" \
    "$(jq -nc '{modes:["http1"],runs:20000,concurrency:1,timeout_ms:5000,insecure:true}')")" || exit 1
  RUN4="$(launch "$CFG4")" || exit 1
  # Give the agent a moment to actually start it (20k sequential http1 probes
  # take minutes — plenty of runway), then cancel.
  sleep 5
  CRESP="$(api POST "/api/v2/test-runs/$RUN4/cancel" '{}')"
  wait_run "$RUN4" 90 || fail "phase 4: run $RUN4 still not terminal 90s after cancel (last=$R_STATUS) — $CRESP"
  note "  status after cancel: $R_STATUS"
  [ "$R_STATUS" = "cancelled" ] || fail "phase 4: expected 'cancelled', got '$R_STATUS' (cancel response: $(head -c 200 <<<"$CRESP"))"
  record "phase 4  cancel → $R_STATUS"
  [ "$R_STATUS" = "cancelled" ] && pass "phase 4"
fi

summary
[ "$FAILS" -eq 0 ]
