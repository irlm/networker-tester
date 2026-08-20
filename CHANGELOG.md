# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [0.28.274] - 2026-08-20

### Added

- **CI hosts watchdog (`.github/workflows/ci-hosts-watchdog.yml`).**
  `pick-ci-hosts` decides once, when a run starts; if a CI host goes offline
  between that decision and the job being dispatched, the job queues against
  labels nothing can satisfy and sits there for GitHub's 24-hour limit.
  `timeout-minutes` does **not** cover this — that clock only starts when a job
  starts *running*. Every 13 minutes the watchdog looks for jobs queued longer
  than 12 minutes on `self-hosted` labels, checks whether any online host
  carries every one of those labels, and cancels only the runs that are
  genuinely unsatisfiable (a job waiting behind a *busy* host is left alone).
  Re-running such a run routes it to GitHub-hosted, because the fresh picker
  sees no online host. Found the hard way: on 2026-08-20 a power cut rebooted
  the Mac mini mid-release and `Build x86_64-apple-darwin` wedged v0.28.268
  twice until it was cancelled by hand.
- **`macos/install-ci-host.sh --daemon` — a CI host that survives a reboot.**
  The LaunchAgent the script installed until now only runs while the user is
  logged in, so the same power cut left `ci-macos-1` offline with the Mac up
  and reachable. `--daemon` installs `/Library/LaunchDaemons` instead, running
  the loop at boot as the invoking user (`UserName`/`GroupName`/`HOME` set
  explicitly, since a daemon inherits none of them), at the cost of one sudo.
  `--agent` keeps the old zero-sudo behaviour and now warns about the reboot
  gap; `setup-ci-hosts.sh` asks (`MAC_BOOT_DAEMON`, default yes) and never
  leaves both a daemon and an agent registered under the same runner name.

### Fixed

- **Rust 1.98 clippy broke `main` for every PR.** `dtolnay/rust-toolchain@stable`
  started resolving to 1.98.0 (released 2026-08-18) mid-afternoon, and two lint
  families fired repo-wide: `chunks_exact_to_as_chunks` (new) at
  `runner/ping.rs` and two sites in `runner/http.rs`, and a tightened
  `result_large_err` on `run_one_tls_http_request`, whose ~288-byte error tuple
  every `Result` carried on the success path too. The slice sites now use
  `as_chunks::<N>()`/`as_chunks_mut::<N>()`; the TLS failure tuple is boxed
  behind a documented `TlsRequestFailure` alias (the failure path is cold, so
  the allocation costs nothing measurable). Verified against a locally pinned
  1.98.0 toolchain, not just the current stable.
- **`sdk-js` was a `block` lint section that CI never ran — and it was
  failing.** The `frontend` job only invokes `frontend-eslint` and is gated on
  `dashboard/` changes; `sdk-conformance` builds and tests `sdk/js` without
  type-checking it. So 12 `TS18046`/`TS2571` errors sat on pristine `main`
  with nobody to see them. `lint-all` now runs `sdk-js` unconditionally,
  alongside json/version/workflows — a baseline that only runs on some paths
  is not a baseline. The errors themselves were real: undici types
  `Response.json()` as `Promise<unknown>`, so every contract assertion on a
  wire field needed a cast; `test/helpers.ts` now exports a documented
  `jsonBody()` and the three suites go through it. (Reported by a parallel
  session working in the same tree.)
- **The `streaming memory bound` JS conformance test no longer races its
  probe.** It attached a `data` listener per `nextLine()` call and removed it
  on resolve, so the child's `RESULT` line — written while the parent was busy
  draining 32 MiB — could land with no listener attached and be lost when the
  stream ended, surfacing as a flaky `memprobe exited early (0)` on loaded CI
  hosts. (v0.28.268 moved the give-up signal from `exit` to `close`, which
  narrowed the window without closing it.) One persistent reader now collects
  every line for the child's lifetime and `nextLine()` polls that buffer; the
  failure message quotes what the child actually printed.
- **Self-hosted jobs get the toolchain PATH — for real this time.** The PATH a
  *job step* runs with comes from `<runner>/.path`, not from `.env` and not
  from the Listener's own environment: the runner writes `.path` at configure
  time and reuses the file. v0.28.268 fixed the Listener's environment, which
  was not enough — jobs using `setup-*`/`dtolnay/rust-toolchain` masked it
  (those prepend via `GITHUB_PATH`), while a step calling the toolchain
  directly still got `cargo: command not found` (validate-bench-apis'
  canonical Rust baseline, on every Linux host). All three installers now
  write `.path` explicitly and the loops restore it after each registration,
  because `config.sh` rewrites it from its own environment.

