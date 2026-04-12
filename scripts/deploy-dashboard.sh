#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# AletheDash Dashboard — Full Infrastructure Deployment
#
# Creates everything from scratch:
#   - Resource Group
#   - VM (Ubuntu 24.04, B1s)
#   - PostgreSQL (on VM)
#   - Nginx + Let's Encrypt SSL
#   - Microsoft SSO App Registration
#   - Azure Communication Services (email)
#   - Dashboard binary + frontend from GitHub release
#   - Systemd service with all env vars
#
# Usage:
#   ./scripts/deploy-dashboard.sh \
#     --domain alethedash.com \
#     --admin-email admin@alethedash.com \
#     --location eastus \
#     --release v0.14.0
#
# Prerequisites:
#   - az cli logged in (az login)
#   - Subscription selected (az account set --subscription <id>)
#
# After running:
#   - Dashboard is live at https://<domain>
#   - Login with admin-email + temp password (printed at end)
#   - Must change password on first login
# ============================================================================

# ── Defaults ─────────────────────────────────────────────────────────────────
DOMAIN=""
EXTRA_DOMAINS=""        # comma-separated additional domains (e.g., "example.net,example.info")
ADMIN_EMAIL=""
LOCATION="eastus"
RELEASE="latest"        # GitHub release tag, or "latest"
VM_SIZE="Standard_B1s"
RG_NAME=""              # auto-generated from domain if empty
VM_NAME=""              # auto-generated from domain if empty
DB_PASSWORD=""          # auto-generated if empty
ADMIN_PASSWORD=""       # auto-generated if empty
SKIP_SSO=false
SKIP_EMAIL=false
DRY_RUN=false

# ── Parse arguments ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case $1 in
        --domain)        DOMAIN="$2"; shift 2 ;;
        --extra-domains) EXTRA_DOMAINS="$2"; shift 2 ;;
        --admin-email)   ADMIN_EMAIL="$2"; shift 2 ;;
        --location)      LOCATION="$2"; shift 2 ;;
        --release)       RELEASE="$2"; shift 2 ;;
        --vm-size)       VM_SIZE="$2"; shift 2 ;;
        --rg)            RG_NAME="$2"; shift 2 ;;
        --skip-sso)      SKIP_SSO=true; shift ;;
        --skip-email)    SKIP_EMAIL=true; shift ;;
        --dry-run)       DRY_RUN=true; shift ;;
        -h|--help)
            echo "Usage: $0 --domain <domain> --admin-email <email> [options]"
            echo ""
            echo "Required:"
            echo "  --domain         Primary domain (e.g., alethedash.com)"
            echo "  --admin-email    Admin account email"
            echo ""
            echo "Optional:"
            echo "  --extra-domains  Additional domains, comma-separated (redirect to primary)"
            echo "  --location       Azure region (default: eastus)"
            echo "  --release        GitHub release tag (default: latest)"
            echo "  --vm-size        VM size (default: Standard_B1s)"
            echo "  --rg             Resource group name (default: <domain>-rg)"
            echo "  --skip-sso       Skip Microsoft SSO setup"
            echo "  --skip-email     Skip Azure Communication Services setup"
            echo "  --dry-run        Print what would be done without executing"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ── Validate ─────────────────────────────────────────────────────────────────
if [[ -z "$DOMAIN" ]]; then
    echo "ERROR: --domain is required"
    exit 1
fi
if [[ -z "$ADMIN_EMAIL" ]]; then
    echo "ERROR: --admin-email is required"
    exit 1
fi

# Auto-generate names from domain
DOMAIN_SLUG="${DOMAIN//./-}"
RG_NAME="${RG_NAME:-${DOMAIN_SLUG}-rg}"
VM_NAME="${VM_NAME:-${DOMAIN_SLUG}-vm}"
NSG_NAME="${DOMAIN_SLUG}-nsg"
IP_NAME="${DOMAIN_SLUG}-ip"

