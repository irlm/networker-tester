# TLS Endpoint Profile — Design, Rationale, Scope, and Value

## Purpose

`networker-tester` already measures network behavior, protocol behavior, page-load behavior, and endpoint responsiveness. What it does **not** yet provide as a first-class capability is a **deep TLS endpoint assessment** that answers a broader operational question:

> For a given host/IP/port, what is this endpoint really presenting on the wire, how trustworthy is it, how capable is it, and are there signs of interception, misconfiguration, or degraded transport/security behavior?

This document proposes a new test family centered on a new result type tentatively named:

- `tls_endpoint_profile`

This is **not** intended to differentiate by inventing novel TLS checks. The useful checks are already well understood across the industry.

The real differentiation for `networker-tester` is elsewhere:
- machine-readable telemetry first, not webpage-first output
- project-compatible persistence and dashboard display
- multi-vantage comparisons across testers/regions/clouds
- repeatable execution inside existing workflows and schedules
- actionable findings that can be tracked over time
- integration with the existing project/workspace/security model

So the goal is to capture the **substance** of a strong TLS endpoint analysis while fitting the architecture and philosophy of `networker-tester`, rather than trying to compete on novel check categories.

This test family should work for both:
- **managed endpoints** — where `networker-tester` has stronger environmental knowledge or operational control
- **arbitrary URLs / external hosts** — where the platform only has client-side visibility

That means the design should support a single conceptual test model with different coverage depth depending on target context, instead of pretending that all targets allow the same level of certainty or control.

---

## Why this test is needed

### 1. Current tests do not give a full endpoint security picture

Existing probes can tell us if a host is reachable, whether HTTP/TLS works, and how fast some transactions are. But operators often need a richer answer:

- Which certificate chain is actually being served?
- Is the chain valid and trusted?
- Does the server support TLS 1.3? TLS 1.2 only? legacy versions?
- Are bad ciphers still enabled?
- Is ALPN behaving correctly?
- Is session resumption working?
- Is the endpoint behind a CDN or reverse proxy?
- Is traffic being intercepted or rewritten by middleware?
- Does behavior differ by vantage point, IP, SNI, or protocol?

These questions matter in production outages, hardening work, audits, and performance investigations.

### 2. TLS problems are often subtle

A simple “TLS succeeded” result hides many real issues:

- hostname mismatch only on IP-based access
- wrong/default certificate when SNI is absent
- broken or incomplete intermediate chain
- revocation metadata missing or unreachable
- 1.3 disabled unexpectedly
- session resumption failing after a deploy
- edge/CDN path differences across regions
- transparent proxy or enterprise TLS interception
- downgraded ALPN causing HTTP/2 loss

A dedicated TLS endpoint profile makes these visible.

### 3. This fits the product direction

Recent work in `networker-tester` has expanded from simple probes into:
- page-load diagnostics
- packet capture summaries
- dashboard observability
- project-scoped operational tooling

A TLS endpoint profile fits naturally as another high-value diagnostic mode with structured JSON output, persistence, and dashboard rendering.

### 4. We want a machine-readable security/transport report, not just a webpage

External tools are useful, but they are not integrated into the system’s data model, DB, permissions, dashboard, or multi-tester execution model.

By implementing this test directly in `networker-tester`, we gain:
- consistent result schema
- repeatable test execution
- automation and regression tracking
- integration with project/workspace history
- support for comparison across time, testers, and regions

---

## Target model and coverage levels

The new test should support both endpoint-style and URL-style execution.

### Target kinds

At minimum, the result should distinguish:
- `managed_endpoint`
- `external_url`
- `external_host` (optional if host-only execution is exposed separately)

### Why this matters

A managed endpoint and an arbitrary public URL are not equivalent test subjects.

For managed endpoints, the platform may later be able to correlate or enrich the result with:
- known deployment metadata
- expected certificate ownership
- expected network path
- repeatable controlled validation
- server-side corroboration in future phases

For arbitrary URLs or external hosts, the system only has client-observed behavior. That still provides strong value, but some checks will be:
- best-effort
- inference-based
- intentionally unavailable

### Coverage level

Each result should declare its effective coverage level, for example:
- `full_control`
- `client_observed`
- `best_effort`

And should expose limitations explicitly, such as:
- `unsupported_checks`
- `limitations`
- `notes`

