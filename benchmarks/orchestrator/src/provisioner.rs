use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const RESOURCE_GROUP: &str = "alethabench-rg";
const PROVISION_TIMEOUT: Duration = Duration::from_secs(300);
const START_STOP_TIMEOUT: Duration = Duration::from_secs(120);

/// Default SSH username per cloud provider.
fn default_user(cloud: &str) -> &'static str {
    match cloud {
        "azure" => "azureuser",
        "aws" => "ubuntu",
        "gcp" => "alethabench",
        _ => "ubuntu",
    }
}

/// Information about a provisioned VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmInfo {
    pub name: String,
    pub ip: String,
    pub cloud: String,
    pub region: String,
    pub os: String,
    pub vm_size: String,
    /// Cloud-specific resource grouping (Azure RG, AWS instance-id, GCP project).
    pub resource_group: String,
    /// SSH username for this VM.
    pub ssh_user: String,
}

// ─── Generic CLI helper ────────────────────────────────────────────────────

/// Run a CLI command with a timeout, returning stdout on success.
async fn cli_cmd(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let result = tokio::time::timeout(timeout, async {
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .with_context(|| format!("failed to execute {program}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "{program} {} failed (exit {}): {}",
                args.first().unwrap_or(&""),
                output.status,
                stderr.trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    })
    .await
    .with_context(|| format!("{program} command timed out"))??;

    Ok(result)
}

/// Convenience wrapper for `az` commands.
async fn az_cmd(args: &[&str], timeout: Duration) -> Result<String> {
    cli_cmd("az", args, timeout).await
}

/// Convenience wrapper for `aws` commands.
async fn aws_cmd(args: &[&str], timeout: Duration) -> Result<String> {
    cli_cmd("aws", args, timeout).await
}

/// Convenience wrapper for `gcloud` commands.
async fn gcloud_cmd(args: &[&str], timeout: Duration) -> Result<String> {
    cli_cmd("gcloud", args, timeout).await
}

// ─── Image resolution ──────────────────────────────────────────────────────

/// Resolve the VM image URN for Azure.
fn azure_image_for_os(os: &str) -> Result<&'static str> {
    match os.to_lowercase().as_str() {
        "ubuntu" | "linux" => Ok("Canonical:ubuntu-24_04-lts:server:latest"),
        "windows" => Ok("MicrosoftWindowsServer:WindowsServer:2022-datacenter:latest"),
        other => bail!("unsupported OS for Azure: {other} (expected ubuntu or windows)"),
    }
}

/// Resolve the AMI for AWS Ubuntu in common regions.
/// Returns a recent Ubuntu 24.04 LTS AMI ID.  For production use, these should
/// be looked up dynamically via `aws ec2 describe-images`.
fn aws_ami_for_region(region: &str) -> &'static str {
    // Ubuntu 24.04 LTS HVM SSD (amd64) — representative AMIs per region.
    // These are placeholders; real deployments should query the SSM parameter
    // /aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id
    match region {
        "us-east-1" => "ami-0e1bed4f06a3b463d",
        "us-east-2" => "ami-0ea3405d2d2522162",
        "us-west-1" => "ami-014d05e6b24240371",
        "us-west-2" => "ami-05d38da78ce859165",
        "eu-west-1" => "ami-0e9085e60087ce171",
        "eu-central-1" => "ami-0faab6bdbac9486fb",
        "ap-southeast-1" => "ami-01811d4912b4ccb26",
        "ap-northeast-1" => "ami-0d52744d6551d851e",
        _ => "ami-0e1bed4f06a3b463d", // fallback to us-east-1
    }
}

// ─── Provision ─────────────────────────────────────────────────────────────

/// Provision a new VM on the specified cloud provider.
///
/// * `cloud` — `"azure"`, `"aws"`, or `"gcp"`
/// * `region` — cloud-specific region/zone string
/// * `os` — operating system (`"ubuntu"` or `"windows"`)
/// * `vm_size` — cloud-native size string (use `vm_tiers::resolve_vm_size` first)
/// * `name` — VM name / name-prefix
pub async fn provision_vm(
    cloud: &str,
    region: &str,
    os: &str,
    vm_size: &str,
    name: &str,
) -> Result<VmInfo> {
    match cloud.to_lowercase().as_str() {
        "azure" => provision_azure(region, os, vm_size, name).await,
        "aws" => provision_aws(region, os, vm_size, name).await,
        "gcp" => provision_gcp(region, os, vm_size, name).await,
        other => bail!("unsupported cloud provider: '{other}' (expected azure, aws, or gcp)"),
    }
}

