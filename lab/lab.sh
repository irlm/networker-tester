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
#   ./lab/lab.sh validate                     # end-to-end run matrix (like the prod canary)
#   ./lab/lab.sh status | logs [svc] | shell <svc> | psql | down [--volumes]
#
# Portable: bash 3.2 (macOS), Linux; needs docker (compose v2), curl, jq.
# Nothing here touches your host toolchains — Rust + .NET + Node build inside
# Docker (linux/amd64 or linux/arm64, whatever your Docker runs natively).
# The optional `windows` target is a real Windows Server VM (dockur/windows,
# QEMU+KVM in a container) — Linux with /dev/kvm only; see README "Windows".
set -euo pipefail

LAB_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$LAB_DIR/.." && pwd)"
GEN_DIR="$LAB_DIR/.generated"
STATE_DIR="$LAB_DIR/.state"
TOPOLOGY="$GEN_DIR/topology.yml"
STATE_ENV="$STATE_DIR/lab.env"

# ── Tunables (env-overridable) ───────────────────────────────────────────────
LAB_NET_PREFIX="${LAB_NET_PREFIX:-172.31.100}"       # /24 for the lab network
LAB_CP_PORT="${LAB_CP_PORT:-5030}"                    # control plane on the host
LAB_UI_PORT="${LAB_UI_PORT:-8088}"                    # SPA on the host (with --ui)
LAB_PG_PORT="${LAB_PG_PORT:-55432}"                   # postgres on the host
LAB_ADMIN_EMAIL="${LAB_ADMIN_EMAIL:-admin@lab.local}"
LAB_ADMIN_BOOTSTRAP_PASSWORD="${LAB_ADMIN_BOOTSTRAP_PASSWORD:-LabBootstrap-Pass1!}"
LAB_ADMIN_PASSWORD="${LAB_ADMIN_PASSWORD:-LabAdmin-Pass1!}"   # set on first login (must_change_password)
LAB_PROJECT_NAME="${LAB_PROJECT_NAME:-Local Lab}"
LAB_STARTUP_TIMEOUT="${LAB_STARTUP_TIMEOUT:-240}"     # s for control plane readiness
LAB_AGENT_TIMEOUT="${LAB_AGENT_TIMEOUT:-120}"         # s for all runners to come online
# Windows (IIS) target — a Windows Server VM via dockur/windows (KVM). First
# boot downloads the eval ISO (~6 GB) + installs Windows + runs install.ps1
# (endpoint + IIS): 20-40 min. The disk lives in a named volume, so the next
# `up` boots the installed VM in ~1-2 min.
LAB_WINDOWS_IMAGE="${LAB_WINDOWS_IMAGE:-dockurr/windows:latest}"
LAB_WINDOWS_VERSION="${LAB_WINDOWS_VERSION:-2022}"     # dockur VERSION: 2022 | 2025 (Windows Server eval)
LAB_WINDOWS_RAM="${LAB_WINDOWS_RAM:-6G}"
LAB_WINDOWS_CPUS="${LAB_WINDOWS_CPUS:-4}"
LAB_WINDOWS_DISK="${LAB_WINDOWS_DISK:-40G}"
LAB_WINDOWS_USER="${LAB_WINDOWS_USER:-Docker}"
LAB_WINDOWS_PASSWORD="${LAB_WINDOWS_PASSWORD:-LabWindows-Pass1!}"   # local admin, SSH (lab only — NOT a secret)
LAB_WINDOWS_TIMEOUT="${LAB_WINDOWS_TIMEOUT:-3600}"     # s to wait for the VM's endpoint + IIS on first boot
LAB_WINDOWS_VIEWER_PORT="${LAB_WINDOWS_VIEWER_PORT:-8006}"  # dockur web viewer (VM console) on the host
export LAB_NET_PREFIX LAB_CP_PORT LAB_UI_PORT LAB_PG_PORT LAB_ADMIN_EMAIL LAB_ADMIN_BOOTSTRAP_PASSWORD

