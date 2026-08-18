# syntax=docker/dockerfile:1.7
# ─── Networker.Agent for the lab's WINDOWS runners — published FROM THE CHECKOUT ──
# The Windows runner VM (lab.sh up --windows-runners M) cannot build .NET
# itself before it is up, and nothing here should need a host toolchain, so
# the agent is published inside the dotnet SDK image (cross-RID publish
# linux → win-x64 is a first-class .NET feature; no apphost quirks) and
# exported to the VM's /oem staging folder with BuildKit `--output`:
#
#   docker build -f lab/images/agent-win.Dockerfile --target export \
#     --output type=local,dest=lab/.generated/windows-runner-K/oem/agent .
#
# Same publish shape as the release asset networker-agent-cs-win-x64.zip
# (self-contained single-file: networker-agent.exe + the native runtime libs).
ARG DOTNET_SDK_IMAGE=mcr.microsoft.com/dotnet/sdk:10.0
FROM ${DOTNET_SDK_IMAGE} AS build
WORKDIR /src
COPY Directory.Build.props ./
COPY src/Networker.Agent/Networker.Agent.csproj src/Networker.Agent/
RUN --mount=type=cache,target=/root/.nuget/packages \
    dotnet restore src/Networker.Agent/Networker.Agent.csproj -r win-x64
COPY src/Networker.Agent src/Networker.Agent
COPY benchmarks/configs/apibench.json benchmarks/configs/apibench.json
RUN --mount=type=cache,target=/root/.nuget/packages \
    dotnet publish src/Networker.Agent/Networker.Agent.csproj \
      -c Release -r win-x64 --self-contained true --no-restore -o /out \
 && ls -la /out

FROM scratch AS export
COPY --from=build /out /
