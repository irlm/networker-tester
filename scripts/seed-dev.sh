#!/usr/bin/env bash
# seed-dev.sh — Start local dev stack with mock data.
#
# Usage:
#   ./scripts/seed-dev.sh
#
# Prerequisites:
#   - Docker running
#   - .NET 10 SDK (the C# control plane owns the schema migrations)
#   - Node.js + npm for the dashboard frontend
#
# What it does:
#   1. Starts Postgres via docker compose
#   2. Starts the C# control plane once (runs migrations + bootstrap admin)
#   3. Seeds mock data via seed-dev.sql
#   4. Stops the control plane (you restart it yourself for dev — ./dev.sh)
#
# After running, start the full dev stack per CLAUDE.md (or just ./dev.sh).

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
cd "$ROOT"
# shellcheck disable=SC1091
[ -f "$ROOT/.dev.env" ] && . "$ROOT/.dev.env"
export DEV_PG_PORT="${DEV_PG_PORT:-5432}"


echo "==> Starting Postgres..."
docker compose -f docker-compose.dashboard.yml up -d postgres
sleep 3

CONTAINER="$(docker compose -f docker-compose.dashboard.yml ps -q postgres)"

echo "==> Waiting for Postgres to be ready..."
for i in $(seq 1 30); do
  if docker exec "$CONTAINER" pg_isready -U networker -d networker_core > /dev/null 2>&1; then
    break
  fi
  sleep 1
done

echo "==> Running the control plane once to apply migrations..."
DASHBOARD_DB_URL_NPGSQL="Host=127.0.0.1;Port=${DEV_PG_PORT};Database=networker_core;Username=networker;Password=networker" \
DASHBOARD_ADMIN_PASSWORD=admin \
DASHBOARD_ADMIN_EMAIL=admin@localhost \
DASHBOARD_JWT_SECRET=dev-secret-dev-secret-dev-secret-dev-secret \
DASHBOARD_CREDENTIAL_KEY=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef \
DASHBOARD_BACKGROUND_SERVICES=0 \
ASPNETCORE_URLS=http://127.0.0.1:3099 \
ASPNETCORE_ENVIRONMENT=Development \
  dotnet run --project src/Networker.ControlPlane ${DOTNET_BUILD_EXTRA_ARGS:-} &
DASH_PID=$!

echo "==> Waiting for the control plane to be ready (building + migrating)..."
for i in $(seq 1 240); do
  if curl -sf http://127.0.0.1:3099/api/health/ready > /dev/null 2>&1; then
    echo "    Control plane ready after ${i}s"
    break
  fi
  if ! kill -0 $DASH_PID 2>/dev/null; then
    echo "ERROR: control plane exited before becoming ready"
    exit 1
  fi
  sleep 1
done

echo "==> Seeding mock data..."
docker exec -i "$CONTAINER" psql -U networker -d networker_core < scripts/seed-dev.sql

echo "==> Stopping bootstrap control plane..."
kill $DASH_PID 2>/dev/null || true
wait $DASH_PID 2>/dev/null || true

echo ""
echo "Done! Mock data seeded. Start dev stack:"
echo ""
echo "  ./dev.sh          # postgres + endpoint + control plane (:5030) + frontend (:5173)"
echo ""
echo ""
echo "  Login: admin / admin"
