#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – unified installer (rustup-style)
#
# Installs networker-tester and/or networker-endpoint either:
#   locally  – on this machine (release binary download or source compile)
#   remotely – provisioned on a cloud VM (Azure and AWS supported)
#
# Two local install modes (auto-detected, or choose in customize flow):
#   release  – download pre-built binary from the latest GitHub release via
#              gh CLI (fast, ~10 s); requires: gh installed + gh auth login
#   source   – compile from source via cargo install (slower, ~5-10 min);
#              requires: SSH key for the private repo + Rust/cargo
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
#   -h, --help               Show this help message
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_SSH="ssh://git@github.com/irlm/networker-tester"
REPO_GH="irlm/networker-tester"
INSTALL_DIR="${HOME}/.cargo/bin"

# ── Colours (ANSI C quoting; safe even when stdin is a curl pipe) ─────────────
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

Local install modes (auto-detected; override in customize flow or via flag):
  release   Download pre-built binary via gh CLI — fast (~10 s)
            Requires: gh installed and authenticated (gh auth login)
  source    Compile from private Git repo via cargo install — slower (~5-10 min)
            Requires: SSH key for github.com + Rust/cargo

Options:
  -y, --yes                Non-interactive: accept all defaults (local install)
  --from-source            Force source-compile mode (skip release detection)
  --skip-ssh-check         Skip the GitHub SSH connectivity test (source mode only)
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

  -h, --help               Show this help message

Examples:
  bash install.sh -y endpoint
  bash install.sh --azure endpoint
  bash install.sh --azure --region westeurope both
  bash install.sh --aws endpoint
  bash install.sh --aws --aws-region eu-west-1 both
  bash install.sh --tester-aws --aws both          # both on separate AWS instances
  bash install.sh --tester-azure --aws both        # tester on Azure, endpoint on AWS
EOF
}

# ── Script-level state ────────────────────────────────────────────────────────
COMPONENT="both"
AUTO_YES=0
FROM_SOURCE=0
SKIP_SSH=0
SKIP_RUST=0

INSTALL_METHOD="source"   # "release" | "source"
RELEASE_AVAILABLE=0
RELEASE_TARGET=""
NETWORKER_VERSION=""      # populated in discover_system (gh query or fallback below)
INSTALLER_VERSION="v0.12.62"  # fallback when gh is unavailable

DO_SSH_CHECK=1
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
TESTER_LOCATION="local"       # "local" | "azure" | "aws"
ENDPOINT_LOCATION="local"     # "local" | "azure" | "aws"
DO_REMOTE_TESTER=0
DO_REMOTE_ENDPOINT=0

# ── Azure state ───────────────────────────────────────────────────────────────
AZURE_CLI_AVAILABLE=0
AZURE_LOGGED_IN=0
AZURE_REGION="eastus"
AZURE_REGION_ASKED=0

AZURE_TESTER_RG="networker-rg-tester"
AZURE_TESTER_VM="networker-tester-vm"
AZURE_TESTER_SIZE="Standard_B2s"
AZURE_TESTER_IP=""

AZURE_ENDPOINT_RG="networker-rg-endpoint"
AZURE_ENDPOINT_VM="networker-endpoint-vm"
AZURE_ENDPOINT_SIZE="Standard_B2s"
AZURE_ENDPOINT_IP=""

# ── AWS state ─────────────────────────────────────────────────────────────────
AWS_CLI_AVAILABLE=0
AWS_LOGGED_IN=0
AWS_REGION="us-east-1"
AWS_REGION_ASKED=0

AWS_TESTER_NAME="networker-tester"
AWS_TESTER_INSTANCE_TYPE="t3.small"
AWS_TESTER_INSTANCE_ID=""
AWS_TESTER_IP=""

AWS_ENDPOINT_NAME="networker-endpoint"
AWS_ENDPOINT_INSTANCE_TYPE="t3.small"
AWS_ENDPOINT_INSTANCE_ID=""
AWS_ENDPOINT_IP=""

CONFIG_FILE_PATH=""

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
            --skip-ssh-check)
                SKIP_SSH=1 ;;
            --skip-rust)
                SKIP_RUST=1 ;;
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

    if [[ $SKIP_SSH -eq 1 ]]; then
        DO_SSH_CHECK=0
    fi

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

