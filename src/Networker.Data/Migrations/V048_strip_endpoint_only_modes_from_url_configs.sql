-- V048: strip endpoint-only modes from plain-URL diagnostic configs.
--
-- udp (echo to :9999) and pageload/pageload2/pageload3 (the endpoint's
-- synthetic /asset?id=N&bytes=M ladder) can only ever fail against an
-- arbitrary URL — nothing there speaks the echo protocol and the asset
-- route 404s (user-caught 2026-08-12: every "Full" URL diagnostic carried
-- 4 guaranteed-failed attempts). Their `requires` in shared/modes.json is
-- now "networker-endpoint" and the create path 422s the combination, but
-- URL-diagnostic configs are REUSED by name across launches (and referenced
-- by schedules), so existing rows would keep producing the failures forever
-- without this cleanup.
--
-- Scope: endpoint kind 'network' only (raw URLs). Provisioned endpoint /
-- proxy targets legitimately run these modes and are untouched. Idempotent:
-- the WHERE clause matches only rows that still carry one of the four modes.
UPDATE test_config
SET workload = jsonb_set(
        workload,
        '{modes}',
        COALESCE(
            (SELECT jsonb_agg(m)
             FROM jsonb_array_elements_text(workload->'modes') AS m
             WHERE m NOT IN ('udp', 'pageload', 'pageload2', 'pageload3')),
            '[]'::jsonb),
        false),
    updated_at = now()
WHERE endpoint_ref->>'kind' = 'network'
  AND jsonb_typeof(workload->'modes') = 'array'
  AND EXISTS (
      SELECT 1
      FROM jsonb_array_elements_text(workload->'modes') AS m
      WHERE m IN ('udp', 'pageload', 'pageload2', 'pageload3'));
