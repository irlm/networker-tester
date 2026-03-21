#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# AletheDash Dashboard — Restore from Backup
#
# Full disaster recovery: creates a new VM and restores everything from backup.
#
# Usage:
#   # Full restore to a new VM (disaster recovery)
#   ./scripts/restore-dashboard.sh \
#     --backup ./backups/dashboard-backup-2026-03-21.tar.gz \
#     --domain alethedash.com \
#     --location eastus
#
#   # Restore to an existing VM (e.g., after data corruption)
#   ./scripts/restore-dashboard.sh \
#     --backup ./backups/dashboard-backup-2026-03-21.tar.gz \
#     --host 20.42.8.158
#
#   # Restore from Azure Blob Storage
#   ./scripts/restore-dashboard.sh \
#     --blob dashboard-backup-2026-03-21.tar.gz \
#     --storage-account alethedashstorage \
#     --blob-container backups \
#     --domain alethedash.com
#
# What it restores:
#   - PostgreSQL database (all users, jobs, runs, schedules, settings)
#   - Systemd service config (all env vars: JWT secret, SSO, ACS, DB password)
#   - Nginx config
#   - SSL certificates (if included in backup)
#   - Dashboard binary (downloaded from the release matching backup version)
# ============================================================================

BACKUP_FILE=""
DOMAIN=""
HOST=""
LOCATION="eastus"
BLOB_NAME=""
STORAGE_ACCOUNT=""
BLOB_CONTAINER=""

while [[ $# -gt 0 ]]; do
    case $1 in
        --backup)           BACKUP_FILE="$2"; shift 2 ;;
        --domain)           DOMAIN="$2"; shift 2 ;;
        --host)             HOST="$2"; shift 2 ;;
        --location)         LOCATION="$2"; shift 2 ;;
        --blob)             BLOB_NAME="$2"; shift 2 ;;
        --storage-account)  STORAGE_ACCOUNT="$2"; shift 2 ;;
        --blob-container)   BLOB_CONTAINER="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 --backup <file.tar.gz> --domain <domain> [--host <ip>] [--location <region>]"
            echo ""
            echo "  --backup    Path to backup tarball"
            echo "  --domain    Domain name (for new VM deployment)"
            echo "  --host      Restore to existing VM (skip VM creation)"
            echo "  --location  Azure region for new VM (default: eastus)"
            echo "  --blob      Download backup from Azure Blob Storage"
            exit 0 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ── Download from blob if specified ──────────────────────────────────────────
if [[ -n "$BLOB_NAME" && -n "$STORAGE_ACCOUNT" ]]; then
    BACKUP_FILE="/tmp/${BLOB_NAME}"
    echo "▸ Downloading backup from Azure Blob Storage..."
    az storage blob download \
        --account-name "$STORAGE_ACCOUNT" \
        --container-name "${BLOB_CONTAINER:-backups}" \
        --name "$BLOB_NAME" \
        --file "$BACKUP_FILE" \
        --auth-mode login \
        -o none
    echo "  Downloaded to: $BACKUP_FILE"
fi

# ── Validate ─────────────────────────────────────────────────────────────────
if [[ -z "$BACKUP_FILE" || ! -f "$BACKUP_FILE" ]]; then
    echo "ERROR: --backup <file> is required and must exist"
    exit 1
fi

if [[ -z "$HOST" && -z "$DOMAIN" ]]; then
    echo "ERROR: Specify --domain (create new VM) or --host (restore to existing VM)"
    exit 1
fi

# ── Extract backup to temp dir ───────────────────────────────────────────────
echo "▸ Extracting backup..."
RESTORE_TMP=$(mktemp -d)
tar xzf "$BACKUP_FILE" -C "$RESTORE_TMP"
BACKUP_DIR=$(find "$RESTORE_TMP" -maxdepth 1 -type d | tail -1)

# Check contents
if [[ ! -f "${BACKUP_DIR}/database.sql" ]]; then
    echo "ERROR: backup does not contain database.sql"
    rm -rf "$RESTORE_TMP"
    exit 1
fi

echo "  Database dump: $(wc -l < "${BACKUP_DIR}/database.sql") lines"
echo "  Service config: $([ -f "${BACKUP_DIR}/dashboard.service" ] && echo "present" || echo "missing")"
echo "  SSL certs: $([ -d "${BACKUP_DIR}/ssl-live" ] && echo "present" || echo "missing")"

