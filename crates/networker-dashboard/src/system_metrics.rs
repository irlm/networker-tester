use chrono::{DateTime, Utc};
use serde::Serialize;
use sysinfo::System;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct SystemMetrics {
    pub cpu_usage_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub uptime_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct DbMetrics {
    pub active_connections: i64,
    pub max_connections: i64,
    pub database_size_bytes: i64,
    pub oldest_transaction_age_seconds: Option<f64>,
    pub cache_hit_ratio: f64,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceUsage {
    pub project_id: Uuid,
    pub name: String,
    pub slug: String,
    pub member_count: i64,
    pub tester_count: i64,
    pub jobs_30d: i64,
    pub runs_30d: i64,
    pub last_activity: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub delete_protection: bool,
}

pub fn collect_system_metrics() -> SystemMetrics {
    let mut sys = System::new_all();
    sys.refresh_all();
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let (disk_used, disk_total) = disks
        .iter()
        .max_by_key(|d| d.total_space())
        .map(|d| (d.total_space() - d.available_space(), d.total_space()))
        .unwrap_or((0, 0));
    SystemMetrics {
        cpu_usage_percent: sys.global_cpu_usage(),
        memory_used_bytes: sys.used_memory(),
        memory_total_bytes: sys.total_memory(),
        disk_used_bytes: disk_used,
        disk_total_bytes: disk_total,
        uptime_seconds: System::uptime(),
    }
}

pub async fn collect_db_metrics(client: &Client) -> anyhow::Result<DbMetrics> {
    let active: i64 = client
        .query_one(
            "SELECT count(*) FROM pg_stat_activity WHERE state = 'active'",
            &[],
        )
        .await?
        .get(0);
    let max_str: String = client.query_one("SHOW max_connections", &[]).await?.get(0);
    let max: i64 = max_str.parse().unwrap_or(100);
    let db_size: i64 = client
        .query_one("SELECT pg_database_size(current_database())", &[])
        .await?
        .get(0);
    let oldest: Option<f64> = client
        .query_opt(
            "SELECT EXTRACT(EPOCH FROM (now() - xact_start)) FROM pg_stat_activity \
             WHERE state != 'idle' AND xact_start IS NOT NULL ORDER BY xact_start ASC LIMIT 1",
            &[],
        )
        .await?
        .map(|r| r.get(0));
    let ratio: f64 = client
        .query_one(
            "SELECT COALESCE(sum(blks_hit)::float / NULLIF(sum(blks_hit) + sum(blks_read), 0), 0) \
             FROM pg_stat_database WHERE datname = current_database()",
            &[],
        )
        .await?
        .get(0);
    Ok(DbMetrics {
        active_connections: active,
        max_connections: max,
        database_size_bytes: db_size,
        oldest_transaction_age_seconds: oldest,
        cache_hit_ratio: ratio,
    })
}

pub async fn collect_workspace_usage(client: &Client) -> anyhow::Result<Vec<WorkspaceUsage>> {
    let rows = client
        .query(
            "SELECT p.project_id, p.name, p.slug, p.deleted_at, \
                    COALESCE(p.delete_protection, FALSE) AS delete_protection, \
                    (SELECT COUNT(*) FROM project_member pm WHERE pm.project_id = p.project_id) as member_count, \
                    (SELECT COUNT(*) FROM agent a WHERE a.project_id = p.project_id) as tester_count, \
                    (SELECT COUNT(*) FROM job j WHERE j.project_id = p.project_id AND j.created_at > now() - interval '30 days') as jobs_30d, \
                    (SELECT COUNT(*) FROM job j2 WHERE j2.project_id = p.project_id AND j2.run_id IS NOT NULL AND j2.created_at > now() - interval '30 days') as runs_30d, \
                    (SELECT MAX(u.last_login_at) FROM project_member pm2 JOIN dash_user u ON u.user_id = pm2.user_id WHERE pm2.project_id = p.project_id) as last_activity \
             FROM project p ORDER BY p.name",
            &[],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| WorkspaceUsage {
            project_id: r.get("project_id"),
            name: r.get("name"),
            slug: r.get("slug"),
            member_count: r.get("member_count"),
            tester_count: r.get("tester_count"),
            jobs_30d: r.get("jobs_30d"),
            runs_30d: r.get("runs_30d"),
            last_activity: r.get("last_activity"),
            deleted_at: r.get("deleted_at"),
            delete_protection: r.get("delete_protection"),
        })
        .collect())
}
