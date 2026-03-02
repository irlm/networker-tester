#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – Unix/macOS interactive installer (rustup-style)
#
# Two install modes (auto-detected, or choose in customize flow):
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
#   -y, --yes           Non-interactive: accept all defaults
#   --from-source       Force source-compile mode (skip release download)
#   --skip-ssh-check    Skip the GitHub SSH connectivity test (source mode)
#   --skip-rust         Skip Rust installation (source mode)
#   -h, --help          Show this help message
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_SSH="ssh://git@github.com/irlm/networker-tester"
REPO_GH="irlm/networker-tester"
SCRIPT_VERSION="0.12.19"
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
    printf "${BOLD}      Networker Tester Installer  v%-10s             ${RESET}\n" "$SCRIPT_VERSION"
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

Install modes (auto-detected; override in customize flow or via flag):
  release   Download pre-built binary via gh CLI — fast (~10 s)
            Requires: gh installed and authenticated (gh auth login)
  source    Compile from private Git repo via cargo install — slower (~5-10 min)
            Requires: SSH key for github.com + Rust/cargo

Options:
  -y, --yes           Non-interactive: accept all defaults
  --from-source       Force source-compile mode (skip release detection)
  --skip-ssh-check    Skip the GitHub SSH connectivity test (source mode only)
  --skip-rust         Skip Rust installation (source mode only)
  -h, --help          Show this help message

Examples:
  curl -fsSL <url>/install.sh | bash -s -- tester
  bash install.sh -y endpoint
  bash install.sh --from-source --skip-rust both
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
SYS_OS=""
SYS_ARCH=""
SYS_SHELL=""
STEP_NUM=0

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
# Returns the path of the Chrome/Chromium binary, or empty string if not found.
detect_chrome() {
    # 1. Explicit env override
    if [[ -n "${NETWORKER_CHROME_PATH:-}" && -x "${NETWORKER_CHROME_PATH}" ]]; then
        echo "$NETWORKER_CHROME_PATH"; return
    fi
    # 2. macOS standard locations
    local mac_paths=(
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        "/Applications/Chromium.app/Contents/MacOS/Chromium"
    )
    for p in "${mac_paths[@]}"; do
        [[ -x "$p" ]] && echo "$p" && return
    done
    # 3. Linux / PATH-visible names
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
                *)      echo "" ;;   # ARM64 Linux not yet in release matrix
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

    # Git detection
    if command -v git &>/dev/null; then
        GIT_AVAILABLE=1
    else
        GIT_AVAILABLE=0
        PKG_MGR="$(detect_pkg_manager)"
    fi

    # Release mode: available when gh is authenticated AND this platform has a
    # pre-built binary in the release matrix.
    if [[ $FROM_SOURCE -eq 0 ]]; then
        RELEASE_TARGET="$(detect_release_target)"
        if [[ -n "$RELEASE_TARGET" ]] \
           && command -v gh &>/dev/null \
           && gh auth status &>/dev/null 2>&1; then
            RELEASE_AVAILABLE=1
            INSTALL_METHOD="release"
        fi
    fi

    # Auto-offer git install only in source mode (release mode uses gh, not git)
    if [[ "$INSTALL_METHOD" == "source" && $GIT_AVAILABLE -eq 0 && -n "$PKG_MGR" ]]; then
        DO_GIT_INSTALL=1
    fi

    # Chrome detection (affects --features browser in source mode)
    CHROME_PATH="$(detect_chrome)"
    if [[ -n "$CHROME_PATH" ]]; then
        CHROME_AVAILABLE=1
    else
        CHROME_AVAILABLE=0
        # Auto-offer Chrome install only in source mode when a pkg manager is available
        if [[ "$INSTALL_METHOD" == "source" && -n "$PKG_MGR" ]]; then
            DO_CHROME_INSTALL=1
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
}

