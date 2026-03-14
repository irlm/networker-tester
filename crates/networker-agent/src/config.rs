/// Agent daemon configuration loaded from environment variables.
pub struct AgentConfig {
    /// WebSocket URL of the dashboard control plane (e.g., ws://localhost:3000/ws/agent).
    pub dashboard_url: String,
    /// API key for authenticating with the control plane.
    pub api_key: String,
}

impl AgentConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let dashboard_url = std::env::var("AGENT_DASHBOARD_URL")
            .unwrap_or_else(|_| "ws://localhost:3000/ws/agent".into());
        let api_key = std::env::var("AGENT_API_KEY")
            .map_err(|_| anyhow::anyhow!("AGENT_API_KEY environment variable is required"))?;
        Ok(Self {
            dashboard_url,
            api_key,
        })
    }
}
