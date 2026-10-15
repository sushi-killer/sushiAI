mod amendments;
mod hooks;
mod settlement;

use crate::clock::now_ms;
use crate::engine::cost::settle_unfinished_cost;
use crate::engine::decision::failure;
use crate::engine::lifecycle::execution::{ExecutedAttempt, RunReceipt};
use crate::engine::lifecycle::native_settlement::Publication;
use crate::engine::state::App;
use crate::harness::native::error::RunError;
use crate::harness::RunOutcome;
use crate::task::{attempts, Failure, FailureKind, Task, TaskStatus};
use std::collections::BTreeSet;

pub(super) struct ObservedRun {
    pub(super) run: RunReceipt,
    pub(super) outcome: RunOutcome,
    pub(super) handled: BTreeSet<String>,
    pub(super) discarded: Vec<String>,
}

pub(super) enum NativeEnd {
    Changed(Box<ObservedRun>),
    Stopped,
    Missing,
    Failed(Failure),
}

pub(super) fn settle(
    app: &App,
    task: &mut Task,
    mut executed: ExecutedAttempt,
) -> Result<ObservedRun, NativeEnd> {
    if matches!(executed.run_result, Err(RunError::Cancelled))
        && app.leaves_run_going(task, executed.run.idx)
    {
        if let Some((token, _)) = executed.registered.take() {
            app.unload_hook(&token);
        }
        return Err(NativeEnd::Stopped);
    }
    let handled = hooks::finish(app, task, &mut executed);
    let publication = settlement::publish(app, task, &mut executed)?;
    let outcome = match executed.run_result {
        Ok(outcome) => outcome,
        Err(RunError::Cancelled) => {
            cancel(app, task, &executed.run);
            return Err(NativeEnd::Stopped);
        }
        Err(RunError::NotFound(message)) => {
            app.mark_harness_missing(executed.run.route.harness);
            let attempt = &task.attempts[executed.run.idx];
            if attempt.cost_usd.is_some_and(|cost| cost > 0.0)
                || attempt
                    .usage
                    .as_ref()
                    .is_some_and(|usage| usage.input > 0 || usage.output > 0 || usage.cached > 0)
            {
                return Err(NativeEnd::Failed(failure(FailureKind::Error, message)));
            }
            attempts::discard(task, executed.run.idx, now_ms());
            task.record_decision(format!("Orchestrator: {message}; no attempt was used"));
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
            return Err(NativeEnd::Missing);
        }
        Err(RunError::Io(message)) => {
            return Err(NativeEnd::Failed(failure(FailureKind::Error, message)));
        }
    };
    let observed = ObservedRun {
        run: executed.run,
        outcome,
        handled,
        discarded: executed.best_of.discarded,
    };
    if publication == Publication::Changed {
        Err(NativeEnd::Changed(Box::new(observed)))
    } else {
        Ok(observed)
    }
}

fn cancel(app: &App, task: &mut Task, run: &RunReceipt) {
    attempts::end(task, run.idx, attempts::AttemptEnd::Interrupted, now_ms());
    settle_unfinished_cost(task, run.idx, &app.store, &run.settings.prices);
    app.apply_cancellation(task);
    if task.status == TaskStatus::Stopped {
        app.close_stopped_task(task);
    }
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
}
