#!/usr/bin/env bash
# ─── networker local lab ──────────────────────────────────────────────────────
# Spin up the whole managed path on your laptop with Docker — control plane +
# Postgres + N runner containers (what tester VMs run) + M target containers
# (what endpoint VMs run, optionally behind the real install.sh proxy stacks) —
# then drive real runs through the public API and assert the outcome, BEFORE
# spending money/time on cloud VMs.
#
#   ./lab/lab.sh build                        # build images from this checkout
#   ./lab/lab.sh up --runners 3 --targets rust,nginx,caddy [--ui]
#   ./lab/lab.sh up --runners 1 --targets rust,nginx,windows   # + a Windows Server VM (IIS) target
#   ./lab/lab.sh up --runners 1 --windows-runners 1 --targets rust,nginx  # + a Windows Server VM RUNNER
#   ./lab/lab.sh validate                     # end-to-end run matrix (like the prod canary)
#   ./lab/lab.sh status | logs [svc] | shell <svc> | psql | down [--volumes]
#   LAB_INSTANCE=2 ./lab/lab.sh up …          # a second, independent lab next to the first
#
# Portable: bash 3.2 (macOS), Linux; needs docker (compose v2), curl, jq.
# Nothing here touches your host toolchains — Rust + .NET + Node build inside
# Docker (linux/amd64 or linux/arm64, whatever your Docker runs natively).
# The optional `windows` target and the Windows runners are real Windows Server
# VMs (dockur/windows, QEMU+KVM in a container) — Linux with /dev/kvm only;
# see README "Windows".
set -euo pipefail

LAB_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$LAB_DIR/.." && pwd)"

# ── Instance (LAB_INSTANCE=N, default 1) ─────────────────────────────────────
# Several labs can coexist on one host (e.g. one per branch/agent): instance N
# is compose project `nwk-lab` (N=1) / `nwk-labN`, network 172.31.(99+N).0/24,
# host ports 5030/8088/55432 (+ the Windows console 8006) shifted by N-1 (the
# console by 100·(N-1)), state in lab/.state[-N], generated topology in
# lab/.generated[-N], and per-project docker volume names — so nothing of one
# instance touches another. Every LAB_* default below still overrides.
LAB_INSTANCE="${LAB_INSTANCE:-1}"
case "$LAB_INSTANCE" in ''|*[!0-9]*) echo "LAB_INSTANCE must be a positive integer (got '$LAB_INSTANCE')" >&2; exit 2;; esac
[ "$LAB_INSTANCE" -ge 1 ] || { echo "LAB_INSTANCE must be >= 1" >&2; exit 2; }
if [ "$LAB_INSTANCE" = 1 ]; then LAB_PROJECT="nwk-lab"; LAB_SUFFIX=""; else LAB_PROJECT="nwk-lab${LAB_INSTANCE}"; LAB_SUFFIX="-${LAB_INSTANCE}"; fi
LAB_OFFSET=$((LAB_INSTANCE - 1))
GEN_DIR="$LAB_DIR/.generated${LAB_SUFFIX}"
STATE_DIR="$LAB_DIR/.state${LAB_SUFFIX}"
TOPOLOGY="$GEN_DIR/topology.yml"
STATE_ENV="$STATE_DIR/lab.env"

# ── Tunables (env-overridable) ───────────────────────────────────────────────
LAB_NET_PREFIX="${LAB_NET_PREFIX:-172.31.$((99 + LAB_INSTANCE))}"   # /24 for the lab network
LAB_CP_PORT="${LAB_CP_PORT:-$((5030 + LAB_OFFSET))}"                # control plane on the host
LAB_UI_PORT="${LAB_UI_PORT:-$((8088 + LAB_OFFSET))}"                # SPA on the host (with --ui)
LAB_PG_PORT="${LAB_PG_PORT:-$((55432 + LAB_OFFSET))}"               # postgres on the host
LAB_ADMIN_EMAIL="${LAB_ADMIN_EMAIL:-admin@lab.local}"
LAB_ADMIN_BOOTSTRAP_PASSWORD="${LAB_ADMIN_BOOTSTRAP_PASSWORD:-LabBootstrap-Pass1!}"
LAB_ADMIN_PASSWORD="${LAB_ADMIN_PASSWORD:-LabAdmin-Pass1!}"   # set on first login (must_change_password)
LAB_PROJECT_NAME="${LAB_PROJECT_NAME:-Local Lab}"
LAB_STARTUP_TIMEOUT="${LAB_STARTUP_TIMEOUT:-240}"     # s for control plane readiness
LAB_AGENT_TIMEOUT="${LAB_AGENT_TIMEOUT:-120}"         # s for all runners to come online
# Windows (IIS) target — a Windows Server VM via dockur/windows (KVM). First
# boot downloads the eval ISO (~6 GB) + installs Windows + runs install.ps1
# (endpoint + IIS): 15-40 min (14 min measured). The disk lives in a named volume, so the next
# `up` boots the installed VM in ~1-2 min.
LAB_WINDOWS_IMAGE="${LAB_WINDOWS_IMAGE:-dockurr/windows:latest}"
LAB_WINDOWS_VERSION="${LAB_WINDOWS_VERSION:-2022}"     # dockur VERSION: 2022 | 2025 (Windows Server eval)
LAB_WINDOWS_RAM="${LAB_WINDOWS_RAM:-6G}"
LAB_WINDOWS_CPUS="${LAB_WINDOWS_CPUS:-4}"
LAB_WINDOWS_DISK="${LAB_WINDOWS_DISK:-40G}"
LAB_WINDOWS_USER="${LAB_WINDOWS_USER:-Docker}"
LAB_WINDOWS_PASSWORD="${LAB_WINDOWS_PASSWORD:-LabWindows-Pass1!}"   # local admin, SSH (lab only — NOT a secret)
LAB_WINDOWS_TIMEOUT="${LAB_WINDOWS_TIMEOUT:-3600}"     # s to wait for the VM's endpoint + IIS on first boot
# dockur web viewer (VM console) on the host: the first Windows TARGET gets this
# port, further Windows targets +1 each; Windows RUNNERS start at +10 (runner-K
# → +10+K-1). Instance N shifts the whole block by 100·(N-1).
LAB_WINDOWS_VIEWER_PORT="${LAB_WINDOWS_VIEWER_PORT:-$((8006 + 100 * LAB_OFFSET))}"
# Windows RUNNER VMs (--windows-runners M): same dockur image/VM sizing as the
# target; the guest gets the checkout's install.ps1 (-Component tester → the
# RELEASED networker-tester.exe: Windows binaries can't be cross-built on this
# host) + the C# agent PUBLISHED FROM THE CHECKOUT (win-x64 self-contained),
# and runs the agent as a SYSTEM ONSTART scheduled task (the cloud Windows
# bootstraps' persistence shape) — see images/windows/oem-runner/.
LAB_WINDOWS_RUNNER_RAM="${LAB_WINDOWS_RUNNER_RAM:-4G}"
LAB_WINDOWS_RUNNER_CPUS="${LAB_WINDOWS_RUNNER_CPUS:-2}"
LAB_WINDOWS_RUNNER_DISK="${LAB_WINDOWS_RUNNER_DISK:-32G}"
LAB_WIN_AGENT_BUILD="${LAB_WIN_AGENT_BUILD:-docker}"     # docker (dotnet SDK image, BuildKit --output) | host (your dotnet)
# Image tag for nwk-lab/{rustbin,controlplane,runner,target-*,ui}. Shared by
# every instance by default (one build serves them all); give each CHECKOUT
# its own tag (LAB_IMAGE_TAG=mybranch) when two labs build from different
# trees at the same time — a concurrent `lab.sh build` elsewhere re-tags
# `:local` under your feet (seen 2026-08-16: instance 2 came up on images a
# sibling checkout had just rebuilt from main).
LAB_IMAGE_TAG="${LAB_IMAGE_TAG:-local}"
# Every target is also reachable by NAME on labnet: target-N.<domain> (a docker
# network alias — the embedded DNS answers it for runners AND the control
# plane). The Windows target's deployment is registered with that name as its
# endpoint_hosts[0] and its IIS SNI binding + cert SAN carry it, so dispatch
# hands the tester a hostname and http.sys serves HTTP/3 (SNI required).
LAB_TARGET_DOMAIN="${LAB_TARGET_DOMAIN:-lab}"
export LAB_NET_PREFIX LAB_CP_PORT LAB_UI_PORT LAB_PG_PORT LAB_ADMIN_EMAIL LAB_ADMIN_BOOTSTRAP_PASSWORD LAB_PROJECT LAB_IMAGE_TAG LAB_TARGET_DOMAIN

BASE_URL="http://127.0.0.1:${LAB_CP_PORT}"
COMPOSE=(docker compose -p "$LAB_PROJECT" -f "$LAB_DIR/docker-compose.yml")
[ -f "$TOPOLOGY" ] && COMPOSE+=(-f "$TOPOLOGY")

