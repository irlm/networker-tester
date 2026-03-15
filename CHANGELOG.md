# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

---

## [0.13.21] – 2026-03-15 — First impairment scenario support

### Added
- **First impairment scenario support** — config-driven delay profiles for reproducible benchmark scenarios using the existing endpoint `/delay?ms=N` capability
- **Impairment config** — `impairment.profile` and `impairment.delay_ms` are now available in config resolution

### Changed
- **HTTP-family impairment wiring** — supported HTTP-family probes now consistently route through the delayed target when impairment delay is enabled
- **Deploy docs/example** — impairment configuration is documented and included in the example config

---

## [0.13.20] – 2026-03-15 — Tester-side packet capture MVP

### Added
- **Tester-side packet capture MVP** — optional `tshark` capture integrated into `networker-tester`, producing `.pcapng` artifacts and packet summary JSON for local validation and future real-world benchmarking
- **Packet capture deploy/install config** — new `packet_capture` configuration block with `none|tester|endpoint|both` scope selection, tester-side MVP support, and generated config wiring
- **macOS packet-capture guidance** — deploy docs and installer messaging now explain manual Wireshark/ChmodBPF requirements when package-manager installs are insufficient

### Changed
- **Capture summary UX** — summary output now reports capture status and can explain when a trace succeeded but no UDP/QUIC packets were observed
- **Capture preflight** — packet capture now checks tool availability and macOS BPF permission readiness before attempting a run

---

## [0.13.19] – 2026-03-15 — Dashboard frontend fixes

### Fixed
- **Accessibility** — login form label/input linking (`htmlFor`/`id`), sidebar ARIA (`aria-label`, `aria-current="page"`, `aria-hidden` on icons), modal dialog (`role="dialog"`, `aria-modal`, focus trap, escape key), `motion-safe:` prefix on animations
- **WebSocket reliability** — prevent reconnection after unmount (memory leak), exponential backoff (3s→60s cap), runtime message validation, ref-based callback to avoid HMR reconnect loops
- **TypeScript safety** — extract `LiveAttempt` interface with typed DNS/TCP/TLS/HTTP/UDP sub-results, remove all `Record<string, unknown>` and `as` casts from JobDetailPage
- **Performance** — `usePolling` hook with cancellation guard, `useMemo` for chart data and event feed, stable empty array constant for Zustand selector, only spread `liveAttempts` on `attempt_result` events
- **UX** — loading/error states on all list pages, responsive stat cards grid, mobile sidebar toggle with escape key, error display in CreateJobDialog, per-job attempt cap (2000) with cleanup on terminal state

---

## [0.13.18] – 2026-03-14 — Protocol-specific throughput probes

### Added
- **Protocol-specific throughput modes** — `download1`, `download2`, `download3`, `upload1`, `upload2`, and `upload3` enable paper-style single-file transfer comparisons across HTTP/1.1, HTTP/2, and HTTP/3 with distinct report labels

### Changed
- **`upload1` parity with `download1`** — HTTP/1.1 upload throughput probes now downgrade HTTPS endpoint URLs to the plain-HTTP stack port mapping used by `download1`, keeping H1 transfer comparisons consistent across directions
- **Docs clarified for `webdownload` / `webupload`** — these modes were already targeting the built-in endpoint transfer routes; the documentation now reflects the existing implementation instead of describing them as arbitrary target-URL probes

---

## [0.13.17] – 2026-03-13 — Fix browser1/pageload1 on HTTP stack ports, installer bash 3.2 compat

### Fixed
- **browser1/pageload1 stack port mapping** — HTTP/1.1 probes now correctly map HTTPS stack ports to HTTP: 8444→8081 (nginx), 8445→8082 (IIS). Previously, browser1 sent plain HTTP to HTTPS listeners causing `ERR_CONNECTION_RESET`
- **Installer bash 3.2 compatibility** — replaced `${var^^}` (bash 4+) with `tr` for macOS compatibility in `ask_lan_options()` and Azure password storage
- **Installer IIS heredoc quoting** — quoted `<<'IIS_PS1_GENSITE'` heredoc prevents bash from interpreting embedded PowerShell/HTML content as shell syntax
- **Installer `_iis_setup_powershell`** — removed unused `ep_exe` parameter; hardcoded endpoint binary path

---

## [0.13.16] – 2026-03-13 — Stack column in All Attempts & TCP Stats, chart label suffixes

### Added
- **Stack column in All Attempts table** — dedicated column shows "endpoint", "nginx", or "iis" for each attempt row (only when HTTP stacks are present)
- **Stack column in TCP Stats table** — kernel-level TCP stats are clearly attributed to the correct stack
- **Chart label suffixes** — SVG chart data labels now include the stack name (e.g. "browser1 endpoint", "pageload3 nginx") for unambiguous identification

### Removed
- Inline `[nginx]` tag in Protocol column — replaced by the dedicated Stack column

---

## [0.13.15] – 2026-03-13 — Independent stack report sections, nginx proxy, IIS FQDN, cloud FQDN support

### Changed
- **HTML report: independent stack sections** — each HTTP stack (nginx, IIS) now renders its own full set of sections (timing breakdown, statistics, protocol comparisons, browser results, charts & analysis) instead of a combined comparison table. Stack data is never mixed with endpoint data.

### Added
- **nginx proxy_pass for dynamic paths** — nginx configs in install.sh now proxy `/page` and `/asset` requests to the endpoint, enabling pageload probes through nginx
- **IIS setup with URL Rewrite + ARR** — install.sh configures IIS with Application Request Routing proxy rules, self-signed certs, HTTP/3 registry keys, and reboot-wait-verify flow
- **FQDN support for cloud deployments** — Azure VMs get DNS labels (`.cloudapp.azure.com`), AWS uses `PublicDnsName`; config generation prefers FQDN over IP for target URLs
- **IIS HTTP/3 via FQDN+SNI binding** — hostname-based SSL binding (`sslFlags=1`) enables QUIC/HTTP3 on IIS (requires SNI per Microsoft spec)

### Fixed
- **Browser Results filter bug** — browser results section was not filtering by `http_stack`, causing stack browser probes to leak into endpoint browser results
- **Stack URL rewriting preserves hostname** — `rewrite_url_for_stack()` keeps the FQDN from the target URL when routing to stack ports

---

## [0.13.14] – 2026-03-12 — Stack comparison rejects HTTP 4xx

### Fixed
- **Stack comparison still showed 404 probes**: `success` flag treats HTTP 400-499 as success (`status < 500`). Stack comparison now additionally rejects `status_code >= 400`, correctly showing "not supported" for nginx probes returning 404 (missing static site)

---

## [0.13.13] – 2026-03-12 — Stack comparison: filter failed probes

### Fixed
- **HTTP Stack Comparison false positives**: Stack probes returning HTTP 400/404 (e.g. nginx pageload3 without proper static site) were counted as successful in the comparison table — now only successful attempts (a.success) contribute to stack stats; failed protocols correctly show "not supported"

---

## [0.13.12] – 2026-03-12 — HTML report version display + stack data isolation in summaries

### Added
- **Version display in report header**: Client and server versions shown prominently at the top of both single-target and multi-target HTML reports
- **Stack probe badge in All Attempts**: Stack probe rows now show `[nginx]` or `[iis]` badge next to the protocol name for visual distinction

### Fixed
- **Summary counts included stack probes**: Multi-target summary table and per-target Run Summary showed inflated attempt/success/fail counts (e.g. 120 instead of 60) because they included HTTP stack probe attempts — now filtered to show only endpoint data, with a `(+ N stack probes)` note when applicable

---

## [0.13.11] – 2026-03-12 — Cloud nginx setup in deploy-config + boxplot fix

### Fixed
- **Cloud deploy missing nginx/IIS setup**: `deploy_from_config()` now installs nginx (with HTTP/3) and acknowledges IIS on Azure, AWS, and GCP endpoints when `http_stacks` is specified — previously only local and LAN providers ran the setup
- **Per-target boxplot charts empty with low run counts**: Lowered `svg_boxplot()` minimum from 4 to 2 data points so distribution charts render even with small runs

---

## [0.13.9] – 2026-03-11 — Stack data isolation + nginx HTTP/3 + deploy-config http_stacks

### Added
- **Deploy-config `http_stacks` support**: `endpoints[].http_stacks` and `tests.http_stacks` fields in `--deploy` JSON configs — installer provisions nginx/IIS on endpoints and passes `--http-stacks` to the tester automatically
- **Validation**: OS compatibility checks (nginx→linux, iis→windows) and valid stack name enforcement at config validation time
- **"Not supported" rows** in HTTP Stack Comparison table when a stack lacks data for a protocol the endpoint tested (e.g. nginx without QUIC shows "not supported" for pageload3)
- 13 new bats tests for deploy-config http_stacks validation, parsing, and tester config generation

### Fixed
- **Stack data contamination**: HTTP stack probe attempts (nginx/IIS) were mixed into main report statistics, cross-target comparison, charts, and protocol breakdowns — added `http_stack.is_none()` filters to 8 locations in HTML report generation
- **nginx HTTP/3**: Install mainline nginx from nginx.org (1.27+) with built-in QUIC support; added `listen 8444 quic reuseport`, `http3 on`, and `Alt-Svc` header to all nginx setup paths (local, SSH, GCP)
- **Firewall UDP ports**: Open UDP 8443-8445 (was only 8443) for Azure NSG, AWS SG, and GCP firewall rules to allow QUIC on nginx/IIS ports

---

## [0.13.8] – 2026-03-11 — HTTP stack comparison (nginx & IIS)

### Added
- **HTTP stack comparison**: `--http-stacks nginx,iis` probes additional HTTP servers alongside the default networker-endpoint, enabling side-by-side performance comparison of different web servers serving identical static content
- **`generate-site` subcommand** for networker-endpoint: `networker-endpoint generate-site <dir> --preset mixed --stack nginx` generates a static test site (index.html + 50 assets matching the `mixed` preset) for nginx/IIS/etc. to serve
- **HTML report "HTTP Stack Comparison" section**: table showing avg/p50/min/max load times per protocol per stack (endpoint vs nginx vs IIS)
- **`http_stack` field** on `RequestAttempt`: tags each probe result with the stack name (e.g. `"nginx"`, `"iis"`); `None` = default endpoint
- **Installer nginx setup**: `step_setup_nginx()` for local, `_remote_setup_nginx()` for SSH, `_gcp_setup_nginx()` for GCE — installs nginx, generates static site, configures self-signed cert, HTTP on port 8081, HTTPS/H2 on port 8444
- **Installer IIS setup**: `_iis_setup_powershell()` + `_azure_win_setup_iis()` — installs IIS, enables HTTP/3 via registry, generates static site with `web.config` (MIME types for extensionless `/health` and `.bin`), HTTP on port 8082, HTTPS on port 8445
- **Firewall port ranges**: Azure NSG, AWS SG, GCP firewall rules now open 8081-8082 (HTTP) and 8444-8445 (HTTPS) for stack comparison servers
- **`HttpStack` struct** in CLI: `from_name()` factory for well-known stacks with assigned port pairs
- 40 new unit tests (endpoint: 20 for `generate_static_site`/`resolve_preset`; tester: 14 CLI + 4 main.rs + 2 HTML)

### Fixed
- **nginx `http2` directive**: use `listen 8444 ssl http2;` (compatible with nginx 1.18 on Ubuntu 22.04 through nginx 1.25+) instead of the newer `http2 on;` syntax
- **IIS `web.config`**: use `remove+add` pattern for MIME maps to avoid duplicate entry errors; handle extensionless `/health` file

### Port Convention
| Stack | HTTP | HTTPS |
|-------|------|-------|
| networker-endpoint | 8080 | 8443 |
| nginx | 8081 | 8444 |
| IIS | 8082 | 8445 |
| caddy (future) | 8083 | 8446 |
| apache (future) | 8084 | 8447 |

---

## [0.13.7] – 2026-03-11 — HTML report: short target names & pageload1 plain HTTP

### Added
- **Cloud hostname detection**: AWS internal hostnames (`ip-172-31-*`) now display as "AWS Ubuntu", "AWS Windows", etc.
- **Short target names**: cross-target comparison headers, SVG chart titles, observations, and collapsible details all use provider+OS names instead of "Target N" with full URLs
- **Duplicate name disambiguation**: when multiple targets share the same short name (e.g. two AWS Ubuntu), they get `#1`, `#2` suffixes
- 17 new unit tests for hostname detection, short names, URL rewriting, and display name derivation

### Changed
- **pageload1 now uses plain HTTP** — matches browser1 behavior (both HTTP/1.1 without TLS). Eliminates 6× TLS handshake overhead that made pageload1 vs browser1 comparison unfair. Port mapping: 8443→8080, 443→80.
- Refactored inline display name logic into shared helpers (`derive_display_name`, `is_cloud_internal_hostname`, `os_short_label`, `provider_from_region`, `rewrite_to_http`)

---

## [0.13.6] – 2026-03-10 — Multi-database abstraction layer

### Added
- **Database abstraction**: `DatabaseBackend` trait with auto-detect factory (`postgres://` → PostgreSQL, ADO.NET → SQL Server)
- **PostgreSQL backend** (`--features db-postgres`): `tokio-postgres` driver with embedded idempotent migrations, native UUID/TIMESTAMPTZ types
- **CLI flags**: `--save-to-db`, `--db-url` (env: `NETWORKER_DB_URL`), `--db-migrate` — generic database insertion replacing the SQL Server-specific `--save-to-sql`
- **docker-compose.db.yml**: PostgreSQL 16 + SQL Server 2022 for local development
- **CI**: PostgreSQL integration test job with Docker service container
- 9 PostgreSQL integration tests (round-trip, field verification, cascade delete, migration idempotency)

### Changed
- SQL Server driver (`tiberius`, `tokio-util`) now optional behind `db-mssql` feature (included in default features — no breaking change)
- `--save-to-sql` and `--connection-string` still work as hidden aliases for backward compatibility

---

## [0.13.5] – 2026-03-10 — HTML report server identification

### Fixed
- **HTML report**: Show endpoint version (`v0.13.4`) next to each server name in summary table for quick version verification
- **HTML report**: Derive provider-aware display names (e.g. "Azure Windows", "GCP Ubuntu") when hostname is "unknown" or missing
- **networker-endpoint**: Fix hostname detection on Windows — check `COMPUTERNAME` env var and `hostname` command (previously only checked Linux-specific paths, returning "unknown" on Windows)

---

## [0.13.4] – 2026-03-10 — Pageload3 concurrent multiplexing & report improvements

### Fixed
- **pageload3 (HTTP/3)**: Send all QUIC asset requests concurrently via `join_all()` instead of sequentially awaiting each `send_request()` — matches real Chrome H3 multiplexing behavior, ~40-75% faster on high-latency connections
- **HTML report**: LAN/Loopback targets shown as dimmed reference values (no diff percentages); best overall Internet target (by composite rank-sum score across all protocols) used as diff baseline; cross-target observations only compare Internet targets
- **install.sh**: Use `-WindowStyle Hidden` instead of `-NoNewWindow` for Windows endpoint process start — `az vm run-command` and SSH wait for all child processes sharing the console, causing indefinite hang
- **install.sh**: Skip install when reusing VMs if endpoint already healthy with correct version — saves 3-5 min on Windows cold boot, ~30s on Linux

---

## [0.13.3] – 2026-03-10 — Windows endpoint VC++ runtime fix

### Fixed
- **install.sh**: Windows endpoint crash on GCP/Azure — missing `vcruntime140.dll` (exit code `0xC0000135`); install VC++ Redistributable before running binary
- **install.sh**: Replace `sc.exe create`/`schtasks /Run` with `Start-Process -NoNewWindow` on all Windows paths (GCP startup, Azure run-command, LAN SSH, GCP SSH)
- **install.sh**: Add detailed diagnostics to GCP Windows startup script (ZIP/binary size, file listing, stderr capture, process checks)
- **release.yml**: Static-link CRT on Windows (`-C target-feature=+crt-static`) so binaries work on clean Windows Server images

---

## [0.13.2] – 2026-03-10 — GCP Windows deploy fix & release workflow

### Fixed
- **install.sh**: GCP Windows endpoint deployment via startup-script trampoline (avoids GCE script runner timeout)
- **install.sh**: VS Build Tools `--installPath C:\BuildTools` for correct MSVC detection (Azure + GCP Windows)
- **install.sh**: iptables OUTPUT REDIRECT cleanup (prevents outbound HTTPS breaking on endpoint VMs)
- **install.sh**: Download installer locally then SCP to VM (bypasses CDN/network issues on cloud VMs)
- **install.sh**: Stop endpoint service before copying binary (fixes "Text file busy")
- **install.sh**: Start stopped/deallocated VMs on reuse (Azure + GCP)
- **release.yml**: Use `--notes-file` to prevent shell interpretation of CHANGELOG backticks

---

## [0.13.1] – 2026-03-09 — Windows VM name validation & docs cleanup

### Fixed
- **install.sh**: Validate Windows VM names are <=15 characters in deploy-config mode (Azure rejects longer `osProfile.computerName`)
- **README.md**: Removed outdated "private repo" SSH key prerequisite; repo is public

### Changed
- **install.sh / install.ps1**: Bumped `INSTALLER_VERSION` fallback to `v0.13.1`

---

## [0.13.0] – 2026-03-08 — Config-driven deploy & test

### Added
- **install.sh**: `--deploy deploy.json` flag for non-interactive, config-driven deployment and testing
- JSON config schema (version 1): define tester + endpoint(s) + test parameters in one file
- Multi-endpoint support: deploy to multiple machines/clouds and test against all of them
- Pre-flight validation: checks tools, cloud credentials, SSH connectivity before any deployment
- Auto-generated tester config from deployed endpoint IPs and test parameters
- Remote test execution: runs tester via SSH and downloads HTML/Excel reports
- Deploy plan display: shows full deployment topology before starting
- Completion summary: lists all deployed infrastructure and report locations
- Example configs: `deploy.example.json`, `examples/deploy-lan.json`, `examples/deploy-multi-cloud.json`
- Documentation: `docs/deploy-config.md` with full schema reference
- 19 new bats tests for config validation, parsing, endpoint loading, and config generation

---

## [0.12.99] – 2026-03-08 — LAN deployment via SSH

### Added
- **install.sh / install.ps1**: New "LAN / existing machine (SSH)" deployment option alongside Azure/AWS/GCP
- SSH pre-flight test with detailed troubleshooting help on failure
- Auto-detect remote OS (Linux/macOS/Windows) via SSH
- Install to `~/.local/bin/` when passwordless sudo is unavailable
- Endpoint runs as nohup background process when systemd requires sudo
- Non-standard SSH port support (`--lan-port`)
- CLI flags: `--lan`, `--tester-lan`, `--lan-ip`, `--lan-user`, `--lan-port`, etc.