# ── Target triple detection ───────────────────────────────────────────────────
detect_release_target() {
    local os arch
    os="$(uname -s 2>/dev/null || echo "")"
    arch="$(uname -m 2>/dev/null || echo "")"

    case "$os" in
        Linux)
            case "$arch" in
                x86_64) echo "x86_64-unknown-linux-gnu" ;;
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
        if [[ -n "$RELEASE_TARGET" ]] \
           && command -v gh &>/dev/null \
           && gh auth status &>/dev/null 2>&1; then
            RELEASE_AVAILABLE=1
            INSTALL_METHOD="release"
            NETWORKER_VERSION="$(gh release list --repo "$REPO_GH" \
                --limit 1 --json tagName -q '.[0].tagName' 2>/dev/null || echo "")"
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
            --limit 1 --json tagName -q '.[0].tagName' 2>/dev/null || echo "")"
    fi
    # Fallback: use the version embedded in this installer script
    if [[ -z "$NETWORKER_VERSION" ]]; then
        NETWORKER_VERSION="$INSTALLER_VERSION"
    fi

    # Azure CLI detection
    if command -v az &>/dev/null; then
        AZURE_CLI_AVAILABLE=1
        if az account show &>/dev/null 2>&1; then
            AZURE_LOGGED_IN=1
        fi
    fi

    # AWS CLI detection
    if command -v aws &>/dev/null; then
        AWS_CLI_AVAILABLE=1
        if aws sts get-caller-identity &>/dev/null 2>&1; then
            AWS_LOGGED_IN=1
        fi
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
            printf "    %-22s %s\n" "AWS CLI:" "installed  (run: aws configure)"
        fi
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
            printf "    ${BOLD}Local method:${RESET}  Compile from source  ${DIM}(~5-10 min)${RESET}\n"
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

            if [[ $CHROME_AVAILABLE -eq 0 && $DO_INSTALL_TESTER -eq 1 ]]; then
                if [[ $DO_CHROME_INSTALL -eq 1 ]]; then
                    printf "    %s. ${BOLD}Install Chrome${RESET}         Install via %s (browser probe)\n" "$step" "$PKG_MGR"
                    step=$((step + 1))
                elif [[ -n "$PKG_MGR" ]]; then
                    printf "    ${DIM}-. Install Chrome         (will ask — browser probe disabled if skipped)${RESET}\n"
                else
                    printf "    ${DIM}-. Install Chrome         (not installed — https://www.google.com/chrome/)${RESET}\n"
                fi
            fi

            if [[ $DO_SSH_CHECK -eq 1 ]]; then
                printf "    %s. ${BOLD}SSH check${RESET}              Verify GitHub SSH access\n" "$step"
                step=$((step + 1))
            else
                printf "    ${DIM}-. SSH check              (skipped)${RESET}\n"
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
                printf "    %s. ${BOLD}Install networker-tester${RESET}   cargo install from private Git repo%s\n" "$step" "$browser_note"
                step=$((step + 1))
            fi
            if [[ $do_local_endpoint -eq 1 ]]; then
                printf "    %s. ${BOLD}Install networker-endpoint${RESET} cargo install from private Git repo\n" "$step"
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
            print_dim "Repository:  $REPO_SSH"
            print_dim "Source code is compiled locally — no pre-built binaries are downloaded."
        fi
    fi

    # ── Remote — tester ───────────────────────────────────────────────────────
    if [[ $DO_REMOTE_TESTER -eq 1 ]]; then
        echo ""
        case "$TESTER_LOCATION" in
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
        esac
        echo ""
        printf "    ${DIM}a. Provision VM + open TCP 8080, 8443 and UDP 8443, 9998, 9999\n"
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

    if [[ "$component" == "tester" ]]; then
        printf "  Resource group name [%s]: " "$AZURE_TESTER_RG"
        local rg_ans; read -r rg_ans </dev/tty || true
        AZURE_TESTER_RG="${rg_ans:-$AZURE_TESTER_RG}"

        printf "  VM name             [%s]: " "$AZURE_TESTER_VM"
        local vm_ans; read -r vm_ans </dev/tty || true
        AZURE_TESTER_VM="${vm_ans:-$AZURE_TESTER_VM}"

        AZURE_TESTER_SIZE="$chosen_size"
        print_ok "Size: $AZURE_TESTER_SIZE  |  RG: $AZURE_TESTER_RG  |  VM: $AZURE_TESTER_VM"
    else
        printf "  Resource group name [%s]: " "$AZURE_ENDPOINT_RG"
        local rg_ans; read -r rg_ans </dev/tty || true
        AZURE_ENDPOINT_RG="${rg_ans:-$AZURE_ENDPOINT_RG}"

        printf "  VM name             [%s]: " "$AZURE_ENDPOINT_VM"
        local vm_ans; read -r vm_ans </dev/tty || true
        AZURE_ENDPOINT_VM="${vm_ans:-$AZURE_ENDPOINT_VM}"

        AZURE_ENDPOINT_SIZE="$chosen_size"
        print_ok "Size: $AZURE_ENDPOINT_SIZE  |  RG: $AZURE_ENDPOINT_RG  |  VM: $AZURE_ENDPOINT_VM"
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

    # Instance Name tag
    if [[ "$component" == "tester" ]]; then
        printf "  Instance name tag [%s]: " "$AWS_TESTER_NAME"
        local name_ans; read -r name_ans </dev/tty || true
        AWS_TESTER_NAME="${name_ans:-$AWS_TESTER_NAME}"
        AWS_TESTER_INSTANCE_TYPE="$chosen_type"
        print_ok "Type: $AWS_TESTER_INSTANCE_TYPE  |  Name: $AWS_TESTER_NAME"
    else
        printf "  Instance name tag [%s]: " "$AWS_ENDPOINT_NAME"
        local name_ans; read -r name_ans </dev/tty || true
        AWS_ENDPOINT_NAME="${name_ans:-$AWS_ENDPOINT_NAME}"
        AWS_ENDPOINT_INSTANCE_TYPE="$chosen_type"
        print_ok "Type: $AWS_ENDPOINT_INSTANCE_TYPE  |  Name: $AWS_ENDPOINT_NAME"
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

    if [[ $AZURE_LOGGED_IN -eq 0 ]]; then
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
        if [[ -n "$PKG_MGR" ]]; then
            local install_cmd
            case "$PKG_MGR" in
                brew)    install_cmd="brew install awscli" ;;
                apt-get) install_cmd="sudo apt-get install -y awscli" ;;
                dnf)     install_cmd="sudo dnf install -y awscli" ;;
                pacman)  install_cmd="sudo pacman -S --noconfirm aws-cli" ;;
                zypper)  install_cmd="sudo zypper install -y aws-cli" ;;
                *)       install_cmd="" ;;
            esac
            if [[ -n "$install_cmd" ]]; then
                echo "  Install command:  $install_cmd"
                echo ""
                if ask_yn "Install AWS CLI now?" "y"; then
                    echo ""
                    case "$PKG_MGR" in
                        brew)    brew install awscli ;;
                        apt-get) sudo apt-get install -y awscli ;;
                        dnf)     sudo dnf install -y awscli ;;
                        pacman)  sudo pacman -S --noconfirm aws-cli ;;
                        zypper)  sudo zypper install -y aws-cli ;;
                    esac
                    if command -v aws &>/dev/null; then
                        AWS_CLI_AVAILABLE=1
                        print_ok "AWS CLI installed"
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
        else
            echo "  Install from: https://aws.amazon.com/cli/"
            echo "  Then re-run:  bash install.sh --aws"
            exit 1
        fi
    fi

    if [[ $AWS_LOGGED_IN -eq 0 ]]; then
        echo ""
        print_warn "AWS CLI is not configured or credentials are not valid."
        echo ""
        echo "  Run 'aws configure' to set up your access key, secret, and default region."
        echo "  Or set environment variables: AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, AWS_DEFAULT_REGION"
        echo ""
        if ask_yn "Run 'aws configure' now?" "y"; then
            aws configure
            if aws sts get-caller-identity &>/dev/null 2>&1; then
                AWS_LOGGED_IN=1
                local aws_account
                aws_account="$(aws sts get-caller-identity --query Account --output text 2>/dev/null || echo "")"
                print_ok "AWS authenticated  (account: $aws_account)"
            else
                print_err "AWS credentials invalid — run 'aws configure' then re-run this installer."
                exit 1
            fi
        else
            print_err "AWS credentials required for remote deployment."
            echo "  Run:  aws configure"
            exit 1
        fi
    fi
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
            echo "    2) Remote: Azure VM"
            echo "    3) Remote: AWS EC2"
            echo ""
            printf "  Choice [1]: "
            local ans
            read -r ans </dev/tty || true
            ans="${ans:-1}"
            case "$ans" in
                2) TESTER_LOCATION="azure"; DO_REMOTE_TESTER=1 ;;
                3) TESTER_LOCATION="aws";   DO_REMOTE_TESTER=1 ;;
            esac
        fi
        if [[ $DO_REMOTE_TESTER -eq 1 ]]; then
            case "$TESTER_LOCATION" in
                azure) ensure_azure_cli; ask_azure_options "tester" ;;
                aws)   ensure_aws_cli;   ask_aws_options   "tester" ;;
            esac
        fi
    fi

    # Endpoint location
    if [[ $DO_INSTALL_ENDPOINT -eq 1 ]]; then
        if [[ $DO_REMOTE_ENDPOINT -eq 0 ]]; then
            echo ""
            echo "  ${BOLD}Where to install networker-endpoint?${RESET}"
            echo "    1) Locally on this machine  [default]"
            echo "    2) Remote: Azure VM"
            echo "    3) Remote: AWS EC2"
            echo ""
            printf "  Choice [1]: "
            local ans
            read -r ans </dev/tty || true
            ans="${ans:-1}"
            case "$ans" in
                2) ENDPOINT_LOCATION="azure"; DO_REMOTE_ENDPOINT=1 ;;
                3) ENDPOINT_LOCATION="aws";   DO_REMOTE_ENDPOINT=1 ;;
            esac
        fi
        if [[ $DO_REMOTE_ENDPOINT -eq 1 ]]; then
            case "$ENDPOINT_LOCATION" in
                azure) ensure_azure_cli; ask_azure_options "endpoint" ;;
                aws)   ensure_aws_cli;   ask_aws_options   "endpoint" ;;
            esac
        fi
    fi
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

    while true; do
        if [[ "$default" == "y" ]]; then
            printf "  %s [Y/n]: " "$prompt"
        else
            printf "  %s [y/N]: " "$prompt"
        fi
        read -r ans </dev/tty || true
        ans="${ans:-$default}"
        case "${ans,,}" in
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

        if ask_yn "Run SSH connectivity check for GitHub?" "y"; then
            DO_SSH_CHECK=1
        else
            DO_SSH_CHECK=0
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
        2) DO_INSTALL_TESTER=1; DO_INSTALL_ENDPOINT=0 ;;
        3) DO_INSTALL_TESTER=0; DO_INSTALL_ENDPOINT=1 ;;
        *) DO_INSTALL_TESTER=1; DO_INSTALL_ENDPOINT=1 ;;
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

    next_step "Download $binary"
    print_info "Fetching ${archive} from latest GitHub release…"

    if ! gh release download \
            --repo "$REPO_GH" \
            --latest \
            --pattern "${archive}" \
            --dir "${tmp_dir}" \
            --clobber; then
        echo ""
        print_err "gh release download failed."
        echo ""
        echo "  Expected asset: ${archive}"
        echo "  Check available releases: gh release list --repo $REPO_GH"
        rm -rf "$tmp_dir"
        exit 1
    fi

    mkdir -p "$INSTALL_DIR"
    tar xzf "${tmp_dir}/${archive}" -C "$INSTALL_DIR"
    chmod +x "${INSTALL_DIR}/${binary}"
    rm -rf "$tmp_dir"

    local installed_ver
    installed_ver="$("${INSTALL_DIR}/${binary}" --version 2>/dev/null || echo "unknown")"
    print_ok "$binary installed → ${INSTALL_DIR}/${binary}  ($installed_ver)"
}

