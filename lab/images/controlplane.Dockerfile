# syntax=docker/dockerfile:1.7
# ─── Networker.ControlPlane (C#) ─────────────────────────────────────────────
# Same code path as prod (`dotnet publish -c Release`), framework-dependent on
# the official aspnet runtime image. Migrations run on startup exactly like the
# laghound.com service. Build context = repo root (Directory.Build.props stamps
# the version; shared/ + assets/ are embedded resources).
ARG DOTNET_SDK_IMAGE=mcr.microsoft.com/dotnet/sdk:10.0
ARG DOTNET_RUNTIME_IMAGE=mcr.microsoft.com/dotnet/aspnet:10.0
FROM ${DOTNET_SDK_IMAGE} AS build
WORKDIR /src

# Restore first (layer-cached until a csproj/props changes).
COPY Directory.Build.props ./
COPY src/Networker.ControlPlane/Networker.ControlPlane.csproj src/Networker.ControlPlane/
COPY src/Networker.Data/Networker.Data.csproj                 src/Networker.Data/
COPY src/Networker.Contracts/Networker.Contracts.csproj       src/Networker.Contracts/
COPY src/Networker.Security/Networker.Security.csproj         src/Networker.Security/
RUN --mount=type=cache,target=/root/.nuget/packages \
    dotnet restore src/Networker.ControlPlane/Networker.ControlPlane.csproj

COPY src/Networker.ControlPlane src/Networker.ControlPlane
COPY src/Networker.Data         src/Networker.Data
COPY src/Networker.Contracts    src/Networker.Contracts
COPY src/Networker.Security     src/Networker.Security
COPY shared                     shared
COPY assets/brand               assets/brand
RUN --mount=type=cache,target=/root/.nuget/packages \
    dotnet publish src/Networker.ControlPlane/Networker.ControlPlane.csproj \
      -c Release --no-restore -o /app

FROM ${DOTNET_RUNTIME_IMAGE} AS runtime
# curl for the compose healthcheck + `lab.sh` diagnostics; the provisioning
# code shells out to az/aws/gcloud + install.sh — deliberately NOT installed:
# cloud provisioning is out of scope for the lab (runners/targets are docker
# containers registered directly), and a missing CLI is a captured
# ProvisionResult failure, never a crash.
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app .
ENV ASPNETCORE_URLS=http://0.0.0.0:5030 \
    DOTNET_EnableDiagnostics=0
EXPOSE 5030
HEALTHCHECK --interval=5s --timeout=3s --start-period=40s --retries=30 \
  CMD curl -fsS http://127.0.0.1:5030/api/health/ready || exit 1
ENTRYPOINT ["dotnet", "Networker.ControlPlane.dll"]
