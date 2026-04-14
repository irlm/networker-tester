//! WebSocket message types exchanged between control plane and agents/browsers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────────────────
// Job configuration (subset of ResolvedConfig relevant for remote execution)
// ─────────────────────────────────────────────────────────────────────────────

/// Test job configuration dispatched to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobConfig {
    pub target: String,
    pub modes: Vec<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub tls_profile_url: Option<String>,
    #[serde(default)]
    pub tls_profile_ip: Option<String>,
    #[serde(default)]
    pub tls_profile_sni: Option<String>,
    #[serde(default)]
    pub tls_profile_target_kind: Option<String>,
    #[serde(default = "default_runs")]
    pub runs: u32,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub payload_sizes: Vec<String>,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub dns_enabled: bool,
    #[serde(default)]
    pub ipv4_only: bool,
    #[serde(default)]
    pub ipv6_only: bool,
    #[serde(default)]
    pub connection_reuse: bool,
    #[serde(default)]
    pub retries: u32,
    #[serde(default)]
    pub page_preset: Option<String>,
    #[serde(default)]
    pub page_assets: Option<u32>,
    #[serde(default)]
    pub page_asset_size: Option<String>,
    #[serde(default)]
    pub udp_port: Option<u16>,
    #[serde(default)]
    pub udp_throughput_port: Option<u16>,
    /// Packet capture mode.
    #[serde(default)]
    pub capture_mode: Option<networker_tester::cli::PacketCaptureMode>,
}

fn default_runs() -> u32 {
    3
}
fn default_concurrency() -> usize {
    1
}
fn default_timeout() -> u64 {
    30
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent → Control Plane messages
// ─────────────────────────────────────────────────────────────────────────────

/// Messages sent from agent to control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    /// Periodic heartbeat with agent load info.
    Heartbeat {
        #[serde(default)]
        load: Option<f64>,
        version: Option<String>,
    },
    /// Acknowledge receipt of a job assignment.
    JobAck { job_id: Uuid },
    /// A single probe attempt completed (streamed as it happens).
    AttemptResult {
        job_id: Uuid,
        attempt: Box<networker_tester::metrics::RequestAttempt>,
    },
    /// Full test run completed.
    JobComplete {
        job_id: Uuid,
        run: Box<networker_tester::metrics::TestRun>,
    },
    /// TLS profile job completed.
    TlsProfileComplete {
        job_id: Uuid,
        profile: Box<networker_tester::tls_profile::TlsEndpointProfile>,
    },
    /// Job failed with an error.
    JobError { job_id: Uuid, message: String },
    /// Log line from tester execution (streamed to browser for live logs).
    JobLog {
        job_id: Uuid,
        line: String,
        level: String,
    },
    /// Streamed log line from a running command.
    CommandLog(AgentCommandLog),
    /// Final result of a command execution.
    CommandResult(AgentCommandResult),
}

// ─────────────────────────────────────────────────────────────────────────────
// Control Plane → Agent messages
// ─────────────────────────────────────────────────────────────────────────────

/// Messages sent from control plane to agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Assign a test job to the agent.
    JobAssign {
        job_id: Uuid,
        config: Box<JobConfig>,
    },
    /// Request the agent to cancel a running job.
    JobCancel { job_id: Uuid },
    /// Acknowledge agent registration / reconnection.
    Welcome { agent_id: Uuid, agent_name: String },
    /// Dispatch a typed command envelope to the agent.
    Command(AgentCommand),
    /// Cancel an in-flight command.
    Cancel(AgentCommandCancel),
}

// ─────────────────────────────────────────────────────────────────────────────
// Control Plane → Browser messages (dashboard live updates)
// ─────────────────────────────────────────────────────────────────────────────

