#!/usr/bin/env bash
#
# prod-smoke.sh — post-deploy UI smoke against the LIVE dashboard.
#
# Runs the headless Playwright suite in dashboard/e2e/prod/ against
# PROD_SMOKE_BASE_URL (default https://laghound.com), capturing a full-page
# screenshot per spec. On ANY failure it files (or comments on) a GitHub
# issue per failed spec with the screenshot attached via a secret gist.
#
# The suite is strictly READ-ONLY: it never launches runs, creates configs,
# deploys VMs, or mutates anything. See docs/prod-smoke.md.
#
# Config: sources the git-ignored .prod-smoke.env at the repo root.
#   PROD_SMOKE_TOKEN=<jwt>            (required — see docs/prod-smoke.md)
#   PROD_SMOKE_PROJECT_ID=<uuid>      (required)
#   PROD_SMOKE_BASE_URL=<url>         (optional, default https://laghound.com)
#   PROD_SMOKE_COMPARE_GROUP=<uuid>   (optional — enables the pivots spec)
#
# Bash 3.2-safe (no associative arrays, no readarray).

set -u

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DASHBOARD_DIR="$REPO_ROOT/dashboard"
ARTIFACTS_DIR="$DASHBOARD_DIR/e2e/prod/artifacts"
RESULTS_JSON="$ARTIFACTS_DIR/results.json"
FAILURES_DIR="$ARTIFACTS_DIR/failures"
ENV_FILE="$REPO_ROOT/.prod-smoke.env"

