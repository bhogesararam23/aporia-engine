//! Runtime values, floating-point modes and the flags an execution can raise.

use aporia_ir::{Lit, NumType};

/// A value as it exists during execution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    F64(f64),
    F32(f32),
    I64(i64),
    Bool(bool),
    /// The result of an instruction that computes nothing: a loop or a slot write.
    Unit,
}

impl Value {
    #[must_use]
    pub fn from_lit(l: Lit) -> Self {
        match l {
            Lit::F64(v) => Self::F64(v),
            Lit::F32(v) => Self::F32(v),
            Lit::I64(v) => Self::I64(v),
            Lit::Bool(b) => Self::Bool(b),
        }
    }

    /// The f64 reading of any scalar value. Booleans become 1 and 0 because that is how they enter
    /// arithmetic in a model, and a comparison of a boolean is reported before it gets here.
    #[must_use]
    pub fn as_f64(self) -> f64 {
        match self {
            Self::F64(v) => v,
            Self::F32(v) => f64::from(v),
            Self::I64(v) => v as f64,
            Self::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Unit => f64::NAN,
        }
    }

    #[must_use]
    pub fn as_i64(self) -> Option<i64> {
        match self {
            Self::I64(v) => Some(v),
            Self::Bool(b) => Some(if b { 1 } else { 0 }),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(b),
            _ => None,
        }
    }

    #[must_use]
    pub fn num(self) -> NumType {
        match self {
            Self::F64(_) => NumType::F64,
            Self::F32(_) => NumType::F32,
            Self::I64(_) => NumType::I64,
            Self::Bool(_) => NumType::Bool,
            Self::Unit => NumType::Unit,
        }
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        match self {
            Self::F64(v) => v.is_finite(),
            Self::F32(v) => v.is_finite(),
            Self::Unit => false,
            _ => true,
        }
    }
}

/// Anything unusual an execution ran into.
///
/// These are not errors and they do not stop evaluation: a scientific model that reaches infinity
/// at dt = 0.041 is telling APORIA exactly what the project is looking for. The flags exist so the
/// story survives to the evidence layer instead of being a NaN nobody recorded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub nan: bool,
    pub inf: bool,
    pub zero_division: bool,
    /// A real-valued function given input outside its domain: `sqrt(-1)`, `ln(0)`, `asin(2)`.
    pub invalid_domain: bool,
    /// An integer operation that could not be represented.
    pub integer_overflow: bool,
    /// The loop guard fired: the model asked for more iterations than the budget allows.
    pub step_budget: bool,
}

impl Flags {
    #[must_use]
    pub fn clean() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_clean(self) -> bool {
        self == Self::default()
    }

    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self {
            nan: self.nan || other.nan,
            inf: self.inf || other.inf,
            zero_division: self.zero_division || other.zero_division,
            invalid_domain: self.invalid_domain || other.invalid_domain,
            integer_overflow: self.integer_overflow || other.integer_overflow,
            step_budget: self.step_budget || other.step_budget,
        }
    }

    /// One byte for the experiment store, in a fixed order that must not change without a format
    /// version bump.
    #[must_use]
    pub fn to_bits(self) -> u8 {
        let mut b = 0u8;
        if self.nan {
            b |= 1;
        }
        if self.inf {
            b |= 2;
        }
        if self.zero_division {
            b |= 4;
        }
        if self.invalid_domain {
            b |= 8;
        }
        if self.integer_overflow {
            b |= 16;
        }
        if self.step_budget {
            b |= 32;
        }
        b
    }

    #[must_use]
    pub fn from_bits(b: u8) -> Self {
        Self {
            nan: b & 1 != 0,
            inf: b & 2 != 0,
            zero_division: b & 4 != 0,
            invalid_domain: b & 8 != 0,
            integer_overflow: b & 16 != 0,
            step_budget: b & 32 != 0,
        }
    }

    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.nan {
            v.push("nan");
        }
        if self.inf {
            v.push("inf");
        }
        if self.zero_division {
            v.push("zero-division");
        }
        if self.invalid_domain {
            v.push("invalid-domain");
        }
        if self.integer_overflow {
            v.push("integer-overflow");
        }
        if self.step_budget {
            v.push("step-budget");
        }
        v
    }
}

/// How arithmetic is carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FpMode {
    /// Native f64 everywhere. The ordinary path.
    F64,
    /// Every operation rounds to f32. Not a faster path: an intentionally weaker one, so the
    /// distance between it and f64 measures how much the answer depends on precision.
    F32,
}

impl FpMode {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::F64 => "f64",
            Self::F32 => "f32",
        }
    }
}

/// The execution configuration one experiment records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExecConfig {
    pub fp: FpMode,
    /// An instruction-execution budget per candidate. A model whose time step is small enough to
    /// look like a hang is a finding, not something to wait out.
    pub max_steps: u64,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            fp: FpMode::F64,
            max_steps: 50_000_000,
        }
    }
}

impl ExecConfig {
    #[must_use]
    pub fn with_fp(mut self, fp: FpMode) -> Self {
        self.fp = fp;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_bits_round_trip() {
        for bits in 0u8..=63 {
            assert_eq!(Flags::from_bits(bits).to_bits(), bits);
        }
    }

    #[test]
    fn merge_is_a_union_and_keeps_earlier_signals() {
        let a = Flags {
            nan: true,
            ..Default::default()
        };
        let b = Flags {
            inf: true,
            ..Default::default()
        };
        let m = a.merge(b);
        assert!(m.nan && m.inf);
        assert!(a.is_clean() == false);
    }

    #[test]
    fn unit_reads_as_not_a_number() {
        assert!(!Value::Unit.is_finite());
        assert!(Value::Unit.as_f64().is_nan());
    }

    #[test]
    fn booleans_enter_arithmetic_as_one_and_zero() {
        assert_eq!(Value::Bool(true).as_f64(), 1.0);
        assert_eq!(Value::Bool(false).as_i64(), Some(0));
    }

    #[test]
    fn f32_values_widen_without_surprise() {
        assert_eq!(Value::F32(1.5).as_f64(), 1.5);
        assert_eq!(Value::F32(0.1).as_f64(), 0.1f32 as f64);
    }

    #[test]
    fn names_list_every_flag_that_fired() {
        let f = Flags {
            nan: true,
            zero_division: true,
            ..Default::default()
        };
        assert_eq!(f.names(), vec!["nan", "zero-division"]);
    }

    #[test]
    fn default_config_has_a_bounded_step_budget() {
        let c = ExecConfig::default();
        assert_eq!(c.fp, FpMode::F64);
        assert!(c.max_steps > 0 && c.max_steps <= 100_000_000);
    }
}
