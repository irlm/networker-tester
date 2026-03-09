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
Usage: install.sh [OPTIONS] [tester|endpoint|both]

  tester    Install networker-tester (the diagnostic CLI client)
  endpoint  Install networker-endpoint (the target test server)
  both      Install both binaries  [default]

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
  bash install.sh --deploy deploy.json             # config-driven deploy + test
EOF
}

# ── Script-level state ────────────────────────────────────────────────────────
COMPONENT=""   # "" = not set via CLI; "tester" | "endpoint" | "both" = explicit
AUTO_YES=0
FROM_SOURCE=0
SKIP_RUST=0
SKIP_SERVICE=0

INSTALL_METHOD="source"   # "release" | "source"
RELEASE_AVAILABLE=0
RELEASE_TARGET=""
NETWORKER_VERSION=""      # populated in discover_system (gh query or fallback below)
INSTALLER_VERSION="v0.13.1"  # fallback when gh is unavailable

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

# Arrays for multi-endpoint support (parallel arrays indexed 0..N-1)
DEPLOY_EP_PROVIDERS=()
DEPLOY_EP_LABELS=()
DEPLOY_EP_IPS=()               # populated after deploy (result IPs)

# ── Argument parsing ──────────────────────────────────────────────────────────
parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            tester|endpoint|both)
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
        tester)   DO_INSTALL_ENDPOINT=0 ;;
        endpoint) DO_INSTALL_TESTER=0   ;;
        both)     ;;
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
    ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 "${user}@${ip}" \
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
    _LAN_SSH_OPTS=(-o StrictHostKeyChecking=no -o ConnectTimeout=10 -p "$port")
    _LAN_SCP_OPTS=(-o StrictHostKeyChecking=no -P "$port" -q)
    _LAN_DEST="${user}@${ip}"
}

# Test SSH connectivity to a LAN host.  Exits with helpful message on failure.
_lan_test_ssh() {
    local ip="$1" user="$2" port="$3"

    print_info "Testing SSH connection to ${user}@${ip}:${port}…"
    if ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 -o BatchMode=yes \
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
    local ip_var="LAN_${role^^}_IP"
    local user_var="LAN_${role^^}_USER"
    local port_var="LAN_${role^^}_PORT"

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

    print_info "Creating networker-endpoint Windows service on ${_LAN_DEST}…"
    ssh "${_LAN_SSH_OPTS[@]}" "${_LAN_DEST}" "powershell -ExecutionPolicy Bypass -Command \"& {
        # Create service if not already registered
        if (-not (Get-Service networker-endpoint -ErrorAction SilentlyContinue)) {
            sc.exe create networker-endpoint binPath= 'C:\\networker\\networker-endpoint.exe' start= auto
        }
        sc.exe start networker-endpoint 2>\$null

        # Firewall rules
        New-NetFirewallRule -Name 'NetworkerEndpoint-TCP' -DisplayName 'Networker Endpoint TCP' \
            -Enabled True -Direction Inbound -Protocol TCP -Action Allow \
            -LocalPort 8080,8443 -ErrorAction SilentlyContinue
        New-NetFirewallRule -Name 'NetworkerEndpoint-UDP' -DisplayName 'Networker Endpoint UDP' \
            -Enabled True -Direction Inbound -Protocol UDP -Action Allow \
            -LocalPort 8443,9998,9999 -ErrorAction SilentlyContinue
    }\""
    print_ok "networker-endpoint Windows service created and started"
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
                            apt-get) sudo apt-get install -y unzip 2>&1 | tail -1 || true ;;
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
    echo ""
    local comp_ans
    printf "  Choice [1]: "
    read -r comp_ans </dev/tty || true
    comp_ans="${comp_ans:-1}"
    case "$comp_ans" in
        2) COMPONENT="tester";   DO_INSTALL_TESTER=1; DO_INSTALL_ENDPOINT=0 ;;
        3) COMPONENT="endpoint"; DO_INSTALL_TESTER=0; DO_INSTALL_ENDPOINT=1 ;;
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

    # Verify the binary actually runs (catches GLIBC mismatch, wrong arch, etc.)
    local installed_ver
    if installed_ver="$("${INSTALL_DIR}/${binary}" --version 2>&1)"; then
        print_ok "$binary installed → ${INSTALL_DIR}/${binary}  ($installed_ver)"
    else
        print_warn "Downloaded binary failed to run: ${installed_ver}"
        rm -f "${INSTALL_DIR}/${binary}"
        return 1
    fi
}

