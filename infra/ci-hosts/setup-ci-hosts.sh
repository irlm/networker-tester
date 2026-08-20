#!/usr/bin/env bash
# infra/ci-hosts/setup-ci-hosts.sh — THE entry point for the self-hosted CI
# hosts (docs/self-hosted-ci.md). One interactive, idempotent orchestrator that
# stands every CI host up end to end from any Linux/macOS box: needs only
# bash 4+, ssh, curl, jq (gh optional — reused for the PAT when logged in).
#
# A "CI host" is the machine GitHub calls a self-hosted runner; "runner" in
# this repo means a tester VM, so nothing here is named runner except GitHub's
# own API paths and the actions-runner package.
#
#   setup-ci-hosts.sh [setup]            full flow (re-runnable; finished work is skipped)
#   setup-ci-hosts.sh status             runner table from the API + VM/ssh reachability
#   setup-ci-hosts.sh add-linux N        clone N more Linux CI hosts
#   setup-ci-hosts.sh destroy            deregister + `qm destroy` every CI host VM (asks first)
#   setup-ci-hosts.sh verify             list CI hosts, offer the smoke workflow, print next steps
#   --non-interactive                    answer everything from infra/ci-hosts/ci-hosts.env
#   --env FILE                           alternative env file (default: ci-hosts.env next to this script)
#
# Flow: 1 GitHub (repo + PAT) → 2 Proxmox (template, N Linux VMs via cloud-init
# running linux/install-ci-host.sh, wait until online) → 3 Mac mini over ssh
# (macos/install-ci-host.sh) → 4 Windows (VM from ISO; install-ci-host.ps1 over
# ssh when OpenSSH is enabled, else the manual steps) → 5 verify + next steps.
#
# The PAT: prompted with hidden input (or `gh auth token`), held in memory,
# shipped to each host over ssh into a root-only file (/etc/ci-host/token,
# 0600). It is never written into this repo and never echoed.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$HERE/ci-hosts.env"
NON_INTERACTIVE=0
CMD=setup
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --non-interactive|-y) NON_INTERACTIVE=1 ;;
    --env) ENV_FILE="$2"; shift ;;
    --env=*) ENV_FILE="${1#--env=}" ;;
    -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    setup|status|add-linux|destroy|verify) CMD="$1" ;;
    *) ARGS+=("$1") ;;
  esac
  shift
done

# ── ui ───────────────────────────────────────────────────────────────────────
if [ -t 1 ]; then B=$'\033[1m' C=$'\033[1;36m' G=$'\033[1;32m' Y=$'\033[1;33m' R=$'\033[1;31m' N=$'\033[0m'; else B='' C='' G='' Y='' R='' N=''; fi
say()  { printf '%s\n' "$*"; }
step() { printf '\n%s== %s ==%s\n' "$C" "$*" "$N"; }
ok()   { printf '%s✓%s %s\n' "$G" "$N" "$*"; }
warn() { printf '%s!%s %s\n' "$Y" "$N" "$*"; }
die()  { printf '%sERROR:%s %s\n' "$R" "$N" "$*" >&2; exit 1; }

ask() { # ask VAR "prompt" "default" [optional] — keeps an existing value as the default
  # "optional": an empty answer is valid (runner group, VLAN tag, …), so
  # --non-interactive accepts it instead of dying.
  local var="$1" prompt="$2" def="${3:-}" opt="${4:-}" cur val
  cur="${!var:-}"; [ -n "$cur" ] && def="$cur"
  if [ "$NON_INTERACTIVE" = 1 ]; then
    [ -n "$def" ] || [ "$opt" = optional ] || die "--non-interactive: $var is not set in $ENV_FILE"
    printf -v "$var" '%s' "$def"; return
  fi
  if [ -n "$def" ]; then read -r -p "$prompt [$def]: " val; else read -r -p "$prompt: " val; fi
  printf -v "$var" '%s' "${val:-$def}"
}
ask_secret() { # ask_secret VAR "prompt"
  local var="$1" prompt="$2" val
  [ "$NON_INTERACTIVE" = 1 ] && die "--non-interactive: export $var in the environment"
  read -r -s -p "$prompt: " val; echo
  [ -n "$val" ] || die "empty input"
  printf -v "$var" '%s' "$val"
}
confirm() { # confirm "question" [default y|n]
  local q="$1" def="${2:-n}" a
  if [ "$NON_INTERACTIVE" = 1 ]; then [ "$def" = y ]; return; fi
  read -r -p "$q [$( [ "$def" = y ] && echo Y/n || echo y/N )]: " a
  a="${a:-$def}"; case "$a" in y|Y|yes) return 0 ;; *) return 1 ;; esac
}
need() { for t in "$@"; do command -v "$t" >/dev/null 2>&1 || die "missing tool: $t"; done; }
need bash ssh scp curl jq base64
[ "${BASH_VERSINFO[0]}" -ge 4 ] || die "bash 4+ required (macOS: brew install bash)"

