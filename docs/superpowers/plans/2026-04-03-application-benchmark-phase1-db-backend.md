# Application Benchmark Mode — Phase 1: Database & Backend Core

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add database schema, API endpoints, and worker support for Application benchmark configs with proxy selection and tester OS.

**Architecture:** Extend the existing `benchmark_config`/`benchmark_testbed` tables with new columns for `benchmark_type`, `proxies`, and `tester_os`. Add a parallel set of API endpoints under the same router. Reuse the existing worker poll-and-run pattern, passing `benchmark_type` to the orchestrator config.

**Tech Stack:** PostgreSQL, tokio-postgres, axum 0.7, serde, uuid

**Spec:** `docs/superpowers/specs/2026-04-03-application-benchmark-mode-design.md`

**Depends on:** Nothing (this is the foundation)

**Produces:** Working API that accepts Application benchmark configs, stores them, and passes them to the orchestrator

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `crates/networker-dashboard/src/db/migrations.rs` | V022 migration: add columns to existing tables |
| Modify | `crates/networker-dashboard/src/db/benchmark_configs.rs` | Add `benchmark_type` to row struct and queries |
| Modify | `crates/networker-dashboard/src/db/benchmark_testbeds.rs` | Add `proxies` and `tester_os` to row struct and queries |
| Modify | `crates/networker-dashboard/src/api/benchmark_configs.rs` | Extend request/response types, add proxy validation |
| Modify | `crates/networker-dashboard/src/benchmark_worker.rs` | Pass `benchmark_type` + `proxies` + `tester_os` to orchestrator config |
| Modify | `benchmarks/orchestrator/src/config.rs` | Add `benchmark_type`, `proxies`, `tester_os` to config structs |

---

### Task 1: V022 Migration — Add columns to benchmark tables

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs:982-985`

- [ ] **Step 1: Write the migration SQL constant**

Add after the `V021_BENCHMARK_REQUEST_PROGRESS` constant:

```rust
const V022_APPLICATION_BENCHMARK: &str = "
    ALTER TABLE benchmark_config
        ADD COLUMN IF NOT EXISTS benchmark_type TEXT NOT NULL DEFAULT 'fullstack';

    ALTER TABLE benchmark_testbed
        ADD COLUMN IF NOT EXISTS proxies JSONB NOT NULL DEFAULT '[]'::jsonb,
        ADD COLUMN IF NOT EXISTS tester_os TEXT NOT NULL DEFAULT 'server';

    CREATE INDEX IF NOT EXISTS ix_benchmark_config_type
        ON benchmark_config (benchmark_type);
";
```

- [ ] **Step 2: Add V022 migration block to `run_migrations()`**

Add after the V021 block (after line 982):

```rust
    // V022: Application benchmark mode — add benchmark_type, proxies, tester_os
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 22", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V022 application benchmark migration...");
        client.batch_execute(V022_APPLICATION_BENCHMARK).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (22) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V022 migration complete");
    }
```

- [ ] **Step 3: Build and verify migration compiles**

Run: `cargo build -p networker-dashboard`
Expected: Compiles without errors

- [ ] **Step 4: Test migration against local PostgreSQL**

Run:
```bash
docker compose -f docker-compose.dashboard.yml up -d postgres
DASHBOARD_DB_URL="postgres://networker:test@127.0.0.1:5432/networker" DASHBOARD_ADMIN_PASSWORD=test cargo run -p networker-dashboard &
sleep 3
kill %1
```
Expected: Logs show "V022 migration complete"

Verify columns exist:
```bash
docker compose -f docker-compose.dashboard.yml exec postgres psql -U networker -c "\d benchmark_config" | grep benchmark_type
docker compose -f docker-compose.dashboard.yml exec postgres psql -U networker -c "\d benchmark_testbed" | grep -E "proxies|tester_os"
```
Expected: Both columns visible

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/db/migrations.rs
git commit -m "feat(db): V022 migration — add benchmark_type, proxies, tester_os columns"
```

---

### Task 2: Update BenchmarkConfigRow with benchmark_type

**Files:**
- Modify: `crates/networker-dashboard/src/db/benchmark_configs.rs`

- [ ] **Step 1: Add `benchmark_type` field to `BenchmarkConfigRow`**