BASE_URL="http://127.0.0.1:${LAB_CP_PORT}"
COMPOSE=(docker compose -f "$LAB_DIR/docker-compose.yml")
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
  sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Commands:
  build [--no-cache] [--stacks a,b] [--ui]   Build images (rustbin, controlplane, runner, target-<stack>[, ui])
  up [--runners N] [--targets SPEC] [--ui]   Start everything, register runners, wait until online
     [--netem "delay 40ms 5ms"]              WAN emulation on runners (tc netem)
     [--agents-via-ui]                       Runners connect through the nginx UI proxy (needs --ui)
     [--no-build]                            Skip image build (default builds what is missing)
     [--windows-async]                       Don't wait for the Windows VM (use wait-windows later)
  validate [--modes m1,m2] [--runs N]        Drive real runs through the API + assert (see validate.sh)
  scale --runners N | --targets SPEC         Change topology (re-registers as needed)
  status                                     Runners/targets/agents/health at a glance
  logs [service] [-f]                        docker compose logs
  shell <service>                            bash inside a container
  psql                                       psql into the lab database
  tester <runner> -- <args>                  Run networker-tester directly inside a runner (bypasses the CP)
  wait-windows                               Block until the Windows target(s) serve :8080 + IIS :8445
  windows-log [target-N] [-f]                Show the Windows VM's setup log/status (from the host share)
  windows-ssh [target-N] [-- cmd]            SSH into the Windows VM (user/password printed)
  down [--volumes]                           Stop (and optionally wipe the DB + the Windows VM disk)
  env                                        Print the saved lab env (token, project id, urls)

Targets SPEC = comma list of stacks; one container each:
  rust      networker-endpoint only (8080/8443 + UDP 9997-9999)
  nginx|caddy|apache|haproxy|traefik
            networker-endpoint + that proxy set up by install.sh --setup-stack
  windows   (alias: iis) a Windows Server VM: install.ps1 endpoint (8080/8443)
            + IIS 8082/8445 with HTTP/3 — Linux + /dev/kvm only, first boot 20-40 min
  Default: rust,nginx        Runners default: 2
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
    echo "LAB_TARGETS='${LAB_TARGETS:-}'"
    echo "LAB_UI='${LAB_UI:-0}'"
    echo "LAB_NETEM='${LAB_NETEM:-}'"
    echo "LAB_AGENTS_VIA_UI='${LAB_AGENTS_VIA_UI:-0}'"
    echo "LAB_BASE_URL='${BASE_URL}'"
    echo "LAB_WINDOWS_USER='${LAB_WINDOWS_USER}'"
    echo "LAB_WINDOWS_PASSWORD='${LAB_WINDOWS_PASSWORD}'"
    echo "LAB_WINDOWS_TIMEOUT='${LAB_WINDOWS_TIMEOUT}'"
  } > "$STATE_ENV"
}
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
register_runner() {
  local n="$1" key name ip
  key="$(runner_key "$n")"; name="runner-$n"; ip="${LAB_NET_PREFIX}.$((200 + n))"
  psql_stdin <<SQL
INSERT INTO agent (agent_id, name, api_key_hash, region, provider, project_id, status, tags)
VALUES (gen_random_uuid(), '${name}', encode(sha256(convert_to('${key}','UTF8')),'hex'),
        'lab', 'docker', '${LAB_PROJECT_ID}', 'offline',
        '{"lab":true,"ip":"${ip}"}'::jsonb)
ON CONFLICT (api_key_hash) DO UPDATE SET name = EXCLUDED.name, project_id = EXCLUDED.project_id;
SQL
}
# Register target N (stack != rust) as a COMPLETED deployment so test configs
# of kind "proxy" (proxy_endpoint_id = deployment id, proxy_stack = <stack>)
# resolve to <target ip>:<stack https port> exactly like a cloud endpoint VM.
# The Windows target registers as stack "iis" (its proxy stack) with os windows.
register_target_deployment() {
  local n="$1" stack="$2" ip name id os pstack
  ip="${LAB_NET_PREFIX}.$((100 + n))"; pstack="$(proxy_stack_of_target "$stack")"; name="lab-target-${n}-${pstack}"
  os="ubuntu-24.04"; [ "$stack" = windows ] && os="windows"
  id="$(psql_q "SELECT deployment_id FROM deployment WHERE name='${name}' AND project_id='${LAB_PROJECT_ID}' LIMIT 1" || true)"
  if [ -z "$id" ]; then
    psql_stdin <<SQL
INSERT INTO deployment (deployment_id, name, status, config, endpoint_ips, project_id, created_at, started_at, finished_at, log)
VALUES (gen_random_uuid(), '${name}', 'completed',
        '{"lab":true,"tester":{"provider":"local"},"endpoints":[{"label":"target-${n}","provider":"lan","lan":{"ip":"${ip}","user":"lab"},"http_stacks":["${pstack}"],"os":"${os}"}]}'::jsonb,
        '["${ip}"]'::jsonb, '${LAB_PROJECT_ID}', now(), now(), now(),
        'seeded by lab.sh — docker target ${n} (${stack} → ${pstack}) at ${ip}');
SQL
    id="$(psql_q "SELECT deployment_id FROM deployment WHERE name='${name}' AND project_id='${LAB_PROJECT_ID}' LIMIT 1")"
  fi
  echo "$id"
}

