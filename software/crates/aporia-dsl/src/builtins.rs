//! The function vocabulary of the language.
//!
//! One table answers three questions — is this name a function, how many arguments does it take,
//! and can it be folded into a constant — so the parser, the checker and the lowering all agree
//! without repeating themselves.

/// A builtin and how it is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Func {
    pub name: &'static str,
    pub arity: usize,
    /// True when the result is a pure function of its arguments with no dependence on the model's
    /// parameters, i.e. it may appear in a unit annotation or a domain bound.
    pub constant: bool,
    /// True when the result is a predicate rather than a number.
    pub predicate: bool,
}

pub const FUNCS: &[Func] = &[
    Func {
        name: "abs",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "sqrt",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "cbrt",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "exp",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "ln",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "log2",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "log10",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "sin",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "cos",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "tan",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "asin",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "acos",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "atan",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "sinh",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "cosh",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "tanh",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "floor",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "ceil",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "round",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "trunc",
        arity: 1,
        constant: true,
        predicate: false,
    },
    Func {
        name: "atan2",
        arity: 2,
        constant: true,
        predicate: false,
    },
    Func {
        name: "hypot",
        arity: 2,
        constant: true,
        predicate: false,
    },
    Func {
        name: "min",
        arity: 2,
        constant: true,
        predicate: false,
    },
    Func {
        name: "max",
        arity: 2,
        constant: true,
        predicate: false,
    },
    Func {
        name: "pow",
        arity: 2,
        constant: true,
        predicate: false,
    },
    Func {
        name: "clamp",
        arity: 3,
        constant: true,
        predicate: false,
    },
    Func {
        name: "lerp",
        arity: 3,
        constant: true,
        predicate: false,
    },
    // `fma` is in the vocabulary because a fused multiply-add is an independent numerical path,
    // not just a faster one: writing `fma(a, b, c)` next to `a*b + c` gives APORIA two ways to get
    // the same answer and therefore something to compare.
    Func {
        name: "fma",
        arity: 3,
        constant: true,
        predicate: false,
    },
    Func {
        name: "finite",
        arity: 1,
        constant: false,
        predicate: true,
    },
];

#[must_use]
pub fn lookup(name: &str) -> Option<&'static Func> {
    FUNCS.iter().find(|f| f.name == name)
}

#[must_use]
pub fn is_constant_callable(name: &str) -> bool {
    lookup(name).is_some_and(|f| f.constant)
}

/// Names that mean the same thing in every model and cannot be shadowed by a parameter.
pub const NAMED_CONSTANTS: &[(&str, f64)] = &[
    ("pi", std::f64::consts::PI),
    ("tau", std::f64::consts::TAU),
    ("e", std::f64::consts::E),
    ("sqrt2", std::f64::consts::SQRT_2),
    ("inf", f64::INFINITY),
];

#[must_use]
pub fn named_constant(name: &str) -> Option<f64> {
    NAMED_CONSTANTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_function_name_is_unique_and_usable() {
        for (i, a) in FUNCS.iter().enumerate() {
            assert_ne!(a.name, "", "a builtin needs a name");
            assert!(a.arity >= 1);
            for b in FUNCS.iter().skip(i + 1) {
                assert_ne!(a.name, b.name, "duplicate builtin");
            }
        }
    }

    #[test]
    fn lookup_finds_and_rejects() {
        assert_eq!(lookup("sqrt").map(|f| f.arity), Some(1));
        assert_eq!(lookup("clamp").map(|f| f.arity), Some(3));
        assert!(lookup("numpy").is_none());
    }

    #[test]
    fn finite_is_a_predicate_and_not_foldable() {
        let f = lookup("finite").unwrap();
        assert!(f.predicate);
        assert!(!f.constant);
        assert!(!is_constant_callable("finite"));
        assert!(is_constant_callable("sin"));
    }

    #[test]
    fn named_constants_are_the_ones_a_model_would_expect() {
        assert_eq!(named_constant("pi"), Some(std::f64::consts::PI));
        assert_eq!(named_constant("gravity"), None);
    }
}