# ── Source-mode steps ─────────────────────────────────────────────────────────
step_ssh_check() {
    next_step "Verify GitHub SSH access"
    print_info "Connecting to git@github.com…"

    local ssh_out
    ssh_out=$(ssh -o BatchMode=yes \
                  -o StrictHostKeyChecking=accept-new \
                  -o ConnectTimeout=10 \
                  -T git@github.com </dev/null 2>&1 || true)

    if printf '%s' "$ssh_out" | grep -q "successfully authenticated"; then
        print_ok "SSH access confirmed"
    else
        echo ""
        print_err "SSH authentication to GitHub failed."
        echo ""
        echo "  Raw output: $ssh_out"
        echo ""
        echo "  Ensure your SSH key is loaded and has access to the private repo."
        echo "  Test manually:  ssh -T git@github.com"
        exit 1
    fi
}

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
    print_info "Building and installing $binary from source…"
    print_dim "This compiles from the private Git repo and may take a few minutes."

    if ! command -v cc &>/dev/null && ! command -v gcc &>/dev/null && ! command -v clang &>/dev/null; then
        echo ""
        print_warn "No C linker found (cc/gcc/clang) — cargo will likely fail."
        case "$SYS_OS" in
            Darwin)
                echo "  Install Xcode Command Line Tools:"
                echo "    xcode-select --install"
                ;;
            Linux)
                case "$PKG_MGR" in
                    apt-get) echo "  sudo apt-get install -y build-essential" ;;
                    dnf)     echo "  sudo dnf install -y gcc gcc-c++ make" ;;
                    pacman)  echo "  sudo pacman -S --noconfirm base-devel" ;;
                    zypper)  echo "  sudo zypper install -y gcc make" ;;
                    apk)     echo "  sudo apk add build-base" ;;
                    *)       echo "  Install gcc or clang via your package manager." ;;
                esac
                ;;
        esac
        echo "  Then re-run this installer."
    fi
    echo ""

    local features_arg=""
    if [[ $CHROME_AVAILABLE -eq 1 && "$binary" == "networker-tester" ]]; then
        features_arg="--features browser"
        print_info "Chrome detected — compiling with browser probe support."
    fi

    if command -v git &>/dev/null; then
        CARGO_NET_GIT_FETCH_WITH_CLI=true \
            cargo install --git "$REPO_SSH" "$binary" --locked --force $features_arg </dev/null
    else
        cargo install --git "$REPO_SSH" "$binary" --locked --force $features_arg </dev/null
    fi

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

