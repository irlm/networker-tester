use serde::Serialize;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Thresholds used by [`MetricsSnapshot::status`].
const DEGRADED_DROP_RATIO: f64 = 0.01; // 1 % drop rate → degraded
const FAILING_DROP_RATIO: f64 = 0.10; // 10 % drop rate → failing
const FAILING_ERROR_RATIO: f64 = 0.05; // 5 % flush-error rate → failing
const FAILING_LATENCY_MS: u64 = 5_000; // 5 s last-flush latency → failing
const DEGRADED_LATENCY_MS: u64 = 1_000; // 1 s last-flush latency → degraded

/// Atomic counters shared between the log pipeline and the metrics endpoint.
///
/// All fields use `Relaxed` ordering — these are advisory diagnostics, not
/// synchronisation primitives.
#[derive(Debug, Default)]
pub struct LogPipelineMetrics {
    /// Total log entries successfully written to the sink.
    pub entries_written: AtomicU64,
    /// Total log entries dropped (buffer full, shutdown, etc.).
    pub entries_dropped: AtomicU64,
    /// Total number of flush operations attempted.
    pub flush_count: AtomicU64,
    /// Number of flush operations that returned an error.
    pub flush_errors: AtomicU64,
    /// Wall-clock duration of the **last** flush in milliseconds.
    pub last_flush_ms: AtomicU64,
    /// Current number of entries waiting in the in-memory queue.
    pub queue_depth: AtomicU32,
}

impl LogPipelineMetrics {
    /// Capture an immutable [`MetricsSnapshot`] at this instant.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            entries_written: self.entries_written.load(Ordering::Relaxed),
            entries_dropped: self.entries_dropped.load(Ordering::Relaxed),
            flush_count: self.flush_count.load(Ordering::Relaxed),
            flush_errors: self.flush_errors.load(Ordering::Relaxed),
            last_flush_ms: self.last_flush_ms.load(Ordering::Relaxed),
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time, serialisable copy of [`LogPipelineMetrics`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetricsSnapshot {
    pub entries_written: u64,
    pub entries_dropped: u64,
    pub flush_count: u64,
    pub flush_errors: u64,
    /// Duration of the last flush in milliseconds.
    pub last_flush_ms: u64,
    pub queue_depth: u32,
}

impl MetricsSnapshot {
    /// Overall health status of the log pipeline.
    ///
    /// | Return value | Meaning |
    /// |---|---|
    /// | `"healthy"` | All metrics within normal bounds |
    /// | `"degraded"` | Elevated drops or slow flushes |
    /// | `"failing"` | High drop/error rate or very slow flushes |
    pub fn status(&self) -> &'static str {
        let total = self.entries_written + self.entries_dropped;
        let drop_ratio = if total > 0 {
            self.entries_dropped as f64 / total as f64
        } else {
            0.0
        };

        let error_ratio = if self.flush_count > 0 {
            self.flush_errors as f64 / self.flush_count as f64
        } else {
            0.0
        };

        // Failing conditions (checked first — most severe)
        if drop_ratio >= FAILING_DROP_RATIO
            || error_ratio >= FAILING_ERROR_RATIO
            || self.last_flush_ms >= FAILING_LATENCY_MS
        {
            return "failing";
        }

        // Degraded conditions
        if drop_ratio >= DEGRADED_DROP_RATIO || self.last_flush_ms >= DEGRADED_LATENCY_MS {
            return "degraded";
        }

        "healthy"
    }
}

/// Convenience wrapper for sharing metrics across tasks.
pub type SharedMetrics = Arc<LogPipelineMetrics>;

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(
        written: u64,
        dropped: u64,
        flush_count: u64,
        flush_errors: u64,
        last_flush_ms: u64,
    ) -> MetricsSnapshot {
        MetricsSnapshot {
            entries_written: written,
            entries_dropped: dropped,
            flush_count,
            flush_errors,
            last_flush_ms,
            queue_depth: 0,
        }
    }

    // ── Default / healthy ─────────────────────────────────────────────────────