This prevents false precision. A report for an arbitrary public host should not imply the same certainty as a report for infrastructure under direct operational control.

### Design implication

The right model is **one test family, variable depth**.

Not two different products, and not a fake promise of identical visibility for all targets.

---

## Design principles

### 1. Do not copy external tools directly

We should not mirror another product’s branding, prose, or scoring semantics.

We **should** adopt the useful coverage model:
- target identity
- TLS negotiation
- certificate/chain/trust
- capabilities
- revocation / CT / CAA
- session behavior
- path/interception characteristics
- findings and summary

### 2. Machine-readable first

The result should be designed as a structured JSON object first.

Why:
- persistence is easier
- dashboard rendering becomes a projection of data rather than the source of truth
- test automation becomes possible
- comparisons and regression alerts become possible

### 3. Findings, not walls of prose

The test should emit structured findings like:
- `severity`
- `code`
- `message`
- optionally `evidence`

This makes output easier to filter, display, compare, and alert on.

### 4. Separate facts from judgment

The test should clearly distinguish:
- observed facts (`negotiated_cipher`, `alpn`, `issuer`, `ocsp_urls`)
- derived interpretation (`weak cipher enabled`, `hostname mismatch`, `tls interception suspected`)
- final summary (`ok`, `warn`, `error`, optional score)

### 5. Support real-world indirection

The endpoint path may not be direct, and that is not automatically bad.

The system should distinguish:
- direct path
- indirect but expected path (CDN, reverse proxy, load balancer)
- suspicious path (MITM/interception, wrong cert, strange path mismatch)

---

## Proposed test: `tls_endpoint_profile`

## High-level outcome

Given:
- host
- optional IP override
- port
- optional SNI override

The test should produce a profile describing:
1. what target was intended
2. where the connection actually went
3. what TLS session was negotiated
4. what certificate chain was presented
5. whether the chain and hostname validate
6. what protocol/cipher/ALPN capabilities exist
7. whether session resumption works
8. whether path/interception anomalies are present
9. a list of findings and a summary

---

## What the test should check

## A. Target identity and connection path

### Why
Operators often think they are testing “the server,” but in practice they may be testing:
- a CDN edge
- a reverse proxy
- a corporate TLS inspection box
- a cloud load balancer
- a default virtual host due to missing SNI

### What to capture
- requested host
- requested port
- requested SNI
- optional requested IP override
- DNS-resolved IPs
- actual connected IP
- whether the connected IP matches one of the resolved addresses
- whether proxy use is explicit (env/config)
- whether interception or redirection is suspected

### Value
This tells the operator whether they are truly testing the intended endpoint or a path element in front of it.

---

## B. Basic handshake characteristics

### Why
Even before deep analysis, the handshake itself provides high-value facts:
- TCP connect time
- TLS handshake time
- negotiated TLS version
- negotiated cipher suite
- negotiated ALPN
- key exchange group

### What to capture
- `tcp_connect_ms`
- `tls_handshake_ms`
- `negotiated_tls_version`
- `negotiated_cipher_suite`
- `negotiated_key_exchange_group`
- `alpn`

### Value
This provides a transport baseline and helps explain performance and compatibility issues.

---

## C. Certificate and chain details

### Why
Certificates are central to TLS identity. Operators need more than a yes/no answer.

### What to capture
For the leaf:
- subject
- issuer
- serial number
- validity window
- SAN DNS names
- SAN IP addresses
- key type and size
- signature algorithm
- SHA-256 fingerprint
- SPKI SHA-256 pin
- OCSP Must-Staple presence
- SCT/CT evidence presence

For the chain:
- all presented intermediates
- issuer/subject relationships
- CA flag
- key type/size
- fingerprints
- AIA / CRL / OCSP metadata

### Value
This enables:
- manual operator inspection
- automation
- historical change detection
- trust debugging
- pinning- or identity-related workflows

---

## D. Trust and validation

### Why
A server can present a syntactically valid certificate but still fail in practice.

### What to check
- hostname matches requested host/SNI
- chain validates successfully
- chain trusted by local/system trust store
- chain completeness
- alternate path weirdness where visible
- revocation metadata present
- OCSP staple present or absent
- revocation status if practical to evaluate

### Value
This answers the real operator question:

