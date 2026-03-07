#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Comprehensive test suite for install.sh
#
# Self-contained bash test script that:
#   - Sources install.sh functions (with main() overridden)
#   - Mocks ALL external commands
#   - Tests every code path
#
# Usage:
#   bash tests/test_install_sh.sh
# ──────────────────────────────────────────────────────────────────────────────
set -o pipefail
# NOTE: We intentionally do NOT use set -e or set -u here.
# set -e would cause the test harness to exit on the first assertion failure.
# set -u would fail on unset associative array keys used in mock tracking.

# ── Find the repo root and install.sh ────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
INSTALL_SH="${REPO_ROOT}/install.sh"

if [[ ! -f "$INSTALL_SH" ]]; then
    echo "FATAL: install.sh not found at ${INSTALL_SH}"
    exit 1
fi

# ── Test framework ───────────────────────────────────────────────────────────
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_RUN=0
CURRENT_TEST=""

RED=$'\033[0;31m'
GREEN=$'\033[0;32m'
YELLOW=$'\033[1;33m'
CYAN=$'\033[0;36m'
BOLD=$'\033[1m'
DIM=$'\033[2m'
RESET=$'\033[0m'

_test_start() {
    CURRENT_TEST="$1"
    TESTS_RUN=$((TESTS_RUN + 1))
}

_test_pass() {
    TESTS_PASSED=$((TESTS_PASSED + 1))
    printf "${GREEN}  PASS${RESET} %s\n" "$CURRENT_TEST"
}

_test_fail() {
    TESTS_FAILED=$((TESTS_FAILED + 1))
    printf "${RED}  FAIL${RESET} %s\n" "$CURRENT_TEST"
    if [[ $# -gt 0 ]]; then
        printf "${RED}       %s${RESET}\n" "$*"
    fi
}

assert_eq() {
    local expected="$1" actual="$2" msg="${3:-}"
    if [[ "$expected" == "$actual" ]]; then
        return 0
    fi
    _test_fail "${msg:+$msg: }expected '$expected', got '$actual'"
    return 1
}

assert_neq() {
    local unexpected="$1" actual="$2" msg="${3:-}"
    if [[ "$unexpected" != "$actual" ]]; then
        return 0
    fi
    _test_fail "${msg:+$msg: }expected NOT '$unexpected', but got it"
    return 1
}

assert_contains() {
    local haystack="$1" needle="$2" msg="${3:-}"
    if [[ "$haystack" == *"$needle"* ]]; then
        return 0
    fi
    _test_fail "${msg:+$msg: }'$haystack' does not contain '$needle'"
    return 1
}

assert_not_contains() {
    local haystack="$1" needle="$2" msg="${3:-}"
    if [[ "$haystack" != *"$needle"* ]]; then
        return 0
    fi
    _test_fail "${msg:+$msg: }'$haystack' should not contain '$needle'"
    return 1
}

assert_exit_code() {
    local expected="$1" actual="$2" msg="${3:-}"
    if [[ "$expected" == "$actual" ]]; then
        return 0
    fi
    _test_fail "${msg:+$msg: }expected exit code $expected, got $actual"
    return 1
}

# ── Temporary directory management ───────────────────────────────────────────
TEST_TMPDIR=""
_setup_tmpdir() {
    TEST_TMPDIR="$(mktemp -d /tmp/networker-test-XXXXXX)"
}

_teardown_tmpdir() {
    [[ -n "$TEST_TMPDIR" && -d "$TEST_TMPDIR" ]] && rm -rf "$TEST_TMPDIR"
    TEST_TMPDIR=""
}

# ── Source install.sh with main() overridden ─────────────────────────────────
# We override main() so sourcing the file doesn't run the installer.
# We also override the color detection (force them on for test assertions).
_source_install_sh() {
    # Override main to do nothing
    main() { :; }

    # Source install.sh -- it checks BASH_SOURCE vs $0 at the end,
    # but since we source it, main() won't run.
    # install.sh sets "set -euo pipefail" at the top, so we need to
    # undo that after sourcing.
    source "$INSTALL_SH"

    # Undo strict modes that install.sh sets -- we handle errors ourselves
    set +euo pipefail

    # Force color variables so tests can run without a TTY
    BOLD=$'\033[1m'
    DIM=$'\033[2m'
    GREEN=$'\033[0;32m'
    YELLOW=$'\033[1;33m'
    RED=$'\033[0;31m'
    CYAN=$'\033[0;36m'
    RESET=$'\033[0m'

    # Reset state to defaults after sourcing
    _reset_install_state
}

# Reset all global state variables to their defaults
_reset_install_state() {
    COMPONENT=""
    AUTO_YES=0
    FROM_SOURCE=0
    SKIP_RUST=0
    INSTALL_METHOD="source"
    RELEASE_AVAILABLE=0
    RELEASE_TARGET=""
    NETWORKER_VERSION=""
    INSTALLER_VERSION="v0.12.93"
    DO_RUST_INSTALL=0
    DO_INSTALL_TESTER=1
    DO_INSTALL_ENDPOINT=1
    RUST_VER=""
    RUST_EXISTS=0
    GIT_AVAILABLE=0
    PKG_MGR=""
    DO_GIT_INSTALL=0
    CHROME_AVAILABLE=0
    CHROME_PATH=""
    DO_CHROME_INSTALL=0
    CERTUTIL_AVAILABLE=0
    SYS_OS=""
    SYS_ARCH=""
    SYS_SHELL=""
    STEP_NUM=0
    TESTER_LOCATION="local"
    ENDPOINT_LOCATION="local"
    DO_REMOTE_TESTER=0
    DO_REMOTE_ENDPOINT=0
    AZURE_CLI_AVAILABLE=0
    AZURE_LOGGED_IN=0
    AZURE_REGION="eastus"
    AZURE_REGION_ASKED=0
    AZURE_TESTER_RG="networker-rg-tester"
    AZURE_TESTER_VM="networker-tester-vm"
    AZURE_TESTER_SIZE="Standard_B2s"
    AZURE_TESTER_OS="linux"
    AZURE_TESTER_IP=""
    AZURE_ENDPOINT_RG="networker-rg-endpoint"
    AZURE_ENDPOINT_VM="networker-endpoint-vm"
    AZURE_ENDPOINT_SIZE="Standard_B2s"
    AZURE_ENDPOINT_OS="linux"
    AZURE_ENDPOINT_IP=""
    AZURE_AUTO_SHUTDOWN="yes"
    AZURE_SHUTDOWN_ASKED=0
    AZURE_EXTRA_ENDPOINT_IPS=()
    AWS_CLI_AVAILABLE=0
    AWS_LOGGED_IN=0
    AWS_REGION="us-east-1"
    AWS_REGION_ASKED=0
    AWS_TESTER_NAME="networker-tester"
    AWS_TESTER_INSTANCE_TYPE="t3.small"
    AWS_TESTER_OS="linux"
    AWS_TESTER_INSTANCE_ID=""
    AWS_TESTER_IP=""
    AWS_ENDPOINT_NAME="networker-endpoint"
    AWS_ENDPOINT_INSTANCE_TYPE="t3.small"
    AWS_ENDPOINT_OS="linux"
    AWS_ENDPOINT_INSTANCE_ID=""
    AWS_ENDPOINT_IP=""
    AWS_AUTO_SHUTDOWN="yes"
    AWS_SHUTDOWN_ASKED=0
    GCP_CLI_AVAILABLE=0
    GCP_LOGGED_IN=0
    GCP_PROJECT=""
    GCP_REGION="us-central1"
    GCP_ZONE="us-central1-a"
    GCP_REGION_ASKED=0
    GCP_TESTER_NAME="networker-tester"
    GCP_TESTER_MACHINE_TYPE="e2-small"
    GCP_TESTER_OS="linux"
    GCP_TESTER_IP=""
    GCP_ENDPOINT_NAME="networker-endpoint"
    GCP_ENDPOINT_MACHINE_TYPE="e2-small"
    GCP_ENDPOINT_OS="linux"
    GCP_ENDPOINT_IP=""
    GCP_AUTO_SHUTDOWN="yes"
    GCP_SHUTDOWN_ASKED=0
    CONFIG_FILE_PATH=""
    REPO_HTTPS="https://github.com/irlm/networker-tester"
    REPO_GH="irlm/networker-tester"
    INSTALL_DIR="${HOME}/.cargo/bin"
}

# ── Mock command infrastructure ──────────────────────────────────────────────
# Track calls to mocked commands
declare -A MOCK_CALL_ARGS
declare -A MOCK_CALL_COUNT
declare -A MOCK_RETURN_CODE
declare -A MOCK_STDOUT

_mock_reset() {
    MOCK_CALL_ARGS=()
    MOCK_CALL_COUNT=()
    MOCK_RETURN_CODE=()
    MOCK_STDOUT=()
}

_mock_set_output() {
    local cmd="$1" output="$2"
    MOCK_STDOUT["$cmd"]="$output"
}

_mock_set_rc() {
    local cmd="$1" rc="$2"
    MOCK_RETURN_CODE["$cmd"]="$rc"
}

_mock_get_call_count() {
    local cmd="$1"
    echo "${MOCK_CALL_COUNT[$cmd]:-0}"
}

_mock_get_args() {
    local cmd="$1"
    echo "${MOCK_CALL_ARGS[$cmd]:-}"
}

# Define mock functions for external commands
_setup_mocks() {
    _mock_reset

    # Mock uname to return configurable values
    MOCK_UNAME_S="Linux"
    MOCK_UNAME_M="x86_64"
    uname() {
        local flag="${1:-}"
        case "$flag" in
            -s) echo "$MOCK_UNAME_S" ;;
            -m) echo "$MOCK_UNAME_M" ;;
            *)  echo "$MOCK_UNAME_S" ;;
        esac
        MOCK_CALL_COUNT["uname"]=$(( ${MOCK_CALL_COUNT["uname"]:-0} + 1 ))
        MOCK_CALL_ARGS["uname"]="${MOCK_CALL_ARGS["uname"]:-} $*"
    }

    # Mock command -v to control which commands appear available
    declare -gA MOCK_COMMANDS_AVAILABLE
    MOCK_COMMANDS_AVAILABLE=()

    # We cannot override 'command' as a function easily, so we override
    # the individual external commands instead.

    ssh() {
        MOCK_CALL_COUNT["ssh"]=$(( ${MOCK_CALL_COUNT["ssh"]:-0} + 1 ))
        MOCK_CALL_ARGS["ssh"]="$*"
        echo "${MOCK_STDOUT["ssh"]:-ready}"
        return "${MOCK_RETURN_CODE["ssh"]:-0}"
    }

    scp() {
        MOCK_CALL_COUNT["scp"]=$(( ${MOCK_CALL_COUNT["scp"]:-0} + 1 ))
        MOCK_CALL_ARGS["scp"]="$*"
        return "${MOCK_RETURN_CODE["scp"]:-0}"
    }

    cargo() {
        MOCK_CALL_COUNT["cargo"]=$(( ${MOCK_CALL_COUNT["cargo"]:-0} + 1 ))
        MOCK_CALL_ARGS["cargo"]="$*"
        echo "${MOCK_STDOUT["cargo"]:-Compiling networker 0.1.0}"
        return "${MOCK_RETURN_CODE["cargo"]:-0}"
    }

    curl() {
        MOCK_CALL_COUNT["curl"]=$(( ${MOCK_CALL_COUNT["curl"]:-0} + 1 ))
        MOCK_CALL_ARGS["curl"]="$*"
        echo "${MOCK_STDOUT["curl"]:-}"
        return "${MOCK_RETURN_CODE["curl"]:-0}"
    }

    gh() {
        MOCK_CALL_COUNT["gh"]=$(( ${MOCK_CALL_COUNT["gh"]:-0} + 1 ))
        MOCK_CALL_ARGS["gh"]="$*"
        echo "${MOCK_STDOUT["gh"]:-}"
        return "${MOCK_RETURN_CODE["gh"]:-0}"
    }

    az() {
        MOCK_CALL_COUNT["az"]=$(( ${MOCK_CALL_COUNT["az"]:-0} + 1 ))
        MOCK_CALL_ARGS["az"]="$*"
        echo "${MOCK_STDOUT["az"]:-}"
        return "${MOCK_RETURN_CODE["az"]:-0}"
    }

    aws() {
        MOCK_CALL_COUNT["aws"]=$(( ${MOCK_CALL_COUNT["aws"]:-0} + 1 ))
        MOCK_CALL_ARGS["aws"]="$*"
        echo "${MOCK_STDOUT["aws"]:-}"
        return "${MOCK_RETURN_CODE["aws"]:-0}"
    }

    gcloud() {
        MOCK_CALL_COUNT["gcloud"]=$(( ${MOCK_CALL_COUNT["gcloud"]:-0} + 1 ))
        MOCK_CALL_ARGS["gcloud"]="$*"
        echo "${MOCK_STDOUT["gcloud"]:-}"
        return "${MOCK_RETURN_CODE["gcloud"]:-0}"
    }

    brew() {
        MOCK_CALL_COUNT["brew"]=$(( ${MOCK_CALL_COUNT["brew"]:-0} + 1 ))
        MOCK_CALL_ARGS["brew"]="$*"
        return "${MOCK_RETURN_CODE["brew"]:-0}"
    }

    apt-get() {
        MOCK_CALL_COUNT["apt-get"]=$(( ${MOCK_CALL_COUNT["apt-get"]:-0} + 1 ))
        MOCK_CALL_ARGS["apt-get"]="$*"
        return "${MOCK_RETURN_CODE["apt-get"]:-0}"
    }

    rustup() {
        MOCK_CALL_COUNT["rustup"]=$(( ${MOCK_CALL_COUNT["rustup"]:-0} + 1 ))
        MOCK_CALL_ARGS["rustup"]="$*"
        return "${MOCK_RETURN_CODE["rustup"]:-0}"
    }

    rustc() {
        MOCK_CALL_COUNT["rustc"]=$(( ${MOCK_CALL_COUNT["rustc"]:-0} + 1 ))
        MOCK_CALL_ARGS["rustc"]="$*"
        echo "${MOCK_STDOUT["rustc"]:-rustc 1.77.0 (aedd173a2 2024-03-17)}"
        return "${MOCK_RETURN_CODE["rustc"]:-0}"
    }

    systemctl() {
        MOCK_CALL_COUNT["systemctl"]=$(( ${MOCK_CALL_COUNT["systemctl"]:-0} + 1 ))
        MOCK_CALL_ARGS["systemctl"]="$*"
        return "${MOCK_RETURN_CODE["systemctl"]:-0}"
    }

    sudo() {
        MOCK_CALL_COUNT["sudo"]=$(( ${MOCK_CALL_COUNT["sudo"]:-0} + 1 ))
        MOCK_CALL_ARGS["sudo"]="$*"
        # Actually run the command without sudo
        "$@" 2>/dev/null || true
    }

    tar() {
        MOCK_CALL_COUNT["tar"]=$(( ${MOCK_CALL_COUNT["tar"]:-0} + 1 ))
        MOCK_CALL_ARGS["tar"]="$*"
        return "${MOCK_RETURN_CODE["tar"]:-0}"
    }

    chmod() {
        MOCK_CALL_COUNT["chmod"]=$(( ${MOCK_CALL_COUNT["chmod"]:-0} + 1 ))
        MOCK_CALL_ARGS["chmod"]="$*"
        return 0
    }

    tput() {
        # no-op for tests
        return 0
    }

    stty() {
        echo "24 80"
        return 0
    }

    kill() {
        # Mock kill for spinner (just return failure to stop the loop)
        return 1
    }

    git() {
        MOCK_CALL_COUNT["git"]=$(( ${MOCK_CALL_COUNT["git"]:-0} + 1 ))
        MOCK_CALL_ARGS["git"]="$*"
        echo "${MOCK_STDOUT["git"]:-git version 2.43.0}"
        return "${MOCK_RETURN_CODE["git"]:-0}"
    }

    openssl() {
        MOCK_CALL_COUNT["openssl"]=$(( ${MOCK_CALL_COUNT["openssl"]:-0} + 1 ))
        echo "randompass123"
        return 0
    }

    sleep() {
        # no-op for tests
        return 0
    }

    wait() {
        return 0
    }
}

