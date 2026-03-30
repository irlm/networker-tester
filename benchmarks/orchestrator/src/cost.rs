use crate::config::BenchmarkConfig;

/// Estimate the cloud cost (USD) for running the full benchmark suite.
///
/// Stub — will be implemented in Phase 2.
pub fn estimate_cost(config: &BenchmarkConfig) -> f64 {
    let num_cases = config.test_matrix().len() as f64;
    // Rough placeholder: ~$0.05 per test case (2-min VM at $1.50/hr)
    let cost = num_cases * 0.05;
    tracing::info!(
        "Estimated cost for {} test cases: ${:.2}",
        num_cases as u64,
        cost
    );
    cost
}
