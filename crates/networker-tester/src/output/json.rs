use crate::metrics::TestRun;
use std::path::Path;

/// Serialise a `TestRun` to pretty-printed JSON and write to `path`.
pub fn save(run: &TestRun, path: &Path) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_string_pretty(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Return the JSON string without writing to disk (useful for testing).
pub fn to_string(run: &TestRun) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(run)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{Protocol, RequestAttempt, TestRun};
    use chrono::Utc;
    use tempfile::NamedTempFile;
    use uuid::Uuid;

    fn dummy_run() -> TestRun {
        let run_id = Uuid::new_v4();
        TestRun {
            run_id,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            target_url: "http://localhost/health".into(),
            target_host: "localhost".into(),
            modes: vec!["http1".into()],
            total_runs: 1,
            concurrency: 1,
            timeout_ms: 5000,
            client_os: "test".into(),
            client_version: "0.1.0".into(),
            attempts: vec![RequestAttempt {
                attempt_id: Uuid::new_v4(),
                run_id,
                protocol: Protocol::Http1,
                sequence_num: 0,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                success: true,
                dns: None,
                tcp: None,
                tls: None,
                http: None,
                udp: None,
                error: None,
                retry_count: 0,
                server_timing: None,
                udp_throughput: None,
            }],
        }
    }

    #[test]
    fn json_round_trip() {
        let run = dummy_run();
        let json = to_string(&run).unwrap();
        let de: TestRun = serde_json::from_str(&json).unwrap();
        assert_eq!(de.run_id, run.run_id);
        assert_eq!(de.attempts.len(), 1);
    }

    #[test]
    fn save_creates_file() {
        let tmp = NamedTempFile::new().unwrap();
        let run = dummy_run();
        save(&run, tmp.path()).unwrap();
        let contents = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(contents.contains("\"target_url\""));
    }
}
