# C# Fidelity Audit — Rust-era ambient assumptions vs. the tarball deployment model

**Date:** 2026-07-19 · **Trigger:** prod incident — `DeployRunner` could not find
`install.sh` on the C# deployment (`/opt/alethedash-cs`), so every testbed
provisioning failed. **Question:** what else in `src/Networker.*` assumes the
Rust-era environment (service run from / next to a repo checkout, installer-managed
env file, login-user home dir) that the C# deployment model does not provide?

**The C# deployment model** (evidence: `.github/workflows/release.yml:359-494`):
bare `dotnet publish` tarball extracted to `/opt/alethedash-cs` (contains ONLY
publish output — no repo files), systemd unit `alethedash-cs` with
`EnvironmentFile=/etc/alethedash-cs.env`. The unit file itself is **not in the
repo** (created manually at cutover), so `WorkingDirectory`, `User`, `HOME`, and
`PATH` are unspecified from the repo's point of view; systemd defaults are
`WorkingDirectory=/` and the minimal
`PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`. The deploy
asserts exactly **one** env var into the env file (`DASHBOARD_PUBLIC_URL`,
release.yml:409-410).

**The Rust-era model it replaced** (evidence: `install.sh:4167-4275`): the
installer *copied `install.sh` to `/opt/networker/install.sh` and wrote
`INSTALL_SH_PATH=/opt/networker/install.sh`* plus `DASHBOARD_STATIC_DIR`, DB URL,
JWT secret, and credential key into `/etc/networker-dashboard.env`, and generated
the unit (`User=$(whoami)`, `WorkingDirectory=/tmp`). Every ambient file the Rust
binary needed was placed and pointed to **by the installer**. The C# deploy has no
equivalent placement step — that is the root cause of this class of bug.

---

## Findings

Severity: P0 = breaks a core prod flow now · P1 = breaks/lies on a real user flow
or is one config-recreation away from data/feature loss · P2 = degraded/stub
behavior, wrong under specific config · P3 = latent/dev-only/verified-safe.

### 1. File / path dependencies

