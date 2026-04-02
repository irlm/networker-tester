# Benchmark Wizard Redesign — Testbed + OS Selection

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename "Cell" to "Testbed" across the full stack (DB → Rust → API → frontend), add OS selection per testbed with smart .NET 4.8 auto-detection, and add three new OS-aware templates.

**Architecture:** V020 migration renames the `benchmark_cell` table/columns to `benchmark_testbed`/`testbed_id`. Rust structs, API payloads, and frontend types all rename in lockstep. The `BenchmarkTestbedConfig` gains an `os` field (`"linux"` | `"windows"`). Frontend auto-infers OS from language selection (`.NET 4.8` forces Windows) and shows warnings. Three new templates: Linux Focus, Windows (.NET), Cross-OS.

**Tech Stack:** PostgreSQL (migration), Rust/axum (backend), React/TypeScript (frontend)

---

## File Structure

### Database
- **Modify:** `crates/networker-dashboard/src/db/migrations.rs` — Add V020 migration (ALTER TABLE RENAME + ADD COLUMN os)
- **Rename:** `crates/networker-dashboard/src/db/benchmark_cells.rs` → `benchmark_testbeds.rs` — Rename all structs/functions/SQL
- **Modify:** `crates/networker-dashboard/src/db/mod.rs` — Update module name
- **Modify:** `crates/networker-dashboard/src/db/benchmarks.rs` — Update cell_id → testbed_id references in queries

### Backend API
- **Modify:** `crates/networker-dashboard/src/api/benchmark_configs.rs` — Rename CellInput → TestbedInput, update all references
- **Modify:** `crates/networker-dashboard/src/api/benchmark_callbacks.rs` — Rename cell_id fields → testbed_id
- **Modify:** `crates/networker-dashboard/src/benchmark_worker.rs` — Update cell references
- **Modify:** `crates/networker-dashboard/src/scheduler.rs` — Update cell references

### Orchestrator
- **Modify:** `benchmarks/orchestrator/src/config.rs` — CellConfig → TestbedConfig, cell_id → testbed_id, add `os` field
- **Modify:** `benchmarks/orchestrator/src/executor.rs` — Rename all cell → testbed references
- **Modify:** `benchmarks/orchestrator/src/callback.rs` — Rename cell_id → testbed_id in payloads

### Frontend
- **Modify:** `dashboard/src/api/types.ts` — BenchmarkCellConfig → BenchmarkTestbedConfig, add `os` field, rename all cell types
- **Modify:** `dashboard/src/api/client.ts` — Update payload type reference
- **Modify:** `dashboard/src/stores/liveStore.ts` — BenchmarkCellStatus → BenchmarkTestbedStatus
- **Modify:** `dashboard/src/pages/BenchmarkWizardPage.tsx` — Full rename + OS dropdown + auto-detect + 3 new templates
- **Modify:** `dashboard/src/pages/BenchmarkProgressPage.tsx` — Rename cell → testbed in UI
- **Modify:** `dashboard/src/pages/BenchmarkConfigResultsPage.tsx` — Rename cell → testbed in UI + labels

---

## Task 1: V020 Database Migration — Rename cell → testbed + Add OS Column

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs`

- [ ] **Step 1: Add V020 migration constant**

After the V019 block (~line 867), add:

```rust
/// V020 migration: Rename cell → testbed, add OS column.
const V020_RENAME_CELL_TO_TESTBED: &str = r#"
-- Rename table
ALTER TABLE IF EXISTS benchmark_cell RENAME TO benchmark_testbed;

-- Rename PK column
ALTER TABLE benchmark_testbed RENAME COLUMN cell_id TO testbed_id;

-- Add OS column (default linux for existing rows)
ALTER TABLE benchmark_testbed ADD COLUMN IF NOT EXISTS os TEXT NOT NULL DEFAULT 'linux';

-- Rename indexes
DROP INDEX IF EXISTS ix_benchmark_cell_config;
CREATE INDEX IF NOT EXISTS ix_benchmark_testbed_config ON benchmark_testbed (config_id);

