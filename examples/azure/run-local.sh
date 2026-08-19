#!/usr/bin/env bash
set -euo pipefail

# Build and run the same two images used by the Azure deployment, locally.
# The containers listen on 8080 internally just like Azure Container Apps.

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"

CSHARP_IMAGE="${CSHARP_IMAGE:-laghound-csharp-demo:local}"
RUST_IMAGE="${RUST_IMAGE:-laghound-rust-demo:local}"
CSHARP_CONTAINER="${CSHARP_CONTAINER:-laghound-csharp-demo-local}"
RUST_CONTAINER="${RUST_CONTAINER:-laghound-rust-demo-local}"
CSHARP_PORT="${CSHARP_PORT:-18081}"
RUST_PORT="${RUST_PORT:-18084}"
LAGHOUND_TOKEN="${LAGHOUND_TOKEN:-demo-token-laghound}"
PUBLIC_DEMO_VALUE="${PUBLIC_DEMO_VALUE:-1}"

if [ "${#LAGHOUND_TOKEN}" -lt 16 ]; then
  echo "LAGHOUND_TOKEN must be at least 16 bytes" >&2
  exit 1
fi

command -v docker >/dev/null 2>&1 || {
  echo "Docker CLI is required" >&2
  exit 1
}
docker info >/dev/null

cleanup() {
  docker rm -f "$CSHARP_CONTAINER" "$RUST_CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker build --file "$REPO_ROOT/examples/csharp.Dockerfile" --tag "$CSHARP_IMAGE" "$REPO_ROOT"
docker build --file "$REPO_ROOT/examples/rust.Dockerfile" --tag "$RUST_IMAGE" "$REPO_ROOT"

docker run --detach --init \
  --name "$CSHARP_CONTAINER" \
  --publish "$CSHARP_PORT:8080" \
  --env PORT=8080 \
  --env "LAGHOUND_PUBLIC_DEMO=$PUBLIC_DEMO_VALUE" \
  --env "LAGHOUND_TOKEN=$LAGHOUND_TOKEN" \
  "$CSHARP_IMAGE" >/dev/null

docker run --detach --init \
  --name "$RUST_CONTAINER" \
  --publish "$RUST_PORT:8080" \
  --env PORT=8080 \
  --env "LAGHOUND_PUBLIC_DEMO=$PUBLIC_DEMO_VALUE" \
  --env "LAGHOUND_TOKEN=$LAGHOUND_TOKEN" \
  "$RUST_IMAGE" >/dev/null

wait_for_page() {
  url="$1"
  for _ in $(seq 1 30); do
    if curl -fsS --max-time 2 "$url" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  echo "Timed out waiting for $url" >&2
  return 1
}

assert_contains() {
  url="$1"
  expected="$2"
  body="$(curl -fsS --max-time 5 "$url")"
  case "$body" in
    *"$expected"*) ;;
    *) echo "Expected '$expected' in $url" >&2; return 1 ;;
  esac
}

assert_authenticated_contains() {
  url="$1"
  expected="$2"
  body="$(curl -fsS --max-time 5 -H "X-LagHound-Token: $LAGHOUND_TOKEN" "$url")"
  case "$body" in
    *"$expected"*) ;;
    *) echo "Expected '$expected' in authenticated response from $url" >&2; return 1 ;;
  esac
}

assert_not_contains() {
  url="$1"
  unexpected="$2"
  body="$(curl -fsS --max-time 5 "$url")"
  case "$body" in
    *"$unexpected"*) echo "Unexpected '$unexpected' in $url" >&2; return 1 ;;
    *) ;;
  esac
}

assert_status() {
  expected="$1"
  url="$2"
  actual="$(curl -sS --max-time 5 -o /dev/null -w '%{http_code}' \
    -H "X-LagHound-Token: $LAGHOUND_TOKEN" "$url")"
  if [ "$actual" != "$expected" ]; then
    echo "Expected HTTP $expected from $url, got $actual" >&2
    return 1
  fi
}

wait_for_page "http://127.0.0.1:$CSHARP_PORT/"
wait_for_page "http://127.0.0.1:$RUST_PORT/"
assert_contains "http://127.0.0.1:$CSHARP_PORT/" "LIVE REFERENCE"
assert_contains "http://127.0.0.1:$RUST_PORT/" "LIVE REFERENCE"
assert_contains "http://127.0.0.1:$CSHARP_PORT/" "C# · ASP.NET CORE"
assert_contains "http://127.0.0.1:$RUST_PORT/" "RUST · AXUM / TOWER"
assert_not_contains "http://127.0.0.1:$CSHARP_PORT/" "{{"
assert_not_contains "http://127.0.0.1:$RUST_PORT/" "{{"
assert_contains "http://127.0.0.1:$CSHARP_PORT/work" "C# handler completed"
assert_contains "http://127.0.0.1:$RUST_PORT/work" "Rust handler completed"
assert_authenticated_contains "http://127.0.0.1:$CSHARP_PORT/laghound/health" '"status":"ok"'
assert_authenticated_contains "http://127.0.0.1:$RUST_PORT/laghound/health" '"status":"ok"'
assert_status 404 "http://127.0.0.1:$CSHARP_PORT/laghound/download?bytes=128"
assert_status 404 "http://127.0.0.1:$RUST_PORT/laghound/download?bytes=128"

echo "Local SDK demo smoke test passed."
echo "C#  http://127.0.0.1:$CSHARP_PORT"
echo "Rust http://127.0.0.1:$RUST_PORT"
