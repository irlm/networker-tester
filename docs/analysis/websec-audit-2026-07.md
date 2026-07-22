# Web Security Audit — Networker.ControlPlane (2026-07)

**Scope:** READ-ONLY best-practices audit of the C# control plane
(`src/Networker.ControlPlane`, prod `laghound.com` behind nginx, ASP.NET
Minimal APIs + raw-WS hubs, .NET 10). Judged against OWASP Top 10 (2021) and
current recommended practice. No code was modified.

**Method:** static review of the auth stack (`Auth/`, `Sso/`), every endpoint
module (`Endpoints/`), the provisioning shell-outs (`Provisioning/`), the raw
SQL sites, the crypto (`Networker.Security`), the deploy/nginx config
(`scripts/deploy-dashboard.sh`, `deploy/*.service`), and the CI dependency
gates.

---

## Severity summary

| Severity | Count |
|----------|-------|
| P0 (critical) | 0 |
| P1 (high) | 3 |
| P2 (medium) | 6 |
| P3 (low / hardening) | 5 |

Overall posture is **good**. The auth model is disciplined (fail-closed secret,
DB-fresh role re-check on every request, constant-time credential compares,
no-existence-oracle 404s, single-use hashed tokens), all SQL is parameterized,
and every shell-out uses argv arrays (no shell string). The material gaps are at
the *edges*: transport hardening lives entirely in nginx (and is thin), there is
no login rate-limit, and the agent api-key rides the WS query string.

---

## Findings

### P1-1 — Agent api-key transmitted in the WebSocket URL query string
**OWASP:** A09 Security Logging & Monitoring Failures / A04 Insecure Design
**Files:** `Realtime/RawWs/AgentSocketEndpoint.cs:38-41,71`
(`/ws/agent?key=<api_key>`); nginx config `scripts/deploy-dashboard.sh:396-412`.

Fielded agents authenticate by passing their plaintext api-key as the `?key=`
query parameter on the WS upgrade URL. Query strings are the canonical place
secrets get *logged*: nginx's default `combined`/`main` log format records
`$request` (the full request line **including** the query string), so every
agent connection writes the live api-key into `/var/log/nginx/access.log` in
cleartext. The same value can surface in the proxy's error log, any upstream
APM, and browser/proxy history for the browser hubs (`/ws/dashboard?token=`,
`/ws/testers?token=` carry the *user JWT* the same way —
`Auth/AuthExtensions.cs:82-85`, `BrowserSocketEndpoint.cs:11-12`).

Mitigating facts: keys are 48-char CSPRNG (~285 bits, `TesterCreateLogic.cs:249`),
stored only as SHA-256 (`Security/AgentApiKeys.cs`), compared in constant time,
and the deploy config does **not** add a custom log_format — but it also never
disables the default access log, so the leak is live in prod. TLS protects the
value on the wire; the exposure is at-rest in logs on the control-plane host.

**Recommendation:** move the credential to the `Sec-WebSocket-Protocol` header
or a standard `Authorization` header (the browser can't set arbitrary headers on
`new WebSocket`, but agents are tungstenite clients that can). If the query
param must stay for wire-compat during the cutover window, add an nginx
`log_format` that strips `key`/`token`/`access_token` from the logged URI (or
`set $loggable_uri` with a regex) and rotate any keys already captured. See the
verdict section below.

---

### P1-2 — No rate limiting on `POST /api/auth/login` (password brute-force)
**OWASP:** A07 Identification & Authentication Failures
**File:** `Auth/AuthExtensions.cs:126-171`.

The login endpoint does an unbounded bcrypt verify per request with no per-IP or
per-account throttle, no lockout, and no CAPTCHA. Contrast the agent auth path,
which *does* have a per-IP sliding-window limiter (`AgentAuthLimiter.cs`, 10
failures / 5 min → 429). The same protection was never extended to the
human-facing login, forgot-password, reset-password, or SSO-exchange endpoints.
An attacker can credential-stuff `laghound.com/api/auth/login` at line rate; the
only backstop is bcrypt's cost factor. `forgot-password`
(`AccountEndpoints.cs:91`) is additionally an unlimited outbound-email trigger
once email delivery is wired.

**Recommendation:** add ASP.NET's `AddRateLimiter` (a fixed/sliding window keyed
on client IP + a stricter one keyed on the submitted email) to `/api/auth/login`,
`/api/auth/forgot-password`, `/api/auth/reset-password`, and
`/api/auth/sso/exchange`. Reuse the `X-Real-IP` resolution already in
`AgentSocketEndpoint.ResolveClientIp`. Consider progressive backoff / temporary
account lockout after N failures.

