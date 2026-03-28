use crate::config::TestCase;
use crate::types::NetworkMetrics;
use anyhow::Result;

/// Execute the HTTP load test for a single test case and return network metrics.
///
/// Stub — will be implemented in Phase 3.
pub async fn run_benchmark(
    _target_url: &str,
    case: &TestCase,
    _total_requests: u64,
    _warmup_requests: u64,
) -> Result<NetworkMetrics> {
    tracing::warn!(
        "run_benchmark is a stub — {} concurrency={}",
        case.language.name,
        case.concurrency
    );
    Ok(NetworkMetrics::default())
}
