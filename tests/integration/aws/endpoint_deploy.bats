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
INSTALLER="${INSTALLER:-$(cd "$(dirname "$BATS_TEST_FILENAME")/../../.." && pwd)/install.sh}"

# ---------------------------------------------------------------------------
# Helpers: persist/load state across setup_file → tests → teardown_file
# (bats runs each @test in a subshell so exported vars don't propagate)
# ---------------------------------------------------------------------------
_save() { echo "$2" > "${BATS_FILE_TMPDIR}/${1}"; }
_load() { cat "${BATS_FILE_TMPDIR}/${1}" 2>/dev/null || echo ""; }

# ---------------------------------------------------------------------------
# setup_file — runs once before all tests; creates instance
# ---------------------------------------------------------------------------
setup_file() {
    # Verify prerequisites
    if ! aws sts get-caller-identity --output text >/dev/null 2>&1; then
        echo "ERROR: AWS credentials not configured — run 'aws configure' first" >&2
        exit 1
    fi
    if [[ ! -f "$INSTALLER" ]]; then
        echo "ERROR: installer not found at $INSTALLER" >&2
        exit 1
    fi

    local sg_name="networker-inttest-sg-$(date +%s)"
    local instance_name="nwk-inttest-$(date +%s)"

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

    # --- Security group ---
    echo "=== Creating security group: $sg_name ===" >&3
    local sg_id
    sg_id="$(aws ec2 create-security-group \
        --region "$REGION" \
        --group-name "$sg_name" \
        --description "networker integration test (auto-cleanup)" \
        --query "GroupId" \
        --output text)"
    _save sg_id "$sg_id"
    echo "    SG: $sg_id" >&3

    # SSH
    aws ec2 authorize-security-group-ingress \
        --region "$REGION" --group-id "$sg_id" \
        --protocol tcp --port 22 --cidr 0.0.0.0/0 --output text >/dev/null
    # TCP 80, 443, 8080, 8443
    for port in 80 443 8080 8443; do
        aws ec2 authorize-security-group-ingress \
            --region "$REGION" --group-id "$sg_id" \
            --protocol tcp --port "$port" --cidr 0.0.0.0/0 --output text >/dev/null
    done
    # UDP 8443, 9998, 9999
    for port in 8443 9998 9999; do
        aws ec2 authorize-security-group-ingress \
            --region "$REGION" --group-id "$sg_id" \
            --protocol udp --port "$port" --cidr 0.0.0.0/0 --output text >/dev/null
    done

    # --- Launch instance ---
    echo "=== Launching EC2 instance ($INSTANCE_TYPE, Ubuntu 22.04) ===" >&3
    local instance_id
    instance_id="$(aws ec2 run-instances \
        --region "$REGION" \
        --image-id "$ami" \
        --instance-type "$INSTANCE_TYPE" \
        $key_opt \
        --security-group-ids "$sg_id" \
        --tag-specifications \
            "ResourceType=instance,Tags=[{Key=Name,Value=${instance_name}}]" \
        --query "Instances[0].InstanceId" \
        --output text)"
    _save instance_id "$instance_id"
    echo "    Instance: $instance_id" >&3

    echo "=== Waiting for instance to reach 'running' state ===" >&3
    aws ec2 wait instance-running --region "$REGION" --instance-ids "$instance_id"

    local vm_ip
    vm_ip="$(aws ec2 describe-instances \
        --region "$REGION" \
        --instance-ids "$instance_id" \
        --query "Reservations[0].Instances[0].PublicIpAddress" \
        --output text)"
    _save vm_ip "$vm_ip"
    echo "    Public IP: $vm_ip" >&3

    # --- Wait for SSH ---
    echo "=== Waiting for SSH on $vm_ip ===" >&3
    wait_for_ssh "$vm_ip" "$SSH_USER" 180

    # --- Upload and run installer ---
    echo "=== Uploading installer to instance ===" >&3
    scp -o StrictHostKeyChecking=no -q "$INSTALLER" \
        "${SSH_USER}@${vm_ip}:/tmp/networker-install.sh"

    echo "=== Running installer on instance (endpoint -y) ===" >&3
    # -t allocates a pseudo-TTY so the installer's ask_yn can read from /dev/tty
    ssh -t -o StrictHostKeyChecking=no "${SSH_USER}@${vm_ip}" \
        "bash /tmp/networker-install.sh endpoint -y" >&3 2>&3
}

# ---------------------------------------------------------------------------
# teardown_file — runs once after all tests; always cleans up AWS resources
# ---------------------------------------------------------------------------
teardown_file() {
    local instance_id sg_id
    instance_id="$(_load instance_id)"
    sg_id="$(_load sg_id)"

    if [[ -n "$instance_id" ]]; then
        echo "=== Terminating instance: $instance_id ===" >&3
        aws ec2 terminate-instances \
            --region "$REGION" --instance-ids "$instance_id" \
            --output text >/dev/null 2>&1 || true
        echo "=== Waiting for instance to terminate ===" >&3
        aws ec2 wait instance-terminated \
            --region "$REGION" --instance-ids "$instance_id" 2>/dev/null || true
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

@test "networker-endpoint binary is installed on the instance" {
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

@test "networker-tester on local machine can reach the instance endpoint" {
    if ! command -v networker-tester &>/dev/null; then
        skip "networker-tester not installed locally"
    fi
    local vm_ip; vm_ip="$(_load vm_ip)"
    networker-tester \
        --target "https://${vm_ip}:8443/health" \
        --modes http1 \
        --runs 3 \
        --insecure \
        --output-dir /tmp/nwk-local-test
}