### Changed

- **CI hosts stop paying for the GitHub cache over a home uplink.** `sccache`
  used the Actions cache backend everywhere (`SCCACHE_GHA_ENABLED=true`), so
  on a self-hosted host every cache hit was a download across home broadband —
  which is why the musl build still took 4-6 min there against 8 min on a
  hosted runner with no local cache at all. Self-hosted runs now point sccache
  at `/var/cache/ci-host/sccache`, which survives the per-job workspace wipe,
  and skip `Swatinem/rust-cache` entirely: `CARGO_HOME` already persists on
  the host, so restoring the same registry over the uplink was pure cost.
  GitHub-hosted runs are unchanged (`runner.environment` decides).
- **`validate-bench-apis` retries the base-image pull.** `TLS handshake
  timeout` to registry-1.docker.io and `failed to fetch anonymous token` from
  auth.docker.io each killed a real run on 2026-08-20 — not rate limiting (94
  of 100 anonymous pulls were left), just a flaky uplink. Three attempts with
  backoff instead of a manual re-run.
- **36 routed jobs got a `timeout-minutes`.** They inherited GitHub's 6-hour
  default, so a CI host that dies mid-job held a slot for hours; the caps are
  roughly 3x observed runtime (e.g. `Test (windows-latest)` 45, `Coverage` 30,
  `bats` 30, `action-pins` 10). This bounds a hung job, not a queued one —
  the watchdog above is what handles queueing.

---
## [0.28.273] - 2026-08-20

### Fixed

- **Dashboard layouts remain usable across touch, narrow, and short
  viewports.** The coarse-pointer touch-target rule no longer overrides fixed
  controls; mobile navigation is viewport-bounded, scrollable, and always
  expanded; phone-landscape navigation adapts to limited height; and shared
  dialogs, slide-overs, Help/Search overlays, and performance tools stay
  within the available viewport. Browser regression coverage now exercises
  every rendered route, the 320–1920px width matrix, intermediate resizing,
  touch navigation, and representative overlay interactions.

  The coarse-pointer touch-target rule now lives in `@layer base` rather than
  guarding itself with `:not(.fixed):not(.absolute):not(.sticky)`. Layer order
  beats specificity, so an unlayered rule outranked every Tailwind positioning
  utility — that is what forced `position: relative` onto fixed controls in
  the first place. Layered, the utilities win by construction, including on
  buttons positioned by a component class or an inline style, which a
  class-name guard would still have clobbered.

  Overlay scroll locking is refcounted (`lib/useBodyScrollLock.ts`). Saving
  and restoring `document.body.style.overflow` per overlay loses the page's
  own value as soon as two overlap — open a modal over the mobile drawer and
  whichever unmounts first unlocks the page behind the other.

## [0.28.271] - 2026-08-20

### Added

- **Every dashboard page now stays fresh** (freshness audit of all 45 pages).
  Two cross-cutting triggers: react-query refetches on window focus (bounded
  by the 10s staleTime), and `usePolling` pages fire an immediate tick when
  the tab becomes visible again. Thirteen stale pages got real refresh:
  Canary (dispatch outcomes now appear, Refresh includes status), Network
  Test (a run launched from the page shows up immediately; deployments and
  runner state poll), Comparison Results (group status and attempts track
  running cells at 5s), Leaderboard and Benchmark Config Results (silent 30s
  polls; a spurious double-fetch on first testbed selection fixed), VM
  History (silent, pagination-safe 30s), Settings, Cloud Accounts, Project
  Members, Endpoint hero, Scenarios recent-runs, Run Detail group siblings,
  and the URL-probe runner picker. Command Approvals gains a 30s polling
  safety net under its SSE trigger, and the System Logs pause button now
  actually pauses the poll.

---

## [0.28.269] - 2026-08-20

### Added

