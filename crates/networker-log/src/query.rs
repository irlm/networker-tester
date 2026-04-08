//! Query functions for reading from the `service_log` table.
//!
//! Used by the dashboard API to list log entries and compute per-service
//! level-bucket statistics.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::types::ToSql;
use uuid::Uuid;

// ── Input / output types ──────────────────────────────────────────────────────

/// Parameters for a filtered log listing query.
pub struct LogQuery {
    /// Filter by service name (exact match).
    pub service: Option<String>,
    /// Filter by maximum level value: returns entries WHERE level <= min_level.
    /// 1=ERROR … 5=TRACE.
    pub min_level: Option<i16>,
    /// Filter by dashboard config / probe-set identifier.
    pub config_id: Option<Uuid>,
    /// Filter by project identifier.
    pub project_id: Option<String>,
    /// Case-insensitive substring search against the message column.
    pub search: Option<String>,
    /// Lower bound timestamp (inclusive).
    pub from: DateTime<Utc>,
    /// Upper bound timestamp (inclusive).
    pub to: DateTime<Utc>,
    /// Maximum number of rows to return.
    pub limit: i64,
    /// Row offset for pagination.
    pub offset: i64,
}

/// A single log row returned from the database.
#[derive(Debug, Serialize)]
pub struct LogRow {
    pub ts: DateTime<Utc>,
    pub service: String,
    pub level: i16,
    pub message: String,
    pub config_id: Option<Uuid>,
    pub project_id: Option<String>,
    pub trace_id: Option<Uuid>,
    pub fields: Option<serde_json::Value>,
}

/// Paginated result set for a [`LogQuery`].
#[derive(Debug, Serialize)]
pub struct LogQueryResponse {
    pub entries: Vec<LogRow>,
    /// Total matching rows (before LIMIT/OFFSET).
    pub total: i64,
    /// `true` when `total > 10_000` — callers should encourage narrower filters.
    pub truncated: bool,
}

/// Per-level counts for a single service.
#[derive(Debug, Serialize, Default)]
pub struct ServiceStats {
    pub error: i64,
    pub warn: i64,
    pub info: i64,
    pub debug: i64,
    pub trace: i64,
}

/// Aggregated log statistics over a time window.
#[derive(Debug, Serialize)]
pub struct LogStats {
    pub by_service: HashMap<String, ServiceStats>,
    pub total: i64,
}

// ── list ──────────────────────────────────────────────────────────────────────

/// List log entries matching `q`, with a total count and truncation flag.
pub async fn list(
    client: &tokio_postgres::Client,
    q: &LogQuery,
) -> anyhow::Result<LogQueryResponse> {
    // ── Build dynamic WHERE clause ────────────────────────────────────────────
    // $1 = from, $2 = to are always present.
    let mut conditions: Vec<String> = vec!["ts >= $1".into(), "ts <= $2".into()];
    let mut params: Vec<Box<dyn ToSql + Sync + Send>> = vec![Box::new(q.from), Box::new(q.to)];

    // Helper: next positional placeholder
    let mut next_idx = 3usize;
    let mut push_param = |params: &mut Vec<Box<dyn ToSql + Sync + Send>>,
                          conditions: &mut Vec<String>,
                          sql: &str,
                          val: Box<dyn ToSql + Sync + Send>| {
        conditions.push(sql.replace("{}", &format!("${next_idx}")));
        params.push(val);
        next_idx += 1;
    };

    if let Some(ref svc) = q.service {
        push_param(
            &mut params,
            &mut conditions,
            "service = {}",
            Box::new(svc.clone()),
        );
    }

    if let Some(min_level) = q.min_level {
        push_param(
            &mut params,
            &mut conditions,
            "level <= {}",
            Box::new(min_level),
        );
    }

    if let Some(cid) = q.config_id {
        push_param(
            &mut params,
            &mut conditions,
            "config_id = {}",
            Box::new(cid),
        );
    }

    if let Some(ref pid) = q.project_id {
        push_param(
            &mut params,
            &mut conditions,
            "project_id = {}",
            Box::new(pid.clone()),
        );
    }

    if let Some(ref search) = q.search {
        let escaped = search.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        push_param(
            &mut params,
            &mut conditions,
            "message ILIKE {}",
            Box::new(format!("%{escaped}%")),
        );
    }

    let where_clause = conditions.join(" AND ");

    // ── COUNT(*) ──────────────────────────────────────────────────────────────
    // Cap the scan at 10,001 rows so that large tables don't incur a full seq-scan.
    // The truncated flag is set when total > 10,000.
    let count_sql = format!(
        "SELECT COUNT(*) FROM (SELECT 1 FROM service_log WHERE {where_clause} LIMIT 10001) sub"
    );
    let param_refs: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|p| p.as_ref() as &(dyn ToSql + Sync))
        .collect();

    let count_row = client
        .query_one(&count_sql, &param_refs)
        .await
        .map_err(|e| anyhow::anyhow!("count query failed: {e}"))?;
    let total: i64 = count_row.get(0);

    // ── SELECT rows ───────────────────────────────────────────────────────────
    let select_sql = format!(
        "SELECT ts, service, level, message, config_id, project_id, trace_id, fields \
         FROM service_log WHERE {where_clause} \
         ORDER BY ts DESC \
         LIMIT ${next_idx} OFFSET ${}",
        next_idx + 1,
    );
    params.push(Box::new(q.limit));
    params.push(Box::new(q.offset));

    let param_refs: Vec<&(dyn ToSql + Sync)> = params
        .iter()
        .map(|p| p.as_ref() as &(dyn ToSql + Sync))
        .collect();

    let rows = client
        .query(&select_sql, &param_refs)
        .await
        .map_err(|e| anyhow::anyhow!("list query failed: {e}"))?;

    let entries = rows
        .into_iter()
        .map(|row| LogRow {
            ts: row.get(0),
            service: row.get(1),
            level: row.get(2),
            message: row.get(3),
            config_id: row.get(4),
            project_id: row.get(5),
            trace_id: row.get(6),
            fields: row.get(7),
        })
        .collect();

    Ok(LogQueryResponse {
        entries,
        total,
        truncated: total > 10_000,
    })
}

// ── stats ─────────────────────────────────────────────────────────────────────

/// Compute per-service level-bucket counts over the given time window.
pub async fn stats(
    client: &tokio_postgres::Client,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> anyhow::Result<LogStats> {
    let rows = client
        .query(
            "SELECT service, level, COUNT(*) \
             FROM service_log \
             WHERE ts >= $1 AND ts <= $2 \
             GROUP BY service, level",
            &[&from, &to],
        )
        .await
        .map_err(|e| anyhow::anyhow!("stats query failed: {e}"))?;

    let mut by_service: HashMap<String, ServiceStats> = HashMap::new();
    let mut grand_total: i64 = 0;

    for row in rows {
        let service: String = row.get(0);
        let level: i16 = row.get(1);
        let count: i64 = row.get(2);

        grand_total += count;

        let entry = by_service.entry(service).or_default();
        match level {
            1 => entry.error += count,
            2 => entry.warn += count,
            3 => entry.info += count,
            4 => entry.debug += count,
            5 => entry.trace += count,
            _ => {} // unknown level — skip
        }
    }

    Ok(LogStats {
        by_service,
        total: grand_total,
    })
}
