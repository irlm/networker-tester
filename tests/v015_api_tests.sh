#!/usr/bin/env bash
# v0.15 Multi-Project API Integration Tests
# Requires: dashboard running on localhost:3000, PostgreSQL with test data
# Usage: bash tests/v015_api_tests.sh

set -euo pipefail

BASE="http://localhost:3000"
PASS=0
FAIL=0
ERRORS=()

# Colors
G='\033[0;32m'
R='\033[0;31m'
Y='\033[0;33m'
N='\033[0m'

ok() { ((PASS++)); echo -e "  ${G}✓${N} $1"; }
fail() { ((FAIL++)); ERRORS+=("$1: $2"); echo -e "  ${R}✗${N} $1 — $2"; }

assert_status() {
  local desc="$1" expected="$2" actual="$3"
  if [[ "$actual" == "$expected" ]]; then ok "$desc"; else fail "$desc" "expected $expected, got $actual"; fi
}

assert_contains() {
  local desc="$1" expected="$2" body="$3"
  if echo "$body" | grep -qF -- "$expected" 2>/dev/null || echo "$body" | grep -q -- "$expected" 2>/dev/null; then ok "$desc"; else fail "$desc" "body missing '$expected'"; fi
}

assert_not_contains() {
  local desc="$1" unexpected="$2" body="$3"
  if echo "$body" | grep -q "$unexpected"; then fail "$desc" "body contains '$unexpected'"; else ok "$desc"; fi
}

DEFAULT_PID="00000000-0000-0000-0000-000000000001"

# ── AUTH ──────────────────────────────────────────────────────────────────────

echo ""
echo "═══ AUTH TESTS ═══"

# Login
RESP=$(curl -s -w "\n%{http_code}" -X POST "$BASE/api/auth/login" \
  -H "Content-Type: application/json" \
  -d '{"email":"admin@local","password":"admin"}')
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
assert_status "Login returns 200" "200" "$STATUS"
assert_contains "Login returns token" "token" "$BODY"
# is_platform_admin is in the JWT claims, not the login response body
assert_contains "Login returns role" "role" "$BODY"

TOKEN=$(echo "$BODY" | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])" 2>/dev/null || echo "")
if [[ -z "$TOKEN" ]]; then
  echo -e "${R}FATAL: Cannot extract token. Aborting.${N}"
  exit 1
fi
AUTH="Authorization: Bearer $TOKEN"

# Wrong password
RESP=$(curl -s -w "\n%{http_code}" -X POST "$BASE/api/auth/login" \
  -H "Content-Type: application/json" \
  -d '{"email":"admin@local","password":"wrong"}')
STATUS=$(echo "$RESP" | tail -1)
assert_status "Wrong password returns 401" "401" "$STATUS"

# No auth header
RESP=$(curl -s -w "\n%{http_code}" "$BASE/api/projects")
STATUS=$(echo "$RESP" | tail -1)
assert_status "No auth header returns 401" "401" "$STATUS"

# Invalid token
RESP=$(curl -s -w "\n%{http_code}" -H "Authorization: Bearer invalid.token.here" "$BASE/api/projects")
STATUS=$(echo "$RESP" | tail -1)
assert_status "Invalid token returns 401" "401" "$STATUS"

# ── PROJECT CRUD ──────────────────────────────────────────────────────────────

echo ""
echo "═══ PROJECT CRUD TESTS ═══"

# List projects
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
assert_status "List projects returns 200" "200" "$STATUS"
assert_contains "List includes Default project" "Default" "$BODY"
assert_contains "List includes user role" "role" "$BODY"

# Create project
RESP=$(curl -s -w "\n%{http_code}" -X POST "$BASE/api/projects" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d "{\"name\":\"Test Project $(date +%s)\",\"description\":\"For API testing\"}")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
assert_status "Create project returns 201" "201" "$STATUS"
assert_contains "Create returns project_id" "project_id" "$BODY"
assert_contains "Create returns slug" "test-project" "$BODY"  # slug starts with test-project-*

TEST_PID=$(echo "$BODY" | python3 -c "import sys,json; print(json.load(sys.stdin)['project_id'])" 2>/dev/null || echo "")

