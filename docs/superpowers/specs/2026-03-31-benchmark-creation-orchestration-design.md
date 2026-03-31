# Benchmark Creation & Orchestration Design

## Goal

Enable users to create, configure, and run cross-language, cross-cloud benchmarks from the dashboard UI, with full orchestrator integration for provisioning, deployment, measurement, and result persistence.

## Architecture

```
User → Dashboard UI (wizard) → Dashboard API → Orchestrator CLI (alethabench)
                                                    ↓
                                              Provisioner  → Azure/AWS/GCP VMs
                                              Deployer     → SSH deploy languages
                                              Runner       → networker-tester --benchmark
                                              Collector    → resource metrics
                                                    ↓
                                              HTTP callbacks → Dashboard API
                                                    ↓
                                              PostgreSQL (benchmark pipeline tables)
                                                    ↓
                                              Dashboard UI (progress, results, compare)
```

Dashboard is the control plane (config, storage, display). Orchestrator is the execution engine (provisioning, deployment, measurement). They communicate via CLI arguments and HTTP callbacks.

---

## 1. Benchmark Modes

Two modes, both available from the same wizard:

**Single endpoint** — one target, one tester, tagged as benchmark with full methodology. Quick check against a known endpoint. In the cell model, this is a single cell with `topology: external` — no provisioning, no language deployment. The user provides the target URL directly (e.g., an existing networker-endpoint deployment). The orchestrator runs `networker-tester --benchmark` against it and reports the artifact.

**Multi-language sweep** — one or more cloud/region cells, each running multiple language servers sequentially. The core comparison use case.

---

## 2. Cell Model

A benchmark consists of one or more **cells**. Each cell is an independent cloud/region/topology unit.

### Topologies

| Topology | Tester | Endpoint | Measures |
|----------|--------|----------|----------|
| Loopback | Same VM | Same VM | Pure server performance |
| Same-region | Separate VM | Separate VM | Server + intra-region network |
| Cross-region | VM in region A | VM in region B | Server + inter-region network |

### Multi-cloud comparison

User selects multiple clouds/regions. Each gets its own cell with tester + endpoint. Results grouped by cell for side-by-side comparison.

Example: User selects Azure eastus + AWS us-east-1 + GCP us-central1 → 3 cells, each benchmarks the same languages, compare Azure vs AWS vs GCP.

### Loopback vs same-region

Loopback runs tester and all language servers on the same VM. Same-region provisions a separate tester VM in the same region. User chooses per cell. Loopback is the default for cost efficiency.

---

## 3. Data Model

### benchmark_config

The user's benchmark request, created by the wizard and stored before execution.

| Column | Type | Description |
|--------|------|-------------|
| config_id | UUID PK | |
| project_id | UUID FK → project | Workspace scope |
| name | VARCHAR | User-provided name |
| template | VARCHAR NULL | Template used (quick-check, cross-cloud, etc.) |
| status | VARCHAR | draft, queued, provisioning, deploying, running, completed, failed, cancelled |
| created_by | UUID FK → dash_user | |
| created_at | TIMESTAMPTZ | |
| started_at | TIMESTAMPTZ NULL | When orchestrator started |
| finished_at | TIMESTAMPTZ NULL | |
| config_json | JSONB | Full config (cells, languages, methodology) |
| error_message | TEXT NULL | |
| max_duration_secs | INT | Hard timeout (default 14400 = 4 hours) |
| baseline_run_id | UUID NULL | Reference run for regression detection |

### benchmark_cell

One cloud/region/topology unit within a benchmark.

| Column | Type | Description |
|--------|------|-------------|
| cell_id | UUID PK | |
| config_id | UUID FK → benchmark_config | |
| cloud | VARCHAR | azure, aws, gcp |
| region | VARCHAR | eastus, us-east-1, etc. |
| topology | VARCHAR | loopback, same-region |
| endpoint_vm_id | VARCHAR NULL | Cloud resource ID after provisioning |
| tester_vm_id | VARCHAR NULL | NULL for loopback |
| endpoint_ip | VARCHAR NULL | |
| tester_ip | VARCHAR NULL | |
| status | VARCHAR | pending, provisioning, deployed, running, completed, failed, torn_down |
| languages | JSONB | Array of language identifiers |
| vm_size | VARCHAR | Cloud-specific VM size |

