use crate::task::DeclineRuling;
use crate::task::ReviewScope;

/// Why a review run produced no verdict.
pub(in crate::engine) enum ReviewFailure {
    Cancelled,
    /// The harness did not run or errored.
    Harness(String),
    /// The run finished, but its reply had no parseable verdict.
    NoVerdict(String),
}

/// What a review run looked at: the scope it ran with, the worktree tree it
/// saw (`None` when the snapshot failed) and its ruling on each decline.
pub(in crate::engine) struct ReviewSaw {
    pub scope: ReviewScope,
    pub tree: Option<String>,
    pub rulings: Vec<DeclineRuling>,
}