# ── Output helpers ───────────────────────────────────────────────────────────
if [ -t 1 ]; then C_B=$'\033[1m'; C_D=$'\033[2m'; C_G=$'\033[32m'; C_Y=$'\033[33m'; C_R=$'\033[31m'; C_C=$'\033[36m'; C_0=$'\033[0m'
else C_B=""; C_D=""; C_G=""; C_Y=""; C_R=""; C_C=""; C_0=""; fi
note() { printf '%s▸%s %s\n' "$C_C" "$C_0" "$*"; }
ok()   { printf '%s  ✓%s %s\n' "$C_G" "$C_0" "$*"; }
warn() { printf '%s  ⚠%s %s\n' "$C_Y" "$C_0" "$*" >&2; }
die()  { printf '%s  ✗ %s%s\n' "$C_R" "$*" "$C_0" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1 — $2"; }

usage() {
  sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Commands:
  build [--no-cache] [--stacks a,b] [--ui]   Build images (rustbin, controlplane, runner, target-<stack>[, ui])
  up [--runners N] [--targets SPEC] [--ui]   Start everything, register runners, wait until online
     [--windows-runners M]                   + M Windows Server VM runners (runner-(N+1)..runner-(N+M);
                                             Linux + /dev/kvm only, first boot ~15 min each)
     [--netem "delay 40ms 5ms"]              WAN emulation on Linux runners (tc netem)
     [--agents-via-ui]                       Runners connect through the nginx UI proxy (needs --ui)
     [--no-build]                            Skip image build (default builds what is missing)
     [--windows-async]                       Don't wait for the Windows VM(s) (use wait-windows later)
  validate [--modes m1,m2] [--runs N]        Drive real runs through the API + assert (see validate.sh)
  scale --runners N | --targets SPEC         Change topology (re-registers as needed)
  status                                     Runners/targets/agents/health at a glance
  logs [service] [-f]                        docker compose logs
  compose <args…>                            docker compose with this instance's project + files + env
  shell <service>                            bash inside a container
  psql                                       psql into the lab database
  tester <runner> -- <args>                  Run networker-tester directly inside a runner (bypasses the CP)
  wait-windows                               Block until the Windows target(s)/runner(s) are ready
  windows-log [target-N|runner-K] [-f]       Show a Windows VM's setup log/status (from the host share)
  windows-ssh [target-N|runner-K] [-- cmd]   SSH into a Windows VM (user/password printed)
  windows-iis-refresh [target-N]             Re-run install.ps1 -Setup iis -Fqdn target-N.lab inside the VM (SNI/H3 binding)
  down [--volumes]                           Stop (and optionally wipe the DB + the Windows VM disks)
  env                                        Print the saved lab env (token, project id, urls)

Targets SPEC = comma list of stacks; one container each:
  rust      networker-endpoint only (8080/8443 + UDP 9997-9999)
  nginx|caddy|apache|haproxy|traefik
            networker-endpoint + that proxy set up by install.sh --setup-stack
  windows   (alias: iis) a Windows Server VM: install.ps1 endpoint (8080/8443)
            + IIS 8082/8445 (SNI binding target-N.lab → HTTP/3) — Linux + /dev/kvm only, first boot 15-40 min
  Default: rust,nginx        Runners default: 2 (Linux), Windows runners: 0

Instances: LAB_INSTANCE=N (default 1) → compose project ${LAB_PROJECT}, network
  ${LAB_NET_PREFIX}.0/24, host ports cp ${LAB_CP_PORT} ui ${LAB_UI_PORT} pg ${LAB_PG_PORT}
  windows console ${LAB_WINDOWS_VIEWER_PORT}+, state ${STATE_DIR#$REPO_ROOT/}
EOF
}

# ── State ────────────────────────────────────────────────────────────────────
load_state() { [ -f "$STATE_ENV" ] && . "$STATE_ENV" || true; }
save_state() {
  mkdir -p "$STATE_DIR"
  {
    echo "LAB_PROJECT_ID='${LAB_PROJECT_ID:-}'"
    echo "LAB_TOKEN='${LAB_TOKEN:-}'"
    echo "LAB_RUNNERS='${LAB_RUNNERS:-}'"
    echo "LAB_WINDOWS_RUNNERS='${LAB_WINDOWS_RUNNERS:-0}'"
    echo "LAB_TARGETS='${LAB_TARGETS:-}'"
    echo "LAB_UI='${LAB_UI:-0}'"
    echo "LAB_NETEM='${LAB_NETEM:-}'"
    echo "LAB_AGENTS_VIA_UI='${LAB_AGENTS_VIA_UI:-0}'"
    echo "LAB_BASE_URL='${BASE_URL}'"
    echo "LAB_NET_PREFIX='${LAB_NET_PREFIX}'"
    echo "LAB_INSTANCE='${LAB_INSTANCE}'"
    echo "LAB_COMPOSE_PROJECT='${LAB_PROJECT}'"
    echo "LAB_WINDOWS_USER='${LAB_WINDOWS_USER}'"
    echo "LAB_WINDOWS_PASSWORD='${LAB_WINDOWS_PASSWORD}'"
    echo "LAB_WINDOWS_TIMEOUT='${LAB_WINDOWS_TIMEOUT}'"
  } > "$STATE_ENV"
}
# Runners are runner-1..LAB_RUNNERS; the LAST LAB_WINDOWS_RUNNERS of them are
# Windows VMs (runner-(N-M+1)..runner-N), the rest Linux containers.
linux_runner_count() { echo $(( ${LAB_RUNNERS:-0} - ${LAB_WINDOWS_RUNNERS:-0} )); }
runner_os() { # runner_os K → linux | windows
  if [ "$1" -gt "$(linux_runner_count)" ]; then echo windows; else echo linux; fi
}
has_windows_runner() { [ "${LAB_WINDOWS_RUNNERS:-0}" -ge 1 ]; }
runner_key_file() { echo "$STATE_DIR/runner-$1.key"; }
runner_key() { # runner_key N → stable per-runner key (minted once, reused across ups)
  local f; f="$(runner_key_file "$1")"
  if [ ! -s "$f" ]; then
    mkdir -p "$STATE_DIR"
    LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 48 > "$f"
  fi
  cat "$f"
}

# ── HTTP helpers ─────────────────────────────────────────────────────────────
api() { # api METHOD PATH [json-body]  (uses $LAB_TOKEN)
  local method="$1" path="$2" body="${3:-}"
  if [ -n "$body" ]; then
    curl -sS --max-time 60 -X "$method" -H "Authorization: Bearer ${LAB_TOKEN:-}" \
      -H 'Content-Type: application/json' -d "$body" "$BASE_URL$path"
  else
    curl -sS --max-time 60 -X "$method" -H "Authorization: Bearer ${LAB_TOKEN:-}" "$BASE_URL$path"
  fi
}
login() { # login EMAIL PASSWORD → prints token (empty on failure)
  curl -sS --max-time 15 "$BASE_URL/api/auth/login" -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg e "$1" --arg p "$2" '{email:$e,password:$p}')" 2>/dev/null \
    | jq -r '.token // empty' 2>/dev/null || true
}
authenticate() {
  # First boot: the bootstrap admin must change its password before any API
  # call succeeds (UserStatusMiddleware). Try the final password first.
  local t
  t="$(login "$LAB_ADMIN_EMAIL" "$LAB_ADMIN_PASSWORD")"
  if [ -z "$t" ]; then
    t="$(login "$LAB_ADMIN_EMAIL" "$LAB_ADMIN_BOOTSTRAP_PASSWORD")"
    [ -n "$t" ] || die "login failed for $LAB_ADMIN_EMAIL (bootstrap and final passwords) — is the control plane up? ($BASE_URL)"
    local resp
    resp="$(curl -sS --max-time 15 -X POST "$BASE_URL/api/auth/change-password" \
      -H "Authorization: Bearer $t" -H 'Content-Type: application/json' \
      -d "$(jq -nc --arg c "$LAB_ADMIN_BOOTSTRAP_PASSWORD" --arg n "$LAB_ADMIN_PASSWORD" '{current_password:$c,new_password:$n}')")"
    echo "$resp" | grep -q '"success":true' || die "change-password failed: $resp"
    t="$(login "$LAB_ADMIN_EMAIL" "$LAB_ADMIN_PASSWORD")"
    [ -n "$t" ] || die "login failed after password change"
    ok "admin password set (${LAB_ADMIN_EMAIL})"
  fi
  LAB_TOKEN="$t"
  # Prove the token is usable before anything else parses API JSON (older
  # control planes kept answering 403 "Password change required" for up to 10s
  # after the change because of the status cache — fixed, but be tolerant).
  local i=0 body
  while :; do
    body="$(api GET /api/projects 2>/dev/null || true)"
    case "$body" in \{*|\[*) return 0;; esac
    i=$((i + 1))
    [ "$i" -ge 15 ] && die "authenticated API call keeps failing: ${body:-<empty>}"
    sleep 1
  done
}
ensure_project() {
  local list pid
  list="$(api GET /api/projects)"
  pid="$(jq -r --arg n "$LAB_PROJECT_NAME" '(.projects // .) as $p | ([$p[]|select(.name==$n)][0].project_id // empty)' <<<"$list" 2>/dev/null || true)"
  if [ -z "$pid" ]; then
    local created
    created="$(api POST /api/projects "$(jq -nc --arg n "$LAB_PROJECT_NAME" '{name:$n,description:"docker lab — runners/targets are local containers"}')")"
    pid="$(jq -r '.project_id // empty' <<<"$created")"
    [ -n "$pid" ] || die "project create failed: $created"
    ok "project '${LAB_PROJECT_NAME}' created ($pid)"
  else
    ok "project '${LAB_PROJECT_NAME}' ($pid)"
  fi
  LAB_PROJECT_ID="$pid"
}

# ── DB helpers (psql inside the postgres container) ──────────────────────────
psql_q() { # psql_q SQL  (quiet, tuples only)
  "${COMPOSE[@]}" exec -T postgres psql -U networker -d networker_core -v ON_ERROR_STOP=1 -qtA -c "$1"
}
psql_stdin() { "${COMPOSE[@]}" exec -T postgres psql -U networker -d networker_core -v ON_ERROR_STOP=1 -q; }

# Register runner N as a standalone agent (no project_tester/VM behind it) —
# the same INSERT the create-tester path performs (TesterWriteEndpoints.Create),
# minus the cloud VM. Only the SHA-256 of the key is stored (V040/V045).
#
# A WINDOWS runner (VM) additionally gets a project_tester row (cloud docker /
# region lab, power_state running, requested_os windows, auto-shutdown off)
# and the agent is BOUND to it (agent.tester_id): the public launch API pins a
# run to a runner only through LaunchRequest.tester_id (a project_tester FK —
# RunDispatcher's tester affinity), which is how validate.sh proves the Windows
# runner executes the same modes as a Linux one. Linux runners stay standalone
# (validate's fan-out phase wants free spreading across them).
register_runner() {
  local n="$1" key name ip os
  key="$(runner_key "$n")"; name="runner-$n"; ip="${LAB_NET_PREFIX}.$((200 + n))"; os="$(runner_os "$n")"
  psql_stdin <<SQL
INSERT INTO agent (agent_id, name, api_key_hash, region, provider, project_id, status, tags)
VALUES (gen_random_uuid(), '${name}', encode(sha256(convert_to('${key}','UTF8')),'hex'),
        'lab', 'docker', '${LAB_PROJECT_ID}', 'offline',
        '{"lab":true,"ip":"${ip}","os":"${os}"}'::jsonb)
ON CONFLICT (api_key_hash) DO UPDATE SET name = EXCLUDED.name, project_id = EXCLUDED.project_id, tags = agent.tags || EXCLUDED.tags;
SQL
  if [ "$os" = windows ]; then
    psql_stdin <<SQL
INSERT INTO project_tester (project_id, name, cloud, region, vm_size, vm_name, public_ip, ssh_user,
                            power_state, allocation, status_message, auto_shutdown_enabled, auto_probe_enabled,
                            created_by, requested_os, requested_variant, os_distro, os_arch)
SELECT '${LAB_PROJECT_ID}', 'lab-${name}-windows', 'docker', 'lab', 'lab-vm', '${LAB_PROJECT}-${name}-1', '${ip}', '${LAB_WINDOWS_USER}',
       'running', 'idle', 'lab.sh: Windows Server VM runner (dockur/windows) — agent bound to this row for tester pinning',
       false, false,
       COALESCE((SELECT user_id FROM dash_user WHERE email = '${LAB_ADMIN_EMAIL}' LIMIT 1), '00000000-0000-0000-0000-000000000000'::uuid),
       'windows', 'server', 'windows-server-${LAB_WINDOWS_VERSION}', 'x86_64'
WHERE NOT EXISTS (SELECT 1 FROM project_tester WHERE project_id = '${LAB_PROJECT_ID}' AND name = 'lab-${name}-windows');
UPDATE agent SET tester_id = (SELECT tester_id FROM project_tester WHERE project_id = '${LAB_PROJECT_ID}' AND name = 'lab-${name}-windows')
WHERE api_key_hash = encode(sha256(convert_to('${key}','UTF8')),'hex');
SQL
  fi
}
# Register target N (stack != rust) as a COMPLETED deployment so test configs
# of kind "proxy" (proxy_endpoint_id = deployment id, proxy_stack = <stack>)
# resolve to <target ip>:<stack https port> exactly like a cloud endpoint VM.
# The Windows target registers as stack "iis" (its proxy stack) with os windows
# AND with endpoint_hosts = its labnet name (V050) — like a cloud Windows VM
# whose deploy recorded the Azure/AWS DNS name — so the resolver dispatches
# the run to target-N.lab (TLS SNI) and IIS's SNI binding serves HTTP/3. Linux
# targets stay IP-addressed here (they don't need SNI; prod records their
# cloud FQDN the same way, so nothing differs on the resolver side).
register_target_deployment() {
  local n="$1" stack="$2" ip name id os pstack hosts_json
  ip="$(target_ip "$n")"; pstack="$(proxy_stack_of_target "$stack")"; name="lab-target-${n}-${pstack}"
  os="ubuntu-24.04"; hosts_json="NULL"
  if [ "$stack" = windows ]; then os="windows"; hosts_json="'[\"$(target_host "$n")\"]'::jsonb"; fi
  id="$(psql_q "SELECT deployment_id FROM deployment WHERE name='${name}' AND project_id='${LAB_PROJECT_ID}' LIMIT 1" || true)"
  if [ -z "$id" ]; then
    psql_stdin <<SQL
INSERT INTO deployment (deployment_id, name, status, config, endpoint_ips, endpoint_hosts, project_id, created_at, started_at, finished_at, log)
VALUES (gen_random_uuid(), '${name}', 'completed',
        '{"lab":true,"tester":{"provider":"local"},"endpoints":[{"label":"target-${n}","provider":"lan","lan":{"ip":"${ip}","user":"lab"},"http_stacks":["${pstack}"],"os":"${os}"}]}'::jsonb,
        '["${ip}"]'::jsonb, ${hosts_json}, '${LAB_PROJECT_ID}', now(), now(), now(),
        'seeded by lab.sh — docker target ${n} (${stack} → ${pstack}) at ${ip}$([ "$stack" = windows ] && echo " / $(target_host "$n")")');
SQL
    id="$(psql_q "SELECT deployment_id FROM deployment WHERE name='${name}' AND project_id='${LAB_PROJECT_ID}' LIMIT 1")"
  else
    # A row seeded by an older lab.sh (pre-V050) has no hostname — bring it up to date.
    psql_q "UPDATE deployment SET endpoint_hosts = ${hosts_json} WHERE deployment_id='${id}'" >/dev/null
  fi
  echo "$id"
}

# ── Topology generation ──────────────────────────────────────────────────────
# Runners: runner-1..N at .201+; targets: target-1..M at .101+.
stack_of() { echo "$LAB_TARGETS" | tr ',' '\n' | sed -n "${1}p"; }
target_count() { [ -z "${LAB_TARGETS:-}" ] && echo 0 || echo "$LAB_TARGETS" | tr ',' '\n' | grep -c .; }
# The proxy stack a target is registered/validated as: windows → iis, else itself.
proxy_stack_of_target() { case "$1" in windows) echo iis;; *) echo "$1";; esac; }
target_ip() { echo "${LAB_NET_PREFIX}.$((100 + $1))"; }
target_host() { echo "target-$1.${LAB_TARGET_DOMAIN}"; }   # labnet DNS alias of target N
normalize_stacks() { echo "$1" | tr ',' '\n' | sed 's/^iis$/windows/' | grep . | paste -sd, -; }
validate_stacks() {
  local s
  for s in $(echo "$1" | tr ',' ' '); do
    case "$s" in rust|nginx|caddy|apache|haproxy|traefik|windows) ;; *) die "unknown target stack '$s' (rust|nginx|caddy|apache|haproxy|traefik|windows)";; esac
  done
}
has_windows_target() { echo ",${LAB_TARGETS:-}," | grep -q ',windows,'; }
windows_ok() { # KVM-backed VMs need Linux + /dev/kvm (Docker Desktop on macOS/Windows can't); dockerd (root) opens it
  [ "$(uname -s)" = Linux ] && [ -e /dev/kvm ]
}
windows_gen_dir() { echo "$GEN_DIR/windows-$1"; }                  # target N
windows_runner_gen_dir() { echo "$GEN_DIR/windows-runner-$1"; }    # runner K
# dockur web console host ports: targets from LAB_WINDOWS_VIEWER_PORT (first
# windows target = base, next +1, …); runners at base+10+K-1.
windows_target_viewer_port() { # windows_target_viewer_port N → the N-th target's console port
  local i=1 idx=0 st
  while [ "$i" -le "$1" ]; do st="$(stack_of "$i")"; [ "$st" = windows ] && idx=$((idx + 1)); i=$((i + 1)); done
  [ "$idx" -lt 1 ] && idx=1
  echo $((LAB_WINDOWS_VIEWER_PORT + idx - 1))
}
windows_runner_viewer_port() { echo $((LAB_WINDOWS_VIEWER_PORT + 10 + $1 - 1)); }
# The Windows runner's agent: Networker.Agent published win-x64 self-contained
# FROM THIS CHECKOUT (the release asset's shape) into the runner's /oem/agent —
# inside the dotnet SDK image by default (BuildKit --output; no host toolchain),
# or with your own dotnet when LAB_WIN_AGENT_BUILD=host.
publish_windows_agent() { # publish_windows_agent DEST_DIR
  local dest="$1"
  rm -rf "$dest"; mkdir -p "$dest"
  if [ "$LAB_WIN_AGENT_BUILD" = host ]; then
    need dotnet "install the .NET 10 SDK or unset LAB_WIN_AGENT_BUILD"
    note "publishing Networker.Agent (win-x64, self-contained) with the host dotnet → ${dest#$REPO_ROOT/}"
    dotnet publish "$REPO_ROOT/src/Networker.Agent/Networker.Agent.csproj" -c Release -r win-x64 --self-contained true -o "$dest" --nologo -v q < /dev/null
  else
    note "publishing Networker.Agent (win-x64, self-contained) in the dotnet SDK image → ${dest#$REPO_ROOT/}"
    docker build -f "$LAB_DIR/images/agent-win.Dockerfile" --target export --output "type=local,dest=$dest" "$REPO_ROOT"
  fi
  [ -s "$dest/networker-agent.exe" ] || die "Networker.Agent win-x64 publish produced no networker-agent.exe in $dest"
  ok "networker-agent.exe ($(du -h "$dest/networker-agent.exe" | cut -f1)) published from the checkout"
}
# Stage the /oem folder for Windows runner K: oem-runner scripts + THE
# CHECKOUT'S install.ps1 (-Component tester inside the VM → released
# networker-tester.exe) + the checkout's agent publish + lab.env.ps1 (the
# AGENT_* contract: control plane WS url, this runner's key, its name).
stage_windows_runner() {
  local k="$1" d ws_url; d="$(windows_runner_gen_dir "$k")"
  ws_url="ws://${LAB_NET_PREFIX}.10:5030/ws/agent"
  [ "${LAB_AGENTS_VIA_UI:-0}" = "1" ] && ws_url="ws://${LAB_NET_PREFIX}.11/ws/agent"
  mkdir -p "$d/oem" "$d/shared"
  # Windows PowerShell 5.1 reads BOM-less .ps1 files as ANSI: one non-ASCII
  # byte inside a string breaks the parse of the whole script (no SSH, no
  # agent, no log — 2026-08-16). Refuse to stage such a file.
  if LC_ALL=C grep -q '[^ -~[:space:]]' "$LAB_DIR/images/windows/oem-runner/lab-runner-setup.ps1"; then
    die "images/windows/oem-runner/lab-runner-setup.ps1 contains non-ASCII bytes (line $(LC_ALL=C grep -n '[^ -~[:space:]]' "$LAB_DIR/images/windows/oem-runner/lab-runner-setup.ps1" | head -1 | cut -d: -f1)) — Windows PowerShell 5.1 would fail to parse it"
  fi
  cp "$LAB_DIR/images/windows/oem-runner/"* "$d/oem/"
  cp "$REPO_ROOT/install.ps1" "$d/oem/install.ps1"
  {
    echo "# GENERATED by lab.sh"
    echo "\$LabUser           = \"${LAB_WINDOWS_USER}\""
    echo "\$LabPassword       = \"${LAB_WINDOWS_PASSWORD}\""
    echo "\$AgentDashboardUrl = \"${ws_url}\""
    echo "\$AgentApiKey       = \"$(runner_key "$k")\""
    echo "\$AgentName         = \"runner-${k}\""
  } > "$d/oem/lab.env.ps1"
  awk '{ sub(/\r$/, ""); printf "%s\r\n", $0 }' "$LAB_DIR/images/windows/oem-runner/install.bat" > "$d/oem/install.bat"
  # The agent publish is per checkout, not per runner: publish once per `up`
  # into runner-K's oem and hard-link/copy for the others.
  if [ -z "${WIN_AGENT_PUBLISHED:-}" ]; then
    publish_windows_agent "$d/oem/agent"; WIN_AGENT_PUBLISHED="$d/oem/agent"
  elif [ "$WIN_AGENT_PUBLISHED" != "$d/oem/agent" ]; then
    rm -rf "$d/oem/agent"; cp -R "$WIN_AGENT_PUBLISHED" "$d/oem/agent"
  fi
  chmod 0777 "$d/shared" 2>/dev/null || true
  case "$(windows_runner_status "$k")" in failed:*) rm -f "$d/shared/status";; esac
}
# Stage the /oem folder for target N: the repo's oem scripts + THE CHECKOUT'S
# install.ps1 (so what runs inside the VM is the installer you are shipping) +
# the generated lab.env.ps1 (password, stacks). /shared is where the VM writes
# its setup log + status for the host.
stage_windows_target() {
  local n="$1" d; d="$(windows_gen_dir "$n")"
  mkdir -p "$d/oem" "$d/shared"
  cp "$LAB_DIR/images/windows/oem/"* "$d/oem/"
  cp "$REPO_ROOT/install.ps1" "$d/oem/install.ps1"
  # The share (\\host.lan\Data inside the VM) also gets the checkout's install.ps1
  # + a refresh script, so an ALREADY-INSTALLED VM disk can be brought up to
  # date without reinstalling Windows (windows-iis-refresh).
  cp "$REPO_ROOT/install.ps1" "$d/shared/install.ps1"
  {
    echo "# GENERATED by lab.sh"
    echo "\$LabUser     = \"${LAB_WINDOWS_USER}\""
    echo "\$LabPassword = \"${LAB_WINDOWS_PASSWORD}\""
    echo "\$LabStacks   = \"iis\""
    echo "\$LabFqdn     = \"$(target_host "$n")\""
  } > "$d/oem/lab.env.ps1"
  {
    echo "# GENERATED by lab.sh — run inside the VM (windows-iis-refresh): the checkout's"
    echo "# install.ps1 -Setup iis with the labnet hostname (SNI binding + cert SAN → HTTP/3)."
    echo "Copy-Item '\\\\host.lan\\Data\\install.ps1' 'C:\\OEM\\install.ps1' -Force -ErrorAction SilentlyContinue"
    echo "& powershell.exe -ExecutionPolicy Bypass -NoProfile -NonInteractive -File 'C:\\OEM\\install.ps1' -Setup iis -Fqdn '$(target_host "$n")'"
    echo "\$rc = \$LASTEXITCODE"
    echo "# virtio USO breaks multi-packet QUIC sends on the dockur path (see lab-setup.ps1) — turn it off,"
    echo "# detached: the adapter reset drops this SSH session."
    echo "Start-Process powershell.exe -WindowStyle Hidden -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep 2; Get-NetAdapter -Physical | ForEach-Object { Set-NetAdapterUso -Name \$_.Name -IPv4Enabled \$false -IPv6Enabled \$false -ErrorAction SilentlyContinue }'"
    echo "exit \$rc"
  } > "$d/shared/iis-refresh.ps1"
  # cmd.exe wants CRLF in .bat files (git may have checked it out LF).
  awk '{ sub(/\r$/, ""); printf "%s\r\n", $0 }' "$LAB_DIR/images/windows/oem/install.bat" > "$d/oem/install.bat"
  # The VM's log/status files must be writable by the container's samba user.
  chmod 0777 "$d/shared" 2>/dev/null || true
  # A stale failed:* status from a previous attempt must not fail this `up`
  # before the (possibly re-run) setup had a chance to overwrite it.
  case "$(windows_status "$n")" in failed:*) rm -f "$d/shared/status";; esac
}
write_topology() {
  mkdir -p "$GEN_DIR"
  local n i stack ws_url
  ws_url="ws://controlplane:5030/ws/agent"
  [ "${LAB_AGENTS_VIA_UI:-0}" = "1" ] && ws_url="ws://ui/ws/agent"
  {
    echo "# GENERATED by lab.sh — do not edit (runners=${LAB_RUNNERS} targets=${LAB_TARGETS})"
    echo "services:"
    i=0
    for stack in $(echo "$LAB_TARGETS" | tr ',' ' '); do
      i=$((i + 1))
      if [ "$stack" = windows ]; then
        stage_windows_target "$i" >&2
        cat <<YML
  target-${i}:
    # Windows Server VM (dockur/windows: QEMU + KVM). The container's IP is the
    # VM's IP: dockur DNATs every TCP/UDP port (except its :8006 web console)
    # to the guest, so runners reach the endpoint/IIS at ${LAB_NET_PREFIX}.$((100 + i)).
    image: ${LAB_WINDOWS_IMAGE}
    hostname: target-${i}
    environment:
      VERSION: "${LAB_WINDOWS_VERSION}"
      RAM_SIZE: "${LAB_WINDOWS_RAM}"
      CPU_CORES: "${LAB_WINDOWS_CPUS}"
      DISK_SIZE: "${LAB_WINDOWS_DISK}"
      USERNAME: "${LAB_WINDOWS_USER}"
      PASSWORD: "${LAB_WINDOWS_PASSWORD}"
    devices: [/dev/kvm, /dev/net/tun]
    cap_add: [NET_ADMIN]
    ports:
      - "$(windows_target_viewer_port "$i"):8006"
    stop_grace_period: 2m
    volumes:
      - windows-storage-${i}:/storage
      - $(windows_gen_dir "$i")/oem:/oem
      - $(windows_gen_dir "$i")/shared:/shared
    networks:
      labnet:
        ipv4_address: ${LAB_NET_PREFIX}.$((100 + i))
        aliases: [$(target_host "$i")]
YML
        continue
      fi
      cat <<YML
  target-${i}:
    image: nwk-lab/target-${stack}:${LAB_IMAGE_TAG}
    build:
      context: ..
      dockerfile: lab/images/target.Dockerfile
      args:
        STACK: $([ "$stack" = rust ] && echo none || echo "$stack")
    hostname: target-${i}
    environment:
      TARGET_STACK: $([ "$stack" = rust ] && echo none || echo "$stack")
    networks:
      labnet:
        ipv4_address: ${LAB_NET_PREFIX}.$((100 + i))
        aliases: [$(target_host "$i")]
YML
    done
    n=1
    while [ "$n" -le "$LAB_RUNNERS" ]; do
      if [ "$(runner_os "$n")" = windows ]; then
        stage_windows_runner "$n" >&2   # its progress must not land in the topology file
        cat <<YML
  runner-${n}:
    # Windows Server VM RUNNER (dockur/windows: QEMU + KVM) — Networker.Agent
    # (published from this checkout) + the released networker-tester.exe,
    # installed by the checkout's install.ps1 on first boot (oem-runner/), the
    # agent as a SYSTEM ONSTART task dialling ${LAB_NET_PREFIX}.10:5030 with
    # runner-${n}'s key. The container's IP is the VM's IP (dockur DNAT).
    image: ${LAB_WINDOWS_IMAGE}
    hostname: runner-${n}
    environment:
      VERSION: "${LAB_WINDOWS_VERSION}"
      RAM_SIZE: "${LAB_WINDOWS_RUNNER_RAM}"
      CPU_CORES: "${LAB_WINDOWS_RUNNER_CPUS}"
      DISK_SIZE: "${LAB_WINDOWS_RUNNER_DISK}"
      USERNAME: "${LAB_WINDOWS_USER}"
      PASSWORD: "${LAB_WINDOWS_PASSWORD}"
    devices: [/dev/kvm, /dev/net/tun]
    cap_add: [NET_ADMIN]
    ports:
      - "$(windows_runner_viewer_port "$n"):8006"
    stop_grace_period: 2m
    depends_on:
      controlplane:
        condition: service_healthy
    volumes:
      - windows-runner-storage-${n}:/storage
      - $(windows_runner_gen_dir "$n")/oem:/oem
      - $(windows_runner_gen_dir "$n")/shared:/shared
    networks:
      labnet:
        ipv4_address: ${LAB_NET_PREFIX}.$((200 + n))
YML
        n=$((n + 1)); continue
      fi
      cat <<YML
  runner-${n}:
    image: nwk-lab/runner:${LAB_IMAGE_TAG}
    build:
      context: ..
      dockerfile: lab/images/runner.Dockerfile
    hostname: runner-${n}
    environment:
      AGENT_DASHBOARD_URL: ${ws_url}
      AGENT_API_KEY: "$(runner_key "$n")"
      AGENT_NAME: runner-${n}
      LAB_NETEM: "${LAB_NETEM:-}"
    cap_add: [NET_ADMIN, NET_RAW]
    sysctls:
      net.ipv4.ping_group_range: "0 2147483647"
    depends_on:
      controlplane:
        condition: service_healthy
    networks:
      labnet:
        ipv4_address: ${LAB_NET_PREFIX}.$((200 + n))
YML
      n=$((n + 1))
    done
    if has_windows_target || has_windows_runner; then
      # The Windows VM disks: a named volume per target/runner index (project-
      # prefixed: ${LAB_PROJECT}_windows-storage-N / _windows-runner-storage-K),
      # so a plain `down` + `up` boots the installed VM instead of reinstalling
      # (`down --volumes` wipes them).
      echo "volumes:"
      i=0
      for stack in $(echo "$LAB_TARGETS" | tr ',' ' '); do
        i=$((i + 1)); [ "$stack" = windows ] && echo "  windows-storage-${i}: {}"
      done
      n=1
      while [ "$n" -le "$LAB_RUNNERS" ]; do
        [ "$(runner_os "$n")" = windows ] && echo "  windows-runner-storage-${n}: {}"
        n=$((n + 1))
      done
    fi
  } > "$TOPOLOGY"
  COMPOSE=(docker compose -p "$LAB_PROJECT" -f "$LAB_DIR/docker-compose.yml" -f "$TOPOLOGY")
}

