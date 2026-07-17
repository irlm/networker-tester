# Scorecard — Security Posture + Test Quality

Read-only review. Platform: hybrid Rust + C#. C# ControlPlane serves prod (alethedash.com).
Date: 2026-07-17. Scope: secrets, injection surfaces, authz coverage, test quality, dependency hygiene.

## Grade Table

| # | Dimension | Grade | One-line justification |
|---|-----------|-------|------------------------|
| 1 | Secrets handling | **D** | Prod injection is clean (env/GH secrets, generated at install, nothing logged) — but the **live prod JWT signing secret is hardcoded in a committed test** (P0). |
| 2 | Injection surfaces | **B** | C# cloud CLI + SSH + cloud-init are argv-array/regex-whitelisted (SAFE); the Rust orchestrator interpolates config values into `bash -c` strings, held safe only by a `validate_shell_safe` allowlist (defense-in-depth, fragile). |
| 3 | AuthZ coverage | **A-** | Consistent RBAC (global + per-project hierarchy), all mutations gated, no anonymous-by-accident routes; open surfaces intentional and bounded. Minor: agent api_key uses non-constant-time compare + stored plaintext. |
| 4 | Test quality | **B** | Security primitives (JWT/cipher/authz) and Rust measurement-correctness are genuinely deep incl. denial paths and trust-audit bounds; cloud provisioning, SSH exec, WS resilience, and frontend RBAC are thin/absent. |
| 5 | Dependency hygiene | **B+** | C# vuln gate + npm audit gate present, 97% of GH Actions SHA-pinned; **no Rust cargo-audit/deny** for still-shipped crates, no Dependabot/Renovate. |

**P0 count: 1** (hardcoded production JWT secret).

---

## 1. Secrets Handling — D

### P0 — Production JWT signing secret committed to the repo
`tests/Networker.ControlPlane.Tests/JwtShortKeyInteropTests.cs:11`
```csharp
// The exact prod secret (29 bytes) and a real Rust-minted token captured during cutover.
private const string ProdSecret = "<redacted-rotated-2026-07-17>";
private const string RustToken = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...admin@alethedash.com...";
```
The comment self-identifies this as **the exact prod secret** for the live domain (`alethedash.com`), and the bundled token decodes to `admin@alethedash.com`, `role: admin`, `is_platform_admin: true`. Because the platform intentionally signs with **raw HMAC-SHA256 accepting any key length** (short-key Rust interop), anyone with repo (or git-history) read access can **forge platform-admin JWTs** against prod. This is a critical credential leak.
- **Fix (loud):** Rotate `DASHBOARD_JWT_SECRET` in prod immediately; invalidate outstanding tokens (24h TTL). Replace the test constant with a synthetic secret + synthetic token (the regression only needs a <32-byte key and a matching signature, not the real one). Scrub git history / rotate is mandatory since the secret is already in history.

### Clean parts (why it isn't lower)
- Prod secrets injected via env + GH Actions secrets, never hardcoded: `${{ secrets.AZURE_CREDENTIALS }}`, `${{ secrets.DASHBOARD_ADMIN_PASSWORD }}`, `${{ secrets.GIST_TOKEN }}`, etc.
- Installer generates random DB password + JWT secret at install time into `/etc/networker-dashboard.env` (`install.sh:~4167-4189`) — not hardcoded.
- No secret values logged: searches for `LogInformation`/`tracing::`/`println!` around password/secret/token found none leaking values; auth code logs actions ("Password changed") not values.
- Dev defaults are intentional and correctly scoped: `docker-compose.db.yml:15` (`NetworkerTest1!`), `docker-compose.dashboard.yml:9` (`networker`), CI `NETWORKER_SQL_CONN` in `.github/workflows/ci.yml`, `JwtTokenService.cs` dev fallback `dev-insecure-jwt-secret-change-me-please-32b` (throws at startup outside Development). These are fine.

---

## 2. Injection Surfaces — B

### SAFE (C# control plane)
- **CliComputeProvisioner** (`src/Networker.ControlPlane/Provisioning/CliComputeProvisioner.cs:146,177,218`): Azure/AWS/GCP invoked with `List<string>` → `ArgumentList` and `UseShellExecute=false` (`:1103,1108`). User-controllable VM name / region / resource id are literal argv elements — no shell. **SAFE.**
- **SshLanguageDetector** (`SshLanguageDetector.cs:85-92,128`): `ssh` via `ArgumentList`; the remote `command` is a compile-time constant; host/user from DB, passed as argv. **SAFE.**
- **CloudInitScripts** (`CloudInitScripts.cs:27-59,117-120`): three placeholders validated by whitelist regex before substitution:
  - `target_triple`: `^[A-Za-z0-9_-]+$`
  - `api_key`: `^[A-Za-z0-9]{32,128}$`
  - `dashboard_url`: `^(https?|wss?)://[A-Za-z0-9.\-:]+(/[A-Za-z0-9._\-/]*)?$`
  All block quotes, `$`, backticks, `;`, `|`, newlines; substituted into systemd `Environment=` and PowerShell `SetEnvironmentVariable`. Whitelist is sufficient for those contexts. **SAFE.**