---

## [0.12.98] – 2026-03-08 — GCP Windows OS choice

### Added
- **install.sh / install.ps1**: GCP deployment now prompts for OS (Ubuntu 22.04 or Windows Server 2022), matching Azure and AWS
- GCP Windows helpers: VM wait, password reset, binary install, service creation, auto-shutdown scheduled task
- RDP port 3389 added to GCP firewall rule in both installers

---

## [0.12.97] – 2026-03-07 — Comprehensive test coverage

### Added
- **pageload.rs**: 51 integration tests covering H1/H2/H3 success, error paths, DNS, warmup+warm sequences
- **http3.rs**: refactored `build_quic_endpoint()` and `resolve_addr()` for testability; 16 new tests
- **sql.rs**: 6 INSERT→SELECT round-trip tests — verify all columns in every table, CASCADE DELETE, PK constraint, multi-attempt counts (run in CI via Docker SQL Server)
- Total: 73 new tests (294 total), all passing on Ubuntu + Windows CI

---

## [0.12.96] – 2026-03-07 — Cloud env credential checks (Azure, AWS, GCP)

### Added
- Installer (install.sh + install.ps1): check environment credentials before prompting interactive login for all three cloud providers
  - **AWS**: `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` — validates with `aws sts get-caller-identity`
  - **Azure**: `AZURE_CLIENT_ID` + `AZURE_CLIENT_SECRET` + `AZURE_TENANT_ID` — authenticates with `az login --service-principal`
  - **GCP**: `GOOGLE_APPLICATION_CREDENTIALS` (service account JSON key file) — activates with `gcloud auth activate-service-account`
- All providers display identity details when env credentials are found (ARN, subscription name, service account email)
- README: new "Cloud Deployment Authentication" section with env var reference, setup examples, and links to official docs

---

## [0.12.95] – 2026-03-07 — Connection reuse for pageload probes

### Added
- `--connection-reuse` flag: warmup + warm probes for pageload2 (HTTP/2) and pageload3 (HTTP/3 QUIC)
- Warmup probe establishes connection (cold, visible in report); subsequent runs reuse it (warm)
- Matches Chrome browser3's behavior (Alt-Svc warmup + QUIC 0-RTT session reuse)
- `connection_reused` field in `PageLoadResult` JSON output
- HTML report: Protocol Comparison table splits into cold/warm rows when reuse is detected
- HTML report: Observations section shows cold→warm timing, % improvement, and TLS savings

---

## [0.12.94] – 2026-03-07 — Docs: correct CLI flag descriptions

### Fixed
- README: clarify `--log-level` overrides `RUST_LOG` (and `--verbose` on tester only)
- README: note `--verbose` is tester-only
- docs/testing.md: fix JSON output description — automatic to `output/`, not `--json` to stdout
- docs/testing.md: update `--html` → `--html-report`, `--json` → `--output-dir` in quick-ref table

---

## [0.12.93] – 2026-03-07 — Windows installer: cloud deployment parity

### Added
- `install.ps1`: Full cloud deployment support matching `install.sh` — Azure, AWS, and GCP VM provisioning from Windows
- `install.ps1`: Cloud CLI detection (az, aws, gcloud) with auto-install via winget
- `install.ps1`: Interactive region/size/OS/auto-shutdown prompts for all three providers
- `install.ps1`: VM existence check (reuse/rename/delete) for all providers
- `install.ps1`: Remote binary install via SSH + Gist bootstrap installer
- `install.ps1`: Component selection and where-to-install prompts
- `install.ps1`: INSTALLER_VERSION fallback when gh CLI is unavailable
- `install.ps1`: Config file generation and completion summary with cleanup commands

### Fixed
- Installer (both): All three cloud providers now check if VM/instance already exists before creation — offers reuse, rename, or delete+recreate instead of crashing

---

## [0.12.91] – 2026-03-07 — Installer: fix GCP re-login on every run

### Fixed
- Installer: GCP login status now checked in `ensure_gcp_cli` before prompting — since `discover_system` defers all gcloud execution, `GCP_LOGGED_IN` was always 0, causing unnecessary re-login every run

---

## [0.12.90] – 2026-03-07 — Installer: GCP improvements, curl|bash robustness

### Fixed
- Installer: AWS keypair import — delete and re-import to match local key (was silently reusing stale keypair)
- Installer: GCP project number auto-resolved to project ID in all code paths
- Installer: GCP auto-enable Compute Engine API + billing check in prerequisites
- Installer: GCP gcloud detected in `~/google-cloud-sdk/bin` on repeat runs
- Installer: GCP login re-checked before prompting (avoids re-login every run)
- Installer: GCP VM install downloads installer from Gist when running via `curl|bash` (SCP fails when no local file)
- Installer: GCP SSH runner uses `< /dev/null` to prevent stdin consumption
- Installer: prevent CLI tools (`gh`, `az`, `aws`, `gcloud`) from consuming stdin in `curl|bash` mode

---

## [0.12.89] – 2026-03-07 — Fix AWS security group printf-v shadowing

### Fixed
- Installer: AWS `sg_id` variable shadowed by `local` inside `_aws_create_security_group` — `printf -v` wrote to the function's local, leaving caller's variable unset (`unbound variable` crash)

---

## [0.12.88] – 2026-03-06 — Installer: curl|bash fixes, Windows VM, GCP auto-install

### Fixed
- Installer: complementary component prompt reads from `/dev/tty` (was exiting silently in `curl|bash`)
- Installer: Azure Windows VM creation generates admin password (was failing in non-interactive mode)
- Installer: all interactive CLI logins (`aws configure sso`, `az login`, `gcloud auth login`) redirect stdin from `/dev/tty`
- Installer: `az vm auto-shutdown` now passes `--location` (was silently failing)
- Installer: auto-shutdown errors shown instead of suppressed

### Added
- Installer: auto-install Google Cloud SDK on Linux via official tarball (was manual-only on non-brew systems)

---

## [0.12.87] – 2026-03-06 — TCP Stats browser note, installer version-aware fallback

### Fixed
- Installer: source-compile fallback now checks binary version, not just existence — old version on PATH no longer silently skips recompilation
### Added
- HTML report: note in TCP Stats section explaining why browser probes are absent (Chrome owns the TCP connections)

---

## [0.12.86] – 2026-03-06 — Installer: GLIBC compatibility check, multi-region fix

### Fixed
- **Installer**: downloaded binaries are now verified with `--version` before declaring
  success. If the binary fails to run (e.g., GLIBC mismatch on Ubuntu 22.04), it is
  removed and the installer falls back to source compilation automatically.
- **Installer**: fixed `bad substitution` error in multi-region endpoint count display
  (`${#arr[@]+1}` → `$(( ${#arr[@]} + 1 ))`).

---

## [0.12.85] – 2026-03-06 — Installer: binary-first install, spinner fix, region-aware names

### Fixed
- **Installer**: build spinner no longer floods terminal with identical lines on SSH
  pseudo-TTYs — only redraws when compiled crate count changes; hides `[0 crates]`
  during fetch phase; sanitizes `grep -c` output (fixes `[[: 0` error on some systems).
- **Installer**: suggested VM/RG names now include the region (e.g.,
  `nwk-ep-lnx-b1s-eastus`) to avoid cross-region resource conflicts when creating
  VMs in different regions.

### Changed
- **Installer**: tries downloading pre-built binary from GitHub releases via `curl`
  before falling back to source compilation. Works without `gh` CLI — queries GitHub
  API directly for the latest release tag. Fresh VM installs go from ~5-10 min compile
  to seconds (when release binaries exist).

---

## [0.12.84] – 2026-03-06 — Network baseline RTT, cloud region, network type

### Added
- **Endpoint**: `GET /info` now includes `region` field, auto-detected from cloud
  instance metadata at startup (Azure, AWS, GCP). Shows `azure/eastus`,
  `aws/us-east-1`, `gcp/us-central1 (us-central1-a)`, or `null` if not on cloud.
- **Tester**: measures network baseline RTT before probes start — 5 TCP connect
  round-trips with min/avg/max/p50/p95 statistics.
- **Tester**: classifies network path as Loopback (127.x), LAN (10.x/172.16-31.x/
  192.168.x/fe80::/fc00::), or Internet (public IP).
- **HTML report**: "Network Baseline" card shows RTT stats and network type
  badge (green=Loopback, orange=LAN, red=Internet).
- **HTML report**: Multi-target summary table now includes Network type and RTT
  columns for at-a-glance latency comparison between targets.
- **HTML report**: Server info card shows cloud region when available.
- **JSON output**: `baseline` object (RTT + network_type) and `region` in
  `server_info` / `HostInfo` (backward-compatible via `#[serde(default)]`).

---

## [0.12.83] – 2026-03-06 — Google Cloud Platform (GCP) support in installer

### Added
- **Installer**: GCP / Google Cloud Engine (GCE) support — provision and deploy
  networker-tester and networker-endpoint on GCE instances, alongside existing
  Azure and AWS support.
- New CLI flags: `--gcp`, `--tester-gcp`, `--gcp-region`, `--gcp-zone`,
  `--gcp-machine-type`, `--gcp-project`.
- Interactive deployment: zone selection (8 regions), machine type, project
  auto-detection from `gcloud config`, auto-shutdown via cron.
- `ensure_gcp_cli()`: install gcloud SDK (brew on macOS), device-code login
  via `gcloud auth login --no-launch-browser`.
- `ask_gcp_options()`: interactive zone/machine-type/name prompts.
- GCE deployment: `gcloud compute instances create` (Ubuntu 22.04 LTS),
  firewall rules (tagged `networker-endpoint`), SSH via `gcloud compute ssh`,
  systemd service + iptables redirects, health check verification.
- Completion summary shows GCP-specific SSH commands (`gcloud compute ssh`)
  and cleanup instructions (`gcloud compute instances delete`).
- `INSTALLER_VERSION` bumped to v0.12.83.

---

## [0.12.82] – 2026-03-06 — Server & client system info in reports

### Added
- **Endpoint**: `GET /info` now returns system metadata (`system` object):
  OS, architecture, CPU cores, total memory, OS version, hostname, uptime.
  Computed at startup; no external dependencies.
- **Tester**: fetches `/info` from the target before running probes (doubles
  as a connectivity check). Server metadata stored in `TestRun.server_info`.
- **Tester**: collects local client system info (`TestRun.client_info`).
- **HTML report**: "Client Info" and "Server Info" cards displayed side-by-side
  after the Run Summary. Multi-target summary table shows server hostname,
  OS, cores, and memory per target for easy comparison.
- **JSON output**: `server_info` and `client_info` fields included in the
  serialized `TestRun` (backward-compatible via `#[serde(default)]`).

---

## [0.12.81] – 2026-03-06 — Installer: AWS SSO device-code login

