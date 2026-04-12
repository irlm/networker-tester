use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio_postgres::Client;
use uuid::Uuid;

#[cfg(test)]
use crate::auth::default_project_id;
use crate::auth::ProjectRole;

#[derive(Debug, Serialize)]
pub struct ProjectRow {
    pub project_id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub settings: serde_json::Value,
    pub deleted_at: Option<DateTime<Utc>>,
    pub delete_protection: bool,
}

/// Project with the requesting user's role included (for list endpoint).
#[derive(Debug, Serialize)]
pub struct ProjectWithRole {
    pub project_id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ProjectMemberRow {
    pub project_id: String,
    pub user_id: Uuid,
    pub role: String,
    pub joined_at: DateTime<Utc>,
    pub invited_by: Option<Uuid>,
    pub email: String,
    pub display_name: Option<String>,
    pub status: String,
    pub invite_sent_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub enum AddMemberResult {
    Added,
    AlreadyMember,
    AlreadyPending,
    ReInvited,
}

/// List projects visible to a user, including the user's role in each.
/// Platform admins see all projects with role = "admin".
pub async fn list_user_projects(
    client: &Client,
    user_id: &Uuid,
    is_platform_admin: bool,
) -> anyhow::Result<Vec<ProjectWithRole>> {
    let rows = if is_platform_admin {
        // Platform admins see all projects; LEFT JOIN to get actual role if they're a member
        client
            .query(
                "SELECT p.project_id, p.name, p.slug, p.description, p.created_at, \
                        COALESCE(pm.role, 'admin') AS role \
                 FROM project p \
                 LEFT JOIN project_member pm ON pm.project_id = p.project_id AND pm.user_id = $1 \
                 WHERE p.deleted_at IS NULL \
                 ORDER BY p.created_at",
                &[user_id],
            )
            .await?
    } else {
        client
            .query(
                "SELECT p.project_id, p.name, p.slug, p.description, p.created_at, pm.role \
                 FROM project p \
                 JOIN project_member pm ON pm.project_id = p.project_id \
                 WHERE pm.user_id = $1 AND p.deleted_at IS NULL \
                 ORDER BY p.created_at",
                &[user_id],
            )
            .await?
    };

    Ok(rows
        .iter()
        .map(|r| ProjectWithRole {
            project_id: r.get("project_id"),
            name: r.get("name"),
            slug: r.get("slug"),
            description: r.get("description"),
            role: r.get("role"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Get a single project by ID.
pub async fn get_project(client: &Client, project_id: &str) -> anyhow::Result<Option<ProjectRow>> {
    let row = client
        .query_opt(
            "SELECT project_id, name, slug, description, created_by, created_at, updated_at, settings, \
                    deleted_at, COALESCE(delete_protection, FALSE) AS delete_protection \
             FROM project WHERE project_id = $1",
            &[&project_id],
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
        deleted_at: r.get("deleted_at"),
        delete_protection: r.get("delete_protection"),
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
    let project_id = crate::project_id::ProjectId::generate("us", "a20").to_string();
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
        deleted_at: None,
        delete_protection: false,
    })
}

/// Update a project's name, description, or settings.
pub async fn update_project(
    client: &Client,
    project_id: &str,
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
            &[&new_name, &new_desc, new_settings, &now, &project_id],
        )
        .await?;

    Ok(n > 0)
}

/// Delete a project. The Default project cannot be deleted.
pub async fn delete_project(
    client: &Client,
    project_id: &str,
) -> anyhow::Result<Result<(), &'static str>> {
    if project_id == crate::auth::default_project_id() {
        return Ok(Err("Cannot delete the Default project"));
    }

    // Delete members first, then project
    client
        .execute(
            "DELETE FROM project_member WHERE project_id = $1",
            &[&project_id],
        )
        .await?;

    let n = client
        .execute("DELETE FROM project WHERE project_id = $1", &[&project_id])
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
    project_id: &str,
    user_id: &Uuid,
) -> anyhow::Result<Option<ProjectRole>> {
    let row = client
        .query_opt(
            "SELECT role FROM project_member WHERE project_id = $1 AND user_id = $2",
            &[&project_id, user_id],
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
    project_id: &str,
) -> anyhow::Result<Vec<ProjectMemberRow>> {
    let rows = client
        .query(
            "SELECT pm.project_id, pm.user_id, pm.role, pm.joined_at, pm.invited_by, \
                    u.email, u.display_name, pm.status, pm.invite_sent_at \
             FROM project_member pm \
             JOIN dash_user u ON u.user_id = pm.user_id \
             WHERE pm.project_id = $1 \
             ORDER BY pm.joined_at",
            &[&project_id],
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
            status: r.get("status"),
            invite_sent_at: r.get("invite_sent_at"),
        })
        .collect())
}

/// Add a user to a project with the given role.
pub async fn add_member(
    client: &Client,
    project_id: &str,
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
            &[&project_id, user_id, &role, &now, invited_by],
        )
        .await?;
    Ok(())
}

/// Update a member's role within a project. Prevents demoting the last admin.
pub async fn update_member_role(
    client: &Client,
    project_id: &str,
    user_id: &Uuid,
    role: &str,
) -> anyhow::Result<Result<bool, &'static str>> {
    // If demoting from admin, check we're not removing the last admin
    if role != "admin" {
        let current_role = client
            .query_opt(
                "SELECT role FROM project_member WHERE project_id = $1 AND user_id = $2",
                &[&project_id, user_id],
            )
            .await?;
        if let Some(row) = current_role {
            let current: String = row.get("role");
            if current == "admin" {
                let admin_count: i64 = client
                    .query_one(
                        "SELECT COUNT(*) FROM project_member WHERE project_id = $1 AND role = 'admin'",
                        &[&project_id],
                    )
                    .await?
                    .get(0);
                if admin_count <= 1 {
                    return Ok(Err("Cannot demote the last admin"));
                }
            }
        }
    }

    let n = client
        .execute(
            "UPDATE project_member SET role = $1 WHERE project_id = $2 AND user_id = $3",
            &[&role, &project_id, user_id],
        )
        .await?;
    Ok(Ok(n > 0))
}

/// Remove a member from a project. Prevents removing the last admin.
pub async fn remove_member(
    client: &Client,
    project_id: &str,
    user_id: &Uuid,
) -> anyhow::Result<Result<(), &'static str>> {
    // Check if this user is an admin
    let target_role = client
        .query_opt(
            "SELECT role FROM project_member WHERE project_id = $1 AND user_id = $2",
            &[&project_id, user_id],
        )
        .await?;