    #[test]
    fn default_metrics_are_zero() {
        let m = LogPipelineMetrics::default();
        let s = m.snapshot();
        assert_eq!(s.entries_written, 0);
        assert_eq!(s.entries_dropped, 0);
        assert_eq!(s.flush_count, 0);
        assert_eq!(s.flush_errors, 0);
        assert_eq!(s.last_flush_ms, 0);
        assert_eq!(s.queue_depth, 0);
    }

    #[test]
    fn zero_metrics_are_healthy() {
        let s = snapshot(0, 0, 0, 0, 0);
        assert_eq!(s.status(), "healthy");
    }

    #[test]
    fn normal_traffic_is_healthy() {
        // 1000 written, 0 dropped, fast flushes
        let s = snapshot(1000, 0, 50, 0, 200);
        assert_eq!(s.status(), "healthy");
    }

    // ── Degraded ──────────────────────────────────────────────────────────────

    #[test]
    fn drop_rate_above_1pct_is_degraded() {
        // 99 written + 2 dropped = 101 total; 2/101 ≈ 1.98 % → degraded
        let s = snapshot(99, 2, 10, 0, 0);
        assert_eq!(s.status(), "degraded");
    }

    #[test]
    fn slow_flush_above_1s_is_degraded() {
        let s = snapshot(1000, 0, 50, 0, 1_500);
        assert_eq!(s.status(), "degraded");
    }

    #[test]
    fn drop_rate_exactly_1pct_is_degraded() {
        // 99 written + 1 dropped = 100 total; exactly 1 %
        let s = snapshot(99, 1, 10, 0, 0);
        assert_eq!(s.status(), "degraded");
    }

    // ── Failing ───────────────────────────────────────────────────────────────

    #[test]
    fn drop_rate_above_10pct_is_failing() {
        // 89 written + 11 dropped = 100 total; 11 % → failing
        let s = snapshot(89, 11, 10, 0, 0);
        assert_eq!(s.status(), "failing");
    }

    #[test]
    fn flush_error_rate_above_5pct_is_failing() {
        // 94 flushes OK, 6 errors → 6/100 = 6 % → failing
        let s = snapshot(1000, 0, 100, 6, 0);
        assert_eq!(s.status(), "failing");
    }

    #[test]
    fn very_slow_flush_above_5s_is_failing() {
        let s = snapshot(1000, 0, 50, 0, 5_000);
        assert_eq!(s.status(), "failing");
    }

    #[test]
    fn flush_error_rate_exactly_5pct_is_failing() {
        // 95 + 5 = 100; exactly 5 %
        let s = snapshot(1000, 0, 100, 5, 0);
        assert_eq!(s.status(), "failing");
    }

    // ── Atomic counters increment correctly ───────────────────────────────────

    #[test]
    fn atomic_counters_increment() {
        let m = LogPipelineMetrics::default();
        m.entries_written.fetch_add(42, Ordering::Relaxed);
        m.entries_dropped.fetch_add(3, Ordering::Relaxed);
        m.flush_count.fetch_add(10, Ordering::Relaxed);
        m.flush_errors.fetch_add(1, Ordering::Relaxed);
        m.last_flush_ms.store(250, Ordering::Relaxed);
        m.queue_depth.store(7, Ordering::Relaxed);

        let s = m.snapshot();
        assert_eq!(s.entries_written, 42);
        assert_eq!(s.entries_dropped, 3);
        assert_eq!(s.flush_count, 10);
        assert_eq!(s.flush_errors, 1);
        assert_eq!(s.last_flush_ms, 250);
        assert_eq!(s.queue_depth, 7);
    }

    // ── Serialisation ─────────────────────────────────────────────────────────

    #[test]
    fn snapshot_serialises_to_json() {
        let s = snapshot(100, 2, 5, 0, 300);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"entries_written\":100"));
        assert!(json.contains("\"entries_dropped\":2"));
    }

    // ── SharedMetrics alias ───────────────────────────────────────────────────

    #[test]
    fn shared_metrics_arc_clone() {
        let m: SharedMetrics = Arc::new(LogPipelineMetrics::default());
        let m2 = Arc::clone(&m);
        m.entries_written.fetch_add(1, Ordering::Relaxed);
        assert_eq!(m2.entries_written.load(Ordering::Relaxed), 1);
    }
}