# Auto-generate secrets
DB_PASSWORD="${DB_PASSWORD:-$(openssl rand -base64 18)}"
ADMIN_PASSWORD="${ADMIN_PASSWORD:-$(openssl rand -base64 12)}"
JWT_SECRET="$(openssl rand -base64 32)"
CREDENTIAL_KEY="$(openssl rand -hex 32)"

# Resolve release tag
if [[ "$RELEASE" == "latest" ]]; then
    RELEASE=$(curl -s https://api.github.com/repos/irlm/networker-tester/releases/latest | grep tag_name | cut -d'"' -f4)
    echo "Latest release: $RELEASE"
fi

REPO="https://github.com/irlm/networker-tester/releases/download"
TARGET="x86_64-unknown-linux-musl"

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  AletheDash Dashboard Deployment                            ║"
echo "╠══════════════════════════════════════════════════════════════╣"
echo "║  Domain:       $DOMAIN"
echo "║  Admin:        $ADMIN_EMAIL"
echo "║  Location:     $LOCATION"
echo "║  Release:      $RELEASE"
echo "║  VM:           $VM_SIZE"
echo "║  RG:           $RG_NAME"
echo "║  SSO:          $([ "$SKIP_SSO" = true ] && echo "skipped" || echo "Microsoft Entra ID")"
echo "║  Email:        $([ "$SKIP_EMAIL" = true ] && echo "skipped" || echo "Azure Communication Services")"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

if [[ "$DRY_RUN" == true ]]; then
    echo "[DRY RUN] Would create the above infrastructure. Exiting."
    exit 0
fi

echo "Starting deployment... (this takes ~5 minutes)"
echo ""

# ── Step 1: Resource Group ───────────────────────────────────────────────────
echo "▸ Creating resource group..."
az group create --name "$RG_NAME" --location "$LOCATION" -o none

# ── Step 2: VM ───────────────────────────────────────────────────────────────
echo "▸ Creating VM ($VM_SIZE)..."
VM_OUTPUT=$(az vm create \
    --resource-group "$RG_NAME" \
    --name "$VM_NAME" \
    --image Ubuntu2404 \
    --size "$VM_SIZE" \
    --admin-username azureuser \
    --generate-ssh-keys \
    --public-ip-sku Standard \
    --public-ip-address "$IP_NAME" \
    --nsg "$NSG_NAME" \
    --os-disk-size-gb 30 \
    -o json 2>&1)

VM_IP=$(echo "$VM_OUTPUT" | grep -o '"publicIpAddress": "[^"]*"' | cut -d'"' -f4)
echo "  VM IP: $VM_IP"

# Open HTTP/HTTPS ports
echo "▸ Opening ports 80/443..."
az network nsg rule create \
    --resource-group "$RG_NAME" \
    --nsg-name "$NSG_NAME" \
    --name AllowHTTP \
    --priority 100 \
    --destination-port-ranges 80 443 \
    --protocol Tcp \
    --access Allow \
    -o none

# ── Step 3: SSO App Registration ────────────────────────────────────────────
SSO_CLIENT_ID=""
SSO_CLIENT_SECRET=""
SSO_TENANT_ID=""

if [[ "$SKIP_SSO" != true ]]; then
    echo "▸ Creating Microsoft SSO app registration..."
    SSO_OUTPUT=$(az ad app create \
        --display-name "${DOMAIN} Dashboard" \
        --web-redirect-uris "https://${DOMAIN}/api/auth/sso/callback" \
        --sign-in-audience AzureADMyOrg \
        --query 'appId' -o tsv 2>&1)
    SSO_CLIENT_ID="$SSO_OUTPUT"

    # Create service principal
    az ad sp create --id "$SSO_CLIENT_ID" -o none 2>/dev/null || true

    # Create client secret
    SSO_CRED=$(az ad app credential reset \
        --id "$SSO_CLIENT_ID" \
        --append --years 2 \
        --query '{secret: password, tenant: tenant}' -o json 2>&1)
    SSO_CLIENT_SECRET=$(echo "$SSO_CRED" | grep -o '"secret": "[^"]*"' | cut -d'"' -f4)
    SSO_TENANT_ID=$(echo "$SSO_CRED" | grep -o '"tenant": "[^"]*"' | cut -d'"' -f4)

    # Grant OpenID permissions
    az ad app permission add \
        --id "$SSO_CLIENT_ID" \
        --api 00000003-0000-0000-c000-000000000000 \
        --api-permissions \
            e1fe6dd8-ba31-4d61-89e7-88639da4683d=Scope \
            37f7f235-527c-4136-accd-4a02d197296e=Scope \
            14dad69e-099b-42c9-810b-d002981feec1=Scope \
        -o none 2>/dev/null || true
    az ad app permission grant \
        --id "$SSO_CLIENT_ID" \
        --api 00000003-0000-0000-c000-000000000000 \
        --scope "openid email profile" \
        -o none 2>/dev/null || true

    echo "  SSO App ID: $SSO_CLIENT_ID"
fi

# ── Step 4: Azure Communication Services ─────────────────────────────────────
ACS_CONN=""
ACS_SENDER=""

if [[ "$SKIP_EMAIL" != true ]]; then
    echo "▸ Creating Azure Communication Services..."
    ACS_NAME="${DOMAIN_SLUG}-comm"
    EMAIL_NAME="${DOMAIN_SLUG}-email"

    az communication create \
        --name "$ACS_NAME" \
        --resource-group "$RG_NAME" \
        --location global \
        --data-location unitedstates \
        -o none 2>/dev/null || true

    az communication email create \
        --name "$EMAIL_NAME" \
        --resource-group "$RG_NAME" \
        --location global \
        --data-location unitedstates \
        -o none 2>/dev/null || true

    # Create Azure-managed email domain
    DOMAIN_OUTPUT=$(az communication email domain create \
        --name AzureManagedDomain \
        --email-service-name "$EMAIL_NAME" \
        --resource-group "$RG_NAME" \
        --location global \
        --domain-management AzureManaged \
        --query 'fromSenderDomain' -o tsv 2>/dev/null || echo "")

    # Link email domain
    az rest --method patch \
        --uri "/subscriptions/$(az account show --query id -o tsv)/resourceGroups/${RG_NAME}/providers/Microsoft.Communication/communicationServices/${ACS_NAME}?api-version=2023-04-01" \
        --body "{\"properties\":{\"linkedDomains\":[\"/subscriptions/$(az account show --query id -o tsv)/resourceGroups/${RG_NAME}/providers/Microsoft.Communication/emailServices/${EMAIL_NAME}/domains/AzureManagedDomain\"]}}" \
        -o none 2>/dev/null || true

    ACS_CONN=$(az communication list-key \
        --name "$ACS_NAME" \
        --resource-group "$RG_NAME" \
        --query 'primaryConnectionString' -o tsv 2>/dev/null || echo "")
    ACS_SENDER="DoNotReply@${DOMAIN_OUTPUT}"

    echo "  ACS Sender: $ACS_SENDER"
fi

# ── Step 5: Set up the VM ────────────────────────────────────────────────────
echo "▸ Installing PostgreSQL, Nginx, dashboard on VM..."

# Build the domain list for Nginx and certbot
ALL_DOMAINS="$DOMAIN www.$DOMAIN"
if [[ -n "$EXTRA_DOMAINS" ]]; then
    IFS=',' read -ra EXTRA_ARR <<< "$EXTRA_DOMAINS"
    for d in "${EXTRA_ARR[@]}"; do
        ALL_DOMAINS="$ALL_DOMAINS $d"
    done
fi

# Build certbot domain flags
CERTBOT_DOMAINS=""
for d in $ALL_DOMAINS; do
    CERTBOT_DOMAINS="$CERTBOT_DOMAINS -d $d"
done

# Build SSO env vars for systemd
SSO_ENV=""
if [[ -n "$SSO_CLIENT_ID" ]]; then
    SSO_ENV="Environment=SSO_MICROSOFT_CLIENT_ID=${SSO_CLIENT_ID}
Environment=SSO_MICROSOFT_CLIENT_SECRET=${SSO_CLIENT_SECRET}
Environment=SSO_MICROSOFT_TENANT_ID=${SSO_TENANT_ID}"
fi

ACS_ENV=""
if [[ -n "$ACS_CONN" ]]; then
    ACS_ENV="Environment=DASHBOARD_ACS_CONNECTION_STRING=${ACS_CONN}
Environment=DASHBOARD_ACS_SENDER=${ACS_SENDER}"
fi

az vm run-command invoke \
    --resource-group "$RG_NAME" \
    --name "$VM_NAME" \
    --command-id RunShellScript \
    --scripts "
set -e

# Install packages
apt-get update -qq < /dev/null
apt-get install -y postgresql-16 nginx certbot python3-certbot-nginx < /dev/null

# Start PostgreSQL
systemctl enable postgresql
systemctl start postgresql

# Create database
sudo -u postgres psql -c \"CREATE USER dashboard WITH PASSWORD '${DB_PASSWORD}';\" 2>/dev/null || true
sudo -u postgres psql -c \"CREATE DATABASE dashboard OWNER dashboard;\" 2>/dev/null || true
sudo -u postgres psql -c \"GRANT ALL PRIVILEGES ON DATABASE dashboard TO dashboard;\" 2>/dev/null || true

# Create V001 schema (networker-tester tables)
sudo -u postgres psql -d dashboard -c \"
CREATE TABLE IF NOT EXISTS TestRun (
    RunId UUID NOT NULL PRIMARY KEY,
    StartedAt TIMESTAMPTZ NOT NULL DEFAULT now(),
    FinishedAt TIMESTAMPTZ,
    TargetUrl TEXT NOT NULL,
    TargetHost TEXT NOT NULL,
    Modes TEXT,
    TotalRuns INT NOT NULL DEFAULT 0,
    ClientVersion TEXT,
    ClientOs TEXT,
    EndpointVersion TEXT,
    ExtraJson JSONB,
    SuccessCount INT NOT NULL DEFAULT 0,
    FailureCount INT NOT NULL DEFAULT 0,
    packet_capture_json JSONB
);
CREATE TABLE IF NOT EXISTS RequestAttempt (
    AttemptId UUID NOT NULL PRIMARY KEY,
    RunId UUID NOT NULL REFERENCES TestRun(RunId),
    Protocol TEXT NOT NULL,
    SequenceNum INT NOT NULL DEFAULT 0,
    StartedAt TIMESTAMPTZ NOT NULL DEFAULT now(),
    FinishedAt TIMESTAMPTZ,
    Success BOOLEAN NOT NULL DEFAULT FALSE,
    ErrorMessage TEXT,
    RetryCount INT NOT NULL DEFAULT 0,
    ExtraJson JSONB
);
CREATE INDEX IF NOT EXISTS ix_attempt_run ON RequestAttempt(RunId);
ALTER TABLE TestRun OWNER TO dashboard;
ALTER TABLE RequestAttempt OWNER TO dashboard;
GRANT ALL ON ALL TABLES IN SCHEMA public TO dashboard;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO dashboard;
\"

# Download release binaries
cd /tmp
curl -sL ${REPO}/${RELEASE}/networker-dashboard-${TARGET}.tar.gz | tar xz
curl -sL ${REPO}/${RELEASE}/dashboard-frontend.tar.gz -o frontend.tar.gz
mkdir -p /opt/dashboard/dashboard/dist
mv networker-dashboard /opt/dashboard/
chmod +x /opt/dashboard/networker-dashboard
tar xzf frontend.tar.gz -C /opt/dashboard/dashboard/dist/

# Create systemd service
cat > /etc/systemd/system/dashboard.service << SVCEOF
[Unit]
Description=AletheDash Dashboard
After=network.target postgresql.service

[Service]
Type=simple
User=root
WorkingDirectory=/opt/dashboard
ExecStart=/opt/dashboard/networker-dashboard
Restart=always
RestartSec=5

Environment=DASHBOARD_DB_URL=postgres://dashboard:${DB_PASSWORD}@localhost:5432/dashboard
Environment=DASHBOARD_JWT_SECRET=${JWT_SECRET}
Environment=DASHBOARD_CREDENTIAL_KEY=${CREDENTIAL_KEY}
Environment=DASHBOARD_ADMIN_EMAIL=${ADMIN_EMAIL}
Environment=DASHBOARD_ADMIN_PASSWORD=${ADMIN_PASSWORD}
Environment=DASHBOARD_PORT=3000
Environment=DASHBOARD_BIND_ADDR=127.0.0.1
Environment=DASHBOARD_PUBLIC_URL=https://${DOMAIN}
Environment=DASHBOARD_STATIC_DIR=/opt/dashboard/dashboard/dist
${SSO_ENV}
${ACS_ENV}

[Install]
WantedBy=multi-user.target
SVCEOF

systemctl daemon-reload
systemctl enable dashboard
systemctl start dashboard

# Configure Nginx
cat > /etc/nginx/sites-available/dashboard << NGXEOF
server {
    listen 80;
    server_name ${ALL_DOMAINS};
    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \\\$http_upgrade;
        proxy_set_header Connection \"upgrade\";
        proxy_set_header Host \\\$host;
        proxy_set_header X-Real-IP \\\$remote_addr;
        proxy_set_header X-Forwarded-For \\\$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \\\$scheme;
        proxy_read_timeout 86400;
    }
}
NGXEOF