# ── Source-mode steps ─────────────────────────────────────────────────────────
step_install_git() {
    next_step "Install git"
    print_info "Installing git via ${PKG_MGR}…"
    echo ""

    case "$PKG_MGR" in
        brew)    brew install git ;;
        apt-get) sudo apt-get update -qq && sudo apt-get install -y git ;;
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
            sudo apt-get update -qq \
                && (sudo apt-get install -y chromium-browser 2>/dev/null \
                    || sudo apt-get install -y chromium)
            sudo apt-get install -y libnss3-tools 2>/dev/null || true
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
        apt-get) sudo apt-get install -y libnss3-tools 2>/dev/null || true ;;
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
                # Verify the binary actually runs (catches GLIBC mismatch, wrong arch, etc.)
                local installed_ver
                installed_ver="$("${INSTALL_DIR}/${binary}" --version 2>&1)" && {
                    print_ok "$binary installed → ${INSTALL_DIR}/${binary}  ($installed_ver)"
                    return 0
                }
                print_warn "Downloaded binary failed to run: ${installed_ver}"
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
                    apt-get) sudo apt-get update -qq 2>/dev/null; sudo apt-get install -y build-essential 2>&1 | tail -3 || true ;;
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
                -o StrictHostKeyChecking=no \
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
        scp -o StrictHostKeyChecking=no -q "$script_path" "${user}@${ip}:/tmp/networker-install.sh"
    else
        # Running as curl|bash — no local file. Download locally first, then SCP.
        print_info "Downloading installer locally, then uploading to VM…"
        local tmp_installer="/tmp/networker-install-$$.sh"
        if curl -fsSL "${gist_url}" -o "$tmp_installer" 2>/dev/null || \
           curl -fsSL "${repo_url}" -o "$tmp_installer" 2>/dev/null; then
            scp -o StrictHostKeyChecking=no -q "$tmp_installer" "${user}@${ip}:/tmp/networker-install.sh"
            rm -f "$tmp_installer"
        else
            # Local download also failed — try on VM directly as last resort
            rm -f "$tmp_installer"
            print_warn "Local download failed — trying directly on VM…"
            ssh -o StrictHostKeyChecking=no "${user}@${ip}" \
                "curl -fsSLk '${repo_url}' -o /tmp/networker-install.sh"
        fi
    fi

    # Remove OUTPUT iptables REDIRECT rules from prior installs — they break all outbound HTTPS
    # by redirecting the VM's own port 80/443 traffic to the local endpoint.
    ssh -o StrictHostKeyChecking=no "${user}@${ip}" \
        "sudo iptables -t nat -D OUTPUT -p tcp --dport 80  -j REDIRECT --to-port 8080 2>/dev/null; \
         sudo iptables -t nat -D OUTPUT -p tcp --dport 443 -j REDIRECT --to-port 8443 2>/dev/null; \
         true" < /dev/null 2>/dev/null

    print_info "Running installer on VM (the terminal will show the VM's install progress)…"
    echo ""
    # -t allocates a pseudo-TTY so the VM's spinner + colors work
    ssh -t -o StrictHostKeyChecking=no "${user}@${ip}" \
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
    fi

    # Detect remote architecture (needed for both download path and source fallback)
    local remote_arch
    remote_arch="$(ssh -o StrictHostKeyChecking=no "${user}@${ip}" "uname -m" 2>/dev/null || echo "x86_64")"

    # Check whether release has pre-built assets; compile locally if not.
    local has_assets=""
    if [[ -n "$ver" ]]; then
        has_assets="$(gh release view --repo "$REPO_GH" "$ver" --json assets \
                      -q '[.assets[].name] | join(" ")' 2>/dev/null || echo "")"
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
        scp -o StrictHostKeyChecking=no -q \
            "${tmp_dir}/${binary}" \
            "${user}@${ip}:/tmp/${binary}"
        rm -rf "${tmp_dir}"

        ssh -o StrictHostKeyChecking=no "${user}@${ip}" \
            "sudo mv /tmp/${binary} /usr/local/bin/${binary} && \
             sudo chmod +x /usr/local/bin/${binary}"
    else
        rm -rf "${tmp_dir}"
        # Fallback: download directly on the remote VM
        print_info "Downloading directly on VM (${ver})…"
        if ! ssh -o StrictHostKeyChecking=no "${user}@${ip}" \
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
    remote_ver="$(ssh -o StrictHostKeyChecking=no "${user}@${ip}" \
        "/usr/local/bin/${binary} --version 2>/dev/null" || echo "unknown")"
    print_ok "$binary installed on VM  ($remote_ver)"
}

