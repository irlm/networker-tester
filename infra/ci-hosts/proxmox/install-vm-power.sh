#!/usr/bin/env bash
# infra/ci-hosts/proxmox/install-vm-power.sh — install the CI VM power scripts
# on a Proxmox node: the idle stopper, its timer, and the start counterpart.
#
# Run as root ON the Proxmox node, or point it at one over ssh:
#
#   scp -r infra/ci-hosts/proxmox root@pve:/tmp/ && ssh root@pve /tmp/proxmox/install-vm-power.sh
#   ./install-vm-power.sh --uninstall     stop + disable the timer, leave the scripts
#   ./install-vm-power.sh --dry-run       print what it would do
#
# Idempotent: re-running overwrites the scripts and reloads the units. It never
# starts or stops a VM itself — that is what the two scripts are for.
#
# Why this exists: both scripts lived only on the node, hand-installed and
# untracked, so a node rebuild silently lost the whole power policy — and
# nobody could see that idle-stop had no counterpart (v0.28.313).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DRY_RUN=0
UNINSTALL=0
for a in "$@"; do
  case "$a" in
    --dry-run|-n) DRY_RUN=1 ;;
    --uninstall)  UNINSTALL=1 ;;
    -h|--help)    sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//;$d'; exit 0 ;;
    *) echo "unknown argument: $a" >&2; exit 2 ;;
  esac
done

log() { echo "[install-vm-power] $*"; }
run() { if [ "$DRY_RUN" = "1" ]; then echo "  would: $*"; else "$@"; fi; }

[ "$(id -u)" = "0" ] || { echo "must run as root on the Proxmox node" >&2; exit 1; }
command -v qm >/dev/null || { echo "no 'qm' — this is not a Proxmox node" >&2; exit 1; }

if [ "$UNINSTALL" = "1" ]; then
  log "disabling the idle-stop timer (scripts are left in place)"
  run systemctl disable --now ci-vm-idle-stop.timer || true
  log "done — VMs will no longer be stopped automatically"
  exit 0
fi

log "installing scripts into /usr/local/sbin"
for s in ci-vm-idle-stop ci-vm-start; do
  [ -f "$HERE/$s" ] || { echo "missing $HERE/$s" >&2; exit 1; }
  run install -m 0755 "$HERE/$s" "/usr/local/sbin/$s"
done

# Never clobber a node's tuned thresholds; ship the example instead.
if [ ! -e /etc/default/ci-vm-idle-stop ]; then
  log "seeding /etc/default/ci-vm-idle-stop from the example"
  run install -m 0644 "$HERE/ci-vm-idle-stop.default.example" /etc/default/ci-vm-idle-stop
else
  log "/etc/default/ci-vm-idle-stop exists — left as is"
fi

log "installing systemd units"
for u in ci-vm-idle-stop.service ci-vm-idle-stop.timer; do
  run install -m 0644 "$HERE/systemd/$u" "/etc/systemd/system/$u"
done
run systemctl daemon-reload
run systemctl enable --now ci-vm-idle-stop.timer

log "installed. Useful commands:"
echo "  ci-vm-start --dry-run          # what would start, with the RAM guard applied"
echo "  ci-vm-start 3                  # start up to 3 stopped CI VMs"
echo "  ci-vm-idle-stop                # run the stopper once, now"
echo "  systemctl list-timers ci-vm-idle-stop.timer"
