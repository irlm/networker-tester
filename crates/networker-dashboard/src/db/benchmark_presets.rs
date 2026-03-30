use anyhow::{bail, Context};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkComparePresetFilters {
    pub target_search: String,
    pub scenario: String,
    pub phase_model: String,
    pub server_region: String,
    pub network_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkComparePresetInput {
    pub id: Option<Uuid>,
    pub name: String,
    pub run_ids: Vec<Uuid>,
    pub baseline_run_id: Option<Uuid>,
    pub filters: Option<BenchmarkComparePresetFilters>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkComparePreset {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub run_ids: Vec<Uuid>,
    pub baseline_run_id: Option<Uuid>,
    pub filters: Option<BenchmarkComparePresetFilters>,
}

#[derive(Debug, Clone)]
struct NormalizedPresetInput {
    id: Uuid,
    name: String,
    name_key: String,
    run_ids: Vec<Uuid>,
    baseline_run_id: Uuid,
    filters: BenchmarkComparePresetFilters,
}

fn normalize_input(input: BenchmarkComparePresetInput) -> anyhow::Result<NormalizedPresetInput> {
    let name = input.name.trim();
    if name.is_empty() {
        bail!("preset name cannot be empty");
    }

    let mut run_ids = Vec::new();
    for run_id in input.run_ids {
        if !run_ids.contains(&run_id) {
            run_ids.push(run_id);
        }
        if run_ids.len() == 4 {
            break;
        }
    }
    if run_ids.len() < 2 {
        bail!("benchmark compare presets need at least two unique runs");
    }

    let baseline_run_id = input
        .baseline_run_id
        .filter(|run_id| run_ids.contains(run_id))
        .unwrap_or(run_ids[0]);

    let filters = input.filters.unwrap_or_default();
    let id = input.id.unwrap_or_else(Uuid::new_v4);

    Ok(NormalizedPresetInput {
        id,
        name: name.to_string(),
        name_key: name.to_lowercase(),
        run_ids,
        baseline_run_id,
        filters,
    })
}

fn row_to_preset(row: &tokio_postgres::Row) -> BenchmarkComparePreset {
    let filters = BenchmarkComparePresetFilters {
        target_search: row.get::<_, String>("target_search"),
        scenario: row.get::<_, String>("scenario"),
        phase_model: row.get::<_, String>("phase_model"),
        server_region: row.get::<_, String>("server_region"),
        network_type: row.get::<_, String>("network_type"),
    };
    let filters = if filters == BenchmarkComparePresetFilters::default() {
        None
    } else {
        Some(filters)
    };

    BenchmarkComparePreset {
        id: row.get("preset_id"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        run_ids: row.get("run_ids"),
        baseline_run_id: row.get("baseline_run_id"),
        filters,
    }
}

pub async fn list(
    client: &Client,
    project_id: &Uuid,
) -> anyhow::Result<Vec<BenchmarkComparePreset>> {
    let rows = client
        .query(
            "SELECT preset_id, name, created_at, updated_at, run_ids, baseline_run_id,
                    target_search, scenario, phase_model, server_region, network_type
             FROM benchmark_compare_preset
             WHERE project_id = $1
             ORDER BY updated_at DESC, created_at DESC, name ASC",
            &[project_id],
        )
        .await
        .context("list benchmark compare presets")?;

    Ok(rows.iter().map(row_to_preset).collect())
}

pub async fn upsert(
    client: &Client,
    project_id: &Uuid,
    created_by: &Uuid,
    input: BenchmarkComparePresetInput,
) -> anyhow::Result<BenchmarkComparePreset> {
    let normalized = normalize_input(input)?;
    let row = client
        .query_one(
            "INSERT INTO benchmark_compare_preset (
                preset_id, project_id, created_by, name, name_key,
                run_ids, baseline_run_id,
                target_search, scenario, phase_model, server_region, network_type,
                created_at, updated_at
             ) VALUES (
                $1, $2, $3, $4, $5,
                $6, $7,
                $8, $9, $10, $11, $12,
                now(), now()
             )
             ON CONFLICT (project_id, name_key) DO UPDATE SET
                name = EXCLUDED.name,
                run_ids = EXCLUDED.run_ids,
                baseline_run_id = EXCLUDED.baseline_run_id,
                target_search = EXCLUDED.target_search,
                scenario = EXCLUDED.scenario,
                phase_model = EXCLUDED.phase_model,
                server_region = EXCLUDED.server_region,
                network_type = EXCLUDED.network_type,
                updated_at = now()
             RETURNING preset_id, name, created_at, updated_at, run_ids, baseline_run_id,
                       target_search, scenario, phase_model, server_region, network_type",
            &[
                &normalized.id,
                project_id,
                created_by,
                &normalized.name,
                &normalized.name_key,
                &normalized.run_ids,
                &Some(normalized.baseline_run_id),
                &normalized.filters.target_search,
                &normalized.filters.scenario,
                &normalized.filters.phase_model,
                &normalized.filters.server_region,
                &normalized.filters.network_type,
            ],
        )
        .await
        .context("upsert benchmark compare preset")?;

    Ok(row_to_preset(&row))
}

pub async fn delete(client: &Client, project_id: &Uuid, preset_id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "DELETE FROM benchmark_compare_preset
             WHERE project_id = $1 AND preset_id = $2",
            &[project_id, preset_id],
        )
        .await
        .context("delete benchmark compare preset")?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::{normalize_input, BenchmarkComparePresetFilters, BenchmarkComparePresetInput};
    use uuid::Uuid;

    #[test]
    fn normalize_input_deduplicates_and_defaults_baseline() {
        let run_a = Uuid::new_v4();
        let run_b = Uuid::new_v4();
        let normalized = normalize_input(BenchmarkComparePresetInput {
            id: None,
            name: "  Shared compare  ".to_string(),
            run_ids: vec![run_a, run_a, run_b],
            baseline_run_id: Some(Uuid::new_v4()),
            filters: None,
        })
        .unwrap();

        assert_eq!(normalized.name, "Shared compare");
        assert_eq!(normalized.name_key, "shared compare");
        assert_eq!(normalized.run_ids, vec![run_a, run_b]);
        assert_eq!(normalized.baseline_run_id, run_a);
        assert_eq!(normalized.filters, BenchmarkComparePresetFilters::default());
    }

    #[test]
    fn normalize_input_rejects_too_few_runs() {
        let run_a = Uuid::new_v4();
        let err = normalize_input(BenchmarkComparePresetInput {
            id: None,
            name: "Invalid".to_string(),
            run_ids: vec![run_a],
            baseline_run_id: Some(run_a),
            filters: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("at least two unique runs"));
    }
}
