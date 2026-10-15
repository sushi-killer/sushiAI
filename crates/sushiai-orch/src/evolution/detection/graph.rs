use super::Detection;
use crate::task::evolution::signals::SignalKind;
use crate::task::messages::Message;
use crate::task::{FailureKind, Task, TaskStatus};

impl Detection<'_> {
    pub(super) fn graph(&mut self, repo_tasks: &[Task], messages: &[Message]) {
        let task = self.task;
        let siblings: Vec<&Task> = match &task.parent {
            Some(p) => repo_tasks
                .iter()
                .filter(|t| t.id != task.id && t.parent.as_deref() == Some(p.as_str()))
                .collect(),
            None => Vec::new(),
        };
        if task.parent.is_some() {
            for a in &task.attempts {
                let Some(f) = &a.failure else { continue };
                if f.kind == FailureKind::Verify && f.detail.contains("These files conflict:") {
                    self.source(a.n, None).record(
                        SignalKind::Graph,
                        "the subtask conflicted with files changed on its parent's branch"
                            .to_string(),
                        0,
                    );
                }
            }
        }
        let children: Vec<&Task> = repo_tasks
            .iter()
            .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
            .collect();
        if task.status == TaskStatus::Failed
            && !children.is_empty()
            && children.iter().all(|c| c.status == TaskStatus::Done)
        {
            self.source(0, None).record(
                SignalKind::Graph,
                "every subtask is done but the parent failed".to_string(),
                0,
            );
        }
        let undeclared: Vec<&Task> = siblings
            .into_iter()
            .filter(|s| !task.depends_on.contains(&s.id))
            .collect();
        if undeclared.is_empty() {
            return;
        }
        let mut sources: Vec<(u32, String)> = Vec::new();
        for a in &task.attempts {
            if let Some(f) = a
                .failure
                .as_ref()
                .filter(|f| f.kind == FailureKind::Blocked)
            {
                sources.push((a.n, f.detail.clone()));
            }
        }
        if let Some(q) = &task.question {
            sources.push((0, q.text.clone()));
        }
        for m in messages.iter().filter(|m| m.from == task.id) {
            sources.push((0, m.text.clone()));
        }
        for s in undeclared {
            let hit = sources.iter().find(|(_, text)| {
                text.contains(s.id.as_str()) || (!s.title.is_empty() && text.contains(&s.title))
            });
            if let Some((n, _)) = hit {
                let n = *n;
                self.source(n, None).record(
                    SignalKind::Graph,
                    format!(
                        "asked about sibling \"{}\" ({}) that it does not depend on",
                        s.title, s.id
                    ),
                    0,
                );
            }
        }
    }
}
