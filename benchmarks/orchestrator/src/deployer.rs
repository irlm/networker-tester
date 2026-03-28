use crate::config::LanguageEntry;
use crate::provisioner::VmInfo;
use anyhow::Result;

/// Deploy an API implementation to a target VM (or localhost).
///
/// Stub — will be implemented in Phase 2.
pub async fn deploy_api(_vm: &VmInfo, _lang: &LanguageEntry) -> Result<()> {
    tracing::warn!("deploy_api is a stub");
    Ok(())
}

/// Stop a previously deployed API.
///
/// Stub — will be implemented in Phase 2.
pub async fn stop_api(_vm: &VmInfo, _lang: &LanguageEntry) -> Result<()> {
    tracing::warn!("stop_api is a stub");
    Ok(())
}
