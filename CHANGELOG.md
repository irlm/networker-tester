# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.2.4] – 2026-02-28 — Versioning display

### Added
- `networker-endpoint` emits `X-Networker-Server-Version` on every response via the `add_server_timestamp` middleware.
- `ServerTimingResult` gains `server_version: Option<String>` — captured in JSON output per attempt.
- Terminal summary now shows both **Client version** and **Server version** rows.
- HTML report Run Summary card shows Client and Server version rows.
- Version is logged at tester startup.

### Changed
- Workspace `Cargo.toml` version bumped to `0.2.4` — cascades to both binaries automatically.

---

## [0.2.3] – 2026-02-27 — Upload throughput: max() guard

### Fixed
- Upload throughput was still near-zero on some endpoints that respond to the client before fully draining the request body. Changed denominator from `server_recv_ms.unwrap_or(total_duration_ms)` to `max(server_recv_ms, total_duration_ms)` so the larger (correct) value is always used. (#24)

---

## [0.2.2] – 2026-02-27 — Upload throughput: ttfb_ms as stopwatch

### Fixed
- Upload throughput showed absurdly high values (millions of MB/s) because `total_duration_ms` includes the time to receive the server's response body — noise, not upload wire time.
- `ttfb_ms` is now the primary upload denominator: it starts just before `send_request()` and stops when the server sends response headers. Because `networker-endpoint` only responds after draining the full request body, `ttfb_ms` captures the actual upload wire time.
- Formula is now `max(server_recv_ms, ttfb_ms)`: `server_recv_ms` (Server-Timing: recv;dur=X) wins when it is larger (old-style endpoints that respond before draining); `ttfb_ms` wins otherwise. (#25)

---

## [0.2.1] – 2026-02-26 — Upload throughput: server-side timing

### Fixed
- Upload throughput: switched denominator from client-side `ttfb_ms` (near-zero for same-machine connections because hyper resolves `send_request` as soon as headers arrive) to `server_recv_ms` from `Server-Timing: recv;dur=X` header — the time the server spent draining the request body, which is accurate regardless of network path. (#22)

---

## [0.2.0] – 2026-02-25 — Extended metrics: TCP kernel stats, retries, server timing, Excel

### Added
- **TCP kernel stats** — 8 new fields on `TcpResult`: `retransmits`, `total_retrans`, `snd_cwnd`, `snd_ssthresh`, `rtt_variance_ms`, `rcv_space`, `segs_out`, `segs_in`.
  - Linux: read via `TCP_INFO` socket option (no root required).
  - macOS: read via `TCP_CONNECTION_INFO` (`tcp_connection_info` struct at `IPPROTO_TCP` opt `0x24`).
- **Application-level retries** — `--retries N` CLI flag; failed probes are retried up to N times; `retry_count` field added to `RequestAttempt`.
- **Server timing** — `ServerTimingResult` struct captures `Server-Timing` header fields (`recv`, `proc`, `total`), `X-Networker-Server-Timestamp`, clock skew estimate, and echoed `X-Networker-Request-Id`.
- **Excel output** — `--excel` CLI flag; generates an `.xlsx` report alongside JSON + HTML using `rust_xlsxwriter`. 8 sheets: Summary, HTTP Timings, TCP Stats, TLS Details, UDP Stats, Throughput, Server Timing, Errors.
- **Privilege notice** — on Linux without root, a startup message explains which metrics are still captured vs. what would require elevated privileges.
- **`networker-endpoint` server timing** — `/download` returns `Server-Timing: proc;dur=X`; `/upload` returns `Server-Timing: recv;dur=X` and echoes `X-Networker-Request-Id`.
- **SQL migrations** — `sql/05_ExtendedTcpStats.sql` (8 new `TcpResult` columns + `RetryCount` on `RequestAttempt`) and `sql/06_ServerTiming.sql` (`ServerTimingResult` table). (#21)

---

## [0.1.0] – 2026-02-20 — Initial release

### Added
- **Workspace** — Cargo workspace with two crates: `networker-tester` (CLI) and `networker-endpoint` (server).
- **Probe modes**: `http1`, `http2`, `tcp`, `udp`, `download`, `upload`; HTTP/3 gated behind `--features http3`.
- **Per-phase timing** — DNS, TCP connect, TLS handshake, TTFB, total; measured using raw `hyper 1.x` connection APIs.
- **TLS** — `rustls 0.23` with `ring` provider; self-signed cert via `rcgen`; `--insecure` flag.
- **UDP echo** — configurable probe count, RTT percentiles, jitter, loss%.
- **Download/upload throughput probes** — `GET /download?bytes=N` and `POST /upload`.
- **Output formats** — JSON, HTML report (embedded CSS, protocol comparison tables), SQL Server via `tiberius`.
- **`networker-endpoint`** — axum-based server; routes: `/health`, `/echo`, `/download`, `/upload`, `/delay`, `/headers`, `/status/:code`, `/http-version`, `/info`; ALPN HTTP/1.1 + HTTP/2.
- **SQL Server schema** — `dbo.TestRun`, `dbo.RequestAttempt`, `dbo.HttpResult`, `dbo.TcpResult`, `dbo.TlsResult`, `dbo.UdpResult`, `dbo.ThroughputResult`; stored procedures; sample queries.
- **CI** — GitHub Actions on Ubuntu + Windows; `cargo test`, `cargo fmt --check`, `cargo clippy`.
- **Installation script** — `install.sh` builds and installs both binaries from source.

---

[0.2.4]: https://github.com/irlm/networker-tester/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/irlm/networker-tester/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/irlm/networker-tester/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/irlm/networker-tester/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/irlm/networker-tester/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/irlm/networker-tester/releases/tag/v0.1.0
