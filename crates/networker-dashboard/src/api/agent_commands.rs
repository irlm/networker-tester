//! REST endpoints for the command-based agent orchestration flow.
//!
//! Three handlers, all project-scoped under `/api/projects/{project_id}`:
//!
//! * `POST /agents/{agent_id}/commands` — Operator dispatches a typed command.
//! * `GET  /commands/{command_id}` — Viewer reads the current `agent_command`
//!   row (status/result/error).
//! * `GET  /commands/{command_id}/stream` — Viewer receives an SSE stream of
//!   `service_log` lines tagged with this command_id, plus a final `done`
//!   event once `finished_at IS NOT NULL`.
//!
//! The `dispatch_command` service (Task 4) handles the DB INSERT, JWT minting
//! and WS push; this module is a thin HTTP facade over it with project
//! membership checks.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, post},
    Json, Router,
};
use futures::stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::auth::{require_project_role, AuthUser, ProjectContext, ProjectRole};
use crate::db::agent_commands as db_cmd;
use crate::AppState;

// ─── Dispatch ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DispatchBody {
    verb: String,
    #[serde(default)]
    args: serde_json::Value,
    #[serde(default)]
    config_id: Option<Uuid>,
    /// Defaults to 60s if absent. Caller-controllable.
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
struct DispatchResponse {
    command_id: Uuid,
    agent_id: Uuid,
    verb: String,
}

/// Confirms `agent_id` exists *within this project*. Returns 404 otherwise.
async fn ensure_agent_in_project(
    state: &Arc<AppState>,
    agent_id: &Uuid,
    project_id: &str,
) -> Result<(), (StatusCode, String)> {
    let client = state
        .db
        .get()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db pool".into()))?;
    let row = client
        .query_opt(
            "SELECT 1 FROM agent WHERE agent_id = $1 AND project_id = $2",
            &[&agent_id, &project_id],
        )
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db query".into()))?;
    if row.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("agent {agent_id} not in this project"),
        ));
    }
    Ok(())
}

async fn dispatch(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, agent_id)): Path<(String, Uuid)>,
    req: axum::extract::Request,
) -> Result<(StatusCode, Json<DispatchResponse>), (StatusCode, String)> {
    require_project_role(&ctx, ProjectRole::Operator)
        .map_err(|s| (s, "Operator role required".into()))?;

    let user = req
        .extensions()
        .get::<AuthUser>()
        .cloned()
        .ok_or((StatusCode::UNAUTHORIZED, "auth required".into()))?;

    ensure_agent_in_project(&state, &agent_id, &ctx.project_id).await?;

    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 64)
        .await
        .map_err(|_| (StatusCode::BAD_REQUEST, "body read".into()))?;
    let body: DispatchBody = serde_json::from_slice(&body_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad body: {e}")))?;

    let timeout = body.timeout_secs.unwrap_or(60);
    let envelope = crate::agent_dispatch::dispatch_command(
        &state,
        agent_id,
        &body.verb,
        body.args,
        body.config_id,
        timeout,
        Some(user.user_id),
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("dispatch: {e}")))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(DispatchResponse {
            command_id: envelope.command_id,
            agent_id,
            verb: envelope.verb,
        }),
    ))
}

// ─── Fetch ───────────────────────────────────────────────────────────────────

async fn fetch(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, command_id)): Path<(String, Uuid)>,
) -> Result<Json<db_cmd::AgentCommandRow>, (StatusCode, String)> {
    // Viewer role is implied by project membership; no extra check needed.
    let client = state
        .db
        .get()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db pool".into()))?;
    let row = db_cmd::fetch_by_id(&client, &command_id)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db query".into()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("command {command_id} not found"),
            )
        })?;

    // Ensure the command's agent belongs to this project.
    ensure_agent_in_project(&state, &row.agent_id, &ctx.project_id).await?;

    Ok(Json(row))
}

// ─── SSE stream ──────────────────────────────────────────────────────────────

