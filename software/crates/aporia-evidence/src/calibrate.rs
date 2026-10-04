//! Calibrating raw measurements into comparable strengths.
//!
//! Channels do not share a unit. A numerical disagreement is a relative distance, a physical
//! violation is a signed rule residual, a sensitivity signal is a derivative ratio. Turning each of
//! them into `1 - exp(-magnitude / scale)` with a scale fitted from the experiment's own data is
//! what makes "numerical HIGH" and "behavioral HIGH" mean the same thing in a report.
//!
//! The scale is the **median** of the channel's magnitudes over the fitted sample, not the mean:
//! magnitudes are heavy-tailed by nature, and a mean would be dragged upward by exactly the
//! outliers the channel is trying to point at, which would flatten them.
//!
//! One median per channel is not enough, because a channel measures several different claims at
//! once. Sensitivity readings for a stiff output and a slack one land in the same population, and the
//! median of that mixture is typical of neither: the stiff claim saturates at maximum strength in
//! every cell it appears in and the slack one can never reach its own typical value however far it
//! moves. So a claim that has gathered [`MIN_STRATUM`] or more of its own measurements is compared
//! against *its own* typical value, and a claim with fewer falls back to the channel's. That is the
//! difference between "this signal is large for this model, here, for this quantity" and "this
//! signal is large compared with other quantities that happen to be measured in the same run".
//!
//! This is a calibration, not a probability. A strength of 0.8 means "this is a large signal for
//! this channel in this experiment", never "there is an 80% chance the model is wrong".

use crate::channel::{Channel, Evidence, EvidenceSet};
use std::collections::HashMap;

/// How many measurements a claim needs before its own typical value is used as its reference.
///
/// Below this the sample cannot say what "typical" is, and the channel-wide scale is used instead.
/// Eight is not a statistical threshold so much as a statement that a median of fewer than eight
/// numbers is dominated by the two in the middle.
const MIN_STRATUM: usize = 8;

/// Per-channel scales fitted from observed magnitudes, plus per-claim scales where a claim has
/// enough of its own measurements to have a typical value.
#[derive(Clone, Debug, PartialEq)]
pub struct Calibrator {
    scale: [f64; crate::channel::Channel::ALL.len()],
    fitted: [bool; crate::channel::Channel::ALL.len()],
    /// Reference scale keyed by `"<channel code>|<claim key>"`. See [`Calibrator::claim_scale`].
    claims: HashMap<String, f64>,
}

impl Default for Calibrator {
    fn default() -> Self {
        // A scale of 1.0 leaves a magnitude untouched in the exponential, which is the honest
        // fallback before any data has been seen.
        Self {
            scale: [1.0; 5],
            fitted: [false; 5],
            claims: HashMap::new(),
        }
    }
}

