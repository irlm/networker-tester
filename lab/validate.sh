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
#            by dispatch (0 attempts), never failed. A `windows` target is
#            validated as proxy stack `iis` (:8445 — the deployment carries
#            endpoint_hosts=[target-N.lab], so dispatch hands the tester the
#            hostname, the QUIC ClientHello carries SNI and IIS's SNI binding
#            serves HTTP/3: http3/pageload3 are in the iis matrix) AND,
#            mirroring phase 1, its bare networker-endpoint on :8443
#            (kind=network, incl. http3).
#   phase 3  fan-out — launch 2×runners runs at once; all complete and ≥2
#            distinct workers execute them (dispatch spreads across agents).
#   phase 4  cancel — launch a long run, cancel it, assert terminal `cancelled`.
#   phase 5  provider — the Docker (local) cloud provider (skipped with a note
#            when GET /api/version says docker_provider=false): POST a tester
#            with cloud "docker" → the control plane `docker run`s a runner and
#            the agent comes online (power_state running / allocation idle);
#            POST a deployment with one docker endpoint behind nginx → completed
#            with an endpoint_ip; a proxy-kind config against that deployment
#            pinned to that tester runs with successes; DELETE both → the
#            containers are gone (docker ps -a --filter label=networker.role).
#   phase 6  windows runner — for every online agent whose os is windows
#            (lab.sh up --windows-runners M: a Windows Server VM running the
#            checkout's agent + the released tester, bound to a project_tester
#            row so LaunchRequest.tester_id pins to it): its heartbeat carries
#            os=windows + capabilities {chrome:false,tshark:false}; the phase-1
#            network modes (incl. ping via IcmpSendEcho, http3) and the phase-2
#            proxy matrix through the first Linux proxy target (nginx) run
#            PINNED to it, execute on that agent (worker_id) and every mode
#            has ≥1 success — "a Windows runner runs the same tests as a Linux
#            runner"; modes that differ from the Linux runs are listed.
#            Skipped with a note when the lab has no Windows runner.
#
# Exit non-zero on the first failed assertion, printing the run/attempt detail
# needed to debug it (and `lab.sh logs runner-N` for the rest).
#
# Flags:
#   --modes a,b,c     phase-2 matrix override (default below)
#   --runs N          runs per mode (default 2)
#   --timeout SECS    per-run wall-clock budget (default 300)
#   --skip PHASES     comma list of phase numbers to skip (e.g. --skip 3,4,5,6)
#   --only PHASES     comma list of phase numbers to run
#
# Env overrides (all optional — set by lab.sh or by other lab front-ends such
# as lab/native/lab-native.ps1, which drives this same matrix against native
# Windows processes through Git Bash):
#   LAB_STATE_ENV      path of the lab.env to source (default lab/.state/lab.env,
#                      or lab/.state-N/lab.env for LAB_INSTANCE=N)
#   LAB_TARGET_HOSTS   comma list: host of target-1, target-2, … (overrides the
#                      NET_PREFIX+index addressing; LAB_TARGET_IPS is an alias)
#   LAB_H3_OFF_STACKS  comma list of stacks whose HTTP/3 modes must be EXCLUDED
#                      on this host even though shared/http-stacks.json says
#                      h3=true (e.g. IIS reached by IP literal — http.sys binds
#                      QUIC on SNI hostname bindings only; or no reboot yet)
set -uo pipefail

