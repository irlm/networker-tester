# Proposed PR - richer packet / telemetry observability

- Status: proposed
- Branch: `feat/richer-packet-telemetry-observability`
- PR: _TBD after creation_

## Why this PR exists

`networker-tester` already supports optional tester-side packet capture and writes a small
capture summary, but the current output is too shallow to explain benchmark behavior.
Right now we can usually answer:

- was capture enabled?
- how many TCP / UDP / QUIC packets were seen?
- were there retransmissions / duplicate ACKs / resets?

That is useful, but it is not enough to explain questions like:

- did the browser stay on QUIC or fall back to TCP/TLS?
- which endpoints and ports dominated the trace?
- was the trace mostly DNS, handshake, app data, or unrelated background traffic?
- was the run ambiguous because visibility was loopback-only, off-box, or mixed traffic?
- which capture signals should make us trust or distrust a protocol comparison?

Recent paper review reinforced that realistic benchmarking needs stronger observability,
not just more timing numbers. Browser QoE and impairment testing tell us **what** changed;
packet telemetry helps explain **why**.

## Problem statement

Current packet capture support is best described as MVP capture plumbing:

- start `tshark`
- save `.pcapng`
- produce a compact JSON summary

The summary is missing structure needed for benchmark interpretation and report surfacing.
It does not yet provide a good per-run explanation layer.

## Goals

This PR should improve packet telemetry enough that a test run can answer:

1. **Protocol path clarity**
   - Did traffic look primarily like TCP, TLS, HTTP, UDP, or QUIC?
   - Did the run show mixed transport behavior that may indicate fallback or third-party traffic?

2. **Endpoint / flow visibility**
   - Which remote IPs / ports dominated the trace?
   - Which conversations likely correspond to the target workload?

3. **Interpretation support**
   - When should the user trust the capture summary?
   - When should the tool warn that the trace may be incomplete, noisy, or ambiguous?

4. **Better reporting**
   - Surface richer capture findings in machine-readable JSON and human-readable reports.

## In scope

### Phase 1: richer capture summary structure

Extend the packet-capture summary JSON with higher-value fields such as:

- packet ratios / percentages by protocol family
- top conversations / endpoints / ports
- likely target-related endpoint candidates
- capture interpretation warnings
- a small number of derived booleans such as:
  - `observed_quic`
  - `observed_tcp_only`
  - `observed_mixed_transport`
  - `capture_may_be_ambiguous`

### Phase 2: report surfacing

Surface the richer summary in outputs such as:

- JSON artifacts
- HTML report
- Excel workbook where sensible

The goal is not to dump raw packet trivia everywhere; it is to expose the fields that help
explain benchmark outcomes.

### Phase 3: quality / validation

- add unit coverage for summary derivation helpers
- keep behavior graceful when `tshark` filters fail or some fields are unavailable
- preserve current opt-in capture behavior

## Out of scope

This PR should **not** try to do all of the following at once:

- endpoint-side packet capture implementation
- full qlog export / import support
- deep TCP stream reconstruction
- packet-by-packet timeline UI
- PCAP post-processing at large scale
- full browser fallback detection guarantees

Those can follow later. This PR should create a stronger observability layer without turning into
an entire network-forensics product.

## Proposed deliverables

1. New structured packet summary fields in `capture.rs`
2. Serialization of the richer summary to `packet-capture-summary.json`
3. Report surfacing in HTML / Excel / JSON-facing outputs where appropriate
4. Documentation update describing how to interpret the new fields
5. Tests covering summary derivation and ambiguity warnings

## Proposed implementation notes

Likely implementation shape:

- keep `tshark` as the source of truth for now
- avoid excessive repeated passes where possible
- prefer a compact derived summary over raw field explosion
- make warnings explicit when loopback / remote capture visibility may hide transport details

## Suggested user-facing interpretation model

Each capture summary should try to communicate:

- **Observed transport:** what was clearly seen
- **Dominant conversations:** where the traffic mostly went
- **Signals of trouble:** retransmits, resets, duplicate ACKs, unexpected transport mix
- **Confidence / ambiguity note:** whether the trace cleanly supports protocol conclusions

## Planned follow-up after this PR

If this lands cleanly, the next logical PRs are:

- impairment-profile matrix improvements
- deployment / implementation comparison improvements
- richer congestion-control / context metadata

## Commit / PR workflow note

This folder exists so the branch carries its own rationale. The plan is:

1. commit this spec first
2. open the PR
3. write the actual PR number back into this document
4. continue implementation on the same branch
