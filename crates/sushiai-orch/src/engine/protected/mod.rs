//! The planner's test files and the files the checks name: kept with the task
//! (content and hash), written into the worktree before the first attempt
//! and written again whenever an attempt changes them. A file the checks name
//! that exists in the base may grow (only added lines); the review is shown
//! the additions. The planner's files are committed only in the final landing
//! or pull request commit.

pub(in crate::engine) mod carry;
pub(in crate::engine) mod paths;
pub(in crate::engine) mod prepare;
pub(in crate::engine) mod restore;
pub(in crate::engine) mod snapshot;

#[cfg(test)]
mod tests;
