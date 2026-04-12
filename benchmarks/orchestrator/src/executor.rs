use crate::callback::CallbackClient;
use crate::config::{DashboardBenchmarkConfig, MethodologyConfig, TestbedConfig};
use crate::deployer;
use crate::provisioner::{self, VmInfo};
use crate::runner;
use crate::ssh;
use crate::tester_state::{self, AcquireOutcome};
use anyhow::{Context, Result};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_postgres::{Client as PgClient, NoTls};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// DB plumbing helpers for the persistent-tester lock flow.
//
// The orchestrator historically had no direct DB access — it only spoke to
// the dashboard via HTTP callbacks. Task 23 introduces direct Postgres access
// so the orchestrator can participate in the tester-lock protocol without a
// round-trip-heavy callback API.
//
// For MVP we lazily construct a single short-lived connection inside
// `execute_testbed_application` by reading `ORCHESTRATOR_DB_URL` (fallback
// `DASHBOARD_DB_URL`). A top-down `Arc<Client>` is the cleaner end state but
// would touch far more files; see the persistent-testers plan for the
// follow-up refactor.
// ---------------------------------------------------------------------------

/// Lazily connect to Postgres using `ORCHESTRATOR_DB_URL` or `DASHBOARD_DB_URL`.
/// The spawned background task drives the connection to completion; callers
/// keep the returned `Client` for the duration of the work.
async fn connect_orchestrator_db() -> Result<Arc<PgClient>> {
    let url = std::env::var("ORCHESTRATOR_DB_URL")
        .or_else(|_| std::env::var("DASHBOARD_DB_URL"))
        .context(
            "ORCHESTRATOR_DB_URL (or DASHBOARD_DB_URL) must be set for tester-lock flow",
        )?;
    let (client, conn) = tokio_postgres::connect(&url, NoTls)
        .await
        .context("connecting to Postgres for tester-lock flow")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::error!("orchestrator Postgres connection error: {e:#}");
        }
    });
    Ok(Arc::new(client))
}

/// Update `benchmark_config.current_phase` — a lightweight progress marker
/// consumed by the dashboard's phase-update WebSocket hub (future task).
async fn set_phase(client: &PgClient, config_id: &Uuid, phase: &str) -> Result<()> {
    client
        .execute(
            "UPDATE benchmark_config SET current_phase = $2, updated_at = NOW() \
             WHERE config_id = $1",
            &[config_id, &phase],
        )
        .await
        .with_context(|| format!("set_phase({phase}) for config {config_id} failed"))?;
    Ok(())
}

/// Update `benchmark_config.status`. When transitioning into `queued`, also
/// stamp `queued_at = NOW()` so the dispatcher's fairness ordering is correct.
async fn set_benchmark_status(
    client: &PgClient,
    config_id: &Uuid,
    status: &str,
) -> Result<()> {
    if status == "queued" {
        client
            .execute(
                "UPDATE benchmark_config \
                    SET status = 'queued', queued_at = NOW(), updated_at = NOW() \
                  WHERE config_id = $1",
                &[config_id],
            )
            .await
            .with_context(|| format!("set status=queued for config {config_id}"))?;
    } else {
        client
            .execute(
                "UPDATE benchmark_config SET status = $2, updated_at = NOW() \
                 WHERE config_id = $1",
                &[config_id, &status],
            )
            .await
            .with_context(|| format!("set status={status} for config {config_id}"))?;
    }
    Ok(())
}

/// TODO(Task 10 integration): push a `promote_next` event to the tester
/// dispatcher (a separate dashboard process). For MVP this is a tracing-only
/// stub — the dispatcher's periodic sweep (every 30s) will notice any dropped
/// events and still make forward progress.
async fn notify_queue_dispatcher(tester_id: &Uuid) {
    tracing::info!(
        tester_id = %tester_id,
        "notify_queue_dispatcher stub — dispatcher sweep will promote next queued config"
    );
}

/// Drop-safe guard that releases the tester lock on scope exit. The preferred
/// path is `release_now().await` which synchronously releases and marks the
/// guard as consumed. If a panic or early `return` skips that call, the `Drop`
/// impl spawns a background task to release the lock — best-effort; if the
/// tokio runtime is shutting down the release may be lost and the dashboard's
/// crash-recovery sweep (Task 12) will reclaim the lock.
struct ReleaseGuard {
    client: Arc<PgClient>,
    tester_id: Uuid,
    config_id: Uuid,
    released: bool,
}

impl ReleaseGuard {
    fn new(client: Arc<PgClient>, tester_id: Uuid, config_id: Uuid) -> Self {
        Self {
            client,
            tester_id,
            config_id,
            released: false,
        }
    }

    async fn release_now(mut self) {
        if self.released {
            return;
        }
        self.released = true;
        if let Err(e) = tester_state::release(&self.client, &self.tester_id, &self.config_id)
            .await
        {
            tracing::error!(
                tester_id = %self.tester_id,
                config_id = %self.config_id,
                "failed to release tester lock: {e:#}"
            );
        }
    }
}

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let client = self.client.clone();
        let tid = self.tester_id;
        let cid = self.config_id;
        // Best-effort: spawn on the current runtime if one is available.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(e) = tester_state::release(&client, &tid, &cid).await {
                    tracing::error!(
                        tester_id = %tid,
                        config_id = %cid,
                        "Drop release failed: {e:#}"
                    );
                }
            });
        } else {
            tracing::warn!(
                tester_id = %tid,
                config_id = %cid,
                "ReleaseGuard dropped without tokio runtime — crash recovery must reclaim lock"
            );
        }
    }
}

