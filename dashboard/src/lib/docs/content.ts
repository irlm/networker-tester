export type DocCategory = 'protocols' | 'metrics' | 'statistics' | 'benchmarks' | 'data-flow';

export interface DocEntry {
  id: string;
  category: DocCategory;
  title: string;
  aliases: string[];
  brief: string;
  detail: string;
}

export const DOC_CATEGORIES: { id: DocCategory; label: string; icon: string }[] = [
  { id: 'protocols', label: 'Protocols', icon: '\u25B6' },
  { id: 'metrics', label: 'Metrics', icon: '\u25C8' },
  { id: 'statistics', label: 'Statistics', icon: '\u25A6' },
  { id: 'benchmarks', label: 'Benchmarks', icon: '\u25A3' },
  { id: 'data-flow', label: 'Data Flow', icon: '\u21BB' },
];

export const DOC_ENTRIES: DocEntry[] = [
  // ── Protocols ──────────────────────────────────────────────────────────

  {
    id: 'proto-tcp',
    category: 'protocols',
    title: 'TCP Connect',
    aliases: ['tcp', 'connect', 'handshake', '3-way'],
    brief: 'DNS resolution + TCP 3-way handshake. No HTTP.',
    detail: `Measures the time to resolve DNS and complete a TCP 3-way handshake.
No TLS or HTTP layer is involved.

Phases: DNS \u2192 TCP
Primary metric: connect_duration_ms
Fields populated: dns, tcp

Terminal output: DNS:0.5ms TCP:1.2ms

Use this to isolate network-level latency from application-level
overhead. High TCP times indicate network congestion or distance.`,
  },
  {
    id: 'proto-dns',
    category: 'protocols',
    title: 'DNS Resolution',
    aliases: ['dns', 'resolve', 'lookup', 'nameserver'],
    brief: 'Standalone DNS resolution without opening a connection.',
    detail: `Resolves the target hostname and records the results without
opening a TCP connection.

Phases: DNS only
Primary metric: duration_ms
Fields populated: dns (duration_ms, query_name, resolved_ips)

Terminal output: DNS:0.5ms \u2192 93.184.216.34

Useful for measuring DNS infrastructure performance in isolation.
Compare across DNS providers or caching layers.`,
  },
  {
    id: 'proto-tls',
    category: 'protocols',
    title: 'TLS Handshake',
    aliases: ['tls', 'ssl', 'certificate', 'cert', 'handshake', 'encryption'],
    brief: 'DNS \u2192 TCP \u2192 TLS only. Captures full certificate chain.',
    detail: `Performs DNS resolution, TCP connect, and TLS handshake without
sending an HTTP request.

Phases: DNS \u2192 TCP \u2192 TLS
Primary metric: handshake_duration_ms
Fields populated: dns, tcp, tls (cert chain, cipher suite,
  TLS version, ALPN, subject, issuer, SANs, expiry)

Useful for auditing certificate configuration, measuring TLS
overhead, and comparing cipher suite performance.`,
  },
  {
    id: 'proto-tlsresume',
    category: 'protocols',
    title: 'TLS Session Resumption',
    aliases: ['tlsresume', 'resumption', 'session', 'ticket', 'tls13'],
    brief: 'Two TLS connections: cold (full) then warm (resumed).',
    detail: `Makes two fresh TLS connections to the same origin. The first
seeds resumption state (TLS 1.3 NewSessionTicket), and the second
succeeds only if rustls reports a resumed handshake.

Primary metric: warm handshake_duration_ms
Fields populated: dns, tcp, tls (with previous_handshake_duration_ms,
  previous_handshake_kind, cold/warm HTTP status codes)

Terminal: cold=full:Xms warm=resumed:Yms resumed=true

Both connections use real HTTP/1.1 requests. The transport result
is useful even if HTTP status is non-2xx.`,
  },
  {
    id: 'proto-http1',
    category: 'protocols',
    title: 'HTTP/1.1',
    aliases: ['http1', 'http', 'h1', 'http/1.1'],
    brief: 'Full HTTP/1.1 request: DNS \u2192 TCP \u2192 TLS (if HTTPS) \u2192 HTTP.',
    detail: `Measures a complete HTTP/1.1 request/response cycle including
all network phases.

Phases: DNS \u2192 TCP \u2192 TLS (if HTTPS) \u2192 HTTP request/response
Primary metric: total_duration_ms
Fields populated: dns, tcp, tls (if HTTPS), http (status_code,
  ttfb_ms, total_duration_ms, cpu_time_ms, csw_voluntary/involuntary)

Terminal: DNS:0.5ms TCP:1.2ms TLS:12.4ms TTFB:3.1ms Total:15.8ms
  CPU:2.3ms CSW:12v/3i

The most common probe for general latency measurement.`,
  },
  {
    id: 'proto-http2',
    category: 'protocols',
    title: 'HTTP/2',
    aliases: ['http2', 'h2', 'http/2', 'alpn', 'hpack'],
    brief: 'HTTP/2 over TLS with ALPN h2 negotiation.',
    detail: `Measures DNS \u2192 TCP \u2192 TLS (ALPN "h2") \u2192 HTTP/2 request/response.
TLS is required for ALPN negotiation.

Phases: DNS \u2192 TCP \u2192 TLS \u2192 HTTP/2
Primary metric: total_duration_ms
Fields populated: same as http1 plus tls always present

CPU and CSW will be higher than H/1.1 for large payloads due to
HPACK header compression overhead. Attempting http2 over plain
HTTP will fail with a TLS error.`,
  },
  {
    id: 'proto-http3',
    category: 'protocols',
    title: 'HTTP/3 (QUIC)',
    aliases: ['http3', 'h3', 'quic', 'http/3', 'udp-based'],
    brief: 'HTTP/3 over QUIC \u2014 UDP-based, no separate DNS/TCP phase.',
    detail: `Uses QUIC protocol which combines transport + crypto into a single
UDP-based handshake. There is no separate DNS or TCP phase.

Phases: QUIC handshake (1-RTT including TLS 1.3) \u2192 HTTP/3
Primary metric: total_duration_ms
Fields populated: tls (labeled QUIC:), http \u2014 dns and tcp are None

Terminal: QUIC:Xms TTFB:Xms Total:Xms CPU:Xms CSW:Xv/Xi

CPU is higher than H/1.1 or H/2 because encryption runs in
userspace per UDP datagram rather than in the kernel TCP stack.
UDP must not be firewalled for this probe to work.`,
  },
  {
    id: 'proto-udp',
    category: 'protocols',
    title: 'UDP Echo',
    aliases: ['udp', 'echo', 'rtt', 'jitter', 'packet loss'],
    brief: 'Round-trip time for UDP packets to a UDP echo server.',
    detail: `Sends UDP probes to an echo server and measures per-packet RTT.

Primary metric: rtt_avg_ms
Fields populated: udp (rtt_min_ms, rtt_avg_ms, rtt_p95_ms,
  jitter_ms, loss_percent, probe_count, success_count)

Terminal: UDP RTT min/mean/max/jitter Loss

Captures the full RTT distribution per-probe. Jitter is computed
as the average absolute difference between consecutive RTT values.
Loss percent = (probe_count - success_count) / probe_count * 100.`,
  },
  {
    id: 'proto-download',
    category: 'protocols',
    title: 'Download (Throughput)',
    aliases: ['download', 'throughput', 'bandwidth', 'speed', 'download1', 'download2', 'download3'],
    brief: 'Bulk HTTP download from /download endpoint. Measures throughput.',
    detail: `Measures end-to-end download throughput from the networker-endpoint
/download route. URL path is rewritten: /health \u2192 /download?bytes=N.

Variants: download (H/1.1), download1 (H/1.1), download2 (H/2),
  download3 (H/3)
Primary metric: throughput_mbps
Fields populated: all http fields plus throughput_mbps, goodput_mbps,
  cpu_time_ms, csw_*, server_timing.proc_ms, server_timing.srv_csw_*

Throughput = payload_bytes / (total_duration_ms - ttfb_ms)
  \u2192 body receive phase only
Goodput = payload_bytes / (dns + tcp + tls + total_duration_ms)
  \u2192 full delivery including connection setup

Use --payload-sizes to test different transfer sizes (e.g. 64k,1m,10m).`,
  },
  {
    id: 'proto-upload',
    category: 'protocols',
    title: 'Upload (Throughput)',
    aliases: ['upload', 'upload1', 'upload2', 'upload3', 'post'],
    brief: 'Bulk HTTP upload to /upload endpoint. Measures throughput.',
    detail: `Measures end-to-end upload throughput to the networker-endpoint
/upload route. URL path is rewritten automatically.

Variants: upload (H/1.1), upload1 (H/1.1), upload2 (H/2),
  upload3 (H/3)
Primary metric: throughput_mbps
Fields populated: same as download but server_timing.recv_ms
  replaces proc_ms

Throughput formula uses max(server_recv_ms, ttfb_ms) to avoid
near-zero readings when the server responds before fully draining
the request body.`,
  },
  {
    id: 'proto-webdownload',
    category: 'protocols',
    title: 'Web Download / Web Upload',
    aliases: ['webdownload', 'webupload', 'labeled'],
    brief: 'Same as download/upload but with a different report label.',
    detail: `Uses the same /download and /upload routes as the download/upload
probes. The only difference is the protocol label in the output
and report (webdownload vs download, webupload vs upload).

Useful when you want side-by-side comparison groups in a report
without the labels colliding.

Fields populated: identical to download/upload respectively.`,
  },
  {
    id: 'proto-udpdownload',
    category: 'protocols',
    title: 'UDP Throughput',
    aliases: ['udpdownload', 'udpupload', 'nwkt', 'datagram', 'bulk udp'],
    brief: 'Bulk UDP throughput using custom NWKT protocol (port 9998).',
    detail: `Measures bulk UDP throughput using the custom NWKT protocol on
the endpoint's UDP port (default 9998).

Variants: udpdownload, udpupload
Primary metric: throughput_mbps
Fields populated: udp_throughput (payload_bytes, datagrams_sent,
  datagrams_received, bytes_acked, loss_percent, transfer_ms,
  throughput_mbps)

Unlike TCP-based throughput, this captures raw datagram-level
performance including loss characteristics.`,
  },
  {
    id: 'proto-pageload',
    category: 'protocols',
    title: 'Page Load (Synthetic)',
    aliases: ['pageload', 'pageload2', 'pageload3', 'page', 'assets', 'multi-asset'],
    brief: 'Simulated browser page load: root HTML + N parallel assets.',
    detail: `Simulates a browser page load: fetches a root HTML page then
downloads N parallel assets.

Variants:
  pageload  \u2014 HTTP/1.1, up to 6 concurrent TCP connections
  pageload2 \u2014 HTTP/2, single connection, multiplexed
  pageload3 \u2014 HTTP/3 over QUIC, single connection, multiplexed

Primary metric: total_ms
Fields populated: page_load (asset_count, assets_fetched,
  total_bytes, total_ms, ttfb_ms, connections_opened,
  tls_setup_ms, tls_overhead_ratio, cpu_time_ms)

connections_opened: 1 for H2/H3, up to 6 for H1
tls_overhead_ratio: sum of TLS handshake times / total time

Configure with --page-assets (count) and --page-asset-size.
Requires networker-endpoint (uses /page + /asset routes).`,
  },
  {
    id: 'proto-browser',
    category: 'protocols',
    title: 'Browser (Real Chromium)',
    aliases: ['browser', 'browser1', 'browser2', 'browser3', 'chrome', 'cdp', 'headless'],
    brief: 'Real headless Chromium via CDP. Actual browser page-load metrics.',
    detail: `Drives a real headless Chromium instance via Chrome DevTools Protocol
(chromiumoxide) to measure actual page-load performance.

Requires: --features browser at compile time + local Chrome install
Primary metric: load_ms
Fields populated: browser (load_ms, dom_content_loaded_ms, ttfb_ms,
  resource_count, transferred_bytes, protocol, resource_protocols)

Terminal: [browser] proto=h2 TTFB:Xms DCL:Xms Load:Xms res=21

URL is rewritten to /page so results are directly comparable with
pageload/pageload2/pageload3 synthetic probes.

Chrome binary search: NETWORKER_CHROME_PATH env \u2192 system paths.
If no Chrome found, the probe returns a skipped attempt (no crash).`,
  },
  {
    id: 'proto-native',
    category: 'protocols',
    title: 'Native TLS',
    aliases: ['native', 'schannel', 'securetransport', 'openssl', 'platform tls'],
    brief: 'HTTP/1.1 using the platform native TLS stack.',
    detail: `Like http1 but uses the platform's native TLS library instead
of rustls:
  macOS: Secure Transport
  Windows: SChannel
  Linux: OpenSSL

Requires: --features native
Primary metric: total_duration_ms
Fields populated: same as http1; tls.tls_backend = "native-tls"

Useful for comparing rustls performance against the OS TLS stack.`,
  },
  {
    id: 'proto-curl',
    category: 'protocols',
    title: 'curl (System Binary)',
    aliases: ['curl', 'ground truth', 'baseline', 'write-out'],
    brief: 'Runs system curl binary and parses --write-out timing fields.',
    detail: `Spawns the system curl binary and parses its --write-out timing
output. Useful as a ground-truth baseline to validate the Rust
implementation's measurements.

Primary metric: total_duration_ms
Fields populated: http fields from curl timing; tls.tls_backend = "curl"

Timing fields captured: DNS lookup, TCP connect, TLS handshake,
TTFB (time_starttransfer), total time.`,
  },

  // ── Metrics ────────────────────────────────────────────────────────────

  {
    id: 'metric-ttfb',
    category: 'metrics',
    title: 'TTFB (Time to First Byte)',
    aliases: ['ttfb', 'time to first byte', 'first byte', 'starttransfer'],
    brief: 'Time from request sent to first response byte received.',
    detail: `TTFB measures the delay between sending the HTTP request and
receiving the first byte of the response. It includes:
  - Server processing time
  - Network latency (one-way, request \u2192 response start)

It does NOT include: DNS, TCP, or TLS handshake time.

TTFB is the most important single metric for perceived latency
in HTTP probes. A high TTFB with low TCP/TLS times points to
server-side bottlenecks or backend latency.

Field: http.ttfb_ms`,
  },
  {
    id: 'metric-throughput',
    category: 'metrics',
    title: 'Throughput vs Goodput',
    aliases: ['throughput', 'goodput', 'mbps', 'bandwidth', 'speed', 'transfer rate'],
    brief: 'Throughput = body phase only. Goodput = full delivery including setup.',
    detail: `Two complementary throughput measurements:

Throughput (MB/s):
  payload_bytes / (total_duration_ms - ttfb_ms)
  Measures the raw body transfer rate only, excluding connection
  setup and server processing. Best for comparing raw network
  bandwidth across protocols.

Goodput (MB/s):
  payload_bytes / (dns_ms + tcp_ms + tls_ms + total_duration_ms)
  Measures effective delivery rate including all overhead. Better
  reflects real-world user experience for small transfers where
  connection setup is a significant fraction.

Upload throughput uses: max(server_recv_ms, ttfb_ms) to avoid
near-zero readings when the server responds before fully draining
the body.

Fields: http.throughput_mbps, http.goodput_mbps`,
  },
  {
    id: 'metric-jitter',
    category: 'metrics',
    title: 'Jitter',
    aliases: ['jitter', 'variation', 'consistency', 'stability'],
    brief: 'Average absolute difference between consecutive RTT values.',
    detail: `Jitter measures the variability of round-trip times in UDP probes.
Computed as the mean of absolute differences between consecutive
RTT measurements:

  jitter = mean(|rtt[i+1] - rtt[i]|) for i = 0..n-2

Low jitter (\u2264 1ms) indicates a stable, predictable network path.
High jitter causes buffering issues for real-time protocols
(VoIP, video, gaming).

Field: udp.jitter_ms
Related: udp.rtt_avg_ms, udp.rtt_p95_ms`,
  },
  {
    id: 'metric-loss',
    category: 'metrics',
    title: 'Packet Loss',
    aliases: ['loss', 'packet loss', 'drop', 'loss percent'],
    brief: 'Percentage of probes that did not receive a response.',
    detail: `Packet loss percentage for UDP probes:

  loss_percent = (probe_count - success_count) / probe_count * 100

Any loss above 0% in a controlled environment indicates:
  - Network congestion
  - Firewall interference
  - Endpoint overload
  - Path instability

For UDP throughput probes (udpdownload/udpupload), loss is
measured at the datagram level:
  datagrams_sent vs datagrams_received

Fields: udp.loss_percent, udp_throughput.loss_percent`,
  },
  {
    id: 'metric-cpu-csw',
    category: 'metrics',
    title: 'CPU Time & Context Switches',
    aliases: ['cpu', 'csw', 'context switch', 'voluntary', 'involuntary', 'resource usage'],
    brief: 'Process CPU time and context switches per probe (Unix only).',
    detail: `Per-probe resource usage metrics (Unix/Linux/macOS only):

Client-side:
  http.cpu_time_ms     \u2014 Process CPU (user+sys) consumed by this probe
  http.csw_voluntary   \u2014 Voluntary context switches (I/O waits)
  http.csw_involuntary \u2014 Involuntary context switches (preemptions)

Server-side (from Server-Timing header):
  server_timing.srv_csw_voluntary
  server_timing.srv_csw_involuntary

Terminal: CPU:2.3ms CSW:12v/3i sCSW:4v/1i

High voluntary CSW = lots of I/O waits (normal for network probes)
High involuntary CSW = CPU contention (noisy neighbor, overloaded)

HTTP/3 shows higher CPU than H/1.1 because QUIC runs encryption
in userspace rather than the kernel TCP stack.`,
  },
  {
    id: 'metric-server-timing',
    category: 'metrics',
    title: 'Server Timing',
    aliases: ['server timing', 'server-timing', 'recv_ms', 'proc_ms', 'clock skew'],
    brief: 'Server-side processing metrics from the Server-Timing header.',
    detail: `The networker-endpoint injects a Server-Timing HTTP header with:

  server_timing.request_id     \u2014 Echo of client request ID
  server_timing.server_timestamp \u2014 Server wall clock
  server_timing.clock_skew_ms  \u2014 Client vs server clock difference
  server_timing.recv_body_ms   \u2014 Time server spent draining upload body
  server_timing.processing_ms  \u2014 Time server spent generating download body
  server_timing.total_server_ms \u2014 Total server-side duration
  server_timing.srv_csw_*      \u2014 Server context switches

Clock skew is useful for detecting NTP drift between client and
server. Large skew can distort timing measurements.`,
  },
  {
    id: 'metric-tls-overhead',
    category: 'metrics',
    title: 'TLS Overhead Ratio',
    aliases: ['tls overhead', 'tls_overhead_ratio', 'tls_setup_ms', 'encryption cost'],
    brief: 'Sum of TLS handshake times / total page load time.',
    detail: `For page load probes (pageload, pageload2, pageload3):

  tls_setup_ms = sum of all TLS handshake durations
  tls_overhead_ratio = tls_setup_ms / total_ms

pageload (H/1.1): up to 6 TLS handshakes (one per connection)
pageload2 (H/2): 1 TLS handshake (single multiplexed connection)
pageload3 (H/3): 1 QUIC handshake

This metric directly shows the cost of connection proliferation
in HTTP/1.1 vs the multiplexing advantage of HTTP/2 and HTTP/3.

Fields: page_load.tls_setup_ms, page_load.tls_overhead_ratio`,
  },
  {
    id: 'metric-tcp-kernel',
    category: 'metrics',
    title: 'TCP Kernel Stats',
    aliases: ['kernel', 'mss', 'cwnd', 'retransmit', 'congestion', 'delivery rate', 'tcp info'],
    brief: 'Extended kernel TCP stats: MSS, cwnd, retransmits, congestion algo.',
    detail: `On Linux and macOS, the TCP probe captures kernel-level stats
from the socket after the request completes:

  tcp.mss_bytes          \u2014 Maximum segment size
  tcp.rtt_estimate_ms    \u2014 Kernel smoothed RTT estimate
  tcp.retransmits        \u2014 Retransmit count for this connection
  tcp.total_retrans      \u2014 Total retransmissions
  tcp.snd_cwnd           \u2014 Sender congestion window (segments)
  tcp.snd_ssthresh       \u2014 Slow start threshold
  tcp.rtt_variance_ms    \u2014 RTT variance
  tcp.rcv_space          \u2014 Receive window space
  tcp.segs_out / segs_in \u2014 Segments sent/received
  tcp.congestion_algorithm \u2014 e.g. "cubic", "bbr"
  tcp.delivery_rate_bps  \u2014 Kernel estimated delivery rate
  tcp.min_rtt_ms         \u2014 Minimum observed RTT

These are especially useful for diagnosing middlebox interference,
MTU issues, and congestion behavior.`,
  },

  // ── Statistics ─────────────────────────────────────────────────────────

  {
    id: 'stat-percentiles',
    category: 'statistics',
    title: 'Percentiles (p5, p25, p50, p75, p95, p99)',
    aliases: ['percentile', 'p5', 'p25', 'p50', 'p75', 'p95', 'p99', 'p999', 'quantile', 'median'],
    brief: 'Distribution markers computed via linear interpolation.',
    detail: `Percentiles describe the distribution of measurements. For example,
p95 = 12.3ms means 95% of measurements were \u2264 12.3ms.

Computation (linear interpolation, matches Rust backend):
  1. Sort all values ascending
  2. rank = (p / 100) * (n - 1)
  3. lo = floor(rank), hi = ceil(rank)
  4. result = values[lo] + (values[hi] - values[lo]) * (rank - lo)

Available percentiles: p5, p25, p50 (median), p75, p95, p99
Benchmark mode adds: p999

Key percentiles:
  p50 (median) \u2014 typical experience, robust to outliers
  p95 \u2014 tail latency, what slow users experience
  p99 \u2014 worst-case latency for SLA monitoring
  p5  \u2014 best-case, useful for detecting measurement floor`,
  },
  {
    id: 'stat-mean-stddev',
    category: 'statistics',
    title: 'Mean & Standard Deviation',
    aliases: ['mean', 'average', 'stddev', 'standard deviation', 'variance', 'spread'],
    brief: 'Arithmetic mean and population standard deviation.',
    detail: `Mean: sum of all values / count
  Sensitive to outliers. A single 10x spike pulls the mean up.
  Prefer p50 (median) for "typical" experience.

Standard deviation (population):
  variance = sum((value - mean)^2) / count
  stddev = sqrt(variance)

Uses population variance (divides by N, not N-1) because we have
the complete dataset, not a sample of a larger population.

Low stddev relative to mean = consistent measurements.
High stddev = high variability, investigate outliers.

Coefficient of Variation (CV) in benchmark mode:
  cv = stddev / mean
  Used by the quality engine to assess measurement noise.`,
  },
  {
    id: 'stat-success-rate',
    category: 'statistics',
    title: 'Success Rate',
    aliases: ['success', 'failure', 'error', 'rate', 'reliability'],
    brief: 'Percentage of probe attempts that completed successfully.',
    detail: `success_rate = (successful_attempts / total_attempts) * 100

Color coding in the dashboard:
  \u2265 100% \u2014 green  (perfect)
  \u2265 80%  \u2014 yellow (degraded)
  < 80%  \u2014 red    (failing)

Failed attempts are excluded from statistical calculations
(percentiles, mean, stddev) to avoid skewing results with
timeout/error values. The success rate is always displayed
alongside stats so you know the denominator.`,
  },
  {
    id: 'stat-primary-metric',
    category: 'statistics',
    title: 'Primary Metric Selection',
    aliases: ['primary metric', 'higher is better', 'lower is better', 'metric selection'],
    brief: 'Each protocol has a primary metric. Latency = lower is better. Throughput = higher is better.',
    detail: `The primary metric determines what value is used for statistics,
charts, and comparisons:

Latency protocols (lower is better):
  tcp       \u2192 connect_duration_ms
  dns       \u2192 duration_ms
  tls       \u2192 handshake_duration_ms
  udp       \u2192 rtt_avg_ms
  http1/2/3 \u2192 total_duration_ms
  pageload* \u2192 total_ms
  browser*  \u2192 load_ms

Throughput protocols (higher is better):
  download* \u2192 throughput_mbps
  upload*   \u2192 throughput_mbps
  webdownload/webupload \u2192 throughput_mbps

This selection is consistent between the Rust backend
(metrics.rs::primary_metric_value) and the frontend
(lib/analysis.ts::primaryMetricValue).`,
  },
  {
    id: 'stat-timing-breakdown',
    category: 'statistics',
    title: 'Timing Breakdown',
    aliases: ['breakdown', 'phases', 'waterfall', 'dns tcp tls ttfb'],
    brief: 'Average time per phase: DNS \u2192 TCP \u2192 TLS \u2192 TTFB \u2192 Total.',
    detail: `For latency protocols, the timing breakdown shows the average
duration of each network phase:

  DNS  \u2014 dns.duration_ms (name resolution)
  TCP  \u2014 tcp.connect_duration_ms (3-way handshake)
  TLS  \u2014 tls.handshake_duration_ms (TLS negotiation)
  TTFB \u2014 http.ttfb_ms (server processing + first byte)
  Total \u2014 http.total_duration_ms (complete round-trip)

Not all protocols populate all phases:
  tcp  \u2192 DNS + TCP only
  dns  \u2192 DNS only
  http3 \u2192 QUIC (labeled as TLS) + TTFB + Total
  udp  \u2192 separate RTT distribution (no phase breakdown)

Throughput protocols are excluded from timing breakdown
(they have their own throughput summary).`,
  },

  // ── Benchmarks ─────────────────────────────────────────────────────────

  {
    id: 'bench-phases',
    category: 'benchmarks',
    title: 'Benchmark Phases',
    aliases: ['phase', 'warmup', 'pilot', 'overhead', 'measurement', 'cooldown', 'sample'],
    brief: 'Five phases: warmup \u2192 pilot \u2192 overhead \u2192 measurement \u2192 cooldown.',
    detail: `Benchmark mode runs probes through five sequential phases:

1. Warmup
   - Stabilizes caches, JIT, connection pools
   - Results are discarded (not included in statistics)
   - Configurable attempt count

2. Pilot
   - Small initial sample to estimate variance
   - Used by adaptive sampling to determine how many measurement
     samples are needed for the target statistical confidence

3. Overhead
   - Measures framework/instrumentation overhead
   - Subtracted from measurement results (if significant)

4. Measurement (primary phase)
   - The actual samples used for statistics and reporting
   - Sample count determined by pilot analysis (adaptive) or
     fixed by configuration
   - Only "included" samples count (outliers may be excluded)

5. Cooldown
   - Allows the system to settle after measurement
   - Results are discarded

Each phase's sample count is recorded in the benchmark artifact.`,
  },
  {
    id: 'bench-adaptive',
    category: 'benchmarks',
    title: 'Adaptive Sampling',
    aliases: ['adaptive', 'sample size', 'pilot', 'confidence', 'relative error', 'execution plan'],
    brief: 'Pilot phase determines measurement sample count for target confidence.',
    detail: `The benchmark execution plan controls sample sizing:

  source: "adaptive" or "fixed"
  pilot_sample_count: initial samples to estimate variance
  min_samples / max_samples: bounds on measurement phase
  min_duration_ms: minimum measurement duration
  target_relative_error: desired margin of error (e.g. 0.05 = 5%)
  target_absolute_error: alternative absolute error target

Adaptive flow:
  1. Run pilot_sample_count probes
  2. Compute variance from pilot data
  3. Calculate required N for target_relative_error at 95% CI
  4. Clamp N between min_samples and max_samples
  5. Run that many measurement samples

This ensures low-variance measurements use fewer samples
(fast) while high-variance measurements get more samples
(accurate).`,
  },
  {
    id: 'bench-environment',
    category: 'benchmarks',
    title: 'Environment & Stability Checks',
    aliases: ['environment', 'stability', 'baseline', 'network type', 'noise', 'pre-check'],
    brief: 'Pre-measurement checks: baseline RTT, jitter, packet loss, network type.',
    detail: `Before measurement, benchmarks run environment checks:

Environment Check:
  - Sends baseline probes to measure network conditions
  - Records: rtt_min/avg/max/p50/p95, packet_loss_percent
  - Classifies network_type (e.g. "local", "regional", "internet")

Stability Check (extends environment check):
  - Adds jitter_ms measurement
  - Verifies conditions are stable enough for benchmarking

Noise Thresholds:
  max_packet_loss_percent \u2014 fail if loss exceeds this
  max_jitter_ratio        \u2014 fail if jitter/rtt_avg exceeds this
  max_rtt_spread_ratio    \u2014 fail if (p95-p5)/p50 exceeds this

If checks fail, the benchmark may still run but results are
flagged as having poor data quality.`,
  },
  {
    id: 'bench-quality',
    category: 'benchmarks',
    title: 'Data Quality & Publication Readiness',
    aliases: ['quality', 'publication', 'noise level', 'sufficiency', 'tier', 'blocker', 'warning'],
    brief: 'Quality engine scores results: noise level, sufficiency, publication readiness.',
    detail: `After measurement, the quality engine evaluates results:

noise_level:
  "low"    \u2014 CV < 5%, highly consistent
  "medium" \u2014 CV 5-15%, acceptable
  "high"   \u2014 CV > 15%, noisy data

sufficiency:
  "sufficient"   \u2014 enough samples for target confidence
  "insufficient" \u2014 need more samples

publication_ready: boolean
  true only if no publication blockers exist

quality_tier: overall grade

publication_blockers: string[]
  Critical issues preventing publication (e.g. "high packet loss",
  "insufficient samples", "unstable environment")

warnings: string[]
  Non-blocking issues (e.g. "high jitter detected",
  "outliers removed")

Outlier handling:
  low_outlier_count, high_outlier_count \u2014 removed outliers
  outlier_policy \u2014 method used (e.g. "iqr", "zscore")
  confidence_level \u2014 typically 0.95 (95% CI)
  relative_margin_of_error \u2014 CI width relative to mean`,
  },
  {
    id: 'bench-comparison',
    category: 'benchmarks',
    title: 'Benchmark Comparison',
    aliases: ['compare', 'comparison', 'delta', 'regression', 'baseline', 'candidate'],
    brief: 'Compare runs: baseline vs candidates with delta %, verdict, and comparability checks.',
    detail: `The comparison engine evaluates benchmark runs against a baseline:

Comparability checks:
  - Same protocol and payload size
  - Compatible environment (OS, arch, CPU cores, region)
  - Similar network conditions (baseline RTT, network type)
  - comparability_notes explain any issues

Per-case comparison:
  absolute_delta \u2014 candidate - baseline
  percent_delta  \u2014 (candidate - baseline) / baseline * 100
  ratio          \u2014 candidate / baseline
  verdict        \u2014 "faster", "slower", "same", "inconclusive"

Regression detection:
  Automated detection compares configs over time.
  severity: "minor", "major", "critical"
  Tracked per language + metric combination.`,
  },

  // ── Data Flow ──────────────────────────────────────────────────────────

  {
    id: 'flow-collection',
    category: 'data-flow',
    title: 'Data Collection',
    aliases: ['collection', 'agent', 'tester', 'probe', 'attempt', 'request'],
    brief: 'Agent executes probes via networker-tester, streams results to dashboard.',
    detail: `Data collection flow:

1. Dashboard creates a Job with target URL, modes, and parameters
2. Agent picks up the Job via WebSocket or polling
3. Agent spawns networker-tester as a subprocess with the job config
4. Tester executes probes sequentially:
   - Each probe produces a RequestAttempt (JSON)
   - Contains: dns, tcp, tls, http, udp, page_load, browser results
   - Plus metadata: protocol, sequence_num, success, timestamps
5. Agent streams each attempt to Dashboard via WebSocket
6. Dashboard persists attempts to PostgreSQL
7. Dashboard broadcasts live updates to connected browsers

Each attempt is self-contained with all timing data.
The tester also writes local output files (JSON, HTML, Excel).`,
  },
  {
    id: 'flow-processing',
    category: 'data-flow',
    title: 'Data Processing',
    aliases: ['processing', 'analysis', 'computation', 'stats', 'frontend'],
    brief: 'Frontend computes statistics client-side from raw attempt data.',
    detail: `The frontend (lib/analysis.ts) processes raw attempt data:

1. Group attempts by protocol (and payload size for throughput)
2. Filter to successful attempts only
3. Extract primary metric value per attempt
4. Compute statistics: min, mean, p5/p25/p50/p75/p95/p99, max, stddev
5. Compute timing breakdown: avg DNS/TCP/TLS/TTFB/Total per protocol
6. Calculate success rate per protocol

Key functions:
  computeStats(values)          \u2192 Stats object
  primaryMetricValue(attempt)   \u2192 number (protocol-dependent)
  primaryMetricLabel(protocol)  \u2192 string label
  computeProtocolStats(attempts) \u2192 ProtocolStats[]
  computeTimingBreakdown(attempts) \u2192 TimingBreakdown[]
  isThroughputProtocol(protocol) \u2192 boolean

Statistics are computed identically on the Rust backend
(metrics.rs::compute_stats) and the TypeScript frontend
(lib/analysis.ts::computeStats) using the same algorithm.`,
  },
  {
    id: 'flow-display',
    category: 'data-flow',
    title: 'Data Display',
    aliases: ['display', 'chart', 'table', 'visualization', 'render', 'dashboard', 'ui'],
    brief: 'Results shown as statistical tables, box-whisker charts, and timing breakdowns.',
    detail: `The dashboard displays processed data in several views:

Run Detail:
  - Protocol stats table (p50, p95, p99, mean, stddev, success rate)
  - Timing breakdown table (avg DNS/TCP/TLS/TTFB/Total per protocol)
  - Box-whisker charts (min/p25/p50/p75/max distribution)
  - Raw attempt timeline

Benchmark Detail:
  - Case summary table (per protocol + payload combination)
  - Distribution stats (CI, margin of error, sample counts)
  - Data quality indicators (noise level, sufficiency badges)
  - Launch-by-launch phase breakdown

Comparison View:
  - Side-by-side baseline vs candidates
  - Delta percentages with color coding
  - Verdicts (faster/slower/same)
  - Environment comparability notes

Formatting:
  Latency: ms with 2 decimal places (< 100ms) or 1 decimal (>= 100ms)
  Sub-ms: displayed as \u00b5s (microseconds)
  Throughput: MB/s or GB/s (auto-scaled)
  Bytes: auto-scaled (B, KB, MB, GB)`,
  },
];
