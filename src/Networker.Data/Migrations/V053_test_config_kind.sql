-- V053: persist the product-level purpose of each test configuration.
--
-- endpoint_kind answers "what does this run target?" (network/proxy/runtime),
-- while test_kind answers "what workflow created it?". Keeping those axes
-- separate lets the Runs UI distinguish URL probes, SDK probes, benchmarks,
-- and ordinary network tests without relying on display-name conventions.
ALTER TABLE test_config
    ADD COLUMN IF NOT EXISTS test_kind TEXT NOT NULL DEFAULT 'network';

UPDATE test_config
SET test_kind = CASE
    WHEN workload->'modes' ? 'sdkprobe' THEN 'sdk_probe'
    WHEN methodology IS NOT NULL OR workload->'modes' ? 'apibench' THEN 'benchmark'
    -- Matrix cells of methodology-less comparison groups carry the cell name
    -- shape "{label} · cg-{id8}·{i}·{nonce}" — they are benchmark cells even
    -- without an apibench mode or a stored methodology.
    WHEN name LIKE '% · cg-%' THEN 'benchmark'
    -- URL-probe page configs across all naming generations: "Diag: <host>",
    -- multi-URL sets "Diag set: <host> +N" (v0.28.231), and the pre-rename
    -- watchlist shape "Probe: <host>" — all still watchlist members.
    WHEN name LIKE 'Diag: %' OR name LIKE 'Diag set: %' OR name LIKE 'Probe: %' THEN 'url_probe'
    ELSE 'network'
END
WHERE workload->'modes' ? 'sdkprobe'
   OR methodology IS NOT NULL
   OR workload->'modes' ? 'apibench'
   OR name LIKE '% · cg-%'
   OR name LIKE 'Diag: %'
   OR name LIKE 'Diag set: %'
   OR name LIKE 'Probe: %';

ALTER TABLE test_config
    DROP CONSTRAINT IF EXISTS test_config_test_kind_check;

ALTER TABLE test_config
    ADD CONSTRAINT test_config_test_kind_check
    CHECK (test_kind IN ('network', 'url_probe', 'sdk_probe', 'benchmark'));

CREATE INDEX IF NOT EXISTS ix_test_config_project_test_kind
    ON test_config (project_id, test_kind);
