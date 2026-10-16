mod brief;
mod fallback;
mod launch;
mod preflight;

use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::lifecycle::adopted;
use crate::engine::run_control::{HookContext, TaskSignals};
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::harness::{native::error::RunError, RunOutcome};
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::state::Task;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(in crate::engine) enum ExecutionMode<'a> {
    Adopt(usize),
    Start(Option<&'a str>),
}

#[derive(Clone)]
pub(in crate::engine) struct RunReceipt {
    pub(in crate::engine) settings: Settings,
    pub(in crate::engine) attempt_n: u32,
    pub(in crate::engine) idx: usize,
    pub(in crate::engine) route: Route,
    pub(in crate::engine) worktree: PathBuf,
    pub(in crate::engine) base_sha: String,
    pub(in crate::engine) run_dir: PathBuf,
}

pub(in crate::engine) struct ExecutedAttempt {
    pub(in crate::engine) run: RunReceipt,
    pub(in crate::engine) key_path: PathBuf,
    pub(in crate::engine) registered: Option<(String, Arc<HookContext>)>,
    pub(in crate::engine) run_result: Result<RunOutcome, RunError>,
    pub(in crate::engine) best_of: BestOfExtra,
}

pub(in crate::engine) enum ExecutionStep {
    Executed(Box<ExecutedAttempt>),
    Retry,
    Stop,
}

pub(in crate::engine) async fn execute(
    app: &Arc<App>,
    task: &mut Task,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
    mode: ExecutionMode<'_>,
) -> ExecutionStep {
    let execution = match mode {
        ExecutionMode::Adopt(index) => adopted::follow(app, task, index, &signals.cancel).await,
        ExecutionMode::Start(parent) => {
            let (selected, changes) = match preflight::prepare(app, task, signals, permit).await {
                Ok(ready) => ready,
                Err(step) => return step,
            };
            let planned = match brief::prepare(app, task, selected, changes, parent) {
                Ok(planned) => planned,
                Err(step) => {
                    if matches!(step, ExecutionStep::Stop) {
                        *permit = None;
                        app.finish_task_loop(&task.id);
                    }
                    return step;
                }
            };
            launch::execute(app, task, planned, &signals.cancel).await
        }
    };
    ExecutionStep::Executed(Box::new(execution))
}