LAB_DIR="$(cd "$(dirname "$0")" && pwd)"
LAB_INSTANCE="${LAB_INSTANCE:-1}"
STATE_SUFFIX=""; [ "$LAB_INSTANCE" != 1 ] && STATE_SUFFIX="-${LAB_INSTANCE}"
STATE_ENV="${LAB_STATE_ENV:-$LAB_DIR/.state${STATE_SUFFIX}/lab.env}"
[ -f "$STATE_ENV" ] || { echo "no lab state — run ./lab/lab.sh up first (or set LAB_STATE_ENV)" >&2; exit 2; }
# shellcheck disable=SC1090
. "$STATE_ENV"
BASE="${LAB_BASE_URL:-http://127.0.0.1:5030}"
PID="${LAB_PROJECT_ID:?}"
TOKEN="${LAB_TOKEN:?}"
NET_PREFIX="${LAB_NET_PREFIX:-172.31.100}"
RUNNERS="${LAB_RUNNERS:-1}"
TARGETS="${LAB_TARGETS:-rust}"
TARGET_HOSTS="${LAB_TARGET_HOSTS:-${LAB_TARGET_IPS:-}}"
H3_OFF_STACKS="${LAB_H3_OFF_STACKS:-}"

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
    -h|--help) sed -n '2,39p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
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
  case ",$H3_OFF_STACKS," in *",$1,"*) return 1;; esac   # host-level override (LAB_H3_OFF_STACKS)
  local v; v="$(jq -r --arg s "$1" '([.stacks[]|select(.id==$s)][0].h3) | if . == null then true else . end' "$STACKS_JSON" 2>/dev/null || echo true)"
  [ "$v" = "true" ]
}
# HTTP/3 modes only make sense on stacks the installer configures with QUIC
# (shared/http-stacks.json h3=true); on the others they are dropped from the
# expected matrix rather than counted as regressions.
H3_MODES="http3,pageload3,browser3"
strip_h3_modes() { echo "$1" | tr ',' '\n' | grep -vxF -e http3 -e pageload3 -e browser3 | paste -sd, -; }
stack_of() { echo "$TARGETS" | tr ',' '\n' | sed -n "${1}p"; }
# lab.sh registers the windows target as its proxy stack (iis); the rest are 1:1.
proxy_stack_of() { case "$1" in windows) echo iis;; *) echo "$1";; esac; }
target_count() { echo "$TARGETS" | tr ',' '\n' | grep -c .; }
target_ip() { # target_ip N → LAB_TARGET_HOSTS[N] when given, else the docker /24 scheme
  local h=""
  [ -n "$TARGET_HOSTS" ] && h="$(echo "$TARGET_HOSTS" | tr ',' '\n' | sed -n "${1}p")"
  if [ -n "$h" ]; then echo "$h"; else echo "${NET_PREFIX}.$((100 + $1))"; fi
}
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
      pst="$(proxy_stack_of "$st")"   # windows → iis
      lbl="$st"; [ "$pst" != "$st" ] && lbl="$st/$pst"
      DEP="$(api GET "/api/projects/$PID/deployments?limit=100" | jq -r --arg n "lab-target-${i}-${pst}" \
        '(if type=="array" then . else (.deployments // .items // []) end) | [.[]|select(.name==$n)][0] | (.id // .deployment_id) // empty')"
      [ -n "$DEP" ] || { fail "phase 2: deployment for target-$i ($lbl) not found (lab.sh up registers it)"; i=$((i+1)); continue; }
      DEP_HOST="$(api GET "/api/projects/$PID/deployments/$DEP" | jq -r '(.endpoint_hosts // [])[0] // empty')"
      note "phase 2 — mode matrix through target-$i ($lbl) deployment ${DEP:0:8} at ${DEP_HOST:-$(target_ip "$i")}:$(jq -r --arg s "$pst" '[.stacks[]|select(.id==$s)][0].https_port // "?"' "$STACKS_JSON")${DEP_HOST:+ (endpoint_hosts → hostname dispatch, TLS SNI)}"
      STACK_MATRIX="$MATRIX"
      if ! stack_h3 "$pst"; then
        STACK_MATRIX="$(strip_h3_modes "$MATRIX")"
        case ",$H3_OFF_STACKS," in
          *",$pst,"*) note "  ($pst: HTTP/3 modes excluded on THIS host by LAB_H3_OFF_STACKS — not a verdict on the stack)";;
          *) note "  ($pst has no HTTP/3 per shared/http-stacks.json — h3 modes excluded)";;
        esac
      fi
      MODES_JSON="$(jq -nc --arg m "$STACK_MATRIX" '($m|split(",")) + ["native"]')"
      CFG2="$(create_config "lab-p2-${pst}-t${i}-$STAMP" \
        "$(jq -nc --arg d "$DEP" --arg s "$pst" '{kind:"proxy",proxy_endpoint_id:$d,proxy_stack:$s}')" \
        "$(jq -nc --argjson modes "$MODES_JSON" --argjson r "$RUNS" '{modes:$modes,runs:$r,concurrency:1,timeout_ms:15000,capture_mode:"headers-only",payload_sizes:[]}')")" || exit 1
      RUN2="$(launch "$CFG2")" || exit 1
      note "  run $RUN2 launched — waiting (matrix: $STACK_MATRIX + native)"
      wait_run "$RUN2" || fail "phase 2 ($lbl): run $RUN2 did not finish in ${RUN_TIMEOUT}s (last=$R_STATUS)"
      ATT2="$(attempts_of "$RUN2")"
      note "  status=$R_STATUS ok=$R_OK fail=$R_FAIL attempts=$(jq length <<<"$ATT2") worker=${R_WORKER:-?}"
      echo "    $(per_mode_stats "$ATT2")"
      # Remembered for phase 6 (Windows runner vs Linux runner, same target).
      if [ -z "${P2_LINUX_ATT:-}" ] && [ "$st" != windows ]; then P2_LINUX_ATT="$ATT2"; P2_LINUX_STACK="$pst"; P2_LINUX_WORKER="$R_WORKER"; fi
      case "$R_STATUS" in completed|partial) ;; *) fail "phase 2 ($lbl): status '$R_STATUS' ${R_ERR:+— $R_ERR}";; esac
      B2="$(broken_modes "$ATT2")"
      if [ -n "$B2" ]; then fail "phase 2 ($lbl): mode(s) with ZERO successes through $pst: $B2"; first_errors "$ATT2" | sed 's/^/      /'; fi
      NATIVE="$(jq '[.[]|select(.protocol=="native")]|length' <<<"$ATT2")"
      [ "$NATIVE" -eq 0 ] || fail "phase 2 ($lbl): 'native' produced $NATIVE attempt(s) — dispatch must DROP it (v0.28.120 filter)"
      NOTFOUND="$(jq '[.[]|select(.success==false and ((.error_message // "")|test("404")))]|length' <<<"$ATT2")"
      [ "$NOTFOUND" -eq 0 ] || fail "phase 2 ($lbl): $NOTFOUND attempt(s) got HTTP 404 through $pst — the proxy is not forwarding a route (v0.28.112 class)"
      record "phase 2  $lbl matrix → $R_STATUS ok=$R_OK fail=$R_FAIL${B2:+ BROKEN=$B2}"
      [ -z "$B2" ] && [ "$NATIVE" -eq 0 ] && [ "$NOTFOUND" -eq 0 ] && pass "phase 2 ($lbl)"

      # Windows target: also the BARE Windows networker-endpoint on :8443
      # (kind=network, like phase 1 against the rust target) — proves the
      # Windows build of the endpoint offers the same tests as the Linux one.
      if [ "$st" = "windows" ]; then
        note "phase 2 — network probe (kind=network) against target-$i (windows networker-endpoint) at $(target_ip "$i"):8443"
        CFG2W="$(create_config "lab-p2-windows-endpoint-t${i}-$STAMP" \
          "$(jq -nc --arg h "$(target_ip "$i")" '{kind:"network",host:$h,port:8443}')" \
          "$(jq -nc --arg m "$NETWORK_MODES" --argjson r "$RUNS" '{modes:($m|split(",")),runs:$r,concurrency:1,timeout_ms:8000,insecure:true}')")" || exit 1
        RUN2W="$(launch "$CFG2W")" || exit 1
        note "  run $RUN2W launched — waiting (modes: $NETWORK_MODES)"
        wait_run "$RUN2W" || fail "phase 2 (windows endpoint): run $RUN2W did not finish in ${RUN_TIMEOUT}s (last=$R_STATUS)"
        ATT2W="$(attempts_of "$RUN2W")"
        note "  status=$R_STATUS ok=$R_OK fail=$R_FAIL attempts=$(jq length <<<"$ATT2W") worker=${R_WORKER:-?}"
        echo "    $(per_mode_stats "$ATT2W")"
        [ "$R_STATUS" = "completed" ] || fail "phase 2 (windows endpoint): status '$R_STATUS' (expected completed) ${R_ERR:+— $R_ERR}"
        [ "${R_OK:-0}" -gt 0 ] || fail "phase 2 (windows endpoint): 0 successful attempts"
        B2W="$(broken_modes "$ATT2W")"
        if [ -n "$B2W" ]; then fail "phase 2 (windows endpoint): mode(s) with zero successes: $B2W"; first_errors "$ATT2W" | sed 's/^/      /'; fi
        record "phase 2  windows endpoint :8443 → $R_STATUS ok=$R_OK fail=$R_FAIL${B2W:+ BROKEN=$B2W}"
        [ -z "$B2W" ] && [ "$R_STATUS" = "completed" ] && pass "phase 2 (windows endpoint)"
      fi
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