### RISKY (Rust orchestrator) — fragile-by-design
`benchmarks/orchestrator/src/executor.rs` builds remote commands by interpolating config-derived strings into `bash -c` / bash command strings, then ships them via `ssh_exec` (which itself is a safe argv wrapper — `ssh.rs:64-96` — so the risk is entirely in the payload string):
- `:518-522` `deploy_proxy()` — `{proxy}` into `bash -c '... --benchmark-proxy-swap {} ...'`
- `:553-556` `deploy_app_language()` — `{language}` and `{proxy}` into an `export ... | bash -s` string
- `:456-460` `start_existing_server()` — `{lang}` into a bash string
- `:589` `run_chrome_benchmark()` — `--token {bench_token}` unescaped (token is system-generated today, not user-controlled)

Each is guarded by `validate_shell_safe(...)` (`:501-507`, allowlist `[a-zA-Z0-9\-_.]`) applied before interpolation, which does block metacharacters. Inputs originate from **benchmark config JSON** (`config.rs`) — file/API-sourced, i.e. potentially operator-controllable. Verdict **RISKY**: safe today only because the allowlist is never removed; the pattern (unescaped interpolation into a shell string) is one refactor away from injection.
- **Fix:** pass the payload as `bash -s` stdin or use `shlex`/single-quote escaping rather than relying solely on the allowlist; keep `validate_shell_safe` as defense-in-depth.

---

## 3. AuthZ Coverage — A-

Auth is enforced via `UseNetworkerAuth()` (`Program.cs:103`); per-endpoint opt-in with `.RequireAuthorization(policy)` / explicit `.AllowAnonymous()`. Role hierarchy `Admin ≥ Operator ≥ Viewer` (global) and `ProjectAdmin ≥ ProjectOperator ≥ ProjectViewer` (per-project), policies in `AuthExtensions.cs:88-100`; flat routes use row-level `ProjectAccessChecker`.

Sampled 15+ endpoints — all correctly gated:

| Route | Method | Auth | Evidence |
|-------|--------|------|----------|
| `/api/health`, `/api/health/ready` | GET | Anonymous (intentional) | `Program.cs:107`, `OpsEndpoints.cs:97` |
| `/api/version` | GET | Authenticated | `VersionEndpoints.cs:107` |
| `/api/auth/login` | POST | Anonymous | `AuthExtensions.cs:126` |
| `/api/auth/forgot-password` / `reset-password` | POST | Anonymous, token-gated | `AccountEndpoints.cs:91,137` |
| `/api/users` (+ pending/invite/approve/disable) | GET/POST | GlobalAdmin | `UsersEndpoints.cs:42,50,58,120,183` |
| `/api/admin/*` | GET/POST | GlobalAdmin | `AdminEndpoints.cs:39,58,89` |
| `/api/projects/{id}` | GET | ProjectMember | `ProjectsEndpoints.cs:93` |
| `/api/v2/.../test-configs` | POST | ProjectOperator | `TestConfigWriteEndpoints.cs:38` |
| `/api/v2/test-configs/{id}` | PATCH/DELETE | Auth + row-level Operator | `TestConfigWriteEndpoints.cs:108,177` |
| `/api/projects/{id}/testers` | POST | ProjectOperator | `TesterWriteEndpoints.cs:50` |
| `/api/projects/{id}/testers/{id}/force-stop` | POST | ProjectAdmin | `TesterWriteEndpoints.cs:59` |
| `/api/projects/{id}/members` | GET/POST | ProjectAdmin | `MembersEndpoints.cs:44,76` |
| `/api/projects/{id}/cloud-accounts` | POST | Operator(personal)/Admin(shared) | `CloudAccountsEndpoints.cs:62` |
| `/api/leaderboard*` | GET | Anonymous (intentional, stub) | `LeaderboardEndpoints.cs:17,20,23` |
| `/ws/agent?key=` | WS | api_key, validated pre-upgrade → 401 | `AgentSocketEndpoint.cs:71-87` |

