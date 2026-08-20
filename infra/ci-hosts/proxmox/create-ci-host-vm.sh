#!/usr/bin/env bash
# infra/ci-hosts/proxmox/create-ci-host-vm.sh — Proxmox-side building block for
# the CI hosts (docs/self-hosted-ci.md). Runs as root ON the Proxmox node
# (setup-ci-hosts.sh copies it there and drives it over ssh).
#
#   ensure-snippets  make sure the snippet storage serves the "snippets" content
#                    type (pve's `dir: local` ships with iso,vztmpl,backup,import
#                    only — `--cicustom` fails without it) and its dir exists
#   snippet-dir      print the directory behind <snippet-storage>:snippets/ (quiet)
#   ensure-template  create the Ubuntu 24.04 (noble) cloud-init template if missing
#                    (downloads noble-server-cloudimg-amd64.img, verifies it against
#                    Ubuntu's SHA256SUMS — the existing 22.04 template is NOT reused)
#   create           clone one Linux CI host VM from the template + cloud-init
#   windows          create a UEFI+vTPM q35 VM booting a Windows ISO, with the
#                    virtio-win ISO and an optional autounattend answer ISO attached
#   start            start an EXISTING VM (e.g. reuse VM 101 as the Windows CI host)
#   destroy          stop + destroy a VM (and its cloud-init snippet)
#   list             the CI host VMs (name prefix) with status
#   ip               the guest's IPv4 via the qemu guest agent
#
# Examples (defaults match pve.home.lab: local-lvm, vmbr0, no VLAN tag):
#   create-ci-host-vm.sh ensure-template --template-id 9001
#   create-ci-host-vm.sh create --template-id 9001 --vmid 301 --name ci-linux-1 \
#       --cores 4 --memory 8192 --disk 60G --user-data /var/lib/vz/snippets/ci-linux-1.yaml
#   create-ci-host-vm.sh windows --vmid 310 --name ci-windows-1 \
#       --iso local:iso/26100.32230.260111-0550.lt_release_svc_refresh_SERVER_EVAL_x64FRE_en-us.iso \
#       --virtio-iso local:iso/virtio-win.iso --answer-iso local:iso/win-answer.iso \
#       --cores 6 --memory 16384 --disk 120G
#
# Dev tooling: bash 4+.

set -euo pipefail

log() { printf '\033[1;34m[pve]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[pve] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }
command -v qm >/dev/null 2>&1 || die "qm not found — run this on the Proxmox node"

CMD="${1:-}"; [ $# -gt 0 ] && shift
TEMPLATE_ID=9001 VMID="" NAME="" CORES=4 MEMORY=8192 DISK=60G STORAGE=local-lvm BRIDGE=vmbr0 VLAN=""
USER_DATA="" SNIPPET_STORAGE=local IP=dhcp GW="" PREFIX=ci- ISO="" VIRTIO_ISO="" ANSWER_ISO="" FORCE=0
ANSWER_SRC="" ANSWER_OUT="" PASSWORD_FILE=""
IMAGE_BASE="${CI_HOST_CLOUD_IMAGE_BASE:-https://cloud-images.ubuntu.com/noble/current}"
IMAGE_NAME=noble-server-cloudimg-amd64.img

while [ $# -gt 0 ]; do
  case "$1" in
    --template-id) TEMPLATE_ID="$2"; shift ;;
    --vmid) VMID="$2"; shift ;;
    --name) NAME="$2"; shift ;;
    --cores) CORES="$2"; shift ;;
    --memory) MEMORY="$2"; shift ;;
    --disk) DISK="$2"; shift ;;
    --storage) STORAGE="$2"; shift ;;
    --bridge) BRIDGE="$2"; shift ;;
    --vlan) VLAN="$2"; shift ;;
    --user-data) USER_DATA="$2"; shift ;;
    --snippet-storage) SNIPPET_STORAGE="$2"; shift ;;
    --ip) IP="$2"; shift ;;
    --gw) GW="$2"; shift ;;
    --prefix) PREFIX="$2"; shift ;;
    --iso) ISO="$2"; shift ;;
    --virtio-iso) VIRTIO_ISO="$2"; shift ;;
    --answer-iso) ANSWER_ISO="$2"; shift ;;
    --answer-src) ANSWER_SRC="$2"; shift ;;
    --answer-out) ANSWER_OUT="$2"; shift ;;
    --password-file) PASSWORD_FILE="$2"; shift ;;
    --image-base) IMAGE_BASE="$2"; shift ;;
    --force) FORCE=1 ;;
    -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown flag: $1" ;;
  esac
  shift
