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
#   AZURE_REGION=westeurope bats tests/integration/azure/endpoint_deploy.bats

load "../helpers/vm_helpers"

# ---------------------------------------------------------------------------
# Config — override via env vars for CI / custom environments
# ---------------------------------------------------------------------------
REGION="${AZURE_REGION:-eastus}"
SIZE="${AZURE_SIZE:-Standard_B2s}"
SSH_USER="azureuser"
INSTALLER="${INSTALLER:-$(cd "$(dirname "$BATS_TEST_FILENAME")/../../.." && pwd)/install.sh}"

# ---------------------------------------------------------------------------
# Helpers: persist/load state across setup_file → tests → teardown_file
# (bats runs each @test in a subshell so exported vars don't propagate)
# ---------------------------------------------------------------------------
_save() { echo "$2" > "${BATS_FILE_TMPDIR}/${1}"; }
_load() { cat "${BATS_FILE_TMPDIR}/${1}" 2>/dev/null || echo ""; }

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

    local rg="nwk-inttest-$(date +%s)"
    local vm="nwk-ep-lnx-b1s"
    _save rg "$rg"

    echo "=== Creating resource group: $rg in $REGION ===" >&3
    az group create --name "$rg" --location "$REGION" --output none

    # Pick whichever public key exists
    local auth_opt="--generate-ssh-keys"
    for key in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
        [[ -f "$key" ]] && { auth_opt="--ssh-key-values @${key}"; break; }
    done

    echo "=== Creating VM: $vm ($SIZE, Ubuntu 22.04) ===" >&3
    local vm_ip
    vm_ip="$(az vm create \
        --resource-group "$rg" \
        --name "$vm" \
        --image Ubuntu2204 \
        --size "$SIZE" \
        --admin-username "$SSH_USER" \
        $auth_opt \
        --only-show-errors \
        --output tsv \
        --query publicIpAddress)"
    _save vm_ip "$vm_ip"
    echo "    VM IP: $vm_ip" >&3

    echo "=== Opening firewall ports ===" >&3
    local nsg
    nsg="$(az network nsg list --resource-group "$rg" \
        --query "[0].name" -o tsv 2>/dev/null || echo "")"
    if [[ -n "$nsg" && "$nsg" != "None" ]]; then
        az network nsg rule create \
            --resource-group "$rg" --nsg-name "$nsg" \
            --name Networker-TCP --protocol Tcp --direction Inbound \
            --priority 1100 --destination-port-ranges 80 443 8080 8443 \
            --access Allow --output none
        az network nsg rule create \
            --resource-group "$rg" --nsg-name "$nsg" \
            --name Networker-UDP --protocol Udp --direction Inbound \
            --priority 1110 --destination-port-ranges 8443 9998 9999 \
            --access Allow --output none
    fi

    echo "=== Waiting for SSH on $vm_ip ===" >&3
    wait_for_ssh "$vm_ip" "$SSH_USER" 180

    echo "=== Uploading installer to VM ===" >&3
    scp -o StrictHostKeyChecking=no -q "$INSTALLER" \
        "${SSH_USER}@${vm_ip}:/tmp/networker-install.sh"

    echo "=== Running installer on VM (endpoint -y) ===" >&3
    ssh -t -o StrictHostKeyChecking=no "${SSH_USER}@${vm_ip}" \
        "bash /tmp/networker-install.sh endpoint -y" >&3 2>&3
}

# ---------------------------------------------------------------------------
# teardown_file — runs once after all tests; always deletes RG
# ---------------------------------------------------------------------------
teardown_file() {
    local rg; rg="$(_load rg)"
    if [[ -n "$rg" ]]; then
        echo "=== Deleting resource group: $rg ===" >&3
        az group delete --name "$rg" --yes --no-wait 2>/dev/null || true
    fi
}

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

@test "networker-endpoint binary is installed on the VM" {
    local vm_ip; vm_ip="$(_load vm_ip)"
    local ver
    ver="$(ssh_run "$vm_ip" "$SSH_USER" \
        "networker-endpoint --version 2>/dev/null || ~/.cargo/bin/networker-endpoint --version 2>/dev/null")"
    [[ "$ver" == networker-endpoint* ]]
}

@test "networker-endpoint version matches expected release" {
    local vm_ip; vm_ip="$(_load vm_ip)"
    local expected
    expected="$(grep -E '^version\s*=' "$(dirname "$INSTALLER")/Cargo.toml" \
                | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')"
    local actual
    actual="$(ssh_run "$vm_ip" "$SSH_USER" \
        "networker-endpoint --version 2>/dev/null || ~/.cargo/bin/networker-endpoint --version 2>/dev/null" \
        | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')"
    [[ "$actual" == "$expected" ]]
}

@test "networker-endpoint systemd service is active" {
    local vm_ip; vm_ip="$(_load vm_ip)"
    local status
    status="$(ssh_run "$vm_ip" "$SSH_USER" \
        "systemctl is-active networker-endpoint 2>/dev/null")"
    [[ "$status" == "active" ]]
}

@test "networker-endpoint responds on /health (port 8080)" {
    local vm_ip; vm_ip="$(_load vm_ip)"
    local resp
    resp="$(curl -sf --max-time 10 "http://${vm_ip}:8080/health" 2>/dev/null)"
    [[ -n "$resp" ]]
    echo "$resp" | grep -qi "ok\|healthy\|networker"
}

@test "networker-endpoint responds on HTTPS /health (port 8443)" {
    local vm_ip; vm_ip="$(_load vm_ip)"
    local resp
    resp="$(curl -sfk --max-time 10 "https://${vm_ip}:8443/health" 2>/dev/null)"
    [[ -n "$resp" ]]
    echo "$resp" | grep -qi "ok\|healthy\|networker"
}

@test "networker-endpoint landing page responds on port 80 (iptables redirect)" {
    local vm_ip; vm_ip="$(_load vm_ip)"
    local http_code
    http_code="$(curl -so /dev/null -w "%{http_code}" --max-time 10 "http://${vm_ip}/" 2>/dev/null)"
    [[ "$http_code" == "200" ]]
}

@test "networker-tester on local machine can reach the VM endpoint" {
    if ! command -v networker-tester &>/dev/null; then
        skip "networker-tester not installed locally"
    fi
    local vm_ip; vm_ip="$(_load vm_ip)"
    networker-tester \
        --target "https://${vm_ip}:8443/health" \
        --modes http1 \
        --runs 3 \
        --insecure \
        --quiet
}
