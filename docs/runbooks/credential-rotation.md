# Runbook: Rotate DASHBOARD_CREDENTIAL_KEY / DASHBOARD_JWT_SECRET

Both secrets are read by the C# control plane and are **fail-closed** outside
`Development` — the service refuses to start without a valid value. They live in
the service unit environment file `/etc/alethedash-cs.env` on the control-plane
VM; the service is `alethedash-cs`.

| Secret | Format | Protects | Rotation impact |
|---|---|---|---|
| `DASHBOARD_CREDENTIAL_KEY` | 64 hex chars (32 bytes) | AES-256-GCM encryption of stored cloud-account credentials + alert-webhook secrets | Data at rest — needs a re-encrypt or dual-key window (below) |
| `DASHBOARD_JWT_SECRET` | base64 (≥32 bytes) | HS256 JWT signing | Invalidates **all** live sessions — everyone must log in again |

## Rotating `DASHBOARD_CREDENTIAL_KEY`

`CredentialCipher` supports a **dual-key decrypt window** via
`DASHBOARD_CREDENTIAL_KEY_OLD`
(`src/Networker.Security/CredentialCipher.cs`,
`src/Networker.ControlPlane/Security/CredentialCipherExtensions.cs`): `Decrypt`
tries the primary key first and falls back to the old key on a
`CryptographicException`. This lets you rotate without a flag-day re-encrypt:

1. Generate a new key: `openssl rand -hex 32`.
2. In `/etc/alethedash-cs.env`, set:
   - `DASHBOARD_CREDENTIAL_KEY` = **new** key
   - `DASHBOARD_CREDENTIAL_KEY_OLD` = **previous** key
3. Restart: `sudo systemctl restart alethedash-cs`.
4. New writes encrypt under the new key; old rows still decrypt via the fallback.
5. Re-encrypt existing rows under the new key (re-save each cloud account, e.g.
   `PUT /api/projects/{id}/cloud-accounts/{id}`, or run the project's re-encrypt
   pass), then remove `DASHBOARD_CREDENTIAL_KEY_OLD` and restart again.

> If you set only `DASHBOARD_CREDENTIAL_KEY` to a fresh value with no
> `_OLD` fallback, **every previously stored credential becomes undecryptable**.
> Always stage the old key first.

## Rotating `DASHBOARD_JWT_SECRET`

1. Generate: `openssl rand -base64 32`.
2. Set `DASHBOARD_JWT_SECRET` in `/etc/alethedash-cs.env`.
3. `sudo systemctl restart alethedash-cs`.
4. All existing JWTs (minted under the old secret) now fail validation — expected.
   Users re-authenticate.

## Validation

- Service is up: `systemctl status alethedash-cs` and
  `curl -s https://laghound.com/api/health` returns `ok`.
- Credential key works: validate a stored cloud account
  (`POST /api/projects/{id}/cloud-accounts/{id}/validate`) — a successful decrypt
  proves the key.
- JWT works: log in and call a protected endpoint.
- After a credential-key rotation that touched the soak, confirm the nightly
  **Prod soak check** stays green.
