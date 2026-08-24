#!/usr/bin/env bash
# Turn the container's environment into the ephemeral loop's config, then run
# the loop in the foreground. The container runtime provides the `Restart=always`
# that systemd provides on a VM host.
#
# Everything registration-specific lives here rather than in the image: the same
# image serves any repo, name and label set, and the PAT arrives as a mounted
# file that is never in a layer.
set -euo pipefail

log() { printf '\033[1;34m[ci-host]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[ci-host] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

REPO="${CI_HOST_REPO:?CI_HOST_REPO=OWNER/REPO is required}"
NAME="${CI_HOST_NAME:-$(hostname -s)}"
LABELS="${CI_HOST_LABELS:-self-hosted,linux,networker-ci}"
GROUP="${CI_HOST_RUNNER_GROUP:-}"
TOKEN_FILE="${CI_HOST_TOKEN_FILE:-/run/secrets/ci-host-token}"
CI_USER="${CI_HOST_USER:-ci}"
RUNNER_DIR="${CI_HOST_RUNNER_DIR:-/opt/actions-runner}"
CACHE_DIR="${CI_HOST_CACHE_DIR:-/var/cache/ci-host}"

[ -s "$TOKEN_FILE" ] || die "PAT file $TOKEN_FILE is missing or empty (mount it read-only)"

# The daemon is the sidecar's, never the host's. Refusing to start without it is
# deliberate: silently falling back to a mounted host socket would hand CI jobs
# — and the between-jobs `docker rm -f` sweep — the machine's real containers,
# which is the one thing this whole arrangement exists to prevent.
[ -n "${DOCKER_HOST:-}" ] || die "DOCKER_HOST must point at the DinD sidecar"
log "waiting for the docker sidecar at $DOCKER_HOST"
for i in $(seq 1 60); do
  if docker info >/dev/null 2>&1; then
    log "docker sidecar ready after ~${i}s ($(docker version --format '{{.Server.Version}}' 2>/dev/null))"
    break
  fi
  [ "$i" = 60 ] && die "docker sidecar never became ready at $DOCKER_HOST"
  sleep 1
done

CI_HOME="$(getent passwd "$CI_USER" | cut -d: -f6)"
install -d -m 0755 "$CACHE_DIR"
for d in npm nuget dotnet-cli playwright sccache; do
  install -d -o "$CI_USER" -g "$CI_USER" -m 0755 "$CACHE_DIR/$d"
done
chown -R "$CI_USER:$CI_USER" "$RUNNER_DIR"

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

# DOCKER_HOST has to reach the JOB, not just this script: the runner hands jobs
# the contents of RUNNER_DIR/.env.
grep -q '^DOCKER_HOST=' "$RUNNER_DIR/.env" 2>/dev/null \
  || printf 'DOCKER_HOST=%s\n' "$DOCKER_HOST" >> "$RUNNER_DIR/.env"
chown "$CI_USER:$CI_USER" "$RUNNER_DIR/.env"

log "CI host '$NAME' for $REPO — labels $LABELS"
set -a
# shellcheck disable=SC1091  # generated immediately above, by this script
. /etc/ci-host/env
set +a
exec /usr/local/bin/ci-host-loop.sh
