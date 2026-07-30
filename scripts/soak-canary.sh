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
#   5. tear the runner AND the apibench endpoint down (validates P1-16 teardown),
#      ALWAYS — even on failure.
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
set -uo pipefail

BASE="${LAGHOUND_URL:-https://laghound.com}"
EMAIL="${CANARY_ADMIN_EMAIL:-admin@laghound.com}"
PROJECT_NAME="${CANARY_PROJECT_NAME:-Pre-Prod Testing}"
TARGET_HOST="${CANARY_TARGET_HOST:-example.com}"
REUSE_RUNNER="${CANARY_REUSE_RUNNER:-1}"
APIBENCH="${CANARY_APIBENCH:-1}"

PROVISION_TIMEOUT=480   # s to wait for a fresh runner to come online (~5-7 min)
RUN_TIMEOUT=240         # s to wait for the run to reach a terminal state
APIBENCH_TIMEOUT=720    # s for the apibench endpoint to provision (~5 min) + run
POLL=10

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
# PRIMARY signal is: attempts return 2xx and ZERO return 404.
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
    AB_PASS=1
    summary "✅ phase 2 (apibench through nginx): $AB_OK/$AB_TOTAL attempts 2xx, zero proxy-404s."
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
note "CANARY PASS (phases 1 + 2)"
