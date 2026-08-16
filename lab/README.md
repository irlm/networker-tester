# Local lab — the managed path in Docker, before any cloud VM

`lab/` spins up the **whole managed stack on your laptop** — control plane +
PostgreSQL + *N* runner containers (what tester VMs run) + *M* target
containers (what endpoint VMs run, optionally behind the real proxy stacks) —
and then drives real runs through the public API and asserts the outcome.
It is the local twin of the nightly prod canary (`scripts/soak-canary.sh`),
built entirely from **this checkout** (no GitHub releases involved), so what
you validate is the code you are about to ship, running on Linux in a private
network exactly like the cloud VMs will run it.

```
┌──────────────── docker network 172.31.100.0/24 (nwk-lab_labnet) ────────────────┐
│  postgres .5      controlplane .10 (:5030 → host)      [ui .11 (:8088 → host)]   │
│                                                                                  │
│  target-1 .101  rust      networker-endpoint 8080/8443 + UDP 9997-9999            │
│  target-2 .102  nginx     endpoint + nginx 8081/8444 (install.sh --setup-stack)  │
│  target-3 .103  caddy     endpoint + caddy 8091/8454   … apache/haproxy/traefik  │
│                                                                                  │
│  runner-1 .201  Networker.Agent + networker-tester  (AGENT_API_KEY, WS to CP)     │
│  runner-2 .202  …                                                                 │
└──────────────────────────────────────────────────────────────────────────────────┘
```

Works on macOS (Apple Silicon or Intel) and Linux. Needs **Docker (compose
v2), curl, jq** — nothing else; Rust, .NET and Node build inside Docker for
the container's native arch. `lab.sh` is bash-3.2-clean.

## Quick start

```bash
./lab/lab.sh build                              # images: rustbin, controlplane, runner, target-rust, target-nginx
./lab/lab.sh up --runners 3 --targets rust,nginx,caddy
./lab/lab.sh validate                           # end-to-end matrix; non-zero exit on any regression
./lab/lab.sh status                             # runners/targets/agents at a glance
./lab/lab.sh down [--volumes]                   # stop (wipe DB + state with --volumes)
```

First `build` takes a while (Rust release build + .NET publishes + one
`install.sh --setup-stack` per proxy); rebuilds are incremental (BuildKit cache
mounts). `up` builds anything missing; after code changes run `build` again
(or `docker build` just the image you touched, see the Dockerfiles).

### More knobs

| Command / flag | What it does |
|---|---|
| `up --targets rust,nginx,caddy,apache,haproxy,traefik` | one target container per entry; `rust` = bare endpoint, others = endpoint + that proxy set up **by the real `install.sh --setup-stack`** at image build |
| `up --netem "delay 40ms 5ms loss 0.1%"` | WAN emulation on every runner (`tc netem`, NET_ADMIN is granted) |
| `up --ui` | build + serve the React SPA at http://127.0.0.1:8088 (nginx proxies /api + /ws like prod) — login `admin@lab.local` / `LabAdmin-Pass1!` |
| `up --agents-via-ui` | runners connect through the nginx WS proxy (`ws://ui/ws/agent`) instead of the control plane directly — exercises the proxied WebSocket path |
| `validate --modes tcp,http1 --runs 5 --only 2 / --skip 3,4` | shrink or select phases |
| `tester runner-1 -- --target https://172.31.100.102:8444/health --modes http3 --insecure` | run `networker-tester` directly inside a runner (bypasses the control plane) |
| `logs [svc] -f`, `shell <svc>`, `psql`, `env` | the usual |
| `LAB_CP_PORT / LAB_UI_PORT / LAB_PG_PORT / LAB_NET_PREFIX` | host ports and the /24 (defaults 5030 / 8088 / 55432 / 172.31.100) |

## What `validate` asserts