> Would a normal client trust this endpoint, and if not, why not?

---

## E. TLS capability matrix

### Why
A single successful handshake does not describe the endpoint’s compatibility surface.

### What to check
Per protocol version where practical:
- TLS 1.0 support
- TLS 1.1 support
- TLS 1.2 support
- TLS 1.3 support

Per supported version:
- accepted cipher suites
- supported groups/curves where observable
- ALPN results

Additional behavior:
- SNI required / optional
- default cert when no SNI is sent
- client certificate requested or required

### Value
This is the part that makes the report feel comprehensive and helps with:
- hardening
- compatibility analysis
- regression detection
- documenting support posture

---

## F. Session resumption and 0-RTT

### Why
TLS session behavior matters for latency, scale, and user experience.

If resumption is broken, users may pay repeated full handshake cost. If 0-RTT is enabled incorrectly, operators may want to know.

### What to check
- whether resumption is supported
- whether the second handshake resumes
- timing difference between cold and resumed handshake
- probable method (`ticket`, `session_id`, `unknown`)
- whether early data is offered/accepted where supported

### Value
This adds a performance and operational layer to the test that basic certificate tooling usually doesn’t emphasize as well for repeated monitoring.

---

## G. Revocation, CT, and CAA

### Why
These are supporting trust signals that matter for real deployments and audits.

### What to check
- OCSP URLs present
- CRL URLs present
- OCSP stapling seen
- Must-Staple flag
- SCTs/CT evidence present
- DNS CAA present and records collected

### Value
This enriches the report for:
- security posture
- PKI hygiene
- compliance-style review
- troubleshooting certificate issuance and revocation behavior

---

## H. Path and interception analysis

### Why
A TLS endpoint can be “valid” but still not be a direct connection to what the operator thinks they are hitting.

### What to check
- connected IP vs resolved IPs
- default cert behavior with missing SNI
- cert issuer or identity unusual for target context
- ALPN downgrade clues
- explicit proxy detected via env/config
- signs of enterprise TLS interception
- expected indirect path vs suspicious path

### Suggested classifications
- `direct`
- `indirect_expected`
- `indirect_suspicious`

### Value
This makes the test much more operationally useful in enterprise and cloud environments.

---

## I. Findings and summary

### Why
Raw facts are useful, but operators need help prioritizing.

### What to emit
A list of findings such as:
- `INFO`: CT evidence present
- `WARNING`: no OCSP stapling observed
- `WARNING`: no TLS 1.3 support
- `ERROR`: hostname mismatch
- `ERROR`: certificate expired
- `WARNING`: SNI required for correct certificate
- `WARNING`: path appears indirect/suspicious

Summary fields:
- overall status (`ok`, `warn`, `error`)
- optional score
- headline summary text

### Value
This makes the result useful for dashboards and fast triage.

---

## Proposed result model

At a high level:

- `target`
- `path_characteristics`
- `connectivity`
- `certificate`
- `trust`
- `capabilities`
- `resumption`
- `findings`
- `summary`

This should be kept as a stable JSON contract suitable for storage and UI rendering.

Notes on lifecycle:
- `capabilities` is expected to be `null` in the Phase 1 foundation implementation and populated once the active-probing capability matrix is implemented in Phase 3.
- `summary.score` should remain `null` until a documented scoring rubric exists; the presence of `status` does not require scoring.

A representative shape:

