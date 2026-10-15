use crate::clock::now_ms;
use crate::engine::decision::record_failure;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::FailureKind;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::Arc;

/// Sends a passed attempt back to the loop: it follows the finished run
/// again, so verify and the review judge the tree with no new implement
/// attempt.
pub(in crate::engine) fn back_to_review(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    permit: &mut Option<SlotPermit>,
) -> Tail {
    crate::task::transitions::reopen(task, idx, now_ms());
    task.resume_phase(TaskStatus::Running, task.updated_at);
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    *permit = None;
    Tail::Continue
}

pub(in crate::engine) fn carry_admission(
    app: &Arc<App>,
    task: &mut Task,
    before: &Task,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Option<Tail> {
    use crate::engine::delivery_gate::{publish_carry, DeliveryGate};
    match publish_carry(app, task, before, cancel) {
        Ok(DeliveryGate::Ready) => None,
        Ok(DeliveryGate::Recheck) => {
            *permit = None;
            Some(Tail::Continue)
        }
        Ok(DeliveryGate::Stopped) | Err(_) => {
            *permit = None;
            app.finish_task_loop(&task.id);
            Some(Tail::Return)
        }
    }
}

pub(in crate::engine) fn delivery_admission(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Option<Tail> {
    use crate::engine::delivery_gate::{begin_delivery, DeliveryGate};
    let expected = crate::task::checks::snapshot::CheckSnapshot::capture(task);
    match begin_delivery(app, task, cancel, &expected) {
        Ok(DeliveryGate::Ready) => None,
        Ok(DeliveryGate::Recheck) => {
            match app.store.load_task(&task.id) {
                Ok(Some(fresh)) => *task = fresh,
                _ => {
                    *permit = None;
                    app.finish_task_loop(&task.id);
                    return Some(Tail::Return);
                }
            }
            if cancel.is_cancelled() {
                interrupt_attempt(app, task, idx);
                *permit = None;
                app.finish_task_loop(&task.id);
                return Some(Tail::Return);
            }
            Some(back_to_review(app, task, idx, permit))
        }
        Ok(DeliveryGate::Stopped) => {
            *permit = None;
            app.finish_task_loop(&task.id);
            Some(Tail::Return)
        }
        Err(error) => {
            record_failure(task, idx, FailureKind::Error, error);
            task.mark_failed();
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
            *permit = None;
            app.finish_task_loop(&task.id);
            Some(Tail::Return)
        }
    }
}