- **URL Probe: compare a host by runner provider and by runner capacity.**
  The Watched URLs list folded every run on a host into one row no matter
  which tester VM probed it, so picking `microsoft.com` averaged an Azure
  `Standard_B1s` against a GCP `e2-standard-4` and the spread read as a
  network-path difference when it was a runner-infrastructure one. The
  toolbar gains a **By host / By provider / By capacity** select (host is
  the unchanged default; provider = host × runner cloud; capacity = host ×
  runner cloud × VM size) plus **Provider** and **Size** filters whose
  options are the values present in the loaded runs — sizes labelled with
  the VM catalog's `N vCPU / N GB` and sorted by vCPU, memory, then name.
  Grouped rows render the provider badge, size, region (or `N regions` when
  a capacity row spans several) and catalog specs in the infra-envelope
  `SideLine` style. A run whose runner cannot be resolved lands in an
  explicit **unknown runner** row — never silently merged into a real
  bucket. `?group=`, `?provider=` and `?size=` persist next to `?host=` so a
  comparison view is shareable. Pure bucket-key / option helpers live in
  `dashboard/src/lib/probe-grouping.ts` with vitest coverage.
- **Run list carries the runner identity.** `GET
  /api/v2/projects/{projectId}/test-runs` items gain the additive
  snake_case fields `runner_cloud`, `runner_region`, `runner_vm_size`
  (from the run's tester; null when the run has no tester or the tester
  was deleted) and `runner_vcpus` / `runner_memory_gb` (from
  `VmNetworkSpecs`; null when the size is not catalogued). The list
  projection is now a `RunListRow` + `BuildRunListItem` seam, mirroring
  `BuildRunDetail`, and `TestRunsContractTests` pins the list field set.
  The run DETAIL shape is unchanged.

---

## [0.28.268] - 2026-08-20

### Fixed

- **Windows CI host: first logon installs the VirtIO serial driver before
  the QEMU guest agent.** The `qemu-ga` MSI does not carry `vioserial`, so
  the service ran but `qm guest cmd ping` never answered and
  `setup-ci-hosts.sh` could not discover the guest's IP. `pnputil` now
  installs `vioserial\2k25` from the virtio-win ISO first. Found on the first
  successful unattended Server 2025 install (VM 310), which itself needed
  every answer-file `<component>` to carry `publicKeyToken`/`versionScope`
  (#842) — WinPE tolerates their absence, the specialize pass does not.
- **`setup-ci-hosts.sh` ships the PAT to the Windows guest without nested
  PowerShell quoting.** The guest's ssh shell is already PowerShell; wrapping
  the token write in `powershell -Command "..."` failed with "The string is
  missing the terminator". A failing `install-ci-host.ps1` now warns and
  returns instead of aborting the whole run.
- **Windows CI host: `bash` on the machine PATH, and the loop survives a
  killed Listener.** The first self-hosted Windows job failed in 22 s because
  `dtolnay/rust-toolchain` (like every `shell: bash` step) needs `bash.exe`
  from `Git\bin` — choco only adds `Git\cmd`, GitHub-hosted images have both.
  `install-ci-host.ps1` now adds `Git\bin` and `Git\usr\bin`; its loop
  removes the hidden `.runner`/`.credentials` files before every
  `config.cmd` (otherwise "already configured" forever after a restart, as on
  Linux) and writes a transcript to `C:\ProgramData\ci-host\ci-host-loop.log`.
- **Tester h3 unit tests wait up to 30 s (was 5 s) for their in-process QUIC
  server.** `wait_for_quic` is a positive-signal gate — it returns the moment
  Quinn is bound — so the cap only matters under load, where 5 s produced
  "QUIC server did not start" on a busy self-hosted Linux host and once on the
  Mac (`pageload_h3_empty_assets`,
  `h3_download_carries_quic_stats_without_resumption_stats`).
- **Linux CI-host loop removes stray `/usr/local/bin/networker-*` binaries
  between jobs.** A tester an integration job had `sudo install`ed turned
  the stubbed bats test `_offer_quick_test … release download` into a real
  5-run, 7-mode probe against `1.2.3.4` — 18 minutes on `ci-linux-1`, the
  only outlier in an otherwise 13-minute run.

---
## [0.28.267] - 2026-08-20

### Added

- **Self-hosted CI hosts with automatic failover to GitHub-hosted.** (A *CI
  host* is the machine GitHub calls a "self-hosted runner"; "runner" in this
  repo already means a tester VM.) A new composite action
  `.github/actions/pick-ci-hosts` decides, per OS, whether a workflow run's
  jobs land on the repo's own CI hosts (labels
  `self-hosted,<linux|windows|macos>,networker-ci`) or on `ubuntu-latest` /
  `windows-latest` / `macos-latest`. In `auto` mode it lists the repo's
  self-hosted runners with the `CI_HOSTS_STATUS_TOKEN` PAT and picks
  self-hosted only when a host with the right labels is **online right now**;
  a missing token (fork PRs never see secrets), an API error, an unknown
  `CI_HOSTS_MODE`, or no online host all fall back to hosted, so a powered-off
  VM can never stall CI. `CI_HOSTS_MODE=hosted|self-hosted` forces either
  side; a pull request from a fork is always hosted regardless. Routed through
  it: `ci.yml`, `dotnet.yml`, `test-installer.yml`, `rust-audit.yml`,
  `sdk-conformance.yml`, `validate-bench-apis.yml`, `test-endpoint.yml` and
  `release.yml` (every job except the prod `deploy`, which stays hosted by
  design). The deciding hop itself stays on `ubuntu-latest` (~10 hosted
  seconds per run).