    if let Some(row) = target_role {
        let role: String = row.get("role");
        if role == "admin" {
            // Count remaining admins
            let admin_count: i64 = client
                .query_one(
                    "SELECT COUNT(*) FROM project_member WHERE project_id = $1 AND role = 'admin'",
                    &[&project_id],
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
            &[&project_id, user_id],
        )
        .await?;

    Ok(Ok(()))
}

/// Soft-delete a project by setting deleted_at to now.
pub async fn suspend_project(client: &Client, project_id: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE project SET deleted_at = now() WHERE project_id = $1 AND deleted_at IS NULL",
            &[&project_id],
        )
        .await?;
    Ok(())
}

/// Restore a soft-deleted project by clearing deleted_at and any warnings.
pub async fn restore_project(client: &Client, project_id: &str) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE project SET deleted_at = NULL WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    // Clear warnings
    client
        .execute(
            "DELETE FROM workspace_warning WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    Ok(())
}

/// Toggle delete_protection on a project. Returns the new value.
pub async fn toggle_protection(client: &Client, project_id: &str) -> anyhow::Result<bool> {
    let row = client
        .query_one(
            "UPDATE project SET delete_protection = NOT COALESCE(delete_protection, FALSE) \
             WHERE project_id = $1 RETURNING delete_protection",
            &[&project_id],
        )
        .await?;
    Ok(row.get(0))
}

/// Permanently delete a project and all associated data (cascade).
/// The project must already be soft-deleted (deleted_at IS NOT NULL).
pub async fn hard_delete_project(client: &Client, project_id: &str) -> anyhow::Result<()> {
    // Cascade delete in dependency order
    client
        .execute(
            "DELETE FROM workspace_warning WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    client
        .execute(
            "DELETE FROM workspace_invite WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    client
        .execute(
            "DELETE FROM test_visibility_rule WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    client
        .execute(
            "DELETE FROM command_approval WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    client
        .execute(
            "DELETE FROM share_link WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    client
        .execute(
            "DELETE FROM cloud_account WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    client
        .execute("DELETE FROM schedule WHERE project_id = $1", &[&project_id])
        .await?;
    client
        .execute("DELETE FROM job WHERE project_id = $1", &[&project_id])
        .await?;
    client
        .execute(
            "DELETE FROM deployment WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    client
        .execute("DELETE FROM agent WHERE project_id = $1", &[&project_id])
        .await?;
    client
        .execute(
            "DELETE FROM project_member WHERE project_id = $1",
            &[&project_id],
        )
        .await?;
    client
        .execute("DELETE FROM project WHERE project_id = $1", &[&project_id])
        .await?;
    Ok(())
}

/// Find workspaces where no member has logged in within N days.
/// Excludes protected workspaces and already-suspended ones.
pub async fn find_inactive_workspaces(
    client: &Client,
    days: i64,
) -> anyhow::Result<Vec<ProjectRow>> {
    let rows = client
        .query(
            "SELECT p.project_id, p.name, p.slug, p.description, p.created_by, \
                    p.created_at, p.updated_at, p.settings, p.deleted_at, \
                    COALESCE(p.delete_protection, FALSE) AS delete_protection \
             FROM project p \
             WHERE p.deleted_at IS NULL \
               AND COALESCE(p.delete_protection, FALSE) = FALSE \
               AND NOT EXISTS ( \
                   SELECT 1 FROM project_member pm \
                   JOIN dash_user u ON u.user_id = pm.user_id \
                   WHERE pm.project_id = p.project_id \
                     AND u.last_login_at > now() - ($1::text || ' days')::interval \
               ) \
             ORDER BY p.name",
            &[&days.to_string()],
        )
        .await?;

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
            deleted_at: r.get("deleted_at"),
            delete_protection: r.get("delete_protection"),
        })
        .collect())
}

/// Find suspended workspaces where deleted_at is older than N days.
pub async fn find_suspended_older_than(
    client: &Client,
    days: i64,
) -> anyhow::Result<Vec<ProjectRow>> {
    let rows = client
        .query(
            "SELECT p.project_id, p.name, p.slug, p.description, p.created_by, \
                    p.created_at, p.updated_at, p.settings, p.deleted_at, \
                    COALESCE(p.delete_protection, FALSE) AS delete_protection \
             FROM project p \
             WHERE p.deleted_at IS NOT NULL \
               AND COALESCE(p.delete_protection, FALSE) = FALSE \
               AND p.deleted_at < now() - ($1::text || ' days')::interval \
             ORDER BY p.name",
            &[&days.to_string()],
        )
        .await?;

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
            deleted_at: r.get("deleted_at"),
            delete_protection: r.get("delete_protection"),
        })
        .collect())
}

/// Add a user to a project as pending (invitation).
/// Handles existing membership: active → AlreadyMember, pending → AlreadyPending,
/// denied → re-invite to pending.
pub async fn add_pending_member(
    client: &Client,
    project_id: &str,
    user_id: &Uuid,
    role: &str,
    invited_by: &Uuid,
) -> anyhow::Result<AddMemberResult> {
    let existing = client
        .query_opt(
            "SELECT status FROM project_member WHERE project_id = $1 AND user_id = $2",
            &[&project_id, user_id],
        )
        .await?;

    match existing {
        Some(row) => {
            let status: String = row.get("status");
            match status.as_str() {
                "active" => Ok(AddMemberResult::AlreadyMember),
                "pending_acceptance" => Ok(AddMemberResult::AlreadyPending),
                "denied" => {
                    // Re-invite: reset to pending
                    client
                        .execute(
                            "UPDATE project_member SET status = 'pending_acceptance', role = $3, \
                             invited_by = $4, joined_at = NOW() \
                             WHERE project_id = $1 AND user_id = $2",
                            &[&project_id, user_id, &role, invited_by],
                        )
                        .await?;
                    Ok(AddMemberResult::ReInvited)
                }
                _ => Ok(AddMemberResult::AlreadyMember),
            }
        }
        None => {
            client
                .execute(
                    "INSERT INTO project_member (project_id, user_id, role, invited_by, status) \
                     VALUES ($1, $2, $3, $4, 'pending_acceptance')",
                    &[&project_id, user_id, &role, invited_by],
                )
                .await?;
            Ok(AddMemberResult::Added)
        }
    }
}

/// Update invite_sent_at for a project member to NOW().
pub async fn update_invite_sent_at(
    client: &Client,
    project_id: &str,
    user_id: &Uuid,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE project_member SET invite_sent_at = NOW() WHERE project_id = $1 AND user_id = $2",
            &[&project_id, user_id],
        )
        .await?;
    Ok(())
}

/// Update a pending member's status (accept or deny).
/// Only transitions from 'pending_acceptance' to the given status.
#[allow(dead_code)] // Used by acceptance flow (Task 8)
pub async fn update_member_status(
    client: &Client,
    project_id: &str,
    user_id: &Uuid,
    new_status: &str,
) -> anyhow::Result<bool> {
    let rows = client
        .execute(
            "UPDATE project_member SET status = $3 \
             WHERE project_id = $1 AND user_id = $2 AND status = 'pending_acceptance'",
            &[&project_id, user_id, &new_status],
        )
        .await?;
    Ok(rows > 0)
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
    fn default_project_uuid_matches() {
        let expected: Uuid = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        assert_eq!(crate::auth::DEFAULT_PROJECT_UUID, expected);
    }

    #[test]
    fn default_project_id_is_14_chars() {
        let id = default_project_id();
        assert_eq!(id.len(), 14, "default project ID should be 14 chars: {id}");
        assert!(
            crate::project_id::ProjectId::validate(id),
            "default project ID should be a valid ProjectId: {id}"
        );
    }
}
