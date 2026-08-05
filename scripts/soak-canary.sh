#!/usr/bin/env bash
# ── Prod run-execution canary ────────────────────────────────────────────────
#
# The soak check verifies /health + background loops — it does NOT execute a
# run, which is exactly how the run pipeline stayed silently broken for weeks
# (E2E pass 2026-07-28: P0-1 every run `failed` ~10ms after spawn; P0-2 attempts
# never persisted; P1-14 full-stack benchmarks never worked). This canary closes
# that gap: it drives a REAL run end-to-end through the public API and asserts
# the outcome the health checks can't see —
#
#   1. provision an ephemeral runner (also validates the installer bootstrap +
#      provisioning path each night),
#   2. launch a lightweight network probe against a stable public target,
#   3. assert the run reaches `completed` (catches P0-1: stderr-relay-fails-runs)
#      AND persisted attempt rows with successes (catches P0-2: attempts starved),
#   4. PHASE 2 (proxy-fronted apibench): provision a rust endpoint behind nginx
#      and run apibench THROUGH the proxy, asserting the /api/* attempts return
#      2xx (not the proxy's own 404). A network probe can't see this: apibench's
#      /api/* workloads only reach the backend if the proxy stack forwards them,
#      and that config regressed silently (v0.28.112: /api was never in the
#      nginx/caddy/apache/IIS allowlist → 404 on every attempt). This phase
#      exercises the DEPLOYED install.sh proxy config each night.
#   5. PHASE 3 (mode coverage): reuse the SAME rust@nginx endpoint (it is a full
#      networker-endpoint behind nginx) to run the deterministic HTTP/TCP mode
#      matrix through the proxy and assert every mode returns a successful
#      attempt — nothing else runs the full matrix against a proxied cloud
#      target, which is why v0.28.118's pageload3 (h3-GREASE) and websocket
#      (/ws not proxied) reached prod unflagged. Also asserts dispatch DROPS
#      native (v0.28.120) rather than failing it.
#   6. tear the runner AND the endpoint down (validates P1-16 teardown), ALWAYS
#      — even on failure.
#
# Any assertion miss exits non-zero → the workflow goes red → watchers are
# alerted. Self-contained: no standing infra, ~$0.03 of VM time per run.
#
# Env:
#   LAGHOUND_URL           base URL (default https://laghound.com)
#   DASHBOARD_ADMIN_PASSWORD   admin password (required; from the GH secret)
#   CANARY_ADMIN_EMAIL     admin email (default admin@laghound.com)
#   CANARY_PROJECT_NAME    project to run in (default "Pre-Prod Testing")
#   CANARY_TARGET_HOST     probe target (default example.com)
#   CANARY_REUSE_RUNNER    "1" → use an existing idle runner if present, don't
#                          provision/teardown (default "1"; set "0" to force a
#                          fresh ephemeral runner every run)
#   CANARY_APIBENCH        "1" → also drive the proxy-fronted apibench phase
#                          (default "1"; set "0" to skip, e.g. to save the
#                          endpoint VM cost during an incident)
#   CANARY_MODE_COVERAGE   "1" → also run the full mode matrix through the proxy
#                          (default "1"; requires the apibench phase since it
#                          reuses that endpoint)
set -uo pipefail

BASE="${LAGHOUND_URL:-https://laghound.com}"
EMAIL="${CANARY_ADMIN_EMAIL:-admin@laghound.com}"
PROJECT_NAME="${CANARY_PROJECT_NAME:-Pre-Prod Testing}"
TARGET_HOST="${CANARY_TARGET_HOST:-example.com}"
REUSE_RUNNER="${CANARY_REUSE_RUNNER:-1}"
APIBENCH="${CANARY_APIBENCH:-1}"
MODE_COVERAGE="${CANARY_MODE_COVERAGE:-1}"
# Phase 4 (matrix flow) is OFF by default: it provisions several VMs at once,
# so it runs on the weekly schedule, not nightly. CANARY_MATRIX=1 enables it.
MATRIX="${CANARY_MATRIX:-0}"

PROVISION_TIMEOUT=480   # s to wait for a fresh runner to come online (~5-7 min)
RUN_TIMEOUT=240         # s to wait for the run to reach a terminal state
APIBENCH_TIMEOUT=720    # s for the apibench endpoint to provision (~5 min) + run
MODE_TIMEOUT=420        # s for the full mode-matrix run against the endpoint
MATRIX_TIMEOUT=1500     # s for a multi-cell matrix to provision concurrently
POLL=10

