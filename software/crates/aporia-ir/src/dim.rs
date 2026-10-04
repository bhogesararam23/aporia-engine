//! Numeric types and dimensional algebra.
//!
//! A-IR values have two independent type components: the *representation*
//! ([`NumType`]) and the *physical dimension* ([`Dimension`]). Keeping them apart is what lets
//! APORIA tell "this expression adds metres to seconds" (a real modelling bug, caught before any
//! execution) from "this expression overflows at dt = 0.041" (an empirical question).
//!
//! Exponents are stored doubled. `sqrt(m)` is `m^0.5`, and doubling keeps that representable in an
//! integer without ever dividing at the use site. `DISPLAY`-facing code divides by two.

use std::fmt;
use std::fmt::Write as _;

/// Index of each SI base dimension inside [`Dimension::exp`].
pub const LENGTH: usize = 0;
pub const MASS: usize = 1;
pub const TIME: usize = 2;
pub const CURRENT: usize = 3;
pub const TEMPERATURE: usize = 4;
pub const AMOUNT: usize = 5;
pub const LUMINOUS: usize = 6;

const BASE_SYMBOLS: [&str; 7] = ["m", "kg", "s", "A", "K", "mol", "cd"];

/// Print order for base dimensions. SI writes mass before length, so the canonical text form is
/// `kg*m/s^2` rather than `m*kg/s^2`; parsing accepts either because it keys on the symbol.
const ORDER: [usize; 7] = [MASS, LENGTH, TIME, CURRENT, TEMPERATURE, AMOUNT, LUMINOUS];

/// A physical dimension as seven integer base exponents, each multiplied by two.
///
/// `unknown` marks a dimension that could not be derived — the usual cause is a variable exponent
/// such as `x^p`. Propagation through an unknown is always unknown, and the front end decides
/// whether that is an error or merely a lost check.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dimension {
    exp: [i8; 7],
    unknown: bool,
}

impl Dimension {
    /// The dimension of a pure number.
    #[must_use]
    pub const fn dimensionless() -> Self {
        Self {
            exp: [0; 7],
            unknown: false,
        }
    }

