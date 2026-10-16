use super::carry::PrCarry;
use super::conflict::loop_gone;
use super::resolution::{Handoff, ResolutionReceipt};
use crate::clock::now_ms;
use crate::engine::decision::failure;
use crate::engine::lifecycle::carry::{carry_onto_newest_base, BaseCarry};
use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::checks::admission::CheckAdmission;
use crate::task::{
    AskedBy, FailureKind, FreshBase, PrPlan, Question, QuestionKind, Task, TaskStatus,
    VerifyOutcome,
};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

/// Names are exact and ordered canonically; changing passing tests or timings
/// does not grant a second repair of the same failure.
fn cause(failed: &VerifyOutcome) -> String {
    let mut hash = Sha256::new();
    hash.update(failed.command.as_bytes());
    let mut names = failed.failing.clone();
    names.sort();
    names.dedup();
    for name in &names {
        hash.update([0]);
        hash.update(name.as_bytes());
    }
    if names.is_empty() {
        hash.update([0]);
        hash.update(
            if failed.output_key.is_empty() {
                &failed.tail
            } else {
                &failed.output_key
            }
            .as_bytes(),
        );
    }
    format!("{:x}", hash.finalize())
}

/// Where a persistent check failure was measured.
pub(in crate::engine) enum Origin {
    /// The carry's own run of the task's checks, before the pull request
    /// opens; the plan it will open with, if one is chosen.
    Carry(Option<PrPlan>),
    /// The CI of the pull request already open at this URL: the repair keeps
    /// it and pushes to its branch.
    PullRequest(String),
}

impl Origin {
    fn pr_url(&self) -> Option<&str> {
        match self {
            Self::Carry(_) => None,
            Self::PullRequest(url) => Some(url),
        }
    }
}

fn detail(failed: &VerifyOutcome) -> String {
    detail_from(failed, &Origin::Carry(None))
}

fn detail_from(failed: &VerifyOutcome, origin: &Origin) -> String {
    let tests = if failed.failing.is_empty() {
        "the check output below".into()
    } else {
        failed.failing.join(", ")
    };
    let check = match origin {
        Origin::Carry(_) => format!(
            "Post-carry check `{}` exited {}.",
            failed.command,
            failed
                .code
                .map_or_else(|| "null".into(), |code| code.to_string())
        ),
        Origin::PullRequest(url) => format!(
            "The CI check `{}` of the pull request {url} failed, and again on a rerun of its failed jobs; below is the failed job log.",
            failed.command
        ),
    };
    format!("{check} Failing tests: {tests}\n{}", failed.tail)
}

fn question(failed: &VerifyOutcome, reason: &str) -> Question {
    question_from(failed, reason, &Origin::Carry(None))
}

fn question_from(failed: &VerifyOutcome, reason: &str, origin: &Origin) -> Question {
    let mut question = Question::new(
        format!(
            "The pull request is waiting: {reason}.\n\n{}",
            detail_from(failed, origin)
        ),
        vec!["retry".into(), "stop".into()],
        QuestionKind::PrCheck,
        AskedBy::Verify,
        now_ms(),
    );
    question.for_owner = true;
    question.commands = vec![failed.command.clone()];
    question.failure_signature = Some(cause(failed));
    question.evidence = Some(failed.tail.clone());
    question
}