impl Calibrator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fit scales from a set of already-produced evidence.
    ///
    /// Fitting happens after a first pass of exploration and before fusion, which is why the
    /// evidence, rather than raw magnitudes, is the input: a channel that produced nothing in this
    /// experiment keeps its default scale instead of collapsing to zero.
    #[must_use]
    pub fn fit<'a>(items: impl Iterator<Item = &'a Evidence>) -> Self {
        let mut collected: [Vec<f64>; 5] = Default::default();
        let mut by_claim: HashMap<(usize, String), Vec<f64>> = HashMap::new();
        for e in items {
            if e.magnitude.is_finite() && e.magnitude > 0.0 {
                let i = e.channel.index();
                collected[i].push(e.magnitude);
                by_claim
                    .entry((i, e.subject.key()))
                    .or_default()
                    .push(e.magnitude);
            }
        }
        let mut c = Self::new();
        for (i, values) in collected.iter().enumerate() {
            if values.is_empty() {
                continue;
            }
            // The channel's own noise floor, not a global epsilon: the typical magnitude of an
            // f32-versus-f64 comparison is round-off, and dividing by it would call every smooth
            // model suspicious wherever the rounding happens to wobble.
            let floor = Channel::ALL[i].noise_floor().max(1e-12);
            c.scale[i] = median(values).max(floor);
            c.fitted[i] = true;
        }
        for ((i, key), values) in by_claim {
            if values.len() < MIN_STRATUM {
                continue;
            }
            let floor = Channel::ALL[i].noise_floor().max(1e-12);
            c.claims.insert(
                claim_code(Channel::ALL[i], &key),
                median(&values).max(floor),
            );
        }
        c
    }

    /// Fit directly from evidence sets gathered during an experiment.
    #[must_use]
    pub fn fit_from(sets: &[EvidenceSet]) -> Self {
        Self::fit(sets.iter().flat_map(|s| s.items.iter()))
    }

    /// The reference scale for one claim of one channel, when that claim carried its own.
    #[must_use]
    pub fn claim_scale(&self, channel: Channel, key: &str) -> f64 {
        self.claims
            .get(&claim_code(channel, key))
            .copied()
            .unwrap_or_else(|| self.scale_of(channel))
    }

    #[must_use]
    pub fn has_claim_scale(&self, channel: Channel, key: &str) -> bool {
        self.claims.contains_key(&claim_code(channel, key))
    }

    /// How many claims were given their own reference scale, for the manifest.
    #[must_use]
    pub fn claim_count(&self) -> usize {
        self.claims.len()
    }

    /// How unusual a measurement is against the experiment-wide scale of its channel.
    #[must_use]
    pub fn strength(&self, channel: Channel, magnitude: f64) -> f64 {
        excess_over(self.scale[channel.index()], magnitude)
    }

    /// How unusual a measurement is for the claim it belongs to.
    #[must_use]
    pub fn strength_of(&self, evidence: &Evidence) -> f64 {
        excess_over(
            self.claim_scale(evidence.channel, &evidence.subject.key()),
            evidence.magnitude,
        )
    }

    #[must_use]
    pub fn is_fitted(&self, channel: Channel) -> bool {
        self.fitted[channel.index()]
    }

    #[must_use]
    pub fn scale_of(&self, channel: Channel) -> f64 {
        self.scale[channel.index()]
    }

    /// Attach strengths to a batch of evidence in place.
    pub fn apply(&self, items: &mut [Evidence]) {
        for e in items {
            if e.is_fixed() {
                continue;
            }
            e.strength = self.strength_of(e);
        }
    }

    /// A description that goes into the experiment manifest, so a replay can state what the numbers
    /// meant.
    #[must_use]
    pub fn describe(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for c in Channel::ALL {
            let _ = write!(
                out,
                "{}={}{}",
                c.code(),
                self.scale_of(c),
                if self.is_fitted(c) { "" } else { "(default)" }
            );
            if c != Channel::Sensitivity {
                out.push(',');
            }
        }
        // The count is part of the record, not decoration: with it a reader can tell whether a
        // quiet result came from a claim that was compared to its own typical value or from one
        // that fell back to the channel-wide scale.
        let _ = write!(out, ",K={}", self.claim_count());
        out
    }
}

fn claim_code(channel: Channel, key: &str) -> String {
    format!("{}|{key}", channel.code())
}

/// The excess form shared by the channel-wide and per-claim references.
///
/// This is *excess over typical*, not a raw saturating map, and the difference decides whether the
/// tool is usable. Every probe of a smooth model moves its output by roughly its ordinary amount,
/// and mapping that ordinary amount to 0.63 would call the whole domain of a well-behaved model
/// suspicious. Reporting only what stands out above the population is what makes a high risk mean
/// "somewhere in here is unlike elsewhere" rather than "this model contains numbers".
///
/// The consequence, stated plainly: an experiment in which everything is anomalous has nothing that
/// stands out, and this reports nothing unusual. That is the honest reading, and the fitted scale
/// goes into the manifest so a reader can see that it happened.
#[must_use]
fn excess_over(scale: f64, magnitude: f64) -> f64 {
    if !magnitude.is_finite() || magnitude <= 0.0 || !scale.is_finite() || scale <= 0.0 {
        return 0.0;
    }
    let excess = magnitude / scale - 1.0;
    if excess <= 0.0 {
        return 0.0;
    }
    1.0 - (-excess).exp()
}

