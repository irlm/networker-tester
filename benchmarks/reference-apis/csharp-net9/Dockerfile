# --- Build stage ---
FROM mcr.microsoft.com/dotnet/sdk:9.0 AS build
WORKDIR /src

COPY csharp-net9.csproj .
RUN dotnet restore -r linux-x64

COPY Program.cs .
RUN dotnet publish -c Release -r linux-x64 --self-contained -o /app/publish

# --- Runtime stage ---
FROM mcr.microsoft.com/dotnet/runtime-deps:9.0 AS runtime
WORKDIR /app

COPY --from=build /app/publish .

EXPOSE 8443

ENV BENCH_CERT_DIR=/opt/bench
ENV BENCH_PORT=8443

ENTRYPOINT ["./csharp-net9"]
