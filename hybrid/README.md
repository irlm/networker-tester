# Networker Hybrid (Rust probe core + C# app layer)

This directory holds the first two phases of a **hybrid** migration for the
network-diagnostics platform. The decision is settled:

- **Keep** the Rust probe engine (`networker-tester`) — the measurement core.
- **Rewrite** the control-plane / agent / endpoint app layer in C# (.NET 8),
  on developer-delivery-speed grounds.

The two halves never link against each other. They communicate across a stable,
versioned data contract.

---

## The seam: `networker-tester --json-stdout`

`networker-tester --json-stdout` emits a single JSON document per target — a
serialized `TestRun` — carrying a top-level **`schema_version`** string plus,
for every probe attempt, the per-phase timings:

| Phase | JSON path (per attempt)        | Rust field                    |
|-------|--------------------------------|-------------------------------|
| DNS   | `dns.duration_ms`              | `DnsResult::duration_ms`      |
| TCP   | `tcp.connect_duration_ms`      | `TcpResult::connect_duration_ms` |
| TLS   | `tls.handshake_duration_ms`    | `TlsResult::handshake_duration_ms` |
| TTFB  | `http.ttfb_ms`                 | `HttpResult::ttfb_ms`         |
| Total | `http.total_duration_ms`       | `HttpResult::total_duration_ms` |

This JSON **is the contract**. The C# side (`Networker.Contracts`) models the
subset of fields it consumes; unknown fields are ignored on deserialization, so
the schema can grow **additively** without breaking consumers. Structural
changes (renames, removals) require bumping `schema_version` (currently `1.0`,
defined by `SCHEMA_VERSION` in `crates/networker-tester/src/metrics.rs`).

The Rust side of the contract is pinned by the golden-style test
`crates/networker-tester/tests/json_contract.rs`, which asserts the presence of
`schema_version` and all five phase timings without any network I/O.

```
┌──────────────────────┐   networker-tester --json-stdout   ┌─────────────────────┐
│  Rust probe core     │  ───────────────────────────────▶  │  C# app layer        │
│  (networker-tester)  │      schema_version'd JSON          │  Networker.Agent     │
│  Rust today,         │      over a process boundary        │  → Networker.Contracts│
│  C++/Zig later?      │                                     │  (System.Text.Json)  │
└──────────────────────┘                                     └─────────────────────┘
```

---

## Differential-testing architecture (the key idea)

Because the contract is a language-neutral, `schema_version`'d JSON schema,
**any** probe-engine implementation that emits the same schema is a drop-in for
the core — Rust today, potentially C++ or Zig later.

That unlocks **differential testing of the measurement methodology itself**:

> Run N iterations of *each* implementation against the *same* fixed local
> endpoint, then assert their measurement **distributions** are statistically
> equivalent — not exact values (network + scheduler noise makes that
> impossible), but equivalent `p50` / `p95` within a tolerance.

If two independent implementations agree on the distribution, the numbers are
**methodology-driven, not language-artifacts**. That is what lets the owner swap
the core's implementation language purely on delivery-speed grounds, with
evidence that measurements are preserved.

### Harness sketch

```
for impl in [rust, cpp, zig]:            # each emits schema_version'd JSON
    samples[impl] = []
    for i in 1..N:                       # N ~ 100–500
        run impl --target http://127.0.0.1:PORT/health --json-stdout
        parse ProbeRunResult             # via the shared contract
        samples[impl].push(dns_ms, tcp_ms, tls_ms, ttfb_ms, total_ms)

baseline = samples[rust]                 # current core is the reference
for impl in others:
    for phase in [dns, tcp, tls, ttfb, total]:
        assert within_tolerance(p50(baseline[phase]), p50(samples[impl][phase]))
        assert within_tolerance(p95(baseline[phase]), p95(samples[impl][phase]))
        # e.g. two-sided: |a-b| <= max(abs_tol, rel_tol * max(a,b))
        # optionally a distribution test (Mann–Whitney U / K–S) with a p-value gate
```

Design notes:
- **Fixed local endpoint** (`networker-endpoint` on loopback) removes WAN
  variance so residual differences are attributable to the implementation.
- Compare **percentiles**, not means — latency is heavy-tailed.
- Tolerances are per-phase (DNS on loopback is ~0; TLS handshake dominates).
- The harness only needs the JSON contract + the `Networker.Contracts` parser,
  so it lives naturally in the C# side.

---

## What is built vs stubbed (this branch)

**Proof-of-seam (real, buildable):**
- `schema_version` added to the Rust `TestRun` output (additive, serde-defaulted).
- `Networker.Contracts` — C# records mirroring the JSON, System.Text.Json
  source-gen.
- `ProbeRunner` — shells out to `networker-tester --json-stdout` via
  `System.Diagnostics.Process`, captures stdout, deserializes into
  `ProbeRunResult`.
- `AgentWorker` (`BackgroundService`) — runs one probe on startup, logs
  `schema_version` + phase timings.

**Stubbed (clearly marked TODO):**
- `IDashboardClient` / `NoOpDashboardClient` — logs instead of talking to a
  control plane. This is exactly where the Phase 2 SignalR hub client slots in.

---

## Roadmap (Phase 2 / 3 — not implemented)

- **Phase 2 — Control plane (ASP.NET Core + EF Core + SignalR):**
  REST API for test configs / runs, JWT auth, PostgreSQL via EF Core, and a
  SignalR hub the agent connects to (replacing `NoOpDashboardClient`) to stream
  results live and answer heartbeats.
- **Phase 3 — Endpoint + cutover:** reimplement `networker-endpoint`
  (health / download / upload / page-load routes) in ASP.NET Core; run
  Rust and C# control planes side by side against the same agents; migrate
  data; retire the Rust dashboard/agent/endpoint crates. The Rust
  `networker-tester` probe core remains.

---

## Build & run

### Rust probe core (contract producer)

```bash
# from repo root
cargo build -p networker-tester
cargo test  -p networker-tester --test json_contract   # freezes the contract

# see the actual JSON the C# side consumes
cargo run -p networker-tester -- --target https://www.cloudflare.com \
  --modes http1 --runs 1 --json-stdout
```

### C# app layer (contract consumer)

```bash
# from repo root
dotnet build hybrid/Networker.sln

# run the agent skeleton (one-shot probe on startup)
dotnet run --project hybrid/Networker.Agent
```

The agent looks for the tester on `PATH`. Point it at a local build instead:

```bash
# macOS/Linux
AGENT_TESTERPATH="$(pwd)/target/debug/networker-tester" \
AGENT_TARGET="https://www.cloudflare.com" \
  dotnet run --project hybrid/Networker.Agent
```

Configuration (env var → `Agent` option): `AGENT_TESTERPATH`, `AGENT_TARGET`,
`AGENT_MODES`, `AGENT_TIMEOUTSECONDS`. Defaults live in
`hybrid/Networker.Agent/appsettings.json`.