# Phase 3 exercises the FULL HTTP/TCP mode matrix through the proxy. These are
# deterministic + proxy-reachable; each must return >0 successful attempts or a
# mode regressed (v0.28.118: pageload3/websocket were 0/N through nginx and
# nothing caught it until a user ran it). Deliberately excludes: path/ping
# (Azure blocks ICMP to public IPs), udp/stamp/mthroughput (direct UDP ports,
# not proxied), browser* (needs Chrome, flaky), rpm (UDP-timing). native is
# added separately to prove dispatch DROPS it (v0.28.120 filter).
MODE_MATRIX='["tcp","dns","tls","tlsresume","http1","http2","http3","curl","download","upload","pageload","pageload2","pageload3","websocket"]'

fail() { echo "❌ CANARY FAIL: $*" >&2; exit 1; }
note() { echo "→ $*"; }
summary() { [ -n "${GITHUB_STEP_SUMMARY:-}" ] && echo "$*" >>"$GITHUB_STEP_SUMMARY"; echo "$*"; }

need() { command -v "$1" >/dev/null 2>&1 || fail "missing dependency: $1"; }
need curl; need jq

[ -n "${DASHBOARD_ADMIN_PASSWORD:-}" ] || fail "DASHBOARD_ADMIN_PASSWORD is not set"

# ── auth ─────────────────────────────────────────────────────────────────────
TOKEN=$(curl -sf --max-time 15 "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' \
  -d "$(jq -nc --arg e "$EMAIL" --arg p "$DASHBOARD_ADMIN_PASSWORD" '{email:$e,password:$p}')" \
  | jq -r '.token // empty')
[ -n "$TOKEN" ] || fail "login failed (no token) for $EMAIL @ $BASE"
AUTH=(-H "Authorization: Bearer $TOKEN")
note "authenticated as $EMAIL"

api() { # api METHOD PATH [json-body]
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -s --max-time 30 -X "$method" "${AUTH[@]}" -H 'Content-Type: application/json' -d "$body" "$BASE$path"
  else
    curl -s --max-time 30 -X "$method" "${AUTH[@]}" "$BASE$path"
  fi
}

# ── discover project + azure account ─────────────────────────────────────────
PROJECTS=$(api GET /api/projects)
# /api/projects returns {projects:[…]}; tolerate a bare array too.
PID=$(jq -r --arg n "$PROJECT_NAME" \
  '(.projects // .) as $p | ([$p[]|select(.name==$n)][0].project_id // $p[0].project_id // empty)' <<<"$PROJECTS")
[ -n "$PID" ] || fail "no project found (looked for '$PROJECT_NAME')"
note "project: $PID"

ACCT=$(api GET "/api/projects/$PID/cloud-accounts" \
  | jq -r '[.[]|select(.provider=="azure" and .status=="active")][0].account_id // empty')
[ -n "$ACCT" ] || fail "no active azure cloud account in project $PID"

# ── ensure a runner ──────────────────────────────────────────────────────────
PROVISIONED=""
APIBENCH_CGS=""  # space-separated apibench comparison-group ids to reap (phase 2)
RUNNER_ID=$(api GET "/api/projects/$PID/testers" \
  | jq -r '[.[]|select(.power_state=="running" and .allocation=="idle")][0].tester_id // empty')

if [ "$REUSE_RUNNER" = "1" ] && [ -n "$RUNNER_ID" ]; then
  note "reusing online idle runner $RUNNER_ID"
else
  note "provisioning an ephemeral canary runner (azure/eastus/B1s)…"
  RUNNER_ID=$(api POST "/api/projects/$PID/testers" \
    "$(jq -nc --arg a "$ACCT" '{name:"soak-canary",cloud:"azure",region:"eastus",vm_size:"Standard_B1s",cloud_account_id:$a,requested_os:"linux",auto_probe_enabled:true}')" \
    | jq -r '.tester_id // .id // empty')
  [ -n "$RUNNER_ID" ] || fail "provision request returned no tester id"
  PROVISIONED="$RUNNER_ID"
  note "provisioning runner $RUNNER_ID; waiting up to ${PROVISION_TIMEOUT}s for it to come online…"
  deadline=$((SECONDS + PROVISION_TIMEOUT))
  while :; do
    state=$(api GET "/api/projects/$PID/testers" \
      | jq -r --arg id "$RUNNER_ID" '[.[]|select(.tester_id==$id)][0]|"\(.power_state)/\(.allocation)"')
    note "  runner state: $state"
    case "$state" in running/idle) break;; esac
    [ "$SECONDS" -ge "$deadline" ] && fail "runner did not come online within ${PROVISION_TIMEOUT}s (last=$state)"
    sleep "$POLL"
  done
  note "runner online"
fi