# ══════════════════════════════════════════════════════════════════════════════
# TEST SECTIONS
# ══════════════════════════════════════════════════════════════════════════════

# ── 1. detect_release_target() ───────────────────────────────────────────────
test_detect_release_target() {
    printf "\n${BOLD}--- detect_release_target ---${RESET}\n"

    _test_start "detect_release_target: Linux x86_64"
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    local result
    result="$(detect_release_target)"
    if assert_eq "x86_64-unknown-linux-musl" "$result" "Linux x86_64"; then _test_pass; fi

    _test_start "detect_release_target: Linux aarch64 returns empty (no musl build)"
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="aarch64"
    result="$(detect_release_target)"
    if assert_eq "" "$result" "Linux aarch64 should be empty"; then _test_pass; fi

    _test_start "detect_release_target: Darwin x86_64"
    MOCK_UNAME_S="Darwin"; MOCK_UNAME_M="x86_64"
    result="$(detect_release_target)"
    if assert_eq "x86_64-apple-darwin" "$result" "Darwin x86_64"; then _test_pass; fi

    _test_start "detect_release_target: Darwin arm64"
    MOCK_UNAME_S="Darwin"; MOCK_UNAME_M="arm64"
    result="$(detect_release_target)"
    if assert_eq "aarch64-apple-darwin" "$result" "Darwin arm64"; then _test_pass; fi

    _test_start "detect_release_target: unknown OS returns empty"
    MOCK_UNAME_S="FreeBSD"; MOCK_UNAME_M="x86_64"
    result="$(detect_release_target)"
    if assert_eq "" "$result" "unknown OS"; then _test_pass; fi

    _test_start "detect_release_target: Linux unknown arch returns empty"
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="armv7l"
    result="$(detect_release_target)"
    if assert_eq "" "$result" "unknown arch"; then _test_pass; fi
}

