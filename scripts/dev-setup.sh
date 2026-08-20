#!/usr/bin/env bash
# ─── Developer environment setup ─────────────────────────────────────────────
# One-shot, idempotent configuration of a dev machine for this repo — macOS
# (Homebrew) and the supported Linux distros: Ubuntu/Debian (apt), Fedora (dnf),
# Arch (pacman). Windows: use scripts/dev-setup.ps1 (then run dev.sh from Git
# Bash or WSL). Checks every prerequisite, offers to install what is missing, works out
# the machine-specific quirks (a busy :5432, a .NET SDK with an odd RID) and
# writes them to `.dev.env`, which dev.sh / scripts/seed-dev.sh / lab/lab.sh
# source. Safe to re-run any time.
#
#   ./scripts/dev-setup.sh            # interactive (asks before installing)
#   ./scripts/dev-setup.sh -y         # install everything missing without asking
#   ./scripts/dev-setup.sh --check    # report only, exit 1 if something is missing
#
# Then: ./dev.sh   (or ./lab/lab.sh up for the Docker lab)
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT" || exit 1
DEV_ENV="$REPO_ROOT/.dev.env"

AUTO_YES=0; CHECK_ONLY=0
for a in "$@"; do
  case "$a" in
    -y|--yes) AUTO_YES=1 ;;
    --check) CHECK_ONLY=1 ;;
    -h|--help) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown flag $a" >&2; exit 2 ;;
  esac
done

if [ -t 1 ]; then B=$'\033[1m'; D=$'\033[2m'; G=$'\033[32m'; Y=$'\033[33m'; R=$'\033[31m'; C=$'\033[36m'; N=$'\033[0m'; else B=""; D=""; G=""; Y=""; R=""; C=""; N=""; fi
ok()   { printf '%s  ✓%s %s\n' "$G" "$N" "$*"; }
warn() { printf '%s  ⚠%s %s\n' "$Y" "$N" "$*"; }
bad()  { printf '%s  ✗%s %s\n' "$R" "$N" "$*"; }
note() { printf '%s▸%s %s\n' "$C" "$N" "$*"; }
MISSING=0
ask() { # ask "question" → 0 = yes
  [ "$AUTO_YES" = 1 ] && return 0
  [ "$CHECK_ONLY" = 1 ] && return 1
  printf '  %s [Y/n] ' "$1"; read -r r </dev/tty || return 1
  case "$r" in n|N|no|NO) return 1;; *) return 0;; esac
}

# ── OS / package manager ─────────────────────────────────────────────────────
OS="$(uname -s)"; ARCH="$(uname -m)"
PKG=""
case "$OS" in
  Darwin) command -v brew >/dev/null 2>&1 && PKG=brew ;;
  Linux)
    for p in apt-get dnf pacman; do command -v $p >/dev/null 2>&1 && { PKG=$p; break; }; done
    [ -n "$PKG" ] || warn "unsupported Linux package manager — supported: Ubuntu/Debian (apt), Fedora (dnf), Arch (pacman); install tools manually" ;;
  MINGW*|MSYS*|CYGWIN*) warn "Windows shell detected — run scripts/dev-setup.ps1 from PowerShell instead (this script only records settings here)" ;;
esac
pkg_install() { # pkg_install <package names for this manager...>
  case "$PKG" in
    brew)   brew install "$@" ;;
    apt-get) sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "$@" ;;
    dnf)    sudo dnf install -y "$@" ;;
    pacman) sudo pacman -S --noconfirm --needed "$@" ;;
    *) return 1 ;;
  esac
}
printf '\n%s%s  networker dev-setup%s  %s(%s %s, pkg=%s)%s\n\n' "$B" "$C" "$N" "$D" "$OS" "$ARCH" "${PKG:-none}" "$N"

# ── Tools ────────────────────────────────────────────────────────────────────
# need NAME  CHECK_CMD  MIN_VERSION_HINT  brew_pkg  apt_pkg  dnf_pkg  pacman_pkg  url
need_tool() {
  local name="$1" hint="$2" brew_p="$3" apt_p="$4" dnf_p="$5" pac_p="$6" url="$7"
  if command -v "$name" >/dev/null 2>&1; then ok "$name $($name --version 2>/dev/null | head -1 | cut -c1-60)"; return 0; fi
  bad "$name missing${hint:+ ($hint)}"
  local pkg=""
  case "$PKG" in brew) pkg="$brew_p";; apt-get) pkg="$apt_p";; dnf) pkg="$dnf_p";; pacman) pkg="$pac_p";; esac
  if [ -n "$pkg" ] && ask "install $name via $PKG ($pkg)?"; then
    pkg_install $pkg && command -v "$name" >/dev/null 2>&1 && { ok "$name installed"; return 0; }
  fi
  [ -n "$url" ] && printf '      → %s\n' "$url"
  MISSING=$((MISSING + 1)); return 1
}

