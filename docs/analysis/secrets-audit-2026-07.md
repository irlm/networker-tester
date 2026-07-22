# Secrets & Token Protection Audit — July 2026

Read-only audit of every secret the system stores, transmits, or generates.
Scope: C# control plane / agent / data layer (production stack), provisioning
scripts, deploy pipeline, tester CLI seam, frontend token storage, and a
repo-wide committed-secret sweep. Retired Rust crates checked only where their
artifacts (deploy scripts, shared DB columns) still matter.

Verdict scale per axis: **OK** (verified sound), **COND** (sound but depends on
an out-of-repo control — noted), **GAP** (finding filed below).

---

## 1. Secret inventory — verdict table

| # | Secret | At rest | In responses | In logs | In transit | Entropy | Findings |
|---|--------|---------|--------------|---------|------------|---------|----------|
| 1 | Cloud-account credentials (`cloud_account.credentials_enc/nonce`) | **OK** AES-256-GCM | **OK** never serialized | **OK** | **OK** | **OK** key = `openssl rand -hex 32` | — |
| 2 | SDK-endpoint LagHound token (`test_config.token_enc/token_nonce`, V043) | **OK** AES-256-GCM | **OK** write-only, masked | **OK** redacted end-to-end | **OK** (COND: wss via nginx) | n/a (customer-supplied) | P2-4 (generic-surface bypass) |
| 3 | Agent api-key (`agent.api_key` + `api_key_hash`, V040/V044) | **GAP** plaintext column still written | **OK** (rotate returns once) | **OK** app logs / **COND** proxy logs (query-string) | **COND** `?key=` in URL | **OK** 48-char CSPRNG (~285 bits) | **P1-1**, **P1-3** |
| 4 | Alert webhook HMAC secret (`alert_channel.config`) | **GAP** plaintext JSONB | **OK** masked (`********`) | **OK** | **OK** | n/a (user-supplied) | **P1-2** |
| 5 | JWT signing secret (`DASHBOARD_JWT_SECRET`) | **OK** env file, fail-closed | **OK** | **OK** | n/a | **GAP** no min length enforced | P2-1 |
| 6 | User passwords (`dash_user.password_hash`) | **OK** bcrypt (lib default cost 11) | **OK** | **OK** | **OK** POST body | user-supplied, min 8 | — |
| 7 | Password-reset / invite / share tokens | **OK** SHA-256 hash only | **OK** raw shown once | **OK** deliberately not logged | **OK** | **OK** 64-char / 32-byte CSPRNG | — |
| 8 | SSO OIDC client secret (`sso_provider.client_secret_enc/nonce`) | **OK** AES-256-GCM | **OK** `has_client_secret` only | **OK** generic error codes | **OK** HTTPS token exchange | provider-supplied | — |
| 9 | Provisioning secrets (bootstrap api-key, Windows admin password, az/gcloud creds) | **COND** VM-local exposure | **OK** | **OK** redacted | **OK** cloud APIs | **GAP** Win password from `Guid.NewGuid` | P2-2, P2-3, P2-5 |
| 10 | Credential-cipher key (`DASHBOARD_CREDENTIAL_KEY`) | **OK** env file, fail-closed | **OK** | **OK** | n/a | **OK** 256-bit | — |
| 11 | Frontend JWT (browser) | **GAP** `localStorage` | — | — | **OK** | — | P2-6 |
| 12 | Committed key material (repo sweep) | one benchmark TLS key, by design | — | — | — | — | P2-7 |

**No P0 (actively leaking / logged plaintext secret) was found.** Three P1s and
seven P2s below.

---

## 2. Per-secret analysis

### 2.1 Cloud provider credentials — SOUND

- **Scheme** — `src/Networker.Security/CredentialCipher.cs`: AES-256-GCM,
  32-byte key, 12-byte random nonce per encryption
  (`RandomNumberGenerator.Fill`, line 84-85), 16-byte tag, `ciphertext||tag`
  blob + nonce in separate columns, no AAD, decrypt-time old-key rotation
  fallback (lines 111-132). Byte-compatible with the Rust scheme it replaced.
  Random 96-bit nonces are safe at this write volume (NIST bound ~2^32
  encryptions per key; this table holds tens of rows).
