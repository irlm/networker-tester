//! Tester lock primitives — the only authoritative writers of the
//! (allocation, locked_by_config_id) pair on `project_tester`.
//!
//! `release` and `force_release` are the ONLY functions allowed to clear
//! the pair. A grep-guard test in Task 7 enforces this invariant.

#![allow(dead_code)] // downstream tasks will wire these in

use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum AcquireOutcome {
    Acquired,
    NeedsStart,
    Transient(String),
    Upgrading,
    AlreadyLockedBy(Uuid),
    Errored,
    NotIdle(String),
}

pub async fn try_acquire(
    client: &Client,
    tester_id: &Uuid,
    config_id: &Uuid,
) -> anyhow::Result<AcquireOutcome> {
    let row = client
        .query_opt(
            r#"
            UPDATE project_tester
               SET allocation          = 'locked',
                   locked_by_config_id = $2,
                   last_used_at        = NOW(),
                   updated_at          = NOW()
             WHERE tester_id           = $1
               AND power_state         = 'running'
               AND allocation          = 'idle'
               AND locked_by_config_id IS NULL
             RETURNING tester_id
            "#,
            &[tester_id, config_id],
        )
        .await?;

    if row.is_some() {
        return Ok(AcquireOutcome::Acquired);
    }

    let cur = client
        .query_one(
            "SELECT power_state, allocation, locked_by_config_id \
             FROM project_tester WHERE tester_id = $1",
            &[tester_id],
        )
        .await?;
    let power: String = cur.get(0);
    let alloc: String = cur.get(1);
    let locker: Option<Uuid> = cur.get(2);

    Ok(match (power.as_str(), alloc.as_str()) {
        ("stopped", _) => AcquireOutcome::NeedsStart,
        ("starting" | "stopping" | "provisioning", _) => AcquireOutcome::Transient(power),
        ("running", "locked") => AcquireOutcome::AlreadyLockedBy(locker.unwrap_or_else(Uuid::nil)),
        ("running", "upgrading") => AcquireOutcome::Upgrading,
        ("error", _) => AcquireOutcome::Errored,
        _ => AcquireOutcome::NotIdle(format!("{power}/{alloc}")),
    })
}

pub async fn release(client: &Client, tester_id: &Uuid, config_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            r#"
            UPDATE project_tester
               SET allocation          = 'idle',
                   locked_by_config_id = NULL,
                   updated_at          = NOW()
             WHERE tester_id           = $1
               AND locked_by_config_id = $2
            "#,
            &[tester_id, config_id],
        )
        .await?;
    Ok(())
}

pub async fn try_power_transition(
    client: &Client,
    tester_id: &Uuid,
    expected: &str,
    next: &str,
) -> anyhow::Result<bool> {
    let rows = client
        .execute(
            r#"
            UPDATE project_tester
               SET power_state = $3,
                   updated_at  = NOW()
             WHERE tester_id   = $1
               AND power_state = $2
            "#,
            &[tester_id, &expected, &next],
        )
        .await?;
    Ok(rows == 1)
}

pub async fn set_status_message(
    client: &Client,
    tester_id: &Uuid,
    msg: &str,
) -> anyhow::Result<()> {
    client
        .execute(
            "UPDATE project_tester SET status_message = $2, updated_at = NOW() WHERE tester_id = $1",
            &[tester_id, &msg],
        )
        .await?;
    Ok(())
}

