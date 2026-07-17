# Networker Setup Guide

Complete guide to deploy and configure Networker (the dashboard/control plane) from scratch. Covers infrastructure, SSO, cloud federation, email, and ongoing operations.

> **Naming:** the product brand is **Networker** (see
> [`branding.md`](branding.md)). The `alethedash*` names below (domain,
> resource group, VM, systemd service, DB, `/opt`/`/etc` paths) are the
> current production deployment's identifiers — kept as-is because renaming
> them is an ops migration, not a doc change.

> **Current stack:** production runs the **C# control plane**
> (`Networker.ControlPlane`, systemd service `alethedash-cs`, port `5030`
> behind nginx) — not the retired Rust `networker-dashboard`. The
> manual-setup and updating sections below install the C# stack. Where a
> section still shows Rust-era details (env-var SSO configuration, the
> `alethedash` service name) it is marked **[LEGACY]**. Ongoing operations:
> [`phase2-cutover-runbook.md`](phase2-cutover-runbook.md); release/deploy
> mechanics: [`release-flow.md`](release-flow.md).

**Time estimate:** ~30 minutes for a fresh deployment.

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Quick Deploy (One Command)](#2-quick-deploy)
3. [Manual Setup](#3-manual-setup)
4. [Configure Microsoft SSO](#4-configure-microsoft-sso)
5. [Configure Google SSO](#5-configure-google-sso)
6. [Configure Azure Communication Services (Email)](#6-configure-email)
7. [Configure AWS Federation](#7-configure-aws-federation)
8. [Configure GCP Federation](#8-configure-gcp-federation)
9. [Add Cloud Connections in the UI](#9-add-cloud-connections)
10. [Custom Domain + SSL](#10-custom-domain--ssl)
11. [Backups](#11-backups)
12. [Disaster Recovery](#12-disaster-recovery)
13. [Updating](#13-updating)
14. [Environment Variables Reference](#14-environment-variables)
15. [Troubleshooting](#15-troubleshooting)

---

## 1. Prerequisites

- **Azure CLI** (`az`) installed and logged in: `az login`
- **Azure subscription** with Contributor access
- **Domain name** (optional but recommended for SSL + SSO)
- **AWS CLI** (`aws`) — only if using AWS federation
- **Google Cloud CLI** (`gcloud`) — only if using GCP federation

```bash
# Verify Azure CLI
az account show --query '{name: name, id: id}' -o table

# Set the target subscription
az account set --subscription <subscription-id>
```

---

## 2. Quick Deploy

> **[LEGACY — script installs the retired Rust stack]**
> `scripts/deploy-dashboard.sh` predates the C# cutover: it still downloads
> the `networker-dashboard` binary (no longer published in current releases)
> and creates the Rust-era `alethedash` service. Its infrastructure steps
> (VM, PostgreSQL, nginx, SSL, SSO app registration, ACS) remain a useful
> reference, but for the control plane itself follow
> [Manual Setup](#3-manual-setup) below.

The one-command path — creates everything in ~5 minutes:

```bash
./scripts/deploy-dashboard.sh \
  --domain alethedash.com \
  --admin-email admin@yourcompany.com \
  --location eastus \
  --release latest
```

This creates:
- Azure VM (B1s Ubuntu 24.04)
- PostgreSQL 16 on the VM
- Nginx reverse proxy
- Let's Encrypt SSL certificate
- Microsoft SSO app registration
- Azure Communication Services for email
- Dashboard binary + frontend from GitHub release
- Systemd service with all env vars

**After running:**
1. Point your DNS A records to the IP printed at the end
2. Log in with the admin email + temp password (printed at the end)
3. Change your password on first login

**Options:**
```bash
--extra-domains "example.net,example.info"  # Additional domains (redirect to primary)
--vm-size Standard_B2s                       # Bigger VM
--skip-sso                                   # Skip Microsoft SSO setup
--skip-email                                 # Skip ACS email setup
--dry-run                                    # Preview without creating anything
```

---

## 3. Manual Setup

If you prefer to set up each component individually:

### 3.1 Create Resource Group + VM

```bash
az group create --name alethedash-rg --location eastus

az vm create \
  --resource-group alethedash-rg \
  --name alethedash-vm \
  --image Ubuntu2404 \
  --size Standard_B1s \
  --admin-username azureuser \
  --generate-ssh-keys \
  --public-ip-sku Standard

# Open HTTP/HTTPS ports
az network nsg rule create \
  --resource-group alethedash-rg \
  --nsg-name alethedash-vmNSG \
  --name AllowHTTP --priority 100 \
  --destination-port-ranges 80 443 --protocol Tcp --access Allow
```

### 3.2 Assign Managed Identity

```bash
az vm identity assign \
  --resource-group alethedash-rg \
  --name alethedash-vm

# Grant Contributor on the subscription (for VM management)
PRINCIPAL_ID=$(az vm identity show -g alethedash-rg -n alethedash-vm --query principalId -o tsv)
az role assignment create \
  --assignee $PRINCIPAL_ID \
  --role Contributor \
  --scope /subscriptions/$(az account show --query id -o tsv)
```

### 3.3 Install on the VM

SSH into the VM and run:

```bash
ssh azureuser@<vm-ip>

# Install PostgreSQL + Nginx
sudo apt-get update && sudo apt-get install -y postgresql-16 nginx certbot python3-certbot-nginx

# Create database
sudo -u postgres psql -c "CREATE USER alethedash WITH PASSWORD '<db-password>';"
sudo -u postgres psql -c "CREATE DATABASE alethedash OWNER alethedash;"

# Download latest release: the C# control plane + the built frontend
cd /tmp
curl -fsSL https://github.com/irlm/networker-tester/releases/latest/download/networker-controlplane-linux-x64.tar.gz -o controlplane.tar.gz
curl -fsSL https://github.com/irlm/networker-tester/releases/latest/download/dashboard-frontend.tar.gz -o frontend.tar.gz

# Control plane → /opt/alethedash-cs (self-contained; no .NET runtime install needed)
sudo mkdir -p /opt/alethedash-cs
sudo tar xzf controlplane.tar.gz -C /opt/alethedash-cs
sudo chmod +x /opt/alethedash-cs/Networker.ControlPlane

# Frontend → served static by nginx
sudo mkdir -p /opt/alethedash/dashboard/dist
sudo tar xzf frontend.tar.gz -C /opt/alethedash/dashboard/dist/
sudo chmod -R a+rX /opt/alethedash/dashboard/dist
```

### 3.4 Create Systemd Service

The control plane reads its configuration from `/etc/alethedash-cs.env`
(the full variable reference is
[`phase2-cutover-runbook.md`](phase2-cutover-runbook.md) §1.1):

```bash
sudo tee /etc/alethedash-cs.env > /dev/null << EOF
ASPNETCORE_ENVIRONMENT=Production
ASPNETCORE_URLS=http://127.0.0.1:5030
DASHBOARD_DB_URL_NPGSQL=Host=localhost;Port=5432;Database=alethedash;Username=alethedash;Password=<db-password>
DASHBOARD_JWT_SECRET=$(openssl rand -base64 32)
DASHBOARD_CREDENTIAL_KEY=$(openssl rand -hex 32)
DASHBOARD_PUBLIC_URL=https://yourdomain.com
EOF
sudo chmod 600 /etc/alethedash-cs.env

sudo tee /etc/systemd/system/alethedash-cs.service << 'EOF'
[Unit]
Description=Networker Control Plane (C#)
After=network.target postgresql.service

[Service]
Type=simple
User=root
WorkingDirectory=/opt/alethedash-cs
ExecStart=/opt/alethedash-cs/Networker.ControlPlane
EnvironmentFile=/etc/alethedash-cs.env
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable alethedash-cs
sudo systemctl start alethedash-cs

# Readiness probe
curl -sf http://localhost:5030/api/health/ready
```

`DASHBOARD_JWT_SECRET` and `DASHBOARD_CREDENTIAL_KEY` are **fail-closed**
outside Development — the service refuses to start without them.
`DASHBOARD_PUBLIC_URL` is required for tester provisioning (without it, new
tester agents are pointed at `ws://localhost:3000` and never come online).

### 3.5 Configure Nginx

nginx serves the SPA static from disk and proxies only `/api` + `/ws` to the
control plane on `:5030`:

```bash
sudo tee /etc/nginx/sites-available/alethedash << 'EOF'
server {
    listen 80;
    server_name yourdomain.com;

    root /opt/alethedash/dashboard/dist;
    index index.html;

    # SPA: serve static files, fall back to index.html for client routes
    location / {
        try_files $uri $uri/ /index.html;
    }

    # API + WebSockets → C# control plane
    location ~ ^/(api|ws)/ {
        proxy_pass http://127.0.0.1:5030;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 86400;
    }
}
EOF

sudo ln -sf /etc/nginx/sites-available/alethedash /etc/nginx/sites-enabled/
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t && sudo systemctl reload nginx
```

---

## 4. Configure Microsoft SSO

Microsoft SSO uses Azure AD (Entra ID). The deploy script does this automatically, but here's the manual process:

### 4.1 Create App Registration

```bash
# Create the app
az ad app create \
  --display-name "Networker" \
  --web-redirect-uris "https://yourdomain.com/api/auth/sso/callback" \
  --sign-in-audience AzureADMyOrg

# Note the App ID from the output
APP_ID=<app-id-from-output>

# Create a service principal
az ad sp create --id $APP_ID

# Create a client secret (valid 2 years)
az ad app credential reset --id $APP_ID --append --years 2
# Note the password (client secret) and tenant from the output
```

### 4.2 Grant Permissions

```bash
# Add OpenID permissions
az ad app permission add --id $APP_ID \
  --api 00000003-0000-0000-c000-000000000000 \
  --api-permissions \
    e1fe6dd8-ba31-4d61-89e7-88639da4683d=Scope \
    37f7f235-527c-4136-accd-4a02d197296e=Scope \
    14dad69e-099b-42c9-810b-d002981feec1=Scope

# Grant admin consent
az ad app permission grant --id $APP_ID \
  --api 00000003-0000-0000-c000-000000000000 \
  --scope "openid email profile"
```

### 4.3 Register the Provider

In the **C# control plane**, SSO providers are stored in the database (with
the client secret encrypted at rest) and managed through the platform-admin
UI or the `/api/sso-providers` admin API — no service restart needed. Enter
the App ID, client secret, and tenant ID from 4.1 there.

**[LEGACY — Rust dashboard only]** The old env-var route
(`SSO_MICROSOFT_CLIENT_ID` / `SSO_MICROSOFT_CLIENT_SECRET` /
`SSO_MICROSOFT_TENANT_ID` in the `alethedash` unit) applies only to the
retired Rust service.

### 4.4 Verify

The login page should show "Continue with Microsoft." Click it to test.

---

## 5. Configure Google SSO

### 5.1 Create OAuth Consent Screen

1. Go to https://console.cloud.google.com/auth/overview?project=YOUR_PROJECT
2. Click **"Get Started"** or **"Configure consent screen"**
3. Fill in:
   - App name: **Networker**
   - Support email: your email
   - Audience: **External**
4. Save

### 5.2 Create OAuth Client

1. Go to **Clients** tab (or https://console.cloud.google.com/apis/credentials?project=YOUR_PROJECT)
2. Click **"Create Credentials"** → **"OAuth client ID"**
3. Application type: **Web application**
4. Name: **Networker**
5. Authorized redirect URI: `https://yourdomain.com/api/auth/sso/callback`
6. Click **Create**
7. Copy the **Client ID** and **Client Secret**

### 5.3 Register the Provider

As with Microsoft (see 4.3): add the Google client ID + secret through the
platform-admin UI or the `/api/sso-providers` admin API — stored encrypted in
the database, no restart needed.

**[LEGACY — Rust dashboard only]** `SSO_GOOGLE_CLIENT_ID` /
`SSO_GOOGLE_CLIENT_SECRET` env vars applied only to the retired Rust service.

### 5.4 Verify

The login page should show both "Continue with Microsoft" and "Continue with Google."

---

## 6. Configure Email

Networker uses Azure Communication Services for password reset and invite emails.

### 6.1 Create ACS Resource

```bash
az communication create \
  --name alethedash-comm \
  --resource-group alethedash-rg \
  --location global \
  --data-location unitedstates

az communication email create \
  --name alethedash-email \
  --resource-group alethedash-rg \
  --location global \
  --data-location unitedstates

# Create Azure-managed email domain (no DNS setup needed)
az communication email domain create \
  --name AzureManagedDomain \
  --email-service-name alethedash-email \
  --resource-group alethedash-rg \
  --location global \
  --domain-management AzureManaged
```

Note the `fromSenderDomain` from the output (e.g., `85e30427-...azurecomm.net`).

### 6.2 Link Email Domain

```bash
SUBSCRIPTION_ID=$(az account show --query id -o tsv)
az rest --method patch \
  --uri "/subscriptions/$SUBSCRIPTION_ID/resourceGroups/alethedash-rg/providers/Microsoft.Communication/communicationServices/alethedash-comm?api-version=2023-04-01" \
  --body "{\"properties\":{\"linkedDomains\":[\"/subscriptions/$SUBSCRIPTION_ID/resourceGroups/alethedash-rg/providers/Microsoft.Communication/emailServices/alethedash-email/domains/AzureManagedDomain\"]}}"
```

### 6.3 Get Connection String

```bash
az communication list-key \
  --name alethedash-comm \
  --resource-group alethedash-rg \
  --query primaryConnectionString -o tsv
```

### 6.4 Add Environment Variables

Add to `/etc/alethedash-cs.env` (the C# control plane reads the same names
the Rust dashboard used):

```bash
DASHBOARD_ACS_CONNECTION_STRING=endpoint=https://...;accesskey=...
DASHBOARD_ACS_SENDER=DoNotReply@<domain>.azurecomm.net
```

Then: `sudo systemctl restart alethedash-cs`

**Note:** If ACS is not configured, password reset links are logged to the server console instead of emailed. Check logs with: `journalctl -u alethedash-cs | grep "PASSWORD RESET LINK"`

---

## 7. Configure AWS Federation

Allows the dashboard to manage AWS EC2 instances **without storing any AWS credentials**. The Azure managed identity gets temporary AWS credentials via OIDC token exchange.

### 7.1 Prerequisites

- AWS CLI authenticated on your local machine (`aws sts get-caller-identity`)
- Azure CLI authenticated (`az account show`)

### 7.2 Run the Setup Script

```bash
export AWS_ACCOUNT_ID="123456789012"  # Your AWS account ID
bash scripts/setup-aws-federation.sh
```

This creates:
- Azure AD App Registration (token audience)
- AWS OIDC Identity Provider (trusts Azure AD)
- AWS IAM Role with least-privilege EC2 permissions
- Credential helper script for the VM

### 7.3 Deploy to the VM

```bash
# Copy the credential helper
scp /tmp/networker-aws-credential-helper.sh azureuser@<vm-ip>:/tmp/

# On the VM (or via az vm run-command):
sudo mv /tmp/networker-aws-credential-helper.sh /usr/local/bin/
sudo chmod 755 /usr/local/bin/networker-aws-credential-helper.sh

# Configure AWS CLI
sudo mkdir -p /root/.aws
sudo tee /root/.aws/config << EOF
[default]
region = us-east-1
credential_process = /usr/local/bin/networker-aws-credential-helper.sh
EOF
```

### 7.4 Verify

```bash
# On the VM:
aws sts get-caller-identity
# Should show: assumed-role/networker-dashboard-role/networker-dashboard
```

---

## 8. Configure GCP Federation

Allows the dashboard to manage GCP Compute Engine instances **without storing any service account keys**.

### 8.1 Prerequisites

- Google Cloud CLI authenticated (`gcloud auth list`)
- Azure CLI authenticated (`az account show`)
- AWS federation already set up (reuses the same Azure AD App Registration)

### 8.2 Run the Setup Script

```bash
export GCP_PROJECT_ID="your-project-id"
export AZURE_APP_ID="<app-id-from-aws-federation>"  # Reuses the same app
bash scripts/setup-gcp-federation.sh
```

This creates:
- Workload Identity Pool in GCP
- OIDC Provider (trusts Azure AD)
- Service Account with Compute Engine permissions
- Credential configuration file for the VM

### 8.3 Deploy to the VM

```bash
# Copy the credential config
scp /tmp/networker-gcp-credential-config.json azureuser@<vm-ip>:/tmp/

# On the VM:
sudo mv /tmp/networker-gcp-credential-config.json /etc/networker-gcp-credentials.json
sudo chmod 644 /etc/networker-gcp-credentials.json

# Install gcloud CLI
curl -sL https://dl.google.com/dl/cloudsdk/channels/rapid/downloads/google-cloud-cli-linux-x86_64.tar.gz | sudo tar xz -C /opt/
sudo ln -sf /opt/google-cloud-sdk/bin/gcloud /usr/local/bin/gcloud

# Add to systemd service:
Environment=GOOGLE_APPLICATION_CREDENTIALS=/etc/networker-gcp-credentials.json
```

### 8.4 Verify

```bash
# On the VM:
export GOOGLE_APPLICATION_CREDENTIALS=/etc/networker-gcp-credentials.json
gcloud auth list
# Should show: networker-dashboard@<project>.iam.gserviceaccount.com
```

---

## 9. Add Cloud Connections

After federation is configured, add the connections in the dashboard UI:

1. Log in as admin
2. Go to **Settings** → scroll to **Cloud Accounts**
3. Click **"+ Add Account"**

### Azure
- Provider: **Azure**
- Subscription ID: your Azure subscription ID
- Click Add → auto-validates via managed identity

### AWS
- Provider: **AWS**
- Account ID: your AWS account ID
- Role ARN: `arn:aws:iam::<account-id>:role/networker-dashboard-role`
- Click Add → validates via federation

### GCP
- Provider: **GCP**
- Project ID: your GCP project ID
- Click Add → validates via workload identity

---

## 10. Custom Domain + SSL

### 10.1 DNS Configuration

Add A records at your domain registrar:

| Domain | Type | Name | Value |
|--------|------|------|-------|
| `yourdomain.com` | A | `@` | `<vm-ip>` |
| `yourdomain.com` | CNAME | `www` | `yourdomain.com` |

### 10.2 SSL Certificate

```bash
# On the VM:
sudo certbot --nginx --non-interactive --agree-tos \
  --email admin@yourdomain.com \
  -d yourdomain.com -d www.yourdomain.com \
  --redirect

# Auto-renewal is enabled automatically
sudo systemctl enable certbot.timer
```

### 10.3 Redirect Additional Domains

If you have `.net` / `.info` domains, add them to Nginx:

```nginx
server {
    listen 443 ssl;
    server_name yourdomain.net yourdomain.info;
    ssl_certificate /etc/letsencrypt/live/yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/yourdomain.com/privkey.pem;
    return 301 https://yourdomain.com$request_uri;
}
```

---

## 11. Backups

### Automatic Daily Backups

Already configured by the deploy script. Runs at 2 AM UTC via cron:
- **Local:** `/opt/backups/backup-YYYY-MM-DD.tar.gz` (30-day retention)
- **Azure Blob Storage:** `alethedashbackups/backups/` container

### Manual Backup

```bash
# From your machine (via SSH):
./scripts/backup-dashboard.sh --host <vm-ip> --output ./backups/

# Or upload to Azure Blob Storage:
./scripts/backup-dashboard.sh --host <vm-ip> --blob-container backups --storage-account alethedashbackups
```

### What's Backed Up

- PostgreSQL full dump (all users, tests, results, schedules)
- Systemd service config (all env vars including secrets)
- Nginx configuration
- SSL certificates

---

## 12. Disaster Recovery

Full restore from backup to a new VM:

```bash
./scripts/restore-dashboard.sh \
  --backup ./backups/backup-2026-03-22.tar.gz \
  --domain yourdomain.com \
  --location eastus
```

This creates a new VM, restores the database + config, downloads the matching dashboard release, and starts everything. Then:

1. Update DNS A records to the new VM IP
2. Run certbot for SSL
3. Verify at `https://yourdomain.com`

**Recovery time:** ~10 minutes.

---

## 13. Updating

### Automatic (the normal path)

Production updates itself: every release tag triggers the Release workflow,
whose `deploy` job ships the new control plane, endpoint, tester, and
frontend to the VM, health-checks it, and auto-rolls-back on failure. See
[`release-flow.md`](release-flow.md). To re-ship a specific version:

```bash
gh workflow run release.yml --field tag=vX.Y.Z
```

### Manual Update (on the VM)

Mirrors what the deploy job does:

```bash
TAG=v0.28.32  # or the tag you want
cd /tmp
curl -fsSL https://github.com/irlm/networker-tester/releases/download/$TAG/networker-controlplane-linux-x64.tar.gz -o controlplane.tar.gz
curl -fsSL https://github.com/irlm/networker-tester/releases/download/$TAG/dashboard-frontend.tar.gz -o frontend.tar.gz

sudo systemctl stop alethedash-cs
sudo rm -rf /opt/alethedash-cs.prevbuild
sudo mv /opt/alethedash-cs /opt/alethedash-cs.prevbuild   # rollback copy
sudo mkdir -p /opt/alethedash-cs
sudo tar xzf controlplane.tar.gz -C /opt/alethedash-cs
sudo chmod +x /opt/alethedash-cs/Networker.ControlPlane

sudo rm -rf /opt/alethedash/dashboard/dist/*
sudo tar xzf frontend.tar.gz -C /opt/alethedash/dashboard/dist/

sudo systemctl start alethedash-cs
curl -sf http://localhost:5030/api/health/ready
```

If the readiness probe fails, restore `/opt/alethedash-cs.prevbuild` and
restart.

---

## 14. Environment Variables

The authoritative reference for the C# control plane is
[`phase2-cutover-runbook.md`](phase2-cutover-runbook.md) §1.1. Summary:

### Required (fail-closed outside Development)

| Variable | Description | Example |
|----------|-------------|---------|
| `DASHBOARD_JWT_SECRET` | HS256 JWT signing key (32+ bytes) | `openssl rand -base64 32` |
| `DASHBOARD_CREDENTIAL_KEY` | 64-hex AEAD key for cloud-account secrets | `openssl rand -hex 32` |
| `DASHBOARD_DB_URL_NPGSQL` (or `ConnectionStrings__Networker`) | Npgsql connection string | `Host=localhost;Port=5432;Database=alethedash;Username=alethedash;Password=…` |
| `ASPNETCORE_ENVIRONMENT` | `Production` in prod (unset counts as Production) | `Production` |

### Server

| Variable | Default | Description |
|----------|---------|-------------|
| `ASPNETCORE_URLS` | `http://localhost:5000` | Listen address; prod uses `http://127.0.0.1:5030` behind nginx |
| `DASHBOARD_PUBLIC_URL` | — | Public base URL — **required for tester provisioning** (derives the agent WebSocket URL) and SSO callbacks |
| `DASHBOARD_BACKGROUND_SERVICES` | on | Set `0` for an API-only replica (no scheduler/watchdog/reaper loops) |

### SSO

SSO providers are configured in the database via the platform-admin UI /
`/api/sso-providers` (see sections 4–5), not env vars.
**[LEGACY]** `SSO_MICROSOFT_*` / `SSO_GOOGLE_*` env vars applied only to the
retired Rust dashboard, as did `DASHBOARD_DB_URL`, `DASHBOARD_PORT`,
`DASHBOARD_BIND_ADDR`, `DASHBOARD_STATIC_DIR`, and `INSTALL_SH_PATH`.

### Email (Azure Communication Services)

| Variable | Description |
|----------|-------------|
| `DASHBOARD_ACS_CONNECTION_STRING` | ACS connection string |
| `DASHBOARD_ACS_SENDER` | Sender email address |

### Cloud Federation

| Variable | Description |
|----------|-------------|
| `GOOGLE_APPLICATION_CREDENTIALS` | Path to GCP credential config JSON |

### Optional

| Variable | Default | Description |
|----------|---------|-------------|
| `DASHBOARD_HIDE_SSO_DOMAINS` | `false` | Hide SSO domain routing in check-email |
| `DASHBOARD_CORS_ORIGIN` | `http://localhost:5173` | CORS allowed origin |

---

## 15. Troubleshooting

### Control plane won't start

```bash
# Check logs
journalctl -u alethedash-cs --no-pager -n 50

# Common issues:
# - "DASHBOARD_JWT_SECRET must be set" → Add the env var
# - "db error: relation testrun does not exist" → Run the V001 schema (see manual setup)
# - "must be owner of table" → Fix DB permissions:
#   sudo -u postgres psql -d alethedash -c "GRANT ALL ON ALL TABLES IN SCHEMA public TO alethedash;"
```

### SSO login fails

```bash
# "AADSTS50011: redirect URI mismatch" → Update the app registration:
az ad app update --id <app-id> --web-redirect-uris "https://yourdomain.com/api/auth/sso/callback"

# "id_token_invalid" → Check the tenant ID on the SSO provider record matches your tenant

# Google "redirect_uri_mismatch" → Add the URI in Google Cloud Console
```

### AWS federation fails

```bash
# "NoCredentials" → Check the credential helper:
/usr/local/bin/networker-aws-credential-helper.sh
# Should output JSON with AccessKeyId, SecretAccessKey, SessionToken

# "InvalidIdentityToken" → The Azure AD App Registration may be wrong
# Verify: az ad app show --id <app-id>
```

### Tester VM deploy fails

```bash
# Check logs
journalctl -u alethedash-cs | grep "deploy-vm\|VM deployment"

# "InvalidResourceGroupLocation" → Per-region RG already exists in different region
# The script creates networker-testers-{region}-rg per region

# "No credential providers" on the tester VM → Managed identity not assigned
```

### Browser tests fail

```bash
# "Chrome not found" → Install Chrome on the tester VM:
# The provisioning bootstrap installs it automatically; if a tester
# predates that, install manually:
sudo apt-get install -y google-chrome-stable
```

### Backup restore fails

```bash
# "role alethedash does not exist" → Create the DB user first:
sudo -u postgres psql -c "CREATE USER alethedash WITH PASSWORD '<password>';"
```
