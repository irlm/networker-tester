# AletheBench Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **PARALLELISM:** Tasks 2-10 (reference API implementations) are fully independent and SHOULD be dispatched as parallel agents. Task 1 (orchestrator skeleton) and Task 11 (metrics agent) are also independent of each other and of Tasks 2-10. Task 12 (integration) depends on all previous tasks.

**Goal:** Build the AletheBench MVP — a CLI tool that benchmarks the same HTTP API implemented in 5 languages (Rust, C# .NET 10, C# .NET 10 AOT, Go, Node.js) on Azure Ubuntu 24.04, collecting latency, throughput, CPU, memory, startup time, and binary size metrics.

**Architecture:** `alethabench` Rust CLI reads a JSON config, provisions Azure VMs, deploys reference API implementations, runs networker-tester against each, collects server-side metrics via a lightweight agent, and outputs JSON + HTML reports. VMs are stopped (not deleted) after each run.

**Tech Stack:** Rust (alethabench CLI + metrics-agent), C# (.NET 10), Go, Node.js, Azure CLI, networker-tester (as library), serde/serde_json, clap, tokio

**Spec:** `docs/superpowers/specs/2026-03-25-alethabench-cross-platform-benchmark-design.md`

---

## Dependency Graph

```
Tasks 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11 → all independent, run in parallel
Task 12 (integration + end-to-end test) → depends on all above
Task 13 (HTML report) → depends on Task 12
Task 14 (validation + dry-run) → depends on Task 12
```

---

## Task 1: Orchestrator CLI Skeleton (alethabench)

**Files:**
- Create: `benchmarks/orchestrator/Cargo.toml`
- Create: `benchmarks/orchestrator/src/main.rs`
- Create: `benchmarks/orchestrator/src/config.rs`
- Create: `benchmarks/orchestrator/src/types.rs`
- Create: `benchmarks/orchestrator/src/progress.rs`
- Create: `benchmarks/orchestrator/src/provisioner.rs`
- Create: `benchmarks/orchestrator/src/deployer.rs`
- Create: `benchmarks/orchestrator/src/runner.rs`
- Create: `benchmarks/orchestrator/src/collector.rs`
- Create: `benchmarks/orchestrator/src/reporter.rs`
- Create: `benchmarks/orchestrator/src/cost.rs`
- Create: `benchmarks/sample-benchmark.json`

**This task builds the shell — no actual cloud provisioning yet, just the structure and config parsing.**

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "alethabench"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "alethabench"
path = "src/main.rs"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
anyhow = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4", "serde"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

- [ ] **Step 2: Create `config.rs` — benchmark config parsing**

Parse the `benchmark.json` config file. Define all config structs:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_tier")]
    pub tier: String,  // "core", "extended", "full"
    pub languages: Vec<String>,
    #[serde(default = "default_server_os")]
    pub server_os: Vec<String>,
    #[serde(default = "default_client_os")]
    pub client_os: Vec<String>,
    #[serde(default = "default_clouds")]
    pub clouds: Vec<String>,
    #[serde(default)]
    pub browsers: Vec<String>,
    #[serde(default = "default_vm_size")]
    pub vm_size: String,
    #[serde(default)]
    pub test_params: TestParams,
    #[serde(default)]
    pub output: OutputConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestParams {
    #[serde(default = "default_warmup")]
    pub warmup_requests: u64,
    #[serde(default = "default_benchmark")]
    pub benchmark_requests: u64,
    #[serde(default = "default_concurrency")]
    pub concurrency: Vec<u32>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_modes")]
    pub modes: Vec<String>,
    #[serde(default)]
    pub capture_packets: bool,
    #[serde(default = "default_repeat")]
    pub repeat: u32,  // number of times to repeat each test (min 3 for stats)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    #[serde(default = "default_formats")]
    pub format: Vec<String>,  // "json", "html", "pdf"
    #[serde(default)]
    pub upload_to_dashboard: bool,
    #[serde(default)]
    pub dashboard_url: Option<String>,
    #[serde(default)]
    pub output_dir: Option<String>,
}

// Default functions
fn default_tier() -> String { "core".into() }
fn default_server_os() -> Vec<String> { vec!["ubuntu-24.04".into()] }
fn default_client_os() -> Vec<String> { vec!["ubuntu-24.04".into()] }
fn default_clouds() -> Vec<String> { vec!["azure".into()] }
fn default_vm_size() -> String { "Standard_B2s".into() }
fn default_warmup() -> u64 { 5000 }
fn default_benchmark() -> u64 { 50000 }
fn default_concurrency() -> Vec<u32> { vec![1, 10, 50, 100] }
fn default_timeout() -> u64 { 10 }
fn default_modes() -> Vec<String> { vec!["http1".into(), "http2".into()] }
fn default_repeat() -> u32 { 3 }
fn default_formats() -> Vec<String> { vec!["json".into()] }

impl BenchmarkConfig {
    pub fn from_file(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Generate the test matrix from config
    pub fn test_matrix(&self) -> Vec<TestCase> {
        let mut cases = vec![];
        for lang in &self.languages {
            for os in &self.server_os {
                // Skip invalid combinations
                if lang == "csharp-net48" && !os.contains("windows") { continue; }
                if lang == "php" && os.contains("windows") { continue; }
                for cloud in &self.clouds {
                    for client_os in &self.client_os {
                        cases.push(TestCase {
                            language: lang.clone(),
                            server_os: os.clone(),
                            client_os: client_os.clone(),
                            cloud: cloud.clone(),
                        });
                    }
                }
            }
        }
        cases
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    pub language: String,
    pub server_os: String,
    pub client_os: String,
    pub cloud: String,
}
```

Add unit tests for config parsing and matrix generation.

- [ ] **Step 3: Create `types.rs` — result types**

Define all result/metric structs used across the orchestrator:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRun {
    pub run_id: Uuid,
    pub name: String,
    pub config: serde_json::Value,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,  // "running", "completed", "failed"
    pub results: Vec<BenchmarkResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    pub result_id: Uuid,
    pub language: String,
    pub runtime: String,
    pub server_os: String,
    pub client_os: String,
    pub cloud: String,
    pub phase: String,  // "cold", "warm"
    pub concurrency: u32,
    pub network_metrics: NetworkMetrics,
    pub resource_metrics: ResourceMetrics,
    pub startup_metrics: StartupMetrics,
    pub binary_metrics: BinaryMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkMetrics {
    pub requests_total: u64,
    pub requests_per_second: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_p99_ms: f64,
    pub latency_mean_ms: f64,
    pub latency_stddev_ms: f64,
    pub connection_time_ms: f64,
    pub tls_handshake_ms: f64,
    pub http_version: String,
    pub errors: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceMetrics {
    pub cpu_percent_avg: f64,
    pub cpu_percent_peak: f64,
    pub memory_rss_bytes: u64,
    pub memory_peak_bytes: u64,
    pub thread_count: u32,
    pub open_fds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StartupMetrics {
    pub cold_start_ms: f64,
    pub cold_warm_delta_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BinaryMetrics {
    pub binary_size_bytes: u64,
    pub idle_memory_bytes: u64,
}
```

- [ ] **Step 4: Create `progress.rs` — terminal progress display**

Simple progress reporting to stdout (not TUI yet):

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub struct Progress {
    total: usize,
    completed: Arc<AtomicUsize>,
    start: std::time::Instant,
}

impl Progress {
    pub fn new(total: usize) -> Self { ... }
    pub fn tick(&self, label: &str, status: &str) { ... }
    pub fn complete(&self, label: &str, summary: &str) { ... }
    pub fn fail(&self, label: &str, error: &str) { ... }
    pub fn finish(&self) { ... }
}
```

Print format:
```
[3/17] ✓ Rust / Ubuntu 24.04 / Azure     12,400 req/s  p99=1.2ms  CPU=23%  MEM=18MB
[4/17] ⟳ C# .NET 10 / Ubuntu 24.04 / Azure  running warm phase...
```

- [ ] **Step 5: Create stub modules for provisioner, deployer, runner, collector, reporter, cost**

Each as a minimal file with TODO implementations:

`provisioner.rs` — `pub async fn provision_vm(cloud, os, vm_size) -> Result<VmInfo>`
`deployer.rs` — `pub async fn deploy_api(vm: &VmInfo, language: &str) -> Result<()>`
`runner.rs` — `pub async fn run_benchmark(vm: &VmInfo, params: &TestParams) -> Result<NetworkMetrics>`
`collector.rs` — `pub async fn collect_metrics(vm: &VmInfo) -> Result<ResourceMetrics>`
`reporter.rs` — `pub fn generate_json(run: &BenchmarkRun) -> Result<String>` and `pub fn generate_html(run: &BenchmarkRun) -> Result<String>`
`cost.rs` — `pub fn estimate_cost(cloud: &str, vm_size: &str, duration_secs: u64) -> f64`

- [ ] **Step 6: Create `main.rs` with clap CLI**

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "alethabench", about = "Cross-platform HTTP benchmark suite")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a benchmark from config file
    Run {
        #[arg(short, long)]
        config: String,
        #[arg(long)]
        languages: Option<String>,  // comma-separated override
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        quick: bool,
    },
    /// List available languages and runtimes
    List,
    /// View results
    Results {
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        latest: bool,
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Compare two runs
    Compare {
        #[arg(long)]
        runs: String,  // comma-separated run IDs
    },
}
```

Implement `Run` command: parse config, generate matrix, print dry-run if `--dry-run`, else iterate matrix and call stub functions.

- [ ] **Step 7: Create sample `benchmark.json`**

```json
{
  "name": "Phase 1 Core Benchmark",
  "tier": "core",
  "languages": ["rust", "csharp-net10", "csharp-net10-aot", "go", "nodejs"],
  "server_os": ["ubuntu-24.04"],
  "client_os": ["ubuntu-24.04"],
  "clouds": ["azure"],
  "test_params": {
    "warmup_requests": 5000,
    "benchmark_requests": 50000,
    "concurrency": [1, 10, 50, 100],
    "timeout_secs": 10,
    "modes": ["http1", "http2", "download", "upload"],
    "repeat": 3
  },
  "output": {
    "format": ["json", "html"]
  }
}
```

- [ ] **Step 8: Build + test**

```bash
cd benchmarks/orchestrator
cargo build
cargo test
./target/debug/alethabench list
./target/debug/alethabench run --config ../sample-benchmark.json --dry-run
```

- [ ] **Step 9: Commit**

---

## Tasks 2-6: Reference API Implementations (PARALLEL)

**These 5 tasks are fully independent and should be dispatched as parallel agents.**

Each implementation MUST:
- Listen on port 8443 HTTPS with a self-signed cert (provided at `benchmarks/shared/cert.pem` + `key.pem`)
- Implement exactly 3 endpoints: `GET /health`, `GET /download/:size`, `POST /upload`
- `/health` returns `{"status":"ok","runtime":"<name>","version":"<ver>"}`
- `/download/:size` streams `size` bytes (use repeating byte pattern, not random)
- `/upload` reads full body, returns `{"bytes_received": N}`
- Support HTTP/1.1 and HTTP/2, TLS 1.2 and 1.3
- Include: source code, `build.sh`, `deploy.sh`, `Dockerfile`, `README.md`

### Task 2: Shared Assets + Rust Reference (existing)

**Files:**
- Create: `benchmarks/shared/cert.pem` (self-signed)
- Create: `benchmarks/shared/key.pem`
- Create: `benchmarks/shared/generate-cert.sh`
- Create: `benchmarks/reference-apis/rust/README.md`
- Create: `benchmarks/reference-apis/rust/deploy.sh`

- [ ] **Step 1: Generate shared self-signed cert**

```bash
mkdir -p benchmarks/shared
openssl req -x509 -newkey rsa:2048 -keyout benchmarks/shared/key.pem \
  -out benchmarks/shared/cert.pem -days 3650 -nodes \
  -subj "/CN=alethabench"
```

- [ ] **Step 2: Create Rust reference (symlink to networker-endpoint)**

```bash
cd benchmarks/reference-apis
ln -s ../../crates/networker-endpoint rust
```

Write `README.md` documenting: why hyper (raw performance, no framework), how to build, expected binary size.

- [ ] **Step 3: Create `deploy.sh` for Rust**

Script that copies the pre-built binary to a target VM and starts it:

```bash
#!/bin/bash
set -e
VM_IP=$1
scp /path/to/networker-endpoint $VM_IP:/opt/bench/server
scp benchmarks/shared/cert.pem $VM_IP:/opt/bench/
scp benchmarks/shared/key.pem $VM_IP:/opt/bench/
ssh $VM_IP "cd /opt/bench && nohup ./server --cert cert.pem --key key.pem --port 8443 > server.log 2>&1 &"
```

- [ ] **Step 4: Commit**

---

### Task 3: C# .NET 10 Reference API

**Files:**
- Create: `benchmarks/reference-apis/csharp-net10/Program.cs`
- Create: `benchmarks/reference-apis/csharp-net10/csharp-net10.csproj`
- Create: `benchmarks/reference-apis/csharp-net10/build.sh`
- Create: `benchmarks/reference-apis/csharp-net10/deploy.sh`
- Create: `benchmarks/reference-apis/csharp-net10/Dockerfile`
- Create: `benchmarks/reference-apis/csharp-net10/README.md`

- [ ] **Step 1: Create minimal Kestrel API**

Single-file `Program.cs` using .NET 10 minimal API with Kestrel:

```csharp
using System.Security.Cryptography.X509Certificates;
using Microsoft.AspNetCore.Server.Kestrel.Core;

var builder = WebApplication.CreateBuilder(args);
builder.WebHost.ConfigureKestrel(options =>
{
    options.ListenAnyIP(8443, listenOptions =>
    {
        listenOptions.UseHttps("/opt/bench/cert.pem", "/opt/bench/key.pem");
        listenOptions.Protocols = HttpProtocols.Http1AndHttp2;
    });
});

var app = builder.Build();

app.MapGet("/health", () => Results.Json(new {
    status = "ok",
    runtime = "csharp-net10",
    version = Environment.Version.ToString()
}));

app.MapGet("/download/{size:long}", (long size) =>
{
    return Results.Stream(async stream =>
    {
        var buffer = new byte[8192];
        Array.Fill(buffer, (byte)0x42);
        long remaining = size;
        while (remaining > 0)
        {
            int chunk = (int)Math.Min(remaining, buffer.Length);
            await stream.WriteAsync(buffer.AsMemory(0, chunk));
            remaining -= chunk;
        }
    }, "application/octet-stream");
});

app.MapPost("/upload", async (HttpRequest req) =>
{
    long total = 0;
    var buffer = new byte[8192];
    int read;
    while ((read = await req.Body.ReadAsync(buffer)) > 0)
        total += read;
    return Results.Json(new { bytes_received = total });
});

app.Run();
```

- [ ] **Step 2: Create `.csproj`**

```xml
<Project Sdk="Microsoft.NET.Sdk.Web">
  <PropertyGroup>
    <TargetFramework>net10.0</TargetFramework>
  </PropertyGroup>
</Project>
```

- [ ] **Step 3: Create `build.sh`, `deploy.sh`, `Dockerfile`, `README.md`**

`build.sh`: `dotnet publish -c Release -r linux-x64 --self-contained -o ./publish`
`deploy.sh`: scp publish dir + cert + start
`README.md`: why Kestrel minimal API, expected binary size, .NET version

- [ ] **Step 4: Test locally** (if .NET 10 SDK available)

- [ ] **Step 5: Commit**

---

### Task 4: C# .NET 10 AOT Reference API

**Files:**
- Create: `benchmarks/reference-apis/csharp-net10-aot/Program.cs`
- Create: `benchmarks/reference-apis/csharp-net10-aot/csharp-net10-aot.csproj`
- Create: `benchmarks/reference-apis/csharp-net10-aot/build.sh`
- Create: `benchmarks/reference-apis/csharp-net10-aot/deploy.sh`
- Create: `benchmarks/reference-apis/csharp-net10-aot/Dockerfile`
- Create: `benchmarks/reference-apis/csharp-net10-aot/README.md`

- [ ] **Step 1: Create AOT-compatible Kestrel API**

Same as Task 3 `Program.cs` but with AOT-compatible JSON serializer:

```csharp
// Add at top:
using System.Text.Json.Serialization;

[JsonSerializable(typeof(HealthResponse))]
[JsonSerializable(typeof(UploadResponse))]
internal partial class AppJsonContext : JsonSerializerContext { }

record HealthResponse(string status, string runtime, string version);
record UploadResponse(long bytes_received);

// Use typed results instead of anonymous objects
app.MapGet("/health", () => Results.Json(
    new HealthResponse("ok", "csharp-net10-aot", Environment.Version.ToString()),
    AppJsonContext.Default.HealthResponse));
```

- [ ] **Step 2: Create `.csproj` with AOT enabled**

```xml
<Project Sdk="Microsoft.NET.Sdk.Web">
  <PropertyGroup>
    <TargetFramework>net10.0</TargetFramework>
    <PublishAot>true</PublishAot>
    <InvariantGlobalization>true</InvariantGlobalization>
  </PropertyGroup>
</Project>
```

- [ ] **Step 3: Create `build.sh` (must build on matching OS), `deploy.sh`, `Dockerfile`, `README.md`**

`build.sh`: `dotnet publish -c Release -r linux-x64 -o ./publish` (AOT is automatic from csproj)

Note in README: AOT must be built on the target OS. Cross-compilation is NOT supported for AOT.

- [ ] **Step 4: Commit**

---

### Task 5: Go Reference API

**Files:**
- Create: `benchmarks/reference-apis/go/main.go`
- Create: `benchmarks/reference-apis/go/go.mod`
- Create: `benchmarks/reference-apis/go/build.sh`
- Create: `benchmarks/reference-apis/go/deploy.sh`
- Create: `benchmarks/reference-apis/go/Dockerfile`
- Create: `benchmarks/reference-apis/go/README.md`

- [ ] **Step 1: Create Go HTTP server using stdlib**

```go
package main

import (
    "crypto/tls"
    "encoding/json"
    "fmt"
    "io"
    "net/http"
    "runtime"
    "strconv"
    "strings"
)

func main() {
    mux := http.NewServeMux()

    mux.HandleFunc("GET /health", func(w http.ResponseWriter, r *http.Request) {
        w.Header().Set("Content-Type", "application/json")
        json.NewEncoder(w).Encode(map[string]string{
            "status":  "ok",
            "runtime": "go",
            "version": runtime.Version(),
        })
    })

    mux.HandleFunc("GET /download/{size}", func(w http.ResponseWriter, r *http.Request) {
        size, _ := strconv.ParseInt(r.PathValue("size"), 10, 64)
        w.Header().Set("Content-Type", "application/octet-stream")
        buf := make([]byte, 8192)
        for i := range buf { buf[i] = 0x42 }
        remaining := size
        for remaining > 0 {
            chunk := int64(len(buf))
            if remaining < chunk { chunk = remaining }
            w.Write(buf[:chunk])
            remaining -= chunk
        }
    })

    mux.HandleFunc("POST /upload", func(w http.ResponseWriter, r *http.Request) {
        n, _ := io.Copy(io.Discard, r.Body)
        w.Header().Set("Content-Type", "application/json")
        fmt.Fprintf(w, `{"bytes_received":%d}`, n)
    })

    server := &http.Server{
        Addr:    ":8443",
        Handler: mux,
        TLSConfig: &tls.Config{
            MinVersion: tls.VersionTLS12,
        },
    }
    server.ListenAndServeTLS("/opt/bench/cert.pem", "/opt/bench/key.pem")
}
```

- [ ] **Step 2: Create `go.mod`, `build.sh` (cross-compiles), `deploy.sh`, `Dockerfile`, `README.md`**

`build.sh`: `GOOS=linux GOARCH=amd64 go build -o server .`

- [ ] **Step 3: Commit**

---

### Task 6: Node.js Reference API

**Files:**
- Create: `benchmarks/reference-apis/nodejs/server.js`
- Create: `benchmarks/reference-apis/nodejs/package.json`
- Create: `benchmarks/reference-apis/nodejs/deploy.sh`
- Create: `benchmarks/reference-apis/nodejs/Dockerfile`
- Create: `benchmarks/reference-apis/nodejs/README.md`

- [ ] **Step 1: Create Node.js HTTP/2 server using built-in modules**

```javascript
const http2 = require('node:http2');
const fs = require('node:fs');
const path = require('node:path');

const server = http2.createSecureServer({
  key: fs.readFileSync('/opt/bench/key.pem'),
  cert: fs.readFileSync('/opt/bench/cert.pem'),
  allowHTTP1: true,
});

const FILL_BUF = Buffer.alloc(8192, 0x42);

server.on('request', (req, res) => {
  const url = new URL(req.url, `https://${req.headers.host}`);

  if (req.method === 'GET' && url.pathname === '/health') {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({
      status: 'ok',
      runtime: 'nodejs',
      version: process.version,
    }));
    return;
  }

  const downloadMatch = url.pathname.match(/^\/download\/(\d+)$/);
  if (req.method === 'GET' && downloadMatch) {
    let remaining = parseInt(downloadMatch[1], 10);
    res.writeHead(200, { 'content-type': 'application/octet-stream' });
    function writeChunk() {
      while (remaining > 0) {
        const chunk = Math.min(remaining, FILL_BUF.length);
        const ok = res.write(FILL_BUF.subarray(0, chunk));
        remaining -= chunk;
        if (!ok) { res.once('drain', writeChunk); return; }
      }
      res.end();
    }
    writeChunk();
    return;
  }

  if (req.method === 'POST' && url.pathname === '/upload') {
    let total = 0;
    req.on('data', chunk => { total += chunk.length; });
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ bytes_received: total }));
    });
    return;
  }

  res.writeHead(404);
  res.end('Not Found');
});