/// Messages sent to browser WebSocket subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardEvent {
    /// A job's status changed.
    JobUpdate {
        job_id: Uuid,
        status: String,
        agent_id: Option<Uuid>,
        started_at: Option<DateTime<Utc>>,
        finished_at: Option<DateTime<Utc>>,
    },
    /// A probe attempt completed (live streaming).
    AttemptResult {
        job_id: Uuid,
        attempt: Box<networker_tester::metrics::RequestAttempt>,
    },
    /// A job completed with the full test run.
    JobComplete {
        job_id: Uuid,
        run_id: Uuid,
        success_count: usize,
        failure_count: usize,
    },
    /// An agent's status changed.
    AgentStatus {
        agent_id: Uuid,
        status: String,
        last_heartbeat: Option<DateTime<Utc>>,
    },
    /// A tester log line (streamed from job execution).
    JobLog {
        job_id: Uuid,
        line: String,
        level: String,
    },
    /// A deployment log line (streamed from install.sh).
    DeployLog {
        deployment_id: Uuid,
        line: String,
        stream: String,
    },
    /// A deployment completed or failed.
    DeployComplete {
        deployment_id: Uuid,
        status: String,
        endpoint_ips: Vec<String>,
    },
    /// A benchmark config update (status, log, result, complete).
    BenchmarkUpdate {
        config_id: Uuid,
        event_type: String,
        payload: serde_json::Value,
    },
    /// Benchmark regression detected after completion.
    BenchmarkRegression {
        config_id: Uuid,
        config_name: String,
        regression_count: usize,
        regressions: serde_json::Value,
    },
}

// ─────────────────────────────────────────────────────────────────────────────
// Browser → Control Plane messages (commands)
// ─────────────────────────────────────────────────────────────────────────────

/// Messages sent from browser to control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserCommand {
    /// Subscribe to live updates for a specific job.
    SubscribeJob { job_id: Uuid },
    /// Unsubscribe from a job's updates.
    UnsubscribeJob { job_id: Uuid },
    /// Subscribe to all dashboard events (agent status, new jobs).
    SubscribeAll,
}

// ─────────────────────────────────────────────────────────────────────────────
// Typed command envelope (dashboard → agent orchestration)
// ─────────────────────────────────────────────────────────────────────────────

/// A command dispatched from the dashboard to an agent.
///
/// The `token` field carries a short-lived JWT that the agent validates
/// before executing the command. It is opaque at this layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommand {
    pub command_id: Uuid,
    #[serde(default)]
    pub config_id: Option<Uuid>,
    pub token: String,
    pub verb: String,
    #[serde(default)]
    pub args: serde_json::Value,
    pub timeout_secs: u64,
}

/// Stream identifier for command log lines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
}

/// A log line emitted while a command executes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommandLog {
    pub command_id: Uuid,
    pub stream: LogStream,
    pub line: String,
}

/// Terminal status for a command execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CommandStatus {
    Ok,
    Error,
    Timeout,
    Cancelled,
}

/// Result of a command execution reported back to the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommandResult {
    pub command_id: Uuid,
    pub status: CommandStatus,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Request to cancel an in-flight command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCommandCancel {
    pub command_id: Uuid,
}

#[cfg(test)]
mod command_tests {
    use super::*;

    fn sample_command() -> AgentCommand {
        AgentCommand {
            command_id: Uuid::new_v4(),
            config_id: Some(Uuid::new_v4()),
            token: "opaque.jwt.token".to_string(),
            verb: "run_benchmark".to_string(),
            args: serde_json::json!({ "target": "example.com", "runs": 3 }),
            timeout_secs: 120,
        }
    }

    #[test]
    fn command_envelope_round_trips_as_json() {
        let cmd = sample_command();
        let json = serde_json::to_string(&cmd).unwrap();
        let back: AgentCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(back.command_id, cmd.command_id);
        assert_eq!(back.config_id, cmd.config_id);
        assert_eq!(back.token, cmd.token);
        assert_eq!(back.verb, cmd.verb);
        assert_eq!(back.args, cmd.args);
        assert_eq!(back.timeout_secs, cmd.timeout_secs);
    }

