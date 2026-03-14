---
applyTo: "**/tests/**,**/*test*,**/*_test.rs"
---

# Test Rules

## Unit Tests (Rust)
- Place unit tests in `#[cfg(test)] mod tests { ... }` at the bottom of each source file.
- Use `#[test]` for sync tests, `#[tokio::test]` for async tests.
- Name tests descriptively: `fn protocol_fromstr_rejects_invalid()`, not `fn test1()`.
- Assert with `assert!`, `assert_eq!`, `assert_ne!` — avoid `unwrap()` in test assertions when `assert!(result.is_ok())` is clearer.

## Integration Tests (Rust)
- Located at `crates/networker-tester/tests/integration.rs`.
- Tests spawn an in-process `networker-endpoint` via the `Endpoint` fixture struct.
- The fixture allocates random ports (HTTP, HTTPS, UDP echo, UDP throughput) to avoid conflicts.
- MUST run with `--test-threads=1` — multiple endpoints on overlapping ports will cause flaky failures.
- Readiness: TCP connect polling + UDP echo probe, with timeout. Do not assume the endpoint is ready instantly.
- New protocol probes need an integration test that exercises the full client→endpoint path.

## Database Tests
- SQL tests are `#[ignore]` by default — they require Docker services from `docker-compose.db.yml`.
- CI enables them with `--include-ignored` when `NETWORKER_SQL_TESTS=true`.
- Filter: `--test-threads=1 -- db_mssql` or `-- db_postgres`.
- Use `test_fixtures::InMemoryBackend` for unit tests that don't need a real database connection.
- Never hardcode connection strings in test code — read from `NETWORKER_SQL_CONN` / `NETWORKER_DB_URL` env vars.

## Installer Tests (bats)
- Located at `tests/installer.bats` (84 tests currently).
- Stubs in `tests/stubs/` mock external tools (gh, cargo, ssh, cloud CLIs).
- Tests source install.sh functions and validate output/behavior.
- Deploy-config validation tests: 36 tests covering JSON parsing, field validation, provider checks.
- HTTP stack tests: 7 tests covering nginx/IIS setup logic.
- Add bats tests for: new deploy-config fields, new validation rules, new installer prompts.

## Test Patterns to Follow
- Probe tests: create a `ThroughputConfig` / `RunConfig` / `PageLoadConfig` with the target URL pointing at the local endpoint fixture.
- Timing assertions: check that durations are > 0 and < timeout, not exact values (network timing is non-deterministic).
- Error path tests: verify that `ErrorCategory` is set correctly and `error.message` is non-empty.
- Feature-gated tests: use `#[cfg(feature = "http3")]` to skip H3 tests when the feature is disabled.

## What NOT to Do
- Do not add `#[ignore]` to non-database tests without justification.
- Do not use `thread::sleep` for synchronization — use the endpoint readiness polling pattern.
- Do not test against external hosts — all network tests must use the local endpoint fixture.
- Do not assert on exact throughput values — they depend on hardware and system load.
