#!/usr/bin/env bash
# ─── Local Development Launcher ──────────────────────────────────────────────
# Starts the C# control plane stack on your machine (macOS or Linux):
#   PostgreSQL (docker) → networker-endpoint (Rust) → Networker.ControlPlane
#   (C#, :5030) → optional local Networker.Agent → Vite dev server (:5173).
# Ctrl+C stops everything (PostgreSQL is left running).
# First time on a machine: ./scripts/dev-setup.sh (macOS/Linux) or
# scripts/dev-setup.ps1 (Windows, then run this from Git Bash / WSL).
#
# Env overrides: DEV_ADMIN_EMAIL, DEV_ADMIN_PASSWORD (default admin@localhost /
# admin — a bootstrap admin is only seeded when dash_user is EMPTY, so a fixed
# default keeps the login stable across restarts), DEV_WITH_AGENT=1 (also run a
# local C# agent registered as a standalone agent in the first project),
# DEV_CP_PORT (5030), DEV_ENDPOINT_PORT (8080).
#
# For a multi-runner / multi-target Docker environment that mirrors cloud VMs
# (validate before provisioning), see ./lab/lab.sh instead.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Machine-specific settings from scripts/dev-setup.sh (free Postgres host port,
# .NET SDK quirks). Run it once: ./scripts/dev-setup.sh
# shellcheck disable=SC1091
[ -f "$SCRIPT_DIR/.dev.env" ] && . "$SCRIPT_DIR/.dev.env"

# ── Colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; CYAN='\033[0;36m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'
DIM='\033[2m'; BOLD='\033[1m'; NC='\033[0m'

ADMIN_EMAIL="${DEV_ADMIN_EMAIL:-admin@localhost}"
ADMIN_PASSWORD="${DEV_ADMIN_PASSWORD:-admin}"
CP_PORT="${DEV_CP_PORT:-5030}"
ENDPOINT_PORT="${DEV_ENDPOINT_PORT:-8080}"
WITH_AGENT="${DEV_WITH_AGENT:-0}"
COMPOSE_FILE="docker-compose.dashboard.yml"
DB_NAME="networker_core"   # must match POSTGRES_DB in $COMPOSE_FILE
export DEV_PG_PORT="${DEV_PG_PORT:-5432}"   # host port for the compose postgres (see .dev.env)
DB_URL_NPGSQL="Host=127.0.0.1;Port=${DEV_PG_PORT};Database=${DB_NAME};Username=networker;Password=networker"
# Extra msbuild args (e.g. -p:UseAppHost=false on SDKs with a non-nuget RID).
# shellcheck disable=SC2206
DOTNET_ARGS=( ${DOTNET_BUILD_EXTRA_ARGS:-} )

# ── Dependency checks ────────────────────────────────────────────────────────
MISSING=()
check_cmd() { command -v "$1" > /dev/null 2>&1 || MISSING+=("$1: $2"); }
check_cmd docker  "Install Docker Desktop: https://docs.docker.com/get-docker/"
check_cmd cargo   "Install Rust: https://rustup.rs/"
check_cmd dotnet  "Install .NET 10 SDK: https://dotnet.microsoft.com/download"
check_cmd node    "Install Node.js (>=20): https://nodejs.org/"
check_cmd npm     "Install Node.js (>=20): https://nodejs.org/"
check_cmd curl    "Install curl: brew install curl (macOS) or apt install curl (Linux)"
check_cmd openssl "Install openssl: brew install openssl (macOS) or apt install openssl (Linux)"
if command -v docker > /dev/null 2>&1; then
  docker info > /dev/null 2>&1 || MISSING+=("docker (daemon): Docker is installed but not running. Start Docker Desktop.")
  docker compose version > /dev/null 2>&1 || MISSING+=("docker compose (v2 plugin): https://docs.docker.com/compose/install/")