done

net_arg() { # --net0 value with optional VLAN tag
  local v="virtio,bridge=${BRIDGE}"
  [ -n "$VLAN" ] && v="${v},tag=${VLAN}"
  printf '%s' "$v"
}

vm_exists() { qm status "$1" >/dev/null 2>&1; }

# `pvesm config <storage>` prints "key value" (or "key: value" on older
# releases) lines; match both.
# storage_field content|path → that field of $SNIPPET_STORAGE. `pvesm config`
# does not exist on PVE 9 ("unknown command", exit 255 — found on the first
# real run); `pvesh get /storage/<name>` is stable across PVE 7/8/9 and emits
# flat JSON, so a sed on "key":"value" is enough (no jq on a stock node). The
# config file is the fallback when the API socket is unavailable.
storage_field() {
  local v
  v="$(pvesh get "/storage/${SNIPPET_STORAGE}" --output-format json 2>/dev/null \
        | sed -n "s/.*\"$1\":\"\([^\"]*\)\".*/\1/p" | head -1)"
  if [ -z "$v" ]; then
    v="$(awk -v s="$SNIPPET_STORAGE" -v k="$1" \
          '$1 ~ /:$/ { insec = ($2 == s); next } insec && $1 == k { print $2; exit }' \
          /etc/pve/storage.cfg 2>/dev/null)"
  fi
  printf '%s' "$v"
}
snippet_dir() { local p; p="$(storage_field path)"; printf '%s/snippets' "${p:-/var/lib/vz}"; }

cmd_ensure_snippets() {
  local current
  current="$(storage_field content)"
  [ -n "$current" ] || die "pvesm config ${SNIPPET_STORAGE}: no content list — does storage '${SNIPPET_STORAGE}' exist?"
  case ",${current}," in
    *,snippets,*) log "storage ${SNIPPET_STORAGE} already serves snippets (content: ${current})" ;;
    *)
      log "storage ${SNIPPET_STORAGE} content '${current}' lacks snippets — enabling (every existing type kept)"
      pvesm set "$SNIPPET_STORAGE" --content "${current},snippets"
      log "storage ${SNIPPET_STORAGE} content is now: $(storage_field content)" ;;
  esac
  install -d -m 0700 "$(snippet_dir)"
}

