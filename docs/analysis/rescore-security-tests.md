# Re-score — Security Posture + Test Quality

Read-only re-review against `docs/analysis/scorecard-security-tests.md` (baseline: 2026-07-17 morning).
Re-scored: 2026-07-17, after #442 (secret scrub + rotation), #443 (cargo-audit gate, SSH hardening, EF log level),
#444–#446 (schema ownership, version single-sourcing, vulnerable-crate upgrades), #449–#451 (move-only refactors).
Numeric mapping: A=95, A-=90, B+=87, B=83, B-=80, C+=77, C=73, D=60.

## Score Table

| # | Dimension | Baseline | Now | Delta | One-line justification |
|---|-----------|---------:|----:|------:|------------------------|
| 1 | Secrets handling | D / 60 | **A- / 90** | **+30** | P0 closed: prod JWT secret rotated (old tokens verified 401) and test scrubbed; committed token cryptographically verified to be signed by the synthetic key; repo-wide re-grep clean. No secret-scanning CI yet. |
| 2 | Injection surfaces | B / 83 | **A- / 90** | **+7** | All 4 risky orchestrator sites now `shell_quote`'d (correct POSIX `'\''` escaping), with a real-shell inert-transport test proving `$()`/backtick payloads don't execute; `validate_shell_safe` kept as defense-in-depth. |
| 3 | AuthZ coverage | A- / 90 | **A- / 90** | **0** | Gating unchanged through the #449 endpoint split (all 9 `RequireAuthorization` stayed in the route registrar); the baseline's agent api_key finding (non-constant-time `==`, plaintext at rest) is **still open**. |
| 4 | Test quality | B / 83 | **B / 85** | **+2** | Counts grew everywhere (tester 878→880, orchestrator 100→104 incl. new shell-quote security tests, C# 434→450 attrs + 301 InlineData); zero regressions from the move-only refactors — but all three baseline thin areas (cloud provisioning, WS resilience, frontend RBAC) are unchanged. |
| 5 | Dependency hygiene | B+ / 87 | **A- / 90** | **+3** | cargo-audit gate live (PR + weekly + dual lockfile), ignore list 11→5 with per-entry justifications that all check out against `Cargo.lock`; Dependabot still absent, two `setup-dotnet@v4` still tag-pinned, and the audit job is not a required branch-protection check. |

**Weighted average (equal weights): 89.0** (baseline: 80.6 → **+8.4**). P0 count: 1 → **0**.

---

## 1. Secrets Handling — A- / 90 (+30)

The baseline P0 (live prod JWT signing secret + platform-admin token committed to a public repo) is closed:

- **Rotation confirmed before scrub.** Commit `a46c1324` (#442): "The secret was ROTATED on prod immediately (old tokens verified 401) before this scrub, so history exposure is defanged."
- **Synthetic values verified, not just claimed.** `tests/Networker.ControlPlane.Tests/JwtShortKeyInteropTests.cs:15-16` now uses `"synthetic29bytetestsecretxyz1"`. Independently recomputed HMAC-SHA256 over the committed token's `header.payload` with that synthetic key → signature `I5b4NbM0SDHp8APB1L-s7Eg8pISKjA7AX7R_AVbyOM4` — **exact match**. The committed token is minted from the synthetic secret; it is not a prod artifact.
- **Re-grep clean.** Repo-wide sweep for `(secret|password|api_key|token) = "<16+ chars>"` across .rs/.cs/.ts/.sh/.ps1/.yml/.json/.sql/.toml (excluding known dev defaults) returns only the synthetic interop token above. No credential-looking strings anywhere.
- The test file now carries an explicit never-again comment documenting the incident.

**Why not A:** (a) the old secret remains in public git history — inert only because rotation holds; (b) the synthetic token still embeds real prod identifiers (`admin@alethedash.com`, a real user sub UUID) — identifiers, not credentials, but unnecessary; (c) no automated secret-scanning gate (gitleaks/trufflehog) to prevent recurrence; (d) the interop test's first assertion is expiry-guarded and the embedded token expired 2026-07-16 — that branch is now permanently skipped (dead assertion; mint the token in-test instead).

## 2. Injection Surfaces — A- / 90 (+7)

The baseline's exact P1 recommendation was implemented (#443, then move-only reorganized by #450 into `benchmarks/orchestrator/src/executor/`):

- **All 4 baseline sites now escape:**
  - `deploy_proxy` → `executor/ssh_exec.rs:124` (`shell_quote(proxy)`)
  - `deploy_app_language` → `executor/ssh_exec.rs:159-161` (`shell_quote(language)` ×2 + `shell_quote(proxy)`)
  - `start_existing_server` → `executor/ssh_exec.rs:45` (`shell_quote(language)`)
  - `run_chrome_benchmark` token + args → `executor/benchmark.rs:36,53-54` (`shell_quote(bench_token)`, `shell_quote(http_version)`, `shell_quote(connection_mode)`)
- **`shell_quote` implementation** (`executor/ssh_exec.rs:95`): POSIX single-quote wrapping with correct `'\''` interior-quote escaping — the standard safe transform.
- **Inert-transport test exists and is real:** `test_shell_quote_hostile_value_is_inert_through_shell` (`ssh_exec.rs:241`) runs a hostile `$(...)`-bearing value **through an actual shell** and asserts the substitution did not execute; plus `test_shell_quote_escapes_single_quotes` (`:230`, incl. `'` and empty-string edge cases) and `test_shell_quote_survives_single_eval_layer_with_metachars` (`:270`).
- `validate_shell_safe` allowlist retained in `executor/ssh_exec.rs` as defense-in-depth (belt-and-braces, as the baseline prescribed).
- C# side unchanged and still SAFE (argv-array CLI, whitelist-regex cloud-init).

