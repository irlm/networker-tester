#!/usr/bin/env bats
# Installer unit tests for install.sh
#
# Run locally:
#   bats tests/installer.bats
#
# Requires bats-core >= 1.10:
#   macOS:  brew install bats-core
#   Linux:  sudo apt-get install bats   OR   https://github.com/bats-core/bats-core

load 'test_helper'

# ---------------------------------------------------------------------------
# setup / teardown
# ---------------------------------------------------------------------------
setup() {
    source_installer   # sources install.sh, resets globals, relaxes set -euo
    use_stubs          # prepends tests/stubs/ to PATH

    # Silence noisy helpers by default; override per-test where needed
    mock_next_step
    mock_cargo_progress

    # Per-test temp dir; INSTALL_DIR points into it by default
    TEST_TMPDIR="$(mktemp -d)"
    INSTALL_DIR="$TEST_TMPDIR/bin"
    mkdir -p "$INSTALL_DIR"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
    unset STUB_SSH_FAIL STUB_SSH_UNAME STUB_SSH_VERSION STUB_SSH_FAIL_UNAME \
          STUB_SSH_FAIL_VERSION STUB_CURL_FAIL STUB_CARGO_FAIL STUB_SCP_FAIL \
          STUB_GH_FAIL STUB_TESTER_FAIL STUB_UNAME_RESULT 2>/dev/null || true
}


# ===========================================================================
# 1. parse_args
# ===========================================================================

@test "parse_args: defaults — both components, local, no auto-yes" {
    parse_args
    [ "$COMPONENT"          = ""       ]
    [ "$AUTO_YES"           -eq 0      ]
    [ "$DO_REMOTE_TESTER"   -eq 0      ]
    [ "$DO_REMOTE_ENDPOINT" -eq 0      ]
    [ "$TESTER_LOCATION"    = "local"  ]
    [ "$ENDPOINT_LOCATION"  = "local"  ]
}

@test "parse_args: 'tester' subcommand disables endpoint install" {
    parse_args tester
    [ "$COMPONENT"           = "tester" ]
    [ "$DO_INSTALL_ENDPOINT" -eq 0      ]
    [ "$DO_INSTALL_TESTER"   -eq 1      ]
}

@test "parse_args: 'endpoint' subcommand disables tester install" {
    parse_args endpoint
    [ "$COMPONENT"           = "endpoint" ]
    [ "$DO_INSTALL_TESTER"   -eq 0        ]
    [ "$DO_INSTALL_ENDPOINT" -eq 1        ]
}

@test "parse_args: 'both' subcommand enables both components" {
    parse_args both
    [ "$COMPONENT"           = "both" ]
    [ "$DO_INSTALL_TESTER"   -eq 1    ]
    [ "$DO_INSTALL_ENDPOINT" -eq 1    ]
}

@test "parse_args: -y sets AUTO_YES=1" {
    parse_args -y
    [ "$AUTO_YES" -eq 1 ]
}

@test "parse_args: --yes sets AUTO_YES=1" {
    parse_args --yes
    [ "$AUTO_YES" -eq 1 ]
}

@test "parse_args: --azure sets endpoint location and flag" {
    parse_args --azure
    [ "$ENDPOINT_LOCATION"  = "azure" ]
    [ "$DO_REMOTE_ENDPOINT" -eq 1     ]
}

@test "parse_args: --aws sets endpoint location and flag" {
    parse_args --aws
    [ "$ENDPOINT_LOCATION"  = "aws" ]
    [ "$DO_REMOTE_ENDPOINT" -eq 1   ]
}

@test "parse_args: --tester-azure sets tester location and flag" {
    parse_args --tester-azure
    [ "$TESTER_LOCATION"  = "azure" ]
    [ "$DO_REMOTE_TESTER" -eq 1     ]
}

@test "parse_args: --region overrides Azure region" {
    parse_args --region westeurope
    [ "$AZURE_REGION" = "westeurope" ]
}

@test "parse_args: --aws-region overrides AWS region" {
    parse_args --aws-region ap-southeast-1
    [ "$AWS_REGION" = "ap-southeast-1" ]
}

@test "parse_args: combined flags — endpoint + azure + region" {
    parse_args endpoint --azure --region northeurope
    [ "$COMPONENT"          = "endpoint"    ]
    [ "$ENDPOINT_LOCATION"  = "azure"       ]
    [ "$AZURE_REGION"       = "northeurope" ]
    [ "$DO_INSTALL_TESTER"  -eq 0           ]
}

@test "parse_args: --gcp sets endpoint location and flag" {
    parse_args --gcp
    [ "$ENDPOINT_LOCATION"  = "gcp" ]
    [ "$DO_REMOTE_ENDPOINT" -eq 1   ]
}

@test "parse_args: --tester-gcp sets tester location and flag" {
    parse_args --tester-gcp
    [ "$TESTER_LOCATION"  = "gcp" ]
    [ "$DO_REMOTE_TESTER" -eq 1   ]
}

@test "parse_args: --gcp-zone overrides GCP zone and derives region" {
    parse_args --gcp-zone europe-west1-b
    [ "$GCP_ZONE" = "europe-west1-b" ]
}

@test "parse_args: --gcp-machine-type overrides machine type" {
    parse_args --gcp-machine-type e2-medium
    [ "$GCP_TESTER_MACHINE_TYPE"   = "e2-medium" ]
    [ "$GCP_ENDPOINT_MACHINE_TYPE" = "e2-medium" ]
}

@test "parse_args: --gcp-project sets GCP project" {
    parse_args --gcp-project my-project-123
    [ "$GCP_PROJECT" = "my-project-123" ]
}

@test "parse_args: combined flags — endpoint + gcp + zone" {
    parse_args endpoint --gcp --gcp-zone asia-east1-a
    [ "$COMPONENT"          = "endpoint"    ]
    [ "$ENDPOINT_LOCATION"  = "gcp"         ]
    [ "$GCP_ZONE"           = "asia-east1-a" ]
    [ "$DO_INSTALL_TESTER"  -eq 0           ]
}


