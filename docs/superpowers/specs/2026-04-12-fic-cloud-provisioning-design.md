# FIC-Compliant Cloud Provisioning Design

> **Source requirements:** User-provided "Cloud Provisioning and Identity Security Requirements" document (2026-04-12).
> **Version scope:** v1.0 = Azure only. AWS + GCP prepared via abstraction layer but not implemented.
> **Relationship to persistent testers:** This spec replaces the Azure-hardcoded VM lifecycle in `services/azure_vm.rs` with a provider-agnostic, secretless architecture.

---

## 1. Core principle: zero stored secrets

The system authenticates to cloud providers exclusively via **identity-based, short-lived tokens** obtained at runtime from native cloud identity services. No long-lived credentials (passwords, client secrets, API keys, service account JSON, certificates) are stored in the database, configuration files, environment variables, logs, or source code.

**What this means for the existing codebase:**

| Existing artifact | Disposition |
|---|---|
| `cloud_account` table (stores AES-256-GCM encrypted credentials) | **Retained for backward compatibility** with non-tester features that still use stored creds. New tester/endpoint provisioning flows MUST NOT use it. |
| `cloud_connection` table (stores only non-sensitive config) | **Primary model** for tester provisioning. Stores subscription_id, resource_group, tenant_id — no secrets. |
| `crypto.rs` (AES-256-GCM encrypt/decrypt) | Retained; not used by tester provisioning flows. |
| `services/azure_vm.rs` (shells out to `az` CLI) | **Replaced** by `services/cloud_provider.rs` abstraction. The `az` CLI authenticates via managed identity — no credentials needed. |
| `DASHBOARD_AZURE_RG` env var (hardcoded resource group) | **Removed**; resource group comes from the `cloud_connection.config` JSON. |

---

## 2. Authentication model per provider

### Azure (v1.0 — fully implemented)

**Preferred:** System-assigned or user-assigned **Managed Identity** on the dashboard VM. The `az` CLI automatically uses the VM's managed identity when no explicit login is configured.

**Fallback:** **Federated Identity Credentials (FIC / Workload Identity Federation)** for non-Azure-hosted dashboards. An Entra ID App Registration is configured with a FIC trust policy; the dashboard exchanges a platform token for an Azure AD token without any client secret.

**What the dashboard stores (non-sensitive):**
```json
{
  "tenant_id": "1ecbc8ed-...",
  "subscription_id": "xxxxxxxx-...",
  "resource_group": "ALETHEDASH-RG",
  "identity_type": "managed_identity",
  "identity_ref": null
}
```
Or for FIC:
```json
{
  "tenant_id": "1ecbc8ed-...",
  "subscription_id": "xxxxxxxx-...",
  "resource_group": "ALETHEDASH-RG",
  "identity_type": "fic",
  "identity_ref": "app-registration-client-id"
}
```

**RBAC requirement:** The managed identity (or FIC app) must have **Virtual Machine Contributor** scoped to the target resource group. Nothing more.

### AWS (future — architecture prepared, not implemented)

**Model:** IAM Role + AWS STS `AssumeRoleWithWebIdentity`. The dashboard exchanges its Azure AD token for temporary AWS credentials (AccessKey + SecretKey + SessionToken, 1-hour lifetime).

**What the dashboard would store:**
```json
{
  "role_arn": "arn:aws:iam::123456789012:role/networker-dashboard",
  "region_default": "us-east-1",
  "external_id": "optional-external-id"
}
```

No AWS access keys stored.

### GCP (future — architecture prepared, not implemented)

**Model:** Workload Identity Federation (WIF) pool + Service Account Impersonation. The dashboard exchanges its Azure AD token for a GCP federated access token, then impersonates a GCP service account scoped to the target project.

**What the dashboard would store:**
```json
{
  "project_id": "my-gcp-project-123",
  "wif_pool_provider": "projects/123/locations/global/workloadIdentityPools/azure-pool/providers/azure-provider",
  "service_account_email": "networker-dashboard@my-project.iam.gserviceaccount.com",
  "region_default": "us-central1"
}
```

No service account key files stored.

---

## 3. Data model changes

### Link `project_tester` to `cloud_connection`

Add to V029 migration:
```sql
ALTER TABLE project_tester
  ADD COLUMN IF NOT EXISTS cloud_connection_id UUID
    REFERENCES cloud_connection(connection_id) ON DELETE RESTRICT;
```

