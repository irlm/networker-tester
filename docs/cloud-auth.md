# Zero-Credential Multi-Cloud Authentication

The networker dashboard runs on an Azure VM with a system-assigned managed
identity. It manages cloud resources (EC2 instances on AWS, GCE instances on
GCP) without storing any long-lived credentials. Instead, it uses
**federated identity** — exchanging an Azure AD token for temporary
credentials on each target cloud.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                      Azure Dashboard VM                                │
│                                                                        │
│  1. Request token from IMDS                                            │
│     GET http://169.254.169.254/metadata/identity/oauth2/token          │
│         ?resource=<APP_REGISTRATION_APP_ID>                            │
│                                                                        │
│  2. Azure AD returns JWT:                                              │
│     iss: https://sts.windows.net/<TENANT_ID>/                          │
│     aud: <APP_REGISTRATION_APP_ID>                                     │
│     sub: <MI_PRINCIPAL_ID>                                             │
│                                                                        │
│                    ┌───────────────┬───────────────┐                   │
│                    │     AWS       │     GCP       │                   │
│                    │               │               │                   │
│  3a. STS           │ AssumeRole    │ STS exchange  │                   │
│      exchange      │ WithWeb       │ + access      │                   │
│                    │ Identity      │ token         │                   │
│                    │               │               │                   │
│  4. Temp creds     │ AccessKey +   │ OAuth2        │                   │
│     returned       │ SecretKey +   │ access token  │                   │
│                    │ SessionToken  │               │                   │
│                    └───────────────┴───────────────┘                   │
└─────────────────────────────────────────────────────────────────────────┘
```

## Critical Design Decision: App Registration as Audience

Azure managed identity tokens **cannot** use the MI's own `appId` (client
ID) as the token audience. Requesting a token with
`resource=<MI_CLIENT_ID>` fails with:

```
AADSTS500011: The resource principal named <MI_CLIENT_ID> was not found
```

or:

```
AADSTS100040: The request body must contain the following parameter: 'resource' or 'scope'
```

The solution is to create an **Azure AD App Registration** (a separate
resource from the managed identity) and use its `appId` as the token
audience:

1. Create App Registration: `az ad app create --display-name networker-dashboard-federation`
2. Request IMDS token with `resource=<APP_REGISTRATION_APP_ID>`
3. Configure AWS OIDC client-id and GCP allowed-audiences to match the App ID

Both the AWS and GCP setup scripts handle this automatically.

## Prerequisites

| Requirement | Purpose |
|---|---|
| Azure VM with system-assigned managed identity | Source of identity tokens |
| Azure AD App Registration | Token audience (created by setup scripts) |
| Azure CLI (`az`) authenticated | Creating the App Registration |
| AWS CLI authenticated (for AWS setup) | Creating OIDC provider + IAM role |
| gcloud CLI authenticated (for GCP setup) | Creating WIF pool + service account |
| `AWS_ACCOUNT_ID` env var | Required by AWS setup script |
| `GCP_PROJECT_ID` env var | Required by GCP setup script |

## Setup Steps

### 1. Azure: Managed Identity (already done if VM exists)

The dashboard VM needs a system-assigned managed identity. This is
typically enabled at VM creation time or via:

```bash
az vm identity assign --resource-group <RG> --name <VM_NAME>
```

No additional Azure-side configuration is needed — the setup scripts
create the App Registration automatically.

### 2. AWS: OIDC Federation

```bash
export AWS_ACCOUNT_ID="123456789012"
bash scripts/setup-aws-federation.sh
```

This script:
1. Creates (or reuses) the Azure AD App Registration
   `networker-dashboard-federation`
2. Creates an AWS OIDC Identity Provider trusting
   `https://sts.windows.net/<TENANT_ID>/`
3. Creates an IAM policy with least-privilege EC2 permissions
4. Creates an IAM role with a trust policy checking `aud == <APP_ID>`
5. Generates a credential helper script for the VM