/// Invoke `az vm start --ids <resource_id>` via `tokio::process::Command`
/// (execvp, no shell) and wait for SSH to come up.
async fn ensure_running_via_azure(tester: &ProjectTesterRow) -> Result<()> {
    let resource_id = tester
        .vm_resource_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("tester {} has no vm_resource_id", tester.tester_id))?;
    let ip = tester
        .public_ip
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("tester {} has no public_ip", tester.tester_id))?;

    tracing::info!(
        tester_id = %tester.tester_id,
        %resource_id,
        "starting stopped tester VM via az vm start"
    );
    let out = tokio::process::Command::new("az")
        .arg("vm")
        .arg("start")
        .arg("--ids")
        .arg(resource_id)
        .output()
        .await
        .context("spawning az vm start")?;
    if !out.status.success() {
        anyhow::bail!(
            "az vm start failed (status={:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // Poll SSH up to ~5 minutes.
    for attempt in 1..=30u32 {
        match ssh::ssh_exec(ip, "echo ok").await {
            Ok(_) => {
                tracing::info!(tester_id = %tester.tester_id, attempt, "SSH ready after VM start");
                return Ok(());
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
    anyhow::bail!("SSH not available on {ip} within 5 minutes after VM start")
}

/// Subset of the dashboard's `project_tester` row that the orchestrator needs
/// when executing an application benchmark against a persistent tester.
///
/// This is defined locally (rather than imported from `networker-dashboard`)
/// because the orchestrator is a standalone crate that talks to Postgres
/// directly via tokio-postgres. Only the columns consumed by the executor
/// are included — extend as needed.
///
/// `dead_code` is allowed because Task 23 (the `execute_testbed_application`
/// rewrite) is the first caller; this helper is committed independently so
/// Task 23 lands as a pure swap.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProjectTesterRow {
    pub tester_id: Uuid,
    pub project_id: String,
    pub name: String,
    pub public_ip: Option<String>,
    pub ssh_user: String,
    pub vm_name: Option<String>,
    pub vm_resource_id: Option<String>,
    pub power_state: String,
    pub allocation: String,
    pub installer_version: Option<String>,
}

/// Look up the persistent tester associated with a given benchmark config.
///
/// Joins `benchmark_config` and `project_tester` on `benchmark_config.tester_id`.
/// Returns an error if the config has no tester (`tester_id IS NULL`) — for
/// application-mode benchmarks the V027 SQL CHECK constraint should make this
/// impossible, but we defend against it so a malformed row fails loudly
/// instead of silently skipping the tester-lock flow.
///
/// Task 23 (`execute_testbed_application` rewrite) is the primary caller.
#[allow(dead_code)]
pub async fn lookup_tester(
    client: &tokio_postgres::Client,
    config_id: &Uuid,
) -> Result<ProjectTesterRow> {
    // `public_ip::text` casts INET → TEXT so tokio-postgres can decode it
    // as `Option<String>` without needing the `with-cidr` feature.
    let row = client
        .query_opt(
            r#"
            SELECT t.tester_id,
                   t.project_id,
                   t.name,
                   t.public_ip::text,
                   t.ssh_user,
                   t.vm_name,
                   t.vm_resource_id,
                   t.power_state,
                   t.allocation,
                   t.installer_version
              FROM benchmark_config c
              JOIN project_tester   t ON t.tester_id = c.tester_id
             WHERE c.config_id = $1
            "#,
            &[config_id],
        )
        .await
        .with_context(|| format!("lookup_tester query failed for config {config_id}"))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "benchmark_config {} has no tester_id (or the referenced tester no longer exists)",
                config_id
            )
        })?;

    Ok(ProjectTesterRow {
        tester_id: row.get(0),
        project_id: row.get(1),
        name: row.get(2),
        public_ip: row.get::<_, Option<String>>(3),
        ssh_user: row.get(4),
        vm_name: row.get(5),
        vm_resource_id: row.get(6),
        power_state: row.get(7),
        allocation: row.get(8),
        installer_version: row.get(9),
    })
}

/// Start a pre-deployed language server on an existing VM.
async fn start_existing_server(vm: &VmInfo, language: &str) -> Result<()> {
    validate_shell_safe(language, "language")?;

    // Kill anything on port 8443
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo lsof -ti :8443 | xargs sudo kill -9 2>/dev/null || true",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let start_cmd = match language {
        "rust" => "nohup /opt/bench/rust-server --https-port 8443 > /dev/null 2>&1 &",
        "go" => "BENCH_CERT_DIR=/opt/bench nohup /opt/bench/go-server > /dev/null 2>&1 &",
        "cpp" => "BENCH_CERT_DIR=/opt/bench nohup /opt/bench/cpp-build/server > /dev/null 2>&1 &",
        "nodejs" => "BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup node /opt/bench/nodejs-server.js > /dev/null 2>&1 &",
        "python" => "cd /opt/bench && BENCH_CERT_DIR=/opt/bench nohup uvicorn server:app --host 0.0.0.0 --port 8443 --ssl-keyfile /opt/bench/key.pem --ssl-certfile /opt/bench/cert.pem --log-level error > /dev/null 2>&1 &",
        "java" => "cd /opt/bench && BENCH_CERT_DIR=/opt/bench nohup java Server > /dev/null 2>&1 &",
        "ruby" => "cd /opt/bench/ruby && BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup bundle exec puma -C puma.rb > /dev/null 2>&1 &",
        "php" => "BENCH_CERT_DIR=/opt/bench nohup php /opt/bench/php/server.php > /dev/null 2>&1 &",
        "nginx" => "sudo systemctl restart nginx",
        _ if language.starts_with("csharp-") => {
            // Handled below with dynamic string
            ""
        }
        _ => anyhow::bail!("Unknown language: {language}"),
    };

    if language.starts_with("csharp-") {
        let cmd = format!(
            "chmod +x /opt/bench/{lang}/{lang} 2>/dev/null; BENCH_CERT_DIR=/opt/bench BENCH_PORT=8443 nohup /opt/bench/{lang}/{lang} > /dev/null 2>&1 &",
            lang = language
        );
        ssh::ssh_exec(&vm.ip, &cmd).await?;
    } else {
        ssh::ssh_exec(&vm.ip, start_cmd).await?;
    }

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Health check
    for i in 0..15 {
        if let Ok(out) = ssh::ssh_exec(
            &vm.ip,
            "curl -sk --max-time 2 https://localhost:8443/health 2>/dev/null",
        )
        .await
        {
            if out.contains("ok") || out.contains("status") {
                tracing::info!("{} server healthy on {}", language, vm.ip);
                return Ok(());
            }
        }
        if i < 14 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    anyhow::bail!("{} server failed health check after 15s", language)
}

/// Stop any running server on port 8443.
#[allow(dead_code)]
async fn stop_existing_server(vm: &VmInfo) {
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo lsof -ti :8443 | xargs sudo kill -9 2>/dev/null || true",
    )
    .await;
}

/// Deploy a reverse proxy on a VM. Uses install.sh --benchmark-proxy-swap.
// Token generation moved to crate::token_manager

/// Validate a name is safe for shell interpolation (alphanumeric + dash/underscore/dot).
fn validate_shell_safe(name: &str, label: &str) -> Result<()> {
    anyhow::ensure!(
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'),
        "{label} name {name:?} contains unsafe characters for shell interpolation"
    );
    Ok(())
}

async fn deploy_proxy(vm: &VmInfo, proxy: &str) -> Result<()> {
    validate_shell_safe(proxy, "proxy")?;
    tracing::info!("Deploying proxy {} on {}", proxy, vm.ip);
    // Deploy proxy with install.sh. The --benchmark-proxy-swap health check
    // may timeout because the upstream server isn't running yet — that's expected.
    // We run it in a subshell with || true, then verify nginx config separately.
    let cmd = format!(
        "bash -c 'export DEBIAN_FRONTEND=noninteractive; curl -fsSL https://raw.githubusercontent.com/irlm/networker-tester/main/install.sh | sudo -E bash -s -- --benchmark-proxy-swap {} 2>&1; true' && sudo nginx -t 2>&1",
        proxy
    );
    ssh::ssh_exec(&vm.ip, &cmd)
        .await
        .with_context(|| format!("Failed to deploy proxy {proxy} on {}", vm.ip))?;
    tracing::info!("Proxy {} deployed successfully (health check deferred until backend starts)", proxy);
    // Health check is deferred — in application mode, the backend (language server)
    // starts AFTER the proxy. The proxy will respond once the backend is running.
    Ok(())
}

/// Stop the current proxy and flush connections (isolation protocol).
async fn stop_proxy(vm: &VmInfo) {
    tracing::info!("Stopping proxy on {}", vm.ip);
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo systemctl stop nginx caddy traefik haproxy apache2 httpd 2>/dev/null; sudo fuser -k 8443/tcp 2>/dev/null; sleep 2",
    )
    .await;
}