# ── Topology generation ──────────────────────────────────────────────────────
# Runners: runner-1..N at .201+; targets: target-1..M at .101+.
stack_of() { echo "$LAB_TARGETS" | tr ',' '\n' | sed -n "${1}p"; }
target_count() { [ -z "${LAB_TARGETS:-}" ] && echo 0 || echo "$LAB_TARGETS" | tr ',' '\n' | grep -c .; }
# The proxy stack a target is registered/validated as: windows → iis, else itself.
proxy_stack_of_target() { case "$1" in windows) echo iis;; *) echo "$1";; esac; }
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
windows_gen_dir() { echo "$GEN_DIR/windows-$1"; }
# Stage the /oem folder for target N: the repo's oem scripts + THE CHECKOUT'S
# install.ps1 (so what runs inside the VM is the installer you are shipping) +
# the generated lab.env.ps1 (password, stacks). /shared is where the VM writes
# its setup log + status for the host.
stage_windows_target() {
  local n="$1" d; d="$(windows_gen_dir "$n")"
  mkdir -p "$d/oem" "$d/shared"
  cp "$LAB_DIR/images/windows/oem/"* "$d/oem/"
  cp "$REPO_ROOT/install.ps1" "$d/oem/install.ps1"
  {
    echo "# GENERATED by lab.sh"
    echo "\$LabUser     = \"${LAB_WINDOWS_USER}\""
    echo "\$LabPassword = \"${LAB_WINDOWS_PASSWORD}\""
    echo "\$LabStacks   = \"iis\""
  } > "$d/oem/lab.env.ps1"
  # cmd.exe wants CRLF in .bat files (git may have checked it out LF).
  awk '{ sub(/\r$/, ""); printf "%s\r\n", $0 }' "$LAB_DIR/images/windows/oem/install.bat" > "$d/oem/install.bat"
  # The VM's log/status files must be writable by the container's samba user.
  chmod 0777 "$d/shared" 2>/dev/null || true
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
        stage_windows_target "$i"
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
      - "${LAB_WINDOWS_VIEWER_PORT}:8006"
    stop_grace_period: 2m
    volumes:
      - windows-storage-${i}:/storage
      - $(windows_gen_dir "$i")/oem:/oem
      - $(windows_gen_dir "$i")/shared:/shared
    networks:
      labnet:
        ipv4_address: ${LAB_NET_PREFIX}.$((100 + i))
YML
        continue
      fi
      cat <<YML
  target-${i}:
    image: nwk-lab/target-${stack}:local
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
YML
    done
    n=1
    while [ "$n" -le "$LAB_RUNNERS" ]; do
      cat <<YML
  runner-${n}:
    image: nwk-lab/runner:local
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
    if has_windows_target; then
      # The Windows VM disk: a named volume per target index, so a plain
      # `down` + `up` boots the installed VM instead of reinstalling
      # (`down --volumes` wipes it).
      echo "volumes:"
      i=0
      for stack in $(echo "$LAB_TARGETS" | tr ',' ' '); do
        i=$((i + 1)); [ "$stack" = windows ] && echo "  windows-storage-${i}: {}"
      done
    fi
  } > "$TOPOLOGY"
  COMPOSE=(docker compose -f "$LAB_DIR/docker-compose.yml" -f "$TOPOLOGY")
}

