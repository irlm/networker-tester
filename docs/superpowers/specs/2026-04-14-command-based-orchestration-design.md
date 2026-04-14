---
title: Command-based orchestration (replace SSH with agent commands)
status: draft
owner: dashboard
related: crates/networker-dashboard, crates/networker-agent, benchmarks/orchestrator
---

# Command-based orchestration

## Problem

Today the dashboard uses three different mechanisms to "do things to a VM":

1. **Cloud CLI** (az vm start, gcloud compute instances create, aws ec2 terminate-instances) for VM lifecycle. Works.
2. **SSH** for installing binaries onto a fresh VM (tester_install.rs).
3. **SSH** (inside alethabench) for everything the orchestrator does at benchmark time - start/stop nginx, deploy language servers, run probes, collect logs.

(2) and (3) are fragile:

- Per-cloud SSH user mismatch (azureuser / ubuntu / admin / ec2-user) - every cloud needs different assumptions, silently breaks when one image changes defaults.
- Windows doesn't ship OpenSSH - forces a second protocol (WinRM) or skipping Windows entirely.
- Key propagation delays (GCP metadata keys, EC2 key pairs) block benchmarks for minutes after provision.
- NAT/firewall - requires inbound SSH port, limits where testers can live.
- Orchestrator commands often need sudo - requires passwordless sudoers rules, a failure vector.
- SSH session state (ControlMaster sockets, known_hosts) is process-local and leaks across runs.
- Audit trail is stderr-shaped, not a queryable record.

Symptom: we hit all of these in a 12-VM test matrix. Each cloud+OS combo had its own class of failure.

## Goal

Replace SSH-driven orchestration with a command protocol so:

- Same protocol across Linux and Windows.
- No key or sudoer management.
- Every action is an auditable, typed, DB-logged event.
- Works with NAT / private VMs (agents initiate the connection).
- Windows works day one.

## Scope

In scope:

- VM lifecycle (start / stop / delete): remains cloud-CLI-driven from the dashboard. No change.
- Benchmark / test orchestration inside a running VM: moves from SSH to agent commands.
- Initial bootstrap (install networker-agent on a freshly-provisioned VM): moves from SSH to cloud-init / user-data / custom-data.
- Auth model for commands: documented below.

Out of scope (future work):

- Removing SSH entirely. Keep it as a debug-only fallback behind a flag.
- Cross-region agent mesh / agent-to-agent traffic. Not needed - all traffic flows through the dashboard.
- Kubernetes-style control plane. The agent WebSocket is sufficient.

## Architecture

```
+----------------------+        HTTPS (WSS)       +-----------------------+
|      Dashboard       |<-- agent registers ------|  Tester agent         |
|                      |    (long-lived API key)  |  (long-lived client)  |
|   - cloud CLI driver |                          |   - runs probes       |
|   - job router       |-- pushes Job over WS --> |   - runs Chrome tests |
|   - benchmark worker |                          |   - reports results   |
|                      |<-- streams logs/results -|                       |
+----------+-----------+                          +-----------------------+
           |                                      +-----------------------+
           +-- pushes Job over WS --------------> |  Endpoint agent       |
                                                  |  (per-test SERVER)    |
                                                  |   - installs nginx    |
                                                  |   - starts/stops apps |
                                                  +-----------------------+
           |
           | (direct cloud CLI calls from dashboard process)
           v
   az vm start / stop / delete
   aws ec2 start-instances / stop-instances / terminate-instances
   gcloud compute instances start / stop / delete
```

Two command channels, each with a clear owner:

| Channel | Owner | Purpose | Direction |
|---------|-------|---------|-----------|
| **Cloud CLI** | Dashboard to cloud API | VM power state + delete | Dashboard to cloud (outbound) |
| **Agent Commands** | Dashboard to Agent over WS | Everything inside a running VM | Dashboard <-> Agent (agent-initiated WS) |

## Agent command protocol

A command is a typed JSON message on the existing agent WebSocket. The agent is already connected to the dashboard - no inbound connections required.

### Wire format

```
{
  "type": "command",
  "command_id": "cmd-uuid",
  "config_id": "bench-uuid",
  "token": "<JWT, exp = config.max_duration_secs + 300>",
  "verb": "start_server",
  "args": {
    "language": "nginx",
    "port": 8080
  }
}
```

Response messages (streamed):

```
{"type":"command_log","command_id":"cmd","stream":"stdout","line":"..."}
{"type":"command_result","command_id":"cmd","status":"ok","result":{"pid":1234}}
{"type":"command_result","command_id":"cmd","status":"error","error":"bind 8080: already in use"}
```

### Initial verb set

Minimum set required to retire SSH for benchmarks:

