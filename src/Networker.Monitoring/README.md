# Networker.Monitoring

Independent LagHound API-monitoring service. It deliberately has no reference
to `Networker.ControlPlane` and uses a separate PostgreSQL database through
`Networker.Monitoring.Data`.

This foundation provides:

- `GET|HEAD /health` without authentication;
- API-key-protected monitor, location, and check-history routes under
  `/api/v1`;
- one active monitoring location per monitor, represented through an
  assignment table that can expand to multiple locations later;
- database-leased scheduling that is safe to run with multiple service
  replicas;
- HTTP status and total-latency assertions; and
- explicit `healthy`, `warning`, `critical`, and `unknown` outcomes.

## Configuration

| Setting | Required | Purpose |
|---|---:|---|
| `ConnectionStrings__Monitoring` or `MONITORING_DB_URL_NPGSQL` | Production | Connection to the monitoring-owned PostgreSQL database |
| `MONITORING_API_KEY` | Outside Development | Independent service-to-service management key |
| `MONITORING_RUN_MIGRATIONS=0` | No | Disable startup migrations for externally managed/test hosts |
| `MONITORING_BACKGROUND_SERVICES=1` | To execute checks | Explicitly enable the scheduler and runner |

The scheduler is disabled by default. Do not enable it for untrusted target
configuration until the target-address, redirect, and DNS-rebinding controls
from the hardening phase are present. The foundation client does not follow
redirects.

The API key is a bootstrap boundary for the service foundation. The integrated
dashboard and standalone outage console will use independently validated signed
identity; browser clients must never receive this service key.

## Local run

Create a separate database, set the connection string and API key, then run:

```bash
dotnet run --project src/Networker.Monitoring/Networker.Monitoring.csproj
```

To exercise scheduling against targets you control, also set
`MONITORING_BACKGROUND_SERVICES=1`.

## Container

Publish beside the project Dockerfile before building its runtime image:

```bash
dotnet publish src/Networker.Monitoring/Networker.Monitoring.csproj \
  --configuration Release \
  --output src/Networker.Monitoring/publish
docker build -t laghound-monitoring src/Networker.Monitoring
```

Deploy the resulting container and its PostgreSQL database in a failure domain
separate from the main control plane.
