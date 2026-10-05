//! Asking a point what the report would have said about it.
//!
//! The rule oracle in `aporia-minimize` answers one question — *does a declared rule fail here?* —
//! and that is the only question a counterexample can be verified against today. Findings produced by
//! the measurement channels answer to none of it: a cell flagged because one output moves twenty-five
//! times further than usual along an axis violates no rule, so ddmin has nothing to preserve and the
//! harness reports no smaller description at all. That gap is recorded in decision 0012 and in the
//! results README, and it is why four corpus entries never produce a verified minimisation.
//!
//! [`RiskScorer`] is the rest of that answer: the campaign's own evidence model, frozen — its fitted
//! calibrator, its measured channel correlation, its ordinary slopes per output and axis, its policy
//! threshold — asked about coordinates the search never sampled. It is deliberately narrower than the
//! report, and says which way: see [`RiskScorer::from_campaign`].

use aporia_evidence::{Calibrator, ChannelCorrelation, Evidence, EvidenceSet, fuse};
use aporia_ir::Model;
use aporia_minimize::{Oracle, Verdict};
use aporia_properties::{
    SlopeReference, against_reference, constraints, divergence, numerical, sensitivity_at,
    slope_reference,
};
use aporia_runtime::interp::run as evaluate;
use aporia_runtime::observe::Observation;
use aporia_runtime::value::{ExecConfig, FpMode};
use aporia_search::{Campaign, campaign::perturb};

/// One point, measured: what the report would have said about it, and what asking cost.
///
/// The executions are counted where they are spent rather than derived from the number of queries,
/// because this oracle does not run the model once per answer: it runs the point, a perturbed point
/// per axis, a reduced-precision re-run and an independent reference evaluation — whichever of those
/// the finding being minimised was actually made of. A published cost column that meant *work* had to
/// come from here.
#[derive(Clone, Debug)]
pub struct Reading {
    /// The fused risk the report would have attached to the point.
    pub risk: f64,
    /// The evidence behind that score, calibrated exactly as the campaign calibrates.
    pub items: Vec<Evidence>,
    /// Model executions performed to produce the two fields above.
    pub executions: u64,
}

/// The campaign's evidence model, frozen, asked about one point at a time.
///
/// Everything it holds was *measured by the campaign that produced the finding*, which is the whole
/// point: a minimiser proposing new coordinates must not be able to move the reference out from under
/// itself. Recomputing the ordinary slope from the candidate's own neighbourhood would make every
/// candidate typical, and a calibrator refitted on candidates would grade the reduction by a standard
/// the report never used.
#[derive(Clone, Debug)]
pub struct RiskScorer {
    calibrator: Calibrator,
    correlation: ChannelCorrelation,
    reference: SlopeReference,
    threshold: f64,
    max_steps: u64,
    /// The subset of the campaign's channels this scorer is allowed to consult. See
    /// [`RiskScorer::for_finding`].
    channels: u8,
}

/// The channel's bit in a mask, taken from the channel itself rather than from a number written down
/// next to this code: `Channel::ALL` is behavioural-first, and an assumed order here would silently
/// let the wrong channel through.
fn bit(channel: aporia_evidence::Channel) -> u8 {
    1u8 << channel.index()
}

