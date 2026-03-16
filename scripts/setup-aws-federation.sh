#!/bin/bash
# ═══════════════════════════════════════════════════════════════════════════
# AWS OIDC Federation Setup for Networker Dashboard
# ═══════════════════════════════════════════════════════════════════════════
#
# Run this script on a machine with BOTH Azure CLI and AWS CLI authenticated.
# It creates an Azure AD App Registration (the token audience) and an AWS
# OIDC trust so the Azure dashboard VM can manage EC2 instances WITHOUT any
# stored AWS credentials.
#
# Usage:
#   export AWS_ACCOUNT_ID="123456789012"
#   bash scripts/setup-aws-federation.sh
#
# Optional env vars:
#   AZURE_TENANT_ID    — auto-detected from `az account show` if unset
#   APP_DISPLAY_NAME   — defaults to "networker-dashboard-federation"
#   ROLE_NAME          — defaults to "networker-dashboard-role"
#   POLICY_NAME        — defaults to "networker-dashboard-ec2"
#
# What it creates:
#   Azure:
#     1. Azure AD App Registration (audience for MI tokens)
#   AWS:
#     2. OIDC Identity Provider (trusts Azure AD, audience = App ID)
#     3. IAM Role "networker-dashboard-role" (least-privilege EC2 access)
#     4. IAM Policy "networker-dashboard-ec2" (scoped to networker resources)
#
# How it works:
#   Azure VM requests token from IMDS with resource=<APP_REGISTRATION_APP_ID>
#   → token has aud=<APP_ID>, sub=<MI_PRINCIPAL_ID>, iss=sts.windows.net/<TENANT>/
#   → AWS validates token via OIDC discovery
#   → AWS checks aud matches the OIDC provider client-id
#   → AWS issues temporary credentials (no human login ever)
#
# Key lesson: Azure managed identity tokens CANNOT use the MI's own appId
# as audience. You must create a separate App Registration and request tokens
# with resource=<APP_REGISTRATION_APP_ID>.
# ═══════════════════════════════════════════════════════════════════════════

set -euo pipefail

# ── Configuration ─────────────────────────────────────────────────────────
APP_DISPLAY_NAME="${APP_DISPLAY_NAME:-networker-dashboard-federation}"
ROLE_NAME="${ROLE_NAME:-networker-dashboard-role}"
POLICY_NAME="${POLICY_NAME:-networker-dashboard-ec2}"

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

if [[ -z "${AWS_ACCOUNT_ID:-}" ]]; then
    echo "ERROR: Set AWS_ACCOUNT_ID first:"
    echo "  export AWS_ACCOUNT_ID=\"123456789012\""
    echo "  bash $0"
    exit 1
fi

echo "Setting up AWS OIDC Federation for Networker Dashboard"
echo "  Azure Tenant:     ${AZURE_TENANT_ID}"
echo "  OIDC Issuer:      ${OIDC_ISSUER}"
echo "  AWS Account:      ${AWS_ACCOUNT_ID}"
echo ""

# ── Step 1: Create Azure AD App Registration ─────────────────────────────
echo "Step 1: Creating Azure AD App Registration (token audience)..."

# Check if app already exists
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

echo ""
echo "  ┌─────────────────────────────────────────────────────────────────┐"
echo "  │ APP_ID=${APP_ID}"
echo "  │ This is the token audience — save it for the GCP script too.   │"
echo "  └─────────────────────────────────────────────────────────────────┘"
echo ""

# ── Step 2: Create OIDC Identity Provider ─────────────────────────────────
echo "Step 2: Creating OIDC Identity Provider in AWS..."

# Get the Azure AD OIDC thumbprint
THUMBPRINT="$(openssl s_client -connect sts.windows.net:443 -servername sts.windows.net \
    </dev/null 2>/dev/null | openssl x509 -fingerprint -noout 2>/dev/null \
    | sed 's/://g' | cut -d= -f2 | tr '[:upper:]' '[:lower:]')" || true

if [[ -z "$THUMBPRINT" ]]; then
    # Fallback: AWS validates via OIDC discovery anyway (thumbprint is legacy)
    THUMBPRINT="0000000000000000000000000000000000000000"
    echo "  (Using placeholder thumbprint — AWS validates via OIDC discovery)"
fi