cmd_ensure_template() {
  if vm_exists "$TEMPLATE_ID"; then
    log "template $TEMPLATE_ID already exists — keeping it"
    return 0
  fi
  # Lives next to the jammy image Proxmox already has; verified against
  # Ubuntu's published SHA256SUMS so a truncated/poisoned download never
  # becomes the base of every CI host.
  local img="/var/lib/vz/template/${IMAGE_NAME}" sums expected actual
  sums="$(curl -fsSL --retry 3 "${IMAGE_BASE}/SHA256SUMS")" || die "could not fetch ${IMAGE_BASE}/SHA256SUMS"
  expected="$(printf '%s\n' "$sums" | awk -v n="*${IMAGE_NAME}" '$2 == n {print $1}')"
  [ -n "$expected" ] || die "SHA256SUMS has no entry for ${IMAGE_NAME}"
  if [ -s "$img" ]; then
    actual="$(sha256sum "$img" | awk '{print $1}')"
    [ "$actual" = "$expected" ] || { log "cached $img does not match SHA256SUMS — re-downloading"; rm -f "$img"; }
  fi
  if [ ! -s "$img" ]; then
    log "downloading ${IMAGE_BASE}/${IMAGE_NAME}"
    curl -fsSL --retry 3 -o "$img.part" "${IMAGE_BASE}/${IMAGE_NAME}"
    actual="$(sha256sum "$img.part" | awk '{print $1}')"
    [ "$actual" = "$expected" ] || { rm -f "$img.part"; die "SHA256 mismatch for ${IMAGE_NAME}: got $actual, expected $expected"; }
    mv "$img.part" "$img"
  fi
  log "SHA256 verified: $expected"
  cmd_ensure_snippets
  log "creating template $TEMPLATE_ID (ubuntu-2404-template) from $img"
  qm create "$TEMPLATE_ID" --name ubuntu-2404-template --memory 2048 --cores 2 \
    --net0 "$(net_arg)" --scsihw virtio-scsi-single --agent enabled=1 \
    --ostype l26 --cpu host --serial0 socket --vga serial0
  qm importdisk "$TEMPLATE_ID" "$img" "$STORAGE" >/dev/null
  qm set "$TEMPLATE_ID" --scsi0 "${STORAGE}:vm-${TEMPLATE_ID}-disk-0,discard=on,ssd=1" \
    --ide2 "${STORAGE}:cloudinit" --boot order=scsi0 --ipconfig0 ip=dhcp >/dev/null
  qm template "$TEMPLATE_ID"
  log "template $TEMPLATE_ID ready"
}

cmd_create() {
  [ -n "$VMID" ] && [ -n "$NAME" ] || die "create needs --vmid and --name"
  [ -n "$USER_DATA" ] || die "create needs --user-data <snippet path on this node>"
  [ -s "$USER_DATA" ] || die "user-data $USER_DATA is missing or empty"
  vm_exists "$TEMPLATE_ID" || die "template $TEMPLATE_ID missing — run ensure-template first"
  if vm_exists "$VMID"; then
    if [ "$FORCE" = 1 ]; then
      log "VM $VMID exists — --force: destroying first"; cmd_destroy
    else
      log "VM $VMID ($NAME) already exists — leaving it alone (use --force to recreate)"
      return 0
    fi
  fi
  # Snippets are referenced as <storage>:snippets/<file>; the file must live
  # in that storage's snippets dir and the storage must serve that content type.
  cmd_ensure_snippets
  local sdir snippet_name
  sdir="$(snippet_dir)"
  snippet_name="$(basename "$USER_DATA")"
  if [ "$(readlink -f "$USER_DATA")" != "$(readlink -f "$sdir/$snippet_name")" ]; then
    install -m 0600 "$USER_DATA" "$sdir/$snippet_name"
  fi
  chmod 0600 "$sdir/$snippet_name"

  log "cloning template $TEMPLATE_ID → $VMID ($NAME): ${CORES} vCPU, ${MEMORY} MB, ${DISK}"
  qm clone "$TEMPLATE_ID" "$VMID" --name "$NAME" --full 1 --storage "$STORAGE" >/dev/null
  local ipcfg="ip=${IP}"
  if [ "$IP" != dhcp ]; then
    [ -n "$GW" ] || die "--ip CIDR needs --gw"
    ipcfg="ip=${IP},gw=${GW}"
  fi
  qm set "$VMID" --cores "$CORES" --memory "$MEMORY" --balloon 0 --cpu host \
    --net0 "$(net_arg)" --ipconfig0 "$ipcfg" --agent enabled=1 --onboot 1 \
    --cicustom "user=${SNIPPET_STORAGE}:snippets/${snippet_name}" \
    --description "networker CI host (GitHub self-hosted runner). Managed by infra/ci-hosts/setup-ci-hosts.sh" >/dev/null
  qm resize "$VMID" scsi0 "$DISK" >/dev/null
  qm cloudinit update "$VMID" >/dev/null 2>&1 || true
  qm start "$VMID"
  log "VM $VMID ($NAME) started — cloud-init installs the toolchain and registers the CI host (10-15 min first boot)"
}

