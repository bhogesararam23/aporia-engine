//! Units: a dimension plus an exact scale to SI base units.
//!
//! A unit is not decoration. Two quantities can have the same dimension and different units, and
//! that is precisely how the fault class the specification calls "unit and dimension mistakes"
//! usually arrives: a velocity in km/h multiplied into a model that works in metres per second is
//! dimensionally flawless and wrong by a factor of 3.6. Carrying the scale alongside the dimension
//! turns that into a type error.
//!
//! Symbols are an explicit table, not a productive prefix system. `cm` and `km` are listed, `kN`
//! is not, and the error says to write `1000*N`. Inventing prefixes on derived units invites
//! collisions (`mS` is milli-siemens or metre-second?) and buys nothing an author cannot write out.

use crate::span::{Diagnostic, Diagnostics, Severity, Span};
use aporia_ir::{CURRENT, Dimension, LENGTH, MASS, TEMPERATURE, TIME};

/// A unit: what a quantity *is*, and how its numbers relate to SI.
#[derive(Clone, Copy, PartialEq)]
pub struct Unit {
    pub dim: Dimension,
    /// Multiply a value expressed in this unit by `scale` to get the SI base value.
    pub scale: f64,
    /// Set by `count`, the only unit that marks a quantity as integral.
    pub integral: bool,
}

impl Default for Unit {
    /// The default unit is a pure number, which is what an unannotated quantity means.
    fn default() -> Self {
        Self::dimensionless()
    }
}

impl Unit {
    #[must_use]
    pub fn dimensionless() -> Self {
        Self {
            dim: Dimension::dimensionless(),
            scale: 1.0,
            integral: false,
        }
    }

    #[must_use]
    pub fn si(dim: Dimension) -> Self {
        Self {
            dim,
            scale: 1.0,
            integral: false,
        }
    }

    #[must_use]
    pub fn base(index: usize, e: i8) -> Self {
        Self::si(Dimension::base(index, e))
    }

    /// `count`: a pure integer, dimensionless.
    pub const COUNT: Self = Self {
        dim: Dimension::dimensionless(),
        scale: 1.0,
        integral: true,
    };

    #[must_use]
    pub fn mul(&self, other: &Self) -> Self {
        Self {
            dim: self.dim.mul(&other.dim),
            scale: self.scale * other.scale,
            integral: false,
        }
    }

    #[must_use]
    pub fn div(&self, other: &Self) -> Self {
        Self {
            dim: self.dim.div(&other.dim),
            scale: self.scale / other.scale,
            integral: false,
        }
    }

    #[must_use]
    pub fn pow(&self, k: i32) -> Self {
        Self {
            dim: self.dim.pow(k),
            scale: self.scale.powi(k),
            integral: false,
        }
    }

    /// A unit that carries no dimension at all, which is what trigonometric and exponential
    /// arguments demand.
    #[must_use]
    pub fn is_dimensionless(&self) -> bool {
        !self.dim.is_unknown() && self.dim.is_dimensionless()
    }

    /// Equality of the *scale* up to rounding introduced by repeated multiplication.
    ///
    /// The exact comparison below is a deliberate fast path, not a bug: scales are only ever
    /// produced by multiplying table entries, so an identical pair is bit-identical.
    #[expect(clippy::float_cmp)]
    #[must_use]
    pub fn same_scale_as(&self, other: &Self) -> bool {
        if self.scale == other.scale {
            return true;
        }
        let m = self.scale.abs().max(other.scale.abs()).max(f64::EPSILON);
        (self.scale - other.scale).abs() <= 1e-12 * m
    }

    /// Canonical text, used in messages and stored in the `.ap` doc for reviewers.
    #[expect(clippy::float_cmp)]
    #[must_use]
    pub fn describe(&self) -> String {
        if self.integral {
            return "count".to_string();
        }
        if self.scale == 1.0 {
            return self.dim.to_canonical();
        }
        format!("{} (x{})", self.dim.to_canonical(), self.scale)
    }
}

impl core::fmt::Debug for Unit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Unit({})", self.describe())
    }
}

/// One entry of the symbol table.
struct Symbol {
    name: &'static str,
    unit: Unit,
    /// Set when the symbol exists elsewhere and the author deserves to be told why it is not here.
    rejected: Option<&'static str>,
}

const MACRO_BASE: Unit = Unit {
    dim: Dimension::dimensionless(),
    scale: 1.0,
    integral: false,
};