# ── 2. parse_args() ─────────────────────────────────────────────────────────
test_parse_args() {
    printf "\n${BOLD}--- parse_args ---${RESET}\n"

    _test_start "parse_args: component tester"
    _reset_install_state
    parse_args tester
    if assert_eq "tester" "$COMPONENT" "COMPONENT" && \
       assert_eq "0" "$DO_INSTALL_ENDPOINT" "DO_INSTALL_ENDPOINT" && \
       assert_eq "1" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER"; then _test_pass; fi

    _test_start "parse_args: component endpoint"
    _reset_install_state
    parse_args endpoint
    if assert_eq "endpoint" "$COMPONENT" "COMPONENT" && \
       assert_eq "0" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER" && \
       assert_eq "1" "$DO_INSTALL_ENDPOINT" "DO_INSTALL_ENDPOINT"; then _test_pass; fi

    _test_start "parse_args: component both"
    _reset_install_state
    parse_args both
    if assert_eq "both" "$COMPONENT" "COMPONENT" && \
       assert_eq "1" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER" && \
       assert_eq "1" "$DO_INSTALL_ENDPOINT" "DO_INSTALL_ENDPOINT"; then _test_pass; fi

    _test_start "parse_args: -y flag"
    _reset_install_state
    parse_args -y
    if assert_eq "1" "$AUTO_YES" "AUTO_YES"; then _test_pass; fi

    _test_start "parse_args: --yes flag"
    _reset_install_state
    parse_args --yes
    if assert_eq "1" "$AUTO_YES" "AUTO_YES"; then _test_pass; fi

    _test_start "parse_args: --from-source"
    _reset_install_state
    parse_args --from-source
    if assert_eq "1" "$FROM_SOURCE" "FROM_SOURCE"; then _test_pass; fi

    _test_start "parse_args: --skip-rust"
    _reset_install_state
    parse_args --skip-rust
    if assert_eq "1" "$SKIP_RUST" "SKIP_RUST"; then _test_pass; fi

    _test_start "parse_args: --azure"
    _reset_install_state
    parse_args --azure
    if assert_eq "azure" "$ENDPOINT_LOCATION" "ENDPOINT_LOCATION" && \
       assert_eq "1" "$DO_REMOTE_ENDPOINT" "DO_REMOTE_ENDPOINT"; then _test_pass; fi

    _test_start "parse_args: --tester-azure"
    _reset_install_state
    parse_args --tester-azure
    if assert_eq "azure" "$TESTER_LOCATION" "TESTER_LOCATION" && \
       assert_eq "1" "$DO_REMOTE_TESTER" "DO_REMOTE_TESTER"; then _test_pass; fi

    _test_start "parse_args: --region"
    _reset_install_state
    parse_args --region westeurope
    if assert_eq "westeurope" "$AZURE_REGION" "AZURE_REGION"; then _test_pass; fi

    _test_start "parse_args: --rg"
    _reset_install_state
    parse_args --rg my-rg
    if assert_eq "my-rg" "$AZURE_ENDPOINT_RG" "AZURE_ENDPOINT_RG"; then _test_pass; fi

    _test_start "parse_args: --vm"
    _reset_install_state
    parse_args --vm my-vm
    if assert_eq "my-vm" "$AZURE_ENDPOINT_VM" "AZURE_ENDPOINT_VM"; then _test_pass; fi

    _test_start "parse_args: --tester-rg"
    _reset_install_state
    parse_args --tester-rg tester-rg
    if assert_eq "tester-rg" "$AZURE_TESTER_RG" "AZURE_TESTER_RG"; then _test_pass; fi

    _test_start "parse_args: --tester-vm"
    _reset_install_state
    parse_args --tester-vm tester-vm
    if assert_eq "tester-vm" "$AZURE_TESTER_VM" "AZURE_TESTER_VM"; then _test_pass; fi

    _test_start "parse_args: --vm-size sets both tester and endpoint sizes"
    _reset_install_state
    parse_args --vm-size Standard_D2s_v3
    if assert_eq "Standard_D2s_v3" "$AZURE_TESTER_SIZE" "AZURE_TESTER_SIZE" && \
       assert_eq "Standard_D2s_v3" "$AZURE_ENDPOINT_SIZE" "AZURE_ENDPOINT_SIZE"; then _test_pass; fi

    _test_start "parse_args: --aws"
    _reset_install_state
    parse_args --aws
    if assert_eq "aws" "$ENDPOINT_LOCATION" "ENDPOINT_LOCATION" && \
       assert_eq "1" "$DO_REMOTE_ENDPOINT" "DO_REMOTE_ENDPOINT"; then _test_pass; fi

    _test_start "parse_args: --tester-aws"
    _reset_install_state
    parse_args --tester-aws
    if assert_eq "aws" "$TESTER_LOCATION" "TESTER_LOCATION" && \
       assert_eq "1" "$DO_REMOTE_TESTER" "DO_REMOTE_TESTER"; then _test_pass; fi

    _test_start "parse_args: --aws-region"
    _reset_install_state
    parse_args --aws-region eu-west-1
    if assert_eq "eu-west-1" "$AWS_REGION" "AWS_REGION"; then _test_pass; fi

    _test_start "parse_args: --aws-instance-type sets both"
    _reset_install_state
    parse_args --aws-instance-type t3.large
    if assert_eq "t3.large" "$AWS_TESTER_INSTANCE_TYPE" "AWS_TESTER_INSTANCE_TYPE" && \
       assert_eq "t3.large" "$AWS_ENDPOINT_INSTANCE_TYPE" "AWS_ENDPOINT_INSTANCE_TYPE"; then _test_pass; fi

    _test_start "parse_args: --aws-endpoint-name"
    _reset_install_state
    parse_args --aws-endpoint-name my-ep
    if assert_eq "my-ep" "$AWS_ENDPOINT_NAME" "AWS_ENDPOINT_NAME"; then _test_pass; fi

    _test_start "parse_args: --aws-tester-name"
    _reset_install_state
    parse_args --aws-tester-name my-ts
    if assert_eq "my-ts" "$AWS_TESTER_NAME" "AWS_TESTER_NAME"; then _test_pass; fi

    _test_start "parse_args: --gcp"
    _reset_install_state
    parse_args --gcp
    if assert_eq "gcp" "$ENDPOINT_LOCATION" "ENDPOINT_LOCATION" && \
       assert_eq "1" "$DO_REMOTE_ENDPOINT" "DO_REMOTE_ENDPOINT"; then _test_pass; fi

    _test_start "parse_args: --tester-gcp"
    _reset_install_state
    parse_args --tester-gcp
    if assert_eq "gcp" "$TESTER_LOCATION" "TESTER_LOCATION" && \
       assert_eq "1" "$DO_REMOTE_TESTER" "DO_REMOTE_TESTER"; then _test_pass; fi

    _test_start "parse_args: --gcp-region"
    _reset_install_state
    parse_args --gcp-region europe-west1
    if assert_eq "europe-west1" "$GCP_REGION" "GCP_REGION"; then _test_pass; fi

    _test_start "parse_args: --gcp-zone"
    _reset_install_state
    parse_args --gcp-zone europe-west1-b
    if assert_eq "europe-west1-b" "$GCP_ZONE" "GCP_ZONE"; then _test_pass; fi

    _test_start "parse_args: --gcp-machine-type sets both"
    _reset_install_state
    parse_args --gcp-machine-type e2-medium
    if assert_eq "e2-medium" "$GCP_TESTER_MACHINE_TYPE" "GCP_TESTER_MACHINE_TYPE" && \
       assert_eq "e2-medium" "$GCP_ENDPOINT_MACHINE_TYPE" "GCP_ENDPOINT_MACHINE_TYPE"; then _test_pass; fi

    _test_start "parse_args: --gcp-project"
    _reset_install_state
    parse_args --gcp-project my-project
    if assert_eq "my-project" "$GCP_PROJECT" "GCP_PROJECT"; then _test_pass; fi

    _test_start "parse_args: unknown option produces error"
    _reset_install_state
    local output
    output="$(parse_args --bogus-flag 2>&1)" || true
    # The function calls exit 1 on unknown options, which in subshell produces rc!=0
    # We just check it was recognized as unknown
    if assert_contains "$output" "Unknown option" "unknown flag error"; then _test_pass; fi

    _test_start "parse_args: --azure + endpoint enables endpoint"
    _reset_install_state
    parse_args --azure endpoint
    if assert_eq "1" "$DO_REMOTE_ENDPOINT" "DO_REMOTE_ENDPOINT" && \
       assert_eq "1" "$DO_INSTALL_ENDPOINT" "DO_INSTALL_ENDPOINT" && \
       assert_eq "0" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER"; then _test_pass; fi

    _test_start "parse_args: cloud flag enables component if not set"
    _reset_install_state
    parse_args --tester-aws tester
    # --tester-aws sets DO_REMOTE_TESTER=1; tester sets DO_INSTALL_ENDPOINT=0
    # Then the reconciliation ensures DO_INSTALL_TESTER=1
    if assert_eq "1" "$DO_REMOTE_TESTER" "DO_REMOTE_TESTER" && \
       assert_eq "1" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER"; then _test_pass; fi

    _test_start "parse_args: combined flags"
    _reset_install_state
    parse_args -y --from-source --skip-rust both
    if assert_eq "1" "$AUTO_YES" "AUTO_YES" && \
       assert_eq "1" "$FROM_SOURCE" "FROM_SOURCE" && \
       assert_eq "1" "$SKIP_RUST" "SKIP_RUST" && \
       assert_eq "both" "$COMPONENT" "COMPONENT"; then _test_pass; fi
}