| Phase | Drives | Catches |
|---|---|---|
| 1 network probe | `kind=network` config → target-1 rust endpoint :8443 (insecure); tcp,dns,tls,tlsresume,http1,http2,http3,curl,ping | run reaches `completed`, attempts persisted, every mode ≥1 success (P0-1/P0-2 class) |
| 2 proxy matrix | for **every** proxy target: `kind=proxy` config (deployment id + stack) → the dispatcher resolves ip:stack-port + injects `insecure`; the deterministic HTTP/TCP/UDP matrix incl. download/upload/pageload*/websocket/udp/stamp, plus `native` | a stack that stops forwarding a route (v0.28.112 class), a mode broken through a proxy (v0.28.118 class), `native` not dropped by dispatch, 404s; HTTP/3 modes are expected only on stacks `shared/http-stacks.json` marks `h3: true` |
| 3 fan-out | 2×runners launches at once | all complete **and** dispatch spreads across ≥2 workers |
| 4 cancel | long run → cancel | terminal `cancelled`, not `completed`/stuck |
| 5 provider | the **Docker (local) cloud provider** through the public API: `POST /testers {cloud:"docker"}` → the control plane `docker run`s a runner and its agent comes online (`running`/`idle`); `POST /deployments` with one `provider:"docker"` endpoint behind nginx → `completed` with an `endpoint_ip`; a proxy-kind config against that deployment pinned to that tester (`LaunchRequest.tester_id`) → run completes with successes on that agent; `DELETE` both → the containers are gone (`docker ps -a --filter label=networker.role`) | the create-tester → provision → agent-online path, the deploy runner, the tester/deployment delete teardown, the proxy resolver — with zero VM cost. Skipped with a note when `GET /api/version` says `docker_provider=false` |

Every assertion failure prints the run id, per-mode ok/total and the first
distinct error messages; `lab.sh logs runner-N` / `logs controlplane` have the
rest.

### Docker (local) provider — from the UI or the API

The lab control plane runs with `DASHBOARD_DOCKER_PROVIDER=1`
(`lab/docker-compose.yml`; `GET /api/version` → `docker_provider: true`), so
with `up --ui` the Create Runner modal and both deploy wizards offer
**"Docker (local)"** next to the cloud accounts (no account, region `local`).
Same thing over the API:

```bash
. lab/.state/lab.env
curl -s -X POST -H "Authorization: Bearer $LAB_TOKEN" -H 'Content-Type: application/json'   "$LAB_BASE_URL/api/projects/$LAB_PROJECT_ID/testers" -d '{"name":"my-runner","cloud":"docker","region":"local"}'
curl -s -X POST -H "Authorization: Bearer $LAB_TOKEN" -H 'Content-Type: application/json'   "$LAB_BASE_URL/api/projects/$LAB_PROJECT_ID/deployments"   -d '{"name":"caddy target","config":{"version":1,"tester":{"provider":"local"},"endpoints":[{"provider":"docker","http_stacks":["caddy"]}],"tests":{"run_tests":false}}}'
docker ps --filter label=networker.role        # nwk-<project>-tester-local-… / nwk-<project>-ep-caddy-…
```

Images: `DASHBOARD_DOCKER_RUNNER_IMAGE` (default `nwk-lab/runner:local`) and
`DASHBOARD_DOCKER_TARGET_IMAGE_PREFIX` + stack + `:` +
`DASHBOARD_DOCKER_TARGET_IMAGE_TAG` (`nwk-lab/target-caddy:local`) — build the
stacks you deploy first (`lab.sh build --stacks …`). Other knobs:
`DASHBOARD_DOCKER_NETWORK`, `DASHBOARD_DOCKER_BIN`, `DASHBOARD_DOCKER_AGENT_URL`
(control plane on the host: `ws://host.docker.internal:5030/ws/agent`).

## What is (and isn't) faithful

Faithful — same code paths as production:

* **Control plane**: `dotnet publish -c Release` of `Networker.ControlPlane`,
  migrations + bootstrap admin on startup, `Production` environment with the
  fail-closed secrets set, background services on.
* **Runners**: Ubuntu 24.04 + the self-contained `Networker.Agent` publish +
  the release-feature `networker-tester` at `/usr/local/bin`, driven by the
  same env contract the cloud-init systemd unit uses (`AGENT_DASHBOARD_URL`,
  `AGENT_API_KEY`); tshark + `ping_group_range` sysctl as on a provisioned VM.
