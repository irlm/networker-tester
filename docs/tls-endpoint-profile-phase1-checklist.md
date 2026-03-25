# TLS Endpoint Profile — Phase 1 Implementation Checklist

This checklist derives from `docs/tls-endpoint-profile-design.md` and narrows the work to the first practical implementation milestone.

## Goal of Phase 1

Deliver a useful first version of `tls_endpoint_profile` that provides:
- target resolution
- path basics
- one TLS handshake worth of negotiated facts
- certificate chain parsing
- basic trust and hostname validation
- basic resumption check
- structured findings
- summary status
- explicit coverage/limitations metadata

Phase 1 is intentionally **not** the capability matrix phase.

---

## Scope included in Phase 1

## 1. Result contract

- [ ] Define a stable top-level result struct for `tls_endpoint_profile`
- [ ] Include:
  - [ ] `target_kind`
  - [ ] `coverage_level`
  - [ ] `unsupported_checks`
  - [ ] `limitations`
  - [ ] `target`
  - [ ] `path_characteristics`
  - [ ] `connectivity`
  - [ ] `certificate`
  - [ ] `trust`
  - [ ] `resumption`
  - [ ] `findings`
  - [ ] `summary`
- [ ] Keep `capabilities` absent or `null` in Phase 1, but reserve room for later expansion
- [ ] Ensure JSON serialization is stable and test-covered

---

## 2. Input model

- [ ] Support host + port input
- [ ] Support optional SNI override
- [ ] Support optional IP override
- [ ] Distinguish target contexts:
  - [ ] `managed_endpoint`
  - [ ] `external_url` / `external_host`
- [ ] Record target context in output

---

## 3. DNS and connection path basics

- [ ] Resolve hostname to IPs when target is a hostname
- [ ] Record resolved IP list
- [ ] Record actual connected IP
- [ ] Determine whether the connected IP matched the resolved set
- [ ] Detect obvious explicit proxy influence if available from config/env
- [ ] Emit a conservative path classification:
  - [ ] `direct`
  - [ ] `indirect_expected`
  - [ ] `indirect_suspicious`
- [ ] Add evidence strings for path classification decisions

Phase 1 note:
- keep path/interception detection conservative
- prefer “unknown” / weak claims over overconfident heuristics

---

## 4. Single-handshake TLS observation

- [ ] Open TCP connection
- [ ] Measure TCP connect duration
- [ ] Perform TLS handshake
- [ ] Measure TLS handshake duration
- [ ] Record negotiated:
  - [ ] TLS version
  - [ ] cipher suite
  - [ ] ALPN
  - [ ] key exchange group if available
- [ ] Capture peer certificate chain from the handshake

---

## 5. Certificate parsing

- [ ] Parse leaf certificate
- [ ] Parse presented intermediates
- [ ] Extract for leaf and chain entries where available:
  - [ ] subject
  - [ ] issuer
  - [ ] serial number
  - [ ] validity window
  - [ ] SAN DNS names
  - [ ] SAN IPs
  - [ ] key type
  - [ ] key size
  - [ ] signature algorithm
  - [ ] SHA-256 fingerprint
  - [ ] SPKI SHA-256 hash
- [ ] Extract revocation-related metadata passively:
  - [ ] OCSP URLs
  - [ ] CRL URLs
  - [ ] AIA issuer URLs
- [ ] Detect presence of Must-Staple if available
- [ ] Detect presence of SCT/CT evidence if available

Phase 1 note:
- extract revocation metadata, but do not overpromise active revocation certainty

---

## 6. Trust and hostname validation

- [ ] Validate hostname against leaf certificate
- [ ] Evaluate whether the chain appears structurally complete
- [ ] Attempt validation against system/local trust store
- [ ] Record:
  - [ ] `hostname_matches`
  - [ ] `chain_valid`
  - [ ] `trusted_by_system_store`
  - [ ] trust issues / notes
- [ ] Distinguish between:
  - [ ] cryptographic/chain failure
  - [ ] hostname mismatch
  - [ ] trust-store failure
  - [ ] inability to determine with certainty

---

## 7. Basic revocation / CT / CAA enrichment

- [ ] Include passive revocation metadata in the result
- [ ] Record whether OCSP stapling was observed, if visible
- [ ] Mark revocation status as best-effort / unknown unless definitively checked
- [ ] Detect SCT/CT evidence presence where available
- [ ] Perform DNS CAA lookup when hostname context exists
- [ ] Record whether CAA is present and the raw records found