# Create a systemd service for networker-endpoint on a remote host.
# $1 = public IP, $2 = SSH user
_remote_create_endpoint_service() {
    local ip="$1" user="$2"

    ssh -o StrictHostKeyChecking=no "${user}@${ip}" bash <<'REMOTE'
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

    # Redirect privileged ports 80/443 → 8080/8443 so browsers can reach the server
    if command -v iptables &>/dev/null; then
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
            ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 "${ssh_user}@${ip}" \
                "sudo systemctl status networker-endpoint --no-pager -l 2>&1 | head -30" 2>/dev/null || true
            echo ""
            print_info "Last 30 log lines:"
            ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 "${ssh_user}@${ip}" \
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
    print_info "Building ${binary} from source on Windows VM (this may take several minutes)…"

    local ps_tmp
    ps_tmp="$(mktemp /tmp/networker-ps-XXXXX.ps1)"
    cat > "$ps_tmp" <<PSEOF
\$ErrorActionPreference = 'Stop'
\$dest = 'C:\\networker'

# Install Rust if not present
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Installing Rust…"
    Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile C:\\rustup-init.exe -UseBasicParsing
    & C:\\rustup-init.exe -y --default-toolchain stable 2>&1 | Select-Object -Last 3
    \$env:Path = [System.Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [System.Environment]::GetEnvironmentVariable('Path','User')
}

Write-Host "Building ${binary} from source…"
cargo install --git ${REPO_HTTPS} ${binary} 2>&1 | Select-Object -Last 5

New-Item -ItemType Directory -Force -Path \$dest | Out-Null
Copy-Item "\$env:USERPROFILE\\.cargo\\bin\\${binary}.exe" "\$dest\\${binary}.exe" -Force

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
# Create service (ignore error if already exists)
sc.exe create networker-endpoint binPath=$exe start=auto | Out-Host
sc.exe description networker-endpoint 'Networker Endpoint diagnostics server' | Out-Host
sc.exe start networker-endpoint | Out-Host
# Windows Firewall rules
netsh advfirewall firewall add rule name='Networker-HTTP'  protocol=TCP dir=in action=allow localport=8080  | Out-Null
netsh advfirewall firewall add rule name='Networker-HTTPS' protocol=TCP dir=in action=allow localport=8443  | Out-Null
netsh advfirewall firewall add rule name='Networker-UDP'   protocol=UDP dir=in action=allow localport='8443,9998,9999' | Out-Null
sc.exe query networker-endpoint | Select-String 'STATE'
Write-Host 'Firewall rules added'
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
    local tester_ip="" tester_user="" tester_scp_opts=(-o StrictHostKeyChecking=no -q)
    case "$TESTER_LOCATION" in
        lan)   tester_ip="$LAN_TESTER_IP"; tester_user="$LAN_TESTER_USER"
               tester_scp_opts=(-o StrictHostKeyChecking=no -P "$LAN_TESTER_PORT" -q) ;;
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
                print_ok "Reusing VM '$vm' — Public IP: ${BOLD}${ip}${RESET}"
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

    printf -v "$ip_var" "%s" "$ip"
    print_ok "VM created ($os_label) — Public IP: ${BOLD}${ip}${RESET}"

    if [[ "$os_type" == "windows" && -n "${win_pass:-}" ]]; then
        # Store credentials for later display in the summary
        local pass_var="AZURE_${label^^}_WIN_PASS"
        printf -v "$pass_var" "%s" "$win_pass"
        echo ""
        print_info "Windows credentials:"
        echo "    User:     azureuser"
        echo "    Password: ${win_pass}"
        echo "    RDP:      mstsc /v:${ip}"
        echo ""
    fi
}

