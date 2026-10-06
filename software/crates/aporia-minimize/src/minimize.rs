//! Shrinking a failure to the part that causes it.
//!
//! Three reductions run in a fixed order, and the order is the substance:
//!
//! 1. [`ddmin`] removes parameters. It runs first because it changes the shape of everything after
//!    it — a parameter that turns out to be irrelevant should not be narrowed or rounded, and every
//!    narrowing decision is cheaper with fewer axes standing.
//! 2. [`narrow`] replaces a pinned value with the widest interval around it that still fails.
//!    Second, because it needs the surviving parameters pinned to something.
//! 3. [`reduce_digits`] shortens the printed numbers last, because both earlier steps are stated in
//!    terms of exact values and rounding them first would move the boundaries being searched for.
//!
//! A single-axis drop check runs between 2 and 3: narrowing can widen a claim enough that a
//! parameter previously needed becomes unnecessary, and re-running full ddmin for that would cost
//! more than the handful of checks it saves.
//!
//! Every reduction is accepted only after the resulting case is re-verified against the oracle, so
//! the output is a description that still fails, or nothing at all. When the budget runs out the
//! result says so instead of reporting an unverified case as a minimal one.
//!
//! The oracle is asked through an [`aporia_runtime::Executor`] that the run threads through every
//! stage, and it is the caller's: the same execution path that produced the finding is the only one
//! allowed to say whether a smaller case still fails it. A minimiser that reached for the interpreter
//! by itself would be able to shrink a failure found in someone else's program by asking arithmetic
//! that program never performed.

use aporia_ir::Model;
use aporia_runtime::Executor;

use crate::case::{Axis, AxisState, Case, Span, round_sig};
use crate::oracle::Oracle;

/// How far minimisation is allowed to go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Oracle *calls* for the whole run, not model executions: one call may cost more than one
    /// execution, and which is recorded separately in [`Minimal::executions`] rather than being
    /// decided by the caller's arithmetic. Verification is the cost, not the search: one dropped
    /// parameter costs a sample per edge and quartile of its domain.
    pub budget: u64,
    /// Bisection stops when an interval edge is within this fraction of the parameter's own width.
    pub shrink_tol: f64,
    /// Fewest significant digits to try. One is usually too coarse for a scientific quantity and
    /// reads as sloppy, so the default is two.
    pub digit_floor: u8,
    /// Most significant digits to try, which is the number of decimal digits that round-trip an
    /// f64; above it there is nothing left to shorten.
    pub digit_ceiling: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            budget: 20_000,
            shrink_tol: 1e-6,
            digit_floor: 2,
            digit_ceiling: 17,
        }
    }
}

/// The result of minimisation, including what it cost in both units.
#[derive(Clone, Debug)]
pub struct Minimal {
    /// The failing point minimisation started from.
    pub started: Vec<f64>,
    pub case: Case,
    /// Oracle calls actually made, which is what the budget is spent from.
    pub queries: u64,
    /// Model executions those calls performed. A published cost that means *work* is built from
    /// this, not from `queries`; for an oracle that runs the model once per point the two agree.
    pub executions: u64,
    /// Names of the parameters that turned out not to matter.
    pub dropped: Vec<String>,
    /// True when the final case was verified within the budget.
    pub verified: bool,
    /// True when the budget ended before the work did. A truncated minimisation is a real result
    /// and has to be labelled, not reported as if it were complete.
    pub over_budget: bool,
}

/// Bookkeeping shared by the stages, so cost is attributed to the run rather than to a stage.
#[derive(Clone, Copy, Debug, Default)]
struct Cost {
    queries: u64,
    executions: u64,
    over_budget: bool,
}

impl Cost {
    fn verify(
        &mut self,
        oracle: &dyn Oracle,
        engine: &mut dyn Executor,
        case: &Case,
        budget: u64,
    ) -> bool {
        if self.queries >= budget {
            self.over_budget = true;
            return false;
        }
        let v = case.verify(oracle, engine, budget - self.queries);
        self.queries += v.queries;
        self.executions += v.executions;
        if v.over_budget {
            self.over_budget = true;
            return false;
        }
        v.holds
    }

    fn spent(&self, budget: u64) -> bool {
        self.queries >= budget
    }
}

