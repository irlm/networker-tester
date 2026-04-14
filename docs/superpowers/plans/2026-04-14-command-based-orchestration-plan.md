# Command-based orchestration - Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace SSH-driven orchestration with a typed command protocol over the existing agent WebSocket. SSH survives only as a debug fallback behind a flag. Cloud CLI survives only for VM power-state changes (start/stop/restart/delete from outside the VM).

**Architecture:** Agents already connect outbound to the dashboard over WS. Add typed `Command/CommandLog/CommandResult` message variants. Dashboard mints short-lived JWT per command (`exp = max_duration + 300`, re-minted at dispatch if tight). Agents validate + execute + stream logs + return structured results. Cloud-init / user-data replaces install-by-SSH.

**Tech stack:** Rust (networker-common, networker-agent, networker-dashboard, alethabench orchestrator), PostgreSQL (new `agent_command` table), React (new Tester selector in wizard).

**Decisions baked in:**
- Endpoints run `networker-agent` too (same protocol both sides).
- `tester_id` is per-testbed (co-located or cross-region each benchmark).
- Windows lands in Phase 2 via PowerShell + cloud-init `custom-data`.
- SSH kept only behind `--ssh-bootstrap` / `ORCH_DEBUG_SSH=1`.
- Cloud CLI kept for: `vm start`, `vm stop`, `vm restart`, `vm delete`, `vm deallocate` (power-state ops where agent may be unreachable).

---

## Phase 1 - Protocol + infrastructure

### Task 1: Add command message variants to networker-common

**Files:**
- Modify: `crates/networker-common/src/messages.rs`
- Test: `crates/networker-common/src/messages.rs` (same file, #[cfg(test)] mod)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn command_envelope_round_trips_as_json() {
    let c = AgentCommand {
        command_id: Uuid::new_v4(),
        config_id: Some(Uuid::new_v4()),
        token: "jwt.example.token".into(),
        verb: "health".into(),
        args: serde_json::json!({}),
        timeout_secs: 30,
    };
    let j = serde_json::to_string(&c).unwrap();
    let back: AgentCommand = serde_json::from_str(&j).unwrap();
    assert_eq!(back.command_id, c.command_id);
    assert_eq!(back.verb, "health");
}
```

- [ ] **Step 2: Run test (expect fail - AgentCommand not defined yet)**

Run: `cargo test -p networker-common command_envelope`
Expected: FAIL with "cannot find type AgentCommand"

- [ ] **Step 3: Add message types**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommand {
    pub command_id: Uuid,
    pub config_id: Option<Uuid>,
    pub token: String,
    pub verb: String,
    #[serde(default)]
    pub args: serde_json::Value,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommandLog {
    pub command_id: Uuid,
    pub stream: LogStream,
    pub line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream { Stdout, Stderr }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommandResult {
    pub command_id: Uuid,
    pub status: CommandStatus,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandStatus { Ok, Error, Timeout, Cancelled }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommandCancel { pub command_id: Uuid }
```

- [ ] **Step 4: Extend the DashboardMessage / AgentMessage enums**

Add new variants to the existing dashboard->agent and agent->dashboard enums: `Command(AgentCommand)`, `Cancel(AgentCommandCancel)` (dashboard->agent); `CommandLog(AgentCommandLog)`, `CommandResult(AgentCommandResult)` (agent->dashboard).

- [ ] **Step 5: Run tests - expect pass**

Run: `cargo test -p networker-common`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/networker-common/src/messages.rs
git commit -m "feat(common): add Command/CommandLog/CommandResult message types"
```

### Task 2: V033 migration - agent_command table

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs`

- [ ] **Step 1: Add migration SQL constant**

```rust
const V033_AGENT_COMMAND: &str = r#"
CREATE TABLE IF NOT EXISTS agent_command (
  command_id     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  agent_id       UUID NOT NULL REFERENCES agent(agent_id) ON DELETE CASCADE,
  config_id      UUID,
  verb           TEXT NOT NULL,
  args           JSONB NOT NULL DEFAULT '{}'::jsonb,
  status         TEXT NOT NULL DEFAULT 'pending',
  result         JSONB,
  error_message  TEXT,
  created_by     UUID REFERENCES dash_user(user_id),
  created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  started_at     TIMESTAMPTZ,
  finished_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_agent_command_agent  ON agent_command(agent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_agent_command_config ON agent_command(config_id) WHERE config_id IS NOT NULL;
"#;
```

