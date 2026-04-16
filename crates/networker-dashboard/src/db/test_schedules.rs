//! CRUD for the unified `test_schedule` table.
//!
//! Schedules reference `test_config_id` only — the old polymorphic
//! `schedule` table (with job/benchmark_config/deployment columns) is gone.

use chrono::{DateTime, Utc};
use networker_common::TestSchedule;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct NewTestSchedule<'a> {
    pub test_config_id: &'a Uuid,
    pub project_id: &'a str,
    pub cron_expr: &'a str,
    pub timezone: &'a str,
    pub enabled: bool,
    pub created_by: Option<&'a Uuid>,
}

pub async fn create(client: &Client, new: &NewTestSchedule<'_>) -> anyhow::Result<TestSchedule> {
    let row = client
        .query_one(
            "INSERT INTO test_schedule
                (test_config_id, project_id, cron_expr, timezone, enabled, created_by)
             VALUES ($1,$2,$3,$4,$5,$6)
             RETURNING id, test_config_id, project_id, cron_expr, timezone,
                       enabled, last_fired_at, last_run_id, next_fire_at,
                       created_by, created_at",
            &[
                &new.test_config_id,
                &new.project_id,
                &new.cron_expr,
                &new.timezone,
                &new.enabled,
                &new.created_by,
            ],
        )
        .await?;
    Ok(row_to_schedule(&row))
}

pub async fn get(client: &Client, id: &Uuid) -> anyhow::Result<Option<TestSchedule>> {
    let row = client
        .query_opt(
            "SELECT id, test_config_id, project_id, cron_expr, timezone,
                    enabled, last_fired_at, last_run_id, next_fire_at,
                    created_by, created_at
             FROM test_schedule WHERE id = $1",
            &[id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_schedule))
}

pub async fn list(client: &Client, project_id: &str) -> anyhow::Result<Vec<TestSchedule>> {
    let rows = client
        .query(
            "SELECT id, test_config_id, project_id, cron_expr, timezone,
                    enabled, last_fired_at, last_run_id, next_fire_at,
                    created_by, created_at
             FROM test_schedule
             WHERE project_id = $1
             ORDER BY created_at DESC",
            &[&project_id],
        )
        .await?;
    Ok(rows.iter().map(row_to_schedule).collect())
}

/// Return schedules that are enabled and due to fire (next_fire_at <= now or NULL).
pub async fn list_due(client: &Client) -> anyhow::Result<Vec<TestSchedule>> {
    let rows = client
        .query(
            "SELECT id, test_config_id, project_id, cron_expr, timezone,
                    enabled, last_fired_at, last_run_id, next_fire_at,
                    created_by, created_at
             FROM test_schedule
             WHERE enabled = TRUE
               AND (next_fire_at IS NULL OR next_fire_at <= now())
             ORDER BY next_fire_at NULLS FIRST",
            &[],
        )
        .await?;
    Ok(rows.iter().map(row_to_schedule).collect())
}

#[derive(Debug, Default, Clone)]
pub struct UpdateTestSchedule<'a> {
    pub cron_expr: Option<&'a str>,
    pub timezone: Option<&'a str>,
    pub enabled: Option<bool>,
    pub next_fire_at: Option<Option<DateTime<Utc>>>,
}

pub async fn update(
    client: &Client,
    id: &Uuid,
    patch: &UpdateTestSchedule<'_>,
) -> anyhow::Result<Option<TestSchedule>> {
    let next_set = patch.next_fire_at.is_some();
    let next_val: Option<DateTime<Utc>> = patch.next_fire_at.flatten();
    let row = client
        .query_opt(
            "UPDATE test_schedule
             SET cron_expr    = COALESCE($2, cron_expr),
                 timezone     = COALESCE($3, timezone),
                 enabled      = COALESCE($4, enabled),
                 next_fire_at = CASE WHEN $5 THEN $6 ELSE next_fire_at END
             WHERE id = $1
             RETURNING id, test_config_id, project_id, cron_expr, timezone,
                       enabled, last_fired_at, last_run_id, next_fire_at,
                       created_by, created_at",
            &[
                id,
                &patch.cron_expr,
                &patch.timezone,
                &patch.enabled,
                &next_set,
                &next_val,
            ],
        )
        .await?;
    Ok(row.as_ref().map(row_to_schedule))
}

/// Record a fire: stamp last_fired_at = now, set last_run_id, push next_fire_at.
pub async fn mark_fired(
    client: &Client,
    id: &Uuid,
    last_run_id: &Uuid,
    next_fire_at: Option<DateTime<Utc>>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE test_schedule
             SET last_fired_at = now(),
                 last_run_id   = $2,
                 next_fire_at  = $3
             WHERE id = $1",
            &[id, last_run_id, &next_fire_at],
        )
        .await?;
    Ok(())
}

pub async fn delete(client: &Client, id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute("DELETE FROM test_schedule WHERE id = $1", &[id])
        .await?;
    Ok(n > 0)
}

// ── helpers ─────────────────────────────────────────────────────────────
fn row_to_schedule(r: &tokio_postgres::Row) -> TestSchedule {
    TestSchedule {
        id: r.get("id"),
        test_config_id: r.get("test_config_id"),
        project_id: r.get("project_id"),
        cron_expr: r.get("cron_expr"),
        timezone: r.get("timezone"),
        enabled: r.get("enabled"),
        last_fired_at: r.get("last_fired_at"),
        last_run_id: r.get("last_run_id"),
        next_fire_at: r.get("next_fire_at"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
    }
}