/// Deploy a language server in application mode (localhost:8080, no TLS).
/// Token is already on the VM at /opt/bench/.api-token (deployed via SCP).
/// The language server reads the token from that file at startup.
async fn deploy_app_language(vm: &VmInfo, language: &str, proxy: &str) -> Result<()> {
    validate_shell_safe(language, "language")?;
    validate_shell_safe(proxy, "proxy")?;
    tracing::info!(
        "Deploying {} in application mode on {}",
        language,
        vm.ip
    );
    // Server reads BENCH_API_TOKEN from /opt/bench/.api-token at startup.
    // LOG_FORMAT=json enables structured JSON logging on the server process.
    let cmd = format!(
        "export DEBIAN_FRONTEND=noninteractive LOG_FORMAT=json LOG_SERVICE={} BENCH_API_TOKEN=$(cat /opt/bench/.api-token 2>/dev/null) && curl -fsSL https://raw.githubusercontent.com/irlm/networker-tester/main/install.sh | sudo -E bash -s -- --benchmark-server {} --benchmark-proxy {} 2>&1",
        language, language, proxy
    );
    ssh::ssh_exec(&vm.ip, &cmd)
        .await
        .with_context(|| format!("Failed to deploy {language} in application mode on {}", vm.ip))?;
    Ok(())
}

/// Stop the language server on port 8080.
async fn stop_app_language(vm: &VmInfo) {
    let _ = ssh::ssh_exec(
        &vm.ip,
        "sudo lsof -ti :8080 | xargs sudo kill -9 2>/dev/null || true",
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
}

/// Run the Chrome-based benchmark for a proxy+language combination.
async fn run_chrome_benchmark(
    vm: &VmInfo,
    proxy: &str,
    language: &str,
    http_version: &str,
    connection_mode: &str,
    methodology: &MethodologyConfig,
    bench_token: &str,
) -> Result<serde_json::Value> {
    let token_arg = if bench_token.is_empty() {
        String::new()
    } else {
        format!(" --token {bench_token}")
    };
    // Write output to a file to avoid SSH stdout buffer limits (64KB).
    // Then read the file back separately.
    let output_file = "/tmp/bench-chrome-result.json";
    let cmd = format!(
        "export PATH=/usr/bin:/usr/local/bin:$PATH && cd /opt/bench/chrome-harness && \
         node runner.js \
         --target https://localhost:8443 \
         --warmup {} \
         --measured {} \
         --concurrency 10 \
         --http-version {} \
         --connection-mode {} \
         --timeout {}{} > {} 2>/dev/null",
        methodology.warmup_runs,
        methodology.min_measured,
        http_version,
        connection_mode,
        methodology.timeout_secs,
        token_arg,
        output_file,
    );

    tracing::info!(
        "Running Chrome benchmark: {} behind {}, http={}, conn={}",
        language, proxy, http_version, connection_mode,
    );

    ssh::ssh_exec(&vm.ip, &cmd)
        .await
        .with_context(|| {
            format!(
                "Chrome benchmark failed for {language} behind {proxy} (http={http_version}, conn={connection_mode})"
            )
        })?;

    // Read the result file
    let output = ssh::ssh_exec(&vm.ip, &format!("cat {output_file}"))
        .await
        .with_context(|| format!("Failed to read Chrome benchmark output from {}", vm.ip))?;

    // Parse JSON output
    let result: serde_json::Value = serde_json::from_str(&output).with_context(|| {
        format!(
            "Failed to parse Chrome benchmark output for {language} behind {proxy}: {}",
            &output[..output.len().min(200)]
        )
    })?;

    // Check for error in results
    if result.get("error").is_some() {
        anyhow::bail!(
            "Chrome benchmark returned error: {}",
            result["error"].as_str().unwrap_or("unknown")
        );
    }

    Ok(result)
}

/// In application mode, HTTP/3 support depends on the proxy, not the language.
fn proxy_supports_http3(proxy: &str) -> bool {
    matches!(proxy, "nginx" | "caddy" | "traefik" | "iis")
}

/// Convert methodology modes to HTTP version labels for Chrome harness.
/// Maps http1→h1, http2→h2, http3→h3, filters by proxy capability.
fn effective_http_versions_for_proxy(proxy: &str, modes: &[String]) -> Vec<String> {
    modes
        .iter()
        .filter_map(|m| match m.as_str() {
            "http1" => Some("h1".to_string()),
            "http2" => Some("h2".to_string()),
            "http3" if proxy_supports_http3(proxy) => Some("h3".to_string()),
            "http3" => {
                tracing::info!("Skipping http3 for proxy {} (no QUIC support)", proxy);
                None
            }
            _ => None, // skip non-HTTP modes like download/upload
        })
        .collect()
}

fn effective_modes_for_proxy(proxy: &str, modes: &[String]) -> String {
    if proxy_supports_http3(proxy) {
        modes.join(",")
    } else {
        let filtered: Vec<&str> = modes
            .iter()
            .map(|s| s.as_str())
            .filter(|m| *m != "http3")
            .collect();
        if filtered.len() < modes.len() {
            tracing::info!("Skipping http3 for proxy {} (no QUIC support)", proxy);
        }
        filtered.join(",")
    }
}

/// Outcome of a single testbed execution.
#[allow(dead_code)]
struct TestbedOutcome {
    testbed_id: String,
    languages_completed: u32,
    languages_failed: u32,
    provisioned_vm: bool,
}

/// Execute the full benchmark sweep triggered by the dashboard.
///
/// For each testbed in the config, provisions/reuses a VM, deploys each language,
/// runs the benchmark, reports results via callback, and optionally tears down.
pub async fn execute_dashboard_benchmark(
    config: &DashboardBenchmarkConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    bench_dir: &Path,
) -> Result<()> {
    let overall_start = Instant::now();
    let total_testbeds = config.testbeds.len();

    tracing::info!(
        "Starting dashboard benchmark: config_id={}, testbeds={}, languages_per_testbed=variable",
        config.config_id,
        total_testbeds,
    );

    let mut any_failure = false;

    for (testbed_index, testbed) in config.testbeds.iter().enumerate() {
        // Check cancellation before each testbed.
        if *cancel_rx.borrow() {
            tracing::warn!(
                "Cancellation requested before testbed {}",
                testbed.testbed_id
            );
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!(
                    "Cancelled before testbed {} of {}",
                    testbed_index + 1,
                    total_testbeds
                )],
            )
            .await;
            break;
        }

        tracing::info!(
            "--- Testbed {}/{}: {} ({}/{}) ---",
            testbed_index + 1,
            total_testbeds,
            testbed.testbed_id,
            testbed.cloud,
            testbed.region,
        );

        let outcome = execute_testbed(testbed, config, callback, cancel_rx, bench_dir).await;

        match outcome {
            Ok(outcome) => {
                if outcome.languages_failed > 0 {
                    any_failure = true;
                }
                // Teardown if auto_teardown and we provisioned the VM
                if config.auto_teardown && outcome.provisioned_vm {
                    teardown_testbed(testbed, callback).await;
                }
            }
            Err(e) => {
                any_failure = true;
                tracing::error!("Testbed {} failed: {:#}", testbed.testbed_id, e);
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Testbed failed: {e:#}")],
                )
                .await;
            }
        }
    }

    // Report overall completion.
    let duration_secs = overall_start.elapsed().as_secs_f64();
    let final_status = if *cancel_rx.borrow() {
        "cancelled"
    } else if any_failure {
        "completed_with_errors"
    } else {
        "completed"
    };

    tracing::info!(
        "Benchmark run finished: status={}, duration={:.1}s",
        final_status,
        duration_secs,
    );

    let error_msg = if any_failure {
        Some("One or more testbeds had errors".to_string())
    } else {
        None
    };
    if let Err(e) = callback
        .complete(final_status, duration_secs, error_msg)
        .await
    {
        tracing::error!("Failed to report completion: {e:#}");
    }

    Ok(())
}

