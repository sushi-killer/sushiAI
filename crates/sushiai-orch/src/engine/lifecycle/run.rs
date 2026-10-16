mod candidate;
mod cycle;
mod end;

use crate::engine::decision::{fail_and_continue, LoopSignal};
use crate::engine::leases::watch_lease;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::round::{self, LoopExit, LoopProgress};
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use end::CycleEnd;
use std::sync::Arc;

pub(in crate::engine) async fn run_task_loop(
    app: Arc<App>,
    task_id: String,
    signals: TaskSignals,
    auto_start_after_plan: bool,
) {
    crate::policy::pipelines::task::compiled();
    let mut progress = LoopProgress::new(
        app.settings.read().unwrap().max_attempts,
        auto_start_after_plan,
    );
    loop {
        let (mut task, mut permit) =
            match round::prepare(&app, &task_id, &signals, &mut progress).await {
                Ok(prepared) => prepared,
                Err(LoopExit::Retry) => continue,
                Err(LoopExit::Return) => return,
            };
        let _lease_watch = watch_lease(&app, &task);
        match cycle::execute(&app, &mut task, &signals, &mut permit, &mut progress).await {
            Ok(Tail::Continue) => continue,
            Ok(Tail::Return) | Err(CycleEnd::Finished) => return,
            Err(CycleEnd::Retry(extra)) => progress.budget += extra,
            Err(CycleEnd::Changed) => {}
            Err(CycleEnd::Stopped) => {
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            Err(CycleEnd::Failed(failure)) => {
                match fail_and_continue(
                    &app,
                    &mut task,
                    failure,
                    &mut progress.budget,
                    &mut QuestionSlot::new(&signals, &mut permit),
                )
                .await
                {
                    LoopSignal::Continue | LoopSignal::Changed => {}
                    LoopSignal::Stop => {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
            }
        }
    }
}
