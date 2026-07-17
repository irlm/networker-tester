# CLAUDE.md

Project-specific instructions for Claude Code.

## Project Overview

Hybrid Rust + C# repo: Rust owns measurement, C# owns the application layer.
See `docs/architecture.md` for the full picture.

| Rust crate (`crates/`) | Status | Role |
|-------|--------|------|
| `networker-tester` | **current — permanent core** | CLI probe engine: per-phase timing across HTTP/1.1-2-3, UDP, DNS, TLS; DB persistence; HTML/Excel reports; versioned JSON contract |
| `networker-endpoint` | current | Diagnostic server — health, download, upload, page-load endpoints |
| `networker-log` | current | Shared tracing subscriber (console + Postgres log sinks) |
| `networker-dashboard` | **retired** | Legacy axum control plane — replaced by `Networker.ControlPlane` |
| `networker-agent` | **retired** | Legacy worker — replaced by `Networker.Agent` |
| `networker-common` | **retired** | Legacy dashboard↔agent message types |

C# solution (`Networker.sln`, .NET 10): `src/Networker.ControlPlane` (ASP.NET
Minimal APIs + raw-WS hubs — **serves prod alethedash.com**, port 5030 behind
nginx), `src/Networker.Agent` (what tester VMs bootstrap; release assets
`networker-agent-cs-*`), plus `Contracts` (the versioned JSON seam), `Data`
(EF Core — **owns the schema migrations**, see `docs/schema-ownership.md`),
`Security`, `Endpoint`. C# tests live in `tests/Networker.*Tests/`.

`dashboard/` is the React + TypeScript + Vite frontend (Tailwind dark theme) —
served static by nginx in prod. `benchmarks/orchestrator` (excluded from the
workspace) ships as `alethabench`; benchmark reference APIs have their own
solution at `benchmarks/reference-apis/benchmarks.sln`.

**Migration status: complete.** The C# control plane serves production; the
retired Rust crates are off the release train and stay in-tree only for the
decommission soak/rollback window (`docs/phase2-cutover-runbook.md` §7; nightly
`Prod soak check` workflow). Do NOT add features to the retired crates. The
full-Rust snapshot is on the `legacy/rust` branch and the `rust-legacy-*` tag;
the migration rationale is archived at `docs/archive/hybrid-migration-plan.md`.

## Build Commands

```bash
# Format + lint (CI runs these — fix before committing)
cargo fmt --all
cargo clippy --all-targets -- -D warnings

# Build workspace
cargo build --workspace
cargo build --release --workspace

# Verify no-default-features compiles (http3/pageload3 stubs)
cargo build -p networker-tester --no-default-features

# Build with all features (browser + both DB backends)
cargo build -p networker-tester --all-features

# C# solution (control plane, agent, contracts, data, security, endpoint)
dotnet build Networker.sln -c Release
```

## Test Commands

```bash
# Unit tests (fast, no network required)
cargo test --workspace --lib

# Integration tests (spawns in-process endpoint, must serialize)
cargo test --test integration -p networker-tester -- --test-threads=1

# SQL integration tests (requires docker-compose.db.yml running)
NETWORKER_SQL_CONN="Server=tcp:127.0.0.1,1433;..." cargo test -p networker-tester --all-features --include-ignored -- db_mssql --test-threads=1
NETWORKER_DB_URL="postgres://networker:test@127.0.0.1:5432/networker" cargo test -p networker-tester --all-features --include-ignored -- db_postgres --test-threads=1

# Coverage (same as CI)
cargo llvm-cov --all-features --workspace --html

# Installer tests
shellcheck install.sh
bats tests/installer.bats

# Endpoint tests
cargo test -p networker-endpoint --lib

# C# tests (xUnit; some use Testcontainers → need Docker)
dotnet test Networker.sln

# Dashboard frontend
cd dashboard && npm install && npm run build && npm run lint
```

## Control Plane Local Dev (C#)

```bash
# 1. PostgreSQL
docker compose -f docker-compose.dashboard.yml up -d postgres

# 2. Endpoint (test target)
cargo run -p networker-endpoint

# 3. Control plane (port 5030; runs DB migrations on startup)
DASHBOARD_JWT_SECRET=$(openssl rand -base64 32) \
DASHBOARD_CREDENTIAL_KEY=$(openssl rand -hex 32) \
ASPNETCORE_URLS=http://0.0.0.0:5030 \
  dotnet run --project src/Networker.ControlPlane

# 4. Agent (C#)
AGENT_API_KEY=dev-key AGENT_DASHBOARD_URL=ws://localhost:5030/ws/agent \
  dotnet run --project src/Networker.Agent

# 5. Frontend (port 5173, proxies /api and /ws to the control plane)
cd dashboard && npm install && npm run dev
```

