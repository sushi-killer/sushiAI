use super::brief::judge_brief;
use super::checkout::scratch_path;
use super::reply::{parse_ruling, Verdict};
use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::engine::attempts::report::TestClaim;
use crate::engine::protected::paths::clean_test_path;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::{AskedBy, Question, QuestionKind, Task, TaskStatus};
use std::path::PathBuf;
use std::sync::Arc;

const KEEP_TEST: &str = "Keep the test";
const REWRITE_TEST: &str = "Rewrite the test";

/// A claim batch owns one clean checkout of the planner tests for its attempt.
pub(in crate::engine) struct ClaimReview {
    pub(super) attempt_n: u32,
    pub(super) worktree: PathBuf,
    pub(super) scratch: PathBuf,
    pub(super) cancel: CancelToken,
    scratch_made: bool,
}

impl ClaimReview {
    pub(in crate::engine) fn new(task: &Task, idx: usize, cancel: &CancelToken) -> Self {
        Self {
            attempt_n: task.attempts[idx].n,
            worktree: PathBuf::from(&task.worktree),
            scratch: scratch_path(task, "claim-tests"),
            cancel: cancel.clone(),
            scratch_made: false,
        }
    }

    pub(in crate::engine) async fn run(
        mut self,
        app: &Arc<App>,
        task: &mut Task,
        claims: &[TestClaim],
        slot: &mut QuestionSlot<'_>,
    ) -> Result<(), WaitEnd> {
        let mut result = Ok(());
        for claim in claims {
            if let Err(end) = self.rule_one(app, task, claim, slot).await {
                result = Err(end);
                break;
            }
        }
        if self.scratch_made {
            let (repo, scratch) = (PathBuf::from(&task.repo), self.scratch);
            let _ = tokio::task::spawn_blocking(move || {
                crate::git::worktree::remove_worktree(&repo, &scratch)
            })
            .await;
        }
        result
    }

    async fn prepare(&mut self, task: &Task) -> bool {
        if self.scratch_made {
            return true;
        }
        let (repo, scratch, base, files) = (
            PathBuf::from(&task.repo),
            self.scratch.clone(),
            task.base_sha.clone(),
            task.planner_tests.files.clone(),
        );
        let made = tokio::task::spawn_blocking(move || {
            crate::git::worktree::remove_worktree(&repo, &scratch);
            crate::git::worktree::add_detached_worktree(&repo, &scratch, &base)
                .map_err(|e| e.to_string())?;
            crate::engine::protected::prepare::write_files(&scratch, &files).map(|_| ())
        })
        .await;
        self.scratch_made = matches!(made, Ok(Ok(())));
        self.scratch_made
    }

    async fn rule_one(
        &mut self,
        app: &Arc<App>,
        task: &mut Task,
        claim: &TestClaim,
        slot: &mut QuestionSlot<'_>,
    ) -> Result<(), WaitEnd> {
        let Ok(path) = clean_test_path(&claim.path) else {
            return Ok(());
        };
        let Some(current) = task
            .planner_tests
            .files
            .iter()
            .find(|t| t.path == path)
            .map(|t| t.content.clone())
        else {
            task.record_decision(format!("Orchestrator: the claim that {path} is wrong was not ruled on: it is not one of the planner's tests"));
            return Ok(());
        };
        if task.planner_tests.ruled.contains(&path) {
            return Ok(());
        }
        task.record_ruled_test(path.clone());
        task.touch(now_ms());
        app.save_task_logged(task);
        if !self.prepare(task).await {
            task.record_decision(format!("Orchestrator: the claim that {path} is wrong was not ruled on (no clean checkout of the tests); the test stays"));
            return Ok(());
        }
        let normalized = TestClaim {
            path,
            reason: claim.reason.clone(),
        };
        let brief = judge_brief(task, &normalized.path, &current, &normalized.reason);
        let ruling = self
            .planner_run(app, task, "test-judge", &brief)
            .await
            .and_then(|text| parse_ruling(&text).ok_or_else(|| "no verdict".to_string()));
        let (verdict, why) = match ruling {
            Ok(ruling) => ruling,
            Err(error) => {
                task.record_decision(format!("Orchestrator: the claim that {} is wrong was not ruled on ({error}); the test stays", normalized.path));
                return Ok(());
            }
        };
        let rewrite = Self::should_rewrite(app, task, &normalized, (verdict, &why), slot).await?;
        if rewrite {
            match self.rewrite_test(app, task, &normalized, &why).await {
                Ok(()) => {}
                Err(super::rewrite::RewriteError::Deferred(end)) => return Err(end),
                Err(super::rewrite::RewriteError::Failed(error)) => {
                    task.record_decision(format!(
                        "Orchestrator: rewriting {} failed ({error}); the test stays",
                        normalized.path
                    ));
                }
            }
        }
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        Ok(())
    }

    async fn should_rewrite(
        app: &Arc<App>,
        task: &mut Task,
        claim: &TestClaim,
        ruling: (Verdict, &str),
        slot: &mut QuestionSlot<'_>,
    ) -> Result<bool, WaitEnd> {
        let (verdict, why) = ruling;
        let path = &claim.path;
        match verdict {
            Verdict::Confirmed => Ok(true),
            Verdict::Rejected => {
                task.record_decision(format!(
                    "Orchestrator: the claim that {path} is wrong was rejected: {why}"
                ));
                Ok(false)
            }
            Verdict::Unclear => {
                task.ask(Question::new(format!("The agent says {path} is wrong: {}\nThe judge could not settle it: {why}", truncate_chars(&claim.reason, 400)),
                    vec![REWRITE_TEST.to_string(), KEEP_TEST.to_string()], QuestionKind::AgentQuestion, AskedBy::Implement, now_ms()), now_ms());
                task.resume_phase(TaskStatus::Waiting, now_ms());
                task.touch(now_ms());
                slot.answer(app, task)
                    .await
                    .map(|answer| answer.trim().eq_ignore_ascii_case(REWRITE_TEST))
            }
        }
    }
}