- [ ] **Step 2: Invoke the migration in run_migrations (pattern copied from V032)**

- [ ] **Step 3: Run migration locally against the prod DB via tunnel**

Run: restart the dashboard; observe log line `Applying V033...`

- [ ] **Step 4: Smoke check**

```bash
psql "$DASHBOARD_DB_URL" -c "\d agent_command"
```

Expected: table + two indexes listed.

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/db/migrations.rs
git commit -m "feat(db): V033 agent_command table"
```

### Task 3: JWT minting for per-command auth

**Files:**
- Modify: `crates/networker-dashboard/src/auth.rs` (or new `auth/commands.rs`)
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Test first**

```rust
#[test]
fn mint_command_token_enforces_exp_buffer() {
    let secret = "test-secret";
    let agent_id = Uuid::new_v4();
    let config_id = Uuid::new_v4();
    let t = mint_command_token(secret, agent_id, config_id, &["health".into()], 60);
    let claims = validate_command_token(secret, &t, agent_id, config_id).unwrap();
    assert!(claims.scope.iter().any(|s| s == "health"));
    let min_remaining = claims.exp - (chrono::Utc::now().timestamp() as u64);
    assert!(min_remaining >= 60, "exp should include the buffer");
}
```

- [ ] **Step 2: Implement**

Create `CommandClaims { sub: agent_id, aud: config_id, scope: Vec<String>, exp, iat }`. Use the existing jsonwebtoken crate used elsewhere in the dashboard. Sign with `DASHBOARD_JWT_SECRET`. Validator checks: signature OK, exp > now + 60s guard, sub matches expected agent, aud matches expected config, verb is in scope.

- [ ] **Step 3: Run tests**

Run: `cargo test -p networker-dashboard command_token`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/auth.rs
git commit -m "feat(auth): mint + validate short-lived command tokens"
```

### Task 4: Dashboard dispatch_command - dispatch-side plumbing

**Files:**
- Create: `crates/networker-dashboard/src/services/agent_dispatch.rs`
- Modify: `crates/networker-dashboard/src/services/mod.rs` (add mod declaration)
- Test: integration test with an in-process mock WS client

- [ ] **Step 1: Test**

```rust
#[tokio::test]
async fn dispatch_command_returns_command_id_and_persists_row() {
    let (state, mock_ws_rx) = test_state_with_mock_agent().await;
    let agent_id = fixture_agent(&state).await;
    let cmd = dispatch_command(&state, agent_id, "health", serde_json::json!({}), None, Some(30)).await.unwrap();
    assert!(!cmd.command_id.is_nil());
    let sent = mock_ws_rx.recv().await.unwrap();
    matches!(sent, DashboardMessage::Command(_));
    let row = fetch_agent_command(&state, cmd.command_id).await.unwrap();
    assert_eq!(row.status, "pending");
    assert_eq!(row.verb, "health");
}
```

- [ ] **Step 2: Implement dispatch_command**

Signature: `async fn dispatch_command(state, agent_id, verb, args, config_id, timeout_secs) -> anyhow::Result<AgentCommand>`. Steps: 1) INSERT agent_command row (pending); 2) mint JWT with scope = [verb]; 3) build `AgentCommand` envelope; 4) look up the agent's WS sender via AppState's agent registry; 5) push envelope over WS; 6) return the `AgentCommand`.

- [ ] **Step 3: Receive-side handler**

In the agent WS handler, on receipt of `CommandLog`: append to `service_log` with service=`agent-command`, command_id in context. On receipt of `CommandResult`: UPDATE agent_command SET status/result/error_message/finished_at.

- [ ] **Step 4: Run tests**

