# syntax=docker/dockerfile:1.7
# ─── Target: an endpoint VM in a container ───────────────────────────────────
# Ubuntu 24.04 + /usr/local/bin/networker-endpoint (from nwk-lab/rustbin) and,
# for STACK != none, the comparison proxy installed BY THE REAL install.sh
# (`--setup-stack <stack>`, the exact code path the deploy runner pipes over SSH
# to cloud endpoint VMs). systemd is replaced by lab/images/target/systemctl,
# a shim that runs the unit files' ExecStart directly — so the proxy configs,
# certs, static site and port layout (nginx 8081/8444, caddy 8454, traefik 8455,
# haproxy 8456, apache 8094/8457) are byte-identical to what a VM gets.
ARG RUSTBIN_IMAGE=nwk-lab/rustbin:local
FROM ${RUSTBIN_IMAGE} AS rustbin

FROM ubuntu:24.04
ARG STACK=none
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends \
      ca-certificates curl tar sudo openssl gnupg lsb-release \
      procps iproute2 net-tools psmisc jq bash \
 && rm -rf /var/lib/apt/lists/*
COPY --from=rustbin /networker-endpoint /usr/local/bin/networker-endpoint
COPY lab/images/target/systemctl /usr/local/sbin/systemctl
COPY lab/images/target/entrypoint.sh /usr/local/bin/lab-target-entrypoint
COPY install.sh /opt/networker/install.sh
RUN chmod 0755 /usr/local/bin/networker-endpoint /usr/local/sbin/systemctl /usr/local/bin/lab-target-entrypoint \
 && ln -sf /usr/local/sbin/systemctl /usr/bin/systemctl \
 && /usr/local/bin/networker-endpoint --version
# Run the real installer's single-stack setup at build time (network needed:
# apt repos / GitHub for caddy+traefik). The daemon really starts inside the
# build step (through the shim) so install.sh's OWN post-start health checks
# (e.g. apache's "is 8094 serving?") run exactly as on a VM; processes don't
# survive the layer, and the entrypoint clears stale pid files + restarts.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
RUN if [ "$STACK" != "none" ]; then \
      bash /opt/networker/install.sh --setup-stack "$STACK" 2>&1 | tail -n 60; \
      rm -rf /run/lab-systemctl /run/nginx.pid /run/apache2 /run/haproxy* /var/log/lab-*.log; \
    fi
ENV TARGET_STACK=${STACK}
EXPOSE 8080 8443 8443/udp 9997/udp 9998/udp 9999/udp 8081 8444 8444/udp 8454 8454/udp 8455 8456 8094 8457
HEALTHCHECK --interval=5s --timeout=3s --start-period=20s --retries=20 \
  CMD curl -fsS http://127.0.0.1:8080/health || exit 1
ENTRYPOINT ["/usr/local/bin/lab-target-entrypoint"]
