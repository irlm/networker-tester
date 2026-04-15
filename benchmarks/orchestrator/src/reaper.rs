//! Lightweight cloud orphan reaper for the orchestrator crate.
//!
//! Mirrors the dashboard's `cloud_orphan_reaper` service but stands alone so
//! the orchestrator doesn't need to pull in the whole dashboard lib. Targets
//! only Azure resources whose names start with `ab-` inside the orchestrator's
//! own resource group.
//!
//! Soft-fail everywhere — called opportunistically before `provision_vm` to
//! keep the `ab-*` fleet from blocking itself on Azure public-IP quota.

use std::time::Duration;
use tokio::time::timeout;

/// Prefix that identifies orchestrator-owned resources.
const OWNED_PREFIX: &str = "ab-";

/// How long the whole cleanup loop may take before we give up.
const REAP_BUDGET: Duration = Duration::from_secs(45);

/// Delete every orphaned `ab-*` VM / NIC / disk / public IP in `resource_group`.
///
/// "Orphan" here means "name starts with `ab-`". The orchestrator treats its
/// resource group as its own — anything not following the convention is
/// left alone. Errors are logged and swallowed.
pub async fn reap_orphans_best_effort(resource_group: &str) {
    let rg = resource_group.to_string();
    let fut = async move {
        // List everything in parallel, then delete in dependency order.
        let rg_s = rg.as_str();
        let vm_args = ["vm", "list", "--resource-group", rg_s];
        let nic_args = ["network", "nic", "list", "--resource-group", rg_s];
        let ip_args = ["network", "public-ip", "list", "--resource-group", rg_s];
        let disk_args = ["disk", "list", "--resource-group", rg_s];
        let (vms, nics, ips, disks) = tokio::join!(
            az_list_ids(&vm_args),
            az_list_ids(&nic_args),
            az_list_ids(&ip_args),
            az_list_ids(&disk_args),
        );
        let vms = filter_owned(vms);
        let nics = filter_owned(nics);
        let ips = filter_owned(ips);
        let disks = filter_owned(disks);

        let total = vms.len() + nics.len() + ips.len() + disks.len();
        if total == 0 {
            tracing::debug!(resource_group = %rg, "reaper: nothing to clean");
            return;
        }

        tracing::info!(
            resource_group = %rg,
            vms = vms.len(),
            nics = nics.len(),
            ips = ips.len(),
            disks = disks.len(),
            "reaper: cleaning orphaned ab-* resources"
        );

        // VM → NIC → disk → IP is dependency-safe.
        for (id, _name) in &vms {
            let _ = az_cmd(&["vm", "delete", "--ids", id, "--yes"]).await;
        }
        for (id, _name) in &nics {
            let _ = az_cmd(&["network", "nic", "delete", "--ids", id]).await;
        }
        for (id, _name) in &disks {
            let _ = az_cmd(&["disk", "delete", "--ids", id, "--yes"]).await;
        }
        for (id, _name) in &ips {
            let _ = az_cmd(&["network", "public-ip", "delete", "--ids", id]).await;
        }
    };

    if timeout(REAP_BUDGET, fut).await.is_err() {
        tracing::warn!("reaper: timed out (soft-fail)");
    }
}

fn filter_owned(list: Vec<(String, String)>) -> Vec<(String, String)> {
    list.into_iter()
        .filter(|(_id, name)| name.to_ascii_lowercase().starts_with(OWNED_PREFIX))
        .collect()
}

async fn az_list_ids(args: &[&str]) -> Vec<(String, String)> {
    let output = tokio::process::Command::new("az")
        .args(args)
        .arg("--query")
        .arg("[].{id:id,name:name}")
        .arg("--output")
        .arg("json")
        .output()
        .await;
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(error = %e, "reaper: az list failed");
            return Vec::new();
        }
    };
    if !output.status.success() {
        tracing::warn!(
            stderr = %String::from_utf8_lossy(&output.stderr),
            "reaper: az list non-zero"
        );
        return Vec::new();
    }
    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    parsed
        .into_iter()
        .filter_map(|v| {
            let id = v.get("id")?.as_str()?.to_string();
            let name = v.get("name")?.as_str()?.to_string();
            Some((id, name))
        })
        .collect()
}

async fn az_cmd(args: &[&str]) {
    let output = tokio::process::Command::new("az").args(args).output().await;
    match output {
        Ok(o) if o.status.success() => {}
        Ok(o) => tracing::warn!(
            args = ?args,
            stderr = %String::from_utf8_lossy(&o.stderr),
            "reaper: az delete non-zero (soft-fail)"
        ),
        Err(e) => tracing::warn!(error = %e, args = ?args, "reaper: az delete failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_owned_keeps_only_ab_prefix() {
        let input = vec![
            ("/r/1".to_string(), "ab-vm-01".to_string()),
            ("/r/2".to_string(), "AB-VM-02".to_string()),
            ("/r/3".to_string(), "prod-server".to_string()),
            ("/r/4".to_string(), "ab".to_string()),
        ];
        let out = filter_owned(input);
        let names: Vec<&str> = out.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, vec!["ab-vm-01", "AB-VM-02"]);
    }
}
