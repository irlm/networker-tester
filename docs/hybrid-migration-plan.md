# Hybrid Migration Plan — Rust → C# / .NET 10

**Status:** Phase 0 + Phase 1 scaffolded and proven; Phase 2 proof-of-concept working (the C# solution is `Networker.sln` at the repo root — projects under `src/`, tests under `tests/Networker.Tests/`).
**Decision:** Keep the Rust probe engine; re-architect the control plane, agent, and endpoint in C#/.NET 10.
**Score:** Hybrid **7.9/10** vs. stay-in-Rust 6.6 vs. full-C#-rewrite 6.3 (weighted for a solo, C#-fluent owner).

---

## Principle: recreate the function, don't transliterate the code

For every component, the test is *what does this accomplish?* — then reach for the framework feature that already does it, rather than re-typing the Rust mechanism in C#.

| Rust today | Transliteration ❌ | Re-architecture ✅ |
|---|---|---|
| 3 hand-rolled WS hubs (`HashMap<ConnId,Sender>` + ping/pong) | port to `ConcurrentDictionary` | **SignalR** groups + strongly-typed hubs; reconnection/backplane free |
| ~40 hand-written SQL migrations + manual row mapping | SQL in C# strings | **EF Core** model + generated migrations + LINQ (dropped-table → compile error) |
| `tokio::spawn` loops (scheduler, reaper, auto-shutdown) | `Task.Run` loops | **`BackgroundService`/`IHostedService`** + Quartz.NET (clustered scheduler) |
| shell out to `az`/`aws`/`gcloud` | shell CLIs from C# | **cloud SDKs** behind `IComputeProvisioner` (typed, retryable, mockable) |
| hand-rolled JWT/SSO | port validation | **ASP.NET `JwtBearer` + OpenIdConnect** |

**Reuse (don't rebuild):** the Rust `networker-tester` (measurement moat), the React frontend, the DB schema (captured once as an EF model), and the wire contract (REST + WS) so cutover is a config flip and the frontend/agents don't notice.

---

## Why hybrid, in one paragraph

104k Rust LOC. The bugs this project keeps shipping live in the **41.6k-line control plane** (hand-SQL drift, hand-rolled WS/dispatch/lifecycle) — exactly what EF Core + SignalR + hosted services eliminate by construction. The **moat** is only ~3,500 lines of true low-level measurement inside the 56k tester crate — code that's rarely touched and that .NET genuinely can't match (Linux `TCP_INFO`, HTTP/2-3 sub-phase timing, cross-compiled single binary). So: **rewrite the churn, freeze the crown jewels.** The hybrid deletes ~52k lines of Rust (dashboard/agent/endpoint/common/log) and keeps one small crate behind a stable contract.

**Build-time bonus:** the slow ~13-min release is 4 parallel Rust cross-compiles of the whole workspace. After the hybrid, C# compiles in ~1–2 min and only the small tester crate stays Rust → release drops to ~3–4 min, and the daily edit→run loop becomes `dotnet run`.

---

## The seam: `networker-tester --json-stdout`

The Rust probe core exposes a **versioned JSON contract** (`schema_version` field, currently `"1.0"`). The C# side consumes it via `Networker.Contracts`. Two integration modes:

- **Phase 1 (now):** the C# agent shells out to the tester binary via `System.Diagnostics.Process`. Zero FFI; the seam already existed (the old Rust agent shelled out too). **Verified working** — a C# `BackgroundService` runs the Rust tester and parses real per-phase timings.
- **Later (optional):** compile the Rust core as a C-ABI `cdylib` and P/Invoke it for tighter integration. Points the native-DLL idea at the existing Rust rather than new C++.

### Differential-testing (multi-language oracle)

Because the contract is language-agnostic, the probe core could be re-implemented in C++ or Zig and validated against the Rust implementation: run N iterations of each against a fixed local endpoint and assert their **measurement distributions** match (statistically equivalent p50/p95, not exact values — network noise). This proves the numbers are methodology-driven, not language artifacts, and lets the core language be chosen on developer-delivery-speed. (See `docs/dotnet-migration.md`.)

**On C++ specifically:** don't rewrite the working, memory-safe Rust core into C++ to ship — modern C++ safety (RAII, smart pointers, sanitizers) is opt-in *runtime* tooling, not Rust's compile-time proof, and this is the code that parses untrusted network bytes. Use C++/Zig only as validation oracles or where a platform demands it.

---

## Target stack (.NET 10 LTS)

.NET 10 is the LTS (3-year support) — right for a long-lived rewrite: improved NativeAOT, HTTP/3, EF Core 10, ASP.NET Core 10, C# 14. All projects target `net10.0`.

| Concern | Tech |
|---|---|
| Control plane | ASP.NET Core Minimal APIs |
| Live updates (browser + agent) | SignalR (MessagePack; Redis backplane for scale-out) |
| Data | EF Core 10 + Npgsql (database-first from the existing schema, then model-first) |
| Background loops | `BackgroundService` + Quartz.NET |
| Provisioning | `Azure.ResourceManager`, `AWSSDK.EC2`, `Google.Cloud.Compute` behind `IComputeProvisioner` |
| Local dev + tracing | .NET Aspire (one-command up; OpenTelemetry traces) |
| Tests | xUnit + Testcontainers (real Postgres, real migrations) + `WebApplicationFactory` |
| Reports | ClosedXML (Excel), Razor/string templates (HTML) |

---

## Phased plan (strangler-fig; ships continuously; ~3–5 months solo)

**Phase 0 — Freeze the seam** *(done)*
`schema_version` added to the tester JSON (additive, backward-compatible) + golden contract test. `Networker.Contracts` mirrors it.

**Phase 1 — C# agent shells the Rust binary** *(scaffolded + proven)*
`Networker.Agent` (`IHostedService` + DI, SignalR client stubbed as `IDashboardClient`). Verified: C# → Rust tester → parsed `schema_version=1.0` + dns/tls/ttfb/total timings. First shippable milestone.

**Phase 2 — C# control plane** *(PoC working in `src/Networker.ControlPlane`; full build is the main effort)*
`Networker.Data` (EF Core DbContext scaffolded database-first from the live schema) + `Networker.ControlPlane` (Minimal APIs + SignalR hub). PoC serves `/api/health`, `/testers`, `/test-runs` from the **real Postgres** via LINQ, and `/ws/dashboard` negotiates with transport fallback. Full phase: port all REST endpoints + WS events behind the existing contract, add EF migrations (model-first from here), cloud SDKs, auth. Cut over by running C# on a separate port, pointing one agent + staging frontend at it, validating parity, then flipping config. Rollback = point back at the Rust dashboard.

**Phase 3 — Endpoint + cleanup** *(1–2 wk)*
Port `networker-endpoint` to a Minimal API; fold `networker-common`/`networker-log` into C#. Decommission the Rust dashboard/agent/endpoint crates. Repo = 1 Rust crate (`networker-tester`) + 1 .NET solution + React.

---

## Risks & kill criteria

- **Contract drift** (highest probability): the JSON/CLI seam breaks silently. *Mitigation:* schema-versioned JSON + golden tests in CI against the real binary.
- **Second-system syndrome:** "while rewriting, let me redesign the schema/API." *Mitigation:* Phase 2 preserves the wire contract byte-for-byte; redesign is a separate later project.
- **Kill switch:** if Phase 1 (the cheapest) fights you → stop, stay Rust + add tests (lost only weeks). If Phase 2 slips past ~2× estimate with no cutover → freeze C#, keep Rust in prod, reassess. Any measurable fidelity regression in a moved component → roll that component back to Rust-as-binary. If velocity doesn't actually improve in C# on the next 5 bugs → the premise fails; stop.

**Rollback is cheap by design:** the wire contract never changes, so every phase can point back at the still-running Rust equivalent — one config flip from the last working state.

---

## Verified so far (branch `feat/hybrid-phase0-scaffold`)

```bash
# Rust probe core (contract producer) — builds, 676 lib + 3 contract tests pass
cargo build -p networker-tester
cargo test  -p networker-tester --test json_contract

# C# app layer (contract consumer) — builds clean
dotnet build Networker.sln

# Phase 1 seam — C# agent shells the Rust tester (real per-phase timings)
AGENT_TESTERPATH="$(pwd)/target/debug/networker-tester" dotnet run --project src/Networker.Agent

# Phase 2 control plane — EF Core → real Postgres, SignalR negotiate
dotnet run --project src/Networker.ControlPlane --urls http://127.0.0.1:5210
#   GET /api/health                         → {"status":"ok","db":"ok"}
#   GET /api/projects/{id}/testers          → live testers via LINQ
#   GET /api/projects/{id}/test-runs        → EF join TestRun→TestConfig
#   POST /ws/dashboard/negotiate            → SignalR (WebSockets + SSE)
```
