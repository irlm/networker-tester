#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# Networker Tester – Unix/macOS installer
#
# Installs either the diagnostic CLIENT or the test ENDPOINT from the private
# GitHub repo using SSH (your existing SSH key is used – no token required).
#
# Usage:
#   curl -fsSL <raw-gist-url>/install.sh | bash -s -- tester
#   curl -fsSL <raw-gist-url>/install.sh | bash -s -- endpoint
#
#   Or download and run directly:
#   bash install.sh tester
#   bash install.sh endpoint
# ──────────────────────────────────────────────────────────────────────────────
set -euo pipefail

COMPONENT="${1:-}"
REPO_SSH="ssh://git@github.com/irlm/networker-tester"

# ── Validate argument ──────────────────────────────────────────────────────────
usage() {
    echo "Usage: $0 [tester|endpoint]"
    echo ""
    echo "  tester   – installs networker-tester (the diagnostic CLI client)"
    echo "  endpoint – installs networker-endpoint (the target test server)"
    exit 1
}

case "${COMPONENT}" in
    tester)   BINARY="networker-tester"   ;;
    endpoint) BINARY="networker-endpoint" ;;
    *)        usage ;;
esac

# ── Colours (skip if not a terminal) ──────────────────────────────────────────
if [ -t 1 ]; then
    BOLD='\033[1m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RESET='\033[0m'
else
    BOLD=''; GREEN=''; YELLOW=''; RESET=''
fi

info()    { echo -e "${BOLD}[info]${RESET}  $*"; }
success() { echo -e "${GREEN}[ok]${RESET}    $*"; }
warn()    { echo -e "${YELLOW}[warn]${RESET}  $*"; }

# ── Check SSH access to GitHub ────────────────────────────────────────────────
# `ssh -T git@github.com` always exits with code 1 (GitHub provides no shell
# access), so capture the output with || true and grep the message separately.
#
# </dev/null is required when this script is piped from curl: without it, ssh
# inherits bash's stdin (the curl pipe) and consumes the remaining script,
# causing bash to hit EOF and exit silently after this block.
info "Checking SSH access to GitHub..."
SSH_OUT=$(ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new \
              -o ConnectTimeout=10 -T git@github.com </dev/null 2>&1 || true)
if ! printf '%s' "${SSH_OUT}" | grep -q "successfully authenticated"; then
    echo ""
    echo "  SSH authentication to GitHub failed."
    echo "  Make sure your SSH key is loaded and has access to the private repo."
    echo "  Test manually: ssh -T git@github.com"
    exit 1
fi
success "SSH access confirmed"

# ── Ensure Rust / cargo ───────────────────────────────────────────────────────
if ! command -v cargo &>/dev/null; then
    warn "cargo not found – installing Rust via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path
    # shellcheck source=/dev/null
    source "${HOME}/.cargo/env"
    success "Rust installed"
else
    RUST_VER=$(rustc --version)
    info "Using existing ${RUST_VER}"
fi

# ── Build and install ─────────────────────────────────────────────────────────
# </dev/null prevents cargo from reading the curl pipe for interactive prompts
# (same reason as the ssh call above).
info "Installing ${BINARY} (this compiles from source – may take a few minutes)..."
cargo install --git "${REPO_SSH}" --bin "${BINARY}" --locked </dev/null

echo ""
INSTALLED_PATH=$(command -v "${BINARY}" 2>/dev/null || echo "(not in PATH)")
success "${BINARY} installed → ${INSTALLED_PATH}"

if [ "${COMPONENT}" = "tester" ]; then
    echo ""
    echo "  Quick test:"
    echo "    networker-tester --help"
    echo "    networker-tester --target http://localhost:8080/health --modes http1 --runs 3"
else
    echo ""
    echo "  Start the endpoint:"
    echo "    networker-endpoint"
    echo "  (listens on :8080 HTTP, :8443 HTTPS, :9999 UDP)"
fi