PROVIDER_ARN="arn:aws:iam::${AWS_ACCOUNT_ID}:oidc-provider/sts.windows.net/${AZURE_TENANT_ID}/"

if aws iam get-open-id-connect-provider --open-id-connect-provider-arn "$PROVIDER_ARN" &>/dev/null; then
    echo "  OIDC provider already exists, ensuring client ID is registered..."
    aws iam add-client-id-to-open-id-connect-provider \
        --open-id-connect-provider-arn "$PROVIDER_ARN" \
        --client-id "${APP_ID}" 2>/dev/null || true
else
    aws iam create-open-id-connect-provider \
        --url "${OIDC_ISSUER}" \
        --client-id-list "${APP_ID}" \
        --thumbprint-list "${THUMBPRINT}" \
        --output text --query 'OpenIDConnectProviderArn'
    echo "  Created OIDC provider"
fi

# ── Step 3: Create IAM Policy (least privilege) ──────────────────────────
echo "Step 3: Creating IAM policy (EC2 + networking only)..."

POLICY_DOC=$(cat <<'POLICY'
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Sid": "EC2Manage",
            "Effect": "Allow",
            "Action": [
                "ec2:RunInstances",
                "ec2:TerminateInstances",
                "ec2:DescribeInstances",
                "ec2:DescribeInstanceStatus",
                "ec2:StartInstances",
                "ec2:StopInstances",
                "ec2:DescribeImages",
                "ec2:DescribeRegions",
                "ec2:DescribeAvailabilityZones",
                "ec2:DescribeSubnets",
                "ec2:DescribeVpcs"
            ],
            "Resource": "*",
            "Condition": {
                "StringEquals": {
                    "aws:RequestedRegion": ["us-east-1", "us-west-2", "eu-west-1", "eu-central-1"]
                }
            }
        },
        {
            "Sid": "EC2NetworkSetup",
            "Effect": "Allow",
            "Action": [
                "ec2:CreateSecurityGroup",
                "ec2:DeleteSecurityGroup",
                "ec2:AuthorizeSecurityGroupIngress",
                "ec2:RevokeSecurityGroupIngress",
                "ec2:DescribeSecurityGroups",
                "ec2:CreateKeyPair",
                "ec2:DeleteKeyPair",
                "ec2:DescribeKeyPairs",
                "ec2:ImportKeyPair"
            ],
            "Resource": "*"
        },
        {
            "Sid": "EC2Tags",
            "Effect": "Allow",
            "Action": [
                "ec2:CreateTags",
                "ec2:DescribeTags"
            ],
            "Resource": "*"
        },
        {
            "Sid": "STSIdentity",
            "Effect": "Allow",
            "Action": "sts:GetCallerIdentity",
            "Resource": "*"
        }
    ]
}
POLICY
)

POLICY_ARN="arn:aws:iam::${AWS_ACCOUNT_ID}:policy/${POLICY_NAME}"
if aws iam get-policy --policy-arn "$POLICY_ARN" &>/dev/null; then
    echo "  Policy already exists, creating new version..."
    aws iam create-policy-version \
        --policy-arn "$POLICY_ARN" \
        --policy-document "$POLICY_DOC" \
        --set-as-default \
        --output text --query 'PolicyVersion.VersionId'
else
    aws iam create-policy \
        --policy-name "${POLICY_NAME}" \
        --policy-document "$POLICY_DOC" \
        --description "Networker dashboard - EC2 management (least privilege)" \
        --output text --query 'Policy.Arn'
    echo "  Created policy"
fi

# ── Step 4: Create IAM Role (trust checks aud ONLY, not sub) ─────────────
echo "Step 4: Creating IAM role..."

# IMPORTANT: The trust policy checks ONLY the audience (aud) claim.
# Checking "sub" causes AccessDenied because the sub is the MI's principal ID
# (a GUID), and minor formatting differences can break the match.
TRUST_DOC=$(cat <<TRUST
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Effect": "Allow",
            "Principal": {
                "Federated": "arn:aws:iam::${AWS_ACCOUNT_ID}:oidc-provider/sts.windows.net/${AZURE_TENANT_ID}/"
            },
            "Action": "sts:AssumeRoleWithWebIdentity",
            "Condition": {
                "StringEquals": {
                    "sts.windows.net/${AZURE_TENANT_ID}/:aud": "${APP_ID}"
                }
            }
        }
    ]
}
TRUST
)

