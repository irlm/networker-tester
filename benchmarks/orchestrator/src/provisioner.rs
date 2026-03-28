use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Information about a provisioned VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub ip: String,
    pub provider: String,
    pub instance_id: String,
    pub ssh_user: String,
}

/// Provision a VM for running a benchmark target.
///
/// Stub — will be implemented in Phase 2.
pub async fn provision_vm(provider: &str, _region: &str) -> Result<VmInfo> {
    tracing::warn!("provision_vm is a stub — returning placeholder for {}", provider);
    Ok(VmInfo {
        ip: "127.0.0.1".into(),
        provider: provider.into(),
        instance_id: "stub-instance".into(),
        ssh_user: "benchuser".into(),
    })
}

/// Tear down a previously provisioned VM.
///
/// Stub — will be implemented in Phase 2.
pub async fn destroy_vm(_vm: &VmInfo) -> Result<()> {
    tracing::warn!("destroy_vm is a stub");
    Ok(())
}
