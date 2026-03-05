# Cloud Deployment

Deploy `networker-endpoint` (or `networker-tester`) to a cloud VM for real-world latency and throughput measurements.

---

## Overview

The installer handles the full provisioning flow:
1. Creates VM (or reuses existing)
2. Opens required firewall ports
3. Compiles or downloads the binary
4. Starts the server
5. Runs a health check

After deployment the installer writes a `networker-cloud.json` config file containing the VM's IP, which you can pass directly to `networker-tester`.

---

## Azure Deployment

### Prerequisites

- Azure CLI (`az`) installed and logged in (`az login`)
- An active Azure subscription

### Deploy the endpoint

```bash
# Interactive — shows plan, prompts before doing anything
bash install.sh --azure endpoint

# With explicit options (skips most prompts)
bash install.sh --azure endpoint \
  --region eastus \
  --rg networker-rg \
  --vm networker-endpoint-vm \
  --vm-size Standard_B2s
```

The installer:
- Creates the resource group if it does not exist (and prompts before reusing)
- Provisions a VM with an auto-generated name like `nwk-ep-lnx-b1s` (collision detection included)
- Opens TCP ports 8080, 8443 and UDP port 8443 in the NSG
- Deploys and starts `networker-endpoint`
- Writes the VM's public IP to `networker-cloud.json`

### Available Azure flags

| Flag | Default | Description |
|------|---------|-------------|
| `--region REGION` | `eastus` | Azure region |
| `--rg NAME` | `networker-rg-endpoint` | Resource group name |
| `--vm NAME` | auto-generated | VM name (smart naming, collision detection) |
| `--vm-size SIZE` | `Standard_B2s` | Azure VM size |

### Deploy the tester to Azure

```bash
bash install.sh --tester-azure tester \
  --tester-rg networker-rg-tester \
  --tester-vm networker-tester-vm
```

### Run a local-vs-Azure comparison

```bash
# Start the local endpoint
networker-endpoint

# Run the tester against both
networker-tester \
  --target http://127.0.0.1:8080/health \
  --target https://<azure-vm-ip>:8443/health \
  --modes tcp,http1,http2,http3,udp,download,pageload,pageload2,pageload3 \
  --payload-sizes 1m \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

---

## AWS Deployment

### Prerequisites

- AWS CLI (`aws`) installed and configured (`aws configure`)
- An active AWS account with EC2 permissions

### Deploy the endpoint

```bash
# Interactive
bash install.sh --aws endpoint

# With explicit options
bash install.sh --aws endpoint \
  --aws-region us-east-1 \
  --aws-instance-type t3.small \
  --aws-endpoint-name networker-endpoint
```

The installer:
- Provisions an EC2 instance with the given name tag
- Configures a security group opening ports 8080, 8443 (TCP) and 8443 (UDP)
- Deploys and starts `networker-endpoint`
- Writes the instance's public IP to `networker-cloud.json`

### Available AWS flags

| Flag | Default | Description |
|------|---------|-------------|
| `--aws-region REGION` | `us-east-1` | AWS region |
| `--aws-instance-type TYPE` | `t3.small` | EC2 instance type |
| `--aws-endpoint-name NAME` | `networker-endpoint` | EC2 Name tag |
| `--aws-tester-name NAME` | `networker-tester` | EC2 Name tag for tester VM |

### Deploy the tester to AWS

```bash
bash install.sh --tester-aws tester \
  --aws-region us-east-1
```

---

## Auto-Shutdown

To avoid unexpected cloud costs, the installer configures automatic VM shutdown at 11 PM EST by default.

### Azure

The installer uses `az vm auto-shutdown` to set a native Azure auto-shutdown schedule. You can change the time in the Azure Portal under the VM's "Auto-shutdown" blade, or update it via CLI:

```bash
az vm auto-shutdown \
  --resource-group networker-rg \
  --name networker-endpoint-vm \
  --time 2300 \
  --timezone "Eastern Standard Time"
```

Disable auto-shutdown:

```bash
az vm auto-shutdown \
  --resource-group networker-rg \
  --name networker-endpoint-vm \
  --off
```

### AWS

The installer adds a cron job on the EC2 instance to power down at 11 PM UTC:

```bash
# 23:00 UTC shutdown (already configured by the installer)
# To change it, SSH into the instance and edit the crontab:
crontab -e
# Example: shutdown at 22:00 UTC instead
0 22 * * * /sbin/shutdown -h now
```

---

## Multi-Region Comparison

Run the same test against endpoints in multiple cloud regions to measure WAN latency and regional throughput differences.

### 1. Deploy endpoints in multiple regions

```bash
# US East
bash install.sh --azure endpoint --region eastus --rg networker-rg-eastus

# UK South
bash install.sh --azure endpoint --region uksouth --rg networker-rg-uksouth

# Southeast Asia
bash install.sh --azure endpoint --region southeastasia --rg networker-rg-sea
```

### 2. Run a multi-target comparison

```bash
networker-tester \
  --target http://127.0.0.1:8080/health \
  --target https://<eastus-ip>:8443/health \
  --target https://<uksouth-ip>:8443/health \
  --target https://<sea-ip>:8443/health \
  --modes tcp,http1,http2,http3,download \
  --payload-sizes 1m \
  --runs 10 \
  --insecure \
  --output-dir ./output
```

### 3. Read the HTML report

The report opens with a **Cross-Target Protocol Comparison** table — one row per mode, one column per target, with % delta vs the first (local) baseline:

| What to compare | What it tells you |
|-----------------|-------------------|
| `tcp` connect_ms | Raw TCP round-trip to each region |
| `http1`/`http2`/`http3` total_ms | Protocol overhead added on top of RTT |
| `download` throughput | Available bandwidth to each region |
| `pageload`/`pageload2`/`pageload3` total_ms | Real page-load impact at WAN latency |

**Typical patterns:**
- WAN HTTP total_ms is 5-50x higher than loopback
- H3 may show a larger delta than H2 on high-latency links due to QUIC handshake overhead
- Download throughput is limited by the uplink/downlink of both sides

---

## Troubleshooting

### Health check times out after deployment

The installer runs an SSH health check and prints diagnostics if the endpoint does not respond. Common causes:

- **Firewall not yet propagated**: wait 30-60 seconds and retry
- **UDP port blocked**: HTTP/3 requires UDP 8443; check NSG / security group rules
- **Wrong SSH key**: the installer uses your default SSH key; set `NETWORKER_SSH_KEY` to override

### Source-build fallback

If no pre-built binary is available for the VM's OS/architecture, the installer falls back to compiling from source on the VM itself. This adds 2-10 minutes and requires a Rust toolchain to be installed on the VM (the installer handles this automatically).

### Azure NSG port conflicts

The installer detects priority conflicts when adding NSG rules and picks the next available priority automatically.