**Why not A:** the architecture is still quote-then-interpolate into `bash -c` strings rather than argv-only end-to-end; safety now rests on two independent layers instead of one, which is good, but the pattern remains one careless `format!` away from a gap.

## 3. AuthZ Coverage — A- / 90 (0)

- **Spot-check after #449 split:** `TesterWriteEndpoints.cs` retains all **9** `RequireAuthorization` calls (identical count to pre-split `a46c1324^`); the new `TesterWriteEndpoints.{Create,Lifecycle,Provisioning,Schedule}.cs` files contain handler bodies only, with route registration (and gating) centralized in the registrar. No gate was lost in the move.
- **Baseline minor gap still open:** agent api_key is still compared via EF query equality — `src/Networker.ControlPlane/Realtime/RawWs/AgentMessageProcessor.cs:125`: `.FirstOrDefaultAsync(a => a.ApiKey == apiKey, ct)` — i.e., a DB-side non-constant-time string compare against an **api_key stored plaintext** in `agents.api_key`. The repo's only `FixedTimeEquals` remains the JWT signature path (`JwtTokenService.cs:133`). Unchanged since baseline.
- No new endpoints or policy changes since baseline that would move this grade in either direction.

## 4. Test Quality — B / 85 (+2)

**Counts (test attributes, baseline commit `a46c1324^` vs HEAD):**

| Suite | Baseline | Now | Delta |
|-------|---------:|----:|------:|
| `crates/networker-tester` `#[test]`/`#[tokio::test]` | 878 | 880 | +2 |
| All workspace crates | — | 1,285 | — |
| `benchmarks/orchestrator` | 100 | 104 | +4 (incl. the 3 shell_quote security tests) |
| C# `[Fact]`/`[Theory]` attrs | 434 | 450 | +16 (plus 301 `[InlineData]` cases; ~556+ executed cases) |
| Frontend test files | 7 | 7 | 0 |

- **No regressions from #449–#451:** the three refactors are verifiably move-only (e.g., #449: 1,563 insertions / 1,484 deletions redistributing `TesterWriteEndpoints.cs`); every suite's attribute count is flat or up. Orchestrator golden/snapshot tests added in #440/#450 (`reporter/tests.rs`, executor module tests).
- **New security-property tests are the right kind:** the shell_quote suite asserts behavior through a live shell, not string equality only.

