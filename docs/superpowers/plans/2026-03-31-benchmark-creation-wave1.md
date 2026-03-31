# Benchmark Creation — Wave 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the foundational DB schema, API endpoints, orchestrator callback infrastructure, wizard UI, and VM catalog — the 4 independent streams that unblock Wave 2.

**Architecture:** Wave 1 has 4 parallel streams with zero cross-dependencies. S0 modifies the orchestrator binary (Rust). S1 adds new DB tables and REST endpoints to the dashboard (Rust + PostgreSQL). S3 builds the React wizard UI. S4 adds a VM catalog management page. Each stream produces independently testable, committable code.

**Tech Stack:** Rust (axum, tokio-postgres, tokio::process), React 19, TypeScript, Tailwind 4, PostgreSQL 16

**Spec:** `docs/superpowers/specs/2026-03-31-benchmark-creation-orchestration-design.md`

---

## Stream S0: Orchestrator Foundations

### Task S0.1: Heartbeat loop + callback URL flag

**Files:**
- Modify: `benchmarks/orchestrator/src/main.rs` — add `--callback-url` and `--callback-token` CLI flags
- Create: `benchmarks/orchestrator/src/callback.rs` — HTTP callback client
- Modify: `benchmarks/orchestrator/Cargo.toml` — ensure `reqwest` dependency

- [ ] **Step 1: Add CLI flags**

Add to the `Run` subcommand in clap: `--callback-url` (Option String), `--callback-token` (Option String).

- [ ] **Step 2: Create callback.rs with CallbackClient**

Struct with `base_url`, `token`, `client` (reqwest), `config_id`. Methods: `status()`, `log()`, `result()`, `complete()`, `heartbeat()`, `check_cancelled()`. Each POSTs JSON to `{base_url}/api/benchmarks/callback/{endpoint}` with Bearer token header.

- [ ] **Step 3: Add heartbeat background task**

In run command handler, if callback_url is set, spawn a tokio task that calls heartbeat every 60s and checks cancellation.

- [ ] **Step 4: Register module, build, verify**

Run: `cargo build --manifest-path benchmarks/orchestrator/Cargo.toml`

- [ ] **Step 5: Commit**

### Task S0.2: Graceful shutdown + PID file

**Files:**
- Modify: `benchmarks/orchestrator/src/main.rs` — PID file write, SIGTERM handler

- [ ] **Step 1: Write PID file on start**

Write `std::process::id()` to `/tmp/alethabench-{config_id}.pid`. Remove on exit.

- [ ] **Step 2: Add SIGTERM handler with tokio::select**

On SIGTERM: log, signal cancellation to main loop, teardown provisioned VMs, exit cleanly.

- [ ] **Step 3: Build, verify, commit**

---

## Stream S1: Database + API