* **Targets**: `networker-endpoint` on its real ports; the proxy stacks are
  installed and configured by **the unmodified `install.sh --setup-stack`**, so
  the nginx/caddy/apache/haproxy/traefik configs, certs, static site and port
  layout are byte-identical to what a cloud endpoint VM gets — including
  install.sh's own post-start health checks.
* **Registration**: runners are `agent` rows (standalone, no `project_tester`)
  with only the SHA-256 of the key stored — the same INSERT the create-tester
  path performs minus the VM; proxy targets are `completed` deployments with
  `endpoint_ips` + `http_stacks`, exactly what the proxy-kind resolver reads.
* **Cloud provisioning path — via the Docker (local) provider**: the control
  plane runs with `DASHBOARD_DOCKER_PROVIDER=1` and the host's docker socket
  mounted, so a runner created with cloud `docker` (UI: "Docker (local)" in
  Create Runner / the deploy wizards; API: `POST /testers {cloud:"docker"}`)
  goes through the REAL create-tester flow (mint key → `IComputeProvisioner
  .CreateVmAsync` → persist ip/resource id → wait for the agent → running),
  and a deployment with `provider:"docker"` endpoints goes through the REAL
  deploy runner (status/log/`endpoint_ips`/`DeployComplete`), start/stop/
  delete/probe through the same lifecycle handlers, the deployment delete
  through the same by-endpoint teardown, and orphaned containers through the
  same reaper tick. Only the last hop differs: `docker run` of the lab images
  instead of `az vm create` + cloud-init / `install.sh --deploy`. Phase 5
  validates it.

Not faithful (documented gaps):

* **No systemd** in containers — `lab/images/target/systemctl` is a shim that
  runs unit files' `ExecStart` directly. Unit files themselves are validated
  (they must resolve), but `Restart=`/`After=` semantics are not.
* **The cloud CLIs themselves** — `az`/`aws`/`gcloud`, cloud-init rendering,
  the deploy runner's `install.sh --deploy` SSH install. Everything above them
  is exercised through the Docker (local) provider; the CLI hop stays covered
  by the nightly canary. (Roadmap: sshd on targets so `install.sh --deploy`
  with `provider: lan` can be pointed at containers.)
* **Docker provider limits**: one `http_stack` per docker endpoint (one
  container = one ip = one stack, which is how the proxy resolver reads
  `endpoint_ips[0]`); no reference-API `languages` on docker targets (the
  target images ship the endpoint + proxy stacks only); Linux only.