# ── Build ────────────────────────────────────────────────────────────────────
build_rustbin() {
  note "building Rust binaries (networker-tester + networker-endpoint) — first time takes a while"
  docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/rust.Dockerfile" -t nwk-lab/rustbin:${LAB_IMAGE_TAG} "$REPO_ROOT"
  ok "nwk-lab/rustbin:${LAB_IMAGE_TAG}"
}
build_images() { # build_images STACKS(csv) UI(0|1)
  local stacks="$1" ui="$2" s
  build_rustbin
  note "building control plane image"
  docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/controlplane.Dockerfile" -t nwk-lab/controlplane:${LAB_IMAGE_TAG} "$REPO_ROOT"
  note "building runner image (Networker.Agent + tester)"
  docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/runner.Dockerfile" --build-arg "RUSTBIN_IMAGE=nwk-lab/rustbin:${LAB_IMAGE_TAG}" -t nwk-lab/runner:${LAB_IMAGE_TAG} "$REPO_ROOT"
  local arg
  for s in $(echo "$stacks" | tr ',' ' ' | tr ' ' '\n' | sort -u); do
    [ "$s" = windows ] && continue          # windows = dockur/windows image, pulled at up
    arg="$s"; [ "$s" = rust ] && arg=none   # rust = bare endpoint, no proxy stack
    note "building target image for '$s'$([ "$arg" = none ] || echo " (install.sh --setup-stack $s runs at build time)")"
    docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/target.Dockerfile" --build-arg "RUSTBIN_IMAGE=nwk-lab/rustbin:${LAB_IMAGE_TAG}" --build-arg "STACK=$arg" -t "nwk-lab/target-$s:${LAB_IMAGE_TAG}" "$REPO_ROOT"
  done
  if [ "$ui" = "1" ]; then
    note "building UI image (dashboard SPA + nginx)"
    docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/ui.Dockerfile" -t nwk-lab/ui:${LAB_IMAGE_TAG} "$REPO_ROOT"
  fi
  ok "images built"
}
image_exists() { docker image inspect "$1" >/dev/null 2>&1; }

