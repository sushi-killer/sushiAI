use super::*;

impl App {
    /// spec step 10: attempts left `running` from a previous process
    /// become `interrupted`; their tasks become `stopped`.
    /// Every task that was in flight when the daemon stopped gets its loop
    /// back: interrupted attempts are marked and requeued by the store, and
    /// queued or drafting tasks simply resume. A `waiting` task needs no loop
    /// until its answer arrives (`task.answer` relaunches one).
    pub fn recover_on_start(&self) -> std::io::Result<()> {
        let prices = self.settings.read().unwrap().prices.clone();
        let recovered = self.store.recover_interrupted(|task, idx| {
            let run_dir = self.store.run_dir(&task.id, task.attempts[idx].n);
            settle_unfinished_cost(task, idx, &run_dir, &prices);
        })?;
        for t in recovered {
            self.broadcast_task(&t);
        }
        for a in self.store.recover_interrupted_audits()? {
            audit::broadcast(self, &a);
        }
        for mut t in self.store.list_tasks()? {
            if t.archived {
                continue;
            }
            if matches!(
                t.status,
                TaskStatus::Queued | TaskStatus::Running | TaskStatus::Drafting
            ) {
                let id = t.id.clone();
                if settle_interrupted_advisor(&mut t, |n| self.store.run_dir(&id, n), &prices) {
                    t.updated_at = now_ms();
                    self.store.save_task(&t)?;
                    self.broadcast_task(&t);
                }
                self.start_task_loop(t.id);
            }
        }
        Ok(())
    }
}
