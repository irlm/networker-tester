use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

use crate::auth::{ProjectRole, DEFAULT_PROJECT_ID};

#[derive(Debug, Serialize)]
pub struct ProjectRow {
    pub project_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub settings: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ProjectMemberRow {
    pub project_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub joined_at: DateTime<Utc>,
    pub invited_by: Option<Uuid>,
    pub email: String,
    pub display_name: Option<String>,
}

/// List projects visible to a user. Platform admins see all projects.
pub async fn list_user_projects(
    client: &Client,
    user_id: &Uuid,
    is_platform_admin: bool,
) -> anyhow::Result<Vec<ProjectRow>> {
    let rows = if is_platform_admin {
        client
            .query(
                "SELECT project_id, name, slug, description, created_by, created_at, updated_at, settings \
                 FROM project ORDER BY created_at",
                &[],
            )
            .await?
    } else {
        client
            .query(
                "SELECT p.project_id, p.name, p.slug, p.description, p.created_by, \
                        p.created_at, p.updated_at, p.settings \
                 FROM project p \
                 JOIN project_member pm ON pm.project_id = p.project_id \
                 WHERE pm.user_id = $1 \
                 ORDER BY p.created_at",
                &[user_id],
            )
            .await?
    };

    Ok(rows
        .iter()
        .map(|r| ProjectRow {
            project_id: r.get("project_id"),
            name: r.get("name"),
            slug: r.get("slug"),
            description: r.get("description"),
            created_by: r.get("created_by"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
            settings: r.get("settings"),
        })
        .collect())
}

/// Get a single project by ID.
pub async fn get_project(client: &Client, project_id: &Uuid) -> anyhow::Result<Option<ProjectRow>> {
    let row = client
        .query_opt(
            "SELECT project_id, name, slug, description, created_by, created_at, updated_at, settings \
             FROM project WHERE project_id = $1",
            &[project_id],
        )
        .await?;

    Ok(row.map(|r| ProjectRow {
        project_id: r.get("project_id"),
        name: r.get("name"),
        slug: r.get("slug"),
        description: r.get("description"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
        settings: r.get("settings"),
    }))
}

/// Create a new project. Auto-generates slug from name.
pub async fn create_project(
    client: &Client,
    name: &str,
    slug: &str,
    description: Option<&str>,
    created_by: &Uuid,
) -> anyhow::Result<ProjectRow> {
    let project_id = Uuid::new_v4();
    let now = Utc::now();
    let settings = serde_json::json!({});

    client
        .execute(
            "INSERT INTO project (project_id, name, slug, description, created_by, created_at, updated_at, settings) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[&project_id, &name, &slug, &description, created_by, &now, &now, &settings],
        )
        .await?;

    // Also add creator as project admin
    client
        .execute(
            "INSERT INTO project_member (project_id, user_id, role, joined_at, invited_by) \
             VALUES ($1, $2, 'admin', $3, $4)",
            &[&project_id, created_by, &now, created_by],
        )
        .await?;

    Ok(ProjectRow {
        project_id,
        name: name.to_string(),
        slug: slug.to_string(),
        description: description.map(|s| s.to_string()),
        created_by: Some(*created_by),
        created_at: now,
        updated_at: now,
        settings,
    })
}

/// Update a project's name, description, or settings.
pub async fn update_project(
    client: &Client,
    project_id: &Uuid,
    name: Option<&str>,
    description: Option<&str>,
    settings: Option<&serde_json::Value>,
) -> anyhow::Result<bool> {
    let now = Utc::now();
    // Build the update dynamically based on what's provided
    let current = match get_project(client, project_id).await? {
        Some(p) => p,
        None => return Ok(false),
    };

    let new_name = name.unwrap_or(&current.name);
    let new_desc = description.or(current.description.as_deref());
    let default_settings = current.settings.clone();
    let new_settings = settings.unwrap_or(&default_settings);

    let n = client
        .execute(
            "UPDATE project SET name = $1, description = $2, settings = $3, updated_at = $4 \
             WHERE project_id = $5",
            &[&new_name, &new_desc, new_settings, &now, project_id],
        )
        .await?;

    Ok(n > 0)
}

/// Delete a project. The Default project cannot be deleted.
pub async fn delete_project(
    client: &Client,
    project_id: &Uuid,
) -> anyhow::Result<Result<(), &'static str>> {
    if *project_id == DEFAULT_PROJECT_ID {
        return Ok(Err("Cannot delete the Default project"));
    }

    // Delete members first, then project
    client
        .execute(
            "DELETE FROM project_member WHERE project_id = $1",
            &[project_id],
        )
        .await?;

    let n = client
        .execute("DELETE FROM project WHERE project_id = $1", &[project_id])
        .await?;

    if n > 0 {
        Ok(Ok(()))
    } else {
        Ok(Err("Project not found"))
    }
}

/// Get a user's role within a project.
pub async fn get_member_role(
    client: &Client,
    project_id: &Uuid,
    user_id: &Uuid,
) -> anyhow::Result<Option<ProjectRole>> {
    let row = client
        .query_opt(
            "SELECT role FROM project_member WHERE project_id = $1 AND user_id = $2",
            &[project_id, user_id],
        )
        .await?;

    Ok(row.and_then(|r| {
        let role_str: String = r.get("role");
        match role_str.as_str() {
            "admin" => Some(ProjectRole::Admin),
            "operator" => Some(ProjectRole::Operator),
            "viewer" => Some(ProjectRole::Viewer),
            _ => None,
        }
    }))
}

/// List all members of a project, joined with dash_user for email/display_name.
pub async fn list_members(
    client: &Client,
    project_id: &Uuid,
) -> anyhow::Result<Vec<ProjectMemberRow>> {
    let rows = client
        .query(
            "SELECT pm.project_id, pm.user_id, pm.role, pm.joined_at, pm.invited_by, \
                    u.email, u.display_name \
             FROM project_member pm \
             JOIN dash_user u ON u.user_id = pm.user_id \
             WHERE pm.project_id = $1 \
             ORDER BY pm.joined_at",
            &[project_id],
        )
        .await?;

    Ok(rows
        .iter()
        .map(|r| ProjectMemberRow {
            project_id: r.get("project_id"),
            user_id: r.get("user_id"),
            role: r.get("role"),
            joined_at: r.get("joined_at"),
            invited_by: r.get("invited_by"),
            email: r.get("email"),
            display_name: r.get("display_name"),
        })
        .collect())
}

/// Add a user to a project with the given role.
pub async fn add_member(
    client: &Client,
    project_id: &Uuid,
    user_id: &Uuid,
    role: &str,
    invited_by: &Uuid,
) -> anyhow::Result<()> {
    let now = Utc::now();
    client
        .execute(
            "INSERT INTO project_member (project_id, user_id, role, joined_at, invited_by) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (project_id, user_id) DO UPDATE SET role = $3",
            &[project_id, user_id, &role, &now, invited_by],
        )
        .await?;
    Ok(())
}

/// Update a member's role within a project.
pub async fn update_member_role(
    client: &Client,
    project_id: &Uuid,
    user_id: &Uuid,
    role: &str,
) -> anyhow::Result<bool> {
    let n = client
        .execute(
            "UPDATE project_member SET role = $1 WHERE project_id = $2 AND user_id = $3",
            &[&role, project_id, user_id],
        )
        .await?;
    Ok(n > 0)
}

/// Remove a member from a project. Prevents removing the last admin.
pub async fn remove_member(
    client: &Client,
    project_id: &Uuid,
    user_id: &Uuid,
) -> anyhow::Result<Result<(), &'static str>> {
    // Check if this user is an admin
    let target_role = client
        .query_opt(
            "SELECT role FROM project_member WHERE project_id = $1 AND user_id = $2",
            &[project_id, user_id],
        )
        .await?;

    if let Some(row) = target_role {
        let role: String = row.get("role");
        if role == "admin" {
            // Count remaining admins
            let admin_count: i64 = client
                .query_one(
                    "SELECT COUNT(*) FROM project_member WHERE project_id = $1 AND role = 'admin'",
                    &[project_id],
                )
                .await?
                .get(0);
            if admin_count <= 1 {
                return Ok(Err("Cannot remove the last admin from a project"));
            }
        }
    } else {
        return Ok(Err("Member not found"));
    }

    client
        .execute(
            "DELETE FROM project_member WHERE project_id = $1 AND user_id = $2",
            &[project_id, user_id],
        )
        .await?;

    Ok(Ok(()))
}

/// Convert a project name to a URL-safe slug.
pub fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else if c == ' ' {
                '-'
            } else {
                // strip
                '\0'
            }
        })
        .filter(|c| *c != '\0')
        .collect::<String>()
        // collapse multiple hyphens
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My Project"), "my-project");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(slugify("Test & Demo (v2)"), "test-demo-v2");
    }

    #[test]
    fn slugify_multiple_spaces() {
        assert_eq!(slugify("a   b"), "a-b");
    }

    #[test]
    fn slugify_already_clean() {
        assert_eq!(slugify("hello-world"), "hello-world");
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn default_project_id_matches() {
        let expected: Uuid = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        assert_eq!(DEFAULT_PROJECT_ID, expected);
    }
}