# ── Waiters ──────────────────────────────────────────────────────────────────
wait_controlplane() {
  note "waiting for control plane at $BASE_URL (≤${LAB_STARTUP_TIMEOUT}s)"
  local deadline=$((SECONDS + LAB_STARTUP_TIMEOUT))
  while :; do
    if curl -fsS --max-time 3 "$BASE_URL/api/health/ready" >/dev/null 2>&1; then ok "control plane ready"; return 0; fi
    [ "$SECONDS" -ge "$deadline" ] && { "${COMPOSE[@]}" logs --tail 60 controlplane || true; die "control plane not ready after ${LAB_STARTUP_TIMEOUT}s"; }
    sleep 2
  done
}
wait_agents_online() { # wait_agents_online N — the LINUX runners runner-1..N (Windows VMs are waited for separately)
  local want="$1" deadline=$((SECONDS + LAB_AGENT_TIMEOUT)) online agents
  note "waiting for ${want} linux runner(s) to come online (≤${LAB_AGENT_TIMEOUT}s)"
  while :; do
    agents="$(api GET "/api/projects/${LAB_PROJECT_ID}/agents" 2>/dev/null || echo '[]')"
    online="$(jq -r --argjson n "$want" '(if type=="array" then . else (.agents // .items // []) end) | [.[]|select(.status=="online" and (.name|test("^runner-[0-9]+$")) and ((.name|ltrimstr("runner-")|tonumber) <= $n))] | length' <<<"$agents" 2>/dev/null || echo 0)"
    if [ "${online:-0}" -ge "$want" ]; then
      ok "${online} runner(s) online: $(jq -r '(if type=="array" then . else (.agents // .items // []) end) | [.[]|select(.status=="online")|"\(.name)@\(.version // "?")\(if .os then " " + .os else "" end)"] | join(", ")' <<<"$agents")"
      return 0
    fi
    [ "$SECONDS" -ge "$deadline" ] && { warn "only ${online}/${want} online — runner logs:"; "${COMPOSE[@]}" logs --tail 30 $(linux_runner_services) || true; die "runners did not come online"; }
    sleep 3
  done
}
windows_status() { # windows_status N → the target VM's status word (installing|endpoint|iis|rebooting|ready|failed:*) or "-"
  local f; f="$(windows_gen_dir "$1")/shared/status"
  [ -f "$f" ] && tr -d '\r\n' < "$f" || echo "-"
}
windows_runner_status() { # windows_runner_status K → the runner VM's status word (installing|tester|agent|ready|failed:*) or "-"
  local f; f="$(windows_runner_gen_dir "$1")/shared/status"
  [ -f "$f" ] && tr -d '\r\n' < "$f" || echo "-"
}
agent_online() { # agent_online NAME → 0 when the control plane lists that agent online
  api GET "/api/projects/${LAB_PROJECT_ID}/agents" 2>/dev/null \
    | jq -e --arg n "$1" '(if type=="array" then . else (.agents // .items // []) end) | any(.[]; .name==$n and .status=="online")' >/dev/null 2>&1
}
# Wait for Windows runner K: its agent online at the control plane (the only
# verdict that matters), with a progress line every 30 s (VM status word from
# the share, last dockur log line). First boot installs Windows + runs
# install.ps1 (tester) + starts the agent: ~15 min; a reboot ~1 min.
wait_windows_runner() {
  local k="$1" ip start deadline st last="" line
  ip="${LAB_NET_PREFIX}.$((200 + k))"; start=$SECONDS; deadline=$((SECONDS + LAB_WINDOWS_TIMEOUT))
  note "waiting for runner-${k} (windows) at ${ip}: agent online (≤${LAB_WINDOWS_TIMEOUT}s — first boot installs Windows + install.ps1 tester + the checkout's agent; console: http://127.0.0.1:$(windows_runner_viewer_port "$k") · log: ./lab/lab.sh windows-log runner-${k})"
  while :; do
    st="$(windows_runner_status "$k")"
    if agent_online "runner-${k}"; then
      ok "runner-${k} (windows) online at ${ip} (vm-status=${st}, $((SECONDS - start))s)"
      return 0
    fi
    case "$st" in failed:*)
      warn "runner-${k} (windows) reports '${st}' — see: ./lab/lab.sh windows-log runner-${k}"
      "$LAB_DIR/lab.sh" windows-log "runner-${k}" 2>/dev/null | tail -25 || true
      die "windows runner setup failed (${st})";;
    esac
    if [ "$SECONDS" -ge "$deadline" ]; then
      "$LAB_DIR/lab.sh" windows-log "runner-${k}" 2>/dev/null | tail -25 || true
      die "runner-${k} (windows) not online after ${LAB_WINDOWS_TIMEOUT}s (status=${st}) — raise LAB_WINDOWS_TIMEOUT or watch http://127.0.0.1:$(windows_runner_viewer_port "$k")"
    fi
    line="$(docker logs --tail 1 "${LAB_PROJECT}-runner-${k}-1" 2>&1 | tr -d '\r' | sed 's/\x1b\[[0-9;]*m//g' | tail -1 | cut -c1-90)"
    if [ "$line" != "$last" ] || [ $(( (SECONDS - start) % 60 )) -lt 30 ]; then
      printf '%s    … %4ds  vm-status=%-12s agent=offline  %s%s\n' "$C_D" "$((SECONDS - start))" "$st" "$line" "$C_0"
      last="$line"
    fi
    sleep 30
  done
}
windows_http_ok() { curl -fsS --max-time 4 "http://${LAB_NET_PREFIX}.$((100 + $1)):8080/health" >/dev/null 2>&1; }
windows_iis_ok()  { curl -fsSk --max-time 6 "https://${LAB_NET_PREFIX}.$((100 + $1)):8445/health" >/dev/null 2>&1; }
# Wait for Windows target N: endpoint :8080 AND IIS :8445 answering, with a
# progress line every 30 s (VM status word from the share, last dockur log
# line) — a first install takes 15-40 min, so this is deliberately chatty.
wait_windows_target() {
  local n="$1" ip start deadline st last="" line st_ep st_iis
  ip="${LAB_NET_PREFIX}.$((100 + n))"; start=$SECONDS; deadline=$((SECONDS + LAB_WINDOWS_TIMEOUT))
  note "waiting for target-${n} (windows) at ${ip}: endpoint :8080 + IIS :8445 (≤${LAB_WINDOWS_TIMEOUT}s — first boot installs Windows + runs install.ps1; console: http://127.0.0.1:$(windows_target_viewer_port "$n") · log: ./lab/lab.sh windows-log target-${n})"
  while :; do
    st_ep=0; st_iis=0
    windows_http_ok "$n" && st_ep=1
    [ "$st_ep" = 1 ] && windows_iis_ok "$n" && st_iis=1
    st="$(windows_status "$n")"
    if [ "$st_ep" = 1 ] && [ "$st_iis" = 1 ] && [ "$st" != rebooting ]; then
      ok "target-${n} (windows) ready at ${ip}: endpoint :8080 + IIS :8445 answering (status=${st}, $((SECONDS - start))s)"
      windows_ensure_iis_sni "$n"
      return 0
    fi
    case "$st" in failed:*)
      warn "target-${n} (windows) reports '${st}' — see: ./lab/lab.sh windows-log target-${n}"
      "$LAB_DIR/lab.sh" windows-log "target-${n}" 2>/dev/null | tail -25 || true
      die "windows target setup failed (${st})";;
    esac
    if [ "$SECONDS" -ge "$deadline" ]; then
      "$LAB_DIR/lab.sh" windows-log "target-${n}" 2>/dev/null | tail -25 || true
      die "target-${n} (windows) not ready after ${LAB_WINDOWS_TIMEOUT}s (endpoint=${st_ep} iis=${st_iis} status=${st}) — raise LAB_WINDOWS_TIMEOUT or watch http://127.0.0.1:$(windows_target_viewer_port "$n")"
    fi
    line="$(docker logs --tail 1 "${LAB_PROJECT}-target-${n}-1" 2>&1 | tr -d '\r' | sed 's/\x1b\[[0-9;]*m//g' | tail -1 | cut -c1-90)"
    if [ "$line" != "$last" ] || [ $(( (SECONDS - start) % 60 )) -lt 30 ]; then
      printf '%s    … %4ds  vm-status=%-12s endpoint=%s iis=%s  %s%s\n' "$C_D" "$((SECONDS - start))" "$st" "$st_ep" "$st_iis" "$line" "$C_0"
      last="$line"
    fi
    sleep 30
  done
}
# Does IIS on Windows target N serve the certificate whose SAN carries the
# target's labnet name? (install.ps1 -Setup iis -Fqdn creates the SNI binding
# and the SAN together, so the SAN is the observable proof of the binding.)
windows_iis_sni_ok() { # windows_iis_sni_ok N
  local ip fqdn; ip="$(target_ip "$1")"; fqdn="$(target_host "$1")"
  command -v openssl >/dev/null 2>&1 || return 0   # can't tell — assume fine
  echo | openssl s_client -connect "${ip}:8445" -servername "$fqdn" 2>/dev/null \
    | openssl x509 -noout -text 2>/dev/null | grep -q "DNS:${fqdn}"
}
# A VM disk installed by an older lab.sh (or with a different domain) has IIS
# bound without the hostname → re-run the installer's IIS setup with -Fqdn over
# SSH (sshpass in a throwaway alpine on labnet — nothing on the host needed);
# the same script turns the NIC's UDP Segmentation Offload off (lab-setup.ps1
# does it on fresh installs — needed for multi-packet QUIC through dockur).
windows_iis_refresh() { # windows_iis_refresh N
  local n="$1" ip fqdn net; ip="$(target_ip "$n")"; fqdn="$(target_host "$n")"
  net="$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{$k}}{{end}}' "nwk-lab-target-${n}-1" 2>/dev/null | head -1)"
  [ -n "$net" ] || net="nwk-lab_labnet"
  note "target-${n} (windows): re-running install.ps1 -Setup iis -Fqdn ${fqdn} inside the VM (SNI binding + cert SAN → HTTP/3; ~2-4 min)"
  [ -f "$(windows_gen_dir "$n")/shared/iis-refresh.ps1" ] || stage_windows_target "$n"
  # The VM's SSH shell is PowerShell; \\host.lan\Data is the /shared bind mount.
  local rcmd='powershell -ExecutionPolicy Bypass -NoProfile -NonInteractive -File \\host.lan\Data\iis-refresh.ps1'
  docker run --rm --network "$net" -e SSHPASS="$LAB_WINDOWS_PASSWORD" alpine:3 sh -c \
    "apk add -q --no-cache sshpass openssh-client >/dev/null 2>&1 && sshpass -e ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=20 -o ServerAliveInterval=5 -o ServerAliveCountMax=3 ${LAB_WINDOWS_USER}@${ip} '${rcmd}'" \
    2>&1 | tr -d '\r' | sed 's/^/    /' | grep -v '^ *$' | tail -40 || true
  local i=0
  while [ "$i" -lt 12 ]; do windows_iis_ok "$n" && break; sleep 5; i=$((i + 1)); done
}
windows_ensure_iis_sni() { # windows_ensure_iis_sni N — refresh once if the SNI/SAN name is missing, then report
  local n="$1"
  if windows_iis_sni_ok "$n"; then ok "target-${n} (windows) IIS :8445 serves the SNI/SAN name $(target_host "$n") (HTTP/3 reachable by hostname)"; return 0; fi
  warn "target-${n} (windows) IIS :8445 has no SNI binding for $(target_host "$n") (VM disk installed without a hostname) — refreshing"
  windows_iis_refresh "$n"
  if windows_iis_sni_ok "$n"; then ok "target-${n} (windows) IIS :8445 now serves the SNI/SAN name $(target_host "$n")"
  else warn "target-${n} (windows) IIS still has no SNI binding for $(target_host "$n") — h3 modes through iis will fail in validate; see ./lab/lab.sh windows-log target-${n} / windows-iis-refresh"; fi
}
wait_targets_healthy() {
  local i total; total="$(target_count)"; i=1
  while [ "$i" -le "$total" ]; do
    local deadline=$((SECONDS + 90)) st
    if [ "$(stack_of "$i")" = windows ]; then
      if [ "${LAB_WINDOWS_ASYNC:-0}" = 1 ]; then
        note "target-${i} (windows) left installing in the background (--windows-async) — later: ./lab/lab.sh wait-windows"
      else
        wait_windows_target "$i"
      fi
      i=$((i + 1)); continue
    fi
    while :; do
      st="$(docker inspect --format '{{.State.Health.Status}}' "${LAB_PROJECT}-target-${i}-1" 2>/dev/null || echo missing)"
      [ "$st" = "healthy" ] && break
      [ "$SECONDS" -ge "$deadline" ] && { "${COMPOSE[@]}" logs --tail 40 "target-${i}" || true; die "target-${i} not healthy (state=$st)"; }
      sleep 2
    done
    ok "target-${i} ($(stack_of "$i")) healthy at ${LAB_NET_PREFIX}.$((100 + i))"
    i=$((i + 1))
  done
}
runner_services() { local n=1 out=""; while [ "$n" -le "${LAB_RUNNERS:-0}" ]; do out="$out runner-$n"; n=$((n + 1)); done; echo "$out"; }
linux_runner_services() { local n=1 out=""; while [ "$n" -le "$(linux_runner_count)" ]; do out="$out runner-$n"; n=$((n + 1)); done; echo "$out"; }
wait_windows_runners() { # after the Linux runners: each Windows runner VM until its agent is online (or --windows-async)
  local n=1
  while [ "$n" -le "${LAB_RUNNERS:-0}" ]; do
    if [ "$(runner_os "$n")" = windows ]; then
      if [ "${LAB_WINDOWS_ASYNC:-0}" = 1 ]; then
        note "runner-${n} (windows) left installing in the background (--windows-async) — later: ./lab/lab.sh wait-windows"
      else
        wait_windows_runner "$n"
      fi
    fi
    n=$((n + 1))
  done
}
target_services() { local n=1 out="" t; t="$(target_count)"; while [ "$n" -le "$t" ]; do out="$out target-$n"; n=$((n + 1)); done; echo "$out"; }