# Open TCP 80/443/8080/8443 and UDP 8443/9998/9999 on the NSG for the endpoint VM.
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
        print_warn "    --priority 1100 --destination-port-ranges 80 443 8080 8443 --access Allow"
        print_warn "  az network nsg rule create --resource-group $rg --nsg-name <nsg> \\"
        print_warn "    --name Networker-UDP --protocol Udp --direction Inbound \\"
        print_warn "    --priority 1110 --destination-port-ranges 8443 9998 9999 --access Allow"
        return 0
    fi

    print_info "Opening TCP 80, 443, 8080, 8443…"
    az network nsg rule create \
        --resource-group "$rg" \
        --nsg-name "$nsg_name" \
        --name "Networker-TCP" \
        --protocol Tcp \
        --direction Inbound \
        --priority 1100 \
        --destination-port-ranges 80 443 8080 8443 \
        --access Allow \
        --output none
    print_ok "TCP 80, 443, 8080, 8443 open"

    print_info "Opening UDP 8443, 9998, 9999…"
    az network nsg rule create \
        --resource-group "$rg" \
        --nsg-name "$nsg_name" \
        --name "Networker-UDP" \
        --protocol Udp \
        --direction Inbound \
        --priority 1110 \
        --destination-port-ranges 8443 9998 9999 \
        --access Allow \
        --output none
    print_ok "UDP 8443, 9998, 9999 open"
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
    if ssh -o StrictHostKeyChecking=no -o ConnectTimeout=10 "${user}@${ip}" \
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
        if ssh -o StrictHostKeyChecking=no "${user}@${ip}" "$install_cmd" 2>/dev/null; then
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

    step_azure_open_endpoint_ports "$rg" "$vm"

    next_step "Install networker-endpoint on Azure VM ($label)"
    if [[ "$os_type" == "windows" ]]; then
        _azure_wait_for_windows_vm "$rg" "$vm" "$label"
        _azure_win_install_binary "networker-endpoint" "$rg" "$vm" "${NETWORKER_VERSION:-latest}"
        next_step "Create networker-endpoint service ($label)"
        _azure_win_create_endpoint_service "$rg" "$vm"
    else
        _wait_for_ssh "$ip" "azureuser" "$label"
        _remote_install_binary "networker-endpoint" "$ip" "azureuser"
        next_step "Create networker-endpoint service ($label)"
        _remote_create_endpoint_service "$ip" "azureuser"
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
        # TCP 80, 443, 8080, 8443 (80/443 redirect to 8080/8443 via iptables on VM)
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol tcp --port 80 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol tcp --port 443 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol tcp --port 8080 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol tcp --port 8443 --cidr 0.0.0.0/0 --output text >/dev/null
        # UDP 8443, 9998, 9999
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol udp --port 8443 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol udp --port 9998 --cidr 0.0.0.0/0 --output text >/dev/null
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$_sg_created" \
            --protocol udp --port 9999 --cidr 0.0.0.0/0 --output text >/dev/null
        print_ok "Security group created: $_sg_created  (TCP 22/80/443/8080/8443, UDP 8443/9998/9999)"
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

    local public_ip
    public_ip="$(aws ec2 describe-instances \
        --region "$AWS_REGION" \
        --instance-ids "$instance_id" \
        --query "Reservations[0].Instances[0].PublicIpAddress" \
        --output text)"

    if [[ -z "$public_ip" || "$public_ip" == "None" ]]; then
        print_err "Instance has no public IP — check that it is in a public subnet."
        exit 1
    fi
    printf -v "$ip_var" "%s" "$public_ip"
    print_ok "Instance running — Public IP: ${BOLD}${public_ip}${RESET}"
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

    _wait_for_ssh "$AWS_ENDPOINT_IP" "ubuntu" "endpoint instance"
    step_aws_set_auto_shutdown "$AWS_ENDPOINT_IP" "ubuntu" "endpoint instance"

    next_step "Install networker-endpoint on AWS EC2"
    _remote_install_binary "networker-endpoint" "$AWS_ENDPOINT_IP" "ubuntu"

    next_step "Create networker-endpoint service (AWS)"
    _remote_create_endpoint_service "$AWS_ENDPOINT_IP" "ubuntu"

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
        --rules=tcp:22,tcp:80,tcp:443,tcp:3389,tcp:8080,tcp:8443,udp:8443,udp:9998,udp:9999 \
        --source-ranges=0.0.0.0/0 \
        --target-tags=networker-endpoint \
        --quiet
    print_ok "Firewall rule created: TCP 22/80/443/3389/8080/8443, UDP 8443/9998/9999"
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
                --ssh-flag="-o StrictHostKeyChecking=no" \
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
        --ssh-flag="-o StrictHostKeyChecking=no" \
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
                --ssh-flag="-o StrictHostKeyChecking=no" \
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
        --ssh-flag="-o StrictHostKeyChecking=no" \
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
        --ssh-flag="-o StrictHostKeyChecking=no" \
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

