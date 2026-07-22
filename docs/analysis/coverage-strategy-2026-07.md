# Test-coverage strategy (2026-07)

Synthesis of three risk-weighted surveys (`coverage-rust-2026-07.md`,
`coverage-controlplane-2026-07.md`, `coverage-libs-sdks-frontend-2026-07.md`).
The goal is **tests that catch real regressions**, not a line-% .

## Principles (why we add a test, and why we don't)

1. **Test where a bug ships green *and wrong*.** The enemy is a defect that
   passes CI and corrupts a measurement, a verdict, an on-screen number, a
   permission, or a cloud action. Executing a line ≠ verifying its behavior.
2. **Every test must be able to fail for a real reason.** No `assert value > 0.0`
   over hardcoded inputs (that passes even if the number is wrong). Use
   known-input → known-output vectors so a logic change trips the test.
3. **Rank by blast radius × likelihood × invisibility.** Invisibility = "what
   else would catch this?" A wrong percentile is invisible (no crash, no log); a
   null-deref usually isn't. Prioritize the invisible.
4. **Don't retest what's already well covered** (false productivity). Leave
   alone: `CredentialCipher` (round-trip + tamper + wrong-key + rotation),
   `throughput.rs` TCP math (57 tests), the delete cascade, dispatch token
   isolation, the schema migrator, the JSON contract, the reporters (90%+).
5. **A test that never runs is worse than none** — it's false confidence. Fix
   the CI so the suite actually executes on the changes that touch it.

## Where we actually are (grounded)

| Area | State | The real risk |
|---|---|---|
| Rust probe engine | 47.6% lib; core well-tested | The **output/verdict layer** (`summary.rs`, `dispatch.rs`) is 0% — "the product silently lies" |
| C# control plane | Crown jewels well-tested on **real Postgres** (Testcontainers) | Gap is **breadth**: authz auto-tested on ~4/40 modules; background cloud-mutation loops untested |
| Frontend | 159 tests, but stat math untested | `lib/analysis.ts` (percentiles/format for every view) = wrong on-screen numbers; RBAC render gating gaps |
| SDKs (5 langs) | Best-covered surface; all 7 security props | Python thinnest (no streaming-memory test) |
| C# support libs | `Security` exemplary | `Agent` resilience (backpressure/reconnect) untested |
| **CI itself** | — | `sdk/csharp/**` not in the C# build path filter → the C# SDK tests run on **no** SDK-only PR |

The feared systemic weakness (SQLite-only tests hiding Postgres bugs) is **not**
the reality — the control-plane HTTP suite runs against real Postgres, so the EF
`.Select()`/JSONB/raw-SQL bug class *is* caught for tested endpoints. The gap is
which endpoints are tested, not how.

## Prioritized backlog (each line = the bug it catches)

### Tier 0 — CI infra (do first; unblocks everything)
- **Add `sdk/csharp/**` to the C# build path filter** (`dotnet.yml`). Today the C# SDK tests never run on an SDK-only change — a real regression there ships green.

### Tier 1 — "the product silently lies" (output correctness; highest value, locally verifiable)
- **`summary.rs`** — known runs → known verdict + p50/p95/aggregates. Catches a wrong percentile / pass-fail flip that no crash reveals.
- **`dispatch.rs::dispatch_once`** — every `Protocol` routes to the right runner. Catches a mis-wired variant (the class of bug the #377–379 fixes chased).
- **`metrics.rs::primary_metric_value/label`** — per-protocol headline-number selector. Catches "wrong field → wrong number everywhere."
- **Frontend `lib/analysis.ts`** — percentile/stat/format vectors. Catches latency↔throughput swaps, div-by-zero, rounding.
- **TLS-resume** integration test — a resumed handshake must set `resumed:true`/`handshake_kind:"resumed"` (today the contract hardcodes `false`; the feature is invisibly dead if broken).

### Tier 2 — safety / isolation breadth (blast radius)
- **`UserStatusMiddleware`** — a disabled/pending user must 403 (account-lifecycle bypass).
- **One parametrized `foreign-id → 404` test per mutating project-scoped module** — isolation is *implemented* everywhere but *auto-tested* on ~4/40; a future dropped `project_id` filter would pass CI silently.
- **Cloud reaper / auto-shutdown scope** — extend the delete-cascade fake-CLI harness to prove they never deallocate an out-of-scope or live-tester VM (wrong-VM-delete).
- **SSO callback** — state/nonce/replay + token-exchange negatives (account-takeover surface).
- **Leader election / single-dispatch** — no double-assignment / double-provision.
- **`UrlTestsEndpoints`** detail-by-`run_id` — add project scoping or a test proving no cross-project leak.

### Tier 3 — resilience & polish
- **Agent `RawWebSocketClient`** backpressure/reconnect — a heartbeat dropped under a frame flood marks a live agent dead.
- **Agent `TesterBinaryLocator`** — a misconfigured path silently falls through to `PATH`; every job then fails "not found."
- **`benchmark.rs`** warmup/pilot/cooldown accounting.
- **Python SDK** — add the streaming memory-bound test the other four languages have; strengthen the concurrency-cap assertion.

## What NOT to do
- No snapshot tests that pin current output without asserting it's *correct*.
- No coverage-% target; a module at 40% with the right 6 assertions beats 90% of getters.
- Don't touch the exemplary suites (above) except to reuse their patterns.

## Execution order
1. **Batch 1 (locally verifiable now):** Tier-1 Rust (`summary`/`dispatch`/`metrics`) + frontend `analysis.ts` — I can run `cargo test`/`cargo llvm-cov` and `vitest` to prove them.
2. **Batch 2 (CI-validated):** Tier-0 CI filter + Tier-2 C# (UserStatusMiddleware, foreign-id→404 harness, reaper scope).
3. **Batch 3:** Agent resilience, TLS-resume, benchmark, Python SDK.