ln -sf /etc/nginx/sites-available/dashboard /etc/nginx/sites-enabled/
rm -f /etc/nginx/sites-enabled/default
nginx -t && systemctl reload nginx

echo 'SETUP_COMPLETE'
" -o none 2>&1

echo "  VM setup complete"

# ── Step 6: SSL Certificate ──────────────────────────────────────────────────
echo "▸ Installing SSL certificate..."
echo "  NOTE: DNS must be pointing to $VM_IP before this step works."
echo "  If this fails, run manually after DNS propagates:"
echo "    ssh azureuser@$VM_IP"
echo "    sudo certbot --nginx --non-interactive --agree-tos --email $ADMIN_EMAIL $CERTBOT_DOMAINS --redirect"
echo ""

az vm run-command invoke \
    --resource-group "$RG_NAME" \
    --name "$VM_NAME" \
    --command-id RunShellScript \
    --scripts "
certbot --nginx --non-interactive --agree-tos \
    --email ${ADMIN_EMAIL} \
    ${CERTBOT_DOMAINS} \
    --redirect 2>&1 || echo 'SSL_SKIPPED (DNS not ready — run certbot manually)'

systemctl enable certbot.timer
systemctl start certbot.timer
" -o none 2>&1 || echo "  SSL setup deferred (DNS may not be ready)"

