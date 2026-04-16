-- seed-dev.sql — Idempotent mock data for local development (v0.28.0+ schema).
--
-- Prerequisites: dashboard binary has run at least once (creates tables via migrations).
-- Usage:
--   PGPASSWORD=networker psql -h localhost -U networker -d networker_core -f scripts/seed-dev.sql
--
-- Safe to re-run — all INSERTs use ON CONFLICT DO NOTHING.

-- ────────────────────────────────────────────────────────────────────────────
-- 1. Dev project (uses the well-known "Default" project from V011 migration)
-- ────────────────────────────────────────────────────────────────────────────

-- Project already exists from migration V011 with id 00000000-...-000000000001.
-- Ensure the admin user is a member (dashboard startup creates the user, but
-- project_member may not be seeded if the admin was created after V011 ran).
INSERT INTO project_member (project_id, user_id, role)
SELECT '00000000-0000-0000-0000-000000000001', user_id, 'admin'
FROM dash_user WHERE role = 'admin' OR is_platform_admin = TRUE
ON CONFLICT DO NOTHING;

-- ────────────────────────────────────────────────────────────────────────────
-- 2. Mock agents (testers) — 3 across 2 regions
-- ────────────────────────────────────────────────────────────────────────────

INSERT INTO agent (agent_id, name, region, provider, status, version, os, arch, api_key, project_id)
VALUES
  ('a0000001-0000-0000-0000-000000000001', 'tester-us-east-1-dev01', 'us-east-1', 'aws', 'online', '0.28.0', 'ubuntu-22.04', 'x86_64', 'dev-key-agent-01', '00000000-0000-0000-0000-000000000001'),
  ('a0000001-0000-0000-0000-000000000002', 'tester-us-east-1-dev02', 'us-east-1', 'aws', 'online', '0.28.0', 'ubuntu-22.04', 'x86_64', 'dev-key-agent-02', '00000000-0000-0000-0000-000000000001'),
  ('a0000001-0000-0000-0000-000000000003', 'tester-westeurope-dev03', 'westeurope', 'azure', 'offline', '0.27.25', 'ubuntu-22.04', 'x86_64', 'dev-key-agent-03', '00000000-0000-0000-0000-000000000001')
ON CONFLICT DO NOTHING;

-- Link testers to project
INSERT INTO project_tester (project_id, agent_id)
VALUES
  ('00000000-0000-0000-0000-000000000001', 'a0000001-0000-0000-0000-000000000001'),
  ('00000000-0000-0000-0000-000000000001', 'a0000001-0000-0000-0000-000000000002'),
  ('00000000-0000-0000-0000-000000000001', 'a0000001-0000-0000-0000-000000000003')
ON CONFLICT DO NOTHING;

-- ────────────────────────────────────────────────────────────────────────────
-- 3. Test configs — one per endpoint kind
-- ────────────────────────────────────────────────────────────────────────────

-- Simple network test (connectivity check)
INSERT INTO test_config (id, project_id, name, endpoint_kind, endpoint_ref, workload, methodology, max_duration_secs)
VALUES (
  'c0000001-0000-0000-0000-000000000001',
  '00000000-0000-0000-0000-000000000001',
  'Cloudflare connectivity',
  'network',
  '{"kind":"network","host":"www.cloudflare.com"}'::jsonb,
  '{"modes":["dns","tcp","tls","http2"],"runs":5,"concurrency":1,"timeout_ms":5000,"payload_sizes":[],"capture_mode":"headers-only"}'::jsonb,
  NULL,
  120
) ON CONFLICT DO NOTHING;

-- Full-matrix network test with benchmark methodology
INSERT INTO test_config (id, project_id, name, endpoint_kind, endpoint_ref, workload, methodology, max_duration_secs)
VALUES (
  'c0000001-0000-0000-0000-000000000002',
  '00000000-0000-0000-0000-000000000001',
  'Full matrix (benchmark mode)',
  'network',
  '{"kind":"network","host":"nwk-ep-ubuntu-dev.eastus.cloudapp.azure.com"}'::jsonb,
  '{"modes":["tcp","dns","tls","http1","http2","udp","download","upload"],"runs":10,"concurrency":2,"timeout_ms":10000,"payload_sizes":[1024,65536],"capture_mode":"full"}'::jsonb,
  '{"warmup_runs":3,"measured_runs":20,"cooldown_ms":1000,"target_error_pct":5.0,"outlier_policy":{"policy":"iqr","k":1.5},"quality_gates":{"max_cv_pct":15.0,"min_samples":10,"max_noise_level":0.3},"publication_gates":{"max_failure_pct":10.0,"require_all_phases":true}}'::jsonb,
  900
) ON CONFLICT DO NOTHING;