note "core tools"
need_tool git     "" git git git git "https://git-scm.com"
need_tool curl    "" curl curl curl curl ""
need_tool jq      "" jq jq jq jq "https://jqlang.github.io/jq/"
need_tool openssl "" openssl openssl openssl openssl ""
need_tool node    ">= 20" node nodejs nodejs nodejs "https://nodejs.org (or: brew install node / nvm)"
need_tool npm     "" node npm npm npm ""

# Docker + compose v2
note "docker"
if command -v docker >/dev/null 2>&1; then
  if docker info >/dev/null 2>&1; then ok "docker $(docker version --format '{{.Server.Version}}' 2>/dev/null) (daemon running)"
  else bad "docker installed but the daemon is not running (start Docker Desktop / systemctl start docker)"; MISSING=$((MISSING + 1)); fi
  if docker compose version >/dev/null 2>&1; then ok "docker compose $(docker compose version --short 2>/dev/null)"
  else bad "docker compose v2 plugin missing (Docker Desktop includes it; Linux: docker-compose-plugin)"; MISSING=$((MISSING + 1)); fi
else
  bad "docker missing → https://docs.docker.com/get-docker/"; MISSING=$((MISSING + 1))
fi

# Rust
note "rust"
if command -v cargo >/dev/null 2>&1; then
  ok "$(rustc --version) / $(cargo --version | cut -d' ' -f1-2)"
  command -v rustfmt >/dev/null 2>&1 || { warn "rustfmt missing → rustup component add rustfmt"; ask "add rustfmt + clippy now?" && rustup component add rustfmt clippy; }
else
  bad "rust toolchain missing"
  if ask "install via rustup (https://rustup.rs)?"; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile default && . "$HOME/.cargo/env" && ok "$(rustc --version)"
  else MISSING=$((MISSING + 1)); fi
fi
# aws-lc-sys (via axum-server in networker-endpoint) needs a C compiler + cmake
for t in cc cmake; do
  if command -v $t >/dev/null 2>&1; then ok "$t"; else
    warn "$t missing (needed to build networker-endpoint's aws-lc-sys)"
    case "$PKG" in
      brew)    ask "brew install cmake?" && pkg_install cmake ;;
      apt-get) ask "apt install build-essential cmake?" && pkg_install build-essential cmake ;;
      dnf)     ask "dnf install gcc cmake?" && pkg_install gcc gcc-c++ cmake ;;
      pacman)  ask "pacman -S base-devel cmake?" && pkg_install base-devel cmake ;;
    esac
    command -v $t >/dev/null 2>&1 || MISSING=$((MISSING + 1))
  fi
done

# .NET 10 SDK
note ".NET"
DOTNET_EXTRA_ARGS=""
if command -v dotnet >/dev/null 2>&1; then
  sdk_major="$(dotnet --version 2>/dev/null | cut -d. -f1)"
  if [ "${sdk_major:-0}" -ge 10 ]; then ok "dotnet SDK $(dotnet --version)"; else bad "dotnet SDK $(dotnet --version 2>/dev/null) — need 10.x → https://dotnet.microsoft.com/download"; MISSING=$((MISSING + 1)); fi
  # RID quirk: distro-packaged SDKs (Arch/Omarchy report `arch-x64`) try to
  # restore Microsoft.NETCore.App.Host.<rid>, which does not exist on nuget.org
  # → NU1101 on every plain `dotnet build`. Framework-dependent dev builds don't
  # need the apphost, so pass -p:UseAppHost=false through .dev.env.
  rid="$(dotnet --info 2>/dev/null | sed -n 's/^ *RID: *//p' | head -1)"
  case "$rid" in
    linux-x64|linux-arm64|linux-musl-*|osx-x64|osx-arm64|win-*|"") ok "RID $rid" ;;
    *) warn "RID '$rid' is not a nuget.org apphost RID — dev builds will use -p:UseAppHost=false"; DOTNET_EXTRA_ARGS="-p:UseAppHost=false" ;;
  esac
else
  bad "dotnet missing → https://dotnet.microsoft.com/download (SDK 10.x)"
  case "$PKG" in
    brew)    ask "brew install --cask dotnet-sdk?" && brew install --cask dotnet-sdk ;;
    apt-get) ask "apt install dotnet-sdk-10.0? (Ubuntu 24.04+ has it in-repo; older: packages.microsoft.com)" && pkg_install dotnet-sdk-10.0 ;;
    dnf)     ask "dnf install dotnet-sdk-10.0?" && pkg_install dotnet-sdk-10.0 ;;
    pacman)  ask "pacman -S dotnet-sdk? (Arch's build reports RID arch-x64 — handled below)" && pkg_install dotnet-sdk ;;
  esac
  if command -v dotnet >/dev/null 2>&1; then
    rid="$(dotnet --info 2>/dev/null | sed -n 's/^ *RID: *//p' | head -1)"
    case "$rid" in linux-x64|linux-arm64|osx-*|win-*|"") ;; *) DOTNET_EXTRA_ARGS="-p:UseAppHost=false";; esac
    ok "dotnet SDK $(dotnet --version)"
  else MISSING=$((MISSING + 1)); fi