- **No anonymous-by-accident routes**, no viewer-reachable mutations, all `AllowAnonymous` are login/reset/invite/leaderboard (token-gated or intentionally public).
- Open surfaces bounded: health/ready return no error detail; `/api/version` exposes only a public compile-time string; leaderboard is a stub.
- **JWT** (`JwtTokenService.cs`): raw HMAC-SHA256, `ValidAlgorithms=["HS256"]` (no alg-confusion), constant-time signature compare (`FixedTimeEquals`, `:133`), expiry enforced w/ 60s skew, 24h TTL. Sound — the weakness is the *secret management* (see #1), not the validation. No token revocation/logout (Low).

### Minor gaps
- Agent api_key compared with plain `==` (`AgentMessageProcessor.cs:125`) — not constant-time; and stored plaintext (`agents.api_key`). Medium. Use `FixedTimeEquals` + hash at rest.
- `/api/system/health` enforces platform-admin via inline check (`SystemHealthEndpoints.cs:43-50`) instead of a policy — consistency nit.
- Password-reset / invite tokens hashed SHA-256 without salt (matches Rust) — Low.

---

## 4. Test Quality — B

**Deep where it counts:**
- **AuthZ denial paths tested** (not just happy paths): `ControlPlaneIntegrationTests.cs:217-237` asserts 401 unauthenticated, 403 non-member project, 403 non-platform-admin on `/api/users` — against real Postgres + WebApplicationFactory.
- **JWT negatives:** `JwtTokenServiceTests.cs:92-126` rejects wrong secret + expired token; short-key interop + tamper in `JwtShortKeyInteropTests.cs`.
- **CredentialCipher (gold standard):** `CredentialCipherTests.cs` — roundtrip sizes, wrong-key AEAD failure, bit-flip tamper rejection, key-rotation fallback, and a cross-language Rust known-answer vector.
- **Rust measurement correctness present (trust-audit bounds verified):** `crates/networker-tester/tests/integration.rs` pins `/delay?ms=100` → TTFB ≥ 90ms and ≤ 500ms, total ≤ 600ms (`:397-431`); UDP throughput asserts loss derived from `CMD_REPORT` not assumed + transfer window excludes report wait (`:931-985`); TLS handshake < 100ms to prove trust-store I/O isn't leaking (V5, `:1091-1155`); error classification (`connection refused → Tcp`, `:456-481`); unified 4xx-is-failure across HTTP versions (`:595-634`). These are correctness assertions, not smoke tests.

**Thin / absent:**
- **Frontend RBAC untested (high-value gap):** 7 test files (`client.test.ts`, `CreateTesterModal`, `TesterDetailDrawer`, `TesterRegionGroup`, `PhaseBar`, `LanguageSelector`, `testerSubscription`). API client 401/403 handling is well covered (`client.test.ts:45-77`) but **no test verifies role-gated rendering** (`isAdmin`/`isOperator` hiding controls) despite that being a hard product requirement.

### 3 weakest-tested high-risk areas
1. **Cloud provisioning + orphan reaper (~40%):** only CLI-arg builders and reaper scope logic tested (`CliProvisionerCreateArgsTests`, `OrphanReaperScopeTests`); no test executes a real `az/aws/gcloud` call or a provision→register→reap reconciliation.
2. **SSH exec + agent command approval (~20%):** language detection, token gen, approval-decision logic tested in isolation; the full request→approve→execute path and credential non-leakage into command output are untested (`SshLanguageDetectorTests`, `CommandTokenTests`, `ApprovalDecisionTests`).
3. **WebSocket/SignalR live streaming resilience (~50%):** bridge fan-out and subscribe framing tested (`RawWsBridgeTests`, `testerSubscription.test.ts`) but no reconnection/backoff, fragmented-frame, or concurrent-load coverage — plus **frontend role-visibility rendering** (tie-in with #4 gap).

---

## 5. Dependency Hygiene — B+

- **C# vuln gate: present.** `.github/workflows/dotnet.yml:49-67` runs `dotnet list package --vulnerable --include-transitive` over `src/*/*.csproj` and fails on "has the following vulnerable packages". Scoped to shipped projects.
- **npm gate: present.** `.github/workflows/ci.yml:320-321` `npm audit --audit-level=high`.
- **Rust vuln gate: MISSING.** No `cargo-audit`, `cargo deny`, `deny.toml`, or `audit.toml` anywhere — yet `networker-tester`/`networker-endpoint` still ship. Gap.
- **GH Actions pinning: 72/74 SHA-pinned (97%).** Two tag-pinned exceptions, both `actions/setup-dotnet@v4` (`dotnet.yml:37`, `release.yml:276`) with an acknowledged TODO to pin.
- **Dependabot/Renovate: none** (`.github/dependabot.yml` / `renovate.json` absent) — dependency bumps are manual.

**Fixes:** add a `cargo audit`/`cargo deny` job for shipped crates; SHA-pin the two `setup-dotnet@v4`; add Dependabot (Actions + NuGet + npm + cargo).

---

## Prioritized Fixes

**P0 — SECURITY-CRITICAL (do first)**
1. **Rotate the leaked prod JWT secret NOW.** `JwtShortKeyInteropTests.cs:11` commits `<redacted-rotated-2026-07-17>` (the live `alethedash.com` signing key per its own comment) + a platform-admin token. With raw-HMAC any-length signing, this allows admin token forgery against prod. Rotate `DASHBOARD_JWT_SECRET`, invalidate outstanding tokens, replace the test with synthetic values, and treat git history as compromised for this key.

**P1**
2. Add a Rust `cargo audit`/`cargo deny` CI gate (parity with the C#/npm gates) for the still-shipped probe crates.
3. Harden the orchestrator SSH payload construction (`executor.rs:456,518,553,589`): escape via `shlex`/single-quote or pass over `bash -s` stdin instead of relying solely on `validate_shell_safe`.

**P2**
4. Agent api_key: constant-time compare (`FixedTimeEquals`) + hash at rest (`AgentMessageProcessor.cs:125`).
5. Add frontend role-visibility tests (viewer vs operator vs admin control gating) and at least one WS reconnection/backoff test.
6. Add Dependabot config; SHA-pin the two `setup-dotnet@v4` actions.
