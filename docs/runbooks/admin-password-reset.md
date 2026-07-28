# Runbook: Reset the admin password

This runbook resets the production admin login for the C# control plane.

## Facts

- Admin user: `admin@laghound.com`.
- The control plane stores the password as a **BCrypt** hash (`BCrypt.Net`,
  cost 11) in `dash_user.password_hash`.
- Database: `alethedash_core` on the **alethedash-vm** (resource group
  `ALETHEDASH-RG`). You reach it through `az vm run-command`.

## Critical gotcha — a bcrypt hash contains `$`

A bcrypt hash (`$2a$11$...`) contains `$` characters. The remote shell
**expands** these characters when you pass the SQL inline. You **must** ship the
reset SQL as **base64** and decode it on the VM. Never put it inline in the
run-command script.

## Procedure

1. Generate the bcrypt hash for the new password (cost 11). Any BCrypt.Net or
   `bcrypt` tool works. The control plane verifies against `BCrypt.Net` cost 11.

2. Build the reset SQL. This SQL also clears the forced-reset and reset-token fields:

   ```sql
   UPDATE dash_user
   SET password_hash = '<BCRYPT_HASH>',
       must_change_password = false,
       password_reset_token = NULL,
       password_reset_expires = NULL
   WHERE email = 'admin@laghound.com';
   ```

3. Encode the SQL as base64. Then ship it, decode it, and apply it on the VM. Do NOT use inline SQL:

   ```bash
   az vm run-command invoke \
     --resource-group ALETHEDASH-RG \
     --name alethedash-vm \
     --command-id RunShellScript \
     --scripts "echo '<BASE64_SQL>' | base64 -d > /tmp/reset.sql && \
                sudo -u postgres psql -d alethedash_core -f /tmp/reset.sql && \
                rm -f /tmp/reset.sql"
   ```

4. Verify the reset. Log in as `admin@laghound.com` with the new password.

## MANDATORY follow-up — update the soak secret

The nightly **Prod soak check** workflow logs in as this admin. It uses the
`DASHBOARD_ADMIN_PASSWORD` repo secret. You **must** update that secret after any
reset. If you do not, the soak turns red and resets the decommission clock:

```bash
gh secret set DASHBOARD_ADMIN_PASSWORD
```

> This exact failure happened on 2026-07-23. An operator reset the admin password
> but did not update `DASHBOARD_ADMIN_PASSWORD`. The soak went red, and the
> decommission soak window restarted.
