#!/usr/bin/env bash
# ==============================================================================
# migrate-to-split.sh — One-time migration: networker_dashboard → networker_core + networker_logs
#
# Splits the monolithic networker_dashboard database into:
#   networker_core  — all non-log tables (application state, config, runs)
#   networker_logs  — benchmark_request_progress + perf_log (high-write log tables)
#
# Usage:
#   ./scripts/migrate-to-split.sh [--db-host HOST] [--db-user USER] [--db-port PORT]
#
# Defaults: host=localhost, user=networker, port=5432
#
# Pre-flight requirements:
#   - networker_dashboard must exist (source)
#   - networker_core must NOT exist (prevents accidental re-run)
#   - pg_dump and psql must be on PATH
# ==============================================================================
set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
DB_HOST="localhost"
DB_USER="networker"
DB_PORT="5432"
SOURCE_DB="networker_dashboard"
CORE_DB="networker_core"
LOGS_DB="networker_logs"

# Log tables to move to networker_logs
LOG_TABLES="benchmark_request_progress perf_log"

# ── Argument parsing ──────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
    case "$1" in
        --db-host) DB_HOST="$2"; shift 2 ;;
        --db-user) DB_USER="$2"; shift 2 ;;
        --db-port) DB_PORT="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 [--db-host HOST] [--db-user USER] [--db-port PORT]"
            exit 0
            ;;
        *) echo "ERROR: Unknown option: $1" >&2; exit 1 ;;
    esac
done

log() { echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] $*"; }
die() { echo "ERROR: $*" >&2; exit 1; }

# ── Wrapper functions (avoid unquoted command-string variables) ───────────────
run_psql() {
    psql -h "$DB_HOST" -U "$DB_USER" -p "$DB_PORT" "$@" < /dev/null
}

run_pgdump() {
    pg_dump -h "$DB_HOST" -U "$DB_USER" -p "$DB_PORT" "$@" < /dev/null
}

run_pgrestore() {
    pg_restore -h "$DB_HOST" -U "$DB_USER" -p "$DB_PORT" "$@" < /dev/null
}

# ── Pre-flight checks ─────────────────────────────────────────────────────────
log "Pre-flight: checking source database '$SOURCE_DB'..."

SOURCE_EXISTS=$(run_psql -d postgres -tAc \
    "SELECT 1 FROM pg_database WHERE datname='$SOURCE_DB'" 2>/dev/null || echo "")
if [ "$SOURCE_EXISTS" != "1" ]; then
    die "Source database '$SOURCE_DB' does not exist."
fi

log "Pre-flight: checking that '$CORE_DB' does not exist..."
CORE_EXISTS=$(run_psql -d postgres -tAc \
    "SELECT 1 FROM pg_database WHERE datname='$CORE_DB'" 2>/dev/null || echo "")
if [ "$CORE_EXISTS" = "1" ]; then
    die "Target database '$CORE_DB' already exists. Aborting to prevent data loss."
fi

log "Pre-flight checks passed."

# ── Temp files ────────────────────────────────────────────────────────────────
WORK_DIR=$(mktemp -d)
CORE_DUMP="${WORK_DIR}/core_full.dump"
LOGS_DUMP="${WORK_DIR}/logs_tables.dump"

