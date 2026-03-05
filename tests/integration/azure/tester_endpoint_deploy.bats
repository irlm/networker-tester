#!/usr/bin/env bats
# Integration test: deploy networker-endpoint on one Azure VM and networker-tester
# on a second VM, run the tester against the endpoint, save the report locally.
#
# What it does:
#   1. Creates a shared resource group with two VMs:
#      - nwk-ep-lnx-b1s   (endpoint, Standard_B2s, Ubuntu 22.04)
#      - nwk-ts-lnx-b1s   (tester,   Standard_B2s, Ubuntu 22.04)
#   2. Opens endpoint firewall ports (TCP 80/443/8080/8443, UDP 8443/9998/9999)
#   3. Installs networker-endpoint on the endpoint VM
#   4. Installs networker-tester on the tester VM
#   5. Runs networker-tester → saves JSON report on the tester VM
#   6. Downloads the report to results/ locally (named with timestamp+region+OS)
#   7. Tears down: deletes the entire resource group (VMs + disks + IPs — no ongoing cost)
#
# Prerequisites:
#   - az login already done
#   - SSH key at ~/.ssh/id_ed25519 or ~/.ssh/id_rsa
#   - INSTALLER path set (or falls back to repo root install.sh)
#   - bats-core  (brew install bats-core)
#
# Run:
#   cd tests/integration && ./run.sh azure
#   # or directly:
#   AZURE_REGION=westeurope bats tests/integration/azure/tester_endpoint_deploy.bats

load "../helpers/vm_helpers"

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
REGION="${AZURE_REGION:-eastus}"
SIZE="${AZURE_SIZE:-Standard_B2s}"
SSH_USER="azureuser"
INSTALLER="${INSTALLER:-$(cd "$(dirname "$BATS_TEST_FILENAME")/../../.." && pwd)/install.sh}"
RESULTS_DIR="${RESULTS_DIR:-$(cd "$(dirname "$BATS_TEST_FILENAME")/../../.." && pwd)/tests/integration/results}"

# ---------------------------------------------------------------------------
# Helpers: persist/load state across setup_file → tests → teardown_file
# (bats runs each @test in a subshell so exported vars don't propagate)
# ---------------------------------------------------------------------------
_save() { echo "$2" > "${BATS_FILE_TMPDIR}/${1}"; }
_load() { cat "${BATS_FILE_TMPDIR}/${1}" 2>/dev/null || echo ""; }

_install_component() {
    local component="$1" ip="$2"
    echo "=== Uploading installer to $component VM ($ip) ===" >&3
    scp -o StrictHostKeyChecking=no -q "$INSTALLER" \
        "${SSH_USER}@${ip}:/tmp/networker-install.sh"
    echo "=== Running installer on $component VM (${component} -y) ===" >&3
    ssh -t -o StrictHostKeyChecking=no "${SSH_USER}@${ip}" \
        "bash /tmp/networker-install.sh ${component} -y" >&3 2>&3
}

# ---------------------------------------------------------------------------
# setup_file — creates both VMs and installs both components
# ---------------------------------------------------------------------------
setup_file() {
    if ! az account show --output none 2>/dev/null; then
        echo "ERROR: not logged in to Azure — run 'az login' first" >&2
        exit 1
    fi
    if [[ ! -f "$INSTALLER" ]]; then
        echo "ERROR: installer not found at $INSTALLER" >&2
        exit 1
    fi

    local rg="nwk-inttest-$(date +%s)"
    local ep_vm="nwk-ep-lnx-b1s"
    local ts_vm="nwk-ts-lnx-b1s"
    _save rg "$rg"

    local auth_opt="--generate-ssh-keys"
    for key in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
        [[ -f "$key" ]] && { auth_opt="--ssh-key-values @${key}"; break; }
    done

    echo "=== Creating resource group: $rg in $REGION ===" >&3
    az group create --name "$rg" --location "$REGION" --output none

    # --- Create endpoint VM ---
    echo "=== Creating endpoint VM: $ep_vm ($SIZE) ===" >&3
    local ep_ip
    ep_ip="$(az vm create \
        --resource-group "$rg" --name "$ep_vm" \
        --image Ubuntu2204 --size "$SIZE" \
        --admin-username "$SSH_USER" \
        $auth_opt \
        --only-show-errors --output tsv --query publicIpAddress)"
    _save ep_ip "$ep_ip"
    echo "    Endpoint IP: $ep_ip" >&3

    # Open endpoint firewall ports
    local ep_nsg
    ep_nsg="$(az network nsg list --resource-group "$rg" \
        --query "[?contains(name,'${ep_vm}')].name | [0]" -o tsv 2>/dev/null || echo "")"
    [[ -z "$ep_nsg" || "$ep_nsg" == "None" ]] && \
        ep_nsg="$(az network nsg list --resource-group "$rg" \
            --query "[0].name" -o tsv 2>/dev/null || echo "")"
    if [[ -n "$ep_nsg" && "$ep_nsg" != "None" ]]; then
        az network nsg rule create \
            --resource-group "$rg" --nsg-name "$ep_nsg" \
            --name Networker-TCP --protocol Tcp --direction Inbound \
            --priority 1100 --destination-port-ranges 80 443 8080 8443 \
            --access Allow --output none
        az network nsg rule create \
            --resource-group "$rg" --nsg-name "$ep_nsg" \
            --name Networker-UDP --protocol Udp --direction Inbound \
            --priority 1110 --destination-port-ranges 8443 9998 9999 \
            --access Allow --output none
    fi

    # --- Create tester VM ---
    echo "=== Creating tester VM: $ts_vm ($SIZE) ===" >&3
    local ts_ip
    ts_ip="$(az vm create \
        --resource-group "$rg" --name "$ts_vm" \
        --image Ubuntu2204 --size "$SIZE" \
        --admin-username "$SSH_USER" \
        $auth_opt \
        --only-show-errors --output tsv --query publicIpAddress)"
    _save ts_ip "$ts_ip"
    echo "    Tester IP: $ts_ip" >&3

    # --- Install endpoint ---
    echo "=== Waiting for SSH on endpoint VM ($ep_ip) ===" >&3
    wait_for_ssh "$ep_ip" "$SSH_USER" 180
    _install_component "endpoint" "$ep_ip"

    # --- Install tester ---
    echo "=== Waiting for SSH on tester VM ($ts_ip) ===" >&3
    wait_for_ssh "$ts_ip" "$SSH_USER" 180
    _install_component "tester" "$ts_ip"

    # --- Run the tester and save the remote report path ---
    echo "=== Running networker-tester on tester VM → endpoint VM ===" >&3
    local remote_outdir="/tmp/nwk-inttest-report"
    ssh_run "$ts_ip" "$SSH_USER" \
        "mkdir -p ${remote_outdir} && \
         ~/.cargo/bin/networker-tester \
            --target https://${ep_ip}:8443/health \
            --modes http1,http2,http3 \
            --runs 5 \
            --insecure \
            --output-dir ${remote_outdir}" >&3 2>&3 || true

    # Find the JSON artifact written by the tester (run-<ts>.json)
    local remote_report
    remote_report="$(ssh_run "$ts_ip" "$SSH_USER" \
        "ls ${remote_outdir}/run-*.json 2>/dev/null | head -1" || echo "")"
    _save remote_report "${remote_report:-}"

    # --- Download report to local results/ directory ---
    mkdir -p "$RESULTS_DIR"
    local timestamp; timestamp="$(date -u +%Y-%m-%dT%H-%M-%S)"
    local local_report="${RESULTS_DIR}/azure-${REGION}-Ubuntu22-${timestamp}.json"
    _save local_report "$local_report"
    if [[ -n "$remote_report" ]] && scp -o StrictHostKeyChecking=no -q \
           "${SSH_USER}@${ts_ip}:${remote_report}" "$local_report" 2>/dev/null; then
        echo "=== Report saved locally: $local_report ===" >&3
    else
        echo "    (report download failed — tester may not have produced output)" >&3
    fi
}

