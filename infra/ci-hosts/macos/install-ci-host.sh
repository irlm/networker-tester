#!/bin/bash
# infra/ci-hosts/macos/install-ci-host.sh — turn a Mac (Apple Silicon or Intel)
# into an ephemeral CI host for this repo: what GitHub calls a "self-hosted
# runner" (labels self-hosted,macos,networker-ci). "Runner" in this repo means a
# tester VM, hence the name.
#
# NO sudo, NO Homebrew. Everything lives in the login user's home, because a CI
# host executes arbitrary repo code as that user — it must never hold
# privileges, and you should never give it a NOPASSWD sudoers rule:
#   ~/.cargo + ~/.rustup   rustup stable + rustfmt/clippy + BOTH darwin targets
#                          (release.yml builds x86_64- and aarch64-apple-darwin)
#   ~/.dotnet              .NET SDK 10 via dotnet-install.sh
#   ~/ci-host/node         Node 22 (official tarball)
#   ~/ci-host/pwsh         PowerShell 7 (official tarball)
#   ~/ci-host/cmake        CMake (aws-lc-sys behind networker-endpoint needs it;
#                          Xcode CLT ships cc/git/make but no cmake)
#   ~/ci-host/bin          jq
#   ~/ci-host/actions-runner  the runner, registered --ephemeral by a user
#                          LaunchAgent loop that runs every job under
#                          `caffeinate -is` so the Mac cannot sleep mid-job
# The only system-level prerequisite is the Xcode Command Line Tools (cc,
# git, make); `xcode-select --install` if they are missing.
#
#   install-ci-host.sh --repo OWNER/REPO [--token-file ~/ci-host/token] [--name NAME]
#                      [--runner-group GROUP] [--runner-version X.Y.Z] [--skip-toolchains]
#
# Runs on the stock /bin/bash 3.2 on purpose (no bash 4 on a fresh Mac).

set -euo pipefail

REPO="${CI_HOST_REPO:-}"
CI_DIR="$HOME/ci-host"
TOKEN_FILE="${CI_HOST_TOKEN_FILE:-$CI_DIR/token}"
LABELS="${CI_HOST_LABELS:-self-hosted,macos,networker-ci}"
NAME="${CI_HOST_NAME:-ci-macos-1}"
GROUP="${CI_HOST_RUNNER_GROUP:-}"
RUNNER_VERSION="${CI_HOST_RUNNER_VERSION:-}"
NODE_MAJOR="${CI_HOST_NODE_MAJOR:-22}"
DOTNET_CHANNEL="${CI_HOST_DOTNET_CHANNEL:-10.0}"
SKIP_TOOLCHAINS=0

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="$2"; shift ;;
    --token-file) TOKEN_FILE="$2"; shift ;;
    --labels) LABELS="$2"; shift ;;
    --name) NAME="$2"; shift ;;
    --runner-group) GROUP="$2"; shift ;;
    --runner-version) RUNNER_VERSION="$2"; shift ;;
    --skip-toolchains) SKIP_TOOLCHAINS=1 ;;
    -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
  shift
done

log() { printf '\033[1;34m[install-ci-host]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[install-ci-host] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = Darwin ] || die "this script is for macOS"
[ "$(id -u)" != 0 ] || die "run as the login user, not root (everything lives in \$HOME)"
[ -n "$REPO" ] || die "--repo OWNER/REPO is required"
[ -s "$TOKEN_FILE" ] || die "PAT file $TOKEN_FILE is missing or empty (mode 0600; a PAT that can mint runner registration tokens)"
chmod 0600 "$TOKEN_FILE"
mkdir -p "$CI_DIR/bin" "$CI_DIR/logs"

case "$(uname -m)" in
  arm64)  RUNNER_ARCH=osx-arm64; NODE_ARCH=darwin-arm64; PWSH_ARCH=osx-arm64; JQ_ARCH=macos-arm64; DOTNET_ARCH=arm64 ;;
  x86_64) RUNNER_ARCH=osx-x64;   NODE_ARCH=darwin-x64;   PWSH_ARCH=osx-x64;   JQ_ARCH=macos-amd64; DOTNET_ARCH=x64 ;;
  *) die "unsupported arch $(uname -m)" ;;