# ── Commands ─────────────────────────────────────────────────────────────────
cmd_build() {
  local stacks="rust,nginx" ui=0
  BUILD_FLAGS=()
  while [ $# -gt 0 ]; do
    case "$1" in
      --no-cache) BUILD_FLAGS+=(--no-cache) ;;
      --stacks) shift; stacks="$1" ;;
      --ui) ui=1 ;;
      *) die "build: unknown flag $1" ;;
    esac; shift
  done
  # rustbin is required by runner/target; controlplane independent.
  validate_stacks "$stacks"
  build_images "$stacks" "$ui"
}

cmd_up() {
  load_state
  # --runners N = LINUX runners (as before); --windows-runners M adds M Windows
  # VM runners numbered after them (LAB_RUNNERS = N+M in the state file).
  local runners="$(( ${LAB_RUNNERS:-2} - ${LAB_WINDOWS_RUNNERS:-0} ))" win_runners="${LAB_WINDOWS_RUNNERS:-0}" targets="${LAB_TARGETS:-rust,nginx}" ui="${LAB_UI:-0}" no_build=0
  LAB_NETEM="${LAB_NETEM:-}"; LAB_AGENTS_VIA_UI="${LAB_AGENTS_VIA_UI:-0}"
  while [ $# -gt 0 ]; do
    case "$1" in
      --runners) shift; runners="$1" ;;
      --windows-runners) shift; win_runners="$1" ;;
      --targets) shift; targets="$1" ;;
      --ui) ui=1 ;;
      --netem) shift; LAB_NETEM="$1" ;;
      --agents-via-ui) LAB_AGENTS_VIA_UI=1 ;;
      --no-build) no_build=1 ;;
      --windows-async) LAB_WINDOWS_ASYNC=1 ;;
      *) die "up: unknown flag $1" ;;
    esac; shift
  done
  case "$runners" in ''|*[!0-9]*) die "--runners must be an integer";; esac
  case "$win_runners" in ''|*[!0-9]*) die "--windows-runners must be an integer";; esac
  [ $((runners + win_runners)) -ge 1 ] || die "--runners + --windows-runners must be ≥ 1"
  targets="$(normalize_stacks "$targets")"
  validate_stacks "$targets"
  # Windows target preflight: a KVM-backed VM only runs on Linux with /dev/kvm
  # (Docker Desktop on macOS/Windows does not expose KVM). Drop it with a
  # clear message rather than failing the whole lab.
  if echo ",$targets," | grep -q ',windows,' && ! windows_ok; then
    warn "target 'windows' skipped: it needs Linux with /dev/kvm (uname=$(uname -s), /dev/kvm missing). Docker Desktop on macOS/Windows cannot run it; on Linux load kvm_intel/kvm_amd or enable nested virtualisation."
    targets="$(echo "$targets" | tr ',' '\n' | grep -vx windows | paste -sd, -)"
    [ -n "$targets" ] || die "no targets left after dropping windows"
  fi
  if [ "$win_runners" -ge 1 ] && ! windows_ok; then
    warn "--windows-runners ${win_runners} skipped: Windows VM runners need Linux with /dev/kvm (uname=$(uname -s), /dev/kvm missing)."
    win_runners=0
    [ "$runners" -ge 1 ] || die "no runners left after dropping the windows runners"
  fi
  [ "$LAB_AGENTS_VIA_UI" = "1" ] && ui=1
  LAB_RUNNERS=$((runners + win_runners)); LAB_WINDOWS_RUNNERS="$win_runners"; LAB_TARGETS="$targets"; LAB_UI="$ui"

  need docker "https://docs.docker.com/get-docker/"; need curl "install curl"; need jq "brew install jq / apt install jq"
  docker info >/dev/null 2>&1 || die "docker daemon not running"
  docker compose version >/dev/null 2>&1 || die "docker compose v2 required"

  # Build what's missing (or everything with `lab.sh build`).
  BUILD_FLAGS=()
  if [ "$no_build" = 0 ]; then
    local missing=0 s
    image_exists nwk-lab/rustbin:${LAB_IMAGE_TAG} || missing=1
    image_exists nwk-lab/controlplane:${LAB_IMAGE_TAG} || missing=1
    image_exists nwk-lab/runner:${LAB_IMAGE_TAG} || missing=1
    for s in $(echo "$targets" | tr ',' ' ' | sort -u); do [ "$s" = windows ] || image_exists "nwk-lab/target-$s:${LAB_IMAGE_TAG}" || missing=1; done   # windows = pulled dockur image, not built
    [ "$ui" = 1 ] && { image_exists nwk-lab/ui:${LAB_IMAGE_TAG} || missing=1; }
    if [ "$missing" = 1 ]; then
      build_images "$targets" "$ui"
    else
      note "images present (use 'lab.sh build' to rebuild after code changes)"
    fi
  fi

  write_topology
  save_state

  note "starting postgres + control plane"
  local profiles=()
  [ "$ui" = 1 ] && profiles=(--profile ui)
  "${COMPOSE[@]}" ${profiles[@]+"${profiles[@]}"} up -d --remove-orphans postgres controlplane $([ "$ui" = 1 ] && echo ui || true)
  wait_controlplane
  authenticate
  ensure_project
  save_state

  # Register runners BEFORE they start: an agent that connects with an unknown
  # key gets 401 and, after enough retries, its IP is rate-limited (V044).
  local n=1
  while [ "$n" -le "$LAB_RUNNERS" ]; do register_runner "$n"; n=$((n + 1)); done
  ok "${runners} runner key(s) registered as standalone agents in project ${LAB_PROJECT_ID}$([ "$win_runners" -ge 1 ] && echo " + ${win_runners} windows runner(s) as agents bound to a project_tester row (pinnable via tester_id)")"

  # Register proxy targets as completed deployments (kind=proxy configs).
  local i=1 st dep
  while [ "$i" -le "$(target_count)" ]; do
    st="$(stack_of "$i")"
    if [ "$st" != rust ]; then
      dep="$(register_target_deployment "$i" "$st")"
      ok "target-${i} (${st}$([ "$st" = windows ] && echo ' → proxy stack iis, os windows')) registered as deployment ${dep}"
    fi
    i=$((i + 1))
  done
  if has_windows_target || has_windows_runner; then
    docker image inspect "$LAB_WINDOWS_IMAGE" >/dev/null 2>&1 || { note "pulling ${LAB_WINDOWS_IMAGE}"; docker pull "$LAB_WINDOWS_IMAGE" >/dev/null; }
    note "windows VM(s): user ${LAB_WINDOWS_USER} / ${LAB_WINDOWS_PASSWORD}; consoles http://127.0.0.1:${LAB_WINDOWS_VIEWER_PORT}+ (targets) / :$(windows_runner_viewer_port "$((runners + 1))")+ (runners); Windows Server ${LAB_WINDOWS_VERSION} eval (Microsoft licence terms apply)"
  fi

  note "starting $(target_count) target(s) + ${runners} linux runner(s)$([ "$win_runners" -ge 1 ] && echo " + ${win_runners} windows runner VM(s)")"
  "${COMPOSE[@]}" ${profiles[@]+"${profiles[@]}"} up -d --remove-orphans $(target_services) $(runner_services)
  wait_targets_healthy
  [ "$runners" -ge 1 ] && wait_agents_online "$runners"
  wait_windows_runners
  save_state
  cmd_status
}

