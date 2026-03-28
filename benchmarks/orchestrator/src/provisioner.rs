use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const RESOURCE_GROUP: &str = "alethabench-rg";
const PROVISION_TIMEOUT: Duration = Duration::from_secs(300);
const START_STOP_TIMEOUT: Duration = Duration::from_secs(120);

/// Information about a provisioned VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub name: String,
    pub ip: String,
    pub cloud: String,
    pub os: String,
    pub vm_size: String,
    pub resource_group: String,
}

/// Run an Azure CLI command with a timeout, returning stdout on success.
async fn az_cmd(args: &[&str], timeout: Duration) -> Result<String> {
    let result = tokio::time::timeout(timeout, async {
        let output = tokio::process::Command::new("az")
            .args(args)
            .output()
            .await
            .context("failed to execute az CLI")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "az {} failed (exit {}): {}",
                args.first().unwrap_or(&""),
                output.status,
                stderr.trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    })
    .await
    .context("az command timed out")??;

    Ok(result)
}

/// Resolve the VM image URN for the given OS.
fn image_for_os(os: &str) -> Result<&'static str> {
    match os.to_lowercase().as_str() {
        "ubuntu" | "linux" => Ok("Canonical:ubuntu-24_04-lts:server:latest"),
        "windows" => Ok("MicrosoftWindowsServer:WindowsServer:2022-datacenter:latest"),
        other => bail!("unsupported OS: {other} (expected ubuntu or windows)"),
    }
}

/// Provision a new Azure VM.
pub async fn provision_vm(cloud: &str, os: &str, vm_size: &str, name: &str) -> Result<VmInfo> {
    if cloud != "azure" {
        bail!("only 'azure' cloud is currently supported, got '{cloud}'");
    }

    let image = image_for_os(os)?;
    let is_linux = !os.eq_ignore_ascii_case("windows");

    tracing::info!("Provisioning VM {name} (size={vm_size}, os={os}, image={image})");

    // Ensure resource group exists
    let _ = az_cmd(
        &[
            "group",
            "create",
            "--name",
            RESOURCE_GROUP,
            "--location",
            "eastus",
            "--output",
            "none",
        ],
        START_STOP_TIMEOUT,
    )
    .await;

    // Build az vm create command
    let mut args = vec![
        "vm",
        "create",
        "--resource-group",
        RESOURCE_GROUP,
        "--name",
        name,
        "--image",
        image,
        "--size",
        vm_size,
        "--tags",
        "alethabench=true",
        "--output",
        "json",
    ];

    if is_linux {
        args.extend_from_slice(&["--generate-ssh-keys", "--admin-username", "azureuser"]);
    } else {
        // Windows: enable WinRM, set admin password
        args.extend_from_slice(&[
            "--admin-username",
            "azureuser",
            "--admin-password",
            "AletheBench!2026",
        ]);
    }

    let ip = match az_cmd(&args, PROVISION_TIMEOUT).await {
        Ok(stdout) => {
            // Parse the JSON output to extract the public IP
            let parsed: serde_json::Value =
                serde_json::from_str(&stdout).context("parsing az vm create JSON output")?;
            parsed["publicIpAddress"].as_str().unwrap_or("").to_string()
        }
        Err(e) => {
            // az vm create can fail at the CLI level even when the VM was created.
            // Fall back to checking if the VM exists and get its IP.
            tracing::warn!("az vm create failed ({e:#}), checking if VM was created anyway...");
            String::new()
        }
    };

    // If we didn't get an IP from create output, try to fetch it
    let ip = if ip.is_empty() {
        match find_existing_vm(name).await? {
            Some(vm) if !vm.ip.is_empty() => {
                tracing::info!("VM {name} exists at {}, using existing", vm.ip);
                vm.ip
            }
            _ => bail!("VM {name} was not created and does not exist"),
        }
    } else {
        ip
    };

    // Open port 8443 (HTTPS) and 9100 (metrics agent)
    for port in ["8443", "9100"] {
        let _ = az_cmd(
            &[
                "vm",
                "open-port",
                "--resource-group",
                RESOURCE_GROUP,
                "--name",
                name,
                "--port",
                port,
                "--priority",
                if port == "8443" { "1001" } else { "1002" },
                "--output",
                "none",
            ],
            START_STOP_TIMEOUT,
        )
        .await;
    }

    tracing::info!("VM {name} provisioned at {ip}");

    Ok(VmInfo {
        name: name.to_string(),
        ip,
        cloud: cloud.to_string(),
        os: os.to_string(),
        vm_size: vm_size.to_string(),
        resource_group: RESOURCE_GROUP.to_string(),
    })
}

