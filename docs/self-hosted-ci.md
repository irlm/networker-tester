# Self-hosted CI hosts

CI for this repo can run on the owner's own machines — 2-3 Linux VMs and an
optional Windows VM on the Proxmox box, plus the Mac mini — with **automatic
failover to GitHub-hosted runners** whenever a self-hosted machine is offline.
Self-hosted minutes are free, which is what makes flipping the repo to private
affordable (hosted minutes on a private repo bill ×2 for Windows and ×10 for
macOS).

**Vocabulary.** A *CI host* is the machine GitHub calls a "self-hosted
runner". In this repo "runner" already means a tester VM (`project_tester`,
the Runners UI) and "agent" means `Networker.Agent`, so nothing we name here
uses either word. GitHub's literal term survives only where its API or YAML
forces it: `runs-on`, the `/actions/runners` endpoints, the `actions-runner`
package, and the `RUNNER_GROUP` setting.

## Entry point: `infra/ci-hosts/setup-ci-hosts.sh`

One interactive, idempotent orchestrator stands every CI host up end to end.
Run it from any Linux/macOS box (bash 4+, ssh, curl, jq; `gh` optional):

```bash
infra/ci-hosts/setup-ci-hosts.sh               # full flow, prompts with defaults, re-runnable
infra/ci-hosts/setup-ci-hosts.sh status        # API table + VM state + ssh reachability
infra/ci-hosts/setup-ci-hosts.sh add-linux 2   # two more Linux CI hosts
infra/ci-hosts/setup-ci-hosts.sh destroy       # deregister + qm destroy every ci-* VM (asks first)
infra/ci-hosts/setup-ci-hosts.sh --non-interactive   # answers from infra/ci-hosts/ci-hosts.env
```

The flow, each step skipping work that is already done:

1. **GitHub** — repo slug (default from `git remote`), a PAT for registering
   hosts (hidden prompt, or `gh auth token` when it can mint registration
   tokens). The PAT is held in memory, shipped to each host over ssh into a
   file only root (Linux) or the login user (Mac) can read, and never written
   into the repo or echoed.
2. **Proxmox** — ssh alias or `user@host` (default `pve`; offers
   `ssh-copy-id`), storage/bridge/optional VLAN tag, N Linux hosts with
   cores/RAM/disk (default 3 × 4 vCPU / 8 GB / 60 GB on `local-lvm`, `vmbr0`
   untagged, VMIDs 301+). Creates the Ubuntu **24.04** cloud-init template at
   VMID 9001 if missing (downloads the noble cloud image and verifies it
   against Ubuntu's `SHA256SUMS`; the old 22.04 template at 9000 is never
   reused), enables the `snippets` content type on the snippet storage if
   it is missing (pve's `dir: local` ships with `iso,vztmpl,backup,import`
   only — every existing type is kept), clones each VM with cloud-init
   user-data that carries
   `linux/install-ci-host.sh` and the PAT, starts it, waits until it shows
   `online` in `GET /repos/{repo}/actions/runners`, then scrubs the PAT out of
   the cloud-init snippet on the Proxmox node.
3. **Mac mini** — ssh target (default alias `macmini`; Remote Login must be
   on), runs `macos/install-ci-host.sh` over `ssh -t` so the one-time sudo
   prompts (Homebrew, the `.pkg` casks, `pmset`) reach you, waits for
   `ci-macos-1` to be online. Re-runs skip the sudo steps, so the password is
   needed on the first run only. **Do not add a NOPASSWD sudoers rule for
   that user** (the only way to make the *first* run unattended): CI jobs run
   arbitrary repo code as that user, so passwordless sudo hands root to any
   pull request. The script never writes one.
4. **Windows (optional)** — creates VMID 310 (6 vCPU / 16 GB / 120 GB, q35 +
   OVMF + vTPM like VM 101) from the Server 2025 Evaluation ISO with
   `virtio-win.iso` and the `win-answer.iso` autounattend ISO attached, or
   reuses an existing VMID (e.g. 101). Once OpenSSH Server is enabled in the
   guest it runs `windows/install-ci-host.ps1` over ssh; otherwise it prints
   the exact manual steps.
5. **Verify** — the host table, an offer to dispatch a smoke run
   (`gh workflow run rust-audit.yml`), and the next steps (variable, secret,
   scheduled workflows, go private).

Per-OS building blocks it calls (usable on their own):

