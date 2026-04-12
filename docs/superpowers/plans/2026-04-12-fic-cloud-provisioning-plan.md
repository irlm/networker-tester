# FIC-Compliant Cloud Provisioning Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Replace Azure-hardcoded VM lifecycle with a secretless, provider-agnostic abstraction. Azure v1.0 via managed identity. AWS + GCP stubs prepared.

**Source spec:** `docs/superpowers/specs/2026-04-12-fic-cloud-provisioning-design.md`

---

## Build Order

8 tasks in 3 phases. Each phase produces compilable, testable code.

1. **Phase A — Abstraction layer** (Tasks 1–3): Provider trait, AzureProvider, region dispatch.
2. **Phase B — Integration** (Tasks 4–6): V029 migration, update tester CRUD + API, update background services.
3. **Phase C — Installer + validation** (Tasks 7–8): install.sh Azure identity config, security audit tests.

---

## Phase A — Abstraction layer

### Task 1: CloudProvider trait + AzureProvider

**Goal:** Create `crates/networker-dashboard/src/services/cloud_provider.rs` with the provider abstraction.

**Files:**
- Create `crates/networker-dashboard/src/services/cloud_provider.rs`
- Modify `crates/networker-dashboard/src/services/mod.rs`

The module defines:
- `pub struct VmConfig { name, region, vm_size, ssh_user, image, tags }`
- `pub struct VmInfo { resource_id, public_ip, vm_name, power_state }`
- `pub enum CloudProvider { Azure(AzureProvider) }` with methods: `create_vm`, `start_vm`, `stop_vm`, `delete_vm`, `get_vm_state`, `tag_vm`
- `impl CloudProvider { pub fn from_connection(conn_provider: &str, conn_config: &serde_json::Value) -> Result<Self> }`
- `pub struct AzureProvider { subscription_id, resource_group, identity_type }`
- AzureProvider implements all 6 methods by shelling out to `az` CLI with explicit `--subscription` and `--resource-group` flags. Every command includes these two flags — no ambient defaults.

Port the logic from `services/azure_vm.rs` into `AzureProvider`, replacing the `DASHBOARD_AZURE_RG` env var with `self.resource_group` from the config.

Unit tests:
- `azure_provider_from_valid_config` — parses config JSON correctly
- `azure_provider_rejects_missing_subscription` — errors on incomplete config
- `from_connection_rejects_unknown_provider` — returns error for "aws"/"gcp" (stubs)

- [ ] Step 1: Write the module
- [ ] Step 2: Wire into services/mod.rs
- [ ] Step 3: `cargo build -p networker-dashboard` clean
- [ ] Step 4: `cargo test -p networker-dashboard --lib cloud_provider` — tests pass
- [ ] Step 5: Commit: `feat(dashboard): CloudProvider trait + AzureProvider (secretless)`

---

### Task 2: Delete azure_vm.rs, update callers to use CloudProvider

**Goal:** Remove the old hardcoded module and update all call sites.

**Files:**
- Delete `crates/networker-dashboard/src/services/azure_vm.rs`
- Modify `crates/networker-dashboard/src/services/mod.rs` (remove `pub mod azure_vm;`)
- Modify `crates/networker-dashboard/src/api/testers.rs` (update create/start/stop/upgrade/delete handlers)
- Modify `crates/networker-dashboard/src/services/tester_scheduler.rs` (update vm_deallocate call)
- Modify `crates/networker-dashboard/src/services/tester_recovery.rs` (update probe_azure_state call)

For now, callers that don't yet have a `cloud_connection_id` on the tester row will construct a **legacy fallback** AzureProvider from env vars:

```rust
fn legacy_azure_provider() -> anyhow::Result<CloudProvider> {
    let sub = std::env::var("AZURE_SUBSCRIPTION_ID")
        .or_else(|_| std::env::var("DASHBOARD_AZURE_SUBSCRIPTION"))
        .unwrap_or_default();
    let rg = std::env::var("DASHBOARD_AZURE_RG")
        .unwrap_or_else(|_| "networker-testers".to_string());
    let config = serde_json::json!({
        "tenant_id": "",
        "subscription_id": sub,
        "resource_group": rg,
        "identity_type": "managed_identity"
    });
    CloudProvider::from_connection("azure", &config)
}
```

This keeps existing testers (created during v0.25.x with no cloud_connection_id) working until Task 4 adds the migration and Task 5 updates the API to require cloud_connection_id.

- [ ] Step 1: Delete azure_vm.rs
- [ ] Step 2: Update all callers to use CloudProvider (with legacy fallback for now)
- [ ] Step 3: `cargo build -p networker-dashboard` clean
- [ ] Step 4: `cargo clippy -p networker-dashboard --all-targets -- -D warnings` clean
- [ ] Step 5: Commit: `refactor(dashboard): replace azure_vm.rs with CloudProvider abstraction`

---

### Task 3: Multi-provider region → timezone dispatch

**Goal:** Make region_timezone and next_shutdown_at provider-aware.

**Files:**
- Modify `crates/networker-dashboard/src/services/azure_regions.rs` — rename to `cloud_regions.rs` or keep name and add dispatch.

Add:
```rust
pub fn region_timezone(provider: &str, region: &str) -> chrono_tz::Tz {
    match provider {
        "azure" => azure_region_timezone(region),
        "aws" => aws_region_timezone(region),
        "gcp" => gcp_region_timezone(region),
        _ => chrono_tz::UTC,
    }
}

fn aws_region_timezone(region: &str) -> chrono_tz::Tz {
    match region {
        "us-east-1" | "us-east-2" => chrono_tz::US::Eastern,
        "us-west-1" | "us-west-2" => chrono_tz::US::Pacific,
        "eu-west-1" => chrono_tz::Europe::Dublin,
        "eu-west-2" => chrono_tz::Europe::London,
        "eu-central-1" => chrono_tz::Europe::Berlin,
        "ap-northeast-1" => chrono_tz::Asia::Tokyo,
        "ap-southeast-1" => chrono_tz::Asia::Singapore,
        "ap-southeast-2" => chrono_tz::Australia::Sydney,
        "sa-east-1" => chrono_tz::America::Sao_Paulo,
        _ => chrono_tz::UTC,
    }
}

fn gcp_region_timezone(region: &str) -> chrono_tz::Tz {
    match region {
        "us-central1" | "us-east1" | "us-east4" => chrono_tz::US::Eastern,
        "us-west1" | "us-west2" | "us-west4" => chrono_tz::US::Pacific,
        "europe-west1" | "europe-west4" => chrono_tz::Europe::Amsterdam,
        "europe-west2" => chrono_tz::Europe::London,
        "europe-west3" => chrono_tz::Europe::Berlin,
        "asia-east1" | "asia-east2" => chrono_tz::Asia::Taipei,
        "asia-northeast1" => chrono_tz::Asia::Tokyo,
        "asia-southeast1" => chrono_tz::Asia::Singapore,
        "australia-southeast1" => chrono_tz::Australia::Sydney,
        _ => chrono_tz::UTC,
    }
}
```

Update `next_shutdown_at` to take `provider` parameter.

Update callers in `tester_scheduler.rs` and `api/testers.rs` to pass the tester's `cloud` field as the provider.

Tests: `aws_known_regions_resolve`, `gcp_known_regions_resolve`.

- [ ] Step 1: Add AWS + GCP region maps
- [ ] Step 2: Update region_timezone to dispatch by provider
- [ ] Step 3: Update callers
- [ ] Step 4: Tests pass
- [ ] Step 5: Commit: `feat(dashboard): multi-provider region → timezone dispatch`

---

## Phase B — Integration

### Task 4: V029 migration — link testers to cloud_connections