    /// A dimension APORIA could not work out.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            exp: [0; 7],
            unknown: true,
        }
    }

    /// Build a dimension from base exponents given in ordinary units (not doubled).
    ///
    /// Build a dimension from exponents that are already doubled, which is what
    /// [`Dimension::doubled`] reports. `exp[TIME] == -4` is therefore "per second squared".
    #[must_use]
    pub fn from_halves(exp: [i8; 7]) -> Self {
        Self {
            exp,
            unknown: false,
        }
    }

    /// A dimension with one base raised to `e`.
    #[must_use]
    pub fn base(index: usize, e: i8) -> Self {
        let mut exp = [0i8; 7];
        exp[index] = e * 2;
        Self {
            exp,
            unknown: false,
        }
    }

    #[must_use]
    pub fn is_unknown(&self) -> bool {
        self.unknown
    }

    /// The doubled exponent of base dimension `index`.
    #[must_use]
    pub fn doubled(&self, index: usize) -> i8 {
        self.exp[index]
    }

    /// The ordinary exponent of base dimension `index`, or `None` when it is fractional.
    #[must_use]
    pub fn exponent(&self, index: usize) -> Option<f64> {
        if self.unknown {
            None
        } else {
            Some(self.exp[index] as f64 / 2.0)
        }
    }

    #[must_use]
    pub fn is_dimensionless(&self) -> bool {
        !self.unknown && self.exp == [0; 7]
    }

    /// `self * other` in the dimensional sense (add exponents).
    #[must_use]
    pub fn mul(&self, other: &Self) -> Self {
        if self.unknown || other.unknown {
            return Self::unknown();
        }
        let exp = std::array::from_fn(|i| self.exp[i].saturating_add(other.exp[i]));
        Self {
            exp,
            unknown: false,
        }
    }

    /// `self / other` in the dimensional sense (subtract exponents).
    #[must_use]
    pub fn div(&self, other: &Self) -> Self {
        if self.unknown || other.unknown {
            return Self::unknown();
        }
        let exp = std::array::from_fn(|i| self.exp[i].saturating_sub(other.exp[i]));
        Self {
            exp,
            unknown: false,
        }
    }

    /// Raise to an integer power.
    #[must_use]
    pub fn pow(&self, k: i32) -> Self {
        if self.unknown {
            return Self::unknown();
        }
        // `saturating_mul` on i8 is already the clamp; a power that overflows the exponent
        // range is a modelling error the caller sees as a saturated dimension, never a wrap.
        let factor = i8::try_from(k).unwrap_or(i8::MAX);
        let exp = std::array::from_fn(|i| self.exp[i].saturating_mul(factor));
        Self {
            exp,
            unknown: false,
        }
    }

    /// Square root: halves every exponent. `None` when any exponent is odd, which is the
    /// "this quantity has no clean dimensional square root" case.
    #[must_use]
    pub fn sqrt(&self) -> Option<Self> {
        if self.unknown {
            return Some(Self::unknown());
        }
        if self.exp.iter().any(|&e| e % 2 != 0) {
            return None;
        }
        let exp = std::array::from_fn(|i| self.exp[i] / 2);
        Some(Self {
            exp,
            unknown: false,
        })
    }

    /// Cube root, same reasoning in thirds.
    #[must_use]
    pub fn cbrt(&self) -> Option<Self> {
        if self.unknown {
            return Some(Self::unknown());
        }
        // Doubled exponents are always even, so divisibility by six is exactly "the real
        // exponent is divisible by three", which is when a cube root stays representable.
        if self.exp.iter().any(|&e| e % 6 != 0) {
            return None;
        }
        let exp = std::array::from_fn(|i| self.exp[i] / 3);
        Some(Self {
            exp,
            unknown: false,
        })
    }

    /// Canonical text form used by the `.air` format, e.g. `kg^1*m^1*s^-2` or `1`.
    #[must_use]
    pub fn to_canonical(&self) -> String {
        if self.unknown {
            return "?".to_string();
        }
        if self.is_dimensionless() {
            return "1".to_string();
        }
        let mut out = String::new();
        let mut numerator = String::new();
        let mut denominator = String::new();
        for &index in &ORDER {
            let doubled = self.exp[index];
            if doubled == 0 {
                continue;
            }
            let magnitude = doubled.abs();
            let whole = magnitude / 2;
            let half = magnitude % 2 == 1;
            let mut piece = String::from(BASE_SYMBOLS[index]);
            // Exponents are stored doubled, so a plain single unit is magnitude 2 and prints bare:
            // `m`, not `m^1`.
            if magnitude != 2 {
                let _ = if half {
                    write!(piece, "^{whole}.5")
                } else {
                    write!(piece, "^{whole}")
                };
            }
            if doubled > 0 {
                if !numerator.is_empty() {
                    numerator.push('*');
                }
                numerator.push_str(&piece);
            } else {
                if !denominator.is_empty() {
                    denominator.push('*');
                }
                denominator.push_str(&piece);
            }
        }
        if denominator.is_empty() {
            out.push_str(&numerator);
        } else if numerator.is_empty() {
            out.push_str("1/");
            out.push_str(&denominator);
        } else {
            out.push_str(&numerator);
            out.push('/');
            out.push_str(&denominator);
        }
        out
    }

    /// Parse the canonical form produced by [`Dimension::to_canonical`].
    pub fn parse_canonical(text: &str) -> Result<Self, DimParseError> {
        if text == "?" {
            return Ok(Self::unknown());
        }
        if text == "1" {
            return Ok(Self::dimensionless());
        }
        let mut exp = [0i8; 7];
        let (num, den) = match text.split_once('/') {
            Some((n, d)) => (n, d),
            None => (text, ""),
        };
        if num.is_empty() {
            parse_side(den, &mut exp, -1)?;
        } else {
            parse_side(num, &mut exp, 1)?;
            if !den.is_empty() {
                parse_side(den, &mut exp, -1)?;
            }
        }
        Ok(Self {
            exp,
            unknown: false,
        })
    }
}

fn parse_side(side: &str, exp: &mut [i8; 7], sign: i8) -> Result<(), DimParseError> {
    for term in side.split('*') {
        if term.is_empty() || term == "1" {
            // The numerator of `1/s^2`.
            continue;
        }
        let (sym, power) = match term.split_once('^') {
            Some((s, p)) => (
                s,
                p.parse::<f64>()
                    .map_err(|_| DimParseError(term.to_string()))?,
            ),
            None => (term, 1.0),
        };
        let index = BASE_SYMBOLS
            .iter()
            .position(|s| *s == sym)
            .ok_or_else(|| DimParseError(term.to_string()))?;
        let doubled = power * 2.0;
        if doubled.fract() != 0.0 {
            return Err(DimParseError(term.to_string()));
        }
        exp[index] = i8::try_from((doubled as i32) * i32::from(sign))
            .map_err(|_| DimParseError(term.to_string()))?;
    }
    Ok(())
}

