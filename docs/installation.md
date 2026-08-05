# Installation and Startup

This guide gives the supported install paths and the local build steps. It
also shows you how to start each component in this repository.

## Components

- `networker-tester` (Rust): CLI probe runner
- `networker-endpoint` (Rust): HTTP/HTTPS/UDP target server
- `Networker.ControlPlane` (C#): control plane API (`/api` + `/ws`) — prod runs this
- `Networker.Agent` (C#): control-plane-connected worker that runs tester jobs
- `dashboard/` (React): browser SPA — served static by nginx in prod, Vite dev server locally

The retired Rust control plane (`networker-dashboard`, `networker-agent`) is
off the release train. See
[`architecture.md`](architecture.md#retired-components-rust-control-plane).

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

### Self-hosting the control plane

`install.sh dashboard` installs a complete single-VM deployment: PostgreSQL,
the C# control plane, a local agent, the prebuilt frontend, and an nginx
reverse proxy.

```bash
DASHBOARD_ADMIN_PASSWORD='choose-a-strong-one' \
  curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh \
  | bash -s -- dashboard
```

Everything is fetched as prebuilt release assets — `networker-controlplane`
is a self-contained .NET publish directory, so the host needs no .NET runtime,
and the frontend ships prebuilt, so it needs no Node.js either. Requires
**v0.28.156 or newer**: earlier releases still asked for the Rust
`networker-dashboard` assets, which stopped being built at v0.28.148.

**`DASHBOARD_ADMIN_PASSWORD` is how you get in.** The control plane has no
signup page. On first start, if — and only if — the `dash_user` table is
completely empty, it seeds one platform admin (`DASHBOARD_ADMIN_EMAIL`,
default `admin@localhost`) with that password and requires a change at first
login. If you omit the variable the installer generates a random one and
prints it at the end. On an existing deployment this is a permanent no-op: it
will never modify, reset, or overwrite an account that already exists.

Useful knobs: `DASHBOARD_FQDN` (enables Let's Encrypt and sets
`DASHBOARD_PUBLIC_URL`), `DASHBOARD_DB_PASSWORD`, `CONTROLPLANE_PORT`
(default 5030), `NETWORKER_VERSION` to pin a release.

Afterwards:

```bash
systemctl status networker-dashboard          # the control plane
journalctl -u networker-dashboard -f          # logs
sudo cat /etc/networker-dashboard.env         # config + secrets (mode 600)
```

`DASHBOARD_JWT_SECRET` and `DASHBOARD_CREDENTIAL_KEY` in that file are
generated once at install. Back them up: the app fail-closes without them, and
losing `DASHBOARD_CREDENTIAL_KEY` permanently orphans every stored cloud
credential.

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

The control plane runs the DB migrations at startup. It can also seed the first
admin user. It reads its configuration from the environment. Use this local
flow:

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

The agent connects to the control plane over a WebSocket. It runs the tester
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

The Vite dev server on port `5173` sends `/api` and `/ws` to the control plane
through a proxy. In production, nginx serves the built SPA from disk. nginx also
sends `/api` and `/ws` to the control plane on port `5030`.

## Config Files

The repository keeps sample JSON files in
[`examples/configs/`](../examples/configs/). Use
[`config-examples.md`](config-examples.md) to select the correct starting point.

## Next Reading

- [`probes.md`](probes.md)
- [`testing.md`](testing.md)
- [`deploy-config.md`](deploy-config.md)
- [`release-flow.md`](release-flow.md)