```json
{
  "target_kind": "external_url",
  "coverage_level": "client_observed",
  "unsupported_checks": [
    "server_side_policy_validation"
  ],
  "limitations": [
    "Result based only on client-visible handshake and DNS behavior"
  ],
  "target": {
    "host": "microsoft.com",
    "port": 443,
    "requested_ip": "150.171.109.147",
    "sni": "microsoft.com",
    "resolved_ips": ["150.171.109.147", "150.171.110.147"]
  },
  "path_characteristics": {
    "connected_ip": "150.171.109.147",
    "direct_ip_match": true,
    "proxy_detected": false,
    "classification": "direct",
    "evidence": []
  },
  "connectivity": {
    "tcp_connect_ms": 12.4,
    "tls_handshake_ms": 28.7,
    "negotiated_tls_version": "tls1.3",
    "negotiated_cipher_suite": "TLS_AES_256_GCM_SHA384",
    "negotiated_key_exchange_group": "x25519",
    "alpn": "h2"
  },
  "certificate": {
    "leaf": {
      "subject": "CN=microsoft.com",
      "issuer": "CN=Microsoft TLS G2 RSA CA OCSP 02",
      "serial_number": "41000b07862b61ee4229ded1760000000b0786",
      "not_before": "2026-03-10T18:31:55Z",
      "not_after": "2026-09-06T18:31:55Z",
      "san_dns": ["microsoft.com"],
      "san_ip": [],
      "key_type": "RSA",
      "key_bits": 2048,
      "signature_algorithm": "sha384WithRSAEncryption",
      "must_staple": false,
      "scts_present": true,
      "sha256_fingerprint": "...",
      "spki_sha256": "..."
    },
    "chain": [
      {
        "subject": "CN=Microsoft TLS G2 RSA CA OCSP 02",
        "issuer": "CN=Microsoft TLS RSA Root G2",
        "key_type": "RSA",
        "key_bits": 4096,
        "signature_algorithm": "sha384WithRSAEncryption"
      }
    ]
  },
  "trust": {
    "hostname_matches": true,
    "chain_valid": true,
    "trusted_by_system_store": true,
    "revocation": {
      "ocsp_stapled": false,
      "method": "best_effort",
      "status": "unknown",
      "notes": ["OCSP responder not queried in passive mode"]
    },
    "caa": {
      "present": true,
      "records": ["issue digicert.com"]
    }
  },
  "capabilities": null,
  "resumption": {
    "supported": true,
    "method": "ticket",
    "initial_handshake_ms": 28.7,
    "resumed_handshake_ms": 9.8,
    "early_data_offered": false,
    "early_data_accepted": null
  },
  "findings": [
    {
      "severity": "info",
      "code": "CT_PRESENT",
      "message": "Certificate transparency evidence present"
    }
  ],
  "summary": {
    "status": "ok",
    "score": null
  }
}
```

---

## What this test is **not** trying to do

To avoid scope creep, this test should not initially attempt to be:
- a vulnerability scanner for every historical TLS flaw
- a generic web application scanner
- a complete browser rendering test
- a substitute for packet capture
- a substitute for page-load diagnostics

It may later grow some optional checks for legacy weakness patterns, but the first implementation should stay focused on **endpoint identity, TLS capabilities, trust, and path behavior**.

---

## Relationship to existing tests

Existing tests already provide:
- connectivity and timing basics
- HTTP request behavior
- page-load performance
- packet summaries
- endpoint reachability

The TLS endpoint profile adds:
- deep endpoint identity and trust analysis
- protocol/cipher support surface
- certificate chain inspection
- resumption behavior
- indirect path/interception clues

So it is complementary, not redundant.

It also clarifies a product boundary:
- for **external URLs/hosts**, the result is primarily a client-observed TLS/security profile
- for **managed endpoints**, the same model can later be enriched with stronger contextual confidence and deeper validation

---

## Recommended implementation phases

## Phase 1 — Foundation

Deliver a useful first version with:
- target resolution
- one TLS handshake
- negotiated version/cipher/ALPN
- certificate chain parsing
- hostname validation
- basic trust result
- findings list
- summary
- explicit `target_kind`, `coverage_level`, and limitations

### Why Phase 1 matters
This already gives a lot of operator value while keeping implementation manageable.

---

## Phase 2 — Trust enrichment

Add:
- revocation metadata extraction
- CT/SCT evidence detection
- CAA lookup
- richer chain diagnostics

### Important note on revocation
Revocation should be treated as **best-effort**, not absolute truth.

In practice:
- OCSP responders may be slow or unreachable from some vantage points
- CRL downloads may be large or blocked
- seeing a stapled OCSP response is different from independently validating revocation status
- some environments intentionally avoid live revocation lookups for performance or policy reasons

So the test should report:
- what revocation metadata was present
- whether stapling was observed
- whether an online lookup was attempted
- what result was obtained
- when status is `unknown` or `best_effort`

It should avoid overstating certainty when the network conditions do not support a definitive result.

### Gain
The report becomes much more security- and PKI-relevant.

---

## Phase 3 — Capability matrix

Add:
- TLS 1.0/1.1/1.2/1.3 support probing
- accepted cipher coverage per version
- SNI behavior
- ALPN behavior
- client-auth behavior

