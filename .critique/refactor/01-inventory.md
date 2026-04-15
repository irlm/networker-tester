# Data Model Refactor Audit: Jobs ↔ Benchmarks Unification

**Objective:** Enumerate every place in the codebase that directly references Jobs (`Job`, `JobConfig`, `TestRun`, `RequestAttempt`) or Benchmarks (`BenchmarkConfig`, `BenchmarkArtifact`, `BenchmarkSample`, `BenchmarkSummary`) to plan unified `TestConfig { methodology?: Methodology }` refactor.

---

## 1. DATABASE SCHEMA

### SQL Server / Azure (sql/)

| Table | Purpose | Dependencies | Notes |
|-------|---------|--------------|-------|
| `TestRun` | One row per `networker-tester` invocation | PK: RunId | Stores: TargetUrl, Modes (comma-sep), TotalRuns, Concurrency, TimeoutMs, ClientOs, ClientVersion, SuccessCount, FailureCount. Not time-windowed. |
| `RequestAttempt` | One row per protocol probe within run | FK→TestRun.RunId (CASCADE) | Stores: Protocol, SequenceNum, Success, ErrorMessage. Leaf node for sub-results. |
| `DnsResult` | DNS query outcome | FK→RequestAttempt.AttemptId (CASCADE) | Stores: QueryName, ResolvedIPs, DurationMs, Success. |
| `TcpResult` | TCP connection outcome | FK→RequestAttempt.AttemptId (CASCADE) | Stores: LocalAddr, RemoteAddr, ConnectDurationMs, AttemptCount, MssBytesEstimate, RttEstimateMs, Success. |
| `TlsResult` | TLS handshake outcome | FK→RequestAttempt.AttemptId (CASCADE) | Stores: ProtocolVersion, CipherSuite, AlpnNegotiated, CertSubject, CertIssuer, CertExpiry, HandshakeDurationMs, Success. |
| `HttpResult` | HTTP request outcome | FK→RequestAttempt.AttemptId (CASCADE) | Stores: NegotiatedVersion, StatusCode, HeadersSizeBytes, BodySizeBytes, TtfbMs, TotalDurationMs, RedirectCount. |
| `UdpResult` | UDP probe outcome | FK→RequestAttempt.AttemptId (CASCADE) | Stores: RemoteAddr, ProbeCount, SuccessCount, LossPercent, RttMinMs, RttAvgMs, RttP95Ms, JitterMs. |
| `ErrorRecord` | Aggregated error log | FK→RequestAttempt (NO ACTION), FK→TestRun.RunId (NO ACTION) | Stores: ErrorCategory, ErrorMessage, ErrorDetail, OccurredAt. Run-level errors have NULL AttemptId. |
| `BenchmarkRun` | One row per benchmark execution | FK→TestRun.RunId (CASCADE) | Stores: ContractVersion, GeneratedAt, Source, TargetUrl, Modes, TotalRuns, Concurrency, TimeoutMs, MethodologyJson, DiagnosticsJson, AggregateSummaryJson. **Dual-key with TestRun.** |
| `BenchmarkLaunch` | One per scenario/phase launch within BenchmarkRun | FK→BenchmarkRun.BenchmarkRunId (CASCADE) | Stores: LaunchIndex, Scenario, PrimaryPhase, SampleCount, PrimarySampleCount, WarmupSampleCount, SuccessCount, FailureCount, PhasesJson. |
| `BenchmarkEnvironment` | Environment snapshot for a benchmark | FK→BenchmarkRun.BenchmarkRunId (CASCADE) | Stores: ClientInfoJson, ServerInfoJson, NetworkBaselineJson, PacketCaptureEnabled, EnvironmentJson. |
| `BenchmarkDataQuality` | Publication readiness metrics | FK→BenchmarkRun.BenchmarkRunId (CASCADE) | Stores: NoiseLevel, SampleStabilityCv, Sufficiency, PublicationReady, WarningsJson, QualityJson. |
| `BenchmarkCase` | One test case definition (protocol+stack combo) | FK→BenchmarkRun.BenchmarkRunId (CASCADE) | Stores: CaseId, Protocol, PayloadBytes, HttpStack, MetricName, MetricUnit, HigherIsBetter, CaseJson. |
| `BenchmarkSample` | Individual measurement within a case/phase | FK→BenchmarkRun, FK→BenchmarkCase, FK→RequestAttempt (CASCADE) | Stores: AttemptId, CaseId, LaunchIndex, Phase, IterationIndex, Success, RetryCount, InclusionStatus, MetricValue, MetricUnit, TotalDurationMs, TtfbMs, SampleJson. **Links back to RequestAttempt.** |
| `BenchmarkSummary` | Rolled-up stats per case | FK→BenchmarkRun.BenchmarkRunId (CASCADE) | Stores: CaseId, Protocol, PayloadBytes, HttpStack, MetricName, SampleCount, IncludedSampleCount, SuccessCount, FailureCount, Min, Mean, P5–P999, Max, Stddev, LatencyMeanMs, SummaryJson. |

