//! DESTRUCTIVE reset binary -- wipes ALL data in ALL dashboard tables.
//!
//! Gated by three independent checks:
//!   1. `DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true` env var must be set
//!   2. `DASHBOARD_ALLOW_DESTRUCTIVE_RESET_CONFIRM_HOST` must match the host
//!      extracted from `DASHBOARD_DB_URL` (defense-in-depth so an operator
//!      can't accidentally run it against a non-pre-prod host)
//!   3. A pre-flight query refuses to run if any `cloud_account` row has a
//!      name matching `prod%` (case-insensitive)
//!
//! Intended for one-time pre-production bootstrap only. Never wire this into
//! automated migrations or CI.

use anyhow::{anyhow, Context, Result};
use tokio_postgres::NoTls;

const RESET_SQL: &str = include_str!("../../bootstrap/reset-pre-prod.sql");

/// Extract the host portion from a libpq-style URL, e.g.
/// `postgres://user:pw@db.example.com:5432/networker` -> `db.example.com`.
/// Returns `None` if the URL has no `@` or no recognisable host segment.
fn extract_db_host(url: &str) -> Option<String> {
    // Strip scheme.
    let after_scheme = url.split_once("://").map(|x| x.1).unwrap_or(url);
    // Everything after the last `@` is the authority + path; before it is creds.
    let authority_and_path = match after_scheme.rsplit_once('@') {
        Some((_, rest)) => rest,
        None => after_scheme,
    };
    // Host runs up to `:` (port), `/` (db), or `?` (query), whichever comes first.
    let end = authority_and_path
        .find([':', '/', '?'])
        .unwrap_or(authority_and_path.len());
    let host = &authority_and_path[..end];
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    if std::env::var("DASHBOARD_ALLOW_DESTRUCTIVE_RESET").as_deref() != Ok("true") {
        return Err(anyhow!(
            "refusing to run without DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true.\n\
             To execute, run:\n\
             \n    \
             DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true \\\n    \
             DASHBOARD_ALLOW_DESTRUCTIVE_RESET_CONFIRM_HOST=<db-host> \\\n    \
             DASHBOARD_DB_URL=postgres://... \\\n    \
             cargo run -p networker-dashboard --bin reset_pre_prod\n"
        ));
    }

    let db_url = std::env::var("DASHBOARD_DB_URL").context("DASHBOARD_DB_URL must be set")?;

    // Defense-in-depth: require the operator to echo the DB host we're about
    // to wipe. This prevents accidental execution against prod.
    let db_host = extract_db_host(&db_url)
        .ok_or_else(|| anyhow!("could not parse host from DASHBOARD_DB_URL"))?;
    let confirm_host = std::env::var("DASHBOARD_ALLOW_DESTRUCTIVE_RESET_CONFIRM_HOST")
        .context("DASHBOARD_ALLOW_DESTRUCTIVE_RESET_CONFIRM_HOST must be set")?;
    if confirm_host != db_host {
        return Err(anyhow!(
            "CONFIRM_HOST mismatch: DASHBOARD_ALLOW_DESTRUCTIVE_RESET_CONFIRM_HOST={confirm_host:?} \
             but DASHBOARD_DB_URL host is {db_host:?}. Refusing to run."
        ));
    }

    let (mut client, conn) = tokio_postgres::connect(&db_url, NoTls)
        .await
        .context("connect to postgres")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("postgres connection error: {e}");
        }
    });

    // Pre-flight: refuse if any cloud_account looks like production.
    // (cloud_account has no `labels` JSONB column -- name check only.)
    //
    // Note: V024b migrated project_id from uuid to CHAR(14) (base36 ProjectId),
    // so we decode it as String rather than uuid::Uuid.
    let prod_rows = client
        .query(
            "SELECT account_id, project_id, name \
             FROM cloud_account \
             WHERE name ILIKE 'prod%'",
            &[],
        )
        .await
        .context("pre-flight cloud_account scan")?;
    if !prod_rows.is_empty() {
        let summary: Vec<String> = prod_rows
            .iter()
            .map(|r| {
                let name: String = r.get("name");
                let project: String = r.get("project_id");
                let account: uuid::Uuid = r.get("account_id");
                format!("  - account={account} project={project} name={name:?}")
            })
            .collect();
        return Err(anyhow!(
            "refusing to reset: production-looking cloud_accounts present:\n{}\n\
             Investigate and remove before re-running.",
            summary.join("\n")
        ));
    }

    println!("WARNING: executing destructive reset against {db_host} in 3 seconds...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Run the TRUNCATE block inside an explicit transaction so a partial
    // failure leaves the DB untouched (instead of half-reset). VACUUM cannot
    // run inside a transaction, so we execute it separately after commit.
    let tx = client
        .transaction()
        .await
        .context("begin reset transaction")?;
    tx.batch_execute(RESET_SQL)
        .await
        .context("reset SQL failed")?;
    tx.commit().await.context("commit reset transaction")?;

    // VACUUM FULL ANALYZE runs autonomously (no transaction).
    client
        .batch_execute("VACUUM FULL ANALYZE")
        .await
        .context("post-reset VACUUM FULL ANALYZE failed")?;

    println!("OK: reset complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::extract_db_host;

    #[test]
    fn parses_host_with_creds_and_port() {
        assert_eq!(
            extract_db_host("postgres://user:pw@db.example.com:5432/networker"),
            Some("db.example.com".into())
        );
    }

    #[test]
    fn parses_host_without_creds() {
        assert_eq!(
            extract_db_host("postgres://db.example.com/networker"),
            Some("db.example.com".into())
        );
    }

    #[test]
    fn parses_host_without_port_or_db() {
        assert_eq!(
            extract_db_host("postgres://user@db.example.com"),
            Some("db.example.com".into())
        );
    }

    #[test]
    fn parses_host_with_query() {
        assert_eq!(
            extract_db_host("postgres://u:p@db.example.com?sslmode=require"),
            Some("db.example.com".into())
        );
    }

    #[test]
    fn parses_localhost() {
        assert_eq!(
            extract_db_host("postgres://networker:test@127.0.0.1:5432/networker"),
            Some("127.0.0.1".into())
        );
    }

    #[test]
    fn returns_none_for_empty_host() {
        assert_eq!(extract_db_host("postgres://:5432/db"), None);
    }
}
