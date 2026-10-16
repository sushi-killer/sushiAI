use crate::api::validation::validate_task_id;
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::task::graph::has_cycle;
use crate::task::graph::wait_edges;
use crate::task::AnswerSource;
use crate::task::ParkRecord;
use crate::task::Task;
use crate::task::TaskStatus;
use serde::Deserialize;

use crate::task::queue::cancellation::Cancellation;

impl App {
    pub(super) fn is_suspended(&self, task: &Task) -> bool {
        task.queue.park.is_some()
            || task.queue.stopped_explicitly
            || self
                .cancellations
                .lock()
                .unwrap()
                .get(&task.id)
                .is_some_and(|c| !c.restart)
    }

    /// Saves the interruption before cancelling; queued waiters need not wait
    /// for the harness process or its loop to exit.
    pub(super) fn interrupt_task(
        &self,
        task: &mut Task,
        park: Option<ParkRecord>,
        explicit: bool,
        line: String,
    ) -> Result<(), String> {
        let mut cancellations = self.cancellations.lock().unwrap();
        let live = self.controls.lock().unwrap().contains_key(&task.id);
        task.interrupt(park.clone(), explicit, line.clone(), now_ms());
        {
            let lock = self.task_lock(&task.id);
            let _guard = lock.lock().unwrap();
            self.store.save_task(task).map_err(|e| e.to_string())?;
        }
        if live {
            cancellations.insert(
                task.id.clone(),
                Cancellation::new(park, explicit, line, task.question_history.last().cloned()),
            );
        }
        drop(cancellations);
        self.broadcast_task(task);
        if let Some(control) = self.controls.lock().unwrap().get(&task.id) {
            control.signals.cancel.cancel();
        }
        self.advance_queue(&task.repo);
        Ok(())
    }

    pub(super) fn apply_cancellation(&self, task: &mut Task) {
        if let Some(end) = self.cancellations.lock().unwrap().get(&task.id) {
            end.apply(task);
        } else {
            task.finish_cancellation(self.cancelled_status(&task.status));
        }
    }

    pub(super) async fn handle_task_park(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            reason: String,
            #[serde(default)]
            until: Option<String>,
            #[serde(default)]
            source: AnswerSource,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let reason = p.reason.trim();
        if reason.is_empty() {
            return Err("a park reason is required".into());
        }
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.archived || task.status == TaskStatus::Done || task.queue.stopped_explicitly {
            return Err("a done, archived or explicitly stopped task cannot be parked".into());
        }
        if let Some(until) = &p.until {
            validate_task_id(&self.store, until)?;
            let target = self
                .store
                .load_task(until)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "until task not found".to_string())?;
            if target.repo != task.repo {
                return Err("until must be a task in the same repo".into());
            }
            let all = self.repo_tasks(&task.repo);
            let mut edges = wait_edges(&all);
            for t in &all {
                if let Some(until) = t.queue.park.as_ref().and_then(|p| p.until.as_ref()) {
                    edges.entry(t.id.clone()).or_default().push(until.clone());
                }
            }
            edges
                .entry(task.id.clone())
                .or_default()
                .push(until.clone());
            if has_cycle(&edges) {
                return Err("park until would create a dependency cycle".into());
            }
        }
        let park = ParkRecord {
            reason: reason.to_string(),
            until: p.until,
            by: p.source,
        };
        self.interrupt_task(
            &mut task,
            Some(park),
            false,
            p.source.control_line("park", Some(reason)),
        )?;
        self.advance_graph(&task.repo);
        serde_json::to_value(task).map_err(|e| e.to_string())
    }
}
