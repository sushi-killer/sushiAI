use crate::clock::now_ms;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::decision::record_failure;
use crate::engine::planning::draft::parse_plan;
use crate::engine::planning::publication::PlanProposal;
use crate::engine::planning::retry::Retry;
use crate::engine::planning::round::PlannerRound;
use crate::engine::planning::run_brief::planner_brief;
use crate::engine::review::review_request;
use crate::engine::run_control::TaskSignals;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::harness::{RunOutcome, RunRequest};
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::{FailureKind, Usage};
use std::path::PathBuf;

pub(in crate::engine::planning) enum DraftRun {
    Parsed(Box<PlanProposal>),
    Malformed,
    OverBudget,
    Cancelled,
    Ended,
}
enum RunDecision {
    Parsed(Box<PlanProposal>),
    Retry(Retry),
    Malformed,
    Failed,
    Cancelled,
}

impl PlannerRound {
    pub(in crate::engine::planning) async fn draft(&mut self, signals: &TaskSignals) -> DraftRun {
        let mut retry = Retry::First;
        let mut draft = None;
        let mut hard_failure = false;
        let mut over_budget = false;
        for number in 0..2 {
            if signals.cancel.is_cancelled() {
                let _ = std::fs::remove_file(&self.key_path);
                return DraftRun::Cancelled;
            }
            if number > 0 {
                if self
                    .task
                    .cost_budget()
                    .is_some_and(|budget| self.task.cost_usd >= budget)
                {
                    over_budget = true;
                    break;
                }
                self.app.save_task_logged(&self.task);
            }
            match self.run_once(&retry, signals).await {
                Ok((outcome, based_on)) => match self.accept(outcome, based_on, &retry, signals) {
                    RunDecision::Parsed(parsed) => {
                        draft = Some(parsed);
                        break;
                    }
                    RunDecision::Retry(next) => {
                        retry = next;
                        continue;
                    }
                    RunDecision::Cancelled => {
                        let _ = std::fs::remove_file(&self.key_path);
                        return DraftRun::Cancelled;
                    }
                    RunDecision::Malformed => retry = Retry::Parse,
                    RunDecision::Failed => {
                        hard_failure = true;
                        break;
                    }
                },
                Err(RunError::Cancelled) => {
                    let _ = std::fs::remove_file(&self.key_path);
                    return DraftRun::Cancelled;
                }
                Err(RunError::Io(message) | RunError::NotFound(message)) => {
                    if signals.cancel.is_cancelled()
                        || self.task.status != crate::task::TaskStatus::Drafting
                    {
                        let _ = std::fs::remove_file(&self.key_path);
                        return DraftRun::Cancelled;
                    }
                    self.fail_run(FailureKind::Error, message);
                    hard_failure = true;
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(&self.key_path);
        if hard_failure {
            return DraftRun::Ended;
        }
        if over_budget {
            record_failure(
                &mut self.task,
                self.index,
                FailureKind::NoDeliverable,
                "planner did not return a parseable sushi-plan block; the budget stopped the retry"
                    .to_string(),
            );
            self.task.touch(now_ms());
            self.app.save_task_logged(&self.task);
            self.app.broadcast_task(&self.task);
            return DraftRun::OverBudget;
        }
        draft.map(DraftRun::Parsed).unwrap_or(DraftRun::Malformed)
    }

    async fn run_once(
        &mut self,
        retry: &Retry,
        signals: &TaskSignals,
    ) -> Result<(RunOutcome, CheckSnapshot), RunError> {
        let based_on = CheckSnapshot::capture(&self.task);
        let worktree = PathBuf::from(&self.task.worktree);
        let brief = planner_brief(
            &self.app,
            &self.task,
            retry.number(),
            &self.past_work,
            retry.session().is_some(),
        );
        let suffix = if retry.number() == 0 { "" } else { "-retry" };
        let _ = std::fs::write(self.run_dir.join(format!("brief{suffix}.md")), &brief);
        let events = self.run_dir.join(format!("events{suffix}.jsonl"));
        let limit = self.app.planner_timeout() * if retry.doubled() { 2 } else { 1 };
        let request = RunRequest {
            resume: retry.session(),
            ..review_request(
                &self.route,
                &worktree,
                &self.mcp_path,
                &self.settings_path,
                &[],
            )
        };
        let result = run_harness(
            &self.app,
            &self.task.id,
            self.task.attempts[self.index].n,
            RunTrack::Attempt,
            &worktree,
            &request,
            CostTag::task("plan", &self.route.id),
            &brief,
            &events,
            &signals.cancel,
            Guard::helper("planner", limit),
        )
        .await;
        if let Ok(Some(reloaded)) = self.app.store.load_task(&self.task.id) {
            self.task = reloaded;
        }
        result.map(|outcome| (outcome, based_on))
    }

    fn accept(
        &mut self,
        outcome: RunOutcome,
        based_on: CheckSnapshot,
        retry: &Retry,
        signals: &TaskSignals,
    ) -> RunDecision {
        let observed = self.app.update_task(&self.task.id, |task| {
            if outcome.timed_out {
                let limit = self.app.planner_timeout() * if retry.doubled() { 2 } else { 1 };
                task.record_decision(helper_timeout_message("planner", limit));
            }
            crate::task::attempts::record_plan_run(
                task,
                self.index,
                crate::task::attempts::PlanRunObservation {
                    cost: outcome.cost_usd,
                    estimated: outcome.cost_estimated,
                    fingerprint: outcome.fingerprint.clone(),
                    usage: Some(Usage {
                        input: outcome.usage_input,
                        output: outcome.usage_output,
                        cached: outcome.usage_cached,
                    }),
                },
            );
        });
        match observed {
            Ok(task) => self.task = task,
            Err(_) => return RunDecision::Failed,
        }
        if signals.cancel.is_cancelled() || self.task.status != crate::task::TaskStatus::Drafting {
            return RunDecision::Cancelled;
        }
        if let Some(error) = outcome.error {
            if retry.number() == 0 && !signals.cancel.is_cancelled() {
                self.task.record_decision(format!(
                    "Planner: {error}; trying once more, resumed, with twice the time"
                ));
                return RunDecision::Retry(Retry::Error(outcome.session_id.clone()));
            }
            self.fail_run(FailureKind::Error, error);
            return RunDecision::Failed;
        }
        parse_plan(&outcome.final_text.unwrap_or_default())
            .map(|draft| {
                RunDecision::Parsed(Box::new(PlanProposal {
                    based_on,
                    draft,
                    attempt: self.index,
                }))
            })
            .unwrap_or(RunDecision::Malformed)
    }

    fn fail_run(&mut self, kind: FailureKind, message: String) {
        record_failure(&mut self.task, self.index, kind, message);
        self.task.mark_failed();
        self.task.touch(now_ms());
        self.app.save_task_logged(&self.task);
        self.app.broadcast_task(&self.task);
    }
}