# ── config ───────────────────────────────────────────────────────────────────
# shellcheck disable=SC1090 # the env file path is user-supplied by design
[ -f "$ENV_FILE" ] && . "$ENV_FILE"
GH_REPO="${GH_REPO:-}"; RUNNER_GROUP="${RUNNER_GROUP:-}"
# Defaults match the owner's node (pve.home.lab: `ssh pve` alias, local-lvm,
# vmbr0 untagged, VMIDs 300-310 free, 9000 = the OLD 22.04 template).
PVE_HOST="${PVE_HOST:-pve}"; PVE_STORAGE="${PVE_STORAGE:-local-lvm}"; PVE_SNIPPET_STORAGE="${PVE_SNIPPET_STORAGE:-local}"
PVE_BRIDGE="${PVE_BRIDGE:-vmbr0}"; PVE_VLAN="${PVE_VLAN:-}"; PVE_TEMPLATE_ID="${PVE_TEMPLATE_ID:-9001}"
LINUX_COUNT="${LINUX_COUNT:-3}"; LINUX_VMID_BASE="${LINUX_VMID_BASE:-301}"
LINUX_CORES="${LINUX_CORES:-4}"; LINUX_MEMORY_MB="${LINUX_MEMORY_MB:-8192}"; LINUX_DISK="${LINUX_DISK:-60G}"
LINUX_IP="${LINUX_IP:-dhcp}"; LINUX_GW="${LINUX_GW:-}"
default_pubkey() { # first key that exists; the plain ed25519 name if none does
  local k
  for k in "$HOME/.ssh/id_ed25519.pub" "$HOME/.ssh/id_ed25519_cihosts.pub" "$HOME/.ssh/id_rsa.pub"; do
    [ -s "$k" ] && { printf '%s' "$k"; return; }
  done
  printf '%s' "$HOME/.ssh/id_ed25519.pub"
}
SSH_PUBKEY_FILE="${SSH_PUBKEY_FILE:-$(default_pubkey)}"; WAIT_ONLINE_MINUTES="${WAIT_ONLINE_MINUTES:-25}"
SNIP_DIR=/var/lib/vz/snippets              # refreshed from the node by pve_snippets (storage-dependent)
WINDOWS_ENABLE="${WINDOWS_ENABLE:-no}"; WINDOWS_VMID="${WINDOWS_VMID:-310}"; WINDOWS_EXISTING_VMID="${WINDOWS_EXISTING_VMID:-}"
WINDOWS_ISO="${WINDOWS_ISO:-local:iso/26100.32230.260111-0550.lt_release_svc_refresh_SERVER_EVAL_x64FRE_en-us.iso}"
WINDOWS_VIRTIO_ISO="${WINDOWS_VIRTIO_ISO:-local:iso/virtio-win.iso}"; WINDOWS_ANSWER_ISO="${WINDOWS_ANSWER_ISO:-local:iso/win-answer.iso}"
WINDOWS_CORES="${WINDOWS_CORES:-6}"; WINDOWS_MEMORY_MB="${WINDOWS_MEMORY_MB:-16384}"; WINDOWS_DISK="${WINDOWS_DISK:-120G}"; WINDOWS_SSH="${WINDOWS_SSH:-}"
MAC_ENABLE="${MAC_ENABLE:-yes}"; MAC_SSH="${MAC_SSH:-macmini}"; MAC_NAME="${MAC_NAME:-ci-macos-1}"
DESTROY_CONFIRM="${DESTROY_CONFIRM:-no}"
CI_HOSTS_PAT="${CI_HOSTS_PAT:-}"
PVE_DIR=/root/ci-hosts                     # where the building blocks live on the Proxmox node
SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=8 -o StrictHostKeyChecking=accept-new)