cmd_windows() {
  [ -n "$VMID" ] && [ -n "$NAME" ] || die "windows needs --vmid and --name"
  [ -n "$ISO" ] || die "windows needs --iso <storage>:iso/<file>"
  if vm_exists "$VMID"; then
    log "VM $VMID ($NAME) already exists — leaving it alone"
    return 0
  fi
  # Same shape as the node's existing VM 101: ostype win11 (what Proxmox uses
  # for Server 2025 too), q35, OVMF + vTPM 2.0, virtio-scsi disk, virtio net.
  # ide2 = install ISO, ide0 = virtio-win drivers, ide1 = autounattend answer
  # ISO (optional — with it the install runs unattended after the "press any
  # key to boot from CD" prompt on the console).
  log "creating Windows VM $VMID ($NAME): q35 + OVMF + vTPM, ${CORES} vCPU, ${MEMORY} MB, ${DISK}"
  qm create "$VMID" --name "$NAME" --ostype win11 --machine q35 --bios ovmf \
    --cores "$CORES" --memory "$MEMORY" --balloon 0 --cpu host \
    --scsihw virtio-scsi-single --agent enabled=1 --onboot 1 \
    --net0 "$(net_arg)" \
    --efidisk0 "${STORAGE}:1,efitype=4m,pre-enrolled-keys=1" \
    --tpmstate0 "${STORAGE}:1,version=v2.0" \
    --scsi0 "${STORAGE}:${DISK%G},discard=on,ssd=1" \
    --ide2 "${ISO},media=cdrom" --boot order=ide2 \
    --vga std >/dev/null
  [ -n "$VIRTIO_ISO" ] && qm set "$VMID" --ide0 "${VIRTIO_ISO},media=cdrom" >/dev/null
  [ -n "$ANSWER_ISO" ] && qm set "$VMID" --ide1 "${ANSWER_ISO},media=cdrom" >/dev/null
  qm set "$VMID" --description "networker CI host (Windows). After the OS install: enable OpenSSH Server → infra/ci-hosts/windows/install-ci-host.ps1 (or setup-ci-hosts.sh does it over ssh)" >/dev/null
  qm start "$VMID"
  # OVMF + a Windows ISO show "Press any key to boot from CD or DVD…" for a
  # few seconds; an unattended install must not stall on it. Tap Enter on the
  # VM's keyboard through QEMU for the first 25 s (harmless once Setup is up).
  log "VM $VMID ($NAME) started from the ISO${ANSWER_ISO:+ with answer ISO $ANSWER_ISO} — pressing Enter past the CD-boot prompt"
  local i
  for i in $(seq 1 50); do qm sendkey "$VMID" ret >/dev/null 2>&1 || true; sleep 0.5; done
  log "Windows Setup should be running — unattended with the answer ISO (~15 min), then first-logon.ps1 enables ssh"
}

