//! Double-double arithmetic: a second, deliberately independent numerical path.
//!
//! APORIA needs to answer "is this disagreement real numerical error, or just the ordinary noise of
//! floating point?" The only honest way is to compute the same expression more accurately than the
//! model does and compare. A double-double carries roughly 106 bits of mantissa as an unevaluated
//! sum `hi + lo`, which is enough that the gap between it and plain f64 measures the model's own
//! rounding error rather than the comparator's.
//!
//! Transcendental functions are not implemented at this precision; `reference` falls back to f64 at
//! those nodes and says so. Claiming accuracy the code does not have is exactly the failure mode
//! this project exists to detect.

/// A value held as a non-overlapping sum of two f64s: `hi` carries the leading 53 bits, `lo` the
/// next batch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dd {
    pub hi: f64,
    pub lo: f64,
}

#[expect(clippy::should_implement_trait)]
impl Dd {
    #[must_use]
    pub fn zero() -> Self {
        Self { hi: 0.0, lo: 0.0 }
    }

    #[must_use]
    pub fn from_f64(v: f64) -> Self {
        Self { hi: v, lo: 0.0 }
    }

    #[must_use]
    pub fn to_f64(self) -> f64 {
        self.hi + self.lo
    }

    /// Named `neg` rather than implementing [`std::ops::Neg`]: the operators would invite writing
    /// `a + b` on double-doubles next to f64 arithmetic in the same file, and the difference in
    /// accuracy is the whole reason both are here.
    #[expect(clippy::should_implement_trait)]
    #[must_use]
    pub fn neg(self) -> Self {
        Self {
            hi: -self.hi,
            lo: -self.lo,
        }
    }

    #[must_use]
    pub fn abs(self) -> Self {
        if self.hi < 0.0 || (self.hi == 0.0 && self.lo < 0.0) {
            self.neg()
        } else {
            self
        }
    }

    /// Add two double-doubles.
    ///
    /// The leading words are summed with an exact error term, the two discarded low words are
    /// folded into it, and the pair is renormalised. An earlier draft of this function returned `2`
    /// for `1e16 + 1 - 1e16`; the test that now pins the answer to `1` is why the algorithm is
    /// written out instead of trusted.
    #[must_use]
    pub fn add(self, other: Self) -> Self {
        let (s, e) = two_sum(self.hi, other.hi);
        let e = e + self.lo + other.lo;
        let (hi, lo) = quick_two_sum(s, e);
        Self { hi, lo }
    }

    #[must_use]
    pub fn sub(self, other: Self) -> Self {
        self.add(other.neg())
    }

    #[must_use]
    pub fn mul(self, other: Self) -> Self {
        let (p, e) = two_product(self.hi, other.hi);
        // The cross terms are small by construction, so they can be accumulated into the error word
        // before renormalising instead of needing exact sums of their own.
        let e = e + self.hi * other.lo + self.lo * other.hi;
        let (hi, lo) = quick_two_sum(p, e);
        Self { hi, lo }
    }

    /// One Newton refinement of the f64 quotient: the residual `self - other*q0` is computed in
    /// double-double, so the correction carries the bits the first division threw away.
    #[must_use]
    pub fn div(self, other: Self) -> Self {
        if other.hi == 0.0 && other.lo == 0.0 {
            // Let IEEE answer a division by zero. The refinement below would turn the infinity the
            // scalar interpreter produces into a NaN, and the differential channel would then
            // report a disagreement that is really a bug in the comparator.
            return Self::from_f64(self.hi / other.hi);
        }
        let q0 = self.hi / other.hi;
        let r = self.sub(other.mul(Self::from_f64(q0)));
        Self::from_f64(q0).add(r.mul(Self::from_f64(1.0 / other.hi)))
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.hi.is_finite() && self.lo.is_finite()
    }
}

/// Exact sum of two f64s and its rounding error, so that `s + e` is the true sum.
#[must_use]
pub fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    let bb = s - a;
    let e = (a - (s - bb)) + (b - bb);
    (s, e)
}

/// Renormalise an unevaluated sum where `|a| >= |b|`, which is exactly what the error term of
/// [`two_sum`] and [`two_product`] guarantees.
///
/// The dominance requirement is not decorative: `b - (s - a)` is only the exact low word when `a` is
/// at least as large as `b`.
#[must_use]
fn quick_two_sum(a: f64, b: f64) -> (f64, f64) {
    let s = a + b;
    (s, b - (s - a))
}