**Indexes:** See SQL files for IX_TestRun_StartedAt, IX_Attempt_Protocol, IX_BenchmarkRun_GeneratedAt, IX_BenchmarkSample_RunCase, etc. 8 primary + 5 benchmark-specific.

### PostgreSQL (sql/postgres/)

Same logical structure as SQL Server, using UUID instead of NVARCHAR(36), JSONB for JSON columns, TIMESTAMPTZ for time, DOUBLE PRECISION for floats. **V001 = TestRun/RequestAttempt baseline; V003 = Benchmark tables.**

---

## 2. RUST TYPES & STRUCTS

### networker-common crate

| Type | File | Current Shape | Used In |
|------|------|---------------|---------|
| `JobConfig` | `src/messages.rs:13–59` | struct with target, modes[], runs, concurrency, timeout_secs, payload_sizes[], insecure, dns_enabled, ipv4_only, ipv6_only, connection_reuse, retries, page_preset, page_assets, page_asset_size, udp_port, udp_throughput_port, capture_mode | Dispatched over WS to agents; also in dashboard API request |
| `AgentMessage` enum | `src/messages.rs:76–` | Variants: Heartbeat, JobAck, AttemptResult, JobComplete, TlsProfileComplete, BenchmarkComplete, BenchmarkPhaseComplete, BenchmarkLog, BenchmarkError, BenchmarkHeartbeat | Agent↔Dashboard WebSocket stream |
| `Phase` enum | `src/phase.rs:12–19` | Queued, Starting, Deploy, Running, Collect, Done | Shared lifecycle; serializes lowercase |
| `Outcome` enum | `src/phase.rs:36–44` | Success, PartialSuccess, Failure, Cancelled | Terminal status; serializes lowercase |

### networker-tester crate

| Type | File | Current Shape | Used In |
|------|------|---------------|---------|
| `TestRun` struct | `src/metrics.rs:~200+` | Owns RunId, StartedAt, FinishedAt, TargetUrl, Modes, TotalRuns, Concurrency, TimeoutMs, ClientOs, ClientVersion, SuccessCount, FailureCount, attempts: Vec<RequestAttempt> | CLI output serialization; agent message payload; DB write |
| `RequestAttempt` struct | `src/metrics.rs:~300+` | Owns AttemptId, Protocol, SequenceNum, StartedAt, FinishedAt, Success, ErrorMessage, RetryCount, plus optional DnsResult, TcpResult, TlsResult, HttpResult, UdpResult, ErrorRecord | Aggregated in TestRun.attempts; persisted per protocol |
| `DnsResult` | `src/metrics.rs` | QueryName, ResolvedIPs, DurationMs, StartedAt, Success | Optional on RequestAttempt |
| `TcpResult` | `src/metrics.rs` | LocalAddr, RemoteAddr, ConnectDurationMs, AttemptCount, MssBytesEstimate, RttEstimateMs, Success | Optional on RequestAttempt |
| `TlsResult` | `src/metrics.rs` | ProtocolVersion, CipherSuite, AlpnNegotiated, CertSubject, CertIssuer, CertExpiry, HandshakeDurationMs, Success | Optional on RequestAttempt |
| `HttpResult` | `src/metrics.rs` | NegotiatedVersion, StatusCode, HeadersSizeBytes, BodySizeBytes, TtfbMs, TotalDurationMs, RedirectCount | Optional on RequestAttempt |
| `UdpResult` | `src/metrics.rs` | ProbeCount, SuccessCount, LossPercent, RttMinMs, RttAvgMs, RttP95Ms, JitterMs | Optional on RequestAttempt |
| `BenchmarkEnvironmentCheck` | `src/metrics.rs:84–96` | Stores environment baseline: attempted_samples, successful_samples, duration_ms, rtt percentiles, packet_loss_percent | Benchmark report generation |
| `BenchmarkStabilityCheck` | `src/metrics.rs:100–113` | Noise measurement: jitter_ms, packet_loss_percent, rtt stats | Pre-benchmark validation |
| `BenchmarkExecutionPlan` | `src/metrics.rs:117–131` | Adaptive sampling: source, min_samples, max_samples, min_duration_ms, target_relative_error, pilot_sample_count | Benchmark phase planning |
| `BenchmarkNoiseThresholds` | `src/metrics.rs:134–148` | max_packet_loss_percent, max_jitter_ratio, max_rtt_spread_ratio; impl Default | Publication gate logic |