`ON DELETE RESTRICT` prevents deleting a cloud_connection that still has testers. The user must delete or reassign testers first.

### Extend `cloud_connection.config` schema validation

Currently `cloud_connection.config` is arbitrary JSONB. Add provider-specific validation in the API layer:

**Azure config (required fields):**
- `tenant_id` (string, UUID format)
- `subscription_id` (string, UUID format)
- `resource_group` (string, 1-90 chars, alphanumeric + hyphens)
- `identity_type` (enum: `"managed_identity"` | `"fic"`)
- `identity_ref` (string, required if identity_type = "fic"; nullable for managed_identity)

**AWS config (future, validated but not acted on):**
- `role_arn` (string, ARN format)
- `region_default` (string)
- `external_id` (string, optional)

**GCP config (future, validated but not acted on):**
- `project_id` (string)
- `wif_pool_provider` (string)
- `service_account_email` (string, email format)
- `region_default` (string)

### `cloud_connection` scoped to project

The existing schema has `cloud_connection.project_id` (added in a later migration). Tester creation validates that the selected `cloud_connection_id` belongs to the same project as the tester. Cross-project access is structurally impossible.

---

## 4. Provider abstraction layer

### Trait definition

```rust
// crates/networker-dashboard/src/services/cloud_provider.rs

pub struct VmConfig {
    pub name: String,
    pub region: String,
    pub vm_size: String,
    pub ssh_user: String,
    pub image: String,           // e.g. "Canonical:ubuntu-24_04-lts:server:latest"
    pub tags: HashMap<String, String>,
}

pub struct VmInfo {
    pub resource_id: String,     // provider-specific resource ID
    pub public_ip: Option<String>,
    pub vm_name: String,
    pub power_state: String,     // normalized: "running" | "stopped" | "starting" | "stopping" | "deallocated"
}

pub enum CloudProvider {
    Azure(AzureProvider),
    // Aws(AwsProvider),   // future
    // Gcp(GcpProvider),   // future
}

impl CloudProvider {
    pub fn from_connection(conn: &CloudConnectionRow) -> anyhow::Result<Self> {
        match conn.provider.as_str() {
            "azure" => Ok(CloudProvider::Azure(AzureProvider::new(&conn.config)?)),
            "aws" => anyhow::bail!("AWS provider not yet implemented"),
            "gcp" => anyhow::bail!("GCP provider not yet implemented"),
            other => anyhow::bail!("Unknown provider: {other}"),
        }
    }

    pub async fn create_vm(&self, config: &VmConfig) -> anyhow::Result<VmInfo>;
    pub async fn start_vm(&self, resource_id: &str) -> anyhow::Result<()>;
    pub async fn stop_vm(&self, resource_id: &str) -> anyhow::Result<()>;
    pub async fn delete_vm(&self, resource_id: &str) -> anyhow::Result<()>;
    pub async fn get_vm_state(&self, resource_id: &str) -> anyhow::Result<String>;
    pub async fn tag_vm(&self, resource_id: &str, tags: &HashMap<String, String>) -> anyhow::Result<()>;
}
```

### Azure implementation (v1.0)

```rust
pub struct AzureProvider {
    subscription_id: String,
    resource_group: String,
    identity_type: String,  // "managed_identity" | "fic"
}

impl AzureProvider {
    pub fn new(config: &serde_json::Value) -> anyhow::Result<Self> { ... }

    // All methods shell out to `az` CLI with --subscription and --resource-group.
    // The CLI authenticates via the VM's managed identity automatically.
    // Example:
    //   az vm create --subscription <sub> --resource-group <rg> --name <name> ...
    //   az vm start --subscription <sub> --resource-group <rg> --name <name>
    //   az vm deallocate --subscription <sub> --resource-group <rg> --name <name>
    //   az vm delete --subscription <sub> --resource-group <rg> --name <name> --yes
    //   az vm get-instance-view --subscription <sub> --resource-group <rg> --name <name>
}
```

**Critical:** every `az` command includes `--subscription` and `--resource-group` from the cloud_connection config. No hardcoded defaults. No ambient subscription leakage.

### Region → timezone mapping

`services/azure_regions.rs` already handles Azure. Extend with a dispatch:
```rust
pub fn region_timezone(provider: &str, region: &str) -> chrono_tz::Tz {
    match provider {
        "azure" => azure_region_timezone(region),
        "aws" => aws_region_timezone(region),   // future
        "gcp" => gcp_region_timezone(region),   // future
        _ => chrono_tz::UTC,
    }
}
```