# ── Done ─────────────────────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Deployment Complete!                                       ║"
echo "╠══════════════════════════════════════════════════════════════╣"
echo "║                                                             ║"
echo "║  URL:           https://${DOMAIN}                           "
echo "║  VM IP:         ${VM_IP}                                    "
echo "║  SSH:           azureuser@${VM_IP}                          "
echo "║                                                             ║"
echo "║  Login:         ${ADMIN_EMAIL}                              "
echo "║  Temp Password: ${ADMIN_PASSWORD}                           "
echo "║  (must change on first login)                               ║"
echo "║                                                             ║"
if [[ -n "$SSO_CLIENT_ID" ]]; then
echo "║  SSO App ID:    ${SSO_CLIENT_ID}                            "
fi
echo "║                                                             ║"
echo "║  DNS: Point these A records to ${VM_IP}:                    "
echo "║    ${DOMAIN} → ${VM_IP}                                     "
if [[ -n "$EXTRA_DOMAINS" ]]; then
    IFS=',' read -ra EXTRA_ARR <<< "$EXTRA_DOMAINS"
    for d in "${EXTRA_ARR[@]}"; do
echo "║    ${d} → ${VM_IP}                                          "
    done
fi
echo "║                                                             ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "After DNS is configured, run SSL setup if it was skipped:"
echo "  ssh azureuser@${VM_IP} sudo certbot --nginx --non-interactive --agree-tos --email ${ADMIN_EMAIL} ${CERTBOT_DOMAINS} --redirect"
