use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct SovereigntyZone {
    pub code: String,
    pub parent_code: Option<String>,
    pub name: String,
    pub display: String,
    pub legal_note: Option<String>,
    pub compliance_level: Option<String>,
    pub fallback_zone: Option<String>,
    pub requires_approval: bool,
    pub requires_mfa: bool,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

/// Return all sovereignty zones ordered by code.
#[allow(dead_code)]
pub async fn list_zones(client: &Client) -> anyhow::Result<Vec<SovereigntyZone>> {
    let rows = client
        .query(
            "SELECT code, parent_code, name, display, legal_note, compliance_level, \
                    fallback_zone, requires_approval, requires_mfa, status, created_at \
             FROM sovereignty_zone \
             ORDER BY code",
            &[],
        )
        .await?;

    Ok(rows.iter().map(row_to_zone).collect())
}

/// Return a single zone by its 2-character code, or `None` if not found.
#[allow(dead_code)]
pub async fn get_zone(client: &Client, code: &str) -> anyhow::Result<Option<SovereigntyZone>> {
    let row = client
        .query_opt(
            "SELECT code, parent_code, name, display, legal_note, compliance_level, \
                    fallback_zone, requires_approval, requires_mfa, status, created_at \
             FROM sovereignty_zone \
             WHERE code = $1",
            &[&code],
        )
        .await?;

    Ok(row.as_ref().map(row_to_zone))
}

/// Follow the fallback chain from `zone_code` until we find a zone that has at
/// least one active server, then return that zone's code.
///
/// If the starting zone already has an active server it is returned immediately.
/// The chain is capped at 10 hops to prevent infinite loops from bad data.
/// If no zone with an active server is found, the original `zone_code` is returned.
#[allow(dead_code)]
pub async fn resolve_fallback(client: &Client, zone_code: &str) -> anyhow::Result<String> {
    let mut current = zone_code.to_string();

    for _ in 0..10 {
        // Check whether this zone has at least one active server.
        let has_server: bool = client
            .query_one(
                "SELECT EXISTS(\
                    SELECT 1 FROM server_registry \
                    WHERE zone_code = $1 AND status = 'active'\
                )",
                &[&current],
            )
            .await?
            .get(0);

        if has_server {
            return Ok(current);
        }

        // Walk one step up the fallback chain.
        let next: Option<String> = client
            .query_opt(
                "SELECT fallback_zone FROM sovereignty_zone WHERE code = $1",
                &[&current],
            )
            .await?
            .and_then(|r| r.get::<_, Option<String>>(0));

        match next {
            Some(fb) if fb != current => current = fb,
            _ => break,
        }
    }

    // No zone with an active server found — return original.
    Ok(zone_code.to_string())
}

#[allow(dead_code)]
fn row_to_zone(r: &tokio_postgres::Row) -> SovereigntyZone {
    SovereigntyZone {
        code: r.get("code"),
        parent_code: r.get("parent_code"),
        name: r.get("name"),
        display: r.get("display"),
        legal_note: r.get("legal_note"),
        compliance_level: r.get("compliance_level"),
        fallback_zone: r.get("fallback_zone"),
        requires_approval: r.get("requires_approval"),
        requires_mfa: r.get("requires_mfa"),
        status: r.get("status"),
        created_at: r.get("created_at"),
    }
}
