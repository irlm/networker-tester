use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct PerfLogRow {
    pub id: i64,
    pub logged_at: DateTime<Utc>,
    pub user_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub kind: String,
    pub method: Option<String>,
    pub path: Option<String>,
    pub status: Option<i16>,
    pub total_ms: Option<f32>,
    pub server_ms: Option<f32>,
    pub network_ms: Option<f32>,
    pub source: Option<String>,
    pub component: Option<String>,
    pub trigger: Option<String>,
    pub render_ms: Option<f32>,
    pub item_count: Option<i32>,
    pub meta: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct PerfLogInput {
    pub kind: String,
    pub timestamp: Option<i64>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub status: Option<i16>,
    pub total_ms: Option<f32>,
    pub server_ms: Option<f32>,
    pub network_ms: Option<f32>,
    pub source: Option<String>,
    pub component: Option<String>,
    pub trigger: Option<String>,
    pub render_ms: Option<f32>,
    pub item_count: Option<i32>,
    pub meta: Option<serde_json::Value>,
}

pub async fn insert_batch(
    client: &Client,
    user_id: &Uuid,
    session_id: Option<&str>,
    entries: &[PerfLogInput],
) -> anyhow::Result<u64> {
    if entries.is_empty() {
        return Ok(0);
    }

    let stmt = client
        .prepare(
            "INSERT INTO perf_log (logged_at, user_id, session_id, kind, method, path, status,
                                   total_ms, server_ms, network_ms, source,
                                   component, \"trigger\", render_ms, item_count, meta)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .await?;

    let mut count: u64 = 0;
    let sess = session_id.map(|s| s.to_string());

    for entry in entries {
        let logged_at = entry
            .timestamp
            .and_then(DateTime::from_timestamp_millis)
            .unwrap_or_else(Utc::now);

        client
            .execute(
                &stmt,
                &[
                    &logged_at,
                    user_id,
                    &sess,
                    &entry.kind,
                    &entry.method,
                    &entry.path,
                    &entry.status,
                    &entry.total_ms,
                    &entry.server_ms,
                    &entry.network_ms,
                    &entry.source,
                    &entry.component,
                    &entry.trigger,
                    &entry.render_ms,
                    &entry.item_count,
                    &entry.meta,
                ],
            )
            .await?;
        count += 1;
    }

    Ok(count)
}

pub async fn list(
    client: &Client,
    kind: Option<&str>,
    path_filter: Option<&str>,
    user_id_filter: Option<&Uuid>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<PerfLogRow>> {
    let base = "SELECT id, logged_at, user_id, session_id, kind, method, path, status,
                       total_ms, server_ms, network_ms, source,
                       component, \"trigger\", render_ms, item_count, meta
                FROM perf_log WHERE 1=1";

    let mut clauses = Vec::new();
    let mut param_idx: usize = 1;

    if kind.is_some() {
        clauses.push(format!("kind = ${param_idx}"));
        param_idx += 1;
    }
    if path_filter.is_some() {
        clauses.push(format!("path ILIKE '%' || ${param_idx} || '%'"));
        param_idx += 1;
    }
    if user_id_filter.is_some() {
        clauses.push(format!("user_id = ${param_idx}"));
        param_idx += 1;
    }

    let order = format!("ORDER BY logged_at DESC LIMIT ${param_idx} OFFSET ${}", param_idx + 1);
    let sql = if clauses.is_empty() {
        format!("{base} {order}")
    } else {
        format!("{base} AND {} {order}", clauses.join(" AND "))
    };

    let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
    if let Some(k) = &kind {
        params.push(k);
    }
    if let Some(p) = &path_filter {
        params.push(p);
    }
    if let Some(u) = &user_id_filter {
        params.push(u);
    }
    params.push(&limit);
    params.push(&offset);

    let rows = client.query(&sql, &params).await?;
    Ok(rows
        .iter()
        .map(|r| PerfLogRow {
            id: r.get("id"),
            logged_at: r.get("logged_at"),
            user_id: r.get("user_id"),
            session_id: r.get("session_id"),
            kind: r.get("kind"),
            method: r.get("method"),
            path: r.get("path"),
            status: r.get("status"),
            total_ms: r.get("total_ms"),
            server_ms: r.get("server_ms"),
            network_ms: r.get("network_ms"),
            source: r.get("source"),
            component: r.get("component"),
            trigger: r.get("trigger"),
            render_ms: r.get("render_ms"),
            item_count: r.get("item_count"),
            meta: r.get("meta"),
        })
        .collect())
}

pub async fn stats(client: &Client) -> anyhow::Result<serde_json::Value> {
    let row = client
        .query_one(
            "SELECT
                COUNT(*) FILTER (WHERE kind = 'api') AS api_count,
                COUNT(*) FILTER (WHERE kind = 'render') AS render_count,
                AVG(total_ms) FILTER (WHERE kind = 'api') AS avg_total_ms,
                AVG(server_ms) FILTER (WHERE kind = 'api') AS avg_server_ms,
                AVG(render_ms) FILTER (WHERE kind = 'render') AS avg_render_ms,
                PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY total_ms) FILTER (WHERE kind = 'api') AS p95_total_ms,
                PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY render_ms) FILTER (WHERE kind = 'render') AS p95_render_ms,
                COUNT(*) FILTER (WHERE kind = 'api' AND total_ms > 200) AS slow_api_count,
                COUNT(*) FILTER (WHERE kind = 'render' AND render_ms > 16) AS janky_render_count
             FROM perf_log",
            &[],
        )
        .await?;

    Ok(serde_json::json!({
        "api_count": row.get::<_, Option<i64>>(0).unwrap_or(0),
        "render_count": row.get::<_, Option<i64>>(1).unwrap_or(0),
        "avg_total_ms": row.get::<_, Option<f64>>(2),
        "avg_server_ms": row.get::<_, Option<f64>>(3),
        "avg_render_ms": row.get::<_, Option<f64>>(4),
        "p95_total_ms": row.get::<_, Option<f64>>(5),
        "p95_render_ms": row.get::<_, Option<f64>>(6),
        "slow_api_count": row.get::<_, Option<i64>>(7).unwrap_or(0),
        "janky_render_count": row.get::<_, Option<i64>>(8).unwrap_or(0),
    }))
}