esac

# Everything the loop (and the jobs) must find, in one place.
HOST_PATH="$HOME/.cargo/bin:$HOME/.dotnet:$CI_DIR/node/bin:$CI_DIR/pwsh:$CI_DIR/cmake/bin:$CI_DIR/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH="$HOST_PATH"
export DOTNET_ROOT="$HOME/.dotnet"

fetch() { curl -fsSL --retry 3 --retry-delay 5 -o "$2" "$1"; }

if [ "$SKIP_TOOLCHAINS" = 0 ]; then
  # ── 1. Xcode CLT (cc/git/make) — the one thing this script cannot install ──
  xcode-select -p >/dev/null 2>&1 || die "Xcode Command Line Tools are missing: run 'xcode-select --install' once on the Mac, then re-run"

  # ── 2. jq (the loop parses the registration-token JSON with it) ──────────
  if [ ! -x "$CI_DIR/bin/jq" ]; then
    log "installing jq ($JQ_ARCH)"
    fetch "https://github.com/jqlang/jq/releases/latest/download/jq-${JQ_ARCH}" "$CI_DIR/bin/jq"
    chmod 0755 "$CI_DIR/bin/jq"
  fi

  # ── 3. rustup: stable + fmt/clippy + both darwin targets ─────────────────
  if [ ! -x "$HOME/.cargo/bin/rustup" ]; then
    log "installing rustup (stable, minimal) into ~/.cargo + ~/.rustup"
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path >/dev/null
  fi
  log "rustup: rustfmt clippy + x86_64-apple-darwin aarch64-apple-darwin"
  "$HOME/.cargo/bin/rustup" toolchain install stable --profile minimal --component rustfmt,clippy >/dev/null
  "$HOME/.cargo/bin/rustup" default stable >/dev/null
  "$HOME/.cargo/bin/rustup" target add x86_64-apple-darwin aarch64-apple-darwin >/dev/null

  # ── 4. .NET SDK (dotnet-install.sh, user-local) ──────────────────────────
  if ! "$HOME/.dotnet/dotnet" --list-sdks 2>/dev/null | grep -q "^${DOTNET_CHANNEL%%.*}\."; then
    log "installing .NET SDK ${DOTNET_CHANNEL} into ~/.dotnet"
    tmp="$(mktemp)"; fetch https://dot.net/v1/dotnet-install.sh "$tmp"
    bash "$tmp" --channel "$DOTNET_CHANNEL" --architecture "$DOTNET_ARCH" --install-dir "$HOME/.dotnet" >/dev/null
    rm -f "$tmp"
  fi

  # ── 5. Node (official tarball) ───────────────────────────────────────────
  if ! "$CI_DIR/node/bin/node" --version 2>/dev/null | grep -q "^v${NODE_MAJOR}\."; then
    ver="$(curl -fsSL https://nodejs.org/dist/index.json | "$CI_DIR/bin/jq" -r --arg m "v${NODE_MAJOR}." 'map(select(.version | startswith($m))) | .[0].version')"
    [ -n "$ver" ] && [ "$ver" != null ] || die "could not resolve the latest Node ${NODE_MAJOR}"
    log "installing node ${ver} (${NODE_ARCH}) into ~/ci-host/node"
    tmp="$(mktemp -d)"; fetch "https://nodejs.org/dist/${ver}/node-${ver}-${NODE_ARCH}.tar.gz" "$tmp/node.tgz"
    rm -rf "$CI_DIR/node"; mkdir -p "$CI_DIR/node"
    tar xzf "$tmp/node.tgz" -C "$CI_DIR/node" --strip-components 1; rm -rf "$tmp"
  fi

  # ── 6. PowerShell (official tarball) ─────────────────────────────────────
  if [ ! -x "$CI_DIR/pwsh/pwsh" ]; then
    ver="$(curl -fsSL https://api.github.com/repos/PowerShell/PowerShell/releases/latest | "$CI_DIR/bin/jq" -r '.tag_name | ltrimstr("v")')"
    [ -n "$ver" ] && [ "$ver" != null ] || die "could not resolve the latest PowerShell release"
    log "installing PowerShell ${ver} (${PWSH_ARCH}) into ~/ci-host/pwsh"
    tmp="$(mktemp -d)"; fetch "https://github.com/PowerShell/PowerShell/releases/download/v${ver}/powershell-${ver}-${PWSH_ARCH}.tar.gz" "$tmp/pwsh.tgz"
    rm -rf "$CI_DIR/pwsh"; mkdir -p "$CI_DIR/pwsh"
    tar xzf "$tmp/pwsh.tgz" -C "$CI_DIR/pwsh"; chmod 0755 "$CI_DIR/pwsh/pwsh"; rm -rf "$tmp"
  fi

  # ── 7. CMake (official universal tarball) ────────────────────────────────
  if [ ! -x "$CI_DIR/cmake/bin/cmake" ]; then
    ver="$(curl -fsSL https://api.github.com/repos/Kitware/CMake/releases/latest | "$CI_DIR/bin/jq" -r '.tag_name | ltrimstr("v")')"
    [ -n "$ver" ] && [ "$ver" != null ] || die "could not resolve the latest CMake release"
    log "installing cmake ${ver} into ~/ci-host/cmake"
    tmp="$(mktemp -d)"; fetch "https://github.com/Kitware/CMake/releases/download/v${ver}/cmake-${ver}-macos-universal.tar.gz" "$tmp/cmake.tgz"
    rm -rf "$CI_DIR/cmake"; mkdir -p "$CI_DIR/cmake"
    # the tarball wraps a CMake.app bundle; expose its Contents/ as ~/ci-host/cmake
    tar xzf "$tmp/cmake.tgz" -C "$tmp"
    cp -R "$tmp"/cmake-*/CMake.app/Contents/. "$CI_DIR/cmake/"; rm -rf "$tmp"
  fi

  log "toolchains: $(rustc --version) | dotnet $(dotnet --version) | node $(node --version) | $(pwsh --version) | $(cmake --version | head -1)"