/// Execute a single testbed: provision/reuse VM, deploy + benchmark each language.
async fn execute_testbed(
    testbed: &TestbedConfig,
    config: &DashboardBenchmarkConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    bench_dir: &Path,
) -> Result<TestbedOutcome> {
    let methodology = &config.methodology;
    let language_total = testbed.languages.len() as u32;
    let mut languages_completed = 0u32;
    let mut languages_failed = 0u32;

    // Step 1: Resolve VM — use existing_vm_ip or provision.
    status_callback(
        callback,
        &testbed.testbed_id,
        "provisioning",
        "",
        0,
        language_total,
        "Resolving VM...",
    )
    .await;

    let (vm, provisioned) = resolve_vm(testbed)
        .await
        .with_context(|| format!("resolving VM for testbed {}", testbed.testbed_id))?;

    // Wait for SSH to become available (fresh VMs need 30-60s after creation)
    if provisioned {
        tracing::info!("Waiting for SSH on {}...", vm.ip);
        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!("Waiting for SSH on {}...", vm.ip)],
        )
        .await;
        let mut ssh_ready = false;
        for attempt in 1..=30 {
            match ssh::ssh_exec(&vm.ip, "echo ok").await {
                Ok(_) => {
                    tracing::info!("SSH ready on {} (attempt {})", vm.ip, attempt);
                    ssh_ready = true;
                    break;
                }
                Err(_) => {
                    if attempt % 5 == 0 {
                        tracing::info!("SSH not ready on {} (attempt {}/30)", vm.ip, attempt);
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
        if !ssh_ready {
            anyhow::bail!("SSH not available on {} after 5 minutes", vm.ip);
        }
    }

    log_callback(
        callback,
        &testbed.testbed_id,
        vec![format!(
            "VM ready: {} at {} (provisioned={})",
            vm.name, vm.ip, provisioned
        )],
    )
    .await;

    // Branch: application mode uses proxy × language matrix.
    if config.benchmark_type == "application" {
        return execute_testbed_application(
            testbed, config, callback, cancel_rx, bench_dir, &vm, provisioned,
        )
        .await;
    }

    // Step 2: Iterate over languages (fullstack mode).
    for (lang_index, language) in testbed.languages.iter().enumerate() {
        let lang_index_u32 = lang_index as u32;

        // Check cancellation between languages.
        if *cancel_rx.borrow() {
            tracing::warn!(
                "Cancellation requested, stopping testbed {}",
                testbed.testbed_id
            );
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!(
                    "Cancelled after {languages_completed} of {language_total} languages"
                )],
            )
            .await;
            break;
        }

        // Also check via callback (in case heartbeat hasn't caught up yet).
        match callback.check_cancelled().await {
            Ok(true) => {
                tracing::warn!(
                    "Dashboard cancelled, stopping testbed {}",
                    testbed.testbed_id
                );
                break;
            }
            Ok(false) => {}
            Err(e) => tracing::warn!("Cancellation check failed: {e}"),
        }

        tracing::info!(
            "Language {}/{}: {} on testbed {}",
            lang_index + 1,
            language_total,
            language,
            testbed.testbed_id,
        );

        status_callback(
            callback,
            &testbed.testbed_id,
            "running",
            language,
            lang_index_u32 + 1,
            language_total,
            &format!(
                "Running language {} of {}: {}",
                lang_index + 1,
                language_total,
                language
            ),
        )
        .await;

        // Start language server — skip full deploy for existing VMs (already deployed).
        let use_existing = testbed.existing_vm_ip.is_some();
        if use_existing {
            // Existing VM: just start the server, skip build+deploy
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!("Starting {} server on existing VM...", language)],
            )
            .await;

            if let Err(e) = start_existing_server(&vm, language).await {
                tracing::error!(
                    "Start failed for {} on testbed {}: {:#}",
                    language,
                    testbed.testbed_id,
                    e
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Start failed for {}: {e:#}", language)],
                )
                .await;
                languages_failed += 1;
                continue;
            }
        } else {
            // New VM: full deploy (build + copy + start)
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!("Deploying {} server...", language)],
            )
            .await;

            if let Err(e) = deployer::deploy_api(&vm, language, bench_dir).await {
                tracing::error!(
                    "Deploy failed for {} on testbed {}: {:#}",
                    language,
                    testbed.testbed_id,
                    e
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Deploy failed for {}: {e:#}", language)],
                )
                .await;
                languages_failed += 1;
                continue;
            }
        }

        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!("{} server ready", language)],
        )
        .await;

        // Run benchmark for each mode.
        let modes_str = methodology
            .modes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(",");

        let test_params = runner::TestParams {
            warmup_requests: methodology.warmup_runs as u64,
            benchmark_requests: methodology.min_measured as u64,
            timeout_secs: methodology.timeout_secs as u64,
        };

        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!(
                "Running benchmark: modes={}, warmup={}, measured={}, timeout={}s",
                modes_str,
                methodology.warmup_runs,
                methodology.min_measured,
                methodology.timeout_secs,
            )],
        )
        .await;

        match run_language_benchmark(
            &vm,
            &test_params,
            language,
            &modes_str,
            config.callback_url.as_deref(),
            config.callback_token.as_deref(),
            &config.config_id,
            &testbed.testbed_id,
        )
        .await
        {
            Ok(artifact_json) => {
                tracing::info!(
                    "Benchmark complete for {} on testbed {}",
                    language,
                    testbed.testbed_id
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("{} benchmark complete", language)],
                )
                .await;

                // Report result via callback.
                if let Err(e) = callback
                    .result(&testbed.testbed_id, language, artifact_json)
                    .await
                {
                    tracing::error!("Failed to report result for {}: {e:#}", language);
                    log_callback(
                        callback,
                        &testbed.testbed_id,
                        vec![format!("Result callback failed for {}: {e:#}", language)],
                    )
                    .await;
                    languages_failed += 1;
                } else {
                    languages_completed += 1;
                }
            }
            Err(e) => {
                tracing::error!(
                    "Benchmark failed for {} on testbed {}: {:#}",
                    language,
                    testbed.testbed_id,
                    e
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Benchmark failed for {}: {e:#}", language)],
                )
                .await;
                languages_failed += 1;
            }
        }

        // Stop the server before the next language.
        if use_existing {
            stop_existing_server(&vm).await;
        } else if let Err(e) = deployer::stop_api(&vm).await {
            tracing::warn!("Failed to stop API after {}: {e}", language);
        }
    }

    // Report testbed complete.
    let testbed_status = if languages_completed > 0 && languages_failed == 0 && !*cancel_rx.borrow() {
        "completed"
    } else if *cancel_rx.borrow() {
        "cancelled"
    } else {
        "completed_with_errors"
    };

    status_callback(
        callback,
        &testbed.testbed_id,
        testbed_status,
        "",
        language_total,
        language_total,
        &format!("Testbed complete: {languages_completed} succeeded, {languages_failed} failed"),
    )
    .await;

    Ok(TestbedOutcome {
        testbed_id: testbed.testbed_id.clone(),
        languages_completed,
        languages_failed,
        provisioned_vm: provisioned,
    })
}