-- Rename cell_id in benchmark_run
ALTER TABLE benchmark_run RENAME COLUMN cell_id TO testbed_id;
DROP INDEX IF EXISTS ix_benchmark_run_cell;
CREATE INDEX IF NOT EXISTS ix_benchmark_run_testbed ON benchmark_run (testbed_id);
"#;
```

- [ ] **Step 2: Add V020 application block**

After the V019 application block, add:

```rust
    // V020: Rename cell → testbed, add OS column
    let v020_applied = client
        .query_one("SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmark_testbed')", &[])
        .await?
        .get::<_, bool>(0);
    if !v020_applied {
        tracing::info!("Applying V020 rename_cell_to_testbed migration...");
        client.batch_execute(V020_RENAME_CELL_TO_TESTBED).await?;
        client
            .execute(
                "INSERT INTO schema_version (version, description) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                &[&20i32, &"rename cell to testbed + os column"],
            )
            .await?;
        tracing::info!("V020 migration complete");
    }
```

- [ ] **Step 3: Build to verify migration compiles**

Run: `cargo build -p networker-dashboard 2>&1 | head -30`
Expected: Compiles (migration is just a string constant, no type deps yet)

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/db/migrations.rs
git commit -m "feat: V020 migration — rename benchmark_cell to benchmark_testbed + os column"
```

---

## Task 2: Rename Rust DB Module — benchmark_cells → benchmark_testbeds

**Files:**
- Rename: `crates/networker-dashboard/src/db/benchmark_cells.rs` → `crates/networker-dashboard/src/db/benchmark_testbeds.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`

- [ ] **Step 1: Rename the file**

```bash
mv crates/networker-dashboard/src/db/benchmark_cells.rs crates/networker-dashboard/src/db/benchmark_testbeds.rs
```

- [ ] **Step 2: Update mod.rs**

Change `pub mod benchmark_cells;` → `pub mod benchmark_testbeds;`

- [ ] **Step 3: Rename all types and SQL in benchmark_testbeds.rs**

Replace the full file contents:

```rust
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

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
}

fn row_to_testbed(r: &tokio_postgres::Row) -> BenchmarkTestbedRow {
    BenchmarkTestbedRow {
        testbed_id: r.get("testbed_id"),
        config_id: r.get("config_id"),
        cloud: r.get("cloud"),
        region: r.get("region"),
        topology: r.get("topology"),
        endpoint_vm_id: r.get("endpoint_vm_id"),
        tester_vm_id: r.get("tester_vm_id"),
        endpoint_ip: r.get("endpoint_ip"),
        tester_ip: r.get("tester_ip"),
        status: r.get("status"),
        languages: r.get("languages"),
        vm_size: r.get("vm_size"),
        os: r.get("os"),
    }
}

pub async fn create(
    client: &Client,
    config_id: &Uuid,
    cloud: &str,
    region: &str,
    topology: &str,
    languages: &serde_json::Value,
    vm_size: Option<&str>,
    os: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO benchmark_testbed
                (testbed_id, config_id, cloud, region, topology, languages, vm_size, os)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                &id, config_id, &cloud, &region, &topology, languages, &vm_size, &os,
            ],
        )
        .await?;
    Ok(id)
}

pub async fn list_for_config(
    client: &Client,
    config_id: &Uuid,
) -> anyhow::Result<Vec<BenchmarkTestbedRow>> {
    let rows = client
        .query(
            "SELECT testbed_id, config_id, cloud, region, topology, endpoint_vm_id,
                    tester_vm_id, endpoint_ip, tester_ip, status, languages, vm_size, os
             FROM benchmark_testbed WHERE config_id = $1
             ORDER BY cloud, region",
            &[config_id],
        )
        .await?;
    Ok(rows.iter().map(row_to_testbed).collect())
}

pub async fn update_status(client: &Client, testbed_id: &Uuid, status: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE benchmark_testbed SET status = $1 WHERE testbed_id = $2",
            &[&status, testbed_id],
        )
        .await?;
    Ok(())
}

pub async fn update_vm_info(
    client: &Client,
    testbed_id: &Uuid,
    endpoint_vm_id: Option<&str>,
    tester_vm_id: Option<&str>,
    endpoint_ip: Option<&str>,
    tester_ip: Option<&str>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE benchmark_testbed SET
                endpoint_vm_id = COALESCE($1, endpoint_vm_id),
                tester_vm_id = COALESCE($2, tester_vm_id),
                endpoint_ip = COALESCE($3, endpoint_ip),
                tester_ip = COALESCE($4, tester_ip)
             WHERE testbed_id = $5",
            &[
                &endpoint_vm_id,
                &tester_vm_id,
                &endpoint_ip,
                &tester_ip,
                testbed_id,
            ],
        )
        .await?;
    Ok(())
}
```