* **No Windows targets/runners in Docker** — that is what the [native Windows lab](#native-windows-lab)
  below is for (IIS via the real cloud payload, Windows runner).
* The bare `rust` target is only reachable as a `network` kind (there is no
  "direct endpoint" proxy stack in the product), so endpoint-only modes run
  through the proxy targets.

## Native Windows lab

`lab/native/lab-native.ps1` is the Docker lab's **native-Windows twin**: the
whole managed path as plain Windows **processes** on one machine (a
`windows-latest` GitHub runner every Monday, or your own Windows box), built
from this checkout, then the **same `validate.sh` matrix** — so we confirm on
every run that a **Windows target** offers and passes the same probes as the
Linux targets and that a **Windows runner** executes them.

```
PostgreSQL       existing server (LAB_PG_*) | docker-compose.dashboard.yml postgres | CI: action-setup-postgres
control plane    dotnet Networker.ControlPlane.dll (framework-dependent publish)   http://127.0.0.1:5030
target-1  rust   C:\networker\networker-endpoint.exe   8080/8443 + UDP 9997-9999 (built here)
target-2  iis    IIS "networker-iis" 8082/8445 -> ARR -> endpoint  (install.sh _iis_setup_powershell payload)
runner-1         networker-agent.exe (self-contained win-x64 publish) + networker-tester.exe (Windows CI feature set)
```

On a Windows dev box (elevated PowerShell — IIS, `C:\networker`, firewall rules):

```powershell
powershell -ExecutionPolicy Bypass -File scripts/dev-setup.ps1     # rustup + MSVC + CMake + .NET 10 + Git (Bash) + jq + Docker
.\lab\native\lab-native.ps1 build        # cargo build --release (tester: --no-default-features --features http3,db-mssql) + dotnet publish
.\lab\native\lab-native.ps1 up           # postgres -> control plane -> endpoint -> IIS -> register runner/target (psql) -> agent online
.\lab\native\lab-native.ps1 validate     # lab/validate.sh through Git Bash (add --only 1 / --modes tcp,http1 / --runs N)
.\lab\native\lab-native.ps1 status | logs | env | down
```

Knobs: `-NoIis` (rust target only), `-Fqdn <hostname>` (bind IIS's SNI
listener to a name and register the targets under it — the only way http.sys
will speak HTTP/3), `-IisSetup installer` (use `install.ps1 -Setup iis`
instead of the cloud payload — see below), `-DebugBuild`, `-DryRun` (prints
every action; also runs under pwsh on Linux/macOS to exercise the
orchestration). Env: `LAB_PG_MODE=psql|docker|auto`, `LAB_PG_HOST/PORT/USER/
PASSWORD/DB` (defaults `127.0.0.1` / `.dev.env` `DEV_PG_PORT` or 5432 /
`networker` / `networker` / `networker_core`), `LAB_CP_PORT`, `LAB_TARGET_HOST`,
`LAB_BASH` (path to Git Bash's `bash.exe` if not auto-found), and the same
`LAB_ADMIN_*` / `LAB_JWT_SECRET` / `LAB_CREDENTIAL_KEY` as `lab.sh`. State
(keys, token, pids, logs, built binaries, the rendered IIS payload) lives in
`lab/native/.state/` (git-ignored). `down` stops the processes and keeps the
DB rows and the IIS site.

`validate.sh` gained three additive env overrides for this (Linux behaviour
unchanged): `LAB_STATE_ENV` (which `lab.env` to source), `LAB_TARGET_HOSTS`
(comma list, replaces the `NET_PREFIX+index` addressing) and
`LAB_H3_OFF_STACKS` (stacks whose h3 modes must be excluded *on this host*).

CI: `.github/workflows/lab-windows-native.yml` (weekly Mondays 07:17 UTC +
`workflow_dispatch` with `no_iis` / `fqdn` / `validate_args`; not a required
check) runs `build → up → validate → down` on `windows-latest` with
sccache + rust-cache (`windows-lab-native`), uploads control plane / agent /
endpoint / IIS-payload / validate logs as `lab-native-windows-logs`, and puts
the validate summary block in the job summary.

What is faithful: the control plane and agent are the release publishes; the
tester is the Windows CI feature set (`http3,db-mssql`, no `browser` — no
Chrome on the runner); registration is the same SQL as `lab.sh`; the IIS
target is set up by **the payload every cloud Windows endpoint VM gets**
(`install.sh _iis_setup_powershell`, rendered through Git Bash and executed
under Windows PowerShell 5.1: IIS + URL Rewrite + ARR, HTTP 8082 + HTTPS 8445
bindings, web.config proxy rules, HTTP/3 registry keys, firewall rules).

What legitimately differs on Windows / on a hosted runner — surfaced, not hidden:

* **HTTP/3 through IIS.** http.sys only binds QUIC on **SNI hostname**
  bindings, and the `EnableHttp3` registry keys need a **reboot** (a hosted
  runner cannot). With the default IP-literal target `lab-native.ps1` excludes
  the h3 modes for the `iis` stack (`LAB_H3_OFF_STACKS=iis`) and says so; with
  `-Fqdn` it restarts http.sys (best effort) and **probes** http3 with the
  built tester — h3 modes stay in the iis matrix only when that probe
  succeeds. `http3`/`pageload3` are still exercised against the bare Windows
  endpoint in phase 1. (`shared/http-stacks.json` keeps `iis: h3=true`
  because that is what the installer configures on a rebooted Server 2022+
  VM reached by hostname — Azure deployments record the FQDN in
  `endpoint_ips`; AWS/GCP Windows endpoints are reached by IP and therefore
  never get h3 through IIS in production either — worth a follow-up.)
* **`install.ps1 -Setup iis` is an HTTP-only stub** (site on 8082, no 8445 /
  ARR / H3 — it says so itself); the proxy matrix on 8445 cannot pass with it,
  which is why the lab defaults to the cloud payload. `-IisSetup installer`
  keeps that gap visible. `install.ps1 endpoint` insists on a release download
  or `cargo install` (no local-binary flag), so the lab starts the locally
  built exe directly, like the Docker lab does.
* **websocket through IIS** needed the `Web-WebSockets` feature and a `/ws`
  rewrite rule in the payload (added in the same PR — the Linux stacks already
  proxied `/ws`; the first native run would have failed on it).
* `capture` (no tshark) degrades to a warning inside the tester; `browser*`
  are not built (no `browser` feature / no Chrome); `ping` uses `IcmpSendEcho`
  and works without elevation.
* Only one runner (phase 3 fan-out completes 2 runs on 1 worker; the
  spread-across-workers assertion applies from 2 runners); phase 5 (Docker
  provider) is skipped (`docker_provider=false`).

## Files

```
lab/
  lab.sh                  CLI (build/up/validate/status/logs/shell/psql/tester/down)
  validate.sh             the assertion matrix (also runnable directly)
  docker-compose.yml      postgres + controlplane (+ ui profile; docker socket mounted for the
                          Docker (local) provider); topology.yml is generated
  images/
    rust.Dockerfile       cargo build --release --locked → nwk-lab/rustbin (tester + endpoint)
    controlplane.Dockerfile
    runner.Dockerfile     Networker.Agent self-contained publish + tester on ubuntu:24.04
    target.Dockerfile     endpoint (+ install.sh --setup-stack $STACK at build) on ubuntu:24.04
    target/systemctl      the systemd shim
    target/entrypoint.sh  starts endpoint, then the stack via the shim; keeps both alive
    runner/entrypoint.sh  optional netem, then exec networker-agent
    ui.Dockerfile, ui/nginx.conf
  .generated/topology.yml, .state/   (git-ignored) generated services, keys, token, project id
  native/
    lab-native.ps1        native Windows twin (build/up/validate/status/logs/env/down) — drives validate.sh via Git Bash
    .state/               (git-ignored) bin/, publish/, logs/, lab.env, runner-1.key, iis-setup.ps1, pids.json
```

## Bugs the first lab run found (all fixed in the same PR)

* A **fresh control-plane database persisted zero attempts, forever** — the
  tester's V001 probe schema (`RequestAttempt`, …) only ever existed where a
  DB-backed tester or install.sh's psql seed had created it; the ingest
  swallowed 42P01. The control plane now bootstraps it lazily from
  `shared/tester-schema.postgres.sql` (guarded against `postgres.rs`) and adapts
  to the fielded schema shapes (`extrajson` vs `extra_json`, `testrun` NOT NULLs).
* Persisted failures had **no reason**: the tester emits `error:{message,detail}`,
  the extractor read a flat `error_message`.
* Dispatch sent **every run to the first online agent** while the others idled
  — now least-loaded among compatible agents (tester affinity still first).
* `dns` mode **failed against any IP-literal target** (the standalone probe
  appended the search domain to `172.31.100.101`).
* `pageload` (forced HTTP/1.1) **fetched 0/N assets through caddy/apache/haproxy/traefik**
  — the HTTPS→HTTP port rewrite knew only nginx/IIS; the layout now lives in
  `shared/http-stacks.json` for Rust, C# and the lab.
* Catalog mode `pageload` (H1) reached the tester as the **all-three shorthand**
  (`pageload,pageload2,pageload3`) — the agent now sends the explicit `pageload1`.
* First-login `change-password` was followed by up to **10 s of 403** (status cache).
* `apt-get install apache2/haproxy/caddy` in `--setup-stack` failed on hosts
  without package lists (no `apt-get update` first — nginx already had one).