---

### P1-3 — Transport hardening (HSTS + security headers) is absent
**OWASP:** A05 Security Misconfiguration
**Files:** `Program.cs` (no `UseHsts`/`UseHttpsRedirection`/header middleware);
nginx `scripts/deploy-dashboard.sh:396-416`.

Neither layer emits the standard security headers. The app pipeline has no
`UseHsts()`, no security-header middleware, and no CORS policy (see P2-4). The
nginx server block terminates TLS via certbot `--redirect` (HTTP→HTTPS is
handled) but adds **none** of:

- `Strict-Transport-Security` (no HSTS → SSL-strip / downgrade window on first
  visit; no `preload`)
- `Content-Security-Policy` (the dashboard is a data-dense SPA — a CSP would
  materially cut XSS blast radius)
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY` / `frame-ancestors` (clickjacking)
- `Referrer-Policy`

**Recommendation:** add these as nginx `add_header` directives on the TLS server
block (single source, applies to static SPA + proxied API alike), or as an
ASP.NET header middleware if you want them co-located with the app. HSTS with a
long max-age + `includeSubDomains` is the highest-value one.

---

### P2-1 — SSO CSRF `state` cookie is not `Secure` when `DASHBOARD_PUBLIC_URL` is non-HTTPS; state has no server-side binding beyond the cookie
**OWASP:** A01 Broken Access Control (CSRF) / A05
**Files:** `Sso/OidcFlowService.cs:134-145`, `Sso/SsoFlowEndpoints.cs:100,133-138`.

The OIDC state cookie is `HttpOnly; SameSite=Lax; Path=/; Max-Age=300` and adds
`Secure` **only** when the public URL starts with `https://`. That's correct for
prod, but the flag is config-derived rather than unconditional, so a
misconfigured `DASHBOARD_PUBLIC_URL` silently drops `Secure`. The CSRF defense
itself is sound (state echoed by provider must equal the cookie —
`StateMatchesCookie`), and the provider-id is parsed back out of state; this is a
robustness note, not a live break in the HTTPS prod config.

**Recommendation:** set `Secure` unconditionally (the flow only ever runs over
HTTPS in any real deployment), or fail-closed if `DASHBOARD_PUBLIC_URL` is not
HTTPS outside Development.

---

### P2-2 — Password / reset-token policy is weak by modern standards
**OWASP:** A07
**Files:** `Sso/AccountSecurity.cs:30-82`, `Endpoints/AccountEndpoints.cs`,
`Endpoints/InvitesEndpoints.cs:267-269`.

Minimum password length is **8 characters** with no complexity, no breached-
password check, and no upper bound (bcrypt silently truncates at 72 bytes — a
very long passphrase is quietly cut, though not a vuln). NIST 800-63B recommends
≥8 but explicitly pairs that with a breach-corpus check, which is absent. The
positives are strong: reset tokens are 64 alphanumeric CSPRNG chars, stored
SHA-256-hashed, 1-hour single-use expiry (`AccountSecurity`), and
`forgot-password` always returns `{sent:true}` (no user-enumeration oracle,
`AccountEndpoints.cs:90-134`).

**Recommendation:** raise the floor to 12, add a Have-I-Been-Pwned k-anonymity
check (or a local top-N breached list) on set/reset, and reject passwords >72
bytes explicitly rather than truncating.

---

### P2-3 — JWT lifetime is 24h with no revocation and no refresh-token rotation
**OWASP:** A07
**File:** `Auth/JwtTokenService.cs:33` (`TokenTtlSeconds = 24*3600`), 24h TTL,
HS256, no `jti`, no denylist.

Sessions are stateless 24-hour bearer JWTs. There is no refresh-token flow and
no server-side revocation list — a stolen token is valid for up to 24h. This is
**strongly mitigated** by `UserStatusMiddleware` (`Auth/UserStatusMiddleware.cs`),
which re-reads the `dash_user` row on every request (10s cache) and fails closed
if the user is deleted/disabled/pending, and lets the *fresh DB* role override
the token claim. So a disabled/deleted account is cut within ~10s regardless of
token lifetime — that closes the worst case. What remains uncovered: a token
stolen from an account that stays *active* is replayable for the full 24h (no
per-session revoke, no "log out everywhere").