- [ ] **Step 4: Verify it compiles (expect errors from callers — that's fine)**

Run: `cargo build -p networker-dashboard 2>&1 | grep "error\[" | head -20`
Expected: Errors in files that still reference `benchmark_cells` — we fix those next.

- [ ] **Step 5: Commit**

```bash
git add -A crates/networker-dashboard/src/db/
git commit -m "refactor: rename benchmark_cells module to benchmark_testbeds"
```

---

## Task 3: Update Dashboard Backend Callers — cell → testbed

**Files:**
- Modify: `crates/networker-dashboard/src/db/benchmarks.rs` — cell_id → testbed_id in SQL and structs
- Modify: `crates/networker-dashboard/src/api/benchmark_configs.rs` — CellInput → TestbedInput, cell references
- Modify: `crates/networker-dashboard/src/api/benchmark_callbacks.rs` — cell_id → testbed_id
- Modify: `crates/networker-dashboard/src/benchmark_worker.rs` — cell references
- Modify: `crates/networker-dashboard/src/scheduler.rs` — cell references

- [ ] **Step 1: Fix benchmarks.rs**

Find-and-replace throughout the file:
- `benchmark_cells` → `benchmark_testbeds` (module path)
- `cell_id` → `testbed_id` (in SQL strings AND Rust field names/params)
- `BenchmarkCellRow` → `BenchmarkTestbedRow` (if referenced)
- `cross-cell` → `cross-testbed` (comments)
- `cell_filter` → `testbed_filter` (variable names)
- In the `ConfigCellResult`-like struct (line ~721): rename `cell_id` field to `testbed_id`

SQL queries that reference column names must change:
- `br.cell_id` → `br.testbed_id`
- `bc.cell_id` → `bc.testbed_id`
- `benchmark_cell` → `benchmark_testbed` (in FROM/JOIN)

- [ ] **Step 2: Fix benchmark_configs.rs**

Replace throughout:
- `CellInput` → `TestbedInput` (struct name + all usages)
- `cell_ids` → `testbed_ids` (Vec field + all usages)
- `cells` → `testbeds` (payload field names in JSON and struct fields)
- `BenchmarkConfigWithCells` → `BenchmarkConfigWithTestbeds`
- `benchmark_cells` → `benchmark_testbeds` (module path in `crate::db::`)
- `BenchmarkCellRow` → `BenchmarkTestbedRow`
- `cell_count` → `testbed_count` (tracing)
- `"Failed to create benchmark cell"` → `"Failed to create benchmark testbed"`
- `"Failed to list benchmark cells"` → `"Failed to list benchmark testbeds"`
- Add `os` field to `TestbedInput` struct: `pub os: Option<String>` (default `"linux"`)
- Pass `testbed.os.as_deref().unwrap_or("linux")` to the `create()` call

- [ ] **Step 3: Fix benchmark_callbacks.rs**

Replace throughout:
- `cell_id` → `testbed_id` (struct fields, JSON keys, function params)
- `benchmark_cells` → `benchmark_testbeds` (module path)
- `"Failed to update cell status"` → `"Failed to update testbed status"`
- `"cell_id"` → `"testbed_id"` (JSON string keys in serde_json::json! macros)

- [ ] **Step 4: Fix benchmark_worker.rs**

Replace throughout:
- `benchmark_cells` → `benchmark_testbeds` (module path)
- `db_cells` → `db_testbeds` (variable names)
- `"cells"` → `"testbeds"` (JSON key strings)
- `merged_cells` → `merged_testbeds`
- `config_cells` → `config_testbeds`
- `cell` → `testbed` (local variable names in the loop)
- `"cell_id"` → `"testbed_id"` (JSON key in obj.insert)

- [ ] **Step 5: Fix scheduler.rs**

Replace:
- `benchmark_cells` → `benchmark_testbeds` (module path)
- `cells` → `testbeds` (variable name)
- `cell` → `testbed` (loop variable)
- Add `testbed.os.as_str()` as the new `os` parameter in the `create()` call

- [ ] **Step 6: Build and fix any remaining issues**

Run: `cargo build -p networker-dashboard 2>&1 | head -50`
Expected: Clean build (or minor fixes to apply)

- [ ] **Step 7: Commit**

```bash
git add crates/networker-dashboard/src/
git commit -m "refactor: rename cell → testbed in dashboard backend"
```

---

## Task 4: Update Orchestrator — cell → testbed

**Files:**
- Modify: `benchmarks/orchestrator/src/config.rs`
- Modify: `benchmarks/orchestrator/src/executor.rs`
- Modify: `benchmarks/orchestrator/src/callback.rs`

- [ ] **Step 1: Fix config.rs**

Replace throughout:
- `CellConfig` → `TestbedConfig` (struct name + all usages)
- `cell_id` → `testbed_id` (field + all usages)
- `cells` → `testbeds` (field on BenchmarkConfig + all usages)
- `"cells list must not be empty"` → `"testbeds list must not be empty"`
- `"cell {} has no languages"` → `"testbed {} has no languages"`
- Add `pub os: String` field to `TestbedConfig` with `#[serde(default = "default_os")]` and `fn default_os() -> String { "linux".to_string() }`
- Update test data: `cell_id: "cell-uuid-5678"` → `testbed_id: "testbed-uuid-5678"`, add `os: "linux".into()`

- [ ] **Step 2: Fix executor.rs**

This file has 65+ references. Do a systematic find-replace:
- `CellOutcome` → `TestbedOutcome`
- `cell_id` → `testbed_id` (field names)
- `cell_index` → `testbed_index`
- `total_cells` → `total_testbeds`
- `execute_cell` → `execute_testbed`
- `teardown_cell` → `teardown_testbed`
- `cell` → `testbed` (function params, loop vars — but be careful not to replace inside string literals that don't need it)
- `config.cells` → `config.testbeds`
- In log messages: `"Executing cell"` → `"Executing testbed"`, `"Cell {} failed"` → `"Testbed {} failed"`, etc.
- `report_cell_status` → `report_testbed_status`

- [ ] **Step 3: Fix callback.rs**

Replace throughout:
- `cell_id` → `testbed_id` (struct fields + function params + JSON values)
- In function signatures: `cell_id: &str` → `testbed_id: &str`
- In struct construction: `cell_id: cell_id.to_string()` → `testbed_id: testbed_id.to_string()`

- [ ] **Step 4: Build the orchestrator**

Run: `cargo build -p benchmark-orchestrator 2>&1 | head -30`
Expected: Clean build. If the package name differs, check `benchmarks/orchestrator/Cargo.toml` for the correct name.

- [ ] **Step 5: Commit**

```bash
git add benchmarks/orchestrator/src/
git commit -m "refactor: rename cell → testbed in orchestrator"
```

---

## Task 5: Full Workspace Build Check

- [ ] **Step 1: Build entire workspace**

Run: `cargo build --workspace 2>&1 | tail -20`
Expected: Clean build with zero errors.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: No warnings.

- [ ] **Step 3: Run unit tests**

Run: `cargo test --workspace --lib 2>&1 | tail -20`
Expected: All tests pass.

- [ ] **Step 4: Commit any fixups**

If steps 1-3 required fixes, commit them:
```bash
git add -A
git commit -m "fix: resolve testbed rename build issues"
```

---

## Task 6: Frontend Types + API Client — cell → testbed + OS field

**Files:**
- Modify: `dashboard/src/api/types.ts`
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/stores/liveStore.ts`

- [ ] **Step 1: Update types.ts**

Rename all cell types and add `os` field:

- `BenchmarkCellConfig` → `BenchmarkTestbedConfig`, add `os: string;` field
- `BenchmarkCellRow` → `BenchmarkTestbedRow`, rename `cell_id` → `testbed_id`, add `os: string;`
- `ConfigCellResult` → rename `cell_id` → `testbed_id`
- `cell_count` → `testbed_count` (in BenchmarkConfigRow if present)
- In `BenchmarkConfigWithCells` (or similar): `cells` → `testbeds`, type → `BenchmarkTestbedRow[]`
- Comment: `cross-cell` → `cross-testbed`

- [ ] **Step 2: Update client.ts**

In `createBenchmarkConfig` payload type:
- `cells: import('./types').BenchmarkCellConfig[]` → `testbeds: import('./types').BenchmarkTestbedConfig[]`

- [ ] **Step 3: Update liveStore.ts**

- `BenchmarkCellStatus` → `BenchmarkTestbedStatus`, rename `cell_id` → `testbed_id`
- `cells: Record<string, BenchmarkCellStatus>` → `testbeds: Record<string, BenchmarkTestbedStatus>`
- In the WS event handler: `event.payload.cell_id` → `event.payload.testbed_id`, `updated.cells` → `updated.testbeds`
- In `clearBenchmarkLive`: `cells: {}` → `testbeds: {}`

- [ ] **Step 4: Build frontend to check for type errors**

Run: `cd dashboard && npx tsc --noEmit 2>&1 | head -30`
Expected: Type errors in the page components (BenchmarkWizardPage, etc.) — we fix those in the next tasks.

- [ ] **Step 5: Commit**

```bash
git add dashboard/src/api/ dashboard/src/stores/
git commit -m "refactor: rename cell → testbed in frontend types and stores"
```

---

## Task 7: Wizard Page — Rename + OS Dropdown + Auto-detect + New Templates

**Files:**
- Modify: `dashboard/src/pages/BenchmarkWizardPage.tsx`

This is the largest single-file change. The steps below are grouped logically.

- [ ] **Step 1: Rename all cell references to testbed**

Systematic replacements:
- `STEP_LABELS`: `'Cells'` → `'Testbeds'`
- `CellState` → `TestbedState`
- `makeCell` → `makeTestbed`
- `cellKey` / `setCellKey` → `testbedKey` / `setTestbedKey`
- `cells` / `setCells` → `testbeds` / `setTestbeds`
- `addCell` → `addTestbed`
- `removeCell` → `removeTestbed`
- `updateCell` → `updateTestbed`
- `cellConfigs` → `testbedConfigs`
- `BenchmarkCellConfig` → `BenchmarkTestbedConfig`
- `catalog` / `catalogLoaded` — keep as-is (VM catalog is not "cell")
- `totalVMs`, `totalExisting`, `totalCombinations` — update to reference `testbeds`
- All UI text: "Cell" → "Testbed", "cells" → "testbeds", "No cells yet" → "No testbeds yet", etc.
- Template descriptions: update "cell" → "testbed" in all description strings

- [ ] **Step 2: Add OS field to TestbedState and makeTestbed**

```typescript
interface TestbedState {
  key: number;
  cloud: string;
  region: string;
  topology: string;
  vmSize: string;
  os: 'linux' | 'windows';
  useExisting: boolean;
  existingVmId: string;
}

function makeTestbed(key: number, cloud?: string, os?: 'linux' | 'windows'): TestbedState {
  const c = cloud ?? 'Azure';
  return {
    key,
    cloud: c,
    region: REGIONS[c]?.[0] ?? '',
    topology: 'Loopback',
    vmSize: 'Medium',
    os: os ?? 'linux',
    useExisting: false,
    existingVmId: '',
  };
}
```

- [ ] **Step 3: Add OS compatibility constants**

After the existing `LANGUAGE_GROUPS` constant, add:

```typescript
// OS compatibility: which languages require or exclude specific OS
const WINDOWS_ONLY_LANGS = new Set(['csharp-net48']);
const LINUX_ONLY_LANGS = new Set<string>(); // none currently, but ready for future

function requiresWindows(langs: Set<string>): boolean {
  return [...langs].some(id => WINDOWS_ONLY_LANGS.has(id));
}

function hasWindowsOnlyLangs(langs: Set<string>): string[] {
  return [...langs].filter(id => WINDOWS_ONLY_LANGS.has(id));
}
```

- [ ] **Step 4: Add OS dropdown to the testbed config grid**

In the Step 1 (Configure Testbeds) section, after the VM Size dropdown and before the "Use existing VM" toggle, add an OS dropdown. Change the grid from `xl:grid-cols-4` to `xl:grid-cols-5`:

```tsx
{/* OS */}
<label className="text-xs text-gray-500">
  OS
  <select
    value={testbed.os}
    onChange={e => updateTestbed(testbed.key, { os: e.target.value as 'linux' | 'windows' })}
    className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
  >
    <option value="linux">Linux (Ubuntu)</option>
    <option value="windows">Windows Server</option>
  </select>
</label>
```

- [ ] **Step 5: Add .NET 4.8 auto-detection on step navigation**

In the `goNext` function, before advancing from step 2 (Languages) to step 3, add OS auto-detection logic:

```typescript
const goNext = () => {
  if (step === 0 && selectedTemplate === null) return;
  if (step < STEP_LABELS.length - 1) {
    const next = step + 1;
    if (next === 1) loadCatalog();

    // Auto-detect OS: if .NET 4.8 selected and only 1 testbed with Linux, auto-switch
    if (step === 2 && requiresWindows(selectedLangs)) {
      setTestbeds(prev => prev.map(tb => {
        if (tb.os === 'linux' && prev.length === 1) {
          return { ...tb, os: 'windows' };
        }
        return tb;
      }));
    }

    setStep(next);
  }
};
```

- [ ] **Step 6: Add OS/language compatibility warning in the Languages step**

At the bottom of the Languages step (step === 2), after the "X languages selected" text, add:

```tsx
{requiresWindows(selectedLangs) && testbeds.length === 1 && testbeds[0].os === 'linux' && (
  <div className="mt-3 border border-yellow-500/30 bg-yellow-500/5 rounded-lg p-3">
    <p className="text-xs text-yellow-300">
      C# .NET 4.8 requires Windows Server. When you proceed, your testbed will be
      switched to Windows automatically.
    </p>
  </div>
)}

{requiresWindows(selectedLangs) && testbeds.length > 1 && testbeds.some(tb => tb.os === 'linux') && (
  <div className="mt-3 border border-yellow-500/30 bg-yellow-500/5 rounded-lg p-3">
    <p className="text-xs text-yellow-300">
      C# .NET 4.8 requires Windows Server. It will only run on testbeds configured with Windows OS.
      Linux testbeds will skip .NET 4.8 automatically.
    </p>
  </div>
)}
```

- [ ] **Step 7: Add three new templates**

Replace the `TEMPLATES` array with 7 templates (4 existing + 3 new). Insert the new ones before "Custom":

```typescript
{
  id: 'linux-focus',
  name: 'Linux Focus',
  description: 'Single Linux testbed with top-performing languages. Best for quick language comparison on Ubuntu.',
  defaultTestbedCount: 1,
  defaultOs: 'linux' as const,
  defaultLanguages: ['nginx', 'rust', 'go', 'csharp-net8', 'java', 'nodejs'],
  methodology: 'standard',
},
{
  id: 'windows-dotnet',
  name: 'Windows .NET',
  description: 'Single Windows testbed focused on the C# .NET ecosystem — Framework 4.8 through .NET 10 AOT.',
  defaultTestbedCount: 1,
  defaultOs: 'windows' as const,
  defaultLanguages: ['nginx', 'csharp-net48', 'csharp-net8', 'csharp-net8-aot', 'csharp-net9', 'csharp-net9-aot', 'csharp-net10', 'csharp-net10-aot'],
  methodology: 'standard',
},
{
  id: 'cross-os',
  name: 'Cross-OS',
  description: 'Two testbeds — Linux and Windows — same cloud and region. Compares language performance across operating systems.',
  defaultTestbedCount: 2,
  defaultOs: null,
  defaultLanguages: ['nginx', 'rust', 'go', 'csharp-net8', 'csharp-net8-aot', 'java', 'nodejs'],
  methodology: 'standard',
},
```

Update the `TemplateOption` interface to include OS:

```typescript
interface TemplateOption {
  id: string;
  name: string;
  description: string;
  defaultTestbedCount: number;
  defaultOs: 'linux' | 'windows' | null;
  defaultLanguages: string[];
  methodology: string;
}
```

Add `defaultOs` to existing templates: `'linux'` for quick-check, regional-comparison, cross-cloud. `null` for custom.

- [ ] **Step 8: Update applyTemplate to handle OS-aware templates**

```typescript
const applyTemplate = (tmpl: TemplateOption) => {
  setSelectedTemplate(tmpl.id);

  const newTestbeds: TestbedState[] = [];
  if (tmpl.id === 'quick-check' || tmpl.id === 'linux-focus' || tmpl.id === 'windows-dotnet') {
    const k = testbedKey;
    setTestbedKey(k + 1);
    newTestbeds.push(makeTestbed(k, 'Azure', tmpl.defaultOs ?? 'linux'));
  } else if (tmpl.id === 'regional-comparison') {
    let k = testbedKey;
    newTestbeds.push(makeTestbed(k++, 'Azure', tmpl.defaultOs ?? 'linux'));
    const tb2 = makeTestbed(k++, 'Azure', tmpl.defaultOs ?? 'linux');
    tb2.region = REGIONS.Azure[1] ?? '';
    newTestbeds.push(tb2);
    setTestbedKey(k);
  } else if (tmpl.id === 'cross-cloud') {
    let k = testbedKey;
    newTestbeds.push(makeTestbed(k++, 'Azure', tmpl.defaultOs ?? 'linux'));
    newTestbeds.push(makeTestbed(k++, 'AWS', tmpl.defaultOs ?? 'linux'));
    newTestbeds.push(makeTestbed(k++, 'GCP', tmpl.defaultOs ?? 'linux'));
    setTestbedKey(k);
  } else if (tmpl.id === 'cross-os') {
    let k = testbedKey;
    newTestbeds.push(makeTestbed(k++, 'Azure', 'linux'));
    newTestbeds.push(makeTestbed(k++, 'Azure', 'windows'));
    setTestbedKey(k);
  }
  // custom: no testbeds
  setTestbeds(newTestbeds);
  setSelectedLangs(new Set(tmpl.defaultLanguages));

  const preset = METHODOLOGY_PRESETS.find(p => p.id === tmpl.methodology) ?? METHODOLOGY_PRESETS[1];
  setMethodPreset(preset.id);
  setWarmup(preset.warmup);
  setMeasured(preset.measured);
  setTargetError(preset.targetError);

  loadCatalog();
  setStep(1);
};
```

- [ ] **Step 9: Update buildPayload to include os**

```typescript
const buildPayload = () => {
  const testbedConfigs: BenchmarkTestbedConfig[] = testbeds.map(tb => ({
    cloud: tb.cloud,
    region: tb.region,
    topology: tb.topology,
    vm_size: tb.vmSize,
    os: tb.os,
    existing_vm_ip: tb.useExisting ? (catalog.find(v => v.vm_id === tb.existingVmId)?.ip ?? null) : null,
    languages: Array.from(selectedLangs),
  }));

  return {
    name: benchmarkName.trim() || `Benchmark ${new Date().toISOString().slice(0, 16)}`,
    template: selectedTemplate,
    testbeds: testbedConfigs,
    languages: Array.from(selectedLangs),
    methodology: { ... },
    auto_teardown: autoTeardown,
  };
};
```

- [ ] **Step 10: Update Review step to show OS**

In the testbed summary cards (step 4), add OS badge:

```tsx
<div className="flex items-center gap-3">
  <span className="text-xs font-medium text-gray-400">Testbed {idx + 1}</span>
  <span className="text-xs text-gray-300 font-mono">
    {testbed.cloud} / {testbed.region}
  </span>
  <span className="text-xs text-gray-500">{testbed.topology}</span>
  <span className="text-xs text-gray-500">{testbed.vmSize}</span>
  <span className={`text-[10px] px-1.5 py-0.5 rounded border ${
    testbed.os === 'windows'
      ? 'border-blue-500/30 text-blue-300'
      : 'border-green-500/30 text-green-300'
  }`}>
    {testbed.os === 'windows' ? 'Windows' : 'Linux'}
  </span>
  {testbed.useExisting && <span className="text-[10px] text-yellow-500/80">existing VM</span>}
</div>
```

Update totals: `"Cells"` → `"Testbeds"`.

- [ ] **Step 11: Update template info display**

In the template cards, update the testbed count text:

```tsx
{tmpl.defaultTestbedCount > 0 && <span>{tmpl.defaultTestbedCount} testbed{tmpl.defaultTestbedCount > 1 ? 's' : ''}</span>}
```

- [ ] **Step 12: Build frontend**

Run: `cd dashboard && npx tsc --noEmit 2>&1 | head -30`
Expected: Errors only from Progress/Results pages (fixed in next tasks).

- [ ] **Step 13: Commit**

```bash
git add dashboard/src/pages/BenchmarkWizardPage.tsx
git commit -m "feat: wizard redesign — testbed rename, OS dropdown, auto-detect, 3 new templates"
```

---

## Task 8: Progress Page — Rename cell → testbed

**Files:**
- Modify: `dashboard/src/pages/BenchmarkProgressPage.tsx`

- [ ] **Step 1: Rename all cell references**

Systematic replacements:
- `CellDetail` → `TestbedDetail`, `cell_id` → `testbed_id`
- `cells` / `setCells` state → `testbeds` / `setTestbeds`
- `rawCells` → `rawTestbeds`
- `live.cells` → `live.testbeds`
- `cellLive` → `testbedLive`
- `cellCount` → `testbedCount`
- UI text: `"cell status"` → `"testbed status"`, `"cell"/"cells"` → `"testbed"/"testbeds"`
- Labels: `"Cell {idx+1}"` → `"Testbed {idx+1}"` (if present)

- [ ] **Step 2: Add OS badge to testbed status cards**

In the testbed status section, after the cloud/region info, add:

```tsx
{testbed.os && (
  <span className={`text-[10px] px-1.5 py-0.5 rounded border ${
    testbed.os === 'windows'
      ? 'border-blue-500/30 text-blue-300'
      : 'border-green-500/30 text-green-300'
  }`}>
    {testbed.os === 'windows' ? 'Windows' : 'Linux'}
  </span>
)}
```

- [ ] **Step 3: Build and verify**

Run: `cd dashboard && npx tsc --noEmit 2>&1 | head -20`

- [ ] **Step 4: Commit**

```bash
git add dashboard/src/pages/BenchmarkProgressPage.tsx
git commit -m "refactor: rename cell → testbed in progress page"
```

---

## Task 9: Results Page — Rename cell → testbed

**Files:**
- Modify: `dashboard/src/pages/BenchmarkConfigResultsPage.tsx`

- [ ] **Step 1: Rename all cell references**

Systematic replacements:
- `cellLabel` → `testbedLabel`, param `cell: BenchmarkCellRow` → `testbed: BenchmarkTestbedRow`
- `activeCell` / `setActiveCell` → `activeTestbed` / `setActiveTestbed`
- `cellMap` → `testbedMap`
- `activeCellResults` → `activeTestbedResults`
- `crossCellRows` → `crossTestbedRows`
- `buildCrossCellRows` → `buildCrossTestbedRows`
- `CrossCellRow` → `CrossTestbedRow`
- `hasMultipleCells` → `hasMultipleTestbeds`
- `'__cross_cell__'` → `'__cross_testbed__'`
- `data.cells` → `data.testbeds`
- `res.cells` → `res.testbeds`
- `cell_id` → `testbed_id` (data access)
- `BenchmarkCellRow` → `BenchmarkTestbedRow`
- UI text: `"cell"/"cells"` → `"testbed"/"testbeds"`, `"Cross-cell"` → `"Cross-testbed"`, `"Unknown Cell"` → `"Unknown Testbed"`

- [ ] **Step 2: Add OS to testbed label**

```typescript
function testbedLabel(testbed: BenchmarkTestbedRow): string {
  const os = testbed.os === 'windows' ? 'Win' : 'Linux';
  return `${testbed.cloud} / ${testbed.region} (${testbed.topology}) [${os}]`;
}
```

- [ ] **Step 3: Build and verify**

Run: `cd dashboard && npx tsc --noEmit 2>&1 | head -20`
Expected: Clean.

- [ ] **Step 4: Commit**

```bash
git add dashboard/src/pages/BenchmarkConfigResultsPage.tsx
git commit -m "refactor: rename cell → testbed in results page"
```

---

## Task 10: Full Frontend Build + Lint

- [ ] **Step 1: TypeScript check**

Run: `cd dashboard && npx tsc --noEmit`
Expected: Zero errors.

- [ ] **Step 2: Lint**

Run: `cd dashboard && npm run lint`
Expected: Zero errors (or only pre-existing ones).

- [ ] **Step 3: Build**

Run: `cd dashboard && npm run build`
Expected: Clean build.

- [ ] **Step 4: Fix any issues and commit**

```bash
git add dashboard/
git commit -m "fix: resolve frontend build issues from testbed rename"
```

---

## Task 11: Full Workspace Verification

- [ ] **Step 1: Rust format**

Run: `cargo fmt --all`

- [ ] **Step 2: Rust clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`

- [ ] **Step 3: Rust tests**

Run: `cargo test --workspace --lib 2>&1 | tail -20`

- [ ] **Step 4: Frontend build**

Run: `cd dashboard && npm run build && npm run lint`

- [ ] **Step 5: Commit any final fixes**

```bash
git add -A
git commit -m "chore: fmt + clippy fixes for testbed rename"
```

---

## Task 12: Version Bump + CHANGELOG

**Files:**
- Modify: `Cargo.toml` (workspace version)
- Modify: `CHANGELOG.md`
- Modify: `install.sh` (INSTALLER_VERSION)
- Modify: `install.ps1` (INSTALLER_VERSION)

- [ ] **Step 1: Check current version**

Run: `grep '^version' Cargo.toml | head -1`

- [ ] **Step 2: Bump version**

Increment patch version in all 3 locations (Cargo.toml workspace version, install.sh INSTALLER_VERSION, install.ps1 INSTALLER_VERSION).

- [ ] **Step 3: Add CHANGELOG entry**

Add a new section at the top of CHANGELOG.md:

```markdown
## [X.Y.Z] - 2026-04-02

### Changed
- Renamed "Cell" to "Testbed" across entire stack (DB migration V020, API, frontend)
- Added OS selection per testbed (Linux/Windows) in benchmark wizard

### Added
- Three new benchmark templates: Linux Focus, Windows .NET, Cross-OS
- Auto-detection: selecting C# .NET 4.8 auto-switches testbed to Windows
- OS badge shown in wizard review, progress, and results pages
```

- [ ] **Step 4: Regenerate lockfile**

Run: `cargo generate-lockfile`

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md install.sh install.ps1
git commit -m "chore: bump version to X.Y.Z — testbed rename + OS selection"
```
