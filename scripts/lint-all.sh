#!/usr/bin/env bash
# scripts/lint-all.sh — ONE entry point that lints / format-checks every
# language in the repo (Rust, C#, TypeScript, Go, Python, bash, PowerShell,
# GitHub workflows, Dockerfiles, JSON, TOML/YAML/Markdown).
#
# Every section is either
#   block : CI enforces it (or it is clean and cheap and we want it enforced) —
#           a failure makes this script exit non-zero.
#   info  : extended scope with a known non-zero baseline — reported, never
#           fails unless --strict. Promote a section to `block` in the
#           SECTIONS table once its baseline is cleaned; CI picks the
#           promotion up automatically because it runs this same script.
#
# The `block` sections are the EXACT commands CI runs (each body carries a
# `# CI:` comment naming the job, and those jobs call
# `scripts/lint-all.sh --ci --only <section>`), so there is one definition of
# every lint command. Keep it that way when you touch either side.
#
# Usage:
#   scripts/lint-all.sh                  # all sections; block sections gate, info sections report
#   scripts/lint-all.sh --no-build       # skip the compiling sections (clippy, rustdoc, dotnet) — ~1-2 min pre-push pass
#   scripts/lint-all.sh --fix            # apply auto-fixes where the tool has them, then re-check
#   scripts/lint-all.sh --only rust,shell   # groups and/or section ids, comma-separated
#   scripts/lint-all.sh --skip dotnet    # everything except a group/section
#   scripts/lint-all.sh --strict         # info sections fail too
#   scripts/lint-all.sh --ci             # missing tool in a block section = FAIL (not SKIP); no colour;
#                                        # GitHub ::error:: annotations
#   scripts/lint-all.sh --list           # show sections, groups, tiers, tools
#
# Env:
#   LINT_DOCKER=1   run shellcheck / actionlint / hadolint through their official
#                   images (pinned tags below) when the binary is not installed.
#   LINT_VERBOSE=1  print full tool output on failure (default: last 40 lines).
#
# Portability: bash 4+ (macOS users: brew install bash). install.sh's bash-3.2
# rule applies to install.sh, not to dev tooling.

set -uo pipefail

ROOT="${LINT_ALL_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$ROOT" || exit 2

# ── CLI ─────────────────────────────────────────────────────────────────────
FIX=0 STRICT=0 CI=0 LIST=0 NO_BUILD=0 ONLY="" SKIP=""
while [ $# -gt 0 ]; do
  case "$1" in
    --fix) FIX=1 ;;
    --strict) STRICT=1 ;;
    --ci) CI=1 ;;
    --list) LIST=1 ;;
    --no-build) NO_BUILD=1 ;;
    --only) ONLY="$2"; shift ;;
    --only=*) ONLY="${1#--only=}" ;;
    --skip) SKIP="$2"; shift ;;
    --skip=*) SKIP="${1#--skip=}" ;;
    -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown flag: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

if [ "$CI" = 1 ] || [ ! -t 1 ]; then R='' G='' Y='' B='' D='' N=''
else R=$'\033[0;31m' G=$'\033[0;32m' Y=$'\033[0;33m' B=$'\033[1m' D=$'\033[2m' N=$'\033[0m'; fi

# Docker fallback images (pinned — bump deliberately, like the SHA-pinned actions).
SHELLCHECK_IMAGE="${SHELLCHECK_IMAGE:-koalaman/shellcheck:v0.11.0}"
ACTIONLINT_IMAGE="${ACTIONLINT_IMAGE:-rhysd/actionlint:1.7.7}"
HADOLINT_IMAGE="${HADOLINT_IMAGE:-hadolint/hadolint:v2.12.0}"

# A .NET SDK installed by dotnet-install.sh lives in ~/.dotnet and is often not
# on a non-login shell's PATH (Arch's pacman `dotnet-host` alone reports "No
# .NET SDKs were found"). Prefer it when the PATH dotnet has no SDK.
if ! { command -v dotnet >/dev/null 2>&1 && [ -n "$(dotnet --list-sdks 2>/dev/null)" ]; } \
   && [ -x "${HOME:-/nonexistent}/.dotnet/dotnet" ] && [ -n "$("$HOME/.dotnet/dotnet" --list-sdks 2>/dev/null)" ]; then
  export DOTNET_ROOT="$HOME/.dotnet" PATH="$HOME/.dotnet:$PATH"