```rust
#[derive(Debug, Serialize)]
pub struct BenchmarkConfigRow {
    pub config_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub template: Option<String>,
    pub status: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub config_json: serde_json::Value,
    pub error_message: Option<String>,
    pub max_duration_secs: i32,
    pub baseline_run_id: Option<Uuid>,
    pub worker_id: Option<String>,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub benchmark_type: String,
}
```

- [ ] **Step 2: Update `row_to_config` to read `benchmark_type`**

Add to `row_to_config`:

```rust
        benchmark_type: r.get("benchmark_type"),
```

- [ ] **Step 3: Update all SELECT queries to include `benchmark_type`**

In `get()`, `list()`, `claim_queued()`, `find_stalled()` — add `benchmark_type` to every SELECT column list.

For `get()` (line 81):
```sql
SELECT config_id, project_id, name, template, status, created_by,
       created_at, started_at, finished_at, config_json, error_message,
       max_duration_secs, baseline_run_id, worker_id, last_heartbeat,
       benchmark_type
FROM benchmark_config WHERE config_id = $1
```

Apply the same pattern to `list()` (line 99), `claim_queued()` (line 153), and `find_stalled()` (line 179).

- [ ] **Step 4: Update `create()` to accept and insert `benchmark_type`**

```rust
#[allow(clippy::too_many_arguments)]
pub async fn create(
    client: &Client,
    project_id: &Uuid,
    name: &str,
    template: Option<&str>,
    config_json: &serde_json::Value,
    created_by: Option<&Uuid>,
    max_duration_secs: i32,
    baseline_run_id: Option<&Uuid>,
    benchmark_type: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO benchmark_config
                (config_id, project_id, name, template, config_json, created_by,
                 max_duration_secs, baseline_run_id, benchmark_type)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                &id,
                project_id,
                &name,
                &template,
                config_json,
                &created_by,
                &max_duration_secs,
                &baseline_run_id,
                &benchmark_type,
            ],
        )
        .await?;
    Ok(id)
}
```

- [ ] **Step 5: Build to verify**

Run: `cargo build -p networker-dashboard`
Expected: Compile error at call site in `api/benchmark_configs.rs` — that's expected, we fix it in Task 4

- [ ] **Step 6: Commit**

```bash
git add crates/networker-dashboard/src/db/benchmark_configs.rs
git commit -m "feat(db): add benchmark_type to BenchmarkConfigRow and queries"
```

---

### Task 3: Update BenchmarkTestbedRow with proxies and tester_os

**Files:**
- Modify: `crates/networker-dashboard/src/db/benchmark_testbeds.rs`

- [ ] **Step 1: Add `proxies` and `tester_os` fields to `BenchmarkTestbedRow`**

```rust
#[derive(Debug, Serialize)]
pub struct BenchmarkTestbedRow {
    pub testbed_id: Uuid,
    pub config_id: Uuid,
    pub cloud: String,
    pub region: String,
    pub topology: String,
    pub endpoint_vm_id: Option<String>,
    pub tester_vm_id: Option<String>,
    pub endpoint_ip: Option<String>,
    pub tester_ip: Option<String>,
    pub status: String,
    pub languages: serde_json::Value,
    pub vm_size: Option<String>,
    pub os: String,
    pub proxies: serde_json::Value,
    pub tester_os: String,
}
```

- [ ] **Step 2: Update `row_to_testbed`**

Add:
```rust
        proxies: r.get("proxies"),
        tester_os: r.get("tester_os"),
```

- [ ] **Step 3: Update `list_for_config` SELECT**

```sql
SELECT testbed_id, config_id, cloud, region, topology, endpoint_vm_id,
       tester_vm_id, endpoint_ip, tester_ip, status, languages, vm_size, os,
       proxies, tester_os
FROM benchmark_testbed WHERE config_id = $1
ORDER BY cloud, region
```

- [ ] **Step 4: Update `create()` to accept `proxies` and `tester_os`**