# ── teardown guard: force-delete a runner WE provisioned, on any exit ─────────
# BULLETPROOF: a teardown that can itself crash-and-leak defeats the purpose, so
# disable nounset inside the trap and default-guard every var — nothing here may
# fail to delete the runner (an earlier `$PROVISIONED…` abutment crashed this
# under `set -u` and leaked a live VM; never again).
cleanup() {
  local code=$?
  set +u
  # Tear down every apibench endpoint FIRST (the pricier resource). Resolve each
  # by the group's short id in the deployment name, so a mid-provision timeout
  # still reaps it. Deployment-delete best-effort deletes the VM by reverse
  # lookup; the OrphanReaperService backstops any it can't resolve (a failed
  # provision may have no resolvable endpoint yet). Multiple ids if phase 2
  # retried.
  if [ -n "${APIBENCH_CGS:-}" ] && [ -n "${PID:-}" ]; then
    local cg gid8 dep
    for cg in ${APIBENCH_CGS}; do
      gid8="${cg:0:8}"
      for dep in $(api GET "/api/projects/${PID}/deployments?limit=30" 2>/dev/null \
          | jq -r --arg g "cg-${gid8}" \
            '[ (if type=="array" then . else (.deployments // .items // []) end)[]? ] | .[] | select((.name // "")|contains($g)) | (.id // .deployment_id // .deploymentId) // empty' 2>/dev/null); do
        [ -n "$dep" ] && { note "tearing down apibench endpoint deployment ${dep} ..."; api DELETE "/api/projects/${PID}/deployments/${dep}" >/dev/null 2>&1 || true; }
      done
    done
  fi
  if [ -n "${PROVISIONED:-}" ]; then
    note "tearing down ephemeral runner ${PROVISIONED} ..."
    api DELETE "/api/projects/${PID:-}/testers/${PROVISIONED}?force=true" >/dev/null 2>&1 || true
  fi
  exit "$code"
}
trap cleanup EXIT

# ── find-or-create a lightweight probe config ────────────────────────────────
# dns+tcp+tls against a stable public host on :443 — produces attempt rows with
# per-phase results without depending on an HTTP path (avoids the P1-4 class).
# The config NAME is unique-constrained, so REUSE one canary config across runs
# (creating one every night collides after the first and accumulates rows).
CFG_NAME="soak-canary-probe"
CFG_ID=$(api GET "/api/v2/projects/$PID/test-configs" \
  | jq -r --arg n "$CFG_NAME" \
    '(if type=="array" then . else (.configs // .items // []) end) | [.[]|select(.name==$n)][0].id // empty')
if [ -z "$CFG_ID" ]; then
  CFG=$(api POST "/api/v2/projects/$PID/test-configs" \
    "$(jq -nc --arg n "$CFG_NAME" --arg h "$TARGET_HOST" \
       '{name:$n,endpoint:{kind:"network",host:$h,port:443},workload:{modes:["dns","tcp","tls"],runs:2,concurrency:1,timeout_ms:5000}}')")
  CFG_ID=$(jq -r '.id // empty' <<<"$CFG")
  [ -n "$CFG_ID" ] || fail "config create failed: $(head -c 200 <<<"$CFG")"
  note "created canary config $CFG_ID"
else
  note "reusing canary config $CFG_ID"
fi

RUN=$(api POST "/api/v2/test-configs/$CFG_ID/launch" '{}')
RUN_ID=$(jq -r '.run_id // .id // empty' <<<"$RUN")
[ -n "$RUN_ID" ] || fail "launch failed: $(head -c 200 <<<"$RUN")"
note "launched run $RUN_ID against $TARGET_HOST:443"

# ── poll to terminal ─────────────────────────────────────────────────────────
deadline=$((SECONDS + RUN_TIMEOUT))
STATUS="" ; OK="" ; FAILN=""
while :; do
  R=$(api GET "/api/v2/test-runs/$RUN_ID")
  STATUS=$(jq -r '.status // empty' <<<"$R")
  OK=$(jq -r '.success_count // 0' <<<"$R")
  FAILN=$(jq -r '.failure_count // 0' <<<"$R")
  note "  run: status=$STATUS ok=$OK fail=$FAILN"
  case "$STATUS" in completed|failed|partial) break;; esac
  [ "$SECONDS" -ge "$deadline" ] && fail "run $RUN_ID did not reach a terminal state within ${RUN_TIMEOUT}s (last status=$STATUS) — the pipeline may be stalled (P0-1 class)"
  sleep "$POLL"
done

# ── assert: completed + attempts persisted + successes ───────────────────────
ATT=$(api GET "/api/v2/test-runs/$RUN_ID/attempts?limit=50")
ATT_COUNT=$(jq -r 'if type=="array" then length else (.attempts // .items // []|length) end' <<<"$ATT")

