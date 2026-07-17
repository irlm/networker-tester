# Final Re-score — 2026-07-17 (evening)

Date: 2026-07-17 (late evening) · Version: 0.28.34+ (post-#468) · READ-ONLY.
Baselines: same-day afternoon re-scores —
[`rescore-code-quality.md`](rescore-code-quality.md) (86.2),
[`rescore-security-tests.md`](rescore-security-tests.md) (89.0),
[`rescore-docs-ops.md`](rescore-docs-ops.md) (90.8) — overall **88.7**.

Intervening merges verified in-tree: #452 (orchestrator CI job, soak alerting +
streak, dependabot.yml), cargo-audit promoted to a required branch-protection
check, #458 (api_key hash-at-rest V040 + constant-time compare, global 500
envelope, orchestrator swallow logged, SchemaMigrator wired into startup),
#469 (shared modes manifest + 3-stack drift guards + 5 frontend RBAC suites +
live RBAC hole fix), #470/#471 (dependabot version-check exemption), #468
(first automated dependency update merged end-to-end).

## Score table (all 15 dimensions)

| # | Dimension | Afternoon | Now | Δ |
|---|-----------|----------:|----:|---:|
| **Code quality, naming & structure** | | **86.2** | **88.2** | **+2.0** |
| 1 | Naming consistency | 87 | 87 | 0 |
| 2 | Duplication & drift risk | 85 | 91 | +6 |
| 3 | Module organization | 90 | 90 | 0 |
| 4 | Comment / doc quality | 82 | 83 | +1 |
| 5 | Error handling | 87 | 90 | +3 |
| **Security posture + test quality** | | **89.0** | **91.0** | **+2.0** |
| 6 | Secrets handling | 90 | 90 | 0 |
| 7 | Injection surfaces | 90 | 90 | 0 |
| 8 | AuthZ coverage | 90 | 93 | +3 |
| 9 | Test quality | 85 | 88 | +3 |
| 10 | Dependency hygiene | 90 | 94 | +4 |
| **Docs accuracy + operational readiness** | | **90.8** | **91.6** | **+0.8** |
| 11 | README + docs/ accuracy | 90 | 90 | 0 |
| 12 | CLAUDE.md accuracy | 95 | 92 | −3 |
| 13 | CI/CD coherence | 87 | 91 | +4 |
| 14 | Observability / ops | 87 | 91 | +4 |
| 15 | Release/versioning docs | 95 | 94 | −1 |
| — | **Overall (equal-weight groups)** | **88.7** | **90.3** | **+1.6** |

---

## Verification evidence (every claimed evening fix checked in the tree)

### #452 — orchestrator CI, soak alerting, Dependabot — ALL VERIFIED

- **Orchestrator lint & test job**: `ci.yml` job `orchestrator` (line ~230) —
  `cargo clippy --manifest-path benchmarks/orchestrator/Cargo.toml
  --all-targets -- -D warnings` + `cargo test` on the same manifest, gated by
  the `changes` job (PRs run it only on `benchmarks/orchestrator/**` churn;
  push events always run). **Green on the live post-#468 main run** — the
  #441 clippy-debt regression guard the afternoon report named fix #1 exists
  and works.
- **Soak alerting** (`soak-check.yml`): on `failure()` — create-or-comment a
  pinned issue titled "Soak check failing" carrying run URL + the failing
  step names (queried mid-run from the jobs API); on `success()` — comment
  resolution, unpin, close (title search matches OPEN only, so greens don't
  re-spam). `permissions: issues: write` declared; `gh issue pin` failure is
  a warning, not a job failure.
- **Streak counter**: `Consecutive-green streak` step walks the workflow's own
  run history newest→oldest until the first red (cancelled/skipped ignored)
  and prints the count against the runbook-§7 target of 14 into the step
  summary. The 14-green decommission gate is machine-countable, as prescribed.
