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
#   4. tear the runner down (validates P1-16 teardown), ALWAYS — even on failure.
#
# Any assertion miss exits non-zero → the workflow goes red → watchers are
# alerted. Self-contained: no standing infra, ~$0.02 of VM time per run.
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
set -uo pipefail

BASE="${LAGHOUND_URL:-https://laghound.com}"
EMAIL="${CANARY_ADMIN_EMAIL:-admin@laghound.com}"
PROJECT_NAME="${CANARY_PROJECT_NAME:-Pre-Prod Testing}"
TARGET_HOST="${CANARY_TARGET_HOST:-example.com}"
REUSE_RUNNER="${CANARY_REUSE_RUNNER:-1}"

PROVISION_TIMEOUT=480   # s to wait for a fresh runner to come online (~5-7 min)
RUN_TIMEOUT=240         # s to wait for the run to reach a terminal state
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

summary "✅ run executed end-to-end: completed, $ATT_COUNT attempts persisted, $OK succeeded."
note "CANARY PASS"