/// Minimise the failure at `x0` down to a case.
///
/// `engine` is the execution path the finding came from, and every one of the hundreds of checks below
/// is asked of it. It is a separate argument from the oracle rather than a field of it because the two
/// have different contracts: the oracle must stay a function of the point alone, and an execution may
/// be a live process with state.
#[must_use]
pub fn minimize(
    oracle: &dyn Oracle,
    engine: &mut dyn Executor,
    model: &Model,
    x0: &[f64],
    cfg: Config,
) -> Minimal {
    let mut cost = Cost::default();
    let start = x0.to_vec();
    let mut case = Case::from_model(model, x0);

    if !cost.verify(oracle, engine, &case, cfg.budget) {
        // Nothing to minimise: either the point is not a failure under this oracle, or the very
        // first verification could not be afforded. Reporting a "minimal case" here would be
        // inventing a finding.
        return Minimal {
            started: start,
            case,
            queries: cost.queries,
            executions: cost.executions,
            dropped: Vec::new(),
            verified: false,
            over_budget: cost.over_budget,
        };
    }

    ddmin(oracle, engine, &mut case, &cfg, &mut cost);
    narrow(oracle, engine, &mut case, &cfg, &mut cost);
    drop_singles(oracle, engine, &mut case, &cfg, &mut cost);
    reduce_digits(oracle, engine, &mut case, &cfg, &mut cost);

    let verified = cost.verify(oracle, engine, &case, cfg.budget);
    let dropped = case
        .axes
        .iter()
        .filter(|a| matches!(a.state, AxisState::Dropped))
        .map(|a| a.name.clone())
        .collect();

    Minimal {
        started: start,
        case,
        queries: cost.queries,
        executions: cost.executions,
        dropped,
        verified,
        over_budget: cost.over_budget,
    }
}

/// Remove parameters: delta-computation minimisation over the parameter index set.
///
/// The ddmin assumption is that a subset of a failing set still fails once the failing set is
/// minimal. Here the "set" is the parameters the case names, and removing one means sampling its
/// whole declared domain instead: [`Case::witnesses`] defines exactly what was executed, and a drop
/// is only accepted when every one of those samples failed too.
fn ddmin(
    oracle: &dyn Oracle,
    engine: &mut dyn Executor,
    case: &mut Case,
    cfg: &Config,
    cost: &mut Cost,
) {
    // The interesting corner: a failure that needs no parameter at all. Checked first because it
    // terminates the whole algorithm, and because it is a finding in its own right.
    let free = Case {
        axes: case
            .axes
            .iter()
            .map(|a| Axis {
                name: a.name.clone(),
                span: a.span.clone(),
                state: AxisState::Dropped,
            })
            .collect(),
    };
    if cost.verify(oracle, engine, &free, cfg.budget) {
        *case = free;
        return;
    }

    let mut keep: Vec<usize> = (0..case.axes.len())
        .filter(|i| !matches!(case.axes[*i].state, AxisState::Dropped))
        .collect();
    let mut n = 2;
    while !cost.spent(cfg.budget) && !keep.is_empty() {
        let mut progressed = false;
        for chunk in partitions(&keep, n) {
            let candidate = without(case, &chunk);
            if cost.verify(oracle, engine, &candidate, cfg.budget) {
                *case = candidate;
                keep.retain(|i| !chunk.contains(i));
                n = 2;
                progressed = true;
                break;
            }
        }
        if !progressed {
            if n >= keep.len() {
                break;
            }
            n = (n * 2).min(keep.len());
        }
    }
}

/// Consecutive chunks of `v`, `n` of them, as ddmin specifies.
fn partitions(v: &[usize], n: usize) -> Vec<Vec<usize>> {
    let size = v.len().div_ceil(n.max(1)).max(1);
    v.chunks(size).map(<[usize]>::to_vec).collect()
}

/// The case with these axes dropped.
fn without(case: &Case, drop: &[usize]) -> Case {
    let mut next = case.clone();
    for i in drop {
        next.axes[*i].state = AxisState::Dropped;
    }
    next
}

/// One stage's worth of context for the interval search, kept together because the alternative is a
/// ten-argument function.
struct Narrow<'a> {
    oracle: &'a dyn Oracle,
    engine: &'a mut dyn Executor,
    cfg: &'a Config,
    cost: &'a mut Cost,
}

impl Narrow<'_> {
    /// Walk from the pinned value toward `edge` for the furthest point that still verifies.
    fn edge(&mut self, case: &Case, axis: usize, value: f64, edge: f64, tol: f64) -> f64 {
        let mut good = value;
        let mut bad = edge;
        while (bad - good).abs() > tol && !self.cost.spent(self.cfg.budget) {
            let mid = good + (bad - good) / 2.0;
            let candidate = if edge < value {
                case.with(
                    axis,
                    AxisState::Band {
                        lo: mid,
                        hi: value,
                        digits: None,
                    },
                )
            } else {
                case.with(
                    axis,
                    AxisState::Band {
                        lo: value,
                        hi: mid,
                        digits: None,
                    },
                )
            };
            if self
                .cost
                .verify(self.oracle, self.engine, &candidate, self.cfg.budget)
            {
                good = mid;
            } else {
                bad = mid;
            }
        }
        good
    }
}