**Recommendation:** acceptable given the DB-fresh middleware, but consider
shortening the access-token TTL (e.g. 1h) with a refresh token, adding a `jti` +
a small revocation set for explicit logout, or a per-user `token_epoch` column
the middleware already-in-place could compare cheaply.

---

### P2-4 — No explicit CORS policy (implicit same-origin only)
**OWASP:** A05
**Files:** `Program.cs` (no `AddCors`/`UseCors`), confirmed absent repo-wide.

There is no CORS configuration at all. In prod nginx serves the SPA and proxies
the API under the same origin, so browsers never send cross-origin requests and
the *absence* of CORS is effectively deny-by-default (this is correct and safe).
Flagging it only so it stays intentional: if a second origin (mobile app, admin
console, partner) is ever added, the safe default silently blocks it and someone
may be tempted to reach for `AllowAnyOrigin()`.

**Recommendation:** keep it absent; if cross-origin is ever needed, add an
explicit allow-list policy, never `AllowAnyOrigin` with credentials.

---

### P2-5 — WS agent frame cap is 64 MiB; browser/tester hubs authenticate but large frames are buffered
**OWASP:** A04 Insecure Design (resource exhaustion)
**File:** `Realtime/RawWs/AgentSocketConnection.cs:38-41` (`MaxMessageBytes =
64 * 1024 * 1024`).

Each authenticated agent socket will assemble inbound messages up to 64 MiB
(matching the Rust hub's `max_message_size`). That is per-connection buffered
memory; a compromised/misbehaving agent (already past api-key auth) can pin tens
of MiB. Bounded and auth-gated, so low likelihood, but worth a ceiling review.

**Recommendation:** confirm 64 MiB is actually required by the largest benchmark
artifact frame; if not, lower it. Ensure the idle-timeout (120s) + tree limits
bound total concurrent buffered memory.

---

### P2-6 — No request-body size limits declared on JSON endpoints
**OWASP:** A04
**Files:** endpoint modules generally; e.g. `InvitesEndpoints.cs:194`
(`ReadFromJsonAsync`), all `MapPost` handlers.

Kestrel's default `MaxRequestBodySize` (~28.6 MiB) applies, but no endpoint
tightens it for small-payload routes (login, invites, config writes). Combined
with no auth rate-limit (P1-2), a large-body POST flood to unauthenticated
routes is a cheap DoS vector.

**Recommendation:** set a small per-endpoint body limit on the auth + public
routes (a few KiB), or a global sane default via `RequestSizeLimit`.

---

### P3-1 — Dev-fallback JWT secret is a hardcoded constant
**OWASP:** A05 / A02
**File:** `Auth/AuthExtensions.cs:29-53`.

Outside Development the app **fails closed** if `DASHBOARD_JWT_SECRET` is unset
(`AuthExtensions.cs:35-42`) — this is the correct, strong behavior and a
highlighted **strength**. The residual note: the Development fallback is a
literal `"dev-insecure-jwt-secret-change-me-please-32b"` in source. An operator
who runs prod with `ASPNETCORE_ENVIRONMENT=Development` (or unset → treated as
Production, so safe) would get it. Risk is low because unset env ⇒ Production ⇒
throw. Keep the console warning; consider generating an ephemeral random dev
secret per boot instead of a constant so it can never be a known value.

---

### P3-2 — Short prod JWT secret accepted (raw-HMAC bypasses the 256-bit floor)
**OWASP:** A02 Cryptographic Failures
**File:** `Auth/JwtTokenService.cs:21-28,63-67`.

To stay byte-compatible with the Rust `jsonwebtoken` crate, the service signs/
verifies with `HMACSHA256` directly and *deliberately* bypasses
Microsoft.IdentityModel's ≥256-bit key enforcement (the comment documents a live
29-byte prod secret). HMAC-SHA256 with a 29-byte (232-bit) secret is still
comfortably strong, so this is not a break — but it removes the guardrail that
would catch a genuinely short/weak secret. The deploy script generates
`openssl rand -base64 32` (`scripts/deploy-dashboard.sh:104`), so fresh installs
are fine; legacy ones may carry the 29-byte value.

**Recommendation:** add an explicit minimum-length assertion (e.g. ≥32 bytes) in
`JwtTokenService`'s constructor for non-Development, so the raw-HMAC path can't
silently accept a truly short secret. Rotate the known 29-byte prod secret to a
full 32-byte one once the Rust dashboard is decommissioned (removing the
compat constraint).