| script | runs where | what |
|---|---|---|
| `linux/install-ci-host.sh` | as root on an Ubuntu 24.04 VM | toolchains (rustup stable + rustfmt/clippy + musl target, .NET 10, Node 22, Docker, shellcheck, bats, jq, gh, pwsh, Chromium libs), persistent caches under `/var/cache/ci-host`, `actions-runner` under `/opt/actions-runner`, the `ci-host.service` ephemeral loop |
| `proxmox/create-ci-host-vm.sh` | as root on the Proxmox node | `ensure-snippets`, `snippet-dir`, `ensure-template`, `create`, `windows`, `start`, `destroy`, `list`, `ip` |
| `macos/install-ci-host.sh` | as the login user on the Mac | Homebrew: rustup (both darwin targets), dotnet-sdk, node@22, jq, gh, powershell; `~/ci-host/actions-runner`; a user LaunchAgent running the loop under `caffeinate -s`; `pmset` no-sleep |
| `windows/install-ci-host.ps1` | elevated PowerShell / ssh as admin | Chocolatey: git, 7zip, jq, gh, nodejs 22, dotnet-sdk 10, pwsh, VS 2022 Build Tools + C++; rustup msvc; IIS; PSScriptAnalyzer; `C:\actions-runner`; ephemeral loop as a SYSTEM Scheduled Task (or `-AsService`) |

## How a workflow picks its side

`.github/actions/pick-ci-hosts` is a composite action (plain bash, no external
actions). The first job of each routed workflow — the existing `changes`
detect job, or a tiny `ci_hosts` job where there was none — stays on
`ubuntu-latest`, runs the action, and exposes three outputs:

```yaml
outputs:
  linux_host:   ${{ steps.ci_hosts.outputs.linux }}    # e.g. ["self-hosted","linux","networker-ci"]
  windows_host: ${{ steps.ci_hosts.outputs.windows }}  # e.g. ["windows-latest"]
  macos_host:   ${{ steps.ci_hosts.outputs.macos }}
```

Every other job does `runs-on: ${{ fromJSON(needs.changes.outputs.linux_host) }}`
(windows/macos likewise) and lists that job in `needs`. The deciding hop costs
about ten hosted seconds per workflow run.

**Decision table** (`CI_HOSTS_MODE` is a repository *variable*):

| `CI_HOSTS_MODE` | result |
|---|---|
| `auto` (default, also when unset) | per OS: self-hosted iff at least one runner with status `online` carries `self-hosted` + that OS label + `networker-ci`; otherwise hosted |
| `hosted` | hosted for every OS |
| `self-hosted` | self-hosted for every OS, no online check (use to force a test or when the API is flaky) |
| anything else | warning, treated as `auto` |

Every fallback resolves to **hosted**: no token available (fork PRs never see
secrets), an API error or non-200, an unparseable body, no online host. A
pull request whose head lives in another repository is routed hosted
regardless of mode — fork code never runs on the owner's VMs. Each run prints
one `::notice::` line per OS saying which side was chosen and why, and the
same table lands in the job summary.

Busy hosts still count as online: queueing behind a busy self-hosted machine
is cheaper than hosted minutes. Ephemeral hosts drop off after every job and
re-register from their loop, so `online` is the right liveness signal.

### `CI_HOSTS_STATUS_TOKEN` — why a PAT

`GET /repos/{repo}/actions/runners` requires repository *Administration*
permission, which the workflow's `GITHUB_TOKEN` never has (there is no
`permissions:` key that grants it). Create a **fine-grained PAT** scoped to
this repository only with *Administration: Read* and store it as the secret
`CI_HOSTS_STATUS_TOKEN`. Read-only is enough for the picker; it is a different,
weaker token than the *Administration: Read and write* PAT the hosts use to
mint registration tokens (that one lives only on the hosts). Without the
secret every run stays hosted, loudly (`auto: no CI_HOSTS_STATUS_TOKEN
available to this run`).

### What is routed

`ci.yml`, `dotnet.yml`, `test-installer.yml`, `rust-audit.yml`,
`sdk-conformance.yml`, `validate-bench-apis.yml`, `test-endpoint.yml` and
`release.yml`. In `release.yml` the `build-native` matrix carries a `host:
macos|windows` key and uses
`runs-on: ${{ fromJSON(needs.ci_hosts.outputs[format('{0}_host', matrix.host)]) }}`;
the prod **`deploy` job is deliberately not routed** (see the security model).
`verdict` in `validate-bench-apis.yml` runs with `if: always()`, so its
`runs-on` falls back to `ubuntu-latest` if the picker job itself failed.

