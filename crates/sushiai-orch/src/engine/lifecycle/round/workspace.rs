use super::LoopExit;
use crate::engine::decision::completed_implement_attempts;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use std::sync::Arc;

pub(super) async fn prepare(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
) -> Result<(), LoopExit> {
    let cancel = &signals.cancel;
    let pending_amend = &signals.pending_amend;
    let Some(recreated) = crate::engine::lifecycle::workspace::restore(app, task, cancel).await
    else {
        return Err(LoopExit::Return);
    };
    if !crate::engine::lifecycle::workspace::setup(app, task, recreated, cancel).await {
        return Err(LoopExit::Return);
    }

    if let Err(error) = apply_pending_amendment(app, task, pending_amend, cancel).await {
        eprintln!("orch: applying amendment to {} failed: {error}", task.id);
        app.finish_task_loop(&task.id);
        return Err(LoopExit::Return);
    }
    if completed_implement_attempts(task) > 0
        && crate::engine::verification::baseline::needs_baseline(task)
    {
        crate::engine::verification::base::amendments::baseline_current_plan(app, task, cancel)
            .await;
    }

    Ok(())
}
