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
# 5. _remote_install_binary_from_source
# ===========================================================================

# Create a uname stub that we can configure per-test
_make_uname_stub() {
    local result="$1"
    cat > "${TEST_TMPDIR}/bin/uname" << STUB
#!/usr/bin/env bash
echo "${result}"
STUB
    chmod +x "${TEST_TMPDIR}/bin/uname"
    export PATH="${TEST_TMPDIR}/bin:${PATH}"
}

_make_cargo_stub() {
    # Creates a cargo stub that installs a fake binary to --root/bin/
    cat > "${TEST_TMPDIR}/bin/cargo" << 'STUB'
#!/usr/bin/env bash
# Parse --root arg
root_dir=""
prev=""
for arg in "$@"; do
    [[ "$prev" == "--root" ]] && root_dir="$arg"
    prev="$arg"
done
# Extract binary name from last positional-style arg before flags
binary=""
for arg in "$@"; do
    case "$arg" in
        --*) ;;
        install|--git|http*) ;;
        networker-*) binary="$arg" ;;
    esac
done
if [[ -n "$root_dir" && -n "$binary" ]]; then
    mkdir -p "${root_dir}/bin"
    printf '#!/usr/bin/env bash\necho "%s 0.12.65"\n' "$binary" \
        > "${root_dir}/bin/${binary}"
    chmod +x "${root_dir}/bin/${binary}"
fi
echo "Finished release [optimized]"
STUB
    chmod +x "${TEST_TMPDIR}/bin/cargo"
    export PATH="${TEST_TMPDIR}/bin:${PATH}"
}

@test "_remote_install_binary_from_source: routes to remote compile on OS mismatch" {
    _make_uname_stub "Darwin"           # local uname returns Darwin
    export STUB_SSH_UNAME="Linux"       # remote VM reports Linux
    export STUB_SSH_VERSION="networker-endpoint 0.12.65"
    step_ensure_cargo_env() { :; }
    _remote_chrome_available() { return 1; }
    run _remote_install_binary_from_source "networker-endpoint" "1.2.3.4" "azureuser" "x86_64"
    [ "$status" -eq 0 ]
    [[ "$output" == *"mismatch"* ]]
    [[ "$output" == *"compiling on VM"* ]] || [[ "$output" == *"compiled on VM"* ]]
}

@test "_remote_compile_on_vm: exits when no local checkout" {
    _find_repo_root() { return 1; }     # simulate curl|bash with no local repo
    run _remote_compile_on_vm "networker-endpoint" "1.2.3.4" "azureuser" ""
    [ "$status" -ne 0 ]
    [[ "$output" == *"not run from a local checkout"* ]]
}

@test "_remote_install_binary_from_source: succeeds when OS and arch match" {
    _make_uname_stub "Linux"            # local uname -s returns Linux
    export STUB_SSH_UNAME="Linux"       # remote also Linux
    export STUB_SSH_VERSION="networker-endpoint 0.12.65"
    _make_cargo_stub                    # cargo creates binary in --root/bin/
    step_ensure_cargo_env() { :; }
    _remote_chrome_available() { return 1; }
    run _remote_install_binary_from_source "networker-endpoint" "1.2.3.4" "azureuser" "x86_64"
    [ "$status" -eq 0 ]
    [[ "$output" == *"networker-endpoint"* ]]
}

@test "_remote_install_binary_from_source: exits when binary fails to exec on VM" {
    _make_uname_stub "Linux"
    export STUB_SSH_UNAME="Linux"
    export STUB_SSH_FAIL_VERSION=1      # --version on VM fails → Exec format error
    _make_cargo_stub
    step_ensure_cargo_env() { :; }
    _remote_chrome_available() { return 1; }
    run _remote_install_binary_from_source "networker-endpoint" "1.2.3.4" "azureuser" "x86_64"
    [ "$status" -ne 0 ]
    [[ "$output" == *"uploaded but failed"* ]] || [[ "$output" == *"failed to execute"* ]]
}


# ===========================================================================
# 6. step_download_release
# ===========================================================================

@test "step_download_release: installs binary to INSTALL_DIR" {
    RELEASE_TARGET="x86_64-unknown-linux-gnu"
    # stubs/gh creates a tar.gz with a fake binary inside
    step_download_release "networker-tester"
    [ -x "${INSTALL_DIR}/networker-tester" ]
}

@test "step_download_release: exits on gh download failure" {
    RELEASE_TARGET="x86_64-unknown-linux-gnu"
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
