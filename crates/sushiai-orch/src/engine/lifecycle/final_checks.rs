use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::git_in;
#[cfg(test)]
use crate::engine::test_support::init_git_repo;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::{run_verify_commands, CheckRunner};
use crate::engine::verification::scoping::deferred_line;
use crate::engine::verification::scoping::scope_task_checks;
use crate::engine::verification::scoping::split_deferred;
use crate::harness::cancel::CancelToken;
#[cfg(test)]
use crate::policy::settings::SandboxMode;
use crate::policy::settings::Settings;
use crate::task::Task;
#[cfg(test)]
use crate::task::TaskStatus;
use crate::task::VerifyOutcome;
use std::path::Path;
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

pub(in crate::engine) struct FinalChecks {
    pub(in crate::engine) results: Vec<VerifyOutcome>,
    pub(in crate::engine) skipped: Vec<String>,
}

pub(in crate::engine) struct BackgroundFinal {
    pub(in crate::engine) cancel: CancelToken,
    pub(in crate::engine) review_cancel: CancelToken,
    handle: Option<tokio::task::JoinHandle<FinalChecks>>,
}

impl BackgroundFinal {
    pub(in crate::engine) fn start(
        app: &Arc<App>,
        task: &Task,
        run_dir: &Path,
        settings: &Settings,
        parent: &CancelToken,
    ) -> Self {
        let worktree = std::path::PathBuf::from(&task.worktree);
        let cancel = parent.child();
        let review_cancel = parent.child();
        let (commands, deferred) = split_deferred(app, settings, task);
        let sandbox = app.task_sandbox(&task.repo, &worktree, run_dir);
        let app = app.clone();
        let task = task.clone();
        let base = task.base_sha.clone();
        let final_cancel = cancel.clone();
        let failed = review_cancel.clone();
        let handle = tokio::spawn(async move {
            let mut finished = run_final_checks(
                CheckRunner::from_context(
                    CheckContext::new(&app, Some(&task.id)).with_gate("final checks"),
                    &worktree,
                    sandbox.as_ref(),
                    &base,
                    &final_cancel,
                ),
                &task,
                &commands,
                Some(&failed),
            )
            .await;
            finished
                .skipped
                .extend(deferred.iter().map(|c| deferred_line(c)));
            finished
        });
        Self {
            cancel,
            review_cancel,
            handle: Some(handle),
        }
    }

    pub(in crate::engine) async fn finish(mut self) -> Option<FinalChecks> {
        self.handle.take().unwrap().await.ok()
    }
}

impl Drop for BackgroundFinal {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub(in crate::engine) async fn run_final_checks(
    runner: CheckRunner<'_>,
    task: &Task,
    commands: &[String],
    cancel_review: Option<&CancelToken>,
) -> FinalChecks {
    let (commands, skipped) =
        scope_task_checks(runner.ctx.app, task, runner.cwd, runner.base, commands).await;
    let mut results = Vec::new();
    for command in &commands {
        if runner.cancel.is_cancelled() {
            break;
        }
        let checked = run_verify_commands(runner, std::slice::from_ref(command)).await;
        if checked.iter().any(|r| r.code.is_some_and(|code| code != 0)) {
            if let Some(review) = cancel_review {
                review.cancel();
            }
        }
        results.extend(checked);
    }
    FinalChecks { results, skipped }
}

#[cfg(test)]
mod final_check_tests {
    use super::*;

    fn fixture(
        commands: Vec<String>,
    ) -> (
        Arc<App>,
        tempfile::TempDir,
        tempfile::TempDir,
        Task,
        Settings,
    ) {
        let (app, home) = test_app();
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.path().to_string_lossy().into_owned();
        task.fixture_data().worktree = task.repo.clone();
        task.fixture_data().base_sha = git_in(repo.path(), &["rev-parse", "HEAD"]);
        task.fixture_data().final_verify = commands;
        let mut settings = app.settings.read().unwrap().clone();
        settings.sandbox = SandboxMode::Host;
        (app, home, repo, task, settings)
    }

    #[tokio::test]
    async fn background_final_failure_cancels_review_without_stopping_the_task() {
        let (app, _home, repo, task, settings) = fixture(vec!["echo failed; exit 7".into()]);
        let parent = CancelToken::new();
        let finals = BackgroundFinal::start(&app, &task, repo.path(), &settings, &parent);
        tokio::time::timeout(Duration::from_secs(5), finals.review_cancel.cancelled())
            .await
            .unwrap();
        assert!(!parent.is_cancelled());
        assert!(!finals.cancel.is_cancelled());
        let result = finals.finish().await.unwrap();
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].code, Some(7));
        assert!(result.results[0].tail.contains("failed"));
    }

    #[tokio::test]
    async fn background_final_cancellation_stops_the_command_before_its_done_marker() {
        let (app, _home, repo, task, settings) =
            fixture(vec!["touch STARTED; sleep 30; touch DONE".into()]);
        let parent = CancelToken::new();
        let finals = BackgroundFinal::start(&app, &task, repo.path(), &settings, &parent);
        crate::engine::test_support::wait_async("the final command to start", || {
            repo.path().join("STARTED").exists()
        })
        .await;
        finals.cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(10), finals.finish())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.results[0].code, None);
        assert!(!repo.path().join("DONE").exists());
        assert!(!parent.is_cancelled());
    }
}