# ── Create new VM if --domain specified ──────────────────────────────────────
if [[ -z "$HOST" ]]; then
    echo ""
    echo "▸ Creating new VM for disaster recovery..."

    DOMAIN_SLUG="${DOMAIN//./-}"
    RG_NAME="${DOMAIN_SLUG}-dr-rg"
    VM_NAME="${DOMAIN_SLUG}-dr-vm"

    # Create resource group
    az group create --name "$RG_NAME" --location "$LOCATION" -o none

    # Create VM
    VM_OUTPUT=$(az vm create \
        --resource-group "$RG_NAME" \
        --name "$VM_NAME" \
        --image Ubuntu2404 \
        --size Standard_B1s \
        --admin-username azureuser \
        --generate-ssh-keys \
        --public-ip-sku Standard \
        --nsg "${DOMAIN_SLUG}-dr-nsg" \
        --os-disk-size-gb 30 \
        -o json 2>&1)

    HOST=$(echo "$VM_OUTPUT" | grep -o '"publicIpAddress": "[^"]*"' | cut -d'"' -f4)
    echo "  New VM IP: $HOST"

    # Open ports
    az network nsg rule create \
        --resource-group "$RG_NAME" \
        --nsg-name "${DOMAIN_SLUG}-dr-nsg" \
        --name AllowHTTP \
        --priority 100 \
        --destination-port-ranges 80 443 \
        --protocol Tcp \
        --access Allow \
        -o none

    # Install base packages
    echo "▸ Installing packages on new VM..."
    az vm run-command invoke \
        --resource-group "$RG_NAME" \
        --name "$VM_NAME" \
        --command-id RunShellScript \
        --scripts '
            apt-get update -qq < /dev/null
            apt-get install -y postgresql-16 nginx certbot python3-certbot-nginx < /dev/null
            systemctl enable postgresql
            systemctl start postgresql
        ' -o none
fi

# ── Upload backup to VM ─────────────────────────────────────────────────────
echo "▸ Uploading backup to VM..."
scp -o StrictHostKeyChecking=no "$BACKUP_FILE" "azureuser@${HOST}:/tmp/restore-backup.tar.gz"

# ── Restore on VM ────────────────────────────────────────────────────────────
echo "▸ Restoring database and configuration..."