# Get project detail
if [[ -n "$TEST_PID" ]]; then
  RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$TEST_PID")
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  assert_status "Get project detail returns 200" "200" "$STATUS"
  assert_contains "Detail includes name" "Test Project" "$BODY"
  assert_contains "Detail includes description" "For API testing" "$BODY"
  assert_contains "Detail includes settings" "settings" "$BODY"
fi

# Update project
if [[ -n "$TEST_PID" ]]; then
  RESP=$(curl -s -w "\n%{http_code}" -X PUT "$BASE/api/projects/$TEST_PID" \
    -H "$AUTH" -H "Content-Type: application/json" \
    -d '{"name":"Updated Test Project"}')
  STATUS=$(echo "$RESP" | tail -1)
  assert_status "Update project returns 200" "200" "$STATUS"
fi

# Cannot delete Default project
RESP=$(curl -s -w "\n%{http_code}" -X DELETE "$BASE/api/projects/$DEFAULT_PID" -H "$AUTH")
STATUS=$(echo "$RESP" | tail -1)
assert_status "Cannot delete Default project (400)" "400" "$STATUS"

# ── PROJECT MEMBERS ───────────────────────────────────────────────────────────

echo ""
echo "═══ PROJECT MEMBERS TESTS ═══"

if [[ -n "$TEST_PID" ]]; then
  # List members
  RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$TEST_PID/members")
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  assert_status "List members returns 200" "200" "$STATUS"
  assert_contains "Members includes creator" "admin@local" "$BODY"

  # Add member (nonexistent email)
  RESP=$(curl -s -w "\n%{http_code}" -X POST "$BASE/api/projects/$TEST_PID/members" \
    -H "$AUTH" -H "Content-Type: application/json" \
    -d '{"email":"nonexistent@test.com","role":"viewer"}')
  STATUS=$(echo "$RESP" | tail -1)
  assert_status "Add nonexistent member returns 404" "404" "$STATUS"

  # Invalid role
  RESP=$(curl -s -w "\n%{http_code}" -X POST "$BASE/api/projects/$TEST_PID/members" \
    -H "$AUTH" -H "Content-Type: application/json" \
    -d '{"email":"admin@local","role":"superadmin"}')
  STATUS=$(echo "$RESP" | tail -1)
  assert_status "Invalid role returns 400" "400" "$STATUS"
fi

# ── PROJECT-SCOPED RESOURCES ──────────────────────────────────────────────────

echo ""
echo "═══ PROJECT-SCOPED RESOURCE TESTS ═══"

# Dashboard summary
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/dashboard/summary")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
assert_status "Dashboard summary returns 200" "200" "$STATUS"
assert_contains "Summary has agents_online" "agents_online" "$BODY"
assert_contains "Summary has jobs_running" "jobs_running" "$BODY"

# Agents
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/agents")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List agents returns 200" "200" "$STATUS"

# Jobs
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/jobs?limit=3")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List jobs returns 200" "200" "$STATUS"

# Runs
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/runs?limit=3")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List runs returns 200" "200" "$STATUS"

# Schedules
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/schedules")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List schedules returns 200" "200" "$STATUS"

# Deployments
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/deployments?limit=3")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List deployments returns 200" "200" "$STATUS"

# Cloud connections
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/cloud-connections")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List cloud connections returns 200" "200" "$STATUS"

# URL tests
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/url-tests?limit=3")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List url-tests returns 200" "200" "$STATUS"

# Cloud status
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/cloud/status")
STATUS=$(echo "$RESP" | tail -1)
assert_status "Cloud status returns 200" "200" "$STATUS"

# Inventory
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/inventory")
STATUS=$(echo "$RESP" | tail -1)
assert_status "Inventory returns 200" "200" "$STATUS"

# ── PROJECT ISOLATION ─────────────────────────────────────────────────────────

echo ""
echo "═══ PROJECT ISOLATION TESTS ═══"

if [[ -n "$TEST_PID" ]]; then
  # New project should have no resources
  RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$TEST_PID/agents")
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  assert_status "New project agents returns 200" "200" "$STATUS"
  assert_contains "New project has empty agents" '\[\]' "$BODY"

  RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$TEST_PID/jobs?limit=10")
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  assert_status "New project jobs returns 200" "200" "$STATUS"
  assert_contains "New project has empty jobs" '\[\]' "$BODY"

  RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$TEST_PID/dashboard/summary")
  STATUS=$(echo "$RESP" | tail -1)
  BODY=$(echo "$RESP" | sed '$d')
  assert_status "New project summary returns 200" "200" "$STATUS"
  assert_contains "New project summary shows 0 agents" "\"agents_online\":0" "$BODY"
