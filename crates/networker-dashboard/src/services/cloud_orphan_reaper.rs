//! Cloud orphan reaper — list and delete dangling cloud resources that are
//! not referenced by any row in our database.
//!
//! # Why
//!
//! Failed tester/benchmark VM creations leave behind NICs, Public IPs, and
//! disks that still have valid references between themselves but no
//! corresponding row in our DB. Over time these pile up and hit cloud quotas
//! (Azure defaults to 10 public IPs per subscription per region).
//!
//! The existing `api::tester_precheck` only deletes unattached public IPs;
//! that doesn't help when a failed VM creation left a NIC attached to an IP
//! attached to nothing else.
//!
//! # What it does
//!
//! - Lists every VM / NIC / Public IP / Disk in the configured resource group.
//! - Marks each resource as an orphan if:
//!   1. Its `resource_id` is NOT in the known-set the caller provides
//!      (typically every `vm_resource_id` + related NIC/IP/disk ID we've
//!      seen in `project_tester` and `benchmark_config.config_json`).
//!   2. Its name matches a conservative allow-list of prefixes we actually
//!      create (`tester-*`, `ab-*`, `nwk-ep-*`). This is defensive — we
//!      never touch resources that don't look like ours.
//! - Deletes them in the correct dependency order: VM → NIC → disk → IP.
//! - Soft-fails: one resource's delete failure doesn't stop the loop.
//!
//! # Providers
//!
//! Azure has a real implementation. AWS and GCP are stubs for now (return
//! empty Vec). The orchestrator crate has its own sibling helper because it
//! doesn't depend on this lib.

use super::cloud_provider::{AzureProvider, CloudProvider};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashSet;

/// A cloud resource the reaper has identified as an orphan.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OrphanResource {
    /// Full provider resource ID (e.g. Azure ARM ID).
    pub resource_id: String,
    /// Short resource name for human-readable reporting.
    pub name: String,
    /// One of: "vm", "nic", "public_ip", "disk".
    pub kind: String,
    /// Provider name: "azure", "aws", "gcp".
    pub provider: String,
}

/// Outcome of a single resource delete attempt.
#[derive(Debug, Clone, Serialize)]
pub struct DeletedResource {
    pub resource_id: String,
    pub name: String,
    pub kind: String,
}

/// Outcome of a failed delete attempt.
#[derive(Debug, Clone, Serialize)]
pub struct FailedResource {
    pub resource_id: String,
    pub name: String,
    pub kind: String,
    pub error: String,
}

/// Summary of a `delete_orphans` call.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DeleteOrphansReport {
    pub deleted: Vec<DeletedResource>,
    pub failed: Vec<FailedResource>,
}

impl DeleteOrphansReport {
    pub fn total(&self) -> usize {
        self.deleted.len() + self.failed.len()
    }
}

/// Allow-list of name prefixes this reaper is willing to touch.
///
/// Anything else is left alone regardless of whether its resource ID is in
/// the known-set. This is a defence-in-depth guard against the reaper
/// destroying resources that belong to other tenants of the same
/// subscription / resource group.
pub const OWNED_NAME_PREFIXES: &[&str] = &["tester-", "ab-", "nwk-ep-"];

/// Returns true if `name` begins with one of our owned prefixes.
pub fn name_is_ours(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    OWNED_NAME_PREFIXES.iter().any(|p| n.starts_with(p))
}

/// A single raw cloud resource record used by the filter logic. Broken out
/// so tests can drive the filter without any cloud calls.
#[derive(Debug, Clone)]
pub struct RawResource {
    pub resource_id: String,
    pub name: String,
    pub kind: String,
    pub provider: String,
}

/// Pure filter: keep only resources whose ID is NOT in `known_ids` AND whose
/// name matches an owned prefix.
pub fn filter_orphans(
    resources: &[RawResource],
    known_ids: &HashSet<String>,
) -> Vec<OrphanResource> {
    resources
        .iter()
        .filter(|r| !known_ids.contains(&r.resource_id) && name_is_ours(&r.name))
        .map(|r| OrphanResource {
            resource_id: r.resource_id.clone(),
            name: r.name.clone(),
            kind: r.kind.clone(),
            provider: r.provider.clone(),
        })
        .collect()
}