fi

# ── 8. actions-runner package ─────────────────────────────────────────────────
RUNNER_DIR="$CI_DIR/actions-runner"
if [ -z "$RUNNER_VERSION" ]; then
  RUNNER_VERSION="$(curl -fsSL https://api.github.com/repos/actions/runner/releases/latest | "$CI_DIR/bin/jq" -r '.tag_name | ltrimstr("v")')"
  [ -n "$RUNNER_VERSION" ] && [ "$RUNNER_VERSION" != null ] || die "could not resolve the latest actions/runner version (pass --runner-version)"
fi
PLIST_LABEL=com.networker.ci-host
PLIST="$HOME/Library/LaunchAgents/${PLIST_LABEL}.plist"
if [ ! -x "$RUNNER_DIR/run.sh" ] || ! grep -q "\"$RUNNER_VERSION\"" "$RUNNER_DIR/.runner-version" 2>/dev/null; then
  log "installing actions-runner ${RUNNER_VERSION} (${RUNNER_ARCH}) into $RUNNER_DIR"
  launchctl bootout "gui/$(id -u)/${PLIST_LABEL}" 2>/dev/null || true
  mkdir -p "$RUNNER_DIR"
  tmp="$(mktemp -d)"
  fetch "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-${RUNNER_ARCH}-${RUNNER_VERSION}.tar.gz" "$tmp/runner.tgz"
  tar xzf "$tmp/runner.tgz" -C "$RUNNER_DIR"
  rm -rf "$tmp"
  printf '"%s"\n' "$RUNNER_VERSION" > "$RUNNER_DIR/.runner-version"