fi

# Non-existent project returns 404
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/00000000-0000-0000-0000-000000000099/agents")
STATUS=$(echo "$RESP" | tail -1)
assert_status "Non-existent project returns 404" "404" "$STATUS"

# ── SHARE LINKS ───────────────────────────────────────────────────────────────

echo ""
echo "═══ SHARE LINKS TESTS ═══"

# List share links (empty)
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/share-links")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List share links returns 200" "200" "$STATUS"

# Resolve invalid share link
RESP=$(curl -s -w "\n%{http_code}" "$BASE/api/share/invalid-token-that-does-not-exist")
STATUS=$(echo "$RESP" | tail -1)
assert_status "Invalid share link returns 404" "404" "$STATUS"

# ── CLOUD ACCOUNTS ────────────────────────────────────────────────────────────

echo ""
echo "═══ CLOUD ACCOUNTS TESTS ═══"

# List cloud accounts (empty)
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/cloud-accounts")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List cloud accounts returns 200" "200" "$STATUS"

# Create without credential key → 503
RESP=$(curl -s -w "\n%{http_code}" -X POST "$BASE/api/projects/$DEFAULT_PID/cloud-accounts" \
  -H "$AUTH" -H "Content-Type: application/json" \
  -d '{"name":"Test Azure","provider":"azure","credentials":{"tenant_id":"t","client_id":"c","client_secret":"s"},"personal":false}')
STATUS=$(echo "$RESP" | tail -1)
assert_status "Create account without key returns 503" "503" "$STATUS"

# ── COMMAND APPROVALS ─────────────────────────────────────────────────────────

echo ""
echo "═══ COMMAND APPROVALS TESTS ═══"

# List pending approvals
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/command-approvals")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List pending approvals returns 200" "200" "$STATUS"

# Pending count
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/command-approvals/count")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
assert_status "Pending count returns 200" "200" "$STATUS"
assert_contains "Count includes count field" "count" "$BODY"

# ── VISIBILITY RULES ─────────────────────────────────────────────────────────

echo ""
echo "═══ VISIBILITY RULES TESTS ═══"

RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/projects/$DEFAULT_PID/visibility-rules")
STATUS=$(echo "$RESP" | tail -1)
assert_status "List visibility rules returns 200" "200" "$STATUS"

# ── GLOBAL ENDPOINTS ──────────────────────────────────────────────────────────

echo ""
echo "═══ GLOBAL ENDPOINTS TESTS ═══"

# Modes
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/modes")
STATUS=$(echo "$RESP" | tail -1)
assert_status "Modes returns 200" "200" "$STATUS"

# Version
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/version")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
assert_status "Version returns 200" "200" "$STATUS"
assert_contains "Version includes dashboard_version" "dashboard_version" "$BODY"

# Users (admin only)
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/users")
STATUS=$(echo "$RESP" | tail -1)
assert_status "Users list returns 200" "200" "$STATUS"

# Profile
RESP=$(curl -s -w "\n%{http_code}" -H "$AUTH" "$BASE/api/auth/profile")
STATUS=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
assert_status "Profile returns 200" "200" "$STATUS"
assert_contains "Profile includes email" "admin@local" "$BODY"

# ── CLEANUP ───────────────────────────────────────────────────────────────────

echo ""
echo "═══ CLEANUP ═══"

if [[ -n "$TEST_PID" ]]; then
  RESP=$(curl -s -w "\n%{http_code}" -X DELETE "$BASE/api/projects/$TEST_PID" -H "$AUTH")
  STATUS=$(echo "$RESP" | tail -1)
  assert_status "Delete test project returns 200" "200" "$STATUS"
fi

# ── SUMMARY ───────────────────────────────────────────────────────────────────

echo ""
echo "════════════════════════════════"
echo -e " ${G}PASSED: $PASS${N}  ${R}FAILED: $FAIL${N}"
echo "════════════════════════════════"

if [[ $FAIL -gt 0 ]]; then
  echo ""
  echo "Failures:"
  for err in "${ERRORS[@]}"; do
    echo -e "  ${R}✗${N} $err"
  done
  exit 1
fi

echo ""
echo -e "${G}All tests passed!${N}"