# Download binary from GitHub release and install it on a remote host.
# $1 = binary ("networker-tester" or "networker-endpoint")
# $2 = public IP
# $3 = SSH user ("azureuser" or "ubuntu")
_remote_install_binary() {
    local binary="$1" ip="$2" user="$3"

    # Detect remote architecture
    local remote_arch remote_target
    remote_arch="$(ssh -o StrictHostKeyChecking=no "${user}@${ip}" "uname -m" 2>/dev/null || echo "x86_64")"
    case "$remote_arch" in
        x86_64)        remote_target="x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) remote_target="aarch64-unknown-linux-gnu" ;;
        *)             remote_target="x86_64-unknown-linux-gnu" ;;
    esac

    local archive="${binary}-${remote_target}.tar.gz"
    local tmp_dir
    tmp_dir="$(mktemp -d)"

    print_info "Downloading ${archive} from GitHub release…"
    if gh release download \
            --repo "$REPO_GH" \
            --latest \
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
        local ver="${NETWORKER_VERSION:-}"
        if [[ -z "$ver" ]]; then
            ver="$(gh release list --repo "$REPO_GH" --limit 1 --json tagName \
                   -q '.[0].tagName' 2>/dev/null || echo "")"
        fi
        if [[ -z "$ver" ]]; then
            print_err "Cannot determine release version for remote download."
            exit 1
        fi
        print_info "Downloading directly on VM (${ver})…"
        ssh -o StrictHostKeyChecking=no "${user}@${ip}" \
            "curl -fsSL https://github.com/${REPO_GH}/releases/download/${ver}/${archive} \
               -o /tmp/${archive} && \
             tar xzf /tmp/${archive} -C /tmp && \
             sudo mv /tmp/${binary} /usr/local/bin/${binary} && \
             sudo chmod +x /usr/local/bin/${binary} && \
             rm /tmp/${archive}"
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
REMOTE

    sleep 2
    print_ok "networker-endpoint service enabled and started"
}