// ─── Azure ─────────────────────────────────────────────────────────────────

async fn provision_azure(region: &str, os: &str, vm_size: &str, name: &str) -> Result<VmInfo> {
    let image = azure_image_for_os(os)?;
    let is_linux = !os.eq_ignore_ascii_case("windows");

    tracing::info!("Provisioning Azure VM {name} (size={vm_size}, os={os}, region={region})");

    // Ensure resource group exists in the requested region
    let _ = az_cmd(
        &[
            "group",
            "create",
            "--name",
            RESOURCE_GROUP,
            "--location",
            region,
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
        "--location",
        region,
        "--tags",
        "alethabench=true",
        "--output",
        "json",
    ];

    if is_linux {
        // Use the ed25519 key (same key used by ssh_exec/scp_to).
        // --generate-ssh-keys only installs id_rsa which doesn't match the ed25519 key
        // used by the SSH module, causing auth failures.
        let ed25519_pub = std::path::Path::new("/root/.ssh/id_ed25519.pub");
        if ed25519_pub.exists() {
            args.extend_from_slice(&[
                "--ssh-key-values",
                "/root/.ssh/id_ed25519.pub",
                "--admin-username",
                "azureuser",
            ]);
        } else {
            args.extend_from_slice(&["--generate-ssh-keys", "--admin-username", "azureuser"]);
        }
    } else {
        args.extend_from_slice(&[
            "--admin-username",
            "azureuser",
            "--admin-password",
            "AletheBench!2026",
        ]);
    }

    let ip = match az_cmd(&args, PROVISION_TIMEOUT).await {
        Ok(stdout) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&stdout).context("parsing az vm create JSON output")?;
            parsed["publicIpAddress"].as_str().unwrap_or("").to_string()
        }
        Err(e) => {
            tracing::warn!("az vm create failed ({e:#}), checking if VM was created anyway...");
            String::new()
        }
    };

    // If we didn't get an IP from create output, try to fetch it
    let ip = if ip.is_empty() {
        match find_existing_vm_azure(name).await? {
            Some(vm) if !vm.ip.is_empty() => {
                tracing::info!("VM {name} exists at {}, using existing", vm.ip);
                vm.ip
            }
            _ => bail!("Azure VM {name} was not created and does not exist"),
        }
    } else {
        ip
    };

    // Open TCP ports 8443 (HTTPS) and 9100 (metrics agent)
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

    // Open UDP 8443 for HTTP/3 (QUIC) — az vm open-port only does TCP,
    // so we add an NSG rule directly for UDP.
    let nsg_name = format!("{name}NSG");
    let _ = az_cmd(
        &[
            "network",
            "nsg",
            "rule",
            "create",
            "--resource-group",
            RESOURCE_GROUP,
            "--nsg-name",
            &nsg_name,
            "--name",
            "AllowUDP8443",
            "--priority",
            "1003",
            "--protocol",
            "Udp",
            "--destination-port-ranges",
            "8443",
            "--access",
            "Allow",
            "--direction",
            "Inbound",
            "--output",
            "none",
        ],
        START_STOP_TIMEOUT,
    )
    .await;

    tracing::info!("Azure VM {name} provisioned at {ip}");

    Ok(VmInfo {
        name: name.to_string(),
        ip,
        cloud: "azure".to_string(),
        region: region.to_string(),
        os: os.to_string(),
        vm_size: vm_size.to_string(),
        resource_group: RESOURCE_GROUP.to_string(),
        ssh_user: default_user("azure").to_string(),
    })
}

// ─── AWS ───────────────────────────────────────────────────────────────────

