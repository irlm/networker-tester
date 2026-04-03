# Real-Time Benchmark Progress — Per-Request DB Tracking

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show live per-mode progress bars with running p50 stats during benchmark runs, backed by per-request results saved to the database.

**Architecture:** The tester gains `--progress-url` and `--progress-token` flags. After each measured request, it POSTs a compact progress payload to the dashboard API. The dashboard saves each request to a new `benchmark_request_progress` table and broadcasts via WebSocket. The orchestrator passes the callback URL/token through to the tester subprocess. The frontend polls the progress table and shows per-mode progress bars with running p50.

**Tech Stack:** Rust (networker-tester CLI, orchestrator, dashboard/axum), PostgreSQL (V021 migration), React/TypeScript (frontend)

---

## File Structure

### Database
- **Modify:** `crates/networker-dashboard/src/db/migrations.rs` — V021: `benchmark_request_progress` table
- **Create:** `crates/networker-dashboard/src/db/benchmark_progress.rs` — insert + query functions
- **Modify:** `crates/networker-dashboard/src/db/mod.rs` — add module

### Dashboard API
- **Modify:** `crates/networker-dashboard/src/api/benchmark_callbacks.rs` — new `callback_request_progress` endpoint
- **Modify:** `crates/networker-dashboard/src/api/benchmark_configs.rs` — new `get_progress` endpoint for frontend polling

### Tester CLI
- **Modify:** `crates/networker-tester/src/cli.rs` — add `--progress-url`, `--progress-token`, `--progress-interval` flags
- **Modify:** `crates/networker-tester/src/main.rs` — POST progress after each request batch

### Orchestrator
- **Modify:** `benchmarks/orchestrator/src/executor.rs` — pass `--progress-url` and `--progress-token` to tester subprocess
- **Modify:** `benchmarks/orchestrator/src/config.rs` — add callback_url/token fields to config (passed from benchmark_worker)

### Frontend
- **Modify:** `dashboard/src/pages/BenchmarkProgressPage.tsx` — per-mode progress bars with running p50
- **Modify:** `dashboard/src/api/client.ts` — add `getBenchmarkProgress` API call
- **Modify:** `dashboard/src/api/types.ts` — add progress types

---

## Task 1: V021 Database Migration — benchmark_request_progress Table

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs`

- [ ] **Step 1: Add V021 migration constant**

After the V020 block, add:

```rust
/// V021 migration: Per-request benchmark progress tracking.
const V021_BENCHMARK_REQUEST_PROGRESS: &str = r#"
CREATE TABLE IF NOT EXISTS benchmark_request_progress (
    id              BIGSERIAL       PRIMARY KEY,
    config_id       UUID            NOT NULL,
    testbed_id      UUID,
    language        TEXT            NOT NULL,
    mode            TEXT            NOT NULL,
    request_index   INT             NOT NULL,
    total_requests  INT             NOT NULL,
    latency_ms      DOUBLE PRECISION NOT NULL,
    success         BOOLEAN         NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ     NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ix_brp_config_lang
    ON benchmark_request_progress (config_id, language, mode);

CREATE INDEX IF NOT EXISTS ix_brp_config
    ON benchmark_request_progress (config_id);
"#;
```

- [ ] **Step 2: Add V021 application block**

After V020 application block:

```rust
    // V021: Per-request benchmark progress tracking
    let v021_applied = client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmark_request_progress')",
            &[],
        )
        .await?
        .get::<_, bool>(0);
    if !v021_applied {
        tracing::info!("Applying V021 benchmark_request_progress migration...");
        client.batch_execute(V021_BENCHMARK_REQUEST_PROGRESS).await?;
        client
            .execute(
                "INSERT INTO schema_version (version, description) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                &[&21i32, &"benchmark request progress tracking"],
            )
            .await?;
        tracing::info!("V021 migration complete");
    }
```

- [ ] **Step 3: Build to verify**

Run: `cargo build -p networker-dashboard 2>&1 | tail -5`
Expected: Compiles (migration is a string constant)

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/db/migrations.rs
git commit -m "feat: V021 migration — benchmark_request_progress table"
```

---

## Task 2: DB Functions — Insert + Query Progress

**Files:**
- Create: `crates/networker-dashboard/src/db/benchmark_progress.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`

