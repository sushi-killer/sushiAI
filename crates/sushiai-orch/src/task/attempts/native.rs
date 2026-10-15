use crate::task::{Attempt, Task};

/// A native invocation's selection spend that has not reached the task store yet.
pub(crate) struct Settlement {
    task_id: String,
    started: Attempt,
    selection_cost: f64,
    discarded: Vec<String>,
}

impl Settlement {
    pub(crate) fn capture(
        task: &Task,
        index: usize,
        selection_cost: f64,
        discarded: &[String],
    ) -> Result<Self, String> {
        let started = task
            .attempts
            .get(index)
            .ok_or_else(|| "native attempt not found".to_string())?
            .clone();
        Ok(Self {
            task_id: task.id.clone(),
            started,
            selection_cost,
            discarded: discarded.to_vec(),
        })
    }

    pub(crate) fn observation<'a>(&self, task: &'a Task) -> Result<&'a Attempt, String> {
        if task.id != self.task_id {
            return Err("native receipt belongs to another task".to_string());
        }
        task.attempts
            .iter()
            .find(|attempt| same_run(attempt, &self.started))
            .ok_or_else(|| "native attempt was replaced".to_string())
    }

    pub(crate) fn merge(&self, task: &mut Task, observed: &Attempt) -> Result<(), String> {
        if task.id != self.task_id || !same_run(observed, &self.started) {
            return Err("native observation belongs to another attempt".to_string());
        }
        let saved = task
            .data
            .attempts
            .iter_mut()
            .find(|attempt| same_run(attempt, &self.started))
            .ok_or_else(|| "native attempt was replaced".to_string())?;
        if !saved.candidates.is_empty() && saved.candidates != observed.candidates {
            return Err("native selection already changed".to_string());
        }
        let selection_cost = if saved.candidates.is_empty() {
            self.selection_cost
        } else {
            0.0
        };
        let old_cost = saved.cost_usd.unwrap_or(0.0);
        let known_cost = saved.cost_usd;
        let known_estimated = saved.cost_estimated;
        copy_native(saved, observed);
        if observed.cost_usd.is_none() {
            saved.cost_usd = known_cost;
            saved.cost_estimated = known_estimated;
        }
        task.data.cost_usd += saved.cost_usd.unwrap_or(0.0) - old_cost + selection_cost;
        if !self.discarded.is_empty() {
            task.data
                .planner_tests
                .discarded
                .extend(self.discarded.iter().cloned());
            task.data.planner_tests.discarded.sort();
            task.data.planner_tests.discarded.dedup();
        }
        Ok(())
    }

    pub(crate) fn accept(&mut self, task: &mut Task, saved: &Task) -> Result<(), String> {
        let receipt = self.observation(saved)?;
        let local = task
            .data
            .attempts
            .iter_mut()
            .find(|attempt| same_run(attempt, &self.started))
            .ok_or_else(|| "native attempt was replaced".to_string())?;
        copy_native(local, receipt);
        task.data.cost_usd = saved.cost_usd;
        task.data
            .planner_tests
            .discarded
            .clone_from(&saved.planner_tests.discarded);
        self.selection_cost = 0.0;
        self.discarded.clear();
        Ok(())
    }
}

fn same_run(left: &Attempt, right: &Attempt) -> bool {
    left.n == right.n && left.stage == right.stage && left.started_at == right.started_at
}

fn copy_native(target: &mut Attempt, source: &Attempt) {
    target.route_id.clone_from(&source.route_id);
    target.harness = source.harness;
    target.model.clone_from(&source.model);
    target.reason.clone_from(&source.reason);
    target.session_id.clone_from(&source.session_id);
    target.fingerprint.clone_from(&source.fingerprint);
    target.usage.clone_from(&source.usage);
    target.prefix_tokens = source.prefix_tokens;
    target.cost_usd = source.cost_usd;
    target.cost_estimated = source.cost_estimated;
    target.gate_blocks = source.gate_blocks;
    target.candidates.clone_from(&source.candidates);
}

#[cfg(test)]
mod tests;