# ── Build ────────────────────────────────────────────────────────────────────
build_rustbin() {
  note "building Rust binaries (networker-tester + networker-endpoint) — first time takes a while"
  docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/rust.Dockerfile" -t nwk-lab/rustbin:local "$REPO_ROOT"
  ok "nwk-lab/rustbin:local"
}
build_images() { # build_images STACKS(csv) UI(0|1)
  local stacks="$1" ui="$2" s
  build_rustbin
  note "building control plane image"
  docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/controlplane.Dockerfile" -t nwk-lab/controlplane:local "$REPO_ROOT"
  note "building runner image (Networker.Agent + tester)"
  docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/runner.Dockerfile" -t nwk-lab/runner:local "$REPO_ROOT"
  local arg
  for s in $(echo "$stacks" | tr ',' ' ' | tr ' ' '\n' | sort -u); do
    [ "$s" = windows ] && continue          # windows = dockur/windows image, pulled at up
    arg="$s"; [ "$s" = rust ] && arg=none   # rust = bare endpoint, no proxy stack
    note "building target image for '$s'$([ "$arg" = none ] || echo " (install.sh --setup-stack $s runs at build time)")"
    docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/target.Dockerfile" --build-arg "STACK=$arg" -t "nwk-lab/target-$s:local" "$REPO_ROOT"
  done
  if [ "$ui" = "1" ]; then
    note "building UI image (dashboard SPA + nginx)"
    docker build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} -f "$LAB_DIR/images/ui.Dockerfile" -t nwk-lab/ui:local "$REPO_ROOT"
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
wait_agents_online() { # wait_agents_online N
  local want="$1" deadline=$((SECONDS + LAB_AGENT_TIMEOUT)) online agents
  note "waiting for ${want} runner(s) to come online (≤${LAB_AGENT_TIMEOUT}s)"
  while :; do
    agents="$(api GET "/api/projects/${LAB_PROJECT_ID}/agents" 2>/dev/null || echo '[]')"
    online="$(jq -r '(if type=="array" then . else (.agents // .items // []) end) | [.[]|select(.status=="online" and (.name|startswith("runner-")))] | length' <<<"$agents" 2>/dev/null || echo 0)"
    if [ "${online:-0}" -ge "$want" ]; then
      ok "${online} runner(s) online: $(jq -r '(if type=="array" then . else (.agents // .items // []) end) | [.[]|select(.status=="online")|"\(.name)@\(.version // "?")"] | join(", ")' <<<"$agents")"
      return 0
    fi
    [ "$SECONDS" -ge "$deadline" ] && { warn "only ${online}/${want} online — runner logs:"; "${COMPOSE[@]}" logs --tail 30 $(runner_services) || true; die "runners did not come online"; }
    sleep 3
  done
}
windows_status() { # windows_status N → the VM's status word (installing|endpoint|iis|rebooting|ready|failed:*) or "-"
  local f; f="$(windows_gen_dir "$1")/shared/status"
  [ -f "$f" ] && tr -d '\r\n' < "$f" || echo "-"
}
windows_http_ok() { curl -fsS --max-time 4 "http://${LAB_NET_PREFIX}.$((100 + $1)):8080/health" >/dev/null 2>&1; }
windows_iis_ok()  { curl -fsSk --max-time 6 "https://${LAB_NET_PREFIX}.$((100 + $1)):8445/health" >/dev/null 2>&1; }
# Wait for Windows target N: endpoint :8080 AND IIS :8445 answering, with a
# progress line every 30 s (VM status word from the share, last dockur log
# line) — a first install takes 20-40 min, so this is deliberately chatty.
wait_windows_target() {
  local n="$1" ip start deadline st last="" line st_ep st_iis
  ip="${LAB_NET_PREFIX}.$((100 + n))"; start=$SECONDS; deadline=$((SECONDS + LAB_WINDOWS_TIMEOUT))
  note "waiting for target-${n} (windows) at ${ip}: endpoint :8080 + IIS :8445 (≤${LAB_WINDOWS_TIMEOUT}s — first boot installs Windows + runs install.ps1; console: http://127.0.0.1:${LAB_WINDOWS_VIEWER_PORT} · log: ./lab/lab.sh windows-log target-${n})"
  while :; do
    st_ep=0; st_iis=0
    windows_http_ok "$n" && st_ep=1
    [ "$st_ep" = 1 ] && windows_iis_ok "$n" && st_iis=1
    st="$(windows_status "$n")"
    if [ "$st_ep" = 1 ] && [ "$st_iis" = 1 ] && [ "$st" != rebooting ]; then
      ok "target-${n} (windows) ready at ${ip}: endpoint :8080 + IIS :8445 answering (status=${st}, $((SECONDS - start))s)"
      return 0
    fi
    case "$st" in failed:*)
      warn "target-${n} (windows) reports '${st}' — see: ./lab/lab.sh windows-log target-${n}"
      "$LAB_DIR/lab.sh" windows-log "target-${n}" 2>/dev/null | tail -25 || true
      die "windows target setup failed (${st})";;
    esac
    if [ "$SECONDS" -ge "$deadline" ]; then
      "$LAB_DIR/lab.sh" windows-log "target-${n}" 2>/dev/null | tail -25 || true
      die "target-${n} (windows) not ready after ${LAB_WINDOWS_TIMEOUT}s (endpoint=${st_ep} iis=${st_iis} status=${st}) — raise LAB_WINDOWS_TIMEOUT or watch http://127.0.0.1:${LAB_WINDOWS_VIEWER_PORT}"
    fi
    line="$(docker logs --tail 1 "nwk-lab-target-${n}-1" 2>&1 | tr -d '\r' | sed 's/\x1b\[[0-9;]*m//g' | tail -1 | cut -c1-90)"
    if [ "$line" != "$last" ] || [ $(( (SECONDS - start) % 60 )) -lt 30 ]; then
      printf '%s    … %4ds  vm-status=%-12s endpoint=%s iis=%s  %s%s\n' "$C_D" "$((SECONDS - start))" "$st" "$st_ep" "$st_iis" "$line" "$C_0"
      last="$line"
    fi
    sleep 30
  done
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
      st="$(docker inspect --format '{{.State.Health.Status}}' "nwk-lab-target-${i}-1" 2>/dev/null || echo missing)"
      [ "$st" = "healthy" ] && break
      [ "$SECONDS" -ge "$deadline" ] && { "${COMPOSE[@]}" logs --tail 40 "target-${i}" || true; die "target-${i} not healthy (state=$st)"; }
      sleep 2
    done
    ok "target-${i} ($(stack_of "$i")) healthy at ${LAB_NET_PREFIX}.$((100 + i))"
    i=$((i + 1))
  done
}
runner_services() { local n=1 out=""; while [ "$n" -le "${LAB_RUNNERS:-0}" ]; do out="$out runner-$n"; n=$((n + 1)); done; echo "$out"; }
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
  local runners="${LAB_RUNNERS:-2}" targets="${LAB_TARGETS:-rust,nginx}" ui="${LAB_UI:-0}" no_build=0
  LAB_NETEM="${LAB_NETEM:-}"; LAB_AGENTS_VIA_UI="${LAB_AGENTS_VIA_UI:-0}"
  while [ $# -gt 0 ]; do
    case "$1" in
      --runners) shift; runners="$1" ;;
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
  [ "$runners" -ge 1 ] || die "--runners must be ≥ 1"
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
  [ "$LAB_AGENTS_VIA_UI" = "1" ] && ui=1
  LAB_RUNNERS="$runners"; LAB_TARGETS="$targets"; LAB_UI="$ui"

  need docker "https://docs.docker.com/get-docker/"; need curl "install curl"; need jq "brew install jq / apt install jq"
  docker info >/dev/null 2>&1 || die "docker daemon not running"
  docker compose version >/dev/null 2>&1 || die "docker compose v2 required"

  # Build what's missing (or everything with `lab.sh build`).
  BUILD_FLAGS=()
  if [ "$no_build" = 0 ]; then
    local missing=0 s
    image_exists nwk-lab/rustbin:local || missing=1
    image_exists nwk-lab/controlplane:local || missing=1
    image_exists nwk-lab/runner:local || missing=1
    for s in $(echo "$targets" | tr ',' ' ' | sort -u); do image_exists "nwk-lab/target-$s:local" || missing=1; done
    [ "$ui" = 1 ] && { image_exists nwk-lab/ui:local || missing=1; }
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
  while [ "$n" -le "$runners" ]; do register_runner "$n"; n=$((n + 1)); done
  ok "${runners} runner key(s) registered as standalone agents in project ${LAB_PROJECT_ID}"

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
  if has_windows_target; then
    docker image inspect "$LAB_WINDOWS_IMAGE" >/dev/null 2>&1 || { note "pulling ${LAB_WINDOWS_IMAGE}"; docker pull "$LAB_WINDOWS_IMAGE" >/dev/null; }
    note "windows target: VM user ${LAB_WINDOWS_USER} / ${LAB_WINDOWS_PASSWORD}; console http://127.0.0.1:${LAB_WINDOWS_VIEWER_PORT}; Windows Server ${LAB_WINDOWS_VERSION} eval (Microsoft licence terms apply)"
  fi

  note "starting $(target_count) target(s) + ${runners} runner(s)"
  "${COMPOSE[@]}" ${profiles[@]+"${profiles[@]}"} up -d --remove-orphans $(target_services) $(runner_services)
  wait_targets_healthy
  wait_agents_online "$runners"
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
  printf '  network       : %s.0/24  cp=.10  targets=.101+  runners=.201+\n' "$LAB_NET_PREFIX"
  [ -n "${LAB_NETEM:-}" ] && printf '  netem         : %s\n' "$LAB_NETEM"
  echo
  printf '  %-12s %-9s %-16s %s\n' "TARGET" "STACK" "IP" "URLS (from inside the lab network)"
  local i=1 st ip port
  while [ "$i" -le "$(target_count)" ]; do
    st="$(stack_of "$i")"; ip="${LAB_NET_PREFIX}.$((100 + i))"
    case "$st" in rust) port=8443;; nginx) port=8444;; caddy) port=8454;; traefik) port=8455;; haproxy) port=8456;; apache) port=8457;; windows) port=8445;; esac
    if [ "$st" = windows ]; then
      printf '  %-12s %-9s %-16s http://%s:8080  https://%s:8443  iis https://%s:%s  [vm %s · console :%s]\n' "target-$i" "$st" "$ip" "$ip" "$ip" "$ip" "$port" "$(windows_status "$i")" "$LAB_WINDOWS_VIEWER_PORT"
    else
      printf '  %-12s %-9s %-16s http://%s:8080  https://%s:%s\n' "target-$i" "$st" "$ip" "$ip" "$ip" "$port"
    fi
    i=$((i + 1))
  done
  echo
  printf '  %-12s %-9s %-16s %-10s %s\n' "RUNNER" "STATUS" "IP" "VERSION" "LAST HEARTBEAT"
  api GET "/api/projects/${LAB_PROJECT_ID}/agents" 2>/dev/null \
    | jq -r '(if type=="array" then . else (.agents // .items // []) end)
             | .[] | select(.name|startswith("runner-"))
             | "\(.name)\t\(.status)\t\(.tags.ip // "?")\t\(.version // "?")\t\(.last_heartbeat // "-")"' 2>/dev/null \
    | sort -t- -k2,2n | while IFS=$'\t' read -r nm stt ipp ver hb; do
        printf '  %-12s %-9s %-16s %-10s %s\n' "$nm" "$stt" "$ipp" "$ver" "$hb"
      done
  echo
  printf '%s  next: ./lab/lab.sh validate      (or open the UI / hit %s/api with the token from ./lab/lab.sh env)%s\n' "$C_D" "$BASE_URL" "$C_0"
}

