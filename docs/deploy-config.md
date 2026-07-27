# Deploy Config Reference

The `--deploy` flag enables non-interactive, config-driven deployment and testing.
A single JSON file describes where to install the tester, where to deploy endpoint(s),
and what tests to run.

```bash
bash install.sh --deploy deploy.json
```

## Quick Start

```json
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    {
      "provider": "lan",
      "lan": { "ip": "192.168.1.100", "user": "admin" }
    }
  ],
  "tests": {
    "modes": ["http1", "http2", "http3"],
    "runs": 5,
    "insecure": true,
    "html_report": "report.html"
  }
}
```

## Execution Flow

1. **Validate** — JSON syntax, required fields, valid modes
2. **Pre-flight** — tool availability, cloud credentials, SSH connectivity
3. **Display plan** — show what will be deployed and tested
4. **Deploy tester** — install binary on local or remote machine
5. **Deploy endpoint(s)** — install + start service on each endpoint
6. **Generate tester config** — build `networker-cloud.json` from deployed IPs
7. **Run tests** — execute networker-tester (locally or via SSH on remote tester)
8. **Download results** — copy HTML/Excel reports back to local machine
9. **Summary** — display deployed infrastructure and report locations

## Schema

### Top-level

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `version` | number | yes | Schema version. Must be `1`. |
| `tester` | object | yes | Where to install the tester binary. |
| `endpoints` | array | yes | One or more endpoint deployments. |
| `tests` | object | no | Test configuration. Defaults to all modes, 5 runs. |
| `packet_capture` | object | no | Optional packet capture on tester, endpoint, or both. Default is disabled. |
| `impairment` | object | no | Optional benchmark impairment profile. Initial scoped support focuses on delay injection. |
| `dashboard` | object | no | **Legacy.** Installs the retired Rust dashboard stack — see [the `dashboard` object](#dashboard-object-legacy). |

### `tester` object

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `provider` | string | yes | — | `"local"`, `"lan"`, `"azure"`, `"aws"`, or `"gcp"` |
| `install_method` | string | no | `"release"` | `"release"` (download binary) or `"source"` (cargo build) |
| `lan` | object | if provider=lan | — | LAN connection details |
| `azure` | object | if provider=azure | — | Azure VM config |
| `aws` | object | if provider=aws | — | AWS EC2 config |
| `gcp` | object | if provider=gcp | — | GCP GCE config |

### `endpoints[]` items

Each endpoint has the same structure as `tester`, plus:

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `label` | string | no | `"endpoint-1"` | Human-readable name (used in plan/report) |

### LAN provider (`lan` object)

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `ip` | string | yes | — | IP address or hostname |
| `user` | string | no | `""` | SSH username |
| `port` | number | no | `22` | SSH port |

**Pre-flight:** Tests SSH connectivity with `BatchMode=yes`. On failure, suggests `ssh-copy-id`.

### Azure provider (`azure` object)

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `region` | string | no | `"eastus"` | Azure region |
| `resource_group` | string | no | `"networker-rg-endpoint"` | Resource group name |
| `vm_name` | string | no | `"networker-endpoint-vm"` | VM name |
| `vm_size` | string | no | `"Standard_B2s"` | VM size |
| `os` | string | no | `"linux"` | `"linux"` or `"windows"` |
| `auto_shutdown` | boolean | no | `true` | Enable auto-shutdown at 04:00 UTC |

**Pre-flight:** Checks `az` CLI installed and `az account show` succeeds.

### AWS provider (`aws` object)

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `region` | string | no | `"us-east-1"` | AWS region |
| `instance_name` | string | no | `"networker-endpoint"` | EC2 Name tag |
| `instance_type` | string | no | `"t3.small"` | EC2 instance type |
| `os` | string | no | `"linux"` | `"linux"` or `"windows"` |
| `auto_shutdown` | boolean | no | `true` | Install cron to shutdown at 04:00 UTC |

**Pre-flight:** Checks `aws` CLI and `aws sts get-caller-identity`.

### GCP provider (`gcp` object)

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `region` | string | no | `"us-central1"` | GCP region |
| `zone` | string | no | `"us-central1-a"` | GCP zone |
| `instance_name` | string | no | `"networker-endpoint"` | Instance name |
| `machine_type` | string | no | `"e2-small"` | GCE machine type |
| `project_id` | string | no | auto-detected | GCP project ID (alias: `project`). Auto-detect order: this field → service-account email → `gcloud config get-value project`. |
| `os` | string | no | `"linux"` | `"linux"` or `"windows"` |
| `auto_shutdown` | boolean | no | `true` | Install cron to shutdown at 04:00 UTC |

**Pre-flight:** Checks `gcloud` CLI and `gcloud auth print-access-token`.

### `tests` object

All fields are optional. If `tests` is omitted entirely, defaults are used.

**Default modes** (when `modes` is not specified): `tcp`, `http1`, `http2`, `http3`,
`udp`, `download`, `upload`, `pageload`, `pageload2`, `pageload3`.

`apibench` is additionally accepted as a runner-level mode (not a tester
protocol): the agent expands it into one tester run per measured `/api/*`
workload defined in `benchmarks/configs/apibench.json` (API-SPEC.md §4),
driving the frozen request shapes via `--request-body`/`--request-body-file`
over http1/http2. Targets that serve no `/api/*` endpoints (e.g. nginx) are
skipped.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `run_tests` | boolean | `true` | Set `false` for deploy-only (no test execution) |
| `modes` | string[] | see above | Test modes to run |
| `runs` | number | `5` | Number of test iterations per mode |
| `concurrency` | number | `1` | Concurrent connections |
| `timeout` | number | `30` | Timeout per probe in seconds |
| `payload_sizes` | string[] | `["64k", "1m"]` | Sizes for download/upload probes |
| `insecure` | boolean | `false` | Skip TLS certificate verification |
| `connection_reuse` | boolean | `false` | Reuse connections (warm probes) |
| `udp_port` | number | `9999` | UDP echo port |
| `udp_throughput_port` | number | `9998` | UDP throughput port |
| `page_assets` | number | `50` | Number of assets for pageload |
| `page_asset_size` | string | `"50k"` | Size of each page asset |
| `page_preset` | string | — | Page preset name (see below) |
| `retries` | number | `0` | Retry failed probes |
| `html_report` | string | `"report.html"` | HTML report filename |
| `output_dir` | string | `"."` | Output directory for reports |
| `excel` | boolean | `false` | Generate Excel report |
| `dns_enabled` | boolean | `true` | Include DNS resolution timing |
| `ipv4_only` | boolean | `false` | Force IPv4 |
| `ipv6_only` | boolean | `false` | Force IPv6 |
| `verbose` | boolean | `false` | Verbose output |
| `log_level` | string | — | Log level (e.g. `"debug"`, `"info"`) |

### `packet_capture` object

All fields are optional. If omitted, packet capture is disabled.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | string | `"none"` | `"none"`, `"tester"`, `"endpoint"`, or `"both"` |
| `install_requirements` | boolean | implementation default | Try to install `tshark`/`dumpcap` when capture is enabled |
| `interface` | string | `"auto"` | Interface name to capture on (`en0`, `eth0`, etc.) |
| `write_pcap` | boolean | `true` | Save raw `.pcapng` artifacts |
| `write_summary_json` | boolean | `true` | Save parsed packet summary JSON |

> Packet capture is intentionally **off by default**. The installer should only enable it when the
> user explicitly selects it, or when a deploy config requests it.
>
> **macOS note:** having `tshark` installed is not sufficient by itself. Capturing on `lo0`/other
> interfaces also requires Wireshark/TShark BPF permissions (for example via ChmodBPF). If you see
> `/dev/bpf*: Permission denied`, packet capture is configured correctly but the OS permission layer
> still needs to be enabled.
>
> In practice, macOS users may need to **install the full Wireshark app manually** and run its
> **ChmodBPF** helper once. The CLI/package-manager install path can provide `tshark`, but it may
> not complete the privileged BPF permission setup non-interactively.

### `impairment` object

All fields are optional. If omitted, no impairment is applied.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `profile` | string | `"none"` | `"none"`, `"wan"`, `"slow"`, or `"satellite"` |
| `delay_ms` | number | profile default | Explicit delay override in milliseconds |

Current scoped support in this first version focuses on **delay injection** by routing supported
HTTP-family probes through the endpoint's built-in `/delay?ms=N` behavior.

Security note:
- `/delay` is intended for **controlled benchmark environments**.
- Do not expose it broadly on public/shared endpoints unless you understand and accept the risk.
- The client-side config clamps `delay_ms` to a maximum of `10000 ms` in this version.

Default profile mapping:
- `none` → `0 ms`
- `wan` → `40 ms`
- `slow` → `150 ms`
- `satellite` → `600 ms`

**Current scope:** request-style HTTP-family probes only. This first version is meant to make
paper-style delay scenarios easy to reproduce without claiming full traffic shaping or netem-style
loss/jitter control yet.

### Valid test modes

> The canonical, machine-readable mode list lives in
> [`shared/modes.json`](../shared/modes.json) — generated from the engine's
> `Protocol` enum (`crates/networker-tester/src/metrics.rs`) and enforced by
> drift-guard tests in all three stacks (Rust: `modes_manifest_guard.rs`,
> dashboard: `modes-manifest.test.ts`, C#: `ModesManifestTests.cs`). The
> tables below are the deploy-config subset; `pageload1` is a CLI alias for
> `pageload`, and `apibench` is runner-level (see note under Default modes).

**Network probes:**
`tcp`, `http1`, `http2`, `http3`, `udp`, `rpm`, `responsiveness`, `stamp`,
`ping`, `path`, `dualstack`, `websocket`, `pmtud`, `download`, `upload`,
`webdownload`, `webupload`, `udpdownload`, `udpupload`

> `rpm` is the latency-under-load / bufferbloat probe: it samples UDP echo RTT
> on an idle link, then repeats the sampling at a steady cadence while the
> endpoint's `/download` route loads the link. Reports unloaded vs loaded
> RTT (min/avg/p95), loaded jitter + loss, RPM (60000 / loaded avg RTT —
> round-trips per minute, higher is better), and the bufferbloat factor
> (loaded avg / unloaded avg). Loaded-phase echoes are waited for up to the
> full `--udp-timeout` so deep-queue echoes count as high RTTs, not loss
> (`loaded_probes_censored` reports any truncated waits). Requires a
> networker-endpoint target (the `/download` route plus the UDP echo server
> on port 9999). NOTE: this is a UDP-echo-under-single-flow-load diagnostic,
> not the draft-ietf-ippm-responsiveness methodology — its RPM figure is not
> comparable with Apple `networkQuality`/Cloudflare/Ookla RPM numbers.
> For the spec-conformant methodology use `responsiveness`.

> `responsiveness` is the draft-ietf-ippm-responsiveness (draft-08)
> working-conditions probe: per direction (download, then upload) it ramps
> parallel HTTP/2 load connections against the endpoint's `/download` and
> `/upload` routes (1 connection added per 1 s interval, up to 16) until
> goodput stabilizes (stddev of the last 4 moving averages < 5 % of the
> current average), while measuring HTTP probe latency on NEW connections
> ("foreign": TCP + TLS + 1-byte GET) and multiplexed ON the load connections
> ("self" — shares the loaded flow's queue, so flow-isolating AQMs like
> fq_codel cannot hide bufferbloat from it). RPM per direction follows the
> draft's 95 % trimmed-mean formula over the final 4 intervals; capacity at
> saturation, per-component trimmed means, connection counts, and
> `saturation_reached` flags are reported alongside. Cleartext targets use
> h2c (prior knowledge) and the draft's TCP-only aggregation variant.
> Requires a networker-endpoint target. Unlike `rpm`, these figures ARE
> comparable with other draft implementations.

> `stamp` is a STAMP (RFC 8762, unauthenticated mode) probe against the
> endpoint's Session-Reflector (UDP port 9997, `--stamp-port`): 50 probes at
> a fixed 50 ms cadence. The reflector's receive/transmit timestamps and
> stateful sequence number yield RTT corrected for reflector processing time
> ((T4−T1) − (T3−T2) — no clock sync needed), per-direction delay variation
> (near = sender→reflector, far = reflector→sender; clock offsets cancel
> within each direction), and DIRECTIONAL loss (`loss_sent_percent` vs
> `loss_return_percent`). Timestamps are NTP 64-bit. When the run's SNTP
> clock-sync query succeeded, indicative absolute one-way delays are also
> reported with an explicit ± uncertainty (half the SNTP round-trip) —
> labeled estimates, never measurements. Each probe gets its full
> `--udp-timeout` before being declared lost.

> `ping` sends ICMP echo probes (count/timeout follow `--udp-probes` /
> `--udp-timeout`) — no raw sockets, no root on any platform. Unix uses
> unprivileged ICMP datagram sockets; Windows uses the iphlpapi echo API
> (`IcmpSendEcho` for IPv4, `Icmp6SendEcho2` for IPv6). Reports RTT
> min/avg/p95, jitter, loss, and the reply TTL when observable. On Linux the
> process gid must be inside `net.ipv4.ping_group_range` (denials are
> reported as a `config` error with the sysctl fix hint). On Windows the
> reply TTL is reported for IPv4 targets only (the IPv6 echo reply structure
> carries no hop limit).

> `path` is a traceroute-style hop-discovery probe: UDP probes to a constant
> high port (33434) with rising TTL — the 5-tuple is held constant across
> TTLs (Paris-traceroute semantics) so per-flow ECMP load balancers keep all
> hops on one path. On Linux, `IP_RECVERR` yields each hop's router
> address and RTT unprivileged (`method: "udp-ttl/ip-recverr"`). On
> macOS/Windows hop addresses are not observable without raw sockets, so the
> probe degrades honestly to a hop-count estimate + destination reachability
> with `hops: []` (`method: "udp-ttl-estimate"`) — it never fabricates hops.

> `dualstack` resolves A and AAAA separately, runs one HTTP/1.1 GET pinned to
> IPv4 and one pinned to IPv6, and compares per-phase timing (DNS, TCP, TLS,
> TTFB, total) plus a happy-eyeballs (RFC 8305, 250 ms grace) verdict. One
> working family counts as success; a family with no records or a failed leg
> is reported as absent/failed rather than failing the attempt.

> `websocket` runs the full DNS + TCP + TLS ladder, measures the HTTP 101
> upgrade round-trip (`upgrade_ms`), then sends echo messages over the open
> socket against the endpoint's `/ws` route — message RTT min/avg/p95,
> mean inter-probe delay variation (IPDV) as `jitter_ms`, and loss. Message
> count/size/timeout follow
> `--udp-probes` / `--udp-payload` / `--udp-timeout`. Requires a
> networker-endpoint target (the `/ws` echo route).

> `pmtud` discovers the path MTU by binary-searching DF-flagged UDP datagram
> sizes toward the target's UDP echo port. On Linux, ICMP
> fragmentation-needed errors (with the next-hop MTU) are read unprivileged
> from the socket error queue (`IP_RECVERR`) — so it concludes even against
> targets that never answer. On macOS it relies on `IP_DONTFRAG` + `EMSGSIZE`
> delivery. On Windows it sets `IP_DONTFRAGMENT`/`IPV6_DONTFRAG` (`method:
> "df-dontfragment/..."`): oversized sends fail locally with `WSAEMSGSIZE`
> and an ICMP port-unreachable surfaces as `WSAECONNRESET` (delivery
> confirmation), but path ICMP fragmentation-needed is not surfaced to UDP
> sockets — so Windows needs delivery confirmation (echo or
> port-unreachable) to conclude, and without it a path MTU below the local
> MTU is undetectable. When the target runs a networker-endpoint, its UDP
> echo (:9999) positively confirms unfragmented delivery; with no echo, no
> ICMP, and no send errors the probe reports an honest `path_mtu: null`
> (`method: "df-no-feedback"`) — it never fabricates.

**Pageload probes** (HTTP client, no real browser — fetches `/page` manifest + assets):

| Mode | Protocol | Description |
|------|----------|-------------|
| `pageload` | shorthand | Runs all three: pageload1 + pageload2 + pageload3 |
| `pageload1` | HTTP/1.1 | 6 parallel connections (browser-like) |
| `pageload2` | HTTP/2 | Single multiplexed TLS connection |
| `pageload3` | HTTP/3 | Single QUIC connection |

**Browser probes** (real headless Chrome via CDP — requires Chrome/Chromium):

| Mode | Protocol | Description |
|------|----------|-------------|
| `browser` | shorthand | Runs all three: browser1 + browser2 + browser3 |
| `browser1` | HTTP/1.1 | Chrome forced to plain `http://` (no ALPN) |
| `browser2` | HTTP/2 | Chrome with `--disable-quic` |
| `browser3` | HTTP/3 | Chrome with `--origin-to-force-quic-on` + SPKI cert pinning |

> **Note:** All `browser*` and `pageload*` modes require Chrome/Chromium on the tester
> machine. The installer will auto-detect and install it if missing.
>
> `dns`, `tls`, `native`, `curl`, and `sdkprobe` probe modes are supported by the
> tester binary but are not available in deploy-config mode. Use the tester CLI
> directly for those modes.

### `sdkprobe` — LagHound network-vs-server split

`sdkprobe` probes a customer's app that has embedded a **LagHound SDK
endpoint** (see [`docs/sdk/contract-v1.md`](sdk/contract-v1.md)) and splits the
total request latency into a **network** leg and a **server-processing** leg —
the core of the "find the main issue" report.

**What it measures.** It runs an HTTP/1.1 request to `<target>/laghound/echo`
(the cheapest LagHound route that still carries server timing), reusing the
standard per-phase timing path (DNS → TCP → TLS → TTFB → total). The
customer's SDK reports its own processing time via a
`Server-Timing: app;dur=<ms>` response header, from which the tester derives:

| Field | Meaning |
|-------|---------|
| `server_ms` | Server processing — the `app;dur` value (falls back to `total;dur`). |
| `network_ms` | `max(0, ttfb_ms − server_ms)` — request upstream + response first byte. |
| `app_ms` | The raw `app;dur` metric, if present. |
| `split_anomaly` | `true` when the reported server time exceeded the measured wall (clock/measure anomaly); `network_ms` is then clamped to 0. |

DNS, TCP, and TLS are measured client-side exactly as for `http1`.

**Token.** LagHound routes are invisible without the shared secret: a bad or
missing token returns a bare `404`. Provide the token via the
`--laghound-token` flag or the `LAGHOUND_TOKEN` environment variable; it is
sent as the `X-LagHound-Token` header (contract §5). A `404` in `sdkprobe`
mode is classified as a **config** error with the message
`SDK endpoint returned 404 — check token or that /laghound is mounted`.

**Route.** Defaults to `/laghound/echo`. Override with `--laghound-route`
(e.g. to point at a non-default LagHound prefix).

```bash
networker-tester --target https://checkout.example.com \
  --modes sdkprobe --laghound-token "$LAGHOUND_TOKEN"
```

### Page presets

Presets model real-world page profiles (based on HTTP Archive data and sites like microsoft.com).
Use `page_preset` to override `page_assets` and `page_asset_size` with a realistic asset mix.

| Preset | Assets | Total size | Model |
|--------|-------:|-----------:|-------|
| `tiny` | 10 | ~100 KB | Simple landing / API docs |
| `small` | 25 | ~900 KB | Blog / article page |
| `default` | 50 | ~6 MB | Corporate homepage (first-party content) |
| `medium` | 100 | ~10 MB | Full enterprise page (microsoft.com transferred) |
| `large` | 200 | ~31 MB | Media-rich portal (uncompressed resources) |
| `mixed` | 50 | ~7 MB | Varied-size distribution (realistic mix) |

Each preset includes a realistic distribution of asset sizes — small tracking pixels, medium
CSS/JS/fonts, and large images/bundles — rather than uniform-size assets.

### `dashboard` object (legacy)

> **Legacy — do not use for new deployments.** This installer path sets up the
> **retired Rust** `networker-dashboard` stack, whose binaries are no longer
> published in releases (the release train ships the C# control plane instead;
> only older tags carry Rust dashboard assets). The current control plane is
> the C# `Networker.ControlPlane` — deployed to prod by the Release workflow
> (see [`release-flow.md`](release-flow.md)) and operated per
> [`phase2-cutover-runbook.md`](phase2-cutover-runbook.md). The `tester` and
> `endpoints` objects above remain fully current.

Optional. When present, the installer sets up the legacy dashboard control plane on the local
machine. This includes PostgreSQL, the dashboard binary, the agent binary, and the React frontend.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | string | `"local"` | Only `"local"` is currently supported |
| `admin_password` | string | `"admin"` | Dashboard admin password |
| `port` | number | `3000` | Dashboard HTTP port |

Example:
```json
{
  "version": 1,
  "tester": { "provider": "local" },
  "endpoints": [
    { "provider": "lan", "lan": { "ip": "192.168.1.100", "user": "admin" } }
  ],
  "dashboard": {
    "provider": "local",
    "admin_password": "secret",
    "port": 3000
  }
}
```

The dashboard setup installs:
1. **PostgreSQL** — database for storing test results, agents, deployments
2. **networker-dashboard** — axum HTTP server (REST API + WebSocket + static files)
3. **networker-agent** — daemon that connects to dashboard and runs probe jobs
4. **React frontend** — built from source and served by the dashboard at `/`
5. **systemd service** — `networker-dashboard.service` with auto-restart

After install, access the dashboard at `http://localhost:<port>`.

## Examples

- [`examples/configs/deploy.example.json`](../examples/configs/deploy.example.json) — Minimal LAN endpoint with local tester
- [`examples/configs/deploy-lan.json`](../examples/configs/deploy-lan.json) — Multi-endpoint LAN deployment with remote tester
- [`examples/configs/deploy-multi-cloud.json`](../examples/configs/deploy-multi-cloud.json) — Compare Azure vs AWS vs GCP endpoints

## Validation & Limitations

The installer validates your deploy config before any resources are created. Validation
errors are shown with the endpoint index and a description of the problem:

```
Validating deploy config ────
  ✗ endpoints[1]: Windows VM name 'networker-ep-eastus-windows' is 27 chars (max 15)
```

Known constraints:

| Constraint | Scope | Detail |
|------------|-------|--------|
| Windows VM name ≤ 15 characters | Azure, AWS, GCP | Windows computer names (NetBIOS) are limited to 15 characters. Applies to `vm_name` (Azure/GCP) and `instance_name` (AWS) when `os` is `"windows"`. Use short names like `"nw-ep-east"`. |
| LAN requires `ip` | LAN endpoints | The `lan.ip` field is required for LAN provider endpoints. |
| Valid provider required | All entries | Each `tester` and `endpoints[]` entry must specify a valid `provider` (`local`, `lan`, `azure`, `aws`, `gcp`). |
| Schema version must be `1` | Top-level | The `version` field is required and must be `1`. |

## Non-interactive mode

The `--deploy` flag automatically sets `AUTO_YES=1`, so all confirmation prompts
(e.g., VM existence check: reuse/rename/delete) proceed with the default choice
without user input. This is required for CI/CD pipelines and scripted automation.

## Requirements

- **jq** — required for JSON parsing (`brew install jq` / `apt install jq`)
- **SSH key auth** — required for LAN provider (password prompts not supported in non-interactive mode)
- **Cloud CLIs** — required for their respective providers (`az`, `aws`, `gcloud`)
- **Bash 3.2+** — compatible with macOS default bash
