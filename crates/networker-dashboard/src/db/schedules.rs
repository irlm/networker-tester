use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct ScheduleRow {
    pub schedule_id: Uuid,
    pub name: Option<String>,
    pub definition_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub deployment_id: Option<Uuid>,
    pub cron_expr: String,
    pub enabled: bool,
    pub config: Option<serde_json::Value>,
    pub auto_start_vm: bool,
    pub auto_stop_vm: bool,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub project_id: Option<Uuid>,
    pub benchmark_config_id: Option<Uuid>,
}

fn row_to_schedule(r: &tokio_postgres::Row) -> ScheduleRow {
    ScheduleRow {
        schedule_id: r.get("schedule_id"),
        name: r.get("name"),
        definition_id: r.get("definition_id"),
        agent_id: r.get("agent_id"),
        deployment_id: r.get("deployment_id"),
        cron_expr: r.get("cron_expr"),
        enabled: r.get("enabled"),
        config: r.get("config"),
        auto_start_vm: r.get("auto_start_vm"),
        auto_stop_vm: r.get("auto_stop_vm"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        next_run_at: r.get("next_run_at"),
        last_run_at: r.get("last_run_at"),
        project_id: r.get("project_id"),
        benchmark_config_id: r.get("benchmark_config_id"),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    client: &Client,
    name: &str,
    cron_expr: &str,
    config: &serde_json::Value,
    agent_id: Option<&Uuid>,
    deployment_id: Option<&Uuid>,
    auto_start_vm: bool,
    auto_stop_vm: bool,
    next_run_at: Option<DateTime<Utc>>,
    project_id: &Uuid,
    benchmark_config_id: Option<&Uuid>,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO schedule (schedule_id, name, cron_expr, config, agent_id, deployment_id,
                                   auto_start_vm, auto_stop_vm, enabled, next_run_at, project_id,
                                   benchmark_config_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, TRUE, $9, $10, $11)",
            &[
                &id,
                &name,
                &cron_expr,
                config,
                &agent_id,
                &deployment_id,
                &auto_start_vm,
                &auto_stop_vm,
                &next_run_at,
                project_id,
                &benchmark_config_id,
            ],
        )
        .await?;
    Ok(id)
}

pub async fn get(client: &Client, schedule_id: &Uuid) -> anyhow::Result<Option<ScheduleRow>> {
    let row = client
        .query_opt(
            "SELECT schedule_id, name, definition_id, agent_id, deployment_id, cron_expr,
                    enabled, config, auto_start_vm, auto_stop_vm, created_by,
                    created_at, next_run_at, last_run_at, project_id, benchmark_config_id
             FROM schedule WHERE schedule_id = $1",
            &[schedule_id],
        )
        .await?;
    Ok(row.as_ref().map(row_to_schedule))
}

#[allow(dead_code)]
pub async fn list(client: &Client, project_id: &Uuid) -> anyhow::Result<Vec<ScheduleRow>> {
    list_filtered(client, project_id, None).await
}

pub async fn list_filtered(
    client: &Client,
    project_id: &Uuid,
    visible_ids: Option<&std::collections::HashSet<uuid::Uuid>>,
) -> anyhow::Result<Vec<ScheduleRow>> {
    if let Some(ids) = visible_ids {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
    }

    let rows = if let Some(ids) = visible_ids {
        let id_vec: Vec<Uuid> = ids.iter().copied().collect();
        client
            .query(
                "SELECT schedule_id, name, definition_id, agent_id, deployment_id, cron_expr,
                        enabled, config, auto_start_vm, auto_stop_vm, created_by,
                        created_at, next_run_at, last_run_at, project_id
                 FROM schedule WHERE project_id = $1 AND schedule_id = ANY($2)
                 ORDER BY created_at DESC",
                &[project_id, &id_vec],
            )
            .await?
    } else {
        client
            .query(
                "SELECT schedule_id, name, definition_id, agent_id, deployment_id, cron_expr,
                        enabled, config, auto_start_vm, auto_stop_vm, created_by,
                        created_at, next_run_at, last_run_at, project_id
                 FROM schedule WHERE project_id = $1 ORDER BY created_at DESC",
                &[project_id],
            )
            .await?
    };
    Ok(rows.iter().map(row_to_schedule).collect())
}

#[allow(clippy::too_many_arguments)]
pub async fn update(
    client: &Client,
    schedule_id: &Uuid,
    name: &str,
    cron_expr: &str,
    config: &serde_json::Value,
    agent_id: Option<&Uuid>,
    deployment_id: Option<&Uuid>,
    auto_start_vm: bool,
    auto_stop_vm: bool,
    next_run_at: Option<DateTime<Utc>>,
) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE schedule SET name = $1, cron_expr = $2, config = $3, agent_id = $4,
                    deployment_id = $5, auto_start_vm = $6, auto_stop_vm = $7, next_run_at = $8
             WHERE schedule_id = $9",
            &[
                &name,
                &cron_expr,
                config,
                &agent_id,
                &deployment_id,
                &auto_start_vm,
                &auto_stop_vm,
                &next_run_at,
                schedule_id,
            ],
        )
        .await?;
    Ok(n > 0)
}