async fn provision_aws(region: &str, _os: &str, vm_size: &str, name: &str) -> Result<VmInfo> {
    // Currently only Ubuntu is supported on AWS.
    let ami = aws_ami_for_region(region);

    tracing::info!(
        "Provisioning AWS EC2 instance {name} (type={vm_size}, region={region}, ami={ami})"
    );

    // Ensure security group exists (idempotent — ignores "already exists" errors)
    let sg_name = "alethabench-sg";
    let _ = aws_cmd(
        &[
            "ec2",
            "create-security-group",
            "--group-name",
            sg_name,
            "--description",
            "AletheBench benchmark VMs",
            "--region",
            region,
            "--output",
            "json",
        ],
        START_STOP_TIMEOUT,
    )
    .await;

    // Open TCP ports 22, 8443, 9100 (idempotent — ignores duplicate rules)
    for port in ["22", "8443", "9100"] {
        let _ = aws_cmd(
            &[
                "ec2",
                "authorize-security-group-ingress",
                "--group-name",
                sg_name,
                "--protocol",
                "tcp",
                "--port",
                port,
                "--cidr",
                "0.0.0.0/0",
                "--region",
                region,
            ],
            START_STOP_TIMEOUT,
        )
        .await;
    }

    // Open UDP 8443 for HTTP/3 (QUIC)
    let _ = aws_cmd(
        &[
            "ec2",
            "authorize-security-group-ingress",
            "--group-name",
            sg_name,
            "--protocol",
            "udp",
            "--port",
            "8443",
            "--cidr",
            "0.0.0.0/0",
            "--region",
            region,
        ],
        START_STOP_TIMEOUT,
    )
    .await;

    // Launch instance
    let tag_spec = format!(
        "ResourceType=instance,Tags=[{{Key=Name,Value={name}}},{{Key=alethabench,Value=true}}]"
    );
    let stdout = aws_cmd(
        &[
            "ec2",
            "run-instances",
            "--image-id",
            ami,
            "--instance-type",
            vm_size,
            "--region",
            region,
            "--key-name",
            "alethabench-key",
            "--security-groups",
            sg_name,
            "--tag-specifications",
            &tag_spec,
            "--count",
            "1",
            "--output",
            "json",
        ],
        PROVISION_TIMEOUT,
    )
    .await
    .context("aws ec2 run-instances failed")?;

    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).context("parsing run-instances JSON")?;
    let instance_id = parsed["Instances"][0]["InstanceId"]
        .as_str()
        .context("no InstanceId in run-instances output")?
        .to_string();

    tracing::info!("AWS instance {instance_id} launched, waiting for public IP...");

    // Wait for the instance to get a public IP (poll describe-instances)
    let ip = poll_aws_public_ip(&instance_id, region).await?;

    tracing::info!("AWS instance {name} ({instance_id}) provisioned at {ip}");

    Ok(VmInfo {
        name: name.to_string(),
        ip,
        cloud: "aws".to_string(),
        region: region.to_string(),
        os: "ubuntu".to_string(),
        vm_size: vm_size.to_string(),
        resource_group: instance_id,
        ssh_user: default_user("aws").to_string(),
    })
}