/// Row we stream out as an SSE `log` event.
#[derive(Debug, Serialize)]
struct LogEvent {
    log_id: i64,
    ts: chrono::DateTime<chrono::Utc>,
    level: i16,
    message: String,
    fields: Option<serde_json::Value>,
}

/// Final `done` event payload.
#[derive(Debug, Serialize)]
struct DoneEvent {
    command_id: Uuid,
    status: String,
    error_message: Option<String>,
}

async fn stream(
    State(state): State<Arc<AppState>>,
    ctx: ProjectContext,
    Path((_project_id, command_id)): Path<(String, Uuid)>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    // Auth: confirm the command exists and belongs to this project.
    let client = state
        .db
        .get()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db pool".into()))?;
    let row = db_cmd::fetch_by_id(&client, &command_id)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "db query".into()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("command {command_id} not found"),
            )
        })?;
    drop(client);
    ensure_agent_in_project(&state, &row.agent_id, &ctx.project_id).await?;

    // Poll state captured by the async-stream closure.
    struct PollState {
        state: Arc<AppState>,
        command_id: Uuid,
        last_log_id: i64,
        finished: bool,
        emitted_done: bool,
    }

    let initial = PollState {
        state: state.clone(),
        command_id,
        last_log_id: 0,
        finished: false,
        emitted_done: false,
    };

    // `stream::unfold` yields (Event, NextState) on each tick.
    let sse_stream = stream::unfold(initial, move |mut s| async move {
        if s.emitted_done {
            return None;
        }

        // 1. Poll new logs.
        let logs: Vec<LogEvent> = match s.state.logs_db.get().await {
            Ok(client) => {
                let cid = s.command_id.to_string();
                match client
                    .query(
                        "SELECT log_id, ts, level, message, fields \
                         FROM service_log \
                         WHERE service = 'agent-command' \
                           AND fields->>'command_id' = $1 \
                           AND log_id > $2 \
                         ORDER BY log_id ASC \
                         LIMIT 500",
                        &[&cid, &s.last_log_id],
                    )
                    .await
                {
                    Ok(rows) => rows
                        .into_iter()
                        .map(|r| LogEvent {
                            log_id: r.get("log_id"),
                            ts: r.get("ts"),
                            level: r.get("level"),
                            message: r.get("message"),
                            fields: r.get("fields"),
                        })
                        .collect(),
                    Err(_) => Vec::new(),
                }
            }
            Err(_) => Vec::new(),
        };

        if let Some(last) = logs.last() {
            s.last_log_id = last.log_id;
        }

        // 2. Check terminal state on the command row (if not already known).
        if !s.finished {
            if let Ok(client) = s.state.db.get().await {
                if let Ok(Some(row)) = db_cmd::fetch_by_id(&client, &s.command_id).await {
                    if row.finished_at.is_some() {
                        s.finished = true;
                    }
                }
            }
        }

        // 3. Build the next event.
        //    - If we have log rows, emit them one at a time (by re-entering the
        //      unfold with logs buffered). Simpler: emit a batched `log` event
        //      containing all new rows as a JSON array. The frontend demux is
        //      trivial and it avoids a nested stream.
        if !logs.is_empty() {
            let payload = match serde_json::to_string(&logs) {
                Ok(p) => p,
                Err(_) => return None,
            };
            let ev = Event::default().event("log").data(payload);
            return Some((Ok::<_, Infallible>(ev), s));
        }

        // No new logs. If command finished and we've drained logs, emit done.
        if s.finished {
            let (status, err) = {
                // Fetch latest row one more time so we report the final status.
                match s.state.db.get().await {
                    Ok(client) => match db_cmd::fetch_by_id(&client, &s.command_id).await {
                        Ok(Some(r)) => (r.status, r.error_message),
                        _ => ("unknown".to_string(), None),
                    },
                    _ => ("unknown".to_string(), None),
                }
            };
            let done = DoneEvent {
                command_id: s.command_id,
                status,
                error_message: err,
            };
            s.emitted_done = true;
            let payload = serde_json::to_string(&done).unwrap_or_else(|_| "{}".into());
            let ev = Event::default().event("done").data(payload);
            return Some((Ok::<_, Infallible>(ev), s));
        }

        // Idle tick — sleep 500ms and emit a comment (heartbeat) so the
        // connection stays alive even if the client/proxy has short read
        // timeouts. KeepAlive also handles this but a comment here means we
        // never starve the unfold driver.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let ev = Event::default().comment("tick");
        Some((Ok::<_, Infallible>(ev), s))
    });

    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