/// Execute application benchmark: proxy × language matrix, guarded by the
/// persistent-tester lock flow.
///
/// Task 23 rewrite: this function now looks up the `project_tester` row bound
/// to the benchmark config, acquires its lock via `tester_state::try_acquire`,
/// runs the existing proxy × language matrix under a `ReleaseGuard`, then
/// releases and notifies the queue dispatcher. Queued-class outcomes short
/// circuit with `benchmark_config.status='queued'` so the dashboard
/// dispatcher can promote the next waiter.
async fn execute_testbed_application(
    testbed: &TestbedConfig,
    config: &DashboardBenchmarkConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    _bench_dir: &Path,
    vm: &VmInfo,
    provisioned: bool,
) -> Result<TestbedOutcome> {
    // ---------------------------------------------------------------
    // Persistent-tester lock flow
    // ---------------------------------------------------------------
    let config_uuid = Uuid::parse_str(&config.config_id)
        .with_context(|| format!("config_id {:?} is not a valid UUID", config.config_id))?;

    let db = connect_orchestrator_db().await?;
    let tester = lookup_tester(&db, &config_uuid).await?;
    tracing::info!(
        tester_id = %tester.tester_id,
        tester_name = %tester.name,
        power_state = %tester.power_state,
        allocation = %tester.allocation,
        "resolved persistent tester for config"
    );

    set_phase(&db, &config_uuid, "starting").await.ok();

    // Acquire loop: bounded retries with a small backoff so a stuck
    // transient state never spins hot. NeedsStart is the one outcome where
    // the orchestrator actively drives the VM back to running; everything
    // else either retries briefly, queues, or bails.
    let max_attempts = 20u32;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if attempt > max_attempts {
            anyhow::bail!(
                "could not acquire tester {} after {} attempts",
                tester.tester_id,
                max_attempts
            );
        }

        let outcome = tester_state::try_acquire(&db, &tester.tester_id, &config_uuid).await?;
        match outcome {
            AcquireOutcome::Acquired => {
                break;
            }
            AcquireOutcome::NeedsStart => {
                tracing::info!(
                    tester_id = %tester.tester_id,
                    "tester stopped — starting VM before retrying acquire"
                );
                if let Err(e) = ensure_running_via_azure(&tester).await {
                    tracing::error!(
                        tester_id = %tester.tester_id,
                        "ensure_running_via_azure failed: {e:#}"
                    );
                    set_benchmark_status(&db, &config_uuid, "failed").await.ok();
                    anyhow::bail!("failed to start tester {}: {e:#}", tester.tester_id);
                }
                // Nudge power_state forward; best-effort, dispatcher also reconciles.
                let _ = tester_state::try_power_transition(
                    &db,
                    &tester.tester_id,
                    "stopped",
                    "running",
                )
                .await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            AcquireOutcome::Transient(state) => {
                tracing::info!(
                    tester_id = %tester.tester_id,
                    state,
                    "tester in transient state — queuing"
                );
                set_benchmark_status(&db, &config_uuid, "queued").await.ok();
                return Ok(TestbedOutcome {
                    testbed_id: testbed.testbed_id.clone(),
                    languages_completed: 0,
                    languages_failed: 0,
                    provisioned_vm: provisioned,
                });
            }
            AcquireOutcome::Upgrading => {
                tracing::info!(
                    tester_id = %tester.tester_id,
                    "tester upgrading — queuing"
                );
                set_benchmark_status(&db, &config_uuid, "queued").await.ok();
                return Ok(TestbedOutcome {
                    testbed_id: testbed.testbed_id.clone(),
                    languages_completed: 0,
                    languages_failed: 0,
                    provisioned_vm: provisioned,
                });
            }
            AcquireOutcome::AlreadyLockedBy(other) => {
                tracing::info!(
                    tester_id = %tester.tester_id,
                    locked_by = %other,
                    "tester already locked by another config — queuing"
                );
                set_benchmark_status(&db, &config_uuid, "queued").await.ok();
                return Ok(TestbedOutcome {
                    testbed_id: testbed.testbed_id.clone(),
                    languages_completed: 0,
                    languages_failed: 0,
                    provisioned_vm: provisioned,
                });
            }
            AcquireOutcome::Errored => {
                set_benchmark_status(&db, &config_uuid, "failed").await.ok();
                anyhow::bail!(
                    "tester {} is in error state — cannot run benchmark",
                    tester.tester_id
                );
            }
            AcquireOutcome::Gone => {
                // RR-007: tester row was deleted during acquire. Treat as
                // terminal failure — there is nothing to queue against.
                tracing::error!(
                    tester_id = %tester.tester_id,
                    config_id = %config_uuid,
                    "tester deleted during acquire — failing benchmark"
                );
                set_benchmark_status(&db, &config_uuid, "failed").await.ok();
                anyhow::bail!("tester {} deleted during acquire", tester.tester_id);
            }
            AcquireOutcome::NotIdle(state) => {
                tracing::warn!(
                    tester_id = %tester.tester_id,
                    state,
                    "tester in unexpected state — queuing"
                );
                set_benchmark_status(&db, &config_uuid, "queued").await.ok();
                return Ok(TestbedOutcome {
                    testbed_id: testbed.testbed_id.clone(),
                    languages_completed: 0,
                    languages_failed: 0,
                    provisioned_vm: provisioned,
                });
            }
        }
    }

    // ---------------------------------------------------------------
    // RR-002: we now hold the lock. The matrix must run inside a panic
    // boundary so that a panic in deploy/runner code cannot skip
    // `release_now().await`. Drop is a defensive backstop only — Drop
    // spawns release as a detached task, which can be cancelled by
    // runtime shutdown, leaking the lock.
    // ---------------------------------------------------------------
    let guard = ReleaseGuard::new(db.clone(), tester.tester_id, config_uuid);
    set_phase(&db, &config_uuid, "deploy").await.ok();

    let matrix_result = AssertUnwindSafe(run_application_matrix(
        testbed,
        config,
        callback,
        cancel_rx,
        vm,
        provisioned,
        &db,
        &config_uuid,
    ))
    .catch_unwind()
    .await;

    // Synchronous release, awaited before any terminal status write so the
    // dispatcher notification below observes an idle row.
    guard.release_now().await;
    notify_queue_dispatcher(&tester.tester_id).await;

    match matrix_result {
        Ok(Ok(outcome)) => {
            // Happy path — final status based on matrix outcome.
            let final_status = if outcome.languages_completed > 0 && outcome.languages_failed == 0 {
                "completed"
            } else if outcome.languages_completed == 0 && outcome.languages_failed == 0 {
                // No work ran (cancel before loop body). Leave as completed.
                "completed"
            } else if outcome.languages_completed == 0 {
                "failed"
            } else {
                "completed_with_errors"
            };
            set_benchmark_status(&db, &config_uuid, final_status).await.ok();
            set_phase(&db, &config_uuid, "done").await.ok();
            Ok(outcome)
        }
        Ok(Err(e)) => {
            // Matrix returned Err — record failed, surface the error.
            tracing::error!(
                config_id = %config_uuid,
                "application matrix returned error: {e:#}"
            );
            set_benchmark_status(&db, &config_uuid, "failed").await.ok();
            set_phase(&db, &config_uuid, "done").await.ok();
            Err(e)
        }
        Err(panic_payload) => {
            // Matrix panicked — lock already released above, now record a
            // terminal failed status and surface a synthesised error.
            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            tracing::error!(
                target: "orchestrator_matrix_panic",
                config_id = %config_uuid,
                tester_id = %tester.tester_id,
                panic = %panic_msg,
                "application matrix panicked — released lock, recording failed status"
            );
            set_benchmark_status(&db, &config_uuid, "failed").await.ok();
            set_phase(&db, &config_uuid, "done").await.ok();
            Err(anyhow::anyhow!(
                "application matrix panicked for config {}: {}",
                config_uuid,
                panic_msg
            ))
        }
    }
}

