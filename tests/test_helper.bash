#!/usr/bin/env bash
# Common setup for installer bats tests.
#
# Usage in each .bats file:
#   load '../test_helper'
#
# Provides:
#   - SCRIPT: absolute path to install.sh
#   - STUBS_DIR: absolute path to tests/stubs/
#   - setup / teardown helpers

SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/install.sh"
STUBS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/stubs" && pwd)"

# ---------------------------------------------------------------------------
# reset_state — restore every global variable to its default value.
# Call at the start of each test that sources install.sh.
# ---------------------------------------------------------------------------
reset_state() {
    # Core settings
    COMPONENT=""
    AUTO_YES=0
    FROM_SOURCE=0
    SKIP_SSH=0
    SKIP_RUST=0
    INSTALL_METHOD="source"
    RELEASE_AVAILABLE=0
    RELEASE_TARGET=""
    NETWORKER_VERSION=""
    INSTALLER_VERSION="v0.12.70"

    # Flags
    DO_SSH_CHECK=1
    DO_RUST_INSTALL=0
    DO_INSTALL_TESTER=1
    DO_INSTALL_ENDPOINT=1
    DO_GIT_INSTALL=0
    DO_CHROME_INSTALL=0
    DO_REMOTE_TESTER=0
    DO_REMOTE_ENDPOINT=0
    STEP_NUM=0

    # System info
    RUST_VER=""
    RUST_EXISTS=0
    GIT_AVAILABLE=0
    PKG_MGR=""
    CHROME_AVAILABLE=0
    CHROME_PATH=""
    CERTUTIL_AVAILABLE=0
    SYS_OS="Linux"
    SYS_ARCH="x86_64"
    SYS_SHELL="bash"

    # Locations
    TESTER_LOCATION="local"
    ENDPOINT_LOCATION="local"

    # Azure
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

    # AWS
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

    CONFIG_FILE_PATH=""
    INSTALL_DIR="${HOME}/.cargo/bin"

    # Color codes — always empty in tests (stdout is not a TTY)
    BOLD=''; DIM=''; GREEN=''; YELLOW=''; RED=''; CYAN=''; RESET=''
}

# ---------------------------------------------------------------------------
# Mock helpers — redefine interactive / external functions inside the test.
# ---------------------------------------------------------------------------

# ask_yn QUESTION DEFAULT → 0 = yes, 1 = no
# Override with:  ask_yn() { return 0; }   (always yes)
#               ask_yn() { return 1; }   (always no)
mock_ask_yn_yes() { ask_yn() { return 0; }; }
mock_ask_yn_no()  { ask_yn() { return 1; }; }

# Silence next_step so tests don't emit step-counter noise.
mock_next_step() { next_step() { :; }; }

# Silence the cargo-spinner wrapper so tests don't try to build anything.
mock_cargo_progress() { _cargo_progress() { shift; "$@" 2>/dev/null; return 0; }; }

# ---------------------------------------------------------------------------
# Source the installer script (without running main).
# Call once per test file in a top-level `setup_file` if desired,
# or in each `setup` to ensure a clean slate.
# ---------------------------------------------------------------------------
source_installer() {
    # shellcheck disable=SC1090
    source "$SCRIPT"
    # install.sh runs `set -euo pipefail`; relax those for tests so that:
    #  - an empty array expansion doesn't trigger nounset (set -u)
    #  - a non-zero exit from a test helper doesn't abort the whole suite (set -e)
    set +euo pipefail
    reset_state
}

# hide_tester_from_path — remove the stubs/networker-tester binary from PATH
# so that _offer_quick_test sees "no local tester installed".
hide_tester_from_path() {
    # Build a PATH that does not contain STUBS_DIR
    local new_path=""
    local IFS=":"
    for d in $PATH; do
        [[ "$d" == "$STUBS_DIR" ]] && continue
        new_path="${new_path:+${new_path}:}${d}"
    done
    export PATH="$new_path"
    # Also clear INSTALL_DIR so the [[ -x "${INSTALL_DIR}/..." ]] check fails
    INSTALL_DIR="$TEST_TMPDIR/empty-bin"
    mkdir -p "$INSTALL_DIR"
}

# ---------------------------------------------------------------------------
# Prepend the stubs dir to PATH so stub executables shadow real commands.
# ---------------------------------------------------------------------------
use_stubs() {
    export PATH="${STUBS_DIR}:${PATH}"
}