- [ ] **Step 1: Create benchmark_progress.rs**

```rust
use tokio_postgres::Client;
use uuid::Uuid;

/// Insert a batch of request progress rows.
pub async fn insert_batch(
    client: &Client,
    config_id: &Uuid,
    testbed_id: Option<&Uuid>,
    language: &str,
    mode: &str,
    requests: &[(i32, i32, f64, bool)], // (request_index, total_requests, latency_ms, success)
) -> anyhow::Result<()> {
    if requests.is_empty() {
        return Ok(());
    }

    // Build a multi-row INSERT for efficiency
    let mut query = String::from(
        "INSERT INTO benchmark_request_progress (config_id, testbed_id, language, mode, request_index, total_requests, latency_ms, success) VALUES "
    );
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> = Vec::new();
    let mut param_idx = 1u32;

    for (i, (req_index, total, latency, success)) in requests.iter().enumerate() {
        if i > 0 {
            query.push_str(", ");
        }
        query.push_str(&format!(
            "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${})",
            param_idx, param_idx + 1, param_idx + 2, param_idx + 3,
            param_idx + 4, param_idx + 5, param_idx + 6, param_idx + 7,
        ));
        params.push(Box::new(*config_id));
        params.push(Box::new(testbed_id.copied()));
        params.push(Box::new(language.to_string()));
        params.push(Box::new(mode.to_string()));
        params.push(Box::new(*req_index));
        params.push(Box::new(*total));
        params.push(Box::new(*latency));
        params.push(Box::new(*success));
        param_idx += 8;
    }

    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> =
        params.iter().map(|p| p.as_ref()).collect();
    client.execute(&query, &param_refs).await?;
    Ok(())
}

/// Progress summary per mode for a config+language.
#[derive(Debug, serde::Serialize)]
pub struct ModeProgress {
    pub mode: String,
    pub completed: i64,
    pub total: i32,
    pub p50_ms: Option<f64>,
    pub mean_ms: Option<f64>,
    pub success_count: i64,
    pub fail_count: i64,
}

/// Language-level progress summary.
#[derive(Debug, serde::Serialize)]
pub struct LanguageProgress {
    pub language: String,
    pub testbed_id: Option<Uuid>,
    pub modes: Vec<ModeProgress>,
}

/// Get progress for all languages in a benchmark config.
pub async fn get_progress(
    client: &Client,
    config_id: &Uuid,
) -> anyhow::Result<Vec<LanguageProgress>> {
    let rows = client
        .query(
            "SELECT
                language,
                testbed_id,
                mode,
                COUNT(*) as completed,
                MAX(total_requests) as total,
                PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY latency_ms) as p50,
                AVG(latency_ms) as mean,
                COUNT(*) FILTER (WHERE success) as success_count,
                COUNT(*) FILTER (WHERE NOT success) as fail_count
             FROM benchmark_request_progress
             WHERE config_id = $1
             GROUP BY language, testbed_id, mode
             ORDER BY language, mode",
            &[config_id],
        )
        .await?;

    // Group by (language, testbed_id)
    let mut map: std::collections::BTreeMap<(String, Option<Uuid>), Vec<ModeProgress>> =
        std::collections::BTreeMap::new();

    for row in &rows {
        let language: String = row.get("language");
        let testbed_id: Option<Uuid> = row.get("testbed_id");
        let mode: String = row.get("mode");
        let completed: i64 = row.get("completed");
        let total: i32 = row.get("total");
        let p50: Option<f64> = row.get("p50");
        let mean: Option<f64> = row.get("mean");
        let success_count: i64 = row.get("success_count");
        let fail_count: i64 = row.get("fail_count");

        map.entry((language, testbed_id))
            .or_default()
            .push(ModeProgress {
                mode,
                completed,
                total,
                p50_ms: p50,
                mean_ms: mean,
                success_count,
                fail_count,
            });
    }

    Ok(map
        .into_iter()
        .map(|((language, testbed_id), modes)| LanguageProgress {
            language,
            testbed_id,
            modes,
        })
        .collect())
}

/// Delete progress rows for a config (cleanup after benchmark completes).
pub async fn delete_for_config(client: &Client, config_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "DELETE FROM benchmark_request_progress WHERE config_id = $1",
            &[config_id],
        )
        .await?;
    Ok(())
}
```

