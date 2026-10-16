use super::{CarryFailure, CarryTree};
use crate::engine::baseline::{judge_check, BaseVerdict};
use crate::engine::graph::landing::Landing;
use crate::engine::state::App;
use crate::engine::verification::base::answers::{
    command_named_by_task, is_kept, keep_planned_checks,
};
use crate::engine::verification::base::checks::note_flaky_base_checks;
use crate::engine::verification::cache::run_verify_cached;
use crate::engine::verification::context::{
    record_check_decisions, record_check_outcomes, CheckContext,
};
use crate::engine::verification::runner::{run_verify_commands, CheckRunner};
use crate::engine::verification::scoping::scope_task_checks;
use crate::task::{FailureKind, Task, VerifyOutcome};
use std::sync::Arc;

impl CarryTree<'_> {
    pub(super) async fn verify(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        commands: &[String],
        finals: bool,
    ) -> Result<(), CarryFailure> {
        let (commands, skipped) =
            scope_task_checks(app, task, self.worktree, &task.base_sha, commands).await;
        record_check_decisions(task, skipped);
        let (id, base) = (task.id.clone(), task.base_sha.clone());
        let sandbox = app.task_sandbox(&task.repo, self.worktree, self.run_dir);
        let runner = CheckRunner::from_context(
            CheckContext::new(app, Some(&id)).with_gate(if finals {
                "final checks"
            } else {
                "verify"
            }),
            self.worktree,
            sandbox.as_ref(),
            &base,
            self.cancel,
        );
        let results = if finals {
            run_verify_commands(runner, &commands).await
        } else {
            let (results, lines) = run_verify_cached(runner, &id, &commands).await;
            record_check_decisions(task, lines);
            results
        };
        record_check_outcomes(app, task, &results);
        task.record_delivery_checks(results.clone());
        if let Some(index) = self.index {
            let observation = if finals {
                crate::task::attempts::VerifyObservation::Appended(results.clone())
            } else {
                crate::task::attempts::VerifyObservation::Replaced(results.clone())
            };
            crate::task::attempts::record_verify(task, index, observation);
        }
        if self.cancel.is_cancelled() {
            return Err(Landing::Cancelled.into());
        }
        let mut failed = results.into_iter().filter(|result| result.code != Some(0));
        if !finals {
            return failed
                .next()
                .map_or(Ok(()), |failed| Err(CarryFailure::Check(Box::new(failed))));
        }
        let failed: Vec<_> = failed.collect();
        let names: Vec<_> = failed.iter().map(|failed| failed.command.clone()).collect();
        let kept = keep_planned_checks(app, task, &names);
        for failed in failed {
            if kept.contains(&failed.command)
                || is_kept(task, &failed.command)
                || command_named_by_task(task, &failed.command)
            {
                return Err(CarryFailure::Check(Box::new(failed)));
            }
            let verdict = judge_check(app, task, &failed, self.run_dir, runner).await;
            if self.cancel.is_cancelled() {
                return Err(Landing::Cancelled.into());
            }
            match verdict {
                BaseVerdict::Waived => {}
                BaseVerdict::NewFailures(failed) => return Err(CarryFailure::Check(failed)),
                BaseVerdict::Unclear => return Err(CarryFailure::Check(Box::new(failed))),
            }
        }
        note_flaky_base_checks(app, task, &commands);
        Ok(())
    }
}

pub(super) fn check_failure(failed: &VerifyOutcome) -> Landing {
    let names = if failed.failing.is_empty() {
        String::new()
    } else {
        format!("\nFailing tests: {}", failed.failing.join(", "))
    };
    Landing::Failed {
        kind: FailureKind::Verify,
        detail: format!(
            "After carrying onto the new base, check {} exited {}.{names}\n{}",
            failed.command,
            failed
                .code
                .map_or_else(|| "null".into(), |code| code.to_string()),
            failed.tail
        ),
    }
}
