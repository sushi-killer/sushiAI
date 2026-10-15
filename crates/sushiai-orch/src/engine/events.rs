use crate::api::events::Event;
use crate::engine::state::App;
use crate::task::Task;
use tokio::sync::broadcast;
impl App {
    pub fn broadcast_task(&self, task: &Task) {
        let _ = self.events_tx.send(Event::Task {
            task: Box::new(task.clone()),
        });
    }
}

impl App {
    /// Appends one overseer action to `overseer.jsonl` and broadcasts it.
    pub(crate) fn record_overseer(&self, entry: crate::task::notes::overseer::Entry) {
        if crate::store::overseer::append(&self.data_dir, &entry).is_ok() {
            let _ = self.events_tx.send(Event::Overseer {
                entry: Box::new(entry),
            });
        }
    }
}

impl App {
    pub fn broadcast_log(&self, task_id: &str, attempt: u32, line: String) {
        let _ = self.events_tx.send(Event::Log {
            task_id: task_id.to_string(),
            attempt,
            line,
        });
    }
}

impl App {
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }
}
