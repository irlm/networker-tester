use std::time::Duration;

/// Fire-and-forget HTTP progress reporter for benchmark orchestrator integration.
pub struct ProgressReporter {
    client: reqwest::Client,
    url: String,
    token: String,
    config_id: String,
    testbed_id: Option<String>,
    language: String,
    interval: u32,
}

impl ProgressReporter {
    pub fn new(
        url: String,
        token: String,
        config_id: String,
        testbed_id: Option<String>,
        language: String,
        interval: u32,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap_or_default(),
            url,
            token,
            config_id,
            testbed_id,
            language,
            interval,
        }
    }

    pub async fn report(
        &self,
        mode: &str,
        request_index: u32,
        total_requests: u32,
        latency_ms: f64,
        success: bool,
    ) {
        // Only POST at the configured interval
        if self.interval > 1
            && !request_index.is_multiple_of(self.interval)
            && request_index < total_requests
        {
            return;
        }
        let payload = serde_json::json!({
            "config_id": self.config_id,
            "testbed_id": self.testbed_id,
            "language": self.language,
            "mode": mode,
            "request_index": request_index,
            "total_requests": total_requests,
            "latency_ms": latency_ms,
            "success": success,
        });
        // Fire and forget — don't block the benchmark
        let _ = self
            .client
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&payload)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
    }
}
