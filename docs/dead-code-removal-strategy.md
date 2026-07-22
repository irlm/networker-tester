# Dead-code removal strategy

The repeatable protocol for deleting code safely. Written for the 2026-07 sweep;
applies to every future removal. The principle mirrors the testing strategy
(`docs/analysis/coverage-strategy-2026-07.md`): **prove before you act** — a
deletion is a claim ("nothing uses this") and every claim needs evidence that
could have falsified it.

## The triple-check (all three, per candidate, before deletion)

**Check 1 — Static proof (find every reference).**
Grep the WHOLE repo for the symbol/file/route/env-var name — including docs,
workflows, installers, SQL, and the OTHER language stacks (a Rust symbol may be
referenced by a C# port comment or a wire contract; an env var by `install.sh`
or the VM's service unit). Exclude only build output (`/target/`, `/bin/`,
`/obj/`, `node_modules`). Record the exact pattern used. Beware the invisible
reference classes — each has bitten this repo or is idiomatic here:
- **Serde / System.Text.Json wire names** — fields read by serialization, not
  by code. A "0-reference" DTO field may be a versioned JSON contract.
- **ASP.NET minimal-API handler parameters** — a type used only as a lambda
  parameter IS live (DI/model-binding resolves it by type).
- **EF entities/DbSets** — mapped to schema, not called.
- **String-addressed things** — router paths, `lazy()` imports, env-var names,
  systemd unit names, nginx locations, cron workflow names.
- **Cross-stack twins** — `shared/modes.json`, the JSON contract, the SDK
  contract: "unused here" may be "the guard for drift over there".

**Check 2 — Mechanical proof (build + full test suite, per batch).**
After removing, run everything runnable locally: `cargo build --workspace` +
`cargo clippy --all-targets -- -D warnings` + `cargo test --workspace --lib`
(Rust); `npx tsc --noEmit` + `vitest run` + eslint on touched files (frontend);
`python -m unittest` (Python SDK). C# cannot build locally (net10 vs local
net8): C# removals get their OWN batch/PR so the CI `Build & audit (C#)`
verdict maps 1:1 to the change, and are the batch most likely to need a CI
round-trip. A batch is one PR; a failed check reverts the one batch, not a
mixed bag.

**Check 3 — Semantic proof (why does this exist, and why is it safe NOW?).**
`git log --follow` the file: who added it, for what, and did the reason expire?
Three traps this check catches that 1+2 cannot:
- **Deliberate keeps** — code kept intentionally (SignalR `/hub/*` "for future
  native clients", the http3/pageload3 stubs, `BuildUri` as the legacy `?key=`
  reference). Removal is an owner DECISION, not a cleanup — surface it, don't
  delete it.
- **Not-dead-yet** — things that die only when a gate passes (anything
  referenced by the retired crates dies when #518 merges; the `?key=` fallback
  dies at the decommission). Park in a dated "dead-after-<gate>" list; delete
  after the gate, in the gate's PR.
- **Dead-but-load-bearing history** — shipped migrations, frozen contracts,
  `docs/analysis/` reports: append-only by policy, never "cleaned up".

## Process rules

1. **Small batches, one concern per PR.** Same-stack, same-risk-class items
   batch together; anything MEDIUM/LOW confidence ships alone.
2. **Confidence gates the action.** HIGH (provably unreferenced) → delete.
   MEDIUM (referenced by tests only / deliberate-keep / dies-at-a-gate) →
   surface for decision or park. LOW (possible reflection/string/serde use) →
   do NOT delete; add a test or tracer first, or leave it.
3. **Removal-only diffs.** Never mix dead-code removal with behavior change;
   the diff should be entirely deletions (plus the version bump when a
   shipping artifact is touched).
4. **The reaper rule:** if unsure whether it's dead, it isn't. The cost of
   carrying a dead function is trivial; the cost of deleting a live one is an
   incident. Asymmetry decides ties.
5. **Verify-first applies to surveys too.** Survey findings are CANDIDATES.
   This session's coverage surveys overstated gaps 6 times; expect dead-code
   surveys to do the same. Re-prove every candidate yourself before deleting.

## Standing exclusions (never report, never delete)

- Shipped migration scripts (`V0NN_*.sql` — frozen by SHA test), the
  `_migrations` chain, `schema.sql`.
- `docs/analysis/*` (dated reports), `CHANGELOG.md` history.
- The versioned wire contracts (tester JSON contract, SDK contract-v1,
  `shared/modes.json`) and their mirrored types.
- The intentional feature stubs (`--no-default-features` http3/pageload3).
- `legacy/rust` branch / `rust-legacy-*` tag contents (history, not tree).
