#!/usr/bin/env bash
# Regenerate the Rust↔C# contract golden fixture from the REAL tester.
#
# tests/Networker.Tests/fixtures/tester-golden.json is the payload
# ContractRoundTripTests deserializes to guard the JSON seam between the Rust
# probe engine and the C# control plane/agent. It must never be hand-typed:
# this script captures actual `networker-tester --json-stdout` output by
# probing a local in-process `networker-endpoint` (the same pattern as
# crates/networker-tester/tests/integration.rs), so the fixture is exactly
# what the tester emits at the current schema.
#
# Run from the repo root whenever the tester's TestRun JSON schema changes
# (new fields are additive-safe; the C# tests assert structure, not exact
# timing values):
#
#   ./scripts/regenerate-contract-golden.sh
#
# Requires: cargo (builds networker-endpoint + networker-tester), python3
# (pretty-printing). No network access — everything runs on 127.0.0.1.
set -euo pipefail

cd "$(dirname "$0")/.."
FIXTURE="tests/Networker.Tests/fixtures/tester-golden.json"

echo "building networker-endpoint + networker-tester..."
cargo build -q -p networker-endpoint -p networker-tester

# Free ports (bind :0, read back the assigned port).
free_port() {
  python3 - <<'EOF'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
EOF
}
HTTP_PORT="$(free_port)"
HTTPS_PORT="$(free_port)"
UDP_PORT="$(free_port)"
UDP_TP_PORT="$(free_port)"

echo "starting networker-endpoint (http :${HTTP_PORT}, https :${HTTPS_PORT})..."
./target/debug/networker-endpoint \
  --http-port "${HTTP_PORT}" \
  --https-port "${HTTPS_PORT}" \
  --udp-port "${UDP_PORT}" \
  --udp-throughput-port "${UDP_TP_PORT}" \
  < /dev/null &
ENDPOINT_PID=$!
trap 'kill "${ENDPOINT_PID}" 2>/dev/null || true' EXIT

# Wait for the endpoint to accept connections.
for _ in $(seq 1 50); do
  if curl -sf "http://127.0.0.1:${HTTP_PORT}/health" > /dev/null 2>&1; then
    break
  fi
  sleep 0.2
done

# One http1 attempt over HTTPS (self-signed → --insecure) so the golden
# exercises every phase the C# contract models: DNS → TCP → TLS → HTTP.
# Logs go to stderr; stdout is the pure JSON payload.
echo "probing https://localhost:${HTTPS_PORT}/health (modes=http1, runs=1)..."
RAW="$(./target/debug/networker-tester \
  --target "https://localhost:${HTTPS_PORT}/health" \
  --modes http1 \
  --runs 1 \
  --insecure \
  --json-stdout \
  < /dev/null)"

# Pretty-print for reviewable diffs (stable key order as emitted by serde).
printf '%s' "${RAW}" | python3 -m json.tool > "${FIXTURE}"

echo "wrote ${FIXTURE}:"
head -20 "${FIXTURE}"
echo "..."
echo "done — commit the regenerated fixture together with the schema change."
