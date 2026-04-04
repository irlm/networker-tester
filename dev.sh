#!/usr/bin/env bash
# ─── Local Development Launcher ──────────────────────────────────────────────
# Starts all dashboard dependencies: PostgreSQL, endpoint, dashboard, frontend.
# Generates a random admin password each run. Ctrl+C stops everything.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# ── Colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'
CYAN='\033[0;36m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
DIM='\033[2m'
BOLD='\033[1m'
NC='\033[0m'

# ── Dependency checks ────────────────────────────────────────────────────────
MISSING=()

check_cmd() {
  if ! command -v "$1" > /dev/null 2>&1; then
    MISSING+=("$1: $2")
  fi
}

check_cmd docker      "Install Docker Desktop: https://docs.docker.com/get-docker/"
check_cmd cargo       "Install Rust: https://rustup.rs/"
check_cmd node        "Install Node.js (>=18): https://nodejs.org/"
check_cmd npm         "Install Node.js (>=18): https://nodejs.org/"
check_cmd curl        "Install curl: brew install curl (macOS) or apt install curl (Linux)"
check_cmd openssl     "Install openssl: brew install openssl (macOS) or apt install openssl (Linux)"

# Docker must be running
if command -v docker > /dev/null 2>&1; then
  if ! docker info > /dev/null 2>&1; then
    MISSING+=("docker (daemon): Docker is installed but not running. Start Docker Desktop.")
  fi
fi

