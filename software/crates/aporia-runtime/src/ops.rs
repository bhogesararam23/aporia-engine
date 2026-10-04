//! One definition of what each A-IR operation means.
//!
//! The scalar interpreter, the batched interpreter and any later backend call these functions, so
//! "scalar and vector disagree" can only ever mean a real numerical finding, never two copies of
//! the same arithmetic that drifted apart during development.

use crate::value::{Flags, FpMode};
use aporia_ir::{Binop, Builtin, NumType, Unop};

/// Arithmetic on two floating values.
#[must_use]
pub fn bin_f64(op: Binop, a: f64, b: f64, mode: FpMode) -> (f64, Flags) {
    let mut f = Flags::default();
    match op {
        Binop::Div | Binop::Rem => f.zero_division = b == 0.0,
        Binop::Pow => f.invalid_domain = a < 0.0 && b.fract() != 0.0,
        _ => {}
    }
    let v = match mode {
        FpMode::F64 => apply_f64(op, a, b),
        // Reduced precision is applied to the operands and the result, which is what an f32 model
        // actually does: nothing in it ever sees more than 24 bits of mantissa.
        FpMode::F32 => f64::from(apply_f32(op, a as f32, b as f32)),
    };
    (v, f.with_result(v))
}

fn apply_f64(op: Binop, a: f64, b: f64) -> f64 {
    match op {
        Binop::Add => a + b,
        Binop::Sub => a - b,
        Binop::Mul => a * b,
        Binop::Div => a / b,
        Binop::Rem => a.rem_euclid(b),
        Binop::Pow => a.powf(b),
        Binop::Min => a.min(b),
        Binop::Max => a.max(b),
        other => compare_f64(other, a, b).0,
    }
}

fn apply_f32(op: Binop, a: f32, b: f32) -> f32 {
    match op {
        Binop::Add => a + b,
        Binop::Sub => a - b,
        Binop::Mul => a * b,
        Binop::Div => a / b,
        Binop::Rem => a.rem_euclid(b),
        Binop::Pow => a.powf(b),
        Binop::Min => a.min(b),
        Binop::Max => a.max(b),
        other => compare_f64(other, f64::from(a), f64::from(b)).0 as f32,
    }
}

/// Round a value the way the active precision mode says it should be stored.
///
/// Reduced precision is applied to the result, not only to the operands: an f32 model never holds
/// more than 24 bits of mantissa anywhere, and rounding only at the end would hide exactly the
/// accumulation error this mode exists to expose.
fn round_mode(v: f64, mode: FpMode) -> f64 {
    match mode {
        FpMode::F64 => v,
        FpMode::F32 => f64::from(v as f32),
    }
}

/// A comparison, with the IEEE ordering rules kept visible rather than hidden in a `<`.
#[must_use]
pub fn compare_f64(op: Binop, a: f64, b: f64) -> (f64, Flags) {
    let r = match op {
        Binop::Lt => a < b,
        Binop::Le => a <= b,
        Binop::Gt => a > b,
        Binop::Ge => a >= b,
        Binop::Eq => a == b,
        Binop::Ne => a != b,
        Binop::And => a != 0.0 && b != 0.0,
        Binop::Or => a != 0.0 || b != 0.0,
        _ => unreachable!("compare_f64 called with an arithmetic operator"),
    };
    (if r { 1.0 } else { 0.0 }, Flags::default())
}

/// Integer arithmetic, used for `count` quantities and loop trip counts.
#[must_use]
pub fn bin_i64(op: Binop, a: i64, b: i64) -> (i64, Flags) {
    let checked = match op {
        Binop::Add => a.checked_add(b),
        Binop::Sub => a.checked_sub(b),
        Binop::Mul => a.checked_mul(b),
        Binop::Div => a.checked_div(b),
        Binop::Rem => a.checked_rem(b),
        Binop::Min => Some(a.min(b)),
        Binop::Max => Some(a.max(b)),
        other => {
            let (r, _) = compare_f64(other, a as f64, b as f64);
            return (r as i64, Flags::default());
        }
    };
    match checked {
        Some(v) => (v, Flags::default()),
        None => (
            0,
            Flags {
                zero_division: matches!(op, Binop::Div | Binop::Rem) && b == 0,
                integer_overflow: true,
                ..Default::default()
            },
        ),
    }
}

