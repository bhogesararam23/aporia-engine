//! A failure *description*, as opposed to a failing point.
//!
//! The minimizer's output cannot be a coordinate: "the model breaks at x = 0.3171563941" is only
//! interesting once you know which of those digits matter and which parameters matter at all. So a
//! [`Case`] keeps one [`Axis`] per parameter, and each axis is in one of three states — pinned to a
//! value, narrowed to a band, or dropped because the failure does not depend on it.
//!
//! The states are claims, and each claim is checked rather than asserted. A dropped parameter means
//! "the failure survived when this parameter was set to samples across its whole declared domain",
//! not "the failure is independent of this parameter", and the sample points are listed by
//! [`Case::witnesses`] so a reader can see exactly what was executed. Turning a drop into a proof
//! would need symbolic reasoning about the model, which is not what this project claims to do.

use aporia_ir::{Domain, Model};

use crate::oracle::Oracle;

/// The declared extent of one parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    /// A continuous interval, inclusive as declared.
    Continuous { lo: f64, hi: f64 },
    /// A list of settings, such as a solver index or a fixed set of test points.
    Discrete(Vec<f64>),
}

impl Span {
    fn from(domain: &Domain) -> Self {
        match domain {
            Domain::Interval { lo, hi } => Self::Continuous { lo: *lo, hi: *hi },
            Domain::Choices(v) => Self::Discrete(v.clone()),
        }
    }

    /// The value used when this axis carries no information.
    #[must_use]
    pub fn centre(&self) -> f64 {
        match self {
            Self::Continuous { lo, hi } => lo + (hi - lo) / 2.0,
            Self::Discrete(v) if v.is_empty() => 0.0,
            Self::Discrete(v) => v[v.len() / 2],
        }
    }

    #[must_use]
    pub fn width(&self) -> f64 {
        match self {
            Self::Continuous { lo, hi } => hi - lo,
            Self::Discrete(v) => v.len() as f64,
        }
    }
}

/// What a minimised case currently says about one parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum AxisState {
    /// The failure survived samples across the whole declared domain, so this parameter is not part
    /// of the explanation.
    Dropped,
    /// The failure needs this value. `digits` is the number of significant digits that still
    /// reproduce it, once reduction has run.
    Pinned { value: f64, digits: Option<u8> },
    /// The failure needs *some* value in this interval, and every sampled point inside it violated.
    /// `digits` shortens both edges together, since a printed interval is only as long as its
    /// longest number.
    Band {
        lo: f64,
        hi: f64,
        digits: Option<u8>,
    },
}

/// One parameter's role in the case.
#[derive(Clone, Debug, PartialEq)]
pub struct Axis {
    pub name: String,
    pub span: Span,
    pub state: AxisState,
}

/// A failure description over the whole parameter vector.
#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub axes: Vec<Axis>,
}