**Baseline thin areas — all unchanged:**
- **Frontend RBAC: still untested.** Same 7 test files; grep for `isAdmin`/`isOperator` across them returns nothing. The hard product requirement (role-gated rendering) still has zero test coverage.
- **WS resilience: unchanged.** The files matching "reconnect" (`AgentRawSocketRegistryTests.cs`, `AgentConnectionRegistryTests.cs`, `RawWebSocketIntegrationTests.cs`) all predate the baseline (#395/#402) and were already counted; no new reconnection/backoff/fragmentation tests.
- **Cloud provisioning + SSH exec path:** no new tests; still arg-builder/scope-logic only.

Score moves 83 → 85: real, verified growth and better security-property testing, but none of the three named high-value gaps closed.

## 5. Dependency Hygiene — A- / 90 (+3)

- **cargo-audit gate: LIVE.** `.github/workflows/rust-audit.yml` — PR-triggered (required-check-compatible pattern: no workflow-level `paths:`, cheap `changes` job), weekly schedule (Mondays 05:23 UTC, so new advisories surface against unchanged lockfiles), audits **both** lockfiles (workspace `Cargo.lock` + `benchmarks/orchestrator/Cargo.lock`), cargo-audit installed `--locked` from crates.io (no third-party action).
- **Ignore list 11 → 5, every remaining entry's justification verified against `Cargo.lock`:**
  - `RUSTSEC-2023-0071` (rsa Marvin): `rsa 0.9.10` present via jsonwebtoken; **no fixed release exists** (fix slated for 0.10); workspace is HS256-only, no RSA private-key ops; Rust dashboard decommission (~2026-07-30) removes it. **Stands.**
  - `RUSTSEC-2026-0098/0099/0104` (rustls-webpki): lockfile confirms the dual copy — patched `0.103.13` on all modern rustls-0.23 probe paths, vulnerable `0.101.7` only via `tiberius 0.12.3` (opt-in `db-mssql`), and tiberius has no rustls-0.23 release. Exposure limited to the user's own SQL Server TLS. **Stands.**
  - `RUSTSEC-2025-0134` (rustls-pemfile unmaintained): informational, no fix by definition; lockfile shows `1.0.4` (tiberius-pinned) alongside maintained `2.2.0` (direct use). Clears only with the tiberius exit. **Stands.**
  - Upgrades confirmed: `hickory-resolver 0.26.1` (RUSTSEC-2026-0119 cleared in the DNS probe path).
- **Branch protection verified live** (`gh api .../protection/required_status_checks`): 6 required checks — `Test (ubuntu-latest)`, `Test (windows-latest)`, `Detect changed areas`, `Build & audit (C#)`, `bats (installer unit tests)`, `shellcheck`. Note: `Detect changed areas` is ci.yml's changes job and `Build & audit (C#)` is dotnet.yml's NuGet-vuln gate — **the new `cargo audit (RUSTSEC advisories)` job is NOT itself a required check**, so a red Rust audit does not currently block merge.
- **Still missing:** Dependabot/Renovate absent (verified); the two `setup-dotnet@v4` remain tag-pinned (`dotnet.yml:75`, `release.yml:276`).

---

## Remaining Fixes (priority order)

1. **P2 (only untouched baseline security finding):** Agent api_key — hash at rest and compare with `CryptographicOperations.FixedTimeEquals` instead of the EF `a.ApiKey == apiKey` lookup (`AgentMessageProcessor.cs:125`).
2. **P2:** Frontend role-visibility tests (viewer/operator/admin control gating) — the hard product requirement still has zero coverage; add at least one WS reconnection/backoff test alongside.
3. **P2:** Make `cargo audit (RUSTSEC advisories)` a required branch-protection check (it was built required-check-compatible for exactly this); add Dependabot (actions + nuget + npm + cargo); SHA-pin the two `setup-dotnet@v4`.
4. **P3:** Secrets hardening tail — add a secret-scanning CI gate (gitleaks); replace the interop test's expired committed token with one minted in-test (the expiry guard makes the first assertion permanently dead as of 2026-07-16); drop the real prod email/sub UUID from the synthetic token claims.
5. **P3 (tracked):** tiberius exit (drop or re-TLS the MSSQL backend) clears the last 4 ignore entries in one move.
