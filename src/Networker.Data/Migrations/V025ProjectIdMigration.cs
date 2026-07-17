using Npgsql;

namespace Networker.Data.Migrations;

/// <summary>
/// V025: convert <c>project.project_id</c> (and every FK that references it)
/// from UUID to the 14-char base36 <see cref="ProjectId36"/> format.
///
/// This is the one migration the Rust runner implemented in code rather than
/// SQL (<c>migrate_project_ids</c> in
/// <c>crates/networker-dashboard/src/db/migrations.rs</c>) because the base36
/// encoding + Damm check digits cannot be computed in pure SQL. This class is
/// a step-for-step port: same temporary columns, same FK-table lists, same
/// constraint names, same index rebuilds, same <c>project_routing</c> seed.
/// New ids are generated with <c>zone = "us"</c>, <c>server_id = "a20"</c> and
/// the project's original <c>created_at</c> — identical inputs to the Rust
/// call <c>ProjectId::generate_deterministic("us", "a20", unix_secs)</c>.
/// </summary>
internal static class V025ProjectIdMigration
{
    /// <summary>Tables whose project_id was NOT NULL when V025 ran (re-FK'd with ON DELETE CASCADE).</summary>
    private static readonly string[] NotNullFkTables =
    [
        "project_member",
        "cloud_account",
        "share_link",
        "command_approval",
        "test_visibility_rule",
        "workspace_invite",
        "workspace_warning",
        "benchmark_compare_preset",
        "benchmark_vm_catalog",
        "benchmark_config",
    ];

    /// <summary>Tables made NOT NULL by V011 (re-FK'd without CASCADE).</summary>
    private static readonly string[] V011NotNullTables = ["agent", "job", "schedule", "deployment"];

    /// <summary>Tables where project_id may still be nullable.</summary>
    private static readonly string[] NullableFkTables = ["test_definition", "cloud_connection"];

    private static IEnumerable<string> AllFkTables =>
        NotNullFkTables.Concat(V011NotNullTables).Concat(NullableFkTables);