# Poll /health until the endpoint responds.
# $1 = public IP
_remote_verify_health() {
    local ip="$1"

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
            print_warn "Check logs on the VM: sudo journalctl -u networker-endpoint -n 50"
            return 0
        fi
        printf "."
        sleep 5
    done
}

# Write networker-cloud.json pointing at the remote endpoint.
# $1 = endpoint public IP
step_generate_config() {
    local endpoint_ip="$1"

    next_step "Generate test config file"

    CONFIG_FILE_PATH="${PWD}/networker-cloud.json"

    cat > "$CONFIG_FILE_PATH" <<EOF
{
  "target": "https://${endpoint_ip}:8443/health",
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

    # If the tester is also remote, upload the config there too
    local tester_ip="" tester_user=""
    case "$TESTER_LOCATION" in
        azure) tester_ip="$AZURE_TESTER_IP"; tester_user="azureuser" ;;
        aws)   tester_ip="$AWS_TESTER_IP";   tester_user="ubuntu" ;;
    esac
    if [[ -n "$tester_ip" ]]; then
        print_info "Uploading config to tester VM ($tester_ip)…"
        scp -o StrictHostKeyChecking=no -q \
            "$CONFIG_FILE_PATH" \
            "${tester_user}@${tester_ip}:~/networker-cloud.json"
        print_ok "Config uploaded to ~/networker-cloud.json on tester VM"
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

    if [[ $AZURE_LOGGED_IN -eq 0 ]]; then
        print_info "Not logged in to Azure — running az login…"
        az login
        if ! az account show &>/dev/null 2>&1; then
            print_err "Azure login failed."
            exit 1
        fi
        AZURE_LOGGED_IN=1
    fi

    local sub_name sub_id
    sub_name="$(az account show --query name -o tsv 2>/dev/null || echo "unknown")"
    sub_id="$(az account show --query id -o tsv 2>/dev/null || echo "")"
    print_ok "Subscription: ${sub_name}  (${sub_id})"
}