Run: `cargo test -p networker-dashboard dispatch_command`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/services/agent_dispatch.rs crates/networker-dashboard/src/services/mod.rs
git commit -m "feat(dashboard): agent command dispatch service + result/log ingestion"
```

### Task 5: Agent-side command dispatcher (health verb only)

**Files:**
- Create: `crates/networker-agent/src/commands/mod.rs`
- Create: `crates/networker-agent/src/commands/health.rs`
- Modify: `crates/networker-agent/src/executor.rs` (route incoming Command messages)

- [ ] **Step 1: Test**

```rust
#[tokio::test]
async fn health_command_returns_ok_with_version() {
    let result = run_command("health", serde_json::json!({})).await;
    assert_eq!(result.status, CommandStatus::Ok);
    let r = result.result.unwrap();
    assert!(r.get("version").is_some());
    assert!(r.get("uptime_secs").is_some());
}
```

- [ ] **Step 2: Implement run_command dispatcher + health handler**

`run_command` validates the token (using the shared crate once extracted) then dispatches to a handler by verb. Handler for `health` returns `{ "version": env!("CARGO_PKG_VERSION"), "os": ..., "uptime_secs": ..., "disk_free_mb": ... }`.

- [ ] **Step 3: Wire into agent message loop**

In the agent's WS loop, when a `DashboardMessage::Command(cmd)` arrives: spawn a task that calls `run_command(cmd.verb, cmd.args)` and sends back `CommandResult` + streamed `CommandLog`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p networker-agent health_command`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/networker-agent/src/commands/ crates/networker-agent/src/executor.rs
git commit -m "feat(agent): command dispatcher + health verb"
```

### Task 6: REST endpoint to dispatch commands

**Files:**
- Create: `crates/networker-dashboard/src/api/agent_commands.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs` (register router)

- [ ] **Step 1: Test**

```rust
#[tokio::test]
async fn dispatch_command_endpoint_happy_path() {
    // Fixture a project + agent; POST /projects/{pid}/agents/{aid}/commands
    // Body: {"verb":"health","args":{}}
    // Expect: 202, returns command_id
}
```

- [ ] **Step 2: Implement POST /api/projects/{pid}/agents/{aid}/commands**

Body: `{verb, args, config_id?, timeout_secs?}`. Requires ProjectRole::Operator. Calls `dispatch_command`. Returns 202 with `{command_id}`.

- [ ] **Step 3: Implement GET /api/projects/{pid}/commands/{cid}**

Reads agent_command row. 404 if not found.

- [ ] **Step 4: Implement GET /api/projects/{pid}/commands/{cid}/stream (SSE)**

Streams service_log entries for this command_id + a final event when `finished_at` is set.

- [ ] **Step 5: Run tests**

Run: `cargo test -p networker-dashboard dispatch_command_endpoint`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/networker-dashboard/src/api/agent_commands.rs crates/networker-dashboard/src/api/mod.rs
git commit -m "feat(api): POST/GET /agents/{aid}/commands + SSE stream"
```

### Task 7: E2E smoke - round-trip health command

**Files:**
- Test: `crates/networker-dashboard/tests/e2e_agent_command.rs`

- [ ] **Step 1: Write the test**

```rust
#[tokio::test]
async fn health_command_round_trip() {
    let harness = TestHarness::start().await;
    let agent = harness.spawn_agent().await;
    let cmd = harness.dispatch("health", json!({})).await;
    let result = harness.wait_for_result(cmd.command_id).await;
    assert_eq!(result.status, "ok");
    assert!(result.result.get("version").is_some());
}
```

- [ ] **Step 2: Run**

Run: `cargo test -p networker-dashboard --test e2e_agent_command`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/networker-dashboard/tests/e2e_agent_command.rs
git commit -m "test(dashboard): e2e agent command round-trip (health)"
```

---

## Phase 2 - Core verbs + Linux + Windows via cloud-init

### Task 8: Bootstrap script generator

**Files:**
- Create: `crates/networker-dashboard/src/services/cloud_init.rs`

- [ ] **Step 1: Test - templates produce valid scripts**

```rust
#[test]
fn bootstrap_sh_embeds_api_key_and_url() {
    let s = render_linux_bootstrap("https://alethedash.com", "apikey12345", "x86_64-unknown-linux-musl");
    assert!(s.contains("AGENT_DASHBOARD_URL=https://alethedash.com"));
    assert!(s.contains("AGENT_API_KEY=apikey12345"));
    assert!(s.contains("networker-agent-x86_64-unknown-linux-musl"));
}

