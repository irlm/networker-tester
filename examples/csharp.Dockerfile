# LagHound C# sample — ASP.NET Core (net10).
# Build context MUST be the repo root: `docker build -f examples/csharp.Dockerfile .`
# Multi-stage: full SDK to publish, thin aspnet runtime to run.
FROM mcr.microsoft.com/dotnet/sdk:10.0 AS build
WORKDIR /src

# The Example project references ../LagHound.Endpoint, so copy the whole csharp
# tree (excludes tests via publish — they aren't referenced by Example).
COPY sdk/csharp/LagHound.Endpoint ./LagHound.Endpoint
COPY sdk/csharp/Example ./Example

RUN dotnet publish Example/Example.csproj -c Release -o /app --nologo

FROM mcr.microsoft.com/dotnet/aspnet:10.0 AS runtime
WORKDIR /app
# wget for the compose healthcheck (aspnet base ships neither curl nor wget).
RUN apt-get update && apt-get install -y --no-install-recommends wget \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app ./

# Container listens on 8081 internally (the sample's default PORT); compose maps
# it to the assigned host port. LAGHOUND_TOKEN comes from the environment.
ENV PORT=8081
EXPOSE 8081
ENTRYPOINT ["dotnet", "LagHound.Example.dll"]