| Verb | Args | Used by |
|------|------|---------|
| install_prereqs | {os_family, packages?} | tester install (post-bootstrap) |
| install_server | {language, version?} | endpoint setup |
| start_server | {language, port, env?} | per-language benchmark step |
| stop_server | {language} | cleanup |
| run_probe | {target, modes, runs, timeout} | tester executes networker-tester |
| run_browser_probe | {target, modes, runs} | tester runs chrome-harness |
| collect_logs | {paths?, max_bytes?} | any VM |
| upload_artifact | {path, presigned_url} | tester uploads screenshots / traces |
| health | {} | liveness probe |
| shutdown_self | {} | graceful shutdown before cloud stop |

Every verb is idempotent where it makes sense (start_server is a no-op if already running; stop_server never errors on "not running"). Every verb streams logs and returns a structured result.

### Transport details

- Existing WebSocket session (AGENT_API_KEY long-lived, AGENT_DASHBOARD_URL target) stays.
- New command / command_log / command_result message types added alongside existing Heartbeat, RunResult variants in networker-common::messages.
- Concurrency: one command per agent at a time; queued on the agent side.
- Timeout: every command carries timeout_secs. Server enforces, agent enforces. If either hits, command returns status: timeout.
- Cancellation: dashboard can send {type:"cancel",command_id}. Agent SIGTERMs the running process.

## Auth model

Two layers - long-lived identity, short-lived authorization.

### Agent identity (long-lived)

- agent.api_key: 48-char random string, stored encrypted at rest, never rotated automatically.
- Issued when the VM is provisioned; baked into the agent's systemd Environment= lines via cloud-init.
- Used by the agent to authenticate its WebSocket connection.
- Rotated only on user action ("rotate API key" button, forces agent reconnect).
- This is the existing agent table column.

### Job authorization (short-lived)

Per-command JWT signed by DASHBOARD_JWT_SECRET with claims:

```
{
  "sub": "<agent_id>",
  "aud": "<benchmark_config_id OR job_id>",
  "scope": ["start_server", "stop_server", "run_probe", "collect_logs"],
  "exp": now + max_duration_secs + 300,
  "iat": now
}
```