```rust
#[allow(clippy::too_many_arguments)]
pub async fn create(
    client: &Client,
    config_id: &Uuid,
    cloud: &str,
    region: &str,
    topology: &str,
    languages: &serde_json::Value,
    vm_size: Option<&str>,
    os: &str,
    proxies: &serde_json::Value,
    tester_os: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO benchmark_testbed
                (testbed_id, config_id, cloud, region, topology, languages, vm_size, os, proxies, tester_os)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
            &[
                &id, config_id, &cloud, &region, &topology, languages, &vm_size, &os,
                proxies, &tester_os,
            ],
        )
        .await?;
    Ok(id)
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/db/benchmark_testbeds.rs
git commit -m "feat(db): add proxies and tester_os to BenchmarkTestbedRow"
```

---

### Task 4: Update API — CreateBenchmarkConfigRequest with new fields

**Files:**
- Modify: `crates/networker-dashboard/src/api/benchmark_configs.rs`

- [ ] **Step 1: Add `benchmark_type` to `CreateBenchmarkConfigRequest`**

Add after `auto_teardown`:

```rust
    #[serde(default = "default_benchmark_type")]
    pub benchmark_type: String,
```

Add the default function:

```rust
fn default_benchmark_type() -> String {
    "fullstack".to_string()
}
```

- [ ] **Step 2: Add `proxies` and `tester_os` to `TestbedInput`**

```rust
#[derive(Debug, Deserialize, Serialize)]
pub struct TestbedInput {
    pub cloud: String,
    pub region: String,
    #[serde(default = "default_topology")]
    pub topology: String,
    #[serde(default)]
    pub languages: serde_json::Value,
    pub vm_size: Option<String>,
    pub existing_vm_ip: Option<String>,
    pub os: Option<String>,
    #[serde(default)]
    pub proxies: Vec<String>,
    #[serde(default = "default_tester_os")]
    pub tester_os: String,
}

fn default_tester_os() -> String {
    "server".to_string()
}
```

- [ ] **Step 3: Add validation for `benchmark_type` and `proxies`**

In `create_config()`, after the name length check (line 131):

```rust
    // Validate benchmark_type
    if !["fullstack", "application"].contains(&payload.benchmark_type.as_str()) {
        tracing::warn!(benchmark_type = %payload.benchmark_type, "Invalid benchmark type");
        return Err(StatusCode::BAD_REQUEST);
    }

    // For application mode, each testbed must have at least one proxy
    if payload.benchmark_type == "application" {
        for testbed in &payload.testbeds {
            if testbed.proxies.is_empty() {
                tracing::warn!("Application benchmark testbed missing proxies");
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    // Validate proxy names
    const VALID_PROXIES: &[&str] = &["nginx", "iis", "caddy", "traefik", "haproxy", "apache"];
    for testbed in &payload.testbeds {
        for proxy in &testbed.proxies {
            if !VALID_PROXIES.contains(&proxy.as_str()) {
                tracing::warn!(proxy = %proxy, "Invalid proxy name");
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    // Validate tester_os
    const VALID_TESTER_OS: &[&str] = &["server", "desktop-linux", "desktop-windows"];
    for testbed in &payload.testbeds {
        if !VALID_TESTER_OS.contains(&testbed.tester_os.as_str()) {
            tracing::warn!(tester_os = %testbed.tester_os, "Invalid tester OS");
            return Err(StatusCode::BAD_REQUEST);
        }
    }
```

- [ ] **Step 4: Update config_json builder to include new fields**

Update the `config_json` builder (line 136) to include `benchmark_type`, `proxies`, and `tester_os`:

```rust
    let config_json = payload.config_json.unwrap_or_else(|| {
        serde_json::json!({
            "benchmark_type": payload.benchmark_type,
            "languages": payload.languages,
            "methodology": payload.methodology,
            "auto_teardown": payload.auto_teardown.unwrap_or(true),
            "testbeds": payload.testbeds.iter().map(|t| serde_json::json!({
                "cloud": t.cloud,
                "region": t.region,
                "topology": t.topology,
                "vm_size": t.vm_size,
                "languages": t.languages,
                "existing_vm_ip": t.existing_vm_ip,
                "os": t.os,
                "proxies": t.proxies,
                "tester_os": t.tester_os,
            })).collect::<Vec<_>>(),
        })
    });
```

- [ ] **Step 5: Update `create()` call to pass `benchmark_type`**