impl RiskScorer {
    /// Freeze everything the finished campaign measured, and consult every channel it ran.
    ///
    /// **Coverage, and it is not the whole report.** The channels scored here are the four whose
    /// measurement is defined at a single point: Physical (the model's own rules and its divergence),
    /// Sensitivity (a probe slope against the campaign's frozen median), Numerical (f64 against f32 at
    /// the same coordinates) and Differential (the runtime against the independent reference).
    ///
    /// The Behavioral channel is *not* in here, because its two producers are statements about a
    /// population rather than about a point: a declared relation is judged over the whole record set
    /// (`aporia_properties::relations` fits a scaling exponent, scans monotonicity, compares a swap
    /// against a recorded execution), and the inferred-pattern sensor is advisory by decision 0014.
    /// Rebuilding either from a candidate's three-point neighbourhood would let a fit rest on one
    /// step — a *louder* claim than the report made, which is the direction that turns a verified
    /// counterexample into one the report would not have flagged. So a finding whose risk comes only
    /// from a declared relation is not minimisable by this oracle, and `metrics::counterexamples`
    /// reports the absence rather than treating it as a pass.
    #[must_use]
    pub fn from_campaign(campaign: &Campaign) -> Self {
        let mut channels = bit(aporia_evidence::Channel::Physical);
        if campaign.config.probe_every > 0 {
            channels |= bit(aporia_evidence::Channel::Sensitivity);
        }
        if campaign.config.numerical_every > 0 {
            channels |= bit(aporia_evidence::Channel::Numerical);
        }
        if campaign.config.differential_every > 0 {
            channels |= bit(aporia_evidence::Channel::Differential);
        }
        Self {
            calibrator: campaign.calibrator.clone(),
            correlation: campaign.correlation.clone(),
            reference: slope_reference(&campaign.records, &campaign.probes),
            threshold: campaign.config.policy.suspicious_mean,
            max_steps: campaign.config.max_steps_per_evaluation,
            channels,
        }
    }

    /// A scorer restricted to the channels that actually made this finding.
    ///
    /// The campaign applied its sensors sparsely — a probe star every `probe_every` rounds, a
    /// precision pair every `numerical_every` — so a candidate given *every* sensor is measured more
    /// thoroughly than any point in the atlas was. Measured on three corpus campaigns, that scorer
    /// reproduced every finding's own risk exactly (1.000 → 1.000, 0.997 → 0.997, 0.740 → 0.740) but
    /// flagged 34 records where the report flagged 4, because it was adding evidence the report never
    /// had at those points. Over-flagging is not the safe direction: ddmin would then be able to
    /// "verify" a reduction at coordinates the report would have left alone. So a finding is minimised
    /// against the channels it was made of, and nothing else.
    #[must_use]
    pub fn for_finding(campaign: &Campaign, finding: &aporia_search::Finding) -> Self {
        let mut base = Self::from_campaign(campaign);
        let mut wanted = 0u8;
        for e in &finding.evidence {
            let bit_of = bit(e.channel);
            if e.channel == aporia_evidence::Channel::Behavioral {
                // Not reproducible at a point, and see `from_campaign`: dropping it is deliberate.
                continue;
            }
            wanted |= bit_of;
        }
        base.channels &= wanted;
        base
    }

    /// Does this scorer reproduce the report's own judgement of the finding it was built from?
    ///
    /// The one check that makes the fallback honest: if the frozen model cannot flag the point the
    /// report flagged, it is not asking the report's question and must not be used to decide what a
    /// smaller counterexample may drop.
    #[must_use]
    pub fn agrees_with(&self, model: &Model, finding: &aporia_search::Finding) -> bool {
        if finding
            .evidence
            .iter()
            .all(|e| e.channel == aporia_evidence::Channel::Behavioral)
        {
            return false;
        }
        self.read(model, &finding.representative).risk >= self.threshold
    }

    #[must_use]
    fn wants(&self, channel: aporia_evidence::Channel) -> bool {
        self.channels & bit(channel) != 0
    }

    /// The report's judgement of one point that is not in its record set.
    ///
    /// Identifiers are local to the query (0 for the point itself, 1.. per probe) because the point
    /// is not one of the campaign's records; the numbers they name are the candidate's own
    /// executions, and that is what a replayable claim needs.
    #[must_use]
    pub fn read(&self, model: &Model, x: &[f64]) -> Reading {
        let mut executions = 0;
        let mut items = self.gather(model, x, &mut executions);
        // Calibrated exactly as the report calibrates: the fitted scales are the campaign's, so a
        // candidate is unusual against the same ordinary values the finding was.
        self.calibrator.apply(&mut items);
        let score = fuse(
            &EvidenceSet {
                items: items.clone(),
            },
            &self.correlation,
        )
        .score;
        Reading {
            risk: score,
            items,
            executions,
        }
    }

