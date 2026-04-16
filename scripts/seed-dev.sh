#!/usr/bin/env bash
# seed-dev.sh — Start local dev stack with mock data.
#
# Usage:
#   ./scripts/seed-dev.sh
#
# Prerequisites:
#   - Docker running
#   - Rust toolchain + cargo installed
#   - Node.js + npm for the dashboard frontend
#
# What it does:
#   1. Starts Postgres via docker-compose
#   2. Starts the dashboard binary (creates tables via auto-migration)
#   3. Seeds mock data via seed-dev.sql
#   4. Stops the dashboard (you restart it yourself for dev)
#
# After running, start the full dev stack per CLAUDE.md:
#   DASHBOARD_ADMIN_PASSWORD=admin cargo run -p networker-dashboard
#   cd dashboard && npm run dev

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
cd "$ROOT"

DB_URL="postgres://networker:networker@127.0.0.1:5432/networker_core"

echo "==> Starting Postgres..."
docker compose -f docker-compose.dashboard.yml up -d postgres
sleep 3

echo "==> Waiting for Postgres to be ready..."
for i in $(seq 1 30); do
  if PGPASSWORD=networker psql -h 127.0.0.1 -U networker -d networker_core -c "SELECT 1" > /dev/null 2>&1; then
    break
  fi
  sleep 1
done

echo "==> Running dashboard to apply migrations..."
DASHBOARD_DB_URL="$DB_URL" \
DASHBOARD_ADMIN_PASSWORD=admin \
DASHBOARD_JWT_SECRET=dev-secret \
DASHBOARD_PORT=3099 \
  cargo run -p networker-dashboard &
DASH_PID=$!

echo "==> Waiting for dashboard (PID $DASH_PID) to finish migrations..."
sleep 8

echo "==> Seeding mock data..."
PGPASSWORD=networker psql -h 127.0.0.1 -U networker -d networker_core -f scripts/seed-dev.sql

echo "==> Stopping bootstrap dashboard..."
kill $DASH_PID 2>/dev/null || true
wait $DASH_PID 2>/dev/null || true

echo ""
echo "Done! Mock data seeded. Start dev stack:"
echo ""
echo "  # Terminal 1: dashboard (port 3000)"
echo "  DASHBOARD_ADMIN_PASSWORD=admin cargo run -p networker-dashboard"
echo ""
echo "  # Terminal 2: frontend (port 5173)"
echo "  cd dashboard && npm run dev"
echo ""
echo "  Login: admin / admin"
