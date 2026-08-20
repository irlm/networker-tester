-- V054: deployment.exit_code + deployment.failed_step — post-mortem install
-- diagnostics (issue #816).
--
-- A failed matrix cell's only durable trace used to be the generic
-- "install.sh exited with code 1" on the deployment/run rows: the in-guest
-- output and remote diagnostics #806 captures were visible ONLY on the live
-- deploy stream, so an unattended failure (the 2026-08-19 Windows/Caddy
-- matrix, group 7c3ab4a7 — all 8 cells) lost its diagnosis. DeployRunner now
-- persists, alongside the existing bounded/ANSI-stripped `log`:
--
--   * exit_code    — install.sh's raw exit code (NULL while running, and for
--                    docker-provider deployments which never shell out);
--   * failed_step  — the last "Step N: …" header install.sh printed before
--                    dying, i.e. the failing phase without reading the log.
--
-- The runner also folds the last "✗ …" error line into error_message, which
-- the orchestrator's DeploymentFailed arm copies onto the run — so run rows
-- carry the first actionable line, not just the exit code.
--
-- Idempotent (ADD COLUMN IF NOT EXISTS); no backfill — historical rows keep
-- NULL (unknown), which every reader treats as "not recorded".
ALTER TABLE deployment
    ADD COLUMN IF NOT EXISTS exit_code INT,
    ADD COLUMN IF NOT EXISTS failed_step TEXT;
