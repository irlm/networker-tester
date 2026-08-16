#!/usr/bin/env bash
# Runner entrypoint — the container analogue of the systemd unit the cloud-init
# bootstrap installs (CloudInitScripts.LinuxTemplate). Optional WAN emulation
# via LAB_NETEM (e.g. "delay 40ms 5ms loss 0.1%") — needs cap NET_ADMIN, which
# lab/docker-compose.yml grants runners.
set -euo pipefail

: "${AGENT_DASHBOARD_URL:?AGENT_DASHBOARD_URL is required}"
: "${AGENT_API_KEY:?AGENT_API_KEY is required}"

if [ -n "${LAB_NETEM:-}" ]; then
  if tc qdisc replace dev eth0 root netem ${LAB_NETEM} 2>/dev/null; then
    echo "lab-runner: applied netem on eth0: ${LAB_NETEM}"
  else
    echo "lab-runner: WARN could not apply netem '${LAB_NETEM}' (missing NET_ADMIN?)" >&2
  fi
fi

echo "lab-runner: $(hostname) → ${AGENT_DASHBOARD_URL} (tester $(/usr/local/bin/networker-tester --version 2>/dev/null | head -1))"
# exec so the agent is PID 1 and receives SIGTERM from `docker stop` directly.
exec /usr/local/bin/networker-agent
