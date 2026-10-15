use super::verdict::Judgement;
use super::{ReviewEnd, ReviewRound, Turn};
use crate::engine::answer_policy::{record_policy_answer, AutomaticAnswer};
use crate::engine::ladder;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::review::run::{ReviewMode, ReviewSession};
use crate::engine::review::types::ReviewFailure;
use crate::engine::review_findings::MAX_REVIEW_RUNS;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::policy::routing::Route;
use crate::task::attempts::record_review_cost;
use crate::task::questions::{AskedBy, Question, QuestionKind};
use crate::task::state::Task;
use std::sync::Arc;

pub(super) struct ReviewCycle {
    pub(super) route: Route,
    pub(super) base: String,
    pub(super) round: u32,
    pub(super) after_no_verdict: bool,
    pub(super) auto_rerun: bool,
    pub(super) fallback_tried: bool,
    pub(super) harness_retries: u32,
    pub(super) force_full: bool,
    pub(super) parent: CancelToken,
    pub(super) cancel: CancelToken,
}

impl ReviewCycle {
    pub(super) fn new(
        route: Route,
        base: String,
        parent: CancelToken,
        cancel: CancelToken,
    ) -> Self {
        Self {
            route,
            base,
            round: 1,
            after_no_verdict: false,
            auto_rerun: true,
            fallback_tried: false,
            harness_retries: 0,
            force_full: false,
            parent,
            cancel,
        }
    }
}

enum MissingVerdict {
    Reply(String),
    Harness(String),
}

impl ReviewRound {
    pub(super) async fn turns(
        &mut self,
        app: &Arc<App>,
        task: &mut Task,
        cycle: &mut ReviewCycle,
    ) -> Result<(), ReviewEnd> {
        loop {
            if cycle.cancel.is_cancelled() && !cycle.parent.is_cancelled() {
                break;
            }
            let session = ReviewSession::new(
                task,
                self.run.attempt_n,
                &cycle.route,
                &cycle.cancel,
                ReviewMode {
                    after_no_verdict: cycle.after_no_verdict,
                    force_full: cycle.force_full || cycle.round == MAX_REVIEW_RUNS,
                },
            );
            let observation = session.run(app, task, &self.report, &self.verified).await;
            record_review_cost(
                task,
                self.run.idx,
                observation.cost_usd,
                observation.fingerprint,
            );
            if cycle.cancel.is_cancelled() && !cycle.parent.is_cancelled() {
                break;
            }
            let turn =
                match observation.result {
                    Ok((result, recorded_as_pass, saw)) => {
                        self.accept_verdict(
                            app,
                            task,
                            cycle,
                            Judgement {
                                result,
                                recorded_as_pass,
                                saw,
                            },
                        )
                        .await?
                    }
                    Err(ReviewFailure::Cancelled) => {
                        if !cycle.parent.is_cancelled() {
                            break;
                        }
                        return Err(self.interrupt_and_join(app, task).await);
                    }
                    Err(ReviewFailure::Harness(why))
                        if cycle.harness_retries < ladder::SETUP_RETRIES
                            && !cycle.parent.is_cancelled() =>
                    {
                        cycle.harness_retries += 1;
                        task.record_decision(format!(
                        "Orchestrator: attempt {} review run failed ({why}); trying again ({}/{})",
                        self.run.attempt_n, cycle.harness_retries, ladder::SETUP_RETRIES
                    ));
                        if !ladder::sleep_unless_cancelled(
                            ladder::setup_retry_delay(cycle.harness_retries),
                            &cycle.parent,
                        )
                        .await
                        {
                            return Err(self.interrupt_and_join(app, task).await);
                        }
                        Turn::Continue
                    }
                    Err(ReviewFailure::NoVerdict(why)) => {
                        self.after_missing(app, task, cycle, MissingVerdict::Reply(why))
                    }
                    Err(ReviewFailure::Harness(why)) => {
                        self.after_missing(app, task, cycle, MissingVerdict::Harness(why))
                    }
                };
            match turn {
                Turn::Continue => continue,
                Turn::Finished => break,
            }
        }
        Ok(())
    }

    async fn interrupt_and_join(&mut self, app: &Arc<App>, task: &mut Task) -> ReviewEnd {
        interrupt_attempt(app, task, self.run.idx);
        if let Some(finals) = self.finals.take() {
            let _ = finals.finish().await;
        }
        ReviewEnd::Stopped
    }

    fn after_missing(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        cycle: &mut ReviewCycle,
        missing: MissingVerdict,
    ) -> Turn {
        let no_verdict = matches!(missing, MissingVerdict::Reply(_));
        let why = match missing {
            MissingVerdict::Reply(why) | MissingVerdict::Harness(why) => why,
        };
        let attempt_n = self.run.attempt_n;
        if no_verdict && cycle.auto_rerun && cycle.round < MAX_REVIEW_RUNS {
            cycle.auto_rerun = false;
            cycle.round += 1;
            cycle.after_no_verdict = true;
            task.record_decision(format!(
                "Orchestrator: attempt {attempt_n} review reply had no verdict; reviewing the same attempt again"
            ));
            return Turn::Continue;
        }
        if cycle.cancel.is_cancelled() && !cycle.parent.is_cancelled() {
            return Turn::Finished;
        }
        if !cycle.fallback_tried {
            cycle.fallback_tried = true;
            if let Some((route, swap)) =
                ladder::fallback_review_route(app, &self.run.settings, &cycle.route)
            {
                for line in swap.into_iter().chain([format!(
                    "Orchestrator: attempt {attempt_n} review gave no verdict ({why}); reviewing on {}", route.id
                )]) {
                    task.record_decision(line);
                }
                cycle.route = route;
                cycle.force_full = true;
                cycle.round += 1;
                cycle.after_no_verdict = no_verdict;
                return Turn::Continue;
            }
        }
        let question = Question::new(
            format!("The review gave no verdict ({why}). Commit this attempt unreviewed?"),
            vec!["approve".into()],
            QuestionKind::ReviewNoVerdict,
            AskedBy::Review,
            crate::clock::now_ms(),
        );
        record_policy_answer(
            task,
            &question,
            &AutomaticAnswer::Policy {
                text: "approve".to_string(),
                evidence: "no review route gave a verdict; the attempt is committed unreviewed"
                    .to_string(),
            },
            attempt_n,
        );
        task.record_unreviewed(true);
        let after = if task.parent.is_some() {
            "it lands on the parent, which then becomes unreviewed"
        } else {
            "it is not landed, the owner gets a pull request"
        };
        task.record_decision(format!(
            "Orchestrator: attempt {attempt_n} committed unreviewed: no review route gave a verdict; {after}"
        ));
        Turn::Finished
    }
}