// ─── Router ──────────────────────────────────────────────────────────────────

pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/agents/{agent_id}/commands", post(dispatch))
        .route("/commands/{command_id}", get(fetch))
        .route("/commands/{command_id}/stream", get(stream))
        .with_state(state)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    //! Handler-level tests.
    //!
    //! We follow the existing convention in this crate (see `agent_dispatch`
    //! tests): DB-backed scenarios are gated on `DASHBOARD_DB_URL` and marked
    //! `#[ignore]` so CI without a live Postgres still passes. Pure logic
    //! tests run unconditionally.

    use super::*;

    /// DispatchBody default for `timeout_secs` lands on 60s in the handler.
    #[test]
    fn dispatch_body_defaults() {
        let body: DispatchBody = serde_json::from_str(r#"{"verb":"health"}"#).unwrap();
        assert_eq!(body.verb, "health");
        assert!(body.timeout_secs.is_none());
        assert!(body.config_id.is_none());
        // Handler resolves None → 60.
        assert_eq!(body.timeout_secs.unwrap_or(60), 60);
    }

    #[test]
    fn dispatch_response_serialises_expected_shape() {
        let cid = Uuid::new_v4();
        let aid = Uuid::new_v4();
        let resp = DispatchResponse {
            command_id: cid,
            agent_id: aid,
            verb: "health".into(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["command_id"], serde_json::json!(cid.to_string()));
        assert_eq!(json["agent_id"], serde_json::json!(aid.to_string()));
        assert_eq!(json["verb"], "health");
    }

    #[test]
    fn done_event_serialises_expected_shape() {
        let cid = Uuid::new_v4();
        let done = DoneEvent {
            command_id: cid,
            status: "ok".into(),
            error_message: None,
        };
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["error_message"].is_null());
    }

    // ── DB-backed integration tests ──────────────────────────────────────
    //
    // These run the SQL path used by the handlers against a live Postgres.
    // They require V033 to be applied. They are `#[ignore]` by default.

    async fn connect_test_db_with_project_and_agent(
    ) -> Option<(tokio_postgres::Client, String, Uuid)> {
        use tokio_postgres::NoTls;
        let url = std::env::var("DASHBOARD_DB_URL").ok()?;
        let (client, conn) = tokio_postgres::connect(&url, NoTls).await.ok()?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let project_id: String = client
            .query_opt(
                "SELECT project_id FROM project ORDER BY created_at LIMIT 1",
                &[],
            )
            .await
            .ok()
            .flatten()?
            .get(0);
        let agent_id = Uuid::new_v4();
        let api_key = format!("test-{}", agent_id);
        let name = format!("agent-api-test-{}", &agent_id.to_string()[..8]);
        client
            .execute(
                "INSERT INTO agent (agent_id, project_id, name, api_key, status) \
                 VALUES ($1, $2, $3, $4, 'offline')",
                &[&agent_id, &project_id, &name, &api_key],
            )
            .await
            .ok()?;
        Some((client, project_id, agent_id))
    }

    /// The `SELECT 1 FROM agent WHERE agent_id=$1 AND project_id=$2` check
    /// in `ensure_agent_in_project` is the heart of the project-membership
    /// guard. Verify it rejects cross-project lookups.
    #[tokio::test]
    #[ignore]
    async fn agent_project_scoping_sql_rejects_wrong_project() -> anyhow::Result<()> {
        let Some((client, project_id, agent_id)) = connect_test_db_with_project_and_agent().await
        else {
            eprintln!("SKIP: DASHBOARD_DB_URL not set");
            return Ok(());
        };

        // Correct project → hit.
        let hit = client
            .query_opt(
                "SELECT 1 FROM agent WHERE agent_id = $1 AND project_id = $2",
                &[&agent_id, &project_id],
            )
            .await?;
        assert!(hit.is_some(), "agent must resolve in its own project");

        // Wrong project → miss.
        let miss = client
            .query_opt(
                "SELECT 1 FROM agent WHERE agent_id = $1 AND project_id = $2",
                &[&agent_id, &"not-a-real-project".to_string()],
            )
            .await?;
        assert!(
            miss.is_none(),
            "agent must NOT resolve in a foreign project"
        );
        Ok(())
    }

    /// Round-trip: insert pending → fetch_by_id → mark_finished → fetch_by_id.
    /// This exercises the exact SQL the `fetch` handler runs.
    #[tokio::test]
    #[ignore]
    async fn fetch_reports_pending_then_ok_after_result() -> anyhow::Result<()> {
        let Some((client, _project_id, agent_id)) = connect_test_db_with_project_and_agent().await
        else {
            eprintln!("SKIP: DASHBOARD_DB_URL not set");
            return Ok(());
        };
        let command_id = Uuid::new_v4();
        db_cmd::insert_pending(
            &client,
            &command_id,
            &agent_id,
            None,
            "health",
            &serde_json::json!({}),
            None,
        )
        .await?;
        let row1 = db_cmd::fetch_by_id(&client, &command_id).await?.unwrap();
        assert_eq!(row1.status, "pending");
        assert!(row1.finished_at.is_none());

        let payload = serde_json::json!({"ok": true});
        db_cmd::mark_finished(&client, &command_id, "ok", Some(&payload), None).await?;
        let row2 = db_cmd::fetch_by_id(&client, &command_id).await?.unwrap();
        assert_eq!(row2.status, "ok");
        assert!(row2.finished_at.is_some());
        assert_eq!(row2.result, Some(payload));
        Ok(())
    }

    /// The SSE stream's core query — poll service_log rows tagged with a
    /// specific command_id after a given log_id watermark.
    #[tokio::test]
    #[ignore]
    async fn sse_log_poll_query_returns_new_rows_by_watermark() -> anyhow::Result<()> {
        use tokio_postgres::NoTls;
        let Some(url) = std::env::var("DASHBOARD_LOGS_DB_URL")
            .ok()
            .or_else(|| std::env::var("DASHBOARD_DB_URL").ok())
        else {
            eprintln!("SKIP: no logs DB url set");
            return Ok(());
        };
        let (client, conn) = tokio_postgres::connect(&url, NoTls).await?;
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let command_id = Uuid::new_v4().to_string();
        let fields = serde_json::json!({"command_id": command_id, "stream": "stdout"});
        client
            .execute(
                "INSERT INTO service_log (service, level, message, fields) \
                 VALUES ('agent-command', 3, 'hello', $1)",
                &[&fields],
            )
            .await?;

        let rows = client
            .query(
                "SELECT log_id FROM service_log \
                 WHERE service = 'agent-command' \
                   AND fields->>'command_id' = $1 \
                   AND log_id > $2 \
                 ORDER BY log_id ASC LIMIT 500",
                &[&command_id, &0i64],
            )
            .await?;
        assert_eq!(rows.len(), 1, "new row must be returned when watermark=0");

        let last: i64 = rows[0].get("log_id");
        let rows2 = client
            .query(
                "SELECT log_id FROM service_log \
                 WHERE service = 'agent-command' \
                   AND fields->>'command_id' = $1 \
                   AND log_id > $2 \
                 ORDER BY log_id ASC LIMIT 500",
                &[&command_id, &last],
            )
            .await?;
        assert!(
            rows2.is_empty(),
            "watermark advance must exclude already-seen rows"
        );
        Ok(())
    }
}