#[test]
fn bootstrap_ps1_uses_invoke_webrequest() {
    let s = render_windows_bootstrap("https://alethedash.com", "apikey", "x86_64-pc-windows-msvc");
    assert!(s.contains("Invoke-WebRequest"));
    assert!(s.contains("$env:AGENT_API_KEY = 'apikey'"));
    assert!(s.contains("networker-agent-x86_64-pc-windows-msvc"));
}
```

- [ ] **Step 2: Implement render_linux_bootstrap + render_windows_bootstrap**

Escape user inputs (API key is base64-safe already; URL must be validated - reject shell metacharacters). Templates mirror the spec's bootstrap.sh example. Windows template installs via `New-Service` and sets machine-wide env vars via `setx /M`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p networker-dashboard cloud_init`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/networker-dashboard/src/services/cloud_init.rs
git commit -m "feat(cloud-init): bootstrap script templates (Linux + Windows)"
```

### Task 9: AWS cloud-init via --user-data

**Files:**
- Modify: `crates/networker-dashboard/src/services/cloud_provider.rs` (AwsProvider::create_vm)
- Test: unit test for user-data injection

- [ ] **Step 1: Test**

```rust
#[test]
fn aws_create_vm_args_include_user_data_when_bootstrap_set() {
    let config = VmConfig { bootstrap_script: Some("#!/bin/bash\necho hi".into()), ..fixture() };
    let args = AwsProvider::build_run_instances_args(&config);
    let idx = args.iter().position(|a| a == "--user-data").expect("--user-data arg present");
    assert!(args[idx+1].contains("echo hi"));
}
```

- [ ] **Step 2: Add bootstrap_script field to VmConfig**

```rust
pub struct VmConfig {
    // existing fields...
    pub bootstrap_script: Option<String>,
}
```

- [ ] **Step 3: Refactor AWS create_vm**

Extract a `build_run_instances_args(&self, config)` pure function. When `config.bootstrap_script.is_some()`, append `--user-data` with the script contents. Use `file://` with a temp file to avoid arg length limits.

- [ ] **Step 4: Run tests**

