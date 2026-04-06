# Installation and Startup

This guide covers the supported install paths, local development builds, and the fastest way to
start each major component in this repository.

## Components

- `networker-tester`: CLI probe runner
- `networker-endpoint`: HTTP/HTTPS/UDP target server
- `networker-dashboard`: control plane API + static frontend hosting
- `networker-agent`: dashboard-connected worker that runs tester jobs

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
cargo build --release
```

Binaries are written to:
- `target/release/networker-tester`
- `target/release/networker-endpoint`
- `target/release/networker-dashboard`
- `target/release/networker-agent`

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

### `networker-dashboard`

The dashboard server expects environment-based configuration for database, JWT, and frontend
serving. It also runs DB migrations and may seed the first admin user.

Typical local flow:

```bash
# Start PostgreSQL (use the dashboard compose file, not docker-compose.db.yml which is for MSSQL tests)
docker compose -f docker-compose.dashboard.yml up -d postgres
cd dashboard && npm install && npm run build && cd ..
DASHBOARD_JWT_SECRET=$(openssl rand -base64 32) \
DASHBOARD_ADMIN_PASSWORD=admin \
  cargo run -p networker-dashboard
```

Required environment variables:
- `DASHBOARD_JWT_SECRET`: signing key for JWT tokens (generate with `openssl rand -base64 32`)
- `DASHBOARD_ADMIN_PASSWORD`: initial admin password (prompted interactively if unset)

Optional:
- `DASHBOARD_DB_URL`: PostgreSQL connection string (defaults to `postgres://networker:networker@localhost:5432/networker_dashboard`)
- `DASHBOARD_PORT`: API listen port (defaults to 3000)
- `DASHBOARD_ADMIN_EMAIL`: admin user email address

### `networker-agent`

The agent connects back to the dashboard over WebSocket and runs tester jobs on that machine.

```bash
AGENT_API_KEY=dev-key cargo run -p networker-agent
```

Required environment variables:
- `AGENT_API_KEY`: authentication key matching an agent record in the dashboard database

Optional:
- `AGENT_DASHBOARD_URL`: WebSocket URL (defaults to `ws://localhost:3000/ws/agent`)

## Config Files

Checked-in sample JSON files are in [`examples/configs/`](../examples/configs/).
Use [`config-examples.md`](config-examples.md)
to choose the right starting point.

## Next Reading

- [`probes.md`](probes.md)
- [`testing.md`](testing.md)
- [`deploy-config.md`](deploy-config.md)
