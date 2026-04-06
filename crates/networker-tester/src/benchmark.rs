use crate::cli::ResolvedConfig;
use crate::metrics::{
    attempt_payload_bytes, primary_metric_value, BenchmarkExecutionPlan, RequestAttempt,
};
use std::cmp::Ordering;
use std::collections::BTreeMap;

pub const ADAPTIVE_BOOTSTRAP_RESAMPLES: usize = 1_024;
pub const ADAPTIVE_CONFIDENCE_LEVEL: f64 = 0.95;
pub const DEFAULT_AUTO_TARGET_RELATIVE_ERROR: f64 = 0.05;
pub const DEFAULT_PILOT_MIN_SAMPLES: u32 = 6;
pub const DEFAULT_PILOT_MAX_SAMPLES: u32 = 12;
pub const DEFAULT_PILOT_MIN_DURATION_MS: u64 = 0;
pub const DEFAULT_OVERHEAD_SAMPLES: u32 = 1;
pub const DEFAULT_COOLDOWN_SAMPLES: u32 = 1;

#[derive(Debug, Clone, Copy)]
pub struct BenchmarkAdaptiveCriteria {
    pub min_samples: u32,
    pub max_samples: u32,
    pub min_duration_ms: u64,
    pub target_relative_error: Option<f64>,
    pub target_absolute_error: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct BenchmarkPilotCriteria {
    pub min_samples: u32,
    pub max_samples: u32,
    pub min_duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchmarkAdaptiveStopReason {
    AccuracyTargetReached,
    MaxSamplesReached,
}

#[derive(Debug, Clone)]
pub struct BenchmarkAdaptiveStatus {
    pub completed_samples: u32,
    pub elapsed_ms: f64,
    pub stop_reason: Option<BenchmarkAdaptiveStopReason>,
}

#[derive(Debug, Clone, Copy)]
pub struct MedianErrorBounds {
    pub median: f64,
    pub absolute_half_width: f64,
}

#[derive(Debug, Clone)]
pub struct DeterministicRng {
    pub state: u64,
}

impl DeterministicRng {
    pub fn from_values(values: &[f64]) -> Self {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ values.len() as u64;
        for value in values {
            state ^= value.to_bits().wrapping_mul(0xbf58_476d_1ce4_e5b9);
            state = state.rotate_left(13);
        }
        if state == 0 {
            state = 0x94d0_49bb_1331_11eb;
        }
        Self { state }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    pub fn next_index(&mut self, upper: usize) -> usize {
        (self.next_u64() as usize) % upper
    }
}

pub fn benchmark_pilot_criteria(cfg: &ResolvedConfig) -> Option<BenchmarkPilotCriteria> {
    if !cfg.benchmark_mode || cfg.benchmark_phase != "measured" || !cfg.http_stacks.is_empty() {
        return None;
    }

    let pilot_requested = cfg.benchmark_pilot_min_samples.is_some()
        || cfg.benchmark_pilot_max_samples.is_some()
        || cfg.benchmark_pilot_min_duration_ms.is_some();
    let no_explicit_measured_controls = cfg.benchmark_min_samples.is_none()
        && cfg.benchmark_max_samples.is_none()
        && cfg.benchmark_min_duration_ms.is_none()
        && cfg.benchmark_target_relative_error.is_none()
        && cfg.benchmark_target_absolute_error.is_none();
    if !pilot_requested && !no_explicit_measured_controls {
        return None;
    }

    let default_pilot_max = cfg.runs.clamp(1, DEFAULT_PILOT_MAX_SAMPLES);
    let default_pilot_min = default_pilot_max.min(DEFAULT_PILOT_MIN_SAMPLES);

    Some(BenchmarkPilotCriteria {
        min_samples: cfg.benchmark_pilot_min_samples.unwrap_or(default_pilot_min),
        max_samples: cfg.benchmark_pilot_max_samples.unwrap_or(default_pilot_max),
        min_duration_ms: cfg
            .benchmark_pilot_min_duration_ms
            .unwrap_or(DEFAULT_PILOT_MIN_DURATION_MS),
    })
}

pub fn benchmark_adaptive_criteria(cfg: &ResolvedConfig) -> Option<BenchmarkAdaptiveCriteria> {
    let controls_requested = cfg.benchmark_min_samples.is_some()
        || cfg.benchmark_max_samples.is_some()
        || cfg.benchmark_min_duration_ms.is_some()
        || cfg.benchmark_target_relative_error.is_some()
        || cfg.benchmark_target_absolute_error.is_some();
    if !cfg.benchmark_mode || cfg.benchmark_phase != "measured" || !controls_requested {
        return None;
    }

    Some(BenchmarkAdaptiveCriteria {
        min_samples: cfg.benchmark_min_samples.unwrap_or(cfg.runs),
        max_samples: cfg.benchmark_max_samples.unwrap_or(cfg.runs),
        min_duration_ms: cfg.benchmark_min_duration_ms.unwrap_or(0),
        target_relative_error: cfg.benchmark_target_relative_error,
        target_absolute_error: cfg.benchmark_target_absolute_error,
    })
}

pub fn derive_measured_plan_from_pilot(
    cfg: &ResolvedConfig,
    pilot_attempts: &[RequestAttempt],
) -> BenchmarkExecutionPlan {
    let pilot_status = benchmark_adaptive_status(
        &BenchmarkAdaptiveCriteria {
            min_samples: 1,
            max_samples: u32::MAX,
            min_duration_ms: 0,
            target_relative_error: None,
            target_absolute_error: None,
        },
        pilot_attempts,
    );
    let target_relative_error = cfg
        .benchmark_target_relative_error
        .or(Some(DEFAULT_AUTO_TARGET_RELATIVE_ERROR));
    let target_absolute_error = cfg.benchmark_target_absolute_error;

    let mut values_by_case: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for attempt in pilot_attempts {
        if attempt.success {
            if let Some(value) = primary_metric_value(attempt) {
                values_by_case
                    .entry(adaptive_case_id(attempt))
                    .or_default()
                    .push(value);
            }
        }
    }

    let estimated_max_samples = if values_by_case.is_empty() {
        cfg.runs
    } else {
        values_by_case
            .values()
            .map(|values| {
                estimated_samples_for_error_targets(
                    values,
                    target_relative_error,
                    target_absolute_error,
                )
            })
            .max()
            .unwrap_or(cfg.runs)
            .clamp(1, cfg.runs.max(1))
    };

    let min_samples = cfg
        .benchmark_min_samples
        .unwrap_or(pilot_status.completed_samples.max(1));
    let max_samples = cfg.benchmark_max_samples.unwrap_or(
        estimated_max_samples
            .max(min_samples)
            .min(cfg.runs.max(min_samples)),
    );
    let min_duration_ms = cfg
        .benchmark_min_duration_ms
        .unwrap_or(pilot_status.elapsed_ms.ceil().clamp(0.0, u64::MAX as f64) as u64);
    let source = if cfg.benchmark_min_samples.is_none()
        && cfg.benchmark_max_samples.is_none()
        && cfg.benchmark_min_duration_ms.is_none()
        && cfg.benchmark_target_relative_error.is_none()
        && cfg.benchmark_target_absolute_error.is_none()
    {
        "pilot-derived"
    } else {
        "pilot-assisted"
    };

    BenchmarkExecutionPlan {
        source: source.to_string(),
        min_samples,
        max_samples,
        min_duration_ms,
        target_relative_error,
        target_absolute_error,
        pilot_sample_count: pilot_status.completed_samples,
        pilot_elapsed_ms: Some(pilot_status.elapsed_ms),
    }
}

pub fn benchmark_adaptive_status(
    criteria: &BenchmarkAdaptiveCriteria,
    attempts: &[RequestAttempt],
) -> BenchmarkAdaptiveStatus {
    let elapsed_ms = benchmark_attempt_wall_time_ms(attempts);
    let mut samples_by_case: BTreeMap<String, usize> = BTreeMap::new();
    let mut values_by_case: BTreeMap<String, Vec<f64>> = BTreeMap::new();

    for attempt in attempts {
        let case_id = adaptive_case_id(attempt);
        *samples_by_case.entry(case_id.clone()).or_default() += 1;
        if attempt.success {
            if let Some(value) = primary_metric_value(attempt) {
                values_by_case.entry(case_id).or_default().push(value);
            }
        }
    }

    let completed_samples = samples_by_case
        .values()
        .min()
        .copied()
        .unwrap_or_default()
        .try_into()
        .unwrap_or(u32::MAX);
    let min_samples_satisfied = completed_samples >= criteria.min_samples;
    let min_duration_satisfied = elapsed_ms >= criteria.min_duration_ms as f64;
    let requires_accuracy_target =
        criteria.target_relative_error.is_some() || criteria.target_absolute_error.is_some();
    let accuracy_satisfied = if requires_accuracy_target {
        !samples_by_case.is_empty()
            && samples_by_case.keys().all(|case_id| {
                values_by_case.get(case_id).is_some_and(|values| {
                    median_error_bounds(values).is_some_and(|error_bounds| {
                        let relative_ok = criteria.target_relative_error.is_none_or(|target| {
                            error_bounds.median.abs() > f64::EPSILON
                                && error_bounds.absolute_half_width / error_bounds.median.abs()
                                    <= target
                        });
                        let absolute_ok = criteria
                            .target_absolute_error
                            .is_none_or(|target| error_bounds.absolute_half_width <= target);
                        relative_ok && absolute_ok
                    })
                })
            })
    } else {
        true
    };

    let stop_reason = if completed_samples >= criteria.max_samples {
        Some(BenchmarkAdaptiveStopReason::MaxSamplesReached)
    } else if min_samples_satisfied && min_duration_satisfied && accuracy_satisfied {
        Some(BenchmarkAdaptiveStopReason::AccuracyTargetReached)
    } else {
        None
    };

    BenchmarkAdaptiveStatus {
        completed_samples,
        elapsed_ms,
        stop_reason,
    }
}

pub fn adaptive_case_id(attempt: &RequestAttempt) -> String {
    let payload = attempt_payload_bytes(attempt)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "default".into());
    let stack = attempt
        .http_stack
        .as_deref()
        .unwrap_or("default")
        .replace(':', "_");
    format!("{}:{}:{}", attempt.protocol, payload, stack)
}

pub fn benchmark_attempt_wall_time_ms(attempts: &[RequestAttempt]) -> f64 {
    let start = attempts.iter().map(|attempt| attempt.started_at).min();
    let end = attempts
        .iter()
        .map(|attempt| attempt.finished_at.unwrap_or(attempt.started_at))
        .max();
    match (start, end) {
        (Some(start), Some(end)) => (end - start)
            .num_microseconds()
            .map(|micros| micros as f64 / 1000.0)
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn median_error_bounds(values: &[f64]) -> Option<MedianErrorBounds> {
    let mut sorted = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    if sorted.len() < 2 {
        return None;
    }
    let median = percentile_from_sorted(&sorted, 50.0);
    let (_, lower, upper) = bootstrap_median_interval(&sorted);
    Some(MedianErrorBounds {
        median,
        absolute_half_width: (upper - lower) / 2.0,
    })
}

pub fn estimated_samples_for_error_targets(
    values: &[f64],
    target_relative_error: Option<f64>,
    target_absolute_error: Option<f64>,
) -> u32 {
    let current_n = values.len().clamp(1, u32::MAX as usize) as f64;
    let Some(error_bounds) = median_error_bounds(values) else {
        return current_n as u32;
    };

    let mut estimated = current_n;
    if let Some(target) = target_relative_error {
        if error_bounds.median.abs() > f64::EPSILON {
            let current_relative_error =
                error_bounds.absolute_half_width / error_bounds.median.abs();
            estimated = estimated.max(current_n * (current_relative_error / target).powi(2));
        }
    }
    if let Some(target) = target_absolute_error {
        estimated = estimated.max(current_n * (error_bounds.absolute_half_width / target).powi(2));
    }

    estimated.ceil().clamp(1.0, u32::MAX as f64) as u32
}

pub fn median_from_sorted(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        0.0
    } else if sorted.len().is_multiple_of(2) {
        let upper = sorted.len() / 2;
        (sorted[upper - 1] + sorted[upper]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    }
}

pub fn bootstrap_median_interval(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    if values.len() == 1 {
        return (0.0, values[0], values[0]);
    }

    let mut rng = DeterministicRng::from_values(values);
    let mut estimates = Vec::with_capacity(ADAPTIVE_BOOTSTRAP_RESAMPLES);
    for _ in 0..ADAPTIVE_BOOTSTRAP_RESAMPLES {
        let mut sample = Vec::with_capacity(values.len());
        for _ in 0..values.len() {
            sample.push(values[rng.next_index(values.len())]);
        }
        sample.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        estimates.push(median_from_sorted(&sample));
    }

    estimates.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let estimate_mean = estimates.iter().sum::<f64>() / estimates.len() as f64;
    let estimate_variance = estimates
        .iter()
        .map(|value| (value - estimate_mean).powi(2))
        .sum::<f64>()
        / (estimates.len() as f64 - 1.0);
    let standard_error = estimate_variance.sqrt();
    let tail = (1.0 - ADAPTIVE_CONFIDENCE_LEVEL) * 50.0;
    let lower = percentile_from_sorted(&estimates, tail);
    let upper = percentile_from_sorted(&estimates, 100.0 - tail);
    (standard_error, lower, upper)
}

pub fn percentile_from_sorted(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }

    let rank = percentile / 100.0 * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo as f64)
    }
}
