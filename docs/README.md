# Documentation Index

Use this folder for the full documentation set. The root
[`README.md`](../README.md) is intentionally brief and
points here for the detailed material.

## Current Docs

- [`architecture.md`](architecture.md): current hybrid architecture — Rust probe core + C# control plane/agent + React SPA
- [`branding.md`](branding.md): the product brand is Networker; deployment/legacy name policy
- [`installation.md`](installation.md): install from scripts, build from source (cargo + dotnet), and start each component
- [`release-flow.md`](release-flow.md): version bump → auto-tag → deploy-first release graph → rollback
- [`probes.md`](probes.md): probe modes, metrics, and CLI examples
- [`testing.md`](testing.md): reproducible test plans and report interpretation
- [`config-examples.md`](config-examples.md): which JSON sample to copy for tester, endpoint, and deploy workflows
- [`deploy-config.md`](deploy-config.md): full `--deploy` schema and execution flow (the `dashboard` object is legacy)
- [`setup-guide.md`](setup-guide.md): production deployment guide (infrastructure, SSO, cloud federation, email) — some manual-setup sections still show the legacy Rust service and are marked as such
- [`cloud-auth.md`](cloud-auth.md): cloud authentication for the control plane — ambient CLI auth + AES-256-GCM stored credentials (materialised to 0600 temp files at provision time)
- [`alerting.md`](alerting.md): threshold alert rules + notification channels (webhook/email) — concepts, API, webhook payload + signature contract
- [`schema-ownership.md`](schema-ownership.md): the control-plane PostgreSQL schema is owned by `src/Networker.Data` (migrations, migrator, compatibility guarantees)
- [`ops-nginx-ws-redaction.md`](ops-nginx-ws-redaction.md): nginx access-log redaction for the `/ws/*` credential query strings — re-apply procedure for a VM rebuild
- [`reports-app-network.md`](reports-app-network.md): the Application Network Performance report — splits SDK-probe latency into application (server) vs network time
- [`reports-perf-per-cost.md`](reports-perf-per-cost.md): the provider performance-per-cost report — probe results + tester metadata + curated price table
- [`sdk/`](sdk/README.md): the LagHound SDK — embeddable diagnostic endpoint spec ([`sdk/contract-v1.md`](sdk/contract-v1.md) is the v1 wire contract)
- [`dead-code-removal-strategy.md`](dead-code-removal-strategy.md): the post-decommission dead-code sweep plan (what dies with the retired crates, what is a deliberate keep)
- [`dotnet-migration.md`](dotnet-migration.md): the Rust↔C# seam — versioned JSON contract, differential-testing architecture
- [`phase2-cutover-runbook.md`](phase2-cutover-runbook.md): production ops runbook — leader election, health endpoints, soak checklist (§4), rollback (§5), decommission criteria (§7). Cutover is complete; §4/§5/§7 remain operative until the Rust crates are decommissioned
- [`tls-endpoint-profile-design.md`](tls-endpoint-profile-design.md): TLS endpoint profiling feature design
- [`tls-endpoint-profile-phase1-checklist.md`](tls-endpoint-profile-phase1-checklist.md): implementation checklist companion to the TLS profile design

## Operational Runbooks

- [`runbooks/observability.md`](runbooks/observability.md): every control-plane signal — health endpoints, TickMonitor, perf_log, journald, and the watchdog/reaper WARN messages
- [`runbooks/admin-password-reset.md`](runbooks/admin-password-reset.md): reset the production admin login (base64-ship the bcrypt SQL; update the `DASHBOARD_ADMIN_PASSWORD` soak secret)
- [`runbooks/credential-rotation.md`](runbooks/credential-rotation.md): rotate `DASHBOARD_CREDENTIAL_KEY` (dual-key window) and `DASHBOARD_JWT_SECRET`
- [`runbooks/perf-log-diagnosis.md`](runbooks/perf-log-diagnosis.md): read `perf_log` for a slow-page investigation (server_ms vs network_ms; aborted-poll artifact)

## Historical / Archive

- [`archive/hybrid-migration-plan.md`](archive/hybrid-migration-plan.md): the Rust→C# migration decision + phased plan (completed 2026-07)
- [`archive/phase2-scope.md`](archive/phase2-scope.md): the M0–M6 build-out scope for the C# control plane (completed 2026-07)

## Other Material In This Tree

- [`analysis/`](analysis/): point-in-time audit reports and scorecards (code quality, security/tests, docs/ops, tester trust, language benchmarks) — snapshots, not maintained docs
- [`examples/`](examples/): sample YAML test plans
- `prs/`, `superpowers/`: working notes for specific PRs and plans/specs — not user documentation
- [`../benchmarks/shared/API-SPEC.md`](../benchmarks/shared/API-SPEC.md): the frozen (v1, 2026-07-16) benchmark API contract that `deploy-config.md`'s `apibench` mode references

## By Task

### Install or build the tools

Read [`installation.md`](installation.md).

### Run probes against a local or remote endpoint

Read [`installation.md`](installation.md) for
startup, then [`probes.md`](probes.md) for mode
selection.

### Use config files instead of long CLI commands

Read [`config-examples.md`](config-examples.md).

### Run reproducible protocol comparisons

Read [`testing.md`](testing.md).

### Deploy testers and endpoints from JSON

Read [`deploy-config.md`](deploy-config.md).

### Ship a release (or roll one back)

Read [`release-flow.md`](release-flow.md).

### Operate production / the decommission soak

Read [`phase2-cutover-runbook.md`](phase2-cutover-runbook.md).

### Understand control-plane cloud authentication

Read [`cloud-auth.md`](cloud-auth.md).

### Operate the control plane day-to-day

Read [`runbooks/observability.md`](runbooks/observability.md), and the task-specific
runbooks for [password reset](runbooks/admin-password-reset.md),
[credential rotation](runbooks/credential-rotation.md), and
[perf-log diagnosis](runbooks/perf-log-diagnosis.md).

### Deploy the control plane to production

Read [`setup-guide.md`](setup-guide.md) and [`release-flow.md`](release-flow.md).

### Change the database schema

Read [`schema-ownership.md`](schema-ownership.md).
