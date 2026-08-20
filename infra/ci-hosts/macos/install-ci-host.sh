#!/bin/bash
# infra/ci-hosts/macos/install-ci-host.sh — make a Mac (mini) an ephemeral CI
# host for this repo: what GitHub calls a "self-hosted runner", labels
# self-hosted,macos,networker-ci. ("Runner" in this repo means a tester VM.)
#
# Idempotent; run AS THE LOGIN USER (not root). Everything lives in that user's
# home — Homebrew, rustup, the actions-runner package under ~/ci-host — so the
# loop is a user LaunchAgent, not a LaunchDaemon. sudo is needed only for the
# first run (Homebrew's installer, the dotnet-sdk / powershell .pkg casks, and
# pmset); every sudo step is skipped once done, so a re-run needs no password.
# setup-ci-hosts.sh drives it over `ssh -t` so that one-time prompt reaches
# you. This script NEVER adds a sudoers/NOPASSWD drop-in, and you should not
# either: CI jobs run arbitrary repo code as this user, so passwordless sudo
# would hand root to any pull request. If you must run the first install
# unattended, do it once interactively instead.
#
# Installs via Homebrew: rustup (stable + rustfmt/clippy + BOTH darwin targets —
# release.yml builds x86_64-apple-darwin and aarch64-apple-darwin), dotnet-sdk
# (10), node@22, jq, gh, powershell (cask); then actions-runner (osx-arm64 /
# osx-x64) under ~/ci-host/actions-runner with a LaunchAgent running the
# ephemeral re-register loop under `caffeinate -s`, plus pmset so the Mac never
# sleeps mid-job (the mini shipped with sleep=1 min). 16 GB → ONE CI host on
# this machine; do not register a second.
#
# Usage:
#   install-ci-host.sh --repo OWNER/REPO [--token-file ~/ci-host/token]
#                      [--labels self-hosted,macos,networker-ci] [--name ci-macos-1]
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
[ "$(id -u)" != 0 ] || die "run as the login user, not root (Homebrew and rustup live in \$HOME)"
[ -n "$REPO" ] || die "--repo OWNER/REPO is required"
[ -s "$TOKEN_FILE" ] || die "PAT file $TOKEN_FILE is missing or empty (mode 0600; a PAT with repository Administration:write)"
chmod 0600 "$TOKEN_FILE"
mkdir -p "$CI_DIR/logs"

# sudo only when a step really needs it; -n first so a cached/NOPASSWD sudo
# never prompts, and a clear message when no tty can carry the prompt.
as_root() {
  if sudo -n true 2>/dev/null; then sudo -n "$@"
  elif [ -t 0 ]; then sudo "$@"
  else die "'$*' needs sudo but there is no tty — run this once interactively (ssh -t); do NOT add a NOPASSWD sudoers rule for the CI user"
  fi
}

case "$(uname -m)" in
  arm64) RUNNER_ARCH=osx-arm64; BREW_PREFIX=/opt/homebrew ;;
  x86_64) RUNNER_ARCH=osx-x64; BREW_PREFIX=/usr/local ;;
  *) die "unsupported arch $(uname -m)" ;;
esac
export PATH="$BREW_PREFIX/bin:$BREW_PREFIX/opt/node@22/bin:$BREW_PREFIX/opt/rustup/bin:$HOME/.cargo/bin:/usr/local/share/dotnet:$PATH"
export HOMEBREW_NO_AUTO_UPDATE=1 HOMEBREW_NO_ENV_HINTS=1

if [ "$SKIP_TOOLCHAINS" = 0 ]; then
  # ── 1. Xcode CLT + Homebrew ───────────────────────────────────────────────
  xcode-select -p >/dev/null 2>&1 || { log "installing Xcode command line tools (accept the dialog)"; xcode-select --install || true; }
  if [ ! -x "$BREW_PREFIX/bin/brew" ]; then
    log "installing Homebrew (its installer will ask for your password)"
    NONINTERACTIVE=1 /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  fi
  # ── 2. formulae ───────────────────────────────────────────────────────────
  for f in jq gh node@22 rustup; do
    brew list --formula "$f" >/dev/null 2>&1 || { log "brew install $f"; brew install "$f"; }
  done
  brew link --overwrite --force node@22 >/dev/null 2>&1 || true
  # casks ship .pkg installers → sudo on first install only
  for c in dotnet-sdk powershell; do
    brew list --cask "$c" >/dev/null 2>&1 || { log "brew install --cask $c (may ask for your password)"; brew install --cask "$c"; }
  done
  # ── 3. rustup: stable + fmt/clippy + both darwin targets ──────────────────
  log "rustup: stable + rustfmt clippy + x86_64/aarch64-apple-darwin"
  "$BREW_PREFIX/opt/rustup/bin/rustup" toolchain install stable --profile minimal --component rustfmt clippy >/dev/null
  "$BREW_PREFIX/opt/rustup/bin/rustup" default stable >/dev/null
  "$BREW_PREFIX/opt/rustup/bin/rustup" target add x86_64-apple-darwin aarch64-apple-darwin >/dev/null
  rustc --version; dotnet --version; node --version; pwsh --version
