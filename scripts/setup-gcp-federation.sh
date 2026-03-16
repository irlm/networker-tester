#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# GCP Workload Identity Federation Setup for Networker Dashboard
# ═══════════════════════════════════════════════════════════════════════════
#
# Run this script on a machine with BOTH Azure CLI and gcloud CLI authenticated.
# It creates/reuses an Azure AD App Registration (same one the AWS script
# creates) and sets up GCP Workload Identity Federation so the Azure
# dashboard VM can manage GCE instances WITHOUT any stored GCP credentials.
#
# Usage:
#   export GCP_PROJECT_ID="my-project-123"
#   bash scripts/setup-gcp-federation.sh
#
# Optional env vars:
#   AZURE_TENANT_ID    — auto-detected from `az account show` if unset
#   APP_DISPLAY_NAME   — defaults to "networker-dashboard-federation"
#   POOL_ID            — defaults to "networker-azure-pool"
#   PROVIDER_ID        — defaults to "azure-ad"
#   SA_NAME            — defaults to "networker-dashboard"
#
# What it creates:
#   Azure:
#     1. Azure AD App Registration (token audience — reuses if already exists)
#   GCP:
#     2. Workload Identity Pool "networker-azure-pool"
#     3. OIDC Provider in the pool (trusts Azure AD, audience = App ID)
#     4. Service Account "networker-dashboard@<project>.iam"
#     5. Binds the Azure MI principal to the service account
#     6. Credential config file for the VM
#
# How it works:
#   Azure VM requests token from IMDS with resource=<APP_REGISTRATION_APP_ID>
#   → token has aud=<APP_ID>, sub=<MI_PRINCIPAL_ID>, iss=sts.windows.net/<TENANT>/
#   → GCP STS validates token via OIDC discovery
#   → GCP checks aud matches allowed-audiences on the provider
#   → GCP exchanges for a GCP access token (no JSON key files ever)
#
# Key lesson: The credential_source URL in the generated config must use
# resource=<APP_REGISTRATION_APP_ID>, NOT the MI's own client ID.
# ═══════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────
APP_DISPLAY_NAME="${APP_DISPLAY_NAME:-networker-dashboard-federation}"
POOL_ID="${POOL_ID:-networker-azure-pool}"
PROVIDER_ID="${PROVIDER_ID:-azure-ad}"
SA_NAME="${SA_NAME:-networker-dashboard}"

# Auto-detect Azure Tenant ID if not set
if [[ -z "${AZURE_TENANT_ID:-}" ]]; then
    echo "Detecting Azure Tenant ID from 'az account show'..."
    AZURE_TENANT_ID="$(az account show --query tenantId -o tsv)"
    if [[ -z "$AZURE_TENANT_ID" ]]; then
        echo "ERROR: Could not detect Azure Tenant ID. Run 'az login' or set AZURE_TENANT_ID."
        exit 1
    fi
fi

OIDC_ISSUER="https://sts.windows.net/${AZURE_TENANT_ID}/"

if [[ -z "${GCP_PROJECT_ID:-}" ]]; then
    echo "ERROR: Set GCP_PROJECT_ID first:"
    echo "  export GCP_PROJECT_ID=\"my-project-123\""
    echo "  bash $0"
    exit 1
fi

echo "Setting up GCP Workload Identity Federation for Networker Dashboard"
echo "  Azure Tenant:     ${AZURE_TENANT_ID}"
echo "  OIDC Issuer:      ${OIDC_ISSUER}"
echo "  GCP Project:      ${GCP_PROJECT_ID}"
echo ""

# ── Step 1: Create/reuse Azure AD App Registration ───────────────────────
echo "Step 1: Creating Azure AD App Registration (token audience)..."

EXISTING_APP_ID="$(az ad app list --display-name "${APP_DISPLAY_NAME}" --query '[0].appId' -o tsv 2>/dev/null || true)"

if [[ -n "$EXISTING_APP_ID" && "$EXISTING_APP_ID" != "None" ]]; then
    APP_ID="$EXISTING_APP_ID"
    echo "  App Registration already exists: ${APP_ID}"
else
    APP_ID="$(az ad app create \
        --display-name "${APP_DISPLAY_NAME}" \
        --sign-in-audience AzureADMyOrg \
        --query appId -o tsv)"
    echo "  Created App Registration: ${APP_ID}"
