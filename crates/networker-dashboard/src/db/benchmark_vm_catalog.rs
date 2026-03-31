use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct VmCatalogRow {
    pub vm_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub cloud: String,
    pub region: String,
    pub ip: String,
    pub ssh_user: String,
    pub languages: serde_json::Value,
    pub vm_size: Option<String>,
    pub status: String,
    pub last_health_check: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

fn row_to_vm(r: &tokio_postgres::Row) -> VmCatalogRow {
    VmCatalogRow {
        vm_id: r.get("vm_id"),
        project_id: r.get("project_id"),
        name: r.get("name"),
        cloud: r.get("cloud"),
        region: r.get("region"),
        ip: r.get("ip"),
        ssh_user: r.get("ssh_user"),
        languages: r.get("languages"),
        vm_size: r.get("vm_size"),
        status: r.get("status"),
        last_health_check: r.get("last_health_check"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    client: &Client,
    project_id: &Uuid,
    name: &str,
    cloud: &str,
    region: &str,
    ip: &str,
    ssh_user: &str,
    vm_size: Option<&str>,
    created_by: Option<&Uuid>,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO benchmark_vm_catalog
                (vm_id, project_id, name, cloud, region, ip, ssh_user, vm_size, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            &[
                &id,
                project_id,
                &name,
                &cloud,
                &region,
                &ip,
                &ssh_user,
                &vm_size,
                &created_by,
            ],
        )
        .await?;
    Ok(id)
}

pub async fn get(client: &Client, vm_id: &Uuid) -> anyhow::Result<Option<VmCatalogRow>> {
    let row = client
        .query_opt(
            "SELECT vm_id, project_id, name, cloud, region, ip, ssh_user, languages,
                    vm_size, status, last_health_check, created_by, created_at
             FROM benchmark_vm_catalog WHERE vm_id = $1",
            &[vm_id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_vm))
}

pub async fn list_for_project(
    client: &Client,
    project_id: &Uuid,
) -> anyhow::Result<Vec<VmCatalogRow>> {
    let rows = client
        .query(
            "SELECT vm_id, project_id, name, cloud, region, ip, ssh_user, languages,
                    vm_size, status, last_health_check, created_by, created_at
             FROM benchmark_vm_catalog WHERE project_id = $1
             ORDER BY name",
            &[project_id],
        )
        .await?;
    Ok(rows.iter().map(row_to_vm).collect())
}

pub async fn update_status(client: &Client, vm_id: &Uuid, status: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE benchmark_vm_catalog SET status = $1, last_health_check = now()
             WHERE vm_id = $2",
            &[&status, vm_id],
        )
        .await?;
    Ok(())
}

pub async fn update_languages(
    client: &Client,
    vm_id: &Uuid,
    languages: &serde_json::Value,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE benchmark_vm_catalog SET languages = $1 WHERE vm_id = $2",
            &[languages, vm_id],
        )
        .await?;
    Ok(())
}

pub async fn delete(client: &Client, vm_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "DELETE FROM benchmark_vm_catalog WHERE vm_id = $1",
            &[vm_id],
        )
        .await?;
    Ok(())
}
