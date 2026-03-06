#!/usr/bin/env bats
# Integration test: deploy networker-endpoint on one AWS EC2 Windows Server 2022
# instance and networker-tester on a second, run probes, save the report locally,
# then terminate both instances.
#
# What it does:
#   1. Looks up latest Windows Server 2022 AMI for the region
#   2. Creates IAM role + instance profile for SSM (idempotent)
#   3. Creates a shared security group with all required ports open
#   4. Launches two instances:
#      - endpoint  (t3.xlarge, Windows Server 2022)
#      - tester    (t3.xlarge, Windows Server 2022)
#   5. Waits for SSM agent to register on both instances
#   6. Installs both components IN PARALLEL via SSM (VS Build Tools + Rust + cargo):
#      - Endpoint: installs networker-endpoint, adds firewall rules, starts process
#      - Tester:   installs networker-tester
#      - Total setup time: ~35–50 min (both instances compile simultaneously)
#   7. Runs networker-tester on the tester instance → endpoint instance via SSM
#   8. Downloads JSON report to results/ via base64 encode/decode
#   9. Tears down: terminates both instances, waits, deletes security group
#
# Prerequisites:
#   - AWS credentials configured (aws sts get-caller-identity works)
#   - jq available (brew install jq)
#   - AWS_REGION env var optional (defaults to us-east-1)
#   - bats-core  (brew install bats-core)
#
# Run:
#   AWS_REGION=us-east-1 bats tests/integration/aws/tester_endpoint_deploy_windows.bats
#
# Note: setup_file takes ~35–50 min due to Rust compilation on Windows instances.

load "../helpers/vm_helpers"

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------
REGION="${AWS_REGION:-us-east-1}"
WIN_SIZE="${AWS_WIN_SIZE:-m7i-flex.large}"  # 2 vCPU, 4 GB — free-tier eligible x86 with enough RAM for Windows + Rust compile
SSM_ROLE="nwk-ssm-role"
SSM_PROFILE="nwk-ssm-profile"
RESULTS_DIR="${RESULTS_DIR:-$(cd "$(dirname "$BATS_TEST_FILENAME")/../../.." && pwd)/tests/integration/results}"
REPO_HTTPS="https://github.com/irlm/networker-tester"

# ---------------------------------------------------------------------------
# Helpers: persist/load state across setup_file → tests → teardown_file
# ---------------------------------------------------------------------------
_save() { echo "$2" > "${BATS_FILE_TMPDIR}/${1}"; }
_load() { cat "${BATS_FILE_TMPDIR}/${1}" 2>/dev/null || echo ""; }

# ---------------------------------------------------------------------------
# SSM helpers
# ---------------------------------------------------------------------------

# Send a PowerShell script string via SSM and wait; print StandardOutputContent.
# $1 = instance_id  $2 = PS script string  $3 = timeout_min (default 2)
# Uses JSON {"commands":[...]} format so AWS CLI does proper JSON parsing (no \n issues).
_ssm_ps() {
    local instance_id="$1" script="$2" timeout_min="${3:-2}"
    # Build proper JSON parameters object: {"commands":["script"]}
    local params
    params="$(jq -Rn --arg s "$script" '{"commands":[$s]}')"

    local cmd_id
    cmd_id="$(aws ssm send-command \
        --region "$REGION" \
        --instance-ids "$instance_id" \
        --document-name "AWS-RunPowerShellScript" \
        --parameters "$params" \
        --timeout-seconds $(( timeout_min * 60 )) \
        --query "Command.CommandId" \
        --output text 2>/dev/null || echo "")"
    [[ -z "$cmd_id" ]] && { echo "ERROR: send-command failed" >&2; return 1; }

    _ssm_wait "$instance_id" "$cmd_id" "$timeout_min"
    aws ssm get-command-invocation \
        --region "$REGION" \
        --command-id "$cmd_id" \
        --instance-id "$instance_id" \
        --query "StandardOutputContent" \
        --output text 2>/dev/null || echo ""
}