# ── GitHub API ───────────────────────────────────────────────────────────────
gh_api() { # gh_api METHOD path [curl args...] → body; sets GH_CODE
  local method="$1" path="$2"; shift 2
  local body; body="$(mktemp)"
  GH_CODE="$(curl -sS --max-time 30 -X "$method" -o "$body" -w '%{http_code}' \
    -H "Authorization: Bearer ${CI_HOSTS_PAT}" -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" "$@" "https://api.github.com${path}" || echo 000)"
  cat "$body"; rm -f "$body"
}
pat_works() { # the real check: can it mint a registration token (admin)?
  local out; out="$(gh_api POST "/repos/${GH_REPO}/actions/runners/registration-token")"
  [ "$GH_CODE" = 201 ] && jq -e '.token' <<<"$out" >/dev/null 2>&1
}
list_hosts() { gh_api GET "/repos/${GH_REPO}/actions/runners?per_page=100"; }
host_online() { # host_online NAME
  list_hosts | jq -e --arg n "$1" '.runners[] | select(.name == $n and .status == "online")' >/dev/null 2>&1
}
wait_online() { # wait_online NAME MINUTES
  local name="$1" mins="$2" deadline
  deadline=$(( $(date +%s) + mins * 60 ))
  printf '   waiting for %s to come online (up to %s min) ' "$name" "$mins"
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if host_online "$name"; then echo; ok "$name is online"; return 0; fi
    printf '.'; sleep 15
  done
  echo; warn "$name did not come online within ${mins} min"; return 1
}
deregister() { # deregister NAME — every registration with that name
  local ids id
  ids="$(list_hosts | jq -r --arg n "$1" '.runners[] | select(.name == $n) | .id')"
  for id in $ids; do gh_api DELETE "/repos/${GH_REPO}/actions/runners/${id}" >/dev/null; say "   deregistered $1 (id $id, HTTP $GH_CODE)"; done
}

# ── step 1: GitHub ───────────────────────────────────────────────────────────
step_github() {
  step "1/5 GitHub"
  local def_repo=""
  def_repo="$(git -C "$HERE" remote get-url origin 2>/dev/null | sed -E 's#(git@github.com:|https://github.com/)##; s#\.git$##' || true)"
  ask GH_REPO "Repository (OWNER/REPO)" "$def_repo"
  [[ "$GH_REPO" =~ ^[^/]+/[^/]+$ ]] || die "GH_REPO must be OWNER/REPO"
  ask RUNNER_GROUP "GitHub runner group (empty = Default; create one restricted to this repo if you can)" "$RUNNER_GROUP" optional

  if [ -z "$CI_HOSTS_PAT" ] && command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    CI_HOSTS_PAT="$(gh auth token 2>/dev/null || true)"
    if [ -n "$CI_HOSTS_PAT" ] && pat_works; then
      ok "reusing the gh CLI token (it can mint registration tokens for $GH_REPO)"
    else
      CI_HOSTS_PAT=""
      warn "the gh CLI token cannot administer $GH_REPO's runners — a PAT is needed"
    fi
  fi
  while [ -z "$CI_HOSTS_PAT" ] || ! pat_works; do
    say "   Create a fine-grained PAT (github.com → Settings → Developer settings → Fine-grained tokens):"
    say "   repository access: $GH_REPO only; permissions: Administration = Read and write."
    say "   (This PAT mints registration tokens on each CI host; the read-only CI_HOSTS_STATUS_TOKEN secret is a separate, weaker one.)"
    ask_secret CI_HOSTS_PAT "PAT (hidden)"
    pat_works || warn "that token cannot POST /repos/$GH_REPO/actions/runners/registration-token (HTTP $GH_CODE) — try again"
  done
  ok "PAT verified for $GH_REPO"
}