Key env vars: `DASHBOARD_DB_URL_NPGSQL`, `DASHBOARD_JWT_SECRET`,
`DASHBOARD_CREDENTIAL_KEY` (both fail-closed outside Development),
`ASPNETCORE_URLS`, `DASHBOARD_PUBLIC_URL`, `DASHBOARD_BACKGROUND_SERVICES`,
`AGENT_DASHBOARD_URL`, `AGENT_API_KEY`. See README + `docs/phase2-cutover-runbook.md` §1.1.
Do NOT run the retired Rust dashboard/agent for dev.

## Quality Checks

After implementing any fix, test the exact end-to-end workflow (e.g., curl|bash install, remote SSH deploy, CI pipeline) before marking complete. Do not assume partial unit tests cover integration paths.

## Rust / Cargo

- Always commit Cargo.lock when making release tags or merging deployment PRs. Run `cargo generate-lockfile` if needed.
- Use `anyhow::Result` with `.context()` for error propagation. Classify probe errors into `ErrorCategory`.
- rustls with ring provider only — call `ring::default_provider().install_default()` before TLS.
- HTTP/3 stub module must mirror the real module's public API. CI verifies `--no-default-features` builds.

## Version Sync (5 locations)

1. `Cargo.toml` workspace `version` field
2. `CHANGELOG.md` — new `## [X.Y.Z]` section
3. `install.sh` — `INSTALLER_VERSION`
4. `install.ps1` — `InstallerVersion`
5. `Directory.Build.props` `<Version>` (repo root — stamps every C# assembly)

Every PR must bump all five files. CI (`version-check`) enforces that
Directory.Build.props == Cargo.toml, CHANGELOG has the section, and the
installers match.

Everything else on the C# side is DERIVED from the assembly version at build
time and must never be hand-bumped: the agent's heartbeat/self-reported
version (`AgentVersion.Current`), the endpoint's `ServerInfo.Version` +
`/health`, the control plane's `/api/health` + `/api/version`
`dashboard_version` (`VersionEndpoints.DashboardVersion`), and the
version-refresh floor. Do NOT add `<Version>` to individual .csproj files.

## Adding a New Protocol Variant

Update all of these in a single PR:
- `Protocol` enum in `metrics.rs` (variant + Display + FromStr)
- `primary_metric_label()` and `primary_metric_value()` in `metrics.rs`
- `dispatch_once()` + `log_attempt()` in `dispatch.rs`, `print_summary()` in `summary.rs`
- Throughput/payload size mapping in `main.rs` (if applicable)
- `docs/deploy-config.md` valid modes table
- Integration test in `tests/integration.rs`

(All paths under `crates/networker-tester/src/`. Note: `apibench` is a
runner-level mode, not a tester protocol — the agent expands it per
`benchmarks/configs/apibench.json` + `benchmarks/shared/API-SPEC.md` §4.)

## Installer Constraints

- **Bash 3.2** — no `declare -A`, no `[[ -v ]]`, no nameref, no `readarray`
- **stdin protection** — `< /dev/null` for non-interactive commands in curl|bash pipe
- **Windows** — `Start-Process -WindowStyle Hidden` (never `-NoNewWindow`); VM names ≤15 chars
- **After merge** — the `Sync install scripts to Gist` workflow updates the Gist
  automatically on every main push (verified working 2026-07-13). Confirm it
  succeeded with `gh run list --branch main`; only if it failed, update manually:
  ```bash
  jq -n --rawfile sh install.sh --rawfile ps install.ps1 \
    '{"files":{"install.sh":{"content":$sh},"install.ps1":{"content":$ps}}}' \
    > /tmp/gist_payload.json && \
  gh api --method PATCH /gists/37a1af64b70ef6e58ea117839407f4f9 \
    --input /tmp/gist_payload.json --jq '.updated_at'
  ```

## Documentation

When writing documentation for CLI flags or environment variables (e.g., RUST_LOG), verify the documented values by actually running the binary with those settings before committing.

## Git Workflow & Release

- Never commit directly to main — all changes go through a PR.
- Branch → commit → push → `gh pr create` → merge. Auto-tag (from Cargo.toml)
  and the Gist sync run automatically on the main push — verify both landed.
- Required CI checks (branch protection): `Test (ubuntu-latest)`,
  `Test (windows-latest)`, `Detect changed areas`, `Build & audit (C#)`,
  `bats (installer unit tests)`, `shellcheck`.
- Release = deploy-first graph: the tag triggers release.yml; build-linux +
  build-csharp gate the GitHub release and the prod deploy to alethedash.com
  (~8-9 min, auto-rollback on failed readiness); mac/windows binaries attach
  asynchronously. Full flow + rollback: `docs/release-flow.md`.

## Design Context

See `.impeccable.md` for full design context. Key principles:

- **Users**: Network/IT engineers, DevOps/SRE teams — high technical level
- **Personality**: Technical, precise, reliable
- **Aesthetic**: Terminal/hacker — monospace-first, dark theme, data-dense. References: Grafana, Datadog, Warp
- **Brand colors**: Purple `#863bff` (logo), Cyan (primary accent), deep navy backgrounds
- **Principles**: Data density over decoration | Terminal confidence | Trust through consistency | Progressive disclosure | Zero chrome (no gradients, no shadows, flat surfaces, thin borders)