fi

echo "  APP_ID=${APP_ID}"
echo ""

# Get the MI principal ID (needed for the service account binding)
echo "  Detecting managed identity principal ID..."
MI_PRINCIPAL_ID="$(az vm list --query "[?name=='networker-dashboard'].identity.principalId | [0]" -o tsv 2>/dev/null || true)"

if [[ -z "$MI_PRINCIPAL_ID" || "$MI_PRINCIPAL_ID" == "None" ]]; then
    echo "  WARNING: Could not auto-detect MI principal ID from VM list."
    echo "  You can set it manually if needed for the service account binding."
    echo "  Falling back to using the App ID for the subject binding."
    MI_PRINCIPAL_ID=""
fi

# Set the project
gcloud config set project "${GCP_PROJECT_ID}" --quiet

# ── Step 2: Enable required APIs ─────────────────────────────────────────
echo "Step 2: Enabling required APIs..."
gcloud services enable \
    iam.googleapis.com \
    iamcredentials.googleapis.com \
    sts.googleapis.com \
    compute.googleapis.com \
    cloudresourcemanager.googleapis.com \
    --quiet

# ── Step 3: Create Workload Identity Pool ────────────────────────────────
echo "Step 3: Creating Workload Identity Pool..."

if gcloud iam workload-identity-pools describe "${POOL_ID}" \
    --location="global" &>/dev/null; then
    echo "  Pool already exists"
else
    gcloud iam workload-identity-pools create "${POOL_ID}" \
        --location="global" \
        --display-name="Networker Azure Federation" \
        --description="Allows Azure VM managed identity to access GCP"
    echo "  Created pool"
fi

# ── Step 4: Create OIDC Provider in the pool ─────────────────────────────
echo "Step 4: Creating OIDC Provider..."

# IMPORTANT: issuer-uri must be sts.windows.net/<TENANT_ID>/ (NOT login.microsoftonline.com)
# IMPORTANT: allowed-audiences must be the APP_ID (NOT the MI client ID)
if gcloud iam workload-identity-pools providers describe "${PROVIDER_ID}" \
    --workload-identity-pool="${POOL_ID}" \
    --location="global" &>/dev/null; then
    echo "  Provider already exists, updating..."
    gcloud iam workload-identity-pools providers update-oidc "${PROVIDER_ID}" \
        --workload-identity-pool="${POOL_ID}" \
        --location="global" \
        --issuer-uri="${OIDC_ISSUER}" \
        --allowed-audiences="${APP_ID}" \
        --attribute-mapping="google.subject=assertion.sub,attribute.tid=assertion.tid"
else
    gcloud iam workload-identity-pools providers create-oidc "${PROVIDER_ID}" \
        --workload-identity-pool="${POOL_ID}" \
        --location="global" \
        --issuer-uri="${OIDC_ISSUER}" \
        --allowed-audiences="${APP_ID}" \
        --attribute-mapping="google.subject=assertion.sub,attribute.tid=assertion.tid"
    echo "  Created provider"
fi

# ── Step 5: Create Service Account ───────────────────────────────────────
echo "Step 5: Creating service account..."

SA_EMAIL="${SA_NAME}@${GCP_PROJECT_ID}.iam.gserviceaccount.com"

if gcloud iam service-accounts describe "${SA_EMAIL}" &>/dev/null; then
    echo "  Service account already exists"
else
    gcloud iam service-accounts create "${SA_NAME}" \
        --display-name="Networker Dashboard (Azure federated)" \
        --description="Used by Azure-hosted dashboard to manage GCE instances"
    echo "  Created service account"
fi

# Grant Compute Instance Admin role (least privilege for VM management)
echo "  Granting Compute Instance Admin role..."
gcloud projects add-iam-policy-binding "${GCP_PROJECT_ID}" \
    --member="serviceAccount:${SA_EMAIL}" \
    --role="roles/compute.instanceAdmin.v1" \
    --condition=None \
    --quiet &>/dev/null

# ── Step 6: Bind Azure identity to GCP service account ──────────────────
echo "Step 6: Binding Azure managed identity to GCP service account..."

