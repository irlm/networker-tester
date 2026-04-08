#!/usr/bin/env bash
# ==============================================================================
# backup-daily.sh — Daily pg_dump to Azure Blob Storage
#
# Dumps networker_core (or configured DB), gzips it, uploads via azcopy.
# On the 1st of the month, also archives a monthly copy.
# Writes a last_backup.json marker to the blob container root.
#
# Required env:
#   BACKUP_BLOB_URL    — SAS URL to blob container root (no trailing slash)
#                        e.g. https://account.blob.core.windows.net/backups?sv=...
#
# Optional env (with defaults):
#   BACKUP_DB_HOST     — localhost
#   BACKUP_DB_USER     — backup_user
#   BACKUP_DB_PORT     — 5432
#   BACKUP_DB_NAME     — networker_core
#   BACKUP_RETAIN_DAYS — 30
#
# Usage (typically from cron):
#   BACKUP_BLOB_URL="https://..." ./scripts/backup-daily.sh
# ==============================================================================
set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────────
BACKUP_DB_HOST="${BACKUP_DB_HOST:-localhost}"
BACKUP_DB_USER="${BACKUP_DB_USER:-backup_user}"
BACKUP_DB_PORT="${BACKUP_DB_PORT:-5432}"
BACKUP_DB_NAME="${BACKUP_DB_NAME:-networker_core}"
BACKUP_RETAIN_DAYS="${BACKUP_RETAIN_DAYS:-30}"

if [ -z "${BACKUP_BLOB_URL:-}" ]; then
    echo "ERROR: BACKUP_BLOB_URL is required." >&2
    exit 1
fi

# Strip trailing slash from blob URL
BACKUP_BLOB_URL="${BACKUP_BLOB_URL%/}"

# ── Timestamps ────────────────────────────────────────────────────────────────
NOW_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)
DATE_UTC=$(date -u +%Y-%m-%d)
MONTH_UTC=$(date -u +%Y-%m)
DAY_OF_MONTH=$(date -u +%d)

log() { echo "[$NOW_UTC] $*"; }
die() { echo "ERROR: $*" >&2; exit 1; }

# ── Temp workspace ────────────────────────────────────────────────────────────
WORK_DIR=$(mktemp -d)
DUMP_FILE="${WORK_DIR}/${DATE_UTC}.sql.gz"
MARKER_FILE="${WORK_DIR}/last_backup.json"

cleanup() {
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

# ── Decompose blob URL into base + SAS suffix ─────────────────────────────────
# e.g. https://account.blob.core.windows.net/container?sv=...&sig=...
# BLOB_BASE = everything before '?'; BLOB_SAS = '?' + query string (or empty)
BLOB_BASE="${BACKUP_BLOB_URL%%\?*}"
if echo "$BACKUP_BLOB_URL" | grep -q '?'; then
    BLOB_SAS="?${BACKUP_BLOB_URL#*\?}"
else
    BLOB_SAS=""
fi

blob_url() {
    # $1 = relative path within the container (e.g. daily/2026-04-07.sql.gz)
    echo "${BLOB_BASE}/$1${BLOB_SAS}"
}

# ── Step 1: pg_dump -Fc | gzip → temp file ────────────────────────────────────
log "Dumping '$BACKUP_DB_NAME' from $BACKUP_DB_HOST:$BACKUP_DB_PORT..."
pg_dump \
    -h "$BACKUP_DB_HOST" \
    -U "$BACKUP_DB_USER" \
    -p "$BACKUP_DB_PORT" \
    -Fc \
    -d "$BACKUP_DB_NAME" \
    < /dev/null \
    | gzip > "$DUMP_FILE"

log "Dump complete."

# ── Step 2: Compute size and SHA-256 checksum ──────────────────────────────────
# stat syntax differs between Linux (-c%s) and macOS (-f%z); try both.
if stat -c%s "$DUMP_FILE" > /dev/null 2>&1; then
    FILE_SIZE=$(stat -c%s "$DUMP_FILE")
else
    FILE_SIZE=$(stat -f%z "$DUMP_FILE")
fi

CHECKSUM_SHA256=$(shasum -a 256 "$DUMP_FILE" | cut -d' ' -f1)
log "Size: ${FILE_SIZE} bytes, SHA256: ${CHECKSUM_SHA256}"

# ── Step 3: Upload daily backup ───────────────────────────────────────────────
DAILY_PATH="daily/${DATE_UTC}.sql.gz"
log "Uploading to ${DAILY_PATH}..."
azcopy copy \
    "$DUMP_FILE" \
    "$(blob_url "$DAILY_PATH")" \
    --overwrite true \
    < /dev/null
log "Daily backup uploaded."

# ── Step 4: Monthly archive on 1st of month ──────────────────────────────────
if [ "$DAY_OF_MONTH" = "01" ]; then
    MONTHLY_PATH="monthly/${MONTH_UTC}.sql.gz"
    log "1st of month — archiving to ${MONTHLY_PATH}..."
    azcopy copy \
        "$DUMP_FILE" \
        "$(blob_url "$MONTHLY_PATH")" \
        --overwrite true \
        < /dev/null
    log "Monthly archive uploaded."
fi

# ── Step 5: Write and upload last_backup.json marker ─────────────────────────
log "Writing last_backup.json marker..."
cat > "$MARKER_FILE" << EOF
{
  "timestamp": "${NOW_UTC}",
  "date": "${DATE_UTC}",
  "database": "${BACKUP_DB_NAME}",
  "size_bytes": ${FILE_SIZE},
  "checksum_sha256": "${CHECKSUM_SHA256}",
  "blob_path": "${DAILY_PATH}"
}
EOF

azcopy copy \
    "$MARKER_FILE" \
    "$(blob_url "last_backup.json")" \
    --overwrite true \
    < /dev/null
log "Marker uploaded."

# ── Step 6: Prune daily backups older than BACKUP_RETAIN_DAYS ────────────────
log "Removing daily backups older than ${BACKUP_RETAIN_DAYS} days..."

# Compute cutoff date string (GNU date vs BSD date)
if date -u -d "-${BACKUP_RETAIN_DAYS} days" +%Y-%m-%d > /dev/null 2>&1; then
    CUTOFF_DATE=$(date -u -d "-${BACKUP_RETAIN_DAYS} days" +%Y-%m-%d)
else
    CUTOFF_DATE=$(date -u -v"-${BACKUP_RETAIN_DAYS}d" +%Y-%m-%d)
fi
log "Cutoff date: $CUTOFF_DATE"

# List blobs in daily/ and delete those with a date before the cutoff.
# azcopy list output lines look like:
#   INFO: daily/2026-01-01.sql.gz; Content Length: 12345678
azcopy list \
    "$(blob_url "daily/")" \
    < /dev/null 2>/dev/null \
    | grep '\.sql\.gz' \
    | while IFS= read -r list_line; do
        # Extract just the filename, e.g. "2026-01-01.sql.gz"
        blob_name=$(echo "$list_line" | sed 's|.*daily/||' | cut -d';' -f1 | tr -d ' ')
        blob_date="${blob_name%.sql.gz}"
        # String comparison works for ISO dates (YYYY-MM-DD)
        if [ "$blob_date" \< "$CUTOFF_DATE" ]; then
            log "  Deleting expired backup: $blob_name"
            azcopy remove \
                "$(blob_url "daily/${blob_name}")" \
                < /dev/null || true
        fi
    done

echo ""
log "Backup complete: $DAILY_PATH (${FILE_SIZE} bytes, SHA256: ${CHECKSUM_SHA256})"
