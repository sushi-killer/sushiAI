use crate::engine::attempts::best_of::candidates::run_candidates;
use crate::engine::attempts::best_of::evidence::evaluate_pair;
use crate::engine::attempts::best_of::outcome::{finish, BestOfExtra};
use crate::engine::attempts::best_of::selection::select;
use crate::engine::attempts::best_of::sibling::Sibling;
use crate::engine::attempts::prepare::ImplementRun;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::harness::RunOutcome;
use crate::policy::routing::Route;
use std::sync::Arc;

/// Two routes implementing the same task input, with one retained checkout.
pub(in crate::engine) struct BestOfRun<'a> {
    pub(in crate::engine) primary: ImplementRun<'a>,
    pub(in crate::engine) secondary: &'a Route,
    pub(in crate::engine) brief: &'a str,
    pub(in crate::engine) deny_read: &'a [String],
}

pub(in crate::engine) async fn run_best_of<F>(
    app: &Arc<App>,
    run: &BestOfRun<'_>,
    a_run: F,
    cancel: &CancelToken,
) -> (Result<RunOutcome, RunError>, BestOfExtra)
where
    F: std::future::Future<Output = Result<RunOutcome, RunError>>,
{
    let mut extra = BestOfExtra::default();
    let primary = &run.primary;
    let sibling = match Sibling::create(app, primary.task, primary.run_dir, cancel).await {
        Ok(sibling) => sibling,
        Err(error) => {
            extra.decisions.push(format!(
                "Best-of: could not create the sibling worktree ({error}); ran a single candidate"
            ));
            return (a_run.await, extra);
        }
    };
    let (candidates, notes) = run_candidates(app, run, a_run, &sibling, cancel).await;
    extra.decisions.extend(notes);
    if matches!(&candidates.a.outcome, Err(RunError::Cancelled))
        || matches!(&candidates.b.outcome, Err(RunError::Cancelled))
    {
        extra.cost = candidates.b.cost;
        sibling.cleanup().await;
        return (Err(RunError::Cancelled), extra);
    }
    let evaluated = evaluate_pair(app, primary.task, candidates, primary.settings, cancel).await;
    extra.decisions.extend(evaluated.decisions.iter().cloned());
    let (mut pick_b, mut decision) = match select(app, run, &evaluated, &mut extra, cancel).await {
        Ok(selection) => selection,
        Err(_) => {
            extra.cost += evaluated.b.run.cost;
            sibling.cleanup().await;
            return (Err(RunError::Cancelled), extra);
        }
    };
    if pick_b {
        if let Err(error) = crate::delivery::candidates::carry(
            primary.worktree,
            &sibling.worktree,
            &primary.task.base_sha,
        )
        .await
        {
            pick_b = false;
            decision.push_str(&format!(
                "; could not carry b into the task worktree ({error}), kept a"
            ));
        }
    }
    sibling.cleanup().await;
    extra.decisions.push(decision);
    finish(extra, evaluated, pick_b)
}
