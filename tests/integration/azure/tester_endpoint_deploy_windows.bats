#!/usr/bin/env bats
# Integration test: deploy networker-endpoint on one Azure Windows Server 2022 VM and
# networker-tester on a second, run the tester against the endpoint, save the report.
#
# What it does:
#   1. Creates a shared resource group with two Windows Server 2022 VMs:
#      - nwk-ep-win   (endpoint, Standard_B4ms, Win2022Datacenter)
#      - nwk-ts-win   (tester,   Standard_B4ms, Win2022Datacenter)
#   2. Opens Azure NSG ports on the endpoint VM
#   3. Waits for both VMs' Azure agent to be responsive (run-command polling)
#   4. Installs both components IN PARALLEL via az vm run-command invoke:
#      - Each VM: VS Build Tools → Rust → cargo install → (endpoint only) service + firewall
#      - Total setup time: ~35–50 min (wall clock, both VMs compile simultaneously)
#   5. Runs networker-tester on the tester VM → endpoint VM via run-command
#   6. Downloads JSON report to results/ via base64 encode/decode
#   7. Tears down: az group delete --yes --no-wait (no ongoing cost)
#
# Prerequisites:
#   - az login already done
#   - jq available (brew install jq)
#   - AZURE_REGION / AZURE_WIN_SIZE env vars optional
#   - bats-core  (brew install bats-core)
#
# Run:
#   AZURE_REGION=eastus bats tests/integration/azure/tester_endpoint_deploy_windows.bats
#
# Note: setup_file takes ~35–50 min due to Rust compilation on Windows VMs.

load "../helpers/vm_helpers"

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
REGION="${AZURE_REGION:-eastus}"
SIZE="${AZURE_WIN_SIZE:-Standard_B4ms}"   # 4 vCPU recommended for Rust compile
WIN_USER="azureuser"
RESULTS_DIR="${RESULTS_DIR:-$(cd "$(dirname "$BATS_TEST_FILENAME")/../../.." && pwd)/tests/integration/results}"
REPO_HTTPS="https://github.com/irlm/networker-tester"

EP_VM="nwk-ep-win"
TS_VM="nwk-ts-win"

# ---------------------------------------------------------------------------
# Helpers: persist/load state across setup_file → tests → teardown_file
# ---------------------------------------------------------------------------
_save() { echo "$2" > "${BATS_FILE_TMPDIR}/${1}"; }
_load() { cat "${BATS_FILE_TMPDIR}/${1}" 2>/dev/null || echo ""; }

# Run inline PowerShell on a Windows Azure VM.
# Outputs only the [stdout] section of the response.
_az_ps() {
    local rg="$1" vm="$2" script="$3"
    az vm run-command invoke \
        --resource-group "$rg" --name "$vm" \
        --command-id RunPowerShellScript \
        --scripts "$script" \
        --output json 2>/dev/null \
    | jq -r '.value[0].message // ""' \
    | awk '/\[stdout\]/{p=1;next} /\[stderr\]/{p=0} p'
}

# Run a PowerShell script file on a Windows Azure VM.
# Outputs only the [stdout] section of the response.
_az_ps_file() {
    local rg="$1" vm="$2" ps_file="$3"
    az vm run-command invoke \
        --resource-group "$rg" --name "$vm" \
        --command-id RunPowerShellScript \
        --scripts "@${ps_file}" \
        --output json 2>/dev/null \
    | jq -r '.value[0].message // ""' \
    | awk '/\[stdout\]/{p=1;next} /\[stderr\]/{p=0} p'
}

# Poll az run-command until the VM's Azure agent responds (max ~10 min).
_wait_for_windows_vm() {
    local rg="$1" vm="$2" label="${3:-$vm}"
    echo "=== Waiting for $label Windows VM to be ready ===" >&3
    local attempts=0
    while true; do
        local out
        out="$(az vm run-command invoke \
            --resource-group "$rg" --name "$vm" \
            --command-id RunPowerShellScript \
            --scripts 'Write-Host ready' \
            --output json 2>/dev/null \
          | jq -r '.value[0].message // ""' 2>/dev/null)"
        if echo "$out" | grep -qi "ready"; then
            echo "    $label: agent ready" >&3
            return 0
        fi
        attempts=$(( attempts + 1 ))
        if [[ $attempts -ge 20 ]]; then
            echo "ERROR: $label Windows VM not ready after 10 minutes" >&2
            return 1
        fi
        sleep 30
    done
}

