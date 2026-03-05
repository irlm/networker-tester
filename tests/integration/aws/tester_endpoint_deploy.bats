#!/usr/bin/env bats
# Integration test: deploy networker-endpoint on one AWS EC2 instance and
# networker-tester on a second instance, run the tester against the endpoint,
# save the report locally, then terminate both instances.
#
# What it does:
#   1. Looks up latest Ubuntu 22.04 LTS AMI for the region
#   2. Imports local SSH public key as 'networker-keypair' (idempotent)
#   3. Creates a shared security group with all required ports open
#   4. Launches two instances:
#      - endpoint  (t3.small, Ubuntu 22.04)
#      - tester    (t3.small, Ubuntu 22.04)
#   5. Installs networker-endpoint on the endpoint instance
#   6. Installs networker-tester on the tester instance
#   7. Runs networker-tester → saves JSON report on the tester instance
#   8. Downloads the report to results/ locally (named with timestamp+region+OS)
#   9. Tears down: terminates both instances, waits for termination,
#      deletes the security group (no ongoing cost)
#
# Prerequisites:
#   - AWS credentials configured (aws sts get-caller-identity works)
#   - SSH key at ~/.ssh/id_ed25519 or ~/.ssh/id_rsa
#   - INSTALLER path set (or falls back to repo root install.sh)
#   - bats-core  (brew install bats-core)
#
# Run:
#   cd tests/integration && ./run.sh aws
#   # or directly:
#   AWS_REGION=eu-west-1 bats tests/integration/aws/tester_endpoint_deploy.bats

load "../helpers/vm_helpers"

# ---------------------------------------------------------------------------
# Config — override via env vars for CI / custom environments
# ---------------------------------------------------------------------------
REGION="${AWS_REGION:-us-east-1}"
INSTANCE_TYPE="${AWS_INSTANCE_TYPE:-t3.small}"
SSH_USER="ubuntu"
KEY_NAME="networker-keypair"
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
    echo "=== Uploading installer to $component instance ($ip) ===" >&3
    scp -o StrictHostKeyChecking=no -q "$INSTALLER" \
        "${SSH_USER}@${ip}:/tmp/networker-install.sh"
    echo "=== Running installer on $component instance (${component} -y) ===" >&3
    ssh -t -o StrictHostKeyChecking=no "${SSH_USER}@${ip}" \
        "bash /tmp/networker-install.sh ${component} -y" >&3 2>&3
}

