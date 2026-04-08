#!/usr/bin/env bash
# ==============================================================================
# retention-cleanup.sh — 7-day log retention with batched deletes
#
# Deletes rows older than RETAIN_DAYS from the networker_logs database:
#   - benchmark_request_progress  (WHERE created_at < threshold)
#   - perf_log                    (WHERE logged_at < threshold)
#
# Uses batched deletes (ctid subquery + RETURNING) to avoid long-running
# transactions and reduce lock pressure. Sleeps 1 second between batches.
# Runs VACUUM ANALYZE after each table to reclaim space.
#
# Optional env (with defaults):
#   LOGS_DB_HOST  — localhost
#   LOGS_DB_USER  — networker
#   LOGS_DB_PORT  — 5432
#   LOGS_DB_NAME  — networker_logs
#   RETAIN_DAYS   — 7
#   BATCH_SIZE    — 10000
#
# Usage:
#   ./scripts/retention-cleanup.sh
#   RETAIN_DAYS=14 ./scripts/retention-cleanup.sh
# ==============================================================================
set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────────
LOGS_DB_HOST="${LOGS_DB_HOST:-localhost}"
LOGS_DB_USER="${LOGS_DB_USER:-networker}"
LOGS_DB_PORT="${LOGS_DB_PORT:-5432}"
LOGS_DB_NAME="${LOGS_DB_NAME:-networker_logs}"
RETAIN_DAYS="${RETAIN_DAYS:-7}"
BATCH_SIZE="${BATCH_SIZE:-10000}"

# Validate numeric inputs to prevent SQL injection
case "$RETAIN_DAYS" in
    ''|*[!0-9]*) echo "ERROR: RETAIN_DAYS must be a positive integer, got '$RETAIN_DAYS'"; exit 1 ;;
esac
case "$BATCH_SIZE" in
    ''|*[!0-9]*) echo "ERROR: BATCH_SIZE must be a positive integer, got '$BATCH_SIZE'"; exit 1 ;;
esac

NOW_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)
log() { echo "[$NOW_UTC] $*"; }

# ── Wrapper: run psql non-interactively ───────────────────────────────────────
run_psql() {
    psql \
        -h "$LOGS_DB_HOST" \
        -U "$LOGS_DB_USER" \
        -p "$LOGS_DB_PORT" \
        -d "$LOGS_DB_NAME" \
        "$@" \
        < /dev/null
}

# ── Compute cutoff (as a Postgres interval expression) ───────────────────────
# Passed inline into SQL; not user-supplied, so safe from injection.
CUTOFF_EXPR="now() - interval '$RETAIN_DAYS days'"

log "Retention cleanup: RETAIN_DAYS=$RETAIN_DAYS, BATCH_SIZE=$BATCH_SIZE"
log "Cutoff: rows older than $RETAIN_DAYS days will be deleted."

# ── Batched delete function ───────────────────────────────────────────────────
# Usage: batched_delete <table> <timestamp_column>
# Returns (via echo) total rows deleted.
batched_delete() {
    _table="$1"
    _ts_col="$2"
    _total=0
    _batch=0

    log "Starting batched delete on ${_table} (${_ts_col} < cutoff)..."

    while true; do
        _batch=$((_batch + 1))

        # CTE delete: select up to BATCH_SIZE old rows by ctid (avoids seq scan
        # of the full table per batch), delete them, count via RETURNING.
        _deleted=$(
            run_psql -tAc "
                WITH removed AS (
                    DELETE FROM ${_table}
                    WHERE ctid IN (
                        SELECT ctid
                        FROM   ${_table}
                        WHERE  ${_ts_col} < ${CUTOFF_EXPR}
                        LIMIT  ${BATCH_SIZE}
                    )
                    RETURNING 1
                )
                SELECT COUNT(*) FROM removed;
            " 2>/dev/null \
            | tr -d '[:space:]' \
            || echo "0"
        )

        _total=$((_total + _deleted))
        log "  Batch ${_batch}: deleted ${_deleted} rows (running total: ${_total})"

        if [ "$_deleted" -eq 0 ]; then
            break
        fi

        sleep 1
    done

    log "${_table} done: ${_total} rows deleted."
    echo "$_total"
}

# ── Clean benchmark_request_progress ─────────────────────────────────────────
log "=== benchmark_request_progress ==="
BRP_DELETED=$(batched_delete "benchmark_request_progress" "created_at")

# ── Clean perf_log ────────────────────────────────────────────────────────────
log "=== perf_log ==="
PERF_DELETED=$(batched_delete "perf_log" "logged_at")

# ── VACUUM ANALYZE ────────────────────────────────────────────────────────────
log "VACUUM ANALYZE benchmark_request_progress..."
run_psql -c "VACUUM ANALYZE benchmark_request_progress;"

log "VACUUM ANALYZE perf_log..."
run_psql -c "VACUUM ANALYZE perf_log;"

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
log "Retention cleanup complete."
log "  benchmark_request_progress: ${BRP_DELETED} rows deleted"
log "  perf_log:                   ${PERF_DELETED} rows deleted"
log "  Retain threshold:           ${RETAIN_DAYS} days"