### Added
- **AWS SSO / Identity Center login**: new device-code authentication flow
  (similar to Azure's `az login --use-device-code`). Users are now prompted to
  choose between SSO (opens browser, no keys needed) and classic access keys.
  SSO is the default option.
- Detects existing SSO profiles and offers to reuse them or create a new one.
- `AWS_PROFILE` is exported after SSO login so subsequent AWS commands use the
  SSO session automatically.

---

## [0.12.80] – 2026-03-06 — Fix installer bugs: unzip, $VAR…, --html

### Fixed
- **`unzip` not installed on fresh Linux VMs**: auto-install `unzip` via `$PKG_MGR`
  before extracting the AWS CLI v2 zip archive (previously `unzip: command not found`).
- **`$AWS_REGION…` / `$AZURE_REGION…` unbound variable**: bash treated the UTF-8
  ellipsis `…` (U+2026, bytes `\xe2\x80\xa6`) as part of the variable name, causing
  `AWS_REGION…: unbound variable` under `set -u`.  Fixed with `${AWS_REGION}` braces.
- **`--html` no-such-flag in quick-test**: removed bogus `--html` flag from the
  `_offer_quick_test` call; the HTML report is always generated by default.

---

## [0.12.79] – 2026-03-06 — AWS Windows two-VM integration test

### Added
- **AWS Windows integration test** (`tests/integration/aws/tester_endpoint_deploy_windows.bats`):
  deploys networker-endpoint and networker-tester on two Windows Server 2022 EC2 instances
  via AWS SSM, runs HTTP/1.1 and HTTP/2 probes, downloads JSON report; 7/7 pass.
  Uses `m7i-flex.large` (2 vCPU, 4 GB; free-tier eligible), IAM role `nwk-ssm-role` with
  `AmazonSSMManagedInstanceCore`, and `AWS-RunPowerShellScript` document.
  SSM parameters passed as `{"commands":[...]}` JSON object; multi-line PS scripts split
  line-by-line so SSM writes a proper `.ps1` file preserving backtick line-continuation.

---

## [0.12.78] – 2026-03-05 — Fix HTTP/2 ~40 ms latency spikes (Nagle + delayed-ACK)

### Fixed
- **TCP_NODELAY on all HTTP/1+2 connections (client and server).**
  HTTP/2 probes showed intermittent ~40–42 ms TTFB spikes (1–2 of every 5 runs)
  due to the classic Linux Nagle + delayed-ACK interaction: during the HTTP/2
  SETTINGS handshake, one side holds a small segment (SETTINGS/SETTINGS_ACK)
  waiting for data to piggyback the ACK on, while the other side waits for an ACK
  before sending. The TCP delayed-ACK timer fires after 40 ms, unblocking both.

  Fix:
  - **Tester (`http.rs`, `native.rs`)**: call `set_nodelay(true)` on the
    `TcpStream` immediately after `connect()`, before passing it to hyper.
  - **Endpoint (`lib.rs`)**: use `axum_server::accept::NoDelayAcceptor` (plain
    HTTP server) and `RustlsAcceptor::new(cfg).acceptor(NoDelayAcceptor)` (HTTPS
    server) so every accepted socket has TCP_NODELAY set before the TLS handshake.

  HTTP/2 mean TTFB drops from ~9 ms (with occasional 41 ms outliers) to sub-1 ms,
  matching HTTP/1.1 on the same connection.

---

## [0.12.77] – 2026-03-05 — Fix installer prompts in non-interactive mode; fix Azure two-VM integration test

### Fixed
- **`_offer_also_endpoint` now skips in auto-yes (`-y`) mode.**
  When installing only `networker-tester` with `-y` (e.g., via the Azure two-VM
  integration test), the "also install endpoint?" prompt would hang or exit non-zero
  because there is no stdin terminal. Added `[[ $AUTO_YES -eq 1 ]] && return 0`.
- **Azure two-VM integration test: fixed `networker-tester` invocation.**
  - Replaced `--output json --report <path>` (invalid flags) with `--output-dir <dir>`.
  - Removed `--quiet` flag (does not exist in the CLI).
  - Tester output dir is now `ls`-ed to find the `run-*.json` artifact before SCP.

### Integration tests
- `tests/integration/azure/tester_endpoint_deploy.bats` — **7/7 pass** end-to-end:
  endpoint service active, /health HTTP, tester binary, HTTP/1.1 probe, HTTP/2 probe,
  JSON report downloaded (azure-eastus-Ubuntu22-<timestamp>.json), report fields valid.

---

## [0.12.76] – 2026-03-05 — Fix source install on fresh Linux VMs (apt-get update, binary path)

### Fixed
- **`apt-get install build-essential` now runs `apt-get update -qq` first.**
  Fresh Ubuntu VMs (Azure/AWS) have stale package metadata and fail with 404
  errors when trying to install GCC without updating first.
- **`networker-endpoint` systemd service now copies binary to `/usr/local/bin/`**
  before writing the unit file. The `networker` system user cannot traverse
  `/home/<user>/.cargo/bin/` on Ubuntu 22.04 where home dirs are `drwx------`.
  The binary is now copied to a world-executable location so the service starts.
- **`aws ec2` commands: replace `--output none` with `--output text >/dev/null`.**
  AWS CLI v2 does not support `--output none` (unlike Azure CLI); all AWS EC2
  calls in the installer now redirect output silently.

### Tests
- AWS integration test (`tests/integration/aws/endpoint_deploy.bats`): 7/7 pass
  end-to-end against a real `t3.small` in `us-east-1`.
- Both Azure and AWS integration tests now use `BATS_FILE_TMPDIR` state files
  to pass VM IP / instance ID between `setup_file` and `@test` subshells
  (bats runs each `@test` in a subshell; exported vars don't propagate).

---

## [0.12.75] – 2026-03-05 — Sync install.ps1 to public repo; fix spinner in curl|bash

### Fixed
- **install.ps1 still used SSH URL and SSH check** after the repo went public in
  v0.12.72. `$RepoSsh` renamed to `$RepoHttps` (`https://github.com/irlm/networker-tester`);
  `Invoke-SshStep`, `$script:DoSshCheck`, and `-SkipSshCheck` parameter removed;
  `--locked` dropped from both `cargo install` calls; `CARGO_NET_GIT_FETCH_WITH_CLI`
  removed; "private Git repo" and SSH-key prerequisite language updated throughout
  (`Show-Help`, `Show-Plan`, `Invoke-CustomizeFlow`, `Invoke-CargoInstallStep`).
- **Spinner showed `[0` on a new line each frame in `curl | bash` installs.**
  Root cause: `\r` (carriage return) is unreliable as an in-place overwrite mechanism
  when stdin is a pipe — any terminal line-wrapping causes `\r` to land on the overflow
  row instead of the spinner row, leaving every frame permanently visible. Fix: replaced
  the `\r\033[2K` pattern with `printf "\n"` before the loop and `\033[1A\033[2K...\n`
  inside (VT100 cursor-up + erase-line + newline). This is unambiguous on all terminals
  regardless of stdin mode.

---

## [0.12.74] – 2026-03-05 — Auto-install build tools; fix spinner when stdin is piped

### Fixed
- **Missing C linker no longer aborts with a cryptic cargo error.** When `cc`/`gcc`/`clang`
  is absent on Linux the installer now automatically runs the appropriate package manager
  command (`apt-get install -y build-essential`, `dnf install -y gcc gcc-c++ make`, etc.)
  before invoking `cargo install`. This affects both local source builds and the
  remote VM bootstrap path (the VM's own installer runs the same code).
  On macOS the installer still prints the `xcode-select --install` instruction and exits
  cleanly (Xcode CLI tools cannot be installed non-interactively).
- **Build spinner prints on a new line every frame when running as `curl | bash`.**
  Root cause: when bash's stdin is a pipe, `tput cols` queries stdin (which is the
  pipe, not the terminal) and returns 0. With `cols=0` the phase was dropped but
  `count_tag` still made the line long enough to wrap on narrow terminals. Wrapping
  causes `\r` to land on the overflow line, so each frame scrolls down instead of
  overwriting in place. Fix: use `stty size </dev/tty` as primary width source
  (queries the terminal directly regardless of stdin), with `tput cols` and
  `$COLUMNS` as fallbacks. Also hard-limit the entire visible line to `cols-1`
  characters so wrapping is impossible even if width is mis-detected.

---

## [0.12.73] – 2026-03-05 — Fix build spinner and iptables persistence on fresh VMs

### Fixed
- **Build spinner**: each spinner frame was printing on a new line instead of updating
  in place when the terminal was narrow. Replaced `\r%-*s\r` column-padding with
  `\r\033[2K` (ANSI erase-line), which reliably clears the current line without
  requiring accurate column counting. Also added a `max_phase <= 0` guard to prevent
  an empty `phase` variable from causing an arithmetic overflow.
- **iptables persistence** (`step_setup_endpoint_service` and
  `_remote_create_endpoint_service`): `sh: cannot create /etc/iptables/rules.v4:
  Directory nonexistent` error when `/etc/iptables/` did not exist (systems without
  `iptables-persistent` pre-installed). Fix: `sudo mkdir -p /etc/iptables` before
  attempting `iptables-save`.

---

## [0.12.72] – 2026-03-04 — Public repo: HTTPS install, VM bootstrap, local service setup

### Changed
- **Repo is now public** — source installs no longer require an SSH key. All
  `cargo install --git` calls now use the HTTPS URL (`https://github.com/irlm/networker-tester`).
  `CARGO_NET_GIT_FETCH_WITH_CLI` and the redundant `if command -v git` branch are removed.
- **Remote VM source fallback** rewritten as `_remote_bootstrap_install`: uploads this
  installer script to the VM via SCP (or downloads it from the Gist when running as
  `curl | bash`) and runs `bash /tmp/networker-install.sh <component> -y` on the VM.
  The VM's own installer handles Rust installation, binary download/compile, and service
  setup. Removes the need for SSH agent forwarding and local cross-compilation.
- **SSH check removed**: `step_ssh_check`, `DO_SSH_CHECK`, `SKIP_SSH`, `--skip-ssh-check`
  all removed. Source installs over public HTTPS require no SSH key.
- **Dead code removed**: `_find_repo_root`, `_remote_compile_on_vm`,
  `_remote_vm_cargo_install`, `_remote_install_binary_from_source`.

### Added
- **`step_setup_endpoint_service`** — sets up `networker-endpoint` as a systemd service
  on the local machine (Linux only). Asks Y/N after a local endpoint install. Handles
  `useradd`, `systemd` unit file, `enable`, `start`, and iptables port redirect
  (80→8080, 443→8443) with persistence via `netfilter-persistent` or `iptables-save`.

### Security
- **`config.json`** private IP (`172.16.32.106`) replaced with `localhost` (public repo).
- **`ci.yml`** SQL SA password annotated as ephemeral CI-only container credential.

---

## [0.12.71] – 2026-03-04 — Fix silent exit in `_cargo_progress` (set -e + pipefail)

### Fixed
- **`_cargo_progress` silent exit** (`install.sh`): four `set -e` + `pipefail` traps caused
  the installer to exit silently with no error message whenever `cargo install` was invoked:

  1. **Spinner loop** (lines ~134, ~138): `phase="$(grep … | tail -1)"` — when the build log
     is still empty (cargo just started or failed immediately), `grep` returns 1 (no matches),
     `pipefail` propagates the non-zero exit through the `$(…)`, and `set -e` kills the shell
     before the spinner prints a single character or the error banner is shown.
     Fix: added `|| true` inside each `$(…)` so the subshell always exits 0.

  2. **Success path** (line ~168): `timing="$(grep … | tail -1)"` — same trap; if the
     "Finished" line is absent from the log, silent exit instead of printing the ✓ line.
     Fix: added `|| true` inside `$(…)`.

  3. **Non-TTY path** (line ~106): `"$@" | tee "$log_file"` — with `pipefail`, a cargo
     failure causes `set -e` to exit before `PIPESTATUS` is captured, so the "build failed"
     banner is never printed and `return $rc` is never reached.
     Fix: wrap with `set +e` / `set -e` to capture `PIPESTATUS` before re-enabling errexit.

---

## [0.12.70] – 2026-03-04 — Fix source installs: drop --locked, always compile on VM

### Fixed
- **`--locked` breaks source installs** (`install.sh`): `Cargo.lock` in the v0.12.69 tag
  still had workspace package versions at `0.12.66` (lock file was not staged with the
  version bump commit). Dropped `--locked` from all four `cargo install --git` calls —
  the git tag/commit already pins the code; `--locked` is redundant and fragile when the
  lock file can drift.
- **Remote VM installs compiled locally** (`install.sh`): the source-build fallback path
  compiled the binary on the **local machine** and SCP-uploaded it, requiring local Rust +
  git and failing silently for cross-OS scenarios. Replaced `_remote_install_binary_from_source`
  with a new approach: SSH to the VM with agent forwarding (`-A`), install Rust on the VM
  if absent, then run `cargo install --git` directly on the VM. Also updated `Cargo.lock`
  to reflect the 0.12.70 workspace version.

### Added
- **`_remote_vm_cargo_install()`** (`install.sh`): new helper that SSHes to a remote VM
  with agent forwarding and runs `cargo install --git $REPO_SSH` directly on the VM.
  No local Rust or cross-compilation required; SSH agent forwards credentials so the VM
  can clone the private GitHub repo.

---

## [0.12.69] – 2026-03-04 — Azure RG reuse; remote source compilation

### Changed
- **Azure naming** (`install.sh`): resource group name no longer increments a suffix
  to avoid collision — the same RG is reused when it already exists (you may want to
  add a second VM to an existing RG, e.g. Linux then Windows endpoint in `nwk-ep-lnx-b1s`).
  Only the VM name is made unique within the RG via the new `_azure_suggest_vm_name()`
  helper (`nwk-ep-lnx-b1s-vm`, `-vm-2`, `-vm-3`, …).  When the RG already exists the
  installer prints an info message ("exists — adding VM to it") rather than a warning.

### Added
- **`_find_repo_root()`** (`install.sh`): walks up the directory tree from `install.sh`
  to locate the Cargo workspace root (presence of `Cargo.toml` + `crates/`).  Returns 1
  when invoked via `curl | bash` with no local checkout.
- **`_remote_compile_on_vm()`** (`install.sh`): when no pre-built release binary is
  available and the local OS or arch does not match the remote VM (e.g. macOS → Linux),
  tars the workspace source (excluding `target/`, `.git/`, `docs/`, `tests/`, …),
  uploads it via SCP, installs Rust on the VM if absent, and compiles natively on the
  remote machine.  Clear error + instructions are shown if `_find_repo_root()` fails.

### Fixed
- **OS/arch mismatch** (`_remote_install_binary_from_source`): previously exited with an
  error; now routes to `_remote_compile_on_vm()` so cross-OS deployments (macOS laptop →
  Linux Azure VM) succeed without requiring GitHub Actions billing to be active.

---

## [0.12.68] – 2026-03-04 — Smart Azure resource naming

### Added
- **Smart Azure resource naming** (`install.sh`): resource group and VM names are now
  auto-generated from component (`ep`/`ts`), OS (`lnx`/`win`), and size slug
  (`b1s`, `b2s`, `d2sv3`, …) — e.g. `nwk-ep-lnx-b1s` / `nwk-ep-lnx-b1s-vm`.
  If the generated name already exists as an Azure resource group, a numeric
  suffix is appended (`-2`, `-3`, …) until a free name is found.  Users can
  still type their own name at the prompt; the generated name is shown as the
  default.  This makes it safe to deploy multiple VMs of different OS/size
  combinations without name collisions.

---

## [0.12.66] – 2026-03-04 — Endpoint landing page; port 80/443 redirect; installer pipe fix; US spelling

### Added
- **HTML landing page** (`networker-endpoint`): `GET /` now returns a dark-themed
  HTML status page showing service version, hostname, uptime, all listening ports,
  supported protocols, and a table of every available endpoint with method and
  description.  Accessible at `http://VM-IP` and `https://VM-IP` via the iptables
  redirect added below.
- **Port 80/443 iptables redirect** (`install.sh`): `_remote_create_endpoint_service()`
  now adds `iptables -t nat PREROUTING` and `OUTPUT` rules redirecting TCP 80→8080
  and 443→8443 on the VM, so the landing page and `/health` are reachable on standard
  HTTP/HTTPS ports.  Rules are persisted via `netfilter-persistent` or `iptables-save`
  if available.
- **Firewall ports 80/443 opened** (`install.sh`): Azure NSG and AWS security group
  now also open TCP 80 and 443 alongside the existing 8080/8443.

### Fixed
- **Installer pipe regression** (`install.sh`): `curl -fsSL .../install.sh | bash`
  crashed with `BASH_SOURCE[0]: unbound variable` because `set -u` (nounset) is
  active and `BASH_SOURCE[0]` is unbound when bash executes from a pipe.
  Fixed: `${BASH_SOURCE[0]:-$0}` — defaults to `$0` when unbound, which equals `$0`
  in a pipe so `main` runs correctly.

### Changed
- All UK English spellings in source code comments replaced with US English:
  `behaviour`→`behavior`, `honoured`→`honored`, `Colours`→`Colors`,
  `serialisation`→`serialization`, `Customise`→`Customize`.

---

## [0.12.65] – 2026-03-04 — Installer: complementary component offer; cargo crate counter; bats tests

### Added
- **Offer tester install when only endpoint was deployed** (`install.sh`):
  if `networker-tester` is not found locally after deploying a remote endpoint,
  the installer now asks "Install networker-tester locally now?" instead of just
  printing the command.  If the user accepts, the tester is installed immediately
  (via release download or `cargo install`) and the quick test runs straight away.
- **Offer endpoint install when only tester was installed** (`install.sh`):
  new `_offer_also_endpoint()` step at the end of `display_completion()`.  When
  only `networker-tester` was installed (no endpoint anywhere), a prompt asks
  whether to: (1) install `networker-endpoint` locally, (2) deploy one on a cloud
  VM (shows the re-run command), or (3) skip.
- **Cargo spinner crate counter** (`install.sh`): `_cargo_progress()` now shows
  `[N crates]` counting up as each crate is compiled — e.g.
  `⠋  Building networker-tester  [47 crates]  Compiling tokio v1.0.0`.
  On completion the summary line shows the final count and the `Finished` timing.
- **Installer unit tests** (`tests/installer.bats`): 35 bats tests covering
  `parse_args`, `_offer_quick_test`, `_offer_also_endpoint`,
  `_remote_verify_health`, `_remote_install_binary_from_source`, and
  `step_download_release`.  Includes configurable stub executables for `ssh`,
  `curl`, `cargo`, `gh`, `scp`, `az`, and the networker binaries.
- **CI: shellcheck + bats + PSScriptAnalyzer**
  (`.github/workflows/test-installer.yml`): runs on every change to `install.sh`,
  `install.ps1`, or `tests/**`.

---

## [0.12.64] – 2026-03-04 — Installer: fix quick-test --config bug, OS mismatch detection, SSH diagnostics

### Fixed
- **`_offer_quick_test()` — removed `--config` flag** (`install.sh`): the quick-test
  invocation used `--config <file>` which does not exist in tester binaries older than
  v0.12.62.  Replaced with explicit `--target`/`--modes`/`--runs` flags built from the
  installer's state variables; works with any installed version of `networker-tester`.
  Multiple endpoint IPs (multi-region) are passed as repeated `--target` flags.
- **`_remote_install_binary_from_source()` — OS mismatch detection** (`install.sh`):
  detects the remote VM OS via SSH (`uname -s`) before compiling locally.  If
  local OS ≠ remote OS (e.g. macOS compiling for a Linux VM), the installer exits
  with a clear error instead of uploading an unrunnable binary.
- **`_remote_install_binary_from_source()` — binary execution check** (`install.sh`):
  after uploading, runs `--version` on the VM and verifies output starts with
  `networker`.  If the binary crashes (wrong arch), shows the error and exits rather
  than proceeding to start a broken service.
- **`_remote_verify_health()` — SSH diagnostics on timeout** (`install.sh`): when the
  60 s health poll times out, SSHes in and prints `systemctl status` and the last 30
  `journalctl` lines so the root cause is immediately visible.
- **`_remote_verify_health()` — correct SSH user** (`install.sh`): call sites now pass
  `azureuser` (Azure) or `ubuntu` (AWS) explicitly, preventing permission-denied errors
  on AWS VMs that used the defaulted value.

---

## [0.12.63] – 2026-03-04 — Installer: source-build fallback + VM auto-shutdown + component prompt + remote Chrome check

### Added
- **Local source-build fallback** (`install.sh`): when no GitHub release artifacts exist
  (e.g. billing limit hit), the installer compiles the binary locally via
  `cargo install --git <repo>` and uploads it to the remote VM via SCP.  No manual
  intervention needed — the fallback is transparent.
- **VM auto-shutdown policy** (`install.sh`): new prompt in Azure / AWS options asks whether
  to configure a daily auto-shutdown at 11 PM EST (04:00 UTC).  Default is yes.
  Azure uses the native `az vm auto-shutdown` policy; AWS installs a cron job on the
  instance.  The completion summary shows a green confirmation or a yellow warning if
  the user opted out, and always prints the delete/terminate commands.
- **Component selection prompt** (`install.sh`): when the script is invoked with no
  positional argument (i.e. not `install.sh tester` / `install.sh endpoint`), a
  friendly menu now asks "what do you want to install?" before displaying the plan —
  so users are no longer silently defaulted to installing both components.

### Fixed
- **Remote Chrome detection** (`install.sh`): Chrome is now checked on the **remote VM**
  (not locally) when deploying a remote tester.
  - Linux VMs: SSHes to detect Chrome; offers to install Chromium via the distro package
    manager if absent, then compiles `networker-tester` with `--features browser` accordingly.
  - Windows VMs: checks via `az vm run-command` PowerShell; shows a manual-install reminder
    if Chrome is missing.
  - `display_plan()` no longer shows a local Chrome install step when the tester is
    remote-only.

---

## [0.12.62] – 2026-03-04 — Multi-target support

### Added
- **`--target` accepts multiple values**: repeat `--target URL` for each target to test.
  All targets run the same modes and produce one HTML report with a side-by-side comparison.
- **Multi-Target Summary table**: one row per target showing run count, modes, attempts,
  succeeded, failed and duration.
- **Cross-Target Protocol Comparison table**: rows = protocols present in any run;
  columns = each target with average primary metric (ms or MB/s) and color-coded
  delta vs the first (baseline) target — green = faster/higher, red = slower/lower.
- **Per-target collapsible details**: each target's full per-protocol report is wrapped
  in a `<details>` card (open by default for ≤ 2 targets).
- **Multi-target JSON output**: `run-{ts}-1.json`, `run-{ts}-2.json` … when more than one
  target is specified; single-target runs keep the original `run-{ts}.json` name.
- **`html::save_multi()` / `html::render_multi()`**: new public API for building a single
  combined HTML report from a slice of `TestRun`s.
- **Config file `"targets"` key**: `targets: ["url1","url2"]` in JSON config;
  backward-compatible with existing `"target": "url"` key.

### Changed
- Single-target runs produce identical output to pre-0.12.62 (render_multi delegates
  to render when `runs.len() == 1`).

## [0.12.61] – 2026-03-04 — HTML report: TTFB + DCL box-plot distribution charts

### Added
- **TTFB Distribution box plot** (`Charts & Analysis` section): box-and-whisker chart
  (p5–p95 whiskers, IQR box, median line) for TTFB across all browser protocols
  (`browser1`/`browser2`/`browser3`) and synthetic pageload protocols
  (`pageload`/`pageload2`/`pageload3`). Shows variance in first-byte latency — not
  just averages — making connection-setup differences between H1/H2/H3 visible.
- **DOM Content Loaded Distribution box plot**: same box-and-whisker style for the
  DCL event across browser protocols. Reveals rendering-pipeline variance that
  differs from TTFB and total load time.

### Changed
- **Load Time Distribution box plot** now includes synthetic pageload (`pageload`,
  `pageload2`, `pageload3`) alongside real-browser data, enabling direct comparison
  of real browser vs synthetic load time variance by protocol. Title updated to
  "Load Time Distribution — All Protocols".
- No installer changes; no binary API changes.

---

## [0.12.60] – 2026-03-04 — installer: AWS EC2 support + banner version display

### Added
- **AWS EC2 remote deployment**: `install.sh` now supports deploying each component
  (`tester` / `endpoint`) to AWS EC2 in addition to local and Azure options.
- **Interactive location selection (3 options)**: after choosing which components to
  install, the installer asks *where* — `1) Locally`, `2) Remote: Azure VM`, or
  `3) Remote: AWS EC2`.
- **AWS provisioning flow**: security group creation (TCP 22, and for endpoint also
  TCP 8080/8443 + UDP 8443/9998/9999), key pair import from `~/.ssh/id_ed25519.pub`,
  dynamic Ubuntu 22.04 AMI lookup (Canonical owner), `run-instances`, wait for
  running state, SSH install, systemd service, health-check verification.
- **AWS options prompt**: 8-region menu, 4 instance types with pricing guide
  (`t3.micro` ~$7/mo to `t3.large` ~$60/mo), EC2 instance name tags — all with
  sensible defaults.
- **New CLI flags**: `--aws`, `--tester-aws`, `--aws-region`, `--aws-instance-type`,
  `--aws-endpoint-name`, `--aws-tester-name` for non-interactive scripted use.
- **Cross-cloud mixing**: tester and endpoint can be on different providers, e.g.
  `--tester-azure --aws both` deploys tester to Azure and endpoint to AWS.
- **Cleanup hints**: completion summary shows `aws ec2 terminate-instances` commands
  with the provisioned instance IDs for easy teardown.
- **Banner version display**: installer banner now shows the latest Networker release
  version (`v0.12.60`) even when run in source mode (best-effort `gh release list`
  query, fails silently if gh is not available).

### Changed
- Shared remote-install helpers (`_wait_for_ssh`, `_remote_install_binary`,
  `_remote_create_endpoint_service`, `_remote_verify_health`) now accept a `user`
  parameter and are shared between Azure (`azureuser`) and AWS (`ubuntu`) paths.
- Config file renamed from `networker-azure.json` → `networker-cloud.json` to be
  provider-agnostic.
- AWS CLI presence and login state detected in `discover_system()` and shown in
  the System Information section.
- No Rust binary changes; version bump is for the installer only.

---

## [0.12.59] – 2026-03-04 — installer: unified local + Azure remote deployment

### Changed
- **`install.sh` unified local and cloud deployment**: instead of a separate
  `deploy-azure.sh`, `install.sh` now supports deploying each component either
  locally (existing behavior) or remotely to a cloud VM.
- **Interactive location selection**: after choosing which components to install
  (`tester` / `endpoint` / `both`), the installer asks *where* to install each
  — `1) Locally` or `2) Remote: Azure VM`.
- **Azure deployment for endpoint**: provisions an Ubuntu 22.04 VM, opens TCP
  8080/8443 and UDP 8443/9998/9999 on the NSG, installs `networker-endpoint` as
  a systemd service, verifies `/health`, and writes `networker-azure.json` with
  the full test config pointing at the new VM.
- **Azure deployment for tester**: provisions a second VM (or the same VM if
  desired), installs `networker-tester`, uploads the generated config, and shows
  the SSH command to run tests from the cloud.
- **Azure options prompt**: region (8 choices), VM size (4 choices, with pricing
  guide), resource group name, and VM name — all with sensible defaults so a
  single Enter key deploys immediately.
- **New CLI flags**: `--azure`, `--tester-azure`, `--region`, `--rg`, `--vm`,
  `--tester-rg`, `--tester-vm`, `--vm-size` for non-interactive scripted use.
- **Cleanup hint**: completion summary shows `az group delete` commands for each
  provisioned resource group.
- **Azure CLI detection**: `discover_system()` detects `az` and `az account`
  login state; shown in the System Information section.
- No Rust binary changes; version bump is for the installer only.

---

## [0.12.58] – 2026-03-04 — installer: show installed version instead of stale script version

### Changed
- **`SCRIPT_VERSION` removed from both installers**: `install.sh` and `install.ps1` no longer
  carry a hardcoded version number that drifts out of sync with the binary.
- **Dynamic version display**: when `gh` is available and release mode is active, the installer
  queries the latest GitHub release tag (`gh release list --limit 1 --json tagName`) and shows
  it in the banner and installation plan.  Users now see exactly which version will be installed
  (e.g. `v0.12.58`) rather than the script's own stale version.
- **Fallback**: source-mode installs or environments without `gh` show "Networker Tester
  Installer" in the banner (no version) and "latest" in the plan steps.
- **Gist update required** after merge (sync-gist.yml is broken — run the `gh api PATCH` command
  manually).

---

## [0.12.57] – 2026-03-04 — HTML report: box-and-whisker chart + CDF chart + new observations

### Added
- **Box-and-whisker chart** ("Load Time Distribution"): new Chart 3 in the Charts & Analysis
  section.  Shows p5 whisker, Q1–Q3 IQR box, median line, and p95 whisker per protocol.
  Inspired by GLOBECOM 2016 paper which demonstrates that variance — not just average — is
  essential for comparing protocol performance.
- **CDF chart** ("Load Time CDF — All Protocols"): new Chart 4.  Empirical step-function CDF
  line per protocol (browser1/2/3 + pageload1/2/3 when both are present) with percentage
  y-axis and time x-axis.  Inspired by GLOBECOM 2016 Figures 6–7, which use CDFs to show
  whether one protocol is *consistently* faster or only occasionally faster.
- **Two new observations** in the analysis section:
  - *Lowest TTFB*: identifies which browser protocol has the lowest avg TTFB and the
    advantage over the slowest.  Motivated by CCNC 2023 which highlights TTFB as the key
    metric where H3 wins even when goodput is similar.
  - *Consistency (p95−p50)*: compares the tail-latency spread across protocols; identifies
    the most and least stable modes.  Paper 1 key finding: QUIC shows higher variance on
    well-connected networks with many small objects.

### Changed
- Existing throughput chart renumbered from Chart 3 → Chart 5.

---

## [0.12.56] – 2026-03-03 — fix browser probe: use content-length headers for transferred_bytes

### Fixed
- **`transferred_bytes` browser1 (HTTP/1.1) still under-reported after v0.12.55**: Both
  `Network.loadingFinished.encodedDataLength` (CDP) and `performance.getEntriesByType('resource')
  .encodedBodySize` (JS Performance API) give nearly identical low values (~88 KiB vs ~200 KiB
  expected for 20 × 10 KiB assets).  Root cause: Chrome tracks socket-level byte accounting
  identically for both APIs — the under-reporting is intrinsic to how Chrome measures H1.1
  connection-reuse bytes, not to the measurement API.
- **Fix**: Replaced the JS bytes evaluation entirely with content-length header summing inside
  the existing `EventResponseReceived` drain loop.  `Headers::inner()` exposes the
  `serde_json::Value`; the code reads `content-length` (H2/H3 lowercase) with fallback to
  `Content-Length` (H1.1 title-case).  The server sets `content-length` explicitly on every
  asset response, so this produces accurate, consistent byte counts for all protocols.

---

## [0.12.55] – 2026-03-03 — fix browser probe: use JS Performance API for transferred_bytes

### Fixed
- **`transferred_bytes` browser1 (HTTP/1.1) vastly under-reported**: `Network.loadingFinished.encodedDataLength`
  in Chrome's CDP is unreliable for HTTP/1.1 connections — values varied from 38 % to 55 % of expected
  bytes across runs (79–113 KiB for 20 × 10 KiB assets, vs the correct ~200 KiB).  H2/H3 were
  consistent (the same multiplexed connection makes the accounting deterministic), but H1.1 uses
  multiple serial connections where Chrome's internal byte counter drifts.
- **Fix**: Replaced the `EventLoadingFinished` CDP subscription with a single JavaScript evaluation
  of `performance.getEntriesByType('resource')` + `getEntriesByType('navigation')[0]`, summing
  `encodedBodySize` for every resource.  This value is computed post-load by the browser, is
  consistent across HTTP/1.1, H2, and H3, and does not depend on CDP event timing or ordering.
  `EventResponseReceived` is still used for resource count and per-protocol breakdown (unchanged).

---

## [0.12.54] – 2026-03-03 — installer: sync SCRIPT_VERSION to 0.12.54

### Changed
- `install.sh` and `install.ps1`: bump `SCRIPT_VERSION` from 0.12.48 → 0.12.54 so the installer
  banner matches the binary version being installed.

---

## [0.12.53] – 2026-03-03 — fix browser probe: use EventLoadingFinished for accurate transferred_bytes

### Fixed
- **`transferred_bytes` in browser probes**: Switched from `EventResponseReceived.response.encoded_data_length`
  to `EventLoadingFinished.encoded_data_length` for byte accounting.
  `Network.responseReceived` fires when headers arrive — its `encodedDataLength` is unreliable for
  multiplexed protocols (H2/H3) because it captures connection-level bytes at an arbitrary point
  mid-transfer, producing bimodal values (e.g. browser2 alternating between ~1 KiB and ~86 KiB
  across runs). `Network.loadingFinished` fires after the full response body is received and gives
  an accurate per-request byte count for all protocols.
  Protocol and resource-count accounting still uses `EventResponseReceived` (unchanged).

---

## [0.12.52] – 2026-03-03 — HTML report: browser comparison table, SVG charts, analysis, favicon fix

### Added
- **Browser Protocol Comparison table**: New aggregated summary table showing browser1/2/3 side-by-side
  (Avg TTFB, DCL, Load, p50, Min, Max, Avg Resources, Avg Bytes) — mirrors the existing pageload comparison table.
- **SVG bar charts** (self-contained, no CDN): Three horizontal bar charts embedded in the HTML report:
  1. "Page Load Time — All Protocols": merged browser + pageload avg load times, slowest→fastest
  2. "Browser TTFB / DCL / Load Breakdown": interleaved bars per browser mode with TTFB/DCL/Load breakdown
  3. "Throughput by Protocol (MB/s)": shown only when download/upload data is present
- **Analysis observations**: Auto-generated bullet list including fastest browser mode, H3 vs H2
  improvement percentage, real-browser vs synthetic overhead, and resource protocol breakdown.

### Fixed
- **Favicon 404**: Added `<link rel="icon" href="data:,">` to `<head>` in both `/browser-page` HTML
  endpoints (HTTP/2 `routes.rs` and HTTP/3 `http3_server.rs`). Chrome no longer auto-requests
  `/favicon.ico`, eliminating 404 noise from endpoint server debug logs.

---

## [0.12.51] – 2026-03-03 — fix browser probe: close browser gracefully to suppress chromiumoxide warning

### Fixed
- **`browser` probe**: Call `browser.close().await` before aborting the CDP handler task.
  Without this, chromiumoxide logs `WARN: Browser was not closed manually, it will be
  killed automatically in the background` on every probe run.

---

## [0.12.50] – 2026-03-03 — fix browser3: SPKI hash pinning replaces unreliable platform cert stores

### Fixed
- **`browser3` QUIC cert trust (cross-platform)**: Replaced the entire platform cert-store
  approach (NSS db on Linux, macOS Keychain, Windows Root store) with
  `--ignore-certificate-errors-spki-list=<spki_hash>`.  Chrome's QUIC TLS cert verifier
  **does** honor this flag (unlike `--ignore-certificate-errors`), making it the only
  reliable way to accept self-signed certs over QUIC/H3 on all platforms.
- **SPKI hash computation**: Added `compute_spki_sha256_base64()` which fetches the server
  cert via a custom-verifier TLS handshake, extracts the DER-encoded SubjectPublicKeyInfo
  bytes via `x509-parser`, and Base64-encodes their SHA-256 hash — exactly the format
  Chrome's SPKI list expects.
- Removed dead code: `der_to_pem`, `CertTrustGuard`, `install_cert_trust`,
  `install_cert_trust_inner` (all platform variants), `which_command`,
  `compute_cert_sha256_hex`.  The new SPKI approach is ~15 lines vs ~290 lines of
  platform-specific cert management.

---

## [0.12.49] – 2026-03-03 — fix browser3 Linux cert trust: import into ~/.pki/nssdb

### Fixed
- **`browser3` Linux cert trust**: `install_cert_trust_inner` was importing the server
  cert into `profile_dir/Default/` (the Chrome profile's data directory).  Chrome does
  **not** read TLS certificates from the profile directory — it reads `~/.pki/nssdb` when
  `--use-system-cert-store` is set.  Changed the NSS db path to `~/.pki/nssdb`; certutil
  creates the directory if absent.  The cert is still removed by `CertTrustGuard::drop`
  on probe completion.

---

## [0.12.48] – 2026-03-03 — installer: certutil shown in plan and confirmed before install

### Changed
- **Installer (`install.sh`)**: `certutil` (NSS tools) now appears in the Installation Plan
  when Chrome is available on Linux and `certutil` is not yet installed.  Shown in both the
  release-download and source-build plan sections.
- **Installer (`install.sh`)**: `step_ensure_certutil` now asks `Y/n` before installing
  (`ask_yn`) instead of silently running `apt-get install`.  Skipping prints a warning that
  browser3 will fall back to H2.
- **Installer (`install.sh`)**: `CERTUTIL_AVAILABLE` flag detected in `discover_system`
  (via `command -v certutil`) so the plan display knows whether to show the step.

---

## [0.12.47] – 2026-03-03 — installer: fix SYS_OS case check in step_ensure_certutil

### Fixed
- **Installer (`install.sh`)**: `step_ensure_certutil` compared `"$SYS_OS" == "linux"`
  (lowercase) but `SYS_OS` is set from `uname -s` which returns `"Linux"` (capital L).
  The guard returned immediately on every Linux system, so certutil was never installed.
  Fixed to `"Linux"`.

---

## [0.12.46] – 2026-03-03 — installer: certutil check runs for release downloads too

### Fixed
- **Installer (`install.sh`)**: `step_ensure_certutil()` was placed inside the `else`
  (source-build) branch of `main()`, so it was never reached when using the default
  release-binary download path.  Moved the call outside the `if/else` so it runs
  regardless of install method, whenever Chrome is available.

---

## [0.12.45] – 2026-03-03 — installer: ensure certutil for existing Chrome installs

### Fixed
- **Installer (`install.sh`)**: `libnss3-tools` / `nss-tools` was only installed inside
  `step_install_chrome()`, which is skipped when Chrome is already present.  Added a new
  `step_ensure_certutil()` function that runs whenever Chrome is available (pre-existing
  **or** freshly installed), checks for `certutil` with `command -v`, and installs the
  appropriate NSS package if missing.  This is a no-op on macOS (uses `security(1)`) and
  when `certutil` is already installed.
- **Installer (`install.ps1`, `install.sh`)**: bump `$ScriptVersion` / `SCRIPT_VERSION` to
  `0.12.45`.

---

## [0.12.44] – 2026-03-03 — installer: libnss3-tools; Windows browser3 cert trust

### Added
- **Installer (`install.sh`)**: automatically installs the NSS `certutil` helper tool
  alongside Chromium for all supported Linux package managers — `libnss3-tools` (apt-get),
  `nss-tools` (dnf/apk), `nss` (pacman), `mozilla-nss-tools` (zypper).  `certutil` is
  required for `browser3` QUIC cert trust on Linux; without it the probe silently falls
  back to H2.
- **`browser3` cert trust on Windows**: uses PowerShell to add the server's self-signed cert
  to `CurrentUser\Root` temporarily (no extra package installation required — Windows
  Certificate Store and PowerShell 5.1+ are built-in).  The cert is removed automatically
  via `CertTrustGuard::drop` after the probe completes.
- **Installer (`install.ps1`)**: bump `$ScriptVersion` to match binary version `0.12.43`
  (was stale at `0.12.38`).

---

## [0.12.43] – 2026-03-03 — browser3: fix ERR_QUIC_PROTOCOL_ERROR when cert trust fails on Linux

### Fixed
- **`browser3` fails with `ERR_QUIC_PROTOCOL_ERROR`** when `certutil` is not installed on Linux
  (or cert trust installation fails for any reason).  Root cause: `--origin-to-force-quic-on`
  was unconditionally added, causing Chrome to attempt QUIC even in the cert-trust-failed
  fallback path.  QUIC TLS does **not** honor `--ignore-certificate-errors`, so the handshake
  fails outright instead of falling back gracefully to H2.
  Fix: gate `--origin-to-force-quic-on` on cert trust success (`_cert_trust_guard.is_some()`).
  When cert trust fails, browser3 falls back to H2 (same as pre-v0.12.41 behavior) rather
  than erroring.

---

## [0.12.42] – 2026-03-03 — fix dead_code warning on Linux

### Fixed
- Suppressed `dead_code` compiler warning for `compute_cert_sha256_hex` on Linux CI builds.
  The function is macOS-only (used by `security delete-certificate -Z`); gated with
  `#[cfg(target_os = "macos")]`.

---

## [0.12.41] – 2026-03-03 — browser3: fix Chrome arg double-dash bug (root cause of H3 regression)

### Fixed
- **`browser3` always reporting `proto=h2` despite all cert-trust fixes** — definitive root cause
  found: chromiumoxide's `BrowserConfig::arg(&str)` treats the entire string as the arg *key*
  and prepends `"--"` at launch time.  All our calls used `"--flag=value"` format, producing
  `"----flag=value"` which Chrome silently ignores.  Affected flags:
  - `--origin-to-force-quic-on` (the QUIC-forcing flag) — **was always ignored**
  - `--use-system-cert-store` (Chrome Root Store bypass)
  - `--ignore-certificate-errors`
  - `--disable-quic` (browser2)
  - All manually re-added DEFAULT_ARGS in the `disable_default_args()` block
  Fix: strip `--` prefix from all raw arg strings; use chromiumoxide's typed
  `.no_sandbox()` / `.new_headless_mode()` helpers instead of manual arg strings for
  sandbox/headless so chromiumoxide's Arg machinery produces correct `--` flags.
- **Chrome 127+ Chrome Root Store** — even with correct arg format, Chrome 127+ ignores
  `security add-trusted-cert` because it defaults to its own Chrome Root Store (not the
  macOS trust store).  Added `--use-system-cert-store` (now correctly passed) to force Chrome
  to use the platform verifier which reads `login.keychain-db`.

### Result
All three protocols are now consistently correct across 3 runs:
- `browser1` → `http/1.1×22` ✓
- `browser2` → `h2×22` ✓
- `browser3` → `h3×21` ✓ (was always `h2` before this fix)

---

## [0.12.40] – 2026-03-03 — browser3: fix QUIC cert trust (CA:TRUE) and remove --ignore-certificate-errors when cert is trusted

### Fixed
- **`browser3` QUIC TLS still failing with `CERTIFICATE_VERIFY_FAILED`** even after v0.12.39
  installed the server cert in the macOS Keychain.
  Root cause (definitive, from Chrome net-log analysis):
  1. Chrome's cert verifier (BoringSSL) requires trust anchors to have `basicConstraints: CA:TRUE`
     per RFC 5280 §4.2.1.9.  The endpoint used `rcgen::generate_simple_self_signed()` which
     produces a plain leaf cert with **no** `basicConstraints` extension.  Adding a non-CA leaf
     cert to the macOS Keychain as `trustRoot` is silently ignored by Chrome's QUIC verifier —
     it accepted the Keychain entry for display but refused to use the cert as a chain anchor,
     producing `CERTIFICATE_VERIFY_FAILED`.
  2. `--ignore-certificate-errors` was still added to all browser modes including browser3.
     Chrome discards `Alt-Svc` hints from connections with overridden cert errors, so QUIC was
     never scheduled even though cert trust installation logged success.
  Fix:
  - **Endpoint** (`networker-endpoint`): switched cert generation from
    `generate_simple_self_signed()` to `CertificateParams::new()` with
    `is_ca = IsCa::Ca(BasicConstraints::Unconstrained)`.  The server cert now carries
    `basicConstraints: CA:TRUE` and can serve as a proper trust anchor.
  - **Tester** (`networker-tester`, `runner/browser.rs`): `--ignore-certificate-errors` is now
    **omitted** for browser3 when cert trust installation succeeded (`CertTrustGuard` is `Some`).
    Chrome sees no cert errors → `Alt-Svc` hint is processed → QUIC session scheduled →
    main navigation uses H3.  Falls back to adding the flag (H2 only) if cert trust failed.
  - Removed the `--log-net-log` diagnostic flag that was left in browser3 in v0.12.39.

---

## [0.12.39] – 2026-03-03 — browser3: install server cert as trusted root before Chrome launch to enable QUIC/H3

### Fixed
- **`browser3` still shows `proto=h2` with zero QUIC packets** in all previous attempts
  (v0.12.35–v0.12.38).
  Root cause (definitive): Chrome silently discards `Alt-Svc` hints from connections where
  certificate errors were *overridden* via `--ignore-certificate-errors`.  This is a deliberate
  Chrome security policy: upgrading a "broken" (cert-error) connection to QUIC would allow a
  MITM to force protocol negotiation.  Even with the CDP `Security.setIgnoreCertificateErrors`
  command and the SPKI-list flag, Chrome never scheduled a background QUIC probe because the
  initial H2 warmup connection was flagged as having an overridden cert error.
  Fix: **install the server's leaf certificate as a temporarily-trusted root before Chrome
  launches**, so Chrome sees a fully-authenticated connection, processes the `Alt-Svc` hint,
  schedules a background QUIC session, and the main navigation uses H3.
  - **macOS**: `security add-trusted-cert -d -r trustRoot -k login.keychain-db <cert.pem>`
    before Chrome launch; `security delete-certificate -Z <SHA-256>` in RAII `Drop` guard.
  - **Linux**: `certutil -A -d sql:<profile>/Default/ -t CT,, -n <name> -i <cert.pem>` into
    the Chrome user-data-dir NSS database (populated before `Browser::launch` so Chrome reads
    it at startup); cert name uses PID for uniqueness; cleanup in RAII `Drop` guard.
  - Falls back gracefully (logs a warning, browser3 may show H2) if `certutil` is absent
    (`apt install libnss3-tools`), or if the macOS keychain is locked.
- Replaced `fetch_spki_hash` + `--ignore-certificate-errors-spki-list` (did not work in
  `--headless=new` mode) with `fetch_cert_der` used by the cert trust path.
- Re-enabled `--origin-to-force-quic-on=<host>:<port>` as belt-and-suspenders alongside
  the Alt-Svc warmup (cert now trusted, so the forced probe succeeds).
- Increased browser3 post-warmup sleep from 500 ms to 1 s for more reliable QUIC session
  establishment on higher-latency links.
- Added two new unit tests: `der_to_pem_has_header_and_footer`,
  `compute_cert_sha256_hex_is_64_uppercase_hex`.

---

## [0.12.38] – 2026-03-03 — browser3: remove forced-QUIC flags to fix Alt-Svc H3 negotiation

### Fixed
- **`browser3` still shows `proto=h2`** after CDP cert override (v0.12.37).
  Root cause: `--origin-to-force-quic-on=<ip>:<port>` causes Chrome's network stack to probe
  the origin with a raw QUIC connection during initialization — **before** the CDP
  `Security.setIgnoreCertificateErrors(true)` command is applied.  This early probe fails the
  TLS handshake (cert not yet trusted) and Chrome permanently marks the origin as "QUIC broken"
  for the duration of the session.  Subsequent Alt-Svc hints from the warmup navigation are
  then silently discarded for that broken origin.  Confirmed via server logs: Chrome sent zero
  QUIC packets to the server in all v0.12.37 runs.
  Fix: **remove `--origin-to-force-quic-on` and `--enable-quic`** from browser3's Chrome flags.
  QUIC is Chrome's default; without the forced-probe flag, no early QUIC attempt is made.
  H3 is triggered naturally via the server's `Alt-Svc: h3=":PORT"` warmup response: Chrome
  schedules a background QUIC session, the CDP cert override (already active) makes the TLS
  handshake succeed, and the main navigation uses the established QUIC session → H3.

---

## [0.12.37] – 2026-03-03 — browser3: CDP Security.setIgnoreCertificateErrors to fix QUIC cert trust in --headless=new

### Fixed
- **`browser3` failed with `net::ERR_CONNECTION_REFUSED`** (v0.12.36 regression): the localhost
  URL approach was broken because Chrome internally hardcodes `localhost` → `127.0.0.1` and
  silently ignores `--host-resolver-rules` for that hostname.  Reverted to using the actual
  server IP in the URL; removed `--host-resolver-rules`, `--allow-insecure-localhost`.
- **`browser3` root cause identified**: Chrome's `--headless=new` mode (Chrome 112+) does NOT
  apply `--ignore-certificate-errors-spki-list` to QUIC/H3 TLS connections.  The flag is
  handled in the browser process but QUIC TLS runs in the network service process which does
  not receive it in headless mode.  Fix: send `Security.setIgnoreCertificateErrors(true)` via
  CDP immediately after page creation.  The DevTools Protocol command applies to ALL
  connections made by that page, including QUIC, reliably in `--headless=new`.
  `--ignore-certificate-errors-spki-list` is kept as belt-and-suspenders for older Chrome.

### Changed
- browser3: `INFO` log `CDP Security.setIgnoreCertificateErrors(true) applied` for visibility.

---

## [0.12.36] – 2026-03-02 — browser3: localhost URL rewrite + host-resolver-rules to force H3

### Fixed
- **`browser3` still showed `proto=h2`** even after Alt-Svc + warmup navigation (v0.12.35).
  Root cause: Chrome's QUIC TLS path is stricter about SAN matching than the TCP/TLS path.
  When connecting to `172.16.32.106:8443`, Chrome's QUIC stack rejects the handshake if the
  cert has no SAN matching that exact IP — even with `--ignore-certificate-errors-spki-list`
  pinning the cert.  (`--ignore-certificate-errors` alone does not bypass QUIC TLS failures.)
  Fix: browser3 now **rewrites the navigation URL to `https://localhost:<port>`** and adds:
  - `--host-resolver-rules=MAP localhost <actual_ip>` — Chrome routes `localhost` DNS to the
    real server IP, so the connection reaches the endpoint.
  - `--allow-insecure-localhost` — Chrome trusts self-signed certs on localhost without
    requiring a SPKI pin (the cert always has `localhost` as a SAN).
  - `--origin-to-force-quic-on=localhost:<port>` — QUIC is forced on the localhost origin.
  - `--ignore-certificate-errors-spki-list=<hash>` — belt-and-suspenders SPKI pin still
    computed and passed.
  - `fetch_spki_hash()` log level promoted from `DEBUG` to `INFO` for visibility.
  - Warmup sleep increased from 200 ms to 500 ms to give Chrome more time to establish the
    QUIC session from the Alt-Svc hint.

---

## [0.12.35] – 2026-03-02 — browser3: Alt-Svc + warmup navigation to reliably force H3

### Fixed
- **`browser3` always showed `proto=h2`** despite `--origin-to-force-quic-on` and SPKI hash
  pinning.  Root cause: on a LAN, Chrome's TCP connection is established in <1 ms and wins
  the TCP-vs-QUIC race before the QUIC handshake completes.
  Two changes together fix this:
  1. **Endpoint now serves `Alt-Svc: h3=":PORT"; ma=86400`** on every HTTPS response
     (Chrome ignores Alt-Svc from plain-HTTP origins).  Chrome caches the hint and opens a
     QUIC connection in the background after the first response.
  2. **browser3 probe does a warmup navigation** to `/health` before the measured page
     load: the warmup receives the Alt-Svc header, Chrome establishes the QUIC session
     (200 ms sleep), and the main navigation uses the already-open QUIC connection → H3.
     Network events are subscribed **after** the warmup so only the main navigation's
     resources are counted.
- **Self-signed cert now includes the server's primary LAN IP** (e.g. `172.16.32.106`) as
  an IP SAN (rcgen auto-detects IP strings).  Detected via the UDP-socket routing trick —
  no packets sent.  Eliminates the hostname-mismatch cert error for clients connecting via
  LAN IP, reducing reliance on SPKI-pin bypass.

### Changed
- `build_router()` → `build_router(h3_port: Option<u16>)` in `networker-endpoint`.
  Callers pass `Some(https_port)` when the `http3` feature is compiled in, `None` otherwise.

---

## [0.12.34] – 2026-03-02 — browser3: SPKI hash pinning for QUIC cert trust; browser1: plain-HTTP URL

### Fixed
- **`browser3` (HTTP/3 QUIC)** probe was always showing `proto=h2` because Chrome's QUIC
  TLS stack does not honor `--ignore-certificate-errors` for self-signed certs, causing
  fallback to H2.  Fix: before launching Chrome, the probe now performs a plain TLS
  connection to the target, extracts the leaf certificate's SubjectPublicKeyInfo (SPKI)
  DER bytes, SHA-256-hashes them, and passes the base64 hash to Chrome via
  `--ignore-certificate-errors-spki-list=<hash>`.  Chrome trusts the cert over QUIC and
  successfully negotiates H3.  If the SPKI fetch fails a warning is logged and Chrome
  may still fall back to H2.
- **`browser1` (HTTP/1.1)** probe was always showing `proto=h2` because the
  `--disable-http2` Chrome flag is silently ignored in Chrome ≥ 136's network service
  subprocess.  Fix: the probe now rewrites the target URL to `http://` (plain HTTP) and
  auto-derives the HTTP port (8443 → 8080, 443/default → 80, other ports unchanged).
  Plain HTTP has no TLS ALPN negotiation, so Chrome physically cannot negotiate H2 or H3.

### Added
- `fetch_spki_hash()` helper inside `mod real` — extracts and hashes the server's SPKI
  for Chrome cert-pinning (uses `sha2 0.10` + `base64 0.22`, both already optional deps
  under the `browser` feature)
- `build_browser1_url()` public helper — rewrites a URL to `http://` with port mapping
- Unit tests for `build_browser1_url` (4 new tests: port mapping, param encoding,
  non-standard port preserved, standard port omitted)

---

## [0.12.33] – 2026-03-02 — pageload/browser as all-3 shortcuts; pageload1 alias; script version sync

### Changed
- **`pageload` mode** now expands to `pageload1 + pageload2 + pageload3` automatically —
  a single `--modes pageload` runs all three HTTP versions for a full comparison
- **`browser` mode** now expands to `browser1 + browser2 + browser3` automatically —
  a single `--modes browser` runs all three forced-protocol variants
- Deduplication: if a mode appears multiple times after expansion it is only run once
- `SCRIPT_VERSION` in `install.sh` and `install.ps1` now always matches the binary
  workspace version (previously drifted; was 0.12.25/0.12.24, now 0.12.33)

### Added
- **`pageload1`** mode — explicit alias for the HTTP/1.1 pageload probe (equivalent to
  the old `pageload` single-mode behavior); consistent with `pageload2` / `pageload3`

---

## [0.12.32] – 2026-03-02 — Add browser1/browser2/browser3 forced-protocol probe modes

### Added
- **`browser1`** probe mode — real headless Chromium forced to HTTP/1.1
  via `--disable-http2`
- **`browser2`** probe mode — real headless Chromium forced to HTTP/2
  via `--disable-quic` (prevents H3 upgrade); requires HTTPS
- **`browser3`** probe mode — real headless Chromium forced to HTTP/3 QUIC
  via `--enable-quic --origin-to-force-quic-on=<host>:<port>`; requires HTTPS
- All three appear in the Protocol Comparison table alongside
  `pageload`/`pageload2`/`pageload3`
- HTML "Browser Results" table gains a **Mode** column so all four browser
  variants are shown together with their mode labels
- Terminal log line now shows `[browser1]`/`[browser2]`/`[browser3]` instead of
  hardcoded `[browser]`

---

## [0.12.31] – 2026-03-02 — Fix browser probe root: unique per-run user-data-dir

### Fixed
- **`runner/browser.rs`**: after the `runuser` fix (v0.12.30), Chrome now
  runs as `SUDO_USER` but was still failing with
  `readlink(/tmp/chromiumoxide-runner/SingletonLock): Permission denied` —
  the `/tmp/chromiumoxide-runner` directory was left over from a previous
  root run and is owned by root, so the non-root user Chrome now runs as
  cannot write to it
- Fix: compute a unique per-run profile directory
  (`/tmp/networker-chrome-profile-<pid>`) and pass it to chromiumoxide via
  `BrowserConfig::builder().user_data_dir()`, bypassing the default
  `/tmp/chromiumoxide-runner` path; Chrome (running as `SUDO_USER`) creates
  this fresh directory itself in world-writable `/tmp`
- RAII guard (`ProfileDirGuard`) cleans up the profile directory on all
  return paths so `/tmp` is not littered with leftover profiles

---

## [0.12.30] – 2026-03-02 — Fix browser probe as root: launch Chrome via runuser as SUDO_USER

### Fixed
- **`runner/browser.rs`**: snap chromium's internal launcher script
  (`chromium.launcher`) strips `--no-sandbox` when the process UID is 0,
  causing Chrome to always exit with
  `Running as root without --no-sandbox is not supported`
  regardless of flags passed — previous workarounds (`XDG_RUNTIME_DIR`,
  `--disable-setuid-sandbox`, pre-creating `/run/user/0`) could not
  overcome this launcher-level filter
- Fix: when running as root and `SUDO_USER` is set (i.e. invoked via
  `sudo`), generate a temporary wrapper script that re-executes the
  Chrome binary as the original non-root user via
  `runuser -u <SUDO_USER> -- <chrome> "$@"`; Chrome then runs as a
  normal user and the snap root check never triggers
- `runuser` (util-linux) is used without a password when called by root
  and is available on all mainstream Linux distributions

---

## [0.12.29] – 2026-03-02 — Fix browser probe root/snap: pre-create /run/user/0

### Fixed
- **`runner/browser.rs`**: `XDG_RUNTIME_DIR=/tmp` (v0.12.28) did not prevent
  the `mkdir: cannot create directory '/run/user/0': Permission denied` error
  because snap-confine hardcodes `/run/user/<uid>` and ignores `XDG_RUNTIME_DIR`
- Fix: pre-create `/run/user/<uid>` with mode `0700` before launching Chrome
  (only when the directory does not already exist); as root we have permission
  to create it, so snap-confine's subsequent `mkdir` succeeds (directory already
  exists) and Chrome proceeds to accept `--no-sandbox`

---

## [0.12.28] – 2026-03-02 — Fix browser probe when running as root (sudo)

### Fixed
- **`runner/browser.rs`**: browser probe failed with
  `Running as root without --no-sandbox is not supported` when
  `networker-tester` was invoked via `sudo`
- Root cause: snap Chromium tries to create `/run/user/0` (XDG runtime dir
  for root) which doesn't exist; this caused the snap wrapper to fail before
  Chrome could read the `--no-sandbox` flag
- Set `XDG_RUNTIME_DIR=/tmp` at launch time when the variable is not already
  set (Unix only); this allows the snap setup to proceed
- Added `--disable-setuid-sandbox` alongside `--no-sandbox` for belt-and-braces
  compatibility with Chrome's setuid sandbox check when running as root

---

## [0.12.27] – 2026-03-02 — Sync Cargo workspace version to match CHANGELOG/tag version

### Changed
- Cargo workspace `version` bumped from `0.12.13` to `0.12.27` to re-align
  `networker-tester --version` / `networker-endpoint --version` with the
  CHANGELOG and git tag versioning (they had drifted apart over several
  installer-only releases that did not bump the Cargo version)

---

## [0.12.26] – 2026-03-02 — Upgrade chromiumoxide 0.7 → 0.9 to fix CDP crash on Chrome 145+

### Fixed
- **`runner/browser.rs`**: browser probe crashed with
  `data did not match any variant of untagged enum Message` on Ubuntu 22.04
  with Chromium 145 (snap); root cause was chromiumoxide 0.7 failing to
  deserialize new CDP message types introduced in newer Chrome versions,
  which killed the WebSocket handler and caused all subsequent page operations
  to fail
- Upgraded `chromiumoxide` dependency from `0.7` to `0.9` (latest: 0.9.1);
  the `tokio-runtime` feature flag was removed in 0.9 (tokio is now the
  default runtime), so the dependency entry is simplified accordingly

---

## [0.12.25] – 2026-03-02 — Fix Chrome prompt skipped when git is installed

### Fixed
- **`install.sh`**: `PKG_MGR` was only populated inside the `git not installed` else-branch,
  so on machines where git is already present the package manager was never detected;
  this caused the Chrome Y/N prompt to be silently skipped and the plan to show
  `(not installed — https://www.google.com/chrome/)` instead of asking
- Package manager detection now always runs, independent of git availability

---

## [0.12.24] – 2026-03-02 — Installer skips Chrome prompt when only installing endpoint

### Fixed
- **`install.sh`** / **`install.ps1`**: Chrome detection, plan display, and install prompt
  are now skipped entirely when only `networker-endpoint` is being installed
  (`install.sh endpoint`); Chrome/browser probe is only relevant to `networker-tester`

---

## [0.12.23] – 2026-03-02 — Installer always asks before installing Chrome

### Fixed
- **`install.sh`** / **`install.ps1`**: Chrome/Chromium was installed silently whenever
  a package manager was available and the user chose "1 — Proceed with default installation";
  the user was only asked in the "2 — Customize" path
- Now the installer always prompts `[Y/n]` for Chrome when it is not found, regardless
  of whether the user chose Proceed or Customize

---

## [0.12.22] – 2026-03-02 — Fix browser probe: add `/browser-page` HTML endpoint

### Fixed
- **`networker-endpoint` `/browser-page` route** (HTTP/1.1, HTTP/2, HTTP/3): new endpoint
  returns a real HTML page with `<img>` tags for each asset; the browser's `load` event
  fires only after all image requests settle, giving accurate end-to-end page load timing
- **`runner/browser.rs` `build_page_url()`**: previously pointed to `/page` which returns
  a JSON manifest — a real browser displays the JSON as text and never fetches the listed
  assets (`res=2 bytes=178`); now points to `/browser-page`
- **`build_page_url()`**: fixed query param name `size=` → `bytes=` to match the endpoint's
  `PageParams` struct (was silently ignored, defaulting to 10 KiB regardless)

---

## [0.12.21] – 2026-03-02 — Fix browser probe Chrome detection on Windows

### Fixed
- **`runner/browser.rs` `find_chrome()`**: added Windows path detection; previously only
  Linux and macOS paths were searched, so Chrome installed via the installer was never
  found at runtime on Windows
- Now checks `%PROGRAMFILES%\Google\Chrome\Application\chrome.exe`,
  `%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe`, and
  `%PROGRAMFILES(X86)%\...` before falling through to Linux/macOS paths

---

## [0.12.20] – 2026-03-02 — Fix: `--features browser` applied only to networker-tester

### Fixed
- **`install.sh`** / **`install.ps1`**: `--features browser` was incorrectly passed to
  `networker-endpoint` as well as `networker-tester`; the `browser` feature does not
  exist in the endpoint crate, causing `cargo install` to fail with
  "the package 'networker-endpoint' does not contain this feature: browser"
- Fixed by guarding the flag: only added when the binary being installed is
  `networker-tester`

---

## [0.12.19] – 2026-03-02 — Installer detects Chrome; browser feature follows Chrome availability

### Changed
- **`browser` feature removed from `default`** in `Cargo.toml`; it was briefly added in
  v0.12.18 but is now driven by the installer instead (same pattern as git/MSVC detection)
- **`release.yml`**: pre-built release binaries are always built with `--features browser`
  (Chrome is a runtime dep only; CI build machines need no Chrome installed)
- **`install.sh`** (SCRIPT_VERSION 0.12.19): detects Chrome/Chromium at startup via
  `NETWORKER_CHROME_PATH` env var or standard macOS/Linux paths; shows in System
  Information table; offers to install via the system package manager if absent;
  user can toggle in the Customize flow; adds `--features browser` to `cargo install`
  only when Chrome is present after the install step
- **`install.ps1`** (SCRIPT_VERSION 0.12.19): same Chrome detection and offer via
  `winget install Google.Chrome`; Chrome status shown in System Information table;
  `--features browser` added to `cargo install` only when Chrome is available

---

## [0.12.18] – 2026-03-02 — Make `browser` probe a default feature

### Changed
- **`browser` feature is now included in `default`** alongside `http3`; previously it
  had to be explicitly opt-in at compile time (`--features browser`), which meant the
  install script and pre-built binaries did not include it
- The probe still fails gracefully with a clear message if Chrome/Chromium is not
  installed at runtime; no behavioral change for users without Chrome

---

## [0.12.17] – 2026-03-02 — Expand README: all probe modes + page-load comparison guide

### Changed
- **`README.md`**: Probe Modes table now lists all 18 probe modes (previously only 9),
  including `dns`, `tls`, `native`, `curl`, `webdownload`, `webupload`, `udpdownload`,
  `udpupload`, `pageload2`, `pageload3`, and `browser`
- **`README.md`**: new "Page-Load Protocol Comparison (H1 / H2 / H3 / Browser)" section
  shows the exact command for comparing protocol performance across all four page-load
  probes against a local endpoint
- **`README.md`**: updated installer description and removed stale Known Limitations row

---

## [0.12.16] – 2026-03-02 — Fix MSVC install: wait for background VS installation

### Fixed
- **`install.ps1`**: the VS bootstrapper winget downloads (~4 MB) launches the real
  ~2 GB installation as a background process; the previous version called `.Trim()`
  on `$null` (vswhere output before VS had registered) causing a script crash
- **`install.ps1`**: added `--wait` to the VS installer override so it blocks until
  fully complete; added a polling loop (15 s intervals, 15 min timeout) as a
  belt-and-suspenders fallback in case `--wait` is insufficient
- **`install.ps1`**: all `vswhere` output is cast via `[string]` before null/whitespace
  checks so the script never crashes on empty output
- **`install.ps1`**: wrapped `vcvars64.bat` environment loading in `try/catch`; on
  failure, shows a clear "reopen terminal and re-run" message instead of crashing

---

## [0.12.15] – 2026-03-02 — Detect missing build tools before cargo install

### Added
- **`install.ps1`**: detects Visual C++ Build Tools via `vswhere.exe` before
  attempting `cargo install`; if absent and `winget` is available, auto-offers
  `winget install Microsoft.VisualStudio.2022.BuildTools` (~2-3 GB, VC++ workload);
  after install, sources `vcvars64.bat` so `link.exe` is on PATH for the current
  session without reopening the terminal
- **`install.ps1`**: VC++ build tools status shown in System Information table
  (`installed ✓` or `not installed`)
- **`install.ps1`**: customize flow lets user toggle the VC++ install step; shows
  the exact `winget` command to run manually if user opts out
- **`install.ps1`**: pre-flight warning in cargo install step if MSVC still absent —
  prints the exact `winget` one-liner to fix it
- **`install.sh`**: pre-flight check in `step_cargo_install` warns when no C linker
  (`cc`/`gcc`/`clang`) is found and prints the correct install command
  (`xcode-select --install` on macOS; `build-essential`/`gcc`/`base-devel` on Linux)

---

## [0.12.14] – 2026-03-02 — Optional git installation in both installers

### Added
- **`install.sh`**: detects installed package manager (`brew`, `apt-get`, `dnf`,
  `pacman`, `zypper`, `apk`) and offers to install git automatically when git is
  absent in source mode; new `detect_pkg_manager()` helper
- **`install.ps1`**: detects `winget` and offers to install `Git.Git` automatically
  when git is absent in source mode; refreshes `$env:PATH` after install so git
  is usable without reopening a terminal
- Both installers: git version shown in System Information table; installation plan
  includes a numbered "Install git" step when git is absent + package manager found
- Customize flow in both installers lets the user toggle the git install step

### Fixed
- **`install.sh`**: `CARGO_NET_GIT_FETCH_WITH_CLI=true` is now only set when `git`
  is found on PATH (mirrors the PR #84 fix already applied to `install.ps1`)

---

## [0.12.13] – 2026-03-02 — Fix Windows source-mode install when git is absent

### Fixed
- **`install.ps1`**: `CARGO_NET_GIT_FETCH_WITH_CLI=true` is now only set when
  `git.exe` is found on PATH. Previously, setting it unconditionally caused
  `cargo install` to fail with "program not found" on machines that have SSH
  (and Rust) but not Git for Windows. Without the flag, cargo falls back to its
  built-in libgit2, which reads SSH keys from `%USERPROFILE%\.ssh\` directly.
- **`install.ps1`**: `cargo install` exit code is now checked; a non-zero exit
  immediately prints a clear error (with a Git-for-Windows hint if git was absent)
  and exits — previously the installer reported success even when cargo failed.

---

## [0.12.12] – 2026-03-02 — Binary release downloads + cross-compile CI

### Added
- **Release workflow now builds and uploads native binaries** for 4 platforms:
  `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`. Each release asset is a `.tar.gz` (Unix) or `.zip`
  (Windows) containing the single binary. Archives are attached to every GitHub
  release going forward.
- **Both installers now auto-detect install mode**:
  - **release mode** (default when available): if `gh` CLI is installed and
    `gh auth login` has been run, and the current platform has a release binary,
    the installer downloads and extracts the pre-built binary via
    `gh release download --latest` (~10 s total)
  - **source mode** (fallback): compiles from source via `cargo install` as before
    (~5-10 min); chosen automatically when `gh` is not available or
    `--from-source` / `-FromSource` flag is passed
- **`install.sh`**: new `--from-source` flag; system info shows `gh CLI: authenticated ✓`
  when release mode is available; plan section shows install method + target triple;
  customize flow offers install method choice
- **`install.ps1`**: new `-FromSource` switch; same UX improvements as bash
- Release mode installs to `~/.cargo/bin/` (Unix) / `%USERPROFILE%\.cargo\bin\`
  (Windows) — same path as source mode, identical PATH instructions

---

## [0.12.11] – 2026-03-02 — Rustup-style interactive installer

### Changed
- **`install.sh`** rewritten with a rustup-style interactive UX:
  - Banner with version, system info table (OS, arch, shell, Rust version, install path)
  - Numbered installation plan displayed before any action is taken
  - Three-option prompt: `1) Proceed` / `2) Customize` / `3) Cancel`
  - Customize flow lets the user toggle each step individually (SSH check, Rust install,
    component selection: tester / endpoint / both) then shows a revised plan before executing
  - Step headers (`Step N: …`) printed during execution so progress is always visible
  - Completion summary with PATH notice and quick-start commands
  - New CLI flags: `-y`/`--yes` (non-interactive), `--skip-ssh-check`, `--skip-rust`,
    `-h`/`--help`; positional `tester|endpoint|both` (default: `both`)
- **`install.ps1`** rewritten with the equivalent rustup-style UX for Windows:
  - Same banner → system info → plan → 1/2/3 prompt → customize → step execution → summary
  - PS 5.1 compatible throughout (no `??` operator, no `?.`); `$script:` prefix for
    cross-function state mutation
  - ARM64 vs x64 rustup-init URL auto-detected via `$env:PROCESSOR_ARCHITECTURE`
  - `$ErrorActionPreference` guarded around SSH call (GitHub always exits 1 by design)
  - Validates `-Component` value even when script is piped through `iex`
  - Equivalent flags: `-Yes`, `-SkipSshCheck`, `-SkipRust`, `-Help`

---

## [0.12.10] – 2026-03-02 — Coverage phase 5: tls/curl/pageload/html unit tests

### Added
- **`runner/tls.rs`** +2 tests: `make_failed` constructor — verifies `Protocol::Tls`, error
  message/detail/category, `retry_count=0`, `finished_at` present, `tls/dns/tcp` all None.
- **`runner/curl.rs`** +2 tests: `make_failed` constructor — verifies `Protocol::Curl`, error
  fields, no-detail variant.
- **`runner/pageload.rs`** +5 tests: `parse_server_timing_simple` — no headers → None;
  version header → Some with version; clock_skew formula (`diff_ms - ttfb/2`, verified to
  within 5ms tolerance); invalid RFC3339 timestamp → `server_timestamp`/`clock_skew` both None;
  server-timing header alone → Some.
- **`output/html.rs`** +10 tests: `append_proto_row` — all-success → `"ok"` CSS class; partial
  failure → `"warn"` CSS class; TTFB/total averages computed correctly; no HTTP result → em
  dash. `append_attempt_row` — HTTP 200 success shows status + `"ok"`; failed attempt shows
  `row-err` class + error message + detail in title; UDP echo shows `rtt_avg_ms` + `loss%`;
  UDP throughput shows `transfer_ms` + `throughput_mbps`; no-result attempt shows multiple em
  dashes + `OK` span; Download attempt shows `throughput_mbps` + formatted payload size.

### Coverage
- Total: 70.28% → **71.78%** lines (+1.5 pp); 74.48% → **75.40%** functions

---

## [0.12.9] – 2026-03-02 — Coverage phase 4: http.rs + udp.rs unit tests + integration flakiness fix

### Added
- **30 new unit tests in `runner/http.rs`**: `is_no_proxy` edge cases (empty string,
  case-insensitive, whitespace trimming, empty entries, non-suffix match); `parse_server_timing_header`
  edge cases (unknown name ignored, invalid `dur=` ignored, `dur=` among multiple attrs);
  `parse_cert_fields` (invalid DER returns None, empty bytes, valid cert subject/issuer/expiry via
  rcgen, expiry is in future); `pick_ip` fallback cases (no IPv4 available → first, `prefer_v4=false`
  → first regardless); `failed_attempt` constructor (all fields verified); `parse_server_timing`
  with HeaderMap (no headers → None, server-timing only, `x-networker-server-version`, request-id);
  `build_request` method/header selection (GET for empty payload, POST with content-length and
  upload-bytes headers, host + request-id headers set correctly).
- **3 new unit tests in `runner/udp.rs`**: `udp_failed` constructor correctness; loopback echo
  server with `probe_rtts_ms` all-Some verification; existing test reuse without duplication.

### Fixed
- **Integration test flakiness** (`free_udp_port()`): UDP server binds `0.0.0.0:{port}` but the
  previous `free_port()` used a TCP listener (different port namespace). New `free_udp_port()` now
  binds `0.0.0.0:0` — the same address family as the server — so the port is guaranteed free for a
  subsequent `0.0.0.0:{port}` bind, eliminating "UDP throughput server did not bind within 10s" panics.

### Coverage
- `runner/http.rs`: 68% → 78.22% lines, 75% → 86.67% functions
- `runner/udp.rs`: 60% → 84.68% lines, 78% → 88.89% functions
- Total: 67.46% → 70.28% lines, 74.92% → 74.48% functions (line improvement of +2.8 pp)

---

## [0.12.8] – 2026-03-02 — Upload size verification via response header

### Added
- **`X-Networker-Upload-Bytes: N`** request header: the client now declares the
  intended upload size on every POST so the server knows what to expect.
- **`X-Networker-Received-Bytes: N`** response header: the endpoint echoes the
  actual number of bytes drained from the request body, enabling end-to-end
  verification without parsing the JSON body.
- `verify_upload()` in `throughput.rs`: after every `upload` / `webupload` probe,
  the received byte count is compared against the declared payload size. A mismatch
  marks the attempt `success = false` with a clear `ErrorCategory::Http` message
  ("sent N bytes but server received M bytes"). Absent header (third-party or older
  endpoint) is silently skipped — no false failures.
- 6 new unit tests covering the verification logic: match passes, mismatch fails with
  descriptive message, absent header is skipped, already-failed attempt is not
  overwritten, header name matching is case-insensitive.
- 1 new endpoint unit test: `upload_returns_received_bytes_header`.

---

## [0.12.7] – 2026-03-02 — Streaming upload body; large-payload timeout scaling

### Fixed
- **Upload probes** (`upload`, `webupload`) no longer allocate the entire upload payload in
  RAM at once. The body is now streamed in 256 KiB chunks from a static zero buffer, so a
  5 GiB upload uses ~256 KiB of memory instead of ~5 GB.
- **Timeout auto-scaling**: both upload probes extend the request timeout if the payload
  cannot complete at an assumed minimum speed of ~100 MB/s within the user-specified timeout.
  A 5 GiB upload now gets ~60 s instead of the default 30 s, preventing spurious timeouts
  on large but healthy uploads. Users on slower links can still set `--timeout` explicitly.
- `content-length` header is now set on streaming upload requests so HTTP/1.1 uses
  fixed-length framing rather than chunked transfer encoding.

### Added
- 9 new unit tests: `upload_body_exact_byte_count`, `upload_body_all_zeros`,
  `upload_body_small_payload`, `upload_body_zero_bytes_yields_nothing` (streaming body
  correctness); `timeout_unchanged_for_small_payloads`, `timeout_unchanged_for_one_gib`,
  `timeout_extended_for_five_gib`, `timeout_never_below_base`,
  `timeout_zero_payload_returns_overhead_only` (timeout scaling logic).

---

## [0.12.6] – 2026-03-02 — Integration tests for pageload H1/H2/H3

### Added
- Integration tests for `run_pageload_probe` (HTTP/1.1), `run_pageload2_probe` (HTTP/2),
  and `run_pageload3_probe` (HTTP/3 over QUIC) — all using the in-process endpoint on random
  ports, no external dependencies required
- `Endpoint::wait_for_quic()` helper in the integration test fixture: waits 300 ms after
  TCP-HTTPS readiness to let the QUIC server bind its UDP port
- `pageload.rs` line coverage: 11% → 74%; function coverage: 28% → 84%
- Overall (lib + integration): lines 47.9% → 57.4%, functions 54% → 65.8%

---

## [0.12.5] – 2026-03-02 — Test coverage phase 2

### Added
- **`runner/curl.rs`** — unit tests for `parse_write_out`, `secs_to_ms`, and
  `error_category_for_exit` (all pure-logic, no live curl needed): 11 new tests
- **`metrics.rs`** — tests for `primary_metric_label` (Dns / Tls / Browser),
  `primary_metric_value` (with and without sub-results), `attempt_payload_bytes`
  (http, udp_throughput, zero-payload filter), `TestRun::protocols_tested`
  (deduplication), and `RequestAttempt::total_duration_ms`: 18 new tests
- **`runner/pageload.rs`** — tests for all 6 named presets (tiny / small / medium /
  large / default / mixed), mixed-asset composition, case-insensitive matching,
  and unknown-preset error messaging: 8 new tests
- **`cli.rs`** — tests for empty-modes validation, verbose flag → log-level debug,
  log-level overriding verbose, `parsed_modes` filtering invalid strings, `--page-preset`
  tiny resolution, invalid preset fallback, `parse_size` gigabyte suffix, and
  `load_config` error paths: 8 new tests
- **`output/html.rs`** — tests for CSS `<link>` injection, error section, Throughput
  Results section (Download attempt), TLS Details section, Page Load section, Browser
  Results section, `escape_html` quotes/ampersand: 8 new tests
- **`output/excel.rs`** — test exercising the Throughput sheet (Download + Upload
  attempts) that was previously uncovered: 1 new test
- **`runner/browser.rs`** — tests for `find_chrome` with non-existent env-var path
  (skip), existing tempfile path (return), and `build_page_url` scheme/host preservation:
  3 new tests

---

## [0.12.4] – 2026-03-02 — README: fix Windows installer examples

### Fixed
- **README.md** — Windows PowerShell section now shows self-contained one-liners for both
  `tester` and `endpoint`: the endpoint example previously assumed the script was already
  downloaded locally (`.\install.ps1 -Component endpoint`); replaced with an
  `Invoke-WebRequest` + `&` pattern that downloads from the Gist URL before running.
  Added PS 5.1 / PS 7+ compatibility note.

---

## [0.12.3] – 2026-03-02 — Fix install.ps1 NativeCommandError on SSH probe

### Fixed
- **`install.ps1`** — `ssh -T git@github.com` always exits with code 1 (GitHub design).
  With `$ErrorActionPreference = "Stop"` set globally, PowerShell 5.1 throws
  `NativeCommandError` before the authentication string can be checked.
  Fixed by temporarily lowering preference to `"Continue"` around the SSH call and
  restoring it immediately after.

---

## [0.12.2] – 2026-03-02 — Fix Gist sync to include install.ps1

### Fixed
- **`sync-gist.yml`** — workflow only watched `install.sh` changes and only uploaded `install.sh`
  to the Gist; `install.ps1` was never synced even when it changed. Updated path trigger and
  payload to include both `install.sh` and `install.ps1`.

---

## [0.12.1] – 2026-03-01 — Fix install.ps1 compatibility with Windows PowerShell 5.1

### Fixed
- **`install.ps1`** — replaced `?.Source` (null-conditional member access, requires PS 7.1+)
  with a PS 5.1-compatible `if ($cmd) { $cmd.Source }` pattern. The script declared
  `#Requires -Version 5.1` but used syntax only available in PowerShell 7.1+, causing a
  `ParseException: UnexpectedToken` when run via `irm … | iex` on Windows with the default
  Windows PowerShell 5.1.

