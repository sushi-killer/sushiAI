use super::{LoopExit, LoopProgress};
use crate::engine::decision::completed_implement_attempts;
use crate::engine::graph::relay::start_relay_link;
use crate::engine::lifecycle::carry::carry_onto_newest_base;
use crate::engine::lifecycle::carry::BaseCarry;
use crate::engine::lifecycle::completion::admission::carry_admission;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::protected::restore::repair_protection;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) async fn prepare(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
    permit: &mut Option<crate::engine::slots::SlotPermit>,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    let cancel = &signals.cancel;
    // A task in a graph starts from its base branch as it is now, so a
    // dependent begins on top of the work it waited for (a subtask on
    // its parent's head, with every earlier sibling landed).
    if let Some(prev_id) = task.queue.relay_of.clone() {
        if completed_implement_attempts(task) == 0 && task.queue.relay_base.is_none() {
            let before_relay = task.clone();
            start_relay_link(app, task, &prev_id).await;
            if let Some(tail) = carry_admission(
                app,
                task,
                &before_relay,
                &mut QuestionSlot::new(signals, permit),
            ) {
                match tail {
                    Tail::Return => return Err(LoopExit::Return),
                    Tail::Continue => return Err(LoopExit::Retry),
                }
            }
        }
    }
    let wt = PathBuf::from(&task.worktree);
    repair_protection(task, &wt).await;
    let carried = if task.parent.is_some() {
        let _ = app.sync_graph_base(&task.id, false).await;
        app.carry_part_onto_parent(task, cancel).await
    } else if completed_implement_attempts(task) == 0
        && progress.parent_failure.is_none()
        && !task.depends_on.is_empty()
        && task.queue.relay_of.is_none()
    {
        let wt = PathBuf::from(&task.worktree);
        carry_onto_newest_base(app, task, &wt, cancel).await
    } else {
        BaseCarry::Ready
    };
    match carried {
        BaseCarry::Stale => {
            permit.take();
            return Err(LoopExit::Retry);
        }
        BaseCarry::Stopped => {
            permit.take();
            app.finish_task_loop(&task_id);
            return Err(LoopExit::Return);
        }
        _ => {}
    }
    if cancel.is_cancelled() {
        mark_stopped_if_not_already(app, &task_id).await;
        permit.take();
        app.finish_task_loop(&task_id);
        return Err(LoopExit::Return);
    }

    Ok(())
}