/// Start a stopped/deallocated VM.
pub async fn start_vm(vm: &VmInfo) -> Result<()> {
    tracing::info!("Starting VM {}", vm.name);
    az_cmd(
        &[
            "vm",
            "start",
            "--resource-group",
            &vm.resource_group,
            "--name",
            &vm.name,
            "--output",
            "none",
        ],
        START_STOP_TIMEOUT,
    )
    .await?;

    // Refresh the public IP (it may change after deallocate/start)
    tracing::info!("VM {} started", vm.name);
    Ok(())
}

/// Deallocate a VM (stops billing).
pub async fn stop_vm(vm: &VmInfo) -> Result<()> {
    tracing::info!("Deallocating VM {}", vm.name);
    az_cmd(
        &[
            "vm",
            "deallocate",
            "--resource-group",
            &vm.resource_group,
            "--name",
            &vm.name,
            "--output",
            "none",
        ],
        START_STOP_TIMEOUT,
    )
    .await?;
    tracing::info!("VM {} deallocated", vm.name);
    Ok(())
}

/// Delete a VM and its associated resources.
#[allow(dead_code)]
pub async fn destroy_vm(vm: &VmInfo) -> Result<()> {
    tracing::info!("Destroying VM {}", vm.name);
    az_cmd(
        &[
            "vm",
            "delete",
            "--resource-group",
            &vm.resource_group,
            "--name",
            &vm.name,
            "--yes",
            "--force-deletion",
            "true",
            "--output",
            "none",
        ],
        PROVISION_TIMEOUT,
    )
    .await?;
    tracing::info!("VM {} destroyed", vm.name);
    Ok(())
}

/// Check if a VM with the given name already exists, returning its info if so.
pub async fn find_existing_vm(name: &str) -> Result<Option<VmInfo>> {
    tracing::debug!("Checking for existing VM {name}");

    let result = az_cmd(
        &[
            "vm",
            "show",
            "--resource-group",
            RESOURCE_GROUP,
            "--name",
            name,
            "--show-details",
            "--output",
            "json",
        ],
        START_STOP_TIMEOUT,
    )
    .await;

    match result {
        Ok(stdout) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&stdout).context("parsing az vm show JSON")?;

            let ip = parsed["publicIps"].as_str().unwrap_or("").to_string();
            let vm_size = parsed["hardwareProfile"]["vmSize"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            let os = if parsed["storageProfile"]["osDisk"]["osType"]
                .as_str()
                .unwrap_or("")
                .eq_ignore_ascii_case("windows")
            {
                "windows".to_string()
            } else {
                "ubuntu".to_string()
            };

            tracing::info!("Found existing VM {name} at {ip}");
            Ok(Some(VmInfo {
                name: name.to_string(),
                ip,
                cloud: "azure".to_string(),
                os,
                vm_size,
                resource_group: RESOURCE_GROUP.to_string(),
            }))
        }
        Err(_) => {
            tracing::debug!("VM {name} does not exist");
            Ok(None)
        }
    }
}

/// Refresh the public IP address for a VM (useful after start).
pub async fn refresh_ip(vm: &mut VmInfo) -> Result<()> {
    let stdout = az_cmd(
        &[
            "vm",
            "show",
            "--resource-group",
            &vm.resource_group,
            "--name",
            &vm.name,
            "--show-details",
            "--output",
            "json",
        ],
        START_STOP_TIMEOUT,
    )
    .await?;

    let parsed: serde_json::Value = serde_json::from_str(&stdout)?;
    if let Some(ip) = parsed["publicIps"].as_str() {
        vm.ip = ip.to_string();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_for_os() {
        assert!(image_for_os("ubuntu").is_ok());
        assert!(image_for_os("linux").is_ok());
        assert!(image_for_os("windows").is_ok());
        assert!(image_for_os("freebsd").is_err());
    }

    #[test]
    fn test_vm_info_serialization() {
        let vm = VmInfo {
            name: "test-vm".into(),
            ip: "1.2.3.4".into(),
            cloud: "azure".into(),
            os: "ubuntu".into(),
            vm_size: "Standard_D2s_v3".into(),
            resource_group: "alethabench-rg".into(),
        };
        let json = serde_json::to_string(&vm).unwrap();
        let deserialized: VmInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test-vm");
        assert_eq!(deserialized.ip, "1.2.3.4");
    }

    #[test]
    fn test_unsupported_cloud() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(provision_vm("gcp", "ubuntu", "n1-standard-2", "test"));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("only 'azure'"),
            "should reject non-azure clouds"
        );
    }
}
