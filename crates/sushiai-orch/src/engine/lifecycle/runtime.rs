use crate::clock::now_ms;
use crate::engine::lifecycle::run::run_task_loop;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::state::App;
use crate::engine::verification::context::record_check_decisions;
use crate::engine::verification::context::record_check_outcomes;

impl App {
    pub(in crate::engine) fn start_task_loop(&self, task_id: String) {
        self.spawn_task_loop(task_id, true);
    }

    /// `auto_start_after_plan` only matters for a task still `drafting`: once
    /// planning finishes, `true` lets the loop fall straight through into
    /// the implement stage, `false` leaves it `stopped` (fields filled in)
    /// for the owner to review before calling `task.start` themselves. Every
    /// caller except the `{repo, request}` form of `task.create` wants the
    /// former, since for them planning has either already happened or never
    /// applies.
    pub(in crate::engine) fn spawn_task_loop(&self, task_id: String, auto_start_after_plan: bool) {
        let lock = self.task_lock(&task_id);
        let _guard = lock.lock().unwrap();
        let mut controls = self.controls.lock().unwrap();
        if controls.contains_key(&task_id) {
            return;
        }
        let control = TaskControl::spawn(self.arc(), task_id.clone(), auto_start_after_plan);
        controls.insert(task_id, control);
    }

    pub(in crate::engine) fn finish_task_loop(&self, task_id: &str) {
        let restart = {
            let mut cancellations = self.cancellations.lock().unwrap();
            crate::engine::lifecycle::control::retire(self, task_id);
            let end = cancellations.remove(task_id);
            if let Some(end) = &end {
                let _ = self.update_task(task_id, |task| {
                    end.apply(task);
                    task.interrupt_running_attempts(now_ms());
                });
            }
            end.is_some_and(|c| c.restart)
        };
        self.record_task_metrics(task_id);
        self.final_base_jobs.lock().unwrap().remove(task_id);
        if let Ok(Some(mut task)) = self.store.load_task(task_id) {
            let before = task.decisions.len();
            record_check_outcomes(self, &mut task, &[]);
            if task.decisions.len() != before {
                let lines = task.take_new_decisions(before);
                let _ = self.update_task(task_id, |t| record_check_decisions(t, lines));
            }
        }
        if restart {
            self.spawn_task_loop(task_id.to_string(), true);
        }
        if let Ok(Some(task)) = self.store.load_task(task_id) {
            self.notify_task_failed(&task);
            self.notify_task_unreviewed(&task);
            self.drop_settled_output(&task);
            self.advance_queue(&task.repo);
            self.advance_graph(&task.repo);
        }
        self.record_evolution_signals(task_id);
    }
}

impl TaskControl {
    pub(in crate::engine) fn spawn(
        app: std::sync::Arc<App>,
        task_id: String,
        auto_start_after_plan: bool,
    ) -> Self {
        let signals = TaskSignals::new();
        if let Ok(Some(task)) = app.store.load_task(&task_id) {
            *signals.pending_amend.lock().unwrap() = task.pending_amendment.clone();
        }
        let for_loop = signals.clone();
        let handle = tokio::spawn(async move {
            run_task_loop(app, task_id, for_loop, auto_start_after_plan).await;
        });
        Self { signals, handle }
    }
}