/// Pre-Task-23 body of `execute_testbed_application`. Extracted verbatim so
/// the lock flow can wrap it in a `ReleaseGuard` without reshuffling the
/// proxy × language loop. Task 24 will prune the dead chrome-harness deploy.
#[allow(clippy::too_many_arguments)]
async fn run_application_matrix(
    testbed: &TestbedConfig,
    config: &DashboardBenchmarkConfig,
    callback: &Arc<CallbackClient>,
    cancel_rx: &watch::Receiver<bool>,
    vm: &VmInfo,
    provisioned: bool,
    db: &PgClient,
    config_uuid: &Uuid,
) -> Result<TestbedOutcome> {
    let methodology = &config.methodology;

    // Filter out OS-incompatible languages (e.g. csharp-net48 on Linux).
    let languages: Vec<String> = testbed
        .languages
        .iter()
        .filter(|lang| {
            let needs_windows = matches!(lang.as_str(), "csharp-net48");
            if needs_windows && testbed.os != "windows" {
                tracing::warn!(
                    "Skipping {} on {} testbed {} (requires Windows)",
                    lang, testbed.os, testbed.testbed_id
                );
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();

    let mut languages_completed = 0u32;
    let mut languages_failed = 0u32;
    let total_combinations = (testbed.proxies.len() * languages.len()) as u32;
    let mut combination_index = 0u32;

    // NOTE: deadline is set AFTER setup (token deploy + harness install) completes,
    // not at function entry. Setup can take several minutes and shouldn't count
    // against the benchmark execution time budget.

    // Generate a unique API token for this VM (isolated per testbed)
    let bench_token = crate::token_manager::generate_token();
    tracing::info!(
        "Generated bench API token for testbed {} ({}...)",
        testbed.testbed_id,
        &bench_token[..8]
    );

    // Store token in Key Vault (if configured) for audit trail + revocation
    if let Err(e) = crate::token_manager::store_in_keyvault(
        &config.config_id,
        &testbed.testbed_id,
        &bench_token,
        config.created_by_email.as_deref().unwrap_or("unknown"),
        config.project_id.as_deref().unwrap_or("unknown"),
    )
    .await
    {
        tracing::warn!("Key Vault store failed (non-fatal): {e:#}");
    }

    // Deploy token to VM via SCP (secure file, not command line)
    if let Err(e) = crate::token_manager::deploy_to_vm(&vm.ip, &bench_token).await {
        tracing::error!("Failed to deploy API token to VM: {e:#}");
        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!("Failed to deploy API token: {e:#}")],
        )
        .await;
        return Ok(TestbedOutcome {
            testbed_id: testbed.testbed_id.clone(),
            languages_completed: 0,
            languages_failed: total_combinations,
            provisioned_vm: provisioned,
        });
    }

    // Deploy test harness (Node.js HTTP client — not Chrome browser)
    log_callback(
        callback,
        &testbed.testbed_id,
        vec!["Installing test harness (Node.js)...".to_string()],
    )
    .await;

    // Chrome harness is installed once at tester creation (services::tester_install); no per-benchmark install.

    // Set deadline AFTER setup completes — setup (token deploy, harness install)
    // can take several minutes and must not count against benchmark time.
    let deadline = Instant::now()
        + std::time::Duration::from_secs(
            methodology.timeout_secs as u64 * total_combinations.max(1) as u64,
        );

    tracing::info!(
        testbed_id = %testbed.testbed_id,
        proxies = ?testbed.proxies,
        languages = ?languages,
        total_combinations,
        deadline_secs = methodology.timeout_secs as u64 * total_combinations.max(1) as u64,
        "Starting application benchmark proxy/language matrix"
    );

    set_phase(db, config_uuid, "running").await.ok();

    for proxy in &testbed.proxies {
        // Check cancellation or deadline
        if *cancel_rx.borrow() {
            break;
        }
        if Instant::now() > deadline {
            tracing::warn!(
                "Application benchmark exceeded deadline on testbed {}, stopping",
                testbed.testbed_id
            );
            log_callback(
                callback,
                &testbed.testbed_id,
                vec!["Benchmark exceeded wall-clock deadline, stopping".to_string()],
            )
            .await;
            break;
        }

        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!("Deploying proxy: {}", proxy)],
        )
        .await;

        // Deploy proxy
        if let Err(e) = deploy_proxy(vm, proxy).await {
            tracing::error!(
                "Proxy deploy failed for {} on testbed {}: {:#}",
                proxy,
                testbed.testbed_id,
                e
            );
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!("Proxy {} deploy failed: {e:#}", proxy)],
            )
            .await;
            languages_failed += testbed.languages.len() as u32;
            continue;
        }

        for language in &languages {
            combination_index += 1;

            if *cancel_rx.borrow() {
                break;
            }

            tracing::info!(
                "Application benchmark {}/{}: {} behind {} on testbed {}",
                combination_index,
                total_combinations,
                language,
                proxy,
                testbed.testbed_id,
            );

            status_callback(
                callback,
                &testbed.testbed_id,
                "running",
                language,
                combination_index,
                total_combinations,
                &format!(
                    "{} behind {} ({}/{})",
                    language, proxy, combination_index, total_combinations
                ),
            )
            .await;

            // Deploy language in application mode
            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!(
                    "Deploying {} (application mode, behind {})...",
                    language, proxy
                )],
            )
            .await;

            if let Err(e) = deploy_app_language(vm, language, proxy).await {
                tracing::error!(
                    "App deploy failed for {} behind {}: {:#}",
                    language,
                    proxy,
                    e
                );
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!(
                        "Deploy failed for {} behind {}: {e:#}",
                        language, proxy
                    )],
                )
                .await;
                languages_failed += 1;
                continue;
            }

            // Run Chrome benchmark for each HTTP version the proxy supports
            let http_versions = effective_http_versions_for_proxy(proxy, &methodology.modes);

            log_callback(
                callback,
                &testbed.testbed_id,
                vec![format!(
                    "Running Chrome benchmark: {} behind {}, versions={:?}",
                    language, proxy, http_versions,
                )],
            )
            .await;

            let mut lang_ok = true;
            for http_ver in &http_versions {
                // Run warm connection phase
                match run_chrome_benchmark(vm, proxy, language, http_ver, "warm", methodology, &bench_token).await
                {
                    Ok(result) => {
                        tracing::info!(
                            "{} behind {} ({}:warm) complete",
                            language,
                            proxy,
                            http_ver
                        );

                        // Wrap result with metadata for callback
                        let artifact = serde_json::json!({
                            "proxy": proxy,
                            "http_version": http_ver,
                            "connection_mode": "warm",
                            "chrome_results": result,
                        });

                        if let Err(e) = callback
                            .result(&testbed.testbed_id, language, artifact)
                            .await
                        {
                            tracing::error!(
                                "Failed to report result for {} behind {} ({}:warm): {e:#}",
                                language,
                                proxy,
                                http_ver
                            );
                            log_callback(
                                callback,
                                &testbed.testbed_id,
                                vec![format!(
                                    "Result callback failed for {} behind {} ({}:warm): {e:#}",
                                    language, proxy, http_ver
                                )],
                            )
                            .await;
                            lang_ok = false;
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "{} behind {} ({}:warm) failed: {:#}",
                            language,
                            proxy,
                            http_ver,
                            e
                        );
                        log_callback(
                            callback,
                            &testbed.testbed_id,
                            vec![format!(
                                "{} behind {} ({}:warm) failed: {e:#}",
                                language, proxy, http_ver
                            )],
                        )
                        .await;
                        lang_ok = false;
                    }
                }
            }

            if lang_ok {
                languages_completed += 1;
            } else {
                languages_failed += 1;
            }

            // Collect server logs before stopping
            collect_server_logs(vm, language, callback, &testbed.testbed_id).await;

            // Stop language server before next language
            stop_app_language(vm).await;
        }

        // Stop proxy before swap (isolation protocol)
        stop_proxy(vm).await;
    }

    // Detect anomaly: 0 completed + 0 failed means loops didn't execute
    if languages_completed == 0 && languages_failed == 0 {
        tracing::error!(
            testbed_id = %testbed.testbed_id,
            total_combinations,
            proxies = ?testbed.proxies,
            languages = ?languages,
            "Application benchmark produced 0 completed and 0 failed — \
             proxy/language loops may not have executed"
        );
        log_callback(
            callback,
            &testbed.testbed_id,
            vec![format!(
                "BUG: 0 completed + 0 failed with {} combinations (proxies={:?}, languages={:?})",
                total_combinations, testbed.proxies, languages,
            )],
        )
        .await;
    }

    set_phase(db, config_uuid, "collect").await.ok();

    // Report testbed complete.
    let testbed_status = if languages_completed > 0 && languages_failed == 0 && !*cancel_rx.borrow() {
        "completed"
    } else if *cancel_rx.borrow() {
        "cancelled"
    } else {
        "completed_with_errors"
    };

    status_callback(
        callback,
        &testbed.testbed_id,
        testbed_status,
        "",
        total_combinations,
        total_combinations,
        &format!(
            "Testbed complete: {languages_completed} succeeded, {languages_failed} failed"
        ),
    )
    .await;

    // Cleanup: delete token from VM and Key Vault
    crate::token_manager::cleanup_vm(&vm.ip).await;
    if let Err(e) =
        crate::token_manager::cleanup_keyvault_vm(&config.config_id, &testbed.testbed_id).await
    {
        tracing::warn!("Key Vault cleanup failed (non-fatal): {e:#}");
    }

    Ok(TestbedOutcome {
        testbed_id: testbed.testbed_id.clone(),
        languages_completed,
        languages_failed,
        provisioned_vm: provisioned,
    })
}

