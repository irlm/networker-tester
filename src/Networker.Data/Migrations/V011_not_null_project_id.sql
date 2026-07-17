DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM agent WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found agent rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM job WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found job rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM schedule WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found schedule rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM deployment WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found deployment rows with NULL project_id — run backfill first';
    END IF;
END $$;

ALTER TABLE agent ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE job ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE schedule ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE deployment ALTER COLUMN project_id SET NOT NULL;
