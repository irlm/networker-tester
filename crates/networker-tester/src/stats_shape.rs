//! Distribution-shape diagnostics — the mvalue multimodality detector.
//!
//! Network latency distributions go multimodal for real, diagnosable
//! reasons: a route flap, a cache split, a Nagle/delayed-ACK floor that only
//! some connections hit (the 2026-08 cpp incident: the same workload
//! answered in ~1.5 ms or ~44 ms and NOTHING in the stats layer noticed —
//! the medians simply wandered between modes across runs). A mean or median
//! of a multimodal sample describes nothing that actually happened.
//!
//! The detector is Brendan Gregg's "modal value" (mvalue) following the
//! BenchmarkDotNet/Perfolizer implementation (MValueCalculator):
//!
//! 1. remove Tukey outliers (1.5×IQR, both sides) — a stray tail point must
//!    not read as a mode;
//! 2. start from an optimal bin width (Scott's rule × 0.75, Perfolizer's
//!    constant) and DOUBLE the width until it exceeds the data range — the
//!    scan only ever COARSENS, which is the noise bound: at Scott width the
//!    expected bin occupancy is statistically stable, so sampling flutter
//!    cannot masquerade as modes (a finer scan reads pure noise as
//!    multimodal — measured mvalue 9.0 on uniform noise in this module's
//!    own calibration before the coarsen-only rule);
//! 3. per width: data-anchored histogram (see `mvalue_at_width`) — clusters
//!    of values chained at the bin width, fixed-edge bins inside each
//!    cluster, inter-cluster gaps as explicit empty bins (a gap IS the
//!    evidence), mvalue = Σ|Δ adjacent counts| / max_count, zero-padded;
//! 4. report the max across widths.
//!
//! A clean unimodal sample scores ≈ 2 (0→max→0). Warning tiers follow
//! BenchmarkDotNet's MultimodalDistributionAnalyzer exactly:
//!
//! | mvalue  | verdict                |
//! |---------|------------------------|
//! | > 4.2   | multimodal             |
//! | > 3.2   | bimodal                |
//! | > 2.8   | may have several modes |
//!
//! Known limit, deliberate: a MINORITY mode (say 10 % of samples on a stall
//! floor) is mostly removed by the Tukey pre-clean or normalizes to a short
//! bar below the tiers — that class is covered by the Tukey-fence outlier
//! counts and kurtosis warnings that already exist in `data_quality`.
//! mvalue owns the split-mass case those metrics are blind to.

/// BenchmarkDotNet gates the analyzer at its minimum workload iteration
/// count; we adopt the same floor.
pub const MVALUE_MIN_SAMPLES: usize = 15;

/// Warning tiers, per BenchmarkDotNet's MultimodalDistributionAnalyzer.
pub const MVALUE_SEVERAL_MODES: f64 = 2.8;
pub const MVALUE_BIMODAL: f64 = 3.2;
pub const MVALUE_MULTIMODAL: f64 = 4.2;

/// Compute the mvalue of a sample. Returns `None` below
/// [`MVALUE_MIN_SAMPLES`] finite values (before outlier cleaning).
pub fn mvalue(values: &[f64]) -> Option<f64> {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    if sorted.len() < MVALUE_MIN_SAMPLES {
        return None;
    }
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // 1. Tukey pre-clean (1.5×IQR both sides), mirroring Perfolizer: a
    // handful of stragglers is outlier territory, not a mode.
    let cleaned = without_tukey_outliers(&sorted);
    let n = cleaned.len();
    if n < 2 {
        return Some(2.0);
    }
    let range = cleaned[n - 1] - cleaned[0];
    if range <= 0.0 {
        return Some(2.0); // every retained sample identical: one mode
    }

    // 2. Optimal starting width: Scott's rule × 0.75 (Perfolizer's
    // AdaptiveHistogramBuilder.OptimalBinSize).
    let mean = cleaned.iter().sum::<f64>() / n as f64;
    let variance = cleaned.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let scott = 3.49 * stddev / (n as f64).cbrt();
    let mut width = if scott > 0.0 { scott * 0.75 } else { range };

    // Quantized-data guard: values on a lattice (ms-resolution timers,
    // integer-graded metrics) comb into alternating occupied/empty bins
    // whenever the width drops below the lattice spacing, reading falsely
    // multimodal. When a lattice hypothesis is credible — at least 5
    // distinct positions — floor the width at 2× the MEDIAN positive gap
    // (the spacing estimate; the median ignores the few large inter-mode
    // gaps a genuinely multimodal lattice also has). Below 5 distinct
    // positions the "lattice" IS the mode structure (two point masses 43 ms
    // apart are a cache split, not a 43 ms-resolution timer) and the floor
    // must stay out of the way.
    let mut positive_gaps: Vec<f64> = cleaned
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|gap| *gap > 0.0)
        .collect();
    if positive_gaps.len() >= 4 {
        positive_gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median_gap = positive_gaps[positive_gaps.len() / 2];
        width = width.max(2.0 * median_gap);
    }

    // 3./4. Coarsen-only scan, max of Σ|Δ|/max_count per width.
    let mut best: f64 = 0.0;
    while width <= range {
        best = best.max(mvalue_at_width(&cleaned, width));
        width *= 2.0;
    }
    // Always evaluate at least one histogram even when the starting width
    // already exceeds the range (very tight clusters).
    if best == 0.0 {
        best = mvalue_at_width(&cleaned, range);
    }

    Some(best)
}

