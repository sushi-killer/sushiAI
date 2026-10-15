use crate::task::attempt::DiffStat;
use crate::task::state::Task;

/// The implementer's own work: the diff from the task's base without the
/// planner's test files, which orchd writes into the worktree itself.
#[derive(Debug, Clone)]
pub struct OwnDiff {
    base: String,
    exclude: Vec<String>,
}

impl OwnDiff {
    /// The changed files (committed, staged, unstaged, untracked), without the
    /// planner's paths.
    pub fn changed_files(
        &self,
        worktree: &std::path::Path,
    ) -> Result<Vec<String>, crate::git::GitError> {
        let mut files = crate::git::changes::changed_files(worktree, &self.base)?;
        files.retain(|f| !self.exclude.contains(f));
        Ok(files)
    }

    /// `git diff` against the base, without the planner's paths, truncated to
    /// `max_chars`.
    pub fn diff(
        &self,
        worktree: &std::path::Path,
        max_chars: usize,
    ) -> Result<String, crate::git::GitError> {
        crate::git::changes::diff_full_without(worktree, &self.base, &self.exclude, max_chars)
    }

    /// The size of the worktree's content against the base, without the
    /// planner's paths.
    pub fn stat(&self, worktree: &std::path::Path) -> Result<DiffStat, crate::git::GitError> {
        let tree = crate::git::changes::worktree_tree(worktree)?;
        let mut entries = crate::git::changes::numstat(worktree, &self.base, &tree)?;
        entries.retain(|(path, _, _)| !self.exclude.contains(path));
        Ok(crate::git::changes::fold_numstat(&entries))
    }
}

impl Task {
    /// The implementer's own work, measured from the base.
    pub fn own_diff(&self) -> OwnDiff {
        OwnDiff {
            base: self.base_sha.clone(),
            exclude: self
                .planner_tests
                .files
                .iter()
                .map(|t| t.path.clone())
                .collect(),
        }
    }
}