### networker-dashboard crate

| Type | File | Current Shape | Used In |
|------|------|---------------|---------|
| `JobRow` | `src/db/jobs.rs:6–21` | job_id, project_id, tls_profile_run_id, definition_id, agent_id, status, config (JSONB), created_by, created_at, started_at, finished_at, run_id, error_message | DB->API serialization |
| `BenchmarkConfigRow` | `src/db/benchmark_configs.rs:7–24` | config_id, project_id, name, template, status, created_by, created_at, started_at, finished_at, config_json, error_message, max_duration_secs, baseline_run_id, worker_id, last_heartbeat, benchmark_type | DB->API serialization |
| `BenchmarkRunSummary` | `src/api/types.ts` (TypeScript analog) | Stores run_id, generated_at, target_url, modes[], concurrency, total_runs, contract_version, scenario, primary_phase, execution_plan_source, server_region, network_type, baseline_rtt_p50_ms, total_cases, publication_ready, noise_level, sufficiency | API response for benchmark list |

---

## 3. REST API ENDPOINTS

### Jobs routes (`/projects/{project_id}/jobs*`)

| Verb | Path | Handler | Request Type | Response Type | Auth Required |
|------|------|---------|--------------|---------------|---------------|
| GET | `/jobs` | `list_jobs_scoped` | Query: status?, agent_id?, created_by?, limit?, offset? | Job[] | ProjectContext + Operator/Admin |
| POST | `/jobs` | `create_job_scoped` | CreateJobRequest { config: JobConfig, agent_id?: UUID } | CreateJobResponse { job_id, status } | ProjectContext + Operator |
| GET | `/jobs/{job_id}` | `get_job` | Path: job_id | serde_json::Value (Job) | ProjectContext |
| POST | `/jobs/{job_id}/cancel` | `cancel_job_scoped` | empty | { status: "cancelled" } | ProjectContext + Operator |

**File:** `src/api/jobs.rs:265–268`

### Benchmark routes (`/projects/{project_id}/benchmarks*`)

| Verb | Path | Handler | Request Type | Response Type | Auth Required |
|------|------|---------|--------------|---------------|---------------|
| GET | `/benchmarks` | `list_benchmarks_scoped` | Query: target_host?, limit?, offset? | BenchmarkRunSummary[] | ProjectContext |
| GET | `/benchmarks/{run_id}` | `get_benchmark_scoped` | Path: run_id | BenchmarkArtifact | ProjectContext |
| POST | `/benchmarks/compare` | `compare_benchmarks_scoped` | CompareBenchmarksRequest { run_ids[], baseline_run_id? } | BenchmarkComparisonReport | ProjectContext + Operator |
| GET | `/benchmarks/presets` | `list_benchmark_presets_scoped` | none | BenchmarkComparePreset[] | ProjectContext |
| POST | `/benchmarks/presets` | `save_benchmark_preset_scoped` | BenchmarkComparePresetInput { name, runIds[], baselineRunId? } | BenchmarkComparePreset[] | ProjectContext + Operator |
| DELETE | `/benchmarks/presets/{preset_id}` | `delete_benchmark_preset_scoped` | Path: preset_id | BenchmarkComparePreset[] | ProjectContext + Operator |

**File:** `src/api/benchmarks.rs:309–317`

### Benchmark Config routes (`/projects/{project_id}/benchmark-configs*`)

