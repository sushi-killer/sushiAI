use super::lenses::route_lenses;
use super::preparation::PreparedReview;
use super::types::{ReviewFailure, ReviewSaw};
use crate::engine::lifecycle::report::ImplementerReport;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::policy::routing::Route;
use crate::task::{Fingerprint, ReviewResult, Task, VerifyOutcome};
use crate::verify::review::routing::Lenses;
use std::path::PathBuf;
use std::sync::Arc;

/// Retry guidance and scope of a review pass.
#[derive(Clone, Copy)]
pub(in crate::engine) struct ReviewMode {
    pub(in crate::engine) after_no_verdict: bool,
    pub(in crate::engine) force_full: bool,
}

/// The immutable target of one review pass, including its cancellation scope.
pub(in crate::engine) struct ReviewSession {
    pub(super) attempt_n: u32,
    pub(super) worktree: PathBuf,
    pub(super) base_sha: String,
    pub(super) route: Route,
    pub(super) cancel: CancelToken,
    pub(super) mode: ReviewMode,
}

/// Spend and provenance survive every review outcome, including cancellation.
pub(in crate::engine) struct ReviewObservation {
    pub(in crate::engine) result: Result<(ReviewResult, bool, ReviewSaw), ReviewFailure>,
    pub(in crate::engine) cost_usd: f64,
    pub(in crate::engine) fingerprint: Option<Fingerprint>,
}

impl ReviewSession {
    pub(in crate::engine) fn new(
        task: &Task,
        attempt_n: u32,
        route: &Route,
        cancel: &CancelToken,
        mode: ReviewMode,
    ) -> Self {
        Self {
            attempt_n,
            worktree: PathBuf::from(&task.worktree),
            base_sha: task.base_sha.clone(),
            route: route.clone(),
            cancel: cancel.clone(),
            mode,
        }
    }

    pub(in crate::engine) async fn run(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        report: &ImplementerReport,
        verified: &[VerifyOutcome],
    ) -> ReviewObservation {
        let prepared = PreparedReview::gather(self, task).await;
        let (lenses, route_cost) = if prepared.narrow.is_none() {
            let (lenses, note, cost) = route_lenses(self, app, task).await;
            task.record_decision(note);
            (lenses, cost)
        } else {
            (Lenses::ALL, 0.0)
        };
        let mut brief = prepared.brief(task, report, verified, &lenses);
        if self.mode.after_no_verdict {
            brief.push('\n');
            brief.push_str(&crate::verify::review::brief::review_retry_note());
        }
        let execution = self.execute(app, &task.id, &brief, &prepared.images).await;
        task.record_decisions(execution.timeout_note);
        ReviewObservation {
            result: execution
                .outcome
                .and_then(|outcome| prepared.resolve(task, outcome)),
            cost_usd: route_cost + execution.cost_usd,
            fingerprint: execution.fingerprint,
        }
    }
}