---

## [0.12.0] – 2026-03-01 — Real-browser probe (`browser` mode via chromiumoxide)

### Added
- **`browser` probe mode** (`--features browser`) — drives a real headless Chromium instance
  via the Chrome DevTools Protocol (chromiumoxide 0.7) to measure actual page-load performance
  that no synthetic probe can replicate.
- **Metrics captured**: load time (navigation start → load event), DOMContentLoaded (ms),
  TTFB (ms), total resource count, total transferred bytes, negotiated protocol for the main
  document, and per-protocol resource counts (e.g. `h2×18 h3×2`).
- URL is rewritten to `/page` endpoint for a fair comparison with `pageload`/`pageload2`/
  `pageload3` probes.
- Self-skips with a `success: false` `RequestAttempt` if no Chrome/Chromium binary is found;
  binary search order: `NETWORKER_CHROME_PATH` env var → common Linux paths → macOS app bundles.
- `--features browser` is opt-in (not part of `default`); the stub build always compiles and
  returns a clear error message.
- HTML report **"Browser Results"** section with a per-attempt table (Protocol, TTFB, DCL,
  Load, Resources, Bytes, per-protocol counts).
- Terminal summary: `Protocol Comparison` table now includes a `browser` row.
- New `BrowserResult` struct in `metrics.rs`; `Protocol::Browser` variant in the `Protocol`
  enum; `RequestAttempt.browser: Option<BrowserResult>` field (backwards-compatible via
  `#[serde(default, skip_serializing_if = "Option::is_none")]`).