# Create an Azure resource group and Ubuntu 22.04 VM.
# $1 = label ("tester" or "endpoint"), $2 = RG, $3 = VM name,
# $4 = VM size, $5 = name of global variable to receive public IP
step_azure_create_vm() {
    local label="$1" rg="$2" vm="$3" size="$4" ip_var="$5"

    next_step "Create Azure VM for $label ($vm in $AZURE_REGION)"

    print_info "Creating resource group '$rg' in $AZURE_REGION…"
    az group create --name "$rg" --location "$AZURE_REGION" --output none
    print_ok "Resource group: $rg"

    local ssh_key_option="--generate-ssh-keys"
    local key_file
    for key_file in "${HOME}/.ssh/id_ed25519.pub" "${HOME}/.ssh/id_rsa.pub"; do
        if [[ -f "$key_file" ]]; then
            ssh_key_option="--ssh-key-values @${key_file}"
            break
        fi
    done

    print_info "Creating Ubuntu 22.04 VM '$vm' ($size)…"
    print_dim "This typically takes 1–2 minutes…"
    echo ""

    local ip
    ip="$(az vm create \
        --resource-group "$rg" \
        --name "$vm" \
        --image Ubuntu2204 \
        --size "$size" \
        --admin-username azureuser \
        $ssh_key_option \
        --output tsv \
        --query publicIpAddress)"

    if [[ -z "$ip" ]]; then
        print_err "Failed to retrieve VM public IP address."
        echo "  Check the Azure portal for resource group: $rg"
        exit 1
    fi

    printf -v "$ip_var" "%s" "$ip"
    print_ok "VM created — Public IP: ${BOLD}${ip}${RESET}"
}

# Open TCP 8080/8443 and UDP 8443/9998/9999 on the NSG for the endpoint VM.
step_azure_open_endpoint_ports() {
    local rg="$1" vm="$2"

    next_step "Open firewall ports (Azure)"

    print_info "Opening TCP 8080, 8443…"
    az vm open-port --resource-group "$rg" --name "$vm" --port 8080 --priority 1001 --output none
    az vm open-port --resource-group "$rg" --name "$vm" --port 8443 --priority 1002 --output none
    print_ok "TCP 8080, 8443 open"

    print_info "Opening UDP 8443, 9998, 9999…"
    local nsg_name
    nsg_name="$(az network nsg list \
        --resource-group "$rg" \
        --query "[?contains(name, '${vm}')].name | [0]" \
        -o tsv 2>/dev/null || echo "")"

    if [[ -z "$nsg_name" ]]; then
        nsg_name="$(az network nsg list \
            --resource-group "$rg" \
            --query "[0].name" \
            -o tsv 2>/dev/null || echo "")"
    fi

    if [[ -z "$nsg_name" ]]; then
        print_warn "Could not detect NSG name; UDP ports may not be open."
        print_warn "Open manually: az network nsg rule create --resource-group $rg --nsg-name <nsg> \\"
        print_warn "  --name Networker-UDP --protocol Udp --direction Inbound \\"
        print_warn "  --priority 1010 --destination-port-ranges 8443 9998 9999 --access Allow"
    else
        az network nsg rule create \
            --resource-group "$rg" \
            --nsg-name "$nsg_name" \
            --name "Networker-UDP" \
            --protocol Udp \
            --direction Inbound \
            --priority 1010 \
            --destination-port-ranges 8443 9998 9999 \
            --access Allow \
            --output none
        print_ok "UDP 8443, 9998, 9999 open"
    fi
}

step_azure_deploy_tester() {
    step_check_azure_prereqs
    step_azure_create_vm "tester" \
        "$AZURE_TESTER_RG" "$AZURE_TESTER_VM" "$AZURE_TESTER_SIZE" "AZURE_TESTER_IP"
    _wait_for_ssh "$AZURE_TESTER_IP" "azureuser" "tester VM"

    next_step "Install networker-tester on Azure VM"
    _remote_install_binary "networker-tester" "$AZURE_TESTER_IP" "azureuser"
}

