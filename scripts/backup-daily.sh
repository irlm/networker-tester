#!/usr/bin/env bash
# ==============================================================================
# backup-daily.sh — nightly PostgreSQL + host-config backup to Azure Blob
#
# WHY THIS SCRIPT IS SHAPED THE WAY IT IS
# ---------------------------------------
# On 2026-08-24 the prod VM was found to have been backing up the WRONG DATABASE
# for five months. Its ad-hoc /opt/backups/daily-backup.sh ran:
#
#     pg_dump alethedash > database.sql 2>/dev/null || \
#     pg_dump dashboard  > database.sql 2>/dev/null || true
#
# `alethedash` is the RETIRED Rust dashboard's database, abandoned at the C#
# cutover on 2026-03-31. It still exists, so pg_dump exited 0 and the backup
# looked healthy every single night — 69 blobs, "Backup complete" in the log —
# while the live database (alethedash_core, since renamed networker_core) was never
# captured. A backup that reports success while protecting nothing is worse than
# no backup, because it removes the pressure to notice.
#
# Three rules follow from that, and they are the whole design:
#
#   1. NEVER hardcode the database name. The primary database is READ FROM THE
#      LIVE SERVICE CONFIG (DASHBOARD_DB_URL_NPGSQL), so it cannot drift from
#      what the control plane is actually using. If that cannot be determined,
#      this script fails instead of guessing.
#   2. Back up EVERY database on the server, not a named subset. A database that
#      nobody remembered is exactly the one that gets missed.
#   3. VERIFY every dump and FAIL LOUDLY. No `2>/dev/null`, no `|| true` on the
#      backup path. A dump is only a backup once pg_restore can list it.
#
# Auth uses the VM's managed identity (`az login --identity`) — there is no
# storage key in this file. The blob layout is role-based (`core`, `logs`), not
# brand-based, so a rebrand or a domain change never invalidates it; the real
# source database name travels as blob metadata instead.
#
# Retention is server-side (storage-account lifecycle policy: db/daily 30 days,
# db/monthly 365 days) plus a local prune. The monthly copy is written on the
# 1st of each month.
#
# Env (all optional except where noted):
#   BACKUP_ENV_FILE        /etc/alethedash-cs.env   — source of truth for the DB
#   BACKUP_STORAGE_ACCOUNT alethedashbackups
#   BACKUP_CONTAINER       backups
#   BACKUP_LOCAL_DIR       /opt/backups
#   BACKUP_RETAIN_DAYS     30
#   BACKUP_MIN_BYTES       1000     — absolute floor; the real check is table coverage
#   BACKUP_SKIP_UPLOAD     unset|1  — dump + verify only (used by tests)
#
# Usage:  sudo ./backup-daily.sh
# ==============================================================================
set -euo pipefail

ENV_FILE="${BACKUP_ENV_FILE:-/etc/alethedash-cs.env}"
STORAGE_ACCOUNT="${BACKUP_STORAGE_ACCOUNT:-alethedashbackups}"
CONTAINER="${BACKUP_CONTAINER:-backups}"
LOCAL_DIR="${BACKUP_LOCAL_DIR:-/opt/backups}"
RETAIN_DAYS="${BACKUP_RETAIN_DAYS:-30}"
MIN_BYTES="${BACKUP_MIN_BYTES:-1000}"

NOW_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
DATE_UTC="$(date -u +%Y-%m-%d)"
MONTH_UTC="$(date -u +%Y-%m)"
DAY_OF_MONTH="$(date -u +%d)"

log()  { echo "[$(date -u +%H:%M:%SZ)] $*"; }
die()  { echo "BACKUP FAILED: $*" >&2; exit 1; }

# psql/pg_dump as the postgres superuser when we are root (peer auth — no
# password anywhere); otherwise assume the caller already has a working login.
if [ "$(id -u)" -eq 0 ]; then
    PG_RUN=(sudo -u postgres)
else
    PG_RUN=()
fi
pg_psql()    { "${PG_RUN[@]}" psql "$@"; }
pg_dump_run() { "${PG_RUN[@]}" pg_dump "$@"; }