fn without_tukey_outliers(sorted: &[f64]) -> Vec<f64> {
    let n = sorted.len();
    let q1 = interpolated_quartile(sorted, 0.25);
    let q3 = interpolated_quartile(sorted, 0.75);
    let iqr = q3 - q1;
    let lo = q1 - 1.5 * iqr;
    let hi = q3 + 1.5 * iqr;
    let cleaned: Vec<f64> = sorted
        .iter()
        .copied()
        .filter(|v| *v >= lo && *v <= hi)
        .collect();
    if cleaned.is_empty() {
        sorted.to_vec() // degenerate; keep the data rather than divide by zero
    } else {
        let _ = n;
        cleaned
    }
}

fn interpolated_quartile(sorted: &[f64], q: f64) -> f64 {
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let rank = q * (n - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f64;
    sorted[lo] + (sorted[hi] - sorted[lo]) * frac
}

/// One data-anchored histogram at `width` (the essence of Perfolizer's
/// AdaptiveHistogramBuilder): values chain into a cluster while consecutive
/// gaps stay ≤ `width`; each cluster is subdivided into fixed-edge bins
/// anchored at ITS OWN minimum; each inter-cluster gap contributes
/// `⌊gap/width⌋` explicit empty bins. Anchoring bins to the data (not to
/// global fixed edges) is what stops bin-boundary alignment from
/// manufacturing empty bins inside a dense region while keeping real gaps
/// visible. mvalue = zero-padded Σ|Δ adjacent heights| / max height.
fn mvalue_at_width(sorted: &[f64], width: f64) -> f64 {
    let n = sorted.len();
    let mut heights: Vec<u32> = Vec::new();
    let mut i = 0;
    while i < n {
        let start = i;
        while i + 1 < n && sorted[i + 1] - sorted[i] <= width {
            i += 1;
        }
        let cluster = &sorted[start..=i];
        let lo = cluster[0];
        let extent = cluster[cluster.len() - 1] - lo;
        let bin_count = ((extent / width) as usize + 1).min(4096);
        let mut counts = vec![0u32; bin_count];
        for &v in cluster {
            let idx = ((v - lo) / width) as usize;
            counts[idx.min(bin_count - 1)] += 1;
        }
        heights.extend(counts);

        if i + 1 < n {
            let gap = sorted[i + 1] - sorted[i];
            let empties = ((gap / width) as usize).clamp(1, 64);
            heights.extend(std::iter::repeat_n(0, empties));
        }
        i += 1;
    }

    let max_height = *heights.iter().max().unwrap_or(&1) as f64;
    if max_height <= 0.0 {
        return 2.0;
    }

    let mut sum = 0.0;
    let mut prev = 0.0;
    for &h in &heights {
        let h = h as f64;
        sum += (h - prev).abs();
        prev = h;
    }
    sum += prev; // trailing zero pad
    sum / max_height
}

/// The warning text for an mvalue, or `None` when the distribution looks
/// unimodal. Wording mirrors BenchmarkDotNet so operators can cross-read.
pub fn mvalue_warning(mvalue: f64) -> Option<String> {
    if mvalue > MVALUE_MULTIMODAL {
        Some(format!(
            "Distribution is multimodal (mvalue = {mvalue:.2}) — the summary statistics \
             describe a mixture, not a behaviour; investigate the modes before quoting numbers"
        ))
    } else if mvalue > MVALUE_BIMODAL {
        Some(format!(
            "Distribution is bimodal (mvalue = {mvalue:.2}) — two distinct behaviours are \
             mixed in this case (e.g. a per-connection stall or cache split)"
        ))
    } else if mvalue > MVALUE_SEVERAL_MODES {
        Some(format!(
            "Distribution may have several modes (mvalue = {mvalue:.2})"
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-noise in [-0.5, 0.5) without rand: a splitmix64
    /// walk keyed by the index (the stats_rng no-Date/no-rand discipline).
    fn jitter(i: u64) -> f64 {
        let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z as f64 / u64::MAX as f64) - 0.5
    }

    #[test]
    fn too_few_samples_is_none() {
        let v: Vec<f64> = (0..(MVALUE_MIN_SAMPLES - 1)).map(|i| i as f64).collect();
        assert_eq!(mvalue(&v), None);
    }

    #[test]
    fn identical_values_are_unimodal() {
        let v = vec![42.0; 50];
        assert_eq!(mvalue(&v), Some(2.0));
    }

    #[test]
    fn unimodal_cluster_stays_below_the_first_tier() {
        // 100 samples tightly around 50 ms — ordinary jitter, one behaviour.
        let v: Vec<f64> = (0..100).map(|i| 50.0 + jitter(i) * 4.0).collect();
        let m = mvalue(&v).unwrap();
        assert!(
            m <= MVALUE_SEVERAL_MODES,
            "unimodal cluster must not warn, got mvalue {m:.2}"
        );
    }

    #[test]
    fn uniform_noise_stays_below_the_first_tier() {
        // The failure mode of a fine-binned scan: pure noise reading as many
        // modes (mvalue 9.0 under a since-removed finer-scan variant). The
        // coarsen-only rule must keep this quiet.
        let v: Vec<f64> = (0..100).map(|i| 10.0 + (jitter(i) + 0.5) * 90.0).collect();
        let m = mvalue(&v).unwrap();
        assert!(
            m <= MVALUE_SEVERAL_MODES,
            "uniform noise must not warn, got mvalue {m:.2}"
        );
    }

    #[test]
    fn split_mass_bimodal_crosses_the_bimodal_tier() {
        // The cpp-incident shape ON AN AFFECTED RUN: roughly half the
        // requests answer at ~1.5 ms, half stall at ~44 ms.
        let mut v: Vec<f64> = (0..50).map(|i| 1.5 + jitter(i) * 0.4).collect();
        v.extend((0..50).map(|i| 44.0 + jitter(i + 1000) * 2.0));
        let m = mvalue(&v).unwrap();
        assert!(
            m > MVALUE_BIMODAL,
            "a half/half two-behaviour mix must read bimodal, got mvalue {m:.2}"
        );
    }

    #[test]
    fn three_separated_modes_cross_the_multimodal_tier() {
        let mut v: Vec<f64> = (0..40).map(|i| 5.0 + jitter(i) * 0.6).collect();
        v.extend((0..40).map(|i| 50.0 + jitter(i + 500) * 0.6));
        v.extend((0..40).map(|i| 95.0 + jitter(i + 900) * 0.6));
        let m = mvalue(&v).unwrap();
        assert!(
            m > MVALUE_MULTIMODAL,
            "three equal well-separated modes must read multimodal, got mvalue {m:.2}"
        );
    }

    #[test]
    fn minority_mode_is_documented_as_below_tier() {
        // 10% of samples on a stall floor: mvalue's documented blind spot —
        // the Tukey pre-clean removes the minority cluster or normalization
        // shrinks it below the tiers. The Tukey outlier counts in
        // data_quality own this class; this test PINS the division of
        // labour so a future "fix" here knows what it is changing.
        let mut v: Vec<f64> = (0..90).map(|i| 1.5 + jitter(i) * 0.4).collect();
        v.extend((0..10).map(|i| 44.0 + jitter(i + 77) * 2.0));
        let m = mvalue(&v).unwrap();
        assert!(
            m <= MVALUE_BIMODAL,
            "minority mode staying sub-tier is the documented contract, got {m:.2}"
        );
    }

    #[test]
    fn lattice_values_do_not_comb_into_false_modes() {
        // ms-resolution timers produce values on a 1 ms grid; without the
        // min-gap width floor these comb into alternating tall/empty bins
        // and read multimodal. Five adjacent grid values, one behaviour.
        let v: Vec<f64> = (0..100).map(|i| 15.0 + ((i * 7) % 5) as f64).collect();
        let m = mvalue(&v).unwrap();
        assert!(
            m <= MVALUE_SEVERAL_MODES,
            "unimodal lattice data must not warn, got mvalue {m:.2}"
        );
    }

    #[test]
    fn bimodal_lattice_values_are_still_detected() {
        // The guard must not blind the detector: two grid-valued clusters
        // separated by a real gap remain bimodal.
        let mut v: Vec<f64> = (0..50).map(|i| 1.0 + (i % 3) as f64).collect();
        v.extend((0..50).map(|i| 44.0 + (i % 3) as f64));
        let m = mvalue(&v).unwrap();
        assert!(
            m > MVALUE_BIMODAL,
            "grid-valued two-cluster mix must still read bimodal, got mvalue {m:.2}"
        );
    }

    #[test]
    fn warnings_map_the_tiers() {
        assert!(mvalue_warning(2.0).is_none());
        assert!(mvalue_warning(2.81).unwrap().contains("several modes"));
        assert!(mvalue_warning(3.3).unwrap().contains("bimodal"));
        assert!(mvalue_warning(4.5).unwrap().contains("multimodal"));
    }
}
