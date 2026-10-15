use super::{ReviewEnd, ReviewRound};
use crate::engine::decision::implement_attempt_count;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::state::App;
use crate::engine::verification::healing::evidence::findings_old_on_base;
use crate::harness::cancel::CancelToken;
use crate::task::attempts::{record_review, record_review_view, ReviewState};
use crate::task::checks::results::refresh_criteria_results;
use crate::task::review::Verdict;
use crate::task::state::Task;
use crate::verify::healing::scope::scope_review;
use std::sync::Arc;

impl ReviewRound {
    pub(super) async fn finish(
        mut self,
        app: &Arc<App>,
        task: &mut Task,
        cancel: &CancelToken,
    ) -> ReviewEnd {
        let mut final_failed = self
            .finals
            .as_ref()
            .is_some_and(|f| f.review_cancel.is_cancelled())
            && !cancel.is_cancelled();
        if final_failed {
            self.verdict = None;
        }
        let follow_ups = self.follow_ups(app, task, cancel).await;
        let finished = match self.finals.take() {
            Some(finals)
                if !final_failed
                    && !finals.review_cancel.is_cancelled()
                    && self
                        .verdict
                        .as_ref()
                        .is_some_and(|r| r.verdict == Verdict::Fail) =>
            {
                // Only the scoped and healed verdict can stop the final checks.
                finals.cancel.cancel();
                let _ = finals.finish().await;
                task.record_decision(format!(
                    "Orchestrator: attempt {} review failed; cancelled the final checks",
                    self.run.attempt_n
                ));
                None
            }
            Some(finals) => finals.finish().await,
            None => None,
        };
        if cancel.is_cancelled() {
            interrupt_attempt(app, task, self.run.idx);
            return ReviewEnd::Stopped;
        }
        final_failed |= finished.as_ref().is_some_and(|f| {
            f.results
                .iter()
                .any(|r| r.code.is_some_and(|code| code != 0))
        });
        if final_failed {
            task.record_decision(format!(
                "Orchestrator: attempt {} review discarded because a final check failed",
                self.run.attempt_n
            ));
            self.verdict = None;
            record_review_view(task, self.run.idx, None);
        } else {
            task.add_review_follow_ups(follow_ups);
        }
        record_review(
            task,
            self.run.idx,
            self.verdict
                .clone()
                .map(ReviewState::Recorded)
                .unwrap_or(ReviewState::Discarded),
        );
        refresh_criteria_results(task, self.run.idx);
        match self.verdict {
            Some(result) if result.verdict == Verdict::Fail => {
                ReviewEnd::Failed(result.findings.join("; "))
            }
            _ => ReviewEnd::Finished(finished),
        }
    }

    async fn follow_ups(
        &mut self,
        app: &Arc<App>,
        task: &mut Task,
        cancel: &CancelToken,
    ) -> Vec<String> {
        let Some(result) = self.verdict.as_mut() else {
            return Vec::new();
        };
        if result.verdict == Verdict::Fail && implement_attempt_count(task) >= 2 {
            let was = result.findings.len();
            let old_on_base =
                findings_old_on_base(app, task, result, &self.run.run_dir, cancel).await;
            let follow_ups = scope_review(result, &old_on_base);
            if !follow_ups.is_empty() {
                task.record_decision(if result.verdict == Verdict::Pass {
                    format!(
                        "Orchestrator: review FAIL of attempt {} recorded as PASS: its {was} finding(s) are P2/P3 or unrelated to this task, kept as follow-ups",
                        self.run.attempt_n
                    )
                } else {
                    format!(
                        "Orchestrator: attempt {}'s review: {} finding(s) are P2/P3 or unrelated to this task and are follow-ups, not blockers",
                        self.run.attempt_n,
                        follow_ups.len()
                    )
                });
            }
            follow_ups
        } else if result.verdict == Verdict::Pass {
            result.findings.clone()
        } else {
            Vec::new()
        }
    }
}
