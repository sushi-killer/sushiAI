use super::{Task, TaskData, TaskStatus};

impl Task {
    /// Keeps a persisted stop or park when an in-flight loop saves an older copy.
    pub(crate) fn preserve_suspension(&self, stored: &TaskData) -> Option<Task> {
        let suspended = stored.queue.stopped_explicitly || stored.queue.park.is_some();
        let live = matches!(
            self.status,
            TaskStatus::Running | TaskStatus::Drafting | TaskStatus::Waiting | TaskStatus::Landing
        );
        if !suspended || !live || !matches!(stored.status, TaskStatus::Stopped | TaskStatus::Queued)
        {
            return None;
        }
        let mut copy = self.clone();
        let task = &mut copy.data;
        task.status = stored.status;
        task.question = None;
        task.queue.park = stored.queue.park.clone();
        task.queue.stopped_explicitly = stored.queue.stopped_explicitly;
        task.loop_state.agent_restarted |= stored.loop_state.agent_restarted;
        task.loop_state.non_owner_continues = task
            .loop_state
            .non_owner_continues
            .max(stored.loop_state.non_owner_continues);
        for part in &stored.loop_state.unreviewed_parts {
            if !task.loop_state.unreviewed_parts.contains(part) {
                task.loop_state.unreviewed_parts.push(part.clone());
            }
        }
        for line in &stored.decisions {
            if !task.decisions.contains(line) {
                task.decisions.push(line.clone());
            }
        }
        for answer in &stored.question_history {
            if !task.question_history.contains(answer) {
                task.question_history.push(answer.clone());
            }
        }
        Some(copy)
    }
}
