# Database schema ownership

**The control-plane PostgreSQL schema is owned by `src/Networker.Data` — not by
the Rust dashboard.** This removes the last structural dependency on
`crates/networker-dashboard` and unblocks deleting the Rust control-plane crates
when the decommission soak completes.

**Status (2026-07-22): COMPLETE and decommission-ready.** The transfer is
finished and re-verified end to end:

- **Every table** the Rust `migrations.rs` creates is present in the C# scripts —
  a `CREATE TABLE` set comparison shows the 42 Rust-created tables are a subset
  of the 46 C# ones (the extra four are the C#-era `alert_channel`/`alert_rule`/
  `alert_event`). Nothing is orphaned by deleting the crate.
- The **only** reference to `crates/networker-dashboard/src/db/migrations.rs`
  anywhere on the current stack is a historical comment in `SchemaMigrationTests`.
- The C# control plane **serves production** and **runs `SchemaMigrator` at its
  own startup** (`Program.cs`, gated on `NETWORKER_RUN_MIGRATIONS != 0`); the
  retired Rust dashboard no longer boots in prod. The C# migrator is now the sole
  runtime schema authority.

Chain latest: **V045** (`SchemaMigrator.LatestVersion`).

## Where the schema lives

| Piece | Location |
|---|---|
| Ordered migration scripts (V002…V045) | `src/Networker.Data/Migrations/V0NN_*.sql` (embedded resources) |
| V025 (UUID → base36 project ids) | `src/Networker.Data/Migrations/V025ProjectIdMigration.cs` (code, like the Rust original) |
| ProjectId base36 + Damm implementation | `src/Networker.Data/Migrations/ProjectId36.cs` |
| Runner | `src/Networker.Data/Migrations/SchemaMigrator.cs` |
| Cumulative snapshot (reference only, currently a V041-era dump — see note) | `src/Networker.Data/Migrations/schema.sql` |
| Legacy source (frozen, UNREFERENCED — deletable) | `crates/networker-dashboard/src/db/migrations.rs` |

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

- applies the full chain to a fresh database and asserts versions 2…45 land
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

The chain was additionally validated by replaying all migrations against
a live PostgreSQL 16 container and diffing the resulting table/column sets
against the EF model (`schema.sql` is the `pg_dump --schema-only` of that
replay).

**Note — `schema.sql` lags at V041.** It has not been regenerated since V042
(the reference-only snapshot; the source of truth is the ordered `V0NN` scripts,
which are current, frozen, and CI-tested). Regenerating it requires the C#
migrator (V025 is a code migration — base36/Damm can't run in raw SQL), so it is
a low-priority refresh, not a correctness gap: the V042–V045 deltas are small and
documented under "Out-of-band DDL" below, and every mapped table is proven
against the live-applied chain by `SchemaMigrationTests`. Refresh it (fresh PG16
→ `SchemaMigrator.MigrateAsync` → `pg_dump`) next time a migration is added.

## How to add a migration (post-decommission workflow)

1. Create `src/Networker.Data/Migrations/V046_short_name.sql` (next free
   number, zero-padded, one underscore after the version). Make it
   idempotent where cheap (`IF NOT EXISTS` guards) — the runner's
   transaction makes idempotence optional, but it keeps manual recovery
   easy.
2. Bump `SchemaMigrator.LatestVersion` to `46`.
3. Pin the script's SHA-256 in `MigrationScriptFreezeTests.FrozenSha256`.
4. Update the EF model (`NetworkerDbContext` + entity) to match, if the
   change touches mapped tables. The equivalence test fails if they drift.
5. Regenerate `Migrations/schema.sql` (fresh Postgres 16 → run the chain →
   `pg_dump --schema-only --no-owner --no-privileges`).
6. **Never edit a shipped `V0NN` script** — databases that already ran it
   would silently diverge from fresh installs. The freeze test enforces
   this.

## Invoking the migrator

**DONE — the control plane runs the migrator at startup.** `Program.cs` calls
`SchemaMigrator.MigrateAsync(connString)` during boot, gated on
`NETWORKER_RUN_MIGRATIONS != "0"`, before the app serves traffic; on failure the
app refuses to start and the deploy's readiness check rolls back. On an
already-migrated database this is a no-op (the `_migrations` bookkeeping rows
short-circuit). The C# control plane boots first in production (it *is*
production — the Rust dashboard is retired and no longer runs), so it owns
applying the chain.

```csharp
using Networker.Data.Migrations;

var result = await SchemaMigrator.MigrateAsync(connectionString);
// result.Applied      → versions applied by this call
// result.WasUpToDate  → true on an already-migrated database
```

Test hosts that materialize the schema themselves opt out with
`NETWORKER_RUN_MIGRATIONS=0` (the integration fixture renders the EF model's own
DDL instead of replaying the chain).

## Out-of-band DDL to be aware of

- `crates/networker-dashboard/src/db/perf_log.rs::ensure_schema` creates
  `perf_log` in a **split logs database** (no `dash_user` FK there). On the
  core database V023 owns `perf_log`; if a split logs DB is still configured
  after decommission, its bootstrap needs a home in the C# stack.
  **This bit prod (v0.28.39):** the Rust runner recorded V023 in the MAIN
  database's `_migrations` while the table only ever existed in the logs DB,
  so the C# migrator skipped it and `GET /api/perf-log` 500'd with 42P01.
  **V042** re-asserts the exact V023 DDL idempotently on the main database —
  a no-op on fresh installs, the fix on Rust-era databases. If a future
  migration ever depended on a table the Rust runner created out-of-band,
  the same recorded-but-absent pattern applies: ship a new idempotent
  re-assert migration, never edit the original.
- **V043** adds `test_config.token_enc` / `token_nonce` (both `BYTEA`,
  nullable) — the encrypted LagHound SDK-endpoint probe token for `sdkprobe`
  configs. Encrypted with `Networker.Security.CredentialCipher` (the same
  AES-256-GCM scheme as `cloud_account.credentials_enc`); never returned to a
  client. Only populated for `sdkprobe` endpoints; NULL everywhere else.
- **V044** adds `agent.api_key_expires_at` / `api_key_last_used_at` (both
  `TIMESTAMPTZ`) and `api_key_last_used_ip` (`VARCHAR(64)`), all nullable — the
  agent api-key hardening wave. `api_key_expires_at` non-null + in the past →
  agent auth rejects the key (NULL = no expiry, back-compat for the whole
  fleet); `api_key_last_used_at` / `_ip` are the write-throttled "last seen"
  audit stamps. The rotate endpoint replaces `api_key` + `api_key_hash` and
  resets the expiry.
- **V045** drops the plaintext `agent.api_key` column (auth has been hash-only —
  `api_key_hash` — since V040; the plaintext column was write-only dead weight).
  Dropping the column also drops its `agent_api_key_key` UNIQUE constraint via
  the column dependency (an explicit `DROP INDEX` would fail — it is a constraint,
  not a standalone index).
- `bootstrap/reset-pre-prod.sql` and the tester's V001 schema are separate,
  unaffected artifacts.

## Decommission impact

With this in place, deleting `crates/networker-dashboard` (and the other
retired Rust control-plane crates) no longer orphans the schema: fresh
installs, upgrades, and the migration history all live in
`src/Networker.Data`, exercised by CI on every PR.