Then on the dashboard VM:
1. Install the credential helper at `/usr/local/bin/networker-aws-credential-helper.sh`
2. Configure `~/.aws/config` with `credential_process` pointing to the helper

### 3. GCP: Workload Identity Federation

```bash
export GCP_PROJECT_ID="my-project-123"
bash scripts/setup-gcp-federation.sh
```

This script:
1. Creates (or reuses) the same Azure AD App Registration
2. Enables required GCP APIs (IAM, STS, Compute, Cloud Resource Manager)
3. Creates a Workload Identity Pool and OIDC provider
4. Creates a service account with Compute Instance Admin role
5. Binds the Azure MI principal to the service account
6. Generates a credential config JSON file

Then on the dashboard VM:
1. Copy the credential config to `/etc/networker-gcp-credentials.json`
2. Set `GOOGLE_APPLICATION_CREDENTIALS=/etc/networker-gcp-credentials.json`

## How the AWS Credential Helper Works

AWS does not natively support Azure AD tokens. The credential helper
bridges the gap using the `credential_process` mechanism in `~/.aws/config`.

**Flow:**

```
AWS CLI/SDK needs credentials
  → runs credential_process
    → helper requests token from Azure IMDS (resource=APP_ID)
    → helper calls aws sts assume-role-with-web-identity
    → returns AccessKeyId + SecretAccessKey + SessionToken as JSON
  → AWS CLI uses temporary credentials
  → credentials expire after 1 hour → process repeats automatically
```

**`~/.aws/config` on the VM:**

```ini
[default]
region = us-east-1
credential_process = /usr/local/bin/networker-aws-credential-helper.sh
```

The helper script is generated by `setup-aws-federation.sh` with the
correct `APP_ID` and `ROLE_ARN` baked in.

## How the GCP Credential Config Works

GCP natively supports external identity federation via a JSON credential
config file. The Google Cloud client libraries read this file when
`GOOGLE_APPLICATION_CREDENTIALS` points to it.

**Flow:**

```
Google Cloud SDK/library needs access token
  → reads credential config JSON
  → fetches Azure AD token from IMDS URL in credential_source
    (URL includes resource=APP_ID as query parameter)
  → sends token to GCP STS (securitytoken.googleapis.com)
  → GCP validates: issuer matches, audience matches allowed-audiences
  → GCP STS returns a federated access token
  → library uses access token for Compute Engine API calls
  → token expires → process repeats automatically
```

The credential config JSON looks like:

```json
{
  "type": "external_account",
  "audience": "//iam.googleapis.com/projects/<NUMBER>/locations/global/workloadIdentityPools/<POOL>/providers/<PROVIDER>",
  "subject_token_type": "urn:ietf:params:oauth:token-type:jwt",
  "token_url": "https://sts.googleapis.com/v1/token",
  "credential_source": {
    "url": "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=<APP_ID>",
    "headers": { "Metadata": "true" },
    "format": { "type": "json", "subject_token_field_name": "access_token" }
  },
  "service_account_impersonation_url": "https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/<SA>:generateAccessToken"
}
```

The critical field is `credential_source.url` — its `resource=` parameter
**must** be the App Registration's App ID, not the MI client ID.

## Troubleshooting

### AADSTS100040: Missing resource or scope

**Error:** `The request body must contain the following parameter: 'resource' or 'scope'`

**Cause:** The IMDS token request is missing the `resource` query parameter,
or the resource value is empty.

**Fix:** Ensure the credential helper or config uses
`resource=<APP_REGISTRATION_APP_ID>` in the IMDS URL.

### AADSTS500011: Resource principal not found

**Error:** `The resource principal named <ID> was not found in the tenant`

**Cause:** You are using the MI's own client ID as the `resource` parameter.
Managed identity client IDs are not valid token audiences.

**Fix:** Create an App Registration (`az ad app create`) and use its `appId`
as the resource. The setup scripts do this automatically.

### AWS AccessDenied on AssumeRoleWithWebIdentity

**Error:** `An error occurred (AccessDenied) when calling the AssumeRoleWithWebIdentity operation`

