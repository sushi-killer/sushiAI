use super::transfer::{PlanningInput, PlanningOperation, PlanningState, PlanningTransfer};
use super::PlanOutcome;
use crate::engine::planning::draft_run::DraftRun;
use crate::engine::planning::records::end_plan_stage;
use crate::engine::planning::round::{prepare_round, PlannerRound};
use crate::engine::questions::parking::WaitEnd;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
#[cfg(test)]
use crate::policy::pipelines::planning::PlanningStep;
use crate::policy::pipelines::planning::{PlanningFlow, PlanningOutcome};
use std::sync::Arc;

pub(super) struct PlanningSession<'a> {
    task_id: &'a str,
    choice: (String, String),
    auto_start: bool,
    clarify_rounds: u32,
    flow: &'a PlanningFlow,
    #[cfg(test)]
    trace: Vec<(PlanningStep, PlanningOutcome, PlanningStep)>,
}

impl<'a> PlanningSession<'a> {
    pub(super) fn new(
        task_id: &'a str,
        choice: (String, String),
        auto_start: bool,
        flow: &'a PlanningFlow,
    ) -> Self {
        Self {
            task_id,
            choice,
            auto_start,
            clarify_rounds: 0,
            flow,
            #[cfg(test)]
            trace: Vec::new(),
        }
    }

    pub(super) async fn drive(
        &mut self,
        app: &Arc<App>,
        signals: &TaskSignals,
        permit: &mut Option<SlotPermit>,
        mut state: PlanningState,
    ) -> PlanOutcome {
        loop {
            let operation = match state {
                PlanningState::Return(outcome) => return outcome,
                PlanningState::Running(operation) => operation,
            };
            let from = operation.step();
            let transfer = operation.execute(self, app, signals, permit).await;
            #[cfg(test)]
            let outcome = transfer.outcome;
            match transfer.route(self.flow, from) {
                Ok((next, entered)) => {
                    #[cfg(test)]
                    self.trace.push((from, outcome, next));
                    #[cfg(not(test))]
                    let _ = next;
                    state = entered;
                }
                Err((attempt, error)) => {
                    eprintln!("orch: planning pipeline {}: {error}", self.task_id);
                    return end_plan_stage(app, self.task_id, attempt).await;
                }
            }
        }
    }

    async fn prepare(
        &self,
        app: &Arc<App>,
        signals: &TaskSignals,
        permit: &mut Option<SlotPermit>,
    ) -> PlanningTransfer {
        match prepare_round(app, self.task_id, &self.choice, signals, permit).await {
            Ok(round) => PlanningTransfer::new(
                PlanningOutcome::Ready,
                PlanningInput::Round(Box::new(round)),
            ),
            Err(WaitEnd::Changed) => PlanningTransfer::retry(),
            Err(WaitEnd::Stopped) => {
                PlanningTransfer::new(PlanningOutcome::Stopped, PlanningInput::Interrupted(None))
            }
        }
    }
}

impl PlanningOperation {
    async fn execute(
        self,
        session: &mut PlanningSession<'_>,
        app: &Arc<App>,
        signals: &TaskSignals,
        permit: &mut Option<SlotPermit>,
    ) -> PlanningTransfer {
        match self {
            Self::Prepare => session.prepare(app, signals, permit).await,
            Self::Draft(round) => draft(round, signals).await,
            Self::Clarify(mut round) => match round
                .clarify(&mut session.clarify_rounds, signals, permit)
                .await
            {
                Some(outcome) => PlanningTransfer::complete(outcome),
                None => PlanningTransfer::retry(),
            },
            Self::Review {
                mut round,
                proposal,
            } => match round.review_plan(*proposal, signals).await {
                Ok(Some(draft)) => PlanningTransfer::new(
                    PlanningOutcome::Ready,
                    PlanningInput::Draft {
                        round,
                        draft: Box::new(draft),
                    },
                ),
                Ok(None) => PlanningTransfer::retry(),
                Err(outcome) => PlanningTransfer::complete(outcome),
            },
            Self::Questions { mut round, draft } => {
                match round.answer_questions(&draft, signals, permit).await {
                    Some(outcome) => PlanningTransfer::complete(outcome),
                    None => PlanningTransfer::new(
                        PlanningOutcome::Ready,
                        PlanningInput::Draft { round, draft },
                    ),
                }
            }
            Self::Split { mut round, draft } => match round.split(&draft, session.auto_start).await
            {
                Some(outcome) => PlanningTransfer::complete(outcome),
                None => PlanningTransfer::new(PlanningOutcome::Ready, PlanningInput::Round(round)),
            },
            Self::Command(mut round) => match round.verify_command(signals, permit).await {
                Some(outcome) => PlanningTransfer::complete(outcome),
                None => PlanningTransfer::new(PlanningOutcome::Ready, PlanningInput::Round(round)),
            },
            Self::Finish(round) => {
                PlanningTransfer::complete(round.finish(session.auto_start, signals))
            }
            Self::Cleanup(attempt) => {
                PlanningTransfer::complete(end_plan_stage(app, session.task_id, attempt).await)
            }
        }
    }
}

async fn draft(mut round: Box<PlannerRound>, signals: &TaskSignals) -> PlanningTransfer {
    match round.draft(signals).await {
        DraftRun::Parsed(proposal) => PlanningTransfer::new(
            PlanningOutcome::Ready,
            PlanningInput::Proposal { round, proposal },
        ),
        DraftRun::OverBudget => PlanningTransfer::retry(),
        DraftRun::Malformed => {
            PlanningTransfer::new(PlanningOutcome::Malformed, PlanningInput::Round(round))
        }
        DraftRun::Cancelled | DraftRun::Ended => PlanningTransfer::new(
            PlanningOutcome::Stopped,
            PlanningInput::Interrupted(Some(round.index)),
        ),
    }
}

#[cfg(test)]
mod tests;