/// A dimension string that is not in canonical form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimParseError(pub String);

impl fmt::Display for DimParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unrecognised dimension term `{}`", self.0)
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_canonical())
    }
}

impl fmt::Debug for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Dimension({self})")
    }
}

/// How a value is represented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NumType {
    F64,
    F32,
    I64,
    Bool,
    /// The type of an instruction that computes no value: a loop or a slot write.
    Unit,
}

impl NumType {
    #[must_use]
    pub fn is_float(&self) -> bool {
        matches!(self, Self::F64 | Self::F32)
    }

    /// Width in bytes of one element, used by the binary observation format.
    #[must_use]
    pub fn width(&self) -> usize {
        match self {
            Self::F64 | Self::I64 => 8,
            Self::F32 => 4,
            Self::Bool => 1,
            Self::Unit => 0,
        }
    }
}

/// The type of an A-IR value: representation plus physical dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ty {
    pub num: NumType,
    pub dim: Dimension,
}

impl Ty {
    #[must_use]
    pub fn float(dim: Dimension) -> Self {
        Self {
            num: NumType::F64,
            dim,
        }
    }

    #[must_use]
    pub fn dimensionless_f64() -> Self {
        Self {
            num: NumType::F64,
            dim: Dimension::dimensionless(),
        }
    }

    #[must_use]
    pub fn count() -> Self {
        Self {
            num: NumType::I64,
            dim: Dimension::dimensionless(),
        }
    }

    #[must_use]
    pub fn boolean() -> Self {
        Self {
            num: NumType::Bool,
            dim: Dimension::dimensionless(),
        }
    }

    #[must_use]
    pub fn unit() -> Self {
        Self {
            num: NumType::Unit,
            dim: Dimension::dimensionless(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_divides_length_by_time() {
        let v = Dimension::base(LENGTH, 1).div(&Dimension::base(TIME, 1));
        assert_eq!(v.to_canonical(), "m/s");
    }

    #[test]
    fn acceleration_prints_exponent() {
        let a = Dimension::base(LENGTH, 1).div(&Dimension::base(TIME, 1).pow(2));
        assert_eq!(a.to_canonical(), "m/s^2");
    }

    #[test]
    fn newton_expands_to_base_dimensions() {
        // N = kg*m/s^2
        let n = Dimension::base(MASS, 1)
            .mul(&Dimension::base(LENGTH, 1))
            .div(&Dimension::base(TIME, 1).pow(2));
        assert_eq!(n.to_canonical(), "kg*m/s^2");
    }

    #[test]
    fn stiffness_per_mass_is_inverse_time_squared() {
        // N/m / kg = (kg*m/s^2)/m/kg = 1/s^2
        let k = Dimension::base(MASS, 1)
            .mul(&Dimension::base(LENGTH, 1))
            .div(&Dimension::base(TIME, 1).pow(2))
            .div(&Dimension::base(LENGTH, 1))
            .div(&Dimension::base(MASS, 1));
        assert_eq!(k.to_canonical(), "1/s^2");
    }

    #[test]
    fn half_powers_survive_a_round_trip() {
        let d = Dimension::base(LENGTH, 1).sqrt().expect("m^0.5 exists");
        assert_eq!(d.to_canonical(), "m^0.5");
        assert_eq!(Dimension::parse_canonical(&d.to_canonical()).unwrap(), d);
    }

    #[test]
    fn a_half_power_has_no_dimensional_sqrt() {
        // (m^0.5)^0.5 is m^0.25, which doubled exponents cannot hold.
        let half = Dimension::base(LENGTH, 1).sqrt().unwrap();
        assert!(half.sqrt().is_none());
    }

    #[test]
    fn canonical_round_trip_examples() {
        for text in [
            "1",
            "m",
            "m/s",
            "kg*m/s^2",
            "m^0.5",
            "1/s^2",
            "kg*m^2/s^2",
            "?",
        ] {
            let parsed = Dimension::parse_canonical(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(parsed.to_canonical(), text, "round trip");
        }
    }

    #[test]
    fn unknown_propagates() {
        let u = Dimension::unknown();
        assert!(u.mul(&Dimension::base(LENGTH, 1)).is_unknown());
        assert!(u.pow(2).is_unknown());
        assert_eq!(u.to_canonical(), "?");
    }
}
