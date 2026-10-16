//! The planner's test files and the files the checks name: kept with the task
//! (content and hash), written into the worktree before the first attempt,
//! denied to the implementer and written again whenever an attempt changes
//! them. They are committed only in the final landing or pull request commit.

pub(in crate::engine) mod carry;
pub(in crate::engine) mod paths;
pub(in crate::engine) mod prepare;
pub(in crate::engine) mod restore;
pub(in crate::engine) mod snapshot;

#[cfg(test)]
mod tests;