/// List orphans for a given provider.
pub async fn list_orphans(
    provider: &CloudProvider,
    known_resource_ids: &HashSet<String>,
) -> Result<Vec<OrphanResource>> {
    match provider {
        CloudProvider::Azure(az) => list_orphans_azure(az, known_resource_ids).await,
        // TODO: implement AWS orphan detection (EC2 instances, volumes, ENIs, EIPs).
        CloudProvider::Aws(_) => Ok(Vec::new()),
        // TODO: implement GCP orphan detection (instances, disks, addresses).
        CloudProvider::Gcp(_) => Ok(Vec::new()),
    }
}

/// Delete the orphans in dependency-safe order: VM → NIC → disk → IP.
///
/// Each delete is best-effort; one failure does not stop the rest.
pub async fn delete_orphans(
    provider: &CloudProvider,
    orphans: &[OrphanResource],
) -> DeleteOrphansReport {
    match provider {
        CloudProvider::Azure(az) => delete_orphans_azure(az, orphans).await,
        // TODO: AWS
        CloudProvider::Aws(_) => DeleteOrphansReport::default(),
        // TODO: GCP
        CloudProvider::Gcp(_) => DeleteOrphansReport::default(),
    }
}

// ── Azure ───────────────────────────────────────────────────────────────────

fn az_bin() -> String {
    std::env::var("AZ_CMD").unwrap_or_else(|_| "az".to_string())
}

async fn list_orphans_azure(
    az: &AzureProvider,
    known_ids: &HashSet<String>,
) -> Result<Vec<OrphanResource>> {
    let mut raw: Vec<RawResource> = Vec::new();

    // az vm list → [{id, name}, ...]
    raw.extend(
        az_list_json(&[
            "vm",
            "list",
            "--subscription",
            &az.subscription_id,
            "--resource-group",
            &az.resource_group,
            "--query",
            "[].{id:id,name:name}",
        ])
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name)| RawResource {
            resource_id: id,
            name,
            kind: "vm".into(),
            provider: "azure".into(),
        }),
    );

    raw.extend(
        az_list_json(&[
            "network",
            "nic",
            "list",
            "--subscription",
            &az.subscription_id,
            "--resource-group",
            &az.resource_group,
            "--query",
            "[].{id:id,name:name}",
        ])
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name)| RawResource {
            resource_id: id,
            name,
            kind: "nic".into(),
            provider: "azure".into(),
        }),
    );

    raw.extend(
        az_list_json(&[
            "network",
            "public-ip",
            "list",
            "--subscription",
            &az.subscription_id,
            "--resource-group",
            &az.resource_group,
            "--query",
            "[].{id:id,name:name}",
        ])
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name)| RawResource {
            resource_id: id,
            name,
            kind: "public_ip".into(),
            provider: "azure".into(),
        }),
    );

    raw.extend(
        az_list_json(&[
            "disk",
            "list",
            "--subscription",
            &az.subscription_id,
            "--resource-group",
            &az.resource_group,
            "--query",
            "[].{id:id,name:name}",
        ])
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name)| RawResource {
            resource_id: id,
            name,
            kind: "disk".into(),
            provider: "azure".into(),
        }),
    );

    Ok(filter_orphans(&raw, known_ids))
}

async fn az_list_json(args: &[&str]) -> Result<Vec<(String, String)>> {
    let output = tokio::process::Command::new(az_bin())
        .env("PYTHONWARNINGS", "ignore")
        .args(args)
        .arg("--output")
        .arg("json")
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "az {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    Ok(parsed
        .into_iter()
        .filter_map(|v| {
            let id = v.get("id")?.as_str()?.to_string();
            let name = v.get("name")?.as_str()?.to_string();
            Some((id, name))
        })
        .collect())
}

async fn delete_orphans_azure(
    az: &AzureProvider,
    orphans: &[OrphanResource],
) -> DeleteOrphansReport {
    let mut report = DeleteOrphansReport::default();

    // Group by kind so we can delete in dependency order: VM → NIC → disk → IP.
    // (A VM delete releases its attached NIC lease; deleting the NIC first
    // releases the IP; deleting the disk last is safe either way.)
    let order = ["vm", "nic", "disk", "public_ip"];
    for kind in order {
        for o in orphans.iter().filter(|o| o.kind == kind) {
            match az_delete_one(az, &o.kind, &o.resource_id).await {
                Ok(()) => report.deleted.push(DeletedResource {
                    resource_id: o.resource_id.clone(),
                    name: o.name.clone(),
                    kind: o.kind.clone(),
                }),
                Err(e) => report.failed.push(FailedResource {
                    resource_id: o.resource_id.clone(),
                    name: o.name.clone(),
                    kind: o.kind.clone(),
                    error: e.to_string(),
                }),
            }
        }
    }

    report
}

