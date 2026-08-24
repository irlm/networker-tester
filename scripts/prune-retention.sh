#!/usr/bin/env bash
# ==============================================================================
# prune-retention.sh — age out probe data and logs
#
# DRY RUN BY DEFAULT. Pass --apply to actually delete.
#
# Intended to run immediately after backup-daily.sh, chained with `&&` so that
# nothing is ever deleted unless a fresh, VERIFIED backup was just taken:
#
#     /usr/local/sbin/db-backup.sh && /usr/local/sbin/db-prune.sh --apply
#
# WHY THE DELETE ORDER IS EXPLICIT
# --------------------------------
# The canonical DDL (shared/tester-schema.postgres.sql) declares
# `RequestAttempt.RunId REFERENCES TestRun ... ON DELETE CASCADE`, but the LIVE
# production database has that constraint as NO ACTION (verified 2026-08-24 —
# the table predates the current DDL). Three of RequestAttempt's own children
# are NO ACTION too: errorrecord, mthroughputresult, servertimingresult.
#
# So "just delete the parent and let it cascade" does not work here — it fails
# outright with a foreign-key violation. This script deletes bottom-up, in
# dependency order, and does not rely on any cascade being configured:
#
#   1. NO ACTION children of requestattempt   (errorrecord, mthroughput, servertiming)
#   2. requestattempt                          (cascades dns/tcp/tls/http/udp results)
#   3. testrun rows that no longer have attempts
#   4. service_log / perf_log by their own timestamps
#
# Everything runs in ONE transaction: a partial prune that removed children but
# not parents would leave the database in a state no reader expects.
#
# Env:
#   RETENTION_RAW_DAYS   90   raw probe attempts; 0 disables
#   RETENTION_LOG_DAYS   90   service_log + perf_log; 0 disables
#   BACKUP_ENV_FILE      /etc/alethedash-cs.env
# ==============================================================================
set -euo pipefail

ENV_FILE="${BACKUP_ENV_FILE:-/etc/alethedash-cs.env}"
RAW_DAYS="${RETENTION_RAW_DAYS:-90}"
LOG_DAYS="${RETENTION_LOG_DAYS:-90}"
APPLY=0
[ "${1:-}" = "--apply" ] && APPLY=1

log() { echo "[$(date -u +%H:%M:%SZ)] $*"; }
die() { echo "PRUNE FAILED: $*" >&2; exit 1; }

[ -r "$ENV_FILE" ] || die "cannot read $ENV_FILE"
CONN="$(sed -n 's/^DASHBOARD_DB_URL_NPGSQL=//p' "$ENV_FILE" | head -1)"
CONN="${CONN%\"}"; CONN="${CONN#\"}"
[ -n "$CONN" ] || die "DASHBOARD_DB_URL_NPGSQL not found in $ENV_FILE"
field() { printf '%s' "$CONN" | tr ';' '\n' | sed -n "s/^ *$1 *= *//Ip" | head -1; }
DB="$(field 'database')";  HOST="$(field 'host')"
USER="$(field 'username')"; [ -n "$USER" ] || USER="$(field 'user id')"
PGPASSWORD="$(field 'password')"; export PGPASSWORD
[ -n "$DB" ] || die "no Database= in the connection string"

psql_run() { psql -h "${HOST:-127.0.0.1}" -U "$USER" -d "$DB" -v ON_ERROR_STOP=1 "$@"; }

log "database=${DB} raw_retention=${RAW_DAYS}d log_retention=${LOG_DAYS}d apply=${APPLY}"
[ "$RAW_DAYS" -eq 0 ] && [ "$LOG_DAYS" -eq 0 ] && { log "both retentions disabled — nothing to do"; exit 0; }

# ── What WOULD go (always shown, apply or not) ────────────────────────────────
log "counting..."
psql_run -q <<SQL
\pset pager off
SELECT 'requestattempt' AS table, count(*) AS expiring FROM requestattempt
 WHERE ${RAW_DAYS} > 0 AND startedat < now() - make_interval(days => ${RAW_DAYS})
UNION ALL
SELECT 'testrun (childless after prune)', count(*) FROM testrun t
 WHERE ${RAW_DAYS} > 0 AND NOT EXISTS (
   SELECT 1 FROM requestattempt a WHERE a.runid = t.runid
     AND a.startedat >= now() - make_interval(days => ${RAW_DAYS}))
UNION ALL
SELECT 'service_log', count(*) FROM service_log
 WHERE ${LOG_DAYS} > 0 AND ts < now() - make_interval(days => ${LOG_DAYS})
UNION ALL
SELECT 'perf_log', count(*) FROM perf_log
 WHERE ${LOG_DAYS} > 0 AND logged_at < now() - make_interval(days => ${LOG_DAYS});
SQL

if [ "$APPLY" -ne 1 ]; then
    log "DRY RUN — nothing deleted. Re-run with --apply to delete the rows above."
    exit 0
fi

# ── Apply, bottom-up, in one transaction ─────────────────────────────────────
log "applying..."
psql_run <<SQL
\pset pager off
BEGIN;

CREATE TEMP TABLE expiring_attempts ON COMMIT DROP AS
  SELECT attemptid FROM requestattempt
   WHERE ${RAW_DAYS} > 0 AND startedat < now() - make_interval(days => ${RAW_DAYS});

-- 1. children that do NOT cascade (a plain delete of the parent errors here)
DELETE FROM errorrecord        WHERE attemptid IN (SELECT attemptid FROM expiring_attempts);
DELETE FROM mthroughputresult  WHERE attemptid IN (SELECT attemptid FROM expiring_attempts);
DELETE FROM servertimingresult WHERE attemptid IN (SELECT attemptid FROM expiring_attempts);

-- 2. the attempts themselves; dns/tcp/tls/http/udp results cascade from here
DELETE FROM requestattempt     WHERE attemptid IN (SELECT attemptid FROM expiring_attempts);

-- 3. run rows left with no attempts at all
DELETE FROM testrun t
 WHERE ${RAW_DAYS} > 0
   AND NOT EXISTS (SELECT 1 FROM requestattempt a WHERE a.runid = t.runid);

-- 4. logs, by their own clocks
DELETE FROM service_log WHERE ${LOG_DAYS} > 0 AND ts        < now() - make_interval(days => ${LOG_DAYS});
DELETE FROM perf_log    WHERE ${LOG_DAYS} > 0 AND logged_at < now() - make_interval(days => ${LOG_DAYS});

COMMIT;
SQL

log "prune complete"
