# Cloud Authentication (Ambient + Stored Credentials)

The C# control plane (`Networker.ControlPlane`) manages cloud resources (Azure
VMs, AWS EC2, GCP GCE) by shelling out to the vendor CLIs (`az`, `aws`,
`gcloud`) from `CliComputeProvisioner`. It authenticates those CLI calls with a
**dual model**:

1. **Ambient CLI auth** — when a project has no stored credentials, the CLI uses
   whatever identity the host already has: an Azure system-assigned managed
   identity / `az login` session, an AWS instance profile / default profile, and
   so on. Nothing is written to disk in this mode.
2. **Per-project stored credentials** — a project can register provider
   credentials (`CloudAccount`) that are encrypted at rest and materialised into
   short-lived, owner-only temp files at provision time so the CLI can read them.

> **This is not an IMDS token-exchange / federated-identity system.** The C#
> control plane contains **zero** IMDS references
> (`grep 169.254.169.254 src/Networker.ControlPlane` returns nothing). The
> Azure→AWS/GCP OIDC-federation flow that earlier revisions of this document
> described was a Rust-dashboard design and is **not** how the current control
> plane works. The only IMDS use anywhere in the codebase is
> `src/Networker.Endpoint/CloudMetadata.cs`, where a *tester* detects which cloud
> it is running on — unrelated to control-plane auth.

## The two authentication modes

### Ambient CLI auth (no stored credentials)

When a provision request carries no credentials (or the subscription/region
fields are empty), `CliComputeProvisioner` omits the corresponding CLI flags and
lets the CLI resolve identity from the host:

| Provider | Ambient source | Behaviour in code |
|---|---|---|
| Azure | Managed identity / `az login` | `--subscription` / `--resource-group` flags omitted when empty; ARM `--ids` is self-describing (`BuildAzure`, `CliComputeProvisioner.cs`) |
| AWS | Instance profile / default profile | No `AWS_*` env vars set → the CLI falls back to the host's ambient profile (`BuildAws`, `CliComputeProvisioner.cs`) |
| GCP | *Not supported ambiently* | GCP requires a stored `json_key`; `CreateGcpVmAsync` fails with `gcp config: missing json_key` if none is present |

### Stored per-project credentials

A project registers credentials through the cloud-account API (below). They are
stored **AES-256-GCM encrypted** in the database (`CredentialCipher`) and, at
provision time, decrypted and written to **owner-only (`0600`) temp files** that
the CLI reads via `@file` / `file://` references. Secrets never appear on `argv`.

## How stored credentials are protected

### At rest — AES-256-GCM

`src/Networker.Security/CredentialCipher.cs`:

- Algorithm: **AES-256-GCM** — 32-byte key, random 12-byte nonce per encryption,
  16-byte GCM auth tag.
- Key: `DASHBOARD_CREDENTIAL_KEY` — 64 hex chars (32 bytes), byte-compatible with
  the legacy Rust cipher so C# can decrypt rows Rust wrote.
- Key rotation: `DASHBOARD_CREDENTIAL_KEY_OLD` (optional). `Decrypt` tries the
  primary key first and falls back to the old key on a `CryptographicException`
  (identical to the Rust `decrypt_with_fallback`). See
  [`runbooks/credential-rotation.md`](runbooks/credential-rotation.md) for the
  rotation procedure.
- Fail-closed: outside `Development`, the control plane refuses to start without a
  valid `DASHBOARD_CREDENTIAL_KEY` (`CredentialCipherExtensions.cs`).

The encrypted bytes live in `CloudAccount.credentials_enc` +
`CloudAccount.credentials_nonce` (`src/Networker.Data/Entities/CloudAccount.cs`).
The list/detail APIs return the account **redacted** — the ciphertext is never
sent to a client.

### In transit to the CLI — 0600 temp files, never argv