# ---------------------------------------------------------------------------
# setup_file — creates both instances and installs both components
# ---------------------------------------------------------------------------
setup_file() {
    if ! aws sts get-caller-identity --output text >/dev/null 2>&1; then
        echo "ERROR: AWS credentials not configured — run 'aws configure' first" >&2
        exit 1
    fi
    if [[ ! -f "$INSTALLER" ]]; then
        echo "ERROR: installer not found at $INSTALLER" >&2
        exit 1
    fi

    local sg_name="networker-inttest-sg-$(date +%s)"

    # --- AMI lookup ---
    echo "=== Looking up Ubuntu 22.04 LTS AMI in $REGION ===" >&3
    local ami
    ami="$(aws ec2 describe-images \
        --region "$REGION" \
        --owners 099720109477 \
        --filters \
            "Name=name,Values=ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*" \
            "Name=state,Values=available" \
            "Name=architecture,Values=x86_64" \
        --query "sort_by(Images, &CreationDate)[-1].ImageId" \
        --output text 2>/dev/null || echo "")"
    if [[ -z "$ami" || "$ami" == "None" ]]; then
        echo "ERROR: could not find Ubuntu 22.04 AMI in $REGION" >&2
        exit 1
    fi
    echo "    AMI: $ami" >&3

    # --- SSH key import (idempotent) ---
    local ssh_key="" key_opt=""
    for kf in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
        [[ -f "$kf" ]] && { ssh_key="$kf"; break; }
    done
    if [[ -n "$ssh_key" ]]; then
        echo "=== Importing SSH key as $KEY_NAME ===" >&3
        aws ec2 import-key-pair \
            --region "$REGION" \
            --key-name "$KEY_NAME" \
            --public-key-material "fileb://${ssh_key}" \
            --output text >/dev/null 2>&1 || true   # ignore "already exists"
        key_opt="--key-name $KEY_NAME"
    fi

    # --- Shared security group ---
    echo "=== Creating security group: $sg_name ===" >&3
    local sg_id
    sg_id="$(aws ec2 create-security-group \
        --region "$REGION" \
        --group-name "$sg_name" \
        --description "networker integration test — tester+endpoint (auto-cleanup)" \
        --query "GroupId" \
        --output text)"
    _save sg_id "$sg_id"
    echo "    SG: $sg_id" >&3

    # SSH from anywhere
    aws ec2 authorize-security-group-ingress \
        --region "$REGION" --group-id "$sg_id" \
        --protocol tcp --port 22 --cidr 0.0.0.0/0 --output text >/dev/null
    # Endpoint TCP ports (accessible from tester instance and local curl)
    for port in 80 443 8080 8443; do
        aws ec2 authorize-security-group-ingress \
            --region "$REGION" --group-id "$sg_id" \
            --protocol tcp --port "$port" --cidr 0.0.0.0/0 --output text >/dev/null
    done
    # Endpoint UDP ports
    for port in 8443 9998 9999; do
        aws ec2 authorize-security-group-ingress \
            --region "$REGION" --group-id "$sg_id" \
            --protocol udp --port "$port" --cidr 0.0.0.0/0 --output text >/dev/null
    done

    # --- Launch endpoint instance ---
    echo "=== Launching endpoint instance ($INSTANCE_TYPE, Ubuntu 22.04) ===" >&3
    local ep_instance_id
    ep_instance_id="$(aws ec2 run-instances \
        --region "$REGION" \
        --image-id "$ami" \
        --instance-type "$INSTANCE_TYPE" \
        $key_opt \
        --security-group-ids "$sg_id" \
        --tag-specifications \
            "ResourceType=instance,Tags=[{Key=Name,Value=nwk-ep-inttest}]" \
        --query "Instances[0].InstanceId" \
        --output text)"
    _save ep_instance_id "$ep_instance_id"
    echo "    Endpoint instance: $ep_instance_id" >&3

    # --- Launch tester instance ---
    echo "=== Launching tester instance ($INSTANCE_TYPE, Ubuntu 22.04) ===" >&3
    local ts_instance_id
    ts_instance_id="$(aws ec2 run-instances \
        --region "$REGION" \
        --image-id "$ami" \
        --instance-type "$INSTANCE_TYPE" \
        $key_opt \
        --security-group-ids "$sg_id" \
        --tag-specifications \
            "ResourceType=instance,Tags=[{Key=Name,Value=nwk-ts-inttest}]" \
        --query "Instances[0].InstanceId" \
        --output text)"
    _save ts_instance_id "$ts_instance_id"
    echo "    Tester instance: $ts_instance_id" >&3

    # --- Wait for both instances to reach 'running' state ---
    echo "=== Waiting for instances to reach 'running' state ===" >&3
    aws ec2 wait instance-running \
        --region "$REGION" \
        --instance-ids "$ep_instance_id" "$ts_instance_id"

    local ep_ip ts_ip
    ep_ip="$(aws ec2 describe-instances \
        --region "$REGION" \
        --instance-ids "$ep_instance_id" \
        --query "Reservations[0].Instances[0].PublicIpAddress" \
        --output text)"
    ts_ip="$(aws ec2 describe-instances \
        --region "$REGION" \
        --instance-ids "$ts_instance_id" \
        --query "Reservations[0].Instances[0].PublicIpAddress" \
        --output text)"
    _save ep_ip "$ep_ip"
    _save ts_ip "$ts_ip"
    echo "    Endpoint IP: $ep_ip" >&3
    echo "    Tester IP:   $ts_ip" >&3

    # --- Install endpoint ---
    echo "=== Waiting for SSH on endpoint ($ep_ip) ===" >&3
    wait_for_ssh "$ep_ip" "$SSH_USER" 180
    _install_component "endpoint" "$ep_ip"

    # --- Install tester ---
    echo "=== Waiting for SSH on tester ($ts_ip) ===" >&3
    wait_for_ssh "$ts_ip" "$SSH_USER" 180
    _install_component "tester" "$ts_ip"

    # --- Run the tester against the endpoint ---
    echo "=== Running networker-tester on tester → endpoint ===" >&3
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
    local local_report="${RESULTS_DIR}/aws-${REGION}-Ubuntu22-${timestamp}.json"
    _save local_report "$local_report"
    if [[ -n "$remote_report" ]] && scp -o StrictHostKeyChecking=no -q \
           "${SSH_USER}@${ts_ip}:${remote_report}" "$local_report" 2>/dev/null; then
        echo "=== Report saved locally: $local_report ===" >&3
    else
        echo "    (report download failed — tester may not have produced output)" >&3
    fi
}

# ---------------------------------------------------------------------------
# teardown_file — terminates both instances and deletes the security group.
# Waits for termination before deleting the SG (SGs can't be deleted while
# instances are still referencing them).
# ---------------------------------------------------------------------------
teardown_file() {
    local ep_id ts_id sg_id
    ep_id="$(_load ep_instance_id)"
    ts_id="$(_load ts_instance_id)"
    sg_id="$(_load sg_id)"

    # Terminate both instances in one call (faster, parallel AWS-side)
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

@test "networker-endpoint service is active on endpoint instance" {
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

@test "networker-tester binary is installed on tester instance" {
    local ts_ip; ts_ip="$(_load ts_ip)"
    local ver
    ver="$(ssh_run "$ts_ip" "$SSH_USER" \
        "networker-tester --version 2>/dev/null || ~/.cargo/bin/networker-tester --version 2>/dev/null")"
    [[ "$ver" == networker-tester* ]]
}

@test "networker-tester can probe endpoint via HTTP/1.1 from tester instance" {
    local ep_ip; ep_ip="$(_load ep_ip)"
    local ts_ip; ts_ip="$(_load ts_ip)"
    local out
    out="$(ssh_run "$ts_ip" "$SSH_USER" \
        "~/.cargo/bin/networker-tester \
            --target http://${ep_ip}:8080/health \
            --modes http1 --runs 3 2>&1")"
    echo "$out" | grep -qi "http1\|pass\|ms\|networker"
}

@test "networker-tester can probe endpoint via HTTP/2 from tester instance" {
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