# ── 1. The primary database, read from the LIVE service config ────────────────
# This is rule 1. Everything else can be discovered; this one must be derived,
# because getting it wrong is the failure this script exists to prevent.
[ -r "$ENV_FILE" ] || die "cannot read $ENV_FILE — refusing to guess the database name"

CONN="$(sed -n 's/^DASHBOARD_DB_URL_NPGSQL=//p' "$ENV_FILE" | head -1)"
CONN="${CONN%\"}"; CONN="${CONN#\"}"
[ -n "$CONN" ] || die "DASHBOARD_DB_URL_NPGSQL not found in $ENV_FILE"

PRIMARY_DB="$(printf '%s' "$CONN" | tr ';' '\n' | sed -n 's/^ *[Dd]atabase *= *//p' | head -1)"
[ -n "$PRIMARY_DB" ] || die "no Database= key in DASHBOARD_DB_URL_NPGSQL"
log "primary database (from $ENV_FILE): $PRIMARY_DB"

# It must actually exist. The old script's bug was dumping a database that
# existed but was not the live one; this asserts the live one is reachable.
if ! pg_psql -tAc "SELECT 1 FROM pg_database WHERE datname='${PRIMARY_DB}'" | grep -q 1; then
    die "primary database '${PRIMARY_DB}' does not exist on this server"
fi

# ── 2. Every database on the server ───────────────────────────────────────────
mapfile -t ALL_DBS < <(pg_psql -tAc \
    "SELECT datname FROM pg_database WHERE datistemplate=false AND datname<>'postgres' ORDER BY datname")
[ "${#ALL_DBS[@]}" -gt 0 ] || die "no databases found — is PostgreSQL running?"
log "databases to back up: ${ALL_DBS[*]}"

# Role name = what a database is FOR, never what it is branded. Keeps the blob
# layout stable across rebrands and database renames.
#
# The brand prefix is DERIVED, not hardcoded: the primary database is named
# <brand>_<role> (networker_core), so everything before the first underscore is
# this deployment's brand. Stripping it turns <brand>_logs into "logs" and a
# bare <brand> into "legacy" — and when the product IS renamed, the same code
# keeps producing the same brand-free blob names with no edit. Proven on
# 2026-08-24: alethedash_core was renamed networker_core and this script picked
# it up with zero changes, still writing db/daily/core-<date>.dump.
BRAND_PREFIX="${PRIMARY_DB%%_*}"

role_for() {
    local db="$1" rest
    if [ "$db" = "$PRIMARY_DB" ]; then echo "core"; return; fi
    rest="${db#"$BRAND_PREFIX"}"
    rest="${rest#_}"
    case "$rest" in
        "")        echo "legacy" ;;          # the bare pre-cutover database
        log|logs)  echo "logs" ;;
        *)         echo "${rest//[^a-zA-Z0-9]/-}" ;;
    esac
}

WORK_DIR="$(mktemp -d)"
cleanup() { rm -rf "$WORK_DIR"; }
trap cleanup EXIT

mkdir -p "$LOCAL_DIR"
# root:postgres 0750, NOT root:root 0700. pg_restore runs as the postgres user,
# and a 0700 root-owned directory blocks it at TRAVERSAL — the files inside can
# be owned by postgres and it still gets "Permission denied". 0750 with the
# postgres group keeps the directory closed to everyone else.
if [ "$(id -u)" -eq 0 ] && getent group postgres >/dev/null 2>&1; then
    chown root:postgres "$LOCAL_DIR" 2>/dev/null || true
    chmod 750 "$LOCAL_DIR"
else
    chmod 700 "$LOCAL_DIR"
fi

# ── 3. Dump + verify each database ────────────────────────────────────────────
declare -a MANIFEST=()
PRIMARY_OK=0