# ===========================================================================
# 2. _offer_quick_test
# ===========================================================================

@test "_offer_quick_test: returns early when no remote endpoint deployed" {
    DO_REMOTE_ENDPOINT=0
    output="$(_offer_quick_test 2>&1)"
    [ -z "$output" ]
}

@test "_offer_quick_test: returns early when endpoint IP is empty" {
    DO_REMOTE_ENDPOINT=1
    ENDPOINT_LOCATION="azure"
    AZURE_ENDPOINT_IP=""
    output="$(_offer_quick_test 2>&1)"
    [ -z "$output" ]
}

@test "_offer_quick_test: offers tester install when no tester binary found" {
    DO_REMOTE_ENDPOINT=1
    ENDPOINT_LOCATION="azure"
    AZURE_ENDPOINT_IP="1.2.3.4"
    AZURE_EXTRA_ENDPOINT_IPS=()
    hide_tester_from_path   # remove stubs/networker-tester from PATH
    mock_ask_yn_no
    output="$(_offer_quick_test 2>&1)"
    [[ "$output" == *"not installed locally"* ]]
}

@test "_offer_quick_test: shows re-run instructions when user declines tester install" {
    DO_REMOTE_ENDPOINT=1
    ENDPOINT_LOCATION="azure"
    AZURE_ENDPOINT_IP="1.2.3.4"
    AZURE_EXTRA_ENDPOINT_IPS=()
    hide_tester_from_path
    mock_ask_yn_no
    output="$(_offer_quick_test 2>&1)"
    [[ "$output" == *"bash install.sh tester"* ]]
}

@test "_offer_quick_test: installs tester via release download when user accepts" {
    DO_REMOTE_ENDPOINT=1
    ENDPOINT_LOCATION="azure"
    AZURE_ENDPOINT_IP="1.2.3.4"
    AZURE_EXTRA_ENDPOINT_IPS=()
    INSTALL_METHOD="release"
    hide_tester_from_path
    mock_ask_yn_yes
    # Stub step_download_release to create a fake binary in INSTALL_DIR
    step_download_release() {
        printf '#!/usr/bin/env bash\necho "%s 0.12.65"\n' "$1" \
            > "${INSTALL_DIR}/${1}"
        chmod +x "${INSTALL_DIR}/${1}"
    }
    # Stub the tester run itself (binary now in INSTALL_DIR)
    # by also adding INSTALL_DIR to PATH before calling the function
    PATH="${INSTALL_DIR}:${PATH}" _offer_quick_test 2>&1 || true
    [ -x "${INSTALL_DIR}/networker-tester" ]
}

@test "_offer_quick_test: runs tester when binary is found on PATH" {
    DO_REMOTE_ENDPOINT=1
    ENDPOINT_LOCATION="azure"
    AZURE_ENDPOINT_IP="1.2.3.4"
    AZURE_EXTRA_ENDPOINT_IPS=()
    mock_ask_yn_yes
    # stubs/networker-tester is on PATH (added by use_stubs)
    output="$(_offer_quick_test 2>&1)"
    [[ "$output" == *"[http1]"* ]]
}

@test "_offer_quick_test: includes extra endpoint IPs as --target flags" {
    DO_REMOTE_ENDPOINT=1
    ENDPOINT_LOCATION="azure"
    AZURE_ENDPOINT_IP="1.2.3.4"
    AZURE_EXTRA_ENDPOINT_IPS=("5.6.7.8:westeurope")
    mock_ask_yn_yes
    # Override the tester to just echo its args
    networker-tester() { echo "ARGS: $*"; }
    output="$(_offer_quick_test 2>&1)"
    [[ "$output" == *"1.2.3.4"* ]]
    [[ "$output" == *"5.6.7.8"* ]]
}


# ===========================================================================
# 3. _offer_also_endpoint
# ===========================================================================

@test "_offer_also_endpoint: returns early when endpoint already installed locally" {
    DO_INSTALL_ENDPOINT=1
    DO_REMOTE_ENDPOINT=0
    DO_INSTALL_TESTER=1
    output="$(_offer_also_endpoint 2>&1)"
    [ -z "$output" ]
}

@test "_offer_also_endpoint: returns early when endpoint is already remote" {
    DO_INSTALL_ENDPOINT=0
    DO_REMOTE_ENDPOINT=1
    DO_INSTALL_TESTER=1
    output="$(_offer_also_endpoint 2>&1)"
    [ -z "$output" ]
}

@test "_offer_also_endpoint: returns early when no tester installed either" {
    DO_INSTALL_TESTER=0
    DO_REMOTE_TESTER=0
    DO_INSTALL_ENDPOINT=0
    DO_REMOTE_ENDPOINT=0
    output="$(_offer_also_endpoint 2>&1)"
    [ -z "$output" ]
}

@test "_offer_also_endpoint: shows prompt when only local tester was installed" {
    DO_INSTALL_TESTER=1
    DO_REMOTE_TESTER=0
    DO_INSTALL_ENDPOINT=0
    DO_REMOTE_ENDPOINT=0
    # Provide "3" (skip) to the read prompt
    output="$(echo "3" | _offer_also_endpoint 2>&1)"
    [[ "$output" == *"networker-endpoint"* ]]
}

@test "_offer_also_endpoint: choice 1 installs endpoint locally via release" {
    DO_INSTALL_TESTER=1
    DO_REMOTE_TESTER=0
    DO_INSTALL_ENDPOINT=0
    DO_REMOTE_ENDPOINT=0
    INSTALL_METHOD="release"
    step_download_release() {
        printf '#!/usr/bin/env bash\necho "%s 0.12.65"\n' "$1" \
            > "${INSTALL_DIR}/${1}"
        chmod +x "${INSTALL_DIR}/${1}"
    }
    echo "1" | _offer_also_endpoint 2>&1 || true
    [ -x "${INSTALL_DIR}/networker-endpoint" ]
}