#[must_use]
pub fn un_f64(op: Unop, a: f64, mode: FpMode) -> (f64, Flags) {
    let (v, f) = match op {
        Unop::Neg => (-a, Flags::default()),
        Unop::Abs => (a.abs(), Flags::default()),
        Unop::Sqrt => (
            a.sqrt(),
            Flags {
                invalid_domain: a < 0.0,
                ..Default::default()
            },
        ),
        Unop::Cbrt => (a.cbrt(), Flags::default()),
        Unop::Exp => (a.exp(), Flags::default()),
        Unop::Ln => (
            a.ln(),
            Flags {
                invalid_domain: a <= 0.0,
                ..Default::default()
            },
        ),
        Unop::Log2 => (
            a.log2(),
            Flags {
                invalid_domain: a <= 0.0,
                ..Default::default()
            },
        ),
        Unop::Log10 => (
            a.log10(),
            Flags {
                invalid_domain: a <= 0.0,
                ..Default::default()
            },
        ),
        Unop::Sin => (a.sin(), Flags::default()),
        Unop::Cos => (a.cos(), Flags::default()),
        Unop::Tan => (a.tan(), Flags::default()),
        // asin and acos leave the real numbers outside [-1, 1]; sinh and tanh do not.
        Unop::Asin => (
            a.asin(),
            Flags {
                invalid_domain: !(-1.0..=1.0).contains(&a),
                ..Default::default()
            },
        ),
        Unop::Acos => (
            a.acos(),
            Flags {
                invalid_domain: !(-1.0..=1.0).contains(&a),
                ..Default::default()
            },
        ),
        Unop::Atan => (a.atan(), Flags::default()),
        Unop::Sinh => (a.sinh(), Flags::default()),
        Unop::Cosh => (a.cosh(), Flags::default()),
        Unop::Tanh => (a.tanh(), Flags::default()),
        Unop::Floor => (a.floor(), Flags::default()),
        Unop::Ceil => (a.ceil(), Flags::default()),
        Unop::Round => (a.round(), Flags::default()),
        Unop::Trunc => (a.trunc(), Flags::default()),
        Unop::Not => (if a == 0.0 { 1.0 } else { 0.0 }, Flags::default()),
        Unop::IsFinite => (if a.is_finite() { 1.0 } else { 0.0 }, Flags::default()),
        Unop::Cast(_) => (a, Flags::default()),
    };
    let r = round_mode(v, mode);
    (r, f.with_result(r))
}

/// Booleans are carried as 1.0 and 0.0 inside the numeric pipeline; this is where they turn back.
#[must_use]
pub fn un_bool(op: Unop, a: bool) -> bool {
    match op {
        Unop::Not => !a,
        Unop::IsFinite => true,
        _ => a,
    }
}

#[must_use]
pub fn call_f64(builtin: Builtin, args: &[f64], mode: FpMode) -> (f64, Flags) {
    let (v, f) = match builtin {
        Builtin::Atan2 => (args[0].atan2(args[1]), Flags::default()),
        Builtin::Hypot => (args[0].hypot(args[1]), Flags::default()),
        // `fma` is one rounding instead of two. That difference is the point: it is an independent
        // path to the same answer, not merely a faster one.
        Builtin::Fma => (args[0].mul_add(args[1], args[2]), Flags::default()),
        Builtin::Clamp => (
            args[0].clamp(args[1], args[2]),
            Flags {
                invalid_domain: args[1] > args[2],
                ..Default::default()
            },
        ),
        Builtin::Lerp => (args[0] + (args[1] - args[0]) * args[2], Flags::default()),
    };
    let r = round_mode(v, mode);
    (r, f.with_result(r))
}

/// Convert between the representations A-IR names.
#[must_use]
pub fn cast(to: NumType, v: f64) -> f64 {
    match to {
        // Unit carries no value, so it passes through like F64; the type is what changes, not the
        // number.
        NumType::F64 | NumType::Unit => v,
        NumType::F32 => f64::from(v as f32),
        NumType::I64 => v.trunc() as i64 as f64,
        NumType::Bool => f64::from(v != 0.0),
    }
}

trait WithResult {
    fn with_result(self, v: f64) -> Self;
}