summary "### Run-execution canary — run \`$RUN_ID\` (target $TARGET_HOST:443)"
summary "- status: **$STATUS**  ·  ok: **$OK**  ·  fail: **$FAILN**  ·  attempts persisted: **$ATT_COUNT**"

[ "$STATUS" = "completed" ] || fail "run status is '$STATUS', expected 'completed' (P0-1: runs marked failed by relayed tester output?)"
[ "$ATT_COUNT" -gt 0 ] || fail "run completed but persisted 0 attempts (P0-2: attempt persistence broken?)"
[ "$OK" -gt 0 ] || fail "run completed with 0 successful attempts (probe never succeeded — dispatch/target/TLS regression?)"

summary "✅ phase 1 (network probe): completed, $ATT_COUNT attempts persisted, $OK succeeded."
note "PHASE 1 PASS"

# ── PHASE 2: proxy-fronted apibench ───────────────────────────────────────────
# Provision a rust endpoint behind nginx and run apibench THROUGH the proxy.
# A network probe can't catch a proxy-routing regression: apibench's /api/*
# workloads only reach the backend if the stack forwards them (v0.28.112 shipped
# with /api absent from every stack's allowlist → 404 on all attempts). The
# PRIMARY signal is: attempts return 2xx and ZERO return 404 — plus LANGUAGE
# AUTHENTICITY: the deploy log must show the cell's reference-API install and
# the /api reroute, or apibench silently measured the built-in endpoint /api
# (v0.28.114 class: `languages` written by the orchestrator, dropped on the
# install path — invisible to the 2xx assert).
#
# Endpoint provisioning (a fresh VM + install.sh bootstrap each night) has real
# transient-failure risk (apt/download/cert hiccups → "install.sh exited 1"),
# which is NOT the regression we're guarding and must not cry wolf. So a
# PROVISIONING failure (terminal-but-not-completed, zero attempts) is RETRIED
# once; only a routing failure (ran, but 404s) fails immediately, and a repeated
# provisioning failure fails as a genuine provisioning regression. On every
# non-pass the endpoint deploy log is captured to the summary BEFORE teardown so
# a red canary is diagnosable.
if [ "$APIBENCH" != "1" ]; then
  note "apibench phase disabled (CANARY_APIBENCH=$APIBENCH) — skipping"
  note "CANARY PASS (phase 1 only)"
  exit 0
fi

# deploy_log_of <group-id> — print the group's endpoint deploy log (empty if none).
deploy_log_of() {
  local g8="${1:0:8}" dep
  dep=$(api GET "/api/projects/$PID/deployments?limit=30" \
    | jq -r --arg g "cg-$g8" '[ (if type=="array" then . else (.deployments // .items // []) end)[]? | select((.name // "")|contains($g)) ][0] | (.id // .deployment_id // .deploymentId) // empty')
  [ -n "$dep" ] || return 0
  api GET "/api/projects/$PID/deployments/$dep" | jq -r '.log // ""'
}

# deployment_id_of <group-id> — the endpoint deployment id for the group.
deployment_id_of() {
  local g8="${1:0:8}"
  api GET "/api/projects/$PID/deployments?limit=30" \
    | jq -r --arg g "cg-$g8" '[ (if type=="array" then . else (.deployments // .items // []) end)[]? | select((.name // "")|contains($g)) ][0] | (.id // .deployment_id // .deploymentId) // empty'
}

# capture_deploy_log <group-id> — dump the endpoint deploy-log tail to the summary.
capture_deploy_log() {
  local g8="${1:0:8}" dep log
  dep=$(api GET "/api/projects/$PID/deployments?limit=30" \
    | jq -r --arg g "cg-$g8" '[ (if type=="array" then . else (.deployments // .items // []) end)[]? | select((.name // "")|contains($g)) ][0] | (.id // .deployment_id // .deploymentId) // empty')
  [ -n "$dep" ] || return 0
  log=$(api GET "/api/projects/$PID/deployments/$dep" | jq -r '.log // ""')
  [ -n "$log" ] || return 0
  summary "<details><summary>endpoint deploy log (tail) — deployment $dep</summary>"
  summary ''; summary '```'; printf '%s\n' "$log" | tail -c 1800 >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"; summary '```'; summary "</details>"
}

