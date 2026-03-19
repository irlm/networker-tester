# CLAUDE.md

Project-specific instructions for Claude Code.

## Project Overview

Rust workspace: five crates plus Bash/PowerShell installers and a React frontend.

| Crate | Role |
|-------|------|
| `networker-tester` | CLI client -- per-phase network timing across HTTP/1.1, HTTP/2, HTTP/3, UDP, DNS, TLS with database persistence and HTML/Excel reporting |
| `networker-endpoint` | Diagnostic server -- serves health, download, upload, page-load endpoints |
| `networker-common` | Shared WebSocket message types for dashboard-agent protocol |
| `networker-dashboard` | axum control plane -- REST API, WebSocket hubs, JWT auth, PostgreSQL |
| `networker-agent` | Daemon -- connects to dashboard, executes probe jobs via networker-tester lib, streams results live |

The `dashboard/` directory contains a React + TypeScript + Vite frontend (Tailwind dark theme).

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

# Dashboard frontend
cd dashboard && npm install && npm run build && npm run lint
```

## Dashboard Local Dev

```bash
# 1. PostgreSQL
docker compose -f docker-compose.dashboard.yml up postgres

# 2. Endpoint
cargo run -p networker-endpoint

# 3. Dashboard (port 3000)
DASHBOARD_ADMIN_PASSWORD=admin cargo run -p networker-dashboard

# 4. Agent
AGENT_API_KEY=dev-key cargo run -p networker-agent

# 5. Frontend (port 5173, proxies /api and /ws to dashboard)
cd dashboard && npm install && npm run dev
```

Key env vars: `DASHBOARD_DB_URL`, `DASHBOARD_ADMIN_PASSWORD`, `DASHBOARD_JWT_SECRET`, `DASHBOARD_PORT`, `AGENT_DASHBOARD_URL`, `AGENT_API_KEY`. See README for defaults.

## Quality Checks

After implementing any fix, test the exact end-to-end workflow (e.g., curl|bash install, remote SSH deploy, CI pipeline) before marking complete. Do not assume partial unit tests cover integration paths.

## Rust / Cargo

- Always commit Cargo.lock when making release tags or merging deployment PRs. Run `cargo generate-lockfile` if needed.
- Use `anyhow::Result` with `.context()` for error propagation. Classify probe errors into `ErrorCategory`.
- rustls with ring provider only — call `ring::default_provider().install_default()` before TLS.
- HTTP/3 stub module must mirror the real module's public API. CI verifies `--no-default-features` builds.

## Version Sync (3 locations)

1. `Cargo.toml` workspace `version` field
2. `CHANGELOG.md` — new `## [X.Y.Z]` section
3. `INSTALLER_VERSION` in both `install.sh` AND `install.ps1`

Every PR must bump all three.

## Adding a New Protocol Variant

Update all of these in a single PR:
- `Protocol` enum in `metrics.rs` (variant + Display + FromStr)
- `primary_metric_label()` and `primary_metric_value()` in `metrics.rs`
- `dispatch_once()`, `log_attempt()`, `print_summary()` in `main.rs`
- Throughput/payload size mapping in `main.rs` (if applicable)
- `docs/deploy-config.md` valid modes table
- Integration test in `tests/integration.rs`

## Installer Constraints

- **Bash 3.2** — no `declare -A`, no `[[ -v ]]`, no nameref, no `readarray`
- **stdin protection** — `< /dev/null` for non-interactive commands in curl|bash pipe
- **Windows** — `Start-Process -WindowStyle Hidden` (never `-NoNewWindow`); VM names ≤15 chars
- **After merge** — update Gist manually (sync-gist.yml is broken):
  ```bash
  jq -n --rawfile sh install.sh --rawfile ps install.ps1 \
    '{"files":{"install.sh":{"content":$sh},"install.ps1":{"content":$ps}}}' \
    > /tmp/gist_payload.json && \
  gh api --method PATCH /gists/37a1af64b70ef6e58ea117839407f4f9 \
    --input /tmp/gist_payload.json --jq '.updated_at'
  ```

## Documentation

When writing documentation for CLI flags or environment variables (e.g., RUST_LOG), verify the documented values by actually running the binary with those settings before committing.

## Git Workflow

- Never commit directly to main — all changes go through a PR.
- Branch → commit → push → `gh pr create` → merge → tag → push tag → update Gist.
- Required CI checks: `Test (ubuntu-latest)`, `Test (windows-latest)`, `bats (installer unit tests)`, `shellcheck`.

## Design Context

See `.impeccable.md` for full design context. Key principles:

- **Users**: Network/IT engineers, DevOps/SRE teams — high technical level
- **Personality**: Technical, precise, reliable
- **Aesthetic**: Terminal/hacker — monospace-first, dark theme, data-dense. References: Grafana, Datadog, Warp
- **Brand colors**: Purple `#863bff` (logo), Cyan (primary accent), deep navy backgrounds
- **Principles**: Data density over decoration | Terminal confidence | Trust through consistency | Progressive disclosure | Zero chrome (no gradients, no shadows, flat surfaces, thin borders)