fi
if [ ${#MISSING[@]} -gt 0 ]; then
  echo ""; echo -e "${RED}${BOLD}  Missing dependencies:${NC}"; echo ""
  for dep in "${MISSING[@]}"; do echo -e "  ${RED}x${NC} ${dep}"; done
  echo ""; echo "  Install the missing dependencies above and try again."; echo ""
  exit 1
fi

# npm deps
if [ ! -d "dashboard/node_modules" ]; then
  echo -e "${DIM}Installing frontend dependencies...${NC}"
  (cd dashboard && npm install --silent)
fi

# ── Secrets (fixed per checkout so JWTs survive restarts) ────────────────────
SECRET_FILE="$SCRIPT_DIR/.dev-secrets.env"
if [ ! -f "$SECRET_FILE" ]; then
  {
    echo "DASHBOARD_JWT_SECRET=$(openssl rand -base64 32)"
    echo "DASHBOARD_CREDENTIAL_KEY=$(openssl rand -hex 32)"
  } > "$SECRET_FILE"
fi
# shellcheck disable=SC1090
. "$SECRET_FILE"

# ── Process tracking ─────────────────────────────────────────────────────────
PIDS=()
cleanup() {
  echo ""; echo -e "${YELLOW}Shutting down...${NC}"
  local p
  for p in ${PIDS[@]+"${PIDS[@]}"}; do
    # Kill the whole process group so `dotnet run` / vite children die too.
    kill -- "-$p" 2>/dev/null || kill "$p" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  echo -e "${GREEN}Done.${NC} (PostgreSQL left running — stop with: docker compose -f $COMPOSE_FILE down)"
}
trap cleanup EXIT INT TERM

# ── Kill stale processes on our ports (portable: lsof → ss → fuser) ──────────
pids_on_port() {
  local port="$1"
  if command -v lsof > /dev/null 2>&1; then lsof -ti :"$port" 2>/dev/null || true
  elif command -v ss > /dev/null 2>&1; then ss -lptnH "sport = :$port" 2>/dev/null | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | sort -u
  elif command -v fuser > /dev/null 2>&1; then fuser "$port"/tcp 2>/dev/null | tr -s ' ' '\n' | grep . || true
  elif command -v netstat > /dev/null 2>&1; then # Windows (Git Bash / WSL without ss)
    netstat -ano 2>/dev/null | awk -v p=":$port" '$2 ~ p"$" && $4 == "LISTENING" {print $5}' | sort -u
  fi
}
for port in "$CP_PORT" 5173 "$ENDPOINT_PORT"; do
  for pid in $(pids_on_port "$port"); do
    echo -e "${DIM}Killing stale process on port $port (PID $pid)${NC}"
    kill "$pid" 2>/dev/null || true
  done
done

# ── Banner ───────────────────────────────────────────────────────────────────
echo ""; echo -e "${CYAN}${BOLD}  networker local dev (C# control plane)${NC}"
echo -e "${DIM}  ─────────────────────────────────────${NC}"; echo ""

# ── 1. PostgreSQL ────────────────────────────────────────────────────────────
echo -e "${DIM}[1/5]${NC} Starting PostgreSQL..."
if ! docker compose -f "$COMPOSE_FILE" up postgres -d --wait; then
  echo -e "  ${RED}PostgreSQL failed to start${NC} — is something else bound to :${DEV_PG_PORT}? Run ./scripts/dev-setup.sh to pick a free port, or: docker compose -f $COMPOSE_FILE logs postgres"
  exit 1
fi
PG_CONTAINER="$(docker compose -f "$COMPOSE_FILE" ps -q postgres)"
echo -e "       ${GREEN}PostgreSQL ready${NC} (localhost:${DEV_PG_PORT}, db ${DB_NAME})"

# ── 2. Build ─────────────────────────────────────────────────────────────────
echo -e "${DIM}[2/5]${NC} Building endpoint + control plane..."
build_step() { # build_step LABEL CMD... — quiet on success, full output on failure
  local label="$1"; shift
  local out
  if out="$("$@" 2>&1)"; then echo "       $label ok"; else echo "$out" | tail -40; echo -e "  ${RED}$label failed${NC}"; exit 1; fi
}
build_step "networker-endpoint"        cargo build -p networker-endpoint
build_step "Networker.ControlPlane"    dotnet build src/Networker.ControlPlane -c Debug -v quiet -nologo ${DOTNET_ARGS[@]+"${DOTNET_ARGS[@]}"}
[ "$WITH_AGENT" = 1 ] && build_step "Networker.Agent" dotnet build src/Networker.Agent -c Debug -v quiet -nologo ${DOTNET_ARGS[@]+"${DOTNET_ARGS[@]}"}

# ── 3. Endpoint ──────────────────────────────────────────────────────────────
echo -e "${DIM}[3/5]${NC} Starting endpoint (port ${ENDPOINT_PORT})..."
set -m
cargo run -q -p networker-endpoint -- --http-port "$ENDPOINT_PORT" > /dev/null 2>&1 &
PIDS+=($!)

# ── 4. Control plane (runs migrations + bootstrap admin on startup) ──────────
echo -e "${DIM}[4/5]${NC} Starting control plane (port ${CP_PORT})..."
DASHBOARD_ADMIN_EMAIL="$ADMIN_EMAIL" \
DASHBOARD_ADMIN_PASSWORD="$ADMIN_PASSWORD" \
DASHBOARD_JWT_SECRET="$DASHBOARD_JWT_SECRET" \
DASHBOARD_CREDENTIAL_KEY="$DASHBOARD_CREDENTIAL_KEY" \
DASHBOARD_DB_URL_NPGSQL="$DB_URL_NPGSQL" \
DASHBOARD_PUBLIC_URL="http://localhost:${CP_PORT}" \
ASPNETCORE_URLS="http://0.0.0.0:${CP_PORT}" \
ASPNETCORE_ENVIRONMENT=Development \
  dotnet run --project src/Networker.ControlPlane --no-build ${DOTNET_ARGS[@]+"${DOTNET_ARGS[@]}"} 2>&1 &
PIDS+=($!)

# ── 5. Frontend ──────────────────────────────────────────────────────────────
echo -e "${DIM}[5/5]${NC} Starting frontend (port 5173, proxies /api + /ws → :${CP_PORT})..."
( cd dashboard && exec npm run dev > /dev/null 2>&1 ) &
PIDS+=($!)
set +m

# ── Wait for control plane ───────────────────────────────────────────────────
echo ""; echo -e "${DIM}  Waiting for control plane API...${NC}"
READY=0
for _ in $(seq 1 90); do
  if curl -sf "http://localhost:${CP_PORT}/api/health/ready" > /dev/null 2>&1; then READY=1; break; fi
  sleep 1
done
if [ "$READY" = 1 ]; then
  echo -e "  ${GREEN}Control plane ready${NC}"
  # The bootstrap admin is seeded with must_change_password=true by design;
  # for local dev skip the forced change.
  docker exec -i "$PG_CONTAINER" psql -U networker -d "$DB_NAME" -q -c \
    "UPDATE dash_user SET must_change_password = false WHERE email = '$ADMIN_EMAIL';" 2>/dev/null || true
else
  echo -e "  ${RED}Control plane did not become ready in 90s — check the output above.${NC}"
fi

# ── Optional local agent (standalone, first project) ────────────────────────
if [ "$WITH_AGENT" = 1 ] && [ "$READY" = 1 ]; then
  AGENT_KEY_FILE="$SCRIPT_DIR/.dev-agent.key"
  [ -s "$AGENT_KEY_FILE" ] || LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 48 > "$AGENT_KEY_FILE"
  AGENT_KEY="$(cat "$AGENT_KEY_FILE")"
  # Register (idempotently) as a standalone agent in the first project, if any.
  docker exec -i "$PG_CONTAINER" psql -U networker -d "$DB_NAME" -q <<SQL 2>/dev/null || true
INSERT INTO agent (agent_id, name, api_key_hash, region, provider, project_id, status)
SELECT gen_random_uuid(), 'dev-local-agent', encode(sha256(convert_to('${AGENT_KEY}','UTF8')),'hex'),
       'local', 'local', p.project_id, 'offline'
FROM project p ORDER BY created_at LIMIT 1
ON CONFLICT (api_key_hash) DO NOTHING;
SQL
  echo -e "${DIM}  Starting local agent (registered in the first project; create a project first if none)...${NC}"
  set -m
  AGENT_API_KEY="$AGENT_KEY" AGENT_DASHBOARD_URL="ws://localhost:${CP_PORT}/ws/agent" \
  AGENT_TESTER_PATH="$SCRIPT_DIR/target/debug/networker-tester" \
    dotnet run --project src/Networker.Agent --no-build ${DOTNET_ARGS[@]+"${DOTNET_ARGS[@]}"} > /dev/null 2>&1 &
  PIDS+=($!)
  set +m
fi

# ── Print credentials ─────────────────────────────────────────────────────────
echo ""; echo -e "${DIM}  ─────────────────────────────────────${NC}"
echo -e "  ${BOLD}Dashboard${NC}  ${CYAN}http://localhost:5173/${NC}"
echo -e "  ${BOLD}API${NC}        ${DIM}http://localhost:${CP_PORT}/api${NC}"
echo -e "  ${BOLD}Endpoint${NC}   ${DIM}http://localhost:${ENDPOINT_PORT}/${NC}"
echo ""; echo -e "  ${BOLD}Login${NC}"
echo -e "    Email:    ${GREEN}${ADMIN_EMAIL}${NC}"
echo -e "    Password: ${GREEN}${ADMIN_PASSWORD}${NC}  ${DIM}(only seeded on an empty database)${NC}"
echo -e "${DIM}  ─────────────────────────────────────${NC}"; echo ""
echo -e "  ${DIM}Press Ctrl+C to stop all services${NC}"; echo ""

wait
