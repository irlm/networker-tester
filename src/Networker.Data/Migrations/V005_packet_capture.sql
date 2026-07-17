DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'testrun') THEN
    ALTER TABLE TestRun ADD COLUMN IF NOT EXISTS packet_capture_json JSONB;
  END IF;
END $$;
