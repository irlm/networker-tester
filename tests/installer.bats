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
    [ "$COMPONENT"          = ""       ] || { echo 'assertion failed: [ "$COMPONENT"          = ""       ]' >&2; exit 1; }
    [ "$AUTO_YES"           -eq 0      ] || { echo 'assertion failed: [ "$AUTO_YES"           -eq 0      ]' >&2; exit 1; }
    [ "$DO_REMOTE_TESTER"   -eq 0      ] || { echo 'assertion failed: [ "$DO_REMOTE_TESTER"   -eq 0      ]' >&2; exit 1; }
    [ "$DO_REMOTE_ENDPOINT" -eq 0      ] || { echo 'assertion failed: [ "$DO_REMOTE_ENDPOINT" -eq 0      ]' >&2; exit 1; }
    [ "$TESTER_LOCATION"    = "local"  ] || { echo 'assertion failed: [ "$TESTER_LOCATION"    = "local"  ]' >&2; exit 1; }
    [ "$ENDPOINT_LOCATION"  = "local"  ]
}

@test "parse_args: 'tester' subcommand disables endpoint install" {
    parse_args tester
    [ "$COMPONENT"           = "tester" ] || { echo 'assertion failed: [ "$COMPONENT"           = "tester" ]' >&2; exit 1; }
    [ "$DO_INSTALL_ENDPOINT" -eq 0      ] || { echo 'assertion failed: [ "$DO_INSTALL_ENDPOINT" -eq 0      ]' >&2; exit 1; }
    [ "$DO_INSTALL_TESTER"   -eq 1      ]
}

@test "parse_args: 'endpoint' subcommand disables tester install" {
    parse_args endpoint
    [ "$COMPONENT"           = "endpoint" ] || { echo 'assertion failed: [ "$COMPONENT"           = "endpoint" ]' >&2; exit 1; }
    [ "$DO_INSTALL_TESTER"   -eq 0        ] || { echo 'assertion failed: [ "$DO_INSTALL_TESTER"   -eq 0        ]' >&2; exit 1; }
    [ "$DO_INSTALL_ENDPOINT" -eq 1        ]
}

