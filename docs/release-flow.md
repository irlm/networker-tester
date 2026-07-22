# Release Flow

How a merged PR becomes a deployed release. Written for a new maintainer; the
authoritative implementations are `.github/workflows/ci.yml` (version-check +
auto-tag) and `.github/workflows/release.yml` (build + release + deploy).

## The whole flow in one line

PR (bump 5 version locations) → merge → `auto-tag` pushes `vX.Y.Z` and
dispatches Release → all five platform builds run in parallel → one complete
GitHub release publishes → deploy to laghound.com (~12–14 minutes after
the tag, auto-rollback on failed readiness).

## 1. Version bump (5 locations, CI-enforced)

Every PR that touches a **shipping artifact** (`crates/`, `dashboard/`,
`install.sh`, `install.ps1`, `Cargo.toml`/`Cargo.lock`,
`Directory.Build.props`) must bump the version in exactly these five files:

1. `Cargo.toml` — workspace `version`
2. `CHANGELOG.md` — new `## [X.Y.Z]` section (becomes the release notes)
3. `install.sh` — `INSTALLER_VERSION`
4. `install.ps1` — `InstallerVersion`
5. `Directory.Build.props` — `<Version>` (single source for **all** C# assemblies)

Everything else is derived at build time — the C# agent heartbeat, endpoint
`ServerInfo`, and the control plane's `/api/health` + `/api/version` all read
the assembly version. Never hand-bump a version constant in C# code or add
`<Version>` to an individual `.csproj`.

Docs-only / C#-source-only / CI-only PRs do **not** require a bump (the
`version-check` job's SHIPPING filter skips them).

## 2. Gates on the PR

The `Version bump check` job fails the PR if any of the five locations is
missing or inconsistent. Branch protection additionally requires:
`Test (ubuntu-latest)`, `Test (windows-latest)`, `Detect changed areas`,
`Build & audit (C#)`, `bats (installer unit tests)`, `shellcheck`,
`cargo audit (RUSTSEC advisories)`, and `Orchestrator lint & test`.

## 3. Auto-tag on main push

On every push to `main`, the `Auto-tag & deploy` job (ci.yml) reads the
version from `Cargo.toml`. If tag `vX.Y.Z` doesn't exist yet, it creates and
pushes it, then explicitly dispatches `release.yml` with that tag (tags pushed
by the Actions token don't trigger workflows on their own). If the tag already
exists — e.g. a docs-only merge with no bump — nothing happens.

## 4. The release graph (release.yml)

All five builds start in parallel at the tag push; the release is created
once, complete (GitHub releases are immutable after publish — asset uploads
then fail with HTTP 422, which killed the old attach-async pattern):

```
build-linux (musl: tester, endpoint, alethabench + frontend)  ┐
build-csharp (control plane + C# agent, ubuntu runner)        ├─→ release ─→ deploy ─→ verify
build-native (mac x64/ARM64, win x64 — the ~11 min floor)     ┘
```

1. **build-linux** + **build-csharp** (~6 min) and **build-native**
   (~11 min — windows-msvc/chromiumoxide is the floor) run in parallel and
   hand their archives to the release job as artifacts.
2. **release** waits for all of them and publishes ONE complete GitHub
   release with every platform's assets, using the `## [X.Y.Z]` CHANGELOG
   section as notes. No missing-asset window; `releases/latest` is always
   complete.
3. **deploy** ships to the Azure VM as soon as the release exists
   (~12–14 min after the tag): `az vm run-command` on `alethedash-vm` stops
   `alethedash-cs`, moves the old build to `/opt/alethedash-cs.prevbuild`,
   extracts the new control plane, asserts
   `DASHBOARD_PUBLIC_URL=https://laghound.com` into `/etc/alethedash-cs.env`
   (replacing a stale value if present), swaps the Rust
   endpoint/tester/orchestrator binaries and the static frontend, restarts,
   and polls `/api/health/ready` (30 s budget — **auto-rolls-back to
   `.prevbuild` on failure**). It then verifies the public path through nginx
   — BOTH `https://laghound.com/api/health` and the `https://alethedash.com`
   bridge must be 200 (fielded agents hold provision-time alethedash.com WS
   URLs; see `branding.md` domain bridge policy), login must 401 bad creds —
   and refreshes the installer Gist.

Releases v0.28.35–v0.28.38 predate this fix and permanently lack mac/windows
assets (immutable — cannot be repaired); installers fall back to the newest
release that carries the requested asset.

## 5. Asset inventory

| Asset | Built by | Notes |
|-------|----------|-------|
| `networker-tester-<target>.tar.gz` / `.zip` | build-linux / build-native | Rust probe engine (musl, mac x2, windows) |
| `networker-endpoint-<target>.tar.gz` / `.zip` | build-linux / build-native | Rust diagnostic server |
| `alethabench-<target>.tar.gz` / `.zip` | build-linux / build-native | Benchmark orchestrator |
| `dashboard-frontend.tar.gz` | build-linux | Built React SPA (served static by nginx) |
| `networker-controlplane-linux-x64.tar.gz` | build-csharp | Self-contained C# control plane — the laghound.com deployable |
| `networker-agent-cs-linux-x64.tar.gz` | build-csharp | Self-contained single-file C# agent (binary named `networker-agent`) — what tester VMs bootstrap since v0.28.26 |
| `networker-agent-cs-win-x64.zip` | build-csharp | Windows C# agent |

**Not shipped:** the retired Rust `networker-dashboard`/`networker-agent`
crates are off the release train (older tags still carry the Rust agent asset;
the bootstrap's legacy fallback only fires for those).

## 6. Rollback

Two levels:

- **Automatic (in-deploy):** if `/api/health/ready` doesn't come up within
  30 s, the deploy job restores `/opt/alethedash-cs.prevbuild` and fails the
  run.
- **Manual (previous release):** re-dispatch the Release workflow with the
  last-good tag — the deploy job re-ships that version end to end:

  ```bash
  gh workflow run release.yml --field tag=vX.Y.Z
  ```

  The graph rebuilds from the tagged sources and re-runs the deploy job.
  Note: the `release` job runs `gh release create`, which fails if a GitHub
  release for that tag already exists — delete the release first (keeping the
  tag) so the job can recreate it:

  ```bash
  gh release delete vX.Y.Z --yes   # deletes the release, not the tag
  gh workflow run release.yml --field tag=vX.Y.Z
  ```

For rollback of the *cutover itself* (C# → Rust control plane) see
[`phase2-cutover-runbook.md`](phase2-cutover-runbook.md) §5 — only relevant
during the decommission soak window.

## 7. Post-release checks

- `gh run list --branch main` — confirm auto-tag and the Gist sync landed.
- `gh run watch` the Release run; the `deploy` and `Verify deployment` steps
  print service status and binary versions from the VM.
- The nightly `Prod soak check` workflow (06:47 UTC) validates
  `/api/health`, `/api/health/background` (`all_healthy`), queue depth, and
  that the retired Rust services stay inactive.

## Dependency updates (Dependabot)

Dependabot PRs are exempt from the version-bump requirement (author-gated in
ci.yml's version-check): they are dependency-only, and their lockfile/manifest
changes ship with the **next** bumped release — release builds are `--locked`
against the merged lockfile.
