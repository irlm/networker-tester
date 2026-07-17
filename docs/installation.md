# Installation and Startup

This guide covers the supported install paths, local development builds, and the fastest way to
start each major component in this repository.

## Components

- `networker-tester` (Rust): CLI probe runner
- `networker-endpoint` (Rust): HTTP/HTTPS/UDP target server
- `Networker.ControlPlane` (C#): control plane API (`/api` + `/ws`) — prod runs this
- `Networker.Agent` (C#): control-plane-connected worker that runs tester jobs
- `dashboard/` (React): browser SPA — served static by nginx in prod, Vite dev server locally

The legacy Rust control plane (`networker-dashboard`, `networker-agent`) is retired and off the
release train; see [`architecture.md`](architecture.md#retired-components-rust-control-plane).

## Install from the Hosted Scripts

### macOS and Linux

Install the tester:

```bash
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- tester
```

Install the endpoint:

```bash
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- endpoint
```

### Windows PowerShell

```powershell
$GistUrl = 'https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.ps1'

# Tester
Invoke-RestMethod $GistUrl | Invoke-Expression

# Endpoint
Invoke-WebRequest $GistUrl -OutFile "$env:TEMP\networker-install.ps1"
& "$env:TEMP\networker-install.ps1" -Component endpoint
```

## Build from Source

```bash
git clone git@github.com:irlm/networker-tester.git
cd networker-tester

# Rust probe engine + endpoint
cargo build --release -p networker-tester -p networker-endpoint

# C# control plane + agent (requires .NET 10 SDK)
dotnet build Networker.sln -c Release
```

Binaries are written to:
- `target/release/networker-tester`
- `target/release/networker-endpoint`
- `src/Networker.ControlPlane/bin/Release/net10.0/Networker.ControlPlane`
- `src/Networker.Agent/bin/Release/net10.0/Networker.Agent`

## Local Quick Start

### 1. Start the endpoint

```bash
./target/release/networker-endpoint
```

Default ports:
- HTTP: `8080`
- HTTPS: `8443`
- UDP echo: `9999`
- UDP throughput: `9998`

You can also start it from a config file:

```bash
./target/release/networker-endpoint --config examples/configs/endpoint.example.json
```

### 2. Run the tester

```bash
./target/release/networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,http3,udp,download,pageload,pageload2,pageload3 \
  --payload-sizes 1m \
  --runs 3 \
  --insecure
```

Or use a config file:

```bash
./target/release/networker-tester --config examples/configs/tester.example.json
```

By default, output goes to `output/`.

### 3. Open the report

```bash
open output/report.html
```

Linux:

```bash
xdg-open output/report.html
```

Windows:

```powershell
Invoke-Item output\report.html
```

## Component-Specific Notes

### `networker-tester`

Useful entrypoints:
- `--target ...`: repeat for multi-target comparisons
- `--modes ...`: select probe families
- `--config ...`: load a JSON config and override individual values with CLI flags
- `--url-test-url ...`: run the higher-level website diagnostic flow

For mode details, read [`probes.md`](probes.md).

### `networker-endpoint`

Useful entrypoints:
- `--config ...`: read endpoint ports and log level from JSON
- `generate-site`: create static assets for nginx/IIS stack comparisons

Example:

```bash
./target/release/networker-endpoint generate-site ./site --preset mixed --stack nginx
```

### `Networker.ControlPlane` (C#)

The control plane runs DB migrations on startup, may seed the first admin
user, and expects environment-based configuration. Typical local flow:

```bash
# Start PostgreSQL (use the dashboard compose file, not docker-compose.db.yml which is for MSSQL tests)
docker compose -f docker-compose.dashboard.yml up -d postgres

DASHBOARD_JWT_SECRET=$(openssl rand -base64 32) \
DASHBOARD_CREDENTIAL_KEY=$(openssl rand -hex 32) \
ASPNETCORE_URLS=http://0.0.0.0:5030 \
  dotnet run --project src/Networker.ControlPlane
```

Required environment variables (fail-closed outside Development):
- `DASHBOARD_JWT_SECRET`: HS256 signing key for JWT tokens (generate with `openssl rand -base64 32`)
- `DASHBOARD_CREDENTIAL_KEY`: 64-hex AEAD key for cloud-account secrets (generate with `openssl rand -hex 32`)

Optional:
- `DASHBOARD_DB_URL_NPGSQL`: Npgsql connection string (`Host=…;Database=…;Username=…;Password=…`; defaults to localhost dev values)
- `ASPNETCORE_URLS`: listen address (defaults to `http://localhost:5000`; prod uses `:5030`)
- `DASHBOARD_BACKGROUND_SERVICES`: set `0` for an API-only replica (no scheduler/watchdog/reaper loops)
- `DASHBOARD_PUBLIC_URL`: public URL used in SSO callbacks and agent bootstrap

### `Networker.Agent` (C#)

The agent connects back to the control plane over WebSocket and runs tester
jobs on that machine.

```bash
AGENT_API_KEY=dev-key AGENT_DASHBOARD_URL=ws://localhost:5030/ws/agent \
  dotnet run --project src/Networker.Agent
```

Required environment variables:
- `AGENT_API_KEY`: authentication key matching an agent record in the control-plane database (also accepted: `AGENT_APIKEY`)

Optional:
- `AGENT_DASHBOARD_URL`: full agent WebSocket URL (defaults to `ws://localhost:3000/ws/agent`; also accepted: `AGENT_DASHBOARDURL`)

### Frontend (`dashboard/`)

```bash
cd dashboard && npm install && npm run dev
```

The Vite dev server on port `5173` proxies `/api` and `/ws` to the control
plane. In production, nginx serves the built SPA from disk and proxies
`/api` + `/ws` to the control plane on port `5030`.

## Config Files

Checked-in sample JSON files are in [`examples/configs/`](../examples/configs/).
Use [`config-examples.md`](config-examples.md)
to choose the right starting point.

## Next Reading

- [`probes.md`](probes.md)
- [`testing.md`](testing.md)
- [`deploy-config.md`](deploy-config.md)
- [`release-flow.md`](release-flow.md)
