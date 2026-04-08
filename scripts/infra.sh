#!/usr/bin/env bash
# infra.sh — Disaster Recovery & Sandbox CLI
#
# Commands:
#   restore --env prod --confirm-production [--db-only]
#   sandbox --name NAME [--destroy] [--ttl HOURS]
#   list
#
# Required env:
#   BACKUP_BLOB_URL    Azure Blob SAS URL to the backups container
#
# Optional env (defaults shown):
#   AZURE_RESOURCE_GROUP   networker-rg
#   AZURE_LOCATION         eastus
#   AZURE_VNET             networker-vnet
#   AZURE_SUBNET           default
#   AZURE_DB_IMAGE         networker-db-image
#   AZURE_WEB_IMAGE        networker-web-image
#   AZURE_VM_SIZE          Standard_B1ls

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────────────────
AZURE_RESOURCE_GROUP="${AZURE_RESOURCE_GROUP:-networker-rg}"
AZURE_LOCATION="${AZURE_LOCATION:-eastus}"
AZURE_VNET="${AZURE_VNET:-networker-vnet}"
AZURE_SUBNET="${AZURE_SUBNET:-default}"
AZURE_DB_IMAGE="${AZURE_DB_IMAGE:-networker-db-image}"
AZURE_WEB_IMAGE="${AZURE_WEB_IMAGE:-networker-web-image}"
AZURE_VM_SIZE="${AZURE_VM_SIZE:-Standard_B1ls}"

# ── Helpers ───────────────────────────────────────────────────────────────────
log()  { printf '[%s] %s\n' "$(date -u '+%H:%M:%S')" "$*"; }
die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" > /dev/null 2>&1 || die "Required tool not found: $1"; }

# ── Usage ─────────────────────────────────────────────────────────────────────
usage() {
    cat <<'EOF'
infra.sh — Disaster Recovery & Sandbox CLI

USAGE
  infra.sh restore --env prod --confirm-production [--db-only]
  infra.sh sandbox --name NAME [--destroy] [--ttl HOURS]
  infra.sh list

COMMANDS
  restore   Full DR restore to Azure VMs from the latest backup blob.
            --env prod                  Target environment (only 'prod' accepted).
            --confirm-production        Safety flag required for production restores.
            --db-only                   Skip web-server provisioning; restore DB only.

  sandbox   Create or destroy an isolated sandbox environment.
            --name NAME                 Sandbox identifier (alphanumeric + hyphens).
            --destroy                   Delete sandbox VM, NIC, and disk.
            --ttl HOURS                 Auto-shutdown after N hours (default: 48).

  list      Show all sandbox VMs (env=sandbox tag) as a table.

ENVIRONMENT VARIABLES
  BACKUP_BLOB_URL        (required) Azure Blob SAS URL to the backups container.
  AZURE_RESOURCE_GROUP   Resource group (default: networker-rg).
  AZURE_LOCATION         Azure region   (default: eastus).
  AZURE_VNET             VNet name      (default: networker-vnet).
  AZURE_SUBNET           Subnet name    (default: default).
  AZURE_DB_IMAGE         DB VM image    (default: networker-db-image).
  AZURE_WEB_IMAGE        Web VM image   (default: networker-web-image).
  AZURE_VM_SIZE          VM size        (default: Standard_B1ls).

EXAMPLES
  # Full production restore
  BACKUP_BLOB_URL=https://... ./scripts/infra.sh restore \
    --env prod --confirm-production

  # Restore DB only
  BACKUP_BLOB_URL=https://... ./scripts/infra.sh restore \
    --env prod --confirm-production --db-only

  # Create a 24-hour sandbox
  BACKUP_BLOB_URL=https://... ./scripts/infra.sh sandbox \
    --name pr-1234 --ttl 24

  # Destroy a sandbox
  ./scripts/infra.sh sandbox --name pr-1234 --destroy

  # List all sandboxes
  ./scripts/infra.sh list
EOF
}

