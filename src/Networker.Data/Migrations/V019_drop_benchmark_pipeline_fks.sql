DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmarkrun') THEN
    ALTER TABLE benchmarkrun DROP CONSTRAINT IF EXISTS fk_benchmarkrun_testrun;
  END IF;
  IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmarksample') THEN
    ALTER TABLE benchmarksample DROP CONSTRAINT IF EXISTS fk_benchmarksample_attempt;
  END IF;
END $$;