### What stays on GitHub-hosted

Three jobs carry a literal `runs-on` on purpose and are never routed:

| job | why |
|---|---|
| `test-installer.yml` → `stack-exec` | runs the real `install.sh --setup-stack` as root: installs nginx/caddy/apache/haproxy/traefik plus `networker-*.service` units and leaves them running |
| `test-installer.yml` → `linux-bench-exec` | real `install.sh --benchmark-server`: bench servers under `/opt/bench` bound to :8085/:8086, and `/opt/bench/bench-data.json` |
| `test-installer.yml` → `windows-exec` | real `install.ps1` run on Windows |
| `release.yml` → `deploy` | prod credentials (security model below) |

Ephemeral registration resets the *runner*, not the *disk*: a hosted VM is
thrown away after the job, a CI host persists. The first full run on the
hosts showed what that costs — `networker-endpoint`'s PRNG-fallback unit
tests failed on `ci-linux-1` because `load_bench_data()` found the
`/opt/bench/bench-data.json` a previous `linux-bench-exec` had installed, and
`ci-linux-2` had five proxies and two bench servers holding ports 80-8457
between jobs. Anything that installs packages or services system-wide belongs
on a hosted runner (or, later, on a VM the hypervisor rolls back to a snapshot
after every job — `qm rollback`, not yet built).

Per-job residue that does not need a fresh disk is wiped by the Linux loop
between jobs (`ci-host-loop.sh`): the `_work` checkout (root-owned files from
sudo/docker steps otherwise break the next `actions/checkout`), every Docker
container and unused network, `/tmp/bench`, any `/usr/local/bin/networker-*`
binary an integration job `sudo install`ed (a leftover tester turned a stubbed
installer unit test into an 18-minute real probe), any process still running
as the CI user (sccache servers, stray endpoints), and foreign-owned entries in
the shared caches under `/var/cache/ci-host` (a `sudo -E dotnet` leaves
root-owned NuGet packages and the next restore dies with EACCES). The loop
also starts the runner with the toolchain `PATH` (`JOB_PATH` in
`/etc/ci-host/env`): the actions-runner regenerates `.path` from its own
process PATH on every start and ignores `PATH=` in `.env`, so a job with no
toolchain action would otherwise not find `cargo`. To reset a host that has
drifted anyway: `setup-ci-hosts.sh destroy` + `setup` rebuilds the Linux VMs
from the template in ~15 minutes while `auto` routes to hosted.

## Security model

- **Network.** CI host VMs sit on their own VLAN with no route to prod or to
  other LAN hosts; they need outbound HTTPS (github.com, api.github.com,
  package mirrors) and nothing inbound except ssh from the operator's box.
  `PVE_VLAN` tags the VMs' NIC; the firewall policy is the hypervisor's job.
- **Ephemeral registration.** Every host registers with `--ephemeral`: the
  runner process serves exactly one job, de-registers, and the loop
  (`ci-host.service` / LaunchAgent / Scheduled Task) mints a fresh
  registration token and registers again. A job cannot leave a registered
  runner behind for the next one. The PAT that mints tokens is in a file only
  root (Linux) or SYSTEM/Administrators (Windows) or the login user (Mac) can
  read, and is never exported into the job's environment.
- **A host that dies mid-run does not fail the run — it wedges it.** The
  picker decides once, when the run starts. A host lost before a job is
  dispatched leaves that job queued against labels nothing can satisfy, and
  `timeout-minutes` does not help: that clock only starts when a job starts
  *running*, so GitHub's 24-hour queue limit is the only backstop.
  `ci-hosts-watchdog.yml` closes this — every 13 minutes it cancels runs whose
  jobs have been queued past 12 minutes on labels no ONLINE host carries
  (a job queued behind a merely *busy* host is left alone), so a re-run picks
  GitHub-hosted. It runs on `ubuntu-latest` by design: a watchdog that can be
  taken out by the outage it exists to detect is no watchdog. Jobs that are
  already running when a host dies are failed by GitHub within ~10 minutes,
  and every routed job now carries a `timeout-minutes` instead of the 6-hour
  default.
- **VM state is not reset between jobs.** Ephemeral covers the *registration*,
  not the disk. The jobs that install services system-wide (`stack-exec`,
  `linux-bench-exec`, `windows-exec`) are therefore pinned to hosted runners
  (§ "What stays on GitHub-hosted"); the loop wipes per-job residue; rebuild
  the Linux hosts from the template when they drift anyway (`destroy` +
  `setup`, ~15 minutes, safe because `auto` routes to hosted meanwhile).