# ---------------------------------------------------------------------------
# setup_file — creates both VMs, compiles and installs both components
# ---------------------------------------------------------------------------
setup_file() {
    if ! az account show --output none 2>/dev/null; then
        echo "ERROR: not logged in to Azure — run 'az login' first" >&2
        exit 1
    fi
    if ! command -v jq &>/dev/null; then
        echo "ERROR: jq is required (brew install jq)" >&2
        exit 1
    fi

    local rg="nwk-win-inttest-$(date +%s)"
    _save rg "$rg"

    # Password must meet Windows complexity: upper + lower + digit + special, ≥ 12 chars
    local win_pass="NwkIntT3st$(openssl rand -hex 4)Aa!"
    _save win_pass "$win_pass"

    echo "=== Creating resource group: $rg in $REGION ===" >&3
    az group create --name "$rg" --location "$REGION" --output none

    # --- Create endpoint VM ---
    echo "=== Creating endpoint VM: $EP_VM ($SIZE, Win2022) ===" >&3
    local ep_ip
    ep_ip="$(az vm create \
        --resource-group "$rg" --name "$EP_VM" \
        --image Win2022Datacenter --size "$SIZE" \
        --admin-username "$WIN_USER" \
        --admin-password "$(_load win_pass)" \
        --only-show-errors --output tsv --query publicIpAddress)"
    _save ep_ip "$ep_ip"
    echo "    Endpoint IP: $ep_ip" >&3

    # Save the private IP for intra-VNet probes (tester→endpoint within same VNet).
    # In Azure, VMs in the same VNet cannot reach each other via public IP.
    local ep_private_ip
    ep_private_ip="$(az vm list-ip-addresses \
        --resource-group "$rg" --name "$EP_VM" \
        --query '[0].virtualMachine.network.privateIpAddresses[0]' -o tsv 2>/dev/null)"
    _save ep_private_ip "$ep_private_ip"
    echo "    Endpoint private IP: $ep_private_ip" >&3

    # Open Azure NSG ports on endpoint VM
    local ep_nsg
    ep_nsg="$(az network nsg list --resource-group "$rg" \
        --query "[?contains(name,'${EP_VM}')].name | [0]" -o tsv 2>/dev/null || echo "")"
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
        echo "    NSG rules added on $ep_nsg" >&3
    fi

    # --- Create tester VM ---
    echo "=== Creating tester VM: $TS_VM ($SIZE, Win2022) ===" >&3
    local ts_ip
    ts_ip="$(az vm create \
        --resource-group "$rg" --name "$TS_VM" \
        --image Win2022Datacenter --size "$SIZE" \
        --admin-username "$WIN_USER" \
        --admin-password "$(_load win_pass)" \
        --only-show-errors --output tsv --query publicIpAddress)"
    _save ts_ip "$ts_ip"
    echo "    Tester IP: $ts_ip" >&3

    # --- Wait for both VM agents ---
    _wait_for_windows_vm "$rg" "$EP_VM" "endpoint"
    _wait_for_windows_vm "$rg" "$TS_VM" "tester"

    # -------------------------------------------------------------------------
    # Write PowerShell install scripts to local temp files.
    # CARGO_HOME=C:\cargo so the SYSTEM account (which run-command uses) gets a
    # predictable binary path that doesn't vary by user profile.
    # -------------------------------------------------------------------------
    local ep_ps ts_ps
    ep_ps="$(mktemp /tmp/nwk-ep-XXXXX.ps1)"
    ts_ps="$(mktemp /tmp/nwk-ts-XXXXX.ps1)"

    # --- Endpoint install script (VS Build Tools + Rust + endpoint + service + firewall) ---
    cat > "$ep_ps" <<'PSEOF'
$ErrorActionPreference = 'Continue'
$env:RUSTUP_HOME = 'C:\rustup'
$env:CARGO_HOME  = 'C:\cargo'
[Net.ServicePointManager]::SecurityProtocol = 'Tls12'

Write-Host "=== Step 1: Install VS Build Tools (with Windows SDK) ==="
Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vs_BuildTools.exe' `
    -OutFile 'C:\vs_buildtools.exe' -UseBasicParsing
& 'C:\vs_buildtools.exe' --quiet --wait --norestart `
    --add Microsoft.VisualStudio.Workload.VCTools `
    --includeRecommended 2>&1 | Out-Null
Write-Host "VS Build Tools exit: $LASTEXITCODE"
Remove-Item 'C:\vs_buildtools.exe' -Force -ErrorAction SilentlyContinue

Write-Host "=== Step 2: Install Rust ==="
Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile 'C:\rustup-init.exe' -UseBasicParsing
& 'C:\rustup-init.exe' -y --no-modify-path --profile minimal --default-toolchain stable 2>&1 | Write-Host
Remove-Item 'C:\rustup-init.exe' -Force -ErrorAction SilentlyContinue

$env:PATH = "C:\cargo\bin;$env:PATH"

Write-Host "=== Step 3: cargo install networker-endpoint (via vcvars64 env) ==="
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
Write-Host "vcvars64 exists: $(Test-Path $vcvars)"
cmd /c "`"$vcvars`" && C:\cargo\bin\cargo.exe install --git https://github.com/irlm/networker-tester networker-endpoint --force 2>&1"
Write-Host "cargo exit: $LASTEXITCODE"

Write-Host "=== Step 4: Start networker-endpoint ==="
netsh advfirewall firewall add rule name='Networker-TCP' protocol=TCP dir=in action=allow localport='80,443,8080,8443' 2>&1 | Out-Null
netsh advfirewall firewall add rule name='Networker-UDP' protocol=UDP dir=in action=allow localport='8443,9998,9999' 2>&1 | Out-Null
Write-Host "Firewall rules added"

Start-Process -FilePath 'C:\cargo\bin\networker-endpoint.exe' -WindowStyle Hidden
Start-Sleep -Seconds 5
$proc = Get-Process networker-endpoint -ErrorAction SilentlyContinue
if ($proc) { Write-Host "networker-endpoint: RUNNING (PID $($proc.Id))" }
else        { Write-Host "networker-endpoint: NOT RUNNING" }

$ver = & 'C:\cargo\bin\networker-endpoint.exe' --version 2>&1
Write-Host "Endpoint version: $ver"
PSEOF

    # --- Tester install script (VS Build Tools + Rust + tester only) ---
    cat > "$ts_ps" <<'PSEOF'
$ErrorActionPreference = 'Continue'
$env:RUSTUP_HOME = 'C:\rustup'
$env:CARGO_HOME  = 'C:\cargo'
[Net.ServicePointManager]::SecurityProtocol = 'Tls12'

Write-Host "=== Step 1: Install VS Build Tools (with Windows SDK) ==="
Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vs_BuildTools.exe' `
    -OutFile 'C:\vs_buildtools.exe' -UseBasicParsing
& 'C:\vs_buildtools.exe' --quiet --wait --norestart `
    --add Microsoft.VisualStudio.Workload.VCTools `
    --includeRecommended 2>&1 | Out-Null
Write-Host "VS Build Tools exit: $LASTEXITCODE"
Remove-Item 'C:\vs_buildtools.exe' -Force -ErrorAction SilentlyContinue

Write-Host "=== Step 2: Install Rust ==="
Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile 'C:\rustup-init.exe' -UseBasicParsing
& 'C:\rustup-init.exe' -y --no-modify-path --profile minimal --default-toolchain stable 2>&1 | Write-Host
Remove-Item 'C:\rustup-init.exe' -Force -ErrorAction SilentlyContinue

$env:PATH = "C:\cargo\bin;$env:PATH"

Write-Host "=== Step 3: cargo install networker-tester (via vcvars64 env) ==="
$vcvars = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat'
Write-Host "vcvars64 exists: $(Test-Path $vcvars)"
cmd /c "`"$vcvars`" && C:\cargo\bin\cargo.exe install --git https://github.com/irlm/networker-tester networker-tester --force 2>&1"
Write-Host "cargo exit: $LASTEXITCODE"

$ver = & 'C:\cargo\bin\networker-tester.exe' --version 2>&1
Write-Host "Tester version: $ver"
PSEOF

    # -------------------------------------------------------------------------
    # Run both installs in parallel (each takes ~35–50 min; wall clock = same).
    # Output is captured to log files so we can show it after both finish.
    # -------------------------------------------------------------------------
    echo "=== Installing endpoint + tester in parallel (~35-50 min) ===" >&3

    local ep_log ts_log
    ep_log="$(mktemp)"
    ts_log="$(mktemp)"

    az vm run-command invoke \
        --resource-group "$rg" --name "$EP_VM" \
        --command-id RunPowerShellScript \
        --scripts "@${ep_ps}" \
        --output json > "$ep_log" 2>&1 &
    local ep_pid=$!

    az vm run-command invoke \
        --resource-group "$rg" --name "$TS_VM" \
        --command-id RunPowerShellScript \
        --scripts "@${ts_ps}" \
        --output json > "$ts_log" 2>&1 &
    local ts_pid=$!

    wait $ep_pid; local ep_rc=$?
    wait $ts_pid; local ts_rc=$?

    echo "=== Endpoint install output ===" >&3
    jq -r '.value[0].message // "no output"' < "$ep_log" 2>/dev/null >&3 || cat "$ep_log" >&3
    echo "=== Tester install output ===" >&3
    jq -r '.value[0].message // "no output"' < "$ts_log" 2>/dev/null >&3 || cat "$ts_log" >&3

    rm -f "$ep_ps" "$ts_ps" "$ep_log" "$ts_log"

    [[ $ep_rc -eq 0 ]] || { echo "ERROR: endpoint install command failed (rc=$ep_rc)" >&2; exit 1; }
    [[ $ts_rc -eq 0 ]] || { echo "ERROR: tester install command failed (rc=$ts_rc)" >&2; exit 1; }

    # -------------------------------------------------------------------------
    # Run the tester against the endpoint
    # -------------------------------------------------------------------------
    echo "=== Running networker-tester on tester VM → endpoint VM ===" >&3
    local run_ps ep_priv_ip_val
    ep_priv_ip_val="$(_load ep_private_ip)"
    run_ps="$(mktemp /tmp/nwk-run-XXXXX.ps1)"

    # Use unquoted heredoc: bash expands ${ep_priv_ip_val}; PowerShell vars escaped with \$
    # Use private IP: in Azure, VMs in the same VNet cannot reach each other via public IP.
    # IMPORTANT: $ErrorActionPreference='Continue' prevents 2>&1 ErrorRecords from being
    # treated as terminating errors (Azure's RunPowerShellScript default is 'Stop').
    # Capture output to variable first, then Write-Host — avoids pipe + ErrorRecord issues.
    cat > "$run_ps" <<PSEOF
\$ErrorActionPreference = 'Continue'
\$env:PATH = "C:\\cargo\\bin;\$env:PATH"
\$outDir = 'C:\\networker-report'
New-Item -ItemType Directory -Force -Path \$outDir | Out-Null
Write-Host "Running tester against http://${ep_priv_ip_val}:8080/health ..."
\$r = & 'C:\\cargo\\bin\\networker-tester.exe' --target 'http://${ep_priv_ip_val}:8080/health' --modes http1 --runs 3 --output-dir \$outDir 2>&1
Write-Host "tester exit: \$LASTEXITCODE"
Write-Host "\$r"
PSEOF

    local run_out
    run_out="$(_az_ps_file "$rg" "$TS_VM" "$run_ps")"
    echo "$run_out" >&3
    rm -f "$run_ps"

    # -------------------------------------------------------------------------
    # Download JSON report via base64
    # -------------------------------------------------------------------------
    local dl_ps
    dl_ps="$(mktemp /tmp/nwk-dl-XXXXX.ps1)"
    cat > "$dl_ps" <<'PSEOF'
$f = Get-ChildItem 'C:\networker-report\run-*.json' -ErrorAction SilentlyContinue |
     Sort-Object LastWriteTime | Select-Object -Last 1
if ($f) {
    Write-Host ([Convert]::ToBase64String([System.IO.File]::ReadAllBytes($f.FullName)))
} else {
    Write-Host "NO_REPORT"
}
PSEOF

    mkdir -p "$RESULTS_DIR"
    local timestamp; timestamp="$(date -u +%Y-%m-%dT%H-%M-%S)"
    local local_report="${RESULTS_DIR}/azure-${REGION}-Win2022-${timestamp}.json"
    _save local_report "$local_report"

    local b64
    b64="$(_az_ps_file "$rg" "$TS_VM" "$dl_ps" | tr -d '\n\r ')"
    rm -f "$dl_ps"

    if [[ -n "$b64" && "$b64" != "NO_REPORT" ]]; then
        echo "$b64" | base64 --decode > "$local_report" 2>/dev/null
        if [[ -s "$local_report" ]]; then
            echo "=== Report saved locally: $local_report ===" >&3
        else
            echo "    (report decode failed)" >&3
            rm -f "$local_report"
        fi
    else
        echo "    (no report produced by tester)" >&3
    fi

    # Re-confirm VM agents are ready for the @test assertions that follow.
    # After the long install + probe sessions the agents need a moment to reset.
    echo "=== Re-confirming VM agents are ready for assertions ===" >&3
    _wait_for_windows_vm "$rg" "$EP_VM" "endpoint"
    _wait_for_windows_vm "$rg" "$TS_VM" "tester"
}

# ---------------------------------------------------------------------------
# teardown_file — delete entire resource group (VMs + disks + NSGs + IPs)
# ---------------------------------------------------------------------------
teardown_file() {
    local rg; rg="$(_load rg)"
    if [[ -n "$rg" ]]; then
        echo "=== Deleting resource group: $rg ===" >&3
        az group delete --name "$rg" --yes --no-wait 2>/dev/null || true
        echo "    Deletion started — all resources will be removed by Azure." >&3
    fi
}

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

@test "networker-endpoint is listening on port 8080 on endpoint VM" {
    local rg; rg="$(_load rg)"
    local ps_file; ps_file="$(mktemp /tmp/nwk-t1-XXXXX.ps1)"
    # Use PS-native Get-NetTCPConnection (no cmd.exe) and variable-capture to avoid
    # pipe+ErrorRecord terminating errors under Azure's default $ErrorActionPreference='Stop'.
    cat > "$ps_file" <<'PS'
$ErrorActionPreference = 'Continue'
Write-Host "Checking port 8080..."
$conn = Get-NetTCPConnection -LocalPort 8080 -ErrorAction SilentlyContinue
if ($conn) {
    Write-Host "LISTENING :8080 (state=$($conn[0].State))"
} else {
    Write-Host "NOT_LISTENING :8080"
}
PS
    local out
    out="$(_az_ps_file "$rg" "$EP_VM" "$ps_file")"
    rm -f "$ps_file"
    echo "port 8080 check: $out"
    [[ "$out" == *"LISTENING :8080"* ]]
}

@test "networker-endpoint responds on /health (port 8080)" {
    local ep_ip; ep_ip="$(_load ep_ip)"
    local resp
    resp="$(curl -sf --max-time 15 "http://${ep_ip}:8080/health" 2>/dev/null)"
    [[ -n "$resp" ]]
    echo "$resp" | grep -qi "ok\|healthy\|networker"
}

@test "networker-tester binary is installed on tester VM" {
    local rg; rg="$(_load rg)"
    local ps_file; ps_file="$(mktemp /tmp/nwk-t3-XXXXX.ps1)"
    # Capture output to variable before Write-Host to avoid pipe+ErrorRecord issues.
    cat > "$ps_file" <<'PS'
$ErrorActionPreference = 'Continue'
Write-Host "Checking tester version..."
$v = & 'C:\cargo\bin\networker-tester.exe' --version 2>&1
Write-Host "$v"
PS
    local ver
    ver="$(_az_ps_file "$rg" "$TS_VM" "$ps_file")"
    rm -f "$ps_file"
    echo "tester version: $ver"
    [[ "$ver" == *networker-tester* ]]
}

@test "networker-tester can probe endpoint via HTTP/1.1 from tester VM" {
    local rg ep_ip
    rg="$(_load rg)"; ep_ip="$(_load ep_private_ip)"
    local ps_file; ps_file="$(mktemp /tmp/nwk-t4-XXXXX.ps1)"
    # Use private IP: Azure VMs in the same VNet cannot reach each other via public IP.
    # Variable capture avoids pipe+ErrorRecord terminating errors.
    cat > "$ps_file" <<PSEOF
\$ErrorActionPreference = 'Continue'
\$env:PATH = "C:\\cargo\\bin;\$env:PATH"
Write-Host "Probing http://${ep_ip}:8080/health via HTTP/1.1 ..."
\$r = & 'C:\\cargo\\bin\\networker-tester.exe' --target 'http://${ep_ip}:8080/health' --modes http1 --runs 3 2>&1
Write-Host "exit: \$LASTEXITCODE"
Write-Host "\$r"
PSEOF
    local out
    out="$(_az_ps_file "$rg" "$TS_VM" "$ps_file")"
    rm -f "$ps_file"
    echo "http1 probe: $out"
    echo "$out" | grep -qi "http1\|pass\|ms\|networker"
}

@test "networker-tester can probe endpoint via HTTP/2 from tester VM" {
    local rg ep_ip
    rg="$(_load rg)"; ep_ip="$(_load ep_private_ip)"
    local ps_file; ps_file="$(mktemp /tmp/nwk-t5-XXXXX.ps1)"
    # Use private IP: Azure VMs in the same VNet cannot reach each other via public IP.
    # Variable capture avoids pipe+ErrorRecord terminating errors.
    cat > "$ps_file" <<PSEOF
\$ErrorActionPreference = 'Continue'
\$env:PATH = "C:\\cargo\\bin;\$env:PATH"
Write-Host "Probing https://${ep_ip}:8443/health via HTTP/2 ..."
\$r = & 'C:\\cargo\\bin\\networker-tester.exe' --target 'https://${ep_ip}:8443/health' --modes http2 --runs 3 --insecure 2>&1
Write-Host "exit: \$LASTEXITCODE"
Write-Host "\$r"
PSEOF
    local out
    out="$(_az_ps_file "$rg" "$TS_VM" "$ps_file")"
    rm -f "$ps_file"
    echo "http2 probe: $out"
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