# ── step 2: Proxmox ──────────────────────────────────────────────────────────
pve() { ssh "${SSH_OPTS[@]}" "$PVE_HOST" "$@"; }
pve_sync_scripts() {
  pve "mkdir -p $PVE_DIR" >/dev/null
  scp -q "${SSH_OPTS[@]}" "$HERE/proxmox/create-ci-host-vm.sh" "$HERE/linux/install-ci-host.sh" "$PVE_HOST:$PVE_DIR/"
  pve "chmod 0755 $PVE_DIR/*.sh"
}
pve_snippets() { # the snippet storage must serve "snippets" BEFORE any user-data is shipped
  pve "$PVE_DIR/create-ci-host-vm.sh ensure-snippets --snippet-storage $PVE_SNIPPET_STORAGE"
  SNIP_DIR="$(pve "$PVE_DIR/create-ci-host-vm.sh snippet-dir --snippet-storage $PVE_SNIPPET_STORAGE" | tail -1)"
  [ -n "$SNIP_DIR" ] || die "could not determine the snippet directory for storage $PVE_SNIPPET_STORAGE"
  say "   snippets: ${PVE_SNIPPET_STORAGE} → ${SNIP_DIR}"
}
linux_name() { printf 'ci-linux-%s' "$1"; }
linux_vmid() { printf '%s' $(( LINUX_VMID_BASE + $1 - 1 )); }
linux_ipcfg() { # index → ip arg for create-ci-host-vm.sh
  if [ "$LINUX_IP" = dhcp ]; then printf 'dhcp'; return; fi
  local base="${LINUX_IP%/*}" cidr="${LINUX_IP#*/}" last
  last="${base##*.}"; printf '%s.%s/%s' "${base%.*}" "$(( last + $1 - 1 ))" "$cidr"
}
render_user_data() { # render_user_data NAME with_token(1|0) → stdout
  local name="$1" with_token="$2" pub installer
  pub="$(cat "$SSH_PUBKEY_FILE")"
  installer="$(base64 < "$HERE/linux/install-ci-host.sh" | tr -d '\n')"
  cat <<YAML
#cloud-config
hostname: ${name}
manage_etc_hosts: true
timezone: UTC
users:
  - name: ops
    groups: [sudo]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    lock_passwd: true
    ssh_authorized_keys:
      - ${pub}
package_update: true
packages: [qemu-guest-agent, curl, ca-certificates, jq]
write_files:
  - path: /usr/local/sbin/install-ci-host.sh
    permissions: '0755'
    owner: root:root
    encoding: b64
    content: ${installer}
YAML
  if [ "$with_token" = 1 ]; then
    cat <<YAML
  - path: /etc/ci-host/token
    permissions: '0600'
    owner: root:root
    content: '${CI_HOSTS_PAT}'
YAML
  fi
  cat <<YAML
runcmd:
  - systemctl enable --now qemu-guest-agent
  - [ sh, -c, "/usr/local/sbin/install-ci-host.sh --repo '${GH_REPO}' --name '${name}' --labels self-hosted,linux,networker-ci${RUNNER_GROUP:+ --runner-group '${RUNNER_GROUP}'} > /var/log/ci-host-install.log 2>&1" ]
YAML
}
create_linux_host() { # create_linux_host INDEX
  local i="$1" name vmid ud
  name="$(linux_name "$i")"; vmid="$(linux_vmid "$i")"
  if host_online "$name"; then ok "$name already online — skipping"; return 0; fi
  if pve "qm status $vmid" >/dev/null 2>&1; then
    warn "$name (VM $vmid) exists on $PVE_HOST but is not online yet — leaving it (status: $(pve "qm status $vmid" | awk '{print $2}'))"
    return 0
  fi
  ud="$(mktemp)"; render_user_data "$name" 1 > "$ud"
  scp -q "${SSH_OPTS[@]}" "$ud" "$PVE_HOST:/tmp/${name}.yaml"; rm -f "$ud"
  local gw_arg=""; [ "$LINUX_IP" != dhcp ] && gw_arg="--gw $LINUX_GW"
  # shellcheck disable=SC2086 # gw_arg/PVE_VLAN are intentionally word-split flags
  pve "install -m 0600 /tmp/${name}.yaml ${SNIP_DIR}/${name}.yaml && rm -f /tmp/${name}.yaml && \
       $PVE_DIR/create-ci-host-vm.sh create --template-id $PVE_TEMPLATE_ID --vmid $vmid --name $name \
         --cores $LINUX_CORES --memory $LINUX_MEMORY_MB --disk $LINUX_DISK --storage $PVE_STORAGE \
         --bridge $PVE_BRIDGE ${PVE_VLAN:+--vlan $PVE_VLAN} --snippet-storage $PVE_SNIPPET_STORAGE \
         --user-data ${SNIP_DIR}/${name}.yaml --ip $(linux_ipcfg "$i") $gw_arg"
}
scrub_linux_token() { # after first boot the PAT leaves the Proxmox node
  local name="$1" ud
  ud="$(mktemp)"; render_user_data "$name" 0 > "$ud"
  scp -q "${SSH_OPTS[@]}" "$ud" "$PVE_HOST:/tmp/${name}.yaml"; rm -f "$ud"
  pve "install -m 0600 /tmp/${name}.yaml ${SNIP_DIR}/${name}.yaml && rm -f /tmp/${name}.yaml"
  say "   scrubbed the PAT from ${name}'s cloud-init snippet on $PVE_HOST"
}
step_proxmox() {
  step "2/5 Proxmox — Linux CI hosts"
  ask PVE_HOST "Proxmox node (ssh alias or user@host)" "$PVE_HOST"
  if ! pve true 2>/dev/null; then
    warn "ssh key login to $PVE_HOST does not work yet"
    if confirm "run ssh-copy-id $PVE_HOST now?" y; then ssh-copy-id "$PVE_HOST"; fi
    pve true || die "still cannot ssh to $PVE_HOST non-interactively"
  fi
  ok "ssh to $PVE_HOST works"
  ask PVE_STORAGE "VM disk storage" "$PVE_STORAGE"
  ask PVE_SNIPPET_STORAGE "Snippet storage (must have 'Snippets' content enabled)" "$PVE_SNIPPET_STORAGE"
  ask PVE_BRIDGE "Bridge" "$PVE_BRIDGE"
  ask PVE_VLAN "VLAN tag for the CI VLAN (empty = untagged)" "$PVE_VLAN" optional
  ask PVE_TEMPLATE_ID "Ubuntu 24.04 cloud-init template VMID (created if missing; 9000 is the old 22.04 one — don't reuse it)" "$PVE_TEMPLATE_ID"
  ask LINUX_VMID_BASE "First Linux VMID (ci-linux-1; the next ones follow)" "$LINUX_VMID_BASE"
  ask LINUX_COUNT "Number of Linux CI hosts" "$LINUX_COUNT"
  ask LINUX_CORES "vCPU per host" "$LINUX_CORES"
  ask LINUX_MEMORY_MB "RAM per host (MB)" "$LINUX_MEMORY_MB"
  ask LINUX_DISK "Disk per host" "$LINUX_DISK"
  ask LINUX_IP "IP: dhcp, or a CIDR base (10.30.0.10/24 → .10, .11, …)" "$LINUX_IP"
  [ "$LINUX_IP" = dhcp ] || ask LINUX_GW "Gateway" "$LINUX_GW"
  ask SSH_PUBKEY_FILE "ssh public key to inject (user 'ops')" "$SSH_PUBKEY_FILE"
  SSH_PUBKEY_FILE="${SSH_PUBKEY_FILE/#\~/$HOME}"
  [ -s "$SSH_PUBKEY_FILE" ] || die "$SSH_PUBKEY_FILE not found"

  pve_sync_scripts
  pve_snippets
  say "   ensuring template $PVE_TEMPLATE_ID"
  pve "$PVE_DIR/create-ci-host-vm.sh ensure-template --template-id $PVE_TEMPLATE_ID --storage $PVE_STORAGE --bridge $PVE_BRIDGE ${PVE_VLAN:+--vlan $PVE_VLAN}"

  local i created=()
  for i in $(seq 1 "$LINUX_COUNT"); do
    create_linux_host "$i" && created+=("$(linux_name "$i")")
  done
  for i in $(seq 1 "$LINUX_COUNT"); do
    local name; name="$(linux_name "$i")"
    if wait_online "$name" "$WAIT_ONLINE_MINUTES"; then scrub_linux_token "$name"; fi
  done
}