- **Key handling** — `src/Networker.ControlPlane/Security/CredentialCipherExtensions.cs`:
  `DASHBOARD_CREDENTIAL_KEY` (64 hex chars) with **fail-closed** startup outside
  Development for both missing (line 84-92) and invalid (line 63-72) keys;
  unset `ASPNETCORE_ENVIRONMENT` is treated as Production. Dev fallback is an
  ephemeral in-memory key, never persisted. `DASHBOARD_CREDENTIAL_KEY_OLD` is
  decrypt-only rotation fallback. There is **no key-file fallback** in the C#
  path (the Rust `credential.key` file mechanism was not ported) — env/env-file
  only, which is fine given the deploy guard below.
- **Prod key custody** — `.github/workflows/release.yml` (lines ~415-430):
  deploy refuses to proceed if `/etc/alethedash-cs.env` lacks
  `DASHBOARD_CREDENTIAL_KEY` / `DASHBOARD_JWT_SECRET`; it never auto-generates.
  The env file is read via `sudo grep` (root-only perms implied — see COND
  note in §3). Unit `deploy/alethedash-cs.service` loads it via
  `EnvironmentFile=/etc/alethedash-cs.env` (line 36).
- **API exposure** — `CloudAccountsEndpoints.cs`: list/detail project to
  `AccountSummaryDto` (lines 46-54, 149-151, 406-414) — **no credential
  fields**. Create/update accept credentials inbound only; update
  merge-then-reencrypt (lines 196-218). Validate returns only
  `status`/`validation_error` (line 299).
- **Decryption call sites** (all verified non-logging):
  `CloudAccountsEndpoints.cs:342`, `TesterPrecheckEndpoints.cs:82` (decrypt
  failure → generic `decrypt_failed` blocker, line 85-95),
  `TesterWriteEndpoints.Create.cs:591`, `OrphanReaperService.cs:290` (decrypt
  failure → silent skip), `RunDispatcher.cs:728`, `SsoFlowEndpoints.cs:159`.
  Grep of `Log*`/`Console.Write*` across the control plane found **no
  statement interpolating decrypted material** — logs carry IDs, exception
  *type names* (`RunDispatcher.cs:737`), or counts only.
- **In transit to providers** — decrypted values go to `az`/`aws`/`gcloud`
  argv/env locally (see P2-5), then to provider APIs over TLS.

### 2.2 SDK-endpoint LagHound token — SOUND (one bypass, P2-4)

- **At rest** — `SdkEndpointsEndpoints.cs:92`: encrypted with the same
  `CredentialCipher` into `token_enc`/`token_nonce`
  (`src/Networker.Data/Migrations/V043_sdk_endpoint_token.sql`;
  `TestConfig.cs:41-43`).
- **Read masking** — `ToDto` (lines 265-286): token **never** returned; only
  `token_set` + the `********` mask. Both generic config readers
  (`TestConfigsEndpoints.cs:83`, `TestConfigWriteEndpoints.cs:355`) serialize
  the workload column, which never contains the token (it lives only in the
  bytea columns).
- **Dispatch splice** — `RunDispatcher.cs:596-762`: token is decrypted only
  into a **wire clone** of the workload (`WithLagHoundToken`, line 723); the
  stored row is untouched. Decrypt failure logs only the exception type
  (line 734-737). Defense-in-depth: run/config/agent project identity is
  re-asserted before splicing (lines 610-624); a mismatch ships the run
  without the token and the error log contains IDs only.
- **Agent → tester** — `Networker.Agent/RunExecutor.cs:395-407` passes
  `--laghound-token <value>` on the tester argv; the spawn log line is
  redacted via `RedactSecretArgs` (lines 430-453, masks values after
  `--laghound-token` and `--bearer-token`). The tester
  (`crates/networker-tester/src/runner/http.rs:855-861`) only places the token
  in the `x-laghound-token` / `authorization` request headers; it never
  appears in summary/metrics/artifact output (verified by grep of
  `summary.rs`, `metrics.rs`, artifact builders).
- **Residual exposure** — the plaintext token is momentarily visible in
  `/proc/<pid>/cmdline` of the tester process on the (single-tenant) probe VM.
  Acceptable; noted for completeness.

### 2.3 Agent api-key — mostly sound; two findings

- **Entropy** — `TesterCreateLogic.cs:249-251`: 48 chars from a 62-char
  alphabet via `RandomNumberGenerator.GetItems` → ~285 bits, CSPRNG. OK.
