use super::brief::BriefedAttempt;
use super::ExecutedAttempt;
use crate::engine::attempts::best_of::run::{run_best_of, BestOfRun};
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::prepare::{prepare_run, ImplementRun};
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::task::state::Task;
use std::sync::Arc;

pub(super) async fn execute(
    app: &Arc<App>,
    task: &Task,
    planned: BriefedAttempt,
    cancel: &CancelToken,
) -> ExecutedAttempt {
    let BriefedAttempt {
        run,
        brief,
        tools,
        secondary,
        extra,
    } = planned;
    let implement = ImplementRun {
        task,
        route: &run.route,
        settings: &run.settings,
        worktree: &run.worktree,
        attempt_n: run.attempt_n,
        run_dir: &run.run_dir,
    };
    let prepared = prepare_run(app, &implement, &tools);
    let req = implement.request(&prepared, brief.session.as_deref());
    let _ = std::fs::write(run.run_dir.join("brief.md"), &brief.current);
    let events = run.run_dir.join("events.jsonl");
    let a_run = run_harness(
        app,
        RunIdentity {
            id: &task.id,
            attempt_n: run.attempt_n,
            track: RunTrack::Resumable,
            tag: CostTag::task("implement", &run.route.id),
        },
        RunInvocation {
            request: &req,
            brief: &brief.current,
            events: &events,
            schema: None,
        },
        cancel,
        implement.guard(&prepared),
    );
    let (mut result, mut best_of) = match secondary {
        Some(route) => {
            run_best_of(
                app,
                &BestOfRun {
                    primary: implement,
                    secondary: &route,
                    brief: &brief.current,
                    deny_read: &[app.data_dir.to_string_lossy().to_string()],
                },
                a_run,
                cancel,
            )
            .await
        }
        None => (a_run.await, extra),
    };
    let rejected = match &result {
        Ok(outcome) => outcome.error.is_some() && outcome.session_id.is_none(),
        Err(RunError::Io(_) | RunError::NotFound(_)) => true,
        Err(RunError::Cancelled) => false,
    };
    if req.resume.is_some() && rejected && !cancel.is_cancelled() {
        best_of.decisions.push(format!("Orchestrator: attempt {} resume rejected without a session init; falling back to a fresh run in the same attempt", run.attempt_n));
        result = super::fallback::ResumeFallback {
            run: implement,
            prepared: &prepared,
            brief: &brief,
        }
        .run(app, result, cancel)
        .await;
    }
    ExecutedAttempt {
        run,
        key_path: prepared.key_path,
        registered: prepared.registered,
        run_result: result,
        best_of,
    }
}