cmd_scale() { cmd_up "$@"; }

cmd_status() {
  load_state
  [ -n "${LAB_TOKEN:-}" ] || { warn "lab not initialised — run 'lab.sh up'"; "${COMPOSE[@]}" ps 2>/dev/null || true; return 0; }
  # Refresh token silently if expired.
  if ! api GET /api/projects 2>/dev/null | grep -q project_id; then authenticate; save_state; fi
  echo
  printf '%s%s%s\n' "$C_B" "networker lab" "$C_0"
  printf '  control plane : %s   (health: %s)\n' "$BASE_URL" "$(curl -fsS --max-time 3 "$BASE_URL/api/health/ready" 2>/dev/null | head -c 60 || echo unreachable)"
  [ "${LAB_UI:-0}" = 1 ] && printf '  ui            : http://127.0.0.1:%s   (login %s / %s)\n' "$LAB_UI_PORT" "$LAB_ADMIN_EMAIL" "$LAB_ADMIN_PASSWORD"
  printf '  project       : %s (%s)\n' "$LAB_PROJECT_NAME" "${LAB_PROJECT_ID:-?}"
  printf '  instance      : %s  (compose project %s, state %s)\n' "$LAB_INSTANCE" "$LAB_PROJECT" "${STATE_DIR#$REPO_ROOT/}"
  printf '  network       : %s.0/24  cp=.10  targets=.101+  runners=.201+\n' "$LAB_NET_PREFIX"
  [ -n "${LAB_NETEM:-}" ] && printf '  netem         : %s\n' "$LAB_NETEM"
  echo
  printf '  %-12s %-9s %-16s %s\n' "TARGET" "STACK" "IP" "URLS (from inside the lab network)"
  local i=1 st ip port
  while [ "$i" -le "$(target_count)" ]; do
    st="$(stack_of "$i")"; ip="${LAB_NET_PREFIX}.$((100 + i))"
    case "$st" in rust) port=8443;; nginx) port=8444;; caddy) port=8454;; traefik) port=8455;; haproxy) port=8456;; apache) port=8457;; windows) port=8445;; esac
    if [ "$st" = windows ]; then
      printf '  %-12s %-9s %-16s http://%s:8080  https://%s:8443  iis https://%s:%s (SNI, h3)  [vm %s · console :%s]\n' "target-$i" "$st" "$ip" "$ip" "$ip" "$(target_host "$i")" "$port" "$(windows_status "$i")" "$(windows_target_viewer_port "$i")"
    else
      printf '  %-12s %-9s %-16s http://%s:8080  https://%s:%s\n' "target-$i" "$st" "$ip" "$ip" "$ip" "$port"
    fi
    i=$((i + 1))
  done
  echo
  printf '  %-12s %-9s %-16s %-8s %-10s %-22s %s\n' "RUNNER" "STATUS" "IP" "OS" "VERSION" "CAPABILITIES" "LAST HEARTBEAT"
  api GET "/api/projects/${LAB_PROJECT_ID}/agents" 2>/dev/null \
    | jq -r '(if type=="array" then . else (.agents // .items // []) end)
             | .[] | select(.name|startswith("runner-"))
             | "\(.name)\t\(.status)\t\(.tags.ip // "?")\t\(.os // .tags.os // "?")\t\(.version // "?")\t\(if .capabilities then "chrome=\(.capabilities.chrome) tshark=\(.capabilities.tshark)" else "-" end)\t\(.last_heartbeat // "-")\t\(if .tester_id then "tester " + .tester_id[0:8] else "" end)"' 2>/dev/null \
    | sort -t- -k2,2n | while IFS=$'\t' read -r nm stt ipp os ver caps hb tid; do
        printf '  %-12s %-9s %-16s %-8s %-10s %-22s %s %s\n' "$nm" "$stt" "$ipp" "$os" "$ver" "$caps" "$hb" "$tid"
      done
  local k=1
  while [ "$k" -le "${LAB_RUNNERS:-0}" ]; do
    [ "$(runner_os "$k")" = windows ] && printf '  %-12s windows VM: status=%s · console http://127.0.0.1:%s · log: ./lab/lab.sh windows-log runner-%s\n' "runner-$k" "$(windows_runner_status "$k")" "$(windows_runner_viewer_port "$k")" "$k"
    k=$((k + 1))
  done
  echo
  printf '%s  next: ./lab/lab.sh validate      (or open the UI / hit %s/api with the token from ./lab/lab.sh env)%s\n' "$C_D" "$BASE_URL" "$C_0"
}

