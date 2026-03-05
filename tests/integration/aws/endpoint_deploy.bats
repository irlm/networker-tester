#!/usr/bin/env bats
# Integration test: deploy networker-endpoint on a fresh AWS EC2 Ubuntu instance.
#
# What it does:
#   1. Looks up latest Ubuntu 22.04 LTS AMI for the region
#   2. Imports local SSH public key as 'networker-keypair' (idempotent)
#   3. Creates a security group with all required ports open
#   4. Launches a t3.small instance, waits until running
#   5. Uploads the installer and runs it on the instance non-interactively
#   6. Asserts the binary version, systemd service, and /health endpoint
#   7. Tears down: terminates instance, deletes security group (always, even on failure)
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
#   AWS_REGION=eu-west-1 bats tests/integration/aws/endpoint_deploy.bats

load "../helpers/vm_helpers"

# ---------------------------------------------------------------------------
# Config — override via env vars for CI / custom environments
# ---------------------------------------------------------------------------
REGION="${AWS_REGION:-us-east-1}"
INSTANCE_TYPE="${AWS_INSTANCE_TYPE:-t3.small}"
SSH_USER="ubuntu"
KEY_NAME="networker-keypair"
SG_NAME="networker-inttest-sg-$(date +%s)"   # unique per run to avoid conflicts
INSTANCE_NAME="nwk-inttest-$(date +%s)"
INSTALLER="${INSTALLER:-$(cd "$(dirname "$BATS_TEST_FILENAME")/../../.." && pwd)/install.sh}"

# Global state set in setup_file, used by tests and teardown_file
INSTANCE_ID=""
SG_ID=""
VM_IP=""

# ---------------------------------------------------------------------------
# setup_file — runs once before all tests; creates instance
# ---------------------------------------------------------------------------
setup_file() {
    # Verify prerequisites
    if ! aws sts get-caller-identity --output none 2>/dev/null; then
        echo "ERROR: AWS credentials not configured — run 'aws configure' first" >&2
        exit 1
    fi
    if [[ ! -f "$INSTALLER" ]]; then
        echo "ERROR: installer not found at $INSTALLER" >&2
        exit 1
    fi

    export REGION INSTANCE_TYPE SSH_USER KEY_NAME SG_NAME INSTANCE_NAME INSTALLER

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
    local ssh_key=""
    for kf in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
        [[ -f "$kf" ]] && { ssh_key="$kf"; break; }
    done
    local key_opt=""
    if [[ -n "$ssh_key" ]]; then
        echo "=== Importing SSH key as $KEY_NAME ===" >&3
        aws ec2 import-key-pair \
            --region "$REGION" \
            --key-name "$KEY_NAME" \
            --public-key-material "fileb://${ssh_key}" \
            --output none 2>/dev/null || true   # ignore "already exists"
        key_opt="--key-name $KEY_NAME"
    fi

    # --- Security group ---
    echo "=== Creating security group: $SG_NAME ===" >&3
    SG_ID="$(aws ec2 create-security-group \
        --region "$REGION" \
        --group-name "$SG_NAME" \
        --description "networker integration test (auto-cleanup)" \
        --query "GroupId" \
        --output text)"
    export SG_ID
    echo "    SG: $SG_ID" >&3

    # SSH
    aws ec2 authorize-security-group-ingress \
        --region "$REGION" --group-id "$SG_ID" \
        --protocol tcp --port 22 --cidr 0.0.0.0/0 --output none
    # TCP 80, 443, 8080, 8443
    for port in 80 443 8080 8443; do
        aws ec2 authorize-security-group-ingress \
            --region "$REGION" --group-id "$SG_ID" \
            --protocol tcp --port "$port" --cidr 0.0.0.0/0 --output none
    done
    # UDP 8443, 9998, 9999
    for port in 8443 9998 9999; do
        aws ec2 authorize-security-group-ingress \
            --region "$REGION" --group-id "$SG_ID" \
            --protocol udp --port "$port" --cidr 0.0.0.0/0 --output none
    done

    # --- Launch instance ---
    echo "=== Launching EC2 instance ($INSTANCE_TYPE, Ubuntu 22.04) ===" >&3
    INSTANCE_ID="$(aws ec2 run-instances \
        --region "$REGION" \
        --image-id "$ami" \
        --instance-type "$INSTANCE_TYPE" \
        $key_opt \
        --security-group-ids "$SG_ID" \
        --tag-specifications \
            "ResourceType=instance,Tags=[{Key=Name,Value=${INSTANCE_NAME}}]" \
        --query "Instances[0].InstanceId" \
        --output text)"
    export INSTANCE_ID
    echo "    Instance: $INSTANCE_ID" >&3

    echo "=== Waiting for instance to reach 'running' state ===" >&3
    aws ec2 wait instance-running \
        --region "$REGION" \
        --instance-ids "$INSTANCE_ID"

    VM_IP="$(aws ec2 describe-instances \
        --region "$REGION" \
        --instance-ids "$INSTANCE_ID" \
        --query "Reservations[0].Instances[0].PublicIpAddress" \
        --output text)"
    export VM_IP
    echo "    Public IP: $VM_IP" >&3

    # --- Wait for SSH ---
    echo "=== Waiting for SSH on $VM_IP ===" >&3
    wait_for_ssh "$VM_IP" "$SSH_USER" 180

    # --- Upload and run installer ---
    echo "=== Uploading installer to instance ===" >&3
    scp -o StrictHostKeyChecking=no -q "$INSTALLER" \
        "${SSH_USER}@${VM_IP}:/tmp/networker-install.sh"

    echo "=== Running installer on instance (endpoint -y) ===" >&3
    ssh -t -o StrictHostKeyChecking=no "${SSH_USER}@${VM_IP}" \
        "bash /tmp/networker-install.sh endpoint -y" >&3 2>&3
}

# ---------------------------------------------------------------------------
# teardown_file — runs once after all tests; always cleans up AWS resources
# ---------------------------------------------------------------------------
teardown_file() {
    if [[ -n "$INSTANCE_ID" ]]; then
        echo "=== Terminating instance: $INSTANCE_ID ===" >&3
        aws ec2 terminate-instances \
            --region "$REGION" \
            --instance-ids "$INSTANCE_ID" \
            --output none 2>/dev/null || true

        # Wait for termination before deleting SG (SG can't be deleted while in use)
        echo "=== Waiting for instance to terminate ===" >&3
        aws ec2 wait instance-terminated \
            --region "$REGION" \
            --instance-ids "$INSTANCE_ID" 2>/dev/null || true
    fi

    if [[ -n "$SG_ID" ]]; then
        echo "=== Deleting security group: $SG_ID ===" >&3
        aws ec2 delete-security-group \
            --region "$REGION" \
            --group-id "$SG_ID" \
            --output none 2>/dev/null || true
    fi
}

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

@test "networker-endpoint binary is installed on the instance" {
    local ver
    ver="$(ssh_run "$VM_IP" "$SSH_USER" "networker-endpoint --version 2>/dev/null || ~/.cargo/bin/networker-endpoint --version 2>/dev/null")"
    [[ "$ver" == networker-endpoint* ]]
}

@test "networker-endpoint version matches expected release" {
    local expected
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

@test "networker-tester on local machine can reach the instance endpoint" {
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