/// Validate an IPv4 address string: must be 4 octets 0-255, no shell metacharacters.
/// For cloud-provisioned VMs, blocks link-local (169.254.x.x) and localhost (127.x.x.x).
fn validate_ip(ip: &str, is_cloud: bool) -> Result<()> {
    // Reject any shell metacharacters
    if ip.chars().any(|c| !c.is_ascii_digit() && c != '.') {
        anyhow::bail!("IP address contains invalid characters: {ip}");
    }
    let octets: Vec<&str> = ip.split('.').collect();
    if octets.len() != 4 {
        anyhow::bail!("IP address must have exactly 4 octets: {ip}");
    }
    for octet in &octets {
        let val: u16 = octet
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid octet in IP address: {ip}"))?;
        if val > 255 {
            anyhow::bail!("Octet out of range in IP address: {ip}");
        }
    }
    if is_cloud {
        let first: u8 = octets[0].parse().unwrap();
        let second: u8 = octets[1].parse().unwrap();
        if first == 127 {
            anyhow::bail!("Localhost address not allowed for cloud VMs: {ip}");
        }
        if first == 169 && second == 254 {
            anyhow::bail!("Link-local address not allowed for cloud VMs: {ip}");
        }
        if first == 10 {
            anyhow::bail!("RFC1918 private address not allowed for cloud VMs: {ip}");
        }
        if first == 172 && (16..=31).contains(&second) {
            anyhow::bail!("RFC1918 private address not allowed for cloud VMs: {ip}");
        }
        if first == 192 && second == 168 {
            anyhow::bail!("RFC1918 private address not allowed for cloud VMs: {ip}");
        }
        if first == 0 {
            anyhow::bail!("Invalid address not allowed for cloud VMs: {ip}");
        }
    }
    Ok(())
}

/// Resolve the VM for a testbed: use existing IP or provision a new one.
async fn resolve_vm(testbed: &TestbedConfig) -> Result<(VmInfo, bool)> {
    if let Some(ip) = &testbed.existing_vm_ip {
        let is_cloud = ["azure", "aws", "gcp"]
            .contains(&testbed.cloud.to_lowercase().as_str());
        validate_ip(ip, is_cloud)
            .with_context(|| format!("Invalid existing_vm_ip for testbed {}", testbed.testbed_id))?;
        tracing::info!(
            "Using existing VM at {} for testbed {}",
            ip,
            testbed.testbed_id
        );
        let vm = VmInfo {
            name: format!(
                "existing-{}",
                &testbed.testbed_id[..8.min(testbed.testbed_id.len())]
            ),
            ip: ip.clone(),
            cloud: testbed.cloud.clone(),
            region: testbed.region.clone(),
            os: "ubuntu".to_string(),
            vm_size: testbed.vm_size.clone(),
            resource_group: String::new(),
            ssh_user: "azureuser".to_string(),
        };
        Ok((vm, false))
    } else {
        // For now, auto-provisioning requires cloud CLI tools (az/aws/gcloud).
        // If none are available, fail fast with a helpful message.
        let vm_name = format!(
            "ab-{}-{}",
            &testbed.testbed_id[..8.min(testbed.testbed_id.len())],
            testbed.region
        );

        // Check if VM already exists.
        if let Some(existing) = provisioner::find_existing_vm(&vm_name).await? {
            if !existing.ip.is_empty() {
                tracing::info!("Reusing existing VM {} at {}", existing.name, existing.ip);
                return Ok((existing, false));
            }
        }

        tracing::info!(
            "Provisioning new VM: name={}, cloud={}, region={}, size={}",
            vm_name,
            testbed.cloud,
            testbed.region,
            testbed.vm_size,
        );

        let cloud_lower = testbed.cloud.to_lowercase();
        let size_lower = testbed.vm_size.to_lowercase();
        let resolved_size = crate::vm_tiers::resolve_vm_size(&cloud_lower, &size_lower);
        let vm = provisioner::provision_vm(
            &testbed.cloud,
            &testbed.region,
            "ubuntu",
            resolved_size,
            &vm_name,
        )
        .await?;
        Ok((vm, true))
    }
}