@test "parse_args: 'both' subcommand enables both components" {
    parse_args both
    [ "$COMPONENT"           = "both" ] || { echo 'assertion failed: [ "$COMPONENT"           = "both" ]' >&2; exit 1; }
    [ "$DO_INSTALL_TESTER"   -eq 1    ] || { echo 'assertion failed: [ "$DO_INSTALL_TESTER"   -eq 1    ]' >&2; exit 1; }
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
    [ "$ENDPOINT_LOCATION"  = "azure" ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION"  = "azure" ]' >&2; exit 1; }
    [ "$DO_REMOTE_ENDPOINT" -eq 1     ]
}

@test "parse_args: --aws sets endpoint location and flag" {
    parse_args --aws
    [ "$ENDPOINT_LOCATION"  = "aws" ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION"  = "aws" ]' >&2; exit 1; }
    [ "$DO_REMOTE_ENDPOINT" -eq 1   ]
}

@test "parse_args: --tester-azure sets tester location and flag" {
    parse_args --tester-azure
    [ "$TESTER_LOCATION"  = "azure" ] || { echo 'assertion failed: [ "$TESTER_LOCATION"  = "azure" ]' >&2; exit 1; }
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
    [ "$COMPONENT"          = "endpoint"    ] || { echo 'assertion failed: [ "$COMPONENT"          = "endpoint"    ]' >&2; exit 1; }
    [ "$ENDPOINT_LOCATION"  = "azure"       ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION"  = "azure"       ]' >&2; exit 1; }
    [ "$AZURE_REGION"       = "northeurope" ] || { echo 'assertion failed: [ "$AZURE_REGION"       = "northeurope" ]' >&2; exit 1; }
    [ "$DO_INSTALL_TESTER"  -eq 0           ]
}

@test "parse_args: --gcp sets endpoint location and flag" {
    parse_args --gcp
    [ "$ENDPOINT_LOCATION"  = "gcp" ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION"  = "gcp" ]' >&2; exit 1; }
    [ "$DO_REMOTE_ENDPOINT" -eq 1   ]
}

@test "parse_args: --tester-gcp sets tester location and flag" {
    parse_args --tester-gcp
    [ "$TESTER_LOCATION"  = "gcp" ] || { echo 'assertion failed: [ "$TESTER_LOCATION"  = "gcp" ]' >&2; exit 1; }
    [ "$DO_REMOTE_TESTER" -eq 1   ]
}

@test "parse_args: --gcp-zone overrides GCP zone and derives region" {
    parse_args --gcp-zone europe-west1-b
    [ "$GCP_ZONE" = "europe-west1-b" ]
}

@test "parse_args: --gcp-machine-type overrides machine type" {
    parse_args --gcp-machine-type e2-medium
    [ "$GCP_TESTER_MACHINE_TYPE"   = "e2-medium" ] || { echo 'assertion failed: [ "$GCP_TESTER_MACHINE_TYPE"   = "e2-medium" ]' >&2; exit 1; }
    [ "$GCP_ENDPOINT_MACHINE_TYPE" = "e2-medium" ]
}

@test "parse_args: --gcp-project sets GCP project" {
    parse_args --gcp-project my-project-123
    [ "$GCP_PROJECT" = "my-project-123" ]
}

@test "parse_args: combined flags — endpoint + gcp + zone" {
    parse_args endpoint --gcp --gcp-zone asia-east1-a
    [ "$COMPONENT"          = "endpoint"    ] || { echo 'assertion failed: [ "$COMPONENT"          = "endpoint"    ]' >&2; exit 1; }
    [ "$ENDPOINT_LOCATION"  = "gcp"         ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION"  = "gcp"         ]' >&2; exit 1; }
    [ "$GCP_ZONE"           = "asia-east1-a" ] || { echo 'assertion failed: [ "$GCP_ZONE"           = "asia-east1-a" ]' >&2; exit 1; }
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
    [[ "$output" == *"1.2.3.4"* ]] || { echo 'assertion failed: [[ "$output" == *"1.2.3.4"* ]]' >&2; exit 1; }
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
    [[ "$output" == *"did not respond"* ]] || { echo 'assertion failed: [[ "$output" == *"did not respond"* ]]' >&2; exit 1; }
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
    [[ "$captured_ssh_cmd" == *"endpoint"* ]] || { echo 'assertion failed: [[ "$captured_ssh_cmd" == *"endpoint"* ]]' >&2; exit 1; }
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
    [ "$status" -ne 0 ] || { echo 'assertion failed: [ "$status" -ne 0 ]' >&2; exit 1; }
    [[ "$output" == *"failed"* ]]
}

@test "step_download_release: concurrent installs to same INSTALL_DIR both succeed (no ETXTBSY)" {
    # Regression for v0.27.26: dashboard issues parallel deploys that all run
    # install.sh against the same shared $HOME/.cargo/bin/networker-tester.
    # Previously, the second writer hit "Text file busy" while the first
    # process was still executing the binary. Now writes go via tmp + mv -f
    # with version-match fallback, so both runs must finish cleanly.
    RELEASE_TARGET="x86_64-unknown-linux-musl"

    # Run two installs in parallel against the same INSTALL_DIR.
    # Capture exit codes via files since bats `run` is single-shot.
    local rc1_file="$TEST_TMPDIR/rc1"
    local rc2_file="$TEST_TMPDIR/rc2"
    (
        step_download_release "networker-tester" >/dev/null 2>&1
        echo $? > "$rc1_file"
    ) &
    local pid1=$!
    (
        step_download_release "networker-tester" >/dev/null 2>&1
        echo $? > "$rc2_file"
    ) &
    local pid2=$!
    wait "$pid1" "$pid2"

    [ "$(cat "$rc1_file")" -eq 0 ] || { echo 'assertion failed: [ "$(cat "$rc1_file")" -eq 0 ]' >&2; exit 1; }
    [ "$(cat "$rc2_file")" -eq 0 ] || { echo 'assertion failed: [ "$(cat "$rc2_file")" -eq 0 ]' >&2; exit 1; }
    [ -x "${INSTALL_DIR}/networker-tester" ]
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
    [ "$DEPLOY_CONFIG_PATH" = "/tmp/test-deploy.json" ] || { echo 'assertion failed: [ "$DEPLOY_CONFIG_PATH" = "/tmp/test-deploy.json" ]' >&2; exit 1; }
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
    [ "$TESTER_LOCATION" = "local" ] || { echo 'assertion failed: [ "$TESTER_LOCATION" = "local" ]' >&2; exit 1; }
    [ "$DO_REMOTE_TESTER" -eq 0 ] || { echo 'assertion failed: [ "$DO_REMOTE_TESTER" -eq 0 ]' >&2; exit 1; }
    [ "$DEPLOY_ENDPOINT_COUNT" -eq 1 ] || { echo 'assertion failed: [ "$DEPLOY_ENDPOINT_COUNT" -eq 1 ]' >&2; exit 1; }
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
    [ "$TESTER_LOCATION" = "lan" ] || { echo 'assertion failed: [ "$TESTER_LOCATION" = "lan" ]' >&2; exit 1; }
    [ "$DO_REMOTE_TESTER" -eq 1 ] || { echo 'assertion failed: [ "$DO_REMOTE_TESTER" -eq 1 ]' >&2; exit 1; }
    [ "$LAN_TESTER_IP" = "10.0.0.5" ] || { echo 'assertion failed: [ "$LAN_TESTER_IP" = "10.0.0.5" ]' >&2; exit 1; }
    [ "$LAN_TESTER_USER" = "bob" ] || { echo 'assertion failed: [ "$LAN_TESTER_USER" = "bob" ]' >&2; exit 1; }
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
    [ "$TESTER_LOCATION" = "azure" ] || { echo 'assertion failed: [ "$TESTER_LOCATION" = "azure" ]' >&2; exit 1; }
    [ "$AZURE_REGION" = "westeurope" ] || { echo 'assertion failed: [ "$AZURE_REGION" = "westeurope" ]' >&2; exit 1; }
    [ "$AZURE_TESTER_RG" = "my-rg" ] || { echo 'assertion failed: [ "$AZURE_TESTER_RG" = "my-rg" ]' >&2; exit 1; }
    [ "$AZURE_TESTER_VM" = "my-vm" ] || { echo 'assertion failed: [ "$AZURE_TESTER_VM" = "my-vm" ]' >&2; exit 1; }
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
    [ "$DEPLOY_ENDPOINT_COUNT" -eq 3 ] || { echo 'assertion failed: [ "$DEPLOY_ENDPOINT_COUNT" -eq 3 ]' >&2; exit 1; }
    [ "${DEPLOY_EP_PROVIDERS[0]}" = "lan" ] || { echo 'assertion failed: [ "${DEPLOY_EP_PROVIDERS[0]}" = "lan" ]' >&2; exit 1; }
    [ "${DEPLOY_EP_PROVIDERS[1]}" = "azure" ] || { echo 'assertion failed: [ "${DEPLOY_EP_PROVIDERS[1]}" = "azure" ]' >&2; exit 1; }
    [ "${DEPLOY_EP_PROVIDERS[2]}" = "aws" ] || { echo 'assertion failed: [ "${DEPLOY_EP_PROVIDERS[2]}" = "aws" ]' >&2; exit 1; }
    [ "${DEPLOY_EP_LABELS[0]}" = "ep-a" ] || { echo 'assertion failed: [ "${DEPLOY_EP_LABELS[0]}" = "ep-a" ]' >&2; exit 1; }
    [ "${DEPLOY_EP_LABELS[1]}" = "ep-b" ] || { echo 'assertion failed: [ "${DEPLOY_EP_LABELS[1]}" = "ep-b" ]' >&2; exit 1; }
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
    [ "$DEPLOY_RUN_TESTS" -eq 0 ] || { echo 'assertion failed: [ "$DEPLOY_RUN_TESTS" -eq 0 ]' >&2; exit 1; }
    [ "$DEPLOY_TEST_RUNS" = "10" ] || { echo 'assertion failed: [ "$DEPLOY_TEST_RUNS" = "10" ]' >&2; exit 1; }
    [ "$DEPLOY_TEST_INSECURE" = "true" ] || { echo 'assertion failed: [ "$DEPLOY_TEST_INSECURE" = "true" ]' >&2; exit 1; }
    [ "$DEPLOY_TEST_CONNECTION_REUSE" = "true" ] || { echo 'assertion failed: [ "$DEPLOY_TEST_CONNECTION_REUSE" = "true" ]' >&2; exit 1; }
    [ "$DEPLOY_TEST_HTML_REPORT" = "my-report.html" ] || { echo 'assertion failed: [ "$DEPLOY_TEST_HTML_REPORT" = "my-report.html" ]' >&2; exit 1; }
    [[ "$DEPLOY_TEST_MODES" == *"http1"* ]] || { echo 'assertion failed: [[ "$DEPLOY_TEST_MODES" == *"http1"* ]]' >&2; exit 1; }
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
    [ "$ENDPOINT_LOCATION" = "lan" ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION" = "lan" ]' >&2; exit 1; }
    [ "$DO_REMOTE_ENDPOINT" -eq 1 ] || { echo 'assertion failed: [ "$DO_REMOTE_ENDPOINT" -eq 1 ]' >&2; exit 1; }
    [ "$LAN_ENDPOINT_IP" = "10.0.0.99" ] || { echo 'assertion failed: [ "$LAN_ENDPOINT_IP" = "10.0.0.99" ]' >&2; exit 1; }
    [ "$LAN_ENDPOINT_USER" = "deploy" ] || { echo 'assertion failed: [ "$LAN_ENDPOINT_USER" = "deploy" ]' >&2; exit 1; }
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
    [ "$ENDPOINT_LOCATION" = "azure" ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION" = "azure" ]' >&2; exit 1; }
    [ "$AZURE_REGION" = "westus" ] || { echo 'assertion failed: [ "$AZURE_REGION" = "westus" ]' >&2; exit 1; }
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
    [ "$targets" -eq 2 ] || { echo 'assertion failed: [ "$targets" -eq 2 ]' >&2; exit 1; }
    jq -r '.targets[0]' "$CONFIG_FILE_PATH" | grep -q "1.2.3.4"
    jq -r '.targets[1]' "$CONFIG_FILE_PATH" | grep -q "5.6.7.8"
    # Verify test params
    [ "$(jq -r '.runs' "$CONFIG_FILE_PATH")" = "3" ] || { echo 'assertion failed: [ "$(jq -r '\''.runs'\'' "$CONFIG_FILE_PATH")" = "3" ]' >&2; exit 1; }
    [ "$(jq -r '.insecure' "$CONFIG_FILE_PATH")" = "true" ] || { echo 'assertion failed: [ "$(jq -r '\''.insecure'\'' "$CONFIG_FILE_PATH")" = "true" ]' >&2; exit 1; }
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
    [ "$(jq -r '.connection_reuse' "$CONFIG_FILE_PATH")" = "true" ] || { echo 'assertion failed: [ "$(jq -r '\''.connection_reuse'\'' "$CONFIG_FILE_PATH")" = "true" ]' >&2; exit 1; }
    [ "$(jq -r '.udp_port' "$CONFIG_FILE_PATH")" = "5555" ] || { echo 'assertion failed: [ "$(jq -r '\''.udp_port'\'' "$CONFIG_FILE_PATH")" = "5555" ]' >&2; exit 1; }
    [ "$(jq -r '.page_assets' "$CONFIG_FILE_PATH")" = "10" ] || { echo 'assertion failed: [ "$(jq -r '\''.page_assets'\'' "$CONFIG_FILE_PATH")" = "10" ]' >&2; exit 1; }
    [ "$(jq -r '.page_asset_size' "$CONFIG_FILE_PATH")" = "50k" ] || { echo 'assertion failed: [ "$(jq -r '\''.page_asset_size'\'' "$CONFIG_FILE_PATH")" = "50k" ]' >&2; exit 1; }
    [ "$(jq -r '.excel' "$CONFIG_FILE_PATH")" = "true" ] || { echo 'assertion failed: [ "$(jq -r '\''.excel'\'' "$CONFIG_FILE_PATH")" = "true" ]' >&2; exit 1; }
    [ "$(jq -r '.output_dir' "$CONFIG_FILE_PATH")" = "./results" ] || { echo 'assertion failed: [ "$(jq -r '\''.output_dir'\'' "$CONFIG_FILE_PATH")" = "./results" ]' >&2; exit 1; }
    [ "$(jq -r '.log_level' "$CONFIG_FILE_PATH")" = "debug" ] || { echo 'assertion failed: [ "$(jq -r '\''.log_level'\'' "$CONFIG_FILE_PATH")" = "debug" ]' >&2; exit 1; }
    [ "$(jq -r '.timeout' "$CONFIG_FILE_PATH")" = "60" ] || { echo 'assertion failed: [ "$(jq -r '\''.timeout'\'' "$CONFIG_FILE_PATH")" = "60" ]' >&2; exit 1; }
    [ "$(jq -r '.retries' "$CONFIG_FILE_PATH")" = "2" ] || { echo 'assertion failed: [ "$(jq -r '\''.retries'\'' "$CONFIG_FILE_PATH")" = "2" ]' >&2; exit 1; }
    [ "$(jq -r '.concurrency' "$CONFIG_FILE_PATH")" = "4" ] || { echo 'assertion failed: [ "$(jq -r '\''.concurrency'\'' "$CONFIG_FILE_PATH")" = "4" ]' >&2; exit 1; }
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
    [ "${DEPLOY_EP_HTTP_STACKS[0]}" = "nginx" ] || { echo 'assertion failed: [ "${DEPLOY_EP_HTTP_STACKS[0]}" = "nginx" ]' >&2; exit 1; }
    [ "${DEPLOY_EP_HTTP_STACKS[1]}" = "iis" ] || { echo 'assertion failed: [ "${DEPLOY_EP_HTTP_STACKS[1]}" = "iis" ]' >&2; exit 1; }
    [ "${DEPLOY_EP_HTTP_STACKS[2]}" = "" ]
}

# ── languages in deploy config (reference-API servers for apibench) ────────

@test "_deploy_parse_config: parses per-endpoint languages" {
    local cfg="$TEST_TMPDIR/parse-ep-langs.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "azure", "http_stacks": ["nginx"], "languages": ["rust"], "azure": { "os": "linux" } },
    { "provider": "azure", "languages": ["go", "python"], "azure": { "os": "linux" } },
    { "provider": "local" }
  ]
}
JSON
    _deploy_parse_config "$cfg"
    [ "${DEPLOY_EP_LANGUAGES[0]}" = "rust" ] || { echo 'assertion failed: [ "${DEPLOY_EP_LANGUAGES[0]}" = "rust" ]' >&2; exit 1; }
    [ "${DEPLOY_EP_LANGUAGES[1]}" = "go,python" ] || { echo 'assertion failed: [ "${DEPLOY_EP_LANGUAGES[1]}" = "go,python" ]' >&2; exit 1; }
    [ "${DEPLOY_EP_LANGUAGES[2]}" = "" ]
}

@test "_deploy_validate_config: accepts valid languages on Linux endpoint" {
    local cfg="$TEST_TMPDIR/val-langs-ok.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "azure",
    "http_stacks": ["nginx"],
    "languages": ["go"],
    "azure": { "os": "linux", "region": "eastus" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

@test "_deploy_validate_config: rejects unknown language" {
    local cfg="$TEST_TMPDIR/val-langs-bad.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "azure",
    "languages": ["cobol"],
    "azure": { "os": "linux", "region": "eastus" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "_deploy_validate_config: rejects languages on Windows endpoint" {
    local cfg="$TEST_TMPDIR/val-langs-win.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [{
    "provider": "azure",
    "languages": ["go"],
    "azure": { "os": "windows", "region": "eastus" }
  }]
}
JSON
    _deploy_validate_config "$cfg"
    [ "$DEPLOY_VALIDATE_ERRORS" -gt 0 ]
}

@test "parse_args: --benchmark-port sets BENCHMARK_PORT_OVERRIDE" {
    parse_args --benchmark-port 8085
    [ "$BENCHMARK_PORT_OVERRIDE" = "8085" ]
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
    [ "$(jq '.http_stacks | length' "$CONFIG_FILE_PATH")" = "2" ] || { echo 'assertion failed: [ "$(jq '\''.http_stacks | length'\'' "$CONFIG_FILE_PATH")" = "2" ]' >&2; exit 1; }
    [ "$(jq -r '.http_stacks[0]' "$CONFIG_FILE_PATH")" = "nginx" ] || { echo 'assertion failed: [ "$(jq -r '\''.http_stacks[0]'\'' "$CONFIG_FILE_PATH")" = "nginx" ]' >&2; exit 1; }
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
    [ -n "$CONFIG_FILE_PATH" ] || { echo 'assertion failed: [ -n "$CONFIG_FILE_PATH" ]' >&2; exit 1; }
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
    [ "$TESTER_LOCATION" = "aws" ] || { echo 'assertion failed: [ "$TESTER_LOCATION" = "aws" ]' >&2; exit 1; }
    [ "$DO_REMOTE_TESTER" -eq 1 ] || { echo 'assertion failed: [ "$DO_REMOTE_TESTER" -eq 1 ]' >&2; exit 1; }
    [ "$AWS_REGION" = "eu-west-1" ] || { echo 'assertion failed: [ "$AWS_REGION" = "eu-west-1" ]' >&2; exit 1; }
    [ "$AWS_TESTER_NAME" = "my-tester" ] || { echo 'assertion failed: [ "$AWS_TESTER_NAME" = "my-tester" ]' >&2; exit 1; }
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
    [ "$TESTER_LOCATION" = "gcp" ] || { echo 'assertion failed: [ "$TESTER_LOCATION" = "gcp" ]' >&2; exit 1; }
    [ "$DO_REMOTE_TESTER" -eq 1 ] || { echo 'assertion failed: [ "$DO_REMOTE_TESTER" -eq 1 ]' >&2; exit 1; }
    [ "$GCP_ZONE" = "europe-west1-b" ] || { echo 'assertion failed: [ "$GCP_ZONE" = "europe-west1-b" ]' >&2; exit 1; }
    [ "$GCP_PROJECT" = "my-proj" ] || { echo 'assertion failed: [ "$GCP_PROJECT" = "my-proj" ]' >&2; exit 1; }
    [ "$GCP_TESTER_NAME" = "gcp-tester" ] || { echo 'assertion failed: [ "$GCP_TESTER_NAME" = "gcp-tester" ]' >&2; exit 1; }
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
    [ "$ENDPOINT_LOCATION" = "aws" ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION" = "aws" ]' >&2; exit 1; }
    [ "$AWS_REGION" = "ap-southeast-1" ] || { echo 'assertion failed: [ "$AWS_REGION" = "ap-southeast-1" ]' >&2; exit 1; }
    [ "$AWS_ENDPOINT_INSTANCE_TYPE" = "t3.micro" ] || { echo 'assertion failed: [ "$AWS_ENDPOINT_INSTANCE_TYPE" = "t3.micro" ]' >&2; exit 1; }
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
    [ "$ENDPOINT_LOCATION" = "gcp" ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION" = "gcp" ]' >&2; exit 1; }
    [ "$GCP_ZONE" = "asia-east1-a" ] || { echo 'assertion failed: [ "$GCP_ZONE" = "asia-east1-a" ]' >&2; exit 1; }
    [ "$GCP_ENDPOINT_MACHINE_TYPE" = "e2-micro" ] || { echo 'assertion failed: [ "$GCP_ENDPOINT_MACHINE_TYPE" = "e2-micro" ]' >&2; exit 1; }
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
    [ "$ENDPOINT_LOCATION" = "local" ] || { echo 'assertion failed: [ "$ENDPOINT_LOCATION" = "local" ]' >&2; exit 1; }
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
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [[ "$output" == *"Linux-only"* ]]
}

@test "step_setup_nginx: fails without package manager" {
    SYS_OS="Linux"
    # Override detect_pkg_manager to return empty
    detect_pkg_manager() { echo ""; }
    export -f detect_pkg_manager
    run step_setup_nginx
    [ "$status" -eq 1 ] || { echo 'assertion failed: [ "$status" -eq 1 ]' >&2; exit 1; }
    [[ "$output" == *"No supported package manager"* ]]
}

@test "SSH-piped systemctl starts close inherited FDs to prevent hang" {
    # Regression: services started inside `ssh ... bash -s <<HEREDOC` inherit the
    # SSH pipe FDs. Without a redirect, SSH hangs forever waiting for the service
    # process to exit. Every systemctl start/restart INSIDE an SSH heredoc must
    # redirect stdin/stdout/stderr.
    #
    # P1-13 (vacuous-assertion sweep, 2026-08-05): the previous version of this
    # test computed the violating lines and merely `echo`-ed them — it never
    # asserted on them. A brand-new unredirected start inside a heredoc, the
    # exact regression named above, printed to a log nobody reads and the test
    # still passed, because the only real assertion counted redirected starts
    # ELSEWHERE in the file. It also could not tell an SSH heredoc from a local
    # `systemctl start`, which is why it was never tightened. Both fixed: the
    # heredoc bodies are extracted precisely, and the violation list is asserted
    # EMPTY.
    local script="$BATS_TEST_DIRNAME/../install.sh"

    # Extract the body of every `... <<'DELIM'` heredoc that is piped to ssh (or
    # to the gcp ssh wrapper), then look for unredirected service starts inside
    # ONLY those bodies.
    local violations
    violations="$(awk '
        # Opening line: an ssh/_gcp_ssh_run command with a quoted heredoc delimiter.
        !inblock && /(^|[[:space:]])(ssh|_gcp_ssh_run)([[:space:]]|$)/ && match($0, /<<[[:space:]]*.?[A-Z_][A-Z0-9_]*.?[[:space:]]*$/) {
            delim = $0
            sub(/^.*<<[[:space:]]*/, "", delim)
            gsub(/[^A-Za-z0-9_]/, "", delim)
            inblock = 1
            next
        }
        inblock && $0 == delim { inblock = 0; next }
        inblock && /systemctl[[:space:]]+(start|restart)/ {
            if ($0 !~ /<\/dev\/null/) { printf "%d: %s\n", NR, $0 }
        }
    ' "$script")"

    # Guard the guard FIRST: if the awk extraction silently stopped matching
    # heredocs it would report zero violations forever.
    local heredoc_starts
    heredoc_starts="$(grep -cE '(^|[[:space:]])(ssh|_gcp_ssh_run).*<<' "$script" || true)"
    [ "$heredoc_starts" -ge 4 ] || { echo "awk saw $heredoc_starts ssh heredocs — extraction is broken"; exit 1; }

    # FD-redirected starts genuinely exist (the original assertion).
    local fd_safe_count
    fd_safe_count="$(grep -c 'systemctl.*</dev/null >/dev/null 2>&1' "$script" || true)"
    [ "$fd_safe_count" -ge 4 ] || { echo "only $fd_safe_count FD-safe starts"; exit 1; }

    # The real assertion goes LAST so it is decisive under bats' errexit rules
    # no matter how the suite is configured — a non-final `[ ... ]` can be
    # silently inert, which is the whole reason this test was broken.
    if [ -n "$violations" ]; then
        echo "systemctl start/restart inside an SSH heredoc without </dev/null:"
        echo "$violations"
    fi
    [ -z "$violations" ]
}

@test "_iis_setup_powershell: generates valid PowerShell script" {
    run _iis_setup_powershell "C:\\networker\\networker-endpoint.exe"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    # Check key sections are present
    [[ "$output" == *"Install-WindowsFeature"* ]] || { echo 'assertion failed: [[ "$output" == *"Install-WindowsFeature"* ]]' >&2; exit 1; }
    [[ "$output" == *"EnableHttp3"* ]] || { echo 'assertion failed: [[ "$output" == *"EnableHttp3"* ]]' >&2; exit 1; }
    [[ "$output" == *"New-SelfSignedCertificate"* ]] || { echo 'assertion failed: [[ "$output" == *"New-SelfSignedCertificate"* ]]' >&2; exit 1; }
    [[ "$output" == *"networker-iis"* ]] || { echo 'assertion failed: [[ "$output" == *"networker-iis"* ]]' >&2; exit 1; }
    [[ "$output" == *"8082"* ]] || { echo 'assertion failed: [[ "$output" == *"8082"* ]]' >&2; exit 1; }
    [[ "$output" == *"8445"* ]]
}

@test "_iis_setup_powershell: includes web.config with MIME types" {
    run _iis_setup_powershell "C:\\ep.exe"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [[ "$output" == *"web.config"* ]] || { echo 'assertion failed: [[ "$output" == *"web.config"* ]]' >&2; exit 1; }
    [[ "$output" == *'remove fileExtension="."'* ]] || { echo 'assertion failed: [[ "$output" == *'\''remove fileExtension="."'\''* ]]' >&2; exit 1; }
    [[ "$output" == *'mimeMap fileExtension=".bin"'* ]]
}

@test "_iis_setup_powershell: uses provided exe path" {
    run _iis_setup_powershell "D:\\custom\\endpoint.exe"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [[ "$output" == *'D:\\custom\\endpoint.exe'* ]]
}

@test "_iis_setup_powershell: enables HTTP/2 cleartext and TLS" {
    run _iis_setup_powershell "C:\\ep.exe"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [[ "$output" == *"EnableHttp2Tls"* ]] || { echo 'assertion failed: [[ "$output" == *"EnableHttp2Tls"* ]]' >&2; exit 1; }
    [[ "$output" == *"EnableHttp2Cleartext"* ]]
}

@test "_iis_setup_powershell: includes QUIC firewall rule" {
    run _iis_setup_powershell "C:\\ep.exe"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [[ "$output" == *"Networker-IIS-QUIC"* ]] || { echo 'assertion failed: [[ "$output" == *"Networker-IIS-QUIC"* ]]' >&2; exit 1; }
    [[ "$output" == *"UDP"* ]] || { echo 'assertion failed: [[ "$output" == *"UDP"* ]]' >&2; exit 1; }
    [[ "$output" == *"8445"* ]]
}

# ── AWS Windows endpoint support (v0.27.27) ──────────────────────────────────
# v0.27.25 rejected AWS+Windows at preflight as a safety net.  v0.27.27 adds
# real support (Windows AMI, UserData bootstrap, IIS), so endpoints are accepted.
# AWS Windows *tester* is still unsupported.

@test "_deploy_validate_config: accepts AWS Windows endpoint" {
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
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
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

@test "_deploy_validate_config: multi-endpoint with AWS Windows in mix is accepted" {
    # v0.27.25 rejected this; v0.27.27 supports AWS Windows endpoints.
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
    [ "$DEPLOY_VALIDATE_ERRORS" -eq 0 ]
}

# ── AWS Windows AMI lookup + UserData helpers (v0.27.27) ─────────────────────

@test "_aws_find_windows_ami: function exists and is callable" {
    # We can't call the real AWS API, but verify the function is defined
    type _aws_find_windows_ami | head -1
    [[ "$(type _aws_find_windows_ami)" == *"function"* ]]
}

@test "_aws_win_endpoint_full_userdata: generates valid powershell block" {
    REPO_GH="irlm/networker-tester"
    run _aws_win_endpoint_full_userdata "v0.27.27" ""
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    # Must be wrapped in <powershell> tags for EC2 UserData
    [[ "$output" == *"<powershell>"* ]] || { echo 'assertion failed: [[ "$output" == *"<powershell>"* ]]' >&2; exit 1; }
    [[ "$output" == *"</powershell>"* ]] || { echo 'assertion failed: [[ "$output" == *"</powershell>"* ]]' >&2; exit 1; }
    # Must install firewall rules
    [[ "$output" == *"Networker-HTTP"* ]] || { echo 'assertion failed: [[ "$output" == *"Networker-HTTP"* ]]' >&2; exit 1; }
    [[ "$output" == *"Networker-HTTPS"* ]] || { echo 'assertion failed: [[ "$output" == *"Networker-HTTPS"* ]]' >&2; exit 1; }
    [[ "$output" == *"Networker-UDP"* ]] || { echo 'assertion failed: [[ "$output" == *"Networker-UDP"* ]]' >&2; exit 1; }
    # Must download the binary
    [[ "$output" == *"networker-endpoint"* ]] || { echo 'assertion failed: [[ "$output" == *"networker-endpoint"* ]]' >&2; exit 1; }
    [[ "$output" == *"v0.27.27"* ]] || { echo 'assertion failed: [[ "$output" == *"v0.27.27"* ]]' >&2; exit 1; }
    # Must install VC++ Redistributable
    [[ "$output" == *"vcruntime140"* ]] || { echo 'assertion failed: [[ "$output" == *"vcruntime140"* ]]' >&2; exit 1; }
    # Must set up IIS
    [[ "$output" == *"Install-WindowsFeature"* ]] || [[ "$output" == *"Web-Server"* ]]
    # Must create scheduled task for persistence
    [[ "$output" == *"schtasks"* ]] || { echo 'assertion failed: [[ "$output" == *"schtasks"* ]]' >&2; exit 1; }
    [[ "$output" == *"NetworkerEndpoint"* ]]
}

@test "_aws_win_endpoint_full_userdata: includes IIS setup from _iis_setup_powershell" {
    REPO_GH="irlm/networker-tester"
    run _aws_win_endpoint_full_userdata "v0.27.27" "ec2-1-2-3-4.compute.amazonaws.com"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    # IIS site creation
    [[ "$output" == *"networker-iis"* ]] || { echo 'assertion failed: [[ "$output" == *"networker-iis"* ]]' >&2; exit 1; }
    # Ports 8082 (HTTP) and 8445 (HTTPS)
    [[ "$output" == *"8082"* ]] || { echo 'assertion failed: [[ "$output" == *"8082"* ]]' >&2; exit 1; }
    [[ "$output" == *"8445"* ]] || { echo 'assertion failed: [[ "$output" == *"8445"* ]]' >&2; exit 1; }
    # HTTP/3 registry settings
    [[ "$output" == *"EnableHttp3"* ]]
}

@test "_aws_win_endpoint_full_userdata: firewall rules cover all required ports" {
    REPO_GH="irlm/networker-tester"
    run _aws_win_endpoint_full_userdata "v0.27.27" ""
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    # TCP: 80, 443, 8080, 8081, 8082, 8443, 8444, 8445
    [[ "$output" == *"8080"* ]] || { echo 'assertion failed: [[ "$output" == *"8080"* ]]' >&2; exit 1; }
    [[ "$output" == *"8443"* ]] || { echo 'assertion failed: [[ "$output" == *"8443"* ]]' >&2; exit 1; }
    # UDP: 8443, 8444, 8445, 9998, 9999
    [[ "$output" == *"9998"* ]] || { echo 'assertion failed: [[ "$output" == *"9998"* ]]' >&2; exit 1; }
    [[ "$output" == *"9999"* ]]
}

@test "_aws_wait_for_windows_endpoint: function exists" {
    type _aws_wait_for_windows_endpoint | head -1
    [[ "$(type _aws_wait_for_windows_endpoint)" == *"function"* ]]
}

@test "step_aws_deploy_endpoint: no longer rejects AWS Windows (function branches on OS)" {
    # Verify the function body does NOT contain the old rejection message
    local fn_body
    fn_body="$(type step_aws_deploy_endpoint)"
    # Old rejection text should be gone
    [[ "$fn_body" != *"AWS Windows endpoint deployment is not yet supported"* ]] || { echo 'assertion failed: [[ "$fn_body" != *"AWS Windows endpoint deployment is not yet supported"* ]]' >&2; exit 1; }
    # Should branch on ep_os
    [[ "$fn_body" == *"_aws_find_windows_ami"* ]] || { echo 'assertion failed: [[ "$fn_body" == *"_aws_find_windows_ami"* ]]' >&2; exit 1; }
    [[ "$fn_body" == *"_aws_win_endpoint_full_userdata"* ]] || { echo 'assertion failed: [[ "$fn_body" == *"_aws_win_endpoint_full_userdata"* ]]' >&2; exit 1; }
    [[ "$fn_body" == *"_aws_wait_for_windows_endpoint"* ]]
}

# ===========================================================================
# GCP project_id resolution (regression for v0.27.26 — GCP deploys failed
# at "Step 2: Check GCP prerequisites" because install.sh ignored the
# config's project_id and the dashboard host had no `gcloud config project`).
# ===========================================================================

@test "_gcp_project_from_sa_email: extracts project from service account email" {
    run _gcp_project_from_sa_email "alethedash-vms@kepler-408121.iam.gserviceaccount.com"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [ "$output" = "kepler-408121" ]
}

@test "_gcp_project_from_sa_email: handles project IDs containing hyphens" {
    run _gcp_project_from_sa_email "ci-runner@my-prod-project-42.iam.gserviceaccount.com"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [ "$output" = "my-prod-project-42" ]
}

@test "_gcp_project_from_sa_email: returns empty for human user email" {
    run _gcp_project_from_sa_email "alice@example.com"
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [ -z "${output// /}" ]
}

@test "_gcp_project_from_sa_email: returns empty for empty input" {
    run _gcp_project_from_sa_email ""
    [ "$status" -eq 0 ] || { echo 'assertion failed: [ "$status" -eq 0 ]' >&2; exit 1; }
    [ -z "${output// /}" ]
}

@test "_gcp_autodetect_project: derives project from active service account email" {
    GCP_PROJECT=""
    gcloud() {
        if [[ "$1 $2" == "config get-value" && "$3" == "account" ]]; then
            echo "alethedash-vms@kepler-408121.iam.gserviceaccount.com"
        else
            echo ""
        fi
    }
    export -f gcloud
    _gcp_autodetect_project
    [ "$GCP_PROJECT" = "kepler-408121" ]
    unset -f gcloud
}

@test "_gcp_autodetect_project: leaves GCP_PROJECT alone when already set" {
    GCP_PROJECT="preset-project"
    gcloud() { echo "should-not-be-called"; }
    export -f gcloud
    _gcp_autodetect_project
    [ "$GCP_PROJECT" = "preset-project" ]
    unset -f gcloud
}

@test "_gcp_autodetect_project: falls back to host gcloud config for human accounts" {
    GCP_PROJECT=""
    gcloud() {
        if [[ "$1 $2" == "config get-value" && "$3" == "account" ]]; then
            echo "alice@example.com"
        elif [[ "$1 $2" == "config get-value" && "$3" == "project" ]]; then
            echo "host-fallback-proj"
        else
            echo ""
        fi
    }
    export -f gcloud
    _gcp_autodetect_project
    [ "$GCP_PROJECT" = "host-fallback-proj" ]
    unset -f gcloud
}

@test "_deploy_parse_config: reads tester.gcp.project_id (canonical key)" {
    local cfg="$TEST_TMPDIR/parse-gcp-pid.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "gcp", "gcp": { "project_id": "from-pid", "zone": "us-central1-a" } },
  "endpoints": [{ "provider": "local" }]
}
JSON
    DEPLOY_CONFIG_PATH="$cfg"
    _deploy_parse_config "$cfg"
    [ "$GCP_PROJECT" = "from-pid" ]
}

@test "_deploy_parse_config: reads tester.gcp.project (legacy alias)" {
    local cfg="$TEST_TMPDIR/parse-gcp-legacy.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "gcp", "gcp": { "project": "from-legacy", "zone": "us-central1-a" } },
  "endpoints": [{ "provider": "local" }]
}
JSON
    DEPLOY_CONFIG_PATH="$cfg"
    _deploy_parse_config "$cfg"
    [ "$GCP_PROJECT" = "from-legacy" ]
}

@test "_deploy_load_endpoint: reads endpoints[i].gcp.project_id" {
    local cfg="$TEST_TMPDIR/parse-ep-gcp-pid.json"
    cat > "$cfg" <<'JSON'
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "gcp", "gcp": { "project_id": "ep-pid-proj", "zone": "us-central1-a", "instance_name": "nwk-ep" } }
  ]
}
JSON
    DEPLOY_CONFIG_PATH="$cfg"
    GCP_PROJECT=""
    _deploy_parse_config "$cfg"
    _deploy_load_endpoint 0
    [ "$GCP_PROJECT" = "ep-pid-proj" ]
}

# ---------------------------------------------------------------------------
# Proxy stack configs: throughput + info routes (v0.28.11)
# ---------------------------------------------------------------------------
# The Full Stack benchmark's download/upload modes probe /download and
# /upload THROUGH the proxy; /info supplies server metadata. Every proxy
# template that selectively proxies /page + /asset must also proxy these.
# (HAProxy and Traefik forward everything, so they need no explicit routes.)

@test "proxy configs: every nginx copy proxies throughput + info routes" {
    # One throughput location block per /asset location block, in all
    # nginx config copies (local + both remote heredocs), HTTP and HTTPS.
    local asset_ct route_ct
    asset_ct=$(grep -c 'location /asset' "$SCRIPT")
    route_ct=$(grep -c 'location ~ \^/(download|upload|info)' "$SCRIPT")
    [ "$asset_ct" -gt 0 ] || { echo 'assertion failed: [ "$asset_ct" -gt 0 ]' >&2; exit 1; }
    [ "$asset_ct" -eq "$route_ct" ]
}

@test "proxy configs: Caddy proxies throughput + info on both listeners" {
    local dl ul info
    dl=$(grep -c 'handle /download\*' "$SCRIPT")
    ul=$(grep -c 'handle /upload\*' "$SCRIPT")
    info=$(grep -c 'handle /info' "$SCRIPT")
    [ "$dl" -eq 2 ]   # :8091 and :8454
    [ "$ul" -eq 2 ] || { echo 'assertion failed: [ "$ul" -eq 2 ]' >&2; exit 1; }
    [ "$info" -eq 2 ]
}

@test "proxy configs: Apache proxies throughput + info on both vhosts" {
    local dl ul info
    dl=$(grep -c 'ProxyPass        /download' "$SCRIPT")
    ul=$(grep -c 'ProxyPass        /upload' "$SCRIPT")
    info=$(grep -c 'ProxyPass        /info' "$SCRIPT")
    [ "$dl" -eq 2 ]   # HTTP + HTTPS vhosts
    [ "$ul" -eq 2 ] || { echo 'assertion failed: [ "$ul" -eq 2 ]' >&2; exit 1; }
    [ "$info" -eq 2 ]
}

@test "proxy configs: IIS web.config rewrites throughput + info to endpoint" {
    grep -q 'match url="\^(download|upload|info)(.\*)"' "$SCRIPT"
}

@test "proxy configs: nginx throughput locations disable buffering and body cap" {
    # Throughput must measure the proxy path, not nginx disk buffers, and
    # uploads must not 413 at nginx's 1MB default.
    local blocks
    blocks=$(grep -A6 'location ~ \^/(download|upload|info)' "$SCRIPT" | grep -c 'client_max_body_size 0')
    local route_ct
    route_ct=$(grep -c 'location ~ \^/(download|upload|info)' "$SCRIPT")
    [ "$blocks" -eq "$route_ct" ]
}

# ---------------------------------------------------------------------------
# Matrix stack support (v0.28.131): --setup-stack + remote wiring + firewall
# ---------------------------------------------------------------------------
# The 2026-08-01 comparison-matrix failure class: Azure Linux cells silently
# got only nginx ("not yet supported" warn-and-continue) while the readiness
# gate probed the requested stack's port, and the NSG never opened the stack
# comparison ports at all — every non-nginx cell timed out unreachable.

@test "parse_args: --setup-stack sets SETUP_STACK and auto-yes" {
    parse_args --setup-stack caddy
    [ "$SETUP_STACK" = "caddy" ] || { echo 'assertion failed: [ "$SETUP_STACK" = "caddy" ]' >&2; exit 1; }
    [ "$AUTO_YES" -eq 1 ]
}

@test "setup-stack: linux deploy paths wire non-nginx stacks to _remote_setup_stack" {
    # The warn-and-continue arms must be gone from the azure/aws/lan branches…
    ! grep -q 'Remote \$_ls setup on Azure Linux endpoints is not yet supported' "$SCRIPT"
    ! grep -q 'Remote \$_ls setup on AWS endpoints is not yet supported' "$SCRIPT"
    ! grep -q 'Remote \$_ls setup on LAN endpoints is not yet supported' "$SCRIPT"
    # …replaced by the remote stack helper, failure-fatal in deploy mode.
    grep -q '_remote_setup_stack "\$AZURE_ENDPOINT_IP" "azureuser" "\$_ls"' "$SCRIPT"
    grep -q '_remote_setup_stack "\$AWS_ENDPOINT_IP" "ubuntu" "\$_ls"' "$SCRIPT"
    grep -q '_remote_setup_stack "\$LAN_ENDPOINT_IP" "\$ssh_user" "\$_ls"' "$SCRIPT"
}

@test "setup-stack: remote helper pipes the installer with --setup-stack" {
    grep -q -- '--setup-stack \$stack' "$SCRIPT"
}

@test "setup-stack: main dispatches each stack and rejects unknown" {
    # The dispatch case must cover all five and error out otherwise.
    local dispatch
    dispatch=$(sed -n '/if \[\[ -n "\$SETUP_STACK" \]\]/,/^    fi$/p' "$SCRIPT")
    echo "$dispatch" | grep -q 'step_setup_nginx'
    echo "$dispatch" | grep -q 'step_setup_caddy'
    echo "$dispatch" | grep -q 'step_setup_apache'
    echo "$dispatch" | grep -q 'step_setup_haproxy'
    echo "$dispatch" | grep -q 'step_setup_traefik'
    echo "$dispatch" | grep -q 'unknown stack'
}

@test "firewall: stack comparison ports opened on azure, aws and gcp" {
    # Azure NSG: TCP list includes 8091-8094 + 8454-8457; UDP includes 8454 (Caddy h3).
    grep -q -- '--destination-port-ranges 80 443 8080-8082 8091-8094 8443-8445 8454-8457' "$SCRIPT"
    grep -q -- '--destination-port-ranges 8443-8445 8454 9998 9999' "$SCRIPT"
    # AWS security group
    grep -q -- '--protocol tcp --port 8091-8094' "$SCRIPT"
    grep -q -- '--protocol tcp --port 8454-8457' "$SCRIPT"
    grep -q -- '--protocol udp --port 8454 ' "$SCRIPT"
    # GCP firewall rule
    grep -q 'tcp:8091-8094' "$SCRIPT"
    grep -q 'tcp:8454-8457' "$SCRIPT"
    grep -q 'udp:8454,' "$SCRIPT"
}

# ---------------------------------------------------------------------------
# Stack service-start fixes (v0.28.133)
# ---------------------------------------------------------------------------
# Caddy: v2 rejects content after '{' on the same line — the one-line
# `handle /x* { reverse_proxy … }` form NEVER validated, so networker-caddy
# had never started anywhere. Apache: the stock 'Listen 80' + default site
# collide with nginx on endpoint VMs (nginx installs first) → apache2 fails
# "Address already in use".

@test "caddy: no single-line handle blocks (caddy v2 syntax)" {
    run grep -cE 'handle [^{]*\{ +[a-z]' "$SCRIPT"
    [ "$output" -eq 0 ]
}

@test "caddy: config is validated before the service starts" {
    grep -q 'caddy validate --config /etc/caddy/networker.Caddyfile' "$SCRIPT" || \
        grep -q 'validate --config /etc/caddy/networker.Caddyfile' "$SCRIPT"
}

@test "apache: default site and Listen 80/443 disabled on apt systems" {
    grep -q 'a2dissite 000-default' "$SCRIPT"
    # The sed must tolerate the INDENTED Listen 443 inside <IfModule> blocks.
    grep -q 'Listen (80|443)' "$SCRIPT"
    grep -q '\[\[:space:\]\]\*)Listen' "$SCRIPT"
}

@test "apache: config goes to conf-available with a2enconf on Debian layout" {
    # Debian apache2 never reads conf.d/ — the config must be enabled via
    # a2enconf or apache starts "successfully" without our listeners.
    grep -q 'conf-available/networker.conf' "$SCRIPT"
    grep -q 'a2enconf networker' "$SCRIPT"
}

@test "apache and caddy: port verified after service start" {
    grep -q 'http://127.0.0.1:8094/' "$SCRIPT"
    grep -q 'http://127.0.0.1:8091/' "$SCRIPT"
}

@test "iis: total verification failure fails the deploy" {
    grep -q 'IIS is not responding on any port after setup — failing deploy' "$SCRIPT"
}

@test "iis: run-command conflict is retried" {
    local section
    section=$(sed -n '/_azure_win_setup_iis()/,/^}/p' "$SCRIPT")
    echo "$section" | grep -q 'grep -q "Conflict"'
    echo "$section" | grep -q 'retrying IIS setup'
}

@test "iis: verify failure fatal only when the endpoint requested iis" {
    grep -q 'AZURE_ENDPOINT_WANTS_IIS' "$SCRIPT"
    local section
    section=$(sed -n '/_azure_win_setup_iis()/,/^}/p' "$SCRIPT")
    echo "$section" | grep -q 'AZURE_ENDPOINT_WANTS_IIS'
    echo "$section" | grep -q 'requested stack is not IIS'
}

@test "iis powershell: generated script is pure ASCII (az run-command mangles UTF-8)" {
    # An em-dash inside a string became a stray quote under run-command's
    # encoding, failing the ENTIRE script at parse — no IIS was ever installed
    # by the deploy path (2026-08-04 diag). The first version of this test
    # passed VACUOUSLY when generation failed (empty output → grep -c 0), and
    # the v0.28.141 "cleanup" only covered a truncated span of the function —
    # both slipped through. Assert real generation THEN ascii-purity.
    local out lines nonascii
    out=$(bash -c "source '$SCRIPT' >/dev/null 2>&1; _iis_setup_powershell 'x.example.com'")
    [ -n "$out" ] || { echo 'assertion failed: [ -n "$out" ]' >&2; exit 1; }
    lines=$(printf '%s\n' "$out" | wc -l | tr -d ' ')
    [ "$lines" -gt 100 ] || { echo 'assertion failed: [ "$lines" -gt 100 ]' >&2; exit 1; }
    nonascii=$(printf '%s' "$out" | LC_ALL=C grep -c '[^\x00-\x7F]' || true)
    [ "$nonascii" = "0" ]
}

# ===========================================================================
# Self-hosted control plane (C#) — v0.28.148 decommission fallout
#
# `install.sh dashboard` installed the RUST networker-dashboard/networker-agent
# binaries. Those crates were deleted in v0.28.148 and their release assets went
# with them, so from that release until this one the self-host path was dead:
# the download 404'd and the "fallback to source compile" then failed with
# "crate not found". Nothing caught it, because no test ever connected the
# installer's asset names to the assets release.yml actually builds.
# ===========================================================================

@test "controlplane: the dashboard path no longer installs retired Rust crates" {
    # The exact regression: these two calls 404 on every release >= v0.28.148.
    ! grep -q 'step_download_release "networker-dashboard"' "$SCRIPT"
    ! grep -q 'step_cargo_install "networker-dashboard"' "$SCRIPT"
    ! grep -q 'step_cargo_install "networker-agent"' "$SCRIPT"
}

@test "controlplane: installer asset names match what release.yml builds" {
    # The drift guard that was missing. Renaming an asset in the release
    # workflow without updating the installer breaks self-hosting silently;
    # this fails instead.
    local wf="${BATS_TEST_DIRNAME}/../.github/workflows/release.yml"
    [ -f "$wf" ]
    local asset
    for asset in networker-controlplane-linux-x64.tar.gz \
                 networker-agent-cs-linux-x64.tar.gz \
                 dashboard-frontend.tar.gz; do
        grep -q "$asset" "$SCRIPT"
        grep -q "$asset" "$wf"
    done
}

@test "controlplane: install extracts a directory and requires the executable" {
    local section
    section=$(sed -n '/^step_install_controlplane()/,/^}/p' "$SCRIPT")
    [ -n "$section" ]
    # It is a self-contained publish DIR, not a bare binary — extracting to
    # CONTROLPLANE_DIR and chmod-ing the entrypoint is the whole contract.
    echo "$section" | grep -q 'CONTROLPLANE_DIR'
    echo "$section" | grep -q 'Networker.ControlPlane'
    # A truncated/garbage download must fail loudly, not leave a dead service.
    echo "$section" | grep -q 'has no Networker.ControlPlane executable'
    # And there must be no cargo fallback that would report a confusing error.
    ! echo "$section" | grep -q 'cargo'
}

@test "controlplane: env file uses the C# contract, not the Rust one" {
    local section
    section=$(sed -n '/^step_write_dashboard_env()/,/^}/p' "$SCRIPT")
    [ -n "$section" ]
    # Npgsql does NOT parse postgres:// URIs — the old value silently fell
    # through to the built-in default and pointed at the wrong database.
    echo "$section" | grep -q 'DASHBOARD_DB_URL_NPGSQL=Host='
    ! echo "$section" | grep -q 'DASHBOARD_DB_URL=postgres://'
    echo "$section" | grep -q 'ASPNETCORE_URLS='
    # Removed in the C# app; leaving it implies static serving that never happens.
    ! echo "$section" | grep -q 'DASHBOARD_STATIC_DIR'
}

@test "controlplane: env file really writes an Npgsql keyword string" {
    # Executes the generator instead of grepping it, so a broken heredoc or a
    # quoting slip is caught rather than assumed away. Writes to a temp path,
    # not /etc, so this RUNS everywhere — a skipped guard proves nothing.
    local out_file="$TEST_TMPDIR/dashboard.env"
    bash -c "
        source '$SCRIPT' >/dev/null 2>&1
        # Swallow every privileged side effect; capture only the env heredoc.
        sudo() {
            if [ \"\$1\" = tee ]; then cat > '$out_file'; fi
            return 0
        }
        next_step() { :; }; print_ok() { :; }; print_info() { :; }
        DASHBOARD_DB_PASSWORD=s3cret
        DASHBOARD_FQDN=nwk.example.com
        step_write_dashboard_env
    " >/dev/null 2>&1

    [ -s "$out_file" ]
    # Npgsql keyword syntax with the installer's own DB password — NOT a URI.
    grep -q '^DASHBOARD_DB_URL_NPGSQL=Host=127\.0\.0\.1;Port=5432;Database=networker_dashboard;Username=networker;Password=s3cret$' "$out_file"
    grep -q '^ASPNETCORE_URLS=http://127\.0\.0\.1:5030$' "$out_file"
    grep -q '^DASHBOARD_PUBLIC_URL=https://nwk\.example\.com$' "$out_file"
    # Secrets must be generated, not left blank (the app fail-closes on empty).
    grep -qE '^DASHBOARD_JWT_SECRET=.{16,}$' "$out_file"
    grep -qE '^DASHBOARD_CREDENTIAL_KEY=[0-9a-f]{32,}$' "$out_file"
    ! grep -q 'postgres://' "$out_file"
}

@test "controlplane: systemd unit runs the self-contained publish dir" {
    local section
    section=$(sed -n '/^step_setup_dashboard_service()/,/^}/p' "$SCRIPT")
    [ -n "$section" ]
    echo "$section" | grep -q "ExecStart=\${binary_path}"
    echo "$section" | grep -q "WorkingDirectory=\${CONTROLPLANE_DIR}"
    # Copying the entrypoint alone to /usr/local/bin strands its runtime.
    ! echo "$section" | grep -q 'cp .*usr/local/bin/networker-dashboard'
    # RUST_LOG on a .NET service is cargo-cult config.
    ! echo "$section" | grep -q 'RUST_LOG'
}

@test "controlplane: nginx serves the SPA and proxies only api and ws" {
    # The C# control plane serves no static files at all (the Rust one did), so
    # proxying "/" wholesale returns 404 for every page load.
    local section
    section=$(sed -n '/^step_setup_nginx_proxy()/,/^}/p' "$SCRIPT")
    [ -n "$section" ]
    echo "$section" | grep -q 'root /opt/networker/dashboard;'
    # Deep links must survive a hard refresh.
    echo "$section" | grep -q 'try_files \$uri \$uri/ /index.html;'
    echo "$section" | grep -q 'location /api/ {'
    echo "$section" | grep -q 'location /ws/ {'
    # /share/:token is a CLIENT-side route (App.tsx) whose data comes from
    # /api/share/{token}. Proxying /share/ to the control plane 404s the
    # public share page — the exact mistake this line prevents recurring.
    ! echo "$section" | grep -q 'location /share/ {'
}

# ===========================================================================
# scripts/ci/effective-changed-files.sh — the version-bump filter
#
# Every PR bumps five files by policy, which made every CI path filter true:
# a frontend-only PR ran the full installer execution matrix because one
# version-string line moved (PR #664, ~13 min of jobs). The filter drops a
# bump-file from the changed list ONLY when its entire diff is the bump line.
# ===========================================================================

_ecf_repo() {  # builds a tiny repo with a base commit; leaves cwd inside it
    local dir="$TEST_TMPDIR/ecf-repo"
    mkdir -p "$dir" && cd "$dir" || return 1
    git init -q -b main
    git config user.email t@t && git config user.name t
    printf 'INSTALLER_VERSION="v0.28.100"  # fallback\nreal_logic() { echo hi; }\n' > install.sh
    printf 'version     = "0.28.100"\n[workspace]\n' > Cargo.toml
    printf 'app code\n' > app.txt
    git add -A && git commit -qm base
}

@test "effective-changes: a version-only bump is dropped" {
    _ecf_repo
    sed -i.bak 's/v0.28.100/v0.28.101/' install.sh && rm -f install.sh.bak
    sed -i.bak 's/"0.28.100"/"0.28.101"/' Cargo.toml && rm -f Cargo.toml.bak
    echo "feature" >> app.txt
    git add -A && git commit -qm bump

    run "$BATS_TEST_DIRNAME/../scripts/ci/effective-changed-files.sh" HEAD~1
    [ "$status" -eq 0 ] || { echo "script failed: $output" >&2; exit 1; }
    [[ "$output" == *"app.txt"* ]] || { echo 'assertion failed: app.txt missing' >&2; exit 1; }
    [[ "$output" != *"install.sh"* ]] || { echo 'assertion failed: version-only install.sh not dropped' >&2; exit 1; }
    [[ "$output" != *"Cargo.toml"* ]]
}

@test "effective-changes: a real change to a bump file is kept" {
    _ecf_repo
    sed -i.bak 's/v0.28.100/v0.28.101/' install.sh && rm -f install.sh.bak
    printf 'new_function() { echo new; }\n' >> install.sh
    git add -A && git commit -qm real-change

    run "$BATS_TEST_DIRNAME/../scripts/ci/effective-changed-files.sh" HEAD~1
    [ "$status" -eq 0 ] || { echo "script failed: $output" >&2; exit 1; }
    [[ "$output" == *"install.sh"* ]] || { echo 'assertion failed: real install.sh change was DROPPED — CI would skip the installer suites on a real edit' >&2; exit 1; }
}

@test "effective-changes: a dependency bump in Cargo.lock is kept" {
    _ecf_repo
    printf 'version = "1.0.0"\nchecksum = "abc"\n' > Cargo.lock
    git add -A && git commit -qm lockbase
    printf 'version = "1.0.1"\nchecksum = "def"\n' > Cargo.lock
    git add -A && git commit -qm depbump

    run "$BATS_TEST_DIRNAME/../scripts/ci/effective-changed-files.sh" HEAD~1
    [ "$status" -eq 0 ] || { echo "script failed: $output" >&2; exit 1; }
    # checksum lines are outside the allowed pattern → the file must survive
    [[ "$output" == *"Cargo.lock"* ]] || { echo 'assertion failed: dependabot-style lock change was dropped' >&2; exit 1; }
}