# Render autounattend.xml.tmpl + first-logon.ps1 + the operator's authorized_keys
# into a small answer ISO under the ISO storage. The Administrator password
# comes from a file (never argv). Re-running overwrites the ISO.
cmd_make_answer_iso() {
  [ -n "$ANSWER_SRC" ] && [ -d "$ANSWER_SRC" ] || die "make-answer-iso needs --answer-src <dir with autounattend.xml.tmpl, first-logon.ps1, authorized_keys>"
  [ -n "$ANSWER_OUT" ] || die "make-answer-iso needs --answer-out <file.iso>"
  [ -n "$NAME" ] || die "make-answer-iso needs --name <computer name>"
  [ -n "$PASSWORD_FILE" ] && [ -s "$PASSWORD_FILE" ] || die "make-answer-iso needs --password-file <file holding the Administrator password>"
  command -v xorriso >/dev/null 2>&1 || die "xorriso not found on this node (apt install xorriso)"
  local tmpl="$ANSWER_SRC/autounattend.xml.tmpl" pw work iso_dir
  [ -s "$tmpl" ] || die "$tmpl missing"
  pw="$(head -n1 "$PASSWORD_FILE")"
  case "$pw" in *'&'*|*'<'*|*'>'*|*'"'*|*"'"*|*'|'*) die "the Administrator password must not contain & < > \" ' | (XML/sed-unsafe)" ;; esac
  work="$(mktemp -d)"
  sed -e "s|@@COMPUTERNAME@@|${NAME}|g" -e "s|@@ADMIN_PASSWORD@@|${pw}|g" "$tmpl" > "$work/autounattend.xml"
  install -m 0644 "$ANSWER_SRC/first-logon.ps1" "$work/first-logon.ps1"
  [ -s "$ANSWER_SRC/authorized_keys" ] && install -m 0644 "$ANSWER_SRC/authorized_keys" "$work/authorized_keys"
  iso_dir="$(dirname "$(pvesm path "local:iso/x" 2>/dev/null || echo /var/lib/vz/template/iso/x)")"
  install -d -m 0755 "$iso_dir"
  xorriso -as mkisofs -quiet -o "$iso_dir/$ANSWER_OUT" -V CIANSWER -J -R "$work" >/dev/null 2>&1 \
    || die "xorriso failed building $iso_dir/$ANSWER_OUT"
  chmod 0600 "$iso_dir/$ANSWER_OUT"   # it carries the Administrator password
  rm -rf "$work"
  log "answer ISO ready: local:iso/$ANSWER_OUT (computer name $NAME; first-logon.ps1 + $( [ -s "$ANSWER_SRC/authorized_keys" ] && echo "authorized_keys" || echo "NO authorized_keys"))"
}

cmd_start() {
  [ -n "$VMID" ] || die "start needs --vmid"
  vm_exists "$VMID" || die "VM $VMID does not exist"
  if [ "$(qm status "$VMID" | awk '{print $2}')" = running ]; then log "VM $VMID already running"; return 0; fi
  qm start "$VMID"; log "VM $VMID started"
}

cmd_destroy() {
  [ -n "$VMID" ] || die "destroy needs --vmid"
  vm_exists "$VMID" || { log "VM $VMID does not exist"; return 0; }
  local snip
  snip="$(qm config "$VMID" | sed -n 's/^cicustom: user=[^:]*:snippets\/\(.*\)$/\1/p')"
  log "stopping + destroying VM $VMID"
  qm stop "$VMID" --timeout 60 >/dev/null 2>&1 || true
  qm destroy "$VMID" --purge --destroy-unreferenced-disks 1 >/dev/null
  [ -n "$snip" ] && rm -f "$(snippet_dir)/${snip}"
  log "VM $VMID destroyed"
}

cmd_list() {
  qm list | awk -v p="$PREFIX" 'NR==1 || index($2, p)==1'
}

cmd_ip() {
  [ -n "$VMID" ] || die "ip needs --vmid"
  qm guest cmd "$VMID" network-get-interfaces 2>/dev/null \
    | python3 -c 'import json,sys
for i in json.load(sys.stdin):
    if i.get("name") in ("lo",): continue
    for a in i.get("ip-addresses", []):
        if a.get("ip-address-type") == "ipv4" and not a["ip-address"].startswith("172.17."):
            print(a["ip-address"]); sys.exit(0)' 2>/dev/null || true
}

case "$CMD" in
  ensure-snippets) cmd_ensure_snippets ;;
  snippet-dir) snippet_dir; echo ;;
  ensure-template) cmd_ensure_template ;;
  create) cmd_create ;;
  windows) cmd_windows ;;
  make-answer-iso) cmd_make_answer_iso ;;
  start) cmd_start ;;
  destroy) cmd_destroy ;;
  list) cmd_list ;;
  ip) cmd_ip ;;
  ""|-h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) die "unknown command: $CMD" ;;
esac
