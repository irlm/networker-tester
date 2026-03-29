#!/usr/bin/env bash
# Deploy the Java reference API to a target VM.
#
# Usage:
#   ./deploy.sh <user@host>
#
# Prerequisites:
#   - SSH access to the target
#   - server.jar built locally (run build.sh first)
#   - cert.pem + key.pem in /opt/bench/ on the target (or set BENCH_CERT_DIR)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TARGET="${1:?Usage: deploy.sh <user@host>}"
REMOTE_DIR="/opt/bench/java"

if [[ ! -f "$SCRIPT_DIR/server.jar" ]]; then
    echo "server.jar not found — running build.sh first"
    bash "$SCRIPT_DIR/build.sh"
fi

echo "==> Installing JDK 21 on $TARGET ..."
ssh "$TARGET" << 'INSTALL_EOF'
set -euo pipefail
if java -version 2>&1 | grep -q '"21'; then
    echo "JDK 21 already installed"
else
    if command -v apt-get >/dev/null 2>&1; then
        sudo apt-get update -qq
        sudo apt-get install -y -qq openjdk-21-jdk-headless < /dev/null
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y java-21-openjdk-headless < /dev/null
    elif command -v yum >/dev/null 2>&1; then
        sudo yum install -y java-21-openjdk-headless < /dev/null
    else
        echo "ERROR: unsupported package manager" >&2
        exit 1
    fi
fi
java -version
INSTALL_EOF

echo "==> Deploying server.jar to $TARGET:$REMOTE_DIR ..."
ssh "$TARGET" "sudo mkdir -p $REMOTE_DIR && sudo chown \$(whoami) $REMOTE_DIR"
scp "$SCRIPT_DIR/server.jar" "$TARGET:$REMOTE_DIR/server.jar"

echo "==> Stopping any existing Java server ..."
ssh "$TARGET" "pkill -f 'java.*server.jar' || true"

echo "==> Starting Java HTTPS server ..."
ssh "$TARGET" << REMOTE_EOF
set -euo pipefail
cd $REMOTE_DIR
nohup java -jar server.jar > server.log 2>&1 &
echo "PID: \$!"

# Wait for health check
for i in \$(seq 1 30); do
    if curl -sk https://localhost:8443/health >/dev/null 2>&1; then
        echo "Server ready"
        curl -sk https://localhost:8443/health
        exit 0
    fi
    sleep 0.5
done
echo "ERROR: server did not become healthy within 15s" >&2
tail -20 server.log >&2
exit 1
REMOTE_EOF

echo "==> Java reference API deployed to $TARGET"
