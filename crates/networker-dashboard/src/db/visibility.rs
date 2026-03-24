use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashSet;
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct VisibilityRuleRow {
    pub rule_id: Uuid,
    pub project_id: Uuid,
    pub user_id: Option<Uuid>,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    // joined fields
    pub user_email: Option<String>,
    pub created_by_email: String,
}

pub async fn list_rules(
    client: &Client,
    project_id: &Uuid,
) -> anyhow::Result<Vec<VisibilityRuleRow>> {
    let rows = client
        .query(
            "SELECT r.rule_id, r.project_id, r.user_id, r.resource_type, r.resource_id,
                    r.created_by, r.created_at,
                    u.email AS user_email,
                    cb.email AS created_by_email
             FROM test_visibility_rule r
             LEFT JOIN dash_user u ON u.user_id = r.user_id
             JOIN dash_user cb ON cb.user_id = r.created_by
             WHERE r.project_id = $1
             ORDER BY r.created_at DESC",
            &[project_id],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| VisibilityRuleRow {
            rule_id: r.get("rule_id"),
            project_id: r.get("project_id"),
            user_id: r.get("user_id"),
            resource_type: r.get("resource_type"),
            resource_id: r.get("resource_id"),
            created_by: r.get("created_by"),
            created_at: r.get("created_at"),
            user_email: r.get("user_email"),
            created_by_email: r.get("created_by_email"),
        })
        .collect())
}

pub async fn add_rule(
    client: &Client,
    project_id: &Uuid,
    user_id: Option<&Uuid>,
    resource_type: &str,
    resource_id: &Uuid,
    created_by: &Uuid,
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    client
        .execute(
            "INSERT INTO test_visibility_rule (rule_id, project_id, user_id, resource_type, resource_id, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[&id, project_id, &user_id, &resource_type, resource_id, created_by],
        )
        .await?;
    Ok(id)
}

pub async fn remove_rule(client: &Client, rule_id: &Uuid, project_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            "DELETE FROM test_visibility_rule WHERE rule_id = $1 AND project_id = $2",
            &[rule_id, project_id],
        )
        .await?;
    Ok(())
}

/// Returns the set of resource IDs visible to the given user, or None if no filtering is needed.
///
/// - Reads `project.settings` JSONB to check the `test_visibility` field.
/// - If missing or `"all"` (default): returns `None` (no filtering).
/// - If `"explicit"`: queries `test_visibility_rule` for rules where
///   `(user_id IS NULL OR user_id = $user_id) AND resource_type = $resource_type`.
pub async fn visible_resources(
    client: &Client,
    project_id: &Uuid,
    user_id: &Uuid,
    resource_type: &str,
) -> anyhow::Result<Option<HashSet<Uuid>>> {
    // Read project settings
    let row = client
        .query_opt(
            "SELECT settings FROM project WHERE project_id = $1",
            &[project_id],
        )
        .await?;

    let settings: serde_json::Value = match row {
        Some(r) => r.get("settings"),
        None => return Ok(None),
    };

    let visibility = settings
        .get("test_visibility")
        .and_then(|v| v.as_str())
        .unwrap_or("all");

    if visibility != "explicit" {
        return Ok(None);
    }

    // Query visibility rules for this user (including global rules where user_id IS NULL)
    let rows = client
        .query(
            "SELECT resource_id FROM test_visibility_rule
             WHERE project_id = $1
               AND (user_id IS NULL OR user_id = $2)
               AND resource_type = $3",
            &[project_id, user_id, &resource_type],
        )
        .await?;

    let ids: HashSet<Uuid> = rows.iter().map(|r| r.get("resource_id")).collect();
    Ok(Some(ids))
}

#[cfg(test)]
mod tests {
    use super::VisibilityRuleRow;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_rule() -> VisibilityRuleRow {
        VisibilityRuleRow {
            rule_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            user_id: None,
            resource_type: "job".to_string(),
            resource_id: Uuid::new_v4(),
            created_by: Uuid::new_v4(),
            created_at: Utc::now(),
            user_email: None,
            created_by_email: "admin@example.com".to_string(),
        }
    }

    #[test]
    fn rule_fields_default() {
        let rule = make_rule();
        assert!(rule.user_id.is_none());
        assert!(rule.user_email.is_none());
        assert_eq!(rule.resource_type, "job");
        assert!(!rule.created_by_email.is_empty());
    }

    #[test]
    fn rule_with_user() {
        let user_id = Uuid::new_v4();
        let rule = VisibilityRuleRow {
            user_id: Some(user_id),
            user_email: Some("viewer@example.com".to_string()),
            ..make_rule()
        };
        assert_eq!(rule.user_id, Some(user_id));
        assert_eq!(rule.user_email.as_deref(), Some("viewer@example.com"));
    }

    #[test]
    fn rule_ids_unique() {
        let a = make_rule();
        let b = make_rule();
        assert_ne!(a.rule_id, b.rule_id);
    }

    #[test]
    fn serializes_to_json() {
        let rule = make_rule();
        let json = serde_json::to_value(&rule).expect("must serialize");
        assert!(json["rule_id"].is_string());
        assert!(json["user_id"].is_null());
        assert_eq!(json["resource_type"], "job");
    }

    #[test]
    fn schedule_resource_type() {
        let rule = VisibilityRuleRow {
            resource_type: "schedule".to_string(),
            ..make_rule()
        };
        assert_eq!(rule.resource_type, "schedule");
    }
}
