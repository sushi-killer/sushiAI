//! A red pull request: the failed jobs' logs, the base branch's latest run
//! of the same workflows, and, when no fast path settles it, the attribution
//! turn and the repair of the task's own failure.

use super::gh::{outcome, passed, Check, Gh, Run, Runs};
use super::{tell, update_watch, Mine, Note};
use crate::delivery::pr::checkout::TempWorktree;
use crate::delivery::pr::command::{run_cmd, GIT_TIMEOUT};
use crate::engine::attribution::{attribute, Attribution};
use crate::engine::baseline::Comparison;
use crate::engine::delivery::pr::repair::{resolve_checks, Origin};
use crate::engine::state::App;
use crate::engine::verification::runner::CheckRunner;
use crate::harness::cancel::CancelToken;
use crate::task::delivery::PrCiWatch;
use crate::task::{Blame, Task, VerifyOutcome};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

/// What failed on the pull request's head, and the base branch's run.
pub(super) struct Red {
    /// The failed jobs' logs as one check outcome.
    pub failed: VerifyOutcome,
    /// The failed runs of the head.
    pub runs: Vec<u64>,
    /// The failing tests, else the failing checks.
    pub names: Vec<String>,
    /// The pull request's base branch, when it is known.
    pub branch: Option<String>,
    /// The base branch's latest run of the same workflows.
    pub base: VerifyOutcome,
}