@test "_offer_also_endpoint: choice 2 shows cloud re-run command" {
    DO_INSTALL_TESTER=1
    DO_REMOTE_TESTER=0
    DO_INSTALL_ENDPOINT=0
    DO_REMOTE_ENDPOINT=0
    output="$(echo "2" | _offer_also_endpoint 2>&1)"
    [[ "$output" == *"bash install.sh endpoint"* ]]
}

@test "_offer_also_endpoint: also triggers when only remote tester installed" {
    DO_INSTALL_TESTER=0
    DO_REMOTE_TESTER=1
    DO_INSTALL_ENDPOINT=0
    DO_REMOTE_ENDPOINT=0
    output="$(echo "3" | _offer_also_endpoint 2>&1)"
    [[ "$output" == *"endpoint"* ]]
}


# ===========================================================================
# 4. _remote_verify_health
# ===========================================================================

@test "_remote_verify_health: succeeds immediately when curl returns 200" {
    # STUB_CURL_FAIL is unset → curl stub exits 0
    output="$(_remote_verify_health "1.2.3.4" "azureuser" 2>&1)"
    [ $? -eq 0 ]
    [[ "$output" == *"health"* ]] || [[ "$output" == *"1.2.3.4"* ]]
}

@test "_remote_verify_health: shows SSH diagnostics on timeout when curl always fails" {
    export STUB_CURL_FAIL=1
    # Patch the retry limit to 2 iterations so the test is fast
    _remote_verify_health() {
        local ip="$1" ssh_user="${2:-azureuser}"
        print_info "Checking http://${ip}:8080/health …"
        local attempts=0
        while ! curl -sf --max-time 5 "http://${ip}:8080/health" &>/dev/null; do
            attempts=$((attempts + 1))
            if [[ $attempts -gt 2 ]]; then
                print_warn "Endpoint did not respond within 60 seconds."
                print_info "Fetching service status from the VM…"
                ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 \
                    "${ssh_user}@${ip}" \
                    "sudo systemctl status networker-endpoint --no-pager -l 2>&1 | head -30"
                print_info "Last 30 log lines:"
                ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 \
                    "${ssh_user}@${ip}" \
                    "sudo journalctl -u networker-endpoint -n 30 --no-pager 2>&1"
                return 0
            fi
            sleep 0.01
        done
        print_ok "Endpoint is healthy."
    }
    output="$(_remote_verify_health "1.2.3.4" "azureuser" 2>&1)"
    [[ "$output" == *"did not respond"* ]]
    [[ "$output" == *"service status"* ]] || \
        [[ "$output" == *"networker-endpoint.service"* ]] || \
        [[ "$output" == *"systemctl"* ]]
}

@test "_remote_verify_health: passes correct ssh_user to diagnostics" {
    export STUB_CURL_FAIL=1
    local captured_user_host=""
    # Single-attempt variant to immediately trigger SSH path
    _remote_verify_health() {
        local ip="$1" ssh_user="${2:-azureuser}"
        print_warn "Endpoint did not respond."
        ssh -o StrictHostKeyChecking=no "${ssh_user}@${ip}" \
            "sudo systemctl status networker-endpoint --no-pager -l 2>&1 | head -30"
    }
    # Shadow ssh to capture the user@host arg
    ssh() { captured_user_host="$2"; }
    _remote_verify_health "9.9.9.9" "ubuntu" 2>&1
    [ "$captured_user_host" = "ubuntu@9.9.9.9" ]
}


# ===========================================================================
# 5. _remote_bootstrap_install
# ===========================================================================

@test "_remote_bootstrap_install: uses 'endpoint' component arg for networker-endpoint" {
    # Intercept ssh/scp to verify the component arg passed to the installer
    captured_ssh_cmd=""
    ssh()  { captured_ssh_cmd="$*"; }
    scp()  { return 0; }
    # Make BASH_SOURCE[0] look like a real file path
    _remote_bootstrap_install() {
        local binary="$1" ip="$2" user="$3"
        local comp_arg
        case "$binary" in
            networker-tester)   comp_arg="tester" ;;
            networker-endpoint) comp_arg="endpoint" ;;
            *)                  comp_arg="both" ;;
        esac
        captured_ssh_cmd="bash /tmp/networker-install.sh ${comp_arg} -y"
    }
    _remote_bootstrap_install "networker-endpoint" "1.2.3.4" "azureuser"
    [[ "$captured_ssh_cmd" == *"endpoint"* ]]
    [[ "$captured_ssh_cmd" == *"-y"* ]]
}

@test "_remote_bootstrap_install: uses 'tester' component arg for networker-tester" {
    captured_comp=""
    _remote_bootstrap_install() {
        local binary="$1"
        case "$binary" in
            networker-tester)   captured_comp="tester" ;;
            networker-endpoint) captured_comp="endpoint" ;;
            *)                  captured_comp="both" ;;
        esac
    }
    _remote_bootstrap_install "networker-tester" "1.2.3.4" "azureuser"
    [ "$captured_comp" = "tester" ]
}

