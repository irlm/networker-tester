# Backup, restore and retention

> **2026-08-24 — production had no usable backup for five months.** The VM's
> ad-hoc `/opt/backups/daily-backup.sh` ran `pg_dump alethedash`, the *retired
> Rust dashboard's* database, abandoned at the C# cutover on 2026-03-31. That
> database still exists, so `pg_dump` exited 0, the tarball looked plausible
> (4.1 MB every night, 69 of them in blob storage) and the log said "Backup
> complete" — while `alethedash_core`, the database production actually uses,
> was never captured. This document exists so that cannot recur silently.

## The three rules

Everything in `scripts/backup-daily.sh` follows from the failure above:

1. **Never hardcode the database name.** The primary database is read from the
   live service config (`DASHBOARD_DB_URL_NPGSQL` in `/etc/alethedash-cs.env`),
   so it cannot drift from what the control plane is really using. If it cannot
   be determined, the script fails rather than guessing.
2. **Back up every database, not a named subset.** A database nobody remembered
   is exactly the one that gets missed.
3. **Verify, and fail loudly.** No `2>/dev/null`, no `|| true` on the backup
   path. A dump is a backup only once `pg_restore -l` can list it *and* its
   TABLE DATA entries cover every table the live database has.

That last check is deliberately not a byte threshold. A compressed dump's size
swings by an order of magnitude with content, so a floor either passes empty
dumps or fails legitimate small ones (both were observed while testing this).
Table coverage is compression-independent.

## What runs, and when

| | |
|---|---|
| Script (repo) | `scripts/backup-daily.sh` → installed as `/usr/local/sbin/db-backup.sh` |
| Schedule | cron, `0 2 * * *` → `/var/log/db-backup.log` |
| Auth | the VM's **managed identity** — there is no storage key in the script |
| Destination | storage account `alethedashbackups`, container `backups` |

### Blob layout

Names are **role-based, never brand-based**, so a rebrand or a domain change
never invalidates the layout. The brand prefix is *derived* from the primary
database name (`alethedash_core` → prefix `alethedash`), so a product rename
keeps producing the same paths with no code edit. The real source database
travels as blob metadata (`sourcedb`), not in the filename.

```
db/daily/core-YYYY-MM-DD.dump        the live control-plane database
db/daily/logs-YYYY-MM-DD.dump        legacy Rust log database
db/daily/legacy-YYYY-MM-DD.dump      pre-cutover Rust dashboard database
db/monthly/core-YYYY-MM.dump         written on the 1st of each month
config/daily/host-YYYY-MM-DD.tar.gz  systemd unit + env file + nginx sites
last_backup.json                     what a monitor should read
```

**The config bundle is not optional.** `DASHBOARD_CREDENTIAL_KEY` encrypts
stored cloud credentials at rest. Restoring the database *without* that file
leaves every stored credential undecryptable — the rows come back, the
credentials do not.

## Retention

Two independent layers:

- **Blobs** — storage-account lifecycle policy: `db/daily/` deleted after 30
  days, `db/monthly/` after 365.
- **Database rows** — `scripts/prune-retention.sh`, **dry-run by default**.
  `RETENTION_RAW_DAYS` (default 90) ages out probe attempts;
  `RETENTION_LOG_DAYS` (default 90) ages out `service_log` and `perf_log`.

Chain it after the backup so nothing is ever deleted without a fresh verified
dump having just succeeded:

```bash
/usr/local/sbin/db-backup.sh && /usr/local/sbin/db-prune.sh --apply
```

### Why the delete order is explicit

The canonical DDL declares `RequestAttempt.RunId REFERENCES TestRun ... ON
DELETE CASCADE`, **but the live database has that constraint as NO ACTION**
(the table predates the current DDL). Three of `requestattempt`'s own children
are NO ACTION too: `errorrecord`, `mthroughputresult`, `servertimingresult`.

So "delete the parent and let it cascade" does not work — it fails with a
foreign-key violation. Verified:

```
ERROR:  update or delete on table "requestattempt" violates foreign key
        constraint "errorrecord_attemptid_fkey" on table "errorrecord"
```

The script therefore deletes bottom-up in one transaction and relies on no
cascade being configured: NO ACTION children → attempts (the remaining results
cascade) → childless runs → logs.

## Restore

```bash
# 1. fetch the dump (managed identity on the VM, or an authorised az login)
az storage blob download --account-name alethedashbackups --auth-mode login \
  --container-name backups --name db/daily/core-YYYY-MM-DD.dump --file core.dump

# 2. restore into a NEW database first — never straight over the live one
sudo -u postgres createdb restore_check
sudo -u postgres pg_restore -d restore_check --no-owner core.dump

# 3. sanity-check before you cut over
sudo -u postgres psql -d restore_check -c "SELECT count(*) FROM requestattempt;"

# 4. the secrets, without which stored cloud credentials stay encrypted
az storage blob download --account-name alethedashbackups --auth-mode login \
  --container-name backups --name config/daily/host-YYYY-MM-DD.tar.gz --file host.tar.gz
```

`scripts/restore-dashboard.sh` automates the full VM rebuild path.

## Monitoring

Read `last_backup.json` from the container. It carries the timestamp, the
primary database name, and per-artifact byte counts, TOC entry counts and
SHA-256 checksums. **Alert if its timestamp is older than 48 hours** — that is
the signal the five-month gap never produced.

## Open items

- **Rotate the storage-account key.** The retired script held one in plaintext
  and was world-readable (`-rwxr-xr-x`). It has been chmod 600'd and moved to
  `daily-backup.sh.disabled-2026-08-24`, but the key itself must be rotated;
  nothing uses it any more (the new script authenticates with the managed
  identity).
- The account is **Standard_LRS** — three copies in one datacenter. Consider
  ZRS/GRS if the recovery objective covers losing a region.
- The 69 legacy `backup-*.tar.gz` blobs at the container root are backups of
  the dead database and can be deleted once nobody wants them for forensics.
