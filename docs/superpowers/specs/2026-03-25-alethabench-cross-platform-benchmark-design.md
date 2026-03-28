# AletheBench — Cross-Platform HTTP Benchmark Suite

## Goal

A reproducible, cross-language HTTP performance benchmark suite that compares the same minimal API implemented in 9+ languages across multiple OS versions and cloud providers, measuring network performance, server resources, capacity limits, packet behavior, browser performance, and cost efficiency. Results are viewable via CLI, HTML reports, JSON API, and the AletheDash dashboard.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  alethabench CLI (Rust binary)                          │
│  - Reads benchmark.json config                          │
│  - Provisions VMs per test matrix                       │
│  - Deploys reference API implementations                │
│  - Dispatches networker-tester runs + browser tests     │
│  - Collects metrics (perf + resource + packets)         │
│  - Calculates cost estimates                            │
│  - Outputs JSON/HTML reports                            │
│  - Optionally uploads results to AletheDash             │
└──────────┬──────────────────────────┬───────────────────┘
           │                          │
    ┌──────▼──────┐          ┌────────▼────────┐
    │ Client VMs  │          │  Server VMs     │
    │ Ubuntu 24   │          │  per-language   │
    │ Win Srv 2022│          │  reference API  │
    │ Windows 11  │          │  + metrics agent│
    │ networker-  │          │                 │
    │ tester +    │          │                 │
    │ browsers    │          │                 │
    └─────────────┘          └─────────────────┘
           │                          │
           └──────────┬───────────────┘
                      │
              ┌───────▼───────┐
              │  AletheDash   │
              │  /benchmarks  │
              │  page + API   │
              └───────────────┘