# ── cmd_restore ───────────────────────────────────────────────────────────────
cmd_restore() {
    local env="" confirm_production=false db_only=false

    while [ $# -gt 0 ]; do
        case "$1" in
            --env)                  env="$2"; shift 2 ;;
            --confirm-production)   confirm_production=true; shift ;;
            --db-only)              db_only=true; shift ;;
            *) die "Unknown restore option: $1" ;;
        esac
    done

    [ "$env" = "prod" ] || die "--env must be 'prod'"
    $confirm_production || die "--confirm-production flag required for production restores"
    [ -n "${BACKUP_BLOB_URL:-}" ] || die "BACKUP_BLOB_URL environment variable is required"

    need az
    need azcopy
    need ssh
    need scp

    local start_ts
    start_ts="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    log "=== Production restore started at $start_ts ==="

    # ── 1. Provision db-server VM from pre-baked image (no public IP, in VNet) ──
    local db_vm="networker-db-server"
    log "Provisioning $db_vm from image $AZURE_DB_IMAGE ..."
    az vm create \
        --resource-group "$AZURE_RESOURCE_GROUP" \
        --name "$db_vm" \
        --image "$AZURE_DB_IMAGE" \
        --size "$AZURE_VM_SIZE" \
        --admin-username azureuser \
        --generate-ssh-keys \
        --public-ip-address "" \
        --vnet-name "$AZURE_VNET" \
        --subnet "$AZURE_SUBNET" \
        --nsg "" \
        --no-wait \
        --output none < /dev/null

    log "Waiting for $db_vm to be running ..."
    az vm wait \
        --resource-group "$AZURE_RESOURCE_GROUP" \
        --name "$db_vm" \
        --created \
        --output none < /dev/null

    local db_private_ip
    db_private_ip="$(az vm show \
        --resource-group "$AZURE_RESOURCE_GROUP" \
        --name "$db_vm" \
        --show-details \
        --query 'privateIps' \
        --output tsv < /dev/null)"
    log "DB server private IP: $db_private_ip"

    # ── 2. Download latest backup from Blob (read last_backup.json marker) ───
    local tmp_dir
    tmp_dir="$(mktemp -d)"
    log "Fetching last_backup.json marker from blob ..."
    azcopy copy \
        "${BACKUP_BLOB_URL}/last_backup.json" \
        "${tmp_dir}/last_backup.json" \
        --output-type none < /dev/null

    local backup_file
    backup_file="$(grep -o '"file":"[^"]*"' "${tmp_dir}/last_backup.json" | cut -d'"' -f4)"
    [ -n "$backup_file" ] || die "Could not parse backup filename from last_backup.json"
    log "Latest backup: $backup_file"

    local dump_path="${tmp_dir}/${backup_file}"
    log "Downloading backup dump ..."
    azcopy copy \
        "${BACKUP_BLOB_URL}/${backup_file}" \
        "$dump_path" \
        --output-type none < /dev/null

    # ── 3. SCP dump to db-server, restore via pg_restore ─────────────────────
    log "Uploading dump to $db_vm ($db_private_ip) ..."
    scp -o StrictHostKeyChecking=no \
        "$dump_path" \
        "azureuser@${db_private_ip}:/tmp/networker_core.dump" < /dev/null

    log "Restoring networker_core via pg_restore ..."
    ssh -o StrictHostKeyChecking=no \
        "azureuser@${db_private_ip}" \
        'sudo -u postgres pg_restore --clean --if-exists -d networker_core /tmp/networker_core.dump && rm /tmp/networker_core.dump' \
        < /dev/null

    # ── 4. Create empty networker_logs ────────────────────────────────────────
    log "Creating empty networker_logs database ..."
    ssh -o StrictHostKeyChecking=no \
        "azureuser@${db_private_ip}" \
        "sudo -u postgres psql -c \"CREATE DATABASE networker_logs OWNER networker;\" 2>/dev/null || true" \
        < /dev/null

    # ── 5. Configure PostgreSQL users and SSL ─────────────────────────────────
    log "Configuring PostgreSQL users and SSL ..."
    ssh -o StrictHostKeyChecking=no \
        "azureuser@${db_private_ip}" \
        <<'REMOTE'
set -e
sudo -u postgres psql <<'SQL'
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'networker') THEN
        CREATE ROLE networker WITH LOGIN;
    END IF;
END$$;
GRANT ALL PRIVILEGES ON DATABASE networker_core TO networker;
GRANT ALL PRIVILEGES ON DATABASE networker_logs TO networker;
SQL

# Enable SSL in postgresql.conf if not already on
sudo grep -q "^ssl = on" /etc/postgresql/*/main/postgresql.conf 2>/dev/null \
    || echo "ssl = on" | sudo tee -a /etc/postgresql/*/main/postgresql.conf > /dev/null