fi
cat > "$RUNNER_DIR/.env" <<ENV
PATH=$HOST_PATH
DOTNET_ROOT=$HOME/.dotnet
DOTNET_CLI_TELEMETRY_OPTOUT=1
RUSTUP_HOME=$HOME/.rustup
CARGO_HOME=$HOME/.cargo
ENV

# ── 9. ephemeral loop + LaunchAgent ──────────────────────────────────────────
# Mint a registration token from the PAT, register --ephemeral, run ONE job
# under caffeinate (no idle/system sleep mid-job), repeat. The PAT stays in
# this process; the short-lived registration token is all the runner sees.
cat > "$CI_DIR/ci-host-loop.sh" <<'LOOP'
#!/bin/bash
set -u
: "${REPO:?}" "${RUNNER_DIR:?}" "${TOKEN_FILE:?}" "${LABELS:?}" "${NAME:?}"
GROUP="${GROUP:-}"
cd "$RUNNER_DIR" || exit 1
while :; do
  pat="$(cat "$TOKEN_FILE" 2>/dev/null || true)"
  reg="$(curl -fsS -X POST -H "Authorization: Bearer ${pat}" -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "https://api.github.com/repos/${REPO}/actions/runners/registration-token" | jq -r '.token // empty')"
  unset pat
  if [ -z "$reg" ]; then echo "registration token mint failed — retrying in 60s" >&2; sleep 60; continue; fi
  if ! "$RUNNER_DIR/config.sh" --unattended --ephemeral --replace \
        --url "https://github.com/${REPO}" --token "$reg" \
        --name "$NAME" --labels "$LABELS" --work _work ${GROUP:+--runnergroup "$GROUP"} >/dev/null; then
    echo "config.sh failed — retrying in 30s" >&2; sleep 30; continue
  fi
  caffeinate -is "$RUNNER_DIR/run.sh"
  rm -f "$RUNNER_DIR/.runner" "$RUNNER_DIR/.credentials" "$RUNNER_DIR/.credentials_rsaparams"
  sleep 3
done
LOOP
chmod 0755 "$CI_DIR/ci-host-loop.sh"

mkdir -p "$HOME/Library/LaunchAgents"
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${PLIST_LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>${CI_DIR}/ci-host-loop.sh</string></array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>REPO</key><string>${REPO}</string>
    <key>RUNNER_DIR</key><string>${RUNNER_DIR}</string>
    <key>TOKEN_FILE</key><string>${TOKEN_FILE}</string>
    <key>LABELS</key><string>${LABELS}</string>
    <key>NAME</key><string>${NAME}</string>
    <key>GROUP</key><string>${GROUP}</string>
    <key>PATH</key><string>${HOST_PATH}</string>
    <key>DOTNET_ROOT</key><string>${HOME}/.dotnet</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>${CI_DIR}/logs/loop.log</string>
  <key>StandardErrorPath</key><string>${CI_DIR}/logs/loop.err</string>
</dict>
</plist>
PLIST
launchctl bootout "gui/$(id -u)/${PLIST_LABEL}" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
launchctl kickstart -k "gui/$(id -u)/${PLIST_LABEL}"

# ── 10. sleep: handled without sudo ──────────────────────────────────────────
# The loop runs under `caffeinate -is`, which holds the Mac awake for as long
# as the loop lives (always). `pmset -a sleep 0` would be belt-and-braces but
# needs sudo — say so instead of asking for it.
if pmset -g custom 2>/dev/null | grep -Eq '^ *(sleep|powernap) +[1-9]'; then
  log "note: pmset still allows sleep; caffeinate in the loop keeps the Mac awake. For belt-and-braces run once as an admin: sudo pmset -a sleep 0 disksleep 0 powernap 0 autorestart 1"
fi

log "done — CI host '$NAME' for $REPO with labels $LABELS (no sudo was used)"
log "  loop: launchctl print gui/$(id -u)/${PLIST_LABEL}   logs: $CI_DIR/logs/"
