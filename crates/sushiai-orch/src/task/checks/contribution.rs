use crate::task::Task;

/// Final checks a child contributes to its parent's combined tree.
pub(crate) struct ChildChecks<'a> {
    child: &'a str,
    commands: &'a [String],
}

impl<'a> ChildChecks<'a> {
    pub(crate) fn new(child: &'a str, commands: &'a [String]) -> Self {
        Self { child, commands }
    }

    pub(crate) fn append_to(self, task: &mut Task, now: i64) -> bool {
        let task = &mut task.data;
        let mut changed = false;
        for command in self.commands {
            let command = command.trim();
            if task
                .verify
                .iter()
                .chain(&task.final_verify)
                .any(|current| current.trim() == command)
            {
                continue;
            }
            task.final_verify.push(command.to_string());
            task.decisions.push(format!(
                "Orchestrator: added {command} to finalVerify for subtask {}",
                self.child
            ));
            changed = true;
        }
        if changed {
            task.updated_at = now;
        }
        changed
    }
}