/// Clear the lock pair unconditionally. ONLY the recovery loop on
/// dashboard restart (Task 12) is allowed to call this.
pub async fn force_release(client: &Client, tester_id: &Uuid) -> anyhow::Result<()> {
    client
        .execute(
            r#"
            UPDATE project_tester
               SET allocation          = 'idle',
                   locked_by_config_id = NULL,
                   updated_at          = NOW()
             WHERE tester_id           = $1
            "#,
            &[tester_id],
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Grep guard: only `tester_state.rs` is allowed to write the unlock pair
    /// `allocation='idle'` + `locked_by_config_id=NULL`. Any other file in the
    /// services tree that contains a literal `allocation = 'idle'` in a WRITE context
    /// (i.e., after SET) is breaking the invariant and must route through
    /// `release`/`force_release`. Legitimate reads (in WHERE or SELECT) are allowed.
    #[test]
    fn release_is_only_writer_of_idle_unlock() {
        use std::fs;
        use std::path::PathBuf;

        let services_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/services");
        assert!(
            services_dir.is_dir(),
            "services dir not found: {services_dir:?}"
        );

        let needles = [
            "allocation = 'idle'",
            "allocation='idle'",
            "allocation= 'idle'",
            "allocation ='idle'",
            "allocation  = 'idle'",
            "allocation =  'idle'",
            "allocation\t=\t'idle'",
        ];

        /// Check if a needle at the given byte position is in a write context
        /// (preceded by SET) rather than a read context (preceded by WHERE/SELECT).
        fn is_write_context(body: &str, needle_start: usize) -> bool {
            // Look back up to 200 chars for the most recent SET or WHERE.
            let window_start = needle_start.saturating_sub(200);
            let prefix = &body[window_start..needle_start];

            // Find the LAST occurrence of "SET " or "WHERE " in the prefix (case-insensitive).
            let prefix_upper = prefix.to_ascii_uppercase();
            let set_pos = prefix_upper.rfind("SET ");
            let where_pos = prefix_upper.rfind("WHERE ");

            match (set_pos, where_pos) {
                (Some(s), Some(w)) => s > w, // SET is more recent → write
                (Some(_), None) => true,     // only SET → write
                (None, _) => false,          // only WHERE (or neither) → read
            }
        }

        let mut offenders: Vec<String> = Vec::new();

        fn visit(dir: &std::path::Path, offenders: &mut Vec<String>, needles: &[&str]) {
            for entry in fs::read_dir(dir).expect("read_dir services") {
                let entry = entry.expect("dir entry");
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, offenders, needles);
                    continue;
                }
                if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                    continue;
                }
                // Skip tester_state.rs itself — it is the authorised writer.
                if path.file_name().and_then(|s| s.to_str()) == Some("tester_state.rs") {
                    continue;
                }
                let body = fs::read_to_string(&path).expect("read file");
                for needle in needles {
                    let mut search_start = 0;
                    while let Some(pos) = body[search_start..].find(needle) {
                        let abs_pos = search_start + pos;
                        // Check if this occurrence is in a write context
                        if is_write_context(&body, abs_pos) {
                            offenders.push(format!(
                                "{}: contains {:?} in write context (line approx. {})",
                                path.display(),
                                needle,
                                body[..abs_pos].matches('\n').count() + 1
                            ));
                        }
                        search_start = abs_pos + needle.len();
                    }
                }
            }
        }

        visit(&services_dir, &mut offenders, &needles);

        assert!(
            offenders.is_empty(),
            "Only tester_state.rs may write `allocation='idle'`. Offenders:\n{}",
            offenders.join("\n")
        );
    }

    /// Race test: two concurrent `try_acquire` calls against the same tester
    /// must result in exactly one `Acquired` outcome. Gated by `#[ignore]`
    /// because it requires a live Postgres instance reachable via
    /// `DASHBOARD_DB_URL` with the V027 schema applied.
    #[tokio::test]
    #[ignore]
    async fn concurrent_acquires_only_one_wins() -> anyhow::Result<()> {
        use crate::project_id::ProjectId;
        use tokio_postgres::NoTls;

        let url =
            std::env::var("DASHBOARD_DB_URL").expect("DASHBOARD_DB_URL must be set for this test");

        let (setup, setup_conn) = tokio_postgres::connect(&url, NoTls).await?;
        tokio::spawn(async move {
            let _ = setup_conn.await;
        });

        // Pick an existing user so the FK on created_by resolves.
        let user_row = setup
            .query_one(
                "SELECT user_id FROM dash_user ORDER BY created_at LIMIT 1",
                &[],
            )
            .await?;
        let created_by: Uuid = user_row.get(0);

        // Unique project so parallel runs don't collide. Since V024b,
        // project.project_id is CHAR(14) populated by the base36 ProjectId
        // generator; binding a Uuid here would fail the pg type check.
        let project_id = ProjectId::generate("us", "tst");
        let project_id_str: &str = project_id.as_str();
        let suffix = Uuid::new_v4().simple().to_string();
        let project_name = format!("tester-state-race-{}", &suffix[..8]);
        let project_slug = format!("tester-state-race-{}", &suffix[..8]);

        setup
            .execute(
                "INSERT INTO project (project_id, name, slug, created_by) \
                 VALUES ($1, $2, $3, $4)",
                &[&project_id_str, &project_name, &project_slug, &created_by],
            )
            .await?;

        let tester_row = setup
            .query_one(
                r#"
                INSERT INTO project_tester
                    (project_id, name, cloud, region, power_state, allocation, created_by)
                VALUES ($1, $2, 'azure', 'eastus', 'running', 'idle', $3)
                RETURNING tester_id
                "#,
                &[&project_id_str, &"race-1", &created_by],
            )
            .await?;
        let tester_id: Uuid = tester_row.get(0);

        // Two distinct config ids so we can tell which racer won.
        let config_a = Uuid::new_v4();
        let config_b = Uuid::new_v4();

        for (cfg, name) in [(&config_a, "race-cfg-a"), (&config_b, "race-cfg-b")] {
            setup
                .execute(
                    "INSERT INTO benchmark_config \
                     (config_id, project_id, name, status, created_by) \
                     VALUES ($1, $2, $3, 'draft', $4)",
                    &[cfg, &project_id_str, &name, &created_by],
                )
                .await?;
        }

        // Each racer gets its own connection so the UPDATEs contend in PG.
        let (client_a, conn_a) = tokio_postgres::connect(&url, NoTls).await?;
        tokio::spawn(async move {
            let _ = conn_a.await;
        });
        let (client_b, conn_b) = tokio_postgres::connect(&url, NoTls).await?;
        tokio::spawn(async move {
            let _ = conn_b.await;
        });

        let (a, b) = tokio::join!(
            try_acquire(&client_a, &tester_id, &config_a),
            try_acquire(&client_b, &tester_id, &config_b),
        );
        let a = a?;
        let b = b?;

        let acquired_count = [&a, &b]
            .iter()
            .filter(|o| matches!(o, AcquireOutcome::Acquired))
            .count();

        // Cleanup: release whichever won, then delete the rows we created.
        if matches!(a, AcquireOutcome::Acquired) {
            release(&setup, &tester_id, &config_a).await?;
        } else if matches!(b, AcquireOutcome::Acquired) {
            release(&setup, &tester_id, &config_b).await?;
        }
        setup
            .execute(
                "DELETE FROM benchmark_config WHERE project_id = $1",
                &[&project_id_str],
            )
            .await?;
        setup
            .execute(
                "DELETE FROM project_tester WHERE project_id = $1",
                &[&project_id_str],
            )
            .await?;
        setup
            .execute(
                "DELETE FROM project WHERE project_id = $1",
                &[&project_id_str],
            )
            .await?;

        assert_eq!(
            acquired_count, 1,
            "expected exactly one racer to win, got a={a:?} b={b:?}"
        );
        Ok(())
    }
}
