use crate::clip::truncate_chars;
use crate::engine::persistence::contract::{ContractInput, Publication};
use crate::engine::protected::prepare::write_files;
use crate::engine::protected::snapshot::protected_hashes;
use crate::engine::questions::parking::WaitEnd;
use crate::engine::state::App;
use crate::task::PlannedTest;
use crate::task::Task;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::brief::rewrite_brief;
use super::checkout::accept_rewrite;
use super::reply::parse_rewrite;
use super::run::ClaimReview;

impl ClaimReview {
    pub(super) async fn rewrite_test(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        claim: &crate::engine::attempts::report::TestClaim,
        why: &str,
    ) -> Result<(), RewriteError> {
        let path = claim.path.as_str();
        let reason = claim.reason.as_str();
        let worktree = self.worktree.as_path();
        let cancel = &self.cancel;
        let idx = task
            .planner_tests
            .files
            .iter()
            .position(|t| t.path == path)
            .ok_or("not a planner test")?;
        let old = task.planner_tests.files[idx].content.clone();
        let input = ContractInput::capture(task);
        let brief_text = rewrite_brief(task, path, &old, reason);
        let text = self
            .planner_run(app, task, "test-rewrite", &brief_text)
            .await?;
        let new = parse_rewrite(&text).ok_or("the planner gave no usable file")?;
        let files = accept_rewrite(app, task, idx, new.clone(), cancel).await?;
        let decision = format!(
            "Orchestrator: the planner rewrote protected test {path} ({} -> {} lines) after the agent's claim ({}); judge: {why}",
            old.lines().count(), new.lines().count(), truncate_chars(reason, 200)
        );
        let proposal = TestRewrite {
            input,
            files,
            decision,
        };
        let (app, observed, cancel, worktree) = (
            app.clone(),
            task.clone(),
            cancel.clone(),
            worktree.to_path_buf(),
        );
        let receipt = tokio::task::spawn_blocking(move || {
            proposal.publish(&app, &observed, &cancel, &worktree)
        })
        .await
        .map_err(|error| {
            eprintln!("orch: protected rewrite worker: {error}");
            RewriteError::Deferred(WaitEnd::Stopped)
        })??;
        crate::task::checks::edits::replace_planner_files(task, receipt.files);
        task.record_protected_tests(receipt.hashes);
        task.record_decision(receipt.decision);
        Ok(())
    }
}

pub(super) enum RewriteError {
    Failed(String),
    Deferred(WaitEnd),
}

impl From<String> for RewriteError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}
impl From<&str> for RewriteError {
    fn from(error: &str) -> Self {
        Self::Failed(error.into())
    }
}

struct TestRewrite {
    input: ContractInput,
    files: Vec<PlannedTest>,
    decision: String,
}

struct RewriteReceipt {
    files: Vec<PlannedTest>,
    hashes: BTreeMap<String, String>,
    decision: String,
}

impl TestRewrite {
    fn publish(
        self,
        app: &App,
        observed: &Task,
        cancel: &crate::harness::cancel::CancelToken,
        worktree: &std::path::Path,
    ) -> Result<RewriteReceipt, RewriteError> {
        let published = self
            .input
            .publish(app, observed, cancel, |fresh| {
                write_files(worktree, &self.files)?;
                crate::task::checks::edits::replace_planner_files(fresh, self.files.clone());
                let hashes = protected_hashes(worktree, &fresh.base_sha, fresh);
                fresh.record_protected_tests(hashes.clone());
                fresh.record_decision(self.decision.clone());
                Ok(hashes)
            })
            .map_err(|error| {
                eprintln!(
                    "orch: protected rewrite publication {}: {error}",
                    observed.id
                );
                RewriteError::Deferred(WaitEnd::Stopped)
            })?;
        match published {
            Publication::Applied(hashes) => Ok(RewriteReceipt {
                files: self.files,
                hashes,
                decision: self.decision,
            }),
            Publication::Deferred(end) => Err(RewriteError::Deferred(end)),
        }
    }
}

#[cfg(test)]
mod tests;