fi

# ── Section table ───────────────────────────────────────────────────────────
# id | group | tier | builds? | tools (space-separated, "-" = none) | description
SECTIONS=$(cat <<'EOF'
rust-fmt            | rust       | block | no  | cargo                 | cargo fmt --all -- --check
rust-clippy         | rust       | block | yes | cargo                 | cargo clippy --all-targets -- -D warnings
rust-nodefault      | rust       | block | yes | cargo                 | cargo build -p networker-tester --no-default-features
rust-doc            | rust       | block | yes | cargo                 | rustdoc lint (broken links / bare URLs / bad HTML)
rust-musl           | rust       | info  | yes | cargo                 | cargo check shipped crates against x86_64-unknown-linux-musl
orchestrator-clippy | rust       | block | yes | cargo                 | benchmarks/orchestrator clippy -D warnings
orchestrator-fmt    | rust       | block | no  | cargo                 | benchmarks/orchestrator cargo fmt --check
metrics-agent       | rust       | info  | yes | cargo                 | benchmarks/metrics-agent fmt + clippy (no CI coverage today)
sdk-rust            | rust       | block | yes | cargo                 | sdk/rust fmt --check + clippy --all-features -D warnings
dotnet-build        | dotnet     | block | yes | dotnet-sdk            | dotnet build Networker.sln -c Release
dotnet-format       | dotnet     | info  | yes | dotnet-sdk            | dotnet format Networker.sln --verify-no-changes
frontend-tsc        | frontend   | block | no  | node dashboard-nm     | dashboard: tsc -b
frontend-eslint     | frontend   | block | no  | node dashboard-nm     | dashboard: eslint .
sdk-js              | frontend   | block | no  | node sdkjs-nm         | sdk/js: tsc --noEmit (npm run lint)
sdk-go-fmt          | go         | block | no  | gofmt                 | sdk/go: gofmt -l must be empty
sdk-go-vet          | go         | info  | no  | go                    | sdk/go: go vet ./...
python-syntax       | python     | block | no  | python3               | ast-parse every tracked .py (stdlib only)
python-ruff         | python     | info  | no  | ruff                  | ruff check + ruff format --check
shellcheck-installer| shell      | block | no  | shellcheck            | shellcheck install.sh (CI flags)
shellcheck-all      | shell      | info  | no  | shellcheck            | shellcheck every other tracked .sh (same flags)
bats-parse          | shell      | block | no  | bats                  | every .bats suite parses to >0 tests
psa-installer       | powershell | block | no  | pwsh psscriptanalyzer | PSScriptAnalyzer install.ps1 (Warning,Error)
ps51-parse          | powershell | block | no  | pwsh                  | install.ps1 tokenizes under the cp1252 / PowerShell 5.1 view
psa-all             | powershell | info  | no  | pwsh psscriptanalyzer | PSScriptAnalyzer on the other tracked .ps1 files
action-pins         | workflows  | block | no  | -                     | every `uses:` is SHA-pinned
actionlint          | workflows  | block | no  | actionlint            | actionlint (structural; shellcheck integration off)
actionlint-sc       | workflows  | info  | no  | actionlint shellcheck | actionlint incl. shellcheck of run: blocks
hadolint            | docker     | info  | no  | hadolint              | hadolint every tracked Dockerfile
json-parse          | json       | block | no  | -                     | shared/ + examples/configs + benchmarks/configs JSON parses
drift-csharp        | drift      | block | no  | python3               | benchmarks C# variants match csharp-template
version-sync        | version    | block | no  | -                     | the five version files agree + CHANGELOG section exists
markdown            | docs       | info  | no  | markdownlint          | markdownlint over tracked *.md
yaml                | docs       | info  | no  | yamllint              | yamllint (relaxed) over compose files + workflows
toml                | docs       | info  | no  | taplo                 | taplo fmt --check over tracked Cargo.toml files
EOF
)

# ── Helpers ─────────────────────────────────────────────────────────────────
PASS=0 FAIL=0 WARN=0 SKIPPED=0
FAILED_IDS="" WARNED_IDS=""

field() { echo "$1" | cut -d'|' -f"$2" | sed 's/^ *//;s/ *$//'; }

in_list() { # in_list needle "a,b,c"
  case ",$2," in *",$1,"*) return 0 ;; esac; return 1
}

