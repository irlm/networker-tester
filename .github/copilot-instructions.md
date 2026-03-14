# Copilot Code Review Instructions — networker-tester

## Project Context
Rust workspace: `networker-tester` (CLI diagnostic client) + `networker-endpoint` (axum HTTP/UDP server).
Measures per-phase network timing (DNS→TCP→TLS→TTFB→Total) across HTTP/1.1, HTTP/2, HTTP/3, UDP, DNS, TLS.
Optional features: `http3` (default), `browser`, `db-mssql`, `db-postgres`, `native`.

## Rust Rules
- Use `anyhow::Result` for fallible functions; add `.context("description")` to every `?` in new code.
- Classify probe errors into `ErrorCategory` (Dns, Tcp, Tls, Http, Udp, Timeout, Config, Other) — never use `Other` when a specific category fits.
- Never add connection pooling to probe runners — each probe intentionally creates a fresh TCP+TLS connection for accurate per-phase timing.
- `rustls` with `ring` provider only — call `ring::default_provider().install_default()` before any TLS operation.
- All new `Protocol` enum variants must be added to: `Display`, `FromStr`, `primary_metric_label`, `primary_metric_value`, `print_summary` order, and the deploy-config valid-modes list.
- HTTP/3 stub module (`#[cfg(not(feature = "http3"))]`) must expose the same public API as the real module. If you add a function to `real`, add it to `stub`.
- Feature-gated code must compile with `--no-default-features` (CI verifies this).
- Run `cargo fmt` and `cargo clippy -- -D warnings` before every commit.

## SQL Rules
- All SQL inserts must use parameterized queries (`@P1, @P2` for tiberius; `$1, $2` for tokio-postgres). Never interpolate user values into SQL strings.
- Schema changes require a new numbered migration file in `sql/` (SQL Server) and `sql/postgres/` (PostgreSQL). Keep both backends in sync.
- The `DatabaseBackend` async trait (connect, migrate, save, ping) must be implemented for both backends when adding new tables.
- Test SQL changes against both MSSQL and PostgreSQL in CI (docker-compose.db.yml).

## Installer Rules (install.sh / install.ps1)
- install.sh must work on Bash 3.2 (macOS default). No associative arrays (`declare -A`), no `[[ -v VAR ]]`, no `${!ref}`.
- Any command that reads stdin in install.sh must use `< /dev/null` (curl|bash pipe consumes stdin). Interactive prompts use `< /dev/tty`.
- Version changes require bumping `INSTALLER_VERSION` in both install.sh AND install.ps1.
- Windows VM names must be ≤15 characters (NetBIOS limit) — validate in deploy-config.
- Never use `-NoNewWindow` for endpoint process start on Windows — use `Start-Process -WindowStyle Hidden`.
- Cloud CLI commands (`az`, `aws`, `gcloud`) must be checked with `command -v` before execution; never execute gcloud in `discover_system` (Python startup is slow).

## Version & Release
- Workspace version in root `Cargo.toml`, CHANGELOG.md, and `INSTALLER_VERSION` in both installers must stay in sync.
- Every PR must include a CHANGELOG entry and version bump.
- Commit `Cargo.lock` on release tags and deployment PRs.
- Never commit directly to main — all changes go through PRs.

## Testing
- Integration tests (`tests/integration.rs`) run with `--test-threads=1` to avoid port conflicts.
- Installer tests use bats (`tests/installer.bats`) with stubs in `tests/stubs/`.
- New probe modes need: unit test, integration test with in-process endpoint, bats test for deploy-config validation.

## Security
- No hardcoded credentials. SQL connection strings come from env vars only (`NETWORKER_SQL_CONN`, `NETWORKER_DB_URL`).
- `--insecure` flag must only skip TLS verification, never disable other security checks.
- Never log or serialize credential values; redact connection strings in error messages.