if [ ${#MISSING[@]} -gt 0 ]; then
  echo ""
  echo -e "${RED}${BOLD}  Missing dependencies:${NC}"
  echo ""
  for dep in "${MISSING[@]}"; do
    echo -e "  ${RED}x${NC} ${dep}"
  done
  echo ""
  echo -e "  Install the missing dependencies above and try again."
  echo ""
  exit 1
fi

# Check npm dependencies
if [ ! -d "dashboard/node_modules" ]; then
  echo -e "${DIM}Installing frontend dependencies...${NC}"
  (cd dashboard && npm install --silent)
fi

# ── Generate random secrets ───────────────────────────────────────────────────
ADMIN_EMAIL="admin@localhost"
ADMIN_PASSWORD="$(openssl rand -base64 12 | tr -d '/+=' | head -c 12)"
JWT_SECRET="$(openssl rand -base64 32)"
DB_URL="postgres://networker:networker@127.0.0.1:5432/networker_dashboard"

# ── Process tracking ─────────────────────────────────────────────────────────
PID_ENDPOINT=""
PID_DASHBOARD=""
PID_FRONTEND=""

cleanup() {
  echo ""
  echo -e "${YELLOW}Shutting down...${NC}"
  [ -n "$PID_FRONTEND"  ] && kill "$PID_FRONTEND"  2>/dev/null || true
  [ -n "$PID_DASHBOARD" ] && kill "$PID_DASHBOARD" 2>/dev/null || true
  [ -n "$PID_ENDPOINT"  ] && kill "$PID_ENDPOINT"  2>/dev/null || true
  wait "$PID_FRONTEND" "$PID_DASHBOARD" "$PID_ENDPOINT" 2>/dev/null || true
  echo -e "${GREEN}Done.${NC} (PostgreSQL left running — stop with: docker compose -f docker-compose.dashboard.yml down)"
}
trap cleanup EXIT INT TERM

# ── Kill stale processes on our ports ─────────────────────────────────────────
for port in 3000 5173 8080; do
  pid=$(lsof -ti :"$port" 2>/dev/null || true)
  if [ -n "$pid" ]; then
    echo -e "${DIM}Killing stale process on port $port (PID $pid)${NC}"
    kill "$pid" 2>/dev/null || true
    sleep 1
  fi
done

# ── Print banner ──────────────────────────────────────────────────────────────
echo ""
echo -e "${CYAN}${BOLD}  networker-tester local dev${NC}"
echo -e "${DIM}  ─────────────────────────────────────${NC}"
echo ""

# ── 1. PostgreSQL ─────────────────────────────────────────────────────────────
echo -e "${DIM}[1/5]${NC} Starting PostgreSQL..."
docker compose -f docker-compose.dashboard.yml up postgres -d --wait 2>&1 | grep -v "^time=" || true
echo -e "       ${GREEN}PostgreSQL ready${NC} (localhost:5432)"

# ── 2. Seed tester schema (TestRun etc, normally created by networker-tester) ─
echo -e "${DIM}[2/5]${NC} Seeding database schema..."
docker exec -i networker-tester-postgres-1 \
  psql -U networker -d networker_dashboard -q < "$SCRIPT_DIR/scripts/seed-schema.sql"
echo -e "       ${GREEN}Schema ready${NC}"

# ── 3. Build if needed ───────────────────────────────────────────────────────
echo -e "${DIM}[3/5]${NC} Building workspace..."
cargo build -p networker-endpoint -p networker-dashboard 2>&1 | tail -1

# ── 4. Start services ────────────────────────────────────────────────────────
echo -e "${DIM}[4/5]${NC} Starting endpoint (port 8080)..."
cargo run -p networker-endpoint -- --port 8080 > /dev/null 2>&1 &
PID_ENDPOINT=$!

echo -e "${DIM}[4/5]${NC} Starting dashboard (port 3000)..."
DASHBOARD_ADMIN_EMAIL="$ADMIN_EMAIL" \
DASHBOARD_ADMIN_PASSWORD="$ADMIN_PASSWORD" \
DASHBOARD_JWT_SECRET="$JWT_SECRET" \
DASHBOARD_DB_URL="$DB_URL" \
cargo run -p networker-dashboard 2>&1 &
PID_DASHBOARD=$!

echo -e "${DIM}[5/5]${NC} Starting frontend (port 5173)..."
(cd dashboard && npm run dev > /dev/null 2>&1) &
PID_FRONTEND=$!

# ── Wait for dashboard to be ready ───────────────────────────────────────────
echo ""
echo -e "${DIM}  Waiting for dashboard API...${NC}"
for i in $(seq 1 30); do
  if curl -sf http://localhost:3000/api/health > /dev/null 2>&1; then
    echo -e "  ${GREEN}Dashboard API ready${NC}"
    break
  fi
  # Check if process died
  if ! kill -0 "$PID_DASHBOARD" 2>/dev/null; then
    echo -e "  ${RED}Dashboard process exited unexpectedly. Check output above.${NC}"
    break
  fi
  sleep 1
done

if curl -sf http://localhost:3000/api/health > /dev/null 2>&1; then
  # seed_admin sets must_change_password=true by design. For local dev,
  # skip the forced change since we generated a random password above.
  docker exec -i networker-tester-postgres-1 \
    psql -U networker -d networker_dashboard -q -c \
    "UPDATE dash_user SET must_change_password = false WHERE email = '$ADMIN_EMAIL';" 2>/dev/null || true
fi

# ── Print credentials ─────────────────────────────────────────────────────────
echo ""
echo -e "${DIM}  ─────────────────────────────────────${NC}"
echo -e "  ${BOLD}Dashboard${NC}  ${CYAN}http://localhost:5173/${NC}"
echo -e "  ${BOLD}API${NC}        ${DIM}http://localhost:3000/${NC}"
echo -e "  ${BOLD}Endpoint${NC}   ${DIM}http://localhost:8080/${NC}"
echo ""
echo -e "  ${BOLD}Login${NC}"
echo -e "    Email:    ${GREEN}${ADMIN_EMAIL}${NC}"
echo -e "    Password: ${GREEN}${ADMIN_PASSWORD}${NC}"
echo -e "${DIM}  ─────────────────────────────────────${NC}"
echo ""
echo -e "  ${DIM}Press Ctrl+C to stop all services${NC}"
echo -e "  ${DIM}Press ? for help, / for search (in dashboard)${NC}"
echo ""

# ── Keep running ──────────────────────────────────────────────────────────────
wait
