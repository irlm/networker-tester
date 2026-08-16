# syntax=docker/dockerfile:1.7
# ─── Runner: what a tester VM looks like after the cloud-init bootstrap ──────
# Ubuntu 24.04 (the default requested_os for provisioned testers) with
#   /usr/local/bin/networker-tester   (Rust probe engine, from nwk-lab/rustbin)
#   /usr/local/bin/networker-agent    (self-contained C# Networker.Agent publish)
# and the same env contract the systemd unit passes on a real VM
# (AGENT_DASHBOARD_URL + AGENT_API_KEY). The agent runs as PID 1 via the
# entrypoint (there is no systemd in the container; that is the ONE documented
# fidelity gap versus a VM). tshark + ping sysctl mirror the bootstrap so
# capture_mode and ping-mode probes behave as they do on a provisioned runner.
ARG DOTNET_SDK_IMAGE=mcr.microsoft.com/dotnet/sdk:10.0
ARG RUSTBIN_IMAGE=nwk-lab/rustbin:local
FROM ${RUSTBIN_IMAGE} AS rustbin

FROM ${DOTNET_SDK_IMAGE} AS build
ARG TARGETARCH
WORKDIR /src
COPY Directory.Build.props ./
COPY src/Networker.Agent/Networker.Agent.csproj src/Networker.Agent/
RUN --mount=type=cache,target=/root/.nuget/packages \
    dotnet restore src/Networker.Agent/Networker.Agent.csproj \
      -r linux-$( [ "$TARGETARCH" = "arm64" ] && echo arm64 || echo x64 )
COPY src/Networker.Agent src/Networker.Agent
COPY benchmarks/configs/apibench.json benchmarks/configs/apibench.json
# Same publish shape as the release asset networker-agent-cs-linux-*.tar.gz:
# self-contained single-file, so the runtime image needs no dotnet install.
RUN --mount=type=cache,target=/root/.nuget/packages \
    RID=linux-$( [ "$TARGETARCH" = "arm64" ] && echo arm64 || echo x64 ) \
 && dotnet publish src/Networker.Agent/Networker.Agent.csproj \
      -c Release -r "$RID" --self-contained true --no-restore -o /out \
 && ls -la /out

FROM ubuntu:24.04 AS runtime
ARG WITH_CHROMIUM=0
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq \
 && apt-get install -y -qq --no-install-recommends \
      ca-certificates curl tar iproute2 iputils-ping procps jq \
      tshark libcap2-bin \
 && ( [ "$WITH_CHROMIUM" = "1" ] && apt-get install -y -qq --no-install-recommends chromium || true ) \
 && rm -rf /var/lib/apt/lists/* \
 && setcap cap_net_raw,cap_net_admin=eip "$(command -v dumpcap)" || true
COPY --from=rustbin /networker-tester /usr/local/bin/networker-tester
COPY --from=build   /out/networker-agent /usr/local/bin/networker-agent
COPY lab/images/runner/entrypoint.sh /usr/local/bin/lab-runner-entrypoint
RUN chmod 0755 /usr/local/bin/networker-tester /usr/local/bin/networker-agent /usr/local/bin/lab-runner-entrypoint \
 && /usr/local/bin/networker-tester --version
ENV AGENT_DASHBOARD_URL=ws://controlplane:5030/ws/agent \
    RUST_LOG=info \
    DOTNET_EnableDiagnostics=0
ENTRYPOINT ["/usr/local/bin/lab-runner-entrypoint"]
