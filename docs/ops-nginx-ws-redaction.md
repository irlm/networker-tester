# nginx WebSocket access-log redaction

**Status: APPLIED LIVE on `alethedash-vm` (2026-07-22).** This document records
the change so that you can re-apply it after a VM rebuild. The VM manages the
production nginx config, and the repo does not otherwise hold it.

## Problem

Agents authenticate at `/ws/agent`. Browsers and testers authenticate at
`/ws/dashboard?access_token=<jwt>` and `/ws/testers?token=<jwt>`. Since v0.28.56
the C# agent sends its key in the `X-LagHound-Agent-Key` request header. But the
server still accepts the legacy `?key=<api_key>` query for older fielded agents.
For those clients — and for the browser `?access_token=`/`?token=` JWTs — the
credential is in the **query string**. This is a wire-compatibility carryover
from the retired Rust dashboard. The default nginx `combined` log format logs
`$request`, which is the full request line and includes the query. Therefore,
nginx writes the live credential to `/var/log/nginx/access.log` in cleartext on
every WebSocket connect. The 2026-07 audit flagged this as a P1
(`docs/analysis/websec-audit-2026-07.md`,
`docs/analysis/secrets-audit-2026-07.md`).

This fix is defence-in-depth. The credential is not exposed on the wire, because
TLS terminates at nginx. The agent keys are 48-char CSPRNG. The server stores
them as a SHA-256 hash and compares them in constant time. But a plaintext key
on disk in the access log is a needless exposure.

## Fix (interim)

`deploy/nginx-ws-log-redaction.conf` defines a `ws_redacted` log format. This
format uses the standard `combined` layout, but it logs `$uri` (the request
path, with no query string) instead of `$request`. Each `location /ws` block
opts in. The format keeps the IP, timestamp, status, and user-agent audit
signal. It drops only the query string, and thus the secret.

### Apply

1. Install the format drop-in. `nginx.conf` includes it through
   `include /etc/nginx/conf.d/*.conf;`, which runs before `sites-enabled`:

   ```
   cp deploy/nginx-ws-log-redaction.conf /etc/nginx/conf.d/ws-log-redaction.conf
   ```

2. Add this line as the first line inside **each** `location /ws { ... }` block.
   These blocks are currently in `/etc/nginx/sites-enabled/alethedash` and
   `/etc/nginx/sites-enabled/laghound`:

   ```
   access_log /var/log/nginx/access.log ws_redacted;
   ```

   A `location`-level `access_log` overrides the inherited default for that
   location only. Nothing else changes.

3. Validate the config. Then reload it. Never reload an invalid config:

   ```
   nginx -t && systemctl reload nginx
   ```

CI-adjacent tooling drove the live apply through
`az vm run-command invoke --resource-group ALETHEDASH-RG --name alethedash-vm`.
The apply was gated on `nginx -t`. On failure, it rolled back automatically from
a `/root/nginx-ws-redact-backup-*` copy.

### Verify

```
curl -sk "https://127.0.0.1/ws/agent?key=SENTINEL" -H "Host: laghound.com"   # 400/401 expected
grep "/ws/agent" /var/log/nginx/access.log | tail -1     # logs "GET /ws/agent HTTP/1.1" — no ?key=
grep SENTINEL /var/log/nginx/access.log                  # MUST return nothing
```

### Rollback

Remove `/etc/nginx/conf.d/ws-log-redaction.conf` and the per-location
`access_log ... ws_redacted;` lines. Then run `nginx -t && systemctl reload nginx`.

## Definitive fix

Move the credential out of the query string and into a header. This move is
**done for agents (v0.28.56)**: the C# agent sends `X-LagHound-Agent-Key`. The
server-side `?key=` fallback goes away at the Rust-agent decommission. The move
is still pending for browsers (`Sec-WebSocket-Protocol` or an equivalent for the
`?access_token=` / `?token=` JWTs). When no client sends a credential in the
query string, this redaction is obsolete and you can remove it.
