#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# AletheDash Dashboard — Backup
#
# Creates a compressed backup of:
#   - PostgreSQL database (full dump)
#   - Systemd service config (env vars, secrets)
#   - Nginx config + SSL certificates
#   - Dashboard version info
#
# Usage:
#   # Remote backup (from your machine)
#   ./scripts/backup-dashboard.sh --host 20.42.8.158 --output ./backups/
#
#   # Local backup (on the VM itself)
#   ./scripts/backup-dashboard.sh --local --output /opt/backups/
#
#   # Automated daily backup to Azure Blob Storage
#   ./scripts/backup-dashboard.sh --host 20.42.8.158 --blob-container backups --storage-account alethedashstorage
#
# Restore:
#   ./scripts/restore-dashboard.sh --backup ./backups/alethedash-2026-03-21.tar.gz --domain alethedash.com
# ============================================================================

HOST=""
OUTPUT_DIR="./backups"
LOCAL=false
BLOB_CONTAINER=""
STORAGE_ACCOUNT=""
RG=""
VM_NAME=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --host)             HOST="$2"; shift 2 ;;
        --output)           OUTPUT_DIR="$2"; shift 2 ;;
        --local)            LOCAL=true; shift ;;
        --blob-container)   BLOB_CONTAINER="$2"; shift 2 ;;
        --storage-account)  STORAGE_ACCOUNT="$2"; shift 2 ;;
        --rg)               RG="$2"; shift 2 ;;
        --vm)               VM_NAME="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 --host <ip> [--output <dir>] [--blob-container <name> --storage-account <name>]"
            exit 0 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

TIMESTAMP=$(date -u +%Y-%m-%d-%H%M%S)
BACKUP_NAME="dashboard-backup-${TIMESTAMP}"

if [[ "$LOCAL" == true ]]; then
    # ── Running on the VM itself ─────────────────────────────────────────
    echo "▸ Creating local backup..."
    TMPDIR=$(mktemp -d)
    BACKUP_DIR="${TMPDIR}/${BACKUP_NAME}"
    mkdir -p "$BACKUP_DIR"

    # Database dump
    echo "  Dumping PostgreSQL..."
    sudo -u postgres pg_dump dashboard > "${BACKUP_DIR}/database.sql"

    # Service config (contains env vars / secrets)
    echo "  Saving service config..."
    sudo cp /etc/systemd/system/dashboard.service "${BACKUP_DIR}/dashboard.service" 2>/dev/null || \
    sudo cp /etc/systemd/system/alethedash.service "${BACKUP_DIR}/dashboard.service" 2>/dev/null || true

    # Nginx config
    echo "  Saving nginx config..."
    sudo cp -r /etc/nginx/sites-available/ "${BACKUP_DIR}/nginx-sites/" 2>/dev/null || true

    # SSL certificates
    echo "  Saving SSL certificates..."
    sudo cp -rL /etc/letsencrypt/live/ "${BACKUP_DIR}/ssl-live/" 2>/dev/null || true
    sudo cp -rL /etc/letsencrypt/renewal/ "${BACKUP_DIR}/ssl-renewal/" 2>/dev/null || true

    # Dashboard version
    echo "  Saving version info..."
    /opt/dashboard/networker-dashboard --version > "${BACKUP_DIR}/version.txt" 2>/dev/null || \
    /opt/alethedash/networker-dashboard --version > "${BACKUP_DIR}/version.txt" 2>/dev/null || \
    echo "unknown" > "${BACKUP_DIR}/version.txt"

    # Metadata
    cat > "${BACKUP_DIR}/metadata.json" << EOF
{
  "timestamp": "${TIMESTAMP}",
  "hostname": "$(hostname)",
  "ip": "$(curl -s ifconfig.me 2>/dev/null || echo 'unknown')",
  "db_size": "$(sudo -u postgres psql -d dashboard -c "SELECT pg_size_pretty(pg_database_size('dashboard'));" -t 2>/dev/null || echo 'unknown')",
  "users": $(sudo -u postgres psql -d dashboard -c "SELECT COUNT(*) FROM dash_user;" -t 2>/dev/null || echo '0'),
  "runs": $(sudo -u postgres psql -d dashboard -c "SELECT COUNT(*) FROM testrun;" -t 2>/dev/null || echo '0')
}
EOF

    # Compress
    mkdir -p "$OUTPUT_DIR"
    tar czf "${OUTPUT_DIR}/${BACKUP_NAME}.tar.gz" -C "$TMPDIR" "$BACKUP_NAME"
    rm -rf "$TMPDIR"

    echo "✓ Backup saved to: ${OUTPUT_DIR}/${BACKUP_NAME}.tar.gz"