---

### P3-3 — cargo-audit ignore-list carries 5 RUSTSEC advisories
**OWASP:** A06 Vulnerable & Outdated Components
**File:** `.github/workflows/rust-audit.yml:85-118`.

The ignore-list is **well-justified and documented** (a strength — each entry has
a rationale + exit plan):
- `RUSTSEC-2023-0071` (rsa Marvin timing) — via `jsonwebtoken`→`rsa`; **no RSA
  op is performed** (HS256 only), not exploitable; removed when the Rust
  dashboard is decommissioned (~2026-07-30).
- `RUSTSEC-2026-0098/-0099/-0104` (rustls-webpki) — the modern rustls 0.23 stack
  already uses patched 0.103.13; the vulnerable 0.101.7 survives only via the
  opt-in `db-mssql` tiberius backend, exposed only on TLS to the user's own SQL
  Server.
- `RUSTSEC-2025-0134` (rustls-pemfile unmaintained) — informational, same
  tiberius dead-end.

These affect the **retired Rust crates**, not the C# control plane in scope. The
C# side is gated separately by `dotnet list package --vulnerable`
(`dotnet.yml:87-105`, fails on any vulnerable transitive in `src/`) and npm audit
`--audit-level=high` (`ci.yml:396`) for the SPA.

**Recommendation:** none urgent — revisit after the 2026-07-30 Rust
decommission, which clears 4 of the 5 outright.

---

### P3-4 — `X-Real-IP` / `X-Forwarded-For` trusted without a trusted-proxy allow-list
**OWASP:** A04
**File:** `Realtime/RawWs/AgentSocketEndpoint.cs:194-213`.

