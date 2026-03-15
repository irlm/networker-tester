use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct RunSummary {
    pub run_id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub target_url: String,
    pub target_host: String,
    pub modes: String,
    pub total_runs: i32,
    pub success_count: i32,
    pub failure_count: i32,
}

pub async fn list(
    client: &Client,
    target_host: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<RunSummary>> {
    let rows = if let Some(host) = target_host {
        client
            .query(
                "SELECT RunId, StartedAt, FinishedAt, TargetUrl, TargetHost, Modes,
                        TotalRuns, SuccessCount, FailureCount
                 FROM TestRun WHERE TargetHost = $1
                 ORDER BY StartedAt DESC LIMIT $2 OFFSET $3",
                &[&host, &limit, &offset],
            )
            .await?
    } else {
        client
            .query(
                "SELECT RunId, StartedAt, FinishedAt, TargetUrl, TargetHost, Modes,
                        TotalRuns, SuccessCount, FailureCount
                 FROM TestRun ORDER BY StartedAt DESC LIMIT $1 OFFSET $2",
                &[&limit, &offset],
            )
            .await?
    };

    Ok(rows
        .iter()
        .map(|r| RunSummary {
            run_id: r.get("runid"),
            started_at: r.get("startedat"),
            finished_at: r.get("finishedat"),
            target_url: r.get("targeturl"),
            target_host: r.get("targethost"),
            modes: r.get("modes"),
            total_runs: r.get("totalruns"),
            success_count: r.get("successcount"),
            failure_count: r.get("failurecount"),
        })
        .collect())
}

pub async fn get_attempts(client: &Client, run_id: &Uuid) -> anyhow::Result<serde_json::Value> {
    // Return attempts with their sub-results as JSON
    let rows = client
        .query(
            "SELECT a.AttemptId, a.Protocol, a.SequenceNum, a.StartedAt, a.FinishedAt,
                    a.Success, a.ErrorMessage, a.RetryCount
             FROM RequestAttempt a WHERE a.RunId = $1
             ORDER BY a.SequenceNum",
            &[run_id],
        )
        .await?;

    let attempts: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "attempt_id": r.get::<_, Uuid>("attemptid").to_string(),
                "protocol": r.get::<_, String>("protocol"),
                "sequence_num": r.get::<_, i32>("sequencenum"),
                "started_at": r.get::<_, DateTime<Utc>>("startedat").to_rfc3339(),
                "finished_at": r.get::<_, Option<DateTime<Utc>>>("finishedat").map(|d| d.to_rfc3339()),
                "success": r.get::<_, bool>("success"),
                "error_message": r.get::<_, Option<String>>("errormessage"),
                "retry_count": r.get::<_, i32>("retrycount"),
            })
        })
        .collect();

    Ok(serde_json::Value::Array(attempts))
}
