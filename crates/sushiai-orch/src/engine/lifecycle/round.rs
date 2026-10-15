mod base;
mod brief;
mod budget;
mod capabilities;
mod graph;
mod loading;
mod parent;
mod planning;
mod protection;
mod resume;
mod tree;
mod workspace;

use crate::engine::run_control::TaskSignals;
use crate::engine::slots::{take_slot, Slot, SlotPermit};
use crate::engine::state::App;
use crate::task::Task;
use std::sync::Arc;

pub(super) enum LoopExit {
    Retry,
    Return,
}

/// Decisions whose lifetime spans the task's attempt and waiting rounds.
pub(super) struct LoopProgress {
    pub(super) budget: u32,
    pub(super) parent_failure: Option<String>,
    budget_loaded: bool,
    base_refreshed: bool,
    auto_start_after_plan: bool,
}
impl LoopProgress {
    pub(super) fn new(budget: u32, auto_start_after_plan: bool) -> Self {
        Self {
            budget,
            parent_failure: None,
            budget_loaded: false,
            base_refreshed: false,
            auto_start_after_plan,
        }
    }
}

pub(super) async fn prepare(
    app: &Arc<App>,
    task_id: &str,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
) -> Result<(Task, Option<SlotPermit>), LoopExit> {
    let mut task = loading::load(app, task_id, signals, progress).await?;
    workspace::prepare(app, &mut task, signals).await?;
    graph::admit(app, &mut task, signals, progress).await?;
    base::retry(app, &mut task, signals, progress).await?;
    let mut permit = match take_slot(app, &mut task, &signals.cancel).await {
        Slot::Got(permit) => Some(permit),
        Slot::Cancelled => {
            crate::engine::lifecycle::interruption::mark_stopped_if_not_already(app, task_id).await;
            app.finish_task_loop(task_id);
            return Err(LoopExit::Return);
        }
        Slot::Closed => {
            app.finish_task_loop(task_id);
            return Err(LoopExit::Return);
        }
    };
    if !progress.base_refreshed {
        progress.base_refreshed = true;
        crate::engine::fresh_base::refresh_at_start(app, &mut task).await;
    }
    resume::delivery(app, &mut task, signals, progress, &mut permit).await?;
    planning::plan(app, &mut task, signals, progress, &mut permit).await?;
    parent::admit(app, &mut task, signals, progress, &mut permit).await?;
    brief::prepare(app, &mut task, signals, progress, &mut permit).await?;
    capabilities::check(app, &mut task, signals, progress, &mut permit).await?;
    budget::wait(app, &mut task, signals, progress, &mut permit).await?;
    tree::prepare(app, &mut task, signals, progress, &mut permit).await?;
    protection::prepare(app, &mut task, signals, progress, &mut permit).await?;
    Ok((task, permit))
}

#[cfg(test)]
#[path = "round/tests/repair.rs"]
mod repair_tests;