AWS and GCP region maps added as stubs returning UTC until those providers are implemented.

---

## 5. Tester creation flow (updated)

1. User selects a `cloud_connection` from their project's connections list (filtered by provider).
2. `POST /api/projects/{pid}/testers` body includes `cloud_connection_id`.
3. API validates: connection belongs to project, connection status is `active`, provider is supported.
4. API constructs `CloudProvider::from_connection(conn)` and calls `provider.create_vm(config)`.
5. Background task monitors provisioning, calls `tester_install::install_tester` on success.
6. `project_tester` row stores `cloud_connection_id` for all future lifecycle operations.

All subsequent operations (start, stop, deallocate, delete, probe) load the cloud_connection from the tester row, construct the provider, and execute.

---

## 6. Installer changes (install.sh)

### Azure setup flow (non-interactive collection)

The installer already validates Azure credentials. Update to collect:

```bash
# Collect non-sensitive Azure config
echo "Enter your Azure Tenant ID:"
read AZURE_TENANT_ID
echo "Enter your Azure Subscription ID:"
read AZURE_SUBSCRIPTION_ID
echo "Enter the Resource Group for tester VMs:"
read AZURE_RESOURCE_GROUP

# Validate via managed identity (no secrets needed)
az account show --subscription "$AZURE_SUBSCRIPTION_ID" --query '{name:name,state:state}' -o json < /dev/null
```

The installer does NOT collect or store any passwords, client secrets, or API keys.

### Future AWS/GCP flows

Stub placeholders that print "AWS/GCP support coming soon" and exit cleanly.

---

## 7. What gets deleted / replaced

| File | Action |
|---|---|
| `services/azure_vm.rs` | **Replace** with `services/cloud_provider.rs` (trait + AzureProvider) |
| `DASHBOARD_AZURE_RG` env var usage | **Remove** — resource group comes from cloud_connection config |
| `api/testers.rs` hardcoded Azure region list | **Replace** — query `cloud_connection.config.resource_group` regions dynamically, or use provider-specific region lists |
| `tester_scheduler.rs` `vm_deallocate` call | Update to use `CloudProvider::stop_vm()` |
| `tester_recovery.rs` `probe_azure_state` call | Update to use `CloudProvider::get_vm_state()` |
| orchestrator `ensure_running_via_azure` | Update to use cloud_connection from tester row |

---

## 8. Security audit checklist (acceptance criteria)

- [ ] No long-lived credentials stored in DB, config, logs, source, or env vars for tester provisioning
- [ ] `az` CLI authenticates via managed identity (verify: `az account show` succeeds without `az login`)
- [ ] All `az` commands include explicit `--subscription` and `--resource-group` (no ambient defaults)
- [ ] `cloud_connection.config` contains only non-sensitive fields (tenant_id, subscription_id, resource_group)
- [ ] `project_tester.cloud_connection_id` enforces FK to project-scoped connection
- [ ] Cross-project cloud_connection access returns 404
- [ ] Azure Activity Log captures all VM operations performed by the managed identity
- [ ] Managed identity has only Virtual Machine Contributor on the target resource group (not broader)
- [ ] Provider abstraction compiles with AWS/GCP stubs (no runtime failures if not called)
- [ ] Installer collects zero secrets

---

## 9. Migration plan (V029)

```sql
-- V029: Link testers to cloud_connections; remove hardcoded Azure dependency.

-- Add cloud_connection_id to project_tester
ALTER TABLE project_tester
  ADD COLUMN IF NOT EXISTS cloud_connection_id UUID
    REFERENCES cloud_connection(connection_id) ON DELETE RESTRICT;

-- Index for join performance
CREATE INDEX IF NOT EXISTS idx_project_tester_cloud_conn
  ON project_tester(cloud_connection_id)
  WHERE cloud_connection_id IS NOT NULL;

-- Ensure cloud_connection has project_id if not already present
-- (V010 added it; this is a safety net)
ALTER TABLE cloud_connection
  ADD COLUMN IF NOT EXISTS project_id CHAR(14)
    REFERENCES project(project_id) ON DELETE CASCADE;
```

Existing testers (created during v0.25.x with hardcoded Azure) will have `cloud_connection_id = NULL`. They continue to work via the legacy `DASHBOARD_AZURE_RG` fallback until a cloud_connection is assigned. New testers require a cloud_connection.
