# nginx WebSocket access-log redaction

**Status: APPLIED LIVE on `alethedash-vm` (2026-07-22).** This document codifies
the change so a VM rebuild can re-apply it — the production nginx config is
VM-managed and not otherwise in this repo.

## Problem

Agents authenticate at `/ws/agent`; browsers/testers at
`/ws/dashboard?access_token=<jwt>` and `/ws/testers?token=<jwt>`. Since
v0.28.56 the C# agent sends its key in the `X-LagHound-Agent-Key` request
header, but the server still accepts the legacy `?key=<api_key>` query for
fielded agents that predate it — so for those clients (and for the browser
`?access_token=`/`?token=` JWTs) the credential is in the **query string** (a
wire-compat carryover from the retired Rust dashboard). nginx's default
`combined` log format logs `$request` — the full
request line including the query — so the live credential is written to
`/var/log/nginx/access.log` in cleartext on every WebSocket connect. Flagged P1
by the 2026-07 audit (`docs/analysis/websec-audit-2026-07.md`,
`docs/analysis/secrets-audit-2026-07.md`).

This is defence-in-depth: the credential is not exposed on the wire (TLS
terminates at nginx) and agent keys are 48-char CSPRNG, SHA-256-hashed at rest,
and constant-time compared — but a plaintext key on disk in the access log is
needless exposure.

## Fix (interim)

`deploy/nginx-ws-log-redaction.conf` defines a `ws_redacted` log format — the
standard `combined` layout but logging `$uri` (the request path, no query
string) instead of `$request`. Each `location /ws` block opts in. The IP /
timestamp / status / user-agent audit signal is preserved; only the query string
(and thus the secret) is dropped.

### Apply

1. Install the format drop-in (included by `nginx.conf`'s
   `include /etc/nginx/conf.d/*.conf;`, which runs before `sites-enabled`):

   ```
   cp deploy/nginx-ws-log-redaction.conf /etc/nginx/conf.d/ws-log-redaction.conf
   ```

2. In **each** `location /ws { ... }` block (currently in
   `/etc/nginx/sites-enabled/alethedash` and `/etc/nginx/sites-enabled/laghound`),
   add as the first line inside the block:

   ```
   access_log /var/log/nginx/access.log ws_redacted;
   ```

   A `location`-level `access_log` overrides the inherited default for that
   location only, so nothing else changes.

3. Validate then reload (never reload an invalid config):

   ```
   nginx -t && systemctl reload nginx
   ```

The live apply was driven from CI-adjacent tooling via
`az vm run-command invoke --resource-group ALETHEDASH-RG --name alethedash-vm`,
gated on `nginx -t` with automatic rollback from a `/root/nginx-ws-redact-backup-*`
copy on failure.

### Verify

```
curl -sk "https://127.0.0.1/ws/agent?key=SENTINEL" -H "Host: laghound.com"   # 400/401 expected
grep "/ws/agent" /var/log/nginx/access.log | tail -1     # logs "GET /ws/agent HTTP/1.1" — no ?key=
grep SENTINEL /var/log/nginx/access.log                  # MUST return nothing
```

### Rollback

Remove `/etc/nginx/conf.d/ws-log-redaction.conf` and the per-location
`access_log ... ws_redacted;` lines, then `nginx -t && systemctl reload nginx`.

## Definitive fix

Move the credential out of the query string into a header. **Done for agents
(v0.28.56):** the C# agent sends `X-LagHound-Agent-Key`; the server-side
`?key=` fallback is removed at the Rust-agent decommission. Still pending for
browsers (`Sec-WebSocket-Protocol` or equivalent for the `?access_token=` /
`?token=` JWTs). Once no client sends a credential in the query string, this
redaction is obsolete and can be removed.
