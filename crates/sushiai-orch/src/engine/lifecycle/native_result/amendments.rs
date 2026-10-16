use super::{NativeEnd, ObservedRun};
use crate::engine::lifecycle::native_settlement::{NativeSettlement, Publication};
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::task::Task;
use std::sync::Arc;

impl ObservedRun {
    pub(in crate::engine::lifecycle) async fn after_amendments(
        self: Box<Self>,
        app: &Arc<App>,
        task: &mut Task,
        signals: &TaskSignals,
    ) -> Result<Box<Self>, NativeEnd> {
        loop {
            *task = app
                .store
                .load_task(&task.id)
                .map_err(|error| {
                    eprintln!(
                        "orch: reloading native result for {} failed: {error}",
                        task.id
                    );
                    NativeEnd::Stopped
                })?
                .ok_or(NativeEnd::Stopped)?;
            apply_pending_amendment(app, task, &signals.pending_amend, &signals.cancel)
                .await
                .map_err(|error| {
                    eprintln!(
                        "orch: applying amendment after native result for {} failed: {error}",
                        task.id
                    );
                    NativeEnd::Stopped
                })?;
            let mut settlement = NativeSettlement::capture(task, self.run.idx, 0.0, &[])
                .map_err(|_| NativeEnd::Stopped)?;
            match settlement.publish(app, task).map_err(|error| {
                eprintln!(
                    "orch: admitting observed native result for {} failed: {error}",
                    task.id
                );
                NativeEnd::Stopped
            })? {
                Publication::Current => return Ok(self),
                Publication::Changed => continue,
                Publication::Stopped => return Err(NativeEnd::Stopped),
            }
        }
    }
}
