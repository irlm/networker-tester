-- V046: test_run.client_envelope — the tester's run-envelope pass-through.
--
-- The Rust tester's final TestRun JSON carries run-scoped context fields
-- (client_network, client_geo, target_geo, client_load_before/after,
-- clock_sync, client_info, server_info) that previously died inside the agent:
-- RunExecutor parsed the full TestRun but only streamed attempts + a bare
-- run_finished, so no control-plane API could serve them. The agent now
-- extracts exactly those envelope fields into a compact JSON object and sends
-- it on run_finished; AgentMessageProcessor.OnRunFinished re-filters it
-- through the same allowlist and persists it here.
--
-- Nullable, no default: old agents simply don't send an envelope and the
-- column stays NULL (the API omits it, so old runs' wire shape is unchanged);
-- a NULL is "tester predates the envelope or ran with collection disabled",
-- never fabricated. Fully idempotent (ADD COLUMN IF NOT EXISTS) so re-running
-- the chain no-ops.
ALTER TABLE test_run
    ADD COLUMN IF NOT EXISTS client_envelope JSONB;