elif [[ -n "$HOST" ]]; then
    # ── Remote backup via SSH ────────────────────────────────────────────
    echo "▸ Creating remote backup from $HOST..."
    mkdir -p "$OUTPUT_DIR"

    # Run backup on remote, download the result
    ssh -o StrictHostKeyChecking=no "azureuser@${HOST}" "
        sudo bash -c '
            set -e
            TMPDIR=\$(mktemp -d)
            BACKUP_DIR=\"\${TMPDIR}/${BACKUP_NAME}\"
            mkdir -p \"\$BACKUP_DIR\"

            # Database
            sudo -u postgres pg_dump dashboard > \"\${BACKUP_DIR}/database.sql\" 2>/dev/null || \
            sudo -u postgres pg_dump alethedash > \"\${BACKUP_DIR}/database.sql\" 2>/dev/null || true

            # Service config
            cp /etc/systemd/system/dashboard.service \"\${BACKUP_DIR}/\" 2>/dev/null || \
            cp /etc/systemd/system/alethedash.service \"\${BACKUP_DIR}/dashboard.service\" 2>/dev/null || true

            # Nginx
            cp -r /etc/nginx/sites-available/ \"\${BACKUP_DIR}/nginx-sites/\" 2>/dev/null || true

            # SSL
            cp -rL /etc/letsencrypt/live/ \"\${BACKUP_DIR}/ssl-live/\" 2>/dev/null || true
            cp -rL /etc/letsencrypt/renewal/ \"\${BACKUP_DIR}/ssl-renewal/\" 2>/dev/null || true

            # Compress
            tar czf \"/tmp/${BACKUP_NAME}.tar.gz\" -C \"\$TMPDIR\" \"${BACKUP_NAME}\"
            rm -rf \"\$TMPDIR\"
            echo \"BACKUP_READY\"
        '
    "

    scp -o StrictHostKeyChecking=no "azureuser@${HOST}:/tmp/${BACKUP_NAME}.tar.gz" "${OUTPUT_DIR}/"
    ssh -o StrictHostKeyChecking=no "azureuser@${HOST}" "rm -f /tmp/${BACKUP_NAME}.tar.gz"

    echo "✓ Backup saved to: ${OUTPUT_DIR}/${BACKUP_NAME}.tar.gz"

elif [[ -n "$RG" && -n "$VM_NAME" ]]; then
    # ── Remote backup via az vm run-command ───────────────────────────────
    echo "▸ Creating backup via az vm run-command..."
    mkdir -p "$OUTPUT_DIR"

    az vm run-command invoke \
        --resource-group "$RG" \
        --name "$VM_NAME" \
        --command-id RunShellScript \
        --scripts "
            set -e
            TMPDIR=\$(mktemp -d)
            BACKUP_DIR=\"\${TMPDIR}/${BACKUP_NAME}\"
            mkdir -p \"\$BACKUP_DIR\"
            sudo -u postgres pg_dump dashboard > \"\${BACKUP_DIR}/database.sql\" 2>/dev/null || \
            sudo -u postgres pg_dump alethedash > \"\${BACKUP_DIR}/database.sql\" 2>/dev/null || true
            cp /etc/systemd/system/dashboard.service \"\${BACKUP_DIR}/\" 2>/dev/null || \
            cp /etc/systemd/system/alethedash.service \"\${BACKUP_DIR}/dashboard.service\" 2>/dev/null || true
            cp -r /etc/nginx/sites-available/ \"\${BACKUP_DIR}/nginx-sites/\" 2>/dev/null || true
            tar czf \"/tmp/${BACKUP_NAME}.tar.gz\" -C \"\$TMPDIR\" \"${BACKUP_NAME}\"
            rm -rf \"\$TMPDIR\"
            echo 'BACKUP_READY'
        " -o none

    # Download via SCP (need VM IP)
    VM_IP=$(az vm show -g "$RG" -n "$VM_NAME" -d --query publicIps -o tsv)
    scp -o StrictHostKeyChecking=no "azureuser@${VM_IP}:/tmp/${BACKUP_NAME}.tar.gz" "${OUTPUT_DIR}/"

    echo "✓ Backup saved to: ${OUTPUT_DIR}/${BACKUP_NAME}.tar.gz"
else
    echo "ERROR: Specify --host <ip>, --local, or --rg <rg> --vm <vm>"
    exit 1
fi

# ── Optional: Upload to Azure Blob Storage ───────────────────────────────────
if [[ -n "$BLOB_CONTAINER" && -n "$STORAGE_ACCOUNT" ]]; then
    echo "▸ Uploading to Azure Blob Storage..."
    az storage blob upload \
        --account-name "$STORAGE_ACCOUNT" \
        --container-name "$BLOB_CONTAINER" \
        --file "${OUTPUT_DIR}/${BACKUP_NAME}.tar.gz" \
        --name "${BACKUP_NAME}.tar.gz" \
        --auth-mode login \
        -o none 2>&1
    echo "✓ Uploaded to: ${STORAGE_ACCOUNT}/${BLOB_CONTAINER}/${BACKUP_NAME}.tar.gz"
fi

echo ""
echo "Backup contents:"
echo "  database.sql        — Full PostgreSQL dump (users, jobs, runs, schedules)"
echo "  dashboard.service   — Systemd config with all env vars and secrets"
echo "  nginx-sites/        — Nginx virtual host configs"
echo "  ssl-live/           — Let's Encrypt certificates"
echo "  ssl-renewal/        — Certbot renewal configs"