impl Case {
    /// The unminimised case: every parameter pinned to the value that failed.
    #[must_use]
    pub fn from_model(model: &Model, x0: &[f64]) -> Self {
        let axes = model
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| Axis {
                name: p.name.clone(),
                span: Span::from(&p.domain),
                state: AxisState::Pinned {
                    value: x0.get(i).copied().unwrap_or(p.domain.midpoint()),
                    digits: None,
                },
            })
            .collect();
        Self { axes }
    }

    #[must_use]
    pub fn arity(&self) -> usize {
        self.axes.len()
    }

    /// Parameters the case still talks about.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        self.axes
            .iter()
            .filter(|a| !matches!(a.state, AxisState::Dropped))
            .count()
    }

    /// Total significant digits printed across the case. Together with [`Case::dimensions`] this is
    /// the counterexample-size metric: a smaller number is a more useful report, and it is only
    /// meaningful when the violation still reproduces.
    #[must_use]
    pub fn digits(&self) -> u32 {
        self.axes
            .iter()
            .map(|a| match a.state {
                AxisState::Dropped => 0,
                AxisState::Pinned {
                    digits: Some(d), ..
                } => u32::from(d),
                AxisState::Pinned { digits: None, .. } => 17,
                AxisState::Band {
                    digits: Some(d), ..
                } => 2 * u32::from(d),
                AxisState::Band { digits: None, .. } => 36,
            })
            .sum()
    }

    /// The single point this case stands on: pinned values, band midpoints, domain centres for
    /// dropped axes.
    #[must_use]
    pub fn representative(&self) -> Vec<f64> {
        self.axes
            .iter()
            .map(|a| match a.state {
                AxisState::Dropped => a.span.centre(),
                AxisState::Pinned { value, .. } => value,
                AxisState::Band { lo, hi, .. } => lo + (hi - lo) / 2.0,
            })
            .collect()
    }

    /// The executions that make the case's claims. Always includes the representative point, then a
    /// star: for every axis, the sample points that state depends on.
    ///
    /// A star rather than a cross product because the product is `5^n` and the claim each state makes
    /// is per-axis: dropping parameter `k` means the failure survived across `k`'s domain *with the
    /// other parameters held at the case's own values*. That is the claim being verified, and a
    /// cross product would be verifying something the report does not say.
    #[must_use]
    pub fn witnesses(&self) -> Vec<Vec<f64>> {
        let base = self.representative();
        let mut out = vec![base.clone()];
        for (i, a) in self.axes.iter().enumerate() {
            for v in sample_points(&a.span, &a.state) {
                if (v - base[i]).abs() <= f64::EPSILON * v.abs().max(1.0) {
                    continue;
                }
                let mut x = base.clone();
                x[i] = v;
                out.push(x);
            }
        }
        out
    }

    /// Check every witness. `budget` caps the number of oracle *calls*, because verification cost is
    /// part of what a minimiser has to be judged on — and the calls are not the executions, which is
    /// why a [`Verify`] reports both.
    ///
    /// `engine` is threaded to every call rather than held here: the witnesses of one case are asked
    /// of the same execution path that produced the finding they came from.
    #[must_use]
    pub fn verify(
        &self,
        oracle: &dyn Oracle,
        engine: &mut dyn aporia_runtime::Executor,
        budget: u64,
    ) -> Verify {
        let mut queries = 0;
        let mut executions = 0;
        for x in self.witnesses() {
            // The budget is checked before the call, not after, so `queries` stays the number of
            // answers actually obtained. It used to count the refusal as a call, which made a
            // budget-limited run report one query more than anything had answered.
            if queries >= budget {
                return Verify {
                    holds: false,
                    queries,
                    executions,
                    over_budget: true,
                };
            }
            queries += 1;
            let verdict = oracle.query(&x, engine);
            executions += verdict.executions;
            if !verdict.violating {
                return Verify {
                    holds: false,
                    queries,
                    executions,
                    over_budget: false,
                };
            }
        }
        Verify {
            holds: true,
            queries,
            executions,
            over_budget: false,
        }
    }

    /// The case with one axis replaced.
    #[must_use]
    pub fn with(&self, index: usize, state: AxisState) -> Self {
        let mut next = self.clone();
        next.axes[index].state = state;
        next
    }

    /// A report line per parameter, in the shape the finding format wants.
    #[must_use]
    pub fn describe(&self) -> String {
        self.axes
            .iter()
            .map(|a| match a.state {
                AxisState::Dropped => format!("{}: not part of the failure", a.name),
                AxisState::Pinned { value, digits } => {
                    format!("{} = {}", a.name, fmt_sig(value, digits))
                }
                AxisState::Band { lo, hi, digits } => format!(
                    "{} in [{}, {}]",
                    a.name,
                    fmt_sig(lo, digits),
                    fmt_sig(hi, digits)
                ),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Outcome of checking a case, with the cost it took in both units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verify {
    pub holds: bool,
    /// Oracle calls made, which is the unit the budget is stated in.
    pub queries: u64,
    /// Model executions those calls performed. Equal to `queries` for an oracle that runs the model
    /// once per point, and larger for one that measures a star around it.
    pub executions: u64,
    /// Set when the budget ran out rather than a witness passing: a budget refusal is not evidence
    /// that the case is wrong, and the two must not be reported as the same thing.
    pub over_budget: bool,
}

/// The points a state depends on, for one axis.
fn sample_points(span: &Span, state: &AxisState) -> Vec<f64> {
    match (span, state) {
        // A dropped continuous axis is claimed irrelevant over its whole domain, so the domain gets
        // sampled at its edges and quartiles rather than trusted.
        (Span::Continuous { lo, hi }, AxisState::Dropped) => {
            let w = hi - lo;
            vec![*lo, lo + w * 0.25, lo + w * 0.5, lo + w * 0.75, *hi]
        }
        (Span::Discrete(v), AxisState::Dropped) => v.clone(),
        // The band's own edges and quarter points, not the declared domain's: the claim is about
        // this interval, and sampling the domain instead would verify a different statement.
        (Span::Continuous { .. }, AxisState::Band { lo, hi, .. }) => {
            let w = hi - lo;
            vec![*lo, lo + w * 0.25, lo + w * 0.75, *hi]
        }
        // Nothing extra to execute: a pinned value is the representative itself, and a discrete
        // axis cannot be narrowed to a band without naming which choices survive, which is a
        // different claim than the one `Band` makes.
        (_, AxisState::Pinned { .. }) | (Span::Discrete(_), AxisState::Band { .. }) => Vec::new(),
    }
}

fn pow10(e: i32) -> f64 {
    10f64.powi(e)
}

/// The exponent of the leading significant digit.
fn leading(value: f64) -> i32 {
    if value == 0.0 || !value.is_finite() {
        return 0;
    }
    value.abs().log10().floor() as i32
}

/// Round to `d` significant digits, nearest.
///
/// The decimal path is deliberate: `(v / 1e-5).round() * 1e-5` lands one ulp off `0.00012` because
/// the divisor is not exact, and a number that will be printed in a report should be the double
/// closest to the digits being claimed, not the closest to an artefact of how the scaling was done.
#[must_use]
pub fn round_sig(value: f64, d: u8) -> f64 {
    if !value.is_finite() || value == 0.0 || d == 0 {
        return value;
    }
    let decimals = i32::from(d) - 1 - leading(value);
    if (0..=17).contains(&decimals) {
        let decimals = decimals as usize;
        let text = format!("{value:.decimals$}");
        if let Ok(rounded) = text.parse::<f64>() {
            return rounded;
        }
    }
    let factor = pow10(leading(value) - i32::from(d) + 1);
    (value / factor).round() * factor
}

/// Print a value with exactly `d` significant digits when given one, and compactly otherwise.
#[must_use]
pub fn fmt_sig(value: f64, d: Option<u8>) -> String {
    let Some(d) = d else { return trim(value) };
    if !value.is_finite() {
        return format!("{value}");
    }
    let decimals = i32::from(d) - 1 - leading(value);
    if decimals <= 0 {
        let scale = pow10(-decimals);
        trim((value / scale).round() * scale)
    } else {
        let decimals = decimals as usize;
        format!("{value:.decimals$}")
    }
}

/// The shortest round-tripping form of a number, which is what `Display` guarantees for f64.
fn trim(value: f64) -> String {
    format!("{value}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::Verdict;

    fn case(states: Vec<(Span, AxisState)>) -> Case {
        Case {
            axes: states
                .into_iter()
                .enumerate()
                .map(|(i, (span, state))| Axis {
                    name: format!("a{i}"),
                    span,
                    state,
                })
                .collect(),
        }
    }

    const C: Span = Span::Continuous { lo: -1.0, hi: 1.0 };

    #[test]
    fn rounding_to_significant_digits_is_not_rounding_to_decimal_places() {
        assert_eq!(round_sig(1234.0, 2), 1200.0);
        assert_eq!(round_sig(0.000_123_4, 2), 0.000_12);
        assert_eq!(round_sig(-0.1234, 3), -0.123);
        // A value that is already short survives its own rounding.
        assert_eq!(round_sig(0.25, 2), 0.25);
    }

    #[test]
    fn rounding_leaves_the_special_values_alone() {
        assert_eq!(round_sig(0.0, 1), 0.0);
        assert!(round_sig(f64::NAN, 3).is_nan());
        assert!(round_sig(f64::INFINITY, 3).is_infinite());
        assert_eq!(round_sig(1.5, 0), 1.5, "zero digits means do nothing");
    }

    #[test]
    fn printing_with_digits_uses_that_many_significant_digits() {
        assert_eq!(fmt_sig(1234.0, Some(2)), "1200");
        assert_eq!(fmt_sig(0.25, Some(2)), "0.25");
        assert_eq!(fmt_sig(0.25, Some(4)), "0.2500");
        // Without a digit budget the number prints in its shortest round-tripping form.
        assert_eq!(fmt_sig(0.1 + 0.2, None), "0.30000000000000004");
    }

    #[test]
    fn a_pinned_axis_needs_only_its_own_point() {
        let c = case(vec![(
            C,
            AxisState::Pinned {
                value: 0.3,
                digits: None,
            },
        )]);
        assert_eq!(c.witnesses(), vec![vec![0.3]]);
        assert_eq!(c.dimensions(), 1);
        assert_eq!(
            c.digits(),
            17,
            "an unreduced value is as long as an f64 can be"
        );
    }

    #[test]
    fn a_dropped_axis_is_sampled_across_its_whole_domain() {
        let c = case(vec![(C, AxisState::Dropped)]);
        let w = c.witnesses();
        // Five, not six: the domain centre is both the representative and the 50% sample, and a
        // point is never executed twice for the same claim.
        assert_eq!(
            w.len(),
            5,
            "the centre plus the samples it does not already cover: {w:?}"
        );
        let seen: Vec<f64> = w.iter().map(|x| x[0]).collect();
        assert!(seen.contains(&-1.0) && seen.contains(&1.0), "{seen:?}");
        assert_eq!(c.dimensions(), 0);
        assert_eq!(c.digits(), 0);
    }

    #[test]
    fn a_band_is_checked_at_both_edges_and_between() {
        let c = case(vec![(
            Span::Continuous { lo: 0.0, hi: 10.0 },
            AxisState::Band {
                lo: 2.0,
                hi: 4.0,
                digits: None,
            },
        )]);
        let w = c.witnesses();
        let mut seen: Vec<f64> = w.iter().map(|x| x[0]).collect();
        // An assertion needs the samples ordered, and `total_cmp` is the IEEE bit-pattern order rather
        // than a partial one, so it puts negatives and zeros consistently.
        seen.sort_by(f64::total_cmp);
        // Both edges, the centre as representative, and the two quarter points in between: the
        // claim is "everywhere in this interval fails", so the interior gets sampled too.
        assert_eq!(seen, vec![2.0, 2.5, 3.0, 3.5, 4.0], "{w:?}");
    }

    #[test]
    fn a_discrete_drop_executes_every_choice() {
        let c = case(vec![(
            Span::Discrete(vec![1.0, 2.0, 3.0]),
            AxisState::Dropped,
        )]);
        // The centre of the list is the representative, so two remaining choices get added.
        assert_eq!(c.witnesses().len(), 3, "every choice, once each");
    }

    #[test]
    fn duplicate_sample_points_are_not_executed_twice() {
        // The centre of a band is its own representative, so the star must not repeat it.
        let c = case(vec![(
            C,
            AxisState::Pinned {
                value: 0.0,
                digits: None,
            },
        )]);
        assert_eq!(c.witnesses().len(), 1);
    }

    #[test]
    fn the_representative_uses_pinned_values_and_band_centres() {
        let c = case(vec![
            (
                Span::Continuous { lo: 0.0, hi: 10.0 },
                AxisState::Band {
                    lo: 4.0,
                    hi: 6.0,
                    digits: None,
                },
            ),
            (
                C,
                AxisState::Pinned {
                    value: -0.5,
                    digits: None,
                },
            ),
            (C, AxisState::Dropped),
        ]);
        assert_eq!(c.representative(), vec![5.0, -0.5, 0.0]);
    }

    #[test]
    fn a_description_names_every_parameter_and_what_is_claimed_about_it() {
        let c = case(vec![
            (
                C,
                AxisState::Pinned {
                    value: 0.4,
                    digits: Some(1),
                },
            ),
            (C, AxisState::Dropped),
            (
                Span::Continuous { lo: 0.0, hi: 1.0 },
                AxisState::Band {
                    lo: 0.2,
                    hi: 0.8,
                    digits: None,
                },
            ),
        ]);
        let text = c.describe();
        assert!(text.contains("a0 = 0.4"), "{text}");
        assert!(text.contains("a1: not part of the failure"), "{text}");
        assert!(text.contains("a2 in [0.2, 0.8]"), "{text}");
    }

    #[test]
    fn verification_stops_at_the_first_witness_that_does_not_fail() {
        use std::cell::Cell;
        let seen = Cell::new(0u64);
        let oracle = move |_x: &[f64], _path: &mut dyn aporia_runtime::Executor| {
            let n = seen.get() + 1;
            seen.set(n);
            // One query, no model execution: this predicate reads a counter, not a model, and the
            // accounting has to be able to say that rather than assume a call is an evaluation.
            Verdict::new(n < 3, 0)
        };
        let c = case(vec![(C, AxisState::Dropped)]);
        let v = c.verify(&oracle, &mut aporia_runtime::Interp, 100);
        assert!(!v.holds);
        assert!(!v.over_budget);
        assert_eq!(v.queries, 3);
        assert_eq!(v.executions, 0, "queries are not executions");
    }

    #[test]
    fn a_budget_refusal_is_not_the_same_answer_as_a_passing_case() {
        let c = case(vec![(C, AxisState::Dropped)]);
        let v = c.verify(
            &|_x: &[f64], _path: &mut dyn aporia_runtime::Executor| Verdict::new(true, 1),
            &mut aporia_runtime::Interp,
            2,
        );
        assert!(v.over_budget);
        assert_eq!(v.executions, 2, "the two calls it could afford did run");
        assert!(
            !v.holds,
            "an unaffordable check must not read as a verified case"
        );
    }

    #[test]
    fn an_oracle_that_costs_several_executions_per_answer_reports_several() {
        // The whole reason `Verify` carries two numbers: a witness set of five points asked of an
        // oracle that measures a star of three executions around each one cost fifteen runs, and a
        // reader of the cost column has to be able to tell the two apart.
        let c = case(vec![(C, AxisState::Dropped)]);
        let v = c.verify(
            &|_x: &[f64], _path: &mut dyn aporia_runtime::Executor| Verdict::new(true, 3),
            &mut aporia_runtime::Interp,
            100,
        );
        assert!(v.holds);
        assert_eq!((v.queries, v.executions), (5, 15));
    }

    #[test]
    fn a_case_is_asked_of_the_path_it_is_handed_and_no_other() {
        // The architectural half of the routing change. Nothing in this crate can reach the
        // interpreter by itself any more, so the only way a witness gets answered is through the
        // `Executor` the caller brings — which is what makes it impossible for a finding discovered
        // in an external program to be "verified" by arithmetic that program never ran. The predicate
        // here charges nothing for its answer, so any execution counted by `Verify` would have to have
        // come from the path, and the path counts what it was asked to do.
        use std::cell::RefCell;
        #[derive(Default)]
        struct Counting {
            asks: usize,
            seen: RefCell<Vec<f64>>,
        }
        impl aporia_runtime::Executor for Counting {
            fn execute(
                &mut self,
                _model: &aporia_ir::Model,
                x: &[f64],
                _cfg: aporia_runtime::ExecConfig,
            ) -> aporia_runtime::Outcome {
                self.asks += 1;
                self.seen.borrow_mut().extend_from_slice(x);
                aporia_runtime::Outcome {
                    outputs: vec![1.0],
                    traces: Vec::new(),
                    flags: aporia_runtime::Flags::default(),
                    steps: 0,
                    rule_values: Vec::new(),
                }
            }
        }
        // A predicate that does consult the path: it asks the model and calls the answer a failure
        // when the path returns the value this stand-in program always returns.
        let model = aporia_ir::Model::new("t");
        let oracle = move |x: &[f64], path: &mut dyn aporia_runtime::Executor| {
            let out = path.execute(&model, x, aporia_runtime::ExecConfig::default());
            Verdict::new(out.outputs.first() == Some(&1.0), 1)
        };
        let c = case(vec![(C, AxisState::Dropped)]);
        let asked = c.witnesses();
        let mut path = Counting::default();
        let v = c.verify(&oracle, &mut path, 100);
        assert!(v.holds, "every witness was answered by the path");
        assert_eq!(path.asks, 5, "one execution per witness");
        assert_eq!(v.executions, 5, "and each of them charged");
        assert_eq!(v.queries, 5);
        assert_eq!(
            path.seen.into_inner(),
            asked.concat(),
            "the points the path was asked are the case's own witnesses"
        );
    }
}