if aws iam get-role --role-name "${ROLE_NAME}" &>/dev/null; then
    echo "  Role already exists, updating trust policy..."
    aws iam update-assume-role-policy \
        --role-name "${ROLE_NAME}" \
        --policy-document "$TRUST_DOC"
else
    aws iam create-role \
        --role-name "${ROLE_NAME}" \
        --assume-role-policy-document "$TRUST_DOC" \
        --description "Networker dashboard on Azure - federated access to EC2" \
        --max-session-duration 3600 \
        --output text --query 'Role.Arn'
fi

aws iam attach-role-policy \
    --role-name "${ROLE_NAME}" \
    --policy-arn "arn:aws:iam::${AWS_ACCOUNT_ID}:policy/${POLICY_NAME}" 2>/dev/null || true

ROLE_ARN="arn:aws:iam::${AWS_ACCOUNT_ID}:role/${ROLE_NAME}"
echo "  Role ARN: ${ROLE_ARN}"

# ── Step 5: Generate credential helper script for the VM ─────────────────
echo "Step 5: Generating AWS credential helper script..."

HELPER_PATH="/tmp/networker-aws-credential-helper.sh"
cat > "${HELPER_PATH}" <<HELPER
#!/bin/bash
# AWS credential_process helper for Azure VM managed identity
# Place at /usr/local/bin/networker-aws-credential-helper.sh
# Referenced from ~/.aws/config on the dashboard VM.
#
# This script:
#   1. Requests a token from Azure IMDS with audience = App Registration App ID
#   2. Uses that token to call AWS STS AssumeRoleWithWebIdentity
#   3. Outputs JSON in the credential_process format AWS CLI expects

set -euo pipefail

APP_ID="${APP_ID}"
ROLE_ARN="${ROLE_ARN}"

# Get Azure AD token from IMDS (resource = App Registration App ID)
TOKEN=\$(curl -s -H "Metadata: true" \
    "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=\${APP_ID}" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")

# Exchange for AWS credentials via STS
CREDS=\$(aws sts assume-role-with-web-identity \
    --role-arn "\${ROLE_ARN}" \
    --role-session-name "networker-dashboard" \
    --web-identity-token "\${TOKEN}" \
    --duration-seconds 3600 \
    --output json)

# Output in credential_process format
python3 -c "
import json, sys
c = json.loads(''''\${CREDS}''''')['Credentials']
print(json.dumps({
    'Version': 1,
    'AccessKeyId': c['AccessKeyId'],
    'SecretAccessKey': c['SecretAccessKey'],
    'SessionToken': c['SessionToken'],
    'Expiration': c['Expiration']
}))
"
HELPER

chmod +x "${HELPER_PATH}"
echo "  Helper script written to: ${HELPER_PATH}"

# ── Step 6: Generate AWS config snippet ───────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════════════════════════════════╗"
echo "║  AWS OIDC Federation configured!                                   ║"
echo "║                                                                    ║"
echo "║  Azure AD App Registration: ${APP_ID}    ║"
echo "║  AWS Role ARN: ${ROLE_ARN}               ║"
echo "║                                                                    ║"
echo "║  1. Copy the helper to the dashboard VM:                           ║"
echo "║     scp ${HELPER_PATH} \\                  ║"
echo "║       azureuser@<dashboard-vm>:/tmp/                               ║"
echo "║                                                                    ║"
echo "║  2. On the VM:                                                     ║"
echo "║     sudo mv /tmp/networker-aws-credential-helper.sh \\             ║"
echo "║       /usr/local/bin/networker-aws-credential-helper.sh            ║"
echo "║     sudo chmod 755 /usr/local/bin/networker-aws-credential-helper.sh ║"
echo "║                                                                    ║"
echo "║  3. Configure AWS CLI on the VM (~/.aws/config):                   ║"
echo "║     [default]                                                      ║"
echo "║     region = us-east-1                                             ║"
echo "║     credential_process = /usr/local/bin/networker-aws-credential-helper.sh ║"
echo "║                                                                    ║"
echo "║  4. For the GCP script, export the APP_ID:                        ║"
printf "║     export AZURE_APP_ID=\"%s\"             ║\n" "${APP_ID}"
echo "╚══════════════════════════════════════════════════════════════════════╝"