impl WithResult for Flags {
    /// NaN and infinity are recorded where they are produced, so a report can tell "this model
    /// divides by zero" from "this model reaches infinity".
    fn with_result(mut self, v: f64) -> Self {
        if v.is_nan() {
            self.nan = true;
        } else if v.is_infinite() {
            self.inf = true;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_by_zero_is_recorded_not_raised() {
        let (v, f) = bin_f64(Binop::Div, 1.0, 0.0, FpMode::F64);
        assert!(v.is_infinite());
        assert!(f.zero_division && f.inf);
    }

    #[test]
    fn zero_over_zero_is_a_number_not_an_exception() {
        let (v, f) = bin_f64(Binop::Div, 0.0, 0.0, FpMode::F64);
        assert!(v.is_nan() && f.nan && f.zero_division);
    }

    #[test]
    fn sqrt_of_a_negative_marks_an_invalid_domain() {
        let (v, f) = un_f64(Unop::Sqrt, -4.0, FpMode::F64);
        assert!(v.is_nan());
        assert!(f.invalid_domain && f.nan);
    }

    #[test]
    fn log_of_zero_is_a_boundary_case_worth_seeing() {
        let (v, f) = un_f64(Unop::Ln, 0.0, FpMode::F64);
        assert!(v.is_infinite() && f.invalid_domain);
    }

    #[test]
    fn asin_outside_the_unit_interval_is_flagged() {
        let (_, f) = un_f64(Unop::Asin, 1.5, FpMode::F64);
        assert!(f.invalid_domain);
        let (_, ok) = un_f64(Unop::Asin, 0.5, FpMode::F64);
        assert!(ok.is_clean());
    }

    #[test]
    fn a_negative_base_with_a_fractional_exponent_is_reported() {
        let (v, f) = bin_f64(Binop::Pow, -2.0, 0.5, FpMode::F64);
        assert!(v.is_nan() && f.invalid_domain);
        let (v2, f2) = bin_f64(Binop::Pow, -2.0, 3.0, FpMode::F64);
        assert_eq!(v2, -8.0);
        assert!(f2.is_clean());
    }

    #[test]
    fn f32_mode_rounds_every_result() {
        let (v64, _) = bin_f64(Binop::Add, 1.0, 1e-9, FpMode::F64);
        let (v32, _) = bin_f64(Binop::Add, 1.0, 1e-9, FpMode::F32);
        assert!(v64 > 1.0);
        // In f32, 1 + 1e-9 is exactly 1: the tiny term disappears, which is precisely the kind of
        // precision dependence APORIA is meant to measure.
        assert_eq!(v32, 1.0);
    }

    #[test]
    fn integer_overflow_is_caught_before_it_wraps() {
        let (v, f) = bin_i64(Binop::Mul, i64::MAX, 2);
        assert_eq!(v, 0);
        assert!(f.integer_overflow);
    }

    #[test]
    fn fma_and_the_two_step_form_differ_by_a_rounding() {
        // The reason both spellings are in the language: they are two paths, not one path twice.
        let (a, _) = bin_f64(Binop::Mul, 1e10, 1.000_000_000_000_000_2, FpMode::F64);
        let (b, _) = bin_f64(Binop::Add, a, -1e10, FpMode::F64);
        let (c, _) = call_f64(
            Builtin::Fma,
            &[1e10, 1.000_000_000_000_000_2, -1e10],
            FpMode::F64,
        );
        assert_ne!(
            b, c,
            "fma should not be identical to multiply-then-add here"
        );
    }

    #[test]
    fn nan_compares_false_in_every_direction() {
        let n = f64::NAN;
        for op in [Binop::Lt, Binop::Le, Binop::Gt, Binop::Ge, Binop::Eq] {
            let (v, _) = compare_f64(op, n, n);
            assert_eq!(v, 0.0, "{op:?} of NaN must be false");
        }
        let (v, _) = compare_f64(Binop::Ne, n, n);
        assert_eq!(v, 1.0, "IEEE says NaN is not equal to itself");
    }

    #[test]
    fn casting_to_f32_loses_the_low_bits_and_nothing_else() {
        assert_eq!(cast(NumType::F32, 1.0 + f64::EPSILON), 1.0);
        assert_eq!(cast(NumType::F64, 2.5), 2.5);
        assert_eq!(cast(NumType::I64, 2.9), 2.0);
    }
}
