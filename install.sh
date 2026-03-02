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
SCRIPT_VERSION="0.12.12"
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
}

display_system_info() {
    print_section "System Information"
    echo ""
    printf "    %-22s %s\n" "OS:"           "$SYS_OS"
    printf "    %-22s %s\n" "Architecture:" "$SYS_ARCH"
    printf "    %-22s %s\n" "Shell:"        "$SYS_SHELL"
    printf "    %-22s %s\n" "Home:"         "$HOME"
    printf "    %-22s %s\n" "Rust / cargo:" "$RUST_VER"
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

        if [[ $DO_INSTALL_TESTER -eq 1 ]]; then
            printf "    %s. ${BOLD}Install networker-tester${RESET}   cargo install from private Git repo\n" "$step"
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

    # SSH check + Rust install – only relevant in source mode
    if [[ "$INSTALL_METHOD" == "source" ]]; then
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
    echo ""

    # CARGO_NET_GIT_FETCH_WITH_CLI=true delegates git operations to the system
    # git binary (rather than libgit2), reliably picking up the SSH agent.
    # --force rebuilds unconditionally even when cargo's SHA cache is current.
    # </dev/null prevents cargo reading the curl pipe for interactive prompts.
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
        cargo install --git "$REPO_SSH" "$binary" --locked --force </dev/null

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
