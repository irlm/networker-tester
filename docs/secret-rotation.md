# Secret rotation

> **Why this exists.** On 2026-08-24 a storage-account key was found sitting in
> plaintext in a world-readable script on the prod VM, and nobody could say how
> old it was — because nothing recorded rotations. That is the same shape as the
> backup gap found the same day: the information needed to notice something was
> wrong existed nowhere. This splits the problem in two.

## The split: visibility in the UI, action in a script

| | Where | Why |
|---|---|---|
| **See** what is overdue | System → **Secrets** (`GET /api/admin/secrets`) | Knowing a key is 400 days old is what an operator needs, and it carries none of the risk of exposing the value |
| **Rotate** it | `scripts/rotate-secrets.sh`, run by an operator | The control plane is internet-facing; an endpoint that can rotate turns any single compromise — XSS, an auth bypass, a stolen operator token — into total credential compromise |

Rotation is quarterly-or-on-incident work. The convenience of a button is worth
very little against that trade, so there isn't one.

**The API never returns secret material** — not a value, not a hash, not a
prefix. Only identity, age, policy and status. That is asserted by a test which
boots the host with a sentinel signing key and fails if the string ever appears
in the response.

## What is tracked

The inventory is **static, in code** (`SecretsEndpoints.Inventory`), not derived
from the rotation table. That is deliberate: a secret nobody has ever rotated
must still appear — as `never`, counted under "needs attention" — and a design
keyed off the table alone would hide exactly the case that matters most.

| Key | Policy | Automated | Note |
|---|---|---|---|
| `jwt-secret` | 90 d | yes | Rotating signs every user out |
| `credential-key` | 365 d | **no** | Data-encryption key — see below |
| `db-password` | 180 d | yes | Needs a control-plane restart |
| `github-token` | 90 d | yes | Revoke the old PAT afterwards |
| `storage-key` | 90 d | yes | Backups use managed identity, so unaffected |

Status is `never` → `ok` → `due` (last 14 days before the deadline) → `overdue`.

## Rotating

```bash
sudo ./rotate-secrets.sh --list                 # what has been recorded
sudo ./rotate-secrets.sh --rotate jwt-secret    # one secret, with confirmation
sudo ./rotate-secrets.sh --rotate storage-key --yes
```

There is **no `--all`**. These secrets differ in blast radius — a JWT rotation
signs everyone out, a database password needs a restart — and a single flag that
does all of them invites running it without reading what it will do.

Each rotation restarts the service where required, waits for `/api/health`, and
**fails loudly if the service does not come back**, so a bad rotation stops there
instead of being followed by another one. It then records the event in
`secret_rotation` (V055), which is what the panel reads.

## Credential key — read before touching

`DASHBOARD_CREDENTIAL_KEY` is a **data-encryption key, not a password.** It
encrypts stored cloud credentials at rest.

Replacing it the way you replace a password does not rotate anything: it makes
every stored cloud credential **permanently unreadable**, and there is no error
at the moment of damage — the failure surfaces later, when a deployment tries to
use a credential it can no longer decrypt. `rotate-secrets.sh` refuses it and
exits 2.

Rotating it correctly means decrypting with the old key and re-encrypting with
the new one in a single transaction, across every table holding ciphertext
(`cloud_account`, `alert_channel.config`, SDK endpoint secrets). That is
application work, not shell work. Until that job exists, treat the key as
non-rotatable and protect it by other means:

- it lives only in `/etc/alethedash-cs.env`, mode `0600`, root-owned;
- it is captured in the nightly config bundle
  (`config/daily/host-*.tar.gz`) — **a database restore without it recovers the
  rows but not the credentials**, so the two must be restored together;
- if it is ever believed compromised, the recovery path is to re-enter the
  affected cloud credentials, not to swap the key.

## After rotating anything

- **`github-token`** — revoke the old PAT in GitHub. Rotation here only replaces
  what this host uses; it does not invalidate the old token.
- **`storage-key`** — nothing on the host holds an account key any more (backups
  authenticate with the VM's managed identity), so there is nothing to update.
  Verify with a manual `db-backup.sh` run anyway.
- **`jwt-secret`** — every session is invalid; users simply log in again.

## Related

- `docs/backup-and-retention.md` — the config bundle that carries these secrets
- The real perimeter on this VM is **Azure RBAC**, not SSH: `az vm run-command`
  runs as root without ever touching the SSH door. Restricting a script to SSH
  would give a false sense of containment; restrict who holds Virtual Machine
  Contributor on the resource group instead.