/// Poll `aws ec2 describe-instances` until a public IP is available.
async fn poll_aws_public_ip(instance_id: &str, region: &str) -> Result<String> {
    let deadline = tokio::time::Instant::now() + PROVISION_TIMEOUT;

    loop {
        if tokio::time::Instant::now() > deadline {
            bail!("timed out waiting for public IP on AWS instance {instance_id}");
        }

        let result = aws_cmd(
            &[
                "ec2",
                "describe-instances",
                "--instance-ids",
                instance_id,
                "--region",
                region,
                "--query",
                "Reservations[0].Instances[0].PublicIpAddress",
                "--output",
                "text",
            ],
            START_STOP_TIMEOUT,
        )
        .await;

        if let Ok(ip) = result {
            let ip = ip.trim().to_string();
            if !ip.is_empty() && ip != "None" {
                return Ok(ip);
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// ─── GCP ───────────────────────────────────────────────────────────────────

async fn provision_gcp(zone: &str, _os: &str, vm_size: &str, name: &str) -> Result<VmInfo> {
    // Currently only Ubuntu is supported on GCP.
    tracing::info!("Provisioning GCP instance {name} (type={vm_size}, zone={zone})");

    let stdout = gcloud_cmd(
        &[
            "compute",
            "instances",
            "create",
            name,
            "--zone",
            zone,
            "--machine-type",
            vm_size,
            "--image-family",
            "ubuntu-2404-lts",
            "--image-project",
            "ubuntu-os-cloud",
            "--tags",
            "alethabench",
            "--metadata",
            "alethabench=true",
            "--format",
            "json",
        ],
        PROVISION_TIMEOUT,
    )
    .await
    .context("gcloud compute instances create failed")?;

    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).context("parsing gcloud create JSON")?;

    // gcloud returns an array; the first element has networkInterfaces
    let ip = parsed
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|inst| inst["networkInterfaces"].as_array())
        .and_then(|ifaces| ifaces.first())
        .and_then(|iface| iface["accessConfigs"].as_array())
        .and_then(|configs| configs.first())
        .and_then(|config| config["natIP"].as_str())
        .unwrap_or("")
        .to_string();

    if ip.is_empty() {
        bail!("GCP instance {name} was created but has no external IP");
    }

    // Open firewall rules for 8443 and 9100 (idempotent)
    let rule_name = format!("alethabench-allow-{name}");
    let _ = gcloud_cmd(
        &[
            "compute",
            "firewall-rules",
            "create",
            &rule_name,
            "--allow",
            "tcp:8443,udp:8443,tcp:9100,tcp:22",
            "--target-tags",
            "alethabench",
            "--source-ranges",
            "0.0.0.0/0",
            "--quiet",
        ],
        START_STOP_TIMEOUT,
    )
    .await;

    tracing::info!("GCP instance {name} provisioned at {ip}");

    Ok(VmInfo {
        name: name.to_string(),
        ip,
        cloud: "gcp".to_string(),
        region: zone.to_string(),
        os: "ubuntu".to_string(),
        vm_size: vm_size.to_string(),
        resource_group: String::new(), // GCP uses project-level scoping
        ssh_user: default_user("gcp").to_string(),
    })
}

// ─── Start / Stop / Destroy ────────────────────────────────────────────────

/// Start a stopped/deallocated VM.
pub async fn start_vm(vm: &VmInfo) -> Result<()> {
    tracing::info!("Starting VM {} ({})", vm.name, vm.cloud);
    match vm.cloud.as_str() {
        "azure" => {
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
        }
        "aws" => {
            aws_cmd(
                &[
                    "ec2",
                    "start-instances",
                    "--instance-ids",
                    &vm.resource_group,
                    "--region",
                    &vm.region,
                ],
                START_STOP_TIMEOUT,
            )
            .await?;
        }
        "gcp" => {
            gcloud_cmd(
                &[
                    "compute",
                    "instances",
                    "start",
                    &vm.name,
                    "--zone",
                    &vm.region,
                    "--quiet",
                ],
                START_STOP_TIMEOUT,
            )
            .await?;
        }
        other => bail!("start_vm: unsupported cloud '{other}'"),
    }
    tracing::info!("VM {} started", vm.name);
    Ok(())
}

/// Deallocate / stop a VM (stops billing).
pub async fn stop_vm(vm: &VmInfo) -> Result<()> {
    tracing::info!("Stopping VM {} ({})", vm.name, vm.cloud);
    match vm.cloud.as_str() {
        "azure" => {
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
        }
        "aws" => {
            aws_cmd(
                &[
                    "ec2",
                    "stop-instances",
                    "--instance-ids",
                    &vm.resource_group,
                    "--region",
                    &vm.region,
                ],
                START_STOP_TIMEOUT,
            )
            .await?;
        }
        "gcp" => {
            gcloud_cmd(
                &[
                    "compute",
                    "instances",
                    "stop",
                    &vm.name,
                    "--zone",
                    &vm.region,
                    "--quiet",
                ],
                START_STOP_TIMEOUT,
            )
            .await?;
        }
        other => bail!("stop_vm: unsupported cloud '{other}'"),
    }
    tracing::info!("VM {} stopped", vm.name);
    Ok(())
}

/// Delete a VM and its associated resources.
pub async fn destroy_vm(vm: &VmInfo) -> Result<()> {
    tracing::info!("Destroying VM {} ({})", vm.name, vm.cloud);
    match vm.cloud.as_str() {
        "azure" => {
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
        }
        "aws" => {
            aws_cmd(
                &[
                    "ec2",
                    "terminate-instances",
                    "--instance-ids",
                    &vm.resource_group,
                    "--region",
                    &vm.region,
                ],
                PROVISION_TIMEOUT,
            )
            .await?;
        }
        "gcp" => {
            gcloud_cmd(
                &[
                    "compute",
                    "instances",
                    "delete",
                    &vm.name,
                    "--zone",
                    &vm.region,
                    "--quiet",
                ],
                PROVISION_TIMEOUT,
            )
            .await?;
        }
        other => bail!("destroy_vm: unsupported cloud '{other}'"),
    }
    tracing::info!("VM {} destroyed", vm.name);
    Ok(())
}

// ─── Lookup helpers ────────────────────────────────────────────────────────

/// Check if an Azure VM with the given name already exists, returning its info if so.
pub async fn find_existing_vm_azure(name: &str) -> Result<Option<VmInfo>> {
    tracing::debug!("Checking for existing Azure VM {name}");

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
            let location = parsed["location"].as_str().unwrap_or("eastus").to_string();
            let os = if parsed["storageProfile"]["osDisk"]["osType"]
                .as_str()
                .unwrap_or("")
                .eq_ignore_ascii_case("windows")
            {
                "windows".to_string()
            } else {
                "ubuntu".to_string()
            };

            tracing::info!("Found existing Azure VM {name} at {ip}");
            Ok(Some(VmInfo {
                name: name.to_string(),
                ip,
                cloud: "azure".to_string(),
                region: location,
                os,
                vm_size,
                resource_group: RESOURCE_GROUP.to_string(),
                ssh_user: default_user("azure").to_string(),
            }))
        }
        Err(_) => {
            tracing::debug!("Azure VM {name} does not exist");
            Ok(None)
        }
    }
}

