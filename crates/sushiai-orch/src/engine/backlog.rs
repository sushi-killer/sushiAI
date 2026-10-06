//! The planning backlog: tasks parked in a `next` or `later` bucket until the
//! owner (`task.start`) or the autopilot starts them.

use super::*;

/// Who starts a task; decides the decision line recorded for a backlog task.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Starter {
    Owner,
    Autopilot,
}

/// `task.create`'s `backlog` parameter.
#[derive(Deserialize)]
pub(super) struct BacklogIn {
    pub(super) bucket: BacklogBucket,
    #[serde(default)]
    pub(super) order: Option<i64>,
}

impl App {
    /// A top-level task that has not implemented anything and has no live
    /// loop: the only kind that may enter, move within or leave the backlog.
    pub(super) fn unstarted(&self, task: &Task) -> bool {
        !task.archived
            && task.parent.is_none()
            && implement_attempt_count(task) == 0
            && !self.controls.lock().unwrap().contains_key(&task.id)
            && matches!(task.status, TaskStatus::Queued | TaskStatus::Stopped)
    }

    /// The backlog position for a task of `repo`: the given order, or one
    /// past the largest order among the repo's other tasks in the bucket.
    pub(super) fn backlog_slot(
        &self,
        repo: &str,
        except: Option<&str>,
        bucket: BacklogBucket,
        order: Option<i64>,
    ) -> Result<Backlog, String> {
        if let Some(order) = order {
            return Ok(Backlog { bucket, order });
        }
        let tasks = self.store.list_tasks().map_err(|e| e.to_string())?;
        let next = tasks
            .iter()
            .filter(|t| t.repo == repo && !t.archived && Some(t.id.as_str()) != except)
            .filter_map(|t| t.queue.backlog)
            .filter(|b| b.bucket == bucket)
            .map(|b| b.order)
            .max()
            .map_or(0, |max| max.saturating_add(1));
        Ok(Backlog {
            bucket,
            order: next,
        })
    }

    pub(super) async fn handle_task_backlog(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "task.backlog requires id".to_string())?
            .to_string();
        let bucket = match params.get("bucket") {
            None => return Err("task.backlog requires bucket (\"next\", \"later\" or null)".into()),
            Some(serde_json::Value::Null) => None,
            Some(v) => Some(
                serde_json::from_value::<BacklogBucket>(v.clone())
                    .map_err(|_| "bucket must be \"next\", \"later\" or null".to_string())?,
            ),
        };
        let order = match params.get("order") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(
                v.as_i64()
                    .ok_or_else(|| "order must be an integer".to_string())?,
            ),
        };
        validate_task_id(&self.store, &id)?;
        let task = {
            // Held so a start cannot slip in between the check and the save.
            let _admit = self.autopilot_admission.lock().unwrap();
            let mut task = self
                .store
                .load_task(&id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "task not found".to_string())?;
            if task.archived {
                return Err("task is archived".to_string());
            }
            if task.parent.is_some() {
                return Err("a subtask cannot be in the backlog".to_string());
            }
            if !self.unstarted(&task) {
                return Err(format!(
                    "task has already started or is running (status {})",
                    serde_json::to_value(task.status)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default()
                ));
            }
            task.queue.backlog = match bucket {
                Some(bucket) => Some(self.backlog_slot(&task.repo, Some(&id), bucket, order)?),
                None => None,
            };
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            task
        };
        let value = serde_json::to_value(&task).map_err(|e| e.to_string())?;
        self.advance_autopilot();
        Ok(value)
    }

    /// The one start path of `task.start` and the autopilot: a backlog task
    /// leaves the backlog with a decision line saying who started it.
    pub(super) fn start_task(&self, id: &str, by: Starter) {
        if let Ok(Some(mut task)) = self.store.load_task(id) {
            if task.queue.backlog.take().is_some() {
                task.decisions.push(
                    match by {
                        Starter::Owner => "Owner: started from the backlog",
                        Starter::Autopilot => "Autopilot: started from the next backlog",
                    }
                    .to_string(),
                );
                task.updated_at = now_ms();
                if self.store.save_task(&task).is_ok() {
                    self.broadcast_task(&task);
                }
            }
        }
        self.spawn_task_loop(id.to_string(), true);
    }

    /// Starts ready `next`-bucket tasks, lowest order first, while live task
    /// loops are below the parallel limit.
    pub(super) fn advance_autopilot(&self) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let (enabled, parallel) = {
            let settings = self.settings.read().unwrap();
            (settings.autopilot, settings.parallel)
        };
        if !enabled {
            return;
        }
        let limit = parallel.max(1).min(self.parallel_limit) as usize;
        let _admit = self.autopilot_admission.lock().unwrap();
        let all = self.store.list_tasks().unwrap_or_default();
        let mut next: Vec<&Task> = all
            .iter()
            .filter(|t| {
                t.queue
                    .backlog
                    .is_some_and(|b| b.bucket == BacklogBucket::Next)
                    && self.unstarted(t)
            })
            .collect();
        next.sort_by(|a, b| {
            let order = |t: &Task| t.queue.backlog.map_or(0, |b| b.order);
            (order(a), a.created_at, &a.id).cmp(&(order(b), b.created_at, &b.id))
        });
        for task in next {
            if self.controls.lock().unwrap().len() >= limit {
                break;
            }
            if task.status == TaskStatus::Drafting
                || task.question.is_some()
                || !matches!(own_waits_state(task, &all), Waits::Ready | Waits::Nothing)
            {
                continue;
            }
            self.start_task(&task.id, Starter::Autopilot);
        }
    }
}
