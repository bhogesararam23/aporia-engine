//! Measuring how far apart two numbers are, in the units that mean something for floating point.

use std::fmt;

/// Distance between two f64 bit patterns, in units in the last place.
///
/// Signed magnitudes close to zero have dense representable neighbours, so `ulps` alone is a poor
/// measure across a wide range; `relative` and `absolute` are reported together and each is used for
/// the case it suits.
#[must_use]
pub fn ulps(a: f64, b: f64) -> u64 {
    if a.is_nan() || b.is_nan() {
        return u64::MAX;
    }
    if a == b {
        // Exact equality is the question being asked here, not an approximation of one.
        return 0;
    }
    // Reading the bit patterns as signed integers is the algorithm: IEEE-754 orders them
    // monotonically that way, wrapping included, which is why the casts below are exact.
    #[expect(clippy::cast_possible_wrap)]
    let ia = a.to_bits() as i64;
    #[expect(clippy::cast_possible_wrap)]
    let ib = b.to_bits() as i64;
    // The IEEE ordering of bit patterns, read as signed integers, is monotonic across both signs,
    // so the distance is one wrapping subtraction and its magnitude.
    ia.wrapping_sub(ib).unsigned_abs()
}

/// Relative difference, using the larger magnitude as the denominator so the value stays in
/// `[0, ∞)` and behaves when one side is zero.
#[must_use]
pub fn relative(a: f64, b: f64) -> f64 {
    if a == b {
        return 0.0;
    }
    if !a.is_finite() || !b.is_finite() {
        return f64::INFINITY;
    }
    let scale = a.abs().max(b.abs());
    if scale == 0.0 {
        0.0
    } else {
        (a - b).abs() / scale
    }
}

/// How much of the inputs' precision the operation destroyed.
///
/// `a - b` where `a ≈ b` cancels leading digits, and the result's meaningful precision is a fraction
/// of the inputs'. This counts how many decimal digits vanished, which is the number worth
/// reporting: a cancellation of 14 digits in an f64 expression leaves noise dressed as an answer.
#[must_use]
pub fn cancellation(a: f64, b: f64) -> f64 {
    let sum = a - b;
    if sum == 0.0 || !sum.is_finite() {
        return 0.0;
    }
    let scale = a.abs().max(b.abs());
    if scale == 0.0 {
        return 0.0;
    }
    (scale / sum.abs()).log10().max(0.0)
}

/// Median of a slice of measurements, without disturbing the caller's buffer. Used wherever a
/// robust scale is needed instead of one a single outlier can move.
#[must_use]
pub fn median_of(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        v[mid - 1].midpoint(v[mid])
    }
}

/// The three ways to compare a pair of numbers, kept together because which one is meaningful
/// depends on magnitudes, and picking one silently is how false confidence gets produced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distance {
    pub absolute: f64,
    pub relative: f64,
    pub ulps: u64,
}

impl Distance {
    #[must_use]
    pub fn between(a: f64, b: f64) -> Self {
        Self {
            absolute: (a - b).abs(),
            relative: relative(a, b),
            ulps: ulps(a, b),
        }
    }

    #[must_use]
    pub fn identical(&self) -> bool {
        self.ulps == 0
    }

    /// A disagreement worth calling evidence: larger than both the tolerance and a hair of f64
    /// noise, and not just the two values sitting at opposite ends of a scale-free zero.
    #[must_use]
    pub fn disagrees_beyond(&self, tolerance: f64) -> bool {
        self.relative > tolerance && self.ulps > 1
    }

    /// Relative distance normalised against f64 rounding noise, so "one unit of precision" and
    /// "one percent wrong" are distinguishable in a report.
    #[must_use]
    pub fn in_epsilons(&self) -> f64 {
        self.relative / f64::EPSILON
    }
}

impl fmt::Display for Distance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "|Δ|={} rel={:.3e} ulps={}",
            self.absolute, self.relative, self.ulps
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_values_are_one_ulp_apart() {
        let a = 1.0f64;
        let b = f64::from_bits(a.to_bits() + 1);
        assert_eq!(ulps(a, b), 1);
        assert_eq!(ulps(a, a), 0);
    }

    #[test]
    fn crossing_zero_is_not_a_bit_pattern_cliff() {
        // Naive bit subtraction explodes across the sign bit; the ordering trick must not.
        let d = ulps(0.0, -0.0);
        assert!(d < 4, "{d}");
    }

    #[test]
    fn a_nan_is_the_furthest_distance_representable() {
        assert_eq!(ulps(1.0, f64::NAN), u64::MAX);
        assert!(relative(1.0, f64::NAN).is_infinite());
    }

    #[test]
    fn relative_distance_is_scale_free() {
        assert!((relative(100.0, 101.0) - 1.0 / 101.0).abs() < 1e-15);
        assert!((relative(1e-9, 2e-9) - 0.5).abs() < 1e-15);
        assert_eq!(relative(0.0, 0.0), 0.0);
    }

    #[test]
    fn cancellation_counts_the_digits_that_disappeared() {
        let a = 1.0f64;
        let b = 1.0 + 1e-14;
        let c = cancellation(a, b);
        // log10((|a|+|b|) / |a-b|) for a pair 1e-14 apart is a little over 14 digits.
        assert!(c > 13.5 && c < 15.0, "{c}");
        // Nothing worth calling cancellation when the operands are far apart: less than one
        // decimal digit is lost, and the measure must not report that as a problem.
        assert!(cancellation(1.0, 2.0) < 1.0, "{}", cancellation(1.0, 2.0));
    }

    #[test]
    fn identical_and_disagreeing_are_not_confused() {
        let same = Distance::between(2.5, 2.5);
        assert!(same.identical());
        assert!(!same.disagrees_beyond(1e-12));

        // One ulp apart is noise, however the tolerance is set.
        let hair = Distance::between(1.0, f64::from_bits(1.0f64.to_bits() + 1));
        assert!(!hair.identical());
        assert!(
            !hair.disagrees_beyond(1e-12),
            "one ulp must not count as disagreement"
        );

        let wrong = Distance::between(1.0, 1.0001);
        assert!(wrong.disagrees_beyond(1e-6));
        assert!(wrong.in_epsilons() > 100.0);
    }

    #[test]
    fn the_display_form_carries_all_three_measures() {
        let d = Distance::between(1.0, 1.5);
        let text = d.to_string();
        assert!(text.contains("rel=") && text.contains("ulps="), "{text}");
    }
}