# ── 3. discover_system() ────────────────────────────────────────────────────
test_discover_system() {
    printf "\n${BOLD}--- discover_system ---${RESET}\n"

    _test_start "discover_system: detects OS and arch"
    _reset_install_state
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    # Mock command -v to make cargo unavailable
    local _orig_path="$PATH"
    # Override cargo to fail
    cargo() { return 1; }
    # Override gh to fail
    gh() { return 1; }
    # Override az to fail
    az() { return 1; }
    # Override aws to fail
    aws() { return 1; }
    # Override gcloud to fail
    gcloud() { return 1; }
    # Override certutil
    certutil() { return 1; }

    discover_system
    if assert_eq "Linux" "$SYS_OS" "SYS_OS" && \
       assert_eq "x86_64" "$SYS_ARCH" "SYS_ARCH"; then _test_pass; fi
    # Re-setup mocks since we overrode some
    _setup_mocks

    _test_start "discover_system: sets RELEASE_TARGET for Linux x86_64"
    _reset_install_state
    FROM_SOURCE=0
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    # gh fails, curl fails => source mode
    gh() { return 1; }
    curl() { return 1; }
    cargo() { return 1; }
    az() { return 1; }
    aws() { return 1; }
    gcloud() { return 1; }
    certutil() { return 1; }

    discover_system
    if assert_eq "x86_64-unknown-linux-musl" "$RELEASE_TARGET" "RELEASE_TARGET"; then _test_pass; fi
    _setup_mocks

    _test_start "discover_system: gh auth ok => release mode"
    _reset_install_state
    FROM_SOURCE=0
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    # gh succeeds for auth status and release list
    gh() {
        MOCK_CALL_COUNT["gh"]=$(( ${MOCK_CALL_COUNT["gh"]:-0} + 1 ))
        MOCK_CALL_ARGS["gh"]="$*"
        if [[ "$*" == *"auth status"* ]]; then return 0; fi
        if [[ "$*" == *"release list"* ]]; then echo "v0.12.93"; return 0; fi
        return 0
    }
    cargo() { return 1; }
    az() { return 1; }
    aws() { return 1; }
    gcloud() { return 1; }
    certutil() { return 1; }

    discover_system
    if assert_eq "1" "$RELEASE_AVAILABLE" "RELEASE_AVAILABLE" && \
       assert_eq "release" "$INSTALL_METHOD" "INSTALL_METHOD" && \
       assert_eq "v0.12.93" "$NETWORKER_VERSION" "NETWORKER_VERSION"; then _test_pass; fi
    _setup_mocks

    _test_start "discover_system: gh unavailable, curl fallback to GitHub API"
    _reset_install_state
    FROM_SOURCE=0
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    gh() { return 1; }
    curl() {
        MOCK_CALL_COUNT["curl"]=$(( ${MOCK_CALL_COUNT["curl"]:-0} + 1 ))
        MOCK_CALL_ARGS["curl"]="$*"
        if [[ "$*" == *"api.github.com"* ]]; then
            echo '{"tag_name": "v0.12.90"}'
            return 0
        fi
        return 1
    }
    cargo() { return 1; }
    az() { return 1; }
    aws() { return 1; }
    gcloud() { return 1; }
    certutil() { return 1; }

    discover_system
    if assert_eq "1" "$RELEASE_AVAILABLE" "RELEASE_AVAILABLE" && \
       assert_eq "release" "$INSTALL_METHOD" "INSTALL_METHOD" && \
       assert_eq "v0.12.90" "$NETWORKER_VERSION" "NETWORKER_VERSION"; then _test_pass; fi
    _setup_mocks

    _test_start "discover_system: FROM_SOURCE=1 skips release detection"
    _reset_install_state
    FROM_SOURCE=1
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    gh() { echo "v1.0.0"; return 0; }
    cargo() { return 1; }
    az() { return 1; }
    aws() { return 1; }
    gcloud() { return 1; }
    certutil() { return 1; }

    discover_system
    if assert_eq "source" "$INSTALL_METHOD" "INSTALL_METHOD" && \
       assert_eq "" "$RELEASE_TARGET" "RELEASE_TARGET should be empty when FROM_SOURCE=1"; then _test_pass; fi
    _setup_mocks

    _test_start "discover_system: Rust not installed => DO_RUST_INSTALL=1"
    _reset_install_state
    SKIP_RUST=0
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    unset -f cargo 2>/dev/null || true
    gh() { return 1; }
    curl() { return 1; }
    az() { return 1; }
    aws() { return 1; }
    gcloud() { return 1; }
    certutil() { return 1; }

    discover_system
    if assert_eq "0" "$RUST_EXISTS" "RUST_EXISTS" && \
       assert_eq "1" "$DO_RUST_INSTALL" "DO_RUST_INSTALL"; then _test_pass; fi
    _setup_mocks

    _test_start "discover_system: SKIP_RUST=1 prevents DO_RUST_INSTALL"
    _reset_install_state
    SKIP_RUST=1
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    cargo() { return 1; }
    gh() { return 1; }
    curl() { return 1; }
    az() { return 1; }
    aws() { return 1; }
    gcloud() { return 1; }
    certutil() { return 1; }

    discover_system
    if assert_eq "0" "$DO_RUST_INSTALL" "DO_RUST_INSTALL"; then _test_pass; fi
    _setup_mocks

    _test_start "discover_system: version fallback to INSTALLER_VERSION"
    _reset_install_state
    FROM_SOURCE=1
    MOCK_UNAME_S="Linux"; MOCK_UNAME_M="x86_64"
    cargo() { return 1; }
    gh() { return 1; }
    curl() { return 1; }
    az() { return 1; }
    aws() { return 1; }
    gcloud() { return 1; }
    certutil() { return 1; }

    discover_system
    if assert_eq "v0.12.93" "$NETWORKER_VERSION" "NETWORKER_VERSION should fallback to INSTALLER_VERSION"; then _test_pass; fi
    _setup_mocks
}

# ── 4. prompt_component_selection ────────────────────────────────────────────
test_prompt_component_selection() {
    printf "\n${BOLD}--- prompt_component_selection ---${RESET}\n"

    _test_start "prompt_component_selection: AUTO_YES skips prompt"
    _reset_install_state
    AUTO_YES=1
    prompt_component_selection
    # Should remain at defaults (both)
    if assert_eq "1" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER" && \
       assert_eq "1" "$DO_INSTALL_ENDPOINT" "DO_INSTALL_ENDPOINT"; then _test_pass; fi

    _test_start "prompt_component_selection: CLI component set skips prompt"
    _reset_install_state
    COMPONENT="tester"
    prompt_component_selection
    # Should not change anything -- early return
    if assert_eq "tester" "$COMPONENT" "COMPONENT unchanged"; then _test_pass; fi

    _test_start "parse_args: --component tester sets correct flags"
    _reset_install_state
    parse_args tester
    if assert_eq "1" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER" && \
       assert_eq "0" "$DO_INSTALL_ENDPOINT" "DO_INSTALL_ENDPOINT"; then _test_pass; fi

    _test_start "parse_args: --component endpoint sets correct flags"
    _reset_install_state
    parse_args endpoint
    if assert_eq "0" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER" && \
       assert_eq "1" "$DO_INSTALL_ENDPOINT" "DO_INSTALL_ENDPOINT"; then _test_pass; fi

    _test_start "parse_args: --component both sets correct flags"
    _reset_install_state
    parse_args both
    if assert_eq "1" "$DO_INSTALL_TESTER" "DO_INSTALL_TESTER" && \
       assert_eq "1" "$DO_INSTALL_ENDPOINT" "DO_INSTALL_ENDPOINT"; then _test_pass; fi
}

# ── 5. step_cargo_install ────────────────────────────────────────────────────
test_step_cargo_install() {
    printf "\n${BOLD}--- step_cargo_install ---${RESET}\n"

    _test_start "step_cargo_install: uses --git and --force"
    _reset_install_state
    _mock_reset
    FROM_SOURCE=1
    RELEASE_TARGET=""
    CHROME_AVAILABLE=0

    # We need to capture the cargo args. Override _cargo_progress to
    # just call cargo directly (skip spinner).
    _cargo_progress() {
        shift  # label
        MOCK_CALL_ARGS["_cargo_progress_cmd"]="$*"
        return 0
    }

    # Mock the binary to be found after install
    local _mock_bin="${TEST_TMPDIR}/networker-tester"
    INSTALL_DIR="$TEST_TMPDIR"
    echo '#!/bin/sh' > "$_mock_bin"
    echo 'echo "networker-tester 0.12.93"' >> "$_mock_bin"
    command chmod +x "$_mock_bin"

    # Also need cc/gcc/clang to be "available" to skip linker check
    cc() { return 0; }

    step_cargo_install "networker-tester" >/dev/null 2>&1
    local cargo_cmd="${MOCK_CALL_ARGS["_cargo_progress_cmd"]:-}"
    if assert_contains "$cargo_cmd" "--git" "should have --git flag" && \
       assert_contains "$cargo_cmd" "--force" "should have --force flag"; then _test_pass; fi

    _test_start "step_cargo_install: does NOT use --locked flag"
    if assert_not_contains "$cargo_cmd" "--locked" "must not have --locked flag"; then _test_pass; fi

    _test_start "step_cargo_install: adds --features browser for tester with Chrome"
    _reset_install_state
    _mock_reset
    FROM_SOURCE=1
    RELEASE_TARGET=""
    CHROME_AVAILABLE=1
    INSTALL_DIR="$TEST_TMPDIR"

    _cargo_progress() {
        shift
        MOCK_CALL_ARGS["_cargo_progress_cmd"]="$*"
        return 0
    }
    cc() { return 0; }

    step_cargo_install "networker-tester" >/dev/null 2>&1
    cargo_cmd="${MOCK_CALL_ARGS["_cargo_progress_cmd"]:-}"
    if assert_contains "$cargo_cmd" "--features browser" "should have --features browser"; then _test_pass; fi

    _test_start "step_cargo_install: no --features browser for endpoint even with Chrome"
    _reset_install_state
    _mock_reset
    FROM_SOURCE=1
    RELEASE_TARGET=""
    CHROME_AVAILABLE=1
    INSTALL_DIR="$TEST_TMPDIR"

    local _mock_bin2="${TEST_TMPDIR}/networker-endpoint"
    echo '#!/bin/sh' > "$_mock_bin2"
    echo 'echo "networker-endpoint 0.12.93"' >> "$_mock_bin2"
    command chmod +x "$_mock_bin2"

    _cargo_progress() {
        shift
        MOCK_CALL_ARGS["_cargo_progress_cmd"]="$*"
        return 0
    }
    cc() { return 0; }

    step_cargo_install "networker-endpoint" >/dev/null 2>&1
    cargo_cmd="${MOCK_CALL_ARGS["_cargo_progress_cmd"]:-}"
    if assert_not_contains "$cargo_cmd" "--features browser" "endpoint should not have --features browser"; then _test_pass; fi

    _test_start "step_cargo_install: no --features browser for tester without Chrome"
    _reset_install_state
    _mock_reset
    FROM_SOURCE=1
    RELEASE_TARGET=""
    CHROME_AVAILABLE=0
    INSTALL_DIR="$TEST_TMPDIR"

    _cargo_progress() {
        shift
        MOCK_CALL_ARGS["_cargo_progress_cmd"]="$*"
        return 0
    }
    cc() { return 0; }

    step_cargo_install "networker-tester" >/dev/null 2>&1
    cargo_cmd="${MOCK_CALL_ARGS["_cargo_progress_cmd"]:-}"
    if assert_not_contains "$cargo_cmd" "--features browser" "no Chrome -> no browser feature"; then _test_pass; fi

    # Restore mocks
    _setup_mocks
}