`src/Networker.ControlPlane/Provisioning/SecretFile.cs` writes each secret to a
temp file created with `UnixFileMode.UserRead | UserWrite` (`0600`, owner-only)
at file-creation time (not a post-hoc `chmod`), with no trailing newline. On
Windows the mode flag is skipped (no POSIX modes). The caller deletes the file
best-effort after the CLI call.

`CliComputeProvisioner` routes every secret through `SecretFile` and references
it by path, so credentials never land on the process command line (which would
be visible in `ps` / logs):

| Secret | CLI reference |
|---|---|
| Azure service-principal secret | `az login --service-principal -p @<file>` |
| Azure Windows admin password | `az vm create --admin-password @<file>` |
| Azure protected settings / API key | `--protected-settings @<file>` |
| Azure custom-data bootstrap (carries API key) | `--custom-data @<file>` |
| AWS user-data bootstrap | `--user-data file://<file>` |
| GCP service-account key | `GOOGLE_APPLICATION_CREDENTIALS=<file>` (env, not argv) |
| GCP startup script | `--metadata-from-file startup-script=<file>` |

When spawning a process whose args contain a credential, the provisioner logs
`(args redacted: contains credentials)` instead of the arg list.

## Managing stored credentials — the API

Two resources, both project-scoped
(`src/Networker.ControlPlane/Endpoints/CloudAccountsEndpoints.cs`,
`CloudConnectionsEndpoints.cs`):

### Cloud accounts (encrypted provider credentials)

| Verb | Route |
|---|---|
| GET | `/api/projects/{projectId}/cloud-accounts` (redacted list) |
| POST | `/api/projects/{projectId}/cloud-accounts` (encrypts on create) |
| GET | `/api/projects/{projectId}/cloud-accounts/{id}` (redacted) |
| PUT | `/api/projects/{projectId}/cloud-accounts/{id}` (re-encrypts) |
| DELETE | `/api/projects/{projectId}/cloud-accounts/{id}` |
| POST | `/api/projects/{projectId}/cloud-accounts/{id}/validate` |

Credentials submitted here are what get AES-256-GCM encrypted and later written
to the `0600` temp files above.

### Cloud connections (non-secret provider config)

| Verb | Route |
|---|---|
| GET | `/api/projects/{projectId}/cloud-connections` |
| POST | `/api/projects/{projectId}/cloud-connections` |
| GET | `/api/projects/{projectId}/cloud-connections/{id}` (admin only) |
| PUT | `/api/projects/{projectId}/cloud-connections/{id}` |
| DELETE | `/api/projects/{projectId}/cloud-connections/{id}` |
| POST | `/api/projects/{projectId}/cloud-connections/{id}/validate` |

`CloudConnection.config` is stored **plaintext** (it holds non-secret provider
configuration, not credentials) and is not returned by the list endpoint.

## Security model

### What is protected, and how

| Concern | Mechanism |
|---|---|
| Credentials at rest | AES-256-GCM (`CredentialCipher`), key `DASHBOARD_CREDENTIAL_KEY`, fail-closed outside Development |
| Credentials during a CLI call | `0600` owner-only temp file (`SecretFile`), read via `@file` / env — never on `argv` |
| Log exposure | provisioner redacts credential-bearing arg lists |
| Key rotation | `DASHBOARD_CREDENTIAL_KEY_OLD` decrypt-fallback window |

### Attack surface

If the control-plane VM is compromised, an attacker can (a) read the
`DASHBOARD_CREDENTIAL_KEY` from the service environment and decrypt stored
provider credentials, and (b) act as any ambient identity the host holds
(managed identity / instance profile). Mitigations:

- Scope provider credentials and any ambient identity to least privilege
  (single subscription/account, compute-only roles).
- Keep `DASHBOARD_CREDENTIAL_KEY` in the service unit environment
  (`/etc/alethedash-cs.env`), not in the repo or the database.
- Monitor Azure Activity Log / AWS CloudTrail / GCP Audit Logs for anomalous
  instance activity.
- Prefer ambient auth (no stored secret on disk) where the provider supports it.
