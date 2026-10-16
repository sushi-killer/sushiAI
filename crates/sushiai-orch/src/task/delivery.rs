use serde::{Deserialize, Serialize};

/// The autonomy metric: a person or the lead session had to fix a task's
/// work after orchd said done. `by` is `owner` (`task.leadTouch`) or `auto`
/// (the landing was rewritten, or someone else edited its files soon after).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeadTouch {
    pub touched: bool,
    #[serde(default)]
    pub note: String,
    pub at: i64,
    pub by: String,
}

/// A named file that was not committed with the task. `path` is relative to
/// the worktree, `saved` is the copy in the task's data dir, `placed` the
/// path it was copied to in the repo's main working copy after landing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deliverable {
    pub path: String,
    pub saved: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed: Option<String>,
}

/// Patch-equivalent commits on a done task's branch that have not landed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unlanded {
    pub commits: u32,
    pub base: String,
}

/// A named base branch as its remote had it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshBase {
    /// The remote tip last fetched. When the local branch does not contain it
    /// (a dirty or diverged checkout left as it was), the task's work is
    /// carried onto this commit instead of the stale local branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<String>,
}

/// What `pr_pending` opens: the remote branch, the title (empty: the task's
/// own) and the body, already redacted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrPlan {
    pub branch: String,
    pub title: String,
    pub body: String,
    /// Who chose `branch`; a plan stored before this field existed is `Task`.
    #[serde(default)]
    pub branch_from: BranchFrom,
}

/// Where a pull request plan's branch name came from. Only `Owner` and
/// `Drafted` names were chosen by someone and are reused on a retry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BranchFrom {
    Owner,
    Drafted,
    /// The task's own branch: the fallback, never a choice.
    #[default]
    Task,
}

impl PrPlan {
    /// Whether a retry opens the pull request as stored, without a new draft.
    pub fn is_reusable(&self) -> bool {
        match self.branch_from {
            BranchFrom::Owner => true,
            BranchFrom::Drafted => !self.title.trim().is_empty() && !self.body.trim().is_empty(),
            BranchFrom::Task => false,
        }
    }
}

#[path = "delivery/operations.rs"]
pub(crate) mod operations;

#[cfg(test)]
#[path = "delivery/tests.rs"]
mod tests;

pub(crate) mod validation;

#[path = "delivery/repair.rs"]
pub(crate) mod repair;
pub use repair::PrCheckRepair;