Run: `cargo test -p networker-dashboard aws_create_vm_args`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/networker-dashboard/src/services/cloud_provider.rs
git commit -m "feat(cloud): AWS --user-data bootstrap support"
```

### Task 10: GCP cloud-init via --metadata-from-file startup-script

Same pattern as Task 9 but for GCP; `--metadata-from-file startup-script=/tmp/bootstrap-<id>.sh`. Include test + implementation + commit.

### Task 11: Azure cloud-init via --custom-data

Same pattern for Azure; `--custom-data "@/tmp/bootstrap-<id>.sh"`. Azure also supports `--user-data` for some image types; use `--custom-data` as the universal path. Include test + implementation + commit.

### Task 12: Wire cloud-init into persistent tester create

**Files:**
- Modify: `crates/networker-dashboard/src/api/testers.rs` (create handler)
- Modify: `crates/networker-dashboard/src/services/tester_install.rs`

- [ ] **Step 1: Provision the agent row first (create API key)**

Move `provision_agent_for_tester` to happen BEFORE cloud VM creation so the api_key is available for cloud-init.

- [ ] **Step 2: Build bootstrap script + pass to VmConfig**

```rust
let bootstrap = cloud_init::render_linux_bootstrap(
    &state.public_url, &agent_api_key, os_info.release_target()
);
let vm_config = VmConfig { bootstrap_script: Some(bootstrap), ..vm_config };
```

- [ ] **Step 3: Replace install_tester_with_options with a "wait for agent" poll**

After VM create, poll `SELECT status FROM agent WHERE agent_id = $1` until `online` or 180s timeout. No SSH.

- [ ] **Step 4: Retain --ssh-bootstrap flag on the create API as a debug fallback**

Query param `?ssh_bootstrap=1` uses the old path. Default = cloud-init.

- [ ] **Step 5: Integration test with a real GCP VM**

Run the existing 12-combo test matrix. Expect Linux combos to come up without any SSH call from the dashboard.

- [ ] **Step 6: Commit**

```bash
git add crates/networker-dashboard/src/api/testers.rs crates/networker-dashboard/src/services/tester_install.rs
git commit -m "feat(testers): cloud-init bootstrap; SSH install path behind ?ssh_bootstrap=1"
```

### Task 13: Core verbs - install_server, start_server, stop_server, collect_logs

**Files:**
- Create: `crates/networker-agent/src/commands/install_server.rs`
- Create: `crates/networker-agent/src/commands/server_lifecycle.rs`
- Create: `crates/networker-agent/src/commands/collect_logs.rs`

One sub-step per verb, with a test for each:

- [ ] **Step 1: install_server(language, version?)** - detects OS family, apt/dnf/choco install, idempotent (no-op if already installed). Test with `language="nginx"` against a Linux container fixture.
- [ ] **Step 2: start_server(language, port, env?)** - spawns via systemd user unit on Linux, via sc.exe service on Windows. Returns `{pid, port, started_at}`.
- [ ] **Step 3: stop_server(language)** - `systemctl --user stop` or `sc.exe stop`. Idempotent.
- [ ] **Step 4: collect_logs(paths?, max_bytes?)** - reads paths (defaults to systemd journal via `journalctl -u <service>`), returns last N KB.
- [ ] **Step 5: Commit each incrementally**

```bash
git commit -m "feat(agent): install_server verb"
git commit -m "feat(agent): start_server + stop_server verbs"
git commit -m "feat(agent): collect_logs verb"
```

### Task 14: Orchestrator refactor - alethabench uses dispatch_command

**Files:**
- Create: `benchmarks/orchestrator/src/dispatch.rs` (HTTP client to dashboard)
- Modify: `benchmarks/orchestrator/src/executor.rs` (replace ssh_exec calls)

- [ ] **Step 1: HTTP client wrapping POST /api/projects/{pid}/agents/{aid}/commands**

`async fn dispatch(agent_id, verb, args) -> CommandResult`. Polls status until finished. 20-min hard timeout.

- [ ] **Step 2: Replace ssh call sites one at a time**

Current call sites in executor.rs: 233, 431, 460, 462, 469, 490, 522, 537, 557, 568, 621, 630, 847, 2070. For each, substitute `dispatch(vm.agent_id, "start_server", json!({...}))` etc.

- [ ] **Step 3: Feature-flag during rollout**

Env `ORCH_USE_AGENT_COMMANDS=1` selects new path, else keeps old SSH. Lets us A/B compare.

- [ ] **Step 4: Run the 12-combo matrix with flag on**

All Linux combos must succeed.

- [ ] **Step 5: Commit**

```bash
git add benchmarks/orchestrator/src/dispatch.rs benchmarks/orchestrator/src/executor.rs
git commit -m "feat(orchestrator): dispatch commands via dashboard instead of SSH (feature flag)"
```

### Task 15: Windows agent build + cloud-init

**Files:**
- Modify: `.github/workflows/release.yml` (ensure Windows agent binary in release)
- Modify: `crates/networker-dashboard/src/services/cloud_provider.rs` (Windows cloud-init args)

- [ ] **Step 1: Verify `networker-agent-x86_64-pc-windows-msvc.tar.gz` is published by release CI**
- [ ] **Step 2: AWS Windows - `--user-data` takes `<powershell>...</powershell>`**
- [ ] **Step 3: GCP Windows - `--metadata-from-file windows-startup-script-ps1=...`**
- [ ] **Step 4: Azure Windows - `--custom-data @bootstrap.ps1` with base64 encode for Windows**
- [ ] **Step 5: Re-run the 3 Windows combos end to end**
- [ ] **Step 6: Commit**

```bash
git commit -m "feat(cloud): Windows bootstrap via PowerShell user-data / startup-script / custom-data"
```

### Task 16: Wizard - per-testbed Client Tester selector

**Files:**
- Modify: `dashboard/src/pages/BenchmarkWizardPage.tsx`
- Modify: `crates/networker-dashboard/src/db/benchmark_testbeds.rs` (add tester_id column via V034)
- Modify: `crates/networker-dashboard/src/api/benchmark_configs.rs` (accept + persist tester_id)

- [ ] **Step 1: V034 migration** - `ALTER TABLE benchmark_testbed ADD COLUMN tester_id UUID REFERENCES project_tester(tester_id) ON DELETE SET NULL;`
- [ ] **Step 2: API body + DB persistence** - `payload.testbeds[i].tester_id` round-trips to the DB.
- [ ] **Step 3: Wizard UI** - under each testbed's "existing" row, add a second dropdown "Client Tester" listing persistent testers in the same region (or "auto = orchestrator host").
- [ ] **Step 4: benchmark_worker** - resolve `tester_id` to agent_id; include in config JSON under `testbeds[i].tester_agent_id`.
- [ ] **Step 5: Orchestrator** - when `tester_agent_id` is set, dispatch `run_probe` there; otherwise run `networker-tester` locally (back-compat).
- [ ] **Step 6: Commit each chunk separately**

### Task 17: Probe verbs - run_probe, run_browser_probe, upload_artifact

**Files:**
- Create: `crates/networker-agent/src/commands/run_probe.rs`
- Create: `crates/networker-agent/src/commands/run_browser_probe.rs`
- Create: `crates/networker-agent/src/commands/upload_artifact.rs`

- [ ] **Step 1: run_probe** - spawns `/usr/local/bin/networker-tester` with args, captures JSON stdout, returns parsed structure unchanged.
- [ ] **Step 2: run_browser_probe** - invokes `/opt/bench/chrome-harness` runner with args, same JSON return.
- [ ] **Step 3: upload_artifact** - PUTs a file to a presigned URL (S3/GCS/Azure Blob). Dashboard mints the presigned URL and passes it.
- [ ] **Step 4: Orchestrator converts existing probe invocation to dispatch**
- [ ] **Step 5: Commit per verb**

---

## Phase 3 - Remove SSH (except debug)

### Task 18: Gate SSH behind debug flag

**Files:**
- Modify: `benchmarks/orchestrator/src/ssh.rs`, `benchmarks/orchestrator/src/executor.rs`
- Modify: `crates/networker-dashboard/src/services/tester_install.rs`

- [ ] **Step 1: Compile-time cfg(feature = "ssh_debug")** - hide ssh module behind feature flag.
- [ ] **Step 2: Runtime gate** - `ORCH_DEBUG_SSH=1` required to use SSH paths.
- [ ] **Step 3: All tests pass with feature off**
- [ ] **Step 4: Commit**

```bash
git commit -m "chore: gate SSH orchestration behind debug feature flag"
```

### Task 19: Delete ssh.rs + related

**Files:**
- Delete: `benchmarks/orchestrator/src/ssh.rs`
- Delete: SSH code paths in `tester_install.rs`
- Keep: `debug_ssh.rs` helper for incident response only, documented as such.

- [ ] **Step 1: Delete files + callers**
- [ ] **Step 2: CI passes**
- [ ] **Step 3: Commit**

```bash
git commit -m "chore: remove SSH orchestration paths; keep debug_ssh for incident use only"
```

### Task 20: Update CLAUDE.md + README

**Files:**
- Modify: `CLAUDE.md` (orchestration section)
- Modify: `README.md`

- [ ] **Step 1: Document the command protocol, cloud-init bootstrap, tester/endpoint distinction**
- [ ] **Step 2: Commit**

```bash
git commit -m "docs: orchestration is command-based; SSH removed"
```

---

## Verification gates (run at phase boundaries)

- End of Phase 1: `health` round-trips in <1s from dispatch to result.
- End of Phase 2: 12-combo benchmark matrix (3 clouds x 2 OS x 2 variants) all succeed with `ORCH_USE_AGENT_COMMANDS=1`. Windows combos work via PowerShell bootstrap.
- End of Phase 3: `grep -rn "ssh_exec\|ssh::" crates benchmarks` returns only the `debug_ssh` path.


---

## Phase 2.5 — Packet capture + control-plane isolation (port-based)

**Decision**: port-based capture filter first. Multi-NIC provisioning deferred. Verify via a bit-for-bit packet count match between `networker-tester`'s own counters and the resulting pcap (within ±1%) before shipping.

### Task 13b — Install Wireshark CLI (tshark) in cloud-init

**Files:**
- Modify: `crates/networker-dashboard/src/services/cloud_init.rs` (Linux + Windows templates)

- [ ] **Step 1: Linux — add tshark to apt/dnf install in bootstrap.sh**

```bash
# Add to render_linux_bootstrap
export DEBIAN_FRONTEND=noninteractive
echo "wireshark-common wireshark-common/install-setuid boolean true" | debconf-set-selections
apt-get install -y tshark || dnf install -y wireshark-cli
# Allow non-root capture via setcap on dumpcap
setcap cap_net_raw,cap_net_admin=eip /usr/bin/dumpcap 2>/dev/null || \
  setcap cap_net_raw,cap_net_admin=eip /usr/sbin/dumpcap || true