log() { printf '%s\n' "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

# ── Environment ─────────────────────────────────────────────────────────
if [ -f "$ENV_FILE" ]; then
    set -a
    # shellcheck source=/dev/null
    . "$ENV_FILE"
    set +a
else
    log "note: $ENV_FILE not found — relying on already-exported PROD_SMOKE_* vars"
fi

[ -n "${PROD_SMOKE_TOKEN:-}" ] || die "PROD_SMOKE_TOKEN is not set (add it to .prod-smoke.env — see docs/prod-smoke.md)"
[ -n "${PROD_SMOKE_PROJECT_ID:-}" ] || die "PROD_SMOKE_PROJECT_ID is not set (add it to .prod-smoke.env — see docs/prod-smoke.md)"
BASE_URL="${PROD_SMOKE_BASE_URL:-https://laghound.com}"

command -v node > /dev/null 2>&1 || die "node is required"
command -v npx > /dev/null 2>&1 || die "npx is required"

# ── Run the suite ───────────────────────────────────────────────────────
mkdir -p "$ARTIFACTS_DIR"
rm -rf "$FAILURES_DIR"
rm -f "$RESULTS_JSON"

log "prod smoke: $BASE_URL (project $PROD_SMOKE_PROJECT_ID)"
(
    cd "$DASHBOARD_DIR" || exit 1
    npx playwright test --config playwright.prod.config.ts < /dev/null
)
PLAYWRIGHT_EXIT=$?

if [ "$PLAYWRIGHT_EXIT" -eq 0 ]; then
    log "prod smoke PASSED against $BASE_URL — screenshots in $ARTIFACTS_DIR"
    exit 0
fi

# ── Failure path: extract failed specs from the JSON reporter output ────
[ -f "$RESULTS_JSON" ] || die "playwright failed (exit $PLAYWRIGHT_EXIT) but $RESULTS_JSON is missing — cannot file issues"

mkdir -p "$FAILURES_DIR"
FAILED_COUNT=$(node - "$RESULTS_JSON" "$FAILURES_DIR" << 'NODE_EOF'
const fs = require('fs');
const path = require('path');
const results = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const outDir = process.argv[3];
// Must match slugify() in dashboard/e2e/prod/support.ts (screenshot names).
const slugify = (t) => t.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
const stripAnsi = (s) => s.replace(new RegExp(String.fromCharCode(27) + '\\[[0-9;:?]*[ -/]*[@-~]', 'g'), '');
let n = 0;
function walk(suite, file) {
  (suite.suites || []).forEach((s) => walk(s, s.file || file));
  (suite.specs || []).forEach((spec) => {
    if (spec.ok) return;
    const skipped = (spec.tests || []).every((t) =>
      (t.results || []).every((r) => r.status === 'skipped'));
    if (skipped) return;
    const messages = [];
    (spec.tests || []).forEach((t) => (t.results || []).forEach((r) => {
      if (r.error && r.error.message) messages.push(stripAnsi(r.error.message));
    }));
    n += 1;
    const id = String(n).padStart(3, '0');
    fs.writeFileSync(path.join(outDir, id + '.title'), spec.title + '\n');
    fs.writeFileSync(path.join(outDir, id + '.slug'), slugify(spec.title) + '\n');
    fs.writeFileSync(path.join(outDir, id + '.file'), (spec.file || file || 'unknown') + '\n');
    fs.writeFileSync(path.join(outDir, id + '.error'),
      (messages[0] || '(no assertion message captured — see the trace in artifacts/test-output)') + '\n');
  });
}
(results.suites || []).forEach((s) => walk(s, s.file));
console.log(n);
NODE_EOF
) || die "failed to parse $RESULTS_JSON"

if [ -z "$FAILED_COUNT" ] || [ "$FAILED_COUNT" -eq 0 ]; then
    die "playwright failed (exit $PLAYWRIGHT_EXIT) but no failed specs were parsed from $RESULTS_JSON"
fi
log "prod smoke FAILED: $FAILED_COUNT spec(s) — filing GitHub issues"

command -v gh > /dev/null 2>&1 || die "gh CLI is required to file failure issues (suite still failed; artifacts in $ARTIFACTS_DIR)"
command -v curl > /dev/null 2>&1 || die "curl is required"

# Prod version for the issue title, straight from the deployed control plane.
PROD_VERSION=$(curl -fsS --max-time 15 "$BASE_URL/api/health" < /dev/null 2> /dev/null \
    | node -e 'let d="";process.stdin.on("data",c=>d+=c).on("end",()=>{try{console.log(JSON.parse(d).version||"unknown")}catch{console.log("unknown")}})' \
    2> /dev/null)
[ -n "$PROD_VERSION" ] || PROD_VERSION="unknown"

ISSUE_EXIT=0
for title_file in "$FAILURES_DIR"/*.title; do
    [ -f "$title_file" ] || continue
    id_base="${title_file%.title}"
    spec_title=$(head -n1 "$title_file")
    spec_slug=$(head -n1 "$id_base.slug")
    spec_file=$(head -n1 "$id_base.file")
    screenshot="$ARTIFACTS_DIR/$spec_slug.png"

    # ── Screenshot → secret gist ─────────────────────────────────────────
    gist_url=""
    if [ -f "$screenshot" ]; then
        # Gists are text-only: ship the PNG base64-encoded (decode with
        # `base64 -d shot.png.b64.txt > shot.png`).
        b64_file="$ARTIFACTS_DIR/$spec_slug.png.b64.txt"
        base64 < "$screenshot" > "$b64_file"
        gist_url=$(gh gist create "$b64_file" \
            --desc "prod smoke failure screenshot: $spec_title (base64 PNG — decode with base64 -d)" \
            < /dev/null 2> /dev/null | tail -n1)
        [ -n "$gist_url" ] || gist_url="(gist upload failed — screenshot at $screenshot)"
    else
        gist_url="(no screenshot captured — trace in $ARTIFACTS_DIR/test-output)"
    fi

    issue_title="prod smoke failure: $spec_title (v$PROD_VERSION from /api/health)"

    body_file="$id_base.issue-body.md"
    {
        printf '## Prod UI smoke failure\n\n'
        printf -- '- **Spec**: `%s` (`dashboard/e2e/prod/%s`)\n' "$spec_title" "$spec_file"
        printf -- '- **Prod version**: `v%s` (from `%s/api/health`)\n' "$PROD_VERSION" "$BASE_URL"
        printf -- '- **Base URL**: %s\n' "$BASE_URL"
        printf -- '- **Screenshot**: %s\n\n' "$gist_url"
        printf '### Assertion error\n\n```\n'
        cat "$id_base.error"
        printf '```\n\n'
        printf '### Checklist\n\n'
        printf -- '- [ ] Reproduce: `./scripts/prod-smoke.sh` (needs `.prod-smoke.env`, see docs/prod-smoke.md)\n'
        printf -- '- [ ] Decode + review the screenshot (`base64 -d` the gist file) and the trace in `dashboard/e2e/prod/artifacts/test-output/`\n'
        printf -- '- [ ] Check what shipped in v%s (CHANGELOG.md / the release deploy)\n' "$PROD_VERSION"
        printf -- '- [ ] Fix forward or roll back per docs/release-flow.md\n'
        printf -- '- [ ] Re-run the smoke to confirm green\n'
    } > "$body_file"

    # ── De-dupe: comment on an open issue for the same spec if one exists ─
    existing=$(gh issue list --state open \
        --search "\"prod smoke failure: $spec_title\" in:title" \
        --json number --jq '.[0].number' < /dev/null 2> /dev/null)
    if [ -n "$existing" ] && [ "$existing" != "null" ]; then
        log "  '$spec_title' → commenting on existing open issue #$existing"
        if ! gh issue comment "$existing" --body-file "$body_file" < /dev/null > /dev/null; then
            log "  failed to comment on issue #$existing"
            ISSUE_EXIT=1
        fi
    else
        log "  '$spec_title' → creating issue"
        if ! gh issue create --title "$issue_title" --body-file "$body_file" < /dev/null; then
            log "  failed to create issue for '$spec_title'"
            ISSUE_EXIT=1
        fi
    fi
done

if [ "$ISSUE_EXIT" -ne 0 ]; then
    log "prod smoke FAILED and some issues could not be filed — artifacts in $ARTIFACTS_DIR"
else
    log "prod smoke FAILED — issues filed; artifacts in $ARTIFACTS_DIR"
fi
exit 1