cmd_logs()  { load_state; "${COMPOSE[@]}" logs --tail 200 "$@"; }
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
cmd_wait_windows() {
  load_state
  local i=1 any=0
  while [ "$i" -le "$(target_count)" ]; do
    [ "$(stack_of "$i")" = windows ] && { any=1; wait_windows_target "$i"; }
    i=$((i + 1))
  done
  [ "$any" = 1 ] || die "no windows target in this lab (LAB_TARGETS=${LAB_TARGETS:-})"
}
cmd_windows_log() {
  load_state
  local n follow=0 a d
  for a in "$@"; do [ "$a" = "-f" ] && follow=1; done
  n="$(windows_target_index "$(echo "$*" | tr ' ' '\n' | grep '^target-' | head -1)")"
  d="$(windows_gen_dir "$n")/shared"
  printf '%starget-%s (windows) status: %s%s\n' "$C_B" "$n" "$(windows_status "$n")" "$C_0"
  if [ ! -f "$d/lab-setup.log" ]; then
    warn "no setup log yet at $d/lab-setup.log — Windows is still installing (watch http://127.0.0.1:${LAB_WINDOWS_VIEWER_PORT} or 'lab.sh logs target-${n}')"
    docker logs --tail 5 "nwk-lab-target-${n}-1" 2>&1 | tr -d '\r' || true
    [ "$follow" = 1 ] || return 0
    until [ -f "$d/lab-setup.log" ]; do sleep 5; done
  fi
  if [ "$follow" = 1 ]; then tail -n 50 -f "$d/lab-setup.log"; else tail -n 200 "$d/lab-setup.log"; fi
}
cmd_windows_ssh() {
  load_state
  local n ip
  n="$(windows_target_index "$(echo "$*" | tr ' ' '\n' | grep '^target-' | head -1)")"
  ip="${LAB_NET_PREFIX}.$((100 + n))"
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do shift; done
  [ "${1:-}" = "--" ] && shift
  note "ssh ${LAB_WINDOWS_USER}@${ip}  (password: ${LAB_WINDOWS_PASSWORD}; shell is PowerShell)"
  exec ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "${LAB_WINDOWS_USER}@${ip}" "$@"
}
cmd_down() {
  load_state
  local vols=()
  [ "${1:-}" = "--volumes" ] && vols=(--volumes)
  "${COMPOSE[@]}" --profile ui down --remove-orphans ${vols[@]+"${vols[@]}"}
  if [ "${1:-}" = "--volumes" ]; then rm -rf "$STATE_DIR" "$GEN_DIR"; ok "lab removed (db + state wiped)"; else ok "lab stopped (state kept; 'down --volumes' wipes the DB)"; fi
}
cmd_validate() { load_state; exec "$LAB_DIR/validate.sh" "$@"; }

# ── Main ─────────────────────────────────────────────────────────────────────
cmd="${1:-}"; shift || true
case "$cmd" in
  build)    cmd_build "$@" ;;
  up)       cmd_up "$@" ;;
  scale)    cmd_scale "$@" ;;
  validate) cmd_validate "$@" ;;
  status)   cmd_status ;;
  logs)     cmd_logs "$@" ;;
  shell)    cmd_shell "$@" ;;
  psql)     cmd_psql "$@" ;;
  tester)   cmd_tester "$@" ;;
  wait-windows) cmd_wait_windows ;;
  windows-log)  cmd_windows_log "$@" ;;
  windows-ssh)  cmd_windows_ssh "$@" ;;
  env)      cmd_env ;;
  down)     cmd_down "$@" ;;
  ""|-h|--help|help) usage ;;
  *) die "unknown command '$cmd' (see --help)" ;;
esac