# ── step 3: Mac mini ─────────────────────────────────────────────────────────
# The Mac's loop is a user LaunchAgent (Homebrew + rustup live in $HOME), so
# the PAT goes to ~/ci-host/token (0600, the login user) — no sudo needed for
# that. sudo IS needed on the first run (Homebrew, the .pkg casks, pmset):
# interactively the installer runs over `ssh -t` so the password prompt reaches
# you; non-interactive re-runs skip the sudo steps once done. No NOPASSWD
# sudoers rule is ever added — CI jobs run arbitrary repo code as that user.
step_mac() {
  step "3/5 Mac mini — macOS CI host"
  ask MAC_ENABLE "Set up a Mac mini? (yes/no)" "$MAC_ENABLE"
  [ "$MAC_ENABLE" = yes ] || { say "   skipped"; return 0; }
  say "   Remote Login must be enabled on the Mac (System Settings → General → Sharing → Remote Login)."
  ask MAC_SSH "Mac ssh target (alias or user@host)" "$MAC_SSH"
  ask MAC_NAME "CI host name" "$MAC_NAME"
  if host_online "$MAC_NAME"; then ok "$MAC_NAME already online — re-running the installer is safe but skipped"; return 0; fi
  if ! ssh "${SSH_OPTS[@]}" "$MAC_SSH" true 2>/dev/null; then
    warn "ssh key login to $MAC_SSH does not work yet"
    confirm "run ssh-copy-id $MAC_SSH now?" y && ssh-copy-id "$MAC_SSH"
    ssh "${SSH_OPTS[@]}" "$MAC_SSH" true || die "cannot ssh to $MAC_SSH"
  fi
  ssh "${SSH_OPTS[@]}" "$MAC_SSH" 'umask 077; mkdir -p ~/ci-host && cat > ~/ci-host/token' <<<"$CI_HOSTS_PAT"
  scp -q "${SSH_OPTS[@]}" "$HERE/macos/install-ci-host.sh" "$MAC_SSH:ci-host/install-ci-host.sh"
  local cmd="bash ~/ci-host/install-ci-host.sh --repo '$GH_REPO' --name '$MAC_NAME'${RUNNER_GROUP:+ --runner-group '$RUNNER_GROUP'}"
  if [ "$NON_INTERACTIVE" = 1 ]; then
    ssh "${SSH_OPTS[@]}" "$MAC_SSH" "$cmd"
  else
    say "   running the installer over ssh -t (first run: Homebrew / casks / pmset will ask for the Mac password)"
    ssh -t -o StrictHostKeyChecking=accept-new "$MAC_SSH" "$cmd"
  fi
  wait_online "$MAC_NAME" 10 || true
}