- Minted by the dashboard when dispatching a command.
- Validated by the agent on every incoming command (signature + exp + aud match the command's config_id).
- Agent rejects commands whose token:
  - has exp - now < 60s (too close to expiry to run anything meaningful)
  - scope doesn't include the verb
  - aud doesn't match the command's declared config/job ID
- Dashboard logic before dispatch: if token.exp < now + estimated_remaining_work + 60s, re-mint.

This matches the requirement: check before running a command that the token has enough time; re-mint before dispatch if not. Done by the dashboard at dispatch time, not the agent at receive time. The agent is only a gate.

### Security properties

- Compromised api_key -> attacker can connect one agent, but can't issue commands without a JWT signed by the dashboard.
- Compromised JWT -> scoped to one benchmark run, expires automatically.
- Dashboard is the sole auth authority - agents never trust each other; no lateral movement.
- Revocation: flag agent.status = 'revoked'; dashboard drops WS + refuses to mint new JWTs. Existing JWTs expire within minutes.

## Bootstrap (replacing install SSH)

Replace install_tester_with_options SSH script with cloud-init / user-data.

- AWS: run-instances --user-data file://bootstrap.sh
- GCP: compute instances create --metadata-from-file startup-script=bootstrap.sh
- Azure: vm create --custom-data @bootstrap.sh

bootstrap.sh (templated per-VM before upload):

```
#!/bin/bash
set -e
TAG="$(curl -fsSL https://api.github.com/repos/irlm/networker-tester/releases/latest | jq -r .tag_name)"
TARGET="x86_64-unknown-linux-musl"  # templated based on arch
curl -fsSL "https://github.com/irlm/networker-tester/releases/download/$TAG/networker-agent-$TARGET.tar.gz" \
  | tar xz -C /usr/local/bin/
curl -fsSL "https://github.com/irlm/networker-tester/releases/download/$TAG/networker-tester-$TARGET.tar.gz" \
  | tar xz -C /usr/local/bin/

cat > /etc/systemd/system/networker-agent.service <<'UNIT'
[Unit]
Description=Networker Agent
After=network.target
[Service]
Type=simple
ExecStart=/usr/local/bin/networker-agent
Restart=on-failure
Environment=AGENT_DASHBOARD_URL=__DASHBOARD_URL__
Environment=AGENT_API_KEY=__API_KEY__
[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable --now networker-agent
```

The dashboard templates __DASHBOARD_URL__ and __API_KEY__ before passing the script to the cloud CLI. VM boots, agent starts, registers, accepts commands. Zero SSH.

Windows variant: bootstrap.ps1 using PowerShell - same idea, Invoke-WebRequest + New-Service. Same command protocol once running - identical orchestration path.

Fallback: if cloud-init isn't available or bootstrap failed, a debug-only --ssh-bootstrap CLI flag re-enables the old SSH install.

## Dashboard changes

### New / changed API

| Method | Path | Purpose |
|--------|------|---------|
| POST | /api/projects/{pid}/agents/{aid}/commands | Dispatch a command to an agent (used by orchestrator). Mints JWT, pushes WS message, returns command_id. |
| GET | /api/projects/{pid}/commands/{cid} | Poll command status + logs (for UI + orchestrator). |
| GET | /api/projects/{pid}/commands/{cid}/stream | SSE stream of logs + final result. |

### Schema

Single new table; logs stay in the existing service_log if present.

```
CREATE TABLE agent_command (
  command_id     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  agent_id       UUID NOT NULL REFERENCES agent(agent_id) ON DELETE CASCADE,
  config_id      UUID,  -- benchmark_config_id OR job_id; nullable for ad-hoc
  verb           TEXT NOT NULL,
  args           JSONB NOT NULL DEFAULT '{}'::jsonb,
  status         TEXT NOT NULL DEFAULT 'pending', -- pending|running|ok|error|timeout|cancelled
  result         JSONB,
  error_message  TEXT,
  created_by     UUID REFERENCES dash_user(user_id),
  created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  started_at     TIMESTAMPTZ,
  finished_at    TIMESTAMPTZ
);
CREATE INDEX idx_agent_command_agent ON agent_command(agent_id, created_at DESC);
CREATE INDEX idx_agent_command_config ON agent_command(config_id) WHERE config_id IS NOT NULL;
```

Command logs go to service_log (existing) with service = 'agent-command' and command_id in the context JSONB.

### benchmark_worker / alethabench changes

- benchmark_worker.rs: for each testbed, resolve the endpoint's agent_id (from benchmark_vm_catalog.agent_id - a new column) and the tester's agent_id (if one is selected). Pass both to alethabench via env or the config JSON.
- alethabench executor: replace ssh::ssh_exec(&vm.ip, cmd) with dashboard::dispatch_command(agent_id, verb, args). One function per current SSH call site.
- Keep the result-parsing logic - it already assumes JSON output from networker-tester; commands just return the same JSON structure via command_result.

## Migration plan

Phased - each phase lands independently and both old and new paths coexist.

### Phase 1 - Protocol + infrastructure

- Add command / command_log / command_result message variants to networker-common.
- Add agent_command table (V033).
- Implement agent-side command dispatcher with the health verb only (smoke test).
- Dashboard endpoint POST /agents/{aid}/commands that round-trips a health and returns the result.
- Tests: agent-side unit tests + integration test that hits health on a live agent.

### Phase 2 - Core verbs

- Implement install_prereqs, install_server, start_server, stop_server, collect_logs on the agent.
- Map each current ssh_exec call site in alethabench::executor to the corresponding verb. Feature-flag - new path only when ORCH_USE_AGENT_COMMANDS=1.
- Run the existing benchmark test matrix with both paths; compare result parity.

### Phase 3 - Probe verbs

- Implement run_probe, run_browser_probe, upload_artifact.
- These replace the local-to-orchestrator networker-tester subprocess with a remote tester VM running the same binary.
- This is where "tester = client, endpoint = server" becomes real - the orchestrator dispatches run_probe to the tester's agent.

### Phase 4 - Cloud-init bootstrap

- Template bootstrap.sh / bootstrap.ps1 generator.
- Swap install_tester_with_options to cloud-init path per-cloud (AWS -> GCP -> Azure -> Windows).
- Keep --ssh-bootstrap flag as fallback.
- Removal target: 2 releases after all clouds working cleanly on cloud-init.

### Phase 5 - SSH removal

- Delete benchmarks/orchestrator/src/ssh.rs (except a thin debug wrapper).
- Delete SSH-driven code from tester_install.rs.
- Keep debug ssh as a manual one-shot for incident response.

## Decisions needed

1. **Endpoint agent**: does every endpoint (the per-test server VM) run networker-agent too? Recommend yes - it's a one-time 30-second install via cloud-init and buys us the same orchestration protocol for server-side steps.
2. **Client-tester selection on benchmark**: does the benchmark config carry one tester_id (one client per benchmark) or tester_id per testbed (one client per endpoint)? Recommend per-testbed - enables co-located or distant-client topologies.
3. **Windows Phase-2 priority**: do we stand up Windows via cloud-init / PowerShell in Phase 2 or Phase 4? Recommend Phase 2 - it's the biggest payoff of this whole refactor (Windows works for free).

## What doesn't change

- VM lifecycle (start / stop / delete) stays as cloud CLI calls from the dashboard. No agent involvement.
- The dashboard's public API shape (users see /api/projects/.../benchmark-configs etc. unchanged).
- The networker-tester binary itself - it's still the probe tool, just run remotely via a command.
- The project_tester table shape - just wired to an agent_id.
