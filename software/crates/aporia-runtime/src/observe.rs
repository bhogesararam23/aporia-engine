//! The observation record: one execution, kept whole.
//!
//! An observation is the atom everything else is built from: the point that was evaluated, the
//! numbers that came back, what went wrong, and how much work it cost. Analyses are allowed to read
//! it but never to rewrite it, so a finding can always be traced back to the execution that produced
//! it — which is what makes a Trust Atlas replayable rather than merely plausible.

use crate::interp::Outcome;
use crate::value::Flags;

/// One execution of a model at one point in its parameter space.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    /// The parameter values used, in declaration order and in the declared units.
    pub x: Vec<f64>,
    /// Output values, in declaration order.
    pub y: Vec<f64>,
    /// One series per trace: the values recorded at each iteration of its loop. Empty for a model
    /// with no `watch`, and ragged across a batch because candidates loop different numbers of
    /// times.
    pub traces: Vec<Vec<f64>>,
    pub flags: Flags,
    /// Instructions executed. This is the unit compute cost is measured in.
    pub steps: u64,
    /// A stable id within an experiment, used by evidence and by the report to point back here.
    pub id: u64,
    /// The values of instruction results that a declared rule reads, paired with the node id. Empty
    /// for a model whose rules only name parameters and outputs.
    pub rule_values: Vec<(u32, f64)>,
}

impl Observation {
    #[must_use]
    pub fn new(id: u64, x: Vec<f64>, outcome: &Outcome) -> Self {
        Self {
            x,
            y: outcome.outputs.clone(),
            traces: outcome.traces.clone(),
            flags: outcome.flags,
            steps: outcome.steps,
            id,
            rule_values: outcome.rule_values.clone(),
        }
    }

    #[must_use]
    pub fn arity(&self) -> usize {
        self.x.len()
    }

    #[must_use]
    pub fn outputs(&self) -> usize {
        self.y.len()
    }

    #[must_use]
    pub fn output(&self, j: usize) -> f64 {
        self.y[j]
    }

    /// True when the execution stayed inside the real numbers everywhere.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.flags.is_clean()
    }

    /// Trace length actually recorded, which for a looped model is the iteration count and is
    /// itself a quantity worth comparing between candidates.
    #[must_use]
    pub fn trace_len(&self, t: usize) -> usize {
        self.traces.get(t).map_or(0, Vec::len)
    }
}

/// A column-oriented view over a set of observations.
///
/// The analyses ask column questions — "how does output 2 vary with parameter 0" — and pulling a
/// column out once beats indexing thousands of structs.
#[derive(Clone, Debug, Default)]
pub struct Records {
    pub items: Vec<Observation>,
}

impl Records {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, o: Observation) -> u64 {
        let id = o.id;
        self.items.push(o);
        id
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[must_use]
    pub fn column(&self, param: usize) -> Vec<f64> {
        self.items.iter().map(|o| o.x[param]).collect()
    }

    #[must_use]
    pub fn outputs(&self, output: usize) -> Vec<f64> {
        self.items.iter().map(|o| o.y[output]).collect()
    }

    /// Total instruction executions, which is what "evaluations" means in a cost comparison.
    #[must_use]
    pub fn total_steps(&self) -> u64 {
        self.items.iter().map(|o| o.steps).sum()
    }

    #[must_use]
    pub fn by_id(&self, id: u64) -> Option<&Observation> {
        self.items.iter().find(|o| o.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interp::Outcome;
    use crate::value::Flags;

    fn obs(id: u64, x: &[f64], y: &[f64], steps: u64) -> Observation {
        Observation::new(
            id,
            x.to_vec(),
            &Outcome {
                outputs: y.to_vec(),
                traces: vec![vec![1.0, 2.0]],
                flags: Flags::default(),
                steps,
                rule_values: Vec::new(),
            },
        )
    }

    #[must_use]
    fn sample() -> (Observation, Observation) {
        (
            obs(0, &[1.0, 2.0], &[10.0], 5),
            obs(1, &[3.0, 4.0], &[30.0], 7),
        )
    }

    #[test]
    fn an_observation_carries_the_point_and_the_result() {
        let (o, _) = sample();
        assert_eq!(o.arity(), 2);
        assert_eq!(o.outputs(), 1);
        assert_eq!(o.output(0), 10.0);
        assert!(o.is_clean());
        assert_eq!(o.trace_len(0), 2);
        assert_eq!(o.trace_len(3), 0);
    }

    #[test]
    fn columns_read_straight_out_of_a_set() {
        let (a, b) = sample();
        let mut r = Records::new();
        r.push(a);
        r.push(b);
        assert_eq!(r.column(0), vec![1.0, 3.0]);
        assert_eq!(r.column(1), vec![2.0, 4.0]);
        assert_eq!(r.outputs(0), vec![10.0, 30.0]);
        assert_eq!(r.total_steps(), 12);
        assert_eq!(r.by_id(1).map(|o| o.x[0]), Some(3.0));
        assert!(r.by_id(9).is_none());
    }

    #[test]
    fn a_dirty_execution_is_not_clean_and_keeps_why() {
        let mut o = obs(2, &[0.0], &[f64::NAN], 3);
        o.flags.nan = true;
        assert!(!o.is_clean());
        assert_eq!(o.flags.names(), vec!["nan"]);
    }

    #[test]
    fn an_empty_set_asks_no_questions() {
        let r = Records::new();
        assert!(r.is_empty());
        assert_eq!(r.total_steps(), 0);
        assert!(r.column(0).is_empty());
    }
}