step_azure_deploy_endpoint() {
    # Only check prereqs once (shared with tester path if both are Azure)
    if [[ "$TESTER_LOCATION" != "azure" || -z "$AZURE_TESTER_IP" ]]; then
        step_check_azure_prereqs
    fi
    step_azure_create_vm "endpoint" \
        "$AZURE_ENDPOINT_RG" "$AZURE_ENDPOINT_VM" "$AZURE_ENDPOINT_SIZE" "AZURE_ENDPOINT_IP"
    step_azure_open_endpoint_ports "$AZURE_ENDPOINT_RG" "$AZURE_ENDPOINT_VM"
    _wait_for_ssh "$AZURE_ENDPOINT_IP" "azureuser" "endpoint VM"

    next_step "Install networker-endpoint on Azure VM"
    _remote_install_binary "networker-endpoint" "$AZURE_ENDPOINT_IP" "azureuser"

    next_step "Create networker-endpoint service (Azure)"
    _remote_create_endpoint_service "$AZURE_ENDPOINT_IP" "azureuser"

    next_step "Verify endpoint health (Azure)"
    _remote_verify_health "$AZURE_ENDPOINT_IP"

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

    if [[ $AWS_LOGGED_IN -eq 0 ]]; then
        print_info "No active AWS credentials — running aws configure…"
        print_info "You need: Access Key ID, Secret Access Key, and default region."
        aws configure
        if ! aws sts get-caller-identity &>/dev/null 2>&1; then
            print_err "AWS credentials are not valid."
            echo "  Run: aws configure"
            exit 1
        fi
        AWS_LOGGED_IN=1
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
    # Import is idempotent when the key already exists (returns existing key)
    aws ec2 import-key-pair \
        --region "$AWS_REGION" \
        --key-name "networker-keypair" \
        --public-key-material "fileb://${ssh_key_file}" \
        --output none 2>/dev/null || true
    print_ok "Key pair: networker-keypair"
}

# Look up the latest Ubuntu 22.04 LTS AMI for the current region.
# Sets the global AWS_AMI_ID.
AWS_AMI_ID=""
_aws_find_ubuntu_ami() {
    print_info "Looking up Ubuntu 22.04 LTS AMI in $AWS_REGION…"
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
    local sg_id
    sg_id="$(aws ec2 create-security-group \
        --region "$AWS_REGION" \
        --group-name "$sg_name" \
        --description "Networker ${component} security group" \
        --query "GroupId" \
        --output text)"

    # SSH (always)
    aws ec2 authorize-security-group-ingress \
        --region "$AWS_REGION" --group-id "$sg_id" \
        --protocol tcp --port 22 --cidr 0.0.0.0/0 --output none

    if [[ "$component" == "endpoint" ]]; then
        # TCP 8080, 8443
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$sg_id" \
            --protocol tcp --port 8080 --cidr 0.0.0.0/0 --output none
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$sg_id" \
            --protocol tcp --port 8443 --cidr 0.0.0.0/0 --output none
        # UDP 8443, 9998, 9999
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$sg_id" \
            --protocol udp --port 8443 --cidr 0.0.0.0/0 --output none
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$sg_id" \
            --protocol udp --port 9998 --cidr 0.0.0.0/0 --output none
        aws ec2 authorize-security-group-ingress \
            --region "$AWS_REGION" --group-id "$sg_id" \
            --protocol udp --port 9999 --cidr 0.0.0.0/0 --output none
        print_ok "Security group created: $sg_id  (TCP 22/8080/8443, UDP 8443/9998/9999)"
    else
        print_ok "Security group created: $sg_id  (TCP 22)"
    fi

    printf -v "$sg_var" "%s" "$sg_id"
}

