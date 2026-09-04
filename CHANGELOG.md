# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [0.28.309] - 2026-09-02

### Fixed

- **Endpoint: UDP replies now leave from the address the request arrived on.**
  The echo (9999), STAMP reflector (9997) and UDP throughput (9998) servers
  bind `0.0.0.0` and answered with a plain `send_to`, so on a multihomed
  target the kernel chose the reply's source address from its routing table.
  Home lab, 2026-09-02: a Raspberry Pi with ethernet and wifi on the same
  subnet — probes sent to the wifi address, echoes came back from the ethernet
  address. The tester's UDP probes use *connected* sockets, which silently
  drop datagrams from any other source, so `udp`, `stamp` and `rpm` reported
  100% loss while `tcp`/`http*`/`http3` against the same host were fine, and
  nothing in either log said why (`tcpdump` did). Stateful firewalls and NATs
  drop such replies for the same reason. New `pktinfo_socket.rs` wraps the
  server sockets with IP_PKTINFO / IP_RECVDSTADDR receive + pinned-source send
  (via `quinn-udp`, already in the tree for HTTP/3; its QUIC-oriented
  don't-fragment and GRO settings are switched back off so a large echo still
  fragments and one receive is one datagram). Falls back to the old behaviour
  with a warning if the platform setup fails. Linux regression tests reproduce
  the bug on loopback (`127.0.0.2` vs `127.0.0.1`). The C# endpoint port
  (`Networker.Endpoint`) has the same `UdpClient` pattern and is NOT changed
  here — .NET exposes no pinned-source send; follow-up.

- **Tester: "Client network" now describes the interface the probes actually
  use.** `NetworkContext` took the default-route interface, so on a
  dual-homed client (ethernet default route, wifi to the lab subnet) the
  report said `iface=enp3s0 (ethernet) gw=172.16.48.1` while every packet
  left over `wlo1`. The interface, kind, MTU and VPN verdict now come from a
  longest-prefix-match route lookup toward the resolved target (Linux
  `/proc/net/route`, macOS `route -n get <ip>`); `gateway_ip` is the next hop
  of THAT route and is empty for an on-link target. Default route stays the
  fallback (IPv6 targets on Linux, Windows). JSON field names are unchanged.
  The "Client network" log line prints `gw=on-link` instead of `gw=?` for a
  gateway-less route.

### Notes

- `.markdownlint.json` (new) sets `MD024: siblings_only` — the standard
  Keep-a-Changelog setting — so the `markdown` lint section stops flagging
  every `### Fixed` heading in this file (86 of its 99 findings).

---
## [0.28.308] - 2026-08-27

### Fixed

- **Actions artifact storage was 138x over the allowance.** Measured: **69.2 GB
  across 4021 artifacts** against the 0.5 GB included, which put the account at
  100% and into billed usage. 97% of it is release build output:

  | artifact | size | copies |
  |---|---|---|
  | `dist-csharp` | 32.6 GB | 276 |
  | `dist-x86_64-unknown-linux-musl` | 9.7 GB | 402 |
  | `dist-x86_64-apple-darwin` | 8.5 GB | 406 |
  | `dist-aarch64-apple-darwin` | 8.1 GB | 407 |
  | `dist-x86_64-pc-windows-msvc` | 7.9 GB | 406 |
  | `coverage-report` | 2.1 GB | 1636 |

  Every `dist-*` upload is a **same-run handoff** — `build-*` uploads it, the
  `release` job downloads it, and nothing reads it again — but none set
  `retention-days`, so all inherited the repo default of **90 days**. They now
  set `retention-days: 1`. `coverage-report` drops from 30 days to 7.

- **Release assets now mirror to Azure Blob, in a storage account of their
  own.** `release.yml` uploads every artifact to
  `alethedashreleases/releases/v{VERSION}/` after publishing the GitHub
  release. The account is separate from `alethedashbackups` so release traffic
  and database backups do not share a blast radius; public blob access is
  **disabled**, HTTPS-only, TLS 1.2 minimum.

  `GET /api/artifacts/{name}` now **302s to a container-scoped, read-only SAS
  URL** instead of streaming from GitHub. Three things improve at once: the
  bytes stop passing through the process that also serves the API and the
  agent WS hubs; the credential on prod becomes a SAS that can read one
  container and nothing else, instead of a GitHub token that can read the whole
  private repo; and artifact distribution stops touching the Actions storage
  quota entirely. With no `?tag=`, the version defaults to the running control
  plane's own build — a VM gets the tester from the release that provisioned
  it, not whatever is newest. The GitHub-token path remains as a fallback.

### Notes

- Self-hosted runners do **not** help here. They eliminate billed *minutes*;
  artifact *storage* is charged the same wherever the job ran. The two limits
  are independent, and only the minutes one is addressed by routing jobs to
  the local CI hosts.
- This stops the growth. It does not reclaim the 69 GB already stored —
  existing artifacts keep their original expiry and have to be deleted
  explicitly.

## [0.28.307] - 2026-08-26

### Fixed

- **auto-tag: the second missing scope.** v0.28.306 added `pull-requests: read`
  so the guard could query the merged PR — which worked, and moved the 403 one
  line down. The very next call,
  `repos/{repo}/commits/{sha}/check-runs` (`ci.yml` ~L1082), needs
  **`checks: read`** on a private repo. Still no tag, still nothing deployed.

  This time the rest of the job was audited rather than assumed: `gh release
  view`, `git push` of the tag, and `gh workflow run release.yml` are covered
  by the `contents: write` / `actions: write` already granted, so this should
  be the last gap.

### Notes

- Five versions are now merged and untagged (0.28.302 through 0.28.306). None
  needs back-filling — the next successful tag comes from `Cargo.toml` and
  carries all of that code.

---

## [0.28.306] - 2026-08-26

### Fixed

- **Auto-tag works again — nothing had deployed since v0.28.301.** The
  `auto-tag` job verifies the merged PR's required checks by querying
  `repository.pullRequest` over GraphQL. On a PRIVATE repo the default
  `GITHUB_TOKEN` cannot read that without `pull-requests: read`, so the query
  failed with `Resource not accessible by integration`, the guard exited 1, and
  **no tag was created for v0.28.302, v0.28.304 or v0.28.305**. While the repo
  was public the same query needed no scope, which is why this only appeared
  after the visibility flip.

  Same shape as the `sync-gist.yml` `permissions: {}` bug: a permissions block
  that was sufficient for a public repo and silently insufficient for a private
  one. `ci.yml` is the only workflow that touches the PR API — audited the rest.

### Notes

- The three merged-but-untagged versions do not need back-filling: the next
  merge tags from `Cargo.toml`, so this release carries all of that code.

---

## [0.28.305] - 2026-08-26

### Added

- **`error_category` on the REST attempts DTO** (`GET /api/v2/test-runs/{id}/attempts`).
  The category was ALWAYS persisted — `ErrorRecord.ErrorCategory`, indexed —
  but `AttemptView` returned only `error_message`, so a UI could read the
  reason a probe failed but could not classify it. **No migration:** the data
  was already there, just never selected.

  Selected under a stable alias as the last column of every query tier and read
  by name, the same discipline `target_url` and `sample_index` use, so the
  positional phase ordinals are untouched on all three tiers. Guarded by a
  table probe: a partially-created tester schema without `ErrorRecord` gets a
  `NULL` literal rather than a failed query. Omitted (not null) for a
  successful attempt and for pre-0.28.305 rows, keeping the wire shape
  additive.

### Fixed