**Common causes:**

1. **Trust policy checks `sub` claim.** The `sub` in an Azure MI token is
   the MI's principal ID (object ID). If the trust policy has a
   `StringEquals` condition on `sub`, even minor formatting differences
   (uppercase vs lowercase GUID) cause rejection. **Fix:** Remove the `sub`
   condition — check only `aud`.

2. **Wrong `aud` value.** The trust policy expects the OIDC client-id
   (audience) to be the App Registration's App ID, but the OIDC provider
   was created with the MI client ID. **Fix:** Update the OIDC provider's
   client-id list: `aws iam add-client-id-to-open-id-connect-provider`.

3. **Wrong issuer URL.** Azure AD v1 tokens use
   `https://sts.windows.net/<TENANT_ID>/` (with trailing slash). The v2
   endpoint (`login.microsoftonline.com/<TENANT_ID>/v2.0`) issues tokens
   with a different `iss` claim. IMDS uses the v1 endpoint, so the OIDC
   provider must use `sts.windows.net`. **Fix:** Recreate the OIDC provider
   with the correct issuer URL.

### GCP: INVALID_GRANT or token exchange failure

**Error:** Token exchange returns 400 or the credential config does not work.

**Common causes:**

1. **Issuer mismatch.** The WIF provider's `issuer-uri` must be
   `https://sts.windows.net/<TENANT_ID>/` — same issuer that appears in the
   token's `iss` claim.

2. **Audience mismatch.** The provider's `allowed-audiences` must match the
   `aud` in the token, which is the App Registration's App ID.

3. **Wrong resource in credential_source URL.** If the `resource=` in the
   IMDS URL is wrong (e.g., uses MI client ID), the token's `aud` will not
   match `allowed-audiences`. Regenerate the credential config with
   `--app-id-uri=<APP_ID>`.

4. **Subject binding mismatch.** The service account binding uses
   `subject/<MI_PRINCIPAL_ID>`. If the MI principal ID is wrong, GCP cannot
   map the federated identity to the service account. Verify with:
   `az vm show --name <VM> --query identity.principalId -o tsv`

### Verifying the token contents

To inspect what Azure IMDS returns (run on the dashboard VM):

```bash
# Request token with the App Registration as audience
TOKEN=$(curl -s -H "Metadata: true" \
    "http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=<APP_ID>" \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")

# Decode the JWT (without validating signature)
echo "$TOKEN" | cut -d. -f2 | base64 -d 2>/dev/null | python3 -m json.tool
```

Check that:
- `iss` is `https://sts.windows.net/<TENANT_ID>/` (with trailing slash)
- `aud` is the App Registration's App ID
- `sub` is the MI's principal ID (object ID)

## Security Model

### What is trusted

| Cloud | Trust anchor | What is checked |
|---|---|---|
| AWS | OIDC provider → Azure AD | `iss` (issuer URL) + `aud` (App ID) |
| GCP | WIF pool → Azure AD | `iss` (issuer URL) + `aud` (App ID) + `sub` (MI principal → SA binding) |

### Least privilege

- **AWS:** The IAM policy restricts to EC2 actions in specific regions,
  with tagging for resource tracking. No S3, Lambda, RDS, etc.
- **GCP:** The service account has only `roles/compute.instanceAdmin.v1`.
  No storage, BigQuery, etc.

### No stored secrets

- No AWS access keys on disk
- No GCP service account JSON keys
- Credentials are ephemeral (1-hour lifetime) and auto-refresh
- Revoking access: delete the App Registration, or remove the OIDC
  provider (AWS) / WIF pool (GCP)

### Attack surface

If the Azure VM is compromised, the attacker can manage EC2/GCE instances
within the scoped permissions. Mitigations:
- Restrict EC2 regions in the IAM policy
- Use GCP organization policies to limit instance creation
- Monitor CloudTrail (AWS) and Cloud Audit Logs (GCP) for anomalous activity
- The MI token is only obtainable from the specific VM (IMDS is
  link-local, not routable)