AB_ATTEMPTS=2
AB_PASS=0
# The cell's reference-API language. Keep in sync with the cell JSON below —
# the authenticity assert greps the deploy log for THIS language's install.
AB_LANG="rust"
for try in $(seq 1 "$AB_ATTEMPTS"); do
  CG_NAME="soak-canary-apibench-$(date -u +%Y%m%dT%H%M%SZ)-t${try}"
  CG_CREATE=$(api POST "/api/v2/projects/$PID/comparison-groups" \
    "$(jq -nc --arg n "$CG_NAME" --arg a "$ACCT" --arg r "$RUNNER_ID" '{
        name: $n,
        base_workload: {modes:["apibench"],runs:1,concurrency:1,timeout_ms:8000,capture_mode:"headers-only",payload_sizes:[]},
        methodology: null,
        cells: [ { label:"soak-canary rust@nginx apibench",
                   endpoint:{os:"linux",kind:"pending",region:"eastus",vm_size:"Standard_B2s",
                             language:"rust",topology:"loopback",proxy_stack:"nginx",cloud_account_id:$a},
                   runner_id:$r } ]
      }')")
  CG=$(jq -r '.id // .comparison_group_id // empty' <<<"$CG_CREATE")
  [ -n "$CG" ] || fail "apibench comparison-group create failed: $(head -c 200 <<<"$CG_CREATE")"
  APIBENCH_CGS="$APIBENCH_CGS $CG"   # register for teardown (cleanup reaps all)
  note "attempt $try/$AB_ATTEMPTS: created apibench group $CG ($CG_NAME)"

  LAUNCH=$(api POST "/api/v2/comparison-groups/$CG/launch" '{}')
  LAUNCHED=$(jq -r '.launched // 0' <<<"$LAUNCH")
  [ "$LAUNCHED" -ge 1 ] || fail "apibench launch created 0 runs: $(head -c 200 <<<"$LAUNCH")"
  note "  launched ($LAUNCHED cell) — provisioning rust@nginx endpoint…"

  AB_RUN=""; deadline=$((SECONDS + 60))
  while [ -z "$AB_RUN" ]; do
    AB_RUN=$(api GET "/api/v2/projects/$PID/test-runs?limit=20" \
      | jq -r --arg g "$CG" '(if type=="array" then . else (.items // .runs // .data // []) end) | [.[]|select((.comparisonGroupId // .comparison_group_id)==$g)][0].id // empty')
    [ -n "$AB_RUN" ] && break
    [ "$SECONDS" -ge "$deadline" ] && fail "no run appeared for apibench group $CG within 60s"
    sleep 5
  done
  note "  apibench run $AB_RUN"

  deadline=$((SECONDS + APIBENCH_TIMEOUT)); AB_STATUS=""
  while :; do
    AB_STATUS=$(api GET "/api/v2/test-runs/$AB_RUN" | jq -r '.status // empty')
    note "    run: status=$AB_STATUS"
    case "$AB_STATUS" in completed|failed|partial|cancelled|error) break;; esac
    [ "$SECONDS" -ge "$deadline" ] && fail "apibench run $AB_RUN did not reach a terminal state within ${APIBENCH_TIMEOUT}s (last=$AB_STATUS) — provisioning or dispatch stalled"
    sleep "$POLL"
  done

  AB_ATT=$(api GET "/api/v2/test-runs/$AB_RUN/attempts?limit=50")
  AB_TOTAL=$(jq -r '(if type=="array" then . else (.attempts // .items // .data // []) end)|length' <<<"$AB_ATT")
  AB_OK=$(jq -r '[(if type=="array" then . else (.attempts // .items // .data // []) end)[]|select(.success==true)]|length' <<<"$AB_ATT")
  AB_404=$(jq -r '[(if type=="array" then . else (.attempts // .items // .data // []) end)[]|select(.http.status_code==404)]|length' <<<"$AB_ATT")
  note "  result: status=$AB_STATUS attempts=$AB_TOTAL ok=$AB_OK 404s=$AB_404"

  if [ "$AB_STATUS" = "completed" ] && [ "$AB_TOTAL" -gt 0 ]; then
    # It RAN — this is the authoritative routing check; never retry a routing verdict.
    summary "### Phase 2 (proxy-fronted apibench) — run \`$AB_RUN\` (rust@nginx, try $try)"
    summary "- status: **$AB_STATUS**  ·  attempts: **$AB_TOTAL**  ·  2xx-ok: **$AB_OK**  ·  proxy-404s: **$AB_404**"
    [ "$AB_404" -eq 0 ] || { capture_deploy_log "$CG"; fail "apibench had $AB_404/$AB_TOTAL attempts return HTTP 404 — the proxy stack is NOT forwarding /api/* to the backend (v0.28.112 regression class)"; }
    [ "$AB_OK" -gt 0 ] || { capture_deploy_log "$CG"; fail "apibench completed with 0 successful (2xx) attempts — /api/* never reached the backend"; }

    # ── language authenticity ────────────────────────────────────────────
    # 2xx alone can't prove WHO served /api: networker-endpoint's built-in
    # /api answers 200 even when the cell's language server was silently
    # never installed (the v0.28.114 class — `languages` written by the
    # orchestrator but dropped on the install path). The target's ports are
    # NSG-blocked from CI, so the in-band evidence is the deploy log: the
    # language install + /api reroute both log unconditionally on success.
    AB_LOG=$(deploy_log_of "$CG")
    if ! grep -q "${AB_LANG} reference API running on" <<<"$AB_LOG"; then
      capture_deploy_log "$CG"
      fail "deploy log has no '${AB_LANG} reference API running' marker — the language server was never installed; apibench measured the built-in endpoint /api (v0.28.114 languages-wire regression class)"
    fi
    if ! grep -qE '/api (now|already) routed to the language server' <<<"$AB_LOG"; then
      capture_deploy_log "$CG"
      fail "deploy log has no '/api routed to the language server' marker — apibench measured the built-in endpoint /api, not the ${AB_LANG} reference server"
    fi
    summary "- language authenticity: **${AB_LANG} reference API installed + /api rerouted** (deploy-log markers present)"

    AB_PASS=1
    summary "✅ phase 2 (apibench through nginx): $AB_OK/$AB_TOTAL attempts 2xx, zero proxy-404s, served by the ${AB_LANG} reference API."
    break
  fi

  # Provisioning/dispatch failure (didn't run) — capture the log, free the VM, maybe retry.
  note "  attempt $try did NOT provision+run (status=$AB_STATUS, $AB_TOTAL attempts)"
  capture_deploy_log "$CG"
  if [ "$try" -lt "$AB_ATTEMPTS" ]; then
    note "  transient provisioning failure — tearing this endpoint down and retrying…"
    for dep in $(api GET "/api/projects/$PID/deployments?limit=30" | jq -r --arg g "cg-${CG:0:8}" '[ (if type=="array" then . else (.deployments // .items // []) end)[]? | select((.name // "")|contains($g)) ][]? | (.id // .deployment_id // .deploymentId) // empty'); do
      [ -n "$dep" ] && api DELETE "/api/projects/$PID/deployments/$dep" >/dev/null 2>&1 || true
    done
  fi
