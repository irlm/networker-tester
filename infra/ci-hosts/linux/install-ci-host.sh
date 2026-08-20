#!/usr/bin/env bash
# infra/ci-hosts/linux/install-ci-host.sh — turn a fresh Ubuntu 24.04 VM into
# an ephemeral CI host for this repo: what GitHub calls a "self-hosted runner"
# (labels self-hosted,linux,networker-ci). "Runner" in this repo means a
# tester VM, so everything we name here says "CI host"; GitHub's literal term
# survives only in its API paths and the actions-runner package.
#
# Runs as root, idempotent (re-run after a toolchain bump). Normally invoked by
# infra/ci-hosts/setup-ci-hosts.sh through cloud-init (docs/self-hosted-ci.md).
#
# What it installs (what the routed workflows need):
#   rustup stable + rustfmt + clippy + x86_64-unknown-linux-musl, .NET 10 SDK,
#   Node 22, Docker (services: containers, Testcontainers, the lab), shellcheck,
#   bats, jq, gh, pwsh (+PSScriptAnalyzer), musl-tools, tc/netem, Chromium's
#   shared libraries for Playwright, and the actions-runner package itself
#   under /opt/actions-runner.
#
# Registration: `--ephemeral` — the host de-registers after ONE job and the
# systemd unit re-registers it with a fresh registration token minted via
#   POST /repos/OWNER/REPO/actions/runners/registration-token
# using a PAT read from a root-only file (never exported to the job).
#
# Usage:
#   install-ci-host.sh --repo OWNER/REPO [--token-file /etc/ci-host/token]
#                      [--labels self-hosted,linux,networker-ci] [--name HOST]
#                      [--runner-group GROUP] [--runner-version X.Y.Z]
#                      [--user ci] [--skip-toolchains]
#
# Dev tooling: bash 4+, not subject to install.sh's bash-3.2 rule.

set -euo pipefail

REPO="${CI_HOST_REPO:-}"
TOKEN_FILE="${CI_HOST_TOKEN_FILE:-/etc/ci-host/token}"
LABELS="${CI_HOST_LABELS:-self-hosted,linux,networker-ci}"
NAME="${CI_HOST_NAME:-$(hostname -s)}"
GROUP="${CI_HOST_RUNNER_GROUP:-}"
RUNNER_VERSION="${CI_HOST_RUNNER_VERSION:-}"
CI_USER="${CI_HOST_USER:-ci}"
RUNNER_DIR="${CI_HOST_RUNNER_DIR:-/opt/actions-runner}"
CACHE_DIR="${CI_HOST_CACHE_DIR:-/var/cache/ci-host}"
SKIP_TOOLCHAINS=0
DOTNET_CHANNEL="${DOTNET_CHANNEL:-10.0}"
NODE_MAJOR="${NODE_MAJOR:-22}"

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="$2"; shift ;;
    --token-file) TOKEN_FILE="$2"; shift ;;
    --labels) LABELS="$2"; shift ;;
    --name) NAME="$2"; shift ;;
    --runner-group) GROUP="$2"; shift ;;
    --runner-version) RUNNER_VERSION="$2"; shift ;;
    --user) CI_USER="$2"; shift ;;
    --skip-toolchains) SKIP_TOOLCHAINS=1 ;;
    -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
  shift
done

