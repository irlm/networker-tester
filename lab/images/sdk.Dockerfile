# syntax=docker/dockerfile:1.7
# ─── SDK target: a customer app instrumented with the LagHound SDK ────────────
# `sdkprobe` is the one catalog mode that needs neither a networker-endpoint nor
# a proxy stack but a CUSTOMER application carrying the SDK middleware, so it was
# the only mode the lab (and the prod sweep) could not exercise. This image is the
# repo's own sample app (sdk/csharp/Example): it mounts LagHound at /laghound —
# so /laghound/echo answers with the Server-Timing the probe measures — plus two
# ordinary app routes (/ and /work) so the container also looks like a real app.
#
# The token is a build/run-time env var; lab.sh registers the endpoint through
# POST /api/projects/{id}/sdk-endpoints so the control plane stores it ENCRYPTED
# and the dispatcher splices it into the run exactly as in production.
ARG DOTNET_SDK_IMAGE=mcr.microsoft.com/dotnet/sdk:10.0
ARG DOTNET_RUNTIME_IMAGE=mcr.microsoft.com/dotnet/aspnet:10.0
FROM ${DOTNET_SDK_IMAGE} AS build
WORKDIR /src
COPY sdk/csharp/LagHound.Endpoint/LagHound.Endpoint.csproj sdk/csharp/LagHound.Endpoint/
COPY sdk/csharp/Example/Example.csproj                     sdk/csharp/Example/
RUN --mount=type=cache,target=/root/.nuget/packages \
    dotnet restore sdk/csharp/Example/Example.csproj
COPY sdk/csharp/LagHound.Endpoint sdk/csharp/LagHound.Endpoint
COPY sdk/csharp/Example           sdk/csharp/Example
RUN --mount=type=cache,target=/root/.nuget/packages \
    dotnet publish sdk/csharp/Example/Example.csproj -c Release --no-restore -o /app

FROM ${DOTNET_RUNTIME_IMAGE} AS runtime
RUN apt-get update -qq \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends curl \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /app .
# PORT/LAGHOUND_TOKEN are the sample's own knobs (see sdk/csharp/Example/Program.cs).
ENV PORT=8081 \
    LAGHOUND_TOKEN=lab-sdk-token-0123456789abcdef \
    DOTNET_EnableDiagnostics=0
EXPOSE 8081
HEALTHCHECK --interval=5s --timeout=3s --start-period=20s --retries=20 \
  CMD curl -fsS http://127.0.0.1:8081/ || exit 1
ENTRYPOINT ["dotnet", "LagHound.Example.dll"]
