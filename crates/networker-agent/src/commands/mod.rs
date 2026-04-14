//! Agent command handlers.
//!
//! Each verb is implemented as a free function in its own submodule and
//! returns a `serde_json::Value` on success. The [`run_command`] dispatcher
//! routes by the `verb` string on the incoming [`AgentCommand`] and wraps
//! the result into an [`AgentCommandResult`] envelope.
//!
//! Token validation is intentionally NOT performed here — the dispatcher
//! trusts the token field. WebSocket channel auth gates the connection
//! itself, and a later task in the command-based orchestration plan will
//! add per-command JWT validation.

use networker_common::messages::{
    AgentCommand, AgentCommandLog, AgentCommandResult, CommandStatus,
};
use std::time::Instant;
use tokio::sync::mpsc;

pub mod health;

/// Run an incoming [`AgentCommand`] and stream logs back via `log_tx`.
///
/// Returns the terminal result envelope; the caller is responsible for
/// sending it back to the dashboard.
pub async fn run_command(
    cmd: AgentCommand,
    log_tx: mpsc::Sender<AgentCommandLog>,
) -> AgentCommandResult {
    let start = Instant::now();
    let command_id = cmd.command_id;

    let result: anyhow::Result<serde_json::Value> = match cmd.verb.as_str() {
        "health" => health::run(cmd.args, &log_tx).await,
        other => Err(anyhow::anyhow!("unknown verb: {other}")),
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(v) => AgentCommandResult {
            command_id,
            status: CommandStatus::Ok,
            result: Some(v),
            error: None,
            duration_ms,
        },
        Err(e) => AgentCommandResult {
            command_id,
            status: CommandStatus::Error,
            result: None,
            error: Some(format!("{e:#}")),
            duration_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn cmd(verb: &str) -> AgentCommand {
        AgentCommand {
            command_id: Uuid::new_v4(),
            config_id: None,
            token: "ignored".into(),
            verb: verb.into(),
            args: serde_json::json!({}),
            timeout_secs: 30,
        }
    }

    #[tokio::test]
    async fn unknown_verb_returns_error_status() {
        let (tx, _rx) = mpsc::channel(8);
        let result = run_command(cmd("no_such_verb"), tx).await;
        assert_eq!(result.status, CommandStatus::Error);
        assert!(result.result.is_none());
        assert!(result
            .error
            .expect("error message should be set")
            .contains("unknown verb"));
    }

    #[tokio::test]
    async fn health_returns_ok_with_version_and_os() {
        let (tx, _rx) = mpsc::channel(8);
        let c = cmd("health");
        let expected_id = c.command_id;
        let result = run_command(c, tx).await;

        assert_eq!(result.command_id, expected_id);
        assert_eq!(result.status, CommandStatus::Ok);
        let r = result.result.expect("health should return a JSON body");
        assert!(r.get("version").is_some(), "missing version: {r}");
        assert!(r.get("os").is_some(), "missing os: {r}");
        assert!(r.get("arch").is_some(), "missing arch: {r}");
        assert!(r.get("uptime_secs").is_some(), "missing uptime_secs: {r}");
    }
}