async fn az_delete_one(az: &AzureProvider, kind: &str, id: &str) -> Result<()> {
    let args: Vec<&str> = match kind {
        "vm" => vec![
            "vm",
            "delete",
            "--subscription",
            &az.subscription_id,
            "--ids",
            id,
            "--yes",
        ],
        "nic" => vec![
            "network",
            "nic",
            "delete",
            "--subscription",
            &az.subscription_id,
            "--ids",
            id,
        ],
        "public_ip" => vec![
            "network",
            "public-ip",
            "delete",
            "--subscription",
            &az.subscription_id,
            "--ids",
            id,
        ],
        "disk" => vec![
            "disk",
            "delete",
            "--subscription",
            &az.subscription_id,
            "--ids",
            id,
            "--yes",
        ],
        other => anyhow::bail!("unknown orphan kind: {other}"),
    };
    let output = tokio::process::Command::new(az_bin())
        .env("PYTHONWARNINGS", "ignore")
        .args(&args)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "az delete failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(id: &str, name: &str, kind: &str) -> RawResource {
        RawResource {
            resource_id: id.into(),
            name: name.into(),
            kind: kind.into(),
            provider: "azure".into(),
        }
    }

    #[test]
    fn filter_keeps_only_unknown_ids_with_owned_names() {
        let resources = vec![
            raw("/r/1", "tester-eastus-01", "vm"),
            raw("/r/2", "tester-eastus-02-nic", "nic"),
            raw("/r/3", "tester-eastus-02-ip", "public_ip"),
            raw("/r/4", "ab-ubuntu-loop-01", "vm"),
            raw("/r/5", "nwk-ep-eu-west-01", "vm"),
            // Known → ignored.
            raw("/r/6", "tester-known-01", "vm"),
            // Wrong prefix → never touched.
            raw("/r/7", "prod-app-server", "vm"),
            raw("/r/8", "bastion-nic", "nic"),
        ];
        let mut known = HashSet::new();
        known.insert("/r/6".to_string());

        let orphans = filter_orphans(&resources, &known);
        let ids: Vec<&str> = orphans.iter().map(|o| o.resource_id.as_str()).collect();
        assert_eq!(ids, vec!["/r/1", "/r/2", "/r/3", "/r/4", "/r/5"]);
    }

    #[test]
    fn filter_case_insensitive_on_name_prefix() {
        let resources = vec![
            raw("/r/1", "Tester-EastUS-01", "vm"),
            raw("/r/2", "AB-Ubuntu-01", "vm"),
            raw("/r/3", "NWK-EP-01", "vm"),
        ];
        let orphans = filter_orphans(&resources, &HashSet::new());
        assert_eq!(orphans.len(), 3);
    }

    #[test]
    fn filter_empty_inputs_empty_output() {
        let orphans = filter_orphans(&[], &HashSet::new());
        assert!(orphans.is_empty());
    }

    #[test]
    fn filter_everything_known_returns_none() {
        let resources = vec![raw("/r/1", "tester-eastus-01", "vm")];
        let mut known = HashSet::new();
        known.insert("/r/1".to_string());
        assert!(filter_orphans(&resources, &known).is_empty());
    }

    #[test]
    fn name_is_ours_rejects_foreign_prefixes() {
        assert!(name_is_ours("tester-x"));
        assert!(name_is_ours("ab-x"));
        assert!(name_is_ours("nwk-ep-x"));
        assert!(!name_is_ours("prod-x"));
        assert!(!name_is_ours("bastion"));
        assert!(!name_is_ours("tst-x"));
    }

    #[test]
    fn delete_report_totals() {
        let mut r = DeleteOrphansReport::default();
        r.deleted.push(DeletedResource {
            resource_id: "a".into(),
            name: "a".into(),
            kind: "vm".into(),
        });
        r.failed.push(FailedResource {
            resource_id: "b".into(),
            name: "b".into(),
            kind: "nic".into(),
            error: "nope".into(),
        });
        assert_eq!(r.total(), 2);
    }
}