done

[ "$AB_PASS" = "1" ] || fail "apibench endpoint failed to provision+run after $AB_ATTEMPTS attempts (last status=$AB_STATUS) — provisioning regressed (see deploy log above)"
note "PHASE 2 PASS"

# ── PHASE 3: full mode-matrix coverage through the proxy ──────────────────────
# The phase-2 rust@nginx endpoint IS a full networker-endpoint behind nginx (the
# rust "language" == the endpoint binary), so it serves /download, /upload, /ws,
# /page, /asset, HTTP/1-2-3, etc. Reuse that SAME VM (no extra provision): run
# the deterministic HTTP/TCP mode matrix against it and assert every mode
# returns >0 successful attempts. Nothing else runs the full matrix against a
# proxied cloud target automatically — which is exactly why v0.28.118's
# pageload3 (h3-GREASE) and websocket (/ws not proxied) reached prod unflagged.
# Also proves dispatch DROPS native (v0.28.120) rather than failing it.
if [ "$MODE_COVERAGE" != "1" ]; then
  note "mode-coverage phase disabled (CANARY_MODE_COVERAGE=$MODE_COVERAGE) — skipping"
  note "CANARY PASS (phases 1 + 2)"
  exit 0
fi

# Target the phase-2 endpoint as a PROXY (its deployment id), NOT a raw network
# host/port: the server's mode↔target gate treats kind=network as an arbitrary
# URL and 422s the networker-endpoint modes (download/upload/rpm/websocket/
# page-load); kind=proxy maps to the "endpoint" capability, and dispatch
# resolves it to the endpoint IP + stack port and injects insecure (self-signed
# cert). native is included so the assert can prove dispatch DROPS it.
MC_DEP=$(deployment_id_of "$CG")
[ -n "$MC_DEP" ] || { capture_deploy_log "$CG"; fail "phase 3: could not resolve the phase-2 endpoint deployment id"; }
note "phase 3: mode matrix against the phase-2 endpoint (proxy deployment ${MC_DEP:0:8}, nginx)"

MC_NAME="soak-canary-modes-$(date -u +%Y%m%dT%H%M%SZ)"
MC_MODES=$(jq -nc --argjson m "$MODE_MATRIX" '$m + ["native"]')
MC_CFG=$(api POST "/api/v2/projects/$PID/test-configs" \
  "$(jq -nc --arg n "$MC_NAME" --arg d "$MC_DEP" --argjson modes "$MC_MODES" \
     '{name:$n, endpoint:{kind:"proxy", proxy_endpoint_id:$d, proxy_stack:"nginx"},
       workload:{modes:$modes, runs:2, concurrency:1, timeout_ms:8000, capture_mode:"headers-only", payload_sizes:[]}}')")
