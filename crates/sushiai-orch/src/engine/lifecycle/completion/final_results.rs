use crate::engine::baseline::{judge_against_base, BaseVerdict, Comparison};
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::state::App;
use crate::engine::verification::base::answers::{
    command_named_by_task, is_kept, keep_check, keep_planned_checks, KEPT_BY_TASK_TARGET,
};
use crate::engine::verification::base::checks::{failing_on_base, note_flaky_base_checks};
use crate::harness::cancel::CancelToken;
use crate::task::{Task, VerifyOutcome};
use crate::verify::load::timed_out_under_load;
use std::sync::Arc;

pub(super) struct FinalFailures {
    pub(super) results: Vec<VerifyOutcome>,
    pub(super) ordinary: Vec<VerifyOutcome>,
    pub(super) pre_existing: Vec<VerifyOutcome>,
}

pub(super) fn final_failure_detail(failed: &VerifyOutcome) -> String {
    format!(
        "Final check {} exited {}.\n{}",
        failed.command,
        failed
            .code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "null".into()),
        failed.tail
    )
}

impl FinalFailures {
    pub(super) fn ordinary_detail(&self) -> String {
        self.ordinary
            .iter()
            .map(final_failure_detail)
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

pub(super) struct FinalCheckRun<'a> {
    pub(super) commands: &'a [String],
    pub(super) results: Vec<VerifyOutcome>,
}

impl FinalCheckRun<'_> {
    pub(super) async fn classify(
        self,
        app: &Arc<App>,
        task: &mut Task,
        files: &AttemptFiles,
        cancel: &CancelToken,
    ) -> Option<FinalFailures> {
        let (mut ordinary, on_base) = load_base_failures(
            app,
            task,
            &self.results,
            &files.run_dir.join("final-base"),
            cancel,
        )
        .await;
        if cancel.is_cancelled() {
            return None;
        }
        note_flaky_base_checks(app, task, self.commands);
        let names: Vec<String> = on_base
            .iter()
            .map(|(failed, _)| failed.command.clone())
            .collect();
        let kept = keep_planned_checks(app, task, &names);
        let mut pre_existing = Vec::new();
        let (judge_id, judge_base) = (task.id.clone(), task.base_sha.clone());
        let judge_worktree = files.worktree.clone();
        let judge_sandbox = app.task_sandbox(&task.repo, &judge_worktree, &files.run_dir);
        let judge_runner = crate::engine::verification::runner::CheckRunner::from_context(
            crate::engine::verification::context::CheckContext::new(app, Some(&judge_id))
                .with_gate("final checks"),
            &judge_worktree,
            judge_sandbox.as_ref(),
            &judge_base,
            cancel,
        );
        for (failed, base_run) in on_base {
            if !kept.contains(&failed.command)
                && !is_kept(task, &failed.command)
                && command_named_by_task(task, &failed.command)
            {
                keep_check(task, &failed.command, KEPT_BY_TASK_TARGET);
            }
            if kept.contains(&failed.command) || is_kept(task, &failed.command) {
                ordinary.push(failed);
                continue;
            }
            match judge_against_base(
                app,
                task,
                Comparison {
                    failed: &failed,
                    base: &base_run,
                },
                &files.run_dir,
                judge_runner,
            )
            .await
            {
                BaseVerdict::Waived => {}
                BaseVerdict::NewFailures(own) => ordinary.push(*own),
                BaseVerdict::Unclear => pre_existing.push(base_run),
            }
        }
        if !kept.is_empty() {
            task.touch(crate::clock::now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        Some(FinalFailures {
            results: self.results,
            ordinary,
            pre_existing,
        })
    }
}

async fn load_base_failures(
    app: &Arc<App>,
    task: &mut Task,
    results: &[VerifyOutcome],
    dir: &std::path::Path,
    cancel: &CancelToken,
) -> (Vec<VerifyOutcome>, Vec<(VerifyOutcome, VerifyOutcome)>) {
    let mut ordinary = Vec::new();
    let mut on_base = Vec::new();
    for failed in results.iter().filter(|result| result.code != Some(0)) {
        if timed_out_under_load(failed) {
            ordinary.push(failed.clone());
            continue;
        }
        match failing_on_base(app, task, failed, dir, cancel).await {
            Some(base_run) => on_base.push((failed.clone(), base_run)),
            None => ordinary.push(failed.clone()),
        }
    }
    (ordinary, on_base)
}
