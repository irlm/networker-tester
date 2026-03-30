use chrono::{DateTime, Utc};
use networker_tester::tls_profile::TlsEndpointProfile;
use serde::Serialize;
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;
use uuid::Uuid;

fn is_undefined_table(err: &tokio_postgres::Error) -> bool {
    err.as_db_error()
        .map(|db_err| db_err.code() == &SqlState::UNDEFINED_TABLE)
        .unwrap_or(false)
}

#[derive(Debug, Serialize)]
pub struct TlsProfileSummaryRow {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub host: String,
    pub port: i32,
    pub target_kind: String,
    pub coverage_level: String,
    pub summary_status: String,
    pub summary_score: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct TlsProfileDetail {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub host: String,
    pub port: i32,
    pub target_kind: String,
    pub coverage_level: String,
    pub summary_status: String,
    pub summary_score: Option<i32>,
    pub profile: TlsEndpointProfile,
}

pub async fn list(
    client: &Client,
    project_id: &Uuid,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<TlsProfileSummaryRow>> {
    let rows = match client
        .query(
            "SELECT Id, StartedAt, Host, Port, TargetKind, CoverageLevel, SummaryStatus, SummaryScore
             FROM TlsProfileRun
             WHERE ProjectId = $1
             ORDER BY StartedAt DESC
             LIMIT $2 OFFSET $3",
            &[project_id, &limit, &offset],
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            if is_undefined_table(&e) {
                return Ok(vec![]);
            }
            return Err(e.into());
        }
    };

    Ok(rows
        .into_iter()
        .map(|row| TlsProfileSummaryRow {
            id: row.get("id"),
            started_at: row.get("startedat"),
            host: row.get("host"),
            port: row.get("port"),
            target_kind: row.get("targetkind"),
            coverage_level: row.get("coveragelevel"),
            summary_status: row.get("summarystatus"),
            summary_score: row.get("summaryscore"),
        })
        .collect())
}

pub async fn get(
    client: &Client,
    project_id: &Uuid,
    id: &Uuid,
) -> anyhow::Result<Option<TlsProfileDetail>> {
    let row = match client
        .query_opt(
            "SELECT Id, StartedAt, Host, Port, TargetKind, CoverageLevel, SummaryStatus, SummaryScore, ProfileJson
             FROM TlsProfileRun
             WHERE ProjectId = $1 AND Id = $2",
            &[project_id, id],
        )
        .await
    {
        Ok(row) => row,
        Err(e) => {
            if is_undefined_table(&e) {
                return Ok(None);
            }
            return Err(e.into());
        }
    };

    let Some(row) = row else {
        return Ok(None);
    };

    let profile_json: serde_json::Value = row.get("profilejson");
    let profile: TlsEndpointProfile = serde_json::from_value(profile_json)?;

    Ok(Some(TlsProfileDetail {
        id: row.get("id"),
        started_at: row.get("startedat"),
        host: row.get("host"),
        port: row.get("port"),
        target_kind: row.get("targetkind"),
        coverage_level: row.get("coveragelevel"),
        summary_status: row.get("summarystatus"),
        summary_score: row.get("summaryscore"),
        profile,
    }))
}
