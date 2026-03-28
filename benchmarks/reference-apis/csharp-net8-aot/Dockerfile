# Build stage: must use the SDK image for AOT compilation.
# Native AOT requires building on the target OS — no cross-compilation.
FROM mcr.microsoft.com/dotnet/sdk:8.0 AS build

# Native AOT requires clang and build tools
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang zlib1g-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY csharp-net8-aot.csproj .
RUN dotnet restore -r linux-x64

COPY Program.cs .
RUN dotnet publish -c Release -r linux-x64 -o /app --no-restore

# Runtime stage: no .NET runtime needed — AOT produces a native binary.
# Use a minimal base image.
FROM mcr.microsoft.com/dotnet/runtime-deps:8.0
WORKDIR /opt/bench
COPY --from=build /app/csharp-net8-aot ./server

EXPOSE 8443
ENTRYPOINT ["./server"]