# ── step 4: Windows ──────────────────────────────────────────────────────────
windows_manual_steps() {
  cat <<TXT
   Manual steps on the Windows VM (console → after the OS install):
     1. Settings → System → Optional features → add "OpenSSH Server"; then in an elevated PowerShell:
          Set-Service sshd -StartupType Automatic; Start-Service sshd
          New-NetFirewallRule -Name sshd -DisplayName 'OpenSSH' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
     2. Put the PAT in C:\\ProgramData\\ci-host\\token (one line), then:
          Set-ExecutionPolicy Bypass -Scope Process -Force
          .\\install-ci-host.ps1 -Repo $GH_REPO -Name ci-windows-1${RUNNER_GROUP:+ -RunnerGroup $RUNNER_GROUP}
        (the script is infra/ci-hosts/windows/install-ci-host.ps1; copy it over with scp once sshd is up)
     3. Re-run this orchestrator with WINDOWS_SSH=Administrator@<ip> to do step 2 over ssh next time.
TXT
}
step_windows() {
  step "4/5 Windows CI host (optional)"
  ask WINDOWS_ENABLE "Create/manage a Windows CI host? (yes/no)" "$WINDOWS_ENABLE"
  [ "$WINDOWS_ENABLE" = yes ] || { say "   skipped"; return 0; }
  if host_online ci-windows-1; then ok "ci-windows-1 already online"; return 0; fi
  ask PVE_HOST "Proxmox node (ssh alias or user@host)" "$PVE_HOST"; pve true || die "cannot ssh to $PVE_HOST"
  pve_sync_scripts
  ask WINDOWS_EXISTING_VMID "Reuse an EXISTING Windows VM? (VMID, e.g. 101 'WindowsDesktop'; empty = create a new one)" "$WINDOWS_EXISTING_VMID" optional
  if [ -n "$WINDOWS_EXISTING_VMID" ]; then
    pve "$PVE_DIR/create-ci-host-vm.sh start --vmid $WINDOWS_EXISTING_VMID"
    say "   using existing VM $WINDOWS_EXISTING_VMID — skip to the in-guest install below"
  else
    ask WINDOWS_VMID "New Windows VMID" "$WINDOWS_VMID"
    if ! pve "qm status $WINDOWS_VMID" >/dev/null 2>&1; then
      ask WINDOWS_ISO "Windows Server 2025 Evaluation ISO (storage:iso/file)" "$WINDOWS_ISO"
      ask WINDOWS_VIRTIO_ISO "virtio-win ISO (storage:iso/file, empty to skip)" "$WINDOWS_VIRTIO_ISO" optional
      ask WINDOWS_ANSWER_ISO "autounattend answer ISO (storage:iso/file, empty = interactive install)" "$WINDOWS_ANSWER_ISO" optional
      if [ -n "$WINDOWS_ANSWER_ISO" ] && ! pve "pvesm list ${WINDOWS_ANSWER_ISO%%:*} --content iso" 2>/dev/null | grep -q "${WINDOWS_ANSWER_ISO}"; then
        warn "$WINDOWS_ANSWER_ISO not found on $PVE_HOST — continuing without an answer ISO"; WINDOWS_ANSWER_ISO=""
      fi
      ask WINDOWS_CORES "vCPU" "$WINDOWS_CORES"; ask WINDOWS_MEMORY_MB "RAM (MB)" "$WINDOWS_MEMORY_MB"; ask WINDOWS_DISK "Disk" "$WINDOWS_DISK"
      pve "$PVE_DIR/create-ci-host-vm.sh windows --vmid $WINDOWS_VMID --name ci-windows-1 --iso '$WINDOWS_ISO' \
           ${WINDOWS_VIRTIO_ISO:+--virtio-iso '$WINDOWS_VIRTIO_ISO'} ${WINDOWS_ANSWER_ISO:+--answer-iso '$WINDOWS_ANSWER_ISO'} \
           --cores $WINDOWS_CORES --memory $WINDOWS_MEMORY_MB \
           --disk $WINDOWS_DISK --storage $PVE_STORAGE --bridge $PVE_BRIDGE ${PVE_VLAN:+--vlan $PVE_VLAN}"
      say "   VM created and booted from the ISO — press a key at the console's 'Press any key to boot from CD' prompt;"
      say "   with the answer ISO the rest of the OS install is unattended."
    fi
  fi
  ask WINDOWS_SSH "Windows ssh target once OpenSSH Server is enabled (empty = print manual steps)" "$WINDOWS_SSH" optional
  if [ -z "$WINDOWS_SSH" ] || ! ssh "${SSH_OPTS[@]}" "$WINDOWS_SSH" 'powershell -NoProfile -Command "echo ok"' 2>/dev/null | grep -q ok; then
    [ -n "$WINDOWS_SSH" ] && warn "cannot reach $WINDOWS_SSH over ssh"
    windows_manual_steps; return 0
  fi
  ok "ssh to $WINDOWS_SSH works — installing over ssh"
  ssh "${SSH_OPTS[@]}" "$WINDOWS_SSH" 'powershell -NoProfile -Command "New-Item -ItemType Directory -Force C:\ProgramData\ci-host | Out-Null; [IO.File]::WriteAllText(\"C:\ProgramData\ci-host\token\", [Console]::In.ReadToEnd().Trim())"' <<<"$CI_HOSTS_PAT"
  scp -q "${SSH_OPTS[@]}" "$HERE/windows/install-ci-host.ps1" "$WINDOWS_SSH:C:/ProgramData/ci-host/install-ci-host.ps1"
  ssh "${SSH_OPTS[@]}" "$WINDOWS_SSH" "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\ProgramData\\ci-host\\install-ci-host.ps1 -Repo $GH_REPO -Name ci-windows-1${RUNNER_GROUP:+ -RunnerGroup $RUNNER_GROUP}"
  wait_online ci-windows-1 15 || true
}

