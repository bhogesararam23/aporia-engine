//! The generator every experiment draws from.
//!
//! Sampling is core algorithm here, not plumbing: an experiment whose numbers cannot be produced
//! again from the same seed is not evidence. So APORIA uses one small, explicit generator,
//! deterministic and versioned by its own behaviour.
//!
//! `splitmix64` is the choice. It is eleven lines, has no state beyond one word, produces full-period
//! output with a documented statistical profile, and its every-next-state is a bijective mix of the
//! previous one, so skipping ahead by an index is exact rather than approximated by iterating.

/// A splitmix64 stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
    /// How many values have been taken, so a recorded position can be resumed exactly.
    pub taken: u64,
}

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

impl Rng {
    #[must_use]
    pub fn seeded(seed: u64) -> Self {
        Self {
            state: seed ^ GOLDEN,
            taken: 0,
        }
    }

    /// A stable seed from a label, so `"experiment-2026-10-04"` and a numeric seed describe the same
    /// stream on any machine. FNV-1a is not a hash for security; it is a deterministic mixer, which
    /// is all this needs.
    #[must_use]
    pub fn from_label(label: &str) -> Self {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in label.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        Self::seeded(h)
    }

    #[must_use]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(GOLDEN);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        self.taken += 1;
        z ^ (z >> 31)
    }

    /// A value in `[0, 1)` using the top 53 bits, which is how a double can hold an exact binary
    /// fraction of a 2^53 grid. Using fewer bits would quietly reduce the resolution of every
    /// sampler built on it.
    #[must_use]
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    #[must_use]
    pub fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next_f64()
    }

    #[must_use]
    pub fn index(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        // A multiply-shift instead of a modulo: `rand % bound` is biased whenever bound is not a
        // power of two, and this sampler decides where millions of experiments look.
        ((self.next_u64() >> 33) as usize).wrapping_mul(bound) >> 31
    }

    /// Move forward without producing values, so a sub-stream can be reserved per axis.
    pub fn skip(&mut self, count: u64) {
        for _ in 0..count {
            let _ = self.next_u64();
        }
    }

    #[must_use]
    pub fn state(&self) -> u64 {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_produces_the_same_stream() {
        let mut r1 = Rng::seeded(7);
        let a: Vec<u64> = (0..64).map(|_| r1.next_u64()).collect();
        let mut r2 = Rng::seeded(7);
        let b: Vec<u64> = (0..64).map(|_| r2.next_u64()).collect();
        assert_eq!(a, b, "a replayed seed must reproduce the whole sequence");
        let unique = a.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(
            unique,
            a.len(),
            "a 64-value stream should not repeat itself"
        );
        let mut other = Rng::seeded(8);
        assert_ne!(
            other.next_u64(),
            a[0],
            "a different seed must open a different stream"
        );
    }

    #[test]
    fn doubles_stay_inside_the_half_open_interval() {
        let mut r = Rng::seeded(1);
        for _ in 0..100_000 {
            let v = r.next_f64();
            assert!((0.0..1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn a_uniform_stream_looks_uniform_enough_to_trust() {
        let mut r = Rng::seeded(42);
        let n = 200_000;
        let mut bins = [0u32; 100];
        let mut sum = 0.0f64;
        let mut sum2 = 0.0f64;
        for _ in 0..n {
            let v = r.next_f64();
            sum += v;
            sum2 += v * v;
            bins[(v * 100.0) as usize % 100] += 1;
        }
        let mean = sum / n as f64;
        let var = sum2 / n as f64 - mean * mean;
        assert!((mean - 0.5).abs() < 0.005, "mean {mean}");
        assert!((var - 1.0 / 12.0).abs() < 0.002, "variance {var}");
        let expected = n as f64 / 100.0;
        let chi: f64 = bins
            .iter()
            .map(|&c| (c as f64 - expected).powi(2) / expected)
            .sum();
        // 100 bins: the 99th percentile of chi-square with 99 degrees of freedom is about 149.
        assert!(chi < 149.0, "chi-square {chi}");
    }

    #[test]
    fn index_is_unbiased_over_a_non_power_of_two() {
        let mut r = Rng::seeded(9);
        let mut counts = [0u32; 7];
        let n = 70_000;
        for _ in 0..n {
            counts[r.index(7)] += 1;
        }
        for c in counts {
            assert!((c as f64 - n as f64 / 7.0).abs() < 700.0, "{counts:?}");
        }
        assert_eq!(r.index(0), 0);
    }

    #[test]
    fn skipping_is_the_same_as_iterating() {
        let mut a = Rng::seeded(3);
        let mut b = Rng::seeded(3);
        for _ in 0..5 {
            let _ = a.next_u64();
        }
        b.skip(5);
        assert_eq!(
            a.state(),
            b.state(),
            "skip must land exactly on the same position"
        );
        assert_eq!(a.next_u64(), b.next_u64());
        assert_eq!(a.taken, b.taken);
    }

    #[test]
    fn a_range_maps_the_unit_interval() {
        let mut r = Rng::seeded(11);
        for _ in 0..10_000 {
            let v = r.range(-2.0, 5.0);
            assert!((-2.0..5.0).contains(&v), "{v}");
        }
    }
}