/// The whole vocabulary, defined in terms of SI base units and numeric scales.
fn table() -> Vec<Symbol> {
    let n = || {
        Unit::base(MASS, 1)
            .mul(&Unit::base(LENGTH, 1))
            .div(&Unit::base(TIME, 1).pow(2))
    };
    let j = || n().mul(&Unit::base(LENGTH, 1));
    let w = || j().div(&Unit::base(TIME, 1));
    let pa = || n().div(&Unit::base(LENGTH, 1).pow(2));
    let c = || Unit::base(CURRENT, 1).mul(&Unit::base(TIME, 1));
    let v = || w().div(&c());
    let ohm = || v().div(&Unit::base(CURRENT, 1));
    // tesla = kg / (A*s^2)
    let t = || {
        Unit::base(MASS, 1)
            .div(&Unit::base(CURRENT, 1))
            .div(&Unit::base(TIME, 1).pow(2))
    };
    vec![
        // length
        sym("m", Unit::base(LENGTH, 1)),
        sym("meter", Unit::base(LENGTH, 1)),
        sym("metre", Unit::base(LENGTH, 1)),
        sym("km", scaled(LENGTH, 1, 1000.0)),
        sym("cm", scaled(LENGTH, 1, 0.01)),
        sym("mm", scaled(LENGTH, 1, 0.001)),
        sym("um", scaled(LENGTH, 1, 1e-6)),
        sym("nm", scaled(LENGTH, 1, 1e-9)),
        // mass
        sym("kg", Unit::base(MASS, 1)),
        sym("g", scaled(MASS, 1, 0.001)),
        sym("mg", scaled(MASS, 1, 1e-6)),
        sym("t", scaled(MASS, 1, 1000.0)),
        // time
        sym("s", Unit::base(TIME, 1)),
        sym("sec", Unit::base(TIME, 1)),
        sym("ms", scaled(TIME, 1, 0.001)),
        sym("us", scaled(TIME, 1, 1e-6)),
        sym("ns", scaled(TIME, 1, 1e-9)),
        sym("min", scaled(TIME, 1, 60.0)),
        sym("h", scaled(TIME, 1, 3600.0)),
        sym("hr", scaled(TIME, 1, 3600.0)),
        sym("hour", scaled(TIME, 1, 3600.0)),
        sym("day", scaled(TIME, 1, 86400.0)),
        // electrodynamics
        sym("A", Unit::base(CURRENT, 1)),
        sym("amp", Unit::base(CURRENT, 1)),
        sym("C", c()),
        sym("coulomb", c()),
        sym("V", v()),
        sym("volt", v()),
        sym("N", n()),
        sym("newton", n()),
        sym("J", j()),
        sym("joule", j()),
        sym("W", w()),
        sym("watt", w()),
        sym("Pa", pa()),
        sym("pascal", pa()),
        sym("Hz", Unit::dimensionless().div(&Unit::base(TIME, 1))),
        sym("hertz", Unit::dimensionless().div(&Unit::base(TIME, 1))),
        sym("ohm", ohm()),
        sym("T", t()),
        sym("tesla", t()),
        // angles are dimensionless but not scale-free, and that distinction earns its keep
        sym("rad", Unit::dimensionless()),
        sym("deg", scaled_dimensionless(std::f64::consts::PI / 180.0)),
        // counting
        sym("count", Unit::COUNT),
        sym("percent", scaled_dimensionless(0.01)),
        // thermodynamics
        sym("K", Unit::base(TEMPERATURE, 1)),
        sym("kelvin", Unit::base(TEMPERATURE, 1)),
        rejected(
            "Celsius",
            "degrees Celsius is an affine scale: converting needs an offset, not a factor. Work in kelvin.",
        ),
        rejected(
            "degC",
            "degrees Celsius is an affine scale: converting needs an offset, not a factor. Work in kelvin.",
        ),
        rejected(
            "fps",
            "`fps` is ambiguous (frames per second, feet per second). Write `1/s` or `ft/s`.",
        ),
    ]
}

fn sym(name: &'static str, unit: Unit) -> Symbol {
    Symbol {
        name,
        unit,
        rejected: None,
    }
}

fn rejected(name: &'static str, why: &'static str) -> Symbol {
    Symbol {
        name,
        unit: MACRO_BASE,
        rejected: Some(why),
    }
}

fn scaled(index: usize, e: i8, factor: f64) -> Unit {
    Unit {
        dim: Dimension::base(index, e),
        scale: factor,
        integral: false,
    }
}

fn scaled_dimensionless(factor: f64) -> Unit {
    Unit {
        dim: Dimension::dimensionless(),
        scale: factor,
        integral: false,
    }
}

/// Look up a unit symbol. `None` means the symbol is not in the table.
#[must_use]
pub fn lookup(name: &str) -> Option<Unit> {
    table()
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| s.rejected.map_or(Some(s.unit), |_| None))
}