    #[test]
    fn command_envelope_config_id_optional() {
        // Missing config_id should deserialize to None.
        let json = r#"{
            "command_id": "00000000-0000-0000-0000-000000000001",
            "token": "t",
            "verb": "noop",
            "timeout_secs": 5
        }"#;
        let cmd: AgentCommand = serde_json::from_str(json).unwrap();
        assert!(cmd.config_id.is_none());
        assert_eq!(cmd.args, serde_json::Value::Null);
    }

    #[test]
    fn command_result_handles_error_variant() {
        let result = AgentCommandResult {
            command_id: Uuid::new_v4(),
            status: CommandStatus::Error,
            result: None,
            error: Some("something exploded".to_string()),
            duration_ms: 42,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"status\":\"error\""));
        let back: AgentCommandResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, CommandStatus::Error);
        assert_eq!(back.error.as_deref(), Some("something exploded"));
        assert_eq!(back.duration_ms, 42);
        assert!(back.result.is_none());
    }

    #[test]
    fn command_status_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&CommandStatus::Ok).unwrap(), "\"ok\"");
        assert_eq!(
            serde_json::to_string(&CommandStatus::Timeout).unwrap(),
            "\"timeout\""
        );
        assert_eq!(
            serde_json::to_string(&CommandStatus::Cancelled).unwrap(),
            "\"cancelled\""
        );
    }

    #[test]
    fn command_log_enum_serializes_lowercase() {
        let log = AgentCommandLog {
            command_id: Uuid::new_v4(),
            stream: LogStream::Stdout,
            line: "hello".to_string(),
        };
        let json = serde_json::to_string(&log).unwrap();
        assert!(json.contains("\"stream\":\"stdout\""));

        let err_log = AgentCommandLog {
            command_id: Uuid::new_v4(),
            stream: LogStream::Stderr,
            line: "boom".to_string(),
        };
        let json_err = serde_json::to_string(&err_log).unwrap();
        assert!(json_err.contains("\"stream\":\"stderr\""));

        let back: AgentCommandLog = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stream, LogStream::Stdout);
        assert_eq!(back.line, "hello");
    }

    #[test]
    fn control_message_command_variant_round_trips() {
        let cmd = sample_command();
        let msg = ControlMessage::Command(cmd.clone());
        let json = serde_json::to_string(&msg).unwrap();
        // snake_case external tag = "command"
        assert!(json.contains("\"type\":\"command\""));
        let back: ControlMessage = serde_json::from_str(&json).unwrap();
        match back {
            ControlMessage::Command(c) => {
                assert_eq!(c.command_id, cmd.command_id);
                assert_eq!(c.verb, cmd.verb);
            }
            other => panic!("expected Command variant, got {:?}", other),
        }
    }

    #[test]
    fn control_message_cancel_variant_round_trips() {
        let cancel = AgentCommandCancel {
            command_id: Uuid::new_v4(),
        };
        let msg = ControlMessage::Cancel(cancel.clone());
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"cancel\""));
        let back: ControlMessage = serde_json::from_str(&json).unwrap();
        match back {
            ControlMessage::Cancel(c) => assert_eq!(c.command_id, cancel.command_id),
            other => panic!("expected Cancel variant, got {:?}", other),
        }
    }

    #[test]
    fn control_message_existing_variants_unchanged() {
        // Ensure new variants did not break existing Welcome serialization.
        let msg = ControlMessage::Welcome {
            agent_id: Uuid::nil(),
            agent_name: "a".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"welcome\""));
    }

    #[test]
    fn agent_message_command_log_and_result_round_trip() {
        let log_msg = AgentMessage::CommandLog(AgentCommandLog {
            command_id: Uuid::new_v4(),
            stream: LogStream::Stderr,
            line: "warn".to_string(),
        });
        let json = serde_json::to_string(&log_msg).unwrap();
        assert!(json.contains("\"type\":\"command_log\""));
        let back: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AgentMessage::CommandLog(_)));

        let result_msg = AgentMessage::CommandResult(AgentCommandResult {
            command_id: Uuid::new_v4(),
            status: CommandStatus::Ok,
            result: Some(serde_json::json!({ "ok": true })),
            error: None,
            duration_ms: 10,
        });
        let json = serde_json::to_string(&result_msg).unwrap();
        assert!(json.contains("\"type\":\"command_result\""));
        let back: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AgentMessage::CommandResult(_)));
    }
}
