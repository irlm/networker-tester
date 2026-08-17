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
│  target-4 .104  windows   Windows Server VM (KVM): install.ps1 endpoint 8080/8443 │
│      = target-4.lab       + IIS 8082/8445 (h1/h2/h3 — SNI binding target-4.lab) [KVM] │
│                                                                                  │
│  runner-1 .201  Networker.Agent + networker-tester  (AGENT_API_KEY, WS to CP)     │
│  runner-2 .202  …                                                                 │
│  runner-3 .203  windows   Windows Server VM (KVM): install.ps1 tester (release exe)│
│                           + the checkout's agent as a SYSTEM ONSTART task        │
└──────────────────────────────────────────────────────────────────────────────────┘
```

Works on macOS (Apple Silicon or Intel) and Linux. Needs **Docker (compose
v2), curl, jq** — nothing else; Rust, .NET and Node build inside Docker for
the container's native arch. `lab.sh` is bash-3.2-clean. The optional
[Windows (IIS) target](#windows-iis-target) and [Windows runners](#windows-runners)
additionally need Linux + KVM. Several labs can run side by side
([`LAB_INSTANCE`](#multiple-instances-lab_instance)).

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
| `up --targets rust,nginx,windows` (`iis` = alias) | + a **Windows Server VM** running the real `install.ps1` (endpoint + IIS with an SNI hostname binding → h1/h2/h3 through IIS) — see [Windows (IIS) target](#windows-iis-target); Linux + KVM only, first boot 15-40 min (14 min measured: 6 GB ISO at ~35 MB/s + install + install.ps1 + reboot); `--windows-async` returns immediately, `wait-windows` / `windows-log [-f]` / `windows-ssh` afterwards |
| `up --runners 1 --windows-runners 1` | + one **Windows Server VM runner** (`runner-2`): the checkout's `install.ps1 -Component tester` (released `networker-tester.exe`) + the checkout's `Networker.Agent` (win-x64 publish) as a SYSTEM ONSTART task, registered + bound to a `project_tester` row so runs can be pinned to it — see [Windows runners](#windows-runners); Linux + KVM only, first boot ~15 min, reboot ~1 min |
| `LAB_INSTANCE=2 ./lab/lab.sh up …` | a second, fully independent lab (project `nwk-lab2`, `172.31.101.0/24`, ports 5031/8089/55433, `lab/.state-2`) — see [Multiple instances](#multiple-instances-lab_instance) |
| `up --netem "delay 40ms 5ms loss 0.1%"` | WAN emulation on every runner (`tc netem`, NET_ADMIN is granted) |
| `up --ui` | build + serve the React SPA at http://127.0.0.1:8088 (nginx proxies /api + /ws like prod) — login `admin@lab.local` / `LabAdmin-Pass1!` |
| `up --agents-via-ui` | runners connect through the nginx WS proxy (`ws://ui/ws/agent`) instead of the control plane directly — exercises the proxied WebSocket path |
| `validate --modes tcp,http1 --runs 5 --only 2 / --skip 3,4` | shrink or select phases |
| `tester runner-1 -- --target https://172.31.100.102:8444/health --modes http3 --insecure` | run `networker-tester` directly inside a runner (bypasses the control plane) |
| `logs [svc] -f`, `shell <svc>`, `psql`, `env`, `compose <args>` | the usual (`compose` = raw `docker compose` with this instance's project, files and env) |
| `LAB_IMAGE_TAG=mybranch` | tag for the `nwk-lab/*` images (default `local`, shared by every instance). Set one per checkout when two labs build from different trees at once — a concurrent `lab.sh build` elsewhere re-tags `:local` |
| `LAB_CP_PORT / LAB_UI_PORT / LAB_PG_PORT / LAB_NET_PREFIX` | host ports and the /24 (defaults 5030 / 8088 / 55432 / 172.31.100 for instance 1; shifted per `LAB_INSTANCE`) |

## What `validate` asserts

| Phase | Drives | Catches |
|---|---|---|
| 1 network probe | `kind=network` config → target-1 rust endpoint :8443 (insecure); tcp,dns,tls,tlsresume,http1,http2,http3,curl,ping | run reaches `completed`, attempts persisted, every mode ≥1 success (P0-1/P0-2 class) |
| 2 proxy matrix | for **every** proxy target: `kind=proxy` config (deployment id + stack) → the dispatcher resolves ip:stack-port + injects `insecure`; the deterministic HTTP/TCP/UDP matrix incl. download/upload/pageload*/websocket/udp/stamp, plus `native`. A `windows` target is validated as stack **`iis`** (:8445, dispatched **by hostname** `target-N.lab` — the deployment's `endpoint_hosts[0]` — so the QUIC handshake carries SNI and `http3`/`pageload3` are in the matrix) **and** as a bare Windows endpoint on :8443 (`kind=network`, the phase-1 modes incl. `http3`) | a stack that stops forwarding a route (v0.28.112 class), a mode broken through a proxy (v0.28.118 class), `native` not dropped by dispatch, 404s; HTTP/3 modes are expected only on stacks `shared/http-stacks.json` marks `h3: true` — the `iis` entry was measured here (see below) |
| 3 fan-out | 2×runners launches at once | all complete **and** dispatch spreads across ≥2 workers |
| 4 cancel | long run → cancel | terminal `cancelled`, not `completed`/stuck |
| 5 provider | the **Docker (local) cloud provider** through the public API: `POST /testers {cloud:"docker"}` → the control plane `docker run`s a runner and its agent comes online (`running`/`idle`); `POST /deployments` with one `provider:"docker"` endpoint behind nginx → `completed` with an `endpoint_ip`; a proxy-kind config against that deployment pinned to that tester (`LaunchRequest.tester_id`) → run completes with successes on that agent; `DELETE` both → the containers are gone (`docker ps -a --filter label=networker.role`) | the create-tester → provision → agent-online path, the deploy runner, the tester/deployment delete teardown, the proxy resolver — with zero VM cost. Skipped with a note when `GET /api/version` says `docker_provider=false` |
| 6 windows runner | for every online agent with `os=windows` ([Windows runners](#windows-runners)): the heartbeat reports `os=windows` + `capabilities {chrome:false,tshark:false}`; the phase-1 network modes and the phase-2 proxy matrix through the first Linux proxy target (nginx) launched **pinned to it** (`tester_id` of the bound `project_tester`) execute on that agent (`worker_id`) with every mode ≥1 success, `native` dropped; modes whose verdict differs from the Linux runner's phase-1/2 runs on the same target are listed | "a Windows runner runs the same tests as a Linux runner": the Windows tester build (ping via `IcmpSendEcho`, http3, websocket, udp/stamp through nginx), the C# agent on Windows (process spawning, path handling, ONSTART persistence), tester pinning. Skipped with a note without a Windows runner |

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

## Windows (IIS) target

`--targets …,windows` (alias `iis`) adds one **real Windows Server VM** to the
lab so the tester runs the same matrix against a Windows endpoint as against
the Linux stacks — proving that `install.ps1` still installs, that the Windows
build of `networker-endpoint` offers the same tests (incl. HTTP/3 on :8443),
and what IIS as set up by the installer really serves on :8445. The first run
(v0.28.208) settled the `iis` row of `shared/http-stacks.json`, which gates
the UI/API mode pickers: **HTTP/1.1 and HTTP/2 yes** (after fixing the
tester's h2 requests, which http.sys rejected for missing `:scheme`/`:authority`),
**websocket yes** (after adding the `/ws` ARR rule), and — since v0.28.211 —
**HTTP/3 yes**: http.sys answers QUIC only to clients sending TLS SNI, so the
platform now addresses the target **by hostname**. The lab gives every target
a labnet DNS alias `target-N.lab` (docker's embedded DNS answers it for the
runners and the control plane), the Windows VM's IIS is set up with
`install.ps1 -Setup iis -Fqdn target-N.lab` (SNI binding + certificate SAN),
and `lab.sh up` registers the deployment with `endpoint_hosts:["target-N.lab"]`
(V050) — exactly what a cloud Windows VM gets from its Azure DNS label / AWS
public DNS. The proxy resolver hands the tester `https://target-N.lab:8445`,
the QUIC ClientHello carries SNI, and validate phase 2 measures
`http3 2/2 · pageload3 2/2` through IIS. A VM disk installed by an older
`lab.sh` (IIS bound without a hostname) is upgraded in place: `up` checks the
certificate served for SNI `target-N.lab` and, if the name is missing, re-runs
the installer's IIS setup inside the VM over SSH (`windows-iis-refresh`, ~2-4
min, no reinstall).

Docker on Linux cannot run Windows containers, so the target is a VM inside a
container: [dockur/windows](https://github.com/dockur/windows) (QEMU + KVM),
on `labnet` with the usual `.10N` IP — dockur DNATs every TCP/UDP port of the
container to the guest, so runners reach `172.31.100.10N:8080/8443/8445` and
the UDP ports exactly like any other target.

**Requirements / caveats**

* **Linux with `/dev/kvm`** (Docker Desktop on macOS/Windows does not expose
  KVM). Elsewhere `lab.sh up` prints a message and drops the windows target;
  the rest of the lab keeps working.
* ~**15 GB** disk (6 GB Windows Server 2022 eval ISO + the VM disk, `LAB_WINDOWS_DISK`
  40G sparse), 6 GB RAM / 4 vCPUs by default (`LAB_WINDOWS_RAM`, `LAB_WINDOWS_CPUS`).
* **First boot 15-40 min** (14 min measured here; the disk boots in ~30 s afterwards): ISO download from Microsoft, unattended install,
  then `install.ps1` (endpoint download + IIS/ARR/URL-Rewrite MSIs from GitHub
  / microsoft.com — the VM has NAT internet), one reboot for http.sys HTTP/3.
  The disk is a **named volume** (`nwk-lab_windows-storage-N`), so a plain
  `down` + `up` boots the installed VM in ~1-2 min; `down --volumes` wipes it.
* The VM runs the **Windows Server 2022 evaluation** (`LAB_WINDOWS_VERSION`;
  `2025` also works) — Microsoft's evaluation licence terms apply (180 days,
  non-production).
* Windows binaries are **not** built from the checkout: `install.ps1`
  downloads the **released** `networker-endpoint.exe` (Windows can't be
  cross-built on the Linux host). What IS the checkout's is `install.ps1`
  itself (copied in through `/oem`) — its endpoint install path (release
  download without `gh`, VC++ runtime) and its `-Setup iis` are the code under test.
* Runners are Linux containers unless you add [Windows runners](#windows-runners)
  (`--windows-runners M`) — Windows Server VMs on the same dockur pattern.

**What runs inside the VM** (`lab/images/windows/oem/lab-setup.ps1`, invoked
once by dockur from `C:\OEM\install.bat` after setup): fixed local admin
password (`Docker` / `LabWindows-Pass1!`, `LAB_WINDOWS_USER/PASSWORD`), OpenSSH
server (PowerShell as the SSH shell), firewall TCP/UDP 8080-8082, 8443-8445,
UDP 9997-9999; then **`install.ps1 -Yes -Component endpoint`**, the endpoint
started like the cloud Windows bootstraps (hidden process + `schtasks` ONSTART
as SYSTEM), then **`install.ps1 -Setup iis`**; reboot when the installer says
`REBOOT_NEEDED`. nginx on Windows is not something `install.ps1` supports (it
says so), so there is no nginx-on-Windows target.

**Driving it**

```bash
./lab/lab.sh up --runners 1 --targets rust,nginx,windows      # waits (≤ LAB_WINDOWS_TIMEOUT=3600 s) with progress lines
./lab/lab.sh up … --windows-async && ./lab/lab.sh wait-windows # or don't block
./lab/lab.sh windows-log target-3 -f     # the VM's setup log + status (installing|endpoint|iis|rebooting|ready|failed:*)
./lab/lab.sh windows-ssh target-3        # ssh Docker@172.31.100.103 (password printed) — PowerShell prompt
./lab/lab.sh windows-iis-refresh target-3 # re-run install.ps1 -Setup iis -Fqdn target-3.lab inside the VM (SNI/H3 binding)
open http://127.0.0.1:8006               # dockur's VM console (LAB_WINDOWS_VIEWER_PORT)
./lab/lab.sh validate                    # phase 2: matrix through iis :8445 (h1/h2/h3 by hostname) + network modes incl. http3 on :8443
```

`lab.sh up` registers the target as a completed deployment named
`lab-target-N-iis` with `http_stacks:["iis"]`, os `windows`,
`endpoint_ips:["<ip>"]` and `endpoint_hosts:["target-N.lab"]`, so a
proxy-kind config resolves to `target-N.lab:8445` exactly like a cloud Windows
endpoint VM whose deploy recorded its DNS name (`LAB_TARGET_DOMAIN` changes
the suffix).

## Windows runners

`--windows-runners M` adds M **Windows Server VM runners** — the tester side
of the Windows story: does the Windows build of `networker-tester` + the C#
agent *on Windows* execute the same probe matrix a Linux runner does, driven
by the real control plane through the real agent protocol? (The
[native Windows lab](#native-windows-lab) answers this on a Windows host; this
answers it from a Linux box, next to Linux runners, in one lab.)

```bash
./lab/lab.sh up --ui --runners 1 --windows-runners 1 --targets rust,nginx   # runner-1 linux, runner-2 windows VM
./lab/lab.sh status                       # RUNNER … OS … CAPABILITIES … tester <id>; windows VM status line
./lab/lab.sh windows-log runner-2 -f      # the VM's setup log (installing|tester|agent|ready|failed:*)
./lab/lab.sh windows-ssh runner-2 -- 'Get-Content C:\lab\agent.log -Tail 30'
./lab/lab.sh validate                     # phases 1-5 as before (fan-out spreads over both) + phase 6 pinned to the Windows runner
./lab/lab.sh down && ./lab/lab.sh up …    # the VM disk persists: reboot ~1 min, the agent (ONSTART task) reconnects by itself
```

Runners are numbered Linux first: `--runners N --windows-runners M` gives
`runner-1..N` (containers, `.201+`) and `runner-(N+1)..runner-(N+M)` (VMs,
same `.20K` IPs). Same VM plumbing as the target — `dockurr/windows`, KVM,
`/oem` first-boot hook, `/shared` status share, a persistent named volume
`nwk-lab[N]_windows-runner-storage-K`, console on
`LAB_WINDOWS_VIEWER_PORT+10+K-1` (8016 for runner-2, instance 1) — sized
`LAB_WINDOWS_RUNNER_RAM/CPUS/DISK` (4G / 2 / 32G).

**What runs inside** (`lab/images/windows/oem-runner/lab-runner-setup.ps1`,
once, from `C:\OEM\install.bat`): password + OpenSSH + firewall (SSH only —
a runner dials out); **`install.ps1 -Yes -Component tester`** (the checkout's
installer, copied in via `/oem`) → the **released** `networker-tester.exe`
(+ VC++ runtime) — copied to `C:\networker\networker-tester.exe`; the
**checkout's `Networker.Agent`** (published win-x64 self-contained by
`lab.sh` into `/oem/agent` — inside the dotnet SDK image with BuildKit
`--output`, `lab/images/agent-win.Dockerfile`; or with your host dotnet when
`LAB_WIN_AGENT_BUILD=host`) → `C:\networker\agent\`; then the cloud Windows
tester VMs' env contract (`AGENT_DASHBOARD_URL=ws://<cp ip>:5030/ws/agent`,
`AGENT_API_KEY`, `AGENT_NAME=runner-K`, `AGENT_TESTER_PATH`) as machine env
vars + a wrapper `C:\networker\run-agent.cmd`, persisted as **`schtasks
/SC ONSTART /RU SYSTEM` task `NetworkerAgent`** and started through that same
task (log `C:\lab\agent.log`). Status/log go to `\\host.lan\Data` =
`lab/.generated[-N]/windows-runner-K/shared/`; `lab.sh up` waits until the
agent is **online at the control plane** (≤ `LAB_WINDOWS_TIMEOUT`).

**Registration**: `lab.sh` mints the key and inserts the `agent` row exactly
like for Linux runners (tags `{"lab":true,"os":"windows"}`), and additionally
inserts a `project_tester` row (`lab-runner-K-windows`, cloud `docker`, region
`lab`, `power_state running`, `requested_os windows`, auto-shutdown off) and
sets `agent.tester_id` to it — because the public launch API pins a run to a
runner only through `LaunchRequest.tester_id` (`RunDispatcher` tester
affinity: the agent *bound* to that project_tester wins). That is what
validate.sh phase 6 uses; the Linux runners stay standalone so fan-out spreads
freely.

**Not built from the checkout**: the Windows *tester* is the released
`networker-tester.exe` (x86_64-pc-windows-msvc is built on `windows-latest`
in release.yml; there is no cross-toolchain here). The *installer*
(`install.ps1 -Component tester`), the *agent* (published from `src/`) and
the *control plane* it talks to are the checkout's.

**Expected Windows differences** (phase 6 prints them; the native lab lists
the same): no Chrome / no tshark → `capabilities {chrome:false,tshark:false}`
(browser* modes are not in the matrix; `capture_mode: headers-only` degrades
to a warning inside the tester and the attempt still succeeds); `ping` uses
`IcmpSendEcho` (works without elevation, no `ping_group_range`); the h3 modes
through nginx and against the bare endpoint DO run (the release build carries
`http3`).

## Multiple instances (`LAB_INSTANCE`)

`LAB_INSTANCE=N` (default 1) makes every command act on an independent lab:

| | instance 1 (default) | instance N |
|---|---|---|
| compose project / containers | `nwk-lab` / `nwk-lab-runner-1-1` … | `nwk-labN` / `nwk-labN-runner-1-1` … |
| network | `172.31.100.0/24` (`nwk-lab_labnet`) | `172.31.(99+N).0/24` (`nwk-labN_labnet`) |
| host ports cp / ui / pg | 5030 / 8088 / 55432 | +N-1 each |
| Windows consoles | 8006+ | +100·(N-1) |
| state / topology | `lab/.state/`, `lab/.generated/` | `lab/.state-N/`, `lab/.generated-N/` |
| Windows VM volumes | `nwk-lab_windows-storage-N`, `…_windows-runner-storage-K` | `nwk-labN_…` |
| Docker (local) provider containers | join `nwk-lab_labnet` | join `nwk-labN_labnet` (`DASHBOARD_DOCKER_NETWORK`) |

Images (`nwk-lab/*:local`) are shared — pass `LAB_IMAGE_TAG` per checkout
when two trees build concurrently. All `LAB_*` overrides still apply on top;
`validate.sh` follows `LAB_INSTANCE` too (`lab.sh validate` passes it).

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
  `endpoint_ips` + `http_stacks` (+ `endpoint_hosts` for the Windows target),
  exactly what the proxy-kind resolver reads.
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
  `endpoint_hosts[0]` / `endpoint_ips[0]` — docker deployments record the
  container name as the hostname, resolvable on the lab network); no reference-API `languages` on docker targets (the
  target images ship the endpoint + proxy stacks only); Linux only.
* **Windows target / runner ≠ fully built from the checkout**: the Windows
  endpoint and tester binaries are the released ones (see above); the Windows
  *installer*, the *agent* (Windows runners) and the control plane are the
  checkout's. The [native Windows lab](#native-windows-lab) below builds the
  Windows tester from source on a Windows host (and uses the real cloud IIS
  payload).
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
  endpoint in phase 1. (The Docker lab's Windows VM is dispatched by its
  labnet hostname with a matching SNI binding, which is why `shared/http-stacks.json`
  says `iis: h3=true` — the manifest describes what the managed path does;
  the native lab's IP-literal default is the exception it documents. With
  `-Fqdn` the native lab registers the target under that name too; see the
  [Windows (IIS) target](#windows-iis-target) section.)
* **`install.ps1 -Setup iis`** was an HTTP-only stub until v0.28.208 (site on
  8082, no 8445 / ARR / H3); it is now the twin of the cloud payload (the
  Docker lab's Windows VM runs it), so `-IisSetup installer` is a valid
  choice too. `install.ps1 endpoint` insists on a release download or `cargo
  install` (no local-binary flag), so the native lab starts the locally built
  exe directly, like the Docker lab does.
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
    windows/oem/          the Windows TARGET VM's first-boot hook (install.bat → lab-setup.ps1 →
                          install.ps1 endpoint + -Setup iis; lab-postboot.ps1 after the reboot);
                          staged with the checkout's install.ps1 into .generated/windows-N/oem
    windows/oem-runner/   the Windows RUNNER VM's first-boot hook (install.bat → lab-runner-setup.ps1 →
                          install.ps1 tester + the checkout's agent as a SYSTEM ONSTART task);
                          staged with install.ps1 + the agent publish into .generated/windows-runner-K/oem
    agent-win.Dockerfile  Networker.Agent win-x64 self-contained publish (BuildKit --output → oem/agent)
    runner/entrypoint.sh  optional netem, then exec networker-agent
    ui.Dockerfile, ui/nginx.conf
  .generated[-N]/topology.yml, .state[-N]/   (git-ignored) generated services, keys, token, project id
  .generated[-N]/windows-N/{oem,shared}      (git-ignored) the target VM's /oem payload + its log/status share
  .generated[-N]/windows-runner-K/{oem,shared} (git-ignored) the runner VM's /oem payload (+ agent/) + share
  native/
    lab-native.ps1        native Windows twin (build/up/validate/status/logs/env/down) — drives validate.sh via Git Bash
    .state/               (git-ignored) bin/, publish/, logs/, lab.env, runner-1.key, iis-setup.ps1, pids.json
```

## Bugs the hostname/HTTP/3 run found (v0.28.211, fixed in the same PR)

* **Every multi-packet QUIC exchange with the Windows VM stalled** — QUIC
  handshakes took exactly ~1 s (one PTO retransmit), `pageload3` got 0/50
  assets and any h3 response larger than one packet hung until the idle
  timeout, against IIS *and* the bare Windows endpoint alike, while `/health`
  over h3 (one packet) was fine. Cause: the guest's virtio NIC has **UDP
  Segmentation Offload** on and msquic uses it; the USO super-datagrams never
  make it through dockur's tap/DNAT path. `lab-setup.ps1` (fresh installs) and
  the `windows-iis-refresh` script (existing disks) now run
  `Set-NetAdapterUso -IPv4Enabled $false -IPv6Enabled $false`; handshakes drop
  to ~1-2 ms and pageload3 is 50/50 in ~25 ms. This is a dockur/virtio quirk,
  not a Windows or http.sys one — the earlier "~1 s QUIC handshake, sporadic
  H3_INTERNAL_ERROR" note (v0.28.208) was this.

## Bugs the first Windows-target run found (fixed in the same PR)

* **Every HTTP/2 probe failed through IIS** (`http2`, `download2`, `upload2`,
  `pageload2`, the rpm load generator): the tester built h2 requests with a
  path-only URI, so hyper sent them without the `:scheme`/`:authority`
  pseudo-headers RFC 9113 §8.3.1 requires; nginx/caddy/hyper/quinn tolerate
  it, http.sys answers `RST_STREAM PROTOCOL_ERROR` (curl `--http2` worked, which
  is what pointed at the request). Now absolute-form URIs.
* **`websocket` through IIS 404'd**: no `/ws` ARR rule + no WebSocket Protocol
  feature in either IIS payload.
* **`install.ps1 -Setup iis` was a stub** (8082 only) and `install.ps1 -Yes
  -Component endpoint` on a fresh Server VM (no `gh`) fell into a source
  compile that needs MSVC — release download without gh + VC++ runtime now.
* **`iis` was `h3: true` on faith**: http.sys does QUIC on :8445 only with TLS
  SNI; by IP (how proxy targets were addressed) it closes the connection —
  manifest flipped to `false` in v0.28.208, then back to `true` in v0.28.211
  once proxy targets are dispatched by their recorded hostname
  (`deployment.endpoint_hosts`, V050) and the lab measured h3 2/2 through IIS.

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