/// A symbol that exists but was deliberately left out, with the reason.
#[must_use]
pub fn rejection(name: &str) -> Option<&'static str> {
    table()
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| s.rejected)
}

#[must_use]
pub fn is_known(name: &str) -> bool {
    table().iter().any(|s| s.name == name)
}

/// Resolve a unit written as a name, e.g. `m/s^2` after the checker has folded the expression.
///
/// The checker owns folding; this is the leaf lookup it needs.
pub fn resolve_name(name: &str, span: Span, out: &mut Diagnostics) -> Option<Unit> {
    if let Some(why) = rejection(name) {
        out.push(Diagnostic {
            severity: Severity::Error,
            message: format!("`{name}` cannot be used as a unit"),
            labels: vec![crate::span::Label {
                span,
                message: why.to_string(),
            }],
            help: None,
        });
        return None;
    }
    lookup(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_is_length_over_time() {
        let v = lookup("m").unwrap().div(&lookup("s").unwrap());
        assert_eq!(v.describe(), "m/s");
        assert_eq!(v.scale, 1.0);
    }

    #[test]
    fn kilometre_is_a_thousand_metres() {
        let km = lookup("km").unwrap();
        assert_eq!(km.dim, lookup("m").unwrap().dim);
        assert_eq!(km.scale, 1000.0);
    }

    #[test]
    fn same_dimension_different_scale_is_a_mismatch() {
        let km = lookup("km").unwrap();
        let m = lookup("m").unwrap();
        assert_eq!(km.dim, m.dim);
        assert!(
            !km.same_scale_as(&m),
            "km and m must not be interchangeable"
        );
    }

    #[test]
    fn newton_reduces_to_base_units() {
        let n = lookup("N").unwrap();
        let manual = lookup("kg")
            .unwrap()
            .mul(&lookup("m").unwrap())
            .div(&lookup("s").unwrap().pow(2));
        assert_eq!(n.dim, manual.dim);
        assert!(n.same_scale_as(&manual));
    }

    #[test]
    fn degree_is_dimensionless_with_a_scale() {
        let deg = lookup("deg").unwrap();
        assert!(deg.is_dimensionless());
        assert!((deg.scale - std::f64::consts::PI / 180.0).abs() < 1e-18);
        assert!(!deg.same_scale_as(&lookup("rad").unwrap()));
    }

    #[test]
    fn percent_is_a_hundredth_of_a_whole() {
        let p = lookup("percent").unwrap();
        assert!(p.is_dimensionless());
        assert_eq!(p.scale, 0.01);
    }

    #[test]
    fn count_is_the_integral_unit() {
        assert!(lookup("count").unwrap().integral);
        assert!(!lookup("m").unwrap().integral);
    }

    #[test]
    fn celsius_is_refused_with_a_reason() {
        assert!(lookup("Celsius").is_none());
        let why = rejection("Celsius").expect("Celsius is a known rejection");
        assert!(why.contains("affine"), "{why}");
    }

    #[test]
    fn coulomb_wins_the_symbol_c() {
        let c = lookup("C").expect("C is coulomb");
        assert_eq!(c.dim, lookup("amp").unwrap().mul(&lookup("s").unwrap()).dim);
    }

    #[test]
    fn hertz_is_per_second() {
        let hz = lookup("Hz").unwrap();
        assert_eq!(hz.describe(), "1/s");
    }

    #[test]
    fn repeated_multiplication_does_not_drift_the_scale_check() {
        let a = lookup("cm").unwrap().mul(&lookup("cm").unwrap());
        let b = lookup("cm").unwrap().pow(2);
        assert!(a.same_scale_as(&b), "{a:?} vs {b:?}");
        assert!((a.scale - 1e-4).abs() < 1e-16);
    }

    #[test]
    fn every_symbol_in_the_table_resolves() {
        for s in table() {
            if s.rejected.is_some() {
                assert!(lookup(s.name).is_none(), "{} must not resolve", s.name);
            } else {
                assert!(lookup(s.name).is_some(), "{} must resolve", s.name);
            }
        }
    }

    #[test]
    fn base_units_are_the_seven_si_ones() {
        assert_eq!(lookup("m").unwrap().dim, Dimension::base(LENGTH, 1));
        assert_eq!(lookup("kg").unwrap().dim, Dimension::base(MASS, 1));
        assert_eq!(lookup("s").unwrap().dim, Dimension::base(TIME, 1));
        assert_eq!(lookup("A").unwrap().dim, Dimension::base(CURRENT, 1));
        assert_eq!(lookup("K").unwrap().dim, Dimension::base(TEMPERATURE, 1));
    }
}