for db in "${ALL_DBS[@]}"; do
    role="$(role_for "$db")"
    out="${WORK_DIR}/${role}-${DATE_UTC}.dump"

    log "dumping ${db} (role=${role}) ..."
    # Write via STDOUT, not pg_dump -f: when this runs as root, pg_dump runs as
    # the postgres user and cannot create files inside root's mktemp -d (mode
    # 700). Redirecting means the file is created by the caller, so the dump
    # works the same whether invoked as root from cron or as a normal user.
    if ! pg_dump_run -Fc -Z6 -d "$db" > "$out"; then
        if [ "$db" = "$PRIMARY_DB" ]; then
            die "pg_dump of the PRIMARY database '${db}' failed"
        fi
        log "  WARNING: dump of secondary database '${db}' failed — continuing"
        continue
    fi
    chmod 600 "$out"

    # A file is not a backup until pg_restore can read it back.
    if ! pg_restore -l "$out" > "${out}.toc" 2>/dev/null; then
        if [ "$db" = "$PRIMARY_DB" ]; then
            die "dump of PRIMARY '${db}' is not a readable pg_dump archive"
        fi
        log "  WARNING: dump of '${db}' failed verification — continuing"
        continue
    fi

    size="$(stat -c%s "$out")"
    entries="$(grep -c '^[0-9;]' "${out}.toc" || true)"
    sha="$(sha256sum "$out" | cut -d' ' -f1)"

    if [ "$db" = "$PRIMARY_DB" ]; then
        [ "$size" -ge "$MIN_BYTES" ] \
            || die "PRIMARY dump is only ${size} bytes (floor ${MIN_BYTES}) — refusing to call this a backup"
        [ "$entries" -gt 0 ] \
            || die "PRIMARY dump has an empty table of contents"

        # The real gate. Bytes are a weak signal: a compressed dump's size swings
        # by an order of magnitude with content, so a floor either passes empty
        # dumps or fails legitimate small ones (both seen while testing this).
        # Comparing the archive's TABLE DATA entries against the live table count
        # is compression-independent and answers the question that actually
        # matters — did we capture THIS database, all of it?
        src_tables="$(pg_psql -d "$db" -tAc \
            "SELECT count(*) FROM information_schema.tables
              WHERE table_schema NOT IN ('pg_catalog','information_schema')
                AND table_type='BASE TABLE'")"
        toc_tables="$(grep -c 'TABLE DATA' "${out}.toc" || true)"
        [ "$toc_tables" -ge "$src_tables" ] \
            || die "dump carries ${toc_tables} TABLE DATA entries but '${db}' has ${src_tables} tables — incomplete"
        log "  verified: ${toc_tables} table-data entries cover ${src_tables} live tables"
        PRIMARY_OK=1
    fi

    log "  ok: ${size} bytes, ${entries} TOC entries"
    MANIFEST+=("{\"role\":\"${role}\",\"database\":\"${db}\",\"bytes\":${size},\"toc_entries\":${entries},\"sha256\":\"${sha}\"}")
done

[ "$PRIMARY_OK" -eq 1 ] || die "the primary database was never successfully dumped"

# ── 4. Host config — restore needs the secrets, not just the rows ─────────────
# DASHBOARD_CREDENTIAL_KEY encrypts stored cloud credentials at rest. Restoring
# the database WITHOUT this file leaves every stored credential undecryptable,
# so the config is part of the backup, not a nice-to-have.
CONFIG_TAR="${WORK_DIR}/host-${DATE_UTC}.tar.gz"
tar czf "$CONFIG_TAR" \
    --ignore-failed-read \
    -C / \
    etc/alethedash-cs.env \
    etc/systemd/system/alethedash-cs.service \
    etc/nginx/sites-available \
    2>/dev/null || log "WARNING: some host-config paths were missing"
# Defensive: the database dumps are already verified at this point, so a failed
# config bundle must not abort the run under `set -e`.
if [ -f "$CONFIG_TAR" ]; then
    chmod 600 "$CONFIG_TAR"
    log "host config bundle: $(stat -c%s "$CONFIG_TAR") bytes"
else
    log "WARNING: no host-config bundle was produced"
fi

# ── 5. Upload via managed identity (no storage key in this file) ──────────────
if [ "${BACKUP_SKIP_UPLOAD:-}" = "1" ]; then
    log "BACKUP_SKIP_UPLOAD=1 — skipping upload"