# Send a PowerShell script FILE via SSM (async — returns cmd_id immediately).
# $1 = instance_id  $2 = ps_file  $3 = timeout_min (default 90)
# Echoes the command-id to stdout for later polling.
# Uses line-split approach: each non-empty line becomes an array element.
# AWS-RunPowerShellScript writes these as separate lines to a .ps1 file on the instance,
# preserving PS backtick line-continuation and multi-line constructs.
_ssm_ps_file_async() {
    local instance_id="$1" ps_file="$2" timeout_min="${3:-90}"
    # Split PS file into one JSON string per non-empty line, then build {"commands":[...]}
    local commands_array params
    commands_array="$(jq -Rs '[split("\n")[] | select(length > 0)]' "$ps_file")"
    params="{\"commands\":${commands_array}}"

    aws ssm send-command \
        --region "$REGION" \
        --instance-ids "$instance_id" \
        --document-name "AWS-RunPowerShellScript" \
        --parameters "$params" \
        --timeout-seconds $(( timeout_min * 60 )) \
        --query "Command.CommandId" \
        --output text 2>/dev/null || echo ""
}

# Wait for an SSM command to reach a terminal state.
# $1 = instance_id  $2 = cmd_id  $3 = timeout_min (default 90)
_ssm_wait() {
    local instance_id="$1" cmd_id="$2" timeout_min="${3:-90}"
    local deadline=$(( $(date +%s) + timeout_min * 60 ))
    while true; do
        local status
        status="$(aws ssm get-command-invocation \
            --region "$REGION" \
            --command-id "$cmd_id" \
            --instance-id "$instance_id" \
            --query "Status" \
            --output text 2>/dev/null || echo "Pending")"
        case "$status" in
            Success|Failed|TimedOut|Cancelled|DeliveryTimedOut) return 0 ;;
        esac
        if [[ $(date +%s) -gt $deadline ]]; then
            echo "ERROR: SSM command timed out after ${timeout_min} min" >&2
            return 1
        fi
        sleep 15
    done
}

# Get StandardOutputContent for a completed SSM command.
# $1 = instance_id  $2 = cmd_id
_ssm_output() {
    local instance_id="$1" cmd_id="$2"
    aws ssm get-command-invocation \
        --region "$REGION" \
        --command-id "$cmd_id" \
        --instance-id "$instance_id" \
        --query "StandardOutputContent" \
        --output text 2>/dev/null || echo ""
}

# Poll until SSM agent on instance is Online (max ~15 min).
# $1 = instance_id  $2 = label
_wait_for_ssm_agent() {
    local instance_id="$1" label="${2:-$instance_id}"
    echo "=== Waiting for SSM agent on $label ===" >&3
    local attempts=0
    while true; do
        local ping
        ping="$(aws ssm describe-instance-information \
            --region "$REGION" \
            --filters "Key=InstanceIds,Values=${instance_id}" \
            --query "InstanceInformationList[0].PingStatus" \
            --output text 2>/dev/null || echo "None")"
        if [[ "$ping" == "Online" ]]; then
            echo "    $label: SSM agent online" >&3
            return 0
        fi
        attempts=$(( attempts + 1 ))
        if [[ $attempts -ge 60 ]]; then   # 60 * 15s = 15 min
            echo "ERROR: $label SSM agent not online after 15 minutes" >&2
            return 1
        fi
        sleep 15
    done
}