# ── step 5: verify ───────────────────────────────────────────────────────────
print_hosts_table() {
  local out; out="$(list_hosts)"
  [ "$GH_CODE" = 200 ] || { warn "GET /repos/$GH_REPO/actions/runners → HTTP $GH_CODE"; return 1; }
  printf '   %-16s %-8s %-5s %s\n' NAME STATUS BUSY LABELS
  jq -r '.runners[] | [.name, .status, (.busy|tostring), ([.labels[].name] | join(","))] | @tsv' <<<"$out" \
    | awk -F'\t' '{printf "   %-16s %-8s %-5s %s\n", $1, $2, $3, $4}'
  [ "$(jq '.total_count' <<<"$out")" -gt 0 ] || warn "no self-hosted runners registered yet"
}
step_verify() {
  step "5/5 Verify"
  print_hosts_table || true
  local smoke="gh workflow run rust-audit.yml --repo $GH_REPO"
  if command -v gh >/dev/null 2>&1 && confirm "dispatch the smoke workflow now ($smoke)?" n; then
    gh workflow run rust-audit.yml --repo "$GH_REPO" && ok "dispatched — watch the 'Pick CI hosts'/'Detect…' job's notices for the per-OS decision"
  else
    say "   smoke: $smoke   (its changes job prints which side each OS was routed to)"
  fi
  cat <<TXT

   ${B}Next steps${N} (docs/self-hosted-ci.md § Rollout):
     1. Repo variable  CI_HOSTS_MODE = auto          (Settings → Secrets and variables → Actions → Variables)
     2. Repo secret    CI_HOSTS_STATUS_TOKEN          = a fine-grained PAT, this repo only, Administration: READ
        (the picker only lists runners with it; without it every run stays GitHub-hosted)
     3. Watch a few PRs: each routed workflow's first job prints "::notice:: linux -> [...] (why)"
     4. Route or disable the scheduled workflows before going private (minute math in the doc)
     5. Flip the repo to private
TXT
}