usermod -aG wireshark ubuntu 2>/dev/null || true
```

- [ ] **Step 2: Windows — install Wireshark via Chocolatey in bootstrap.ps1**

```powershell
# Add to render_windows_bootstrap
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Set-ExecutionPolicy Bypass -Scope Process -Force
    iex ((New-Object System.Net.WebClient).DownloadString('https://chocolatey.org/install.ps1'))
}
choco install -y wireshark --params '/NoDesktopIcon /NoInstallNpcap'
choco install -y npcap --params '/WinPcapMode=no /LoopbackSupport=yes'
# Add tshark to PATH for the agent service
[Environment]::SetEnvironmentVariable("Path", $env:Path + ";C:\Program Files\Wireshark", "Machine")
```

- [ ] **Step 3: Test — bootstrap script renders with tshark install lines**

```rust
#[test]
fn linux_bootstrap_installs_tshark() {
    let s = render_linux_bootstrap("https://x", "k", "x86_64-unknown-linux-musl");
    assert!(s.contains("apt-get install -y tshark") || s.contains("wireshark-cli"));
    assert!(s.contains("setcap cap_net_raw"));
}

#[test]
fn windows_bootstrap_installs_wireshark() {
    let s = render_windows_bootstrap("https://x", "k", "x86_64-pc-windows-msvc");
    assert!(s.contains("choco install -y wireshark"));
}
```

- [ ] **Step 4: Run tests + commit**

```bash
cargo test -p networker-dashboard cloud_init
git commit -m "feat(cloud-init): install tshark/Wireshark + capabilities for non-root capture"
```

### Task 13c — run_probe verb honors capture_mode + excludes control plane

**Files:**
- Create: `crates/networker-agent/src/commands/run_probe.rs`
- Modify: `crates/networker-agent/src/commands/mod.rs`

- [ ] **Step 1: Resolve dashboard host/port at agent startup**

Parse `AGENT_DASHBOARD_URL` once at agent start:
```rust
let (dashboard_host, dashboard_port) = parse_url(&env::var("AGENT_DASHBOARD_URL")?)?;
// Cache DNS resolution too — any IP in the A/AAAA set
let dashboard_ips: Vec<IpAddr> = resolve(&dashboard_host).await?;
```

Save to agent state so command handlers can access it.

- [ ] **Step 2: When `run_probe.capture_mode != None`, build tshark filter excluding control plane**

```rust
// Pre-capture: construct BPF filter that EXCLUDES control plane
let mut excludes = vec![format!("port {}", dashboard_port)];
for ip in dashboard_ips {
    excludes.push(format!("host {}", ip));
}
// Also exclude DNS lookups of the dashboard host
excludes.push(format!("(udp port 53 and (host {}))", dashboard_host));
let bpf_filter = format!("not ({})", excludes.join(" or "));
// -f "<BPF>" — kernel-level filter; control-plane packets never reach tshark
let args = ["-i", "any", "-f", &bpf_filter, "-w", &pcap_path, /* ... */];
```

This is a **kernel-level BPF filter** — control plane packets are dropped by the capture socket before reaching userspace. Zero overhead, zero pollution.

- [ ] **Step 3: Test — filter excludes dashboard IP + port**

```rust
#[test]
fn capture_filter_excludes_dashboard_traffic() {
    let f = build_capture_filter("alethedash.com", 443, &["1.2.3.4".parse().unwrap()]);
    assert!(f.contains("port 443"));
    assert!(f.contains("host 1.2.3.4"));
    assert!(f.starts_with("not ("));
}
```

- [ ] **Step 4: Verification gate — data-plane parity test**

Run a known probe (e.g. 100 HTTP/1 requests to a test endpoint) with capture on. Assert:
```rust
let captured = tshark_count_packets(&pcap_path, "tcp.port == TEST_PORT");
let reported = probe_result.requests_sent * 2; // req + resp
assert!((captured as i64 - reported as i64).abs() <= reported / 100, "±1% parity required");
```

If it fails: filter is too aggressive (dropping probe traffic) or too loose (including control). Fix before proceeding.

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(agent): run_probe honors capture_mode with dashboard-exclusion BPF filter"
```

