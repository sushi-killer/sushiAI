use crate::engine::attempts::best_of::evaluate::Eval;
use crate::harness::native::error::RunError;
use crate::harness::RunOutcome;
use crate::policy::routing::Route;
use std::path::PathBuf;

/// A completed native candidate and the checkout that produced its result.
pub(super) struct CandidateRun {
    pub(super) route: Route,
    pub(super) worktree: PathBuf,
    pub(super) run_dir: PathBuf,
    pub(super) outcome: Result<RunOutcome, RunError>,
    pub(super) cost: f64,
}

pub(super) struct CandidateEvidence {
    pub(super) run: CandidateRun,
    pub(super) checks: Eval,
    pub(super) restored: Vec<String>,
}

pub(super) struct CompletedCandidates {
    pub(super) a: CandidateRun,
    pub(super) b: CandidateRun,
}

pub(super) struct EvaluatedCandidates {
    pub(super) a: CandidateEvidence,
    pub(super) b: CandidateEvidence,
    pub(super) decisions: Vec<String>,
}
