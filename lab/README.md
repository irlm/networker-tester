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

Every assertion failure prints the run id, per-mode ok/total and the first
distinct error messages; `lab.sh logs runner-N` / `logs controlplane` have the
rest.

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

Not faithful (documented gaps):

* **No systemd** in containers — `lab/images/target/systemctl` is a shim that
  runs unit files' `ExecStart` directly. Unit files themselves are validated
  (they must resolve), but `Restart=`/`After=` semantics are not.
* **No cloud provisioning path** — `az`/`aws`/`gcloud`, cloud-init, the deploy
  runner's SSH install. Those stay covered by the nightly canary. (Roadmap:
  sshd on targets so `install.sh --deploy` with `provider: lan` can be pointed
  at containers.)
* **No Windows targets/runners** (IIS, install.ps1).
* The bare `rust` target is only reachable as a `network` kind (there is no
  "direct endpoint" proxy stack in the product), so endpoint-only modes run
  through the proxy targets.

## Files

```
lab/
  lab.sh                  CLI (build/up/validate/status/logs/shell/psql/tester/down)
  validate.sh             the assertion matrix (also runnable directly)
  docker-compose.yml      postgres + controlplane (+ ui profile); topology.yml is generated
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