# ── phase 5: Docker (local) provider — the managed path end to end ───────────
if run_phase 5; then
  DOCKER_PROVIDER="$(api GET /api/version | jq -r '.docker_provider // false')"
  if [ "$DOCKER_PROVIDER" != "true" ]; then
    note "phase 5 — Docker (local) provider is OFF on this control plane (GET /api/version docker_provider=false) — skipped"
    record "phase 5  (skipped: docker_provider=false — set DASHBOARD_DOCKER_PROVIDER=1 on the control plane)"
  else
    note "phase 5 — Docker (local) provider: create-tester → agent online → docker deployment → proxy run → delete"
    P5_FAILS_BEFORE=$FAILS
    P5_TESTER=""; P5_DEP=""; P5_TESTER_NAME="lab-p5-docker-$STAMP"
    T_STATE=""; T_ALLOC=""; D_STATUS=""; D_IP=""; D_ERR=""; LEFT=""

    # 5a. POST /testers with cloud "docker" (no cloud account) → 202 + row.
    TRESP="$(api POST "/api/projects/$PID/testers" "$(jq -nc --arg n "$P5_TESTER_NAME" '{name:$n,cloud:"docker",region:"local"}')")"
    P5_TESTER="$(jq -r '.tester_id // empty' <<<"$TRESP")"
    if [ -z "$P5_TESTER" ]; then
      fail "phase 5a: create tester (cloud=docker) failed: $(head -c 300 <<<"$TRESP")"
    else
      note "  tester $P5_TESTER created ($(jq -r '"cloud=\(.cloud) region=\(.region) vm_size=\(.vm_size) power_state=\(.power_state)"' <<<"$TRESP")) — waiting for the runner container + agent"
      deadline=$((SECONDS + 240)); T_MSG=""; T_IP=""; T_VM=""
      while :; do
        TROW="$(api GET "/api/projects/$PID/testers/$P5_TESTER")"
        T_STATE="$(jq -r '.power_state // empty' <<<"$TROW")"
        T_ALLOC="$(jq -r '.allocation // empty' <<<"$TROW")"
        T_MSG="$(jq -r '.status_message // empty' <<<"$TROW")"
        T_IP="$(jq -r '.public_ip // empty' <<<"$TROW")"
        T_VM="$(jq -r '.vm_resource_id // .vm_name // empty' <<<"$TROW")"
        [ "$T_STATE" = "running" ] && [ "$T_ALLOC" = "idle" ] && break
        [ "$T_STATE" = "error" ] && break
        [ "$SECONDS" -ge "$deadline" ] && break
        sleep 3
      done
      note "  tester power_state=$T_STATE allocation=$T_ALLOC public_ip=${T_IP:-?} container=${T_VM:-?} ${T_MSG:+msg=\"$T_MSG\"}"
      if [ "$T_STATE" = "running" ] && [ "$T_ALLOC" = "idle" ]; then
        pass "phase 5a: docker tester provisioned + agent online"
      else
        fail "phase 5a: docker tester did not reach running/idle (power_state=$T_STATE, msg=$T_MSG)"
        docker ps -a --filter "label=networker.tester_id=$P5_TESTER" --format '    {{.Names}}  {{.Status}}' 2>/dev/null || true
      fi
      # The agent row behind the tester must be online (that is what the wait gated on).
      AGENT_ONLINE="$(api GET "/api/projects/$PID/agents" | jq -r --arg t "$P5_TESTER" '(.agents // .) | [.[]|select(.tester_id==$t and .status=="online")] | length')"
      [ "${AGENT_ONLINE:-0}" -ge 1 ] || fail "phase 5a: no online agent linked to tester $P5_TESTER"
      # The container is a runner labelled with the tester id.
      C_RUN="$(docker ps --filter "label=networker.role=runner" --filter "label=networker.tester_id=$P5_TESTER" --format '{{.Names}}' 2>/dev/null | head -1)"
      if [ -n "$C_RUN" ]; then pass "phase 5a: runner container $C_RUN (label networker.tester_id) is running"; else fail "phase 5a: no running container labelled networker.tester_id=$P5_TESTER"; fi
    fi

    # 5b. POST /deployments with ONE docker endpoint behind nginx → completed + endpoint_ips.
    DRESP="$(api POST "/api/projects/$PID/deployments" "$(jq -nc --arg n "lab-p5-docker-nginx-$STAMP" \
      '{name:$n,config:{version:1,tester:{provider:"local"},endpoints:[{provider:"docker",label:"p5-nginx",http_stacks:["nginx"],docker:{os:"linux"}}],tests:{run_tests:false}}}')")"
    P5_DEP="$(jq -r '.deployment_id // empty' <<<"$DRESP")"
    if [ -z "$P5_DEP" ]; then
      fail "phase 5b: create docker deployment failed: $(head -c 300 <<<"$DRESP")"
    else
      note "  deployment $P5_DEP created — waiting for the target container (nginx) to become healthy"
      deadline=$((SECONDS + 240)); DROW="{}"
      while :; do
        DROW="$(api GET "/api/projects/$PID/deployments/$P5_DEP")"
        D_STATUS="$(jq -r '.status // empty' <<<"$DROW")"
        D_IP="$(jq -r '(.endpoint_ips // [])[0] // empty' <<<"$DROW")"
        D_ERR="$(jq -r '.error_message // empty' <<<"$DROW")"
        case "$D_STATUS" in completed|failed|cancelled) break;; esac
        [ "$SECONDS" -ge "$deadline" ] && break
        sleep 3
      done
      note "  deployment status=$D_STATUS endpoint_ip=${D_IP:-?} ${D_ERR:+error=\"$D_ERR\"}"
      if [ "$D_STATUS" = "completed" ] && [ -n "$D_IP" ]; then
        pass "phase 5b: docker deployment completed with endpoint_ip $D_IP"
      else
        fail "phase 5b: deployment ended '$D_STATUS' (expected completed with an endpoint_ip) ${D_ERR:+— $D_ERR}"
        jq -r '.log // ""' <<<"$DROW" | tail -15 | sed 's/^/      /'
      fi
      C_TGT="$(docker ps --filter "label=networker.role=target" --filter "label=networker.deployment_id=$P5_DEP" --format '{{.Names}}' 2>/dev/null | head -1)"
      if [ -n "$C_TGT" ]; then pass "phase 5b: target container $C_TGT (label networker.deployment_id) is running"; else fail "phase 5b: no running container labelled networker.deployment_id=$P5_DEP"; fi
    fi

    # 5c. proxy-kind config against that deployment, pinned to that tester → run completes with successes.
    if [ -n "$P5_TESTER" ] && [ "$D_STATUS" = "completed" ] && [ -n "$D_IP" ]; then
      # Dispatch's version gate only admits agents that have reported a version
      # (first heartbeat — immediate since v0.28.208, one interval before);
      # wait for it so the tester pin below is a fair assertion.
      deadline=$((SECONDS + 90)); P5_AGENT_VER=""
      while :; do
        P5_AGENT_VER="$(api GET "/api/projects/$PID/agents" | jq -r --arg t "$P5_TESTER" '(.agents // .) | [.[]|select(.tester_id==$t)][0].version // empty')"
        [ -n "$P5_AGENT_VER" ] && break
        [ "$SECONDS" -ge "$deadline" ] && break
        sleep 2
      done
      [ -n "$P5_AGENT_VER" ] && note "  docker runner's agent reports version $P5_AGENT_VER" || fail "phase 5c: docker runner's agent never reported a version (dispatch would skip it)"
      CFG5="$(create_config "lab-p5-proxy-$STAMP" \
        "$(jq -nc --arg d "$P5_DEP" '{kind:"proxy",proxy_endpoint_id:$d,proxy_stack:"nginx"}')" \
        "$(jq -nc --argjson r "$RUNS" '{modes:["tcp","tls","http1","http2","download"],runs:$r,concurrency:1,timeout_ms:15000,capture_mode:"headers-only",payload_sizes:[]}')")" || exit 1
      LRESP="$(api POST "/api/v2/test-configs/$CFG5/launch" "$(jq -nc --arg t "$P5_TESTER" '{tester_id:$t}')")"
      RUN5="$(jq -r '.run_id // .id // empty' <<<"$LRESP")"
      if [ -z "$RUN5" ]; then
        fail "phase 5c: launch pinned to tester $P5_TESTER failed: $(head -c 300 <<<"$LRESP")"
      else
        note "  run $RUN5 launched on tester $P5_TESTER (proxy nginx @ $D_IP) — waiting"
        wait_run "$RUN5" || fail "phase 5c: run $RUN5 did not finish in ${RUN_TIMEOUT}s (last=$R_STATUS)"
        ATT5="$(attempts_of "$RUN5")"
        note "  status=$R_STATUS ok=$R_OK fail=$R_FAIL attempts=$(jq length <<<"$ATT5") worker=${R_WORKER:-?}"
        echo "    $(per_mode_stats "$ATT5")"
        case "$R_STATUS" in completed|partial) ;; *) fail "phase 5c: status '$R_STATUS' ${R_ERR:+— $R_ERR}";; esac
        [ "${R_OK:-0}" -gt 0 ] || fail "phase 5c: 0 successful attempts through the docker-provisioned nginx target"
        B5="$(broken_modes "$ATT5")"
        if [ -n "$B5" ]; then fail "phase 5c: mode(s) with zero successes: $B5"; first_errors "$ATT5" | sed 's/^/      /'; fi
        # The run must have executed on the docker runner's agent (tester pin).
        P5_AGENT="$(api GET "/api/projects/$PID/agents" | jq -r --arg t "$P5_TESTER" '(.agents // .) | [.[]|select(.tester_id==$t)][0].agent_id // empty')"
        if [ -n "$P5_AGENT" ] && [ -n "$R_WORKER" ] && [ "$R_WORKER" != "$P5_AGENT" ]; then
          fail "phase 5c: run executed on worker $R_WORKER, not the pinned docker tester's agent $P5_AGENT"
        fi
        [ "$FAILS" -eq "$P5_FAILS_BEFORE" ] && pass "phase 5c: proxy run through the docker target on the docker runner"
      fi
    else
      note "  phase 5c skipped (tester or deployment not ready)"
    fi

    # 5d. DELETE both → containers gone.
    if [ -n "$P5_TESTER" ]; then
      DEL="$(api DELETE "/api/projects/$PID/testers/$P5_TESTER")"
      note "  DELETE tester → $(head -c 120 <<<"$DEL")"
    fi
    if [ -n "$P5_DEP" ]; then
      DEL="$(api DELETE "/api/projects/$PID/deployments/$P5_DEP")"
      note "  DELETE deployment → $(head -c 120 <<<"$DEL")"
    fi
    leftovers() {
      { [ -n "$P5_TESTER" ] && docker ps -a --filter "label=networker.role" --filter "label=networker.tester_id=$P5_TESTER" --format '{{.Names}}'
        [ -n "$P5_DEP" ] && docker ps -a --filter "label=networker.role" --filter "label=networker.deployment_id=$P5_DEP" --format '{{.Names}}'
      } 2>/dev/null | grep . || true
    }
    deadline=$((SECONDS + 120))
    while :; do
      LEFT="$(leftovers)"
      [ -z "$LEFT" ] && break
      [ "$SECONDS" -ge "$deadline" ] && break
      sleep 3
    done
    if [ -z "$LEFT" ]; then
      pass "phase 5d: containers removed (docker ps -a --filter label=networker.role shows none for tester/deployment)"
    else
      fail "phase 5d: containers still present after delete: $(echo "$LEFT" | tr '\n' ' ')"
    fi
    if [ -n "$P5_TESTER" ]; then
      deadline=$((SECONDS + 60)); TGONE="x"
      while :; do
        TGONE="$(api GET "/api/projects/$PID/testers/$P5_TESTER" | jq -r '.tester_id // empty')"
        [ -z "$TGONE" ] && break
        [ "$SECONDS" -ge "$deadline" ] && break
        sleep 3
      done
      if [ -z "$TGONE" ]; then pass "phase 5d: tester row removed"; else fail "phase 5d: tester row $P5_TESTER still present (power_state=$(api GET "/api/projects/$PID/testers/$P5_TESTER" | jq -r '.power_state'))"; fi
    fi
    record "phase 5  docker provider → tester=${T_STATE:-?}/${T_ALLOC:-?} deployment=${D_STATUS:-?}@${D_IP:-?} run=${R_STATUS:-?} ok=${R_OK:-0} cleanup=$([ -z "$LEFT" ] && echo clean || echo LEFTOVERS)"
    [ "$FAILS" -eq "$P5_FAILS_BEFORE" ] && pass "phase 5"
  fi