```rust
    let config_id = crate::db::benchmark_configs::create(
        &client,
        &ctx.project_id,
        &payload.name,
        payload.template.as_deref(),
        &config_json,
        Some(&user.user_id),
        payload.max_duration_secs,
        payload.baseline_run_id.as_ref(),
        &payload.benchmark_type,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create benchmark config");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
```

- [ ] **Step 6: Update testbed `create()` calls to pass `proxies` and `tester_os`**

```rust
    for testbed in &payload.testbeds {
        let proxies_json = serde_json::to_value(&testbed.proxies).unwrap_or_default();
        let testbed_id = crate::db::benchmark_testbeds::create(
            &client,
            &config_id,
            &testbed.cloud,
            &testbed.region,
            &testbed.topology,
            &testbed.languages,
            testbed.vm_size.as_deref(),
            testbed.os.as_deref().unwrap_or("linux"),
            &proxies_json,
            &testbed.tester_os,
        )
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create benchmark testbed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        testbed_ids.push(testbed_id);
    }
```

- [ ] **Step 7: Build to verify**

Run: `cargo build -p networker-dashboard`
Expected: Compiles successfully

- [ ] **Step 8: Commit**

```bash
git add crates/networker-dashboard/src/api/benchmark_configs.rs
git commit -m "feat(api): extend benchmark config API with benchmark_type, proxies, tester_os"
```

---

### Task 5: Update Orchestrator Config Structs

**Files:**
- Modify: `benchmarks/orchestrator/src/config.rs`

- [ ] **Step 1: Add `benchmark_type` to `DashboardBenchmarkConfig`**

Add after `callback_token`:

```rust
    #[serde(default = "default_benchmark_type")]
    pub benchmark_type: String,
```

Add:
```rust
fn default_benchmark_type() -> String {
    "fullstack".to_string()
}
```

- [ ] **Step 2: Add `proxies` and `tester_os` to `TestbedConfig`**

Add after `os`:

```rust
    #[serde(default)]
    pub proxies: Vec<String>,

    #[serde(default = "default_tester_os")]
    pub tester_os: String,
```

Add:
```rust
fn default_tester_os() -> String {
    "server".to_string()
}
```

- [ ] **Step 3: Update `validate()` to check application mode constraints**

Add to the `validate()` method:

```rust
        if self.benchmark_type == "application" {
            for testbed in &self.testbeds {
                if testbed.proxies.is_empty() {
                    anyhow::bail!(
                        "Application benchmark testbed '{}' requires at least one proxy",
                        testbed.testbed_id
                    );
                }
            }
        }
```

- [ ] **Step 4: Build to verify**

Run: `cargo build -p alethabench`
Expected: Compiles successfully (orchestrator binary name is `alethabench`)

- [ ] **Step 5: Commit**

```bash
git add benchmarks/orchestrator/src/config.rs
git commit -m "feat(orchestrator): add benchmark_type, proxies, tester_os to config structs"
```

---

### Task 6: Update Benchmark Worker to Pass New Fields

**Files:**
- Modify: `crates/networker-dashboard/src/benchmark_worker.rs`

- [ ] **Step 1: Read the current worker config-building code**

Read `benchmark_worker.rs` to find where the orchestrator config JSON is built (around the temp file write).

- [ ] **Step 2: Include `benchmark_type`, `proxies`, and `tester_os` in the orchestrator config file**

When building the config JSON for the orchestrator, the worker already merges DB testbeds with config_json. The new fields (`benchmark_type`, `proxies`, `tester_os`) are already in the DB rows (from Task 2 and 3), so they'll be included in the serialized config_json.

Verify that the testbed merge logic includes the new fields. If the worker manually constructs the testbed objects (rather than using config_json passthrough), add the new fields:

```rust
    "proxies": testbed_row.proxies,
    "tester_os": testbed_row.tester_os,
```

And at the top level:
```rust
    "benchmark_type": config_row.benchmark_type,
```

- [ ] **Step 3: Build to verify**