The client-IP resolver prefers `X-Real-IP` then the left-most `X-Forwarded-For`
hop. The comment correctly notes nginx overwrites `X-Real-IP` (so a client can't
spoof it *when behind nginx*), but there is no `ForwardedHeaders` middleware with
a `KnownProxies` allow-list, and `X-Forwarded-For`'s left-most value **is**
client-controlled. If the app were ever reachable directly (not via nginx), an
attacker could spoof the IP to evade the `AgentAuthLimiter` per-IP bucket
(defeating P1-2's cousin) or poison `api_key_last_used_ip`.

**Recommendation:** register `UseForwardedHeaders` with `KnownProxies`/
`KnownNetworks` set to the nginx loopback, and only trust the headers from it.

---

### P3-5 — Error envelope is clean, but plain-text 4xx bodies echo internal-ish messages
**OWASP:** A05 (info exposure — low)
**Files:** `ErrorEnvelope.cs` (strength), `Auth/UserStatusMiddleware.cs:97-102`,
`Endpoints/AccountEndpoints.cs:178-179`.

The global 500 handler is a **strength**: it returns a fixed
`{"error":"internal server error"}` and logs the real exception server-side —
**no stack traces leak** (`ErrorEnvelope.cs:19-40`). Minor note: some 4xx paths
return descriptive plain-text (`"pending_approval"`, `"Password change required
before accessing this resource"`, `"SSO accounts cannot change password here"`).
These are intentional (the SPA surfaces them verbatim) and reveal nothing
sensitive, but they do disclose account-state internals. Acceptable; documented
for completeness.

---

## Strengths (called out explicitly)

1. **Fail-closed JWT secret** outside Development (`AuthExtensions.cs:35-42`) —
   refuses to boot with a built-in key; unset env treated as Production.
2. **DB-fresh authorization on every request** (`UserStatusMiddleware.cs`) — the
   JWT claims are never trusted alone; deleted/disabled/pending users are cut
   within ~10s, role + `is_platform_admin` re-read from the DB. Negative caching
   blunts forged-token storms.
3. **No-existence-oracle 404s** — `ProjectAccessChecker` maps no-access to 404,
   not 403 (`ProjectAccessChecker.cs:13-24`); invites/share-links return the same
   404 for expired/revoked/consumed/unknown (`InvitesEndpoints.cs:148-161`).
4. **All SQL is parameterized.** Every raw-Npgsql site uses positional `$N` +
   bound `NpgsqlParameter` values (`LogsEndpoints.cs`, `PerfLogEndpoints.cs`,
   `UrlTestsEndpoints.cs`); the only interpolations are constant clause fragments
   and integer indexes (`$"kind = ${idx++}"`) and compile-time-constant intervals
   (`TesterRecovery.cs:134`, `StuckThresholdMinutes` is a const int) — **no user
   value ever reaches a query string.** ILIKE inputs are escaped
   (`LogsEndpoints.cs:209`, `PerfLogEndpoints.cs` `EscapeIlike`). EF Core LINQ
   everywhere else.
5. **No command injection in shell-outs.** `az`/`aws`/`gcloud`/`ssh`/`bash
   install.sh` all use `ProcessStartInfo.ArgumentList` (argv, no shell),
   `UseShellExecute=false`, tree-kill timeouts, stdin nulled
   (`CliComputeProvisioner.cs:1306-1316`, `SshLanguageDetector.cs:120-131`,
   `DeployRunner.cs:178-197`). Cloud-init bootstrap inputs (api-key, URL, target
   triple) are validated against strict allow-list regexes before templating
   (`CloudInitScripts.cs:34-50`, api-key `^[A-Za-z0-9]{32,128}$`).
6. **Constant-time credential comparison** for both JWT signature
   (`JwtTokenService.cs:133`, `CryptographicOperations.FixedTimeEquals`) and
   agent api-key hash (`AgentApiKeys.cs:37-47`).
7. **Secrets hashed at rest** — agent keys and all collab/invite/reset tokens
   stored SHA-256 only; cloud/SSO secrets AES-256-GCM with rotation fallback
   (`Networker.Security/CredentialCipher.cs`).
8. **Agent-auth brute-force limiter** (`AgentAuthLimiter.cs`) — per-IP sliding
   window, short-circuits before DB work, clears on success.
9. **Single-use, short-lived tokens** — SSO exchange code 2-min single-use
   (`SsoExchangeCodeCache.cs`), reset token 1h single-use, invites single-use
   (`status='pending'`→`'accepted'`).
10. **Admin routes consistently gated** `RequireAuthorization(GlobalAdmin)`;
    platform-admin is an *elevation* (implicit project Admin) not an auth bypass —
    it still passes through `UserStatusMiddleware`, so a disabled platform admin
    is cut (`ProjectAccessChecker.cs:79-93`).
11. **Vulnerable-dependency CI gates** on all three stacks (cargo-audit,
    `dotnet list --vulnerable`, npm audit) with a documented, justified Rust
    ignore-list.

### AuthZ route coverage note
Every partial-class tester write route (`TesterWriteEndpoints.*.cs`, which show
0 `RequireAuthorization` in their own files) is actually wired with the correct
policy in the central `TesterWriteEndpoints.cs:50-80` (`ProjectOperator` /
`ProjectAdmin`). The public `AllowAnonymous` routes were each reviewed and are
intentional: invite/share resolve + accept, forgot/reset password, login, and
the SSO login flow — all either resolve hashed single-use tokens or are the
login surface itself. The stub `LeaderboardEndpoints` returns empty arrays (no
data exposure). **No mutating route was found missing authorization.**

---

## Top findings (priority order)

1. **P1-1 api-key-in-query-string** → leaks into nginx access logs (verdict
   below).
2. **P1-2 no login rate-limit** → credential-stuffing / brute-force on the human
   login surface, while the agent path is already protected.
3. **P1-3 missing HSTS + security headers** → SSL-strip / clickjacking / no CSP
   defense-in-depth.

## Verdict: api_key-in-query-string

**Confirmed anti-pattern, real exposure, fix it — but not a P0.** Passing the
agent api-key (and the browser/tester JWTs) as `?key=` / `?token=` /
`?access_token=` puts a live credential into the one place credentials reliably
get logged. nginx's default access log records the full request URI, so
`/ws/agent?key=<48-char-key>` is being written to disk in cleartext on the
control-plane host on every agent connect, and can propagate to error logs and
any log shipper. It is **not** exposed on the wire (TLS) and the keys are
high-entropy, hashed at rest, and constant-time-compared — which is why this
lands P1, not P0. The clean fix is to move the credential to a header
(`Sec-WebSocket-Protocol` for browsers, `Authorization` for the tungstenite
agents); the pragmatic interim fix is an nginx `log_format` that redacts
`key`/`token`/`access_token` from the logged URI plus rotation of any keys
already captured in existing logs. This is a known constraint carried from the
Rust dashboard for wire-compat; retiring that compatibility requirement (post
2026-07-30 decommission) is the natural moment to move it to a header.
