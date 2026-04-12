-- WARNING: DESTRUCTIVE -- wipes ALL data in ALL dashboard tables.
-- This is a one-time pre-production reset, NOT a regular migration.
--
-- Table list is synchronised with CREATE TABLE statements in
-- crates/networker-dashboard/src/db/migrations.rs (through V027).
-- The _migrations bookkeeping table is intentionally excluded so the
-- schema version tracking is preserved.
--
-- Note: the binary `reset_pre_prod` wraps this file inside an explicit
-- transaction (so partial TRUNCATE failures leave the DB untouched) and
-- runs `VACUUM FULL ANALYZE` separately afterwards, since VACUUM cannot
-- execute inside a transaction block.

-- TRUNCATE every application table in one statement; CASCADE handles FK order.
TRUNCATE TABLE
    project_tester,
    system_health,
    migration_audit_log,
    migration_request,
    project_routing,
    server_registry,
    sovereignty_zone,
    perf_log,
    benchmark_request_progress,
    benchmark_regression,
    benchmark_cell,
    benchmark_config,
    benchmark_vm_catalog,
    benchmark_result,
    benchmark_run,
    benchmark_compare_preset,
    workspace_warning,
    workspace_invite,
    test_visibility_rule,
    command_approval,
    share_link,
    cloud_account,
    project_member,
    project,
    cloud_connection,
    deployment,
    schedule,
    job,
    test_definition,
    agent,
    dash_user
RESTART IDENTITY CASCADE;
