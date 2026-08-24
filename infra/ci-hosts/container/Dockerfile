# A networker CI host, containerised.
#
# Why this exists: a machine that is NOT dedicated CI — one already running
# nginx, Samba, ollama and someone's containers — can still contribute runner
# capacity, as long as the runner cannot reach any of it. A bare-metal CI host
# cannot promise that: the ephemeral loop's between-jobs sweep does
# `docker ps -aq | xargs docker rm -f`, wipes /tmp, and `pkill -u` the CI user.
# On a shared machine each of those is a live grenade.
#
# In here, all three are harmless. The container's /tmp is its own, `pkill`
# reaches only its own processes, and `docker` talks to a DinD sidecar
# (DOCKER_HOST), so the sweep can only ever delete containers the CI jobs
# themselves created. The host's Docker socket is deliberately NOT mounted.
#
# The image runs the REAL infra/ci-hosts/linux/install-ci-host.sh, so a
# containerised host and a VM host get the same toolchains from the same code —
# a second install script would drift within a release.
#
# Build context is the REPO ROOT:
#   docker build -f infra/ci-hosts/container/Dockerfile -t networker-ci-host .
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# install-ci-host.sh needs these before it can run at all.
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends \
      ca-certificates curl jq sudo lsb-release gnupg \
 && rm -rf /var/lib/apt/lists/*

COPY infra/ci-hosts/linux/install-ci-host.sh /tmp/install-ci-host.sh

# Toolchains and the actions-runner package are baked in; registration is not.
# The PAT and the repo arrive at RUN time (entrypoint.sh), so nothing secret and
# nothing environment-specific is ever committed to a layer. The placeholder
# token exists only to satisfy the installer's up-front validation and is
# removed in the same layer it is created in.
ARG RUNNER_VERSION=""
RUN install -d -m 0700 /etc/ci-host \
 && printf 'placeholder-not-a-real-pat\n' > /etc/ci-host/token \
 && chmod 0600 /etc/ci-host/token \
 && bash /tmp/install-ci-host.sh \
      --container \
      --repo placeholder/placeholder \
      --name placeholder \
      ${RUNNER_VERSION:+--runner-version "$RUNNER_VERSION"} \
 && rm -f /etc/ci-host/token /etc/ci-host/env /tmp/install-ci-host.sh \
 && rm -rf /var/lib/apt/lists/*

COPY infra/ci-hosts/container/entrypoint.sh /usr/local/bin/ci-host-entrypoint.sh
RUN chmod 0755 /usr/local/bin/ci-host-entrypoint.sh

ENTRYPOINT ["/usr/local/bin/ci-host-entrypoint.sh"]
