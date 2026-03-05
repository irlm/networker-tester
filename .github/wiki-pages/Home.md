# Networker Tester

A cross-platform network diagnostics suite that probes an endpoint across TCP, HTTP/1.1, HTTP/2, HTTP/3, and UDP — collecting per-phase telemetry that is normally buried in the kernel or scattered across multiple tools.

```
networker-tester  ──────────────────────►  networker-endpoint
(Rust CLI)           TCP / HTTP1 / HTTP2   (Rust server)
                      HTTP3 (QUIC)
                      UDP echo
        │
        ▼
JSON artifact  ·  HTML report  ·  Excel workbook
```

---

## What It Measures

| Metric | Why it matters |
|--------|---------------|
| **TTFB** | Head-of-line blocking sensitivity; H/2 and H/3 should win on high-latency links |
| **Throughput / Goodput** | Raw transfer rate vs. true end-to-end rate (including connection setup) |
| **CPU** | QUIC does TLS in userspace — measurably higher than H/1.1 or H/2 |
| **Context switches** | Voluntary (I/O yield) and involuntary (preemption); reflects concurrency differences |
| **Page-load total** | Wall-clock impact of multiplexing; H/2 and H/3 win on multi-asset pages |
| **TCP kernel stats** | RTT, cwnd, ssthresh, retransmits, delivery rate — via `getsockopt`, no root needed |
| **TLS details** | Version, cipher suite, ALPN, full certificate chain |
| **UDP RTT / jitter / loss** | Raw UDP path quality |

---

## Quick Install

### macOS and Linux

```bash
# Install the diagnostic CLI
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- tester

# Install the test server (run on the machine you want to probe)
curl -fsSL https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.sh | bash -s -- endpoint
```

### Windows (PowerShell)

```powershell
Invoke-RestMethod 'https://gist.githubusercontent.com/irlm/37a1af64b70ef6e58ea117839407f4f9/raw/install.ps1' | Invoke-Expression
```

---

## Quick Start (60 seconds)

```bash
# 1. Start the endpoint
networker-endpoint

# 2. Run your first probe
networker-tester \
  --target https://127.0.0.1:8443/health \
  --modes http1,http2,http3 \
  --runs 5 \
  --insecure \
  --output-dir ./output
```

Open `output/report.html` in any browser.

---

## Wiki Pages

| Page | Contents |
|------|---------|
| [[Installation]] | curl one-liners, local install options, cloud deploy |
| [[Usage]] | All flags, all modes, output formats, config file |
| [[Protocol-Comparison]] | H1 vs H2 vs H3 — what to run and how to read results |
| [[Cloud-Deployment]] | Azure and AWS deployment, auto-shutdown, multi-region |

---

## Two Binaries

| Binary | Role |
|--------|------|
| `networker-tester` | CLI client — sends probes, collects metrics, writes reports |
| `networker-endpoint` | Diagnostic server — serves `/health`, `/download`, `/upload`, `/page`, `/browser-page` |

Both are written in Rust and ship as single static binaries for macOS (Apple Silicon + Intel), Linux (x86-64 + ARM64), and Windows (x86-64).
