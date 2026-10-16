/// A refusal orchd gives on its own. Whether a retry can help is decided by
/// the variant ([`PrRefusal::permanent`]), never by its text; `Display` is the
/// message the owner reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PrRefusal {
    /// The request names no usable task or arguments; the text says what.
    Invalid(String),
    NotFound,
    NotTopLevel,
    NotDone,
    TaskChanged,
    /// The overseer asked while orchd owns the pull request.
    OverseerOwns,
    Archived,
    Landed,
    /// Another `task.pr` for this task is still running.
    InFlight,
    /// An agent named a branch; it may only open the task's own.
    AgentOwnBranch,
    /// The remote branch name is not one git accepts.
    BadBranch(String),
    /// The task's own branch is gone.
    BranchGone(String),
    /// The repo has no default branch to open the pull request against.
    NoDefaultBranch,
    /// The remote branch name is a base branch.
    BaseBranch(String),
    /// An agent asked while automatic pull requests are off for the repo.
    AutomaticOff,
    /// `origin/<base>` already holds the work.
    Contained(String),
    /// The work conflicts with `origin/<base>`; a resolution attempt runs first.
    Resolving {
        base: String,
        files: Vec<String>,
    },
    /// The carry ended the task `failed`; the reason.
    CarryFailed(String),
    NoCommits {
        branch: String,
        base: String,
    },
    /// `origin` has a branch of this name that this task did not push.
    ForeignBranch(String),
    /// A pull request for this task is already open on `on`, at `url`.
    AlreadyOpen {
        on: String,
        url: String,
    },
}

impl PrRefusal {
    /// Whether a retry can only be refused again. A busy `task.pr` may pass later.
    pub(crate) fn permanent(&self) -> bool {
        !matches!(self, Self::InFlight)
    }
}

impl std::fmt::Display for PrRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(why) => f.write_str(why),
            Self::NotFound => f.write_str("task not found"),
            Self::NotTopLevel => f.write_str("only a top-level task can open a pull request"),
            Self::NotDone => f.write_str("only a done task can open a pull request"),
            Self::TaskChanged => f.write_str("task changed or was stopped while carrying onto the base"),
            Self::OverseerOwns => f.write_str(
                "orchd opens this task's pull request itself; do not call task_pr while prPending is set",
            ),
            Self::Archived => f.write_str("task is archived; unarchive it first"),
            Self::Landed => f.write_str(
                "task is already landed; there is nothing to open a pull request for",
            ),
            Self::InFlight => f.write_str("a pull request is already being opened"),
            Self::AgentOwnBranch => {
                f.write_str("an agent opens the pull request for the task's own branch only")
            }
            Self::BadBranch(name) => write!(f, "{name:?} is not a valid branch name"),
            Self::BranchGone(branch) => write!(f, "the task branch {branch} no longer exists"),
            Self::NoDefaultBranch => f.write_str(
                "the repo has no default branch to open a pull request against",
            ),
            Self::AutomaticOff => f.write_str(
                "automatic pull requests are off for this repo: the owner opens the pull request",
            ),
            Self::BaseBranch(branch) => write!(
                f,
                "{branch} is a base branch; pick another remote branch name for the pull request"
            ),
            Self::Contained(base) => write!(
                f,
                "origin/{base} already contains this work; nothing to open a pull request for"
            ),
            Self::Resolving { base, files } => write!(
                f,
                "the work conflicts with origin/{base} in {}; one conflict-resolution attempt runs now and the pull request opens when it is done",
                files.join(", ")
            ),
            Self::CarryFailed(reason) => f.write_str(reason),
            Self::NoCommits { branch, base } => write!(
                f,
                "branch {branch} has no commits beyond origin/{base}; nothing to open a pull request for"
            ),
            Self::ForeignBranch(branch) => write!(
                f,
                "origin already has a branch {branch} that is not part of this task; pick another name"
            ),
            Self::AlreadyOpen { on, url } => {
                write!(f, "a pull request is already open for this task on {on}: {url}")
            }
        }
    }
}

/// What a `task.pr` attempt failed with: orchd's own refusal, or a git, gh or
/// store failure whose text the retry reads once (`crate::delivery::pr::failure::classify_external`).
#[derive(Debug)]
pub(crate) enum PrError {
    Refused(PrRefusal),
    External(String),
}

impl From<PrRefusal> for PrError {
    fn from(refusal: PrRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<String> for PrError {
    fn from(error: String) -> Self {
        Self::External(error)
    }
}

impl std::fmt::Display for PrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(f, "{refusal}"),
            Self::External(error) => f.write_str(error),
        }
    }
}
