# Runbook: Reset the admin password

Resets the production admin login for the C# control plane.

## Facts

- Admin user: `admin@laghound.com`.
- Password is stored as a **BCrypt** hash (`BCrypt.Net`, cost 11) in
  `dash_user.password_hash`.
- Database: `alethedash_core` on the **alethedash-vm** (resource group
  `ALETHEDASH-RG`), reached via `az vm run-command`.

## Critical gotcha — a bcrypt hash contains `$`

A bcrypt hash (`$2a$11$...`) contains `$` characters that the remote shell will
**expand** if the SQL is passed inline. The reset SQL **must** be shipped
**base64-encoded** and decoded on the VM — never inlined into the run-command
script.

## Procedure

1. Generate the bcrypt hash for the new password (cost 11). Any BCrypt.Net /
   `bcrypt` tool works; the control plane verifies against `BCrypt.Net` cost 11.

2. Build the reset SQL (clears the forced-reset and reset-token fields too):

   ```sql
   UPDATE dash_user
   SET password_hash = '<BCRYPT_HASH>',
       must_change_password = false,
       password_reset_token = NULL,
       password_reset_expires = NULL
   WHERE email = 'admin@laghound.com';
   ```

3. Base64-encode that SQL, then ship + decode + apply on the VM (NOT inline):

   ```bash
   az vm run-command invoke \
     --resource-group ALETHEDASH-RG \
     --name alethedash-vm \
     --command-id RunShellScript \
     --scripts "echo '<BASE64_SQL>' | base64 -d > /tmp/reset.sql && \
                sudo -u postgres psql -d alethedash_core -f /tmp/reset.sql && \
                rm -f /tmp/reset.sql"
   ```

4. Verify by logging in as `admin@laghound.com` with the new password.

## MANDATORY follow-up — update the soak secret

The nightly **Prod soak check** workflow logs in as this admin using the
`DASHBOARD_ADMIN_PASSWORD` repo secret. **After any reset you must update that
secret**, or the soak turns red and resets the decommission clock:

```bash
gh secret set DASHBOARD_ADMIN_PASSWORD
```

> This exact failure happened on 2026-07-23: the admin password was reset without
> updating `DASHBOARD_ADMIN_PASSWORD`, the soak went red, and the decommission
> soak window restarted.
