use crate::clock::now_ms;
use crate::engine::answer_policy::authority::check_authority;
use crate::engine::state::App;

impl App {
    pub(in crate::engine) fn recover_pending_amendments(&self) -> std::io::Result<()> {
        for listed in self.store.list_tasks()? {
            if listed.pending_amendment.is_none() {
                continue;
            }
            if let Err(error) = self.recover_pending_amendment(&listed.id) {
                eprintln!(
                    "orch: recovering pending amendment for {} failed: {error}",
                    listed.id
                );
            }
        }
        Ok(())
    }

    fn recover_pending_amendment(&self, id: &str) -> std::io::Result<()> {
        let lock = self.task_lock(id);
        let _guard = lock.lock().unwrap();
        let Some(mut task) = self.store.load_task(id)? else {
            return Ok(());
        };
        let Some(receipt) = task.pending_amendment.clone() else {
            return Ok(());
        };
        let authority = check_authority(&self.settings.read().unwrap(), &task);
        task.settle_pending_amendment(&authority, now_ms());
        self.store.save_task_with_pending(&task, Some(&receipt))?;
        if receipt.verify.is_some() {
            self.verify_cache.lock().unwrap().remove(&task.id);
        }
        self.broadcast_task(&task);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
