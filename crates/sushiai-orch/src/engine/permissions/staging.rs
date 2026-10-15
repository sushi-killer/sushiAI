use crate::engine::run_control::HookContext;
use std::collections::BTreeSet;

/// Where an allowed write to a protected path is made; inside the worktree,
/// copied to the real path by [`apply_staged`] and then removed.
pub(in crate::engine) const STAGING_DIR: &str = ".orchd-staging";

pub(super) fn staging_reason(ctx: &HookContext, rel: &str) -> String {
    let staged = ctx.worktree.join(STAGING_DIR).join(rel);
    format!(
        "The owner allowed this write, but Claude Code blocks {rel} for agents. Make the same change at {} instead (if {rel} exists, copy it there first, then edit the copy): orchd copies it to {rel} before verification. Do not retry {rel} itself.",
        staged.display()
    )
}

/// Copies what the agent staged at an approved path to its real place, and
/// clears the staging dir. Returns the paths it applied.
pub(in crate::engine) fn apply_staged(ctx: &HookContext) -> Vec<String> {
    let staging = ctx.worktree.join(STAGING_DIR);
    let mut applied = Vec::new();
    let staged: BTreeSet<String> = ctx.staged.lock().unwrap().clone();
    for rel in staged {
        let from = staging.join(&rel);
        let to = ctx.worktree.join(&rel);
        if !from.is_file() {
            continue;
        }
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::copy(&from, &to).is_ok() {
            applied.push(rel);
        }
    }
    applied
}

/// Final apply at the end of a run: what was staged lands, the dir goes.
pub(in crate::engine) fn finish_staging(ctx: &HookContext) -> Vec<String> {
    let applied = apply_staged(ctx);
    let _ = std::fs::remove_dir_all(ctx.worktree.join(STAGING_DIR));
    applied
}

pub(in crate::engine) fn staging_lines(applied: &[String]) -> Vec<String> {
    applied
        .iter()
        .map(|rel| format!("Permission: copied the staged {rel} into place"))
        .collect()
}