**Goal:** Add `cloud_connection_id` column to `project_tester`.

**Files:** Modify `crates/networker-dashboard/src/db/migrations.rs`

```sql
-- V029: Link testers to cloud_connections for secretless provisioning.
ALTER TABLE project_tester
  ADD COLUMN IF NOT EXISTS cloud_connection_id UUID
    REFERENCES cloud_connection(connection_id) ON DELETE RESTRICT;

CREATE INDEX IF NOT EXISTS idx_project_tester_cloud_conn
  ON project_tester(cloud_connection_id)
  WHERE cloud_connection_id IS NOT NULL;
```

Add the V029 runner block mirroring existing pattern.

Existing testers with `cloud_connection_id = NULL` continue to work via legacy fallback.

- [ ] Step 1: Add V029 constant + runner
- [ ] Step 2: `cargo build -p networker-dashboard` clean
- [ ] Step 3: Commit: `feat(dashboard): V029 link project_tester to cloud_connection`

---

### Task 5: Update tester CRUD + API to use cloud_connection

**Goal:** Tester creation requires `cloud_connection_id`. API validates connection belongs to same project and provider is supported.

**Files:**
- Modify `crates/networker-dashboard/src/db/project_testers.rs` — add `cloud_connection_id` to `ProjectTesterRow`, `CreateTesterInput`, `insert()`
- Modify `crates/networker-dashboard/src/api/testers.rs` — create handler validates cloud_connection, constructs CloudProvider from it

Create handler flow:
1. `POST /api/projects/{pid}/testers` body now includes `cloud_connection_id: UUID`
2. Validate: `SELECT * FROM cloud_connection WHERE connection_id = $1 AND project_id = $2` — 404 if not found, 409 if status != 'active'
3. Validate: `CloudProvider::from_connection(conn.provider, &conn.config)?` — 400 if provider unsupported
4. Insert tester with `cloud_connection_id`
5. Background task constructs `CloudProvider` from the connection and calls `provider.create_vm(...)`

All other lifecycle handlers (start/stop/delete/probe/upgrade) load the cloud_connection via the tester's `cloud_connection_id` and construct the provider. If `cloud_connection_id IS NULL` (legacy tester), fall back to `legacy_azure_provider()`.

Update the `GET /testers/regions` endpoint to return regions based on the project's cloud connections (not a hardcoded Azure list).

- [ ] Step 1: Update ProjectTesterRow + CreateTesterInput
- [ ] Step 2: Update create handler with cloud_connection validation
- [ ] Step 3: Update lifecycle handlers to load provider from connection
- [ ] Step 4: Update regions endpoint
- [ ] Step 5: `cargo build && cargo test` clean
- [ ] Step 6: Commit: `feat(dashboard): tester API requires cloud_connection (secretless provisioning)`

---

### Task 6: Update background services + orchestrator

**Goal:** Background services and orchestrator use CloudProvider instead of direct `az` calls.

