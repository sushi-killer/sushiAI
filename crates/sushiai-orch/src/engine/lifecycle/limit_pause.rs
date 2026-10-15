use crate::clock::now_ms;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::task::{Task, TaskStatus};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum PauseWait {
    Ready,
    Stopped,
}

pub(in crate::engine) async fn wait(
    app: &App,
    task: &mut Task,
    signals: &TaskSignals,
) -> Result<PauseWait, String> {
    loop {
        let notified = signals.resume.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if signals.cancel.is_cancelled() {
            return Ok(PauseWait::Stopped);
        }
        *task = app
            .store
            .load_task(&task.id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if signals.cancel.is_cancelled()
            || task.archived
            || task.queue.park.is_some()
            || matches!(
                task.status,
                TaskStatus::Stopped | TaskStatus::Failed | TaskStatus::Done
            )
        {
            return Ok(PauseWait::Stopped);
        }
        let Some(until) = task.queue.paused_until else {
            return Ok(PauseWait::Ready);
        };
        let delay = Duration::from_millis((until - now_ms()).max(0) as u64);
        tokio::select! {
            biased;
            _ = signals.cancel.cancelled() => return Ok(PauseWait::Stopped),
            _ = &mut notified => continue,
            _ = tokio::time::sleep(delay) => {}
        }
        app.update_task_checked(&task.id, |fresh| {
            if fresh.queue.paused_until == Some(until) {
                crate::task::transitions::end_limit_pause(
                    fresh,
                    crate::task::transitions::PauseEnd::Elapsed,
                    now_ms(),
                );
            }
            Ok(())
        })?;
    }
}

#[cfg(test)]
mod tests;