display_plan() {
    print_section "Installation Plan"
    echo ""

    if [[ "$INSTALL_METHOD" == "release" ]]; then
        printf "    ${BOLD}Method:${RESET}  Download binary from GitHub release  ${DIM}(fast)${RESET}\n"
        printf "    ${DIM}Target:  %s${RESET}\n" "$RELEASE_TARGET"
        echo ""

        local step=1
        if [[ $DO_INSTALL_TESTER -eq 1 ]]; then
            printf "    %s. ${BOLD}Download networker-tester${RESET}    gh release download (latest)\n" "$step"
            step=$((step + 1))
        fi
        if [[ $DO_INSTALL_ENDPOINT -eq 1 ]]; then
            printf "    %s. ${BOLD}Download networker-endpoint${RESET}  gh release download (latest)\n" "$step"
        fi
        echo ""
        print_dim "Repository:  $REPO_GH  (latest release)"
    else
        printf "    ${BOLD}Method:${RESET}  Compile from source  ${DIM}(~5-10 min)${RESET}\n"
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

        if [[ $CHROME_AVAILABLE -eq 0 ]]; then
            if [[ $DO_CHROME_INSTALL -eq 1 ]]; then
                printf "    %s. ${BOLD}Install Chrome${RESET}         Install via %s (browser probe)\n" "$step" "$PKG_MGR"
                step=$((step + 1))
            elif [[ -n "$PKG_MGR" ]]; then
                printf "    ${DIM}-. Install Chrome         (skip – toggle in Customize; browser probe disabled)${RESET}\n"
            else
                printf "    ${DIM}-. Install Chrome         (not installed – https://www.google.com/chrome/)${RESET}\n"
            fi
        fi

        if [[ $DO_SSH_CHECK -eq 1 ]]; then
            printf "    %s. ${BOLD}SSH check${RESET}              Verify GitHub SSH access\n" "$step"
            step=$((step + 1))
        else
            printf "    ${DIM}-. SSH check              (skipped)${RESET}\n"
        fi

        if [[ $DO_RUST_INSTALL -eq 1 ]]; then
            printf "    %s. ${BOLD}Install Rust${RESET}           Download rustup from sh.rustup.rs and run installer\n" "$step"
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
        if [[ $DO_INSTALL_TESTER -eq 1 ]]; then
            printf "    %s. ${BOLD}Install networker-tester${RESET}   cargo install from private Git repo%s\n" "$step" "$browser_note"
            step=$((step + 1))
        fi
        if [[ $DO_INSTALL_ENDPOINT -eq 1 ]]; then
            printf "    %s. ${BOLD}Install networker-endpoint${RESET} cargo install from private Git repo\n" "$step"
        fi
        echo ""
        print_dim "Repository:  $REPO_SSH"
        print_dim "Source code is compiled locally — no pre-built binaries are downloaded."
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
            1) return 0 ;;
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

    # Install method (only offered when release mode is available)
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

    # SSH check + Rust install + git install – only relevant in source mode
    if [[ "$INSTALL_METHOD" == "source" ]]; then
        # git install (only offered when git is absent and pkg manager is available)
        if [[ $GIT_AVAILABLE -eq 0 ]]; then
            if [[ -n "$PKG_MGR" ]]; then
                if ask_yn "git is not installed — install it via ${PKG_MGR}?" "y"; then
                    DO_GIT_INSTALL=1
                else
                    DO_GIT_INSTALL=0
                    echo ""
                    print_warn "Skipping git install — cargo will use its built-in libgit2 for SSH."
                    print_warn "If SSH authentication fails, install git manually:"
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

        # Chrome install (only offered when Chrome is absent)
        if [[ $CHROME_AVAILABLE -eq 0 ]]; then
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

    # Component selection (always shown)
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

    # ssh -T always exits 1 on GitHub (no shell access by design).
    # </dev/null prevents ssh from consuming the curl pipe.
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
        brew)
            brew install git
            ;;
        apt-get)
            sudo apt-get update -qq && sudo apt-get install -y git
            ;;
        dnf)
            sudo dnf install -y git
            ;;
        pacman)
            sudo pacman -S --noconfirm git
            ;;
        zypper)
            sudo zypper install -y git
            ;;
        apk)
            sudo apk add git
            ;;
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
            ;;
        dnf)    sudo dnf install -y chromium ;;
        pacman) sudo pacman -S --noconfirm chromium ;;
        zypper) sudo zypper install -y chromium ;;
        apk)    sudo apk add chromium ;;
        *)
            print_warn "Unknown package manager: $PKG_MGR"
            print_warn "Install Chrome manually from: https://www.google.com/chrome/"
            return
            ;;
    esac

    # Re-detect after install
    CHROME_PATH="$(detect_chrome)"
    if [[ -n "$CHROME_PATH" ]]; then
        CHROME_AVAILABLE=1
        print_ok "Chrome/Chromium ready: $CHROME_PATH"
    else
        print_warn "Chrome/Chromium installed but not yet detectable in standard paths."
        print_warn "browser probe will be compiled in; set NETWORKER_CHROME_PATH if needed."
        CHROME_AVAILABLE=1  # Compile with feature; user can set path at runtime
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

    # Pre-flight: warn if no C linker is available (cargo needs cc/gcc/clang to link)
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

    # CARGO_NET_GIT_FETCH_WITH_CLI=true delegates git operations to the system
    # git binary (rather than libgit2), reliably picking up the SSH agent.
    # Only set when git is on PATH; otherwise cargo uses its built-in libgit2.
    # --force rebuilds unconditionally even when cargo's SHA cache is current.
    # --features browser is added only when Chrome/Chromium is available.
    # </dev/null prevents cargo reading the curl pipe for interactive prompts.
    local features_arg=""
    if [[ $CHROME_AVAILABLE -eq 1 ]]; then
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

