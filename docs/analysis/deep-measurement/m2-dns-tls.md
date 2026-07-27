# Deep Measurement Audit — M2: DNS Resolution + TLS/PKI

Date: 2026-07-27 · Auditor scope: `runner/dns.rs`, `runner/tls.rs`, the TLS
surface of `runner/http3.rs`/`runner/native.rs`, and the `DnsResult` /
`TlsResult` / `CertEntry` contracts in `metrics.rs`. Successor to the DNS/TLS
rows (#6, #7) of `docs/analysis/measurement-gap-analysis-2026-07.md` — that
audit's asks are now largely shipped; this one goes a level deeper against
professional practice (RIPE Atlas, SSLLabs/testssl.sh-class observation,
current DoH/DoQ/SVCB/PQC research).

Scoring rubric (same as the 2026-07 audit): **value /40 · measurement-trust
/20 · effort-inverse /20 · product-fit /20** → 0–100. Sub-scores shown so the
totals are auditable. Library capability claims below were verified against
the vendored sources actually in this build (`hickory-resolver 0.26.1`,
`hickory-proto 0.26.1`, `rustls 0.23.42`, `x509-parser 0.18.1`), not from
memory.

---

## (a) Current state

### DNS (`crates/networker-tester/src/runner/dns.rs`, 651 lines)

| Capability | Where |
|---|---|
| Process-wide hickory resolver from **system config** (resolv.conf / SystemConfiguration / registry), Google-fallback loudly labeled | `dns.rs:30-87` (`shared_resolver`) |
| Resolver identity recorded per result (`"system (192.168.1.1:53)"`) | `dns.rs:163`, `metrics.rs:1550-1555` |
| `Ipv4thenIpv6` strategy pinned so downstream connect-address ordering is stable | `dns.rs:74` |
| Family pinning queries A or AAAA **directly** (not resolve-then-filter) | `dns.rs:118-133` |
| IP-literal fast path incl. honest error on family-pin mismatch | `dns.rs:114-117`, tests `dns.rs:503-518` |
| `dns` probe mode: separately timed A and AAAA lookups (`a_ms`/`aaaa_ms`), record counts, **CNAME chain** (loop-bounded, case/root-dot normalized) | `dns.rs:195-273`, `280-369`; `metrics.rs:1556-1576` |
| Resolver construction and config reads kept **outside** the timing window | `dns.rs:102-109` |
| NXDOMAIN/NODATA duration still captured as a valid latency | `dns.rs:194-236` |

### TLS (`crates/networker-tester/src/runner/tls.rs`, 1549 lines)

| Capability | Where |
|---|---|
| Standalone `tls` probe: DNS → TCP (with full `SocketInfo` kernel stats) → handshake, timer scoped to exactly the handshake | `tls.rs:22-279` |
| Version, cipher suite, ALPN (offers `h2`+`http/1.1`) | `tls.rs:897-958` |
| **Full cert chain**: subject/issuer/expiry/SANs + key algorithm & size (RSA/EC-curve/Ed25519 OID mapping) + signature algorithm | `tls.rs:974-1084`, `metrics.rs:1753-1773` |
| Trust store built once per process (webpki + native certs), `--ca-bundle`, `--insecure` NoVerifier | `tls.rs:732-781`, `1090-1128` |
| **OCSP staple observation** via pass-through `ServerCertVerifier` wrapper (presence + byte length; `None` when verification didn't run) | `tls.rs:791-870` |
| `tlsresume` mode: cold-vs-warm two-connection probe, real HTTP/1.1 request to flush TLS 1.3 tickets, handshake-kind classification (full/full-hrr/resumed), ticket counts | `tls.rs:287-489`, `694-700` |
| `native` mode: same probe over OS TLS (Schannel / SecureTransport / OpenSSL), backend labeled | `native.rs:27-35`, `752-787` |
| HTTP/3: QUIC session resumption + **0-RTT attempted/accepted** + resumed-handshake timing via `Connecting::into_0rtt()` | `http3.rs:243-343`, `metrics.rs:1827-1849` |

### Contract (`metrics.rs`)

`DnsResult` (`metrics.rs:1544-1577`) and `TlsResult` (`metrics.rs:1776-1850`)
are additive/serde-defaulted throughout — every gap below can ship without a
contract break, consistent with the versioned-JSON policy.

**Overall verdict:** for a client-path probe this TLS surface is already above
open-source-tool median (staple observation, resumption depth, 0-RTT, chain
key/sig detail). The DNS surface is mid-pack: good timing hygiene, but it
records less per query than RIPE Atlas has captured per query since ~2011, and
two genuine trust defects exist (D1, D2 below).

---

## (b) Gaps against professional practice

Ordered by score. "Trust" flags mean the current number can mislead, not just
that a feature is missing.

### D1 — In-process resolver cache silently converts repeat DNS timings into hashmap lookups · **88** (V32 T20 E18 F18)

**What.** hickory-resolver caches positively- and negatively-resolved answers
in an in-process LRU; the default is `cache_size: 8192`
(`hickory-resolver-0.26.1/src/config.rs:519,674`). `shared_resolver()` applies
the system-config opts and overrides only `ip_strategy` (`dns.rs:67-79`) — the
cache stays on. Consequence: **attempt 1 of a run measures the real resolver
path; attempts 2..N within TTL measure an in-memory cache hit** (tens of µs).
This poisons `dns_ms` distributions in *every* mode that resolves per attempt
— the `dns` probe itself plus tls/tlsresume (`tls.rs:57,328`), http
(`http.rs:244`), pageload (`pageload.rs:234,822,1879`), websocket
(`websocket.rs:110`), native (`native.rs:129`). A run's "min/p50 dns_ms ≈ 0.0"
is an artifact, and the recorded resolver label ("system (…:53)") claims a
network path that was never touched after the first attempt.

**Why (professional practice).** RIPE Atlas sends a real query for every
measurement and records the per-response `response_time`; cache-hit
contamination is treated as an invalidating methodology error in every DNS
performance study (the DoH/DoT literature explicitly separates cold path from
amortized/cached path rather than mixing them). A probe engine whose sales
pitch is per-phase trust cannot mix the two silently — this is the same class
of defect as trust-audit V1 (hardcoded 8.8.8.8) and V5 (trust-store load
inside the handshake timer).

**Implementation path.**
1. For the standalone `dns` probe: build its resolver with
   `opts.cache_size = 0` → every attempt is a real query (matches Atlas
   semantics). One-line opts change plus a second `OnceLock` (or a
   `cache_size`-parameterized constructor).
2. For connection-modes (http/tls/…): caching arguably *mimics* what an OS
   with a system cache does, so keep it — but label it: add additive
   `DnsResult.served_from_cache: Option<bool>`. hickory does not expose a
   cache-hit flag on `Lookup`; the honest cheap proxy is a threshold on
   duration is *not* acceptable — instead query via a per-run flag: first
   attempt `false`, subsequent attempts within `valid_until()`
   (`hickory-resolver/src/lookup.rs:103`) `true`. Or simplest defensible
   option: resolve once per run, reuse the `SocketAddr` for attempts 2..N, and
   set `dns: None` on those attempts (no fake number at all).
3. Document the semantics in `docs/` per the CLAUDE.md rule about verifying
   documented behavior by running the binary.

**Platform notes.** None — pure library configuration.
**Cites.** RIPE Atlas DNS measurement docs (per-response `response_time`,
every query real); RFC 8767 (serve-stale — why cache-hit latency is not
resolver latency).

### D2 — HTTP/3 resolves via `getaddrinfo`, HTTP/1.1/2 via hickory: the h1/h2/h3 DNS comparison is apples-to-oranges · **71** (V18 T18 E19 F16)

**What.** `http3.rs:216` uses `tokio::net::lookup_host` (OS `getaddrinfo`,
label "system (OS getaddrinfo)" at `http3.rs:232`), while http/tls/pageload
use `dns_runner::resolve` (hickory, direct-to-nameserver). The two paths have
different caches (OS system cache vs hickory LRU), different transports and
different search-list behavior, so in the head-to-head h1/h2/h3 story — a
headline feature — the `dns_ms` column is measured by two different
instruments. `getaddrinfo` also cannot honor `ipv4_only`/`ipv6_only` the way
`resolve()` does.

**Why.** Cross-protocol comparability is the product's core trust claim;
measurement methodology must vary in exactly one dimension (the protocol under
test).

**Implementation path.** Replace `resolve_addr`'s `lookup_host` with
`dns_runner::resolve(&host, ipv4_only, ipv6_only)` (the RunConfig flags are
already threaded into the h3 path for other purposes). ~20-line change +
integration-test assertion that h3 attempts carry the hickory resolver label.
**Platform notes.** None; hickory is already a mandatory dep (h3 is the
feature-gated one).

### D3 — No SVCB/HTTPS (type 65) record capture — the record that now gates real connection setup · **75** (V30 T12 E16 F17)

**What.** No probe queries HTTPS RRs (RFC 9460). Since iOS 14/macOS 11, Apple
platforms query type 65 for *every* http(s) connection; Chrome and Firefox use
it for h3 discovery (`alpn=h2,h3`), ECH configs, and address hints. A target
whose HTTPS RR is missing/wrong will behave differently in Safari than in our
probe, and our `http3` mode discovers h3 support only by trying it — never
checking whether the server *advertises* it.

**Why.** SVCB/HTTPS is the single biggest change to connection establishment
since happy eyeballs; a DNS+TLS measurement tool that ignores type 65 in 2026
is measuring yesterday's setup path. It is also the delivery channel for ECH
(RFC 9849 §"ech" SvcParam) — D8 depends on it.

**Implementation path.** hickory-proto 0.26 ships full rdata support:
`rr/rdata/svcb.rs` + `rr/rdata/https.rs` (verified in-tree), including typed
`SvcParamKey`s (alpn, port, ipv4hint/ipv6hint, ech). In `resolve_detailed`,
add a third timed typed lookup for `RecordType::HTTPS`; extend `DnsResult`
additively: `https_record_present: Option<bool>`, `https_alpn: Vec<String>`,
`https_ech_present: Option<bool>`, `https_ipv4_hints`/`https_ipv6_hints`
counts, `https_ms: Option<f64>`. Cross-check in `http3` summary: "server
advertises h3 in DNS: yes/no" vs measured h3 reachability — that mismatch
(advertised-but-blocked, or working-but-unadvertised) is a genuinely useful
diagnostic no popular CLI tool surfaces today.
**Platform notes.** None.
**Cites.** RFC 9460; RFC 9849 (ECH; config via HTTPS RR); PowerDNS
svcb-implementations list (client adoption).

### D4 — TTLs are parsed and thrown away · **74** (V26 T12 E19 F17)

**What.** `timed_typed_lookup` iterates `lookup.answers()` (`dns.rs:204-219`)
where every `Record` carries its TTL, and discards it. No TTL appears in
`DnsResult`.

**Why.** TTL is a first-class diagnostic: sub-60 s TTLs explain re-resolution
churn and load-balancer flap; TTL=0 explains cache-less latency; the
answer-section TTL also bounds how long the run's resolved IP stays valid
(relevant to long throughput runs). Atlas stores full answer TTLs; `dig`
prints them on every line; engineers will expect them.

**Implementation path.** Additive `DnsResult` fields: `a_min_ttl_s`,
`aaaa_min_ttl_s: Option<u32>` (min across the answer RRset; the CNAME chain
entries can carry their own TTLs later if wanted). On the negative side,
capture negative-caching TTL from the SOA in the authority section (RFC 2308)
— that requires raw-message access, so it lands with D5, not here. ~30 lines +
tests.
**Platform notes.** None. **Cites.** RFC 1035 §3.2.1; RFC 2308.

### D5 — No response-level metadata: rcode, flags, EDNS, response size, NSID, TCP fallback (RIPE-Atlas-class capture) · **70** (V28 T14 E12 F16)

**What.** A RIPE Atlas DNS result records per response: response time,
response size, rcode, header flags (QR/AA/TC/RD/RA/**AD**/CD), section counts,
EDNS0 UDP size, optional NSID, and whether TCP was used — plus the full abuf.
We record: duration, IPs, counts, CNAMEs. We cannot answer: Was the answer
authoritative? Truncated (→ did hickory silently retry over TCP)? Did a
validating resolver assert AD? Which anycast instance answered (NSID, RFC
5001)? What was the wire size?

**Why.** These are the fields that turn "DNS was slow" into "DNS was slow
*because* the UDP answer truncated at 1232 B and fell back to TCP" or "your
resolver isn't validating". For a fleet product, NSID additionally
disambiguates anycast instances across testers — exactly what Atlas/DNSMON
use it for.

**Implementation path.** hickory-resolver's `Lookup` deliberately does not
expose the raw `Message` (verified: `lookup.rs` exposes records/query/
valid_until only; rcode surfaces only inside `NoRecordsFound` errors). The
path is a **raw-query lane for the `dns` probe mode only**: use
hickory-proto's `DnsHandle`/`UdpClientStream` (or hickory-client) against the
same nameservers `read_system_conf()` returned, with an EDNS0 OPT carrying
NSID. Capture `Header` flags, rcode, message byte length, TC + explicit TCP
retry timing (measure UDP leg and TCP leg separately instead of letting the
library hide the fallback — Atlas reports both). Keep `resolve()` untouched
for connection modes. New additive fields: `rcode`, `flags: Vec<String>`,
`response_bytes`, `edns_udp_size`, `nsid`, `tcp_fallback: Option<bool>`,
`tcp_ms: Option<f64>`. Moderate effort (~250-400 lines incl. tests); no new
crate needed beyond what hickory already pulls in (hickory-proto is a
transitive dep).
**Platform notes.** None.
**Cites.** RIPE Atlas measurement/result docs + Sagan `DnsResult` attributes;
RFC 6891 (EDNS0); RFC 5001 (NSID); RFC 7766 (DNS over TCP).

### D6 — Negotiated key-exchange group is one call away and not recorded · **74** (V26 T10 E20 F18)

**What.** `TlsResult` has version/cipher/ALPN but not the negotiated group.
rustls 0.23.42 exposes it directly:
`CommonState::negotiated_key_exchange_group()`
(`rustls-0.23.42/src/common_state.rs:168`) — verified present. With the ring
provider this reports X25519 / secp256r1 / secp384r1.

**Why.** SSLLabs-class reporting has shown key exchange for a decade; in 2026
it is *the* interesting handshake fact because it is where hybrid PQC shows up
(D7). Even ring-only, "your API terminator negotiates P-384 (two orders of
magnitude slower than X25519 in some stacks)" is actionable. Also the honest
prerequisite for any PQC story: report what we negotiate today.

**Implementation path.** In `extract_tls_probe_info` (`tls.rs:897`):
`conn.negotiated_key_exchange_group().map(|g| format!("{:?}", g.name()))` →
additive `TlsResult.kx_group: Option<String>`. Mirror in the native probe
where obtainable (Schannel/SecureTransport largely don't expose it — leave
`None`, the field is already optional). ~15 lines + test.
**Platform notes.** rustls path all platforms; `native` mode mostly `None`.
**Cites.** RFC 8446 §4.2.8 (key_share); SSLLabs methodology docs.

### D7 — Post-quantum readiness probing (X25519MLKEM768) · **64** (V32 T8 E12 F12)

**What.** No way to answer "does this endpoint negotiate hybrid PQ key
exchange, and what does it cost?". 2026 context: ~49% of scanned domains
support hybrid KX; Akamai made PQ default client-side Jan-2026; Chrome sends
X25519MLKEM768 key shares by default; Cloudflare Radar shows >30-57% of
handshakes carrying hybrid shares (measurement varies by vantage). The
performance angle is real and on-brand: the ML-KEM key share grows the
ClientHello past ~1.5 KB (often 2 TCP segments / possible extra RTT on some
middleboxes), which is precisely a lag-measurement story.

**Why value but capped fit.** Verified in-tree: the **ring provider has no
ML-KEM and no HPKE** (`rustls-0.23.42/src/crypto/ring/` has no `pq/`);
`crypto/aws_lc_rs/pq/` exists. CLAUDE.md pins "rustls with ring provider
only". The good news: the global `install_default()` constraint is not
actually violated by a per-connection
`ClientConfig::builder_with_provider(aws_lc_rs…)` used *only* inside a new
`pqc` probe mode — no process-wide provider change. But it adds the aws-lc-rs
build dependency (cmake; NASM on Windows for some versions) to a feature
flag, which is an owner decision → fit/effort penalized. Recommend:
feature-gated `pqc` mode (off in default build, on in release binaries once CI
proves Windows builds), probing three handshakes: classical-only offer,
hybrid offer, and hybrid-preferred — reporting negotiated group + handshake
ms + ClientHello size for each.
**Implementation path.** New protocol variant → full CLAUDE.md checklist
(metrics/dispatch/summary/modes.json/docs/integration test). Reuses the
`tls.rs` probe skeleton with a provider/kx-group-restricted config per leg.
**Platform notes.** aws-lc-rs build chain on Windows is the main risk;
`--no-default-features` stub must mirror the API per repo rule.
**Cites.** FIPS 203 (ML-KEM); draft-kwiatkowski-tls-ecdhe-mlkem (codepoint
0x11ec); Cloudflare Radar / 2026 PQ-readiness measurement study (arXiv
2606.16473).

### D8 — Encrypted-DNS comparison mode (Do53 vs DoT vs DoH vs DoQ) · **68** (V30 T10 E12 F16)

**What.** The resolver speaks Do53 only. Published methodology (Hounsel et al.
WWW'20 for DoT/DoH; Kosek et al. for DoQ) measures: cold query (includes
TCP/TLS/QUIC setup), warm query (connection reuse amortization), and impact
on page load; DoQ lands within ~2% of Do53 once amortized. Network engineers
increasingly need "is DoH hurting us, and which resolver?" answers.

**Why this is in reach.** Verified in hickory-resolver 0.26.1's feature list:
`tls-ring`, `https-ring`, `quic-ring`, `h3-ring` (Cargo.toml lines 84-140) —
i.e. DoT/DoH/DoQ/DoH3 all available under the ring provider we already pin.
No new TLS stack.

**Implementation path.** Either a `--dns-transport` flag on the existing `dns`
mode (smaller: no new Protocol variant, but mode-manifest drift guards still
need the flag documented) or a `dnscompare` composite mode probing the same
QNAME over each transport against a configured resolver (system resolvers
don't speak DoT/DoH → needs a resolver target argument, e.g. the user's
enterprise resolver or 1.1.1.1/8.8.8.8 presets). Report cold_ms and warm_ms
per transport (two sequential queries on one resolver instance — hickory
keeps the connection). Cache must be off (D1 machinery). Moderate effort:
feature wiring + config plumbing + per-transport `ResolverConfig`.
**Platform notes.** None beyond binary size.
**Cites.** RFC 7858 (DoT), RFC 8484 (DoH), RFC 9250 (DoQ); Hounsel et al.
2020; Kosek et al. 2023.

### D9 — Chain trust-path diagnosis: missing intermediates, AIA, cross-signs · **69** (V27 T12 E14 F16)

**What.** We record the chain *as served* (`tls.rs:961-970`) and rustls
verifies it, but when a server omits an intermediate, our probe fails with a
generic TLS error while Chrome/Safari succeed (browsers do AIA chasing and
carry intermediate preload sets; rustls/webpki deliberately do neither). We
produce no diagnosis for the single most common "works in the browser, fails
from the app" PKI incident. We also can't distinguish a cross-signed chain
(e.g. historical ISRG-via-DST paths) from a clean one.

**Implementation path.** Pure post-hoc analysis on bytes we already hold, all
outside timing windows: (1) chain-completeness check — walk served chain,
verify `chain[i].issuer == chain[i+1].subject`, flag gaps and
leaf-only-served; (2) parse AIA (`caIssuers` OID 1.3.6.1.5.5.7.48.2) from the
leaf with x509-parser and report the URL; optionally (feature-gated
network step) fetch it, timed + sized, and report "chain repairable via AIA:
yes (took N ms)"; (3) flag duplicate-subject/different-issuer pairs as
cross-sign evidence; (4) on verification *failure*, still surface the served
chain — today `make_failed` drops `tls: None` (`tls.rs:1209`), so the user
gets no chain to debug with; capturing the peer chain from the
`OcspRecordingVerifier` wrapper (it already sees `end_entity` +
`intermediates` before verification concludes) fixes that at near-zero cost
and is the highest-trust slice of this item.
**Platform notes.** None. **Cites.** RFC 5280 §4.2.2.1 (AIA); CA/B BR on AIA;
Mozilla intermediate-preloading docs.

### D10 — Certificate Transparency SCT observation · **64** (V22 T10 E17 F15)

**What.** No SCT reporting. Chrome/Safari enforce CT: a leaf without valid
SCTs is rejected by browsers but accepted by our probe (webpki doesn't check
CT) — another "browser disagrees with probe" blind spot, plus SCT count/log
diversity is a standard SSLLabs-class report line.

**Implementation path.** Verified: x509-parser 0.18.1 parses the SCT
extension (`ParsedExtension::SCT(Vec<SignedCertificateTimestamp>)`,
`extensions/mod.rs:234`). In `parse_cert_entry` (`tls.rs:974`), read the SCT
list from the leaf: additive `CertEntry.sct_count: Option<u32>` and
`sct_log_ids: Vec<String>` (hex, truncated) + timestamps. *Signature
verification* against the CT log list is explicitly out of scope (needs a
maintained log-key dataset — see rejected R6); presence/count/log-diversity is
the honest deliverable and matches what testssl.sh reports. ~60 lines.
Optionally record TLS-extension-delivered SCTs later (rustls does not expose
the signed_certificate_timestamp extension to clients — verified absence;
embedded-in-cert covers ~all of the ecosystem since 2021).
**Platform notes.** None. **Cites.** RFC 6962 / RFC 9162; Chrome CT policy.

### D11 — Handshake message-level split (ClientHello→ServerHello vs rest) · **58** (V20 T12 E12 F14)

**What.** `tls_ms` is one number. Professional latency analysis splits: (a)
flight-1 network RTT (CH→SH), (b) server crypto/processing (cert +
CertVerify generation), (c) client verification cost. A 300 ms handshake at
20 ms RTT is a server-side crypto/queueing story; the same handshake at 140 ms
RTT is a network story — today we can't tell them apart within TLS.

**Implementation path.** rustls has no message-timing callbacks. Two options:
(1) **Analytic (cheap, ship first):** we already hold `min_rtt_ms` /
`rtt_estimate_ms` from `SocketInfo` on the same socket (`tls.rs:144`);
`handshake_rtt_overhead = tls_ms − expected_flights × rtt` is derivable *in
the report layer today* — TLS 1.3 full = 1 RTT, full-hrr = 2 (we already
record `handshake_kind`!), TLS 1.2 full = 2. Additive computed field or
report-side only. (2) **Instrumented stream (real split):** wrap `TcpStream`
in an `AsyncRead/AsyncWrite` adapter timestamping first-write-byte,
first-read-byte, and handshake completion → `ch_to_sh_ms` ≈ network+server
flight-1, remainder ≈ client verify + ticket flights. ~150 lines, no unsafe,
works for the tls/tlsresume modes; not portable into quinn (h3) without
deeper surgery.
**Platform notes.** None. **Cites.** RFC 8446 §2 (flights); Apple/Meta
handshake-latency writeups use exactly this CH→SH split.

### D12 — Server TLS-configuration matrix (version tolerance, group/cipher preference) · **54** (V24 T8 E10 F12)

**What.** SSLLabs-class scanners enumerate what the *server* supports and
prefers by handshaking repeatedly with narrowed offers. We record only the
single negotiated outcome. A `tlsscan` mode with ~6-10 handshakes could
report: TLS 1.2-only reachability, TLS 1.3-only reachability, group
preference (offer [P-256] vs [X25519] separately, then both reordered),
cipher preference order within our supported set.

**Honest limits (don't inflate).** rustls implements only TLS 1.2/1.3 — we
can never report SSLv3/TLS 1.0/1.1 exposure, so this will not be an SSLLabs
substitute; it's a preference/tolerance probe. Multi-handshake scans also
change the probe's character (N connections per attempt) and can trip
rate-limiters/IDS — needs explicit opt-in flag semantics.
**Implementation path.** Loop over restricted `ClientConfig`s
(`versions`/`kx_groups`/`cipher_suites` are all pluggable per-config in
rustls 0.23); reuse the `tls.rs` skeleton; new mode per CLAUDE.md checklist.
**Cites.** SSLLabs SSL Server Rating Guide; testssl.sh methodology.

### D13 — DNS64/NAT64 detection · **52** (V16 T10 E14 F12)

**What.** RFC 7050 standardizes detection: query AAAA for
`ipv4only.arpa` — a synthesized answer reveals DNS64 + the NAT64 prefix.
One extra lookup in the `dualstack` probe explains an entire class of
"IPv6 leg weirdness" (common on mobile/carrier and some corp networks) that
currently shows up as unexplained AAAA behavior. Small, additive
(`DualStackResult.nat64_detected`, `nat64_prefix`).
**Cites.** RFC 7050; RFC 6146/6147.

### D14 — Certificate lifetime analytics (short-lived-cert era) · **51** (V16 T8 E18 F9)

**What.** We store `expiry` but not `not_before`, so validity-period length
and remaining-lifetime ratio aren't derivable server-side. With Let's Encrypt
6-day certs shipping and the CA/B ballot trajectory (200 d → 100 d → 47 d by
2029), "days remaining" alerting needs period context (10% left of a 6-day
cert ≠ 10% of a 398-day cert). Trivial: add `not_before` to `CertEntry` in
`parse_cert_entry` (x509-parser already exposes `validity().not_before`).
Fit scored low only because alerting lives in the C# layer (persistence gap
#1 of the 2026-07 audit governs whether it's ever seen).

---

## (c) Considered and REJECTED

| # | Item | Why rejected |
|---|---|---|
| R1 | **OCSP live-fetch timing as a measurement** (AIA OCSP URL → timed responder request) | The OCSP era is ending: Let's Encrypt stopped embedding OCSP URLs (May 2025) and turned off responders (Aug 2025); Firefox 137+ uses CRLite and 142 disables OCSP for DV certs; Must-Staple is dead. Timing an infrastructure that covers a shrinking minority of certs would produce mostly-null data. **Corollary worth acting on:** the existing `ocsp_stapled` field (`metrics.rs:1815-1822`) will increasingly read `false`/absent for LE-family certs — the report/UI should annotate it "expected absent for post-2025 Let's Encrypt certs", or it becomes a false red flag. |
| R2 | **CRL fetch timing/size** | Same era problem inverted: browsers don't fetch CRLs on-path either (CRLite/Bloom-cascade distribution); a multi-MB CRL download is not on any user's connection path, so it fails the "measure the path users experience" test. Revisit only as a PKI-hygiene (not latency) report line. |
| R3 | **Full ECH handshake probing** | Requires an HPKE provider — verified absent from rustls' ring backend (only `crypto/aws_lc_rs/hpke.rs` exists). Server-side ECH outside Cloudflare/nginx-1.29.4 is still thin, and rustls' `EchStatus` (verified, `client/ech.rs:282-293`) only becomes reachable with the aws-lc-rs provider (same policy decision as D7). **Partial acceptance:** ECH *config presence in the HTTPS RR* ships cheaply inside D3, which is the deployment-relevant fact ("is ECH available to browsers for this origin"); GREASE-ECH middlebox-tolerance probing can ride D7's provider decision later. |
| R4 | **Session-ticket lifetime hints** | rustls does not expose NewSessionTicket `ticket_lifetime` to the client API (verified: no public accessor). Measuring it empirically means resuming after graduated delays — minutes-long probes for a low-actionability number. `tls13_tickets_received` + tlsresume verdict already cover the operational question ("does resumption work"). |
| R5 | **Per-authoritative-server / iterative-resolution timing** (`dig +trace`-class, per-NS RTT) | Different product: that's authoritative-DNS monitoring (DNSMON territory), not client-path measurement — LagHound's vantage is "what does the user's resolver path feel like". hickory-recursor exists but would bring a recursive resolver into the probe binary for a question our users aren't asking. D5's NSID gets the most valuable slice (which anycast instance) for ~5% of the cost. |
| R6 | **SCT signature verification / CT-log validity** | Needs a continuously-updated CT log key list (Chrome's log_list.json) shipped and refreshed in the binary — a dataset-maintenance treadmill for marginal gain over D10's presence/count. Same reasoning as the standing rejection of offline GeoIP DBs. |
| R7 | **HSTS preload-list membership** | The header itself is already captured (SecurityHeaders, measurement-gap #14). Membership requires bundling and refreshing the ~100k-entry Chromium preload list. Dataset treadmill; header + `includeSubDomains`/`preload` token presence is the measurable-at-probe-time truth. |
| R8 | **DANE/TLSA** | Re-affirmed from 2026-07 audit (scored 30 there): HTTPS-world deployment remains negligible; no browser validates it. |
| R9 | **Full happy-eyeballs v2 racing implementation** (RFC 8305 staggered connect race as the *measurement*) | The `dualstack` probe already reports per-family truth plus an RFC 8305 §5 250 ms-grace *verdict* (`dualstack.rs:27`), which is the diagnostic users need. Actually racing connections would measure our scheduler as much as the network, and hides the slow family's absolute numbers — sequential-legs-plus-verdict is the better instrument. Revisit only if a "simulate real client connect latency" number is demanded, and then in the TCP module (M1), not here. |
| R10 | **DNSSEC full validation in-probe** (hickory `dnssec-ring` + `ResolverOpts::validate`, both verified present) | Deliberately *deferred* rather than embraced: validating in-probe measures *our* validator, not the user's path — the user-relevant fact is whether *their resolver* validates, which is the AD bit + a known-bad-domain (dnssec-failed.org) check, and that rides D5's raw-query lane for a fraction of the effort. Full in-probe validation (chase DS/DNSKEY, per-zone timing) is real work with real value for DNS-heavy customers; score it when D5 exists to build on. |

---

## (d) Top-5 shortlist

| Rank | Item | Score | One-line why |
|---|---|---|---|
| 1 | **D1 — defeat/label the in-process DNS cache** | 88 | Trust-class defect: today's repeat-attempt `dns_ms` measures a hashmap. Cheapest fix with the biggest honesty payoff; same family as trust-audit V1/V5. |
| 2 | **D3 — HTTPS/SVCB (type 65) capture + h3-advertisement cross-check** | 75 | The 2026 connection-setup gate (h3 discovery, ECH, hints); hickory-proto rdata already in-tree; differentiating diagnostic vs every mainstream CLI tool. |
| 3 | **D4 — record TTLs** | 74 | Already parsed, currently discarded; ~30 lines; every DNS tool users compare us against shows it. |
| 4 | **D6 — negotiated key-exchange group** | 74 | One verified rustls call; unlocks the PQC narrative (D7) and closes an SSLLabs-class basic. |
| 5 | **D2 — unify h3 DNS onto hickory** | 71 | Restores single-instrument comparability for the flagship h1/h2/h3 story; trivial diff. |

Next tier (do after the five, in order): **D5** (raw-query metadata lane —
also unblocks the deferred DNSSEC AD-bit check, R10), **D9** (chain
diagnosis; ship the "surface served chain on verification failure" slice
first), **D8** (encrypted-DNS comparison), then the owner decision on
**D7/aws-lc-rs** which also unlocks the ECH remainder (R3).

Report-layer follow-ups that need no probe work: annotate `ocsp_stapled` for
the post-OCSP era (R1 corollary) and the analytic handshake-RTT split
(D11 option 1).

---

*Method: full read of dns.rs/tls.rs, targeted reads of http3.rs/native.rs/
dualstack.rs/metrics.rs/dispatch.rs; dependency capabilities verified in
`~/.cargo/registry` sources for hickory-resolver/proto 0.26.1, rustls 0.23.42,
x509-parser 0.18.1 (feature lists, `cache_size` default 8192,
`negotiated_key_exchange_group`, `EchStatus`, aws-lc-rs-only `pq/`+`hpke`,
`ParsedExtension::SCT`, SVCB/HTTPS rdata). External practice grounded in: RIPE
Atlas DNS measurement docs & Sagan result schema; Hounsel et al. (WWW'20
DoT/DoH), Kosek et al. (DoQ); RFC 9460, 9849, 8305, 7050, 6962, 8446, 2308,
6891, 5001, 7858, 8484, 9250; FIPS 203 + draft-kwiatkowski-tls-ecdhe-mlkem;
2026 PQ-readiness measurement study (arXiv 2606.16473); Mozilla CRLite
rollout + Let's Encrypt OCSP shutdown announcements; SSLLabs/testssl.sh
methodology.*