fi

# Optional
note "optional (installer tests / linting — scripts/lint-all.sh prints the install hint for anything else it skips)"
for t in shellcheck bats actionlint; do
  if command -v $t >/dev/null 2>&1; then ok "$t"; else
    warn "$t not installed (installer tests / scripts/lint-all.sh; shellcheck+actionlint also run via LINT_DOCKER=1)"
    case "$PKG" in
      brew) [ $t = bats ] && ask "brew install bats-core?" && pkg_install bats-core; [ $t = shellcheck ] && ask "brew install shellcheck?" && pkg_install shellcheck; [ $t = actionlint ] && ask "brew install actionlint?" && pkg_install actionlint ;;
      apt-get) [ $t = actionlint ] || { ask "apt install $t?" && pkg_install $t; } ;;
      pacman) [ $t = bats ] && ask "pacman -S bash-bats?" && pkg_install bash-bats; [ $t = shellcheck ] && ask "pacman -S shellcheck?" && pkg_install shellcheck; [ $t = actionlint ] && ask "pacman -S actionlint?" && pkg_install actionlint ;;
      dnf) [ $t = actionlint ] || { ask "dnf install $t?" && pkg_install $t; } ;;
    esac
  fi
done

# ── Ports: pick a free host port for the dev PostgreSQL ──────────────────────
note "ports"
port_busy() { # port_busy N → 0 if something listens on 127.0.0.1:N
  # A TCP connect probe first: it sees root-owned listeners (docker-proxy) that
  # an unprivileged lsof does not; then ss (Linux) / lsof (macOS) as fallbacks.
  if (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; then exec 3>&- 2>/dev/null; return 0; fi
  if command -v ss >/dev/null 2>&1; then ss -ltn 2>/dev/null | awk '{print $4}' | grep -qE "[:.]$1\$" && return 0; fi
  if command -v lsof >/dev/null 2>&1; then lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1 && return 0; fi
  return 1
}
our_pg() { # our own compose postgres already bound to that port?
  docker compose -f docker-compose.dashboard.yml ps -q postgres 2>/dev/null | grep -q . && \
  docker compose -f docker-compose.dashboard.yml port postgres 5432 2>/dev/null | grep -q ":$1\$"
}
DEV_PG_PORT="${DEV_PG_PORT:-5432}"
if port_busy "$DEV_PG_PORT" && ! our_pg "$DEV_PG_PORT"; then
  warn "127.0.0.1:$DEV_PG_PORT is already in use by something else (a local PostgreSQL?)"
  for cand in 15432 25432 35432 45432; do   # (lab.sh uses 55432)
    if ! port_busy "$cand"; then DEV_PG_PORT="$cand"; break; fi
  done
  ok "dev PostgreSQL will use host port $DEV_PG_PORT (written to .dev.env)"
else
  ok "dev PostgreSQL host port $DEV_PG_PORT"
fi
for p in 5030 5173 8080; do port_busy "$p" && warn "port $p is in use — dev.sh will try to stop whatever holds it (control plane 5030 / vite 5173 / endpoint 8080)"; done

# ── Frontend deps ────────────────────────────────────────────────────────────
note "frontend"
if [ -d dashboard/node_modules ]; then ok "dashboard/node_modules present"
elif command -v npm >/dev/null 2>&1 && [ "$CHECK_ONLY" = 0 ]; then
  if ask "run npm install in dashboard/ (first time)?"; then (cd dashboard && npm install --no-audit --no-fund) && ok "npm install done"; fi
else warn "dashboard/node_modules missing — cd dashboard && npm install"; fi

# ── Write .dev.env ───────────────────────────────────────────────────────────
if [ "$CHECK_ONLY" = 0 ]; then
  {
    echo "# Generated by scripts/dev-setup.sh — machine-specific dev settings (git-ignored)."
    echo "# Sourced by dev.sh, scripts/seed-dev.sh, tests/cli_smoke.sh. Re-run dev-setup.sh to refresh."
    echo "DEV_PG_PORT=$DEV_PG_PORT"
    echo "DOTNET_BUILD_EXTRA_ARGS=\"$DOTNET_EXTRA_ARGS\""
  } > "$DEV_ENV"
  ok "wrote $(basename "$DEV_ENV") (DEV_PG_PORT=$DEV_PG_PORT${DOTNET_EXTRA_ARGS:+, DOTNET_BUILD_EXTRA_ARGS=$DOTNET_EXTRA_ARGS})"
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo
if [ "$MISSING" -gt 0 ]; then
  bad "$MISSING prerequisite(s) still missing — fix the items above and re-run"
  exit 1
fi
ok "environment ready"
printf '%s  next: ./dev.sh                     (endpoint + C# control plane :5030 + vite :5173)\n        ./lab/lab.sh up && ./lab/lab.sh validate   (Docker lab: control plane + runners + targets)\n        ./test-all.sh                  (all suites)%s\n' "$D" "$N"
