//! `Server-Timing` header construction (contract v1 §4).
//!
//! Constraints enforced: <= 8 metrics, header value <= 512 bytes, metric names
//! `[a-z0-9-]{1,32}`, `dur` in ms with <= 3 decimal places.

const MAX_METRICS: usize = 8;
const MAX_HEADER_BYTES: usize = 512;

/// A single `Server-Timing` metric (`name;dur=<ms>`).
#[derive(Clone, Debug)]
pub struct Metric {
    name: String,
    dur_ms: f64,
}

impl Metric {
    /// Create a metric. `name` is validated against `[a-z0-9-]{1,32}`; an
    /// invalid name or a negative/non-finite duration yields `None` so bad
    /// input never produces a malformed header.
    pub fn new(name: impl Into<String>, dur_ms: f64) -> Option<Self> {
        let name = name.into();
        if !valid_name(&name) || !dur_ms.is_finite() || dur_ms < 0.0 {
            return None;
        }
        Some(Self { name, dur_ms })
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Format a duration with at most 3 decimal places, trimming trailing zeros.
fn fmt_dur(ms: f64) -> String {
    // Round to 3 decimals then strip trailing zeros / dot.
    let rounded = (ms * 1000.0).round() / 1000.0;
    let mut s = format!("{rounded:.3}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

/// Build a `Server-Timing` header value from an ordered metric list, honoring
/// the metric-count and byte-length caps (contract §4.1). Metrics that would
/// overflow either cap are dropped from the tail.
pub fn build_header(metrics: &[Metric]) -> String {
    let mut out = String::new();
    for m in metrics.iter().take(MAX_METRICS) {
        let piece = format!("{};dur={}", m.name, fmt_dur(m.dur_ms));
        let added = if out.is_empty() {
            piece.len()
        } else {
            piece.len() + 2 // ", "
        };
        if out.len() + added > MAX_HEADER_BYTES {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(&piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dur_formatting_trims_and_rounds() {
        assert_eq!(fmt_dur(12.3), "12.3");
        assert_eq!(fmt_dur(0.0), "0");
        assert_eq!(fmt_dur(57.1), "57.1");
        assert_eq!(fmt_dur(41.0), "41");
        assert_eq!(fmt_dur(1.23456), "1.235");
    }

    #[test]
    fn header_caps_metric_count() {
        let metrics: Vec<Metric> = (0..12)
            .map(|i| Metric::new(format!("m{i}"), i as f64).unwrap())
            .collect();
        let header = build_header(&metrics);
        assert_eq!(header.matches(';').count(), MAX_METRICS);
    }

    #[test]
    fn header_within_byte_budget() {
        let metrics: Vec<Metric> = (0..8)
            .map(|i| Metric::new(format!("metric-name-{i:02}"), 12345.678).unwrap())
            .collect();
        assert!(build_header(&metrics).len() <= MAX_HEADER_BYTES);
    }

    #[test]
    fn rejects_bad_names_and_durations() {
        assert!(Metric::new("APP", 1.0).is_none());
        assert!(Metric::new("app_x", 1.0).is_none());
        assert!(Metric::new("app", -1.0).is_none());
        assert!(Metric::new("app", f64::NAN).is_none());
        assert!(Metric::new("app", 1.0).is_some());
    }
}
