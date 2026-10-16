use super::NativeEnd;
use crate::engine::lifecycle::execution::ExecutedAttempt;
use crate::engine::lifecycle::native_settlement::{NativeSettlement, Publication};
use crate::engine::state::App;
use crate::engine::verification::context::record_check_decisions;
use crate::task::attempts::{self, BestOfSelection, RunCompletion};
use crate::task::{Task, Usage};

pub(super) fn publish(
    app: &App,
    task: &mut Task,
    executed: &mut ExecutedAttempt,
) -> Result<Publication, NativeEnd> {
    record_check_decisions(task, std::mem::take(&mut executed.best_of.decisions));
    let sibling_won = executed.best_of.winner.is_some();
    if let Some(route) = &executed.best_of.winner {
        executed.run.route = route.clone();
    }
    let mut settlement = NativeSettlement::capture(
        task,
        executed.run.idx,
        executed.best_of.cost,
        &executed.best_of.discarded,
    )
    .map_err(|error| {
        eprintln!(
            "orch: capturing native result for {} failed: {error}",
            task.id
        );
        NativeEnd::Stopped
    })?;
    attempts::record_best_of(
        task,
        executed.run.idx,
        BestOfSelection {
            winner: executed.best_of.winner.take(),
            candidates: std::mem::take(&mut executed.best_of.candidates),
            extra_cost: executed.best_of.cost,
        },
    );
    if let Ok(outcome) = &executed.run_result {
        let mut cost = outcome.cost_usd;
        let mut estimated = outcome.cost_estimated;
        let mut usage = Usage {
            input: outcome.usage_input,
            output: outcome.usage_output,
            cached: outcome.usage_cached,
        };
        let recorded = task.attempts[executed.run.idx].fingerprint.is_some()
            && task.attempts[executed.run.idx].usage.is_some();
        let rejected = (!recorded)
            .then(|| {
                crate::engine::cost::rejected_resume_cost(
                    task,
                    executed.run.idx,
                    &app.store,
                    &executed.run.settings.prices,
                )
            })
            .flatten();
        if let Some(rejected) = rejected {
            cost = match (cost, rejected.cost_usd) {
                (Some(fresh), Some(prior)) => Some(fresh + prior),
                (known, None) | (None, known) => known,
            };
            estimated |= rejected.cost_estimated;
            usage.input = usage.input.saturating_add(rejected.usage_input);
            usage.output = usage.output.saturating_add(rejected.usage_output);
            usage.cached = usage.cached.saturating_add(rejected.usage_cached);
        }
        attempts::record_run_completion(
            task,
            executed.run.idx,
            RunCompletion {
                session: outcome.session_id.clone().filter(|_| !sibling_won),
                fingerprint: outcome.fingerprint.clone(),
                usage,
                first_turn_tokens: outcome.first_turn_tokens,
                cost,
                estimated,
            },
        );
    }
    if executed.run_result.is_err() {
        crate::engine::cost::settle_unfinished_cost(
            task,
            executed.run.idx,
            &app.store,
            &executed.run.settings.prices,
        );
    }
    match settlement.publish(app, task) {
        Ok(publication @ (Publication::Current | Publication::Changed)) => Ok(publication),
        Ok(Publication::Stopped) => Err(NativeEnd::Stopped),
        Err(error) => {
            eprintln!("orch: saving native result for {} failed: {error}", task.id);
            Err(NativeEnd::Stopped)
        }
    }
}
