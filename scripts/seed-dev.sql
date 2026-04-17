-- seed-dev.sql — Idempotent mock data for local development (v0.28.0+ schema).
--
-- Run AFTER the dashboard binary has started and applied migrations:
--   docker exec -i networker-tester-postgres-1 psql -U networker -d networker_core < scripts/seed-dev.sql
--
-- Safe to re-run — all INSERTs use ON CONFLICT DO NOTHING.

DO $$
DECLARE
  pid TEXT;
  uid UUID;
BEGIN
  -- Discover the first available project (created by V011 migration or setup)
  SELECT project_id INTO pid FROM project LIMIT 1;
  IF pid IS NULL THEN
    RAISE EXCEPTION 'No project found. Run the dashboard binary first to create one.';
  END IF;

  -- Discover admin user
  SELECT user_id INTO uid FROM dash_user WHERE role = 'admin' OR is_platform_admin = TRUE LIMIT 1;

  RAISE NOTICE 'Seeding into project % with admin user %', pid, uid;

  -- ── Persistent testers (project_tester) ─────────────────────────────
  INSERT INTO project_tester (tester_id, project_id, name, cloud, region, vm_size, power_state, allocation, created_by)
  VALUES
    ('a0000001-aaaa-4000-8000-000000000001', pid, 'tester-us-east-1-dev01', 'aws', 'us-east-1', 'Standard_D2s_v3', 'running', 'idle', uid),
    ('a0000001-aaaa-4000-8000-000000000002', pid, 'tester-us-east-1-dev02', 'aws', 'us-east-1', 'Standard_D2s_v3', 'running', 'idle', uid),
    ('a0000001-aaaa-4000-8000-000000000003', pid, 'tester-westeurope-dev03', 'azure', 'westeurope', 'Standard_B2s', 'stopped', 'idle', uid)
  ON CONFLICT DO NOTHING;

  -- WS-connected agents (linked to persistent testers)
  INSERT INTO agent (agent_id, name, region, provider, status, version, os, arch, api_key, project_id, tester_id)
  VALUES
    ('b0000001-bbbb-4000-8000-000000000001', 'tester-us-east-1-dev01', 'us-east-1', 'aws', 'online', '0.28.0', 'ubuntu-22.04', 'x86_64', 'dev-key-agent-01', pid, 'a0000001-aaaa-4000-8000-000000000001'),
    ('b0000001-bbbb-4000-8000-000000000002', 'tester-us-east-1-dev02', 'us-east-1', 'aws', 'online', '0.28.0', 'ubuntu-22.04', 'x86_64', 'dev-key-agent-02', pid, 'a0000001-aaaa-4000-8000-000000000002'),
    ('b0000001-bbbb-4000-8000-000000000003', 'tester-westeurope-dev03', 'westeurope', 'azure', 'offline', '0.27.25', 'ubuntu-22.04', 'x86_64', 'dev-key-agent-03', pid, 'a0000001-aaaa-4000-8000-000000000003')
  ON CONFLICT DO NOTHING;

  -- ── Test configs ───────────────────────────────────────────────────

  -- Simple network test
  INSERT INTO test_config (id, project_id, name, endpoint_kind, endpoint_ref, workload, methodology, max_duration_secs, created_by)
  VALUES (
    'c0000001-cccc-4000-8000-000000000001', pid, 'Cloudflare connectivity', 'network',
    '{"kind":"network","host":"www.cloudflare.com"}'::jsonb,
    '{"modes":["dns","tcp","tls","http2"],"runs":5,"concurrency":1,"timeout_ms":5000,"payload_sizes":[],"capture_mode":"headers-only"}'::jsonb,
    NULL, 120, uid
  ) ON CONFLICT DO NOTHING;

  -- Network test with benchmark methodology
  INSERT INTO test_config (id, project_id, name, endpoint_kind, endpoint_ref, workload, methodology, max_duration_secs, created_by)
  VALUES (
    'c0000001-cccc-4000-8000-000000000002', pid, 'Full matrix (benchmark mode)', 'network',
    '{"kind":"network","host":"nwk-ep-ubuntu-dev.eastus.cloudapp.azure.com"}'::jsonb,
    '{"modes":["tcp","dns","tls","http1","http2","udp","download","upload"],"runs":10,"concurrency":2,"timeout_ms":10000,"payload_sizes":[1024,65536],"capture_mode":"full"}'::jsonb,
    '{"warmup_runs":3,"measured_runs":20,"cooldown_ms":1000,"target_error_pct":5.0,"outlier_policy":{"policy":"iqr","k":1.5},"quality_gates":{"max_cv_pct":15.0,"min_samples":10,"max_noise_level":0.3},"publication_gates":{"max_failure_pct":10.0,"require_all_phases":true}}'::jsonb,
    900, uid
  ) ON CONFLICT DO NOTHING;

  -- Proxy endpoint test
  INSERT INTO test_config (id, project_id, name, endpoint_kind, endpoint_ref, workload, max_duration_secs, created_by)
  VALUES (
    'c0000001-cccc-4000-8000-000000000003', pid, 'nginx proxy API check', 'proxy',
    '{"kind":"proxy","proxy_endpoint_id":"d0000001-dddd-4000-8000-000000000001"}'::jsonb,
    '{"modes":["http1","http2"],"runs":10,"concurrency":4,"timeout_ms":10000,"payload_sizes":[],"capture_mode":"headers-only"}'::jsonb,
    300, uid
  ) ON CONFLICT DO NOTHING;

  -- Runtime comparison test
  INSERT INTO test_config (id, project_id, name, endpoint_kind, endpoint_ref, workload, methodology, max_duration_secs, created_by)
  VALUES (
    'c0000001-cccc-4000-8000-000000000004', pid, 'Rust vs Go vs Node throughput', 'runtime',
    '{"kind":"runtime","runtime_id":"e0000001-eeee-4000-8000-000000000001","language":"rust"}'::jsonb,
    '{"modes":["http2"],"runs":30,"concurrency":8,"timeout_ms":15000,"payload_sizes":[4096],"capture_mode":"metrics-only"}'::jsonb,
    '{"warmup_runs":5,"measured_runs":50,"cooldown_ms":2000,"target_error_pct":2.0,"outlier_policy":{"policy":"iqr","k":1.5},"quality_gates":{"max_cv_pct":10.0,"min_samples":20,"max_noise_level":0.2},"publication_gates":{"max_failure_pct":5.0,"require_all_phases":true}}'::jsonb,
    1200, uid
  ) ON CONFLICT DO NOTHING;

  -- ── Test runs ──────────────────────────────────────────────────────

  -- Completed simple test (20 ok, 0 fail)
  INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, tester_id)
  VALUES ('f0000001-ffff-4000-8000-000000000001', 'c0000001-cccc-4000-8000-000000000001', pid, 'completed', now()-interval '2 hours', now()-interval '2 hours'+interval '6 seconds', 20, 0, 'a0000001-aaaa-4000-8000-000000000001')
  ON CONFLICT DO NOTHING;

  -- Completed mixed (18 ok, 2 fail)
  INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, tester_id)
  VALUES ('f0000002-ffff-4000-8000-000000000002', 'c0000001-cccc-4000-8000-000000000001', pid, 'completed', now()-interval '1 hour', now()-interval '1 hour'+interval '8 seconds', 18, 2, 'a0000001-aaaa-4000-8000-000000000001')
  ON CONFLICT DO NOTHING;

  -- Running 40 minutes
  INSERT INTO test_run (id, test_config_id, project_id, status, started_at, success_count, failure_count, tester_id, last_heartbeat)
  VALUES ('f0000003-ffff-4000-8000-000000000003', 'c0000001-cccc-4000-8000-000000000002', pid, 'running', now()-interval '40 minutes', 85, 3, 'a0000001-aaaa-4000-8000-000000000002', now()-interval '5 seconds')
  ON CONFLICT DO NOTHING;

  -- Failed
  INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, error_message, tester_id)
  VALUES ('f0000004-ffff-4000-8000-000000000004', 'c0000001-cccc-4000-8000-000000000003', pid, 'failed', now()-interval '45 minutes', now()-interval '44 minutes', 0, 10, 'Connection refused: nginx proxy not reachable at target host', 'a0000001-aaaa-4000-8000-000000000001')
  ON CONFLICT DO NOTHING;

  -- Queued
  INSERT INTO test_run (id, test_config_id, project_id, status, success_count, failure_count)
  VALUES ('f0000005-ffff-4000-8000-000000000005', 'c0000001-cccc-4000-8000-000000000004', pid, 'queued', 0, 0)
  ON CONFLICT DO NOTHING;

  -- Completed benchmark run (artifact linked after insert below)
  INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, tester_id)
  VALUES ('f0000006-ffff-4000-8000-000000000006', 'c0000001-cccc-4000-8000-000000000002', pid, 'completed', now()-interval '3 hours', now()-interval '3 hours'+interval '180 seconds', 200, 4, 'a0000001-aaaa-4000-8000-000000000002')
  ON CONFLICT DO NOTHING;

  -- Batch of older runs for list density
  INSERT INTO test_run (id, test_config_id, project_id, status, started_at, finished_at, success_count, failure_count, tester_id) VALUES
    ('f0000007-ffff-4000-8000-000000000007', 'c0000001-cccc-4000-8000-000000000001', pid, 'completed', now()-interval '6 hours', now()-interval '6 hours'+interval '4 seconds', 20, 0, 'a0000001-aaaa-4000-8000-000000000001'),
    ('f0000008-ffff-4000-8000-000000000008', 'c0000001-cccc-4000-8000-000000000001', pid, 'completed', now()-interval '12 hours', now()-interval '12 hours'+interval '5 seconds', 20, 0, 'a0000001-aaaa-4000-8000-000000000002'),
    ('f0000009-ffff-4000-8000-000000000009', 'c0000001-cccc-4000-8000-000000000001', pid, 'cancelled', now()-interval '18 hours', now()-interval '18 hours'+interval '2 seconds', 5, 0, 'a0000001-aaaa-4000-8000-000000000001'),
    ('f000000a-ffff-4000-8000-00000000000a', 'c0000001-cccc-4000-8000-000000000002', pid, 'completed', now()-interval '24 hours', now()-interval '24 hours'+interval '120 seconds', 180, 8, 'a0000001-aaaa-4000-8000-000000000002')
  ON CONFLICT DO NOTHING;

  -- ── Benchmark artifact (inserted after test_run to satisfy FK) ────

  INSERT INTO benchmark_artifact (id, test_run_id, environment, methodology, launches, cases, samples, summaries, data_quality)
  VALUES (
    'ab000001-abab-4000-8000-000000000001',
    'f0000006-ffff-4000-8000-000000000006',
    '{"os":"ubuntu-22.04","arch":"x86_64","cpu":"Intel Xeon 8375C","memory_gb":16,"region":"us-east-1","provider":"aws"}'::jsonb,
    '{"warmup_runs":3,"measured_runs":20,"cooldown_ms":1000,"target_error_pct":5.0}'::jsonb,
    '[{"tester":"tester-us-east-1-dev01","started_at":"2026-04-15T12:00:00Z","finished_at":"2026-04-15T12:03:00Z"}]'::jsonb,
    '[{"mode":"tcp","p50":16.8,"p95":24.1,"p99":28.7,"mean":17.1,"stddev":4.2,"cv":0.245,"count":20},{"mode":"dns","p50":12.4,"p95":18.9,"p99":22.3,"mean":12.6,"stddev":3.1,"count":20},{"mode":"tls","p50":55.2,"p95":78.4,"p99":92.1,"mean":56.8,"stddev":12.4,"count":20},{"mode":"http2","p50":88.9,"p95":128.7,"p99":148.2,"mean":91.4,"stddev":21.3,"count":20}]'::jsonb,
    NULL,
    '{"overall_p50":43.3,"overall_p95":62.5,"total_samples":80,"modes_tested":4}'::jsonb,
    '{"noise_level":0.12,"sufficiency":"adequate","publication_ready":true,"publication_blocker_count":0}'::jsonb
  ) ON CONFLICT DO NOTHING;

  -- Link the artifact back to the test_run
  UPDATE test_run SET artifact_id = 'ab000001-abab-4000-8000-000000000001'
  WHERE id = 'f0000006-ffff-4000-8000-000000000006' AND artifact_id IS NULL;

  -- ── Schedules ──────────────────────────────────────────────────────

  INSERT INTO test_schedule (id, test_config_id, project_id, cron_expr, timezone, enabled, next_fire_at) VALUES
    ('50000001-5555-4000-8000-000000000001', 'c0000001-cccc-4000-8000-000000000001', pid, '*/5 * * * *', 'UTC', TRUE, now()+interval '5 minutes'),
    ('50000002-5555-4000-8000-000000000002', 'c0000001-cccc-4000-8000-000000000002', pid, '0 3 * * *', 'America/New_York', TRUE, (CURRENT_DATE+1)+interval '7 hours')
  ON CONFLICT DO NOTHING;

  -- ── Summary ────────────────────────────────────────────────────────
  RAISE NOTICE 'Seed complete for project %', pid;
END $$;

-- Verify counts
SELECT 'test_config' AS tbl, count(*) FROM test_config
UNION ALL SELECT 'test_run', count(*) FROM test_run
UNION ALL SELECT 'test_schedule', count(*) FROM test_schedule
UNION ALL SELECT 'benchmark_artifact', count(*) FROM benchmark_artifact
UNION ALL SELECT 'agent', count(*) FROM agent;