MC_CFG_ID=$(jq -r '.id // empty' <<<"$MC_CFG")
[ -n "$MC_CFG_ID" ] || fail "phase 3: mode-coverage config create failed: $(head -c 200 <<<"$MC_CFG")"

MC_RUN=$(api POST "/api/v2/test-configs/$MC_CFG_ID/launch" '{}')
MC_RUN_ID=$(jq -r '.run_id // .id // empty' <<<"$MC_RUN")
[ -n "$MC_RUN_ID" ] || fail "phase 3: mode-coverage launch failed (gate?): $(head -c 200 <<<"$MC_RUN")"
note "  mode-coverage run $MC_RUN_ID"

deadline=$((SECONDS + MODE_TIMEOUT)); MC_STATUS=""
while :; do
  MC_STATUS=$(api GET "/api/v2/test-runs/$MC_RUN_ID" | jq -r '.status // empty')
  note "    run: status=$MC_STATUS"
  case "$MC_STATUS" in completed|failed|partial|cancelled|error) break;; esac
  [ "$SECONDS" -ge "$deadline" ] && fail "phase 3 mode run did not reach a terminal state within ${MODE_TIMEOUT}s (last=$MC_STATUS)"
  sleep "$POLL"
done

MC_ATT=$(api GET "/api/v2/test-runs/$MC_RUN_ID/attempts?limit=300")
# Per-mode ok/total; identify any mode with ZERO successes (= regressed).
MC_STATS=$(jq -c '(if type=="array" then . else (.attempts // .items // .data // []) end)
  | group_by(.protocol) | map({m:.[0].protocol, ok:(map(select(.success==true))|length), n:length})' <<<"$MC_ATT")
MC_LINE=$(jq -r 'map("\(.m) \(.ok)/\(.n)")|join(" · ")' <<<"$MC_STATS")
MC_BROKEN=$(jq -r '[.[]|select(.ok==0)|.m]|join(", ")' <<<"$MC_STATS")
MC_NATIVE=$(jq -r '[(if type=="array" then . else (.attempts // .items // .data // []) end)[]|select(.protocol=="native")]|length' <<<"$MC_ATT")

summary "### Phase 3 (mode coverage through nginx) — run \`$MC_RUN_ID\` (proxy ${MC_DEP:0:8})"
summary "- $MC_LINE"

[ "$MC_STATUS" = "completed" ] || { capture_deploy_log "$CG"; fail "phase 3 mode run status is '$MC_STATUS', expected 'completed'"; }
# native must be DROPPED at dispatch (v0.28.120), not attempted-and-failed.
[ "$MC_NATIVE" -eq 0 ] || fail "phase 3: 'native' was attempted ($MC_NATIVE) — dispatch should filter catalog:false modes (v0.28.120 regression)"
# every matrix mode must have produced at least one success through the proxy.
[ -z "$MC_BROKEN" ] || { capture_deploy_log "$CG"; fail "phase 3: mode(s) returned ZERO successful attempts through nginx: ${MC_BROKEN} — a mode regressed (v0.28.118 pageload3/websocket class)"; }

summary "✅ phase 3 (mode coverage): all matrix modes returned successful attempts through nginx; native correctly dropped at dispatch."

# ── Phase 4: MULTI-CELL MATRIX FLOW (audit P0-5) ─────────────────────────────
# Phases 1-3 all drive ONE cell. The entire v0.28.129-147 campaign — VM-name
# collisions, IP-quota exhaustion, relaunch unique-name failures, cross-cell
# contention — lives in the CONCURRENT multi-cell path, which no automated
# check has ever exercised. This phase launches a small mixed matrix and
# asserts the flow-level invariants: every cell gets its own config, its own
# VM name, and reaches a terminal state without the group aborting.
if [ "$MATRIX" != "1" ]; then
  note "matrix-flow phase disabled (CANARY_MATRIX=$MATRIX) — skipping"
  note "CANARY PASS (phases 1 + 2 + 3)"
  exit 0
fi

note "phase 4: multi-cell matrix flow (concurrent provisioning)"
MX_NAME="soak-canary-matrix-$(date -u +%Y%m%dT%H%M%SZ)"
# 3 linux cells across DIFFERENT stacks: enough to exercise concurrency, the
# IP-quota throttle and per-cell naming without a large cloud bill.
MX_CELLS=$(jq -nc --arg acct "$ACCOUNT_ID" '[
  {label:"canary linux · nginx",   endpoint:{kind:"pending", cloud_account_id:$acct, region:"eastus", vm_size:"Standard_B2s", os:"linux", proxy_stack:"nginx",   language:"rust"}},
  {label:"canary linux · caddy",   endpoint:{kind:"pending", cloud_account_id:$acct, region:"eastus", vm_size:"Standard_B2s", os:"linux", proxy_stack:"caddy",   language:"rust"}},
  {label:"canary linux · traefik", endpoint:{kind:"pending", cloud_account_id:$acct, region:"eastus", vm_size:"Standard_B2s", os:"linux", proxy_stack:"traefik", language:"rust"}}
]')
MX_CG=$(api POST "/api/v2/projects/$PID/comparison-groups" \
  "$(jq -nc --arg n "$MX_NAME" --argjson cells "$MX_CELLS" \
     '{name:$n, base_workload:{modes:["http1","download"], runs:2, concurrency:1, timeout_ms:8000, payload_sizes:[]}, cells:$cells}')")
MX_CG_ID=$(jq -r '.id // empty' <<<"$MX_CG")
[ -n "$MX_CG_ID" ] || fail "phase 4: matrix group create failed: $(head -c 200 <<<"$MX_CG")"
# Register for teardown BEFORE launching, so a mid-provision failure still reaps.
APIBENCH_CGS="${APIBENCH_CGS:-} ${MX_CG_ID}"

MX_LAUNCH=$(api POST "/api/v2/comparison-groups/$MX_CG_ID/launch" '{}')
MX_TOTAL=$(jq -r '.total // 0' <<<"$MX_LAUNCH")
MX_OK=$(jq -r '.launched // 0' <<<"$MX_LAUNCH")
MX_FAILED=$(jq -r '.failed // 0' <<<"$MX_LAUNCH")
note "  matrix ${MX_CG_ID:0:8}: launched=${MX_OK}/${MX_TOTAL} failed=${MX_FAILED}"
[ "$MX_TOTAL" = "3" ] || fail "phase 4: expected 3 cells, group reports ${MX_TOTAL}"
[ "$MX_OK" = "3" ] || fail "phase 4: only ${MX_OK}/3 cells launched — errors: $(jq -c '.errors // []' <<<"$MX_LAUNCH")"

# Every cell must get its OWN config (the v0.28.129 shared-name collision).
MX_RUNS=$(api GET "/api/v2/test-runs?comparison_group_id=$MX_CG_ID&limit=20")
MX_CFG_COUNT=$(jq -r '[ (if type=="array" then . else (.runs // .items // .data // []) end)[]? | (.test_config_id // empty) ] | unique | length' <<<"$MX_RUNS")
[ "$MX_CFG_COUNT" = "3" ] || fail "phase 4: cells share configs (${MX_CFG_COUNT} distinct for 3 cells) — v0.28.129 collision class"

# Poll to terminal. Cells provision CONCURRENTLY under the IP-quota throttle,
# so this is the only automated check on that interaction.
deadline=$((SECONDS + MATRIX_TIMEOUT))
while :; do
  MX_STATE=$(api GET "/api/v2/test-runs?comparison_group_id=$MX_CG_ID&limit=20" \
    | jq -r '[ (if type=="array" then . else (.runs // .items // .data // []) end)[]? | (.status // "?") ] | join(",")')
  note "    cells: $MX_STATE"
  case "$MX_STATE" in *queued*|*provisioning*|*running*) : ;; *) break ;; esac
  [ "$SECONDS" -ge "$deadline" ] && { note "phase 4: matrix did not settle within ${MATRIX_TIMEOUT}s (last=$MX_STATE)"; break; }
  sleep 30
