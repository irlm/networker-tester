use crate::types::ResourceMetrics;
use anyhow::Result;

/// Collect resource-usage metrics (CPU, memory, FDs) for a running process.
///
/// Stub — will be implemented in Phase 3.
pub async fn collect_metrics(_pid: u32) -> Result<ResourceMetrics> {
    tracing::warn!("collect_metrics is a stub");
    Ok(ResourceMetrics::default())
}