/// Median of a non-empty slice, without disturbing the caller's buffer.
#[must_use]
pub fn median(values: &[f64]) -> f64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Subject;

    fn evidence(channel: Channel, magnitude: f64) -> Evidence {
        Evidence::new(
            channel,
            Subject::Constraint(0),
            magnitude,
            vec![1],
            String::new(),
        )
    }

    fn slope(output: u16, magnitude: f64) -> Evidence {
        Evidence::new(
            Channel::Sensitivity,
            Subject::LocalSlope { output, axis: 0 },
            magnitude,
            vec![1],
            String::new(),
        )
    }

    #[test]
    fn an_unfitted_channel_treats_one_as_the_typical_value() {
        let c = Calibrator::new();
        assert!(!c.is_fitted(Channel::Numerical));
        // Scale 1 is the default, so a magnitude of one is "typical" and reads as nothing unusual,
        // while twice that lands at 1 - e^-1.
        assert_eq!(c.strength(Channel::Numerical, 1.0), 0.0);
        assert!((c.strength(Channel::Numerical, 2.0) - (1.0 - (-1.0f64).exp())).abs() < 1e-12);
    }

    #[test]
    fn the_typical_measurement_of_a_channel_reads_as_zero() {
        // The property the search depends on: nothing counts as standing out unless it differs from
        // the rest of the experiment.
        let items: Vec<Evidence> = (0..21)
            .map(|i| evidence(Channel::Sensitivity, 0.1 + i as f64 * 0.1))
            .collect();
        let c = Calibrator::fit(items.iter());
        let typical = c.scale_of(Channel::Sensitivity);
        assert_eq!(c.strength(Channel::Sensitivity, typical * 0.9), 0.0);
        assert!(c.strength(Channel::Sensitivity, typical) < 1e-12);
        assert!(c.strength(Channel::Sensitivity, typical * 2.0) > 0.5);
    }

    #[test]
    fn a_fired_rule_is_not_calibrated_away() {
        // Twenty identical hard violations plus one ordinary measurement: the violations must stay
        // at full strength even though a purely relative calibration would flatten them.
        let mut items: Vec<Evidence> = (0..20)
            .map(|i| {
                Evidence::new(
                    Channel::Physical,
                    crate::channel::Subject::Constraint(0),
                    1.0,
                    vec![i],
                    String::new(),
                )
                .absolute(1.0)
            })
            .collect();
        items.push(evidence(Channel::Sensitivity, 3.0));
        let c = Calibrator::fit(items.iter());
        c.apply(&mut items);
        assert_eq!(items[0].strength, 1.0);
        assert!(items[0].is_fixed());
        assert!(!items[20].is_fixed());
    }

    #[test]
    fn a_fitted_scale_sends_the_typical_value_to_about_two_thirds() {
        let items: Vec<Evidence> = (0..21)
            .map(|i| evidence(Channel::Sensitivity, 0.1 + i as f64 * 0.1))
            .collect();
        let c = Calibrator::fit(items.iter());
        let typical = c.scale_of(Channel::Sensitivity);
        let s = c.strength(Channel::Sensitivity, typical * 2.0);
        assert!((s - 0.632).abs() < 0.01, "{s}");
        assert!(c.is_fitted(Channel::Sensitivity));
    }

    #[test]
    fn a_heavy_tail_does_not_move_the_scale() {
        // The mean of these is dominated by the outlier; the median is not, and that is the reason
        // for using the median in the first place.
        let mut items: Vec<Evidence> = (0..50)
            .map(|_| evidence(Channel::Numerical, 1e-14))
            .collect();
        items.push(evidence(Channel::Numerical, 1e6));
        let c = Calibrator::fit(items.iter());
        // The median is 1e-14, far below the numerical channel's floor, so the floor is what ends
        // up in the scale. That is the intended interaction: comparing an f32 and an f64 path of a
        // smooth model disagrees at about 1e-7 no matter what, and a channel whose typical
        // magnitude is at that level must not treat ordinary jitter as a finding.
        assert_eq!(
            c.scale_of(Channel::Numerical),
            Channel::Numerical.noise_floor()
        );
        assert_eq!(
            c.strength(Channel::Numerical, 1e-8),
            0.0,
            "round-off is not evidence"
        );
        // The outlier still saturates to nearly one, which is the wanted behaviour.
        assert!(c.strength(Channel::Numerical, 1e6) > 0.999);
    }

    #[test]
    fn a_channel_that_produced_nothing_keeps_its_default() {
        let items = [evidence(Channel::Physical, 2.0)];
        let c = Calibrator::fit(items.iter());
        assert!(c.is_fitted(Channel::Physical));
        assert!(!c.is_fitted(Channel::Differential));
        assert_eq!(c.scale_of(Channel::Differential), 1.0);
    }

    #[test]
    fn a_tiny_typical_magnitude_does_not_make_noise_look_alarming() {
        // Conservation residuals live around 1e-17. Without the floor, a residual of 2e-17 would
        // read as a maximum-strength violation.
        let items: Vec<Evidence> = (0..20)
            .map(|_| evidence(Channel::Physical, 1e-17))
            .collect();
        let c = Calibrator::fit(items.iter());
        assert!(c.scale_of(Channel::Physical) >= 1e-12, "floor not applied");
        assert!(c.strength(Channel::Physical, 2e-17) < 1e-4);
    }

    #[test]
    fn nonsense_magnitudes_calibrate_to_zero() {
        let c = Calibrator::new();
        for m in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(c.strength(Channel::Behavioral, m), 0.0, "{m}");
        }
    }

    #[test]
    fn applying_a_calibration_leaves_only_the_unusual_above_zero() {
        // Two channels, each with a typical value and an outlier: the outlier must be lifted, the
        // typical one must stay at zero, and nothing may exceed one.
        let items = vec![
            evidence(Channel::Behavioral, 0.5),
            evidence(Channel::Behavioral, 0.5),
            evidence(Channel::Behavioral, 5.0),
            evidence(Channel::Sensitivity, 2.0),
            evidence(Channel::Sensitivity, 2.0),
            evidence(Channel::Sensitivity, 20.0),
        ];
        let c = Calibrator::fit(items.iter());
        let mut items = items;
        c.apply(&mut items);
        assert!(items.iter().all(|e| (0.0..=1.0).contains(&e.strength)));
        assert_eq!(items[0].strength, 0.0);
        assert!(
            items[2].strength > 0.9,
            "the behavioural outlier stayed quiet"
        );
        assert!(
            items[5].strength > 0.9,
            "the sensitivity outlier stayed quiet"
        );
    }

    #[test]
    fn a_claim_with_enough_of_its_own_measurements_is_judged_against_them() {
        // Twelve measurements of a slack claim around 1.5 and twelve of a stiff one around 455. The
        // single median of that mixture is about 200, which leaves the slack claim unable to reach
        // its own typical value however far it moves, and the stiff one at maximum strength in every
        // cell it appears in. Neither of those is a statement about the model.
        let items: Vec<Evidence> = (0..12)
            .map(|i| slope(0, 1.0 + i as f64 * 0.1))
            .chain((0..12).map(|i| slope(1, 400.0 + i as f64 * 10.0)))
            .collect();
        let c = Calibrator::fit(items.iter());
        let stiff = Subject::LocalSlope { output: 1, axis: 0 }.key();
        assert!(c.has_claim_scale(Channel::Sensitivity, &stiff));
        assert_eq!(c.claim_scale(Channel::Sensitivity, &stiff), 455.0);
        let mut items = items;
        c.apply(&mut items);
        // Within its own claim, nothing here is out of the ordinary.
        assert!(
            items.iter().all(|e| e.strength < 0.35),
            "{:?}",
            items.iter().map(|e| e.strength).collect::<Vec<_>>()
        );
        // And a reading far above its own claim still saturates, which is the whole point.
        assert!(c.strength_of(&slope(1, 455.0 * 100.0)) > 0.99);
    }

    #[test]
    fn a_claim_with_too_few_measurements_falls_back_to_the_channel() {
        // Two numbers cannot say what "typical" is: the median of a pair is its midpoint, so a claim
        // measured twice would be calibrated to report neither of them as unusual, whatever they say.
        let items = [slope(3, 1.0), slope(3, 1000.0)];
        let c = Calibrator::fit(items.iter());
        let key = Subject::LocalSlope { output: 3, axis: 0 }.key();
        assert!(!c.has_claim_scale(Channel::Sensitivity, &key));
        assert_eq!(
            c.claim_scale(Channel::Sensitivity, &key),
            c.scale_of(Channel::Sensitivity)
        );
    }

    #[test]
    fn median_handles_even_and_odd_lengths() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
        assert_eq!(median(&[]), 0.0);
        assert_eq!(median(&[7.0]), 7.0);
    }

    #[test]
    fn the_description_records_what_the_numbers_were_scaled_by() {
        let text = Calibrator::new().describe();
        assert!(text.contains("B=1"), "{text}");
        assert!(text.contains("(default)"), "{text}");
    }

    #[test]
    fn fitting_from_evidence_sets_is_the_same_as_fitting_from_items() {
        let mut set = EvidenceSet::new();
        set.push(evidence(Channel::Numerical, 0.3));
        set.push(evidence(Channel::Numerical, 0.7));
        let a = Calibrator::fit_from(std::slice::from_ref(&set));
        let b = Calibrator::fit(set.items.iter());
        assert_eq!(
            a.scale_of(Channel::Numerical),
            b.scale_of(Channel::Numerical)
        );
    }
}