- **`.github/dependabot.yml`**: all four ecosystems — `github-actions`,
  `nuget` (`/src`), `npm` (`/dashboard`), `cargo` with **both** directories
  (`/` and `/benchmarks/orchestrator`, matching the dual-lockfile audit) —
  weekly, minor/patch grouped per ecosystem, majors individual.

### cargo-audit as 7th required check — VERIFIED LIVE

`gh api repos/irlm/networker-tester/branches/main/protection/required_status_checks`
returns exactly 7 contexts: `Test (ubuntu-latest)`, `Test (windows-latest)`,
`Detect changed areas`, `Build & audit (C#)`, `bats (installer unit tests)`,
`shellcheck`, **`cargo audit (RUSTSEC advisories)`** — and that string matches
the job `name:` in `rust-audit.yml` (line 68), so the check actually reports.
Last 3 Rust Audit runs green. The afternoon report's dep-hygiene caveat ("a
red Rust audit does not block merge") is closed.

### #458 — api_key hash-at-rest + 500 envelope + swallow log + SchemaMigrator — ALL VERIFIED

- **`V040_agent_api_key_hash.sql`**: adds `api_key_hash VARCHAR(64)`, backfills
  with `encode(sha256(convert_to(api_key,'UTF8')),'hex')` (PG built-ins —
  byte-identical to the C# side), unique index `agent_api_key_hash_key`.
  Header comment explicitly documents that the plaintext column is *kept for
  now* (zero wire-protocol change; drop deferred to a later migration once the
  fleet is verified authenticating against the hash).
- **`Security/AgentApiKeys.cs`**: `HashHex` (SHA-256 lowercase hex) +
  `FixedTimeEqualsHex` via `CryptographicOperations.FixedTimeEquals`.
- **`AgentMessageProcessor.cs:136-141`**: lookup is
  `.Where(a => a.ApiKeyHash == presentedHash)` then re-verified with
  `AgentApiKeys.FixedTimeEqualsHex` — the afternoon's
  `a.ApiKey == apiKey` plaintext EF compare is gone. Key minting
  (`TesterWriteEndpoints.Create.cs:304-310`) writes both columns.
  EF model + unique index mapped (`NetworkerDbContext.cs:90,104-106`).
  Prod verification (42/42 rows hashed, SQL≡C# digest match) is per the #458
  evidence trail — consistent with everything in-tree; not re-runnable
  read-only from here.
- **Global 500 envelope**: `ErrorEnvelope.cs` — `UseExceptionHandler`-based,
  fixed non-leaking `{ "error": "internal server error" }` body, server-side
  `LogError` with method+path; registered **first** in the pipeline
  (`Program.cs`, before `UseNetworkerAuth`). The afternoon's only untouched
  code-quality P1 is closed.
- **Orchestrator swallow**: `ProvisioningOrchestrator.cs:521-530` — the
  `catch { return null; }` in the pending-endpoint parse path now logs a
  warning with the exception and WHY ("a pending-kind ref that fails to parse
  means a stuck run"); the neighboring `FirstEndpointHost` catches the
  narrower `JsonException`. All other catch blocks in the file log.
- **SchemaMigrator wired into startup**: `Program.cs:111-117` —
  `SchemaMigrator.MigrateAsync(connString)` runs unless
  `NETWORKER_RUN_MIGRATIONS=0`; failure refuses startup, which the deploy's
  readiness check converts into an auto-rollback. The "nobody runs it at
  cutover" residual from the afternoon report is closed.

### #469 — shared modes manifest + drift guards + RBAC — ALL VERIFIED

- **`shared/modes.json`**: canonical machine-readable manifest (29 modes with
  level/catalog/family/UI-text fields, groups, cli_aliases), with a `$comment`
  naming all three guards and the update rule.
- **Three-stack drift guards, all present**:
  - Rust: `crates/networker-tester/src/modes_manifest_guard.rs` — 6 tests,
    explicitly **bidirectional** (every manifest tester-mode parses+round-trips;
    every `Protocol` variant is in the manifest; `Protocol::all_modes()` ==
    catalog exactly; runner-level ids are *not* tester protocols).
  - TS: `dashboard/src/lib/modes-manifest.test.ts` (guards
    `mode-family.ts` + `testbed-constants.ts`).
  - C#: `tests/Networker.ControlPlane.Tests/ModesManifestTests.cs` — 5 tests
    against the `/api/modes` catalog, reading the manifest from build output.
- **Phantom ids fixed**: `mode-family.ts` (now at
  `dashboard/src/components/common/`) contains only manifest ids + the
  documented `pageload1` CLI alias, with a header pointing at the manifest and
  its guard. The 6-way unguarded copy — the #377–379 bug class, the afternoon
  code-quality report's worst remaining drift item — is closed *with* guards.
- **RBAC hole fixed + tested**: `TesterDetailDrawer.tsx` gates all four
  mutating-control blocks behind `isOperator` (lines 226/474/531/578).
  **5 new frontend RBAC suites**: `TesterDetailDrawer.rbac.test.tsx`,
  `useProject.rbac.test.tsx`, `InfrastructurePage.rbac.test.tsx`,
  `Sidebar.rbac.test.tsx`, `SettingsTabs.rbac.test.tsx`. Frontend test files
  7 → **13**. The afternoon's "hard product requirement with zero coverage"
  finding is closed — and the guard-writing found a live viewer-facing hole,
  proving the tests were load-bearing on day one.

### #470/#471 — dependabot version-check exemption — VERIFIED (proven by failure)

`ci.yml` version-check exempts when
`github.event.pull_request.user.login == "dependabot[bot]"`, with an in-file
comment explaining exactly why it's the PR **author** and not `github.actor`
(a maintainer's update-branch push made the actor the maintainer — "broke the
exemption on first use"). CI history confirms the story: the tower-http PR's
`Version bump check` failed at 22:13 and passed at 22:21 after #471. Works
now; it took two iterations on live traffic.

### #468 — first automated dep update — VERIFIED

`chore(deps): Bump tower-http from 0.6.11 to 0.7.0` merged (commit
`0692d0f4`); `Cargo.lock` carries `tower-http 0.7.0` for the workspace's
direct consumers (a 0.6.11 copy remains as a transitive dep of axum —
normal dual-version residue, both audited). Full dependabot → CI →
exemption → merge pipeline exercised end-to-end on day one.

---

## New issues found from the evening churn

1. **CLAUDE.md drifted twice** (dimension 12, −3):
   - The required-checks list (line 187) still says **6** checks; live branch
     protection has **7** (`cargo audit (RUSTSEC advisories)` missing from the
     doc). Introduced the moment the check was promoted.
   - The "Adding a New Protocol Variant" checklist (line 148) does **not**
     mention `shared/modes.json` — the manifest's own `$comment` says "update
     this file in the same PR", and the three guards make the omission
     self-enforcing (CI fails), but the checklist exists to capture exactly
     this step and now under-documents it.
2. **release-flow.md doesn't mention the dependabot ride-next-release policy**
   (dimension 15, −1): dep-only PRs now merge without a version bump and their
   lockfile changes ship with the *next* bumped release — documented in
   `dependabot.yml` and the ci.yml comment, but the release prose doc is
   silent on it.
3. **`Orchestrator lint & test` is not itself a required check** — a red
   orchestrator clippy/test on a PR does not block merge (it's in the CI
   workflow but not in the 7 required contexts). Mitigated: push events always
   run it, and the area-gating means PR reds are visible on exactly the PRs
   that touch it.
4. Nothing else: spot-checks of the new files (AgentApiKeys, ErrorEnvelope,
   modes_manifest_guard, soak-check steps) found no bare catches, no unwraps,
   no unlogged failure paths; the current main push run is green on every
   completed job.

## Per-dimension deltas — reasoning in one line each

- **Duplication 85→91**: the last *materialized, unguarded* drift class
  (mode lists ×6) got a canonical manifest + guards in all three stacks that
  caught real drift immediately; SchemaMigrator now actually runs in prod
  startup. Remaining: hand-written TS API types, golden not CI-regenerated.
- **Error handling 87→90**: both C# gaps (global 500, silent swallow) closed
  exactly as prescribed; Rust string-matched classification
  (`http.rs:602`, `http3.rs`) and `metrics.rs:1439 partial_cmp().unwrap()`
  remain.
- **Comment quality 82→83**: new artifacts again exemplary (V040 header is a
  model migration comment; ErrorEnvelope doc states the before/after
  contract); the prescribed stale-`crate::`-comment sweep still hasn't run.
- **AuthZ 90→93**: the only untouched baseline security finding (plaintext,
  non-constant-time api_key) closed with a matching SQL backfill and prod
  verification; a live viewer-RBAC hole found+fixed+tested. Held back from
  95 by the plaintext column still being written (drop pending fleet
  verification) and client-side-only nature of the frontend gating.
- **Test quality 85→88**: frontend RBAC (the named product-requirement gap)
  now covered; 3-stack manifest guards; orchestrator tests in CI. WS
  resilience and cloud-provisioning/SSH paths still thin.
- **Dep hygiene 90→94**: required cargo-audit + full Dependabot + a merged
  automated update. `setup-dotnet@v4` still tag-pinned ×2; exemption needed
  two same-day fixes.
- **CI/CD 87→91**: afternoon fix #1 (orchestrator lint/test) done; auto-tag
  still `needs: [lint, test-ubuntu, frontend]` with no C# leg (ci.yml:568).
- **Observability 87→91**: afternoon fix #2 (soak alerting + machine streak)
  done and well-built; untested in anger (zero real failures since it landed —
  one scheduled green on 2026-07-17), still no "prod at a glance" page.
- **CLAUDE.md 95→92 / release docs 95→94**: the only regressions — the
  evening's infra changes outran the docs by a few lines (see above).
- **Unchanged (87, 90, 90, 90, 90)**: naming, module org, secrets, injection,
  README/docs — no evening change touched their remaining items (branding.md
  is *still* missing from the docs/README.md index).

## What genuinely remains

**Near-term (the honest list):**
1. **Drop the plaintext `agent.api_key` column** — deliberate V040 deferral;
   needs the fleet verified authenticating via hash first, then a V041 drop +
   removal of the dual-column insert in `TesterWriteEndpoints.Create.cs`.
2. **Soak alerting is untested in anger** — the pinned-issue create/resolve
   path has never fired on a real failure; `gh issue pin` may also lack rights
   with the default token (the workflow warns rather than fails). Consider a
   one-off dispatch with a forced failure to prove the loop.
3. **Dependabot day-one flake** — the version-check exemption broke on first
   contact (#470 then #471); it's correct now, but watch the first full
   weekly batch (Mondays) for the grouped-PR + update-branch interaction.
4. **Doc catch-ups (minutes of work)**: CLAUDE.md required-checks 6→7 +
   add `shared/modes.json` to the protocol-variant checklist; add
   `branding.md` to `docs/README.md`; one line in release-flow.md on the
   dependabot ride-next-release policy.
5. **CI tails**: make `Orchestrator lint & test` required (it's already
   required-check-shaped); add the C# build to auto-tag's `needs`; SHA-pin
   `setup-dotnet@v4` ×2.

**Carried P2/P3 (unchanged from the afternoon reports):**
stale `crate::`/`db::` provenance comments in `Endpoints/`; typed error
classification at `http.rs:602`/`http3.rs` + `total_cmp` at `metrics.rs:1439`;
WS-resilience and cloud-provisioning/SSH-exec tests; gitleaks CI gate + mint
the interop token in-test (committed one expired 2026-07-16); env-var
rationalization (`DASHBOARD_*` → `CONTROLPLANE_*`); golden-regen automation;
`pageload.rs` (3,817) split; OpenAPI-generated TS types; "prod at a glance"
page; tiberius exit (clears the last 4 audit-ignore entries).