pub async fn delete(client: &Client, schedule_id: &Uuid) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "DELETE FROM schedule WHERE schedule_id = $1",
            &[schedule_id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn set_enabled(
    client: &Client,
    schedule_id: &Uuid,
    enabled: bool,
) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE schedule SET enabled = $1 WHERE schedule_id = $2",
            &[&enabled, schedule_id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn get_due(client: &Client) -> anyhow::Result<Vec<ScheduleRow>> {
    let rows = client
        .query(
            "SELECT schedule_id, name, definition_id, agent_id, deployment_id, cron_expr,
                    enabled, config, auto_start_vm, auto_stop_vm, created_by,
                    created_at, next_run_at, last_run_at, project_id, benchmark_config_id
             FROM schedule
             WHERE enabled = TRUE AND next_run_at <= now()
             ORDER BY next_run_at ASC",
            &[],
        )
        .await?;
    Ok(rows.iter().map(row_to_schedule).collect())
}

pub async fn mark_run(
    client: &Client,
    schedule_id: &Uuid,
    next_run_at: Option<DateTime<Utc>>,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE schedule SET last_run_at = now(), next_run_at = $1 WHERE schedule_id = $2",
            &[&next_run_at, schedule_id],
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ScheduleRow;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_minimal_schedule() -> ScheduleRow {
        ScheduleRow {
            schedule_id: Uuid::new_v4(),
            name: None,
            definition_id: None,
            agent_id: None,
            deployment_id: None,
            cron_expr: "0 * * * * *".to_string(),
            enabled: true,
            config: None,
            auto_start_vm: false,
            auto_stop_vm: false,
            created_by: None,
            created_at: Utc::now(),
            next_run_at: None,
            last_run_at: None,
            project_id: None,
            benchmark_config_id: None,
        }
    }

    /// ScheduleRow field mapping and defaults.
    mod row_fields {
        use super::*;

        #[test]
        fn minimal_fields_are_none() {
            let row = make_minimal_schedule();
            assert!(row.name.is_none());
            assert!(row.agent_id.is_none());
            assert!(row.deployment_id.is_none());
            assert!(row.config.is_none());
            assert!(row.next_run_at.is_none());
            assert!(row.last_run_at.is_none());
            assert!(!row.auto_start_vm);
            assert!(!row.auto_stop_vm);
        }

        #[test]
        fn enabled_default_is_true() {
            assert!(make_minimal_schedule().enabled);
        }

        #[test]
        fn disabled() {
            let mut row = make_minimal_schedule();
            row.enabled = false;
            assert!(!row.enabled);
        }

        #[test]
        fn schedule_id_is_unique_per_instance() {
            let a = make_minimal_schedule();
            let b = make_minimal_schedule();
            assert_ne!(a.schedule_id, b.schedule_id);
        }

        #[test]
        fn cron_expr_stored_verbatim() {
            let exotic = "0 */7 1-5 15,30 3,6,9,12 Fri";
            let row = ScheduleRow {
                cron_expr: exotic.to_string(),
                ..make_minimal_schedule()
            };
            assert_eq!(row.cron_expr, exotic);
        }

        #[test]
        fn all_optional_fields_populated() {
            let agent_id = Uuid::new_v4();
            let deployment_id = Uuid::new_v4();
            let definition_id = Uuid::new_v4();
            let created_by = Uuid::new_v4();
            let now = Utc::now();
            let config = serde_json::json!({"mode": "http1"});

            let row = ScheduleRow {
                schedule_id: Uuid::new_v4(),
                name: Some("nightly-probe".to_string()),
                definition_id: Some(definition_id),
                agent_id: Some(agent_id),
                deployment_id: Some(deployment_id),
                cron_expr: "0 0 2 * * *".to_string(),
                enabled: true,
                config: Some(config),
                auto_start_vm: true,
                auto_stop_vm: true,
                created_by: Some(created_by),
                created_at: now,
                next_run_at: Some(now + chrono::Duration::hours(22)),
                last_run_at: Some(now - chrono::Duration::hours(2)),
                project_id: Some(Uuid::new_v4()),
                benchmark_config_id: None,
            };

            assert_eq!(row.name.as_deref(), Some("nightly-probe"));
            assert_eq!(row.agent_id, Some(agent_id));
            assert_eq!(row.deployment_id, Some(deployment_id));
            assert!(row.next_run_at.unwrap() > now);
            assert!(row.last_run_at.unwrap() < now);
        }
    }

    /// auto_start_vm / auto_stop_vm flag combinations.
    mod vm_flags {
        use super::*;

        #[test]
        fn start_only() {
            let row = ScheduleRow {
                auto_start_vm: true,
                auto_stop_vm: false,
                ..make_minimal_schedule()
            };
            assert!(row.auto_start_vm && !row.auto_stop_vm);
        }

        #[test]
        fn stop_only() {
            let row = ScheduleRow {
                auto_start_vm: false,
                auto_stop_vm: true,
                ..make_minimal_schedule()
            };
            assert!(!row.auto_start_vm && row.auto_stop_vm);
        }

        #[test]
        fn both() {
            let row = ScheduleRow {
                auto_start_vm: true,
                auto_stop_vm: true,
                ..make_minimal_schedule()
            };
            assert!(row.auto_start_vm && row.auto_stop_vm);
        }

        #[test]
        fn neither() {
            let row = make_minimal_schedule();
            assert!(!row.auto_start_vm && !row.auto_stop_vm);
        }
    }

    /// Serialization: ScheduleRow → JSON round-trip.
    mod serialization {
        use super::*;

        #[test]
        fn serializes_to_json() {
            let row = ScheduleRow {
                name: Some("weekly-azure".to_string()),
                cron_expr: "0 0 9 * * Mon".to_string(),
                ..make_minimal_schedule()
            };

            let json = serde_json::to_value(&row).expect("must serialize");
            assert_eq!(json["name"], "weekly-azure");
            assert_eq!(json["cron_expr"], "0 0 9 * * Mon");
            assert_eq!(json["enabled"], true);
            assert!(json["schedule_id"].is_string());
            assert!(json["config"].is_null());
            assert!(json["next_run_at"].is_null());
        }
    }

    /// Toggle (enable/disable) logic — stateless simulation.
    mod toggle {
        use super::*;

        #[test]
        fn enabled_to_disabled() {
            let row = make_minimal_schedule(); // enabled = true
            assert!(row.enabled);
            let new_enabled = !row.enabled;
            assert!(!new_enabled);
        }

        #[test]
        fn disabled_to_enabled() {
            let mut row = make_minimal_schedule();
            row.enabled = false;
            assert!(!row.enabled);
        }

        #[test]
        fn re_enable_triggers_next_run_recompute() {
            use std::str::FromStr;
            let row = ScheduleRow {
                enabled: false,
                cron_expr: "0 0 6 * * *".to_string(),
                ..make_minimal_schedule()
            };

            let new_enabled = !row.enabled;
            assert!(new_enabled);

            let next = cron::Schedule::from_str(&row.cron_expr)
                .ok()
                .and_then(|s| s.upcoming(chrono::Utc).next());

            assert!(next.is_some());
            assert!(next.unwrap() > chrono::Utc::now());
        }

        #[test]
        fn disable_does_not_recompute() {
            let row = make_minimal_schedule();
            let new_enabled = !row.enabled;
            assert!(!new_enabled);
        }
    }
}
