//! Shared synchronous Git IO. Task transitions are owned by `task`.

pub mod bootstrap;
pub mod branches;
pub mod changes;
pub(crate) mod command;
pub mod history;
pub mod own_diff;
pub mod worktree;

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for GitError {}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod test_support;

pub mod paths;

pub mod repo_settings;