- **Hash-at-rest & auth** — `Security/AgentApiKeys.cs`: lowercase-hex SHA-256
  (appropriate for a ~285-bit machine credential; a slow KDF is unnecessary)
  + `CryptographicOperations.FixedTimeEquals` re-verify.
  `RawWs/AgentMessageProcessor.cs:146-176`: lookup keyed on `api_key_hash`
  (never the plaintext column), constant-time re-check, V044 expiry check.
  `AgentSocketEndpoint.cs:71-110`: per-IP brute-force limiter (429) before any
  DB work; rejection logs say only "no api key" / "unknown or expired api key"
  (line 96-98) — the presented key is never logged. The agent client logs the
  **base** URL, not the keyed URI (`RawWebSocketClient.cs:62-63` vs
  `BuildUri` line 205).
- **Rotate** — `TesterWriteEndpoints.RotateKey.cs`: project-scoped (flat 404
  for foreign testers), new plaintext returned exactly once (line 81-87), old
  key instantly dead via hash replacement, live WS dropped, audit log carries
  IDs only (lines 75-78). Read surfaces never expose it
  (`AgentsEndpoints.cs:14,22,161`).
- **GAP (P1-1)** — the **plaintext `agent.api_key` column is still populated**
  on both mint (`TesterWriteEndpoints.Create.cs:298-303`) and rotate
  (`RotateKey.cs:66`), and `V040_agent_api_key_hash.sql` explicitly promises
  "api_key is dropped by a later migration once the fleet is verified" — that
  migration does not exist through V044. The RotateKey doc-comment
  ("never stored/re-shown", line 33) is inaccurate while the column persists.
- **GAP (P1-3)** — the key travels as a **URL query parameter**
  (`GET /ws/agent?key=...`, `AgentSocketEndpoint.cs:40-41,71`;
  `AgentProtocolHub.cs`). In-app exposure is controlled
  (`appsettings.json` sets `Microsoft.AspNetCore: Warning`, which suppresses
  the hosting request-start line that would include the query string; the 500
  envelope logs `Request.Path` only, `ErrorEnvelope.cs:36-37`), but in prod
  the request line **transits nginx**, whose default `access_log` records the
  full URI including `?key=`. The nginx config is hand-managed VM state (not
  in-repo), so this cannot be verified from the repository. The same applies
  to the SignalR `?access_token=<JWT>` fallback for `/ws/*`
  (`AuthExtensions.cs:82-85`).

### 2.4 Alert webhook HMAC secret — GAP (P1-2)

- **At rest** — the secret is stored **plaintext** inside the
  `alert_channel.config` JSONB: `AlertsEndpoints.cs:485-487`
  (`JsonSerializer.Serialize(new { url, secret })`). Entity doc confirms the
  shape (`Entities/AlertChannel.cs:17-22`). This is inconsistent with the SDK
  token and SSO client secret, which use `CredentialCipher` for exactly this
  class of per-project shared secret.
- **Read masking** — OK: `MaskedConfig` (`AlertsEndpoints.cs:521-537`)
  replaces `secret` with `********` in every channel DTO; PATCH round-trip of
  the mask preserves the stored value (lines 445-479).
- **Signing path** — OK: `Alerting/AlertWebhook.cs:53-59` — HMAC-SHA256 over
  the exact serialized payload bytes, emitted as
  `X-Networker-Signature: sha256=<hex>`. `AlertNotifier.cs` reads the secret
  only to sign (lines 70-113); its logs carry channel IDs, never the secret
  or config (line 61).

### 2.5 JWT signing secret & session tokens

- **Fail-closed** — `AuthExtensions.cs:29-53`: missing `DASHBOARD_JWT_SECRET`
  outside Development throws at startup; unset environment = Production. The
  dev fallback constant is obviously non-secret. Deploy guard refuses to run
  without it (release.yml, §2.1 above). Never logged anywhere (grep clean);
  `cli_smoke.sh:308` uses an obvious placeholder.