- [ ] **Step 2: Add module to mod.rs**

In `crates/networker-dashboard/src/db/mod.rs`, add:
```rust
pub mod benchmark_progress;
```

- [ ] **Step 3: Build to verify**

Run: `cargo build -p networker-dashboard 2>&1 | tail -5`

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/db/benchmark_progress.rs crates/networker-dashboard/src/db/mod.rs
git commit -m "feat: benchmark_progress DB functions — insert batch + query with p50"
```

---

## Task 3: Dashboard API — Progress Callback + Query Endpoints

**Files:**
- Modify: `crates/networker-dashboard/src/api/benchmark_callbacks.rs` — new POST endpoint
- Modify: `crates/networker-dashboard/src/api/benchmark_configs.rs` — new GET endpoint

- [ ] **Step 1: Add request progress callback payload and handler**

In `benchmark_callbacks.rs`, add the payload struct:

```rust
#[derive(Deserialize)]
struct RequestProgressPayload {
    config_id: Uuid,
    testbed_id: Option<Uuid>,
    language: String,
    mode: String,
    request_index: i32,
    total_requests: i32,
    latency_ms: f64,
    success: bool,
}
```

Add the handler:

```rust
/// POST /api/benchmarks/callback/request-progress
async fn callback_request_progress(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<RequestProgressPayload>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _claims = extract_callback_token(&headers, &state.jwt_secret)?;

    let db = state.db.get().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::db::benchmark_progress::insert_batch(
        &db,
        &payload.config_id,
        payload.testbed_id.as_ref(),
        &payload.language,
        &payload.mode,
        &[(payload.request_index, payload.total_requests, payload.latency_ms, payload.success)],
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to insert request progress");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Broadcast progress update via WebSocket (throttled — only every 10th request)
    if payload.request_index % 10 == 0 || payload.request_index == payload.total_requests {
        let _ = state.events_tx.send(
            networker_common::messages::DashboardEvent::BenchmarkUpdate {
                config_id: payload.config_id,
                event_type: "request_progress".into(),
                payload: serde_json::json!({
                    "testbed_id": payload.testbed_id,
                    "language": payload.language,
                    "mode": payload.mode,
                    "completed": payload.request_index,
                    "total": payload.total_requests,
                    "latency_ms": payload.latency_ms,
                }),
            },
        );
    }

    Ok(Json(serde_json::json!({"ok": true})))
}
```

- [ ] **Step 2: Register the route**

In the router function at the bottom of `benchmark_callbacks.rs`, add:

```rust
.route("/benchmarks/callback/request-progress", post(callback_request_progress))
```

- [ ] **Step 3: Add GET progress endpoint in benchmark_configs.rs**

Add a new handler:

```rust
/// GET /api/benchmark-configs/:config_id/progress
async fn get_benchmark_progress(
    State(state): State<Arc<AppState>>,
    _claims: crate::auth::Claims,
    Path(config_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.get().await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let progress = crate::db::benchmark_progress::get_progress(&db, &config_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to get benchmark progress");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(serde_json::json!({ "progress": progress })))
}
```

Register the route alongside existing benchmark config routes:

```rust
.route("/benchmark-configs/:config_id/progress", get(get_benchmark_progress))
```

- [ ] **Step 4: Build and verify**

Run: `cargo build -p networker-dashboard 2>&1 | tail -10`

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/api/benchmark_callbacks.rs crates/networker-dashboard/src/api/benchmark_configs.rs
git commit -m "feat: request-progress callback endpoint + GET progress query"
```

---

## Task 4: Tester CLI — Add Progress Reporting Flags

**Files:**
- Modify: `crates/networker-tester/src/cli.rs`

- [ ] **Step 1: Add CLI flags**

In the `Cli` struct in `cli.rs`, add after the benchmark flags section:

```rust
    // ── Benchmark progress reporting ────────────────────────────────────────
    /// URL to POST per-request progress (used by orchestrator integration)
    #[arg(long, hide = true)]
    pub progress_url: Option<String>,

    /// Bearer token for progress URL authentication
    #[arg(long, hide = true)]
    pub progress_token: Option<String>,

    /// POST progress every N requests (default: 1 = every request)
    #[arg(long, hide = true, default_value = "1")]
    pub progress_interval: u32,

    /// Config ID for progress reporting (passed by orchestrator)
    #[arg(long, hide = true)]
    pub progress_config_id: Option<String>,

    /// Testbed ID for progress reporting (passed by orchestrator)
    #[arg(long, hide = true)]
    pub progress_testbed_id: Option<String>,
```

Also add these fields to the `ResolvedConfig` struct and the `resolve()` method, passing them through unchanged.

- [ ] **Step 2: Build to verify**

Run: `cargo build -p networker-tester 2>&1 | tail -5`

- [ ] **Step 3: Commit**

```bash
git add crates/networker-tester/src/cli.rs
git commit -m "feat: add --progress-url/token/interval flags to tester CLI"
```

---

## Task 5: Tester — POST Progress After Each Request

**Files:**
- Modify: `crates/networker-tester/src/main.rs`

- [ ] **Step 1: Create a progress reporter helper**

Near the top of `main.rs` (after imports), add a progress reporter struct:

```rust
/// Sends per-request progress to the dashboard via HTTP POST.
struct ProgressReporter {
    client: reqwest::Client,
    url: String,
    token: String,
    config_id: String,
    testbed_id: Option<String>,
    interval: u32,
    counter: std::sync::atomic::AtomicU32,
}

impl ProgressReporter {
    fn new(url: String, token: String, config_id: String, testbed_id: Option<String>, interval: u32) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
            token,
            config_id,
            testbed_id,
            interval,
            counter: std::sync::atomic::AtomicU32::new(0),
        }
    }

    async fn report(&self, language: &str, mode: &str, request_index: u32, total_requests: u32, latency_ms: f64, success: bool) {
        let count = self.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Only POST every `interval` requests (or the last one)
        if count % self.interval != 0 && request_index < total_requests {
            return;
        }
        let payload = serde_json::json!({
            "config_id": self.config_id,
            "testbed_id": self.testbed_id,
            "language": language,
            "mode": mode,
            "request_index": request_index,
            "total_requests": total_requests,
            "latency_ms": latency_ms,
            "success": success,
        });
        let _ = self.client
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
    }
}
```

- [ ] **Step 2: Initialize reporter in main if flags are set**

In the `run()` function, after config resolution, create the reporter:

```rust
    let progress_reporter = cfg.progress_url.as_ref().map(|url| {
        std::sync::Arc::new(ProgressReporter::new(
            format!("{}/api/benchmarks/callback/request-progress", url.trim_end_matches('/')),
            cfg.progress_token.clone().unwrap_or_default(),
            cfg.progress_config_id.clone().unwrap_or_default(),
            cfg.progress_testbed_id.clone(),
            cfg.progress_interval,
        ))
    });
```

- [ ] **Step 3: Call reporter after each `log_attempt` call**

After every `log_attempt(attempt)` call in the main request loop, add:

```rust
    if let Some(ref reporter) = progress_reporter {
        let mode_str = attempt.protocol.to_string();
        let latency = attempt.total_time_ms;
        let success = attempt.success;
        let reporter = reporter.clone();
        // Fire-and-forget — don't block the benchmark
        tokio::spawn(async move {
            reporter.report(
                &cfg.benchmark_language,  // language name passed from orchestrator
                &mode_str,
                request_index as u32,
                total_requests as u32,
                latency,
                success,
            ).await;
        });
    }
```

Note: The exact placement depends on the request loop structure. The reporter needs: the mode name, request index within the mode, total requests per mode, latency, and success. These are available from the `attempt` struct and the loop counter.

The `benchmark_language` field needs to be added to the CLI as `--benchmark-language` (a new hidden flag passed by the orchestrator).

- [ ] **Step 4: Build and test**

Run: `cargo build -p networker-tester 2>&1 | tail -5`

Test with a local endpoint:
```bash
cargo run -p networker-tester -- \
  --target https://localhost:8443/health \
  --modes http1 --runs 5 --insecure \
  --benchmark-mode --json-stdout \
  --progress-url http://localhost:3000 \
  --progress-token test \
  --progress-config-id 00000000-0000-0000-0000-000000000001 \
  --benchmark-language rust
```

- [ ] **Step 5: Commit**

```bash
git add crates/networker-tester/src/main.rs
git commit -m "feat: tester POSTs per-request progress to dashboard callback"
```

---

## Task 6: Orchestrator — Pass Progress URL/Token to Tester

**Files:**
- Modify: `benchmarks/orchestrator/src/executor.rs`
- Modify: `benchmarks/orchestrator/src/config.rs`

- [ ] **Step 1: Add callback fields to DashboardBenchmarkConfig**

In `config.rs`, add to `DashboardBenchmarkConfig`:

```rust
    /// Callback URL for the dashboard (used to construct progress URL for tester)
    #[serde(default)]
    pub callback_url: Option<String>,

    /// Callback token for authentication
    #[serde(default)]
    pub callback_token: Option<String>,
```

- [ ] **Step 2: Pass progress flags to tester in executor.rs**

In the `run_language_benchmark` function, modify the args construction. Add `callback_url`, `callback_token`, `testbed_id`, and `language` as parameters to the function. Then append the progress flags:

```rust
    // Add progress reporting flags if callback URL is available
    if let Some(ref url) = callback_url {
        args.push("--progress-url".to_string());
        args.push(url.clone());
        if let Some(ref token) = callback_token {
            args.push("--progress-token".to_string());
            args.push(token.clone());
        }
        args.push("--progress-config-id".to_string());
        args.push(config_id.to_string());
        if let Some(ref tid) = testbed_id {
            args.push("--progress-testbed-id".to_string());
            args.push(tid.clone());
        }
        args.push("--benchmark-language".to_string());
        args.push(language.to_string());
    }
```

- [ ] **Step 3: Update benchmark_worker.rs to include callback URL/token in config JSON**

In `crates/networker-dashboard/src/benchmark_worker.rs`, add the callback URL and token to the config JSON that gets written to the temp file:

```rust
    let config_data = serde_json::json!({
        "config_id": config.config_id.to_string(),
        "testbeds": merged_testbeds,
        "methodology": inner.get("methodology").cloned().unwrap_or(serde_json::json!({})),
        "auto_teardown": inner.get("auto_teardown").and_then(|v| v.as_bool()).unwrap_or(true),
        "callback_url": callback_url,
        "callback_token": callback_token,
    });
```

- [ ] **Step 4: Build all**

Run: `cargo build --workspace 2>&1 | tail -10`
Run: `cd benchmarks/orchestrator && cargo build 2>&1 | tail -5`

- [ ] **Step 5: Commit**

```bash
git add benchmarks/orchestrator/src/ crates/networker-dashboard/src/benchmark_worker.rs
git commit -m "feat: orchestrator passes progress URL/token to tester subprocess"
```

---

## Task 7: Frontend Types + API Client

**Files:**
- Modify: `dashboard/src/api/types.ts`
- Modify: `dashboard/src/api/client.ts`

- [ ] **Step 1: Add progress types**

In `types.ts`, add:

```typescript
export interface BenchmarkModeProgress {
  mode: string;
  completed: number;
  total: number;
  p50_ms: number | null;
  mean_ms: number | null;
  success_count: number;
  fail_count: number;
}

export interface BenchmarkLanguageProgress {
  language: string;
  testbed_id: string | null;
  modes: BenchmarkModeProgress[];
}

export interface BenchmarkProgressResponse {
  progress: BenchmarkLanguageProgress[];
}
```

- [ ] **Step 2: Add API call**

In `client.ts`, add:

```typescript
getBenchmarkProgress: (projectId: string, configId: string) =>
  request<BenchmarkProgressResponse>(
    `/api/projects/${projectId}/benchmark-configs/${configId}/progress`
  ),
```

- [ ] **Step 3: Commit**

```bash
git add dashboard/src/api/types.ts dashboard/src/api/client.ts
git commit -m "feat: frontend types + API client for benchmark progress"
```

---

## Task 8: Frontend — Per-Mode Progress Bars with Running P50

**Files:**
- Modify: `dashboard/src/pages/BenchmarkProgressPage.tsx`

- [ ] **Step 1: Add progress polling**

Import the new types and add a polling state:

```typescript
import type { BenchmarkLanguageProgress } from '../api/types';

// Inside component:
const [langProgress, setLangProgress] = useState<BenchmarkLanguageProgress[]>([]);

// Poll progress every 5 seconds while active
useEffect(() => {
  if (!configId || !projectId || !isActive) return;
  const fetchProgress = () => {
    api.getBenchmarkProgress(projectId, configId)
      .then(data => setLangProgress(data.progress))
      .catch(() => {});
  };
  fetchProgress();
  const interval = setInterval(fetchProgress, 5000);
  return () => clearInterval(interval);
}, [configId, projectId, isActive]);
```

- [ ] **Step 2: Replace the language progress table with per-mode progress bars**

For each language in the table, if it has mode progress data, show expandable per-mode bars:

```tsx
{/* Per-mode progress bars for running language */}
{langModes.length > 0 && (
  <tr key={`${lang}-modes`}>
    <td colSpan={7} className="px-4 py-1 bg-gray-900/30">
      <div className="space-y-1 py-1">
        {langModes.map(m => (
          <div key={m.mode} className="flex items-center gap-3 text-[11px] font-mono">
            <span className="w-16 text-gray-500 text-right">{m.mode}</span>
            <div className="flex-1 h-1.5 bg-gray-800 rounded-full overflow-hidden">
              <div
                className="h-full bg-cyan-500/60 rounded-full transition-all"
                style={{ width: `${m.total > 0 ? (m.completed / m.total) * 100 : 0}%` }}
              />
            </div>
            <span className="w-20 text-gray-400">{m.completed}/{m.total}</span>
            {m.p50_ms != null && (
              <span className="w-24 text-gray-500">p50: {m.p50_ms < 1 ? `${(m.p50_ms * 1000).toFixed(0)}µs` : `${m.p50_ms.toFixed(2)}ms`}</span>
            )}
          </div>
        ))}
      </div>
    </td>
  </tr>
)}
```

- [ ] **Step 3: Update the overall summary to use real progress counts from DB**

Replace the `savedResults.length`-based counter with the sum of completed requests from `langProgress`:

```typescript
const totalCompletedRequests = langProgress.reduce(
  (sum, lp) => sum + lp.modes.reduce((ms, m) => ms + m.completed, 0), 0
);
const totalExpectedRequests = langProgress.reduce(
  (sum, lp) => sum + lp.modes.reduce((ms, m) => ms + m.total, 0), 0
) || (progressStats.totalRuns * (methodology?.measured ?? 50) * (methodology?.modeCount ?? 1) * (methodology?.payloadMultiplier ?? 1));
```

- [ ] **Step 4: Build and verify**

Run: `cd dashboard && npx tsc --noEmit && npm run build`

- [ ] **Step 5: Commit**

```bash
git add dashboard/src/pages/BenchmarkProgressPage.tsx
git commit -m "feat: per-mode progress bars with running p50 stats"
```

---

## Task 9: Full Build + Test

- [ ] **Step 1: Workspace build**

Run: `cargo build --workspace && cd benchmarks/orchestrator && cargo build`

- [ ] **Step 2: Clippy**

Run: `cargo clippy --all-targets -- -D warnings`

- [ ] **Step 3: Unit tests**

Run: `cargo test --workspace --lib`

- [ ] **Step 4: Frontend build**

Run: `cd dashboard && npm run build`

- [ ] **Step 5: Commit any fixes**

```bash
git add -A && git commit -m "fix: build/lint issues"
```

---

## Task 10: Version Bump + CHANGELOG

- [ ] **Step 1: Bump version** in `Cargo.toml`, `install.sh`, `install.ps1`

- [ ] **Step 2: Add CHANGELOG entry**

```markdown
## [0.17.2] - 2026-04-03

### Added
- Real-time per-request benchmark progress: tester POSTs each request result to dashboard
- V021 migration: `benchmark_request_progress` table with per-mode tracking
- Progress page: per-mode progress bars with running p50 latency stats
- `--progress-url`/`--progress-token` flags on networker-tester for orchestrator integration
```

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md install.sh install.ps1
git commit -m "chore: bump to 0.17.2 — real-time benchmark progress"
```

---

## Deployment Notes

This feature requires deploying **all three binaries**:
1. `networker-dashboard` — new V021 migration + API endpoints
2. `alethabench` orchestrator — passes progress flags to tester
3. `networker-tester` — the tester binary on the VM must also be updated (it's what runs the benchmark and POSTs progress)

The orchestrator binary must be built from `benchmarks/orchestrator/` (excluded from workspace). The VM (B1ls) needs 2G swap enabled for Rust builds.