| # | Location | Assumption | Verdict | Sev |
|---|----------|------------|---------|-----|
| F1 | `src/Networker.ControlPlane/Provisioning/DeployRunner.cs:297-329` (`FindInstallSh`) | `install.sh` findable via `INSTALL_SH_PATH`, else cwd + 5 parents, else `AppContext.BaseDirectory` + 7 parents | **BREAKS (today's incident).** Tarball ships no `install.sh` (release.yml:278-284 packs only `dotnet publish` output). Under systemd cwd=`/`; BaseDirectory walk covers `/opt/alethedash-cs → /opt → /` — none has the script. The deploy never asserts `INSTALL_SH_PATH` into `/etc/alethedash-cs.env` (only `DASHBOARD_PUBLIC_URL`, release.yml:409-410). Rust survived because `install.sh:4194,4199-4204` copied itself to `/opt/networker/install.sh` and set `INSTALL_SH_PATH` — an installer-era guarantee the tarball deploy silently dropped. Every `ProvisioningOrchestrator` tick (ProvisioningOrchestrator.cs:258) and `POST` deploy (DeploymentWriteEndpoints.cs:225) soft-fails with "install.sh not found". | **P0** |
| F2 | `src/Networker.ControlPlane/Security/CredentialCipherExtensions.cs:50-107` | `DASHBOARD_CREDENTIAL_KEY` env var is the *only* key source | **DIVERGES from Rust, one env-file recreation from disaster.** Rust `config.rs:199-244` fell back to `DASHBOARD_CREDENTIAL_KEY_FILE`, `/var/lib/networker/credential.key`, `~/.config/networker/credential.key` (and auto-generated + persisted). C# is env-only, fail-closed. Good: it can't silently rotate. Bad: a key that lives only in a Rust-era key file is invisible to C#, and the deploy does **not** assert/back up the key in `/etc/alethedash-cs.env` — recreating that file loses decryptability of every cloud-account secret permanently (this class already bit once — the v0.28 "credential key loss" bug). | **P1** |
| F3 | `CliComputeProvisioner.cs:769-771, 804, 932-934`; `TesterPrecheckEndpoints.cs:203-205` | `Environment.GetEnvironmentVariable("HOME")` + `/.ssh/id_rsa.pub` | **FRAGILE.** systemd system units do not reliably set `HOME`; code does `?? string.Empty`, so the probe becomes the *relative* path `.ssh/id_rsa.pub` under cwd `/` — silently "no key found": AWS key-pair import silently skipped, GCP VMs created without SSH metadata (warning only), precheck emits a false `gcp_no_local_ssh_key`. Rust had the same env read but ran as the login user with the unit generated at install time. Use `Environment.GetFolderPath(SpecialFolder.UserProfile)` (passwd-backed) and assert `HOME`/`User=` in the codified unit (F13). | **P2** |
| F4 | `CliComputeProvisioner.cs:519` (`--generate-ssh-keys`); `CliComputeProvisioner.cs:807` (writes `~/.ssh/*.pem`) | Service user has a writable home with `~/.ssh` | **FRAGILE.** `az vm create --generate-ssh-keys` and the AWS `create-key-pair` PEM persist need a real, writable home. A `--no-create-home` system user (the pattern `install.sh:4257` uses for `networker`) breaks Linux VM creates. Works today only if the unit runs as a login user — unverifiable from the repo (unit not codified). | **P2** |
| F5 | `src/Networker.Agent/ApibenchWorkloads.cs:41`; `Networker.Agent.csproj:30` | `benchmarks/configs/apibench.json` available at runtime | **SAFE (verified).** Embedded resource (`EmbeddedResource Include="../../benchmarks/configs/apibench.json"`), read via `GetManifestResourceStream` — repo file is a build-time input only. This is the correct pattern F1 should copy. | P3 |
| F6 | `src/Networker.ControlPlane/Provisioning/CloudInitScripts.cs:144-405` | Bootstrap templates | **SAFE (verified).** Templates are compiled string constants (verbatim ports of Rust `cloud_init.rs`); no template files read from disk. Binaries are fetched from GitHub releases at VM boot, not from the control-plane host. | P3 |
| F7 | `src/Networker.ControlPlane/Endpoints/VersionEndpoints.cs:198-223` | Local `networker-tester` binary discoverable | **SAFE on prod (verified).** Candidates include `/usr/local/bin`; the deploy copies the tester there (release.yml:430). `AGENT_TESTERPATH` override exists. | P3 |
| F8 | `src/Networker.Agent/TesterBinaryLocator.cs:30-77` | `target/{debug,release}/` cwd walk, then `which` | **SAFE on provisioned VMs (verified).** The cargo-layout walk is dev-era residue; on tester VMs the bootstrap installs to `/usr/local/bin` (CloudInitScripts.cs:240) and the `which` fallback finds it. `AGENT_TESTERPATH` pins it. | P3 |
| F9 | `src/Networker.Endpoint/BenchData.cs:56-60` | Relative fallback `benchmarks/reference-apis/shared/bench-data.json` | **LATENT.** cwd-dependent relative path — dead under systemd (cwd=`/`). Covered by the `/opt/bench/bench-data.json` absolute candidate and `BENCH_DATA_PATH`; prod endpoint is still the Rust binary anyway. Degrades to `null` (no silent wrong data). | P3 |
| F10 | Temp-file usage: `DeployRunner.cs:95`, `CliComputeProvisioner.cs:302,346,446,634,918,937,948`, `OrphanReaperService.cs:448` | `Path.GetTempPath()` writable | **SAFE.** All temp files are written and consumed by the same process (az/aws/gcloud children inherit the namespace even under `PrivateTmp=`). | P3 |

### 2. Process shell-outs

| # | Location | Binary | Verdict | Sev |
|---|----------|--------|---------|-----|
| F11 | `DeployRunner.cs:180` | `bash` | Present everywhere on Linux; the *argument* (install.sh) is the problem (F1). | — |
| F12 | `CliComputeProvisioner.cs` (`az`/`aws`/`gcloud`), `OrphanReaperService.cs:575` (`az`), `SshLanguageDetector.cs:122` (`ssh`), `TesterBinaryLocator.cs:88` (`which`/`where`), `VersionEndpoints.cs:164` (tester), `RunExecutor.cs:198` (tester) | bare names resolved via PATH | **MOSTLY OK, gcloud fragile.** systemd's minimal PATH covers `/usr/bin` (apt-installed `az`, `ssh`) and `/usr/local/bin` (`aws` v2, tester). **snap-installed `gcloud` lives in `/snap/bin` — not on systemd's default PATH** → every GCP op soft-fails "failed to launch 'gcloud'". `AZ_CMD` override exists for az only; there is no `AWS_CMD`/`GCLOUD_CMD`. Same class as the Rust era (same VM, same systemd), so not a regression — but the soft-fail contract makes it *silent*. | **P2** |
| F13 | Whole shell-out surface | Ambient `az` session (managed-identity scopes), `ssh` default identity (`~/.ssh`), cwd, `HOME` all belong to the **unit's user** — and the `alethedash-cs` unit exists only as hand-applied state on the VM | **STRUCTURAL GAP.** Nothing in the repo pins `User=`, `WorkingDirectory=`, `Environment=`, or `PATH` for the C# service; release.yml just `systemctl stop/start`s a unit it assumes exists. The Rust era *generated* its unit in `install.sh:4259-4275`. Until the unit (or a drop-in) is committed and asserted by the deploy, every finding in this table is one manual-VM-edit from recurring. SP-credential az paths are already hardened (isolated `AZURE_CONFIG_DIR` in temp — CliComputeProvisioner.cs:302, OrphanReaperService.cs:448); ambient/managed-identity paths and ssh are not. | **P1** |

### 3. Environment variables (code vs. runbook §1.1 vs. deploy assertions)

Documented in `docs/phase2-cutover-runbook.md` §1.1: `DASHBOARD_JWT_SECRET`,
`DASHBOARD_CREDENTIAL_KEY(_OLD)`, `DASHBOARD_DB_URL_NPGSQL`/`ConnectionStrings__Networker`,
`DASHBOARD_BACKGROUND_SERVICES`, `ASPNETCORE_ENVIRONMENT`, `ASPNETCORE_URLS`,
`DASHBOARD_PUBLIC_URL`. Asserted by the deploy: **only `DASHBOARD_PUBLIC_URL`**.

| # | Var(s) | Read at | Verdict | Sev |
|---|--------|---------|---------|-----|
| F14 | `INSTALL_SH_PATH` | DeployRunner.cs:299 | **Undocumented + unasserted** — the designated escape hatch for F1 is mentioned nowhere in §1.1, release.yml, or README. | **P1** (part of F1 fix) |
| F15 | `AZURE_SUBSCRIPTION_ID`, `DASHBOARD_AZURE_SUBSCRIPTION`, `DASHBOARD_AZURE_RG` | TesterWriteEndpoints.Create.cs:650-668 | Legacy provider fallback (Rust `legacy_azure_provider`) — undocumented; a prod box relying on it in the Rust env file must be carried to `/etc/alethedash-cs.env` by hand. | P2 |
| F16 | `AZ_CMD` | CliComputeProvisioner.cs:131,288; OrphanReaperService.cs:575 | Undocumented dev shim; honored — fine, document it. | P3 |
| F17 | `DEPLOY_CONCURRENCY` | TesterWriteEndpoints.Create.cs:208 | Undocumented (Rust main.rs:458 read the same). | P3 |
| F18 | `DASHBOARD_ACS_CONNECTION_STRING`, `DASHBOARD_ACS_SENDER` | EmailSenderExtensions.cs:28-29; MembersEndpoints.cs:347-348 | Same names as Rust `email.rs` — carries over, but undocumented in §1.1; without them email silently degrades to log-only (see F26). | P3 |
| F19 | `DASHBOARD_PORT`, `DASHBOARD_SHARE_URL`, `DASHBOARD_SHARE_MAX_DAYS`, `DASHBOARD_INVITE_EXPIRY_DAYS` | InvitesEndpoints.cs:448-474 | Parity with Rust config.rs; undocumented. `PublicUrl()` falls back to `http://localhost:3000` — share/invite links break if `DASHBOARD_PUBLIC_URL` is ever lost (deploy guard covers it). | P3 |
| F20 | `BENCH_KEYVAULT_NAME`, `BENCH_MOCK_TOKENS` | BenchTokensEndpoints.cs:52,139 | Undocumented; and the CLI behind them is stubbed (F24). | P3 |
| F21 | `DASHBOARD_MAX_SUBS_PER_PROJECT`, `DASHBOARD_MAX_SUB_MSGS_PER_MIN`, `NETWORKER_RUN_MIGRATIONS` | TesterQueueRegistry.cs:44; TesterQueueHub.cs:258; RawWs/TesterQueueSocketEndpoint.cs:383; Program.cs:116 | Undocumented ops knobs. | P3 |
| F22 | Rust-only vars silently dropped: `DASHBOARD_BIND_ADDR`, `DASHBOARD_CORS_ORIGIN`, `DASHBOARD_STATIC_DIR`, `DASHBOARD_ADMIN_PASSWORD/EMAIL`, `SSO_MICROSOFT_*`/`SSO_GOOGLE_*` (config.rs:41-151) | — | **Intentional** (Kestrel binds via `ASPNETCORE_URLS`; nginx serves static; SSO providers live in DB). No hidden dependency — but §1.1 should say so, or an operator will keep setting them. | P3 |

### 4. Rust-vs-C# behavior diffs on the ambient seams (the "pretends to work" list)

| # | C# location | Rust counterpart | Diff | Sev |
|---|-------------|------------------|------|-----|
| F23 | `TesterWriteEndpoints.Lifecycle.cs:185-256` `POST /upgrade` | `api/testers.rs` + `services/tester_install.rs:163-559` (`install_tester` over SSH re-installs binaries, `wait_for_ssh_ready`) | **SILENT LIE.** C# marks `upgrading`, runs a cloud `ShowAsync` probe, then writes **"Upgrade completed (state re-probed)"** — no binary was touched. Testers stay on old versions while the UI reports success. Return 501 or wire the SSH installer/agent self-update command. | **P1** |
| F24 | `UpdateEndpoints.cs:33-72` `POST /api/update/{tester,dashboard}` | `api/update.rs` (tar download, verify, exec-restart, frontend/static + agent update) | **SILENT LIE.** Returns `200 {"status":"updating", update_id}` and does nothing. On the tarball model self-update arguably belongs to the release pipeline — then say so: 501/410, not a fake "updating". | **P1** |
| F25 | `CloudAccountsEndpoints.cs:258-382`, `CloudConnectionsEndpoints.cs:161-230` (validate) | `api/cloud_accounts.rs:652-694`, `api/cloud_connections.rs:166-267` (real `az account show` / `aws sts get-caller-identity` / gcloud round-trips) | Field-presence check marked `valid` — bad/expired creds pass validation and only explode at VM create (or worse, in the orphan reaper's `az login`). | P2 |
| F26 | `InvitesEndpoints.cs:85-88`, `AccountEndpoints.cs:112-118`, `InactivityService.cs:254-311` | Rust emailed via ACS when configured | Email delivery is a logged TODO even though `IEmailSender` is registered (Program.cs:91). Notably the inactivity loop **suspends/deletes workspaces without the 90-day warning email** (runbook §8.2 admits this). | P2 |
| F27 | `InventoryEndpoints.cs:44-48` | `api/inventory.rs:92-255` (az/aws/gcloud VM listing) | Cloud scan stubbed — returns empty `vms`/`errors`, indistinguishable from a genuinely empty cloud. | P2 |
| F28 | `BenchTokensEndpoints.cs:60-127` | `api/bench_tokens.rs:86-305` (az keyvault secret list/set/delete) | With `BENCH_KEYVAULT_NAME` set: list=empty, revoke=log-only. Token revocation **claims success without revoking**. | P2 |
| F29 | `TesterPrecheckEndpoints.cs:151-152,181-183` | `api/tester_precheck.rs:280-388` (orphaned-IP auto-delete, AWS STS expiry check) | Precheck passes without the CLI-backed checks — "ready" is weaker than Rust's "ready". | P2 |
| F30 | `ComparisonGroupsEndpoints.cs:157-161` (launch → bare 202); `LeaderboardEndpoints.cs:16-22` (empty arrays) | Rust dispatched cells / served leaderboards | Known stubs, still current as of this audit. | P2 |
| F31 | `TesterWriteEndpoints.Create.cs:41-52` (`?ssh_bootstrap=1` → 400) | `deploy/agent_provisioner.rs` (`run_create_tester_ssh`) | Explicit, honest 400 — and no frontend caller found (`dashboard/src` grep clean). Correct way to not-port something. | P3 |
| F32 | `BenchmarkCatalogEndpoints.cs:139-150` + `SshLanguageDetector` | `api/benchmark_catalog.rs:227-292` | **PORTED** (runbook §8.3 says "stub" — doc is stale). Depends on the service user's `~/.ssh` identity (F13). | P3 |

### 5. Serving / routing assumptions

| # | Location | Verdict | Sev |
|---|----------|---------|-----|
| F33 | `Program.cs` (no `UseStaticFiles`) vs Rust `main.rs:567` (`DASHBOARD_STATIC_DIR` default `./dashboard/dist` — a *relative* path) | **SAFE by design.** nginx serves the SPA from `/opt/alethedash/dashboard/dist` (release.yml:436-443); C# deliberately dropped the static-serving (and its cwd-relative default). Raw-WS paths `/ws/dashboard|testers|agent` mapped (Program.cs:201-203) — parity per runbook §6. | P3 |
| F34 | `dashboard/vite.config.ts:24-27` proxies dev `/api` + `/ws` to `localhost:3000` — the **retired Rust dashboard's port** | **DEV BREAKAGE.** CLAUDE.md's dev flow starts the control plane on `:5030`; the checked-in proxy still points at the dead Rust port. Every fresh dev setup hits connection-refused until they discover this. | P2 |
| F35 | `SsoFlowEndpoints.cs:38-41`, `CollabConfig.PublicUrl()` fallback `http://localhost:3000` | Harmless while the deploy asserts `DASHBOARD_PUBLIC_URL`; the fallback still names the retired service's port (misleading in logs/links if the guard ever regresses). | P3 |

---

## Prioritized fix list

1. **[P0 — F1/F14] Restore the install.sh guarantee.** Two independent layers,
   mirroring the Rust installer's own pattern:
   (a) ship `install.sh` inside the control-plane tarball (add it to the publish
   output in release.yml's `build-csharp`, next to `Networker.ControlPlane` so
   `FindInstallSh`'s BaseDirectory probe hits with zero config — the F5
   embedded-resource philosophy applied at package level);
   (b) make the deploy script assert `INSTALL_SH_PATH` into
   `/etc/alethedash-cs.env` idempotently, exactly like the existing
   `DASHBOARD_PUBLIC_URL` guard (release.yml:409-410), after `curl`ing the
   release's `install.sh` to a stable path. Document `INSTALL_SH_PATH` in
   runbook §1.1. Add a readiness-adjacent check (`/api/health/background` or a
   startup log) that reports whether install.sh resolved, so the next drop is
   loud, not a per-deploy soft-fail.
2. **[P1 — F13] Commit the `alethedash-cs` systemd unit** (or a
   `systemd/alethedash-cs.service` + deploy-time `install -m644`) pinning
   `User=`, `WorkingDirectory=/opt/alethedash-cs`, `EnvironmentFile=`, and an
   explicit `Environment=HOME=...` (or `PATH=` extension for snap gcloud). This
   is the structural fix that prevents the whole class.
3. **[P1 — F2] Protect the credential key:** deploy-time assert that
   `DASHBOARD_CREDENTIAL_KEY` exists in `/etc/alethedash-cs.env` (fail the
   deploy loudly if missing — the app will fail-closed anyway, but before the
   old build is already stopped); document that the Rust key-file fallback is
   gone.
4. **[P1 — F23] Stop lying on `POST /upgrade`:** either wire a real upgrade
   (agent self-update command or SSH re-install) or set
   `status_message="Upgrade not supported on this control plane"` + 501.
5. **[P1 — F24] `POST /api/update/*` → 501** (self-update is the release
   pipeline's job now) or implement tarball-aware self-update.
6. **[P2 — F3] Replace `GetEnvironmentVariable("HOME") ?? ""` with
   `Environment.GetFolderPath(SpecialFolder.UserProfile)`** in
   CliComputeProvisioner + TesterPrecheckEndpoints (3 sites).
7. **[P2 — F12] Add `GCLOUD_CMD`/`AWS_CMD` overrides** (parity with `AZ_CMD`)
   and log the resolved absolute path at startup for each cloud CLI found.
8. **[P2 — F34] Fix `vite.config.ts` proxy** to `localhost:5030` (or
   `process.env.VITE_API_TARGET`).
9. **[P2 — F25/F27/F28/F29] Un-stub the CLI-backed validations** now that the
   prod host provably has `az` (the deploy itself runs on it): cloud-account
   validate, inventory scan, bench-token keyvault ops, precheck extras — or
   make each return an explicit `"not_checked"` status instead of
   `valid`/empty.
10. **[P2 — F26] Wire `IEmailSender`** into invites, password reset, and (at
    minimum) the inactivity warning before the 90-day suspend path fires again.
11. **[P3 — docs] Runbook §1.1/§8 refresh:** add the F14-F21 env-var table,
    mark §8.3 (detect-languages) as ported, and note the intentionally dropped
    Rust vars (F22).

## Verified-safe (no action)

Embedded apibench.json (F5); embedded cloud-init templates (F6); tester binary
discovery on prod (F7) and on agent VMs (F8); temp-file usage (F10); static
serving via nginx + raw-WS route parity (F33); `ssh_bootstrap` honest 400 with
no callers (F31); SP-credential az paths already isolated via temp
`AZURE_CONFIG_DIR` (CliComputeProvisioner.cs:302, OrphanReaperService.cs:448).
