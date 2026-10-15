use super::{LoopExit, LoopProgress};
use crate::engine::graph::parent::ParentEnd;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use std::sync::Arc;

pub(super) async fn admit(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    match crate::engine::lifecycle::admission::graph(
        app,
        task,
        signals,
        progress.parent_failure.as_deref(),
    )
    .await
    {
        crate::engine::lifecycle::admission::Admission::Ready { carry_conflict } => {
            if let Some(failure) = carry_conflict {
                progress.parent_failure = Some(failure);
            }
        }
        crate::engine::lifecycle::admission::Admission::Waiting
        | crate::engine::lifecycle::admission::Admission::Parent(ParentEnd::Finished) => {
            app.finish_task_loop(&task_id);
            return Err(LoopExit::Return);
        }
        crate::engine::lifecycle::admission::Admission::Parent(ParentEnd::Attempt {
            failure,
            budget,
        }) => {
            progress.parent_failure = Some(failure);
            progress.budget = budget.unwrap_or(progress.budget);
            return Err(LoopExit::Retry);
        }
    }

    Ok(())
}