/// Exact product of two f64s and the error, using a fused multiply-add so no word split is needed.
#[must_use]
pub fn two_product(a: f64, b: f64) -> (f64, f64) {
    let p = a * b;
    let e = a.mul_add(b, -p);
    (p, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sum_holds_the_term_that_rounding_dropped() {
        // 1e16 has a spacing of two, so adding one changes nothing after rounding and the whole term
        // has to be carried in the error word.
        let (s, e) = two_sum(1e16, 1.0);
        assert_eq!(s, 1e16);
        assert_eq!(
            e, 1.0,
            "the lost one is exactly what the error term exists to hold"
        );
    }

    #[test]
    fn two_sum_is_exact_for_ordinary_pairs() {
        for (a, b) in [(1.0, 2.0), (0.1, 0.2), (-3.5, 3.5), (1e300, 1.0)] {
            let (s, e) = two_sum(a, b);
            assert_eq!(s + e, a + b, "{a} {b}");
            assert!(
                e.abs() <= (a.abs() + b.abs()) * f64::EPSILON * 2.0 + f64::EPSILON,
                "{a} {b} -> {e}"
            );
        }
    }

    #[test]
    fn a_naive_sum_vanishes_and_the_double_double_one_does_not() {
        let naive = 1e16 + 1.0 - 1e16;
        let dd = Dd::from_f64(1e16)
            .add(Dd::from_f64(1.0))
            .sub(Dd::from_f64(1e16));
        assert_eq!(naive, 0.0, "the plain path loses the term entirely");
        assert_eq!(dd.to_f64(), 1.0, "double-double must recover exactly one");
    }

    #[test]
    fn addition_is_commutative_and_keeps_the_residual() {
        let a = Dd { hi: 1.0, lo: 1e-20 };
        let b = Dd {
            hi: 2.0,
            lo: -3e-21,
        };
        let ab = a.add(b);
        let ba = b.add(a);
        assert_eq!(ab.to_f64(), ba.to_f64());
        assert!((ab.to_f64() - 3.0).abs() < 1e-20);
    }

    #[test]
    fn adding_zero_changes_nothing() {
        let x = Dd { hi: 2.5, lo: 1e-20 };
        let s = x.add(Dd::zero());
        assert_eq!(s.hi, x.hi);
        assert_eq!(s.lo, x.lo);
    }

    #[test]
    fn two_product_captures_an_error_that_is_not_representable() {
        let (p, e) = two_product(0.1, 0.1);
        assert_ne!(e, 0.0, "0.1 squared is not exact in binary");
        let dd = Dd { hi: p, lo: e };
        assert!((dd.to_f64() - 0.010_000_000_000_000_002).abs() <= f64::EPSILON);
    }

    #[test]
    fn multiplication_keeps_a_term_the_plain_product_drops() {
        let (big, small) = (1e154, 1e-154);
        let dd = Dd::from_f64(big)
            .mul(Dd::from_f64(1.0).add(Dd::from_f64(1e-16)))
            .mul(Dd::from_f64(small));
        assert!((dd.to_f64() - 1.0).abs() < 1e-14, "dd {}", dd.to_f64());
        assert_ne!(dd.lo, 0.0, "the product should still carry a residual");
    }

    #[test]
    fn division_recovers_digits_a_plain_quotient_cannot_hold() {
        let q = Dd::from_f64(1.0).div(Dd::from_f64(3.0));
        assert_eq!(q.to_f64(), 1.0 / 3.0, "the rounded answer must match");
        assert_ne!(
            q.lo, 0.0,
            "one third is not representable, so the residual must be nonzero"
        );
        let back = q.mul(Dd::from_f64(3.0));
        assert!((back.to_f64() - 1.0).abs() < 1e-30, "{}", back.to_f64());
    }

    #[test]
    fn abs_keeps_the_low_order() {
        let x = Dd {
            hi: -3.0,
            lo: -1e-20,
        };
        let a = x.abs();
        assert_eq!(a.hi, 3.0);
        assert_eq!(a.lo, 1e-20);
    }

    #[test]
    fn non_finite_input_stays_visible() {
        assert!(!Dd::from_f64(f64::INFINITY).is_finite());
        assert!(
            !Dd {
                hi: f64::NAN,
                lo: 0.0
            }
            .is_finite()
        );
        assert!(Dd { hi: 1.0, lo: 1e-20 }.is_finite());
    }

    #[test]
    fn a_long_sum_keeps_terms_the_plain_accumulator_drops() {
        // A hundred ones added to 1e16. The spacing at 1e16 is two, so the plain accumulator
        // absorbs every one of them and returns exactly what it started with; the double-double one
        // keeps the count.
        let mut acc64 = 1e16;
        let mut acc_dd = Dd::from_f64(1e16);
        for _ in 0..100 {
            acc64 += 1.0;
            acc_dd = acc_dd.add(Dd::from_f64(1.0));
        }
        assert_eq!(acc64, 1e16, "the plain path lost all hundred terms");
        assert_eq!(acc_dd.to_f64(), 1e16 + 100.0, "the reference kept them");
    }
}
