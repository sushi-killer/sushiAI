use super::rounds::ReviewCycle;
use super::{ReviewEnd, ReviewRound, Turn};
use crate::clock::now_ms;
use crate::engine::lifecycle::changes::unnamed_changes_since_full_review;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::persistence::contract::Publication;
use crate::engine::questions::parking::WaitEnd;
use crate::engine::review::types::ReviewSaw;
use crate::engine::review_findings::{
    handle_repeated_findings, review_stopped_note, MAX_REVIEW_RUNS,
};
use crate::engine::state::App;
use crate::engine::verification::healing::repeated::heal_repeated_unmet;
use crate::task::attempts::{record_review_view, ReviewView};
use crate::task::findings::{apply_declines, move_minor_findings, register_findings};
use crate::task::review::{ReviewResult, ReviewScope, Verdict};
use crate::task::state::Task;
use std::sync::Arc;

pub(super) struct Judgement {
    pub(super) result: ReviewResult,
    pub(super) recorded_as_pass: bool,
    pub(super) saw: ReviewSaw,
}

impl ReviewRound {
    pub(super) async fn accept_verdict(
        &mut self,
        app: &Arc<App>,
        task: &mut Task,
        cycle: &mut ReviewCycle,
        mut judgement: Judgement,
    ) -> Result<Turn, ReviewEnd> {
        record_review_view(
            task,
            self.run.idx,
            Some(ReviewView {
                tree: judgement.saw.tree.clone(),
                base: cycle.base.clone(),
                scope: judgement.saw.scope,
            }),
        );
        // Rulings determine which findings remain open for narrow coverage.
        let declines = task.attempts[self.run.idx].declines.clone();
        apply_declines(
            task,
            self.run.attempt_n,
            &mut judgement.result,
            &declines,
            &judgement.saw.rulings,
        );
        if self.expand_scope(task, cycle, &judgement).await {
            return Ok(Turn::Continue);
        }
        let mut result = judgement.result;
        move_minor_findings(task, self.run.attempt_n, &mut result);
        register_findings(task, self.run.attempt_n, cycle.round, &mut result);
        if judgement.recorded_as_pass {
            task.record_decision(
                "Orchestrator: review FAIL recorded as PASS (only P2/P3 findings)".to_string(),
            );
        }
        if result.verdict == Verdict::Fail {
            match handle_repeated_findings(
                app,
                task,
                self.run.attempt_n,
                &mut result,
                &cycle.parent,
            )
            .await
            {
                Ok(()) => {}
                Err(WaitEnd::Changed) => return Err(ReviewEnd::Changed),
                Err(WaitEnd::Stopped) => {
                    interrupt_attempt(app, task, self.run.idx);
                    return Err(ReviewEnd::Stopped);
                }
            }
            if cycle.parent.is_cancelled() {
                interrupt_attempt(app, task, self.run.idx);
                return Err(ReviewEnd::Stopped);
            }
            if cycle.cancel.is_cancelled() {
                return Ok(Turn::Finished);
            }
            match heal_repeated_unmet(app, task, self.run.idx, &result, &cycle.parent).await {
                Ok(Publication::Applied(true)) => {
                    task.touch(now_ms());
                    app.save_task_logged(task);
                    app.broadcast_task(task);
                    if cycle.round < MAX_REVIEW_RUNS {
                        cycle.round += 1;
                        return Ok(Turn::Continue);
                    }
                    task.record_decision(review_stopped_note(self.run.attempt_n));
                }
                Ok(Publication::Applied(false)) => {}
                Ok(Publication::Deferred(WaitEnd::Changed)) => return Err(ReviewEnd::Changed),
                Ok(Publication::Deferred(WaitEnd::Stopped)) | Err(_) => {
                    interrupt_attempt(app, task, self.run.idx);
                    return Err(ReviewEnd::Stopped);
                }
            }
        }
        self.verdict = Some(result);
        Ok(Turn::Finished)
    }

    async fn expand_scope(
        &self,
        task: &mut Task,
        cycle: &mut ReviewCycle,
        judgement: &Judgement,
    ) -> bool {
        if judgement.result.verdict != Verdict::Pass
            || judgement.saw.scope != ReviewScope::Narrow
            || cycle.force_full
        {
            return false;
        }
        let unnamed = unnamed_changes_since_full_review(
            task,
            self.run.attempt_n,
            judgement.saw.tree.as_deref(),
            &self.run.worktree,
            &cycle.base,
        )
        .await;
        if unnamed.is_empty() {
            return false;
        }
        task.record_decision(format!(
            "Orchestrator: attempt {} narrow review passed, but {} changed since the last full review and no open finding names {}; reviewing the whole change once",
            self.run.attempt_n,
            unnamed.join(", "),
            if unnamed.len() == 1 { "it" } else { "them" }
        ));
        cycle.force_full = true;
        cycle.round += 1;
        cycle.after_no_verdict = false;
        true
    }
}