# ── 6. _cargo_progress: stdin from /dev/null ─────────────────────────────────
test_cargo_progress_stdin() {
    printf "\n${BOLD}--- _cargo_progress stdin handling ---${RESET}\n"

    _test_start "_cargo_progress: feeds stdin from /dev/null (non-TTY mode)"
    _reset_install_state
    _mock_reset

    # In non-TTY mode (which we are in during tests), _cargo_progress
    # runs the command with </dev/null. We check the source code.
    local source_lines
    source_lines="$(grep '</dev/null' "$INSTALL_SH" | grep -c '_cargo_progress\|"$@" </dev/null')"
    # The _cargo_progress function should have </dev/null on both the
    # TTY and non-TTY paths.
    if [[ "$source_lines" -ge 2 ]]; then
        _test_pass
    else
        _test_fail "Expected at least 2 lines with </dev/null in _cargo_progress"
    fi
}

# ── 7. Interactive prompts read from /dev/tty ────────────────────────────────
test_tty_reads() {
    printf "\n${BOLD}--- Interactive prompts use /dev/tty ---${RESET}\n"

    _test_start "Interactive read commands use /dev/tty"
    local tty_reads
    tty_reads="$(grep -c '</dev/tty' "$INSTALL_SH")"
    # There should be many read </dev/tty calls throughout
    if [[ "$tty_reads" -ge 20 ]]; then
        _test_pass
    else
        _test_fail "Expected >= 20 '</dev/tty' occurrences, found $tty_reads"
    fi

    _test_start "ask_yn() reads from /dev/tty"
    local ask_yn_source
    ask_yn_source="$(sed -n '/^ask_yn()/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$ask_yn_source" "/dev/tty" "ask_yn should read from /dev/tty"; then _test_pass; fi
}

# ── 8. Cloud CLI commands use </dev/null ─────────────────────────────────────
test_cloud_stdin_protection() {
    printf "\n${BOLD}--- Cloud CLI stdin protection ---${RESET}\n"

    _test_start "gh auth status uses </dev/null"
    local gh_auth_lines
    gh_auth_lines="$(grep 'gh auth status' "$INSTALL_SH" | grep -c '</dev/null')"
    if [[ "$gh_auth_lines" -ge 1 ]]; then
        _test_pass
    else
        _test_fail "gh auth status should use </dev/null"
    fi

    _test_start "gh release list uses </dev/null"
    local gh_release_lines
    gh_release_lines="$(grep -A2 'gh release list' "$INSTALL_SH" | grep -c '</dev/null')"
    if [[ "$gh_release_lines" -ge 1 ]]; then
        _test_pass
    else
        _test_fail "gh release list should use </dev/null"
    fi

    _test_start "az account show uses </dev/null"
    local az_acct_lines
    az_acct_lines="$(grep 'az account show' "$INSTALL_SH" | grep -c '</dev/null')"
    if [[ "$az_acct_lines" -ge 1 ]]; then
        _test_pass
    else
        _test_fail "az account show should use </dev/null"
    fi

    _test_start "aws sts get-caller-identity uses </dev/null"
    local aws_sts_lines
    aws_sts_lines="$(grep 'aws sts get-caller-identity' "$INSTALL_SH" | grep -c '</dev/null')"
    if [[ "$aws_sts_lines" -ge 1 ]]; then
        _test_pass
    else
        _test_fail "aws sts get-caller-identity should use </dev/null"
    fi

    _test_start "curl GitHub API uses </dev/null"
    local curl_api_lines
    curl_api_lines="$(grep 'api.github.com' "$INSTALL_SH" | grep -c '</dev/null')"
    if [[ "$curl_api_lines" -ge 1 ]]; then
        _test_pass
    else
        _test_fail "curl GitHub API call should use </dev/null"
    fi

    _test_start "_gcp_ssh_run uses < /dev/null"
    local gcp_ssh_run_lines
    gcp_ssh_run_lines="$(sed -n '/_gcp_ssh_run()/,/^}/p' "$INSTALL_SH" | grep -c '/dev/null')"
    if [[ "$gcp_ssh_run_lines" -ge 1 ]]; then
        _test_pass
    else
        _test_fail "_gcp_ssh_run should use < /dev/null"
    fi
}

# ── 9. SSH commands don't consume piped stdin ────────────────────────────────
test_ssh_stdin_protection() {
    printf "\n${BOLD}--- SSH stdin protection ---${RESET}\n"

    _test_start "SSH commands in _wait_for_ssh use BatchMode=yes"
    local ssh_batch
    ssh_batch="$(sed -n '/_wait_for_ssh()/,/^}/p' "$INSTALL_SH" | grep -c 'BatchMode=yes')"
    if [[ "$ssh_batch" -ge 1 ]]; then
        _test_pass
    else
        _test_fail "SSH in _wait_for_ssh should use BatchMode=yes"
    fi

    _test_start "SSH commands use StrictHostKeyChecking=no"
    local ssh_strict
    ssh_strict="$(grep 'StrictHostKeyChecking=no' "$INSTALL_SH" | wc -l)"
    if [[ "$ssh_strict" -ge 5 ]]; then
        _test_pass
    else
        _test_fail "Expected >= 5 SSH commands with StrictHostKeyChecking=no, found $ssh_strict"
    fi
}

# ── 10. step_download_release ────────────────────────────────────────────────
test_step_download_release() {
    printf "\n${BOLD}--- step_download_release ---${RESET}\n"

    _test_start "step_download_release: uses correct archive naming"
    _reset_install_state
    _mock_reset
    RELEASE_TARGET="x86_64-unknown-linux-musl"
    NETWORKER_VERSION="v0.12.93"
    INSTALL_DIR="$TEST_TMPDIR"

    # gh succeeds for auth but fail for download to test naming
    gh() {
        MOCK_CALL_COUNT["gh"]=$(( ${MOCK_CALL_COUNT["gh"]:-0} + 1 ))
        MOCK_CALL_ARGS["gh"]="$*"
        if [[ "$*" == *"auth status"* ]]; then return 0; fi
        if [[ "$*" == *"release download"* ]]; then
            # Record the download pattern
            MOCK_CALL_ARGS["gh_download"]="$*"
            return 1  # fail so we can check the args
        fi
        return 0
    }
    curl() {
        MOCK_CALL_COUNT["curl"]=$(( ${MOCK_CALL_COUNT["curl"]:-0} + 1 ))
        MOCK_CALL_ARGS["curl"]="$*"
        return 1  # fail so it falls back
    }
    mktemp() {
        if [[ "$1" == "-d" ]]; then
            command mkdir -p "$TEST_TMPDIR/dl" 2>/dev/null
            echo "$TEST_TMPDIR/dl"
        else
            command mktemp "$@"
        fi
    }

    step_download_release "networker-tester" >/dev/null 2>&1 || true
    local gh_dl_args="${MOCK_CALL_ARGS["gh_download"]:-}"
    if assert_contains "$gh_dl_args" "networker-tester-x86_64-unknown-linux-musl.tar.gz" \
         "archive naming: binary-target.tar.gz"; then _test_pass; fi

    _test_start "step_download_release: uses gh release download with --pattern"
    if assert_contains "$gh_dl_args" "--pattern" "should use --pattern flag"; then _test_pass; fi

    _test_start "step_download_release: uses --clobber flag"
    if assert_contains "$gh_dl_args" "--clobber" "should use --clobber flag"; then _test_pass; fi

    _test_start "step_download_release: falls back to curl on gh failure"
    local curl_count
    curl_count="${MOCK_CALL_COUNT["curl"]:-0}"
    if [[ "$curl_count" -ge 1 ]]; then
        _test_pass
    else
        _test_fail "should fall back to curl when gh download fails"
    fi

    unset -f mktemp 2>/dev/null || true
    _setup_mocks
}

# ── 11. _wait_for_ssh ────────────────────────────────────────────────────────
test_wait_for_ssh() {
    printf "\n${BOLD}--- _wait_for_ssh ---${RESET}\n"

    _test_start "_wait_for_ssh: succeeds on first try"
    _reset_install_state
    _mock_reset
    local ssh_attempt=0
    ssh() {
        ssh_attempt=$((ssh_attempt + 1))
        MOCK_CALL_COUNT["ssh"]=$(( ${MOCK_CALL_COUNT["ssh"]:-0} + 1 ))
        MOCK_CALL_ARGS["ssh"]="$*"
        echo "ready"
        return 0
    }

    _wait_for_ssh "1.2.3.4" "azureuser" "test-vm" >/dev/null 2>&1
    if assert_eq "1" "$ssh_attempt" "should succeed on first attempt"; then _test_pass; fi

    _test_start "_wait_for_ssh: retries on failure then succeeds"
    _reset_install_state
    _mock_reset
    ssh_attempt=0
    ssh() {
        ssh_attempt=$((ssh_attempt + 1))
        MOCK_CALL_COUNT["ssh"]=$(( ${MOCK_CALL_COUNT["ssh"]:-0} + 1 ))
        MOCK_CALL_ARGS["ssh"]="$*"
        if [[ $ssh_attempt -lt 3 ]]; then
            return 1
        fi
        echo "ready"
        return 0
    }

    _wait_for_ssh "1.2.3.4" "ubuntu" "test-vm" >/dev/null 2>&1
    if [[ $ssh_attempt -eq 3 ]]; then
        _test_pass
    else
        _test_fail "expected 3 attempts, got $ssh_attempt"
    fi

    _test_start "_wait_for_ssh: uses ConnectTimeout=5"
    local ssh_args="${MOCK_CALL_ARGS["ssh"]:-}"
    if assert_contains "$ssh_args" "ConnectTimeout=5" "should use ConnectTimeout=5"; then _test_pass; fi

    _test_start "_wait_for_ssh: uses StrictHostKeyChecking=no"
    if assert_contains "$ssh_args" "StrictHostKeyChecking=no" "should use StrictHostKeyChecking=no"; then _test_pass; fi

    _setup_mocks
}

