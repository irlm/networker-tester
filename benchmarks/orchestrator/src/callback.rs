use anyhow::{Context, Result};
use reqwest::Client;
use serde::Serialize;

/// HTTP callback client for reporting orchestrator progress to the dashboard.
pub struct CallbackClient {
    base_url: String,
    token: String,
    config_id: String,
    client: Client,
}

#[derive(Serialize)]
struct StatusPayload {
    config_id: String,
    testbed_id: String,
    status: String,
    current_language: String,
    language_index: u32,
    language_total: u32,
    message: String,
}

#[derive(Serialize)]
struct LogPayload {
    config_id: String,
    testbed_id: String,
    lines: Vec<String>,
}

#[derive(Serialize)]
struct ResultPayload {
    config_id: String,
    testbed_id: String,
    language: String,
    artifact: serde_json::Value,
}

#[derive(Serialize)]
struct CompletePayload {
    config_id: String,
    status: String,
    duration_seconds: Option<i64>,
    error_message: Option<String>,
    teardown_status: Option<String>,
}

#[derive(Serialize)]
struct HeartbeatPayload {
    config_id: String,
}

impl CallbackClient {
    /// Create a new callback client.
    pub fn new(base_url: &str, token: &str, config_id: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            config_id: config_id.to_string(),
            client: Client::new(),
        }
    }

    /// Report testbed status to the dashboard.
    pub async fn status(
        &self,
        testbed_id: &str,
        status: &str,
        current_language: &str,
        language_index: u32,
        language_total: u32,
        message: &str,
    ) -> Result<()> {
        let payload = StatusPayload {
            config_id: self.config_id.clone(),
            testbed_id: testbed_id.to_string(),
            status: status.to_string(),
            current_language: current_language.to_string(),
            language_index,
            language_total,
            message: message.to_string(),
        };
        self.post("status", &payload).await
    }

    /// Send log lines for a testbed.
    pub async fn log(&self, testbed_id: &str, lines: Vec<String>) -> Result<()> {
        let payload = LogPayload {
            config_id: self.config_id.clone(),
            testbed_id: testbed_id.to_string(),
            lines,
        };
        self.post("log", &payload).await
    }

    /// Submit a benchmark result artifact for a testbed/language.
    pub async fn result(
        &self,
        testbed_id: &str,
        language: &str,
        artifact: serde_json::Value,
    ) -> Result<()> {
        let payload = ResultPayload {
            config_id: self.config_id.clone(),
            testbed_id: testbed_id.to_string(),
            language: language.to_string(),
            artifact,
        };
        self.post("result", &payload).await
    }

    /// Report that the orchestrator run is complete.
    pub async fn complete(
        &self,
        status: &str,
        duration_secs: f64,
        error_message: Option<String>,
    ) -> Result<()> {
        let payload = CompletePayload {
            config_id: self.config_id.clone(),
            status: status.to_string(),
            duration_seconds: Some(duration_secs as i64),
            error_message,
            teardown_status: None,
        };
        self.post("complete", &payload).await
    }

    /// Send a heartbeat to the dashboard.
    pub async fn heartbeat(&self) -> Result<()> {
        let payload = HeartbeatPayload {
            config_id: self.config_id.clone(),
        };
        self.post("heartbeat", &payload).await
    }

    /// Check whether this config has been cancelled by the dashboard.
    pub async fn check_cancelled(&self) -> Result<bool> {
        let url = format!(
            "{}/api/benchmarks/callback/cancelled/{}",
            self.base_url, self.config_id
        );
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("callback cancelled check failed")?;

        if !resp.status().is_success() {
            anyhow::bail!("callback cancelled check returned HTTP {}", resp.status());
        }

        #[derive(serde::Deserialize)]
        struct CancelledResponse {
            cancelled: bool,
        }

        let body: CancelledResponse = resp
            .json()
            .await
            .context("failed to parse cancelled response")?;
        Ok(body.cancelled)
    }

    /// POST a JSON payload to a callback endpoint.
    async fn post<T: Serialize>(&self, endpoint: &str, payload: &T) -> Result<()> {
        let url = format!("{}/api/benchmarks/callback/{}", self.base_url, endpoint);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .json(payload)
            .send()
            .await
            .with_context(|| format!("callback POST to {endpoint} failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("callback {endpoint} returned HTTP {status}: {body}");
        }
        Ok(())
    }
}
