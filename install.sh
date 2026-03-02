#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – Unix/macOS interactive installer (rustup-style)
#
# Usage (piped from curl):
#   curl -fsSL <raw-gist-url>/install.sh | bash -s -- [OPTIONS] [tester|endpoint|both]
#
# Usage (downloaded):
#   bash install.sh [OPTIONS] [tester|endpoint|both]
#
# Options:
#   -y, --yes           Non-interactive: accept all defaults
#   --skip-ssh-check    Skip the GitHub SSH connectivity test
#   --skip-rust         Skip Rust installation (assume cargo is available)
#   -h, --help          Show this help message
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO_SSH="ssh://git@github.com/irlm/networker-tester"
SCRIPT_VERSION="0.12.11"

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

Options:
  -y, --yes           Non-interactive: accept all defaults
  --skip-ssh-check    Skip the GitHub SSH connectivity test
  --skip-rust         Skip Rust installation (assume cargo is available)
  -h, --help          Show this help message

Examples:
  curl -fsSL <url>/install.sh | bash -s -- tester
  bash install.sh -y endpoint
  bash install.sh --skip-rust both
EOF
}

# ── Script-level state ─────────────────────────────────────────────────────────
# (all globals are set by parse_args / discover_system / customize_flow)
COMPONENT="both"
AUTO_YES=0
SKIP_SSH=0
SKIP_RUST=0
DO_SSH_CHECK=1
DO_RUST_INSTALL=0   # determined after checking whether cargo already exists
DO_INSTALL_TESTER=1
DO_INSTALL_ENDPOINT=1
RUST_VER=""
RUST_EXISTS=0
SYS_OS=""
SYS_ARCH=""
SYS_SHELL=""
SYS_HOME=""
STEP_NUM=0

# ── Argument parsing ──────────────────────────────────────────────────────────
parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            tester|endpoint|both)
                COMPONENT="$1" ;;
            -y|--yes)
                AUTO_YES=1 ;;
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

# ── System discovery ──────────────────────────────────────────────────────────
discover_system() {
    SYS_OS="$(uname -s 2>/dev/null || echo "unknown")"
    SYS_ARCH="$(uname -m 2>/dev/null || echo "unknown")"
    SYS_SHELL="${SHELL:-unknown}"
    SYS_HOME="$HOME"

    if command -v cargo &>/dev/null; then
        RUST_VER="$(rustc --version 2>/dev/null || echo "unknown version")"
        RUST_EXISTS=1
    else
        RUST_VER="not installed"
        RUST_EXISTS=0
    fi

    # Propose installing Rust only if absent and not explicitly skipped
    if [[ $RUST_EXISTS -eq 0 && $SKIP_RUST -eq 0 ]]; then
        DO_RUST_INSTALL=1
    fi
}

display_system_info() {
    print_section "System Information"
    echo ""
    printf "    %-22s %s\n" "OS:"           "$SYS_OS"
    printf "    %-22s %s\n" "Architecture:" "$SYS_ARCH"
    printf "    %-22s %s\n" "Shell:"        "$SYS_SHELL"
    printf "    %-22s %s\n" "Home:"         "$SYS_HOME"
    printf "    %-22s %s\n" "Rust / cargo:" "$RUST_VER"
    printf "    %-22s %s\n" "Install to:"   "${HOME}/.cargo/bin/"
}

display_plan() {
    print_section "Installation Plan"
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
        step=$((step + 1))
    fi

    echo ""
    print_dim "Repository:  $REPO_SSH"
    print_dim "Source code is compiled locally — no pre-built binaries are downloaded."
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

# ── Yes/No helper (reads from /dev/tty even when stdin is a curl pipe) ────────
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

# ── Customise flow ────────────────────────────────────────────────────────────
customize_flow() {
    print_section "Customize Installation"
    echo ""

    # SSH check
    if ask_yn "Run SSH connectivity check for GitHub?" "y"; then
        DO_SSH_CHECK=1
    else
        DO_SSH_CHECK=0
    fi

    # Rust install – only relevant when Rust is not already present
    if [[ $RUST_EXISTS -eq 0 ]]; then
        echo ""
        if ask_yn "Install Rust via rustup (sh.rustup.rs)?" "y"; then
            DO_RUST_INSTALL=1
        else
            DO_RUST_INSTALL=0
            echo ""
            print_warn "Rust is not installed — cargo must be on PATH before proceeding."
            echo "  Install manually: https://rustup.rs"
            echo "  Then re-run this script with --skip-rust"
        fi
    fi

    # Component selection
    echo ""
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

    # Show revised plan and ask for confirmation
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

# ── Step execution helpers ────────────────────────────────────────────────────
next_step() {
    STEP_NUM=$((STEP_NUM + 1))
    print_step_header "$STEP_NUM" "$@"
}

step_ssh_check() {
    next_step "Verify GitHub SSH access"
    print_info "Connecting to git@github.com..."

    # ssh -T always exits 1 on GitHub (by design — no shell access).
    # </dev/null prevents ssh from consuming the curl pipe when stdin is piped.
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
    print_info "Downloading rustup from https://sh.rustup.rs ..."
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
    # Ensure ~/.cargo/bin is on PATH (may need sourcing after a fresh install)
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

    # CARGO_NET_GIT_FETCH_WITH_CLI=true makes cargo use the system git binary
    # instead of libgit2, which reliably picks up the SSH agent.
    # --force rebuilds unconditionally even when cargo's SHA cache is current.
    # </dev/null prevents cargo from reading the curl pipe for interactive prompts.
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
        cargo install --git "$REPO_SSH" "$binary" --locked --force </dev/null

    local installed_path="${HOME}/.cargo/bin/${binary}"
    local installed_ver
    installed_ver="$("$installed_path" --version 2>/dev/null || echo "unknown")"
    echo ""
    print_ok "$binary installed → $installed_path  ($installed_ver)"
}

# ── Completion summary ────────────────────────────────────────────────────────
display_completion() {
    echo ""
    echo "${BOLD}══════════════════════════════════════════════════════════${RESET}"
    echo "${GREEN}${BOLD}  Installation complete!${RESET}"
    echo "${BOLD}══════════════════════════════════════════════════════════${RESET}"
    echo ""

    # PATH notice – check the parent shell's PATH (not the env we may have sourced)
    if ! echo ":${PATH}:" | grep -q ":${HOME}/.cargo/bin:"; then
        print_warn "~/.cargo/bin is not in your shell PATH."
        echo ""
        echo "  Run now (activates for this terminal session):"
        echo "    . \"\$HOME/.cargo/env\""
        echo ""
        echo "  Make permanent (add to your shell profile):"
        echo "    echo '. \"\$HOME/.cargo/env\"' >> ~/.bashrc   # bash"
        echo "    echo '. \"\$HOME/.cargo/env\"' >> ~/.zshrc    # zsh"
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

    display_completion
}

main "$@"