# ---------------------------------------------------------------------------
# setup_file — creates both instances and installs both components
# ---------------------------------------------------------------------------
setup_file() {
    if ! aws sts get-caller-identity --output text >/dev/null 2>&1; then
        echo "ERROR: AWS credentials not configured — run 'aws configure' first" >&2
        exit 1
    fi
    if ! command -v jq &>/dev/null; then
        echo "ERROR: jq is required (brew install jq)" >&2
        exit 1
    fi

    local sg_name="nwk-win-inttest-sg-$(date +%s)"

    # --- Windows Server 2022 AMI lookup ---
    echo "=== Looking up Windows Server 2022 AMI in $REGION ===" >&3
    local ami
    ami="$(aws ec2 describe-images \
        --region "$REGION" \
        --owners amazon \
        --filters \
            "Name=name,Values=Windows_Server-2022-English-Full-Base-*" \
            "Name=state,Values=available" \
            "Name=architecture,Values=x86_64" \
        --query "sort_by(Images, &CreationDate)[-1].ImageId" \
        --output text 2>/dev/null || echo "")"
    if [[ -z "$ami" || "$ami" == "None" ]]; then
        echo "ERROR: could not find Windows Server 2022 AMI in $REGION" >&2
        exit 1
    fi
    echo "    AMI: $ami" >&3

    # --- IAM role + instance profile for SSM (idempotent) ---
    echo "=== Ensuring IAM SSM role + instance profile ===" >&3
    aws iam create-role \
        --role-name "$SSM_ROLE" \
        --assume-role-policy-document \
            '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}' \
        --output text >/dev/null 2>&1 || true
    aws iam attach-role-policy \
        --role-name "$SSM_ROLE" \
        --policy-arn "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore" \
        2>/dev/null || true
    aws iam create-instance-profile \
        --instance-profile-name "$SSM_PROFILE" \
        --output text >/dev/null 2>&1 || true
    aws iam add-role-to-instance-profile \
        --instance-profile-name "$SSM_PROFILE" \
        --role-name "$SSM_ROLE" \
        2>/dev/null || true
    # IAM profile propagation delay
    sleep 10

    # --- Shared security group ---
    echo "=== Creating security group: $sg_name ===" >&3
    local sg_id
    sg_id="$(aws ec2 create-security-group \
        --region "$REGION" \
        --group-name "$sg_name" \
        --description "networker Win integration test - tester+endpoint (auto-cleanup)" \
        --query "GroupId" \
        --output text)"
    _save sg_id "$sg_id"
    echo "    SG: $sg_id" >&3

    # TCP: SSH-equivalent (WinRM 5985/5986) + networker ports
    for port in 80 443 8080 8443; do
        aws ec2 authorize-security-group-ingress \
            --region "$REGION" --group-id "$sg_id" \
            --protocol tcp --port "$port" --cidr 0.0.0.0/0 --output text >/dev/null
    done
    # UDP
    for port in 8443 9998 9999; do
        aws ec2 authorize-security-group-ingress \
            --region "$REGION" --group-id "$sg_id" \
            --protocol udp --port "$port" --cidr 0.0.0.0/0 --output text >/dev/null
    done
    # Allow all traffic within the security group (for tester→endpoint via private IP)
    aws ec2 authorize-security-group-ingress \
        --region "$REGION" --group-id "$sg_id" \
        --protocol -1 --source-group "$sg_id" --output text >/dev/null 2>/dev/null || true

    # --- Launch endpoint instance ---
    echo "=== Launching endpoint instance ($WIN_SIZE, Win2022) ===" >&3
    local ep_id
    ep_id="$(aws ec2 run-instances \
        --region "$REGION" \
        --image-id "$ami" \
        --instance-type "$WIN_SIZE" \
        --security-group-ids "$sg_id" \
        --iam-instance-profile "Name=${SSM_PROFILE}" \
        --tag-specifications \
            "ResourceType=instance,Tags=[{Key=Name,Value=nwk-ep-win-inttest}]" \
        --query "Instances[0].InstanceId" \
        --output text)"
    _save ep_id "$ep_id"
    echo "    Endpoint instance: $ep_id" >&3

    # --- Launch tester instance ---
    echo "=== Launching tester instance ($WIN_SIZE, Win2022) ===" >&3
    local ts_id
    ts_id="$(aws ec2 run-instances \
        --region "$REGION" \
        --image-id "$ami" \
        --instance-type "$WIN_SIZE" \
        --security-group-ids "$sg_id" \
        --iam-instance-profile "Name=${SSM_PROFILE}" \
        --tag-specifications \
            "ResourceType=instance,Tags=[{Key=Name,Value=nwk-ts-win-inttest}]" \
        --query "Instances[0].InstanceId" \
        --output text)"
    _save ts_id "$ts_id"
    echo "    Tester instance: $ts_id" >&3

    # --- Wait for instances to be running ---
    echo "=== Waiting for instances to reach 'running' state ===" >&3
    aws ec2 wait instance-running \
        --region "$REGION" \
        --instance-ids "$ep_id" "$ts_id"

    local ep_pub_ip ts_pub_ip ep_priv_ip
    ep_pub_ip="$(aws ec2 describe-instances \
        --region "$REGION" --instance-ids "$ep_id" \
        --query "Reservations[0].Instances[0].PublicIpAddress" \
        --output text)"
    ts_pub_ip="$(aws ec2 describe-instances \
        --region "$REGION" --instance-ids "$ts_id" \
        --query "Reservations[0].Instances[0].PublicIpAddress" \
        --output text)"
    ep_priv_ip="$(aws ec2 describe-instances \
        --region "$REGION" --instance-ids "$ep_id" \
        --query "Reservations[0].Instances[0].PrivateIpAddress" \
        --output text)"
    _save ep_pub_ip "$ep_pub_ip"
    _save ts_pub_ip "$ts_pub_ip"
    _save ep_priv_ip "$ep_priv_ip"
    echo "    Endpoint public IP:  $ep_pub_ip" >&3
    echo "    Endpoint private IP: $ep_priv_ip" >&3
    echo "    Tester public IP:    $ts_pub_ip" >&3

    # --- Wait for SSM agents (Windows needs ~5-10 min after boot to register) ---
    _wait_for_ssm_agent "$ep_id" "endpoint"
    _wait_for_ssm_agent "$ts_id" "tester"

    # -------------------------------------------------------------------------
    # PowerShell install scripts (identical to Azure Windows test)
    # CARGO_HOME=C:\cargo so SYSTEM account gets a predictable binary path.
    # -------------------------------------------------------------------------
    local ep_ps ts_ps
    ep_ps="$(mktemp /tmp/nwk-ep-XXXXX.ps1)"
    ts_ps="$(mktemp /tmp/nwk-ts-XXXXX.ps1)"

    # --- Endpoint install script ---
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

    # --- Tester install script ---
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
    # Run both installs in parallel via SSM async commands
    # -------------------------------------------------------------------------
    echo "=== Installing endpoint + tester in parallel (~35-50 min) ===" >&3

    local ep_cmd_id ts_cmd_id
    ep_cmd_id="$(_ssm_ps_file_async "$ep_id" "$ep_ps" 90)"
    ts_cmd_id="$(_ssm_ps_file_async "$ts_id" "$ts_ps" 90)"
    rm -f "$ep_ps" "$ts_ps"

    if [[ -z "$ep_cmd_id" ]]; then
        echo "ERROR: endpoint SSM send-command failed" >&2; exit 1
    fi
    if [[ -z "$ts_cmd_id" ]]; then
        echo "ERROR: tester SSM send-command failed" >&2; exit 1
    fi

    # Wait for both to complete (poll in a loop alternating between both)
    local ep_done=0 ts_done=0
    local deadline=$(( $(date +%s) + 90 * 60 ))
    while [[ $ep_done -eq 0 || $ts_done -eq 0 ]]; do
        if [[ $ep_done -eq 0 ]]; then
            local ep_status
            ep_status="$(aws ssm get-command-invocation \
                --region "$REGION" --command-id "$ep_cmd_id" --instance-id "$ep_id" \
                --query "Status" --output text 2>/dev/null || echo "Pending")"
            case "$ep_status" in
                Success|Failed|TimedOut|Cancelled|DeliveryTimedOut) ep_done=1 ;;
            esac
        fi
        if [[ $ts_done -eq 0 ]]; then
            local ts_status
            ts_status="$(aws ssm get-command-invocation \
                --region "$REGION" --command-id "$ts_cmd_id" --instance-id "$ts_id" \
                --query "Status" --output text 2>/dev/null || echo "Pending")"
            case "$ts_status" in
                Success|Failed|TimedOut|Cancelled|DeliveryTimedOut) ts_done=1 ;;
            esac
        fi
        if [[ $ep_done -eq 1 && $ts_done -eq 1 ]]; then break; fi
        if [[ $(date +%s) -gt $deadline ]]; then
            echo "ERROR: parallel install timed out after 90 minutes" >&2; exit 1
        fi
        sleep 20
    done

    echo "=== Endpoint install output ===" >&3
    _ssm_output "$ep_id" "$ep_cmd_id" >&3
    local ep_err
    ep_err="$(aws ssm get-command-invocation --region "$REGION" \
        --command-id "$ep_cmd_id" --instance-id "$ep_id" \
        --query "StandardErrorContent" --output text 2>/dev/null || echo "")"
    [[ -n "$ep_err" ]] && { echo "=== Endpoint install stderr ===" >&3; echo "$ep_err" >&3; }

    echo "=== Tester install output ===" >&3
    _ssm_output "$ts_id" "$ts_cmd_id" >&3
    local ts_err
    ts_err="$(aws ssm get-command-invocation --region "$REGION" \
        --command-id "$ts_cmd_id" --instance-id "$ts_id" \
        --query "StandardErrorContent" --output text 2>/dev/null || echo "")"
    [[ -n "$ts_err" ]] && { echo "=== Tester install stderr ===" >&3; echo "$ts_err" >&3; }

    # -------------------------------------------------------------------------
    # Run the tester against the endpoint (private IP for intra-VPC comms)
    # -------------------------------------------------------------------------
    echo "=== Running networker-tester on tester instance → endpoint instance ===" >&3
    local ep_priv_ip_val
    ep_priv_ip_val="$(_load ep_priv_ip)"

    local run_out
    # Inline single-line SSM: bash expands ${ep_priv_ip_val}; PS vars escaped with \$
    run_out="$(_ssm_ps "$ts_id" \
        "\$ErrorActionPreference='Continue'; \$env:PATH='C:\\cargo\\bin;'+\$env:PATH; New-Item -ItemType Directory -Force -Path 'C:\\networker-report' | Out-Null; \$r=& 'C:\\cargo\\bin\\networker-tester.exe' --target 'http://${ep_priv_ip_val}:8080/health' --modes http1 --runs 3 --output-dir 'C:\\networker-report' 2>&1; Write-Host \"tester exit: \$LASTEXITCODE\"; Write-Host \$r" \
        10)"
    echo "$run_out" >&3

    # -------------------------------------------------------------------------
    # Download JSON report via base64
    # -------------------------------------------------------------------------
    local b64
    b64="$(_ssm_ps "$ts_id" \
        "\$f=Get-ChildItem 'C:\\networker-report\\run-*.json' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1; if(\$f){ Write-Host ([Convert]::ToBase64String([System.IO.File]::ReadAllBytes(\$f.FullName))) } else { Write-Host 'NO_REPORT' }" \
        5 | tr -d '\n\r ')"

    mkdir -p "$RESULTS_DIR"
    local timestamp; timestamp="$(date -u +%Y-%m-%dT%H-%M-%S)"
    local local_report="${RESULTS_DIR}/aws-${REGION}-Win2022-${timestamp}.json"
    _save local_report "$local_report"

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
}

