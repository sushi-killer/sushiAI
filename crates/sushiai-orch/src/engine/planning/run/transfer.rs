use super::PlanOutcome;
use crate::engine::planning::draft::PlanDraft;
use crate::engine::planning::publication::PlanProposal;
use crate::engine::planning::round::PlannerRound;
use crate::policy::pipelines::planning::{PlanningFlow, PlanningOutcome, PlanningStep};

pub(super) enum PlanningOperation {
    Prepare,
    Draft(Box<PlannerRound>),
    Clarify(Box<PlannerRound>),
    Review {
        round: Box<PlannerRound>,
        proposal: Box<PlanProposal>,
    },
    Questions {
        round: Box<PlannerRound>,
        draft: Box<PlanDraft>,
    },
    Split {
        round: Box<PlannerRound>,
        draft: Box<PlanDraft>,
    },
    Command(Box<PlannerRound>),
    Finish(Box<PlannerRound>),
    Cleanup(Option<usize>),
}

pub(super) enum PlanningState {
    Running(PlanningOperation),
    Return(PlanOutcome),
}

pub(super) enum PlanningInput {
    Admission,
    Round(Box<PlannerRound>),
    Proposal {
        round: Box<PlannerRound>,
        proposal: Box<PlanProposal>,
    },
    Draft {
        round: Box<PlannerRound>,
        draft: Box<PlanDraft>,
    },
    Interrupted(Option<usize>),
    Outcome(PlanOutcome),
}

pub(super) struct PlanningTransfer {
    pub(super) outcome: PlanningOutcome,
    input: PlanningInput,
}

impl PlanningTransfer {
    pub(super) fn new(outcome: PlanningOutcome, input: PlanningInput) -> Self {
        Self { outcome, input }
    }

    pub(super) fn complete(outcome: PlanOutcome) -> Self {
        Self::new(PlanningOutcome::Complete, PlanningInput::Outcome(outcome))
    }

    pub(super) fn retry() -> Self {
        Self::new(PlanningOutcome::Retry, PlanningInput::Admission)
    }

    pub(super) fn route(
        self,
        flow: &PlanningFlow,
        from: PlanningStep,
    ) -> Result<(PlanningStep, PlanningState), (Option<usize>, String)> {
        let attempt = self.input.attempt();
        let next = flow
            .next(from, self.outcome)
            .map_err(|error| (attempt, error))?;
        self.input
            .enter(next)
            .map(|state| (next, state))
            .map_err(|error| (attempt, error))
    }
}

impl PlanningInput {
    fn attempt(&self) -> Option<usize> {
        match self {
            Self::Round(round) | Self::Proposal { round, .. } | Self::Draft { round, .. } => {
                Some(round.index)
            }
            Self::Interrupted(attempt) => *attempt,
            Self::Admission | Self::Outcome(_) => None,
        }
    }

    pub(super) fn enter(self, step: PlanningStep) -> Result<PlanningState, String> {
        let operation = match (step, self) {
            (PlanningStep::Prepare, Self::Admission) => PlanningOperation::Prepare,
            (PlanningStep::Draft, Self::Round(round)) => PlanningOperation::Draft(round),
            (PlanningStep::Clarify, Self::Round(round)) => PlanningOperation::Clarify(round),
            (PlanningStep::Review, Self::Proposal { round, proposal }) => {
                PlanningOperation::Review { round, proposal }
            }
            (PlanningStep::Questions, Self::Draft { round, draft }) => {
                PlanningOperation::Questions { round, draft }
            }
            (PlanningStep::Split, Self::Draft { round, draft }) => {
                PlanningOperation::Split { round, draft }
            }
            (PlanningStep::Command, Self::Round(round)) => PlanningOperation::Command(round),
            (PlanningStep::Finish, Self::Round(round)) => PlanningOperation::Finish(round),
            (PlanningStep::Cleanup, Self::Interrupted(attempt)) => {
                PlanningOperation::Cleanup(attempt)
            }
            (PlanningStep::Return, Self::Outcome(outcome)) => {
                return Ok(PlanningState::Return(outcome))
            }
            _ => {
                return Err(format!(
                    "planning {step:?} did not receive its required typed input"
                ))
            }
        };
        Ok(PlanningState::Running(operation))
    }
}

impl PlanningOperation {
    pub(super) fn step(&self) -> PlanningStep {
        match self {
            Self::Prepare => PlanningStep::Prepare,
            Self::Draft(_) => PlanningStep::Draft,
            Self::Clarify(_) => PlanningStep::Clarify,
            Self::Review { .. } => PlanningStep::Review,
            Self::Questions { .. } => PlanningStep::Questions,
            Self::Split { .. } => PlanningStep::Split,
            Self::Command(_) => PlanningStep::Command,
            Self::Finish(_) => PlanningStep::Finish,
            Self::Cleanup(_) => PlanningStep::Cleanup,
        }
    }
}
