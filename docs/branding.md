# Branding

The product brand is **Networker**. One suite, one name:

| Surface | Name |
|---------|------|
| Product / UI (dashboard title, login, reports) | **Networker** |
| CLI probe engine (Rust crate + binary) | `networker-tester` |
| Diagnostic server (Rust crate + binary) | `networker-endpoint` |
| C# projects | `Networker.*` (`ControlPlane`, `Agent`, `Contracts`, `Data`, `Security`, `Endpoint`) |
| Frontend npm package | `networker-dashboard` |
| Benchmark orchestrator | **Networker Bench** — binary/package name `alethabench` (historical, see below) |

## Historical / deployment names (intentionally NOT renamed)

These are live identifiers, not brand. Renaming them is an ops migration that
has not been ordered — they stay until one is.

- **`alethedash.com`** — the current production deployment's domain, and the
  naming stem for its infrastructure: Azure resource group `ALETHEDASH-RG`,
  VM `alethedash-vm`, systemd service `alethedash-cs`, database `alethedash`,
  `/etc/alethedash-cs.env`, `/opt/alethedash*` paths, the AWS security-group/
  key-pair/tag name `alethedash-tester`, backup storage, and related GitHub
  secrets. Docs and workflows that reference these are describing the
  deployment, not the brand.
- **`alethabench`** — the orchestrator's historical binary/package name
  (`benchmarks/orchestrator`, package `alethabench-orchestrator`), plus its
  cloud identifiers (`alethabench-rg`, `alethabench-sg`, `alethabench-key`,
  the `alethabench=true` tag) and release assets `alethabench-<target>.*`.
  Renaming the binary would break the release asset chain (`release.yml`
  packaging, deploy install step, existing install references) — possible
  future step, deliberately deferred. All display strings say
  "Networker Bench (alethabench)". Note the spelling: the identifier is
  `aletha…`, the retired prose form was "AletheBench" — the prose form is
  gone; only the lowercase identifier remains.

## Known legacy env-var dialects

Env-var *names* are wire/ops compatibility surfaces and are not renamed for
brand reasons:

- `DASHBOARD_*` — read by the C# control plane (inherited from the Rust
  dashboard era).
- `AGENT_*` — agent settings (`AGENT_DASHBOARD_URL`, `AGENT_API_KEY`, …).
- `NETWORKER_*` — tester CLI (DB URLs etc.).
- `BENCH_*` — Rust endpoint + benchmark reference APIs.
- `ENDPOINT_*` — C# endpoint.

Any future rationalization (e.g. `CONTROLPLANE_*` with deprecated fallbacks)
is tracked separately from branding.