### Task S1.1: V016 migration

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs`

- [ ] **Step 1: Add V016 SQL**

Creates 3 tables: `benchmark_vm_catalog`, `benchmark_config`, `benchmark_cell` with all columns from the spec. Includes indexes on project_id and status.

- [ ] **Step 2: Add migration check following V015 pattern**
- [ ] **Step 3: Build, start dashboard against local postgres, verify V016 applies**
- [ ] **Step 4: Commit**

### Task S1.2: DB module — benchmark_configs.rs

**Files:**
- Create: `crates/networker-dashboard/src/db/benchmark_configs.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`

- [ ] **Step 1: Implement BenchmarkConfigRow struct + create/get/list**
- [ ] **Step 2: Implement update_status, claim_queued (atomic), update_heartbeat, find_stalled**
- [ ] **Step 3: Register module, build, commit**

### Task S1.3: DB module — benchmark_cells.rs

**Files:**
- Create: `crates/networker-dashboard/src/db/benchmark_cells.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`

- [ ] **Step 1: Implement BenchmarkCellRow + create, list_for_config, update_status, update_vm_info**
- [ ] **Step 2: Build, commit**

### Task S1.4: DB module — benchmark_vm_catalog.rs

**Files:**
- Create: `crates/networker-dashboard/src/db/benchmark_vm_catalog.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`

- [ ] **Step 1: Implement VmCatalogRow + create, get, list_for_project, update_status, update_languages, delete**
- [ ] **Step 2: Build, commit**

### Task S1.5: API — benchmark config endpoints

**Files:**
- Create: `crates/networker-dashboard/src/api/benchmark_configs.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`

Project-scoped routes (Operator role required):
- `GET /projects/:pid/benchmark-configs` — list
- `POST /projects/:pid/benchmark-configs` — create (wizard submit)
- `GET /projects/:pid/benchmark-configs/:id` — get with cells
- `POST /projects/:pid/benchmark-configs/:id/launch` — set queued
- `POST /projects/:pid/benchmark-configs/:id/cancel` — set cancelled

- [ ] **Step 1: Implement all 5 handlers**
- [ ] **Step 2: Register in mod.rs project_scoped section**
- [ ] **Step 3: Build, commit**

### Task S1.6: API — callback endpoints

**Files:**
- Create: `crates/networker-dashboard/src/api/benchmark_callbacks.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`

Public routes (verified via callback JWT internally):
- `POST /api/benchmarks/callback/status`
- `POST /api/benchmarks/callback/log`
- `POST /api/benchmarks/callback/result`
- `POST /api/benchmarks/callback/complete`
- `POST /api/benchmarks/callback/heartbeat`
- `GET /api/benchmarks/callback/cancelled/:config_id`

Each handler verifies the JWT, updates DB, broadcasts to WebSocket.

- [ ] **Step 1: Implement status + heartbeat + cancelled handlers**
- [ ] **Step 2: Implement log handler (broadcast via events_tx)**
- [ ] **Step 3: Implement result handler (save BenchmarkArtifact to pipeline tables)**
- [ ] **Step 4: Implement complete handler**
- [ ] **Step 5: Register routes, build, commit**

### Task S1.7: API — VM catalog endpoints

**Files:**
- Create: `crates/networker-dashboard/src/api/benchmark_catalog.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`

Project-scoped:
- `GET /projects/:pid/benchmark-catalog` — list
- `POST /projects/:pid/benchmark-catalog` — register
- `DELETE /projects/:pid/benchmark-catalog/:vm_id` — remove
- `POST /projects/:pid/benchmark-catalog/:vm_id/detect` — SSH detect languages

- [ ] **Step 1: Implement CRUD handlers**
- [ ] **Step 2: Implement detect handler — SSH to VM, check for language binaries, update catalog row**
- [ ] **Step 3: Build, commit**

### Task S1.8: Benchmark worker loop

**Files:**
- Create: `crates/networker-dashboard/src/benchmark_worker.rs`
- Modify: `crates/networker-dashboard/src/main.rs` — spawn worker

Worker loop:
1. Poll for queued configs every 5s
2. Claim via atomic UPDATE
3. Write config JSON to temp file
4. Generate scoped callback JWT
5. Spawn alethabench as child process
6. Monitor exit, update status

Cleanup: every 15 min, find stalled configs (no heartbeat 10 min), mark failed.

- [ ] **Step 1: Implement worker polling + claim logic**
- [ ] **Step 2: Implement process spawn + monitoring**
- [ ] **Step 3: Implement cleanup task**
- [ ] **Step 4: Add tokio::spawn in main.rs**
- [ ] **Step 5: Build, commit**

---

## Stream S3: Wizard UI

### Task S3.1: Wizard page skeleton + routing

**Files:**
- Create: `dashboard/src/pages/BenchmarkWizardPage.tsx`
- Modify: `dashboard/src/App.tsx`
- Modify: `dashboard/src/pages/BenchmarksPage.tsx`

- [ ] **Step 1: Create 5-step wizard with stepper navigation, Next/Back, state management**
- [ ] **Step 2: Add route and "New Benchmark" button on Benchmarks page**
- [ ] **Step 3: Build, commit**

### Task S3.2: Step 1 — Template selector

- [ ] **Step 1: 4 template cards (Quick Check, Regional Comparison, Cross-Cloud, Custom)**
- [ ] **Step 2: Pre-fill wizard state on template selection, build, commit**

### Task S3.3: Step 2 — Cell editor

- [ ] **Step 1: Add Cell cards with cloud/region/topology/vm-size/catalog-vm dropdowns**
- [ ] **Step 2: Build, commit**

### Task S3.4: Step 3 — Language selector

- [ ] **Step 1: Grouped checkboxes (Systems/Managed/Scripting/Static), shortcuts, nginx baseline**
- [ ] **Step 2: Build, commit**

### Task S3.5: Step 4 — Methodology

- [ ] **Step 1: Preset buttons (Quick/Standard/Rigorous) + advanced toggle with all fields**
- [ ] **Step 2: Build, commit**

### Task S3.6: Step 5 — Review and launch

- [ ] **Step 1: Summary cards per cell, totals, auto-teardown checkbox, Launch button**
- [ ] **Step 2: Connect to API (create config → launch), build, commit**

### Task S3.7: API client types + methods

**Files:**
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/api/types.ts`

- [ ] **Step 1: Add TypeScript interfaces for BenchmarkConfig, BenchmarkCell, BenchmarkVmCatalog**
- [ ] **Step 2: Add API methods for config CRUD, launch, cancel, catalog CRUD, detect**
- [ ] **Step 3: Build, commit**

---

## Stream S4: VM Catalog Page

### Task S4.1: Catalog management page

**Files:**
- Create: `dashboard/src/pages/BenchmarkCatalogPage.tsx`
- Modify: `dashboard/src/App.tsx`
- Modify: `dashboard/src/components/layout/Sidebar.tsx`

- [ ] **Step 1: VM list table (name, cloud, region, IP, languages tags, status, actions)**
- [ ] **Step 2: Register VM dialog (name, IP, SSH user, cloud, region)**
- [ ] **Step 3: Detect Languages button (calls API, shows spinner, updates tags)**
- [ ] **Step 4: Add route + sidebar link (under Benchmarks, admin only)**
- [ ] **Step 5: Build, commit**

### Task S4.2: Backend SSH language detection

**Files:**
- Modify: `crates/networker-dashboard/src/api/benchmark_catalog.rs`

- [ ] **Step 1: SSH to VM, check for each language binary/file path**

Check paths: rust-server, go-server, cpp-build/server, nodejs/server.js, python/server.py, java/server.jar, ruby/config.ru, php/server.php, nginx binary. Also detect csharp-net* versions.

- [ ] **Step 2: Update catalog row with detected languages array**
- [ ] **Step 3: Build, commit**

---

## Verification Checklist

After all 4 streams complete:

- [ ] `cargo build --manifest-path benchmarks/orchestrator/Cargo.toml` — S0 builds
- [ ] `cargo build -p networker-dashboard` — S1 builds
- [ ] `cd dashboard && npm run build` — S3 + S4 build
- [ ] Dashboard starts, V016 migration applies
- [ ] `POST /projects/:pid/benchmark-configs` creates a config
- [ ] `POST /projects/:pid/benchmark-catalog` registers a VM
- [ ] Wizard navigates through all 5 steps
- [ ] Catalog page shows VMs with detected languages

Wave 2 (S2, S5, S6, S7) starts immediately after.