sudo systemctl reload postgresql 2>/dev/null || sudo service postgresql reload
REMOTE

    # ── 6. Provision web-server and deploy binaries (unless --db-only) ────────
    if ! $db_only; then
        local web_vm="networker-web-server"
        log "Provisioning $web_vm from image $AZURE_WEB_IMAGE ..."
        az vm create \
            --resource-group "$AZURE_RESOURCE_GROUP" \
            --name "$web_vm" \
            --image "$AZURE_WEB_IMAGE" \
            --size "$AZURE_VM_SIZE" \
            --admin-username azureuser \
            --generate-ssh-keys \
            --public-ip-sku Standard \
            --vnet-name "$AZURE_VNET" \
            --subnet "$AZURE_SUBNET" \
            --nsg "" \
            --output none < /dev/null

        az vm wait \
            --resource-group "$AZURE_RESOURCE_GROUP" \
            --name "$web_vm" \
            --created \
            --output none < /dev/null

        local web_public_ip
        web_public_ip="$(az vm show \
            --resource-group "$AZURE_RESOURCE_GROUP" \
            --name "$web_vm" \
            --show-details \
            --query 'publicIps' \
            --output tsv < /dev/null)"
        log "Web server public IP: $web_public_ip"

        # Fetch latest GH release tag
        local release_tag
        release_tag="$(curl -sf \
            https://api.github.com/repos/irlm/networker-tester/releases/latest \
            | grep '"tag_name"' | cut -d'"' -f4)"
        [ -n "$release_tag" ] || die "Could not determine latest GitHub release tag"
        log "Deploying binaries from release $release_tag ..."

        local target="x86_64-unknown-linux-musl"
        ssh -o StrictHostKeyChecking=no \
            "azureuser@${web_public_ip}" \
            "$(printf 'set -e
RELEASE=%s
TARGET=%s
DB_IP=%s
mkdir -p /opt/dashboard/dashboard/dist
cd /tmp
curl -sL "https://github.com/irlm/networker-tester/releases/download/${RELEASE}/networker-dashboard-${TARGET}.tar.gz" | tar xz
sudo cp networker-dashboard /opt/dashboard/
sudo chmod +x /opt/dashboard/networker-dashboard
curl -sL "https://github.com/irlm/networker-tester/releases/download/${RELEASE}/dashboard-frontend.tar.gz" -o frontend.tar.gz
sudo tar xzf frontend.tar.gz -C /opt/dashboard/dashboard/dist/
sudo sed -i "s|DASHBOARD_DB_URL=.*|DASHBOARD_DB_URL=postgres://networker@${DB_IP}/networker_core|" \
    /etc/systemd/system/dashboard.service 2>/dev/null || true
sudo systemctl daemon-reload
sudo systemctl enable dashboard
sudo systemctl restart dashboard
rm -f /tmp/networker-dashboard /tmp/frontend.tar.gz' \
            "$release_tag" "$target" "$db_private_ip")" \
            < /dev/null

        # ── 7. Health check ───────────────────────────────────────────────────
        log "Health check ..."
        local attempts=0
        local healthy=false
        while [ $attempts -lt 12 ]; do
            if curl -sf --max-time 5 "http://${web_public_ip}:3000/api/version" > /dev/null 2>&1; then
                healthy=true
                break
            fi
            attempts=$((attempts + 1))
            sleep 5
        done

        if $healthy; then
            log "Health check passed."
        else
            log "WARNING: Health check did not pass within 60 seconds — check dashboard service."
        fi
    fi

    rm -rf "$tmp_dir"

    local end_ts
    end_ts="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    log "=== Restore complete ==="
    printf '\nSummary\n'
    printf '  Started:  %s\n' "$start_ts"
    printf '  Finished: %s\n' "$end_ts"
    printf '  DB VM:    %s (%s)\n' "$db_vm" "${db_private_ip:-unknown}"
    if ! $db_only; then
        printf '  Web VM:   %s (%s)\n' "$web_vm" "${web_public_ip:-unknown}"
        printf '  API:      http://%s:3000\n' "${web_public_ip:-<web-ip>}"
    fi
}

