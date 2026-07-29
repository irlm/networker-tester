# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Network/IT engineers and DevOps/SRE teams. High technical level — comfortable
with CLIs, network protocols, and raw data. They arrive in two situations:
under time pressure (diagnosing a network/performance issue during an incident,
validating a deployment) or in deliberate evaluation mode (comparing cloud
providers, proxies, or language runtimes before committing infrastructure).
Their job: get clear, trustworthy network diagnostics quickly — live in the
dashboard, and as shareable artifacts (HTML/PDF/Markdown/DOCX/Excel reports).

## Product Purpose

LagHound measures network and application performance the way an engineer would
want to defend it: per-phase probe timing (DNS / TCP / TLS / HTTP) across
HTTP/1.1, HTTP/2, HTTP/3, UDP and QUIC, run from real infrastructure the user
provisions in their own cloud account, with statistical methodology (warmup vs
measured runs, quality gates, publication blockers) that refuses to publish
numbers it cannot stand behind. Success = an engineer trusts a LagHound number
enough to make an infrastructure decision on it, and can hand the report to a
colleague who trusts it too.

## Positioning

**The combination is the moat — no single neighbor can truthfully copy all
three at once:**

1. **Per-phase probe depth** — attempt-level DNS/TCP/TLS/HTTP phase timing
   across HTTP/1.1-2-3, UDP, QUIC (plus RPM, STAMP, path, dual-stack,
   WebSocket, PMTU modes), not endpoint-level ping averages.
2. **Bring-your-own-cloud benchmarking** — ephemeral runners and provisioned
   targets live in the user's own Azure/AWS/GCP account; real machines, real
   networks, perf-per-cost accounting, no shared synthetic probe fleet.
3. **Defensible statistics** — warmup/measured separation, outlier policy,
   quality gates (CV%, noise, sample floor), and publication blockers that
   suppress numbers the methodology can't support.

Grafana/Datadog observe what you already run; k6 load-tests your app; Pingdom
pings from its fleet. None combine probe-phase depth, self-provisioned real
infrastructure, and gated statistics.

## Operating Context

- **Ambition: open-source project.** A public tool others self-host; adoption
  and community are the success currency, not revenue. laghound.com is the
  reference/flagship deployment.
- Two entry points: a `curl | bash` (or PowerShell) installer that provisions
  everything from a terminal, and the web dashboard (React, served by the
  control plane) for ongoing operation.
- Core workflows: URL Probe (watchlist diagnostics), Network Test, Full Stack
  Benchmark (proxy stacks on provisioned VMs), Application Benchmark
  (language/framework comparison) — four test types over one backend.
- Runs execute on **runners** (agent VMs in the user's cloud) against
  **targets** (provisioned endpoint VMs or arbitrary hosts); the control plane
  orchestrates provisioning, dispatch, and teardown, including cost hygiene
  (auto-shutdown, orphan reaping, full delete cascades).
- Artifacts matter: reports are shared with colleagues who never open the
  dashboard; the report is the product for that second audience.
- Nightly self-verification exists (soak check + run-execution canary) — the
  product measures things, so its own numbers being wrong is the worst failure
  class.

## Capabilities and Constraints

- Hybrid stack: Rust probe engine (`crates/networker-tester` — permanent
  measurement core), C# .NET control plane + agent (application layer), React +
  Tailwind dashboard. Versioned JSON contract between engine and platform.
- Terminology: **runner** (agent VM that executes), **target** (what it probes),
  **probe / test / benchmark** (increasing methodology), **modes** (protocol
  variants; `shared/modes.json` is the manifest of record).
- Provisioned targets serve self-signed certificates by construction; the
  platform accounts for this (insecure-on-promote) — never present it as a
  defect.
- Cross-cutting invariant: every mode/capability change must stay in sync
  across engine, contract, agent, dashboard, and manifest (CI drift guards).
- Windows and Linux both first-class for runners/targets; installer constrained
  to Bash 3.2 compatibility.
- Open product decisions: none recorded beyond the above; monetization
  explicitly out of scope (open-source ambition).

## Brand Commitments

- **Name: LagHound** (product + domain laghound.com). Repo name
  `networker-tester` is historical; user-facing brand is LagHound.
- **Logo:** terminal-style glyph — prompt `>` in purple + latency waveform in
  cyan + cursor block on a dark tile (`assets/brand/laghound-glyph.svg`,
  `laghound-wordmark.svg`; embedded in the control plane for reports).
- **Binding visual constraints** (volunteered, recorded verbatim — see
  `.impeccable.md` for the full statement): terminal/hacker aesthetic;
  monospace-first typography (Cascadia Code / JetBrains Mono / Consolas); dark
  theme primary (`#0a0b0f` base) with light mode for print reports; brand
  purple `#863bff`, cyan `#47bfff` primary accent; data density over
  decoration; zero chrome (no gradients/shadows; flat surfaces, thin borders);
  references Grafana/Datadog/Warp; anti-reference: generic gradient-hero SaaS.
- **Voice:** technical, precise, reliable — a tool built by engineers for
  engineers; data is the hero.

## Evidence on Hand

- **Real measurement data** from the flagship deployment: genuine runs,
  attempt-level phase data, and generated reports (HTML/PDF/MD/DOCX/Excel) from
  the maintainer's own infrastructure. Usable in screenshots, samples, and
  documentation.
- **No external users yet.** Future work must not fabricate testimonials,
  customer names, adoption numbers, or third-party benchmarks. Comparative
  performance claims must come from LagHound's own published methodology.

## Product Principles

1. **Never ship a number you can't defend** — measurement correctness and
   statistical honesty outrank features; gates that suppress bad numbers are a
   feature, not friction.
2. **The user's infrastructure, the user's data** — everything runs in the
   user's own cloud and self-hosted deployment; no dependency on a vendor probe
   fleet or hosted service.
3. **The report is a first-class user** — every result must survive being
   exported and read by someone who never saw the dashboard.
4. **Leave no orphans** — anything the product provisions, it must account for
   and tear down; cost hygiene is part of trust.
5. **Engineer-to-engineer clarity** — precise labels, semantically consistent
   status colors, raw data reachable from every summary.