PROJECT_NUMBER=$(gcloud projects describe "${GCP_PROJECT_ID}" --format='value(projectNumber)')

# The subject in the token is the MI's principal ID (object ID in Azure AD).
# We bind using subject/<MI_PRINCIPAL_ID> so GCP maps the Azure identity
# to this service account.
if [[ -n "$MI_PRINCIPAL_ID" ]]; then
    MEMBER="principal://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${POOL_ID}/subject/${MI_PRINCIPAL_ID}"
    echo "  Binding subject: ${MI_PRINCIPAL_ID}"
else
    echo "  WARNING: No MI principal ID detected. You must manually bind:"
    echo "    gcloud iam service-accounts add-iam-policy-binding ${SA_EMAIL} \\"
    echo "      --member='principal://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${POOL_ID}/subject/<MI_PRINCIPAL_ID>' \\"
    echo "      --role='roles/iam.workloadIdentityUser'"
    MEMBER=""
fi

if [[ -n "$MEMBER" ]]; then
    gcloud iam service-accounts add-iam-policy-binding "${SA_EMAIL}" \
        --member="${MEMBER}" \
        --role="roles/iam.workloadIdentityUser" \
        --quiet &>/dev/null
    echo "  Bound Azure MI → ${SA_EMAIL}"
fi

# ── Step 7: Generate credential config file ──────────────────────────────
echo "Step 7: Generating credential configuration..."

CRED_CONFIG_PATH="/tmp/networker-gcp-credential-config.json"

# IMPORTANT: --app-id-uri must be the APP_ID (App Registration), NOT the MI client ID.
# This controls the "resource" parameter in the IMDS token request URL.
gcloud iam workload-identity-pools create-cred-config \
    "projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${POOL_ID}/providers/${PROVIDER_ID}" \
    --service-account="${SA_EMAIL}" \
    --output-file="${CRED_CONFIG_PATH}" \
    --azure \
    --app-id-uri="${APP_ID}"

echo "  Config written to: ${CRED_CONFIG_PATH}"

# Verify the credential_source URL uses the correct APP_ID
echo ""
echo "  Verifying credential config uses correct audience..."
if python3 -c "
import json, sys
with open('${CRED_CONFIG_PATH}') as f:
    cfg = json.load(f)
url = cfg.get('credential_source', {}).get('url', '')
if '${APP_ID}' in url:
    print('  OK: credential_source URL contains APP_ID')
else:
    print('  WARNING: credential_source URL may not contain correct APP_ID')
    print('  URL:', url)
    sys.exit(1)
" 2>/dev/null; then
    true
else
    echo "  (Could not verify — check ${CRED_CONFIG_PATH} manually)"
fi

# ── Done ─────────────────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════════════════════════════╗"
echo "║  GCP Workload Identity Federation configured!                      ║"
echo "║                                                                    ║"
echo "║  Azure AD App Registration: ${APP_ID}    ║"
echo "║                                                                    ║"
echo "║  1. Copy credential config to the dashboard VM:                    ║"
echo "║     scp ${CRED_CONFIG_PATH} \\             ║"
echo "║       azureuser@<dashboard-vm>:/tmp/                               ║"
echo "║                                                                    ║"
echo "║  2. On the VM:                                                     ║"
echo "║     sudo mv /tmp/networker-gcp-credential-config.json \\           ║"
echo "║       /etc/networker-gcp-credentials.json                          ║"
echo "║     sudo chmod 600 /etc/networker-gcp-credentials.json             ║"
echo "║     sudo chown networker:networker /etc/networker-gcp-credentials.json ║"
echo "║                                                                    ║"
echo "║  3. Add to dashboard env:                                          ║"
echo "║     echo 'GOOGLE_APPLICATION_CREDENTIALS=/etc/networker-gcp-credentials.json' \\ ║"
echo "║       | sudo tee -a /etc/networker-dashboard.env                   ║"
printf "║     echo 'GCP_PROJECT_ID=%s' \\\\\n" "${GCP_PROJECT_ID}"
echo "║       | sudo tee -a /etc/networker-dashboard.env                   ║"
echo "║     sudo systemctl restart networker-dashboard                     ║"
echo "╚══════════════════════════════════════════════════════════════════════╝"
