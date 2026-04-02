use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct BenchmarkTestbedRow {
    pub testbed_id: Uuid,
    pub config_id: Uuid,
    pub cloud: String,
    pub region: String,
    pub topology: String,
    pub endpoint_vm_id: Option<String>,
    pub tester_vm_id: Option<String>,
    pub endpoint_ip: Option<String>,
    pub tester_ip: Option<String>,
    pub status: String,
    pub languages: serde_json::Value,
    pub vm_size: Option<String>,
    pub os: String,
}

fn row_to_testbed(r: &tokio_postgres::Row) -> BenchmarkTestbedRow {
    BenchmarkTestbedRow {
        testbed_id: r.get("testbed_id"),
        config_id: r.get("config_id"),
        cloud: r.get("cloud"),
        region: r.get("region"),
        topology: r.get("topology"),
        endpoint_vm_id: r.get("endpoint_vm_id"),
        tester_vm_id: r.get("tester_vm_id"),
        endpoint_ip: r.get("endpoint_ip"),
        tester_ip: r.get("tester_ip"),
        status: r.get("status"),
        languages: r.get("languages"),
        vm_size: r.get("vm_size"),
        os: r.get("os"),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    client: &Client,
    config_id: &Uuid,
    cloud: &str,
    region: &str,
    topology: &str,
    languages: &serde_json::Value,
    vm_size: Option<&str>,
    os: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO benchmark_testbed
                (testbed_id, config_id, cloud, region, topology, languages, vm_size, os)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[
                &id, config_id, &cloud, &region, &topology, languages, &vm_size, &os,
            ],
        )
        .await?;
    Ok(id)
}

pub async fn list_for_config(
    client: &Client,
    config_id: &Uuid,
) -> anyhow::Result<Vec<BenchmarkTestbedRow>> {
    let rows = client
        .query(
            "SELECT testbed_id, config_id, cloud, region, topology, endpoint_vm_id,
                    tester_vm_id, endpoint_ip, tester_ip, status, languages, vm_size, os
             FROM benchmark_testbed WHERE config_id = $1
             ORDER BY cloud, region",
            &[config_id],
        )
        .await?;
    Ok(rows.iter().map(row_to_testbed).collect())
}

pub async fn update_status(client: &Client, testbed_id: &Uuid, status: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE benchmark_testbed SET status = $1 WHERE testbed_id = $2",
            &[&status, testbed_id],
        )
        .await?;
    Ok(())
}

#[allow(dead_code)] // Used by orchestrator callbacks in Wave 2
pub async fn update_vm_info(
    client: &Client,
    testbed_id: &Uuid,
    endpoint_vm_id: Option<&str>,
    tester_vm_id: Option<&str>,
    endpoint_ip: Option<&str>,
    tester_ip: Option<&str>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE benchmark_testbed SET
                endpoint_vm_id = COALESCE($1, endpoint_vm_id),
                tester_vm_id = COALESCE($2, tester_vm_id),
                endpoint_ip = COALESCE($3, endpoint_ip),
                tester_ip = COALESCE($4, tester_ip)
             WHERE testbed_id = $5",
            &[
                &endpoint_vm_id,
                &tester_vm_id,
                &endpoint_ip,
                &tester_ip,
                testbed_id,
            ],
        )
        .await?;
    Ok(())
}