**Files:**
- Modify `crates/networker-dashboard/src/services/tester_scheduler.rs` — load cloud_connection for each tester, construct provider, call `provider.stop_vm()`
- Modify `crates/networker-dashboard/src/services/tester_recovery.rs` — use `provider.get_vm_state()` for probe
- Modify `benchmarks/orchestrator/src/executor.rs` — update `ensure_running_via_azure` to use provider (or add a cloud_connection lookup to the orchestrator's tester_state)

The orchestrator is the hardest part — it's a separate crate. Two approaches:
- **Option A:** Duplicate the CloudProvider trait in the orchestrator (same pattern as tester_state duplication). Simpler.
- **Option B:** Extract CloudProvider into `networker-common` (shared crate). Cleaner but larger refactor.

**Choose Option A** for this PR. A future PR can consolidate.

For the scheduler/recovery: the tester row already has `cloud` (provider name) and now `cloud_connection_id`. Load the connection, construct the provider, execute.

- [ ] Step 1: Update tester_scheduler to use CloudProvider
- [ ] Step 2: Update tester_recovery to use CloudProvider
- [ ] Step 3: Update orchestrator with duplicated CloudProvider
- [ ] Step 4: `cargo build --workspace && cargo build -p alethabench-orchestrator` clean
- [ ] Step 5: Commit: `feat: background services + orchestrator use CloudProvider`

---

## Phase C — Installer + validation

### Task 7: install.sh Azure identity configuration

**Goal:** Update install.sh to collect non-sensitive Azure config for cloud_connection setup.

**Files:** Modify `install.sh`

Add a new section (after the existing cloud credential checks) that:
1. Detects the cloud provider context (Azure managed identity via IMDS, or manual config)
2. Collects: tenant_id, subscription_id, resource_group
3. Validates via `az account show --subscription <sub_id>` (uses managed identity, no secrets)
4. Prints the RBAC requirement: "Ensure the managed identity has Virtual Machine Contributor on resource group <rg>"
5. Stores the config in a local file (`/etc/networker/cloud-config.json`) for the dashboard to read at startup, OR outputs a curl command to POST the cloud_connection to the dashboard API

The installer does NOT collect or store any passwords, client secrets, or API keys.

- [ ] Step 1: Add Azure identity config collection
- [ ] Step 2: Add validation via `az account show`
- [ ] Step 3: `shellcheck install.sh` clean
- [ ] Step 4: `bats tests/installer.bats` passes
- [ ] Step 5: Commit: `feat(installer): secretless Azure identity configuration`

---

### Task 8: Security audit tests + CHANGELOG + version bump

**Goal:** Automated tests verifying zero-secret compliance + ship.

**Files:**
- Create test in `crates/networker-dashboard/src/services/cloud_provider.rs` — grep guard ensuring no `credentials_enc`, `credentials_nonce`, or `crypto::decrypt` calls in any `services/cloud_provider*` or `services/tester_*` file
- CHANGELOG, Cargo.toml, install.sh, install.ps1 version bump (0.25.1 → 0.26.0)

Security grep guard:
```rust
#[test]
fn cloud_provider_never_touches_stored_credentials() {
    // Walk services/ and api/testers.rs, fail if any file imports crypto::decrypt
    // or references credentials_enc/credentials_nonce.
    // This enforces the FIC principle: tester provisioning is secretless.
}
```

Acceptance criteria verification:
- [ ] `az account show` succeeds on the dashboard VM (managed identity auth)
- [ ] `az vm create` includes `--subscription` and `--resource-group` in every invocation
- [ ] `cloud_connection.config` contains zero secret fields
- [ ] No `DASHBOARD_CREDENTIAL_KEY` usage in tester provisioning code paths
- [ ] `cargo test --workspace --lib` passes
- [ ] `bash tests/cli_smoke.sh` passes
- [ ] `cd dashboard && npm run build && npm run lint && npm test` clean

- [ ] Step 1: Write security grep guard test
- [ ] Step 2: Version bump (0.25.1 → 0.26.0)
- [ ] Step 3: CHANGELOG entry
- [ ] Step 4: Full validation checklist
- [ ] Step 5: Commit + push + PR: `feat: FIC-compliant cloud provisioning (v0.26.0)`

---

## Final verification before merge

- [ ] `cargo fmt --all` clean
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo test --workspace --lib` passes
- [ ] `cargo build --workspace` clean
- [ ] `cargo build -p networker-tester --no-default-features` clean
- [ ] `bash tests/cli_smoke.sh` — all scenarios pass
- [ ] `shellcheck install.sh` clean
- [ ] `bats tests/installer.bats` passes
- [ ] `cd dashboard && npm run build && npm run lint && npm test` clean
- [ ] No `credentials_enc` / `crypto::decrypt` in tester provisioning code paths
- [ ] All `az` commands include `--subscription` and `--resource-group`
- [ ] PR description references the FIC design spec
