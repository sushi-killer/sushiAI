use super::Tail;
use crate::engine::decision::{fail_and_continue, LoopSignal};
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::state::App;
use crate::task::{Failure, Task};
use std::sync::Arc;

pub(super) async fn after_failure(
    app: &Arc<App>,
    task: &mut Task,
    failure: Failure,
    budget: &mut u32,
    slot: &mut QuestionSlot<'_>,
) -> Tail {
    let signal = fail_and_continue(app, task, failure, budget, slot).await;
    slot.release();
    match signal {
        LoopSignal::Continue | LoopSignal::Changed => Tail::Continue,
        LoopSignal::Stop => {
            app.finish_task_loop(&task.id);
            Tail::Return
        }
    }
}