cleanup() {
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

# ── Step 1: Full dump of networker_dashboard ──────────────────────────────────
log "Step 1/6: Dumping '$SOURCE_DB' (full, custom format)..."
run_pgdump -Fc -d "$SOURCE_DB" -f "$CORE_DUMP"
log "  Dump size: $(du -sh "$CORE_DUMP" | cut -f1)"

# ── Step 2: Create networker_core and restore full schema+data ─────────────────
log "Step 2/6: Creating '$CORE_DB' and restoring full schema+data..."
run_psql -d postgres -c "CREATE DATABASE $CORE_DB OWNER $DB_USER;"
run_pgrestore -d "$CORE_DB" --no-owner --role="$DB_USER" "$CORE_DUMP"
log "  '$CORE_DB' restored."

# ── Step 3: Dump log tables for networker_logs ────────────────────────────────
log "Step 3/6: Dumping log tables for '$LOGS_DB'..."
# Build argument list for pg_dump -t flags (one per log table)
DUMP_TABLE_ARGS=""
for tbl in $LOG_TABLES; do
    DUMP_TABLE_ARGS="$DUMP_TABLE_ARGS -t $tbl"
done
# SC2086: intentional word-split of DUMP_TABLE_ARGS to pass as separate -t flags
# shellcheck disable=SC2086
run_pgdump -Fc $DUMP_TABLE_ARGS -d "$SOURCE_DB" -f "$LOGS_DUMP"
log "  Logs dump size: $(du -sh "$LOGS_DUMP" | cut -f1)"

# ── Step 4: Create networker_logs and restore log tables ──────────────────────
log "Step 4/6: Creating '$LOGS_DB' and restoring log tables..."
run_psql -d postgres -c "CREATE DATABASE $LOGS_DB OWNER $DB_USER;"
run_pgrestore -d "$LOGS_DB" --no-owner --role="$DB_USER" "$LOGS_DUMP"
log "  '$LOGS_DB' populated."

# ── Step 5: Drop log tables from networker_core ───────────────────────────────
log "Step 5/6: Dropping log tables from '$CORE_DB'..."
for tbl in $LOG_TABLES; do
    log "  Dropping $tbl from $CORE_DB..."
    run_psql -d "$CORE_DB" -c "DROP TABLE IF EXISTS $tbl CASCADE;"
done
log "  Log tables removed from '$CORE_DB'."

# ── Step 6: Verify ────────────────────────────────────────────────────────────
log "Step 6/6: Verification..."

CORE_TABLE_COUNT=$(run_psql -d "$CORE_DB" -tAc \
    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public';" \
    2>/dev/null || echo "?")

LOGS_TABLE_COUNT=$(run_psql -d "$LOGS_DB" -tAc \
    "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='public';" \
    2>/dev/null || echo "?")

for tbl in $LOG_TABLES; do
    LOG_EXISTS=$(run_psql -d "$CORE_DB" -tAc \
        "SELECT 1 FROM information_schema.tables
         WHERE table_schema='public' AND table_name='$tbl';" \
        2>/dev/null || echo "")
    if [ "$LOG_EXISTS" = "1" ]; then
        die "Table '$tbl' still present in '$CORE_DB' after drop. Migration incomplete."
    fi

    LOG_IN_LOGS=$(run_psql -d "$LOGS_DB" -tAc \
        "SELECT 1 FROM information_schema.tables
         WHERE table_schema='public' AND table_name='$tbl';" \
        2>/dev/null || echo "")
    if [ "$LOG_IN_LOGS" != "1" ]; then
        die "Table '$tbl' missing from '$LOGS_DB'. Migration incomplete."
    fi
done

log "  $CORE_DB: $CORE_TABLE_COUNT tables"
log "  $LOGS_DB: $LOGS_TABLE_COUNT tables"
log "  All log tables confirmed absent from core, present in logs."

echo ""
echo "================================================================"
echo "Migration complete."
echo ""
echo "  Source:  $SOURCE_DB (UNTOUCHED — kept for rollback)"
echo "  Core:    $CORE_DB  — application state, config, benchmark runs"
echo "  Logs:    $LOGS_DB  — benchmark_request_progress, perf_log"
echo ""
echo "Next steps:"
echo "  1. Update DASHBOARD_DB_URL env var to point to $CORE_DB"
echo "  2. Update LOGS_DB_URL env var to point to $LOGS_DB"
echo "  3. Restart networker-dashboard and networker-agent"
echo "  4. Verify health: curl http://localhost:3000/api/version"
echo "  5. Run retention-cleanup.sh once to set baseline"
echo "  6. After confirming production health, DROP DATABASE $SOURCE_DB"
echo "================================================================"