# Create a Windows Service for networker-endpoint on a GCE Windows VM.
_gcp_win_create_endpoint_service() {
    local name="$1"
    next_step "Create networker-endpoint Windows service (GCP)"
    print_info "Creating Windows Service and opening firewall ports…"
    gcloud compute ssh "$name" \
        --project "$GCP_PROJECT" \
        --zone "$GCP_ZONE" \
        --quiet \
        --ssh-flag="-o StrictHostKeyChecking=no" \
        --command "powershell -Command \"\$ErrorActionPreference='Continue'; \
            sc.exe create networker-endpoint binPath='C:\\networker\\networker-endpoint.exe' start=auto; \
            sc.exe description networker-endpoint 'Networker Endpoint diagnostics server'; \
            sc.exe start networker-endpoint; \
            netsh advfirewall firewall add rule name='Networker-HTTP'  protocol=TCP dir=in action=allow localport=8080; \
            netsh advfirewall firewall add rule name='Networker-HTTPS' protocol=TCP dir=in action=allow localport=8443; \
            netsh advfirewall firewall add rule name='Networker-UDP'   protocol=UDP dir=in action=allow localport='8443,9998,9999'\"" < /dev/null

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
        --ssh-flag="-o StrictHostKeyChecking=no" \
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

    if [[ "$GCP_ENDPOINT_OS" == "windows" ]]; then
        _gcp_wait_for_windows_vm "$GCP_ENDPOINT_NAME" "endpoint instance"
        _gcp_reset_windows_password "$GCP_ENDPOINT_NAME" "endpoint"
        _gcp_win_set_auto_shutdown "$GCP_ENDPOINT_NAME" "endpoint instance"
        _gcp_win_install_binary "networker-endpoint" "$GCP_ENDPOINT_NAME"
        _gcp_win_create_endpoint_service "$GCP_ENDPOINT_NAME"
        _gcp_verify_health "$GCP_ENDPOINT_NAME" "$GCP_ENDPOINT_IP"
    else
        _gcp_wait_for_ssh "$GCP_ENDPOINT_NAME" "endpoint instance"
        _gcp_set_auto_shutdown "$GCP_ENDPOINT_NAME" "endpoint instance"
        _gcp_install_binary "networker-endpoint" "$GCP_ENDPOINT_NAME"
        _gcp_create_endpoint_service "$GCP_ENDPOINT_NAME"
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
            ssh -o StrictHostKeyChecking=no "$dest"
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
    local ssh_opts=(-o StrictHostKeyChecking=no)
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
        DEPLOY_EP_PROVIDERS+=("$ep_prov")
        DEPLOY_EP_LABELS+=("$ep_label")
        DEPLOY_EP_IPS+=("")  # placeholder, filled after deploy
    done

    DO_INSTALL_ENDPOINT=1

    # ── Tests ─────────────────────────────────────────────────────────────
    local run_tests; run_tests="$(jq -r '.tests.run_tests // true' "$cfg")"
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
                           -o StrictHostKeyChecking=no \
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
                           -o StrictHostKeyChecking=no \
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

    # Build targets array from deployed endpoint IPs
    local targets_json=""
    local i
    for i in $(seq 0 $((DEPLOY_ENDPOINT_COUNT - 1))); do
        local ip="${DEPLOY_EP_IPS[$i]}"
        [[ -z "$ip" ]] && continue
        [[ -n "$targets_json" ]] && targets_json="${targets_json}, "
        targets_json="${targets_json}\"https://${ip}:8443/health\""
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

    json="${json}\n}"

    printf "%b" "$json" > "$CONFIG_FILE_PATH"
    print_ok "Config written to ${CONFIG_FILE_PATH}"
    print_info "Targets: $(echo "$targets_json" | tr ',' '\n' | wc -l | tr -d ' ') endpoint(s)"

    # Upload config to remote tester if applicable
    local tester_ip="" tester_user="" tester_scp_opts=(-o StrictHostKeyChecking=no -q)
    case "$TESTER_LOCATION" in
        lan)   tester_ip="$LAN_TESTER_IP"; tester_user="$LAN_TESTER_USER"
               tester_scp_opts=(-o StrictHostKeyChecking=no -P "$LAN_TESTER_PORT" -q) ;;
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
        print_info "Running: $tester_bin --config $CONFIG_FILE_PATH"
        echo ""
        "$tester_bin" --config "$CONFIG_FILE_PATH"
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
    local ssh_opts=(-o StrictHostKeyChecking=no)
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
    local scp_opts=(-o StrictHostKeyChecking=no -q)
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
                DEPLOY_EP_IPS[$i]="$LAN_ENDPOINT_IP"
                ;;
            azure)
                step_check_azure_prereqs
                step_azure_deploy_endpoint
                DEPLOY_EP_IPS[$i]="$AZURE_ENDPOINT_IP"
                ;;
            aws)
                step_aws_deploy_endpoint
                DEPLOY_EP_IPS[$i]="$AWS_ENDPOINT_IP"
                ;;
            gcp)
                step_gcp_deploy_endpoint
                DEPLOY_EP_IPS[$i]="$GCP_ENDPOINT_IP"
                ;;
        esac

        print_ok "Endpoint $label deployed: ${DEPLOY_EP_IPS[$i]}"
    done

    # Phase 7: Generate tester config from deployed IPs
    _deploy_generate_tester_config

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

    # ── Local endpoint: offer systemd service (Linux only) ────────────────────
    if [[ $do_local_endpoint -eq 1 && "$SYS_OS" == "Linux" ]] && command -v systemctl &>/dev/null; then
        echo ""
        if ask_yn "Set up networker-endpoint as a systemd service (auto-starts on boot)?" "y"; then
            step_setup_endpoint_service
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
