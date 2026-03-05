#!/usr/bin/env bats
# Integration test: deploy networker-endpoint on a fresh Azure Ubuntu VM via the installer.
#
# What it does:
#   1. Creates a dedicated resource group + Standard_B2s Ubuntu 22.04 VM
#   2. Opens firewall ports (TCP 80/443/8080/8443, UDP 8443/9998/9999)
#   3. Uploads the installer and runs it on the VM non-interactively
#   4. Asserts the binary version, systemd service, and /health endpoint
#   5. Tears down the resource group (always, even on failure)
#
# Prerequisites:
#   - az login already done  (az account show works)
#   - SSH key at ~/.ssh/id_ed25519 or ~/.ssh/id_rsa
#   - INSTALLER path set (or falls back to repo root install.sh)
#   - bats-core  (brew install bats-core)
#
# Run:
#   cd tests/integration && ./run.sh azure
#   # or directly:
#   INSTALLER=/path/to/install.sh bats tests/integration/azure/endpoint_deploy.bats

load "../helpers/vm_helpers"

# ---------------------------------------------------------------------------
# Config — override via env vars for CI / custom environments
# ---------------------------------------------------------------------------
REGION="${AZURE_REGION:-eastus}"
SIZE="${AZURE_SIZE:-Standard_B2s}"
SSH_USER="azureuser"
INSTALLER="${INSTALLER:-$(cd "$(dirname "$BATS_TEST_FILENAME")/../../.." && pwd)/install.sh}"

# Unique RG per run so parallel runs and leftover VMs don't collide
RG="nwk-inttest-$(date +%s)"
VM="nwk-ep-lnx-b1s"

# ---------------------------------------------------------------------------
# setup_file — runs once before all tests; creates VM
# ---------------------------------------------------------------------------
setup_file() {
    # Verify prerequisites
    if ! az account show --output none 2>/dev/null; then
        echo "ERROR: not logged in to Azure — run 'az login' first" >&2
        exit 1
    fi
    if [[ ! -f "$INSTALLER" ]]; then
        echo "ERROR: installer not found at $INSTALLER" >&2
        exit 1
    fi

    export RG VM REGION SIZE SSH_USER INSTALLER

    echo "=== Creating resource group: $RG in $REGION ===" >&3
    az group create --name "$RG" --location "$REGION" --output none

    # Pick whichever public key exists
    local auth_opt="--generate-ssh-keys"
    for key in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
        [[ -f "$key" ]] && { auth_opt="--ssh-key-values @${key}"; break; }
    done

    echo "=== Creating VM: $VM ($SIZE, Ubuntu 22.04) ===" >&3
    VM_IP="$(az vm create \
        --resource-group "$RG" \
        --name "$VM" \
        --image Ubuntu2204 \
        --size "$SIZE" \
        --admin-username "$SSH_USER" \
        $auth_opt \
        --only-show-errors \
        --output tsv \
        --query publicIpAddress)"
    export VM_IP
    echo "    VM IP: $VM_IP" >&3

    echo "=== Opening firewall ports ===" >&3
    local nsg
    nsg="$(az network nsg list --resource-group "$RG" \
        --query "[0].name" -o tsv 2>/dev/null || echo "")"
    if [[ -n "$nsg" && "$nsg" != "None" ]]; then
        az network nsg rule create \
            --resource-group "$RG" --nsg-name "$nsg" \
            --name Networker-TCP --protocol Tcp --direction Inbound \
            --priority 1100 --destination-port-ranges 80 443 8080 8443 \
            --access Allow --output none
        az network nsg rule create \
            --resource-group "$RG" --nsg-name "$nsg" \
            --name Networker-UDP --protocol Udp --direction Inbound \
            --priority 1110 --destination-port-ranges 8443 9998 9999 \
            --access Allow --output none
    fi

    echo "=== Waiting for SSH on $VM_IP ===" >&3
    wait_for_ssh "$VM_IP" "$SSH_USER" 180

    echo "=== Uploading installer to VM ===" >&3
    scp -o StrictHostKeyChecking=no -q "$INSTALLER" \
        "${SSH_USER}@${VM_IP}:/tmp/networker-install.sh"

    echo "=== Running installer on VM (endpoint -y) ===" >&3
    # -t allocates a pseudo-TTY so the spinner/colour codes work
    ssh -t -o StrictHostKeyChecking=no "${SSH_USER}@${VM_IP}" \
        "bash /tmp/networker-install.sh endpoint -y" >&3 2>&3
}

# ---------------------------------------------------------------------------
# teardown_file — runs once after all tests; always deletes RG
# ---------------------------------------------------------------------------
teardown_file() {
    echo "=== Deleting resource group: $RG ===" >&3
    az group delete --name "$RG" --yes --no-wait 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

@test "networker-endpoint binary is installed on the VM" {
    local ver
    ver="$(ssh_run "$VM_IP" "$SSH_USER" "networker-endpoint --version 2>/dev/null || ~/.cargo/bin/networker-endpoint --version 2>/dev/null")"
    [[ "$ver" == networker-endpoint* ]]
}

@test "networker-endpoint version matches expected release" {
    local expected
    # Read the version from the workspace Cargo.toml next to the installer
    expected="$(grep -E '^version\s*=' "$(dirname "$INSTALLER")/Cargo.toml" \
                | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')"
    local actual
    actual="$(ssh_run "$VM_IP" "$SSH_USER" \
        "networker-endpoint --version 2>/dev/null || ~/.cargo/bin/networker-endpoint --version 2>/dev/null" \
        | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')"
    [[ "$actual" == "$expected" ]]
}

@test "networker-endpoint systemd service is active" {
    local status
    status="$(ssh_run "$VM_IP" "$SSH_USER" \
        "systemctl is-active networker-endpoint 2>/dev/null")"
    [[ "$status" == "active" ]]
}

@test "networker-endpoint responds on /health (port 8080)" {
    local resp
    resp="$(curl -sf --max-time 10 "http://${VM_IP}:8080/health" 2>/dev/null)"
    [[ -n "$resp" ]]
    echo "$resp" | grep -qi "ok\|healthy\|networker"
}

@test "networker-endpoint responds on HTTPS /health (port 8443)" {
    local resp
    resp="$(curl -sfk --max-time 10 "https://${VM_IP}:8443/health" 2>/dev/null)"
    [[ -n "$resp" ]]
    echo "$resp" | grep -qi "ok\|healthy\|networker"
}

@test "networker-endpoint landing page responds on port 80 (iptables redirect)" {
    local http_code
    http_code="$(curl -so /dev/null -w "%{http_code}" --max-time 10 "http://${VM_IP}/" 2>/dev/null)"
    [[ "$http_code" == "200" ]]
}

@test "networker-tester on local machine can reach the VM endpoint" {
    if ! command -v networker-tester &>/dev/null; then
        skip "networker-tester not installed locally"
    fi
    networker-tester \
        --target "https://${VM_IP}:8443/health" \
        --modes http1 \
        --runs 3 \
        --insecure \
        --quiet
}