| Verb | Path | Handler | Request Type | Response Type | Auth Required |
|------|------|---------|--------------|---------------|---------------|
| GET | `/benchmark-configs` | `list_configs` | Query: limit?, offset? | BenchmarkConfigRow[] | ProjectContext |
| POST | `/benchmark-configs` | `create_config` | CreateBenchmarkConfigRequest { name, template?, testbeds, languages, methodology, benchmark_type, auto_teardown? } | CreateBenchmarkConfigResponse { config_id, testbed_ids[] } | ProjectContext + Operator |
| GET | `/benchmark-configs/{config_id}` | `get_config` | Path: config_id | BenchmarkConfigWithTestbeds { config, testbeds } | ProjectContext |
| POST | `/benchmark-configs/{config_id}/launch` | `launch_config` | empty | { status, error? } | ProjectContext + Operator |
| POST | `/benchmark-configs/{config_id}/cancel` | `cancel_config` | empty | { status } | ProjectContext + Operator |
| GET | `/benchmark-configs/{config_id}/results` | `get_config_results` | Path: config_id | BenchmarkConfigResults | ProjectContext |
| GET | `/benchmark-configs/{config_id}/progress` | `get_benchmark_progress` | Path: config_id | { progress: BenchmarkLanguageProgress[] } | ProjectContext |

**File:** `src/api/benchmark_configs.rs` (router not shown; inferred from handlers)

### Benchmark Callback routes (public, no auth)

| Verb | Path | Handler | Purpose |
|------|------|---------|---------|
| POST | `/benchmarks/callback/status` | `callback_status` | Worker heartbeat during benchmark |
| POST | `/benchmarks/callback/log` | `callback_log` | Worker log streaming |
| POST | `/benchmarks/callback/result` | `callback_result` | Phase-level results |
| POST | `/benchmarks/callback/complete` | `callback_complete` | Benchmark completion |
| POST | `/benchmarks/callback/heartbeat` | `callback_heartbeat` | Keep-alive |

**File:** `src/api/benchmark_callbacks.rs:526–530`

### Runs routes (`/projects/{project_id}/runs*`)

| Verb | Path | Handler | Request Type | Response Type | Auth Required |
|------|------|---------|--------------|---------------|---------------|
| GET | `/runs` | `list_runs` | Query: target_host?, mode?, limit?, offset? | RunSummary[] | ProjectContext |
| GET | `/runs/{run_id}` | `get_run` | Path: run_id | serde_json::Value (TestRun summary) | ProjectContext |
| GET | `/runs/{run_id}/attempts` | `get_run_attempts` | Path: run_id | serde_json::Value (Attempt[]) | ProjectContext |

**File:** `src/api/runs.rs` (inferred from impl)

### Schedules routes (also use JobConfig/BenchmarkConfig)

| Verb | Path | Relevant Fields | Notes |
|------|------|-----------------|-------|
| POST | `/schedules` | config: JobConfig \| {}, benchmark_config_id? | Can carry either JobConfig or reference benchmark_config_id |
| PUT | `/schedules/{id}` | config: JobConfig | Allows JobConfig updates |

**File:** `src/api/schedules.rs` (not examined; referenced in client.ts)

---

## 4. FRONTEND API CALL SITES (TypeScript)

### dashboard/src/api/client.ts