pub(super) async fn read_red<G: Gh>(gh: &G, task: &Task, head: &str, failing: &[&Check]) -> Red {
    let workflows: BTreeSet<&str> = failing.iter().map(|c| c.workflow.as_str()).collect();
    let label = failing
        .iter()
        .map(|c| match c.workflow.as_str() {
            "" => c.name.clone(),
            workflow => format!("{workflow} / {}", c.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let runs: Vec<u64> = gh
        .runs(Runs::Commit(head))
        .await
        .into_iter()
        .filter(|run| run.failed() && workflows.contains(run.workflow_name.as_str()))
        .map(|run| run.database_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let failed = outcome(&label, &logs(gh, &runs).await);
    let names = if failed.failing.is_empty() {
        failing.iter().map(|c| c.name.clone()).collect()
    } else {
        failed.failing.clone()
    };
    let branch = base_branch(task).await;
    let base = match &branch {
        Some(branch) => base_run(gh, branch, &workflows, &label).await,
        None => VerifyOutcome::default(),
    };
    Red {
        failed,
        runs,
        names,
        branch,
        base,
    }
}

async fn logs<G: Gh>(gh: &G, runs: &[u64]) -> String {
    let mut out = String::new();
    for run in runs {
        out.push_str(&gh.failed_log(*run).await);
        out.push('\n');
    }
    out
}

/// The latest completed run of each workflow on the base branch: their
/// failed logs when one failed, else a green outcome. Unknown when none ran.
async fn base_run<G: Gh>(
    gh: &G,
    branch: &str,
    workflows: &BTreeSet<&str>,
    label: &str,
) -> VerifyOutcome {
    let mut latest: Vec<Run> = Vec::new();
    for run in gh.runs(Runs::Branch(branch)).await {
        let completed = run.status == super::gh::RunStatus::Completed;
        if completed
            && workflows.contains(run.workflow_name.as_str())
            && !latest.iter().any(|r| r.workflow_name == run.workflow_name)
        {
            latest.push(run);
        }
    }
    if latest.is_empty() {
        return VerifyOutcome::default();
    }
    let failed: Vec<u64> = latest
        .iter()
        .filter(|run| run.failed())
        .map(|run| run.database_id)
        .collect();
    if failed.is_empty() {
        return passed(label);
    }
    outcome(label, &logs(gh, &failed).await)
}

async fn base_branch(task: &Task) -> Option<String> {
    let repo = PathBuf::from(&task.repo);
    let default = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || crate::git::branches::default_branch(&repo))
            .await
            .ok()?
    };
    crate::delivery::pr::branch::pr_base(task, &repo, default)
        .await
        .ok()
}

/// One attribution turn reads the red check; code acts on its typed verdict.
/// No valid verdict, `introduced`, or a check the task is meant to turn green
/// is the task's own failure.
pub(super) async fn judge(app: &Arc<App>, task: &Task, watch: &PrCiWatch, red: Red) {
    let mine = Mine::of(watch);
    let verdict = attribution(app, task, &watch.head, &red).await;
    match verdict {
        Some(Attribution {
            cause: Blame::Inherited,
            target: false,
            pre_existing,
            ..
        }) => {
            let base = red.branch.as_deref().unwrap_or_default();
            tell(
                app,
                task,
                &mine,
                Note::Base {
                    names: &pre_existing,
                    base,
                },
            );
        }
        Some(Attribution {
            cause: cause @ (Blame::Environment | Blame::Incomplete),
            target: false,
            evidence,
            ..
        }) => {
            let cause = match cause {
                Blame::Environment => "the environment's",
                _ => "an incomplete run",
            };
            let names = &red.names;
            tell(
                app,
                task,
                &mine,
                Note::Verdict {
                    names,
                    cause,
                    evidence: &evidence,
                },
            );
        }
        _ => repair(app, task, &mine, red).await,
    }
}

/// The attribution turn on a checkout of the pull request's head; its cost is
/// the task's. `None` when the checkout or the turn gave no verdict.
async fn attribution(app: &Arc<App>, task: &Task, head: &str, red: &Red) -> Option<Attribution> {
    let repo = PathBuf::from(&task.repo);
    let path =
        crate::git::worktree::worktree_path(&repo, &app.home, "", &format!("pr-ci-{}", task.id));
    let temp = TempWorktree::checkout(repo, path, head.to_string(), &task.branch)
        .await
        .ok()?;
    let cancel = CancelToken::new();
    let runner = CheckRunner::new(app, &temp.path, None, &task.base_sha, &cancel);
    let mut judged = task.clone();
    let comparison = Comparison {
        failed: &red.failed,
        base: &red.base,
    };
    let verdict = attribute(app, &mut judged, comparison, runner).await;
    let cost = judged.cost_usd - task.cost_usd;
    if cost > 0.0 {
        let _ = app.update_task(&task.id, |t| t.charge(cost));
    }
    verdict
}

/// The task's own failure: one repair step through the pull request check
/// repair, with the failed job log in its brief. A cause already repaired on
/// this pull request goes to the owner instead.
async fn repair(app: &Arc<App>, task: &Task, mine: &Mine, red: Red) {
    let cause = format!("repair:{}", red.names.join(","));
    let mut first = false;
    let current = update_watch(app, &task.id, mine, |t| {
        first = t.record_pr_ci_repair(&cause);
        t.finish_pr_ci_watch();
    });
    if !current {
        return;
    }
    if !first {
        return tell_repaired(app, task, &red.names);
    }
    let (Some(url), Some(base)) = (task.pr_url.clone(), red.branch.as_deref()) else {
        let line =
            "PR: the red CI check did not go to repair: the pull request's base branch is unknown";
        return app.append_decision(&task.id, line.to_string());
    };
    let repo = PathBuf::from(&task.repo);
    let fetched = run_cmd("git", &["fetch", "origin", base], &repo, GIT_TIMEOUT).await;
    let started = match (fetched, app.store.load_task(&task.id)) {
        (Ok(_), Ok(Some(fresh))) => {
            resolve_checks(app, &fresh, base, &red.failed, Origin::PullRequest(url))
                .await
                .map(|_| ())
        }
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error.to_string()),
        (_, Ok(None)) => Err("task not found".to_string()),
    };
    if let Err(error) = started {
        app.append_decision(
            &task.id,
            format!("PR: the repair of the red CI check could not start: {error}"),
        );
    }
}

/// Tells the owner once that the pull request is still red after its repair;
/// the watch is already finished.
fn tell_repaired(app: &Arc<App>, task: &Task, names: &[String]) {
    let note = Note::Repaired(names);
    let cause = note.cause();
    let mut first = false;
    let _ = app.update_task(&task.id, |t| first = t.record_pr_ci_told(&cause));
    if first {
        let url = task.pr_url.as_deref().unwrap_or_default();
        crate::engine::delivery::pr::pending::notify_owner(app, &task.repo, note.text(task, url));
    }
}