- **`docs/self-hosted-ci.md` + `infra/ci-hosts/`.** `setup-ci-hosts.sh` is
  the one interactive, idempotent entry point (Proxmox Linux VMs from a
  cloud-init template, the Mac mini over ssh, the optional Windows VM,
  verification, plus `status` / `add-linux N` / `destroy` and a
  `--non-interactive` mode driven by `ci-hosts.env`); the per-OS building
  blocks it calls are `linux/install-ci-host.sh` (Ubuntu 24.04 toolchain +
  ephemeral systemd loop), `proxmox/create-ci-host-vm.sh`,
  `macos/install-ci-host.sh` (launchd loop) and `windows/install-ci-host.ps1`
  (IIS for the installer stack tests). The doc covers the failover semantics,
  the security model (own VLAN, `--ephemeral` registration, repo-restricted
  runner group, prod secrets stay hosted), the private-repo minute math (the
  scheduled workflows that must be routed or disabled first —
  `uptime-monitor` alone is ~4,400 hosted minutes/month), the Windows Server
  2025 Evaluation licensing note, and the rollout order.

### Changed

- **`auto-tag` no longer queues behind the main-branch test matrix.** It
  needed `[lint, test-ubuntu, frontend]` — ~8 minutes re-running what branch
  protection had already required green on the PR, sitting squarely on the
  green-PR→prod path (25-30 min measured). It now needs only `changes` and,
  before tagging, re-proves the merge: resolves the PR from the squash
  subject's `(#N)`, checks that the PR is MERGED **as this commit**, and
  requires every branch-protection-required check on the PR head to have
  conclusion `success` or `skipped`. A direct push to main (no `(#N)`) or any
  non-green required check prints exactly what was not green and refuses to
  tag. The main-branch matrix still runs (coverage, soak record) — the tag
  just does not wait for it.
- **The installer exec jobs are pinned to GitHub-hosted runners.**
  `test-installer.yml`'s `stack-exec`, `linux-bench-exec` and `windows-exec`
  run the real `install.sh`/`install.ps1` as root and install system services
  (five proxies, `networker-*.service` units, bench servers under
  `/opt/bench`). The first full run on the CI hosts showed why that cannot
  share a persistent machine with the rest of CI: `networker-endpoint`'s
  PRNG-fallback unit tests failed on `ci-linux-1` because `load_bench_data()`
  found the `/opt/bench/bench-data.json` a previous `linux-bench-exec` had
  left, and `ci-linux-2` held ports 80-8457 between jobs. `dotnet.yml`'s
  `reinstall-exec` now removes its stub `networker-agent.service` when it is
  done, and the Linux loop wipes per-job residue (Docker containers,
  `/tmp/bench`, processes still running as the CI user, root-owned entries a
  `sudo -E` build left in the shared NuGet/npm caches) between jobs. The
  loop also launches the runner with the toolchain `PATH` explicitly — the
  actions-runner rewrites `.path` from its own process PATH at every start
  and ignores a `PATH` line in `.env`, which surfaced as `cargo: command not
  found` in the one job without a toolchain action — and clears a stale
  `.runner`/`.credentials` pair before every registration (a Listener killed
  mid-flight otherwise wedges `config.sh` on "already configured").
  `docs/self-hosted-ci.md` § "What stays on GitHub-hosted".

### Fixed

- **JS SDK conformance `streaming memory bound` raced the probe's exit.** The
  test rejected on the child's `exit` event, which Node can emit before the
  final stdout chunk (the `RESULT` line) reaches the parent — "memprobe exited
  early (0)" with a correct result in flight, first seen on a self-hosted CI
  host. It now waits for `close` (every stdio stream drained) and scans the
  buffered lines once more before giving up.