fn record_request(
    app: &App,
    before: &Task,
    failed: &VerifyOutcome,
    origin: &Origin,
    tip: String,
) -> Result<Option<(Task, bool)>, String> {
    let lock = app.task_lock(&before.id);
    let _guard = lock.lock().unwrap();
    let mut fresh = app
        .store
        .load_task(&before.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    if fresh.status != TaskStatus::Done
        || fresh.archived
        || fresh.queue.park.is_some()
        || fresh.pending_amendment.is_some()
        || fresh.pr_url.as_deref() != origin.pr_url()
        || fresh.landed_sha.is_some()
        || !CheckAdmission::capture(before).matches(&fresh)
    {
        return Ok(None);
    }
    let repair = fresh.schedule_pr_check_repair(
        cause(failed),
        failure(FailureKind::Verify, detail_from(failed, origin)),
    );
    fresh.record_pr_pending(true);
    if let Origin::Carry(Some(plan)) = origin {
        fresh.record_pr_plan(plan.clone());
    }
    if repair {
        fresh.record_fresh_base(FreshBase { tip: Some(tip) });
        fresh.resume_phase(TaskStatus::Queued, now_ms());
        fresh.record_decision(match origin {
            Origin::Carry(_) => format!(
                "PR: persistent post-carry check {} goes to the task's own agent for one repair step",
                failed.command
            ),
            Origin::PullRequest(url) => format!(
                "PR: CI check {} of {url} still failed after a rerun; it goes to the task's own agent for one repair step, and the fix is pushed to the same pull request",
                failed.command
            ),
        });
    } else {
        fresh.ask(
            question_from(
                failed,
                "the same check failure survived its repair step",
                origin,
            ),
            now_ms(),
        );
    }
    fresh.touch(now_ms());
    app.store
        .save_task(&fresh)
        .map_err(|error| error.to_string())?;
    app.broadcast_task(&fresh);
    Ok(Some((fresh, repair)))
}

pub(super) async fn resolve_checks(
    app: &Arc<App>,
    before: &Task,
    base: &str,
    failed: &VerifyOutcome,
    origin: Origin,
) -> Result<PrCarry, String> {
    if !loop_gone(app, &before.id).await {
        return Ok(PrCarry::Deferred);
    }
    let tip = crate::delivery::pr::command::run_cmd(
        "git",
        &["rev-parse", &format!("origin/{base}")],
        &PathBuf::from(&before.repo),
        crate::delivery::pr::command::GIT_TIMEOUT,
    )
    .await?;
    let Some((mut task, repair)) = record_request(app, before, failed, &origin, tip)? else {
        return Ok(PrCarry::Deferred);
    };
    if !repair {
        wake_orchestrator_for_waiting(app, &task);
        return Ok(PrCarry::ChecksPending);
    }
    if task.base_ref.as_deref() != Some(base) {
        return wait_on_setup(
            app,
            &task,
            failed,
            "the task's original base differs from the PR base",
            &origin,
        );
    }
    let resolution = ResolutionReceipt::capture(&task)?;
    let cancel = CancelToken::new();
    let setup = async {
        let recreated = app.ensure_worktree(&mut task).await?;
        app.ensure_worktree_setup(&task, recreated, &cancel).await?;
        let worktree = PathBuf::from(&task.worktree);
        Ok::<_, String>(carry_onto_newest_base(app, &mut task, &worktree, &cancel).await)
    }
    .await;
    match setup {
        Ok(BaseCarry::Ready | BaseCarry::Conflict(_)) => {
            complete_setup(app, &task)?;
            resume_resolution(app, &resolution)
        }
        Ok(BaseCarry::Stale) => resume_resolution(app, &resolution),
        Ok(BaseCarry::Stopped) => Ok(PrCarry::Deferred),
        Ok(BaseCarry::Failed(reason) | BaseCarry::Skipped(reason)) | Err(reason) => {
            wait_on_setup(app, &task, failed, &reason, &origin)
        }
    }
}

fn complete_setup(app: &App, observed: &Task) -> Result<(), String> {
    let expected = CheckAdmission::capture(observed);
    app.update_task(&observed.id, |fresh| {
        if fresh.status == TaskStatus::Queued
            && !fresh.archived
            && fresh.queue.park.is_none()
            && fresh.pending_amendment.is_none()
            && expected.matches(fresh)
        {
            fresh.record_worktree_removed(observed.worktree_removed);
            crate::engine::verification::context::record_check_decisions(
                fresh,
                observed.decisions.clone(),
            );
        }
    })?;
    Ok(())
}

fn wait_on_setup(
    app: &App,
    before: &Task,
    failed: &VerifyOutcome,
    reason: &str,
    origin: &Origin,
) -> Result<PrCarry, String> {
    let expected = CheckAdmission::capture(before);
    let mut changed = false;
    let task = app.update_task(&before.id, |fresh| {
        if fresh.status != TaskStatus::Queued
            || fresh.archived
            || fresh.queue.park.is_some()
            || fresh.pending_amendment.is_some()
            || !expected.matches(fresh)
        {
            changed = true;
            return;
        }
        fresh.ask(
            question_from(
                failed,
                &format!("the repair checkout could not be prepared: {reason}"),
                origin,
            ),
            now_ms(),
        );
    })?;
    if changed {
        return resume_resolution(app, &ResolutionReceipt::capture(before)?);
    }
    wake_orchestrator_for_waiting(app, &task);
    Ok(PrCarry::ChecksPending)
}

fn resume_resolution(app: &App, resolution: &ResolutionReceipt) -> Result<PrCarry, String> {
    Ok(match resolution.resume(app)? {
        Handoff::Active(_) => PrCarry::ChecksPending,
        Handoff::Deferred => PrCarry::Deferred,
    })
}

#[cfg(test)]
mod tests;

pub(in crate::engine) mod step;
