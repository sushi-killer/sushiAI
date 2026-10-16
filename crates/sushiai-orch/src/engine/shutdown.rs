use crate::clip::truncate_chars;
use crate::engine::state::App;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::atomic::Ordering;
impl App {
    #[cfg(test)]
    pub(crate) fn recovery_timers_started(&self) -> bool {
        self.recovery_timers_started.load(Ordering::SeqCst)
    }
}

impl App {
    /// Cancels every live task loop (each one's own `run_harness`/verify
    /// call kills its child's process group on seeing this), every live
    /// orchestrator chat turn (`crate::engine::chat::stream::run` kills its child the same way on
    /// seeing its own `CancelToken`). Called when the daemon stops -- a chat turn's `setsid`'d
    /// child is just as capable of leaking past the daemon exiting as a task
    /// attempt's, so it needs the same cancel-on-shutdown treatment.
    #[cfg(test)]
    pub(in crate::engine) fn shutdown(&self) {
        self.shutdown_for(false);
    }
}

impl App {
    /// `true` while a daemon shutdown leaves agent runs going for the next daemon to adopt.
    pub(super) fn runs_survive_shutdown(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst) && !self.disabling.load(Ordering::SeqCst)
    }
}

impl App {
    /// `shutdown`; `disabling` also kills every agent run (see `runs_survive_shutdown`).
    pub fn shutdown_for(&self, disabling: bool) {
        self.disabling.store(disabling, Ordering::SeqCst);
        self.shutting_down.store(true, Ordering::SeqCst);
        self.deferred_wakes.lock().unwrap().clear();
        for ctrl in self.controls.lock().unwrap().values() {
            ctrl.signals.cancel.cancel();
        }
        for cancel in self.chat_turns.lock().unwrap().values() {
            cancel.cancel();
        }
        for cancel in self.audits.lock().unwrap().values() {
            cancel.cancel();
        }
        for cancel in self.proposals.lock().unwrap().values() {
            cancel.cancel();
        }
        self.base_cancel.cancel();
    }
}

impl App {
    /// The status a task takes when its loop is cancelled: `stopped` for an
    /// owner's stop, but a daemon shutdown (upgrade, SIGTERM) leaves the task
    /// in the state crash recovery already resumes -- a running task goes back
    /// to `queued`, a drafting or waiting one stays as it is.
    pub(super) fn cancelled_status(&self, current: &TaskStatus) -> TaskStatus {
        if !self.shutting_down.load(Ordering::SeqCst) {
            return TaskStatus::Stopped;
        }
        match current {
            TaskStatus::Running => TaskStatus::Queued,
            other => *other,
        }
    }
}

impl App {
    pub(super) fn close_stopped_task(&self, task: &mut Task) {
        if let Some(question) = task.take_question() {
            task.record_decision(format!(
                "Stop: closed the open question \"{}\"",
                truncate_chars(&question.text, 200)
            ));
        }
        self.leases.lock().unwrap().remove(&task.id);
        self.permission_waits.lock().unwrap().remove(&task.id);
        task.end_dirty_wait();
    }
}

impl App {
    /// Whether any task loop is still winding down after `shutdown()` --
    /// `Orch::shutdown_with` polls this briefly before the daemon exits, so
    /// a cancelled child gets a real chance to be killed instead of just
    /// orphaned by the daemon disappearing out from under it.
    pub fn any_task_loop_running(&self) -> bool {
        !self.controls.lock().unwrap().is_empty()
    }
}

impl App {
    /// Same as `any_task_loop_running`, but for orchestrator chat turns:
    /// `crate::engine::chat::turn::run_turn` only removes its `chat_turns` entry once `run()` has
    /// returned (child killed and reaped, or exited on its own), so an
    /// empty map here means there is no live chat child left to leak.
    pub fn any_chat_turn_running(&self) -> bool {
        !self.chat_turns.lock().unwrap().is_empty()
    }
}

impl App {
    /// Same again for `repo.audit` and evolution proposer runs: an entry
    /// goes only once its run has returned.
    pub fn any_audit_running(&self) -> bool {
        !self.audits.lock().unwrap().is_empty() || !self.proposals.lock().unwrap().is_empty()
    }
}

impl App {
    /// Same again for background base runs: a key leaves `base_jobs` only
    /// after its run returned and its scratch worktree is gone.
    pub fn any_base_job_running(&self) -> bool {
        !self.base_jobs.lock().unwrap().is_empty()
    }
}