---
## [0.28.266] - 2026-08-20

### Fixed

- **GCP endpoint firewall rule is reconciled, not "reused"** (#840 — the 7th
  site of the GCP chain). With #836 fixed the stacks finally installed on the
  GCE VMs (`nginx configured on ports 8081/8444`, `caddy set up`), yet every
  proxy port still timed out from outside: `_gcp_create_firewall_rule`
  matched the rule `networker-endpoint-allow` by name and returned "already
  exists — reusing", so a rule created back in v0.12.83 never gained the
  proxy-stack ports the installer has added six times since. An existing rule
  is now updated to the single canonical port list the create path uses;
  bats pins both branches and checks the list covers every port in
  `shared/http-stacks.json`.

## [0.28.265] - 2026-08-20

### Fixed

- **GCP endpoint VMs are torn down when their run finishes** (#838 — the 6th
  site of the GCP chain). The deployment teardown reverse-looks the VM up by
  the public IP install.sh reported, and that lookup was Azure-only: for GCP
  it logged "not implemented … skipping VM teardown", flipped the row to
  `torn_down` anyway, and the orphan reaper (also Azure-only) never came —
  every comparison cell leaked its GCE instance until the 04:00 shutdown
  cron stopped (not deleted) it. `ResolveByEndpointAsync` now lists the
  project's instances by NAT IP and returns the selfLink the gcp lifecycle
  delete parses zone+name from; the teardown threads the deployment's account
  key (same resolution as the install.sh staging from #833) because gcloud
  authenticates only from the per-invocation override (#827). Azure/AWS
  teardown is unchanged (ambient). A GCP sweep for the orphan reaper remains
  open in #838.

## [0.28.264] - 2026-08-20

### Fixed

- **GCP endpoint deploys now actually get their HTTP stack** (#836 — the 5th
  site of the GCP chain). With #833 fixed, both GCP cells passed pre-flight
  and their VMs came up healthy, then died at the readiness gate ("never
  became reachable within 6m"): `_gcp_ssh_run` nulls stdin (curl|bash
  protection), which silently replaced the heredoc carrying the nginx
  configuration to the GCE VM — `bash -s` read EOF, did nothing, exited 0,
  nginx never listened. Heredoc callers now use `_gcp_ssh_script` (stdin
  forwarded) and a failing remote script is reported with its exit status.
  Caddy/Apache/HAProxy/Traefik on GCP Linux no longer print "not yet
  supported" and let the cell time out: they run the installer's own
  `--setup-stack` over `gcloud compute ssh` — the same lab-validated path
  Azure/AWS/LAN use — and a failed stack setup fails the deploy immediately.
  The two inline "resolve the installer to pipe over SSH" copies became one
  `_installer_self_for_ssh` helper.

## [0.28.263] - 2026-08-20

### Added

- **`scripts/lint-all.sh` — one lint entry point for every language.** Rust
  (fmt, clippy, no-default-features build, rustdoc lint, orchestrator/SDK
  crates), C# (`dotnet build`, `dotnet format`), dashboard (tsc, ESLint),
  sdk/js, Go, Python, bash (shellcheck), bats, PowerShell (PSScriptAnalyzer +
  the PowerShell 5.1 parse), GitHub workflows (SHA pins, actionlint),
  Dockerfiles (hadolint), JSON, the C# benchmark template drift check, and
  the five-file version sync. Sections are `block` (CI-enforced) or `info`
  (known baseline, reported; `--strict` to enforce); `--fix`, `--only`,
  `--skip`, `--no-build`, `--list`; a missing tool prints its install command
  (`LINT_DOCKER=1` runs shellcheck/actionlint/hadolint from pinned images).
  CI's Rust `Lint` steps, the installer `shellcheck` job and the `Action pins`
  job now call the script, so local and CI run one definition of each
  command; **ESLint is now enforced in CI** (it was local-only), and a new
  ungated `lint-all (cross-cutting)` job runs actionlint, JSON syntax and
  version-sync on every PR.
- **`AGENTS.md`** — the rules every coding agent follows here (lint as
  strictly as the tree allows before reporting done, no unjustified
  suppressions, promote cleaned `info` sections to `block`, scope
  discipline), imported by `CLAUDE.md` so Claude Code and the other agents
  share one source.

### Changed

- `scripts/dev-setup.sh` offers `actionlint` alongside shellcheck/bats; the
  PR template points at `scripts/lint-all.sh` and no longer claims the Gist
  sync is broken (it has auto-run on every `main` push since 2026-07-13).
## [0.28.262] - 2026-08-20

### Fixed

- **GCP endpoint deploys authenticate install.sh** (#833 — the #827 auth bug
  at its fourth site). Endpoint deployments delegate VM creation to
  `install.sh --deploy` on the control-plane host, and the deploy runner
  spawned it with no cloud credentials, so install.sh's GCP pre-flight — a
  bare `gcloud auth list` against the host's never-authenticated config
  store — failed every GCP comparison cell in seconds with "Not authenticated
  to GCP". The runner now decrypts the cloud account's service-account key
  (the deployment's account, or the project's single active GCP account for
  wizard deploys), stages it 0600 inside a 0700 throwaway `CLOUDSDK_CONFIG`,
  hands install.sh `CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE` /
  `GOOGLE_APPLICATION_CREDENTIALS` / `CLOUDSDK_CORE_PROJECT`, records what it
  did in the persisted deploy log, and deletes the staging dir afterwards.
- **install.sh accepts credential-file auth for GCP.** The `--deploy`
  pre-flight and both interactive GCP checks prove a supplied key with a real
  token exchange instead of `gcloud auth list` (which cannot see the
  override); a key that does not authenticate is reported as the cause rather
  than answered with a device-code login prompt; `GOOGLE_APPLICATION_CREDENTIALS`
  alone is promoted to the variable gcloud actually reads; and the project is
  taken from `CLOUDSDK_CORE_PROJECT` / the key's `project_id` before any
  gcloud round-trip.

## [0.28.261] - 2026-08-20

### Fixed

Review follow-ups to the #820/#782 diag-set work — ten findings, the two worst
first:

- **Removing a URL from the watchlist can no longer hard-delete a shared set
  config it failed to classify.** A set config evicted from the 200-newest
  config window had no detail and no list name, was guessed "single-URL", and
  its deletion CASCADE-erased every member's probe history. Classification now
  also reads the run-borne `config_name`, and — the fail-safe inversion — a
  config that cannot be positively classified is KEPT, never deleted.
- **Two different URL sets no longer silently collide into one config.** The
  set config name (the find_or_create reuse key) encoded only the first host
  and member count, so "a.com b.com" and "a.com c.com" reused each other's
  configs and probed the wrong URLs. Set names now carry a membership hash
  over the sorted probe URLs, and the server's `find_or_create` reconciles a
  reused row's endpoint/workload/max-duration toward the request (validated by
  the capability gate like a fresh create).
- Per-member verdicts now truly override run-level status: a watchdog-killed
  set run no longer paints a member red when that member's own attempts all
  succeeded, and a member with no attributed attempts in an attributed run
  renders 'pending' (no evidence), not green.
- Watchlist membership is structural (`test_kind='url_probe'`) with the name
  prefix as legacy fallback — renaming a probe config no longer erases its
  history, and a benchmark config named "Diag: …" is no longer injected.
- Set-host attribution reads `endpoint.hosts` straight off the config LIST
  items (the wire always carried it), killing the ~76-request per-config
  detail fan-out; details are fetched only for configs evicted from the
  200-newest window, which previously attributed to their first member only.
- Hourly monitoring: schedule create is idempotent server-side (an identical
  config+cron+timezone row is returned, not duplicated), the button has an
  in-flight guard and re-enables a paused schedule instead of duplicating it,
  only the hourly cron renders the "Monitoring hourly" badge, and
  pausing a shared set's schedule says it affects every member URL.
- URL-comparison timing medians only pool modes successful on EVERY compared
  URL — protocol support no longer masquerades as latency; excluded modes are
  footnoted, and no winner is crowned when the URLs share no successful mode.
- Watchdog headroom scales with the workload (preset estimate × samples ×
  URLs, floored at 900s, capped at 7200s) — a Full x5 over 8+ URLs no longer
  breaches the flat 1800s cap and dies mid-flight.

---

## [0.28.260] - 2026-08-20

### Fixed

- **GCP endpoint deployments get a real zone too** (#831). deploy.json's GCP
  block now carries the zone resolved through the authenticated listing (shared
  cache with tester creation) instead of hardcoding "<region>-a"; the hardcoded
  form survives only as the documented no-credentials fallback.

## [0.28.259] - 2026-08-20

### Fixed

- **GCP zone is resolved, not assumed** (#829). Tester creation listed the
  region's zones through the authenticated gcloud env and picks the first UP
  zone (cached per project+region), instead of hardcoding "<region>-a" — which
  does not exist in us-east1, the only GCP region in the cost table. Listing
  failures fall back to the old behavior and say so in the error.

## [0.28.258] - 2026-08-20

### Fixed

- **GCP provisioning works from a host that never ran \`gcloud auth\`** (#827).
  The provisioner passed GOOGLE_APPLICATION_CREDENTIALS to the gcloud CLI, which
  does not read it — every GCP create failed with "no active account selected"
  while the account validated as active (the validator authenticates per-call).
  All gcloud invocations (create AND the start/stop/delete/describe lifecycle,
  which ran with no credentials at all) now pass the stored service-account key
  via CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE, stateless per invocation.

## [0.28.257] - 2026-08-20

### Fixed

- **Failed probe attempts show their reason** (#824). Each failed attempt row on
  the run detail page renders its recorded error message (ANSI-stripped,
  category-prefixed) next to the FAIL chip, and a protocol block whose failures
  share a dominant reason says so in the collapsed header ("5 FAIL — QUIC
  handshake timeout") instead of a bare count.

## [0.28.256] - 2026-08-20

### Fixed

- **Install diagnostics persist for post-mortem, and Windows caddy cells stop
  failing TLS-less** (#816). The deployment-events SSE route existed only in the
  retired Rust dashboard — the C# control plane now serves it (persisted-log
  replay for finished deployments, live ring+tail for active ones). The
  deployment row records exit_code and failed_step (V054), the persisted log is
  ANSI-scrubbed and tail-bounded, and failure messages carry the last fatal line
  and dying step. On Windows, the caddy stack declared a hostless TLS site under
  auto_https off and started certificate-less — every handshake failed while TCP
  checks stayed green; the Caddyfile now issues a real internal cert
  (default_sni localhost) and the in-guest verify asserts an actual TLS
  handshake.

## [0.28.255] - 2026-08-20

### Fixed

- **Install budgets age from install start, and infrastructure kills retry** (#817).
  The deploy timeout re-arms with the full scaled budget when install.sh reaches
  its install step, so quota-contended cloud provisioning no longer burns the
  window an AOT publish needs; the watchdog shares the same anchor. Kills during
  a control-plane shutdown and reaps of never-started deployments are marked
  retryable and go through the existing retry machinery instead of failing
  terminally; credential failures stay terminal.

## [0.28.254] - 2026-08-20

### Fixed

- **Multi-URL "Diag set" runs now update every member URL's watched row** (#820).
  Set configs join the probe-page watchlist, per-URL health verdicts come from
  the run's target_url-filtered attempts, and the `?host=` query param no longer
  double-encodes.

### Added

- **URL Probe burst sampling, hourly monitoring, and set comparison** (#782 P2).
  A Samples selector (1/3/5) runs each URL in a burst, watched rows can be
  monitored hourly via the existing scheduler, and set runs get a side-by-side
  per-phase median comparison table on the run detail page.

## [0.28.253] - 2026-08-19

### Fixed

- **The Benchmark Regressions page finally says why it is empty** (#810). An
  empty state now distinguishes "these runs were never compared" (with a
  pointer to how comparisons get produced) from "compared, no regressions
  found", and a run can be pinned as the comparison baseline directly from
  its detail page instead of the baseline being implicit.

## [0.28.252] - 2026-08-19

### Fixed

- **Launch-flow hardening, #793 slices (b)+(d) plus the last #791 item.** The
  wizard's review jump is gated until autoprovision actually completes, cost
  and runner notices state what they will really do (no more silent runner
  reuse surprises), "runner online" now means `power_state == running` AND
  `agent_status == online` everywhere the wizard checks readiness, and the
  cloud-account combobox looks like the control it is. Closes #793 and #791.

## [0.28.251] - 2026-08-19

### Fixed

- **URL Probe no longer 409s on previously-probed hosts** (#812). The probe
  page's find-or-create matched config names against the 200-newest list, so
  a config older than 200 rows (easy after heavy matrix testing) was missed
  and the create hit the UNIQUE(project_id, name) constraint. Config create
  now supports opt-in idempotency (`find_or_create: true` returns the
  existing row, race-free via the unique-violation catch), the config list
  gains an exact `?name=` filter that bypasses the cap, and the probe page
  uses both. Same failure class as the historical canary wedge — now dead
  product-wide.

## [0.28.250] - 2026-08-19

### Added

- **Local C# and Rust SDK demo deployments.** Runnable SDK examples
  (sdk/csharp/Example, sdk/rust/example) with Dockerfiles and local/Azure
  run scripts under examples/, plus an SDK examples panel and refreshed
  create-endpoint dialog on the SDK Endpoints page.

## [0.28.249] - 2026-08-19

### Fixed

- **The deploy wedge-watchdog enforces the SAME scaled budget as the deploy
  runner** (#804). It killed deploys at a flat 30 minutes while #740's scaled
  budget legitimately allowed more (a 1-language cpp deploy had 38m; the
  watchdog always won the race). The watchdog now computes the shared budget
  (+5m slack) per deployment, the reap re-checks the aging basis so a
  concurrent #785 recovery re-claim wins, and the timeout message states the
  enforced budget and language count — with the control-plane-restart hint
  only when a recovery actually occurred.

## [0.28.248] - 2026-08-19

### Added

- **Group-first benchmark results** (#803). Comparison groups render as ONE
  expandable row on the Runs list (live progress `X/N · F failed`, aggregate
  status, fastest-so-far chip) linking to the group's comparison page;
  benchmark wizards land on the comparison page after launch; run detail
  pages of group members gain a group breadcrumb and cell X-of-N prev/next
  navigation; the comparison page gets a completion banner listing failed
  cells with their reasons, and a Delete-group action. Standalone runs and
  individual run URLs are unchanged.

## [0.28.247] - 2026-08-19

### Fixed

- **.NET language installs unbroken across the board** (#801). net9-aot and
  net10-aot were missing from the Linux allowed-language list (instant
  install failure); plain net9 failed whenever a system dotnet existed (the
  SDK-channel check was skipped and the publish error silenced); launches
  now pin DOTNET_ROOT so dotnet-install runtimes resolve. AOT publishes get
  clang/zlib prerequisites on Linux. AOT on Windows is genuinely
  unsupportable (needs VS C++ Build Tools) and is now EXCLUDED at the wizard
  (languageAllowedOnOs both directions) and rejected server-side — no more
  doomed cells.
- **Caddy-on-Windows failures are no longer invisible** (#801 pattern B —
  4/4 systematic in the field). The Windows proxy setup surfaced no remote
  output, never retried Conflicts, and only warned on verify failure; now:
  output captured, Conflict retry, success marker required, ~90s serve
  verify (HTTP + HTTPS) inside install.ps1, UDP 8454 NSG rule for h3, and a
  fatal error with diagnostics instead of a silent 6-minute reachability
  death. One live Windows Caddy cell after deploy pinpoints any remaining
  in-guest cause.

## [0.28.246] - 2026-08-19

### Added

- **Production UI smoke harness** (`scripts/prod-smoke.sh` +
  `dashboard/e2e/prod/`): 11 read-only Playwright specs against the live
  dashboard — runs list + purpose tabs, URL probe, Start-a-test cards, both
  benchmark wizards' gates, comparison pivots, run detail (incl. the failed-
  run error banner), the canary panel, and system versions — each with a
  full-page screenshot and page-error collection. Auth by session-token
  injection (never a password); the runner auto-files a GitHub issue with
  the failing screenshot (secret gist) per failed spec, de-duped against
  open issues. Never mutates: no launches, no dispatches. Setup in
  docs/prod-smoke.md.

### Fixed

- RunsPage's time-filter unit test used hardcoded 2026-08-18 fixtures and
  became a date time-bomb (red on main once the fixture aged past the 24h
  window it asserted); timestamps are now relative to the test clock.

---

## [0.28.245] - 2026-08-19

### Fixed

- **CI: matrix-launch tests seed the active cloud account the #795 launch
  gate requires.** The tests referenced a cloud-account id no fixture ever
  created; once the gate (correctly) started failing cells whose account is
  missing or not active, all four tests failed with zero launched runs —
  blocking every C#-touching PR. The tests now seed an active account,
  matching the real-project contract the gate enforces.

## [0.28.244] - 2026-08-19

### Added

- **Start a test is now an incident-ready triage console.** Tests are grouped
  by the signal an operator needs, ranked without moving under the user during
  background refreshes, and checked against the same runner, endpoint, and
  cloud readiness rules used by the launch flows. Blocked tests lead directly
  to the required repair, recent non-provisioning configurations can be safely
  repeated, and all scenarios are searchable from the command palette. The
  complete flow supports persisted Vim-style keyboard controls with an explicit
  off switch and responsive, WCAG-checked layouts.

---