# ── cmd_sandbox ───────────────────────────────────────────────────────────────
cmd_sandbox() {
    local name="" destroy=false ttl=48

    while [ $# -gt 0 ]; do
        case "$1" in
            --name)     name="$2"; shift 2 ;;
            --destroy)  destroy=true; shift ;;
            --ttl)      ttl="$2"; shift 2 ;;
            *) die "Unknown sandbox option: $1" ;;
        esac
    done

    [ -n "$name" ] || die "--name NAME is required"

    # Sanitise: only alphanumeric + hyphens, max 15 chars for VM name
    local safe_name
    safe_name="$(printf '%s' "$name" | tr -cd 'a-zA-Z0-9-' | cut -c1-15)"
    [ -n "$safe_name" ] || die "Sandbox name must contain alphanumeric characters"

    local vm_name="sb-${safe_name}"
    local nic_name="${vm_name}-nic"
    local disk_name="${vm_name}-osdisk"

    if $destroy; then
        log "Destroying sandbox: $vm_name ..."

        # Delete VM
        az vm delete \
            --resource-group "$AZURE_RESOURCE_GROUP" \
            --name "$vm_name" \
            --yes \
            --output none < /dev/null 2>/dev/null \
            || log "VM $vm_name not found (already deleted?)"

        # Delete NIC
        az network nic delete \
            --resource-group "$AZURE_RESOURCE_GROUP" \
            --name "$nic_name" \
            --output none < /dev/null 2>/dev/null \
            || log "NIC $nic_name not found (already deleted?)"

        # Delete disk
        az disk delete \
            --resource-group "$AZURE_RESOURCE_GROUP" \
            --name "$disk_name" \
            --yes \
            --output none < /dev/null 2>/dev/null \
            || log "Disk $disk_name not found (already deleted?)"

        log "Sandbox $name destroyed."
        return 0
    fi

    # ── Create sandbox ────────────────────────────────────────────────────────
    [ -n "${BACKUP_BLOB_URL:-}" ] || die "BACKUP_BLOB_URL environment variable is required"

    need az
    need azcopy
    need ssh
    need scp

    local created_at
    created_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    local auto_shutdown_time
    auto_shutdown_time="$(date -u -v+"${ttl}"H '+%H%M' 2>/dev/null \
        || date -u -d "+${ttl} hours" '+%H%M' 2>/dev/null \
        || printf '2359')"

    log "Creating sandbox VM: $vm_name (TTL: ${ttl}h) ..."

    az vm create \
        --resource-group "$AZURE_RESOURCE_GROUP" \
        --name "$vm_name" \
        --image "$AZURE_DB_IMAGE" \
        --size "$AZURE_VM_SIZE" \
        --admin-username azureuser \
        --generate-ssh-keys \
        --public-ip-sku Standard \
        --vnet-name "$AZURE_VNET" \
        --subnet "$AZURE_SUBNET" \
        --nsg "" \
        --tags "env=sandbox" "ttl=${ttl}" "created=${created_at}" "sandbox_name=${name}" \
        --os-disk-name "$disk_name" \
        --nic-delete-option Delete \
        --output none < /dev/null

    local sb_ip
    sb_ip="$(az vm show \
        --resource-group "$AZURE_RESOURCE_GROUP" \
        --name "$vm_name" \
        --show-details \
        --query 'publicIps' \
        --output tsv < /dev/null)"
    log "Sandbox VM public IP: $sb_ip"

    # Configure auto-shutdown
    az vm auto-shutdown \
        --resource-group "$AZURE_RESOURCE_GROUP" \
        --name "$vm_name" \
        --time "$auto_shutdown_time" \
        --output none < /dev/null

    # ── Download and restore core DB ─────────────────────────────────────────
    local tmp_dir
    tmp_dir="$(mktemp -d)"
    log "Fetching last_backup.json marker ..."
    azcopy copy \
        "${BACKUP_BLOB_URL}/last_backup.json" \
        "${tmp_dir}/last_backup.json" \
        --output-type none < /dev/null

    local backup_file
    backup_file="$(grep -o '"file":"[^"]*"' "${tmp_dir}/last_backup.json" | cut -d'"' -f4)"
    [ -n "$backup_file" ] || die "Could not parse backup filename from last_backup.json"

    local dump_path="${tmp_dir}/${backup_file}"
    log "Downloading backup dump: $backup_file ..."
    azcopy copy \
        "${BACKUP_BLOB_URL}/${backup_file}" \
        "$dump_path" \
        --output-type none < /dev/null

    log "Uploading dump to sandbox VM ..."
    scp -o StrictHostKeyChecking=no \
        "$dump_path" \
        "azureuser@${sb_ip}:/tmp/networker_core.dump" < /dev/null

    log "Restoring networker_core ..."
    ssh -o StrictHostKeyChecking=no \
        "azureuser@${sb_ip}" \
        'sudo -u postgres pg_restore --clean --if-exists -d networker_core /tmp/networker_core.dump && rm /tmp/networker_core.dump' \
        < /dev/null

    # ── Run anonymize.sql ─────────────────────────────────────────────────────
    local script_dir
    script_dir="$(cd "$(dirname "$0")" && pwd)"
    local anon_sql="${script_dir}/anonymize.sql"
    [ -f "$anon_sql" ] || die "anonymize.sql not found at: $anon_sql"

    log "Uploading anonymize.sql ..."
    scp -o StrictHostKeyChecking=no \
        "$anon_sql" \
        "azureuser@${sb_ip}:/tmp/anonymize.sql" < /dev/null

    log "Running anonymize.sql ..."
    ssh -o StrictHostKeyChecking=no \
        "azureuser@${sb_ip}" \
        'sudo -u postgres psql -d networker_core -f /tmp/anonymize.sql && rm /tmp/anonymize.sql' \
        < /dev/null

    # ── Deploy latest app binary ──────────────────────────────────────────────
    log "Deploying latest app binary ..."
    local release_tag
    release_tag="$(curl -sf \
        https://api.github.com/repos/irlm/networker-tester/releases/latest \
        | grep '"tag_name"' | cut -d'"' -f4)"
    [ -n "$release_tag" ] || die "Could not determine latest GitHub release tag"

    local target="x86_64-unknown-linux-musl"
    ssh -o StrictHostKeyChecking=no \
        "azureuser@${sb_ip}" \
        "$(printf 'set -e
RELEASE=%s
TARGET=%s
mkdir -p /opt/dashboard/dashboard/dist
cd /tmp
curl -sL "https://github.com/irlm/networker-tester/releases/download/${RELEASE}/networker-dashboard-${TARGET}.tar.gz" | tar xz
sudo cp networker-dashboard /opt/dashboard/
sudo chmod +x /opt/dashboard/networker-dashboard
curl -sL "https://github.com/irlm/networker-tester/releases/download/${RELEASE}/dashboard-frontend.tar.gz" -o frontend.tar.gz
sudo tar xzf frontend.tar.gz -C /opt/dashboard/dashboard/dist/
sudo systemctl daemon-reload
sudo systemctl enable dashboard
sudo systemctl restart dashboard
rm -f /tmp/networker-dashboard /tmp/frontend.tar.gz' \
        "$release_tag" "$target")" \
        < /dev/null

    rm -rf "$tmp_dir"

    log "Sandbox $name ready."
    printf '\nSandbox\n'
    printf '  Name:      %s\n' "$name"
    printf '  VM:        %s\n' "$vm_name"
    printf '  IP:        %s\n' "$sb_ip"
    printf '  TTL:       %sh (auto-shutdown at %s UTC)\n' "$ttl" "$auto_shutdown_time"
    printf '  Created:   %s\n' "$created_at"
    printf '  API:       http://%s:3000\n' "$sb_ip"
    printf '  Destroy:   %s sandbox --name %s --destroy\n' "$0" "$name"
}

# ── cmd_list ──────────────────────────────────────────────────────────────────
cmd_list() {
    need az

    log "Listing sandbox VMs (env=sandbox) in $AZURE_RESOURCE_GROUP ..."
    printf '\n'

    az vm list \
        --resource-group "$AZURE_RESOURCE_GROUP" \
        --show-details \
        --query "[?tags.env=='sandbox'].[name, powerState, tags.sandbox_name, tags.ttl, tags.created, publicIps]" \
        --output table < /dev/null
}

# ── Dispatch ──────────────────────────────────────────────────────────────────
if [ $# -eq 0 ]; then
    usage
    exit 0
fi

command="$1"
shift

case "$command" in
    restore) cmd_restore "$@" ;;
    sandbox) cmd_sandbox "$@" ;;
    list)    cmd_list ;;
    -h|--help|help) usage ;;
    *) die "Unknown command: $command. Run '$0 --help' for usage." ;;
esac
