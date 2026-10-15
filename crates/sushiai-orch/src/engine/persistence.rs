use crate::clock::now_ms;
use crate::engine::state::App;
use crate::engine::verification::context::record_check_decisions;
use crate::task::Task;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
/// Saves a typed observation as soon as a running harness reports it.
pub(super) async fn persist_attempt_observation(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    observation: crate::task::attempts::RunObservation,
) {
    if let Err(e) = app.update_task(task_id, |task| {
        crate::task::attempts::observe_run(task, attempt_n, observation, now_ms());
    }) {
        eprintln!("orch: updating task {task_id} attempt {attempt_n} failed: {e}");
    }
}

impl App {
    pub(super) fn task_lock(&self, id: &str) -> Arc<StdMutex<()>> {
        self.task_locks
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }
}

impl App {
    pub(crate) fn save_task_logged(&self, task: &Task) {
        let lock = self.task_lock(&task.id);
        let _guard = lock.lock().unwrap();
        let kept;
        let mut task = task;
        if let Ok(Some(stored)) = self.store.load_task(&task.id) {
            if let Some(copy) = task.preserve_suspension(&stored) {
                kept = copy;
                task = &kept;
            }
        }
        if let Err(e) = self.store.save_task(task) {
            eprintln!("orch: saving task {} failed: {e}", task.id);
        }
    }
}

impl App {
    pub(crate) fn update_task(&self, id: &str, f: impl FnOnce(&mut Task)) -> Result<Task, String> {
        self.update_task_checked(id, |task| {
            f(task);
            Ok(())
        })
    }
}

impl App {
    pub(super) fn update_task_checked(
        &self,
        id: &str,
        f: impl FnOnce(&mut Task) -> Result<(), String>,
    ) -> Result<Task, String> {
        let lock = self.task_lock(id);
        let _guard = lock.lock().unwrap();
        let mut task = self
            .store
            .load_task(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        f(&mut task)?;
        task.touch(now_ms());
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        Ok(task)
    }
}

impl App {
    /// Appends a line to a task's `decisions` and persists it right away --
    /// for call sites that don't otherwise hold the task in memory. Call sites that
    /// already hold `&mut Task` push the line onto their own copy instead, so
    /// it rides along with their next save instead of racing a reload.
    pub(super) fn append_decision(&self, task_id: &str, line: String) {
        if let Err(e) = self.update_task(task_id, |task| record_check_decisions(task, vec![line])) {
            eprintln!("orch: updating task {task_id} failed: {e}");
        }
    }
}
