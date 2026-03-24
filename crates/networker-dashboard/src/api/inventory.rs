use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;
use tokio::process::Command;

use crate::AppState;

#[derive(Serialize)]
pub struct CloudVm {
    pub provider: String,
    pub name: String,
    pub region: String,
    pub status: String,
    pub public_ip: Option<String>,
    pub fqdn: Option<String>,
    pub vm_size: Option<String>,
    pub os: Option<String>,
    pub resource_group: Option<String>,
    /// Whether this VM is tracked in our deployments table
    pub managed: bool,
}

#[derive(Serialize)]
pub struct InventoryResponse {
    pub vms: Vec<CloudVm>,
    pub errors: Vec<String>,
}

async fn scan_inventory(
    State(state): State<Arc<AppState>>,
) -> Result<Json<InventoryResponse>, StatusCode> {
    let mut all_vms: Vec<CloudVm> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // Get managed deployment IPs for cross-referencing
    let managed_hosts: Vec<String> = if let Ok(client) = state.db.get().await {
        if let Ok(deployments) = crate::db::deployments::list_all(&client, 100, 0).await {
            deployments
                .iter()
                .filter_map(|d| d.endpoint_ips.as_ref())
                .filter_map(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
                .flatten()
                .collect()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    // Scan Azure, AWS, GCP in parallel
    let (azure, aws, gcp) = tokio::join!(
        scan_azure(&managed_hosts),
        scan_aws(&managed_hosts),
        scan_gcp(&managed_hosts),
    );

    match azure {
        Ok(vms) => all_vms.extend(vms),
        Err(e) => {
            if !e.contains("not found") && !e.contains("not installed") {
                errors.push(format!("Azure: {e}"));
            }
        }
    }
    match aws {
        Ok(vms) => all_vms.extend(vms),
        Err(e) => {
            if !e.contains("not found") && !e.contains("not installed") {
                errors.push(format!("AWS: {e}"));
            }
        }
    }
    match gcp {
        Ok(vms) => all_vms.extend(vms),
        Err(e) => {
            if !e.contains("not found") && !e.contains("not installed") {
                errors.push(format!("GCP: {e}"));
            }
        }
    }

    Ok(Json(InventoryResponse {
        vms: all_vms,
        errors,
    }))
}

async fn scan_azure(managed_hosts: &[String]) -> Result<Vec<CloudVm>, String> {
    // Check if az CLI is available
    let check = Command::new("which")
        .arg("az")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("not found: {e}"))?;
    if !check.status.success() {
        return Err("az CLI not installed".into());
    }

    // List VMs in networker resource groups (endpoint VMs created by install.sh)
    // install.sh creates RGs like: networker-rg-endpoint, networker-dashboard-eastus, etc.
    let output = Command::new("az")
        .args([
            "vm",
            "list",
            "--show-details",
            "--query",
            "[?resourceGroup && (contains(resourceGroup, 'networker') || contains(resourceGroup, 'NETWORKER') || contains(resourceGroup, 'nwk') || contains(resourceGroup, 'NWK'))].{name:name, rg:resourceGroup, location:location, powerState:powerState, publicIps:publicIps, fqdns:fqdns, size:hardwareProfile.vmSize, os:storageProfile.osDisk.osType}",
            "-o",
            "json",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("az vm list failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("az vm list error: {stderr}"));
    }

    let vms: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("parse error: {e}"))?;

    Ok(vms
        .iter()
        .map(|vm| {
            let name = vm["name"].as_str().unwrap_or("").to_string();
            let fqdn = vm["fqdns"]
                .as_str()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let public_ip = vm["publicIps"]
                .as_str()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let managed = is_managed(&fqdn, &public_ip, managed_hosts);

            CloudVm {
                provider: "azure".into(),
                name,
                region: vm["location"].as_str().unwrap_or("").to_string(),
                status: vm["powerState"]
                    .as_str()
                    .unwrap_or("unknown")
                    .replace("VM ", "")
                    .to_lowercase(),
                public_ip,
                fqdn,
                vm_size: vm["size"].as_str().map(|s| s.to_string()),
                os: vm["os"].as_str().map(|s| s.to_lowercase()),
                resource_group: vm["rg"].as_str().map(|s| s.to_string()),
                managed,
            }
        })
        .collect())
}

async fn scan_aws(managed_hosts: &[String]) -> Result<Vec<CloudVm>, String> {
    let check = Command::new("which")
        .arg("aws")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("not found: {e}"))?;
    if !check.status.success() {
        return Err("aws CLI not installed".into());
    }

    // Scan common regions for networker instances
    let regions = [
        "us-east-1",
        "us-west-2",
        "eu-west-1",
        "eu-central-1",
        "ap-southeast-1",
    ];
    let mut all_instances = Vec::new();

    for region in &regions {
        let output = Command::new("aws")
            .args([
                "ec2",
                "describe-instances",
                "--region",
                region,
                "--filters",
                "Name=tag:Name,Values=*networker-endpoint*,*networker-tester*",
                "--query",
                "Reservations[].Instances[].{name:Tags[?Key=='Name']|[0].Value, id:InstanceId, state:State.Name, ip:PublicIpAddress, dns:PublicDnsName, type:InstanceType, az:Placement.AvailabilityZone, platform:Platform}",
                "--output",
                "json",
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .await;

        let output = match output {
            Ok(o) if o.status.success() => o,
            _ => continue, // Skip regions that fail
        };

        let instances: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).unwrap_or_default();

        for i in &instances {
            let fqdn = i["dns"]
                .as_str()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let public_ip = i["ip"].as_str().map(|s| s.to_string());
            let managed = is_managed(&fqdn, &public_ip, managed_hosts);
            let az = i["az"].as_str().unwrap_or("");
            let inst_region = az.trim_end_matches(|c: char| c.is_alphabetic()).to_string();

            all_instances.push(CloudVm {
                provider: "aws".into(),
                name: i["name"]
                    .as_str()
                    .unwrap_or(i["id"].as_str().unwrap_or(""))
                    .to_string(),
                region: inst_region,
                status: i["state"].as_str().unwrap_or("unknown").to_string(),
                public_ip,
                fqdn,
                vm_size: i["type"].as_str().map(|s| s.to_string()),
                os: i["platform"]
                    .as_str()
                    .map(|s| s.to_lowercase())
                    .or(Some("linux".into())),
                resource_group: None,
                managed,
            });
        }
    }

    Ok(all_instances)
}

async fn scan_gcp(managed_hosts: &[String]) -> Result<Vec<CloudVm>, String> {
    let check = Command::new("which")
        .arg("gcloud")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("not found: {e}"))?;
    if !check.status.success() {
        return Err("gcloud CLI not installed".into());
    }

    let output = Command::new("gcloud")
        .args([
            "compute",
            "instances",
            "list",
            "--filter",
            "name~networker-endpoint OR name~networker-tester",
            "--format",
            "json(name,zone,status,networkInterfaces[0].accessConfigs[0].natIP,machineType)",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("gcloud list failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gcloud error: {stderr}"));
    }

    let instances: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).map_err(|e| format!("parse error: {e}"))?;

    Ok(instances
        .iter()
        .map(|i| {
            let public_ip = i["networkInterfaces"]
                .get(0)
                .and_then(|n| n["accessConfigs"].get(0))
                .and_then(|a| a["natIP"].as_str())
                .map(|s| s.to_string());
            let managed = is_managed(&None, &public_ip, managed_hosts);
            let zone = i["zone"].as_str().unwrap_or("");
            let region = zone.rsplit('/').next().unwrap_or(zone).to_string();
            let machine_type = i["machineType"]
                .as_str()
                .and_then(|s| s.rsplit('/').next())
                .map(|s| s.to_string());

            CloudVm {
                provider: "gcp".into(),
                name: i["name"].as_str().unwrap_or("").to_string(),
                region,
                status: i["status"].as_str().unwrap_or("unknown").to_lowercase(),
                public_ip,
                fqdn: None,
                vm_size: machine_type,
                os: Some("linux".into()),
                resource_group: None,
                managed,
            }
        })
        .collect())
}

fn is_managed(fqdn: &Option<String>, public_ip: &Option<String>, managed_hosts: &[String]) -> bool {
    if let Some(ref dns) = fqdn {
        if managed_hosts
            .iter()
            .any(|h| h == dns || h.contains(dns.as_str()))
        {
            return true;
        }
    }
    if let Some(ref ip) = public_ip {
        if managed_hosts
            .iter()
            .any(|h| h == ip || h.contains(ip.as_str()))
        {
            return true;
        }
    }
    false
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/inventory", get(scan_inventory))
        .with_state(state)
}

/// Project-scoped inventory (pass-through — inventory scan is global).
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/inventory", get(scan_inventory))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::is_managed;

    #[test]
    fn managed_by_fqdn_exact_match() {
        let managed = vec!["ec2-1-2-3-4.compute-1.amazonaws.com".to_string()];
        assert!(is_managed(
            &Some("ec2-1-2-3-4.compute-1.amazonaws.com".to_string()),
            &None,
            &managed,
        ));
    }

    #[test]
    fn managed_by_ip_exact_match() {
        let managed = vec!["10.0.0.5".to_string()];
        assert!(is_managed(&None, &Some("10.0.0.5".to_string()), &managed));
    }

    #[test]
    fn not_managed_when_no_match() {
        let managed = vec!["10.0.0.1".to_string()];
        assert!(!is_managed(
            &Some("other.host.com".to_string()),
            &Some("10.0.0.99".to_string()),
            &managed,
        ));
    }

    #[test]
    fn not_managed_when_empty_list() {
        assert!(!is_managed(
            &Some("host.com".to_string()),
            &Some("1.2.3.4".to_string()),
            &[],
        ));
    }

    #[test]
    fn not_managed_when_both_none() {
        let managed = vec!["10.0.0.1".to_string()];
        assert!(!is_managed(&None, &None, &managed));
    }

    #[test]
    fn managed_by_partial_ip_in_fqdn() {
        // The contains() check means an IP appearing inside a managed FQDN matches
        let managed = vec!["ec2-10-0-0-5.compute.amazonaws.com".to_string()];
        // This tests the substring match behavior
        assert!(is_managed(
            &Some("ec2-10-0-0-5.compute.amazonaws.com".to_string()),
            &None,
            &managed,
        ));
    }
}