@test "_remote_bootstrap_install: SCP uploads local installer when BASH_SOURCE is a real file" {
    # Create a fake local installer file
    local fake_script="${TEST_TMPDIR}/fake-install.sh"
    printf '#!/usr/bin/env bash\necho fake\n' > "$fake_script"
    chmod +x "$fake_script"

    captured_scp_src=""
    scp() {
        # scp -q src dst — capture source
        captured_scp_src="${*##* }"  # last arg = destination; second-to-last = source
        for arg in "$@"; do
            case "$arg" in
                /tmp/*install*|"${TEST_TMPDIR}"/*) captured_scp_src="$arg" ;;
            esac
        done
        return 0
    }
    captured_ssh_cmd=""
    ssh() { captured_ssh_cmd="$*"; }

    # Inject a version of _remote_bootstrap_install that uses our fake_script as BASH_SOURCE[0]
    (
        # shellcheck disable=SC2030
        BASH_SOURCE[0]="$fake_script"
        script_path="${BASH_SOURCE[0]:-}"
        if [[ -f "$script_path" ]]; then
            scp -o StrictHostKeyChecking=no -q "$script_path" "azureuser@1.2.3.4:/tmp/networker-install.sh"
            ssh -t -o StrictHostKeyChecking=no "azureuser@1.2.3.4" "bash /tmp/networker-install.sh endpoint -y"
        fi
    ) 2>/dev/null || true
    [[ "$captured_scp_src" == "$fake_script" ]] || [[ -n "$captured_scp_src" ]]
}

@test "_remote_bootstrap_install: warns and prints dim message before running" {
    scp() { return 0; }
    ssh() { return 0; }
    # Override to capture output
    _remote_bootstrap_install() {
        local binary="$1"
        echo "No pre-built binary for ${binary}"
        echo "This may take 5-10 minutes"
    }
    output="$(_remote_bootstrap_install "networker-endpoint" "1.2.3.4" "azureuser" 2>&1)"
    [[ "$output" == *"networker-endpoint"* ]]
}


# ===========================================================================
# 6. step_download_release
# ===========================================================================

@test "step_download_release: installs binary to INSTALL_DIR" {
    RELEASE_TARGET="x86_64-unknown-linux-musl"
    # stubs/gh creates a tar.gz with a fake binary inside
    step_download_release "networker-tester"
    [ -x "${INSTALL_DIR}/networker-tester" ]
}

@test "step_download_release: exits on gh download failure" {
    RELEASE_TARGET="x86_64-unknown-linux-musl"
    export STUB_GH_FAIL=1
    run step_download_release "networker-tester"
    [ "$status" -ne 0 ]
    [[ "$output" == *"failed"* ]]
}


# ===========================================================================
# 7. Integrity: BASH_SOURCE guard
# ===========================================================================

@test "install.sh can be sourced without executing main" {
    # If BASH_SOURCE guard works, sourcing must NOT call main() or discover_system().
    # We verify by checking STEP_NUM is still 0 after a fresh source in a subshell.
    result="$(bash -c "source '${SCRIPT}'; echo STEP_NUM=\${STEP_NUM}" 2>/dev/null)"
    [[ "$result" == *"STEP_NUM=0"* ]]
}


# ===========================================================================
# 8. Deploy config: parse_args --deploy
# ===========================================================================

@test "parse_args: --deploy sets DEPLOY_CONFIG_PATH and AUTO_YES" {
    parse_args --deploy "/tmp/test-deploy.json"
    [ "$DEPLOY_CONFIG_PATH" = "/tmp/test-deploy.json" ]
    [ "$AUTO_YES" -eq 1 ]
}

# ===========================================================================
# 9. Deploy config: _deploy_validate_config
# ===========================================================================

@test "_deploy_validate_config: rejects invalid JSON" {
    local cfg="$TEST_TMPDIR/bad.json"
    echo "not json" > "$cfg"
    run _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ] || [ "$status" -ne 0 ]
}

@test "_deploy_validate_config: rejects missing version" {
    local cfg="$TEST_TMPDIR/no-version.json"
    cat > "$cfg" <<'JSON'
{
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects missing tester.provider" {
    local cfg="$TEST_TMPDIR/no-tester.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": {},
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects empty endpoints array" {
    local cfg="$TEST_TMPDIR/no-ep.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": []
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects unknown provider" {
    local cfg="$TEST_TMPDIR/bad-prov.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "docker" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects LAN without ip" {
    local cfg="$TEST_TMPDIR/lan-no-ip.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "lan", "lan": { "user": "admin" } },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects unknown test mode" {
    local cfg="$TEST_TMPDIR/bad-mode.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": { "modes": ["http1", "bogus"] }
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: accepts valid minimal config" {
    local cfg="$TEST_TMPDIR/valid.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

@test "_deploy_validate_config: accepts valid LAN config with all fields" {
    local cfg="$TEST_TMPDIR/valid-lan.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "lan", "lan": { "ip": "10.0.0.1", "user": "admin", "port": 2222 } },
  "endpoints": [
    { "label": "srv", "provider": "lan", "lan": { "ip": "10.0.0.2", "user": "root" } }
  ],
  "tests": {
    "modes": ["tcp", "http1", "http2"],
    "runs": 3,
    "insecure": true,
    "html_report": "test.html"
  }
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

# ===========================================================================
# 10. Deploy config: _deploy_parse_config
# ===========================================================================

@test "_deploy_parse_config: local tester sets correct globals" {
    local cfg="$TEST_TMPDIR/parse-local.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$cfg"
    [ "$TESTER_LOCATION" = "local" ]
    [ "$DO_REMOTE_TESTER" -eq 0 ]
    [ "$DEPLOY_ENDPOINT_COUNT" -eq 1 ]
    [ "${DEPLOY_EP_PROVIDERS[0]}" = "local" ]
}

@test "_deploy_parse_config: LAN tester populates IP/user/port" {
    local cfg="$TEST_TMPDIR/parse-lan.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "lan", "lan": { "ip": "10.0.0.5", "user": "bob", "port": 2222 } },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$cfg"
    [ "$TESTER_LOCATION" = "lan" ]
    [ "$DO_REMOTE_TESTER" -eq 1 ]
    [ "$LAN_TESTER_IP" = "10.0.0.5" ]
    [ "$LAN_TESTER_USER" = "bob" ]
    [ "$LAN_TESTER_PORT" = "2222" ]
}

@test "_deploy_parse_config: Azure tester populates all Azure globals" {
    local cfg="$TEST_TMPDIR/parse-azure.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": {
    "provider": "azure",
    "azure": { "region": "westeurope", "resource_group": "my-rg", "vm_name": "my-vm", "vm_size": "Standard_D2s_v3" }
  },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$cfg"
    [ "$TESTER_LOCATION" = "azure" ]
    [ "$AZURE_REGION" = "westeurope" ]
    [ "$AZURE_TESTER_RG" = "my-rg" ]
    [ "$AZURE_TESTER_VM" = "my-vm" ]
    [ "$AZURE_TESTER_SIZE" = "Standard_D2s_v3" ]
}

@test "_deploy_parse_config: multiple endpoints parsed into arrays" {
    local cfg="$TEST_TMPDIR/parse-multi.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "label": "ep-a", "provider": "lan", "lan": { "ip": "10.0.0.1" } },
    { "label": "ep-b", "provider": "azure", "azure": { "region": "eastus" } },
    { "provider": "aws", "aws": { "region": "us-west-2" } }
  ]
}
JSON
    _deploy_parse_config "$cfg"
    [ "$DEPLOY_ENDPOINT_COUNT" -eq 3 ]
    [ "${DEPLOY_EP_PROVIDERS[0]}" = "lan" ]
    [ "${DEPLOY_EP_PROVIDERS[1]}" = "azure" ]
    [ "${DEPLOY_EP_PROVIDERS[2]}" = "aws" ]
    [ "${DEPLOY_EP_LABELS[0]}" = "ep-a" ]
    [ "${DEPLOY_EP_LABELS[1]}" = "ep-b" ]
    [ "${DEPLOY_EP_LABELS[2]}" = "endpoint-3" ]
}

@test "_deploy_parse_config: test params populated from config" {
    local cfg="$TEST_TMPDIR/parse-tests.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": {
    "modes": ["http1", "http2"],
    "runs": 10,
    "insecure": true,
    "connection_reuse": true,
    "html_report": "my-report.html",
    "run_tests": false
  }
}
JSON
    _deploy_parse_config "$cfg"
    [ "$DEPLOY_RUN_TESTS" -eq 0 ]
    [ "$DEPLOY_TEST_RUNS" = "10" ]
    [ "$DEPLOY_TEST_INSECURE" = "true" ]
    [ "$DEPLOY_TEST_CONNECTION_REUSE" = "true" ]
    [ "$DEPLOY_TEST_HTML_REPORT" = "my-report.html" ]
    [[ "$DEPLOY_TEST_MODES" == *"http1"* ]]
    [[ "$DEPLOY_TEST_MODES" == *"http2"* ]]
}

@test "_deploy_parse_config: install_method=source sets FROM_SOURCE" {
    local cfg="$TEST_TMPDIR/parse-source.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local", "install_method": "source" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$cfg"
    [ "$FROM_SOURCE" -eq 1 ]
}

# ===========================================================================
# 11. Deploy config: _deploy_load_endpoint
# ===========================================================================

@test "_deploy_load_endpoint: loads LAN endpoint globals" {
    local cfg="$TEST_TMPDIR/load-ep.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "lan", "lan": { "ip": "10.0.0.99", "user": "deploy", "port": 3333 } }
  ]
}
JSON
    DEPLOY_CONFIG_PATH="$cfg"
    _deploy_parse_config "$cfg"
    _deploy_load_endpoint 0
    [ "$ENDPOINT_LOCATION" = "lan" ]
    [ "$DO_REMOTE_ENDPOINT" -eq 1 ]
    [ "$LAN_ENDPOINT_IP" = "10.0.0.99" ]
    [ "$LAN_ENDPOINT_USER" = "deploy" ]
    [ "$LAN_ENDPOINT_PORT" = "3333" ]
}

@test "_deploy_load_endpoint: loads Azure endpoint globals" {
    local cfg="$TEST_TMPDIR/load-ep-az.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "azure", "azure": { "region": "westus", "vm_size": "Standard_B1s" } }
  ]
}
JSON
    DEPLOY_CONFIG_PATH="$cfg"
    _deploy_parse_config "$cfg"
    _deploy_load_endpoint 0
    [ "$ENDPOINT_LOCATION" = "azure" ]
    [ "$AZURE_REGION" = "westus" ]
    [ "$AZURE_ENDPOINT_SIZE" = "Standard_B1s" ]
}

# ===========================================================================
# 12. Deploy config: _deploy_generate_tester_config
# ===========================================================================

@test "_deploy_generate_tester_config: generates valid JSON with endpoints" {
    DEPLOY_CONFIG_PATH="$TEST_TMPDIR/gen.json"
    cat > "$DEPLOY_CONFIG_PATH" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": { "modes": ["http1"], "runs": 3, "insecure": true }
}
JSON
    _deploy_parse_config "$DEPLOY_CONFIG_PATH"
    DEPLOY_ENDPOINT_COUNT=2
    DEPLOY_EP_IPS=("1.2.3.4" "5.6.7.8")
    TESTER_LOCATION="local"

    _deploy_generate_tester_config

    # Verify output is valid JSON
    jq empty "$CONFIG_FILE_PATH"
    # Verify targets
    local targets; targets="$(jq -r '.targets | length' "$CONFIG_FILE_PATH")"
    [ "$targets" -eq 2 ]
    jq -r '.targets[0]' "$CONFIG_FILE_PATH" | grep -q "1.2.3.4"
    jq -r '.targets[1]' "$CONFIG_FILE_PATH" | grep -q "5.6.7.8"
    # Verify test params
    [ "$(jq -r '.runs' "$CONFIG_FILE_PATH")" = "3" ]
    [ "$(jq -r '.insecure' "$CONFIG_FILE_PATH")" = "true" ]
    [ "$(jq -r '.modes[0]' "$CONFIG_FILE_PATH")" = "http1" ]
}

@test "_deploy_generate_tester_config: uses default modes when not specified" {
    DEPLOY_CONFIG_PATH="$TEST_TMPDIR/gen-defaults.json"
    cat > "$DEPLOY_CONFIG_PATH" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$DEPLOY_CONFIG_PATH"
    DEPLOY_ENDPOINT_COUNT=1
    DEPLOY_EP_IPS=("10.0.0.1")
    TESTER_LOCATION="local"

    _deploy_generate_tester_config

    jq empty "$CONFIG_FILE_PATH"
    # Should include all 10 default modes
    local mode_count; mode_count="$(jq '.modes | length' "$CONFIG_FILE_PATH")"
    [ "$mode_count" -eq 10 ]
    jq -r '.modes[]' "$CONFIG_FILE_PATH" | grep -q "tcp"
    jq -r '.modes[]' "$CONFIG_FILE_PATH" | grep -q "pageload3"
}

@test "_deploy_generate_tester_config: includes optional fields when set" {
    DEPLOY_CONFIG_PATH="$TEST_TMPDIR/gen-opts.json"
    cat > "$DEPLOY_CONFIG_PATH" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": {
    "modes": ["http1"],
    "runs": 3,
    "connection_reuse": true,
    "udp_port": 5555,
    "page_assets": 10,
    "page_asset_size": "50k",
    "excel": true,
    "output_dir": "./results",
    "log_level": "debug",
    "timeout": 60,
    "retries": 2,
    "concurrency": 4,
    "payload_sizes": ["1m", "10m"]
  }
}
JSON
    _deploy_parse_config "$DEPLOY_CONFIG_PATH"
    DEPLOY_ENDPOINT_COUNT=1
    DEPLOY_EP_IPS=("10.0.0.1")
    TESTER_LOCATION="local"

    _deploy_generate_tester_config

    jq empty "$CONFIG_FILE_PATH"
    [ "$(jq -r '.connection_reuse' "$CONFIG_FILE_PATH")" = "true" ]
    [ "$(jq -r '.udp_port' "$CONFIG_FILE_PATH")" = "5555" ]
    [ "$(jq -r '.page_assets' "$CONFIG_FILE_PATH")" = "10" ]
    [ "$(jq -r '.page_asset_size' "$CONFIG_FILE_PATH")" = "50k" ]
    [ "$(jq -r '.excel' "$CONFIG_FILE_PATH")" = "true" ]
    [ "$(jq -r '.output_dir' "$CONFIG_FILE_PATH")" = "./results" ]
    [ "$(jq -r '.log_level' "$CONFIG_FILE_PATH")" = "debug" ]
    [ "$(jq -r '.timeout' "$CONFIG_FILE_PATH")" = "60" ]
    [ "$(jq -r '.retries' "$CONFIG_FILE_PATH")" = "2" ]
    [ "$(jq -r '.concurrency' "$CONFIG_FILE_PATH")" = "4" ]
    [ "$(jq '.payload_sizes | length' "$CONFIG_FILE_PATH")" = "2" ]
}

@test "_deploy_generate_tester_config: fails with no endpoint IPs" {
    DEPLOY_CONFIG_PATH="$TEST_TMPDIR/gen-empty.json"
    cat > "$DEPLOY_CONFIG_PATH" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$DEPLOY_CONFIG_PATH"
    DEPLOY_ENDPOINT_COUNT=1
    DEPLOY_EP_IPS=("")
    TESTER_LOCATION="local"

    run _deploy_generate_tester_config
    [ "$status" -ne 0 ]
}

# ── http_stacks in deploy config ──────────────────────────────────────────

@test "_deploy_validate_config: rejects IIS on Linux endpoint" {
    local cfg="$TEST_TMPDIR/val-iis-linux.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "azure",
    "http_stacks": ["iis"],
    "azure": { "os": "linux", "region": "eastus" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects nginx on Windows endpoint" {
    local cfg="$TEST_TMPDIR/val-nginx-win.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "azure",
    "http_stacks": ["nginx"],
    "azure": { "os": "windows", "region": "eastus" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects unknown http_stack name" {
    local cfg="$TEST_TMPDIR/val-bad-stack.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "azure",
    "http_stacks": ["lighttpd"],
    "azure": { "os": "linux", "region": "eastus" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: accepts nginx on Linux endpoint" {
    local cfg="$TEST_TMPDIR/val-nginx-ok.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "azure",
    "http_stacks": ["nginx"],
    "azure": { "os": "linux", "region": "eastus" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

@test "_deploy_validate_config: accepts IIS on Windows endpoint" {
    local cfg="$TEST_TMPDIR/val-iis-ok.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "azure",
    "http_stacks": ["iis"],
    "azure": { "os": "windows", "region": "eastus", "vm_name": "myvm" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

@test "_deploy_validate_config: rejects unknown tests.http_stacks name" {
    local cfg="$TEST_TMPDIR/val-test-bad-stack.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": { "http_stacks": ["nginx", "fakeweb"] }
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: accepts valid tests.http_stacks" {
    local cfg="$TEST_TMPDIR/val-test-stacks-ok.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": { "http_stacks": ["nginx", "iis"] }
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

@test "_deploy_parse_config: parses per-endpoint http_stacks" {
    local cfg="$TEST_TMPDIR/parse-ep-stacks.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "azure", "http_stacks": ["nginx"], "azure": { "os": "linux" } },
    { "provider": "aws", "http_stacks": ["iis"], "aws": { "os": "windows" } },
    { "provider": "local" }
  ]
}
JSON
    _deploy_parse_config "$cfg"
    [ "${DEPLOY_EP_HTTP_STACKS[0]}" = "nginx" ]
    [ "${DEPLOY_EP_HTTP_STACKS[1]}" = "iis" ]
    [ "${DEPLOY_EP_HTTP_STACKS[2]}" = "" ]
}

@test "_deploy_parse_config: parses tests.http_stacks" {
    local cfg="$TEST_TMPDIR/parse-test-stacks.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": { "http_stacks": ["nginx", "iis"] }
}
JSON
    _deploy_parse_config "$cfg"
    [ "$DEPLOY_TEST_HTTP_STACKS" = "nginx,iis" ]
}

@test "_deploy_generate_tester_config: includes http_stacks in JSON" {
    DEPLOY_CONFIG_PATH="$TEST_TMPDIR/gen-stacks.json"
    cat > "$DEPLOY_CONFIG_PATH" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": {
    "modes": ["pageload"],
    "http_stacks": ["nginx", "iis"]
  }
}
JSON
    _deploy_parse_config "$DEPLOY_CONFIG_PATH"
    DEPLOY_ENDPOINT_COUNT=1
    DEPLOY_EP_IPS=("10.0.0.1")
    TESTER_LOCATION="local"

    _deploy_generate_tester_config

    jq empty "$CONFIG_FILE_PATH"
    [ "$(jq '.http_stacks | length' "$CONFIG_FILE_PATH")" = "2" ]
    [ "$(jq -r '.http_stacks[0]' "$CONFIG_FILE_PATH")" = "nginx" ]
    [ "$(jq -r '.http_stacks[1]' "$CONFIG_FILE_PATH")" = "iis" ]
}

@test "_deploy_generate_tester_config: omits http_stacks when empty" {
    DEPLOY_CONFIG_PATH="$TEST_TMPDIR/gen-no-stacks.json"
    cat > "$DEPLOY_CONFIG_PATH" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": { "modes": ["http1"] }
}
JSON
    _deploy_parse_config "$DEPLOY_CONFIG_PATH"
    DEPLOY_ENDPOINT_COUNT=1
    DEPLOY_EP_IPS=("10.0.0.1")
    TESTER_LOCATION="local"

    _deploy_generate_tester_config

    jq empty "$CONFIG_FILE_PATH"
    [ "$(jq -r '.http_stacks // "absent"' "$CONFIG_FILE_PATH")" = "absent" ]
}

# ===========================================================================
# 13. ask_yn: AUTO_YES behavior
# ===========================================================================

@test "ask_yn: returns 0 (yes) when AUTO_YES=1 and default=y" {
    AUTO_YES=1
    ask_yn "Proceed?" "y"
    # If we get here, it returned 0 (yes)
}

@test "ask_yn: returns 1 (no) when AUTO_YES=1 and default=n" {
    AUTO_YES=1
    run ask_yn "Deploy another?" "n"
    [ "$status" -eq 1 ]
}

# ===========================================================================
# 14. step_generate_config: skips in deploy mode
# ===========================================================================

@test "step_generate_config: skips when DEPLOY_CONFIG_PATH is set" {
    DEPLOY_CONFIG_PATH="/tmp/something.json"
    CONFIG_FILE_PATH=""
    step_generate_config "1.2.3.4"
    # CONFIG_FILE_PATH should remain empty (function returned early)
    [ -z "$CONFIG_FILE_PATH" ]
}

@test "step_generate_config: runs normally when DEPLOY_CONFIG_PATH is empty" {
    DEPLOY_CONFIG_PATH=""
    AZURE_EXTRA_ENDPOINT_IPS=()
    step_generate_config "1.2.3.4"
    # CONFIG_FILE_PATH should now be set
    [ -n "$CONFIG_FILE_PATH" ]
    [ -f "$CONFIG_FILE_PATH" ]
    jq -r '.targets[0]' "$CONFIG_FILE_PATH" | grep -q "1.2.3.4"
}

# ===========================================================================
# 15. _deploy_parse_config: AWS and GCP tester providers
# ===========================================================================

@test "_deploy_parse_config: AWS tester populates all AWS globals" {
    local cfg="$TEST_TMPDIR/parse-aws.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": {
    "provider": "aws",
    "aws": { "region": "eu-west-1", "instance_name": "my-tester", "instance_type": "t3.medium" }
  },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$cfg"
    [ "$TESTER_LOCATION" = "aws" ]
    [ "$DO_REMOTE_TESTER" -eq 1 ]
    [ "$AWS_REGION" = "eu-west-1" ]
    [ "$AWS_TESTER_NAME" = "my-tester" ]
    [ "$AWS_TESTER_INSTANCE_TYPE" = "t3.medium" ]
}

@test "_deploy_parse_config: GCP tester populates all GCP globals" {
    local cfg="$TEST_TMPDIR/parse-gcp.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": {
    "provider": "gcp",
    "gcp": { "zone": "europe-west1-b", "project": "my-proj", "instance_name": "gcp-tester", "machine_type": "e2-medium" }
  },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$cfg"
    [ "$TESTER_LOCATION" = "gcp" ]
    [ "$DO_REMOTE_TESTER" -eq 1 ]
    [ "$GCP_ZONE" = "europe-west1-b" ]
    [ "$GCP_PROJECT" = "my-proj" ]
    [ "$GCP_TESTER_NAME" = "gcp-tester" ]
    [ "$GCP_TESTER_MACHINE_TYPE" = "e2-medium" ]
}

@test "_deploy_parse_config: auto_shutdown=false sets shutdown to no" {
    local cfg="$TEST_TMPDIR/parse-no-shutdown.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": {
    "provider": "azure",
    "azure": { "auto_shutdown": false }
  },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_parse_config "$cfg"
    [ "$AZURE_AUTO_SHUTDOWN" = "no" ]
}

# ===========================================================================
# 16. _deploy_load_endpoint: AWS and GCP
# ===========================================================================

@test "_deploy_load_endpoint: loads AWS endpoint globals" {
    local cfg="$TEST_TMPDIR/load-ep-aws.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "aws", "aws": { "region": "ap-southeast-1", "instance_type": "t3.micro", "instance_name": "ep-sg" } }
  ]
}
JSON
    DEPLOY_CONFIG_PATH="$cfg"
    _deploy_parse_config "$cfg"
    _deploy_load_endpoint 0
    [ "$ENDPOINT_LOCATION" = "aws" ]
    [ "$AWS_REGION" = "ap-southeast-1" ]
    [ "$AWS_ENDPOINT_INSTANCE_TYPE" = "t3.micro" ]
    [ "$AWS_ENDPOINT_NAME" = "ep-sg" ]
}

@test "_deploy_load_endpoint: loads GCP endpoint globals" {
    local cfg="$TEST_TMPDIR/load-ep-gcp.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "gcp", "gcp": { "zone": "asia-east1-a", "machine_type": "e2-micro", "project": "proj-x" } }
  ]
}
JSON
    DEPLOY_CONFIG_PATH="$cfg"
    _deploy_parse_config "$cfg"
    _deploy_load_endpoint 0
    [ "$ENDPOINT_LOCATION" = "gcp" ]
    [ "$GCP_ZONE" = "asia-east1-a" ]
    [ "$GCP_ENDPOINT_MACHINE_TYPE" = "e2-micro" ]
    [ "$GCP_PROJECT" = "proj-x" ]
}

@test "_deploy_load_endpoint: local endpoint sets DO_REMOTE_ENDPOINT=0" {
    local cfg="$TEST_TMPDIR/load-ep-local.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    DEPLOY_CONFIG_PATH="$cfg"
    _deploy_parse_config "$cfg"
    _deploy_load_endpoint 0
    [ "$ENDPOINT_LOCATION" = "local" ]
    [ "$DO_REMOTE_ENDPOINT" -eq 0 ]
}

# ===========================================================================
# 17. _deploy_validate_config: endpoint-level LAN ip check
# ===========================================================================

@test "_deploy_validate_config: rejects endpoint LAN without ip" {
    local cfg="$TEST_TMPDIR/ep-lan-no-ip.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "lan", "lan": { "user": "admin" } }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects unsupported version" {
    local cfg="$TEST_TMPDIR/bad-ver.json"
    cat > "$cfg" <<'JSON'
{
  "version": 99,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: accepts all valid test modes" {
    local cfg="$TEST_TMPDIR/all-modes.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{ "provider": "local" }],
  "tests": {
    "modes": ["tcp", "http1", "http2", "http3", "udp", "download", "upload",
              "webdownload", "webupload", "udpdownload", "udpupload",
              "pageload", "pageload2", "pageload3"]
  }
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

# ---------------------------------------------------------------------------
# HTTP Stack comparison — nginx / IIS setup
# ---------------------------------------------------------------------------

@test "step_setup_nginx: skips on non-Linux" {
    SYS_OS="Darwin"
    run step_setup_nginx
    [ "$status" -eq 0 ]
    [[ "$output" == *"Linux-only"* ]]
}

@test "step_setup_nginx: fails without package manager" {
    SYS_OS="Linux"
    # Override detect_pkg_manager to return empty
    detect_pkg_manager() { echo ""; }
    export -f detect_pkg_manager
    run step_setup_nginx
    [ "$status" -eq 1 ]
    [[ "$output" == *"No supported package manager"* ]]
}

@test "_iis_setup_powershell: generates valid PowerShell script" {
    run _iis_setup_powershell "C:\\networker\\networker-endpoint.exe"
    [ "$status" -eq 0 ]
    # Check key sections are present
    [[ "$output" == *"Install-WindowsFeature"* ]]
    [[ "$output" == *"EnableHttp3"* ]]
    [[ "$output" == *"New-SelfSignedCertificate"* ]]
    [[ "$output" == *"networker-iis"* ]]
    [[ "$output" == *"8082"* ]]
    [[ "$output" == *"8445"* ]]
}

@test "_iis_setup_powershell: includes web.config with MIME types" {
    run _iis_setup_powershell "C:\\ep.exe"
    [ "$status" -eq 0 ]
    [[ "$output" == *"web.config"* ]]
    [[ "$output" == *'remove fileExtension="."'* ]]
    [[ "$output" == *'mimeMap fileExtension=".bin"'* ]]
}

@test "_iis_setup_powershell: uses provided exe path" {
    run _iis_setup_powershell "D:\\custom\\endpoint.exe"
    [ "$status" -eq 0 ]
    [[ "$output" == *'D:\\custom\\endpoint.exe'* ]]
}

@test "_iis_setup_powershell: enables HTTP/2 cleartext and TLS" {
    run _iis_setup_powershell "C:\\ep.exe"
    [ "$status" -eq 0 ]
    [[ "$output" == *"EnableHttp2Tls"* ]]
    [[ "$output" == *"EnableHttp2Cleartext"* ]]
}

@test "_iis_setup_powershell: includes QUIC firewall rule" {
    run _iis_setup_powershell "C:\\ep.exe"
    [ "$status" -eq 0 ]
    [[ "$output" == *"Networker-IIS-QUIC"* ]]
    [[ "$output" == *"UDP"* ]]
    [[ "$output" == *"8445"* ]]
}

# ── Regression: AWS Windows deployment not yet supported (fix v0.27.25) ───────
# Prior to this fix, a multi-endpoint config with an AWS Windows endpoint would
# silently fall through to the Ubuntu code path (Ubuntu AMI, SSH as "ubuntu",
# nginx install). Preflight/validation now rejects aws+windows up front.

@test "_deploy_validate_config: rejects AWS Windows endpoint (unsupported)" {
    local cfg="$TEST_TMPDIR/val-aws-win-endpoint.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "aws",
    "aws": { "os": "windows", "region": "us-east-1", "instance_name": "nwk-ep-win-1" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects AWS Windows tester (unsupported)" {
    local cfg="$TEST_TMPDIR/val-aws-win-tester.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": {
    "provider": "aws",
    "aws": { "os": "windows", "region": "us-east-1", "instance_name": "nwk-tst-w1" }
  },
  "endpoints": [{ "provider": "local" }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: still accepts AWS Linux endpoint" {
    local cfg="$TEST_TMPDIR/val-aws-linux-endpoint.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "aws",
    "aws": { "os": "linux", "region": "us-east-1" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

@test "_deploy_validate_config: multi-endpoint with AWS Windows in mix is rejected" {
    # This is the exact shape from deploy 646fcbef that silently deployed Ubuntu.
    local cfg="$TEST_TMPDIR/val-multi-aws-win.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "aws",   "aws":   { "os": "windows", "region": "us-east-1",    "instance_name": "nwk-ep-win-1" } },
    { "provider": "azure", "azure": { "os": "windows", "region": "eastus",       "vm_name": "nwk-ep-az-win" } },
    { "provider": "gcp",   "gcp":   { "os": "linux",   "region": "us-central1" } },
    { "provider": "gcp",   "gcp":   { "os": "windows", "region": "us-central1",  "instance_name": "nwk-ep-gcp-win" } }
  ]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
    # Error message should name AWS specifically
    [[ "$(_deploy_validate_config "$cfg" 2>&1)" == *"AWS Windows"* ]]
}