    /// The evidence the report would have gathered at these coordinates, before calibration, counting
    /// every execution it takes to gather it.
    fn gather(&self, model: &Model, x: &[f64], executions: &mut u64) -> Vec<Evidence> {
        if x.len() != model.params.len() {
            return Vec::new();
        }
        let cfg = ExecConfig {
            fp: FpMode::F64,
            max_steps: self.max_steps,
        };
        *executions += 1;
        let base = Observation::new(0, x.to_vec(), &evaluate(model, x, cfg));
        let mut items = Vec::new();
        if self.wants(aporia_evidence::Channel::Physical) {
            items.extend(constraints(model, &base));
            items.extend(divergence(model, &base));
        }

        // The same star the campaign builds around a base point: one axis moved by its probe step,
        // away from the nearer bound so the candidate itself never leaves the declared domain.
        if self.wants(aporia_evidence::Channel::Sensitivity) {
            let mut moved: Vec<(u16, Observation)> = Vec::new();
            for axis in 0..model.params.len() {
                let Some((point, _step)) = perturb(model, x, axis) else {
                    continue;
                };
                *executions += 1;
                let o = Observation::new(
                    1 + moved.len() as u64,
                    point.clone(),
                    &evaluate(model, &point, cfg),
                );
                moved.push((axis as u16, o));
            }
            let refs: Vec<(u16, &Observation)> = moved.iter().map(|(a, o)| (*a, o)).collect();
            items.extend(sensitivity_at(&base, &refs, &self.reference));
        }

        // Two paths that should agree, at the same coordinates: precision inside one program, and a
        // second program entirely. Both are only consulted if the campaign ran them *and* the finding
        // being minimised was made of them, because evidence the report never gathered at a point
        // cannot be what a smaller counterexample is verified against.
        if self.wants(aporia_evidence::Channel::Numerical) {
            *executions += 1;
            let reduced = Observation::new(
                0,
                x.to_vec(),
                &evaluate(
                    model,
                    x,
                    ExecConfig {
                        fp: FpMode::F32,
                        max_steps: self.max_steps,
                    },
                ),
            );
            items.extend(numerical(model, &[0], &base.y, &reduced.y));
        }
        if self.wants(aporia_evidence::Channel::Differential) {
            *executions += 1;
            let reference = aporia_numerics::reference::evaluate(model, x, self.max_steps);
            items.extend(against_reference(model, &[0], &base.y, &reference.values()));
        }
        items
    }

    /// The bar the report flagged a cell at — `Policy::suspicious_mean`, frozen with everything else.
    #[must_use]
    pub fn threshold(&self) -> f64 {
        self.threshold
    }
}

/// A [`RiskScorer`] in the shape the minimiser takes: one question per point, and the executions that
/// question spent.
///
/// This is a type rather than a closure at the call site because the cost is part of the answer. A
/// closure that knew only `violating(model, x) -> bool` could report only the call count, and the
/// published column would go on comparing a one-execution oracle with an arity-plus-three one.
#[derive(Debug)]
pub struct RiskOracle<'a> {
    model: &'a Model,
    scorer: &'a RiskScorer,
}

impl<'a> RiskOracle<'a> {
    #[must_use]
    pub fn new(model: &'a Model, scorer: &'a RiskScorer) -> Self {
        Self { model, scorer }
    }
}

impl Oracle for RiskOracle<'_> {
    fn query(&self, x: &[f64]) -> Verdict {
        let reading = self.scorer.read(self.model, x);
        Verdict::new(reading.risk >= self.scorer.threshold(), reading.executions)
    }
}
