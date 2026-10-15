use std::path::PathBuf;

/// The persisted attempt identity and the files belonging to that attempt.
pub(in crate::engine) struct AttemptFiles {
    pub(in crate::engine) idx: usize,
    pub(in crate::engine) n: u32,
    pub(in crate::engine) worktree: PathBuf,
    pub(in crate::engine) run_dir: PathBuf,
}

impl AttemptFiles {
    pub(in crate::engine) fn new(idx: usize, n: u32, worktree: PathBuf, run_dir: PathBuf) -> Self {
        Self {
            idx,
            n,
            worktree,
            run_dir,
        }
    }
}