# ── 12. _remote_install_binary ───────────────────────────────────────────────
test_remote_install_binary() {
    printf "\n${BOLD}--- _remote_install_binary ---${RESET}\n"

    _test_start "_remote_install_binary: uses StrictHostKeyChecking=no"
    _reset_install_state
    _mock_reset
    NETWORKER_VERSION="v0.12.93"

    # gh reports assets available
    gh() {
        MOCK_CALL_COUNT["gh"]=$(( ${MOCK_CALL_COUNT["gh"]:-0} + 1 ))
        MOCK_CALL_ARGS["gh"]="$*"
        if [[ "$*" == *"release view"* ]]; then
            echo "networker-endpoint-x86_64-unknown-linux-musl.tar.gz"
            return 0
        fi
        if [[ "$*" == *"release download"* ]]; then
            # create a fake archive
            MOCK_CALL_ARGS["gh_download"]="$*"
            return 0
        fi
        return 0
    }
    ssh() {
        MOCK_CALL_COUNT["ssh"]=$(( ${MOCK_CALL_COUNT["ssh"]:-0} + 1 ))
        MOCK_CALL_ARGS["ssh"]="${MOCK_CALL_ARGS["ssh"]:-}|||$*"
        if [[ "$*" == *"uname -m"* ]]; then
            echo "x86_64"
            return 0
        fi
        echo "ok"
        return 0
    }
    scp() {
        MOCK_CALL_COUNT["scp"]=$(( ${MOCK_CALL_COUNT["scp"]:-0} + 1 ))
        MOCK_CALL_ARGS["scp"]="$*"
        return 0
    }
    tar() {
        return 0
    }
    mktemp() {
        if [[ "$1" == "-d" ]]; then
            command mkdir -p "$TEST_TMPDIR/remote_dl" 2>/dev/null
            echo "$TEST_TMPDIR/remote_dl"
        else
            command mktemp "$@"
        fi
    }

    _remote_install_binary "networker-endpoint" "1.2.3.4" "azureuser" >/dev/null 2>&1 || true

    local scp_args="${MOCK_CALL_ARGS["scp"]:-}"
    if assert_contains "$scp_args" "StrictHostKeyChecking=no" "scp should use StrictHostKeyChecking=no"; then _test_pass; fi

    _test_start "_remote_install_binary: SSH commands include StrictHostKeyChecking=no"
    local all_ssh_args="${MOCK_CALL_ARGS["ssh"]:-}"
    if assert_contains "$all_ssh_args" "StrictHostKeyChecking=no" "ssh should use StrictHostKeyChecking=no"; then _test_pass; fi

    unset -f mktemp 2>/dev/null || true
    _setup_mocks
}

# ── 13. _remote_create_endpoint_service ──────────────────────────────────────
test_remote_create_endpoint_service() {
    printf "\n${BOLD}--- _remote_create_endpoint_service ---${RESET}\n"

    _test_start "_remote_create_endpoint_service: sends systemd unit"
    _reset_install_state
    _mock_reset

    local ssh_stdin_captured=""
    ssh() {
        MOCK_CALL_COUNT["ssh"]=$(( ${MOCK_CALL_COUNT["ssh"]:-0} + 1 ))
        MOCK_CALL_ARGS["ssh"]="$*"
        # Read stdin to capture the heredoc
        ssh_stdin_captured="$(cat)"
        return 0
    }

    _remote_create_endpoint_service "1.2.3.4" "azureuser" >/dev/null 2>&1

    if assert_contains "$ssh_stdin_captured" "ExecStart=/usr/local/bin/networker-endpoint" \
         "systemd unit should have correct ExecStart"; then _test_pass; fi

    _test_start "_remote_create_endpoint_service: systemd unit has Restart=always"
    if assert_contains "$ssh_stdin_captured" "Restart=always" "should have Restart=always"; then _test_pass; fi

    _test_start "_remote_create_endpoint_service: enables and starts service"
    if assert_contains "$ssh_stdin_captured" "systemctl daemon-reload" "should daemon-reload" && \
       assert_contains "$ssh_stdin_captured" "systemctl enable" "should enable" && \
       assert_contains "$ssh_stdin_captured" "systemctl start" "should start"; then _test_pass; fi

    _test_start "_remote_create_endpoint_service: uses StrictHostKeyChecking=no"
    local ssh_args="${MOCK_CALL_ARGS["ssh"]:-}"
    if assert_contains "$ssh_args" "StrictHostKeyChecking=no" "should use StrictHostKeyChecking=no"; then _test_pass; fi

    _setup_mocks
}

# ── 14. VM existence checks ─────────────────────────────────────────────────
test_vm_existence_checks() {
    printf "\n${BOLD}--- VM existence checks (source code verification) ---${RESET}\n"

    # Azure VM existence: reuse, rename, delete
    _test_start "Azure VM existence: has reuse path"
    local azure_vm_source
    azure_vm_source="$(sed -n '/step_azure_create_vm/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$azure_vm_source" "Reuse existing VM" "Azure should have reuse path"; then _test_pass; fi

    _test_start "Azure VM existence: has rename path"
    if assert_contains "$azure_vm_source" "Pick a different name" "Azure should have rename path"; then _test_pass; fi

    _test_start "Azure VM existence: has delete path"
    if assert_contains "$azure_vm_source" "Delete and recreate" "Azure should have delete path"; then _test_pass; fi

    # AWS instance existence: reuse (start stopped), rename, delete
    _test_start "AWS instance existence: has reuse path"
    local aws_launch_source
    aws_launch_source="$(sed -n '/_aws_launch_instance/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$aws_launch_source" "Reuse existing instance" "AWS should have reuse path"; then _test_pass; fi

    _test_start "AWS instance existence: starts stopped instances"
    if assert_contains "$aws_launch_source" "start-instances" "AWS should start stopped instances"; then _test_pass; fi

    _test_start "AWS instance existence: has rename path"
    if assert_contains "$aws_launch_source" "Pick a different name" "AWS should have rename path"; then _test_pass; fi

    _test_start "AWS instance existence: has terminate path"
    if assert_contains "$aws_launch_source" "Terminate and recreate" "AWS should have terminate path"; then _test_pass; fi

    # GCP instance existence: reuse, rename, delete
    _test_start "GCP instance existence: has reuse path"
    local gcp_create_source
    gcp_create_source="$(sed -n '/_gcp_create_instance/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$gcp_create_source" "Reuse existing instance" "GCP should have reuse path"; then _test_pass; fi

    _test_start "GCP instance existence: has rename path"
    if assert_contains "$gcp_create_source" "Pick a different name" "GCP should have rename path"; then _test_pass; fi

    _test_start "GCP instance existence: has delete path"
    if assert_contains "$gcp_create_source" "Delete and recreate" "GCP should have delete path"; then _test_pass; fi
}

# ── 15. Authentication helpers ───────────────────────────────────────────────
test_ensure_auth() {
    printf "\n${BOLD}--- Authentication helpers ---${RESET}\n"

    _test_start "ensure_azure_cli: installs and logs in (source verification)"
    local az_source
    az_source="$(sed -n '/^ensure_azure_cli/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$az_source" "Install Azure CLI now" "should offer to install" && \
       assert_contains "$az_source" "az login" "should offer to log in"; then _test_pass; fi

    _test_start "ensure_aws_cli: SSO and access key paths"
    local aws_source
    aws_source="$(sed -n '/^ensure_aws_cli/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$aws_source" "_aws_do_login_sso" "should have SSO path" && \
       assert_contains "$aws_source" "_aws_do_login_keys" "should have access key path"; then _test_pass; fi

    _test_start "ensure_gcp_cli: device code login"
    local gcp_source
    gcp_source="$(sed -n '/^ensure_gcp_cli/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$gcp_source" "no-launch-browser" "should use device code (no-launch-browser)"; then _test_pass; fi

    _test_start "_az_do_login: multi-tenant retry logic"
    local az_login_source
    az_login_source="$(sed -n '/^_az_do_login/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$az_login_source" "tenant" "should handle tenant" && \
       assert_contains "$az_login_source" "Enter tenant ID" "should ask for tenant ID on retry"; then _test_pass; fi
}

# ── 16. step_generate_config ─────────────────────────────────────────────────
test_step_generate_config() {
    printf "\n${BOLD}--- step_generate_config ---${RESET}\n"

    _test_start "step_generate_config: produces valid JSON"
    _reset_install_state
    _mock_reset
    AZURE_EXTRA_ENDPOINT_IPS=()
    TESTER_LOCATION="local"
    local _orig_pwd="$PWD"
    cd "$TEST_TMPDIR"

    step_generate_config "10.0.0.1" >/dev/null 2>&1

    local config_content
    config_content="$(cat "$TEST_TMPDIR/networker-cloud.json" 2>/dev/null || echo "")"

    if assert_contains "$config_content" '"targets"' "should have targets key" && \
       assert_contains "$config_content" "10.0.0.1" "should contain endpoint IP" && \
       assert_contains "$config_content" '"modes"' "should have modes key" && \
       assert_contains "$config_content" '"runs": 5' "should have runs: 5"; then _test_pass; fi

    _test_start "step_generate_config: config file path set correctly"
    if assert_eq "$TEST_TMPDIR/networker-cloud.json" "$CONFIG_FILE_PATH" "CONFIG_FILE_PATH"; then _test_pass; fi

    _test_start "step_generate_config: includes insecure flag"
    if assert_contains "$config_content" '"insecure": true' "should have insecure: true"; then _test_pass; fi

    _test_start "step_generate_config: multi-endpoint config"
    _reset_install_state
    AZURE_EXTRA_ENDPOINT_IPS=("10.0.0.2:westeurope" "10.0.0.3:japaneast")
    TESTER_LOCATION="local"

    step_generate_config "10.0.0.1" >/dev/null 2>&1
    config_content="$(cat "$TEST_TMPDIR/networker-cloud.json" 2>/dev/null || echo "")"

    if assert_contains "$config_content" "10.0.0.1" "should contain primary IP" && \
       assert_contains "$config_content" "10.0.0.2" "should contain extra IP 1" && \
       assert_contains "$config_content" "10.0.0.3" "should contain extra IP 2"; then _test_pass; fi

    cd "$_orig_pwd"
    _setup_mocks
}