# ---------------------------------------------------------------------------
# teardown_file — terminate instances, wait, delete security group
# ---------------------------------------------------------------------------
teardown_file() {
    local ep_id ts_id sg_id
    ep_id="$(_load ep_id)"
    ts_id="$(_load ts_id)"
    sg_id="$(_load sg_id)"

    local ids=()
    [[ -n "$ep_id" ]] && ids+=("$ep_id")
    [[ -n "$ts_id" ]] && ids+=("$ts_id")
    if [[ ${#ids[@]} -gt 0 ]]; then
        echo "=== Terminating instances: ${ids[*]} ===" >&3
        aws ec2 terminate-instances \
            --region "$REGION" --instance-ids "${ids[@]}" \
            --output text >/dev/null 2>&1 || true
        echo "=== Waiting for instances to terminate ===" >&3
        aws ec2 wait instance-terminated \
            --region "$REGION" --instance-ids "${ids[@]}" 2>/dev/null || true
    fi

    if [[ -n "$sg_id" ]]; then
        echo "=== Deleting security group: $sg_id ===" >&3
        aws ec2 delete-security-group \
            --region "$REGION" --group-id "$sg_id" \
            --output text >/dev/null 2>&1 || true
    fi
}

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

@test "networker-endpoint is listening on port 8080 on endpoint instance" {
    local ep_id; ep_id="$(_load ep_id)"
    # Inline single-line PS via SSM; StandardOutputContent is direct output (no [stdout] markers).
    local msg
    msg="$(_ssm_ps "$ep_id" \
        'Write-Host "PORTCHK"; $c=Get-NetTCPConnection -LocalPort 8080 -ErrorAction SilentlyContinue; if($c){Write-Host "LISTEN_8080"}else{Write-Host "NO_8080"}' \
        2)"
    echo "t1 msg: ${msg:0:400}" >&3
    echo "$msg" | grep -q "LISTEN_8080"
}

@test "networker-endpoint responds on /health (port 8080)" {
    local ep_pub_ip; ep_pub_ip="$(_load ep_pub_ip)"
    local resp
    resp="$(curl -sf --max-time 15 "http://${ep_pub_ip}:8080/health" 2>/dev/null)"
    [[ -n "$resp" ]]
    echo "$resp" | grep -qi "ok\|healthy\|networker"
}

@test "networker-tester binary is installed on tester instance" {
    local ts_id; ts_id="$(_load ts_id)"
    local msg
    msg="$(_ssm_ps "$ts_id" \
        "Write-Host 'VERCHK'; Write-Host (& 'C:\\cargo\\bin\\networker-tester.exe' --version 2>&1)" \
        2)"
    echo "t3 msg: ${msg:0:400}" >&3
    echo "$msg" | grep -qi "networker-tester"
}

@test "networker-tester can probe endpoint via HTTP/1.1 from tester instance" {
    local ts_id ep_priv_ip
    ts_id="$(_load ts_id)"; ep_priv_ip="$(_load ep_priv_ip)"
    local msg
    msg="$(_ssm_ps "$ts_id" \
        "\$ErrorActionPreference='Continue'; Write-Host 'PROBE1'; \$r=& 'C:\\cargo\\bin\\networker-tester.exe' --target 'http://${ep_priv_ip}:8080/health' --modes http1 --runs 3 2>&1; Write-Host \"EXIT:\$LASTEXITCODE\"; Write-Host \$r" \
        5)"
    echo "t4 msg: ${msg:0:600}" >&3
    echo "$msg" | grep -qi "http1\|pass\|ms\|networker"
}

@test "networker-tester can probe endpoint via HTTP/2 from tester instance" {
    local ts_id ep_priv_ip
    ts_id="$(_load ts_id)"; ep_priv_ip="$(_load ep_priv_ip)"
    local msg
    msg="$(_ssm_ps "$ts_id" \
        "\$ErrorActionPreference='Continue'; Write-Host 'PROBE2'; \$r=& 'C:\\cargo\\bin\\networker-tester.exe' --target 'https://${ep_priv_ip}:8443/health' --modes http2 --runs 3 --insecure 2>&1; Write-Host \"EXIT:\$LASTEXITCODE\"; Write-Host \$r" \
        5)"
    echo "t5 msg: ${msg:0:600}" >&3
    echo "$msg" | grep -qi "http2\|pass\|ms\|networker"
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