| Line Range | Function Name | Call Target | Endpoint | Notes |
|------------|---------------|-------------|----------|-------|
| 334–343 | `getJobs` | GET | `/projects/{pid}/jobs?...` | QueryParams: status, agent_id, created_by, limit, offset |
| 345 | `getJob` | GET | `/projects/{pid}/jobs/{jobId}` | Single job fetch |
| 347–351 | `createJob` | POST | `/projects/{pid}/jobs` | Body: { config: JobConfig, agent_id? } |
| 353–354 | `cancelJob` | POST | `/projects/{pid}/jobs/{jobId}/cancel` | Cancel running/pending job |
| 394–401 | `getBenchmarks` | GET | `/projects/{pid}/benchmarks?...` | QueryParams: target_host, limit, offset |
| 403–404 | `getBenchmark` | GET | `/projects/{pid}/benchmarks/{runId}` | Fetch single benchmark artifact |
| 406–410 | `compareBenchmarks` | POST | `/projects/{pid}/benchmarks/compare` | Body: { run_ids[], baseline_run_id? } |
| 412–413 | `getBenchmarkComparePresets` | GET | `/projects/{pid}/benchmarks/presets` | Fetch saved comparison presets |
| 415–419 | `saveBenchmarkComparePreset` | POST | `/projects/{pid}/benchmarks/presets` | Body: BenchmarkComparePresetInput |
| 421–424 | `deleteBenchmarkComparePreset` | DELETE | `/projects/{pid}/benchmarks/presets/{presetId}` | Remove preset |
| 502–515 | `createSchedule` | POST | `/projects/{pid}/schedules` | Body includes: config: JobConfig \| {}, benchmark_config_id? |
| 517–529 | `updateSchedule` | PUT | `/projects/{pid}/schedules/{id}` | Body includes: config: JobConfig |
| 771–772 | `listBenchmarkConfigs` | GET | `/projects/{pid}/benchmark-configs` | List benchmark configuration templates |
| 774–775 | `getBenchmarkConfig` | GET | `/projects/{pid}/benchmark-configs/{configId}` | Fetch single config |
| 777–781 | `createBenchmarkConfig` | POST | `/projects/{pid}/benchmark-configs` | Body: { name, testbeds[], languages[], methodology, benchmark_type?, auto_teardown? } |
| 783–784 | `launchBenchmarkConfig` | POST | `/projects/{pid}/benchmark-configs/{configId}/launch` | Trigger execution |
| 786–787 | `cancelBenchmarkConfig` | POST | `/projects/{pid}/benchmark-configs/{configId}/cancel` | Stop execution |
| 789–790 | `getBenchmarkConfigResults` | GET | `/projects/{pid}/benchmark-configs/{configId}/results` | Fetch results |
| 792–795 | `getBenchmarkProgress` | GET | `/projects/{pid}/benchmark-configs/{configId}/progress` | Fetch live progress |

**Total direct call sites:** 19

### dashboard/src/api/types.ts

| Interface | Lines | Used By |
|-----------|-------|---------|
| `Job` | 19–32 | getJob response, job list items |
| `JobConfig` | 34–50 | createJob request, schedule.config, editor state |
| `RunSummary` | 52–62 | getRuns response |
| `BenchmarkRunSummary` | 64–87 | getBenchmarks response, benchmark list table |
| `BenchmarkComparePreset` | 105–113 | compareBenchmarks presets |
| `BenchmarkConfigSummary` | (in imports) | listBenchmarkConfigs response |

---

## 5. CLI / CONFIG SURFACE

### networker-tester binary

| File | Shape | Used For |
|------|-------|----------|
| `src/cli.rs` (inferred) | Parses CLI args → ResolvedConfig | Local tester invocation; modes, target, runs, concurrency, timeout, payload_sizes, etc. |
| `src/main.rs` (inferred) | Entry point; dispatches to runner/ based on modes | Invoked by agent with JobConfig or by user locally |
| Environment vars | NETWORKER_DB_URL (optional) | DB persistence when `--save-to-db` flag used |

**Config flow:** CLI args → ResolvedConfig → internally mapped to JobConfig for agent dispatch or TestRun for local execution.

### install.sh / install.ps1

| Script | References | Purpose |
|--------|-----------|---------|
| `install.sh` | Mentions "benchmark", "test", "agent", "mode", "url" | Sets up environment; no embedded test/benchmark config templates |
| `install.ps1` | Same | Windows equivalent |

---

## 6. EXTERNAL SURFACES (Docs, Examples)

### examples/ directory

| File | Contains | Notes |
|------|----------|-------|
| Various JSON/shell scripts (if present) | Sample JobConfig, test invocations | Likely show CLI usage only, not benchmark configs |

### docs/ directory

| File | References | Notes |
|------|-----------|-------|
| (To be verified) | Likely quickstart for CLI, not benchmark details | Benchmark schema is internal to dashboard |

---

## 7. WEBSOCKET & MESSAGE STREAMS

### Agent ↔ Dashboard WebSocket

**File:** `src/messages.rs`

