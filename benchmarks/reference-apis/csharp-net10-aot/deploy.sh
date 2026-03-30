#!/usr/bin/env bash
# Deploy the C# .NET 10 AOT reference API to a target VM.
# Usage: ./deploy.sh <VM_IP>
set -euo pipefail

VM_IP="${1:?Usage: deploy.sh <VM_IP>}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SHARED_DIR="$SCRIPT_DIR/../../shared"

# Verify the AOT binary exists
if [ ! -f "$SCRIPT_DIR/publish/csharp-net10-aot" ]; then
    echo "ERROR: AOT binary not found. Run build.sh first (on the target OS)."
    exit 1
fi

echo "Deploying C# .NET 10 AOT to $VM_IP..."

# Create target directory
ssh "$VM_IP" "mkdir -p /opt/bench"

# Copy the single native binary (no runtime needed)
scp "$SCRIPT_DIR/publish/csharp-net10-aot" "$VM_IP:/opt/bench/server"

# Copy TLS certificates
scp "$SHARED_DIR/cert.pem" "$VM_IP:/opt/bench/"
scp "$SHARED_DIR/key.pem" "$VM_IP:/opt/bench/"

# Make executable and start
ssh "$VM_IP" "chmod +x /opt/bench/server && cd /opt/bench && nohup ./server > server.log 2>&1 &"

echo "Deployed. Waiting for health check..."
sleep 2
if curl -sk "https://$VM_IP:8443/health" | grep -q '"status":"ok"'; then
    echo "Health check passed."
else
    echo "WARNING: Health check did not return expected response."
fi
