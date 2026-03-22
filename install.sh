#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – unified installer (rustup-style)
#
# Installs networker-tester and/or networker-endpoint either:
#   locally  – on this machine (release binary download or source compile)
#   remotely – provisioned on a cloud VM (Azure, AWS, and GCP supported)
#
# Two local install modes (auto-detected, or choose in customize flow):
#   release  – download pre-built binary from the latest GitHub release via
#              gh CLI (fast, ~10 s); requires: gh installed + gh auth login
#   source   – compile from source via cargo install (slower, ~5-10 min);
#              requires: Rust/cargo (repo is public — no SSH key needed)
#
# Usage (piped from curl):
#   curl -fsSL <raw-gist-url>/install.sh | bash -s -- [OPTIONS] [tester|endpoint|both]
#
# Usage (downloaded):
#   bash install.sh [OPTIONS] [tester|endpoint|both]
#
# Options:
#   -y, --yes                Non-interactive: accept all defaults (local install)
#   --from-source            Force source-compile mode (skip release download)
#   --skip-ssh-check         Skip the GitHub SSH connectivity test (source mode)
#   --skip-rust              Skip Rust installation (source mode)
#
#   Azure:
#   --azure                  Deploy endpoint to Azure VM (interactive options)
#   --tester-azure           Deploy tester to Azure VM (interactive options)
#   --region REGION          Azure region (default: eastus)
#   --rg NAME                Azure endpoint resource group (default: networker-rg-endpoint)
#   --vm NAME                Azure endpoint VM name (default: networker-endpoint-vm)
#   --tester-rg NAME         Azure tester resource group (default: networker-rg-tester)
#   --tester-vm NAME         Azure tester VM name (default: networker-tester-vm)
#   --vm-size SIZE           Azure VM size (default: Standard_B2s)
#
#   AWS:
#   --aws                    Deploy endpoint to AWS EC2 (interactive options)
#   --tester-aws             Deploy tester to AWS EC2 (interactive options)
#   --aws-region REGION      AWS region (default: us-east-1)
#   --aws-instance-type TYPE EC2 instance type (default: t3.small)
#   --aws-endpoint-name NAME EC2 Name tag for endpoint (default: networker-endpoint)
#   --aws-tester-name NAME   EC2 Name tag for tester (default: networker-tester)
#
#   GCP:
#   --gcp                    Deploy endpoint to GCP GCE (interactive options)
#   --tester-gcp             Deploy tester to GCP GCE (interactive options)
#   --gcp-region REGION      GCP region (default: us-central1)
#   --gcp-zone ZONE          GCP zone (default: us-central1-a)
#   --gcp-machine-type TYPE  GCE machine type (default: e2-small)
#   --gcp-project PROJECT    GCP project ID (default: auto-detected from gcloud config)
#
#   -h, --help               Show this help message
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_HTTPS="https://github.com/irlm/networker-tester"
REPO_GH="irlm/networker-tester"
INSTALL_DIR="${HOME}/.cargo/bin"

# ── Colors (ANSI C quoting; safe even when stdin is a curl pipe) ──────────────
if [ -t 1 ]; then
    BOLD=$'\033[1m'
    DIM=$'\033[2m'
    GREEN=$'\033[0;32m'
    YELLOW=$'\033[1;33m'
    RED=$'\033[0;31m'
    CYAN=$'\033[0;36m'
    RESET=$'\033[0m'
else
    BOLD=''; DIM=''; GREEN=''; YELLOW=''; RED=''; CYAN=''; RESET=''
fi

# ── Print helpers ─────────────────────────────────────────────────────────────
print_ok()   { printf "${GREEN}  ✓${RESET} %s\n" "$*"; }
print_warn() { printf "${YELLOW}  ⚠${RESET} %s\n" "$*"; }
print_err()  { printf "${RED}  ✗${RESET} %s\n" "$*" >&2; }
print_info() { printf "${CYAN}  →${RESET} %s\n" "$*"; }
print_dim()  { printf "${DIM}    %s${RESET}\n" "$*"; }

# Run a command (typically cargo) showing a single updating spinner line instead
# of per-crate output.  Falls back to plain output when stdout is not a TTY.
#
# Usage: _cargo_progress "label" [env VAR=val ...] command [args...]
#
# The spinner line shows what cargo is currently doing (Fetching / Compiling /
# Linking) extracted from the build log.  On failure the last 40 lines of the
# log are printed so the user can see the error.
_cargo_progress() {
    local label="$1"; shift

    local log_file
    log_file="$(mktemp /tmp/networker-cargo-XXXXXX.log)"

    # Non-interactive: just stream output normally
    if [[ ! -t 1 ]]; then
        # set +e so that a cargo failure doesn't trigger errexit before we can
        # capture PIPESTATUS and print a clear error banner.
        set +e
        "$@" </dev/null 2>&1 | tee "$log_file"
        local rc="${PIPESTATUS[0]}"
        set -e
        if [[ $rc -ne 0 ]]; then
            print_err "${label} — build failed (see output above)"
        fi
        rm -f "$log_file"
        return "$rc"
    fi

    # Spinner characters (Unicode braille; falls back to ASCII on dumb terms)
    local spin
    if [[ "${TERM:-}" == "dumb" ]]; then
        spin=('-' '\\' '|' '/')
    else
        spin=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
    fi

    # Launch cargo in background; stdin from /dev/null to prevent hangs
    "$@" </dev/null >"$log_file" 2>&1 &
    local cargo_pid=$!

    tput civis 2>/dev/null || true   # hide cursor

    # Get terminal width reliably even when stdin is a pipe (curl|bash).
    # stty </dev/tty queries the terminal directly regardless of stdin.
    local cols
    cols="$(stty size </dev/tty 2>/dev/null | cut -d' ' -f2 || true)"
    if [[ -z "$cols" || ! "$cols" =~ ^[0-9]+$ || $cols -le 0 ]]; then
        cols="$(tput cols 2>/dev/null || true)"
    fi
    if [[ -z "$cols" || ! "$cols" =~ ^[0-9]+$ || $cols -le 0 ]]; then
        cols="${COLUMNS:-80}"
    fi
    [[ $cols -lt 20 ]] && cols=20

    # Print the initial spinner line (no cursor-up for the first frame).
    printf "  %s  %s  …\n" "${spin[0]}" "$label"
    local si=1
    local prev_count=0

    while kill -0 "$cargo_pid" 2>/dev/null; do
        # Count crates compiled so far
        local compiled_count=0
        compiled_count="$(grep -c ' Compiling ' "$log_file" 2>/dev/null || echo 0)"
        # Strip any non-digit characters (trailing \r, whitespace, etc.)
        compiled_count="${compiled_count//[!0-9]/}"
        compiled_count="${compiled_count:-0}"

        # Only redraw when the compiled crate count changes.
        # On terminals where cursor-up doesn't work (some SSH pseudo-TTYs,
        # curl|bash), each printf creates a new visible line.  By only
        # printing when the count advances, the output stays compact.
        if [[ "$compiled_count" -gt 0 && "$compiled_count" != "$prev_count" ]]; then
            prev_count="$compiled_count"

            # Get the most recent crate being compiled
            local phase=""
            phase="$(grep -oE 'Compiling[[:space:]]+[^[:space:]]+' \
                        "$log_file" 2>/dev/null | tail -1 || true)"
            phase="${phase:-…}"

            # Build the full visible line and hard-limit to cols-1 characters.
            local line="  ${spin[$si]}  ${label}  [${compiled_count} crates]  ${phase}"
            if [[ ${#line} -ge $cols ]]; then
                line="${line:0:$(( cols - 2 ))}…"
            fi

            # Re-split into the unstyled prefix (spinner+label) and the dim suffix
            # (count+phase) based on the known fixed prefix length.
            local pfx_len=$(( 2 + 1 + 2 + ${#label} + 2 ))   # "  X  label  "
            local pfx="${line:0:$pfx_len}"
            local sfx="${line:$pfx_len}"

            # Move up one line, erase it, print the updated spinner frame, then \n
            # so the cursor sits below the spinner line.
            printf "\033[1A\033[2K%s%s%s%s\n" "$pfx" "$DIM" "$sfx" "$RESET"

            si=$(( (si + 1) % ${#spin[@]} ))
        fi
        sleep 0.25
    done

    wait "$cargo_pid"
    local rc=$?

    tput cnorm 2>/dev/null || true   # restore cursor
    printf "\033[1A\033[2K"          # move up and clear spinner line

    if [[ $rc -eq 0 ]]; then
        # Pull crate count and timing from the build log
        local final_count timing=""
        final_count="$(grep -c ' Compiling ' "$log_file" 2>/dev/null || echo 0)"
        final_count="${final_count//[!0-9]/}"
        final_count="${final_count:-0}"
        timing="$(grep -oE 'Finished[^)]+\)' "$log_file" 2>/dev/null | tail -1 || true)"
        local count_str=""
        [[ "$final_count" -gt 0 ]] && count_str="  ${DIM}[${final_count} crates]${RESET}"
        [[ -n "$timing" ]] && timing="  ${DIM}(${timing})${RESET}"
        print_ok "${label}${count_str}${timing}"
    else
        print_err "${label} — build failed"
        echo ""
        echo "  Last 40 lines of build output:"
        tail -40 "$log_file" | sed 's/^/    /'
        echo ""
    fi

    rm -f "$log_file"
    return $rc
}

print_banner() {
    echo ""
    echo "${BOLD}══════════════════════════════════════════════════════════${RESET}"
    if [[ -n "$NETWORKER_VERSION" ]]; then
        printf "${BOLD}      Networker Tester  %-34s${RESET}\n" "$NETWORKER_VERSION"
    else
        printf "${BOLD}      Networker Tester Installer                          ${RESET}\n"
    fi
    echo "${BOLD}══════════════════════════════════════════════════════════${RESET}"
    echo ""
}

print_section() {
    echo ""
    echo "${BOLD}──── $* ────${RESET}"
}

print_step_header() {
    local n="$1"; shift
    echo ""
    echo "${BOLD}Step ${n}: $*${RESET}"
}

show_help() {
    cat <<'EOF'
Usage: install.sh [OPTIONS] [tester|endpoint|dashboard|both]

  tester      Install networker-tester (the diagnostic CLI client)
  endpoint    Install networker-endpoint (the target test server)
  dashboard   Install networker-dashboard (control plane + web UI + agent)
  both        Install tester + endpoint  [default]

Install location (interactive; can also be set via flags):
  local     Install on this machine  [default]
  azure     Provision Azure VM and deploy there (requires az CLI + az login)
  aws       Provision AWS EC2 instance and deploy there (requires aws CLI)
  gcp       Provision GCP GCE instance and deploy there (requires gcloud CLI)

Local install modes (auto-detected; override in customize flow or via flag):
  release   Download pre-built binary via gh CLI — fast (~10 s)
            Requires: gh installed and authenticated (gh auth login)
  source    Compile from GitHub via cargo install — slower (~5-10 min)
            Requires: Rust/cargo (no SSH key needed — repo is public)

Options:
  -y, --yes                Non-interactive: accept all defaults (local install)
  --from-source            Force source-compile mode (skip release detection)
  --skip-rust              Skip Rust installation (source mode only)

Azure options:
  --azure                  Deploy endpoint to Azure (interactive: choose region/size)
  --tester-azure           Deploy tester to Azure (interactive: choose region/size)
  --region REGION          Azure region for all VMs (default: eastus)
  --rg NAME                Endpoint resource group name (default: networker-rg-endpoint)
  --vm NAME                Endpoint VM name (default: networker-endpoint-vm)
  --tester-rg NAME         Tester resource group name (default: networker-rg-tester)
  --tester-vm NAME         Tester VM name (default: networker-tester-vm)
  --vm-size SIZE           Azure VM size for all VMs (default: Standard_B2s)

AWS options:
  --aws                    Deploy endpoint to AWS EC2 (interactive: choose region/type)
  --tester-aws             Deploy tester to AWS EC2 (interactive: choose region/type)
  --aws-region REGION      AWS region (default: us-east-1)
  --aws-instance-type TYPE EC2 instance type (default: t3.small)
  --aws-endpoint-name NAME EC2 Name tag for endpoint instance (default: networker-endpoint)
  --aws-tester-name NAME   EC2 Name tag for tester instance (default: networker-tester)

GCP options:
  --gcp                    Deploy endpoint to GCP GCE (interactive: choose region/type)
  --tester-gcp             Deploy tester to GCP GCE (interactive: choose region/type)
  --gcp-region REGION      GCP region (default: us-central1)
  --gcp-zone ZONE          GCP zone (default: us-central1-a)
  --gcp-machine-type TYPE  GCE machine type (default: e2-small)
  --gcp-project PROJECT    GCP project ID (default: auto-detected from gcloud config)

Config-driven deploy:
  --deploy FILE            Read a JSON config file and deploy/test non-interactively.
                           Validates prereqs, deploys tester + endpoint(s), runs tests.
                           Requires: jq. See docs/deploy-config.md for schema.

  -h, --help               Show this help message

Examples:
  bash install.sh -y endpoint
  bash install.sh --azure endpoint
  bash install.sh --azure --region westeurope both
  bash install.sh --aws endpoint
  bash install.sh --aws --aws-region eu-west-1 both
  bash install.sh --gcp endpoint
  bash install.sh --gcp --gcp-zone europe-west1-b both
  bash install.sh --tester-aws --aws both          # both on separate AWS instances
  bash install.sh --tester-azure --aws both        # tester on Azure, endpoint on AWS
  bash install.sh --tester-gcp --gcp both          # both on separate GCP instances
  bash install.sh dashboard                        # install dashboard + PostgreSQL + frontend
  bash install.sh --deploy deploy.json             # config-driven deploy + test
EOF
}

# ── Script-level state ────────────────────────────────────────────────────────
COMPONENT=""   # "" = not set via CLI; "tester" | "endpoint" | "dashboard" | "both" = explicit
AUTO_YES=0
FROM_SOURCE=0
SKIP_RUST=0
SKIP_SERVICE=0

INSTALL_METHOD="source"   # "release" | "source"
RELEASE_AVAILABLE=0
RELEASE_TARGET=""
NETWORKER_VERSION=""      # populated in discover_system (gh query or fallback below)
INSTALLER_VERSION="v0.14.4"  # fallback when gh is unavailable

DO_RUST_INSTALL=0
DO_INSTALL_TESTER=1
DO_INSTALL_ENDPOINT=1
DO_INSTALL_DASHBOARD=0
DASHBOARD_FQDN=""
DASHBOARD_NGINX_CONFIGURED=0
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

# ── Remote deployment state ───────────────────────────────────────────────────
TESTER_LOCATION="local"       # "local" | "azure" | "aws" | "gcp" | "lan"
ENDPOINT_LOCATION="local"     # "local" | "azure" | "aws" | "gcp" | "lan"
DO_REMOTE_TESTER=0
DO_REMOTE_ENDPOINT=0

# ── LAN state ────────────────────────────────────────────────────────────────
LAN_TESTER_IP=""
LAN_TESTER_USER=""
LAN_TESTER_PORT="22"
LAN_TESTER_OS=""              # auto-detected: "linux" | "windows"

LAN_ENDPOINT_IP=""
LAN_ENDPOINT_USER=""
LAN_ENDPOINT_PORT="22"
LAN_ENDPOINT_OS=""            # auto-detected: "linux" | "windows"

# ── Azure state ───────────────────────────────────────────────────────────────
AZURE_CLI_AVAILABLE=0
AZURE_LOGGED_IN=0
AZURE_REGION="eastus"
AZURE_REGION_ASKED=0

AZURE_TESTER_RG="networker-rg-tester"
AZURE_TESTER_VM="networker-tester-vm"
AZURE_TESTER_SIZE="Standard_B2s"
AZURE_TESTER_OS="linux"     # "linux" | "windows"
AZURE_TESTER_IP=""

AZURE_ENDPOINT_RG="networker-rg-endpoint"
AZURE_ENDPOINT_VM="networker-endpoint-vm"
AZURE_ENDPOINT_SIZE="Standard_B2s"
AZURE_ENDPOINT_OS="linux"   # "linux" | "windows"
AZURE_ENDPOINT_IP=""

# Auto-shutdown: "yes" = set Azure auto-shutdown + AWS cron at 04:00 UTC (11 PM EST)
AZURE_AUTO_SHUTDOWN="yes"
AZURE_SHUTDOWN_ASKED=0

# Extra endpoint IPs for multi-region comparison (array of "ip:region" pairs)
AZURE_EXTRA_ENDPOINT_IPS=()

# ── AWS state ─────────────────────────────────────────────────────────────────
AWS_CLI_AVAILABLE=0
AWS_LOGGED_IN=0
AWS_REGION="us-east-1"
AWS_REGION_ASKED=0

AWS_TESTER_NAME="networker-tester"
AWS_TESTER_INSTANCE_TYPE="t3.small"
AWS_TESTER_OS="linux"       # "linux" | "windows"
AWS_TESTER_INSTANCE_ID=""
AWS_TESTER_IP=""

AWS_ENDPOINT_NAME="networker-endpoint"
AWS_ENDPOINT_INSTANCE_TYPE="t3.small"
AWS_ENDPOINT_OS="linux"     # "linux" | "windows"
AWS_ENDPOINT_INSTANCE_ID=""
AWS_ENDPOINT_IP=""

# Auto-shutdown: "yes" = install cron job at 04:00 UTC (11 PM EST)
AWS_AUTO_SHUTDOWN="yes"
AWS_SHUTDOWN_ASKED=0

# ── GCP state ────────────────────────────────────────────────────────────────
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

# Auto-shutdown: "yes" = install cron job at 04:00 UTC (11 PM EST)
GCP_AUTO_SHUTDOWN="yes"
GCP_SHUTDOWN_ASKED=0

CONFIG_FILE_PATH=""

# ── Deploy-config state ──────────────────────────────────────────────────
DEPLOY_CONFIG_PATH=""           # path to deploy.json (--deploy flag)
DEPLOY_ENDPOINT_COUNT=0         # number of endpoints in config
DEPLOY_RUN_TESTS=1              # 1 = run tests after deploy
DEPLOY_TEST_MODES=""            # JSON array string of test modes
DEPLOY_TEST_RUNS=""             # number of test runs
DEPLOY_TEST_PAYLOAD_SIZES=""    # JSON array string of payload sizes
DEPLOY_TEST_INSECURE=""         # "true" or "false"
DEPLOY_TEST_CONNECTION_REUSE="" # "true" or "false"
DEPLOY_TEST_UDP_PORT=""
DEPLOY_TEST_UDP_THROUGHPUT_PORT=""
DEPLOY_TEST_PAGE_ASSETS=""
DEPLOY_TEST_PAGE_ASSET_SIZE=""
DEPLOY_TEST_PAGE_PRESET=""
DEPLOY_TEST_TIMEOUT=""
DEPLOY_TEST_RETRIES=""
DEPLOY_TEST_HTML_REPORT=""
DEPLOY_TEST_OUTPUT_DIR=""
DEPLOY_TEST_EXCEL=""
DEPLOY_TEST_CONCURRENCY=""
DEPLOY_TEST_DNS_ENABLED=""
DEPLOY_TEST_IPV4_ONLY=""
DEPLOY_TEST_IPV6_ONLY=""
DEPLOY_TEST_VERBOSE=""
DEPLOY_TEST_LOG_LEVEL=""
DEPLOY_PACKET_CAPTURE_MODE="none"
DEPLOY_PACKET_CAPTURE_INSTALL_REQS=""
DEPLOY_PACKET_CAPTURE_INTERFACE=""
DEPLOY_PACKET_CAPTURE_WRITE_PCAP=""
DEPLOY_PACKET_CAPTURE_WRITE_SUMMARY_JSON=""

# Arrays for multi-endpoint support (parallel arrays indexed 0..N-1)
DEPLOY_EP_PROVIDERS=()
DEPLOY_EP_LABELS=()
DEPLOY_EP_IPS=()               # populated after deploy (result IPs)
DEPLOY_EP_FQDNS=()             # populated after deploy (cloud DNS hostnames)

# ── Argument parsing ──────────────────────────────────────────────────────────
parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            tester|endpoint|dashboard|both)
                COMPONENT="$1" ;;
            -y|--yes)
                AUTO_YES=1 ;;
            --from-source)
                FROM_SOURCE=1 ;;
            --skip-rust)
                SKIP_RUST=1 ;;
            --no-service)
                SKIP_SERVICE=1 ;;
            # Azure
            --azure)
                ENDPOINT_LOCATION="azure"; DO_REMOTE_ENDPOINT=1 ;;
            --tester-azure)
                TESTER_LOCATION="azure"; DO_REMOTE_TESTER=1 ;;
            --region)
                shift; AZURE_REGION="${1:-eastus}" ;;
            --rg)
                shift; AZURE_ENDPOINT_RG="${1:-networker-rg-endpoint}" ;;
            --vm)
                shift; AZURE_ENDPOINT_VM="${1:-networker-endpoint-vm}" ;;
            --tester-rg)
                shift; AZURE_TESTER_RG="${1:-networker-rg-tester}" ;;
            --tester-vm)
                shift; AZURE_TESTER_VM="${1:-networker-tester-vm}" ;;
            --vm-size)
                shift
                AZURE_TESTER_SIZE="${1:-Standard_B2s}"
                AZURE_ENDPOINT_SIZE="${1:-Standard_B2s}" ;;
            # AWS
            --aws)
                ENDPOINT_LOCATION="aws"; DO_REMOTE_ENDPOINT=1 ;;
            --tester-aws)
                TESTER_LOCATION="aws"; DO_REMOTE_TESTER=1 ;;
            --aws-region)
                shift; AWS_REGION="${1:-us-east-1}" ;;
            --aws-instance-type)
                shift
                AWS_TESTER_INSTANCE_TYPE="${1:-t3.small}"
                AWS_ENDPOINT_INSTANCE_TYPE="${1:-t3.small}" ;;
            --aws-endpoint-name)
                shift; AWS_ENDPOINT_NAME="${1:-networker-endpoint}" ;;
            --aws-tester-name)
                shift; AWS_TESTER_NAME="${1:-networker-tester}" ;;
            # GCP
            --gcp)
                ENDPOINT_LOCATION="gcp"; DO_REMOTE_ENDPOINT=1 ;;
            --tester-gcp)
                TESTER_LOCATION="gcp"; DO_REMOTE_TESTER=1 ;;
            --gcp-region)
                shift; GCP_REGION="${1:-us-central1}" ;;
            --gcp-zone)
                shift; GCP_ZONE="${1:-us-central1-a}" ;;
            --gcp-machine-type)
                shift
                GCP_TESTER_MACHINE_TYPE="${1:-e2-small}"
                GCP_ENDPOINT_MACHINE_TYPE="${1:-e2-small}" ;;
            --gcp-project)
                shift; GCP_PROJECT="${1:-}" ;;
            # LAN
            --lan)
                ENDPOINT_LOCATION="lan"; DO_REMOTE_ENDPOINT=1 ;;
            --tester-lan)
                TESTER_LOCATION="lan"; DO_REMOTE_TESTER=1 ;;
            --lan-ip)
                shift
                LAN_TESTER_IP="${1:-}"
                LAN_ENDPOINT_IP="${1:-}" ;;
            --lan-user)
                shift
                LAN_TESTER_USER="${1:-}"
                LAN_ENDPOINT_USER="${1:-}" ;;
            --lan-port)
                shift
                LAN_TESTER_PORT="${1:-22}"
                LAN_ENDPOINT_PORT="${1:-22}" ;;
            --lan-tester-ip)
                shift; LAN_TESTER_IP="${1:-}" ;;
            --lan-tester-user)
                shift; LAN_TESTER_USER="${1:-}" ;;
            --lan-endpoint-ip)
                shift; LAN_ENDPOINT_IP="${1:-}" ;;
            --lan-endpoint-user)
                shift; LAN_ENDPOINT_USER="${1:-}" ;;
            # Deploy config
            --deploy)
                shift; DEPLOY_CONFIG_PATH="${1:-}"
                AUTO_YES=1 ;;
            -h|--help)
                show_help; exit 0 ;;
            *)
                print_err "Unknown option: $1"
                echo ""
                show_help
                exit 1
                ;;
        esac
        shift
    done

    case "$COMPONENT" in
        tester)    DO_INSTALL_ENDPOINT=0 ;;
        endpoint)  DO_INSTALL_TESTER=0   ;;
        dashboard) DO_INSTALL_TESTER=0; DO_INSTALL_ENDPOINT=0; DO_INSTALL_DASHBOARD=1 ;;
        both)      ;;
    esac


    # If a cloud flag was set but matching component wasn't enabled, enable it
    if [[ $DO_REMOTE_ENDPOINT -eq 1 && $DO_INSTALL_ENDPOINT -eq 0 ]]; then
        DO_INSTALL_ENDPOINT=1
    fi
    if [[ $DO_REMOTE_TESTER -eq 1 && $DO_INSTALL_TESTER -eq 0 ]]; then
        DO_INSTALL_TESTER=1
    fi
}

# ── Package manager detection ─────────────────────────────────────────────────
detect_pkg_manager() {
    local os
    os="$(uname -s 2>/dev/null || echo "")"
    case "$os" in
        Darwin)
            if command -v brew &>/dev/null; then echo "brew"; fi
            ;;
        Linux)
            if   command -v apt-get &>/dev/null; then echo "apt-get"
            elif command -v dnf     &>/dev/null; then echo "dnf"
            elif command -v pacman  &>/dev/null; then echo "pacman"
            elif command -v zypper  &>/dev/null; then echo "zypper"
            elif command -v apk     &>/dev/null; then echo "apk"
            fi
            ;;
    esac
}

# ── Chrome/Chromium detection ─────────────────────────────────────────────────
detect_chrome() {
    if [[ -n "${NETWORKER_CHROME_PATH:-}" && -x "${NETWORKER_CHROME_PATH}" ]]; then
        echo "$NETWORKER_CHROME_PATH"; return
    fi
    local mac_paths=(
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        "/Applications/Chromium.app/Contents/MacOS/Chromium"
    )
    for p in "${mac_paths[@]}"; do
        [[ -x "$p" ]] && echo "$p" && return
    done
    local cmd
    for cmd in google-chrome google-chrome-stable chromium-browser chromium; do
        if command -v "$cmd" &>/dev/null; then command -v "$cmd"; return; fi
    done
}

# Check whether Chrome/Chromium is installed on a remote VM via SSH.
# Returns 0 (found) or 1 (not found).
_remote_chrome_available() {
    local ip="$1" user="$2"
    ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "${user}@${ip}" \
        'command -v google-chrome google-chrome-stable chromium-browser chromium chromium-browser-stable 2>/dev/null | head -1 | grep -q . || test -x "$HOME/.local/bin/google-chrome"' 2>/dev/null
}

# ── Target triple detection ───────────────────────────────────────────────────
detect_release_target() {
    local os arch
    os="$(uname -s 2>/dev/null || echo "")"
    arch="$(uname -m 2>/dev/null || echo "")"

    case "$os" in
        Linux)
            case "$arch" in
                x86_64) echo "x86_64-unknown-linux-musl" ;;
                *)      echo "" ;;
            esac ;;
        Darwin)
            case "$arch" in
                x86_64) echo "x86_64-apple-darwin" ;;
                arm64)  echo "aarch64-apple-darwin" ;;
                *)      echo "" ;;
            esac ;;
        *)  echo "" ;;
    esac
}

# ── System discovery ──────────────────────────────────────────────────────────
discover_system() {
    SYS_OS="$(uname -s 2>/dev/null || echo "unknown")"
    SYS_ARCH="$(uname -m 2>/dev/null || echo "unknown")"
    SYS_SHELL="${SHELL:-unknown}"

    if command -v cargo &>/dev/null; then
        RUST_VER="$(rustc --version 2>/dev/null || echo "unknown version")"
        RUST_EXISTS=1
    else
        RUST_VER="not installed"
        RUST_EXISTS=0
    fi

    if [[ $RUST_EXISTS -eq 0 && $SKIP_RUST -eq 0 ]]; then
        DO_RUST_INSTALL=1
    fi

    PKG_MGR="$(detect_pkg_manager)"

    if command -v git &>/dev/null; then
        GIT_AVAILABLE=1
    else
        GIT_AVAILABLE=0
    fi

    if [[ $FROM_SOURCE -eq 0 ]]; then
        RELEASE_TARGET="$(detect_release_target)"
        if [[ -n "$RELEASE_TARGET" ]]; then
            if command -v gh &>/dev/null \
               && gh auth status &>/dev/null 2>&1 </dev/null; then
                RELEASE_AVAILABLE=1
                INSTALL_METHOD="release"
                NETWORKER_VERSION="$(gh release list --repo "$REPO_GH" \
                    --limit 1 --json tagName -q '.[0].tagName' 2>/dev/null </dev/null || echo "")"
            elif command -v curl &>/dev/null; then
                # No gh CLI — try to resolve latest release via GitHub API (unauthenticated)
                local api_tag=""
                api_tag="$(curl -fsSL --connect-timeout 5 \
                    "https://api.github.com/repos/${REPO_GH}/releases/latest" 2>/dev/null </dev/null \
                    | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
                    | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/' || echo "")"
                if [[ -n "$api_tag" ]]; then
                    RELEASE_AVAILABLE=1
                    INSTALL_METHOD="release"
                    NETWORKER_VERSION="$api_tag"
                fi
            fi
        fi
    fi

    if [[ "$INSTALL_METHOD" == "source" && $GIT_AVAILABLE -eq 0 && -n "$PKG_MGR" ]]; then
        DO_GIT_INSTALL=1
    fi

    CHROME_PATH="$(detect_chrome)"
    if [[ -n "$CHROME_PATH" ]]; then
        CHROME_AVAILABLE=1
    else
        CHROME_AVAILABLE=0
    fi

    if command -v certutil &>/dev/null; then
        CERTUTIL_AVAILABLE=1
    fi

    # Always try to resolve the latest release version for the banner, even in
    # source mode — requires gh installed and authenticated, fails silently if not.
    if [[ -z "$NETWORKER_VERSION" ]] && command -v gh &>/dev/null; then
        NETWORKER_VERSION="$(gh release list --repo "$REPO_GH" \
            --limit 1 --json tagName -q '.[0].tagName' 2>/dev/null </dev/null || echo "")"
    fi
    # Fallback: use the version embedded in this installer script
    if [[ -z "$NETWORKER_VERSION" ]]; then
        NETWORKER_VERSION="$INSTALLER_VERSION"
    fi

    # Azure CLI detection
    if command -v az &>/dev/null; then
        AZURE_CLI_AVAILABLE=1
        if az account show &>/dev/null 2>&1 </dev/null; then
            AZURE_LOGGED_IN=1
        fi
    fi

    # AWS CLI detection
    if command -v aws &>/dev/null; then
        AWS_CLI_AVAILABLE=1
        if aws sts get-caller-identity &>/dev/null 2>&1 </dev/null; then
            AWS_LOGGED_IN=1
        fi
    fi

    # GCP CLI detection — only check if binary exists, don't run it.
    # Running gcloud (Python) during discover_system can consume stdin
    # in curl|bash mode. Login/project status is checked later in
    # ensure_gcp_cli / step_check_gcp_prereqs when GCP is actually needed.
    if ! command -v gcloud &>/dev/null; then
        if [[ -x "${HOME}/google-cloud-sdk/bin/gcloud" ]]; then
            export PATH="${HOME}/google-cloud-sdk/bin:${PATH}"
        elif [[ -x "/opt/homebrew/share/google-cloud-sdk/bin/gcloud" ]]; then
            export PATH="/opt/homebrew/share/google-cloud-sdk/bin:${PATH}"
        elif [[ -x "/usr/local/share/google-cloud-sdk/bin/gcloud" ]]; then
            export PATH="/usr/local/share/google-cloud-sdk/bin:${PATH}"
        fi
    fi
    if command -v gcloud &>/dev/null; then
        GCP_CLI_AVAILABLE=1
    fi
}

display_system_info() {
    print_section "System Information"
    echo ""
    printf "    %-22s %s\n" "OS:"           "$SYS_OS"
    printf "    %-22s %s\n" "Architecture:" "$SYS_ARCH"
    printf "    %-22s %s\n" "Shell:"        "$SYS_SHELL"
    printf "    %-22s %s\n" "Home:"         "$HOME"
    printf "    %-22s %s\n" "Rust / cargo:" "$RUST_VER"
    if [[ $GIT_AVAILABLE -eq 1 ]]; then
        printf "    %-22s %s\n" "git:" "$(git --version 2>/dev/null)"
    else
        printf "    %-22s %s\n" "git:" "not installed"
    fi
    if [[ $CHROME_AVAILABLE -eq 1 ]]; then
        printf "    %-22s %s\n" "Chrome/Chromium:" "installed ✓"
    else
        printf "    %-22s %s\n" "Chrome/Chromium:" "not installed  (browser probe disabled)"
    fi
    printf "    %-22s %s\n" "Install to:"   "${INSTALL_DIR}/"

    if [[ $RELEASE_AVAILABLE -eq 1 ]]; then
        printf "    %-22s %s\n" "gh CLI:" "authenticated ✓"
    fi

    if [[ $AZURE_CLI_AVAILABLE -eq 1 ]]; then
        if [[ $AZURE_LOGGED_IN -eq 1 ]]; then
            local az_sub
            az_sub="$(az account show --query name -o tsv 2>/dev/null || echo "")"
            printf "    %-22s %s\n" "Azure CLI:" "authenticated ✓  (${az_sub})"
        else
            printf "    %-22s %s\n" "Azure CLI:" "installed  (run: az login)"
        fi
    fi

    if [[ $AWS_CLI_AVAILABLE -eq 1 ]]; then
        if [[ $AWS_LOGGED_IN -eq 1 ]]; then
            local aws_account
            aws_account="$(aws sts get-caller-identity --query Account --output text 2>/dev/null || echo "")"
            printf "    %-22s %s\n" "AWS CLI:" "authenticated ✓  (account: ${aws_account})"
        else
            printf "    %-22s %s\n" "AWS CLI:" "installed  (run: aws sso login / aws configure)"
        fi
    fi

    if [[ $GCP_CLI_AVAILABLE -eq 1 ]]; then
        printf "    %-22s %s\n" "GCP CLI:" "installed ✓"
    fi
}

display_plan() {
    print_section "Installation Plan"
    echo ""

    # ── Local install plan ────────────────────────────────────────────────────
    local do_local_tester=0
    local do_local_endpoint=0
    [[ $DO_INSTALL_TESTER -eq 1 && $DO_REMOTE_TESTER -eq 0 ]]     && do_local_tester=1
    [[ $DO_INSTALL_ENDPOINT -eq 1 && $DO_REMOTE_ENDPOINT -eq 0 ]]  && do_local_endpoint=1

    if [[ $do_local_tester -eq 1 || $do_local_endpoint -eq 1 ]]; then
        if [[ "$INSTALL_METHOD" == "release" ]]; then
            printf "    ${BOLD}Local method:${RESET}  Download binary from GitHub release  ${DIM}(fast)${RESET}\n"
            printf "    ${DIM}Target:        %s${RESET}\n" "$RELEASE_TARGET"
            echo ""

            local step=1
            local ver_label="${NETWORKER_VERSION:-latest}"
            if [[ $do_local_tester -eq 1 ]]; then
                printf "    %s. ${BOLD}Download networker-tester${RESET}    %s\n" "$step" "$ver_label"
                step=$((step + 1))
            fi
            if [[ $do_local_endpoint -eq 1 ]]; then
                printf "    %s. ${BOLD}Download networker-endpoint${RESET}  %s\n" "$step" "$ver_label"
                step=$((step + 1))
            fi
            if [[ "$SYS_OS" == "Linux" && $CERTUTIL_AVAILABLE -eq 0 && -n "$PKG_MGR" \
                  && ( $CHROME_AVAILABLE -eq 1 || $DO_CHROME_INSTALL -eq 1 ) ]]; then
                local nss_pkg; _nss_pkg_name nss_pkg
                printf "    %s. ${BOLD}Install certutil${RESET}        %s via %s ${DIM}(browser3 QUIC cert trust)${RESET}\n" \
                    "$step" "$nss_pkg" "$PKG_MGR"
            fi
            echo ""
            print_dim "Repository:  $REPO_GH  (${NETWORKER_VERSION:-latest release})"
        else
            if [[ -n "$RELEASE_TARGET" ]]; then
                printf "    ${BOLD}Local method:${RESET}  Download binary or compile from source\n"
            else
                printf "    ${BOLD}Local method:${RESET}  Compile from source  ${DIM}(~5-10 min)${RESET}\n"
            fi
            echo ""
            local step=1

            if [[ $GIT_AVAILABLE -eq 0 ]]; then
                if [[ $DO_GIT_INSTALL -eq 1 ]]; then
                    printf "    %s. ${BOLD}Install git${RESET}            Install via %s\n" "$step" "$PKG_MGR"
                    step=$((step + 1))
                elif [[ -n "$PKG_MGR" ]]; then
                    printf "    ${DIM}-. Install git              (skip – toggle in Customize)${RESET}\n"
                else
                    printf "    ${DIM}-. Install git              (not installed – visit https://git-scm.com/)${RESET}\n"
                fi
            fi

            if [[ $CHROME_AVAILABLE -eq 0 && $do_local_tester -eq 1 ]]; then
                if [[ $DO_CHROME_INSTALL -eq 1 ]]; then
                    printf "    %s. ${BOLD}Install Chrome${RESET}         Install via %s (browser probe)\n" "$step" "$PKG_MGR"
                    step=$((step + 1))
                elif [[ -n "$PKG_MGR" ]]; then
                    printf "    ${DIM}-. Install Chrome         (will ask — browser probe disabled if skipped)${RESET}\n"
                else
                    printf "    ${DIM}-. Install Chrome         (not installed — https://www.google.com/chrome/)${RESET}\n"
                fi
            fi

            if [[ $DO_RUST_INSTALL -eq 1 ]]; then
                printf "    %s. ${BOLD}Install Rust${RESET}           Download rustup and run installer\n" "$step"
                step=$((step + 1))
            elif [[ $RUST_EXISTS -eq 0 ]]; then
                printf "    ${DIM}-. Install Rust            (skipped – --skip-rust)${RESET}\n"
            else
                printf "    ${DIM}-. Install Rust            (skip – already installed: %s)${RESET}\n" "$RUST_VER"
            fi

            local browser_note=""
            if [[ $CHROME_AVAILABLE -eq 1 || $DO_CHROME_INSTALL -eq 1 ]]; then
                browser_note="  ${DIM}[+browser feature]${RESET}"
            fi
            if [[ $do_local_tester -eq 1 ]]; then
                printf "    %s. ${BOLD}Install networker-tester${RESET}   cargo install from GitHub%s\n" "$step" "$browser_note"
                step=$((step + 1))
            fi
            if [[ $do_local_endpoint -eq 1 ]]; then
                printf "    %s. ${BOLD}Install networker-endpoint${RESET} cargo install from GitHub\n" "$step"
                step=$((step + 1))
            fi

            if [[ "$SYS_OS" == "Linux" && $CERTUTIL_AVAILABLE -eq 0 && -n "$PKG_MGR" \
                  && ( $CHROME_AVAILABLE -eq 1 || $DO_CHROME_INSTALL -eq 1 ) ]]; then
                local nss_pkg; _nss_pkg_name nss_pkg
                printf "    %s. ${BOLD}Install certutil${RESET}        %s via %s ${DIM}(browser3 QUIC cert trust)${RESET}\n" \
                    "$step" "$nss_pkg" "$PKG_MGR"
                step=$((step + 1))
            fi
            echo ""
            print_dim "Repository:  $REPO_HTTPS"
            print_dim "Source code is compiled locally — no pre-built binaries are downloaded."
        fi
    fi

    # ── Dashboard ─────────────────────────────────────────────────────────────
    if [[ $DO_INSTALL_DASHBOARD -eq 1 ]]; then
        echo ""
        printf "    ${BOLD}networker-dashboard:${RESET}  Local install\n"
        printf "    %-22s %s\n" "PostgreSQL:"  "auto-install + configure"
        printf "    %-22s %s\n" "Frontend:"    "Node.js build → /opt/networker/dashboard"
        printf "    %-22s %s\n" "Service:"     "systemd (networker-dashboard.service)"
        printf "    %-22s %s\n" "Port:"        "${DASHBOARD_PORT:-3000}"
    fi

    # ── Remote — tester ───────────────────────────────────────────────────────
    if [[ $DO_REMOTE_TESTER -eq 1 ]]; then
        echo ""
        case "$TESTER_LOCATION" in
            lan)
                printf "    ${BOLD}networker-tester:${RESET}  Remote — LAN (%s)\n" "$LAN_TESTER_OS"
                printf "    %-22s %s\n" "Host:"       "$LAN_TESTER_IP"
                printf "    %-22s %s\n" "User:"       "$LAN_TESTER_USER"
                printf "    %-22s %s\n" "SSH port:"   "$LAN_TESTER_PORT"
                ;;
            azure)
                printf "    ${BOLD}networker-tester:${RESET}  Remote — Azure VM\n"
                printf "    %-22s %s\n" "Region:"         "$AZURE_REGION"
                printf "    %-22s %s\n" "Resource group:" "$AZURE_TESTER_RG"
                printf "    %-22s %s\n" "VM name:"        "$AZURE_TESTER_VM"
                printf "    %-22s %s\n" "VM size:"        "$AZURE_TESTER_SIZE"
                ;;
            aws)
                printf "    ${BOLD}networker-tester:${RESET}  Remote — AWS EC2\n"
                printf "    %-22s %s\n" "Region:"         "$AWS_REGION"
                printf "    %-22s %s\n" "Instance name:"  "$AWS_TESTER_NAME"
                printf "    %-22s %s\n" "Instance type:"  "$AWS_TESTER_INSTANCE_TYPE"
                ;;
            gcp)
                printf "    ${BOLD}networker-tester:${RESET}  Remote — GCP GCE\n"
                printf "    %-22s %s\n" "Zone:"           "$GCP_ZONE"
                printf "    %-22s %s\n" "Instance name:"  "$GCP_TESTER_NAME"
                printf "    %-22s %s\n" "Machine type:"   "$GCP_TESTER_MACHINE_TYPE"
                printf "    %-22s %s\n" "Project:"        "$GCP_PROJECT"
                ;;
        esac
        echo ""
        printf "    ${DIM}a. Provision VM (Ubuntu 22.04)\n"
        printf "    b. Install networker-tester binary\n"
        printf "    c. Show SSH access command${RESET}\n"
    fi

    # ── Remote — endpoint ─────────────────────────────────────────────────────
    if [[ $DO_REMOTE_ENDPOINT -eq 1 ]]; then
        echo ""
        case "$ENDPOINT_LOCATION" in
            lan)
                printf "    ${BOLD}networker-endpoint:${RESET}  Remote — LAN (%s)\n" "$LAN_ENDPOINT_OS"
                printf "    %-22s %s\n" "Host:"       "$LAN_ENDPOINT_IP"
                printf "    %-22s %s\n" "User:"       "$LAN_ENDPOINT_USER"
                printf "    %-22s %s\n" "SSH port:"   "$LAN_ENDPOINT_PORT"
                ;;
            azure)
                printf "    ${BOLD}networker-endpoint:${RESET}  Remote — Azure VM\n"
                printf "    %-22s %s\n" "Region:"         "$AZURE_REGION"
                printf "    %-22s %s\n" "Resource group:" "$AZURE_ENDPOINT_RG"
                printf "    %-22s %s\n" "VM name:"        "$AZURE_ENDPOINT_VM"
                printf "    %-22s %s\n" "VM size:"        "$AZURE_ENDPOINT_SIZE"
                ;;
            aws)
                printf "    ${BOLD}networker-endpoint:${RESET}  Remote — AWS EC2\n"
                printf "    %-22s %s\n" "Region:"         "$AWS_REGION"
                printf "    %-22s %s\n" "Instance name:"  "$AWS_ENDPOINT_NAME"
                printf "    %-22s %s\n" "Instance type:"  "$AWS_ENDPOINT_INSTANCE_TYPE"
                ;;
            gcp)
                printf "    ${BOLD}networker-endpoint:${RESET}  Remote — GCP GCE\n"
                printf "    %-22s %s\n" "Zone:"           "$GCP_ZONE"
                printf "    %-22s %s\n" "Instance name:"  "$GCP_ENDPOINT_NAME"
                printf "    %-22s %s\n" "Machine type:"   "$GCP_ENDPOINT_MACHINE_TYPE"
                printf "    %-22s %s\n" "Project:"        "$GCP_PROJECT"
                ;;
        esac
        echo ""
        printf "    ${DIM}a. Provision VM + open TCP 80, 443, 8080, 8443 and UDP 8443, 9998, 9999\n"
        printf "    b. Install networker-endpoint + systemd service\n"
        printf "    c. Verify /health endpoint\n"
        printf "    d. Write networker-cloud.json config file${RESET}\n"
    fi
}

# ── Helper: resolve NSS package name for current distro ──────────────────────
_nss_pkg_name() {
    local varname="$1"
    local pkg
    case "$PKG_MGR" in
        apt-get) pkg="libnss3-tools" ;;
        dnf)     pkg="nss-tools" ;;
        pacman)  pkg="nss" ;;
        zypper)  pkg="mozilla-nss-tools" ;;
        apk)     pkg="nss-tools" ;;
        *)       pkg="nss-tools" ;;
    esac
    printf -v "$varname" "%s" "$pkg"
}

# ── LAN deployment ────────────────────────────────────────────────────────────

# Build SSH options for a LAN target.
# $1 = role ("tester"|"endpoint")  →  sets: _LAN_SSH_OPTS (array), _LAN_SCP_OPTS (array)
_lan_ssh_vars() {
    local role="$1"
    local ip user port
    if [[ "$role" == "tester" ]]; then
        ip="$LAN_TESTER_IP"; user="$LAN_TESTER_USER"; port="$LAN_TESTER_PORT"
    else
        ip="$LAN_ENDPOINT_IP"; user="$LAN_ENDPOINT_USER"; port="$LAN_ENDPOINT_PORT"
    fi
    _LAN_IP="$ip"
    _LAN_USER="$user"
    _LAN_PORT="$port"
    _LAN_SSH_OPTS=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -p "$port")
    _LAN_SCP_OPTS=(-o StrictHostKeyChecking=accept-new -P "$port" -q)
    _LAN_DEST="${user}@${ip}"
}

# Test SSH connectivity to a LAN host.  Exits with helpful message on failure.
_lan_test_ssh() {
    local ip="$1" user="$2" port="$3"

    print_info "Testing SSH connection to ${user}@${ip}:${port}…"
    if ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 -o BatchMode=yes \
           -p "$port" "${user}@${ip}" "echo ok" &>/dev/null; then
        print_ok "SSH connection successful"
        return 0
    fi

    echo ""
    print_err "SSH connection to ${user}@${ip}:${port} failed."
    echo ""
    echo "  ${BOLD}Troubleshooting steps:${RESET}"
    echo ""
    echo "  1. Verify the machine is reachable:"
    echo "     ${DIM}ping ${ip}${RESET}"
    echo ""
    echo "  2. Ensure SSH server is running on the remote machine:"
    echo "     ${DIM}# Linux:   sudo systemctl status sshd${RESET}"
    echo "     ${DIM}# Windows: Get-Service sshd${RESET}"
    echo "     ${DIM}# macOS:   System Settings → General → Sharing → Remote Login${RESET}"
    echo ""
    echo "  3. Copy your SSH key to the remote machine:"
    echo "     ${DIM}ssh-copy-id -p ${port} ${user}@${ip}${RESET}"
    echo ""
    echo "  4. If using a non-standard port, ensure the firewall allows it:"
    echo "     ${DIM}# Linux:   sudo ufw allow ${port}/tcp${RESET}"
    echo "     ${DIM}# Windows: New-NetFirewallRule -Name sshd -DisplayName 'OpenSSH' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort ${port}${RESET}"
    echo ""
    echo "  5. If the remote is Windows, enable OpenSSH Server:"
    echo "     ${DIM}Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0${RESET}"
    echo "     ${DIM}Start-Service sshd; Set-Service -Name sshd -StartupType Automatic${RESET}"
    echo ""
    echo "  6. Test manually:"
    echo "     ${DIM}ssh -v -p ${port} ${user}@${ip}${RESET}"
    echo ""
    return 1
}

# Detect remote OS via SSH.  Sets the appropriate LAN_*_OS variable.
# $1 = role ("tester"|"endpoint")
_lan_detect_os() {
    local role="$1"
    _lan_ssh_vars "$role"

    local remote_os
    remote_os="$(ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" "uname -s" 2>/dev/null || echo "unknown")"

    local detected="linux"
    case "$remote_os" in
        Linux*)          detected="linux" ;;
        Darwin*)         detected="linux" ;;   # macOS treated as unix/linux path
        CYGWIN*|MINGW*|MSYS*) detected="windows" ;;
        *_NT*)           detected="windows" ;;  # Windows via OpenSSH returns MSYS_NT or similar
        unknown)
            # Fallback: try PowerShell command
            if ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
                   "powershell -Command 'Write-Output windows'" 2>/dev/null | grep -q windows; then
                detected="windows"
            fi
            ;;
    esac

    if [[ "$role" == "tester" ]]; then
        LAN_TESTER_OS="$detected"
    else
        LAN_ENDPOINT_OS="$detected"
    fi
    print_ok "Detected remote OS: $detected"
}

# Prompt for LAN connection details.
# $1 = "tester" | "endpoint"
ask_lan_options() {
    local role="$1"
    local upper_role
    upper_role="$(echo "$role" | tr '[:lower:]' '[:upper:]')"
    local ip_var="LAN_${upper_role}_IP"
    local user_var="LAN_${upper_role}_USER"
    local port_var="LAN_${upper_role}_PORT"

    echo ""
    print_section "LAN deployment — networker-${role}"

    # IP address
    if [[ -z "${!ip_var}" ]]; then
        printf "  IP address or hostname: "
        local ip_ans
        read -r ip_ans </dev/tty || true
        if [[ -z "$ip_ans" ]]; then
            print_err "IP address is required for LAN deployment."
            exit 1
        fi
        printf -v "$ip_var" "%s" "$ip_ans"
    fi

    # SSH user
    if [[ -z "${!user_var}" ]]; then
        local default_user
        default_user="$(whoami)"
        printf "  SSH user [%s]: " "$default_user"
        local user_ans
        read -r user_ans </dev/tty || true
        printf -v "$user_var" "%s" "${user_ans:-$default_user}"
    fi

    # SSH port
    if [[ "${!port_var}" == "22" ]]; then
        printf "  SSH port [22]: "
        local port_ans
        read -r port_ans </dev/tty || true
        printf -v "$port_var" "%s" "${port_ans:-22}"
    fi

    # Test connection
    if ! _lan_test_ssh "${!ip_var}" "${!user_var}" "${!port_var}"; then
        exit 1
    fi

    # Detect OS
    _lan_detect_os "$role"
}

# Install binary on a LAN Linux host via SSH.
# $1 = binary ("networker-tester" or "networker-endpoint")
# $2 = role ("tester"|"endpoint")
_lan_install_binary_linux() {
    local binary="$1" role="$2"
    _lan_ssh_vars "$role"

    # Determine release version
    local ver="${NETWORKER_VERSION:-}"
    if [[ -z "$ver" ]]; then
        ver="$(gh release list --repo "$REPO_GH" --limit 1 --json tagName \
               -q '.[0].tagName' 2>/dev/null || echo "")"
    fi

    # Detect remote architecture
    local remote_arch
    remote_arch="$(ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" "uname -m" 2>/dev/null || echo "x86_64")"
    local remote_target
    case "$remote_arch" in
        x86_64)        remote_target="x86_64-unknown-linux-musl" ;;
        aarch64|arm64) remote_target="aarch64-unknown-linux-musl" ;;
        *)             remote_target="x86_64-unknown-linux-musl" ;;
    esac

    local archive="${binary}-${remote_target}.tar.gz"

    # Check if sudo is available (passwordless)
    local has_sudo=0
    if ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" "sudo -n true" &>/dev/null; then
        has_sudo=1
    fi

    if [[ $has_sudo -eq 0 ]]; then
        print_info "No passwordless sudo — installing to ~/.local/bin/"
    fi

    # Download and install on remote host
    print_info "Installing ${binary} on ${_LAN_DEST} (${ver:-source})…"
    local dl_ok=0
    if [[ -n "$ver" ]]; then
        local dl_url="https://github.com/${REPO_GH}/releases/download/${ver}/${archive}"
        if [[ $has_sudo -eq 1 ]]; then
            if ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
                "set -e; curl -fsSL '${dl_url}' -o /tmp/${archive} && tar xzf /tmp/${archive} -C /tmp && sudo mv /tmp/${binary} /usr/local/bin/${binary} && sudo chmod +x /usr/local/bin/${binary} && rm -f /tmp/${archive}" 2>/dev/null; then
                dl_ok=1
            fi
        else
            if ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
                "set -e; mkdir -p \"\$HOME/.local/bin\" && curl -fsSL '${dl_url}' -o /tmp/${archive} && tar xzf /tmp/${archive} -C /tmp && mv /tmp/${binary} \"\$HOME/.local/bin/${binary}\" && chmod +x \"\$HOME/.local/bin/${binary}\" && rm -f /tmp/${archive} && (grep -q '.local/bin' \"\$HOME/.bashrc\" 2>/dev/null || echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> \"\$HOME/.bashrc\") && (grep -q '.local/bin' \"\$HOME/.profile\" 2>/dev/null || echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> \"\$HOME/.profile\")" 2>/dev/null; then
                dl_ok=1
            fi
        fi
    fi
    if [[ $dl_ok -eq 0 ]]; then
        # No release assets or download failed — build from source
        if [[ -n "$ver" ]]; then
            print_info "No pre-built binary for ${ver} — building from source…"
        fi
        _lan_bootstrap_install "$binary" "$role"
        return
    fi

    # Verify
    local remote_ver
    if [[ $has_sudo -eq 1 ]]; then
        remote_ver="$(ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
            "/usr/local/bin/${binary} --version 2>/dev/null" || echo "unknown")"
    else
        remote_ver="$(ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
            "\$HOME/.local/bin/${binary} --version 2>/dev/null" || echo "unknown")"
    fi
    print_ok "${binary} installed on ${_LAN_DEST} (${remote_ver})"
}

# Run the installer on a remote LAN host (builds from source if no release).
# $1 = binary, $2 = role
_lan_bootstrap_install() {
    local binary="$1" role="$2"
    _lan_ssh_vars "$role"

    local comp_arg
    case "$binary" in
        networker-tester)   comp_arg="tester" ;;
        networker-endpoint) comp_arg="endpoint" ;;
        *)                  comp_arg="both" ;;
    esac

    echo ""
    print_info "Installing ${binary} on ${_LAN_DEST} via SSH (port ${_LAN_PORT})…"

    local script_path="${BASH_SOURCE[0]:-}"
    local gist_url="https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh"
    local repo_url="https://raw.githubusercontent.com/irlm/networker-tester/main/install.sh"
    if [[ -f "$script_path" ]]; then
        print_info "Uploading installer to remote host…"
        scp "${_LAN_SCP_OPTS[@]}" "$script_path" "${_LAN_DEST}:/tmp/networker-install.sh"
    else
        # Running as curl|bash — download locally first, then SCP
        print_info "Downloading installer locally, then uploading to remote host…"
        local tmp_installer="/tmp/networker-install-$$.sh"
        if curl -fsSL "${gist_url}" -o "$tmp_installer" 2>/dev/null || \
           curl -fsSL "${repo_url}" -o "$tmp_installer" 2>/dev/null; then
            scp "${_LAN_SCP_OPTS[@]}" "$tmp_installer" "${_LAN_DEST}:/tmp/networker-install.sh"
            rm -f "$tmp_installer"
        else
            rm -f "$tmp_installer"
            print_warn "Local download failed — trying directly on remote host…"
            ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
                "curl -fsSLk '${repo_url}' -o /tmp/networker-install.sh"
        fi
    fi

    print_info "Running installer on remote host…"
    echo ""
    ssh -t "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
        "bash /tmp/networker-install.sh ${comp_arg} -y --no-service"
}

# Install binary on a LAN Windows host via SSH + PowerShell.
# $1 = binary, $2 = role
_lan_install_binary_windows() {
    local binary="$1" role="$2"
    _lan_ssh_vars "$role"

    local installer_url="https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.ps1"

    print_info "Installing ${binary} on Windows host ${_LAN_DEST}…"

    # Download and run the PowerShell installer
    local comp_arg
    case "$binary" in
        networker-tester)   comp_arg="tester" ;;
        networker-endpoint) comp_arg="endpoint" ;;
        *)                  comp_arg="both" ;;
    esac

    ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
        "powershell -ExecutionPolicy Bypass -Command \"& { \
            Invoke-WebRequest -Uri '${installer_url}' -OutFile C:\\networker-install.ps1; \
            & C:\\networker-install.ps1 -Component ${comp_arg} -AutoYes \
        }\""

    local remote_ver
    remote_ver="$(ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" \
        "${binary} --version 2>/dev/null || echo unknown" 2>/dev/null || echo "unknown")"
    print_ok "${binary} installed on remote host (${remote_ver})"
}

# Create endpoint service on a LAN Linux host.
# $1 = role (always "endpoint")
_lan_create_endpoint_service() {
    local role="$1"
    _lan_ssh_vars "$role"

    # Check if sudo is available (passwordless)
    local has_sudo=0
    if ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" "sudo -n true" &>/dev/null; then
        has_sudo=1
    fi

    if [[ $has_sudo -eq 1 ]]; then
        # Full systemd service setup with iptables redirects
        ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" bash <<'REMOTE'
sudo useradd --system --no-create-home --shell /usr/sbin/nologin networker 2>/dev/null || true

# Find the binary
BIN_PATH="/usr/local/bin/networker-endpoint"
if [ ! -f "$BIN_PATH" ]; then
    BIN_PATH="$HOME/.local/bin/networker-endpoint"
fi

sudo tee /etc/systemd/system/networker-endpoint.service > /dev/null <<UNIT
[Unit]
Description=Networker Endpoint
After=network.target

[Service]
User=networker
ExecStart=${BIN_PATH}
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
UNIT

sudo systemctl daemon-reload
sudo systemctl enable networker-endpoint
sudo systemctl start networker-endpoint

if command -v iptables &>/dev/null; then
    sudo iptables -t nat -C PREROUTING -p tcp --dport 80  -j REDIRECT --to-port 8080 2>/dev/null || \
        sudo iptables -t nat -A PREROUTING -p tcp --dport 80  -j REDIRECT --to-port 8080
    sudo iptables -t nat -C PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || \
        sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443
    if command -v netfilter-persistent &>/dev/null; then
        sudo netfilter-persistent save 2>/dev/null || true
    elif command -v iptables-save &>/dev/null; then
        sudo mkdir -p /etc/iptables 2>/dev/null || true
        sudo sh -c 'iptables-save > /etc/iptables/rules.v4' 2>/dev/null || true
    fi
fi
REMOTE
        sleep 2
        print_ok "networker-endpoint systemd service enabled and started"
    else
        # No sudo — start as a background process
        print_info "No passwordless sudo — starting endpoint as background process"
        ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" bash <<'REMOTE'
# Find the binary
BIN_PATH="$HOME/.local/bin/networker-endpoint"
if [ ! -f "$BIN_PATH" ]; then
    BIN_PATH="/usr/local/bin/networker-endpoint"
fi

# Kill any existing instance
pkill -f networker-endpoint 2>/dev/null || true
sleep 1

# Start in background with nohup
mkdir -p "$HOME/.local/log"
RUST_LOG=info nohup "$BIN_PATH" > "$HOME/.local/log/networker-endpoint.log" 2>&1 &
echo $! > "$HOME/.local/networker-endpoint.pid"
echo "PID: $!"
REMOTE
        sleep 2
        print_ok "networker-endpoint started as background process (logs: ~/.local/log/networker-endpoint.log)"
        print_info "To stop: ssh ${_LAN_DEST} 'kill \$(cat ~/.local/networker-endpoint.pid)'"
    fi
}

# Create endpoint service on a LAN Windows host via SSH.
# $1 = role (always "endpoint")
_lan_create_endpoint_service_windows() {
    local role="$1"
    _lan_ssh_vars "$role"

    print_info "Creating networker-endpoint service on ${_LAN_DEST}…"
    ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" "powershell -ExecutionPolicy Bypass -Command \"& {
        \\\$exe = 'C:\\networker\\networker-endpoint.exe'
        # Stop any existing instance
        Stop-Process -Name 'networker-endpoint' -Force -ErrorAction SilentlyContinue
        # Firewall rules
        New-NetFirewallRule -Name 'NetworkerEndpoint-TCP' -DisplayName 'Networker Endpoint TCP' \`
            -Enabled True -Direction Inbound -Protocol TCP -Action Allow \`
            -LocalPort 8080,8443 -ErrorAction SilentlyContinue
        New-NetFirewallRule -Name 'NetworkerEndpoint-UDP' -DisplayName 'Networker Endpoint UDP' \`
            -Enabled True -Direction Inbound -Protocol UDP -Action Allow \`
            -LocalPort 8443,9998,9999 -ErrorAction SilentlyContinue
        # Start endpoint as detached process (SSH waits for child processes sharing console)
        Start-Process -FilePath \\\$exe -WindowStyle Hidden
        # Scheduled task for reboot persistence
        schtasks /Create /TN 'NetworkerEndpoint' /TR \\\$exe /SC ONSTART /RU SYSTEM /F 2>\\\$null
    }\""
    print_ok "networker-endpoint service created and started"
}

# Deploy tester to a LAN host.
step_lan_deploy_tester() {
    print_section "Deploy networker-tester to LAN host"

    local os="$LAN_TESTER_OS"
    if [[ "$os" == "windows" ]]; then
        _lan_install_binary_windows "networker-tester" "tester"
    else
        _lan_install_binary_linux "networker-tester" "tester"
    fi
}

# Deploy endpoint to a LAN host.
step_lan_deploy_endpoint() {
    print_section "Deploy networker-endpoint to LAN host"

    local os="$LAN_ENDPOINT_OS"
    if [[ "$os" == "windows" ]]; then
        _lan_install_binary_windows "networker-endpoint" "endpoint"
        _lan_create_endpoint_service_windows "endpoint"
    else
        _lan_install_binary_linux "networker-endpoint" "endpoint"
        _lan_create_endpoint_service "endpoint"
    fi

    step_generate_config "$LAN_ENDPOINT_IP"
}

# ── Azure naming helpers ──────────────────────────────────────────────────────

# Convert an Azure VM size to a short lowercase slug used in resource names.
# e.g. Standard_B1s → b1s, Standard_D2s_v3 → d2sv3
_azure_size_slug() {
    local size="${1#Standard_}"      # strip "Standard_"
    printf '%s' "$size" | tr '[:upper:]' '[:lower:]' | tr -d '_'
}

# Suggest a unique resource-group base name encoding the component, OS, size, and region.
# Format:  nwk-<ep|ts>-<lnx|win>-<size_slug>-<region>
# $1 = component ("endpoint"|"tester")
# $2 = os        ("linux"|"windows")
# $3 = size       e.g. "Standard_B1s"
# $4 = region     e.g. "eastus", "westeurope"
# Prints the unique base name; append -vm for the VM name.
_azure_suggest_name() {
    local component="$1" os="$2" size="$3" region="${4:-}"
    local c_tag; [[ "$component" == "tester" ]] && c_tag="ts" || c_tag="ep"
    local os_tag; [[ "$os" == "windows" ]] && os_tag="win" || os_tag="lnx"
    local sz_tag; sz_tag="$(_azure_size_slug "$size")"
    if [[ -n "$region" ]]; then
        printf '%s' "nwk-${c_tag}-${os_tag}-${sz_tag}-${region}"
    else
        printf '%s' "nwk-${c_tag}-${os_tag}-${sz_tag}"
    fi
}

# Find a unique VM name within a resource group.
# Format: <base>-vm, <base>-vm-2, <base>-vm-3, …
# $1 = resource group name  $2 = base name
_azure_suggest_vm_name() {
    local rg="$1" base="$2"
    local candidate="${base}-vm"
    local n=2
    while az vm show --resource-group "$rg" --name "$candidate" \
              --output none 2>/dev/null; do
        candidate="${base}-vm-${n}"
        n=$((n + 1))
    done
    printf '%s' "$candidate"
}

# ── Azure interactive configuration ──────────────────────────────────────────
ask_azure_options() {
    local component="$1"  # "tester" or "endpoint"
    local title
    case "$component" in
        tester)   title="networker-tester" ;;
        endpoint) title="networker-endpoint" ;;
    esac

    print_section "Azure options for $title"
    echo ""

    # Region (shared; ask once)
    if [[ $AZURE_REGION_ASKED -eq 0 ]]; then
        AZURE_REGION_ASKED=1
        local regions=(
            "eastus:East US (Virginia)"
            "westus2:West US 2 (Washington)"
            "westeurope:West Europe (Netherlands)"
            "northeurope:North Europe (Ireland)"
            "southeastasia:Southeast Asia (Singapore)"
            "australiaeast:Australia East (New South Wales)"
            "uksouth:UK South (London)"
            "japaneast:Japan East (Tokyo)"
        )
        echo "  Azure region:"
        local i=1
        for r in "${regions[@]}"; do
            local code="${r%%:*}"
            local label="${r#*:}"
            if [[ "$code" == "$AZURE_REGION" ]]; then
                printf "    %s) %-20s %s  ${DIM}[current]${RESET}\n" "$i" "$code" "$label"
            else
                printf "    %s) %-20s %s\n" "$i" "$code" "$label"
            fi
            i=$((i + 1))
        done
        echo ""
        printf "  Choice [1]: "
        local reg_ans
        read -r reg_ans </dev/tty || true
        reg_ans="${reg_ans:-1}"
        if [[ "$reg_ans" =~ ^[0-9]+$ ]] && \
           [[ "$reg_ans" -ge 1 ]] && \
           [[ "$reg_ans" -le "${#regions[@]}" ]]; then
            local chosen="${regions[$((reg_ans - 1))]}"
            AZURE_REGION="${chosen%%:*}"
        fi
        print_ok "Region: $AZURE_REGION"
        echo ""
    else
        print_info "Region: $AZURE_REGION  (shared with other Azure VM)"
        echo ""
    fi

    # VM size
    echo "  VM size:"
    echo "    1) Standard_B1s     1 vCPU,  1 GB RAM  ~\$7/mo   (minimal)"
    echo "    2) Standard_B2s     2 vCPU,  4 GB RAM  ~\$30/mo  [default]"
    echo "    3) Standard_D2s_v3  2 vCPU,  8 GB RAM  ~\$70/mo"
    echo "    4) Standard_D4s_v3  4 vCPU, 16 GB RAM  ~\$140/mo"
    echo ""
    printf "  Choice [2]: "
    local size_ans
    read -r size_ans </dev/tty || true
    size_ans="${size_ans:-2}"
    local chosen_size
    case "$size_ans" in
        1) chosen_size="Standard_B1s" ;;
        3) chosen_size="Standard_D2s_v3" ;;
        4) chosen_size="Standard_D4s_v3" ;;
        *) chosen_size="Standard_B2s" ;;
    esac
    echo ""

    # OS choice
    echo "  Operating System:"
    echo "    1) Ubuntu 22.04 LTS  (Linux)    [default]"
    echo "    2) Windows Server 2022"
    echo ""
    printf "  Choice [1]: "
    local os_ans; read -r os_ans </dev/tty || true
    os_ans="${os_ans:-1}"
    local chosen_os="linux"
    [[ "$os_ans" == "2" ]] && chosen_os="windows"
    echo ""

    # Auto-shutdown policy (ask once across all Azure VMs)
    if [[ $AZURE_SHUTDOWN_ASKED -eq 0 ]]; then
        AZURE_SHUTDOWN_ASKED=1
        echo "  Auto-shutdown policy (avoids unexpected charges):"
        echo "    1) Shut down at 11 PM EST (04:00 UTC) daily  [default]"
        echo "    2) Leave running — I will stop/delete manually"
        echo ""
        printf "  Choice [1]: "
        local sd_ans; read -r sd_ans </dev/tty || true
        sd_ans="${sd_ans:-1}"
        if [[ "$sd_ans" == "2" ]]; then
            AZURE_AUTO_SHUTDOWN="no"
            print_warn "VMs will keep running — remember to delete them when done to avoid charges!"
        else
            AZURE_AUTO_SHUTDOWN="yes"
            print_ok "Auto-shutdown: 04:00 UTC (11 PM EST) daily"
        fi
        echo ""
    fi

    # Generate a suggested base name from the chosen OS and size, then prompt
    # the user for RG (reusing an existing RG is fine) and a unique VM name.
    local suggested_base
    suggested_base="$(_azure_suggest_name "$component" "$chosen_os" "$chosen_size" "$AZURE_REGION")"
    echo ""

    if [[ "$component" == "tester" ]]; then
        printf "  Resource group name [%s]: " "$suggested_base"
        local rg_ans; read -r rg_ans </dev/tty || true
        AZURE_TESTER_RG="${rg_ans:-$suggested_base}"

        if az group show --name "$AZURE_TESTER_RG" --output none 2>/dev/null; then
            print_info "Resource group '${AZURE_TESTER_RG}' exists — adding VM to it."
        fi

        local suggested_vm
        suggested_vm="$(_azure_suggest_vm_name "$AZURE_TESTER_RG" "$suggested_base")"
        printf "  VM name             [%s]: " "$suggested_vm"
        local vm_ans; read -r vm_ans </dev/tty || true
        AZURE_TESTER_VM="${vm_ans:-$suggested_vm}"
        AZURE_TESTER_SIZE="$chosen_size"
        AZURE_TESTER_OS="$chosen_os"
        print_ok "OS: $chosen_os  |  Size: $AZURE_TESTER_SIZE  |  RG: $AZURE_TESTER_RG  |  VM: $AZURE_TESTER_VM"
    else
        printf "  Resource group name [%s]: " "$suggested_base"
        local rg_ans; read -r rg_ans </dev/tty || true
        AZURE_ENDPOINT_RG="${rg_ans:-$suggested_base}"

        if az group show --name "$AZURE_ENDPOINT_RG" --output none 2>/dev/null; then
            print_info "Resource group '${AZURE_ENDPOINT_RG}' exists — adding VM to it."
        fi

        local suggested_vm
        suggested_vm="$(_azure_suggest_vm_name "$AZURE_ENDPOINT_RG" "$suggested_base")"
        printf "  VM name             [%s]: " "$suggested_vm"
        local vm_ans; read -r vm_ans </dev/tty || true
        AZURE_ENDPOINT_VM="${vm_ans:-$suggested_vm}"
        AZURE_ENDPOINT_SIZE="$chosen_size"
        AZURE_ENDPOINT_OS="$chosen_os"
        print_ok "OS: $chosen_os  |  Size: $AZURE_ENDPOINT_SIZE  |  RG: $AZURE_ENDPOINT_RG  |  VM: $AZURE_ENDPOINT_VM"
    fi
    echo ""
}

# ── AWS interactive configuration ─────────────────────────────────────────────
ask_aws_options() {
    local component="$1"  # "tester" or "endpoint"
    local title
    case "$component" in
        tester)   title="networker-tester" ;;
        endpoint) title="networker-endpoint" ;;
    esac

    print_section "AWS options for $title"
    echo ""

    # Region (shared; ask once)
    if [[ $AWS_REGION_ASKED -eq 0 ]]; then
        AWS_REGION_ASKED=1
        local regions=(
            "us-east-1:US East (N. Virginia)"
            "us-west-2:US West (Oregon)"
            "eu-west-1:EU West (Ireland)"
            "eu-central-1:EU Central (Frankfurt)"
            "ap-southeast-1:Asia Pacific (Singapore)"
            "ap-northeast-1:Asia Pacific (Tokyo)"
            "ap-southeast-2:Asia Pacific (Sydney)"
            "sa-east-1:South America (São Paulo)"
        )
        echo "  AWS region:"
        local i=1
        for r in "${regions[@]}"; do
            local code="${r%%:*}"
            local label="${r#*:}"
            if [[ "$code" == "$AWS_REGION" ]]; then
                printf "    %s) %-20s %s  ${DIM}[current]${RESET}\n" "$i" "$code" "$label"
            else
                printf "    %s) %-20s %s\n" "$i" "$code" "$label"
            fi
            i=$((i + 1))
        done
        echo ""
        printf "  Choice [1]: "
        local reg_ans
        read -r reg_ans </dev/tty || true
        reg_ans="${reg_ans:-1}"
        if [[ "$reg_ans" =~ ^[0-9]+$ ]] && \
           [[ "$reg_ans" -ge 1 ]] && \
           [[ "$reg_ans" -le "${#regions[@]}" ]]; then
            local chosen="${regions[$((reg_ans - 1))]}"
            AWS_REGION="${chosen%%:*}"
        fi
        print_ok "Region: $AWS_REGION"
        echo ""
    else
        print_info "Region: $AWS_REGION  (shared with other AWS instance)"
        echo ""
    fi

    # Instance type
    echo "  EC2 instance type:"
    echo "    1) t3.micro   2 vCPU,  1 GB RAM  ~\$7/mo   (free-tier eligible: t2.micro)"
    echo "    2) t3.small   2 vCPU,  2 GB RAM  ~\$15/mo  [default]"
    echo "    3) t3.medium  2 vCPU,  4 GB RAM  ~\$30/mo"
    echo "    4) t3.large   2 vCPU,  8 GB RAM  ~\$60/mo"
    echo ""
    printf "  Choice [2]: "
    local type_ans
    read -r type_ans </dev/tty || true
    type_ans="${type_ans:-2}"
    local chosen_type
    case "$type_ans" in
        1) chosen_type="t3.micro" ;;
        3) chosen_type="t3.medium" ;;
        4) chosen_type="t3.large" ;;
        *) chosen_type="t3.small" ;;
    esac
    echo ""

    # OS choice
    echo "  Operating System:"
    echo "    1) Amazon Linux 2023 / Ubuntu 22.04  (Linux)  [default]"
    echo "    2) Windows Server 2022"
    echo ""
    printf "  Choice [1]: "
    local os_ans; read -r os_ans </dev/tty || true
    os_ans="${os_ans:-1}"
    local chosen_os="linux"
    [[ "$os_ans" == "2" ]] && chosen_os="windows"
    echo ""

    # Auto-shutdown policy (ask once across all AWS instances)
    if [[ $AWS_SHUTDOWN_ASKED -eq 0 ]]; then
        AWS_SHUTDOWN_ASKED=1
        echo "  Auto-shutdown policy (avoids unexpected charges):"
        echo "    1) Shut down at 11 PM EST (04:00 UTC) daily  [default]"
        echo "    2) Leave running — I will terminate manually"
        echo ""
        printf "  Choice [1]: "
        local sd_ans; read -r sd_ans </dev/tty || true
        sd_ans="${sd_ans:-1}"
        if [[ "$sd_ans" == "2" ]]; then
            AWS_AUTO_SHUTDOWN="no"
            print_warn "Instance will keep running — remember to terminate it when done to avoid charges!"
        else
            AWS_AUTO_SHUTDOWN="yes"
            print_ok "Auto-shutdown: 04:00 UTC (11 PM EST) daily (via cron on the instance)"
        fi
        echo ""
    fi

    # Instance Name tag — include region so different-region VMs get unique names
    if [[ "$component" == "tester" ]]; then
        local suggested="networker-tester-${AWS_REGION}"
        printf "  Instance name tag [%s]: " "$suggested"
        local name_ans; read -r name_ans </dev/tty || true
        AWS_TESTER_NAME="${name_ans:-$suggested}"
        AWS_TESTER_INSTANCE_TYPE="$chosen_type"
        AWS_TESTER_OS="$chosen_os"
        print_ok "OS: $chosen_os  |  Type: $AWS_TESTER_INSTANCE_TYPE  |  Name: $AWS_TESTER_NAME"
    else
        local suggested="networker-endpoint-${AWS_REGION}"
        printf "  Instance name tag [%s]: " "$suggested"
        local name_ans; read -r name_ans </dev/tty || true
        AWS_ENDPOINT_NAME="${name_ans:-$suggested}"
        AWS_ENDPOINT_INSTANCE_TYPE="$chosen_type"
        AWS_ENDPOINT_OS="$chosen_os"
        print_ok "OS: $chosen_os  |  Type: $AWS_ENDPOINT_INSTANCE_TYPE  |  Name: $AWS_ENDPOINT_NAME"
    fi
    echo ""
}

# Resolve GCP project number to project ID if needed.
# gcloud compute commands require project ID (string), not number (numeric).
_gcp_resolve_project() {
    if [[ "$GCP_PROJECT" =~ ^[0-9]+$ ]]; then
        local proj_id
        proj_id="$(gcloud projects describe "$GCP_PROJECT" \
            --format='value(projectId)' 2>/dev/null || echo "")"
        if [[ -n "$proj_id" ]]; then
            print_dim "Resolved project number $GCP_PROJECT → $proj_id"
            GCP_PROJECT="$proj_id"
            gcloud config set project "$GCP_PROJECT" 2>/dev/null || true
        fi
    fi
}

# ── GCP interactive configuration ────────────────────────────────────────────
ask_gcp_options() {
    local component="$1"  # "tester" or "endpoint"
    local title
    case "$component" in
        tester)   title="networker-tester" ;;
        endpoint) title="networker-endpoint" ;;
    esac

    print_section "GCP options for $title"
    echo ""

    # Project (ask once, auto-detect from gcloud config)
    if [[ -z "$GCP_PROJECT" ]]; then
        GCP_PROJECT="$(gcloud config get-value project 2>/dev/null || echo "")"
        [[ "$GCP_PROJECT" == "(unset)" ]] && GCP_PROJECT=""
    fi
    if [[ -z "$GCP_PROJECT" ]]; then
        echo "  No GCP project is set."
        printf "  Enter your GCP project ID: "
        local proj_ans
        read -r proj_ans </dev/tty || true
        if [[ -z "$proj_ans" ]]; then
            print_err "GCP project ID is required."
            exit 1
        fi
        GCP_PROJECT="$proj_ans"
        gcloud config set project "$GCP_PROJECT" 2>/dev/null || true
    fi
    _gcp_resolve_project
    print_ok "Project: $GCP_PROJECT"
    echo ""

    # Region / Zone (shared; ask once)
    if [[ $GCP_REGION_ASKED -eq 0 ]]; then
        GCP_REGION_ASKED=1
        local zones=(
            "us-central1-a:US Central (Iowa)"
            "us-east1-b:US East (South Carolina)"
            "us-west1-a:US West (Oregon)"
            "europe-west1-b:Europe West (Belgium)"
            "europe-west2-a:Europe West (London)"
            "asia-east1-a:Asia East (Taiwan)"
            "asia-northeast1-a:Asia NE (Tokyo)"
            "australia-southeast1-a:Australia SE (Sydney)"
        )
        echo "  GCP zone:"
        local i=1
        for z in "${zones[@]}"; do
            local code="${z%%:*}"
            local label="${z#*:}"
            if [[ "$code" == "$GCP_ZONE" ]]; then
                printf "    %s) %-28s %s  ${DIM}[current]${RESET}\n" "$i" "$code" "$label"
            else
                printf "    %s) %-28s %s\n" "$i" "$code" "$label"
            fi
            i=$((i + 1))
        done
        echo ""
        printf "  Choice [1]: "
        local zone_ans
        read -r zone_ans </dev/tty || true
        zone_ans="${zone_ans:-1}"
        if [[ "$zone_ans" =~ ^[0-9]+$ ]] && \
           [[ "$zone_ans" -ge 1 ]] && \
           [[ "$zone_ans" -le "${#zones[@]}" ]]; then
            local chosen="${zones[$((zone_ans - 1))]}"
            GCP_ZONE="${chosen%%:*}"
        fi
        # Derive region from zone (strip trailing -[a-z])
        GCP_REGION="${GCP_ZONE%-*}"
        print_ok "Zone: $GCP_ZONE  (region: $GCP_REGION)"
        echo ""
    else
        print_info "Zone: $GCP_ZONE  (shared with other GCP instance)"
        echo ""
    fi

    # Machine type
    echo "  GCE machine type:"
    echo "    1) e2-micro      2 vCPU (shared), 1 GB RAM  ~\$7/mo   (free-tier eligible)"
    echo "    2) e2-small      2 vCPU (shared), 2 GB RAM  ~\$15/mo  [default]"
    echo "    3) e2-medium     2 vCPU (shared), 4 GB RAM  ~\$27/mo"
    echo "    4) e2-standard-2 2 vCPU,          8 GB RAM  ~\$49/mo"
    echo ""
    printf "  Choice [2]: "
    local type_ans
    read -r type_ans </dev/tty || true
    type_ans="${type_ans:-2}"
    local chosen_type
    case "$type_ans" in
        1) chosen_type="e2-micro" ;;
        3) chosen_type="e2-medium" ;;
        4) chosen_type="e2-standard-2" ;;
        *) chosen_type="e2-small" ;;
    esac
    echo ""

    # OS choice
    echo "  Operating System:"
    echo "    1) Ubuntu 22.04 LTS  (Linux)    [default]"
    echo "    2) Windows Server 2022"
    echo ""
    printf "  Choice [1]: "
    local os_ans; read -r os_ans </dev/tty || true
    os_ans="${os_ans:-1}"
    local chosen_os="linux"
    [[ "$os_ans" == "2" ]] && chosen_os="windows"
    echo ""

    # Auto-shutdown policy (ask once across all GCP instances)
    if [[ $GCP_SHUTDOWN_ASKED -eq 0 ]]; then
        GCP_SHUTDOWN_ASKED=1
        echo "  Auto-shutdown policy (avoids unexpected charges):"
        echo "    1) Shut down at 11 PM EST (04:00 UTC) daily  [default]"
        echo "    2) Leave running — I will stop/delete manually"
        echo ""
        printf "  Choice [1]: "
        local sd_ans; read -r sd_ans </dev/tty || true
        sd_ans="${sd_ans:-1}"
        if [[ "$sd_ans" == "2" ]]; then
            GCP_AUTO_SHUTDOWN="no"
            print_warn "Instance will keep running — remember to delete it when done to avoid charges!"
        else
            GCP_AUTO_SHUTDOWN="yes"
            print_ok "Auto-shutdown: 04:00 UTC (11 PM EST) daily (via cron on the instance)"
        fi
        echo ""
    fi

    # Instance name — include region so different-region VMs get unique names
    # GCP names must be lowercase, alphanumeric, and hyphens only
    local region_tag="${GCP_REGION:-${GCP_ZONE%-*}}"
    if [[ "$component" == "tester" ]]; then
        local suggested="networker-tester-${region_tag}"
        printf "  Instance name [%s]: " "$suggested"
        local name_ans; read -r name_ans </dev/tty || true
        GCP_TESTER_NAME="${name_ans:-$suggested}"
        GCP_TESTER_MACHINE_TYPE="$chosen_type"
        GCP_TESTER_OS="$chosen_os"
        print_ok "OS: $chosen_os  |  Type: $GCP_TESTER_MACHINE_TYPE  |  Name: $GCP_TESTER_NAME  |  Zone: $GCP_ZONE"
    else
        local suggested="networker-endpoint-${region_tag}"
        printf "  Instance name [%s]: " "$suggested"
        local name_ans; read -r name_ans </dev/tty || true
        GCP_ENDPOINT_NAME="${name_ans:-$suggested}"
        GCP_ENDPOINT_MACHINE_TYPE="$chosen_type"
        GCP_ENDPOINT_OS="$chosen_os"
        print_ok "OS: $chosen_os  |  Type: $GCP_ENDPOINT_MACHINE_TYPE  |  Name: $GCP_ENDPOINT_NAME  |  Zone: $GCP_ZONE"
    fi
    echo ""
}

# ── Ensure Azure CLI is installed and authenticated ───────────────────────────
ensure_azure_cli() {
    if [[ $AZURE_CLI_AVAILABLE -eq 0 ]]; then
        echo ""
        print_warn "Azure CLI (az) is not installed."
        echo ""
        echo "  The Azure CLI is required to provision VMs and manage resources."
        echo ""
        if [[ -n "$PKG_MGR" ]]; then
            local install_cmd
            case "$PKG_MGR" in
                brew)    install_cmd="brew install azure-cli" ;;
                apt-get) install_cmd="curl -sL https://aka.ms/InstallAzureCLIDeb | sudo bash" ;;
                dnf)     install_cmd="sudo rpm --import https://packages.microsoft.com/keys/microsoft.asc && sudo dnf install -y azure-cli" ;;
                pacman)  install_cmd="sudo pacman -S --noconfirm azure-cli" ;;
                zypper)  install_cmd="sudo zypper install -y azure-cli" ;;
                *)       install_cmd="" ;;
            esac
            if [[ -n "$install_cmd" ]]; then
                echo "  Install command:  $install_cmd"
                echo ""
                if ask_yn "Install Azure CLI now?" "y"; then
                    echo ""
                    case "$PKG_MGR" in
                        brew)    brew install azure-cli ;;
                        apt-get) curl -sL https://aka.ms/InstallAzureCLIDeb | sudo bash ;;
                        dnf)
                            sudo rpm --import https://packages.microsoft.com/keys/microsoft.asc 2>/dev/null || true
                            sudo dnf install -y azure-cli ;;
                        pacman)  sudo pacman -S --noconfirm azure-cli ;;
                        zypper)  sudo zypper install -y azure-cli ;;
                    esac
                    if command -v az &>/dev/null; then
                        AZURE_CLI_AVAILABLE=1
                        print_ok "Azure CLI installed"
                    else
                        print_err "Azure CLI installation failed — install manually from https://docs.microsoft.com/cli/azure/install-azure-cli"
                        echo "  Then re-run this installer."
                        exit 1
                    fi
                else
                    print_err "Azure CLI is required for remote Azure deployment."
                    echo "  Install from: https://docs.microsoft.com/cli/azure/install-azure-cli"
                    echo "  Then re-run:  bash install.sh --azure"
                    exit 1
                fi
            else
                echo "  Install from: https://docs.microsoft.com/cli/azure/install-azure-cli"
                echo "  Then re-run:  bash install.sh --azure"
                exit 1
            fi
        else
            echo "  Install from: https://docs.microsoft.com/cli/azure/install-azure-cli"
            echo "  Then re-run:  bash install.sh --azure"
            exit 1
        fi
    fi

    # Re-check: service principal env vars may have been set after discover_system
    if [[ $AZURE_LOGGED_IN -eq 0 && -n "${AZURE_CLIENT_ID:-}" && -n "${AZURE_CLIENT_SECRET:-}" && -n "${AZURE_TENANT_ID:-}" ]]; then
        if az account show &>/dev/null 2>&1 </dev/null; then
            AZURE_LOGGED_IN=1
        fi
    fi

    if [[ $AZURE_LOGGED_IN -eq 1 ]]; then
        local az_sub
        az_sub="$(az account show --query name -o tsv 2>/dev/null </dev/null || echo 'unknown')"
        print_ok "Azure credentials found  (subscription: $az_sub)"
    else
        echo ""
        print_warn "Not logged in to Azure."
        echo ""
        if ask_yn "Log in to Azure now (device code)?" "y"; then
            _az_do_login
            if [[ $AZURE_LOGGED_IN -eq 0 ]]; then
                print_err "Azure login failed — fix manually then re-run the installer."
                echo "  Manual fix:  az login --tenant YOUR_TENANT_ID --use-device-code"
                exit 1
            fi
        else
            print_err "Azure login required for remote deployment."
            echo "  Run:  az login --use-device-code"
            exit 1
        fi
    fi
}

# ── Internal: run az login with device-code and optional tenant; retries on
#    "no subscription found" (common when MFA policy requires a specific tenant)
_az_do_login() {
    local tenant="${1:-}"
    echo ""
    if [[ -n "$tenant" ]]; then
        print_info "Logging in to Azure (tenant: $tenant)…"
        az login --tenant "$tenant" --use-device-code
    else
        print_info "Logging in to Azure (device code)…"
        az login --use-device-code
    fi

    if az account show &>/dev/null 2>&1; then
        AZURE_LOGGED_IN=1
        local sub_name
        sub_name="$(az account show --query name -o tsv 2>/dev/null || echo "")"
        print_ok "Logged in: $sub_name"
        return 0
    fi

    # Login appeared to succeed but no subscription is visible — common when the
    # account has multiple tenants and the default one has no subscriptions, or
    # when MFA policy blocks the default tenant.
    echo ""
    print_warn "Logged in but no Azure subscription found."
    print_info "This usually means your account needs a specific tenant."
    echo ""
    echo "  To find your tenant ID:  az account tenant list"
    echo "  Then retry:              az login --tenant TENANT_ID --use-device-code"
    echo ""
    printf "  Enter tenant ID to retry now (or press Enter to cancel): "
    local tenant_id
    read -r tenant_id </dev/tty || true
    tenant_id="${tenant_id// /}"   # strip accidental spaces
    if [[ -n "$tenant_id" ]]; then
        az logout 2>/dev/null || true
        _az_do_login "$tenant_id"
    fi
}

# ── Ensure AWS CLI is installed and authenticated ─────────────────────────────
ensure_aws_cli() {
    if [[ $AWS_CLI_AVAILABLE -eq 0 ]]; then
        echo ""
        print_warn "AWS CLI (aws) is not installed."
        echo ""
        echo "  The AWS CLI is required to provision EC2 instances and manage resources."
        echo ""
        # Determine install method. On Linux use the official AWS CLI v2 zip installer
        # (apt-get/dnf ship an outdated v1 package that is often missing entirely).
        local install_cmd=""
        if [[ "$PKG_MGR" == "brew" ]]; then
            install_cmd="brew install awscli"
        elif [[ "$SYS_OS" == "Linux" ]]; then
            install_cmd="official AWS CLI v2 installer (curl + unzip)"
        fi

        if [[ -n "$install_cmd" ]]; then
            echo "  Install command:  $install_cmd"
            echo ""
            if ask_yn "Install AWS CLI now?" "y"; then
                echo ""
                if [[ "$PKG_MGR" == "brew" ]]; then
                    brew install awscli
                else
                    # Official AWS CLI v2 for Linux (x86_64 and arm64)
                    local arch_url="https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip"
                    [[ "$(uname -m)" == "aarch64" ]] && \
                        arch_url="https://awscli.amazonaws.com/awscli-exe-linux-aarch64.zip"
                    if ! command -v unzip &>/dev/null; then
                        case "$PKG_MGR" in
                            apt-get) sudo DEBIAN_FRONTEND=noninteractive apt-get install -y unzip 2>&1 | tail -1 || true ;;
                            dnf)     sudo dnf install -y unzip     2>&1 | tail -1 || true ;;
                            yum)     sudo yum install -y unzip     2>&1 | tail -1 || true ;;
                            pacman)  sudo pacman -S --noconfirm unzip 2>&1 | tail -1 || true ;;
                            *)       print_err "unzip not found — install it then re-run." ; exit 1 ;;
                        esac
                    fi
                    print_info "Downloading AWS CLI v2…"
                    curl -fsSL "$arch_url" -o /tmp/awscliv2.zip
                    unzip -q /tmp/awscliv2.zip -d /tmp/awscli-install
                    sudo /tmp/awscli-install/aws/install --update
                    rm -rf /tmp/awscliv2.zip /tmp/awscli-install
                fi
                if command -v aws &>/dev/null; then
                    AWS_CLI_AVAILABLE=1
                    print_ok "AWS CLI installed  ($(aws --version 2>&1 | head -1))"
                else
                    print_err "AWS CLI installation failed — install manually from https://aws.amazon.com/cli/"
                    echo "  Then re-run this installer."
                    exit 1
                fi
            else
                print_err "AWS CLI is required for remote AWS deployment."
                echo "  Install from: https://aws.amazon.com/cli/"
                echo "  Then re-run:  bash install.sh --aws"
                exit 1
            fi
        else
            echo "  Install from: https://aws.amazon.com/cli/"
            echo "  Then re-run:  bash install.sh --aws"
            exit 1
        fi
    fi

    # Re-check: env vars may have been set after discover_system ran
    if [[ $AWS_LOGGED_IN -eq 0 && -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]]; then
        if aws sts get-caller-identity &>/dev/null 2>&1 </dev/null; then
            AWS_LOGGED_IN=1
        fi
    fi

    if [[ $AWS_LOGGED_IN -eq 1 ]]; then
        local aws_ident
        aws_ident="$(aws sts get-caller-identity --query 'Arn' --output text 2>/dev/null </dev/null || echo 'unknown')"
        print_ok "AWS credentials found  ($aws_ident)"
    else
        echo ""
        print_warn "AWS CLI is not configured or credentials are not valid."
        echo ""
        echo "  Choose an authentication method:"
        echo "    1) AWS SSO / Identity Center  (device code — opens browser, no keys needed)"
        echo "    2) Access keys                (AWS_ACCESS_KEY_ID + secret)"
        echo ""
        if ask_yn "Log in to AWS now?" "y"; then
            echo ""
            printf "  Auth method [1/2, default 1]: "
            local aws_auth_method
            read -r aws_auth_method </dev/tty || true
            aws_auth_method="${aws_auth_method:-1}"

            if [[ "$aws_auth_method" == "2" ]]; then
                _aws_do_login_keys
            else
                _aws_do_login_sso
            fi

            if [[ $AWS_LOGGED_IN -eq 0 ]]; then
                print_err "AWS authentication failed — fix manually then re-run the installer."
                echo "  SSO:         aws configure sso && aws sso login"
                echo "  Access keys: aws configure"
                exit 1
            fi
        else
            print_err "AWS credentials required for remote deployment."
            echo "  SSO:         aws configure sso && aws sso login"
            echo "  Access keys: aws configure"
            exit 1
        fi
    fi
}

# ── Internal: AWS SSO device-code login (similar to az login --use-device-code)
_aws_do_login_sso() {
    echo ""

    # Check if an SSO profile already exists (user ran configure sso before)
    local sso_profiles
    sso_profiles="$(aws configure list-profiles 2>/dev/null | while read -r p; do
        if aws configure get sso_start_url --profile "$p" &>/dev/null 2>&1; then
            echo "$p"
        fi
    done || true)"

    if [[ -z "$sso_profiles" ]]; then
        print_info "Setting up AWS SSO profile (one-time setup)…"
        echo ""
        echo "  You will need your SSO start URL (e.g. https://my-org.awsapps.com/start)"
        echo "  and your SSO region (e.g. us-east-1)."
        echo ""
        aws configure sso </dev/tty
    else
        # Pick an existing SSO profile or create a new one
        local profile_count
        profile_count="$(echo "$sso_profiles" | wc -l | tr -d ' ')"
        if [[ "$profile_count" -eq 1 ]]; then
            local sso_profile="$sso_profiles"
            print_info "Using SSO profile: $sso_profile"
        else
            echo "  Existing SSO profiles:"
            local i=1
            while IFS= read -r p; do
                echo "    $i) $p"
                i=$((i + 1))
            done <<< "$sso_profiles"
            echo "    $i) Configure a new SSO profile"
            echo ""
            local choice
            if [[ $AUTO_YES -eq 1 ]]; then
                choice="1"
            else
                printf "  Select profile [1]: "
                read -r choice </dev/tty || true
                choice="${choice:-1}"
            fi

            if [[ "$choice" -eq "$i" ]]; then
                aws configure sso </dev/tty
                _aws_check_identity
                return
            else
                local sso_profile
                sso_profile="$(echo "$sso_profiles" | sed -n "${choice}p")"
                if [[ -z "$sso_profile" ]]; then
                    sso_profile="$(echo "$sso_profiles" | head -1)"
                fi
            fi
        fi

        print_info "Logging in via AWS SSO (device code)…"
        aws sso login --profile "$sso_profile" </dev/tty

        # Export the profile so subsequent aws commands use it
        export AWS_PROFILE="$sso_profile"
    fi

    _aws_check_identity
}

# ── Internal: AWS access-key login (classic aws configure)
_aws_do_login_keys() {
    echo ""
    print_info "Running aws configure (access key + secret)…"
    echo ""
    aws configure </dev/tty

    _aws_check_identity
}

# ── Internal: verify AWS identity after login attempt
_aws_check_identity() {
    if aws sts get-caller-identity &>/dev/null 2>&1; then
        AWS_LOGGED_IN=1
        local aws_account
        aws_account="$(aws sts get-caller-identity --query Account --output text 2>/dev/null || echo "")"
        print_ok "AWS authenticated  (account: $aws_account)"
    fi
}

# ── Ensure GCP CLI (gcloud) is installed and authenticated ────────────────────
ensure_gcp_cli() {
    if [[ $GCP_CLI_AVAILABLE -eq 0 ]]; then
        echo ""
        print_warn "Google Cloud SDK (gcloud) is not installed."
        echo ""
        echo "  The gcloud CLI is required to provision GCE instances and manage resources."
        echo ""
        echo "  Install from: https://cloud.google.com/sdk/docs/install"
        if [[ "$PKG_MGR" == "brew" ]]; then
            echo "    macOS:  brew install --cask google-cloud-sdk"
        fi
        echo ""
        local gcp_install_cmd=""
        if [[ "$PKG_MGR" == "brew" ]]; then
            gcp_install_cmd="brew install --cask google-cloud-sdk"
        elif [[ "$SYS_OS" == "Linux" ]]; then
            gcp_install_cmd="official Google Cloud SDK installer (curl + tar)"
        fi

        if [[ -n "$gcp_install_cmd" ]]; then
            echo "  Install command:  $gcp_install_cmd"
            echo ""
            if ask_yn "Install Google Cloud SDK now?" "y"; then
                echo ""
                if [[ "$PKG_MGR" == "brew" ]]; then
                    brew install --cask google-cloud-sdk
                    if [[ -f "$(brew --prefix)/share/google-cloud-sdk/path.bash.inc" ]]; then
                        # shellcheck disable=SC1091
                        source "$(brew --prefix)/share/google-cloud-sdk/path.bash.inc"
                    fi
                else
                    # Official Google Cloud SDK for Linux (installs to ~/google-cloud-sdk)
                    local gcp_arch="x86_64"
                    [[ "$(uname -m)" == "aarch64" ]] && gcp_arch="arm"
                    local gcp_url="https://dl.google.com/dl/cloudsdk/channels/rapid/downloads/google-cloud-cli-linux-${gcp_arch}.tar.gz"
                    print_info "Downloading Google Cloud SDK…"
                    curl -fsSL "$gcp_url" -o /tmp/google-cloud-sdk.tar.gz
                    tar xzf /tmp/google-cloud-sdk.tar.gz -C "${HOME}"
                    "${HOME}/google-cloud-sdk/install.sh" --quiet --path-update true \
                        --usage-reporting false --command-completion false
                    rm -f /tmp/google-cloud-sdk.tar.gz
                    export PATH="${HOME}/google-cloud-sdk/bin:${PATH}"
                    if [[ -f "${HOME}/google-cloud-sdk/path.bash.inc" ]]; then
                        # shellcheck disable=SC1091
                        source "${HOME}/google-cloud-sdk/path.bash.inc"
                    fi
                fi
                if command -v gcloud &>/dev/null; then
                    GCP_CLI_AVAILABLE=1
                    print_ok "Google Cloud SDK installed  ($(gcloud --version 2>&1 < /dev/null | head -1))"
                else
                    print_err "Google Cloud SDK installation failed — install manually."
                    echo "  https://cloud.google.com/sdk/docs/install"
                    exit 1
                fi
            else
                print_err "Google Cloud SDK is required for GCP deployment."
                echo "  Install from: https://cloud.google.com/sdk/docs/install"
                exit 1
            fi
        else
            echo "  Install from: https://cloud.google.com/sdk/docs/install"
            echo "  Install manually, then re-run: bash install.sh --gcp"
            exit 1
        fi
    fi

    # discover_system defers gcloud execution, so check login status now.
    if [[ $GCP_LOGGED_IN -eq 0 ]]; then
        local gcp_account
        gcp_account="$(gcloud config get-value account 2>/dev/null < /dev/null || echo "")"
        if [[ -n "$gcp_account" && "$gcp_account" != "(unset)" ]]; then
            GCP_LOGGED_IN=1
        fi
    fi

    # Check GOOGLE_APPLICATION_CREDENTIALS (service account key file)
    if [[ $GCP_LOGGED_IN -eq 0 && -n "${GOOGLE_APPLICATION_CREDENTIALS:-}" && -f "${GOOGLE_APPLICATION_CREDENTIALS}" ]]; then
        if gcloud auth list --filter="status:ACTIVE" --format="value(account)" 2>/dev/null </dev/null | grep -q .; then
            GCP_LOGGED_IN=1
        fi
    fi

    if [[ $GCP_LOGGED_IN -eq 1 ]]; then
        local gcp_account
        gcp_account="$(gcloud config get-value account 2>/dev/null < /dev/null || echo 'unknown')"
        print_ok "GCP credentials found  ($gcp_account)"
    else
        echo ""
        print_warn "Not logged in to GCP."
        echo ""
        if ask_yn "Log in to GCP now (device code — opens browser)?" "y"; then
            echo ""
            print_info "Logging in to GCP (device code)…"
            gcloud auth login --no-launch-browser </dev/tty
            local gcp_account
            gcp_account="$(gcloud config get-value account 2>/dev/null || echo "")"
            if [[ -n "$gcp_account" && "$gcp_account" != "(unset)" ]]; then
                GCP_LOGGED_IN=1
                print_ok "Logged in: $gcp_account"
            else
                print_err "GCP login failed — fix manually then re-run the installer."
                echo "  Run:  gcloud auth login"
                exit 1
            fi
        else
            print_err "GCP login required for remote deployment."
            echo "  Run:  gcloud auth login"
            exit 1
        fi
    fi

    # Ensure a project is set
    if [[ -z "$GCP_PROJECT" ]]; then
        GCP_PROJECT="$(gcloud config get-value project 2>/dev/null || echo "")"
        [[ "$GCP_PROJECT" == "(unset)" ]] && GCP_PROJECT=""
    fi
    if [[ -z "$GCP_PROJECT" ]]; then
        echo ""
        print_warn "No GCP project set."
        echo ""
        echo "  List your projects:  gcloud projects list"
        echo ""
        printf "  Enter your GCP project ID: "
        local proj_ans
        read -r proj_ans </dev/tty || true
        if [[ -z "$proj_ans" ]]; then
            print_err "GCP project ID is required."
            exit 1
        fi
        GCP_PROJECT="$proj_ans"
        gcloud config set project "$GCP_PROJECT" 2>/dev/null || true
    fi
    _gcp_resolve_project
    print_ok "Project: $GCP_PROJECT"
}

# Ask where to install each component.  Skipped when AUTO_YES=1.
ask_deployment_locations() {
    [[ $AUTO_YES -eq 1 ]] && return 0

    # Tester location
    if [[ $DO_INSTALL_TESTER -eq 1 ]]; then
        if [[ $DO_REMOTE_TESTER -eq 0 ]]; then
            echo ""
            echo "  ${BOLD}Where to install networker-tester?${RESET}"
            echo "    1) Locally on this machine  [default]"
            echo "    2) Remote: LAN / existing machine (SSH)"
            echo "    3) Remote: Azure VM"
            echo "    4) Remote: AWS EC2"
            echo "    5) Remote: Google Cloud GCE"
            echo ""
            printf "  Choice [1]: "
            local ans
            read -r ans </dev/tty || true
            ans="${ans:-1}"
            case "$ans" in
                2) TESTER_LOCATION="lan";   DO_REMOTE_TESTER=1 ;;
                3) TESTER_LOCATION="azure"; DO_REMOTE_TESTER=1 ;;
                4) TESTER_LOCATION="aws";   DO_REMOTE_TESTER=1 ;;
                5) TESTER_LOCATION="gcp";   DO_REMOTE_TESTER=1 ;;
            esac
        fi
        if [[ $DO_REMOTE_TESTER -eq 1 ]]; then
            case "$TESTER_LOCATION" in
                lan)   ask_lan_options "tester" ;;
                azure) ensure_azure_cli; ask_azure_options "tester" ;;
                aws)   ensure_aws_cli;   ask_aws_options   "tester" ;;
                gcp)   ensure_gcp_cli;   ask_gcp_options   "tester" ;;
            esac
        fi
    fi

    ask_packet_capture_options

    # Endpoint location
    if [[ $DO_INSTALL_ENDPOINT -eq 1 ]]; then
        if [[ $DO_REMOTE_ENDPOINT -eq 0 ]]; then
            echo ""
            echo "  ${BOLD}Where to install networker-endpoint?${RESET}"
            echo "    1) Locally on this machine  [default]"
            echo "    2) Remote: LAN / existing machine (SSH)"
            echo "    3) Remote: Azure VM"
            echo "    4) Remote: AWS EC2"
            echo "    5) Remote: Google Cloud GCE"
            echo ""
            printf "  Choice [1]: "
            local ans
            read -r ans </dev/tty || true
            ans="${ans:-1}"
            case "$ans" in
                2) ENDPOINT_LOCATION="lan";   DO_REMOTE_ENDPOINT=1 ;;
                3) ENDPOINT_LOCATION="azure"; DO_REMOTE_ENDPOINT=1 ;;
                4) ENDPOINT_LOCATION="aws";   DO_REMOTE_ENDPOINT=1 ;;
                5) ENDPOINT_LOCATION="gcp";   DO_REMOTE_ENDPOINT=1 ;;
            esac
        fi
        if [[ $DO_REMOTE_ENDPOINT -eq 1 ]]; then
            case "$ENDPOINT_LOCATION" in
                lan)   ask_lan_options "endpoint" ;;
                azure) ensure_azure_cli; ask_azure_options "endpoint" ;;
                aws)   ensure_aws_cli;   ask_aws_options   "endpoint" ;;
                gcp)   ensure_gcp_cli;   ask_gcp_options   "endpoint" ;;
            esac
        fi
    fi
}

# ── Component selection (skipped if set via CLI args or -y) ───────────────────
prompt_component_selection() {
    # Skip if the user already specified a component on the CLI or is in auto mode
    [[ $AUTO_YES -eq 1 ]]         && return 0
    [[ -n "$COMPONENT" ]]         && return 0   # "tester", "endpoint", or "both" from CLI

    print_section "What do you want to install?"
    echo ""
    echo "  1) Both  — networker-tester (client) + networker-endpoint (server)  [default]"
    echo "  2) tester only   — the diagnostic CLI for measuring HTTP/1.1, H2, H3, QUIC"
    echo "  3) endpoint only — the lightweight HTTP/QUIC test server"
    echo "  4) dashboard     — control plane + web UI (includes PostgreSQL, agent)"
    echo ""
    printf "  Choice [1]: "
    local comp_ans
    read -r comp_ans </dev/tty || true
    comp_ans="${comp_ans:-1}"
    case "$comp_ans" in
        2) COMPONENT="tester";   DO_INSTALL_TESTER=1; DO_INSTALL_ENDPOINT=0
           print_ok "Installing: networker-tester only" ;;
        3) COMPONENT="endpoint"; DO_INSTALL_TESTER=0; DO_INSTALL_ENDPOINT=1
           print_ok "Installing: networker-endpoint only" ;;
        4) COMPONENT="dashboard"; DO_INSTALL_TESTER=0; DO_INSTALL_ENDPOINT=0; DO_INSTALL_DASHBOARD=1
           print_ok "Installing: networker-dashboard (control plane + web UI)" ;;
        *) COMPONENT="both";     DO_INSTALL_TESTER=1; DO_INSTALL_ENDPOINT=1
           print_ok "Installing: networker-tester + networker-endpoint" ;;
    esac
    echo ""
}

# ── Main interactive prompt ───────────────────────────────────────────────────
prompt_main() {
    if [[ $AUTO_YES -eq 1 ]]; then
        return 0
    fi

    echo ""
    echo "${BOLD}Proceed with installation?${RESET}"
    echo ""
    echo "  1) Proceed with default installation"
    echo "  2) Customize installation steps"
    echo "  3) Cancel"
    echo ""

    local ans
    while true; do
        printf "Enter choice [1]: "
        read -r ans </dev/tty || true
        ans="${ans:-1}"
        case "$ans" in
            1)
                if [[ $CHROME_AVAILABLE -eq 0 && "$INSTALL_METHOD" == "source" \
                      && -n "$PKG_MGR" && $DO_INSTALL_TESTER -eq 1 \
                      && $DO_REMOTE_TESTER -eq 0 ]]; then
                    echo ""
                    if ask_yn "Chrome/Chromium not found — install it to enable the browser probe?" "y"; then
                        DO_CHROME_INSTALL=1
                    else
                        DO_CHROME_INSTALL=0
                        print_info "Skipping Chrome — browser probe will be disabled."
                        print_info "To enable later: install Chrome then re-run this installer."
                    fi
                fi
                ask_deployment_locations
                return 0 ;;
            2) customize_flow; return 0 ;;
            3)
                echo ""
                echo "Installation cancelled."
                exit 0
                ;;
            *)
                print_warn "Please enter 1, 2, or 3."
                ;;
        esac
    done
}

# ── Yes/No helper ─────────────────────────────────────────────────────────────
ask_yn() {
    local prompt="$1"
    local default="${2:-y}"
    local ans

    # Non-interactive mode: return the default immediately
    if [[ $AUTO_YES -eq 1 ]]; then
        [[ "$default" == "y" ]] && return 0 || return 1
    fi

    while true; do
        if [[ "$default" == "y" ]]; then
            printf "  %s [Y/n]: " "$prompt"
        else
            printf "  %s [y/N]: " "$prompt"
        fi
        read -r ans </dev/tty || true
        ans="${ans:-$default}"
        # Portable lowercase: works on bash 3.x (macOS) and bash 4+
        ans="$(printf '%s' "$ans" | tr '[:upper:]' '[:lower:]')"
        case "$ans" in
            y|yes) return 0 ;;
            n|no)  return 1 ;;
            *)     print_warn "Please enter y or n." ;;
        esac
    done
}

ask_packet_capture_options() {
    [[ $AUTO_YES -eq 1 ]] && return 0

    echo ""
    echo "  ${BOLD}Packet capture during benchmark runs?${RESET}"
    echo "    1) none  [default]"
    echo "    2) tester only"
    echo "    3) endpoint only"
    echo "    4) both tester and endpoint"
    echo ""
    printf "  Choice [1]: "
    local ans
    read -r ans </dev/tty || true
    ans="${ans:-1}"
    case "$ans" in
        2) DEPLOY_PACKET_CAPTURE_MODE="tester" ;;
        3) DEPLOY_PACKET_CAPTURE_MODE="endpoint" ;;
        4) DEPLOY_PACKET_CAPTURE_MODE="both" ;;
        *) DEPLOY_PACKET_CAPTURE_MODE="none" ;;
    esac

    if [[ "$DEPLOY_PACKET_CAPTURE_MODE" != "none" ]]; then
        DEPLOY_PACKET_CAPTURE_WRITE_PCAP="true"
        DEPLOY_PACKET_CAPTURE_WRITE_SUMMARY_JSON="true"
        if ask_yn "Install packet-capture requirements automatically when needed?" "y"; then
            DEPLOY_PACKET_CAPTURE_INSTALL_REQS="true"
        else
            DEPLOY_PACKET_CAPTURE_INSTALL_REQS="false"
        fi
        printf "  Capture interface [auto]: "
        local iface
        read -r iface </dev/tty || true
        iface="${iface:-auto}"
        if [[ ! "$iface" =~ ^[a-zA-Z0-9._-]+$ ]]; then
            print_warn "Invalid capture interface '$iface' — falling back to auto"
            iface="auto"
        fi
        DEPLOY_PACKET_CAPTURE_INTERFACE="$iface"
    fi
}

# ── Customize flow ────────────────────────────────────────────────────────────
customize_flow() {
    print_section "Customize Installation"
    echo ""

    if [[ $RELEASE_AVAILABLE -eq 1 ]]; then
        echo "  Install method:"
        echo "    1) Download binary from latest release  (fast, recommended)"
        echo "    2) Compile from source  (requires SSH key + Rust)"
        echo ""
        local method_ans
        printf "  Choice [1]: "
        read -r method_ans </dev/tty || true
        method_ans="${method_ans:-1}"
        case "$method_ans" in
            2) INSTALL_METHOD="source" ;;
            *) INSTALL_METHOD="release" ;;
        esac
        echo ""
    fi

    if [[ "$INSTALL_METHOD" == "source" ]]; then
        if [[ $GIT_AVAILABLE -eq 0 ]]; then
            if [[ -n "$PKG_MGR" ]]; then
                if ask_yn "git is not installed — install it via ${PKG_MGR}?" "y"; then
                    DO_GIT_INSTALL=1
                else
                    DO_GIT_INSTALL=0
                    echo ""
                    print_warn "Skipping git install — cargo will use its built-in libgit2 for SSH."
                    case "$PKG_MGR" in
                        brew)    echo "    brew install git" ;;
                        apt-get) echo "    sudo apt-get install git" ;;
                        dnf)     echo "    sudo dnf install git" ;;
                        pacman)  echo "    sudo pacman -S git" ;;
                        zypper)  echo "    sudo zypper install git" ;;
                        apk)     echo "    sudo apk add git" ;;
                    esac
                fi
            else
                print_warn "git is not installed."
                echo "  Install git from https://git-scm.com/ then re-run this script."
            fi
            echo ""
        fi

        if [[ $CHROME_AVAILABLE -eq 0 && $DO_INSTALL_TESTER -eq 1 ]]; then
            if [[ -n "$PKG_MGR" ]]; then
                if ask_yn "Chrome/Chromium not found — install it to enable the browser probe?" "y"; then
                    DO_CHROME_INSTALL=1
                else
                    DO_CHROME_INSTALL=0
                    echo ""
                    print_warn "Skipping Chrome — networker-tester will be compiled without the browser probe."
                    print_warn "To enable later: install Chrome then re-run this installer."
                fi
            else
                print_warn "Chrome/Chromium is not installed."
                echo "  Install from https://www.google.com/chrome/ then re-run."
            fi
            echo ""
        fi

        if [[ $RUST_EXISTS -eq 0 ]]; then
            echo ""
            if ask_yn "Install Rust via rustup (sh.rustup.rs)?" "y"; then
                DO_RUST_INSTALL=1
            else
                DO_RUST_INSTALL=0
                echo ""
                print_warn "Rust is not installed — cargo must be on PATH before proceeding."
                echo "  Install manually: https://rustup.rs"
                echo "  Then re-run with --skip-rust"
            fi
        fi
        echo ""
    fi

    echo "  Which components do you want to install?"
    echo ""
    echo "    1) Both  (networker-tester + networker-endpoint)  [default]"
    echo "    2) tester only   – the diagnostic CLI client"
    echo "    3) endpoint only – the target test server"
    echo "    4) dashboard     – control plane + web UI (PostgreSQL, agent)"
    echo ""
    local comp_ans
    printf "  Choice [1]: "
    read -r comp_ans </dev/tty || true
    comp_ans="${comp_ans:-1}"
    case "$comp_ans" in
        2) COMPONENT="tester";   DO_INSTALL_TESTER=1; DO_INSTALL_ENDPOINT=0 ;;
        3) COMPONENT="endpoint"; DO_INSTALL_TESTER=0; DO_INSTALL_ENDPOINT=1 ;;
        4) COMPONENT="dashboard"; DO_INSTALL_TESTER=0; DO_INSTALL_ENDPOINT=0; DO_INSTALL_DASHBOARD=1 ;;
        *) COMPONENT="both";     DO_INSTALL_TESTER=1; DO_INSTALL_ENDPOINT=1 ;;
    esac

    ask_deployment_locations

    echo ""
    print_section "Revised Plan"
    display_plan
    echo ""
    if ! ask_yn "Proceed with this plan?" "y"; then
        echo ""
        echo "Installation cancelled."
        exit 0
    fi
}

# ── Step helpers ──────────────────────────────────────────────────────────────
next_step() {
    STEP_NUM=$((STEP_NUM + 1))
    print_step_header "$STEP_NUM" "$@"
}

# ── Release-mode steps ────────────────────────────────────────────────────────
step_download_release() {
    local binary="$1"
    local archive="${binary}-${RELEASE_TARGET}.tar.gz"
    local tmp_dir
    tmp_dir="$(mktemp -d)"
    local ver="${NETWORKER_VERSION:-latest}"

    next_step "Download $binary"
    print_info "Fetching ${archive} (${ver})…"

    local download_ok=0

    if command -v gh &>/dev/null && gh auth status &>/dev/null 2>&1; then
        # Prefer gh CLI if available and authenticated
        if gh release download \
                --repo "$REPO_GH" \
                "$ver" \
                --pattern "${archive}" \
                --dir "${tmp_dir}" \
                --clobber 2>/dev/null; then
            download_ok=1
        fi
    fi

    if [[ $download_ok -eq 0 ]] && command -v curl &>/dev/null && [[ "$ver" != "latest" ]]; then
        # Direct curl download — no gh CLI needed
        local url="https://github.com/${REPO_GH}/releases/download/${ver}/${archive}"
        print_dim "Downloading from ${url}"
        if curl -fsSL --connect-timeout 10 -o "${tmp_dir}/${archive}" "$url" 2>/dev/null; then
            download_ok=1
        fi
    fi

    if [[ $download_ok -eq 0 ]]; then
        print_warn "Binary download failed — falling back to source compile."
        rm -rf "$tmp_dir"
        return 1
    fi

    mkdir -p "$INSTALL_DIR"
    tar xzf "${tmp_dir}/${archive}" -C "$INSTALL_DIR"
    chmod +x "${INSTALL_DIR}/${binary}"
    rm -rf "$tmp_dir"

    # Verify the binary runs (catches GLIBC mismatch, wrong arch, etc.)
    # Skip full validation for dashboard/agent — they require DB/config to start.
    local installed_ver
    case "$binary" in
        networker-dashboard|networker-agent)
            if file "${INSTALL_DIR}/${binary}" 2>/dev/null | grep -q "ELF\|Mach-O\|PE32"; then
                print_ok "$binary installed → ${INSTALL_DIR}/${binary}"
            else
                print_warn "Downloaded file is not an executable binary"
                rm -f "${INSTALL_DIR}/${binary}"
                return 1
            fi
            ;;
        *)
            if installed_ver="$("${INSTALL_DIR}/${binary}" --version 2>&1)"; then
                print_ok "$binary installed → ${INSTALL_DIR}/${binary}  ($installed_ver)"
            else
                print_warn "Downloaded binary failed to run: ${installed_ver}"
                rm -f "${INSTALL_DIR}/${binary}"
                return 1
            fi
            ;;
    esac

    # Replace any stale copies earlier in PATH that would shadow the new binary.
    local path_bin
    path_bin="$(command -v "$binary" 2>/dev/null || true)"
    if [[ -n "$path_bin" && "$path_bin" != "${INSTALL_DIR}/${binary}" ]]; then
        local path_ver
        path_ver="$("$path_bin" --version 2>/dev/null | awk '{print $NF}')"
        local new_ver
        new_ver="$("${INSTALL_DIR}/${binary}" --version 2>/dev/null | awk '{print $NF}')"
        if [[ "$path_ver" != "$new_ver" ]]; then
            print_warn "Stale $binary v${path_ver} found at $path_bin (shadows ${INSTALL_DIR}/${binary})"
            if cp "${INSTALL_DIR}/${binary}" "$path_bin" 2>/dev/null; then
                print_ok "Updated $path_bin → v${new_ver}"
            else
                print_warn "Cannot update $path_bin — run: sudo cp ${INSTALL_DIR}/${binary} $path_bin"
            fi
        fi
    fi
}

# ── Source-mode steps ─────────────────────────────────────────────────────────
step_install_git() {
    next_step "Install git"
    print_info "Installing git via ${PKG_MGR}…"
    echo ""

    case "$PKG_MGR" in
        brew)    brew install git ;;
        apt-get) sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y git ;;
        dnf)     sudo dnf install -y git ;;
        pacman)  sudo pacman -S --noconfirm git ;;
        zypper)  sudo zypper install -y git ;;
        apk)     sudo apk add git ;;
        *)
            print_err "Unknown package manager: $PKG_MGR"
            exit 1
            ;;
    esac

    if command -v git &>/dev/null; then
        GIT_AVAILABLE=1
        print_ok "git installed: $(git --version)"
    else
        print_warn "git installed, but not yet in PATH — you may need to open a new terminal."
        print_warn "Continuing; cargo will fall back to its built-in libgit2 for SSH."
    fi
}

step_install_chrome() {
    next_step "Install Chrome/Chromium (browser probe)"
    print_info "Installing Chromium via ${PKG_MGR}…"
    echo ""

    case "$PKG_MGR" in
        brew)
            brew install --cask google-chrome 2>/dev/null \
                || brew install chromium
            ;;
        apt-get)
            # Prefer Google Chrome deb over chromium-browser — on Ubuntu 24.04+
            # chromium-browser triggers a snap install that downloads ~2GB of
            # dependencies (snapd, gnome, mesa, cups) and can OOM small VMs.
            if ! command -v google-chrome &>/dev/null && ! command -v chromium-browser &>/dev/null; then
                local chrome_deb="/tmp/google-chrome-stable.deb"
                if curl -fsSL "https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb" -o "$chrome_deb" 2>/dev/null; then
                    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "$chrome_deb" < /dev/null 2>&1 || true
                    rm -f "$chrome_deb"
                fi
            fi
            # Fallback to chromium-browser if Chrome deb failed
            if ! command -v google-chrome &>/dev/null && ! command -v chromium-browser &>/dev/null; then
                sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq \
                    && (sudo DEBIAN_FRONTEND=noninteractive apt-get install -y chromium-browser 2>/dev/null \
                        || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y chromium)
            fi
            sudo DEBIAN_FRONTEND=noninteractive apt-get install -y libnss3-tools 2>/dev/null || true
            ;;
        dnf)
            sudo dnf install -y chromium
            sudo dnf install -y nss-tools 2>/dev/null || true
            ;;
        pacman)
            sudo pacman -S --noconfirm chromium
            sudo pacman -S --noconfirm nss 2>/dev/null || true
            ;;
        zypper)
            sudo zypper install -y chromium
            sudo zypper install -y mozilla-nss-tools 2>/dev/null || true
            ;;
        apk)
            sudo apk add chromium
            sudo apk add nss-tools 2>/dev/null || true
            ;;
        *)
            print_warn "Unknown package manager: $PKG_MGR"
            print_warn "Install Chrome manually from: https://www.google.com/chrome/"
            return
            ;;
    esac

    CHROME_PATH="$(detect_chrome)"
    if [[ -n "$CHROME_PATH" ]]; then
        CHROME_AVAILABLE=1
        print_ok "Chrome/Chromium ready: $CHROME_PATH"
    else
        print_warn "Chrome/Chromium installed but not yet detectable in standard paths."
        print_warn "browser probe will be compiled in; set NETWORKER_CHROME_PATH if needed."
        CHROME_AVAILABLE=1
    fi
}

detect_tshark() {
    command -v tshark 2>/dev/null && return 0
    [[ -x /opt/homebrew/bin/tshark ]] && { echo "/opt/homebrew/bin/tshark"; return 0; }
    [[ -x /usr/local/bin/tshark ]] && { echo "/usr/local/bin/tshark"; return 0; }
    return 1
}

step_install_packet_capture_tools() {
    next_step "Install packet capture tools (tshark/dumpcap)"

    if detect_tshark >/dev/null 2>&1; then
        print_ok "tshark already installed: $(detect_tshark)"
    else
        print_info "Installing packet-capture tools via ${PKG_MGR}…"
        echo ""

        case "$PKG_MGR" in
            brew)
                brew install wireshark < /dev/null
                ;;
            apt-get)
                # Pre-answer the "should non-superusers be able to capture?" prompt
                echo "wireshark-common wireshark-common/install-setuid boolean true" \
                    | sudo debconf-set-selections 2>/dev/null || true
                sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq < /dev/null
                sudo DEBIAN_FRONTEND=noninteractive apt-get install -y tshark < /dev/null
                ;;
            dnf)
                sudo dnf install -y wireshark-cli < /dev/null
                ;;
            pacman)
                sudo pacman -S --noconfirm wireshark-cli
                ;;
            zypper)
                sudo zypper install -y wireshark < /dev/null
                ;;
            apk)
                sudo apk add tshark
                ;;
            *)
                print_warn "Unknown package manager: $PKG_MGR"
                print_warn "Install tshark manually to enable packet capture."
                return
                ;;
        esac
    fi

    # Grant non-root capture permissions (Linux — add current user to wireshark group)
    if [[ "$SYS_OS" == "Linux" ]] && getent group wireshark &>/dev/null; then
        local target_user="${SUDO_USER:-$USER}"
        if ! id -nG "$target_user" 2>/dev/null | grep -qw wireshark; then
            sudo usermod -aG wireshark "$target_user" 2>/dev/null || true
            print_info "Added $target_user to wireshark group (may need re-login for effect)"
        fi
        # Also add the networker service user if it exists
        if id networker &>/dev/null; then
            sudo usermod -aG wireshark networker 2>/dev/null || true
        fi
    fi

    if detect_tshark >/dev/null 2>&1; then
        print_ok "tshark ready: $(detect_tshark)"
    else
        print_warn "Packet-capture install finished but tshark is still not detectable."
    fi
}

step_ensure_certutil() {
    [[ "$SYS_OS" == "Linux" ]] || return 0
    [[ -n "$PKG_MGR" ]]       || return 0
    command -v certutil &>/dev/null && return 0

    next_step "Install certutil (browser3 QUIC cert trust)"
    if ! ask_yn "Install certutil (NSS tools) via ${PKG_MGR}? Required for browser3 to use H3." "y"; then
        print_warn "Skipped — browser3 will fall back to H2 without certutil"
        return 0
    fi
    print_info "Installing certutil (NSS tools) via ${PKG_MGR}…"
    case "$PKG_MGR" in
        apt-get) sudo DEBIAN_FRONTEND=noninteractive apt-get install -y libnss3-tools 2>/dev/null || true ;;
        dnf)     sudo dnf install -y nss-tools 2>/dev/null || true ;;
        pacman)  sudo pacman -S --noconfirm nss 2>/dev/null || true ;;
        zypper)  sudo zypper install -y mozilla-nss-tools 2>/dev/null || true ;;
        apk)     sudo apk add nss-tools 2>/dev/null || true ;;
        *)       print_warn "Unknown PKG_MGR; install certutil manually for browser3 H3 support" ; return ;;
    esac

    if command -v certutil &>/dev/null; then
        print_ok "certutil ready"
    else
        print_warn "certutil not found after install attempt; browser3 will fall back to H2"
    fi
}

step_install_rust() {
    next_step "Install Rust via rustup"
    print_info "Downloading rustup from https://sh.rustup.rs …"
    echo ""
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path
    # shellcheck source=/dev/null
    source "${HOME}/.cargo/env"
    RUST_VER="$(rustc --version 2>/dev/null || echo "unknown")"
    echo ""
    print_ok "Rust installed: $RUST_VER"
}

step_ensure_cargo_env() {
    if [[ ! ":${PATH}:" == *":${HOME}/.cargo/bin:"* ]]; then
        if [[ -f "${HOME}/.cargo/env" ]]; then
            # shellcheck source=/dev/null
            source "${HOME}/.cargo/env"
        fi
    fi

    if ! command -v cargo &>/dev/null; then
        print_err "cargo not found — cannot install binaries."
        echo ""
        echo "  If Rust was just installed, try:"
        echo "    source \"\$HOME/.cargo/env\""
        echo "  then re-run this script."
        exit 1
    fi
}

step_cargo_install() {
    local binary="$1"
    next_step "Install $binary"

    # Try binary download first (much faster than compiling from source).
    # Skip if RELEASE_DOWNLOAD_FAILED is set (already tried and failed in step_download_release).
    if [[ -n "$RELEASE_TARGET" && "$FROM_SOURCE" -eq 0 && "${RELEASE_DOWNLOAD_FAILED:-0}" -eq 0 ]]; then
        local archive="${binary}-${RELEASE_TARGET}.tar.gz"
        local ver="${NETWORKER_VERSION:-}"
        if [[ -n "$ver" ]] && command -v curl &>/dev/null; then
            local url="https://github.com/${REPO_GH}/releases/download/${ver}/${archive}"
            local tmp_dir
            tmp_dir="$(mktemp -d)"
            print_info "Trying binary download (${ver})…"
            mkdir -p "$INSTALL_DIR"
            if curl -fsSL --connect-timeout 10 -o "${tmp_dir}/${archive}" "$url" 2>/dev/null \
               && tar xzf "${tmp_dir}/${archive}" -C "$INSTALL_DIR" 2>/dev/null; then
                chmod +x "${INSTALL_DIR}/${binary}"
                rm -rf "$tmp_dir"
                # Verify the binary (skip full run for dashboard/agent)
                case "$binary" in
                    networker-dashboard|networker-agent)
                        if file "${INSTALL_DIR}/${binary}" 2>/dev/null | grep -q "ELF\|Mach-O\|PE32"; then
                            print_ok "$binary installed → ${INSTALL_DIR}/${binary}"
                            return 0
                        fi ;;
                    *)
                        local installed_ver
                        installed_ver="$("${INSTALL_DIR}/${binary}" --version 2>&1)" && {
                            print_ok "$binary installed → ${INSTALL_DIR}/${binary}  ($installed_ver)"
                            return 0
                        } ;;
                esac
                print_warn "Downloaded binary failed validation"
                rm -f "${INSTALL_DIR}/${binary}"
            fi
            rm -rf "$tmp_dir"
            print_dim "Pre-built binary unavailable or incompatible — compiling from source."
        fi
    fi

    print_info "Building and installing $binary from source…"
    print_dim "Compiling from GitHub — may take a few minutes on first build."

    if ! command -v cc &>/dev/null && ! command -v gcc &>/dev/null && ! command -v clang &>/dev/null; then
        echo ""
        case "$SYS_OS" in
            Darwin)
                print_warn "No C linker found — install Xcode Command Line Tools then re-run:"
                echo "    xcode-select --install"
                exit 1
                ;;
            Linux)
                print_info "No C linker found — installing build tools automatically…"
                case "$PKG_MGR" in
                    apt-get) sudo apt-get update -qq 2>/dev/null; sudo DEBIAN_FRONTEND=noninteractive apt-get install -y build-essential 2>&1 | tail -3 || true ;;
                    dnf)     sudo dnf install -y gcc gcc-c++ make    2>&1 | tail -3 || true ;;
                    pacman)  sudo pacman -S --noconfirm base-devel   2>&1 | tail -3 || true ;;
                    zypper)  sudo zypper install -y gcc make          2>&1 | tail -3 || true ;;
                    apk)     sudo apk add build-base                  2>&1 | tail -3 || true ;;
                    *)
                        print_warn "Cannot auto-install: unknown package manager."
                        print_warn "Install gcc or clang then re-run this installer."
                        exit 1
                        ;;
                esac
                if ! command -v cc &>/dev/null && ! command -v gcc &>/dev/null && ! command -v clang &>/dev/null; then
                    print_err "C linker still not found after install attempt — aborting."
                    exit 1
                fi
                print_ok "Build tools installed"
                ;;
        esac
    fi
    echo ""

    local features_arg=""
    if [[ $CHROME_AVAILABLE -eq 1 && "$binary" == "networker-tester" ]]; then
        features_arg="--features browser"
        print_info "Chrome detected — compiling with browser probe support."
    fi

    _cargo_progress "Building $binary" \
        cargo install --git "$REPO_HTTPS" "$binary" --force $features_arg

    local installed_ver
    installed_ver="$("${INSTALL_DIR}/${binary}" --version 2>/dev/null || echo "unknown")"
    echo ""
    print_ok "$binary installed → ${INSTALL_DIR}/${binary}  ($installed_ver)"
}

# ──────────────────────────────────────────────────────────────────────────────
# Shared remote helpers (used by both Azure and AWS steps)
# ──────────────────────────────────────────────────────────────────────────────

# Wait for SSH to become reachable on a remote VM.
# $1 = public IP, $2 = SSH user, $3 = friendly label
_wait_for_ssh() {
    local ip="$1" user="$2" label="${3:-VM}"

    print_info "Waiting for SSH on $label ($ip)…"
    local attempts=0
    while ! ssh -o ConnectTimeout=5 \
                -o StrictHostKeyChecking=accept-new \
                -o BatchMode=yes \
                "${user}@${ip}" "echo ready" &>/dev/null; do
        attempts=$((attempts + 1))
        if [[ $attempts -gt 36 ]]; then
            echo ""
            print_err "SSH not available after 3 minutes."
            echo "  Check the VM status and try:  ssh ${user}@${ip}"
            exit 1
        fi
        printf "."
        sleep 5
    done
    echo ""
    print_ok "SSH ready"
}

# Check if a binary is already installed with the expected version.
# Returns 0 (true) if the binary exists and its version matches NETWORKER_VERSION.
# Returns 1 (false) if the binary is missing, not executable, or wrong version.
_binary_version_ok() {
    local binary="$1"
    local bin_path
    bin_path="$(command -v "$binary" 2>/dev/null || echo "${INSTALL_DIR}/${binary}")"
    if [[ ! -x "$bin_path" ]]; then
        return 1
    fi
    local installed_ver
    installed_ver="$("$bin_path" --version 2>/dev/null)" || return 1
    local expected="${NETWORKER_VERSION#v}"  # strip leading v if present
    if [[ "$installed_ver" == *"$expected"* ]]; then
        print_ok "$binary already at version $expected — skipping."
        return 0
    fi
    return 1
}

# Check installed binaries against latest GitHub release and offer self-update.
# Called once after discover_system, before the main install flow.
# Skipped in non-interactive (AUTO_YES) or deploy-config mode.
check_for_updates() {
    # Skip in non-interactive or deploy-config mode
    [[ $AUTO_YES -eq 1 ]] && return 0
    [[ -n "${DEPLOY_CONFIG_PATH:-}" ]] && return 0
    [[ -z "$NETWORKER_VERSION" ]] && return 0

    local latest="${NETWORKER_VERSION#v}"
    local outdated=()

    for binary in networker-tester networker-endpoint networker-dashboard networker-agent; do
        local bin_path
        bin_path="$(command -v "$binary" 2>/dev/null || echo "")"
        [[ -z "$bin_path" || ! -x "$bin_path" ]] && continue

        local installed_ver
        installed_ver="$("$bin_path" --version 2>/dev/null)" || continue
        if [[ "$installed_ver" != *"$latest"* ]]; then
            # Extract just the version number from output like "networker-tester 0.13.19"
            local cur_ver
            cur_ver="$(echo "$installed_ver" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
            outdated+=("${binary}:${cur_ver}")
        fi
    done

    if [[ ${#outdated[@]} -eq 0 ]]; then
        return 0
    fi

    echo ""
    print_section "Update available"
    echo ""
    echo "  Latest version: ${BOLD}${latest}${RESET}"
    echo ""
    for entry in "${outdated[@]}"; do
        local name="${entry%%:*}"
        local ver="${entry##*:}"
        printf "    %-24s  %s → %s\n" "$name" "${DIM}${ver}${RESET}" "${GREEN}${latest}${RESET}"
    done
    echo ""

    if ask_yn "Update now?" "y"; then
        for entry in "${outdated[@]}"; do
            local name="${entry%%:*}"
            if [[ "$INSTALL_METHOD" == "release" && -n "$RELEASE_TARGET" ]]; then
                mkdir -p "$INSTALL_DIR"
                if ! step_download_release "$name"; then
                    print_info "Falling back to source compile for $name…"
                    step_ensure_cargo_env
                    step_cargo_install "$name"
                fi
            else
                step_ensure_cargo_env
                step_cargo_install "$name"
            fi
        done

        # Restart systemd services if they're running
        if command -v systemctl &>/dev/null; then
            for entry in "${outdated[@]}"; do
                local name="${entry%%:*}"
                if systemctl is-active "$name" &>/dev/null; then
                    print_info "Restarting $name service…"
                    # Re-copy binary to /usr/local/bin before restart
                    local bin_path
                    bin_path="$(command -v "$name" 2>/dev/null || echo "${INSTALL_DIR}/${name}")"
                    if [[ -x "$bin_path" && "$bin_path" != "/usr/local/bin/$name" ]]; then
                        sudo systemctl stop "$name"
                        sudo cp "$bin_path" "/usr/local/bin/$name"
                        sudo chmod 755 "/usr/local/bin/$name"
                    fi
                    sudo systemctl start "$name"
                    print_ok "$name service restarted"
                fi
            done
        fi

        echo ""
        print_ok "All components updated to ${latest}"
        exit 0
    fi
}

# Install a binary on a remote Linux VM when no pre-built release binary exists.
# Uploads this installer script to the VM and runs it there with --yes, so the VM
# handles Rust install, cargo install from the public GitHub repo, and service setup.
# $1 = binary  $2 = VM IP  $3 = SSH user
_remote_bootstrap_install() {
    local binary="$1" ip="$2" user="$3"

    local comp_arg
    case "$binary" in
        networker-tester)   comp_arg="tester" ;;
        networker-endpoint) comp_arg="endpoint" ;;
        *)                  comp_arg="both" ;;
    esac

    echo ""
    print_warn "No pre-built binary for ${binary} — running installer on VM to build from source."
    print_dim  "This may take 5–10 minutes (Rust install + compile)."
    echo ""

    # Upload this installer to the VM.
    # Prefer: local file SCP → download locally then SCP → remote curl (last resort).
    local script_path="${BASH_SOURCE[0]:-}"
    local gist_url="https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh"
    local repo_url="https://raw.githubusercontent.com/irlm/networker-tester/main/install.sh"
    if [[ -f "$script_path" ]]; then
        print_info "Uploading installer to VM…"
        scp -o StrictHostKeyChecking=accept-new -q "$script_path" "${user}@${ip}:/tmp/networker-install.sh"
    else
        # Running as curl|bash — no local file. Download locally first, then SCP.
        print_info "Downloading installer locally, then uploading to VM…"
        local tmp_installer="/tmp/networker-install-$$.sh"
        if curl -fsSL "${gist_url}" -o "$tmp_installer" 2>/dev/null || \
           curl -fsSL "${repo_url}" -o "$tmp_installer" 2>/dev/null; then
            scp -o StrictHostKeyChecking=accept-new -q "$tmp_installer" "${user}@${ip}:/tmp/networker-install.sh"
            rm -f "$tmp_installer"
        else
            # Local download also failed — try on VM directly as last resort
            rm -f "$tmp_installer"
            print_warn "Local download failed — trying directly on VM…"
            ssh -o StrictHostKeyChecking=accept-new "${user}@${ip}" \
                "curl -fsSLk '${repo_url}' -o /tmp/networker-install.sh"
        fi
    fi

    # Remove OUTPUT iptables REDIRECT rules from prior installs — they break all outbound HTTPS
    # by redirecting the VM's own port 80/443 traffic to the local endpoint.
    ssh -o StrictHostKeyChecking=accept-new "${user}@${ip}" \
        "sudo iptables -t nat -D OUTPUT -p tcp --dport 80  -j REDIRECT --to-port 8080 2>/dev/null; \
         sudo iptables -t nat -D OUTPUT -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null; \
         true" < /dev/null 2>/dev/null

    print_info "Running installer on VM (the terminal will show the VM's install progress)…"
    echo ""
    # -t allocates a pseudo-TTY so the VM's spinner + colors work
    ssh -t -o StrictHostKeyChecking=accept-new "${user}@${ip}" \
        "bash /tmp/networker-install.sh ${comp_arg} -y"
}

# Download binary from GitHub release and install it on a remote host.
# Falls back to local source compilation when no release assets exist.
# $1 = binary ("networker-tester" or "networker-endpoint")
# $2 = public IP
# $3 = SSH user ("azureuser" or "ubuntu")
_remote_install_binary() {
    local binary="$1" ip="$2" user="$3"

    # Determine release version to use
    local ver="${NETWORKER_VERSION:-}"
    if [[ -z "$ver" ]]; then
        ver="$(gh release list --repo "$REPO_GH" --limit 1 --json tagName \
               -q '.[0].tagName' 2>/dev/null || echo "")"
        # Fallback: use GitHub API directly if gh CLI is not available
        if [[ -z "$ver" ]]; then
            ver="$(curl -fsSL "https://api.github.com/repos/${REPO_GH}/releases/latest" 2>/dev/null \
                   | grep '"tag_name"' | head -1 | sed 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/')"
        fi
    fi

    # Detect remote architecture (needed for both download path and source fallback)
    local remote_arch
    remote_arch="$(ssh -o StrictHostKeyChecking=accept-new "${user}@${ip}" "uname -m" 2>/dev/null || echo "x86_64")"

    # Check whether release has pre-built assets; compile locally if not.
    local has_assets=""
    if [[ -n "$ver" ]]; then
        has_assets="$(gh release view --repo "$REPO_GH" "$ver" --json assets \
                      -q '[.assets[].name] | join(" ")' 2>/dev/null || echo "")"
        # Fallback: use GitHub API directly if gh CLI is not available
        if [[ -z "$has_assets" ]]; then
            has_assets="$(curl -fsSL "https://api.github.com/repos/${REPO_GH}/releases/tags/${ver}" 2>/dev/null \
                          | grep '"name"' | sed 's/.*"name":[[:space:]]*"\([^"]*\)".*/\1/' | tr '\n' ' ')"
        fi
    fi
    if [[ -z "$ver" ]] || ! printf '%s' "$has_assets" | grep -q "${binary}-"; then
        print_warn "Release ${ver:-unknown} has no pre-built binaries for ${binary}."
        _remote_bootstrap_install "$binary" "$ip" "$user"
        return
    fi

    # Map remote arch to Rust target triple
    local remote_target
    case "$remote_arch" in
        x86_64)        remote_target="x86_64-unknown-linux-musl" ;;
        aarch64|arm64) remote_target="aarch64-unknown-linux-musl" ;;
        *)             remote_target="x86_64-unknown-linux-musl" ;;
    esac

    local archive="${binary}-${remote_target}.tar.gz"
    local tmp_dir
    tmp_dir="$(mktemp -d)"

    print_info "Downloading ${archive} from GitHub release (${ver})…"
    if gh release download \
            --repo "$REPO_GH" \
            --tag "$ver" \
            --pattern "${archive}" \
            --dir "${tmp_dir}" \
            --clobber 2>/dev/null; then

        tar xzf "${tmp_dir}/${archive}" -C "${tmp_dir}"
        chmod +x "${tmp_dir}/${binary}"

        print_info "Uploading binary to VM…"
        scp -o StrictHostKeyChecking=accept-new -q \
            "${tmp_dir}/${binary}" \
            "${user}@${ip}:/tmp/${binary}"
        rm -rf "${tmp_dir}"

        ssh -o StrictHostKeyChecking=accept-new "${user}@${ip}" \
            "sudo mv /tmp/${binary} /usr/local/bin/${binary} && \
             sudo chmod +x /usr/local/bin/${binary}"
    else
        rm -rf "${tmp_dir}"
        # Fallback: download directly on the remote VM
        print_info "Downloading directly on VM (${ver})…"
        if ! ssh -o StrictHostKeyChecking=accept-new "${user}@${ip}" \
            "curl -fsSL https://github.com/${REPO_GH}/releases/download/${ver}/${archive} \
               -o /tmp/${archive} && \
             tar xzf /tmp/${archive} -C /tmp && \
             sudo mv /tmp/${binary} /usr/local/bin/${binary} && \
             sudo chmod +x /usr/local/bin/${binary} && \
             rm /tmp/${archive}"; then
            echo ""
            print_err "Failed to download ${archive} from release ${ver}."
            echo "  Check:  https://github.com/${REPO_GH}/releases/${ver}"
            exit 1
        fi
    fi

    local remote_ver
    remote_ver="$(ssh -o StrictHostKeyChecking=accept-new "${user}@${ip}" \
        "/usr/local/bin/${binary} --version 2>/dev/null" || echo "unknown")"
    print_ok "$binary installed on VM  ($remote_ver)"
}

# Create a systemd service for networker-endpoint on a remote host.
# $1 = public IP, $2 = SSH user
_remote_create_endpoint_service() {
    local ip="$1" user="$2"

    ssh -o StrictHostKeyChecking=accept-new "${user}@${ip}" bash <<'REMOTE'
sudo useradd --system --no-create-home --shell /usr/sbin/nologin networker 2>/dev/null || true

sudo tee /etc/systemd/system/networker-endpoint.service > /dev/null <<'UNIT'
[Unit]
Description=Networker Endpoint
After=network.target

[Service]
User=networker
ExecStart=/usr/local/bin/networker-endpoint
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
UNIT

sudo systemctl daemon-reload
sudo systemctl enable networker-endpoint
sudo systemctl start networker-endpoint

# Redirect standard ports 80/443 to the unprivileged service ports 8080/8443.
# This lets browsers reach the landing page via http://IP and https://IP.
if command -v iptables &>/dev/null; then
    sudo iptables -t nat -C PREROUTING -p tcp --dport 80  -j REDIRECT --to-port 8080 2>/dev/null || \
        sudo iptables -t nat -A PREROUTING -p tcp --dport 80  -j REDIRECT --to-port 8080
    sudo iptables -t nat -C PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || \
        sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443
    # Remove any OUTPUT REDIRECT rules from prior installs — they break outbound HTTPS.
    sudo iptables -t nat -D OUTPUT -p tcp --dport 80  -j REDIRECT --to-port 8080 2>/dev/null || true
    sudo iptables -t nat -D OUTPUT -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || true
    # Persist rules across reboots if iptables-persistent is available.
    if command -v netfilter-persistent &>/dev/null; then
        sudo netfilter-persistent save 2>/dev/null || true
    elif command -v iptables-save &>/dev/null; then
        sudo mkdir -p /etc/iptables 2>/dev/null || true
        sudo sh -c 'iptables-save > /etc/iptables/rules.v4' 2>/dev/null || \
            sudo sh -c 'iptables-save > /etc/iptables.rules' 2>/dev/null || true
    fi
fi
REMOTE

    sleep 2
    print_ok "networker-endpoint service enabled and started"
}

# Set up networker-endpoint as a systemd service on the LOCAL machine (Linux only).
# Requires sudo. Idempotent — safe to run multiple times.
step_setup_endpoint_service() {
    next_step "Set up networker-endpoint systemd service"

    if [[ $SKIP_SERVICE -eq 1 ]]; then
        print_info "Service setup skipped (--no-service)"
        return 0
    fi
    if [[ "$SYS_OS" != "Linux" ]]; then
        print_info "Systemd service setup is Linux-only — skipping."
        print_dim  "  On macOS: run networker-endpoint manually in a terminal."
        return 0
    fi
    if ! command -v systemctl &>/dev/null; then
        print_info "systemd not found — skipping service setup."
        return 0
    fi

    # Locate the binary (cargo install puts it in ~/.cargo/bin)
    local binary_path="${INSTALL_DIR}/networker-endpoint"
    if [[ ! -x "$binary_path" ]]; then
        binary_path="/usr/local/bin/networker-endpoint"
    fi

    # Copy to /usr/local/bin so the 'networker' system user can execute it.
    # The cargo install dir (~/.cargo/bin) may not be traversable by system users
    # whose home dir is restricted (Ubuntu 22.04 default: drwx------).
    if [[ "$binary_path" != "/usr/local/bin/networker-endpoint" && -x "$binary_path" ]]; then
        # Stop the service first if running — can't overwrite a running binary ("Text file busy")
        if systemctl is-active networker-endpoint &>/dev/null; then
            sudo systemctl stop networker-endpoint
        fi
        sudo cp "$binary_path" /usr/local/bin/networker-endpoint
        sudo chmod 755 /usr/local/bin/networker-endpoint
        binary_path="/usr/local/bin/networker-endpoint"
    fi

    sudo useradd --system --no-create-home --shell /usr/sbin/nologin networker 2>/dev/null || true

    sudo tee /etc/systemd/system/networker-endpoint.service > /dev/null <<UNIT
[Unit]
Description=Networker Endpoint
After=network.target

[Service]
User=networker
ExecStart=${binary_path}
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
UNIT

    sudo systemctl daemon-reload
    sudo systemctl enable networker-endpoint
    sudo systemctl start networker-endpoint

    # Redirect privileged ports 80/443 → 8080/8443 so browsers can reach the server.
    # Skip when installing as part of dashboard — nginx handles 80/443 instead.
    if command -v iptables &>/dev/null && [[ ${DO_INSTALL_DASHBOARD:-0} -eq 0 ]]; then
        sudo iptables -t nat -C PREROUTING -p tcp --dport 80  -j REDIRECT --to-port 8080 2>/dev/null || \
            sudo iptables -t nat -A PREROUTING -p tcp --dport 80  -j REDIRECT --to-port 8080
        sudo iptables -t nat -C PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || \
            sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443
        # Remove any OUTPUT REDIRECT rules from prior installs — they break outbound HTTPS.
        sudo iptables -t nat -D OUTPUT -p tcp --dport 80  -j REDIRECT --to-port 8080 2>/dev/null || true
        sudo iptables -t nat -D OUTPUT -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || true
        if command -v netfilter-persistent &>/dev/null; then
            sudo netfilter-persistent save 2>/dev/null || true
        elif command -v iptables-save &>/dev/null; then
            sudo mkdir -p /etc/iptables 2>/dev/null || true
            sudo sh -c 'iptables-save > /etc/iptables/rules.v4' 2>/dev/null || \
                sudo sh -c 'iptables-save > /etc/iptables.rules' 2>/dev/null || true
        fi
    fi

    sleep 2
    print_ok "networker-endpoint service started — auto-starts on boot"
}

# ── Dashboard installation steps ─────────────────────────────────────────────

# Install PostgreSQL (apt/yum/brew), create DB user + database.
step_install_postgresql() {
    next_step "Install and configure PostgreSQL"

    if command -v psql &>/dev/null; then
        print_info "PostgreSQL client already installed"
    else
        case "$PKG_MGR" in
            apt-get)
                sudo apt-get update -qq < /dev/null
                sudo DEBIAN_FRONTEND=noninteractive apt-get install -y postgresql postgresql-contrib < /dev/null
                ;;
            dnf)
                sudo dnf install -y postgresql-server postgresql < /dev/null
                sudo postgresql-setup --initdb 2>/dev/null || true
                ;;
            brew)
                brew install postgresql@16 < /dev/null
                brew services start postgresql@16 < /dev/null
                ;;
            *)
                print_err "No supported package manager found for PostgreSQL install"
                return 1
                ;;
        esac
    fi

    # Ensure the service is running
    if [[ "$SYS_OS" == "Linux" ]] && command -v systemctl &>/dev/null; then
        sudo systemctl enable postgresql 2>/dev/null || true
        sudo systemctl start postgresql 2>/dev/null || true
    fi

    # Wait briefly for PostgreSQL to accept connections
    local retries=0
    while ! sudo -u postgres psql -c "SELECT 1" &>/dev/null && [[ $retries -lt 10 ]]; do
        sleep 1
        retries=$((retries + 1))
    done

    # Create user and database (idempotent)
    sudo -u postgres createuser networker 2>/dev/null || true
    sudo -u postgres createdb networker_dashboard -O networker 2>/dev/null || true

    # Generate a random password for the PostgreSQL user
    DASHBOARD_DB_PASSWORD="$(head -c 64 /dev/urandom | LC_ALL=C tr -dc 'A-Za-z0-9' | head -c 24)"
    sudo -u postgres psql -c "ALTER USER networker WITH PASSWORD '${DASHBOARD_DB_PASSWORD}';" 2>/dev/null || true

    # Create tester result tables (normally created by networker-tester on first run,
    # but the dashboard's Runs page queries them and returns 500 if they don't exist)
    sudo -u postgres psql -d networker_dashboard 2>/dev/null << 'TESTER_SCHEMA'
CREATE TABLE IF NOT EXISTS _schema_versions (
    version INTEGER PRIMARY KEY, applied_at TIMESTAMP WITH TIME ZONE DEFAULT NOW());
INSERT INTO _schema_versions (version) VALUES (1) ON CONFLICT DO NOTHING;
-- Match the tester's V001 schema exactly so backend.save() works
CREATE TABLE IF NOT EXISTS TestRun (
    RunId          UUID            NOT NULL,
    StartedAt      TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    FinishedAt     TIMESTAMPTZ     NULL,
    TargetUrl      VARCHAR(2048)   NOT NULL DEFAULT '',
    TargetHost     VARCHAR(255)    NOT NULL,
    Modes          VARCHAR(200)    NOT NULL DEFAULT '',
    TotalRuns      INT             NOT NULL DEFAULT 1,
    Concurrency    INT             NOT NULL DEFAULT 1,
    TimeoutMs      BIGINT          NOT NULL DEFAULT 30000,
    ClientOs       VARCHAR(50)     NOT NULL DEFAULT '',
    ClientVersion  VARCHAR(50)     NOT NULL DEFAULT '',
    SuccessCount   INT             NOT NULL DEFAULT 0,
    FailureCount   INT             NOT NULL DEFAULT 0,
    CONSTRAINT PK_TestRun PRIMARY KEY (RunId)
);
CREATE TABLE IF NOT EXISTS RequestAttempt (
    AttemptId     UUID            NOT NULL,
    RunId         UUID            NOT NULL,
    Protocol      VARCHAR(20)     NOT NULL,
    SequenceNum   INT             NOT NULL,
    StartedAt     TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    FinishedAt    TIMESTAMPTZ     NULL,
    Success       BOOLEAN         NOT NULL DEFAULT FALSE,
    ErrorMessage  TEXT            NULL,
    RetryCount    INT             NOT NULL DEFAULT 0,
    extra_json    JSONB           NULL,
    CONSTRAINT PK_RequestAttempt PRIMARY KEY (AttemptId),
    CONSTRAINT FK_Attempt_Run    FOREIGN KEY (RunId)
        REFERENCES TestRun (RunId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS DnsResult (
    DnsId         UUID            NOT NULL DEFAULT gen_random_uuid(),
    AttemptId     UUID            NOT NULL,
    QueryName     VARCHAR(255)    NOT NULL,
    ResolvedIPs   VARCHAR(1024)   NOT NULL DEFAULT '',
    DurationMs    DOUBLE PRECISION NOT NULL DEFAULT 0,
    StartedAt     TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    Success       BOOLEAN         NOT NULL DEFAULT TRUE,
    CONSTRAINT PK_DnsResult   PRIMARY KEY (DnsId),
    CONSTRAINT FK_Dns_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS TcpResult (
    TcpId         UUID            NOT NULL DEFAULT gen_random_uuid(),
    AttemptId     UUID            NOT NULL,
    LocalAddr     VARCHAR(64)     NULL,
    RemoteAddr    VARCHAR(64)     NOT NULL DEFAULT '',
    ConnectDurationMs DOUBLE PRECISION NOT NULL DEFAULT 0,
    AttemptCount  INT             NOT NULL DEFAULT 1,
    StartedAt     TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    Success       BOOLEAN         NOT NULL DEFAULT TRUE,
    MssBytesEstimate INT          NULL,
    RttEstimateMs DOUBLE PRECISION NULL,
    CONSTRAINT PK_TcpResult   PRIMARY KEY (TcpId),
    CONSTRAINT FK_Tcp_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS TlsResult (
    TlsId         UUID            NOT NULL DEFAULT gen_random_uuid(),
    AttemptId     UUID            NOT NULL,
    ProtocolVersion VARCHAR(20)   NOT NULL DEFAULT '',
    CipherSuite   VARCHAR(100)    NOT NULL DEFAULT '',
    AlpnNegotiated VARCHAR(20)    NULL,
    CertSubject   VARCHAR(512)    NULL,
    CertIssuer    VARCHAR(512)    NULL,
    CertExpiry    TIMESTAMPTZ     NULL,
    HandshakeDurationMs DOUBLE PRECISION NOT NULL DEFAULT 0,
    StartedAt     TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    Success       BOOLEAN         NOT NULL DEFAULT TRUE,
    CONSTRAINT PK_TlsResult   PRIMARY KEY (TlsId),
    CONSTRAINT FK_Tls_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS HttpResult (
    HttpId        UUID            NOT NULL DEFAULT gen_random_uuid(),
    AttemptId     UUID            NOT NULL,
    NegotiatedVersion VARCHAR(20) NOT NULL DEFAULT '',
    StatusCode    INT             NOT NULL DEFAULT 0,
    HeadersSizeBytes INT          NOT NULL DEFAULT 0,
    BodySizeBytes INT             NOT NULL DEFAULT 0,
    TtfbMs        DOUBLE PRECISION NOT NULL DEFAULT 0,
    TotalDurationMs DOUBLE PRECISION NOT NULL DEFAULT 0,
    ThroughputMbps DOUBLE PRECISION NULL,
    PayloadBytes  BIGINT          NULL,
    RedirectCount INT             NOT NULL DEFAULT 0,
    StartedAt     TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    CONSTRAINT PK_HttpResult   PRIMARY KEY (HttpId),
    CONSTRAINT FK_Http_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS UdpResult (
    UdpId         UUID            NOT NULL DEFAULT gen_random_uuid(),
    AttemptId     UUID            NOT NULL,
    ProbeCount    INT             NOT NULL DEFAULT 0,
    SuccessCount  INT             NOT NULL DEFAULT 0,
    LossPercent   DOUBLE PRECISION NOT NULL DEFAULT 0,
    RttMinMs      DOUBLE PRECISION NOT NULL DEFAULT 0,
    RttAvgMs      DOUBLE PRECISION NOT NULL DEFAULT 0,
    RttMaxMs      DOUBLE PRECISION NOT NULL DEFAULT 0,
    RttP95Ms      DOUBLE PRECISION NOT NULL DEFAULT 0,
    JitterMs      DOUBLE PRECISION NOT NULL DEFAULT 0,
    StartedAt     TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    CONSTRAINT PK_UdpResult   PRIMARY KEY (UdpId),
    CONSTRAINT FK_Udp_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS ErrorRecord (
    ErrorId       UUID            NOT NULL DEFAULT gen_random_uuid(),
    AttemptId     UUID            NOT NULL,
    ErrorCategory VARCHAR(50)     NOT NULL DEFAULT '',
    ErrorMessage  TEXT            NOT NULL DEFAULT '',
    ErrorDetail   TEXT            NULL,
    OccurredAt    TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    CONSTRAINT PK_ErrorRecord  PRIMARY KEY (ErrorId),
    CONSTRAINT FK_Error_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS ServerTimingResult (
    TimingId      UUID            NOT NULL DEFAULT gen_random_uuid(),
    AttemptId     UUID            NOT NULL,
    ServerTimestamp TIMESTAMPTZ   NULL,
    ClockSkewMs   DOUBLE PRECISION NULL,
    ServerVersion VARCHAR(50)     NULL,
    CONSTRAINT PK_ServerTiming PRIMARY KEY (TimingId),
    CONSTRAINT FK_Timing_Attempt FOREIGN KEY (AttemptId)
        REFERENCES RequestAttempt (AttemptId) ON DELETE CASCADE
);
GRANT ALL ON ALL TABLES IN SCHEMA public TO networker;
GRANT ALL ON ALL SEQUENCES IN SCHEMA public TO networker;
TESTER_SCHEMA

    print_ok "PostgreSQL configured — database: networker_dashboard, user: networker"
}

# Install Node.js (for building the React frontend).
step_install_nodejs() {
    next_step "Install Node.js"

    if command -v node &>/dev/null; then
        local node_ver
        node_ver="$(node --version 2>/dev/null || echo "")"
        print_info "Node.js already installed: $node_ver"
        return 0
    fi

    case "$PKG_MGR" in
        apt-get)
            # Remove system nodejs if present (conflicts with NodeSource)
            sudo apt-get remove -y nodejs npm < /dev/null 2>/dev/null || true
            # Download setup script first (avoid pipe stdin issues with curl|bash)
            local ns_setup="/tmp/nodesource_setup.sh"
            curl -fsSL https://deb.nodesource.com/setup_24.x -o "$ns_setup"
            sudo -E bash "$ns_setup" < /dev/null 2>&1
            rm -f "$ns_setup"
            sudo apt-get update -qq < /dev/null
            sudo DEBIAN_FRONTEND=noninteractive apt-get install -y nodejs < /dev/null
            ;;
        dnf)
            sudo dnf install -y nodejs npm < /dev/null
            ;;
        brew)
            brew install node < /dev/null
            ;;
        *)
            print_err "No supported package manager found for Node.js install"
            return 1
            ;;
    esac

    print_ok "Node.js installed: $(node --version 2>/dev/null)"
}

# Install cloud CLIs (Azure, AWS, GCP) for the dashboard to deploy endpoints.
step_install_cloud_clis() {
    next_step "Install cloud CLIs"

    # Azure CLI
    if command -v az &>/dev/null; then
        print_info "Azure CLI already installed: $(az version --query '"azure-cli"' -o tsv 2>/dev/null)"
    else
        print_info "Installing Azure CLI…"
        case "$PKG_MGR" in
            apt-get)
                local az_setup="/tmp/azure_cli_setup.sh"
                curl -sL https://aka.ms/InstallAzureCLIDeb -o "$az_setup"
                sudo DEBIAN_FRONTEND=noninteractive bash "$az_setup" < /dev/null 2>&1
                rm -f "$az_setup"
                ;;
            dnf)
                sudo rpm --import https://packages.microsoft.com/keys/microsoft.asc < /dev/null
                sudo dnf install -y azure-cli < /dev/null 2>&1
                ;;
            brew)
                brew install azure-cli < /dev/null 2>&1
                ;;
        esac
        if command -v az &>/dev/null; then
            print_ok "Azure CLI installed"
        else
            print_warn "Azure CLI installation failed — install manually"
        fi
    fi

    # unzip is needed for AWS CLI install — ensure it's present first
    if ! command -v unzip &>/dev/null; then
        case "$PKG_MGR" in
            apt-get) sudo DEBIAN_FRONTEND=noninteractive apt-get install -y unzip -qq < /dev/null 2>&1 ;;
            dnf)     sudo dnf install -y unzip < /dev/null 2>&1 ;;
        esac
    fi

    # AWS CLI
    if command -v aws &>/dev/null; then
        print_info "AWS CLI already installed: $(aws --version 2>/dev/null | head -1)"
    else
        print_info "Installing AWS CLI…"
        local tmp_aws
        tmp_aws="$(mktemp -d)"
        if curl -fsSL "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o "${tmp_aws}/awscliv2.zip" 2>/dev/null; then
            (cd "$tmp_aws" && unzip -qo awscliv2.zip && sudo ./aws/install 2>/dev/null) || true
        fi
        rm -rf "$tmp_aws"
        if command -v aws &>/dev/null; then
            print_ok "AWS CLI installed"
        else
            print_warn "AWS CLI installation failed — install manually"
        fi
    fi

    # GCP CLI
    if command -v gcloud &>/dev/null; then
        print_info "GCP CLI already installed: $(gcloud --version 2>/dev/null | head -1)"
    else
        print_info "Installing GCP CLI…"
        case "$PKG_MGR" in
            apt-get)
                if [[ ! -f /usr/share/keyrings/cloud.google.gpg ]]; then
                    curl -fsSL https://packages.cloud.google.com/apt/doc/apt-key.gpg \
                        | sudo gpg --dearmor -o /usr/share/keyrings/cloud.google.gpg 2>/dev/null || true
                    echo "deb [signed-by=/usr/share/keyrings/cloud.google.gpg] https://packages.cloud.google.com/apt cloud-sdk main" \
                        | sudo tee /etc/apt/sources.list.d/google-cloud-sdk.list > /dev/null
                fi
                sudo apt-get update -qq < /dev/null 2>&1
                sudo DEBIAN_FRONTEND=noninteractive apt-get install -y google-cloud-cli -qq < /dev/null 2>&1
                ;;
            dnf)
                sudo tee /etc/yum.repos.d/google-cloud-sdk.repo > /dev/null << 'GCPREPO'
[google-cloud-cli]
name=Google Cloud CLI
baseurl=https://packages.cloud.google.com/yum/repos/cloud-sdk-el9-x86_64
enabled=1
gpgcheck=1
repo_gpgcheck=0
gpgkey=https://packages.cloud.google.com/yum/doc/rpm-package-key.gpg
GCPREPO
                sudo dnf install -y google-cloud-cli < /dev/null 2>&1
                ;;
            brew)
                brew install google-cloud-sdk < /dev/null 2>&1
                ;;
        esac
        if command -v gcloud &>/dev/null; then
            print_ok "GCP CLI installed"
        else
            print_warn "GCP CLI installation failed — install manually"
        fi
    fi

    # jq is needed for credential helpers
    if ! command -v jq &>/dev/null; then
        case "$PKG_MGR" in
            apt-get) sudo DEBIAN_FRONTEND=noninteractive apt-get install -y jq -qq < /dev/null 2>&1 ;;
            dnf)     sudo dnf install -y jq < /dev/null 2>&1 ;;
            brew)    brew install jq < /dev/null 2>&1 ;;
        esac
    fi

}

# Build the React frontend and copy to /opt/networker/dashboard.
step_build_frontend() {
    next_step "Build dashboard frontend"

    local dashboard_src=""
    # Check common locations for the dashboard source
    if [[ -f "./dashboard/package.json" ]]; then
        dashboard_src="./dashboard"
    elif [[ -f "/tmp/networker-dashboard-src/dashboard/package.json" ]]; then
        dashboard_src="/tmp/networker-dashboard-src/dashboard"
    else
        # Clone the repo at the release tag to get matching frontend source
        local tmp_src="/tmp/networker-dashboard-src"
        rm -rf "$tmp_src"
        local clone_ref="${NETWORKER_VERSION:-main}"
        git clone --depth 1 --branch "$clone_ref" "${REPO_HTTPS}.git" "$tmp_src" < /dev/null 2>&1 || {
            # Fallback to main if tag doesn't exist
            git clone --depth 1 "${REPO_HTTPS}.git" "$tmp_src" < /dev/null 2>&1 || {
                print_err "Failed to clone repo for frontend source"
                return 1
            }
        }
        dashboard_src="$tmp_src/dashboard"
    fi

    (
        cd "$dashboard_src"
        npm install --legacy-peer-deps < /dev/null 2>&1
        npm run build < /dev/null 2>&1
    ) || {
        print_err "Frontend build failed"
        return 1
    }

    sudo mkdir -p /opt/networker/dashboard
    sudo cp -r "${dashboard_src}/dist/." /opt/networker/dashboard/
    sudo chown -R networker:networker /opt/networker/dashboard 2>/dev/null || true

    print_ok "Frontend built and installed to /opt/networker/dashboard"
}

# Write /etc/networker-dashboard.env with DB URL, admin password, JWT secret, port.
step_write_dashboard_env() {
    next_step "Write dashboard environment file"

    local admin_pw="${DASHBOARD_ADMIN_PASSWORD:-}"
    if [[ -z "$admin_pw" ]]; then
        # Generate a random temporary password — user must change on first login
        admin_pw="$(head -c 64 /dev/urandom | LC_ALL=C tr -dc 'A-HJ-NP-Za-km-z2-9' | head -c 16)"
        DASHBOARD_TEMP_PASSWORD="$admin_pw"
    fi

    local dashboard_port="${DASHBOARD_PORT:-3000}"
    local jwt_secret
    jwt_secret="$(head -c 32 /dev/urandom | base64 | tr -d '=/+' | head -c 32)"

    local db_pw="${DASHBOARD_DB_PASSWORD:-networker}"
    sudo tee /etc/networker-dashboard.env > /dev/null <<ENVFILE
DASHBOARD_DB_URL=postgres://networker:${db_pw}@127.0.0.1:5432/networker_dashboard
DASHBOARD_ADMIN_PASSWORD=${admin_pw}
DASHBOARD_JWT_SECRET=${jwt_secret}
DASHBOARD_PORT=${dashboard_port}
DASHBOARD_BIND_ADDR=127.0.0.1
DASHBOARD_STATIC_DIR=/opt/networker/dashboard
INSTALL_SH_PATH=/opt/networker/install.sh
ENVFILE

    sudo chmod 600 /etc/networker-dashboard.env

    # Copy install.sh to a persistent location for the dashboard to use for deployments
    local script_path="${BASH_SOURCE[0]:-$0}"
    if [[ -f "$script_path" ]]; then
        sudo cp "$script_path" /opt/networker/install.sh
        sudo chmod +x /opt/networker/install.sh
    fi

    print_ok "Environment file written to /etc/networker-dashboard.env"
}

# Set up networker-dashboard as a systemd service.
step_setup_dashboard_service() {
    next_step "Set up networker-dashboard systemd service"

    if [[ $SKIP_SERVICE -eq 1 ]]; then
        print_info "Service setup skipped (--no-service)"
        return 0
    fi
    if [[ "$SYS_OS" != "Linux" ]]; then
        print_info "Systemd service setup is Linux-only — skipping."
        print_dim  "  On macOS: run networker-dashboard manually."
        return 0
    fi
    if ! command -v systemctl &>/dev/null; then
        print_info "systemd not found — skipping service setup."
        return 0
    fi

    # Locate the binary
    local binary_path="${INSTALL_DIR}/networker-dashboard"
    if [[ ! -x "$binary_path" ]]; then
        binary_path="/usr/local/bin/networker-dashboard"
    fi

    # Copy to /usr/local/bin
    if [[ "$binary_path" != "/usr/local/bin/networker-dashboard" && -x "$binary_path" ]]; then
        if systemctl is-active networker-dashboard &>/dev/null; then
            sudo systemctl stop networker-dashboard
        fi
        sudo cp "$binary_path" /usr/local/bin/networker-dashboard
        sudo chmod 755 /usr/local/bin/networker-dashboard
        binary_path="/usr/local/bin/networker-dashboard"
    fi

    # Also install the agent binary
    local agent_path="${INSTALL_DIR}/networker-agent"
    if [[ -x "$agent_path" && "$agent_path" != "/usr/local/bin/networker-agent" ]]; then
        sudo cp "$agent_path" /usr/local/bin/networker-agent
        sudo chmod 755 /usr/local/bin/networker-agent
    fi

    # Also install the tester binary (agent shells out to it for probe jobs)
    local tester_path="${INSTALL_DIR}/networker-tester"
    if [[ -x "$tester_path" && "$tester_path" != "/usr/local/bin/networker-tester" ]]; then
        sudo cp "$tester_path" /usr/local/bin/networker-tester
        sudo chmod 755 /usr/local/bin/networker-tester
    fi

    sudo useradd --system --no-create-home --shell /usr/sbin/nologin networker 2>/dev/null || true

    sudo tee /etc/systemd/system/networker-dashboard.service > /dev/null <<UNIT
[Unit]
Description=Networker Dashboard
After=network.target postgresql.service

[Service]
User=$(whoami)
WorkingDirectory=/tmp
EnvironmentFile=/etc/networker-dashboard.env
ExecStart=${binary_path}
Restart=always
RestartSec=5
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
UNIT

    sudo systemctl daemon-reload
    sudo systemctl enable networker-dashboard
    sudo systemctl start networker-dashboard

    # Open firewall for dashboard port
    local dashboard_port
    dashboard_port="$(grep DASHBOARD_PORT /etc/networker-dashboard.env 2>/dev/null | cut -d= -f2)"
    dashboard_port="${dashboard_port:-3000}"

    if command -v ufw &>/dev/null; then
        sudo ufw allow "$dashboard_port"/tcp 2>/dev/null || true
    elif command -v firewall-cmd &>/dev/null; then
        sudo firewall-cmd --permanent --add-port="${dashboard_port}/tcp" 2>/dev/null || true
        sudo firewall-cmd --reload 2>/dev/null || true
    fi

    sleep 2
    print_ok "networker-dashboard service started on port ${dashboard_port} — auto-starts on boot"
}

# Set up nginx as a reverse proxy for the dashboard (ports 80/443 → 3000).
step_setup_nginx_proxy() {
    next_step "Set up nginx reverse proxy for dashboard"

    if [[ "$SYS_OS" != "Linux" ]]; then
        print_info "nginx proxy is Linux-only — skipping."
        return 0
    fi

    # Remove any iptables redirects from the endpoint installer (port 80→8080, 443→8443)
    # These conflict with nginx binding to ports 80/443
    if sudo iptables -t nat -L PREROUTING -n 2>/dev/null | grep -q "redir ports 8080"; then
        sudo iptables -t nat -D PREROUTING -p tcp --dport 80 -j REDIRECT --to-port 8080 2>/dev/null || true
        sudo iptables -t nat -D PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null || true
        print_info "Removed iptables port redirects (80→8080, 443→8443)"
    fi

    # Check if port 80 is already in use by something other than nginx
    if ss -tlnp 2>/dev/null | grep -q ':80 ' && ! command -v nginx &>/dev/null; then
        print_warn "Port 80 already in use — skipping nginx proxy setup."
        return 0
    fi

    # Install nginx if not present (system package is fine for reverse proxy)
    if ! command -v nginx &>/dev/null; then
        case "$PKG_MGR" in
            apt-get) sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nginx < /dev/null 2>&1 ;;
            dnf)     sudo dnf install -y nginx < /dev/null 2>&1 ;;
        esac
    fi

    if ! command -v nginx &>/dev/null; then
        print_warn "nginx installation failed — dashboard available on port 3000 only."
        return 0
    fi

    local dashboard_port
    dashboard_port="$(grep DASHBOARD_PORT /etc/networker-dashboard.env 2>/dev/null | cut -d= -f2)"
    dashboard_port="${dashboard_port:-3000}"
    local server_name="${DASHBOARD_FQDN:-_}"

    # Write reverse proxy config
    sudo tee /etc/nginx/conf.d/networker-dashboard.conf > /dev/null <<NGINXCONF
server {
    listen 80;
    server_name ${server_name};

    # Let's Encrypt challenge path (must not be proxied)
    location ^~ /.well-known/acme-challenge/ {
        root /var/www/html;
        default_type "text/plain";
    }

    location / {
        proxy_pass http://127.0.0.1:${dashboard_port};
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }

    location /ws/ {
        proxy_pass http://127.0.0.1:${dashboard_port};
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_read_timeout 86400;
    }
}
NGINXCONF

    # Remove default site if it exists
    sudo rm -f /etc/nginx/sites-enabled/default 2>/dev/null || true
    sudo mkdir -p /var/www/html

    if sudo nginx -t 2>&1; then
        sudo systemctl enable nginx 2>/dev/null || true
        sudo systemctl reload nginx 2>/dev/null || sudo systemctl start nginx
        DASHBOARD_NGINX_CONFIGURED=1
        print_ok "nginx reverse proxy configured — dashboard on port 80"
    else
        print_warn "nginx config test failed — dashboard available on port ${dashboard_port} only."
        sudo rm -f /etc/nginx/conf.d/networker-dashboard.conf
        return 0
    fi

    # Open firewall for ports 80 and 443
    if command -v ufw &>/dev/null; then
        sudo ufw allow 80/tcp 2>/dev/null || true
        sudo ufw allow 443/tcp 2>/dev/null || true
    elif command -v firewall-cmd &>/dev/null; then
        sudo firewall-cmd --permanent --add-service=http 2>/dev/null || true
        sudo firewall-cmd --permanent --add-service=https 2>/dev/null || true
        sudo firewall-cmd --reload 2>/dev/null || true
    fi
}

# Set up HTTPS for the dashboard: Let's Encrypt if FQDN resolves, self-signed otherwise.
step_setup_letsencrypt() {
    next_step "Set up HTTPS certificate"

    if [[ $DASHBOARD_NGINX_CONFIGURED -ne 1 ]]; then
        print_info "nginx not configured — skipping HTTPS."
        return 0
    fi

    local use_letsencrypt=0
    if [[ -n "$DASHBOARD_FQDN" ]]; then
        # Check if FQDN resolves to this machine
        local resolved_ip server_ip
        resolved_ip="$(dig +short "$DASHBOARD_FQDN" 2>/dev/null | tail -1)"
        server_ip="$(curl -s --max-time 5 ifconfig.me 2>/dev/null)"
        if [[ -n "$resolved_ip" && "$resolved_ip" == "$server_ip" ]]; then
            use_letsencrypt=1
        else
            print_warn "FQDN $DASHBOARD_FQDN does not resolve to this server ($server_ip vs $resolved_ip)"
            print_info "Using self-signed certificate instead."
        fi
    fi

    if [[ $use_letsencrypt -eq 1 ]]; then
        # Install certbot
        case "$PKG_MGR" in
            apt-get) sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq certbot python3-certbot-nginx < /dev/null 2>&1 ;;
            dnf)     sudo dnf install -y certbot python3-certbot-nginx < /dev/null 2>&1 ;;
        esac

        if sudo certbot --nginx -d "$DASHBOARD_FQDN" \
                --non-interactive --agree-tos --register-unsafely-without-email \
                --redirect < /dev/null 2>&1; then
            print_ok "Let's Encrypt certificate installed for $DASHBOARD_FQDN"
            return 0
        else
            print_warn "Let's Encrypt failed — falling back to self-signed certificate."
        fi
    fi

    # Self-signed fallback
    sudo mkdir -p /etc/nginx/ssl
    sudo openssl req -x509 -nodes -days 365 -newkey rsa:2048 \
        -keyout /etc/nginx/ssl/dashboard.key \
        -out /etc/nginx/ssl/dashboard.crt \
        -subj "/CN=${DASHBOARD_FQDN:-networker-dashboard}" 2>/dev/null

    local server_name="${DASHBOARD_FQDN:-_}"
    local dashboard_port
    dashboard_port="$(grep DASHBOARD_PORT /etc/networker-dashboard.env 2>/dev/null | cut -d= -f2)"
    dashboard_port="${dashboard_port:-3000}"

    # Add SSL server block
    sudo tee -a /etc/nginx/conf.d/networker-dashboard.conf > /dev/null <<SSLCONF

server {
    listen 443 ssl;
    server_name ${server_name};

    ssl_certificate /etc/nginx/ssl/dashboard.crt;
    ssl_certificate_key /etc/nginx/ssl/dashboard.key;
    ssl_protocols TLSv1.2 TLSv1.3;

    location / {
        proxy_pass http://127.0.0.1:${dashboard_port};
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }

    location /ws/ {
        proxy_pass http://127.0.0.1:${dashboard_port};
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_read_timeout 86400;
    }
}
SSLCONF

    if sudo nginx -t 2>&1; then
        sudo systemctl reload nginx
        print_ok "Self-signed HTTPS certificate configured"
        print_dim "  Browser will show a security warning — this is expected."
    else
        print_warn "SSL config failed — dashboard available on HTTP only."
    fi
}

# ─── HTTP Stack comparison: nginx setup (Ubuntu) ─────────────────────────────

# Install and configure nginx to serve the same static test page on ports 8081 (HTTP) / 8444 (HTTPS).
# This enables fair HTTP stack comparison: same content, same machine, different server software.
step_setup_nginx() {
    next_step "Set up nginx for HTTP stack comparison"

    if [[ "$SYS_OS" != "Linux" ]]; then
        print_info "nginx setup is Linux-only — skipping."
        return 0
    fi

    local pkg_mgr
    pkg_mgr="$(detect_pkg_manager)"
    if [[ -z "$pkg_mgr" ]]; then
        print_warn "No supported package manager found — cannot install nginx."
        return 1
    fi

    # Check if nginx is already installed and its version
    local nginx_installed=false nginx_ver="" needs_upgrade=false
    if command -v nginx &>/dev/null; then
        nginx_installed=true
        nginx_ver="$(nginx -v 2>&1 | sed 's/.*nginx\///' | cut -d. -f1,2)"
        local maj="${nginx_ver%%.*}" min="${nginx_ver#*.}"
        if [[ "$maj" -lt 1 ]] || { [[ "$maj" -eq 1 ]] && [[ "${min%%.*}" -lt 25 ]]; }; then
            print_info "nginx $nginx_ver lacks HTTP/3 — upgrading to mainline…"
            needs_upgrade=true
        else
            print_ok "nginx $nginx_ver supports HTTP/3 — skipping upgrade"
        fi
    else
        needs_upgrade=true
    fi

    if $nginx_installed && ! $needs_upgrade; then
        print_warn "Adding networker config alongside existing nginx sites."
        print_info "Existing configs are untouched. Networker uses ports 8081/8444 only."
    fi

    # Install or upgrade nginx only if needed
    if $needs_upgrade; then
        print_info "Installing nginx (mainline with HTTP/3)…"
        case "$pkg_mgr" in
            apt-get)
                # Add official nginx.org repo for mainline (1.25+ with HTTP/3)
                if ! apt-cache policy nginx 2>/dev/null | grep -q "nginx.org"; then
                    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y curl gnupg ca-certificates lsb-release ubuntu-keyring < /dev/null
                    curl -fsSL https://nginx.org/keys/nginx_signing.key \
                        | gpg --dearmor | sudo tee /usr/share/keyrings/nginx-archive-keyring.gpg > /dev/null
                    echo "deb [signed-by=/usr/share/keyrings/nginx-archive-keyring.gpg] \
http://nginx.org/packages/mainline/ubuntu $(lsb_release -cs) nginx" \
                        | sudo tee /etc/apt/sources.list.d/nginx.list > /dev/null
                    printf 'Package: *\nPin: origin nginx.org\nPin-Priority: 900\n' \
                        | sudo tee /etc/apt/preferences.d/99nginx > /dev/null
                fi
                sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y nginx < /dev/null
                ;;
            dnf)      sudo dnf install -y nginx ;;
            pacman)   sudo pacman -S --noconfirm nginx ;;
            zypper)   sudo zypper install -y nginx ;;
            apk)      sudo apk add nginx ;;
        esac < /dev/null
    fi

    # Generate static test site
    local site_root="/var/www/networker"
    sudo mkdir -p "$site_root"
    local ep_bin="/usr/local/bin/networker-endpoint"
    if [[ ! -x "$ep_bin" ]]; then
        ep_bin="${INSTALL_DIR}/networker-endpoint"
    fi
    if [[ -x "$ep_bin" ]]; then
        print_info "Generating static test site in $site_root…"
        sudo "$ep_bin" generate-site "$site_root" --preset mixed --stack nginx
    else
        print_warn "networker-endpoint not found — generating minimal test site."
        # Fallback: create a simple index.html with a few assets
        echo '<!DOCTYPE html><html><body><p>Networker static test</p></body></html>' | sudo tee "$site_root/index.html" > /dev/null
        printf '{"status":"ok","stack":"nginx"}\n' | sudo tee "$site_root/health" > /dev/null
    fi
    sudo chown -R www-data:www-data "$site_root" 2>/dev/null || true

    # Generate self-signed certificate
    sudo mkdir -p /etc/nginx/ssl
    local ep_ip="${ENDPOINT_IP:-127.0.0.1}"
    sudo openssl req -x509 -newkey rsa:2048 \
        -keyout /etc/nginx/ssl/networker.key \
        -out /etc/nginx/ssl/networker.crt \
        -days 365 -nodes \
        -subj "/CN=localhost" \
        -addext "subjectAltName=DNS:localhost,IP:127.0.0.1,IP:::1,IP:${ep_ip}" \
        2>/dev/null < /dev/null

    # Write nginx config to conf.d (safe: doesn't touch existing sites)
    sudo mkdir -p /etc/nginx/conf.d
    if [[ -f /etc/nginx/conf.d/networker.conf ]]; then
        sudo cp /etc/nginx/conf.d/networker.conf /etc/nginx/conf.d/networker.conf.bak
    fi
    sudo tee /etc/nginx/conf.d/networker.conf > /dev/null <<'NGINX_CONF'
# Networker HTTP stack comparison — nginx
# Serves static test page for fair comparison with networker-endpoint and IIS.
# Uses non-standard ports (8081/8444) to avoid conflicts with existing sites.

server {
    listen 8081;
    server_name _;
    root /var/www/networker;
    index index.html;

    location / {
        try_files $uri $uri/ =404;
    }

    # Proxy dynamic paths to the endpoint (pageload probes use /page and /asset)
    location /page {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
    }
    location /asset {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
    }

    # MIME types for test assets
    location ~* \.bin$ {
        default_type application/octet-stream;
    }
}

server {
    listen 8444 ssl;
    listen 8444 quic reuseport;
    http2 on;
    http3 on;
    server_name _;
    root /var/www/networker;
    index index.html;

    ssl_certificate /etc/nginx/ssl/networker.crt;
    ssl_certificate_key /etc/nginx/ssl/networker.key;

    # Advertise HTTP/3 support
    add_header Alt-Svc 'h3=":8444"; ma=86400' always;

    location / {
        try_files $uri $uri/ =404;
    }

    # Proxy dynamic paths to the endpoint (pageload probes use /page and /asset)
    location /page {
        proxy_pass https://127.0.0.1:8443;
        proxy_ssl_verify off;
        proxy_set_header Host $host;
    }
    location /asset {
        proxy_pass https://127.0.0.1:8443;
        proxy_ssl_verify off;
        proxy_set_header Host $host;
    }

    location ~* \.bin$ {
        default_type application/octet-stream;
    }
}
NGINX_CONF

    # Test config, rollback on failure, then restart
    if sudo nginx -t 2>&1; then
        sudo systemctl enable nginx 2>/dev/null || true
        sudo systemctl restart nginx
        print_ok "nginx serving test page on ports 8081 (HTTP) / 8444 (HTTPS)"
    else
        print_warn "nginx config test failed — rolling back"
        if [[ -f /etc/nginx/conf.d/networker.conf.bak ]]; then
            sudo mv /etc/nginx/conf.d/networker.conf.bak /etc/nginx/conf.d/networker.conf
            sudo nginx -t 2>/dev/null && sudo systemctl reload nginx
            print_info "Rolled back to previous networker.conf"
        else
            sudo rm -f /etc/nginx/conf.d/networker.conf
            sudo nginx -t 2>/dev/null && sudo systemctl reload nginx
            print_info "Removed networker.conf (no previous version)"
        fi
        return 1
    fi
}

# ─── HTTP Stack comparison: IIS setup (Windows via SSH/run-command) ───────────

# Generate a PowerShell script to install and configure IIS on a Windows VM.
# The output can be piped to az vm run-command / AWS SSM / gcloud SSH.
_iis_setup_powershell() {
    local fqdn="${1:-}"  # FQDN for hostname-based binding (enables HTTP/3 via SNI)
    local site_root="C:\\networker-static"
    cat <<IIS_PS1_HEADER
# ── IIS setup for HTTP stack comparison ──────────────────────────────────────
\$ErrorActionPreference = 'Stop'
\$fqdn = "${fqdn}"

# 1. Install IIS + URL Rewrite + ARR (for reverse-proxy of /page, /asset)
Write-Host "Installing IIS…"
Install-WindowsFeature -Name Web-Server -IncludeManagementTools | Out-Null
IIS_PS1_HEADER
    cat <<'IIS_PS1'

Write-Host "Installing URL Rewrite Module…"
$urlRewriteUrl = "https://download.microsoft.com/download/1/2/8/128E2E22-C1B9-44A4-BE2A-5859ED1D4592/rewrite_amd64_en-US.msi"
$urlRewriteMsi = "$env:TEMP\urlrewrite.msi"
Invoke-WebRequest -Uri $urlRewriteUrl -OutFile $urlRewriteMsi -UseBasicParsing
Start-Process msiexec.exe -ArgumentList "/i $urlRewriteMsi /quiet /norestart" -Wait -NoNewWindow

Write-Host "Installing ARR…"
$arrUrl = "https://download.microsoft.com/download/E/9/8/E9849D6A-020E-47E4-9FD0-A023E99B54EB/requestRouter_amd64.msi"
$arrMsi = "$env:TEMP\arr.msi"
Invoke-WebRequest -Uri $arrUrl -OutFile $arrMsi -UseBasicParsing
Start-Process msiexec.exe -ArgumentList "/i $arrMsi /quiet /norestart" -Wait -NoNewWindow

# Enable ARR proxy at server level
Import-Module WebAdministration
Set-WebConfigurationProperty -pspath "MACHINE/WEBROOT/APPHOST" `
    -filter "system.webServer/proxy" -name "enabled" -value "True"

# 2. Enable HTTP/3 (Windows Server 2022+) — requires reboot to take effect
Write-Host "Enabling HTTP/3 via registry…"
$httpParams = "HKLM:\SYSTEM\CurrentControlSet\Services\HTTP\Parameters"
if (-not (Test-Path $httpParams)) { New-Item -Path $httpParams -Force | Out-Null }
$needsReboot = $false
foreach ($prop in @("EnableHttp3","EnableHttp2Tls","EnableHttp2Cleartext")) {
    $cur = (Get-ItemProperty -Path $httpParams -Name $prop -ErrorAction SilentlyContinue).$prop
    if ($cur -ne 1) {
        Set-ItemProperty -Path $httpParams -Name $prop -Value 1 -Type DWord
        $needsReboot = $true
    }
}

# 3. Generate static test site
$siteRoot = "C:\networker-static"
New-Item -ItemType Directory -Path $siteRoot -Force | Out-Null
IIS_PS1

    cat <<'IIS_PS1_GENSITE'
$epExe = "C:\networker\networker-endpoint.exe"
if (Test-Path $epExe) {
    $genSiteHelp = & $epExe --help 2>&1
    if ($genSiteHelp -match 'generate-site') {
        Write-Host "Generating static test site via endpoint…"
        & $epExe generate-site $siteRoot --preset mixed --stack iis
    } else {
        Write-Host "Endpoint does not support generate-site — creating static test page…"
        $genSite = $false
    }
} else {
    Write-Host "Endpoint binary not found at $epExe — creating static test page…"
    $genSite = $false
}
if ($genSite -eq $false -or -not (Test-Path "$siteRoot\index.html")) {
    # Fallback: create test page with 50 assets of mixed sizes
    $html = "<!DOCTYPE html>`n<html><head><title>Networker Page Load Test</title>`n"
    $html += "<link rel=`"stylesheet`" href=`"style.css`">`n<link rel=`"icon`" href=`"data:,`">`n</head><body>`n"
    for ($i = 0; $i -lt 50; $i++) { $html += "<img src=`"asset-$i.bin`" width=`"1`" height=`"1`" alt=`"`">`n" }
    $html += "</body></html>"
    [IO.File]::WriteAllText("$siteRoot\index.html", $html, [Text.Encoding]::UTF8)
    [IO.File]::WriteAllText("$siteRoot\style.css", "body{margin:0}", [Text.Encoding]::UTF8)
    [IO.File]::WriteAllText("$siteRoot\health", '{"status":"ok","stack":"iis"}', [Text.Encoding]::UTF8)
    $sizes = @(512,512,512,512,512,2048,2048,2048,2048,2048,
               4096,4096,4096,4096,4096,8192,8192,8192,8192,8192,
               16384,16384,16384,16384,16384,32768,32768,32768,32768,32768,
               65536,65536,65536,65536,65536,102400,102400,102400,102400,102400,
               204800,204800,204800,204800,204800,409600,409600,614400,614400,1048576)
    $rng = New-Object Random
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $bytes = New-Object byte[] $sizes[$i]
        $rng.NextBytes($bytes)
        [IO.File]::WriteAllBytes("$siteRoot\asset-$i.bin", $bytes)
    }
    Write-Host "Created test page with 50 assets"
}
IIS_PS1_GENSITE

    cat <<'IIS_PS1_REST'

# 3b. Create web.config — default doc, MIME types, and reverse-proxy rules
# for /page and /asset (pageload probes use dynamic endpoints on the backing server)
$webConfig = @"
<?xml version="1.0" encoding="UTF-8"?>
<configuration>
  <system.webServer>
    <defaultDocument>
      <files>
        <clear />
        <add value="index.html" />
      </files>
    </defaultDocument>
    <staticContent>
      <remove fileExtension="." />
      <mimeMap fileExtension="." mimeType="application/json" />
      <remove fileExtension=".bin" />
      <mimeMap fileExtension=".bin" mimeType="application/octet-stream" />
    </staticContent>
    <rewrite>
      <rules>
        <rule name="Proxy /page to endpoint" stopProcessing="true">
          <match url="^page(.*)" />
          <action type="Rewrite" url="http://127.0.0.1:8080/page{R:1}" />
        </rule>
        <rule name="Proxy /asset to endpoint" stopProcessing="true">
          <match url="^asset$" />
          <conditions>
            <add input="{QUERY_STRING}" pattern=".+" />
          </conditions>
          <action type="Rewrite" url="http://127.0.0.1:8080/asset?{C:0}" appendQueryString="false" />
        </rule>
      </rules>
    </rewrite>
  </system.webServer>
</configuration>
"@
$webConfig | Out-File "$siteRoot\web.config" -Encoding UTF8
Write-Host "web.config created (with reverse-proxy rules for /page, /asset)"

# 4. Generate self-signed certificate (include FQDN in SAN for SNI/H3)
Write-Host "Creating self-signed certificate…"
$dnsNames = @("localhost", $env:COMPUTERNAME)
if ($fqdn -and $fqdn -ne "") { $dnsNames += $fqdn }
$cert = New-SelfSignedCertificate `
    -DnsName $dnsNames `
    -CertStoreLocation "Cert:\LocalMachine\My" `
    -NotAfter (Get-Date).AddYears(1) `
    -FriendlyName "Networker IIS Test"
$thumbprint = $cert.Thumbprint
Write-Host "Certificate thumbprint: $thumbprint (SAN: $($dnsNames -join ', '))"

# 5. Remove default IIS site, create networker site
Import-Module WebAdministration
if (Get-Website -Name "Default Web Site" -ErrorAction SilentlyContinue) {
    Remove-Website -Name "Default Web Site"
}
if (Get-Website -Name "networker-iis" -ErrorAction SilentlyContinue) {
    Remove-Website -Name "networker-iis"
}

# Create site with HTTP binding
New-Website -Name "networker-iis" `
    -PhysicalPath $siteRoot `
    -Port 8082 `
    -Force | Out-Null

# Add HTTPS bindings — hostname-based with SNI (required for HTTP/3) + IP fallback
if ($fqdn -and $fqdn -ne "") {
    New-WebBinding -Name "networker-iis" -Protocol "https" -Port 8445 -HostHeader $fqdn -SslFlags 1
    $sniBinding = Get-WebBinding -Name "networker-iis" -Protocol "https" -Port 8445 | Where-Object { $_.sslFlags -eq 1 }
    $sniBinding.AddSslCertificate($thumbprint, "My")
    Write-Host "HTTPS binding: hostname=$fqdn, SNI=required (HTTP/3 enabled)"
}
# Also add IP-based binding for direct-IP connections (no HTTP/3, but H1.1/H2 work)
New-WebBinding -Name "networker-iis" -Protocol "https" -Port 8445 -SslFlags 0
$ipBinding = Get-WebBinding -Name "networker-iis" -Protocol "https" -Port 8445 | Where-Object { $_.sslFlags -eq 0 }
$ipBinding.AddSslCertificate($thumbprint, "My")

# Add alt-svc header to advertise HTTP/3
Set-WebConfigurationProperty -pspath "IIS:\Sites\networker-iis" `
    -filter "system.webServer/httpProtocol/customHeaders" `
    -name "." `
    -value @{name="alt-svc"; value="h3="":8445""; ma=86400"}

# Start the site
Start-Website -Name "networker-iis"

# 6. Firewall rules
New-NetFirewallRule -DisplayName "Networker-IIS-HTTP" -Direction Inbound -Protocol TCP -LocalPort 8082 -Action Allow -ErrorAction SilentlyContinue | Out-Null
New-NetFirewallRule -DisplayName "Networker-IIS-HTTPS" -Direction Inbound -Protocol TCP -LocalPort 8445 -Action Allow -ErrorAction SilentlyContinue | Out-Null
# HTTP/3 uses UDP
New-NetFirewallRule -DisplayName "Networker-IIS-QUIC" -Direction Inbound -Protocol UDP -LocalPort 8445 -Action Allow -ErrorAction SilentlyContinue | Out-Null

Write-Host "IIS configured: HTTP=8082, HTTPS=8445"
if ($needsReboot) {
    Write-Host "REBOOT_NEEDED"
} else {
    Write-Host "HTTP/3 registry already set — no reboot needed"
}
IIS_PS1_REST
}

# Set up nginx on a remote Linux VM via SSH.
# $1 = public IP, $2 = SSH user
_remote_setup_nginx() {
    local ip="$1" ssh_user="${2:-azureuser}"

    next_step "Set up nginx on remote VM (${ip})"

    # Check if nginx is already serving with HTTP/3 support
    if curl -sf --max-time 5 "http://${ip}:8081/health" &>/dev/null; then
        # Check if QUIC/H3 is available (Alt-Svc header present)
        if curl -sfk --max-time 5 -I "https://${ip}:8444/health" 2>/dev/null | grep -qi 'alt-svc.*h3'; then
            print_ok "nginx already responding with HTTP/3 on port 8444 — skipping"
            return 0
        fi
        print_info "nginx responding but without HTTP/3 — upgrading to mainline…"
    else
        print_info "Installing and configuring nginx via SSH…"
    fi
    # We pipe the step_setup_nginx function body over SSH
    # But it's cleaner to run the main commands inline
    ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "${ssh_user}@${ip}" bash -s <<'NGINX_SSH'
set -e
export DEBIAN_FRONTEND=noninteractive

# Check existing nginx version — only upgrade if < 1.25 (no HTTP/3)
needs_upgrade=false
if command -v nginx >/dev/null 2>&1; then
    nginx_ver="$(nginx -v 2>&1 | sed 's/.*nginx\///' | cut -d. -f1,2)"
    maj="${nginx_ver%%.*}"; min="${nginx_ver#*.}"; min="${min%%.*}"
    if [ "$maj" -lt 1 ] || { [ "$maj" -eq 1 ] && [ "$min" -lt 25 ]; }; then
        echo ">> nginx $nginx_ver lacks HTTP/3 — upgrading to mainline"
        needs_upgrade=true
    else
        echo ">> nginx $nginx_ver supports HTTP/3 — adding config alongside existing sites"
    fi
else
    needs_upgrade=true
fi

if $needs_upgrade; then
    if ! apt-cache policy nginx 2>/dev/null | grep -q "nginx.org"; then
        sudo apt-get install -y curl gnupg ca-certificates lsb-release ubuntu-keyring < /dev/null
        curl -fsSL https://nginx.org/keys/nginx_signing.key \
            | gpg --dearmor | sudo tee /usr/share/keyrings/nginx-archive-keyring.gpg > /dev/null
        echo "deb [signed-by=/usr/share/keyrings/nginx-archive-keyring.gpg] \
http://nginx.org/packages/mainline/ubuntu $(lsb_release -cs) nginx" \
            | sudo tee /etc/apt/sources.list.d/nginx.list > /dev/null
        printf 'Package: *\nPin: origin nginx.org\nPin-Priority: 900\n' \
            | sudo tee /etc/apt/preferences.d/99nginx > /dev/null
    fi
    sudo apt-get update -qq && sudo apt-get install -y nginx < /dev/null
fi

# Generate static site
site_root="/var/www/networker"
sudo mkdir -p "$site_root"
ep_bin="/usr/local/bin/networker-endpoint"
if [ -x "$ep_bin" ]; then
    sudo "$ep_bin" generate-site "$site_root" --preset mixed --stack nginx
else
    echo '<!DOCTYPE html><html><body>Networker static test</body></html>' | sudo tee "$site_root/index.html" > /dev/null
    printf '{"status":"ok","stack":"nginx"}\n' | sudo tee "$site_root/health" > /dev/null
fi
sudo chown -R www-data:www-data "$site_root" 2>/dev/null || true

# Self-signed cert
sudo mkdir -p /etc/nginx/ssl
sudo openssl req -x509 -newkey rsa:2048 \
    -keyout /etc/nginx/ssl/networker.key \
    -out /etc/nginx/ssl/networker.crt \
    -days 365 -nodes \
    -subj "/CN=localhost" 2>/dev/null < /dev/null

# Write config to conf.d (safe: doesn't touch existing sites)
sudo mkdir -p /etc/nginx/conf.d
if [ -f /etc/nginx/conf.d/networker.conf ]; then
    sudo cp /etc/nginx/conf.d/networker.conf /etc/nginx/conf.d/networker.conf.bak
fi
sudo tee /etc/nginx/conf.d/networker.conf > /dev/null <<'EOF'
server {
    listen 8081;
    server_name _;
    root /var/www/networker;
    index index.html;
    location / { try_files $uri $uri/ =404; }
    location /page { proxy_pass http://127.0.0.1:8080; proxy_set_header Host $host; }
    location /asset { proxy_pass http://127.0.0.1:8080; proxy_set_header Host $host; }
    location ~* \.bin$ { default_type application/octet-stream; }
}
server {
    listen 8444 ssl;
    listen 8444 quic reuseport;
    http2 on;
    http3 on;
    server_name _;
    root /var/www/networker;
    index index.html;
    ssl_certificate /etc/nginx/ssl/networker.crt;
    ssl_certificate_key /etc/nginx/ssl/networker.key;
    add_header Alt-Svc 'h3=":8444"; ma=86400' always;
    location / { try_files $uri $uri/ =404; }
    location /page { proxy_pass https://127.0.0.1:8443; proxy_ssl_verify off; proxy_set_header Host $host; }
    location /asset { proxy_pass https://127.0.0.1:8443; proxy_ssl_verify off; proxy_set_header Host $host; }
    location ~* \.bin$ { default_type application/octet-stream; }
}
EOF

# Test config — rollback on failure
if sudo nginx -t 2>&1; then
    sudo systemctl enable nginx 2>/dev/null || true
    sudo systemctl restart nginx
    echo "nginx configured on ports 8081/8444 (HTTP/3 enabled)"
else
    echo ">> ERROR: nginx -t failed — rolling back"
    if [ -f /etc/nginx/conf.d/networker.conf.bak ]; then
        sudo mv /etc/nginx/conf.d/networker.conf.bak /etc/nginx/conf.d/networker.conf
        sudo nginx -t 2>/dev/null && sudo systemctl reload nginx
        echo ">> Rolled back to previous networker.conf"
    else
        sudo rm -f /etc/nginx/conf.d/networker.conf
        sudo nginx -t 2>/dev/null && sudo systemctl reload nginx
        echo ">> Removed networker.conf (no previous version)"
    fi
    exit 1
fi
NGINX_SSH

    # Verify
    sleep 2
    if curl -sf --max-time 5 "http://${ip}:8081/health" &>/dev/null; then
        print_ok "nginx serving on port 8081 (HTTP) / 8444 (HTTPS)"
    else
        print_warn "nginx may not be responding yet on port 8081 — check manually"
    fi
}

# Set up IIS on an Azure Windows VM via az vm run-command.
# Installs IIS + URL Rewrite + ARR, enables HTTP/3, reboots if needed,
# then waits for the VM to come back and verifies IIS is serving.
# $1 = resource group, $2 = VM name, $3 = public IP, $4 = FQDN (optional)
_azure_win_setup_iis() {
    local rg="$1" vm="$2" ip="${3:-}" fqdn="${4:-}"
    local reboot_timeout=300  # 5 minutes max wait for reboot

    # Skip if IIS is already responding on both HTTP and HTTPS
    if [[ -n "$ip" ]]; then
        if curl -sf --max-time 5 "http://${ip}:8082/health" &>/dev/null \
           && curl -sfk --max-time 5 "https://${ip}:8445/" &>/dev/null; then
            print_ok "IIS already responding on 8082/8445 — skipping"
            return 0
        fi
    fi

    print_info "Setting up IIS on Windows VM ($vm)…"
    [[ -n "$fqdn" ]] && print_info "Using FQDN for HTTP/3 SNI binding: $fqdn"
    local ps_script output
    ps_script="$(_iis_setup_powershell "$fqdn")"

    output="$(az vm run-command invoke \
        --resource-group "$rg" --name "$vm" \
        --command-id RunPowerShellScript \
        --scripts "$ps_script" 2>&1)" || true

    if echo "$output" | grep -q "REBOOT_NEEDED"; then
        print_info "HTTP/3 registry changed — rebooting VM…"
        az vm restart --resource-group "$rg" --name "$vm" --no-wait 2>/dev/null || true

        # Wait for VM to come back (poll health endpoint)
        print_info "Waiting for VM to reboot (timeout ${reboot_timeout}s)…"
        local elapsed=0
        while (( elapsed < reboot_timeout )); do
            sleep 10
            elapsed=$(( elapsed + 10 ))
            if [[ -n "$ip" ]] && curl -sf --max-time 5 "http://${ip}:8082/health" &>/dev/null; then
                print_ok "VM rebooted — IIS responding after ${elapsed}s"
                break
            fi
            printf "."
        done
        echo

        if (( elapsed >= reboot_timeout )); then
            print_warn "Timed out waiting for IIS after reboot — check VM manually"
            return 1
        fi
    else
        print_ok "IIS configured: HTTP=8082, HTTPS=8445"
    fi

    # Verify all endpoints
    if [[ -n "$ip" ]]; then
        local ok=true
        for url in "http://${ip}:8082/" "http://${ip}:8082/health" "https://${ip}:8445/"; do
            local scheme="${url%%://*}"
            local curl_flags="--max-time 5 -sf"
            [[ "$scheme" == "https" ]] && curl_flags="$curl_flags -k"
            if curl $curl_flags "$url" -o /dev/null; then
                print_ok "  $url → OK"
            else
                print_warn "  $url → FAILED"
                ok=false
            fi
        done
        $ok || print_warn "Some IIS endpoints failed — HTTP/3 may need a reboot to activate"
    fi
}

# Set up nginx on a GCE Linux instance via gcloud ssh.
# $1 = instance name, $2 = public IP (for verification)
_gcp_setup_nginx() {
    local name="$1" ip="${2:-}"

    next_step "Set up nginx on GCE instance ($name)"

    # Check if nginx is already serving with HTTP/3 support
    if [[ -n "$ip" ]] && curl -sf --max-time 5 "http://${ip}:8081/health" &>/dev/null; then
        if curl -sfk --max-time 5 -I "https://${ip}:8444/health" 2>/dev/null | grep -qi 'alt-svc.*h3'; then
            print_ok "nginx already responding with HTTP/3 on port 8444 — skipping"
            return 0
        fi
        print_info "nginx responding but without HTTP/3 — upgrading to mainline…"
    else
        print_info "Installing and configuring nginx via gcloud SSH…"
    fi
    _gcp_ssh_run "$name" "bash -s" <<'NGINX_GCP'
set -e
export DEBIAN_FRONTEND=noninteractive

# Check existing nginx version — only upgrade if < 1.25 (no HTTP/3)
needs_upgrade=false
if command -v nginx >/dev/null 2>&1; then
    nginx_ver="$(nginx -v 2>&1 | sed 's/.*nginx\///' | cut -d. -f1,2)"
    maj="${nginx_ver%%.*}"; min="${nginx_ver#*.}"; min="${min%%.*}"
    if [ "$maj" -lt 1 ] || { [ "$maj" -eq 1 ] && [ "$min" -lt 25 ]; }; then
        echo ">> nginx $nginx_ver lacks HTTP/3 — upgrading to mainline"
        needs_upgrade=true
    else
        echo ">> nginx $nginx_ver supports HTTP/3 — adding config alongside existing sites"
    fi
else
    needs_upgrade=true
fi

if $needs_upgrade; then
    if ! apt-cache policy nginx 2>/dev/null | grep -q "nginx.org"; then
        sudo apt-get install -y curl gnupg ca-certificates lsb-release ubuntu-keyring < /dev/null
        curl -fsSL https://nginx.org/keys/nginx_signing.key \
            | gpg --dearmor | sudo tee /usr/share/keyrings/nginx-archive-keyring.gpg > /dev/null
        echo "deb [signed-by=/usr/share/keyrings/nginx-archive-keyring.gpg] \
http://nginx.org/packages/mainline/ubuntu $(lsb_release -cs) nginx" \
            | sudo tee /etc/apt/sources.list.d/nginx.list > /dev/null
        printf 'Package: *\nPin: origin nginx.org\nPin-Priority: 900\n' \
            | sudo tee /etc/apt/preferences.d/99nginx > /dev/null
    fi
    sudo apt-get update -qq && sudo apt-get install -y nginx < /dev/null
fi

site_root="/var/www/networker"
sudo mkdir -p "$site_root"
ep_bin="/usr/local/bin/networker-endpoint"
if [ -x "$ep_bin" ]; then
    sudo "$ep_bin" generate-site "$site_root" --preset mixed --stack nginx
else
    echo '<!DOCTYPE html><html><body>Networker static test</body></html>' | sudo tee "$site_root/index.html" > /dev/null
    printf '{"status":"ok","stack":"nginx"}\n' | sudo tee "$site_root/health" > /dev/null
fi
sudo chown -R www-data:www-data "$site_root" 2>/dev/null || true
sudo mkdir -p /etc/nginx/ssl
sudo openssl req -x509 -newkey rsa:2048 \
    -keyout /etc/nginx/ssl/networker.key \
    -out /etc/nginx/ssl/networker.crt \
    -days 365 -nodes -subj "/CN=localhost" 2>/dev/null < /dev/null

# Write config to conf.d (safe: doesn't touch existing sites)
sudo mkdir -p /etc/nginx/conf.d
if [ -f /etc/nginx/conf.d/networker.conf ]; then
    sudo cp /etc/nginx/conf.d/networker.conf /etc/nginx/conf.d/networker.conf.bak
fi
sudo tee /etc/nginx/conf.d/networker.conf > /dev/null <<'EOF'
server {
    listen 8081; server_name _;
    root /var/www/networker; index index.html;
    location / { try_files $uri $uri/ =404; }
    location /page { proxy_pass http://127.0.0.1:8080; proxy_set_header Host $host; }
    location /asset { proxy_pass http://127.0.0.1:8080; proxy_set_header Host $host; }
    location ~* \.bin$ { default_type application/octet-stream; }
}
server {
    listen 8444 ssl;
    listen 8444 quic reuseport;
    http2 on;
    http3 on;
    server_name _;
    root /var/www/networker; index index.html;
    ssl_certificate /etc/nginx/ssl/networker.crt;
    ssl_certificate_key /etc/nginx/ssl/networker.key;
    add_header Alt-Svc 'h3=":8444"; ma=86400' always;
    location / { try_files $uri $uri/ =404; }
    location /page { proxy_pass https://127.0.0.1:8443; proxy_ssl_verify off; proxy_set_header Host $host; }
    location /asset { proxy_pass https://127.0.0.1:8443; proxy_ssl_verify off; proxy_set_header Host $host; }
    location ~* \.bin$ { default_type application/octet-stream; }
}
EOF

if sudo nginx -t 2>&1; then
    sudo systemctl enable nginx 2>/dev/null || true
    sudo systemctl restart nginx
    echo "nginx configured on ports 8081/8444 (HTTP/3 enabled)"
else
    echo ">> ERROR: nginx -t failed — rolling back"
    if [ -f /etc/nginx/conf.d/networker.conf.bak ]; then
        sudo mv /etc/nginx/conf.d/networker.conf.bak /etc/nginx/conf.d/networker.conf
        sudo nginx -t 2>/dev/null && sudo systemctl reload nginx
        echo ">> Rolled back to previous networker.conf"
    else
        sudo rm -f /etc/nginx/conf.d/networker.conf
        sudo nginx -t 2>/dev/null && sudo systemctl reload nginx
        echo ">> Removed networker.conf (no previous version)"
    fi
    exit 1
fi
NGINX_GCP

    sleep 2
    if [[ -n "$ip" ]] && curl -sf --max-time 5 "http://${ip}:8081/health" &>/dev/null; then
        print_ok "nginx serving on port 8081 (HTTP) / 8444 (HTTPS)"
    else
        print_warn "nginx may not be responding yet on port 8081 — check manually"
    fi
}

# Poll /health until the endpoint responds.
# $1 = public IP
_remote_verify_health() {
    local ip="$1" ssh_user="${2:-azureuser}"

    print_info "Checking http://${ip}:8080/health …"
    local attempts=0
    while true; do
        local resp
        resp="$(curl -sf --max-time 5 "http://${ip}:8080/health" 2>/dev/null || echo "")"
        if [[ -n "$resp" ]]; then
            print_ok "Health check passed: $resp"
            return 0
        fi
        attempts=$((attempts + 1))
        if [[ $attempts -gt 12 ]]; then
            echo ""
            print_warn "Endpoint did not respond within 60 seconds."
            echo ""
            print_info "Fetching service status from the VM…"
            ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "${ssh_user}@${ip}" \
                "sudo systemctl status networker-endpoint --no-pager -l 2>&1 | head -30" 2>/dev/null || true
            echo ""
            print_info "Last 30 log lines:"
            ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "${ssh_user}@${ip}" \
                "sudo journalctl -u networker-endpoint -n 30 --no-pager 2>&1" 2>/dev/null || true
            echo ""
            print_warn "Manual check:  ssh ${ssh_user}@${ip} 'sudo journalctl -u networker-endpoint -f'"
            return 0
        fi
        printf "."
        sleep 5
    done
}

# Write networker-cloud.json pointing at the remote endpoint.
# $1 = endpoint public IP
# ── Windows VM helpers (Azure run-command / AWS SSM) ─────────────────────────

# Wait for a Windows VM Azure Agent to be responsive (polling via run-command).
# $1 = resource group, $2 = VM name, $3 = friendly label
_azure_wait_for_windows_vm() {
    local rg="$1" vm="$2" label="${3:-VM}"
    print_info "Waiting for $label Windows VM to be ready (Azure Agent)…"
    local attempts=0
    while true; do
        if az vm run-command invoke \
                --resource-group "$rg" \
                --name "$vm" \
                --command-id RunPowerShellScript \
                --scripts 'Write-Host ready' \
                --output none 2>/dev/null; then
            echo ""
            print_ok "Windows VM ready"
            return 0
        fi
        attempts=$((attempts + 1))
        if [[ $attempts -gt 30 ]]; then
            echo ""
            print_err "Windows VM not ready after 5 minutes. Check the Azure portal."
            exit 1
        fi
        printf "."
        sleep 10
    done
}

# Install a binary on a Windows Azure VM via run-command (PowerShell).
# $1 = binary name (no .exe), $2 = resource group, $3 = VM name, $4 = version tag
_azure_win_install_binary() {
    local binary="$1" rg="$2" vm="$3" ver="$4"
    local archive="${binary}-x86_64-pc-windows-msvc.zip"
    local url="https://github.com/${REPO_GH}/releases/download/${ver}/${archive}"
    local dest='C:\networker'

    # Pre-check release assets — fall back to source build if no pre-built binary
    local has_assets
    has_assets="$(gh release view --repo "$REPO_GH" "$ver" --json assets \
                  -q '[.assets[].name] | join(" ")' 2>/dev/null || echo "")"
    if [[ -z "$has_assets" ]]; then
        has_assets="$(curl -fsSL "https://api.github.com/repos/${REPO_GH}/releases/tags/${ver}" 2>/dev/null \
                      | grep '"name"' | sed 's/.*"name":[[:space:]]*"\([^"]*\)".*/\1/' | tr '\n' ' ')"
    fi
    if ! printf '%s' "$has_assets" | grep -q "${binary}-.*windows"; then
        print_warn "Release ${ver} has no Windows binary for ${binary}."
        _azure_win_source_build "$binary" "$rg" "$vm"
        return $?
    fi

    print_info "Installing ${binary}.exe on Windows VM via PowerShell…"
    local ps_tmp
    ps_tmp="$(mktemp /tmp/networker-ps-XXXXX.ps1)"
    cat > "$ps_tmp" <<PSEOF
\$ErrorActionPreference = 'Stop'
\$url  = '${url}'
\$zip  = 'C:\\networker-tmp\\${archive}'
\$dest = '${dest}'
New-Item -ItemType Directory -Force -Path C:\\networker-tmp | Out-Null
New-Item -ItemType Directory -Force -Path \$dest | Out-Null
# Install VC++ Redistributable if missing (MSVC-built binaries need vcruntime140.dll)
if (-not (Test-Path 'C:\\Windows\\System32\\vcruntime140.dll')) {
    Write-Host 'Installing VC++ Redistributable...'
    Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vc_redist.x64.exe' -OutFile C:\\vc_redist.x64.exe -UseBasicParsing
    Start-Process -FilePath 'C:\\vc_redist.x64.exe' -ArgumentList '/install','/quiet','/norestart' -Wait
    Remove-Item 'C:\\vc_redist.x64.exe' -Force -ErrorAction SilentlyContinue
    Write-Host 'VC++ Redistributable installed'
}
Write-Host "Downloading \$url ..."
Invoke-WebRequest -Uri \$url -OutFile \$zip -UseBasicParsing
Expand-Archive -Path \$zip -DestinationPath \$dest -Force
Remove-Item -Recurse -Force C:\\networker-tmp
\$machinePath = [System.Environment]::GetEnvironmentVariable('Path','Machine')
if (\$machinePath -notlike "*\$dest*") {
    [System.Environment]::SetEnvironmentVariable('Path',"\$machinePath;\$dest",'Machine')
    Write-Host "Added \$dest to system PATH"
}
\$ver = & "\$dest\\${binary}.exe" --version 2>&1
Write-Host "Installed: \$ver"
PSEOF

    az vm run-command invoke \
        --resource-group "$rg" --name "$vm" \
        --command-id RunPowerShellScript \
        --scripts "@${ps_tmp}" \
        --output table 2>/dev/null || print_warn "Install command returned non-zero; check VM logs"
    rm -f "$ps_tmp"
    print_ok "${binary}.exe installed on Windows VM"
}

# Build a binary from source on an Azure Windows VM via run-command (PowerShell).
# Installs Rust if needed, then cargo install from the repo.
# $1 = binary name, $2 = resource group, $3 = VM name
_azure_win_source_build() {
    local binary="$1" rg="$2" vm="$3"
    print_info "Building ${binary} from source on Windows VM…"
    print_dim "This may take 25–40 min (VS Build Tools + Rust + compile)."

    local ps_tmp
    ps_tmp="$(mktemp /tmp/networker-ps-XXXXX.ps1)"
    cat > "$ps_tmp" <<PSEOF
\$ErrorActionPreference = 'Continue'
\$dest = 'C:\\networker'

# Set known paths (runs as SYSTEM via run-command)
\$env:CARGO_HOME = 'C:\\cargo'
\$env:RUSTUP_HOME = 'C:\\rustup'
New-Item -ItemType Directory -Force -Path \$env:CARGO_HOME | Out-Null
New-Item -ItemType Directory -Force -Path \$env:RUSTUP_HOME | Out-Null

# Install Visual C++ Build Tools if not present
if (-not (Test-Path 'C:\\BuildTools\\VC\\Tools\\MSVC')) {
    Write-Host 'Installing Visual C++ Build Tools...'
    Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vs_buildtools.exe' -OutFile C:\\vs_buildtools.exe -UseBasicParsing
    & C:\\vs_buildtools.exe --quiet --wait --norestart --nocache --installPath C:\\BuildTools --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended 2>&1 | Out-Null
    Write-Host 'Build Tools installed'
}

# Install Rust if not present
if (-not (Test-Path 'C:\\cargo\\bin\\cargo.exe')) {
    Write-Host 'Installing Rust...'
    Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile C:\\rustup-init.exe -UseBasicParsing
    & C:\\rustup-init.exe -y --default-toolchain stable 2>&1 | Select-Object -Last 3
}
\$env:Path = 'C:\\cargo\\bin;' + \$env:Path

Write-Host "Building ${binary} from source..."
cargo install --git ${REPO_HTTPS} ${binary} 2>&1 | Select-Object -Last 5

New-Item -ItemType Directory -Force -Path \$dest | Out-Null
\$srcExe = "C:\\cargo\\bin\\${binary}.exe"
\$dstExe = Join-Path \$dest "${binary}.exe"
if (Test-Path \$srcExe) {
    Copy-Item \$srcExe \$dstExe -Force
    Write-Host ("Installed: " + (& \$dstExe --version 2>&1))
} else {
    Write-Host "ERROR: Binary not found at \$srcExe"
    exit 1
}

\$mp = [System.Environment]::GetEnvironmentVariable('Path','Machine')
if (\$mp -notlike ('*' + \$dest + '*')) {
    \$newPath = \$mp + ';' + \$dest
    [System.Environment]::SetEnvironmentVariable('Path', \$newPath, 'Machine')
}
PSEOF

    az vm run-command invoke \
        --resource-group "$rg" --name "$vm" \
        --command-id RunPowerShellScript \
        --scripts "@${ps_tmp}" \
        --timeout 2400 \
        --output table 2>/dev/null || {
        print_warn "Source build may have failed — check VM logs"
        rm -f "$ps_tmp"
        return 1
    }
    rm -f "$ps_tmp"
    print_ok "${binary}.exe built and installed on Azure Windows VM"
}

# Create a Windows Service for networker-endpoint on an Azure Windows VM.
# $1 = resource group, $2 = VM name
_azure_win_create_endpoint_service() {
    local rg="$1" vm="$2"
    print_info "Creating Windows Service and opening firewall ports…"
    local ps_tmp
    ps_tmp="$(mktemp /tmp/networker-ps-XXXXX.ps1)"
    cat > "$ps_tmp" <<'PSEOF'
$ErrorActionPreference = 'Continue'
$exe = 'C:\networker\networker-endpoint.exe'
# Stop any existing instance
Stop-Process -Name 'networker-endpoint' -Force -ErrorAction SilentlyContinue
# Windows Firewall rules (before starting the binary)
netsh advfirewall firewall add rule name='Networker-HTTP'  protocol=TCP dir=in action=allow localport='8080,8081,8082'  | Out-Null
netsh advfirewall firewall add rule name='Networker-HTTPS' protocol=TCP dir=in action=allow localport='8443,8444,8445'  | Out-Null
netsh advfirewall firewall add rule name='Networker-UDP'   protocol=UDP dir=in action=allow localport='8443,9998,9999' | Out-Null
Write-Host 'Firewall rules added'
# Start endpoint as detached process (-WindowStyle Hidden, NOT -NoNewWindow,
# because az vm run-command waits for all child processes sharing the console)
Start-Process -FilePath $exe -WindowStyle Hidden
# Scheduled task for reboot persistence
schtasks /Create /TN 'NetworkerEndpoint' /TR "$exe" /SC ONSTART /RU SYSTEM /F 2>$null | Out-Host
Start-Sleep 5
$listening = netstat -an 2>$null | Select-String ':8080.*LISTEN'
if ($listening) { Write-Host 'Endpoint listening on 8080' } else {
    Write-Host 'WARNING: Not listening on 8080 after 5s'
    $proc = Get-Process -Name 'networker-endpoint' -ErrorAction SilentlyContinue
    if ($proc) { Write-Host ('Process running: PID ' + $proc.Id) } else { Write-Host 'ERROR: Process not running' }
}
PSEOF

    az vm run-command invoke \
        --resource-group "$rg" --name "$vm" \
        --command-id RunPowerShellScript \
        --scripts "@${ps_tmp}" \
        --output table 2>/dev/null || print_warn "Service creation returned non-zero; check VM logs"
    rm -f "$ps_tmp"
    print_ok "networker-endpoint service created on Windows VM"
}

# ── step_generate_config: supports multiple endpoint IPs ─────────────────────
step_generate_config() {
    # In deploy-config mode, skip — config is generated by _deploy_generate_tester_config
    [[ -n "$DEPLOY_CONFIG_PATH" ]] && return 0

    local endpoint_ip="$1"

    next_step "Generate test config file"

    CONFIG_FILE_PATH="${PWD}/networker-cloud.json"

    # Build targets array: primary endpoint + any extra endpoints deployed
    local targets_json
    if [[ ${#AZURE_EXTRA_ENDPOINT_IPS[@]} -eq 0 ]]; then
        targets_json="\"https://${endpoint_ip}:8443/health\""
    else
        targets_json="\"https://${endpoint_ip}:8443/health\""
        for extra in "${AZURE_EXTRA_ENDPOINT_IPS[@]}"; do
            local extra_ip="${extra%%:*}"
            targets_json="${targets_json}, \"https://${extra_ip}:8443/health\""
        done
    fi

    cat > "$CONFIG_FILE_PATH" <<EOF
{
  "targets": [${targets_json}],
  "modes": ["tcp", "http1", "http2", "http3", "udp", "download", "upload",
             "pageload", "pageload2", "pageload3"],
  "runs": 5,
  "insecure": true,
  "udp_port": 9999,
  "udp_throughput_port": 9998,
  "payload_sizes": ["64k", "1m"],
  "html_report": "report.html"
}
EOF
    print_ok "Config written to ${CONFIG_FILE_PATH}"
    if [[ ${#AZURE_EXTRA_ENDPOINT_IPS[@]} -gt 0 ]]; then
        print_info "Multi-region config: $(( ${#AZURE_EXTRA_ENDPOINT_IPS[@]} + 1 )) endpoints in targets list"
    fi

    # If the tester is also remote, upload the config there too
    local tester_ip="" tester_user="" tester_scp_opts=(-o StrictHostKeyChecking=accept-new -q)
    case "$TESTER_LOCATION" in
        lan)   tester_ip="$LAN_TESTER_IP"; tester_user="$LAN_TESTER_USER"
               tester_scp_opts=(-o StrictHostKeyChecking=accept-new -P "$LAN_TESTER_PORT" -q) ;;
        azure) tester_ip="$AZURE_TESTER_IP"; tester_user="azureuser" ;;
        aws)   tester_ip="$AWS_TESTER_IP";   tester_user="ubuntu" ;;
        gcp)   tester_ip="$GCP_TESTER_IP";   tester_user="$(whoami)" ;;
    esac
    if [[ -n "$tester_ip" ]]; then
        print_info "Uploading config to tester ($tester_ip)…"
        scp "${tester_scp_opts[@]}" \
            "$CONFIG_FILE_PATH" \
            "${tester_user}@${tester_ip}:~/networker-cloud.json"
        print_ok "Config uploaded to ~/networker-cloud.json on tester"
    fi
}

# ──────────────────────────────────────────────────────────────────────────────
# Azure deployment steps
# ──────────────────────────────────────────────────────────────────────────────

step_check_azure_prereqs() {
    next_step "Check Azure prerequisites"

    if [[ $AZURE_CLI_AVAILABLE -eq 0 ]]; then
        print_err "Azure CLI (az) is not installed."
        echo ""
        echo "  Install from: https://docs.microsoft.com/cli/azure/install-azure-cli"
        echo "    macOS:  brew install azure-cli"
        echo "    Linux:  curl -sL https://aka.ms/InstallAzureCLIDeb | sudo bash"
        exit 1
    fi
    print_ok "az CLI found"

    # Re-check: service principal env vars may have been set after discover_system
    if [[ $AZURE_LOGGED_IN -eq 0 && -n "${AZURE_CLIENT_ID:-}" && -n "${AZURE_CLIENT_SECRET:-}" && -n "${AZURE_TENANT_ID:-}" ]]; then
        if az account show &>/dev/null 2>&1 </dev/null; then
            AZURE_LOGGED_IN=1
        fi
    fi

    if [[ $AZURE_LOGGED_IN -eq 1 ]]; then
        local sub_name sub_id
        sub_name="$(az account show --query name -o tsv 2>/dev/null || echo "unknown")"
        sub_id="$(az account show --query id -o tsv 2>/dev/null || echo "")"
        print_ok "Azure credentials found  (subscription: ${sub_name}, ${sub_id})"
    else
        print_info "Not logged in to Azure — running az login…"
        az login --use-device-code </dev/tty
        if ! az account show &>/dev/null 2>&1; then
            print_err "Azure login failed."
            exit 1
        fi
        AZURE_LOGGED_IN=1
        local sub_name sub_id
        sub_name="$(az account show --query name -o tsv 2>/dev/null || echo "unknown")"
        sub_id="$(az account show --query id -o tsv 2>/dev/null || echo "")"
        print_ok "Subscription: ${sub_name}  (${sub_id})"
    fi
}

# Create an Azure resource group and Ubuntu 22.04 VM.
# $1 = label ("tester" or "endpoint"), $2 = RG, $3 = VM name,
# $4 = VM size, $5 = name of global variable to receive public IP
step_azure_create_vm() {
    local label="$1" rg="$2" vm="$3" size="$4" ip_var="$5"
    # os_type: "linux" | "windows" — read from AZURE_{TESTER,ENDPOINT}_OS
    local os_type="linux"
    [[ "$label" == "tester" ]]   && os_type="$AZURE_TESTER_OS"
    [[ "$label" == "endpoint" ]] && os_type="$AZURE_ENDPOINT_OS"

    next_step "Create Azure VM for $label ($vm in $AZURE_REGION)"

    print_info "Creating resource group '$rg' in ${AZURE_REGION}…"
    az group create --name "$rg" --location "$AZURE_REGION" --output none
    print_ok "Resource group: $rg"

    local image os_label
    if [[ "$os_type" == "windows" ]]; then
        image="Win2022Datacenter"
        os_label="Windows Server 2022"
    else
        image="Ubuntu2204"
        os_label="Ubuntu 22.04 LTS"
    fi

    local auth_options="--generate-ssh-keys"
    if [[ "$os_type" == "windows" ]]; then
        # Windows VMs require --admin-password (SSH keys not supported).
        # Generate a random password meeting Azure complexity requirements.
        local win_pass
        win_pass="Nwk$(openssl rand -base64 12 | tr -dc 'A-Za-z0-9' | head -c 12)!1"
        auth_options="--admin-password ${win_pass}"
    elif [[ "$os_type" == "linux" ]]; then
        local key_file
        for key_file in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
            if [[ -f "$key_file" ]]; then
                auth_options="--ssh-key-values @${key_file}"
                break
            fi
        done
    fi

    # Check if VM already exists in the resource group
    if az vm show --resource-group "$rg" --name "$vm" --output none 2>/dev/null; then
        echo ""
        print_warn "VM '$vm' already exists in resource group '$rg'."
        echo ""
        echo "    1) Reuse existing VM  [default]"
        echo "    2) Pick a different name"
        echo "    3) Delete and recreate"
        echo ""
        local choice
        if [[ $AUTO_YES -eq 1 ]]; then
            choice="1"
            print_info "Auto-selecting: Reuse existing VM"
        else
            printf "  Choice [1]: "
            read -r choice </dev/tty || true
            choice="${choice:-1}"
        fi

        case "$choice" in
            1)
                # Check power state — start if deallocated/stopped
                local power_state
                power_state="$(az vm get-instance-view --resource-group "$rg" --name "$vm" \
                    --query "instanceView.statuses[?starts_with(code,'PowerState/')].displayStatus" \
                    -o tsv 2>/dev/null || echo "")"
                if [[ "$power_state" == *"deallocated"* || "$power_state" == *"stopped"* ]]; then
                    print_info "VM is ${power_state} — starting…"
                    az vm start --resource-group "$rg" --name "$vm" --output none
                    print_ok "VM started"
                fi
                local ip
                ip="$(az vm show --resource-group "$rg" --name "$vm" \
                    --show-details --query publicIps -o tsv 2>/dev/null || echo "")"
                if [[ -z "$ip" ]]; then
                    print_err "Failed to retrieve VM public IP."
                    exit 1
                fi
                printf -v "$ip_var" "%s" "$ip"

                # Try to get FQDN (assign DNS label if missing)
                local pip_name; pip_name="$(az network public-ip list --resource-group "$rg" \
                    --query "[?ipAddress=='${ip}'].name" -o tsv 2>/dev/null || echo "")"
                local fqdn=""
                if [[ -n "$pip_name" ]]; then
                    fqdn="$(az network public-ip show --resource-group "$rg" --name "$pip_name" \
                        --query dnsSettings.fqdn -o tsv 2>/dev/null || echo "")"
                    if [[ -z "$fqdn" ]]; then
                        local dns_label; dns_label="$(echo "$vm" | tr '[:upper:]' '[:lower:]' | tr -cd 'a-z0-9-')"
                        az network public-ip update --resource-group "$rg" --name "$pip_name" \
                            --dns-name "$dns_label" --output none 2>/dev/null || true
                        fqdn="$(az network public-ip show --resource-group "$rg" --name "$pip_name" \
                            --query dnsSettings.fqdn -o tsv 2>/dev/null || echo "")"
                    fi
                fi
                local fqdn_var="${ip_var/IP/FQDN}"
                [[ "$fqdn_var" != "$ip_var" && -n "$fqdn" ]] && printf -v "$fqdn_var" "%s" "$fqdn"

                if [[ -n "$fqdn" ]]; then
                    print_ok "Reusing VM '$vm' — ${BOLD}${fqdn}${RESET} (${ip})"
                else
                    print_ok "Reusing VM '$vm' — Public IP: ${BOLD}${ip}${RESET}"
                fi
                return 0
                ;;
            2)
                printf "  New VM name: "
                local new_name; read -r new_name </dev/tty || true
                if [[ -z "$new_name" ]]; then
                    print_err "VM name is required."
                    exit 1
                fi
                vm="$new_name"
                if [[ "$label" == "tester" ]]; then
                    AZURE_TESTER_VM="$vm"
                else
                    AZURE_ENDPOINT_VM="$vm"
                fi
                ;;
            3)
                print_info "Deleting VM '$vm'…"
                az vm delete --resource-group "$rg" --name "$vm" --yes --output none
                print_ok "VM deleted"
                ;;
        esac
    fi

    print_info "Creating $os_label VM '$vm' ($size)…"
    print_dim "This typically takes 1–2 minutes…"
    echo ""

    local ip
    ip="$(az vm create \
        --resource-group "$rg" \
        --name "$vm" \
        --image "$image" \
        --size "$size" \
        --admin-username azureuser \
        $auth_options \
        --only-show-errors \
        --output tsv \
        --query publicIpAddress)"

    if [[ -z "$ip" ]]; then
        print_err "Failed to retrieve VM public IP address."
        echo "  Check the Azure portal for resource group: $rg"
        exit 1
    fi

    # Assign DNS label to public IP for FQDN (needed for IIS HTTP/3 SNI binding)
    local dns_label; dns_label="$(echo "$vm" | tr '[:upper:]' '[:lower:]' | tr -cd 'a-z0-9-')"
    local pip_name; pip_name="$(az network public-ip list --resource-group "$rg" \
        --query "[?ipAddress=='${ip}'].name" -o tsv 2>/dev/null || echo "")"
    local fqdn=""
    if [[ -n "$pip_name" ]]; then
        az network public-ip update --resource-group "$rg" --name "$pip_name" \
            --dns-name "$dns_label" --output none 2>/dev/null || true
        fqdn="$(az network public-ip show --resource-group "$rg" --name "$pip_name" \
            --query dnsSettings.fqdn -o tsv 2>/dev/null || echo "")"
    fi

    printf -v "$ip_var" "%s" "$ip"
    # Store FQDN in a matching variable (e.g., AZURE_ENDPOINT_FQDN)
    local fqdn_var="${ip_var/IP/FQDN}"
    [[ "$fqdn_var" != "$ip_var" && -n "$fqdn" ]] && printf -v "$fqdn_var" "%s" "$fqdn"

    if [[ -n "$fqdn" ]]; then
        print_ok "VM created ($os_label) — ${BOLD}${fqdn}${RESET} (${ip})"
    else
        print_ok "VM created ($os_label) — Public IP: ${BOLD}${ip}${RESET}"
    fi

    if [[ "$os_type" == "windows" && -n "${win_pass:-}" ]]; then
        # Store credentials for later display in the summary
        local safe_label
        safe_label="$(echo "$label" | tr '[:lower:]-' '[:upper:]_')"
        local pass_var="AZURE_${safe_label}_WIN_PASS"
        printf -v "$pass_var" "%s" "$win_pass"
        echo ""
        print_info "Windows credentials:"
        echo "    User:     azureuser"
        echo "    Password: ${win_pass}"
        echo "    RDP:      mstsc /v:${ip}"
        echo ""
    fi
}

# Open TCP 80/443/8080-8082/8443-8445 and UDP 8443/9998/9999 on the NSG for the endpoint VM.
# Uses az network nsg rule create (not az vm open-port) to avoid priority conflicts
# with the default-allow-ssh rule that Azure always places at priority 1000.
step_azure_open_endpoint_ports() {
    local rg="$1" vm="$2"

    next_step "Open firewall ports (Azure)"

    # Locate the NSG attached to this VM.
    local nsg_name
    nsg_name="$(az network nsg list \
        --resource-group "$rg" \
        --query "[?contains(name, '${vm}')].name | [0]" \
        -o tsv 2>/dev/null || echo "")"

    if [[ -z "$nsg_name" || "$nsg_name" == "None" ]]; then
        nsg_name="$(az network nsg list \
            --resource-group "$rg" \
            --query "[0].name" \
            -o tsv 2>/dev/null || echo "")"
    fi

    if [[ -z "$nsg_name" || "$nsg_name" == "None" ]]; then
        print_warn "Could not detect NSG name — open ports manually:"
        print_warn "  az network nsg rule create --resource-group $rg --nsg-name <nsg> \\"
        print_warn "    --name Networker-TCP --protocol Tcp --direction Inbound \\"
        print_warn "    --priority 1100 --destination-port-ranges 80 443 8080-8082 8443-8445 --access Allow"
        print_warn "  az network nsg rule create --resource-group $rg --nsg-name <nsg> \\"
        print_warn "    --name Networker-UDP --protocol Udp --direction Inbound \\"
        print_warn "    --priority 1110 --destination-port-ranges 8443 9998 9999 --access Allow"
        return 0
    fi

    print_info "Opening TCP 80, 443, 8080-8082, 8443-8445…"
    az network nsg rule create \
        --resource-group "$rg" \
        --nsg-name "$nsg_name" \
        --name "Networker-TCP" \
        --protocol Tcp \
        --direction Inbound \
        --priority 1100 \
        --destination-port-ranges 80 443 8080-8082 8443-8445 \
        --access Allow \
        --output none
    print_ok "TCP 80, 443, 8080-8082, 8443-8445 open"

    print_info "Opening UDP 8443, 9998, 9999…"
    az network nsg rule create \
        --resource-group "$rg" \
        --nsg-name "$nsg_name" \
        --name "Networker-UDP" \
        --protocol Udp \
        --direction Inbound \
        --priority 1110 \
        --destination-port-ranges 8443-8445 9998 9999 \
        --access Allow \
        --output none
    print_ok "UDP 8443-8445, 9998, 9999 open"
}

# Set Azure auto-shutdown policy (04:00 UTC = 11 PM EST).
step_azure_set_auto_shutdown() {
    local rg="$1" vm="$2" label="${3:-VM}"
    [[ "$AZURE_AUTO_SHUTDOWN" != "yes" ]] && return 0

    next_step "Set auto-shutdown policy for $label"
    local shutdown_err
    if shutdown_err="$(az vm auto-shutdown --resource-group "$rg" --name "$vm" \
            --location "$AZURE_REGION" --time 0400 --output none 2>&1)"; then
        print_ok "Auto-shutdown set: 04:00 UTC (11 PM EST) daily — VM stops automatically"
    else
        print_warn "Could not configure Azure auto-shutdown: ${shutdown_err}"
        print_warn "Set it manually:  az vm auto-shutdown -g $rg -n $vm --time 0400"
    fi
}

# Install an auto-shutdown cron job on a Linux VM via SSH (04:00 UTC = 11 PM EST).
step_aws_set_auto_shutdown() {
    local ip="$1" user="$2" label="${3:-instance}"
    [[ "$AWS_AUTO_SHUTDOWN" != "yes" ]] && return 0

    next_step "Set auto-shutdown cron for $label (04:00 UTC = 11 PM EST)"
    if ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "${user}@${ip}" \
        "echo '0 4 * * * root /sbin/shutdown -h now' | sudo tee /etc/cron.d/networker-autostop > /dev/null && sudo chmod 644 /etc/cron.d/networker-autostop" 2>/dev/null; then
        print_ok "Auto-shutdown cron installed: 04:00 UTC (11 PM EST) daily"
    else
        print_warn "Could not install auto-shutdown cron (non-critical — terminate instance manually when done)"
    fi
}

# Check if Chrome is on a remote Linux VM via SSH; if not, offer to install it.
# For Windows VMs, shows a manual install reminder (no automated install via SSH).
# $1=ip  $2=ssh-user  $3=pkg_mgr (apt-get|dnf|pacman|…)  $4=os ("linux"|"windows")
# $5=rg (Azure, Windows path only)  $6=vm (Azure, Windows path only)
_remote_offer_chrome_install() {
    local ip="$1" user="$2" pkg_mgr="${3:-apt-get}" os_type="${4:-linux}"
    local rg="${5:-}" vm="${6:-}"

    if [[ "$os_type" == "windows" ]]; then
        # Check via az run-command (PowerShell); show install reminder if missing
        local chrome_found=0
        if [[ -n "$rg" && -n "$vm" ]]; then
            local out
            out="$(az vm run-command invoke \
                --resource-group "$rg" --name "$vm" \
                --command-id RunPowerShellScript \
                --scripts 'if (Test-Path "C:\Program Files\Google\Chrome\Application\chrome.exe") { Write-Output "found" }' \
                --query 'value[0].message' -o tsv 2>/dev/null || echo "")"
            [[ "$out" == *"found"* ]] && chrome_found=1
        fi
        if [[ $chrome_found -eq 1 ]]; then
            print_ok "Chrome detected on Windows VM — browser probe enabled."
        else
            echo ""
            print_info "Chrome not found on Windows VM."
            print_info "To enable the browser probe, RDP into the VM and install Chrome:"
            print_dim  "  https://www.google.com/chrome/"
            print_dim  "  Then re-run: networker-tester --modes browser ..."
        fi
        return 0
    fi

    # Linux path: check via SSH
    if _remote_chrome_available "$ip" "$user"; then
        print_ok "Chrome/Chromium detected on VM — browser probe enabled."
        return 0
    fi
    echo ""
    print_info "Chrome/Chromium not found on the remote VM."
    print_info "Without it the browser probe (--modes browser) will be skipped."
    if [[ -z "$pkg_mgr" ]]; then
        print_dim "  Install manually on the VM: https://www.google.com/chrome/"
        return 0
    fi
    if ask_yn "Install Chromium on the remote VM (enables browser probe)?" "y"; then
        next_step "Install Chromium on remote VM"
        local install_cmd
        case "$pkg_mgr" in
            apt-get) install_cmd="sudo apt-get update -qq && sudo apt-get install -y chromium-browser 2>/dev/null || sudo apt-get install -y chromium" ;;
            dnf)     install_cmd="sudo dnf install -y chromium" ;;
            pacman)  install_cmd="sudo pacman -S --noconfirm chromium" ;;
            zypper)  install_cmd="sudo zypper install -y chromium" ;;
            *)       install_cmd="sudo apt-get install -y chromium-browser" ;;
        esac
        if ssh -o StrictHostKeyChecking=accept-new "${user}@${ip}" "$install_cmd" 2>/dev/null; then
            print_ok "Chromium installed on VM — browser probe enabled."
        else
            print_warn "Could not install Chromium (non-critical — browser probe will be skipped)."
        fi
    else
        print_info "Skipping Chrome — browser probe will not be available on this VM."
    fi
}

step_azure_deploy_tester() {
    step_check_azure_prereqs
    step_azure_create_vm "tester" \
        "$AZURE_TESTER_RG" "$AZURE_TESTER_VM" "$AZURE_TESTER_SIZE" "AZURE_TESTER_IP"
    step_azure_set_auto_shutdown "$AZURE_TESTER_RG" "$AZURE_TESTER_VM" "tester VM"

    next_step "Install networker-tester on Azure VM"
    if [[ "$AZURE_TESTER_OS" == "windows" ]]; then
        _azure_wait_for_windows_vm "$AZURE_TESTER_RG" "$AZURE_TESTER_VM" "tester VM"
        _remote_offer_chrome_install "" "" "" "windows" "$AZURE_TESTER_RG" "$AZURE_TESTER_VM"
        _azure_win_install_binary "networker-tester" \
            "$AZURE_TESTER_RG" "$AZURE_TESTER_VM" "${NETWORKER_VERSION:-latest}"
        echo ""
        print_info "To connect:  RDP to ${AZURE_TESTER_IP} as azureuser"
        print_info "Run tester:  networker-tester --target ... --modes http1,http2 --runs 5"
    else
        _wait_for_ssh "$AZURE_TESTER_IP" "azureuser" "tester VM"
        _remote_offer_chrome_install "$AZURE_TESTER_IP" "azureuser" "apt-get" "linux"
        _remote_install_binary "networker-tester" "$AZURE_TESTER_IP" "azureuser"
        echo ""
        print_info "To connect:  ssh azureuser@${AZURE_TESTER_IP}"
    fi
}

# Deploy one Azure endpoint VM. Callable multiple times for multi-region.
# $1 = RG, $2 = VM name, $3 = size, $4 = OS ("linux"|"windows"), $5 = ip output var, $6 = label
_azure_deploy_one_endpoint() {
    local rg="$1" vm="$2" size="$3" os_type="$4" ip_var="$5" label="${6:-endpoint}"

    # Temporarily override the OS for create_vm to pick up
    local saved_os="$AZURE_ENDPOINT_OS"
    AZURE_ENDPOINT_OS="$os_type"

    step_azure_create_vm "endpoint" "$rg" "$vm" "$size" "$ip_var"
    AZURE_ENDPOINT_OS="$saved_os"
    step_azure_set_auto_shutdown "$rg" "$vm" "$label"

    local ip="${!ip_var}"

    # Fast-path: if endpoint is already healthy with correct version, skip install
    if curl -sf --max-time 5 "http://${ip}:8080/health" &>/dev/null; then
        local running_ver
        running_ver="$(curl -sf --max-time 5 "http://${ip}:8080/health" 2>/dev/null \
                       | grep -o '"version":"[^"]*"' | head -1 | sed 's/"version":"//;s/"//')"
        local want_ver="${NETWORKER_VERSION:-}"
        want_ver="${want_ver#v}"  # strip leading v
        if [[ -n "$running_ver" && ( -z "$want_ver" || "$running_ver" == "$want_ver" ) ]]; then
            print_ok "Endpoint already healthy (v${running_ver}) at http://${ip}:8080 — skipping install"
            return 0
        fi
        print_info "Endpoint running v${running_ver} but want v${want_ver} — reinstalling…"
    fi

    step_azure_open_endpoint_ports "$rg" "$vm"

    next_step "Install networker-endpoint on Azure VM ($label)"
    if [[ "$os_type" == "windows" ]]; then
        _azure_wait_for_windows_vm "$rg" "$vm" "$label"
        _azure_win_install_binary "networker-endpoint" "$rg" "$vm" "${NETWORKER_VERSION:-latest}"
        next_step "Create networker-endpoint service ($label)"
        _azure_win_create_endpoint_service "$rg" "$vm"
        # IIS HTTP stack comparison setup
        next_step "Set up IIS for HTTP stack comparison ($label)"
        _azure_win_setup_iis "$rg" "$vm"
    else
        _wait_for_ssh "$ip" "azureuser" "$label"
        _remote_install_binary "networker-endpoint" "$ip" "azureuser"
        next_step "Create networker-endpoint service ($label)"
        _remote_create_endpoint_service "$ip" "azureuser"
        # nginx HTTP stack comparison setup
        _remote_setup_nginx "$ip" "azureuser"
        next_step "Verify endpoint health ($label)"
        _remote_verify_health "$ip" "azureuser"
    fi
}

step_azure_deploy_endpoint() {
    # Only check prereqs once (shared with tester path if both are Azure)
    if [[ "$TESTER_LOCATION" != "azure" || -z "$AZURE_TESTER_IP" ]]; then
        step_check_azure_prereqs
    fi

    _azure_deploy_one_endpoint \
        "$AZURE_ENDPOINT_RG" "$AZURE_ENDPOINT_VM" "$AZURE_ENDPOINT_SIZE" \
        "$AZURE_ENDPOINT_OS" "AZURE_ENDPOINT_IP" "endpoint ($AZURE_REGION)"

    # Offer to deploy additional endpoints in other regions for multi-region comparison
    local region_counter=2
    while ask_yn "Deploy another endpoint VM in a different region for comparison?" "n"; do
        AZURE_REGION_ASKED=0  # reset so user is prompted for a new region
        local extra_rg="networker-rg-endpoint-${region_counter}"
        local extra_vm="networker-endpoint-vm-${region_counter}"

        print_section "Additional Endpoint VM #${region_counter}"
        echo ""
        ask_azure_options "endpoint"   # re-prompts region/size/RG/VM/OS

        # apply the choices just set
        extra_rg="$AZURE_ENDPOINT_RG"
        extra_vm="$AZURE_ENDPOINT_VM"
        local extra_os="$AZURE_ENDPOINT_OS"
        local extra_ip_var="AZURE_EXTRA_IP_${region_counter}"
        declare -g "$extra_ip_var"=""

        _azure_deploy_one_endpoint \
            "$extra_rg" "$extra_vm" "$AZURE_ENDPOINT_SIZE" \
            "$extra_os" "$extra_ip_var" "endpoint-${region_counter} ($AZURE_REGION)"

        local extra_ip="${!extra_ip_var}"
        AZURE_EXTRA_ENDPOINT_IPS+=("${extra_ip}:${AZURE_REGION}")
        region_counter=$((region_counter + 1))
    done

    step_generate_config "$AZURE_ENDPOINT_IP"
}

# ──────────────────────────────────────────────────────────────────────────────
# AWS deployment steps
# ──────────────────────────────────────────────────────────────────────────────

step_check_aws_prereqs() {
    next_step "Check AWS prerequisites"

    if [[ $AWS_CLI_AVAILABLE -eq 0 ]]; then
        print_err "AWS CLI (aws) is not installed."
        echo ""
        echo "  Install from: https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html"
        echo "    macOS:  brew install awscli"
        echo "    Linux:  curl https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip -o /tmp/awscliv2.zip"
        echo "            unzip /tmp/awscliv2.zip -d /tmp && sudo /tmp/aws/install"
        exit 1
    fi
    print_ok "aws CLI found"

    # Re-check: env vars may have been set after discover_system ran
    if [[ $AWS_LOGGED_IN -eq 0 && -n "${AWS_ACCESS_KEY_ID:-}" && -n "${AWS_SECRET_ACCESS_KEY:-}" ]]; then
        if aws sts get-caller-identity &>/dev/null 2>&1 </dev/null; then
            AWS_LOGGED_IN=1
        fi
    fi

    if [[ $AWS_LOGGED_IN -eq 1 ]]; then
        local aws_ident
        aws_ident="$(aws sts get-caller-identity --query 'Arn' --output text 2>/dev/null </dev/null || echo 'unknown')"
        print_ok "AWS credentials found  ($aws_ident)"
    else
        echo ""
        print_warn "No active AWS credentials."
        echo ""
        echo "  Choose an authentication method:"
        echo "    1) AWS SSO / Identity Center  (device code — opens browser, no keys needed)"
        echo "    2) Access keys                (AWS_ACCESS_KEY_ID + secret)"
        echo ""
        printf "  Auth method [1/2, default 1]: "
        local aws_auth_method
        read -r aws_auth_method </dev/tty || true
        aws_auth_method="${aws_auth_method:-1}"

        if [[ "$aws_auth_method" == "2" ]]; then
            _aws_do_login_keys
        else
            _aws_do_login_sso
        fi

        if [[ $AWS_LOGGED_IN -eq 0 ]]; then
            print_err "AWS credentials are not valid."
            echo "  SSO:         aws configure sso && aws sso login"
            echo "  Access keys: aws configure"
            exit 1
        fi
    fi

    local aws_acct aws_arn
    aws_acct="$(aws sts get-caller-identity --query Account --output text 2>/dev/null || echo "unknown")"
    aws_arn="$(aws sts get-caller-identity --query Arn --output text 2>/dev/null || echo "")"
    print_ok "Account: ${aws_acct}  (${aws_arn})"
}

# Import SSH public key into AWS if not already present.
_aws_ensure_keypair() {
    local ssh_key_file=""
    for kf in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
        if [[ -f "$kf" ]]; then
            ssh_key_file="$kf"
            break
        fi
    done

    if [[ -z "$ssh_key_file" ]]; then
        print_warn "No local SSH public key found (~/.ssh/id_ed25519.pub or ~/.ssh/id_rsa.pub)."
        print_warn "The instance will be created without a key pair — SSH access will not be available."
        echo ""
        return 0
    fi

    print_info "Importing SSH public key as 'networker-keypair'…"

    # Check if key pair already exists
    local existing_fp
    existing_fp="$(aws ec2 describe-key-pairs \
        --region "$AWS_REGION" \
        --key-names networker-keypair \
        --query "KeyPairs[0].KeyFingerprint" \
        --output text 2>/dev/null || echo "")"

    if [[ -n "$existing_fp" && "$existing_fp" != "None" ]]; then
        # Key exists — delete and re-import to ensure it matches the local key
        aws ec2 delete-key-pair \
            --region "$AWS_REGION" \
            --key-name "networker-keypair" \
            --output text >/dev/null 2>&1 || true
    fi

    aws ec2 import-key-pair \
        --region "$AWS_REGION" \
        --key-name "networker-keypair" \
        --public-key-material "fileb://${ssh_key_file}" \
        --output text >/dev/null
    print_ok "Key pair: networker-keypair"
}

# Look up the latest Ubuntu 22.04 LTS AMI for the current region.
# Sets the global AWS_AMI_ID.
AWS_AMI_ID=""
_aws_find_ubuntu_ami() {
    print_info "Looking up Ubuntu 22.04 LTS AMI in ${AWS_REGION}…"
    AWS_AMI_ID="$(aws ec2 describe-images \
        --region "$AWS_REGION" \
        --owners 099720109477 \
        --filters \
            "Name=name,Values=ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*" \
            "Name=state,Values=available" \
            "Name=architecture,Values=x86_64" \
        --query "sort_by(Images, &CreationDate)[-1].ImageId" \
        --output text 2>/dev/null || echo "")"

    if [[ -z "$AWS_AMI_ID" || "$AWS_AMI_ID" == "None" ]]; then
        print_err "Could not find Ubuntu 22.04 AMI in region $AWS_REGION."
        exit 1
    fi
    print_ok "AMI: $AWS_AMI_ID  (Ubuntu 22.04 LTS)"
}

# Create a security group with the necessary inbound rules.
# $1 = "tester" or "endpoint"; $2 = variable name to receive the SG ID.
_aws_create_security_group() {
    local component="$1" sg_var="$2"
    local sg_name="networker-sg-${component}"

    # Reuse existing SG with same name if present (idempotent re-runs)
    local existing
    existing="$(aws ec2 describe-security-groups \
        --region "$AWS_REGION" \
        --filters "Name=group-name,Values=${sg_name}" \
        --query "SecurityGroups[0].GroupId" \
        --output text 2>/dev/null || echo "")"

    if [[ -n "$existing" && "$existing" != "None" ]]; then
        printf -v "$sg_var" "%s" "$existing"
        print_ok "Reusing existing security group: $existing  ($sg_name)"
        return 0
    fi

    print_info "Creating security group '$sg_name'…"
    local _sg_created
    _sg_created="$(aws ec2 create-security-group \
        --region "$AWS_REGION" \
        --group-name "$sg_name" \
        --description "Networker ${component} security group" \
        --query "GroupId" \
        --output text)"

    # SSH (always)
    aws ec2 authorize-security-group-ingress \
        --region "$AWS_REGION" --group-id "$_sg_created" \
        --protocol tcp --port 22 --cidr 0.0.0.0/0 --output text >/dev/null

    if [[ "$component" == "endpoint" ]]; then
        # TCP 80, 443, 8080-8082, 8443-8445 (80/443 redirect to 8080/8443 via iptables on VM)
        # Ports 8081/8444 = nginx, 8082/8445 = IIS (HTTP stack comparison)
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol tcp --port 80 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol tcp --port 443 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol tcp --port 8080-8082 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol tcp --port 8443-8445 --cidr 0.0.0.0/0 --output text >/dev/null
        # UDP 8443-8445 (QUIC for endpoint + nginx + IIS), 9998, 9999
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol udp --port 8443-8445 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol udp --port 9998 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol udp --port 9999 --cidr 0.0.0.0/0 --output text >/dev/null
        print_ok "Security group created: $_sg_created  (TCP 22/80/443/8080-8082/8443-8445, UDP 8443-8445/9998/9999)"
    else
        print_ok "Security group created: $_sg_created  (TCP 22)"
    fi

    printf -v "$sg_var" "%s" "$_sg_created"
}

# Launch an EC2 instance and wait until it is running.
# $1 = label, $2 = instance type, $3 = name tag,
# $4 = SG ID, $5 = instance_id var, $6 = ip var
_aws_launch_instance() {
    local label="$1" instance_type="$2" name_tag="$3"
    local sg_id="$4" instance_id_var="$5" ip_var="$6"

    # Check if an instance with this Name tag already exists (running or stopped)
    local existing_id
    existing_id="$(aws ec2 describe-instances \
        --region "$AWS_REGION" \
        --filters "Name=tag:Name,Values=${name_tag}" \
                  "Name=instance-state-name,Values=running,stopped,pending" \
        --query "Reservations[0].Instances[0].InstanceId" \
        --output text 2>/dev/null || echo "")"

    if [[ -n "$existing_id" && "$existing_id" != "None" ]]; then
        echo ""
        print_warn "EC2 instance '$name_tag' already exists ($existing_id)."
        echo ""
        echo "    1) Reuse existing instance  [default]"
        echo "    2) Pick a different name"
        echo "    3) Terminate and recreate"
        echo ""
        local choice
        if [[ $AUTO_YES -eq 1 ]]; then
            choice="1"
            print_info "Auto-selecting: Reuse existing instance"
        else
            printf "  Choice [1]: "
            read -r choice </dev/tty || true
            choice="${choice:-1}"
        fi

        case "$choice" in
            1)
                printf -v "$instance_id_var" "%s" "$existing_id"
                local public_ip
                public_ip="$(aws ec2 describe-instances \
                    --region "$AWS_REGION" \
                    --instance-ids "$existing_id" \
                    --query "Reservations[0].Instances[0].PublicIpAddress" \
                    --output text 2>/dev/null || echo "")"
                if [[ -z "$public_ip" || "$public_ip" == "None" ]]; then
                    # Instance may be stopped — start it
                    print_info "Starting stopped instance…"
                    aws ec2 start-instances --region "$AWS_REGION" \
                        --instance-ids "$existing_id" --output text >/dev/null
                    aws ec2 wait instance-running --region "$AWS_REGION" \
                        --instance-ids "$existing_id"
                    public_ip="$(aws ec2 describe-instances \
                        --region "$AWS_REGION" \
                        --instance-ids "$existing_id" \
                        --query "Reservations[0].Instances[0].PublicIpAddress" \
                        --output text 2>/dev/null || echo "")"
                fi
                if [[ -z "$public_ip" || "$public_ip" == "None" ]]; then
                    print_err "Instance has no public IP."
                    exit 1
                fi
                printf -v "$ip_var" "%s" "$public_ip"
                print_ok "Reusing instance '$name_tag' — Public IP: ${BOLD}${public_ip}${RESET}"
                return 0
                ;;
            2)
                printf "  New instance name: "
                local new_name; read -r new_name </dev/tty || true
                if [[ -z "$new_name" ]]; then
                    print_err "Instance name is required."
                    exit 1
                fi
                name_tag="$new_name"
                if [[ "$label" == "tester" ]]; then
                    AWS_TESTER_NAME="$name_tag"
                else
                    AWS_ENDPOINT_NAME="$name_tag"
                fi
                ;;
            3)
                print_info "Terminating instance '$name_tag' ($existing_id)…"
                aws ec2 terminate-instances --region "$AWS_REGION" \
                    --instance-ids "$existing_id" --output text >/dev/null
                aws ec2 wait instance-terminated --region "$AWS_REGION" \
                    --instance-ids "$existing_id"
                print_ok "Instance terminated"
                ;;
        esac
    fi

    print_info "Launching EC2 instance ($instance_type, $name_tag)…"
    print_dim "This typically takes 1–2 minutes…"
    echo ""

    local key_opt=""
    for kf in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
        if [[ -f "$kf" ]]; then
            key_opt="--key-name networker-keypair"
            break
        fi
    done

    local instance_id
    instance_id="$(aws ec2 run-instances \
        --region "$AWS_REGION" \
        --image-id "$AWS_AMI_ID" \
        --instance-type "$instance_type" \
        $key_opt \
        --security-group-ids "$sg_id" \
        --tag-specifications \
            "ResourceType=instance,Tags=[{Key=Name,Value=${name_tag}}]" \
        --query "Instances[0].InstanceId" \
        --output text)"

    if [[ -z "$instance_id" || "$instance_id" == "None" ]]; then
        print_err "Failed to launch EC2 instance."
        exit 1
    fi
    printf -v "$instance_id_var" "%s" "$instance_id"
    print_ok "Instance launched: $instance_id"

    print_info "Waiting for instance to reach 'running' state…"
    aws ec2 wait instance-running \
        --region "$AWS_REGION" \
        --instance-ids "$instance_id"

    local public_ip public_dns
    public_ip="$(aws ec2 describe-instances \
        --region "$AWS_REGION" \
        --instance-ids "$instance_id" \
        --query "Reservations[0].Instances[0].PublicIpAddress" \
        --output text)"
    public_dns="$(aws ec2 describe-instances \
        --region "$AWS_REGION" \
        --instance-ids "$instance_id" \
        --query "Reservations[0].Instances[0].PublicDnsName" \
        --output text)"

    if [[ -z "$public_ip" || "$public_ip" == "None" ]]; then
        print_err "Instance has no public IP — check that it is in a public subnet."
        exit 1
    fi
    printf -v "$ip_var" "%s" "$public_ip"

    # Store FQDN from AWS public DNS
    local fqdn_var="${ip_var/IP/FQDN}"
    if [[ "$fqdn_var" != "$ip_var" && -n "$public_dns" && "$public_dns" != "None" ]]; then
        printf -v "$fqdn_var" "%s" "$public_dns"
        print_ok "Instance running — ${BOLD}${public_dns}${RESET} (${public_ip})"
    else
        print_ok "Instance running — Public IP: ${BOLD}${public_ip}${RESET}"
    fi
}

step_aws_deploy_tester() {
    step_check_aws_prereqs
    _aws_ensure_keypair
    _aws_find_ubuntu_ami

    next_step "Create AWS EC2 instance for tester ($AWS_TESTER_NAME, $AWS_REGION)"
    local sg_id
    _aws_create_security_group "tester" sg_id
    _aws_launch_instance "tester" \
        "$AWS_TESTER_INSTANCE_TYPE" "$AWS_TESTER_NAME" \
        "$sg_id" "AWS_TESTER_INSTANCE_ID" "AWS_TESTER_IP"

    _wait_for_ssh "$AWS_TESTER_IP" "ubuntu" "tester instance"
    step_aws_set_auto_shutdown "$AWS_TESTER_IP" "ubuntu" "tester instance"
    _remote_offer_chrome_install "$AWS_TESTER_IP" "ubuntu" "apt-get" "$AWS_TESTER_OS"

    next_step "Install networker-tester on AWS EC2"
    _remote_install_binary "networker-tester" "$AWS_TESTER_IP" "ubuntu"
}

step_aws_deploy_endpoint() {
    # Only check prereqs once
    if [[ "$TESTER_LOCATION" != "aws" || -z "$AWS_TESTER_IP" ]]; then
        step_check_aws_prereqs
        _aws_ensure_keypair
        _aws_find_ubuntu_ami
    fi

    next_step "Create AWS EC2 instance for endpoint ($AWS_ENDPOINT_NAME, $AWS_REGION)"
    local sg_id
    _aws_create_security_group "endpoint" sg_id
    _aws_launch_instance "endpoint" \
        "$AWS_ENDPOINT_INSTANCE_TYPE" "$AWS_ENDPOINT_NAME" \
        "$sg_id" "AWS_ENDPOINT_INSTANCE_ID" "AWS_ENDPOINT_IP"

    # Fast-path: if endpoint is already healthy with correct version, skip install
    if curl -sf --max-time 5 "http://${AWS_ENDPOINT_IP}:8080/health" &>/dev/null; then
        local running_ver
        running_ver="$(curl -sf --max-time 5 "http://${AWS_ENDPOINT_IP}:8080/health" 2>/dev/null \
                       | grep -o '"version":"[^"]*"' | head -1 | sed 's/"version":"//;s/"//')"
        local want_ver="${NETWORKER_VERSION:-}"
        want_ver="${want_ver#v}"  # strip leading v
        if [[ -n "$running_ver" && ( -z "$want_ver" || "$running_ver" == "$want_ver" ) ]]; then
            print_ok "Endpoint already healthy (v${running_ver}) at http://${AWS_ENDPOINT_IP}:8080 — skipping install"
            step_generate_config "$AWS_ENDPOINT_IP"
            return 0
        fi
        print_info "Endpoint running v${running_ver} but want v${want_ver} — reinstalling…"
    fi

    _wait_for_ssh "$AWS_ENDPOINT_IP" "ubuntu" "endpoint instance"
    step_aws_set_auto_shutdown "$AWS_ENDPOINT_IP" "ubuntu" "endpoint instance"

    next_step "Install networker-endpoint on AWS EC2"
    _remote_install_binary "networker-endpoint" "$AWS_ENDPOINT_IP" "ubuntu"

    next_step "Create networker-endpoint service (AWS)"
    _remote_create_endpoint_service "$AWS_ENDPOINT_IP" "ubuntu"

    # nginx HTTP stack comparison setup
    _remote_setup_nginx "$AWS_ENDPOINT_IP" "ubuntu"

    next_step "Verify endpoint health (AWS)"
    _remote_verify_health "$AWS_ENDPOINT_IP" "ubuntu"

    step_generate_config "$AWS_ENDPOINT_IP"
}

# ──────────────────────────────────────────────────────────────────────────────
# GCP deployment steps
# ──────────────────────────────────────────────────────────────────────────────

step_check_gcp_prereqs() {
    next_step "Check GCP prerequisites"

    if [[ $GCP_CLI_AVAILABLE -eq 0 ]]; then
        print_err "Google Cloud SDK (gcloud) is not installed."
        echo ""
        echo "  Install from: https://cloud.google.com/sdk/docs/install"
        echo "    macOS:  brew install --cask google-cloud-sdk"
        exit 1
    fi
    print_ok "gcloud CLI found"

    if [[ $GCP_LOGGED_IN -eq 0 ]]; then
        # Re-check in case gcloud was just added to PATH
        local gcp_account
        gcp_account="$(gcloud config get-value account 2>/dev/null || echo "")"
        if [[ -n "$gcp_account" && "$gcp_account" != "(unset)" ]]; then
            GCP_LOGGED_IN=1
        fi
    fi

    # Check GOOGLE_APPLICATION_CREDENTIALS (service account key file)
    if [[ $GCP_LOGGED_IN -eq 0 && -n "${GOOGLE_APPLICATION_CREDENTIALS:-}" && -f "${GOOGLE_APPLICATION_CREDENTIALS}" ]]; then
        if gcloud auth list --filter="status:ACTIVE" --format="value(account)" 2>/dev/null </dev/null | grep -q .; then
            GCP_LOGGED_IN=1
        fi
    fi

    if [[ $GCP_LOGGED_IN -eq 1 ]]; then
        local gcp_account
        gcp_account="$(gcloud config get-value account 2>/dev/null </dev/null || echo 'unknown')"
        print_ok "GCP credentials found  ($gcp_account)"
    else
        echo ""
        print_warn "Not logged in to GCP."
        print_info "Logging in via device code…"
        gcloud auth login --no-launch-browser </dev/tty
        local gcp_account
        gcp_account="$(gcloud config get-value account 2>/dev/null || echo "")"
        if [[ -n "$gcp_account" && "$gcp_account" != "(unset)" ]]; then
            GCP_LOGGED_IN=1
        else
            print_err "GCP login failed."
            echo "  Run:  gcloud auth login"
            exit 1
        fi
    fi

    if [[ -z "$GCP_PROJECT" ]]; then
        print_err "No GCP project set."
        echo "  Run:  gcloud config set project YOUR_PROJECT_ID"
        exit 1
    fi
    _gcp_resolve_project

    local gcp_account
    gcp_account="$(gcloud config get-value account 2>/dev/null || echo "")"
    print_ok "Account: ${gcp_account}  (project: ${GCP_PROJECT})"

    # Ensure Compute Engine API is enabled (required for VM creation, firewall rules, etc.)
    print_info "Checking Compute Engine API…"
    local api_status
    api_status="$(gcloud services list --enabled \
        --filter="config.name=compute.googleapis.com" \
        --format="value(config.name)" \
        --project="$GCP_PROJECT" 2>/dev/null || echo "")"
    if [[ "$api_status" != "compute.googleapis.com" ]]; then
        print_warn "Compute Engine API is not enabled on project $GCP_PROJECT."
        if ask_yn "Enable Compute Engine API now?" "y"; then
            print_info "Enabling Compute Engine API (may take a minute)…"
            if gcloud services enable compute.googleapis.com \
                    --project="$GCP_PROJECT" 2>&1; then
                print_ok "Compute Engine API enabled"
            else
                print_err "Failed to enable Compute Engine API."
                echo "  Enable manually: https://console.developers.google.com/apis/api/compute.googleapis.com/overview?project=${GCP_PROJECT}"
                exit 1
            fi
        else
            print_err "Compute Engine API is required for GCE deployment."
            echo "  Enable at: https://console.developers.google.com/apis/api/compute.googleapis.com/overview?project=${GCP_PROJECT}"
            exit 1
        fi
    else
        print_ok "Compute Engine API enabled"
    fi

    # Ensure billing is configured (Compute Engine requires an active billing account)
    local billing_enabled
    billing_enabled="$(gcloud billing projects describe "$GCP_PROJECT" \
        --format="value(billingEnabled)" 2>/dev/null || echo "")"
    if [[ "$billing_enabled" != "True" ]]; then
        print_warn "Billing is not enabled on project $GCP_PROJECT."
        echo "  GCE instances require an active billing account."
        echo "  Enable at: https://console.cloud.google.com/billing/linkedaccount?project=${GCP_PROJECT}"
        echo ""
        if ! ask_yn "Continue anyway (may fail at instance creation)?" "n"; then
            exit 1
        fi
    fi
}

# Create a GCE firewall rule for the endpoint ports (idempotent).
_gcp_create_firewall_rule() {
    local rule_name="networker-endpoint-allow"

    # Check if already exists
    if gcloud compute firewall-rules describe "$rule_name" \
            --project "$GCP_PROJECT" &>/dev/null 2>&1; then
        print_ok "Firewall rule '$rule_name' already exists — reusing"
        return 0
    fi

    print_info "Creating firewall rule '$rule_name'…"
    gcloud compute firewall-rules create "$rule_name" \
        --project "$GCP_PROJECT" \
        --direction=INGRESS \
        --priority=1000 \
        --network=default \
        --action=ALLOW \
        --rules=tcp:22,tcp:80,tcp:443,tcp:3389,tcp:8080-8082,tcp:8443-8445,udp:8443-8445,udp:9998,udp:9999 \
        --source-ranges=0.0.0.0/0 \
        --target-tags=networker-endpoint \
        --quiet
    print_ok "Firewall rule created: TCP 22/80/443/3389/8080-8082/8443-8445, UDP 8443-8445/9998/9999"
}

# Create a GCE instance.
# $1 = label ("tester"|"endpoint"), $2 = instance name, $3 = machine type,
# $4 = name of global variable to receive public IP
_gcp_create_instance() {
    local label="$1" name="$2" machine_type="$3" ip_var="$4"

    next_step "Create GCE instance for $label ($name in $GCP_ZONE)"

    # Check if instance already exists
    if gcloud compute instances describe "$name" \
            --project "$GCP_PROJECT" \
            --zone "$GCP_ZONE" &>/dev/null 2>&1; then
        echo ""
        print_warn "Instance '$name' already exists in $GCP_ZONE."
        echo ""
        echo "    1) Reuse existing instance  [default]"
        echo "    2) Pick a different name"
        echo "    3) Delete and recreate"
        echo ""
        local choice
        if [[ $AUTO_YES -eq 1 ]]; then
            choice="1"
            print_info "Auto-selecting: Reuse existing instance"
        else
            printf "  Choice [1]: "
            read -r choice </dev/tty || true
            choice="${choice:-1}"
        fi

        case "$choice" in
            1)
                # Check status — start if TERMINATED/STOPPED
                local inst_status
                inst_status="$(gcloud compute instances describe "$name" \
                    --project "$GCP_PROJECT" \
                    --zone "$GCP_ZONE" \
                    --format='get(status)' 2>/dev/null || echo "")"
                if [[ "$inst_status" == "TERMINATED" || "$inst_status" == "STOPPED" || "$inst_status" == "SUSPENDED" ]]; then
                    print_info "Instance is ${inst_status} — starting…"
                    gcloud compute instances start "$name" \
                        --project "$GCP_PROJECT" \
                        --zone "$GCP_ZONE" --quiet
                    print_ok "Instance started"
                fi
                local ip
                ip="$(gcloud compute instances describe "$name" \
                    --project "$GCP_PROJECT" \
                    --zone "$GCP_ZONE" \
                    --format='get(networkInterfaces[0].accessConfigs[0].natIP)' 2>/dev/null || echo "")"
                if [[ -z "$ip" ]]; then
                    print_err "Failed to retrieve instance public IP."
                    exit 1
                fi
                # Ensure SSH is enabled on Windows VMs
                local os_type="linux"
                [[ "$label" == "tester" ]]   && os_type="$GCP_TESTER_OS"
                [[ "$label" == "endpoint" ]] && os_type="$GCP_ENDPOINT_OS"
                if [[ "$os_type" == "windows" ]]; then
                    gcloud compute instances add-metadata "$name" \
                        --project "$GCP_PROJECT" \
                        --zone "$GCP_ZONE" \
                        --metadata=enable-windows-ssh=TRUE --quiet 2>/dev/null || true
                fi
                printf -v "$ip_var" "%s" "$ip"
                print_ok "Reusing instance '$name' — Public IP: ${BOLD}${ip}${RESET}"
                return 0
                ;;
            2)
                printf "  New instance name: "
                local new_name; read -r new_name </dev/tty || true
                if [[ -z "$new_name" ]]; then
                    print_err "Instance name is required."
                    exit 1
                fi
                name="$new_name"
                # Update the global variable so later steps use the new name
                if [[ "$label" == "tester" ]]; then
                    GCP_TESTER_NAME="$name"
                else
                    GCP_ENDPOINT_NAME="$name"
                fi
                ;;
            3)
                print_info "Deleting instance '$name'…"
                gcloud compute instances delete "$name" \
                    --project "$GCP_PROJECT" \
                    --zone "$GCP_ZONE" \
                    --quiet
                print_ok "Instance deleted"
                ;;
        esac
    fi

    local tags_opt=""
    [[ "$label" == "endpoint" ]] && tags_opt="--tags=networker-endpoint"

    # Determine OS image
    local os_type="linux"
    [[ "$label" == "tester" ]]   && os_type="$GCP_TESTER_OS"
    [[ "$label" == "endpoint" ]] && os_type="$GCP_ENDPOINT_OS"

    local image_family image_project os_label
    if [[ "$os_type" == "windows" ]]; then
        image_family="windows-2022"
        image_project="windows-cloud"
        os_label="Windows Server 2022"
    else
        image_family="ubuntu-2204-lts"
        image_project="ubuntu-os-cloud"
        os_label="Ubuntu 22.04"
    fi

    print_info "Creating $os_label VM '$name' ($machine_type)…"
    print_dim "This typically takes 1–2 minutes…"
    echo ""

    local metadata_opt=""
    if [[ "$os_type" == "windows" ]]; then
        # Enable SSH on Windows VMs so gcloud compute ssh works
        metadata_opt="--metadata=enable-windows-ssh=TRUE"
    fi

    gcloud compute instances create "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --machine-type "$machine_type" \
        --image-family "$image_family" \
        --image-project "$image_project" \
        $tags_opt \
        $metadata_opt \
        --quiet

    # Retrieve the external IP
    local ip
    ip="$(gcloud compute instances describe "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --format='get(networkInterfaces[0].accessConfigs[0].natIP)' 2>/dev/null || echo "")"

    if [[ -z "$ip" ]]; then
        print_err "Failed to retrieve instance public IP."
        echo "  Check the GCP Console for instance: $name"
        exit 1
    fi

    printf -v "$ip_var" "%s" "$ip"
    print_ok "Instance created — Public IP: ${BOLD}${ip}${RESET}"
}

# Wait for SSH access on a GCE instance via gcloud compute ssh (OS Login or metadata keys).
_gcp_wait_for_ssh() {
    local name="$1" label="${2:-instance}"
    print_info "Waiting for SSH access to ${label}…"
    local attempt=0
    while [[ $attempt -lt 30 ]]; do
        if gcloud compute ssh "$name" \
                --project "$GCP_PROJECT" \
                --zone "$GCP_ZONE" \
                --command "echo ok" \
                --quiet \
                --ssh-flag="-o ConnectTimeout=5" \
                --ssh-flag="-o StrictHostKeyChecking=accept-new" \
                &>/dev/null 2>&1; then
            print_ok "SSH available on $label"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 5
    done
    print_warn "SSH not available after 150s — continuing anyway"
}

# Run a command on a GCE instance via gcloud compute ssh.
_gcp_ssh_run() {
    local name="$1"
    shift
    gcloud compute ssh "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --quiet \
        --ssh-flag="-o StrictHostKeyChecking=accept-new" \
        --command "$*" < /dev/null
}

# Install a binary on a GCE instance using the bootstrap installer.
_gcp_install_binary() {
    local binary="$1" name="$2"
    local component
    [[ "$binary" == "networker-tester" ]] && component="tester" || component="endpoint"

    next_step "Install $binary on GCE instance"

    # Upload or download installer script on the VM
    print_info "Uploading installer to instance…"
    local script_path
    script_path="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
    if [[ -f "$script_path" ]]; then
        gcloud compute scp "$script_path" "${name}:/tmp/networker-install.sh" \
            --project "$GCP_PROJECT" \
            --zone "$GCP_ZONE" \
            --quiet 2>/dev/null || true
    fi

    # If SCP failed (e.g. curl|bash — no local file), download locally then SCP via gcloud
    if ! _gcp_ssh_run "$name" "test -f /tmp/networker-install.sh" 2>/dev/null; then
        print_dim "Downloading installer locally, then uploading to instance…"
        local tmp_installer="/tmp/networker-install-$$.sh"
        local gist_url="https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh"
        local repo_url="https://raw.githubusercontent.com/irlm/networker-tester/main/install.sh"
        if curl -fsSL "${gist_url}" -o "$tmp_installer" 2>/dev/null || \
           curl -fsSL "${repo_url}" -o "$tmp_installer" 2>/dev/null; then
            gcloud compute scp "$tmp_installer" "${name}:/tmp/networker-install.sh" \
                --project "$GCP_PROJECT" \
                --zone "$GCP_ZONE" \
                --quiet 2>/dev/null || true
            rm -f "$tmp_installer"
        else
            rm -f "$tmp_installer"
            # Last resort: try on the VM directly
            _gcp_ssh_run "$name" \
                "curl -fsSLk '${repo_url}' -o /tmp/networker-install.sh" 2>/dev/null || true
        fi
    fi

    # Ensure CA certificates are up to date (cloud VMs can have stale bundles)
    _gcp_ssh_run "$name" \
        "sudo apt-get update -qq && sudo apt-get install -y --only-upgrade ca-certificates >/dev/null 2>&1 || true" \
        2>/dev/null

    # Run the installer on the VM (handles Rust, build tools, binary install)
    if _gcp_ssh_run "$name" "test -f /tmp/networker-install.sh" 2>/dev/null; then
        print_info "Running installer on instance ($component)…"
        _gcp_ssh_run "$name" "bash /tmp/networker-install.sh $component -y" 2>&1
    else
        # Last resort: install build tools + Rust + cargo install
        print_info "Installing binary via cargo install…"
        _gcp_ssh_run "$name" \
            "sudo apt-get update -qq && sudo apt-get install -y build-essential 2>&1 | tail -1 ; \
             curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && \
             source \"\$HOME/.cargo/env\" && \
             cargo install --git ${REPO_HTTPS} ${binary}" 2>&1
    fi

    # Verify
    if _gcp_ssh_run "$name" "command -v $binary || test -f \"\$HOME/.cargo/bin/$binary\"" &>/dev/null 2>&1; then
        print_ok "$binary installed on GCE instance"
    else
        print_warn "$binary may not have installed correctly — check the instance manually"
    fi
}

# Create the endpoint systemd service on a GCE instance.
_gcp_create_endpoint_service() {
    local name="$1"
    next_step "Create networker-endpoint service (GCP)"

    # Find the binary path on the remote
    local bin_path
    bin_path="$(_gcp_ssh_run "$name" "command -v networker-endpoint || echo \"\$HOME/.cargo/bin/networker-endpoint\"" 2>/dev/null)"
    bin_path="${bin_path:-/usr/local/bin/networker-endpoint}"

    # Copy binary to /usr/local/bin if it's in .cargo
    _gcp_ssh_run "$name" "
        if [[ -f \"\$HOME/.cargo/bin/networker-endpoint\" ]]; then
            sudo cp \"\$HOME/.cargo/bin/networker-endpoint\" /usr/local/bin/networker-endpoint
            sudo chmod +x /usr/local/bin/networker-endpoint
        fi
    " 2>/dev/null || true

    _gcp_ssh_run "$name" "
        sudo useradd -r -s /bin/false networker 2>/dev/null || true
        sudo tee /etc/systemd/system/networker-endpoint.service > /dev/null <<'UNIT'
[Unit]
Description=Networker Endpoint
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/networker-endpoint
Restart=always
User=networker
AmbientCapabilities=CAP_NET_BIND_SERVICE
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
UNIT
        sudo systemctl daemon-reload
        sudo systemctl enable networker-endpoint
        sudo systemctl start networker-endpoint
    "
    print_ok "networker-endpoint service started"

    # Set up iptables redirects (80→8080, 443→8443)
    _gcp_ssh_run "$name" "
        sudo iptables -t nat -A PREROUTING -p tcp --dport 80  -j REDIRECT --to-port 8080
        sudo iptables -t nat -A PREROUTING -p tcp --dport 443 -j REDIRECT --to-port 8443
        sudo mkdir -p /etc/iptables
        sudo sh -c 'iptables-save > /etc/iptables/rules.v4'
    " 2>/dev/null || true
    print_ok "iptables port redirects configured (80→8080, 443→8443)"
}

# ── GCP Windows VM helpers ────────────────────────────────────────────────────

# Wait for a Windows GCE VM to become responsive via gcloud compute ssh.
# GCE Windows Server 2022 images include OpenSSH; it takes 3–5 min after boot.
_gcp_wait_for_windows_vm() {
    local name="$1" label="${2:-VM}"
    print_info "Waiting for $label Windows VM to be ready (this may take 3–5 minutes)…"
    local attempt=0
    while true; do
        if gcloud compute ssh "$name" \
                --project "$GCP_PROJECT" \
                --zone "$GCP_ZONE" \
                --command "echo ready" \
                --quiet \
                --ssh-flag="-o ConnectTimeout=10" \
                --ssh-flag="-o StrictHostKeyChecking=accept-new" \
                &>/dev/null 2>&1; then
            echo ""
            print_ok "Windows VM ready (SSH available)"
            return 0
        fi
        attempt=$((attempt + 1))
        if [[ $attempt -gt 40 ]]; then
            echo ""
            print_warn "Windows VM not responding via SSH after ~7 minutes."
            print_info "Try: gcloud compute reset-windows-password $name --zone $GCP_ZONE"
            return 1
        fi
        printf "."
        sleep 10
    done
}

# Retrieve Windows credentials via gcloud compute reset-windows-password.
_gcp_reset_windows_password() {
    local name="$1" label="${2:-VM}"
    print_info "Setting Windows password for ${label}…"
    local creds
    creds="$(gcloud compute reset-windows-password "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --user networker \
        --quiet 2>&1)" || true

    local win_ip win_user win_pass
    win_ip="$(echo "$creds" | grep '^ip_address:' | awk '{print $2}')"
    win_user="$(echo "$creds" | grep '^username:' | awk '{print $2}')"
    win_pass="$(echo "$creds" | grep '^password:' | awk '{print $2}')"

    if [[ -z "$win_pass" ]]; then
        print_warn "Could not retrieve Windows password automatically."
        print_info "Run manually: gcloud compute reset-windows-password $name --zone $GCP_ZONE"
        return 0
    fi

    echo ""
    print_info "Windows credentials for $label:"
    echo "    User:     $win_user"
    echo "    Password: $win_pass"
    echo "    RDP:      mstsc /v:${win_ip}"
    echo ""
}

# Install a binary on a Windows GCE VM via gcloud compute ssh (PowerShell).
_gcp_win_install_binary() {
    local binary="$1" name="$2"
    local archive="${binary}-x86_64-pc-windows-msvc.zip"
    local ver="${NETWORKER_VERSION:-latest}"
    local url="${REPO_HTTPS}/releases/download/${ver}/${archive}"

    next_step "Install ${binary}.exe on GCE Windows VM"

    # Check release has Windows binary
    local has_assets
    has_assets="$(gh release view --repo "$REPO_GH" "$ver" --json assets \
                  -q '[.assets[].name] | join(" ")' 2>/dev/null || echo "")"
    if [[ -z "$has_assets" ]]; then
        has_assets="$(curl -fsSL "https://api.github.com/repos/${REPO_GH}/releases/tags/${ver}" 2>/dev/null \
                      | grep '"name"' | sed 's/.*"name":[[:space:]]*"\([^"]*\)".*/\1/' | tr '\n' ' ')"
    fi
    if ! printf '%s' "$has_assets" | grep -q "${binary}-.*windows"; then
        print_warn "Release ${ver} has no Windows binary for ${binary}."
        print_info "Falling back to source build on the VM…"
        _gcp_win_source_build "$binary" "$name"
        return $?
    fi

    print_info "Installing ${binary}.exe on Windows VM via PowerShell…"
    gcloud compute ssh "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --quiet \
        --ssh-flag="-o StrictHostKeyChecking=accept-new" \
        --command "powershell -Command \"\$ErrorActionPreference='Stop'; \
            New-Item -ItemType Directory -Force -Path C:\\networker-tmp | Out-Null; \
            New-Item -ItemType Directory -Force -Path C:\\networker | Out-Null; \
            Invoke-WebRequest -Uri '${url}' -OutFile 'C:\\networker-tmp\\${archive}' -UseBasicParsing; \
            Expand-Archive -Path 'C:\\networker-tmp\\${archive}' -DestinationPath 'C:\\networker' -Force; \
            Remove-Item -Recurse -Force C:\\networker-tmp; \
            \\\$mp=[System.Environment]::GetEnvironmentVariable('Path','Machine'); \
            if(\\\$mp -notlike '*C:\\networker*'){[System.Environment]::SetEnvironmentVariable('Path',\\\"\\\$mp;C:\\networker\\\",'Machine')}; \
            & 'C:\\networker\\${binary}.exe' --version\"" < /dev/null

    print_ok "${binary}.exe installed on GCE Windows VM"
}

# Fallback: build from source on Windows GCE VM (when no pre-built binary available).
_gcp_win_source_build() {
    local binary="$1" name="$2"
    print_info "Building $binary from source on Windows VM (this may take several minutes)…"
    gcloud compute ssh "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --quiet \
        --ssh-flag="-o StrictHostKeyChecking=accept-new" \
        --command "powershell -Command \"\$ErrorActionPreference='Stop'; \
            Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile C:\\rustup-init.exe -UseBasicParsing; \
            & C:\\rustup-init.exe -y --default-toolchain stable 2>&1 | Select-Object -Last 3; \
            \\\$env:Path = [System.Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [System.Environment]::GetEnvironmentVariable('Path','User'); \
            cargo install --git ${REPO_HTTPS} ${binary} 2>&1 | Select-Object -Last 5; \
            New-Item -ItemType Directory -Force -Path C:\\networker | Out-Null; \
            Copy-Item \\\"\\\$env:USERPROFILE\\.cargo\\bin\\${binary}.exe\\\" C:\\networker\\${binary}.exe -Force; \
            & 'C:\\networker\\${binary}.exe' --version\"" < /dev/null || {
        print_warn "Source build may have failed — check the VM manually"
        return 1
    }
    print_ok "${binary}.exe built and installed on GCE Windows VM"
}

# Deploy endpoint on GCP Windows VM using startup script (no SSH required).
# Sets a startup script that installs Rust, builds the binary, creates the service,
# and opens firewall ports. Then polls health check until the endpoint is ready.
_gcp_win_deploy_endpoint_via_startup() {
    local name="$1" ip="$2"

    next_step "Install networker-endpoint on GCE Windows VM (via startup script)"
    print_info "Setting startup script to install endpoint on Windows VM…"
    print_dim "Downloads pre-built binary from GitHub Releases (fast), or builds from source (slow fallback)."
    echo ""

    # Write the PowerShell startup script — downloads pre-built binary from GitHub Releases,
    # falls back to source build if no release assets exist.
    local ps_tmp
    ps_tmp="$(mktemp /tmp/networker-gcp-win-XXXXX.ps1)"

    cat > "$ps_tmp" <<'PSEOF'
$ErrorActionPreference = 'Continue'
$dest = 'C:\networker'
$binary = 'networker-endpoint'
$repo = 'irlm/networker-tester'
$logFile = 'C:\networker-install.log'

function Log($msg) {
    $ts = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    Add-Content -Path $logFile -Value "$ts  $msg"
    Write-Host "$ts  $msg"
}

# Clean up stale workers from prior runs
schtasks /End /TN 'NetworkerInstallWorker' 2>$null
schtasks /Delete /TN 'NetworkerInstallWorker' /F 2>$null
Stop-Process -Name $binary -Force -ErrorAction SilentlyContinue

# Skip if already listening
$listening = netstat -an 2>$null | Select-String ':8080.*LISTEN'
if ($listening) { Log 'Endpoint already listening on port 8080'; exit 0 }

Log 'Installing endpoint...'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
New-Item -ItemType Directory -Force -Path $dest | Out-Null

# Install VC++ Redistributable if missing (required for MSVC-built binaries)
if (-not (Test-Path 'C:\Windows\System32\vcruntime140.dll')) {
    Log 'Installing VC++ Redistributable...'
    $vcUrl = 'https://aka.ms/vs/17/release/vc_redist.x64.exe'
    Invoke-WebRequest -Uri $vcUrl -OutFile 'C:\vc_redist.x64.exe' -UseBasicParsing
    Start-Process -FilePath 'C:\vc_redist.x64.exe' -ArgumentList '/install','/quiet','/norestart' -Wait
    Remove-Item 'C:\vc_redist.x64.exe' -Force -ErrorAction SilentlyContinue
    if (Test-Path 'C:\Windows\System32\vcruntime140.dll') { Log 'VC++ Redistributable installed' }
    else { Log 'WARNING: VC++ Redistributable install may have failed' }
}
$dstExe = "$dest\${binary}.exe"
$stdoutLog = "$dest\endpoint-stdout.log"
$stderrLog = "$dest\endpoint-stderr.log"

# --- Try downloading pre-built binary from GitHub Releases ---
$downloaded = $false
try {
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -UseBasicParsing
    $asset = $releases.assets | Where-Object { $_.name -like '*endpoint*windows*' } | Select-Object -First 1
    if ($asset) {
        $zipPath = "C:\${binary}-win.zip"
        Log ('Downloading ' + $asset.name)
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath -UseBasicParsing
        Log ('ZIP size: ' + (Get-Item $zipPath).Length + ' bytes')
        Expand-Archive -Path $zipPath -DestinationPath $dest -Force
        Remove-Item $zipPath -Force -ErrorAction SilentlyContinue
        # Log what was extracted
        $extracted = Get-ChildItem $dest -Recurse | ForEach-Object { $_.FullName }
        Log ('Extracted files: ' + ($extracted -join ', '))
        if (Test-Path $dstExe) {
            $sz = (Get-Item $dstExe).Length
            Log ('Binary size: ' + $sz + ' bytes')
            $verOut = (& $dstExe --version 2>&1) | Out-String
            $verOut = $verOut.Trim()
            if ($verOut) { Log ('Version: ' + $verOut) } else { Log 'WARNING: --version returned empty' }
            if ($sz -gt 1000000) { $downloaded = $true } else { Log 'WARNING: Binary too small — likely corrupt download' }
        } else {
            Log ('ERROR: Expected binary not found at ' + $dstExe)
            Log ('Directory contents: ' + ($extracted -join ', '))
        }
    } else { Log 'No matching asset found in latest release' }
} catch { Log ('Release download failed: ' + $_.Exception.Message) }

# --- Fallback: build from source (slow — 30+ min) ---
if (-not $downloaded) {
    Log 'No pre-built binary — building from source...'
    $env:CARGO_HOME = 'C:\cargo'; $env:RUSTUP_HOME = 'C:\rustup'
    New-Item -ItemType Directory -Force -Path $env:CARGO_HOME -ErrorAction SilentlyContinue | Out-Null
    New-Item -ItemType Directory -Force -Path $env:RUSTUP_HOME -ErrorAction SilentlyContinue | Out-Null
    if (-not (Test-Path 'C:\BuildTools\VC\Tools\MSVC')) {
        Log 'Installing VS Build Tools...'; Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vs_buildtools.exe' -OutFile C:\vs_buildtools.exe -UseBasicParsing
        Start-Process -FilePath 'C:\vs_buildtools.exe' -ArgumentList '--quiet','--wait','--norestart','--nocache','--installPath','C:\BuildTools','--add','Microsoft.VisualStudio.Workload.VCTools','--includeRecommended' -Wait
    }
    if (-not (Test-Path 'C:\cargo\bin\cargo.exe')) {
        Log 'Installing Rust...'; Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile C:\rustup-init.exe -UseBasicParsing
        & C:\rustup-init.exe -y --default-toolchain stable 2>&1 | Out-Null
    }
    $env:Path = 'C:\cargo\bin;' + $env:Path
    Log 'Building from source...'; & C:\cargo\bin\cargo.exe install --git "https://github.com/$repo" $binary 2>&1 | Out-Null
    if (Test-Path "C:\cargo\bin\${binary}.exe") {
        Copy-Item "C:\cargo\bin\${binary}.exe" $dstExe -Force
        $verOut = (& $dstExe --version 2>&1) | Out-String
        Log ('Built: ' + $verOut.Trim())
    } else { Log 'ERROR: Build failed'; exit 1 }
}

# Firewall — add rules before starting the binary
netsh advfirewall firewall add rule name='Networker-HTTP'  protocol=TCP dir=in action=allow localport=8080 2>$null
netsh advfirewall firewall add rule name='Networker-HTTPS' protocol=TCP dir=in action=allow localport=8443 2>$null
netsh advfirewall firewall add rule name='Networker-UDP'   protocol=UDP dir=in action=allow localport='8443,9998,9999' 2>$null
Log 'Firewall rules added'

# Start the endpoint as a detached process (-WindowStyle Hidden creates a fully
# independent process that won't block the startup script runner or SSH session)
Log ('Starting endpoint: ' + $dstExe)
Start-Process -FilePath $dstExe -WindowStyle Hidden

# Also create scheduled task for persistence across reboots
schtasks /Create /TN 'NetworkerEndpoint' /TR "$dstExe" /SC ONSTART /RU SYSTEM /F 2>$null
Log 'Scheduled task created for reboot persistence'

# Wait for endpoint to start listening
Start-Sleep 5
$listening = netstat -an 2>$null | Select-String ':8080.*LISTEN'
if ($listening) {
    Log 'Endpoint listening on 8080'
} else {
    Log 'WARNING: Not listening on 8080 after 5s'
    # Check if process is running
    $proc = Get-Process -Name $binary -ErrorAction SilentlyContinue
    if ($proc) { Log ('Process running: PID ' + $proc.Id) } else { Log 'ERROR: Process not running' }
    # Log any stderr output for debugging
    if (Test-Path $stderrLog) {
        $err = Get-Content $stderrLog -ErrorAction SilentlyContinue
        if ($err) { Log ('stderr: ' + ($err -join '; ')) }
    }
    # Retry: wait 10 more seconds
    Start-Sleep 10
    $listening = netstat -an 2>$null | Select-String ':8080.*LISTEN'
    if ($listening) { Log 'Endpoint listening on 8080 (delayed start)' }
    else { Log 'WARNING: Still not listening — check logs at C:\networker\' }
}

# Auto-shutdown
schtasks /Create /TN 'NetworkerAutoShutdown' /TR 'shutdown /s /t 0' /SC DAILY /ST 04:00 /RU SYSTEM /F 2>$null

Log '=== INSTALL COMPLETE ==='
PSEOF

    gcloud compute instances add-metadata "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --metadata-from-file=windows-startup-script-ps1="$ps_tmp" \
        --quiet 2>/dev/null
    rm -f "$ps_tmp"

    # Reset VM to trigger the startup script
    print_info "Resetting VM to trigger install…"
    gcloud compute instances reset "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --quiet 2>/dev/null

    # Poll health check instead of SSH
    print_info "Waiting for endpoint to come online…"
    local attempt=0 max_attempts=180  # 180 × 15s = 45 min max
    while [[ $attempt -lt $max_attempts ]]; do
        if curl -sf --max-time 5 "http://${ip}:8080/health" &>/dev/null; then
            echo ""
            print_ok "Endpoint is healthy at http://${ip}:8080/health"
            return 0
        fi
        attempt=$((attempt + 1))
        if (( attempt % 4 == 0 )); then
            local mins=$(( attempt * 15 / 60 ))
            printf "\r  … waiting (%d min elapsed)" "$mins"
        fi
        sleep 15
    done
    echo ""
    print_warn "Endpoint not responding after 30 minutes."
    print_info "Check VM serial log: gcloud compute instances get-serial-port-output $name --zone $GCP_ZONE"
    print_info "Check install log via RDP: type C:\\networker-install.log"
}

# Create endpoint service on a GCE Windows VM (via SSH).
_gcp_win_create_endpoint_service() {
    local name="$1"
    next_step "Create networker-endpoint service (GCP)"
    print_info "Creating endpoint service and opening firewall ports…"
    gcloud compute ssh "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --quiet \
        --ssh-flag="-o StrictHostKeyChecking=accept-new" \
        --command "powershell -Command \"\$ErrorActionPreference='Continue'; \
            Stop-Process -Name 'networker-endpoint' -Force -ErrorAction SilentlyContinue; \
            netsh advfirewall firewall add rule name='Networker-HTTP'  protocol=TCP dir=in action=allow localport=8080; \
            netsh advfirewall firewall add rule name='Networker-HTTPS' protocol=TCP dir=in action=allow localport=8443; \
            netsh advfirewall firewall add rule name='Networker-UDP'   protocol=UDP dir=in action=allow localport='8443,9998,9999'; \
            Start-Process -FilePath 'C:\\networker\\networker-endpoint.exe' -WindowStyle Hidden; \
            schtasks /Create /TN 'NetworkerEndpoint' /TR 'C:\\networker\\networker-endpoint.exe' /SC ONSTART /RU SYSTEM /F 2>\\\$null\"" < /dev/null

    print_ok "networker-endpoint service created on GCE Windows VM"
}

# Set auto-shutdown via Windows scheduled task on a GCE instance.
_gcp_win_set_auto_shutdown() {
    local name="$1" label="${2:-instance}"
    [[ "$GCP_AUTO_SHUTDOWN" != "yes" ]] && return 0

    next_step "Set auto-shutdown for $label (04:00 UTC)"
    gcloud compute ssh "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --quiet \
        --ssh-flag="-o StrictHostKeyChecking=accept-new" \
        --command "powershell -Command \"\
            \\\$action = New-ScheduledTaskAction -Execute 'shutdown.exe' -Argument '/s /t 60 /f'; \
            \\\$trigger = New-ScheduledTaskTrigger -Daily -At '04:00'; \
            Register-ScheduledTask -TaskName 'NetworkerAutoShutdown' -Action \\\$action -Trigger \\\$trigger -User 'SYSTEM' -RunLevel Highest -Force\"" < /dev/null || {
        print_warn "Could not install auto-shutdown task (non-critical)"
        return 0
    }
    print_ok "Auto-shutdown task installed: 04:00 UTC daily"
}

# Verify endpoint health on a GCE instance.
_gcp_verify_health() {
    local name="$1" ip="$2"
    next_step "Verify endpoint health (GCP)"
    local attempt=0
    while [[ $attempt -lt 12 ]]; do
        if curl -sf --max-time 5 "http://${ip}:8080/health" &>/dev/null; then
            print_ok "Endpoint is healthy at http://${ip}:8080/health"
            return 0
        fi
        attempt=$((attempt + 1))
        sleep 5
    done
    print_warn "Could not reach endpoint health check — verify manually: curl http://${ip}:8080/health"
}

# Set auto-shutdown cron on a GCE instance (04:00 UTC = 11 PM EST).
_gcp_set_auto_shutdown() {
    local name="$1" label="${2:-instance}"
    [[ "$GCP_AUTO_SHUTDOWN" != "yes" ]] && return 0

    next_step "Set auto-shutdown cron for $label (04:00 UTC = 11 PM EST)"
    if _gcp_ssh_run "$name" \
        "echo '0 4 * * * root /sbin/shutdown -h now' | sudo tee /etc/cron.d/networker-autostop > /dev/null && sudo chmod 644 /etc/cron.d/networker-autostop" 2>/dev/null; then
        print_ok "Auto-shutdown cron installed: 04:00 UTC (11 PM EST) daily"
    else
        print_warn "Could not install auto-shutdown cron (non-critical — delete instance manually when done)"
    fi
}

step_gcp_deploy_tester() {
    step_check_gcp_prereqs
    _gcp_create_instance "tester" "$GCP_TESTER_NAME" "$GCP_TESTER_MACHINE_TYPE" "GCP_TESTER_IP"

    if [[ "$GCP_TESTER_OS" == "windows" ]]; then
        _gcp_wait_for_windows_vm "$GCP_TESTER_NAME" "tester instance"
        _gcp_reset_windows_password "$GCP_TESTER_NAME" "tester"
        _gcp_win_set_auto_shutdown "$GCP_TESTER_NAME" "tester instance"
        _gcp_win_install_binary "networker-tester" "$GCP_TESTER_NAME"
        echo ""
        print_info "To connect via RDP:  mstsc /v:${GCP_TESTER_IP}"
        print_info "Or SSH:  gcloud compute ssh $GCP_TESTER_NAME --zone $GCP_ZONE"
    else
        _gcp_wait_for_ssh "$GCP_TESTER_NAME" "tester instance"
        _gcp_set_auto_shutdown "$GCP_TESTER_NAME" "tester instance"
        _gcp_install_binary "networker-tester" "$GCP_TESTER_NAME"
        echo ""
        print_info "To connect:  gcloud compute ssh $GCP_TESTER_NAME --zone $GCP_ZONE"
    fi
}

step_gcp_deploy_endpoint() {
    # Only check prereqs once
    if [[ "$TESTER_LOCATION" != "gcp" || -z "$GCP_TESTER_IP" ]]; then
        step_check_gcp_prereqs
    fi

    _gcp_create_firewall_rule
    _gcp_create_instance "endpoint" "$GCP_ENDPOINT_NAME" "$GCP_ENDPOINT_MACHINE_TYPE" "GCP_ENDPOINT_IP"

    # Fast-path: if endpoint is already healthy with correct version, skip install
    if curl -sf --max-time 5 "http://${GCP_ENDPOINT_IP}:8080/health" &>/dev/null; then
        local running_ver
        running_ver="$(curl -sf --max-time 5 "http://${GCP_ENDPOINT_IP}:8080/health" 2>/dev/null \
                       | grep -o '"version":"[^"]*"' | head -1 | sed 's/"version":"//;s/"//')"
        local want_ver="${NETWORKER_VERSION:-}"
        want_ver="${want_ver#v}"  # strip leading v
        if [[ -n "$running_ver" && ( -z "$want_ver" || "$running_ver" == "$want_ver" ) ]]; then
            print_ok "Endpoint already healthy (v${running_ver}) at http://${GCP_ENDPOINT_IP}:8080 — skipping install"
            step_generate_config "$GCP_ENDPOINT_IP"
            return 0
        fi
        print_info "Endpoint running v${running_ver} but want v${want_ver} — reinstalling…"
    fi

    if [[ "$GCP_ENDPOINT_OS" == "windows" ]]; then
        _gcp_win_deploy_endpoint_via_startup "$GCP_ENDPOINT_NAME" "$GCP_ENDPOINT_IP"
        # IIS setup for GCP Windows would need gcloud SSH — deferred for now
    else
        _gcp_wait_for_ssh "$GCP_ENDPOINT_NAME" "endpoint instance"
        _gcp_set_auto_shutdown "$GCP_ENDPOINT_NAME" "endpoint instance"
        _gcp_install_binary "networker-endpoint" "$GCP_ENDPOINT_NAME"
        _gcp_create_endpoint_service "$GCP_ENDPOINT_NAME"
        # nginx HTTP stack comparison setup
        _gcp_setup_nginx "$GCP_ENDPOINT_NAME" "$GCP_ENDPOINT_IP"
        _gcp_verify_health "$GCP_ENDPOINT_NAME" "$GCP_ENDPOINT_IP"
    fi
    step_generate_config "$GCP_ENDPOINT_IP"
}

# ── Completion summary ────────────────────────────────────────────────────────
display_completion() {
    echo ""
    echo "${BOLD}══════════════════════════════════════════════════════════${RESET}"
    echo "${GREEN}${BOLD}  Installation complete!${RESET}"
    echo "${BOLD}══════════════════════════════════════════════════════════${RESET}"
    echo ""

    local do_local_tester=0 do_local_endpoint=0
    [[ $DO_INSTALL_TESTER -eq 1 && $DO_REMOTE_TESTER -eq 0 ]]     && do_local_tester=1
    [[ $DO_INSTALL_ENDPOINT -eq 1 && $DO_REMOTE_ENDPOINT -eq 0 ]]  && do_local_endpoint=1

    # ── Local PATH check ──────────────────────────────────────────────────────
    if [[ $do_local_tester -eq 1 || $do_local_endpoint -eq 1 ]]; then
        if ! echo ":${PATH}:" | grep -q ":${INSTALL_DIR}:"; then
            print_warn "${INSTALL_DIR} is not in your shell PATH."
            echo ""
            echo "  Run now:      export PATH=\"${INSTALL_DIR}:\$PATH\""
            echo "  Make permanent:"
            echo "    echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc   # bash"
            echo "    echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc    # zsh"
            echo ""
        fi
    fi

    # ── Local quick starts ────────────────────────────────────────────────────
    if [[ $do_local_tester -eq 1 ]]; then
        echo "  ${BOLD}networker-tester${RESET} quick start:"
        echo "    networker-tester --help"
        echo "    networker-tester --target http://localhost:8080/health --modes http1 --runs 3"
        echo ""
    fi

    if [[ $do_local_endpoint -eq 1 ]]; then
        echo "  ${BOLD}networker-endpoint${RESET} quick start:"
        echo "    networker-endpoint"
        echo "    # Listens on :8080 HTTP, :8443 HTTPS/H2/H3, :9998 UDP throughput, :9999 UDP echo"
        echo ""
    fi

    if [[ $DO_INSTALL_DASHBOARD -eq 1 ]]; then
        local dashboard_port
        dashboard_port="$(grep DASHBOARD_PORT /etc/networker-dashboard.env 2>/dev/null | cut -d= -f2)"
        dashboard_port="${dashboard_port:-3000}"
        echo "  ${BOLD}networker-dashboard${RESET}:"
        if [[ -n "${DASHBOARD_FQDN:-}" && ${DASHBOARD_NGINX_CONFIGURED:-0} -eq 1 ]]; then
            echo "    Web UI:   https://${DASHBOARD_FQDN}"
        elif [[ ${DASHBOARD_NGINX_CONFIGURED:-0} -eq 1 ]]; then
            local server_ip
            server_ip="$(curl -s --max-time 3 ifconfig.me 2>/dev/null || hostname -I 2>/dev/null | awk '{print $1}')"
            echo "    Web UI:   http://${server_ip:-localhost}"
        else
            echo "    Web UI:   http://localhost:${dashboard_port}"
        fi
        echo "    Service:  sudo systemctl status networker-dashboard"
        echo "    Logs:     sudo journalctl -u networker-dashboard -f"
        echo "    Config:   /etc/networker-dashboard.env"
        echo ""
        if [[ -n "${DASHBOARD_TEMP_PASSWORD:-}" ]]; then
            echo "  ╔══════════════════════════════════════════════════════════╗"
            echo "  ║  ${BOLD}Login credentials${RESET}                                      ║"
            echo "  ║                                                          ║"
            printf "  ║  Username:  ${BOLD}admin${RESET}%*s║\n" 37 ""
            printf "  ║  Password:  ${BOLD}%-16s${RESET}%*s║\n" "$DASHBOARD_TEMP_PASSWORD" 21 ""
            echo "  ║                                                          ║"
            echo "  ║  ${DIM}You will be asked to change the password on first login.${RESET} ║"
            echo "  ╚══════════════════════════════════════════════════════════╝"
            echo ""
        fi

        # SSH access and cloud credentials guide
        local server_ip
        server_ip="$(curl -s --max-time 3 ifconfig.me 2>/dev/null || hostname -I 2>/dev/null | awk '{print $1}')"
        if [[ -n "$server_ip" ]]; then
            echo "  ${BOLD}SSH access${RESET}:"
            echo "    ssh $(whoami)@${server_ip}"
            echo ""
        fi
        echo "  ${BOLD}Cloud credentials${RESET} (required to deploy endpoints from the dashboard):"
        echo ""
        echo "    ${DIM}Azure:${RESET}  az login                    ${DIM}# or: az login --identity (for VMs with managed identity)${RESET}"
        echo "    ${DIM}AWS:${RESET}    aws configure                ${DIM}# enter access key + secret${RESET}"
        echo "    ${DIM}GCP:${RESET}    gcloud auth login            ${DIM}# then: gcloud config set project PROJECT_ID${RESET}"
        echo ""
        echo "    After authenticating, restart the dashboard:"
        echo "    sudo systemctl restart networker-dashboard"
        echo ""
    fi

    # ── Remote tester summary ─────────────────────────────────────────────────
    if [[ $DO_REMOTE_TESTER -eq 1 ]]; then
        local t_ip="" t_user="" t_provider="" t_ssh_cmd=""
        case "$TESTER_LOCATION" in
            lan)   t_ip="$LAN_TESTER_IP"; t_user="$LAN_TESTER_USER"; t_provider="LAN";
                   if [[ "$LAN_TESTER_PORT" != "22" ]]; then
                       t_ssh_cmd="ssh -p ${LAN_TESTER_PORT} ${LAN_TESTER_USER}@${LAN_TESTER_IP}"
                   else
                       t_ssh_cmd="ssh ${LAN_TESTER_USER}@${LAN_TESTER_IP}"
                   fi ;;
            azure) t_ip="$AZURE_TESTER_IP"; t_user="azureuser"; t_provider="Azure";
                   t_ssh_cmd="ssh azureuser@${AZURE_TESTER_IP}" ;;
            aws)   t_ip="$AWS_TESTER_IP";   t_user="ubuntu";    t_provider="AWS";
                   t_ssh_cmd="ssh ubuntu@${AWS_TESTER_IP}" ;;
            gcp)   t_ip="$GCP_TESTER_IP";   t_user=""; t_provider="GCP";
                   t_ssh_cmd="gcloud compute ssh $GCP_TESTER_NAME --zone $GCP_ZONE" ;;
        esac
        if [[ -n "$t_ip" ]]; then
            echo "  ${BOLD}networker-tester${RESET} (${t_provider} ${t_ip}):"
            echo "    SSH:   ${t_ssh_cmd}"
            if [[ -n "$CONFIG_FILE_PATH" ]]; then
                if [[ "$TESTER_LOCATION" == "gcp" ]]; then
                    echo "    Tests: ${t_ssh_cmd} -- 'networker-tester --config ~/networker-cloud.json'"
                else
                    echo "    Tests: ssh ${t_user}@${t_ip} 'networker-tester --config ~/networker-cloud.json'"
                fi
            else
                if [[ "$TESTER_LOCATION" == "gcp" ]]; then
                    echo "    Run:   ${t_ssh_cmd} -- 'networker-tester --help'"
                else
                    echo "    Run:   ssh ${t_user}@${t_ip} 'networker-tester --help'"
                fi
            fi
            echo ""
        fi
    fi

    # ── Remote endpoint summary ───────────────────────────────────────────────
    if [[ $DO_REMOTE_ENDPOINT -eq 1 ]]; then
        local e_ip="" e_user="" e_provider="" e_ssh_cmd=""
        case "$ENDPOINT_LOCATION" in
            lan)   e_ip="$LAN_ENDPOINT_IP"; e_user="$LAN_ENDPOINT_USER"; e_provider="LAN";
                   if [[ "$LAN_ENDPOINT_PORT" != "22" ]]; then
                       e_ssh_cmd="ssh -p ${LAN_ENDPOINT_PORT} ${LAN_ENDPOINT_USER}@${LAN_ENDPOINT_IP}"
                   else
                       e_ssh_cmd="ssh ${LAN_ENDPOINT_USER}@${LAN_ENDPOINT_IP}"
                   fi ;;
            azure) e_ip="$AZURE_ENDPOINT_IP"; e_user="azureuser"; e_provider="Azure";
                   e_ssh_cmd="ssh azureuser@${AZURE_ENDPOINT_IP}" ;;
            aws)   e_ip="$AWS_ENDPOINT_IP";   e_user="ubuntu";    e_provider="AWS";
                   e_ssh_cmd="ssh ubuntu@${AWS_ENDPOINT_IP}" ;;
            gcp)   e_ip="$GCP_ENDPOINT_IP";   e_user=""; e_provider="GCP";
                   e_ssh_cmd="gcloud compute ssh $GCP_ENDPOINT_NAME --zone $GCP_ZONE" ;;
        esac
        if [[ -n "$e_ip" ]]; then
            echo "  ${BOLD}networker-endpoint${RESET} (${e_provider} ${e_ip}):"
            echo "    Health: curl http://${e_ip}:8080/health"
            echo "    SSH:    ${e_ssh_cmd}"
            if [[ "$ENDPOINT_LOCATION" == "gcp" ]]; then
                echo "    Logs:   ${e_ssh_cmd} -- 'sudo journalctl -u networker-endpoint -f'"
                echo "    Stop:   ${e_ssh_cmd} -- 'sudo systemctl stop networker-endpoint'"
            else
                echo "    Logs:   ssh ${e_user}@${e_ip} 'sudo journalctl -u networker-endpoint -f'"
                echo "    Stop:   ssh ${e_user}@${e_ip} 'sudo systemctl stop networker-endpoint'"
            fi
            echo ""
        fi
    fi

    # ── Generated config ──────────────────────────────────────────────────────
    if [[ -n "$CONFIG_FILE_PATH" ]]; then
        echo "  ${BOLD}Test config:${RESET}  ${CONFIG_FILE_PATH}"
        echo "    networker-tester --config ${CONFIG_FILE_PATH}"
        echo ""
    fi

    # ── Cleanup reminders ─────────────────────────────────────────────────────
    if [[ "$TESTER_LOCATION" == "azure" || "$ENDPOINT_LOCATION" == "azure" ]]; then
        if [[ "$AZURE_AUTO_SHUTDOWN" == "yes" ]]; then
            echo "  ${GREEN}Auto-shutdown configured:${RESET} Azure VMs will stop at 04:00 UTC (11 PM EST) daily."
            echo "  ${DIM}VMs stop but are NOT deleted — storage charges still apply.${RESET}"
        else
            echo "  ${YELLOW}${BOLD}⚠ Azure VMs are left running — delete them when done to avoid charges!${RESET}"
        fi
        echo ""
        echo "  ${DIM}Delete Azure resources when done testing:${RESET}"
        if [[ "$TESTER_LOCATION" == "azure" ]]; then
            printf "  ${DIM}  az group delete --name %s --yes --no-wait${RESET}\n" "$AZURE_TESTER_RG"
        fi
        if [[ "$ENDPOINT_LOCATION" == "azure" ]]; then
            printf "  ${DIM}  az group delete --name %s --yes --no-wait${RESET}\n" "$AZURE_ENDPOINT_RG"
        fi
        for extra in "${AZURE_EXTRA_ENDPOINT_IPS[@]}"; do
            local extra_rg; extra_rg="$(az vm list --query "[?publicIps=='${extra%%:*}'].resourceGroup | [0]" -o tsv 2>/dev/null || echo "")"
            [[ -n "$extra_rg" ]] && printf "  ${DIM}  az group delete --name %s --yes --no-wait${RESET}\n" "$extra_rg"
        done
        echo ""
    fi

    if [[ "$TESTER_LOCATION" == "aws" || "$ENDPOINT_LOCATION" == "aws" ]]; then
        if [[ "$AWS_AUTO_SHUTDOWN" == "yes" ]]; then
            echo "  ${GREEN}Auto-shutdown configured:${RESET} AWS instances will stop at 04:00 UTC (11 PM EST) daily."
            echo "  ${DIM}Instances stop but are NOT terminated — EBS storage charges still apply.${RESET}"
        else
            echo "  ${YELLOW}${BOLD}⚠ AWS instances are left running — terminate them when done to avoid charges!${RESET}"
        fi
        echo ""
        echo "  ${DIM}Terminate AWS instances when done testing:${RESET}"
        if [[ "$TESTER_LOCATION" == "aws" && -n "$AWS_TESTER_INSTANCE_ID" ]]; then
            printf "  ${DIM}  aws ec2 terminate-instances --region %s --instance-ids %s${RESET}\n" \
                "$AWS_REGION" "$AWS_TESTER_INSTANCE_ID"
        fi
        if [[ "$ENDPOINT_LOCATION" == "aws" && -n "$AWS_ENDPOINT_INSTANCE_ID" ]]; then
            printf "  ${DIM}  aws ec2 terminate-instances --region %s --instance-ids %s${RESET}\n" \
                "$AWS_REGION" "$AWS_ENDPOINT_INSTANCE_ID"
        fi
        echo ""
    fi

    if [[ "$TESTER_LOCATION" == "gcp" || "$ENDPOINT_LOCATION" == "gcp" ]]; then
        if [[ "$GCP_AUTO_SHUTDOWN" == "yes" ]]; then
            echo "  ${GREEN}Auto-shutdown configured:${RESET} GCP instances will stop at 04:00 UTC (11 PM EST) daily."
            echo "  ${DIM}Instances stop but are NOT deleted — disk storage charges still apply.${RESET}"
        else
            echo "  ${YELLOW}${BOLD}⚠ GCP instances are left running — delete them when done to avoid charges!${RESET}"
        fi
        echo ""
        echo "  ${DIM}Delete GCP instances when done testing:${RESET}"
        if [[ "$TESTER_LOCATION" == "gcp" ]]; then
            printf "  ${DIM}  gcloud compute instances delete %s --zone %s --quiet${RESET}\n" \
                "$GCP_TESTER_NAME" "$GCP_ZONE"
        fi
        if [[ "$ENDPOINT_LOCATION" == "gcp" ]]; then
            printf "  ${DIM}  gcloud compute instances delete %s --zone %s --quiet${RESET}\n" \
                "$GCP_ENDPOINT_NAME" "$GCP_ZONE"
        fi
        echo ""
    fi

    # ── Offer the complementary component if only one was installed ──────────
    _offer_also_endpoint

    # ── Offer quick test against the endpoint ────────────────────────────────
    _offer_quick_test

    # ── Offer to open SSH session ─────────────────────────────────────────────
    _offer_ssh_connect
}

# Run a quick networker-tester probe against the newly deployed endpoint from
# the local machine.  Only shown when a remote endpoint was just installed and
# networker-tester is available locally (or was just installed locally).
_offer_quick_test() {
    [[ $DO_REMOTE_ENDPOINT -eq 0 ]] && return 0   # no remote endpoint deployed

    # Determine endpoint IP
    local e_ip=""
    case "$ENDPOINT_LOCATION" in
        lan)   e_ip="$LAN_ENDPOINT_IP"   ;;
        azure) e_ip="$AZURE_ENDPOINT_IP" ;;
        aws)   e_ip="$AWS_ENDPOINT_IP"   ;;
        gcp)   e_ip="$GCP_ENDPOINT_IP"   ;;
    esac
    [[ -z "$e_ip" ]] && return 0

    # Find networker-tester (locally installed or just built)
    local tester_bin=""
    if command -v networker-tester &>/dev/null; then
        tester_bin="networker-tester"
    elif [[ -x "${INSTALL_DIR}/networker-tester" ]]; then
        tester_bin="${INSTALL_DIR}/networker-tester"
    elif [[ -x "./target/release/networker-tester" ]]; then
        tester_bin="./target/release/networker-tester"
    fi

    if [[ -z "$tester_bin" ]]; then
        echo ""
        echo "${BOLD}──────────────────────────────────────────────────────────${RESET}"
        print_info "Endpoint is live at ${e_ip}."
        print_info "networker-tester is not installed locally — it is needed to run tests from this machine."

        if ! ask_yn "Install networker-tester locally now so you can run a quick test?" "y"; then
            echo ""
            print_info "Install it later:"
            echo "  bash install.sh tester"
            echo ""
            print_info "Then run:"
            echo "  networker-tester --target https://${e_ip}:8443/health --modes http1,http2,http3 --runs 5 --insecure"
            return 0
        fi

        echo ""
        # Install using the same method already resolved for this session
        if [[ "$INSTALL_METHOD" == "release" ]]; then
            if ! step_download_release "networker-tester"; then
                step_ensure_cargo_env
                step_cargo_install "networker-tester"
            fi
        else
            step_ensure_cargo_env
            step_cargo_install "networker-tester"
        fi

        # Re-locate the binary after install
        if command -v networker-tester &>/dev/null; then
            tester_bin="networker-tester"
        elif [[ -x "${INSTALL_DIR}/networker-tester" ]]; then
            tester_bin="${INSTALL_DIR}/networker-tester"
        fi

        if [[ -z "$tester_bin" ]]; then
            print_warn "networker-tester install did not succeed — skipping quick test."
            return 0
        fi
    fi

    echo ""
    echo "${BOLD}──────────────────────────────────────────────────────────${RESET}"
    print_info "The endpoint is live at ${e_ip}."
    print_info "Quick test: HTTP/1.1 + HTTP/2 + HTTP/3, 5 runs each."

    if ! ask_yn "Run a quick test against the endpoint now (from this machine)?" "y"; then
        echo ""
        print_info "Run it later:"
        echo "  networker-tester --target https://${e_ip}:8443/health --modes http1,http2,http3 --runs 5 --insecure"
        return 0
    fi

    echo ""
    print_info "Running quick test — press Ctrl-C to stop early."
    echo ""

    # Build --target args from primary IP + any extra endpoint IPs
    local target_args=("--target" "https://${e_ip}:8443/health")
    for extra in "${AZURE_EXTRA_ENDPOINT_IPS[@]}"; do
        local extra_ip="${extra%%:*}"
        target_args+=("--target" "https://${extra_ip}:8443/health")
    done

    "$tester_bin" \
        "${target_args[@]}" \
        --modes http1,http2,http3,download,pageload,pageload2,pageload3 \
        --payload-sizes 1m \
        --page-assets 10 \
        --runs 5 \
        --insecure

    echo ""
    if [[ -f "output/report.html" ]]; then
        print_ok "Report saved: output/report.html"
        echo ""
        case "$SYS_OS" in
            Darwin) echo "  open output/report.html" ;;
            Linux)  echo "  xdg-open output/report.html" ;;
        esac
    fi
}

# If only the tester was installed (no endpoint anywhere), offer to also install/deploy
# an endpoint so the user has something to test against.
_offer_also_endpoint() {
    # Only relevant when no endpoint is installed or deployed
    [[ $DO_INSTALL_ENDPOINT -eq 1 || $DO_REMOTE_ENDPOINT -eq 1 ]] && return 0
    # And we must have actually installed a tester
    [[ $DO_INSTALL_TESTER -eq 0 && $DO_REMOTE_TESTER -eq 0 ]] && return 0
    # Skip in non-interactive / auto-yes mode
    [[ $AUTO_YES -eq 1 ]] && return 0

    echo ""
    echo "${BOLD}──────────────────────────────────────────────────────────${RESET}"
    print_info "networker-tester is installed but no endpoint was deployed."
    print_info "You need an endpoint to test against."
    echo ""
    echo "  ${BOLD}1)${RESET} Install networker-endpoint locally (test on this machine)"
    echo "  ${BOLD}2)${RESET} Deploy a networker-endpoint on a cloud VM (Azure, AWS, or GCP)"
    echo "  ${BOLD}3)${RESET} Skip — I already have an endpoint elsewhere"
    echo ""
    local ans
    printf "%b" "${CYAN}?${RESET} What would you like to do? [1/2/3] "
    read -r ans </dev/tty || true
    case "${ans:-3}" in
        1)
            echo ""
            if [[ "$INSTALL_METHOD" == "release" ]]; then
                if ! step_download_release "networker-endpoint"; then
                    step_ensure_cargo_env
                    step_cargo_install "networker-endpoint"
                fi
            else
                step_ensure_cargo_env
                step_cargo_install "networker-endpoint"
            fi
            echo ""
            print_ok "Start the endpoint with:"
            echo "  networker-endpoint"
            echo ""
            print_ok "Then test with:"
            echo "  networker-tester --target http://127.0.0.1:8080/health --modes http1,http2,http3 --runs 5"
            ;;
        2)
            echo ""
            print_info "Run the installer again to deploy a cloud endpoint:"
            echo "  bash install.sh endpoint --azure   # Azure"
            echo "  bash install.sh endpoint --aws     # AWS"
            echo "  bash install.sh endpoint --gcp     # GCP"
            ;;
        *)
            echo ""
            print_info "Test against any running endpoint:"
            echo "  networker-tester --target http://<host>:8080/health --modes http1,http2,http3 --runs 5"
            ;;
    esac
}

# Offer an interactive SSH session into the newly provisioned VM(s).
# Shown only for Linux remote VMs (Windows needs RDP, not SSH).
_offer_ssh_connect() {
    local ssh_targets=()   # array of "label|user@ip" pairs

    if [[ $DO_REMOTE_TESTER -eq 1 ]]; then
        case "$TESTER_LOCATION" in
            azure)
                [[ "$AZURE_TESTER_OS" != "windows" && -n "$AZURE_TESTER_IP" ]] && \
                    ssh_targets+=("networker-tester (Azure)|azureuser@${AZURE_TESTER_IP}")
                ;;
            aws)
                [[ "$AWS_TESTER_OS" != "windows" && -n "$AWS_TESTER_IP" ]] && \
                    ssh_targets+=("networker-tester (AWS)|ubuntu@${AWS_TESTER_IP}")
                ;;
            gcp)
                [[ -n "$GCP_TESTER_IP" ]] && \
                    ssh_targets+=("networker-tester (GCP)|gcloud:${GCP_TESTER_NAME}")
                ;;
        esac
    fi

    if [[ $DO_REMOTE_ENDPOINT -eq 1 ]]; then
        case "$ENDPOINT_LOCATION" in
            azure)
                [[ "$AZURE_ENDPOINT_OS" != "windows" && -n "$AZURE_ENDPOINT_IP" ]] && \
                    ssh_targets+=("networker-endpoint (Azure)|azureuser@${AZURE_ENDPOINT_IP}")
                ;;
            aws)
                [[ "$AWS_ENDPOINT_OS" != "windows" && -n "$AWS_ENDPOINT_IP" ]] && \
                    ssh_targets+=("networker-endpoint (AWS)|ubuntu@${AWS_ENDPOINT_IP}")
                ;;
            gcp)
                [[ -n "$GCP_ENDPOINT_IP" ]] && \
                    ssh_targets+=("networker-endpoint (GCP)|gcloud:${GCP_ENDPOINT_NAME}")
                ;;
        esac
    fi

    [[ ${#ssh_targets[@]} -eq 0 ]] && return 0   # no remote Linux VMs

    echo ""
    echo "${BOLD}──────────────────────────────────────────────────────────${RESET}"

    # Helper: connect to a target (handles gcloud: prefix for GCP)
    _ssh_connect_target() {
        local dest="$1"
        if [[ "$dest" == gcloud:* ]]; then
            local instance_name="${dest#gcloud:}"
            gcloud compute ssh "$instance_name" \
                --project "$GCP_PROJECT" \
                --zone "$GCP_ZONE" \
                --quiet
        else
            ssh -o StrictHostKeyChecking=accept-new "$dest"
        fi
    }

    _ssh_display_dest() {
        local dest="$1"
        if [[ "$dest" == gcloud:* ]]; then
            echo "gcloud compute ssh ${dest#gcloud:} --zone $GCP_ZONE"
        else
            echo "ssh $dest"
        fi
    }

    if [[ ${#ssh_targets[@]} -eq 1 ]]; then
        local label="${ssh_targets[0]%%|*}"
        local dest="${ssh_targets[0]##*|}"
        local display; display="$(_ssh_display_dest "$dest")"
        if ask_yn "Connect to ${label} via SSH now?  (${display})" "y"; then
            echo ""
            print_info "Connecting — type 'exit' to return to your shell."
            echo ""
            _ssh_connect_target "$dest"
        fi
    else
        echo "  Connect to a VM via SSH now?"
        local i=1
        for t in "${ssh_targets[@]}"; do
            local label="${t%%|*}"
            local dest="${t##*|}"
            local display; display="$(_ssh_display_dest "$dest")"
            printf "    %s) %s  (%s)\n" "$i" "$label" "$display"
            i=$((i + 1))
        done
        printf "    %s) Skip\n" "$i"
        echo ""
        printf "  Choice [%s]: " "$i"
        local ans; read -r ans </dev/tty || true
        ans="${ans:-$i}"
        if [[ "$ans" =~ ^[0-9]+$ ]] && [[ "$ans" -ge 1 ]] && [[ "$ans" -lt "$i" ]]; then
            local chosen="${ssh_targets[$((ans - 1))]}"
            local dest="${chosen##*|}"
            echo ""
            print_info "Connecting — type 'exit' to return to your shell."
            echo ""
            _ssh_connect_target "$dest"
        fi
    fi
}

# ══════════════════════════════════════════════════════════════════════════════
# Deploy-config mode: read a JSON config and deploy/test non-interactively
# ══════════════════════════════════════════════════════════════════════════════

# Validate JSON config structure and required fields.
# Sets DEPLOY_VALIDATE_ERRORS (count) and prints each issue.
_deploy_validate_config() {
    local cfg="$1"
    local errors=0

    # JSON syntax
    if ! jq empty "$cfg" 2>/dev/null; then
        print_err "Invalid JSON in $cfg"
        DEPLOY_VALIDATE_ERRORS=1
        return 1
    fi

    # version field
    local ver
    ver="$(jq -r '.version // empty' "$cfg")"
    if [[ -z "$ver" ]]; then
        print_err "Config missing 'version' field (expected 1)"
        errors=$((errors + 1))
    elif [[ "$ver" != "1" ]]; then
        print_err "Unsupported config version: $ver (expected 1)"
        errors=$((errors + 1))
    fi

    # tester section
    local t_provider
    t_provider="$(jq -r '.tester.provider // empty' "$cfg")"
    if [[ -z "$t_provider" ]]; then
        print_err "Config missing 'tester.provider'"
        errors=$((errors + 1))
    else
        case "$t_provider" in
            local|lan|azure|aws|gcp) ;;
            *) print_err "Unknown tester provider: $t_provider"; errors=$((errors + 1)) ;;
        esac
        if [[ "$t_provider" == "lan" ]]; then
            local t_ip; t_ip="$(jq -r '.tester.lan.ip // empty' "$cfg")"
            [[ -z "$t_ip" ]] && { print_err "tester.lan.ip is required"; errors=$((errors + 1)); }
        fi
        # Windows computer name limit for tester too
        local t_os; t_os="$(jq -r ".tester.${t_provider}.os // empty" "$cfg")"
        if [[ "$t_os" == "windows" ]]; then
            local t_vm=""
            case "$t_provider" in
                azure) t_vm="$(jq -r '.tester.azure.vm_name // empty' "$cfg")" ;;
                aws)   t_vm="$(jq -r '.tester.aws.instance_name // empty' "$cfg")" ;;
                gcp)   t_vm="$(jq -r '.tester.gcp.instance_name // empty' "$cfg")" ;;
            esac
            if [[ -n "$t_vm" && ${#t_vm} -gt 15 ]]; then
                print_err "tester: Windows VM name '$t_vm' is ${#t_vm} chars (max 15)"
                errors=$((errors + 1))
            fi
        fi
    fi

    # endpoints array
    local ep_count
    ep_count="$(jq '.endpoints | length // 0' "$cfg" 2>/dev/null)"
    if [[ "${ep_count:-0}" -eq 0 ]]; then
        print_err "Config must have at least one entry in 'endpoints' array"
        errors=$((errors + 1))
    else
        local i
        for i in $(seq 0 $((ep_count - 1))); do
            local ep_prov
            ep_prov="$(jq -r ".endpoints[$i].provider // empty" "$cfg")"
            if [[ -z "$ep_prov" ]]; then
                print_err "endpoints[$i] missing 'provider'"
                errors=$((errors + 1))
            else
                case "$ep_prov" in
                    local|lan|azure|aws|gcp) ;;
                    *) print_err "endpoints[$i]: unknown provider '$ep_prov'"; errors=$((errors + 1)) ;;
                esac
                if [[ "$ep_prov" == "lan" ]]; then
                    local eip; eip="$(jq -r ".endpoints[$i].lan.ip // empty" "$cfg")"
                    [[ -z "$eip" ]] && { print_err "endpoints[$i].lan.ip is required"; errors=$((errors + 1)); }
                fi
                # Windows computer name limit (15 chars) — applies to Azure, AWS, GCP
                local ep_os; ep_os="$(jq -r ".endpoints[$i].${ep_prov}.os // empty" "$cfg")"
                if [[ "$ep_os" == "windows" ]]; then
                    local vm_name=""
                    case "$ep_prov" in
                        azure) vm_name="$(jq -r ".endpoints[$i].azure.vm_name // empty" "$cfg")" ;;
                        aws)   vm_name="$(jq -r ".endpoints[$i].aws.instance_name // empty" "$cfg")" ;;
                        gcp)   vm_name="$(jq -r ".endpoints[$i].gcp.instance_name // empty" "$cfg")" ;;
                    esac
                    if [[ -n "$vm_name" && ${#vm_name} -gt 15 ]]; then
                        print_err "endpoints[$i]: Windows VM name '$vm_name' is ${#vm_name} chars (max 15)"
                        errors=$((errors + 1))
                    fi
                fi
                # Validate http_stacks per endpoint (OS compatibility)
                local stacks_count; stacks_count="$(jq ".endpoints[$i].http_stacks | length // 0" "$cfg" 2>/dev/null)"
                if [[ "${stacks_count:-0}" -gt 0 ]]; then
                    local valid_stacks="nginx iis caddy apache"
                    local s
                    for s in $(seq 0 $((stacks_count - 1))); do
                        local sname; sname="$(jq -r ".endpoints[$i].http_stacks[$s]" "$cfg")"
                        if ! echo "$valid_stacks" | grep -qw "$sname"; then
                            print_err "endpoints[$i].http_stacks[$s]: unknown stack '$sname' (valid: nginx, iis, caddy, apache)"
                            errors=$((errors + 1))
                        fi
                        # OS compatibility: iis requires windows, nginx requires linux
                        if [[ "$sname" == "iis" && "$ep_os" == "linux" ]]; then
                            print_err "endpoints[$i]: IIS requires Windows but os is 'linux'"
                            errors=$((errors + 1))
                        fi
                        if [[ "$sname" == "nginx" && "$ep_os" == "windows" ]]; then
                            print_err "endpoints[$i]: nginx requires Linux but os is 'windows'"
                            errors=$((errors + 1))
                        fi
                    done
                fi
            fi
        done
    fi

    # Test modes validation (if specified)
    local modes_count
    modes_count="$(jq '.tests.modes | length // 0' "$cfg" 2>/dev/null)"
    if [[ "${modes_count:-0}" -gt 0 ]]; then
        local valid_modes="tcp http1 http2 http3 udp download upload webdownload webupload udpdownload udpupload pageload pageload1 pageload2 pageload3 browser browser1 browser2 browser3"
        for i in $(seq 0 $((modes_count - 1))); do
            local m; m="$(jq -r ".tests.modes[$i]" "$cfg")"
            if ! echo "$valid_modes" | grep -qw "$m"; then
                print_err "tests.modes[$i]: unknown mode '$m'"
                errors=$((errors + 1))
            fi
        done
    fi

    # Test http_stacks validation (if specified)
    local test_stacks_count
    test_stacks_count="$(jq '.tests.http_stacks | length // 0' "$cfg" 2>/dev/null)"
    if [[ "${test_stacks_count:-0}" -gt 0 ]]; then
        local valid_stacks="nginx iis caddy apache"
        for i in $(seq 0 $((test_stacks_count - 1))); do
            local sn; sn="$(jq -r ".tests.http_stacks[$i]" "$cfg")"
            if ! echo "$valid_stacks" | grep -qw "$sn"; then
                print_err "tests.http_stacks[$i]: unknown stack '$sn' (valid: nginx, iis, caddy, apache)"
                errors=$((errors + 1))
            fi
        done
    fi

    # Optional dashboard section
    local has_dashboard
    has_dashboard="$(jq 'has("dashboard")' "$cfg" 2>/dev/null)"
    if [[ "$has_dashboard" == "true" ]]; then
        local dash_provider
        dash_provider="$(jq -r '.dashboard.provider // empty' "$cfg")"
        if [[ -n "$dash_provider" ]]; then
            case "$dash_provider" in
                local) ;;
                *) print_err "dashboard.provider: only 'local' is currently supported (got: $dash_provider)"; errors=$((errors + 1)) ;;
            esac
        fi
    fi

    DEPLOY_VALIDATE_ERRORS=$errors
}

# Check if any browser-dependent mode is requested in the deploy config.
# Returns 0 (true) if browser/pageload/pageload2/pageload3 is in modes.
_deploy_needs_browser() {
    local cfg="$1"
    local modes; modes="$(jq -c '.tests.modes // []' "$cfg" 2>/dev/null)"
    echo "$modes" | jq -e '[.[] | select(test("^(browser|pageload)"))] | length > 0' &>/dev/null
}

# Install Chrome/Chromium on the remote tester machine via SSH.
_deploy_install_chrome_remote() {
    local ssh_opts=(-o StrictHostKeyChecking=accept-new)
    local dest=""
    case "$TESTER_LOCATION" in
        lan)
            ssh_opts+=(-p "$LAN_TESTER_PORT")
            dest="${LAN_TESTER_USER}@${LAN_TESTER_IP}"
            ;;
        azure) dest="azureuser@${AZURE_TESTER_IP}" ;;
        aws)   dest="ubuntu@${AWS_TESTER_IP}" ;;
        gcp)
            print_info "Installing Chrome on GCP tester ($GCP_TESTER_NAME)…"
            gcloud compute ssh "$GCP_TESTER_NAME" \
                --project "$GCP_PROJECT" --zone "$GCP_ZONE" \
                --command 'sudo apt-get update -qq && (sudo apt-get install -y chromium-browser 2>/dev/null || sudo apt-get install -y chromium) && sudo apt-get install -y libnss3-tools 2>/dev/null; true' \
                </dev/null 2>&1
            local rc=$?
            if [[ $rc -eq 0 ]]; then
                print_ok "Chrome/Chromium installed on GCP tester"
            else
                print_warn "Chrome install on GCP tester may have failed (exit $rc)"
            fi
            return $rc
            ;;
    esac

    if [[ -z "$dest" ]]; then
        print_warn "Cannot determine remote tester destination for Chrome install"
        return 1
    fi

    print_info "Installing Chrome/Chromium on remote tester ($dest)…"
    # Detect package manager and install Chromium (handles no-sudo case)
    ssh "${ssh_opts[@]}" "$dest" bash -s <<'REMOTE_CHROME'
# Check if Chrome is already installed
for cmd in google-chrome chromium-browser chromium; do
    if command -v "$cmd" &>/dev/null; then
        echo "Chrome/Chromium already available: $(command -v "$cmd")"
        exit 0
    fi
done

# Check for passwordless sudo
HAS_SUDO=0
if sudo -n true 2>/dev/null; then
    HAS_SUDO=1
fi

if [[ $HAS_SUDO -eq 1 ]]; then
    if command -v apt-get &>/dev/null; then
        sudo apt-get update -qq
        sudo apt-get install -y chromium-browser 2>/dev/null || sudo apt-get install -y chromium
        sudo apt-get install -y libnss3-tools 2>/dev/null || true
    elif command -v dnf &>/dev/null; then
        sudo dnf install -y chromium
        sudo dnf install -y nss-tools 2>/dev/null || true
    elif command -v yum &>/dev/null; then
        sudo yum install -y chromium
    elif command -v pacman &>/dev/null; then
        sudo pacman -S --noconfirm chromium
    elif command -v apk &>/dev/null; then
        sudo apk add chromium
    fi
else
    # No sudo — try user-local Chrome install via direct download
    echo "No passwordless sudo — attempting user-local Chrome install…"
    mkdir -p "$HOME/.local/bin"
    CHROME_URL="https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb"
    TMPDIR="$(mktemp -d)"
    if curl -fsSL "$CHROME_URL" -o "$TMPDIR/chrome.deb" 2>/dev/null; then
        # Extract Chrome from .deb without dpkg (user-local)
        cd "$TMPDIR"
        ar x chrome.deb data.tar.xz 2>/dev/null && tar xf data.tar.xz 2>/dev/null
        if [[ -f opt/google/chrome/google-chrome ]]; then
            cp -r opt/google/chrome "$HOME/.local/google-chrome"
            ln -sf "$HOME/.local/google-chrome/google-chrome" "$HOME/.local/bin/google-chrome"
            echo "Chrome installed to $HOME/.local/bin/google-chrome"
        else
            echo "WARNING: Could not extract Chrome from .deb"
        fi
        cd - >/dev/null
    else
        echo "WARNING: Could not download Chrome"
    fi
    rm -rf "$TMPDIR"
fi

# Verify
for cmd in google-chrome chromium-browser chromium; do
    if command -v "$cmd" &>/dev/null; then
        echo "Chrome/Chromium installed successfully: $(command -v "$cmd")"
        exit 0
    fi
done
# Also check user-local path
if [[ -x "$HOME/.local/bin/google-chrome" ]]; then
    echo "Chrome installed at $HOME/.local/bin/google-chrome"
    exit 0
fi
echo "WARNING: Chrome/Chromium not found after install attempt"
exit 1
REMOTE_CHROME
    local rc=$?
    if [[ $rc -eq 0 ]]; then
        print_ok "Chrome/Chromium installed on remote tester"
    else
        print_warn "Chrome install on remote tester may have failed (exit $rc)"
    fi
    return 0
}

# Parse the deploy config and populate shell variables.
# Maps JSON fields into the same globals used by the interactive flow.
_deploy_parse_config() {
    local cfg="$1"

    # ── Tester ────────────────────────────────────────────────────────────
    local t_provider
    t_provider="$(jq -r '.tester.provider' "$cfg")"

    local t_install_method
    t_install_method="$(jq -r '.tester.install_method // "release"' "$cfg")"
    [[ "$t_install_method" == "source" ]] && FROM_SOURCE=1

    case "$t_provider" in
        local)
            TESTER_LOCATION="local"; DO_REMOTE_TESTER=0
            ;;
        lan)
            TESTER_LOCATION="lan"; DO_REMOTE_TESTER=1
            LAN_TESTER_IP="$(jq -r '.tester.lan.ip' "$cfg")"
            LAN_TESTER_USER="$(jq -r '.tester.lan.user // ""' "$cfg")"
            LAN_TESTER_PORT="$(jq -r '.tester.lan.port // 22' "$cfg")"
            ;;
        azure)
            TESTER_LOCATION="azure"; DO_REMOTE_TESTER=1
            AZURE_REGION="$(jq -r '.tester.azure.region // "eastus"' "$cfg")"
            AZURE_TESTER_RG="$(jq -r '.tester.azure.resource_group // "networker-rg-tester"' "$cfg")"
            AZURE_TESTER_VM="$(jq -r '.tester.azure.vm_name // "networker-tester-vm"' "$cfg")"
            AZURE_TESTER_SIZE="$(jq -r '.tester.azure.vm_size // "Standard_B2s"' "$cfg")"
            AZURE_TESTER_OS="$(jq -r '.tester.azure.os // "linux"' "$cfg")"
            local t_shutdown; t_shutdown="$(jq -r '.tester.azure.auto_shutdown // true' "$cfg")"
            [[ "$t_shutdown" == "true" ]] && AZURE_AUTO_SHUTDOWN="yes" || AZURE_AUTO_SHUTDOWN="no"
            ;;
        aws)
            TESTER_LOCATION="aws"; DO_REMOTE_TESTER=1
            AWS_REGION="$(jq -r '.tester.aws.region // "us-east-1"' "$cfg")"
            AWS_TESTER_NAME="$(jq -r '.tester.aws.instance_name // "networker-tester"' "$cfg")"
            AWS_TESTER_INSTANCE_TYPE="$(jq -r '.tester.aws.instance_type // "t3.small"' "$cfg")"
            AWS_TESTER_OS="$(jq -r '.tester.aws.os // "linux"' "$cfg")"
            local t_aws_shutdown; t_aws_shutdown="$(jq -r '.tester.aws.auto_shutdown // true' "$cfg")"
            [[ "$t_aws_shutdown" == "true" ]] && AWS_AUTO_SHUTDOWN="yes" || AWS_AUTO_SHUTDOWN="no"
            ;;
        gcp)
            TESTER_LOCATION="gcp"; DO_REMOTE_TESTER=1
            GCP_REGION="$(jq -r '.tester.gcp.region // "us-central1"' "$cfg")"
            GCP_ZONE="$(jq -r '.tester.gcp.zone // "us-central1-a"' "$cfg")"
            GCP_TESTER_NAME="$(jq -r '.tester.gcp.instance_name // "networker-tester"' "$cfg")"
            GCP_TESTER_MACHINE_TYPE="$(jq -r '.tester.gcp.machine_type // "e2-small"' "$cfg")"
            GCP_TESTER_OS="$(jq -r '.tester.gcp.os // "linux"' "$cfg")"
            GCP_PROJECT="$(jq -r '.tester.gcp.project // ""' "$cfg")"
            local t_gcp_shutdown; t_gcp_shutdown="$(jq -r '.tester.gcp.auto_shutdown // true' "$cfg")"
            [[ "$t_gcp_shutdown" == "true" ]] && GCP_AUTO_SHUTDOWN="yes" || GCP_AUTO_SHUTDOWN="no"
            ;;
    esac

    DO_INSTALL_TESTER=1

    # ── Endpoints ─────────────────────────────────────────────────────────
    DEPLOY_ENDPOINT_COUNT="$(jq '.endpoints | length' "$cfg")"

    local i
    for i in $(seq 0 $((DEPLOY_ENDPOINT_COUNT - 1))); do
        local ep_prov; ep_prov="$(jq -r ".endpoints[$i].provider" "$cfg")"
        local ep_label; ep_label="$(jq -r ".endpoints[$i].label // \"endpoint-$((i+1))\"" "$cfg")"
        local ep_stacks; ep_stacks="$(jq -r '(.endpoints['"$i"'].http_stacks // []) | join(",")' "$cfg")"
        DEPLOY_EP_PROVIDERS+=("$ep_prov")
        DEPLOY_EP_LABELS+=("$ep_label")
        DEPLOY_EP_HTTP_STACKS+=("$ep_stacks")
        DEPLOY_EP_IPS+=("")  # placeholder, filled after deploy
    done

    DO_INSTALL_ENDPOINT=1

    # ── Tests ─────────────────────────────────────────────────────────────
    # Note: jq's // operator treats false as falsy, so we use 'if .tests.run_tests == false'
    local run_tests; run_tests="$(jq -r 'if .tests.run_tests == false then "false" else "true" end' "$cfg")"
    [[ "$run_tests" == "true" ]] && DEPLOY_RUN_TESTS=1 || DEPLOY_RUN_TESTS=0

    DEPLOY_TEST_MODES="$(jq -c '.tests.modes // null' "$cfg")"
    DEPLOY_TEST_RUNS="$(jq -r '.tests.runs // ""' "$cfg")"
    DEPLOY_TEST_PAYLOAD_SIZES="$(jq -c '.tests.payload_sizes // null' "$cfg")"
    DEPLOY_TEST_INSECURE="$(jq -r '.tests.insecure // ""' "$cfg")"
    DEPLOY_TEST_CONNECTION_REUSE="$(jq -r '.tests.connection_reuse // ""' "$cfg")"
    DEPLOY_TEST_UDP_PORT="$(jq -r '.tests.udp_port // ""' "$cfg")"
    DEPLOY_TEST_UDP_THROUGHPUT_PORT="$(jq -r '.tests.udp_throughput_port // ""' "$cfg")"
    DEPLOY_TEST_PAGE_ASSETS="$(jq -r '.tests.page_assets // ""' "$cfg")"
    DEPLOY_TEST_PAGE_ASSET_SIZE="$(jq -r '.tests.page_asset_size // ""' "$cfg")"
    DEPLOY_TEST_PAGE_PRESET="$(jq -r '.tests.page_preset // ""' "$cfg")"
    DEPLOY_TEST_TIMEOUT="$(jq -r '.tests.timeout // ""' "$cfg")"
    DEPLOY_TEST_RETRIES="$(jq -r '.tests.retries // ""' "$cfg")"
    DEPLOY_TEST_HTML_REPORT="$(jq -r '.tests.html_report // ""' "$cfg")"
    DEPLOY_TEST_OUTPUT_DIR="$(jq -r '.tests.output_dir // ""' "$cfg")"
    DEPLOY_TEST_EXCEL="$(jq -r '.tests.excel // ""' "$cfg")"
    DEPLOY_TEST_CONCURRENCY="$(jq -r '.tests.concurrency // ""' "$cfg")"
    DEPLOY_TEST_DNS_ENABLED="$(jq -r '.tests.dns_enabled // ""' "$cfg")"
    DEPLOY_TEST_IPV4_ONLY="$(jq -r '.tests.ipv4_only // ""' "$cfg")"
    DEPLOY_TEST_IPV6_ONLY="$(jq -r '.tests.ipv6_only // ""' "$cfg")"
    DEPLOY_TEST_VERBOSE="$(jq -r '.tests.verbose // ""' "$cfg")"
    DEPLOY_TEST_LOG_LEVEL="$(jq -r '.tests.log_level // ""' "$cfg")"
    DEPLOY_TEST_HTTP_STACKS="$(jq -r '(.tests.http_stacks // []) | join(",")' "$cfg")"

    # packet_capture can be at top level OR inside tests (support both)
    DEPLOY_PACKET_CAPTURE_MODE="$(jq -r '(.packet_capture.mode // .tests.packet_capture.mode // "none")' "$cfg")"
    DEPLOY_PACKET_CAPTURE_INSTALL_REQS="$(jq -r '(.packet_capture.install_requirements // .tests.packet_capture.install_requirements // "")' "$cfg")"
    DEPLOY_PACKET_CAPTURE_INTERFACE="$(jq -r '(.packet_capture.interface // .tests.packet_capture.interface // "")' "$cfg")"
    DEPLOY_PACKET_CAPTURE_WRITE_PCAP="$(jq -r '(.packet_capture.write_pcap // .tests.packet_capture.write_pcap // "")' "$cfg")"
    DEPLOY_PACKET_CAPTURE_WRITE_SUMMARY_JSON="$(jq -r '(.packet_capture.write_summary_json // .tests.packet_capture.write_summary_json // "")' "$cfg")"
}

# Load endpoint config at index $1 into the provider-specific globals.
# This lets us reuse existing step_*_deploy_endpoint functions per endpoint.
_deploy_load_endpoint() {
    local idx="$1"
    local cfg="$DEPLOY_CONFIG_PATH"
    local ep_prov="${DEPLOY_EP_PROVIDERS[$idx]}"

    case "$ep_prov" in
        local)
            ENDPOINT_LOCATION="local"; DO_REMOTE_ENDPOINT=0
            ;;
        lan)
            ENDPOINT_LOCATION="lan"; DO_REMOTE_ENDPOINT=1
            LAN_ENDPOINT_IP="$(jq -r ".endpoints[$idx].lan.ip" "$cfg")"
            LAN_ENDPOINT_USER="$(jq -r ".endpoints[$idx].lan.user // \"\"" "$cfg")"
            LAN_ENDPOINT_PORT="$(jq -r ".endpoints[$idx].lan.port // 22" "$cfg")"
            ;;
        azure)
            ENDPOINT_LOCATION="azure"; DO_REMOTE_ENDPOINT=1
            # Use endpoint-specific region if set, else fallback to tester's
            local ep_region; ep_region="$(jq -r ".endpoints[$idx].azure.region // \"$AZURE_REGION\"" "$cfg")"
            AZURE_REGION="$ep_region"
            AZURE_ENDPOINT_RG="$(jq -r ".endpoints[$idx].azure.resource_group // \"networker-rg-endpoint\"" "$cfg")"
            AZURE_ENDPOINT_VM="$(jq -r ".endpoints[$idx].azure.vm_name // \"networker-endpoint-vm\"" "$cfg")"
            AZURE_ENDPOINT_SIZE="$(jq -r ".endpoints[$idx].azure.vm_size // \"Standard_B2s\"" "$cfg")"
            AZURE_ENDPOINT_OS="$(jq -r ".endpoints[$idx].azure.os // \"linux\"" "$cfg")"
            local ep_shutdown; ep_shutdown="$(jq -r ".endpoints[$idx].azure.auto_shutdown // true" "$cfg")"
            [[ "$ep_shutdown" == "true" ]] && AZURE_AUTO_SHUTDOWN="yes" || AZURE_AUTO_SHUTDOWN="no"
            ;;
        aws)
            ENDPOINT_LOCATION="aws"; DO_REMOTE_ENDPOINT=1
            local ep_aws_region; ep_aws_region="$(jq -r ".endpoints[$idx].aws.region // \"$AWS_REGION\"" "$cfg")"
            AWS_REGION="$ep_aws_region"
            AWS_ENDPOINT_NAME="$(jq -r ".endpoints[$idx].aws.instance_name // \"networker-endpoint\"" "$cfg")"
            AWS_ENDPOINT_INSTANCE_TYPE="$(jq -r ".endpoints[$idx].aws.instance_type // \"t3.small\"" "$cfg")"
            AWS_ENDPOINT_OS="$(jq -r ".endpoints[$idx].aws.os // \"linux\"" "$cfg")"
            local ep_aws_shutdown; ep_aws_shutdown="$(jq -r ".endpoints[$idx].aws.auto_shutdown // true" "$cfg")"
            [[ "$ep_aws_shutdown" == "true" ]] && AWS_AUTO_SHUTDOWN="yes" || AWS_AUTO_SHUTDOWN="no"
            ;;
        gcp)
            ENDPOINT_LOCATION="gcp"; DO_REMOTE_ENDPOINT=1
            local ep_gcp_region; ep_gcp_region="$(jq -r ".endpoints[$idx].gcp.region // \"$GCP_REGION\"" "$cfg")"
            GCP_REGION="$ep_gcp_region"
            local ep_gcp_zone; ep_gcp_zone="$(jq -r ".endpoints[$idx].gcp.zone // \"$GCP_ZONE\"" "$cfg")"
            GCP_ZONE="$ep_gcp_zone"
            GCP_ENDPOINT_NAME="$(jq -r ".endpoints[$idx].gcp.instance_name // \"networker-endpoint\"" "$cfg")"
            GCP_ENDPOINT_MACHINE_TYPE="$(jq -r ".endpoints[$idx].gcp.machine_type // \"e2-small\"" "$cfg")"
            GCP_ENDPOINT_OS="$(jq -r ".endpoints[$idx].gcp.os // \"linux\"" "$cfg")"
            GCP_PROJECT="$(jq -r ".endpoints[$idx].gcp.project // \"$GCP_PROJECT\"" "$cfg")"
            local ep_gcp_shutdown; ep_gcp_shutdown="$(jq -r ".endpoints[$idx].gcp.auto_shutdown // true" "$cfg")"
            [[ "$ep_gcp_shutdown" == "true" ]] && GCP_AUTO_SHUTDOWN="yes" || GCP_AUTO_SHUTDOWN="no"
            ;;
    esac
}

# Run pre-flight checks: tools, credentials, connectivity.
# Accumulates all errors and prints them, then exits if any found.
_deploy_preflight() {
    local cfg="$1"
    local errors=0

    print_section "Pre-flight checks"

    # 1. jq (already used to get here, but verify for completeness)
    if command -v jq &>/dev/null; then
        print_ok "jq available"
    else
        print_err "jq is required (brew install jq / apt install jq)"
        errors=$((errors + 1))
    fi

    # 1b. packet capture prereqs (tester-side for now)
    if [[ "$DEPLOY_PACKET_CAPTURE_MODE" == "tester" || "$DEPLOY_PACKET_CAPTURE_MODE" == "both" ]]; then
        if detect_tshark >/dev/null 2>&1; then
            print_ok "tshark available"
            if [[ "$OSTYPE" == darwin* ]]; then
                print_warn "macOS packet capture also needs BPF permissions (ChmodBPF)."
                print_warn "If capture fails with /dev/bpf permission denied, enable ChmodBPF before running packet capture."
            fi
        else
            if [[ "$DEPLOY_PACKET_CAPTURE_INSTALL_REQS" == "true" ]]; then
                print_warn "packet capture requested; tshark not currently installed (installer will try to install it)"
            else
                print_warn "packet capture requested but tshark is missing"
            fi
        fi
    fi

    # 2. Collect all providers used
    local providers_used=()
    local t_prov; t_prov="$(jq -r '.tester.provider' "$cfg")"
    providers_used+=("$t_prov")
    local i
    for i in $(seq 0 $((DEPLOY_ENDPOINT_COUNT - 1))); do
        providers_used+=("${DEPLOY_EP_PROVIDERS[$i]}")
    done

    # Deduplicate
    local unique_providers
    unique_providers="$(printf '%s\n' "${providers_used[@]}" | sort -u)"

    # 3. Check each provider's prereqs
    local prov
    for prov in $unique_providers; do
        case "$prov" in
            local)
                print_ok "local provider: no external prereqs"
                ;;
            lan)
                # Test SSH for each LAN target
                if [[ "$t_prov" == "lan" ]]; then
                    if ssh -o BatchMode=yes -o ConnectTimeout=5 \
                           -o StrictHostKeyChecking=accept-new \
                           -p "$LAN_TESTER_PORT" \
                           "${LAN_TESTER_USER}@${LAN_TESTER_IP}" true &>/dev/null; then
                        print_ok "SSH to tester ${LAN_TESTER_USER}@${LAN_TESTER_IP} OK"
                    else
                        print_err "SSH to tester ${LAN_TESTER_USER}@${LAN_TESTER_IP}:${LAN_TESTER_PORT} failed"
                        errors=$((errors + 1))
                    fi
                fi
                for i in $(seq 0 $((DEPLOY_ENDPOINT_COUNT - 1))); do
                    [[ "${DEPLOY_EP_PROVIDERS[$i]}" != "lan" ]] && continue
                    local eip euser eport
                    eip="$(jq -r ".endpoints[$i].lan.ip" "$cfg")"
                    euser="$(jq -r ".endpoints[$i].lan.user // \"\"" "$cfg")"
                    eport="$(jq -r ".endpoints[$i].lan.port // 22" "$cfg")"
                    if ssh -o BatchMode=yes -o ConnectTimeout=5 \
                           -o StrictHostKeyChecking=accept-new \
                           -p "$eport" \
                           "${euser}@${eip}" true &>/dev/null; then
                        print_ok "SSH to endpoint ${euser}@${eip} OK"
                    else
                        print_err "SSH to endpoint ${euser}@${eip}:${eport} failed"
                        print_info "  Ensure: ssh-copy-id -p ${eport} ${euser}@${eip}"
                        errors=$((errors + 1))
                    fi
                done
                ;;
            azure)
                if [[ $AZURE_CLI_AVAILABLE -eq 1 ]]; then
                    print_ok "Azure CLI (az) found"
                else
                    print_err "Azure CLI (az) not found — install from https://aka.ms/InstallAzureCLIDeb"
                    errors=$((errors + 1))
                fi
                if [[ $AZURE_LOGGED_IN -eq 1 ]]; then
                    print_ok "Azure credentials found"
                else
                    print_err "Not logged into Azure (run: az login)"
                    errors=$((errors + 1))
                fi
                ;;
            aws)
                if [[ $AWS_CLI_AVAILABLE -eq 1 ]]; then
                    print_ok "AWS CLI found"
                else
                    print_err "AWS CLI not found — install from https://aws.amazon.com/cli/"
                    errors=$((errors + 1))
                fi
                # Live check: discover_system may have missed env vars or SSO token
                if [[ $AWS_LOGGED_IN -eq 0 && $AWS_CLI_AVAILABLE -eq 1 ]]; then
                    if aws sts get-caller-identity &>/dev/null 2>&1 </dev/null; then
                        AWS_LOGGED_IN=1
                    fi
                fi
                if [[ $AWS_LOGGED_IN -eq 1 ]]; then
                    print_ok "AWS credentials found"
                else
                    print_err "Not authenticated to AWS (run: aws configure)"
                    errors=$((errors + 1))
                fi
                ;;
            gcp)
                if [[ $GCP_CLI_AVAILABLE -eq 1 ]]; then
                    print_ok "gcloud CLI found"
                else
                    print_err "gcloud CLI not found — install from https://cloud.google.com/sdk/docs/install"
                    errors=$((errors + 1))
                fi
                # Live check: discover_system defers gcloud execution, so check auth now
                if [[ $GCP_CLI_AVAILABLE -eq 1 ]]; then
                    local gcp_account
                    gcp_account="$(gcloud auth list --filter='status:ACTIVE' --format='value(account)' 2>/dev/null </dev/null)"
                    if [[ -n "$gcp_account" ]]; then
                        GCP_LOGGED_IN=1
                        print_ok "GCP credentials found ($gcp_account)"
                    else
                        print_err "Not authenticated to GCP (run: gcloud auth login)"
                        errors=$((errors + 1))
                    fi
                elif [[ $GCP_LOGGED_IN -eq 1 ]]; then
                    print_ok "GCP credentials found"
                else
                    print_err "Not authenticated to GCP (run: gcloud auth login)"
                    errors=$((errors + 1))
                fi
                ;;
        esac
    done

    # 4. Install method prereqs
    if [[ $FROM_SOURCE -eq 1 ]]; then
        if [[ $RUST_EXISTS -eq 1 ]]; then
            print_ok "Rust/cargo available (source build)"
        else
            print_err "Rust/cargo required for source build (install from https://rustup.rs)"
            errors=$((errors + 1))
        fi
    fi

    # 5. Internet connectivity (for release downloads)
    if [[ $FROM_SOURCE -eq 0 ]]; then
        if curl -sf --max-time 5 "https://api.github.com" &>/dev/null; then
            print_ok "GitHub API reachable (for binary downloads)"
        else
            print_info "GitHub API unreachable — will fall back to source build if needed"
        fi
    fi

    # 6. Chrome/Chromium check if browser-dependent modes are requested
    if _deploy_needs_browser "$cfg"; then
        if [[ "$t_prov" == "local" ]]; then
            CHROME_PATH="$(detect_chrome)"
            if [[ -n "$CHROME_PATH" ]]; then
                CHROME_AVAILABLE=1
                print_ok "Chrome/Chromium found: $CHROME_PATH (required for browser/pageload modes)"
            else
                CHROME_AVAILABLE=0
                DO_CHROME_INSTALL=1
                if [[ -n "$PKG_MGR" ]]; then
                    print_info "Chrome/Chromium not found — will install via $PKG_MGR (required for browser/pageload modes)"
                else
                    print_err "Chrome/Chromium not found and no package manager detected"
                    print_err "  Install Chrome manually: https://www.google.com/chrome/"
                    errors=$((errors + 1))
                fi
            fi
        elif [[ "$t_prov" == "lan" ]]; then
            if _remote_chrome_available "$LAN_TESTER_IP" "$LAN_TESTER_USER"; then
                print_ok "Chrome/Chromium found on tester $LAN_TESTER_IP (required for browser/pageload modes)"
            else
                DO_CHROME_INSTALL=1
                print_info "Chrome/Chromium not found on tester $LAN_TESTER_IP — will install remotely"
            fi
        else
            # Cloud-provisioned tester: Chrome will be installed during deploy
            DO_CHROME_INSTALL=1
            print_info "Chrome/Chromium will be installed on $t_prov tester (required for browser/pageload modes)"
        fi
    fi

    echo ""
    if [[ $errors -gt 0 ]]; then
        print_err "Pre-flight failed: $errors issue(s) found. Fix the above and retry."
        exit 1
    fi
    print_ok "All pre-flight checks passed"
}

# Display the deploy plan from config.
_deploy_display_plan() {
    local cfg="$1"
    echo ""
    echo "${BOLD}═══════════════════════════════════════════════════════════${RESET}"
    echo "${BOLD}  Deploy Plan${RESET}"
    echo "${BOLD}═══════════════════════════════════════════════════════════${RESET}"

    # Tester
    local t_prov; t_prov="$(jq -r '.tester.provider' "$cfg")"
    printf "\n  ${BOLD}Tester:${RESET}\n"
    printf "    %-22s %s\n" "Provider:" "$t_prov"
    case "$t_prov" in
        lan)   printf "    %-22s %s\n" "Host:" "$LAN_TESTER_IP"
               printf "    %-22s %s\n" "User:" "$LAN_TESTER_USER" ;;
        azure) printf "    %-22s %s\n" "Region:" "$AZURE_REGION"
               printf "    %-22s %s\n" "VM:" "$AZURE_TESTER_VM"
               printf "    %-22s %s\n" "Size:" "$AZURE_TESTER_SIZE" ;;
        aws)   printf "    %-22s %s\n" "Region:" "$AWS_REGION"
               printf "    %-22s %s\n" "Instance:" "$AWS_TESTER_NAME"
               printf "    %-22s %s\n" "Type:" "$AWS_TESTER_INSTANCE_TYPE" ;;
        gcp)   printf "    %-22s %s\n" "Zone:" "$GCP_ZONE"
               printf "    %-22s %s\n" "Instance:" "$GCP_TESTER_NAME"
               printf "    %-22s %s\n" "Type:" "$GCP_TESTER_MACHINE_TYPE" ;;
    esac

    # Endpoints
    local i
    for i in $(seq 0 $((DEPLOY_ENDPOINT_COUNT - 1))); do
        local label="${DEPLOY_EP_LABELS[$i]}"
        local ep_prov="${DEPLOY_EP_PROVIDERS[$i]}"
        printf "\n  ${BOLD}Endpoint: %s${RESET}\n" "$label"
        printf "    %-22s %s\n" "Provider:" "$ep_prov"
        case "$ep_prov" in
            lan)
                local eip; eip="$(jq -r ".endpoints[$i].lan.ip" "$cfg")"
                local euser; euser="$(jq -r ".endpoints[$i].lan.user // \"\"" "$cfg")"
                printf "    %-22s %s\n" "Host:" "$eip"
                printf "    %-22s %s\n" "User:" "$euser" ;;
            azure)
                local er; er="$(jq -r ".endpoints[$i].azure.region // \"$AZURE_REGION\"" "$cfg")"
                local evm; evm="$(jq -r ".endpoints[$i].azure.vm_name // \"networker-endpoint-vm\"" "$cfg")"
                local esz; esz="$(jq -r ".endpoints[$i].azure.vm_size // \"Standard_B2s\"" "$cfg")"
                printf "    %-22s %s\n" "Region:" "$er"
                printf "    %-22s %s\n" "VM:" "$evm"
                printf "    %-22s %s\n" "Size:" "$esz" ;;
            aws)
                local er; er="$(jq -r ".endpoints[$i].aws.region // \"$AWS_REGION\"" "$cfg")"
                local en; en="$(jq -r ".endpoints[$i].aws.instance_name // \"networker-endpoint\"" "$cfg")"
                local et; et="$(jq -r ".endpoints[$i].aws.instance_type // \"t3.small\"" "$cfg")"
                printf "    %-22s %s\n" "Region:" "$er"
                printf "    %-22s %s\n" "Instance:" "$en"
                printf "    %-22s %s\n" "Type:" "$et" ;;
            gcp)
                local ez; ez="$(jq -r ".endpoints[$i].gcp.zone // \"$GCP_ZONE\"" "$cfg")"
                local en; en="$(jq -r ".endpoints[$i].gcp.instance_name // \"networker-endpoint\"" "$cfg")"
                local et; et="$(jq -r ".endpoints[$i].gcp.machine_type // \"e2-small\"" "$cfg")"
                printf "    %-22s %s\n" "Zone:" "$ez"
                printf "    %-22s %s\n" "Instance:" "$en"
                printf "    %-22s %s\n" "Type:" "$et" ;;
        esac
    done

    # Tests
    if [[ $DEPLOY_RUN_TESTS -eq 1 ]]; then
        printf "\n  ${BOLD}Tests:${RESET}\n"
        [[ "$DEPLOY_TEST_MODES" != "null" && -n "$DEPLOY_TEST_MODES" ]] && \
            printf "    %-22s %s\n" "Modes:" "$(echo "$DEPLOY_TEST_MODES" | jq -r 'join(", ")')"
        [[ -n "$DEPLOY_TEST_RUNS" ]] && \
            printf "    %-22s %s\n" "Runs:" "$DEPLOY_TEST_RUNS"
        [[ "$DEPLOY_TEST_PAYLOAD_SIZES" != "null" && -n "$DEPLOY_TEST_PAYLOAD_SIZES" ]] && \
            printf "    %-22s %s\n" "Payload sizes:" "$(echo "$DEPLOY_TEST_PAYLOAD_SIZES" | jq -r 'join(", ")')"
        [[ -n "$DEPLOY_TEST_HTML_REPORT" ]] && \
            printf "    %-22s %s\n" "HTML report:" "$DEPLOY_TEST_HTML_REPORT"
        [[ "$DEPLOY_PACKET_CAPTURE_MODE" != "none" ]] && \
            printf "    %-22s %s\n" "Packet capture:" "$DEPLOY_PACKET_CAPTURE_MODE"
        [[ -n "$DEPLOY_PACKET_CAPTURE_INTERFACE" ]] && \
            printf "    %-22s %s\n" "Capture iface:" "$DEPLOY_PACKET_CAPTURE_INTERFACE"
    else
        printf "\n  ${BOLD}Tests:${RESET} deploy only (run_tests: false)\n"
    fi

    echo ""
    echo "${BOLD}═══════════════════════════════════════════════════════════${RESET}"
    echo ""
}

# Generate the tester config JSON from deployed endpoint IPs + test params.
_deploy_generate_tester_config() {
    local cfg="$DEPLOY_CONFIG_PATH"

    next_step "Generate tester config from deploy results"

    # Build targets array — prefer FQDN over IP (FQDN enables IIS HTTP/3 via SNI)
    local targets_json=""
    local i
    for i in $(seq 0 $((DEPLOY_ENDPOINT_COUNT - 1))); do
        local ip="${DEPLOY_EP_IPS[$i]}"
        [[ -z "$ip" ]] && continue
        local fqdn="${DEPLOY_EP_FQDNS[$i]:-}"
        local host="${fqdn:-$ip}"
        [[ -n "$targets_json" ]] && targets_json="${targets_json}, "
        targets_json="${targets_json}\"https://${host}:8443/health\""
    done

    if [[ -z "$targets_json" ]]; then
        print_err "No endpoint IPs available — cannot generate tester config"
        return 1
    fi

    CONFIG_FILE_PATH="${PWD}/networker-cloud.json"

    # Start building JSON
    local json="{\n  \"targets\": [${targets_json}]"

    # Modes
    if [[ "$DEPLOY_TEST_MODES" != "null" && -n "$DEPLOY_TEST_MODES" ]]; then
        local modes_str; modes_str="$(echo "$DEPLOY_TEST_MODES" | jq -c '.')"
        json="${json},\n  \"modes\": ${modes_str}"
    else
        json="${json},\n  \"modes\": [\"tcp\", \"http1\", \"http2\", \"http3\", \"udp\", \"download\", \"upload\", \"pageload\", \"pageload2\", \"pageload3\"]"
    fi

    # Simple fields
    [[ -n "$DEPLOY_TEST_RUNS" ]]                  && json="${json},\n  \"runs\": ${DEPLOY_TEST_RUNS}"
    [[ -n "$DEPLOY_TEST_CONCURRENCY" ]]           && json="${json},\n  \"concurrency\": ${DEPLOY_TEST_CONCURRENCY}"
    [[ -n "$DEPLOY_TEST_TIMEOUT" ]]               && json="${json},\n  \"timeout\": ${DEPLOY_TEST_TIMEOUT}"

    # Payload sizes
    if [[ "$DEPLOY_TEST_PAYLOAD_SIZES" != "null" && -n "$DEPLOY_TEST_PAYLOAD_SIZES" ]]; then
        local ps_str; ps_str="$(echo "$DEPLOY_TEST_PAYLOAD_SIZES" | jq -c '.')"
        json="${json},\n  \"payload_sizes\": ${ps_str}"
    fi

    # Boolean / string fields
    [[ "$DEPLOY_TEST_INSECURE" == "true" ]]          && json="${json},\n  \"insecure\": true"
    [[ "$DEPLOY_TEST_CONNECTION_REUSE" == "true" ]]  && json="${json},\n  \"connection_reuse\": true"
    [[ "$DEPLOY_TEST_DNS_ENABLED" == "false" ]]      && json="${json},\n  \"dns_enabled\": false"
    [[ "$DEPLOY_TEST_IPV4_ONLY" == "true" ]]         && json="${json},\n  \"ipv4_only\": true"
    [[ "$DEPLOY_TEST_IPV6_ONLY" == "true" ]]         && json="${json},\n  \"ipv6_only\": true"
    [[ "$DEPLOY_TEST_EXCEL" == "true" ]]             && json="${json},\n  \"excel\": true"
    [[ "$DEPLOY_TEST_VERBOSE" == "true" ]]           && json="${json},\n  \"verbose\": true"

    [[ -n "$DEPLOY_TEST_UDP_PORT" ]]                 && json="${json},\n  \"udp_port\": ${DEPLOY_TEST_UDP_PORT}"
    [[ -n "$DEPLOY_TEST_UDP_THROUGHPUT_PORT" ]]      && json="${json},\n  \"udp_throughput_port\": ${DEPLOY_TEST_UDP_THROUGHPUT_PORT}"
    [[ -n "$DEPLOY_TEST_PAGE_ASSETS" ]]              && json="${json},\n  \"page_assets\": ${DEPLOY_TEST_PAGE_ASSETS}"
    [[ -n "$DEPLOY_TEST_PAGE_ASSET_SIZE" ]]          && json="${json},\n  \"page_asset_size\": \"${DEPLOY_TEST_PAGE_ASSET_SIZE}\""
    [[ -n "$DEPLOY_TEST_PAGE_PRESET" ]]              && json="${json},\n  \"page_preset\": \"${DEPLOY_TEST_PAGE_PRESET}\""
    [[ -n "$DEPLOY_TEST_RETRIES" ]]                  && json="${json},\n  \"retries\": ${DEPLOY_TEST_RETRIES}"
    [[ -n "$DEPLOY_TEST_HTML_REPORT" ]]              && json="${json},\n  \"html_report\": \"${DEPLOY_TEST_HTML_REPORT}\""
    [[ -n "$DEPLOY_TEST_OUTPUT_DIR" ]]               && json="${json},\n  \"output_dir\": \"${DEPLOY_TEST_OUTPUT_DIR}\""
    [[ -n "$DEPLOY_TEST_LOG_LEVEL" ]]                && json="${json},\n  \"log_level\": \"${DEPLOY_TEST_LOG_LEVEL}\""

    # Packet capture
    if [[ "$DEPLOY_PACKET_CAPTURE_MODE" != "none" ]]; then
        json="${json},\n  \"packet_capture\": {\n    \"mode\": \"${DEPLOY_PACKET_CAPTURE_MODE}\""
        [[ -n "$DEPLOY_PACKET_CAPTURE_INSTALL_REQS" ]] && \
            json="${json},\n    \"install_requirements\": ${DEPLOY_PACKET_CAPTURE_INSTALL_REQS}"
        [[ -n "$DEPLOY_PACKET_CAPTURE_INTERFACE" ]] && \
            json="${json},\n    \"interface\": \"${DEPLOY_PACKET_CAPTURE_INTERFACE}\""
        [[ "$DEPLOY_PACKET_CAPTURE_WRITE_PCAP" == "true" ]] && \
            json="${json},\n    \"write_pcap\": true"
        [[ "$DEPLOY_PACKET_CAPTURE_WRITE_SUMMARY_JSON" == "true" ]] && \
            json="${json},\n    \"write_summary_json\": true"
        json="${json}\n  }"
    fi

    # HTTP stacks for comparison (e.g. "nginx,iis" → ["nginx","iis"])
    if [[ -n "$DEPLOY_TEST_HTTP_STACKS" ]]; then
        local stacks_arr=""
        IFS=',' read -ra _stks <<< "$DEPLOY_TEST_HTTP_STACKS"
        for _s in "${_stks[@]}"; do
            [[ -n "$stacks_arr" ]] && stacks_arr="${stacks_arr}, "
            stacks_arr="${stacks_arr}\"${_s}\""
        done
        json="${json},\n  \"http_stacks\": [${stacks_arr}]"
    fi

    json="${json}\n}"

    printf "%b" "$json" > "$CONFIG_FILE_PATH"
    print_ok "Config written to ${CONFIG_FILE_PATH}"
    print_info "Targets: $(echo "$targets_json" | tr ',' '\n' | wc -l | tr -d ' ') endpoint(s)"

    # Upload config to remote tester if applicable
    local tester_ip="" tester_user="" tester_scp_opts=(-o StrictHostKeyChecking=accept-new -q)
    case "$TESTER_LOCATION" in
        lan)   tester_ip="$LAN_TESTER_IP"; tester_user="$LAN_TESTER_USER"
               tester_scp_opts=(-o StrictHostKeyChecking=accept-new -P "$LAN_TESTER_PORT" -q) ;;
        azure) tester_ip="$AZURE_TESTER_IP"; tester_user="azureuser" ;;
        aws)   tester_ip="$AWS_TESTER_IP";   tester_user="ubuntu" ;;
        gcp)   tester_ip="$GCP_TESTER_IP";   tester_user="$(whoami)" ;;
    esac
    if [[ -n "$tester_ip" ]]; then
        print_info "Uploading config to tester ($tester_ip)…"
        scp "${tester_scp_opts[@]}" \
            "$CONFIG_FILE_PATH" \
            "${tester_user}@${tester_ip}:~/networker-cloud.json"
        print_ok "Config uploaded to ~/networker-cloud.json on tester"
    fi
}

# Execute tests: run networker-tester locally or on the remote tester.
_deploy_execute_tests() {
    print_section "Running tests"

    if [[ "$TESTER_LOCATION" == "local" ]]; then
        # Find local binary
        local tester_bin=""
        if command -v networker-tester &>/dev/null; then
            tester_bin="networker-tester"
        elif [[ -x "${INSTALL_DIR:-/usr/local/bin}/networker-tester" ]]; then
            tester_bin="${INSTALL_DIR:-/usr/local/bin}/networker-tester"
        fi
        if [[ -z "$tester_bin" ]]; then
            print_err "networker-tester not found locally — cannot run tests"
            return 1
        fi

        # ── Version check: ensure local tester matches deployed endpoint version ──
        local installed_ver=""
        installed_ver="$("$tester_bin" --version 2>/dev/null | awk '{print $NF}')"
        local want_ver="${NETWORKER_VERSION:-}"
        want_ver="${want_ver#v}"  # strip leading 'v'
        if [[ -n "$installed_ver" && -n "$want_ver" && "$installed_ver" != "$want_ver" ]]; then
            print_warn "Version mismatch: local tester is v${installed_ver} but endpoints are v${want_ver}"
            print_info "Updating local networker-tester to v${want_ver}…"
            if step_download_release "networker-tester"; then
                # Re-locate the binary after update
                if command -v networker-tester &>/dev/null; then
                    tester_bin="networker-tester"
                elif [[ -x "${INSTALL_DIR:-/usr/local/bin}/networker-tester" ]]; then
                    tester_bin="${INSTALL_DIR:-/usr/local/bin}/networker-tester"
                fi
                local new_ver
                new_ver="$("$tester_bin" --version 2>/dev/null | awk '{print $NF}')"
                print_ok "Updated local tester: v${installed_ver} → v${new_ver}"
            else
                print_warn "Auto-update failed — running tests with v${installed_ver} (results may differ from v${want_ver})"
            fi
        elif [[ -n "$installed_ver" ]]; then
            print_ok "Local tester version: v${installed_ver}"
        fi

        print_info "Running: $tester_bin --config $CONFIG_FILE_PATH"
        echo ""
        # Use 'sg wireshark' if capture is enabled and the group exists,
        # so tshark has capture permissions even without re-login.
        if [[ "$DEPLOY_PACKET_CAPTURE_MODE" != "none" ]] && getent group wireshark &>/dev/null; then
            sg wireshark -c "\"$tester_bin\" --config \"$CONFIG_FILE_PATH\""
        else
            "$tester_bin" --config "$CONFIG_FILE_PATH"
        fi
        local rc=$?
        echo ""
        if [[ $rc -eq 0 ]]; then
            print_ok "Tests completed successfully"
        else
            print_err "Tests exited with code $rc"
        fi
        return $rc
    fi

    # Remote tester: run via SSH
    local ssh_opts=(-o StrictHostKeyChecking=accept-new)
    local tester_dest="" tester_user=""

    case "$TESTER_LOCATION" in
        lan)
            ssh_opts+=(-p "$LAN_TESTER_PORT")
            tester_user="$LAN_TESTER_USER"
            tester_dest="${LAN_TESTER_USER}@${LAN_TESTER_IP}"
            ;;
        azure)
            tester_user="azureuser"
            tester_dest="azureuser@${AZURE_TESTER_IP}"
            ;;
        aws)
            tester_user="ubuntu"
            tester_dest="ubuntu@${AWS_TESTER_IP}"
            ;;
        gcp)
            # Use gcloud compute ssh for GCP
            print_info "Running tests on GCP tester ($GCP_TESTER_NAME)…"
            gcloud compute ssh "$GCP_TESTER_NAME" \
                --project "$GCP_PROJECT" \
                --zone "$GCP_ZONE" \
                --command "networker-tester --config ~/networker-cloud.json"
            local rc=$?
            if [[ $rc -eq 0 ]]; then
                print_ok "Tests completed successfully on GCP tester"
                # Download results
                _deploy_download_results_gcp
            else
                print_err "Tests exited with code $rc"
            fi
            return $rc
            ;;
    esac

    print_info "Running tests on remote tester ($tester_dest)…"
    echo ""
    ssh "${ssh_opts[@]}" "$tester_dest" "networker-tester --config ~/networker-cloud.json"
    local rc=$?
    echo ""

    if [[ $rc -eq 0 ]]; then
        print_ok "Tests completed successfully on remote tester"
    else
        print_err "Tests exited with code $rc"
    fi

    # Download results back
    _deploy_download_results "$tester_dest" "$tester_user" "${ssh_opts[@]}"

    return $rc
}

# Download test results from remote tester via SCP.
_deploy_download_results() {
    local dest="$1" user="$2"
    shift 2
    local ssh_opts=("$@")

    local report_name="${DEPLOY_TEST_HTML_REPORT:-report.html}"
    local output_dir="${DEPLOY_TEST_OUTPUT_DIR:-.}"
    mkdir -p "$output_dir"

    print_info "Downloading results from remote tester…"

    # Try to download HTML report
    local scp_opts=(-o StrictHostKeyChecking=accept-new -q)
    # Extract port from ssh_opts if present
    local p
    for p in "${ssh_opts[@]}"; do
        if [[ "$p" == "-p" ]]; then
            continue
        fi
        if [[ "$p" =~ ^[0-9]+$ && ${#p} -le 5 ]]; then
            scp_opts+=(-P "$p")
        fi
    done

    if scp "${scp_opts[@]}" "${dest}:~/${report_name}" "${output_dir}/${report_name}" 2>/dev/null; then
        print_ok "HTML report downloaded: ${output_dir}/${report_name}"
    else
        print_info "No HTML report found on remote tester"
    fi

    # Try to download Excel report if enabled
    if [[ "$DEPLOY_TEST_EXCEL" == "true" ]]; then
        if scp "${scp_opts[@]}" "${dest}:~/report.xlsx" "${output_dir}/report.xlsx" 2>/dev/null; then
            print_ok "Excel report downloaded: ${output_dir}/report.xlsx"
        fi
    fi
}

# Download results from GCP tester via gcloud compute scp.
_deploy_download_results_gcp() {
    local report_name="${DEPLOY_TEST_HTML_REPORT:-report.html}"
    local output_dir="${DEPLOY_TEST_OUTPUT_DIR:-.}"
    mkdir -p "$output_dir"

    print_info "Downloading results from GCP tester…"
    if gcloud compute scp "${GCP_TESTER_NAME}:~/${report_name}" \
           "${output_dir}/${report_name}" \
           --project "$GCP_PROJECT" \
           --zone "$GCP_ZONE" \
           --quiet 2>/dev/null; then
        print_ok "HTML report downloaded: ${output_dir}/${report_name}"
    else
        print_info "No HTML report found on GCP tester"
    fi
}

# Display deploy completion summary.
_deploy_display_completion() {
    echo ""
    echo "${BOLD}═══════════════════════════════════════════════════════════${RESET}"
    echo "${BOLD}  Deploy Complete${RESET}"
    echo "${BOLD}═══════════════════════════════════════════════════════════${RESET}"
    echo ""

    # Tester info
    printf "  ${BOLD}Tester:${RESET} %s" "$TESTER_LOCATION"
    case "$TESTER_LOCATION" in
        lan)   printf " (%s@%s)" "$LAN_TESTER_USER" "$LAN_TESTER_IP" ;;
        azure) printf " (%s)" "${AZURE_TESTER_IP:-pending}" ;;
        aws)   printf " (%s)" "${AWS_TESTER_IP:-pending}" ;;
        gcp)   printf " (%s)" "${GCP_TESTER_IP:-pending}" ;;
    esac
    echo ""

    # Endpoint info
    local i
    for i in $(seq 0 $((DEPLOY_ENDPOINT_COUNT - 1))); do
        local label="${DEPLOY_EP_LABELS[$i]}"
        local prov="${DEPLOY_EP_PROVIDERS[$i]}"
        local ip="${DEPLOY_EP_IPS[$i]}"
        printf "  ${BOLD}Endpoint %s:${RESET} %s (%s)\n" "$label" "$prov" "${ip:-local}"
    done

    # Results
    if [[ $DEPLOY_RUN_TESTS -eq 1 ]]; then
        local report_name="${DEPLOY_TEST_HTML_REPORT:-report.html}"
        local output_dir="${DEPLOY_TEST_OUTPUT_DIR:-.}"
        if [[ -f "${output_dir}/${report_name}" ]]; then
            echo ""
            printf "  ${BOLD}Report:${RESET} %s/%s\n" "$output_dir" "$report_name"
        fi
    fi

    echo ""
    echo "${BOLD}═══════════════════════════════════════════════════════════${RESET}"
}

# Main orchestration for deploy-config mode.
deploy_from_config() {
    local cfg="$1"

    if ! command -v jq &>/dev/null; then
        print_err "jq is required for --deploy mode (brew install jq / apt install jq)"
        exit 1
    fi

    if [[ ! -f "$cfg" ]]; then
        print_err "Deploy config not found: $cfg"
        exit 1
    fi

    # Phase 1: Validate config structure
    print_section "Validating deploy config"
    _deploy_validate_config "$cfg"
    if [[ ${DEPLOY_VALIDATE_ERRORS:-0} -gt 0 ]]; then
        exit 1
    fi
    print_ok "Config is valid"

    # Phase 2: Parse config into shell variables
    _deploy_parse_config "$cfg"

    # Phase 3: Pre-flight checks (tools, credentials, connectivity)
    _deploy_preflight "$cfg"

    # Phase 4: Display plan
    _deploy_display_plan "$cfg"

    # Phase 5: Deploy tester
    if [[ "$TESTER_LOCATION" == "local" ]]; then
        print_section "Local tester setup"
        # Install Chrome if needed for browser/pageload modes
        if [[ $DO_CHROME_INSTALL -eq 1 && $CHROME_AVAILABLE -eq 0 ]]; then
            step_install_chrome
        fi
        if [[ "$DEPLOY_PACKET_CAPTURE_MODE" == "tester" || "$DEPLOY_PACKET_CAPTURE_MODE" == "both" ]]; then
            if ! detect_tshark >/dev/null 2>&1; then
                step_install_packet_capture_tools
            fi
        fi
        if [[ "$INSTALL_METHOD" == "release" ]]; then
            mkdir -p "$INSTALL_DIR"
            if ! step_download_release "networker-tester"; then
                print_info "Falling back to source compile…"
                step_ensure_cargo_env
                step_cargo_install "networker-tester"
            fi
        else
            step_ensure_cargo_env
            step_cargo_install "networker-tester"
        fi
    else
        case "$TESTER_LOCATION" in
            lan)   step_lan_deploy_tester   ;;
            azure) step_azure_deploy_tester ;;
            aws)   step_aws_deploy_tester   ;;
            gcp)   step_gcp_deploy_tester   ;;
        esac
        # Install Chrome on remote tester if needed
        if [[ $DO_CHROME_INSTALL -eq 1 ]]; then
            _deploy_install_chrome_remote
        fi
    fi

    # Phase 6: Deploy endpoint(s)
    local i
    for i in $(seq 0 $((DEPLOY_ENDPOINT_COUNT - 1))); do
        local label="${DEPLOY_EP_LABELS[$i]}"
        print_section "Deploy endpoint: $label"

        _deploy_load_endpoint "$i"

        case "${DEPLOY_EP_PROVIDERS[$i]}" in
            local)
                # Local endpoint: install binary + service
                if [[ "$INSTALL_METHOD" == "release" ]]; then
                    mkdir -p "$INSTALL_DIR"
                    if ! step_download_release "networker-endpoint"; then
                        step_ensure_cargo_env
                        step_cargo_install "networker-endpoint"
                    fi
                else
                    step_ensure_cargo_env
                    step_cargo_install "networker-endpoint"
                fi
                if [[ "$SYS_OS" == "Linux" ]] && command -v systemctl &>/dev/null; then
                    step_setup_endpoint_service
                fi
                # Set up http_stacks requested for this local endpoint
                local ep_stacks="${DEPLOY_EP_HTTP_STACKS[$i]}"
                if [[ -n "$ep_stacks" ]]; then
                    IFS=',' read -ra _local_stacks <<< "$ep_stacks"
                    for _ls in "${_local_stacks[@]}"; do
                        case "$_ls" in
                            nginx)
                                if [[ "$SYS_OS" == "Linux" ]]; then
                                    step_setup_nginx
                                else
                                    print_warn "Skipping nginx setup: only supported on Linux (detected $SYS_OS)"
                                fi
                                ;;
                            iis)
                                print_warn "Skipping IIS setup: IIS is only available on Windows"
                                ;;
                        esac
                    done
                fi
                DEPLOY_EP_IPS[$i]="127.0.0.1"
                ;;
            lan)
                local os; os="$LAN_ENDPOINT_OS"
                # Auto-detect OS if not already set
                if [[ -z "$os" ]]; then
                    _lan_detect_os "endpoint"
                    os="$LAN_ENDPOINT_OS"
                fi
                if [[ "$os" == "windows" ]]; then
                    _lan_install_binary_windows "networker-endpoint" "endpoint"
                    _lan_create_endpoint_service_windows "endpoint"
                else
                    _lan_install_binary_linux "networker-endpoint" "endpoint"
                    _lan_create_endpoint_service "endpoint"
                fi
                # Set up http_stacks requested for this LAN endpoint
                local ep_stacks="${DEPLOY_EP_HTTP_STACKS[$i]}"
                if [[ -n "$ep_stacks" ]]; then
                    local ssh_user="${LAN_ENDPOINT_USER:-$(whoami)}"
                    IFS=',' read -ra _lan_stacks <<< "$ep_stacks"
                    for _ls in "${_lan_stacks[@]}"; do
                        case "$_ls" in
                            nginx)
                                if [[ "$os" != "windows" ]]; then
                                    next_step "Set up nginx on LAN endpoint ($label)"
                                    _remote_setup_nginx "$LAN_ENDPOINT_IP" "$ssh_user"
                                else
                                    print_warn "Skipping nginx setup on $label: only supported on Linux"
                                fi
                                ;;
                            iis)
                                if [[ "$os" == "windows" ]]; then
                                    print_warn "IIS setup on LAN Windows endpoints is not yet supported"
                                else
                                    print_warn "Skipping IIS setup on $label: only available on Windows"
                                fi
                                ;;
                        esac
                    done
                fi
                DEPLOY_EP_IPS[$i]="$LAN_ENDPOINT_IP"
                ;;
            azure)
                step_check_azure_prereqs
                step_azure_deploy_endpoint
                DEPLOY_EP_IPS[$i]="$AZURE_ENDPOINT_IP"
                DEPLOY_EP_FQDNS[$i]="${AZURE_ENDPOINT_FQDN:-}"
                # Set up http_stacks requested for this Azure endpoint
                local ep_stacks="${DEPLOY_EP_HTTP_STACKS[$i]}"
                if [[ -n "$ep_stacks" ]]; then
                    IFS=',' read -ra _az_stacks <<< "$ep_stacks"
                    for _ls in "${_az_stacks[@]}"; do
                        case "$_ls" in
                            nginx)
                                if [[ "$AZURE_ENDPOINT_OS" != "windows" ]]; then
                                    _remote_setup_nginx "$AZURE_ENDPOINT_IP" "azureuser"
                                else
                                    print_warn "Skipping nginx setup on Azure Windows"
                                fi
                                ;;
                            iis)
                                if [[ "$AZURE_ENDPOINT_OS" == "windows" ]]; then
                                    print_info "IIS is set up during Windows endpoint deploy"
                                else
                                    print_warn "Skipping IIS setup on Azure Linux"
                                fi
                                ;;
                        esac
                    done
                fi
                ;;
            aws)
                step_aws_deploy_endpoint
                DEPLOY_EP_IPS[$i]="$AWS_ENDPOINT_IP"
                DEPLOY_EP_FQDNS[$i]="${AWS_ENDPOINT_FQDN:-}"
                # Set up http_stacks requested for this AWS endpoint
                local ep_stacks="${DEPLOY_EP_HTTP_STACKS[$i]}"
                if [[ -n "$ep_stacks" ]]; then
                    IFS=',' read -ra _aws_stacks <<< "$ep_stacks"
                    for _ls in "${_aws_stacks[@]}"; do
                        case "$_ls" in
                            nginx)
                                if [[ "${AWS_ENDPOINT_OS:-linux}" != "windows" ]]; then
                                    _remote_setup_nginx "$AWS_ENDPOINT_IP" "ubuntu"
                                else
                                    print_warn "Skipping nginx setup on AWS Windows"
                                fi
                                ;;
                            iis)
                                if [[ "${AWS_ENDPOINT_OS:-linux}" == "windows" ]]; then
                                    print_info "IIS is set up during Windows endpoint deploy"
                                else
                                    print_warn "Skipping IIS setup on AWS Linux"
                                fi
                                ;;
                        esac
                    done
                fi
                ;;
            gcp)
                step_gcp_deploy_endpoint
                DEPLOY_EP_IPS[$i]="$GCP_ENDPOINT_IP"
                DEPLOY_EP_FQDNS[$i]="${GCP_ENDPOINT_FQDN:-}"
                # Set up http_stacks requested for this GCP endpoint
                local ep_stacks="${DEPLOY_EP_HTTP_STACKS[$i]}"
                if [[ -n "$ep_stacks" ]]; then
                    IFS=',' read -ra _gcp_stacks <<< "$ep_stacks"
                    for _ls in "${_gcp_stacks[@]}"; do
                        case "$_ls" in
                            nginx)
                                if [[ "$GCP_ENDPOINT_OS" != "windows" ]]; then
                                    _gcp_setup_nginx "$GCP_ENDPOINT_NAME" "$GCP_ENDPOINT_IP"
                                else
                                    print_warn "Skipping nginx setup on GCP Windows"
                                fi
                                ;;
                            iis)
                                if [[ "$GCP_ENDPOINT_OS" == "windows" ]]; then
                                    print_info "IIS is set up during Windows endpoint deploy"
                                else
                                    print_warn "Skipping IIS setup on GCP Linux"
                                fi
                                ;;
                        esac
                    done
                fi
                ;;
        esac

        print_ok "Endpoint $label deployed: ${DEPLOY_EP_IPS[$i]}"
    done

    # Phase 7: Generate tester config from deployed IPs (skip if deploy-only)
    if [[ $DEPLOY_RUN_TESTS -eq 1 ]]; then
        _deploy_generate_tester_config
    fi

    # Phase 7.5: Dashboard (if requested in config)
    local has_dashboard
    has_dashboard="$(jq 'has("dashboard")' "$cfg" 2>/dev/null)"
    if [[ "$has_dashboard" == "true" ]]; then
        print_section "Dashboard setup"
        DO_INSTALL_DASHBOARD=1

        # Read dashboard-specific config
        DASHBOARD_ADMIN_PASSWORD="$(jq -r '.dashboard.admin_password // "admin"' "$cfg")"
        DASHBOARD_PORT="$(jq -r '.dashboard.port // 3000' "$cfg")"
        export DASHBOARD_ADMIN_PASSWORD DASHBOARD_PORT

        step_install_postgresql
        step_install_nodejs
        step_install_cloud_clis

        if [[ "$INSTALL_METHOD" == "release" ]]; then
            mkdir -p "$INSTALL_DIR"
            step_download_release "networker-dashboard" || {
                step_ensure_cargo_env
                step_cargo_install "networker-dashboard"
            }
            step_download_release "networker-agent" || {
                step_ensure_cargo_env
                step_cargo_install "networker-agent"
            }
        else
            step_ensure_cargo_env
            step_cargo_install "networker-dashboard"
            step_cargo_install "networker-agent"
        fi

        # Dashboard also needs tester + endpoint binaries, browser, and capture tools
        if [[ "$INSTALL_METHOD" == "release" ]]; then
            step_download_release "networker-tester" 2>/dev/null || true
            step_download_release "networker-endpoint" 2>/dev/null || true
        fi
        if [[ "$SYS_OS" == "Linux" ]]; then
            step_install_chrome 2>/dev/null || print_warn "Chrome install skipped — browser probes disabled"
            step_install_tshark 2>/dev/null || print_warn "tshark install skipped — packet capture disabled"
            # Set up local endpoint service so tests can target localhost
            if command -v systemctl &>/dev/null; then
                step_setup_endpoint_service 2>/dev/null || true
            fi
        fi

        step_build_frontend
        step_write_dashboard_env
        step_setup_dashboard_service

        # Nginx proxy + TLS (if fqdn is in config)
        DASHBOARD_FQDN="$(jq -r '.dashboard.fqdn // ""' "$cfg")"
        if [[ "$SYS_OS" == "Linux" ]]; then
            step_setup_nginx_proxy
            if [[ -n "$DASHBOARD_FQDN" ]]; then
                step_setup_letsencrypt
            fi
        fi
    fi

    # Phase 8: Run tests
    if [[ $DEPLOY_RUN_TESTS -eq 1 ]]; then
        _deploy_execute_tests
    fi

    # Phase 9: Summary
    _deploy_display_completion
}

# ── Entry point ───────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    discover_system

    # Config-driven deploy mode: skip all interactive flow
    if [[ -n "$DEPLOY_CONFIG_PATH" ]]; then
        print_banner
        deploy_from_config "$DEPLOY_CONFIG_PATH"
        return $?
    fi

    print_banner
    display_system_info

    # Check for updates to installed binaries
    check_for_updates
    prompt_component_selection  # ask tester / endpoint / both (skipped if set via CLI)
    display_plan
    prompt_main

    # ── Local installs ────────────────────────────────────────────────────────
    local do_local_tester=0 do_local_endpoint=0
    [[ $DO_INSTALL_TESTER -eq 1 && $DO_REMOTE_TESTER -eq 0 ]]     && do_local_tester=1
    [[ $DO_INSTALL_ENDPOINT -eq 1 && $DO_REMOTE_ENDPOINT -eq 0 ]]  && do_local_endpoint=1

    if [[ "$INSTALL_METHOD" == "release" ]]; then
        mkdir -p "$INSTALL_DIR"
        local need_source_fallback=0
        if [[ $do_local_tester -eq 1 ]]; then
            if ! step_download_release "networker-tester"; then
                need_source_fallback=1
            fi
        fi
        if [[ $do_local_endpoint -eq 1 ]]; then
            if ! step_download_release "networker-endpoint"; then
                need_source_fallback=1
            fi
        fi
        # If any download failed, fall back to source compile for the failed ones
        if [[ $need_source_fallback -eq 1 ]]; then
            RELEASE_DOWNLOAD_FAILED=1
            print_info "Falling back to source compile for failed downloads…"
            if [[ $DO_RUST_INSTALL -eq 1 ]]; then step_install_rust; fi
            step_ensure_cargo_env
            if [[ $do_local_tester -eq 1 ]] && ! _binary_version_ok "networker-tester"; then
                step_cargo_install "networker-tester"
            fi
            if [[ $do_local_endpoint -eq 1 ]] && ! _binary_version_ok "networker-endpoint"; then
                step_cargo_install "networker-endpoint"
            fi
        fi
    else
        if [[ $DO_CHROME_INSTALL -eq 1 ]]; then
            step_install_chrome
        fi
        if [[ $DO_GIT_INSTALL -eq 1 ]]; then
            step_install_git
        fi
        if [[ $DO_RUST_INSTALL -eq 1 ]]; then
            step_install_rust
        fi
        if [[ $do_local_tester -eq 1 || $do_local_endpoint -eq 1 ]]; then
            step_ensure_cargo_env
        fi
        if [[ $do_local_tester -eq 1 ]]; then
            step_cargo_install "networker-tester"
        fi
        if [[ $do_local_endpoint -eq 1 ]]; then
            step_cargo_install "networker-endpoint"
        fi
    fi

    if [[ $do_local_tester -eq 1 && ( $CHROME_AVAILABLE -eq 1 || $DO_CHROME_INSTALL -eq 1 ) ]]; then
        step_ensure_certutil
    fi

    # ── Packet capture tools (tshark) for tester ─────────────────────────────
    if [[ $do_local_tester -eq 1 && -n "$PKG_MGR" ]]; then
        if ! detect_tshark >/dev/null 2>&1; then
            echo ""
            if [[ $AUTO_YES -eq 1 ]] || ask_yn "Install packet capture tools (tshark) for network analysis?" "y"; then
                step_install_packet_capture_tools
            fi
        fi
    fi

    # ── Local endpoint: offer systemd service (Linux only) ────────────────────
    if [[ $do_local_endpoint -eq 1 && "$SYS_OS" == "Linux" ]] && command -v systemctl &>/dev/null; then
        echo ""
        if ask_yn "Set up networker-endpoint as a systemd service (auto-starts on boot)?" "y"; then
            step_setup_endpoint_service
            step_setup_nginx
        fi
    fi

    # ── Dashboard install ────────────────────────────────────────────────────
    if [[ $DO_INSTALL_DASHBOARD -eq 1 ]]; then
        # Ensure swap exists — dashboard install pulls ~4GB of packages
        # (cloud CLIs, Chrome, Node.js, frontend build) which can OOM small VMs.
        if [[ "$SYS_OS" == "Linux" ]] && ! swapon --show 2>/dev/null | grep -q .; then
            print_info "Creating 4GB swap file to prevent OOM during install…"
            sudo fallocate -l 4G /swapfile 2>/dev/null \
                && sudo chmod 600 /swapfile \
                && sudo mkswap /swapfile >/dev/null \
                && sudo swapon /swapfile \
                && echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab >/dev/null \
                && print_ok "Swap enabled (4GB)" \
                || print_warn "Swap setup failed — continuing without swap"
        fi

        step_install_postgresql
        step_install_nodejs
        step_install_cloud_clis

        if [[ "$INSTALL_METHOD" == "release" ]]; then
            mkdir -p "$INSTALL_DIR"
            local dashboard_dl_ok=1
            if ! step_download_release "networker-dashboard"; then
                dashboard_dl_ok=0
            fi
            if ! step_download_release "networker-agent"; then
                dashboard_dl_ok=0
            fi
            if [[ $dashboard_dl_ok -eq 0 ]]; then
                print_info "Falling back to source compile for dashboard…"
                step_ensure_cargo_env
                step_cargo_install "networker-dashboard"
                step_cargo_install "networker-agent"
            fi
        else
            step_ensure_cargo_env
            step_cargo_install "networker-dashboard"
            step_cargo_install "networker-agent"
        fi

        # Dashboard also needs tester + endpoint binaries, browser, and capture tools
        if [[ "$INSTALL_METHOD" == "release" ]]; then
            step_download_release "networker-tester" 2>/dev/null || true
            step_download_release "networker-endpoint" 2>/dev/null || true
        fi
        if [[ "$SYS_OS" == "Linux" ]]; then
            step_install_chrome 2>/dev/null || print_warn "Chrome install skipped — browser probes disabled"
            step_install_tshark 2>/dev/null || print_warn "tshark install skipped — packet capture disabled"
            # Set up local endpoint service so tests can target localhost
            if command -v systemctl &>/dev/null; then
                step_setup_endpoint_service 2>/dev/null || true
            fi
        fi

        step_build_frontend
        step_write_dashboard_env
        step_setup_dashboard_service

        # Nginx reverse proxy (optional)
        if [[ "$SYS_OS" == "Linux" ]]; then
            step_setup_nginx_proxy
            if [[ $DASHBOARD_NGINX_CONFIGURED -eq 1 ]]; then
                if [[ -z "${DASHBOARD_FQDN:-}" ]]; then
                    echo ""
                    printf "  Enter FQDN for HTTPS certificate (or press Enter for self-signed): "
                    read -r DASHBOARD_FQDN </dev/tty 2>/dev/null || DASHBOARD_FQDN=""
                fi
                step_setup_letsencrypt
            fi
        fi
    fi

    # ── Remote: tester ────────────────────────────────────────────────────────
    if [[ $DO_REMOTE_TESTER -eq 1 ]]; then
        case "$TESTER_LOCATION" in
            lan)   step_lan_deploy_tester   ;;
            azure) step_azure_deploy_tester ;;
            aws)   step_aws_deploy_tester   ;;
            gcp)   step_gcp_deploy_tester   ;;
        esac
    fi

    # ── Remote: endpoint ──────────────────────────────────────────────────────
    if [[ $DO_REMOTE_ENDPOINT -eq 1 ]]; then
        case "$ENDPOINT_LOCATION" in
            lan)   step_lan_deploy_endpoint   ;;
            azure) step_azure_deploy_endpoint ;;
            aws)   step_aws_deploy_endpoint   ;;
            gcp)   step_gcp_deploy_endpoint   ;;
        esac
    fi

    display_completion
}

# Run main only when the script is executed directly (not sourced for testing).
if [[ "${BASH_SOURCE[0]:-$0}" == "${0}" ]]; then main "$@"; fi
