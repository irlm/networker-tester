## Summary
<!-- 1-3 bullet points describing what changed and why -->

## Test plan
<!-- How was this tested? Local smoke test, integration test, cloud deploy? -->

## Checklist

### Required
- [ ] CHANGELOG.md updated with new entry
- [ ] Version bumped in all five files (Cargo.toml, CHANGELOG.md, install.sh, install.ps1, Directory.Build.props)
- [ ] `scripts/lint-all.sh --no-build` passes (covers `cargo fmt`, `cargo clippy -D warnings`, ESLint, shellcheck, action pins, version sync, …)
- [ ] `cargo test --workspace --lib` passes locally

### If adding a new Protocol variant
- [ ] Added to `Display` impl in metrics.rs
- [ ] Added to `FromStr` impl in metrics.rs
- [ ] Added to `primary_metric_label()` in metrics.rs
- [ ] Added to `primary_metric_value()` in metrics.rs
- [ ] Added to `print_summary()` ordering in main.rs
- [ ] Added to `dispatch_once()` match in main.rs
- [ ] Added to `log_attempt()` match in main.rs
- [ ] Added to deploy-config valid modes (docs/deploy-config.md)
- [ ] HTTP/3 stub module updated if touching H3 paths

### If changing SQL schema
- [ ] New numbered migration file in `sql/` (SQL Server)
- [ ] Matching migration in `sql/postgres/` (PostgreSQL)
- [ ] `DatabaseBackend` trait updated (mod.rs, mssql.rs, postgres.rs, test_fixtures.rs)
- [ ] Tested against both MSSQL and PostgreSQL via docker-compose.db.yml

### If changing installers
- [ ] Bash 3.2 compatible (no associative arrays, no `[[ -v ]]`, no nameref)
- [ ] stdin-safe (`< /dev/null` for non-interactive commands)
- [ ] `INSTALLER_VERSION` bumped in BOTH install.sh and install.ps1
- [ ] `bats tests/installer.bats` passes
- [ ] shellcheck passes (`scripts/lint-all.sh --only shellcheck-installer` — CI flags/exclusions built in)
- [ ] PSScriptAnalyzer passes for install.ps1 changes
- [ ] After merge: confirm the `Sync install scripts to Gist` workflow ran (`gh run list --branch main`); update the Gist manually only if it failed (see CLAUDE.md)

### If changing endpoint routes
- [ ] Integration test added/updated in tests/integration.rs
- [ ] Endpoint bats tests pass (`tests/endpoint.bats`)
- [ ] Both HTTP and HTTPS paths tested
- [ ] HTTP/3 path tested if applicable