-- Proxy endpoint test
INSERT INTO test_config (id, project_id, name, endpoint_kind, endpoint_ref, workload, max_duration_secs)
VALUES (
  'c0000001-0000-0000-0000-000000000003',
  '00000000-0000-0000-0000-000000000001',
  'nginx proxy API check',
  'proxy',
  '{"kind":"proxy","proxy_endpoint_id":"d0000001-0000-0000-0000-000000000001"}'::jsonb,
  '{"modes":["http1","http2"],"runs":10,"concurrency":4,"timeout_ms":10000,"payload_sizes":[],"capture_mode":"headers-only"}'::jsonb,
  300
) ON CONFLICT DO NOTHING;

-- Runtime test (language comparison)
INSERT INTO test_config (id, project_id, name, endpoint_kind, endpoint_ref, workload, methodology, max_duration_secs)
VALUES (
  'c0000001-0000-0000-0000-000000000004',
  '00000000-0000-0000-0000-000000000001',
  'Rust vs Go vs Node API throughput',
  'runtime',
  '{"kind":"runtime","runtime_id":"r0000001-0000-0000-0000-000000000001","language":"rust"}'::jsonb,
  '{"modes":["http2"],"runs":30,"concurrency":8,"timeout_ms":15000,"payload_sizes":[4096],"capture_mode":"metrics-only"}'::jsonb,
  '{"warmup_runs":5,"measured_runs":50,"cooldown_ms":2000,"target_error_pct":2.0,"outlier_policy":{"policy":"iqr","k":1.5},"quality_gates":{"max_cv_pct":10.0,"min_samples":20,"max_noise_level":0.2},"publication_gates":{"max_failure_pct":5.0,"require_all_phases":true}}'::jsonb,
  1200
) ON CONFLICT DO NOTHING;

-- ────────────────────────────────────────────────────────────────────────────
-- 4. Test runs — various statuses
-- ────────────────────────────────────────────────────────────────────────────

-- Completed simple test
INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, tester_id)
VALUES (
  'r0000001-0000-0000-0000-000000000001',
  'c0000001-0000-0000-0000-000000000001',
  '00000000-0000-0000-0000-000000000001',
  'completed',
  now() - interval '2 hours',
  now() - interval '2 hours' + interval '6 seconds',
  20, 0,
  'a0000001-0000-0000-0000-000000000001'
) ON CONFLICT DO NOTHING;

-- Completed with failures (mixed)
INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, tester_id)
VALUES (
  'r0000001-0000-0000-0000-000000000002',
  'c0000001-0000-0000-0000-000000000001',
  '00000000-0000-0000-0000-000000000001',
  'completed',
  now() - interval '1 hour',
  now() - interval '1 hour' + interval '8 seconds',
  18, 2,
  'a0000001-0000-0000-0000-000000000001'
) ON CONFLICT DO NOTHING;

-- Currently running
INSERT INTO test_run (id, test_config_id, project_id, status, started_at, success_count, failure_count, tester_id, last_heartbeat)
VALUES (
  'r0000001-0000-0000-0000-000000000003',
  'c0000001-0000-0000-0000-000000000002',
  '00000000-0000-0000-0000-000000000001',
  'running',
  now() - interval '40 minutes',
  85, 3,
  'a0000001-0000-0000-0000-000000000002',
  now() - interval '5 seconds'
) ON CONFLICT DO NOTHING;

-- Failed
INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, error_message, tester_id)
VALUES (
  'r0000001-0000-0000-0000-000000000004',
  'c0000001-0000-0000-0000-000000000003',
  '00000000-0000-0000-0000-000000000001',
  'failed',
  now() - interval '45 minutes',
  now() - interval '44 minutes',
  0, 10,
  'Connection refused: nginx proxy not reachable at target host',
  'a0000001-0000-0000-0000-000000000001'
) ON CONFLICT DO NOTHING;

-- Queued
INSERT INTO test_run (id, test_config_id, project_id, status, success_count, failure_count)
VALUES (
  'r0000001-0000-0000-0000-000000000005',
  'c0000001-0000-0000-0000-000000000004',
  '00000000-0000-0000-0000-000000000001',
  'queued',
  0, 0
) ON CONFLICT DO NOTHING;

-- Completed benchmark run (with artifact)
INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, tester_id, artifact_id)
VALUES (
  'r0000001-0000-0000-0000-000000000006',
  'c0000001-0000-0000-0000-000000000002',
  '00000000-0000-0000-0000-000000000001',
  'completed',
  now() - interval '3 hours',
  now() - interval '3 hours' + interval '180 seconds',
  200, 4,
  'a0000001-0000-0000-0000-000000000002',
  'art00001-0000-0000-0000-000000000001'
) ON CONFLICT DO NOTHING;