### benchmark_vm_catalog

Registry of known VMs with pre-deployed languages.

| Column | Type | Description |
|--------|------|-------------|
| vm_id | UUID PK | |
| project_id | UUID FK → project | |
| name | VARCHAR | Display name |
| cloud | VARCHAR | azure, aws, gcp, manual |
| region | VARCHAR | |
| ip | VARCHAR | Public IP or FQDN |
| ssh_user | VARCHAR | Default: azureuser |
| languages | JSONB | Detected/registered languages |
| vm_size | VARCHAR NULL | |
| status | VARCHAR | online, offline, unknown |
| last_health_check | TIMESTAMPTZ NULL | |
| created_by | UUID FK → dash_user | |
| created_at | TIMESTAMPTZ | |

Each cell produces one or more `benchmarkrun` rows in the existing pipeline tables (one per language).

---

## 4. Wizard UI

### Step 1: Template / Scenario

Templates pre-fill subsequent steps:

- **Quick Check** — 1 cell, loopback, top 5 languages, 10 measured runs
- **Regional Comparison** — 2+ cells same cloud, same-region topology, all languages
- **Cross-Cloud** — Azure + AWS + GCP, loopback, all languages
- **Custom** — blank slate

### Step 2: Cells

"Add Cell" button opens a card editor:
- Cloud selector (Azure / AWS / GCP)
- Region dropdown (populated per cloud)
- Topology toggle (Loopback / Same-region)
- VM size (defaults per cloud, editable)
- "Use existing VM" toggle → select from catalog

Each cell renders as a card showing cloud icon, region, topology. Remove button per card.

### Step 3: Languages

Checkboxes grouped by category:
- **Systems**: Rust, Go, C++
- **Managed**: C# .NET 6/7/8/9/10, Java
- **Scripting**: Node.js, Python, Ruby, PHP
- **Static**: nginx