selected() { # selected id group
  if [ -n "$SKIP" ] && { in_list "$1" "$SKIP" || in_list "$2" "$SKIP"; }; then return 1; fi
  [ -z "$ONLY" ] && return 0
  in_list "$1" "$ONLY" || in_list "$2" "$ONLY"
}

# Install hints: Arch first (the repo's dev boxes), then brew / apt where they differ.
hint() {
  case "$1" in
    cargo) echo "rustup component add rustfmt clippy" ;;
    dotnet-sdk) echo "sudo pacman -S dotnet-sdk  (or dotnet-install.sh → ~/.dotnet, auto-detected) — the runtime alone is not enough" ;;
    node) echo "mise use node@22  (or your node manager)" ;;
    dashboard-nm) echo "(cd dashboard && npm ci --ignore-scripts)" ;;
    sdkjs-nm) echo "(cd sdk/js && npm ci --ignore-scripts)" ;;
    gofmt|go) echo "sudo pacman -S go  / brew install go / apt install golang" ;;
    python3) echo "sudo pacman -S python" ;;
    ruff) echo "sudo pacman -S ruff  / brew install ruff / pipx install ruff" ;;
    shellcheck) echo "sudo pacman -S shellcheck  / brew install shellcheck / apt install shellcheck  (or LINT_DOCKER=1)" ;;
    bats) echo "sudo pacman -S bash-bats  / brew install bats-core / apt install bats" ;;
    pwsh) echo "yay -S powershell-bin  / brew install powershell" ;;
    psscriptanalyzer) echo "pwsh -c 'Install-Module PSScriptAnalyzer -Scope CurrentUser -Force'" ;;
    actionlint) echo "sudo pacman -S actionlint  / brew install actionlint  (or LINT_DOCKER=1)" ;;
    hadolint) echo "yay -S hadolint-bin  / brew install hadolint  (or LINT_DOCKER=1)" ;;
    markdownlint) echo "sudo pacman -S markdownlint-cli  / npm i -g markdownlint-cli" ;;
    yamllint) echo "sudo pacman -S yamllint  / pipx install yamllint" ;;
    taplo) echo "sudo pacman -S taplo-cli  / cargo install taplo-cli" ;;
    *) echo "install $1" ;;
  esac
}

have_docker() { [ "${LINT_DOCKER:-0}" = 1 ] && command -v docker >/dev/null 2>&1; }

# have_tool NAME → 0 if usable (binary, docker fallback, or pseudo-tool check)
have_tool() {
  case "$1" in
    -) return 0 ;;
    dotnet-sdk) command -v dotnet >/dev/null 2>&1 && [ -n "$(dotnet --list-sdks 2>/dev/null)" ] ;;
    dashboard-nm) [ -x dashboard/node_modules/.bin/tsc ] && [ -x dashboard/node_modules/.bin/eslint ] ;;
    sdkjs-nm) [ -x sdk/js/node_modules/.bin/tsc ] ;;
    psscriptanalyzer) pwsh -NoProfile -Command 'if (Get-Module -ListAvailable PSScriptAnalyzer) { exit 0 } else { exit 1 }' >/dev/null 2>&1 ;;
    shellcheck|actionlint|hadolint) command -v "$1" >/dev/null 2>&1 || have_docker ;;
    *) command -v "$1" >/dev/null 2>&1 ;;
  esac
}

# Tool wrappers: native binary, else the official image (read-only mount).
SHELLCHECK() { if command -v shellcheck >/dev/null 2>&1; then shellcheck "$@"; else docker run --rm -v "$ROOT:/mnt:ro" -w /mnt "$SHELLCHECK_IMAGE" "$@"; fi; }
ACTIONLINT() { if command -v actionlint >/dev/null 2>&1; then actionlint "$@"; else docker run --rm -v "$ROOT:/repo:ro" -w /repo "$ACTIONLINT_IMAGE" "$@"; fi; }
HADOLINT()   { if command -v hadolint >/dev/null 2>&1; then hadolint "$@"; else docker run --rm -i "$HADOLINT_IMAGE" hadolint "$@"; fi; }

