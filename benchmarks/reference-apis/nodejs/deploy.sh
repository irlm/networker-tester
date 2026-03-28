#!/usr/bin/env bash
# deploy.sh — Install Node.js v22 (if needed) and start the reference API.
# Usage: deploy.sh [--cert-dir /opt/bench] [--port 8443]
set -euo pipefail

CERT_DIR="/opt/bench"
PORT="8443"
NODE_MAJOR="22"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Parse flags ─────────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --cert-dir) CERT_DIR="$2"; shift 2 ;;
    --port)     PORT="$2";     shift 2 ;;
    *)          echo "Unknown flag: $1" >&2; exit 1 ;;
  esac
done

# ── Install Node.js v22 if not present ──────────────────────────────────────
install_node() {
  if command -v node >/dev/null 2>&1; then
    local current
    current="$(node --version | sed 's/^v//' | cut -d. -f1)"
    if [ "$current" -ge "$NODE_MAJOR" ]; then
      echo "Node.js $(node --version) already installed — skipping."
      return 0
    fi
  fi

  echo "Installing Node.js v${NODE_MAJOR}..."

  if [ -f /etc/os-release ]; then
    # Debian / Ubuntu
    if command -v apt-get >/dev/null 2>&1; then
      curl -fsSL "https://deb.nodesource.com/setup_${NODE_MAJOR}.x" | bash - < /dev/null
      apt-get install -y nodejs < /dev/null
    # RHEL / Fedora / Amazon Linux
    elif command -v dnf >/dev/null 2>&1; then
      curl -fsSL "https://rpm.nodesource.com/setup_${NODE_MAJOR}.x" | bash - < /dev/null
      dnf install -y nodejs < /dev/null
    elif command -v yum >/dev/null 2>&1; then
      curl -fsSL "https://rpm.nodesource.com/setup_${NODE_MAJOR}.x" | bash - < /dev/null
      yum install -y nodejs < /dev/null
    else
      echo "Unsupported package manager" >&2; exit 1
    fi
  elif [ "$(uname)" = "Darwin" ]; then
    if command -v brew >/dev/null 2>&1; then
      brew install "node@${NODE_MAJOR}" < /dev/null
    else
      echo "Homebrew not found — install Node.js manually" >&2; exit 1
    fi
  else
    echo "Unsupported OS" >&2; exit 1
  fi

  echo "Installed Node.js $(node --version)"
}

install_node

# ── Copy source files to deployment directory ───────────────────────────────
DEPLOY_DIR="/opt/bench/nodejs"
mkdir -p "$DEPLOY_DIR"
cp "$SCRIPT_DIR/server.js" "$DEPLOY_DIR/"
cp "$SCRIPT_DIR/package.json" "$DEPLOY_DIR/"

# ── Stop any previous instance ──────────────────────────────────────────────
pkill -f "node.*server\\.js" 2>/dev/null || true
sleep 1

# ── Start the server ────────────────────────────────────────────────────────
echo "Starting Node.js reference API on port ${PORT}..."
cd "$DEPLOY_DIR"
BENCH_CERT_DIR="$CERT_DIR" PORT="$PORT" nohup node server.js > /var/log/bench-nodejs.log 2>&1 &
echo "PID: $!"

# ── Wait for health check ──────────────────────────────────────────────────
for i in $(seq 1 10); do
  if curl -sk "https://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
    echo "Health check passed."
    exit 0
  fi
  sleep 1
done

echo "WARNING: health check did not pass within 10 seconds" >&2
exit 1