```

## Tech Stack

- **CLI**: Rust (alethabench binary, reuses networker-tester as a library)
- **Reference APIs**: Rust, C#, Go, Node.js, Java, Python, C++, Ruby, PHP
- **Cloud provisioning**: Azure CLI, AWS CLI, GCP CLI (reuse install.sh patterns)
- **Metrics agent**: Lightweight Rust binary collecting CPU/memory/disk on server VMs
- **Browser testing**: Chromium (headless), Firefox (Playwright), Edge (Playwright)
- **Dashboard**: React/TypeScript (new Benchmarks page in AletheDash)
- **Database**: PostgreSQL (new benchmark tables in AletheDash DB)

---

## 1. Reference API Specification

### Endpoints (identical across all languages)

| Endpoint | Behavior |
|---|---|
| `GET /health` | Return `{"status":"ok","runtime":"<name>","version":"<ver>"}` |
| `GET /download/:size` | Stream `size` bytes of random data |
| `POST /upload` | Accept body, return `{"bytes_received": N}` |

### Requirements for all implementations

- Listen on port 8443 (HTTPS) with a shared self-signed certificate
- Support HTTP/1.1 and HTTP/2
- Support TLS 1.2 and 1.3
- Use the language's standard/recommended HTTP library (no heavy frameworks)
- Minimal code — no middleware, no ORM, no application framework
- Include `Dockerfile` + native `build.sh` + `deploy.sh` + `README.md`

### Languages & Runtimes

| # | Language | Runtime | HTTP Library | Notes |
|---|---|---|---|---|
| 1 | Rust | stable | hyper | Existing networker-endpoint |
| 2 | C# | .NET 6 JIT | Kestrel minimal API | LTS |
| 3 | C# | .NET 7 JIT | Kestrel minimal API | |
| 4 | C# | .NET 8 JIT | Kestrel minimal API | Current LTS |
| 5 | C# | .NET 8 AOT | Kestrel minimal API | First production AOT |
| 6 | C# | .NET 9 JIT | Kestrel minimal API | |
| 7 | C# | .NET 9 AOT | Kestrel minimal API | |
| 8 | C# | .NET 10 JIT | Kestrel minimal API | Preview/latest |
| 9 | C# | .NET 10 AOT | Kestrel minimal API | |
| 10 | C# | .NET Framework 4.8 | HttpListener | Windows only |
| 11 | Node.js | v22 LTS | Built-in http2 | No Express |
| 12 | Go | stable | net/http stdlib | |
| 13 | Java | JDK 21 LTS | com.sun.net.httpserver + Virtual Threads | |
| 14 | Python | 3.12 | uvicorn + starlette | Async ASGI |
| 15 | C++ | stable | Boost.Beast | Raw performance baseline |
| 16 | Ruby | 3.3 | Puma (direct Rack) | |
| 17 | PHP | 8.3 | Swoole | Linux only |

---

## 2. Test Matrix

### Server Operating Systems

| OS | Azure | AWS | GCP |
|---|---|---|---|
| Ubuntu 22.04 | Yes | Yes | Yes |
| Ubuntu 24.04 | Yes | Yes | Yes |
| Windows Server 2022 | Yes | Yes | Yes |
| Windows Server 2025 | Yes | Yes | Yes |

### Client Operating Systems

| OS | CLI | Chrome | Firefox | Edge |
|---|---|---|---|---|
| Ubuntu 24.04 | Yes | Yes | Yes | — |
| Windows Server 2022 | Yes | Yes | Yes | Yes |
| Windows 11 Pro | Yes | Yes | Yes | Yes |

### Matrix Size

- **Core run**: 17 runtimes × Ubuntu 24.04 × Azure × Ubuntu CLI client = 34 tests (~30 min)
- **Extended run**: 17 runtimes × 4 OS × Azure × CLI = 68 tests (~1 hour)
- **Full matrix**: 198 server combos × 3 clients × 2 phases × CLI + browsers = ~1,188 tests (~4-6 hours parallelized)

### Tiered Execution

Users choose tier in config. Core run is default. Full matrix on-demand.

---

## 3. Test Methodology

### Two-phase testing per server

**Phase 1 — Cold start:**
1. Start VM (or start stopped VM)
2. Deploy and start the reference API server
3. Wait for `/health` to respond (max 30s)
4. Record cold start time (process start → first 200 response)
5. Run 100 requests immediately
6. Record cold performance metrics

**Phase 2 — Warm steady-state:**
1. Run 5,000 warmup requests (results discarded)
2. Run the full benchmark (configurable, default 50,000 requests)
3. Record warm performance metrics
4. Collect server resource snapshots throughout

**After both phases:**
1. Stop the server process
2. Deallocate (stop) the VM — no cost when idle
3. VM preserved for next run (restart instead of re-provision)

### Concurrency levels

Each benchmark runs at multiple concurrency levels: 1, 10, 50, 100 concurrent connections. This reveals how each runtime handles contention.

---

## 4. Metrics

| Category | Metrics | Source |
|---|---|---|
| **Network (CLI)** | Latency p50/p95/p99, throughput req/s, connection time, TLS handshake time, HTTP version | networker-tester library |
| **Network (Browser)** | TTFB, page load time, connection reuse, TLS session resumption, HTTP version negotiated | Browser modes (Playwright) |
| **Packet analysis** | TCP retransmissions, duplicate ACKs, resets, keep-alive behavior, window scaling | networker-tester capture mode |
| **Server resources** | CPU % (per-core + aggregate), memory RSS/working set, disk I/O, network I/O, open FDs, thread count | Metrics agent on server VM |
| **Capacity** | Max concurrent connections, max sustained req/s before p99 doubles, error rate | Progressive load test |
| **Startup** | Process start → first response (ms), cold vs warm delta | Orchestrator timing |
| **Binary/image size** | Compiled binary size, Docker image size, idle memory before requests | Post-deploy measurement |
| **Cost efficiency** | Estimated $/million requests, $/GB transferred | Cloud pricing API + throughput |

---

## 5. CLI Interface

### Commands

```bash
alethabench run --config benchmark.json              # Full run from config
alethabench run --config benchmark.json --languages rust,go  # Override languages
alethabench run --language rust --os ubuntu-24.04 --cloud azure  # Quick single test
alethabench list                                      # List available runtimes
alethabench results --latest --format json            # View results
alethabench results --latest --format html > report.html
alethabench compare --runs abc123,def456              # Compare runs
```

### Config File (`benchmark.json`)

```json
{
  "name": "Q1 2026 Full Matrix",
  "tier": "full",
  "languages": ["rust", "csharp-net10-aot", "go", "nodejs", "java"],
  "server_os": ["ubuntu-24.04", "windows-server-2022"],
  "client_os": ["ubuntu-24.04", "windows-11"],
  "clouds": ["azure"],
  "browsers": ["chrome", "firefox"],
  "vm_size": "Standard_B2s",
  "test_params": {
    "warmup_requests": 5000,
    "benchmark_requests": 50000,
    "concurrency": [1, 10, 50, 100],
    "timeout_secs": 10,
    "modes": ["http1", "http2", "download", "upload"],
    "capture_packets": true
  },
  "output": {
    "format": ["json", "html"],
    "upload_to_dashboard": true,
    "dashboard_url": "https://alethedash.com"
  }
}
```

### Progress Output (terminal)

Live progress with metrics updated every second during active tests. Shows completion percentage, ETA, per-test summaries as they complete, and live sparklines for the currently running test.

---

## 6. Build Strategy

| Type | Languages | Strategy |
|---|---|---|
| **Cross-compile** | Rust, Go, C# (non-AOT) | Build in GitHub Actions, artifacts in release |
| **Build on target** | C# AOT, C++ | GitHub Actions matrix (Linux runner + Windows runner) |
| **Runtime deploy** | Node.js, Python, Ruby, PHP | Install runtime on VM, copy source |

### Special cases

- .NET Framework 4.8: Windows only (skip Linux)
- PHP Swoole: Linux only (skip Windows)
- C# AOT: separate artifact per OS (must build on matching platform)

---

## 7. Dashboard Integration (AletheDash)

### New page: `/benchmarks`

**Views:**
- **Leaderboard** — ranked table sortable by any metric
- **Comparison** — select 2-4 runtimes, side-by-side charts
- **Matrix heatmap** — languages × OS × cloud grid, color-coded by metric
- **C# deep dive** — .NET version progression, JIT vs AOT, Windows vs Linux
- **Live progress** — real-time streaming during active benchmark runs

### Database tables

```sql
benchmark_run (run_id, name, config, status, started_at, finished_at, tier)
benchmark_result (result_id, run_id, language, runtime, server_os, client_os, cloud,
                  phase, concurrency, metrics_json, resource_json, packet_json,
                  browser, started_at, finished_at)