---

## [0.11.7] – 2026-03-01 — Fix ServerTimingResult schema (type mismatch in FK)

### Fixed
- **`sql/06_ServerTiming.sql`** — `ServerId` and `AttemptId` columns were declared
  as `UNIQUEIDENTIFIER`, but `RequestAttempt.AttemptId` (the FK target) is
  `NVARCHAR(36)`. SQL Server rejected the `CREATE TABLE` silently (sqlcmd
  doesn't abort on DDL errors), leaving `dbo.ServerTimingResult` absent from the
  database. Both columns are now `NVARCHAR(36)` to match the schema convention
  used by all other tables and the Rust insert code (`uuid.to_string()`).
  The `DEFAULT NEWSEQUENTIALID()` on `ServerId` is also removed — the insert
  always provides an explicit UUID value.

---

## [0.11.6] – 2026-03-01 — Improve sql.rs coverage

### Added
- **`sql_full_round_trip` test** — exercises all 7 sub-result insert helpers
  (`insert_dns_result`, `insert_tcp_result`, `insert_tls_result`,
  `insert_http_result`, `insert_udp_result`, `insert_error`,
  `insert_server_timing_result`) by inserting a fully-populated `RequestAttempt`
  with every sub-result field set; expected to push `sql.rs` line coverage from
  ~39% to ~85%+.

### Changed
- `sql_insert_round_trip` refactored to use shared `make_run` / `bare_attempt` /
  `sql_conn` helpers so the new comprehensive test reuses the same scaffolding.

---

## [0.11.5] – 2026-03-01 — Fix 6 test failures with --include-ignored in coverage CI

### Fixed
- **`validate_save_to_sql_without_conn_string_fails`** — test now self-skips when
  `NETWORKER_SQL_CONN` is set in the environment; clap picks up the env var automatically,
  so validation correctly passes in that environment (breaking the assertion).
- **`sql_insert_round_trip`** — coverage CI schema creation now runs all 7 SQL migration
  files (01–02 base + 04–07 migrations); previously missing migrations left `RequestAttempt`
  without the `RetryCount` column, causing the INSERT to panic.
- **`http1_probe_succeeds` / `http2_probe_negotiates_h2`** — tests now probe
  `127.0.0.1:8080` / `127.0.0.1:8443` with a TCP connect before running; they self-skip
  with an `eprintln!` message when no endpoint is listening, avoiding false failures in CI.
- **`download_probe_returns_throughput` / `upload_probe_returns_throughput`** — same
  self-skip pattern: TCP connect to `127.0.0.1:8080` before running the probe.
- **`sql-integration` CI job** — also updated to run all 7 SQL migration files (same fix
  as coverage job).

---

## [0.11.4] – 2026-03-01 — Coverage phase 1: SQL Docker, DNS tests, Excel tests

### Added
- **SQL Server in coverage CI** — MSSQL 2022 Docker service added to the coverage
  job; schema migrations run automatically; `NETWORKER_SQL_CONN` env var set so the
  existing `#[ignore]` SQL round-trip test executes under `cargo-llvm-cov`.
- **Excel unit tests** — `save_writes_xlsx_file` exercises all 10 worksheet writers
  with a fully-populated `TestRun` (HTTP, TCP, TLS, UDP, throughput, UDP throughput,
  server timing, page-load, errors); `save_empty_run_does_not_panic` covers the
  zero-attempt edge case.
- **`main.rs` excluded from coverage** — `--ignore-filename-regex 'main\.rs'` added
  to all `cargo llvm-cov report` invocations so the binary entry point does not
  drag down the overall percentage.

### Changed
- DNS tests `resolves_localhost` and `ipv4_only_filter` are no longer `#[ignore]` —
  loopback resolution works in all environments including CI.
- Coverage lib-test step now passes `--include-ignored` to pick up `#[ignore]` tests
  that self-skip when their service is unavailable (SQL, curl).

---

## [0.11.3] – 2026-03-01 — Coverage report as GitHub Actions artifact

### Changed
- CI coverage job now uploads an HTML report + `lcov.info` as a downloadable
  GitHub Actions artifact (`coverage-report`, 30-day retention) instead of
  pushing to Codecov. The coverage summary is also printed directly in the
  CI log (`cargo llvm-cov report --summary-only`).

---

## [0.11.2] – 2026-03-01 — Fix --all-features compile error in native probe

### Fixed
- `runner/native.rs`: second `HttpResult` literal was missing `goodput_mbps`,
  `cpu_time_ms`, `csw_voluntary`, and `csw_involuntary` — fields added in v0.11.0.
  Only exposed by `--all-features` builds (e.g. `cargo-llvm-cov`); the default CI
  build does not enable `native` and did not catch this. (#58)

---

## [0.11.1] – 2026-03-01 — HTTP/3 QUIC endpoint; --insecure for http3 probe

### Added
- **HTTP/3 QUIC server in `networker-endpoint`** — Quinn-based QUIC listener on UDP
  8443 (same port as HTTPS), serving `/health`, `/download`, `/upload`, `/page`, `/asset`
  with full `Server-Timing` (proc/recv/csw-v/csw-i) and `X-Networker-*` headers.
  `http3` is now a default feature of both crates; no extra flags needed.
- **`--insecure` and `--ca-bundle` for `http3` probe** — previously the h3 client always
  used webpki roots and ignored these flags. Now uses the same `build_tls_config()` path
  as HTTP/1.1 and HTTP/2, so `--insecure` works with the self-signed endpoint cert.

### Changed
- `networker-endpoint/Cargo.toml`: `[features] default = ["http3"]`
- `/info` endpoint: `"http3": true` and `"protocols": ["HTTP/1.1","HTTP/2","HTTP/3"]`
  when compiled with the http3 feature (now the default).

---

## [0.11.0] – 2026-02-28 — CPU cost, goodput, context switches & TTFB visibility

### Added
- **CPU time on all HTTP probes** (`http1`, `http2`, `http3`) — `HttpResult.cpu_time_ms`
  captures process CPU (user + system) consumed per probe using `cpu-time::ProcessTime`.
  Enables a fair H1 vs H2 vs H3 comparison; QUIC/HTTP3 is expected to show the highest
  CPU cost due to in-process TLS encryption.
- **Goodput metric** — `HttpResult.goodput_mbps` = payload_bytes / full end-to-end delivery
  time (DNS + TCP + TLS + total HTTP ms). Penalizes connection-setup overhead, giving a
  more complete picture than throughput alone (which only measures the body-transfer phase).
  Set for all four throughput probe types: `download`, `upload`, `webdownload`, `webupload`.
- **Client-side context switches** — `HttpResult.csw_voluntary` and `csw_involuntary`
  capture the `getrusage(RUSAGE_SELF)` delta (`ru_nvcsw`, `ru_nivcsw`) over the full probe
  duration (Unix only; `None` on Windows).
- **Server-side context switches** — `networker-endpoint` now appends
  `csw-v;dur=N, csw-i;dur=N` to the existing `Server-Timing` header on `/download` and
  `/upload` responses. `ServerTimingResult.srv_csw_voluntary` /
  `srv_csw_involuntary` expose these values in the tester's metrics.
- **TTFB + TLS visibility in throughput terminal output** — `log_attempt()` for
  `download`, `upload`, `webdownload`, `webupload` probes now shows:
  `TLS:Xms` (when applicable), `TTFB:Xms`, `Goodput:X MB/s`, `CPU:Xms`,
  `CSW:Xv/Xi` (client), `sCSW:Xv/Xi` (server).
- **New HTML Throughput Results columns**: Goodput (MB/s), CPU (ms),
  Client CSW (v/i), Server CSW (v/i) alongside the existing TTFB and Total columns.

### Internal
- `parse_server_timing_header()` refactored to return a named `ParsedServerTiming` struct
  (replacing the previous 3-tuple) to accommodate the two new `csw-v`/`csw-i` fields.

---

## [0.10.0] – 2026-02-28 — H1.1 keep-alive fix, TLS cost visibility, named presets, CPU measurement

### Added
- **`pageload` H1.1 keep-alive pool** — corrected a fundamental accuracy bug where each
  asset opened a brand-new TCP+TLS connection. The rewritten probe opens `k = min(6, n)`
  persistent TCP connections (one TLS handshake each for HTTPS) and distributes assets
  across them round-robin, so each connection reuses its TCP/TLS handshake for all its
  assigned assets — exactly how a real browser behaves. This eliminates the previous
  inflation of TLS setup cost and makes the H1.1 vs H2 vs H3 comparison accurate.
- **TLS cost fields on `PageLoadResult`** — four new fields report the cost of TLS
  establishment per page-load variant:
  - `tls_setup_ms`: sum of all TLS handshake durations (H1.1: k handshakes; H2/H3: 1).
  - `tls_overhead_ratio`: fraction of `total_ms` spent in TLS (0.0–1.0).
  - `per_connection_tls_ms`: per-connection handshake durations (length = `connections_opened`).
  - `cpu_time_ms`: process CPU time consumed during the probe (highest for HTTP/3 due to
    QUIC userspace encryption).
- **Named `--page-preset` flag** — selects a predefined asset mix, overriding
  `--page-assets` and `--page-asset-size`:

  | Preset    | Assets | Size per asset | Total    |
  |-----------|--------|---------------|----------|
  | `tiny`    | 100    | 1 KB          | ~100 KB  |
  | `small`   | 50     | 5 KB          | ~250 KB  |
  | `default` | 20     | 10 KB         | ~200 KB  |
  | `medium`  | 10     | 100 KB        | ~1 MB    |
  | `large`   | 5      | 1 MB          | ~5 MB    |
  | `mixed`   | 30     | varied        | ~820 KB  |

  The `mixed` preset (1×200KB + 4×50KB + 10×20KB + 15×5KB) approximates a real-world
  web page with a large hero image, medium assets, and many small scripts/styles.
- **Per-asset sizes in `PageLoadConfig`** — `asset_sizes: Vec<usize>` replaces the old
  uniform `asset_count`/`asset_size` pair. Each element specifies the byte count for
  one asset, enabling varied payloads (used by presets and future per-asset control).
- **Extended Protocol Comparison table** — both the terminal output and the HTML report
  now include `TLS Setup (ms)`, `TLS Overhead %`, and `CPU (ms)` columns, making the
  cost structure of each protocol variant immediately visible.

### Changed
- `PageLoadConfig.asset_count` / `asset_size` → `asset_sizes: Vec<usize>` and
  `preset_name: Option<String>`. Consumers must pass `asset_sizes` (a `Vec`).
- `ResolvedConfig.page_assets` / `page_asset_size` → `page_asset_sizes: Vec<usize>` and
  `page_preset_name: Option<String>`.
- Workspace version bumped to `0.10.0` (MINOR — new fields, new flag, keep-alive fix).

---

## [0.9.0] – 2026-02-28 — HTTP/3 page-load probe

### Added
- **`pageload3` probe mode** — fetches the same N assets as `pageload`/`pageload2` but
  multiplexed over a single QUIC/HTTP/3 connection (`connections_opened = 1`).
  All N asset streams are opened sequentially (fast HEADERS frames) then all responses
  are received concurrently. Requires `--features http3` and an HTTPS target.
  Completes the three-protocol page-load comparison: HTTP/1.1 (≤6 conns) vs
  HTTP/2 (1 TLS conn) vs HTTP/3 (1 QUIC conn), motivated by
  "Does QUIC Make the Web Faster?" (Biswal & Gnawali, IEEE GLOBECOM 2016).
- **`--insecure` support for `pageload3`** — reuses `build_tls_config` from `http.rs`
  (same `NoCertVerifier` + custom CA bundle path), overriding ALPN to `h3`.
- **ALPN warning extended** — startup `[WARN]` now also fires for `pageload3` mode
  against a plain `http://` target.
- **Protocol Comparison table extended** — terminal and HTML report now include a
  `pageload3` row alongside `pageload` and `pageload2`.

### Background
Reference [5] cited in "Does QUIC Make the Web Faster?" for the finding that
bandwidth improvements beyond ~5 Mbps yield diminishing returns on page load time is:
Ilya Grigorik, *"Latency: The New Web Performance Bottleneck"*,
https://www.igvita.com/2012/07/19/latency-the-new-web-performance-bottleneck/, 2012.
This motivates testing all three protocols: the wall-clock difference between `pageload`,
`pageload2`, and `pageload3` reveals which bottleneck (connection setup vs multiplexing
vs QUIC handshake latency) dominates under real network conditions.

---

## [0.8.0] – 2026-02-28 — Page-load simulation, ALPN warning

### Added
- **`pageload` probe mode** — fetches `/page?assets=N&bytes=B` manifest from the endpoint
  then downloads all assets over up to 6 parallel HTTP/1.1 connections (browser-like).
  Measures wall-clock `total_ms`, `ttfb_ms`, `connections_opened`, per-asset timings,
  and total bytes. Configure with `--page-assets N` (default 20) and
  `--page-asset-size <size>` (default 10k, accepts k/m suffixes).
- **`pageload2` probe mode** — same N assets multiplexed over a single HTTP/2 TLS
  connection. Records `connections_opened = 1`. Requires an HTTPS target.
- **`/page` and `/asset` endpoints on `networker-endpoint`** — `GET /page?assets=N&bytes=B`
  returns a JSON manifest listing N asset URLs; `GET /asset?id=X&bytes=B` returns B
  zero bytes (cap 100 MiB).
- **ALPN warning** — startup warns with `[WARN]` when `http2`, `http3`, or `pageload2`
  mode is requested against a plain `http://` target (HTTP/2 requires TLS+ALPN; over
  plain HTTP every connection silently falls back to HTTP/1.1).
- **`PageLoadResult` struct** — `asset_count`, `assets_fetched`, `total_bytes`,
  `total_ms`, `ttfb_ms`, `connections_opened`, `asset_timings_ms`, `started_at`.
  Attached to `RequestAttempt.page_load` (serde-default, skip_serializing_if none).
- **Terminal comparison table** — when both `pageload` and `pageload2` are run in the
  same session, a `Protocol Comparison (Page Load)` table is printed showing N,
  assets, avg connections, p50/min/max total_ms per variant.
- **HTML Protocol Comparison card** — same data rendered as an HTML `<table>` after
  the Statistics Summary section whenever any `pageload`/`pageload2` attempts are present.
- `pageload` and `pageload2` appear in terminal averages + statistics tables, HTML
  Timing Breakdown, and HTML Statistics Summary.

### Changed
- CLI `--modes` help text extended to document `pageload` and `pageload2`.
- `runner/http.rs::build_tls_config` promoted to `pub(crate)` for reuse by `pageload.rs`.
- `cli::parse_size` promoted to `pub(crate)` for reuse in `resolve()`.
- Workspace version bumped to `0.8.0` (MINOR — new features).

---

## [0.7.0] – 2026-02-28 — native-TLS probe, curl probe, tls_backend field

### Added
- **`native` probe mode** — DNS + TCP + platform TLS + HTTP/1.1 using the OS TLS
  stack: SChannel (Windows), SecureTransport (macOS), OpenSSL (Linux). Requires
  recompiling with `--features native` (gates the `native-tls` / `tokio-native-tls`
  deps to avoid mandatory OpenSSL headers on Linux CI). Records leaf certificate
  info via `x509-parser`. TLS version and cipher suite are not exposed by
  `native-tls` and are reported as `"unknown"`.
- **`curl` probe mode** — spawns the system `curl` binary with `--write-out` timing
  fields and maps the output to the same `DnsResult` / `TcpResult` / `TlsResult` /
  `HttpResult` structs as an `http1` probe. Requires `curl` on `$PATH`; returns a
  graceful error at runtime if not found. Supports `--insecure`, `--proxy`,
  `--ca-bundle`, `--ipv4-only`, `--ipv6-only`, and `--timeout`.
- **`TlsResult.tls_backend: Option<String>`** — new serde-default field that records
  which TLS implementation performed the handshake: `"rustls"` for all existing
  rustls-based probes (`http1`, `http2`, `http3`, `tls`), `"native/schannel"` /
  `"native/secure-transport"` / `"native/openssl"` for the `native` probe, and
  `"curl"` for the `curl` probe.
- `native` and `curl` appear in the terminal summary tables, HTML Statistics
  Summary, and HTML Timing Breakdown.

### Changed
- CLI `--modes` help text extended to document `native` and `curl`.
- Workspace version bumped to `0.7.0` (MINOR — new features).

### Fixed
- `runner/tls.rs`: default port for non-HTTPS targets was incorrectly `443`; now `80`.

---

## [0.6.0] – 2026-02-28 — DNS probe, TLS probe, proxy support, CA bundle

### Added
- **`dns` probe mode** — standalone DNS resolution probe (`--modes dns`); records
  resolved IPs, query duration, and success state. No TCP or HTTP activity.
- **`tls` probe mode** — standalone TLS handshake probe (`--modes tls`); performs
  DNS + TCP connect + TLS handshake and records the full certificate chain (all
  certs with Subject, Issuer, SANs, and expiry), negotiated cipher suite, TLS
  version, and ALPN protocol. Advertises both `h2` and `http/1.1` in ALPN to
  discover server preference without sending an HTTP request.
- **`--proxy <url>`** — explicit HTTP proxy URL (e.g. `http://proxy.corp:3128`);
  overrides `HTTP_PROXY`/`HTTPS_PROXY` env vars. For HTTPS targets, a CONNECT
  tunnel is established through the proxy before TLS; for HTTP targets an
  absolute-form URI is used.
- **`--no-proxy`** — disable all proxy detection (both `--proxy` flag and
  `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` env vars). Respects `NO_PROXY` /
  `no_proxy` env var when reading proxy settings from the environment.
- **`--ca-bundle <path>`** — path to a PEM-format CA certificate bundle to add
  to the trust store; useful for corporate CAs not present in the OS store.
  Supported by both HTTP/HTTPS probes and the standalone TLS probe.
- **`CertEntry`** struct in `metrics.rs` — captures `subject`, `issuer`, `expiry`,
  and `sans` (Subject Alternative Names) for each certificate in the chain.
- **`cert_chain: Vec<CertEntry>`** field on `TlsResult` — populated by the
  standalone TLS probe.
- **`proxy` / `ca_bundle`** fields in `ConfigFile` / `ResolvedConfig` / `tester.example.json`.
- Terminal progress logging for `dns` and `tls` protocols.
- HTML and terminal summary tables now include `dns` and `tls` rows.

### Changed
- `RunConfig` gains `ca_bundle: Option<String>`, `proxy: Option<String>`, and
  `no_proxy: bool` fields (all defaulting to `None`/`false`).
- `build_tls_config()` in `runner/http.rs` now returns `anyhow::Result` and
  accepts an optional CA bundle path.
- Workspace version bumped to `0.6.0` (MINOR — new features).

---

## [0.5.0] – 2026-02-28 — Payload-grouped stats + collapsible HTML sections

### Added
- **Payload-grouped statistics** — the terminal Statistics Summary and Averages tables now group
  results by `(protocol, payload_size)` rather than by protocol alone. Running
  `--modes download,upload --payload-sizes 64k,1m,4m` produces separate rows for
  "download 64KiB", "download 1MiB", etc., each with their own N/Min/Mean/p50/p95/p99/Max/StdDev.
- **`attempt_payload_bytes()`** — new public helper in `metrics.rs` that returns the payload
  size for throughput attempts (`http.payload_bytes` or `udp_throughput.payload_bytes`),
  `None` for latency-only probes.
- **`fmt_bytes()` helper in `main.rs`** — formats byte counts as KiB/MiB/GiB for terminal output.
- **Collapsible `<details>` sections in HTML report** (no JS — pure HTML5):
  - **Throughput Results** — one `<details>` per `(proto, payload)` group; summary line shows
    `N runs · avg X MB/s · ±stddev · min Y · max Z`. Expanded by default only when there is
    exactly one group with ≤ 20 rows.
  - **UDP Throughput Results** — same treatment; summary line includes average loss %.
  - **All Attempts** — single collapsible block; summary shows succeeded/failed counts;
    open by default when total attempts ≤ 20.
  - **TCP Stats** — single collapsible block showing connection count; open by default when ≤ 20 rows.
- **Inline CSS** and **`assets/report.css`** updated with `<details>`/`<summary>` styles
  (`▶`/`▼` indicator, `.grp-lbl`, `.grp-meta` classes).

### Changed
- HTML Statistics Summary now emits one row per `(protocol, payload_size)` group, matching
  the terminal output. The "Protocol" column value becomes e.g. "download 64 KiB".
- Terminal averages table header widened from 9 → 16 chars to accommodate grouped labels.
- Workspace version bumped to `0.5.0` (MINOR — new feature).

---

## [0.4.0] – 2026-02-28 — JSON config file support

### Added
- **`--config` / `-c` flag (both binaries)** — accepts a path to a JSON config file. Any key
  from the file can be overridden by a CLI flag (priority: CLI arg > JSON key > built-in default).
- **`--log-level` flag (both binaries)** — set the `tracing` filter directly (e.g.
  `"debug"`, `"info,tower_http=debug"`). Overrides `--verbose` (tester only) and `RUST_LOG`.
- **`ConfigFile` / `ResolvedConfig` structs in `cli.rs`** — all previously hard-defaulted
  tester fields are now `Option<T>` in the raw `Cli` struct; `Cli::resolve(Option<ConfigFile>)`
  merges CLI + file + built-in defaults into a concrete `ResolvedConfig`.
- **`validate()`, `parsed_modes()`, `parsed_payload_sizes()`** moved to `ResolvedConfig`;
  `validate()` gains an explicit `ipv4_only && ipv6_only` conflict check (catches config-file
  sourced conflicts not covered by clap's `conflicts_with`).
- **`tester.example.json`** — repo-root example file showing every tester key with its default
  value.
- **`endpoint.example.json`** — repo-root example file showing every endpoint key with its
  default value.
- New unit tests: `resolved_defaults`, `config_file_overrides_defaults`,
  `cli_overrides_config_file`.

### Changed
- `Cli` struct field types changed from concrete types with `default_value` annotations to
  `Option<T>` (no observable behavior change — defaults still apply via `resolve()`).
- Existing tests `defaults_parse`, `validate_save_to_sql_without_conn_string_fails`, and
  `payload_sizes_parsed_via_cli` updated to reflect the new raw/resolved split.
- Workspace version bumped to `0.4.0` (MINOR — new feature).

---

## [0.3.3] – 2026-02-28 — Fix RUST_LOG documentation

### Fixed
- **README `RUST_LOG` example** — `RUST_LOG=tower_http=debug` was documented as the way
  to get verbose HTTP logs, but a target-specific directive alone silently suppresses all
  other log targets (including the endpoint's own startup lines). Corrected to
  `RUST_LOG=info,tower_http=debug` with an explanatory note.

---

## [0.3.2] – 2026-02-28 — Endpoint version banner + request logging

### Added
- **Version banner at startup** — `networker-endpoint` now prints its version (e.g.
  `networker-endpoint v0.3.2`) as the first log line before the listening-address lines.
- **HTTP request/response logging** — `TraceLayer` (from `tower-http`) added to the axum
  router; every request is logged at `INFO` with method + URI, and every response with
  status code + latency. Verbosity is controlled by `RUST_LOG`
  (e.g. `RUST_LOG=info,tower_http=debug` for verbose HTTP spans).

---

## [0.3.1] – 2026-02-28 — webdownload/webupload path rewrite

### Fixed
- **`webdownload` and `webupload` path rewrite** — both probes previously left the URL path
  unchanged (e.g. `/health`), so `webdownload` returned whatever the target endpoint happened
  to respond with (e.g. 114 B health JSON) and `webupload` POSTed to a path that ignored the
  request body. Both probes now rewrite the URL path identically to their non-web counterparts:
  `webdownload` → `GET /download?bytes=N`, `webupload` → `POST /upload`. The `--target` flag
  may point at any path; the host and port are preserved and the path is replaced.
- **`--payload-sizes` now required for `webdownload`** — updated CLI help text to document that
  `webdownload` requires `--payload-sizes` (same as `download`), since it now issues a
  `?bytes=N` request and must have a size to request.

---

## [0.3.0] – 2026-02-28 — Web probes, UDP throughput, statistics

> Starting from this release every PR includes a version bump.
> Standard [Semantic Versioning](https://semver.org/) (`MAJOR.MINOR.PATCH`) is used:
> new features → MINOR bump, bug fixes → PATCH bump.

### Fixed
- **`webdownload` ignored `--payload-sizes`** — the mode previously ran once per cycle
  and GETed the target URL as-is, returning whatever the server happened to send (e.g. 114 B
  for a `/health` endpoint). `webdownload` now expands per payload size exactly like `download`,
  and appends `?bytes=N` to the target URL so that any server that supports the parameter (such
  as `networker-endpoint`'s `/download` route) will stream back the requested number of bytes.
  The actual body bytes received are always used for the throughput calculation.
  `--payload-sizes` is now required for `webdownload` (same as `download`).
- **`webupload` absurd throughput when server ignores the request body** — generic targets
  (e.g. a `/health` endpoint) may respond immediately without draining the POST body, making
  `ttfb_ms` near-zero and the computed throughput physically impossible (e.g. 1.3M MB/s).
  `webupload` now uses a dedicated `patch_webupload_throughput` helper that (a) falls back to
  `total_duration_ms` instead of `ttfb_ms` when no `Server-Timing: recv` header is present,
  and (b) caps results at 100,000 MB/s (≈ 800 Gbps — physically impossible on any real link);
  values above the cap are reported as `null`/`—` instead. Four new unit tests cover the
  server-recv, fallback, implausible, and plausible cases.
- **`webdownload`/`webupload` probes always failed** — `run_probe` in the HTTP runner only
  listed `Http1 | Http2 | Tcp | Download | Upload`; both web-probe variants fell through to the
  `other =>` error arm, returning "Protocol not handled by http runner" on every attempt.
  Added `WebDownload | WebUpload` to both match arms (`run_probe` entry point and the
  `send_http1` dispatch inside `run_http_or_tcp`).
- Clippy `redundant_closure` in `html.rs` (`.map(|b| format_bytes(b))`) and `main.rs`
  (`.filter_map(|a| primary_metric_value(a))`); both replaced with the bare function reference.
- Integration test `ServerConfig` initializer missing `udp_throughput_port` field (added in
  the `udpdownload`/`udpupload` PR but not reflected in the test harness).

### Added
- **`udpdownload` probe mode** — bulk UDP download from `networker-endpoint`'s UDP throughput
  server (default port 9998); measures datagrams sent/received, packet loss %, transfer window
  ms, and throughput MB/s. Requires `--payload-sizes`.
- **`udpupload` probe mode** — bulk UDP upload to `networker-endpoint`'s UDP throughput server;
  server reports bytes actually received (CMD_REPORT) so client-side and server-side counts are
  compared. Requires `--payload-sizes`.
- **UDP throughput protocol** — new custom datagram protocol (`b"NWKT"` magic) over a separate
  port. Control packets: CMD_DOWNLOAD, CMD_UPLOAD, CMD_DONE, CMD_ACK, CMD_REPORT. Data packets
  have 8-byte header (seq_num + total_seqs) + up to 1400-byte payload.
- **`UdpThroughputResult`** — new JSON field on `RequestAttempt`; stores remote_addr,
  payload_bytes, datagrams_sent, datagrams_received, bytes_acked, loss_percent, transfer_ms,
  throughput_mbps.
- **HTML UDP Throughput section** — new card in the report showing all UDP throughput attempts
  with loss %, throughput, and bytes-acked.
- **Excel UDP Throughput sheet** — new sheet in the `.xlsx` report.
- **`networker-endpoint --udp-throughput-port`** — new CLI flag (default 9998) for the bulk
  throughput listener.
- **`networker-tester --udp-throughput-port`** — new CLI flag (default 9998) matching the
  endpoint default.
- **`webdownload` probe mode** — GET the target URL as-is (no endpoint path rewriting),
  measures full HTTP phase timing (DNS, TCP, TLS, TTFB, Total) + response body throughput
  + TCP kernel stats. Works with any HTTP server, not just `networker-endpoint`.
- **`webupload` probe mode** — POST to the target URL with a payload body (requires
  `--payload-sizes`), measures full HTTP phase timing + upload throughput + TCP kernel
  stats. Works with any HTTP server.
- Both new modes appear in the HTML Throughput table, TCP Stats card, All Attempts table,
  and Excel Throughput sheet alongside the existing `download`/`upload` modes.
- **TCP Stats card in HTML report** — new section showing all per-connection kernel
  stats: local→remote addresses, MSS, RTT, RTT variance, min RTT, cwnd, ssthresh,
  retransmits, total retransmits, receive window, segments out/in, delivery rate (MB/s),
  and congestion algorithm.
- **Congestion algorithm** — `TCP_CONGESTION` getsockopt added to Linux and macOS;
  stored as `TcpResult.congestion_algorithm` (e.g. "cubic", "bbr").
- **Delivery rate** — `tcpi_delivery_rate` (Linux ≥ 4.9); bytes/sec stored as
  `TcpResult.delivery_rate_bps`; displayed as MB/s in HTML + Excel.
- **Minimum RTT** — `tcpi_min_rtt` (Linux ≥ 4.9); ms stored as `TcpResult.min_rtt_ms`.
- **segs_out / segs_in** — now populated on Linux ≥ 4.2 (were always `None` previously);
  switched from `libc::tcp_info` struct to raw byte-offset reads so all kernel-version-
  gated fields work without a matching libc struct definition.
- `sql/07_MoreTcpStats.sql` — idempotent `ALTER TABLE` adding `CongestionAlgorithm`,
  `DeliveryRateBps`, `MinRttMs` columns to `dbo.TcpResult`.
- Excel TCP Stats sheet gains **Min RTT ms**, **Delivery MB/s**, **Congestion** columns.
- **Statistics Summary** — per-protocol descriptive statistics (N, Min, Mean, p50, p95, p99,
  Max, StdDev) computed from each run's primary metric (total duration ms for HTTP/TCP, RTT avg
  ms for UDP echo, throughput MB/s for all bulk-transfer modes). Shown in three places: (#35)
  - Terminal: second table printed below the existing averages table.
  - HTML report: new "Statistics Summary" card (between Timing Breakdown and UDP Probe
    Statistics); success % column color-coded green/amber/red.
  - Excel: new "Statistics" sheet (sheet 2, directly after Summary).
- `metrics.rs`: new public `Stats` struct, `compute_stats()`, `primary_metric_label()`, and
  `primary_metric_value()` functions; 3 new unit tests for the percentile calculations. (#35)

---

## [0.2.5] – 2026-02-28 — Install script fixes

### Fixed
- `install.sh`: revert curl URL from `raw.githubusercontent.com` back to public Gist —
  `raw.githubusercontent.com` returns 404 for private repos without authentication. (#29)

### Added
- `.github/workflows/sync-gist.yml`: auto-patches the Gist via GitHub API whenever
  `install.sh` changes on `main`. Requires `GIST_TOKEN` secret (PAT with `gist` scope). (#29)

---

## [0.2.4] – 2026-02-28 — Versioning display + install hardening

### Added
- `networker-endpoint` emits `X-Networker-Server-Version` on every response via the
  `add_server_timestamp` middleware. (#26)
- `ServerTimingResult` gains `server_version: Option<String>` — captured in JSON per attempt. (#26)
- Terminal summary prints both **Client version** and **Server version** rows. (#26)
- HTML report Run Summary card shows Client and Server version rows. (#26)
- Version logged at tester startup. (#26)
- `CHANGELOG.md` added following Keep a Changelog format. (#27)

### Changed
- Workspace version bumped to `0.2.4` in `Cargo.toml` — cascades to both binaries. (#26)

### Fixed
- Upload throughput showed absurdly high values (millions of MB/s) because `total_duration_ms`
  includes receiving the server's JSON response body — noise unrelated to the upload transfer.
  `ttfb_ms` is now the primary denominator: it starts just before `send_request()` and stops
  when the server sends response headers. Because `networker-endpoint` only responds after
  draining the full body, `ttfb_ms ≈ upload wire time`. Formula: `max(server_recv_ms, ttfb_ms)`. (#25)
- `install.sh`: added `--force` to `cargo install` so every run unconditionally rebuilds the
  binary, preventing a stale binary when cargo's git-SHA cache considers the installed rev
  current. (#28)
- `install.sh`: prints the installed version at the end (e.g. `networker-tester 0.2.4`)
  for immediate confirmation. (#28)

---

## [0.2.3] – 2026-02-27 — Upload throughput: max() guard

### Fixed
- Upload throughput: changed denominator from `server_recv_ms.unwrap_or(total_duration_ms)`
  to `max(server_recv_ms, total_duration_ms)` so the larger (correct) value is always used.
  Prevents near-zero `server_recv_ms` (kernel-buffer race on same-machine connections) from
  producing absurdly high throughput values. (#24)

---

## [0.2.2] – 2026-02-27 — Throughput unit tests

### Added
- Full unit test coverage for all throughput calculation paths in `runner/throughput.rs`. (#23)

---

## [0.2.1] – 2026-02-27 — Upload throughput: server-side timing

### Fixed
- Upload throughput: switched denominator to `server_recv_ms` from `Server-Timing: recv;dur=X`
  header — the time the server spent draining the request body, accurate regardless of network
  path. (#22)

---

## [0.2.0] – 2026-02-27 — Extended metrics: TCP kernel stats, retries, server timing, Excel

### Added
- **TCP kernel stats** — 8 new fields on `TcpResult`: `retransmits`, `total_retrans`, `snd_cwnd`,
  `snd_ssthresh`, `rtt_variance_ms`, `rcv_space`, `segs_out`, `segs_in`.
  - Linux: read via `TCP_INFO` socket option (no root required).
  - macOS: read via `TCP_CONNECTION_INFO` (`tcp_connection_info` struct at `IPPROTO_TCP` opt `0x24`).
- **Application-level retries** — `--retries N` CLI flag; failed probes are retried up to N times;
  `retry_count` field added to `RequestAttempt`.
- **Server timing** — `ServerTimingResult` struct captures `Server-Timing` header fields (`recv`,
  `proc`, `total`), `X-Networker-Server-Timestamp`, clock skew estimate, and echoed
  `X-Networker-Request-Id`.
- **Excel output** — `--excel` CLI flag; generates an `.xlsx` report alongside JSON + HTML using
  `rust_xlsxwriter`. 8 sheets: Summary, HTTP Timings, TCP Stats, TLS Details, UDP Stats,
  Throughput, Server Timing, Errors.
- **Privilege notice** — on Linux without root, a startup message explains which metrics are still
  captured vs. what would require elevated privileges.
- **`networker-endpoint` server timing** — `/download` returns `Server-Timing: proc;dur=X`;
  `/upload` returns `Server-Timing: recv;dur=X` and echoes `X-Networker-Request-Id`.
- **SQL migrations** — `sql/05_ExtendedTcpStats.sql` (8 new `TcpResult` columns + `RetryCount`
  on `RequestAttempt`) and `sql/06_ServerTiming.sql` (`ServerTimingResult` table). (#21)

---

## [0.1.0] – 2026-02-27 — Initial release

### Added
- **Workspace** — Cargo workspace with two crates: `networker-tester` (CLI) and
  `networker-endpoint` (server).
- **Probe modes** — `http1`, `http2`, `tcp`, `udp`, `download`, `upload`;
  HTTP/3 gated behind `--features http3`.
- **Per-phase timing** — DNS, TCP connect, TLS handshake, TTFB, total; measured using raw
  `hyper 1.x` connection APIs.
- **TLS** — `rustls 0.23` with `ring` provider; self-signed cert via `rcgen`; `--insecure` flag.
- **UDP echo** — configurable probe count, RTT percentiles, jitter, loss%.
- **Download/upload throughput probes** — `GET /download?bytes=N` and `POST /upload`.
- **Output formats** — JSON, HTML report (embedded CSS, protocol comparison tables),
  SQL Server via `tiberius`.
- **`networker-endpoint`** — axum-based server; routes: `/health`, `/echo`, `/download`,
  `/upload`, `/delay`, `/headers`, `/status/:code`, `/http-version`, `/info`;
  ALPN HTTP/1.1 + HTTP/2; `Server-Timing` headers; `X-Networker-Request-Id` echo.
- **SQL Server schema** — `dbo.TestRun`, `dbo.RequestAttempt`, `dbo.HttpResult`, `dbo.TcpResult`,
  `dbo.TlsResult`, `dbo.UdpResult`, `dbo.ThroughputResult`, `dbo.ServerTimingResult`;
  stored procedures; sample queries.
- **CI** — GitHub Actions on Ubuntu + Windows; `cargo test`, `cargo fmt --check`, `cargo clippy`.
- **Installation script** — public Gist serves `install.sh`; compiles from private repo via SSH.

---

[Unreleased]: https://github.com/irlm/networker-tester/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/irlm/networker-tester/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/irlm/networker-tester/compare/v0.3.3...v0.4.0
[0.3.3]: https://github.com/irlm/networker-tester/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/irlm/networker-tester/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/irlm/networker-tester/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/irlm/networker-tester/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/irlm/networker-tester/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/irlm/networker-tester/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/irlm/networker-tester/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/irlm/networker-tester/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/irlm/networker-tester/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/irlm/networker-tester/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/irlm/networker-tester/releases/tag/v0.1.0