- **Prod secrets stay hosted.** `release.yml`'s `deploy` job (the only job
  with `AZURE_CREDENTIALS`, via the `production` environment) keeps
  `runs-on: ubuntu-latest`. Any future job that touches prod credentials does
  the same, or runs on one dedicated, trusted host that never takes PR jobs.
- **Runner group.** Put the hosts in a runner group restricted to this
  repository (Settings → Actions → Runner groups) and set `RUNNER_GROUP` in
  `ci-hosts.env`; the hosts then cannot be targeted by any other repository
  in the account.
- **Fork PRs.** They never receive `CI_HOSTS_STATUS_TOKEN`, so `auto` routes
  them hosted; the action additionally forces hosted for any cross-repository
  pull request even under `CI_HOSTS_MODE=self-hosted`. Keep "Require approval
  for all outside collaborators" on while the repo is public.

## Containerised hosts — for a machine that is not dedicated CI

`infra/ci-hosts/container/` runs a CI host inside Docker, for a machine that is
already doing something else. The first is `ci-turing-1` on `turing`
(10.10.10.93, i7-8750H, 12 threads, 62 GB), which also runs nginx on :8080,
Samba, ollama, an openclaw gateway and node_exporter.

**Why containerised and not bare metal.** The ephemeral loop's between-jobs
sweep is written for a machine it owns:

```
docker ps -aq | xargs -r docker rm -f     # every container on the box
rm -rf /tmp/bench /tmp/networker-*        # anything at those paths
pkill -TERM -u "$CI_USER"                 # every process of the CI user
```

On a dedicated VM those are correct and necessary. On a shared machine each is
a live grenade: the `docker rm -f` alone would have deleted the owner's running
container and, with a prune, six images. Inside a container all three are
harmless — `/tmp` and the process table are the container's own, and `docker`
talks to a **DinD sidecar** rather than the machine's daemon, so the sweep can
only ever delete containers the CI jobs themselves created.

**The host's Docker socket is deliberately not mounted.** The entrypoint
*refuses to start* without `DOCKER_HOST`, rather than falling back to a socket:
a silent fallback would hand both the jobs and that sweep the machine's real
containers, which is the one thing this arrangement exists to prevent.

**No published ports.** A runner only makes outbound connections, so the
container collides with nothing the machine already serves.