else
    command -v az >/dev/null 2>&1 || die "az CLI not found; cannot upload"
    export AZURE_CONFIG_DIR="${AZURE_CONFIG_DIR:-/root/.azure}"
    az login --identity --only-show-errors >/dev/null 2>&1 \
        || die "az login --identity failed; the VM identity needs 'Storage Blob Data Contributor' on ${STORAGE_ACCOUNT}"

    upload() { # <file> <blobpath> <sourcedb>
        local file="$1" path="$2" src="$3" attempt
        for attempt in 1 2 3; do
            if az storage blob upload \
                    --account-name "$STORAGE_ACCOUNT" --auth-mode login \
                    --container-name "$CONTAINER" --name "$path" \
                    --file "$file" --overwrite \
                    --metadata "sourcedb=${src}" "takenutc=${NOW_UTC}" "kind=pg_dump-custom" \
                    --only-show-errors >/dev/null 2>&1; then
                log "  uploaded ${path}"
                return 0
            fi
            sleep $(( attempt * 10 ))
        done
        return 1
    }

    for db in "${ALL_DBS[@]}"; do
        role="$(role_for "$db")"
        f="${WORK_DIR}/${role}-${DATE_UTC}.dump"
        [ -f "$f" ] || continue
        upload "$f" "db/daily/${role}-${DATE_UTC}.dump" "$db" \
            || { [ "$db" = "$PRIMARY_DB" ] && die "upload of the PRIMARY dump failed"; }
        if [ "$DAY_OF_MONTH" = "01" ]; then
            upload "$f" "db/monthly/${role}-${MONTH_UTC}.dump" "$db" \
                || log "  WARNING: monthly archive upload failed for ${role}"
        fi
    done

    if [ -f "$CONFIG_TAR" ]; then
        upload "$CONFIG_TAR" "config/daily/host-${DATE_UTC}.tar.gz" "host-config" \
            || log "WARNING: host-config upload failed"
        if [ "$DAY_OF_MONTH" = "01" ]; then
            upload "$CONFIG_TAR" "config/monthly/host-${MONTH_UTC}.tar.gz" "host-config" \
                || log "WARNING: monthly host-config upload failed"
        fi
    fi

    # Marker: what a monitor should read to answer "when did a backup last work?"
    MARKER="${WORK_DIR}/last_backup.json"
    {
        printf '{\n  "timestamp": "%s",\n  "date": "%s",\n  "primary_database": "%s",\n  "artifacts": [\n    ' \
            "$NOW_UTC" "$DATE_UTC" "$PRIMARY_DB"
        local_sep=""
        for row in "${MANIFEST[@]}"; do printf '%s%s' "$local_sep" "$row"; local_sep=$',\n    '; done
        printf '\n  ]\n}\n'
    } > "$MARKER"
    upload "$MARKER" "last_backup.json" "marker" || log "WARNING: marker upload failed"
fi

# ── 6. Keep a local copy, prune old ones ──────────────────────────────────────
cp -f "${WORK_DIR}"/*.dump "$LOCAL_DIR"/ 2>/dev/null || true
[ -f "$CONFIG_TAR" ] && cp -f "$CONFIG_TAR" "$LOCAL_DIR"/ 2>/dev/null || true

# The local dumps must be readable by the postgres user, because that is who
# runs pg_restore. Copied as root they land root:root 0600, and every restore —
# including the drill in docs/backup-and-retention.md — fails with
# "could not open input file: Permission denied". Found while verifying the
# legacy dumps before dropping those databases; the archives were fine, the
# permissions were not. 0600 as postgres is just as private as 0600 as root.
if [ "$(id -u)" -eq 0 ]; then
    chown postgres:postgres "$LOCAL_DIR"/*.dump 2>/dev/null || true
    chmod 600 "$LOCAL_DIR"/*.dump 2>/dev/null || true
fi
find "$LOCAL_DIR" -maxdepth 1 -name '*.dump'    -mtime "+${RETAIN_DAYS}" -delete
find "$LOCAL_DIR" -maxdepth 1 -name 'host-*.tar.gz' -mtime "+${RETAIN_DAYS}" -delete

log "backup complete — primary '${PRIMARY_DB}' verified, ${#MANIFEST[@]} database(s) captured"
