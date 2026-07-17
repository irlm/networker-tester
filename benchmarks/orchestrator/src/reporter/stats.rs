//! Statistical primitives for benchmark summaries: percentile interpolation,
//! deterministic bootstrap confidence intervals, Tukey fences, quality tiers,
//! and effect sizes. Golden tests in `super::tests` pin this math.

use crate::types::MetricSummary;
use std::cmp::Ordering;

pub(super) const REPORT_CONFIDENCE_LEVEL: f64 = 0.95;
const TUKEY_FENCE_MULTIPLIER: f64 = 1.5;
const BOOTSTRAP_RESAMPLES: usize = 2_048;
#[derive(Debug, Clone)]
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn from_values(values: &[f64]) -> Self {
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

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn next_index(&mut self, upper: usize) -> usize {
        (self.next_u64() as usize) % upper
    }
}

pub(super) fn percentile_from_sorted(sorted: &[f64], percentile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }

    let rank = percentile / 100.0 * (sorted.len() - 1) as f64;
    let lower_index = rank.floor() as usize;
    let upper_index = rank.ceil() as usize;
    if lower_index == upper_index {
        sorted[lower_index]
    } else {
        let lower = sorted[lower_index];
        let upper = sorted[upper_index];
        lower + (upper - lower) * (rank - lower_index as f64)
    }
}

pub(super) fn median_from_sorted(sorted: &[f64]) -> f64 {
    if sorted.is_empty() {
        0.0
    } else if sorted.len().is_multiple_of(2) {
        let upper = sorted.len() / 2;
        (sorted[upper - 1] + sorted[upper]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    }
}

pub(super) fn quality_tier_for_cv(cv: f64) -> String {
    if !cv.is_finite() {
        "unknown".to_string()
    } else if cv <= 0.03 {
        "excellent".to_string()
    } else if cv <= 0.08 {
        "good".to_string()
    } else if cv <= 0.15 {
        "fair".to_string()
    } else {
        "unreliable".to_string()
    }
}

pub(super) fn bootstrap_median_interval(values: &[f64]) -> (f64, f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    if values.len() == 1 {
        return (0.0, values[0], values[0]);
    }

    let mut rng = DeterministicRng::from_values(values);
    let mut estimates = Vec::with_capacity(BOOTSTRAP_RESAMPLES);
    for _ in 0..BOOTSTRAP_RESAMPLES {
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
    let tail = (1.0 - REPORT_CONFIDENCE_LEVEL) * 50.0;
    let lower = percentile_from_sorted(&estimates, tail);
    let upper = percentile_from_sorted(&estimates, 100.0 - tail);
    (standard_error, lower, upper)
}

pub(super) fn summarise_metric<I>(values: I) -> MetricSummary
where
    I: IntoIterator<Item = f64>,
{
    let mut values: Vec<f64> = values
        .into_iter()
        .filter(|value| value.is_finite())
        .collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

    if values.is_empty() {
        return MetricSummary::default();
    }

    let sample_count = values.len() as u32;
    let min = values[0];
    let max = *values.last().unwrap_or(&min);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let median = median_from_sorted(&values);
    let variance = if values.len() > 1 {
        values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / (values.len() as f64 - 1.0)
    } else {
        0.0
    };
    let stddev = variance.sqrt();
    let cv = if mean.abs() > f64::EPSILON {
        stddev / mean.abs()
    } else {
        0.0
    };
    let p25 = percentile_from_sorted(&values, 25.0);
    let p75 = percentile_from_sorted(&values, 75.0);
    let iqr = p75 - p25;
    let lower_fence = p25 - (iqr * TUKEY_FENCE_MULTIPLIER);
    let upper_fence = p75 + (iqr * TUKEY_FENCE_MULTIPLIER);
    let low_outlier_count = values.iter().filter(|value| **value < lower_fence).count() as u32;
    let high_outlier_count = values.iter().filter(|value| **value > upper_fence).count() as u32;
    let outlier_count = low_outlier_count + high_outlier_count;

    let mut deviations: Vec<f64> = values.iter().map(|value| (value - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mad = median_from_sorted(&deviations);

    let skewness = if values.len() > 2 && stddev > f64::EPSILON {
        let third_moment = values
            .iter()
            .map(|value| (value - mean).powi(3))
            .sum::<f64>()
            / values.len() as f64;
        third_moment / stddev.powi(3)
    } else {
        0.0
    };
    let kurtosis = if values.len() > 3 && stddev > f64::EPSILON {
        let fourth_moment = values
            .iter()
            .map(|value| (value - mean).powi(4))
            .sum::<f64>()
            / values.len() as f64;
        (fourth_moment / stddev.powi(4)) - 3.0
    } else {
        0.0
    };

    let (standard_error, ci95_lower, ci95_upper) = bootstrap_median_interval(&values);
    let relative_margin_of_error = if median.abs() > f64::EPSILON {
        ((ci95_upper - ci95_lower) / 2.0) / median.abs()
    } else {
        0.0
    };

    MetricSummary {
        sample_count,
        min,
        mean,
        median,
        max,
        stddev,
        cv,
        standard_error,
        variance,
        iqr,
        lower_fence,
        upper_fence,
        low_outlier_count,
        high_outlier_count,
        outlier_count,
        skewness,
        kurtosis,
        mad,
        ci95_lower,
        ci95_upper,
        relative_margin_of_error,
        quality_tier: quality_tier_for_cv(cv),
    }
}

pub(super) fn confidence_intervals_overlap(a: &MetricSummary, b: &MetricSummary) -> bool {
    a.ci95_lower <= b.ci95_upper && b.ci95_lower <= a.ci95_upper
}

pub(super) fn cohens_d(candidate: &[f64], baseline: &[f64]) -> f64 {
    if candidate.len() < 2 || baseline.len() < 2 {
        return 0.0;
    }

    let candidate_mean = candidate.iter().sum::<f64>() / candidate.len() as f64;
    let baseline_mean = baseline.iter().sum::<f64>() / baseline.len() as f64;

    let candidate_var = candidate
        .iter()
        .map(|value| (value - candidate_mean).powi(2))
        .sum::<f64>()
        / (candidate.len() as f64 - 1.0);
    let baseline_var = baseline
        .iter()
        .map(|value| (value - baseline_mean).powi(2))
        .sum::<f64>()
        / (baseline.len() as f64 - 1.0);

    let pooled_variance = (((candidate.len() - 1) as f64 * candidate_var)
        + ((baseline.len() - 1) as f64 * baseline_var))
        / ((candidate.len() + baseline.len() - 2) as f64);

    if pooled_variance <= f64::EPSILON {
        0.0
    } else {
        (candidate_mean - baseline_mean) / pooled_variance.sqrt()
    }
}
