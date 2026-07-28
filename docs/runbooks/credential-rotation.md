# Runbook: Rotate DASHBOARD_CREDENTIAL_KEY / DASHBOARD_JWT_SECRET

The C# control plane reads both secrets. Both are **fail-closed** outside
`Development`: the service refuses to start without a valid value. Both secrets
live in the service unit environment file `/etc/alethedash-cs.env` on the
control-plane VM. The service is `alethedash-cs`.

| Secret | Format | Protects | Rotation impact |
|---|---|---|---|
| `DASHBOARD_CREDENTIAL_KEY` | 64 hex chars (32 bytes) | AES-256-GCM encryption of stored cloud-account credentials + alert-webhook secrets | Data at rest — needs a re-encrypt or dual-key window (below) |
| `DASHBOARD_JWT_SECRET` | base64 (≥32 bytes) | HS256 JWT signing | Invalidates **all** live sessions — everyone must log in again |

## Rotating `DASHBOARD_CREDENTIAL_KEY`

`CredentialCipher` supports a **dual-key decrypt window** through
`DASHBOARD_CREDENTIAL_KEY_OLD`
(`src/Networker.Security/CredentialCipher.cs`,
`src/Networker.ControlPlane/Security/CredentialCipherExtensions.cs`). `Decrypt`
tries the primary key first. It falls back to the old key on a
`CryptographicException`. This window lets you rotate without a flag-day
re-encrypt:

1. Generate a new key: `openssl rand -hex 32`.
2. In `/etc/alethedash-cs.env`, set:
   - `DASHBOARD_CREDENTIAL_KEY` = **new** key
   - `DASHBOARD_CREDENTIAL_KEY_OLD` = **previous** key
3. Restart the service: `sudo systemctl restart alethedash-cs`.
4. New writes encrypt under the new key. Old rows still decrypt through the fallback.
5. Re-encrypt the existing rows under the new key. Re-save each cloud account
   (for example, `PUT /api/projects/{id}/cloud-accounts/{id}`), or run the
   project's re-encrypt pass. Then remove `DASHBOARD_CREDENTIAL_KEY_OLD` and
   restart the service again.

> If you set only `DASHBOARD_CREDENTIAL_KEY` to a fresh value and give no
> `_OLD` fallback, you cannot decrypt **any** stored credential. Always stage
> the old key first.

## Rotating `DASHBOARD_JWT_SECRET`

1. Generate a new secret: `openssl rand -base64 32`.
2. Set `DASHBOARD_JWT_SECRET` in `/etc/alethedash-cs.env`.
3. Restart the service: `sudo systemctl restart alethedash-cs`.
4. All existing JWTs from the old secret now fail validation. This result is
   expected. Users re-authenticate.

## Validation

- Confirm the service is up. Run `systemctl status alethedash-cs`, and confirm
  that `curl -s https://laghound.com/api/health` returns `ok`.
- Confirm the credential key works. Validate a stored cloud account
  (`POST /api/projects/{id}/cloud-accounts/{id}/validate`). A successful decrypt
  proves the key.
- Confirm the JWT works. Log in and call a protected endpoint.
- After a credential-key rotation that touched the soak, confirm that the nightly
  **Prod soak check** stays green.
