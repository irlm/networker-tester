use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// Simple progress reporter for benchmark execution.
#[derive(Debug, Clone)]
pub struct ProgressReporter {
    total: u32,
    completed: Arc<AtomicU32>,
    failed: Arc<AtomicU32>,
}

impl ProgressReporter {
    pub fn new(total: u32) -> Self {
        Self {
            total,
            completed: Arc::new(AtomicU32::new(0)),
            failed: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Mark one test case as successfully completed.
    pub fn tick(&self, label: &str) {
        let done = self.completed.fetch_add(1, Ordering::Relaxed) + 1;
        let fail = self.failed.load(Ordering::Relaxed);
        tracing::info!(
            "[{}/{}] {} (failed: {})",
            done + fail,
            self.total,
            label,
            fail
        );
    }

    /// Mark one test case as failed.
    pub fn fail(&self, label: &str, err: &dyn std::fmt::Display) {
        let fail = self.failed.fetch_add(1, Ordering::Relaxed) + 1;
        let done = self.completed.load(Ordering::Relaxed);
        tracing::error!("[{}/{}] FAIL {} — {}", done + fail, self.total, label, err);
    }

    /// Print final summary.
    pub fn finish(&self) {
        let done = self.completed.load(Ordering::Relaxed);
        let fail = self.failed.load(Ordering::Relaxed);
        tracing::info!(
            "Benchmark complete: {} passed, {} failed, {} total",
            done,
            fail,
            self.total
        );
    }

    #[allow(dead_code)]
    pub fn completed(&self) -> u32 {
        self.completed.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn failed(&self) -> u32 {
        self.failed.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn total(&self) -> u32 {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_counting() {
        let p = ProgressReporter::new(5);
        assert_eq!(p.total(), 5);
        assert_eq!(p.completed(), 0);
        assert_eq!(p.failed(), 0);

        p.tick("test-1");
        p.tick("test-2");
        p.fail("test-3", &"timeout");

        assert_eq!(p.completed(), 2);
        assert_eq!(p.failed(), 1);
    }
}