# ── subcommands ──────────────────────────────────────────────────────────────
cmd_status() {
  step_github_quiet
  step "CI hosts — GitHub view"
  print_hosts_table || true
  if [ -n "$PVE_HOST" ] && pve true 2>/dev/null; then
    step "Proxmox VMs on $PVE_HOST"
    pve_sync_scripts
    pve "$PVE_DIR/create-ci-host-vm.sh list --prefix ci-" | sed 's/^/   /'
    local vmid ip
    for vmid in $(pve "$PVE_DIR/create-ci-host-vm.sh list --prefix ci-" | awk 'NR>1 {print $1}'); do
      ip="$(pve "$PVE_DIR/create-ci-host-vm.sh ip --vmid $vmid" 2>/dev/null || true)"
      printf '   VM %s ip=%s' "$vmid" "${ip:-?}"
      if [ -n "$ip" ] && ssh "${SSH_OPTS[@]}" -o ConnectTimeout=4 "ops@$ip" true 2>/dev/null; then printf '  ssh=ok\n'; else printf '  ssh=no\n'; fi
    done
  fi
  step "ssh reachability"
  for t in "$MAC_SSH" "$WINDOWS_SSH"; do
    [ -n "$t" ] || continue
    if ssh "${SSH_OPTS[@]}" -o ConnectTimeout=4 "$t" true 2>/dev/null; then ok "$t reachable"; else warn "$t unreachable"; fi
  done
}
step_github_quiet() { # status/destroy need the repo + a PAT but no prompting noise
  [ -n "$GH_REPO" ] || GH_REPO="$(git -C "$HERE" remote get-url origin 2>/dev/null | sed -E 's#(git@github.com:|https://github.com/)##; s#\.git$##' || true)"
  [ -n "$GH_REPO" ] || die "GH_REPO is not set"
  if [ -z "$CI_HOSTS_PAT" ] && command -v gh >/dev/null 2>&1; then CI_HOSTS_PAT="$(gh auth token 2>/dev/null || true)"; fi
  [ -n "$CI_HOSTS_PAT" ] || ask_secret CI_HOSTS_PAT "PAT for $GH_REPO (hidden)"
}
cmd_add_linux() {
  local n="${ARGS[0]:-1}" existing max=0 name i
  step_github; step "add-linux $n"
  ask PVE_HOST "Proxmox node (ssh alias or user@host)" "$PVE_HOST"; pve true || die "cannot ssh to $PVE_HOST"
  ask SSH_PUBKEY_FILE "ssh public key to inject" "$SSH_PUBKEY_FILE"; SSH_PUBKEY_FILE="${SSH_PUBKEY_FILE/#\~/$HOME}"
  pve_sync_scripts
  pve_snippets
  existing="$(pve "$PVE_DIR/create-ci-host-vm.sh list --prefix ci-linux-" | awk 'NR>1 {print $2}' | sed 's/ci-linux-//')"
  for i in $existing; do [ "$i" -gt "$max" ] 2>/dev/null && max="$i"; done
  for i in $(seq $((max + 1)) $((max + n))); do create_linux_host "$i"; done
  for i in $(seq $((max + 1)) $((max + n))); do
    name="$(linux_name "$i")"
    wait_online "$name" "$WAIT_ONLINE_MINUTES" && scrub_linux_token "$name"
  done
}
cmd_destroy() {
  step_github_quiet
  step "destroy"
  ask PVE_HOST "Proxmox node (ssh alias or user@host)" "$PVE_HOST"; pve true || die "cannot ssh to $PVE_HOST"
  pve_sync_scripts
  local vms; vms="$(pve "$PVE_DIR/create-ci-host-vm.sh list --prefix ci-" | awk 'NR>1 {print $1":"$2}')"
  say "   VMs: $(echo "$vms" | tr '\n' ' ')"
  say "   GitHub registrations named ci-* will be deleted too (the Mac's launchd loop is NOT touched)."
  if [ "$NON_INTERACTIVE" = 1 ]; then
    [ "$DESTROY_CONFIRM" = yes ] || die "--non-interactive destroy needs DESTROY_CONFIRM=yes in $ENV_FILE"
  else
    confirm "Destroy ALL of the above?" n || { say "   aborted"; return 0; }
  fi
  local entry vmid name
  for entry in $vms; do
    vmid="${entry%%:*}"; name="${entry#*:}"
    deregister "$name"
    pve "$PVE_DIR/create-ci-host-vm.sh destroy --vmid $vmid"
  done
  ok "done"
}

case "$CMD" in
  setup)  step_github; step_proxmox; step_mac; step_windows; step_verify ;;
  status) cmd_status ;;
  add-linux) cmd_add_linux ;;
  destroy) cmd_destroy ;;
  verify) step_github_quiet; step_verify ;;
esac