### Important note on implementation complexity
This phase is a material jump in complexity compared with Phase 1–2.

Unlike passive observation of one negotiated session, this phase requires **active probing** with multiple independently crafted handshakes, including variations in:
- protocol version bounds
- cipher suite offerings
- ALPN lists
- SNI presence/absence
- possibly supported groups / key share behavior

That means this phase depends heavily on the capabilities of the TLS library or external tooling chosen for implementation.

Examples of questions that affect implementation:
- Can the chosen Rust TLS stack deliberately advertise legacy protocol ranges?
- Can it constrain cipher offerings precisely enough for matrix probing?
- Can it omit SNI cleanly?
- Can it surface enough handshake metadata to distinguish rejection from negotiation fallback?

So Phase 3 should be treated as an explicit active-probing subsystem, not just an extension of the Phase 1 handshake parser.

### Gain
This is where the report becomes truly comprehensive for compatibility and hardening analysis.

---

## Phase 4 — Session behavior

Add:
- session resumption support
- resumed handshake timing
- probable resumption mechanism
- 0-RTT support/results if exposed

### Important note on 0-RTT
0-RTT should not be presented as a simple performance feature with no caveats.

If surfaced, the report should acknowledge that 0-RTT has replay-related tradeoffs and that availability alone is not automatically a positive signal. The test should describe:
- whether early data appears to be supported
- whether it was accepted
- that this is an operational/security characteristic, not just a speed optimization

### Gain
Brings strong performance and operational insight.

---

## Phase 5 — Comparison and product polish

Add:
- persistence and DB schema
- dashboard rendering
- history view
- compare results across testers, projects, or time
- optional scoring refinement

### Gain
Turns the feature from a one-off diagnostic into a durable product capability.

---

## Key gains from adding this test

## 1. Better troubleshooting

Operators can diagnose:
- wrong certificate served
- bad chain
- trust failures
- missing SNI behavior
- ALPN regressions
- resumption failures
- indirect or intercepted connections

## 2. Better hardening visibility

It becomes easy to see:
- legacy protocol exposure
- weak cipher exposure
- missing revocation metadata
- bad PKI hygiene
- inconsistent trust posture

## 3. Better performance insight

By combining TLS facts with timing and resumption behavior, the platform can explain:
- why a secure endpoint feels slow
- why repeated connections aren’t improving
- whether edge/network path behavior is affecting negotiation

## 4. Better regression detection

Because the output is structured and can be persisted, the system can later detect changes like:
- certificate rotation
- sudden loss of TLS 1.3
- ALPN downgrade
- missing intermediate after deployment
- resumption no longer working

## 5. Better multi-vantage analysis

This is especially valuable in a distributed tester model.

The same endpoint can be profiled from:
- different clouds
- different regions
- different organizations/networks
- different projects/workspaces

This can reveal:
- CDN variation
- geo-specific configuration drift
- proxy/interception differences
- split-horizon DNS behavior

## 6. Better dashboard value

This test would produce rich, visualizable results that are more explanatory than simple pass/fail probes.

---

## Important edge cases to support

The implementation should explicitly consider:
- hostname target with DNS resolution
- hostname target with explicit IP override
- direct IP target with SNI override
- no-SNI behavior
- self-signed endpoints
- expired certs
- hostname mismatch
- missing intermediate
- many-SAN certs
- RSA and ECDSA leafs
- CDN-backed targets
- reverse-proxied targets
- enterprise MITM/interception environments
- endpoints that request or require mutual TLS / client certificates

---

## Bottom line

Adding a TLS endpoint profile gives `networker-tester` a new kind of diagnostic depth.

It moves the platform beyond:
- “did the request work?”
- “how fast was it?”

into:
- “what endpoint did we really reach?”
- “what identity and trust material did it present?”
- “what TLS capabilities does it expose?”
- “is the path direct, indirect, or suspicious?”
- “is the endpoint configured well enough for production confidence?”

That is a meaningful product gain for troubleshooting, hardening, auditing, and continuous monitoring.

---

## Proposed next step

Implement the Phase 1 contract first:
- target
- path characteristics
- connectivity
- certificate
- trust
- basic resumption
- findings
- summary

Then expand to capability matrix and dashboard rendering in later steps.