done

MX_FINAL=$(api GET "/api/v2/test-runs?comparison_group_id=$MX_CG_ID&limit=20")
MX_DONE=$(jq -r '[ (if type=="array" then . else (.runs // .items // .data // []) end)[]? | select((.status // "")=="completed") ] | length' <<<"$MX_FINAL")
MX_ERRS=$(jq -r '[ (if type=="array" then . else (.runs // .items // .data // []) end)[]? | select((.status // "")!="completed") | "\(.status // "?"): \((.error_message // "")[0:70])" ] | join(" | ")' <<<"$MX_FINAL")

summary "### Phase 4 (multi-cell matrix flow) — group \`$MX_CG_ID\`"
summary "- cells completed: ${MX_DONE}/3"
[ -n "$MX_ERRS" ] && summary "- non-completed: ${MX_ERRS}"

# At least 2 of 3 must complete: one cell lost to genuine cloud flake is
# tolerable, but a systemic breakage (name collision, quota, dispatch) takes
# them all down together — which is precisely what this phase exists to catch.
[ "$MX_DONE" -ge 2 ] || fail "phase 4: only ${MX_DONE}/3 matrix cells completed — the multi-cell flow regressed: ${MX_ERRS}"

summary "✅ phase 4 (matrix flow): ${MX_DONE}/3 concurrent cells completed with distinct configs."
note "CANARY PASS (phases 1 + 2 + 3 + 4)"