server.listen(8443, () => {
  console.log('Node.js benchmark server listening on :8443');
});
```

- [ ] **Step 2: Create `package.json`, `deploy.sh`, `Dockerfile`, `README.md`**

`deploy.sh`: scp source files + install node on VM + start with `node server.js`

- [ ] **Step 3: Commit**

---

### Task 7: Java Reference API

**Files:**
- Create: `benchmarks/reference-apis/java/Server.java`
- Create: `benchmarks/reference-apis/java/build.sh`
- Create: `benchmarks/reference-apis/java/deploy.sh`
- Create: `benchmarks/reference-apis/java/Dockerfile`
- Create: `benchmarks/reference-apis/java/README.md`

- [ ] **Step 1: Create Java HTTP server using JDK 21 built-in + Virtual Threads**

Single-file Java server using `com.sun.net.httpserver` with Virtual Threads executor. Support HTTPS via SSLContext with the shared cert. Implement all 3 endpoints.

README notes: using JDK built-in HTTP server (no Tomcat/Netty/Spring), Virtual Threads for concurrency.

- [ ] **Step 2: Create `build.sh`, `deploy.sh`, `Dockerfile`, `README.md`**

`build.sh`: `javac Server.java` + package as runnable JAR
`deploy.sh`: scp JAR + install JDK 21 on VM + start

- [ ] **Step 3: Commit**

---

### Task 8: Python Reference API

**Files:**
- Create: `benchmarks/reference-apis/python/server.py`
- Create: `benchmarks/reference-apis/python/requirements.txt`
- Create: `benchmarks/reference-apis/python/deploy.sh`
- Create: `benchmarks/reference-apis/python/Dockerfile`
- Create: `benchmarks/reference-apis/python/README.md`

- [ ] **Step 1: Create Python ASGI server with uvicorn + starlette**

Minimal starlette app with the 3 endpoints. Use `StreamingResponse` for download. uvicorn serves with SSL.

README notes: why uvicorn+starlette (fastest production-ready Python async option), not pure stdlib.

- [ ] **Step 2: Create `requirements.txt`, `deploy.sh`, `Dockerfile`, `README.md`**

- [ ] **Step 3: Commit**

---

### Task 9: C++ Reference API

**Files:**
- Create: `benchmarks/reference-apis/cpp/server.cpp`
- Create: `benchmarks/reference-apis/cpp/CMakeLists.txt`
- Create: `benchmarks/reference-apis/cpp/build.sh`
- Create: `benchmarks/reference-apis/cpp/deploy.sh`
- Create: `benchmarks/reference-apis/cpp/Dockerfile`
- Create: `benchmarks/reference-apis/cpp/README.md`

- [ ] **Step 1: Create C++ server using Boost.Beast**

Implement all 3 endpoints with Boost.Beast + Boost.Asio SSL. Use async operations.

README notes: raw performance baseline, no framework overhead, compiled with `-O3`.

- [ ] **Step 2: Create `CMakeLists.txt`, `build.sh` (must build on target), `deploy.sh`, `Dockerfile`, `README.md`**

- [ ] **Step 3: Commit**

---

### Task 10: Nginx Static Baseline

**Files:**
- Create: `benchmarks/reference-apis/nginx/nginx.conf`
- Create: `benchmarks/reference-apis/nginx/health.json`
- Create: `benchmarks/reference-apis/nginx/generate-download-files.sh`
- Create: `benchmarks/reference-apis/nginx/deploy.sh`
- Create: `benchmarks/reference-apis/nginx/README.md`

- [ ] **Step 1: Create nginx config serving static equivalents**

Static `/health` JSON file, pre-generated download files, upload via nginx upload module or `client_body_in_file_only`. This is the theoretical maximum — pure network stack, zero application code.

- [ ] **Step 2: Create `deploy.sh`, `README.md`**

- [ ] **Step 3: Commit**

---

## Task 11: Metrics Agent

**Files:**
- Create: `benchmarks/metrics-agent/Cargo.toml`
- Create: `benchmarks/metrics-agent/src/main.rs`

**Independent of all other tasks. Can run in parallel.**

- [ ] **Step 1: Create lightweight Rust binary**

Collects CPU%, memory RSS, disk I/O, thread count, open FDs every second. Serves metrics on `:9100/metrics` as JSON. Uses `sysinfo` crate.

```rust
// GET /metrics returns:
{
    "cpu_percent": 45.2,
    "memory_rss_bytes": 18874368,
    "memory_peak_bytes": 22020096,
    "thread_count": 4,
    "open_fds": 23,
    "disk_read_bytes_sec": 0,
    "disk_write_bytes_sec": 1024,
    "net_rx_bytes_sec": 524288,
    "net_tx_bytes_sec": 1048576,
    "uptime_secs": 42
}