# ---------------------------------------------------------------------------
# teardown_file — deletes the entire resource group (VMs, disks, IPs, NSGs).
# Using az group delete (not just az vm deallocate) ensures NO ongoing cost.
# ---------------------------------------------------------------------------
teardown_file() {
    local rg; rg="$(_load rg)"
    if [[ -n "$rg" ]]; then
        echo "=== Deleting resource group: $rg (VMs + all resources) ===" >&3
        # --no-wait starts deletion immediately; Azure completes it in ~2 min.
        # Resources stop accruing cost as soon as deletion begins.
        az group delete --name "$rg" --yes --no-wait 2>/dev/null || true
        echo "    Deletion started — all VMs and disks will be removed by Azure." >&3
    fi
}

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

@test "networker-endpoint service is active on endpoint VM" {
    local ep_ip; ep_ip="$(_load ep_ip)"
    local status
    status="$(ssh_run "$ep_ip" "$SSH_USER" \
        "systemctl is-active networker-endpoint 2>/dev/null")"
    [[ "$status" == "active" ]]
}

@test "networker-endpoint responds on /health (port 8080)" {
    local ep_ip; ep_ip="$(_load ep_ip)"
    local resp
    resp="$(curl -sf --max-time 10 "http://${ep_ip}:8080/health" 2>/dev/null)"
    [[ -n "$resp" ]]
    echo "$resp" | grep -qi "ok\|healthy\|networker"
}

@test "networker-tester binary is installed on tester VM" {
    local ts_ip; ts_ip="$(_load ts_ip)"
    local ver
    ver="$(ssh_run "$ts_ip" "$SSH_USER" \
        "networker-tester --version 2>/dev/null || ~/.cargo/bin/networker-tester --version 2>/dev/null")"
    [[ "$ver" == networker-tester* ]]
}

@test "networker-tester can probe endpoint via HTTP/1.1 from tester VM" {
    local ep_ip; ep_ip="$(_load ep_ip)"
    local ts_ip; ts_ip="$(_load ts_ip)"
    local out
    out="$(ssh_run "$ts_ip" "$SSH_USER" \
        "~/.cargo/bin/networker-tester \
            --target http://${ep_ip}:8080/health \
            --modes http1 --runs 3 2>&1")"
    echo "$out" | grep -qi "http1\|pass\|ms\|networker"
}

@test "networker-tester can probe endpoint via HTTP/2 from tester VM" {
    local ep_ip; ep_ip="$(_load ep_ip)"
    local ts_ip; ts_ip="$(_load ts_ip)"
    local out
    out="$(ssh_run "$ts_ip" "$SSH_USER" \
        "~/.cargo/bin/networker-tester \
            --target https://${ep_ip}:8443/health \
            --modes http2 --runs 3 --insecure 2>&1")"
    echo "$out" | grep -qi "http2\|pass\|ms\|networker"
}

@test "JSON report was generated and downloaded locally" {
    local local_report; local_report="$(_load local_report)"
    [[ -n "$local_report" ]]
    [[ -f "$local_report" ]]
    local size; size="$(wc -c < "$local_report" 2>/dev/null || echo 0)"
    [[ "$size" -gt 100 ]]
    echo "Report: $local_report" >&3
}

@test "JSON report contains expected fields" {
    local local_report; local_report="$(_load local_report)"
    [[ -f "$local_report" ]] || skip "report not downloaded"
    grep -q "http" "$local_report"
}
