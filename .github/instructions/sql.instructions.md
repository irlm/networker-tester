---
applyTo: "**/*.sql,**/db/*.rs,**/sql.rs,**/mssql.rs,**/postgres.rs"
---

# SQL & Database Rules

## Parameterized Queries (Critical)
- tiberius (SQL Server): Use `Query::new("... @P1, @P2 ...")` with `.bind()` — never format!() values into SQL.
- tokio-postgres: Use `client.execute("... $1, $2 ...", &[&val1, &val2])` — never interpolate.
- Rationale: This codebase accepts user-supplied target URLs and config values that flow into test metadata stored in the database.

## Schema Migration Pattern
- SQL Server migrations: numbered files in `sql/` (01_CreateDatabase.sql through 07_MoreTcpStats.sql).
- PostgreSQL migrations: `sql/postgres/01_CreateSchema.sql`.
- New columns or tables must be added to BOTH backends. The `DatabaseBackend` trait requires parity.
- Migration files are additive — never modify existing migration files, create a new numbered one.
- Always use `IF NOT EXISTS` or equivalent idempotent guards for new tables/columns.

## Data Types
- UUIDs: `UNIQUEIDENTIFIER` (SQL Server) / `UUID` (PostgreSQL). Generated in Rust as `Uuid::new_v4()`.
- Timestamps: `DATETIMEOFFSET` (SQL Server) / `TIMESTAMPTZ` (PostgreSQL). Always UTC via `chrono::Utc::now()`.
- Durations: Store as `FLOAT` milliseconds (e.g., `connect_duration_ms`, `ttfb_ms`).
- IP addresses: `NVARCHAR(45)` / `TEXT` — must fit IPv6 mapped addresses.
- Throughput: `FLOAT` as Mbps (`throughput_mbps`, `goodput_mbps`).

## DatabaseBackend Trait
- All methods are `async` via `#[async_trait]`.
- `connect()` → establish connection (tiberius: TCP + TLS handshake; tokio-postgres: spawn connection task).
- `migrate()` → run schema migrations idempotently.
- `save()` → persist a full `TestRun` with all child records.
- `ping()` → verify connection is alive.
- New trait methods must have implementations in: `mssql.rs`, `postgres.rs`, and `test_fixtures.rs` (in-memory mock).

## Connection Handling
- SQL Server: `Config::from_ado_string()` + `TcpStream::connect()` + `compat_write()` for tiberius.
- PostgreSQL: `tokio_postgres::connect(url, NoTls)` with spawned connection task.
- Connection strings come from environment variables only — never hardcode or log them.
- On connection failure, return `anyhow::Error` with context — do not panic.

## Testing
- SQL integration tests require Docker services (see `docker-compose.db.yml`).
- Tests are `#[ignore]` by default; CI runs them with `--include-ignored` when `NETWORKER_SQL_TESTS=true`.
- Test fixtures use `test_fixtures::InMemoryBackend` for unit tests that don't need a real database.
