# Resumption note — Command-based orchestration

## Status (2026-04-14, end of session)

**Phase 1 (Tasks 1-6) — DONE.** All 6 commits on branch `fix/installer-rewrite`.
**Phase 2 — STARTED.** Task 8 (cloud-init bootstrap generator) done. Tasks 9-17 + packet-capture sub-tasks pending.

## Commits in this session (newest first)

```
909e9b7  feat(cloud-init): bootstrap script generator (Linux bash + Windows PowerShell)
9f0f74d  feat(api): POST /agents/{aid}/commands + GET /commands/{cid} + SSE stream
9d429f1  feat(agent): command dispatcher + health verb
b03a4d6  feat(dashboard): agent command dispatch service + result/log ingestion
e7747da  fix(cloud): treat 'already gone' as success on delete_vm
938d67a  feat(auth): mint + validate short-lived command tokens
fd410d7  feat(db): V033 agent_command table
9b09013  feat(common): add Command/CommandLog/CommandResult message types
```

Plus uncommitted prior-session fixes still in working tree (orchestrator SSH user/key,
existing_vm_ip persistence, version fallback, Azure Windows password+computer-name,
agent linger/auto-start, etc.). Those are transitional — Phase 3 deletes the
SSH path entirely, so they can be either committed as-is or left out depending
on whether Phase 2 is finished before next release cut.

## What's missing from Phase 1

- E2E smoke test for `health` round-trip via a live agent. Skipped intentionally —
  the plan's Task 7 will be folded into the final Win11 Desktop verification at
  the end of Phase 2 (the user's requested test). Phase 1 plumbing is unit-tested
  and known-correct in isolation.

## Next session — start here

### What to do first

1. Read `docs/superpowers/specs/2026-04-14-command-based-orchestration-design.md`.
2. Read `docs/superpowers/plans/2026-04-14-command-based-orchestration-plan.md`,
   skip ahead to **Task 9**.
3. Verify branch + HEAD: `git log --oneline -8` should match the list above.
4. Confirm dashboard still runs locally (port 3000) against prod DB via SSH tunnel.
   Restart with: `set -a; . /tmp/dashboard_env.txt; set +a; export ORCH_SSH_KEY="$HOME/.ssh/id_rsa"; export ORCH_SSH_USER=ubuntu; nohup ./target/debug/networker-dashboard > /tmp/dashboard.log 2>&1 &`

### Tasks ahead, in order

| # | Task | Notes |
|---|---|---|
| 9 | AWS `--user-data` cloud-init wiring | Add `bootstrap_script` field to `VmConfig`, pass through in `AwsProvider::create_vm` via `--user-data file://`. |
| 10 | GCP `--metadata-from-file startup-script=...` | Same pattern, GCP-specific. |
| 11 | Azure `--custom-data @...` | Same pattern. Windows needs base64. |
| 12 | Wire cloud-init into tester create | Reorder `provision_agent_for_tester` to mint api_key BEFORE VM create; pass bootstrap script in. Replace SSH install path with poll for agent online. Keep SSH behind `?ssh_bootstrap=1`. |
| 13 | Core verbs (install/start/stop_server, collect_logs) | Per-verb tests. |
| 14 | Orchestrator (alethabench) refactor to dispatch | Big one. ~14 SSH call sites in `executor.rs`. Feature flag `ORCH_USE_AGENT_COMMANDS=1`. |
| 15 | Windows agent build + Windows cloud-init | Verify CI publishes `networker-agent-x86_64-pc-windows-msvc.tar.gz`. |
| 16 | Wizard "Client Tester" selector + V034 `benchmark_testbed.tester_id` | UI + API + orchestrator. |
| 17 | Probe verbs `run_probe`, `run_browser_probe`, `upload_artifact` | |
| 13c | `run_probe` with capture_mode + dashboard-exclusion BPF filter | Folded into Task 17. |
| 17b | Pcap upload via presigned URL | Needs object store decision (S3? GCS? Azure Blob? per-project?) |
| 22 | UI: capture toggle + download .pcap | |
| 23 | **Verification gate** — pcap data-plane parity ±1% | Mandatory before Phase 2.5 ships. |
| 18-20 | Gate + delete SSH; doc updates | |

### Final acceptance test (user's explicit ask)

After all Phase 2 + Phase 2.5 lands:

1. Delete all existing testers (clean slate).
2. Create one `bm-azure-win11` (Windows 11 Desktop on Azure eastus).
3. Run all test types (network probes + benchmarks + browser probes) against it.
4. Run with `Packet Capture: Tester-side` enabled.
5. Verify pcap downloads from UI, parity test passes.

### Lib/bin split — important constraint

Per `feedback_dashboard_lib_bin_split.md`:
- `crates/networker-dashboard/src/services/` is part of the lib crate.
- `db/`, `api/`, `auth/`, `AppState` are bin-only.
- New code in `services/` must NOT depend on bin-only modules.
- Workaround Task 4 used: place dashboard-specific orchestration helpers at
  `crate::agent_dispatch` (top-level bin module), not under `services::`.

### Token budget reality

Each subagent dispatch consumes ~10-30K tokens of conversation context.
A fresh session can do ~6-8 sequential tasks. Plan to need 2-3 fresh
sessions to finish Phase 2 + Phase 2.5.

### Quick verification commands

```bash
# All Phase 1 commits build + test cleanly
cargo build -p networker-common -p networker-agent -p networker-dashboard
cargo test  -p networker-common -p networker-agent -p networker-dashboard

# Verify the new tables exist in the prod DB (via tunnel)
# Migration log lines to look for in /tmp/dashboard.log:
grep -E "V03[23] migration complete" /tmp/dashboard.log

# Working dashboard endpoint added by Task 6 (no agent attached yet):
curl -X POST http://127.0.0.1:3000/api/projects/us057ygm4a200q/agents/<agent-uuid>/commands \
  -H "authorization: Bearer $(curl -sS -X POST http://127.0.0.1:3000/api/auth/login \
       -H 'content-type: application/json' \
       -d '{"email":"admin@alethedash.com","password":"admin123"}' \
       | python3 -c 'import sys,json;print(json.load(sys.stdin)[\"token\"])')" \
  -H 'content-type: application/json' \
  -d '{"verb":"health","args":{}}'
```