**Same installer.** The image runs the real
`infra/ci-hosts/linux/install-ci-host.sh --container`, so a containerised host
and a VM host get their toolchains from the same code. `--container` skips only
what assumes ownership of a machine: systemd (the entrypoint runs the loop as
PID 1's child and the container runtime does the restarting), the local
`dockerd`, and qemu-guest-agent. A parallel Dockerfile with its own apt list
would have drifted within a release.

```bash
cp infra/ci-hosts/container/.env.example infra/ci-hosts/container/.env   # edit
docker compose -f infra/ci-hosts/container/compose.yml up -d --build
```

The PAT is a **mounted file**, never a build arg, an environment variable or a
layer; the image is built with a placeholder that is deleted in the same layer.

Verified on turing at v0.28.285: the container sees an empty Docker world, and
running the loop's full sweep inside it left the machine's own container and all
six of its images untouched.

**Fidelity gaps.** Jobs that install system services (`stack-exec`,
`linux-bench-exec`) are pinned to hosted runners anyway, so nothing is lost
there. Nested Docker is slower for image-heavy jobs than a native daemon, and
the DinD storage volume is a second layer cache rather than a shared one.

## Private-repo minute math

Hosted minutes on a private repo are billed: Linux ×1, Windows ×2, macOS ×10
against the plan's allowance (2,000 min/month on Free, 3,000 on Pro/Team).
The PR→prod path is covered by the routing above, but **the scheduled
workflows are not routed by this PR** and will burn hosted minutes from the
moment the repo goes private:

| workflow | cadence | runs-on | hosted min/month (approx.) |
|---|---|---|---|
| `uptime-monitor.yml` | every 10 min | ubuntu | 4,380 runs × ~1 min ≈ **4,400** |
| `soak-check.yml` | daily | ubuntu | 30 × ~8 ≈ 240 |
| `soak-canary.yml` | daily + weekly | ubuntu | 35 × ~15 ≈ 500 |
| `release-gap-check.yml` | daily | ubuntu | 30 × ~1 ≈ 30 |
| `lab-smoke.yml` | weekly | ubuntu | 4 × ~25 ≈ 100 |
| `lab-windows-native.yml` | weekly | **windows** | 4 × ~40 × 2 ≈ 320 |
| `benchmark.yml` | weekly | ubuntu | 4 × ~60 ≈ 240 |
| `mutation.yml` | weekly | ubuntu | 4 × ~90 ≈ 360 |
| `microbench-dotnet.yml` | weekly | ubuntu | 4 × ~20 ≈ 80 |
| `soak-endurance.yml` | weekly | ubuntu | 4 × ~120 ≈ 480 |
| `rust-audit.yml` (schedule) | weekly | routed | ~0 when a Linux host is online |
| `validate-bench-apis.yml` (schedule) | weekly | routed | ~0 when a Linux host is online |
| `sync-gist.yml`, `wiki-setup.yml` | on push / manual | ubuntu | negligible |

`uptime-monitor` alone exceeds every non-Enterprise allowance. **Precondition
for going private:** route (same pattern — add the `ci_hosts` job, point
`runs-on` at it) or disable every scheduled workflow above first. The
uptime monitor in particular belongs on a self-hosted Linux host or on the
independent monitoring plane (`docs/monitoring-plane-design.md`), not on
hosted minutes.

## Windows licensing

- **Windows Server 2025 Evaluation** is what `windows-latest` runs, so it is
  the faithful choice: 180 days, and `slmgr /rearm` extends it up to 5 times
  (≈ 3 years). After that, rebuild from the ISO/template — the CI host is
  stateless apart from caches. `setup-ci-hosts.sh` defaults to the evaluation
  ISO already on the node and the owner's `win-answer.iso` for an unattended
  install.
- **Windows 11 Pro** with a retail key is the fully-licensed alternative and
  runs fine in a vTPM + UEFI VM (which is exactly how `create-ci-host-vm.sh
  windows` shapes the VM: `--bios ovmf --machine q35 --tpmstate0
  …,version=v2.0 --ostype win11`). The only CI difference is IIS: Pro has the
  IIS optional feature (`Enable-WindowsOptionalFeature`), which
  `install-ci-host.ps1` handles; Server uses `Install-WindowsFeature`.

## Rollout order

1. **Merge this PR while the repo is still public.** Nothing changes until
   the variable/secret exist: `auto` with no token resolves to hosted, so CI
   behaves exactly as before (minus the ~8 min auto-tag wait).
2. **Bring the hosts up:** `infra/ci-hosts/setup-ci-hosts.sh`. Verify with
   `status` that each shows `online` with the right labels.
3. **Add the secret** `CI_HOSTS_STATUS_TOKEN` (fine-grained PAT, this repo,
   Administration: Read) and **set the variable** `CI_HOSTS_MODE=auto`.
   Watch a few PRs: every routed workflow's first job now prints
   `linux -> ["self-hosted","linux","networker-ci"] (auto: 3 online CI
   host(s) …)`. Power a host off and watch the next run say `hosted` instead.
4. **Route the scheduled workflows** (table above) or disable them.
5. **Flip the repo to private.** Hosted minutes are now the exception
   (failover + the deciding hop + `deploy`), not the rule.

Back out at any time with `CI_HOSTS_MODE=hosted` — no workflow edit needed.

## The Mac and reboots

`macos/install-ci-host.sh` defaults to a **LaunchAgent**, which is what makes
the whole macOS install sudo-free — but a LaunchAgent only runs inside a
logged-in user session. After a reboot (a power cut, say) the Mac comes back
up, answers ssh, and is *not* a CI host, because nobody logged in. Tailscale's
menu-bar app has the same property, so the machine can also disappear from the
tailnet at the same time and only be reachable by LAN address.

Run the installer with `--daemon` (or answer yes to `MAC_BOOT_DAEMON`) to get
`/Library/LaunchDaemons/com.networker.ci-host.plist` instead: it starts at
boot, runs as the invoking user, and needs one sudo at install time. The
installer refuses to leave both registered — two loops would fight over the
same runner name, each `--replace`-ing the other.

To convert a Mac that already has the agent:

```bash
ssh macmini 'bash ~/ci-host/install-ci-host.sh --repo irlm/networker-tester \
  --name ci-macos-1 --skip-toolchains --daemon'      # needs a TTY for sudo: use ssh -t
launchctl print system/com.networker.ci-host | head   # verify
```