cmd_logs()  { load_state; "${COMPOSE[@]}" logs --tail 200 "$@"; }
cmd_compose() { load_state; "${COMPOSE[@]}" "$@"; }   # raw docker compose with this instance's project/files/env
cmd_shell() { load_state; [ $# -ge 1 ] || die "shell <service>"; "${COMPOSE[@]}" exec "$1" bash; }
cmd_psql()  { load_state; "${COMPOSE[@]}" exec postgres psql -U networker -d networker_core "$@"; }
cmd_env()   { load_state; [ -f "$STATE_ENV" ] && cat "$STATE_ENV" || warn "no state yet"; }
cmd_tester() {
  load_state
  local r="${1:-}"; shift || true
  [ -n "$r" ] || die "tester <runner-N> -- <networker-tester args>"
  [ "${1:-}" = "--" ] && shift
  "${COMPOSE[@]}" exec "$r" /usr/local/bin/networker-tester "$@"
}
windows_target_index() { # windows_target_index [target-N] → N (default: first windows target)
  local arg="${1:-}" i st
  if [ -n "$arg" ]; then echo "$arg" | sed 's/^target-//'; return 0; fi
  i=1
  while [ "$i" -le "$(target_count)" ]; do
    st="$(stack_of "$i")"; [ "$st" = windows ] && { echo "$i"; return 0; }
    i=$((i + 1))
  done
  die "no windows target in this lab (LAB_TARGETS=${LAB_TARGETS:-})"
}
first_windows_runner_index() { # → K of the first windows runner, or ""
  local k=1
  while [ "$k" -le "${LAB_RUNNERS:-0}" ]; do [ "$(runner_os "$k")" = windows ] && { echo "$k"; return 0; }; k=$((k + 1)); done
  echo ""
}
# windows_vm_pick ARGS… → "target N" | "runner K": an explicit target-N /
# runner-K argument wins; else the first windows target, else the first windows runner.
windows_vm_pick() {
  local a k
  for a in "$@"; do
    case "$a" in target-[0-9]*) echo "target ${a#target-}"; return 0;; runner-[0-9]*) echo "runner ${a#runner-}"; return 0;; esac
  done
  if has_windows_target; then echo "target $(windows_target_index)"; return 0; fi
  k="$(first_windows_runner_index)"
  [ -n "$k" ] && { echo "runner $k"; return 0; }
  die "no windows target or runner in this lab (LAB_TARGETS=${LAB_TARGETS:-}, LAB_WINDOWS_RUNNERS=${LAB_WINDOWS_RUNNERS:-0})"
}
cmd_wait_windows() {
  load_state
  local i=1 any=0
  while [ "$i" -le "$(target_count)" ]; do
    [ "$(stack_of "$i")" = windows ] && { any=1; wait_windows_target "$i"; }
    i=$((i + 1))
  done
  i=1
  while [ "$i" -le "${LAB_RUNNERS:-0}" ]; do
    [ "$(runner_os "$i")" = windows ] && { any=1; wait_windows_runner "$i"; }
    i=$((i + 1))
  done
  [ "$any" = 1 ] || die "no windows target or runner in this lab (LAB_TARGETS=${LAB_TARGETS:-}, LAB_WINDOWS_RUNNERS=${LAB_WINDOWS_RUNNERS:-0})"
}
cmd_windows_log() {
  load_state
  local n follow=0 a d kind st logf svc port
  for a in "$@"; do [ "$a" = "-f" ] && follow=1; done
  set -- $(windows_vm_pick "$@"); kind="$1"; n="$2"
  if [ "$kind" = target ]; then
    d="$(windows_gen_dir "$n")/shared"; st="$(windows_status "$n")"; logf="$d/lab-setup.log"; svc="target-$n"; port="$(windows_target_viewer_port "$n")"
  else
    d="$(windows_runner_gen_dir "$n")/shared"; st="$(windows_runner_status "$n")"; logf="$d/lab-runner-setup.log"; svc="runner-$n"; port="$(windows_runner_viewer_port "$n")"
  fi
  printf '%s%s (windows) status: %s%s\n' "$C_B" "$svc" "$st" "$C_0"
  if [ ! -f "$logf" ]; then
    warn "no setup log yet at $logf — Windows is still installing (watch http://127.0.0.1:${port} or 'lab.sh logs ${svc}')"
    docker logs --tail 5 "${LAB_PROJECT}-${svc}-1" 2>&1 | tr -d '\r' || true
    [ "$follow" = 1 ] || return 0
    until [ -f "$logf" ]; do sleep 5; done
  fi
  if [ "$follow" = 1 ]; then tail -n 50 -f "$logf"; else tail -n 200 "$logf"; fi
}
cmd_windows_iis_refresh() {
  load_state
  local n; n="$(windows_target_index "$(echo "$*" | tr ' ' '\n' | grep '^target-' | head -1)")"
  windows_iis_refresh "$n"
  windows_iis_sni_ok "$n" && ok "IIS on target-${n} serves $(target_host "$n") (SNI/SAN)" || die "IIS on target-${n} still has no SNI binding for $(target_host "$n")"
}
cmd_windows_ssh() {
  load_state
  local n ip kind
  set -- $(windows_vm_pick "$@") "$@"; kind="$1"; n="$2"; shift 2
  if [ "$kind" = target ]; then ip="${LAB_NET_PREFIX}.$((100 + n))"; else ip="${LAB_NET_PREFIX}.$((200 + n))"; fi
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do shift; done
  [ "${1:-}" = "--" ] && shift
  note "ssh ${LAB_WINDOWS_USER}@${ip}  (${kind}-${n}; password: ${LAB_WINDOWS_PASSWORD}; shell is PowerShell)"
  exec ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "${LAB_WINDOWS_USER}@${ip}" "$@"
}
cmd_down() {
  load_state
  local vols=()
  [ "${1:-}" = "--volumes" ] && vols=(--volumes)
  "${COMPOSE[@]}" --profile ui down --remove-orphans ${vols[@]+"${vols[@]}"}
  if [ "${1:-}" = "--volumes" ]; then rm -rf "$STATE_DIR" "$GEN_DIR"; ok "lab removed (db + state wiped)"; else ok "lab stopped (state kept; 'down --volumes' wipes the DB)"; fi
}
cmd_validate() { load_state; export LAB_INSTANCE LAB_STATE_ENV="$STATE_ENV"; exec "$LAB_DIR/validate.sh" "$@"; }

# ── Main ─────────────────────────────────────────────────────────────────────
cmd="${1:-}"; shift || true
case "$cmd" in
  build)    cmd_build "$@" ;;
  up)       cmd_up "$@" ;;
  scale)    cmd_scale "$@" ;;
  validate) cmd_validate "$@" ;;
  status)   cmd_status ;;
  logs)     cmd_logs "$@" ;;
  compose)  cmd_compose "$@" ;;
  shell)    cmd_shell "$@" ;;
  psql)     cmd_psql "$@" ;;
  tester)   cmd_tester "$@" ;;
  wait-windows) cmd_wait_windows ;;
  windows-log)  cmd_windows_log "$@" ;;
  windows-ssh)  cmd_windows_ssh "$@" ;;
  windows-iis-refresh) cmd_windows_iis_refresh "$@" ;;
  env)      cmd_env ;;
  down)     cmd_down "$@" ;;
  ""|-h|--help|help) usage ;;
  *) die "unknown command '$cmd' (see --help)" ;;
esac