/// Backward-compatible alias.
pub async fn find_existing_vm(name: &str) -> Result<Option<VmInfo>> {
    find_existing_vm_azure(name).await
}

/// Refresh the public IP address for a VM (useful after start).
pub async fn refresh_ip(vm: &mut VmInfo) -> Result<()> {
    match vm.cloud.as_str() {
        "azure" => {
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
        }
        "aws" => {
            let stdout = aws_cmd(
                &[
                    "ec2",
                    "describe-instances",
                    "--instance-ids",
                    &vm.resource_group,
                    "--region",
                    &vm.region,
                    "--query",
                    "Reservations[0].Instances[0].PublicIpAddress",
                    "--output",
                    "text",
                ],
                START_STOP_TIMEOUT,
            )
            .await?;
            let ip = stdout.trim().to_string();
            if !ip.is_empty() && ip != "None" {
                vm.ip = ip;
            }
        }
        "gcp" => {
            let stdout = gcloud_cmd(
                &[
                    "compute",
                    "instances",
                    "describe",
                    &vm.name,
                    "--zone",
                    &vm.region,
                    "--format",
                    "json",
                ],
                START_STOP_TIMEOUT,
            )
            .await?;
            let parsed: serde_json::Value = serde_json::from_str(&stdout)?;
            let ip = parsed["networkInterfaces"]
                .as_array()
                .and_then(|ifaces| ifaces.first())
                .and_then(|iface| iface["accessConfigs"].as_array())
                .and_then(|configs| configs.first())
                .and_then(|config| config["natIP"].as_str())
                .unwrap_or("");
            if !ip.is_empty() {
                vm.ip = ip.to_string();
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_azure_image_for_os() {
        assert!(azure_image_for_os("ubuntu").is_ok());
        assert!(azure_image_for_os("linux").is_ok());
        assert!(azure_image_for_os("windows").is_ok());
        assert!(azure_image_for_os("freebsd").is_err());
    }

    #[test]
    fn test_vm_info_serialization() {
        let vm = VmInfo {
            name: "test-vm".into(),
            ip: "1.2.3.4".into(),
            cloud: "azure".into(),
            region: "eastus".into(),
            os: "ubuntu".into(),
            vm_size: "Standard_D2s_v3".into(),
            resource_group: "alethabench-rg".into(),
            ssh_user: "azureuser".into(),
        };
        let json = serde_json::to_string(&vm).unwrap();
        let deserialized: VmInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test-vm");
        assert_eq!(deserialized.ip, "1.2.3.4");
        assert_eq!(deserialized.cloud, "azure");
        assert_eq!(deserialized.region, "eastus");
        assert_eq!(deserialized.ssh_user, "azureuser");
    }

    #[test]
    fn test_unsupported_cloud() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(provision_vm(
            "digitalocean",
            "nyc1",
            "ubuntu",
            "s-1vcpu-1gb",
            "test",
        ));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported cloud"),
            "should reject unknown clouds"
        );
    }

    #[test]
    fn test_default_user() {
        assert_eq!(default_user("azure"), "azureuser");
        assert_eq!(default_user("aws"), "ubuntu");
        assert_eq!(default_user("gcp"), "alethabench");
        assert_eq!(default_user("other"), "ubuntu");
    }

    #[test]
    fn test_aws_ami_fallback() {
        // Known region
        assert_eq!(aws_ami_for_region("us-east-1"), "ami-0e1bed4f06a3b463d");
        // Unknown region falls back to us-east-1 AMI
        assert_eq!(aws_ami_for_region("ap-south-1"), "ami-0e1bed4f06a3b463d");
    }
}