# ── 17. detect_pkg_manager ───────────────────────────────────────────────────
test_detect_pkg_manager() {
    printf "\n${BOLD}--- detect_pkg_manager ---${RESET}\n"

    _test_start "detect_pkg_manager: Darwin with brew"
    MOCK_UNAME_S="Darwin"
    brew() { return 0; }
    local result
    result="$(detect_pkg_manager)"
    if assert_eq "brew" "$result" "should detect brew on macOS"; then _test_pass; fi

    _test_start "detect_pkg_manager: Linux with apt-get"
    MOCK_UNAME_S="Linux"
    # This test is tricky because detect_pkg_manager uses 'command -v'
    # which checks the real system. We verify the source code instead.
    local pkg_source
    pkg_source="$(sed -n '/^detect_pkg_manager/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$pkg_source" "apt-get" "should check for apt-get" && \
       assert_contains "$pkg_source" "dnf" "should check for dnf" && \
       assert_contains "$pkg_source" "pacman" "should check for pacman" && \
       assert_contains "$pkg_source" "zypper" "should check for zypper" && \
       assert_contains "$pkg_source" "apk" "should check for apk"; then _test_pass; fi

    _setup_mocks
}

# ── 18. detect_chrome ────────────────────────────────────────────────────────
test_detect_chrome() {
    printf "\n${BOLD}--- detect_chrome ---${RESET}\n"

    _test_start "detect_chrome: respects NETWORKER_CHROME_PATH"
    # Create a fake chrome binary
    local fake_chrome="$TEST_TMPDIR/fake-chrome"
    echo '#!/bin/sh' > "$fake_chrome"
    command chmod +x "$fake_chrome"
    NETWORKER_CHROME_PATH="$fake_chrome"

    local result
    result="$(detect_chrome)"
    if assert_eq "$fake_chrome" "$result" "should use NETWORKER_CHROME_PATH"; then _test_pass; fi
    unset NETWORKER_CHROME_PATH

    _test_start "detect_chrome: checks macOS paths"
    local chrome_source
    chrome_source="$(sed -n '/^detect_chrome/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$chrome_source" "/Applications/Google Chrome.app" "should check macOS Chrome path"; then _test_pass; fi

    _test_start "detect_chrome: checks linux command names"
    if assert_contains "$chrome_source" "google-chrome" "should check google-chrome" && \
       assert_contains "$chrome_source" "chromium-browser" "should check chromium-browser" && \
       assert_contains "$chrome_source" "chromium" "should check chromium"; then _test_pass; fi
}

# ── 19. Azure naming helpers ────────────────────────────────────────────────
test_azure_naming() {
    printf "\n${BOLD}--- Azure naming helpers ---${RESET}\n"

    _test_start "_azure_size_slug: Standard_B1s -> b1s"
    local result
    result="$(_azure_size_slug "Standard_B1s")"
    if assert_eq "b1s" "$result" "slug for Standard_B1s"; then _test_pass; fi

    _test_start "_azure_size_slug: Standard_D2s_v3 -> d2sv3"
    result="$(_azure_size_slug "Standard_D2s_v3")"
    if assert_eq "d2sv3" "$result" "slug for Standard_D2s_v3"; then _test_pass; fi

    _test_start "_azure_suggest_name: endpoint linux Standard_B1s eastus"
    result="$(_azure_suggest_name "endpoint" "linux" "Standard_B1s" "eastus")"
    if assert_eq "nwk-ep-lnx-b1s-eastus" "$result" "suggest name for endpoint"; then _test_pass; fi

    _test_start "_azure_suggest_name: tester windows Standard_B2s westeurope"
    result="$(_azure_suggest_name "tester" "windows" "Standard_B2s" "westeurope")"
    if assert_eq "nwk-ts-win-b2s-westeurope" "$result" "suggest name for tester windows"; then _test_pass; fi
}

# ── 20. NSS package name resolution ─────────────────────────────────────────
test_nss_pkg_name() {
    printf "\n${BOLD}--- _nss_pkg_name ---${RESET}\n"

    _test_start "_nss_pkg_name: apt-get -> libnss3-tools"
    PKG_MGR="apt-get"
    local result
    _nss_pkg_name result
    if assert_eq "libnss3-tools" "$result" "nss pkg for apt-get"; then _test_pass; fi

    _test_start "_nss_pkg_name: dnf -> nss-tools"
    PKG_MGR="dnf"
    _nss_pkg_name result
    if assert_eq "nss-tools" "$result" "nss pkg for dnf"; then _test_pass; fi

    _test_start "_nss_pkg_name: pacman -> nss"
    PKG_MGR="pacman"
    _nss_pkg_name result
    if assert_eq "nss" "$result" "nss pkg for pacman"; then _test_pass; fi

    _test_start "_nss_pkg_name: zypper -> mozilla-nss-tools"
    PKG_MGR="zypper"
    _nss_pkg_name result
    if assert_eq "mozilla-nss-tools" "$result" "nss pkg for zypper"; then _test_pass; fi

    _test_start "_nss_pkg_name: apk -> nss-tools"
    PKG_MGR="apk"
    _nss_pkg_name result
    if assert_eq "nss-tools" "$result" "nss pkg for apk"; then _test_pass; fi
}

# ── 21. Feature flags ───────────────────────────────────────────────────────
test_feature_flags() {
    printf "\n${BOLD}--- Feature flags ---${RESET}\n"

    _test_start "Browser feature only added when Chrome is detected and binary is tester"
    # Verified in test_step_cargo_install already
    # Here we verify the source code condition
    local cargo_install_source
    cargo_install_source="$(sed -n '/^step_cargo_install/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$cargo_install_source" 'CHROME_AVAILABLE -eq 1 && "$binary" == "networker-tester"' \
         "feature gate for browser"; then _test_pass; fi
}

# ── 22. -y flag bypasses all interactive prompts ─────────────────────────────
test_auto_yes_bypass() {
    printf "\n${BOLD}--- -y flag bypass ---${RESET}\n"

    _test_start "prompt_main: returns immediately with AUTO_YES=1"
    _reset_install_state
    AUTO_YES=1
    # prompt_main should return 0 without reading anything
    local output
    output="$(prompt_main 2>&1)"
    # Should produce no output (no prompt displayed)
    if assert_eq "" "$output" "prompt_main should produce no output with -y"; then _test_pass; fi

    _test_start "prompt_component_selection: returns immediately with AUTO_YES=1"
    _reset_install_state
    AUTO_YES=1
    output="$(prompt_component_selection 2>&1)"
    if assert_eq "" "$output" "prompt_component_selection should produce no output with -y"; then _test_pass; fi

    _test_start "ask_deployment_locations: returns immediately with AUTO_YES=1"
    _reset_install_state
    AUTO_YES=1
    output="$(ask_deployment_locations 2>&1)"
    if assert_eq "" "$output" "ask_deployment_locations should produce no output with -y"; then _test_pass; fi
}

# ── 23. next_step counter ────────────────────────────────────────────────────
test_next_step() {
    printf "\n${BOLD}--- next_step ---${RESET}\n"

    _test_start "next_step: increments STEP_NUM"
    _reset_install_state
    STEP_NUM=0
    next_step "First step" >/dev/null 2>&1
    if assert_eq "1" "$STEP_NUM" "STEP_NUM after first call"; then _test_pass; fi

    _test_start "next_step: increments on subsequent calls"
    next_step "Second step" >/dev/null 2>&1
    next_step "Third step" >/dev/null 2>&1
    if assert_eq "3" "$STEP_NUM" "STEP_NUM after three calls"; then _test_pass; fi
}

# ── 24. print helpers ────────────────────────────────────────────────────────
test_print_helpers() {
    printf "\n${BOLD}--- print helpers ---${RESET}\n"

    _test_start "print_ok outputs message"
    local output
    output="$(print_ok "test message")"
    if assert_contains "$output" "test message" "print_ok should contain message"; then _test_pass; fi

    _test_start "print_err outputs to stderr"
    output="$(print_err "error msg" 2>&1)"
    if assert_contains "$output" "error msg" "print_err should contain error message"; then _test_pass; fi

    _test_start "print_warn outputs message"
    output="$(print_warn "warning msg")"
    if assert_contains "$output" "warning msg" "print_warn should contain warning"; then _test_pass; fi

    _test_start "print_info outputs message"
    output="$(print_info "info msg")"
    if assert_contains "$output" "info msg" "print_info should contain info"; then _test_pass; fi

    _test_start "print_section outputs section header"
    output="$(print_section "My Section")"
    if assert_contains "$output" "My Section" "print_section should contain section name"; then _test_pass; fi

    _test_start "print_banner outputs banner"
    NETWORKER_VERSION="v0.12.93"
    output="$(print_banner)"
    if assert_contains "$output" "Networker Tester" "print_banner should contain title"; then _test_pass; fi
}

# ── 25. show_help ────────────────────────────────────────────────────────────
test_show_help() {
    printf "\n${BOLD}--- show_help ---${RESET}\n"

    _test_start "show_help: shows usage info"
    local output
    output="$(show_help)"
    if assert_contains "$output" "Usage:" "should contain Usage" && \
       assert_contains "$output" "--azure" "should list --azure" && \
       assert_contains "$output" "--aws" "should list --aws" && \
       assert_contains "$output" "--gcp" "should list --gcp" && \
       assert_contains "$output" "--from-source" "should list --from-source"; then _test_pass; fi
}