- **GAP (P2-1)** — `JwtTokenService.cs` was deliberately built to accept
  **any secret length** via raw HMAC (doc lines 21-28 record a live 29-byte
  prod secret). Nothing enforces a ≥32-byte secret, so a future weak secret
  would be accepted silently. Given this repo already had one JWT-secret leak
  incident (rotated, #442), a startup length check (or at least a warning) is
  cheap insurance.
- **Frontend storage (P2-6)** — `dashboard/src/stores/authStore.ts:18-26`
  keeps the JWT in `localStorage` — exfiltratable by any XSS. 24 h TTL bounds
  the damage; httpOnly-cookie or memory+refresh would be stronger.

### 2.6 User passwords & reset/invite/share tokens — SOUND

- **Hashing** — bcrypt via BCrypt.Net (`AuthExtensions.cs:147` verify;
  `AccountEndpoints.cs:83,165`, `InvitesEndpoints.cs:276` hash) at the library
  default work factor (11). Malformed stored hash → treated as wrong password,
  never a 500 (`AuthExtensions.cs:145-153`). No password value ever reaches a
  log statement (grep across `src/` clean; the only "password" logs are
  metadata: `AccountEndpoints.cs:117-123`).
- **Reset tokens** — `Sso/AccountSecurity.cs`: 64-char CSPRNG alphanumeric
  (~380 bits, `RandomNumberGenerator.GetString`), stored **SHA-256-hashed**
  with 1 h expiry (`AccountEndpoints.cs:107-109`); forgot-password always
  returns `{sent:true}` (no user-existence oracle) and deliberately does not
  log the link/token (comment lines 112-119). Admin-created users:
  `UsersEndpoints.cs:91-101` — same pattern (64-char token, hash stored).
- **Invite / share / member tokens** — `CollabTokens`
  (`InvitesEndpoints.cs:418-425`): 32 random bytes URL-safe encoded, stored
  hashed (`InvitesEndpoints.cs:75,350`, `ShareLinksEndpoints.cs:67,186`,
  `MembersEndpoints.cs:384-391`); raw value returned once at mint. Share-link
  tokens appear in URLs by design but are hash-at-rest and revocable.

### 2.7 SSO OIDC client secret — SOUND

- Encrypted with `CredentialCipher` into `client_secret_enc/nonce`
  (`SsoAdminEndpoints.cs:88-98,186-198`; entity `SsoProvider.cs:16-18`);
  responses expose only `has_client_secret` (lines 332, 354-361). The callback
  flow decrypts it solely for the code-exchange POST to the IdP token endpoint
  over HTTPS (`SsoFlowEndpoints.cs:155-190`); every failure path returns a
  generic error code (`internal_error`, `token_exchange_failed`) with no
  secret/exception content.

### 2.8 Provisioning & bootstrap secrets

- **Bootstrap rendering** — `CloudInitScripts.cs`: api-key is
  whitelist-validated (`^[A-Za-z0-9]{32,128}$`, lines 34-52 — also blocks
  template/shell injection). The Linux template runs `set -x` with output to
  serial console + `/var/log/networker-bootstrap.log` (lines 159-162), but the
  key is only ever written inside a **quoted heredoc** (lines 271-287), whose
  body `set -x` does not trace — the key does not hit the bootstrap log. The
  control plane logs only `bootstrap_bytes`, never the script text
  (`TesterWriteEndpoints.Create.cs:347-350`).
- **P2-2 (VM-local exposure)** — the rendered key persists on the VM
  world-readable: Linux systemd unit
  `/etc/systemd/system/networker-agent.service` (default 0644,
  `Environment=AGENT_API_KEY=...`, template line 283; same for
  `TesterInstallScripts.cs:201`) and Windows **machine-wide** env var
  (`WindowsTemplate` → `SetEnvironmentVariable(..., 'Machine')`). Cloud
  user-data/custom-data is additionally readable from inside the VM via the
  instance metadata service by any local process (GCP/AWS), and via cloud
  control-plane APIs by anyone with VM read access. Mitigation: single-tenant,
  single-purpose VMs; key is per-agent and rotatable. Still, `chmod 600` on
  the unit or an `EnvironmentFile` would shrink this.
- **P2-3 (Windows admin password entropy)** —
  `CliComputeProvisioner.cs:543-545`: password is built from
  `Guid.NewGuid():N` plus fixed affixes. .NET v4 GUIDs carry ~122 random bits
  from the OS RNG in practice, but `Guid.NewGuid()` is **not documented as a
  CSPRNG**, and the fixed `Nx!`/`aZ9` scaffolding plus VM-name chars are
  structure, not entropy. Use `RandomNumberGenerator` like every other secret
  in this codebase. (The password is passed to `az` with `sensitiveArgs: true`
  so the arg line is redacted from logs, line 556-558/1329, and is never
  persisted or returned.)
- **P2-5 (local process/tempfile exposure during provisioning)** —
  `az login --service-principal ... -p <client_secret>` puts the decrypted
  secret on **argv** (`CliComputeProvisioner.cs:512`), visible in
  `/proc/*/cmdline` on the control-plane host for the duration of the call
  (app log redacted via `sensitiveArgs: true`, line 522/1329). The GCP
  `json_key` is written to a temp file with `File.WriteAllTextAsync`
  (line ~1123-1124) — created with default umask (typically 0644), unlike
  `Path.GetTempFileName` which is 0600; both are deleted in `finally`
  (`TryDeleteFile(keyFile)`), and the ephemeral `AZURE_CONFIG_DIR` is removed
  on both paths (lines 525, 627-629). Prefer `az login` via stdin/env and
  `UnixFileMode`-restricted key files.
- **Legacy `scripts/deploy-dashboard.sh` (retired Rust dashboard)** embeds
  `DB_PASSWORD`/`JWT_SECRET`/`CREDENTIAL_KEY`/`ADMIN_PASSWORD` inline in a
  world-readable systemd unit (lines ~104-105, 360-385) and generates
  `ADMIN_PASSWORD` at only `openssl rand -base64 12`. Off the release train;
  flagged so it is not resurrected as-is. The current C# prod path correctly
  uses `EnvironmentFile=/etc/alethedash-cs.env`.

### 2.9 Error responses, URLs, and repo sweep

- **500s never leak** — `ErrorEnvelope.cs`: fixed
  `{"error":"internal server error"}` body; real exception logged server-side
  with method+path only (no query string). 4xx bodies audited above are static
  strings or field names.
- **Secrets in URLs** — only the two WS query-string credentials (P1-3) and
  by-design share-link/reset-link tokens (hash-at-rest, expiring). No API
  accepts a secret via GET query.
- **Committed-secret sweep** — `git ls-files` + pattern greps
  (64-hex, `eyJhbGciOi`, `*SECRET*=` literals): no live secrets. Hits are
  SHA-256 test vectors (`AccountSecurityTests.cs:40` et al.), migration
  checksums (`SchemaMigrationTests.cs`), the smoke-test placeholder
  (`tests/cli_smoke.sh:308`), and **`benchmarks/shared/key.pem`** — a
  committed RSA private key for the benchmark reference APIs' self-signed TLS
  (P2-7: by design — targets "serve self-signed certificates by
  construction" — but worth a README note so scanners/humans don't burn time
  on it, and it must never be reused for a real endpoint).
- **`cloud_connection.config` (P2-8 → folded into P2 list as P2-4b)** —
  `CloudConnectionsEndpoints.cs:86,137`: stored plaintext and the **full
  config is returned to project admins** (`ToFullDto`, line 210-224). The
  schema intends identifier-only, managed-identity configs
  (`subscription_id`, `role_arn` — `ValidateConfigStub`), but nothing rejects
  a pasted `client_secret`. Admin-only read scoping mitigates.

---

## 3. Findings

### P0 — none

No plaintext-at-rest *credential-class* secret without masking, no secret in
any log statement, no committed live secret, no secret in an error response.

### P1

1. **`agent.api_key` plaintext column still written at rest.**
   `TesterWriteEndpoints.Create.cs:298-303`, `RotateKey.cs:66`;
   `V040_agent_api_key_hash.sql` promised the drop "once the fleet is
   verified" and V041-V044 never delivered it. Auth has been hash-only since
   V040 and rotation defaults to no-expiry, so the column is now pure
   liability: a DB read (backup, SQL injection, replica leak) yields every
   agent's live credential. Fix: verify fleet on hash-only auth, then ship
   `V045: ALTER TABLE agent DROP COLUMN api_key` and delete the entity
   property + both writers. (Also corrects the false "never stored" claim in
   `RotateKey.cs:33`.)
2. **Alert webhook HMAC secret stored plaintext** in `alert_channel.config`
   JSONB (`AlertsEndpoints.cs:485-487`). Masked on read and never logged, but
   it is the only user-supplied shared secret in the system *not* run through
   `CredentialCipher` (SDK token V043 and SSO secret both are). Fix: add
   `secret_enc`/`secret_nonce` columns (V043 pattern), decrypt only in
   `AlertNotifier`, migrate-and-null the JSON field.
3. **Agent api-key (and SignalR JWT) ride the URL query string** through
   nginx (`/ws/agent?key=...`, `AgentSocketEndpoint.cs:41`;
   `?access_token=<jwt>`, `AuthExtensions.cs:82-85`). In-app logging is safe
   (hosting logs at Warning; `ErrorEnvelope` logs path only), but nginx's
   default access log captures full request URIs, and the nginx config is
   unmanaged VM state that this repo cannot attest. Action: codify the nginx
   config (the F13 pattern already used for the systemd unit) with either
   `access_log off` for `/ws/` locations or a redacting log format; longer
   term, move agent auth to a header/first-frame (wire-protocol change —
   fielded Rust agents pin the `?key=` shape, so gate on fleet version).

### P2

1. **No minimum length/entropy check on `DASHBOARD_JWT_SECRET`** — the raw-HMAC
   path exists precisely to accept short secrets (`JwtTokenService.cs:21-28`;
   prod historically ran 29 bytes). Enforce ≥32 bytes outside Development, or
   log a loud startup warning.
2. **Agent api-key world-readable on tester VMs** — 0644 systemd unit
   (`CloudInitScripts.cs:271-287`, `TesterInstallScripts.cs:201`), Windows
   machine env var, and instance-metadata user-data. Single-tenant VMs
   mitigate; `install -m 600` + `EnvironmentFile` would close it.
3. **Windows VM admin password derives from `Guid.NewGuid()`**
   (`CliComputeProvisioner.cs:543-545`) — not a documented CSPRNG API and
   partially structured. Use `RandomNumberGenerator.GetItems` over the full
   Azure-allowed alphabet.
4. **Generic test-config surface can bypass the encrypted-token path** — a
   client that POSTs a workload containing `laghound_token` via
   `/api/projects/{p}/test-configs` (`TestConfigWriteEndpoints.cs:79,144`
   stores workload verbatim) gets it stored plaintext and echoed on every read
   (`TestConfigsEndpoints.cs:83`). Reject or strip `laghound_token` in generic
   workload writes. Related: `cloud_connection.config` accepts and returns
   arbitrary JSON to admins (`CloudConnectionsEndpoints.cs:86,210-224`) —
   document as identifier-only or encrypt.
5. **Provisioning-time local exposure on the control-plane host** — SP secret
   on `az login` argv (`CliComputeProvisioner.cs:512`; ps-visible), GCP key
   temp file written with default umask (line ~1123). Both short-lived and
   cleaned up; tighten with stdin login and `UnixFileMode.UserRead|UserWrite`.
6. **JWT in `localStorage`** (`dashboard/src/stores/authStore.ts:18-26`) —
   XSS-exfiltratable; 24 h TTL bounds it.
7. **Committed private key `benchmarks/shared/key.pem`** — intentional
   self-signed benchmark material; add a "test-only, never reuse" note and a
   scanner-ignore entry.

---

## 4. What is demonstrably done right

- One vetted cipher (`CredentialCipher`) reused for cloud creds, SDK tokens,
  and SSO secrets; correct GCM construction; rotation fallback; fail-closed
  key handling on **both** key env vars, with the deploy pipeline refusing to
  proceed rather than regenerate (`release.yml` secrets guard).
- Consistent write-only/mask-on-read discipline across every secret-bearing
  API surface (cloud accounts, SDK endpoints, alert channels, SSO providers,
  agents), with flat-404 anti-oracle behavior on foreign resources.
- Hash-at-rest for every bearer-style token (agent keys, reset, invite, share)
  with constant-time comparison and CSPRNG generation ≥256 bits everywhere
  except P2-3.
- Deliberate, commented log hygiene at every decrypt/splice/spawn site
  (`RunDispatcher.cs:596-609,732-737`, `RunExecutor.RedactSecretArgs`,
  `CliComputeProvisioner` `sensitiveArgs`), a non-leaking global 500 envelope,
  and per-IP brute-force limiting on agent auth.