    public static async Task ApplyAsync(NpgsqlConnection conn, CancellationToken ct)
    {
        // ── Step 1: temporary columns on project ─────────────────────────
        await ExecAsync(conn,
            "ALTER TABLE project ADD COLUMN IF NOT EXISTS new_project_id CHAR(14);" +
            "ALTER TABLE project ADD COLUMN IF NOT EXISTS old_project_id UUID;", ct);

        // ── Step 2: generate a new id for each existing project ──────────
        var projects = new List<(Guid OldId, DateTimeOffset CreatedAt)>();
        await using (var cmd = new NpgsqlCommand("SELECT project_id, created_at FROM project", conn))
        await using (var reader = await cmd.ExecuteReaderAsync(ct))
        {
            while (await reader.ReadAsync(ct))
            {
                projects.Add((reader.GetGuid(0), reader.GetFieldValue<DateTimeOffset>(1)));
            }
        }

        foreach (var (oldId, createdAt) in projects)
        {
            var newId = ProjectId36.GenerateDeterministic("us", "a20", createdAt.ToUnixTimeSeconds());
            await using var update = new NpgsqlCommand(
                "UPDATE project SET new_project_id = $1, old_project_id = $2 WHERE project_id = $3", conn);
            update.Parameters.AddWithValue(newId);
            update.Parameters.AddWithValue(oldId);
            update.Parameters.AddWithValue(oldId);
            await update.ExecuteNonQueryAsync(ct);
        }

        // ── Step 3: add new_project_id to all FK tables ──────────────────
        foreach (var table in AllFkTables)
        {
            await ExecAsync(conn, $"ALTER TABLE {table} ADD COLUMN IF NOT EXISTS new_project_id CHAR(14);", ct);
        }

        // TlsProfileRun (mixed-case "ProjectId") — only if the tester created it.
        await ExecAsync(conn, """
            DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    ALTER TABLE tlsprofilerun ADD COLUMN IF NOT EXISTS new_project_id CHAR(14);
                END IF;
            END $$;
            """, ct);

        // ── Step 4: populate new_project_id by joining on project ────────
        foreach (var table in AllFkTables)
        {
            await ExecAsync(conn,
                $"UPDATE {table} t SET new_project_id = p.new_project_id " +
                "FROM project p WHERE t.project_id = p.project_id AND t.new_project_id IS NULL", ct);
        }

        await ExecAsync(conn, """
            DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    UPDATE tlsprofilerun t SET new_project_id = p.new_project_id
                    FROM project p WHERE t."ProjectId" = p.project_id AND t.new_project_id IS NULL;
                END IF;
            END $$;
            """, ct);

        // ── Step 5: drop old FK constraints ──────────────────────────────
        foreach (var table in AllFkTables)
        {
            await ExecAsync(conn, $"ALTER TABLE {table} DROP CONSTRAINT IF EXISTS {table}_project_id_fkey;", ct);
        }

        await ExecAsync(conn, """
            DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    ALTER TABLE tlsprofilerun DROP CONSTRAINT IF EXISTS "TlsProfileRun_ProjectId_fkey";
                    ALTER TABLE tlsprofilerun DROP CONSTRAINT IF EXISTS tlsprofilerun_projectid_fkey;
                END IF;
            END $$;
            """, ct);

        // ── Step 6: drop the project PK ──────────────────────────────────
        await ExecAsync(conn, "ALTER TABLE project DROP CONSTRAINT IF EXISTS project_pkey;", ct);

        // ── Step 7: swap columns, rebuild FKs ────────────────────────────
        await ExecAsync(conn,
            "ALTER TABLE project DROP COLUMN project_id;" +
            "ALTER TABLE project RENAME COLUMN new_project_id TO project_id;" +
            "ALTER TABLE project ALTER COLUMN project_id SET NOT NULL;" +
            "ALTER TABLE project ADD PRIMARY KEY (project_id);", ct);

        foreach (var table in NotNullFkTables)
        {
            await ExecAsync(conn,
                $"ALTER TABLE {table} DROP COLUMN project_id; " +
                $"ALTER TABLE {table} RENAME COLUMN new_project_id TO project_id; " +
                $"ALTER TABLE {table} ALTER COLUMN project_id SET NOT NULL; " +
                $"ALTER TABLE {table} ADD CONSTRAINT {table}_project_id_fkey " +
                "FOREIGN KEY (project_id) REFERENCES project(project_id) ON DELETE CASCADE;", ct);
        }

        foreach (var table in V011NotNullTables)
        {
            await ExecAsync(conn,
                $"ALTER TABLE {table} DROP COLUMN project_id; " +
                $"ALTER TABLE {table} RENAME COLUMN new_project_id TO project_id; " +
                $"ALTER TABLE {table} ALTER COLUMN project_id SET NOT NULL; " +
                $"ALTER TABLE {table} ADD CONSTRAINT {table}_project_id_fkey " +
                "FOREIGN KEY (project_id) REFERENCES project(project_id);", ct);
        }

        foreach (var table in NullableFkTables)
        {
            await ExecAsync(conn,
                $"ALTER TABLE {table} DROP COLUMN project_id; " +
                $"ALTER TABLE {table} RENAME COLUMN new_project_id TO project_id; " +
                $"ALTER TABLE {table} ADD CONSTRAINT {table}_project_id_fkey " +
                "FOREIGN KEY (project_id) REFERENCES project(project_id);", ct);
        }

        await ExecAsync(conn, """
            DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    ALTER TABLE tlsprofilerun DROP COLUMN "ProjectId";
                    ALTER TABLE tlsprofilerun RENAME COLUMN new_project_id TO "ProjectId";
                    ALTER TABLE tlsprofilerun ALTER COLUMN "ProjectId" SET NOT NULL;
                    ALTER TABLE tlsprofilerun ADD CONSTRAINT tlsprofilerun_projectid_fkey
                        FOREIGN KEY ("ProjectId") REFERENCES project(project_id);
                END IF;
            END $$;
            """, ct);

        await ExecAsync(conn,
            "ALTER TABLE project_member DROP CONSTRAINT IF EXISTS project_member_pkey;" +
            "ALTER TABLE project_member ADD PRIMARY KEY (project_id, user_id);", ct);

        // ── Step 8: recreate indexes that referenced project_id ──────────
        await ExecAsync(conn, """
            DROP INDEX IF EXISTS ix_agent_project;
            CREATE INDEX IF NOT EXISTS ix_agent_project ON agent (project_id);
            DROP INDEX IF EXISTS ix_test_def_project;
            CREATE INDEX IF NOT EXISTS ix_test_def_project ON test_definition (project_id);
            DROP INDEX IF EXISTS ix_job_project;
            CREATE INDEX IF NOT EXISTS ix_job_project ON job (project_id, status, created_at DESC);
            DROP INDEX IF EXISTS ix_schedule_project;
            CREATE INDEX IF NOT EXISTS ix_schedule_project ON schedule (project_id) WHERE enabled = TRUE;
            DROP INDEX IF EXISTS ix_deployment_project;
            CREATE INDEX IF NOT EXISTS ix_deployment_project ON deployment (project_id, status, created_at DESC);
            DROP INDEX IF EXISTS ix_cloud_account_project;
            CREATE INDEX IF NOT EXISTS ix_cloud_account_project ON cloud_account (project_id);
            DROP INDEX IF EXISTS ix_share_link_project;
            CREATE INDEX IF NOT EXISTS ix_share_link_project ON share_link (project_id, resource_type);
            DROP INDEX IF EXISTS ix_command_approval_pending;
            CREATE INDEX IF NOT EXISTS ix_command_approval_pending ON command_approval (project_id, status) WHERE status = 'pending';
            DROP INDEX IF EXISTS ix_visibility_project;
            CREATE INDEX IF NOT EXISTS ix_visibility_project ON test_visibility_rule (project_id, user_id, resource_type);
            DROP INDEX IF EXISTS ix_project_member_user;
            CREATE INDEX IF NOT EXISTS ix_project_member_user ON project_member (user_id);
            DROP INDEX IF EXISTS ix_workspace_invite_project;
            CREATE INDEX IF NOT EXISTS ix_workspace_invite_project ON workspace_invite (project_id, status);
            DROP INDEX IF EXISTS ix_workspace_warning_unique;
            CREATE UNIQUE INDEX IF NOT EXISTS ix_workspace_warning_unique ON workspace_warning (project_id, warning_type);
            DROP INDEX IF EXISTS ix_benchmark_compare_preset_name;
            CREATE UNIQUE INDEX IF NOT EXISTS ix_benchmark_compare_preset_name ON benchmark_compare_preset (project_id, name_key);
            DROP INDEX IF EXISTS ix_benchmark_compare_preset_project_updated;
            CREATE INDEX IF NOT EXISTS ix_benchmark_compare_preset_project_updated ON benchmark_compare_preset (project_id, updated_at DESC);
            DROP INDEX IF EXISTS ix_benchmark_vm_catalog_project;
            CREATE INDEX IF NOT EXISTS ix_benchmark_vm_catalog_project ON benchmark_vm_catalog (project_id);
            DROP INDEX IF EXISTS ix_benchmark_config_project;
            CREATE INDEX IF NOT EXISTS ix_benchmark_config_project ON benchmark_config (project_id, created_at DESC);
            """, ct);

        await ExecAsync(conn, """
            DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    DROP INDEX IF EXISTS ix_tlsprofilerun_project;
                    CREATE INDEX IF NOT EXISTS ix_tlsprofilerun_project ON tlsprofilerun ("ProjectId", "StartedAt" DESC);
                END IF;
            END $$;
            """, ct);

        // ── Step 9: seed project_routing (all projects → us/us) ──────────
        await ExecAsync(conn, """
            INSERT INTO project_routing (project_id, home_zone, current_zone)
            SELECT project_id, 'us', 'us' FROM project
            ON CONFLICT DO NOTHING;
            """, ct);

        // ── Step 10: zone-prefix index for routing lookups ───────────────
        await ExecAsync(conn,
            "CREATE INDEX IF NOT EXISTS ix_project_zone_prefix ON project (substring(project_id from 1 for 2));", ct);

        // ── Step 11: drop the old_project_id helper column ───────────────
        await ExecAsync(conn, "ALTER TABLE project DROP COLUMN IF EXISTS old_project_id;", ct);
    }

    private static async Task ExecAsync(NpgsqlConnection conn, string sql, CancellationToken ct)
    {
        await using var cmd = new NpgsqlCommand(sql, conn);
        await cmd.ExecuteNonQueryAsync(ct);
    }
}