# ── 26. _binary_version_ok ──────────────────────────────────────────────────
test_binary_version_ok() {
    printf "\n${BOLD}--- _binary_version_ok ---${RESET}\n"

    _test_start "_binary_version_ok: returns 0 when version matches"
    _reset_install_state
    NETWORKER_VERSION="v0.12.93"
    INSTALL_DIR="$TEST_TMPDIR"

    # Create a fake binary that outputs matching version
    local fake_bin="$TEST_TMPDIR/networker-tester"
    echo '#!/bin/sh' > "$fake_bin"
    echo 'echo "networker-tester 0.12.93"' >> "$fake_bin"
    command chmod +x "$fake_bin"

    _binary_version_ok "networker-tester" >/dev/null 2>&1
    local rc=$?
    if assert_eq "0" "$rc" "should return 0 when version matches"; then _test_pass; fi

    _test_start "_binary_version_ok: returns 1 when version differs"
    echo '#!/bin/sh' > "$fake_bin"
    echo 'echo "networker-tester 0.12.90"' >> "$fake_bin"
    command chmod +x "$fake_bin"

    _binary_version_ok "networker-tester" >/dev/null 2>&1
    rc=$?
    if assert_eq "1" "$rc" "should return 1 when version differs"; then _test_pass; fi

    _test_start "_binary_version_ok: returns 1 when binary missing"
    rm -f "$TEST_TMPDIR/networker-tester"
    _binary_version_ok "networker-tester" >/dev/null 2>&1
    rc=$?
    if assert_eq "1" "$rc" "should return 1 when binary missing"; then _test_pass; fi
}

# ── 27. GCP project resolution ──────────────────────────────────────────────
test_gcp_resolve_project() {
    printf "\n${BOLD}--- _gcp_resolve_project ---${RESET}\n"

    _test_start "_gcp_resolve_project: resolves numeric project to ID"
    _reset_install_state
    GCP_PROJECT="123456789"
    gcloud() {
        MOCK_CALL_COUNT["gcloud"]=$(( ${MOCK_CALL_COUNT["gcloud"]:-0} + 1 ))
        if [[ "$*" == *"projects describe"* ]]; then
            echo "my-project-id"
            return 0
        fi
        return 0
    }

    _gcp_resolve_project >/dev/null 2>&1
    if assert_eq "my-project-id" "$GCP_PROJECT" "should resolve numeric to project ID"; then _test_pass; fi

    _test_start "_gcp_resolve_project: leaves string project unchanged"
    _reset_install_state
    GCP_PROJECT="my-project"
    gcloud() { return 0; }

    _gcp_resolve_project >/dev/null 2>&1
    if assert_eq "my-project" "$GCP_PROJECT" "string project should remain unchanged"; then _test_pass; fi

    _setup_mocks
}

# ── 28. Source guard: main only runs when executed, not sourced ───────────────
test_source_guard() {
    printf "\n${BOLD}--- Source guard ---${RESET}\n"

    _test_start "install.sh has source guard at end"
    local last_lines
    last_lines="$(tail -5 "$INSTALL_SH")"
    if assert_contains "$last_lines" 'BASH_SOURCE' "should check BASH_SOURCE" && \
       assert_contains "$last_lines" 'main "$@"' "should call main"; then _test_pass; fi
}

# ── 29. Repo constants ──────────────────────────────────────────────────────
test_repo_constants() {
    printf "\n${BOLD}--- Repo constants ---${RESET}\n"

    _test_start "REPO_HTTPS is correct"
    if assert_eq "https://github.com/irlm/networker-tester" "$REPO_HTTPS" "REPO_HTTPS"; then _test_pass; fi

    _test_start "REPO_GH is correct"
    if assert_eq "irlm/networker-tester" "$REPO_GH" "REPO_GH"; then _test_pass; fi

    _test_start "INSTALL_DIR defaults to cargo bin"
    if assert_contains "$INSTALL_DIR" ".cargo/bin" "INSTALL_DIR should be in .cargo/bin"; then _test_pass; fi
}

# ── 30. step_ensure_cargo_env ────────────────────────────────────────────────
test_step_ensure_cargo_env() {
    printf "\n${BOLD}--- step_ensure_cargo_env ---${RESET}\n"

    _test_start "step_ensure_cargo_env: checks PATH for .cargo/bin"
    local source_code
    source_code="$(sed -n '/^step_ensure_cargo_env/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$source_code" '.cargo/bin' "should check for .cargo/bin in PATH"; then _test_pass; fi

    _test_start "step_ensure_cargo_env: sources cargo env if exists"
    if assert_contains "$source_code" '.cargo/env' "should source cargo env"; then _test_pass; fi
}

# ── 31. discover_system: GCP deferred check ─────────────────────────────────
test_gcp_deferred_check() {
    printf "\n${BOLD}--- GCP deferred check ---${RESET}\n"

    _test_start "discover_system: does not run gcloud for login during discovery"
    local discover_source
    discover_source="$(sed -n '/^discover_system/,/^}/p' "$INSTALL_SH")"
    # The GCP section should NOT check login status, only binary existence
    if assert_not_contains "$discover_source" "gcloud config get-value account" \
         "discover_system should not check gcloud account"; then _test_pass; fi

    _test_start "discover_system: checks home path for gcloud SDK"
    if assert_contains "$discover_source" "google-cloud-sdk/bin/gcloud" \
         "should check ~/google-cloud-sdk for gcloud"; then _test_pass; fi
}

# ── 32. step_install_rust ────────────────────────────────────────────────────
test_step_install_rust() {
    printf "\n${BOLD}--- step_install_rust (source verification) ---${RESET}\n"

    _test_start "step_install_rust: downloads from sh.rustup.rs"
    local rust_source
    rust_source="$(sed -n '/^step_install_rust/,/^}/p' "$INSTALL_SH")"
    if assert_contains "$rust_source" "sh.rustup.rs" "should download from rustup.rs"; then _test_pass; fi

    _test_start "step_install_rust: uses -y flag for non-interactive"
    if assert_contains "$rust_source" "-- -y" "should pass -y to rustup"; then _test_pass; fi

    _test_start "step_install_rust: sources cargo env after install"
    if assert_contains "$rust_source" '.cargo/env' "should source cargo env after install"; then _test_pass; fi
}

# ── 33. Verify all expected functions exist ──────────────────────────────────
test_function_existence() {
    printf "\n${BOLD}--- Function existence ---${RESET}\n"

    local expected_functions=(
        "parse_args"
        "detect_pkg_manager"
        "detect_chrome"
        "detect_release_target"
        "discover_system"
        "display_system_info"
        "display_plan"
        "prompt_component_selection"
        "prompt_main"
        "ask_yn"
        "customize_flow"
        "next_step"
        "step_download_release"
        "step_install_git"
        "step_install_chrome"
        "step_ensure_certutil"
        "step_install_rust"
        "step_ensure_cargo_env"
        "step_cargo_install"
        "_wait_for_ssh"
        "_remote_install_binary"
        "_remote_create_endpoint_service"
        "step_generate_config"
        "step_check_azure_prereqs"
        "step_azure_create_vm"
        "step_azure_open_endpoint_ports"
        "ensure_azure_cli"
        "ensure_aws_cli"
        "ensure_gcp_cli"
        "_az_do_login"
        "_aws_do_login_sso"
        "_aws_do_login_keys"
        "_aws_check_identity"
        "_gcp_resolve_project"
        "_gcp_create_instance"
        "_gcp_wait_for_ssh"
        "_gcp_ssh_run"
        "_gcp_install_binary"
        "_gcp_create_endpoint_service"
        "step_gcp_deploy_tester"
        "step_gcp_deploy_endpoint"
        "step_aws_deploy_tester"
        "step_aws_deploy_endpoint"
        "display_completion"
        "_cargo_progress"
        "print_ok"
        "print_warn"
        "print_err"
        "print_info"
        "show_help"
        "_nss_pkg_name"
        "_azure_size_slug"
        "_azure_suggest_name"
        "_binary_version_ok"
    )

    for fn in "${expected_functions[@]}"; do
        _test_start "Function exists: $fn"
        if declare -F "$fn" >/dev/null 2>&1; then
            _test_pass
        else
            _test_fail "Function $fn is not defined"
        fi
    done
}

# ══════════════════════════════════════════════════════════════════════════════
# MAIN TEST RUNNER
# ══════════════════════════════════════════════════════════════════════════════

run_all_tests() {
    printf "\n${BOLD}================================================================${RESET}\n"
    printf "${BOLD}  Test Suite: install.sh${RESET}\n"
    printf "${BOLD}================================================================${RESET}\n"

    _setup_tmpdir
    _setup_mocks
    _source_install_sh

    # Run all test groups
    test_detect_release_target
    test_parse_args
    test_discover_system
    test_prompt_component_selection
    test_step_cargo_install
    test_cargo_progress_stdin
    test_tty_reads
    test_cloud_stdin_protection
    test_ssh_stdin_protection
    test_step_download_release
    test_wait_for_ssh
    test_remote_install_binary
    test_remote_create_endpoint_service
    test_vm_existence_checks
    test_ensure_auth
    test_step_generate_config
    test_detect_pkg_manager
    test_detect_chrome
    test_azure_naming
    test_nss_pkg_name
    test_feature_flags
    test_auto_yes_bypass
    test_next_step
    test_print_helpers
    test_show_help
    test_binary_version_ok
    test_gcp_resolve_project
    test_source_guard
    test_repo_constants
    test_step_ensure_cargo_env
    test_gcp_deferred_check
    test_step_install_rust
    test_function_existence

    _teardown_tmpdir

    # Print summary
    printf "\n${BOLD}================================================================${RESET}\n"
    printf "  ${BOLD}Results:${RESET} %d tests run" "$TESTS_RUN"
    if [[ $TESTS_FAILED -eq 0 ]]; then
        printf "  ${GREEN}%d passed${RESET}" "$TESTS_PASSED"
        printf "  ${GREEN}0 failed${RESET}\n"
    else
        printf "  ${GREEN}%d passed${RESET}" "$TESTS_PASSED"
        printf "  ${RED}%d failed${RESET}\n" "$TESTS_FAILED"
    fi
    printf "${BOLD}================================================================${RESET}\n\n"

    if [[ $TESTS_FAILED -gt 0 ]]; then
        return 1
    fi
    return 0
}

run_all_tests
exit $?