benchmark_comparison (comparison_id, run_ids[], created_at, created_by)
```

### API endpoints

```
GET  /api/benchmarks                     — list runs
GET  /api/benchmarks/:runId              — full results
GET  /api/benchmarks/:runId/compare      — comparison data
GET  /api/benchmarks/latest/leaderboard  — current rankings
POST /api/benchmarks                     — upload results from CLI
```

Public access (no auth) for leaderboard and comparison. Auth required for upload.

---

## 8. Project Structure

```
benchmarks/
  reference-apis/
    rust/                  (symlink → crates/networker-endpoint)
    csharp-net6/
    csharp-net7/
    csharp-net8/
    csharp-net8-aot/
    csharp-net9/
    csharp-net9-aot/
    csharp-net10/
    csharp-net10-aot/
    csharp-net48/
    nodejs/
    go/
    java/
    python/
    cpp/
    ruby/
    php/
  orchestrator/            (alethabench Rust crate)
    src/
      main.rs
      config.rs
      provisioner.rs       (VM lifecycle per cloud)
      deployer.rs          (deploy reference APIs)
      runner.rs            (execute benchmark runs)
      collector.rs         (gather metrics)
      reporter.rs          (generate HTML/JSON/PDF reports)
      cost.rs              (cloud pricing calculations)
  metrics-agent/           (lightweight Rust binary for server VMs)
    src/
      main.rs              (collects CPU/memory/disk, serves on :9100)
  dashboard/               (AletheDash integration)
    src/
      pages/BenchmarksPage.tsx
      components/Leaderboard.tsx
      components/ComparisonChart.tsx
      components/MatrixHeatmap.tsx
      components/BenchmarkProgress.tsx
```

---

## 9. Implementation Phases

### Phase 1: Foundation (MVP)
- `alethabench` CLI skeleton (config parsing, progress output)
- Reference APIs: Rust (existing), C# .NET 10, C# .NET 10 AOT, Go, Node.js
- Metrics agent (CPU, memory)
- Single cloud (Azure), single OS (Ubuntu 24.04), CLI client only
- Core metrics: latency, throughput, CPU, memory, startup time
- JSON + HTML report output
- VM lifecycle: provision → deploy → cold → warm → stop

### Phase 2: Full Language Matrix
- Remaining C# versions (.NET 6/7/8/9 JIT + AOT variants + .NET 4.8)
- Java, Python, C++, Ruby, PHP implementations
- Windows Server 2022 + 2025 server OS
- Windows 11 + Windows Server client OS
- Packet capture analysis
- Binary size + idle memory metrics

### Phase 3: Multi-Cloud + Browsers
- AWS and GCP cloud provisioning
- Browser tests (Chrome, Firefox, Edge via Playwright)
- Capacity testing (progressive load to find breaking point)
- Cost efficiency calculations
- Ubuntu 22.04 server OS

### Phase 4: Dashboard Integration
- AletheDash Benchmarks page (leaderboard, comparison, matrix, C# deep dive)
- Real-time progress streaming via WebSocket
- Public JSON API
- PDF report generation
- Historical run-over-run comparison

### Phase 5: User-Configurable
- Custom benchmark configs via dashboard UI
- Scheduled benchmark runs (weekly/monthly cron)
- Custom reference API upload (test your own implementation)
- CI/CD integration (GitHub Action for alethabench)

---

## 10. Non-Goals (deferred)

- gRPC/WebSocket protocol benchmarks (HTTP only in Phase 1)
- Database-backed CRUD API comparison (Phase 2 of the product, after pure network stack)
- ARM architecture (x86_64 only initially)
- Container orchestration benchmarks (Kubernetes, Docker Swarm)
- Client-side framework comparison (React/Vue/Angular rendering)