cargo_version() { grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/'; }

# ── Section bodies ──────────────────────────────────────────────────────────
# Each returns 0 on success. $FIX=1 may mutate the tree first; the check then
# re-runs so the exit status reflects the post-fix state.

lint_rust_fmt() {            # CI: ci.yml "Lint"
  [ "$FIX" = 1 ] && cargo fmt --all
  cargo fmt --all -- --check
}
lint_rust_clippy() {         # CI: ci.yml "Lint"
  [ "$FIX" = 1 ] && cargo clippy --fix --allow-dirty --allow-staged --all-targets
  cargo clippy --all-targets -- -D warnings
}
lint_rust_nodefault() {      # CI: ci.yml "Lint" — the http3/pageload3 stub paths must keep compiling
  cargo build -p networker-tester --no-default-features
}
lint_rust_doc() {            # CI: ci.yml "Lint" — broken intra-doc links / bare URLs / bad HTML in doc comments
  RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links -D rustdoc::bare_urls -D rustdoc::invalid_html_tags" \
    cargo doc -p networker-tester -p networker-endpoint -p networker-log --no-deps --document-private-items
}
lint_rust_musl() {           # CI: ci.yml "musl check (release target types)" — info locally until the target is installed
  rustup target list --installed 2>/dev/null | grep -q x86_64-unknown-linux-musl \
    || { echo "musl target missing: rustup target add x86_64-unknown-linux-musl (and the musl package)"; return 1; }
  cargo check -p networker-tester -p networker-endpoint --target x86_64-unknown-linux-musl
}
lint_orchestrator_clippy() { # CI: ci.yml "Orchestrator lint & test"
  [ "$FIX" = 1 ] && cargo clippy --manifest-path benchmarks/orchestrator/Cargo.toml --fix --allow-dirty --allow-staged --all-targets
  cargo clippy --manifest-path benchmarks/orchestrator/Cargo.toml --all-targets -- -D warnings
}
lint_orchestrator_fmt() {    # not in CI (clean today) — candidate for the orchestrator job
  [ "$FIX" = 1 ] && cargo fmt --manifest-path benchmarks/orchestrator/Cargo.toml
  cargo fmt --manifest-path benchmarks/orchestrator/Cargo.toml -- --check
}
lint_metrics_agent() {       # not in CI; workspace-excluded crate with zero coverage
  [ "$FIX" = 1 ] && cargo fmt --manifest-path benchmarks/metrics-agent/Cargo.toml
  cargo fmt --manifest-path benchmarks/metrics-agent/Cargo.toml -- --check \
    && cargo clippy --manifest-path benchmarks/metrics-agent/Cargo.toml --all-targets -- -D warnings
}
lint_sdk_rust() {            # CI: sdk-conformance.yml "Rust SDK conformance"
  ( cd sdk/rust \
    && { [ "$FIX" = 0 ] || cargo fmt; } \
    && cargo fmt --check \
    && cargo clippy --all-features -- -D warnings )
}

lint_dotnet_build() {        # CI: dotnet.yml "Build & audit (C#)"
  # .dev.env carries DOTNET_BUILD_EXTRA_ARGS (-p:UseAppHost=false on Arch's arch-x64 RID).
  # shellcheck disable=SC1091
  [ -f .dev.env ] && . ./.dev.env
  # shellcheck disable=SC2086
  dotnet build Networker.sln --configuration Release -nologo -v quiet ${DOTNET_BUILD_EXTRA_ARGS:-}
}
lint_dotnet_format() {       # not in CI — no analyzers/.editorconfig in the repo; run `--fix --only dotnet-format` once, review, then promote
  # shellcheck disable=SC1091
  [ -f .dev.env ] && . ./.dev.env
  if [ "$FIX" = 1 ]; then dotnet format Networker.sln -v quiet; fi
  dotnet format Networker.sln --verify-no-changes -v quiet
}

lint_frontend_tsc() {        # CI: ci.yml "Dashboard frontend" (npx tsc -b; also inside npm run build)
  ( cd dashboard && npx --no-install tsc -b )
}
lint_frontend_eslint() {     # CI: ci.yml "Dashboard frontend" (added with this script — it was local-only before)
  ( cd dashboard && { [ "$FIX" = 0 ] || npx --no-install eslint . --fix; } && npx --no-install eslint . )
}
lint_sdk_js() {              # CI: sdk-conformance.yml runs the build (tsc); `lint` is its no-emit twin
  ( cd sdk/js && npm run --silent lint )
}

lint_sdk_go_fmt() {          # CI: sdk-conformance.yml "Go SDK conformance"
  ( cd sdk/go || exit 1
    [ "$FIX" = 1 ] && gofmt -w .
    unformatted="$(gofmt -l .)"
    [ -z "$unformatted" ] || { echo "gofmt found unformatted files:"; echo "$unformatted"; exit 1; } )
}
lint_sdk_go_vet() {          # not in CI
  ( cd sdk/go && go vet ./... )
}

lint_python_syntax() {       # not in CI; pure syntax check, writes no bytecode
  git ls-files '*.py' | python3 -c '
import ast, sys
bad = 0
for f in (l.strip() for l in sys.stdin if l.strip()):
    try:
        with open(f, encoding="utf-8") as fh: ast.parse(fh.read(), f)
    except SyntaxError as e:
        print(f"{f}:{e.lineno}: {e.msg}"); bad = 1
sys.exit(bad)'
}
lint_python_ruff() {         # not in CI; no ruff config in repo → default rule set
  mapfile -t PY < <(git ls-files '*.py')
  [ "$FIX" = 1 ] && { ruff check --fix "${PY[@]}"; ruff format "${PY[@]}"; }
  ruff check "${PY[@]}" && ruff format --check "${PY[@]}"
}

lint_shellcheck_installer() { # CI: test-installer.yml "shellcheck"
  # SC2034 = unused vars (many globals are state); SC1091 = sourced file not
  # found; SC2154 = referenced but not assigned (global state). Everything
  # else at warning severity and above is an error.
  SHELLCHECK --severity=warning --exclude=SC2034,SC1091,SC2154 install.sh
}
lint_shellcheck_all() {      # not in CI — baseline 2026-08-20: 25 findings in 7 files (lab/lab.sh, benchmarks/local-test.sh, …)
  mapfile -t SH < <(git ls-files '*.sh' | grep -v '^install\.sh$')
  SHELLCHECK --severity=warning --exclude=SC2034,SC1091,SC2154 -f gcc "${SH[@]}"
}
lint_bats_parse() {          # CI: test-installer.yml "bats" parses the cloud suites; extended here to every .bats
  rc=0
  for f in $(git ls-files '*.bats'); do
    n="$(bats --count "$f" 2>&1)" || { echo "$f: $n"; rc=1; continue; }
    [ "$n" -gt 0 ] 2>/dev/null || { echo "$f: parses to 0 tests"; rc=1; }
  done
  return $rc
}

lint_psa_installer() {       # CI: test-installer.yml "PSScriptAnalyzer (install.ps1)"
  pwsh -NoProfile -Command '
    $results = Invoke-ScriptAnalyzer -Path install.ps1 -Severity Warning,Error
    if ($results) { $results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host; exit 1 }
    Write-Host "PSScriptAnalyzer: no issues found."'
}
lint_ps51_parse() {          # CI: test-installer.yml — install.ps1 as Windows PowerShell 5.1 sees it (ANSI, no BOM)
  pwsh -NoProfile -Command '
    $bytes = [IO.File]::ReadAllBytes("install.ps1")
    $ansi = [Text.Encoding]::GetEncoding(1252).GetString($bytes)
    $errs = $null
    [System.Management.Automation.PSParser]::Tokenize($ansi, [ref]$errs) | Out-Null
    if ($errs.Count) { $errs | Select-Object -First 5 | Format-List | Out-String | Write-Host; exit 1 }
    Write-Host "cp1252-view tokenizes clean."'
}
lint_psa_all() {             # not in CI — baseline 2026-08-20: 144 findings across the other 6 .ps1 files
  # The file list travels in an env var ON PURPOSE: anything placed after the
  # `-Command` string is appended to the command text, so passing paths as
  # trailing args would EXECUTE the scripts instead of analysing them.
  LINT_PS_FILES="$(git ls-files '*.ps1' | grep -v '^install\.ps1$')" \
  pwsh -NoProfile -Command '
    $files = $env:LINT_PS_FILES -split "`n" | Where-Object { $_ }; $total = 0
    foreach ($f in $files) {
      $r = Invoke-ScriptAnalyzer -Path $f -Severity Warning,Error
      if ($r) { $total += @($r).Count; $r | Select-Object ScriptName,Line,RuleName,Severity | Format-Table -AutoSize | Out-String -Width 200 | Write-Host }
    }
    if ($total) { Write-Host "$total finding(s) across $($files.Count) file(s)"; exit 1 }
    Write-Host "clean: $($files.Count) file(s)"'
}

lint_action_pins() {         # CI: ci.yml "Action pins (SHA-pinned)" — incl. the guard-the-guard floor
  BAD=$(grep -rnE '^\s*(-\s*)?uses:\s*[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@' .github/workflows/*.yml | grep -vE '@[0-9a-f]{40}' || true)
  if [ -n "$BAD" ]; then
    echo "unpinned GitHub Action(s) — pin to a commit SHA (keep the version in a trailing comment):"
    echo "$BAD" | sed 's/^/  /'
    return 1
  fi
  # Guard the guard: if the grep stopped matching, an empty result would pass forever.
  TOTAL=$(grep -rcE '^\s*(-\s*)?uses:\s*[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@' .github/workflows/*.yml | awk -F: '{s+=$2} END{print s+0}')
  [ "$TOTAL" -ge 20 ] || { echo "only found ${TOTAL} action references — the scan is broken, not the workflows"; return 1; }
  echo "all ${TOTAL} action references are SHA-pinned"
}
lint_actionlint() {          # CI: ci.yml "lint-all (cross-cutting)" — structural workflow lint, shellcheck integration OFF
  ACTIONLINT -no-color -oneline -shellcheck=
}
lint_actionlint_sc() {       # not in CI — baseline 2026-08-20: 18 shellcheck findings inside run: blocks
  ACTIONLINT -no-color -oneline
}

lint_hadolint() {            # not in CI — baseline 2026-08-20: 13 warnings (lab/src/examples) + 12 (benchmarks/reference-apis)
  rc=0
  for f in $(git ls-files | grep -iE '(^|/)([^/]*\.)?Dockerfile$'); do
    out=$(HADOLINT --no-color - < "$f" 2>&1) || { rc=1; echo "$out" | sed "s|^-:|$f:|"; }
  done
  return $rc
}

lint_json_parse() {          # CI: ci.yml "lint-all (cross-cutting)" — plain syntax; the drift GUARDS are unit tests
  rc=0
  for f in $(git ls-files 'shared/*.json' 'examples/configs/*.json' 'benchmarks/configs/*.json' 'config.json'); do
    if command -v jq >/dev/null 2>&1; then jq empty "$f" >/dev/null 2>&1 || { echo "$f: invalid JSON"; rc=1; }
    else python3 -m json.tool "$f" >/dev/null 2>&1 || { echo "$f: invalid JSON"; rc=1; }; fi
  done
  return $rc
}
lint_drift_csharp() {        # CI: validate-bench-apis.yml "C# template drift check"
  python3 benchmarks/reference-apis/csharp-template/generate-variants.py --check
}

lint_version_sync() {        # CI: ci.yml "lint-all (cross-cutting)" — the consistency half of "Version bump check" (bump-vs-main needs origin/main)
  v="$(cargo_version)"; rc=0
  sh_v=$(grep -m1 'INSTALLER_VERSION=' install.sh | sed 's/.*"v\(.*\)".*/\1/')
  ps_v=$(grep -m1 'InstallerVersion' install.ps1 | sed 's/.*"v\(.*\)".*/\1/')
  props_v=$(grep -o '<Version>[^<]*</Version>' Directory.Build.props | head -1 | sed 's/<[^>]*>//g')
  [ "$sh_v" = "$v" ]    || { echo "install.sh INSTALLER_VERSION ($sh_v) != Cargo.toml ($v)"; rc=1; }
  [ "$ps_v" = "$v" ]    || { echo "install.ps1 InstallerVersion ($ps_v) != Cargo.toml ($v)"; rc=1; }
  [ "$props_v" = "$v" ] || { echo "Directory.Build.props <Version> ($props_v) != Cargo.toml ($v)"; rc=1; }
  grep -q "## \[${v}\]" CHANGELOG.md || { echo "CHANGELOG.md has no ## [${v}] section"; rc=1; }
  [ $rc = 0 ] && echo "version ${v} consistent across Cargo.toml, install.sh, install.ps1, Directory.Build.props, CHANGELOG.md"
  return $rc
}

lint_markdown() {            # not in CI; no config → markdownlint defaults (expect noise)
  mapfile -t MD < <(git ls-files '*.md')
  [ "$FIX" = 1 ] && markdownlint --fix "${MD[@]}"
  markdownlint "${MD[@]}"
}
lint_yaml() {                # not in CI
  yamllint -d relaxed .github/workflows docker-compose.dashboard.yml docker-compose.db.yml lab/docker-compose.yml examples/docker-compose.yml benchmarks/validate/docker-compose.yml docs/examples
}
lint_toml() {                # not in CI
  mapfile -t TOML < <(git ls-files 'Cargo.toml' '*/Cargo.toml' '**/Cargo.toml' 'supply-chain/*.toml' '.cargo/*.toml')
  [ "$FIX" = 1 ] && taplo fmt "${TOML[@]}"
  taplo fmt --check "${TOML[@]}"
}

# ── Runner ──────────────────────────────────────────────────────────────────
if [ "$LIST" = 1 ]; then
  printf '%-20s %-11s %-5s %-6s %s\n' ID GROUP TIER BUILD TOOLS
  while IFS= read -r row; do
    printf '%-20s %-11s %-5s %-6s %s\n' "$(field "$row" 1)" "$(field "$row" 2)" "$(field "$row" 3)" "$(field "$row" 4)" "$(field "$row" 5)"
  done <<< "$SECTIONS"
  exit 0
fi

LOG="$(mktemp)"; trap 'rm -f "$LOG"' EXIT

while IFS= read -r row; do
  id=$(field "$row" 1); group=$(field "$row" 2); tier=$(field "$row" 3)
  builds=$(field "$row" 4); tools=$(field "$row" 5); desc=$(field "$row" 6)
  selected "$id" "$group" || continue
  if [ "$NO_BUILD" = 1 ] && [ "$builds" = yes ]; then continue; fi

  printf '%s▶ %-20s%s %-62s' "$B" "$id" "$N" "$desc"

  missing=""
  for t in $tools; do have_tool "$t" || missing="$missing $t"; done
  if [ -n "$missing" ]; then
    if [ "$CI" = 1 ] && [ "$tier" = block ]; then
      printf '%s ✗ tool missing:%s%s\n' "$R" "$missing" "$N"
      for t in $missing; do echo "::error::lint-all/$id: missing tool $t — $(hint "$t")"; done
      FAIL=$((FAIL + 1)); FAILED_IDS="$FAILED_IDS $id"
    else
      printf '%s – SKIP (tool missing:%s)%s\n' "$Y" "$missing" "$N"
      for t in $missing; do printf '    %s→ %s%s\n' "$D" "$(hint "$t")" "$N"; done
      SKIPPED=$((SKIPPED + 1))
    fi
    continue
  fi

  fn="lint_$(echo "$id" | tr '-' '_')"
  if "$fn" >"$LOG" 2>&1; then
    printf '%s ✓%s\n' "$G" "$N"; PASS=$((PASS + 1))
  elif [ "$tier" = info ] && [ "$STRICT" = 0 ]; then
    printf '%s ! findings (info — not gating; --strict to enforce)%s\n' "$Y" "$N"
    WARN=$((WARN + 1)); WARNED_IDS="$WARNED_IDS $id"
    if [ "${LINT_VERBOSE:-0}" = 1 ]; then sed 's/^/    /' "$LOG"; else tail -n 15 "$LOG" | sed 's/^/    /'; fi
  else
    printf '%s ✗%s\n' "$R" "$N"; FAIL=$((FAIL + 1)); FAILED_IDS="$FAILED_IDS $id"
    if [ "${LINT_VERBOSE:-0}" = 1 ]; then sed 's/^/    /' "$LOG"; else tail -n 40 "$LOG" | sed 's/^/    /'; fi
    [ "$CI" = 1 ] && echo "::error::lint-all/$id failed — see log above"
  fi
done <<< "$SECTIONS"

echo
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
printf '%s%d passed%s, %s%d failed%s, %s%d with findings%s, %d skipped\n' \
  "$G" "$PASS" "$N" "$R" "$FAIL" "$N" "$Y" "$WARN" "$N" "$SKIPPED"
[ -n "$WARNED_IDS" ] && printf '  findings (info):%s\n' "$WARNED_IDS"
[ -n "$FAILED_IDS" ] && printf '  failed:%s\n' "$FAILED_IDS"
[ "$FAIL" -eq 0 ]
