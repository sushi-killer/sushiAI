use super::Tail;
use crate::clock::now_ms;
use crate::engine::decision::record_failure;
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::questions::wake::wake_orchestrator_for_done;
use crate::engine::reports::refresh_diff_stat;
use crate::engine::state::App;
use crate::engine::verification::context::{record_check_outcomes, CheckContext};
use crate::engine::verification::runner::{run_eval_check, CheckRunner};
use crate::harness::cancel::CancelToken;
use crate::task::{EvalCheck, FailureKind, Task, TaskStatus};
use std::path::Path;
use std::sync::Arc;

impl AttemptFiles {
    pub(super) async fn commit(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        cancel: &CancelToken,
        slot: &mut QuestionSlot<'_>,
    ) -> Tail {
        let (worktree, title, id, n) = (
            self.worktree.clone(),
            task.title.clone(),
            task.id.clone(),
            self.n,
        );
        let committed = tokio::task::spawn_blocking(move || {
            crate::delivery::git::commit::commit(&worktree, &title, &id, n)
        })
        .await;
        match committed {
            Ok(Ok(())) => {
                crate::delivery::git::carry::delete_wip_ref(&self.worktree, &task.id);
                refresh_diff_stat(task).await;
                crate::task::transitions::complete(
                    task,
                    Some(self.idx),
                    crate::task::transitions::Completion::Done,
                    now_ms(),
                );
                self.eval_check(app, task, cancel).await;
            }
            Ok(Err(error)) => {
                record_failure(task, self.idx, FailureKind::Error, error.to_string());
                task.mark_failed();
            }
            Err(error) => {
                record_failure(
                    task,
                    self.idx,
                    FailureKind::Error,
                    format!("commit task panicked: {error}"),
                );
                task.mark_failed();
            }
        }
        if task.status == TaskStatus::Done {
            crate::engine::delivery::pr::pending::mark_pr_pending(app, task);
            task.touch(now_ms());
            app.write_report(task).await;
            app.release_worktree(task, "committed on its branch").await;
        }
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        wake_orchestrator_for_done(app, task);
        crate::engine::delivery::pr::pending::spawn_pr_on_done(app, task);
        slot.release();
        app.finish_task_loop(&task.id);
        Tail::Return
    }

    async fn eval_check(&self, app: &Arc<App>, task: &mut Task, cancel: &CancelToken) {
        let Some(command) = task.eval_check_cmd.clone() else {
            return;
        };
        let sha = crate::git::history::head_sha(&self.worktree).unwrap_or_default();
        let checkout = self.run_dir.join("eval-check");
        let sandbox = app.task_sandbox(&task.repo, &checkout, &self.run_dir);
        let check = run_eval_check(
            CheckRunner::from_context(
                CheckContext::new(app, Some(&task.id)),
                &checkout,
                sandbox.as_ref(),
                &sha,
                cancel,
            ),
            Path::new(&task.repo),
            &command,
        )
        .await;
        record_check_outcomes(app, task, std::slice::from_ref(&check));
        task.record_eval_check(EvalCheck {
            code: check.code,
            tail: check.tail,
        });
    }
}