log() { printf '\033[1;34m[install-ci-host]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[install-ci-host] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = 0 ] || die "run as root"
[ -n "$REPO" ] || die "--repo OWNER/REPO is required"
case "$REPO" in */*) ;; *) die "--repo must be OWNER/REPO, got '$REPO'" ;; esac
[ -s "$TOKEN_FILE" ] || die "PAT file $TOKEN_FILE is missing or empty (mode 0600, root-only; a PAT with repository Administration:write)"
chmod 0600 "$TOKEN_FILE"; chown root:root "$TOKEN_FILE"

export DEBIAN_FRONTEND=noninteractive
ARCH="$(dpkg --print-architecture)"
case "$ARCH" in
  amd64) RUNNER_ARCH=x64 ;;
  arm64) RUNNER_ARCH=arm64 ;;
  *) die "unsupported architecture $ARCH" ;;
esac

# ── 1. the CI user (passwordless sudo: the workflows apt-get / tc / systemctl) ─
if ! id "$CI_USER" >/dev/null 2>&1; then
  log "creating user $CI_USER"
  useradd --create-home --shell /bin/bash "$CI_USER"
fi
install -d -m 0755 /etc/sudoers.d
echo "$CI_USER ALL=(ALL) NOPASSWD:ALL" > "/etc/sudoers.d/90-$CI_USER"
chmod 0440 "/etc/sudoers.d/90-$CI_USER"
CI_HOME="$(getent passwd "$CI_USER" | cut -d: -f6)"

if [ "$SKIP_TOOLCHAINS" = 0 ]; then
  # ── 2. apt: base + the CI tools the workflows apt-get on hosted runners ────
  log "apt: base packages"
  apt-get update -qq
  apt-get install -y -qq --no-install-recommends \
    ca-certificates curl wget git gnupg lsb-release apt-transport-https \
    build-essential pkg-config libssl-dev musl-tools cmake clang lld \
    jq unzip zip tar xz-utils rsync openssl \
    shellcheck bats lsof iproute2 iputils-ping dnsutils net-tools \
    python3 python3-pip python3-venv \
    sudo systemd-timesyncd qemu-guest-agent \
    libicu74 libkrb5-3 zlib1g \
    fonts-liberation libasound2t64 libatk-bridge2.0-0 libatk1.0-0 libatspi2.0-0 \
    libcairo2 libcups2 libdbus-1-3 libdrm2 libgbm1 libglib2.0-0 libgtk-3-0 \
    libnspr4 libnss3 libpango-1.0-0 libx11-6 libxcb1 libxcomposite1 libxdamage1 \
    libxext6 libxfixes3 libxkbcommon0 libxrandr2 xvfb
  systemctl enable --now qemu-guest-agent 2>/dev/null || true

  # ── 3. Docker (services: containers, Testcontainers, lab/lab.sh) ──────────
  if ! command -v docker >/dev/null 2>&1; then
    log "installing docker (Ubuntu archive docker.io + compose v2 plugin)"
    apt-get install -y -qq docker.io docker-compose-v2 docker-buildx
  fi
  systemctl enable --now docker
  usermod -aG docker "$CI_USER"

  # ── 4. Node ${NODE_MAJOR} (NodeSource) ──────────────────────────────────────
  if ! command -v node >/dev/null 2>&1 || [ "$(node -p 'process.versions.node.split(".")[0]')" != "$NODE_MAJOR" ]; then
    log "installing node ${NODE_MAJOR}"
    install -d -m 0755 /etc/apt/keyrings
    curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor --yes -o /etc/apt/keyrings/nodesource.gpg
    echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_${NODE_MAJOR}.x nodistro main" > /etc/apt/sources.list.d/nodesource.list
    apt-get update -qq && apt-get install -y -qq nodejs
  fi

  # ── 5. Microsoft repo: pwsh (+ PSScriptAnalyzer for the lint sections) ─────
  if ! command -v pwsh >/dev/null 2>&1; then
    log "installing powershell"
    curl -fsSL "https://packages.microsoft.com/config/ubuntu/$(lsb_release -rs)/packages-microsoft-prod.deb" -o /tmp/msprod.deb
    dpkg -i /tmp/msprod.deb && rm -f /tmp/msprod.deb
    apt-get update -qq && apt-get install -y -qq powershell
  fi
  pwsh -NoProfile -Command 'if (-not (Get-Module -ListAvailable PSScriptAnalyzer)) { Install-Module PSScriptAnalyzer -Force -Scope AllUsers }' || true

  # ── 6. .NET ${DOTNET_CHANNEL} SDK (dotnet-install.sh → /usr/share/dotnet) ──
  if ! /usr/share/dotnet/dotnet --list-sdks 2>/dev/null | grep -q "^${DOTNET_CHANNEL}"; then
    log "installing .NET SDK ${DOTNET_CHANNEL}"
    curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh
    bash /tmp/dotnet-install.sh --channel "$DOTNET_CHANNEL" --install-dir /usr/share/dotnet
    rm -f /tmp/dotnet-install.sh
  fi
  ln -sf /usr/share/dotnet/dotnet /usr/local/bin/dotnet

  # ── 7. gh CLI (auto-tag + setup-ci-hosts.sh status) ────────────────────────
  if ! command -v gh >/dev/null 2>&1; then
    log "installing gh"
    install -d -m 0755 /etc/apt/keyrings
    curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg -o /etc/apt/keyrings/githubcli-archive-keyring.gpg
    chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg
    echo "deb [arch=${ARCH} signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" > /etc/apt/sources.list.d/github-cli.list
    apt-get update -qq && apt-get install -y -qq gh
  fi

  # ── 8. rustup for the CI user (stable + fmt/clippy + musl target) ──────────
  if [ ! -x "$CI_HOME/.cargo/bin/rustup" ]; then
    log "installing rustup for $CI_USER"
    runuser -u "$CI_USER" -- bash -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable'
  fi
  runuser -u "$CI_USER" -- bash -c '
    export PATH="$HOME/.cargo/bin:$PATH"
    rustup toolchain install stable --profile minimal --component rustfmt,clippy,llvm-tools-preview
    rustup target add x86_64-unknown-linux-musl
    rustup default stable
    rustc --version'
fi

# ── 9. persistent caches (survive ephemeral re-registration) ─────────────────
install -d -m 0755 "$CACHE_DIR"
for d in npm nuget dotnet-cli playwright; do install -d -o "$CI_USER" -g "$CI_USER" -m 0755 "$CACHE_DIR/$d"; done

# ── 10. the actions-runner package ───────────────────────────────────────────
if [ -z "$RUNNER_VERSION" ]; then
  RUNNER_VERSION="$(curl -fsSL https://api.github.com/repos/actions/runner/releases/latest | jq -r '.tag_name | ltrimstr("v")')"
  [ -n "$RUNNER_VERSION" ] && [ "$RUNNER_VERSION" != null ] || die "could not resolve the latest actions/runner version (pass --runner-version)"
fi
if [ ! -x "$RUNNER_DIR/run.sh" ] || ! grep -q "\"$RUNNER_VERSION\"" "$RUNNER_DIR/.runner-version" 2>/dev/null; then
  log "installing actions-runner ${RUNNER_VERSION} (${RUNNER_ARCH}) into $RUNNER_DIR"
  systemctl stop ci-host 2>/dev/null || true
  install -d -m 0755 "$RUNNER_DIR"
  tmp="$(mktemp -d)"
  curl -fsSL -o "$tmp/runner.tgz" "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-${RUNNER_ARCH}-${RUNNER_VERSION}.tar.gz"
  tar xzf "$tmp/runner.tgz" -C "$RUNNER_DIR"
  rm -rf "$tmp"
  printf '"%s"\n' "$RUNNER_VERSION" > "$RUNNER_DIR/.runner-version"
  "$RUNNER_DIR/bin/installdependencies.sh" >/dev/null
fi
chown -R "$CI_USER:$CI_USER" "$RUNNER_DIR"

# Environment every job inherits (the actions-runner reads RUNNER_DIR/.env).
# PATH is NOT set here: the runner rewrites RUNNER_DIR/.path from its own
# process PATH at every start and uses that for jobs, ignoring a PATH line in
# .env — so the loop below launches run.sh with JOB_PATH instead (seen as
# `cargo: command not found` in a job without a toolchain action).
cat > "$RUNNER_DIR/.env" <<ENV
DOTNET_ROOT=/usr/share/dotnet
DOTNET_CLI_TELEMETRY_OPTOUT=1
DOTNET_CLI_HOME=$CACHE_DIR/dotnet-cli
NUGET_PACKAGES=$CACHE_DIR/nuget
npm_config_cache=$CACHE_DIR/npm
PLAYWRIGHT_BROWSERS_PATH=$CACHE_DIR/playwright
CARGO_HOME=$CI_HOME/.cargo
RUSTUP_HOME=$CI_HOME/.rustup
ENV
chown "$CI_USER:$CI_USER" "$RUNNER_DIR/.env"

# The PATH a JOB sees comes from RUNNER_DIR/.path, not from .env and not from
# the Listener's own environment — the runner writes .path at configure time
# from whatever PATH it had then and reuses the file afterwards. A job that
# leans on a `setup-*` action never notices (those prepend via GITHUB_PATH),
# but a step calling the toolchain directly gets "cargo: command not found"
# (seen: validate-bench-apis' canonical Rust baseline). Write it explicitly.
printf '%s\n' "$CI_HOME/.cargo/bin:/usr/share/dotnet:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" > "$RUNNER_DIR/.path"
chown "$CI_USER:$CI_USER" "$RUNNER_DIR/.path"

# ── 11. ephemeral registration loop + systemd unit ───────────────────────────
cat > /usr/local/bin/ci-host-loop.sh <<'LOOP'
#!/usr/bin/env bash
# Mint a registration token with the root-only PAT, register --ephemeral, run
# ONE job, repeat. The PAT never leaves this process; the job only ever sees
# the effects of the short-lived registration token (a configured runner).
set -uo pipefail
: "${REPO:?}" "${RUNNER_DIR:?}" "${CI_USER:?}" "${TOKEN_FILE:?}" "${LABELS:?}" "${NAME:?}" "${JOB_PATH:?}" "${CACHE_DIR:?}"
GROUP_ARGS=()
[ -n "${GROUP:-}" ] && GROUP_ARGS=(--runnergroup "$GROUP")
while :; do
  pat="$(cat "$TOKEN_FILE")"
  reg="$(curl -sS --max-time 30 -X POST \
    -H "Authorization: Bearer ${pat}" -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "https://api.github.com/repos/${REPO}/actions/runners/registration-token" | jq -r '.token // empty')"
  unset pat
  if [ -z "$reg" ]; then
    echo "registration token mint failed — retrying in 60s" >&2
    sleep 60; continue
  fi
  # A Listener killed mid-flight (host reboot, operator restart) leaves its
  # registration files behind and config.sh then refuses with "already
  # configured" forever; the registration itself is ephemeral and --replace
  # takes care of the server side, so start every cycle clean.
  rm -f "$RUNNER_DIR/.runner" "$RUNNER_DIR/.credentials" "$RUNNER_DIR/.credentials_rsaparams"
  if ! runuser -u "$CI_USER" -- "$RUNNER_DIR/config.sh" --unattended --ephemeral --replace \
        --url "https://github.com/${REPO}" --token "$reg" \
        --name "$NAME" --labels "$LABELS" --work _work "${GROUP_ARGS[@]}" >/dev/null; then
    echo "config.sh failed — retrying in 30s" >&2
    sleep 30; continue
  fi
  # config.sh rewrites .path from its own environment, so restore it after
  # every registration — this is the file job steps inherit their PATH from.
  printf '%s\n' "$JOB_PATH" > "$RUNNER_DIR/.path"
  chown "$CI_USER:$CI_USER" "$RUNNER_DIR/.path"
  runuser -u "$CI_USER" -- env PATH="$JOB_PATH" "$RUNNER_DIR/run.sh"
  # An ephemeral registration removes itself after the job; clear the local
  # credentials so the next config.sh starts clean.
  rm -f "$RUNNER_DIR/.runner" "$RUNNER_DIR/.credentials" "$RUNNER_DIR/.credentials_rsaparams"
  # Fresh workspace for the next job, like a hosted runner's fresh VM: jobs that
  # use sudo/docker leave root-owned files in the checkout and the next job's
  # actions/checkout then dies with EACCES (seen: benchmarks/baselines/
  # measurement-accuracy.json). Keep _tool (setup-* tool cache) and _actions
  # (downloaded action code); everything else under _work is per-job.
  find "$RUNNER_DIR/_work" -mindepth 1 -maxdepth 1 ! -name _tool ! -name _actions -exec rm -rf {} + 2>/dev/null || true
  # The rest of what a job can leave behind on a persistent machine. Jobs that
  # install system services (the installer exec jobs) are pinned to
  # GitHub-hosted runners instead — see docs/self-hosted-ci.md — so this is
  # only the per-job residue: containers (a `docker run --name bench-srv
  # -p 8443` from the previous job makes the next one fail "name in use"),
  # staged datasets under /tmp (validate-bench-apis copies bench-data.json to
  # /tmp/bench; networker-endpoint's tests must NOT find a dataset), and
  # processes the job left running as the CI user (sccache servers, stray
  # endpoints holding ports). The runner itself has exited at this point.
  if command -v docker >/dev/null 2>&1; then
    docker ps -aq 2>/dev/null | xargs -r docker rm -f >/dev/null 2>&1 || true
    docker network prune -f >/dev/null 2>&1 || true
  fi
  rm -rf /tmp/bench /tmp/networker-* 2>/dev/null || true
  # Integration jobs `sudo install` the freshly built tester/endpoint/agent
  # into /usr/local/bin; a CI host never legitimately carries them, and a
  # leftover tester turned a stubbed installer unit test into an 18-minute
  # real probe against 1.2.3.4 (bats "_offer_quick_test … release download").
  rm -f /usr/local/bin/networker-tester /usr/local/bin/networker-endpoint /usr/local/bin/networker-agent 2>/dev/null || true
  # A step that ran a build under `sudo -E` leaves root-owned entries in the
  # shared NuGet/npm caches and the next restore dies with EACCES (seen:
  # 1,139 root-owned files under nuget/ after the installer exec jobs).
  find "$CACHE_DIR" ! -user "$CI_USER" -exec chown "$CI_USER:$CI_USER" {} + 2>/dev/null || true
  pkill -TERM -u "$CI_USER" 2>/dev/null || true
  sleep 2
  pkill -KILL -u "$CI_USER" 2>/dev/null || true
  sleep 1
done
LOOP
chmod 0755 /usr/local/bin/ci-host-loop.sh

install -d -m 0700 /etc/ci-host
cat > /etc/ci-host/env <<ENV
REPO=$REPO
RUNNER_DIR=$RUNNER_DIR
CI_USER=$CI_USER
TOKEN_FILE=$TOKEN_FILE
LABELS=$LABELS
NAME=$NAME
GROUP=$GROUP
CACHE_DIR=$CACHE_DIR
JOB_PATH=$CI_HOME/.cargo/bin:/usr/share/dotnet:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ENV
chmod 0600 /etc/ci-host/env

cat > /etc/systemd/system/ci-host.service <<'UNIT'
[Unit]
Description=networker CI host — ephemeral GitHub Actions runner loop
After=network-online.target docker.service
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/ci-host/env
ExecStart=/usr/local/bin/ci-host-loop.sh
Restart=always
RestartSec=10
KillMode=process
TimeoutStopSec=5min

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable ci-host >/dev/null 2>&1
systemctl restart ci-host

log "done — CI host '$NAME' for $REPO with labels $LABELS (ephemeral loop: systemctl status ci-host)"