### Task 17b — pcap artifact upload via presigned URL

**Files:**
- Create: `crates/networker-dashboard/src/api/artifact_urls.rs` (mint presigned URL)
- Modify: `crates/networker-agent/src/commands/run_probe.rs` (upload on success)

- [ ] **Step 1: Dashboard endpoint `POST /api/projects/{pid}/artifacts/presign`**

Body: `{config_id, run_id, kind: "pcap"}`. Returns `{url, expires_at}` for S3/GCS/Azure Blob (based on project's default object store). Operator role required.

- [ ] **Step 2: Agent uploads pcap after probe completes**

In `run_probe`, after the probe finishes and pcap is flushed:
```rust
let presign = dispatch_sub_command("artifact_presign", json!({"config_id":..., "run_id":..., "kind":"pcap"})).await?;
upload_to_presigned_url(&pcap_path, &presign.url).await?;
// Store the URL (not the local path) in the probe result
result["pcap_url"] = presign.url;
```

- [ ] **Step 3: Dashboard stores `pcap_url` into `benchmark_run.pcap_path` (existing column)**

- [ ] **Step 4: Test — E2E from dispatch → upload → URL stored**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat: pcap artifact upload via presigned URL; referenced in benchmark_run.pcap_path"
```

### Task 22 — UI: capture toggle wired + download pcap link

**Files:**
- Modify: `dashboard/src/pages/BenchmarkWizardPage.tsx` (already has the Capture dropdown — wire it to persist)
- Modify: `dashboard/src/pages/BenchmarkDetailPage.tsx` (add download link per run)

- [ ] **Step 1: Wizard — Capture dropdown state flows into payload.testbeds[i].capture_mode**

Already in the UI as dropdown `Disabled | Tester-side`; currently unwired. Add `captureMode` to testbed state; send as `capture_mode: "tester" | "disabled"`.

- [ ] **Step 2: API — accept capture_mode on benchmark create + persist to benchmark_testbed**

Add `capture_mode TEXT` column via V035 migration on `benchmark_testbed`. Orchestrator dispatches `run_probe` with the saved mode.

- [ ] **Step 3: Run detail — show "Download pcap" button when `pcap_path` is non-null**

```tsx
{run.pcap_path && (
  <a href={run.pcap_path} download className="text-cyan-400">
    Download packet capture (.pcap) · {formatBytes(run.pcap_bytes)}
  </a>
)}
```

- [ ] **Step 4: Test — RTL + API integration tests**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(ui): packet capture toggle + download link on run detail"
```

### Task 23 — Data-plane impact test (verification gate)

Must pass before Phase 2.5 ships.

**Files:**
- Create: `tests/integration_capture_parity.rs`

- [ ] **Step 1: Run a 100-request HTTP/1 benchmark against a controlled test endpoint**
- [ ] **Step 2: Parse the resulting pcap with tshark**
- [ ] **Step 3: Assert packet count within ±1% of what networker-tester logged**
- [ ] **Step 4: Assert control-plane IP is NOT in the pcap (`tshark -r ... -Y "ip.addr == dashboard_ip"` returns 0)**
- [ ] **Step 5: Commit**

```bash
git commit -m "test: pcap data-plane parity + control-plane exclusion"
```

---

## Deferred — Multi-NIC (Phase 2.5b)

Not blocking the launch. Only escalate if data-plane parity test fails or user reports control-plane pollution in captures.

Scope when triggered:
- Add `project_tester.control_ip` + `project_tester.data_ip` columns.
- Per-cloud `create_vm` multi-NIC paths:
  - AWS — attach 2nd ENI after instance-running.
  - GCP — `--network-interface` twice (need 2 VPCs or alias IP ranges).
  - Azure — pre-create 2 NICs, `--nics nic1 nic2`. Requires ≥ D2s_v3.
- Cloud-init binds agent WS to NIC1; data traffic defaults to NIC2.
- Capture filter becomes `tshark -i eth1` (no BPF needed).