/// Languages that support HTTP/3 (QUIC).
/// Others will have http3 stripped from modes to avoid wasted benchmark time.
fn supports_http3(language: &str) -> bool {
    matches!(
        language,
        "rust"
            | "nginx"
            | "go"
            | "python"
            | "csharp-net7"
            | "csharp-net8"
            | "csharp-net8-aot"
            | "csharp-net9"
            | "csharp-net9-aot"
            | "csharp-net10"
            | "csharp-net10-aot"
            | "php"
    )
}

/// Run the benchmark for a single language and collect JSON output.
async fn run_language_benchmark(
    vm: &VmInfo,
    params: &runner::TestParams,
    language: &str,
    modes: &str,
    callback_url: Option<&str>,
    callback_token: Option<&str>,
    config_id: &str,
    testbed_id: &str,
) -> Result<serde_json::Value> {
    // Skip http3 for languages that don't support QUIC
    let effective_modes = if supports_http3(language) {
        modes.to_string()
    } else {
        let filtered: Vec<&str> = modes.split(',').filter(|m| m.trim() != "http3").collect();
        if filtered.len() < modes.split(',').count() {
            tracing::info!("Skipping http3 for {} (no QUIC support)", language);
        }
        filtered.join(",")
    };

    let target = format!("https://{}:8443/health", vm.ip);
    let tester_bin = resolve_tester_path();

    tracing::info!(
        "Running tester: target={}, modes={}, runs={}, timeout={}s",
        target,
        effective_modes,
        params.benchmark_requests,
        params.timeout_secs,
    );

    // Build args; add --payload-sizes if download/upload modes are present
    let mut args = vec![
        "--target".to_string(),
        target.clone(),
        "--modes".to_string(),
        effective_modes.clone(),
        "--runs".to_string(),
        params.benchmark_requests.to_string(),
        "--timeout".to_string(),
        params.timeout_secs.to_string(),
        "--insecure".to_string(),
        "--json-stdout".to_string(),
        "--benchmark-mode".to_string(),
    ];

    let needs_payload = effective_modes.split(',').any(|m| {
        let m = m.trim();
        m.starts_with("download") || m.starts_with("upload") || m.starts_with("udp")
    });
    if needs_payload {
        args.push("--payload-sizes".to_string());
        args.push("4k,64k,1m".to_string());
    }

    // Pass progress callback flags so the tester can report live progress to the dashboard.
    if let Some(url) = callback_url {
        args.push("--progress-url".to_string());
        args.push(url.to_string());
        if let Some(token) = callback_token {
            args.push("--progress-token".to_string());
            args.push(token.to_string());
        }
        args.push("--progress-config-id".to_string());
        args.push(config_id.to_string());
        args.push("--progress-testbed-id".to_string());
        args.push(testbed_id.to_string());
        args.push("--benchmark-language".to_string());
        args.push(language.to_string());
    }

    // Timeout: account for modes * payload-sizes * runs * timeout, plus warmup buffer
    let mode_count = effective_modes.split(',').count() as u64;
    let payload_multiplier = if needs_payload { 3u64 } else { 1u64 }; // 4k, 64k, 1m
    let total_requests = mode_count * payload_multiplier * params.benchmark_requests;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(params.timeout_secs * total_requests + 120),
        tokio::process::Command::new(&tester_bin)
            .args(&args)
            .output(),
    )
    .await
    .context("benchmark timed out")?
    .context("failed to execute networker-tester")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "networker-tester failed for {language} (exit={}): {}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let artifact: serde_json::Value =
        serde_json::from_str(&stdout).context("parsing tester JSON output")?;

    Ok(artifact)
}

/// Resolve the path to `networker-tester` (same logic as runner.rs).
fn resolve_tester_path() -> String {
    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|root| root.join("target/release/networker-tester"))
            .unwrap_or_default();
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }
    "networker-tester".to_string()
}

/// Tear down a provisioned VM for a testbed.
async fn teardown_testbed(testbed: &TestbedConfig, callback: &Arc<CallbackClient>) {
    let vm_name = format!(
        "ab-{}-{}",
        &testbed.testbed_id[..8.min(testbed.testbed_id.len())],
        testbed.region
    );

    log_callback(
        callback,
        &testbed.testbed_id,
        vec![format!("Tearing down VM {vm_name}...")],
    )
    .await;

    // Find and destroy the VM.
    match provisioner::find_existing_vm(&vm_name).await {
        Ok(Some(vm)) => {
            if let Err(e) = provisioner::destroy_vm(&vm).await {
                tracing::error!("Failed to destroy VM {}: {e:#}", vm_name);
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("Teardown failed for {vm_name}: {e:#}")],
                )
                .await;
            } else {
                tracing::info!("VM {} destroyed", vm_name);
                log_callback(
                    callback,
                    &testbed.testbed_id,
                    vec![format!("VM {vm_name} destroyed")],
                )
                .await;
            }
        }
        Ok(None) => {
            tracing::debug!("VM {} not found, nothing to tear down", vm_name);
        }
        Err(e) => {
            tracing::warn!("Failed to look up VM {} for teardown: {e}", vm_name);
        }
    }
}

/// Collect recent log output from the benchmark server on the VM.
/// Reads the last 100 lines from the server process output and forwards
/// them via the log callback so they appear in the dashboard live log.
async fn collect_server_logs(
    vm: &VmInfo,
    language: &str,
    callback: &CallbackClient,
    testbed_id: &str,
) {
    // Read last 100 lines from systemd journal or log file
    let cmd = "journalctl -u bench-server --no-pager -n 100 --output=cat 2>/dev/null || \
               tail -100 /opt/bench/server.log 2>/dev/null || \
               echo '(no server logs found)'";
    match ssh::ssh_exec(&vm.ip, cmd).await {
        Ok(output) => {
            let lines: Vec<String> = output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| format!("[{language}] {l}"))
                .collect();
            if !lines.is_empty() {
                tracing::info!(
                    language,
                    line_count = lines.len(),
                    "Collected server logs from VM"
                );
                log_callback(callback, testbed_id, lines).await;
            }
        }
        Err(e) => {
            tracing::debug!("Failed to collect server logs for {language}: {e:#}");
        }
    }
}

/// Helper: send a status callback, logging errors but not failing.
async fn status_callback(
    callback: &CallbackClient,
    testbed_id: &str,
    status: &str,
    current_language: &str,
    language_index: u32,
    language_total: u32,
    message: &str,
) {
    if let Err(e) = callback
        .status(
            testbed_id,
            status,
            current_language,
            language_index,
            language_total,
            message,
        )
        .await
    {
        tracing::warn!("Status callback failed: {e}");
    }
}

/// Helper: send a log callback, logging errors but not failing.
async fn log_callback(callback: &CallbackClient, testbed_id: &str, lines: Vec<String>) {
    if let Err(e) = callback.log(testbed_id, lines).await {
        tracing::warn!("Log callback failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_tester_path_fallback() {
        // When running tests, the binary path won't resolve to a real tester,
        // so we expect the PATH fallback.
        let path = resolve_tester_path();
        // Should either be an absolute path or the fallback "networker-tester"
        assert!(
            path == "networker-tester" || std::path::Path::new(&path).is_absolute(),
            "unexpected tester path: {path}"
        );
    }
}