// GET /metrics/process/:pid returns same but for a specific process (the server under test)
```

- [ ] **Step 2: Build + test**

```bash
cd benchmarks/metrics-agent
cargo build
cargo test
```

- [ ] **Step 3: Commit**

---

## Task 12: Azure Provisioner + Deployer + Runner Integration

**Depends on:** Tasks 1-11

**Files:**
- Modify: `benchmarks/orchestrator/src/provisioner.rs`
- Modify: `benchmarks/orchestrator/src/deployer.rs`
- Modify: `benchmarks/orchestrator/src/runner.rs`
- Modify: `benchmarks/orchestrator/src/collector.rs`

- [ ] **Step 1: Implement Azure provisioner**

Use `az vm create` via `tokio::process::Command`. Pin image URN for Ubuntu 24.04. Create resource group `alethabench-rg`. Tag VMs with language+os. Support start/stop/delete.

- [ ] **Step 2: Implement deployer**

For each language, run the corresponding `deploy.sh` via SSH. Install metrics-agent. Verify `/health` responds within 30s.

- [ ] **Step 3: Implement runner**

Call networker-tester as a library (or shell out to the binary). Run cold phase (100 req), warmup (5000 req), warm phase (benchmark_requests). Collect results per concurrency level. Repeat `test_params.repeat` times.

- [ ] **Step 4: Implement collector**

Poll metrics-agent at `:9100/metrics` every second during test. Aggregate CPU/memory averages and peaks. Measure binary size and idle memory before first request.

- [ ] **Step 5: End-to-end test**

Run `alethabench run --config sample-benchmark.json --quick` against a single language (Rust) on Azure. Verify JSON output has all expected fields.

- [ ] **Step 6: Commit**

---

## Task 13: HTML Report Generator

**Depends on:** Task 12

**Files:**
- Modify: `benchmarks/orchestrator/src/reporter.rs`
- Create: `benchmarks/orchestrator/src/report_template.html`

- [ ] **Step 1: Implement HTML report**

Generate a standalone HTML file (inline CSS/JS, no external dependencies) with:
- Executive summary (top 3 languages by throughput, lowest latency, lowest memory)
- Leaderboard table (sortable by any column)
- Latency comparison bar chart (p50/p95/p99 per language)
- Resource usage comparison (CPU + memory)
- Cold vs warm performance chart
- Methodology section (VM specs, test parameters, config)

Use the same report style as networker-tester's HTML reports (dark theme, monospace).

- [ ] **Step 2: Test with sample data**

- [ ] **Step 3: Commit**

---

## Task 14: Validation + Dry-Run + Quick Mode

**Depends on:** Task 12

**Files:**
- Modify: `benchmarks/orchestrator/src/main.rs`
- Create: `benchmarks/orchestrator/src/validator.rs`

- [ ] **Step 1: Implement API validation**

After deploying each server, run validation:
- `GET /health` returns valid JSON with `status: "ok"`
- `GET /download/1024` returns exactly 1024 bytes
- `POST /upload` with 1024-byte body returns `{"bytes_received": 1024}`

If validation fails, mark the language as "failed" and continue with the next.

- [ ] **Step 2: Implement `--dry-run`**

Print test matrix, estimated VM count, estimated time per tier, estimated cloud cost. No VMs provisioned.

- [ ] **Step 3: Implement `--quick` mode**

Override test params: 1000 requests, concurrency [1, 10] only, no packet capture, repeat 1. For fast iteration.

- [ ] **Step 4: Commit**

---

## Summary: Parallel Execution Map

```
Time →

Agent 1:  [Task 1: Orchestrator CLI skeleton        ]
Agent 2:  [Task 2: Shared assets + Rust reference    ]
Agent 3:  [Task 3: C# .NET 10 reference              ]
Agent 4:  [Task 4: C# .NET 10 AOT reference          ]
Agent 5:  [Task 5: Go reference                      ]
Agent 6:  [Task 6: Node.js reference                 ]
Agent 7:  [Task 7: Java reference                    ]
Agent 8:  [Task 8: Python reference                  ]
Agent 9:  [Task 9: C++ reference                     ]
Agent 10: [Task 10: Nginx baseline                   ]
Agent 11: [Task 11: Metrics agent                    ]
                                                      ↓ all complete
Agent 12:           [Task 12: Integration             ]
                                                       ↓
Agents 13+14:       [Task 13: HTML report] [Task 14: Validation]
```

**11 agents run in parallel** for the first wave, then 1 integration agent, then 2 more in parallel. Total: ~14 agent dispatches, but wall-clock time is dominated by the longest of the 11 parallel tasks.
