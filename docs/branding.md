# Branding

The product brand is **LagHound** (capital L, capital H).

## Decision record: Networker → LagHound (2026-07-20)

The product brand renamed from **Networker** to **LagHound**. Rationale:
catchier and ownable — "Networker" is generic and unclaimable as a name;
the `laghound.com` / `laghound.sh` domains are purchased. The hound logo is a
later design task; the text wordmark (existing brand purple/cyan tokens,
unchanged) is the mark for now. The single source of truth in the frontend is
`dashboard/src/lib/brand.ts` (`PRODUCT_NAME`).

## Domain cutover status (2026-07-20)

- **Phase 1 (product surfaces): complete** — user-visible brand strings say
  LagHound (#483).
- **Phase 2 (serving infra): complete** — `https://laghound.com` is LIVE and
  serves production (nginx server block + TLS cert done). `https://laghound.sh`
  serves the installer script to `curl` clients and 301s browsers to the
  dashboard. `leghound.com` (typo domain) 301s to `laghound.com`.
- **Phase 3 (pipeline + docs): complete** — release deploy asserts
  `DASHBOARD_PUBLIC_URL=https://laghound.com` (replacing stale values), the
  deploy-verify and nightly soak check both probe `laghound.com` AND the
  `alethedash.com` bridge, and docs/installer one-liners point at the new
  domains.

### Domain bridge policy — `alethedash.com`

Fielded tester agents hold **provision-time** WebSocket URLs pointing at
`alethedash.com`, so the old domain is a live compatibility surface:

1. `alethedash.com` stays **fully functional** (API + WS + UI, not a redirect)
   for at least one full fleet re-provision cycle after the cutover. The
   nightly soak check alerts if the bridge stops answering 200.
2. After the fleet no longer holds `alethedash.com` URLs, it may be demoted to
   a browser-only 301 to `laghound.com`.
3. The registration is kept for **at least 1 year** to prevent takeover of a
   domain that fielded binaries and docs have referenced.

Policy restated: **only user-visible product-brand strings rename.**
Infrastructure identifiers — crate/binary names, release asset names, C#
namespaces (`Networker.*`), env var names (`DASHBOARD_*`, `AGENT_*`,
`NETWORKER_*`, …), the `X-Networker-Signature` webhook header, systemd unit
names, Azure/AWS resource names, DB schema, and the repo name/URLs — are
wire/ops compatibility surfaces and do NOT change.

## Surface table

| Surface | Name |
|---------|------|
| Product / UI (dashboard title, login, reports) | **LagHound** |
| CLI probe engine (Rust crate + binary) | `networker-tester` |
| Diagnostic server (Rust crate + binary) | `networker-endpoint` |
| C# projects | `Networker.*` (`ControlPlane`, `Agent`, `Contracts`, `Data`, `Security`, `Endpoint`) |
| Frontend npm package | `networker-dashboard` |
| Benchmark orchestrator | **LagHound Bench** — binary/package name `alethabench` (historical, see below) |

## Historical / deployment names (intentionally NOT renamed)

These are live identifiers, not brand. Renaming them is an ops migration that
has not been ordered — they stay until one is.

- **`alethedash-*` infrastructure names** — the naming stem for the production
  deployment's infrastructure: Azure resource group `ALETHEDASH-RG`, VM
  `alethedash-vm`, systemd service `alethedash-cs`, database `alethedash`,
  `/etc/alethedash-cs.env`, `/opt/alethedash*` paths, the AWS security-group/
  key-pair/tag name `alethedash-tester`, backup storage, and related GitHub
  secrets. These stay even though the public domain is now `laghound.com` —
  docs and workflows that reference them are describing the deployment, not
  the brand. The `alethedash.com` **domain** itself is now the compatibility
  bridge (policy above).
- **`alethabench`** — the orchestrator's historical binary/package name
  (`benchmarks/orchestrator`, package `alethabench-orchestrator`), plus its
  cloud identifiers (`alethabench-rg`, `alethabench-sg`, `alethabench-key`,
  the `alethabench=true` tag) and release assets `alethabench-<target>.*`.
  Renaming the binary would break the release asset chain (`release.yml`
  packaging, deploy install step, existing install references) — possible
  future step, deliberately deferred. Display strings currently still say
  "Networker Bench (alethabench)" — sweeping them to "LagHound Bench
  (alethabench)" (orchestrator CLI about, report title/footer, reference-api
  READMEs) is a phase-2 rename task, not done in the phase-1 product-surface
  pass. Note the spelling: the identifier is
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
