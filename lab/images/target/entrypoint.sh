#!/usr/bin/env bash
# Target entrypoint — an endpoint VM in a container:
#   1. networker-endpoint (8080 http, 8443 https/h2/h3, 9999 udp echo,
#      9998 udp throughput, 9997 stamp) — the same defaults a VM runs.
#   2. Optionally the comparison-proxy stack that `install.sh --setup-stack`
#      installed at image build time (TARGET_STACK=nginx|caddy|apache|haproxy|traefik),
#      started through the systemctl shim exactly the way the installer does.
set -euo pipefail

STACK="${TARGET_STACK:-none}"
ENDPOINT_ARGS="${ENDPOINT_ARGS:-}"
# Fresh process-tracking state for the systemctl shim (build-time pids are gone).
rm -rf /run/lab-systemctl; mkdir -p /run/lab-systemctl

term() { echo "lab-target: stopping"; kill "${EP_PID:-0}" 2>/dev/null || true; systemctl stop "$SVC" 2>/dev/null || true; exit 0; }
trap term TERM INT

echo "lab-target: $(hostname) stack=${STACK} endpoint=$(/usr/local/bin/networker-endpoint --version 2>/dev/null | head -1)"
# shellcheck disable=SC2086
/usr/local/bin/networker-endpoint ${ENDPOINT_ARGS} &
EP_PID=$!

# Wait for the endpoint's own health before starting a proxy in front of it.
for _ in $(seq 1 60); do
  curl -fsS -m 1 http://127.0.0.1:8080/health >/dev/null 2>&1 && break
  sleep 0.5
done

SVC=""
case "$STACK" in
  none|rust) ;;
  nginx)   SVC=nginx ;;
  caddy)   SVC=networker-caddy ;;
  apache)  SVC=apache2 ;;
  haproxy) SVC=networker-haproxy ;;
  traefik) SVC=networker-traefik ;;
  *) echo "lab-target: unknown TARGET_STACK '$STACK'" >&2; exit 2 ;;
esac
if [ -n "$SVC" ]; then
  systemctl start "$SVC" || { echo "lab-target: failed to start $SVC" >&2; exit 1; }
  echo "lab-target: $SVC started (stack ${STACK})"
fi

# Keep the container alive as long as the endpoint lives; if the proxy dies,
# say so loudly (a VM would have systemd restart it — surfacing it here is the
# more useful behaviour for a lab).
while kill -0 "$EP_PID" 2>/dev/null; do
  if [ -n "$SVC" ] && ! systemctl is-active "$SVC" >/dev/null 2>&1; then
    echo "lab-target: WARN $SVC is not running — restarting" >&2
    systemctl start "$SVC" || true
  fi
  sleep 5
done
echo "lab-target: networker-endpoint exited" >&2
exit 1
