//! Deterministic RNG for bootstrap resampling.
//!
//! Single canonical implementation for the tester crate — used by the
//! adaptive-stop machinery in `benchmark.rs` and the benchmark artifact's
//! confidence intervals in `output/json.rs`.
//!
//! DRIFT NOTE: `benchmarks/orchestrator/src/reporter/stats.rs` carries an
//! intentional copy of this type — the orchestrator is excluded from the
//! workspace and cannot depend on this crate. Any change here must be
//! mirrored there (and vice versa).
//!
//! History (v0.28.81 statistics fix): the original implementation drew
//! bounded indices via `lcg_state % upper` on a raw mod-2^64 LCG. Bit k of
//! such an LCG has period 2^k (Hull–Dobell), so for power-of-two `upper`
//! every consecutive block of `upper` draws visited each index exactly once:
//! bootstrap "resamples" were permutations of the data, every resample median
//! equalled the sample median, and CI width collapsed to 0 — making adaptive
//! stops and the `relative_margin_of_error` publication gate vacuous. Fixed
//! by applying a splitmix64 finalizer to the LCG output and mapping to a
//! bounded index with Lemire's multiply-shift instead of raw modulo.

/// Deterministic pseudo-random generator seeded from the sample values
/// themselves, so the same inputs always produce the same bootstrap interval
/// (the resume/replay property: re-running an artifact conversion or an
/// adaptive-stop evaluation over identical data is bit-identical across runs
/// and platforms).
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

    /// Knuth MMIX LCG step (full period mod 2^64) followed by a splitmix64
    /// finalizer. The finalizer is load-bearing: raw LCG output has
    /// provably periodic low bits (bit k has period 2^k) and must never be
    /// consumed directly.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Bounded index in `0..upper` via Lemire's multiply-shift reduction —
    /// never raw modulo, which both biases the distribution and (with an
    /// unfinalized LCG) inherits low-bit periodicity. The residual bias of
    /// plain multiply-shift is < upper/2^64, negligible for bootstrap
    /// sample sizes.
    pub fn next_index(&mut self, upper: usize) -> usize {
        if upper == 0 {
            return 0;
        }
        (((self.next_u64() as u128) * (upper as u128)) >> 64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression pin for the P0 defect: with the raw-LCG-modulo generator,
    /// power-of-two `upper` made every block of `upper` draws a permutation
    /// of 0..upper. With a healthy generator, an 8-draw block is a
    /// permutation of 0..8 with probability 8!/8^8 ≈ 0.24%, so over 200
    /// blocks essentially none should be.
    #[test]
    fn power_of_two_blocks_are_not_permutations() {
        let mut rng = DeterministicRng::from_values(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let blocks = 200;
        let mut permutation_blocks = 0;
        for _ in 0..blocks {
            let mut seen = [false; 8];
            for _ in 0..8 {
                seen[rng.next_index(8)] = true;
            }
            if seen.iter().all(|&s| s) {
                permutation_blocks += 1;
            }
        }
        assert!(
            permutation_blocks <= 2,
            "{permutation_blocks}/{blocks} blocks were permutations — low-bit periodicity is back"
        );
    }

    /// Distribution sanity: bounded indices over small `upper` (both a power
    /// of two and not) must be roughly uniform — chi-square over 8000 draws
    /// with k-1 degrees of freedom stays far below any reasonable critical
    /// value for a healthy generator (p=0.001 critical for df=7 is 24.3, for
    /// df=9 is 27.9; allow generous headroom against seed luck).
    #[test]
    fn bounded_index_chi_square_uniformity() {
        for upper in [8usize, 10] {
            let mut rng = DeterministicRng::from_values(&[0.5, 1.5, 2.5]);
            let draws = 8000;
            let mut counts = vec![0u32; upper];
            for _ in 0..draws {
                counts[rng.next_index(upper)] += 1;
            }
            let expected = draws as f64 / upper as f64;
            let chi_square: f64 = counts
                .iter()
                .map(|&c| {
                    let d = c as f64 - expected;
                    d * d / expected
                })
                .sum();
            assert!(
                chi_square < 35.0,
                "chi-square {chi_square:.1} too high for upper={upper}: counts {counts:?}"
            );
            assert!(
                counts.iter().all(|&c| c > 0),
                "some index never drawn for upper={upper}: {counts:?}"
            );
        }
    }

    /// Parity must not alternate: the raw LCG strictly alternated even/odd
    /// states (a odd, c odd), which stratified every even-`upper` resample.
    #[test]
    fn output_parity_does_not_strictly_alternate() {
        let mut rng = DeterministicRng::from_values(&[3.0, 1.0, 4.0]);
        let parities: Vec<u64> = (0..64).map(|_| rng.next_u64() & 1).collect();
        let alternating = parities.windows(2).all(|w| w[0] != w[1]);
        assert!(!alternating, "output parity strictly alternates");
    }

    /// Determinism / replay property: same seed values → same sequence.
    #[test]
    fn same_values_same_sequence() {
        let values = [10.0, 20.0, 30.0];
        let mut a = DeterministicRng::from_values(&values);
        let mut b = DeterministicRng::from_values(&values);
        for _ in 0..256 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn next_index_zero_upper_is_safe() {
        let mut rng = DeterministicRng::from_values(&[1.0]);
        assert_eq!(rng.next_index(0), 0);
    }

    #[test]
    fn next_index_in_bounds() {
        let mut rng = DeterministicRng::from_values(&[9.0, 8.0]);
        for upper in [1usize, 2, 3, 7, 8, 100, 1024] {
            for _ in 0..200 {
                assert!(rng.next_index(upper) < upper);
            }
        }
    }
}
