use super::admission::{back_to_review, delivery_admission};
use super::retry::after_failure;
use super::Tail;
use crate::clock::now_ms;
use crate::engine::graph::landing::Landing;
use crate::engine::ladder;
use crate::engine::land::run::land_on_base;
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::policy::pipelines::delivery::{DeliveryFlow, DeliveryOutcome, DeliveryStep};
use crate::task::Task;
use std::sync::Arc;

enum DeliveryReceipt {
    Start,
    Admitted,
    Landing(Option<Landing>),
}

impl AttemptFiles {
    /// Delivers a passed attempt to its parent/base or commits its own branch.
    pub(in crate::engine) async fn finish(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        attempt_budget: &mut u32,
        slot: &mut QuestionSlot<'_>,
    ) -> Tail {
        let flow = DeliveryFlow::compiled();
        let mut step = flow.entry();
        let mut receipt = DeliveryReceipt::Start;
        let cancel = slot.cancel().clone();
        loop {
            let (outcome, produced) = match (step, receipt) {
                (DeliveryStep::Admit, DeliveryReceipt::Start) => {
                    if let Some(tail) = delivery_admission(app, task, self.idx, slot) {
                        return tail;
                    }
                    ladder::inherit_unreviewed(app, task);
                    (DeliveryOutcome::Admitted, DeliveryReceipt::Admitted)
                }
                (DeliveryStep::Land, DeliveryReceipt::Admitted) => {
                    let landing = self.land(app, task, &cancel).await;
                    (landing_outcome(&landing), DeliveryReceipt::Landing(landing))
                }
                (
                    DeliveryStep::Commit,
                    DeliveryReceipt::Landing(
                        None | Some(Landing::NoParent | Landing::DefaultBranchRefused),
                    ),
                ) => return self.commit(app, task, &cancel, slot).await,
                (DeliveryStep::Wait, DeliveryReceipt::Landing(Some(Landing::Waiting))) => {
                    return self.waiting(app, task, slot);
                }
                (DeliveryStep::Landed, DeliveryReceipt::Landing(Some(Landing::Landed))) => {
                    return self.landed(app, task, slot).await;
                }
                (
                    DeliveryStep::Retry,
                    DeliveryReceipt::Landing(Some(Landing::Failed { kind, detail })),
                ) => {
                    return after_failure(
                        app,
                        task,
                        crate::engine::decision::failure(kind, detail),
                        attempt_budget,
                        slot,
                    )
                    .await;
                }
                (DeliveryStep::Review, DeliveryReceipt::Landing(Some(Landing::Rereview))) => {
                    return back_to_review(app, task, self.idx, slot);
                }
                (DeliveryStep::Cancel, DeliveryReceipt::Landing(Some(Landing::Cancelled))) => {
                    return self.cancel_delivery(app, task, slot);
                }
                _ => unreachable!(
                    "the validated delivery graph requires a compatible operation receipt"
                ),
            };
            step = flow.next(step, outcome);
            receipt = produced;
        }
    }

    async fn land(&self, app: &Arc<App>, task: &mut Task, cancel: &CancelToken) -> Option<Landing> {
        // Unreviewed top-level work waits on its branch for the owner's PR.
        if task.unreviewed && task.parent.is_none() {
            None
        } else if let Some(parent) = task.parent.clone() {
            Some(
                crate::engine::graph::carry::CarryTree::new(
                    &self.worktree,
                    &self.run_dir,
                    Some(self.idx),
                    cancel,
                )
                .land_on_parent(app, task, &parent)
                .await,
            )
        } else if task.variant().land {
            Some(
                land_on_base(
                    app,
                    task,
                    &crate::engine::graph::carry::CarryTree::new(
                        &self.worktree,
                        &self.run_dir,
                        Some(self.idx),
                        cancel,
                    ),
                )
                .await,
            )
        } else {
            None
        }
    }

    fn waiting(&self, app: &App, task: &mut Task, slot: &mut QuestionSlot<'_>) -> Tail {
        crate::task::transitions::complete(
            task,
            Some(self.idx),
            crate::task::transitions::Completion::WaitingForLanding,
            now_ms(),
        );
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        slot.release();
        app.finish_task_loop(&task.id);
        Tail::Return
    }

    async fn landed(&self, app: &Arc<App>, task: &mut Task, slot: &mut QuestionSlot<'_>) -> Tail {
        crate::delivery::git::carry::delete_wip_ref(&self.worktree, &task.id);
        crate::task::transitions::complete(
            task,
            Some(self.idx),
            crate::task::transitions::Completion::Done,
            now_ms(),
        );
        task.touch(now_ms());
        app.write_report(task).await;
        app.release_worktree(task, "landed").await;
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        slot.release();
        app.finish_task_loop(&task.id);
        Tail::Return
    }

    fn cancel_delivery(&self, app: &App, task: &mut Task, slot: &mut QuestionSlot<'_>) -> Tail {
        crate::task::attempts::end(
            task,
            self.idx,
            crate::task::attempts::AttemptEnd::Interrupted,
            now_ms(),
        );
        app.apply_cancellation(task);
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        slot.release();
        app.finish_task_loop(&task.id);
        Tail::Return
    }
}

fn landing_outcome(landing: &Option<Landing>) -> DeliveryOutcome {
    match landing {
        None | Some(Landing::NoParent | Landing::DefaultBranchRefused) => DeliveryOutcome::Unlanded,
        Some(Landing::Waiting) => DeliveryOutcome::Waiting,
        Some(Landing::Landed) => DeliveryOutcome::Landed,
        Some(Landing::Failed { .. }) => DeliveryOutcome::Failed,
        Some(Landing::Rereview) => DeliveryOutcome::Rereview,
        Some(Landing::Cancelled) => DeliveryOutcome::Cancelled,
    }
}

#[cfg(test)]
#[path = "delivery/tests.rs"]
mod tests;