# Launch an EC2 instance and wait until it is running.
# $1 = label, $2 = instance type, $3 = name tag,
# $4 = SG ID, $5 = instance_id var, $6 = ip var
_aws_launch_instance() {
    local label="$1" instance_type="$2" name_tag="$3"
    local sg_id="$4" instance_id_var="$5" ip_var="$6"

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

    next_step "Install networker-endpoint on AWS EC2"
    _remote_install_binary "networker-endpoint" "$AWS_ENDPOINT_IP" "ubuntu"

    next_step "Create networker-endpoint service (AWS)"
    _remote_create_endpoint_service "$AWS_ENDPOINT_IP" "ubuntu"

    next_step "Verify endpoint health (AWS)"
    _remote_verify_health "$AWS_ENDPOINT_IP"

    step_generate_config "$AWS_ENDPOINT_IP"
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
        local t_ip="" t_user="" t_provider=""
        case "$TESTER_LOCATION" in
            azure) t_ip="$AZURE_TESTER_IP"; t_user="azureuser"; t_provider="Azure" ;;
            aws)   t_ip="$AWS_TESTER_IP";   t_user="ubuntu";    t_provider="AWS" ;;
        esac
        if [[ -n "$t_ip" ]]; then
            echo "  ${BOLD}networker-tester${RESET} (${t_provider} ${t_ip}):"
            echo "    SSH:   ssh ${t_user}@${t_ip}"
            if [[ -n "$CONFIG_FILE_PATH" ]]; then
                echo "    Tests: ssh ${t_user}@${t_ip} 'networker-tester --config ~/networker-cloud.json'"
            else
                echo "    Run:   ssh ${t_user}@${t_ip} 'networker-tester --help'"
            fi
            echo ""
        fi
    fi

    # ── Remote endpoint summary ───────────────────────────────────────────────
    if [[ $DO_REMOTE_ENDPOINT -eq 1 ]]; then
        local e_ip="" e_user="" e_provider=""
        case "$ENDPOINT_LOCATION" in
            azure) e_ip="$AZURE_ENDPOINT_IP"; e_user="azureuser"; e_provider="Azure" ;;
            aws)   e_ip="$AWS_ENDPOINT_IP";   e_user="ubuntu";    e_provider="AWS" ;;
        esac
        if [[ -n "$e_ip" ]]; then
            echo "  ${BOLD}networker-endpoint${RESET} (${e_provider} ${e_ip}):"
            echo "    Health: curl http://${e_ip}:8080/health"
            echo "    SSH:    ssh ${e_user}@${e_ip}"
            echo "    Logs:   ssh ${e_user}@${e_ip} 'sudo journalctl -u networker-endpoint -f'"
            echo "    Stop:   ssh ${e_user}@${e_ip} 'sudo systemctl stop networker-endpoint'"
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
        echo "  ${DIM}Azure cleanup (when done testing):${RESET}"
        if [[ "$TESTER_LOCATION" == "azure" ]]; then
            printf "  ${DIM}  az group delete --name %s --yes --no-wait${RESET}\n" "$AZURE_TESTER_RG"
        fi
        if [[ "$ENDPOINT_LOCATION" == "azure" ]]; then
            printf "  ${DIM}  az group delete --name %s --yes --no-wait${RESET}\n" "$AZURE_ENDPOINT_RG"
        fi
        echo ""
    fi

    if [[ "$TESTER_LOCATION" == "aws" || "$ENDPOINT_LOCATION" == "aws" ]]; then
        echo "  ${DIM}AWS cleanup (when done testing):${RESET}"
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
}

# ── Entry point ───────────────────────────────────────────────────────────────
main() {
    parse_args "$@"
    discover_system

    print_banner
    display_system_info
    display_plan
    prompt_main

    # ── Local installs ────────────────────────────────────────────────────────
    local do_local_tester=0 do_local_endpoint=0
    [[ $DO_INSTALL_TESTER -eq 1 && $DO_REMOTE_TESTER -eq 0 ]]     && do_local_tester=1
    [[ $DO_INSTALL_ENDPOINT -eq 1 && $DO_REMOTE_ENDPOINT -eq 0 ]]  && do_local_endpoint=1

    if [[ "$INSTALL_METHOD" == "release" ]]; then
        mkdir -p "$INSTALL_DIR"
        if [[ $do_local_tester -eq 1 ]]; then
            step_download_release "networker-tester"
        fi
        if [[ $do_local_endpoint -eq 1 ]]; then
            step_download_release "networker-endpoint"
        fi
    else
        if [[ $DO_CHROME_INSTALL -eq 1 ]]; then
            step_install_chrome
        fi
        if [[ $DO_GIT_INSTALL -eq 1 ]]; then
            step_install_git
        fi
        if [[ $DO_SSH_CHECK -eq 1 ]]; then
            step_ssh_check
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

    # ── Remote: tester ────────────────────────────────────────────────────────
    if [[ $DO_REMOTE_TESTER -eq 1 ]]; then
        case "$TESTER_LOCATION" in
            azure) step_azure_deploy_tester ;;
            aws)   step_aws_deploy_tester   ;;
        esac
    fi

    # ── Remote: endpoint ──────────────────────────────────────────────────────
    if [[ $DO_REMOTE_ENDPOINT -eq 1 ]]; then
        case "$ENDPOINT_LOCATION" in
            azure) step_azure_deploy_endpoint ;;
            aws)   step_aws_deploy_endpoint   ;;
        esac
    fi

    display_completion
}

main "$@"