Phase 1 note:
- CAA lookup is useful and relatively self-contained
- live OCSP/CRL validation should remain conservative unless implementation proves robust

---

## 8. Basic session resumption check

- [ ] Perform an initial handshake
- [ ] Attempt a second handshake using the same client/session state
- [ ] Determine whether resumption appears to have occurred
- [ ] Measure resumed handshake time
- [ ] Record:
  - [ ] `supported`
  - [ ] probable method (`ticket`, `session_id`, `unknown`)
  - [ ] initial vs resumed timing
- [ ] Include 0-RTT fields only if explicitly supported by the implementation, otherwise set to false/null

Phase 1 note:
- keep this to a basic resumption probe, not a full 0-RTT analysis

---

## 9. Findings model

- [ ] Define a structured finding type with at least:
  - [ ] `severity`
  - [ ] `code`
  - [ ] `message`
- [ ] Generate findings for obvious cases such as:
  - [ ] hostname mismatch
  - [ ] expired cert
  - [ ] not yet valid cert
  - [ ] self-signed / untrusted chain
  - [ ] missing intermediate / incomplete chain
  - [ ] no TLS 1.3 support (optional only if actually observed in Phase 1, otherwise defer)
  - [ ] no OCSP stapling observed (info/warn as appropriate)
  - [ ] path appears indirect/suspicious
  - [ ] resumption unsupported
- [ ] Keep findings factual and explainable

---

## 10. Summary model

- [ ] Define summary with at least:
  - [ ] `status` (`ok`, `warn`, `error`)
  - [ ] `score` set to `null` in Phase 1 unless a documented rubric exists
- [ ] Derive summary from findings severity, not ad hoc string logic

---

## 11. Coverage and limitation reporting

- [ ] Populate `coverage_level` correctly for the target context
- [ ] Populate `unsupported_checks` for not-yet-implemented features
- [ ] Populate `limitations` to avoid false precision
- [ ] Ensure external URLs/hosts clearly indicate client-observed limitations

This is one of the most important product honesty requirements in Phase 1.

---

## 12. CLI / invocation surface

Choose one approach and keep it minimal.

Option A: dedicated mode/flags
- [ ] Add TLS profile CLI entry point
- [ ] Add host/port/SNI/IP inputs
- [ ] Add JSON output support

Option B: integrate into existing diagnostic surface
- [ ] Add a new test mode under existing execution model
- [ ] Reuse current output and persistence plumbing where possible

Phase 1 recommendation:
- choose the least invasive path that still produces stable JSON output

---

## 13. Tests to add

- [ ] unit tests for certificate parsing helpers
- [ ] unit tests for hostname validation logic
- [ ] unit tests for finding generation
- [ ] unit tests for summary derivation
- [ ] unit tests for JSON serialization shape
- [ ] controlled integration-style tests for:
  - [ ] valid chain
  - [ ] self-signed cert
  - [ ] hostname mismatch
  - [ ] expired cert fixture if practical
  - [ ] IP target with SNI override
  - [ ] resumption supported / unsupported behavior where practical

---

## Explicitly out of scope for Phase 1

- [ ] full TLS version support matrix
- [ ] full accepted cipher suite matrix
- [ ] full ALPN behavior matrix
- [ ] no-SNI / with-SNI multi-variant matrix
- [ ] deep client-auth / mTLS behavior analysis
- [ ] sophisticated interception fingerprinting
- [ ] scoring rubric beyond `status`
- [ ] dashboard persistence/view work unless implementation ends up trivial

Those belong to later phases.

---

## Exit criteria for Phase 1

Phase 1 is done when:
- [ ] a user can run `tls_endpoint_profile` against a host
- [ ] the result is structured and JSON-serializable
- [ ] the result contains certificate, trust, negotiated TLS, and basic resumption information
- [ ] the result clearly states what is known vs best-effort
- [ ] the result includes findings + summary status
- [ ] tests cover the core parsing and output behavior

---

## Recommended implementation order

1. [ ] Result structs and JSON tests
2. [ ] Single handshake + negotiated metadata
3. [ ] Certificate parsing helpers
4. [ ] Hostname/trust validation
5. [ ] Findings + summary
6. [ ] Basic resumption probe
7. [ ] CAA / passive revocation metadata
8. [ ] CLI wiring
9. [ ] Final test pass and cleanup

This ordering gets a useful core working early and keeps the riskier parts for later in the phase.