| Message Type | Variants | Payload |
|--------------|----------|---------|
| `ControlMessage` (CP → Agent) | Job { job_config: JobConfig }, TlsProfile, BenchmarkStart, BenchmarkCancel | Dispatches work; JobConfig embedded |
| `AgentMessage` (Agent → CP) | JobAck, AttemptResult, JobComplete { run }, TlsProfileComplete, BenchmarkComplete, BenchmarkPhaseComplete, BenchmarkLog, BenchmarkError | Streams results; TestRun and BenchmarkSample embedded |

**Channels:** Broadcast per project; sequenced event bus with replay buffer.

---

## 8. DATABASE ACCESS LAYERS (DAL)

### Dashboard crates/networker-dashboard/src/db/

| Module | Queries | Struct Used |
|--------|---------|-------------|
| `jobs.rs` | create, get, list, list_filtered, update status/run_id | JobRow |
| `benchmark_configs.rs` | create, get, list, list_filtered, update status | BenchmarkConfigRow |
| `benchmarks.rs` | list, get_by_run_id, get_summary, compare, insert results | BenchmarkRunSummary (+ internal denorm rows) |
| `runs.rs` | get_attempts, get_summary (wraps TestRun joins) | (anonymous; returns JSON) |
| `benchmark_testbeds.rs` | create, list_for_config | BenchmarkTestbedRow |
| `benchmark_presets.rs` | create, get, list, upsert, delete | BenchmarkComparePreset |
| `schedules.rs` | create, get, list, update, delete; stores config: JSONB | Schedule (stores JobConfig or benchmark_config_id) |

---

## SUMMARY TABLE: REFACTOR BLAST RADIUS

| Category | Count | Details |
|----------|-------|---------|
| **Database Tables** | 15 | 8 for Jobs/Runs; 7 for Benchmarks (BenchmarkRun, Launch, Environment, DataQuality, Case, Sample, Summary) |
| **SQL Indexes** | 13+ | Spread across both schemas; time-based, protocol-based, quality-based filters |
| **Rust Structs (Core)** | 12+ | TestRun, RequestAttempt, DNS/TCP/TLS/HTTP/UDP Results, Benchmark{Environment,Stability,Execution,NoiseThresholds} |
| **Rust Structs (Dashboard)** | 3 | JobRow, BenchmarkConfigRow, BenchmarkRunSummary |
| **REST Endpoints** | 18+ | 4 for Jobs, 6 for Benchmarks, 7 for BenchmarkConfigs, 3+ for Runs, 5 for Callbacks |
| **TypeScript Call Sites** | 19 | createJob, getBenchmarks, createBenchmarkConfig, compareBenchmarks, listBenchmarkConfigs, launchBenchmarkConfig, etc. |
| **WebSocket Message Types** | 2 major | ControlMessage (JobConfig dispatch), AgentMessage (ResultStreaming) |
| **TypeScript Interfaces** | 6+ | Job, JobConfig, BenchmarkRunSummary, BenchmarkComparePreset, BenchmarkConfigSummary |
| **DAL Modules** | 8 | jobs, benchmarks, benchmark_configs, runs, schedules, benchmark_testbeds, benchmark_presets |
| **Config/CLI Entry Points** | 2 | CLI args, environment vars (NETWORKER_DB_URL) |

---

## REFACTOR STRATEGY NOTES

1. **Unified TestConfig { methodology?: Methodology }** would consolidate JobConfig + BenchmarkConfig at the REST/messaging layer.
2. **Database:** Likely requires a view or union over job + benchmark_config tables during transition; full migration defers to Phase 2.
3. **API:** Single endpoint `/projects/{pid}/tests` with POST body { methodology: "benchmark" | "diagnostic" } determines behavior downstream.
4. **Types:** Rename JobConfig → TestConfig; add methodology field (enum or string); benchmark-specific fields become methodology.* properties.
5. **WebSocket:** ControlMessage.Job → ControlMessage.Test; AgentMessage unchanged (already polymorphic).
6. **Frontend:** client.ts needs unified createTest(), listTests(), with methodology param branching to benchmark or diagnostic UI.
7. **CLI:** Minimal change; mode auto-detection or explicit --methodology flag can infer benchmark vs. test.

**Total surfaces to touch:** ~15 tables, 12+ core structs, 18+ endpoints, 19+ FE call sites, 8 DAL modules, 2 message types.

---

**Report Generated:** 2026-04-15  
**Source Audit:** networker-tester repository