/// Replace each pinned value with the widest interval around it that still fails everywhere sampled.
///
/// The search bisects each side of the value independently and then verifies the combination,
/// falling back to a one-sided interval when the two edges only work apart. It finds *a* boundary,
/// not *the* boundary: the predicate is only assumed monotone between the value and the edge, and a
/// failure region with a hole in it is reported as the interval the samples support. That is stated
/// rather than hidden, because the alternative is a claim about a shape nobody evaluated.
fn narrow(
    oracle: &dyn Oracle,
    engine: &mut dyn Executor,
    case: &mut Case,
    cfg: &Config,
    cost: &mut Cost,
) {
    let arity = case.axes.len();
    for i in 0..arity {
        if cost.spent(cfg.budget) {
            return;
        }
        let (Span::Continuous { lo, hi }, AxisState::Pinned { value, .. }) =
            (&case.axes[i].span, &case.axes[i].state)
        else {
            continue;
        };
        let (lo_edge, hi_edge, v) = (*lo, *hi, *value);
        let width = (hi_edge - lo_edge).max(1e-30);
        let tol = cfg.shrink_tol.max(f64::EPSILON) * width;

        let left_all = case.with(
            i,
            AxisState::Band {
                lo: lo_edge,
                hi: v,
                digits: None,
            },
        );
        let left = if cost.verify(oracle, engine, &left_all, cfg.budget) {
            lo_edge
        } else {
            Narrow {
                oracle,
                engine,
                cfg,
                cost,
            }
            .edge(&left_all, i, v, lo_edge, tol)
        };

        let right_all = case.with(
            i,
            AxisState::Band {
                lo: v,
                hi: hi_edge,
                digits: None,
            },
        );
        let right = if cost.verify(oracle, engine, &right_all, cfg.budget) {
            hi_edge
        } else {
            Narrow {
                oracle,
                engine,
                cfg,
                cost,
            }
            .edge(&right_all, i, v, hi_edge, tol)
        };

        let both = case.with(
            i,
            AxisState::Band {
                lo: left,
                hi: right,
                digits: None,
            },
        );
        let accepted = if cost.verify(oracle, engine, &both, cfg.budget) {
            both
        } else {
            let only_left = case.with(
                i,
                AxisState::Band {
                    lo: left,
                    hi: v,
                    digits: None,
                },
            );
            if cost.verify(oracle, engine, &only_left, cfg.budget) {
                only_left
            } else {
                let only_right = case.with(
                    i,
                    AxisState::Band {
                        lo: v,
                        hi: right,
                        digits: None,
                    },
                );
                if cost.verify(oracle, engine, &only_right, cfg.budget) {
                    only_right
                } else {
                    // The pinned value stands: no wider claim survived verification. That is the
                    // honest outcome, not a failure of the algorithm.
                    continue;
                }
            }
        };
        *case = normalise(accepted, i);
    }
}

/// A band that has collapsed to a point is a pinned value, and reporting `x in [0.4, 0.4]` when
/// `x = 0.4` is true makes the description longer without saying more.
fn normalise(mut case: Case, axis: usize) -> Case {
    if let AxisState::Band { lo, hi, .. } = case.axes[axis].state
        && lo == hi
    {
        case.axes[axis].state = AxisState::Pinned {
            value: lo,
            digits: None,
        };
    }
    case
}

/// Re-check every remaining axis for irrelevance, now that the others are intervals.
fn drop_singles(
    oracle: &dyn Oracle,
    engine: &mut dyn Executor,
    case: &mut Case,
    cfg: &Config,
    cost: &mut Cost,
) {
    let arity = case.axes.len();
    for i in 0..arity {
        if cost.spent(cfg.budget) || matches!(case.axes[i].state, AxisState::Dropped) {
            continue;
        }
        let candidate = case.with(i, AxisState::Dropped);
        if cost.verify(oracle, engine, &candidate, cfg.budget) {
            *case = candidate;
        }
    }
}

/// Shorten the printed numbers as far as the failure tolerates.
///
/// Both a value and an interval's edges are rounded to the nearest representable number at `d`
/// digits, and the rounded case is verified before it is accepted — so a description is never
/// shorter than what was actually executed, and never claims an interval wider than the one the
/// samples support. Discrete axes are skipped: rounding `1.0` and `1.4` both to `1` would merge two
/// different solver settings and call the result a smaller failure.
fn reduce_digits(
    oracle: &dyn Oracle,
    engine: &mut dyn Executor,
    case: &mut Case,
    cfg: &Config,
    cost: &mut Cost,
) {
    let arity = case.axes.len();
    for i in 0..arity {
        if cost.spent(cfg.budget) {
            return;
        }
        if matches!(case.axes[i].span, Span::Discrete(_)) {
            continue;
        }
        let original = case.axes[i].state.clone();
        for d in cfg.digit_floor..=cfg.digit_ceiling {
            let state = match original {
                AxisState::Pinned { value, .. } => AxisState::Pinned {
                    value: round_sig(value, d),
                    digits: Some(d),
                },
                AxisState::Band { lo, hi, .. } => AxisState::Band {
                    lo: round_sig(lo, d),
                    hi: round_sig(hi, d),
                    digits: Some(d),
                },
                AxisState::Dropped => break,
            };
            let candidate = normalise(case.with(i, state), i);
            if cost.verify(oracle, engine, &candidate, cfg.budget) {
                *case = candidate;
                break;
            }
        }
    }
}