- **The run detail no longer counts a not-offered sample as a failure.** With
  the v0.28.301 HTTP/3 pre-flight, an h3 mode at a target advertising no
  `Alt-Svc: h3=` is recorded `unsupported` — the probe was never run. Without
  the category on the wire the UI could not tell that from a real failure, so
  the aggregate table still read "5 failed" and the point looked like a network
  fault. The samples cell now reports the two separately ("3 failed · 2 not
  offered"), and a point whose every sample was skipped says "not run — this
  target does not offer HTTP/3" in muted text instead of red.

  `SamplePoint.failedCount` now means *samples that actually ran and failed*;
  the new `notOfferedCount` carries the skipped ones. A not-offered sample is
  still unsuccessful and still excluded from the timing stats — nothing is
  counted as a success that was not measured.

  The mode name in that message comes from the canonical `isH3Mode`
  (`shared/http-stacks.json` `h3_modes`), not a guess at the protocol string.

### Notes

- The category reaches the UI by two routes and both are handled: the live
  stream nests it under `error.category`, REST returns it flat as
  `error_category`. Read it through `attemptErrorCategory()` rather than
  either field directly.
- `ErrorRecord.AttemptId` carries no index — the same as the seven phase
  tables the attempts query already joins laterally, so the cost profile is
  unchanged. Indexing the attempt-keyed lookups is a possible follow-up for
  large runs.

## [0.28.304] - 2026-08-26

### Added

- **laghound.com serves the installers and the release binaries.** Now the repo
  is private, its release download URLs 404 for every caller. Rather than hand
  a GitHub token to every machine that needs a binary, the control plane holds
  one and everything else asks it.
  - `GET /install.sh` and `GET /install.ps1` — anonymous by design (this is the
    `curl | bash` URL), served from a copy embedded in the control-plane
    assembly, so the script handed out always matches the control plane that is
    running. No extra deploy plumbing, and no drift.
  - `GET /api/artifacts/{name}` — authenticated with the **agent api-key the
    VM already holds**, streaming the release asset. `RELEASE_ASSET_TOKEN`
    (a token with `contents:read`) must be set on the control plane; without it
    the route answers **503**, never a misleading 404.
  - The asset name is checked against an **allow-list**. The name arrives from
    a VM and must never be able to steer the upstream request.

### Fixed

- **New tester VMs can provision again.** Both cloud-init templates (Linux and
  Windows) fetched from GitHub and had been broken since the repo went private.
  They now fetch from the control plane with `X-Agent-Key`, which also deletes
  the release-tag lookup they used to do.

  **Why not just put a GitHub token in cloud-init:** user-data is readable by
  any process on the VM and, on Azure/AWS/GCP, through the instance metadata
  service — a standard SSRF target. A repo-scoped GitHub token there would
  expose the whole private repo from every ephemeral runner. The agent api-key
  already in that user-data is a different risk class: per-agent, revocable on
  its own, and useless against GitHub.

- **Provisioning and reinstall scripts fetch through an authenticated path.**
  `TesterInstallScripts` built plain `releases/download/` URLs, which 404 on a
  private repo — this broke the tester `/upgrade` path, the agent auto-upgrade,
  and CI's reinstall job. The reinstall script now takes the control plane's
  artifact base plus the agent key and fetches from there; with neither it
  falls back to `gh release download`, which resolves the asset id itself and
  keeps JSON parsing out of the shell. `DownloadBinaryCommand` uses `gh` for
  the same reason.
- **`install.sh --benchmark-server` can fetch the reference APIs again.** It
  cloned the repo anonymously and exited 128 (`Repository not found`) once the
  repo was private. It now prefers `gh`, then a `GH_TOKEN`/`GITHUB_TOKEN`
  clone, then a plain clone, and says exactly what to do when all three fail
  instead of dying on a bare git error.
- **The Gist sync works again.** `sync-gist.yml` declared `permissions: {}`,
  which gives `GITHUB_TOKEN` no scopes. That was harmless while the repo was
  public (`actions/checkout` could clone anonymously) and fatal once it was not
  — `remote: Repository not found` — so the Gist silently stopped tracking
  `install.sh` while still being the only unauthenticated install URL. Now
  `contents: read`.

### Notes

- **`RELEASE_ASSET_TOKEN` must be set on the control plane before new tester
  VMs will provision.** It is the one place a GitHub credential now lives.
- Still on GitHub: `install.sh` / `install.ps1` fetch binaries from release
  URLs when run by hand, and `VersionRefreshService.cs:70` polls
  `releases/latest`. Both now have a served alternative to point at.
- The 13 GitHub-hosted scheduled workflows still bill Actions minutes. Not a
  bulk edit: `ci-hosts-watchdog.yml:49` is marked *"never self-hosted: this is
  what rescues them"*, and `release.yml`'s deploy is kept on GitHub
  infrastructure by `docs/self-hosted-ci.md`'s security model.

---

## [0.28.302] - 2026-08-26

### Fixed

- **The deploy can fetch release assets again now the repo is private.** A
  private repo's `releases/download/` URLs return **404 to every caller** — a
  Bearer token on them does not help; assets are reachable only through the
  API by numeric id. `release.yml`'s deploy step pulled all five artifacts with
  plain `curl -fsSL` on the Azure VM under `set -e`, so the first deploy after
  the visibility flip would have died at the download and rolled back.

  The workflow now resolves the asset ids on the runner (where the workflow
  token already works), fails loudly and by name if the release is missing an
  expected asset, and hands the VM id-addressed API URLs plus a token. The
  deploy job's token is narrowed from the workflow-level `contents: write` to
  a job-scoped **`contents: read`**, since it is embedded in the script that
  runs on the VM and a private repo offers no other way in. It expires with the
  job.

  `curl | tar xz` became download-then-extract at the same time: a failed
  fetch inside a pipe could be swallowed rather than failing the deploy.

### Notes

- **This fixes ONE of four paths broken by going private.** Still broken, each
  needing its own change:
  - `CloudInitScripts.cs` (`:224`, `:239`, `:258` Linux; `:365`, `:386`,
    `:391` Windows) — **every new tester VM fails to provision.** Needs the
    control plane to resolve asset ids and inject a token into the cloud-init,
    so it needs a token on the control plane too.
  - `install.sh` / `install.ps1` — the `curl | bash` bootstrap. The
    `gh release download` fast paths (`install.sh:2947`, `:3639`, `:4194`)
    still work when `gh` is authenticated; the plain-curl fallbacks 404.
  - `VersionRefreshService.cs:70` — polls `releases/latest`, now 404.
- The 13 GitHub-hosted scheduled workflows bill Actions minutes on a private
  repo. Routing them to self-hosted runners is deliberately NOT bulk work:
  `ci-hosts-watchdog.yml:49` is marked *"never self-hosted: this is what
  rescues them"*, and `release.yml`'s deploy is the only job holding prod
  credentials and is kept on GitHub infrastructure by
  `docs/self-hosted-ci.md`'s security model.

---

## [0.28.301] - 2026-08-25

### Added

- **HTTP/3 pre-flight: h3 modes are no longer dispatched at targets that do not
  offer HTTP/3.** A raw URL's h3 support cannot be known statically, so the
  control-plane gate could never decide it: `http3` and `browser3` are
  `requires: any` in `shared/modes.json`, an arbitrary URL has no proxy stack
  for the stack rule, and there is no `/health` self-report for the live rule.
  A "Full" mode set aimed at a third-party URL therefore always enqueued h3,
  and against a host without HTTP/3 every h3 sample failed with a QUIC
  handshake rejection that said nothing about the network.

  The tester now asks the target first. Over the web an origin advertises h3
  with `Alt-Svc: h3=":443"` (RFC 9114 §3.1); without it no client can discover
  h3 at all. This is the same signal `install.sh` already uses to decide
  whether a stack came up with QUIC, and every h3-capable stack the installer
  configures sets the header (nginx :8444, caddy :8454, IIS :8445, and the bare
  networker-endpoint). When the origin answers and advertises no h3, the h3
  modes are recorded as `unsupported` — an unsuccessful sample, never a
  success, but one that does not blame the network — with the reason spelled
  out. The mode list comes from `shared/http-stacks.json` `h3_modes`, the same
  list the control plane gates on, so the two sides cannot drift.

  **It fails open.** Only a target that answered AND advertised no h3 is
  skipped; a timeout, a connection error, or any other uncertainty runs the
  probe exactly as before. A pre-flight must never invent a failure the network
  did not produce. The check costs at most one HTTP/1.1 request per origin per
  run (memoised), and it stays runner-side, so the control plane never fetches
  a user-supplied URL.

- **`ErrorCategory::Unsupported`** (`"unsupported"`) — the target does not offer
  this protocol, so the probe was not run. Distinct from a probe that ran and
  failed.

### Fixed

- **`advertised_alt_svc` is populated instead of always being null.** The field
  existed end to end — Rust struct, DB column, C# API, frontend type — but
  every construction site set it to `None`, so a URL diagnostic never recorded
  what the origin advertised. It is now derived from the response headers the
  protocol probes already capture, the same way `security_headers` is.

### Notes

- The REST attempts DTO (`AttemptView`) carries `error_message` but no error
  category, so the run detail's aggregate table still counts a not-offered
  sample among the failures; the per-attempt reason is visible because the
  message is carried. The category IS persisted (`ErrorRecord.ErrorCategory`,
  indexed), so surfacing it needs a DTO widening plus UI work and no migration.
- An origin can also publish h3 through a DNS HTTPS/SVCB record (RFC 9460) with
  no `Alt-Svc` header. Such a target reads as not-offered here; the cost is the
  previous behaviour (the probe runs and fails), not a wrong measurement.

---

## [0.28.300] - 2026-08-25

### Fixed

- **The run detail page no longer reports successful modes as total failures.**
  The "Median & spread per point" table rendered "no usable sample — every
  sample failed" for any point whose `stats` came back null, but null `stats`
  only means "no usable *metric*" — which is not the same as a failure. On a
  real diagnostic run that made TLSRESUME, BROWSER1 and BROWSER2 read as
  wholly failed while all three were 5/5 successful. The row now distinguishes
  the three cases it was collapsing: every sample failed (unchanged), some
  failed and the rest carried no metric, and — the false one — every sample
  succeeded but the mode reported no metric, which now renders in muted text
  as "no <metric> recorded — all N samples succeeded" rather than in red.
  `SamplePoint` already tracked `failedCount` separately from
  `sampleCount - usableCount` for exactly this reason; only the render
  conflated them.
- **`tlsresume` reports its handshake time again instead of no metric.** The
  frontend metric map mirrors `metrics.rs::primary_metric_value`, where
  `Tls | TlsResume` both read `tls.handshake_duration_ms`, but the TypeScript
  side listed only `tls`. `tlsresume` fell through to the HTTP default
  (`http.total_duration_ms`), which a TLS-only probe never carries, so every
  tlsresume point looked metric-less and hit the bug above. The mode's metric
  label was drifting the same way ("Total ms" instead of "Handshake ms").
- **The under-sampled footer note no longer counts rows that show no number.**
  "N points had fewer than 3 usable samples — those numbers are readings, not
  medians" counted metric-less points, so the caption described empty cells.
- **QUIC handshake rejections are no longer filed as connect failures.**
  `classify_quic_connection_error` inspected the crypto keywords only inside
  `ConnectionError::TransportError`; a CONNECTION_CLOSE sent by the peer
  arrives as `ConnectionError::ConnectionClosed` and fell through a catch-all
  `_` arm to `ErrorCategory::Tcp`. So an attempt whose own message read
  "aborted by peer: the cryptographic handshake failed: error 80" was stored
  as a TCP connect failure. This is the every-run failure mode for a target
  that does not serve HTTP/3 — the peer answers the Initial, then aborts the
  handshake with CRYPTO_ERROR — so the misfiling hit every h3 probe against
  such a host. The classifier now reads the numeric transport error code
  (`0x0100-0x01ff` = a TLS alert, RFC 9000 §20.1) rather than the rendered
  text, maps a peer close from the HTTP/3 layer (`ApplicationClosed`) to
  `Http`, and spells out every `ConnectionError` variant so a future addition
  cannot silently inherit the `Tcp` default the way this one did.
- **`pageload3` misfiled the same rejection, harder.** Both of its QUIC
  handshake sites (cold `run_pageload3_probe`, warm `warmup_pageload3`)
  hardcoded `ErrorCategory::Tcp` for every handshake failure with no
  classification at all. They reach the same peer over the same handshake as
  the h3 runner, so they now share its classifier rather than keeping a second
  answer. The synchronous `endpoint.connect()` sites are unchanged: those
  return `ConnectError` (a local/config failure), not a handshake result.

### Notes

- Browser modes still show "no load ms recorded" for completed runs loaded over
  REST: `AttemptView` carries no `browser` block and no browser phase table
  exists, so the metric is genuinely absent rather than lost in the UI. The
  page now says so honestly. Surfacing a real median there needs a schema
  migration plus a DTO widening, which is out of scope here.

---

## [0.28.299]

### Added

- **A comparison axis on the URL comparison page — URLs or Runners.** The
  backend gained `group_by=runner` in v0.28.298; the page can now use it. Same
  measurements, different question: which *site* is faster, or which *vantage
  point* is. Runner mode takes exactly one URL and makes every runner that
  probed it a contestant; picking more than one shows the reason rather than
  firing a request the server rejects by design.

- **Hide URLs from the comparison picker, reversibly.** An × on each entry
  removes it from the list, and "Show hidden (N)" brings any of them back. The
  hidden list lives in `project.settings`, so it follows the project rather than
  the browser, and is shared by everyone working in it.

  **Hiding never touches probe data.** A hidden URL keeps its full history and
  still reports when compared explicitly — so hiding cannot blank a comparison
  someone has open, and a URL hidden today can be restored next month with its
  measurements intact. Deleting the attempts would also silently rewrite every
  historical report that covered them. `GET` gained `include_hidden`, and the
  report returns the hidden URLs that still have data so the page can offer them
  back instead of pretending they never existed.

  The write is a whole-list `PUT`: the client always holds the full set, so a
  replace is idempotent and two operators toggling at once cannot interleave
  into a state neither asked for. It merges into `project.settings` rather than
  overwriting, since that key is a shared bag.

---

## [0.28.298]

### Fixed

- **The URL comparison report pooled every runner into one series.** It grouped
  by URL alone, so samples from different vantage points were averaged together
  and presented as a property of the site. Measured on production 2026-08-24:
  `www.microsoft.com`'s median came from 349 attempts across two runners whose
  own medians were **35 ms** (azure/eastus) and **76 ms** (gcp/us-east1) — with
  **321 of the 349 from the slower one**, so the published number described the
  runner, not the URL. Every URL in that project already had two runners' data
  pooled this way.

  This is the same trap the bucket rule exists to prevent ("a window-wide
  aggregate would reward a URL for being probed at quiet hours"), one dimension
  over. The query now carries the runner, and every bucket reports
  `runner_count` and the contributing `runners`.

### Added

- **`group_by=runner` on the comparison report** — races vantage points against
  each other for a single URL, answering "is the site slow, or is it slow *from
  here*". `group_by=url` remains the default, so existing callers are unchanged.

  Runner mode **refuses more than one URL** rather than averaging across sites:
  pooling two different URLs into one per-runner series would recreate exactly
  the blending this mode exists to fix, rotated ninety degrees.

  Covered by tests against a real PostgreSQL, because the changed logic lives
  entirely in SQL — the runner join, the `$6` series key and the appended
  columns are invisible to unit tests over `ProbeComparisonLogic`, which start
  from rows the query already produced.

---

## [0.28.297]

### Fixed

- **Every runner in the list showed "v?" while the drawer knew its version.**
  `src/api/testers.ts` declares `installer_version` on the list row type and
  `TesterRegionGroup` renders `v{installer_version ?? '?'}`, but the field lived
  only on the DETAIL DTO — so the Infrastructure list reported every runner as
  un-versioned while opening the drawer for the same row showed the real number
  (production had 0.28.293 and 0.28.259 stored the whole time). `ToListDto` now
  carries `installer_version` and `last_installed_at`.

  TypeScript could not catch this: the type promised a field the server never
  sent, and nothing validates JSON at that boundary. The regression test is
  server-side for that reason, and it distinguishes *present-and-null* (a runner
  genuinely never installed, where "v?" is correct) from *absent*, which is what
  made every row look un-versioned.

---

## [0.28.296]

### Fixed

- **Every schedule displayed as "Unnamed".** The schedules API never returned
  `config_name`, and the page renders `config_name || 'Unnamed'` — so on every
  deployment, every schedule showed the same placeholder and no operator could
  tell what one actually runs. The field was simply absent from the payload,
  which is why staring at the UI never explained it. The list and detail routes
  now project it, and create/patch resolve it too so a row does not read
  "Unnamed" until the next refresh.

- **Polled pages issued every request twice on mount.** `usePolling` fires an
  immediate tick, and five pages paired it with their own `useAsyncEffect`
  initial load — one production Infrastructure page load fired
  `testers`, `deployments`, `vm-history` and `cloud-accounts` **two times each**.
  `usePolling` takes an `{ immediate: false }` option and those pages opt out.

  They opt out rather than dropping their own load because their poll is
  deliberately *silent* (it never re-raises the loading flag); making it the
  first load would have cost the page its initial spinner. The suppression
  applies to the mount tick **only** — a `resetKey` bump (the Refresh button)
  and an `enabled` flip (un-pausing) still fire at once, or those controls
  would appear dead.

- **Settings → General claimed "No cloud accounts configured" while three were
  configured.** Two different things were both labelled "cloud accounts": the
  General tab lists identity-federated `CloudConnection` rows
  (`/cloud-connections`, genuinely zero), while the Cloud tab lists
  credential-based `CloudAccount` rows (`/cloud-accounts`, three). The General
  section is now labelled "cloud connections", says what it is, and links to the
  Cloud tab for accounts.

---

## [0.28.295]

### Fixed

- **A runner could sit in `starting` forever, and that silently disabled
  stuck-run reaping for the whole deployment.** `starting` has no self-imposed
  exit — only an agent heartbeat promotes it to `running` — so a VM whose agent
  never connects stayed `starting` indefinitely. Beyond the misleading badge, it
  could never be started by hand (that path requires `stopped`), and
  `WatchdogService` read *any* tester in `starting` as "a wake is in flight" and
  held off reaping stuck queued runs. One stuck runner therefore switched off
  that safety net globally.

  Two independent bounds now: the watchdog only counts a wake as in flight for
  15 minutes, and `AutoShutdownService` releases a runner stuck in `starting`
  for over 20 minutes back to `stopped` with a message saying why. Released to
  `stopped` rather than `error` deliberately — that is the state a manual start
  accepts, the wake arm can retry it in the same sweep, and a late agent
  heartbeat still promotes it to `running`; `error` would turn a transient
  failure into a runner nobody can start without operator help.

- **A woken runner showed "starting" beside "auto-shutdown completed".** The
  auto-wake path set `power_state` without touching `status_message`, leaving
  the previous lifecycle's text in place so the two halves of the badge
  contradicted each other. The message now moves with the state — and carries
  the wake reason — on both the claim and the roll-back-on-failure path. The
  manual start path already did this.

- **The URL comparison report now says which URL fell short, and by how much.**
  Comparing two URLs that had never been probed comparably rendered as a single
  series with only a faint note underneath. The report always knew why; the page
  whispered it. The withheld-ranking message now names the excluded URL, its
  sample count, its bucket coverage and the threshold it missed, states which
  URL it would have raced, and points at the two things that actually fix it —
  probing on the same schedule *and* in the same modes, since each mode is
  ranked separately.

---

## [0.28.294]

### Added

- **Secret age panel (System → Secrets) and an operator rotation script.**
  On 2026-08-24 a storage-account key was found in plaintext in a world-readable
  script on the prod VM and nobody could say how old it was, because nothing
  recorded rotations — the same shape as the backup gap found the same day: the
  information needed to notice existed nowhere.

  The work is deliberately split. **Visibility** is in the UI
  (`GET /api/admin/secrets`, platform-admin only): each secret's age, policy and
  status (`never` → `ok` → `due` → `overdue`). **Action** is in
  `scripts/rotate-secrets.sh`, run by an operator. There is no rotate button:
  the control plane is internet-facing, so an endpoint that can rotate turns any
  single compromise — XSS, an auth bypass, a stolen operator token — into total
  credential compromise, and rotation is far too rare for that trade to pay.

  **The API never returns secret material** — not a value, not a hash, not a
  prefix. A test boots the host with a sentinel signing key and fails if that
  string ever appears in the response.

  The inventory is static in code rather than derived from the rotation table, so
  a secret nobody has ever rotated still appears — as `never`, counted under
  "needs attention". That is the case that matters most, and a design keyed off
  the table alone would hide exactly it.

  `DASHBOARD_CREDENTIAL_KEY` is marked **not automated** and the script refuses
  it with exit 2. It is a data-encryption key, not a password: replacing it
  without re-encrypting makes every stored cloud credential permanently
  unreadable, with no error at the moment of damage. A test asserts the flag
  stays `false`, so it cannot be flipped quietly. Procedure and reasoning:
  `docs/secret-rotation.md`.

  New migration **V055** adds `secret_rotation`, which stores no secret material
  — only which secret was rotated, when, and by whom.

---

## [0.28.293]

### Fixed

- **`install.sh` no longer carries its own copy of the tester probe schema**,
  and the control plane repairs the damage the old copy did. The probe tables
  (`TestRun`/`RequestAttempt`/…) are generated from the Rust tester crate and
  shipped as `shared/tester-schema.postgres.sql`; `install.sh` inlined a fourth,
  hand-maintained copy that had drifted two migrations behind (no `SampleIndex`,
  no `IX_Attempt_StartedAt`) and — the real damage — seeded
  `_schema_versions(version INTEGER)` with the row `1`, where both genuine
  writers use `VARCHAR` and rows `'V001'`.. .

  On any host installed that way the control plane's bookkeeping INSERT raised
  22P02 (`invalid input syntax for type integer: "V001"`), the catch latched
  `_schemaState = -1`, and the process **silently stopped ensuring the tester
  schema altogether** — degrading every streamed attempt for its lifetime. The
  138-line copy is deleted, the control plane now applies the canonical schema
  at startup (not only on first ingest, which is what made deleting it safe),

  Applying at startup matters on its own: v0.28.292 shipped tester migration
  V008 (the `requestattempt(startedat)` index) and deployed to production, yet
  production still showed `_schema_versions` at V007 and a Parallel Seq Scan —
  because nothing applies the tester schema until an attempt happens to stream
  in. A shipped migration that waits for traffic is a migration that has not
  shipped.

  and `RepairVersionBookkeepingAsync` drops an incorrectly-typed bookkeeping
  table so existing installs heal themselves. A correctly-typed table carrying
  real migration history is explicitly left alone.

---


## [0.28.292]

### Fixed

- **Production had no usable backup for five months.** The prod VM's ad-hoc
  `daily-backup.sh` ran `pg_dump alethedash` — the *retired Rust dashboard's*
  database, abandoned at the C# cutover on 2026-03-31. That database still
  exists, so `pg_dump` exited 0 and the job looked healthy every night (4.1 MB,
  69 blobs, "Backup complete" in the log) while `alethedash_core`, the database
  production actually uses, was never captured. A verified 40 MB dump of the
  live database has been taken and uploaded, and `scripts/backup-daily.sh` was
  rewritten so it cannot recur: the database name is read from the live service
  config instead of hardcoded, every database on the server is captured, and a
  dump is only accepted once `pg_restore -l` lists it and its TABLE DATA entries
  cover every table the live database has. See `docs/backup-and-retention.md`.

- **`requestattempt` had no index on `startedat`** (tester migration V008).
  It is the largest table in a live deployment (159 MB / 89k rows in prod) and
  every time-scoped read filters on that column — including the URL comparison
  report — but the only indexes were `(Protocol, Success)` and
  `(RunId, SequenceNum)`. `EXPLAIN ANALYZE` on production confirmed a Parallel
  Seq Scan discarding 26,760 rows per worker to keep 2,872. Sibling tables
  already carried the equivalent index; this one was simply missed.

- **The EF model declared two PostgreSQL extensions that do not exist in
  production.** `HasPostgresExtension("timescaledb")` and `timescaledb_toolkit`
  were inherited from the local dev compose file — the only environment that
  ever had them. Prod runs a stock Ubuntu `postgresql-16` where neither is
  installed *or available*. Nothing was broken yet, but EF writes the
  declaration into the next scaffolded migration as a `CREATE EXTENSION`, which
  would have failed on deploy.

### Changed

- **Local control-plane dev now runs plain `postgres:16-alpine`**, matching
  production, the lab, the SQL-test compose file and the C# Testcontainers
  suites. It was the sole environment running `timescale/timescaledb-ha`, which
  is how `time_bucket` nearly shipped into the comparison report: it would have
  worked on every developer machine and thrown in production.

- **Local dumps are now readable by the `postgres` user.** They were written
  root-owned into a `0700` root directory, so `pg_restore` — which runs as
  postgres — failed every restore at directory traversal with "could not open
  input file: Permission denied". The archives were fine; the permissions were
  not. Found while restore-verifying the legacy databases before dropping them,
  which is exactly why a dump you have never read back is only a hypothesis.

- **The production database is renamed `alethedash_core` → `networker_core`**,
  and the two dead Rust-era databases (`alethedash`, `alethedash_logs`) were
  dropped after their dumps were restore-verified (51/51 and 3/3 tables),
  reclaiming 273 MB. `alethedash` was the decoy that made the five-month backup
  gap invisible — a plausible-looking database beside the real one. The name is
  now tied to the codebase rather than a domain, so a URL or brand change no
  longer strands it. `soak-check.yml` and two runbooks are updated; the backup
  and prune scripts needed no change because they read the name from the live
  service config.

### Added

- `scripts/prune-retention.sh` — ages out probe attempts and logs
  (`RETENTION_RAW_DAYS`/`RETENTION_LOG_DAYS`, default 90 days), **dry-run by
  default**, designed to be chained after a successful backup. Deletes bottom-up
  in one transaction because the live database has `NO ACTION` where the
  canonical DDL says `CASCADE` — a naive parent delete fails with a foreign-key
  violation.
- `docs/backup-and-retention.md` — layout, retention, restore drill and the
  monitoring signal (alert if `last_backup.json` is older than 48 hours).

---

## [0.28.291]

### Fixed

- **The VM/IP teardown sweep no longer starves the oldest deployments.**
  `TeardownFinishedRunsAsync` drew its per-tick batch of finished
  auto-provisioned runs with a bare `Take(25)` and no `ORDER BY`, so which 25 of
  the eligible candidates a tick saw was left to the query planner. EF Core warns
  about exactly this ("The query uses a row limiting operator ('Skip'/'Take')
  without an 'OrderBy' operator"), and the warning surfaced in production the
  moment the `service_log` sink was enabled in v0.28.290.

  The consequence was worse than unpredictable paging. The batch is the reaper's
  entire budget for the tick and several of its branches skip a candidate without
  tearing anything down (the `FailedReleaseAllowance` hold; an endpoint still
  referenced by an active run), so a tick can spend all 25 slots on deferrals.
  Because the planner's row order is stable while the heap is unchanged, the same
  deferrable rows could come back ahead of a tearable one every 5-second tick —
  leaving a cloud VM and its public IP billing indefinitely. That is the leak this
  method exists to prevent (see its docstring: ten B2s VMs leaked per launch,
  2026-08-01).

  The batch is now ordered oldest-finish-first, with the run id as a tiebreak for
  a total order. That is both deterministic and the correct priority: the
  longest-idle deployment is the one that has been costing the most.

---

## [0.28.290] - 2026-08-24

### Added

- **The control plane can persist its own logs again (`service_log`).** The Logs
  tab reads a table the **Rust** `networker-log` crate creates and writes, and
  only the tester and endpoint still depend on that crate. The Rust→C#
  migration ported the READ half — `LogsEndpoints` is an explicit port of the
  Rust `api/logs.rs` — and never the write half, so on a C#-only control plane
  `/api/logs` always answered `log_sink: "unconfigured"` and the UI honestly
  said log persistence was not configured. This is the writer.
  - **Opt-in**: `DASHBOARD_LOG_SINK=1`. Turning it on makes every qualifying log
    line a database write — a real cost and a real disk-growth decision for an
    existing deployment — so it is never enabled implicitly. Level floor,
    service name, queue/batch sizes and retention are all overridable.
  - Creates the table with the **same DDL as the Rust crate** (and the same
    optional TimescaleDB hypertable + retention), so a database the tester or
    endpoint already provisioned is untouched and every writer agrees on the
    schema and the `Error=1 … Trace=5` level encoding.
  - **A log call never blocks and never throws**: a bounded queue producers
    only ever `TryWrite` to, drained by one background loop that COPYs batches.
    Overflow is dropped and counted rather than awaited — a control plane that
    stalls request threads because the log database is slow has turned
    observability into an outage. The writer never logs through `ILogger`
    either, or a database failure would produce an error per failed flush,
    forever.
  - `/api/logs/pipeline-status` reports the **real** counters it had been
    returning as hard-coded zeros with a `TODO(phase3)`, and distinguishes
    `unconfigured` from `degraded` — an operator staring at an empty Logs tab
    needs to know "nothing logged" from "writes failing".
  - Failure to start is **not fatal**: a control plane that refuses to serve
    traffic because it could not create a logging table would trade a real
    outage for a cosmetic one.

---

## [0.28.289] - 2026-08-24

### Added

- **The perf log says whether the network time was the payload.** Every row read
  the same before this — "~40 ms network" — with no way to tell a slow link from
  a big response, and those have opposite fixes. The live API panel now carries
  **Size** and **Transfer**: bytes received (wire size, compressed if the
  transfer was), and `responseEnd - responseStart`, the time the body was
  actually arriving. Hover a row for the split in words.
  - **Transfer is measured, not derived.** The obvious column would have been a
    rate — bytes over network time — and it would have been wrong: the network
    leg is dominated by round-trip latency, so that division yields a figure in
    bandwidth units that is not bandwidth. Measured against prod, a 234-byte
    response spends **~0.09 ms** transferring out of a ~40 ms leg, which a rate
    column would print as "48 kbps" and every reader would take for a slow link.
  - No column was given up for this: Method and Status stay. The panel widens
    (760 → 900 px at `lg`) and the clock drops to 24-hour so nothing wraps.

### Changed

- **The perf-log page leads with p95 instead of the average.** Both numbers were
  already in the stats response, but the average was the headline and p95 the
  footnote — which is backwards: an average is the one statistic that cannot
  show a latency problem, because the many fast polls drag it away from the tail
  anyone complaining is actually feeling. p95 is now the tile value, the average
  its sub-line, and the slow/janky counts carry a percentage.
- **"Slowest API Paths" is now "Where the time goes", ranked by total time
  contributed** rather than by average. Ranking by average puts a rarely-called
  report above an endpoint polled three hundred times, and the second is almost
  always what there is to fix. The total was already being accumulated and
  simply was not shown or sorted on; it now has a column.

---

## [0.28.288] - 2026-08-23

### Changed

- **CA1416 platform warnings cleared in the CLI-provisioner test fixtures.**
  `File.SetUnixFileMode` is unsupported on Windows, and the fake-CLI helpers in
  `CliProvisionerDeleteCascadeTests`, `CliProvisionerGcloudEnvTests`,
  `CliProvisionerGcpZoneTests` and `CliProvisionerGcpTeardownTests` called it
  unguarded. Every test reaching those helpers already returns early on Windows,
  so nothing ever threw — but the analyzer cannot see a caller-side guard and
  raised five CA1416 warnings in the `dotnet-format` lint baseline. The guard now
  lives inside each helper (`if (!OperatingSystem.IsWindows())`), matching the
  idiom already used in `DeployJsonGcpZoneTests` and
  `CliComputeProvisioner.cs`. No behaviour change; the helpers are now safe to
  call from a future test that forgets the caller-side skip.

---

## [0.28.287] - 2026-08-24

### Fixed

- **The prod UI smoke harness was crying wolf: three stale assertions, no prod
  defect.** A post-deploy run filed #867/#868/#869; all three were the specs
  drifting behind the product, and each would have failed on a perfectly healthy
  deployment.
  - `runs`: asserted the raw `completed`, but the badge renders
    `runDisplayStatus()` — a completed run with **any** failed attempt reads
    `partial`. Prod's newest run is `ok=3 fail=1`, so it fails on the normal
    case, not an edge case.
  - `scenarios`: demanded `>= 4` action links on one view and matched them by
    the text `Configure`. The page became a **tabbed, readiness-aware triage
    console** in #799 — only the selected tab is in the DOM, and the label is
    `availability.actionLabel` ("View runners →" when no runner is online).
  - `scenarios`: assumed a URL scenario always configures on `/probe`. With no
    online runner it deliberately routes to `/vms` instead, because there is
    nothing to probe from. The spec now asserts **both** branches, which is
    stronger than the original's single state.

### Added

- **A full production route sweep** (`e2e/prod/allroutes.prod.spec.ts`): all 35
  authenticated routes must render their own shell — no error boundary, no blank
  page, no exception during mount. The other prod specs assert deep content on a
  handful of surfaces; "does every page still work after a release" is a
  different question, and a route that 404s its lazy chunk or loses its router
  entry fails here and nowhere else. Includes the new `/probe/compare` (#782 P3).
  **35/35 green against laghound.com on v0.28.286.**

---

## [0.28.286] - 2026-08-24

### Fixed

- **A containerised CI host no longer fails the two jobs that need a real
  machine.** `ci-turing-1` (added in .282) took every self-hosted Linux job,
  including two it physically cannot run: `Measurement accuracy (netem ground
  truth)` needs `tc netem` (NET_ADMIN) and `Reinstall script execution` installs
  and starts a **systemd unit** — a container has no init system at all. Routed
  there they fail on the *host*, not on the change, which is worse than not
  having the host: it cost #862 two red runs before the cause was obvious.
  GitHub has no negative label selector, so the split is additive — every
  VM/bare-metal host now also carries **`bare-metal`**, `pick-ci-hosts` grew a
  `linux_bare` output beside `linux`, and exactly those two jobs use it. The
  containerised host keeps the plain triple and takes everything else;
  `linux_bare` falls back to `ubuntu-latest` when no bare-metal host is online,
  like every other routing decision. Adding the label needs no reinstall — edit
  `LABELS=` in `/etc/ci-host/env` and restart the loop.

---

## [0.28.285] - 2026-08-23

### Added

- **The URL comparison report — #782 P3 of 4.** P1 made it possible to probe
  several URLs in one run and P2 gave every point a real median and spread;
  this is what that data was for. `GET
  /api/projects/{id}/reports/probe-comparison` and a new page at
  `/projects/{id}/probe/compare` answer "of the URLs I watch, which is fastest,
  which flakes, which is steadiest" — and refuse to answer when the data cannot
  support it.
  - **Shared time buckets, never window-wide aggregates.** A URL probed only
    overnight would win a raw aggregate by dodging peak hours. Every figure is
    computed over the buckets in which *every* eligible URL was actually
    measured. A URL covering under 30% of the window is excluded and shown
    greyed with its real counts, so one barely-probed URL cannot shrink the
    shared set for the others.
  - **Ranked head-to-head**, not on whose average is lower: "in the 42 hours
    both were probed, A was faster in 31". A bucket where either side had no
    successful sample is not a race and counts in no column.
  - **Coverage honesty.** Below ~30% shared coverage (or fewer than 5 shared
    buckets) the scoreboard is greyed, the crowns disappear, and the report says
    why and what to do — instead of ranking anyway.
  - **Crowns**: Fastest, Most reliable, Most consistent (lowest p95/p50) and the
    per-phase DNS / TCP / TLS / TTFB winners, which are often different URLs. A
    tie awards no crown, and a phase nothing measured never wins one.
  - **Modes are never pooled** — an http1 probe and an http3 probe of the same
    URL are not the same race, so there is one scoreboard per mode.
  - Overlaid p50 time series (lazy-loaded Recharts, so non-chart routes keep
    their bundle), and PDF/HTML/DOCX/Markdown export that carries the coverage
    block and the full methodology with the numbers.
  - Reachable from the watchlist's new **Compare…** action, which normalises the
    selection with the same `toProbeUrl()` the probe launch uses, so its URLs
    match the `target_url` the tester stamps exactly.
  - Bucketing is `floor(epoch / width)` rather than TimescaleDB's `time_bucket`:
    identical results for these fixed epoch-aligned widths, and no extension
    dependency, so the report also works against a plain-PostgreSQL lab or test
    database. Attribution falls back to the tester `TestRun.TargetUrl` for
    pre-v0.28.231 attempts, which is most of the existing single-URL history.
  - Docs: `docs/reports-url-comparison.md`.

---

## [0.28.284] - 2026-08-23

### Fixed

- **A debug-profile `networker-tester.exe` no longer overflows its stack on
  Windows before it probes anything (#853).** `#[tokio::main]` `block_on`s the
  whole async-main state machine **on the thread that calls it** — the process's
  main thread — and on MSVC that thread's stack is the PE header's reserve, the
  linker default of **1 MB**. An unoptimised build of `main` → `run_for_target`
  (both very large async fns) does not fit, so every debug build died with
  `STATUS_STACK_OVERFLOW` (`0xC00000FD`, exit `-1073741571`, "thread 'main' has
  overflowed its stack") before running a single attempt — `--help` included.
  `main` now runs that work on a thread it sizes itself (16 MiB), and the tokio
  runtime is built with a matching `thread_stack_size` because
  `target_runner.rs` spawns futures onto worker threads whose stacks are
  tokio's 2 MiB default — the same trap one level down. A panic is re-raised
  with `resume_unwind`, so the exit code and the message are unchanged.
  16 MiB is a **reserve**, not an allocation (Rust passes
  `STACK_SIZE_PARAM_IS_A_RESERVATION` on Windows; Unix stacks commit lazily), so
  it costs address space rather than memory.
  - The two burst-sampling CLI tests #852 gated off Windows now run there, and
    they are the regression test: they spawn the **debug** binary, which is
    exactly the build that overflowed.
  - Release builds were unaffected and still are — but the margin had been
    invisible, and an optimised build that grew past 1 MB would have failed the
    same way in production with no warning.
  - Verified on the real Windows CI host (`ci-windows-1`, Server 2025, rustc
    1.98): the pre-fix binary reproduces `EXITCODE=-1073741571`; the fixed
    binary runs the probe and returns a normal exit code.

---

## [0.28.283] - 2026-08-23

### Fixed

- **The SDK Endpoints route test fed the page a wrong-shaped payload (#848
  follow-up).** `GET /api/projects/{id}/sdk-endpoints/samples` returns two
  different shapes in one envelope: `catalog.samples[]` are catalog entries
  (`id`, `sdk_version`) and the top-level `samples[]` are per-language STATUS
  rows (`language`, `label`, `current_version`, `recommended_action`, `reason`,
  `reusable`, `route`). The e2e stub added in #848 used the catalog field names
  for the status rows and omitted `catalog`/`cost_preview` entirely, so the
  panel rendered with blank labels, blank versions and no row actions — and
  nothing failed, because the route test only asserted that the page did not
  crash. The stub now mirrors `SampleView.ToWire()` and
  `SdkSampleCatalog.ToWire()` against `shared/sdk-samples.json`, and
  `routes.spec.ts` asserts the values the panel puts on screen (five language
  labels, the version line, `1 of 5 deployed`, one action per row) so the two
  shapes have to keep agreeing. Verified by running the new test against the
  old stub: it fails.

---

## [0.28.282] - 2026-08-23

### Fixed

- **`cargo install cargo-vet` failed on a warm CI host and took main red.** A
  self-hosted host keeps `CARGO_HOME` between jobs, so a `cargo-vet` an earlier
  run left at a different version made a plain `cargo install` fail outright —
  `error: binary cargo-vet already exists in destination`. A GitHub-hosted
  runner never hits it, because its `CARGO_HOME` is new every time. The step now
  installs only when the pinned version is absent and `--force`s past a stale
  one. (`mutation.yml` and `rust-audit.yml` install unpinned tools the same way
  and carry the same latent risk; left alone as neither is failing.)

### Added

- **A CI host can now run containerised, on a machine that is not dedicated
  CI** (`infra/ci-hosts/container/`). The first is `ci-turing-1` on a box that
  also serves nginx :8080, Samba, ollama, an openclaw gateway and
  node_exporter. The ephemeral loop's between-jobs sweep — `docker ps -aq |
  xargs docker rm -f`, `rm -rf /tmp/...`, `pkill -u` — is correct on a
  dedicated VM and a live grenade on a shared machine; inside a container all
  three are harmless, because `/tmp` and the process table are the container's
  own and `docker` talks to a **DinD sidecar** instead of the machine's daemon.
  The host's Docker socket is deliberately not mounted and the entrypoint
  *refuses to start* without `DOCKER_HOST` rather than falling back to one.
  Nothing is published: a runner is outbound-only, so it cannot collide with
  what the machine already serves.
  - The image runs the **real** `install-ci-host.sh --container`, so a
    containerised host and a VM host get their toolchains from the same code.
    `--container` skips only what assumes ownership of a machine: systemd, the
    local `dockerd`, and qemu-guest-agent.
  - The PAT is a mounted file — never a build arg, an environment variable or a
    layer.
  - Verified on the machine itself: the container sees an empty Docker world,
    and running the loop's full sweep inside it left the machine's own
    container and all six of its images untouched.
  - Two DinD traps found by a real job landing there, both fixed: a **bind
    mount is resolved by the daemon**, so `/tmp` and the runner's `_work` are
    now shared with the sidecar at identical paths (otherwise the daemon
    invents an empty directory and the container dies on missing data); and a
    **published port lands in the daemon's namespace**, so the pair now shares
    one network namespace (`network_mode: service:dind`) and
    `docker run -p X` + `curl localhost:X` works the way every workflow
    assumes.

---

## [0.28.281] - 2026-08-23

### Fixed

- **`validate-bench-apis` could validate the wrong server and report it green.**
  Every job in the 8-language matrix booted its reference API as
  `docker run --name bench-srv -p 8443:8443` with certs staged in `/tmp/bench`.
  On a self-hosted CI host — persistent, and running several of those languages
  at once — that collided three ways: the second `docker run` failed on the
  name, the second publish failed on the port, one job's
  `docker rm -f bench-srv` cleanup killed **another** job's container, and two
  jobs overwrote each other's TLS key while a container was reading it. The
  worst outcome was not a crash: whoever bound `:8443` first answered
  `/health`, so a job could validate a **different language's** server and pass.
  Each job now uses a container name unique per run and language, a cert/data
  directory beside it, and an **ephemeral** published port
  (`-p 127.0.0.1:0:8443`, read back with `docker port`) — Docker allocates and
  binds atomically, which no "find a free port, then bind it" helper can do
  without a race. Cleanup removes only that job's own container and directory.
- **The canonical Rust baseline collided with the same port.**
  `run-validation.sh --rust-only` hardcoded `:8443`/`:8480` — exactly what the
  language containers published. The ports are now overridable
  (`BENCH_VALIDATE_RUST_HTTPS_PORT` / `BENCH_VALIDATE_RUST_HTTP_PORT`,
  defaulting to today's values so nothing else changes), and the workflow takes
  them from the ephemeral range so the baseline and a language job can share a
  host.

These are the prerequisite for giving a CI host a second runner slot: until
now, two jobs on one host could silently produce a wrong green.

---

## [0.28.280] - 2026-08-23

### Fixed

- **GCP testers actually shut themselves down again — an idle VM billed for
  three days because every auto-shutdown tick ran `gcloud` with no credentials
  (#857).** The tester-lifecycle credential resolution built
  `ProviderCredentials` from the `cloud_connection` config alone and never
  loaded or decrypted the cloud ACCOUNT's `json_key`. That is enough for Azure
  (its scope lives in the connection config, and `az` has ambient auth), but
  gcloud authenticates ONLY from its own config store or the per-invocation
  `CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE` (#827) — so on prod every
  `gcloud compute instances stop` failed with *"You do not currently have an
  active account selected"*, `power_state` rolled back to `running`, and the
  next tick failed identically. Forever. The same defect class as #833/#834
  (install.sh spawned without GCP creds) and #838/#839 (Azure-only teardown);
  those fixed the deployment paths, this one closes the tester lifecycle.
  Credential resolution now lives in ONE place
  (`Provisioning/TesterLifecycleCredentials`) shared by the auto-shutdown
  deallocate, the auto-wake start, the manual start/stop/force-stop/delete/probe
  endpoints and the agent auto-upgrade, and it resolves + decrypts the GCP
  service-account key through the SAME `GcpInstallerCredentials` account lookup
  the install.sh staging, the endpoint teardown and the inventory scan use.
  Azure/AWS resolution is unchanged, a host with no resolvable key still
  degrades to ambient auth with a warning naming the reason, a missing CLI
  (`ExitCode == null`) remains a soft success so credential-less hosts converge
  to `stopped`, and `status_message` keeps quoting the CLI's own error without
  ever carrying key material.

---

## [0.28.278] - 2026-08-20

### Added

- **Burst sampling: every sample is published, with a `sample_index`
  (#782 P2).** One probe of a URL was one number, and one cold DNS cache or one
  TCP retransmit was enough to decide what that number said. The tester now
  takes N back-to-back samples of each logical attempt (`--samples N`, default
  1) and publishes **all** of them, so a point has a real median and a real
  spread instead of a single reading.
  - `published_logical_attempts` (`crates/networker-tester/src/dispatch.rs`)
    used to keep only the LAST attempt of a logical attempt. It now collapses
    *within* a sample and keeps *across* samples: a **retry** replaces a failed
    try of one sample (so "success" for a logical attempt is unchanged and a
    retried sample still counts once), a **sample** is an intentional repeat
    that keeps its own row. Failed samples stay in the output as failed
    samples — never dropped, never synthesised.
  - New `RequestAttempt.sample_index` (Rust `metrics.rs`, tester JSON, live
    attempt stream, `AttemptView` on `GET /test-runs/{id}/attempts`, and the
    frontend `Attempt`/`LiveAttempt` types). Orthogonal to `retry_count`.
  - Tester schema **V007**: `RequestAttempt.SampleIndex INT NOT NULL DEFAULT 0`
    (`shared/tester-schema.postgres.sql`, applied by the tester's own
    `migrate()` and by the control plane's lazy bootstrap). `0` is not a
    guess — burst sampling did not exist before this migration, so every
    historical row IS the first and only sample of its logical attempt.
    PostgreSQL 11+ stores the default in the catalog, so the ALTER is
    metadata-only.
  - Workload gains `samples` (`workload.samples` → agent `--samples`), added to
    the tester command line only when > 1 so a default workload spawns a
    byte-identical command line to the pre-#782 one. The URL Probe's Samples
    selector now drives it instead of `runs`.
- **Run detail reports the median and the spread (#782 P2).** New
  "burst sampling — median & spread per point" section: one row per
  (URL × mode × payload) with the median as the headline and p95 / min / max /
  p95-over-p50 jitter beside it. Honest by construction — the usable/total
  sample counts and failed-sample count are shown, and a point resting on fewer
  than three usable samples is labelled `(1 sample)` with no p95 rather than
  being presented as a median.

### Changed

- The agent's fallback invocation deadline (used when a config carries no
  `max_duration_secs`) now multiplies by `samples`; a x5 workload would
  otherwise have been killed mid-flight.
- `insert_request_attempt` (postgres backend) builds its optional-column list
  dynamically instead of enumerating every on/off combination — three optional
  columns (`TargetUrl`, `SampleIndex`, `extra_json`) would have been eight
  hand-written statements. Each still degrades independently on a legacy
  schema via the savepoint retry.

### Notes

- The two new integration tests that SPAWN the tester binary are
  `#[cfg(not(windows))]`: on Windows a debug-profile `networker-tester.exe`
  dies with `STATUS_STACK_OVERFLOW` before probing anything — including on an
  invocation with no `--samples` at all, so it is a property of the debug build
  and not of burst sampling. Tracked as #853; release builds are unaffected.

---

## [0.28.277] - 2026-08-20

### Added

- **URL sets, phase 1 of #782 — assemble a set, probe it in ONE run.** The
  set config shape (`endpoint.hosts[]`), its one-run dispatch and the
  per-URL run detail already shipped with #820/#821/#826; what was missing was
  a way to *build* a set from the watchlist and a run list that admits a set
  row covers several targets.
  - **Watchlist multi-select** (`dashboard/src/pages/DiagnosticsPage.tsx`): a
    checkbox per watched-URL row, a select-all scoped to the visible page, and
    an action bar with **Probe set now** / **Edit as list** / **Clear**. One
    host owning several runner rows (under the provider / capacity grouping)
    contributes ONE member, not one per row.
  - **Multi-URL entry**: a `URL set` / `Single URL` toggle swaps the
    single-line field for a paste-friendly textarea (one URL per line;
    Ctrl/Cmd+Enter launches). It reports what it will not probe — unusable
    lines, duplicates that collapse, entries past the 25-URL cap — before the
    run rather than as failed attempts after it, and the run button is
    disabled when nothing in the box is probeable.
  - **Runs list** (`dashboard/src/pages/RunsPage.tsx`): a set run reads
    `set (4 URLs) · example.com (Quick)` instead of the raw
    `Diag set: example.com +3 [a1b2c3] (Quick)`, which showed a single host
    plus an internal reuse hash and was indistinguishable from an ordinary
    single-URL probe.
  - New pure module `dashboard/src/lib/probe-set.ts` (set parsing, validation,
    de-duplication, selection → hosts) with `probe-set.test.ts`.

### Fixed

- **URL-set members are de-duplicated by resolved probe URL, not raw text**
  (`dashboard/src/lib/diag-request.ts`). `example.com` and
  `https://example.com/` are one probe; both used to survive into
  `endpoint.hosts[]`, so the run carried two `--target` flags for one URL, the
  config name's `+N` overstated the membership, and the run's per-URL grouping
  reported one URL with double the attempts.
- **The control plane now canonicalizes `endpoint.hosts[]` on create and
  PATCH** (`TestConfigEndpointNormalizer`): blanks dropped, duplicates
  collapsed, `host` realigned to `hosts[0]` (a `host` absent from the list is
  prepended, never discarded), non-string members and empty sets rejected with
  a 400, and a 25-member cap so a set cannot become a run the watchdog kills
  halfway. Classic single-host configs and every non-network endpoint round-trip
  byte-identically.
- **The agent de-duplicates targets after resolution** (`RunExecutor`):
  `bare.example` and `https://bare.example/health` resolve to one URL and now
  yield one `--target`.
- **A `Diag set:` run without `test_kind` is classified as a URL probe.** The
  runs-list fallback tested for a literal `Diag: ` prefix, which no set name
  ever matches, so old set runs fell through to `network` and vanished from the
  URL-probes tab.
## [0.28.276] - 2026-08-20

### Added

- **The cloud inventory scan is real** (`GET /api/projects/{id}/inventory`, the
  Settings page's "scan all providers" button). It was a stub that always
  answered `{vms: [], errors: []}` behind a `TODO(phase3)` whose reason — "the
  `az`/`aws`/`gcloud` CLIs are not available in the C# ControlPlane" — stopped
  being true a long time ago: the control plane provisions, reaps and validates
  production VMs through those same CLIs. Clicking the button did nothing
  visible, so it read as dead. (Closes the fidelity audit's F27.)

  `Provisioning/CloudInventoryScanner.cs` now enumerates every **active** cloud
  account on the project **in parallel**, each authenticated with its own stored
  credentials the way the rest of the control plane does it — Azure signs the
  service principal into an isolated `AZURE_CONFIG_DIR`, GCP goes through
  `CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE` pointing at a 0600 key file in a 0700
  throwaway `CLOUDSDK_CONFIG` (#827), AWS passes its keys through the process
  environment across the account's default region plus the five common ones.
  Every command is an enumeration: the endpoint is open to any project member
  and can never create, start, stop or delete anything.

- **Honest inventory errors.** Anything that stops a *configured* account from
  being enumerated — inactive account, undecryptable credentials, missing CLI
  (named with its `AZ_CMD`/`AWS_CMD`/`GCLOUD_CMD` override var), failed sign-in,
  non-zero exit, per-call timeout, exhausted account budget, unreadable output —
  is an `errors[]` line naming the provider and the account, never a silently
  empty list. Repeated per-region AWS failures fold into one line. Vendor
  boilerplate is stripped with the existing `ProviderCredentialValidator`
  helpers, and any credential value that leaks into a CLI's stderr is redacted
  before it reaches the response.

  A provider the project has **no** account for is deliberately *not* an error —
  nothing failed — so it is reported in the new `not_configured` field instead.
  The response also gained `scanned` (providers actually queried) and
  `scanned_at`; `vms[]` and `errors[]` are unchanged.

### Fixed

- **The inventory panel can no longer look dead.** After a scan that found
  nothing, `SettingsPage` re-rendered `Click "scan all providers" to discover
  VMs…` — byte-identical to never having clicked. It now tracks scan state and
  says what happened: `no VMs found — scanned azure, aws at 16:14 · not scanned:
  gcp (no cloud account configured)`, with the same provenance line under a
  populated table and a distinct message when the request itself failed. The
  wording lives in `dashboard/src/lib/inventory-scan.ts` and is unit-tested.

- **Bounded cloud CLI calls.** Each inventory CLI invocation is capped at 45s
  and each account's whole scan at 90s (the request at 150s); a hung CLI is
  tree-killed and reported as a timeout instead of stalling the Settings page.

### Changed

- **One hardened cloud-CLI process runner.** `CloudCli.RunAsync` now owns the
  spawn/drain/timeout/tree-kill semantics that `CliComputeProvisioner` and
  `OrphanReaperService` each carried their own copy of; both delegate to it and
  the inventory scan reuses it rather than adding a third copy. Behaviour is
  unchanged — missing binary, timeout and non-zero exit stay distinguishable at
  each call site.

- **Inventory scan scope diverges from the retired Rust handler on purpose.**
  The Rust scan filtered to resource groups / instance names containing
  `networker-endpoint` / `networker-tester`; those names have not existed for
  many releases (VMs are `nwk-a-*`, `nwk-ep-*`, `tester-*`, `ab-*`), so that
  filter would have hidden every VM we create on AWS and GCP. The scan now
  enumerates what the credential itself is scoped to and lets `managed` mark
  which rows are ours, capped at 500 VMs per account with an explicit note when
  the cap bites.

---

## [0.28.275] - 2026-08-20

### Added

- **The SDK Endpoints page can create the samples it advertises.** The page
  listed the LagHound reference apps and linked their source, but the only way
  to get one was to deploy it yourself and paste the URL back in — "we can see
  the samples but we cannot create". It now provisions them:
  - **Two shapes.** *Consolidated* puts every selected language on one server
    (each sample on its own port, one VM, one bill) — the default, because it
    is the cheap one. *Separated* gives one server per language for isolation
    and per-language infrastructure numbers, at N× the cost. The dialog prices
    both from the same table the deployment cost endpoint uses.
  - **Reuse first.** Before provisioning anything, every language is checked
    against what the project already runs; anything usable is registered
    against the existing server instead of buying a second one. The summary
    names what that avoids in $/mo. Reuse can be turned off explicitly, never
    silently.
  - **Update, not silent redeploy.** A sample whose deployed SDK version is
    behind the catalog is reported as outdated with *both* versions shown and
    an in-place update action that re-runs its existing deployment.
  - **Honest states.** `current`, `outdated`, `unknown_version` (alive but the
    version could not be read), `unhealthy`, `failed`, `deploying`, `none`. A
    stale or unreachable sample is never presented as usable.
- `GET/POST /api/projects/{id}/sdk-endpoints/samples` and
  `POST …/samples/{language}/update` — the catalog joined to the project's
  deployments, the reuse-first create, and the in-place update.
- `shared/sdk-samples.json` — the canonical sample catalog (id, port,
  Dockerfile, and the SDK version each sample reports on `/laghound/health`).
  Drift-guarded: every `sdk_version` is re-derived in CI from the language's
  real package manifest, and no sample port may collide with
  `shared/http-stacks.json`, the endpoint's 8080/8443, or the reference-API
  language server's 8085.
- `install.sh --setup-sdk-sample <lang>` and the deploy-config key
  `endpoints[].sdk_samples` — build a sample from source on a Linux endpoint
  (Azure/AWS/GCP/LAN/local) and run it as a `laghound-sample-<lang>` systemd
  unit on its catalog port (8101-8105, now opened on all three cloud
  firewalls). The token travels in `LAGHOUND_SAMPLE_TOKEN`, never in the
  deploy config or the log.
- Docker (local) provider: `sdk_samples` endpoints become sample containers
  (`nwk-lab/sdk-<lang>`), built by `lab/lab.sh build --samples <csv>`, so the
  whole create/reuse/update path runs in the lab for free.

### Fixed

- AWS endpoint security groups: the STAMP UDP 9997 rule was authorized against
  `$sg_id`, which the caller only assigns *after* `_aws_create_security_group`
  returns — so on a freshly created group the rule went out with an empty
  `--group-id` and aborted the deploy under `set -e`. It now uses
  `$_sg_created` like every sibling call.
- `install.sh`: `DEPLOY_EP_HTTP_STACKS` is declared alongside its four sibling
  per-endpoint arrays instead of being conjured by its first `+=`.
- The deploy budget scales for `sdk_samples` as it already did for
  reference-API `languages` — an SDK sample compiles from source (the Rust one
  is a full cargo release build), so a sample-only deploy no longer gets the
  flat 30-minute base and a tree-kill mid-build.

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
