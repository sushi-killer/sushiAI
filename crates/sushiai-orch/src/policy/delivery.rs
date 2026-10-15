/// Why landing on a default branch was refused, with the one-line way to allow it.
pub fn land_on_default_refusal(branch: &str, repo: &str) -> String {
    format!(
        "{branch} is the default branch and landing there is not allowed for {repo}. To allow it, add {repo} under Settings > Orchestration > Repos where landing on the default branch is allowed."
    )
}

/// Where a new task's base came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BaseFrom {
    /// Taken as it is, with no fetch: `HEAD`, a parent's branch, the branch
    /// a follow-up continues.
    Plain,
    /// `base` on `task.create`.
    Given,
    /// The project's default branch, for a `task.create` with no `base`.
    Project,
}