# ── Completion summary ────────────────────────────────────────────────────────
display_completion() {
    echo ""
    echo "${BOLD}══════════════════════════════════════════════════════════${RESET}"
    echo "${GREEN}${BOLD}  Installation complete!${RESET}"
    echo "${BOLD}══════════════════════════════════════════════════════════${RESET}"
    echo ""

    if ! echo ":${PATH}:" | grep -q ":${INSTALL_DIR}:"; then
        print_warn "${INSTALL_DIR} is not in your shell PATH."
        echo ""
        echo "  Run now (activates for this terminal session):"
        echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
        echo ""
        echo "  Make permanent:"
        echo "    echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.bashrc   # bash"
        echo "    echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc    # zsh"
        echo ""
    fi

    if [[ $DO_INSTALL_TESTER -eq 1 ]]; then
        echo "  ${BOLD}networker-tester${RESET} quick start:"
        echo "    networker-tester --help"
        echo "    networker-tester --target http://localhost:8080/health --modes http1 --runs 3"
        echo ""
    fi

    if [[ $DO_INSTALL_ENDPOINT -eq 1 ]]; then
        echo "  ${BOLD}networker-endpoint${RESET} quick start:"
        echo "    networker-endpoint"
        echo "    # Listens on :8080 HTTP, :8443 HTTPS/H2/H3, :9998 UDP throughput, :9999 UDP echo"
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

    if [[ "$INSTALL_METHOD" == "release" ]]; then
        mkdir -p "$INSTALL_DIR"
        if [[ $DO_INSTALL_TESTER -eq 1 ]]; then
            step_download_release "networker-tester"
        fi
        if [[ $DO_INSTALL_ENDPOINT -eq 1 ]]; then
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
        step_ensure_cargo_env
        if [[ $DO_INSTALL_TESTER -eq 1 ]]; then
            step_cargo_install "networker-tester"
        fi
        if [[ $DO_INSTALL_ENDPOINT -eq 1 ]]; then
            step_cargo_install "networker-endpoint"
        fi
    fi

    display_completion
}

main "$@"