fi

# ── 4. actions-runner package ─────────────────────────────────────────────────
RUNNER_DIR="$CI_DIR/actions-runner"
if [ -z "$RUNNER_VERSION" ]; then
  RUNNER_VERSION="$(curl -fsSL https://api.github.com/repos/actions/runner/releases/latest | jq -r '.tag_name | ltrimstr("v")')"
  [ -n "$RUNNER_VERSION" ] && [ "$RUNNER_VERSION" != null ] || die "could not resolve the latest actions/runner version (pass --runner-version)"
fi
PLIST_LABEL=com.networker.ci-host
PLIST="$HOME/Library/LaunchAgents/${PLIST_LABEL}.plist"
if [ ! -x "$RUNNER_DIR/run.sh" ] || ! grep -q "\"$RUNNER_VERSION\"" "$RUNNER_DIR/.runner-version" 2>/dev/null; then
  log "installing actions-runner ${RUNNER_VERSION} (${RUNNER_ARCH}) into $RUNNER_DIR"
  launchctl bootout "gui/$(id -u)/${PLIST_LABEL}" 2>/dev/null || true
  mkdir -p "$RUNNER_DIR"
  tmp="$(mktemp -d)"
  curl -fsSL -o "$tmp/runner.tgz" "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-${RUNNER_ARCH}-${RUNNER_VERSION}.tar.gz"
  tar xzf "$tmp/runner.tgz" -C "$RUNNER_DIR"
  rm -rf "$tmp"
  printf '"%s"\n' "$RUNNER_VERSION" > "$RUNNER_DIR/.runner-version"
fi
cat > "$RUNNER_DIR/.env" <<ENV
PATH=$HOME/.cargo/bin:$BREW_PREFIX/opt/rustup/bin:$BREW_PREFIX/opt/node@22/bin:$BREW_PREFIX/bin:/usr/local/share/dotnet:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
DOTNET_ROOT=/usr/local/share/dotnet
DOTNET_CLI_TELEMETRY_OPTOUT=1
HOMEBREW_NO_AUTO_UPDATE=1
ENV

# ── 5. ephemeral loop + LaunchAgent ──────────────────────────────────────────
cat > "$CI_DIR/ci-host-loop.sh" <<'LOOP'
#!/bin/bash
# Mint a registration token from the PAT, register --ephemeral, run ONE job
# under caffeinate (no sleep mid-job), repeat. The PAT stays in this process.
set -uo pipefail
: "${REPO:?}" "${RUNNER_DIR:?}" "${TOKEN_FILE:?}" "${LABELS:?}" "${NAME:?}"
GROUP_ARGS=()
[ -n "${GROUP:-}" ] && GROUP_ARGS=(--runnergroup "$GROUP")
while :; do
  pat="$(cat "$TOKEN_FILE")"
  reg="$(curl -sS --max-time 30 -X POST \
    -H "Authorization: Bearer ${pat}" -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "https://api.github.com/repos/${REPO}/actions/runners/registration-token" | jq -r '.token // empty')"
  unset pat
  if [ -z "$reg" ]; then echo "registration token mint failed — retrying in 60s" >&2; sleep 60; continue; fi
  if ! "$RUNNER_DIR/config.sh" --unattended --ephemeral --replace \
        --url "https://github.com/${REPO}" --token "$reg" \
        --name "$NAME" --labels "$LABELS" --work _work "${GROUP_ARGS[@]}" >/dev/null; then
    echo "config.sh failed — retrying in 30s" >&2; sleep 30; continue
  fi
  caffeinate -s "$RUNNER_DIR/run.sh"
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
    <key>PATH</key><string>${BREW_PREFIX}/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
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

# ── 6. never sleep (the mini shipped with sleep=1 min) ───────────────────────
if pmset -g custom 2>/dev/null | grep -Eq '^ *(sleep|powernap) +[1-9]'; then
  log "pmset: disabling sleep + Power Nap (sudo)"
  as_root pmset -a sleep 0 disksleep 0 powernap 0 autorestart 1
fi

log "done — CI host '$NAME' for $REPO with labels $LABELS"
log "  loop: launchctl print gui/$(id -u)/${PLIST_LABEL}   logs: $CI_DIR/logs/"