ssh -o StrictHostKeyChecking=no "azureuser@${HOST}" "
    sudo bash -c '
        set -e

        # Extract
        TMPDIR=\$(mktemp -d)
        tar xzf /tmp/restore-backup.tar.gz -C \"\$TMPDIR\"
        BACKUP=\$(find \"\$TMPDIR\" -maxdepth 1 -type d | tail -1)

        echo \"Restoring from: \$BACKUP\"

        # ── Restore database ─────────────────────────────────────────────
        # Extract DB credentials from service file
        DB_URL=\$(grep DASHBOARD_DB_URL \"\$BACKUP/dashboard.service\" | head -1 | sed \"s/.*=//\" | sed \"s/Environment=//\")
        if [ -z \"\$DB_URL\" ]; then
            DB_USER=dashboard
            DB_PASS=\$(openssl rand -base64 18)
            DB_NAME=dashboard
        else
            DB_USER=\$(echo \"\$DB_URL\" | sed -n \"s|postgres://\([^:]*\):.*|\1|p\")
            DB_PASS=\$(echo \"\$DB_URL\" | sed -n \"s|postgres://[^:]*:\([^@]*\)@.*|\1|p\")
            DB_NAME=\$(echo \"\$DB_URL\" | sed -n \"s|.*/\([^?]*\).*|\1|p\")
        fi

        echo \"DB: user=\$DB_USER db=\$DB_NAME\"

        # Create user + database if needed
        sudo -u postgres psql -c \"CREATE USER \$DB_USER WITH PASSWORD \x27\$DB_PASS\x27;\" 2>/dev/null || \
        sudo -u postgres psql -c \"ALTER USER \$DB_USER WITH PASSWORD \x27\$DB_PASS\x27;\" 2>/dev/null || true
        sudo -u postgres psql -c \"CREATE DATABASE \$DB_NAME OWNER \$DB_USER;\" 2>/dev/null || true

        # Drop existing tables and restore
        sudo -u postgres psql -d \$DB_NAME -c \"DROP SCHEMA public CASCADE; CREATE SCHEMA public; GRANT ALL ON SCHEMA public TO \$DB_USER;\" 2>/dev/null
        sudo -u postgres psql -d \$DB_NAME < \"\$BACKUP/database.sql\"
        sudo -u postgres psql -d \$DB_NAME -c \"GRANT ALL ON ALL TABLES IN SCHEMA public TO \$DB_USER; GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO \$DB_USER; ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO \$DB_USER;\"
        echo \"Database restored\"

        # ── Restore service config ────────────────────────────────────────
        if [ -f \"\$BACKUP/dashboard.service\" ]; then
            cp \"\$BACKUP/dashboard.service\" /etc/systemd/system/dashboard.service
            echo \"Service config restored\"
        fi

        # ── Download dashboard binary ─────────────────────────────────────
        RELEASE=\$(grep -oP \"v[0-9]+\.[0-9]+\.[0-9]+\" \"\$BACKUP/version.txt\" 2>/dev/null || echo \"latest\")
        if [ \"\$RELEASE\" = \"latest\" ] || [ \"\$RELEASE\" = \"unknown\" ]; then
            RELEASE=\$(curl -s https://api.github.com/repos/irlm/networker-tester/releases/latest | grep tag_name | cut -d\\\" -f4)
        fi
        echo \"Downloading release: \$RELEASE\"

        TARGET=x86_64-unknown-linux-musl
        mkdir -p /opt/dashboard/dashboard/dist
        cd /tmp
        curl -sL \"https://github.com/irlm/networker-tester/releases/download/\${RELEASE}/networker-dashboard-\${TARGET}.tar.gz\" | tar xz
        cp networker-dashboard /opt/dashboard/
        chmod +x /opt/dashboard/networker-dashboard
        curl -sL \"https://github.com/irlm/networker-tester/releases/download/\${RELEASE}/dashboard-frontend.tar.gz\" -o frontend.tar.gz
        tar xzf frontend.tar.gz -C /opt/dashboard/dashboard/dist/
        echo \"Binary and frontend installed\"

        # ── Restore Nginx config ──────────────────────────────────────────
        if [ -d \"\$BACKUP/nginx-sites\" ]; then
            cp -f \"\$BACKUP/nginx-sites/\"* /etc/nginx/sites-available/ 2>/dev/null || true
            for f in /etc/nginx/sites-available/*; do
                ln -sf \"\$f\" /etc/nginx/sites-enabled/ 2>/dev/null || true
            done
            rm -f /etc/nginx/sites-enabled/default
            # Remove SSL directives if certs dont exist yet
            sed -i \"/ssl_certificate/d\" /etc/nginx/sites-available/* 2>/dev/null || true
            sed -i \"/listen 443/d\" /etc/nginx/sites-available/* 2>/dev/null || true
            nginx -t && systemctl reload nginx
            echo \"Nginx config restored (SSL stripped — run certbot to re-add)\"
        fi

        # ── Restore SSL certs (if present) ────────────────────────────────
        if [ -d \"\$BACKUP/ssl-live\" ]; then
            mkdir -p /etc/letsencrypt/live /etc/letsencrypt/renewal
            cp -r \"\$BACKUP/ssl-live/\"* /etc/letsencrypt/live/ 2>/dev/null || true
            cp -r \"\$BACKUP/ssl-renewal/\"* /etc/letsencrypt/renewal/ 2>/dev/null || true
            echo \"SSL certificates restored\"
        fi

        # ── Start dashboard ───────────────────────────────────────────────
        systemctl daemon-reload
        systemctl enable dashboard
        systemctl restart dashboard
        sleep 3
        systemctl is-active dashboard && echo \"Dashboard is running\"

        # Cleanup
        rm -rf \"\$TMPDIR\" /tmp/restore-backup.tar.gz /tmp/networker-dashboard /tmp/frontend.tar.gz

        echo \"RESTORE_COMPLETE\"
    '
"

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Restore Complete!                                          ║"
echo "╠══════════════════════════════════════════════════════════════╣"
echo "║  VM:  $HOST                                                 "
echo "║                                                             ║"
echo "║  Next steps:                                                ║"
echo "║  1. Update DNS A records to point to $HOST                  "
echo "║  2. Run certbot for SSL:                                    ║"
echo "║     ssh azureuser@$HOST                                     "
echo "║     sudo certbot --nginx --non-interactive --agree-tos \\    "
echo "║       --email admin@${DOMAIN:-yourdomain.com} \\             "
echo "║       -d ${DOMAIN:-yourdomain.com} --redirect               "
echo "║  3. Verify: https://${DOMAIN:-yourdomain.com}               "
echo "╚══════════════════════════════════════════════════════════════╝"

rm -rf "$RESTORE_TMP"