fi

# ── phase 6: Windows runner — the same tests, executed on Windows ────────────
if run_phase 6; then
  # Windows runners: agents whose heartbeat says os=windows (v0.28.211+) or that
  # lab.sh tagged so at registration, online, and bound to a project_tester
  # (agent.tester_id) — the only handle the launch API pins with.
  WIN_AGENTS="$(api GET "/api/projects/$PID/agents" | jq -c '(.agents // .) | [.[]|select(.status=="online" and ((.os // "")=="windows" or (.tags.os // "")=="windows"))]')"
  WIN_N="$(jq length <<<"$WIN_AGENTS")"
  if [ "${WIN_N:-0}" -eq 0 ]; then
    note "phase 6 — no online Windows runner in this lab (lab.sh up --windows-runners 1) — skipped"
    record "phase 6  (skipped: no windows runner)"
  else
    note "phase 6 — windows runner: $WIN_N online Windows agent(s): $(jq -r '[.[]|"\(.name)@\(.version // "?")"]|join(", ")' <<<"$WIN_AGENTS")"
    P6_FAILS_BEFORE=$FAILS
    # Modes whose verdict differs between the Linux and the Windows runner
    # (same target) — reported per runner; a mode broken ONLY on Windows fails.
    diff_modes() { # diff_modes LINUX_ATT WINDOWS_ATT → "mode: linux ok/n vs windows ok/n" per differing mode
      jq -rn --argjson l "$1" --argjson w "$2" '
        def stats: group_by(.protocol) | map({key:.[0].protocol, value:{ok:(map(select(.success==true))|length), n:length}}) | from_entries;
        ($l|stats) as $L | ($w|stats) as $W
        | [(($L|keys) + ($W|keys)) | unique[] | . as $m
           | {m:$m, l:($L[$m] // {ok:-1,n:0}), w:($W[$m] // {ok:-1,n:0})}
           | select((.l.ok > 0) != (.w.ok > 0))
           | "\(.m): linux \(if .l.ok < 0 then "-" else "\(.l.ok)/\(.l.n)" end) vs windows \(if .w.ok < 0 then "-" else "\(.w.ok)/\(.w.n)" end)"] | .[]'
    }
    w=0
    while [ "$w" -lt "$WIN_N" ]; do
      WA="$(jq -c ".[$w]" <<<"$WIN_AGENTS")"; w=$((w + 1))
      WNAME="$(jq -r '.name' <<<"$WA")"; WID="$(jq -r '.agent_id' <<<"$WA")"; WTESTER="$(jq -r '.tester_id // empty' <<<"$WA")"
      WOS="$(jq -r '.os // empty' <<<"$WA")"; WARCH="$(jq -r '.arch // empty' <<<"$WA")"; WCAPS="$(jq -c '.capabilities // null' <<<"$WA")"
      note "  $WNAME: agent $WID os=${WOS:-?} arch=${WARCH:-?} capabilities=$WCAPS tester=${WTESTER:-<none>}"
      # 6a. identity: the heartbeat reports os=windows + an inventory without Chrome/tshark
      [ "$WOS" = "windows" ] || fail "phase 6 ($WNAME): agent.os is '${WOS:-null}' — the heartbeat should report os=windows (v0.28.211 agent + control plane)"
      if [ "$WCAPS" = "null" ]; then
        fail "phase 6 ($WNAME): no capabilities reported on the heartbeat"
      else
        [ "$(jq -r '.chrome' <<<"$WCAPS")" = "false" ] || fail "phase 6 ($WNAME): capabilities.chrome=true on a lab Windows runner (no Chrome is installed there — detection wrong?)"
        [ "$(jq -r '.tshark' <<<"$WCAPS")" = "false" ] || fail "phase 6 ($WNAME): capabilities.tshark=true on a lab Windows runner (no Wireshark there — detection wrong?)"
      fi
      if [ -z "$WTESTER" ]; then
        fail "phase 6 ($WNAME): agent has no tester_id — lab.sh registers Windows runners bound to a project_tester row so LaunchRequest.tester_id can pin runs to it"
        continue
      fi
      TROW="$(api GET "/api/projects/$PID/testers/$WTESTER")"
      note "  tester $(jq -r '"\(.name) cloud=\(.cloud) power_state=\(.power_state) allocation=\(.allocation) installer_version=\(.installer_version // "?")"' <<<"$TROW")"

      # 6b. the phase-1 network modes, pinned to the Windows runner (target-1 rust endpoint).
      CFG6N="$(create_config "lab-p6-win-network-${WNAME}-$STAMP" \
        "$(jq -nc --arg h "$(target_ip 1)" '{kind:"network",host:$h,port:8443}')" \
        "$(jq -nc --arg m "$NETWORK_MODES" --argjson r "$RUNS" '{modes:($m|split(",")),runs:$r,concurrency:1,timeout_ms:8000,insecure:true}')")" || exit 1
      LRESP="$(api POST "/api/v2/test-configs/$CFG6N/launch" "$(jq -nc --arg t "$WTESTER" '{tester_id:$t}')")"
      RUN6N="$(jq -r '.run_id // .id // empty' <<<"$LRESP")"
      if [ -z "$RUN6N" ]; then
        fail "phase 6 ($WNAME): launch pinned to tester $WTESTER failed: $(head -c 300 <<<"$LRESP")"
      else
        note "  run $RUN6N (network modes $NETWORK_MODES → target-1 $(stack_of 1)) pinned to $WNAME — waiting"
        wait_run "$RUN6N" || fail "phase 6 ($WNAME): network run $RUN6N did not finish in ${RUN_TIMEOUT}s (last=$R_STATUS)"
        ATT6N="$(attempts_of "$RUN6N")"
        note "  status=$R_STATUS ok=$R_OK fail=$R_FAIL attempts=$(jq length <<<"$ATT6N") worker=${R_WORKER:-?}"
        echo "    $(per_mode_stats "$ATT6N")"
        [ "$R_STATUS" = "completed" ] || fail "phase 6 ($WNAME): network run status '$R_STATUS' (expected completed) ${R_ERR:+— $R_ERR}"
        [ "$R_WORKER" = "$WID" ] || fail "phase 6 ($WNAME): network run executed on worker ${R_WORKER:-?}, not the pinned Windows agent $WID (tester affinity broken?)"
        B6N="$(broken_modes "$ATT6N")"
        if [ -n "$B6N" ]; then fail "phase 6 ($WNAME): network mode(s) with zero successes on Windows: $B6N"; first_errors "$ATT6N" | sed 's/^/      /'; fi
        if [ -n "${ATT1:-}" ]; then
          D6N="$(diff_modes "$ATT1" "$ATT6N")"
          if [ -n "$D6N" ]; then note "  differs from the Linux runner (phase 1):"; echo "$D6N" | sed 's/^/      /'; else note "  network verdicts identical to the Linux runner (phase 1) mode for mode"; fi
        fi
        record "phase 6  $WNAME network → $R_STATUS ok=$R_OK fail=$R_FAIL${B6N:+ BROKEN=$B6N} ($(per_mode_stats "$ATT6N"))"
      fi

      # 6c. the phase-2 proxy matrix through the FIRST Linux proxy target, pinned to the Windows runner.
      PT=""; i=1; total="$(target_count)"
      while [ "$i" -le "$total" ]; do st="$(stack_of "$i")"; if [ "$st" != rust ] && [ "$st" != windows ]; then PT="$i"; break; fi; i=$((i + 1)); done
      if [ -z "$PT" ]; then
        note "  no Linux proxy target in this lab (targets=$TARGETS) — proxy matrix on the Windows runner skipped; use: lab.sh up --targets rust,nginx"
        record "phase 6  $WNAME proxy → (skipped: no linux proxy target)"
      else
        pst="$(stack_of "$PT")"
        DEP="$(api GET "/api/projects/$PID/deployments?limit=100" | jq -r --arg n "lab-target-${PT}-${pst}" \
          '(if type=="array" then . else (.deployments // .items // []) end) | [.[]|select(.name==$n)][0] | (.id // .deployment_id) // empty')"
        if [ -z "$DEP" ]; then fail "phase 6 ($WNAME): deployment for target-$PT ($pst) not found"; else
          STACK_MATRIX="$MATRIX"; stack_h3 "$pst" || STACK_MATRIX="$(strip_h3_modes "$MATRIX")"
          MODES_JSON="$(jq -nc --arg m "$STACK_MATRIX" '($m|split(",")) + ["native"]')"
          CFG6P="$(create_config "lab-p6-win-${pst}-${WNAME}-$STAMP" \
            "$(jq -nc --arg d "$DEP" --arg s "$pst" '{kind:"proxy",proxy_endpoint_id:$d,proxy_stack:$s}')" \
            "$(jq -nc --argjson modes "$MODES_JSON" --argjson r "$RUNS" '{modes:$modes,runs:$r,concurrency:1,timeout_ms:15000,capture_mode:"headers-only",payload_sizes:[]}')")" || exit 1
          LRESP="$(api POST "/api/v2/test-configs/$CFG6P/launch" "$(jq -nc --arg t "$WTESTER" '{tester_id:$t}')")"
          RUN6P="$(jq -r '.run_id // .id // empty' <<<"$LRESP")"
          if [ -z "$RUN6P" ]; then
            fail "phase 6 ($WNAME): proxy launch pinned to tester $WTESTER failed: $(head -c 300 <<<"$LRESP")"
          else
            note "  run $RUN6P (matrix $STACK_MATRIX + native → target-$PT $pst at $(target_ip "$PT")) pinned to $WNAME — waiting"
            wait_run "$RUN6P" || fail "phase 6 ($WNAME): proxy run $RUN6P did not finish in ${RUN_TIMEOUT}s (last=$R_STATUS)"
            ATT6P="$(attempts_of "$RUN6P")"
            note "  status=$R_STATUS ok=$R_OK fail=$R_FAIL attempts=$(jq length <<<"$ATT6P") worker=${R_WORKER:-?}"
            echo "    $(per_mode_stats "$ATT6P")"
            case "$R_STATUS" in completed|partial) ;; *) fail "phase 6 ($WNAME): proxy run status '$R_STATUS' ${R_ERR:+— $R_ERR}";; esac
            [ "$R_WORKER" = "$WID" ] || fail "phase 6 ($WNAME): proxy run executed on worker ${R_WORKER:-?}, not the pinned Windows agent $WID"
            B6P="$(broken_modes "$ATT6P")"
            if [ -n "$B6P" ]; then fail "phase 6 ($WNAME): proxy mode(s) with ZERO successes through $pst from Windows: $B6P"; first_errors "$ATT6P" | sed 's/^/      /'; fi
            NATIVE6="$(jq '[.[]|select(.protocol=="native")]|length' <<<"$ATT6P")"
            [ "$NATIVE6" -eq 0 ] || fail "phase 6 ($WNAME): 'native' produced $NATIVE6 attempt(s) — dispatch must DROP it"
            if [ -n "${P2_LINUX_ATT:-}" ] && [ "${P2_LINUX_STACK:-}" = "$pst" ]; then
              D6P="$(diff_modes "$P2_LINUX_ATT" "$ATT6P")"
              if [ -n "$D6P" ]; then note "  differs from the Linux runner (phase 2, $pst):"; echo "$D6P" | sed 's/^/      /'; else note "  proxy verdicts identical to the Linux runner (phase 2, $pst) mode for mode"; fi
            fi
            record "phase 6  $WNAME proxy/$pst → $R_STATUS ok=$R_OK fail=$R_FAIL${B6P:+ BROKEN=$B6P} ($(per_mode_stats "$ATT6P"))"
          fi
        fi
      fi
    done
    [ "$FAILS" -eq "$P6_FAILS_BEFORE" ] && pass "phase 6 (windows runner)"
  fi
fi

summary
[ "$FAILS" -eq 0 ]
