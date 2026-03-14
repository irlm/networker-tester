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
        attempt: networker_tester::metrics::RequestAttempt,
    },
    /// Full test run completed.
    JobComplete {
        job_id: Uuid,
        run: networker_tester::metrics::TestRun,
    },
    /// Job failed with an error.
    JobError { job_id: Uuid, message: String },
}

// ─────────────────────────────────────────────────────────────────────────────
// Control Plane → Agent messages
// ─────────────────────────────────────────────────────────────────────────────

/// Messages sent from control plane to agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Assign a test job to the agent.
    JobAssign { job_id: Uuid, config: JobConfig },
    /// Request the agent to cancel a running job.
    JobCancel { job_id: Uuid },
    /// Acknowledge agent registration / reconnection.
    Welcome { agent_id: Uuid, agent_name: String },
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
        attempt: networker_tester::metrics::RequestAttempt,
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
