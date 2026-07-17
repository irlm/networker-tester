# Database schema ownership

**As of this document, the control-plane PostgreSQL schema is owned by
`src/Networker.Data` — not by the Rust dashboard.** This removes the last
structural dependency on `crates/networker-dashboard` and unblocks deleting
the Rust control-plane crates when the decommission soak completes.

## Where the schema lives

| Piece | Location |
|---|---|
| Ordered migration scripts (V002…V039) | `src/Networker.Data/Migrations/V0NN_*.sql` (embedded resources) |
| V025 (UUID → base36 project ids) | `src/Networker.Data/Migrations/V025ProjectIdMigration.cs` (code, like the Rust original) |
| ProjectId base36 + Damm implementation | `src/Networker.Data/Migrations/ProjectId36.cs` |
| Runner | `src/Networker.Data/Migrations/SchemaMigrator.cs` |
| Cumulative snapshot (reference only) | `src/Networker.Data/Migrations/schema.sql` |
| Legacy source (frozen, to be deleted) | `crates/networker-dashboard/src/db/migrations.rs` |

The scripts are byte-for-byte extractions of the SQL constants in
`migrations.rs` (generated mechanically, not transcribed). Four migrations
had their SQL inline in the Rust `run()` function (V015, V019, V020, V021);
those bodies were copied out, with V020's Rust-side "already renamed?"
pre-check re-expressed as an equivalent SQL `DO` guard. V025 was Rust code
(base36 + Damm check digits can't be computed in SQL) and is a step-for-step
C# port — same temporary columns, same FK lists, same constraint and index
names, ids generated from the same inputs
(`zone="us"`, `server_id="a20"`, project `created_at`).

**V001 is not here on purpose.** The probe-result schema (`TestRun`,
`RequestAttempt`, `DnsResult`, …) is created and owned by the
`networker-tester` crate, which survives the decommission. The dashboard
migrations only ever touched those tables behind
`IF EXISTS (… 'testrun' …)` guards, and the ported scripts keep those guards.

## Bookkeeping-table compatibility guarantee

`SchemaMigrator` uses **the same bookkeeping table the Rust runner used**:

```sql
CREATE TABLE IF NOT EXISTS _migrations (
    version     INT          NOT NULL PRIMARY KEY,
    applied_at  TIMESTAMPTZ  NOT NULL DEFAULT now()
);
```

One row per applied version, versions applied in ascending order, each
recorded with `INSERT … ON CONFLICT DO NOTHING` — identical semantics to
`crates/networker-dashboard/src/db/migrations.rs::run()`. Consequences:

- **Existing production database** (already migrated by the Rust dashboard):
  every version 2…39 is present in `_migrations`, so `MigrateAsync` reports
  `WasUpToDate == true` and executes nothing. Cutover is a no-op.
- **Fresh database**: the full chain replays in the same order the Rust
  runner used, producing an identical schema (verified — see below).
- The two runners can even coexist during the soak: whichever runs first
  records the version; the other skips it.

Differences from the Rust runner (both strictly safer, neither observable in
a healthy deployment):

1. Each SQL migration and its bookkeeping row commit in **one transaction**
   (the Rust runner had a small crash window between script and record).
   V008 drives its own `BEGIN/COMMIT` and V025 runs statement-by-statement,
   exactly like the original.
2. A session advisory lock serializes concurrent migrator instances.

## Equivalence proof

`tests/Networker.Tests/SchemaMigrationTests.cs` (runs in CI's dotnet
workflow, Testcontainers + `postgres:16-alpine`):

- applies the full chain to a fresh database and asserts versions 2…39 land
  in `_migrations`;
- re-runs the migrator and asserts **zero pending** (the prod-cutover case);
- queries **every** EF-mapped entity (30 DbSets) against the migrated
  schema — the EF model was reverse-engineered from the real production
  database that the Rust runner built, so "every mapped column selects
  cleanly" is the strongest available fidelity check;
- asserts the migrated data matches the Rust output (Default project with a
  valid Damm-checked base36 id, `project_routing` seeded to `us/us`, 31
  sovereignty zones, 17 cost rates);
- write round-trips `dash_user → project → test_config → test_run` to
  exercise defaults, CHECKs, and the FK graph.

`MigrationScriptFreezeTests` needs no Docker and pins the SHA-256 of every
shipped script, plus reference vectors proving the C# Damm/base36 port
matches `project_id.rs`.

The chain was additionally validated by replaying all 38 migrations against
a live PostgreSQL 16 container and diffing the resulting table/column sets
against the EF model (`schema.sql` is the `pg_dump --schema-only` of that
replay).

## How to add a migration (post-decommission workflow)

1. Create `src/Networker.Data/Migrations/V040_short_name.sql` (next free
   number, zero-padded, one underscore after the version). Make it
   idempotent where cheap (`IF NOT EXISTS` guards) — the runner's
   transaction makes idempotence optional, but it keeps manual recovery
   easy.
2. Bump `SchemaMigrator.LatestVersion` to `40`.
3. Pin the script's SHA-256 in `MigrationScriptFreezeTests.FrozenSha256`.
4. Update the EF model (`NetworkerDbContext` + entity) to match, if the
   change touches mapped tables. The equivalence test fails if they drift.
5. Regenerate `Migrations/schema.sql` (fresh Postgres 16 → run the chain →
   `pg_dump --schema-only --no-owner --no-privileges`).
6. **Never edit a shipped `V0NN` script** — databases that already ran it
   would silently diverge from fresh installs. The freeze test enforces
   this.

## Invoking the migrator

`Networker.Data` is a library; nothing runs migrations automatically yet.

```csharp
using Networker.Data.Migrations;

var result = await SchemaMigrator.MigrateAsync(connectionString);
// result.Applied      → versions applied by this call
// result.WasUpToDate  → true on an already-migrated database
```

**Follow-up (deliberately out of scope here):** wire this into
`Networker.ControlPlane` startup (env-gated, e.g.
`NETWORKER_RUN_MIGRATIONS=1`) once the Rust dashboard stops being the
process that boots first. Until then the Rust dashboard still runs its own
copy of the same chain at startup, which is harmless — same scripts, same
bookkeeping.

## Out-of-band DDL to be aware of

- `crates/networker-dashboard/src/db/perf_log.rs::ensure_schema` creates
  `perf_log` in a **split logs database** (no `dash_user` FK there). On the
  core database V023 owns `perf_log`; if a split logs DB is still configured
  after decommission, its bootstrap needs a home in the C# stack.
- `bootstrap/reset-pre-prod.sql` and the tester's V001 schema are separate,
  unaffected artifacts.

## Decommission impact

With this in place, deleting `crates/networker-dashboard` (and the other
retired Rust control-plane crates) no longer orphans the schema: fresh
installs, upgrades, and the migration history all live in
`src/Networker.Data`, exercised by CI on every PR.
