use crate::delivery::git::carry::{uncommit_to, Rebase};
use crate::engine::fresh_base;
use crate::git::history::{head_sha, is_ancestor, resolve_commit};
use crate::git::GitError;
use crate::task::checks::types::PlannerTests;
use crate::task::state::Task;
use std::path::Path;

pub(super) struct CarriedWork {
    pub(super) rebase: Rebase,
    pub(super) already_contained: bool,
    pub(super) tests: PlannerTests,
}

pub(super) fn carry_relay(
    snapshot: &mut Task,
    worktree: &Path,
    chain_base: &str,
    parent_branch: &str,
) -> Result<CarriedWork, GitError> {
    uncommit_to(worktree, chain_base)?;
    crate::task::transitions::move_base(snapshot, chain_base.to_string());
    snapshot.record_base_ref(Some(parent_branch.to_string()));
    snapshot.record_relay_base(Some(chain_base.to_string()));
    carry(snapshot, worktree, parent_branch)
}

pub(super) fn carry(
    snapshot: &mut Task,
    worktree: &Path,
    base_ref: &str,
) -> Result<CarriedWork, GitError> {
    let from = snapshot.base_sha.clone();
    let tip = snapshot
        .fresh_base
        .as_ref()
        .and_then(|fresh| fresh.tip.clone());
    let new_sha = resolve_commit(
        worktree,
        &fresh_base::carry_rev(worktree, base_ref, tip.as_deref()),
    )?;
    if new_sha == from {
        return Ok(CarriedWork {
            rebase: Rebase::Unchanged,
            already_contained: false,
            tests: snapshot.planner_tests.clone(),
        });
    }
    // Preserve a merge the task already made; do not carry the base twice.
    if is_ancestor(worktree, &new_sha, "HEAD") == Some(true) {
        return Ok(CarriedWork {
            rebase: Rebase::Moved {
                new_sha,
                rewritten: false,
            },
            already_contained: true,
            tests: snapshot.planner_tests.clone(),
        });
    }
    // Work committed by an attempt goes back to uncommitted.
    if head_sha(worktree)? != from {
        uncommit_to(worktree, &from)?;
    }
    let rebase = fresh_base::carry_work(snapshot, worktree, base_ref)?;
    Ok(CarriedWork {
        rebase,
        already_contained: false,
        tests: snapshot.planner_tests.clone(),
    })
}
