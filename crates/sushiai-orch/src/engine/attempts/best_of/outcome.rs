use crate::engine::attempts::best_of::evidence::Evaluated;
use crate::harness;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use crate::task::Candidate;

/// What a best-of attempt adds to the task beside the winner's own run.
#[derive(Default)]
pub(in crate::engine) struct BestOfExtra {
    /// The loser's cost and the pick run's, on top of the winner's.
    pub cost: f64,
    pub candidates: Vec<Candidate>,
    pub decisions: Vec<String>,
    /// The route that produced the winning changes, when it is not the one
    /// the attempt started on.
    pub winner: Option<Route>,
    /// Protected files the winning candidate changed and orchd restored.
    pub discarded: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finish(
    mut extra: BestOfExtra,
    a_res: Result<harness::RunOutcome, RunError>,
    b_res: Result<harness::RunOutcome, RunError>,
    a_route: &Route,
    b_route: &Route,
    cost_a: f64,
    cost_b: f64,
    evaluated: Evaluated,
    pick_b: bool,
) -> (Result<harness::RunOutcome, RunError>, BestOfExtra) {
    extra.discarded = if pick_b {
        evaluated.restored_b
    } else {
        evaluated.restored_a
    };
    extra.candidates = [
        (a_route, cost_a, &evaluated.a, !pick_b),
        (b_route, cost_b, &evaluated.b, pick_b),
    ]
    .into_iter()
    .map(|(route, cost, eval, picked)| Candidate {
        route: route.id.clone(),
        cost,
        verify: eval.verify_ok,
        checks: eval.tally,
        picked,
    })
    .collect();
    if pick_b {
        extra.cost += cost_a;
        extra.winner = Some(b_route.clone());
        (b_res, extra)
    } else {
        extra.cost += cost_b;
        (a_res, extra)
    }
}
