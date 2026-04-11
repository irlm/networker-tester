//! DESTRUCTIVE reset binary -- wipes ALL data in ALL dashboard tables.
//!
//! Gated by `DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true` env var plus a pre-flight
//! check that refuses to run if any `cloud_account` row has a name matching
//! `prod%` (case-insensitive).
//!
//! Intended for one-time pre-production bootstrap only. Never wire this into
//! automated migrations or CI.

use anyhow::{anyhow, Context, Result};
use tokio_postgres::NoTls;

const RESET_SQL: &str = include_str!("../../bootstrap/reset-pre-prod.sql");

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    if std::env::var("DASHBOARD_ALLOW_DESTRUCTIVE_RESET").as_deref() != Ok("true") {
        return Err(anyhow!(
            "refusing to run without DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true.\n\
             To execute, run:\n\
             \n    \
             DASHBOARD_ALLOW_DESTRUCTIVE_RESET=true \\\n    \
             DASHBOARD_DB_URL=postgres://... \\\n    \
             cargo run -p networker-dashboard --bin reset_pre_prod\n"
        ));
    }

    let db_url = std::env::var("DASHBOARD_DB_URL").context("DASHBOARD_DB_URL must be set")?;
    let (client, conn) = tokio_postgres::connect(&db_url, NoTls)
        .await
        .context("connect to postgres")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("postgres connection error: {e}");
        }
    });

    // Pre-flight: refuse if any cloud_account looks like production.
    // (cloud_account has no `labels` JSONB column -- name check only.)
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
                let project: uuid::Uuid = r.get("project_id");
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

    println!("WARNING: executing destructive reset in 3 seconds...");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    client
        .batch_execute(RESET_SQL)
        .await
        .context("reset SQL failed")?;

    println!("OK: reset complete");
    Ok(())
}
