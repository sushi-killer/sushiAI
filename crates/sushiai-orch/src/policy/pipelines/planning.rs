use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::OnceLock;

const SOURCE: &str = include_str!("../../../pipelines/planning.yaml");

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanningStep {
    Prepare,
    Draft,
    Clarify,
    Review,
    Questions,
    Split,
    Command,
    Finish,
    Cleanup,
    Return,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlanningOutcome {
    Ready,
    Retry,
    Malformed,
    Stopped,
    Complete,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanningTransition {
    from: PlanningStep,
    on: PlanningOutcome,
    to: PlanningStep,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanningFlow {
    version: u16,
    entry: PlanningStep,
    transitions: Vec<PlanningTransition>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Capability {
    Admission,
    Round,
    Proposal,
    Draft,
    Interrupted,
    Outcome,
}

impl PlanningFlow {
    pub(crate) fn compiled() -> Result<&'static Self, String> {
        static FLOW: OnceLock<Result<PlanningFlow, String>> = OnceLock::new();
        FLOW.get_or_init(|| Self::parse(SOURCE))
            .as_ref()
            .map_err(Clone::clone)
    }

    pub(crate) fn parse(source: &str) -> Result<Self, String> {
        let flow: Self = super::decode(source)?;
        flow.validate()?;
        Ok(flow)
    }

    pub(crate) fn entry(&self) -> PlanningStep {
        self.entry
    }

    pub(crate) fn next(
        &self,
        from: PlanningStep,
        on: PlanningOutcome,
    ) -> Result<PlanningStep, String> {
        self.transitions
            .iter()
            .find(|edge| edge.from == from && edge.on == on)
            .map(|edge| edge.to)
            .ok_or_else(|| format!("planning has no {from:?}/{on:?} transition"))
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.entry != PlanningStep::Prepare {
            return Err("planning requires version 1 and preparation as its entry".into());
        }
        let mut seen = BTreeSet::new();
        for edge in &self.transitions {
            let Some(output) = edge.from.output(edge.on) else {
                return Err(format!(
                    "planning has an unsupported {:?}/{:?} outcome",
                    edge.from, edge.on
                ));
            };
            if !seen.insert((edge.from, edge.on)) {
                return Err(format!("planning repeats {:?}/{:?}", edge.from, edge.on));
            }
            if output != edge.to.input() || !edge.keeps_gate_order(self.entry) {
                return Err(format!(
                    "planning {:?}/{:?} cannot enter {:?}",
                    edge.from, edge.on, edge.to
                ));
            }
        }
        for step in PlanningStep::ALL {
            for outcome in step.outcomes() {
                if !seen.contains(&(step, *outcome)) {
                    return Err(format!("planning is missing {step:?}/{outcome:?}"));
                }
            }
        }
        Ok(())
    }
}

impl PlanningTransition {
    fn keeps_gate_order(&self, entry: PlanningStep) -> bool {
        use PlanningOutcome::{Complete, Malformed, Ready, Retry, Stopped};
        match self.on {
            Ready => self
                .from
                .rank()
                .zip(self.to.rank())
                .is_some_and(|(from, to)| to == from + 1),
            Retry => self.to == entry,
            Malformed => self.to == PlanningStep::Clarify,
            Stopped => self.to == PlanningStep::Cleanup,
            Complete => self.to == PlanningStep::Return,
        }
    }
}

impl PlanningStep {
    const ALL: [Self; 10] = [
        Self::Prepare,
        Self::Draft,
        Self::Clarify,
        Self::Review,
        Self::Questions,
        Self::Split,
        Self::Command,
        Self::Finish,
        Self::Cleanup,
        Self::Return,
    ];

    fn outcomes(self) -> &'static [PlanningOutcome] {
        use PlanningOutcome::{Complete, Malformed, Ready, Retry, Stopped};
        match self {
            Self::Prepare => &[Ready, Retry, Stopped],
            Self::Draft => &[Ready, Retry, Malformed, Stopped],
            Self::Clarify => &[Retry, Complete],
            Self::Review => &[Ready, Retry, Complete],
            Self::Questions | Self::Split | Self::Command => &[Ready, Complete],
            Self::Finish | Self::Cleanup => &[Complete],
            Self::Return => &[],
        }
    }

    fn rank(self) -> Option<u8> {
        match self {
            Self::Prepare => Some(0),
            Self::Draft => Some(1),
            Self::Review => Some(2),
            Self::Questions => Some(3),
            Self::Split => Some(4),
            Self::Command => Some(5),
            Self::Finish => Some(6),
            _ => None,
        }
    }

    fn input(self) -> Capability {
        match self {
            Self::Prepare => Capability::Admission,
            Self::Draft | Self::Clarify | Self::Command | Self::Finish => Capability::Round,
            Self::Review => Capability::Proposal,
            Self::Questions | Self::Split => Capability::Draft,
            Self::Cleanup => Capability::Interrupted,
            Self::Return => Capability::Outcome,
        }
    }

    fn output(self, outcome: PlanningOutcome) -> Option<Capability> {
        if !self.outcomes().contains(&outcome) {
            return None;
        }
        match outcome {
            PlanningOutcome::Retry => Some(Capability::Admission),
            PlanningOutcome::Malformed => Some(Capability::Round),
            PlanningOutcome::Stopped => Some(Capability::Interrupted),
            PlanningOutcome::Complete => Some(Capability::Outcome),
            PlanningOutcome::Ready => match self {
                Self::Prepare | Self::Split | Self::Command => Some(Capability::Round),
                Self::Draft => Some(Capability::Proposal),
                Self::Review | Self::Questions => Some(Capability::Draft),
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod tests;