Shortcuts: "Select All", "Top 5" (Rust, Go, C++, C# .NET 10, Node.js), "Systems Only"

Languages apply to all cells — same comparison set across clouds/regions.

### Step 4: Methodology

Presets:
- **Quick** — 5 warmup, 10 measured, no accuracy target
- **Standard** — 10 warmup, 50 measured, 5% relative error target, 95% confidence
- **Rigorous** — 10 warmup, 100-200 measured, 2% relative error target, 99% confidence

Advanced toggle exposes: warmup count, min/max measured samples, target relative error, target absolute error, confidence level, timeout per probe, concurrency, modes (HTTP/1.1, HTTP/2, HTTP/3, download, upload).

### Step 5: Review & Launch

Summary per cell:
- Cloud / region / topology / VM size
- Languages (count)
- Methodology preset
- Estimated duration (based on runs × languages × modes)
- Estimated cost (VM hours × cloud pricing)

Totals: N cells × M languages × R runs = total probes

Options:
- Auto-teardown VMs after completion (default: on for deploy-on-demand, off for catalog VMs)

"Launch Benchmark" button.

### Progress View (after launch)

Replaces the wizard. Shows:
- Status bar per cell: provisioning → deploying → running (lang X/Y) → completed
- Currently running language highlighted with spinner
- Live log stream from orchestrator (scrollable, auto-follow)
- Results table populates as each language completes
- Box-and-whisker chart updates live as results arrive
- "Cancel" button to abort

---

## 5. Orchestrator Integration

### Invocation

Dashboard spawns `alethabench` as a child process:

```bash
alethabench run \
  --config /tmp/bench-<config_id>.json \
  --callback-url https://alethedash.com/api/benchmarks/callback \
  --callback-token <jwt> \
  --stream-logs
```

### Config JSON

Generated by dashboard from wizard input:

```json
{
  "config_id": "uuid",
  "cells": [
    {
      "cell_id": "uuid",
      "cloud": "azure",
      "region": "eastus",
      "topology": "loopback",
      "vm_size": "Standard_D2s_v3",
      "existing_vm_ip": "40.87.23.80",
      "languages": ["rust", "go", "cpp", "csharp-net10", "python"]
    }
  ],
  "methodology": {
    "warmup_runs": 10,
    "min_measured": 50,
    "max_measured": 200,
    "target_relative_error": 0.05,
    "confidence_level": 0.95,
    "modes": ["http1", "http2"],
    "timeout_secs": 30
  },
  "auto_teardown": true
}
```

### Callback API

Orchestrator reports progress via HTTP POST to dashboard:

**POST /api/benchmarks/callback/status**
```json
{
  "config_id": "uuid",
  "cell_id": "uuid",
  "status": "running",
  "current_language": "go",
  "language_index": 2,
  "language_total": 5,
  "message": "Benchmarking Go (net/http)..."
}
```

**POST /api/benchmarks/callback/log**
```json
{
  "config_id": "uuid",
  "cell_id": "uuid",
  "lines": ["Starting Go server on port 8443...", "Health check passed"]
}
```

**POST /api/benchmarks/callback/result**
```json
{
  "config_id": "uuid",
  "cell_id": "uuid",
  "language": "go",
  "artifact": { ... BenchmarkArtifact JSON ... }
}
```

**POST /api/benchmarks/callback/complete**
```json
{
  "config_id": "uuid",
  "status": "completed",
  "teardown_status": "all_vms_destroyed",
  "duration_seconds": 1847
}
```

Dashboard persists each callback to DB and broadcasts to browsers via WebSocket.

### Orchestrator execution loop

Per cell:
1. Provision VM (if no existing_vm_ip) via az/aws/gcloud CLI
2. Deploy all selected languages via SSH
3. For each language:
   a. Start server on port 8443
   b. Wait for health check
   c. Run `networker-tester --benchmark` with full methodology config
   d. Collect BenchmarkArtifact JSON
   e. POST result to callback URL
   f. Stop server
4. If auto_teardown and VM was provisioned, destroy VM
5. POST complete

---

## 6. VM Catalog

### Registration

Users register VMs manually or via auto-discovery:
- **Manual**: provide IP, SSH user, cloud, region. Dashboard SSHs and detects deployed languages.
- **Auto-discovery**: scan cloud provider for VMs tagged with `alethabench` or matching naming patterns.
- **Post-benchmark**: VMs created by deploy-on-demand are auto-registered in the catalog if not torn down.

### Language detection

Dashboard or orchestrator SSHes to VM and checks:
- `/opt/bench/rust-server` exists → Rust
- `/opt/bench/go-server` exists → Go
- `/opt/bench/cpp-build/server` exists → C++
- `/opt/bench/csharp-net*/csharp-net*` exists → C# .NET versions
- `node --version` + `/opt/bench/nodejs/server.js` → Node.js
- `python3 --version` + `/opt/bench/python/server.py` → Python
- `/opt/bench/ruby/config.ru` → Ruby
- `php --version` + `/opt/bench/php/server.php` → PHP
- `nginx -v` → nginx

### Health checks

Periodic (every 5 min) SSH connectivity check. Mark offline VMs in catalog.

---

## 7. Scheduling & Notifications

### Scheduled benchmarks

Re-use the existing schedule infrastructure. A benchmark config can be linked to a schedule:
- "Run this benchmark every Sunday at 00:00 UTC"
- "Run after every deploy to production"

### Regression detection

Compare latest benchmark results against a stored baseline:
- Flag if any language's p50 regresses by more than 10%
- Flag if success rate drops below 99%
- Configurable thresholds per benchmark config

### Notifications

- Email via ACS (existing integration)
- Dashboard toast + live feed entry
- Optional webhook URL for CI/CD integration

---

## 8. Parallel Implementation Streams

### Wave 1 (start immediately, no dependencies)

| Stream | Scope | Agent |
|--------|-------|-------|
| S0: Orchestrator foundations | Heartbeat loop, --callback-url flag, graceful shutdown (SIGTERM), cancel polling, PID file | 1 |
| S1: DB + API | Migration V016, benchmark_config CRUD, benchmark_cell CRUD, benchmark_vm_catalog CRUD, callback endpoints | 1 |
| S3: Wizard UI | All 5 wizard steps, template presets, form validation, review summary | 1 |
| S4: Catalog + VM registry | Catalog management page, manual VM registration, SSH language detection, periodic health checks | 1 |

### Wave 2 (start once S0 or S1 lands)

| Stream | Scope | Depends on | Agent |
|--------|-------|-----------|-------|
| S2: Orchestrator execution | Cell loop, language sweep, tester invocation, callback reporting, heartbeat emission | S0 | 1 |
| S5: Deploy-on-demand | Provision VMs via az/aws/gcloud, tier-to-size mapping, region validation, auto-teardown | S1 | 1 |
| S6: Progress + live logs | WebSocket streaming from callbacks, progress bar per cell, live log viewer, cancel button | S1, S0 | 1 |
| S7: Results pipeline | Callback result → pipeline tables, cross-cell comparison view, box-and-whisker per cell | S1 | 1 |

### Wave 3 (after core loop works end-to-end)

| Stream | Scope | Depends on | Agent |
|--------|-------|-----------|-------|
| S8: Scheduling + notifications | Cron triggers, regression detection, baseline management, email/webhook alerts | S1, S2 | 1 |

### Recommended risk mitigation

- **S5 (deploy-on-demand)** is highest risk — prototype Azure provisioning early, even before S1 is complete, using the existing orchestrator deployer module.
- **S0** is foundational — without heartbeat and graceful shutdown, the worker loop (S6) can't detect stalled runs.
- **S3 (wizard)** can use mock API responses until S1 is ready — build the full UI flow independently.

---

## 9. Reliability & Operational Concerns

### Long-running process management

Benchmarks can run for hours (multi-cell, many languages, rigorous methodology). The orchestrator MUST NOT run on the API worker thread.

**Approach:** PostgreSQL-based job queue. Dashboard writes config to DB with status `queued`. A dedicated benchmark worker loop (separate tokio task in the dashboard binary, not on the API worker) polls for queued configs every 5 seconds and spawns the orchestrator. This avoids zombie processes, works with dashboard restarts (worker re-scans on startup), and doesn't require external infrastructure (Redis, Sidekiq).

The worker:
1. Claims a queued config by setting status `running` + worker_id (prevents double-pickup if multiple dashboard instances exist)
2. Spawns `alethabench` via `tokio::process::Command`
3. Monitors the child process (exit code, stdout/stderr)
4. On dashboard restart, scans for `running` configs and checks if the orchestrator process is still alive (via PID file or heartbeat)

**Heartbeat:** Orchestrator sends a heartbeat callback every 60 seconds. Dashboard marks a benchmark as `stalled` if no heartbeat for 10 minutes. Stalled benchmarks surface in the UI with a "Retry" button.

**Cancellation:** Dashboard sends `POST /api/benchmarks/callback/cancel` which the orchestrator polls every heartbeat cycle. On cancel, orchestrator gracefully stops current language, tears down provisioned VMs, and exits. Dashboard also sends SIGTERM to the child process as a fallback.

**Cleanup cron:** Dashboard background task runs every 15 minutes, checking for orphaned processes (PID in DB but process not running) and marking them as `failed`.

**Retry:** On partial failure (e.g., one cell fails provisioning), other cells continue. Failed cells are marked individually. User can retry failed cells without re-running successful ones.

**Future scaling:** If horizontal scaling is needed, extract the worker into a separate `alethabench-worker` binary that connects to the same DB. No architectural changes required — just move the polling loop.

### Credential management

Cloud CLIs require authentication. Credentials are managed per-project via the existing cloud accounts system (Settings → Cloud tab).

- **Azure:** Service principal or `az login` session on the dashboard VM
- **AWS:** IAM keys stored in cloud_account table (encrypted at rest)
- **GCP:** Service account JSON stored in cloud_account table

The orchestrator inherits the dashboard's environment. For per-project isolation, the dashboard sets `AWS_ACCESS_KEY_ID`, `AZURE_*`, `GOOGLE_APPLICATION_CREDENTIALS` environment variables when spawning the orchestrator.

### VM size normalization

Users pick a logical size tier, not cloud-specific names:

| Tier | Azure | AWS | GCP | Specs |
|------|-------|-----|-----|-------|
| Small | Standard_B2s | t3.small | e2-small | 2 vCPU, 4 GiB |
| Medium | Standard_D2s_v3 | t3.medium | n2-standard-2 | 2 vCPU, 8 GiB |
| Large | Standard_D4s_v3 | m5.xlarge | n2-standard-4 | 4 vCPU, 16 GiB |

Wizard shows tier + specs. Orchestrator maps to cloud-specific names.

### Region validation

Wizard fetches available regions per cloud on cell creation. Orchestrator validates VM size availability in the selected region before provisioning. User-friendly error if unavailable.

### Cost control

- Auto-teardown is default ON for provisioned VMs
- Maximum benchmark duration: configurable per config (default 4 hours)
- Orchestrator self-terminates after max duration
- Dashboard shows estimated cost in review step (VM hours × approximate pricing)

### Partial failure handling

| Failure | Behavior |
|---------|----------|
| One cell fails provisioning | Other cells continue. Failed cell marked `failed`. |
| One language crashes | Skip to next language. Mark as failed in results. |
| Orchestrator crashes | Dashboard detects via missing heartbeat. Status → `stalled`. User can retry. |
| Dashboard restarts | Orchestrator keeps running. Dashboard reconnects via heartbeat on startup. |
| SSH connection lost | Orchestrator retries 3 times with backoff. Then marks cell as failed. |

### Security

- Callback JWT is scoped to the specific `config_id` and expires after `max_duration + 1 hour`
- SSH keys stored per-project in cloud_account table, encrypted with AES-GCM
- Orchestrator runs with minimal permissions (no DB access, only HTTP callback)
- VM provisioning uses project-scoped credentials, not dashboard-admin credentials

---

## 10. Edge Cases & Extensibility

### Manual VMs

Catalog entries with `cloud: manual` skip provisioning entirely. Orchestrator SSHes directly to the provided IP. No teardown.

### Language parallelization (future)

Current design runs languages sequentially on port 8443. Future optimization: assign different ports per language (8443, 8444, 8445...) and run 2-3 languages concurrently on the same VM if CPU/memory allows. Or use container isolation (Docker) per language.

### nginx as baseline

In the wizard, nginx is always selected by default as a static-file baseline. Users can deselect it but get a warning that comparison loses a reference point.

### Regression baselines

Each benchmark config stores a `baseline_run_id` pointing to the last successful run. Regression detection compares against this baseline. Users can manually pin a baseline ("use this run as the reference").

### Multi-tenancy

All queries are scoped by `project_id`. One workspace's benchmarks cannot access another's VMs, results, or credentials. The orchestrator receives only the credentials for the owning project.

---

## 11. Success Criteria

- User can create a benchmark via wizard, selecting Azure + AWS, 5 languages, Standard methodology
- Orchestrator provisions VMs, deploys languages, runs benchmarks, reports results via callbacks
- Dashboard shows live progress with log streaming
- Results appear in Benchmarks page with box-and-whisker charts and cross-cell comparison
- Existing catalog VMs work without provisioning
- Auto-teardown destroys provisioned VMs after completion
- Scheduled benchmarks run on cron and detect regressions