-- More completed runs for list density
INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, tester_id)
VALUES
  ('r0000001-0000-0000-0000-000000000007', 'c0000001-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000001', 'completed', now() - interval '6 hours', now() - interval '6 hours' + interval '4 seconds', 20, 0, 'a0000001-0000-0000-0000-000000000001'),
  ('r0000001-0000-0000-0000-000000000008', 'c0000001-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000001', 'completed', now() - interval '12 hours', now() - interval '12 hours' + interval '5 seconds', 20, 0, 'a0000001-0000-0000-0000-000000000002'),
  ('r0000001-0000-0000-0000-000000000009', 'c0000001-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000001', 'cancelled', now() - interval '18 hours', now() - interval '18 hours' + interval '2 seconds', 5, 0, 'a0000001-0000-0000-0000-000000000001'),
  ('r0000001-0000-0000-0000-00000000000a', 'c0000001-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000001', 'completed', now() - interval '24 hours', now() - interval '24 hours' + interval '120 seconds', 180, 8, 'a0000001-0000-0000-0000-000000000002')
ON CONFLICT DO NOTHING;

-- ────────────────────────────────────────────────────────────────────────────
-- 5. Benchmark artifact (for the benchmark-mode run)
-- ────────────────────────────────────────────────────────────────────────────

INSERT INTO benchmark_artifact (id, test_run_id, environment, methodology, launches, cases, samples, summaries, data_quality)
VALUES (
  'art00001-0000-0000-0000-000000000001',
  'r0000001-0000-0000-0000-000000000006',
  '{"os":"ubuntu-22.04","arch":"x86_64","cpu":"Intel Xeon 8375C","memory_gb":16,"region":"us-east-1","provider":"aws"}'::jsonb,
  '{"warmup_runs":3,"measured_runs":20,"cooldown_ms":1000,"target_error_pct":5.0,"outlier_policy":{"policy":"iqr","k":1.5},"quality_gates":{"max_cv_pct":15.0,"min_samples":10,"max_noise_level":0.3},"publication_gates":{"max_failure_pct":10.0,"require_all_phases":true}}'::jsonb,
  '[{"tester":"tester-us-east-1-dev01","started_at":"2026-04-15T12:00:00Z","finished_at":"2026-04-15T12:03:00Z"}]'::jsonb,
  '[{"mode":"tcp","p5":12.1,"p25":14.3,"p50":16.8,"p75":19.2,"p95":24.1,"p99":28.7,"p999":35.4,"mean":17.1,"stddev":4.2,"cv":0.245,"count":20},{"mode":"dns","p5":8.2,"p25":10.1,"p50":12.4,"p75":14.8,"p95":18.9,"p99":22.3,"p999":26.1,"mean":12.6,"stddev":3.1,"cv":0.246,"count":20},{"mode":"tls","p5":42.3,"p25":48.7,"p50":55.2,"p75":62.1,"p95":78.4,"p99":92.1,"p999":105.3,"mean":56.8,"stddev":12.4,"cv":0.218,"count":20},{"mode":"http2","p5":68.4,"p25":78.2,"p50":88.9,"p75":102.3,"p95":128.7,"p99":148.2,"p999":172.1,"mean":91.4,"stddev":21.3,"cv":0.233,"count":20}]'::jsonb,
  NULL,
  '{"overall_p50":43.3,"overall_p95":62.5,"overall_p99":71.2,"total_samples":80,"total_outliers":2,"modes_tested":4}'::jsonb,
  '{"noise_level":0.12,"sufficiency":"adequate","publication_ready":true,"publication_blocker_count":0}'::jsonb
) ON CONFLICT DO NOTHING;

-- ────────────────────────────────────────────────────────────────────────────
-- 6. Schedules
-- ────────────────────────────────────────────────────────────────────────────

INSERT INTO test_schedule (id, test_config_id, project_id, cron_expr, timezone, enabled, next_fire_at)
VALUES
  ('s0000001-0000-0000-0000-000000000001', 'c0000001-0000-0000-0000-000000000001', '00000000-0000-0000-0000-000000000001', '*/5 * * * *', 'UTC', TRUE, now() + interval '5 minutes'),
  ('s0000001-0000-0000-0000-000000000002', 'c0000001-0000-0000-0000-000000000002', '00000000-0000-0000-0000-000000000001', '0 3 * * *', 'America/New_York', TRUE, (CURRENT_DATE + 1) + interval '7 hours')
ON CONFLICT DO NOTHING;

-- ────────────────────────────────────────────────────────────────────────────
-- Done. Verify:
-- ────────────────────────────────────────────────────────────────────────────
DO $$
DECLARE
  tc INT; tr INT; ts INT; ba INT;
BEGIN
  SELECT count(*) INTO tc FROM test_config;
  SELECT count(*) INTO tr FROM test_run;
  SELECT count(*) INTO ts FROM test_schedule;
  SELECT count(*) INTO ba FROM benchmark_artifact;
  RAISE NOTICE 'Seed complete: % test_configs, % test_runs, % schedules, % artifacts', tc, tr, ts, ba;
END $$;