Run: `cargo build -p networker-dashboard`
Expected: Compiles successfully

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/benchmark_worker.rs
git commit -m "feat(worker): pass benchmark_type, proxies, tester_os to orchestrator"
```

---

### Task 7: Integration Test — Create Application Benchmark Config via API

**Files:**
- This is a manual curl test against the running dashboard

- [ ] **Step 1: Start local dev stack**

```bash
docker compose -f docker-compose.dashboard.yml up -d postgres
DASHBOARD_ADMIN_PASSWORD=admin cargo run -p networker-dashboard &
sleep 3
```

- [ ] **Step 2: Login and get token**

```bash
TOKEN=$(curl -s http://localhost:3000/api/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@localhost","password":"admin"}' | jq -r '.token')
echo $TOKEN
```

- [ ] **Step 3: Get project ID**

```bash
PROJECT_ID=$(curl -s http://localhost:3000/api/projects \
  -H "Authorization: Bearer $TOKEN" | jq -r '.[0].project_id')
echo $PROJECT_ID
```

- [ ] **Step 4: Create an Application benchmark config**

```bash
curl -s http://localhost:3000/api/projects/$PROJECT_ID/benchmark-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Test Application Benchmark",
    "benchmark_type": "application",
    "testbeds": [{
      "cloud": "azure",
      "region": "eastus",
      "topology": "loopback",
      "languages": ["rust", "python"],
      "vm_size": "Standard_D4s_v3",
      "os": "linux",
      "proxies": ["nginx", "caddy"],
      "tester_os": "server"
    }],
    "languages": ["rust", "python"],
    "methodology": {
      "preset": "quick",
      "warmup_runs": 5,
      "measured_runs": 10,
      "modes": ["http1", "http2", "http3"]
    },
    "auto_teardown": true
  }' | jq .
```

Expected: `{ "config_id": "...", "testbed_ids": ["..."] }`

- [ ] **Step 5: Verify the config was stored with correct benchmark_type**

```bash
CONFIG_ID=$(curl -s http://localhost:3000/api/projects/$PROJECT_ID/benchmark-configs \
  -H "Authorization: Bearer $TOKEN" | jq -r '.[0].config_id')

curl -s http://localhost:3000/api/projects/$PROJECT_ID/benchmark-configs/$CONFIG_ID \
  -H "Authorization: Bearer $TOKEN" | jq '.config.benchmark_type, .testbeds[0].proxies, .testbeds[0].tester_os'
```

Expected: `"application"`, `["nginx","caddy"]`, `"server"`

- [ ] **Step 6: Verify validation — application mode requires proxies**

```bash
curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/api/projects/$PROJECT_ID/benchmark-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Bad Config",
    "benchmark_type": "application",
    "testbeds": [{
      "cloud": "azure",
      "region": "eastus",
      "languages": ["rust"],
      "proxies": []
    }]
  }'
```

Expected: `400`

- [ ] **Step 7: Verify fullstack mode still works (backward compatible)**

```bash
curl -s http://localhost:3000/api/projects/$PROJECT_ID/benchmark-configs \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Test Full Stack (no benchmark_type field)",
    "testbeds": [{
      "cloud": "azure",
      "region": "eastus",
      "languages": ["rust"]
    }]
  }' | jq .
```

Expected: `{ "config_id": "...", "testbed_ids": ["..."] }` — defaults to `fullstack`

- [ ] **Step 8: Cleanup and commit**

```bash
kill %1
git add -A
git commit -m "test: verify application benchmark config API end-to-end"
```

---

## Phase Summary

After completing all 7 tasks, the system supports:

1. **Database:** `benchmark_type`, `proxies`, `tester_os` columns with migration
2. **API:** Create/read application benchmark configs with proxy validation
3. **Orchestrator:** Config structs accept the new fields
4. **Worker:** Passes new fields through to the orchestrator subprocess
5. **Backward compatible:** Existing fullstack configs work unchanged (defaults apply)

**Next phases (separate plans):**
- Phase 2: Orchestrator execution loop for Application mode (proxy swap, Chrome harness integration)
- Phase 3: Installer `--benchmark-proxy` flag and proxy deploy functions
- Phase 4: Dashboard frontend — sidebar restructure + Application wizard
- Phase 5: Chrome test harness (determinism flags, collection JS, protocol validation)
- Phase 6: Reference API JSON endpoints (18 languages)
- Phase 7: Golden run validation template
